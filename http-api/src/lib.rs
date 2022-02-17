#![allow(clippy::if_same_then_else)]
mod error;
mod project;

use std::collections::HashMap;
use std::convert::TryFrom as _;
use std::convert::TryInto as _;
use std::net;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use radicle_source::commit::Header;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;
use warp::hyper::StatusCode;
use warp::reply;
use warp::reply::Json;
use warp::{self, filters::BoxedFilter, path, query, Filter, Rejection, Reply};

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

#[derive(Serialize, Deserialize, Clone)]
struct CommitsQueryString {
    parent: Option<String>,
    since: Option<i64>,
    until: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Context {
    paths: Paths,
    theme: String,
    aliases: Arc<RwLock<HashMap<String, Urn>>>,
}

impl Context {
    /// Populates alias map with unique projects' names and their urns
    async fn populate_aliases(&self, map: &mut HashMap<String, Urn>) -> Result<(), Error> {
        use radicle_daemon::git::identities::SomeIdentity::Project;

        let storage = ReadOnly::open(&self.paths).expect("failed to open storage");
        let identities = identities::any::list(&storage)?;

        for identity in identities.flatten() {
            if let Project(project) = identity {
                let urn = project.urn();
                let name = (&project.payload().subject.name).to_string();

                if let std::collections::hash_map::Entry::Vacant(e) = map.entry(name.clone()) {
                    e.insert(urn);
                }
            }
        }

        Ok(())
    }
}

/// Run the HTTP API.
pub async fn run(options: Options) {
    let paths = Paths::from_root(options.root).unwrap();
    let storage = ReadOnly::open(&paths).expect("failed to read storage paths");
    let peer_id = storage.peer_id().to_owned();

    let ctx = Context {
        paths,
        aliases: Default::default(),
        theme: options.theme,
    };

    let v1 = warp::path("v1");

    let peer = path("peer")
        .and(warp::get().and(path::end()))
        .and_then(move || peer_handler(peer_id));

    let projects = path("projects").and(filters(ctx.clone()));

    let delegates = path("delegates").and(
        warp::get()
            .map(move || ctx.clone())
            .and(path::param::<Urn>())
            .and(path("projects"))
            .and(path::end())
            .and_then(delegates_projects_handler),
    );

    let routes = path::end()
        .and_then(root_handler)
        .or(v1.and(peer))
        .or(v1.and(projects))
        .or(v1.and(delegates))
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
        .or(commit_filter(ctx.clone()))
        .or(history_filter(ctx.clone()))
        .or(project_urn_filter(ctx.clone()))
        .or(project_alias_filter(ctx.clone()))
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
            },
            {
                "href": "/v1/delegates/:urn/projects",
                "rel": "projects",
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

async fn remotes_handler(ctx: Context, urn: Urn) -> Result<impl Reply, Rejection> {
    let storage = ReadOnly::open(&ctx.paths).map_err(Error::from)?;
    let project = identities::project::get(&storage, &urn)
        .map_err(Error::Identities)?
        .ok_or(Error::NotFound)?;
    let meta: project::Metadata = project.try_into()?;
    let tracked = tracking::tracked(&storage, Some(&urn)).map_err(|_| Error::NotFound)?;
    let result = tracked
        .collect::<Result<Vec<_>, _>>()
        .map_err(Error::from)?;
    let response = result
        .into_iter()
        .filter_map(|t| t.peer_id())
        .map(|peer| -> Result<serde_json::Value, Rejection> {
            if let Ok(delegate_urn) = Urn::try_from(Reference::rad_self(
                Namespace::from(urn.clone()),
                Some(peer),
            )) {
                if let Ok(Some(person)) = identities::person::get(&storage, &delegate_urn) {
                    let delegate = meta.delegates.iter().any(|d| d.contains(&peer));

                    return Ok(json!({
                        "id": peer,
                        "name": person.subject().name.to_string(),
                        "delegate": delegate
                    }));
                }
            }
            Ok(json!({ "id": peer }))
        })
        .collect::<Result<Vec<_>, _>>()?;

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

async fn history_handler(
    ctx: Context,
    project: Urn,
    qs: CommitsQueryString,
) -> Result<impl Reply, Rejection> {
    let CommitsQueryString {
        since,
        until,
        parent,
    } = qs;

    let (sha, fallback_to_head) = match parent {
        Some(commit) => (commit, false),
        None => {
            let meta = project_info(project.to_owned(), ctx.paths.to_owned())?;
            (meta.head.to_string(), true)
        }
    };

    let reference = Reference::head(
        Namespace::from(project),
        None,
        One::from_str(&sha).map_err(|_| Error::NotFound)?,
    );

    let commits = browse(reference, ctx.paths, |browser| {
        let mut result = radicle_source::commits::<PeerId>(browser, None)?;
        let headers: Vec<Header> = result
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
            .cloned()
            .collect();

        result.headers = headers;
        // Since the headers filtering can alter the amount of commits we have to recalculate it here.
        result.stats.commits = result.headers.len();

        Ok(result)
    })
    .await?;
    let response = json!({
        "headers": &commits.headers,
        "stats": &commits.stats,
    });

    if fallback_to_head {
        return Ok(warp::reply::with_status(
            warp::reply::json(&response),
            StatusCode::FOUND,
        ));
    }

    Ok(warp::reply::with_status(
        warp::reply::json(&response),
        StatusCode::OK,
    ))
}

async fn commit_handler(ctx: Context, project: Urn, sha: One) -> Result<impl Reply, Rejection> {
    let reference = Reference::head(Namespace::from(project), None, sha.to_owned());
    let commit = browse(reference, ctx.paths, |browser| {
        let oid = browser.oid(&sha)?;
        radicle_source::commit(browser, oid)
    })
    .await?;
    let response = json!(&commit);

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

    let storage = ReadOnly::open(&ctx.paths).map_err(Error::from)?;
    let repo = git::Repository::new(&ctx.paths.git_dir().to_owned()).map_err(Error::from)?;
    let projects = identities::any::list(&storage)
        .map_err(Error::from)?
        .filter_map(|res| {
            res.map(|id| match id {
                SomeIdentity::Project(project) => {
                    let meta: project::Metadata = project.try_into().ok()?;
                    let head = get_head_commit(&repo, &meta.urn, &meta.default_branch).ok()?;

                    Some(Info {
                        meta,
                        head: head.id,
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

async fn project_urn_handler(ctx: Context, urn: Urn) -> Result<Json, Rejection> {
    let info = project_info(urn, ctx.paths)?;

    Ok(warp::reply::json(&info))
}

async fn project_alias_handler(ctx: Context, name: String) -> Result<Json, Rejection> {
    let mut aliases = ctx.aliases.write().await;
    if !aliases.contains_key(&name) {
        // If the alias does not exist, rebuild the cache.
        ctx.populate_aliases(&mut aliases).await?;
    }
    let urn = aliases
        .get(&name)
        .cloned()
        .ok_or_else(warp::reject::not_found)?;

    project_urn_handler(ctx.clone(), urn).await
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

/// List all projects that delegate is a part of.
async fn delegates_projects_handler(ctx: Context, delegate: Urn) -> Result<impl Reply, Rejection> {
    use radicle_daemon::git::identities::SomeIdentity;

    let storage = ReadOnly::open(&ctx.paths).map_err(Error::from)?;
    let repo = git::Repository::new(&ctx.paths.git_dir().to_owned()).map_err(Error::from)?;
    let projects = identities::any::list(&storage)
        .map_err(Error::from)?
        .filter_map(|res| {
            res.map(|id| match id {
                SomeIdentity::Project(project) => {
                    use either::Either;

                    if !project.delegations().iter().any(|d| match d {
                        Either::Right(indirect) => indirect.urn() == delegate,
                        Either::Left(_) => false,
                    }) {
                        return None;
                    }

                    let meta: project::Metadata = project.try_into().ok()?;
                    let head = get_head_commit(&repo, &meta.urn, &meta.default_branch).ok()?;

                    Some(Info {
                        meta,
                        head: head.id,
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
    let storage = ReadOnly::open(&paths)?;
    let project = identities::project::get(&storage, &urn)?.ok_or(Error::NotFound)?;
    let meta: project::Metadata = project.try_into()?;
    let head = get_head_commit(&repo, &urn, &meta.default_branch)?;

    Ok(Info {
        head: head.id,
        meta,
    })
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
