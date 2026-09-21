//! Bounded WebSocket delivery.

use axum::extract::ws::{Message, WebSocket};
use std::time::Duration;

const SEND_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) async fn send_text(socket: &mut WebSocket, text: String) -> bool {
    tokio::time::timeout(SEND_TIMEOUT, socket.send(Message::Text(text.into())))
        .await
        .is_ok_and(|result| result.is_ok())
}
