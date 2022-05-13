#![allow(clippy::if_same_then_else)]
mod auth;
mod commit;
mod error;
mod issues;
mod patches;
mod project;

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::convert::TryInto as _;
use std::env;
use std::io;
use std::io::{BufRead, BufReader};
use std::iter::repeat_with;
use std::net;
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::sync::Arc;
use std::time;

use chrono::{DateTime, Utc};
use ethers_core::utils::hex;
use serde_json::json;
use siwe::Message;
use tokio::sync::RwLock;
use warp::hyper::StatusCode;
use warp::reply::Json;
use warp::{self, filters::BoxedFilter, host::Authority, path, query, Filter, Rejection, Reply};

use librad::crypto::BoxedSigner;
use librad::git::identities;
use librad::git::identities::SomeIdentity;
use librad::git::storage;
use librad::git::storage::pool::{InitError, Initialised};
use librad::git::storage::Pool;
use librad::git::storage::Storage;
use librad::git::types::{One, Reference, Single};
use librad::{git::types::Namespace, git::Urn, paths::Paths, profile::Profile, PeerId};

use radicle_common::keys;
use radicle_common::profile;
use radicle_common::profile::LnkHome;
use radicle_common::signer::ToSigner;
use radicle_source::surf::file_system::Path;
use radicle_source::surf::vcs::git;

use crate::auth::{AuthRequest, AuthState, Session};
use crate::project::{Info, PeerInfo};

use commit::{Commit, CommitContext, CommitTeaser, CommitsQueryString, Committer};
use error::Error;
use issues::{issue_filter, issues_filter};
use patches::patches_filter;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const POPULATE_FINGERPRINTS_INTERVAL: time::Duration = time::Duration::from_secs(20);
pub const CLEANUP_SESSIONS_INTERVAL: time::Duration = time::Duration::from_secs(60);
pub const UNAUTHORIZED_SESSIONS_EXPIRATION: time::Duration = time::Duration::from_secs(60);
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
        let output = Command::new("git")
            .current_dir(self.paths.git_dir()) // We need to place the command execution in the git dir
            .args(["show", sha1, "--pretty=%GF", "--raw"])
            .output()
            .map_err(|e| Error::Io("'git' command failed", e))?;

        if !output.status.success() {
            return Err(Error::Io(
                "'git' command failed",
                io::Error::new(
                    io::ErrorKind::Other,
                    String::from_utf8_lossy(&output.stderr),
                ),
            ));
        }

        let string = BufReader::new(output.stdout.as_slice())
            .lines()
            .next()
            .transpose()
            .map_err(|e| Error::Io("'git' command output couldn't be read", e))?;

        // We only return a fingerprint if it's not an empty string
        if let Some(s) = string {
            if !s.is_empty() {
                return Ok(Some(s));
            }
        }

        Ok(None)
    }

    async fn project_info(&self, urn: Urn) -> Result<Info, Error> {
        let storage = self.storage().await?;
        let project = identities::project::get(&*storage, &urn)?.ok_or(Error::NotFound)?;
        let meta: project::Metadata = project.try_into()?;

        let repo = git2::Repository::open_bare(self.paths.git_dir())?;
        let head = get_head_commit(&repo, &urn, &meta.default_branch, &meta.delegates)
            .map(|h| h.id)
            .ok();

        Ok(Info { head, meta })
    }
}

