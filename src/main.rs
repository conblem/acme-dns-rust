use simplelog::{LevelFilter, Config, SimpleLogger};
use sqlx::{Pool, Postgres};
use sqlx::postgres::{PgPoolOptions};
use sqlx::migrate::Migrator;
use tokio::runtime::Runtime;

use crate::http::Http;
use crate::dns::DNS;

mod cert;
mod dns;
mod domain;
mod http;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() -> Result<(), sqlx::Error> {
    // use error handling
    let mut runtime = Runtime::new().unwrap();
    SimpleLogger::init(LevelFilter::Trace, Config::default()).unwrap();


    let pool = setup_database(&mut runtime);

    let dns = runtime.block_on(
        DNS::<Postgres>::new(pool.clone(), "0.0.0.0:3053".to_string())
    );
    let dns = dns.register_socket(&runtime);

    let http = Http::new();

    runtime.block_on(async move {
        //let cert_manager = CertManager::new(pool);
        //cert_manager.test().await;

        let dns_future = tokio::spawn(dns.run());
        let http_future = tokio::spawn(http.run());

        tokio::join!(dns_future, http_future)
    });

    Ok(())
}

fn setup_database(runtime: &mut Runtime) -> Pool<Postgres> {
    runtime.block_on(async {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect("postgresql://postgres:mysecretpassword@localhost/postgres")
            .await.unwrap();

        MIGRATOR.run(&pool).await.unwrap();

        pool
    })
}

