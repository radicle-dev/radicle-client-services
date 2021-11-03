//! # PRE-RECEIVE HOOK
//!
//! Before any ref is updated, if $GIT_DIR/hooks/pre-receive file exists and is executable,
//! it will be invoked once with no parameters.
//!
//! The standard input of the hook will be one line per ref to be updated:

//! `sha1-old SP sha1-new SP refname LF`
//!
//! The refname value is relative to $GIT_DIR; e.g. for the master head this is "refs/heads/master".
//! The two sha1 values before each refname are the object names for the refname before and after the update.
//! Refs to be created will have sha1-old equal to 0{40}, while refs to be deleted will have sha1-new equal to 0{40},
//! otherwise sha1-old and sha1-new should be valid objects in the repository.
//!
//! # Use by Radicle Git-Server
//!
//! The `pre-receive` git hook provides access to GPG certificates for a signed push, useful for authorizing an
//! update the repository.
use std::io::{stdin, Read};
use std::path::Path;
use std::str::FromStr;

use envconfig::Envconfig;
use git2::{Oid, Repository};
use pgp::{types::KeyTrait, Deserializable};

use super::{
    types::{CertNonceStatus, ReceivePackEnv},
    CertSignerDetails,
};
use crate::error::Error;

pub type KeyRing = Vec<String>;

pub const DEFAULT_RAD_KEYS_PATH: &str = ".rad/keys/openpgp/";

/// `PreReceive` provides access to the standard input values passed into the `pre-receive`
/// git hook, as well as parses environmental variables that may be used to process the hook.
#[derive(Debug, Clone)]
pub struct PreReceive {
    // Old SHA1
    pub old: Oid,
    // New SHA1
    pub new: Oid,
    // refname relative to $GIT_DIR
    pub refname: String,
    // Environmental Variables;
    pub env: ReceivePackEnv,
}

// use cert signer details default utility implementations.
impl CertSignerDetails for PreReceive {}

impl PreReceive {
    /// Instantiate from standard input.
    pub fn from_stdin() -> Result<Self, Error> {
        let mut buffer = String::new();
        stdin().read_to_string(&mut buffer)?;

        let input = buffer.split(' ').collect::<Vec<&str>>();

        // parse standard input variables;
        let old = Oid::from_str(input[0])?;
        let new = Oid::from_str(input[1])?;
        let refname = input[2].replace("\n", "");

        // initialize environmental values.
        let env = ReceivePackEnv::init_from_env()?;

        Ok(Self {
            old,
            new,
            refname,
            env,
        })
    }

    /// The main process used by `pre-receive` hook log
    pub fn hook() -> Result<(), Error> {
        let pre_receive = Self::from_stdin()?;

        // check if project exists.
        pre_receive.check_project_exists()?;

        // if allowed authorized keys is enabled, bypass the certificate check.
        if pre_receive.env.allow_unauthorized_keys {
            println!("SECURITY ALERT! UNAUTHORIZED KEYS ARE ALLOWED!");
            println!("Remove git-server flag `--allow-authorized-keys` to enforce GPG certificate verification");
            Ok(())
        } else {
            // Authenticate the request.
            pre_receive.authenticate()
        }
    }

    /// Authenticate the request by verifying the push signed certificate is valid and the GPG
    /// signing key is included in an authorized keyring.
    pub fn authenticate(&self) -> Result<(), Error> {
        // verify the certificate.
        self.verify_certificate()?;

        // ensure is authorized keys.
        self.check_authorized_key()?;

        Ok(())
    }

    pub fn check_project_exists(&self) -> Result<(), Error> {
        let repo = Repository::open(&self.env.git_dir)?;

        // set the namespace for the repo equal to the git namespace env.
        if repo.set_namespace(&self.env.git_namespace).is_err() {
            return Err(Error::NamespaceNotFound);
        }

        // check if the project has a radicle identity.
        if repo.find_reference("refs/rad/id").is_err() {
            return Err(Error::RadicleIdentityNotFound);
        }

        Ok(())
    }

    /// This method will succeed iff the cert status is "OK"
    pub fn verify_certificate(&self) -> Result<(), Error> {
        let status =
            CertNonceStatus::from_str(&self.env.cert_nonce_status.clone().unwrap_or_default())?;
        match status {
            // If we receive "OK", the certificate is verified using GPG.
            CertNonceStatus::OK => return Ok(()),
            // Received an invalid certificate status
            CertNonceStatus::UNKNOWN => {
                eprintln!("Invalid request, please sign push, i.e. `git push --sign ...`");
            }
            CertNonceStatus::SLOP => {
                eprintln!("Received `SLOP` certificate status, please re-submit signed push to request new certificate");
            }
            _ => {
                eprintln!("Received invalid certificate nonce status: {:?}", status);
            }
        }

        Err(Error::FailedCertificateVerification)
    }

    /// Check if the cert_key is found in an authorized keyring
    pub fn check_authorized_key(&self) -> Result<(), Error> {
        if let Some(key) = &self.env.cert_key {
            if self.authorized_keys()?.contains(key) || self.is_cert_authorized()? {
                // exit early, the key is included in the authorized keys list.
                return Ok(());
            }

            eprintln!("Unauthorized GPG Key: {:?}", key);
        }

        Err(Error::Unauthorized)
    }

    /// Return the parsed authorized keys from the provided environmental variable.
    pub fn authorized_keys(&self) -> Result<KeyRing, Error> {
        Ok(self
            .env
            .radicle_authorized_keys
            .clone()
            .map(|k| k.split(',').map(|k| k.to_owned()).collect::<KeyRing>())
            .unwrap_or_default())
    }

    /// Check the local repo .rad/keys/ directory for the GPG key matching the cert key
    /// used to sign the push certificate.
    pub fn is_cert_authorized(&self) -> Result<bool, Error> {
        if let Some(key) = self.env.cert_key.clone() {
            // search for the public key in the rad keys directory.
            let repo = Repository::open(&self.env.git_dir)?;

            // the path of the public key to verify.
            let key_path = Path::new(DEFAULT_RAD_KEYS_PATH).join(&key);

            // set the namespace for the repo equal to the git namespace env.
            repo.set_namespace(&self.env.git_namespace)?;

            let rfc = repo.find_reference(&self.refname)?;

            if let Ok(tree) = rfc.peel_to_tree() {
                if let Ok(entry) = tree.get_path(&key_path) {
                    let obj = entry.to_object(&repo)?;
                    let blob = obj.peel_to_blob()?;
                    let content = std::str::from_utf8(blob.content())?;
                    let (pk, _) = pgp::SignedPublicKey::from_string(content)?;

                    // verify the key on file.
                    pk.verify()?;

                    let key_id = hex::encode(pk.primary_key.key_id().as_ref()).to_uppercase();

                    // check the key matches the key from the signed push certificate.
                    return Ok(key_id == key);
                }
            };
        }

        Ok(false)
    }
}
