use anyhow::{Error, Result};
use core::convert::TryFrom;
use serde::{Deserialize, Serialize};
use sqlx::{Executor, Postgres};

use crate::util::uuid;

#[derive(Debug, Serialize, Clone)]
pub struct DomainDTO {
    pub id: String,
    pub username: String,
    pub password: String,
}

impl Default for DomainDTO {
    fn default() -> Self {
        DomainDTO {
            id: uuid(),
            username: uuid(),
            password: uuid(),
        }
    }
}

#[derive(sqlx::FromRow, Debug, Serialize, Deserialize)]
pub struct Domain {
    pub id: String,
    pub username: String,
    pub password: String,
    pub txt: Option<String>,
}

impl TryFrom<DomainDTO> for Domain {
    type Error = Error;
    fn try_from(input: DomainDTO) -> Result<Self, Self::Error> {
        let password = bcrypt::hash(input.password, bcrypt::DEFAULT_COST)?;

        Ok(Domain {
            id: input.id,
            username: input.username,
            password,
            txt: None,
        })
    }
}

impl Domain {
    pub(crate) fn new() -> Result<Self> {
        let password = bcrypt::hash(uuid(), bcrypt::DEFAULT_COST)?;

        Ok(Domain {
            id: uuid(),
            username: uuid(),
            password,
            txt: None,
        })
    }
}

pub struct DomainFacade {}

impl DomainFacade {
    pub async fn find_by_id<'a, E: Executor<'a, Database = Postgres>>(
        executor: E,
        id: &str,
    ) -> Result<Option<Domain>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM domain WHERE id = $1 LIMIT 1")
            .bind(id)
            .fetch_optional(executor)
            .await
    }

    pub async fn create_domain<'a, E: Executor<'a, Database = Postgres>>(
        executor: E,
        domain: &Domain,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO domain (id, username, password, txt) VALUES ($1, $2, $3, $4)")
            .bind(&domain.id)
            .bind(&domain.username)
            .bind(&domain.password)
            .bind(&domain.txt)
            .execute(executor)
            .await?;

        Ok(())
    }

    pub async fn update_domain<'a, E: Executor<'a, Database = Postgres>>(
        executor: E,
        domain: &Domain,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE domain SET username = $1, password = $2, txt = $3 WHERE id = $4")
            .bind(&domain.username)
            .bind(&domain.password)
            .bind(&domain.txt)
            .bind(&domain.id)
            .execute(executor)
            .await?;

        Ok(())
    }
}
