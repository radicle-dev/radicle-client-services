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
    Surf(#[from] surf::git::error::Error),

    /// An error occurred with radicle storage.
    #[error(transparent)]
    Storage(#[from] librad::git::storage::Error),

    /// An error occurred with the storage pool.
    #[error("{0}")]
    Pool(String),

    /// An error occurred with a project.
    #[error(transparent)]
    Project(#[from] radicle_common::project::Error),

    /// An error occurred with the issues storage.
    #[error(transparent)]
    Issues(#[from] radicle_common::cobs::issue::Error),

    /// An error occurred with the patches storage.
    #[error(transparent)]
    Patches(#[from] radicle_common::cobs::patch::Error),

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

    /// An error occurred with COBs stores
    #[error(transparent)]
    Store(#[from] radicle_common::cobs::StoreError),

    /// An anyhow error originated from radicle-common
    #[error("radicle-common: {0}")]
    Common(#[from] anyhow::Error),
}

//impl warp::reject::Reject for Error {}
