use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::str::FromStr;

use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Json, Router};
use hyper::StatusCode;
use librad::identities::Project;
use serde::Deserialize;
use serde_json::json;

use librad::collaborative_objects::ObjectId;
use librad::git::identities::{self, SomeIdentity};
use librad::git::types::{Namespace, One, Reference, Single};
use librad::git::Storage;
use librad::git::Urn;
use librad::paths::Paths;
use librad::PeerId;

use radicle_common::cobs::{self, issue, patch, Store};
use radicle_common::person;
use radicle_source as source;
use radicle_source::commit::Stats;
use radicle_source::surf::vcs::git;
use radicle_surf::diff;

use crate::axum_extra::{Path, Query};
use crate::commit::{Commit, CommitContext, CommitTeaser, CommitsQueryString, Committer};
use crate::project::{self, Info};
use crate::{get_head_commit, Context, Error};

pub fn router(ctx: Context) -> Router {
    Router::new()
        .route("/projects", get(project_root_handler))
        .route("/projects/:project", get(project_alias_or_urn_handler))
        .route("/projects/:project/commits", get(history_handler))
        .route("/projects/:project/commits/:sha", get(commit_handler))
        .route("/projects/:project/activity", get(activity_handler))
        .route("/projects/:project/tree/:sha/*path", get(tree_handler))
        .route("/projects/:project/remotes", get(remotes_handler))
        .route("/projects/:project/remotes/:peer", get(remote_handler))
        .route("/projects/:project/blob/:sha/*path", get(blob_handler))
        .route("/projects/:project/readme/:sha", get(readme_handler))
        .route("/projects/:project/patches", get(patches_handler))
        .route("/projects/:project/patches/:id", get(patch_handler))
        .route("/projects/:project/issues", get(issues_handler))
        .route("/projects/:project/issues/:id", get(issue_handler))
        .layer(Extension(ctx))
}

fn get_project_info_sync(
    ctx: Context,
    project: Project,
    storage: deadpool::managed::Object<Storage, librad::git::storage::pool::InitError>,
) -> Result<Info, Error> {
    let repo = git2::Repository::open_bare(&ctx.paths.git_dir()).map_err(Error::from)?;
    let whoami = person::local(&storage).map_err(Error::LocalIdentity)?;
    let cobs = cobs::Store::new(whoami, &ctx.paths, &storage);
    let issues = cobs.issues();
    let patches = cobs.patches();

    let meta: project::Metadata = project.try_into()?;
    let head = get_head_commit(&repo, &meta.urn, &meta.default_branch, &meta.delegates)
        .map(|h| h.id)
        .ok();

    let issues = issues.count(&meta.urn).map_err(Error::Cobs)?;
    let patches = patches.count(&meta.urn).map_err(Error::Cobs)?;

    let info = Info {
        meta,
        head,
        issues,
        patches,
    };

    Ok(info)
}

async fn get_project_info(ctx: Context, project: Project) -> Result<Info, Error> {
    let storage: deadpool::managed::Object<Storage, librad::git::storage::pool::InitError> =
        ctx.storage().await?;
    tokio::task::spawn_blocking(move || get_project_info_sync(ctx, project, storage)).await?
}

async fn get_projects_info(
    ctx: Context,
    Query(qs): Query<project::ProjectsQueryString>,
) -> Result<Vec<Info>, Error> {
    let project::ProjectsQueryString { page, per_page } = qs;
    let page = page.unwrap_or(0);
    let per_page = per_page.unwrap_or(10);

    let storage = ctx.storage().await?;
    let projects: Vec<Project> = identities::any::list(storage.read_only())?
        .map(|res| match res {
            Ok(id) => Ok(id),
            Err(err) => Err(Error::from(err)),
        })
        .filter_map(|id_result| match id_result {
            Ok(SomeIdentity::Project(project)) => Some(Ok(project)),
            Err(err) => Some(Err(err)),
            _ => None,
        })
        .skip(page * per_page)
        .take(per_page)
        .collect::<Result<Vec<Project>, Error>>()?;

    let pending_futures = {
        let mut futures: Vec<tokio::task::JoinHandle<_>> = Vec::with_capacity(projects.len());
        for id in &projects {
            let id = id.clone();
            let ctx = ctx.clone();
            let future = tokio::task::spawn(get_project_info(ctx, id));
            futures.push(future);
        }
        futures
    };

    let mut infos: Vec<Info> = Vec::with_capacity(projects.len());
    for result in futures::future::join_all(pending_futures).await {
        let info = match result? {
            Ok(info) => info,
            Err(err) => {
                tracing::error!("Could not fetch project info: {:?}", err);
                continue;
            }
        };
        infos.push(info);
    }

    Ok(infos)
}

