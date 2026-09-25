//! `version_check_probe`: whether a realmd enforces `StrictVersionCheck`, and whether our logon
//! proof satisfies it. It runs the handshake twice, differing only in the proof's `crc_hash`:
//! twenty zeros, which a strict realmd refuses with `WOW_FAIL_VERSION_INVALID` (0x09, shown as
//! `AUTH_VERSION_MISMATCH`, "Wrong client version"), and the computed `SHA1(A ‖ H)`
//! ([`auth::version_proof`]). A non-strict server accepts both.
//!
//! With an install directory it also derives that install's own digest the way the 1.12 client
//! does, HMAC-SHA1 keyed by the challenge over five binaries, and says whether it is the stock
//! one. Deviation: benilla always sends the published constant instead, since an install without
//! those binaries could never answer for itself.
//!
//! Usage: `cargo run -p benilla-protocol --example version_check_probe -- <host[:port]> <user>
//! <pass> [install-dir]`. The local strict-mode realmd is `127.0.0.1:3725`, the ordinary one 3724.

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use benilla_protocol::{auth, host_port, AuthReject, AUTH_PORT, CLIENT_BUILD};
use benilla_srp::{NormalizedString, PublicKey, SrpClientChallenge};
use sha1::{Digest, Sha1};

/// Which `crc_hash` the arm puts in the proof packet.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arm {
    /// Twenty zeros, forced by handing the writer a `crc_salt` we hold no digest for.
    Zeros,
    /// `SHA1(A ‖ H)` for the salt the server sent.
    Computed,
}

impl Arm {
    fn label(self) -> &'static str {
        match self {
            Arm::Zeros => "zeros   ",
            Arm::Computed => "computed",
        }
    }
}

