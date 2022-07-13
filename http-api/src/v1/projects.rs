use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::str::FromStr;

use axum::extract::Query;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use hyper::StatusCode;
use serde_json::json;

use librad::git::identities::{self, SomeIdentity};
use librad::git::types::{Namespace, One, Reference};
use librad::git::Urn;
use librad::PeerId;

use radicle_common::{cobs, person};

use crate::axum_extra::Path;
// TODO: add these 3 filters
//use crate::issues::{issue_filter, issues_filter};
//use crate::patches::patches_filter;
use crate::commit::{Commit, CommitContext, CommitTeaser, CommitsQueryString, Committer};
use crate::project::{self, Info};
use crate::{browse, get_head_commit, Context, Error};

/*
/// Combination of all project filters.
fn project_filters(ctx: Context) -> BoxedFilter<(impl Reply,)> {
    project_root_filter(ctx.clone())
        .or(commit_filter(ctx.clone()))
        .or(history_filter(ctx.clone()))
        .or(project_urn_filter(ctx.clone()))
        .or(project_alias_filter(ctx.clone()))
        .or(tree_filter(ctx.clone()))
        .or(remotes_filter(ctx.clone()))
        .or(remote_filter(ctx.clone()))
        .or(blob_filter(ctx.clone()))
        .or(readme_filter(ctx.clone()))
        .or(patches_filter(ctx.clone()))
        .or(issue_filter(ctx.clone()))
        .or(issues_filter(ctx))
        .boxed()
}

/// `GET /`
fn project_root_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::end())
        .and_then(project_root_handler)
        .boxed()
}

/// `GET /:project/commits/:sha`
fn commit_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("commits"))
        .and(path::param::<One>())
        .and(path::end())
        .and_then(commit_handler)
}

/// `GET /:project/commits?from=<sha>`
fn history_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("commits"))
        .and(query::<CommitsQueryString>())
        .and(path::end())
        .and_then(history_handler)
}

/// `GET /:project-urn`
fn project_urn_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path::end())
        .and_then(project_urn_handler)
        .boxed()
}

/// `GET /:project-alias`
fn project_alias_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<String>())
        .and(path::end())
        .and_then(project_alias_handler)
        .boxed()
}

/// `GET /:project/tree/:prefix`
fn tree_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("tree"))
        .and(path::param::<One>())
        .and(path::tail())
        .and_then(tree_handler)
}

/// `GET /:project/remotes`
fn remotes_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("remotes"))
        .and(path::end())
        .and_then(remotes_handler)
}

/// `GET /:project/remotes/:peer`
fn remote_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("remotes"))
        .and(path::param::<PeerId>())
        .and(path::end())
        .and_then(remote_handler)
}

/// `GET /:project/blob/:sha/:path`
fn blob_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    #[derive(serde::Deserialize)]
    struct Query {
        highlight: bool,
    }

    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("blob"))
        .and(path::param::<One>())
        .and(warp::query().map(|q: Query| q.highlight))
        .and(path::tail())
        .and_then(blob_handler)
}

/// `GET /:project/readme/:sha`
fn readme_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("readme"))
        .and(path::param::<One>())
        .and(path::end())
        .and_then(readme_handler)
}
*/

pub fn router(ctx: Context) -> Router {
    Router::new()
        .route("/projects", get(project_root_handler))
        .route("/projects/:project", get(project_alias_or_urn_handler))
        //.route("/projects/:alias", get(project_alias_handler))
        //.route("/projects/:project", get(project_urn_handler))
        .route("/projects/:project/commits", get(history_handler))
        .route("/projects/:project/commits/:sha", get(commit_handler))
        .route("/projects/:project/tree/:prefix/*path", get(tree_handler))
        .route("/projects/:project/remotes", get(remotes_handler))
        .route("/projects/:project/remotes/:peer", get(remote_handler))
        .route("/projects/:project/blob/:sha/*path", get(blob_handler))
        .route("/projects/:project/readme/:sha", get(readme_handler))
        .layer(Extension(ctx))
}

