//! The login cycle, [`super::run`], against a fake realmd and a fake world server on loopback: the
//! real logon, realm park, world handshake and roster, driven through the parks as the app does.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use benilla_protocol::messages::{self, opcode};
use benilla_protocol::{SessionEvent, WorldWriter};
use benilla_srp::vanilla_header::HeaderCrypto;
use benilla_srp::{
    password_verifier, NormalizedString, GENERATOR, LARGE_SAFE_PRIME_LITTLE_ENDIAN,
    SESSION_KEY_LENGTH,
};
use crossbeam_channel::{Receiver, Sender};
use num_bigint::BigUint;
use sha1::{Digest, Sha1};

use super::{run, Cycle, LoginRequest, Parks, PingClock};
use crate::net::{CharRequest, RealmRequest};

const USER: &str = "empty";
const PASS: &str = "secret";
const REALM: &str = "Fake";
const SALT: [u8; 32] = [0x5a; 32];
/// Every wait in the test: long enough for a loaded machine, short of a hung gate.
const WAIT: Duration = Duration::from_secs(20);

fn sha1(parts: &[&[u8]]) -> [u8; 20] {
    let mut h = Sha1::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

fn le32(v: &BigUint) -> [u8; 32] {
    let raw = v.to_bytes_le();
    let mut out = [0u8; 32];
    out[..raw.len()].copy_from_slice(&raw);
    out
}

/// `SHA1_Interleave` over all 32 bytes of `S`, as vmangos's `SRP6::HashSessionKey` does.
fn interleave(s: &[u8; 32]) -> [u8; SESSION_KEY_LENGTH] {
    let even: Vec<u8> = s.iter().step_by(2).copied().collect();
    let odd: Vec<u8> = s.iter().skip(1).step_by(2).copied().collect();
    let (g, h) = (sha1(&[&even]), sha1(&[&odd]));
    let mut out = [0u8; SESSION_KEY_LENGTH];
    for (i, (gi, hi)) in g.iter().zip(&h).enumerate() {
        out[i * 2] = *gi;
        out[i * 2 + 1] = *hi;
    }
    out
}

/// A realmd that logs [`USER`] in with the server half of SRP6 and lists one realm, [`REALM`] at
/// `world`. The session key goes to the fake world server, which needs it for the header cipher.
fn fake_realmd(world: String, keys: Sender<[u8; SESSION_KEY_LENGTH]>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let n = BigUint::from_bytes_le(&LARGE_SAFE_PRIME_LITTLE_ENDIAN);
        let g = BigUint::from(GENERATOR);
        let user = NormalizedString::new(USER).unwrap();
        let pass = NormalizedString::new(PASS).unwrap();
        let v = BigUint::from_bytes_le(&password_verifier(&user, &pass, &SALT));

        // CMD_AUTH_LOGON_CHALLENGE_Client: opcode, protocol, u16 size, then the body.
        let mut head = [0u8; 4];
        s.read_exact(&mut head).unwrap();
        let mut body = vec![0u8; usize::from(u16::from_le_bytes([head[2], head[3]]))];
        s.read_exact(&mut body).unwrap();

        // B = k·v + g^b, with k = 3; `b` walks until `B`'s top byte is non-zero, since the client
        // redials on a zero one.
        let (b, b_pub) = (1u32..)
            .map(|i| {
                let b = BigUint::from_bytes_le(&[0x3c; 32]) + i;
                let b_pub = le32(&((BigUint::from(3u8) * &v + g.modpow(&b, &n)) % &n));
                (b, b_pub)
            })
            .find(|(_, b_pub)| b_pub[31] != 0)
            .unwrap();
        let mut reply = vec![0x00, 0x00, 0x00];
        reply.extend_from_slice(&b_pub);
        reply.extend_from_slice(&[1, GENERATOR, 32]);
        reply.extend_from_slice(&LARGE_SAFE_PRIME_LITTLE_ENDIAN);
        reply.extend_from_slice(&SALT);
        reply.extend_from_slice(&[0u8; 16]); // crc_salt
        reply.push(0); // security_flag
        s.write_all(&reply).unwrap();

        // CMD_AUTH_LOGON_PROOF_Client: opcode, A, M1, crc_hash, key count, security flag.
        let mut proof = [0u8; 75];
        s.read_exact(&mut proof).unwrap();
        let (a_pub, m1) = (&proof[1..33], &proof[33..53]);
        let u = BigUint::from_bytes_le(&sha1(&[a_pub, &b_pub]));
        let a = BigUint::from_bytes_le(a_pub);
        let big_s = (a * v.modpow(&u, &n) % &n).modpow(&b, &n);
        let key = interleave(&le32(&big_s));
        let mut reply = vec![0x01, 0x00];
        reply.extend_from_slice(&sha1(&[a_pub, m1, &key]));
        reply.extend_from_slice(&0u32.to_le_bytes()); // hardware_survey_id
        s.write_all(&reply).unwrap();
        keys.send(key).unwrap();

        // CMD_REALM_LIST_Client, answered with the one realm.
        s.read_exact(&mut [0u8; 5]).unwrap();
        let mut realm = Vec::new();
        realm.extend_from_slice(&0u32.to_le_bytes()); // realm_type
        realm.push(0); // flags
        realm.extend_from_slice(format!("{REALM}\0{world}\0").as_bytes());
        realm.extend_from_slice(&0f32.to_le_bytes()); // population
        realm.extend_from_slice(&[0, 1, 1]); // characters, category, id
        let mut list = vec![0x10];
        list.extend_from_slice(&((4 + 1 + realm.len() + 2) as u16).to_le_bytes());
        list.extend_from_slice(&0u32.to_le_bytes());
        list.push(1);
        list.extend_from_slice(&realm);
        list.extend_from_slice(&0u16.to_le_bytes());
        s.write_all(&list).unwrap();

        // Held until the cycle drops the logon, as realmd keeps it for the list refresh.
        let _ = s.read(&mut [0u8; 1]);
    });
    addr
}

