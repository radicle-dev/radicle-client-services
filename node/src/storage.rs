pub mod git;
pub mod refs;

use std::collections::hash_map;
use std::io;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::path::Path;

use radicle_git_ext as git_ext;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use radicle_git_ext::Oid;

use crate::collections::HashMap;
use crate::crypto::{self, Unverified, Verified};
use crate::git::Url;
use crate::git::{RefError, RefStr};
use crate::identity;
use crate::identity::{ProjId, ProjIdError, Project, UserId};
use crate::storage::refs::Refs;

use self::refs::SignedRefs;

pub type BranchName = String;
pub type Inventory = Vec<ProjId>;

/// Storage error.
#[derive(Error, Debug)]
pub enum Error {
    #[error("invalid git reference")]
    InvalidRef,
    #[error("git reference error: {0}")]
    Ref(#[from] RefError),
    #[error(transparent)]
    Refs(#[from] refs::Error),
    #[error("git: {0}")]
    Git(#[from] git2::Error),
    #[error("id: {0}")]
    ProjId(#[from] ProjIdError),
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
    #[error("doc: {0}")]
    Doc(#[from] identity::DocError),
    #[error("invalid repository head")]
    InvalidHead,
}

pub type RemoteId = UserId;

/// Project remotes. Tracks the git state of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remotes<V>(HashMap<RemoteId, Remote<V>>);

impl<V> FromIterator<(RemoteId, Remote<V>)> for Remotes<V> {
    fn from_iter<T: IntoIterator<Item = (RemoteId, Remote<V>)>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<V> Deref for Remotes<V> {
    type Target = HashMap<RemoteId, Remote<V>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<V> Remotes<V> {
    pub fn new(remotes: HashMap<RemoteId, Remote<V>>) -> Self {
        Self(remotes)
    }
}

impl Remotes<Verified> {
    pub fn unverified(self) -> Remotes<Unverified> {
        Remotes(
            self.into_iter()
                .map(|(id, r)| (id, r.unverified()))
                .collect(),
        )
    }
}

impl<V> Default for Remotes<V> {
    fn default() -> Self {
        Self(HashMap::default())
    }
}

impl<V> IntoIterator for Remotes<V> {
    type Item = (RemoteId, Remote<V>);
    type IntoIter = hash_map::IntoIter<RemoteId, Remote<V>>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<V> From<Remotes<V>> for HashMap<RemoteId, Refs> {
    fn from(other: Remotes<V>) -> Self {
        let mut remotes = HashMap::with_hasher(fastrand::Rng::new().into());

        for (k, v) in other.into_iter() {
            remotes.insert(k, v.refs.into());
        }
        remotes
    }
}

/// A project remote.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Remote<V> {
    /// ID of remote.
    pub id: UserId,
    /// Git references published under this remote, and their hashes.
    pub refs: SignedRefs<V>,
    /// Whether this remote is of a project delegate.
    pub delegate: bool,
    /// Whether the remote is verified or not, ie. whether its signed refs were checked.
    #[serde(skip)]
    verified: PhantomData<V>,
}

impl<V> Remote<V> {
    pub fn new(id: UserId, refs: impl Into<SignedRefs<V>>) -> Self {
        Self {
            id,
            refs: refs.into(),
            delegate: false,
            verified: PhantomData,
        }
    }
}

impl Remote<Unverified> {
    pub fn verified(self) -> Result<Remote<Verified>, crypto::Error> {
        let refs = self.refs.verified(&self.id)?;

        Ok(Remote {
            id: self.id,
            refs,
            delegate: self.delegate,
            verified: PhantomData,
        })
    }
}

impl Remote<Verified> {
    pub fn unverified(self) -> Remote<Unverified> {
        Remote {
            id: self.id,
            refs: self.refs.unverified(),
            delegate: self.delegate,
            verified: PhantomData,
        }
    }
}

pub trait ReadStorage {
    fn user_id(&self) -> &UserId;
    fn url(&self) -> Url;
    fn get(&self, proj: &ProjId) -> Result<Option<Project>, Error>;
    fn inventory(&self) -> Result<Inventory, Error>;
}

pub trait WriteStorage<'r>: ReadStorage {
    type Repository: UpdateRepository<'r>;

    fn repository(&self, proj: &ProjId) -> Result<Self::Repository, Error>;
    fn sign_refs(&self, repository: &Self::Repository) -> Result<SignedRefs<Verified>, Error>;
}

pub trait ReadRepository<'r> {
    type Remotes: Iterator<Item = Result<(RemoteId, Remote<Verified>), refs::Error>> + 'r;

    fn is_empty(&self) -> Result<bool, git2::Error>;
    fn path(&self) -> &Path;
    fn blob_at<'a>(&'a self, oid: Oid, path: &'a Path) -> Result<git2::Blob<'a>, git_ext::Error>;
    fn reference(
        &self,
        user: &UserId,
        reference: &RefStr,
    ) -> Result<Option<git2::Reference>, git2::Error>;
    fn reference_oid(&self, user: &UserId, reference: &RefStr) -> Result<Option<Oid>, git2::Error>;
    fn references(&self, user: &UserId) -> Result<Refs, Error>;
    fn remote(&self, user: &UserId) -> Result<Remote<Verified>, refs::Error>;
    fn remotes(&'r self) -> Result<Self::Remotes, git2::Error>;
}

pub trait UpdateRepository<'r>: ReadRepository<'r> {
    fn fetch(&mut self, url: &Url) -> Result<(), git2::Error>;
    fn raw(&self) -> &git2::Repository;
}

impl<T, S> ReadStorage for T
where
    T: Deref<Target = S>,
    S: ReadStorage + 'static,
{
    fn user_id(&self) -> &UserId {
        self.deref().user_id()
    }

    fn url(&self) -> Url {
        self.deref().url()
    }

    fn inventory(&self) -> Result<Inventory, Error> {
        self.deref().inventory()
    }

    fn get(&self, proj: &ProjId) -> Result<Option<Project>, Error> {
        self.deref().get(proj)
    }
}

impl<'r, T, S> WriteStorage<'r> for T
where
    T: DerefMut<Target = S>,
    S: WriteStorage<'r> + 'static,
{
    type Repository = S::Repository;

    fn repository(&self, proj: &ProjId) -> Result<Self::Repository, Error> {
        self.deref().repository(proj)
    }

    fn sign_refs(&self, repository: &S::Repository) -> Result<SignedRefs<Verified>, Error> {
        self.deref().sign_refs(repository)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_storage() {}
}
