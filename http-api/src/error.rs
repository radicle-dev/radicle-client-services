#![allow(clippy::large_enum_variant)]
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Errors that may occur when interacting with [`librad::net::peer::Peer`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An error occurred when performing git operations.
    #[error(transparent)]
    Git(#[from] git2::Error),

    /// The namespace was expected in a reference but was not found.
    #[error("missing namespace in reference")]
    MissingNamespace,

    /// The project does not have a default branch.
    #[error("missing default branch in project")]
    MissingDefaultBranch,

    /// Error related to tracking.
    #[error("tracking: {0}")]
    Tracking(#[from] librad::git::tracking::error::Tracked),

    /// Error relating to local identities.
    #[error(transparent)]
    LocalIdentity(#[from] lnk_identities::local::Error),

    /// The entity was not found.
    #[error("entity not found")]
    NotFound,

    /// No local head found and unable to resolve from delegates.
    #[error("could not resolve head: {0}")]
    NoHead(&'static str),

    /// An error occurred during an authentication process.
    #[error("could not authenticate: {0}")]
    Auth(&'static str),

    /// An error occurred while verifying the siwe message.
    #[error(transparent)]
    SiweVerification(#[from] siwe::VerificationError),

    /// An error occurred while parsing the siwe message.
    #[error(transparent)]
    SiweParse(#[from] siwe::ParseError),

    /// An error occurred with radicle identities.
    #[error(transparent)]
    Identities(#[from] librad::git::identities::Error),

    /// An error occurred with radicle surf.
    #[error(transparent)]
    Surf(#[from] radicle_surf::git::error::Error),

    /// An error occurred with radicle storage.
    #[error(transparent)]
    Storage(#[from] librad::git::storage::Error),

    /// An error occurred with the storage pool.
    #[error("{0}")]
    Pool(String),

    /// An error occurred with a project.
    #[error(transparent)]
    Project(#[from] radicle_common::project::Error),

    /// An error occurred with radicle storage.
    #[error("{0}: {1}")]
    Io(&'static str, std::io::Error),

    /// An error occurred with initializing read-only storage.
    #[error(transparent)]
    Init(#[from] librad::git::storage::read::error::Init),

    /// An error occurred with radicle source.
    #[error(transparent)]
    Source(#[from] radicle_source::error::Error),

    /// An error occurred with env variables.
    #[error(transparent)]
    Env(#[from] std::env::VarError),

    /// An error occurred during the identity resolving.
    #[error(transparent)]
    IdentityResolve(#[from] radicle_common::cobs::ResolveError),

    /// An error occurred with COB stores.
    #[error(transparent)]
    Cobs(#[from] radicle_common::cobs::Error),

    /// An async task was either cancelled or panic'ed.
    #[error(transparent)]
    TokioJoinError(#[from] tokio::task::JoinError),

    /// An anyhow error originated from radicle-common
    #[error("radicle-common: {0}")]
    Common(#[from] anyhow::Error),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            Error::NotFound => (StatusCode::NOT_FOUND, None),
            Error::NoHead(msg) => (StatusCode::NOT_FOUND, Some(msg.to_string())),
            Error::Auth(msg) => (StatusCode::BAD_REQUEST, Some(msg.to_string())),
            Error::SiweParse(msg) => (StatusCode::BAD_REQUEST, Some(msg.to_string())),
            Error::SiweVerification(msg) => (StatusCode::BAD_REQUEST, Some(msg.to_string())),
            Error::Git(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Some(e.message().to_owned()),
            ),
            _ => {
                tracing::error!("Error: {:?}", &self);

                (StatusCode::INTERNAL_SERVER_ERROR, None)
            }
        };

        let body = Json(json!({
            "error": msg.or_else(|| status.canonical_reason().map(|r| r.to_string())),
            "code": status.as_u16()
        }));

        (status, body).into_response()
    }
}
