//! Probe: whether realmd feeds the SRP6 big numbers to SHA-1 at fixed width or minimal length.
//! The 1.12 client hashes `A`, `B`, `K`, the salt, `H(N)⊕H(g)` and `M1` at their declared
//! widths, zero-padded in the high bytes (`0x5d3650`). vmangos hashes them as `BigNumber`s
//! (`AsByteArray()` with `minSize = 0`), dropping high-order zero bytes, so the two disagree
//! whenever a value has one; cmangos does the same (`Sha1Hash::UpdateBigNumbers`).
//!
//! ```text
//! cargo run --release -p benilla-protocol --example srp_encoding_probe -- --user U --pass P
//! ```
//!
//! `--stress N` runs N ordinary fixed-width handshakes and reports the failure rate, waiting out
//! realmd's lockout: 60 s after `WrongPass.MaxAttempts` (default 10) failures, during which it
//! answers `0x08 WOW_FAIL_DB_BUSY`.

use std::net::TcpStream;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use benilla_protocol::auth::{self, AuthReject};
use num_bigint::BigUint;
use rand::{thread_rng, RngCore};
use sha1::{Digest, Sha1};

const BUILD: u16 = 5875;
const AUTH_PORT: u16 = 3724;
/// Failures allowed inside realmd's throttle window before it answers `WOW_FAIL_DB_BUSY`.
const THROTTLE_MAX: u32 = 10;

/// How a big number is serialized into the SHA-1 stream.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Enc {
    /// The 1.12 client: the full declared width, zero-padded in the high bytes.
    Fixed,
    /// vmangos: `BigNumber::AsByteArray()`, high-order zero bytes dropped.
    Minimal,
}

impl Enc {
    /// `bytes` is little-endian, so the high-order end is the tail.
    fn apply(self, bytes: &[u8]) -> &[u8] {
        match self {
            Enc::Fixed => bytes,
            Enc::Minimal => {
                let mut end = bytes.len();
                while end > 0 && bytes[end - 1] == 0 {
                    end -= 1;
                }
                &bytes[..end]
            }
        }
    }
}

fn sha1(parts: &[&[u8]]) -> [u8; 20] {
    let mut h = Sha1::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn to_fixed_le_32(v: &BigUint) -> [u8; 32] {
    let raw = v.to_bytes_le();
    let mut out = [0u8; 32];
    let n = raw.len().min(32);
    out[..n].copy_from_slice(&raw[..n]);
    out
}

/// `SHA1_Interleave` as vmangos computes it, over all 32 bytes of `S` (`SRP6::HashSessionKey`);
/// the 1.12 client strips the low-order zero bytes of `S` first.
fn interleave(s: &[u8; 32]) -> [u8; 40] {
    let mut even = [0u8; 16];
    let mut odd = [0u8; 16];
    for i in 0..16 {
        even[i] = s[i * 2];
        odd[i] = s[i * 2 + 1];
    }
    let g = sha1(&[&even]);
    let h = sha1(&[&odd]);
    let mut out = [0u8; 40];
    for i in 0..20 {
        out[i * 2] = g[i];
        out[i * 2 + 1] = h[i];
    }
    out
}

/// The SRP6 values one handshake produced, plus the proof to send.
struct Attempt {
    a_pub: [u8; 32],
    b_pub: [u8; 32],
    k: [u8; 40],
    m1: [u8; 20],
}

impl Attempt {
    /// The hashed values with a high-order zero byte, where the two encodings differ.
    fn short_values(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.a_pub[31] == 0 {
            v.push("A");
        }
        if self.b_pub[31] == 0 {
            v.push("B");
        }
        if self.k[39] == 0 {
            v.push("K");
        }
        if self.m1[19] == 0 {
            v.push("M1");
        }
        v
    }

    /// `M2 = SHA1(A ‖ M1 ‖ K)`, under `enc`.
    fn expected_m2(&self, enc: Enc) -> [u8; 20] {
        sha1(&[
            enc.apply(&self.a_pub),
            enc.apply(&self.m1),
            enc.apply(&self.k),
        ])
    }
}

/// What realmd said and, on acceptance, which encoding its `M2` matches.
enum Outcome {
    Accepted { m2_fixed: bool, m2_minimal: bool },
    Refused(u8),
    Error(String),
}

impl Outcome {
    fn line(&self) -> String {
        match self {
            Outcome::Accepted {
                m2_fixed,
                m2_minimal,
            } => {
                let m2 = match (m2_fixed, m2_minimal) {
                    (true, true) => "M2 ok (both)",
                    (true, false) => "M2 ok (fixed)",
                    (false, true) => "M2 ok (minimal only — a fixed-width client rejects it)",
                    (false, false) => "M2 MATCHES NEITHER",
                };
                format!("ACCEPTED, {m2}")
            }
            Outcome::Refused(c) => format!("REFUSED {c:#04x}"),
            Outcome::Error(e) => format!("ERROR {e}"),
        }
    }
}

/// What the ephemeral search is aiming for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    /// Whatever the first draw gives: an ordinary handshake.
    Any,
    /// Nothing short: the case both encodings agree on.
    Clean,
    /// `A` with a zero high byte.
    ShortA,
    /// `K` with a zero high byte.
    ShortK,
    /// `M1` with a zero high byte: realmd accepts, but hashes a 19-byte `M1` into its `M2`.
    ShortM1,
    /// `B` with a zero high byte: the server's draw, so the caller redials; the rest draw clean.
    ShortB,
}

