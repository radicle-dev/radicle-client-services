//! `pre-receive` git hook binary.

use radicle_git_server::error::Error;

#[cfg(feature = "hooks")]
fn main() -> Result<(), Error> {
    use radicle_git_server::hooks::pre_receive::PreReceive;

    // run the `pre-receive` hook
    match PreReceive::hook() {
        Ok(()) => {
            println!("pre-receive hook success");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("{:?}", e);
            // exit with error and decline the pre-receive;
            std::process::exit(1)
        }
    }
}
