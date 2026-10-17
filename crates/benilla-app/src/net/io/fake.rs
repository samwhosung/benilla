//! A fake realmd and a fake world server on loopback, for the login cycle's tests: the real SRP6
//! logon, the realm list as often as it is asked for, the world handshake and the roster. Both take
//! any number of connections, so a test can log in again.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use benilla_protocol::messages::{self, opcode};
use benilla_protocol::{SessionEnd, SessionEvent};
use benilla_srp::vanilla_header::HeaderCrypto;
use benilla_srp::{
    password_verifier, NormalizedString, GENERATOR, LARGE_SAFE_PRIME_LITTLE_ENDIAN,
    SESSION_KEY_LENGTH,
};
use crossbeam_channel::{Receiver, Sender};
use num_bigint::BigUint;
use sha1::{Digest, Sha1};

use super::{run, writer_loop, Cycle, LoginRequest, Parks, PingClock, WriterSwap};
use crate::net::{CharRequest, ClientCommand, ParkRequest, ParkSender, RealmRequest};

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

/// A client packet as it left the client, header decrypted: `u16` BE size counting the 4-byte
/// opcode, `u32` LE opcode, the body.
fn recv_packet(s: &mut TcpStream, crypto: Option<&mut HeaderCrypto>) -> Option<Vec<u8>> {
    let mut header = [0u8; 6];
    s.read_exact(&mut header).ok()?;
    if let Some(c) = crypto {
        c.decrypter().decrypt(&mut header);
    }
    let size = usize::from(u16::from_be_bytes([header[0], header[1]]));
    let mut packet = header.to_vec();
    packet.resize(2 + size, 0);
    s.read_exact(&mut packet[6..]).ok()?;
    Some(packet)
}

/// A client packet's opcode, reading past its body.
fn recv(s: &mut TcpStream, crypto: Option<&mut HeaderCrypto>) -> Option<u16> {
    let packet = recv_packet(s, crypto)?;
    Some(u16::from_le_bytes([packet[2], packet[3]]))
}

/// An `SMSG_CHAR_ENUM` body for `characters`, in the order the client parses it
/// (`Character::read`, the reference's `0x5b42a0`).
pub(crate) fn char_enum_body(characters: &[benilla_protocol::Character]) -> Vec<u8> {
    let mut b = vec![characters.len() as u8];
    for c in characters {
        b.extend_from_slice(&c.guid.to_le_bytes());
        b.extend_from_slice(c.name.as_bytes());
        b.push(0);
        b.extend_from_slice(&[
            c.race,
            c.class,
            c.gender,
            c.skin,
            c.face,
            c.hair_style,
            c.hair_color,
            c.facial_hair,
            c.level,
        ]);
        b.extend_from_slice(&c.zone.to_le_bytes());
        b.extend_from_slice(&c.map.to_le_bytes());
        for v in [c.position.x, c.position.y, c.position.z] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&0u32.to_le_bytes()); // guild
        b.extend_from_slice(&c.flags.to_le_bytes());
        b.push(0); // first login
        for v in [c.pet_display_id, c.pet_level, c.pet_family] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for item in &c.equipment {
            b.extend_from_slice(&item.display_id.to_le_bytes());
            b.push(item.inventory_type);
        }
        b.extend_from_slice(&0u32.to_le_bytes()); // first bag
        b.push(0);
    }
    b
}

/// The accounts' characters by world session.
type Rosters = Arc<Mutex<Vec<Vec<benilla_protocol::Character>>>>;

/// Packets, by opcode and body, the fake world sends behind `SMSG_LOGOUT_COMPLETE`.
type Trail = Arc<Mutex<Vec<(u16, Vec<u8>)>>>;

/// Whether refusals are held, and the signal that releases them.
type RefusalGate = Arc<(Mutex<bool>, std::sync::Condvar)>;

/// The `SMSG_ADDON_INFO` body a fake world sends, and whether it trails the authentication answer.
type AddonReply = Arc<Mutex<Option<(Vec<u8>, bool)>>>;

