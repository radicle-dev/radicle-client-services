use std::net;
use std::path::PathBuf;

use radicle_git_server as server;

use argh::FromArgs;

use shared::LogFmt;

/// Radicle Git Server.
#[derive(FromArgs)]
pub struct Options {
    /// listen on the following address for HTTP connections (default: 0.0.0.0:8778)
    #[argh(option, default = "std::net::SocketAddr::from(([0, 0, 0, 0], 8778))")]
    pub listen: net::SocketAddr,

    /// radicle root path, for key and git storage
    #[argh(option)]
    pub root: PathBuf,

    /// TLS certificate path
    #[argh(option)]
    pub tls_cert: Option<PathBuf>,

    /// TLS key path
    #[argh(option)]
    pub tls_key: Option<PathBuf>,

    /// either "plain" or "gcp" (gcp available only when compiled-in)
    #[argh(option, default = "LogFmt::Plain")]
    pub log_format: LogFmt,
}

impl Options {
    pub fn from_env() -> Self {
        argh::from_env()
    }
}

impl From<Options> for server::Options {
    fn from(other: Options) -> Self {
        Self {
            root: other.root,
            tls_cert: other.tls_cert,
            tls_key: other.tls_key,
            listen: other.listen,
        }
    }
}

#[tokio::main]
async fn main() {
    let options = Options::from_env();

    shared::init_logger(options.log_format);

    server::run(options.into()).await;
}
