use tokio::net::{ToSocketAddrs, UdpSocket};
use tokio::runtime::Runtime;
use trust_dns_server::authority::{AuthorityObject, Catalog};
use trust_dns_server::ServerFuture;
use anyhow::Result;

mod authority;
mod parse;

pub use self::authority::DatabaseAuthority;

pub struct DNS<'a, A> {
    server: ServerFuture<Catalog>,
    addr: A,
    runtime: &'a Runtime,
}

impl<'a, A: ToSocketAddrs> DNS<'a, A> {
    pub fn new(addr: A, runtime: &'a Runtime, authority: Box<dyn AuthorityObject>) -> Self {
        let mut catalog = Catalog::new();
        catalog.upsert(authority.origin().clone(), authority);

        let server = ServerFuture::new(catalog);

        DNS {
            server,
            addr,
            runtime,
        }
    }

    pub async fn spawn(mut self) -> Result<()> {
        let udp = UdpSocket::bind(self.addr).await?;
        self.server.register_socket(udp, self.runtime);

        tokio::spawn(self.server.block_until_done()).await??;

        Ok(())
    }
}
