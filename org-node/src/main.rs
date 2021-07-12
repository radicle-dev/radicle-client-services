use std::net;
use std::path::PathBuf;
use std::time;

use radicle_org_node as node;

use argh::FromArgs;

/// Radicle Org Node.
#[derive(FromArgs)]
pub struct Options {
    /// listen on the following address for HTTP connections (default: 0.0.0.0:8888)
    #[argh(option, default = "std::net::SocketAddr::from(([0, 0, 0, 0], 8888))")]
    pub listen: net::SocketAddr,

    /// radicle root path, for key and git storage
    #[argh(option)]
    pub root: PathBuf,

    /// node state store path (default: store.json)
    #[argh(option, default = "PathBuf::from(\"store.json\")")]
    pub store: PathBuf,

    /// radicle orgs subgraph (url)
    #[argh(option)]
    pub subgraph: String,

    /// poll interval for subgraph updates (seconds)
    #[argh(option)]
    pub poll_interval: Option<u64>,
}

impl Options {
    pub fn from_env() -> Self {
        argh::from_env()
    }
}

impl From<Options> for node::Options {
    fn from(other: Options) -> Self {
        Self {
            root: other.root,
            store: other.store,
            listen: other.listen,
            subgraph: other.subgraph,
            poll_interval: other
                .poll_interval
                .map(time::Duration::from_secs)
                .unwrap_or(node::DEFAULT_POLL_INTERVAL),
        }
    }
}

fn main() {
    let options = Options::from_env();
    tracing_subscriber::fmt::init();
    node::run(options.into()).unwrap();
}
