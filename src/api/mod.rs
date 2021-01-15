use anyhow::{Error, Result};
use futures_util::future::OptionFuture;
use futures_util::stream::{Stream, TryStream};
use futures_util::FutureExt;
use metrics::{metrics, metrics_wrapper};
use sqlx::PgPool;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::io::{Error as IoError, Result as IoResult};
use tokio::net::TcpListener;
use tokio::net::ToSocketAddrs;
use tracing::info;

use crate::config::ProxyProtocol;

mod metrics;
mod proxy;
mod routes;
mod tls;

pub struct Api<H, P, S> {
    http: Option<H>,
    https: Option<S>,
    prom: Option<P>,
    pool: PgPool,
}

pub async fn new<A: ToSocketAddrs>(
    (http, http_proxy): (Option<A>, ProxyProtocol),
    (https, https_proxy): (Option<A>, ProxyProtocol),
    (prom, prom_proxy): (Option<A>, ProxyProtocol),
    pool: PgPool,
) -> Result<
    Api<
        impl Stream<Item = IoResult<impl AsyncRead + AsyncWrite + Send + Unpin + 'static>> + Send,
        impl Stream<Item = IoResult<impl AsyncRead + AsyncWrite + Send + Unpin + 'static>> + Send,
        impl Stream<Item = Result<impl AsyncRead + AsyncWrite + Send + Unpin + 'static>> + Send,
    >,
> {
    let http = OptionFuture::from(http.map(TcpListener::bind)).map(Option::transpose);
    let https = OptionFuture::from(https.map(TcpListener::bind)).map(Option::transpose);
    let prom = OptionFuture::from(prom.map(TcpListener::bind)).map(Option::transpose);

    let (http, https, prom) = tokio::try_join!(http, https, prom)?;

    let http = http.map(move |http| proxy::wrap(http, http_proxy));
    let prom = prom.map(move |prom| proxy::wrap(prom, prom_proxy));
    let https = https
        .map(move |https| proxy::wrap(https, https_proxy))
        .map(|https| tls::wrap(https, pool.clone()));

    Ok(Api {
        http,
        https,
        prom,
        pool,
    })
}

impl<H, P, S> Api<H, P, S>
where
    H: TryStream<Error = IoError> + Send + Unpin + 'static,
    H::Ok: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    P: TryStream<Error = IoError> + Send + Unpin + 'static,
    P::Ok: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    S: TryStream<Error = Error> + Send + Unpin + 'static,
    S::Ok: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    #[tracing::instrument(name = "Api::spawn", skip(self))]
    pub async fn spawn(self) -> Result<()> {
        info!("Starting API spawn");

        let routes = routes::routes(self.pool.clone());

        let http = self
            .http
            .map(|http| warp::serve(routes.clone()).serve_incoming(http))
            .map(tokio::spawn);

        let https = self
            .https
            .map(|https| warp::serve(routes).serve_incoming(https))
            .map(tokio::spawn);

        let prom = self
            .prom
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

fn noop<T>(_: T) {}
