//! SRP6 client + vanilla world-header crypto for **WoW 1.12.1 (build 5875)** — in-repo, replacing
//! `wow_srp`.
//!
//! WoW uses a lightly customised SRP6 for logon: a fixed 32-byte safe prime `N`, generator `g = 7`,
//! multiplier `k = 3`, and a bespoke "interleave" that folds the shared secret `S` into the 40-byte
//! session key. We implement only the **client** side (the realmd handshake) plus the world server's
//! header obfuscation — the only pieces benilla needs.
//!
//! All byte arrays are **little endian** on the wire (as the client sends them).
//!
//! Proven by a full SRP6 round-trip against `wow_srp`'s server + a byte-exact header-cipher diff
//! during the decision-0021 migration (oracle test in git history); ongoing regression coverage is
//! the oracle-free known-answer + golden tests in this crate.
//!
//! # Encoding-unambiguous handshakes
//!
//! The values the handshake feeds to SHA-1 (`A`, `B`, `K`, the salt, `M1`) are *numbers*, and the two
//! implementations in the wild serialize a number differently:
//!
//! - The **1.12.1 client** writes each at its declared width, zero-padded in the high bytes — `A`/`B`
//!   32, `K` 40, `M1` 20 (byte-exact from `WoW.exe` `0x5d3650`).
//! - The **mangos family** (vmangos `SHA1::Generator::UpdateData(BigNumber const&)`, cmangos
//!   `Sha1Hash::UpdateBigNumbers`) writes `BigNumber::AsByteArray()` with no minimum — **high-order
//!   zero bytes dropped**.
//!
//! They agree only while no value happens to have a high-order zero byte, and disagree silently when
//! one does: realmd answers `WOW_FAIL_UNKNOWN_ACCOUNT` (0x04) to a perfectly correct password. There
//! is a matching disagreement at the other end of `S` (see [`calculate_interleaved`]). Measured
//! against a live vmangos, a client that picks a side loses ~1 handshake in 45 — see
//! `benilla-protocol`'s `srp_encoding_probe` example, which forces each case and prints the verdict.
//!
//! benilla picks **neither side**. `a` is ours to draw, and every ambiguous value is downstream of
//! it, so [`SrpClientChallenge::new`] simply redraws until the handshake it is about to send is one
//! both conventions serialize identically — see [`is_width_stable`]. What we send is then bit-for-bit
//! the real client's arithmetic *and* accepted by a mangos-family server, with no branch on which
//! kind of server we are talking to. The one value we cannot draw is the server's `B`; `benilla-
//! protocol`'s `logon` redials for a fresh challenge when that one lands ambiguous.

use num_bigint::BigInt;
use rand::{thread_rng, RngCore};
use sha1::{Digest, Sha1};

pub mod vanilla_header;

pub use vanilla_header::{DecrypterHalf, EncrypterHalf, HeaderCrypto, ProofSeed};

/// Session-key length in bytes — always 40 (two concatenated SHA-1 hashes).
pub const SESSION_KEY_LENGTH: usize = 40;
/// Proof (`M1`/`M2`) length in bytes — a SHA-1 hash.
pub const PROOF_LENGTH: usize = 20;
/// Public-key (`A`/`B`) length in bytes.
pub const PUBLIC_KEY_LENGTH: usize = 32;
/// Salt length in bytes.
pub const SALT_LENGTH: usize = 32;
/// Generator `g` — statically 7 for WoW.
pub const GENERATOR: u8 = 7;
/// The WoW safe prime `N`, little endian (as sent in `CMD_AUTH_LOGON_CHALLENGE_Server`).
pub const LARGE_SAFE_PRIME_LITTLE_ENDIAN: [u8; 32] = [
    0xb7, 0x9b, 0x3e, 0x2a, 0x87, 0x82, 0x3c, 0xab, 0x8f, 0x5e, 0xbf, 0xbf, 0x8e, 0xb1, 0x1, 0x8,
    0x53, 0x50, 0x6, 0x29, 0x8b, 0x5b, 0xad, 0xbd, 0x5b, 0x53, 0xe1, 0x89, 0x5e, 0x64, 0x4b, 0x89,
];
/// The SRP multiplier `k` — statically 3 in this (pre-SRP6a) flavour.
const K_VALUE: u8 = 3;

// --- bigint helpers (little-endian, unsigned magnitude) -------------------------------------------

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

