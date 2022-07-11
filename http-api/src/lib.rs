#![allow(clippy::if_same_then_else)]
mod auth;
mod commit;
mod custom_axum;
mod error;
mod issues;
mod patches;
mod project;
mod v1;

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::convert::TryInto as _;
use std::env;
use std::iter::repeat_with;
use std::net;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ethers_core::utils::hex;
use hyper::{
    http::{Request, Response},
    Body, StatusCode,
};
use siwe::Message;
use tokio::sync::RwLock;
use tracing::Span;
//use warp::reply::Json;
//use warp::{self, filters::BoxedFilter, host::Authority, path, query, Filter, Rejection, Reply};

use axum::{
    body::BoxBody,
    http::{
        header::{AUTHORIZATION, CONTENT_TYPE},
        Method,
    },
    response::{IntoResponse, Json},
    routing::get,
    Extension, Router,
};
use axum_server::tls_rustls::RustlsConfig;
use serde_json::{json, Value};
use tower_http::cors::{self, CorsLayer};
use tower_http::trace::TraceLayer;

use librad::crypto::BoxedSigner;
use librad::git::identities;
use librad::git::identities::SomeIdentity;
use librad::git::storage;
use librad::git::storage::pool::{InitError, Initialised};
use librad::git::storage::Pool;
use librad::git::storage::Storage;
use librad::git::types::{One, Reference, Single};
use librad::{git::types::Namespace, git::Urn, paths::Paths, PeerId};

use radicle_common::{cobs, keys, person};
use radicle_source::surf::file_system::Path;
use radicle_source::surf::vcs::git;

use crate::auth::{AuthRequest, AuthState, Session};
use crate::project::{Info, PeerInfo};

use commit::{Commit, CommitContext, CommitTeaser, CommitsQueryString, Committer};
use error::Error;
//use issues::{issue_filter, issues_filter};
//use patches::patches_filter;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const POPULATE_FINGERPRINTS_INTERVAL: time::Duration = time::Duration::from_secs(20);
pub const CLEANUP_SESSIONS_INTERVAL: time::Duration = time::Duration::from_secs(60);
pub const STORAGE_POOL_SIZE: usize = 3;

#[derive(Debug, Clone)]
pub struct Options {
    pub root: Option<PathBuf>,
    pub passphrase: Option<String>,
    pub listen: net::SocketAddr,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub theme: String,
}

/// SSH Key fingerprint.
type Fingerprint = String;
/// Mapping between fingerprints and users.
type Fingerprints = HashMap<Fingerprint, PeerInfo>;
/// Identifier for sessions
type SessionId = String;

#[derive(Clone)]
pub struct Context {
    paths: Paths,
    theme: String,
    pool: Pool<Storage>,
    peer_id: PeerId,
    aliases: Arc<RwLock<HashMap<String, Urn>>>,
    projects: Arc<RwLock<HashMap<Urn, Fingerprints>>>,
    sessions: Arc<RwLock<HashMap<SessionId, AuthState>>>,
}

impl Context {
    fn new(paths: Paths, signer: BoxedSigner, theme: String) -> Self {
        let peer_id = signer.peer_id();
        let pool = storage::Pool::new(
            storage::pool::ReadWriteConfig::new(paths.clone(), signer, Initialised::no()),
            STORAGE_POOL_SIZE,
        );

        Self {
            paths,
            pool,
            theme,
            peer_id,
            aliases: Default::default(),
            projects: Default::default(),
            sessions: Default::default(),
        }
    }

    async fn storage(&self) -> Result<deadpool::managed::Object<Storage, InitError>, Error> {
        self.pool
            .get()
            .await
            .map_err(|e| Error::Pool(e.to_string()))
    }

