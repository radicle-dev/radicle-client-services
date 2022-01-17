#![allow(clippy::if_same_then_else)]
mod error;
mod project;

use std::collections::HashMap;
use std::convert::TryFrom as _;
use std::convert::TryInto as _;
use std::net;
use std::path::PathBuf;

use serde_json::json;
use warp::hyper::StatusCode;
use warp::reply;
use warp::reply::Json;
use warp::{self, filters::BoxedFilter, path, Filter, Rejection, Reply};

use radicle_daemon::librad::git::identities;
use radicle_daemon::librad::git::storage::read::ReadOnly;
use radicle_daemon::librad::git::tracking;
use radicle_daemon::librad::git::types::{One, Reference, Single};
use radicle_daemon::{git::types::Namespace, Paths, PeerId, Urn};
use radicle_source::surf::file_system::Path;
use radicle_source::surf::vcs::git;
use radicle_source::surf::vcs::git::RepositoryRef;

use crate::project::Info;

use error::Error;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    pub listen: net::SocketAddr,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub theme: String,
}

#[derive(Debug, Clone)]
pub struct Context {
    paths: Paths,
    theme: String,
}

/// Run the HTTP API.
pub async fn run(options: Options) {
    let paths = Paths::from_root(options.root).unwrap();
    let storage = ReadOnly::open(&paths).expect("failed to read storage paths");
    let peer_id = storage.peer_id().to_owned();

    let ctx = Context {
        paths,
        theme: options.theme,
    };

    let v1 = warp::path("v1");

    let peer = path("peer")
        .and(warp::get().and(path::end()))
        .and_then(move || peer_handler(peer_id));

    let projects = path("projects").and(filters(ctx));

    let routes = path::end()
        .and_then(root_handler)
        .or(v1.and(peer))
        .or(v1.and(projects))
        .recover(recover)
        .with(warp::cors().allow_any_origin())
        .with(warp::log("http::api"));

    let server = warp::serve(routes);

    if let (Some(cert), Some(key)) = (options.tls_cert, options.tls_key) {
        server
            .tls()
            .cert_path(cert)
            .key_path(key)
            .run(options.listen)
            .await
    } else {
        server.run(options.listen).await
    }
}

/// Return the peer id for the node identity.
/// `GET /v1/peer`
async fn peer_handler(peer_id: PeerId) -> Result<impl warp::Reply, warp::Rejection> {
    let response = json!({
        "id": peer_id.to_string(),
    });
    Ok(warp::reply::json(&response))
}

async fn recover(err: Rejection) -> Result<impl Reply, std::convert::Infallible> {
    let status = if err.is_not_found() {
        StatusCode::NOT_FOUND
    } else if let Some(Error::NotFound) = err.find::<Error>() {
        StatusCode::NOT_FOUND
    } else {
        // Log the non-standard errors.
        tracing::error!("Error: {:?}", err);

        StatusCode::BAD_REQUEST
    };
    let res = reply::json(&json!({
        "error": status.canonical_reason(),
        "code": status.as_u16()
    }));

    Ok(reply::with_header(
        reply::with_status(res, status),
        "Content-Type",
        "application/json",
    ))
}

/// Combination of all source filters.
fn filters(ctx: Context) -> BoxedFilter<(impl Reply,)> {
    project_root_filter(ctx.clone())
        .or(commits_filter(ctx.clone()))
        .or(project_filter(ctx.clone()))
        .or(tree_filter(ctx.clone()))
        .or(remotes_filter(ctx.clone()))
        .or(remote_filter(ctx.clone()))
        .or(blob_filter(ctx.clone()))
        .or(readme_filter(ctx))
        .boxed()
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

/// `GET /:project/commits/:sha`
fn commits_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("commits"))
        .and(path::param::<One>())
        .and(path::end())
        .and_then(commits_handler)
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

/// `GET /:project`
fn project_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path::end())
        .and_then(project_handler)
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

async fn root_handler() -> Result<impl Reply, Rejection> {
    let response = json!({
        "message": "Welcome!",
        "service": "radicle-http-api",
        "version": VERSION,
        "path": "/",
        "links": [
            {
                "href": "/v1/projects",
                "rel": "projects",
                "type": "GET"
            },
            {
                "href": "/v1/peer",
                "rel": "peer",
                "type": "GET"
            }
        ]
    });
    Ok(warp::reply::json(&response))
}

async fn blob_handler(
    ctx: Context,
    project: Urn,
    sha: One,
    highlight: bool,
    path: warp::filters::path::Tail,
) -> Result<impl Reply, Rejection> {
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

    Ok(warp::reply::json(&blob))
}

async fn remotes_handler(ctx: Context, project: Urn) -> Result<impl Reply, Rejection> {
    let storage = ReadOnly::open(&ctx.paths).map_err(Error::from)?;
    let tracked = tracking::tracked(&storage, Some(&project)).map_err(|_| Error::NotFound)?;
    let result = tracked
        .collect::<Result<Vec<_>, _>>()
        .map_err(Error::from)?;
    let response = result
        .into_iter()
        .filter_map(|t| t.peer_id())
        .collect::<Vec<_>>();

    Ok(warp::reply::json(&response))
}

