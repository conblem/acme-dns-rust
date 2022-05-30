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
}

struct ResolverCacheImpl {
    last_checked: &'static LocalKey<Cell<Instant>>,
}

impl ResolverCacheImpl {
    fn new() -> Self {
        Self {
            last_checked: &LAST_CHECKED,
        }
    }
}

impl ResolverCache for ResolverCacheImpl {
    fn ms_since_last_checked(&self) -> u128 {
        let last_checked = self.last_checked.with(Cell::get);
        Instant::now().duration_since(last_checked).as_millis()
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
        let resolver = Arc::new(SharedCachingCertResolver { inner });

        (this, resolver)
    }

    fn set_cert(&self, cert: Arc<CertifiedKey>) {
        let mut inner = self.inner.write().unwrap();
        *inner = cert;
    }
}

struct SharedCachingCertResolver {
    inner: Arc<RwLock<Arc<CertifiedKey>>>,
}

impl ResolvesServerCert for SharedCachingCertResolver {
    fn resolve(&self, _client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        let last_checked = LAST_CHECKED.with(Cell::get);
        let since_last_checked = Instant::now().duration_since(last_checked);

        // cache has not run out yet
        if since_last_checked.as_millis() < CACHING_TIME_MS {
            let cached = CACHED.with(|cached| cached.take());
            if let Some(cached) = cached {
                // put cached back
                CACHED.with({
                    let cached = Arc::clone(&cached);
                    move |empty_cached| empty_cached.set(Some(cached))
                });
                return Some(cached);
            }
        }

        let lock = self.inner.read().unwrap();
        let key = Arc::clone(&*lock);
        CACHED.with({
            let key = Arc::clone(&key);
            move |empty_cached| empty_cached.set(Some(key))
        });
        LAST_CHECKED.with(|last_checked| last_checked.set(Instant::now()));

        Some(key)
    }
}

#[cfg(test)]
mod tests {}
