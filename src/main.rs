use anyhow::Result;
use futures_util::TryFutureExt;
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::env;
use std::str::FromStr;
use tokio::runtime::Runtime;
use tracing::{debug, error, info};

use crate::acme::DatabasePersist;
use crate::api::Api;
use crate::cert::CertManager;
use crate::dns::{DatabaseAuthority, DNS};
use tracing_futures::Instrument;

mod acme;
mod api;
mod cert;
mod config;
mod dns;
mod domain;
mod util;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() {
    tracing_subscriber::fmt::init();

    if run().is_err() {
        std::process::exit(1);
    }
}

#[tracing::instrument]
fn run() -> Result<()> {
    let config_path = env::args().nth(1);
    let config = config::load_config(config_path)?;

    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            error!("{}", e);
            return Err(e.into());
        }
    };
    debug!("Created runtime");

    // Async closure cannot be move, if runtime gets moved into it
    // it gets dropped inside an async call
    let res: Result<()> = runtime.handle().block_on(
        async {
            let pool = setup_database(&config.general.db).in_current_span().await?;
            let authority =
                DatabaseAuthority::new(pool.clone(), &config.general.name, config.records);
            let dns = DNS::new(&config.general.dns, &runtime, authority);

            let api = Api::new(
                config.api.http.as_deref(),
                config.api.https.as_deref(),
                pool.clone(),
            )
            .and_then(Api::spawn);

            let persist = DatabasePersist::new(pool.clone(), runtime.handle());
            let cert_manager =
                CertManager::new(pool, persist, config.general.acme).and_then(CertManager::spawn);

            info!("Starting API Cert Manager and DNS");
            tokio::try_join!(api, cert_manager, dns.spawn())?;

            Ok(())
        }
        .in_current_span(),
    );

    if let Err(e) = res {
        error!("{}", e);
        return Err(e.into());
    };

    Ok(())
}

#[tracing::instrument(skip(db))]
async fn setup_database(db: &str) -> Result<PgPool, sqlx::Error> {
    let pool = async {
        let options = PgConnectOptions::from_str(db)?;
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        debug!("Created DB pool");

        MIGRATOR.run(&pool).await?;
        info!("Ran migration");
        Ok(pool)
    }
    .in_current_span()
    .await;

    match pool {
        Ok(pool) => Ok(pool),
        Err(e) => {
            error!("{}", e);
            Err(e)
        }
    }
}
