use acme_lib::persist::{Persist, PersistKey, PersistKind};
use futures_util::TryStreamExt;
use sqlx::{Pool, Postgres};
use sqlx::{Row, Transaction};
use std::sync::Arc;
use tokio::runtime::Runtime;
use tracing::Instrument;

use crate::util::{error, to_i64};

#[derive(Clone)]
pub struct DatabasePersist {
    pool: Pool<Postgres>,
    runtime: Arc<Runtime>,
}

impl DatabasePersist {
    pub fn new(pool: Pool<Postgres>, runtime: &Arc<Runtime>) -> Self {
        DatabasePersist {
            pool,
            runtime: Arc::clone(runtime),
        }
    }
}

fn persist_kind(kind: &PersistKind) -> &'static str {
    match kind {
        PersistKind::Certificate => "crt",
        PersistKind::PrivateKey => "priv_key",
        PersistKind::AccountPrivateKey => "acc_priv_key",
    }
}

impl DatabasePersist {
    async fn exists(
        key: &str,
        realm: i64,
        kind: &str,
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 from acme WHERE key = $1 AND realm = $2 AND kind = $3)",
        )
        .bind(key)
        .bind(realm)
        .bind(kind)
        .fetch_one(transaction)
        .await
    }
}

impl Persist for DatabasePersist {
    #[tracing::instrument(name = "DatabasePersist::put", err, skip(self))]
    fn put<'a>(&self, key: &PersistKey<'a>, value: &[u8]) -> acme_lib::Result<()> {
        let PersistKey { key, realm, kind } = key;
        let realm = to_i64(realm);
        let kind = persist_kind(kind);
        let transaction = self.pool.begin();

        let fut = async move {
            let mut transaction = transaction.await?;
            let query = if DatabasePersist::exists(key, realm, kind, &mut transaction).await? {
                "UPDATE acme SET value = $4 WHERE key = $1 AND realm = $2 AND kind = $3"
            } else {
                "INSERT INTO acme (key, realm, kind, value) VALUES ($1, $2, $3, $4)"
            };

            sqlx::query(query)
                .bind(key)
                .bind(realm)
                .bind(kind)
                .bind(value)
                .execute(&mut transaction)
                .await?;

            transaction.commit().await
        }
        .in_current_span();

        self.runtime.block_on(fut).map_err(error)
    }

    #[tracing::instrument(name = "DatabasePersist::get", err, skip(self))]
    fn get<'a>(&self, key: &PersistKey<'a>) -> acme_lib::Result<Option<Vec<u8>>> {
        let PersistKey { key, realm, kind } = key;

        let mut rows = sqlx::query(
            "SELECT (value) FROM acme WHERE key = $1 AND realm = $2 AND kind = $3 LIMIT 1",
        )
        .bind(key)
        .bind(to_i64(realm))
        .bind(persist_kind(kind))
        .fetch(&self.pool);

        match self.runtime.block_on(rows.try_next().in_current_span()) {
            Ok(Some(row)) => row.try_get("value").map_err(error),
            Ok(None) => Ok(None),
            Err(e) => Err(error(e)),
        }
    }
}
