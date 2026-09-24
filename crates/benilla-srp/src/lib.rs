//! SRP6 client and world-header crypto for 1.12.1 (build 5875): the client side of the realmd
//! handshake and the world server's header obfuscation. Every byte array is little endian, as on
//! the wire.
//!
//! # Encoding-unambiguous handshakes
//!
//! The 1.12.1 client hashes `A`, `B`, `K`, the salt and `M1` at their declared widths, zero-padded
//! (`0x5d3650`); vmangos (`SHA1::Generator::UpdateData(BigNumber const&)`) and cmangos
//! (`Sha1Hash::UpdateBigNumbers`) drop high-order zero bytes, so a value with one hashes
//! differently and realmd answers `WOW_FAIL_UNKNOWN_ACCOUNT` (0x04) to a correct password. The low
//! end of `S` has the same split ([`calculate_interleaved`]).
//!
//! Deviation: [`SrpClientChallenge::new`] redraws `a` until every value it sends serializes the
//! same both ways ([`is_width_stable`]), and `benilla-protocol`'s `logon` redials when the server's
//! `B` lands ambiguous, because a client that picks either serialization fails about 1 handshake
//! in 45 against vmangos. The arithmetic stays the client's; only an ambiguous draw is declined.

use num_bigint::BigInt;
use rand::{thread_rng, RngCore};
use sha1::{Digest, Sha1};

pub mod vanilla_header;

pub use vanilla_header::{DecrypterHalf, EncrypterHalf, HeaderCrypto, ProofSeed};

/// Session-key length in bytes: two interleaved SHA-1 digests.
pub const SESSION_KEY_LENGTH: usize = 40;
/// Proof (`M1`, `M2`) length in bytes: one SHA-1 digest.
pub const PROOF_LENGTH: usize = 20;
/// Public-key (`A`/`B`) length in bytes.
pub const PUBLIC_KEY_LENGTH: usize = 32;
pub const SALT_LENGTH: usize = 32;
/// The generator `g`.
pub const GENERATOR: u8 = 7;
/// The safe prime `N`, little endian, as `CMD_AUTH_LOGON_CHALLENGE_Server` sends it.
pub const LARGE_SAFE_PRIME_LITTLE_ENDIAN: [u8; 32] = [
    0xb7, 0x9b, 0x3e, 0x2a, 0x87, 0x82, 0x3c, 0xab, 0x8f, 0x5e, 0xbf, 0xbf, 0x8e, 0xb1, 0x1, 0x8,
    0x53, 0x50, 0x6, 0x29, 0x8b, 0x5b, 0xad, 0xbd, 0x5b, 0x53, 0xe1, 0x89, 0x5e, 0x64, 0x4b, 0x89,
];
/// The SRP multiplier `k`, fixed at 3 (SRP6, not 6a).
const K_VALUE: u8 = 3;

// --- bigint helpers (little-endian, unsigned magnitude) ------------------------------------------

fn from_le(bytes: &[u8]) -> BigInt {
    BigInt::from_bytes_le(num_bigint::Sign::Plus, bytes)
}

/// Magnitude of `v` as a zero-padded 32-byte little-endian array (`v` is always `< N < 2^256`).
fn to_padded_32_le(v: &BigInt) -> [u8; 32] {
    let (_, bytes) = v.to_bytes_le();
    let mut out = [0u8; 32];
    out[..bytes.len()].copy_from_slice(&bytes);
    out
}

fn sha1(parts: &[&[u8]]) -> [u8; 20] {
    let mut h = Sha1::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// Whether a little-endian value hashes the same at its declared width and at minimal length:
/// exactly when its high-order byte is non-zero.
fn is_width_stable(little_endian: &[u8]) -> bool {
    matches!(little_endian.last(), Some(&b) if b != 0)
}

// --- normalized string ---------------------------------------------------------------------------

/// A username or password as the 1.12 client normalises it: ASCII without control characters,
/// uppercased, 1..=16 bytes. SRP6 hashes this form, and the account name goes on the wire in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedString {
    s: String,
}

/// Error from [`NormalizedString::new`].
#[derive(Debug)]
pub enum NormalizedStringError {
    /// Empty or longer than 16 bytes.
    InvalidLength,
    /// Contained a non-ASCII or ASCII-control character.
    CharacterNotAllowed(char),
}

