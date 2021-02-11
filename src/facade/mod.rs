use sqlx::PgPool;

mod cert;
mod domain;

pub use cert::{Cert, CertFacade, State};
pub use domain::{Domain, DomainDTO, DomainFacade};

#[derive(Debug, Clone)]
pub(super) struct PostgresFacade {
    pool: PgPool,
}

impl From<PgPool> for PostgresFacade {
    fn from(pool: PgPool) -> Self {
        PostgresFacade { pool }
    }
}