async fn remote_handler(
    ctx: Context,
    project: Urn,
    peer_id: PeerId,
) -> Result<impl Reply, Rejection> {
    let repo = git2::Repository::open_bare(ctx.paths.git_dir()).map_err(Error::from)?;
    // This is necessary to get any references to show up in the later calls. Go figure.
    let _ = repo.references().map_err(Error::from)?;

    let namespace = project.encode_id();
    let remote = peer_id.default_encoding();

    repo.set_namespace(&namespace).map_err(Error::from)?;

    let branches = RepositoryRef::from(&repo)
        .list_branches(git::RefScope::Remote { name: Some(remote) })
        .map_err(Error::from)?
        .iter()
        .filter(|branch| !branch.name.to_string().starts_with("rad/"))
        .map(|branch| {
            let reflike = branch
                .name
                .name()
                .try_into()
                .map_err(|_| Error::BranchName)?;
            let reference = Reference::head(Namespace::from(project.clone()), peer_id, reflike);
            let oid = reference.oid(&repo)?;

            Ok::<_, Error>((branch.name.to_string(), oid.to_string()))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;

    let response = json!({ "heads": &branches });

    Ok(warp::reply::json(&response))
}

async fn commits_handler(ctx: Context, project: Urn, sha: One) -> Result<impl Reply, Rejection> {
    let reference = Reference::head(Namespace::from(project), None, sha);
    let commits = browse(reference, ctx.paths, |browser| {
        radicle_source::commits::<PeerId>(browser, None)
    })
    .await?;
    let response = json!({
        "headers": &commits.headers,
        "stats": &commits.stats,
    });

    Ok(warp::reply::json(&response))
}

async fn readme_handler(ctx: Context, project: Urn, sha: One) -> Result<impl Reply, Rejection> {
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
        Err(radicle_source::Error::PathNotFound(
            Path::try_from("README").unwrap(),
        ))
    })
    .await?;

    Ok(warp::reply::json(&blob))
}

/// List all projects
async fn project_root_handler(ctx: Context) -> Result<Json, Rejection> {
    use radicle_daemon::git::identities::SomeIdentity;
    #[derive(serde::Serialize)]
    struct Project {
        name: String,
        description: String,
        head: String,
        urn: String,
        delegates: Vec<PeerId>,
    }

    let storage = ReadOnly::open(&ctx.paths).map_err(Error::from)?;
    let repo = git::Repository::new(&ctx.paths.git_dir().to_owned()).map_err(Error::from)?;
    let projects = identities::any::list(&storage)
        .map_err(Error::from)?
        .filter_map(|res| {
            res.map(|id| match id {
                SomeIdentity::Project(project) => {
                    let urn = &project.urn();
                    let meta: project::Metadata = project.try_into().ok()?;
                    let project::Metadata {
                        name,
                        description,
                        default_branch,
                        maintainers: _,
                        delegates,
                    } = meta;
                    let head = get_head_commit(&repo, urn, &default_branch).ok()?;

                    Some(Project {
                        name,
                        description,
                        urn: urn.to_string(),
                        head: head.id.to_string(),
                        delegates,
                    })
                }
                _ => None,
            })
            .transpose()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Error::from)?;

    Ok(warp::reply::json(&projects))
}

async fn project_handler(ctx: Context, urn: Urn) -> Result<Json, Rejection> {
    let info = project_info(urn, ctx.paths)?;

    Ok(warp::reply::json(&info))
}

/// Fetch a [`radicle_source::Tree`].
async fn tree_handler(
    ctx: Context,
    project: Urn,
    sha: One,
    path: warp::filters::path::Tail,
) -> Result<impl Reply, Rejection> {
    let reference = Reference::head(Namespace::from(project), None, sha);
    let (tree, stats) = browse(reference, ctx.paths, |browser| {
        Ok((
            radicle_source::tree::<PeerId>(browser, None, Some(path.as_str().to_owned()))?,
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

    Ok(warp::reply::json(&response))
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
        Err(_) => remote_branch(
            &reference.name.to_string(),
            &reference.remote.ok_or(Error::NotFound)?,
        )
        .try_into()
        .map_err(|_| Error::NotFound)?,
    };
    let repo = git::Repository::new(paths.git_dir().to_owned())?;
    let mut browser = git::Browser::new_with_namespace(&repo, &namespace, revision)?;

    Ok(callback(&mut browser)?)
}

fn project_info(urn: Urn, paths: Paths) -> Result<Info, Error> {
    let repo = git::Repository::new(paths.git_dir().to_owned())?;
    let meta = get_project_metadata(&urn, &paths)?;
    let head = get_head_commit(&repo, &urn, &meta.default_branch)?;
    Ok(Info {
        meta,
        head: head.id.to_string(),
    })
}

fn get_project_metadata(urn: &Urn, paths: &Paths) -> Result<project::Metadata, Error> {
    let storage = ReadOnly::open(paths)?;
    let project = identities::project::get(&storage, urn)?.ok_or(Error::NotFound)?;
    let meta: project::Metadata = project.try_into()?;

    Ok(meta)
}

fn get_head_commit(
    repo: &git::Repository,
    urn: &Urn,
    default_branch: &str,
) -> Result<git::Commit, Error> {
    let namespace = git::namespace::Namespace::try_from(urn.encode_id().as_str())?;
    let browser =
        git::Browser::new_with_namespace(repo, &namespace, git::Branch::local(default_branch))
            .map_err(|_| Error::MissingLocalState)?;
    let history = browser.get();

    Ok(history.first().to_owned())
}

fn remote_branch(branch_name: &str, peer_id: &PeerId) -> git::Branch {
    // NOTE<sebastinez>: We should be able to pass simply a branch name without heads/ and be able to query that later.
    // Needs work on radicle_surf I assume.
    git::Branch::remote(
        &format!("heads/{}", branch_name),
        &peer_id.default_encoding(),
    )
}
