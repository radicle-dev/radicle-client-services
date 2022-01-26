#![allow(clippy::if_same_then_else)]
mod error;

use error::Error;

use std::net;
use std::path::PathBuf;

use serde_json::json;
use warp::hyper::StatusCode;
use warp::reply;
use warp::reply::Json;
use warp::{self, filters::BoxedFilter, path, Filter, Rejection, Reply};

mod git;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PAGINATION: usize = 5;

mod db;

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
    root: PathBuf,
    handle: db::Handle,
}

/// Run the HTTP API.
pub async fn run(options: Options) {
    let db_path = options.root.join("indexer_rocksdb_storage");
    let handle = db::Handle::from_path(db_path.to_str().unwrap()).unwrap();

    let ctx = Context {
        root: options.root,
        handle,
    };

    let v1 = path("v1");
    let projects = path("projects").and(repo_filters(ctx.clone()));
    let sync = path("sync").and(
        warp::get()
            .map(move || ctx.clone())
            .and(path("seed"))
            .and(path::param::<String>())
            .and(path("namespace"))
            .and(path::param::<String>())
            .and(path::end())
            .and_then(sync_handler),
    );

    let routes = path::end()
        .and_then(root_handler)
        .or(v1.and(projects))
        .or(v1.and(sync))
        .recover(recover)
        .with(warp::cors().allow_any_origin())
        .with(warp::log("indexer-node::api"));

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

async fn root_handler() -> Result<impl Reply, Rejection> {
    let response = json!({
        "message": "Welcome!",
        "service": "radicle-indexer-node",
        "version": VERSION,
        "path": "/",
        "links": [
            {
                "href": "/v1/projects",
                "rel": "projects",
                "type": "GET"
            },
            {
                "href": "/v1/orgs",
                "rel": "orgs",
                "type": "GET"
            },
            {
                "href": "/v1/users",
                "rel": "users",
                "type": "GET"
            }
        ]
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

/// Combination of all repo filters.
fn repo_filters(ctx: Context) -> BoxedFilter<(impl Reply,)> {
    project_root_filter(ctx.clone())
        .or(project_by_id_filter(ctx.clone()))
        .or(project_from_filter(ctx))
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

/// `GET /:project`
fn project_by_id_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<String>())
        .and(path::end())
        .and_then(project_by_id_handler)
}

/// `GET /from/:start`
fn project_from_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path("from"))
        .and(path::param::<String>())
        .and(path::end())
        .and_then(project_from_handler)
}

/// List all projects
async fn project_root_handler(ctx: Context) -> Result<Json, Rejection> {
    let projects: Vec<_> = ctx
        .handle
        .list_repositories()
        .take(PAGINATION)
        .map(|repo| {
            json!({
                repo.0: repo.1,
            })
        })
        .collect();

    Ok(warp::reply::json(&projects))
}

/// List single project by id
async fn project_by_id_handler(ctx: Context, id: String) -> Result<Json, Rejection> {
    let projects: Vec<_> = ctx
        .handle
        .iterate_from_prefix(format!("repo::{}", id))
        .take(1)
        .take_while(|(k, _)| k.starts_with("repo::"))
        .map(|repo| {
            json!({
                repo.0: repo.1
            })
        })
        .collect();

    Ok(warp::reply::json(&projects))
}

/// List next projects from key prefix `from`
async fn project_from_handler(ctx: Context, from: String) -> Result<Json, Rejection> {
    let projects: Vec<_> = ctx
        .handle
        .iterate_from_prefix(from)
        .take(PAGINATION)
        .take_while(|(k, _)| k.starts_with("repo::"))
        .map(|repo| {
            json!({
                repo.0: repo.1
            })
        })
        .collect();

    Ok(warp::reply::json(&projects))
}

/// `GET /sync/seed/:seed/namespace/:namespace/`
/// Handle sync requests
async fn sync_handler(ctx: Context, seed: String, namespace: String) -> Result<Json, Rejection> {
    let seed = format!("http://{}", seed);
    let seed = url::Url::parse(&seed).map_err(Error::from)?;
    let url = seed.join(&namespace).map_err(Error::from)?;
    let repo = ctx.root.join("git");

    let result = git::git(
        &repo,
        [
            "fetch",
            "--force",
            url.as_str(),
            &format!("refs/rad/id:refs/namespaces/{}/refs/rad/id", namespace),
        ],
    )
    .is_ok();

    if result {
        sync_with_db(&ctx, &namespace);
    }

    let response = json!({ "success": result });
    Ok(warp::reply::json(&response))
}

fn sync_with_db(ctx: &Context, namespace: &str) {
    // Only sync if `namespace` is a Project identity.
    // Identities of delegates are supposed to have been pushed beforehand.
    use librad::git::Urn;
    let urn = Urn::try_from_id(namespace).unwrap();

    use librad::git::storage::ReadOnly;
    use librad::paths::Paths;

    let paths = Paths::from_root(&ctx.root).unwrap();
    let storage = ReadOnly::open(&paths).expect("failed to read storage paths");

    let identity = librad::git::identities::project::get(&storage, &urn);

    if identity.is_err() {
        // not a project
        return;
    }

    let identity = identity.unwrap().unwrap();
    let _result = ctx
        .handle
        .add_repository(namespace, format!("{:?}", identity));
}
