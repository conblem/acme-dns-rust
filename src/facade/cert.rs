use anyhow::Result;
use async_trait::async_trait;
use sqlx::FromRow;
use sqlx::{Database, Executor, Postgres};
use tracing::info;
use uuid::Uuid;

use super::domain::{DomainFacadeDatabase, DomainFacadeMemory};
use super::{DatabaseFacade, Domain, InMemoryFacade, InMemoryFacadeGuard};
use crate::util::{now, to_i64, HOUR_IN_SECONDS};

#[derive(sqlx::Type, Debug, PartialEq, Clone)]
#[repr(i32)]
pub enum State {
    Ok = 0,
    Updating = 1,
}

#[derive(FromRow, Debug, Clone)]
pub struct Cert {
    pub id: String,
    pub update: i64,
    pub state: State,
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
            id: Uuid::new_v4().simple().to_string(),
            update: to_i64(now()),
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
trait CertFacadeDatabase<DB: Database> {
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
impl CertFacadeDatabase<Postgres> for DatabaseFacade<Postgres> {
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
        CertFacadeDatabase::first_cert(self, &self.pool).await
    }

    async fn update_cert(&self, cert: &Cert) -> Result<(), sqlx::Error> {
        CertFacadeDatabase::update_cert(self, &self.pool, cert).await
    }

    async fn create_cert(&self, cert: &Cert) -> Result<(), sqlx::Error> {
        CertFacadeDatabase::create_cert(self, &self.pool, cert).await
    }

    async fn start_cert(&self) -> Result<Option<Cert>> {
        let mut transaction = self.pool.begin().await?;

        let cert = CertFacadeDatabase::first_cert(self, &mut transaction).await?;

        let cert = match cert {
            Some(mut cert) if cert.state == State::Ok => {
                cert.state = State::Updating;
                CertFacadeDatabase::update_cert(self, &mut transaction, &cert).await?;
                Some(cert)
            }
            // cert is in updating state as there are only to cert.state
            Some(mut cert) => {
                let now = to_i64(now());
                let one_hour_ago = now - HOUR_IN_SECONDS as i64;
                // longer ago than 1 hour so probably timed out
                if cert.update < one_hour_ago {
                    cert.update = now;
                    // todo: dont think this is needed
                    cert.state = State::Updating;
                    CertFacadeDatabase::update_cert(self, &mut transaction, &cert).await?;
                    Some(cert)
                } else {
                    info!("job still in progress");
                    None
                }
            }
            None => {
                let domain = Domain::new()?;
                let cert = Cert::new(&domain);

                DomainFacadeDatabase::create_domain(self, &mut transaction, &domain).await?;
                CertFacadeDatabase::create_cert(self, &mut transaction, &cert).await?;
                Some(cert)
            }
        };

        transaction.commit().await?;

        Ok(cert)
    }

    async fn stop_cert(&self, memory_cert: &mut Cert) -> Result<(), sqlx::Error> {
        let mut transaction = self.pool.begin().await?;

        match CertFacadeDatabase::first_cert(self, &mut transaction).await? {
            // only stop cert if the update times match and no other interval picked up the job
            Some(cert) if cert.state == State::Updating && cert.update == memory_cert.update => {
                memory_cert.state = State::Ok;
                CertFacadeDatabase::update_cert(self, &self.pool, memory_cert).await?;
            }
            _ => {}
        }

        transaction.commit().await?;
        Ok(())
    }
}

trait CertFacadeMemory {
    fn first_cert(&self, lock: &mut InMemoryFacadeGuard<'_>) -> Option<Cert> {
        lock.certs.values().next().map(Clone::clone)
    }

    fn update_cert(&self, lock: &mut InMemoryFacadeGuard<'_>, cert: &Cert) {
        *lock.certs.get_mut(&cert.id).unwrap() = cert.clone();
    }

    fn create_cert(&self, lock: &mut InMemoryFacadeGuard<'_>, cert: &Cert) {
        lock.certs.insert(cert.id.clone(), cert.clone());
    }
}

impl CertFacadeMemory for InMemoryFacade {}

#[async_trait]
impl CertFacade for InMemoryFacade {
    async fn first_cert(&self) -> Result<Option<Cert>, sqlx::Error> {
        let mut lock = self.0.lock();
        let cert = CertFacadeMemory::first_cert(self, &mut lock);
        Ok(cert)
    }

