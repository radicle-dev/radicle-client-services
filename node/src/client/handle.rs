use std::net;

use crossbeam_channel as chan;
use nakamoto_net::Reactor;
use thiserror::Error;

use crate::identity::ProjId;
use crate::protocol;
use crate::protocol::CommandError;

use serde::Deserialize;

/// An error resulting from a handle method.
#[derive(Error, Debug)]
pub enum Error {
    /// The command channel is no longer connected.
    #[error("command channel is not connected")]
    NotConnected,
    /// The command returned an error.
    #[error("command failed: {0}")]
    Command(#[from] CommandError),
    /// The operation timed out.
    #[error("the operation timed out")]
    Timeout,
    /// An I/O error occured.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<chan::RecvError> for Error {
    fn from(_: chan::RecvError) -> Self {
        Self::NotConnected
    }
}

impl From<chan::RecvTimeoutError> for Error {
    fn from(err: chan::RecvTimeoutError) -> Self {
        match err {
            chan::RecvTimeoutError::Timeout => Self::Timeout,
            chan::RecvTimeoutError::Disconnected => Self::NotConnected,
        }
    }
}

impl<T> From<chan::SendError<T>> for Error {
    fn from(_: chan::SendError<T>) -> Self {
        Self::NotConnected
    }
}

pub struct ClientAPI<R: Reactor> {
    pub(crate) commands: chan::Sender<protocol::Command>,
    pub(crate) waker: R::Waker,
    pub(crate) shutdown: chan::Sender<()>,
    pub(crate) listening: chan::Receiver<net::SocketAddr>,
}

impl<R: Reactor> traits::ClientAPI for ClientAPI<R> {
    /// Notify the client that a project has been updated.
    fn notify_update(&self, id: ProjId) -> Result<(), Error> {
        self.command(protocol::Command::AnnounceRefsUpdate(id))
    }

    /// Send a command to the command channel, and wake up the event loop.
    fn command(&self, cmd: protocol::Command) -> Result<(), Error> {
        self.commands.send(cmd)?;
        R::wake(&self.waker)?;

        Ok(())
    }

    /// Ask the client to shutdown.
    fn shutdown(self) -> Result<(), Error> {
        self.shutdown.send(())?;
        R::wake(&self.waker)?;

        Ok(())
    }

    /// Ask the client to shutdown.
    fn other_command(&self, _args: &OtherCommandArgs) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct OtherCommandArgs {
    id: ProjId,
    amount: u32,
}

pub mod traits {
    use super::*;

    pub trait ClientAPI {
        /// Notify the client that a project has been updated.
        fn notify_update(&self, id: ProjId) -> Result<(), Error>;
        /// Send a command to the command channel, and wake up the event loop.
        fn command(&self, cmd: protocol::Command) -> Result<(), Error>;
        /// Ask the client to shutdown.
        fn shutdown(self) -> Result<(), Error>;
        /// Ask the client to shutdown.
        fn other_command(&self, args: &OtherCommandArgs) -> Result<(), Error>;
    }
}
