use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("'git' command not found")]
    GitNotFound,

    #[error("client request failed: {0}")]
    Handle(#[from] crate::client::handle::Error),

    #[error(transparent)]
    Channel(#[from] tokio::sync::mpsc::error::SendError<crate::client::Urn>),

    #[error(transparent)]
    FromHex(#[from] rustc_hex::FromHexError),

    #[error(transparent)]
    Query(#[from] Box<ureq::Error>),

    #[error("Type conversion failed")]
    ConversionError(#[from] std::num::TryFromIntError),

    #[cfg(feature = "influxdb-metrics")]
    #[error("Metrics reporting error")]
    OutfluxError(#[from] outflux::Error),
}
