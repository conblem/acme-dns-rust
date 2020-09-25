use simplelog::{Config, LevelFilter, SimpleLogger};
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::env;
use std::error::Error;
use std::str::FromStr;
use tokio::runtime::Runtime;

use crate::acme::DatabasePersist;
use crate::api::Api;
use crate::cert::CertManager;
use crate::dns::DNS;
use futures_util::TryFutureExt;

mod acme;
mod api;
mod cert;
mod config;
mod dns;
mod domain;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() -> Result<(), Box<dyn Error>> {
    SimpleLogger::init(LevelFilter::Debug, Config::default())?;

    let runtime = Runtime::new()?;
    runtime.handle().clone().block_on(async move {
        let config_path = env::args().skip(1).next();
        let config = config::config(config_path).await?;

        let pool = setup_database(&config.general.db).await?;
        let dns = DNS::new(pool.clone(), &config.general.dns, &runtime);

        let api = Api::new(
            config.api.http.as_deref(),
            config.api.https.as_deref(),
            pool.clone(),
        )
        .map_err(From::from)
        .and_then(|api| api.spawn());

        let handle = runtime.handle().clone();
        let persist = DatabasePersist::new(pool.clone(), handle);
        let cert_manager = CertManager::new(pool, persist, config.general.acme)
            .and_then(|cert_manager| cert_manager.spawn());

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