impl Want {
    /// Each case wants exactly its own value short, so each verdict isolates one divergence.
    fn accepts(self, a_short: bool, k_short: bool, m1_short: bool) -> bool {
        match self {
            Want::Any => true,
            Want::Clean | Want::ShortB => !a_short && !k_short && !m1_short,
            Want::ShortA => a_short && !k_short && !m1_short,
            Want::ShortK => !a_short && k_short && !m1_short,
            Want::ShortM1 => !a_short && !k_short && m1_short,
        }
    }
}

/// Run the client half of SRP6 for the given challenge, drawing ephemerals until `want` is met.
fn compute(
    reply: &auth::ChallengeReply,
    user: &str,
    pass: &str,
    enc: Enc,
    want: Want,
    tries: u32,
) -> Option<Attempt> {
    let n = BigUint::from_bytes_le(&reply.large_safe_prime);
    let g = BigUint::from_bytes_le(&[reply.generator]);
    let b_pub = reply.server_public_key;
    let b_bn = BigUint::from_bytes_le(&b_pub);

    // x = SHA1(salt ‖ SHA1(USER:PASS)), v = g^x mod N
    let inner = sha1(&[user.as_bytes(), b":", pass.as_bytes()]);
    let x = BigUint::from_bytes_le(&sha1(&[&reply.salt, &inner]));
    let v = g.modpow(&x, &n);
    let base = (&b_bn + 3u32 * (&n - &v)) % &n;

    let xor_hash = xor_hash(reply.generator, &reply.large_safe_prime);
    let user_hash = sha1(&[user.as_bytes()]);

    for _ in 0..tries {
        let mut priv_key = [0u8; 32];
        thread_rng().fill_bytes(&mut priv_key);
        let a = BigUint::from_bytes_le(&priv_key) + BigUint::from(n.bits());
        let a_pub = to_fixed_le_32(&g.modpow(&a, &n));

        // Reject a wrong `A` shape before paying for the second modpow.
        let a_short = a_pub[31] == 0;
        if want != Want::Any && a_short != (want == Want::ShortA) {
            continue;
        }

        let u = BigUint::from_bytes_le(&sha1(&[enc.apply(&a_pub), enc.apply(&b_pub)]));
        let s = to_fixed_le_32(&base.modpow(&(&a + &u * &x), &n));
        let k = interleave(&s);

        let m1 = sha1(&[
            enc.apply(&xor_hash),
            &user_hash,
            enc.apply(&reply.salt),
            enc.apply(&a_pub),
            enc.apply(&b_pub),
            enc.apply(&k),
        ]);

        if want.accepts(a_short, k[39] == 0, m1[19] == 0) {
            return Some(Attempt {
                a_pub,
                b_pub,
                k,
                m1,
            });
        }
    }
    None
}