impl std::fmt::Display for NormalizedStringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLength => write!(f, "string must be 1..=16 bytes"),
            Self::CharacterNotAllowed(c) => write!(f, "character not allowed: {c:?}"),
        }
    }
}
impl std::error::Error for NormalizedStringError {}

impl NormalizedString {
    pub fn new(s: impl AsRef<str>) -> Result<Self, NormalizedStringError> {
        let s = s.as_ref();
        if s.is_empty() || s.len() > 16 {
            return Err(NormalizedStringError::InvalidLength);
        }
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if !c.is_ascii() || c.is_ascii_control() {
                return Err(NormalizedStringError::CharacterNotAllowed(c));
            }
            out.push(c.to_ascii_uppercase());
        }
        Ok(Self { s: out })
    }
}

impl AsRef<str> for NormalizedString {
    fn as_ref(&self) -> &str {
        &self.s
    }
}

// --- public key ----------------------------------------------------------------------------------

/// A validated SRP public key (`A` or `B`), stored little endian. Rejected if it is exactly zero or
/// exactly the safe prime `N` (the only 32-byte values that are `0 mod N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKey {
    key: [u8; 32],
}

/// Error from [`PublicKey::from_le_bytes`].
#[derive(Debug)]
pub enum InvalidPublicKeyError {
    IsZero,
    /// The key is `0 mod N` (equal to the safe prime).
    ModLargeSafePrimeIsZero,
}

impl std::fmt::Display for InvalidPublicKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IsZero => write!(f, "public key is zero"),
            Self::ModLargeSafePrimeIsZero => write!(f, "public key is 0 mod N"),
        }
    }
}
impl std::error::Error for InvalidPublicKeyError {}

impl PublicKey {
    pub fn from_le_bytes(key: [u8; 32]) -> Result<Self, InvalidPublicKeyError> {
        // A byte-wise test for 0 and `N`, the only 32-byte multiples of `N` (`2N` needs 33 bytes).
        let only_zero_or_prime = key
            .iter()
            .zip(LARGE_SAFE_PRIME_LITTLE_ENDIAN.iter())
            .all(|(&k, &n)| k == 0 || k == n);
        if only_zero_or_prime {
            return Err(if key[0] == 0 {
                InvalidPublicKeyError::IsZero
            } else {
                InvalidPublicKeyError::ModLargeSafePrimeIsZero
            });
        }
        Ok(Self { key })
    }

    pub const fn as_le_bytes(&self) -> &[u8; 32] {
        &self.key
    }

    /// Whether this key hashes the same under both conventions; for the server's `B`, false is the
    /// cue to redial for a fresh challenge.
    pub const fn is_width_stable(&self) -> bool {
        self.key[31] != 0
    }

    fn as_bigint(&self) -> BigInt {
        from_le(&self.key)
    }
}

// --- SRP6 client ---------------------------------------------------------------------------------

/// `SHA1(N) XOR SHA1(g)`, the head of `M1`, from the `g` and `N` the server sent.
fn xor_hash(generator: u8, large_safe_prime: &[u8; 32]) -> [u8; 20] {
    let n_hash = sha1(&[large_safe_prime]);
    let g_hash = sha1(&[&[generator]]);
    let mut out = [0u8; 20];
    for (o, (n, g)) in out.iter_mut().zip(n_hash.iter().zip(g_hash.iter())) {
        *o = n ^ g;
    }
    out
}

/// `x = SHA1( salt | SHA1( UPPER(user) ":" UPPER(pass) ) )`.
fn calculate_x(
    username: &NormalizedString,
    password: &NormalizedString,
    salt: &[u8; 32],
) -> [u8; 20] {
    let inner = sha1(&[
        username.as_ref().as_bytes(),
        b":",
        password.as_ref().as_bytes(),
    ]);
    sha1(&[salt, &inner])
}

/// `u = SHA1( A | B )` (both little endian).
fn calculate_u(client_public_key: &PublicKey, server_public_key: &PublicKey) -> [u8; 20] {
    sha1(&[
        client_public_key.as_le_bytes(),
        server_public_key.as_le_bytes(),
    ])
}

