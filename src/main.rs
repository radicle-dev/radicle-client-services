use std::net;
use std::path::PathBuf;

use radicle_daemon::PeerId;
use radicle_http_api as api;

use argh::FromArgs;

/// Radicle HTTP API.
#[derive(FromArgs)]
pub struct Options {
    /// listen on the following address for HTTP connections (default: 0.0.0.0:8888)
    #[argh(option, default = "std::net::SocketAddr::from(([0, 0, 0, 0], 8888))")]
    pub listen: net::SocketAddr,

    /// log level (default: info)
    #[argh(option, default = "tracing::Level::INFO")]
    pub log: tracing::Level,

    /// radicle root path, for key and git storage
    #[argh(option)]
    pub root: PathBuf,

    /// peer/device identifier (a.k.a Device ID)
    #[argh(option)]
    pub peer_id: PeerId,
}

impl Options {
    pub fn from_env() -> Self {
        argh::from_env()
    }
}

impl From<Options> for api::Options {
    fn from(other: Options) -> Self {
        Self {
            root: other.root,
            listen: other.listen,
            peer_id: other.peer_id,
        }
    }
}

#[tokio::main]
async fn main() {
    let options = Options::from_env();
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(options.log.into()),
        )
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("setting tracing subscriber should succeed");

    api::run(options.into()).await;
}
