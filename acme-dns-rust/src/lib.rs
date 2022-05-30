pub mod facade;
pub mod util;

use std::cell::Cell;
use std::sync::{Arc, RwLock};
use std::thread::LocalKey;
use std::time::Instant;
use tokio_rustls::rustls::server::{ClientHello, ResolvesServerCert};
use tokio_rustls::rustls::sign::CertifiedKey;

const CACHING_TIME_MS: u128 = 10_000;

// this cache is probably useless and introduces unnecessary complexity
// but it's my project after all
thread_local! {
    pub static LAST_CHECKED: Cell<Instant> = Cell::new(Instant::now());
    pub static CACHED: Cell<Option<Arc<CertifiedKey>>> = Cell::new(None);
}

trait ResolverCache: Send + Sync {
    fn ms_since_last_checked(&self) -> u128;
    fn get_cached(&self) -> Option<Arc<CertifiedKey>>;
    fn set_cached(&self, key: Arc<CertifiedKey>);
}

struct ResolverCacheImpl {
    last_checked: &'static LocalKey<Cell<Instant>>,
    cached: &'static LocalKey<Cell<Option<Arc<CertifiedKey>>>>,
}

impl ResolverCacheImpl {
    fn new() -> Self {
        Self {
            last_checked: &LAST_CHECKED,
            cached: &CACHED,
        }
    }
}

impl ResolverCache for ResolverCacheImpl {
    fn ms_since_last_checked(&self) -> u128 {
        let last_checked = self.last_checked.with(Cell::get);
        Instant::now().duration_since(last_checked).as_millis()
    }

    fn get_cached(&self) -> Option<Arc<CertifiedKey>> {
        let cached = self.cached.with(Cell::take);
        if let Some(cached) = &cached {
            let cached = Some(Arc::clone(cached));
            self.cached.with(move |tls_cached| tls_cached.set(cached));
        }

        cached
    }

    fn set_cached(&self, key: Arc<CertifiedKey>) {
        self.cached
            .with(move |tls_cached| tls_cached.set(Some(key)));
        self.last_checked
            .with(|tls_last_checked| tls_last_checked.set(Instant::now()));
    }
}

struct SharedCachingCertResolverController {
    inner: Arc<RwLock<Arc<CertifiedKey>>>,
}

impl SharedCachingCertResolverController {
    fn new() -> (Self, Arc<dyn ResolvesServerCert>) {
        let inner = Arc::new(RwLock::new(Arc::new(todo!())));
        let this = Self {
            inner: Arc::clone(&inner),
        };
        let resolver = Arc::new(SharedCachingCertResolver {
            inner,
            cache: ResolverCacheImpl {
                last_checked: &LAST_CHECKED,
                cached: &CACHED,
            },
        });

        (this, resolver)
    }

    fn set_cert(&self, cert: Arc<CertifiedKey>) {
        let mut inner = self.inner.write().unwrap();
        *inner = cert;
    }
}

struct SharedCachingCertResolver<T> {
    inner: Arc<RwLock<Arc<CertifiedKey>>>,
    cache: T,
}

impl<T: ResolverCache> ResolvesServerCert for SharedCachingCertResolver<T> {
    fn resolve(&self, _client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        if self.cache.ms_since_last_checked() < CACHING_TIME_MS {
            if let Some(cached_key) = self.cache.get_cached() {
                return Some(cached_key);
            }
        }
        let guard = self.inner.read().unwrap();
        let key = Arc::clone(&*guard);
        self.cache.set_cached(Arc::clone(&key));
        Some(key)
    }
}

#[cfg(test)]
mod tests {}
