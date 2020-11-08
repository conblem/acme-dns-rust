use anyhow::Result;
use sqlx::PgPool;
use std::convert::TryFrom;
use tracing::error;
use warp::http::{Response, StatusCode};
use warp::{Filter, Rejection, Reply};

use super::metrics_wrapper;
use crate::domain::{Domain, DomainDTO, DomainFacade};

async fn register_handler(pool: PgPool) -> Result<impl Reply, Rejection> {
    let res: Result<DomainDTO> = async {
        let res = DomainDTO::default();
        let domain = Domain::try_from(res.clone())?;
        DomainFacade::create_domain(&pool, &domain).await?;
        Ok(res)
    }
    .await;

    let mut res = match res {
        Ok(res) => warp::reply::json(&res).into_response(),
        Err(e) => {
            error!("{}", e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body("test")
                .into_response()
        }
    };

    // warp::json also returns StatusCode 500 if serializing failed
    if res.status() != StatusCode::INTERNAL_SERVER_ERROR {
        *res.status_mut() = StatusCode::CREATED;
    }
    Ok(res)
}

const X_API_USER_HEADER: &'static str = "X-Api-User";
const X_API_KEY_HEADER: &'static str = "X-Api-Key";

async fn update_handler(user: String, key: String, _pool: PgPool) -> Result<impl Reply, Rejection> {
    Ok(format!("{} {}", user, key))
}

const REGISTER_PATH: &'static str = "register";
const UPDATE_PATH: &'static str = "update";

pub(super) fn routes(
    pool: PgPool,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone + Send + Sync + 'static {
    let pool = warp::any().map(move || pool.clone());

    let register = warp::path(REGISTER_PATH)
        .and(warp::post())
        .and(pool.clone())
        .and_then(register_handler)
        .with(warp::wrap_fn(metrics_wrapper(None)));

    let update = warp::path(UPDATE_PATH)
        .and(warp::post())
        .and(warp::header(X_API_USER_HEADER))
        .and(warp::header(X_API_KEY_HEADER))
        .and(pool)
        .and_then(update_handler)
        .with(warp::wrap_fn(metrics_wrapper(None)));

    register.or(update)
}