/// The fake world server's record: every opcode the client sent, and the sessions still open.
pub(crate) struct World {
    pub(crate) sent: Arc<Mutex<Vec<u16>>>,
    /// Every packet the client sent, header decrypted, as [`recv_packet`] reads it.
    pub(crate) packets: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Every session's packets in the order it sent them, by the order the sessions began.
    pub(crate) session_packets: Arc<Mutex<Vec<Vec<Vec<u8>>>>>,
    /// The characters each session answers its `CMSG_CHAR_ENUM` with, by the order the sessions
    /// began (the last repeats): none until set. A delete or a rename the server accepts changes
    /// every session's.
    rosters: Rosters,
    /// The `SMSG_CHAR_DELETE` and `SMSG_CHAR_RENAME` results the server answers with: success
    /// (`0x39`, 0) until set.
    results: Arc<Mutex<(u8, u8)>>,
    /// The `SMSG_CHAR_CREATE` result the server answers with: success (`0x2e`) until set.
    creates: Arc<Mutex<u8>>,
    /// How long a `CMSG_CHAR_ENUM` waits before its answer.
    enum_delay: Arc<Mutex<Duration>>,
    /// Send `SMSG_WARDEN_DATA` behind the authentication answer, as vmangos does with Warden on.
    warden: Arc<AtomicBool>,
    /// Close the socket on `CMSG_CHAR_ENUM` instead of answering.
    cut: Arc<AtomicBool>,
    /// Answer `CMSG_PLAYER_LOGIN` with `SMSG_CHARACTER_LOGIN_FAILED`.
    refuse: Arc<AtomicBool>,
    /// Refusals wait while this holds, until [`World::release_refusals`].
    refusal_gate: RefusalGate,
    /// Packets sent right behind `SMSG_LOGOUT_COMPLETE`, as the world session leaves them in flight.
    trail: Trail,
    /// The `SMSG_ADDON_INFO` body every session is sent, and whether it follows the authentication
    /// answer (vmangos builds it in the auth handler, so it leads: `WorldSocket.cpp:447`).
    addon_info: AddonReply,
    open: Arc<AtomicUsize>,
    /// Every session's socket, for [`World::drop_links`].
    links: Arc<Mutex<Vec<TcpStream>>>,
}

impl World {
    /// The characters the account holds from now on.
    pub(crate) fn set_roster(&self, characters: &[benilla_protocol::Character]) {
        *self.rosters.lock().unwrap() = vec![characters.to_vec()];
    }

    /// The characters each session sees, by the order the sessions begin.
    pub(crate) fn set_rosters(&self, per_session: &[&[benilla_protocol::Character]]) {
        *self.rosters.lock().unwrap() = per_session.iter().map(|c| c.to_vec()).collect();
    }

    /// Answer every `CMSG_CHAR_DELETE` with `result` from now on; anything but `0x39` deletes
    /// nothing.
    pub(crate) fn answer_deletes(&self, result: u8) {
        self.results.lock().unwrap().0 = result;
    }

    /// Answer every `CMSG_CHAR_CREATE` with `result` from now on; anything but `0x2e` creates
    /// nothing.
    pub(crate) fn answer_creates(&self, result: u8) {
        *self.creates.lock().unwrap() = result;
    }

    /// Answer every `CMSG_CHAR_RENAME` with `result` from now on; anything but 0 renames nothing.
    pub(crate) fn answer_renames(&self, result: u8) {
        self.results.lock().unwrap().1 = result;
    }

