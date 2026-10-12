//! Login packets are not length-framed, so each realmd reader must consume its packet exactly:
//! these read a back-to-back challenge, proof and realm-list stream from one cursor.

use benilla_protocol::auth;

const N: [u8; 32] = benilla_srp::LARGE_SAFE_PRIME_LITTLE_ENDIAN;

/// The `crc_salt` a mangos-family realmd sends: its fixed `VersionChallenge` bytes.
const CRC_SALT: [u8; 16] = [
    0xba, 0xa3, 0x1e, 0x99, 0xa0, 0x0b, 0x21, 0x57, 0xfc, 0x37, 0x3f, 0xb3, 0x69, 0xcd, 0xd2, 0xf1,
];

/// A successful `CMD_AUTH_LOGON_CHALLENGE_Server` body, with its `crc_salt` + `security_flag` tail.
fn challenge_packet(server_public_key: &[u8; 32], salt: &[u8; 32]) -> Vec<u8> {
    let mut p = vec![0x00, 0x00, 0x00]; // opcode, protocol_version, result=success
    p.extend_from_slice(server_public_key);
    p.push(1); // generator length
    p.push(7); // generator
    p.push(32); // large-safe-prime length
    p.extend_from_slice(&N);
    p.extend_from_slice(salt);
    p.extend_from_slice(&CRC_SALT);
    p.push(0); // security_flag = None
    p
}

fn proof_packet(server_proof: &[u8; 20]) -> Vec<u8> {
    let mut p = vec![0x01, 0x00]; // opcode, result=success
    p.extend_from_slice(server_proof);
    p.extend_from_slice(&0u32.to_le_bytes()); // hardware_survey_id
    p
}

fn realm_list_packet() -> Vec<u8> {
    let mut realms = Vec::new();
    realms.extend_from_slice(&0u32.to_le_bytes()); // realm_type
    realms.push(0); // flag
    realms.extend_from_slice(b"Benilla\0");
    realms.extend_from_slice(b"127.0.0.1:8085\0");
    realms.extend_from_slice(&0.0f32.to_le_bytes()); // population
    realms.push(3); // number_of_characters
    realms.push(0); // category
    realms.push(1); // realm_id

    let mut p = vec![0x10]; // opcode
    let size = (4 + 1 + realms.len() + 2) as u16; // header_padding + count + realms + footer
    p.extend_from_slice(&size.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes()); // header_padding
    p.push(1); // number_of_realms
    p.extend_from_slice(&realms);
    p.extend_from_slice(&0u16.to_le_bytes()); // footer_padding
    p
}

#[test]
fn logon_read_sequence_stays_aligned() {
    let server_public_key: [u8; 32] =
        std::array::from_fn(|i| (i as u8).wrapping_mul(7).wrapping_add(9));
    let salt: [u8; 32] = std::array::from_fn(|i| (i as u8).wrapping_mul(11).wrapping_add(2));
    let server_proof: [u8; 20] =
        std::array::from_fn(|i| (i as u8).wrapping_mul(13).wrapping_add(4));

    let mut buf = challenge_packet(&server_public_key, &salt);
    buf.extend_from_slice(&proof_packet(&server_proof));
    buf.extend_from_slice(&realm_list_packet());

    let mut stream: &[u8] = &buf;

    // The challenge read must consume the `crc_salt` + `security_flag` tail to land on opcode 0x01.
    let reply = auth::read_challenge_reply(&mut stream)
        .expect("challenge reply")
        .expect("a challenge with SRP inputs");
    assert_eq!(reply.server_public_key, server_public_key);
    assert_eq!(reply.generator, 7);
    assert_eq!(reply.large_safe_prime, N);
    assert_eq!(reply.salt, salt);
    assert_eq!(reply.crc_salt, CRC_SALT); // the proof's `crc_hash` is computed from this

    let proof =
        auth::read_proof_reply(&mut stream).expect("proof reply (stream desynced if this fails)");
    assert_eq!(proof, Some(server_proof));

    let realms =
        auth::read_realm_list(&mut stream).expect("realm list (stream desynced if this fails)");
    assert_eq!(realms.len(), 1);
    assert_eq!(realms[0].name, "Benilla");
    assert_eq!(realms[0].address, "127.0.0.1:8085");
    assert_eq!(realms[0].characters, 3);

    assert!(
        stream.is_empty(),
        "{} trailing bytes — a reader over/under-consumed",
        stream.len()
    );
}

// ── `logon` against a fake realmd ────────────────────────────────────────────────────────────────

/// A realmd that answers the challenge with `challenge`, then, unless it was the bare 3-byte
/// survey or a refusal, reads the client's proof and answers `proof`; after a success it serves
/// the one-realm list. Returns its address.
fn fake_realmd(challenge: Vec<u8>, proof: Option<Vec<u8>>, then_list: bool) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut head = [0u8; 4];
        s.read_exact(&mut head).unwrap();
        let mut body = vec![0u8; usize::from(u16::from_le_bytes([head[2], head[3]]))];
        s.read_exact(&mut body).unwrap();
        s.write_all(&challenge).unwrap();
        if let Some(proof) = proof {
            let mut client_proof = [0u8; 75];
            s.read_exact(&mut client_proof).unwrap();
            s.write_all(&proof).unwrap();
        }
        if then_list {
            s.read_exact(&mut [0u8; 5]).unwrap();
            s.write_all(&realm_list_packet()).unwrap();
        }
        let _ = s.read(&mut [0u8; 1]);
    });
    addr
}

/// A width-stable `B` (top byte non-zero), so the client does not redial.
const B: [u8; 32] = [9u8; 32];

/// `WOW_SUCCESS_SURVEY` (0x0E) in the proof reply logs in: `Logon::OnAuthResult` sends it down
/// 0x00's success arm (`0x5b2cd4`), with no `M2` read after it.
#[test]
fn a_survey_success_in_the_proof_reply_logs_in() {
    let addr = fake_realmd(
        challenge_packet(&B, &[3u8; 32]),
        Some(vec![0x01, 0x0e]),
        true,
    );
    let logon = benilla_protocol::logon(&addr, "probe", "secret").expect("0x0E is a success");
    assert_eq!(logon.realms.len(), 1);
}

/// 0x0E in the challenge reply logs in with no SRP exchange at all.
#[test]
fn a_survey_success_in_the_challenge_reply_logs_in() {
    let addr = fake_realmd(vec![0x00, 0x00, 0x0e], None, true);
    let logon = benilla_protocol::logon(&addr, "probe", "secret").expect("0x0E is a success");
    assert_eq!(logon.realms.len(), 1);
}

/// A refusal says which reply carried it, since the reference swallows a proof's 0x0A
/// (`0x5bada2`) where a challenge's reaches the patch state.
#[test]
fn a_refusal_names_its_reply() {
    let refused = |addr: String| {
        let e = benilla_protocol::logon(&addr, "probe", "secret")
            .err()
            .expect("refused");
        *e.downcast_ref::<benilla_protocol::AuthReject>()
            .expect("an AuthReject")
    };
    let r = refused(fake_realmd(vec![0x00, 0x00, 0x0a], None, false));
    assert_eq!(
        (r.code, r.reply),
        (0x0a, benilla_protocol::LogonReply::Challenge)
    );
    let r = refused(fake_realmd(
        challenge_packet(&B, &[3u8; 32]),
        Some(vec![0x01, 0x0a]),
        false,
    ));
    assert_eq!(
        (r.code, r.reply),
        (0x0a, benilla_protocol::LogonReply::Proof)
    );
}