/// Run the HTTP API.
pub async fn run(options: Options) -> anyhow::Result<()> {
    let home = if let Some(root) = options.root {
        LnkHome::Root(root)
    } else {
        LnkHome::default()
    };

    // If a profile isn't found, create one.
    let profile = if let Some(profile) = Profile::active(&home)? {
        profile
    } else if let Some(ref pass) = options.passphrase {
        let pwhash = keys::pwhash(pass.clone().into());
        let (profile, _) = profile::create(home, pwhash)?;

        profile
    } else {
        anyhow::bail!("No active profile and no passphrase supplied");
    };
    tracing::info!("Profile {} loaded...", profile.id());

    // Get the signer, either from the passphrase and secret key, or from ssh-agent.
    let signer = if let Some(pass) = options.passphrase {
        keys::load_secret_key(&profile, pass.into())?.to_signer(&profile)?
    } else if let Ok(sock) = keys::ssh_auth_sock() {
        sock.to_signer(&profile)?
    } else {
        anyhow::bail!("No signer found: ssh-agent isn't running, and no passphrase was supplied");
    };

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

    // Setup routing.
    let v1 = warp::path("v1");
    let peer = path("peer")
        .and(warp::get().and(path::end()))
        .and_then(move || peer_handler(peer_id));
    let projects = path("projects").and(project_filters(ctx.clone()));
    let sessions = path("sessions").and(session_filters(ctx.clone()));
    let delegates = path("delegates").and(
        warp::get()
            .map(move || ctx.clone())
            .and(path::param::<Urn>())
            .and(path("projects"))
            .and(path::end())
            .and_then(delegates_projects_handler),
    );
    let routes = path::end()
        .and_then(root_handler)
        .or(v1.and(peer))
        .or(v1.and(projects))
        .or(v1.and(delegates))
        .or(v1.and(sessions))
        .recover(recover)
        .with(
            warp::cors()
                .allow_any_origin()
                .allow_methods(["GET", "POST", "PUT"])
                .allow_headers(["content-type", "authorization"]),
        )
        .with(warp::log("http::api"));

    let server = warp::serve(routes);

    if let (Some(cert), Some(key)) = (options.tls_cert, options.tls_key) {
        server
            .tls()
            .cert_path(cert)
            .key_path(key)
            .run(options.listen)
            .await;
    } else {
        server.run(options.listen).await;
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

async fn recover(err: Rejection) -> Result<impl Reply, std::convert::Infallible> {
    let (status, msg) = if err.is_not_found() {
        (StatusCode::NOT_FOUND, None)
    } else if let Some(Error::NotFound) = err.find::<Error>() {
        (StatusCode::NOT_FOUND, None)
    } else if let Some(Error::NoHead(msg)) = err.find::<Error>() {
        (StatusCode::NOT_FOUND, Some(msg.to_string()))
    } else if let Some(Error::Auth(msg)) = err.find::<Error>() {
        (StatusCode::BAD_REQUEST, Some(msg.to_string()))
    } else if let Some(Error::SiweParseError(_)) = err.find::<Error>() {
        (StatusCode::BAD_REQUEST, None)
    } else if let Some(Error::SiweVerificationError(_)) = err.find::<Error>() {
        (StatusCode::BAD_REQUEST, None)
    } else if let Some(err) = err.find::<warp::filters::body::BodyDeserializeError>() {
        (StatusCode::BAD_REQUEST, Some(err.to_string()))
    } else if let Some(Error::Git(e)) = err.find::<Error>() {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(e.message().to_owned()),
        )
    } else {
        // Log the non-standard errors.
        tracing::error!("Error: {:?}", err);

        (StatusCode::INTERNAL_SERVER_ERROR, None)
    };
    let body = json!({
        "error": msg.or_else(|| status.canonical_reason().map(|r| r.to_string())),
        "code": status.as_u16()
    });

    Ok(warp::http::Response::builder()
        .header("Content-Type", "application/json")
        .header("Access-Control-Allow-Origin", "*")
        .status(status)
        .body(body.to_string()))
}

fn session_filters(ctx: Context) -> BoxedFilter<(impl Reply,)> {
    session_create_filter(ctx.clone())
        .or(session_signin_filter(ctx.clone()))
        .or(session_get_filter(ctx))
        .boxed()
}

/// Combination of all project filters.
fn project_filters(ctx: Context) -> BoxedFilter<(impl Reply,)> {
    project_root_filter(ctx.clone())
        .or(commit_filter(ctx.clone()))
        .or(history_filter(ctx.clone()))
        .or(project_urn_filter(ctx.clone()))
        .or(project_alias_filter(ctx.clone()))
        .or(tree_filter(ctx.clone()))
        .or(remotes_filter(ctx.clone()))
        .or(remote_filter(ctx.clone()))
        .or(blob_filter(ctx.clone()))
        .or(readme_filter(ctx.clone()))
        .or(patches_filter(ctx.clone()))
        .or(issue_filter(ctx.clone()))
        .or(issues_filter(ctx))
        .boxed()
}

/// `GET /:project/blob/:sha/:path`
fn blob_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    #[derive(serde::Deserialize)]
    struct Query {
        highlight: bool,
    }

    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("blob"))
        .and(path::param::<One>())
        .and(warp::query().map(|q: Query| q.highlight))
        .and(path::tail())
        .and_then(blob_handler)
}

