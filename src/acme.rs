use acme_lib::persist::{Persist, PersistKey, PersistKind};
use sqlx::Row;
use sqlx::{Pool, Postgres};
use tokio::runtime::Handle;
use tokio::stream::StreamExt;

use crate::util::to_i64;

#[derive(Clone)]
pub struct DatabasePersist {
    pool: Pool<Postgres>,
    handle: Handle,
}

impl DatabasePersist {
    pub fn new(pool: Pool<Postgres>, handle: &Handle) -> Self {
        DatabasePersist {
            pool,
            handle: handle.clone(),
        }
    }
}

fn persist_kind(kind: &PersistKind) -> &str {
    match kind {
        PersistKind::Certificate => "crt",
        PersistKind::PrivateKey => "key",
        PersistKind::AccountPrivateKey => "key",
    }
}

impl Persist for DatabasePersist {
    fn put<'a>(&self, key: &PersistKey<'a>, value: &[u8]) -> acme_lib::Result<()> {
        let PersistKey { realm, kind, key } = key;

        let query =
            sqlx::query("INSERT INTO acme (key, realm, kind, value) VALUES ($1, $2, $3, $4)")
                .bind(key)
                .bind(to_i64(realm))
                .bind(persist_kind(kind))
                .bind(value)
                .execute(&self.pool);

        self.handle
            .block_on(query)
            .map_err(|err| acme_lib::Error::from(err.to_string()))?;

        Ok(())
    }

    fn get<'a>(&self, key: &PersistKey<'a>) -> acme_lib::Result<Option<Vec<u8>>> {
        let PersistKey { realm, kind, key } = key;

        let mut rows =
            sqlx::query("SELECT (value) FROM acme WHERE key = $1, realm = $2, kind = $3 LIMIT 1")
                .bind(key)
                .bind(to_i64(realm))
                .bind(persist_kind(kind))
                .fetch(&self.pool);

        match self.handle.block_on(rows.try_next()) {
            Ok(Some(row)) => row.try_get("value"),
            Ok(None) => return Ok(None),
            Err(e) => return Err(acme_lib::Error::from(e.to_string())),
        }
        .map_err(|err| acme_lib::Error::from(err.to_string()))
    }
}
