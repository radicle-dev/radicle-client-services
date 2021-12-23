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
use std::io::prelude::*;
use std::io::stdin;

use std::path::Path;
use std::str::FromStr;

use envconfig::Envconfig;
use git2::{Oid, Repository};
use librad::PeerId;
use pgp::{types::KeyTrait, Deserializable};

use super::{
    types::{CertNonceStatus, CertStatus, ReceivePackEnv},
    CertSignerDetails,
};
use crate::error::Error;

pub type KeyRing = Vec<String>;

pub const DEFAULT_RAD_KEYS_PATH: &str = ".rad/keys/openpgp/";

/// `PreReceive` provides access to the standard input values passed into the `pre-receive`
/// git hook, as well as parses environmental variables that may be used to process the hook.
#[derive(Debug, Clone)]
pub struct PreReceive {
    /// Environmental Variables.
    pub env: ReceivePackEnv,
    /// Ref updates.
    pub updates: Vec<(String, Oid, Oid)>,
    /// Authorized keys as SSH key fingerprints.
    pub authorized_keys: Vec<String>,
}

// use cert signer details default utility implementations.
impl CertSignerDetails for PreReceive {}

impl PreReceive {
    /// Instantiate from standard input.
    pub fn from_stdin() -> Result<Self, Error> {
        let env = ReceivePackEnv::init_from_env()?;
        let mut updates = Vec::new();

        for line in stdin().lock().lines() {
            let line = line?;
            let input = line.split(' ').collect::<Vec<&str>>();

            let old = Oid::from_str(input[0])?;
            let new = Oid::from_str(input[1])?;
            let refname = input[2].to_owned();

            updates.push((refname, old, new));
        }

        let authorized_keys = env
            .authorized_keys
            .clone()
            .map(|k| k.split(',').map(|k| k.to_owned()).collect::<KeyRing>())
            .unwrap_or_default();

        Ok(Self {
            env,
            updates,
            authorized_keys,
        })
    }

    /// The main process used by `pre-receive` hook log
    pub fn hook() -> Result<(), Error> {
        eprintln!("Running pre-receive hook...");

        let pre_receive = Self::from_stdin()?;

        pre_receive.check_project_exists()?;
        pre_receive.authenticate()?;

        Ok(())
    }

    /// Authenticate the request by verifying the push signed certificate is valid and the GPG
    /// signing key is included in an authorized keyring.
    pub fn authenticate(&self) -> Result<(), Error> {
        self.authorize_ref_updates()?;
        self.verify_certificate()?;
        self.check_authorized_key()?;

        Ok(())
    }

    /// Authorizes each ref update, making sure the push certificate is signed by the same
    /// key as the owner/parent of the ref.
    pub fn authorize_ref_updates(&self) -> Result<(), Error> {
        // This is the fingerprint of the key used to sign the push certificate.
        let key_fingerprint = self
            .env
            .cert_key
            .as_ref()
            .ok_or(Error::Unauthorized("push certificate is not available"))?;
        let key_fingerprint = key_fingerprint
            .strip_prefix("SHA256:")
            .ok_or(Error::Unauthorized("key fingerprint is not a SHA-256 hash"))?;
        let key_fingerprint = base64::decode(key_fingerprint)
            .map_err(|_| Error::Unauthorized("key fingerprint is not valid"))?;

        // We iterate over each ref update and make sure they are all authorized. We need
        // to check that updates are only done to refs under `<project>/refs/remotes/<peer>`
        // for any give `<project>`, where `<peer>` is the identity of the signer.
        for (refname, _, _) in self.updates.iter() {
            // Get the peer/remote we are attempting to push to, and convert it to an SSH
            // key fingerpint.
            let suffix = refname
                .strip_prefix("refs/remotes/")
                .ok_or(Error::Unauthorized("ref name is not valid"))?;
            let (remote, _) = suffix
                .split_once('/')
                .ok_or(Error::Unauthorized("ref name is not valid"))?;
            let peer_id = PeerId::from_default_encoding(remote)
                .map_err(|_| Error::Unauthorized("ref must include a valid peer-id"))?;
            let peer_fingerprint = crate::to_ssh_fingerprint(&peer_id)?;

            if key_fingerprint[..] != peer_fingerprint[..] {
                return Err(Error::Unauthorized("signer does not match remote ref"));
            }
        }
        Ok(())
    }

    pub fn check_project_exists(&self) -> Result<(), Error> {
        let repo = Repository::open(&self.env.git_dir)?;

        // Set the namespace for the repo equal to the git namespace env.
        if repo.set_namespace(&self.env.git_namespace).is_err() {
            return Err(Error::NamespaceNotFound);
        }
        // Check if the project exists.
        if repo.find_reference("refs/rad/id").is_err() {
            return Err(Error::NamespaceNotFound);
        }
        Ok(())
    }

    /// This method will succeed iff the cert status is "OK"
    pub fn verify_certificate(&self) -> Result<(), Error> {
        eprintln!("Verifying certificate...");

        let status = CertStatus::from_str(self.env.cert_status.as_deref().unwrap_or_default())?;
        if !status.is_ok() {
            eprintln!("Bad signature for push certificate: {:?}", status);
            return Err(Error::FailedCertificateVerification);
        }

        let nonce_status =
            CertNonceStatus::from_str(self.env.cert_nonce_status.as_deref().unwrap_or_default())?;
        match nonce_status {
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
                eprintln!(
                    "Received invalid certificate nonce status: {:?}",
                    nonce_status
                );
            }
        }

        Err(Error::FailedCertificateVerification)
    }

    /// Check if the cert_key is found in an authorized keyring
    pub fn check_authorized_key(&self) -> Result<(), Error> {
        eprintln!("Authorizing...");

        if let Some(key) = &self.env.cert_key {
            if self.env.allow_unauthorized_keys.unwrap_or_default() {
                return Ok(());
            }
            eprintln!("Checking provided key {}...", key);

            if self.authorized_keys.contains(key) {
                eprintln!("Key {} is authorized to push.", key);
                return Ok(());
            }
        }

        Err(Error::Unauthorized("key is not in keyring"))
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

            let (refname, _, _) = &self.updates[0];
            let rfc = repo.find_reference(refname)?;

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
