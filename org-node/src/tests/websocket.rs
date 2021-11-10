//! NOTE: org-node server must be running for these tests to succeed

use websocket::{url::Url, ClientBuilder, Message};

use crate::Error;

#[test]
fn test_websocket() -> Result<(), Error> {
    let url = Url::parse("ws://0.0.0.0:8336/subscribe").expect("invalid url");
    let mut client_builder = ClientBuilder::from_url(&url);
    let mut client = client_builder
        .connect_insecure()
        .expect("unable to connect to web socket");

    let msg = Message::text("radicle");

    client.send_message(&msg).expect("unable to send message");

    Ok(())
}
