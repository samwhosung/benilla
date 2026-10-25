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

/// The seconds of one TOTP step (RFC 6238; vmangos `AuthSocket::GenerateTotpPin`,
/// `AuthSocket.cpp:1274`).
pub const TOTP_STEP_SECS: u64 = 30;

/// The steps around the current one that vmangos accepts for a TOTP account: -2 to 1
/// (`AuthSocket.cpp:720`, `for (i = -2; i != 2; ++i)`).
const TOTP_OFFSETS: [i64; 4] = [0, -1, 1, -2];

/// A base32 secret (RFC 4648: `A-Z`, `2-7`, case-insensitive) as bytes. Spaces and dashes, how
/// authenticator apps group a secret, and trailing `=` padding are skipped; anything else, or an
/// empty secret, is `None`.
pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut bits, mut have) = (0u32, 0u32);
    let mut padded = false;
    for c in text.chars() {
        let value = match c {
            ' ' | '-' => continue,
            '=' => {
                padded = true;
                continue;
            }
            'A'..='Z' => c as u32 - 'A' as u32,
            'a'..='z' => c as u32 - 'a' as u32,
            '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };
        if padded {
            return None;
        }
        bits = bits << 5 | value;
        have += 5;
        if have >= 8 {
            have -= 8;
            out.push((bits >> have) as u8);
            bits &= (1 << have) - 1;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// HMAC-SHA1 (RFC 2104) over the `sha1` the SRP code already uses.
fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.map(|b| b ^ byte);
    let inner = Sha1::new()
        .chain_update(pad(0x36))
        .chain_update(message)
        .finalize();
    Sha1::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

/// The six-digit RFC 6238 code for the HOTP counter `step` (`GenerateTotpPin`: the big-endian
/// counter, HMAC-SHA1, the RFC 4226 dynamic truncation, modulo 10^6).
pub fn totp_code(key: &[u8], step: u64) -> u32 {
    let mac = hmac_sha1(key, &step.to_be_bytes());
    let offset = usize::from(mac[19] & 0xf);
    let word = u32::from_be_bytes([
        mac[offset],
        mac[offset + 1],
        mac[offset + 2],
        mac[offset + 3],
    ]);
    (word & 0x7fff_ffff) % 1_000_000
}

/// The decimal digits of a code as vmangos reads it (`VerifyPinData`, `AuthSocket.cpp:1235`): the
/// number's own digits, so a code with leading zeros loses them (`081804` is the five keys
/// `81804`) and one below 1000 has fewer than the 4 keys a PIN needs, which no proof can pass.
pub fn code_digits(code: u32) -> Vec<u8> {
    let mut digits = Vec::new();
    let mut rest = code;
    while rest != 0 {
        digits.push((rest % 10) as u8);
        rest /= 10;
    }
    digits.reverse();
    digits
}

/// The PIN digits for a TOTP `key` at `unix_secs`: the current step's code, or when that one has
/// fewer than 4 digits (about 1 in 1000, which vmangos rejects) the nearest other step it accepts
/// (`TOTP_OFFSETS`). `None` only if every step is too short.
pub fn totp_digits(key: &[u8], unix_secs: u64) -> Option<Vec<u8>> {
    let now = unix_secs / TOTP_STEP_SECS;
    TOTP_OFFSETS
        .iter()
        .filter_map(|&o| now.checked_add_signed(o))
        .map(|step| code_digits(totp_code(key, step)))
        .find(|digits| digits.len() >= 4)
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

    /// RFC 6238 appendix B's SHA-1 vectors (secret `12345678901234567890`), read at six digits:
    /// the 8-digit values 94287082, 07081804, 14050471, 89005924 and 69279037.
    #[test]
    fn totp_codes_match_rfc_6238() {
        let key = b"12345678901234567890";
        for (time, code) in [
            (59, 287_082),
            (1_111_111_109, 81_804),
            (1_111_111_111, 50_471),
            (1_234_567_890, 5_924),
            (2_000_000_000, 279_037),
        ] {
            assert_eq!(totp_code(key, time / TOTP_STEP_SECS), code, "t={time}");
        }
    }

    #[test]
    fn base32_reads_rfc_4648_and_authenticator_spelling() {
        let key = b"12345678901234567890".to_vec();
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(base32_decode(secret), Some(key.clone()));
        assert_eq!(base32_decode(&secret.to_lowercase()), Some(key.clone()));
        assert_eq!(
            base32_decode("GEZD GNBV-GY3T QOJQ GEZD GNBV GY3T QOJQ"),
            Some(key)
        );
        // RFC 4648 section 10: padding after a partial group.
        assert_eq!(base32_decode("MZXW6YQ="), Some(b"foob".to_vec()));
        assert_eq!(base32_decode("MZXQ===="), Some(b"fo".to_vec()));
        assert_eq!(base32_decode("MZXW6YQ=A"), None, "data after padding");
        assert_eq!(base32_decode("GEZ1"), None, "1 is no base32 digit");
        assert_eq!(base32_decode(""), None);
    }

    /// vmangos reads the code as a number (`VerifyPinData`), so `081804` is `81804`.
    #[test]
    fn a_code_with_leading_zeros_is_sent_without_them() {
        assert_eq!(code_digits(81_804), vec![8, 1, 8, 0, 4]);
        assert_eq!(code_digits(5_924), vec![5, 9, 2, 4]);
        assert_eq!(code_digits(287_082), vec![2, 8, 7, 0, 8, 2]);
        assert_eq!(code_digits(0), Vec::<u8>::new());
        let key = b"12345678901234567890";
        assert_eq!(totp_digits(key, 1_111_111_109), Some(vec![8, 1, 8, 0, 4]));
    }

    /// A step's code below 1000 has under 4 digits and fails vmangos' length check, so the
    /// nearest accepted step answers instead.
    #[test]
    fn a_too_short_code_falls_back_to_another_accepted_step() {
        let key = b"12345678901234567890";
        // Find a step whose code is under 1000 with a search over the key's own steps.
        let step = (1..200_000u64)
            .find(|&s| totp_code(key, s) < 1000)
            .expect("one in a thousand steps");
        let digits = totp_digits(key, step * TOTP_STEP_SECS).unwrap();
        assert!(digits.len() >= 4);
        assert_eq!(digits, code_digits(totp_code(key, step - 1)));
    }
}
