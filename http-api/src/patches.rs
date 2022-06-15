use warp::{self, path, Filter, Rejection, Reply};

use librad::collaborative_objects::ObjectId;
use librad::git::Urn;

use radicle_common::cobs::patch;
use radicle_common::person;

use crate::error::Error;
use crate::Context;

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

/// `GET /:project/patches/:id`
pub fn patch_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("patches"))
        .and(path::param::<ObjectId>())
        .and(path::end())
        .and_then(patch_handler)
}

async fn patches_handler(ctx: Context, urn: Urn) -> Result<impl Reply, Rejection> {
    let storage = ctx.storage().await?;
    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let patches = patch::Patches::new(whoami, &ctx.paths, &storage).map_err(Error::Patches)?;
    let all: Vec<_> = patches
        .all(&urn)
        .map_err(Error::Patches)?
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

    Ok(warp::reply::json(&all))
}

async fn patch_handler(ctx: Context, urn: Urn, patch_id: ObjectId) -> Result<impl Reply, Rejection> {
    let storage = ctx.storage().await?;
    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let patches = patch::Patches::new(whoami, &ctx.paths, &storage).map_err(Error::Patches)?;
    let mut patch = patches
        .get(&urn, &patch_id)
        .map_err(Error::Patches)?
        .ok_or(Error::NotFound)?;
    if let Err(e) = patch
        .resolve(storage.as_ref())
        .map_err(Error::IdentityResolve)
    {
        tracing::warn!("Failed to resolve identities in patch {}: {}", patch_id, e);
    }



    Ok(warp::reply::json(&Cob::new(patch_id, patch)))
}
