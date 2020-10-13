use anyhow::Result;
use futures_util::TryFutureExt;
use simplelog::{Config, LevelFilter, SimpleLogger};
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::env;
use std::str::FromStr;
use tokio::runtime::Runtime;

use crate::acme::DatabasePersist;
use crate::api::Api;
use crate::cert::CertManager;
use crate::dns::{DatabaseAuthority, DNS};

mod acme;
mod api;
mod cert;
mod config;
mod dns;
mod domain;
mod util;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() {
    SimpleLogger::init(LevelFilter::Debug, Config::default()).unwrap();

    if let Err(e) = run() {
        log::error!("{:?}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let config_path = env::args().nth(1);
    let config = config::config(config_path)?;

    let runtime = Runtime::new()?;
    // Async closure cannot be move, if runtime gets moved into it
    // it gets dropped inside an async call
    runtime.handle().block_on(async {
        let pool = setup_database(&config.general.db).await?;
        let authority = DatabaseAuthority::new(pool.clone(), &config.general.name, config.records);
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

        tokio::try_join!(api, cert_manager, dns.spawn())?;

        Ok(())
    })
}

async fn setup_database(db: &str) -> Result<PgPool, sqlx::Error> {
    let options = PgConnectOptions::from_str(db)?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    MIGRATOR.run(&pool).await?;

    Ok(pool)
}
