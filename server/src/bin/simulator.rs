//! Sends synthetic heart-rate telemetry so the server, the API and the web page
//! can be developed and load-tested without a watch or a phone.
//!
//!   cargo run --bin simulator -- 127.0.0.1:9999

#[path = "../crypto.rs"]
mod crypto;
#[path = "../protocol.rs"]
mod protocol;

use std::net::UdpSocket;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crypto::{parse_key_hex, Cipher};
use protocol::{
    encode_packet, Header, Telemetry, FLAG_CONTACT_OK, FLAG_HR_VALID, FLAG_WATCH_CONNECTED,
    PACKET_TELEMETRY,
};

fn main() -> std::io::Result<()> {
    let target = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:9999".into());
    let device_id: u32 = std::env::var("PB_DEVICE_ID")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let key_hex = std::env::var("PB_KEY").unwrap_or_else(|_| {
        "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f".into()
    });

    let cipher = Cipher::new(&parse_key_hex(&key_hex).expect("bad PB_KEY"));
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let session_id: u32 = rand::random();
    let mut sequence: u32 = 0;

    println!("simulator -> {target}  device_id={device_id} session_id={session_id:#010x}");

    // Walk a plausible heart rate rather than emitting a sawtooth, so the web
    // page is exercised with realistic value changes and hold periods.
    let mut hr: f32 = 68.0;
    let mut target_hr: f32 = 68.0;
    let mut hold = 0;

    loop {
        if hold == 0 {
            target_hr = 55.0 + rand::random::<f32>() * 120.0;
            hold = 5 + (rand::random::<u32>() % 25) as i32;
        }
        hold -= 1;
        hr += (target_hr - hr) * 0.12;

        sequence = sequence.wrapping_add(1);
        let header = Header {
            packet_type: PACKET_TELEMETRY,
            device_id,
            session_id,
            sequence,
            timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };
        let telemetry = Telemetry {
            flags: FLAG_HR_VALID | FLAG_CONTACT_OK | FLAG_WATCH_CONNECTED,
            heart_rate: hr.round().clamp(30.0, 220.0) as u8,
            battery_pct: 77,
        };
        let packet = encode_packet(&cipher, &header, &telemetry);
        socket.send_to(&packet, &target)?;
        println!("seq {sequence:>5}  hr {}", telemetry.heart_rate);
        std::thread::sleep(Duration::from_secs(1));
    }
}
