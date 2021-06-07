mod error;

use std::convert::TryFrom as _;
use std::convert::TryInto as _;
use std::net;
use std::path::PathBuf;

use warp::reply::Json;
use warp::{self, filters::BoxedFilter, path, Filter, Rejection, Reply};

use radicle_daemon::librad::git::types::{Reference, Single};
use radicle_daemon::{git::types::Namespace, Paths, Urn};
use radicle_source::surf::vcs::git;
use radicle_source::Revision;

use error::Error;

type PeerId = String;

#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    pub listen: net::SocketAddr,
}

#[derive(Debug, Clone)]
pub struct Context {
    paths: Paths,
}

/// Run the HTTP API.
pub async fn run(options: Options) {
    let paths = Paths::from_root(options.root).unwrap();
    let ctx = Context { paths };
    let api = path("v1")
        .and(filters(ctx))
        .with(warp::cors().allow_any_origin());

    warp::serve(api).run(options.listen).await
}

/// Combination of all source filters.
fn filters(ctx: Context) -> BoxedFilter<(impl Reply,)> {
    project_filter(ctx.clone())
        .or(tree_filter(ctx.clone()))
        .or(blob_filter(ctx.clone()))
        .boxed()
}

/// `GET /:project/blob/:revision/:path`
fn blob_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("blob"))
        .and(path::param::<String>())
        .and(path::tail())
        .and_then(blob_handler)
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

async fn blob_handler(
    ctx: Context,
    project: Urn,
    revision: String,
    path: warp::filters::path::Tail,
) -> Result<impl Reply, Rejection> {
    let reference = Reference::head(Namespace::from(project), None, revision.try_into().unwrap());
    let blob = browse(reference, ctx.paths, |browser| {
        radicle_source::blob::highlighting::blob::<PeerId>(
            browser,
            None,
            path.as_str(),
            Some("base16-ocean.dark"),
        )
    })
    .await
    .map_err(error::Error::from)?;

    Ok(warp::reply::json(&blob))
}

async fn project_handler(_ctx: Context, _project: Urn) -> Result<Json, Rejection> {
    Err(warp::reject())
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
    let revision = Revision::<PeerId>::Sha {
        sha: revision.as_str().try_into().unwrap(),
    };
    let tree = browse(reference, ctx.paths, |mut browser| {
        radicle_source::tree(&mut browser, Some(revision), Some(path.as_str().to_owned()))
    })
    .await?;

    Ok(warp::reply::json(&tree))
}

pub async fn browse<T, F>(
    reference: Reference<Single>,
    paths: Paths,
    callback: F,
) -> Result<T, Error>
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