/// Fold `S` (32 LE bytes) into the 40-byte session key: SHA-1 the even and the odd bytes apart and
/// interleave the two digests (WoW's `SHA1_Interleave`).
///
/// The client strips leading zero bytes first, the low bytes of a little-endian `S` (`0x5d3360`);
/// vmangos hashes all 32 (`SRP6::HashSessionKey`, `S.AsByteArray(32)`). This hashes all 32, and
/// [`SrpClientChallenge::new`] never keeps an `S` with a zero low byte, so this `K` is both.
fn calculate_interleaved(s: &[u8; 32]) -> [u8; 40] {
    let mut e = [0u8; 16];
    for (i, b) in s.iter().step_by(2).enumerate() {
        e[i] = *b;
    }
    let g = sha1(&[&e]);

    let mut f = [0u8; 16];
    for (i, b) in s.iter().skip(1).step_by(2).enumerate() {
        f[i] = *b;
    }
    let h = sha1(&[&f]);

    let mut out = [0u8; 40];
    for (i, (gi, hi)) in g.iter().zip(h.iter()).enumerate() {
        out[i * 2] = *gi;
        out[i * 2 + 1] = *hi;
    }
    out
}

/// `M2 = SHA1( A | M1 | K )`, the server's proof back.
fn calculate_server_proof(
    client_public_key: &PublicKey,
    client_proof: &[u8; 20],
    session_key: &[u8; 40],
) -> [u8; 20] {
    sha1(&[client_public_key.as_le_bytes(), client_proof, session_key])
}

/// The client's side of the logon proof: `A` and `M1` go in `CMD_AUTH_LOGON_PROOF_Client`, and the
/// server's `M2` is checked with [`SrpClientChallenge::verify_server_proof`].
#[derive(Debug, Clone)]
pub struct SrpClientChallenge {
    username: NormalizedString,
    client_proof: [u8; 20],
    client_public_key: [u8; 32],
    session_key: [u8; 40],
}

/// The cap on ephemerals [`SrpClientChallenge::new`] draws, so a hostile `N`/`g` cannot spin it
/// forever. Each passes about 97.4% of the time; the last is sent regardless, which can fail the
/// logon but never corrupt it.
const MAX_EPHEMERAL_DRAWS: u32 = 512;

impl SrpClientChallenge {
    /// Compute `A`, `M1` and the session key as the client does: a random 32-byte `a`,
    /// `A = g^a mod N`, `S = (B - k·g^x)^(a + u·x) mod N`, `K = interleave(S)` and
    /// `M1 = SHA1( H(N)^H(g) | SHA1(user) | salt | A | B | K )`. Deviation: `a` is redrawn until
    /// `A`, `K` and `M1` have no high-order zero byte and `S` no low-order one, because the client
    /// and vmangos hash those values differently.
    pub fn new(
        username: NormalizedString,
        password: NormalizedString,
        generator: u8,
        large_safe_prime: [u8; 32],
        server_public_key: PublicKey,
        salt: [u8; 32],
    ) -> SrpClientChallenge {
        Self::new_with_rng(
            &mut thread_rng(),
            username,
            password,
            generator,
            large_safe_prime,
            server_public_key,
            salt,
        )
    }

    /// [`Self::new`] with the draw source injected, so a test can script a draw that trips a guard.
    fn new_with_rng<R: RngCore>(
        rng: &mut R,
        username: NormalizedString,
        password: NormalizedString,
        generator: u8,
        large_safe_prime: [u8; 32],
        server_public_key: PublicKey,
        salt: [u8; 32],
    ) -> SrpClientChallenge {
        let n = from_le(&large_safe_prime);
        let g = BigInt::from(generator);
        let k = BigInt::from(K_VALUE);

        let x = from_le(&calculate_x(&username, &password, &salt));
        let s_base = server_public_key.as_bigint() - &k * g.modpow(&x, &n);
        let xor = xor_hash(generator, &large_safe_prime);
        let username_hash = sha1(&[username.as_ref().as_bytes()]);

        for draw in 1..=MAX_EPHEMERAL_DRAWS {
            let last = draw == MAX_EPHEMERAL_DRAWS;

            let mut private_key = [0u8; 32];
            rng.fill_bytes(&mut private_key);
            let a = from_le(&private_key);

            let client_public_key = to_padded_32_le(&g.modpow(&a, &n));
            if !is_width_stable(&client_public_key) && !last {
                continue;
            }

            let client_pk = PublicKey::from_le_bytes(client_public_key)
                .expect("generated client public key is valid");
            let u = from_le(&calculate_u(&client_pk, &server_public_key));

            // S = (B - k·(g^x mod N))^(a + u·x) mod N
            let s = to_padded_32_le(&s_base.modpow(&(&a + &u * &x), &n));
            // The client's interleave strips a zero low byte: decline it, so `K` is both sides'.
            if s[0] == 0 && !last {
                continue;
            }

            let session_key = calculate_interleaved(&s);
            if !is_width_stable(&session_key) && !last {
                continue;
            }

            let client_proof = sha1(&[
                &xor,
                &username_hash,
                &salt,
                &client_public_key,
                server_public_key.as_le_bytes(),
                &session_key,
            ]);
            // M1 feeds the server's M2: an ambiguous one passes realmd but fails our M2 check.
            if !is_width_stable(&client_proof) && !last {
                continue;
            }

            return SrpClientChallenge {
                username,
                client_proof,
                client_public_key,
                session_key,
            };
        }
        unreachable!("the final draw is accepted unconditionally")
    }