/// `SHA1(N) XOR SHA1(g)`, little endian: a constant for the fixed `N` and `g`.
fn xor_hash(generator: u8, large_safe_prime: &[u8; 32]) -> [u8; 20] {
    let hn = sha1(&[large_safe_prime]);
    let hg = sha1(&[&[generator]]);
    let mut out = [0u8; 20];
    for i in 0..20 {
        out[i] = hn[i] ^ hg[i];
    }
    out
}

fn handshake(
    host: &str,
    user: &str,
    pass: &str,
    enc: Enc,
    want: Want,
    tries: u32,
) -> Result<(Attempt, Outcome)> {
    let mut s = TcpStream::connect((host, AUTH_PORT))
        .with_context(|| format!("connecting to {host}:{AUTH_PORT}"))?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    auth::write_logon_challenge(&mut s, user, BUILD).context("sending logon challenge")?;
    let reply = auth::read_challenge_reply(&mut s).context("reading logon challenge reply")?;

    // A wrong-shape `B` drops the connection before any proof, so realmd records no failure.
    if want != Want::Any && (reply.server_public_key[31] == 0) != (want == Want::ShortB) {
        return Err(anyhow!("this connection's B is not the wanted shape"));
    }

    let attempt = compute(&reply, user, pass, enc, want, tries)
        .ok_or_else(|| anyhow!("ephemeral search did not reach the wanted shape"))?;

    auth::write_logon_proof(&mut s, &attempt.a_pub, &attempt.m1, &reply.crc_salt)
        .context("sending logon proof")?;
    let outcome = match auth::read_proof_reply(&mut s) {
        Ok(m2) => Outcome::Accepted {
            m2_fixed: m2 == attempt.expected_m2(Enc::Fixed),
            m2_minimal: m2 == attempt.expected_m2(Enc::Minimal),
        },
        Err(e) => match e.downcast_ref::<AuthReject>() {
            Some(r) => Outcome::Refused(r.code),
            None => Outcome::Error(format!("{e:#}")),
        },
    };
    Ok((attempt, outcome))
}

/// Redial until the wanted shape turns up; `ShortB` needs many, `B` being the server's draw.
fn until(
    host: &str,
    user: &str,
    pass: &str,
    enc: Enc,
    want: Want,
    dials: u32,
) -> Result<(Attempt, Outcome)> {
    let mut last = None;
    for _ in 0..dials {
        match handshake(host, user, pass, enc, want, 8192) {
            Ok(r) => return Ok(r),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("no dial succeeded")))
}

fn report(label: &str, r: &Result<(Attempt, Outcome)>) {
    match r {
        Ok((a, v)) => {
            let short = a.short_values();
            let short = if short.is_empty() {
                "none".to_string()
            } else {
                short.join(",")
            };
            println!("  {label:<44} high-zero: {short:<5} → {}", v.line());
        }
        Err(e) => println!("  {label:<44} → probe error: {e:#}"),
    }
}

fn matrix(host: &str, user: &str, pass: &str, enc: Enc, title: &str) {
    println!("\n{title}");
    for (label, want, dials) in [
        ("control: no value has a high zero byte", Want::Clean, 12),
        ("A has a high zero byte", Want::ShortA, 12),
        ("K has a high zero byte", Want::ShortK, 12),
        ("M1 has a high zero byte", Want::ShortM1, 12),
        (
            "B has a high zero byte (the server's draw)",
            Want::ShortB,
            900,
        ),
    ] {
        report(label, &until(host, user, pass, enc, want, dials));
    }
}

