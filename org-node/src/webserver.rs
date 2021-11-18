use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt, TryFutureExt};
use librad::{git::Urn, PeerId};
use serde::Serialize;
use tokio::sync::RwLock;
use tokio::{
    sync::mpsc::{self},
    task::spawn,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use warp::{
    ws::{Message, WebSocket, Ws},
    Filter,
};

/// Connected websocket client sender handles mapped by SocketAddr.
type ConnectedWebSocketClients = Arc<RwLock<HashMap<SocketAddr, mpsc::UnboundedSender<WsEvent>>>>;

/// Message type for establishing websocket sender.
type EstablishWebSocketSender = (SocketAddr, mpsc::UnboundedSender<WsEvent>);

// +----------------+
// |                |    Receiver<WsEvent>
// |  Main Process  +---------------------------+
// |                |                           |
// +-------+--------+                           |
//         |                                    |
//         | Sender<WsEvent>                    |
//         |                                    |
// +-------v--------+     +---------------------+---------------+-----+
// |                |     |  Web Socket Process                 |     |
// |   Client       |     |  (Spawned)                          |     |
// |   Process      |     |                                     |     |
// |   (Spawned)    |     |  Sender<WsEvent>                    |     |
// |                |     |  Sender<EstablishWebSocketSender>   |     |
// +----------------+     |                                     |     |
//                        |  +----------------------------------v--+  |
//                        |  | Connected Clients (Spawned)         |  |
//                        |  |                                     |  |
//                        |  | HashMap<SocketAddr,Sender<WsEvent>> |  |
//                        |  | Receiver<WsEvent>                   |  |
//                        |  | Receiver<EstablishWebSocketSender>  |  |
//                        |  +-----^-+------------+-------------+--+  |
//                        |        | |            |             |     |
//                        +---+----+-+------+-----+---------+---+-----+
//                            |      |      |     |         |   |
//                        +---+------v+  +--+-----v---+  +--+---v-----+
//                        |           |  |            |  |            |
//                        | WS Client |  | WS Client  |  | WS Client  |
//                        +-----------+  +------------+  +------------+

/// Websocket server endpoint, e.g. ws://0.0.0.0:8336/subscribe
pub const WEBSOCKET_PATH: &str = "subscribe";

/// WebSocket event enum type to broadcast to connected websocket peers.
#[derive(Debug, Clone, Serialize)]
pub enum WsEvent {
    UpdatedRef {
        urn: Urn,
        #[serde(serialize_with = "git_oid")]
        oid: git2::Oid,
        peer: PeerId,
    },
}

/// helper method for serializing git2::Oid to a string
fn git_oid<S>(oid: &git2::Oid, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&oid.to_string())
}

/// Establishes a websocket connection and sends new connected client event to message bus
/// to update connected websocket client mapping to broadcast events to connected clients.
async fn establish_connection(
    websocket: WebSocket,
    remote: SocketAddr,
    conn_tx: mpsc::UnboundedSender<EstablishWebSocketSender>,
) {
    // Map the websocket stream to the channel
    tracing::debug!(
        target: "org-node",
        "Received connection request from client address: {}", remote
    );

    let (mut peer_ws_tx, mut peer_ws_rx) = websocket.split();

    // Use an unbounded channel to handle buffering and flushing of messages.
    let (peer_unbounded_tx, peer_unbounded_rx) = mpsc::unbounded_channel::<WsEvent>();
    let mut peer_unbounded_rx = UnboundedReceiverStream::new(peer_unbounded_rx);

    // Listen for internal events and send to connected client.
    spawn(async move {
        while let Some(message) = peer_unbounded_rx.next().await {
            if let Ok(msg) = serde_json::to_string(&message) {
                peer_ws_tx
                    .send(Message::text(msg))
                    .unwrap_or_else(|e| {
                        tracing::error!(target: "org-node", "websocket send error: {}", e);
                    })
                    .await;
            }
        }
    });

    // Send message to update connected web socket clients.
    if let Err(e) = conn_tx.send((remote, peer_unbounded_tx)) {
        tracing::error!(
            target: "org-node",
            "Failed to inform new websocket client connection: {}",
            e
        );
    }

    // Ignore incoming messages from the connected peer.
    while let Some(msg) = peer_ws_rx.next().await {
        // do nothing with the incoming message, subscription only.
        drop(msg);
    }
}

/// Serves a warp web server instance with a websocket endpoint for subscribing to events.
pub async fn serve(listen: std::net::SocketAddr, mut events: UnboundedReceiverStream<WsEvent>) {
    // Spawn connected clients receiver
    let (conn_ws_tx, conn_ws_rx) = mpsc::unbounded_channel::<EstablishWebSocketSender>();
    let mut conn_ws_rx = UnboundedReceiverStream::new(conn_ws_rx);

    // instantiate an empty peer socket map;
    let connected_ws_clients = ConnectedWebSocketClients::default();
    let cloned_ws_clients = Arc::clone(&connected_ws_clients);

    // listen for internal events and broadcast to connected peers.
    tokio::task::spawn(async move {
        while let Some(msg) = events.next().await {
            // send update to all connected web socket clients.
            for (addr, tx) in cloned_ws_clients.read().await.iter() {
                if let Err(e) = tx.send(msg.clone()) {
                    tracing::error!(target: "org-node", "Failed to send message to web socket client: {}", e);

                    // Remove disconnected client from connected clients map.
                    cloned_ws_clients.write().await.remove(addr);
                }
            }
        }
    });

    // handle adding new connected websocket clients.
    tokio::task::spawn(async move {
        while let Some((addr, tx)) = conn_ws_rx.next().await {
            connected_ws_clients.write().await.insert(addr, tx);
        }
    });

    let connected_peers_filter = warp::any().map(move || conn_ws_tx.clone());

    let routes = warp::path(WEBSOCKET_PATH)
        .and(warp::addr::remote())
        .and(warp::ws())
        .and(connected_peers_filter)
        .map(
            move |remote: Option<SocketAddr>,
                  ws: Ws,
                  tx: mpsc::UnboundedSender<EstablishWebSocketSender>| {
                ws.on_upgrade(move |socket| {
                    establish_connection(
                        socket,
                        remote.expect("web socket clients should always have a socket address."),
                        tx,
                    )
                })
            },
        );

    tracing::info!(target: "org-node", "Web Server Listening on http://{}", listen);
    tracing::info!(target: "org-node", "Web Socket Available at ws://{}/{}", listen, WEBSOCKET_PATH);
    warp::serve(routes).run(listen).await;
}
