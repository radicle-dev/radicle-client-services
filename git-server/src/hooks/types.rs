use std::path::PathBuf;
use std::str::FromStr;

use crate::error::Error;

use envconfig::Envconfig;

/// `CertNonceStatus` describes the status of verifying the signed nonce from
/// the user. If it does not match "OK", the `pre-receive` hook should fail unsuccessfully
#[derive(Debug, Clone)]
pub enum CertNonceStatus {
    /// "git push --signed" sent a nonce when we did not ask it to send one.
    UNSOLICITED,
    /// "git push --signed" did not send any nonce header.
    MISSING,
    /// "git push --signed" sent a bogus nonce.
    BAD,
    /// "git push --signed" sent the nonce we asked it to send.
    OK,
    /// "git push --signed" sent a nonce different from what we asked it to send now, but in a previous session.
    /// See GIT_PUSH_CERT_NONCE_SLOP environment variable.
    SLOP,
    /// Unknown type; not associated with git.
    UNKNOWN,
}

impl Default for CertNonceStatus {
    fn default() -> Self {
        Self::UNKNOWN
    }
}

impl FromStr for CertNonceStatus {
    type Err = Error;

    fn from_str(s: &str) -> Result<CertNonceStatus, Self::Err> {
        Ok(match s {
            "UNSOLICITED" => Self::UNSOLICITED,
            "MISSING" => Self::MISSING,
            "BAD" => Self::BAD,
            "OK" => Self::OK,
            "SLOP" => Self::SLOP,
            _ => Self::UNKNOWN,
        })
    }
}

/// `CertStatus` describes the status of verifying the GPG signature.
#[derive(Debug, Clone)]
pub enum CertStatus {
    GoodValid,
    GoodUnknown,
    GoodExpiredSignature,
    GoodExpiredKey,
    GoodRevokedKey,
    Bad,
    Error,
    NoSignature,
    Unknown,
}

impl CertStatus {
    /// Check whether we should authorize this cert signature.
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::GoodValid | Self::GoodUnknown)
    }
}

impl FromStr for CertStatus {
    type Err = Error;

    // From the git docs:
    //
    //   Show "G" for a good (valid) signature, "B" for a bad signature, "U" for a good
    //   signature with unknown validity, "X" for a good signature that has expired, "Y" for a
    //   good signature made by an expired key, "R" for a good signature made by a revoked key,
    //   "E" if the signature cannot be checked (e.g. missing key) and "N" for no signature
    //
    fn from_str(s: &str) -> Result<CertStatus, Self::Err> {
        Ok(match s {
            "G" => Self::GoodValid,
            "U" => Self::GoodUnknown,
            "X" => Self::GoodExpiredSignature,
            "Y" => Self::GoodExpiredKey,
            "R" => Self::GoodRevokedKey,
            "B" => Self::Bad,
            "E" => Self::Error,
            "N" => Self::NoSignature,
            _ => Self::Unknown,
        })
    }
}

/// `ReceivePackEnv` provides access to environmental variables set and used by git-http-backend
/// when a `receive-pack` event is triggered. The values are used by both the `pre-receive` and `post-receive`
/// hooks within the `receive-pack` hook lifecycle.
///
/// Certificate variables are set by the git-http-backend process, while other variables are set
/// when receiving a new git client request and passing those variables to http-backend.
///
/// These variables are not exhaustive; other variables may be set by the backend process, but are unused.
#[derive(Debug, Default, Clone, Envconfig)]
pub struct ReceivePackEnv {
    /// Object Id of the blob where the signed certificate exists.
    #[envconfig(from = "GIT_PUSH_CERT")]
    pub cert: Option<String>,

    /// The name and the e-mail address of the owner of the key that signed the push certificate.
    #[envconfig(from = "GIT_PUSH_CERT_SIGNER")]
    pub cert_signer: Option<String>,

    /// The GPG key ID of the key that signed the push certificate.
    #[envconfig(from = "GIT_PUSH_CERT_KEY")]
    pub cert_key: Option<String>,

