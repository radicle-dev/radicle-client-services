mod error;
mod project;
mod signer;

use std::convert::TryFrom as _;
use std::convert::TryInto as _;
use std::net;
use std::path::PathBuf;

use either::Either;
use warp::reply::Json;
use warp::{self, filters::BoxedFilter, path, Filter, Rejection, Reply};

use radicle_daemon::librad::git::identities;
use radicle_daemon::librad::git::storage::Storage;
use radicle_daemon::librad::git::types::{Reference, Single};
use radicle_daemon::{git::types::Namespace, Paths, PeerId, Urn};
use radicle_source::surf::file_system::Path;
use radicle_source::surf::vcs::git;
use radicle_source::Revision;

use crate::project::Info;

use error::Error;

#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    pub listen: net::SocketAddr,
    pub peer_id: PeerId,
}

#[derive(Debug, Clone)]
pub struct Context {
    paths: Paths,
    signer: signer::Signer,
}

/// Run the HTTP API.
pub async fn run(options: Options) {
    let paths = Paths::from_root(options.root).unwrap();
    let signer = signer::Signer::new(options.peer_id);
    let ctx = Context { paths, signer };
    let api = path("v1")
        .and(path("projects"))
        .and(filters(ctx))
        .with(warp::cors().allow_any_origin())
        .with(warp::log("http::api"));

    warp::serve(api).run(options.listen).await
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
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("blob"))
        .and(path::param::<String>())
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
            if let Ok(blob) = radicle_source::blob::highlighting::blob::<PeerId>(
                browser,
                None,
                path,
                Some("base16-ocean.dark"),
            ) {
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
    let info = project_info(urn, ctx.signer, ctx.paths)?;

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
    let tree = browse(reference, ctx.paths, |mut browser| {
        radicle_source::tree(&mut browser, Some(revision), Some(path.as_str().to_owned()))
    })
    .await?;

    Ok(warp::reply::json(&tree))
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

fn project_info(urn: Urn, signer: signer::Signer, paths: Paths) -> Result<Info, Error> {
    let storage = Storage::open(&paths, signer)?;
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
