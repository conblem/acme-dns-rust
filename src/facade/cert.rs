use anyhow::Result;
use async_trait::async_trait;
use sqlx::FromRow;
use sqlx::{Database, Executor, Postgres};
use tracing::info;
use uuid::Uuid;

use super::domain::{Domain, DomainFacadeInternal};
use super::DatabaseFacade;
use crate::facade::TestFacade;
use crate::util::{now, to_i64, HOUR};

#[derive(sqlx::Type, Debug, PartialEq, Clone)]
#[repr(i32)]
pub enum State {
    Ok = 0,
    Updating = 1,
}

#[derive(FromRow, Debug, Clone)]
pub struct Cert {
    pub(crate) id: String,
    pub(crate) update: i64,
    pub(crate) state: State,
    pub cert: Option<String>,
    pub private: Option<String>,
    #[sqlx(rename = "domain_id")]
    pub domain: String,
}

impl PartialEq for Cert {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.cert == other.cert && self.private == other.private
    }
}

impl Cert {
    // remove expect
    fn new(domain: &Domain) -> Self {
        Cert {
            id: Uuid::new_v4().to_simple().to_string(),
            update: to_i64(&now()),
            state: State::Updating,
            cert: None,
            private: None,
            domain: domain.id.clone(),
        }
    }
}

#[async_trait]
pub trait CertFacade {
    async fn first_cert(&self) -> Result<Option<Cert>, sqlx::Error>;
    async fn update_cert(&self, cert: &Cert) -> Result<(), sqlx::Error>;
    async fn create_cert(&self, cert: &Cert) -> Result<(), sqlx::Error>;
    async fn start_cert(&self) -> Result<Option<Cert>>;
    async fn stop_cert(&self, memory_cert: &mut Cert) -> Result<(), sqlx::Error>;
}

#[async_trait]
trait CertFacadeInternal<DB: Database> {
    async fn first_cert<'a, E: Executor<'a, Database = DB>>(
        &self,
        executor: E,
    ) -> Result<Option<Cert>, sqlx::Error>;

    async fn update_cert<'a, E: Executor<'a, Database = DB>>(
        &self,
        executor: E,
        cert: &Cert,
    ) -> Result<(), sqlx::Error>;

    async fn create_cert<'a, E: Executor<'a, Database = DB>>(
        &self,
        executor: E,
        cert: &Cert,
    ) -> Result<(), sqlx::Error>;
}

#[async_trait]
impl CertFacadeInternal<Postgres> for DatabaseFacade<Postgres> {
    async fn first_cert<'a, E: Executor<'a, Database = Postgres>>(
        &self,
        executor: E,
    ) -> Result<Option<Cert>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM cert LIMIT 1")
            .fetch_optional(executor)
            .await
    }

    async fn update_cert<'a, E: Executor<'a, Database = Postgres>>(
        &self,
        executor: E,
        cert: &Cert,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE cert SET update = $1, state = $2, cert = $3, private = $4, domain_id = $5 WHERE id = $6")
            .bind(&cert.update)
            .bind(&cert.state)
            .bind(&cert.cert)
            .bind(&cert.private)
            .bind(&cert.domain)
            .bind(&cert.id)
            .execute(executor)
            .await?;

        Ok(())
    }

    async fn create_cert<'a, E: Executor<'a, Database = Postgres>>(
        &self,
        executor: E,
        cert: &Cert,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO cert (id, update, state, cert, private, domain_id) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(&cert.id)
            .bind(&cert.update)
            .bind(&cert.state)
            .bind(&cert.cert)
            .bind(&cert.private)
            .bind(&cert.domain)
            .execute(executor)
            .await?;

        Ok(())
    }
}

#[async_trait]
impl CertFacade for DatabaseFacade<Postgres> {
    async fn first_cert(&self) -> Result<Option<Cert>, sqlx::Error> {
        CertFacadeInternal::first_cert(self, &self.pool).await
    }

    async fn update_cert(&self, cert: &Cert) -> Result<(), sqlx::Error> {
        CertFacadeInternal::update_cert(self, &self.pool, cert).await
    }

    async fn create_cert(&self, cert: &Cert) -> Result<(), sqlx::Error> {
        CertFacadeInternal::create_cert(self, &self.pool, cert).await
    }

    async fn start_cert(&self) -> Result<Option<Cert>> {
        let mut transaction = self.pool.begin().await?;

        let cert = CertFacadeInternal::first_cert(self, &mut transaction).await?;

        let cert = match cert {
            Some(mut cert) if cert.state == State::Ok => {
                cert.state = State::Updating;
                CertFacadeInternal::update_cert(self, &mut transaction, &cert).await?;
                Some(cert)
            }
            Some(mut cert) => {
                let now = to_i64(&now());
                let one_hour_ago = now - HOUR as i64;
                // longer ago than 1 hour so probably timed out
                if cert.update < one_hour_ago {
                    cert.update = now;
                    cert.state = State::Updating;
                    CertFacadeInternal::update_cert(self, &mut transaction, &cert).await?;
                    Some(cert)
                } else {
                    info!("job still in progress");
                    None
                }
            }
            None => {
                let domain = Domain::new()?;
                let cert = Cert::new(&domain);

                DomainFacadeInternal::create_domain(self, &mut transaction, &domain).await?;
                CertFacadeInternal::create_cert(self, &mut transaction, &cert).await?;
                Some(cert)
            }
        };

        transaction.commit().await?;

        Ok(cert)
    }

    async fn stop_cert(&self, memory_cert: &mut Cert) -> Result<(), sqlx::Error> {
        let mut transaction = self.pool.begin().await?;

        match CertFacadeInternal::first_cert(self, &mut transaction).await? {
            Some(cert) if cert.state == State::Updating && cert.update == memory_cert.update => {
                memory_cert.state = State::Ok;
                CertFacadeInternal::update_cert(self, &self.pool, &memory_cert).await?;
            }
            _ => {}
        }

        transaction.commit().await?;
        Ok(())
    }
}

#[async_trait]
impl CertFacade for TestFacade {
    async fn first_cert(&self) -> Result<Option<Cert>, sqlx::Error> {
        let certs = self.certs.lock();
        Ok(certs.values().next().map(Clone::clone))
    }

    async fn update_cert(&self, cert: &Cert) -> Result<(), sqlx::Error> {
        let mut certs = self.certs.lock();
        *certs.get_mut(&cert.id).unwrap() = cert.clone();

        Ok(())
    }

    async fn create_cert(&self, cert: &Cert) -> Result<(), sqlx::Error> {
        self.certs.lock().insert(cert.id.clone(), cert.clone());

        Ok(())
    }

    async fn start_cert(&self) -> Result<Option<Cert>> {
        unimplemented!()
    }

    async fn stop_cert(&self, _memory_cert: &mut Cert) -> Result<(), sqlx::Error> {
        unimplemented!()
    }
}