/// A server packet: `u16` BE size counting the opcode, `u16` LE opcode, the body.
fn send(s: &mut TcpStream, crypto: Option<&mut HeaderCrypto>, op: u16, body: &[u8]) {
    let size = ((body.len() + 2) as u16).to_be_bytes();
    let op = op.to_le_bytes();
    let mut header = [size[0], size[1], op[0], op[1]];
    if let Some(c) = crypto {
        c.encrypter().encrypt(&mut header);
    }
    s.write_all(&header).unwrap();
    s.write_all(body).unwrap();
}

/// A client packet's opcode, reading past its body: `u16` BE size counting the 4-byte opcode.
fn recv(s: &mut TcpStream, crypto: Option<&mut HeaderCrypto>) -> Option<u16> {
    let mut header = [0u8; 6];
    s.read_exact(&mut header).ok()?;
    if let Some(c) = crypto {
        c.decrypter().decrypt(&mut header);
    }
    let size = usize::from(u16::from_be_bytes([header[0], header[1]]));
    s.read_exact(&mut vec![0u8; size.saturating_sub(4)]).ok()?;
    Some(u16::from_le_bytes([header[2], header[3]]))
}

/// A world server whose account has no characters: it admits the session under realmd's key,
/// answers every `CMSG_CHAR_ENUM` with an empty roster and a `CMSG_CHAR_CREATE` with success, and
/// logs every opcode the client sends.
fn fake_world(
    listener: TcpListener,
    keys: Receiver<[u8; SESSION_KEY_LENGTH]>,
) -> Arc<Mutex<Vec<u16>>> {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&sent);
    thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        send(
            &mut s,
            None,
            opcode::SMSG_AUTH_CHALLENGE,
            &0xDEAD_BEEFu32.to_le_bytes(),
        );
        assert_eq!(recv(&mut s, None), Some(opcode::CMSG_AUTH_SESSION));
        let mut crypto = HeaderCrypto::from_session_key(keys.recv_timeout(WAIT).unwrap());
        send(
            &mut s,
            Some(&mut crypto),
            opcode::SMSG_AUTH_RESPONSE,
            &[messages::AUTH_OK],
        );
        while let Some(op) = recv(&mut s, Some(&mut crypto)) {
            log.lock().unwrap().push(op);
            match op {
                opcode::CMSG_CHAR_ENUM => {
                    send(&mut s, Some(&mut crypto), opcode::SMSG_CHAR_ENUM, &[0]);
                }
                opcode::CMSG_CHAR_CREATE => send(
                    &mut s,
                    Some(&mut crypto),
                    opcode::SMSG_CHAR_CREATE,
                    &[messages::CHAR_CREATE_SUCCESS],
                ),
                _ => {}
            }
        }
    });
    sent
}

