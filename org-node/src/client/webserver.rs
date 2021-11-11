use futures_util::{SinkExt, StreamExt, TryFutureExt};
use tokio::{
    sync::mpsc::{self},
    task::spawn,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::{
    ws::{Message, WebSocket, Ws},
    Filter,
};

use crate::client::MessageBus;
use crate::Error;

//
//
//
//
//                       +----------------------------+
//                       |                            |
//                       |                            |
//                       |     WebSocket Service      |
//  Peer Connection      |                            |
//  Established          |    +-----------------+     |
// +-----------------+   |    |                 |     |
// |                 +---+---->   Peer          |     |
// |  Peer Client    |   |    |   Connection    |     |
// |                 <---+----+                 |     |
// +-----------------+   |    |                 |     |
// Subscribe to events   |    +-------^----+----+     |
//                       |            |    |          |
//                       |            |    |          |
//                       +------------+----+----------+
//                                    |    |
//            Broadcast message bus   |    |  Send "WebSocketConnection"
//            events to connected ws  |    |  to message bus channel
//            clients                 |    |  with an UnboundedSender<String>
//                                    |    |
//                                    |    |
//                      +-------------+----v----------+
//                      |             |               |
//                      |     +-------+---------+     |
//   Msg Bus receives   |     | Connected Peers |     |
//   protocol events    +-----> mapping to tx   |     |
// +-----------------+  |     | peer handles    |     |
// |                 +-->     +-----------------+     |
// | Protocol Events |  |                             |
// |                 |  |       Message Bus           |
// +-----------------+  +-----------------------------+

/// Websocket server endpoint, e.g. ws://0.0.0.0:8336/subscribe
pub const WEBSOCKET_PATH: &str = "subscribe";

/// Establishes a websocket connection and sends new connected client event to message bus
/// to update connected websocket client mapping to broadcast events to connected clients.
async fn establish_connection(websocket: WebSocket, mb_tx: mpsc::Sender<MessageBus>) {
    // Map the websocket stream to the channel
    tracing::debug!(
        target: "org-node",
        "Received connection request from peer"
    );

    let (mut peer_ws_tx, mut peer_ws_rx) = websocket.split();

    // Use an unbounded channel to handle buffering and flushing of messages.
    let (peer_unbounded_tx, peer_unbounded_rx) = mpsc::unbounded_channel::<String>();
    let mut peer_unbounded_rx = UnboundedReceiverStream::new(peer_unbounded_rx);

    // Listen for messages coming from the websocket message bus.
    spawn(async move {
        while let Some(message) = peer_unbounded_rx.next().await {
            peer_ws_tx
                .send(Message::text(message.to_string()))
                .unwrap_or_else(|e| {
                    eprintln!("websocket send error: {}", e);
                })
                .await;
        }
    });

    // Inform the message bus to handle the incoming peer and map the unbounded channel.
    if let Err(e) = mb_tx
        .send(MessageBus::WebSocketConnection(peer_unbounded_tx))
        .await
    {
        tracing::error!(
            target: "org-node",
            "Failed to inform web socket of new connection: {}",
            e
        );
    }

    // Ignore incoming messages from the connected peer.
    #[allow(clippy::redundant_pattern_matching)]
    while let Some(_) = peer_ws_rx.next().await {
        // Do nothing.
    }
}

/// Serves a warp web server instance with a websocket endpoint for pub/sub events.
/// Requires a message bus transmitter sender to update connected websocket clients.
pub async fn serve(
    mb_tx: mpsc::Sender<MessageBus>,
    listen: std::net::SocketAddr,
) -> Result<(), Error> {
    let connected_peers_filter = warp::any().map(move || mb_tx.clone());

    let routes = warp::path(WEBSOCKET_PATH).and(warp::ws().and(connected_peers_filter).map(
        move |ws: Ws, mb_tx: mpsc::Sender<MessageBus>| {
            ws.on_upgrade(move |socket| establish_connection(socket, mb_tx))
        },
    ));

    tracing::info!(target: "org-node", "Web Server Listening on http://{}", listen);
    tracing::info!(target: "org-node", "Web Socket Available at ws://{}/{}", listen, WEBSOCKET_PATH);
    warp::serve(routes).run(listen).await;

    Ok(())
}