/// List all projects.
/// `GET /projects`
async fn project_root_handler(
    Extension(ctx): Extension<Context>,
    Query(qs): Query<project::ProjectsQueryString>,
) -> impl IntoResponse {
    let projects = get_projects_info(ctx, Query(qs)).await?;
    Ok::<_, Error>(Json(projects))
}

/// Get project commit.
/// `GET /projects/:project/commits/:sha`
async fn commit_handler(
    Extension(ctx): Extension<Context>,
    Path((project, sha)): Path<(Urn, One)>,
) -> impl IntoResponse {
    let reference = Reference::head(Namespace::from(project.clone()), None, sha.to_owned());
    let commit = browse(reference, ctx.paths.clone(), |browser| {
        let oid = browser.oid(&sha)?;
        radicle_source::commit(browser, oid)
    })
    .await?;

    let projects = ctx.projects.read().await;
    let fingerprints = projects.get(&project);
    let fp = ctx.commit_ssh_fingerprint(&commit.header.sha1.to_string())?;
    let committer = if let (Some(fps), Some(fp)) = (fingerprints, fp) {
        fps.get(&fp).cloned().map(|peer| Committer { peer })
    } else {
        None
    };

    let response = Commit {
        header: commit.header,
        diff: commit.diff,
        stats: commit.stats,
        branches: commit.branches,
        context: CommitContext { committer },
    };

    Ok::<_, Error>(Json(json!(response)))
}

/// Get project commit range.
/// `GET /projects/:project/commits?from=<sha>`
async fn history_handler(
    Extension(ctx): Extension<Context>,
    Path(project): Path<Urn>,
    Query(qs): Query<CommitsQueryString>,
) -> impl IntoResponse {
    let CommitsQueryString {
        since,
        until,
        parent,
        page,
        per_page,
        verified,
    } = qs;

    let (sha, fallback_to_head) = match parent {
        Some(commit) => (commit, false),
        None => {
            let info = ctx.project_info(project.to_owned()).await?;

            if let Some(head) = info.head {
                (head.to_string(), true)
            } else {
                return Err(Error::NoHead("project head is not set"));
            }
        }
    };

    let reference = Reference::head(
        Namespace::from(project.to_owned()),
        None,
        One::from_str(&sha).map_err(|_| Error::NotFound)?,
    );

    let commits = browse(reference, ctx.paths.to_owned(), |browser| {
        radicle_source::commits::<PeerId>(browser, None)
    })
    .await?;

    // If a pagination is defined, we do not want to paginate the commits, and we return all of them on the first page.
    let page = page.unwrap_or(0);
    let per_page = if per_page.is_none() && (since.is_some() || until.is_some()) {
        commits.headers.len()
    } else {
        per_page.unwrap_or(30)
    };

    let projects = ctx.projects.read().await;
    let fingerprints = projects.get(&project);
    let headers = commits
        .headers
        .iter()
        .filter(|q| {
            if let (Some(since), Some(until)) = (since, until) {
                q.committer_time.seconds() >= since && q.committer_time.seconds() < until
            } else if let Some(since) = since {
                q.committer_time.seconds() >= since
            } else if let Some(until) = until {
                q.committer_time.seconds() < until
            } else {
                // If neither `since` nor `until` are specified, we include the commit.
                true
            }
        })
        .skip(page * per_page)
        .take(per_page)
        .map(|header| {
            let committer = if verified.unwrap_or_default() {
                let fp = ctx.commit_ssh_fingerprint(&header.sha1.to_string())?;
                if let (Some(fps), Some(fp)) = (fingerprints, fp) {
                    fps.get(&fp).cloned().map(|peer| Committer { peer })
                } else {
                    None
                }
            } else {
                None
            };

            Ok(CommitTeaser {
                header: header.clone(),
                context: CommitContext { committer },
            })
        })
        .collect::<Result<Vec<CommitTeaser>, Error>>()?;

    let response = json!({
        "headers": &headers,
        "stats": &commits.stats,
    });

    if fallback_to_head {
        return Ok::<_, Error>((StatusCode::FOUND, Json(response)));
    }

    Ok::<_, Error>((StatusCode::OK, Json(response)))
}

