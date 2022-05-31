pub mod facade;
pub mod util;

use std::cell::Cell;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
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
    type Guard: DerefMut<Target = Option<Arc<CertifiedKey>>>;

    fn ms_since_last_checked(&self) -> u128;
    fn set_last_checked(&self);
    fn key(&self) -> Self::Guard;
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

struct ResolverCacheGuard {
    cached: &'static LocalKey<Cell<Option<Arc<CertifiedKey>>>>,
    key: Option<Arc<CertifiedKey>>,
    _not_send_and_sync: PhantomData<*const ()>,
}

impl Drop for ResolverCacheGuard {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.cached
                .with(move |tls_cached| tls_cached.set(Some(key)));
        }
    }
}

impl Deref for ResolverCacheGuard {
    type Target = Option<Arc<CertifiedKey>>;

    fn deref(&self) -> &Self::Target {
        &self.key
    }
}

impl DerefMut for ResolverCacheGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.key
    }
}

impl ResolverCache for ResolverCacheImpl {
    type Guard = ResolverCacheGuard;

    fn ms_since_last_checked(&self) -> u128 {
        let last_checked = self.last_checked.with(Cell::get);
        Instant::now().duration_since(last_checked).as_millis()
    }

    fn set_last_checked(&self) {
        self.last_checked
            .with(|tls_last_checked| tls_last_checked.set(Instant::now()));
    }

    fn key(&self) -> Self::Guard {
        let key = self.cached.with(Cell::take);

        ResolverCacheGuard {
            cached: self.cached,
            key,
            _not_send_and_sync: PhantomData,
        }
    }
}

struct SharedCachingCertResolverController {
    inner: Arc<RwLock<Arc<CertifiedKey>>>,
}

impl SharedCachingCertResolverController {
    fn new(initial: Arc<CertifiedKey>) -> (Self, Arc<dyn ResolvesServerCert>) {
        let inner = Arc::new(RwLock::new(initial));
        let this = Self {
            inner: Arc::clone(&inner),
        };
        let resolver = SharedCachingCertResolver {
            inner,
            cache: ResolverCacheImpl {
                last_checked: &LAST_CHECKED,
                cached: &CACHED,
            },
        };

        (this, Arc::new(resolver))
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
        let mut cached_key = self.cache.key();
        let cached_key = cached_key.deref_mut();

        if self.cache.ms_since_last_checked() < CACHING_TIME_MS {
            if let Some(cached_key) = cached_key {
                return Some(Arc::clone(cached_key));
            }
        }

        let guard = self.inner.read().unwrap();

        self.cache.set_last_checked();
        // check if we already have the newest key in the cache
        match cached_key {
            Some(cached_key) if Arc::ptr_eq(cached_key, &*guard) => {
                return Some(Arc::clone(cached_key))
            }
            _ => {}
        }

        *cached_key = Some(Arc::clone(&*guard));
        Some(Arc::clone(&*guard))
    }
}

#[cfg(test)]
mod tests {}
