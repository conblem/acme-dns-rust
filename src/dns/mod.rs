use anyhow::Result;
use std::future::Future;
use tokio::net::{ToSocketAddrs, UdpSocket};
use tokio::runtime::Runtime;
use tracing::field::{debug, Empty};
use tracing::{info_span, Instrument, Span};
use trust_dns_server::authority::{AuthorityObject, Catalog};
use trust_dns_server::proto::rr::Name;
use trust_dns_server::ServerFuture;

mod authority;
mod handler;
mod parse;

pub use self::authority::DatabaseAuthority;
use crate::dns::handler::TraceRequestHandler;

pub struct DNS<A> {
    server: ServerFuture<TraceRequestHandler>,
    addr: A,
    span: Span,
}

impl<A: ToSocketAddrs> DNS<A> {
    pub fn new(addr: A, authority: Box<dyn AuthorityObject>) -> Self {
        let span = info_span!("DNS::spawn", local.addr = Empty);

        let mut catalog = Catalog::new();
        catalog.upsert(Name::root().into(), authority);
        let request_handler = TraceRequestHandler::new(catalog, span.clone());

        let server = ServerFuture::new(request_handler);

        DNS {
            server,
            addr,
            span,
        }
    }

    pub fn spawn(mut self) -> impl Future<Output = Result<()>> {
        let span = self.span.clone();
        async move {
            let udp = UdpSocket::bind(self.addr).await?;
            self.span.record("local.addr", &debug(udp.local_addr()));
            self.server.register_socket(udp);

            tokio::spawn(self.server.block_until_done()).await??;

            Ok(())
        }
        .instrument(span)
    }
}
