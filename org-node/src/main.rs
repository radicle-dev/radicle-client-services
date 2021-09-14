use std::io::Write;
use std::net;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::{fs, fs::File, os::unix::fs::PermissionsExt};

use node::PeerId;
use radicle_org_node as node;

use argh::FromArgs;
use librad::SecretKey;

use shared::LogFmt;

/// Radicle Org Node.
#[derive(FromArgs)]
pub struct Options {
    /// listen on the following address for peer messages (default: 0.0.0.0:8776)
    #[argh(option, default = "std::net::SocketAddr::from(([0, 0, 0, 0], 8776))")]
    pub listen: net::SocketAddr,

    /// radicle root path, for key and git storage
    #[argh(option)]
    pub root: PathBuf,

    /// radicle orgs subgraph (url)
    #[argh(option)]
    pub subgraph: String,

    /// JSON-RPC WebSocket URL of Ethereum node (eg. ws://localhost:8545)
    #[argh(option)]
    pub rpc_url: String,

    /// node identity file path
    #[argh(option)]
    pub identity: PathBuf,

    /// start syncing from a given unix timestamp (seconds)
    #[argh(option)]
    pub timestamp: Option<u64>,

    /// list of bootstrap peers, eg.
    /// 'f00...@seed1.example.com:12345,bad...@seed2.example.com:12345'
    #[argh(option, from_str_fn(parse_bootstrap))]
    pub bootstrap: Option<Vec<(PeerId, net::SocketAddr)>>,

    /// org addresses to watch, ','-delimited (default: all)
    #[argh(option, from_str_fn(parse_orgs))]
    pub orgs: Option<Vec<node::OrgId>>,

    /// either "plain" or "gcp" (gcp available only when compiled-in)
    #[argh(option, default = "LogFmt::Plain")]
    pub log_format: LogFmt,
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
            listen: other.listen,
            subgraph: other.subgraph,
            rpc_url: other.rpc_url,
            identity: other.identity,
            timestamp: other.timestamp,
            bootstrap: other.bootstrap.unwrap_or_default(),
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

fn parse_bootstrap(value: &str) -> Result<Vec<(PeerId, net::SocketAddr)>, String> {
    use std::net::ToSocketAddrs;

    let mut peers = Vec::new();

    for parts in value
        .split(',')
        .map(|entry| entry.splitn(2, '@').collect::<Vec<_>>())
    {
        peers.push((
            PeerId::from_str(parts[0]).map_err(|e| e.to_string())?,
            parts[1]
                .to_socket_addrs()
                .map(|mut a| a.next())
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Could not resolve peer address".to_owned())?,
        ));
    }
    Ok(peers)
}

fn generate_identity(path: &Path) -> anyhow::Result<()> {
    let mut file = File::create(path)?;
    let metadata = file.metadata()?;
    let mut permissions = metadata.permissions();

    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)?;

    let secret_key = SecretKey::new();
    file.write_all(secret_key.as_ref())?;

    Ok(())
}

fn main() {
    let options = Options::from_env();

    shared::init_logger(options.log_format);
    tracing::info!("version {}-{}", env!("CARGO_PKG_VERSION"), env!("GIT_HEAD"));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    if !options.identity.exists() {
        if let Err(e) = generate_identity(&options.identity) {
            tracing::error!(target: "org-node", "Fatal: error creating identity: {:#}", e);
            std::process::exit(2);
        }
        tracing::info!(target: "org-node", "Identity file generated: {:?}", options.identity);
    }

    if let Err(e) = node::run(rt, options.into()) {
        tracing::error!(target: "org-node", "Fatal: {:#}", e);
        std::process::exit(1);
    }
}
