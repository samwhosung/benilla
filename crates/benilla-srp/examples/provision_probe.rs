//! Provision a probe account (`probeN`, password `pprobeN`, character `Probe<N spelled>`) in the
//! local vmangos `realmd` DB, one per checkout that runs probes. It touches no DB: it prints the
//! SQL and the character-create line to run.
//!
//! Check the hex convention against an existing row first, then emit the new slot's row:
//!   cargo run -p benilla-srp --example provision_probe -- check <USER> <PASS> <s-hex> <v-hex>
//!   cargo run -p benilla-srp --example provision_probe -- emit <N>
//!
//! The DB stores `v` and `s` as the big-endian hex of the number, as vmangos `BigNumber` writes it;
//! SRP hashes the salt as that number's little-endian bytes.

use benilla_srp::{generate_account, password_verifier, NormalizedString};

/// Stored big-endian hex → the 32 little-endian bytes SRP feeds to SHA1.
fn le_from_hex(hex: &str) -> [u8; 32] {
    let hex = hex.trim();
    assert!(hex.len() <= 64, "hex too long for a 32-byte value");
    let padded = format!("{hex:0>64}");
    let mut out = [0u8; 32];
    for (i, chunk) in (0..64).step_by(2).enumerate() {
        // big-endian byte i lands at little-endian index 31-i
        out[31 - i] = u8::from_str_radix(&padded[chunk..chunk + 2], 16).expect("hex digit");
    }
    out
}

/// Little-endian bytes → the big-endian hex string the DB stores.
fn hex_from_le(le: &[u8; 32]) -> String {
    le.iter().rev().map(|b| format!("{b:02X}")).collect()
}

fn spelled(n: u32) -> &'static str {
    [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ][n as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") if args.len() == 5 => {
            let user = NormalizedString::new(&args[1]).expect("username");
            let pass = NormalizedString::new(&args[2]).expect("password");
            let v = password_verifier(&user, &pass, &le_from_hex(&args[3]));
            let got = hex_from_le(&v);
            let want = format!("{:0>64}", args[4].trim().to_uppercase());
            if got == want {
                println!("MATCH — convention validated against the stored row");
            } else {
                println!("MISMATCH\n  computed v = {got}\n  stored   v = {want}");
                std::process::exit(1);
            }
        }
        Some("emit") if args.len() == 2 => {
            let n: u32 = args[1].parse().expect("slot number 0-9");
            assert!(n <= 9, "slot number 0-9");
            let user_s = format!("probe{n}");
            let pass_s = format!("pprobe{n}");
            let user = NormalizedString::new(&user_s).expect("username");
            let pass = NormalizedString::new(&pass_s).expect("password");
            let (salt, verifier) = generate_account(&user, &pass);
            println!(
                "INSERT INTO account (username, gmlevel, v, s) VALUES ('{}', 3, '{}', '{}');",
                user_s.to_uppercase(),
                hex_from_le(&verifier),
                hex_from_le(&salt),
            );
            println!("-- then mint the character over the real wire (0423's probe; KEEP it):");
            println!(
                "-- WOW_USER={user_s} WOW_PASS={pass_s} WOW_PROBE_CHARCREATE=Probe{} \\",
                spelled(n)
            );
            println!("--   WOW_PROBE_CHARCREATE_KEEP=1 WOW_DATA=<Data> cargo run -q -p benilla");
            println!("-- then GM-on survival for the new character (extra_flags bit 1):");
            println!(
                "-- UPDATE characters.characters SET extra_flags = extra_flags | 1 WHERE name = 'Probe{}';",
                spelled(n)
            );
        }
        _ => {
            eprintln!("usage: provision_probe check <USER> <PASS> <s-hex> <v-hex> | emit <N>");
            std::process::exit(2);
        }
    }
}
