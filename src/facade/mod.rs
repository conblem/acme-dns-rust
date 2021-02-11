use parking_lot::Mutex;
use sqlx::{Database, PgPool, Pool, Postgres};
use std::collections::HashMap;
use std::sync::Arc;

mod cert;
mod domain;

pub use cert::{Cert, CertFacade, State};
pub use domain::{Domain, DomainDTO, DomainFacade};

#[derive(Debug)]
pub(super) struct DatabaseFacade<DB: Database> {
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

#[derive(Default, Clone)]
pub(super) struct TestFacade {
    certs: Arc<Mutex<HashMap<String, Cert>>>,
    //domains: Mutex<HashMap<String, Domain>>,
}
