use simplelog::{LevelFilter, Config, SimpleLogger};
use warp::{Filter, Reply};
use warp::reject::not_found;
use serde::{Serialize, Deserialize};
use sqlx::{Executor, Pool, Database, Postgres, Row};
use sqlx::postgres::{PgPoolOptions, PgRow};
use chrono::Duration;
use crate::cert::{CertFacade, CertManager};
use sqlx::migrate::Migrator;

mod cert;

static MIGRATOR: Migrator = sqlx::migrate!("migrations/postgres");

#[derive(sqlx::FromRow, Debug, Serialize, Deserialize)]
struct Domain { id: String, username: String, password: String }

struct DomainDOT {
    id: String,
    username: String,
    password: String
}

impl DomainDOT {
    fn new() -> Self {
        let id = uuid::Uuid::new_v4().to_simple().to_string();
        let username = uuid::Uuid::new_v4().to_simple().to_string();
        let password = uuid::Uuid::new_v4().to_simple().to_string();

        DomainDOT {
            id,
            username,
            password
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    SimpleLogger::init(LevelFilter::Trace, Config::default()).unwrap();

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect("postgresql://postgres:mysecretpassword@localhost/postgres")
        .await?;

    MIGRATOR.run(&pool).await.unwrap();

    /*let value = sqlx::query("select 1 + 1")
        .try_map(|row: PgRow| row.try_get::<i32, _>(0))
        .fetch_one(&pool)
        .await.unwrap();*/

    /*let one_hour_ago = chrono::Local::now().naive_local() - Duration::seconds(1);
    let mut cert: Cert = sqlx::query_as("SELECT * FROM certs LIMIT 1")
        .fetch_one(&pool).await?;*/


    let cert_facade = CertFacade::new(pool.clone());
    cert_facade.start().await;

    CertManager::new(cert_facade).test().await;

    let facade_pool = pool.clone();
    let domain_facade = DomainFacade::new(facade_pool);


    let hello = warp::path!("hello" / String)
        .and(warp::any().map(move || domain_facade.clone()));

    /*let serve = warp::serve(hello)
        .run(([127, 0, 0, 1], 3030));*/

    Ok(())
}

struct DomainFacade<DB: Database> {
    pool: Pool<DB>
}

impl <DB: Database> DomainFacade<DB> {
    fn new(pool: Pool<DB>) -> Self {
        DomainFacade {
            pool
        }
    }
}

impl <DB: Database> Clone for DomainFacade<DB> {
    fn clone(&self) -> Self {
        let pool = Clone::clone(&self.pool);
        DomainFacade::new(pool)
    }
}

impl DomainFacade<Postgres> {
    async fn find_by_id(self, id: &str) -> Option<Domain> {
        sqlx::query_as("SELECT * FROM domain WHERE id = $1 LIMIT 1")
            .bind(id)
            .fetch_optional(&self.pool).await.unwrap()
    }
}