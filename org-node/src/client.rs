use std::path::Path;
use std::{io, net::SocketAddr, panic, time::Duration};

use either::Either;
use futures::{future::FutureExt as _, select, stream::StreamExt as _};
use thiserror::Error;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use librad::{
    git::{identities, refs, replication, storage, storage::fetcher, tracking},
    net::{
        discovery::{self, Discovery as _},
        peer::{self, Peer},
        protocol::{self, membership, PeerInfo},
        Network,
    },
    paths::Paths,
};

use crate::client::handle::Request;

pub use handle::{Handle, TrackProjectError};
pub use librad::git::identities::Urn;
pub use librad::PeerId;
pub use signer::Signer;

pub mod handle;
pub mod signer;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("error creating fetcher: {0}")]
    Fetcher(#[from] Box<dyn std::error::Error + Send + Sync + 'static>),

    #[error("replication of {urn} from {remote_peer} already in-flight")]
    Concurrent { urn: Urn, remote_peer: PeerId },

    #[error(transparent)]
    Init(#[from] peer::error::Init),

    #[error(transparent)]
    Storage(#[from] peer::error::Storage),

    #[error(transparent)]
    Tracking(#[from] tracking::Error),

    #[error(transparent)]
    Identities(#[from] Box<identities::Error>),

    #[error(transparent)]
    Replication(#[from] replication::Error),

    #[error("failed to set project head: {0}")]
    SetHead(Box<dyn std::error::Error + Send + Sync + 'static>),

    #[error("error signing refs: {0}")]
    SignRefs(#[from] refs::stored::Error),

    #[error("invalid project: {0}: {1}")]
    Project(Urn, &'static str),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("sending reply failed for {0}")]
    Reply(String),
}

impl From<fetcher::Info> for Error {
    fn from(
        fetcher::Info {
            urn, remote_peer, ..
        }: fetcher::Info,
    ) -> Self {
        Self::Concurrent { urn, remote_peer }
    }
}

/// Client configuration.
pub struct Config {
    /// List of bootstrap peers
    pub bootstrap: Vec<(PeerId, SocketAddr)>,
    /// Knobs to tune timeouts and internal queues.
    pub limits: Limits,
    /// Listen address.
    pub listen: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bootstrap: vec![],
            limits: Default::default(),
            listen: ([0, 0, 0, 0], 0).into(),
        }
    }
}

/// Protocol limits.
pub struct Limits {
    /// Amount of in-flight requests.
    pub request_queue_size: usize,
    /// Duration after which a request is considered failed.
    pub request_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            request_queue_size: 64,
            request_timeout: Duration::from_secs(60),
        }
    }
}

/// A head reference.
pub struct Head {
    remote: String,
    branch: String,
}

/// Client instance.
pub struct Client {
    /// Paths.
    paths: Paths,
    /// Signer.
    signer: Signer,
    /// Config that was passed during Client construction.
    config: Config,
    /// Sender end of user requests.
    handle: mpsc::Sender<Request>,
    /// Receiver end of user requests.
    requests: mpsc::Receiver<Request>,
}

impl Client {
    /// Create a new client.
    pub fn new(paths: Paths, signer: Signer, config: Config) -> Self {
        let (handle, requests) = mpsc::channel::<Request>(config.limits.request_queue_size);

        Self {
            paths,
            signer,
            config,
            handle,
            requests,
        }
    }

    /// Create a new handle.
    pub fn handle(&self) -> Handle {
        Handle::new(self.handle.clone(), self.config.limits.request_timeout)
    }

    /// Run the client. This function runs indefinitely until a fatal error occurs.
    pub async fn run(self, rt: tokio::runtime::Handle) {
        let storage = peer::config::Storage {
            protocol: peer::config::ProtocolStorage {
                fetch_slot_wait_timeout: Default::default(),
                pool_size: 4,
            },
            user: peer::config::UserStorage { pool_size: 4 },
        };
        let membership = membership::Params::default();

        let peer_config = peer::Config {
            signer: self.signer,
            protocol: protocol::Config {
                paths: self.paths.clone(),
                listen_addr: self.config.listen,
                advertised_addrs: None, // TODO: Should we use this?
                membership,
                network: Network::Main,
                replication: replication::Config::default(),
                fetch: Default::default(),
                rate_limits: Default::default(),
            },
            storage,
        };
        let peer = Peer::new(peer_config).expect("signing key must match peer id");
        let mut requests = ReceiverStream::new(self.requests).fuse();

        // Spawn the peer thread.
        let mut protocol = rt
            .spawn({
                let peer = peer.clone();
                let disco = discovery::Static::resolve(self.config.bootstrap.clone()).unwrap();

                async move {
                    loop {
                        match peer.bind().await {
                            Ok(bound) => {
                                let (_kill, run) = bound.accept(disco.clone().discover());

                                if let Err(e) = run.await {
                                    tracing::error!(target: "org-node", err = ?e, "Accept error")
                                }
                            }
                            Err(e) => {
                                tracing::error!(target: "org-node", err = ?e, "Bind error");
                                tokio::time::sleep(Duration::from_secs(2)).await
                            }
                        }
                    }
                }
            })
            .fuse();

        loop {
            select! {
                p = protocol => match p {
                    Err(e) if e.is_panic() => panic::resume_unwind(e.into_panic()),
                    _ => break
                },
                request = requests.next() => {
                    if let Some(r) = request {
                        let peer = peer.clone();
                        let paths = self.paths.clone();

                       rt.spawn(async move {
                            if let Err(err) = Client::handle_request(r, &peer, &paths).await {
                                tracing::error!(target: "org-node", "Request fulfilment failed: {}", err);
                            }
                        });
                    }
                }
            }
        }
    }

    /// Handle user requests.
    async fn handle_request(
        request: Request,
        api: &Peer<Signer>,
        paths: &Paths,
    ) -> Result<(), Error> {
        match request {
            Request::GetMembership(reply) => {
                let info = api.membership().await;
                reply
                    .send(info)
                    .map_err(|_| Error::Reply("GetMembership".to_string()))
            }
            Request::GetPeers(reply) => {
                let peers = api.connected_peers().await;
                reply
                    .send(peers)
                    .map_err(|_| Error::Reply("GetPeers".to_string()))
            }
            Request::TrackProject(urn, timeout, reply) => {
                let project = Client::get_project_head(&urn, api).await?;

                // Don't track projects that already exist locally.
                if let Some(Head { remote, branch }) = project {
                    tracing::debug!(target: "org-node", "Project {} exists, (re-)setting head", urn);

                    // Set the project head if it isn't already set.
                    match Client::set_head(&urn, &remote, &branch, paths) {
                        Err(Error::SetHead(err)) => {
                            tracing::error!(target: "org-node", "Error setting head: {}", err);
                        }
                        Err(err) => return Err(err),
                        Ok(()) => {}
                    }
                    Client::sign_refs(urn, api).await?;

                    return reply
                        .send(Ok(None))
                        .map_err(|_| Error::Reply("TrackProject".to_string()));
                }

                // Get potential peers and attempt to track until we succeed.
                let mut peers = api.providers(urn.clone(), timeout);

                while let Some(peer) = peers.next().await {
                    match Client::track_project(api, &urn, &peer).await {
                        Err(err) => {
                            tracing::error!(target: "org-node", "Error tracking {}: {}", urn, err);
                        }
                        Ok(tracked) => {
                            let response = if tracked {
                                let Head { remote, branch } = Client::get_project_head(&urn, api)
                                    .await?
                                    .expect("a project that was just tracked should exist");
                                // Tracking doesn't automatically set the repository head, we have to do it
                                // manually. We set the head to the default branch of the project
                                // maintainer.
                                match Client::set_head(&urn, &remote, &branch, paths) {
                                    Err(Error::SetHead(err)) => {
                                        tracing::error!(target: "org-node", "Error setting head: {}", err);
                                    }
                                    Err(err) => return Err(err),
                                    Ok(()) => {}
                                }
                                Client::sign_refs(urn, api).await?;

                                Some(peer.peer_id)
                            } else {
                                tracing::debug!(target: "org-node", "Tracking relationship for project {} already exists", urn);

                                None
                            };

                            return reply
                                .send(Ok(response))
                                .map_err(|_| Error::Reply("TrackProject".to_string()));
                        }
                    }
                }
                reply
                    .send(Err(TrackProjectError::NotFound))
                    .map_err(|_| Error::Reply("TrackProject".to_string()))
            }
        }
    }

    /// Get the project head, or return nothing if it isn't found.
    async fn get_project_head(urn: &Urn, api: &Peer<Signer>) -> Result<Option<Head>, Error> {
        api.using_storage({
            let urn = urn.clone();

            move |storage| match identities::project::get(&storage, &urn) {
                Ok(Some(project)) => {
                    let maintainer = project
                        .delegations()
                        .iter()
                        .flat_map(|either| match either {
                            Either::Left(pk) => Either::Left(std::iter::once(PeerId::from(*pk))),
                            Either::Right(indirect) => Either::Right(
                                indirect.delegations().iter().map(|pk| PeerId::from(*pk)),
                            ),
                        })
                        .next()
                        .ok_or_else(|| Error::Project(urn.clone(), "project has no maintainer"))?;
                    let default_branch =
                        project.subject().default_branch.clone().ok_or_else(|| {
                            Error::Project(urn.clone(), "project has no default branch")
                        })?;

                    Ok(Some(Head {
                        remote: maintainer.default_encoding(),
                        branch: default_branch.to_string(),
                    }))
                }
                Ok(None) => Ok(None),
                Err(err) => Err(Error::from(Box::new(err))),
            }
        })
        .await?
        .map_err(Error::from)
    }

    /// Sign updated project refs.
    async fn sign_refs(urn: Urn, api: &Peer<Signer>) -> Result<refs::Updated, Error> {
        let updated = api
            .using_storage({
                let urn = urn.clone();
                move |s| refs::Refs::update(s, &urn)
            })
            .await?
            .map_err(Error::SignRefs)?;

        match &updated {
            refs::Updated::Updated { refs, at } => {
                tracing::debug!(target: "org-node", "Signed refs for {} updated: heads={:?} at={}", urn, refs.heads, at);
            }
            refs::Updated::Unchanged { refs, at } => {
                tracing::debug!(target: "org-node", "Signed refs for {} unchanged: heads={:?} at={}", urn, refs.heads, at);
            }
            refs::Updated::ConcurrentlyModified => {
                tracing::warn!(target: "org-node", "Signed refs for {} concurrently modified", urn);
            }
        }

        Ok(updated)
    }

    /// Set the 'HEAD' of a project.
    ///
    /// Creates the necessary refs so that a `git clone` may succeed and checkout the correct
    /// branch.
    fn set_head(urn: &Urn, maintainer: &str, branch: &str, paths: &Paths) -> Result<(), Error> {
        let namespace = urn.encode_id();
        let repository = git2::Repository::open_bare(paths.git_dir())
            .map_err(|e| Error::SetHead(Box::new(e)))?;

        // eg. refs/namespaces/<namespace>/refs/remotes/<peer>/heads/master
        let namespace_path = Path::new("refs").join("namespaces").join(&namespace);
        let branch_ref = namespace_path
            .join("refs")
            .join("remotes")
            .join(maintainer)
            .join("heads")
            .join(branch);

        tracing::debug!(target: "org-node", "Setting repository head for {} to {:?}", urn, branch_ref);

        if !paths.git_dir().join(&branch_ref).exists() {
            return Err(Error::SetHead(Box::new(io::Error::new(
                io::ErrorKind::NotFound,
                format!("path {:?} does not exist", paths.git_dir().join(branch_ref)),
            ))));
        }
        let branch_ref = branch_ref.to_string_lossy();

        let reference = repository
            .find_reference(&branch_ref)
            .map_err(|e| Error::SetHead(Box::new(e)))?;

        let oid = reference.target().expect("reference target must exist");
        let head = namespace_path.join("HEAD");
        let head = head.to_str().unwrap();

        let local_branch_ref = namespace_path.join("refs").join("heads").join(&branch);
        let local_branch_ref = local_branch_ref.to_str().expect("ref is valid unicode");

        tracing::debug!(target: "org-node", "Setting ref {:?} -> {:?}", &local_branch_ref, oid);
        repository
            .reference(local_branch_ref, oid, true, "set-local-branch (org-node)")
            .map_err(|e| Error::SetHead(Box::new(e)))?;

        tracing::debug!(target: "org-node", "Setting ref {:?} -> {:?}", &branch_ref, oid);
        repository
            .reference(&branch_ref, oid, true, "set-remote-branch (org-node)")
            .map_err(|e| Error::SetHead(Box::new(e)))?;

        tracing::debug!(target: "org-node", "Setting ref {:?} -> {:?}", &head, local_branch_ref);
        repository
            .reference_symbolic(head, local_branch_ref, true, "set-head (org-node)")
            .map_err(|e| Error::SetHead(Box::new(e)))?;

        Ok(())
    }

    /// Attempt to fetch a project from a peer.
    fn fetch_project(
        urn: &Urn,
        peer_id: PeerId,
        addr_hints: Vec<SocketAddr>,
        storage: &storage::Storage,
        cfg: replication::Config,
    ) -> Result<(), Error> {
        let fetcher = fetcher::PeerToPeer::new(urn.clone(), peer_id, addr_hints)
            .build(storage)
            .map_err(|e| Error::Fetcher(e.into()))??;

        let result = replication::replicate(storage, fetcher, cfg, None)?;
        tracing::debug!(target: "org-node", "Replication of {} succeeded: {:?}", urn, result);

        Ok(())
    }

    /// Attempt to track a project from a peer.
    async fn track_project(
        api: &Peer<Signer>,
        urn: &Urn,
        peer_info: &PeerInfo<std::net::SocketAddr>,
    ) -> Result<bool, Error> {
        let peer_id = peer_info.peer_id;
        let addr_hints = peer_info.seen_addrs.iter().copied().collect::<Vec<_>>();

        let result = {
            let cfg = api.protocol_config().replication;
            let urn = urn.clone();

            api.using_storage(move |storage| {
                if tracking::track(storage, &urn, peer_id)? {
                    tracing::debug!(target: "org-node", "Tracking relationship for project {} and peer {} created", urn, peer_id);
                    tracing::debug!(target: "org-node", "Fetching from {} @ {:?}", peer_id, addr_hints);

                    Client::fetch_project(&urn, peer_id, addr_hints, storage, cfg)?;

                    Ok::<_, Error>(true)
                } else {
                    Ok(false)
                }
            })
            .await?
        };

        match &result {
            Ok(tracked) => {
                if *tracked {
                    tracing::info!(
                        target: "org-node",
                        "Successfully tracked project {} from peer {}", urn, peer_id
                    )
                }
            }
            Err(err) => {
                tracing::info!(
                    target: "org-node",
                    "Error tracking project {} from peer {}: {}",
                    urn,
                    peer_id,
                    err
                );
            }
        }
        result
    }
}
