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

    /// An error occured with radicle surf.
    #[error(transparent)]
    Surf(#[from] surf::git::error::Error),

    /// An error occured with radicle source.
    #[error(transparent)]
    Source(#[from] radicle_source::error::Error),
}

impl warp::reject::Reject for Error {}
