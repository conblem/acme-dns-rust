use simplelog::{Config, LevelFilter, SimpleLogger};
use sqlx::migrate::Migrator;
use sqlx::PgPool;
use std::error::Error;
use tokio::runtime::Runtime;

use crate::api::Api;
use crate::cert::CertManager;
use crate::dns::DNS;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::env;
use std::str::FromStr;

mod api;
mod cert;
mod config;
mod dns;
mod domain;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() -> Result<(), Box<dyn Error>> {
    let mut runtime = Runtime::new()?;
    SimpleLogger::init(LevelFilter::Debug, Config::default())?;

    let config_path = env::args().skip(1).next();
    let config = runtime.block_on(config::config(config_path))?;

    let pool = runtime.block_on(setup_database(&config.general.db))?;

    let dns = runtime
        .block_on(DNS::builder(config.general.dns))?
        .build(pool.clone(), &runtime);

    let api = runtime.block_on(Api::new(
        config.api.http.as_deref(),
        config.api.https.as_deref(),
        pool.clone(),
    ))?;

    let cert_manager = CertManager::new(pool, config.general.acme);

    runtime.block_on(
        async move { tokio::try_join!(cert_manager.spawn(), dns.spawn(), api.spawn()) },
    )?;

    Ok(())
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
