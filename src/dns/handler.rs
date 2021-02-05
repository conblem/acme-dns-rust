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
use trust_dns_server::server::{Request, RequestHandler, ResponseHandler};

lazy_static! {
    static ref DNS_REQ_HISTOGRAM: HistogramVec = register_histogram_vec!(
        "dns_request_duration_seconds",
        "The DNS request latencies in seconds.",
        &["name"]
    )
    .unwrap();
}

type ResponseFuture = <Catalog as RequestHandler>::ResponseFuture;
type ResponseFutureOutput = <ResponseFuture as Future>::Output;

type EndTimer = fn((ResponseFutureOutput, HistogramTimer)) -> ResponseFutureOutput;

type JoinedFuture = Join<ResponseFuture, Ready<HistogramTimer>>;
type MappedFuture = Instrumented<Map<JoinedFuture, EndTimer>>;

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
    type ResponseFuture = MappedFuture;

    fn handle_request<R: ResponseHandler>(
        &self,
        request: Request,
        response_handle: R,
    ) -> Self::ResponseFuture {
        let name = request
            .message
            .queries()
            .first()
            .map(LowerQuery::name)
            .map(ToString::to_string);

        let name = match &name {
            Some(name) => Some([name.as_str()]),
            None => None,
        };

        let name = match &name {
            Some(name) => &name[..],
            None => &[],
        };

        let addr = request.src;
        let span = info_span!(parent: &self.span, "request", remote.addr = %addr, name = Empty, query_type = Empty);

        let timer = DNS_REQ_HISTOGRAM.with_label_values(name).start_timer();
        let handle_request = self.catalog.handle_request(request, response_handle);

        future::join(handle_request, future::ready(timer))
            .map(end_timer as EndTimer)
            .instrument(span)
    }
}

fn end_timer((res, timer): (ResponseFutureOutput, HistogramTimer)) -> ResponseFutureOutput {
    timer.observe_duration();

    res
}