    /// Populates alias map with unique projects' names and their urns
    async fn populate_aliases(&self, map: &mut HashMap<String, Urn>) -> Result<(), Error> {
        use librad::git::identities::SomeIdentity::Project;

        let storage = self.storage().await?;
        let identities = identities::any::list(storage.read_only())?;

        for identity in identities.flatten() {
            if let Project(project) = identity {
                let urn = project.urn();
                let name = (&project.payload().subject.name).to_string();

                if let Entry::Vacant(e) = map.entry(name.clone()) {
                    e.insert(urn);
                }
            }
        }

        Ok(())
    }

    fn cleanup_sessions(&self, map: &mut HashMap<SessionId, AuthState>) -> Result<(), Error> {
        let mut to_remove: Vec<SessionId> = Vec::new();

        for (key, value) in map.iter() {
            let current_time = Utc::now();
            match value {
                AuthState::Authorized(auth) => {
                    if let Some(exp_time) = auth.expiration_time {
                        if current_time >= exp_time {
                            to_remove.push(key.clone());
                        }
                    }
                }
                AuthState::Unauthorized {
                    expiration_time, ..
                } => {
                    if current_time >= *expiration_time {
                        to_remove.push(key.clone())
                    }
                }
            }
        }

        for key in to_remove {
            map.remove(&key);
        }

        Ok(())
    }

    /// Populate a map between SSH fingerprints and their peer identities
    async fn populate_fingerprints(
        &self,
        map: &mut HashMap<Urn, Fingerprints>,
    ) -> Result<(), Error> {
        let storage = self.storage().await?;
        let identities = identities::any::list(storage.read_only())?;

        for identity in identities.flatten() {
            if let SomeIdentity::Project(project) = identity {
                let meta = project::Metadata::try_from(project)?;
                let fingerprints = map.entry(meta.urn.clone()).or_default();
                let tracked = project::tracked(&meta, storage.read_only())?;

                for peer in tracked {
                    let fp = keys::to_ssh_fingerprint(&peer.id).expect("Conversion cannot fail");
                    fingerprints.insert(fp, peer);
                }
            }
        }

        Ok(())
    }

    /// From a commit hash, return the signer's fingerprint, if any.
    fn commit_ssh_fingerprint(&self, sha1: &str) -> Result<Option<String>, Error> {
        radicle_common::git::commit_ssh_fingerprint(self.paths.git_dir(), sha1)
            .map_err(|e| Error::Io("failed to get commit's ssh fingerprint", e))
    }

    async fn project_info(&self, urn: Urn) -> Result<Info, Error> {
        let storage = self.storage().await?;
        let project = identities::project::get(&*storage, &urn)?.ok_or(Error::NotFound)?;
        let meta: project::Metadata = project.try_into()?;

        let repo = git2::Repository::open_bare(self.paths.git_dir())?;
        let head = get_head_commit(&repo, &urn, &meta.default_branch, &meta.delegates)
            .map(|h| h.id)
            .ok();

        let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
        let cobs = cobs::Store::new(whoami, &self.paths, &storage)?;
        let issues = cobs.issues();
        let issues = issues.count(&urn).map_err(Error::Issues)?;

        let patches = cobs.patches();
        let patches = patches.count(&urn).map_err(Error::Patches)?;

        Ok(Info {
            head,
            meta,
            issues,
            patches,
        })
    }
}