/// Get project activity for the past year.
/// `GET /projects/:project/activity`
async fn activity_handler(
    Extension(ctx): Extension<Context>,
    Path(project): Path<Urn>,
) -> impl IntoResponse {
    let info = ctx.project_info(project.to_owned()).await?;

    let head = if let Some(head) = info.head {
        head.to_string()
    } else {
        return Err(Error::NoHead("project head is not set"));
    };

    let reference = Reference::head(
        Namespace::from(project.to_owned()),
        None,
        One::from_str(&head).map_err(|_| Error::NotFound)?,
    );

    let current_date = chrono::Utc::now().timestamp();
    let one_year_ago = chrono::Duration::weeks(52);

    let timestamps = browse(reference, ctx.paths.to_owned(), |browser| {
        let activity = browser
            .get()
            .iter()
            .filter_map(|a| {
                let seconds = a.committer.time.seconds();
                if seconds > current_date - one_year_ago.num_seconds() {
                    return Some(seconds);
                }

                None
            })
            .collect::<Vec<i64>>();

        Ok(activity)
    })
    .await?;

    Ok::<_, Error>((StatusCode::OK, Json(json!({ "activity": timestamps }))))
}

/// Get project metadata.
/// `GET /projects/{:project-urn,:project-alias}`
async fn project_alias_or_urn_handler(
    Extension(ctx): Extension<Context>,
    Path(urn_or_alias): Path<String>,
) -> impl IntoResponse {
    let urn = Urn::from_str(&urn_or_alias);
    let urn = if let Ok(urn) = urn {
        urn
    } else {
        let alias = urn_or_alias;
        let mut aliases = ctx.aliases.write().await;
        if !aliases.contains_key(&alias) {
            // If the alias does not exist, rebuild the cache.
            ctx.populate_aliases(&mut aliases).await?;
        }

        aliases.get(&alias).cloned().ok_or(Error::NotFound)?
    };

    let info = ctx.project_info(urn).await?;
    Ok::<_, Error>(Json(info))
}

/// Get project source tree.
/// `GET /projects/:project/tree/:sha/*path`
async fn tree_handler(
    Extension(ctx): Extension<Context>,
    Path((project, sha, path)): Path<(Urn, One, String)>,
) -> impl IntoResponse {
    let path = path.strip_prefix('/').ok_or(Error::NotFound)?.to_string();
    let reference = Reference::head(Namespace::from(project), None, sha);
    let (tree, stats) = browse(reference, ctx.paths, |browser| {
        Ok((
            radicle_source::tree::<PeerId>(browser, None, Some(path))?,
            browser.get_stats()?,
        ))
    })
    .await?;
    let response = json!({
        "path": &tree.path,
        "entries": &tree.entries,
        "info": &tree.info,
        "stats": &stats,
    });

    Ok::<_, Error>(Json(response))
}

/// Get all project remotes.
/// `GET /projects/:project/remotes`
async fn remotes_handler(
    Extension(ctx): Extension<Context>,
    Path(urn): Path<Urn>,
) -> impl IntoResponse {
    let storage = ctx.storage().await?;
    let project = identities::project::get(storage.read_only(), &urn)
        .map_err(Error::Identities)?
        .ok_or(Error::NotFound)?;
    let meta: project::Metadata = project.try_into().map_err(Error::Project)?;
    let response = project::tracked(&meta, storage.read_only())?;

    Ok::<_, Error>(Json(response))
}

