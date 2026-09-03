use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Json, Router};
use pulsebridge_api::ServerMessage;
use serde_json::json;
use std::path::PathBuf;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing::debug;

use crate::state::Store;

#[derive(Clone)]
struct AppState {
    store: Arc<Store>,
    web_dir: Arc<PathBuf>,
}

pub fn router(store: Arc<Store>, web_dir: String) -> Router {
    let state = AppState {
        store,
        web_dir: Arc::new(PathBuf::from(&web_dir)),
    };

    Router::new()
        .route("/api/devices", get(devices))
        .route("/api/device/:id", get(device))
        .route("/embed/:target/:layout", get(embed))
        .route("/ws", get(ws_upgrade))
        .fallback_service(ServeDir::new(web_dir))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn devices(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.store.snapshot_all())
}

async fn device(State(state): State<AppState>, Path(id): Path<u32>) -> impl IntoResponse {
    match state.store.snapshot_one(id) {
        Some(d) => Json(d).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "unknown device"})),
        )
            .into_response(),
    }
}

async fn embed(
    Path((target, layout)): Path<(String, String)>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let valid_target = target.parse::<u32>().is_ok();
    let valid_layout = matches!(layout.as_str(), "minimal" | "compact" | "card" | "live");
    if !valid_target || !valid_layout {
        return StatusCode::NOT_FOUND.into_response();
    }

    // URL parameters are never used as filesystem components. This keeps the
    // dynamic route from becoming a path traversal surface while honoring
    // PB_WEB_DIR for deployments with custom static assets.
    let path = state.web_dir.join("embed.html");
    match tokio::fs::read_to_string(path).await {
        Ok(html) => Html(html).into_response(),
        Err(error) => {
            tracing::error!(%error, "cannot read embed shell");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session(socket, state.store))
}

/// Generic subscriber endpoint. Anything that wants live metrics -- the web
/// dashboard, a VRChat OSC bridge, Home Assistant -- speaks this and nothing
/// else. It is deliberately JSON so consumers stay trivial to write.
async fn ws_session(mut socket: WebSocket, store: Arc<Store>) {
    let mut rx = store.subscribe();

    // Send current state immediately so a fresh client is not blank until the
    // next heartbeat.
    let hello = ServerMessage::Snapshot {
        devices: store.snapshot_all(),
    };
    if socket
        .send(Message::Text(serde_json::to_string(&hello).unwrap()))
        .await
        .is_err()
    {
        return;
    }

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Ok(ev) => {
                    let msg = ServerMessage::Metric { event: ev };
                    if socket
                        .send(Message::Text(serde_json::to_string(&msg).unwrap()))
                        .await
                        .is_err()
                    {
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
                let msg = ServerMessage::Snapshot { devices: store.snapshot_all() };
                if socket
                    .send(Message::Text(serde_json::to_string(&msg).unwrap()))
                    .await
                    .is_err()
                {
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
