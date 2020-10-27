use anyhow::Result;
use futures_util::future::OptionFuture;
use futures_util::FutureExt;
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::net::ToSocketAddrs;
use tracing::{debug_span, error, info};
use tracing_futures::Instrument;
use warp::{http::Response, serve, Filter, Rejection, Reply};

use crate::domain::{Domain, DomainFacade};

mod metrics;
mod tls;

use metrics::{metrics, metrics_wrapper};

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

        let pool = self.pool.clone();
        let routes = warp::path("register")
            .and(warp::post())
            .map(move || pool.clone())
            .and(warp::body::json())
            .and_then(register)
            .with(warp::trace::request())
            .with(warp::wrap_fn(metrics_wrapper));

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
                tls::stream(https, pool).instrument(debug_span!("HTTPS", local.addr = ?addr))
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
            .map(|prom| serve(metrics()).serve_incoming(prom))
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
