use warp::{self, path, Filter, Rejection, Reply};

use librad::collaborative_objects::ObjectId;
use librad::git::Urn;

use radicle_common::cobs::issue;
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

/// `GET /:project/issues`
pub fn issues_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("issues"))
        .and(path::end())
        .and_then(issues_handler)
}

/// `GET /:project/issues/:id`
pub fn issue_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("issues"))
        .and(path::param::<ObjectId>())
        .and(path::end())
        .and_then(issue_handler)
}

async fn issues_handler(ctx: Context, project: Urn) -> Result<impl Reply, Rejection> {
    // TODO: Handle non-existing project.
    let storage = ctx.storage().await?;
    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let issues = issue::Issues::new(whoami, &ctx.paths, &storage).map_err(Error::Issues)?;
    let all: Vec<_> = issues
        .all(&project)
        .map_err(Error::Issues)?
        .into_iter()
        .map(|(id, mut issue)| {
            if let Err(e) = issue
                .resolve(storage.as_ref())
                .map_err(Error::IdentityResolveError)
            {
                tracing::debug!("Failed to resolve issue author: {}", e);
            }

            Cob::new(id, issue)
        })
        .collect();

    Ok(warp::reply::json(&all))
}

async fn issue_handler(
    ctx: Context,
    project: Urn,
    issue_id: ObjectId,
) -> Result<impl Reply, Rejection> {
    // TODO: Handle non-existing project.
    let storage = ctx.storage().await?;
    let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
    let issues = issue::Issues::new(whoami, &ctx.paths, &storage).map_err(Error::Issues)?;
    let mut issue = issues
        .get(&project, &issue_id)
        .map_err(Error::Issues)?
        .ok_or(Error::NotFound)?;
    issue
        .resolve(storage.as_ref())
        .map_err(Error::IdentityResolveError)?;

    Ok(warp::reply::json(&Cob::new(issue_id, issue)))
}