/// List all projects.
/// `GET /projects`
async fn project_root_handler(Extension(ctx): Extension<Context>) -> impl IntoResponse {
    let storage = ctx.storage().await?;
    let repo = git2::Repository::open_bare(&ctx.paths.git_dir()).map_err(Error::from)?;
    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let cobs = cobs::Store::new(whoami, &ctx.paths, &storage).map_err(Error::from)?;
    let issues = cobs.issues();
    let patches = cobs.patches();
    let projects = identities::any::list(storage.read_only())
        .map_err(Error::from)?
        .filter_map(|res| {
            res.map(|id| match id {
                SomeIdentity::Project(project) => {
                    let meta: project::Metadata = project.try_into().ok()?;
                    let head =
                        get_head_commit(&repo, &meta.urn, &meta.default_branch, &meta.delegates)
                            .map(|h| h.id)
                            .ok();

                    let issues = issues.count(&meta.urn).map_err(Error::Issues).ok()?;
                    let patches = patches.count(&meta.urn).map_err(Error::Patches).ok()?;

                    Some(Info {
                        meta,
                        head,
                        issues,
                        patches,
                    })
                }
                _ => None,
            })
            .transpose()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Error::from)?;

    Ok::<_, Error>(Json(projects))
}

/// TODO: Add description.
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

/// TODO: Add description.
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
                return Err(Error::NoHead("project head is not set").into());
            }
        }
    };

    let reference = Reference::head(
        Namespace::from(project.to_owned()),
        None,
        One::from_str(&sha).map_err(|_| Error::NotFound)?,
    );

    let mut commits = browse(reference, ctx.paths.to_owned(), |browser| {
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

    // Since the headers filtering can alter the amount of commits we have to recalculate it here.
    commits.stats.commits = headers.len();

    let response = json!({
        "headers": &headers,
        "stats": &commits.stats,
    });

    if fallback_to_head {
        return Ok::<_, Error>((StatusCode::FOUND, Json(response)));
    }

    Ok::<_, Error>((StatusCode::OK, Json(response)))
}

/// TODO: Add description.
/// `GET /projects/{:project-urn,:project-alias}`
async fn project_alias_or_urn_handler(
    Extension(ctx): Extension<Context>,
    Path(urn_or_alias): Path<String>,
) -> impl IntoResponse {
    let urn = Urn::from_str(&urn_or_alias);
    if let Ok(urn) = urn {
        project_urn_handler(ctx, urn).await
    } else {
        let alias = urn_or_alias;
        project_alias_handler(ctx, alias).await
    }
}

/// TODO: Add description.
/// `GET /projects/:project-urn`
async fn project_urn_handler(ctx: Context, urn: Urn) -> Result<Response, Error> {
    let info = ctx.project_info(urn).await?;

    Ok::<_, Error>(Json(info).into_response())
}

/// TODO: Add description.
/// `GET /projects/:project-alias`
async fn project_alias_handler(ctx: Context, alias: String) -> Result<Response, Error> {
    let mut aliases = ctx.aliases.write().await;
    if !aliases.contains_key(&alias) {
        // If the alias does not exist, rebuild the cache.
        ctx.populate_aliases(&mut aliases).await?;
    }
    let urn = aliases
        .get(&alias)
        .cloned()
        .ok_or_else(|| Error::NotFound)?;

    project_urn_handler(ctx.clone(), urn).await
}

/// Fetch a [`radicle_source::Tree`].
/// `GET /projects/:project/tree/:prefix/*path`
async fn tree_handler(
    Extension(ctx): Extension<Context>,
    Path((project, sha, path)): Path<(Urn, One, String)>,
) -> impl IntoResponse {
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

/// TODO: Add description.
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

/// TODO: Add description.
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

/// TODO: Add description.
/// `GET /projects/:project/blob/:sha/*path?highlight=<bool>`
async fn blob_handler(
    Extension(ctx): Extension<Context>,
    Path((project, sha, path)): Path<(Urn, One, String)>,
    Query(highlight): Query<bool>,
) -> impl IntoResponse {
    let theme = if highlight {
        Some(ctx.theme.as_str())
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

/// TODO: Add description.
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