    /// Our proof `M1`, little endian.
    pub const fn client_proof(&self) -> &[u8; 20] {
        &self.client_proof
    }

    /// Our public key `A`, little endian.
    pub const fn client_public_key(&self) -> &[u8; 32] {
        &self.client_public_key
    }

    /// Verify the server's proof `M2`; the returned [`SrpClient`] holds the session key.
    pub fn verify_server_proof(
        self,
        server_proof: [u8; 20],
    ) -> Result<SrpClient, MatchProofsError> {
        let expected = calculate_server_proof(
            &PublicKey::from_le_bytes(self.client_public_key)
                .expect("our own client public key is valid"),
            &self.client_proof,
            &self.session_key,
        );
        if server_proof != expected {
            return Err(MatchProofsError {
                server_proof,
                expected,
            });
        }
        Ok(SrpClient {
            username: self.username,
            session_key: self.session_key,
        })
    }
}

/// The server's proof `M2` did not match what we computed (usually a wrong password).
#[derive(Debug)]
pub struct MatchProofsError {
    pub server_proof: [u8; 20],
    pub expected: [u8; 20],
}
impl std::fmt::Display for MatchProofsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "server proof mismatch (wrong password?)")
    }
}
impl std::error::Error for MatchProofsError {}

/// A completed SRP6 logon: holds the session key carried into the world server.
#[derive(Debug, Clone)]
pub struct SrpClient {
    #[allow(dead_code)]
    username: NormalizedString,
    session_key: [u8; 40],
}

impl SrpClient {
    /// The SRP session key `K` (40 little-endian bytes).
    pub const fn session_key(&self) -> &[u8; 40] {
        &self.session_key
    }
}

// --- account creation (server-side verifier) -----------------------------------------------------

/// The SRP6 password verifier `v = g^x mod N`, little endian: what a server stores beside the salt.
pub fn password_verifier(
    username: &NormalizedString,
    password: &NormalizedString,
    salt: &[u8; 32],
) -> [u8; 32] {
    let n = from_le(&LARGE_SAFE_PRIME_LITTLE_ENDIAN);
    let g = BigInt::from(GENERATOR);
    let x = from_le(&calculate_x(username, password, salt));
    to_padded_32_le(&g.modpow(&x, &n))
}

/// A fresh random salt and its [`password_verifier`] for a new account, both little endian. The
/// salt's top bit is set, as vmangos's `BigNumber::SetRand(256)` does (`BN_rand(_bn, 256, 0, 1)`):
/// a salt lives as long as its account, so an ambiguous one would never log in.
pub fn generate_account(
    username: &NormalizedString,
    password: &NormalizedString,
) -> ([u8; 32], [u8; 32]) {
    let mut salt = [0u8; 32];
    thread_rng().fill_bytes(&mut salt);
    salt[31] |= 0x80;
    let verifier = password_verifier(username, password, &salt);
    (salt, verifier)
}

#[cfg(test)]
mod tests {
    //! The interleave and `x` are pinned to the published WoW SRP6 vectors (the `wow_srp` crate's
    //! MIT corpus); the header cipher and password verifier to goldens from that implementation.
    use super::*;

