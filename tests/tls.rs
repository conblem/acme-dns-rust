use futures_util::{future, stream, StreamExt};
use rustls::client::{ServerCertVerified, ServerCertVerifier};
use rustls::{Certificate, ClientConfig, Error, RootCertStore, ServerName};
use std::convert::TryFrom;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::webpki::{
    DNSNameRef, EndEntityCert, TLSServerTrustAnchors, Time, RSA_PKCS1_2048_8192_SHA256,
};
use tokio_rustls::TlsConnector;

use acme_dns_rust::api::tls;
use acme_dns_rust::facade::{Cert, CertFacade, InMemoryFacade, State};
use acme_dns_rust::util::{now, to_i64};
use std::time::SystemTime;

struct TestVerifier;

/*impl ServerCertVerifier for TestVerifier {
    fn verify_server_cert(
        &self,
        _roots: &RootCertStore,
        certs: &[Certificate],
        dns_name: DNSNameRef<'_>,
        _ocsp_response: &[u8],
    ) -> Result<ServerCertVerified, Error> {
        /*let ca = rustls_pemfile::certs(&mut &include_bytes!("./ca.crt")[..]).unwrap();
        let anchor = [cert_der_as_trust_anchor(ca[0].as_ref()).unwrap()];
        let anchor = TLSServerTrustAnchors(&anchor);

        let cert = certs[0].as_ref();
        let cert = EndEntityCert::from(cert).unwrap();
        let time = Time::try_from(SystemTime::now()).unwrap();
        cert.verify_is_valid_tls_server_cert(&[&RSA_PKCS1_2048_8192_SHA256], &anchor, &[], time)
            .unwrap();

        let domain = DNSNameRef::try_from_ascii_str("acme-dns-rust.com").unwrap();
        assert_eq!(domain.to_owned(), dns_name.to_owned());

        Ok(ServerCertVerified::assertion())*/
        todo!()
    }
}*/

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
        todo!()
    }
}

#[tokio::test]
#[ignore]
async fn test() {
    let cert = Cert {
        id: "1".to_owned(),
        update: to_i64(&now()),
        state: State::Ok,
        // todo: wrong files just to fix compilation
        cert: Some(include_str!("./ca.crt").to_owned()),
        private: Some(include_str!("./ca.key").to_owned()),
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
        let mut client_config = ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(Arc::new(TestVerifier))
            .with_no_client_auth();

        let connector = TlsConnector::from(Arc::new(client_config));

        let domain = ServerName::try_from("acme-dns-rust.com").unwrap();
        let mut conn = connector.connect(domain, client).await.unwrap();
        conn.write_all("Test".as_ref()).await.unwrap();
        conn.write(&[]).await.unwrap();
    });

    tokio::try_join!(server_future, client_future).unwrap();
}