/// Get project remote.
/// `GET /projects/:project/remotes/:peer`
async fn remote_handler(
    Extension(ctx): Extension<Context>,
    Path((project, peer_id)): Path<(Urn, PeerId)>,
) -> impl IntoResponse {
    let repo = git2::Repository::open_bare(ctx.paths.git_dir()).map_err(Error::from)?;
    // This is necessary to get any references to show up in the later calls. Go figure.
    let _ = repo.references().map_err(Error::from)?;

    let namespace = project.encode_id();
    let remote = peer_id.default_encoding();
    let prefix = format!(
        "refs/namespaces/{}/refs/remotes/{}/heads/",
        namespace, remote
    );
    let glob = format!("{}*", prefix);
    let refs = repo.references_glob(&glob).unwrap();

    let branches = refs
        .filter_map(|r| {
            let reference = r.ok()?;
            let name = reference.name()?.strip_prefix(&prefix)?;
            let oid = reference.target()?;

            Some((name.to_string(), oid.to_string()))
        })
        .collect::<HashMap<_, _>>();

    let response = json!({ "heads": &branches });

    Ok::<_, Error>(Json(response))
}

#[derive(Deserialize, Default)]
struct BlobQuery {
    highlight: bool,
    theme: Option<String>,
}

/// Get project source file.
/// `GET /projects/:project/blob/:sha/*path?highlight=<bool>`
async fn blob_handler(
    Extension(ctx): Extension<Context>,
    Path((project, sha, path)): Path<(Urn, One, String)>,
    query: Option<Query<BlobQuery>>,
) -> impl IntoResponse {
    let path = path.strip_prefix('/').ok_or(Error::NotFound)?.to_string();
    let Query(query) = query.unwrap_or_default();
    let theme = if query.highlight {
        Some(query.theme.as_deref().unwrap_or(ctx.theme.as_str()))
    } else {
        None
    };
    let reference = Reference::head(Namespace::from(project), None, sha);
    let blob = browse(reference, ctx.paths, |browser| {
        radicle_source::blob::highlighting::blob::<PeerId>(browser, None, path.as_str(), theme)
    })
    .await?;

    Ok::<_, Error>(Json(blob))
}

/// Get project readme.
/// `GET /projects/:project/readme/:sha`
async fn readme_handler(
    Extension(ctx): Extension<Context>,
    Path((project, sha)): Path<(Urn, One)>,
) -> impl IntoResponse {
    let reference = Reference::head(Namespace::from(project), None, sha);
    let paths = &[
        "README",
        "README.md",
        "README.markdown",
        "README.txt",
        "README.rst",
        "Readme.md",
    ];
    let blob = browse(reference, ctx.paths, |browser| {
        for path in paths {
            if let Ok(blob) =
                radicle_source::blob::highlighting::blob::<PeerId>(browser, None, path, None)
            {
                return Ok(blob);
            }
        }
        use radicle_source::surf::file_system::Path;
        Err(radicle_source::Error::PathNotFound(
            Path::try_from("README").unwrap(),
        ))
    })
    .await?;

    Ok::<_, Error>(Json(blob))
}

async fn patch_handler(
    Extension(ctx): Extension<Context>,
    Path((urn, patch_id)): Path<(Urn, ObjectId)>,
) -> impl IntoResponse {
    let repo = git::Repository::new(ctx.paths.git_dir()).map_err(Error::from)?;
    let storage = ctx.storage().await?;
    let project = identities::project::get(storage.as_ref(), &urn)
        .map_err(Error::Identities)?
        .ok_or(Error::NotFound)?;
    let meta: project::Metadata = project.try_into().map_err(Error::Project)?;

    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let store = Store::new(whoami, &ctx.paths, &storage);
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

    Ok::<_, Error>(Json(Cob::new(
        patch_id,
        resolve_revisions(patch, &mut browser, &meta, storage.as_ref(), true),
    )))
}

