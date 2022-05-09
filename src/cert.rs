use acme_lib::order::NewOrder;
use acme_lib::{create_p384_key, Directory, DirectoryUrl};
use anyhow::{anyhow, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::time::Interval;
use tracing::{error, info, Instrument, Span};

use crate::acme::DatabasePersist;
use crate::facade::{Cert, CertFacade, Domain, DomainFacade};
use crate::util::HOUR_IN_SECONDS;

#[derive(Clone)]
pub struct CertManager<F> {
    facade: F,
    directory: Directory<DatabasePersist>,
    runtime: Arc<Runtime>,
}

fn interval() -> Interval {
    tokio::time::interval(Duration::from_secs(HOUR_IN_SECONDS))
}

impl<F> CertManager<F>
where
    F: DomainFacade + CertFacade + Clone + Send + Sync + 'static,
{
    #[tracing::instrument(name = "CertManager::new", skip(facade, persist, runtime))]
    pub async fn new(
        facade: F,
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
            facade,
            directory,
            runtime: Arc::clone(runtime),
        })
    }

    // maybe useless function

    #[tracing::instrument(name = "CertManager::spawn", skip(self))]
    pub async fn spawn(self) -> Result<()> {
        tokio::spawn(
            async move {
                let mut interval = interval();
                loop {
                    interval.tick().await;
                    info!("Started Interval");
                    if true {
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
        let memory_cert = match self.facade.start_cert().await? {
            Some(memory_cert) => memory_cert,
            None => return Ok(()),
        };

        let domain = self
            .facade
            .find_domain_by_id(&memory_cert.domain)
            .await?
            .ok_or_else(|| anyhow!("Could not find domain: {}", &memory_cert.domain))?;

        let directory = self.directory.clone();
        let facade = self.facade.clone();
        let runtime = Arc::clone(&self.runtime);

        let span = Span::current();
        let mut cert = tokio::task::spawn_blocking(move || {
            let _span = span.enter();
            let account = directory.account("acme-dns-rust@byom.de")?;
            let order = account.new_order("acme.conblem.me", &[])?;
            CertManager::validate(memory_cert, domain, order, facade, &runtime)
        })
        .await??;

        self.facade.stop_cert(&mut cert).await?;

        Ok(())
    }

    fn validate(
        mut memory_cert: Cert,
        mut domain: Domain,
        mut order: NewOrder<DatabasePersist>,
        facade: F,
        runtime: &Runtime,
    ) -> Result<Cert> {
        let ord_csr = loop {
            if let Some(ord_csr) = order.confirm_validations() {
                break ord_csr;
            }

            let chall = order
                .authorizations()?
                .get(0)
                .ok_or_else(|| anyhow!("couldn't unpack auths"))?
                .dns_challenge();

            domain.txt = Some(chall.dns_proof());
            let update = facade.update_domain(&domain);
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
