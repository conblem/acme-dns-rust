#[cfg(feature = "mysql")]
use sqlx::MySqlPool;
#[cfg(feature = "postgres")]
use sqlx::PgPool;
use sqlx::{ColumnIndex, Database, Decode, Encode, Error, Executor, IntoArguments, Pool, Type};

use crate::cert::Cert;
use sqlx::database::HasArguments;
use std::future::Future;
use std::pin::Pin;

pub(crate) trait CertFacade: Send + Sync + Clone {
    fn first_cert(&self) -> Pin<Box<dyn Future<Output = Result<Option<Cert>, sqlx::Error>>>>;
    fn update_cert(&self, cert: Cert) -> Pin<Box<dyn Future<Output = Result<(), sqlx::Error>>>>;
}

#[derive(Clone)]
struct CertFacadeImpl<T> {
    pool: T,
}

impl<DB: 'static + Database> CertFacade for CertFacadeImpl<Pool<DB>>
where
    <DB as HasArguments<'static>>::Arguments: IntoArguments<'static, DB>,
    for<'b, 'c> &'b Pool<DB>: Executor<'c, Database = DB>,
    for<'b> String: Encode<'b, DB> + Decode<'b, DB> + Type<DB>,
    for<'b> Option<String>: Encode<'b, DB> + Decode<'b, DB> + Type<DB>,
    for<'b> i32: Encode<'b, DB> + Decode<'b, DB> + Type<DB>,
    for<'b> i64: Encode<'b, DB> + Decode<'b, DB> + Type<DB>,
    for<'b> &'b str: ColumnIndex<DB::Row>,
{
    fn first_cert(&self) -> Pin<Box<dyn Future<Output = Result<Option<Cert>, sqlx::Error>>>> {
        let pool = self.pool.clone();
        Box::pin(async move {
            sqlx::query_as("SELECT * FROM cert LIMIT 1")
                .fetch_optional(&pool)
                .await
        })
    }

    fn update_cert(&self, cert: Cert) -> Pin<Box<dyn Future<Output = Result<(), Error>>>> {
        let pool = self.pool.clone();
        Box::pin(async move {
            sqlx::query("UPDATE cert SET update = $1, state = $2, cert = $3, private = $4, domain_id = $5 WHERE id = $6")
                .bind(cert.update)
                .bind(cert.state)
                .bind(cert.cert)
                .bind(cert.private)
                .bind(cert.domain)
                .bind(cert.id)
                .execute(&pool)
                .await?;

            Ok(())
        })
    }
}

#[cfg(feature = "postgres")]
pub(crate) fn new(pool: PgPool) -> impl CertFacade {
    CertFacadeImpl { pool }
}
#[cfg(feature = "mysql")]
pub(crate) fn new(pool: MySqlPool) -> impl CertFacade {
    CertFacadeImpl { pool }
}
