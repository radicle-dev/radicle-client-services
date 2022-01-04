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
use std::io::prelude::*;
use std::io::{stdin, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use envconfig::Envconfig;
use git2::Oid;
use librad::git::Urn;
use librad::paths::Paths;

use super::{types::ReceivePackEnv, CertSignerDetails};
use crate::error::Error;

/// Filename for named pipe / FIFO file.
pub const ORG_SOCKET_FILE: &str = "org-node.sock";

/// `PostReceive` provides access to the standard input values passed into the `post-receive`
/// git hook, as well as parses environmental variables that may be used to process the hook.
#[derive(Debug, Clone)]
pub struct PostReceive {
    /// Project URN being pushed.
    pub urn: Urn,
    /// Radicle paths.
    pub paths: Paths,
    /// Ref updates.
    pub updates: Vec<(String, Oid, Oid)>,
    // Environmental variables.
    pub env: ReceivePackEnv,
}

// use cert signer details default utility implementations.
impl CertSignerDetails for PostReceive {}

impl PostReceive {
    /// Instantiate from standard input.
    pub fn from_stdin() -> Result<Self, Error> {
        let mut updates = Vec::new();

        for line in stdin().lock().lines() {
            let line = line?;
            let input = line.split(' ').collect::<Vec<&str>>();

            let old = Oid::from_str(input[0])?;
            let new = Oid::from_str(input[1])?;
            let refname = input[2].to_owned();

            updates.push((refname, old, new));
        }

        // initialize environmental values.
        let env = ReceivePackEnv::init_from_env()?;
        let urn = Urn::try_from_id(&env.git_namespace).map_err(|_| Error::InvalidId)?;
        let paths = Paths::from_root(env.git_project_root.clone())?;

        Ok(Self {
            urn,
            paths,
            updates,
            env,
        })
    }

    /// The main process used by `post-receive` hook.
    pub fn hook() -> Result<(), Error> {
        let post_receive = Self::from_stdin()?;

        post_receive.update_refs()?;

        Ok(())
    }

    pub fn update_refs(&self) -> Result<(), Error> {
        // If there is no default branch, it means we're pushing a personal identity.
        // In that case there is nothing to do.
        if let Some(default_branch) = &self.env.default_branch {
            let suffix = format!("heads/{}", default_branch);

            for (refname, _, _) in self.updates.iter() {
                // For now, we only create a ref for the default branch.
                if refname.ends_with(&suffix) {
                    self.set_head(refname.as_str(), default_branch)?;
                    break;
                }
            }
        }

        Ok(())
    }

    /// Set the 'HEAD' of a project.
    ///
    /// Creates the necessary refs so that a `git clone` may succeed and checkout the correct
    /// branch.
    fn set_head(&self, branch_ref: &str, branch: &str) -> Result<git2::Oid, git2::Error> {
        let urn = &self.urn;
        let paths = &self.paths;

        let namespace = urn.encode_id();
        let repository = git2::Repository::open_bare(paths.git_dir())?;

        println!("Setting repository head for {} to {}", urn, branch_ref);

        // eg. refs/namespaces/<namespace>/refs/remotes/<peer>/heads/master
        let namespace_path = Path::new("refs").join("namespaces").join(&namespace);
        let branch_ref = namespace_path.join(branch_ref);

        let branch_ref = branch_ref.to_string_lossy();
        let reference = repository.find_reference(&branch_ref)?;

        let oid = reference.target().expect("reference target must exist");
        let head = namespace_path.join("HEAD");
        let head = head.to_str().unwrap();

        let local_branch_ref = namespace_path.join("refs").join("heads").join(&branch);
        let local_branch_ref = local_branch_ref.to_str().expect("ref is valid unicode");

        println!("Setting ref {:?} -> {:?}", &local_branch_ref, oid);
        repository.reference(local_branch_ref, oid, true, "set-local-branch (radicle)")?;

        println!("Setting ref {:?} -> {:?}", &head, local_branch_ref);
        repository.reference_symbolic(head, local_branch_ref, true, "set-head (radicle)")?;

        Ok(oid)
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
