/// # Org Node
///
/// The purpose of the org node is to listen for on-chain anchor events and
/// start replicating the associated radicle projects.
///
/// The org node can be configured to listen to any number of orgs, or *all*
/// orgs.
use radicle_daemon::Paths;
use thiserror::Error;

use tokio::sync::mpsc;

use std::collections::VecDeque;
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
    pub bootstrap: Vec<(PeerId, net::SocketAddr)>,
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

    #[error("client request failed: {0}")]
    Handle(#[from] client::handle::Error),

    #[error(transparent)]
    Channel(#[from] mpsc::error::SendError<Urn>),
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
            bootstrap: options.bootstrap,
            ..client::Config::default()
        },
    );
    let handle = client.handle();
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

    tracing::info!(target: "org-node", "Orgs = {:?}", options.orgs);
    tracing::info!(target: "org-node", "Timestamp = {}", store.state.timestamp);
    tracing::info!(target: "org-node", "Starting protocol client..");

    // Queue of projects to track.
    let (work, queue) = mpsc::channel(256);

    rt.spawn(client.run(rt.handle().clone()));
    rt.spawn(track_projects(handle, queue));

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

                    tracing::info!(target: "org-node", "Queueing {}", urn);
                    work.blocking_send(urn)?;

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

/// Get projects updated or created since the given timestamp, from the given orgs.
/// If no org is specified, gets projects from *all* orgs.
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

/// Track projects sent via the queue.
///
/// This function only returns if the channels it uses to communicate with other
/// tasks are closed.
async fn track_projects(mut handle: client::Handle, mut queue: mpsc::Receiver<Urn>) {
    // URNs to track are added to the back of this queue, and taken from the front.
    let mut work = VecDeque::new();

    loop {
        // Drain ascynchronous tracking queue, moving URNs to work queue.
        // This ensures that we aren't only retrying existing URNs that have timed out
        // and have been added back to the work queue.
        loop {
            tokio::select! {
                result = queue.recv() => {
                    match result {
                        Some(urn) => work.push_back(urn),
                        None => {
                            tracing::warn!(target: "org-node", "Tracking channel closed, exiting task");
                            return;
                        }
                    }
                }
                else => {
                    break;
                }
            }
        }

        // If we have something to work on now, work on it, otherwise block on the
        // async tracking queue. We do this to avoid spin-looping, since the queue
        // is drained without blocking.
        let urn = if let Some(front) = work.pop_front() {
            front
        } else if let Some(urn) = queue.recv().await {
            urn
        } else {
            // This only happens if the tracking queue was closed from another task.
            // In this case we expect the condition to be caught in the next iteration.
            continue;
        };

        // If we fail to track, re-add the URN to the back of the queue.
        match handle.track_project(urn.clone()).await {
            Ok(reply) => match reply {
                Ok(peer_id) => {
                    tracing::info!(target: "org-node", "Project {} fetched from {}", urn, peer_id);
                }
                Err(client::TrackProjectError::NotFound) => {
                    tracing::info!(target: "org-node", "Project {} was not found", urn);
                    work.push_back(urn);
                }
            },
            Err(client::handle::Error::Timeout(err)) => {
                tracing::info!(target: "org-node", "Project {} tracking timed out: {}", urn, err);
                work.push_back(urn);
            }
            Err(err) => {
                tracing::warn!(target: "org-node", "Tracking handle failed, exiting task ({})", err);
                return;
            }
        }
    }
}
