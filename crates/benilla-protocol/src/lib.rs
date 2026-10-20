//! The WoW 1.12.1 (build 5875) wire protocol: [`auth`] for realmd (login protocol version 3),
//! [`world`] and [`events`] for the world server. SRP6 and the header crypto are `benilla-srp`'s.

pub mod auth;
pub mod events;
pub mod guid;
pub mod messages;
pub mod wire;
pub mod world;
pub use auth::{AuthReject, LogonReply};
pub use events::{
    decode, CharAction, EntityKind, LoginRefusal, LoginStage, MoveSpeeds, Poll, RosterCause,
    SessionEnd, SessionEvent, SessionEventKind,
};
pub use messages::field;
pub use messages::{
    AttackSwingError, CharCreateReq, CharEnumItem, Character, CorpseLook, CreateSpline, ItemInfo,
    JumpInfo, MonsterMoveFacing, MoveMode, MoverState, ObjectFields, OwnerFallback, RelayVerb,
    ServerPacket, SpeedKind, SplineMode, TransportPose, WowTime, CHARACTER_FLAG_GHOST,
    CHARACTER_FLAG_HIDE_CLOAK, CHARACTER_FLAG_HIDE_HELM, CHARACTER_FLAG_RENAME,
};
pub use world::{
    observe_sends, LinkTraffic, SendObserver, Unparsed, WorldAuthReject, WorldConnectEvent,
    WorldReader, WorldSession, WorldWriter, WORLD_PORT,
};

use std::net::{SocketAddr, TcpStream};

use anyhow::{anyhow, Context, Result};
use benilla_srp::{NormalizedString, PublicKey, SrpClientChallenge, SESSION_KEY_LENGTH};

/// The port a stock vmangos `realmd` listens on.
pub const AUTH_PORT: u16 = 3724;
/// The 1.12.1 client build we present to the server.
pub const CLIENT_BUILD: u16 = 5875;
/// Challenges [`logon`] draws for an unambiguous `B`; one in ~137 is not, so all 8 fail ~10⁻¹⁷.
const MAX_CHALLENGE_DIALS: u32 = 8;

/// Splits an optional `:port` off a host; one without a numeric port, a raw IPv6 address
/// included, comes back whole with `default`.
pub fn host_port(host: &str, default: u16) -> (&str, u16) {
    match host.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => match p.parse::<u16>() {
            Ok(p) => (h, p),
            Err(_) => (host, default),
        },
        _ => (host, default),
    }
}

/// A dial that got no socket, split by step: the name did not resolve, or nothing answered (one
/// `io::Error` cannot tell them apart; macOS reports a failed lookup as `Uncategorized`).
#[derive(Debug, Clone)]
pub struct DialFailure {
    /// What we tried to reach, as the player would recognise it (`host:port`).
    pub address: String,
    /// `true` = the host name resolved to nothing; `false` = it resolved and the connection failed.
    pub unresolved: bool,
}

impl std::fmt::Display for DialFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.unresolved {
            write!(f, "cannot find a server called {}", self.address)
        } else {
            write!(f, "nothing answered at {}", self.address)
        }
    }
}

impl std::error::Error for DialFailure {}

/// The port a realm's world address dials when it names none (`0x2382`, `0x53785a`).
const WORLD_DEFAULT_PORT: u16 = 9090;

/// Resolves, then dials the first IPv4 address ([`first_ipv4`]).
fn dial(host: &str, port: u16) -> Result<TcpStream> {
    use std::net::ToSocketAddrs;
    let address = format!("{host}:{port}");
    let addrs: Vec<SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(addrs) => addrs.collect(),
        Err(e) => {
            return Err(anyhow::Error::new(DialFailure {
                address,
                unresolved: true,
            })
            .context(format!("resolving {host}: {e}")))
        }
    };
    let Some(target) = first_ipv4(&addrs) else {
        return Err(anyhow::Error::new(DialFailure {
            address,
            unresolved: true,
        }));
    };
    connect(target, &address)
}

/// Open a socket to `target`; a refusal is a [`DialFailure`] naming `address`.
fn connect(target: SocketAddr, address: &str) -> Result<TcpStream> {
    TcpStream::connect(target).map_err(|e| {
        anyhow::Error::new(DialFailure {
            address: address.to_string(),
            unresolved: false,
        })
        .context(format!("connecting: {e}"))
    })
}

