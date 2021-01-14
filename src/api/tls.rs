use anyhow::{anyhow, Error, Result};
use futures_util::stream::{repeat, ErrInto, Stream, Zip};
use futures_util::{StreamExt, TryStreamExt, FutureExt};
use parking_lot::RwLock;
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::{NoClientAuth, ServerConfig};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, Error as IoError};
use tokio_rustls::{TlsAcceptor, Accept};
use tracing::{error, info};

use super::proxy::ProxyListener;
use crate::cert::{Cert, CertFacade};
use crate::util::to_u64;
use futures_util::stream::Repeat;
use std::future::Future;

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
        let new_cert = CertFacade::first_cert(&self.pool).await;

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

trait Tuple {
    type A;
    type B;
}

impl<A, B> Tuple for (A, B) {
    type A = A;
    type B = B;
}

trait ResultInherit {
    type Ok;
    type Error;
}

impl<T, E> ResultInherit for Result<T, E> {
    type Ok = T;
    type Error = E;
}

trait ToTlsListener {
    fn tls(self, pool: PgPool) -> ProxyListener<IoError>;
}

type TlsListenerBeforeMap = Zip<ErrInto<ProxyListener<IoError>, Error>, Repeat<Arc<Acceptor>>>;

type MapResultOutput = Result<
    (
        <<<TlsListenerBeforeMap as Stream>::Item as Tuple>::A as ResultInherit>::Ok,
        Arc<Acceptor>,
    ),
    <<<TlsListenerBeforeMap as Stream>::Item as Tuple>::A as ResultInherit>::Error,
>;

type MapResult = fn(<TlsListenerBeforeMap as Stream>::Item) -> MapResultOutput;

fn map_result((listener, acceptor): <TlsListenerBeforeMap as Stream>::Item) -> MapResultOutput {
    listener.map(|listener| (listener, acceptor))
}

pub(super) fn stream(
    listener: ProxyListener<IoError>,
    pool: PgPool,
) -> impl Stream<Item = Result<impl AsyncRead + AsyncWrite + Send + Unpin + 'static, Error>> + Send
{
    let acceptor = Acceptor::new(pool);

    listener
        .err_into()
        .zip(repeat(acceptor))
        .map(|(conn, acceptor)| conn.map(|c| (c, acceptor)))
        .map_ok(|(conn, acceptor)| async move {
            let tls = acceptor.load_cert().await?;
            Ok(tls.accept(conn).await?)
        })
        .try_buffer_unordered(100)
        .inspect_err(|err| error!("Stream error: {:?}", err))
        .filter(|stream| futures_util::future::ready(stream.is_ok()))
        .into_stream()
}

enum PrepareTlsFuture {
    Start(ProxyListener<IoError>, Arc<Acceptor>),
    LoadingCert()

}
struct PrepareTlsFuture {
    listener: Option<ProxyListener<IoError>>,
    accept: Option<Accept<ProxyListener<IoError>>>
    acceptor: Arc<Acceptor>
}

impl Future for PrepareTlsFuture {
    type Output = Result<Accept<ProxyListener<IoError>>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let tls = match this.acceptor.load_cert().poll_unpin(cx)? {
            Err(e) => return Poll::Ready(Err(e)),
            Ok(tls) => tls,
        };

        let stream = match this.listener.take() {
            Some(stream) => stream,
            None => unreachable!("Future cannot be polled anymore"),
        };

        match tls.accept(stream)
    }
}