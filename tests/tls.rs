use anyhow::{anyhow, Result};
use futures_util::{future, stream, StreamExt};
use rustls::client::{ServerCertVerified, ServerCertVerifier, WebPkiVerifier};
use rustls::{Certificate, ClientConfig, Error, RootCertStore, ServerName};
use std::convert::TryFrom;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;

use acme_dns_rust::api::tls;
use acme_dns_rust::facade::{Cert, CertFacade, InMemoryFacade, State};
use acme_dns_rust::util::{now, to_i64};
use std::time::SystemTime;

struct TestVerifier(WebPkiVerifier);

impl TestVerifier {
    fn new() -> Arc<dyn ServerCertVerifier> {
        let ca = rustls_pemfile::certs(&mut &include_bytes!("./root-ca.crt")[..]).unwrap();
        let mut store = RootCertStore::empty();
        // todo: look at this number
        let (_added, _ignored) = store.add_parsable_certificates(ca.as_ref());
        let inner = WebPkiVerifier::new(store, None);
        Arc::new(TestVerifier(inner))
    }
}

impl ServerCertVerifier for TestVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &Certificate,
        intermediates: &[Certificate],
        server_name: &ServerName,
        scts: &mut dyn Iterator<Item = &[u8]>,
        ocsp_response: &[u8],
        now: SystemTime,
    ) -> Result<ServerCertVerified, Error> {
        let res = self.0.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            scts,
            ocsp_response,
            now,
        );

        // check if client sends correct sni name
        let name = match server_name {
            ServerName::DnsName(name) => name.as_ref(),
            _ => return Err(Error::General("Wrong SNI".to_string())),
        };
        assert_eq!("acme-dns-rust.com", name);

        res
    }
}

// todo: simplify this test alot
#[tokio::test]
async fn test() -> Result<()> {
    let cert = Cert {
        id: "1".to_owned(),
        update: to_i64(now()),
        state: State::Ok,
        // todo: wrong files just to fix compilation
        cert: Some(include_str!("./leaf.crt").to_owned()),
        private: Some(include_str!("./leaf.key").to_owned()),
        domain: "acme-dns-rust.com".to_owned(),
    };

    let facade = InMemoryFacade::default();
    facade.create_cert(&cert).await?;

    let server = TcpListener::bind("127.0.0.1:0").await?;
    let addr = server.local_addr()?;

    let server_future = tokio::spawn(async move {
        let (server, _) = server.accept().await?;
        let server = stream::iter(vec![Ok(future::ready(Ok(server)))]);
        let mut acceptor = tls::wrap(server, facade);

        let mut conn = acceptor
            .next()
            .await
            .ok_or_else(|| anyhow!("Acceptor finished"))??
            .await?;
        let mut actual = String::new();
        let byte_read = conn.read_to_string(&mut actual).await?;
        assert_eq!(4, byte_read);
        assert_eq!("Test", actual);

        Ok(()) as Result<()>
    });

    let client_future = tokio::spawn(async move {
        let client = TcpStream::connect(addr).await?;
        let client_config = ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(TestVerifier::new())
            .with_no_client_auth();

        let connector = TlsConnector::from(Arc::new(client_config));

        let domain = ServerName::try_from("acme-dns-rust.com").unwrap();
        let mut conn = connector.connect(domain, client).await?;
        conn.write_all("Test".as_ref()).await?;
        conn.shutdown().await?;

        Ok(()) as Result<()>
    });

    let (server_res, client_res) = tokio::try_join!(server_future, client_future)?;
    server_res?;
    client_res?;

    Ok(())
}
