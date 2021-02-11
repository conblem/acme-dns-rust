use parking_lot::{Mutex, MutexGuard};
use sqlx::{Database, PgPool, Pool, Postgres};
use std::collections::HashMap;
use std::sync::Arc;

mod cert;
mod domain;

pub use cert::{Cert, CertFacade, State};
pub use domain::{Domain, DomainDTO, DomainFacade};

#[derive(Debug)]
pub struct DatabaseFacade<DB: Database> {
    pool: Pool<DB>,
}

impl<DB: Database> Clone for DatabaseFacade<DB> {
    fn clone(&self) -> Self {
        DatabaseFacade {
            pool: self.pool.clone(),
        }
    }
}

impl From<PgPool> for DatabaseFacade<Postgres> {
    fn from(pool: PgPool) -> Self {
        DatabaseFacade { pool }
    }
}

#[derive(Clone, Default)]
pub struct InMemoryFacade(Arc<Mutex<InMemoryFacadeInner>>);

#[derive(Clone, Default)]
struct InMemoryFacadeInner {
    certs: HashMap<String, Cert>,
    domains: HashMap<String, Domain>,
}

type InMemoryFacadeGuard<'a> = MutexGuard<'a, InMemoryFacadeInner>;
