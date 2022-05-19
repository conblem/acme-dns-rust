use once_cell::sync::Lazy;
use prometheus::{
    register_histogram_vec, register_int_counter_vec, Encoder, HistogramVec, IntCounterVec,
    Registry, TextEncoder,
};
use std::convert::Infallible;
use std::fmt::Display;
use std::io::{BufRead, Error as IoError};
use tokio::time::Instant;
use tracing::{debug, error};
use warp::filters::trace;
use warp::http::{Method, Response, StatusCode};
use warp::path::FullPath;
use warp::reply::Response as WarpResponse;
use warp::{Filter, Rejection, Reply};

static HTTP_STATUS_COUNTER: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "http_status_counter",
        "The HTTP requests status",
        &["path", "method", "status"]
    )
    .unwrap()
});
static HTTP_REQ_HISTOGRAM: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "http_request_duration_seconds",
        "The HTTP request latencies in seconds.",
        &["path", "method"]
    )
    .unwrap()
});

pub(crate) enum MetricsConfig {
    Borrowed(&'static str),
    Owned(FullPath),
}

impl MetricsConfig {
    pub(super) fn new(
        config: &'static str,
    ) -> impl Filter<Extract = (MetricsConfig,), Error = Infallible> + Clone + Send + 'static {
        warp::any().map(move || MetricsConfig::Borrowed(config))
    }

    pub(super) fn path(
    ) -> impl Filter<Extract = (MetricsConfig,), Error = Infallible> + Clone + Send + 'static {
        warp::filters::path::full().map(MetricsConfig::Owned)
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Borrowed(config) => config,
            Self::Owned(path) => path.as_str(),
        }
    }
}

// type magic to return a closure which returns a unnameable warp filter
trait MetricsWrapper<I>: Fn(I) -> <Self as MetricsWrapper<I>>::Output
where
    I: Filter<Extract = (WarpResponse, MetricsConfig), Error = Rejection> + Clone + Send + 'static,
{
    type Output: Filter<Extract = (WarpResponse,), Error = Rejection> + Clone + Send + 'static;
}

impl<F, I, O> MetricsWrapper<I> for F
where
    F: Fn(I) -> O,
    I: Filter<Extract = (WarpResponse, MetricsConfig), Error = Rejection> + Clone + Send + 'static,
    O: Filter<Extract = (WarpResponse,), Error = Rejection> + Clone + Send + 'static,
{
    type Output = O;
}

fn metrics_wrapper_higher<I>(
    http_req_histogram: &'static HistogramVec,
    http_status_counter: &'static IntCounterVec,
) -> impl MetricsWrapper<I>
where
    I: Filter<Extract = (WarpResponse, MetricsConfig), Error = Rejection> + Clone + Send + 'static,
{
    move |filter: I| {
        warp::any()
            .map(Instant::now)
            .and(warp::filters::method::method())
            .and(filter)
            .map(
                move |timer: Instant, method: Method, res: WarpResponse, config: MetricsConfig| {
                    http_status_counter
                        .with_label_values(&[
                            config.as_str(),
                            method.as_str(),
                            res.status().as_str(),
                        ])
                        .inc();

                    let time = timer.elapsed().as_secs_f64();
                    http_req_histogram
                        .with_label_values(&[config.as_str(), method.as_str()])
                        .observe(time);

                    debug!("request took {}ms", time * 1000f64);

                    res
                },
            )
    }
}

pub(crate) fn metrics_wrapper<F>(
    filter: F,
) -> impl Filter<Extract = (WarpResponse,), Error = Rejection> + Clone + Send + 'static
where
    F: Filter<Extract = (WarpResponse, MetricsConfig), Error = Rejection> + Clone + Send + 'static,
{
    metrics_wrapper_higher(&*HTTP_REQ_HISTOGRAM, &*HTTP_STATUS_COUNTER)(filter)
}

fn internal_server_error_and_trace<E: Display>(error: &E) -> WarpResponse {
    error!("{}", error);
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(error.to_string())
        .into_response()
}

// we could use a string here and read_line but this would require checking for utf8
fn remove_zero_metrics(mut data: &[u8]) -> Result<Vec<u8>, IoError> {
    let mut res = Vec::with_capacity(data.len());
    loop {
        let len = match data.read_until(b'\n', &mut res) {
            Ok(0) => break Ok(res),
            Ok(len) => len,
            Err(e) => break Err(e),
        };
        if res.ends_with(b" 0\n") | res.ends_with(b" 0") {
            res.truncate(res.len() - len);
        }
    }
}

// maybe this implementation is wrong as it removes bucket items aswell
// this method leaks internal errors so it should not be public
fn metrics_handler(registry: &Registry) -> WarpResponse {
    let encoder = TextEncoder::new();
    let families = registry.gather();

    let mut res = vec![];
    if let Err(e) = encoder.encode(&families, &mut res) {
        return internal_server_error_and_trace(&e);
    }

    let res = match remove_zero_metrics(&res[..]) {
        Ok(res) => res,
        Err(e) => return internal_server_error_and_trace(&e),
    };

    Response::new(res).into_response()
}

const METRICS_PATH: &str = "metrics";

pub(crate) fn metrics(
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone + Send + 'static {
    warp::path(METRICS_PATH)
        .and(warp::get())
        .map(|| metrics_handler(prometheus::default_registry()))
        .and(MetricsConfig::path())
        .with(warp::wrap_fn(metrics_wrapper))
        .with(trace::request())
}

#[cfg(test)]
mod tests {
    use hyper::body;
    use tracing_test::traced_test;
    use warp::{test, Filter};

    use super::{internal_server_error_and_trace, remove_zero_metrics, MetricsConfig};

    #[test]
    fn test_remove_zero_metrics() {
        let actual = r#"tcp_closed_connection_counter{endpoint="PROM"} 0
http_status_counter{method="GET",path="/metrics",status="200"} 4
tcp_open_connection_counter{endpoint="PROM"} 0"#;

        let expected = r#"http_status_counter{method="GET",path="/metrics",status="200"} 4
"#;
        let actual = remove_zero_metrics(actual.as_bytes()).unwrap();
        assert_eq!(expected.as_bytes(), &actual[..]);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_internal_server_error_and_trace() {
        let error = "This is a error".to_owned();

        let actual = internal_server_error_and_trace(&error);
        assert!(logs_contain("This is a error"));

        let body = body::to_bytes(actual).await.unwrap();
        assert_eq!("This is a error", body);
    }

    #[tokio::test]
    async fn metrics_config_path() {
        let filter = MetricsConfig::path().map(|config: MetricsConfig| config.as_str().to_owned());

        let actual = test::request().path("/test").reply(&filter).await;

        assert_eq!("/test", actual.body())
    }

    #[tokio::test]
    async fn metrics_config_new() {
        let filter =
            MetricsConfig::new("404").map(|config: MetricsConfig| config.as_str().to_owned());

        let actual = test::request().path("/test").reply(&filter).await;

        assert_eq!("404", actual.body())
    }
}
