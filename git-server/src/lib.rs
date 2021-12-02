#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]
pub mod error;

#[cfg(feature = "hooks")]
pub mod hooks;

use std::collections::HashMap;
use std::convert::TryFrom;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::{io, net};

use http::{HeaderMap, Method};
use librad::git::storage::Pool;
use librad::git::{self, Urn};
use librad::paths::Paths;
use librad::profile::Profile;
use rand::{distributions::Alphanumeric, thread_rng, Rng};
use warp::hyper::StatusCode;
use warp::reply;
use warp::{self, path, Buf, Filter, Rejection, Reply};

use error::Error;

use radicle_source::surf::vcs::git::namespace::Namespace;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const STORAGE_POOL_SIZE: usize = 3;

#[derive(Debug, Clone)]
pub struct Options {
    pub root: Option<PathBuf>,
    pub identity: PathBuf,
    pub identity_passphrase: Option<String>,
    pub listen: net::SocketAddr,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub git_receive_pack: bool,
    pub authorized_keys: Vec<String>,
    pub cert_nonce_seed: Option<String>,
    pub allow_unauthorized_keys: bool,
}

#[derive(Clone)]
pub struct Context {
    root: PathBuf,
    git_receive_pack: bool,
    authorized_keys: Vec<String>,
    cert_nonce_seed: Option<String>,
    allow_unauthorized_keys: bool,
    aliases: Arc<RwLock<HashMap<String, Namespace>>>,
    pool: Pool<git::Storage>,
}

impl Context {
    fn from(options: &Options, pool: Pool<git::Storage>) -> Result<Self, Error> {
        let root_path = if let Some(root) = &options.root {
            root.to_owned()
        } else {
            let mut root = Profile::load()?.paths().git_dir().to_path_buf();
            // Remove the `/git/` directory to use the project (parent) directory as the root.
            root.pop();
            root
        };

        tracing::debug!("Root path set to: {:?}", root_path);

        Ok(Context {
            root: root_path.canonicalize()?,
            git_receive_pack: options.git_receive_pack,
            authorized_keys: options.authorized_keys.clone(),
            cert_nonce_seed: options.cert_nonce_seed.clone(),
            allow_unauthorized_keys: options.allow_unauthorized_keys,
            aliases: Default::default(),
            pool,
        })
    }

    /// Sets the config receive.advertisePushOptions, which lets the user known they can provide a push option `-o`,
    /// to specify unique attributes. This is currently not used, but may be used in the future.
    pub fn advertise_push_options(&self) -> Result<(), Error> {
        let field = "receive.advertisePushOptions";
        let value = "true";

        self.set_root_git_config(field, value)?;

        Ok(())
    }

    /// Enables users to submit a signed push: `push --signed`
    pub fn set_cert_nonce_seed(&self) -> Result<(), Error> {
        let field = "receive.certNonceSeed";
        let value = self
            .cert_nonce_seed
            .clone()
            .unwrap_or_else(gen_random_string);

        self.set_root_git_config(field, &value)?;

        Ok(())
    }

    /// updates the git config in the root project
    pub fn set_root_git_config(&self, field: &str, value: &str) -> Result<(), Error> {
        let path = self.root.clone().join("git/config");

        tracing::debug!("Searching for git config at: {:?}", path);

        let mut config = git2::Config::open(&path)?;

        config.set_str(field, value)?;

        Ok(())
    }

    /// Populates alias map with unique projects' names and their urns
    async fn populate_aliases(&self) -> Result<(), Error> {
        use librad::git::identities;
        use librad::git::identities::SomeIdentity::Project;

        let storage = self.pool.get().await?;
        let read_only = &storage.read_only();
        let identities = identities::any::list(read_only)?;
        let mut map = self.aliases.write().unwrap();

        for identity in identities.flatten() {
            if let Project(project) = identity {
                let urn = Namespace::try_from(project.urn().encode_id().as_str()).unwrap();
                let name = (&project.payload().subject.name).to_string() + ".git";

                tracing::info!("alias {:?} for {:?}", name, urn.to_string());

                if let std::collections::hash_map::Entry::Vacant(e) = map.entry(name.clone()) {
                    e.insert(urn);
                } else {
                    tracing::warn!("alias {:?} exists, skipping", name);
                }
            }
        }

        Ok(())
    }
}

