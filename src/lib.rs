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
use tracing::{debug, info, Instrument};

use acme::DatabasePersist;
use cert::CertManager;
use dns::{DatabaseAuthority, Dns};
use facade::DatabaseFacade;

mod acme;
pub mod api;
mod cert;
mod config;
mod dns;
pub mod facade;
pub mod util;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

#[tracing::instrument]
pub fn run() -> Result<()> {
    let config_path = env::args().nth(1);
    let config = config::load_config(config_path)?;

    let runtime = Arc::new(Runtime::new()?);
    debug!("Created runtime");

    // Async closure cannot be move, if runtime gets moved into it
    // it gets dropped inside an async call
    let fut = async {
        debug!("Running in runtime");

        let pool = setup_database(&config.general.db).await?;
        let facade = DatabaseFacade::from(pool.clone());
        let authority =
            DatabaseAuthority::new(facade.clone(), &config.general.name, config.records);
        let dns = Dns::new(&config.general.dns, authority);

        let api = &config.api;
        let api = api::new(
            api.http.clone(),
            api.https.clone(),
            api.prom.clone(),
            facade.clone(),
        );

        let persist = DatabasePersist::new(pool, &runtime);
        let cert_manager = CertManager::new(facade, persist, config.general.acme, &runtime)
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

#[cfg(test)]
mod tests {
    use testcontainers::*;

    use super::setup_database;

    #[cfg(not(feature = "disable-docker"))]
    #[tokio::test]
    async fn test_setup_database() {
        let docker = clients::Cli::default();
        let node = docker.run(images::postgres::Postgres::default());

        let connection_string = &format!(
            "postgres://postgres:postgres@localhost:{}/postgres",
            node.get_host_port(5432).unwrap()
        );

        let pool = setup_database(connection_string).await.unwrap();

        let actual: (i64,) = sqlx::query_as("SELECT $1")
            .bind(150_i64)
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(150_i64, actual.0)
    }
}
