mod crypto;
mod http;
mod protocol;
mod state;
mod udp;

use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crypto::{parse_key_hex, Cipher};
use state::Store;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let udp_addr = env_or("PB_UDP_ADDR", "0.0.0.0:9999");
    let http_addr = env_or("PB_HTTP_ADDR", "0.0.0.0:8080");
    let web_dir = env_or("PB_WEB_DIR", "web");
    let key_hex = env_or(
        "PB_KEY",
        // Development default. Overriding this is the first thing to do before
        // exposing the server to the internet.
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
    );

    let key = parse_key_hex(&key_hex)?;
    if key_hex.starts_with("000102030405") {
        tracing::warn!("PB_KEY is the development default -- set a real key before deploying");
    }

    let cipher = Arc::new(Cipher::new(&key));
    let store = Arc::new(Store::new());

    info!(
        udp_addr = %udp_addr,
        http_addr = %http_addr,
        web_dir = %web_dir,
        "server configuration loaded"
    );

    let socket = UdpSocket::bind(&udp_addr).await?;
    tokio::spawn(udp::run(socket, cipher, store.clone()));

    let app = http::router(store, web_dir);
    let listener = tokio::net::TcpListener::bind(&http_addr).await?;
    info!("http listening on http://{http_addr}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
