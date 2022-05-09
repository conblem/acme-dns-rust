use async_trait::async_trait;
use futures_util::future::{Join, Map, Ready};
use futures_util::{future, FutureExt};
use lazy_static::lazy_static;
use prometheus::{register_histogram_vec, HistogramTimer, HistogramVec};
use std::future::Future;
use tracing::field::Empty;
use tracing::instrument::Instrumented;
use tracing::{info_span, Instrument, Span};
use trust_dns_server::authority::Catalog;
use trust_dns_server::client::op::LowerQuery;
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
        /*let name = request
            .message
            .queries()
            .first()
            .map(LowerQuery::name)
            .map(ToString::to_string);

        let name = name.as_ref().map(|name| [name.as_str()]);

        let name = match &name {
            Some(name) => &name[..],
            None => &[],
        };

        let addr = request.src;
        let span = info_span!(parent: &self.span, "request", remote.addr = %addr, name = Empty, query_type = Empty);

        let timer = DNS_REQ_HISTOGRAM.with_label_values(name).start_timer();
        let handle_request = self.catalog.handle_request(&request, response_handle);

        */

        self.catalog.handle_request(&request, response_handle).await
    }
}
