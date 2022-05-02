#![allow(clippy::large_enum_variant)]
use radicle_source::surf;

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

    /// The entity was not found.
    #[error("entity not found")]
    NotFound,

    /// Failed to parse byte data into string.
    #[error("not valid utf8")]
    Utf8Error,

    /// No local head found and unable to resolve from delegates.
    #[error("could not resolve head: {0}")]
    NoHead(&'static str),

    /// An error occured with radicle identities.
    #[error(transparent)]
    Identities(#[from] librad::git::identities::Error),

    /// An error occured with radicle surf.
    #[error(transparent)]
    Surf(#[from] surf::git::error::Error),

    /// An error occured with radicle storage.
    #[error(transparent)]
    Storage(#[from] librad::git::storage::Error),

    /// An error occured with radicle storage.
    #[error("{0}: {1}")]
    Io(&'static str, std::io::Error),

    /// An error occured with initializing read-only storage.
    #[error(transparent)]
    Init(#[from] librad::git::storage::read::error::Init),

    /// An error occured with radicle source.
    #[error(transparent)]
    Source(#[from] radicle_source::error::Error),
}

impl warp::reject::Reject for Error {}
