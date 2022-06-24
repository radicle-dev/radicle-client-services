use warp::{self, path, Filter, Rejection, Reply};

use librad::collaborative_objects::ObjectId;
use librad::git::Urn;

use radicle_common::cobs::{issue, Store};
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
    let store = Store::new(whoami, &ctx.paths, &storage).map_err(Error::from)?;
    let issues = issue::IssueStore::new(&store);
    let all: Vec<_> = issues
        .all(&project)
        .map_err(Error::Issues)?
        .into_iter()
        .map(|(id, mut issue)| {
            if let Err(e) = issue
                .resolve(storage.as_ref())
                .map_err(Error::IdentityResolve)
            {
                tracing::warn!("Failed to resolve identities in issue {}: {}", id, e);
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
    let store = Store::new(whoami, &ctx.paths, &storage).map_err(Error::from)?;
    let issues = issue::IssueStore::new(&store);
    let mut issue = issues
        .get(&project, &issue_id)
        .map_err(Error::from)?
        .ok_or(Error::NotFound)?;
    if let Err(e) = issue
        .resolve(storage.as_ref())
        .map_err(Error::IdentityResolve)
    {
        tracing::warn!("Failed to resolve identities in issue {}: {}", issue_id, e);
    }

    Ok(warp::reply::json(&Cob::new(issue_id, issue)))
}
