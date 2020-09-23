use acme_lib::persist::MemoryPersist;
use acme_lib::{create_p384_key, Directory, DirectoryUrl};
use chrono::{DateTime, Duration, Local};
use sqlx::{Executor, FromRow, PgPool, Postgres, Type};
use tokio::time::Interval;
use uuid::Uuid;

use crate::domain::{Domain, DomainFacade};
use std::error::Error;
use std::io::ErrorKind;

#[derive(sqlx::Type, Debug, PartialEq)]
#[repr(i32)]
pub enum State {
    Ok = 0,
    Updating = 1,
}

#[derive(FromRow, Debug)]
pub struct Cert {
    id: String,
    update: DateTime<Local>,
    state: State,
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
    fn new(domain: &Domain) -> Self {
        Cert {
            id: Uuid::new_v4().to_simple().to_string(),
            update: Local::now(),
            state: State::Updating,
            cert: None,
            private: None,
            domain: domain.id.clone(),
        }
    }
}

pub struct CertFacade {}

impl CertFacade {
    pub async fn first_cert<'a, E: Executor<'a, Database = Postgres>>(executor: E) -> Option<Cert>
    where
        DateTime<Local>: Type<Postgres>,
    {
        sqlx::query_as("SELECT * FROM cert LIMIT 1")
            .fetch_optional(executor)
            .await
            .unwrap()
    }

    async fn update_cert<'a, E: Executor<'a, Database = Postgres>>(executor: E, cert: &Cert) {
        sqlx::query("UPDATE cert SET update = $1, state = $2, cert = $3, private = $4, domain_id = $5 WHERE id = $6")
            .bind(&cert.update)
            .bind(&cert.state)
            .bind(&cert.cert)
            .bind(&cert.private)
            .bind(&cert.domain)
            .bind(&cert.id)
            .execute(executor)
            .await
            .unwrap();
    }

    async fn create_cert<'a, E: Executor<'a, Database = Postgres>>(executor: E, cert: &Cert) {
        sqlx::query("INSERT INTO cert (id, update, state, cert, private, domain_id) VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(&cert.id)
            .bind(&cert.update)
            .bind(&cert.state)
            .bind(&cert.cert)
            .bind(&cert.private)
            .bind(&cert.domain)
            .execute(executor)
            .await
            .unwrap();
    }

    pub async fn start(pool: &PgPool) -> Option<Cert> {
        let mut transaction = pool.begin().await.unwrap();

        let cert = CertFacade::first_cert(&mut transaction).await;

        let cert = match cert {
            Some(mut cert) if cert.state == State::Ok => {
                cert.state = State::Updating;
                CertFacade::update_cert(&mut transaction, &cert).await;
                Some(cert)
            }
            Some(mut cert) => {
                let one_hour_ago = Local::now() - Duration::hours(1);
                if cert.update < one_hour_ago {
                    cert.update = Local::now();
                    cert.state = State::Updating;
                    CertFacade::update_cert(&mut transaction, &cert).await;
                    Some(cert)
                } else {
                    None
                }
            }
            None => {
                let domain = Domain::default();
                let cert = Cert::new(&domain);

                DomainFacade::create_domain(&mut transaction, &domain).await;
                CertFacade::create_cert(&mut transaction, &cert).await;
                Some(cert)
            }
        };

        transaction.commit().await.unwrap();

        cert
    }

    pub async fn stop(pool: &PgPool, mut memory_cert: Cert) {
        let mut transaction = pool.begin().await.unwrap();

        match CertFacade::first_cert(&mut transaction).await {
            Some(cert) if cert.state == State::Updating && cert.update == memory_cert.update => {
                memory_cert.state = State::Ok;
                CertFacade::update_cert(pool, &memory_cert).await;
            }
            _ => {}
        }

        transaction.commit().await.unwrap();
    }
}

pub struct CertManager {
    pool: PgPool,
    acme: String,
}

impl CertManager {
    pub fn new(pool: PgPool, acme: String) -> Self {
        CertManager { pool, acme }
    }

    fn interval() -> Interval {
        let duration = Duration::hours(1).to_std().unwrap();
        tokio::time::interval(duration)
    }
}

fn error(kind: ErrorKind, message: &str) -> std::io::Error {
    let error: Box<dyn Error + Send + Sync> = From::from(message.to_string());
    std::io::Error::new(kind, error)
}

fn other_error(message: &str) -> std::io::Error {
    error(ErrorKind::Other, message)
}

impl CertManager {
    pub async fn spawn(self) -> Result<(), Box<dyn Error>> {
        tokio::spawn(async move {
            let mut interval = CertManager::interval();
            loop {
                interval.tick().await;
                //self.test().await;
            }
        })
        .await?;

        Ok(())
    }

    async fn test(&self) -> Result<(), Box<dyn Error>> {
        let mut memory_cert = CertFacade::start(&self.pool)
            .await
            .ok_or_else(|| other_error("Cert not found"))?;

        //improve
        let mut domain = DomainFacade::find_by_id(&self.pool, &memory_cert.domain)
            .await
            .expect("must have in sql");

        let dir = Directory::from_url(MemoryPersist::new(), DirectoryUrl::Other(&self.acme))?;
        let mut order = tokio::task::spawn_blocking(move || {
            let account = dir.account("acme-dns-rust@byom.de")?;
            account.new_order("acme.wehrli.ml", &[])
        })
        .await??;

        let mut auths = order.authorizations()?;
        let call = auths
            .pop()
            .ok_or_else(|| other_error("couldn't unpack auths"))?
            .dns_challenge();
        let proof = call.dns_proof();

        domain.txt = Some(proof);
        DomainFacade::update_domain(&self.pool, &domain).await;

        //error handling
        let cert = tokio::task::spawn_blocking(move || {
            call.validate(5000);
            order.refresh()?;
            // fix
            let ord_csr = order.confirm_validations().unwrap();
            let private = create_p384_key();
            let ord_crt = ord_csr.finalize_pkey(private, 5000)?;
            ord_crt.download_and_save_cert()
        })
        .await??;

        let private = cert.private_key().to_string();
        let cert = cert.certificate().to_string();

        memory_cert.cert = Some(cert);
        memory_cert.private = Some(private);
        CertFacade::stop(&self.pool, memory_cert).await;

        Ok(())
    }
}
