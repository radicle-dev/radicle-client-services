use serde::{Deserialize, Serialize};

use radicle_common::project::PeerInfo;
use radicle_source::commit::Header;

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
    pub header: Header,
    pub context: CommitContext,
}

#[derive(Serialize)]
pub struct Commit {
    pub header: Header,
    pub stats: radicle_source::commit::Stats,
    pub diff: radicle_surf::diff::Diff,
    pub branches: Vec<radicle_source::Branch>,
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
