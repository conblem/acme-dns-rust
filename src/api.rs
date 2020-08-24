use futures_util::stream::TryStream;
use futures_util::StreamExt;
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::sign::{CertifiedKey, RSASigningKey, SigningKey};
use rustls::{NoClientAuth, ResolvesServerCertUsingSNI, ServerConfig};
use std::error::Error;
use std::io::Cursor;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, ToSocketAddrs};
use tokio::sync::RwLock;
use tokio_rustls::TlsAcceptor;
use warp::{serve, Filter};

pub struct Https {
    acceptor: Arc<RwLock<TlsAcceptor>>,
    listener: TcpListener,
}

impl Https {
    async fn new<A: ToSocketAddrs>(
        addr: A,
        acceptor: Arc<RwLock<TlsAcceptor>>,
    ) -> tokio::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Https { acceptor, listener })
    }

    fn stream(
        self,
    ) -> impl TryStream<
        Ok = impl AsyncRead + AsyncWrite + Send + 'static + Unpin,
        Error = impl Into<Box<dyn std::error::Error + Send + Sync>>,
    > + Send {
        let acceptor = Arc::clone(&self.acceptor);
        let acceptor_stream =
            futures_util::stream::unfold(
                acceptor,
                |acc| async { Some((Arc::clone(&acc), acc)) },
            );

        self.listener
            .zip(acceptor_stream)
            .then(|(stream, acceptor)| async move {
                match stream {
                    Ok(stream) => acceptor.read().await.accept(stream).await,
                    Err(e) => Err(e),
                }
            })
    }
}

pub struct Api {
    acceptor: Arc<RwLock<TlsAcceptor>>,
    http: Option<TcpListener>,
    https: Option<Https>,
}

impl Api {
    pub async fn new<A: ToSocketAddrs>(
        http: Option<A>,
        https: Option<A>,
    ) -> tokio::io::Result<Self> {
        let config = Arc::new(ServerConfig::new(NoClientAuth::new()));
        let acceptor = Arc::new(RwLock::new(TlsAcceptor::from(config)));

        let http = match http {
            Some(http) => Some(TcpListener::bind(http).await?),
            None => None,
        };
        let https = match https {
            Some(https) => Some(Https::new(https, Arc::clone(&acceptor)).await?),
            None => None,
        };

        Ok(Api {
            acceptor,
            http,
            https,
        })
    }

    //error handling
    pub async fn set_config(&self, private: &mut Vec<u8>, cert: &mut Vec<u8>) {
        let mut private = Cursor::new(private);
        let privates = pkcs8_private_keys(&mut private).unwrap();
        let private = privates.get(0).unwrap();
        let private: Arc<Box<dyn SigningKey>> =
            Arc::new(Box::new(RSASigningKey::new(private).unwrap()));

        let mut cert = Cursor::new(cert);
        let cert = certs(&mut cert).unwrap();

        let certified_key = CertifiedKey::new(cert, private);
        let mut sni = ResolvesServerCertUsingSNI::new();
        sni.add("acme.wehrli.ml", certified_key).unwrap();

        let mut config = ServerConfig::new(Arc::new(NoClientAuth));
        config.cert_resolver = Arc::new(sni);

        let acceptor = TlsAcceptor::from(Arc::new(config));

        *self.acceptor.write().await = acceptor;
    }

    //pub async fn run(mut self) -> Result<(), impl Error> {
    pub async fn run(self) -> Result<(), impl Error> {
        let test = warp::path("hello")
            .and(warp::path::param())
            .map(|map: String| format!("{} test", map));

        let http = self
            .http
            .map(|http| serve(test).run_incoming(http))
            .map(tokio::spawn);

        let https = self
            .https
            .map(|https| serve(test).run_incoming(https.stream()))
            .map(tokio::spawn);

        match (https, http) {
            (Some(https), Some(http)) => match tokio::join!(https, http) {
                (Err(e), _) => Err(e),
                (_, Err(e)) => Err(e),
                _ => Ok(()),
            },
            (Some(https), None) => https.await,
            (None, Some(http)) => http.await,
            _ => Ok(()),
        }
    }
}

impl Clone for Api {
    fn clone(&self) -> Self {
        let acceptor = Arc::clone(&self.acceptor);
        Api {
            acceptor,
            http: None,
            https: None,
        }
    }
}
