// Shared by both binaries; each uses a different subset of the codec.
#![allow(dead_code)]

use crate::crypto::Cipher;

pub const MAGIC: u16 = 0x5042; // "PB"
pub const VERSION: u8 = 1;
pub const PACKET_TELEMETRY: u8 = 1;
pub const HEADER_LEN: usize = 24;
pub const PAYLOAD_LEN: usize = 4;
pub const TAG_LEN: usize = 16;
pub const PACKET_LEN: usize = HEADER_LEN + PAYLOAD_LEN + TAG_LEN; // 44

pub const FLAG_HR_VALID: u8 = 1 << 0;
pub const FLAG_CONTACT_OK: u8 = 1 << 1;
pub const FLAG_WATCH_CONNECTED: u8 = 1 << 2;
pub const FLAG_HEARTBEAT: u8 = 1 << 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub packet_type: u8,
    pub device_id: u32,
    pub session_id: u32,
    pub sequence: u32,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Telemetry {
    pub flags: u8,
    pub heart_rate: u8,
    pub battery_pct: u8,
}

impl Telemetry {
    pub fn hr_valid(&self) -> bool {
        self.flags & FLAG_HR_VALID != 0
    }
    pub fn contact_ok(&self) -> bool {
        self.flags & FLAG_CONTACT_OK != 0
    }
    pub fn watch_connected(&self) -> bool {
        self.flags & FLAG_WATCH_CONNECTED != 0
    }
}

#[derive(Debug)]
pub enum DecodeError {
    TooShort,
    BadMagic,
    BadVersion,
    UnknownType(u8),
    AuthFailed,
}

pub fn encode_header(h: &Header) -> [u8; HEADER_LEN] {
    let mut b = [0u8; HEADER_LEN];
    b[0..2].copy_from_slice(&MAGIC.to_be_bytes());
    b[2] = VERSION;
    b[3] = h.packet_type;
    b[4..8].copy_from_slice(&h.device_id.to_le_bytes());
    b[8..12].copy_from_slice(&h.session_id.to_le_bytes());
    b[12..16].copy_from_slice(&h.sequence.to_le_bytes());
    b[16..24].copy_from_slice(&h.timestamp_ms.to_le_bytes());
    b
}

pub fn parse_header(buf: &[u8]) -> Result<Header, DecodeError> {
    if buf.len() < HEADER_LEN {
        return Err(DecodeError::TooShort);
    }
    if u16::from_be_bytes([buf[0], buf[1]]) != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    if buf[2] != VERSION {
        return Err(DecodeError::BadVersion);
    }
    Ok(Header {
        packet_type: buf[3],
        device_id: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        session_id: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
        sequence: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        timestamp_ms: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
    })
}

