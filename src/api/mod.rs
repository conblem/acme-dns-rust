use anyhow::{Result, Error};
use futures_util::future::OptionFuture;
use futures_util::stream::{BoxStream, Stream};
use futures_util::{FutureExt, StreamExt, TryStreamExt};
use metrics::{metrics, metrics_wrapper};
use sqlx::PgPool;
use tokio::io::{Error as IoError, Result as IoResult};
use tokio::net::TcpListener;
use tokio::net::ToSocketAddrs;
use tokio_stream::wrappers::TcpListenerStream;
use tracing::{debug_span, info};

use crate::api::proxy::{PeerAddr, ProxyStream};
use futures_util::io::{AsyncRead, AsyncWrite};

mod metrics;
mod proxy;
mod routes;
mod tls;

type Connection = Box<dyn PeerAddr<IoError> + Send + Unpin + 'static>;
type Listener = BoxStream<'static, IoResult<Connection>>;

pub struct Api<H, S> {
    http: Option<H>,
    https: Option<S>,
    prom: Option<H>,
    pool: PgPool,
}

impl <H, S> Api<H, S> {
    fn prepare_listener(listener: TcpListener, proxy: bool) -> Listener {
        let listener = match listener {
            Some(listener) => TcpListenerStream::new(listener),
            None => return None,
        };

        let mapper = match proxy {
            true => |stream| Box::new(ProxyStream::from(stream)) as Connection,
            false => |stream| Box::new(stream) as Connection
        };

        let listener = listener
            .map_ok(mapper)
            .boxed();

        Some(listener)
    }

    pub async fn new<A: ToSocketAddrs>(
        (http, http_proxy): (Option<A>, bool),
        (https, https_proxy): (Option<A>, bool),
        (prom, prom_proxy): (Option<A>, bool),
        pool: PgPool,
    ) -> Result<Api<
        impl Stream<Item = IoResult<impl AsyncRead + AsyncWrite + Send + Unpin + 'static>> + Send,
        impl Stream<Item = Result<impl AsyncRead + AsyncWrite + Send + Unpin + 'static, Error>> + Send
    >> {
        let http = OptionFuture::from(http.map(TcpListener::bind)).map(Option::transpose);
        let https = OptionFuture::from(https.map(TcpListener::bind)).map(Option::transpose);
        let prom = OptionFuture::from(prom.map(TcpListener::bind)).map(Option::transpose);

        let (http, https, prom) = tokio::try_join!(http, https, prom)?;

        let http = Api::prepare_listener(http, http_proxy);
        let https = Api::prepare_listener(https, https_proxy);
        let prom = Api::prepare_listener(prom, prom_proxy);

        Ok(Api {
            http: proxy::wrap(http).try_buffer_unordered(100),
            https: tls::stream(https, pool.clone()),
            prom: proxy::wrap(prom).try_buffer_unordered(100),
            pool,
        })
    }

    #[tracing::instrument(name = "Api::spawn", skip(self))]
    pub async fn spawn(self) -> Result<()> {
        info!("Starting API spawn");

        let routes = routes::routes(self.pool.clone());

        let http = self
            .http
            .map(|http|
                proxy::wrap(http).try_buffer_unordered(100)
            )
            .map(|http| warp::serve(routes.clone()).serve_incoming(http))
            .map(tokio::spawn);

        let pool = self.pool.clone();
        let https = self
            .https
            .map(|https| {
                //let addr = https.as_ref().local_addr();
                tls::stream(https, pool)
                //.instrument(debug_span!("HTTPS", local.addr = ?addr))
            })
            .map(|https| warp::serve(routes).serve_incoming(https))
            .map(tokio::spawn);

        let prom = self
            .prom
            .map(|prom| {
                //let addr = prom.as_ref().local_addr();
                prom
                //.instrument(debug_span!("PROM", local.addr = ?addr))
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