/// `GET /:project/remotes`
fn remotes_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("remotes"))
        .and(path::end())
        .and_then(remotes_handler)
}

/// `GET /:project/remotes/:peer`
fn remote_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("remotes"))
        .and(path::param::<PeerId>())
        .and(path::end())
        .and_then(remote_handler)
}

/// `GET /:project/commits?from=<sha>`
fn history_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("commits"))
        .and(query::<CommitsQueryString>())
        .and(path::end())
        .and_then(history_handler)
}

/// `GET /:project/commits/:sha`
fn commit_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("commits"))
        .and(path::param::<One>())
        .and(path::end())
        .and_then(commit_handler)
}

/// `GET /:project/readme/:sha`
fn readme_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("readme"))
        .and(path::param::<One>())
        .and(path::end())
        .and_then(readme_handler)
}

/// `GET /`
fn project_root_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::end())
        .and_then(project_root_handler)
        .boxed()
}

/// `GET /:project-urn`
fn project_urn_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path::end())
        .and_then(project_urn_handler)
        .boxed()
}

/// `GET /:project-alias`
fn project_alias_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<String>())
        .and(path::end())
        .and_then(project_alias_handler)
        .boxed()
}

/// `GET /:project/tree/:prefix`
fn tree_filter(ctx: Context) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<Urn>())
        .and(path("tree"))
        .and(path::param::<One>())
        .and(path::tail())
        .and_then(tree_handler)
}

/// `POST /sessions`
fn session_create_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::post()
        .map(move || ctx.clone())
        .and(path::end())
        .and_then(session_create_handler)
}

/// `PUT /sessions/:session-id`
fn session_signin_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::put()
        .map(move || ctx.clone())
        .and(path::param::<String>())
        .and(path::end())
        .and(warp::body::json())
        .and_then(session_signin_handler)
}

/// `GET /sessions/:session-id`
fn session_get_filter(
    ctx: Context,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    warp::get()
        .map(move || ctx.clone())
        .and(path::param::<String>())
        .and(path::end())
        .and_then(session_get_handler)
}

