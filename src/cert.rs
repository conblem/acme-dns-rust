use acme_lib::order::NewOrder;
use acme_lib::{create_p384_key, Directory, DirectoryUrl};
use anyhow::{anyhow, Result};
use sqlx::{Executor, FromRow, PgPool, Postgres};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::time::Interval;
use tracing::{error, info, Instrument, Span};
use uuid::Uuid;

use crate::acme::DatabasePersist;
use crate::domain::{Domain, DomainFacade};
use crate::util::{now, to_i64, HOUR};
use std::sync::Arc;

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

pub struct CertFacade {}

impl CertFacade {
    pub async fn first_cert<'a, E: Executor<'a, Database = Postgres>>(
        executor: E,
    ) -> Result<Option<Cert>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM cert LIMIT 1")
            .fetch_optional(executor)
            .await
    }

    async fn update_cert<'a, E: Executor<'a, Database = Postgres>>(
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

    pub async fn start(pool: &PgPool) -> Result<Option<Cert>> {
        let mut transaction = pool.begin().await?;

        let cert = CertFacade::first_cert(&mut transaction).await?;

        let cert = match cert {
            Some(mut cert) if cert.state == State::Ok => {
                cert.state = State::Updating;
                CertFacade::update_cert(&mut transaction, &cert).await?;
                Some(cert)
            }
            Some(mut cert) => {
                let now = to_i64(&now());
                let one_hour_ago = now - HOUR as i64;
                // longer ago than 1 hour so probably timed out
                if cert.update < one_hour_ago {
                    cert.update = now;
                    cert.state = State::Updating;
                    CertFacade::update_cert(&mut transaction, &cert).await?;
                    Some(cert)
                } else {
                    info!("job still in progress");
                    None
                }
            }
            None => {
                let domain = Domain::new()?;
                let cert = Cert::new(&domain);

                DomainFacade::create_domain(&mut transaction, &domain).await?;
                CertFacade::create_cert(&mut transaction, &cert).await?;
                Some(cert)
            }
        };

        transaction.commit().await?;

        Ok(cert)
    }

    pub async fn stop(pool: &PgPool, mut memory_cert: Cert) -> Result<(), sqlx::Error> {
        let mut transaction = pool.begin().await?;

        match CertFacade::first_cert(&mut transaction).await? {
            Some(cert) if cert.state == State::Updating && cert.update == memory_cert.update => {
                memory_cert.state = State::Ok;
                CertFacade::update_cert(pool, &memory_cert).await?;
            }
            _ => {}
        }

        transaction.commit().await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct CertManager {
    pool: PgPool,
    directory: Directory<DatabasePersist>,
    runtime: Arc<Runtime>,
}

impl CertManager {
    #[tracing::instrument(name = "CertManager::new", skip(pool, persist))]
    pub async fn new(
        pool: PgPool,
        persist: DatabasePersist,
        acme: String,
        runtime: &Arc<Runtime>,
    ) -> Result<Self> {
        let span = Span::current();
        let directory = tokio::task::spawn_blocking(move || {
            let _enter = span.enter();
            Directory::from_url(persist, DirectoryUrl::Other(&acme))
        })
        .await??;

        Ok(CertManager {
            pool,
            directory,
            runtime: Arc::clone(runtime),
        })
    }

    // maybe useless function
    fn interval() -> Interval {
        // use constant
        tokio::time::interval(Duration::from_secs(HOUR))
    }

    #[tracing::instrument(name = "CertManager::spawn", skip(self))]
    pub async fn spawn(self) -> Result<()> {
        tokio::spawn(
            async move {
                let mut interval = CertManager::interval();
                loop {
                    interval.tick().await;
                    info!("Started Interval");
                    if false {
                        info!("Skipping Interval");
                        continue;
                    }
                    if let Err(e) = self.manage().await {
                        error!("{}", e);
                        continue;
                    }
                    info!("Interval successfully passed");
                }
            }
            .in_current_span(),
        )
        .await?;

        Ok(())
    }

    async fn manage(&self) -> Result<()> {
        // maybe context is not needed here
        let memory_cert = match CertFacade::start(&self.pool).await? {
            Some(memory_cert) => memory_cert,
            None => return Ok(()),
        };

        // todo: improve
        let domain = DomainFacade::find_by_id(&self.pool, &memory_cert.domain)
            .await?
            .ok_or_else(|| anyhow!("Could not find domain: {}", &memory_cert.domain))?;

        let directory = self.directory.clone();
        let pool = self.pool.clone();
        let runtime = Arc::clone(&self.runtime);

        let span = Span::current();
        let cert = tokio::task::spawn_blocking(move || {
            let _span = span.enter();
            let account = directory.account("acme-dns-rust@byom.de")?;
            let order = account.new_order("acme.conblem.me", &[])?;
            CertManager::validate(memory_cert, domain, order, &pool, &runtime)
        })
        .await??;

        CertFacade::stop(&self.pool, cert).await?;

        Ok(())
    }

    fn validate(
        mut memory_cert: Cert,
        mut domain: Domain,
        mut order: NewOrder<DatabasePersist>,
        pool: &PgPool,
        runtime: &Runtime,
    ) -> Result<Cert> {
        let ord_csr = loop {
            if let Some(ord_csr) = order.confirm_validations() {
                break ord_csr;
            }

            let chall = order
                .authorizations()?
                .iter()
                .next()
                .ok_or_else(|| anyhow!("couldn't unpack auths"))?
                .dns_challenge();

            domain.txt = Some(chall.dns_proof());
            let update = DomainFacade::update_domain(pool, &domain);
            runtime.block_on(update.in_current_span())?;

            chall.validate(5000)?;
            order.refresh()?;
        };

        let private = create_p384_key();
        let ord_crt = ord_csr.finalize_pkey(private, 5000)?;
        let cert = ord_crt.download_and_save_cert()?;

        let private = cert.private_key().to_string();
        let cert = cert.certificate().to_string();

        memory_cert.private = Some(private);
        memory_cert.cert = Some(cert);

        Ok(memory_cert)
    }
}
