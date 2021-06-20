use std::net;
use std::path::PathBuf;

use radicle_http_api as api;

use argh::FromArgs;

/// Radicle HTTP API.
#[derive(FromArgs)]
pub struct Options {
    /// listen on the following address for HTTP connections (default: 0.0.0.0:8888)
    #[argh(option, default = "std::net::SocketAddr::from(([0, 0, 0, 0], 8888))")]
    pub listen: net::SocketAddr,

    /// radicle root path, for key and git storage
    #[argh(option)]
    pub root: PathBuf,

    /// syntax highlight theme
    #[argh(option, default = r#"String::from("base16-ocean.dark")"#)]
    pub theme: String,
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
            theme: other.theme,
        }
    }
}

#[tokio::main]
async fn main() {
    let options = Options::from_env();
    tracing_subscriber::fmt::init();
    api::run(options.into()).await;
}
