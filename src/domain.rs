use sqlx::{Pool, Database, Postgres, FromRow};
use serde::{Deserialize, Serialize};

#[derive(sqlx::FromRow, Debug, Serialize, Deserialize)]
pub struct Domain {
    pub id: String,
    pub username: String,
    pub password: String,
    pub txt: Option<String>
}

struct DomainDOT {
    id: String,
    username: String,
    password: String
}

impl DomainDOT {
    fn new() -> Self {
        let id = uuid::Uuid::new_v4().to_simple().to_string();
        let username = uuid::Uuid::new_v4().to_simple().to_string();
        let password = uuid::Uuid::new_v4().to_simple().to_string();

        DomainDOT {
            id,
            username,
            password
        }
    }
}

pub struct DomainFacade<DB: Database> {
    pool: Pool<DB>
}

impl <DB: Database> DomainFacade<DB> {
    pub(crate) fn new(pool: Pool<DB>) -> Self {
        DomainFacade {
            pool
        }
    }
}

impl DomainFacade<Postgres> {
    pub async fn find_by_id(&self, id: &str) -> Option<Domain> {
        sqlx::query_as("SELECT * FROM domain WHERE id = $1 LIMIT 1")
            .bind(id)
            .fetch_optional(&self.pool).await.unwrap()
    }

    pub async fn create_domain(&self, domain: &Domain) {
        sqlx::query("INSERT INTO domain (id, username, password, txt) VALUES ($1, $2, $3, $4)")
            .bind(&domain.id)
            .bind(&domain.username)
            .bind(&domain.password)
            .bind(&domain.txt)
            .execute(&self.pool)
            .await
            .unwrap();
    }
}

impl <DB: Database> Clone for DomainFacade<DB> {
    fn clone(&self) -> Self {
        DomainFacade {
            pool: self.pool.clone()
        }
    }
}