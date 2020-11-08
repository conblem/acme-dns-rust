use lazy_static::lazy_static;
use prometheus::{
    register_histogram_vec, register_int_counter_vec, Encoder, HistogramTimer, HistogramVec,
    IntCounterVec, TextEncoder,
};
use tracing::error;
use warp::filters::trace;
use warp::http::header::{HeaderValue, CONTENT_TYPE};
use warp::http::{Method, Response, StatusCode};
use warp::path::FullPath;
use warp::reply::Response as WarpResponse;
use warp::{Filter, Rejection, Reply};

use self::MetricsConfig::{Borrowed, Owned};

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

enum MetricsConfig {
    Borrowed(&'static str),
    Owned(FullPath),
}

impl MetricsConfig {
    fn new(config: &'static str, path: FullPath) -> MetricsConfig {
        match config {
            "" => Owned(path),
            config => Borrowed(config),
        }
    }

    fn as_str(&self) -> &str {
        match &self {
            Borrowed(config) => config,
            Owned(path) => path.as_str(),
        }
    }
}

pub(super) trait MetricFn<R, I>: Fn(I) -> <Self as MetricFn<R, I>>::Output
where
    R: Reply,
    I: Filter<Extract = (R,), Error = Rejection>,
{
    type Output: Filter<Extract = (WarpResponse,), Error = Rejection>
        + Clone
        + Send
        + Sync
        + 'static;
}

impl<R, I, O, F> MetricFn<R, I> for F
where
    R: Reply,
    I: Filter<Extract = (R,), Error = Rejection> + Clone + Send + Sync + 'static,
    O: Filter<Extract = (WarpResponse,), Error = Rejection> + Clone + Send + Sync + 'static,
    F: Fn(I) -> O,
{
    type Output = O;
}

fn stop_metrics(
    config: MetricsConfig,
    method: Method,
    timer: HistogramTimer,
    res: impl Reply,
) -> WarpResponse {
    let res = res.into_response();
    HTTP_STATUS_COUNTER
        .with_label_values(&[config.as_str(), method.as_str(), res.status().as_str()])
        .inc();

    timer.observe_duration();

    res
}

pub(super) fn metrics_wrapper<R, I, C>(config: C) -> impl MetricFn<R, I>
where
    R: Reply + 'static,
    I: Filter<Extract = (R,), Error = Rejection> + Clone + Send + Sync + 'static,
    C: Into<Option<&'static str>>,
{
    let config = config.into().unwrap_or("");

    move |filter: I| {
        warp::filters::path::full()
            .and(warp::filters::method::method())
            .map(move |path: FullPath, method: Method| {
                let config = MetricsConfig::new(config, path);
                let timer = HTTP_REQ_HISTOGRAM
                    .with_label_values(&[config.as_str(), method.as_str()])
                    .start_timer();

                (config, method, timer)
            })
            .untuple_one()
            .and(filter)
            .map(stop_metrics)
            .with(trace::request())
            .map(Reply::into_response)
    }
}

const TEXT_PLAIN_MIME: &'static str = "text/plain";

fn metrics_handler() -> impl Reply {
    let encoder = TextEncoder::new();
    let families = prometheus::gather();
    let mut res = vec![];
    if let Err(e) = encoder.encode(&families, &mut res) {
        error!("{}", e);
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(e.to_string())
            .into_response();
    }

    Response::builder()
        .header(CONTENT_TYPE, HeaderValue::from_static(TEXT_PLAIN_MIME))
        .body(res)
        .into_response()
}

const METRICS_PATH: &'static str = "metrics";

pub(super) fn metrics(
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone + Send + 'static {
    warp::path(METRICS_PATH)
        .and(warp::get())
        .map(metrics_handler)
        .with(warp::wrap_fn(metrics_wrapper(None)))
}
