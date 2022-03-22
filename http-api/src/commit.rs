use radicle_source::commit::Header;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct CommitsQueryString {
    pub parent: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub page: Option<usize>,
    pub per_page: Option<usize>,
}

#[derive(Serialize)]
pub struct Commit {
    pub header: Header,
    pub context: CommitContext,
}

#[derive(Serialize)]
pub struct CommitContext {
    pub committer: Option<Committer>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Person {
    pub name: String,
}

#[derive(Serialize)]
pub struct Committer {
    pub peer: Peer,
}

#[derive(Serialize, Debug, Clone)]
pub struct Peer {
    pub id: String,
    pub person: Person,
    pub delegate: bool,
}
