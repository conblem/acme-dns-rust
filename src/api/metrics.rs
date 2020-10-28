use lazy_static::lazy_static;
use prometheus::{
    register_histogram_vec, register_int_counter_vec, Encoder, HistogramTimer, HistogramVec,
    IntCounterVec, TextEncoder,
};
use tracing::error;
use warp::filters::trace;
use warp::http::header::{HeaderValue, CONTENT_TYPE};
use warp::http::{Method, Response};
use warp::path::FullPath;
use warp::reply::Response as WarpResponse;
use warp::{Filter, Rejection, Reply};

//todo: dont use int maybe
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

pub(super) fn metrics_wrapper<R, I>(config: impl Into<Option<&'static str>>) -> impl MetricFn<R, I>
where
    R: Reply,
    I: Filter<Extract = (R,), Error = Rejection> + Clone + Send + Sync + 'static,
{
    let config = config.into().unwrap_or("");

    move |filter: I| {
        warp::filters::path::full()
            .and(warp::filters::method::method())
            .map(move |path: FullPath, method: Method| {
                let path = match config {
                    "" => path.as_str().to_string(),
                    config => config.to_string(),
                };
                let timer = HTTP_REQ_HISTOGRAM
                    .with_label_values(&[&path, method.as_str()])
                    .start_timer();

                (path, method, timer)
            })
            .untuple_one()
            .and(filter)
            .map(
                |path: String, method: Method, timer: HistogramTimer, res: R| {
                    let res = res.into_response();
                    HTTP_STATUS_COUNTER
                        .with_label_values(&[&path, method.as_str(), res.status().as_str()])
                        .inc();

                    timer.observe_duration();

                    res
                },
            )
            .with(trace::request())
            .map(Reply::into_response)
    }
}

fn request() -> impl Reply {
    let encoder = TextEncoder::new();
    let families = prometheus::gather();
    let mut res = vec![];
    if let Err(e) = encoder.encode(&families, &mut res) {
        error!("{}", e);
        return Response::builder()
            .status(500)
            .body(e.to_string())
            .into_response();
    }

    Response::builder()
        .header(CONTENT_TYPE, HeaderValue::from_static("text/plain"))
        .body(res)
        .into_response()
}

pub(super) fn metrics(
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone + Send + 'static {
    warp::path("metrics")
        .and(warp::get())
        .map(request)
        .with(warp::wrap_fn(metrics_wrapper(None)))
}
