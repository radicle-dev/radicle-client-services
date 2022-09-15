use serde::{Deserialize, Serialize};

use radicle_common::project::PeerInfo;
use radicle_surf::diff;
use radicle_surf::vcs::git;
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct CommitsQueryString {
    pub parent: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub page: Option<usize>,
    pub per_page: Option<usize>,
    pub verified: Option<bool>,
}

#[derive(Serialize)]
pub struct CommitTeaser {
    pub header: git::Commit,
    pub context: CommitContext,
}

#[derive(Serialize)]
pub struct Header {
    pub header: git::Commit,
    pub stats: diff::Stats,
    pub diff: diff::Diff,
    pub branches: Vec<git::BranchName>,
    pub context: CommitContext,
}

#[derive(Serialize)]
pub struct CommitContext {
    pub committer: Option<Committer>,
}

#[derive(Serialize)]
pub struct Committer {
    pub peer: PeerInfo,
}
