use anyhow::{Error, Result};
use futures_util::future::OptionFuture;
use futures_util::stream::{Stream, TryStream};
use futures_util::FutureExt;
use metrics::{metrics, metrics_wrapper};
use sqlx::PgPool;
use tokio::io::Error as IoError;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::net::ToSocketAddrs;
use tracing::info;

use crate::api::proxy::{ProxyListener, ToProxyListener};
use crate::config::ProxyProtocol;

mod metrics;
mod proxy;
mod routes;
mod tls;

pub struct Api<S> {
    http: Option<ProxyListener<IoError>>,
    https: Option<S>,
    prom: Option<ProxyListener<IoError>>,
    pool: PgPool,
}

pub async fn new<A: ToSocketAddrs>(
    (http, http_proxy): (Option<A>, ProxyProtocol),
    (https, https_proxy): (Option<A>, ProxyProtocol),
    (prom, prom_proxy): (Option<A>, ProxyProtocol),
    pool: PgPool,
) -> Result<
    Api<
        impl Stream<Item = Result<impl AsyncRead + AsyncWrite + Send + Unpin + 'static, Error>> + Send,
    >,
> {
    let http = OptionFuture::from(http.map(TcpListener::bind)).map(Option::transpose);
    let https = OptionFuture::from(https.map(TcpListener::bind)).map(Option::transpose);
    let prom = OptionFuture::from(prom.map(TcpListener::bind)).map(Option::transpose);

    let (http, https, prom) = tokio::try_join!(http, https, prom)?;

    let http = http.map(|http| http.source(http_proxy));
    let https = https
        .map(|https| https.source(https_proxy))
        .map(|listener| tls::stream(listener, pool.clone()));
    let prom = prom.map(|prom| prom.source(prom_proxy));

    Ok(Api {
        http,
        https,
        prom,
        pool,
    })
}

impl<S> Api<S>
where
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

fn noop<T>(_: T) {
    ()
}