    async fn update_cert(&self, cert: &Cert) -> Result<(), sqlx::Error> {
        let mut lock = self.0.lock();
        CertFacadeMemory::update_cert(self, &mut lock, cert);

        Ok(())
    }

    async fn create_cert(&self, cert: &Cert) -> Result<(), sqlx::Error> {
        let mut lock = self.0.lock();
        CertFacadeMemory::create_cert(self, &mut lock, cert);

        Ok(())
    }

    async fn start_cert(&self) -> Result<Option<Cert>> {
        let mut transaction = self.0.lock();
        let cert = CertFacadeMemory::first_cert(self, &mut transaction);

        let cert = match cert {
            Some(mut cert) if cert.state == State::Ok => {
                cert.state = State::Updating;
                CertFacadeMemory::update_cert(self, &mut transaction, &cert);
                Some(cert)
            }
            Some(mut cert) => {
                let now = to_i64(now());
                let one_hour_ago = now - HOUR_IN_SECONDS as i64;
                // longer ago than 1 hour so probably timed out
                if cert.update < one_hour_ago {
                    cert.update = now;
                    cert.state = State::Updating;
                    CertFacadeMemory::update_cert(self, &mut transaction, &cert);
                    Some(cert)
                } else {
                    info!("job still in progress");
                    None
                }
            }
            None => {
                let domain = Domain::new()?;
                let cert = Cert::new(&domain);

                DomainFacadeMemory::create_domain(self, &mut transaction, &domain);
                CertFacadeMemory::create_cert(self, &mut transaction, &cert);
                Some(cert)
            }
        };

        Ok(cert)
    }

    async fn stop_cert(&self, memory_cert: &mut Cert) -> Result<(), sqlx::Error> {
        let mut transaction = self.0.lock();

        match CertFacadeMemory::first_cert(self, &mut transaction) {
            Some(cert) if cert.state == State::Updating && cert.update == memory_cert.update => {
                memory_cert.state = State::Ok;
                CertFacadeMemory::update_cert(self, &mut transaction, memory_cert);
            }
            _ => {}
        }

        Ok(())
    }
}

#[cfg(all(test, not(feature = "disable-docker")))]
pub(crate) mod tests {
    use rstest::*;
    use tokio::runtime::Runtime;

    use super::super::tests::TestPool;
    use super::{Cert, CertFacade, DatabaseFacade, State};
    use crate::facade::{Domain, DomainFacade};
    use crate::util::{now, to_i64, uuid};

    #[fixture]
    #[once]
    fn runtime() -> Runtime {
        Runtime::new().unwrap()
    }

    #[fixture]
    #[once]
    fn test_pool(runtime: &Runtime, domain: &Domain) -> TestPool {
        runtime.block_on(async {
            let pool = super::super::tests::test_pool().await;
            let facade = DatabaseFacade::from(pool.clone());
            facade.create_domain(&domain).await.unwrap();

            pool
        })
    }

    #[fixture]
    #[once]
    fn domain() -> Domain {
        Domain {
            id: uuid(),
            password: "$2b$12$zTUOFwfVurULlALrEHdn7OK0it3BRNy43FOb2Qos1PGOPd/YCPVg.".to_string(),
            txt: Some("TXT Content".to_string()),
            username: "6f791bc4494846ba997562c85d03b940".to_string(),
        }
    }

    #[fixture]
    fn cert(domain: &Domain) -> Cert {
        Cert {
            id: uuid(),
            update: to_i64(now()),
            state: State::Ok,
            cert: Some(include_str!("../../tests/leaf.crt").to_owned()),
            private: Some(include_str!("../../tests/leaf.key").to_owned()),
            domain: domain.id.clone(),
        }
    }

    #[rstest]
    fn test_postgres_cert_facade(runtime: &Runtime, test_pool: &TestPool, mut cert: Cert) {
        runtime.block_on(async {
            let facade = DatabaseFacade::from((**test_pool).clone());

            facade.create_cert(&cert).await.unwrap();

            // check if we received create cert from database
            let actual = facade.first_cert().await.unwrap().unwrap();
            assert_eq!(cert, actual);

            // check if we receive update cert from database
            cert.state = State::Updating;
            facade.update_cert(&cert).await.unwrap();
            let actual = facade.first_cert().await.unwrap().unwrap();
            assert_eq!(cert, actual);
        })
    }
}
