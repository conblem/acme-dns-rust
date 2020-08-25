use crate::cert::{Cert, CertFacade};
use log::error;
use futures_util::stream::TryStream;
use futures_util::{StreamExt, TryStreamExt};
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::{NoClientAuth, ServerConfig};
use sqlx::PgPool;
use std::error::Error;
use std::io::Cursor;
use std::io::ErrorKind;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, ToSocketAddrs};
use tokio_rustls::{Accept, TlsAcceptor, TlsStream};
use warp::{serve, Filter};
use parking_lot::RwLock;

fn error(kind: ErrorKind, message: &str) -> std::io::Error {
    let error: Box<dyn Error + Send + Sync> = From::from(message.to_string());
    std::io::Error::new(kind, error)
}

fn other_error(message: &str) -> std::io::Error {
    error(ErrorKind::Other, message)
}

struct Acceptor {
    pool: PgPool,
    cert: RwLock<Option<Cert>>,
    tls_acceptor: RwLock<TlsAcceptor>,
}

impl Acceptor {
    fn new(pool: PgPool) -> Self {
        let config = ServerConfig::new(NoClientAuth::new());
        let tls_acceptor = TlsAcceptor::from(Arc::new(config));

        Acceptor {
            pool,
            cert: RwLock::new(None),
            tls_acceptor: RwLock::new(tls_acceptor),
        }
    }

    fn create_cert(db_cert: &mut Cert) -> Result<TlsAcceptor, std::io::Error> {
        let (private, cert) = match (&mut db_cert.private, &mut db_cert.cert) {
            (Some(ref mut private), Some(ref mut cert)) => (private, cert),
            _ => return Err(other_error("Cert has no Cert or Private")),
        };

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

        Ok(TlsAcceptor::from(Arc::new(config)))
    }

    async fn load_cert(&self) -> Result<TlsAcceptor, std::io::Error> {
        let mut new_cert = CertFacade::first_cert(&self.pool).await;
        println!("new cert {:?}", new_cert);

        let mut db_cert = match (new_cert, self.cert.read().as_ref()) {
            (Some(new_cert), Some(cert)) if &new_cert == cert => return Ok(self.tls_acceptor.read().clone()),
            (Some(new_cert), _) => new_cert,
            _ => return Ok(self.tls_acceptor.read().clone())
        };

        println!("db cert {:?}", db_cert);

        let tls_acceptor = Acceptor::create_cert(&mut db_cert)?;
        *self.tls_acceptor.write() = tls_acceptor.clone();
        *self.cert.write() = Some(db_cert);
        Ok(tls_acceptor)
    }
}

pub struct Https {
    pool: PgPool,
    listener: TcpListener,
}

impl Https {
    async fn new<A: ToSocketAddrs>(
        pool: PgPool,
        addr: A
    ) -> tokio::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Https { pool, listener })
    }

    fn stream(
        self,
    ) -> impl TryStream<
        Ok = impl AsyncRead + AsyncWrite + Send + 'static + Unpin,
        Error = impl Into<Box<dyn std::error::Error + Send + Sync>>,
    > + Send {
        let acceptor = Acceptor::new(self.pool);
        let acceptor_stream = futures_util::stream::unfold(acceptor, |acc| async {
            Some((acc.load_cert().await, acc))
        });

        self.listener
            .zip(acceptor_stream)
            .then(|item| async move {
                match item {
                    (Ok(stream), Ok(acceptor)) => acceptor.accept(stream).await,
                    (Err(e), _) => Err(e),
                    (_, Err(e)) => Err(e),
                }
            })
            .inspect_err(|err| error!("Stream error: {}", err))
            .filter(|stream| futures_util::future::ready(stream.is_ok()))
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
        pool: PgPool
    ) -> tokio::io::Result<Self> {
        let config = Arc::new(ServerConfig::new(NoClientAuth::new()));
        let acceptor = Arc::new(RwLock::new(TlsAcceptor::from(config)));

        let http = match http {
            Some(http) => Some(TcpListener::bind(http).await?),
            None => None,
        };
        let https = match https {
            Some(https) => Some(Https::new(pool, https).await?),
            None => None,
        };

        Ok(Api {
            acceptor,
            http,
            https,
        })
    }

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

