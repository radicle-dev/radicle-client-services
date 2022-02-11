#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]
pub mod error;

#[cfg(feature = "hooks")]
pub mod hooks;

use std::collections::HashMap;
use std::convert::Infallible;
use std::io::prelude::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::{io, net};

use either::Either;
use flate2::write::GzDecoder;
use http::{HeaderMap, Method};
use librad::git::identities;
use librad::git::storage::Pool;
use librad::git::{self, Urn};
use librad::identities::SomeIdentity;
use librad::paths::Paths;
use librad::profile::Profile;
use librad::PeerId;
use tokio::sync::RwLock;
use warp::hyper::StatusCode;
use warp::reply;
use warp::{self, path, Buf, Filter, Rejection, Reply};

use error::Error;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const STORAGE_POOL_SIZE: usize = 3;

#[derive(Debug, Clone)]
pub struct Options {
    pub root: Option<PathBuf>,
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
    git_receive_hook: PathBuf,
    allow_unauthorized_keys: bool,
    aliases: Arc<RwLock<HashMap<String, Urn>>>,
    pool: Pool<git::storage::ReadOnly>,
}

impl Context {
    fn from(options: &Options, pool: Pool<git::storage::ReadOnly>) -> Result<Self, Error> {
        let root_path = if let Some(root) = &options.root {
            root.to_owned()
        } else {
            let mut root = Profile::load()?.paths().git_dir().to_path_buf();
            // Remove the `/git/` directory to use the project (parent) directory as the root.
            root.pop();
            root
        };

        tracing::debug!("Root path set to: {:?}", root_path);

        let root = root_path.canonicalize()?;
        let git_receive_hook = root.join("hooks").join("receive-hook");

        Ok(Context {
            root,
            git_receive_pack: options.git_receive_pack,
            git_receive_hook,
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

    /// Disables the allowed signers file.
    /// We already have methods for authorizing keys, so this just gets in the way.
    pub fn disable_signers_file(&self) -> Result<(), Error> {
        let field = "gpg.ssh.allowedSignersFile";
        let value = "/dev/null";

        self.set_root_git_config(field, value)?;

        Ok(())
    }

    /// Enables users to submit a signed push: `push --signed`
    ///
    /// "You should set the certNonceSeed setting to some randomly generated long string that should
    /// be kept secret. It is combined with the timestamp to generate a one-time value (“nonce”)
    /// that the git client is required to sign and provides both a mechanism to prevent replay
    /// attacks and to offer proof that the certificate was generated for that specific server
    /// (though for others to verify this, they would need to know the value of the nonce seed)."
    pub fn set_cert_nonce_seed(&self) -> Result<(), Error> {
        let field = "receive.certNonceSeed";
        let value = self
            .cert_nonce_seed
            .clone()
            .unwrap_or_else(gen_random_string);

        self.set_root_git_config(field, &value)?;

        Ok(())
    }

    /// Sets the SLOP delay for signed push verification.
    ///
    /// "When a `git push --signed` sent a push certificate with a "nonce" that was issued by a
    /// receive-pack serving the same repository within this many seconds, export the "nonce" found
    /// in the certificate to GIT_PUSH_CERT_NONCE to the hooks (instead of what the receive-pack
    /// asked the sending side to include). This may allow writing checks in pre-receive and
    /// post-receive a bit easier. Instead of checking GIT_PUSH_CERT_NONCE_SLOP environment
    /// variable that records by how many seconds the nonce is stale to decide if they want to
    /// accept the certificate, they only can check GIT_PUSH_CERT_NONCE_STATUS is OK."
    pub fn set_cert_nonce_slop(&self) -> Result<(), Error> {
        let field = "receive.certNonceSlop";
        let value = 60; // Seconds.

        self.set_root_git_config(field, &value.to_string())?;

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
    async fn populate_aliases(&self, map: &mut HashMap<String, Urn>) -> Result<(), Error> {
        use librad::git::identities::SomeIdentity::Project;

        let storage = self.pool.get().await?;
        let identities = identities::any::list(&storage)?;

        for identity in identities.flatten() {
            if let Project(project) = identity {
                let urn = project.urn();
                let name = (&project.payload().subject.name).to_string();

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

    async fn get_meta(
        &self,
        urn: &Urn,
    ) -> Result<(Option<String>, Vec<PeerId>, Option<String>), Error> {
        let storage = self.pool.get().await?;
        let doc = identities::any::get(&storage, urn)?;

        if let Some(doc) = doc {
            let mut peer_ids = Vec::new();
            let mut default_branch = None;
            let mut name = None;

            match doc {
                SomeIdentity::Person(doc) => {
                    name = Some(doc.payload().subject.name.to_string());
                    peer_ids.extend(doc.delegations().iter().cloned().map(PeerId::from))
                }
                SomeIdentity::Project(doc) => {
                    name = Some(doc.payload().subject.name.to_string());
                    default_branch = Some(
                        doc.subject()
                            .default_branch
                            .clone()
                            .ok_or(Error::NoDefaultBranch)?
                            .to_string(),
                    );

                    for delegation in doc.delegations() {
                        match delegation {
                            Either::Left(pk) => peer_ids.push(PeerId::from(*pk)),
                            Either::Right(indirect) => {
                                peer_ids.extend(
                                    indirect.delegations().iter().cloned().map(PeerId::from),
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
            Ok((name, peer_ids, default_branch))
        } else {
            Ok((None, vec![], None))
        }
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
    let storage = git::storage::Pool::new(
        git::storage::pool::ReadConfig::new(paths),
        STORAGE_POOL_SIZE,
    );

    let ctx =
        Context::from(&options, storage).expect("Failed to create context from service options");
    {
        let mut aliases = ctx.aliases.write().await;

        ctx.populate_aliases(&mut aliases)
            .await
            .expect("Failed to populate aliases");
    }

    if let Err(e) = ctx.set_cert_nonce_seed() {
        tracing::error!(
            "Failed to set certificate nonce seed! required to enable `push --signed`: {:?}",
            e
        );
        std::process::exit(1);
    }
    if let Err(e) = ctx.set_cert_nonce_slop() {
        tracing::error!(
            "Failed to set certificate nonce slop! required to enable `push --signed`: {:?}",
            e
        );
        std::process::exit(1);
    }
    if let Err(e) = ctx.advertise_push_options() {
        tracing::error!(
            "Failed to set push config! required to enable `push --signed`: {:?}",
            e
        );
        std::process::exit(1);
    }
    if let Err(e) = ctx.disable_signers_file() {
        tracing::error!(
            "Failed to set signers file config! required to enable `push --signed`: {:?}",
            e
        );
        std::process::exit(1);
    }

    let peer_id = path::param::<PeerId>()
        .map(Some)
        .or_else(|_| async { Ok::<(Option<PeerId>,), Infallible>((None,)) });

    let server = warp::filters::any::any()
        .map(move || ctx.clone())
        .and(warp::method())
        .and(warp::filters::header::headers_cloned())
        .and(warp::filters::body::aggregate())
        .and(warp::filters::addr::remote())
        .and(path::param())
        .and(peer_id)
        .and(path::peek())
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

async fn git_handler(
    ctx: Context,
    method: Method,
    headers: HeaderMap,
    body: impl Buf,
    remote: Option<net::SocketAddr>,
    project_id: String,
    peer_id: Option<PeerId>,
    request: warp::filters::path::Peek,
    query: String,
) -> Result<Box<dyn Reply>, Rejection> {
    let remote = remote.expect("There is always a remote for HTTP connections");
    let urn = if let Some(name) = project_id.strip_suffix(".git") {
        if let Ok(urn) = Urn::try_from_id(name) {
            urn
        } else {
            tracing::debug!("looking for project alias {:?}", name);

            let mut aliases = ctx.aliases.write().await;
            if !aliases.contains_key(name) {
                // If the alias does not exist, rebuild the cache.
                ctx.populate_aliases(&mut aliases).await?;
            }
            let urn = aliases.get(name).cloned().ok_or(Error::AliasNotFound)?;
            tracing::debug!("project alias resolved to {}", urn);

            urn
        }
    } else {
        Urn::try_from_id(project_id).map_err(|_| Error::InvalidId)?
    };

    let (status, headers, body) = git(
        ctx,
        method,
        headers,
        body,
        remote,
        urn,
        peer_id,
        request.as_str(),
        query,
    )
    .await?;

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
    peer_id: Option<PeerId>,
    request: &str,
    query: String,
) -> Result<(http::StatusCode, HashMap<String, Vec<String>>, Vec<u8>), Error> {
    let namespace = urn.encode_id();
    let content_type =
        if let Some(Ok(content_type)) = headers.get("Content-Type").map(|h| h.to_str()) {
            content_type
        } else {
            ""
        };

    match (request, query.as_str()) {
        // Eg. `git push`
        ("git-receive-pack", _) | (_, "service=git-receive-pack") => {
            if !ctx.git_receive_pack {
                return Err(Error::ServiceUnavailable("git-receive-pack"));
            }
        }
        _ => {}
    }

    let path = Path::new("/git").join(request);
    let (name, delegates, default_branch) = ctx.get_meta(&urn).await?;
    let delegates_encoded = delegates
        .iter()
        .filter_map(|p| match to_ssh_fingerprint(p) {
            Ok(f) => Some(f),
            Err(_) => None,
        })
        .map(base64::encode);

    let authorized_keys = delegates_encoded
        .chain(ctx.authorized_keys.iter().cloned())
        .collect::<Vec<_>>();

    tracing::debug!("headers: {:?}", headers);
    tracing::debug!("namespace: {}", namespace);
    tracing::debug!("path: {:?}", path);
    tracing::debug!("method: {:?}", method.as_str());
    tracing::debug!("remote: {:?}", remote.to_string());
    tracing::debug!("delegates: {:?}", delegates);
    tracing::debug!("authorized keys: {:?}", authorized_keys);

    let mut cmd = Command::new("git");

    cmd.arg("http-backend");

    if !authorized_keys.is_empty() {
        cmd.env("RADICLE_AUTHORIZED_KEYS", authorized_keys.join(","));
    }
    if !delegates.is_empty() {
        cmd.env(
            "RADICLE_DELEGATES",
            delegates
                .iter()
                .map(|d| d.default_encoding())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    if ctx.allow_unauthorized_keys {
        cmd.env("RADICLE_ALLOW_UNAUTHORIZED_KEYS", "true");
    }
    if let Some(name) = name {
        cmd.env("RADICLE_NAME", name);
    }
    if let Some(peer_id) = peer_id {
        cmd.env("RADICLE_PEER_ID", peer_id.default_encoding());
    }
    if let Some(default_branch) = default_branch {
        cmd.env("RADICLE_DEFAULT_BRANCH", default_branch);
    }

    cmd.env("RADICLE_RECEIVE_HOOK", &ctx.git_receive_hook);
    cmd.env("REQUEST_METHOD", method.as_str());
    cmd.env("GIT_PROJECT_ROOT", &ctx.root);
    cmd.env("GIT_NAMESPACE", namespace);
    cmd.env("PATH_INFO", path);
    cmd.env("CONTENT_TYPE", content_type);
    // "The backend process sets GIT_COMMITTER_NAME to $REMOTE_USER and GIT_COMMITTER_EMAIL to
    // ${REMOTE_USER}@http.${REMOTE_ADDR}, ensuring that any reflogs created by git-receive-pack
    // contain some identifying information of the remote user who performed the push."
    cmd.env("REMOTE_USER", remote.ip().to_string());
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

    // Whether the request body is compressed.
    let gzip = matches!(
        headers.get("Content-Encoding").map(|h| h.to_str()),
        Some(Ok("gzip"))
    );

    {
        // This is safe because we captured the child's stdin.
        let mut stdin = child.stdin.take().unwrap();

        // Copy the request body to git-http-backend's stdin.
        if gzip {
            let mut decoder = GzDecoder::new(&mut stdin);
            let mut reader = body.reader();

            io::copy(&mut reader, &mut decoder)?;
            decoder.finish()?;
        } else {
            while body.has_remaining() {
                let mut chunk = body.chunk();
                let count = chunk.len();

                io::copy(&mut chunk, &mut stdin)?;
                body.advance(count);
            }
        }
    }

    match child.wait_with_output() {
        Ok(output) if output.status.success() => {
            tracing::info!("git-http-backend: exited successfully for {}", urn);

            let mut reader = std::io::Cursor::new(output.stdout);
            let mut headers = HashMap::new();

            // Parse headers returned by git so that we can use them in the client response.
            for line in io::Read::by_ref(&mut reader).lines() {
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

            let position = reader.position() as usize;
            let body = reader.into_inner().split_off(position);

            Ok((status, headers, body))
        }
        Ok(output) => {
            tracing::error!("git-http-backend: exited with code {}", output.status);

            if let Ok(output) = std::str::from_utf8(&output.stderr) {
                tracing::error!("git-http-backend: stderr: {}", output.trim_end());
            }
            Err(Error::Backend)
        }
        Err(err) => {
            panic!("failed to wait for git-http-backend: {}", err);
        }
    }
}

async fn recover(err: Rejection) -> Result<Box<dyn Reply>, Infallible> {
    let status = if err.is_not_found() {
        StatusCode::NOT_FOUND
    } else if let Some(error) = err.find::<Error>() {
        tracing::error!("{}", error);

        error.status()
    } else {
        StatusCode::BAD_REQUEST
    };

    Ok(Box::new(reply::with_status(String::default(), status)))
}

/// Helper method to generate random string for cert nonce;
fn gen_random_string() -> String {
    let rng = fastrand::Rng::new();
    let mut out = String::new();

    for _ in 0..12 {
        out.push(rng.alphanumeric());
    }
    out
}

/// Get the SSH key fingerprint from a peer id.
fn to_ssh_fingerprint(peer_id: &PeerId) -> Result<Vec<u8>, io::Error> {
    use byteorder::{BigEndian, WriteBytesExt};
    use sha2::Digest;

    let mut buf = Vec::new();
    let name = b"ssh-ed25519";
    let key = peer_id.as_public_key().as_ref();

    buf.write_u32::<BigEndian>(name.len() as u32)?;
    buf.extend_from_slice(name);
    buf.write_u32::<BigEndian>(key.len() as u32)?;
    buf.extend_from_slice(key);

    Ok(sha2::Sha256::digest(&buf).to_vec())
}

/// Parse a remote git ref into a peer id and return the remaining input.
///
/// Eg. `refs/remotes/<peer>/heads/master`
///
fn parse_ref(input: &str) -> Result<(PeerId, String), io::Error> {
    let suffix = input
        .strip_prefix("refs/remotes/")
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let (remote, rest) = suffix
        .split_once('/')
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let peer_id = PeerId::from_default_encoding(remote)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;

    Ok((peer_id, rest.to_owned()))
}
