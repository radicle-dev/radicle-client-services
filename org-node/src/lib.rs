//! # Org Node
//!
//! The purpose of the org node is to listen for on-chain anchor events and
//! start replicating the associated radicle projects.
//!
//! The org node can be configured to listen to any number of orgs, or *all*
//! orgs.
use anyhow::Context;
use ethers::abi::Address;
use ethers::prelude::*;
use ethers::providers::{Provider, Ws};

use outflux::Client as InfluxDBClient;
use radicle_daemon::Paths;
use thiserror::Error;

use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use std::collections::VecDeque;
use std::convert::TryInto;
use std::fs::File;
use std::io;
use std::net;
use std::path::PathBuf;

mod client;
mod error;
mod observability;
mod query;

pub use client::PeerId;
pub use client::Urn;
pub use error::Error;

use client::Client;

/// Org identifier (Ethereum address).
pub type OrgId = String;

pub struct Options {
    pub root: PathBuf,
    pub identity: PathBuf,
    pub bootstrap: Vec<(PeerId, net::SocketAddr)>,
    pub rpc_url: String,
    pub listen: net::SocketAddr,
    pub subgraph: String,
    pub orgs: Vec<OrgId>,
    pub urns: Vec<Urn>,
    pub timestamp: Option<u64>,
    pub influxdb_client: Option<InfluxDBClient>,
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

/// Run the Node.
pub fn run(rt: tokio::runtime::Runtime, options: Options) -> anyhow::Result<()> {
    let git_version = std::process::Command::new("git")
        .arg("version")
        .output()
        .map_err(|_| Error::GitNotFound)?
        .stdout;
    tracing::info!(target: "org-node", "{}", std::str::from_utf8(&git_version).unwrap().trim());

    let paths = Paths::from_root(options.root).unwrap();
    let identity_path = options.identity.clone();
    let identity = File::open(options.identity)
        .with_context(|| format!("unable to open {:?}", &identity_path))?;
    let signer = client::Signer::new(identity)
        .with_context(|| format!("unable to load identity {:?}", &identity_path))?;
    let peer_id = PeerId::from(signer.clone());
    let client = Client::new(
        paths,
        signer,
        client::Config {
            listen: options.listen,
            bootstrap: options.bootstrap.clone(),
            ..client::Config::default()
        },
    );
    let handle = client.handle();
    let addresses = options
        .orgs
        .iter()
        .map(|a| a.parse())
        .collect::<Result<Vec<_>, _>>()?;
    let timestamp = options.timestamp.unwrap_or_default();

    tracing::info!(target: "org-node", "Peer ID = {}", peer_id);
    tracing::info!(target: "org-node", "Bootstrap = {:?}", options.bootstrap);
    tracing::info!(target: "org-node", "Orgs = {:?}", options.orgs);
    tracing::info!(target: "org-node", "Timestamp = {}", timestamp);
    tracing::info!(target: "org-node", "Starting protocol client..");

    // Queue of projects to track.
    let (work, queue) = mpsc::channel(256);

    // Queue of events on orgs.
    let (update, events) = mpsc::channel(256);

    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Default::default();

    let client_handle_for_metrics = client.handle();
    let client_task = rt.spawn(client.run(rt.handle().clone()));
    tasks.push(client_task);

    let track_task = rt.spawn(track_projects(handle, queue));
    tasks.push(track_task);

    tracing::info!(target: "org-node", "Listening on {}...", options.listen);

    // First get up to speed with existing anchors, before we start listening for events.
    let projects = query(&options.subgraph, timestamp, &addresses).map_err(Box::new)?;
    rt.block_on(process_anchors(projects, &work))?;
    rt.block_on(process_urns(options.urns, &work))?;

    // Now launch the event subscriber and listen on events.
    let event_task = rt.spawn(subscribe_events(options.rpc_url, addresses, update));
    tasks.push(event_task);
    let query_task = rt.spawn(query_projects(timestamp, options.subgraph, events, work));
    tasks.push(query_task);

    if let Some(influxdb_client) = options.influxdb_client {
        let bucket = influxdb_client.make_bucket("radicle", "client-services")?;
        let metrics_reporter_task = rt.spawn(observability::report_metrics_periodically(
            bucket,
            client_handle_for_metrics,
            peer_id,
        ));
        tasks.push(metrics_reporter_task);
    }

    tasks.shrink_to_fit();

    if let (Err(err), _, _) = rt.block_on(futures::future::select_all(tasks)) {
        tracing::info!(target: "org-node", "Task failed: {}", err);
    }
    tracing::info!(target: "org-node", "Exiting..");

    Ok(())
}

async fn query_projects(
    timestamp: u64,
    subgraph: String,
    mut events: mpsc::Receiver<Log>,
    work: mpsc::Sender<Urn>,
) {
    while let Some(event) = events.recv().await {
        match query(&subgraph, timestamp, &[event.address]) {
            Ok(projects) => {
                if let Err(err) = process_anchors(projects, &work).await {
                    tracing::error!(target: "org-node", "Anchor processing failed: {}", err);
                    return;
                }
            }
            Err(ureq::Error::Transport(err)) => {
                tracing::error!(target: "org-node", "Query failed: {}", err);
            }
            Err(err) => {
                tracing::error!(target: "org-node", "{}", err);
            }
        }
    }
}

async fn process_urns(urns: Vec<Urn>, work: &mpsc::Sender<Urn>) -> Result<(), Error> {
    if urns.is_empty() {
        return Ok(());
    }
    tracing::info!(target: "org-node", "Processing {} URNs(s)", urns.len());

    for urn in urns {
        tracing::info!(target: "org-node", "Queueing {}", urn);
        work.send(urn).await?;
    }
    Ok(())
}

async fn process_anchors(projects: Vec<Project>, work: &mpsc::Sender<Urn>) -> Result<(), Error> {
    if projects.is_empty() {
        return Ok(());
    }
    tracing::info!(target: "org-node", "Found {} project(s)", projects.len());

    for project in projects {
        tracing::debug!(target: "org-node", "{:?}", project);

        let urn = match project.urn() {
            Ok(urn) => urn,
            Err(err) => {
                tracing::error!(target: "org-node", "Invalid URN for project: {}", err);
                continue;
            }
        };

        tracing::info!(target: "org-node", "Queueing {}", urn);
        work.send(urn).await?;
    }
    Ok(())
}

/// Get projects updated or created since the given timestamp, from the given orgs.
/// If no org is specified, gets projects from *all* orgs.
fn query(subgraph: &str, timestamp: u64, orgs: &[Address]) -> Result<Vec<Project>, ureq::Error> {
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

/// Subscribe to events emitted by the given org contracts.
async fn subscribe_events(url: String, addresses: Vec<Address>, update: mpsc::Sender<Log>) {
    let provider = match Provider::<Ws>::connect(url).await {
        Ok(provider) => provider,
        Err(err) => {
            tracing::error!(target: "org-node", "WebSocket connection failed, exiting task ({})", err);
            return;
        }
    };
    let filter = Filter::new()
        .address(ValueOrArray::Array(addresses))
        .event("Anchored(bytes32,uint32,bytes)");
    let mut stream = match provider.subscribe_logs(&filter).await {
        Ok(stream) => stream,
        Err(err) => {
            tracing::error!(target: "org-node", "Event subscribe failed, exiting task ({})", err);
            return;
        }
    };

    while let Some(event) = stream.next().await {
        tracing::info!(target: "org-node", "Event received from {:?}", event.address);

        match update.send(event).await {
            Ok(()) => {}
            Err(err) => {
                tracing::error!(target: "org-node", "Send event failed, exiting task ({})", err);
                return;
            }
        }
    }
}

/// Track projects sent via the queue.
///
/// This function only returns if the channels it uses to communicate with other
/// tasks are closed.
async fn track_projects(mut handle: client::Handle, queue: mpsc::Receiver<Urn>) {
    // URNs to track are added to the back of this queue, and taken from the front.
    let mut work = VecDeque::new();
    let mut queue = ReceiverStream::new(queue).fuse();

    loop {
        // Drain ascynchronous tracking queue, moving URNs to work queue.
        // This ensures that we aren't only retrying existing URNs that have timed out
        // and have been added back to the work queue.
        loop {
            futures::select! {
                result = queue.next() => {
                    match result {
                        Some(urn) => {
                            work.push_back(urn.clone());
                            tracing::debug!(target: "org-node", "{}: Added to the work queue ({})", urn, work.len());
                        }
                        None => {
                            tracing::error!(target: "org-node", "Tracking channel closed, exiting task");
                            return;
                        }
                    }
                }
                default => {
                    tracing::debug!(target: "org-node", "Channel is empty");
                    break;
                }
                complete => {
                    tracing::info!(target: "org-node", "Queue shutdown, exiting task");
                    return;
                }
            }
        }

        // If we have something to work on now, work on it, otherwise block on the
        // async tracking queue. We do this to avoid spin-looping, since the queue
        // is drained without blocking.
        let urn = if let Some(front) = work.pop_front() {
            front
        } else if let Some(urn) = queue.next().await {
            urn
        } else {
            // This only happens if the tracking queue was closed from another task.
            // In this case we expect the condition to be caught in the next iteration.
            continue;
        };
        tracing::info!(target: "org-node", "{}: Attempting to track.. (work={})", urn, work.len());

        // If we fail to track, re-add the URN to the back of the queue.
        match handle.track_project(urn.clone()).await {
            Ok(reply) => match reply {
                Ok(Some(peer_id)) => {
                    tracing::info!(target: "org-node", "{}: Fetched from {}", urn, peer_id);
                }
                Ok(None) => {
                    tracing::debug!(target: "org-node", "{}: Nothing to do", urn);
                }
                Err(client::TrackProjectError::NotFound) => {
                    tracing::info!(target: "org-node", "{}: Not found", urn);
                    work.push_back(urn);
                }
            },
            Err(client::handle::Error::Timeout(err)) => {
                tracing::info!(target: "org-node", "{}: Tracking timed out: {}", urn, err);
                work.push_back(urn);
            }
            Err(err) => {
                tracing::error!(target: "org-node", "Tracking handle failed, exiting task ({})", err);
                return;
            }
        }
    }
}