/// Does this little-endian value hash the same whichever serialization the peer uses — its declared
/// width (the 1.12.1 client) or its minimal length (the mangos family)? True exactly when the
/// high-order byte is non-zero, since minimal encoding differs from padded only by dropping those.
/// See the crate docs, "Encoding-unambiguous handshakes".
fn is_width_stable(little_endian: &[u8]) -> bool {
    matches!(little_endian.last(), Some(&b) if b != 0)
}

// --- normalized string ----------------------------------------------------------------------------

/// A username/password normalised the way the 1.12 client does it: ASCII only (no control chars),
/// uppercased, 1..=16 bytes. The SRP6 hashes are computed over this form, and the uppercased account
/// name is what's sent on the wire.
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
    /// Validate + uppercase `s`. See [`NormalizedString`].
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

// --- public key -----------------------------------------------------------------------------------

/// A validated SRP public key (`A` or `B`), stored little endian. Rejected if it is exactly zero or
/// exactly the safe prime `N` (the only 32-byte values that are `0 mod N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKey {
    key: [u8; 32],
}

/// Error from [`PublicKey::from_le_bytes`].
#[derive(Debug)]
pub enum InvalidPublicKeyError {
    /// The key is all zeros.
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
    /// Build from little-endian bytes, validating the key (see [`PublicKey`]).
    pub fn from_le_bytes(key: [u8; 32]) -> Result<Self, InvalidPublicKeyError> {
        // Valid unless every byte is either 0 or matches N[i] — i.e. the key is exactly 0 or exactly
        // N. (A multiple of N ≥ 2N needs 33 bytes, so those are the only two zero residues.)
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

    /// The key as little-endian bytes.
    pub const fn as_le_bytes(&self) -> &[u8; 32] {
        &self.key
    }

    /// Whether this key hashes identically under both serialization conventions (crate docs,
    /// "Encoding-unambiguous handshakes"). [`SrpClientChallenge::new`] guarantees it for the `A` it
    /// draws; for the server's `B` it is luck, and the caller's cue to redial for a fresh challenge.
    pub const fn is_width_stable(&self) -> bool {
        self.key[31] != 0
    }

    fn as_bigint(&self) -> BigInt {
        from_le(&self.key)
    }
}

// --- SRP6 client ----------------------------------------------------------------------------------

/// `H( SHA1(N) XOR SHA1(g) )` — folded into the client proof `M1`. Computed from the server-supplied
/// `g`/`N` (they're fixed for WoW, but we don't assume it).
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

/// Fold the shared secret `S` (32 LE bytes) into the 40-byte session key: split the even / odd bytes,
/// SHA-1 each half, then interleave the two digests. This is WoW's specific `SHA1_Interleave`.
///
/// This is the crate-doc encoding split again, at the *low* end of `S`. The SRP-6 RFC strips leading
/// zero bytes before the split and the real client does too (from `WoW.exe` `0x5d3360`, bit-exact)
/// — leading in its little-endian `S` meaning the **low** bytes. vmangos does not: it hashes all 32
/// unconditionally (`SRP6::HashSessionKey`, `S.AsByteArray(32)`). A trim would derive a different
/// `K`, hence a different `M1`, for the ~1-in-256 `S` ending in a zero low byte — an intermittent
/// `WOW_FAIL_UNKNOWN_ACCOUNT` (0x04) on a correct password.
///
/// We hash all 32 bytes, and [`SrpClientChallenge::new`] only keeps an `S` whose low byte is
/// non-zero — which makes the strip a no-op, so this `K` is simultaneously the real client's and
/// vmangos'. Neither convention is chosen; the ambiguous inputs are simply never presented.
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

/// `M2 = SHA1( A | M1 | K )` — what the server proves back and we verify.
fn calculate_server_proof(
    client_public_key: &PublicKey,
    client_proof: &[u8; 20],
    session_key: &[u8; 40],
) -> [u8; 20] {
    sha1(&[client_public_key.as_le_bytes(), client_proof, session_key])
}

/// First step of the client logon: given the server's challenge values, computes our public key `A`,
/// the proof `M1`, and the session key. Send `A` + `M1` in `CMD_AUTH_LOGON_PROOF_Client`, then call
/// [`SrpClientChallenge::verify_server_proof`] with the server's `M2`.
#[derive(Debug, Clone)]
pub struct SrpClientChallenge {
    username: NormalizedString,
    client_proof: [u8; 20],
    client_public_key: [u8; 32],
    session_key: [u8; 40],
}

/// How many ephemerals [`SrpClientChallenge::new`] will draw looking for an encoding-unambiguous
/// handshake. Each draw succeeds ~97.4% of the time, so the loop all but always ends on the first;
/// the bound only exists so a degenerate `N`/`g` from a hostile server cannot spin forever. On
/// exhaustion we send the last draw anyway — the same handshake we would have sent before this
/// guarantee existed, i.e. it can fail the logon but cannot corrupt one.
const MAX_EPHEMERAL_DRAWS: u32 = 512;

impl SrpClientChallenge {
    /// Compute `A`, `M1`, and the session key from the server challenge. Mirrors the real client:
    /// random 32-byte private key `a`, `A = g^a mod N`, `S = (B - k·g^x)^(a + u·x) mod N`, session
    /// key = interleave(S), `M1 = SHA1( H(N)^H(g) | SHA1(user) | salt | A | B | K )`.
    ///
    /// `a` is redrawn until every value the handshake serializes is one both conventions in the wild
    /// encode identically (crate docs, "Encoding-unambiguous handshakes"): `A`, `K` and `M1` with no
    /// high-order zero byte, and `S` with no low-order one. The arithmetic is untouched — this only
    /// declines to *use* an ephemeral whose handshake would be read two ways.
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