    /// The status of GPG verification of the push certificate,
    /// using the same mnemonic as used in %G? format of git log family of commands (see [git-log](https://git-scm.com/docs/git-log)).
    #[envconfig(from = "GIT_PUSH_CERT_STATUS")]
    pub cert_status: Option<String>,

    /// The nonce string the process asked the signer to include in the push certificate.
    /// If this does not match the value recorded on the "nonce" header in the push certificate,
    /// it may indicate that the certificate is a valid one that is being replayed from a separate "git push" session.
    #[envconfig(from = "GIT_PUSH_CERT_NONCE")]
    pub cert_nonce: Option<String>,

    /// cert nonce status is used as a determinant whether the certificate was correctly signed by the user.
    /// only an `OK` status will be accepted for authorization.
    #[envconfig(from = "GIT_PUSH_CERT_NONCE_STATUS")]
    pub cert_nonce_status: Option<String>,

    /// "git push --signed" sent a nonce different from what we asked it to send now,
    /// but in a different session whose starting time is different by this many seconds from the current session.
    /// Only meaningful when GIT_PUSH_CERT_NONCE_STATUS says SLOP.
    /// Also read about receive.certNonceSlop variable in [git-config](https://git-scm.com/docs/git-config).
    #[envconfig(from = "GIT_PUSH_CERT_NONCE_SLOP")]
    pub cert_nonce_slop: Option<String>,

    /// comma delimited list of SSH key fingerprints authorized for the push.
    #[envconfig(from = "RADICLE_AUTHORIZED_KEYS")]
    pub authorized_keys: Option<String>,

    /// allow unauthorized keys, ignores push certificate verification.
    #[envconfig(from = "RADICLE_ALLOW_UNAUTHORIZED_KEYS")]
    pub allow_unauthorized_keys: Option<bool>,

    /// root directory where `git` directory is found.
    #[envconfig(from = "GIT_PROJECT_ROOT")]
    pub git_project_root: String,

    /// namespace of the target repository.
    #[envconfig(from = "GIT_NAMESPACE")]
    pub git_namespace: String,

    /// The backend process sets GIT_COMMITTER_NAME to $REMOTE_USER
    /// and GIT_COMMITTER_EMAIL to ${REMOTE_USER}@http.${REMOTE_ADDR},
    /// ensuring that any reflogs created by git-receive-pack contain
    /// some identifying information of the remote user who performed
    /// the push.
    ///
    /// NOTE: `remote_user` and `remote_addr` are set by http basic
    /// authentication: i.e. username and password
    ///
    /// `git-receive-pack` and `pre-receive` hook check for a signed push,
    /// which if remote user is set, it should match the signer of the push
    /// to verify basic authentication matches signer email.
    #[envconfig(from = "REMOTE_USER")]
    pub remote_user: Option<String>,

    /// remote address of the socket making the request to the git-server.
    /// set by the git-server.
    #[envconfig(from = "REMOTE_ADDR")]
    pub remote_addr: Option<String>,

    /// should match `REMOTE_USER`
    #[envconfig(from = "GIT_COMMITTER_NAME")]
    pub git_committer_name: Option<String>,

    /// GIT_COMMITTER_EMAIL is set to ${REMOTE_USER}@http.${REMOTE_ADDR} by the git-http-backend;
    /// NOTE: it will likely not match the certificate signer email, as these values are set
    /// by different tools and services.
    #[envconfig(from = "GIT_COMMITTER_EMAIL")]
    pub git_committer_email: Option<String>,

    /// HTTP header set by the git-server.
    #[envconfig(from = "CONTENT_TYPE")]
    pub content_type: String,

    /// HTTP query string set by the git-server.
    #[envconfig(from = "QUERY_STRING")]
    pub query_string: String,

    /// top-level git directory, set by the git-http-backend.
    #[envconfig(from = "GIT_DIR")]
    pub git_dir: PathBuf,
}
