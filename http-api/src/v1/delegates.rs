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
    let cobs = cobs::Store::new(whoami, &ctx.paths, &storage);
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

                    let issues = issues.count(&meta.urn).map_err(Error::Cobs).ok()?;
                    let patches = patches.count(&meta.urn).map_err(Error::Cobs).ok()?;

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

#[cfg(test)]
mod routes {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use tower::ServiceExt;

    use super::*;
    use crate::test_extra::setup;

    const THEME: &str = "base16-ocean.dark";
    const PROJECT_NAME: &str = "nakamoto";

    #[tokio::test]
    async fn test_delegates_projects_route() {
        let (profile, signer, project, _) = setup::env();
        let delegate = project
            .delegations()
            .iter()
            .next()
            .unwrap()
            .unwrap_right()
            .urn();
        let ctx = Context::new(profile.paths().to_owned(), signer, THEME.to_string());
        let app = router(ctx);
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/delegates/{}/projects", delegate))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(body[0]["name"], PROJECT_NAME);
        assert_eq!(body[1], Value::Null);
    }
}
