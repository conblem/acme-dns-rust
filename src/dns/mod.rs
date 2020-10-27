use anyhow::Result;
use std::future::Future;
use tokio::net::{ToSocketAddrs, UdpSocket};
use tokio::runtime::Runtime;
use tracing::field::{debug, Empty};
use tracing::{info_span, Span};
use tracing_futures::Instrument;
use trust_dns_server::authority::{AuthorityObject, Catalog};
use trust_dns_server::proto::rr::Name;
use trust_dns_server::ServerFuture;

mod authority;
mod handler;
mod parse;

pub use self::authority::DatabaseAuthority;
use crate::dns::handler::TraceRequestHandler;

pub struct DNS<'a, A> {
    server: ServerFuture<TraceRequestHandler>,
    addr: A,
    runtime: &'a Runtime,
    span: Span,
}

impl<'a, A: 'a + ToSocketAddrs> DNS<'a, A> {
    pub fn new(addr: A, runtime: &'a Runtime, authority: Box<dyn AuthorityObject>) -> Self {
        let span = info_span!("DNS::spawn", local.addr = Empty);

        let mut catalog = Catalog::new();
        catalog.upsert(Name::root().into(), authority);
        let request_handler = TraceRequestHandler::new(catalog, span.clone());

        let server = ServerFuture::new(request_handler);

        DNS {
            server,
            addr,
            runtime,
            span,
        }
    }

    pub fn spawn(mut self) -> impl 'a + Future<Output = Result<()>> {
        let span = self.span.clone();
        async move {
            let udp = UdpSocket::bind(self.addr).await?;
            self.span.record("local.addr", &debug(udp.local_addr()));
            self.server.register_socket(udp, self.runtime);

            tokio::spawn(self.server.block_until_done()).await??;

            Ok(())
        }
        .instrument(span)
    }
}