    /// The bodies of every packet the client sent with opcode `op`, past the header.
    pub(crate) fn bodies(&self, op: u16) -> Vec<Vec<u8>> {
        self.packets
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p[2..4] == op.to_le_bytes())
            .map(|p| p[6..].to_vec())
            .collect()
    }

    /// Ask every session from now on for Warden as vmangos does with it on: the session starts
    /// Warden right behind `AUTH_OK` (`World.cpp:333-338`) and its thread sends the first request
    /// within a second (`Anticheat.cpp:111-127`, `WardenWin.cpp:1403-1405`). No handler waits on
    /// the answer, so the roster and the world entry are served as ever; a client that never
    /// answers is kicked later by the server's own clock (`Warden.cpp:686-691`), which this
    /// server leaves to [`World::drop_links`].
    pub(crate) fn ask_warden(&self) {
        self.warden.store(true, Ordering::SeqCst);
    }

    /// Close every session's socket from the server's side, as vmangos does for a session idle at
    /// character select (`CharacterScreenMaxIdleTime`).
    pub(crate) fn drop_links(&self) {
        for s in self.links.lock().unwrap().drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Have the server cut the connection on the next roster ask.
    pub(crate) fn cut_on_roster_ask(&self) {
        self.cut.store(true, Ordering::SeqCst);
    }

    /// Have the server refuse every character login from now on (`SMSG_CHARACTER_LOGIN_FAILED`,
    /// vmangos's result 1).
    pub(crate) fn refuse_logins(&self) {
        self.refuse.store(true, Ordering::SeqCst);
    }

    /// Hold every refusal back until [`Self::release_refusals`].
    pub(crate) fn hold_refusals(&self) {
        *self.refusal_gate.0.lock().unwrap() = true;
    }

    /// Send the refusals [`Self::hold_refusals`] held, and every later one at once.
    pub(crate) fn release_refusals(&self) {
        *self.refusal_gate.0.lock().unwrap() = false;
        self.refusal_gate.1.notify_all();
    }

    /// Send `SMSG_ADDON_INFO` with one record per status (`2` hides the addon) to every session
    /// from now on: ahead of the authentication answer, or behind it with `after`.
    pub(crate) fn set_addon_info(&self, statuses: &[u8], after: bool) {
        // `{status, info 1, key 0, u32 revision 0, url 0}`, the shape of a retail reply.
        let body = statuses
            .iter()
            .flat_map(|&s| [s, 1, 0, 0, 0, 0, 0, 0])
            .collect();
        *self.addon_info.lock().unwrap() = Some((body, after));
    }

    /// [`Self::set_addon_info`] with each record's revision and url: `(status, revision, url)`, a
    /// record with neither carrying no info block and no url.
    pub(crate) fn set_addon_records(
        &self,
        records: &[(u8, Option<u32>, Option<&str>)],
        after: bool,
    ) {
        let mut body = Vec::new();
        for &(status, revision, url) in records {
            body.push(status);
            match revision {
                Some(r) => {
                    body.extend([1, 0]); // infoProvided, no key
                    body.extend(r.to_le_bytes());
                }
                None => body.push(0),
            }
            match url {
                Some(u) => {
                    body.push(1);
                    // A C string, its NUL included (`0x4191b0`).
                    body.extend(u.as_bytes());
                    body.push(0);
                }
                None => body.push(0),
            }
        }
        *self.addon_info.lock().unwrap() = Some((body, after));
    }

    /// Send these packets right behind every `SMSG_LOGOUT_COMPLETE`.
    pub(crate) fn trail_logouts_with(&self, packets: Vec<(u16, Vec<u8>)>) {
        *self.trail.lock().unwrap() = packets;
    }

    /// The `CMSG_AUTH_SESSION`s the server admitted: one per world session.
    pub(crate) fn authentications(&self) -> usize {
        self.session_packets.lock().unwrap().len()
    }

    /// Hold every roster answer back this long.
    pub(crate) fn delay_rosters(&self, by: Duration) {
        *self.enum_delay.lock().unwrap() = by;
    }

    /// How many `CMSG_CHAR_ENUM` session `index` has sent.
    pub(crate) fn enumerations_in(&self, index: usize) -> usize {
        self.session_packets
            .lock()
            .unwrap()
            .get(index)
            .map_or(0, |packets| {
                packets
                    .iter()
                    .filter(|p| p[2..4] == opcode::CMSG_CHAR_ENUM.to_le_bytes())
                    .count()
            })
    }

    /// How many `CMSG_CHAR_ENUM` the client has sent.
    pub(crate) fn enumerations(&self) -> usize {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|&&op| op == opcode::CMSG_CHAR_ENUM)
            .count()
    }

    /// Sessions the client still holds.
    pub(crate) fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }
}

