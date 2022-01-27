//! # POST-RECEIVE HOOK
//!
//! <https://git-scm.com/docs/githooks#post-receive>
//!
//! # Use by Radicle Git-Server
//!
//! The `post-receive` git hook sends a request to the org-node for signing references once a `git push` has successfully passed
//! `pre-receive` certification verification and authorization.
//!
use std::io::prelude::*;
use std::io::{stdin, Write};
use std::os::unix::net::UnixStream;
use std::str;
use std::str::FromStr;

use envconfig::Envconfig;
use git2::{Oid, Repository};
use librad::git;
use librad::git::storage::read::ReadOnlyStorage as _;
use librad::git::tracking;
use librad::git::Urn;
use librad::identities;
use librad::paths::Paths;
use librad::PeerId;

use super::storage::Storage;
use super::{types::ReceivePackEnv, CertSignerDetails};
use crate::error::Error;

/// Filename for named pipe / FIFO file.
pub const ORG_SOCKET_FILE: &str = "org-node.sock";
pub const RAD_ID_REF: &str = "rad/id";

/// `PostReceive` provides access to the standard input values passed into the `post-receive`
/// git hook, as well as parses environmental variables that may be used to process the hook.
#[derive(Debug, Clone)]
pub struct PostReceive {
    /// Project URN being pushed.
    urn: Urn,
    /// Project delegates.
    delegates: Vec<PeerId>,
    /// Radicle paths.
    paths: Paths,
    /// SSH key fingerprint of pusher.
    key_fingerprint: String,
    /// Ref updates.
    updates: Vec<(String, Oid, Oid)>,
    // Environmental variables.
    env: ReceivePackEnv,
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

        let env = ReceivePackEnv::init_from_env()?;
        let urn = Urn::try_from_id(&env.git_namespace).map_err(|_| Error::InvalidId)?;
        let paths = Paths::from_root(env.git_project_root.clone())?;
        let delegates = if let Some(keys) = &env.delegates {
            keys.split(',')
                .map(PeerId::from_str)
                .collect::<Result<_, _>>()
                .map_err(|_| Error::InvalidPeerId)?
        } else {
            Vec::new()
        };
        let key_fingerprint = env
            .cert_key
            .as_ref()
            .ok_or(Error::Unauthorized("push certificate is not available"))?
            .to_owned();

