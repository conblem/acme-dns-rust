use parking_lot::{Mutex, MutexGuard};
use sqlx::{Database, PgPool, Pool, Postgres};
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) mod cert;
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

#[cfg(test)]
mod tests {
    use once_cell::sync::OnceCell;
    use sqlx::PgPool;
    use std::ops::Deref;
    use testcontainers::clients::Cli;
    use testcontainers::images::postgres::Postgres;
    use testcontainers::Container;

    use crate::setup_database;

    static CLIENT: OnceCell<Cli> = OnceCell::new();

    pub(super) struct TestPool {
        pool: PgPool,
        _container: Container<'static, Postgres>,
    }

    impl Deref for TestPool {
        type Target = PgPool;

        fn deref(&self) -> &Self::Target {
            &self.pool
        }
    }

    pub(super) async fn test_pool() -> TestPool {
        let client = CLIENT.get_or_init(Cli::default);
        let container = client.run(Postgres::default());

        let connection_string = &format!(
            "postgres://postgres:postgres@localhost:{}/postgres",
            container.get_host_port(5432)
        );

        let pool = setup_database(connection_string).await.unwrap();
        TestPool {
            pool,
            _container: container,
        }
    }
}
