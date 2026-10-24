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
