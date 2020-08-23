use warp::{Filter, serve};
use tokio::net::{TcpListener, ToSocketAddrs};
use std::sync::Arc;
use std::error::Error;
use std::io::Cursor;
use parking_lot::RwLock;
use tokio_rustls::TlsAcceptor;
use rustls::{ServerConfig, NoClientAuth, ResolvesServerCertUsingSNI};
use futures_util::{StreamExt, TryStreamExt};
use rustls::internal::pemfile::{pkcs8_private_keys, certs};
use rustls::sign::{RSASigningKey, CertifiedKey, SigningKey};

pub struct Http {
    acceptor: Arc<RwLock<TlsAcceptor>>,
    http: Option<TcpListener>,
    https: Option<TcpListener>
}

impl Http {
    pub async fn new<A: ToSocketAddrs>(http: Option<A>, https: Option<A>) -> tokio::io::Result<Self> {
        let config = Arc::new(ServerConfig::new(NoClientAuth::new()));
        let acceptor = Arc::new(RwLock::new(TlsAcceptor::from(config)));
        let http = match http {
            Some(http) => Some(TcpListener::bind(http).await?),
            None => None
        };
        let https = match https {
            Some(https) => Some(TcpListener::bind(https).await?),
            None => None
        };

        Ok(Http {
            acceptor,
            http,
            https
        })
    }

    pub fn set_config(&self, private: &mut Vec<u8>, cert: &mut Vec<u8>) {
        let mut private = Cursor::new(private);
        let privates = pkcs8_private_keys(&mut private).unwrap();
        let private = privates.get(0).unwrap();
        let private: Arc<Box<dyn SigningKey>> = Arc::new(Box::new(RSASigningKey::new(private).unwrap()));

        let mut cert = Cursor::new(cert);
        let cert = certs(&mut cert).unwrap();

        let certified_key = CertifiedKey::new(cert, private);
        let mut sni = ResolvesServerCertUsingSNI::new();
        sni.add("acme.wehrli.ml", certified_key).unwrap();

        let mut config = ServerConfig::new(Arc::new(NoClientAuth));
        config.cert_resolver = Arc::new(sni);

        let acceptor = TlsAcceptor::from(Arc::new(config));

        *self.acceptor.write() = acceptor;
    }

    pub async fn run(mut self) -> Result<(), impl Error> {
        let test = warp::path("hello")
            .and(warp::path::param())
            .map(|map: String| {
                format!("{} test", map)
            });

        let acceptor = Arc::clone(&self.acceptor);
        let acceptor_stream = futures_util::stream::unfold(acceptor, |acc| async move {
            Some((Arc::clone(&acc), acc))
        });
        let https = if let Some(ref mut https) = self.https {
            let stream = https
                .incoming()
                .zip(acceptor_stream)
                .map(Ok)
                .and_then(|(stream, acceptor)| {
                    acceptor.read().accept(stream.unwrap())
                });

            Some(serve(test).run_incoming(stream))
        } else {
            None
        };

        let http = self.http.map(|http| serve(test).run_incoming(http));

        match (https, http) {
            (Some(https), Some(http)) =>
                match tokio::join!(tokio::spawn(https), tokio::spawn(http)) {
                    (Err(e), _) => Err(e),
                    (_, Err(e)) => Err(e),
                    _ => Ok(()),
                },
            (Some(https), None) => tokio::spawn(https).await,
            (None, Some(http)) => tokio::spawn(http).await,
            _ => Ok(())
        }
    }
}

impl Clone for Http {
    fn clone(&self) -> Self {
        let acceptor = Arc::clone(&self.acceptor);
        Http {
            acceptor,
            http: None,
            https: None
        }
    }
}