/// The address the world dial takes: IPv4 alone, the first the resolver gives
/// (`WowConnection::ConnectToHost` `0x5b6e40`: `inet_addr`, else `gethostbyname`'s first entry).
/// `localhost` resolves to `::1` first, which a server bound to IPv4 refuses only after about 2 s
/// on Windows.
fn first_ipv4(addrs: &[SocketAddr]) -> Option<SocketAddr> {
    addrs.iter().copied().find(SocketAddr::is_ipv4)
}

/// Dial a realm's world server at its realm-list address ([`world_host_port`]).
pub(crate) fn dial_world(address: &str) -> Result<TcpStream> {
    let (host, port) = world_host_port(address);
    dial(host, port)
}

/// A realm's world address as `NetClient::Connect` (`0x537820`) splits it: the host up to the
/// first `:` (`SStrChr2` `0x64a3f0`, `0x53786c`) and the low 16 bits (`0x5b6e94`) of `_atoi` of
/// the rest (`0x53787a`), or the whole text and [`WORLD_DEFAULT_PORT`] with no `:`.
fn world_host_port(address: &str) -> (&str, u16) {
    match address.split_once(':') {
        Some((host, port)) => (host, atoi(port) as u16),
        None => (address, WORLD_DEFAULT_PORT),
    }
}

