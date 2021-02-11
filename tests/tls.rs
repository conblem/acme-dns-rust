use futures_util::{future, stream, StreamExt};
use rustls::{
    Certificate, ClientConfig, RootCertStore, ServerCertVerified, ServerCertVerifier, TLSError,
};
use std::fs::read_to_string;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::webpki::DNSNameRef;
use tokio_rustls::TlsConnector;

use acme_dns_rust::api::tls;
use acme_dns_rust::facade::{Cert, CertFacade, InMemoryFacade, State};
use acme_dns_rust::util::{now, to_i64};

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
    let cert = read_to_string(Path::new(file!()).with_file_name("cert.crt"));
    let private = read_to_string(Path::new(file!()).with_file_name("key.key"));

    let cert = Cert {
        id: "1".to_owned(),
        update: to_i64(&now()),
        state: State::Ok,
        cert: Some(cert.unwrap()),
        private: Some(private.unwrap()),
        domain: "acme-dns-rust.com".to_owned(),
    };

    let facade = InMemoryFacade::default();
    facade.create_cert(&cert).await.unwrap();

    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = server.local_addr().unwrap();

    let server_future = tokio::spawn(async move {
        let server = server.accept().await.unwrap().0;
        let server = stream::iter(vec![Ok(future::ready(Ok(server)))]);
        let mut acceptor = tls::wrap(server, facade);

        let mut conn = acceptor.next().await.unwrap().unwrap().await.unwrap();
        let mut actual = String::new();
        conn.read_to_string(&mut actual).await.unwrap();
        assert_eq!("Test", actual);
    });

    let client_future = tokio::spawn(async move {
        let client = TcpStream::connect(addr).await.unwrap();
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
