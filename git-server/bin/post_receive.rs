//! `post-receive` git hook binary.

use radicle_git_server::error::Error;

#[cfg(feature = "hooks")]
fn main() -> Result<(), Error> {
    use radicle_git_server::hooks::post_receive::PostReceive;

    match PostReceive::hook() {
        Ok(()) => {
            eprintln!("Post-receive hook success.");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1)
        }
    }
}
