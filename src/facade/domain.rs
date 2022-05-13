use anyhow::{Error, Result};
use async_trait::async_trait;
use core::convert::TryFrom;
use serde::{Deserialize, Serialize};
use sqlx::{Database, Executor, FromRow, Postgres};

use super::{DatabaseFacade, InMemoryFacade, InMemoryFacadeGuard};
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

#[derive(FromRow, Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
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
pub(super) trait DomainFacadeDatabase<DB: Database> {
    async fn create_domain<'a, E: Executor<'a, Database = DB>>(
        &self,
        executor: E,
        domain: &Domain,
    ) -> Result<(), sqlx::Error>;
}

#[async_trait]
impl DomainFacadeDatabase<Postgres> for DatabaseFacade<Postgres> {
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
        DomainFacadeDatabase::create_domain(self, &self.pool, domain).await
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

pub(super) trait DomainFacadeMemory {
    fn create_domain(&self, lock: &mut InMemoryFacadeGuard<'_>, domain: &Domain) {
        lock.domains.insert(domain.id.clone(), domain.clone());
    }
}

impl DomainFacadeMemory for InMemoryFacade {}

#[async_trait]
impl DomainFacade for InMemoryFacade {
    async fn find_domain_by_id(&self, id: &str) -> Result<Option<Domain>, sqlx::Error> {
        let lock = self.0.lock();
        let domain = lock.domains.get(&id.to_owned()).map(Clone::clone);
        Ok(domain)
    }

    async fn create_domain(&self, domain: &Domain) -> Result<(), sqlx::Error> {
        let mut lock = self.0.lock();
        DomainFacadeMemory::create_domain(self, &mut lock, domain);

        Ok(())
    }

    async fn update_domain(&self, domain: &Domain) -> Result<(), sqlx::Error> {
        let mut lock = self.0.lock();
        *lock.domains.get_mut(&domain.id).unwrap() = domain.clone();

        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use testcontainers::clients::Cli;
    use testcontainers::images::postgres::Postgres;

    use super::{DatabaseFacade, Domain, DomainFacade};
    use crate::setup_database;

    pub(crate) fn create_domain() -> Domain {
        Domain {
            id: "0e1f8297564a420eb260749d9f5ddd45".to_string(),
            password: "$2b$12$zTUOFwfVurULlALrEHdn7OK0it3BRNy43FOb2Qos1PGOPd/YCPVg.".to_string(),
            txt: Some("TXT Content".to_string()),
            username: "6f791bc4494846ba997562c85d03b940".to_string(),
        }
    }

    //#[cfg(not(feature = "disable-docker"))]
    #[tokio::test]
    async fn test_postgres_domain_facade() {
        let docker = Cli::default();
        let node = docker.run(Postgres::default());

        let connection_string = &format!(
            "postgres://postgres:postgres@localhost:{}/postgres",
            node.get_host_port(5432)
        );

        let pool = setup_database(connection_string).await.unwrap();
        let facade = DatabaseFacade::from(pool);

        let mut domain = create_domain();
        facade.create_domain(&domain).await.unwrap();

        let actual = facade.find_domain_by_id(&domain.id).await.unwrap().unwrap();
        assert_eq!(domain, actual);

        domain.txt = Some("Another TXT Content".to_owned());
        facade.update_domain(&domain).await.unwrap();
        let actual = facade.find_domain_by_id(&domain.id).await.unwrap().unwrap();
        assert_eq!(domain, actual);
    }
}
