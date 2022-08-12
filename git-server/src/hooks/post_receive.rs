//! # POST-RECEIVE HOOK
//!
//! <https://git-scm.com/docs/githooks#post-receive>
//!
use std::io::prelude::*;
use std::io::{stdin, ErrorKind, Write};
use std::path::Path;
use std::str;
use std::str::FromStr;

use either::Either;
use envconfig::Envconfig;
use git2::{Oid, Repository};
use librad::git;
use librad::git::identities;
use librad::git::identities::SomeIdentity;
use librad::git::storage::read::ReadOnlyStorage as _;
use librad::git::tracking;
use librad::git::Urn;
use librad::paths::Paths;
use librad::profile::Profile;
use librad::PeerId;

use super::storage::Storage;
use super::{types::ReceivePackEnv, CertSignerDetails};
use crate::error::Error;

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
        let paths = if let Some(root) = &env.root {
            Profile::from_root(Path::new(root), None)?.paths().clone()
        } else {
            Profile::load()?.paths().clone()
        };
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
            .ok_or(Error::PostReceive("push certificate is not available"))?
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
                post_receive.update_identity(&repo)?;
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

            for (refname, from, _) in self.updates.iter() {
                let (peer_id, rest) = crate::parse_ref(refname)?;

                if from.is_zero() {
                    println!("Deleted ref {} for {}", rest, peer_id);
                } else {
                    println!("Updated ref {} for {}", rest, peer_id);
                }

                // Only delegates can update HEAD.
                if !self.delegates.contains(&peer_id) {
                    continue;
                }
                // For now, we only create a ref for the default branch.
                if rest != suffix {
                    continue;
                }
                println!("Update to default branch detected, setting HEAD...");

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

    fn update_identity(&mut self, repo: &Repository) -> Result<(), Error> {
        if let Some(oid) = self.find_identity_update() {
            eprintln!("Updating identity to {}...", oid);
            self.set_identity_ref(oid, repo)
        } else {
            Ok(())
        }
    }

    fn find_identity_update(&self) -> Option<Oid> {
        if let Some(update) = self
            .updates
            .iter()
            .find(|(refname, _, _)| refname.ends_with(RAD_ID_REF))
        {
            let (_, _, identity_oid) = update;

            Some(*identity_oid)
        } else {
            None
        }
    }

    fn initialize_identity(&mut self, repo: &Repository) -> Result<(), Error> {
        eprintln!("Initializing identity...");

        // Make sure one of the ref updates is initializing `rad/id`.
        let identity_oid = if let Some(oid) = self.find_identity_update() {
            oid
        } else {
            return Err(Error::PostReceive(
                "identity ref 'rad/id' not found in updates",
            ));
        };

        // When initializing a new identity, We shouldn't be updating anything, we should be
        // creating new refs.
        if !self.updates.iter().all(|(_, from, _)| from.is_zero()) {
            return Err(Error::PostReceive("identity old ref already exists"));
        }

        self.set_identity_ref(identity_oid, repo)
    }

    fn set_identity_ref(&self, identity_oid: Oid, repo: &Repository) -> Result<(), Error> {
        let storage = git::storage::ReadOnly::open(&self.paths)?;
        let lookup = |urn| {
            let refname = git::types::Reference::rad_id(git::types::Namespace::from(urn));
            storage.reference_oid(&refname).map(|oid| oid.into())
        };

        let identity = storage
            .identities::<identities::SomeIdentity>()
            .some_identity(identity_oid)
            .map_err(|_| Error::NamespaceNotFound)?;

        // Make sure that the identity we're pushing matches the namespace
        // we're pushing to.
        if identity.urn() != self.urn {
            return Err(Error::PostReceive(
                "identity document doesn't match project id",
            ));
        }

        match identity {
            identities::SomeIdentity::Person(_) => {
                storage
                    .identities::<git::identities::Person>()
                    .verify(identity_oid)
                    .map_err(|e| Error::VerifyIdentity(e.to_string()))?;
            }
            identities::SomeIdentity::Project(_) => {
                storage
                    .identities::<git::identities::Project>()
                    .verify(identity_oid, lookup)
                    .map_err(|e| Error::VerifyIdentity(e.to_string()))?;
            }
            _ => {
                return Err(Error::PostReceive("unknown identity type"));
            }
        }

        // Set local identity to point to the verified commit pushed by the user.
        repo.reference(
            &self.namespace_ref(RAD_ID_REF),
            identity_oid,
            true,
            &format!("set-id ({})", self.key_fingerprint),
        )?;

        Ok(())
    }

    fn namespace_ref(&self, refname: &str) -> String {
        format!(
            "refs/namespaces/{}/refs/{}",
            &self.env.git_namespace, refname
        )
    }

    fn track_identity(&self, peer_id: Option<PeerId>) -> Result<(), Error> {
        let cfg = tracking::config::Config::default();
        let storage = Storage::open(&self.paths)?;

        if let Some(peer) = peer_id {
            println!("Tracking {}...", peer);

            tracking::track(
                &storage,
                &self.urn,
                Some(peer),
                cfg,
                tracking::policy::Track::Any,
            )??;
        } else {
            println!("Fetching project delegates...");

            let identity = identities::any::get(&storage, &self.urn)?
                .ok_or(Error::PostReceive("identity could not be found"))?;
            let mut delegates: Vec<PeerId> = Vec::new();

            match identity {
                SomeIdentity::Person(doc) => {
                    for key in doc.delegations() {
                        delegates.push(PeerId::from(*key));
                    }
                }
                SomeIdentity::Project(doc) => {
                    for d in doc.delegations() {
                        match d {
                            Either::Left(key) => {
                                delegates.push(PeerId::from(*key));
                            }
                            Either::Right(indirect) => {
                                for key in indirect.delegations() {
                                    delegates.push(PeerId::from(*key));
                                }
                            }
                        }
                    }
                }
                _ => {}
            }

            // TODO: We shouldn't track all delegates because we don't have their branches/remotes!
            // We should only track the peer that is pushing.
            for peer in delegates {
                println!("Tracking {}...", peer);

                tracking::track(
                    &storage,
                    &self.urn,
                    Some(peer),
                    cfg.clone(),
                    tracking::policy::Track::Any,
                )??;
            }
        }
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

        let child = Command::new(hook)
            .stderr(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stdin(Stdio::piped())
            .spawn()
            .map_err(Error::CustomHook);

        if let Err(Error::CustomHook(ref err)) = child {
            if err.kind() == ErrorKind::NotFound {
                println!("Custom receive hook not found in {:?}, skipping...", hook);
                return Ok(());
            }
        }

        let mut child = child?;

        if let Some(mut stdin) = child.stdin.take() {
            for (refname, old, new) in self.updates.iter() {
                let (peer_id, refname) = crate::parse_ref(refname)?;

                if let Some(branch) = refname.strip_prefix("heads/") {
                    writeln!(&mut stdin, "{} {} {} {}", peer_id, old, new, branch)?;
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
}
