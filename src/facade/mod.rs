use sqlx::{Database, PgPool, Pool, Postgres};

mod cert;
mod domain;

pub use cert::{Cert, CertFacade, State};
pub use domain::{Domain, DomainDTO, DomainFacade};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

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
pub struct InMemoryFacade {
    certs: Arc<Mutex<HashMap<String, Cert>>>,
    //domains: Mutex<HashMap<String, Domain>>,
}