/// One full handshake with `arm`'s `crc_hash`: `None` when accepted, else the `WOW_FAIL_*` byte.
fn attempt(host: &str, port: u16, user: &str, pass: &str, arm: Arm) -> Result<Option<u8>> {
    let user_n = NormalizedString::new(user).map_err(|e| anyhow!("invalid username: {e}"))?;
    let pass_n = NormalizedString::new(pass).map_err(|e| anyhow!("invalid password: {e}"))?;

    // Redial past a width-unstable `B`, as `logon` does, so a 0x04 here is a bad password and
    // never the SHA-1 encoding disagreement.
    for _ in 0..8 {
        let mut stream = TcpStream::connect((host, port))
            .with_context(|| format!("connecting to {host}:{port}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        auth::write_logon_challenge(&mut stream, &user.to_uppercase(), CLIENT_BUILD)
            .context("sending logon challenge")?;
        let reply = auth::read_challenge_reply(&mut stream).context("reading challenge reply")?;
        let b = PublicKey::from_le_bytes(reply.server_public_key)
            .map_err(|e| anyhow!("invalid server public key: {e}"))?;
        if !b.is_width_stable() {
            continue;
        }

        let challenge = SrpClientChallenge::new(
            user_n,
            pass_n,
            reply.generator,
            reply.large_safe_prime,
            b,
            reply.salt,
        );
        let salt = match arm {
            Arm::Computed => reply.crc_salt,
            Arm::Zeros => [0u8; 16],
        };
        auth::write_logon_proof(
            &mut stream,
            challenge.client_public_key(),
            challenge.client_proof(),
            &salt,
        )
        .context("sending logon proof")?;

        return match auth::read_proof_reply(&mut stream) {
            Ok(_) => Ok(None),
            Err(e) => match e.downcast_ref::<AuthReject>() {
                Some(r) => Ok(Some(r.code)),
                None => Err(e),
            },
        };
    }
    Err(anyhow!("eight dials, every `B` ambiguous — try again"))
}

/// The five files the 1.12 client hashes, in this order (`ChecksumExecutables` at `0x5b1170`, the
/// table at `0x85def0`); no other of the 120 orders gives the published digest.
const SCANNED: [&str; 5] = [
    "WoW.exe",
    "fmod.dll",
    "ijl15.dll",
    "dbghelp.dll",
    "unicows.dll",
];

/// HMAC-SHA1 keyed by the 16-byte `crc_salt`, framed as the client does (`0x5b117e`–`0x5b11b6`);
/// the key is always 16 bytes, so there is no long-key step, as in the client.
fn hmac_sha1(key: &[u8; 16], msg: &[u8]) -> [u8; 20] {
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for (i, k) in key.iter().enumerate() {
        ipad[i] ^= k;
        opad[i] ^= k;
    }
    let mut inner = Sha1::new();
    inner.update(ipad);
    inner.update(msg);
    let mut outer = Sha1::new();
    outer.update(opad);
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Derive `H` from an install the way the 1.12 client does. An unreadable file contributes no
/// bytes, as in the client (`0x5b11eb`, `0x5b1212`, `0x5b121f`, `0x5b1246`).
fn report_install(install_dir: &Path, crc_salt: &[u8; 16]) {
    println!("\n  install {}", install_dir.display());
    let mut msg = Vec::new();
    let mut missing = Vec::new();
    for name in SCANNED {
        match std::fs::read(install_dir.join(name)) {
            Ok(bytes) => {
                println!("    {name:<12} {} bytes", bytes.len());
                msg.extend_from_slice(&bytes);
            }
            Err(e) => {
                println!("    {name:<12} UNREADABLE ({e}) — skipped, as the client skips it");
                missing.push(name);
            }
        }
    }
    let h = hmac_sha1(crc_salt, &msg);
    let hex: String = h.iter().map(|b| format!("{b:02X}")).collect();
    println!("    H = {hex}");
    // Compare through `SHA1(A ‖ H)`, what we send: equal proofs for one `A` mean equal `H`.
    let a = [0u8; 32];
    let matches = {
        let mut sha = Sha1::new();
        sha.update(a);
        sha.update(h);
        let derived: [u8; 20] = sha.finalize().into();
        derived == auth::version_proof(crc_salt, &a)
    };
    println!(
        "    → {}",
        if matches {
            "matches the digest we send: this install is the stock one for the OS we declare"
        } else if !missing.is_empty() {
            "differs, and files are missing — an install like this could never answer the \
             challenge for itself, which is why we send a constant instead"
        } else {
            "differs from the digest we send. On macOS that is expected and harmless: we declare \
             OSX, so we send the Mac client's digest, while your install is the Windows one. \
             Otherwise one of the five is patched. Login is unaffected either way — the digest we \
             send does not come from here"
        }
    );
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (host_arg, user, pass, install) = match args.as_slice() {
        [h, u, p] => (h.clone(), u.clone(), p.clone(), None),
        [h, u, p, i] => (h.clone(), u.clone(), p.clone(), Some(PathBuf::from(i))),
        _ => {
            eprintln!("usage: version_check_probe <host[:port]> <user> <pass> [install-dir]");
            std::process::exit(2);
        }
    };
    let (host, port) = host_port(&host_arg, AUTH_PORT);

    // Our digest holds only for the one salt every mangos-family realmd sends; under any other,
    // the computed arm sends zeros too.
    let mut stream =
        TcpStream::connect((host, port)).with_context(|| format!("connecting to {host}:{port}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    auth::write_logon_challenge(&mut stream, &user.to_uppercase(), CLIENT_BUILD)?;
    let crc_salt = auth::read_challenge_reply(&mut stream)?.crc_salt;
    drop(stream); // no proof follows, so realmd records nothing for this dial
    let hex: String = crc_salt.iter().map(|b| format!("{b:02x}")).collect();
    let known = auth::version_proof(&crc_salt, &[0u8; 32]) != [0u8; 20];
    println!("{host}:{port} — build {CLIENT_BUILD}, crc_salt {hex}");
    println!(
        "  we {} an integrity digest for that challenge\n",
        if known { "HOLD" } else { "hold NO" }
    );

    for arm in [Arm::Zeros, Arm::Computed] {
        let verdict = match attempt(host, port, &user, &pass, arm)? {
            None => "ACCEPTED".to_string(),
            Some(0x09) => {
                "REJECTED 0x09 WOW_FAIL_VERSION_INVALID (\"Wrong client version\")".into()
            }
            Some(c) => format!("REJECTED {c:#04x}"),
        };
        println!("  crc_hash {} → {verdict}", arm.label());
    }
    if let Some(dir) = install {
        report_install(&dir, &crc_salt);
    }
    Ok(())
}
