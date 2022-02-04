use librad::canonical::Canonical as _;
use librad::git::tracking::git::odb;
use librad::git::tracking::git::odb::Read as _;
use librad::git::tracking::git::refdb;
use librad::git::tracking::reference::RefName;
use librad::git::tracking::Config;
use librad::git_ext::RefspecPattern;
use librad::git_ext::{is_not_found_err, Oid, RefLike};
use librad::paths::Paths;

use std::convert::Infallible;

use crate::error::Error;

type Ref<'a> = refdb::Ref<'a, Oid>;

pub struct Storage {
    pub backend: git2::Repository,
    pub paths: Paths,

    ro: librad::git::storage::ReadOnly,
}

impl AsRef<librad::git::storage::ReadOnly> for Storage {
    fn as_ref(&self) -> &librad::git::storage::ReadOnly {
        &self.ro
    }
}

impl Storage {
    pub fn open(paths: &Paths) -> Result<Self, Error> {
        let backend = git2::Repository::open(paths.git_dir())?;
        let ro = librad::git::storage::ReadOnly::open(paths)?;

        Ok(Self {
            backend,
            paths: paths.clone(),
            ro,
        })
    }

    fn reference<'a, 'b, Ref: 'b>(
        &'a self,
        reference: &'b Ref,
    ) -> Result<Option<git2::Reference<'a>>, librad::git::storage::read::Error>
    where
        RefLike: From<&'b Ref>,
        Ref: std::fmt::Debug,
    {
        self.backend
            .find_reference(RefLike::from(reference).as_str())
            .map(Some)
            .or_else(|e| {
                if is_not_found_err(&e) {
                    Ok(None)
                } else {
                    Err(e.into())
                }
            })
    }
}

impl odb::Read for Storage {
    type FindError = Infallible;
    type Oid = Oid;

    fn find_config(&self, _oid: &Self::Oid) -> Result<Option<Config>, Self::FindError> {
        unimplemented!()
    }
}

impl<'a> refdb::Read<'a> for Storage {
    type FindError = Infallible;
    type ReferencesError = Infallible;
    type IterError = Infallible;
    type Oid = Oid;
    type References = std::iter::Empty<Result<Ref<'a>, Infallible>>;

    fn find_reference(
        &self,
        _reference: &RefName<'_, Self::Oid>,
    ) -> Result<Option<Ref<'_>>, Self::FindError> {
        unimplemented!()
    }

    fn references(
        &'a self,
        _spec: &RefspecPattern,
    ) -> Result<Self::References, Self::ReferencesError> {
        unimplemented!()
    }
}

impl odb::Write for Storage {
    type ModifyError = Infallible;
    type WriteError = Infallible;
    type Oid = Oid;

    fn write_config(&self, config: &Config) -> Result<Self::Oid, Self::WriteError> {
        // unwrap is safe since Error is Infallible
        Ok(self
            .backend
            .blob(&config.canonical_form().unwrap())
            .map(Oid::from)
            .unwrap())
    }

    fn modify_config<F>(&self, oid: &Self::Oid, f: F) -> Result<Self::Oid, Self::ModifyError>
    where
        F: FnOnce(Config) -> Config,
    {
        let config = self
            .find_config(oid)
            .expect("Storage::modify_config: config search should succeed")
            .expect("Storage::modify_config: config should exist");

        Ok(self
            .write_config(&f(config))
            .expect("Storage::modify_config: config should be written successfully"))
    }
}

mod error {
    use super::Oid;
    use thiserror::Error;

    #[derive(Debug, Error)]
    #[error("the reference was symbolic, but it is expected to be direct")]
    pub struct SymbolicRef;

    #[derive(Debug, Error)]
    pub enum Txn {
        #[error("failed to initialise git transaction")]
        Acquire(#[source] git2::Error),
        #[error("failed to commit git transaction")]
        Commit(#[source] git2::Error),
        #[error("failed to delete reference `{refname}`")]
        Delete {
            refname: String,
            #[source]
            source: git2::Error,
        },
        #[error("failed while acquiring lock for `{refname}`")]
        Lock {
            refname: String,
            #[source]
            source: git2::Error,
        },
        #[error(transparent)]
        Read(#[from] librad::git::storage::read::Error),
        #[error(transparent)]
        SymbolicRef(#[from] SymbolicRef),
        #[error("failed to write reference `{refname}` with target `{target}`")]
        Write {
            refname: String,
            target: Oid,
            #[source]
            source: git2::Error,
        },
    }
}

// NOTE: Copied from `radicle-link`.
// If we find a better way to have a storage instance with read/write capabilities without a
// signer instance, we can replace this.
impl refdb::Write for Storage {
    type TxnError = error::Txn;
    type Oid = Oid;

    fn update<'a, I>(&self, updates: I) -> Result<refdb::Applied<'a, Self::Oid>, Self::TxnError>
    where
        I: IntoIterator<Item = refdb::Update<'a, Self::Oid>>,
    {
        use refdb::{Applied, PreviousError, Update, Updated};

        let raw = &self.backend;
        let mut txn = raw.transaction().map_err(error::Txn::Acquire)?;
        let mut applied = Applied::default();
        let mut reject_or_update =
            |apply: Result<Updated<'a, Self::Oid>, PreviousError<Self::Oid>>| match apply {
                Ok(update) => applied.updates.push(update),
                Err(rejection) => applied.rejections.push(rejection),
            };

        for update in updates {
            match update {
                Update::Write {
                    name,
                    target,
                    previous,
                } => {
                    let refname = name.to_string();
                    let message = &format!("writing reference with target `{}`", target);
                    txn.lock_ref(&refname).map_err(|err| error::Txn::Lock {
                        refname: refname.clone(),
                        source: err,
                    })?;
                    let set = || -> Result<(), Self::TxnError> {
                        txn.set_target(&refname, target.into(), None, message)
                            .map_err(|err| error::Txn::Write {
                                refname,
                                target,
                                source: err,
                            })
                    };
                    match self.reference(&name)? {
                        Some(r) => reject_or_update(
                            previous
                                .guard(r.target().map(Oid::from).as_ref(), set)?
                                .map_or(Ok(Updated::Written { name, target }), Err),
                        ),
                        None => reject_or_update(
                            previous
                                .guard(None, set)?
                                .map_or(Ok(Updated::Written { name, target }), Err),
                        ),
                    }
                }
                Update::Delete { name, previous } => {
                    let refname = name.to_string();
                    txn.lock_ref(&refname).map_err(|err| error::Txn::Lock {
                        refname: refname.clone(),
                        source: err,
                    })?;
                    let delete = || -> Result<(), Self::TxnError> {
                        txn.remove(&refname).map_err(|err| error::Txn::Delete {
                            refname,
                            source: err,
                        })
                    };
                    match self.reference(&name)? {
                        Some(r) => reject_or_update(
                            previous
                                .guard(r.target().map(Oid::from).as_ref(), delete)?
                                .map_or(
                                    Ok(Updated::Deleted {
                                        name,
                                        previous: r
                                            .target()
                                            .map(Ok)
                                            .unwrap_or(Err(error::SymbolicRef))?
                                            .into(),
                                    }),
                                    Err,
                                ),
                        ),
                        None => match previous {
                            refdb::PreviousValue::Any
                            | refdb::PreviousValue::MustNotExist
                            | refdb::PreviousValue::IfExistsMustMatch(_) => { /* no-op */ }
                            _ => reject_or_update(Err(PreviousError::DidNotExist)),
                        },
                    }
                }
            }
        }
        txn.commit().map_err(error::Txn::Commit)?;

        Ok(applied)
    }
}
