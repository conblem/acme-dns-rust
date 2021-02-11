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
    wrap_higher(listener, acceptor(facade))
}

// we use a closure which returns a future as an abstraction
// for the acceptor to remove the need for a trait, so there
// is no boxing needed
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

// used for expressing a closure with an impl return
trait Func: Fn() -> <Self as Func>::Output {
    type Output;
}

impl<F, O> Func for F
where
    F: Fn() -> O,
{
    type Output = O;
}

// Func trait is only used here as it inherits Fn
// we just use the Fn trait for input arguments
fn acceptor<F>(
    facade: F,
) -> impl Func<Output = impl Future<Output = Result<TlsAcceptor>>> + Clone + 'static
where
    F: CertFacade + 'static,
{
    let server_config = ServerConfig::new(NoClientAuth::new());

    let config = RwLock::new((None, Arc::new(server_config)));
    let wrapper = Arc::new((facade, config));

    // workarround to make closure Fn instead of FnOnce
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
    // get current certificate from database
    let new_cert = facade.first_cert().await;

    let db_cert = match (new_cert, &*config.read()) {
        // if the current cert is not the same as we have cached
        // create a new server config
        (Ok(Some(new_cert)), (cert, _)) if Some(&new_cert) != cert.as_ref() => new_cert,
        // reuse existing server config because cached cert is already the newest
        (_, (_, server_config)) => {
            info!("Using existing TLS Config");
            return Ok(TlsAcceptor::from(Arc::clone(server_config)));
        }
    };
    info!(timestamp = to_u64(&db_cert.update), "Found new cert");

    let server_config = match create_server_config(&db_cert) {
        Ok(server_config) => server_config,
        // todo: think about if we should return old cert
        // in case of error also reuse the old server config
        // maybe an old expired certificate
        Err(e) => {
            error!("{}", e);
            let (_, server_config) = &*config.read();
            return Ok(TlsAcceptor::from(Arc::clone(server_config)));
        }
    };

    // cache cert for future comparison together with server config
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
    // used to enable http2 support
    config.set_protocols(&["h2".into(), "http/1.1".into()]);

    Ok(Arc::new(config))
}
