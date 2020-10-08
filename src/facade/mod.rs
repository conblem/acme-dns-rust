#[cfg(feature = "mysql")]
use sqlx::MySqlPool;
#[cfg(feature = "postgres")]
use sqlx::PgPool;

use self::cert::{CertFacade, CertFacadeImpl};

mod cert;

#[cfg(feature = "postgres")]
pub(crate) fn new(pool: PgPool) -> impl CertFacade {
    CertFacadeImpl { pool }
}

#[cfg(not(feature = "postgres"))]
pub(crate) fn new(pool: MySqlPool) -> impl CertFacade {
    CertFacadeImpl { pool }
}
