use anyhow::Result;
use futures_util::future::OptionFuture;
use futures_util::{FutureExt, TryStreamExt};
use metrics::{metrics, metrics_wrapper};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tracing::info;

use crate::config::Listener;

mod metrics;
mod proxy;
mod routes;
mod tls;

pub async fn new(
    (http, http_proxy): Listener,
    (https, https_proxy): Listener,
    (prom, prom_proxy): Listener,
    pool: PgPool,
) -> Result<()> {
    let http = OptionFuture::from(http.map(TcpListener::bind)).map(Option::transpose);
    let https = OptionFuture::from(https.map(TcpListener::bind)).map(Option::transpose);
    let prom = OptionFuture::from(prom.map(TcpListener::bind)).map(Option::transpose);

    let (http, https, prom) = tokio::try_join!(http, https, prom)?;

    let routes = routes::routes(pool.clone());

    let http = http
        .map(move |http| proxy::wrap(http, http_proxy).try_buffer_unordered(100))
        .map(|http| warp::serve(routes.clone()).serve_incoming(http))
        .map(tokio::spawn);

    let prom = prom
        .map(move |prom| proxy::wrap(prom, prom_proxy).try_buffer_unordered(100))
        .map(|prom| warp::serve(metrics()).serve_incoming(prom))
        .map(tokio::spawn);

    let https = https
        .map(move |https| proxy::wrap(https, https_proxy))
        .map(|https| tls::wrap(https, pool))
        .map(|https| warp::serve(routes).serve_incoming(https))
        .map(tokio::spawn);

    info!("Starting API");
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

fn noop<T>(_: T) {}
