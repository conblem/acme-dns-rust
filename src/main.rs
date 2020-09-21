use simplelog::{Config, LevelFilter, SimpleLogger};
use sqlx::migrate::Migrator;
use sqlx::PgPool;
use std::error::Error;
use tokio::runtime::Runtime;

use crate::api::Api;
use crate::cert::CertManager;
use crate::dns::DNS;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

mod api;
mod cert;
mod dns;
mod domain;
mod config;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() -> Result<(), Box<dyn Error>> {
    let mut runtime = Runtime::new()?;
    SimpleLogger::init(LevelFilter::Debug, Config::default())?;

    let config = runtime.block_on(config::config())?;
    println!("{:?}", config);

    let pool = runtime.block_on(setup_database())?;

    let dns = runtime
        .block_on(DNS::builder(config.general.listen))?
        .build(pool.clone(), &runtime);

    let https = format!("{}:{}", config.api.ip, config.api.port);
    let api = runtime.block_on(Api::new(
        Some("0.0.0.0:8080"),
        Some(&https),
        pool.clone(),
    ))?;

    let cert_manager = CertManager::new(pool);

    runtime.block_on(
        async move { tokio::try_join!(cert_manager.spawn(), dns.spawn(), api.spawn()) },
    )?;

    Ok(())
}

async fn setup_database() -> Result<PgPool, sqlx::Error> {
    let options =
        PgConnectOptions::from_str("postgresql://postgres:mysecretpassword@localhost/postgres")?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    MIGRATOR.run(&pool).await?;

    Ok(pool)
}