    fn hx(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
    fn rev(mut v: Vec<u8>) -> Vec<u8> {
        v.reverse();
        v
    }

    #[test]
    fn interleave_known_answer() {
        // (S little-endian, expected session key little-endian) from the WoW SRP6 vector corpus.
        let cases = [
            (
                "8F4CEBD60DFC34E5C007E51BD4F3A4FF2BC1D930E2D3EA770D8D3EEDFF2DCCFC",
                "EE144E1AE08DAC891AB63ABC42BF89738003343422E6B58131BEE4C3087A7027E55A7216D18D556C",
            ),
            (
                "CCC1BDE07FC4FA3182DDEAAB036A88F78AD605AB0D8BFBF6F5EE8ED65CDE4F09",
                "2AA23706C3FC3517A4293E2D4944F567E220CC1A227359D70154E5FD3CEE973673130C4AFBAD9E6D",
            ),
        ];
        for (s_hex, key_hex) in cases {
            let s: [u8; 32] = hx(s_hex).try_into().unwrap();
            assert_eq!(calculate_interleaved(&s).to_vec(), hx(key_hex), "S={s_hex}");
        }
    }

    #[test]
    fn interleave_does_not_trim_low_order_zero_bytes() {
        let mut s = [0u8; 32];
        s.copy_from_slice(&hx(
            "8F4CEBD60DFC34E5C007E51BD4F3A4FF2BC1D930E2D3EA770D8D3EEDFF2DCCFC",
        ));
        let mut zeroed = s;
        zeroed[0] = 0;
        zeroed[1] = 0;

        // The untrimmed split, built by hand; a trim would re-split the remaining 30 bytes.
        let mut expect_even = [0u8; 16];
        let mut expect_odd = [0u8; 16];
        for i in 0..16 {
            expect_even[i] = zeroed[i * 2];
            expect_odd[i] = zeroed[i * 2 + 1];
        }
        let g = sha1(&[&expect_even]);
        let h = sha1(&[&expect_odd]);
        let mut expected = [0u8; 40];
        for (i, (gi, hi)) in g.iter().zip(h.iter()).enumerate() {
            expected[i * 2] = *gi;
            expected[i * 2 + 1] = *hi;
        }

        assert_eq!(calculate_interleaved(&zeroed), expected);
        assert_ne!(calculate_interleaved(&zeroed), calculate_interleaved(&s));
    }

    #[test]
    fn calculate_x_known_answer() {
        // The corpus writes the salt and `x` big-endian; we store both little-endian.
        let salt: [u8; 32] = rev(hx(
            "CAC94AF32D817BA64B13F18FDEDEF92AD4ED7EF7AB0E19E9F2AE13C828AEAF57",
        ))
        .try_into()
        .unwrap();
        let cases = [
            (
                "00XD0QOSA9L8KMXC",
                "43R4Z35TKBKFW8JI",
                "E2F9A0F1E824006C98DA753448E743F7DAA1EAA1",
            ),
            (
                "01GJDP3DSFHR56JQ",
                "9ZK1PFJ9LA0JSHPR",
                "553A6123ABCFD539F2E0B77F64860C64675BC0FD",
            ),
        ];
        for (user, pass, x_hex) in cases {
            let x = calculate_x(
                &NormalizedString::new(user).unwrap(),
                &NormalizedString::new(pass).unwrap(),
                &salt,
            );
            assert_eq!(x.to_vec(), rev(hx(x_hex)), "user={user}");
        }
    }

    #[test]
    fn width_stability_is_the_high_order_byte() {
        assert!(is_width_stable(&[0, 0, 1]));
        assert!(!is_width_stable(&[1, 1, 0]));
        assert!(!is_width_stable(&[]));
    }

    #[test]
    fn every_drawn_handshake_is_encoding_unambiguous() {
        use rand::{rngs::StdRng, SeedableRng};
        let user = NormalizedString::new("alice").unwrap();
        let pass = NormalizedString::new("password1").unwrap();
        let mut rng = StdRng::seed_from_u64(0x5875);
        for i in 0..128u32 {
            let mut b = [0u8; 32];
            rng.fill_bytes(&mut b);
            b[31] |= 0x80; // the shape of server key `logon` keeps
            let salt = std::array::from_fn(|j| (j as u8).wrapping_mul(11).wrapping_add(i as u8));
            let c = SrpClientChallenge::new_with_rng(
                &mut rng,
                user.clone(),
                pass.clone(),
                GENERATOR,
                LARGE_SAFE_PRIME_LITTLE_ENDIAN,
                PublicKey::from_le_bytes(b).unwrap(),
                salt,
            );
            assert!(is_width_stable(c.client_public_key()), "A, draw {i}");
            assert!(is_width_stable(&c.session_key), "K, draw {i}");
            assert!(is_width_stable(c.client_proof()), "M1, draw {i}");
            // The M2 we would check against is built from those same bytes, so it round-trips.
            let m2 = calculate_server_proof(
                &PublicKey::from_le_bytes(*c.client_public_key()).unwrap(),
                c.client_proof(),
                &c.session_key,
            );
            assert!(c.verify_server_proof(m2).is_ok(), "M2, draw {i}");
        }
    }

    /// A draw source that hands out exactly the private keys it was given, in order.
    struct Scripted(std::collections::VecDeque<[u8; 32]>);

    impl RngCore for Scripted {
        fn next_u32(&mut self) -> u32 {
            unreachable!("the ephemeral is drawn with fill_bytes")
        }
        fn next_u64(&mut self) -> u64 {
            unreachable!("the ephemeral is drawn with fill_bytes")
        }
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            let key = self
                .0
                .pop_front()
                .expect("a scripted draw was left for this");
            dest.copy_from_slice(&key);
        }
        fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
            self.fill_bytes(dest);
            Ok(())
        }
    }

    /// The `B` (high bit set, as `logon` keeps) and salt the guard vectors were found against;
    /// change either and the vectors mean nothing.
    const FIXTURE_B: [u8; 32] = [
        5, 42, 79, 116, 153, 190, 227, 8, 45, 82, 119, 156, 193, 230, 11, 48, 85, 122, 159, 196,
        233, 14, 51, 88, 125, 162, 199, 236, 17, 54, 91, 128,
    ];
    const FIXTURE_SALT: [u8; 32] = [
        3, 14, 25, 36, 47, 58, 69, 80, 91, 102, 113, 124, 135, 146, 157, 168, 179, 190, 201, 212,
        223, 234, 245, 0, 11, 22, 33, 44, 55, 66, 77, 88,
    ];
    /// Private keys that each trip one guard against the fixture: `TRIPS_A` gives `A` a high-order
    /// zero byte, `TRIPS_S` gives `S` a low-order one, `TRIPS_K` and `TRIPS_M1` give the key and
    /// the proof a high-order one; `CLEAN` trips none.
    const TRIPS_A: [u8; 32] = [
        237, 250, 2, 239, 106, 197, 124, 117, 132, 5, 226, 189, 212, 217, 169, 146, 39, 212, 214,
        11, 198, 57, 225, 84, 17, 219, 168, 107, 118, 105, 11, 95,
    ];
    const TRIPS_S: [u8; 32] = [
        205, 28, 249, 67, 94, 223, 246, 194, 118, 47, 78, 155, 220, 133, 163, 151, 42, 72, 65, 90,
        37, 9, 156, 83, 38, 132, 96, 6, 129, 245, 202, 203,
    ];
    const TRIPS_K: [u8; 32] = [
        2, 176, 65, 251, 162, 142, 23, 37, 53, 25, 44, 220, 201, 39, 234, 48, 141, 207, 127, 19, 0,
        93, 140, 154, 16, 181, 118, 225, 147, 237, 174, 28,
    ];
    const TRIPS_M1: [u8; 32] = [
        143, 237, 243, 79, 25, 151, 163, 98, 11, 221, 191, 202, 69, 148, 140, 221, 123, 95, 152,
        146, 230, 232, 181, 86, 18, 72, 78, 135, 95, 129, 234, 248,
    ];
    const CLEAN: [u8; 32] = [
        52, 226, 21, 16, 93, 106, 129, 125, 56, 197, 129, 14, 149, 1, 254, 105, 63, 23, 9, 190, 46,
        113, 240, 125, 128, 130, 150, 46, 174, 240, 93, 249,
    ];

    /// `A = g^a mod N` for a private key, as the constructor computes it.
    fn public_key_of(private_key: &[u8; 32]) -> [u8; 32] {
        let n = from_le(&LARGE_SAFE_PRIME_LITTLE_ENDIAN);
        to_padded_32_le(&BigInt::from(GENERATOR).modpow(&from_le(private_key), &n))
    }

    /// Each guard gets a key known to trip it, then a clean one, and must return the clean draw.
    #[test]
    fn each_encoding_guard_rejects_the_draw_it_exists_for() {
        let user = NormalizedString::new("alice").unwrap();
        let pass = NormalizedString::new("password1").unwrap();
        let server = PublicKey::from_le_bytes(FIXTURE_B).unwrap();
        let clean_a = public_key_of(&CLEAN);

        // The fixture still means what the vectors say: each trips its guard, CLEAN trips none.
        assert!(
            !is_width_stable(&public_key_of(&TRIPS_A)),
            "TRIPS_A no longer trips A"
        );
        assert!(is_width_stable(&clean_a), "CLEAN no longer passes A");

        for (guard, bad) in [
            ("A", TRIPS_A),
            ("S", TRIPS_S),
            ("K", TRIPS_K),
            ("M1", TRIPS_M1),
        ] {
            let mut rng = Scripted([bad, CLEAN].into_iter().collect());
            let c = SrpClientChallenge::new_with_rng(
                &mut rng,
                user.clone(),
                pass.clone(),
                GENERATOR,
                LARGE_SAFE_PRIME_LITTLE_ENDIAN,
                server,
                FIXTURE_SALT,
            );
            assert_eq!(
                c.client_public_key(),
                &clean_a,
                "guard {guard}: the tripping draw was accepted instead of the clean one"
            );
            assert!(
                rng.0.is_empty(),
                "guard {guard}: the clean draw was never reached"
            );
            assert!(is_width_stable(c.client_public_key()));
            assert!(is_width_stable(&c.session_key));
            assert!(is_width_stable(c.client_proof()));
        }

        // And a draw that trips nothing is taken first time.
        let mut rng = Scripted([CLEAN, TRIPS_A].into_iter().collect());
        let c = SrpClientChallenge::new_with_rng(
            &mut rng,
            user,
            pass,
            GENERATOR,
            LARGE_SAFE_PRIME_LITTLE_ENDIAN,
            server,
            FIXTURE_SALT,
        );
        assert_eq!(c.client_public_key(), &clean_a);
        assert_eq!(rng.0.len(), 1, "a clean first draw is the handshake");
    }

    #[test]
    fn generated_salts_are_encoding_unambiguous() {
        let user = NormalizedString::new("alice").unwrap();
        let pass = NormalizedString::new("password1").unwrap();
        for _ in 0..2048 {
            let (salt, _) = generate_account(&user, &pass);
            assert!(is_width_stable(&salt));
        }
    }

    #[test]
    fn password_verifier_golden() {
        let salt: [u8; 32] = std::array::from_fn(|i| (i as u8).wrapping_mul(11).wrapping_add(2));
        let v = password_verifier(
            &NormalizedString::new("alice").unwrap(),
            &NormalizedString::new("password1").unwrap(),
            &salt,
        );
        assert_eq!(
            v.to_vec(),
            hx("28b837075a12b82553921d9095fa3fdcb0151c4bfc860ab97a69d0fa86a3d213")
        );
    }

    #[test]
    fn header_cipher_golden() {
        let sk: [u8; 40] = std::array::from_fn(|i| (i as u8).wrapping_mul(7).wrapping_add(3));
        let (_p, crypto) = ProofSeed::new().into_client_header_crypto(
            &NormalizedString::new("alice").unwrap(),
            sk,
            0xDEAD_BEEF,
        );
        let (mut enc, mut dec) = crypto.split();
        assert_eq!(
            enc.encrypt_client_header(12, 0x37F).to_vec(),
            hx("03097792b1d7")
        );
        assert_eq!(
            enc.encrypt_client_header(0x1FF, 0xC7).to_vec(),
            hx("03ceca0c55a5")
        );
        let mut buf: Vec<u8> = (0..32u16).map(|i| (i as u8).wrapping_mul(13)).collect();
        dec.decrypt(&mut buf);
        assert_eq!(
            buf,
            hx("03071c15122b2039364f445d5a5368617e778c85829b90a9a6bfb4cdcac3d8d1")
        );
    }
}
