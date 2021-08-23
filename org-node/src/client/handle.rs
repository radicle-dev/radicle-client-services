use std::time::Duration;

use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot},
    time,
};

use librad::{git::identities::Urn, net::protocol::event::downstream::MembershipInfo, PeerId};

/// An error returned by the [`Handle`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Failed to receive response from backend.
    #[error("receive failed")]
    ReceiveFailed(#[from] oneshot::error::RecvError),

    /// Failed to send request to backend.
    #[error("send failed {0}")]
    SendFailed(mpsc::error::TrySendError<Request>),

    /// Request timed out awaiting response.
    #[error("request timed out")]
    Timeout(#[from] time::error::Elapsed),
}

/// Handle used to interact with the seed node.
pub struct Handle {
    channel: mpsc::Sender<Request>,
    timeout: Duration,
}

impl Handle {
    /// Create a new handle.
    pub(super) fn new(channel: mpsc::Sender<Request>, timeout: Duration) -> Self {
        Self { channel, timeout }
    }

    /// Get peer membership information.
    #[allow(dead_code)]
    pub async fn get_membership(&mut self) -> Result<MembershipInfo, Error> {
        let (tx, rx) = oneshot::channel();
        self.channel
            .try_send(Request::GetMembership(tx))
            .map_err(Error::SendFailed)?;

        time::timeout(self.timeout, rx).await?.map_err(Error::from)
    }

    /// Get currently connected peers.
    #[allow(dead_code)]
    pub async fn get_peers(&mut self) -> Result<Vec<PeerId>, Error> {
        let (tx, rx) = oneshot::channel();
        self.channel
            .try_send(Request::GetPeers(tx))
            .map_err(Error::SendFailed)?;

        time::timeout(self.timeout, rx).await?.map_err(Error::from)
    }

    /// Track project.
    pub async fn track_project(
        &mut self,
        urn: Urn,
    ) -> Result<Result<Option<PeerId>, TrackProjectError>, Error> {
        let (tx, rx) = oneshot::channel();
        self.channel
            .try_send(Request::TrackProject(urn, self.timeout / 2, tx))
            .map_err(Error::SendFailed)?;

        time::timeout(self.timeout, rx).await?.map_err(Error::from)
    }
}

/// User request to the seed node.
#[derive(Debug)]
pub enum Request {
    /// Get current membership info.
    GetMembership(oneshot::Sender<MembershipInfo>),
    /// Get connected peers.
    GetPeers(oneshot::Sender<Vec<PeerId>>),
    /// Track a project
    TrackProject(
        Urn,
        std::time::Duration,
        oneshot::Sender<Result<Option<PeerId>, TrackProjectError>>,
    ),
}

/// Error when using the [`Request::TrackProject`] request.
#[derive(Error, Debug)]
pub enum TrackProjectError {
    /// The project was not found after querying all connected peers.
    #[error("project not found")]
    NotFound,
}
