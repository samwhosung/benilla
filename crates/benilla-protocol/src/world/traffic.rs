//! What `GetNetStats` (`0x48b8b0`) reads of a world link: the bytes it has carried each way since
//! it connected, and the rates they make (`0x537f20`).

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

/// One link's byte counts and connect time, the reference's `NetClient` `+0x1ab8` (received),
/// `+0x1ab4` (sent) and `+0x1abc`, which its connect hook `0x537b90` zeroes and stamps. A received
/// packet counts its opcode and body (`0x537b34`), a sent one its four-byte opcode and body
/// (`0x5379c6`), neither its size header; both counts are dwords that wrap.
#[derive(Debug)]
pub struct LinkTraffic {
    connected: Instant,
    received: AtomicU32,
    sent: AtomicU32,
}

impl LinkTraffic {
    /// A link that has just connected: no bytes yet, its clock starting now.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            connected: Instant::now(),
            received: AtomicU32::new(0),
            sent: AtomicU32::new(0),
        }
    }

    /// Count one received packet of `bytes` opcode and body bytes.
    pub(super) fn add_received(&self, bytes: usize) {
        self.received.fetch_add(bytes as u32, Ordering::Relaxed);
    }

    /// Count one sent packet of `bytes` opcode and body bytes.
    pub(super) fn add_sent(&self, bytes: usize) {
        self.sent.fetch_add(bytes as u32, Ordering::Relaxed);
    }

    /// The bytes received since the connect.
    pub fn received(&self) -> u32 {
        self.received.load(Ordering::Relaxed)
    }

    /// The bytes sent since the connect.
    pub fn sent(&self) -> u32 {
        self.sent.load(Ordering::Relaxed)
    }

    /// `(received, sent)` in KB/s since the connect, as `GetNetStats` returns its first two values.
    pub fn rates_kbps(&self) -> (f32, f32) {
        // The tick difference, a dword that wraps (`0x537f3d`).
        let ms = self.connected.elapsed().as_millis() as u32;
        (kbps(self.received(), ms), kbps(self.sent(), ms))
    }
}

/// One rate of `0x537f20`: `bytes · (1/1024) / (ms · 0.001)`, the factors the `f32`s `0x8090d8`
/// and `0x801360` (`0x537f63`-`0x537f94`), at the client's 53-bit x87 precision, stored as an `f32`
/// (`0x537f80`). With no time elapsed it is `+∞`, or NaN with no bytes.
fn kbps(bytes: u32, ms: u32) -> f32 {
    let seconds = f64::from(ms) * f64::from(0.001_f32);
    (f64::from(bytes) * f64::from(1.0_f32 / 1024.0) / seconds) as f32
}

#[cfg(test)]
mod tests {
    use super::kbps;

    /// `bytes · (1/1024) / (ms · 0.001)` with the client's two `f32` constants (`0x8090d8`,
    /// `0x801360`), at 53 bits, stored as an `f32` (`0x537f80`).
    #[test]
    fn a_rate_is_kilobytes_over_seconds_since_the_connect() {
        assert_eq!(kbps(0, 5000), 0.0);
        assert_eq!(kbps(1024, 4096), 0.244_140_61);
        // `0.001f` is a hair over 0.001, so 2 KB in 2 s reads a hair under 1.
        assert_eq!(kbps(2048, 2000), 0.999_999_94);
        assert_eq!(
            kbps(1024, 3000),
            (1.0 / (3000.0 * f64::from(0.001_f32))) as f32
        );
        assert_ne!(kbps(1024, 3000), (1.0_f64 / 3.0) as f32);
    }

    /// With no time elapsed the quotient follows the x87 rules: `+∞`, or NaN with no bytes.
    #[test]
    fn no_time_since_the_connect_divides_by_zero() {
        assert_eq!(kbps(1, 0), f32::INFINITY);
        assert!(kbps(0, 0).is_nan());
    }
}
