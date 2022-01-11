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

    /// The project does not have delegations.
    #[error("missing delegations in project")]
    MissingDelegations,

    /// Error related to tracking.
    #[error("tracking: {0}")]
    Tracking(#[from] radicle_daemon::git::tracking::error::Tracked),

    /// Invalid branch name.
    #[error("invalid branch name")]
    BranchName,

    /// The entity was not found.
    #[error("entity not found")]
    NotFound,

    /// An error occured with radicle identities.
    #[error(transparent)]
    Identities(#[from] radicle_daemon::git::identities::Error),

    /// An error occured with radicle surf.
    #[error(transparent)]
    Surf(#[from] surf::git::error::Error),

    /// An error occured with radicle storage.
    #[error(transparent)]
    Storage(#[from] radicle_daemon::git::storage::Error),

    /// An error occured with initializing read-only storage.
    #[error(transparent)]
    Init(#[from] radicle_daemon::git::storage::read::error::Init),

    /// An error occured with radicle source.
    #[error(transparent)]
    Source(#[from] radicle_source::error::Error),
}

impl warp::reject::Reject for Error {}
