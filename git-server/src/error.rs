#![allow(clippy::large_enum_variant)]

/// Errors that may occur when interacting with the radicle git server or git hooks.
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

    /// Failed certificate verification.
    #[error("failed certification verification")]
    FailedCertificateVerification,

    /// Unauthorized.
    #[error("unauthorized: {0}")]
    Unauthorized(&'static str),

    /// Namespace not found.
    #[error("namespace does not exist")]
    NamespaceNotFound,

    /// Reference not found.
    #[error("reference not found")]
    ReferenceNotFound,

    /// Radicle identity not found for project.
    #[error("radicle identity is not found for project")]
    RadicleIdentityNotFound,

    /// Environmental variable error.
    #[error("environmental variable error: {0}")]
    VarError(#[from] std::env::VarError),

    /// Git config parser error.
    #[error("git2 error: {0:?}")]
    Git2Error(#[from] git2::Error),

    /// Missing certification signer credentials.
    #[error("missing certificate signer credentials: {0:?}")]
    MissingCertificateSignerCredentials(String),

    /// Missing environmental variable.
    #[cfg(feature = "hooks")]
    #[error("missing environmental config variable: {0:?}")]
    EnvConfigError(#[from] envconfig::Error),

    /// Failed to parse byte data into string.
    #[error(transparent)]
    Utf8Error(#[from] std::str::Utf8Error),

    /// OpenPGP errors.
    #[cfg(feature = "hooks")]
    #[error(transparent)]
    PgpError(#[from] pgp::errors::Error),

    /// Librad profile error.
    #[error(transparent)]
    Profile(#[from] librad::profile::Error),

    /// Failed to connect to org-node unix socket.
    #[error("failed to connect to org-node unix socket")]
    UnixSocket,

    /// An error occured with initializing read-only storage.
    #[error(transparent)]
    Init(#[from] librad::git::storage::read::error::Init),

    /// An error occured with radicle identities.
    #[error(transparent)]
    Identities(#[from] librad::git::identities::Error),

    /// An error occured with a git storage pool.
    #[error(transparent)]
    Pool(#[from] librad::git::storage::pool::PoolError),

    /// Stored refs error.
    #[error(transparent)]
    Stored(#[from] librad::git::refs::stored::Error),
}

impl Error {
    pub fn status(&self) -> http::StatusCode {
        match self {
            Error::UnsupportedContentEncoding(_) => http::StatusCode::NOT_IMPLEMENTED,
            Error::ServiceUnavailable(_) => http::StatusCode::SERVICE_UNAVAILABLE,
            Error::Unauthorized(_) => http::StatusCode::UNAUTHORIZED,
            _ => http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl warp::reject::Reject for Error {}
