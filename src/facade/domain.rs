use anyhow::{Error, Result};
use async_trait::async_trait;
use core::convert::TryFrom;
use serde::{Deserialize, Serialize};
use sqlx::{Database, Executor, Postgres};

use super::DatabaseFacade;
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

#[async_trait]
pub trait DomainFacade {
    async fn find_domain_by_id(&self, id: &str) -> Result<Option<Domain>, sqlx::Error>;
    async fn create_domain(&self, domain: &Domain) -> Result<(), sqlx::Error>;
    async fn update_domain(&self, domain: &Domain) -> Result<(), sqlx::Error>;
}

#[async_trait]
pub(super) trait DomainFacadeInternal<DB: Database> {
    async fn create_domain<'a, E: Executor<'a, Database = DB>>(
        &self,
        executor: E,
        domain: &Domain,
    ) -> Result<(), sqlx::Error>;
}

#[async_trait]
impl DomainFacadeInternal<Postgres> for DatabaseFacade<Postgres> {
    async fn create_domain<'a, E: Executor<'a, Database = Postgres>>(
        &self,
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
}

#[async_trait]
impl DomainFacade for DatabaseFacade<Postgres> {
    async fn find_domain_by_id(&self, id: &str) -> Result<Option<Domain>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM domain WHERE id = $1 LIMIT 1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    async fn create_domain(&self, domain: &Domain) -> Result<(), sqlx::Error> {
        DomainFacadeInternal::create_domain(self, &self.pool, domain).await
    }

    async fn update_domain(&self, domain: &Domain) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE domain SET username = $1, password = $2, txt = $3 WHERE id = $4")
            .bind(&domain.username)
            .bind(&domain.password)
            .bind(&domain.txt)
            .bind(&domain.id)
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}
