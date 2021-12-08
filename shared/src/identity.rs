use std::{
    fs,
    fs::File,
    io::{self, Read as _},
    path::{Path, PathBuf},
};

use librad::{
    crypto::keystore::{
        crypto::{KdfParams, Pwhash},
        pinentry::SecUtf8,
        FileStorage, Keystore as _,
    },
    PublicKey, SecStr, SecretKey,
};

use crate::signer::Signer;

pub enum Identity {
    Plain(PathBuf),
    Encrypted { path: PathBuf, passphrase: SecUtf8 },
}

impl Identity {
    pub fn signer(self) -> Result<Signer, io::Error> {
        match self {
            Self::Plain(path) => {
                use librad::crypto::keystore::SecretKeyExt;

                let mut r = File::open(path)?;

                let mut bytes = Vec::new();
                r.read_to_end(&mut bytes)?;

                let sbytes: SecStr = bytes.into();
                match SecretKey::from_bytes_and_meta(sbytes, &()) {
                    Ok(key) => Ok(key.into()),
                    Err(err) => Err(io::Error::new(io::ErrorKind::InvalidData, err)),
                }
            }
            Self::Encrypted { path, passphrase } => {
                let crypto = Pwhash::new(passphrase, KdfParams::recommended());
                let store: FileStorage<_, PublicKey, SecretKey, _> =
                    FileStorage::new(&path, crypto);
                store
                    .get_key()
                    .map(|pair| pair.secret_key.into())
                    .map_err(|err| io::Error::new(io::ErrorKind::PermissionDenied, err))
            }
        }
    }
}

pub fn generate(path: &Path) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let mut file = File::create(path)?;
    let metadata = file.metadata()?;
    let mut permissions = metadata.permissions();

    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)?;

    let secret_key = SecretKey::new();
    file.write_all(secret_key.as_ref())?;

    Ok(())
}
