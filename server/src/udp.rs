use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

use crate::crypto::Cipher;
use crate::protocol::{decode_packet, DecodeError, PACKET_LEN};
use crate::state::{now_ms, Accepted, Store};

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
            warn!(source = %addr, length = len, expected = PACKET_LEN, "drop packet: wrong length");
            continue;
        }

        match decode_packet(&cipher, &buf[..len]) {
            Ok((h, t)) => match store.ingest(&h, &t, addr) {
                Accepted::Ok(report) => {
                    if report.new_device {
                        info!(
                            device_id = h.device_id,
                            session_id = h.session_id,
                            source = %addr,
                            "new device accepted"
                        );
                    } else if report.new_session {
                        info!(
                            device_id = h.device_id,
                            session_id = h.session_id,
                            source = %addr,
                            "new session accepted"
                        );
                    }
                    if report.gap_count > 0 {
                        warn!(
                            device_id = h.device_id,
                            session_id = h.session_id,
                            sequence = h.sequence,
                            gap_count = report.gap_count,
                            "telemetry sequence gap"
                        );
                    }
                    debug!(
                        device_id = h.device_id,
                        session_id = h.session_id,
                        sequence = h.sequence,
                        source = %addr,
                        timestamp_ms = h.timestamp_ms,
                        received_at_ms = report.received_at_ms,
                        ingest_lag_ms = report.ingest_lag_ms,
                        flags = t.flags,
                        heartbeat = t.flags & crate::protocol::FLAG_HEARTBEAT != 0,
                        hr_valid = t.hr_valid(),
                        contact_ok = t.contact_ok(),
                        watch_connected = t.watch_connected(),
                        "telemetry accepted"
                    );
                    if let Some(ev) = report.event {
                        debug!(device_id = h.device_id, metric = ?ev.metric, "metric changed");
                    }
                }
                Accepted::Replay => warn!(
                    device_id = h.device_id,
                    session_id = h.session_id,
                    sequence = h.sequence,
                    source = %addr,
                    "replay packet rejected"
                ),
                Accepted::ClockSkew => {
                    let received_at_ms = now_ms();
                    let clock_skew_ms = if received_at_ms >= h.timestamp_ms {
                        (received_at_ms - h.timestamp_ms) as i64
                    } else {
                        -((h.timestamp_ms - received_at_ms) as i64)
                    };
                    warn!(
                        device_id = h.device_id,
                        session_id = h.session_id,
                        sequence = h.sequence,
                        source = %addr,
                        timestamp_ms = h.timestamp_ms,
                        received_at_ms,
                        clock_skew_ms,
                        "packet rejected: clock skew > 120s"
                    )
                }
            },
            Err(DecodeError::AuthFailed) => warn!(source = %addr, "packet rejected: auth failure"),
            Err(e) => debug!(source = %addr, error = ?e, "drop packet: decode failure"),
        }
    }
}
