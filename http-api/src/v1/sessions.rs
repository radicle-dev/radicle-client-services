use std::collections::HashMap;
use std::convert::TryInto;
use std::env;
use std::iter::repeat_with;
use std::str::FromStr;
use std::time::Duration;

use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use ethers_core::utils::hex;
use hyper::http::uri::Authority;
use serde_json::json;
use siwe::Message;

use crate::auth::{AuthRequest, AuthState, Session};
use crate::axum_extra::Path;
use crate::{Context, Error};

pub const UNAUTHORIZED_SESSIONS_EXPIRATION: Duration = Duration::from_secs(60);

/*
 *
fn session_filters(ctx: Context) -> BoxedFilter<(impl Reply,)> {
    session_create_filter(ctx.clone())
        .or(session_signin_filter(ctx.clone()))
        .or(session_get_filter(ctx))
        .boxed()
}

/// `POST /sessions`
fn session_create_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::post()
        .map(move || ctx.clone())
        .and(path::end())
        .and_then(session_create_handler)
}

/// `PUT /sessions/:session-id`
fn session_signin_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::put()
        .map(move || ctx.clone())
        .and(path::param::<String>())
        .and(path::end())
        .and(warp::body::json())
        .and_then(session_signin_handler)
}

/// `GET /sessions/:session-id`
fn session_get_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<String>())
        .and(path::end())
        .and_then(session_get_handler)
}
*/

pub fn router(ctx: Context) -> Router {
    Router::new()
        .route("/sessions", post(session_create_handler))
        .route(
            "/sessions/:session_id",
            get(session_get_handler).put(session_signin_handler),
        )
        .layer(Extension(ctx.clone()))
}

/// Create session.
/// `POST /sessions`
async fn session_create_handler(Extension(ctx): Extension<Context>) -> impl IntoResponse {
    let expiration_time =
        Utc::now() + chrono::Duration::from_std(UNAUTHORIZED_SESSIONS_EXPIRATION).unwrap();
    let mut sessions = ctx.sessions.write().await;
    let (session_id, nonce) = create_session(&mut sessions, expiration_time);
    let response = Json(json!({ "id": session_id, "nonce": nonce }));

    response
}

/// Get session.
/// `GET /sessions/:session_id`
async fn session_get_handler(
    Extension(ctx): Extension<Context>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let id = session_id;
    let sessions = ctx.sessions.read().await;
    let session = sessions.get(&id).ok_or(Error::NotFound)?;

    match session {
        AuthState::Authorized(session) => {
            Ok::<_, Error>(Json(json!({ "id": id, "session": session })))
        }
        AuthState::Unauthorized {
            nonce,
            expiration_time,
        } => Ok::<_, Error>(Json(
            json!({ "id": id, "nonce": nonce, "expirationTime": expiration_time }),
        )),
    }
}

/// Update session.
/// `PUT /sessions/:session_id`
async fn session_signin_handler(
    Extension(ctx): Extension<Context>,
    Path(session_id): Path<String>,
    Json(request): Json<AuthRequest>,
) -> impl IntoResponse {
    let id = session_id;
    // Get unauthenticated session data, return early if not found
    let mut sessions = ctx.sessions.write().await;
    let session = sessions.get(&id).ok_or(Error::NotFound)?;

    if let AuthState::Unauthorized { nonce, .. } = session {
        let message = Message::from_str(request.message.as_str()).map_err(Error::from)?;

        let host = env::var("RADICLE_DOMAIN").map_err(Error::from)?;

        // Validate nonce
        if *nonce != message.nonce {
            return Err(Error::Auth("Invalid nonce").into());
        }

        // Verify that domain is the correct one
        let authority = Authority::from_str(&host).map_err(|_| Error::Auth("Invalid host"))?;
        if authority != message.domain {
            return Err(Error::Auth("Invalid domain").into());
        }

        // Verifies the following:
        // - AuthRequest sig matches the address passed in the AuthRequest message.
        // - expirationTime is not in the past.
        // - notBefore time is in the future.
        message
            .verify(request.signature.into())
            .map_err(Error::from)?;

        let session: Session = message.try_into()?;
        sessions.insert(id.clone(), AuthState::Authorized(session.clone()));

        return Ok::<_, Error>(Json(json!({ "id": id, "session": session })));
    }

    Err(Error::Auth("Session already authorized").into())
}

fn create_session(
    map: &mut HashMap<String, AuthState>,
    expiration_time: DateTime<Utc>,
) -> (String, String) {
    let nonce = siwe::nonce::generate_nonce();

    // We generate a value from the RNG for the session id
    let rng = fastrand::Rng::new();
    let id = hex::encode(repeat_with(|| rng.u8(..)).take(32).collect::<Vec<u8>>());

    let auth_state = AuthState::Unauthorized {
        nonce: nonce.clone(),
        expiration_time,
    };

    map.insert(id.clone(), auth_state);

    (id, nonce)
}
