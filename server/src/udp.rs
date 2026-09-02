use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use crate::crypto::Cipher;
use crate::protocol::{decode_packet, DecodeError, PACKET_LEN};
use crate::state::{Accepted, Store};

pub async fn run(socket: UdpSocket, cipher: Arc<Cipher>, store: Arc<Store>) {
    let mut buf = [0u8; 512];
    info!("udp listener ready on {}", socket.local_addr().unwrap());

    loop {
        let (len, addr) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                // On Windows an ICMP port-unreachable for a previous send can
                // surface here; it is not fatal for a listening socket.
                warn!("recv_from failed: {e}");
                continue;
            }
        };
        if len != PACKET_LEN {
            debug!("drop {len} bytes from {addr}: wrong length");
            continue;
        }

        match decode_packet(&cipher, &buf[..len]) {
            Ok((h, t)) => match store.ingest(&h, &t, addr) {
                Accepted::Ok(Some(ev)) => debug!("device {} hr event {:?}", h.device_id, ev.metric),
                Accepted::Ok(None) => {}
                Accepted::Replay => debug!("device {} replay seq {}", h.device_id, h.sequence),
                Accepted::ClockSkew => {
                    warn!("device {} rejected: clock skew > 120s", h.device_id)
                }
            },
            Err(DecodeError::AuthFailed) => {
                warn!("auth failure from {addr} (wrong key or tampered packet)")
            }
            Err(e) => debug!("drop packet from {addr}: {e:?}"),
        }
    }
}
