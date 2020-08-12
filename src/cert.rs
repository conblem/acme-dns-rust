use sqlx::{Database, FromRow, Pool, Postgres, Executor};
use chrono::naive::NaiveDateTime;
use chrono::{Duration, Local};
use acme_lib::{Directory, DirectoryUrl};
use acme_lib::persist::MemoryPersist;
use crate::domain::{Domain, DomainFacade};

#[derive(sqlx::Type, Debug, PartialEq)]
#[repr(i32)]
pub enum State { Ok = 0, Updating = 1 }

#[derive(FromRow, Debug)]
pub struct Cert {
    id: String,
    update: NaiveDateTime,
    state: State,
    #[sqlx(rename = "domain_id")]
    domain: String
}

pub struct CertFacade<DB: Database> {
    pool: Pool<DB>,
    domain_facade: DomainFacade<DB>
}

impl <DB: Database> CertFacade<DB> {
    pub fn new(pool: Pool<DB>) -> Self {
        let domain_facade = DomainFacade::new(pool.clone());
        CertFacade {
            pool,
            domain_facade
        }
    }
}

impl CertFacade<Postgres> {
    pub async fn find_by_id(&self, id: &str) -> Option<Cert> {
        sqlx::query_as("SELECT * FROM cert WHERE id = $1 LIMIT 1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .unwrap_or(None)
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

    pub async fn start(&self) {
        let mut transaction = self.pool.begin().await.unwrap();

        let cert = sqlx::query_as::<Postgres, Cert>("SELECT * FROM cert LIMIT 1")
            .fetch_optional(&mut transaction)
            .await
            .unwrap_or(None);

        match cert {
            Some(mut cert) if cert.state == State::Ok => {
                cert.state = State::Updating;
                CertFacade::update_cert(&mut transaction, &cert).await
            },
            Some(mut cert) => {
                let one_hour_ago = Local::now().naive_local() - Duration::hours(1);
                if cert.update < one_hour_ago {
                    cert.update = Local::now().naive_local();
                    CertFacade::update_cert(&mut transaction, &cert).await
                }
            },
            None => {
                let cert = Cert {
                    id: uuid::Uuid::new_v4().to_simple().to_string(),
                    update: chrono::Local::now().naive_local(),
                    state: State::Ok,
                    domain: uuid::Uuid::new_v4().to_simple().to_string()
                };

                CertFacade::create_cert(&mut transaction, &cert).await;
            },
        }

        transaction.commit().await.unwrap();
    }
}

pub struct CertManager<DB: Database> {
    cert_facade: CertFacade<DB>
}

impl CertManager<Postgres> {
    pub fn new(cert_facade: CertFacade<Postgres>) -> Self {
        CertManager {
            cert_facade
        }
    }

    pub async fn test(&self) {
        let url = DirectoryUrl::LetsEncryptStaging;
        let persist = MemoryPersist::new();
        let dir = Directory::from_url(persist, url).unwrap();
        /*let order = tokio::task::spawn_blocking(move || {
            let account = dir.account("acme-dns-rust@byom.de").unwrap();
            let mut order = account.new_order("ns.wehrli.ml", &[]).unwrap();
            let auths = order.authorizations().unwrap();

            let call = auths[0].dns_challenge();
            let proof = call.dns_proof();
            call.validate(5000);
            println!("{:?}", proof);

            order
        }).await.unwrap();*/

        //self.cert_facade.start().await;
    }
}