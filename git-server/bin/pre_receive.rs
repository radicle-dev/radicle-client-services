//! `pre-receive` git hook binary.

use radicle_git_server::error::Error;

#[cfg(feature = "hooks")]
fn main() -> Result<(), Error> {
    use radicle_git_server::hooks::pre_receive::PreReceive;

    match PreReceive::hook() {
        Ok(()) => {
            eprintln!("Pre-receive hook success.");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
