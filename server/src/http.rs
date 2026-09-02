use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::debug;

use crate::state::Store;

pub fn router(store: Arc<Store>, web_dir: String) -> Router {
    Router::new()
        .route("/api/devices", get(devices))
        .route("/api/device/:id", get(device))
        .route("/ws", get(ws_upgrade))
        .fallback_service(ServeDir::new(web_dir))
        .layer(CorsLayer::permissive())
        .with_state(store)
}

async fn devices(State(store): State<Arc<Store>>) -> impl IntoResponse {
    Json(store.snapshot_all())
}

async fn device(State(store): State<Arc<Store>>, Path(id): Path<u32>) -> impl IntoResponse {
    match store.snapshot_one(id) {
        Some(d) => Json(d).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"error": "unknown device"}))).into_response(),
    }
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(store): State<Arc<Store>>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session(socket, store))
}

/// Generic subscriber endpoint. Anything that wants live metrics -- the web
/// dashboard, a VRChat OSC bridge, Home Assistant -- speaks this and nothing
/// else. It is deliberately JSON so consumers stay trivial to write.
async fn ws_session(mut socket: WebSocket, store: Arc<Store>) {
    let mut rx = store.subscribe();

    // Send current state immediately so a fresh client is not blank until the
    // next heartbeat.
    let hello = json!({ "type": "snapshot", "devices": store.snapshot_all() });
    if socket.send(Message::Text(hello.to_string())).await.is_err() {
        return;
    }

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(ev) => {
                    let msg = json!({ "type": "metric", "event": ev });
                    if socket.send(Message::Text(msg.to_string())).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    debug!("ws subscriber lagged {n} events");
                }
                Err(_) => return,
            },
            // Presence decays with wall time, not with packets, so the client
            // needs a periodic snapshot to learn that a device went offline.
            _ = ticker.tick() => {
                let msg = json!({ "type": "snapshot", "devices": store.snapshot_all() });
                if socket.send(Message::Text(msg.to_string())).await.is_err() {
                    return;
                }
            }
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None => return,
                Some(Err(_)) => return,
                _ => {}
            }
        }
    }
}
