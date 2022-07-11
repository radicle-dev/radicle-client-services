use crate::Context;

use axum::Router;

mod delegates;
mod peer;
mod sessions;

pub fn router(ctx: Context) -> Router {
    let routes = Router::new()
        .merge(peer::router(ctx.clone()))
        .merge(sessions::router(ctx.clone()))
        .merge(delegates::router(ctx.clone()));

    Router::new().nest("/v1", routes)
}
