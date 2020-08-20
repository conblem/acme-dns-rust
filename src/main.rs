use simplelog::{LevelFilter, Config, SimpleLogger};
use sqlx::{Pool, Postgres, AnyPool};
use sqlx::postgres::{PgPoolOptions};
use sqlx::migrate::Migrator;
use tokio::runtime::Runtime;
use std::error::Error;

use crate::http::Http;
use crate::dns::DNS;
use crate::cert::CertManager;
use sqlx::any::{AnyPoolOptions, AnyConnectOptions, AnyKind};
use std::str::FromStr;
use crate::domain::DomainFacade;

mod cert;
mod dns;
mod domain;
mod http;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() -> Result<(), Box<dyn Error>> {
    // use error handling
    let mut runtime = Runtime::new()?;
    SimpleLogger::init(LevelFilter::Trace, Config::default())?;

    let (pool, kind) = runtime.block_on(setup_database())?;

    let domain_facade = DomainFacade::new();
    let cert_facade = CertFacade::new();

    let dns = runtime.block_on(
        DNS::builder("0.0.0.0:3053".to_string())
    )?.build(pool.clone(), &runtime);

    let http = runtime.block_on(Http::new(
        Some("0.0.0.0:8080"),
        Some("0.0.0.0:8081")
    ))?;

    let cert_manager = CertManager::new(pool, http.clone());
    runtime.spawn(cert_manager.job());

    runtime.block_on(async move {
        let dns_future = tokio::spawn(dns.run());
        let http_future = tokio::spawn(http.run());

        tokio::join!(dns_future, http_future)
    });

    Ok(())
}


async fn setup_database() -> Result<(AnyPool, AnyKind), sqlx::Error> {
    let options = AnyConnectOptions::from_str("postgresql://postgres:mysecretpassword@localhost/postgres")?;
    let kind = options.kind();
    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    MIGRATOR.run(&pool).await?;

    Ok((pool, kind))
}

