use anyhow::Result;
use futures_util::future::OptionFuture;
use futures_util::FutureExt;
use metrics::{metrics, metrics_wrapper};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::net::ToSocketAddrs;
use tracing::{debug_span, info};
use tracing_futures::Instrument;
use tokio_stream::wrappers::TcpListenerStream;

mod metrics;
mod proxy;
mod routes;
mod tls;

pub struct Api {
    http: Option<TcpListenerStream>,
    https: Option<TcpListenerStream>,
    prom: Option<TcpListenerStream>,
    pool: PgPool,
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
            http: http.map(TcpListenerStream::new),
            https: https.map(TcpListenerStream::new),
            prom: prom.map(TcpListenerStream::new),
            pool,
        })
    }

    #[tracing::instrument(name = "Api::spawn", skip(self))]
    pub async fn spawn(self) -> Result<()> {
        info!("Starting API spawn");

        let routes = routes::routes(self.pool.clone());

        let http = self
            .http
            .map(|http| {
                let addr = http.as_ref().local_addr();
                http.instrument(debug_span!("HTTP", local.addr = ?addr))
            })
            .map(|http| warp::serve(routes.clone()).serve_incoming(http))
            .map(tokio::spawn);

        let pool = self.pool.clone();
        let https = self
            .https
            .map(|https| {
                let addr = https.as_ref().local_addr();
                tls::stream(https, pool).instrument(debug_span!("HTTPS", local.addr = ?addr))
            })
            .map(|https| warp::serve(routes).serve_incoming(https))
            .map(tokio::spawn);

        let prom = self
            .prom
            .map(|prom| {
                let addr = prom.as_ref().local_addr();
                prom.instrument(debug_span!("PROM", local.addr = ?addr))
            })
            .map(|prom| warp::serve(metrics()).serve_incoming(prom))
            .map(tokio::spawn);

        match (https, http, prom) {
            (Some(https), Some(http), Some(prom)) => tokio::try_join!(https, http, prom).map(noop),
            (None, None, None) => Ok(()),

            (Some(https), Some(http), None) => tokio::try_join!(https, http).map(noop),
            (Some(https), None, Some(prom)) => tokio::try_join!(https, prom).map(noop),
            (None, Some(http), Some(prom)) => tokio::try_join!(http, prom).map(noop),

            (Some(https), None, None) => https.await,
            (None, Some(http), None) => http.await,
            (None, None, Some(prom)) => prom.await,
        }?;

        Ok(())
    }
}

fn noop<T>(_: T) {
    ()
}
