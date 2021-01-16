use futures_util::stream;
use hyper::Body;
use lazy_static::lazy_static;
use prometheus::{
    register_histogram_vec, register_int_counter_vec, Encoder, HistogramTimer, HistogramVec,
    IntCounterVec, TextEncoder,
};
use std::io::{BufRead, Cursor};
use tracing::{debug, error};
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
    mut timer: HistogramTimerWrapper,
    res: impl Reply,
) -> WarpResponse {
    let res = res.into_response();

    HTTP_STATUS_COUNTER
        .with_label_values(&[config.as_str(), method.as_str(), res.status().as_str()])
        .inc();

    if let Some(timer) = timer.take() {
        let time = timer.stop_and_record() * 1000f64;
        debug!("request took {}ms", time);
    }

    res
}

pub(super) fn metrics_wrapper<R, I, C>(config: C) -> impl MetricFn<R, I>
where
    R: Reply + 'static,
    I: Filter<Extract = (R,), Error = Rejection> + Clone + Send + Sync + 'static,
    C: Into<Option<&'static str>>,
{
    let config = match config.into() {
        Some("") => panic!("Empty str not allowed as param"),
        Some(config) => config,
        None => "",
    };

    move |filter: I| {
        warp::filters::path::full()
            .and(warp::filters::method::method())
            .map(move |path: FullPath, method: Method| {
                let config = MetricsConfig::new(config, path);
                let timer = HTTP_REQ_HISTOGRAM
                    .with_label_values(&[config.as_str(), method.as_str()])
                    .start_timer()
                    .into();

                (config, method, timer)
            })
            .untuple_one()
            .and(filter)
            .map(stop_metrics)
            .with(trace::request())
            .map(Reply::into_response)
    }
}

const TEXT_PLAIN_MIME: &str = "text/plain";

// maybe this implementation is wrong as it removes bucket items aswell
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

    // we keep the error in the stream so it can later be handled by warp
    let stream = Cursor::new(res).lines().filter_map(|line| match line {
        Ok(line) if !line.ends_with(" 0") => Some(Ok(line + "\n")),
        Err(error) => Some(Err(error)),
        _ => None,
    });

    Response::builder()
        .header(CONTENT_TYPE, HeaderValue::from_static(TEXT_PLAIN_MIME))
        .body(Body::wrap_stream(stream::iter(stream)))
        .into_response()
}

const METRICS_PATH: &str = "metrics";

pub(super) fn metrics(
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone + Send + 'static {
    warp::path(METRICS_PATH)
        .and(warp::get())
        .map(metrics_handler)
        .with(warp::wrap_fn(metrics_wrapper(None)))
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
