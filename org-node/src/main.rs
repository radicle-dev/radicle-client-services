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

    /// node identity file path
    #[argh(option)]
    pub identity: PathBuf,

    /// org addresses to watch, ','-delimited (default: all)
    #[argh(option, from_str_fn(parse_orgs))]
    pub orgs: Option<Vec<node::OrgId>>,
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
            identity: other.identity,
            orgs: other.orgs.unwrap_or_default(),
        }
    }
}

fn parse_orgs(value: &str) -> Result<Vec<node::OrgId>, String> {
    if value.is_empty() {
        Ok(vec![])
    } else {
        Ok(value.split(',').map(|s| s.to_ascii_lowercase()).collect())
    }
}

fn main() {
    tracing_subscriber::fmt::init();

    let options = Options::from_env();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    node::run(rt, options.into()).unwrap();
}
