use futures_util::core_reexport::marker::PhantomData;
use generic_std::plug::{PlugLifetime, PlugType};
use serde::{Deserialize, Serialize};
use sqlx::{Database, Executor, MySql, Pool, Postgres};
use uuid::Uuid;

#[derive(sqlx::FromRow, Debug, Serialize, Deserialize)]
pub struct Domain {
    pub id: String,
    pub username: String,
    pub password: String,
    pub txt: Option<String>,
}

impl Default for Domain {
    fn default() -> Self {
        Domain {
            id: Uuid::new_v4().to_simple().to_string(),
            username: Uuid::new_v4().to_simple().to_string(),
            password: bcrypt::hash(
                uuid::Uuid::new_v4().to_simple().to_string(),
                bcrypt::DEFAULT_COST,
            )
            .unwrap(),
            txt: None,
        }
    }
}

pub struct DomainFacade {}

impl DomainFacade {
    pub fn new() -> Self {
        DomainFacade {}
    }

    pub async fn find_by_id<'a, E: Executor<'a, Database = Postgres>>(
        executor: E,
        id: &str,
    ) -> Option<Domain> {
        sqlx::query_as("SELECT * FROM domain WHERE id = $1 LIMIT 1")
            .bind(id)
            .fetch_optional(executor)
            .await
            .unwrap()
    }

    pub async fn create_domain<'a, E: Executor<'a, Database = Postgres>>(
        executor: E,
        domain: &Domain,
    ) {
        sqlx::query("INSERT INTO domain (id, username, password, txt) VALUES ($1, $2, $3, $4)")
            .bind(&domain.id)
            .bind(&domain.username)
            .bind(&domain.password)
            .bind(&domain.txt)
            .execute(executor)
            .await
            .unwrap();
    }

    pub async fn update_domain<'a, E: Executor<'a, Database = Postgres>>(
        executor: E,
        domain: &Domain,
    ) {
        sqlx::query("UPDATE domain SET username = $1, password = $2, txt = $3 WHERE id = $4")
            .bind(&domain.username)
            .bind(&domain.password)
            .bind(&domain.txt)
            .bind(&domain.id)
            .execute(executor)
            .await
            .unwrap();
    }
}