/// Run the Git Server.
pub async fn run(options: Options) {
    let git_version = Command::new("git")
        .arg("version")
        .output()
        .expect("'git' command must be available")
        .stdout;
    tracing::info!("{}", std::str::from_utf8(&git_version).unwrap().trim());

    let paths = if let Some(ref root) = options.root {
        Paths::from_root(root).unwrap()
    } else {
        Profile::load().unwrap().paths().clone()
    };
    let identity_path = options.identity.clone();
    let identity = if let Some(passphrase) = options.identity_passphrase.clone() {
        shared::identity::Identity::Encrypted {
            path: identity_path.clone(),
            passphrase: passphrase.into(),
        }
    } else {
        shared::identity::Identity::Plain(identity_path.clone())
    };
    let signer = identity
        .signer()
        .unwrap_or_else(|e| panic!("unable to load identity {:?}: {}", &identity_path, e));
    let storage_lock = git::storage::pool::Initialised::no();
    let storage = git::storage::Pool::new(
        git::storage::pool::ReadWriteConfig::new(paths, signer, storage_lock),
        STORAGE_POOL_SIZE,
    );

    let ctx =
        Context::from(&options, storage).expect("Failed to create context from service options");
    ctx.populate_aliases()
        .await
        .expect("Failed to populate aliases");

    if let Err(e) = ctx.set_cert_nonce_seed() {
        tracing::error!(
            "Failed to set certificate nonce seed! required to enable `push --signed`: {:?}",
            e
        );

        std::process::exit(1);
    }

    let server = warp::filters::any::any()
        .map(move || ctx.clone())
        .and(warp::method())
        .and(warp::filters::header::headers_cloned())
        .and(warp::filters::body::aggregate())
        .and(warp::filters::addr::remote())
        .and(path::param())
        .and(path::tail())
        .and(
            warp::filters::query::raw()
                .or(warp::any().map(String::default))
                .unify(),
        )
        .and_then(git_handler)
        .recover(recover)
        .with(warp::log("radicle_git_server"));
    let server = warp::serve(server);

    if let (Some(cert), Some(key)) = (options.tls_cert, options.tls_key) {
        server
            .tls()
            .cert_path(cert)
            .key_path(key)
            .run(options.listen)
            .await
    } else {
        server.run(options.listen).await
    }
}

fn authenticate(headers: &HeaderMap) -> Result<String, Error> {
    if let Some(Ok(auth)) = headers.get("Authorization").map(|h| h.to_str()) {
        if let Some(encoded) = auth.strip_prefix("Basic ") {
            let decoded = base64::decode(encoded).map_err(|_| Error::InvalidAuthorization)?;
            let credentials =
                String::from_utf8(decoded).map_err(|_| Error::InvalidAuthorization)?;
            let mut parts = credentials.splitn(2, ':');
            let username = parts.next().ok_or(Error::InvalidAuthorization)?;
            let _password = parts.next().ok_or(Error::InvalidAuthorization)?;

            return Ok(username.to_owned());
        } else {
            return Err(Error::InvalidAuthorization);
        }
    }
    Err(Error::Unauthorized)
}

async fn git_handler(
    ctx: Context,
    method: Method,
    headers: HeaderMap,
    body: impl Buf,
    remote: Option<net::SocketAddr>,
    namespace: String,
    request: warp::filters::path::Tail,
    query: String,
) -> Result<Box<dyn Reply>, Rejection> {
    let remote = remote.expect("There is always a remote for HTTP connections");
    let urn = if namespace.ends_with(".git") {
        let ns_id = namespace.strip_suffix(".git").unwrap();
        if let Ok(urn) = Urn::try_from_id(ns_id) {
            urn
        } else {
            // not a project-id, thus potentially an alias
            // if the alias does not exist, rebuild the cache
            if !ctx.aliases.read().unwrap().contains_key(&namespace) {
                ctx.populate_aliases().await?;
            }
            Urn::try_from_id(
                ctx.aliases
                    .read()
                    .unwrap()
                    .get(&namespace)
                    .map(Namespace::to_string)
                    .unwrap_or(namespace),
            )
            .unwrap()
        }
    } else {
        Urn::try_from_id(namespace).unwrap()
    };

    let (status, headers, body) =
        git(ctx, method, headers, body, remote, urn, request, query).await?;

    let mut builder = http::Response::builder().status(status);

    for (name, vec) in headers.iter() {
        for value in vec {
            builder = builder.header(name, value);
        }
    }
    let response = builder.body(body).map_err(Error::from)?;

    Ok(Box::new(response))
}

