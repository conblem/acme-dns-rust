use anyhow::Result;
use tokio::net::{ToSocketAddrs, UdpSocket};
use tracing::field::{debug, Empty};
use tracing::{info_span, Span};
use trust_dns_server::authority::{AuthorityObject, Catalog};
use trust_dns_server::proto::rr::Name;
use trust_dns_server::ServerFuture;

mod authority;
mod handler;

pub use authority::DatabaseAuthority;
use handler::TraceRequestHandler;

pub struct Dns<A> {
    server: ServerFuture<TraceRequestHandler>,
    addr: A,
    span: Span,
}

// span setup here makes no sense
// todo: fix this
impl<A: ToSocketAddrs> Dns<A> {
    pub fn new(addr: A, authority: Box<dyn AuthorityObject>) -> Self {
        let span = info_span!("DNS::spawn", local.addr = Empty);

        let mut catalog = Catalog::new();
        catalog.upsert(Name::root().into(), authority);
        let request_handler = TraceRequestHandler::new(catalog, span.clone());

        let server = ServerFuture::new(request_handler);

        Dns { server, addr, span }
    }

    #[tracing::instrument(skip(self))]
    pub async fn spawn(mut self) -> Result<()> {
        let udp = UdpSocket::bind(self.addr).await?;
        self.span.record("local.addr", &debug(udp.local_addr()));
        self.server.register_socket(udp);

        tokio::spawn(self.server.block_until_done()).await??;

        Ok(())
    }
}
