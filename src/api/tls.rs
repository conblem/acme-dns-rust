use anyhow::{anyhow, Error, Result};
use futures_util::stream::{repeat, Stream};
use futures_util::{StreamExt, TryStreamExt};
use parking_lot::RwLock;
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::{NoClientAuth, ServerConfig};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::TlsAcceptor;
use tracing::field::{display, Empty};
use tracing::{debug_span, error, info, Instrument};

use super::proxy::PeerAddr;
use crate::cert::{Cert, CertFacade};
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

pub(super) fn stream<S, O, E, P>(
    listener: S,
    pool: PgPool,
) -> impl Stream<Item = Result<impl AsyncRead + AsyncWrite + Send + Unpin + 'static, Error>> + Send
where
    P: std::error::Error + Send + Sync,
    S: Stream<Item = Result<O, E>> + Send,
    O: AsyncRead + AsyncWrite + Send + Unpin + PeerAddr<P> + 'static,
    E: Into<Error> + Send,
{
    let acceptor = Acceptor::new(pool);

    listener
        .zip(repeat(acceptor))
        .map(|(conn, acceptor)| conn.map(|c| (c, acceptor)))
        .err_into()
        .map_ok(|(mut conn, acceptor)| async move {
            let span = debug_span!("TLS", remote.addr = Empty);
            let addr = conn.proxy_peer().instrument(span.clone());
            match addr.await {
                Ok(addr) => {
                    span.record("remote.addr", &display(addr));
                }
                Err(e) => {
                    span.record("remote.addr", &display("Unknown"));
                    span.in_scope(|| error!("Could net get remote.addr: {}", e));
                }
            }
            let tls = acceptor.load_cert().instrument(span.clone()).await?;
            Ok(tls.accept(conn).instrument(span).await?)
        })
        .try_buffer_unordered(100)
        .inspect_err(|err| error!("Stream error: {:?}", err))
        .filter(|stream| futures_util::future::ready(stream.is_ok()))
        .into_stream()
}