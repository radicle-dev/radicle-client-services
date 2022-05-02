use crate::commit::Peer;
use serde::{Deserialize, Serialize};

/// A patch is a change set that a user wants the maintainer to merge into a projects default branch.
///
/// A patch is represented by an annotated tag, prefixed with `patches/`.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    /// ID of a patch. This is the portion of the tag name following the `patches/` prefix.
    pub id: String,
    /// Peer that the patch originated from
    pub peer: Peer,
    /// Message attached to the patch. This is the message of the annotated tag.
    pub message: String,
    /// Head commit that the author wants to merge with this patch.
    pub commit: String,
    /// The merge base of [`Metadata::commit`] and the head commit of the first maintainer's default branch.
    pub merge_base: Option<String>,
}
