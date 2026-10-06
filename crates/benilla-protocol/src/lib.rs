//! The WoW 1.12.1 (build 5875) wire protocol: [`auth`] for realmd (login protocol version 3),
//! [`world`] and [`events`] for the world server. SRP6 and the header crypto are `benilla-srp`'s.

pub mod auth;
pub mod events;
pub mod guid;
pub mod messages;
pub mod wire;
pub mod world;
pub use auth::AuthReject;
pub use events::{
    decode, CharAction, EntityKind, LoginRefusal, LoginStage, MoveSpeeds, Poll, SessionEnd,
    SessionEvent, SessionEventKind,
};
pub use messages::field;
pub use messages::{
    AttackSwingError, CharCreateReq, CharEnumItem, Character, CorpseLook, CreateSpline, ItemInfo,
    JumpInfo, MonsterMoveFacing, MoveMode, MoverState, ObjectFields, OwnerFallback, RelayVerb,
    ServerPacket, SpeedKind, SplineMode, TransportPose, CHARACTER_FLAG_GHOST,
    CHARACTER_FLAG_HIDE_CLOAK, CHARACTER_FLAG_HIDE_HELM, CHARACTER_FLAG_RENAME,
};
pub use world::{
    observe_sends, SendObserver, WardenRequired, WorldAuthReject, WorldReader, WorldSession,
    WorldWriter, WORLD_PORT,
};

use std::net::TcpStream;

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

/// Resolves, then tries every resolved address in turn: `localhost` gives `::1` first, and a
/// server may bind only IPv4.
fn dial(host: &str, port: u16) -> Result<TcpStream> {
    use std::net::ToSocketAddrs;
    let address = format!("{host}:{port}");
    let addrs: Vec<std::net::SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(addrs) => addrs.collect(),
        Err(e) => {
            return Err(anyhow::Error::new(DialFailure {
                address,
                unresolved: true,
            })
            .context(format!("resolving {host}: {e}")))
        }
    };
    if addrs.is_empty() {
        return Err(anyhow::Error::new(DialFailure {
            address,
            unresolved: true,
        }));
    }
    TcpStream::connect(&addrs[..]).map_err(|e| {
        anyhow::Error::new(DialFailure {
            address,
            unresolved: false,
        })
        .context(format!("connecting: {e}"))
    })
}

/// A realm from realmd's realm list, with every field the wire carries.
#[derive(Debug, Clone)]
pub struct RealmInfo {
    pub name: String,
    /// `host:port` of the world server, as the client would connect to it.
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
}

/// The full SRP6 logon against a vanilla `realmd`, then the realm list.
pub fn logon(host: &str, username: &str, password: &str) -> Result<Logon> {
    let (host, port) = host_port(host, AUTH_PORT);

    let username_n =
        NormalizedString::new(username).map_err(|e| anyhow!("invalid username: {e}"))?;
    let password_n =
        NormalizedString::new(password).map_err(|e| anyhow!("invalid password: {e}"))?;

    // The account name goes uppercased, as SRP6 hashes it. About one `B` in 137 serializes
    // differently in the client and mangos (a right password gets 0x04), so redial for a fresh
    // one; realmd counts nothing before the proof (`AuthSocket::_HandleLogonChallenge`).
    let (mut stream, reply, server_public_key) = {
        let mut dialed = None;
        for _ in 0..MAX_CHALLENGE_DIALS {
            let mut stream = dial(host, port)?;
            auth::write_logon_challenge(&mut stream, &username.to_uppercase(), CLIENT_BUILD)
                .context("sending logon challenge")?;
            let reply =
                auth::read_challenge_reply(&mut stream).context("reading logon challenge reply")?;
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

    let server_proof = auth::read_proof_reply(&mut stream).context("reading logon proof reply")?;
    let client = challenge
        .verify_server_proof(server_proof)
        .map_err(|e| anyhow!("server proof mismatch (wrong password?): {e}"))?;
    let session_key = *client.session_key();

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