async fn root_handler() -> Result<impl Reply, Rejection> {
    let response = json!({
        "message": "Welcome!",
        "service": "radicle-http-api",
        "version": VERSION,
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
    Ok(warp::reply::json(&response))
}

/// Return the peer id for the node identity.
/// `GET /v1/peer`
async fn peer_handler(peer_id: PeerId) -> Result<impl warp::Reply, warp::Rejection> {
    let response = json!({
        "id": peer_id.to_string(),
    });
    Ok(warp::reply::json(&response))
}

/// `GET /v1/sessions/:session-id`
async fn session_get_handler(
    ctx: Context,
    id: String,
) -> Result<impl warp::Reply, warp::Rejection> {
    let sessions = ctx.sessions.read().await;
    let session = sessions.get(&id).ok_or(Error::NotFound)?;

    match session {
        AuthState::Authorized(session) => {
            Ok(warp::reply::json(&json!({ "id": id, "session": session })))
        }
        AuthState::Unauthorized {
            nonce,
            expiration_time,
        } => Ok(warp::reply::json(
            &json!({ "id": id, "nonce": nonce, "expirationTime": expiration_time }),
        )),
    }
}

/// `POST /v1/sessions`
async fn session_create_handler(ctx: Context) -> Result<impl warp::Reply, warp::Rejection> {
    let expiration_time =
        Utc::now() + chrono::Duration::from_std(UNAUTHORIZED_SESSIONS_EXPIRATION).unwrap();
    let mut sessions = ctx.sessions.write().await;
    let (session_id, nonce) = create_session(&mut sessions, expiration_time);

    let response = json!({ "id": session_id, "nonce": nonce });

    Ok(warp::reply::json(&response))
}

/// `PUT /v1/sessions/:session_id`
async fn session_signin_handler(
    ctx: Context,
    id: String,
    request: AuthRequest,
) -> Result<impl warp::Reply, warp::Rejection> {
    // Get unauthenticated session data, return early if not found
    let mut sessions = ctx.sessions.write().await;
    let session = sessions.get(&id).ok_or(Error::NotFound)?;

    if let AuthState::Unauthorized { nonce, .. } = session {
        let message = Message::from_str(request.message.as_str()).map_err(Error::from)?;

        let host = env::var("RADICLE_DOMAIN").map_err(Error::from)?;

        // Validate nonce
        if *nonce != message.nonce {
            return Err(Error::Auth("Invalid nonce").into());
        }

        // Verify that domain is the correct one
        let authority = Authority::from_str(&host).map_err(|_| Error::Auth("Invalid host"))?;
        if authority != message.domain {
            return Err(Error::Auth("Invalid domain").into());
        }

        // Verifies the following:
        // - AuthRequest sig matches the address passed in the AuthRequest message.
        // - expirationTime is not in the past.
        // - notBefore time is in the future.
        message
            .verify(request.signature.into())
            .map_err(Error::from)?;

        let session: Session = message.try_into()?;
        sessions.insert(id.clone(), AuthState::Authorized(session.clone()));

        return Ok(warp::reply::json(&json!({ "id": id, "session": session })));
    }

    Err(Error::Auth("Session already authorized").into())
}

async fn blob_handler(
    ctx: Context,
    project: Urn,
    sha: One,
    highlight: bool,
    path: warp::filters::path::Tail,
) -> Result<impl Reply, Rejection> {
    let theme = if highlight {
        Some(ctx.theme.as_str())
    } else {
        None
    };
    let reference = Reference::head(Namespace::from(project), None, sha);
    let blob = browse(reference, ctx.paths, |browser| {
        radicle_source::blob::highlighting::blob::<PeerId>(browser, None, path.as_str(), theme)
    })
    .await?;

    Ok(warp::reply::json(&blob))
}

async fn remotes_handler(ctx: Context, urn: Urn) -> Result<impl Reply, Rejection> {
    let storage = ctx.storage().await?;
    let project = identities::project::get(storage.read_only(), &urn)
        .map_err(Error::Identities)?
        .ok_or(Error::NotFound)?;
    let meta: project::Metadata = project.try_into().map_err(Error::Project)?;
    let response = project::tracked(&meta, storage.read_only())?;

    Ok(warp::reply::json(&response))
}

async fn remote_handler(
    ctx: Context,
    project: Urn,
    peer_id: PeerId,
) -> Result<impl Reply, Rejection> {
    let repo = git2::Repository::open_bare(ctx.paths.git_dir()).map_err(Error::from)?;
    // This is necessary to get any references to show up in the later calls. Go figure.
    let _ = repo.references().map_err(Error::from)?;

    let namespace = project.encode_id();
    let remote = peer_id.default_encoding();
    let prefix = format!(
        "refs/namespaces/{}/refs/remotes/{}/heads/",
        namespace, remote
    );
    let glob = format!("{}*", prefix);
    let refs = repo.references_glob(&glob).unwrap();

    let branches = refs
        .filter_map(|r| {
            let reference = r.ok()?;
            let name = reference.name()?.strip_prefix(&prefix)?;
            let oid = reference.target()?;

            Some((name.to_string(), oid.to_string()))
        })
        .collect::<HashMap<_, _>>();

    let response = json!({ "heads": &branches });

    Ok(warp::reply::json(&response))
}

async fn history_handler(
    ctx: Context,
    project: Urn,
    qs: CommitsQueryString,
) -> Result<impl Reply, Rejection> {
    let CommitsQueryString {
        since,
        until,
        parent,
        page,
        per_page,
        verified,
    } = qs;

    let (sha, fallback_to_head) = match parent {
        Some(commit) => (commit, false),
        None => {
            let info = ctx.project_info(project.to_owned()).await?;

            if let Some(head) = info.head {
                (head.to_string(), true)
            } else {
                return Err(Error::NoHead("project head is not set").into());
            }
        }
    };

    let reference = Reference::head(
        Namespace::from(project.to_owned()),
        None,
        One::from_str(&sha).map_err(|_| Error::NotFound)?,
    );

    let mut commits = browse(reference, ctx.paths.to_owned(), |browser| {
        radicle_source::commits::<PeerId>(browser, None)
    })
    .await?;

    // If a pagination is defined, we do not want to paginate the commits, and we return all of them on the first page.
    let page = page.unwrap_or(0);
    let per_page = if per_page.is_none() && (since.is_some() || until.is_some()) {
        commits.headers.len()
    } else {
        per_page.unwrap_or(30)
    };

    let projects = ctx.projects.read().await;
    let fingerprints = projects.get(&project);
    let headers = commits
        .headers
        .iter()
        .filter(|q| {
            if let (Some(since), Some(until)) = (since, until) {
                q.committer_time.seconds() >= since && q.committer_time.seconds() < until
            } else if let Some(since) = since {
                q.committer_time.seconds() >= since
            } else if let Some(until) = until {
                q.committer_time.seconds() < until
            } else {
                // If neither `since` nor `until` are specified, we include the commit.
                true
            }
        })
        .skip(page * per_page)
        .take(per_page)
        .map(|header| {
            let committer = if verified.unwrap_or_default() {
                let fp = ctx.commit_ssh_fingerprint(&header.sha1.to_string())?;
                if let (Some(fps), Some(fp)) = (fingerprints, fp) {
                    fps.get(&fp).cloned().map(|peer| Committer { peer })
                } else {
                    None
                }
            } else {
                None
            };

            Ok(CommitTeaser {
                header: header.clone(),
                context: CommitContext { committer },
            })
        })
        .collect::<Result<Vec<CommitTeaser>, Error>>()?;

    // Since the headers filtering can alter the amount of commits we have to recalculate it here.
    commits.stats.commits = headers.len();

    let response = json!({
        "headers": &headers,
        "stats": &commits.stats,
    });

    if fallback_to_head {
        return Ok(warp::reply::with_status(
            warp::reply::json(&response),
            StatusCode::FOUND,
        ));
    }

    Ok(warp::reply::with_status(
        warp::reply::json(&response),
        StatusCode::OK,
    ))
}

async fn commit_handler(ctx: Context, project: Urn, sha: One) -> Result<impl Reply, Rejection> {
    let reference = Reference::head(Namespace::from(project.clone()), None, sha.to_owned());
    let commit = browse(reference, ctx.paths.clone(), |browser| {
        let oid = browser.oid(&sha)?;
        radicle_source::commit(browser, oid)
    })
    .await?;

    let projects = ctx.projects.read().await;
    let fingerprints = projects.get(&project);
    let fp = ctx.commit_ssh_fingerprint(&commit.header.sha1.to_string())?;
    let committer = if let (Some(fps), Some(fp)) = (fingerprints, fp) {
        fps.get(&fp).cloned().map(|peer| Committer { peer })
    } else {
        None
    };

    let response = Commit {
        header: commit.header,
        diff: commit.diff,
        stats: commit.stats,
        branches: commit.branches,
        context: CommitContext { committer },
    };

    Ok(warp::reply::json(&response))
}

async fn readme_handler(ctx: Context, project: Urn, sha: One) -> Result<impl Reply, Rejection> {
    let reference = Reference::head(Namespace::from(project), None, sha);
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
            if let Ok(blob) =
                radicle_source::blob::highlighting::blob::<PeerId>(browser, None, path, None)
            {
                return Ok(blob);
            }
        }
        Err(radicle_source::Error::PathNotFound(
            Path::try_from("README").unwrap(),
        ))
    })
    .await?;

    Ok(warp::reply::json(&blob))
}

