use anyhow::Result;
use futures_util::TryFutureExt;
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::env;
use std::str::FromStr;
use tokio::runtime::Runtime;
use tokio::signal::ctrl_c;
use tracing::{debug, error, info, Instrument};

use crate::acme::DatabasePersist;
use crate::api::Api;
use crate::cert::CertManager;
use crate::dns::{DatabaseAuthority, DNS};
use std::sync::Arc;

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
    let res: Result<()> = runtime.block_on(
        async {
            debug!("Running in runtime");

            let pool = setup_database(&config.general.db).await?;
            let authority =
                DatabaseAuthority::new(pool.clone(), &config.general.name, config.records);
            let dns = DNS::new(&config.general.dns, authority);

            let api = &config.api;
            let api = Api::new(
                (api.http.as_deref(), api.http_proxy),
                (api.https.as_deref(), api.https_proxy),
                (api.prom.as_deref(), api.prom_proxy),
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
        }
        .in_current_span(),
    );

    res?;

    Ok(())
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
