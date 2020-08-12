use sqlx::{Database, FromRow, Pool, Postgres, Executor, MySql};
use chrono::naive::NaiveDateTime;
use chrono::{Duration, Local, DateTime};
use acme_lib::{Directory, DirectoryUrl};
use acme_lib::persist::MemoryPersist;
use crate::domain::{Domain, DomainFacade};
use std::marker::PhantomData;
use std::borrow::Borrow;
use uuid::Uuid;

#[derive(sqlx::Type, Debug, PartialEq)]
#[repr(i32)]
pub enum State { Ok = 0, Updating = 1 }

#[derive(FromRow, Debug)]
pub struct Cert {
    id: String,
    update: NaiveDateTime,
    state: State,
    #[sqlx(rename = "domain_id")]
    pub domain: String
}

impl Cert {
    fn new(domain: &Domain) -> Self {
        Cert {
            id: Uuid::new_v4().to_simple().to_string(),
            update: Local::now().naive_local(),
            state: State::Updating,
            domain: domain.id.clone()
        }
    }
}

pub struct CertFacade<DB: Database> {
    _phantom: PhantomData<DB>
}


impl  CertFacade<Postgres> {
    pub async fn first_cert<'a, E: Executor<'a, Database = Postgres>>(executor: E) -> Option<Cert> {
        sqlx::query_as("SELECT * FROM cert LIMIT 1")
            .fetch_optional(executor)
            .await
            .unwrap()
    }

    async fn update_cert<'a, E: Executor<'a, Database = Postgres>>(executor: E, cert: &Cert) {
        sqlx::query("UPDATE cert SET update = $1, state = $2, domain_id = $3 WHERE id = $4")
            .bind(&cert.update)
            .bind(&cert.state)
            .bind(&cert.domain)
            .bind(&cert.id)
            .execute(executor)
            .await
            .unwrap();
    }

    async fn create_cert<'a, E: Executor<'a, Database = Postgres>>(executor: E, cert: &Cert) {
        sqlx::query("INSERT INTO cert (id, update, state, domain_id) VALUES ($1, $2, $3, $4)")
            .bind(&cert.id)
            .bind(&cert.update)
            .bind(&cert.state)
            .bind(&cert.domain)
            .execute(executor)
            .await
            .unwrap();
    }

    pub async fn start(pool: &Pool<Postgres>) -> Option<Cert> {
        let mut transaction = pool.begin().await.unwrap();

        let cert = CertFacade::first_cert(&mut transaction).await;

        let cert = match cert {
            Some(mut cert) if cert.state == State::Ok => {
                cert.state = State::Updating;
                CertFacade::update_cert(&mut transaction, &cert).await;
                Some(cert)
            },
            Some(mut cert) => {
                let one_hour_ago = Local::now().naive_local() - Duration::hours(1);
                if cert.update < one_hour_ago {
                    cert.update = Local::now().naive_local();
                    cert.state = State::Updating;
                    CertFacade::update_cert(&mut transaction, &cert).await;
                    Some(cert)
                }
                else {
                    None
                }
            },
            None => {
                let domain = Domain::default();
                let cert = Cert::new(&domain);

                DomainFacade::create_domain(&mut transaction, &domain).await;
                CertFacade::create_cert(&mut transaction, &cert).await;
                Some(cert)
            },
        };

        transaction.commit().await.unwrap();

        cert
    }

    pub async fn stop(pool: &Pool<Postgres>, mut memory_cert: Cert) {
        let mut transaction = pool.begin().await.unwrap();

        match CertFacade::first_cert(&mut transaction).await {
            Some(cert) if cert.state == State::Updating && cert.update == memory_cert.update => {
                memory_cert.state = State::Ok;
                CertFacade::update_cert(pool, &memory_cert).await;
            },
            _ => {}
        }


        transaction.commit().await.unwrap();
    }
}

pub struct CertManager<DB: Database> {
    pool: Pool<DB>
}

impl <DB: Database> CertManager<DB> {
    pub fn new(pool: Pool<DB>) -> Self {
        CertManager {
            pool
        }
    }
}

impl CertManager<Postgres> {
    pub async fn test(&self) {
        let cert = match CertFacade::start(&self.pool).await {
            Some(cert) => cert,
            None => return
        };

        let mut domain = DomainFacade::find_by_id(&self.pool, &cert.id)
            .await
            .expect("must have in sql");

        let url = DirectoryUrl::LetsEncryptStaging;
        let persist = MemoryPersist::new();
        let dir = Directory::from_url(persist, url).unwrap();
        let call = tokio::task::spawn_blocking(move || {
            let account = dir.account("acme-dns-rust@byom.de").unwrap();
            let mut order = account.new_order("acme.wehrli.ml", &[]).unwrap();
            let auths = order.authorizations().unwrap();

            auths[0].dns_challenge()
        }).await.unwrap();

        let proof = call.dns_proof();
        domain.txt = Some(proof);
        DomainFacade::update_domain(&self.pool, &domain).await;

        //error handling
        tokio::task::spawn_blocking(move || {
            call.validate(5000);
        }).await.unwrap();

        //self.cert_facade.start().await;
    }
}