/// List all projects
async fn project_root_handler(ctx: Context) -> Result<Json, Rejection> {
    let storage = ctx.storage().await?;
    let repo = git2::Repository::open_bare(&ctx.paths.git_dir()).map_err(Error::from)?;
    let projects = identities::any::list(storage.read_only())
        .map_err(Error::from)?
        .filter_map(|res| {
            res.map(|id| match id {
                SomeIdentity::Project(project) => {
                    let meta: project::Metadata = project.try_into().ok()?;
                    let head =
                        get_head_commit(&repo, &meta.urn, &meta.default_branch, &meta.delegates)
                            .map(|h| h.id)
                            .ok();

                    Some(Info { meta, head })
                }
                _ => None,
            })
            .transpose()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Error::from)?;

    Ok(warp::reply::json(&projects))
}

async fn project_urn_handler(ctx: Context, urn: Urn) -> Result<Json, Rejection> {
    let info = ctx.project_info(urn).await?;

    Ok(warp::reply::json(&info))
}

async fn project_alias_handler(ctx: Context, name: String) -> Result<Json, Rejection> {
    let mut aliases = ctx.aliases.write().await;
    if !aliases.contains_key(&name) {
        // If the alias does not exist, rebuild the cache.
        ctx.populate_aliases(&mut aliases).await?;
    }
    let urn = aliases
        .get(&name)
        .cloned()
        .ok_or_else(warp::reject::not_found)?;

    project_urn_handler(ctx.clone(), urn).await
}

/// Fetch a [`radicle_source::Tree`].
async fn tree_handler(
    ctx: Context,
    project: Urn,
    sha: One,
    path: warp::filters::path::Tail,
) -> Result<impl Reply, Rejection> {
    let reference = Reference::head(Namespace::from(project), None, sha);
    let (tree, stats) = browse(reference, ctx.paths, |browser| {
        Ok((
            radicle_source::tree::<PeerId>(browser, None, Some(path.as_str().to_owned()))?,
            browser.get_stats()?,
        ))
    })
    .await?;
    let response = json!({
        "path": &tree.path,
        "entries": &tree.entries,
        "info": &tree.info,
        "stats": &stats,
    });

    Ok(warp::reply::json(&response))
}

/// List all projects that delegate is a part of.
async fn delegates_projects_handler(ctx: Context, delegate: Urn) -> Result<impl Reply, Rejection> {
    let storage = ctx.storage().await?;
    let repo = git2::Repository::open_bare(&ctx.paths.git_dir()).map_err(Error::from)?;
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

                    Some(Info { meta, head })
                }
                _ => None,
            })
            .transpose()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Error::from)?;

    Ok(warp::reply::json(&projects))
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

fn create_session(
    map: &mut HashMap<String, AuthState>,
    expiration_time: DateTime<Utc>,
) -> (String, String) {
    let nonce = siwe::nonce::generate_nonce();

    // We generate a value from the RNG for the session id
    let rng = fastrand::Rng::new();
    let id = hex::encode(repeat_with(|| rng.u8(..)).take(32).collect::<Vec<u8>>());

    let auth_state = AuthState::Unauthorized {
        nonce: nonce.clone(),
        expiration_time,
    };

    map.insert(id.clone(), auth_state);

    (id, nonce)
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
