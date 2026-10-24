//! The logon's PIN (benilla-re `net/logonlink#pin-challenge`, `#pin-proof`): the keypad grid a
//! challenge's seed builds, and the proof of the keys pressed on it. The keypad shows the grid's
//! digits on its ten keys and the script hands back the keys' positions, never the digits, so the
//! proof is of the positions and only a server that knows the seed can read the PIN out of it.

use sha1::{Digest, Sha1};

use super::{PinChallenge, PinProof};

/// `0x5b1e70`: the grid of a seed, a permutation of the digits 0 to 9. Entry `i` is the digit left
/// at `seed mod (10 - i)` (`0x5b1e97`), which then closes its gap (`0x5b1eb1`), and the seed is
/// divided by `10 - i`.
pub fn grid(mut seed: u32) -> [u8; 10] {
    let mut left: Vec<u8> = (0..10).collect();
    let mut out = [0u8; 10];
    for (i, slot) in out.iter_mut().enumerate() {
        let n = 10 - i as u32;
        *slot = left.remove((seed % n) as usize);
        seed /= n;
    }
    out
}

/// `0x5b3120`'s hash (`0x5b31e0`): `SHA-1(client salt ‖ SHA-1(server salt ‖ text))`, the text
/// each position plus `0x30` (`0x5b3147`) hashed as a C string (`0x5d1b30`), so a position of
/// `0xd0`, a NUL digit, ends it.
pub fn hash(server_salt: &[u8; 16], positions: &[u8], client_salt: &[u8; 16]) -> [u8; 20] {
    let text: Vec<u8> = positions
        .iter()
        .map(|p| p.wrapping_add(0x30))
        .take_while(|&d| d != 0)
        .collect();
    let inner: [u8; 20] = Sha1::new()
        .chain_update(server_salt)
        .chain_update(&text)
        .finalize()
        .into();
    Sha1::new()
        .chain_update(client_salt)
        .chain_update(inner)
        .finalize()
        .into()
}

impl PinProof {
    /// `0x5b3120` for the positions pressed: a fresh 16-byte client salt (`0x5b3177`, drawn there
    /// from a pool the clock seeds) and the hash over it, sent under the challenge's own security
    /// byte (`0x5b1f10`).
    pub fn new(challenge: &PinChallenge, positions: &[u8]) -> PinProof {
        let salt: [u8; 16] = rand::random();
        PinProof {
            flags: challenge.flags,
            salt,
            hash: hash(&challenge.salt, positions, &salt),
        }
    }
}

/// The digits `grid` shows at `positions`: the PIN itself. `None` for a position off the keypad.
pub fn digits(grid: &[u8; 10], positions: &[u8]) -> Option<Vec<u8>> {
    positions
        .iter()
        .map(|&p| grid.get(usize::from(p)).copied())
        .collect()
}

/// The positions `digits` sit at on `grid`, the keys a player presses for them. `None` for a
/// digit the grid lacks.
pub fn positions(grid: &[u8; 10], digits: &[u8]) -> Option<Vec<u8>> {
    digits
        .iter()
        .map(|d| grid.iter().position(|g| g == d).map(|p| p as u8))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// `0x5b1e70` over five seeds; the values are its byte-verified transcription's (benilla-re
    /// `net::pin_grid_shuffle`, checked against the bytes by `net::pin_shuffle`).
    #[test]
    fn the_grid_is_the_seed_read_as_a_permutation() {
        assert_eq!(grid(0), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(grid(1), [1, 0, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(grid(0x1234_5678), [6, 3, 2, 1, 9, 8, 7, 0, 4, 5]);
        assert_eq!(grid(u32::MAX), [5, 8, 2, 0, 4, 9, 3, 1, 7, 6]);
        // 10! - 1, the last permutation.
        assert_eq!(grid(3_628_799), [9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
    }

    fn server_salt() -> [u8; 16] {
        std::array::from_fn(|i| (i as u8).wrapping_mul(7).wrapping_add(3))
    }

    fn client_salt() -> [u8; 16] {
        std::array::from_fn(|i| (i as u8).wrapping_mul(11).wrapping_add(5))
    }

    /// `SHA-1(client salt ‖ SHA-1(server salt ‖ positions as ASCII digits))` (`0x5b3147`,
    /// `0x5b31e0`); the digest computed outside this crate (Python `hashlib`).
    #[test]
    fn the_hash_is_over_the_positions_as_digits() {
        assert_eq!(
            hex(&hash(&server_salt(), &[3, 1, 4, 1, 5], &client_salt())),
            "2adb828bd7fb392698e25a06932267ad5258fa90"
        );
    }

    /// A position of `0xd0` makes a NUL digit, and the inner hash's `strlen` (`0x5d1b30`) stops
    /// there.
    #[test]
    fn a_nul_digit_ends_the_text() {
        let golden = "bbebe757a59c66a25a6552a2bae94590085d0848";
        assert_eq!(
            hex(&hash(&server_salt(), &[1, 0xd0, 2, 3], &client_salt())),
            golden
        );
        assert_eq!(hex(&hash(&server_salt(), &[1], &client_salt())), golden);
    }

    /// The proof carries the challenge's own security byte, a fresh client salt and the hash over
    /// it (`0x5b3177`, `0x5b318f`, `0x5b1f10`).
    #[test]
    fn a_proof_hashes_its_own_salt() {
        let challenge = PinChallenge {
            flags: 1,
            grid_seed: 0x1234_5678,
            salt: server_salt(),
        };
        let a = PinProof::new(&challenge, &[2, 9, 4, 7]);
        let b = PinProof::new(&challenge, &[2, 9, 4, 7]);
        assert_eq!(a.flags, 1);
        assert_ne!(a.salt, b.salt, "each proof draws its own salt");
        assert_eq!(a.hash, hash(&server_salt(), &[2, 9, 4, 7], &a.salt));
    }

    /// The digits a grid shows at the pressed positions, and back on another grid.
    #[test]
    fn digits_and_positions_invert_each_other() {
        let shown = grid(0x1234_5678);
        assert_eq!(digits(&shown, &[2, 9, 4, 7]), Some(vec![2, 5, 9, 0]));
        assert_eq!(digits(&shown, &[2, 10]), None, "no key 10");
        let next = grid(u32::MAX);
        assert_eq!(positions(&next, &[2, 5, 9, 0]), Some(vec![2, 0, 5, 3]));
        assert_eq!(positions(&next, &[2, 11]), None, "no digit 11");
    }
}
