use futures_util::{future, stream, StreamExt};
use rustls::{
    Certificate, ClientConfig, RootCertStore, ServerCertVerified, ServerCertVerifier, TLSError,
};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::webpki::{
    DNSNameRef, EndEntityCert, TLSServerTrustAnchors, Time, ECDSA_P256_SHA256,
    RSA_PKCS1_2048_8192_SHA256,
};
use tokio_rustls::TlsConnector;

use acme_dns_rust::api::tls;
use acme_dns_rust::facade::{Cert, CertFacade, InMemoryFacade, State};
use acme_dns_rust::util::{now, to_i64};
use rustls::internal::pemfile;
use std::time::SystemTime;
use tokio_rustls::webpki::trust_anchor_util::cert_der_as_trust_anchor;

struct TestVerifier;

impl ServerCertVerifier for TestVerifier {
    fn verify_server_cert(
        &self,
        _roots: &RootCertStore,
        certs: &[Certificate],
        dns_name: DNSNameRef<'_>,
        _ocsp_response: &[u8],
    ) -> Result<ServerCertVerified, TLSError> {
        let ca = pemfile::certs(&mut &include_bytes!("./ca.crt")[..]).unwrap();
        let anchor = [cert_der_as_trust_anchor(ca[0].as_ref()).unwrap()];
        let anchor = TLSServerTrustAnchors(&anchor);

        let cert = certs[0].as_ref();
        let cert = EndEntityCert::from(cert).unwrap();
        let time = Time::try_from(SystemTime::now()).unwrap();
        cert.verify_is_valid_tls_server_cert(&[&RSA_PKCS1_2048_8192_SHA256], &anchor, &[], time)
            .unwrap();

        let domain = DNSNameRef::try_from_ascii_str("acme-dns-rust.com").unwrap();
        assert_eq!(domain.to_owned(), dns_name.to_owned());

        Ok(ServerCertVerified::assertion())
    }
}

#[tokio::test]
async fn test() {
    let cert = Cert {
        id: "1".to_owned(),
        update: to_i64(&now()),
        state: State::Ok,
        cert: Some(include_str!("./cert.crt").to_owned()),
        private: Some(include_str!("./cert.key").to_owned()),
        domain: "acme-dns-rust.com".to_owned(),
    };

    let facade = InMemoryFacade::default();
    facade.create_cert(&cert).await.unwrap();

    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = server.local_addr().unwrap();

    let server_future = tokio::spawn(async move {
        let (server, _) = server.accept().await.unwrap();
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