/// Build a complete wire packet. Used by the simulator and by the protocol tests.
pub fn encode_packet(cipher: &Cipher, h: &Header, t: &Telemetry) -> Vec<u8> {
    let header = encode_header(h);
    let payload = [t.flags, t.heart_rate, t.battery_pct, 0u8];
    let nonce = crate::crypto::nonce_for(h.device_id, h.session_id, h.sequence);
    let sealed = cipher.seal(&nonce, &header, &payload);
    let mut out = Vec::with_capacity(HEADER_LEN + sealed.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(&sealed);
    out
}

pub fn decode_packet(cipher: &Cipher, buf: &[u8]) -> Result<(Header, Telemetry), DecodeError> {
    let h = parse_header(buf)?;
    if h.packet_type != PACKET_TELEMETRY {
        return Err(DecodeError::UnknownType(h.packet_type));
    }
    if buf.len() != PACKET_LEN {
        return Err(DecodeError::TooShort);
    }
    let nonce = crate::crypto::nonce_for(h.device_id, h.session_id, h.sequence);
    let plain = cipher
        .open(&nonce, &buf[..HEADER_LEN], &buf[HEADER_LEN..])
        .ok_or(DecodeError::AuthFailed)?;
    Ok((
        h,
        Telemetry {
            flags: plain[0],
            heart_rate: plain[1],
            battery_pct: plain[2],
        },
    ))
}

/// 64-entry sliding replay window. UDP reorders, so a monotonic check is wrong.
#[derive(Debug, Default)]
pub struct ReplayWindow {
    highest: u32,
    bitmap: u64,
    started: bool,
}

impl ReplayWindow {
    /// Returns true if the sequence is fresh and should be accepted.
    pub fn accept(&mut self, seq: u32) -> bool {
        if !self.started {
            self.started = true;
            self.highest = seq;
            self.bitmap = 1;
            return true;
        }
        if seq > self.highest {
            let shift = seq - self.highest;
            self.bitmap = if shift >= 64 { 0 } else { self.bitmap << shift };
            self.bitmap |= 1;
            self.highest = seq;
            true
        } else {
            let back = self.highest - seq;
            if back >= 64 {
                return false; // too old
            }
            let mask = 1u64 << back;
            if self.bitmap & mask != 0 {
                false // already seen
            } else {
                self.bitmap |= mask;
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::parse_key_hex;

    fn test_cipher() -> Cipher {
        Cipher::new(
            &parse_key_hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
                .unwrap(),
        )
    }

    /// Matches the test vector in protocol/protocol.md section 7.
    #[test]
    fn roundtrip_matches_spec_vector() {
        let h = Header {
            packet_type: PACKET_TELEMETRY,
            device_id: 1,
            session_id: 0x1122_3344,
            sequence: 1,
            timestamp_ms: 1_700_000_000_000,
        };
        let t = Telemetry { flags: 0x07, heart_rate: 72, battery_pct: 85 };

        let pkt = encode_packet(&test_cipher(), &h, &t);
        assert_eq!(pkt.len(), PACKET_LEN);
        assert_eq!(&pkt[0..2], &[0x50, 0x42]);
        assert_eq!(pkt[2], VERSION);

        let (h2, t2) = decode_packet(&test_cipher(), &pkt).unwrap();
        assert_eq!(h, h2);
        assert_eq!(t, t2);
    }

    #[test]
    fn tampered_header_fails_auth() {
        let h = Header {
            packet_type: PACKET_TELEMETRY,
            device_id: 1,
            session_id: 7,
            sequence: 3,
            timestamp_ms: 1_700_000_000_000,
        };
        let t = Telemetry { flags: 0x07, heart_rate: 72, battery_pct: 85 };
        let mut pkt = encode_packet(&test_cipher(), &h, &t);
        pkt[16] ^= 0xff; // flip a timestamp byte, which is AAD
        assert!(matches!(
            decode_packet(&test_cipher(), &pkt),
            Err(DecodeError::AuthFailed)
        ));
    }

    #[test]
    fn wrong_key_fails_auth() {
        let h = Header {
            packet_type: PACKET_TELEMETRY,
            device_id: 1,
            session_id: 7,
            sequence: 3,
            timestamp_ms: 1_700_000_000_000,
        };
        let t = Telemetry { flags: 1, heart_rate: 100, battery_pct: 50 };
        let pkt = encode_packet(&test_cipher(), &h, &t);
        let other = Cipher::new(&[0xaa; 32]);
        assert!(matches!(
            decode_packet(&other, &pkt),
            Err(DecodeError::AuthFailed)
        ));
    }

    #[test]
    fn replay_window_accepts_reorder_rejects_duplicates() {
        let mut w = ReplayWindow::default();
        assert!(w.accept(10));
        assert!(w.accept(12));
        assert!(w.accept(11)); // reordered but inside the window
        assert!(!w.accept(11)); // duplicate
        assert!(!w.accept(10)); // duplicate
        assert!(w.accept(200)); // big jump resets the bitmap
        assert!(!w.accept(100)); // now too old
        assert!(w.accept(199));
    }
}