/// Get project patches list.
/// `GET /projects/:project/patches`
async fn patches_handler(
    Extension(ctx): Extension<Context>,
    Path(urn): Path<Urn>,
) -> impl IntoResponse {
    let storage = ctx.storage().await?;
    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let store = Store::new(whoami, &ctx.paths, &storage);
    let patches = patch::PatchStore::new(&store);
    let all: Vec<_> = patches
        .all(&urn)
        .map_err(Error::Cobs)?
        .into_iter()
        .map(|(id, mut patch)| {
            if let Err(e) = patch
                .resolve(storage.as_ref())
                .map_err(Error::IdentityResolve)
            {
                tracing::warn!("Failed to resolve identities in patch {}: {}", id, e);
            }

            Cob::new(id, patch)
        })
        .collect();

    Ok::<_, Error>(Json(all))
}

/// Get project issues list.
/// `GET /projects/:project/issues`
async fn issues_handler(
    Extension(ctx): Extension<Context>,
    Path(project): Path<Urn>,
) -> impl IntoResponse {
    // TODO: Handle non-existing project.
    let storage = ctx.storage().await?;
    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let store = Store::new(whoami, &ctx.paths, &storage);
    let issues = issue::IssueStore::new(&store);
    let all: Vec<_> = issues
        .all(&project)
        .map_err(Error::Cobs)?
        .into_iter()
        .map(|(id, mut issue)| {
            if let Err(e) = issue
                .resolve(storage.as_ref())
                .map_err(Error::IdentityResolve)
            {
                tracing::warn!("Failed to resolve identities in issue {}: {}", id, e);
            }

            Cob::new(id, issue)
        })
        .collect();

    Ok::<_, Error>(Json(all))
}

/// Get project issue.
/// `GET /projects/:project/issues/:id`
async fn issue_handler(
    Extension(ctx): Extension<Context>,
    Path((project, issue_id)): Path<(Urn, ObjectId)>,
) -> impl IntoResponse {
    // TODO: Handle non-existing project.
    let storage = ctx.storage().await?;
    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let store = Store::new(whoami, &ctx.paths, &storage);
    let issues = issue::IssueStore::new(&store);
    let mut issue = issues
        .get(&project, &issue_id)
        .map_err(Error::from)?
        .ok_or(Error::NotFound)?;
    if let Err(e) = issue
        .resolve(storage.as_ref())
        .map_err(Error::IdentityResolve)
    {
        tracing::warn!("Failed to resolve identities in issue {}: {}", issue_id, e);
    }

    Ok::<_, Error>(Json(Cob::new(issue_id, issue)))
}

async fn browse<T, F>(reference: Reference<Single>, paths: Paths, callback: F) -> Result<T, Error>
where
    F: FnOnce(&mut git::Browser) -> Result<T, radicle_source::Error> + Send,
{
    let namespace = git::namespace::Namespace::try_from(
        reference
            .namespace
            .ok_or(Error::MissingNamespace)?
            .to_string()
            .as_str(),
    )
    .map_err(Error::from)?;

    let revision: git::Rev = match git::Oid::from_str(reference.name.as_str()) {
        Ok(oid) => oid.try_into().map_err(|_| Error::NotFound)?,
        Err(_) => remote_branch(&reference.name, &reference.remote.ok_or(Error::NotFound)?)
            .try_into()
            .map_err(|_| Error::NotFound)?,
    };
    let repo = git::Repository::new(paths.git_dir())?;
    let mut browser = git::Browser::new_with_namespace(&repo, &namespace, revision)?;

    callback(&mut browser).map_err(|err| match &err {
        radicle_source::error::Error::PathNotFound(_) => Error::NotFound,
        _ => Error::from(err),
    })
}

fn remote_branch(branch_name: &str, peer_id: &PeerId) -> git::Branch {
    // NOTE<sebastinez>: We should be able to pass simply a branch name without heads/ and be able to query that later.
    // Needs work on radicle_surf I assume.
    git::Branch::remote(
        &format!("heads/{}", branch_name),
        &peer_id.default_encoding(),
    )
}

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
    commits: Vec<source::Commit>,
    diff: git::Diff,
    stats: Stats,
}

