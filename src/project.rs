use std::collections::HashSet;
use std::convert::TryFrom;

use radicle_daemon::Urn;
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
    /// Project name.
    pub name: String,
    /// Project description.
    pub description: String,
    /// Default branch of project.
    pub default_branch: String,
    /// List of maintainers.
    pub maintainers: HashSet<Urn>,
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

        Ok(Self {
            name: subject.name.to_string(),
            description: subject
                .description
                .clone()
                .map_or_else(|| "".into(), |desc| desc.to_string()),
            default_branch,
            maintainers,
        })
    }
}
