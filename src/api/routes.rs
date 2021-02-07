use anyhow::Result;
use sqlx::PgPool;
use std::convert::TryFrom;
use tracing::error;
use warp::filters::trace;
use warp::http::{Response, StatusCode};
use warp::reply::Response as WarpResponse;
use warp::{Filter, Rejection, Reply};

use super::{metrics_wrapper, MetricsConfig};
use crate::domain::{Domain, DomainDTO, DomainFacade};

async fn register_handler(pool: PgPool) -> Result<WarpResponse, Rejection> {
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

const X_API_USER_HEADER: &str = "X-Api-User";
const X_API_KEY_HEADER: &str = "X-Api-Key";

async fn update_handler(
    user: String,
    key: String,
    _pool: PgPool,
) -> Result<WarpResponse, Rejection> {
    Ok(format!("{} {}", user, key).into_response())
}

const REGISTER_PATH: &str = "register";
const UPDATE_PATH: &str = "update";

pub(super) fn routes(
    pool: PgPool,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone + Send + Sync + 'static {
    let pool = warp::any().map(move || pool.clone());

    let register = warp::path(REGISTER_PATH)
        .and(warp::post())
        .and(pool.clone())
        .and_then(register_handler)
        .and(MetricsConfig::path());

    let update = warp::path(UPDATE_PATH)
        .and(warp::post())
        .and(warp::header(X_API_USER_HEADER))
        .and(warp::header(X_API_KEY_HEADER))
        .and(pool)
        .and_then(update_handler)
        .and(MetricsConfig::path());

    let not_found = warp::any()
        .map(warp::reply)
        .and_then(|reply| async move {
            let res = warp::reply::with_status(reply, StatusCode::NOT_FOUND).into_response();
            Ok(res) as Result<WarpResponse, Rejection>
        })
        .and(MetricsConfig::new("404"));

    register
        .or(update)
        .unify()
        .or(not_found)
        .unify()
        .with(warp::wrap_fn(metrics_wrapper))
        .with(trace::request())
}
