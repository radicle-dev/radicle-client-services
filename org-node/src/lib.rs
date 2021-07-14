/// # Org Node
///
/// The purpose of the org node is to listen for on-chain anchor events and
/// start replicating the associated radicle projects.
///
/// The org node can be configured to listen to any number of orgs, or *all*
/// orgs.
use radicle_daemon::Paths;
use thiserror::Error;

use std::fs::File;
use std::io;
use std::net;
use std::path::PathBuf;
use std::thread;
use std::time;

mod client;
mod query;
mod store;

pub use client::PeerId;

use client::{Client, Urn};

/// Default time to wait between polls of the subgraph.
/// Approximates Ethereum block time.
pub const DEFAULT_POLL_INTERVAL: time::Duration = time::Duration::from_secs(14);

/// Org identifier (Ethereum address).
pub type OrgId = String;

#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    pub store: PathBuf,
    pub identity: PathBuf,
    pub listen: net::SocketAddr,
    pub subgraph: String,
    pub poll_interval: time::Duration,
    pub orgs: Vec<OrgId>,
    pub timestamp: Option<u64>,
}

#[derive(serde::Deserialize, Debug)]
struct Project {
    #[serde(deserialize_with = "self::deserialize_timestamp")]
    timestamp: u64,
    anchor: Anchor,
    org: Org,
}

/// Error parsing a Radicle URN.
#[derive(Error, Debug)]
enum ParseUrnError {
    #[error("invalid hex string: {0}")]
    Invalid(String),
    #[error(transparent)]
    Int(#[from] std::num::ParseIntError),
    #[error(transparent)]
    Git(#[from] git2::Error),
}

impl Project {
    fn urn(&self) -> Result<Urn, ParseUrnError> {
        use std::convert::TryInto;

        let mut hex = self.anchor.object_id.as_str();

        if hex.starts_with("0x") {
            hex = &hex[2..];
        } else {
            return Err(ParseUrnError::Invalid(hex.to_owned()));
        }

        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<_>, _>>()?;

        // In Ethereum, the ID is stored as a `bytes32`.
        if bytes.len() != 32 {
            return Err(ParseUrnError::Invalid(hex.to_owned()));
        }
        // We only use the last 20 bytes for Git hashes (SHA-1).
        let bytes = &bytes[bytes.len() - 20..];
        let id = bytes.try_into()?;

        Ok(Urn { id, path: None })
    }
}

#[derive(serde::Deserialize, Debug)]
struct Anchor {
    #[serde(rename(deserialize = "objectId"))]
    object_id: String,
    multihash: String,
}

#[derive(serde::Deserialize, Debug)]
struct Org {
    id: OrgId,
}

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error(transparent)]
    Handle(#[from] client::handle::Error),
}

/// Run the Node.
pub fn run(rt: tokio::runtime::Runtime, options: Options) -> Result<(), Error> {
    let paths = Paths::from_root(options.root).unwrap();
    let identity = File::open(options.identity)?;
    let signer = client::Signer::new(identity)?;
    let client = Client::new(
        paths,
        signer,
        client::Config {
            listen: options.listen,
            ..client::Config::default()
        },
    );
    let mut handle = client.handle();
    let mut store = match store::Store::create(&options.store) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            tracing::info!("Found existing store {:?}", options.store);
            store::Store::open(&options.store)?
        }
        Err(err) => {
            return Err(err.into());
        }
        Ok(store) => {
            tracing::info!("Initializing new store {:?}", options.store);
            store
        }
    };

    if let Some(timestamp) = options.timestamp {
        store.state.timestamp = timestamp;
        store.write()?;
    }

    tracing::info!("Orgs = {:?}", options.orgs);
    tracing::info!("Timestamp = {}", store.state.timestamp);
    tracing::info!("Starting protocol client..");

    rt.spawn(client.run());

    loop {
        match query(&options.subgraph, store.state.timestamp, &options.orgs) {
            Ok(projects) => {
                tracing::info!("found {} project(s)", projects.len());

                for project in projects {
                    tracing::debug!("{:?}", project);

                    let urn = match project.urn() {
                        Ok(urn) => urn,
                        Err(err) => {
                            tracing::error!("Invalid URN for project: {}", err);
                            continue;
                        }
                    };

                    match futures::executor::block_on(handle.track_project(urn))? {
                        Ok(peer_id) => {
                            tracing::debug!("Project {:?} fetched from {}", project.urn(), peer_id);
                        }
                        Err(client::TrackProjectError::NotFound) => {
                            tracing::debug!("Project {:?} was not found", project.urn());
                        }
                    }

                    if project.timestamp > store.state.timestamp {
                        tracing::info!("Timestamp = {}", project.timestamp);

                        store.state.timestamp = project.timestamp;
                        store.write()?;
                    }
                }
            }
            Err(ureq::Error::Transport(err)) => {
                tracing::error!("query failed: {}", err);
            }
            Err(err) => {
                tracing::error!("{}", err);
            }
        }
        thread::sleep(options.poll_interval);
    }
}

fn query(subgraph: &str, timestamp: u64, orgs: &[OrgId]) -> Result<Vec<Project>, ureq::Error> {
    let query = if orgs.is_empty() {
        ureq::json!({
            "query": query::ALL_PROJECTS,
            "variables": { "timestamp": timestamp }
        })
    } else {
        ureq::json!({
            "query": query::ORG_PROJECTS,
            "variables": {
                "timestamp": timestamp,
                "orgs": orgs,
            }
        })
    };
    let response: serde_json::Value = ureq::post(subgraph).send_json(query)?.into_json()?;
    let response = &response["data"]["projects"];
    let anchors = serde_json::from_value(response.clone()).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to parse response: {}: {}", e, response),
        )
    })?;

    Ok(anchors)
}

fn deserialize_timestamp<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    use std::str::FromStr;

    let buf = String::deserialize(deserializer)?;

    u64::from_str(&buf).map_err(serde::de::Error::custom)
}
