use std::collections::HashSet;
use std::convert::TryFrom;

use either::Either;
use librad::{git::Urn, PeerId};
use serde::{Deserialize, Serialize};

use crate::error;

/// Project info.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// Project metadata.
    #[serde(flatten)]
    pub meta: Metadata,
    /// Project HEAD commit. If empty, it's likely that no delegate
    /// branches have been replicated on this node.
    #[serde(with = "option")]
    pub head: Option<git2::Oid>,
}

/// Project delegate.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Delegate {
    /// Direct delegation, ie. public key.
    Direct { id: PeerId },
    /// Indirect delegation, ie. a personal identity.
    Indirect { urn: Urn, ids: HashSet<PeerId> },
}

impl Delegate {
    pub fn contains(&self, other: &PeerId) -> bool {
        match self {
            Self::Direct { id } => id == other,
            Self::Indirect { ids, .. } => ids.contains(other),
        }
    }
}

/// Project metadata.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    /// Project urn.
    pub urn: Urn,
    /// Project name.
    pub name: String,
    /// Project description.
    pub description: String,
    /// Default branch of project.
    pub default_branch: String,
    /// List of delegates.
    pub delegates: Vec<Delegate>,
}

impl TryFrom<librad::identities::Project> for Metadata {
    type Error = error::Error;

    fn try_from(project: librad::identities::Project) -> Result<Self, Self::Error> {
        let subject = project.subject();
        let default_branch = subject
            .default_branch
            .clone()
            .ok_or(error::Error::MissingDefaultBranch)?
            .to_string();

        let mut delegates = Vec::new();
        for delegate in project.delegations().iter() {
            match delegate {
                Either::Left(pk) => {
                    delegates.push(Delegate::Direct {
                        id: PeerId::from(*pk),
                    });
                }
                Either::Right(indirect) => {
                    delegates.push(Delegate::Indirect {
                        urn: indirect.urn(),
                        ids: indirect
                            .delegations()
                            .iter()
                            .map(|pk| PeerId::from(*pk))
                            .collect(),
                    });
                }
            }
        }

        Ok(Self {
            urn: project.urn(),
            name: subject.name.to_string(),
            description: subject
                .description
                .clone()
                .map_or_else(|| "".into(), |desc| desc.to_string()),
            default_branch,
            delegates,
        })
    }
}

mod option {
    use std::fmt::Display;

    use serde::Serializer;

    pub fn serialize<T, S>(value: &Option<T>, serializer: S) -> Result<S::Ok, S::Error>
    where
        T: Display,
        S: Serializer,
    {
        if let Some(value) = value {
            serializer.collect_str(value)
        } else {
            serializer.serialize_none()
        }
    }
}