    /// [`Self::new`] with the ephemeral's draw source injected. `new` hands it `thread_rng`; the
    /// tests hand it a scripted one, so each guard in the loop below is exercised on purpose —
    /// a draw known to trip it, then a clean one — rather than by the ~1-in-137 chance a random
    /// sweep gives each.
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

        // Everything the draw does not move, hoisted out of the loop.
        let x = from_le(&calculate_x(&username, &password, &salt));
        let s_base = server_public_key.as_bigint() - &k * g.modpow(&x, &n);
        let xor = xor_hash(generator, &large_safe_prime);
        let username_hash = sha1(&[username.as_ref().as_bytes()]);

        for draw in 1..=MAX_EPHEMERAL_DRAWS {
            let last = draw == MAX_EPHEMERAL_DRAWS;

            let mut private_key = [0u8; 32];
            rng.fill_bytes(&mut private_key);
            let a = from_le(&private_key);

            // A = g^a mod N
            let client_public_key = to_padded_32_le(&g.modpow(&a, &n));
            if !is_width_stable(&client_public_key) && !last {
                continue;
            }

            let client_pk = PublicKey::from_le_bytes(client_public_key)
                .expect("generated client public key is valid");
            let u = from_le(&calculate_u(&client_pk, &server_public_key));

            // S = (B - k·(g^x mod N))^(a + u·x) mod N
            let s = to_padded_32_le(&s_base.modpow(&(&a + &u * &x), &n));
            // A zero low byte is the one the real client's interleave would strip (see
            // `calculate_interleaved`) — decline it so our K is both implementations' K.
            if s[0] == 0 && !last {
                continue;
            }

            let session_key = calculate_interleaved(&s);
            if !is_width_stable(&session_key) && !last {
                continue;
            }

            // M1 = SHA1( xor_hash | SHA1(username) | salt | A | B | K )
            let client_proof = sha1(&[
                &xor,
                &username_hash,
                &salt,
                &client_public_key,
                server_public_key.as_le_bytes(),
                &session_key,
            ]);
            // M1 is itself hashed back into the server's M2, so it needs the guarantee too — this is
            // the case realmd *accepts* while the M2 it replies with fails our check.
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

    /// Our proof `M1` (little endian) — send in `CMD_AUTH_LOGON_PROOF_Client`.
    pub const fn client_proof(&self) -> &[u8; 20] {
        &self.client_proof
    }

    /// Our public key `A` (little endian) — send in `CMD_AUTH_LOGON_PROOF_Client`.
    pub const fn client_public_key(&self) -> &[u8; 32] {
        &self.client_public_key
    }

    /// Verify the server's proof `M2`. On success the SRP handshake is complete and the session key
    /// is held in the returned [`SrpClient`].
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
    /// The proof the server sent.
    pub server_proof: [u8; 20],
    /// The proof we expected.
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

// --- account creation (server-side verifier) ------------------------------------------------------

/// The SRP6 **password verifier** `v = g^x mod N` (little endian) for a given salt — the value a
/// server stores alongside the salt so it never holds the raw password. Deterministic in
/// `(username, password, salt)`. See [`generate_account`] for the new-account path.
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

/// Generate a fresh random salt and the matching [`password_verifier`] for a new account. Returns
/// `(salt, verifier)`, both little endian.
///
/// The salt's top bit is forced, exactly as vmangos' own account creation does it
/// (`BigNumber::SetRand(256)` → `BN_rand(_bn, 256, 0, 1)`, whose `top = 0` sets the MSB). The salt is
/// the one hashed value fixed for the life of the account rather than redrawn per handshake, so a
/// high-order zero byte there is not a 1-in-256 *login* failure but a permanently unloggable account
/// (crate docs, "Encoding-unambiguous handshakes").
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
    //! Oracle-free regression tests. The SRP6 *interleave* and `x` derivation are pinned to published
    //! WoW SRP6 test vectors (the gtker/wow_srp corpus, MIT — the same values used to byte-validate
    //! this crate against `wow_srp` during the decision-0021 migration). The header cipher + password
    //! verifier are pinned to golden outputs captured from that validated implementation.
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

    /// A low-order zero byte in `S` must *not* shorten the split: we hash all 32 bytes, so `S` and
    /// `S` with its low byte zeroed differ only in that byte, never in length. (Trimming is the
    /// SRP-6 RFC's — and the real client's — behaviour, and against vmangos it is what made ~1 login
    /// in 256 come back as "wrong password"; `SrpClientChallenge::new` keeps the two identical by
    /// never drawing an `S` with a zero low byte at all.)
    #[test]
    fn interleave_does_not_trim_low_order_zero_bytes() {
        let mut s = [0u8; 32];
        s.copy_from_slice(&hx(
            "8F4CEBD60DFC34E5C007E51BD4F3A4FF2BC1D930E2D3EA770D8D3EEDFF2DCCFC",
        ));
        let mut zeroed = s;
        zeroed[0] = 0;
        zeroed[1] = 0;

        // Even halves: byte 0 is the only difference; odd halves: byte 1. Both must change, and the
        // trimming implementation would instead have re-split the remaining 30 bytes wholesale.
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
        // Fixed salt (big-endian in the corpus); user/pass → x (big-endian in the corpus). We store
        // both little-endian, so reverse the corpus hex.
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

    /// The crate's guarantee as a property: every handshake we hand out is one both serialization
    /// conventions read identically, so no value carries a high-order zero byte. Seeded (2331), so
    /// the 128 server keys and every ephemeral drawn against them are the same on every machine —
    /// a red run reproduces. The per-guard proof is `each_encoding_guard_rejects_the_draw_it_exists_for`;
    /// the live gate is `benilla-protocol`'s `srp_encoding_probe`, which forces each case against
    /// a real realmd.
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

    /// The server side of the fixture the guard vectors were found against: a fixed `B` (with the
    /// high bit `logon` keeps) and a fixed salt. Change either and the vectors below stop meaning
    /// anything — the test says so.
    const FIXTURE_B: [u8; 32] = [
        5, 42, 79, 116, 153, 190, 227, 8, 45, 82, 119, 156, 193, 230, 11, 48, 85, 122, 159, 196,
        233, 14, 51, 88, 125, 162, 199, 236, 17, 54, 91, 128,
    ];
    const FIXTURE_SALT: [u8; 32] = [
        3, 14, 25, 36, 47, 58, 69, 80, 91, 102, 113, 124, 135, 146, 157, 168, 179, 190, 201, 212,
        223, 234, 245, 0, 11, 22, 33, 44, 55, 66, 77, 88,
    ];
    /// Private keys that, against [`FIXTURE_B`]/[`FIXTURE_SALT`], trip exactly one guard each —
    /// found once by a seeded search (`StdRng::seed_from_u64(2331)`, first hit per guard) and
    /// pinned. `TRIPS_A`: `A` gets a high-order zero byte. `TRIPS_S`: `S` gets a low-order zero
    /// byte. `TRIPS_K`: the interleaved key gets a high-order zero. `TRIPS_M1`: the proof does.
    /// `CLEAN` passes all four.
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

    /// The crate's guarantee, exercised on purpose: each guard in the draw loop is
    /// handed a private key KNOWN to trip it, then a clean one, and the handshake that comes back
    /// must be the clean draw's. A dropped guard fails its case outright — the old random sweep
    /// caught a dropped guard with "~97 %" probability, which is a 3 % escape by design.
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

    /// A salt is fixed for the life of an account rather than redrawn per handshake, so an ambiguous
    /// one is not a flaky login but an account that can never log in.
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
