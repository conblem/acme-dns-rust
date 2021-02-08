use lazy_static::lazy_static;
use prometheus::{
    register_histogram_vec, register_int_counter_vec, Encoder, HistogramTimer, HistogramVec,
    IntCounterVec, TextEncoder,
};
use std::convert::Infallible;
use std::fmt::Display;
use std::io::{BufRead, Error as IoError};
use tracing::{debug, error};
use warp::filters::trace;
use warp::http::{Method, Response, StatusCode};
use warp::path::FullPath;
use warp::reply::Response as WarpResponse;
use warp::{Filter, Rejection, Reply};

lazy_static! {
    static ref HTTP_STATUS_COUNTER: IntCounterVec = register_int_counter_vec!(
        "http_status_counter",
        "The HTTP requests status",
        &["path", "method", "status"]
    )
    .unwrap();
    static ref HTTP_REQ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        "http_request_duration_seconds",
        "The HTTP request latencies in seconds.",
        &["path", "method"]
    )
    .unwrap();
}

const fn test() {}

pub(super) enum MetricsConfig {
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

fn hihi<I>(
    http_req_histogram: &'static HistogramVec,
    http_status_counter: &'static IntCounterVec,
) -> impl MetricsWrapper<I>
where
    I: Filter<Extract = (WarpResponse, MetricsConfig), Error = Rejection> + Clone + Send + 'static,
{
    move |filter: I| {
        warp::filters::method::method()
            .map(move |method| {
                let timer = http_req_histogram
                    .with_label_values(&["", ""])
                    .start_timer()
                    .into();
                (method, timer)
            })
            .untuple_one()
            .and(filter)
            .map(
                move |method: Method,
                      mut timer: HistogramTimerWrapper,
                      res: WarpResponse,
                      config: MetricsConfig| {
                    http_status_counter
                        .with_label_values(&[
                            config.as_str(),
                            method.as_str(),
                            res.status().as_str(),
                        ])
                        .inc();

                    if let Some(timer) = timer.take() {
                        let time = timer.stop_and_discard();

                        http_req_histogram
                            .with_label_values(&[config.as_str(), method.as_str()])
                            .observe(time);

                        debug!("request took {}ms", time * 1000f64);
                    }

                    res
                },
            )
    }
}

pub(super) fn metrics_wrapper<F>(
    filter: F,
) -> impl Filter<Extract = (WarpResponse,), Error = Rejection> + Clone + Send + 'static
where
    F: Filter<Extract = (WarpResponse, MetricsConfig), Error = Rejection> + Clone + Send + 'static,
{
    hihi(&*HTTP_REQ_HISTOGRAM, &*HTTP_STATUS_COUNTER)(filter)
}

fn internal_server_error_and_trace<E: Display>(error: &E) -> WarpResponse {
    error!("{}", error);
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(error.to_string())
        .into_response()
}

fn remove_zero_metrics(mut data: &[u8]) -> Result<String, IoError> {
    let mut res = String::with_capacity(data.len());
    loop {
        let len = match data.read_line(&mut res) {
            Ok(0) => break Ok(res),
            Ok(len) => len,
            Err(e) => break Err(e),
        };
        if res.ends_with(" 0\n") {
            res.truncate(res.len() - len);
        }
    }
}

// maybe this implementation is wrong as it removes bucket items aswell
// this method leaks internal errors so it should not be public
fn metrics_handler() -> WarpResponse {
    let encoder = TextEncoder::new();
    let families = prometheus::gather();

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

pub(super) fn metrics(
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone + Send + 'static {
    warp::path(METRICS_PATH)
        .and(warp::get())
        .map(metrics_handler)
        .and(MetricsConfig::path())
        .with(warp::wrap_fn(metrics_wrapper))
        .with(trace::request())
}

// Changes the default behaviour of HistogramTimer so it doesn't record the value if it is being dropped
struct HistogramTimerWrapper(Option<HistogramTimer>);

impl From<HistogramTimer> for HistogramTimerWrapper {
    fn from(input: HistogramTimer) -> Self {
        HistogramTimerWrapper(Some(input))
    }
}

impl Drop for HistogramTimerWrapper {
    fn drop(&mut self) {
        if let Some(histogram_timer) = self.take() {
            let time = histogram_timer.stop_and_discard() * 1000f64;
            debug!("rejection took {}ms", time);
        }
    }
}

impl HistogramTimerWrapper {
    fn take(&mut self) -> Option<HistogramTimer> {
        self.0.take()
    }
}