fn main() -> Result<()> {
    let mut host = "localhost".to_string();
    let mut user = String::new();
    let mut pass = String::new();
    let mut stress = 0u32;
    let mut logons = 0u32;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--host" => host = args.next().unwrap_or(host),
            "--user" => user = args.next().unwrap_or(user),
            "--pass" => pass = args.next().unwrap_or(pass),
            "--stress" => stress = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--logon" => logons = args.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            other => return Err(anyhow!("unknown argument {other}")),
        }
    }
    if user.is_empty() || pass.is_empty() {
        return Err(anyhow!("--user and --pass are required"));
    }

    // `--logon N` runs the shipped `logon` N times; every handshake must succeed, or its
    // ephemeral guarantee is not holding.
    if logons > 0 {
        println!("logon: {logons} full handshakes through benilla_protocol::logon\n");
        let (mut ok, mut failed) = (0u32, 0u32);
        for i in 0..logons {
            match benilla_protocol::logon(&host, &user, &pass) {
                Ok(_) => ok += 1,
                Err(e) => {
                    failed += 1;
                    println!("  #{i}: {e:#}");
                    // Stay clear of realmd's lockout.
                    std::thread::sleep(Duration::from_secs(7));
                }
            }
        }
        println!("\nsucceeded {ok}, failed {failed}");
        return Ok(());
    }

    let user = user.to_uppercase();
    let pass = pass.to_uppercase();

    if stress > 0 {
        println!("stress: {stress} handshakes, client-faithful (fixed-width) encoding\n");
        let (mut ok, mut failed, mut throttled) = (0u32, 0u32, 0u32);
        let mut blame: Vec<String> = Vec::new();
        let mut window_failures = 0u32;
        for i in 0..stress {
            if window_failures >= THROTTLE_MAX - 1 {
                std::thread::sleep(Duration::from_secs(61)); // drain realmd's lockout window
                window_failures = 0;
            }
            // `Want::Any` with one try: an ordinary handshake, whatever the shapes of `A` and `B`.
            match handshake(&host, &user, &pass, Enc::Fixed, Want::Any, 1) {
                Ok((a, v)) => {
                    let short = a.short_values();
                    let short = if short.is_empty() {
                        "nothing short!".to_string()
                    } else {
                        format!("high-zero in {}", short.join(","))
                    };
                    match v {
                        Outcome::Refused(0x08) => {
                            throttled += 1;
                            std::thread::sleep(Duration::from_secs(61));
                            window_failures = 0;
                        }
                        Outcome::Accepted { m2_fixed: true, .. } => {
                            ok += 1;
                            window_failures = 0;
                        }
                        other => {
                            failed += 1;
                            window_failures += 1;
                            blame.push(format!("#{i}: {} — {short}", other.line()));
                        }
                    }
                }
                Err(e) => println!("  #{i}: probe error: {e:#}"),
            }
        }
        println!("\naccepted {ok}, failed {failed}, throttled {throttled}");
        if !blame.is_empty() {
            println!("failures:");
            for b in &blame {
                println!("  {b}");
            }
        }
        return Ok(());
    }

    println!("realmd at {host}:{AUTH_PORT}, account {user}");
    // `H(N)⊕H(g)` is hashed as a BigNumber too; it is constant, so a zero high byte would fail
    // every fixed-width login.
    let xh = xor_hash(7, &benilla_srp::LARGE_SAFE_PRIME_LITTLE_ENDIAN);
    println!(
        "H(N)^H(g) high byte = {:#04x} (non-zero, so the constant is unambiguous)",
        xh[19]
    );
    matrix(
        &host,
        &user,
        &pass,
        Enc::Fixed,
        "client-faithful encoding (fixed width — what benilla sends today):",
    );
    matrix(
        &host,
        &user,
        &pass,
        Enc::Minimal,
        "vmangos encoding (AsByteArray() — high-order zero bytes dropped):",
    );
    Ok(())
}
