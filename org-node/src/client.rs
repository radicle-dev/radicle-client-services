use std::{net::SocketAddr, panic, time::Duration};

use futures::{future::FutureExt as _, select, stream::StreamExt as _};
use thiserror::Error;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use librad::{
    git::{identities, replication, storage::fetcher, tracking},
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
    #[error("error creating fetcher")]
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
                paths: self.paths,
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
        let peer = Peer::new(peer_config).unwrap();
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
                                    tracing::error!(err = ?e, "Accept error")
                                }
                            }
                            Err(e) => {
                                tracing::error!(err = ?e, "Bind error");
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

                       rt.spawn(async move {
                            if let Err(err) = Client::handle_request(r, &peer).await {
                                tracing::error!(err = ?err, "Request fulfilment failed");
                            }
                        });
                    }
                }
            }
        }
    }

    /// Handle user requests.
    async fn handle_request(request: Request, api: &Peer<Signer>) -> Result<(), Error> {
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
                let mut peers = api.providers(urn.clone(), timeout);

                // Attempt to track until we succeed.
                while let Some(peer) = peers.next().await {
                    if let Ok(tracked) = Client::track_project(api, &urn, &peer).await {
                        let response = if tracked { Some(peer.peer_id) } else { None };
                        return reply
                            .send(Ok(response))
                            .map_err(|_| Error::Reply("TrackProject".to_string()));
                    }
                }
                reply
                    .send(Err(TrackProjectError::NotFound))
                    .map_err(|_| Error::Reply("TrackProject".to_string()))
            }
        }
    }

    /// Attempt to track a project.
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
                    let fetcher = fetcher::PeerToPeer::new(urn.clone(), peer_id, addr_hints)
                        .build(storage)
                        .map_err(|e| Error::Fetcher(e.into()))??;

                    replication::replicate(storage, fetcher, cfg, None)?;

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
                    tracing::info!("Successfully tracked project {} from peer {}", urn, peer_id)
                }
            }
            Err(err) => {
                tracing::info!(
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
