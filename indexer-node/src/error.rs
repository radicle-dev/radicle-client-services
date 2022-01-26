/// Errors that may occur when interacting with [`librad::net::peer::Peer`].
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum Error {
    /// The entity was not found.
    #[error("entity not found")]
    NotFound,

    /// An error occured while parsing URL.
    #[error(transparent)]
    ParseError(#[from] url::ParseError),
}

impl warp::reject::Reject for Error {}
