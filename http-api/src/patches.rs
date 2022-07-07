use std::convert::TryFrom;
use std::convert::TryInto;

use warp::{self, path, Filter, Rejection, Reply};

use librad::collaborative_objects::ObjectId;
use librad::git::{identities, Storage, Urn};
use librad::PeerId;

use radicle_common::cobs::{patch, Store};
use radicle_common::{person, project};
use radicle_source::commit::Stats;
use radicle_source::surf::vcs::git;
use radicle_source::Commit;
use radicle_surf::diff;

use crate::error::Error;
use crate::remote_branch;
use crate::Context;

/// A collaborative object that includes its id.
#[derive(serde::Serialize)]
struct Cob<T: serde::Serialize> {
    id: ObjectId,
    #[serde(flatten)]
    inner: T,
}

impl<T: serde::Serialize> Cob<T> {
    pub fn new(id: ObjectId, inner: T) -> Self {
        Self { id, inner }
    }
}

#[derive(serde::Serialize, Clone)]
struct Changeset {
    commits: Vec<Commit>,
    diff: git::Diff,
    stats: Stats,
}

impl Changeset {
    pub fn new(commits: Vec<Commit>, diff: git::Diff) -> Self {
        Self {
            commits,
            stats: Changeset::stats(&diff),
            diff,
        }
    }

    // TODO: This function should probably be moved to radicle_surf, where it should be able to be called on radicle_surf::diff::Diff as associated function`
    pub fn stats(diff: &git::Diff) -> Stats {
        let mut deletions = 0;
        let mut additions = 0;

        for file in &diff.modified {
            if let diff::FileDiff::Plain { ref hunks } = file.diff {
                for hunk in hunks.iter() {
                    for line in &hunk.lines {
                        match line {
                            diff::LineDiff::Addition { .. } => additions += 1,
                            diff::LineDiff::Deletion { .. } => deletions += 1,
                            _ => {}
                        }
                    }
                }
            }
        }

        for file in &diff.created {
            if let diff::FileDiff::Plain { ref hunks } = file.diff {
                for hunk in hunks.iter() {
                    for line in &hunk.lines {
                        if let diff::LineDiff::Addition { .. } = line {
                            additions += 1
                        }
                    }
                }
            }
        }

        for file in &diff.deleted {
            if let diff::FileDiff::Plain { ref hunks } = file.diff {
                for hunk in hunks.iter() {
                    for line in &hunk.lines {
                        if let diff::LineDiff::Deletion { .. } = line {
                            deletions += 1
                        }
                    }
                }
            }
        }

        Stats {
            additions,
            deletions,
        }
    }
}

/// `GET /:project/patches`
pub fn patches_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("patches"))
        .and(path::end())
        .and_then(patches_handler)
}

/// `GET /:project/patches/:id`
pub fn patch_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("patches"))
        .and(path::param::<ObjectId>())
        .and(path::end())
        .and_then(patch_handler)
}

async fn patches_handler(ctx: Context, urn: Urn) -> Result<impl Reply, Rejection> {
    let repo = git::Repository::new(ctx.paths.git_dir()).map_err(Error::from)?;
    let storage = ctx.storage().await?;
    let project = identities::project::get(storage.as_ref(), &urn)
        .map_err(Error::Identities)?
        .ok_or(Error::NotFound)?;
    let meta: project::Metadata = project.try_into().map_err(Error::Project)?;

    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let store = Store::new(whoami, &ctx.paths, &storage).map_err(Error::from)?;
    let patches = patch::PatchStore::new(&store);

    let mut browser = git::Browser::new_with_namespace(
        &repo,
        &git::Namespace::try_from(urn.encode_id().as_str()).map_err(|_| Error::MissingNamespace)?,
        git::Branch::local(meta.default_branch.as_str()),
    )
    .map_err(Error::from)?;

    let all: Vec<_> = patches
        .all(&urn)
        .map_err(Error::Patches)?
        .into_iter()
        .map(|(id, mut patch)| {
            if let Err(e) = patch
                .resolve(storage.as_ref())
                .map_err(Error::IdentityResolve)
            {
                tracing::warn!("Failed to resolve identities in patch {}: {}", id, e);
            }

            browser
                .branch(remote_branch(
                    meta.default_branch.as_str(),
                    &patch.author.peer,
                ))
                .expect("Was not able to find {} in namespace and remote");

            Cob::new(
                id,
                resolve_revisions(patch, &mut browser, &meta, &storage, false),
            )
        })
        .collect();

    Ok(warp::reply::json(&all))
}

