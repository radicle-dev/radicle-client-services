pub mod identity;
pub mod signer;

mod logging;
pub use logging::{init_logger, LogFmt};

use std::path::PathBuf;

use librad::crypto::BoxedSigner;
use radicle_common::keys;
use radicle_common::profile;
use radicle_common::profile::{LnkHome, Profile};
use radicle_common::signer::ToSigner;

/// Load or create a profile, given an optional root path and passphrase.
pub fn profile(
    root: Option<PathBuf>,
    passphrase: Option<String>,
) -> anyhow::Result<(Profile, BoxedSigner)> {
    let home = if let Some(root) = root {
        LnkHome::Root(root)
    } else {
        LnkHome::default()
    };

    // If a profile isn't found, create one.
    let profile = if let Some(profile) = Profile::active(&home)? {
        profile
    } else if let Some(ref pass) = passphrase {
        let pwhash = keys::pwhash(pass.clone().into());
        let (profile, _) = profile::create(home, pwhash)?;

        profile
    } else {
        anyhow::bail!("No active profile and no passphrase supplied");
    };
    tracing::info!("Profile {} loaded...", profile.id());

    // Get the signer, either from the passphrase and secret key, or from ssh-agent.
    let signer = if let Some(pass) = passphrase {
        keys::load_secret_key(&profile, pass.into())?.to_signer(&profile)?
    } else if let Ok(sock) = keys::ssh_auth_sock() {
        sock.to_signer(&profile)?
    } else {
        anyhow::bail!("No signer found: ssh-agent isn't running, and no passphrase was supplied");
    };

    Ok((profile, signer))
}
