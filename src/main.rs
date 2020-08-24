use simplelog::{Config, LevelFilter, SimpleLogger};
use sqlx::migrate::Migrator;
use sqlx::AnyPool;
use std::error::Error;
use tokio::runtime::Runtime;

use crate::api::Api;
use crate::cert::CertManager;
use crate::dns::DNS;
use sqlx::any::{AnyConnectOptions, AnyKind, AnyPoolOptions};
use std::str::FromStr;

mod api;
mod cert;
mod dns;
mod domain;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() -> Result<(), Box<dyn Error>> {
    let mut runtime = Runtime::new()?;
    SimpleLogger::init(LevelFilter::Trace, Config::default())?;

    let (pool, kind) = runtime.block_on(setup_database())?;

    let dns = runtime
        .block_on(DNS::builder("0.0.0.0:3053".to_string()))?
        .build(pool.clone(), &runtime);

    let api = runtime.block_on(Api::new(Some("0.0.0.0:8080"), Some("0.0.0.0:8081")))?;

    let cert_manager = CertManager::new(pool, api.clone());
    runtime.spawn(cert_manager.job());

    let server = runtime.block_on(async move {
        let dns_future = tokio::spawn(dns.run());
        let http_future = tokio::spawn(api.run());

        match tokio::join!(dns_future, http_future) {
            (Err(e), _) => Err(e),
            (_, Err(e)) => Err(e),
            _ => Ok(()),
        }
    })?;

    Ok(server)
}

async fn setup_database() -> Result<(AnyPool, AnyKind), sqlx::Error> {
    let options =
        AnyConnectOptions::from_str("postgresql://postgres:mysecretpassword@localhost/postgres")?;
    let kind = options.kind();
    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    MIGRATOR.run(&pool).await?;

    Ok((pool, kind))
}