        Ok(Self {
            urn,
            delegates,
            key_fingerprint,
            paths,
            updates,
            env,
        })
    }

    /// The main process used by `post-receive` hook.
    pub fn hook() -> Result<(), Error> {
        println!("Running post-receive hook...");

        let mut post_receive = Self::from_stdin()?;
        let repo = Repository::open_bare(&post_receive.env.git_dir)?;
        let identity_exists = repo
            .find_reference(&post_receive.namespace_ref(RAD_ID_REF))
            .is_ok();

        if identity_exists {
            println!("Pushing to existing identity...");

            post_receive.update_refs(&repo)?;

            if let Some((refname, _, _)) = post_receive.updates.first() {
                let (peer_id, _) = crate::parse_ref(refname)?;

                post_receive.track_identity(Some(peer_id))?;
                post_receive.receive_hook()?;
            }
        } else {
            println!("Pushing new identity...");

            post_receive.initialize_identity(&repo)?;
            post_receive.track_identity(None)?;
        }

        Ok(())
    }

    pub fn update_refs(&self, repo: &Repository) -> Result<(), Error> {
        // If there is no default branch, it means we're pushing a personal identity.
        // In that case there is nothing to do.
        if let Some(default_branch) = &self.env.default_branch {
            let suffix = format!("heads/{}", default_branch);

            for (refname, _, _) in self.updates.iter() {
                let (peer_id, rest) = crate::parse_ref(refname)?;

                println!("Updating ref for {}: {}", peer_id, rest);

                // Only delegates can update refs.
                if !self.delegates.contains(&peer_id) {
                    continue;
                }
                // For now, we only create a ref for the default branch.
                if rest != suffix {
                    continue;
                }
                println!("Ref update to default branch detected, setting HEAD...");

                // TODO: This should only update when a quorum is reached between delegates.
                // For a single delegate, we can just always allow it.
                if self.delegates.len() == 1 {
                    self.set_head(refname.as_str(), default_branch, repo)?;
                } else {
                    println!("Cannot set head for multi-delegate project: not supported.");
                }
                // TODO
                //
                // For non-default-branch refs, we can add them as:
                //
                // `refs/remotes/cloudhead@<peer-id>/<branch>`
            }
        }

        Ok(())
    }

    /// Set the 'HEAD' of a project.
    ///
    /// Creates the necessary refs so that a `git clone` may succeed and checkout the correct
    /// branch.
    fn set_head(
        &self,
        branch_ref: &str,
        branch: &str,
        repo: &Repository,
    ) -> Result<git2::Oid, git2::Error> {
        let urn = &self.urn;
        let namespace = urn.encode_id();

        println!("Setting repository head for {} to {}.", urn, branch_ref);

        // eg. refs/namespaces/<namespace>
        let namespace_path = format!("refs/namespaces/{}", namespace);
        // eg. refs/namespaces/<namespace>/refs/remotes/<peer>/heads/master
        let branch_ref = format!("{}/{}", namespace_path, branch_ref);
        let reference = repo.find_reference(&branch_ref)?;
        let oid = reference.target().expect("reference target must exist");

        // eg. refs/namespaces/<namespace>/HEAD
        let head_ref = format!("{}/HEAD", namespace_path);
        // eg. refs/namespaces/<namespace>/refs/heads/master
        let local_branch_ref = &format!("{}/refs/heads/{}", namespace_path, branch);

        println!("Setting ref {:?} -> {:?}", &local_branch_ref, oid);
        repo.reference(local_branch_ref, oid, true, "set-local-branch (radicle)")?;

        println!("Setting ref {:?} -> {:?}", head_ref, local_branch_ref);
        repo.reference_symbolic(&head_ref, local_branch_ref, true, "set-head (radicle)")?;

        Ok(oid)
    }

    fn initialize_identity(&mut self, repo: &Repository) -> Result<(), Error> {
        eprintln!("Verifying identity...");

        if let Some((refname, from, to)) = self.updates.pop() {
            // When initializing a new identity, we only expect a single ref update.
            if !self.updates.is_empty() {
                return Err(Error::Unauthorized(
                    "unexpected ref updates for new identity",
                ));
            }
            // We shouldn't be updating anything, we should be creating a new ref.
            if !from.is_zero() {
                return Err(Error::Unauthorized("identity old ref should be zero"));
            }
            // We only authorize updates that first write to the key-specific staging area.
            if !refname.ends_with(RAD_ID_REF) {
                return Err(Error::Unauthorized("identity must be initialized first"));
            }

            let storage = librad::git::storage::ReadOnly::open(&self.paths)?;
            let lookup = |urn| {
                let refname = git::types::Reference::rad_id(git::types::Namespace::from(urn));
                storage.reference_oid(&refname).map(|oid| oid.into())
            };

            let identity = storage
                .identities::<identities::SomeIdentity>()
                .some_identity(to)
                .map_err(|_| Error::NamespaceNotFound)?;

            // Make sure that the identity we're pushing matches the namespace
            // we're pushing to.
            if identity.urn() != self.urn {
                return Err(Error::Unauthorized(
                    "identity document doesn't match project id",
                ));
            }

            match identity {
                identities::SomeIdentity::Person(_) => {
                    storage
                        .identities::<git::identities::Person>()
                        .verify(to)
                        .map_err(|e| Error::VerifyIdentity(e.to_string()))?;
                }
                identities::SomeIdentity::Project(_) => {
                    storage
                        .identities::<git::identities::Project>()
                        .verify(to, lookup)
                        .map_err(|e| Error::VerifyIdentity(e.to_string()))?;
                }
                _ => {
                    return Err(Error::Unauthorized("unknown identity type"));
                }
            }

            // Set local project identity to point to the verified commit pushed by the user.
            repo.reference(
                &self.namespace_ref(RAD_ID_REF),
                to,
                false,
                &format!("set-project-id ({})", self.key_fingerprint),
            )?;
        }
        Ok(())
    }

    fn namespace_ref(&self, refname: &str) -> String {
        format!(
            "refs/namespaces/{}/refs/{}",
            &self.env.git_namespace, refname
        )
    }

    fn track_identity(&self, peer_id: Option<PeerId>) -> Result<(), Error> {
        if let Some(peer_id) = peer_id {
            println!("Tracking peer {}...", peer_id);
        } else {
            println!("Tracking project...");
        }

        let cfg = tracking::config::Config::default();
        let storage = Storage::open(&self.paths)?;

        tracking::track(
            &storage,
            &self.urn,
            peer_id,
            cfg,
            tracking::policy::Track::Any,
        )??;

        println!("Tracking successful.");

        Ok(())
    }

    pub fn receive_hook(&self) -> Result<(), Error> {
        use std::process::{Command, Stdio};

        let hook = if let Some(path) = &self.env.receive_hook {
            path
        } else {
            return Ok(());
        };
        println!("Running custom receive hook...");

        let mut child = Command::new(hook)
            .stderr(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stdin(Stdio::piped())
            .spawn()
            .map_err(Error::CustomHook)?;

        if let Some(mut stdin) = child.stdin.take() {
            for (refname, old, new) in self.updates.iter() {
                let (peer_id, refname) = crate::parse_ref(refname)?;

                if let Some(branch) = refname.strip_prefix("heads/") {
                    writeln!(
                        &mut stdin,
                        "{} {} {} {} {} {}",
                        self.urn, self.key_fingerprint, peer_id, old, new, branch
                    )?;
                }
            }
        }

        match child.wait() {
            Ok(status) => {
                if status.success() {
                    println!("Custom receive hook success.");
                } else {
                    println!("Custom receive hook failed.");
                }
            }
            Err(err) => return Err(err.into()),
        }

        Ok(())
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
