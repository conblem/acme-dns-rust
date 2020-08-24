use acme_lib::persist::MemoryPersist;
use acme_lib::{create_p384_key, Directory, DirectoryUrl};
use chrono::naive::NaiveDateTime;
use chrono::{DateTime, Duration, Local, Utc};
use sqlx::{Any, AnyPool, Executor, FromRow, PgPool, Postgres};
use tokio::time::Interval;
use uuid::Uuid;

use crate::api::Api;
use crate::domain::{Domain, DomainFacade};

#[derive(sqlx::Type, Debug, PartialEq)]
#[repr(i32)]
pub enum State {
    Ok = 0,
    Updating = 1,
}

#[derive(FromRow, Debug)]
pub struct Cert {
    id: String,
    update: i64,
    state: State,
    #[sqlx(rename = "domain_id")]
    pub domain: String,
}

impl Cert {
    fn new(domain: &Domain) -> Self {
        Cert {
            id: Uuid::new_v4().to_simple().to_string(),
            update: Local::now().timestamp(),
            state: State::Updating,
            domain: domain.id.clone(),
        }
    }
}

pub struct CertFacade {}

impl CertFacade {
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
                let update =
                    DateTime::<Utc>::from_utc(NaiveDateTime::from_timestamp(cert.update, 0), Utc);
                if update < one_hour_ago {
                    cert.update = Local::now().timestamp_millis();
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
    api: Api,
}

impl CertManager {
    pub fn new(pool: PgPool, api: Api) -> Self {
        CertManager { pool, api }
    }

    fn interval() -> Interval {
        let duration = Duration::hours(1).to_std().unwrap();
        tokio::time::interval(duration)
    }
}

impl CertManager {
    pub async fn job(self) {
        let mut interval = CertManager::interval();
        loop {
            interval.tick().await;
            self.test().await;
        }
    }

    async fn test(&self) {
        let memory_cert = match CertFacade::start(&self.pool).await {
            Some(cert) => cert,
            None => return,
        };

        println!("cert cert");
        println!("{:?}", memory_cert);

        let mut domain = DomainFacade::find_by_id(&self.pool, &memory_cert.domain)
            .await
            .expect("must have in sql");

        println!("{:?}", domain);

        let url = DirectoryUrl::LetsEncryptStaging;
        let persist = MemoryPersist::new();
        let dir = Directory::from_url(persist, url).unwrap();
        let mut order = tokio::task::spawn_blocking(move || {
            let account = dir.account("acme-dns-rust@byom.de").unwrap();
            account.new_order("acme.wehrli.ml", &[]).unwrap()
        })
        .await
        .unwrap();

        let auths = order.authorizations().unwrap();
        let call = auths[0].dns_challenge();
        let proof = call.dns_proof();

        domain.txt = Some(proof);
        DomainFacade::update_domain(&self.pool, &domain).await;

        //error handling
        let cert = tokio::task::spawn_blocking(move || {
            call.validate(5000);
            order.refresh().unwrap();
            let ord_csr = order.confirm_validations().unwrap();
            let private = create_p384_key();
            let ord_crt = ord_csr.finalize_pkey(private, 5000).unwrap();
            ord_crt.download_and_save_cert().unwrap()
        })
        .await
        .unwrap();

        let mut private = cert.private_key_der();
        let mut cert = cert.certificate_der();

        self.api.set_config(&mut private, &mut cert).unwrap();

        CertFacade::stop(&self.pool, memory_cert).await;
    }
}
