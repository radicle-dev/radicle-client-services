/// # Org Node
///
/// The purpose of the org node is to listen for on-chain anchor events and
/// start replicating the associated radicle projects.
///
/// The org node can be configured to listen to any number of orgs, or *all*
/// orgs.
use radicle_daemon::Paths;

use std::io;
use std::net;
use std::path::PathBuf;
use std::thread;
use std::time;

mod query;
mod store;

/// Default time to wait between polls of the subgraph.
/// Approximates Ethereum block time.
pub const DEFAULT_POLL_INTERVAL: time::Duration = time::Duration::from_secs(14);

/// Org identifier (Ethereum address).
pub type OrgId = String;

#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    pub store: PathBuf,
    pub listen: net::SocketAddr,
    pub subgraph: String,
    pub poll_interval: time::Duration,
    pub orgs: Vec<OrgId>,
}

#[derive(serde::Deserialize, Debug)]
struct Project {
    #[serde(deserialize_with = "self::deserialize_timestamp")]
    timestamp: u64,
    anchor: Anchor,
    org: Org,
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
pub fn run(options: Options) -> Result<(), io::Error> {
    let _paths = Paths::from_root(options.root).unwrap();
    let mut store = match store::Store::create(&options.store) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            tracing::info!("Found existing store {:?}", options.store);
            store::Store::open(&options.store)?
        }
        Err(err) => {
            return Err(err);
        }
        Ok(store) => {
            tracing::info!("Initializing new store {:?}", options.store);
            store
        }
    };
    tracing::info!("orgs = {:?}", options.orgs);
    tracing::info!("timestamp = {}", store.state.timestamp);

    loop {
        match query(&options.subgraph, store.state.timestamp, &options.orgs) {
            Ok(projects) => {
                tracing::info!("found {} project(s)", projects.len());

                for project in projects {
                    tracing::debug!("{:?}", project);

                    if project.timestamp > store.state.timestamp {
                        tracing::info!("timestamp = {}", project.timestamp);

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
