use simplelog::{Config, LevelFilter, SimpleLogger};
use sqlx::migrate::Migrator;
use sqlx::PgPool;
use std::error::Error;
use tokio::runtime::Runtime;

use crate::api::Api;
use crate::cert::{CertFacadeTwo, CertManager};
use crate::dns::DNS;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

mod api;
mod cert;
mod dns;
mod domain;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() -> Result<(), Box<dyn Error>> {
    let mut runtime = Runtime::new()?;
    SimpleLogger::init(LevelFilter::Trace, Config::default())?;

    let pool = runtime.block_on(setup_database())?;

    let dns = runtime
        .block_on(DNS::builder("0.0.0.0:3053".to_string()))?
        .build(pool.clone(), &runtime);

    let api = runtime.block_on(Api::new(
        Some("0.0.0.0:8080"),
        Some("0.0.0.0:8081"),
        pool.clone(),
    ))?;

    let _facade = CertFacadeTwo::new(pool.clone());

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
