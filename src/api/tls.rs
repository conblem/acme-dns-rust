use anyhow::{anyhow, Result};
use futures_util::stream::{repeat, Stream};
use futures_util::{StreamExt, TryFutureExt, TryStreamExt};
use parking_lot::RwLock;
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::{NoClientAuth, ServerConfig};
use std::future::Future;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, Result as IoResult};
use tokio_rustls::TlsAcceptor;
use tracing::{error, info};

use crate::facade::{Cert, CertFacade};
use crate::util::to_u64;

pub(super) fn wrap<L, I, S, F>(
    listener: L,
    facade: F,
) -> impl Stream<
    Item = Result<
        impl Future<Output = Result<impl AsyncRead + AsyncWrite + Send + Unpin + 'static>>,
    >,
> + Send
where
    L: Stream<Item = IoResult<I>> + Send,
    I: Future<Output = IoResult<S>> + Send,
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    F: CertFacade + Send + Sync,
{
    let acceptor = Acceptor::new(facade);

    listener
        .err_into()
        .zip(repeat(acceptor))
        .map(|(conn, acceptor)| conn.map(|c| (c, acceptor)))
        .map_ok(|(conn, acceptor)| async move {
            let (conn, tls) = tokio::try_join!(conn.err_into(), acceptor.load_cert())?;
            Ok(tls.accept(conn).await?)
        })
}

struct Acceptor<F> {
    facade: F,
    config: RwLock<(Option<Cert>, Arc<ServerConfig>)>,
}

impl<F: CertFacade> Acceptor<F> {
    fn new(facade: F) -> Arc<Self> {
        let server_config = ServerConfig::new(NoClientAuth::new());

        Arc::new(Acceptor {
            facade,
            config: RwLock::new((None, Arc::new(server_config))),
        })
    }

    fn create_server_config(&self, db_cert: &Cert) -> Result<Arc<ServerConfig>> {
        let (private, cert) = match (&db_cert.private, &db_cert.cert) {
            (Some(private), Some(cert)) => (private, cert),
            _ => return Err(anyhow!("Cert has no Cert or Private")),
        };

        let mut privates = pkcs8_private_keys(&mut private.as_bytes())
            .map_err(|_| anyhow!("Private is invalid {:?}", private))?;
        let private = privates
            .pop()
            .ok_or_else(|| anyhow!("Private Vec is empty {:?}", privates))?;

        let cert =
            certs(&mut cert.as_bytes()).map_err(|_| anyhow!("Cert is invalid {:?}", cert))?;

        let mut config = ServerConfig::new(NoClientAuth::new());
        config.set_single_cert(cert, private)?;
        config.set_protocols(&["h2".into(), "http/1.1".into()]);

        Ok(Arc::new(config))
    }

    async fn load_cert(&self) -> Result<TlsAcceptor> {
        let new_cert = self.facade.first_cert().await;

        let db_cert = match (new_cert, &*self.config.read()) {
            (Ok(Some(new_cert)), (cert, _)) if Some(&new_cert) != cert.as_ref() => new_cert,
            (_, (_, server_config)) => {
                info!("Using existing TLS Config");
                return Ok(TlsAcceptor::from(Arc::clone(server_config)));
            }
        };
        info!(timestamp = to_u64(&db_cert.update), "Found new cert");

        let server_config = match self.create_server_config(&db_cert) {
            Ok(server_config) => server_config,
            Err(e) => {
                error!("{}", e);
                let (_, server_config) = &*self.config.read();
                return Ok(TlsAcceptor::from(Arc::clone(server_config)));
            }
        };

        *self.config.write() = (Some(db_cert), Arc::clone(&server_config));
        info!("Created new TLS config");
        Ok(TlsAcceptor::from(server_config))
    }
}

#[cfg(test)]
mod tests {
    use futures_util::{future, stream, StreamExt};
    use rustls::{
        Certificate, ClientConfig, RootCertStore, ServerCertVerified, ServerCertVerifier, TLSError,
    };
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::webpki::DNSNameRef;
    use tokio_rustls::TlsConnector;

    use super::wrap;
    use crate::facade::TestFacade;
    use tokio::net::{TcpListener, TcpStream};

    struct TestVerifier;

    impl ServerCertVerifier for TestVerifier {
        fn verify_server_cert(
            &self,
            _roots: &RootCertStore,
            presented_certs: &[Certificate],
            dns_name: DNSNameRef<'_>,
            _ocsp_response: &[u8],
        ) -> Result<ServerCertVerified, TLSError> {
            let domain = DNSNameRef::try_from_ascii_str("acme-dns-rust.com")
                .unwrap()
                .to_owned();
            assert_eq!(domain, dns_name.to_owned());
            assert!(presented_certs.first().is_some());
            Ok(ServerCertVerified::assertion())
        }
    }

    #[tokio::test]
    async fn test() {
        let server_future = tokio::spawn(async move {
            let server = TcpListener::bind("127.0.0.1:33000").await.unwrap();
            let server = server.accept().await.unwrap().0;
            let server = stream::iter(vec![Ok(future::ready(Ok(server)))]);
            let mut acceptor = wrap(server, TestFacade::default());

            let mut conn = acceptor.next().await.unwrap().unwrap().await.unwrap();
            let mut actual = String::new();
            conn.read_to_string(&mut actual).await.unwrap();
            assert_eq!("Test", actual);
        });

        let client_future = tokio::spawn(async move {
            let client = TcpStream::connect("127.0.0.1:33000").await.unwrap();
            let mut client_config = ClientConfig::new();
            client_config
                .dangerous()
                .set_certificate_verifier(Arc::new(TestVerifier {}));

            let connector = TlsConnector::from(Arc::new(client_config));

            let domain = DNSNameRef::try_from_ascii_str("acme-dns-rust.com").unwrap();
            let mut conn = connector.connect(domain, client).await.unwrap();
            conn.write_all("Test".as_ref()).await.unwrap();
            conn.write(&[]).await.unwrap();
        });

        tokio::try_join!(server_future, client_future).unwrap();
    }
}
