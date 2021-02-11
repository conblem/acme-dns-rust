use anyhow::{anyhow, Result};
use futures_util::stream::{repeat, Stream};
use futures_util::{StreamExt, TryFutureExt, TryStreamExt};
use parking_lot::RwLock;
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::{NoClientAuth, ServerConfig};
use std::future::Future;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, Result as IoResult};
use tokio_rustls::TlsAcceptor;
use tracing::{error, info};

use crate::facade::{Cert, CertFacade};
use crate::util::to_u64;

pub fn wrap_higher<L, I, S, A, F>(
    listener: L,
    acceptor: A,
) -> impl Stream<
    Item = Result<
        impl Future<Output = Result<impl AsyncRead + AsyncWrite + Send + Unpin + 'static>>,
    >,
> + Send
where
    L: Stream<Item = IoResult<I>> + Send + 'static,
    I: Future<Output = IoResult<S>> + Send + 'static,
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    A: Fn() -> F + Clone + Send + 'static,
    F: Future<Output = Result<TlsAcceptor>>,
{
    listener
        .err_into()
        .zip(repeat(acceptor))
        .map(|(conn, acceptor)| conn.map(|c| (c, acceptor)))
        .map_ok(|(conn, acceptor)| async move {
            let (conn, tls) = tokio::try_join!(conn.err_into(), acceptor())?;
            Ok(tls.accept(conn).await?)
        })
}

pub fn wrap<L, I, S, F>(
    listener: L,
    facade: F,
) -> impl Stream<
    Item = Result<
        impl Future<Output = Result<impl AsyncRead + AsyncWrite + Send + Unpin + 'static>>,
    >,
> + Send
where
    L: Stream<Item = IoResult<I>> + Send + 'static,
    I: Future<Output = IoResult<S>> + Send + 'static,
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    F: CertFacade + Send + Sync + 'static,
{
    let acceptor = acceptor(facade);
    wrap_higher(listener, acceptor)
}

trait Func: Fn() -> <Self as Func>::Output {
    type Output;
}

impl<F, O> Func for F
where
    F: Fn() -> O,
{
    type Output = O;
}

fn acceptor<F>(
    facade: F,
) -> impl Func<Output = impl Future<Output = Result<TlsAcceptor>>> + Clone + 'static
where
    F: CertFacade + 'static,
{
    let server_config = ServerConfig::new(NoClientAuth::new());

    let config = RwLock::new((None, Arc::new(server_config)));
    let wrapper = Arc::new((facade, config));

    move || {
        let wrapper = Arc::clone(&wrapper);
        async move {
            let (facade, config) = &*wrapper;
            load_cert(facade, config).await
        }
    }
}

async fn load_cert<F>(
    facade: &F,
    config: &RwLock<(Option<Cert>, Arc<ServerConfig>)>,
) -> Result<TlsAcceptor>
where
    F: CertFacade + 'static,
{
    let new_cert = facade.first_cert().await;

    let db_cert = match (new_cert, &*config.read()) {
        (Ok(Some(new_cert)), (cert, _)) if Some(&new_cert) != cert.as_ref() => new_cert,
        (_, (_, server_config)) => {
            info!("Using existing TLS Config");
            return Ok(TlsAcceptor::from(Arc::clone(server_config)));
        }
    };
    info!(timestamp = to_u64(&db_cert.update), "Found new cert");

    let server_config = match create_server_config(&db_cert) {
        Ok(server_config) => server_config,
        Err(e) => {
            error!("{}", e);
            let (_, server_config) = &*config.read();
            return Ok(TlsAcceptor::from(Arc::clone(server_config)));
        }
    };

    *config.write() = (Some(db_cert), Arc::clone(&server_config));
    info!("Created new TLS config");
    Ok(TlsAcceptor::from(server_config))
}

fn create_server_config(db_cert: &Cert) -> Result<Arc<ServerConfig>> {
    let (private, cert) = match (&db_cert.private, &db_cert.cert) {
        (Some(private), Some(cert)) => (private, cert),
        _ => return Err(anyhow!("Cert has no Cert or Private")),
    };

    let mut privates = pkcs8_private_keys(&mut private.as_bytes())
        .map_err(|_| anyhow!("Private is invalid {:?}", private))?;
    let private = privates
        .pop()
        .ok_or_else(|| anyhow!("Private Vec is empty {:?}", privates))?;

    let cert = certs(&mut cert.as_bytes()).map_err(|_| anyhow!("Cert is invalid {:?}", cert))?;

    let mut config = ServerConfig::new(NoClientAuth::new());
    config.set_single_cert(cert, private)?;
    config.set_protocols(&["h2".into(), "http/1.1".into()]);

    Ok(Arc::new(config))
}
