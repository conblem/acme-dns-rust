pub mod facade;
pub mod util;

use std::cell::Cell;
use std::sync::{Arc, RwLock};
use std::thread::LocalKey;
use std::time::Instant;
use tokio_rustls::rustls::server::{ClientHello, ResolvesServerCert};
use tokio_rustls::rustls::sign::CertifiedKey;

// this cache is probably useless and introduces unnecessary complexity
// but it's my project after all
thread_local! {
    pub static LAST_CHECKED: Cell<Instant> = Cell::new(Instant::now());
    pub static CACHED: Cell<Option<Arc<CertifiedKey>>> = Cell::new(None);
}

struct SharedCachingCertResolverController {
    shared_key: Arc<RwLock<Arc<CertifiedKey>>>,
}

impl SharedCachingCertResolverController {
    fn new(initial: Arc<CertifiedKey>) -> (Self, Arc<dyn ResolvesServerCert>) {
        let shared_key = Arc::new(RwLock::new(initial));
        let this = Self {
            shared_key: Arc::clone(&shared_key),
        };

        let shared_resolver = SharedCertResolver { shared_key };
        let caching_resolver = CachingCertResolver {
            inner: shared_resolver,
            cached: &CACHED,
            last_checked: &LAST_CHECKED,
        };

        (this, Arc::new(caching_resolver))
    }

    fn set_cert(&self, cert: Arc<CertifiedKey>) {
        let mut shared_key = self.shared_key.write().unwrap();
        *shared_key = cert;
    }
}

struct SharedCertResolver {
    shared_key: Arc<RwLock<Arc<CertifiedKey>>>,
}

impl ResolvesServerCert for SharedCertResolver {
    fn resolve(&self, _client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let guard = self.shared_key.read().unwrap();
        Some(Arc::clone(&*guard))
    }
}

struct CachingCertResolver<T> {
    inner: T,
    cached: &'static LocalKey<Cell<Option<Arc<CertifiedKey>>>>,
    last_checked: &'static LocalKey<Cell<Instant>>,
}

impl<T: ResolvesServerCert> ResolvesServerCert for CachingCertResolver<T> {
    fn resolve(&self, client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let since_last_checked = self.last_checked.with(Cell::get).elapsed().as_millis();
        if since_last_checked > 10 {
            let cached_key = self.cached.with(Cell::take);
            if let Some(cached_key) = cached_key {
                self.cached.with({
                    let cached_key = Some(Arc::clone(&cached_key));
                    move |tls_cached_key| tls_cached_key.set(cached_key)
                });
                return Some(cached_key);
            }
        }

        self.last_checked
            .with(|tls_last_chekced| tls_last_chekced.set(Instant::now()));

        let key = self.inner.resolve(client_hello);
        self.cached.with({
            let key = key.as_ref().map(Arc::clone);
            move |tls_cached_key| tls_cached_key.set(key)
        });
        key
    }
}

#[cfg(test)]
mod tests {
    use crate::CachingCertResolver;
    use rstest::*;
    use rustls_pemfile::Item;
    use std::cell::Cell;
    use std::sync::Arc;
    use std::time::Instant;
    use tokio_rustls::rustls::server::ResolvesServerCert;
    use tokio_rustls::rustls::sign::CertifiedKey;
    use tokio_rustls::rustls::{
        Certificate, ClientConfig, PrivateKey, RootCertStore, ServerConfig,
    };

    #[fixture]
    fn certificate() -> Certificate {
        let mut cert = include_bytes!("../tests/leaf.crt").as_slice();
        let cert = rustls_pemfile::read_one(&mut cert).unwrap().unwrap();
        match cert {
            Item::X509Certificate(cert) => Certificate(cert),
            _ => panic!("expected certificate"),
        }
    }

    #[fixture]
    fn private_key() -> PrivateKey {
        let mut key = include_bytes!("../tests/leaf.key").as_slice();
        let key = rustls_pemfile::read_one(&mut key).unwrap().unwrap();
        match key {
            Item::ECKey(key) => PrivateKey(key),
            _ => panic!("expected certificate"),
        }
    }

    #[fixture]
    fn static_resolver(
        certificate: Certificate,
        private_key: PrivateKey,
    ) -> Arc<dyn ResolvesServerCert> {
        let config = ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(vec![certificate], private_key)
            .unwrap();

        config.cert_resolver
    }

    #[rstest]
    fn test_last_checked_cache(static_resolver: Arc<dyn ResolvesServerCert>) {
        thread_local! {
            static CACHED: Cell<Option<Arc<CertifiedKey>>> = Cell::new(None);
            static LAST_CHECKED: Cell<Instant> = Cell::new(Instant::now());
        }

        let _resolver = CachingCertResolver {
            cached: &CACHED,
            last_checked: &LAST_CHECKED,
            inner: static_resolver,
        };

        let _client_config = ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(RootCertStore::empty())
            .with_no_client_auth();
    }
}
