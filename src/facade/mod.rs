use parking_lot::Mutex;
use sqlx::{Database, PgPool, Pool, Postgres};
use std::collections::HashMap;
use std::fs::read_to_string;
use std::sync::Arc;

mod cert;
mod domain;

use crate::util::{now, to_i64};
pub use cert::{Cert, CertFacade, State};
pub use domain::{Domain, DomainDTO, DomainFacade};
use std::path::Path;

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

#[derive(Clone)]
pub(super) struct TestFacade {
    certs: Arc<Mutex<HashMap<String, Cert>>>,
    //domains: Mutex<HashMap<String, Domain>>,
}

impl Default for TestFacade {
    fn default() -> Self {
        let cert = read_to_string(Path::new(file!()).with_file_name("cert.crt"));
        let private = read_to_string(Path::new(file!()).with_file_name("key.key"));

        let cert = Cert {
            id: "1".to_owned(),
            update: to_i64(&now()),
            state: State::Ok,
            cert: Some(cert.unwrap()),
            private: Some(private.unwrap()),
            domain: "acme-dns-rust.com".to_owned(),
        };

        let mut certs = HashMap::new();
        certs.insert("1".to_owned(), cert);

        TestFacade {
            certs: Arc::new(Mutex::new(certs)),
        }
    }
}
