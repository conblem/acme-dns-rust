use lazy_static::lazy_static;
use prometheus::{register_histogram_vec, Encoder, HistogramVec, TextEncoder, HistogramTimer};
use tracing::error;
use warp::{http::Response, Filter, Rejection, Reply};
use warp::filters::path::FullPath;

lazy_static! {
    static ref HTTP_REQ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        "http_request_duration_seconds",
        "The HTTP request latencies in seconds.",
        &["path"]
    )
    .unwrap();
}

pub(super) fn metrics_wrapper<R: Reply>(
    filter: impl Filter<Extract = (R,), Error = Rejection> + Clone + Send + 'static,
) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone + Send + 'static {
    warp::path::full()
        .map(|path: FullPath| {
            HTTP_REQ_HISTOGRAM.with_label_values(&[path.as_str()]).start_timer()
        })
        .and(filter)
        .map(|timer: HistogramTimer, res: R| {
            timer.observe_duration();
            //let res = res.into_response();
            res
        })
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
        .header("Content-Type", "text/plain")
        .body(res)
        .into_response()
}

pub(super) fn metrics(
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone + Send + 'static {
    warp::path("metrics")
        .and(warp::get())
        .map(request)
        .with(warp::trace::request())
        .with(warp::wrap_fn(metrics_wrapper))
}
