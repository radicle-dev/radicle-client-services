use serde::Serialize;

use librad::git::storage::ReadOnly;
use librad::git::tracking;

pub use radicle_common::project::{Delegate, Metadata, PeerInfo};

use crate::Error;

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
    pub patches: usize,
    pub issues: usize,
}

pub fn tracked<S: AsRef<ReadOnly>>(meta: &Metadata, storage: &S) -> Result<Vec<PeerInfo>, Error> {
    let tracked =
        tracking::tracked(storage.as_ref(), Some(&meta.urn)).map_err(|_| Error::NotFound)?;
    let result = tracked
        .collect::<Result<Vec<_>, _>>()
        .map_err(Error::from)?;

    let result = result
        .into_iter()
        .filter_map(|t| t.peer_id())
        .map(|id| PeerInfo::get(&id, meta, storage))
        .collect::<Vec<_>>();

    Ok(result)
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
