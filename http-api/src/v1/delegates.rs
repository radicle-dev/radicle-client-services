use std::convert::TryInto;

use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Json, Router};

use librad::git::identities::{self, SomeIdentity};
use librad::git::Urn;

use radicle_common::{cobs, person};

use crate::axum_extra::Path;
use crate::project::{self, Info};
use crate::{get_head_commit, Context, Error};

pub fn router(ctx: Context) -> Router {
    Router::new()
        .route(
            "/delegates/:delegate/projects",
            get(delegates_projects_handler),
        )
        .layer(Extension(ctx))
}

/// List all projects that delegate is a part of.
/// `GET /delegates/:delegate/projects`
async fn delegates_projects_handler(
    Extension(ctx): Extension<Context>,
    Path(delegate): Path<Urn>,
) -> impl IntoResponse {
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
                    use either::Either;

                    if !project.delegations().iter().any(|d| match d {
                        Either::Right(indirect) => indirect.urn() == delegate,
                        Either::Left(_) => false,
                    }) {
                        return None;
                    }

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

    Ok::<_, Error>(Json(projects))
}
