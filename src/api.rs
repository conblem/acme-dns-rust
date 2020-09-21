use crate::cert::{Cert, CertFacade};
use crate::domain::{Domain, DomainFacade};
use futures_util::stream::TryStream;
use futures_util::{StreamExt, TryStreamExt};
use log::error;
use parking_lot::RwLock;
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::{NoClientAuth, ServerConfig};
use sqlx::PgPool;
use std::error::Error;
use std::io::Cursor;
use std::io::ErrorKind;
use std::ops::Deref;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, ToSocketAddrs};
use tokio_rustls::TlsAcceptor;
use warp::{http::Response, reply, serve, Filter, Rejection, Reply};

fn error(kind: ErrorKind, message: &str) -> std::io::Error {
    let error: Box<dyn Error + Send + Sync> = From::from(message.to_string());
    std::io::Error::new(kind, error)
}

fn other_error(message: &str) -> std::io::Error {
    error(ErrorKind::Other, message)
}

struct Acceptor {
    pool: PgPool,
    config: RwLock<(Option<Cert>, Arc<ServerConfig>)>,
}

impl Acceptor {
    fn new(pool: PgPool) -> Self {
        let server_config = ServerConfig::new(NoClientAuth::new());

        Acceptor {
            pool,
            config: RwLock::new((None, Arc::new(server_config))),
        }
    }

    fn create_server_config(db_cert: &mut Cert) -> Result<Arc<ServerConfig>, std::io::Error> {
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
        config.set_protocols(&["h2".into(), "http/1.1".into()]);

        Ok(Arc::new(config))
    }

    async fn load_cert(&self) -> Result<TlsAcceptor, std::io::Error> {
        let new_cert = CertFacade::first_cert(&self.pool).await;

        // could probably be improved
        let mut db_cert = match (new_cert, self.config.read().deref()) {
            (Some(new_cert), (Some(cert), server_config)) if &new_cert == cert => {
                return Ok(TlsAcceptor::from(Arc::clone(server_config)))
            }
            (Some(new_cert), _) => new_cert,
            (_, (_, server_config)) => return Ok(TlsAcceptor::from(Arc::clone(server_config))),
        };

        let server_config = Acceptor::create_server_config(&mut db_cert)?;
        *self.config.write() = (Some(db_cert), Arc::clone(&server_config));
        Ok(TlsAcceptor::from(server_config))
    }
}

pub struct Https {
    pool: PgPool,
    listener: TcpListener,
}

impl Https {
    async fn new<A: ToSocketAddrs>(pool: PgPool, addr: A) -> tokio::io::Result<Self> {
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
    http: Option<TcpListener>,
    https: Option<Https>,
    pool: PgPool,
}

async fn register(pool: PgPool, domain: Domain) -> Result<reply::Response, Rejection> {
    let domain = match DomainFacade::create_domain(&pool, &domain).await {
        Err(e) => {
            return Ok(Response::builder()
                .status(500)
                .body("")
                .unwrap()
                .into_response())
        }
        Ok(domain) => domain,
    };

    Ok(Response::new("no error").into_response())
}

impl Api {
    pub async fn new<A: ToSocketAddrs>(
        http: Option<A>,
        https: Option<A>,
        pool: PgPool,
    ) -> tokio::io::Result<Self> {
        let http = match http {
            Some(http) => Some(TcpListener::bind(http).await?),
            None => None,
        };
        let https = match https {
            Some(https) => Some(Https::new(pool.clone(), https).await?),
            None => None,
        };

        Ok(Api { http, https, pool })
    }

    pub async fn spawn(self) -> Result<(), Box<dyn Error>> {
        let pool = self.pool;
        let routes = warp::path("register")
            .and(warp::post())
            .map(move || pool.clone())
            .and(warp::body::json())
            .and_then(register);

        let http = self
            .http
            .map(|http| serve(routes.clone()).run_incoming(http))
            .map(tokio::spawn);

        let https = self
            .https
            .map(|https| serve(routes).run_incoming(https.stream()))
            .map(tokio::spawn);

        match (https, http) {
            (Some(https), Some(http)) => tokio::try_join!(https, http).map(|_| ()),
            (Some(https), None) => https.await,
            (None, Some(http)) => http.await,
            _ => Ok(()),
        }?;

        Ok(())
    }
}
