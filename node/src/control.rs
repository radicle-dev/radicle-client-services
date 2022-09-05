//! Client control socket implementation.
use std::io::prelude::*;
use std::io::BufReader;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::{fs, io, net};

use crate::client;
use crate::client::handle::traits::ClientAPI;

/// Default name for control socket file.
pub const DEFAULT_SOCKET_NAME: &str = "radicle.sock";

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to bind control socket listener: {0}")]
    Bind(io::Error),
}

/// Listen for commands on the control socket, and process them.
pub fn listen<P: AsRef<Path>, H: ClientAPI>(path: P, handle: H) -> Result<(), Error> {
    // Remove the socket file on startup before rebinding.
    fs::remove_file(&path).ok();

    let listener = UnixListener::bind(path).map_err(Error::Bind)?;
    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => {
                if let Err(e) = drain(&stream, &handle) {
                    log::error!("Received {} on control socket", e);

                    writeln!(stream, "error: {}", e).ok();

                    stream.flush().ok();
                    stream.shutdown(net::Shutdown::Both).ok();
                } else {
                    writeln!(stream, "ok").ok();
                }
            }
            Err(e) => log::error!("Failed to open control socket stream: {}", e),
        }
    }

    Ok(())
}

#[derive(thiserror::Error, Debug)]
enum DrainError {
    #[error("invalid command argument `{0}`")]
    InvalidCommandArg(String),
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("invalid command")]
    InvalidCommand,
    #[error("client error: {0}")]
    Client(#[from] client::handle::Error),
}

fn pop_record(rec: &csv::StringRecord) -> (Option<&str>, csv::StringRecord) {
    let mut ret = csv::StringRecord::new();
    for i in 1..rec.len() {
        let f = rec.get(i).unwrap();
        ret.push_field(f);
    }
    (rec.get(0), ret)
}

fn drain<S: ClientAPI>(stream: &UnixStream, srv: &S) -> Result<(), DrainError> {
    let no_csv_header = None;

    let reader = BufReader::new(stream);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .delimiter(b' ')
        .from_reader(reader);

    for record in reader.records() {
        let record = if let Ok(v) = record {
            v
        } else {
            // parse error
            continue;
        };

        let (cmd, args) = pop_record(&record);
        match cmd {
            Some("other_command") => {
                let a = if let Ok(v) = args.deserialize(no_csv_header) {
                    v
                } else {
                    return Err(DrainError::InvalidCommandArg(
                        args.get(0).unwrap().to_owned(),
                    ));
                };

                if let Err(e) = srv.other_command(&a) {
                    return Err(DrainError::Client(e));
                }
            }
            Some("update") => {
                let args = if let Ok(v) = args.deserialize(no_csv_header) {
                    v
                } else {
                    return Err(DrainError::InvalidCommandArg(
                        args.get(0).unwrap().to_owned(),
                    ));
                };

                if let Err(e) = srv.notify_update(args) {
                    return Err(DrainError::Client(e));
                }
            }
            Some(cmd) => return Err(DrainError::UnknownCommand(cmd.to_owned())),
            None => return Err(DrainError::InvalidCommand),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::prelude::*;
    use std::os::unix::net::UnixStream;
    use std::{net, thread};

    use super::*;
    use crate::identity::ProjId;
    use crate::test;

    #[test]
    fn test_control_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let handle = test::handle::Handle::default();
        let socket = tmp.path().join("alice.sock");
        let projs = test::arbitrary::set::<ProjId>(1..3);

        thread::spawn({
            let socket = socket.clone();
            let handle = handle.clone();

            move || listen(socket, handle)
        });

        let mut stream = loop {
            if let Ok(stream) = UnixStream::connect(&socket) {
                break stream;
            }
        };
        for proj in &projs {
            writeln!(&stream, "update {}", proj).unwrap();
        }

        let mut buf = [0; 2];
        stream.shutdown(net::Shutdown::Write).unwrap();
        stream.read_exact(&mut buf).unwrap();

        assert_eq!(&buf, &[b'o', b'k']);
        for proj in &projs {
            assert!(handle.updates.lock().unwrap().contains(proj));
        }
    }
}
