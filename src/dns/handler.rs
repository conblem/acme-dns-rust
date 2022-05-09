use async_trait::async_trait;
use lazy_static::lazy_static;
use prometheus::{register_histogram_vec, HistogramVec};
use tracing::{info_span, Instrument, Span};
use trust_dns_server::authority::Catalog;
use trust_dns_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};

lazy_static! {
    static ref DNS_REQ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        "dns_request_duration_seconds",
        "The DNS request latencies in seconds.",
        &["name"]
    )
    .unwrap();
}

pub(super) struct TraceRequestHandler {
    catalog: Catalog,
    span: Span,
}

impl TraceRequestHandler {
    pub(super) fn new(catalog: Catalog, span: Span) -> Self {
        TraceRequestHandler { catalog, span }
    }
}

// todo: add tracing back
#[async_trait]
impl RequestHandler for TraceRequestHandler {
    async fn handle_request<R: ResponseHandler>(
        &self,
        request: &Request,
        response_handle: R,
    ) -> ResponseInfo {
        let info = request.request_info();
        let name = info.query.name().to_string();
        let addr = info.src;
        let query_type = info.query.query_type();

        let span =
            info_span!(parent: &self.span, "request", remote.addr = %addr, %name, %query_type);

        let timer = DNS_REQ_HISTOGRAM
            .with_label_values(&[name.as_str()])
            .start_timer();

        let res = self
            .catalog
            .handle_request(request, response_handle)
            .instrument(span)
            .await;

        timer.observe_duration();

        res
    }
}