/// Run the HTTP API.
pub async fn run(options: Options) -> anyhow::Result<()> {
    let (_, profile, signer) = shared::profile(options.root, options.passphrase)?;
    let paths = profile.paths();
    let ctx = Context::new(paths.clone(), signer, options.theme);
    let peer_id = ctx.peer_id;

    // Populate fingerprints
    tokio::spawn(populate_fingerprints_job(
        ctx.clone(),
        POPULATE_FINGERPRINTS_INTERVAL,
    ));
    // Cleanup sessions
    tokio::spawn(cleanup_sessions_job(ctx.clone(), CLEANUP_SESSIONS_INTERVAL));

    let root_router = Router::new()
        .route("/", get(root_handler))
        .layer(Extension(peer_id));

    let app = Router::new()
        .merge(root_router)
        .merge(v1::router(ctx.clone()))
        .layer(
            CorsLayer::new()
                .allow_origin(cors::Any)
                .allow_methods([Method::GET, Method::POST, Method::PUT])
                .allow_headers([CONTENT_TYPE, AUTHORIZATION]),
        )
        .layer(
            TraceLayer::new_for_http()
                .on_request(|request: &Request<Body>, _span: &Span| {
                    tracing::info!("{} {}", request.method(), request.uri().path())
                })
                .on_response(
                    |response: &Response<BoxBody>, latency: Duration, _span: &Span| {
                        tracing::info!("latency={:?}", latency)
                    },
                ),
        );

    if let (Some(cert), Some(key)) = (options.tls_cert, options.tls_key) {
        let config = RustlsConfig::from_pem_file(cert, key).await.unwrap();

        axum_server::bind_rustls(options.listen, config)
            .serve(app.into_make_service())
            .await
            .unwrap();
    } else {
        axum::Server::bind(&options.listen)
            .serve(app.into_make_service())
            .await
            .unwrap();
    }

    Ok(())

    /*
    // Setup routing.
    let v1 = warp::path("v1");
    let peer = path("peer")
        .and(warp::get().and(path::end()))
        .and_then(move || peer_handler(peer_id));
    let projects = path("projects").and(project_filters(ctx.clone()));
    let sessions = path("sessions").and(session_filters(ctx.clone()));
    let delegates = path("delegates").and(
        warp::get()
            .map(move || ctx.clone())
            .and(path::param::<Urn>())
            .and(path("projects"))
            .and(path::end())
            .and_then(delegates_projects_handler),
    );
    let routes = path::end()
        .and_then(move || root_handler(peer_id))
        .or(v1.and(peer))
        .or(v1.and(projects))
        .or(v1.and(delegates))
        .or(v1.and(sessions))
        .recover(recover)
        .with(
            warp::cors()
                .allow_any_origin()
                .allow_methods(["GET", "POST", "PUT"])
                .allow_headers(["content-type", "authorization"]),
        )
        .with(warp::log("http::api"));

    let server = warp::serve(routes);

    if let (Some(cert), Some(key)) = (options.tls_cert, options.tls_key) {
        server
            .tls()
            .cert_path(cert)
            .key_path(key)
            .run(options.listen)
            .await;
    } else {
        server.run(options.listen).await;
    }
    Ok(())
    */
}

async fn cleanup_sessions_job(ctx: Context, interval: time::Duration) {
    let mut timer = tokio::time::interval(interval);

    loop {
        timer.tick().await; // Returns immediately the first time.

        let mut sessions = ctx.sessions.write().await;
        if let Err(err) = ctx.cleanup_sessions(&mut sessions) {
            tracing::error!("Failed to cleanup sessions: {}", err);
        }
    }
}

async fn populate_fingerprints_job(ctx: Context, interval: time::Duration) {
    let mut timer = tokio::time::interval(interval);

    loop {
        timer.tick().await; // Returns immediately the first time.

        let mut projects = ctx.projects.write().await;
        if let Err(err) = ctx.populate_fingerprints(&mut projects).await {
            tracing::error!("Failed to populate project fingerprints: {}", err);
        }
    }
}

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
*/

