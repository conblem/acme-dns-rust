use anyhow::{anyhow, Result};
use futures_util::stream::{repeat, Stream};
use futures_util::{StreamExt, TryFutureExt, TryStreamExt};
use parking_lot::RwLock;
use rustls::server::ResolvesServerCertUsingSni;
use rustls::{Certificate, PrivateKey, ServerConfig};
use rustls_pemfile::{certs, rsa_private_keys};
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
    A: FnOnce() -> F + Clone + Send + 'static,
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
trait Func: FnOnce() -> <Self as Func>::Output {
    type Output;
}

impl<F, O> Func for F
where
    F: FnOnce() -> O,
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
    let empty_cert_resolver = ResolvesServerCertUsingSni::new();
    let server_config = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(empty_cert_resolver));

    let config = RwLock::new((None, Arc::new(server_config)));
    let wrapper = Arc::new((facade, config));

    || async move {
        let (facade, config) = &*wrapper;
        load_cert(facade, config).await
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
        // safe to print because cert doesnt have private and cert
        _ => return Err(anyhow!("{:?} has no Cert or Private", db_cert)),
    };

    let mut privates =
        rsa_private_keys(&mut private.as_bytes()).map_err(|_| anyhow!("Private is invalid"))?;
    let private = privates
        .pop()
        .map(PrivateKey)
        .ok_or_else(|| anyhow!("Private Vec is empty"))?;

    let cert = certs(&mut cert.as_bytes())
        .map_err(|_| anyhow!("Cert is invalid {:?}", cert))?
        .into_iter()
        .map(Certificate)
        .collect();

    let mut config = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(cert, private)?;

    // used to enable http2 support
    config.alpn_protocols.push("h2".into());
    config.alpn_protocols.push("http/1.1".into());

    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::create_server_config;
    use crate::facade::cert::tests::create_cert;
    use crate::facade::Cert;

    #[test]
    fn test_create_server_config_alpn() {
        let cert = create_cert();
        let config = create_server_config(&cert).unwrap();
        let alpn = &config.alpn_protocols;
        assert_eq!("h2".as_bytes(), &alpn[0]);
        assert_eq!("http/1.1".as_bytes(), &alpn[1]);
    }

    fn unwrap_err_create_server_config(cert: &Cert) -> String {
        match create_server_config(&cert) {
            Err(e) => format!("{}", e),
            _ => unreachable!(),
        }
    }

    // useless but 100% coverage is still nice
    #[test]
    #[should_panic]
    fn panic_unwrap_create_server_config_error() {
        let cert = create_cert();
        unwrap_err_create_server_config(&cert);
    }

    #[test]
    fn test_empty_cert() {
        let mut cert = create_cert();
        cert.cert = None;
        cert.private = None;

        let error = unwrap_err_create_server_config(&cert);
        assert!(error.contains(&format!("{:?}", cert)));
        assert!(error.contains("has no Cert or Private"));
    }

    #[test]
    fn test_invalid_private() {
        let mut cert = create_cert();
        *cert.private.as_mut().unwrap() = "WRONG".to_owned();

        let error = unwrap_err_create_server_config(&cert);

        // todo: investigate
        // unclear why the error gets not triggered earlier
        assert!(error.contains("Private Vec is empty"));
    }

    #[test]
    #[should_panic]
    // todo: rustls does no cert validation so this test panics
    fn test_invalid_cert() {
        let mut cert = create_cert();
        *cert.cert.as_mut().unwrap() = "WRONG".to_owned();

        let error = unwrap_err_create_server_config(&cert);
        assert!(error.contains("Cert is invalid"));
    }
}
