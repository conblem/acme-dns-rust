use anyhow::{Error, Result};
use futures_util::future::{Future, OptionFuture};
use futures_util::stream::Stream;
use futures_util::{FutureExt, StreamExt, TryFutureExt};
use hyper::server::conn::Http;
use lazy_static::lazy_static;
use metrics::{metrics, metrics_wrapper};
use prometheus::{register_int_counter_vec, register_int_gauge_vec, IntCounterVec, IntGaugeVec};
use sqlx::PgPool;
use std::fmt::Display;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tracing::field::Empty;
use tracing::{error, info, info_span, Instrument};
use warp::{Filter, Rejection, Reply};

use crate::config::Listener;

mod metrics;
mod proxy;
mod routes;
mod tls;

lazy_static! {
    static ref TCP_TOTAL_CONNECTION_COUNTER: IntCounterVec = register_int_counter_vec!(
        "tcp_total_connection_counter",
        "Sum of TCP Connections",
        &["endpoint"]
    )
    .unwrap();
    static ref TCP_OPEN_CONNECTION_COUNTER: IntGaugeVec = register_int_gauge_vec!(
        "tcp_open_connection_counter",
        "Amount of currently open TCP Connections",
        &["endpoint"]
    )
    .unwrap();
}

async fn serve<I, S, T, E, R>(mut io: I, routes: R, endpoint: &str)
where
    I: Stream<Item = Result<S, E>> + Unpin + Send,
    S: Future<Output = Result<T, E>> + Send + 'static,
    T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    E: Into<Error> + Display + Send,
    R: Filter<Error = Rejection> + Clone + Send + 'static,
    R::Extract: Reply,
{
    let service = warp::service(routes);
    let http = Arc::new(Http::new());

    loop {
        let span = info_span!("conn", remote.addr = Empty, remote.real = Empty);
        let conn = match io.next().instrument(span.clone()).await {
            Some(Ok(conn)) => conn.err_into(),
            Some(Err(err)) => {
                span.in_scope(|| error!("{}", err));
                continue;
            }
            None => break,
        };

        TCP_TOTAL_CONNECTION_COUNTER
            .with_label_values(&[endpoint])
            .inc();
        let open_counter = TCP_OPEN_CONNECTION_COUNTER.with_label_values(&[endpoint]);
        open_counter.inc();

        let http = Arc::clone(&http);
        let service = service.clone();

        tokio::spawn(
            async move {
                let conn = conn.await?;
                Ok(http.serve_connection(conn, service).await?)
            }
            .inspect_err(|err: &Error| error!("{}", err))
            .inspect(move |_| open_counter.dec())
            .instrument(span),
        );
    }
}

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
        .map(move |http| proxy::wrap(http, http_proxy))
        .map(|http| serve(http, routes.clone(), "HTTP").instrument(info_span!("HTTP")))
        .map(tokio::spawn);

    let prom = prom
        .map(move |prom| proxy::wrap(prom, prom_proxy))
        .map(|prom| serve(prom, metrics(), "PROM").instrument(info_span!("PROM")))
        .map(tokio::spawn);

    let https = https
        .map(move |https| proxy::wrap(https, https_proxy))
        .map(|https| tls::wrap(https, pool))
        .map(|https| serve(https, routes, "HTTPS").instrument(info_span!("HTTPS")))
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