impl Changeset {
    pub fn new(commits: Vec<source::Commit>, diff: git::Diff) -> Self {
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
                    .collect::<Vec<source::Commit>>();

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

#[cfg(test)]
mod routes {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::test_extra::setup;

    const THEME: &str = "base16-ocean.dark";
    const PROJECT_NAME: &str = "nakamoto";
    const COMMIT_MSG: &str = "say hi to bob";
    const COMMIT_FILE_NAME: &str = "HI";
    const COMMIT_FILE_CONTENT: &str = "Hi Bob";
    const COMMIT_README_CONTENT: &str = "This is a readme";
    const PATCH_TITLE: &str = "My first patch";
    const ISSUE_TITLE: &str = "My first issue";

    #[tokio::test]
    async fn test_projects_root_route() {
        let (profile, signer, _, _) = setup::env();
        let ctx = Context::new(profile.paths().to_owned(), signer, THEME.to_string());
        let app = router(ctx);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/projects")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body[0]["name"], PROJECT_NAME);
        assert_eq!(body[1], Value::Null);
    }

    #[tokio::test]
    async fn test_project_route() {
        let (profile, signer, project, _) = setup::env();
        let ctx = Context::new(profile.paths().to_owned(), signer, THEME.to_string());
        let app = router(ctx);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}", PROJECT_NAME))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let alias_body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&alias_body).unwrap();

        assert_eq!(body["name"], "nakamoto");

        let urn = body["urn"].as_str().unwrap();
        assert_eq!(project.urn().to_string(), urn);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}", urn))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let urn_body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        assert_eq!(alias_body, urn_body);
    }

    #[tokio::test]
    async fn test_commits_route() {
        let (profile, signer, project, head) = setup::env();
        let ctx = Context::new(profile.paths().to_owned(), signer, THEME.to_string());
        let app = router(ctx);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}/commits", project.urn()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FOUND);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body["headers"][0]["header"]["summary"], COMMIT_MSG);
        assert_eq!(body["headers"][0]["header"]["sha1"], head.to_string());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}/commits/{}", project.urn(), head))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body["header"]["summary"], COMMIT_MSG);
    }

    #[tokio::test]
    async fn test_tree_route() {
        let (profile, signer, project, head) = setup::env();
        let ctx = Context::new(profile.paths().to_owned(), signer, THEME.to_string());
        let app = router(ctx);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}/tree/{}/", project.urn(), head))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body["entries"][0]["path"], COMMIT_FILE_NAME);
    }

    #[tokio::test]
    async fn test_blob_route() {
        let (profile, signer, project, head) = setup::env();
        let ctx = Context::new(profile.paths().to_owned(), signer, THEME.to_string());
        let app = router(ctx);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/projects/{}/blob/{}/{}",
                        project.urn(),
                        head,
                        COMMIT_FILE_NAME
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body["path"], COMMIT_FILE_NAME);
        assert_eq!(body["content"], COMMIT_FILE_CONTENT);
    }

    #[tokio::test]
    async fn test_readme_route() {
        let (profile, signer, project, head) = setup::env();
        let ctx = Context::new(profile.paths().to_owned(), signer, THEME.to_string());
        let app = router(ctx);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}/readme/{}", project.urn(), head))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body["path"], "README");
        assert_eq!(body["content"], COMMIT_README_CONTENT);
    }

    #[tokio::test]
    async fn test_patches_route() {
        let (profile, signer, project, _head) = setup::env();
        let ctx = Context::new(profile.paths().to_owned(), signer, THEME.to_string());
        let app = router(ctx);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}/patches", project.urn()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body[0]["title"], PATCH_TITLE);
        assert_eq!(body[1], Value::Null);

        let patch_id = body[0]["id"].as_str().unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}/patches/{}", project.urn(), patch_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_issues_route() {
        let (profile, signer, project, _head) = setup::env();
        let ctx = Context::new(profile.paths().to_owned(), signer, THEME.to_string());
        let app = router(ctx);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}/issues", project.urn()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body[0]["title"], ISSUE_TITLE);
        assert_eq!(body[1], Value::Null);

        let issue_id = body[0]["id"].as_str().unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/projects/{}/issues/{}", project.urn(), issue_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
