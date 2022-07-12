mod delegates;
mod peer;
mod sessions;

use axum::Router;

use crate::Context;

pub fn router(ctx: Context) -> Router {
    let routes = Router::new()
        .merge(peer::router(ctx.clone()))
        .merge(sessions::router(ctx.clone()))
        .merge(delegates::router(ctx.clone()));

    Router::new().nest("/v1", routes)
}