/// The next event, skipping the stage announcements; a login failure fails the test by name.
fn next_event(events: &Receiver<SessionEvent>) -> SessionEvent {
    loop {
        match events.recv_timeout(WAIT).expect("the cycle went quiet") {
            SessionEvent::LoginStage { .. } => continue,
            SessionEvent::LoginFailed { reason, .. } => panic!("the login failed: {reason}"),
            ev => return ev,
        }
    }
}

/// The reference builds `CMSG_CHAR_CREATE` in one place (`0x5aac50`), reached only from the glue's
/// `CreateCharacter`, which the stock glue calls only from the create screen's Okay; so a login to
/// an account with no characters parks at select on the empty list and creates nothing.
#[test]
fn an_empty_account_parks_at_select_without_creating_a_character() {
    let world_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let world = world_listener.local_addr().unwrap().to_string();
    let (keys_tx, keys_rx) = crossbeam_channel::unbounded();
    let sent = fake_world(world_listener, keys_rx);
    let realmd = fake_realmd(world, keys_tx);

    let (events_tx, events) = crossbeam_channel::unbounded();
    let (writer_tx, _writer_rx) = crossbeam_channel::unbounded::<WorldWriter>();
    let (login_tx, login_rx) = crossbeam_channel::unbounded();
    let (realm_tx, realm_rx) = crossbeam_channel::unbounded();
    let (pick_tx, pick_rx) = crossbeam_channel::unbounded();
    let cycle = thread::spawn(move || {
        let parks = Parks {
            login_rx,
            realm_rx,
            pick_rx,
        };
        let ended = run(
            &events_tx,
            &writer_tx,
            &parks,
            &AtomicU64::new(0),
            &Mutex::new(PingClock::default()),
            &mut Default::default(),
        );
        matches!(ended, Ok(Cycle::Repark))
    });

    login_tx
        .send(LoginRequest {
            user: USER.into(),
            pass: PASS.into(),
            host: realmd,
            generation: 0,
        })
        .unwrap();
    match next_event(&events) {
        SessionEvent::RealmList { realms } => assert_eq!(realms[0].name, REALM),
        other => panic!("expected the realm list, got {other:?}"),
    }
    realm_tx.send(RealmRequest::Enter(REALM.into())).unwrap();
    match next_event(&events) {
        SessionEvent::CharacterList { characters, .. } => assert!(
            characters.is_empty(),
            "the roster at select must be the account's, empty: {characters:?}"
        ),
        other => panic!("expected the roster at select, got {other:?}"),
    }
    // Select's Back ends the cycle, so the socket is done and its log complete.
    pick_tx.send(CharRequest::Abandon).unwrap();
    assert!(cycle.join().unwrap(), "Back returns to the login park");
    let sent = sent.lock().unwrap().clone();
    assert!(
        !sent.contains(&opcode::CMSG_CHAR_CREATE),
        "the login sent CMSG_CHAR_CREATE on its own: {sent:#06x?}"
    );
}