async fn patch_handler(
    ctx: Context,
    urn: Urn,
    patch_id: ObjectId,
) -> Result<impl Reply, Rejection> {
    let repo = git::Repository::new(ctx.paths.git_dir()).map_err(Error::from)?;
    let storage = ctx.storage().await?;
    let project = identities::project::get(storage.as_ref(), &urn)
        .map_err(Error::Identities)?
        .ok_or(Error::NotFound)?;
    let meta: project::Metadata = project.try_into().map_err(Error::Project)?;

    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let store = Store::new(whoami, &ctx.paths, &storage).map_err(Error::from)?;
    let patches = patch::PatchStore::new(&store);
    let mut patch = patches
        .get(&urn, &patch_id)
        .map_err(Error::from)?
        .ok_or(Error::NotFound)?;
    if let Err(e) = patch
        .resolve(storage.as_ref())
        .map_err(Error::IdentityResolve)
    {
        tracing::warn!("Failed to resolve identities in patch {}: {}", patch_id, e);
    }

    let mut browser = git::Browser::new_with_namespace(
        &repo,
        &git::Namespace::try_from(urn.encode_id().as_str()).map_err(|_| Error::MissingNamespace)?,
        remote_branch(meta.default_branch.as_str(), &patch.author.peer),
    )
    .map_err(Error::from)?;

    Ok(warp::reply::json(&Cob::new(
        patch_id,
        resolve_revisions(patch, &mut browser, &meta, &storage, true),
    )))
}

// TODO: This function could eventually be moved to radicle-common and be part of Revision
fn resolve_revisions(
    patch: patch::Patch,
    browser: &mut git::Browser,
    meta: &project::Metadata,
    storage: &Storage,
    include_changeset: bool,
) -> patch::Patch<Option<Changeset>, project::PeerInfo> {
    let revisions = patch
        .revisions
        .into_iter()
        .map(|revision| {
            // Resolves Merge structs with PeerInfo
            let merges = revision
                .merges
                .iter()
                .map(|merge| {
                    let peer = project::PeerInfo::get(&merge.peer, meta, storage);
                    patch::Merge {
                        peer,
                        commit: merge.commit,
                        timestamp: merge.timestamp,
                    }
                })
                .collect();

            // Locates the browser at the Oid of the current revision.
            if let Err(e) = browser.rev(git::Rev::Oid(*revision.oid)) {
                tracing::warn!(
                    "Failed to set the browser history at {}: {}",
                    *revision.oid,
                    e
                );

                return patch::Revision {
                    id: revision.id,
                    peer: revision.peer,
                    oid: revision.oid,
                    base: revision.base,
                    comment: revision.comment,
                    discussion: revision.discussion,
                    reviews: revision.reviews,
                    timestamp: revision.timestamp,
                    changeset: None,
                    merges,
                };
            }

            let mut changeset: Option<Changeset> = None;

            if let (Ok(commits), Ok(diff), true) = (
                radicle_source::commits::<PeerId>(browser, None),
                // Gets the entire diff between the default branch head and the revision Oid.
                browser.diff(*revision.base, *revision.oid),
                // This feature flag, allows us to only generate diffs for e.g. single patch retrieval and skip all this for patch listing.
                include_changeset,
            ) {
                // Iterates over commits headers and retrieves each commit details until it gets to the head of the default branch
                // If radicle_source::commit returns a None the commit won't be collected.
                let commits = commits
                    .headers
                    .iter()
                    .take_while(|header| header.sha1 != *revision.base)
                    .filter_map(|header| radicle_source::commit(browser, header.sha1).ok())
                    .collect::<Vec<Commit>>();

                changeset = Some(Changeset::new(commits, diff));
            };

            patch::Revision {
                id: revision.id,
                peer: revision.peer,
                oid: revision.oid,
                base: revision.base,
                comment: revision.comment,
                discussion: revision.discussion,
                reviews: revision.reviews,
                timestamp: revision.timestamp,
                changeset,
                merges,
            }
        })
        .collect::<Vec<_>>()
        .try_into()
        .unwrap(); // This unwrap is safe, since we work with a NonEmpty struct and won't collect an empty Vec.

    patch::Patch {
        author: patch.author,
        title: patch.title,
        state: patch.state,
        target: patch.target,
        labels: patch.labels,
        timestamp: patch.timestamp,
        revisions,
    }
}
