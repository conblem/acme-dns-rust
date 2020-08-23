use simplelog::{LevelFilter, Config, SimpleLogger};
use sqlx::{Pool, Postgres};
use sqlx::postgres::{PgPoolOptions};
use sqlx::migrate::Migrator;
use tokio::runtime::Runtime;
use std::error::Error;

use crate::http::Http;
use crate::dns::DNS;
use crate::cert::CertManager;

mod cert;
mod dns;
mod domain;
mod http;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() -> Result<(), Box<dyn Error>> {
    let mut runtime = Runtime::new()?;
    SimpleLogger::init(LevelFilter::Trace, Config::default())?;

    let pool = runtime.block_on(setup_database())?;

    let dns = runtime.block_on(
        DNS::builder("0.0.0.0:3053".to_string())
    )?.build(pool.clone(), &runtime);

    let http = runtime.block_on(Http::new(
        Some("0.0.0.0:8080"),
        Some("0.0.0.0:8081")
    ))?;

    let cert_manager = CertManager::new(pool, http.clone());
    runtime.spawn(cert_manager.job());

    let server = runtime.block_on(async move {
        let dns_future = tokio::spawn(dns.run());
        let http_future = tokio::spawn(http.run());

        match tokio::join!(dns_future, http_future) {
            (Err(e), _) => Err(e),
            (_, Err(e)) => Err(e),
            _ => Ok(())
        }
    })?;

    Ok(server)
}


async fn setup_database() -> Result<Pool<Postgres>, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect("postgresql://postgres:mysecretpassword@localhost/postgres")
        .await?;

    MIGRATOR.run(&pool).await?;

    Ok(pool)
}

