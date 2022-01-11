use std::path::Path;
use std::{io, net::SocketAddr, panic, time::Duration};

use either::Either;
use futures::{future::FutureExt as _, select, stream::StreamExt as _};
use thiserror::Error;

use librad::{
    git,
    git::tracking::policy::Track,
    git::{identities, refs, storage, storage::fetcher, tracking},
    net::{
        discovery::{self, Discovery as _},
        peer::{self, event::upstream::Gossip, Peer, PeerInfo, ProtocolEvent},
        protocol::{
            self,
            broadcast::PutResult::Applied,
            broadcast::PutResult::Uninteresting,
            gossip::{Payload, Rev},
            membership,
        },
        replication, Network,
    },
    paths::Paths,
};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;

use crate::client::handle::Request;
use crate::webserver::WsEvent;

pub use handle::{Handle, TrackProjectError};
pub use librad::{git::Urn, PeerId};
pub use shared::signer::Signer;

pub mod handle;

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

    #[error("track: {0}")]
    Track(#[from] librad::git::tracking::error::Track),

    #[error(transparent)]
    Identities(#[from] Box<identities::Error>),

    #[error(transparent)]
    Replication(#[from] git::replication::Error),

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

    #[error("project head was not found")]
    ProjectHeadNotFound,

    #[error("project peer failed to be tracked and fetched: {0}")]
    TrackPeer(String),

    #[error("tokio channel receiver error: {0}")]
    Recv(#[from] tokio::sync::oneshot::error::RecvError),

    #[error("Join handle error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),
    #[error(transparent)]
    RefStorage(#[from] Box<radicle_daemon::git::refs::stored::Error>),
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
    /// Track a list of peers
    pub peers: Vec<PeerId>,
    /// Allow tracking unknown peers
    pub allow_unknown_peers: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bootstrap: vec![],
            limits: Default::default(),
            listen: ([0, 0, 0, 0], 0).into(),
            peers: vec![],
            allow_unknown_peers: false,
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

/// Project peer contains details used to track the peer for an interested project Urn.
/// this struct is primarily used by `TrackPeerTransmitter`
struct TrackPeer {
    api: Peer<Signer>,
    peer_info: PeerInfo<std::net::SocketAddr>,
    paths: Paths,
    urn: Urn,
}

/// Project peer struct with a acknowledgement transmitter.
/// This struct is primarily used by `protocol_listener` and TrackProject request handler.
type TrackPeerTransmitter = (TrackPeer, oneshot::Sender<Result<Option<PeerId>, Error>>);

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
    pub async fn run(self, rt: tokio::runtime::Handle, ws_tx: UnboundedSender<WsEvent>) {
        let storage = peer::config::Storage {
            protocol: peer::config::ProtocolStorage { pool_size: 4 },
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
                rate_limits: Default::default(),
            },
            storage,
        };
        let peer = Peer::new(peer_config).expect("signing key must match peer id");
        let mut requests = ReceiverStream::new(self.requests).fuse();

        // Establish Peer Tracking Channel for protocol listener and request handler.
        let (peer_tx, peer_rx) =
            mpsc::channel::<TrackPeerTransmitter>(self.config.limits.request_queue_size);

        // listen for track peers notifications.
        let mut track_peers = rt.spawn(Client::track_peers(peer_rx)).fuse();

        // Listen for and process protocol events.
        let mut protocol_listener = rt
            .spawn(Client::protocol_listener(
                peer_tx.clone(),
                ws_tx.clone(),
                peer.clone(),
                self.paths.clone(),
                self.config.peers.clone(),
                self.config.allow_unknown_peers,
            ))
            .fuse();

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
                    Ok(_event) => {
                        // TODO: Call protocol handle event, and remove protocol_listener above.
                    },
                    _ => break
                },
                _ = track_peers => tracing::error!(target: "org-node", "Peer tracker failed unexpectedly"),
                _ = protocol_listener => tracing::error!(target: "org-node", "Protocol listener failed unexpectedly"),
                request = requests.next() => {
                    if let Some(r) = request {
                        tracing::debug!(target: "org-node", "Incoming Request: {:?}", r);
                        let peer = peer.clone();
                        let paths = self.paths.clone();
                        let tx = peer_tx.clone();

                       rt.spawn(async move {
                            if let Err(err) = Client::handle_request(r, &peer, &paths, tx).await {
                                tracing::error!(target: "org-node", "Request fulfillment failed: {}", err);
                            }
                        });
                    }
                }
            }
        }
    }

    /// Track peers that are found by the node that are interested in shared projects;
    /// This process is spawned within `Client::run` and is run indefinitely.
    async fn track_peers(mut peer_rx: mpsc::Receiver<TrackPeerTransmitter>) {
        tracing::info!(target: "org-node", "Spawning Track Peers Listener");

        while let Some((
            TrackPeer {
                api,
                urn,
                peer_info,
                paths,
            },
            ack,
        )) = peer_rx.recv().await
        {
            match Client::track_project(&api, &urn, &peer_info).await {
                Err(err) => {
                    tracing::error!(target: "org-node", "Error tracking {}: {}", urn, err);
                    ack.send(Err(err)).ok();
                }
                Ok(tracked) => {
                    let response = if tracked {
                        tracing::debug!(target: "org-node", "Tracking relationship for project {} and peer {} created", urn, peer_info.peer_id);

                        Some(peer_info.peer_id)
                    } else {
                        tracing::debug!(target: "org-node", "Tracking relationship for project {} already exists", urn);

                        None
                    };

                    let peer_id = peer_info.peer_id;
                    let seen_addrs = peer_info.seen_addrs.to_vec();
                    let cfg = git::replication::Config::default();

                    tracing::info!(target: "org-node", "Fetching tracked project {} for peer {}", urn, peer_id);

                    match api
                        .using_storage({
                            let urn = urn.clone();

                            move |s| Client::fetch_project(&urn, peer_id, seen_addrs, s, cfg)
                        })
                        .await
                    {
                        // Check if the response is an error and return early if so.
                        Ok(Err(err)) => {
                            tracing::error!(target: "org-node", "Failed to replicate {} from {}: {}", urn, peer_id, err);

                            ack.send(Err(err)).ok();
                            continue;
                        }
                        Err(e) => {
                            ack.send(Err(Error::Storage(e))).ok();
                            continue;
                        }
                        Ok(_) => {}
                    }

                    match Client::get_project_head(&urn, &api).await {
                        Ok(Some(Head { remote, branch })) => {
                            if let Err(err) = Client::set_head(&urn, &remote, &branch, &paths) {
                                tracing::error!(target: "org-node", "Error setting head: {}", err);
                                ack.send(Err(err)).ok();
                                continue;
                            }

                            if let Err(err) = Client::sign_refs(urn, &api).await {
                                tracing::error!(target: "org-node", "Error signing refs: {}", err);
                                ack.send(Err(err)).ok();
                                continue;
                            }

                            // Acknowledge success;
                            ack.send(Ok(response)).ok();
                        }
                        Ok(None) => {
                            tracing::error!(target: "org-node", "Project head not found! Cannot set head for peer");
                            ack.send(Err(Error::ProjectHeadNotFound)).ok();
                        }
                        Err(err) => {
                            tracing::error!(target: "org-node", "Error getting project head: {}", err);
                            ack.send(Err(err)).ok();
                        }
                    }
                }
            }
        }
    }

    async fn track_peer_handler(
        peer_tx: mpsc::Sender<TrackPeerTransmitter>,
        track_peer: TrackPeer,
    ) -> Result<Option<PeerId>, Error> {
        // create receiver for Project Peer acknowledgement.
        let (tx, rx) = oneshot::channel::<Result<Option<PeerId>, Error>>();

        // spawn rx listener to wait for oneshot response from peer_tx.
        let rx_res = tokio::spawn(rx);

        // notify track peers process to handle tracking and fetching.
        if let Err(e) = peer_tx.send((track_peer, tx)).await {
            tracing::error!(target: "org-node", "Failed to send track peer request: {}", e);
            return Err(Error::TrackPeer(e.to_string()));
        }

        // return the nested result of the acknowledgement.
        rx_res.await??
    }

    /// This process listens for incoming protocol events and processes accordingly.
    /// It is spawned within `Client::run` and is run indefinitely.
    async fn protocol_listener(
        peer_tx: mpsc::Sender<TrackPeerTransmitter>,
        ws_tx: UnboundedSender<WsEvent>,
        api: Peer<Signer>,
        paths: Paths,
        known_peers: Vec<PeerId>,
        allow_unknown_peers: bool,
    ) {
        tracing::info!(target: "org-node", "Spawning Protocol Event Listener");

        api.subscribe()
            .for_each(|incoming| async {
                match incoming {
                    Err(e) => {
                        panic!(
                            "Protocol event stream is unavailable, received error: {}",
                            e
                        );
                    }
                    Ok(ProtocolEvent::Gossip(gossip)) => {
                        Self::handle_gossip(
                            *gossip,
                            &known_peers,
                            allow_unknown_peers,
                            &peer_tx,
                            &ws_tx,
                            &api,
                            &paths,
                        )
                        .await;
                    }
                    Ok(_) => {
                        // Non-gossip messages are ignored.
                    }
                }
            })
            .await;
    }

    async fn handle_gossip(
        gossip: librad::net::peer::event::upstream::Gossip<
            std::net::SocketAddr,
            librad::net::protocol::gossip::Payload,
        >,
        known_peers: &[PeerId],
        allow_unknown_peers: bool,
        peer_tx: &mpsc::Sender<TrackPeerTransmitter>,
        ws_tx: &UnboundedSender<WsEvent>,
        api: &Peer<Signer>,
        paths: &Paths,
    ) {
        match gossip {
            // Check if the event payload rev matches the current project head, if not
            // we want to set the head.
            Gossip::Put {
                payload: Payload { urn, .. },
                result:
                    Applied(Payload {
                        rev: Some(Rev::Git(oid)),
                        origin: Some(origin),
                        ..
                    }),
                ..
            } => {
                tracing::info!(target: "org-node", "Applied revision update for urn {}, oid {}", urn, oid);
                if let Ok(Some(Head { branch, remote })) = Client::get_project_head(&urn, api).await
                {
                    // need to assert that the rev was signed by the maintainer of the project.
                    // TODO: Ideally, we need an ability to update the head when a specified peer publishes a new revision.
                    // This is needed in a `multi-maintainer` project, but currently all projects are singly maintained.
                    if origin.default_encoding() == remote {
                        if let Err(e) = Client::set_head(&urn, &remote, &branch, paths) {
                            tracing::error!(target: "org-node", "Error setting head: {}", e);
                        }
                        // Broadcast applied gossip event to websocket clients
                        if let Err(e) = ws_tx.send(WsEvent::UpdatedRef {
                            oid,
                            urn,
                            peer: origin,
                        }) {
                            tracing::error!(target: "org-node", "Failed to send update refs notification to web socket clients: {}", e);
                        }
                    }
                }
            }
            Gossip::Put {
                payload:
                    Payload {
                        urn,
                        rev: Some(Rev::Git(oid)),
                        origin: Some(peer_id),
                    },
                provider: peer,
                result: Uninteresting,
            } => {
                // Exit early if the peer ID is unknown and `allow_unknown_peers` is false
                if !known_peers.contains(&peer.peer_id) && !allow_unknown_peers {
                    tracing::debug!(target: "org-node", "Ignoring tracking peer: {}", peer.peer_id);
                    return;
                }

                // Send notification to track peers process;
                if let Err(e) = Client::track_peer_handler(
                    peer_tx.clone(),
                    TrackPeer {
                        api: api.clone(),
                        urn: urn.clone(),
                        peer_info: peer,
                        paths: paths.clone(),
                    },
                )
                .await
                {
                    tracing::error!(target: "org-node", "Error tracking project peer: {}", e);
                }

                if let Err(e) = ws_tx.send(WsEvent::UpdatedRef {
                    oid,
                    urn,
                    peer: peer_id,
                }) {
                    tracing::error!(target: "org-node", "Failed to send update refs notification to web socket clients: {}", e);
                }
            }
            _ => {}
        }
    }

    /// Handle user requests.
    async fn handle_request(
        request: Request,
        api: &Peer<Signer>,
        paths: &Paths,
        peer_tx: mpsc::Sender<TrackPeerTransmitter>,
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
                        Ok(_) => {}
                    }

                    Client::sign_refs(urn, api).await?;

                    return reply
                        .send(Ok(None))
                        .map_err(|_| Error::Reply("TrackProject".to_string()));
                }

                // ... Looks like the project wasn't replicated yet ...

                // Get potential peers and attempt to replicate until we succeed.
                let mut peers = api.providers(urn.clone(), timeout);

                while let Some(peer) = peers.next().await {
                    // Send notification to track peers process;
                    let result = Client::track_peer_handler(
                        peer_tx.clone(),
                        TrackPeer {
                            api: api.clone(),
                            urn: urn.clone(),
                            peer_info: peer,
                            paths: paths.clone(),
                        },
                    )
                    .await;

                    // Handle the track peer response
                    let response = match result {
                        Ok(response) => response,
                        Err(e) => {
                            tracing::error!(target: "org-node", "Error tracking peers: {}", e);
                            continue;
                        }
                    };

                    return reply
                        .send(Ok(response))
                        .map_err(|_| Error::Reply("TrackProject".to_string()));
                }

                reply
                    .send(Err(TrackProjectError::NotFound))
                    .map_err(|_| Error::Reply("TrackProject".to_string()))
            }
            Request::UpdateRefs(urn, rx) => {
                tracing::info!(target: "org-node", "updating refs for urn: {:?}", urn);

                // sign the updated ref;
                Client::sign_refs(urn.clone(), api).await?;

                let project = Client::get_project_head(&urn, api).await?;

                // Return the project's default branch;
                let Head { branch, .. } =
                    project.ok_or_else(|| Error::Project(urn.clone(), "failed to find project"))?;

                let namespace = urn.encode_id();
                let namespace_path = Path::new("refs").join("namespaces").join(&namespace);

                // Set symbolic "HEAD" to local branch reference;
                let head = namespace_path.join("HEAD");
                let local_branch_ref = namespace_path.join("refs").join("heads").join(branch);

                tracing::debug!(target: "org-node", "Setting ref {:?} -> {:?}", &head, local_branch_ref);

                let repository = git2::Repository::open_bare(paths.git_dir())
                    .map_err(|e| Error::SetHead(Box::new(e)))?;

                // Set symbolic reference;
                let reference = repository
                    .reference_symbolic(
                        head.to_str().unwrap_or_default(),
                        local_branch_ref.to_str().unwrap_or_default(),
                        true,
                        "set-head (org-node)",
                    )
                    .map_err(|e| Error::SetHead(Box::new(e)))?;

                let oid = reference.target().expect("reference target must exist");

                // Announce the updated refs to the network.
                if let Err(e) = api.announce(Payload {
                    urn,
                    rev: Some(Rev::Git(oid)),
                    origin: Some(api.peer_id()),
                }) {
                    tracing::error!(target: "org-node", "Error announcing refs: {:?}", e);
                }

                // return acknowledgement
                rx.send(oid)
                    .map_err(|_| Error::Reply("UpdateRefs".to_string()))
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
                tracing::debug!(target: "org-node", "Signed refs for {} updated: heads={:?} at={}", urn, refs.heads().collect::<Vec<_>>(), at);
            }
            refs::Updated::Unchanged { refs, at } => {
                tracing::debug!(target: "org-node", "Signed refs for {} unchanged: heads={:?} at={}", urn, refs.heads().collect::<Vec<_>>(), at);
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
    fn set_head(urn: &Urn, maintainer: &str, branch: &str, paths: &Paths) -> Result<Rev, Error> {
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

        Ok(Rev::Git(oid))
    }

    /// Attempt to fetch a project from a peer.
    fn fetch_project(
        urn: &Urn,
        peer_id: PeerId,
        seen_addrs: Vec<SocketAddr>,
        storage: &storage::Storage,
        cfg: git::replication::Config,
    ) -> Result<(), Error> {
        let fetchers = git::storage::fetcher::Fetchers::default();
        let fetcher = fetcher::PeerToPeer::new(urn.clone(), peer_id, seen_addrs)
            .build(storage, &fetchers)
            .map_err(|e| Error::Fetcher(e.into()))??;

        let result = git::replication::replicate(storage, fetcher, cfg, None)?;
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
        let result = {
            let urn = urn.clone();
            let cfg = tracking::config::Config::default();

            api.using_storage(move |storage| {
                match tracking::track(storage, &urn, Some(peer_id), cfg, Track::MustNotExist)? {
                    // If we don't have an error, our policy went through, otherwise it failed
                    // because the relationship already existed.
                    Ok(_) => Ok::<_, Error>(true),
                    Err(_) => Ok(false),
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
