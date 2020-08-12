use simplelog::{LevelFilter, Config, SimpleLogger};
use warp::{Filter, Reply};
use warp::reject::not_found;
use sqlx::{Pool, Postgres};
use sqlx::postgres::{PgPoolOptions};
use sqlx::migrate::Migrator;
use tokio::net::UdpSocket;
use tokio::runtime::Runtime;
use crate::domain::{DomainFacade, Domain};
use crate::cert::{CertFacade, CertManager};

mod cert;
mod dns;
mod domain;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

fn main() -> Result<(), sqlx::Error> {
    // use error handling
    let mut runtime = Runtime::new().unwrap();

    SimpleLogger::init(LevelFilter::Trace, Config::default()).unwrap();

    let pool: Pool<Postgres> = runtime.block_on(async {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect("postgresql://postgres:mysecretpassword@localhost/postgres")
            .await.unwrap();

        MIGRATOR.run(&pool).await.unwrap();
        CertFacade::start(&pool).await;

        pool
    });

    /*let value = sqlx::query("select 1 + 1")
        .try_map(|row: PgRow| row.try_get::<i32, _>(0))
        .fetch_one(&pool)
        .await.unwrap();*/

    /*let hello = warp::path!("hello" / String)
        .and(warp::any().map(move || domain_facade.clone()));*/


    /*runtime.block_on(async {
        let domain = Domain {
            id: uuid::Uuid::new_v4().to_simple().to_string(),
            username: uuid::Uuid::new_v4().to_simple().to_string(),
            password: bcrypt::hash(uuid::Uuid::new_v4().to_simple().to_string(), bcrypt::DEFAULT_COST).unwrap(),
            txt: None
        };
        domain_facade.create_domain(&domain).await;
    });*/

    let mut test = dns::DNS::<Postgres>::new(pool.clone());
    let udp = runtime.block_on(async {
        UdpSocket::bind("0.0.0.0:53".to_string()).await.unwrap()
    });
    test.run(udp, &runtime);

    runtime.block_on(async {
        let server = tokio::spawn(async {
            test.block_until_done().await;
        });

        //let cert_manager = CertManager::new(pool);
        //cert_manager.test().await;
        server.await;
    });

    Ok(())
}

