#![allow(dead_code)]

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

/// Deterministic nonce: device_id || session_id || sequence, all little-endian.
/// See protocol/protocol.md section 2 for why a 12-byte IETF nonce is safe here.
pub fn nonce_for(device_id: u32, session_id: u32, sequence: u32) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0..4].copy_from_slice(&device_id.to_le_bytes());
    n[4..8].copy_from_slice(&session_id.to_le_bytes());
    n[8..12].copy_from_slice(&sequence.to_le_bytes());
    n
}

pub struct Cipher(ChaCha20Poly1305);

impl Cipher {
    pub fn new(key: &[u8; 32]) -> Self {
        Cipher(ChaCha20Poly1305::new(Key::from_slice(key)))
    }

    pub fn seal(&self, nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        self.0
            .encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad })
            .expect("chacha20poly1305 encryption cannot fail for in-memory buffers")
    }

    pub fn open(&self, nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> Option<Vec<u8>> {
        self.0
            .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad })
            .ok()
    }
}

pub fn parse_key_hex(s: &str) -> Result<[u8; 32], String> {
    let raw = hex::decode(s.trim()).map_err(|e| format!("key is not valid hex: {e}"))?;
    if raw.len() != 32 {
        return Err(format!("key must be 32 bytes (64 hex chars), got {}", raw.len()));
    }
    let mut k = [0u8; 32];
    k.copy_from_slice(&raw);
    Ok(k)
}
