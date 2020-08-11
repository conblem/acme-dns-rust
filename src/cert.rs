use sqlx::{Database, FromRow, Pool, Postgres, Transaction, Executor};
use chrono::naive::NaiveDateTime;
use chrono::{Duration, Local};

#[derive(sqlx::Type, Debug, PartialEq)]
#[repr(i32)]
enum State { Ok = 0, Updating = 1 }

#[derive(FromRow, Debug)]
struct Cert {id: String, update: NaiveDateTime, state: State}

pub struct CertFacade<DB: Database> {
    pool: Pool<DB>
}

impl <DB: Database> CertFacade<DB> {
    pub fn new(pool: Pool<DB>) -> Self {
        CertFacade {
            pool
        }
    }
}

impl CertFacade<Postgres> {
    pub async fn find_by_id(&self, id: &str) -> Option<Cert> {
        sqlx::query_as("SELECT * FROM certs WHERE id = $1 LIMIT 1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .unwrap_or(None)
    }

    async fn update_cert<'a, E: Executor<'a, Database = Postgres>>(executor: E, cert: &Cert) {
        sqlx::query("UPDATE certs SET update = $1, state = $1 WHERE id = $3")
            .bind(&cert.update)
            .bind(&cert.state)
            .bind(&cert.id)
            .execute(executor)
            .await
            .unwrap();
    }

    pub async fn create_cert(&self) {
        let mut transaction = self.pool.begin().await.unwrap();

        let cert = sqlx::query_as::<Postgres, Cert>("SELECT * FROM certs LIMIT 1")
            .fetch_optional(&mut transaction)
            .await
            .unwrap_or(None);

        match cert {
            Some(mut cert) if cert.state == State::Ok => {
                cert.state = State::Updating;
                CertFacade::update_cert(&mut transaction, &cert).await
            },
            Some(mut cert) => {
                let one_hour_ago = Local::now().naive_local() - Duration::hours(1);
                if cert.update < one_hour_ago {
                    cert.update = Local::now().naive_local();
                    CertFacade::update_cert(&mut transaction, &cert).await
                }
            },
            None => {
                let cert = Cert {
                    id: uuid::Uuid::new_v4().to_simple().to_string(),
                    update: chrono::Local::now().naive_local(),
                    state: State::Ok
                };

                sqlx::query("INSERT INTO certs (id, update, state) VALUES ($1, $2, $3)")
                    .bind(&cert.id)
                    .bind(&cert.update)
                    .bind(&cert.state)
                    .execute(&mut transaction).await.unwrap();
            },
        }

        transaction.commit().await.unwrap();
    }
}