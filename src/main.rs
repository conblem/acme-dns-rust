use anyhow::Result;
use futures_util::TryFutureExt;
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::env;
use std::str::FromStr;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::signal::ctrl_c;
use tracing::{debug, error, info, Instrument};

use acme::DatabasePersist;
use api::Api;
use cert::CertManager;
use dns::{DatabaseAuthority, DNS};

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

    if let Err(e) = run() {
        error!("{}", e);
        std::process::exit(1);
    }
}

#[tracing::instrument(err)]
fn run() -> Result<()> {
    let config_path = env::args().nth(1);
    let config = config::load_config(config_path)?;

    let runtime = Arc::new(Runtime::new()?);
    debug!("Created runtime");

    // Async closure cannot be move, if runtime gets moved into it
    // it gets dropped inside an async call
    let fut = async {
        debug!("Running in runtime");

        let pool = setup_database(&config.general.db).await?;
        let authority = DatabaseAuthority::new(pool.clone(), &config.general.name, config.records);
        let dns = DNS::new(&config.general.dns, authority);

        let api = &config.api;
        let api = api::new(
            (api.http.clone(), api.http_proxy),
            (api.https.clone(), api.https_proxy),
            (api.prom.clone(), api.prom_proxy),
            pool.clone(),
        )
        .and_then(Api::spawn);

        let persist = DatabasePersist::new(pool.clone(), &runtime);
        let cert_manager = CertManager::new(pool, persist, config.general.acme, &runtime)
            .and_then(CertManager::spawn);

        info!("Starting API Cert Manager and DNS");
        tokio::select! {
            res = api => res,
            res = cert_manager => res,
            res = dns.spawn() => res,
            res = ctrl_c() => {
                res?;
                info!("Ctrl C pressed");
                Ok(())
            }
        }
    };

    runtime.block_on(fut.in_current_span())
}

#[tracing::instrument(skip(db))]
async fn setup_database(db: &str) -> Result<PgPool, sqlx::Error> {
    debug!("Starting DB Setup");
    let options = PgConnectOptions::from_str(db)?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    debug!(?pool, "Created DB pool");

    MIGRATOR.run(&pool).await?;
    info!("Ran migration");
    Ok(pool)
}
