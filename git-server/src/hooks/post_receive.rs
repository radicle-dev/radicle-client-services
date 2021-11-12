//! # POST-RECEIVE HOOK
//!
//! <https://git-scm.com/docs/githooks#post-receive>
//!
//! # Use by Radicle Git-Server
//!
//! The `post-receive` git hook sends a request to the org-node for signing references once a `git push` has successfully passed
//! `pre-receive` certification verification and authorization.
//!
//!
use std::io::{stdin, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use envconfig::Envconfig;
use git2::Oid;

use super::{types::ReceivePackEnv, CertSignerDetails};
use crate::error::Error;

/// Filename for named pipe / FIFO file.
pub const ORG_SOCKET_FILE: &str = "org-node.sock";

/// `PostReceive` provides access to the standard input values passed into the `post-receive`
/// git hook, as well as parses environmental variables that may be used to process the hook.
#[derive(Debug, Clone)]
pub struct PostReceive {
    // Old SHA1
    pub old: Oid,
    // New SHA1
    pub new: Oid,
    // refname relative to $GIT_DIR
    pub refname: String,
    // Environmental variables.
    pub env: ReceivePackEnv,
}

// use cert signer details default utility implementations.
impl CertSignerDetails for PostReceive {}

impl PostReceive {
    /// Instantiate from standard input.
    pub fn from_stdin() -> Result<Self, Error> {
        let mut buffer = String::new();
        stdin().read_to_string(&mut buffer)?;

        let input = buffer.split(' ').collect::<Vec<&str>>();

        // parse standard input variables.
        let old = Oid::from_str(input[0])?;
        let new = Oid::from_str(input[1])?;
        let refname = input[2].replace("\n", "");

        // initialize environmental values.
        let env = ReceivePackEnv::init_from_env()?;

        Ok(Self {
            old,
            new,
            refname,
            env,
        })
    }

    /// The main process used by `post-receive` hook.
    pub fn hook() -> Result<(), Error> {
        let post_receive = Self::from_stdin()?;

        // notify the org-node to update the signed refs.
        post_receive.notify_org_node()
    }

    pub fn notify_org_node(&self) -> Result<(), Error> {
        let path = std::env::temp_dir().join(ORG_SOCKET_FILE);
        match UnixStream::connect(path.clone()) {
            Ok(mut stream) => {
                stream.write_all(format!("{}\n", self.env.git_namespace).as_bytes())?;
            }
            Err(e) => {
                eprintln!("Error connecting to org socket ({:?}): {}", path, e);
                eprintln!("Please ensure org-node service is running.");
                return Err(Error::UnixSocket);
            }
        }

        Ok(())
    }
}