/// `_atoi` (`0x41449c` → `_atol` `0x414411`): C-locale spaces skipped, one `-` or `+`, then the
/// decimal digits in wrapping 32-bit arithmetic up to the first other byte; 0 with none.
fn atoi(text: &str) -> i32 {
    let mut bytes = text
        .bytes()
        .skip_while(|b| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
        .peekable();
    let negative = match bytes.peek() {
        Some(b'-') => {
            bytes.next();
            true
        }
        Some(b'+') => {
            bytes.next();
            false
        }
        _ => false,
    };
    let value = bytes.take_while(u8::is_ascii_digit).fold(0i32, |n, d| {
        n.wrapping_mul(10).wrapping_add(i32::from(d - b'0'))
    });
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

/// The hosts of a realmlist the logon resolves, in order: the text cut at each `;` (`SStrChr2` at
/// `0x43b8f6`) up to the first empty piece, which ends the list (`0x43b941`), each split by
/// [`host_port`] with the request's port 3724 (`0xe8c`, `0x5bc400`) when it names none. The
/// reference branches on a piece's first byte (`0x43b947`, `0x43b94f`, `'0'..'9'`): a digit-led
/// piece splits a `:port` (`0x43f3d0`: `SStrChr`, `SStrToInt`, then `inet_addr` unchecked, so a
/// failed parse stores 255.255.255.255), while a name goes to `gethostbyname` whole, `:port`
/// included (`0x43ba0e`), every address it returns taking 3724 (`0x43ba86`); a name's port here
/// is the deviation below.
fn logon_pieces(realmlist: &str) -> Vec<(&str, u16)> {
    realmlist
        .split(';')
        .take_while(|piece| !piece.is_empty())
        // Deviation: a named piece takes a `:port` here, where the reference hands a name to
        // `gethostbyname` whole (`0x43ba0e`) and only a dotted piece takes a port (`0x43f3d0`),
        // because a server on a non-default logon port behind a host name is everyday use.
        .map(|piece| host_port(piece, AUTH_PORT))
        .collect()
}

/// `ConnectToRandomAddress` (`0x5bc2b0`): entry `draw mod count` (`0x5bc392`) of the list, or none
/// of an empty one, which the reference reports as a failed dial (`0x5bc3cb`).
fn pick(addrs: &[SocketAddr], draw: u32) -> Option<SocketAddr> {
    let count = u32::try_from(addrs.len()).ok().filter(|&n| n > 0)?;
    Some(addrs[(draw % count) as usize])
}

/// The address a logon dials, and the realmlist's hosts for a failure to name. The list is every
/// IPv4 address of each piece in order, at the piece's port ([`logon_pieces`]): one for a dotted
/// address, every `AF_INET` address `gethostbyname` gives for a name (`0x43ba07`-`0x43baf6`), none
/// for a name that resolves to none (`0x43ba18`). One is drawn at random ([`pick`]), as
/// `ClientLink::Connect` (`0x5bc3e0`) hands the list to `ConnectToRandomAddress`.
fn logon_target(realmlist: &str) -> Result<(SocketAddr, String)> {
    use std::net::ToSocketAddrs;
    let pieces = logon_pieces(realmlist);
    let address = pieces
        .iter()
        .map(|(host, port)| format!("{host}:{port}"))
        .collect::<Vec<_>>()
        .join(";");
    let mut addrs = Vec::new();
    let mut unresolved = Vec::new();
    for &(host, port) in &pieces {
        match (host, port).to_socket_addrs() {
            Ok(found) => addrs.extend(found.filter(SocketAddr::is_ipv4)),
            Err(e) => unresolved.push(format!("resolving {host}: {e}")),
        }
    }
    match pick(&addrs, rand::random()) {
        Some(target) => Ok((target, address)),
        None => {
            let failure = anyhow::Error::new(DialFailure {
                address,
                unresolved: true,
            });
            Err(if unresolved.is_empty() {
                failure
            } else {
                failure.context(unresolved.join("; "))
            })
        }
    }
}

/// A realm from realmd's realm list, with every field the wire carries.
#[derive(Debug, Clone)]
pub struct RealmInfo {
    pub name: String,
    /// The world server's `host[:port]`, as the client connects to it ([`WorldSession::connect`]).
    pub address: String,
    /// The population, sentinels already rewritten as the reference parser does
    /// ([`auth::MAGIC_POPULATIONS`]); the list shows a band against all realms' mean and deviation.
    pub population: f32,
    /// This account's character count there, shown as `"(3)"` (client realm record `+0x130`).
    pub characters: u8,
    /// The realm type (record `+0x04`), a join key, not an enum: `GetRealmInfo` takes `(pvp, rp)`
    /// from the `Cfg_Configs.dbc` row whose `RealmType` equals it.
    pub realm_type: u32,
    /// The realm flags as the client holds them (record `+0x08`): the wire carries only `0x01`
    /// invalid, `0x02` offline and `0x04`; `0x20`/`0x40`/`0x80` (Recommended, New, Full) are
    /// synthesized from the magic populations ([`auth::MAGIC_POPULATIONS`]).
    pub flags: u8,
    /// The category (timezone) byte the list's tabs group on, matched by equality, not an index.
    pub category: u8,
    /// The wire's realm id.
    pub id: u8,
}

/// A successful logon: the SRP6 session key, the realms, and the realmd socket, kept open because
/// the reference's `RealmList.lua` re-requests the list every `REALM_LIST_REFRESH_TIME` (5 s).
pub struct Logon {
    pub session_key: [u8; SESSION_KEY_LENGTH],
    pub realms: Vec<RealmInfo>,
    /// `None` once a refresh has failed; the held list is then final.
    stream: Option<TcpStream>,
}

impl Logon {
    /// The reference's `RequestRealmList`: re-reads the list, keeping the old one and dropping
    /// the socket on failure (realmd closes idle sockets); `timeout` bounds a silent server.
    pub fn refresh_realms(&mut self, timeout: std::time::Duration) -> bool {
        let Some(stream) = self.stream.as_mut() else {
            return false;
        };
        let refreshed = (|| -> Result<Vec<RealmInfo>> {
            stream.set_read_timeout(Some(timeout))?;
            auth::write_realm_list_request(stream).context("requesting realm list")?;
            auth::read_realm_list(stream).context("reading realm list")
        })();
        match refreshed {
            Ok(realms) => {
                self.realms = realms;
                true
            }
            Err(_) => {
                self.stream = None;
                false
            }
        }
    }

    /// Whether the realmd socket is still up, so [`Self::refresh_realms`] can do anything.
    pub fn realmd_live(&self) -> bool {
        self.stream.is_some()
    }

    /// `CLoginMgr::Cancel` (`0x5b3320`): the realmd socket closes; the session key and the last
    /// list are kept.
    pub fn close(&mut self) {
        self.stream = None;
    }
}

/// The full SRP6 logon against a vanilla `realmd`, then the realm list. `realmlist` is one or more
/// `host[:port]` separated by `;`, the server dialled one of their addresses (`logon_target`).
/// `locale` is the client's locale tag the challenge carries ([`auth::write_logon_challenge`]).
pub fn logon(realmlist: &str, username: &str, password: &str, locale: u32) -> Result<Logon> {
    let username_n =
        NormalizedString::new(username).map_err(|e| anyhow!("invalid username: {e}"))?;
    let password_n =
        NormalizedString::new(password).map_err(|e| anyhow!("invalid password: {e}"))?;
    // One draw per login, as the reference dials once: the redials below go to the same address.
    let (target, address) = logon_target(realmlist)?;

    // The account name goes uppercased, as SRP6 hashes it. About one `B` in 137 serializes
    // differently in the client and mangos (a right password gets 0x04), so redial for a fresh
    // one; realmd counts nothing before the proof (`AuthSocket::_HandleLogonChallenge`).
    let (mut stream, reply, server_public_key) = {
        let mut dialed = None;
        for _ in 0..MAX_CHALLENGE_DIALS {
            let mut stream = connect(target, &address)?;
            auth::write_logon_challenge(
                &mut stream,
                &username.to_uppercase(),
                CLIENT_BUILD,
                locale,
            )
            .context("sending logon challenge")?;
            let Some(reply) =
                auth::read_challenge_reply(&mut stream).context("reading logon challenge reply")?
            else {
                // `SUCCESS_SURVEY` on the challenge authenticates with no SRP exchange; the
                // reference's success arm copies no key (length 0 at `0x5b2e16`).
                return finish_logon(stream, [0u8; SESSION_KEY_LENGTH]);
            };
            let server_public_key = PublicKey::from_le_bytes(reply.server_public_key)
                .map_err(|e| anyhow!("invalid server public key: {e}"))?;
            let stable = server_public_key.is_width_stable();
            dialed = Some((stream, reply, server_public_key));
            if stable {
                break;
            }
        }
        // All ambiguous: go on with the last, since only the server can say it fails.
        dialed.expect("MAX_CHALLENGE_DIALS is non-zero")
    };

    let challenge = SrpClientChallenge::new(
        username_n,
        password_n,
        reply.generator,
        reply.large_safe_prime,
        server_public_key,
        reply.salt,
    );

    auth::write_logon_proof(
        &mut stream,
        challenge.client_public_key(),
        challenge.client_proof(),
        &reply.crc_salt,
    )
    .context("sending logon proof")?;

    let session_key =
        match auth::read_proof_reply(&mut stream).context("reading logon proof reply")? {
            Some(server_proof) => *challenge
                .verify_server_proof(server_proof)
                .map_err(|e| anyhow!("server proof mismatch (wrong password?): {e}"))?
                .session_key(),
            // `SUCCESS_SURVEY` on the proof authenticates with no `M2` to check; the key is the one
            // this exchange derived.
            None => challenge.into_unverified_session_key(),
        };
    finish_logon(stream, session_key)
}

/// An authenticated logon's tail: the realm list over the same socket.
fn finish_logon(mut stream: TcpStream, session_key: [u8; SESSION_KEY_LENGTH]) -> Result<Logon> {
    auth::write_realm_list_request(&mut stream).context("requesting realm list")?;
    let realms = auth::read_realm_list(&mut stream).context("reading realm list")?;

    Ok(Logon {
        session_key,
        realms,
        stream: Some(stream),
    })
}

#[cfg(test)]
mod host_port_tests {
    use super::{host_port, AUTH_PORT};

    #[test]
    fn explicit_port_splits() {
        assert_eq!(
            host_port("play.example.com:5000", AUTH_PORT),
            ("play.example.com", 5000)
        );
    }

    #[test]
    fn bare_host_gets_default() {
        assert_eq!(host_port("localhost", AUTH_PORT), ("localhost", AUTH_PORT));
    }

    #[test]
    fn raw_ipv6_is_not_split() {
        assert_eq!(host_port("::1", 3724), ("::1", 3724));
    }

    #[test]
    fn non_numeric_suffix_stays_host() {
        assert_eq!(
            host_port("play.example.com:realmd", 3724),
            ("play.example.com:realmd", 3724)
        );
    }
}

#[cfg(test)]
mod dial_tests {
    use super::{first_ipv4, logon_pieces, pick, world_host_port, AUTH_PORT};
    use std::net::SocketAddr;

    fn addrs(list: &[&str]) -> Vec<SocketAddr> {
        list.iter().map(|a| a.parse().unwrap()).collect()
    }

    #[test]
    fn localhost_dials_its_ipv4_address() {
        let got = first_ipv4(&addrs(&["[::1]:3724", "127.0.0.1:3724"]));
        assert_eq!(got, Some("127.0.0.1:3724".parse().unwrap()));
    }

    #[test]
    fn the_first_ipv4_address_is_dialled_and_ipv6_never() {
        let got = first_ipv4(&addrs(&["[2001:db8::2]:1", "10.0.0.2:1", "10.0.0.1:1"]));
        assert_eq!(got, Some("10.0.0.2:1".parse().unwrap()));
        assert_eq!(first_ipv4(&addrs(&["[2001:db8::1]:1"])), None);
    }

    /// `NetClient::Connect` (`0x537820`): the port is 9090 (`0x2382`, `0x53785a`) unless the text
    /// has a `:`, where the host is cut at the first one (`SStrChr2` `0x64a3f0`, `0x53786c`) and
    /// the port is `_atoi` of the rest (`0x53787a`).
    #[test]
    fn a_world_address_without_a_port_dials_9090() {
        assert_eq!(
            world_host_port("realm.example.org"),
            ("realm.example.org", 9090)
        );
        assert_eq!(world_host_port("10.0.0.1"), ("10.0.0.1", 9090));
        assert_eq!(world_host_port("10.0.0.1:8085"), ("10.0.0.1", 8085));
    }

    /// The cut is at the first `:`, and `_atol` (`0x414411`) skips C-locale spaces, takes one
    /// sign and reads digits up to the first other character, 0 with none; the dial keeps the
    /// low 16 bits (`0x5b6e94`).
    #[test]
    fn a_world_port_is_atoi_of_the_text_after_the_first_colon() {
        assert_eq!(world_host_port("a:8085:9"), ("a", 8085));
        assert_eq!(world_host_port("a: 8085x"), ("a", 8085));
        assert_eq!(world_host_port("a:+8085"), ("a", 8085));
        assert_eq!(world_host_port("a:"), ("a", 0));
        assert_eq!(world_host_port("a:port"), ("a", 0));
        assert_eq!(world_host_port("a:-1"), ("a", 0xffff));
        assert_eq!(world_host_port("a:73621"), ("a", 8085));
        assert_eq!(world_host_port(":8085"), ("", 8085));
    }

    /// The logon resolver splits the text at each `;` (`0x43b8f6`) and resolves the pieces in
    /// order (`0x43b890`); the first empty piece ends the list (`0x43b941`).
    #[test]
    fn a_realmlist_is_split_at_each_semicolon_until_an_empty_piece() {
        assert_eq!(
            logon_pieces("logon.example.org"),
            [("logon.example.org", AUTH_PORT)]
        );
        assert_eq!(
            logon_pieces("10.0.0.1;10.0.0.2:3725;b.example.org"),
            [
                ("10.0.0.1", AUTH_PORT),
                ("10.0.0.2", 3725),
                ("b.example.org", AUTH_PORT)
            ]
        );
        assert_eq!(logon_pieces("a;b;"), [("a", AUTH_PORT), ("b", AUTH_PORT)]);
        assert_eq!(logon_pieces("a;;b"), [("a", AUTH_PORT)]);
        assert!(logon_pieces(";a").is_empty());
    }

    /// `ConnectToRandomAddress` (`0x5bc2b0`) dials entry `draw mod count` (`0x5bc392`) of every
    /// address the pieces resolved to.
    #[test]
    fn the_logon_dials_the_entry_its_draw_picks() {
        let list = addrs(&["10.0.0.1:3724", "10.0.0.2:3724", "10.0.0.3:3724"]);
        assert_eq!(pick(&list, 0), Some(list[0]));
        assert_eq!(pick(&list, 5), Some(list[2]));
        assert_eq!(pick(&list, u32::MAX), Some(list[0]));
        assert_eq!(pick(&[], 1), None);
    }
}
