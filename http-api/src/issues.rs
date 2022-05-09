use warp::{self, path, Filter, Rejection, Reply};

use librad::collaborative_objects::ObjectId;

use librad::git::Urn;

use radicle_common::cobs::issue;

use crate::error::Error;
use crate::Context;

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
    let storage = ctx.storage().await?;
    let whoami = radicle_common::person::local(&*storage).unwrap();
    let issues = issue::Issues::new(whoami, &ctx.paths, &storage).map_err(Error::Issues)?;
    let all = issues.all(&project).map_err(Error::Issues)?;

    Ok(warp::reply::json(&all))
}

async fn issue_handler(
    ctx: Context,
    project: Urn,
    issue_id: ObjectId,
) -> Result<impl Reply, Rejection> {
    let storage = ctx.storage().await?;
    let whoami = radicle_common::person::local(&*storage).unwrap();
    let issues = issue::Issues::new(whoami, &ctx.paths, &storage).map_err(Error::Issues)?;
    let issue = issues
        .get(&project, &issue_id)
        .map_err(Error::Issues)?
        .ok_or(Error::NotFound)?;

    Ok(warp::reply::json(&issue))
}
