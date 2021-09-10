#![allow(clippy::type_complexity)]
#![allow(clippy::too_many_arguments)]
mod error;

use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{io, net};

use http::{HeaderMap, Method};
use warp::hyper::StatusCode;
use warp::reply;
use warp::{self, path, Buf, Filter, Rejection, Reply};

use error::Error;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    pub listen: net::SocketAddr,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub git_receive_pack: bool,
}

#[derive(Debug, Clone)]
pub struct Context {
    root: PathBuf,
    git_receive_pack: bool,
}

/// Run the Git Server.
pub async fn run(options: Options) {
    let ctx = Context {
        root: options.root,
        git_receive_pack: options.git_receive_pack,
    };
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
    let remote = remote.expect("there is always a remote for HTTP connections");
    let (status, headers, body) = git(
        ctx, method, headers, body, remote, namespace, request, query,
    )?;
    let mut builder = http::Response::builder().status(status);

    for (name, vec) in headers.iter() {
        for value in vec {
            builder = builder.header(name, value);
        }
    }
    let response = builder.body(body).map_err(Error::from)?;

    Ok(Box::new(response))
}

fn git(
    ctx: Context,
    method: Method,
    headers: HeaderMap,
    mut body: impl Buf,
    remote: net::SocketAddr,
    namespace: String,
    request: warp::filters::path::Tail,
    query: String,
) -> Result<(http::StatusCode, HashMap<String, Vec<String>>, Vec<u8>), Error> {
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

    let mut cmd = Command::new("git");

    cmd.arg("http-backend");

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
