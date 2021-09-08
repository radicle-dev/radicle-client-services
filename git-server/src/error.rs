#![allow(clippy::large_enum_variant)]

/// Errors that may occur when interacting with [`librad::net::peer::Peer`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// I/O error.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// The content encoding is not supported.
    #[error("content encoding '{0}' not supported")]
    UnsupportedContentEncoding(&'static str),

    /// The service is not available.
    #[error("service '{0}' not available")]
    ServiceUnavailable(&'static str),

    /// HTTP error.
    #[error("HTTP error: {0}")]
    Http(#[from] http::Error),

    /// Git backend error.
    #[error("backend error")]
    Backend,

    /// Invalid authorization.
    #[error("invalid authorization")]
    InvalidAuthorization,

    /// Unauthorized.
    #[error("unauthorized")]
    Unauthorized,
}

impl Error {
    pub fn status(&self) -> http::StatusCode {
        match self {
            Error::UnsupportedContentEncoding(_) => http::StatusCode::NOT_IMPLEMENTED,
            Error::ServiceUnavailable(_) => http::StatusCode::SERVICE_UNAVAILABLE,
            Error::InvalidAuthorization => http::StatusCode::BAD_REQUEST,
            _ => http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl warp::reject::Reject for Error {}