async fn root_handler(Extension(peer_id): Extension<PeerId>) -> impl IntoResponse {
    let response = json!({
        "message": "Welcome!",
        "service": "radicle-http-api",
        "version": VERSION,
        "peer": { "id": peer_id },
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

    Json(response)
}

/*
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
    let storage = ctx.storage().await?;
    let project = identities::project::get(storage.read_only(), &urn)
        .map_err(Error::Identities)?
        .ok_or(Error::NotFound)?;
    let meta: project::Metadata = project.try_into().map_err(Error::Project)?;
    let response = project::tracked(&meta, storage.read_only())?;

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

    Ok(warp::reply::json(&projects))
}

async fn project_urn_handler(ctx: Context, urn: Urn) -> Result<Json, Rejection> {
    let info = ctx.project_info(urn).await?;

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
*/

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
    let repo = git::Repository::new(paths.git_dir())?;
    let mut browser = git::Browser::new_with_namespace(&repo, &namespace, revision)?;

    Ok(callback(&mut browser)?)
}

fn get_head_commit(
    repo: &git2::Repository,
    urn: &Urn,
    default_branch: &str,
    delegates: &[project::Delegate],
) -> Result<git::Commit, Error> {
    let namespace = Namespace::try_from(urn).map_err(|_| Error::MissingNamespace)?;
    let branch = One::try_from(default_branch).map_err(|_| Error::MissingDefaultBranch)?;
    let local = Reference::head(namespace.clone(), None, branch.clone()).to_string();
    let result = repo.find_reference(&local);

    let head = match result {
        Ok(b) => b,
        Err(_) => {
            tracing::debug!("No local head, falling back to project delegates");
            let resolved_default_delegate = match delegates {
                [project::Delegate::Direct { id }] => Ok(id),
                [project::Delegate::Indirect { ids, .. }] => {
                    let ids: Vec<&PeerId> = ids.iter().collect();
                    if let [id] = ids.as_slice() {
                        Ok(*id)
                    } else {
                        Err(Error::NoHead("project has single indirect delegate with zero or more than one direct delegate"))
                    }
                }
                other => {
                    if other.len() > 1 {
                        Err(Error::NoHead("project has multiple delegates"))
                    } else {
                        Err(Error::NoHead("project has no delegates"))
                    }
                }
            }?;
            let remote = Reference::head(namespace, *resolved_default_delegate, branch).to_string();

            repo.find_reference(&remote)
                .map_err(|_| Error::NoHead("history lookup failed"))?
        }
    };
    let oid = head
        .target()
        .ok_or(Error::NoHead("head target not found"))?;
    let commit = repo.find_commit(oid)?.try_into()?;

    Ok(commit)
}

fn remote_branch(branch_name: &str, peer_id: &PeerId) -> git::Branch {
    // NOTE<sebastinez>: We should be able to pass simply a branch name without heads/ and be able to query that later.
    // Needs work on radicle_surf I assume.
    git::Branch::remote(
        &format!("heads/{}", branch_name),
        &peer_id.default_encoding(),
    )
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_local_head() {
        use std::convert::TryFrom;

        let branch = String::from("master");
        let branch_ref = One::try_from(branch.as_str()).unwrap();
        let urn = Urn::try_from_id("hnrkfbrd7y9674d8ow8uioki16fniwcyoz67y").unwrap();
        let namespace = Namespace::try_from(urn).unwrap();
        let local = Reference::head(namespace, None, branch_ref);

        assert_eq!(
            local.to_string(),
            "refs/namespaces/hnrkfbrd7y9674d8ow8uioki16fniwcyoz67y/refs/heads/master"
        );
    }

    #[test]
    fn test_remote_head() {
        use std::convert::TryFrom;

        let branch = String::from("master");
        let branch_ref = One::try_from(branch.as_str()).unwrap();
        let urn = Urn::try_from_id("hnrkfbrd7y9674d8ow8uioki16fniwcyoz67y").unwrap();
        let namespace = Namespace::try_from(urn).unwrap();
        let peer =
            PeerId::from_str("hyypw8z5g7tbui9ceh6tng58i1qk696isjnzix9fq9g41fzgjgqk8g").unwrap();
        let remote = Reference::head(namespace, peer, branch_ref);

        assert_eq!(
            remote.to_string(),
            "refs/namespaces/hnrkfbrd7y9674d8ow8uioki16fniwcyoz67y/refs/remotes/hyypw8z5g7tbui9ceh6tng58i1qk696isjnzix9fq9g41fzgjgqk8g/heads/master"
        );
    }
}
