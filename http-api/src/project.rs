use std::collections::HashSet;
use std::convert::TryFrom;

use either::Either;
use radicle_daemon::{PeerId, Urn};
use serde::{Deserialize, Serialize};

use crate::error;

/// Project info.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// Project head commit hash.
    pub head: String,
    /// Project metadata.
    pub meta: Metadata,
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
    /// List of maintainers.
    pub maintainers: HashSet<Urn>,
    /// List of delegates.
    pub delegates: Vec<PeerId>,
}

impl TryFrom<radicle_daemon::Project> for Metadata {
    type Error = error::Error;

    fn try_from(project: radicle_daemon::Project) -> Result<Self, Self::Error> {
        let subject = project.subject();
        // TODO: Some maintainers may be directly delegating, i.e. only supply their PublicKey.
        let maintainers = project
            .delegations()
            .iter()
            .indirect()
            .map(|indirect| indirect.urn())
            .collect();
        let default_branch = subject
            .default_branch
            .clone()
            .ok_or(error::Error::MissingDefaultBranch)?
            .to_string();
        let delegates = project
            .delegations()
            .iter()
            .flat_map(|either| match either {
                Either::Left(pk) => Either::Left(std::iter::once(PeerId::from(*pk))),
                Either::Right(indirect) => {
                    Either::Right(indirect.delegations().iter().map(|pk| PeerId::from(*pk)))
                }
            })
            .collect::<Vec<PeerId>>();

        Ok(Self {
            urn: project.urn(),
            name: subject.name.to_string(),
            description: subject
                .description
                .clone()
                .map_or_else(|| "".into(), |desc| desc.to_string()),
            default_branch,
            maintainers,
            delegates,
        })
    }
}
