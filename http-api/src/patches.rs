use warp::{self, path, Filter, Rejection, Reply};

use librad::git::Urn;

use radicle_common::patch;

use crate::error::Error;
use crate::project;
use crate::Context;

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

async fn patches_handler(ctx: Context, urn: Urn) -> Result<impl Reply, Rejection> {
    let storage = ctx.storage().await?;
    let info = ctx.project_info(urn.clone()).await?;

    // Start off with our own patches, then iterate over peer patches.
    let mut patches = patch::all(&info.meta, None, storage.read_only()).map_err(Error::Patches)?;
    let peers = project::tracked(&info.meta, storage.read_only())?;
    for peer in peers {
        let all =
            patch::all(&info.meta, Some(peer), storage.read_only()).map_err(Error::Patches)?;

        patches.extend(all);
    }

    Ok(warp::reply::json(&patches))
}