async fn git(
    ctx: Context,
    method: Method,
    headers: HeaderMap,
    mut body: impl Buf,
    remote: net::SocketAddr,
    urn: Urn,
    request: warp::filters::path::Tail,
    query: String,
) -> Result<(http::StatusCode, HashMap<String, Vec<String>>, Vec<u8>), Error> {
    let namespace = urn.encode_id();
    let content_type =
        if let Some(Ok(content_type)) = headers.get("Content-Type").map(|h| h.to_str()) {
            content_type
        } else {
            ""
        };
    let request = request.as_str();
    let path = Path::new("/git").join(request);

    let username = match (request, query.as_str()) {
        // Eg. `git push`
        ("git-receive-pack", _) | (_, "service=git-receive-pack") => {
            if ctx.git_receive_pack {
                if let Ok(username) = authenticate(&headers) {
                    username
                } else {
                    return Err(Error::Unauthorized);
                }
            } else {
                return Err(Error::ServiceUnavailable("git-receive-pack"));
            }
        }
        // Other
        _ => String::default(),
    };

    tracing::debug!("namespace: {}", namespace);
    tracing::debug!("path: {:?}", path);
    tracing::debug!("username: {:?}", username);
    tracing::debug!("method: {:?}", method.as_str());
    tracing::debug!("remote: {:?}", remote.to_string());

    let mut cmd = Command::new("git");

    cmd.arg("http-backend");

    // Set Authorized Keys for verifying GPG signatures against key IDs.
    cmd.env("RADICLE_AUTHORIZED_KEYS", ctx.authorized_keys.join(","));

    if ctx.allow_unauthorized_keys {
        cmd.env("RADICLE_ALLOW_UNAUTHORIZED_KEYS", "true");
    }

    cmd.env("REQUEST_METHOD", method.as_str());
    cmd.env("GIT_PROJECT_ROOT", &ctx.root);
    cmd.env("GIT_NAMESPACE", namespace);
    cmd.env("PATH_INFO", path);
    cmd.env("CONTENT_TYPE", content_type);
    // "The backend process sets GIT_COMMITTER_NAME to $REMOTE_USER and GIT_COMMITTER_EMAIL to
    // ${REMOTE_USER}@http.${REMOTE_ADDR}, ensuring that any reflogs created by git-receive-pack
    // contain some identifying information of the remote user who performed the push."
    cmd.env("REMOTE_USER", username);
    cmd.env("REMOTE_ADDR", remote.to_string());
    cmd.env("QUERY_STRING", query);
    // "The GIT_HTTP_EXPORT_ALL environmental variable may be passed to git-http-backend to bypass
    // the check for the "git-daemon-export-ok" file in each repository before allowing export of
    // that repository."
    cmd.env("GIT_HTTP_EXPORT_ALL", String::default());
    cmd.stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .stdin(Stdio::piped());

    // Spawn the git backend.
    let mut child = cmd.spawn()?;

    // Copy the request body to git-http-backend's stdin.
    // CGI requires gzip support, but we're not going to support that.
    if let Some(Ok("gzip")) = headers.get("Content-Encoding").map(|h| h.to_str()) {
        return Err(Error::UnsupportedContentEncoding("gzip"));
    } else {
        // This is safe because we captured the child's stdin.
        let mut stdin = child.stdin.take().unwrap();

        while body.has_remaining() {
            let mut chunk = body.chunk();
            let count = chunk.len();

            io::copy(&mut chunk, &mut stdin)?;
            body.advance(count);
        }
    }

    let mut reader = io::BufReader::new(child.stdout.take().unwrap());
    let mut headers = HashMap::new();

    // Parse headers returned by git so that we can use them in the client response.
    for line in reader.by_ref().lines() {
        let line = line?;

        if line.is_empty() || line == "\r" {
            break;
        }

        let mut parts = line.splitn(2, ':');
        let key = parts.next();
        let value = parts.next();

        if let (Some(key), Some(value)) = (key, value) {
            let value = &value[1..];

            headers
                .entry(key.to_string())
                .or_insert_with(Vec::new)
                .push(value.to_string());
        } else {
            return Err(Error::Backend);
        }
    }

    let status = {
        tracing::debug!("http-backend: {:?}", &headers);

        let line = headers.remove("Status").unwrap_or_default();
        let line = line.into_iter().next().unwrap_or_default();
        let mut parts = line.split(' ');

        parts
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(StatusCode::OK)
    };

    // Read stderr.
    if let Some(mut out) = child.stderr.take() {
        let mut buf = Vec::new();

        while let Ok(count) = out.read(&mut buf) {
            if count == 0 {
                break;
            }
        }
        if let Ok(err) = String::from_utf8(buf) {
            if !err.trim().is_empty() {
                tracing::error!("http-backend: {}", err);
            }
        }
    }

    let mut body = Vec::new();
    reader.read_to_end(&mut body)?;

    match child.wait() {
        Ok(status) if status.success() => {
            tracing::info!("git-http-backend exited successfully for {}", urn);
        }
        Ok(status) => {
            tracing::error!("git-http-backend exited with code {}", status);
        }
        Err(err) => {
            panic!("failed to wait for git-http-backend: {}", err);
        }
    }

    Ok((status, headers, body))
}

async fn recover(err: Rejection) -> Result<Box<dyn Reply>, std::convert::Infallible> {
    let status = if err.is_not_found() {
        StatusCode::NOT_FOUND
    } else if let Some(error) = err.find::<Error>() {
        tracing::error!("{}", error);

        if let Error::Unauthorized = error {
            return Ok(Box::new(reply::with_header(
                reply::with_status(String::default(), http::StatusCode::UNAUTHORIZED),
                http::header::WWW_AUTHENTICATE,
                r#"Basic realm="radicle", charset="UTF-8""#,
            )));
        }
        error.status()
    } else {
        StatusCode::BAD_REQUEST
    };

    Ok(Box::new(reply::with_status(String::default(), status)))
}

// Helper method to generate random string for cert nonce;
fn gen_random_string() -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(12)
        .map(char::from)
        .collect()
}
