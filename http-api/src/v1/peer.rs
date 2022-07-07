use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde_json::json;

use librad::PeerId;

use crate::Context;

pub fn router(ctx: Context) -> Router {
    let peer_id = ctx.peer_id;

    Router::new()
        .route("/peer", get(peer_handler))
        .layer(Extension(peer_id))
}

/// Return the peer id for the node identity.
/// `GET /peer`
async fn peer_handler(Extension(peer_id): Extension<PeerId>) -> impl IntoResponse {
    let response = json!({
        "id": peer_id.to_string(),
    });

    Json(response)
}
