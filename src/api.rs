use futures_util::stream::TryStream;
use futures_util::StreamExt;
use parking_lot::RwLock;
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::{NoClientAuth, ServerConfig};
use std::error::Error;
use std::io::Cursor;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, ToSocketAddrs};
use tokio_rustls::TlsAcceptor;
use warp::{serve, Filter};

fn error(kind: ErrorKind, message: &str) -> std::io::Error {
    let error: Box<dyn Error + Send + Sync> = From::from(message.to_string());
    std::io::Error::new(kind, error)
}

fn other_error(message: &str) -> std::io::Error {
    error(ErrorKind::Other, message)
}

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
        let acceptor_stream = futures_util::stream::unfold(acceptor, |acc| async {
            let acceptor = acc.read().clone();
            Some((acceptor, acc))
        });

        self.listener
            .zip(acceptor_stream)
            .then(|(stream, acceptor)| async move {
                match stream {
                    Ok(stream) => acceptor.accept(stream).await,
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

    pub fn set_config(
        &self,
        private: &mut Vec<u8>,
        cert: &mut Vec<u8>,
    ) -> Result<(), std::io::Error> {
        let mut private = Cursor::new(private);
        let mut privates = pkcs8_private_keys(&mut private)
            .map_err(|_| error(ErrorKind::InvalidInput, "Private is invalid"))?;
        let private = privates
            .pop()
            .ok_or_else(|| other_error("Private Vec is empty"))?;

        let mut cert = Cursor::new(cert);
        let cert =
            certs(&mut cert).map_err(|_| error(ErrorKind::InvalidInput, "Cert is invalid"))?;

        let mut config = ServerConfig::new(NoClientAuth::new());
        config
            .set_single_cert(cert, private)
            .map_err(|_| other_error("Couldn't configure Config with Cert and Private"))?;

        let acceptor = TlsAcceptor::from(Arc::new(config));
        *self.acceptor.write() = acceptor;
        Ok(())
    }

    //pub async fn run(mut self) -> Result<(), impl Error> {
    pub async fn spawn(self) -> Result<(), Box<dyn Error>> {
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

        tokio::spawn(async {
            match (https, http) {
                (Some(https), Some(http)) => tokio::try_join!(https, http).map(|_| ()),
                (Some(https), None) => https.await,
                (None, Some(http)) => http.await,
                _ => Ok(()),
            }
        })
        .await??;

        Ok(())
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
