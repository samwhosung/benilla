//! A fake realmd and a fake world server on loopback, for the login cycle's tests: the real SRP6
//! logon, the realm list as often as it is asked for, the world handshake and the roster. Both take
//! any number of connections, so a test can log in again.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use benilla_protocol::messages::{self, opcode};
use benilla_protocol::{SessionEnd, SessionEvent, WorldWriter};
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

pub(crate) const USER: &str = "empty";
pub(crate) const PASS: &str = "secret";
const SALT: [u8; 32] = [0x5a; 32];
/// Every wait in the tests: long enough for a loaded machine, short of a hung gate.
pub(crate) const WAIT: Duration = Duration::from_secs(20);

/// A session key, realmd to the world server.
type Key = [u8; SESSION_KEY_LENGTH];

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
fn interleave(s: &[u8; 32]) -> Key {
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

/// One realm the fake realmd lists.
#[derive(Clone)]
pub(crate) struct FakeRealm {
    pub(crate) name: String,
    /// `host:port` the realm's world server answers on.
    pub(crate) address: String,
    pub(crate) realm_type: u32,
    pub(crate) flags: u8,
    pub(crate) characters: u8,
    pub(crate) category: u8,
}

impl FakeRealm {
    /// A Normal realm in category 1 at `address`.
    pub(crate) fn new(name: &str, address: &str) -> Self {
        FakeRealm {
            name: name.into(),
            address: address.into(),
            realm_type: 0,
            flags: 0,
            characters: 0,
            category: 1,
        }
    }
}

/// A realmd that logs [`USER`] in with the server half of SRP6 and answers every
/// `CMD_REALM_LIST` with its realms, on every connection, until the client closes it.
pub(crate) struct Realmd {
    pub(crate) addr: String,
    logons: Arc<AtomicUsize>,
    lists: Arc<AtomicUsize>,
    open: Arc<AtomicUsize>,
    links: Arc<Mutex<Vec<TcpStream>>>,
}

impl Realmd {
    /// Listen on loopback; each session key goes to `keys`, for the world server's header cipher.
    pub(crate) fn start(realms: Vec<FakeRealm>, keys: Sender<Key>) -> Realmd {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let logons = Arc::new(AtomicUsize::new(0));
        let lists = Arc::new(AtomicUsize::new(0));
        let open = Arc::new(AtomicUsize::new(0));
        let links = Arc::new(Mutex::new(Vec::new()));
        let (l, n, o) = (logons.clone(), lists.clone(), open.clone());
        let held = links.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(s) = stream else { return };
                if let Ok(c) = s.try_clone() {
                    held.lock().unwrap().push(c);
                }
                let (realms, keys) = (realms.clone(), keys.clone());
                let (l, n, o) = (l.clone(), n.clone(), o.clone());
                thread::spawn(move || {
                    o.fetch_add(1, Ordering::SeqCst);
                    serve_realmd(s, &realms, &keys, &l, &n);
                    o.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Realmd {
            addr,
            logons,
            lists,
            open,
            links,
        }
    }

    /// Close every link from the server's side, as vmangos does at `MaxSessionDuration`.
    pub(crate) fn drop_links(&self) {
        for s in self.links.lock().unwrap().drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Logons completed so far.
    pub(crate) fn logons(&self) -> usize {
        self.logons.load(Ordering::SeqCst)
    }

    /// Realm lists sent so far.
    pub(crate) fn lists(&self) -> usize {
        self.lists.load(Ordering::SeqCst)
    }

    /// Connections the client still holds.
    pub(crate) fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

/// One realmd connection: the logon, then the realm list for every request until EOF.
fn serve_realmd(
    mut s: TcpStream,
    realms: &[FakeRealm],
    keys: &Sender<Key>,
    logons: &AtomicUsize,
    lists: &AtomicUsize,
) {
    let n = BigUint::from_bytes_le(&LARGE_SAFE_PRIME_LITTLE_ENDIAN);
    let g = BigUint::from(GENERATOR);
    let user = NormalizedString::new(USER).unwrap();
    let pass = NormalizedString::new(PASS).unwrap();
    let v = BigUint::from_bytes_le(&password_verifier(&user, &pass, &SALT));

    // CMD_AUTH_LOGON_CHALLENGE_Client: opcode, protocol, u16 size, then the body.
    let mut head = [0u8; 4];
    if s.read_exact(&mut head).is_err() {
        return;
    }
    let mut body = vec![0u8; usize::from(u16::from_le_bytes([head[2], head[3]]))];
    if s.read_exact(&mut body).is_err() {
        return;
    }

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
    if s.write_all(&reply).is_err() {
        return;
    }

    // CMD_AUTH_LOGON_PROOF_Client: opcode, A, M1, crc_hash, key count, security flag.
    let mut proof = [0u8; 75];
    if s.read_exact(&mut proof).is_err() {
        return;
    }
    let (a_pub, m1) = (&proof[1..33], &proof[33..53]);
    let u = BigUint::from_bytes_le(&sha1(&[a_pub, &b_pub]));
    let a = BigUint::from_bytes_le(a_pub);
    let big_s = (a * v.modpow(&u, &n) % &n).modpow(&b, &n);
    let key = interleave(&le32(&big_s));
    let mut reply = vec![0x01, 0x00];
    reply.extend_from_slice(&sha1(&[a_pub, m1, &key]));
    reply.extend_from_slice(&0u32.to_le_bytes()); // hardware_survey_id
    if s.write_all(&reply).is_err() {
        return;
    }
    let _ = keys.send(key);
    logons.fetch_add(1, Ordering::SeqCst);

    // CMD_REALM_LIST_Client (0x10 and a u32), answered every time, until the client closes the
    // link.
    let mut request = [0u8; 5];
    while s.read_exact(&mut request).is_ok() {
        assert_eq!(request[0], 0x10, "a realm list request");
        let mut records = Vec::new();
        for r in realms {
            records.extend_from_slice(&r.realm_type.to_le_bytes());
            records.push(r.flags);
            records.extend_from_slice(format!("{}\0{}\0", r.name, r.address).as_bytes());
            records.extend_from_slice(&1f32.to_le_bytes()); // population
            records.extend_from_slice(&[r.characters, r.category, 1]); // characters, category, id
        }
        let mut list = vec![0x10];
        list.extend_from_slice(&((4 + 1 + records.len() + 2) as u16).to_le_bytes());
        list.extend_from_slice(&0u32.to_le_bytes());
        list.push(realms.len() as u8);
        list.extend_from_slice(&records);
        // vmangos's trailer (`AuthSocket.cpp:1088`).
        list.extend_from_slice(&0x0002u16.to_le_bytes());
        if s.write_all(&list).is_err() {
            return;
        }
        lists.fetch_add(1, Ordering::SeqCst);
    }
}

/// A server packet: `u16` BE size counting the opcode, `u16` LE opcode, the body.
fn send(s: &mut TcpStream, crypto: Option<&mut HeaderCrypto>, op: u16, body: &[u8]) {
    let size = ((body.len() + 2) as u16).to_be_bytes();
    let op = op.to_le_bytes();
    let mut header = [size[0], size[1], op[0], op[1]];
    if let Some(c) = crypto {
        c.encrypter().encrypt(&mut header);
    }
    let _ = s.write_all(&header);
    let _ = s.write_all(body);
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

/// The fake world server's record: every opcode the client sent, and the sessions still open.
pub(crate) struct World {
    pub(crate) sent: Arc<Mutex<Vec<u16>>>,
    open: Arc<AtomicUsize>,
}

impl World {
    /// Sessions the client still holds.
    pub(crate) fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

/// A world server whose account has no characters: it admits each session under the latest key
/// realmd made, answers every `CMSG_CHAR_ENUM` with an empty roster and a `CMSG_CHAR_CREATE` with
/// success, and logs every opcode the client sends.
pub(crate) fn world(listener: TcpListener, keys: Receiver<Key>) -> World {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&sent);
    let open = Arc::new(AtomicUsize::new(0));
    let sessions = Arc::clone(&open);
    thread::spawn(move || {
        let mut last: Option<Key> = None;
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { return };
            send(
                &mut s,
                None,
                opcode::SMSG_AUTH_CHALLENGE,
                &0xDEAD_BEEFu32.to_le_bytes(),
            );
            if recv(&mut s, None) != Some(opcode::CMSG_AUTH_SESSION) {
                continue;
            }
            // The account's latest session key, as realmd stores it for the world server: a newer
            // logon's replaces it, and a second dial on the same logon reuses it.
            while let Ok(k) = keys.try_recv() {
                last = Some(k);
            }
            if last.is_none() {
                last = keys.recv_timeout(WAIT).ok();
            }
            let Some(key) = last else {
                return;
            };
            let mut crypto = HeaderCrypto::from_session_key(key);
            send(
                &mut s,
                Some(&mut crypto),
                opcode::SMSG_AUTH_RESPONSE,
                &[messages::AUTH_OK],
            );
            let log = Arc::clone(&log);
            let sessions = Arc::clone(&sessions);
            sessions.fetch_add(1, Ordering::SeqCst);
            thread::spawn(move || {
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
                sessions.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    World { sent, open }
}

/// A fake realmd listing `realms` and the world server they point at. `realms` takes the world's
/// address.
pub(crate) fn servers(realms: impl FnOnce(&str) -> Vec<FakeRealm>) -> (Realmd, World) {
    let world_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = world_listener.local_addr().unwrap().to_string();
    let (keys_tx, keys_rx) = crossbeam_channel::unbounded();
    let sent = world(world_listener, keys_rx);
    (Realmd::start(realms(&addr), keys_tx), sent)
}

/// An address nothing answers on: a port bound and released.
pub(crate) fn dead_address() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

/// The app's ends of the IO thread's channels.
pub(crate) struct Cycles {
    pub(crate) events: Receiver<SessionEvent>,
    pub(crate) login: Sender<LoginRequest>,
    pub(crate) realm: Sender<RealmRequest>,
    pub(crate) pick: Sender<CharRequest>,
    pub(crate) abandon: Arc<AtomicU64>,
}

/// The read thread's cycle loop, as [`super::spawn_net`] runs it, with the world writer dropped.
pub(crate) fn spawn_cycles() -> Cycles {
    let (events_tx, events) = crossbeam_channel::unbounded();
    let (login, login_rx) = crossbeam_channel::unbounded();
    let (realm, realm_rx) = crossbeam_channel::unbounded();
    let (pick, pick_rx) = crossbeam_channel::unbounded();
    let abandon = Arc::new(AtomicU64::new(0));
    let thread_abandon = Arc::clone(&abandon);
    thread::spawn(move || {
        let (writer_tx, _writer_rx) = crossbeam_channel::unbounded::<WorldWriter>();
        let parks = Parks {
            login_rx,
            realm_rx,
            pick_rx,
        };
        let clock = Mutex::new(PingClock::default());
        let mut tails = Default::default();
        loop {
            match run(
                &events_tx,
                &writer_tx,
                &parks,
                &thread_abandon,
                &clock,
                &mut tails,
            ) {
                Ok(Cycle::Exit) => return,
                Ok(Cycle::Repark) => {}
                Ok(Cycle::LoggedOut | Cycle::LoginRefused) => {
                    let _ = events_tx.send(SessionEvent::Disconnected {
                        reason: "logged out".into(),
                        end: SessionEnd::LoggedOut,
                    });
                }
                Err(e) => {
                    if events_tx
                        .send(SessionEvent::Disconnected {
                            reason: format!("disconnected: {e:#}"),
                            end: SessionEnd::Lost,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    });
    Cycles {
        events,
        login,
        realm,
        pick,
        abandon,
    }
}

/// A login attempt for [`USER`] at `host` under the current abandon generation.
pub(crate) fn login(cycles: &Cycles, host: &str) {
    cycles
        .login
        .send(LoginRequest {
            user: USER.into(),
            pass: PASS.into(),
            host: host.into(),
            generation: cycles.abandon.load(Ordering::SeqCst),
        })
        .unwrap();
}
