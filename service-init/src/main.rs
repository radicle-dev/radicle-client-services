use std::fs::File;
use std::path::PathBuf;

use librad::git::Storage;
use librad::paths::Paths;

use argh::FromArgs;

/// Radicle Services Initializer.
///
/// Initializes git storage for radicle services.
#[derive(FromArgs)]
pub struct Options {
    /// radicle root path, for key and git storage
    #[argh(option)]
    pub root: PathBuf,

    /// node identity file path
    #[argh(option)]
    pub identity: PathBuf,
}

fn main() {
    let options: Options = argh::from_env();
    shared::init_logger(shared::LogFmt::Plain);

    match run(&options) {
        Ok(()) => {
            tracing::info!("Storage initialized at {:?}", options.root.as_path());
        }
        Err(err) => {
            tracing::error!("Error initializing storage: {}", err);
        }
    }
}

fn run(options: &Options) -> Result<(), anyhow::Error> {
    let identity = options.identity.as_path();
    let paths = Paths::from_root(options.root.as_path())?;

    if !identity.exists() {
        shared::identity::generate(identity)?;
    }

    let signer = File::open(identity)?;
    let signer = shared::signer::Signer::new(signer)?;

    Storage::init(&paths, signer)?;

    Ok(())
}
