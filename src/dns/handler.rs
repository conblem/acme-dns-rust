use tracing::field::Empty;
use tracing::{info_span, Span};
use tracing_futures::{Instrument, Instrumented};
use trust_dns_server::authority::Catalog;
use trust_dns_server::server::Request;
use trust_dns_server::server::{RequestHandler, ResponseHandler};

pub(super) struct TraceRequestHandler {
    catalog: Catalog,
    span: Span,
}

impl TraceRequestHandler {
    pub(super) fn new(catalog: Catalog, span: Span) -> Self {
        TraceRequestHandler { catalog, span }
    }
}

impl RequestHandler for TraceRequestHandler {
    type ResponseFuture = Instrumented<<Catalog as RequestHandler>::ResponseFuture>;

    fn handle_request<R: ResponseHandler>(
        &self,
        request: Request,
        response_handle: R,
    ) -> Self::ResponseFuture {
        let addr = request.src;
        self.catalog
            .handle_request(request, response_handle)
            .instrument(info_span!(parent: &self.span, "request", remote.addr = %addr, name = Empty, query_type = Empty))
    }
}
