use async_trait::async_trait;
use once_cell::sync::Lazy;
use prometheus::{register_histogram_vec, HistogramVec};
use tracing::{info_span, Instrument, Span};
use trust_dns_server::authority::Catalog;
use trust_dns_server::server::{Request, RequestHandler, ResponseHandler, ResponseInfo};

static DNS_REQ_HISTOGRAM: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "dns_request_duration_seconds",
        "The DNS request latencies in seconds.",
        &["name"]
    )
    .unwrap()
});

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
        let query = info.query;

        let name = query.name().to_string();
        let addr = info.src;
        let query_type = query.query_type();
        let id = info.header.id();

        let span =
            info_span!(parent: &self.span, "request", remote.addr = %addr, %name, %query_type, %id);

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
