use anyhow::{anyhow, Error, Result};
use futures_util::future::OptionFuture;
use futures_util::stream::{repeat, Stream};
use futures_util::FutureExt;
use futures_util::{StreamExt, TryStreamExt};
use parking_lot::RwLock;
use prometheus::proto::MetricFamily;
use prometheus::{Encoder, TextEncoder};
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::{NoClientAuth, ServerConfig};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, ToSocketAddrs};
use tokio_rustls::TlsAcceptor;
use tracing::{debug_span, error, info};
use tracing_futures::Instrument;
use warp::{http::Response, serve, Filter, Rejection, Reply};

use crate::cert::{Cert, CertFacade};
use crate::domain::{Domain, DomainFacade};
use crate::util::to_u64;

struct Acceptor {
    pool: PgPool,
    config: RwLock<(Option<Cert>, Arc<ServerConfig>)>,
}

impl Acceptor {
    fn new(pool: PgPool) -> Arc<Self> {
        let server_config = ServerConfig::new(NoClientAuth::new());

        Arc::new(Acceptor {
            pool,
            config: RwLock::new((None, Arc::new(server_config))),
        })
    }

    fn create_server_config(db_cert: &Cert) -> Result<Arc<ServerConfig>> {
        let (private, cert) = match (&db_cert.private, &db_cert.cert) {
            (Some(ref private), Some(ref cert)) => (private, cert),
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
        let new_cert = CertFacade::first_cert(&self.pool).in_current_span().await;

        let db_cert = match (new_cert, &*self.config.read()) {
            (Ok(Some(new_cert)), (cert, _)) if Some(&new_cert) != cert.as_ref() => new_cert,
            (_, (_, server_config)) => {
                info!("Using existing TLS Config");
                return Ok(TlsAcceptor::from(Arc::clone(server_config)));
            }
        };
        info!(timestamp = to_u64(&db_cert.update), "Found new cert");

        let server_config = match Acceptor::create_server_config(&db_cert) {
            Ok(server_config) => server_config,
            Err(e) => {
                error!("{:?}", e);
                let (_, server_config) = &*self.config.read();
                return Ok(TlsAcceptor::from(Arc::clone(server_config)));
            }
        };

        *self.config.write() = (Some(db_cert), Arc::clone(&server_config));
        info!("Created new TLS config");
        Ok(TlsAcceptor::from(server_config))
    }
}

fn stream(
    listener: TcpListener,
    pool: PgPool,
) -> impl Stream<Item = Result<impl AsyncRead + AsyncWrite + Send + Unpin + 'static, Error>> + Send
{
    let acceptor = Acceptor::new(pool);

    listener
        .zip(repeat(acceptor))
        .map(|(conn, acceptor)| conn.map(|c| (c, acceptor)))
        .err_into()
        .map_ok(|(conn, acceptor)| {
            let addr = conn.peer_addr();
            async move {
                let tls = acceptor.load_cert().in_current_span().await?;
                Ok(tls.accept(conn).in_current_span().await?)
            }
            .instrument(debug_span!("TLS", remote.addr = ?addr))
        })
        .try_buffer_unordered(100)
        .inspect_err(|err| error!("Stream error: {:?}", err))
        .filter(|stream| futures_util::future::ready(stream.is_ok()))
        .into_stream()
}

pub struct Api {
    http: Option<TcpListener>,
    https: Option<TcpListener>,
    prom: Option<TcpListener>,
    pool: PgPool,
}

async fn register(pool: PgPool, domain: Domain) -> Result<impl Reply, Rejection> {
    let _domain = match DomainFacade::create_domain(&pool, &domain).await {
        Err(e) => {
            error!("{}", e);
            return Ok(Response::builder()
                .status(500)
                .body(e.to_string())
                .into_response());
        }
        Ok(domain) => domain,
    };

    info!("Success for call");
    Ok(Response::new("no error").into_response())
}

fn prom() -> impl Reply {
    let encoder = TextEncoder::new();
    let family = MetricFamily::new();
    let mut res = vec![];
    if let Err(e) = encoder.encode(&[family], &mut res) {
        error!("{}", e);
        return Response::builder()
            .status(500)
            .body(e.to_string())
            .into_response();
    }

    Response::builder()
        .header("Content-Type", "text/plain")
        .body(res)
        .into_response()
}

impl Api {
    pub async fn new<A: ToSocketAddrs>(
        http: Option<A>,
        https: Option<A>,
        prom: Option<A>,
        pool: PgPool,
    ) -> Result<Self> {
        let http = OptionFuture::from(http.map(TcpListener::bind)).map(Option::transpose);
        let https = OptionFuture::from(https.map(TcpListener::bind)).map(Option::transpose);
        let prom = OptionFuture::from(prom.map(TcpListener::bind)).map(Option::transpose);

        let (http, https, prom) = tokio::try_join!(http, https, prom)?;

        Ok(Api {
            http,
            https,
            prom,
            pool,
        })
    }

    #[tracing::instrument(name = "Api::spawn", skip(self))]
    pub async fn spawn(self) -> Result<()> {
        info!("Starting API spawn");
        let metrics = warp::path("metrics")
            .and(warp::get())
            .map(prom)
            .with(warp::trace::request());

        let pool = self.pool.clone();
        let routes = warp::path("register")
            .and(warp::post())
            .map(move || pool.clone())
            .and(warp::body::json())
            .and_then(register)
            .with(warp::trace::request());

        let http = self
            .http
            .map(|http| {
                //info!(?http, "Starting http");
                let addr = http.local_addr();
                http.instrument(debug_span!("HTTP", local.addr = ?addr))
            })
            .map(|http| serve(routes.clone()).serve_incoming(http))
            .map(tokio::spawn);

        let pool = self.pool.clone();
        let https = self
            .https
            .map(|https| {
                //info!(?https, "Starting https");
                let addr = https.local_addr();
                stream(https, pool).instrument(debug_span!("HTTPS", local.addr = ?addr))
            })
            .map(|https| serve(routes).serve_incoming(https))
            .map(tokio::spawn);

        let prom = self
            .prom
            .map(|prom| {
                //info!(?http, "Starting http");
                let addr = prom.local_addr();
                prom.instrument(debug_span!("PROM", local.addr = ?addr))
            })
            .map(|prom| serve(metrics).serve_incoming(prom))
            .map(tokio::spawn);

        match (https, http, prom) {
            (Some(https), Some(http), Some(prom)) => {
                tokio::try_join!(https, http, prom).map(|_| ())
            }
            (None, None, None) => Ok(()),

            (Some(https), Some(http), None) => tokio::try_join!(https, http).map(|_| ()),
            (Some(https), None, Some(prom)) => tokio::try_join!(https, prom).map(|_| ()),
            (None, Some(http), Some(prom)) => tokio::try_join!(http, prom).map(|_| ()),

            (Some(https), None, None) => https.await,
            (None, Some(http), None) => http.await,
            (None, None, Some(prom)) => prom.await,
        }?;

        Ok(())
    }
}
