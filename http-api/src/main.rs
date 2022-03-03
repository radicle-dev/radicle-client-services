use std::net;
use std::path::PathBuf;

use radicle_http_api as api;

use argh::FromArgs;

use shared::LogFmt;

/// Radicle HTTP API.
#[derive(FromArgs)]
pub struct Options {
    /// listen on the following address for HTTP connections (default: 0.0.0.0:8777)
    #[argh(option, default = "std::net::SocketAddr::from(([0, 0, 0, 0], 8777))")]
    pub listen: net::SocketAddr,

    /// radicle root path, for key and git storage
    #[argh(option)]
    pub root: Option<PathBuf>,

    /// TLS certificate path
    #[argh(option)]
    pub tls_cert: Option<PathBuf>,

    /// TLS key path
    #[argh(option)]
    pub tls_key: Option<PathBuf>,

    /// syntax highlight theme
    #[argh(option, default = r#"String::from("base16-ocean.dark")"#)]
    pub theme: String,

    /// either "plain" or "gcp" (gcp available only when compiled-in)
    #[argh(option, default = "LogFmt::Plain")]
    pub log_format: LogFmt,
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
            tls_cert: other.tls_cert,
            tls_key: other.tls_key,
            listen: other.listen,
            theme: other.theme,
        }
    }
}

#[tokio::main]
async fn main() {
    let options = Options::from_env();

    shared::init_logger(options.log_format);
    tracing::info!("version {}-{}", env!("CARGO_PKG_VERSION"), env!("GIT_HEAD"));

    api::run(options.into()).await;
}
