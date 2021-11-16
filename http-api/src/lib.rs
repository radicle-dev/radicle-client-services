mod error;
mod identity;
mod project;
mod signer;

use std::convert::TryFrom as _;
use std::convert::TryInto as _;
use std::net;
use std::path::PathBuf;

use anyhow::Context as AnyhowContext;
use serde_json::json;
use warp::hyper::StatusCode;
use warp::reply;
use warp::reply::Json;
use warp::{self, filters::BoxedFilter, path, Filter, Rejection, Reply};

use either::Either;

use radicle_daemon::librad::git::identities;
use radicle_daemon::librad::git::storage::read::ReadOnly;
use radicle_daemon::librad::git::types::{Reference, Single};
use radicle_daemon::{git::types::Namespace, Paths, PeerId, Urn};
use radicle_source::surf::file_system::Path;
use radicle_source::surf::vcs::git;
use radicle_source::Revision;

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
    pub identity: PathBuf,
    pub identity_passphrase: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Context {
    paths: Paths,
    theme: String,
}

/// Run the HTTP API.
pub async fn run(options: Options) {
    let paths = Paths::from_root(options.root).unwrap();
    let ctx = Context {
        paths,
        theme: options.theme,
    };
    let peer_id = peer_id_from_identity(options.identity, options.identity_passphrase)
        .expect("--identity path flag is missing");

    let peer_id_route = warp::path("peer-id")
        .and(warp::get())
        .and_then(move || get_peer_id(peer_id));

    let api = path("v1")
        .and(path("projects"))
        .and(filters(ctx))
        .or(warp::get().and(path::end()).and_then(root_handler))
        .or(peer_id_route)
        .recover(recover)
        .with(warp::cors().allow_any_origin())
        .with(warp::log("http::api"));
    let server = warp::serve(api);

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

fn peer_id_from_identity(
    identity_path: PathBuf,
    identity_passphrase: Option<String>,
) -> Result<PeerId, Error> {
    let identity = if let Some(passphrase) = identity_passphrase {
        identity::Identity::Encrypted {
            path: identity_path.clone(),
            passphrase: passphrase.into(),
        }
    } else {
        identity::Identity::Plain(identity_path.clone())
    };
    let signer = identity
        .signer()
        .with_context(|| format!("unable to load identity {:?}", &identity_path))?;
    let peer_id = PeerId::from(signer);
    Ok(peer_id)
}

/// Return the peer id for the node identity.
/// `GET /peer-id`
async fn get_peer_id(peer_id: PeerId) -> Result<impl warp::Reply, warp::Rejection> {
    Ok(peer_id.to_string())
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
    project_filter(ctx.clone())
        .or(tree_filter(ctx.clone()))
        .or(blob_filter(ctx.clone()))
        .or(readme_filter(ctx))
        .boxed()
}

/// `GET /:project/blob/:revision/:path`
fn blob_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    #[derive(serde::Deserialize)]
    struct Query {
        highlight: bool,
    }

    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("blob"))
        .and(path::param::<String>())
        .and(warp::query().map(|q: Query| q.highlight))
        .and(path::tail())
        .and_then(blob_handler)
}

/// `GET /:project/readme/:revision`
fn readme_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("readme"))
        .and(path::param::<String>())
        .and(path::end())
        .and_then(readme_handler)
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

/// `GET /:project/tree/:revision/:prefix`
fn tree_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("tree"))
        .and(path::param::<String>())
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
            }
        ]
    });
    Ok(warp::reply::json(&response))
}

async fn blob_handler(
    ctx: Context,
    project: Urn,
    revision: String,
    highlight: bool,
    path: warp::filters::path::Tail,
) -> Result<impl Reply, Rejection> {
    let theme = if highlight {
        Some(ctx.theme.as_str())
    } else {
        None
    };
    let reference = Reference::head(Namespace::from(project), None, revision.try_into().unwrap());
    let blob = browse(reference, ctx.paths, |browser| {
        radicle_source::blob::highlighting::blob::<PeerId>(browser, None, path.as_str(), theme)
    })
    .await
    .map_err(error::Error::from)?;

    Ok(warp::reply::json(&blob))
}

async fn readme_handler(
    ctx: Context,
    project: Urn,
    revision: String,
) -> Result<impl Reply, Rejection> {
    let reference = Reference::head(Namespace::from(project), None, revision.try_into().unwrap());
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
    .await
    .map_err(error::Error::from)?;

    Ok(warp::reply::json(&blob))
}

async fn project_handler(ctx: Context, urn: Urn) -> Result<Json, Rejection> {
    let info = project_info(urn, ctx.paths)?;

    Ok(warp::reply::json(&info))
}

/// Fetch a [`radicle_source::Tree`].
async fn tree_handler(
    ctx: Context,
    project: Urn,
    revision: String,
    path: warp::filters::path::Tail,
) -> Result<impl Reply, Rejection> {
    let reference = Reference::head(
        Namespace::from(project),
        None,
        revision.clone().try_into().unwrap(),
    );
    // Nb. Creating a `Revision` and setting it in the `tree` call seems to be redundant.
    // We can remove this when we figure out what's the best way.
    let revision = Revision::<PeerId>::Sha {
        sha: revision.as_str().try_into().unwrap(),
    };
    let (tree, stats) = browse(reference, ctx.paths, |mut browser| {
        Ok((
            radicle_source::tree(&mut browser, Some(revision), Some(path.as_str().to_owned()))?,
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

    let commit = git::Oid::from_str(reference.name.as_str())?;
    let repo = git::Repository::new(paths.git_dir().to_owned())?;
    let mut browser = git::Browser::new_with_namespace(&repo, &namespace, commit)?;

    Ok(callback(&mut browser)?)
}

fn project_info(urn: Urn, paths: Paths) -> Result<Info, Error> {
    let storage = ReadOnly::open(&paths)?;
    let project = identities::project::get(&storage, &urn)?.ok_or(Error::NotFound)?;

    let remote = project
        .delegations()
        .iter()
        .flat_map(|either| match either {
            Either::Left(pk) => Either::Left(std::iter::once(PeerId::from(*pk))),
            Either::Right(indirect) => {
                Either::Right(indirect.delegations().iter().map(|pk| PeerId::from(*pk)))
            }
        })
        .next()
        .ok_or(Error::MissingDelegations)?;

    let meta: project::Metadata = project.try_into()?;
    let repo = git::Repository::new(paths.git_dir().to_owned())?;
    let namespace = git::namespace::Namespace::try_from(urn.encode_id().as_str())?;
    let branch = git::Branch::remote(
        &format!("heads/{}", meta.default_branch),
        &remote.default_encoding(),
    );
    let browser = git::Browser::new_with_namespace(&repo, &namespace, branch)?;
    let history = browser.get();
    let head = history.first();

    Ok(Info {
        meta,
        head: head.id.to_string(),
    })
}