/// A world server whose account has no characters: it admits each session under the latest key
/// realmd made, answers every `CMSG_CHAR_ENUM` with its roster (empty until [`World::set_roster`]),
/// a `CMSG_CHAR_CREATE`, a `CMSG_CHAR_DELETE` and a `CMSG_CHAR_RENAME` with success applied to the
/// roster (or [`World::answer_creates`]', [`World::answer_deletes`]' and
/// [`World::answer_renames`]' results), a
/// `CMSG_LOGOUT_REQUEST` with `SMSG_LOGOUT_COMPLETE` and a `CMSG_PLAYER_LOGIN` with nothing (or a
/// refusal, [`World::refuse_logins`]), and logs every opcode the client sends.
pub(crate) fn world(listener: TcpListener, keys: Receiver<Key>) -> World {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&sent);
    let packets = Arc::new(Mutex::new(Vec::new()));
    let packet_log = Arc::clone(&packets);
    let rosters: Rosters = Arc::new(Mutex::new(vec![Vec::new()]));
    let roster_bodies = Arc::clone(&rosters);
    let results = Arc::new(Mutex::new((messages::CHAR_DELETE_SUCCESS, 0u8)));
    let answers = Arc::clone(&results);
    let creates = Arc::new(Mutex::new(messages::CHAR_CREATE_SUCCESS));
    let create_answer = Arc::clone(&creates);
    let session_packets = Arc::new(Mutex::new(Vec::<Vec<Vec<u8>>>::new()));
    let session_log = Arc::clone(&session_packets);
    let enum_delay = Arc::new(Mutex::new(Duration::ZERO));
    let enum_wait = Arc::clone(&enum_delay);
    let warden = Arc::new(AtomicBool::new(false));
    let warden_flag = Arc::clone(&warden);
    let cut = Arc::new(AtomicBool::new(false));
    let cut_flag = Arc::clone(&cut);
    let refuse = Arc::new(AtomicBool::new(false));
    let refuse_flag = Arc::clone(&refuse);
    let refusal_gate: RefusalGate = Arc::default();
    let refuse_wait = Arc::clone(&refusal_gate);
    let trail: Trail = Arc::default();
    let trailing = Arc::clone(&trail);
    let addon_info: AddonReply = Arc::default();
    let addon_reply = Arc::clone(&addon_info);
    let open = Arc::new(AtomicUsize::new(0));
    let sessions = Arc::clone(&open);
    let links = Arc::new(Mutex::new(Vec::<TcpStream>::new()));
    let link_log = Arc::clone(&links);
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
            // Held and counted before the client can see the session open, so a drop never misses
            // it and `open` never reads behind the client's authentication.
            if let Ok(link) = s.try_clone() {
                link_log.lock().unwrap().push(link);
            }
            let sessions = Arc::clone(&sessions);
            sessions.fetch_add(1, Ordering::SeqCst);
            let mut crypto = HeaderCrypto::from_session_key(key);
            let reply = addon_reply.lock().unwrap().clone();
            if let Some((body, false)) = &reply {
                send(&mut s, Some(&mut crypto), opcode::SMSG_ADDON_INFO, body);
            }
            send(
                &mut s,
                Some(&mut crypto),
                opcode::SMSG_AUTH_RESPONSE,
                &[messages::AUTH_OK],
            );
            if let Some((body, true)) = &reply {
                send(&mut s, Some(&mut crypto), opcode::SMSG_ADDON_INFO, body);
            }
            // An opaque body: the client hands it to its Warden module and reads none of it
            // (`0x6ca5c0`).
            if warden_flag.load(Ordering::SeqCst) {
                send(
                    &mut s,
                    Some(&mut crypto),
                    opcode::SMSG_WARDEN_DATA,
                    &[0x02; 21],
                );
            }
            let log = Arc::clone(&log);
            let packet_log = Arc::clone(&packet_log);
            let roster_bodies = Arc::clone(&roster_bodies);
            let answers = Arc::clone(&answers);
            let create_answer = Arc::clone(&create_answer);
            let session_log = Arc::clone(&session_log);
            let enum_wait = Arc::clone(&enum_wait);
            let cut_flag = Arc::clone(&cut_flag);
            let refuse_flag = Arc::clone(&refuse_flag);
            let refuse_wait = Arc::clone(&refuse_wait);
            let trailing = Arc::clone(&trailing);
            let index = {
                let mut all = session_log.lock().unwrap();
                all.push(Vec::new());
                all.len() - 1
            };
            thread::spawn(move || {
                while let Some(packet) = recv_packet(&mut s, Some(&mut crypto)) {
                    let op = u16::from_le_bytes([packet[2], packet[3]]);
                    log.lock().unwrap().push(op);
                    session_log.lock().unwrap()[index].push(packet.clone());
                    packet_log.lock().unwrap().push(packet.clone());
                    match op {
                        opcode::CMSG_CHAR_ENUM => {
                            let body = {
                                let all = roster_bodies.lock().unwrap();
                                char_enum_body(&all[index.min(all.len() - 1)])
                            };
                            let wait = *enum_wait.lock().unwrap();
                            thread::sleep(wait);
                            if cut_flag.load(Ordering::SeqCst) {
                                break;
                            }
                            send(&mut s, Some(&mut crypto), opcode::SMSG_CHAR_ENUM, &body);
                        }
                        // A success appends the character at level 1 with the next guid.
                        opcode::CMSG_CHAR_CREATE => {
                            let result = *create_answer.lock().unwrap();
                            if result == messages::CHAR_CREATE_SUCCESS {
                                let body = &packet[6..];
                                let end = body.iter().position(|&b| b == 0).unwrap_or(0);
                                let name = String::from_utf8_lossy(&body[..end]).into_owned();
                                let b = &body[end + 1..];
                                for roster in roster_bodies.lock().unwrap().iter_mut() {
                                    let guid = roster.iter().map(|c| c.guid).max().unwrap_or(0) + 1;
                                    let mut c =
                                        crate::glue_session::roster::test_character(guid, &name);
                                    (c.race, c.class, c.gender, c.skin, c.face) =
                                        (b[0], b[1], b[2], b[3], b[4]);
                                    (c.hair_style, c.hair_color, c.facial_hair) =
                                        (b[5], b[6], b[7]);
                                    (c.level, c.zone) = (1, 0);
                                    roster.push(c);
                                }
                            }
                            send(
                                &mut s,
                                Some(&mut crypto),
                                opcode::SMSG_CHAR_CREATE,
                                &[result],
                            );
                        }
                        opcode::CMSG_CHAR_DELETE => {
                            let guid = u64::from_le_bytes(packet[6..14].try_into().unwrap());
                            let result = answers.lock().unwrap().0;
                            if result == messages::CHAR_DELETE_SUCCESS {
                                for roster in roster_bodies.lock().unwrap().iter_mut() {
                                    roster.retain(|c| c.guid != guid);
                                }
                            }
                            send(
                                &mut s,
                                Some(&mut crypto),
                                opcode::SMSG_CHAR_DELETE,
                                &[result],
                            );
                        }
                        // vmangos renames a flagged character, clears the flag and answers the
                        // guid and the name (`CharacterHandler.cpp:802-835`).
                        opcode::CMSG_CHAR_RENAME => {
                            let guid = u64::from_le_bytes(packet[6..14].try_into().unwrap());
                            let name = &packet[14..packet.len() - 1];
                            let result = answers.lock().unwrap().1;
                            let mut body = vec![result];
                            if result == 0 {
                                let name = String::from_utf8_lossy(name).into_owned();
                                for roster in roster_bodies.lock().unwrap().iter_mut() {
                                    for c in roster.iter_mut().filter(|c| c.guid == guid) {
                                        c.name.clone_from(&name);
                                        c.flags &= !benilla_protocol::CHARACTER_FLAG_RENAME;
                                    }
                                }
                                body.extend_from_slice(&guid.to_le_bytes());
                                body.extend_from_slice(name.as_bytes());
                                body.push(0);
                            }
                            send(&mut s, Some(&mut crypto), opcode::SMSG_CHAR_RENAME, &body);
                        }
                        opcode::CMSG_PLAYER_LOGIN if refuse_flag.load(Ordering::SeqCst) => {
                            let (held, released) = &*refuse_wait;
                            let _ = released
                                .wait_timeout_while(held.lock().unwrap(), WAIT, |h| *h)
                                .unwrap();
                            send(
                                &mut s,
                                Some(&mut crypto),
                                opcode::SMSG_CHARACTER_LOGIN_FAILED,
                                &[1],
                            );
                        }
                        opcode::CMSG_LOGOUT_REQUEST => {
                            send(&mut s, Some(&mut crypto), opcode::SMSG_LOGOUT_COMPLETE, &[]);
                            for (op, body) in trailing.lock().unwrap().clone() {
                                send(&mut s, Some(&mut crypto), op, &body);
                            }
                        }
                        _ => {}
                    }
                }
                sessions.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    World {
        sent,
        packets,
        session_packets,
        rosters,
        results,
        creates,
        enum_delay,
        warden,
        cut,
        refuse,
        refusal_gate,
        trail,
        addon_info,
        open,
        links,
    }
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
    /// The app's commands, to the write thread and on to the world socket while in the world.
    pub(crate) commands: Sender<ClientCommand>,
    pub(crate) login: Sender<LoginRequest>,
    pub(crate) realm: ParkSender<RealmRequest>,
    pub(crate) pick: ParkSender<CharRequest>,
    pub(crate) abandon: Arc<AtomicU64>,
}

/// The read thread's cycle loop and the write thread, as [`super::spawn_net`] runs them.
pub(crate) fn spawn_cycles() -> Cycles {
    let (events_tx, events) = crossbeam_channel::unbounded();
    let (commands, cmd_rx) = crossbeam_channel::unbounded::<ClientCommand>();
    let (writer_tx, writer_rx) = crossbeam_channel::unbounded::<WriterSwap>();
    thread::spawn(move || writer_loop(&cmd_rx, writer_rx, &Mutex::new(PingClock::default())));
    let (login, login_rx) = crossbeam_channel::unbounded();
    let (park_tx, park_rx) = crossbeam_channel::unbounded::<ParkRequest>();
    let abandon = Arc::new(AtomicU64::new(0));
    let thread_abandon = Arc::clone(&abandon);
    thread::spawn(move || {
        let parks = Parks { login_rx, park_rx };
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
        commands,
        login,
        realm: ParkSender::park(park_tx.clone()),
        pick: ParkSender::park(park_tx),
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
            locale: benilla_protocol::auth::LOCALE_TAG_EN_US,
            generation: cycles.abandon.load(Ordering::SeqCst),
        })
        .unwrap();
}
