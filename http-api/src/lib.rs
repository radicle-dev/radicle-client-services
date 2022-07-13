#![allow(clippy::if_same_then_else)]
mod auth;
mod commit;
#[path = "axum.rs"]
mod custom_axum;
mod error;
mod issues;
mod patches;
mod project;
mod v1;

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::convert::{TryFrom, TryInto as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{self, Duration};
use std::{env, net};

use axum::body::BoxBody;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::Method;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::{Extension, Router};
use axum_server::tls_rustls::RustlsConfig;
use chrono::Utc;
use hyper::http::{Request, Response};
use hyper::Body;
use serde_json::json;
use tokio::sync::RwLock;
use tower_http::cors::{self, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::Span;

use librad::crypto::BoxedSigner;
use librad::git::identities::{self, SomeIdentity};
use librad::git::storage::pool::{InitError, Initialised};
use librad::git::storage::{self, Pool, Storage};
use librad::git::types::{Namespace, One, Reference, Single};
use librad::git::Urn;
use librad::paths::Paths;
use librad::PeerId;

use radicle_common::{cobs, keys, person};
use radicle_source::surf::vcs::git;

use crate::auth::AuthState;
use crate::project::{Info, PeerInfo};

use error::Error;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const POPULATE_FINGERPRINTS_INTERVAL: time::Duration = time::Duration::from_secs(20);
pub const CLEANUP_SESSIONS_INTERVAL: time::Duration = time::Duration::from_secs(60);
pub const STORAGE_POOL_SIZE: usize = 3;

#[derive(Debug, Clone)]
pub struct Options {
    pub root: Option<PathBuf>,
    pub passphrase: Option<String>,
    pub listen: net::SocketAddr,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub theme: String,
}

/// SSH Key fingerprint.
type Fingerprint = String;
/// Mapping between fingerprints and users.
type Fingerprints = HashMap<Fingerprint, PeerInfo>;
/// Identifier for sessions
type SessionId = String;

#[derive(Clone)]
pub struct Context {
    paths: Paths,
    theme: String,
    pool: Pool<Storage>,
    peer_id: PeerId,
    aliases: Arc<RwLock<HashMap<String, Urn>>>,
    projects: Arc<RwLock<HashMap<Urn, Fingerprints>>>,
    sessions: Arc<RwLock<HashMap<SessionId, AuthState>>>,
}

impl Context {
    fn new(paths: Paths, signer: BoxedSigner, theme: String) -> Self {
        let peer_id = signer.peer_id();
        let pool = storage::Pool::new(
            storage::pool::ReadWriteConfig::new(paths.clone(), signer, Initialised::no()),
            STORAGE_POOL_SIZE,
        );

        Self {
            paths,
            pool,
            theme,
            peer_id,
            aliases: Default::default(),
            projects: Default::default(),
            sessions: Default::default(),
        }
    }

    async fn storage(&self) -> Result<deadpool::managed::Object<Storage, InitError>, Error> {
        self.pool
            .get()
            .await
            .map_err(|e| Error::Pool(e.to_string()))
    }

    /// Populates alias map with unique projects' names and their urns
    async fn populate_aliases(&self, map: &mut HashMap<String, Urn>) -> Result<(), Error> {
        use librad::git::identities::SomeIdentity::Project;

        let storage = self.storage().await?;
        let identities = identities::any::list(storage.read_only())?;

        for identity in identities.flatten() {
            if let Project(project) = identity {
                let urn = project.urn();
                let name = (&project.payload().subject.name).to_string();

                if let Entry::Vacant(e) = map.entry(name.clone()) {
                    e.insert(urn);
                }
            }
        }

        Ok(())
    }

    fn cleanup_sessions(&self, map: &mut HashMap<SessionId, AuthState>) -> Result<(), Error> {
        let mut to_remove: Vec<SessionId> = Vec::new();

        for (key, value) in map.iter() {
            let current_time = Utc::now();
            match value {
                AuthState::Authorized(auth) => {
                    if let Some(exp_time) = auth.expiration_time {
                        if current_time >= exp_time {
                            to_remove.push(key.clone());
                        }
                    }
                }
                AuthState::Unauthorized {
                    expiration_time, ..
                } => {
                    if current_time >= *expiration_time {
                        to_remove.push(key.clone())
                    }
                }
            }
        }

        for key in to_remove {
            map.remove(&key);
        }

        Ok(())
    }

    /// Populate a map between SSH fingerprints and their peer identities
    async fn populate_fingerprints(
        &self,
        map: &mut HashMap<Urn, Fingerprints>,
    ) -> Result<(), Error> {
        let storage = self.storage().await?;
        let identities = identities::any::list(storage.read_only())?;

        for identity in identities.flatten() {
            if let SomeIdentity::Project(project) = identity {
                let meta = project::Metadata::try_from(project)?;
                let fingerprints = map.entry(meta.urn.clone()).or_default();
                let tracked = project::tracked(&meta, storage.read_only())?;

                for peer in tracked {
                    let fp = keys::to_ssh_fingerprint(&peer.id).expect("Conversion cannot fail");
                    fingerprints.insert(fp, peer);
                }
            }
        }

        Ok(())
    }

    /// From a commit hash, return the signer's fingerprint, if any.
    fn commit_ssh_fingerprint(&self, sha1: &str) -> Result<Option<String>, Error> {
        radicle_common::git::commit_ssh_fingerprint(self.paths.git_dir(), sha1)
            .map_err(|e| Error::Io("failed to get commit's ssh fingerprint", e))
    }

    async fn project_info(&self, urn: Urn) -> Result<Info, Error> {
        let storage = self.storage().await?;
        let project = identities::project::get(&*storage, &urn)?.ok_or(Error::NotFound)?;
        let meta: project::Metadata = project.try_into()?;

        let repo = git2::Repository::open_bare(self.paths.git_dir())?;
        let head = get_head_commit(&repo, &urn, &meta.default_branch, &meta.delegates)
            .map(|h| h.id)
            .ok();

        let whoami = person::local(&*storage).map_err(Error::LocalIdentity)?;
        let cobs = cobs::Store::new(whoami, &self.paths, &storage)?;
        let issues = cobs.issues();
        let issues = issues.count(&urn).map_err(Error::Issues)?;

        let patches = cobs.patches();
        let patches = patches.count(&urn).map_err(Error::Patches)?;

        Ok(Info {
            head,
            meta,
            issues,
            patches,
        })
    }
}

/// Run the HTTP API.
pub async fn run(options: Options) -> anyhow::Result<()> {
    let (_, profile, signer) = shared::profile(options.root, options.passphrase)?;
    let paths = profile.paths();
    let ctx = Context::new(paths.clone(), signer, options.theme);
    let peer_id = ctx.peer_id;

    // Populate fingerprints
    tokio::spawn(populate_fingerprints_job(
        ctx.clone(),
        POPULATE_FINGERPRINTS_INTERVAL,
    ));
    // Cleanup sessions
    tokio::spawn(cleanup_sessions_job(ctx.clone(), CLEANUP_SESSIONS_INTERVAL));

    let root_router = Router::new()
        .route("/", get(root_handler))
        .layer(Extension(peer_id));

    let app = Router::new()
        .merge(root_router)
        .merge(v1::router(ctx.clone()))
        .layer(
            CorsLayer::new()
                .allow_origin(cors::Any)
                .allow_methods([Method::GET, Method::POST, Method::PUT])
                .allow_headers([CONTENT_TYPE, AUTHORIZATION]),
        )
        .layer(
            TraceLayer::new_for_http()
                .on_request(|request: &Request<Body>, _span: &Span| {
                    tracing::info!("{} {}", request.method(), request.uri().path())
                })
                .on_response(
                    |_response: &Response<BoxBody>, latency: Duration, _span: &Span| {
                        tracing::info!("latency={:?}", latency)
                    },
                ),
        );

    if let (Some(cert), Some(key)) = (options.tls_cert, options.tls_key) {
        let config = RustlsConfig::from_pem_file(cert, key).await.unwrap();

        axum_server::bind_rustls(options.listen, config)
            .serve(app.into_make_service())
            .await
            .unwrap();
    } else {
        axum::Server::bind(&options.listen)
            .serve(app.into_make_service())
            .await
            .unwrap();
    }

    Ok(())
}

async fn cleanup_sessions_job(ctx: Context, interval: time::Duration) {
    let mut timer = tokio::time::interval(interval);

    loop {
        timer.tick().await; // Returns immediately the first time.

        let mut sessions = ctx.sessions.write().await;
        if let Err(err) = ctx.cleanup_sessions(&mut sessions) {
            tracing::error!("Failed to cleanup sessions: {}", err);
        }
    }
}

async fn populate_fingerprints_job(ctx: Context, interval: time::Duration) {
    let mut timer = tokio::time::interval(interval);

    loop {
        timer.tick().await; // Returns immediately the first time.

        let mut projects = ctx.projects.write().await;
        if let Err(err) = ctx.populate_fingerprints(&mut projects).await {
            tracing::error!("Failed to populate project fingerprints: {}", err);
        }
    }
}

async fn root_handler(Extension(peer_id): Extension<PeerId>) -> impl IntoResponse {
    let response = json!({
        "message": "Welcome!",
        "service": "radicle-http-api",
        "version": VERSION,
        "peer": { "id": peer_id },
        "path": "/",
        "links": [
            {
                "href": "/v1/projects",
                "rel": "projects",
                "type": "GET"
            },
            {
                "href": "/v1/peer",
                "rel": "peer",
                "type": "GET"
            },
            {
                "href": "/v1/delegates/:urn/projects",
                "rel": "projects",
                "type": "GET"
            }
        ]
    });

    Json(response)
}

// TODO: move this fn to /v1/projects.rs
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

    let revision: git::Rev = match git::Oid::from_str(reference.name.as_str()) {
        Ok(oid) => oid.try_into().map_err(|_| Error::NotFound)?,
        Err(_) => remote_branch(
            &reference.name.to_string(),
            &reference.remote.ok_or(Error::NotFound)?,
        )
        .try_into()
        .map_err(|_| Error::NotFound)?,
    };
    let repo = git::Repository::new(paths.git_dir())?;
    let mut browser = git::Browser::new_with_namespace(&repo, &namespace, revision)?;

    Ok(callback(&mut browser)?)
}

fn get_head_commit(
    repo: &git2::Repository,
    urn: &Urn,
    default_branch: &str,
    delegates: &[project::Delegate],
) -> Result<git::Commit, Error> {
    let namespace = Namespace::try_from(urn).map_err(|_| Error::MissingNamespace)?;
    let branch = One::try_from(default_branch).map_err(|_| Error::MissingDefaultBranch)?;
    let local = Reference::head(namespace.clone(), None, branch.clone()).to_string();
    let result = repo.find_reference(&local);

    let head = match result {
        Ok(b) => b,
        Err(_) => {
            tracing::debug!("No local head, falling back to project delegates");
            let resolved_default_delegate = match delegates {
                [project::Delegate::Direct { id }] => Ok(id),
                [project::Delegate::Indirect { ids, .. }] => {
                    let ids: Vec<&PeerId> = ids.iter().collect();
                    if let [id] = ids.as_slice() {
                        Ok(*id)
                    } else {
                        Err(Error::NoHead("project has single indirect delegate with zero or more than one direct delegate"))
                    }
                }
                other => {
                    if other.len() > 1 {
                        Err(Error::NoHead("project has multiple delegates"))
                    } else {
                        Err(Error::NoHead("project has no delegates"))
                    }
                }
            }?;
            let remote = Reference::head(namespace, *resolved_default_delegate, branch).to_string();

            repo.find_reference(&remote)
                .map_err(|_| Error::NoHead("history lookup failed"))?
        }
    };
    let oid = head
        .target()
        .ok_or(Error::NoHead("head target not found"))?;
    let commit = repo.find_commit(oid)?.try_into()?;

    Ok(commit)
}

fn remote_branch(branch_name: &str, peer_id: &PeerId) -> git::Branch {
    // NOTE<sebastinez>: We should be able to pass simply a branch name without heads/ and be able to query that later.
    // Needs work on radicle_surf I assume.
    git::Branch::remote(
        &format!("heads/{}", branch_name),
        &peer_id.default_encoding(),
    )
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_local_head() {
        use std::convert::TryFrom;

        let branch = String::from("master");
        let branch_ref = One::try_from(branch.as_str()).unwrap();
        let urn = Urn::try_from_id("hnrkfbrd7y9674d8ow8uioki16fniwcyoz67y").unwrap();
        let namespace = Namespace::try_from(urn).unwrap();
        let local = Reference::head(namespace, None, branch_ref);

        assert_eq!(
            local.to_string(),
            "refs/namespaces/hnrkfbrd7y9674d8ow8uioki16fniwcyoz67y/refs/heads/master"
        );
    }

    #[test]
    fn test_remote_head() {
        use std::convert::TryFrom;
        use std::str::FromStr;

        let branch = String::from("master");
        let branch_ref = One::try_from(branch.as_str()).unwrap();
        let urn = Urn::try_from_id("hnrkfbrd7y9674d8ow8uioki16fniwcyoz67y").unwrap();
        let namespace = Namespace::try_from(urn).unwrap();
        let peer =
            PeerId::from_str("hyypw8z5g7tbui9ceh6tng58i1qk696isjnzix9fq9g41fzgjgqk8g").unwrap();
        let remote = Reference::head(namespace, peer, branch_ref);

        assert_eq!(
            remote.to_string(),
            "refs/namespaces/hnrkfbrd7y9674d8ow8uioki16fniwcyoz67y/refs/remotes/hyypw8z5g7tbui9ceh6tng58i1qk696isjnzix9fq9g41fzgjgqk8g/heads/master"
        );
    }
}
