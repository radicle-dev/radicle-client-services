use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Json, Router};
use librad::git::identities::{self, SomeIdentity};
use serde_json::json;

use crate::Context;
use crate::Error;

pub fn router(ctx: Context) -> Router {
    Router::new()
        .route("/stats", get(stats_handler))
        .layer(Extension(ctx))
}

/// Return the stats for the node.
/// `GET /stats`
async fn stats_handler(Extension(ctx): Extension<Context>) -> impl IntoResponse {
    let storage = ctx.storage().await?;
    let (projects, persons): (Vec<_>, Vec<_>) = identities::any::list(storage.read_only())
        .map_err(Error::from)?
        .partition(|identity| match identity {
            Ok(SomeIdentity::Project(_)) => true,
            Ok(SomeIdentity::Person(_)) => false,
            _ => panic!("Error while listing identities"),
        });

    Ok::<_, Error>(Json(
        json!({ "projects": { "count": projects.len() }, "users": { "count": persons.len() } }),
    ))
}
