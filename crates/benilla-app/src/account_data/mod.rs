//! Account data: the eight settings sections 1.12.1 keeps both on disk and on the server (the
//! configuration, key bindings and macros account-wide and per character, the window layout and
//! the chat setup per character), kept in step by digest (benilla-re `net/accountdata`).
//!
//! - Every world entry starts with all eight pending (`0x5af840`, from `0x4015e7`).
//! - The server's digests (`SMSG_ACCOUNT_DATA_MD5`, `0x5af8e0`) meet each section's own and the
//!   one cached when they last agreed ([`check`]); a section changed here goes up
//!   (`CMSG_UPDATE_ACCOUNT_DATA`), one changed there is asked for (`CMSG_REQUEST_ACCOUNT_DATA`) and
//!   stays pending until it comes (`SMSG_UPDATE_ACCOUNT_DATA`, `0x5afc60`), and the cache is saved
//!   (`0x5af92c`).
//! - The interface's shutdown is the one writer of the sections in the reference (`0x490bd0`:
//!   chat `0x490c55`, macros `0x490c5a`, bindings `0x490c64`, layout `0x490c79`, each through
//!   `0x5afed0`, the one caller of the timer's arm `0x5aff40`). So a section is uploaded 30 seconds
//!   after the shutdown that wrote it, when its digest moved off the cache (`0x5afd70`): after a
//!   `/reload` (`0x495664`), the one shutdown that keeps the world. A logout request runs every
//!   armed timer first (`0x5ab049` → `0x5afd30`), and leaving the world drops them (`0x5afe00`),
//!   so what a logout writes goes up at the next login's check. benilla saves bindings and macros
//!   when they change and its layout and chat on a debounce; none of those saves arms a timer.
//!
//! The readers here run at the interface load, before the server's digests, where the
//! reference's readers wait for their pending section (`0x5afe50`); a section that comes down is
//! stored and its reader runs again ([`AccountData::take_arrived`]), as the reference's runs once
//! it lands (`0x5afd19`).

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use bevy::prelude::*;

use benilla_protocol::messages::{AccountDataDigest, UpdateAccountData, ACCOUNT_DATA_SECTIONS};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::keybind::KeyBinding;

use crate::net::{ClientCommand, NetCommands, NetHandlerApp};

mod check;
pub(crate) mod sections;
#[cfg(test)]
mod tests;

use check::{check, Transfer};
use sections::Identity;
pub(crate) use sections::{BINDINGS, CHAT, LAYOUT, MACROS};

/// The save timer's delay after a section's last write, `0x7530` ms (`0x5aff4e`).
const SAVE_DELAY: Duration = Duration::from_millis(30_000);

/// The sections' state, `0x85dcf0`'s records: shared through `&self` ([`AccountData::lock`]), as
/// the logout verb flushes inside a script call while the writers' systems hold it too.
#[derive(Resource, Default)]
pub(crate) struct AccountData(Mutex<State>);

impl AccountData {
    /// The state, held for one step; never across Lua.
    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A section's file was saved here mid-session, where the reference marks it for its
    /// shutdown's write instead (the macros' save flag, the bindings' dirty mask `+0xd4`): the
    /// next [`Self::shut_down_ui`] arms its upload.
    pub(crate) fn saved(&self, sections: &[usize]) {
        let mut state = self.lock();
        for &s in sections.iter().filter(|&&s| s < ACCOUNT_DATA_SECTIONS) {
            state.saved[s] = true;
        }
    }

    /// Whether a file from the server replaced section `section` since its reader last ran; the
    /// reader takes it, at its first run too, so it reads the stored file once.
    pub(crate) fn take_arrived(&self, section: usize) -> bool {
        self.lock()
            .arrived
            .get_mut(section)
            .is_some_and(|at| at.take().is_some())
    }

    /// When section `section`'s file landed, its reader still owed: the stamp of its handler, for
    /// a reader that fires on a lane in the packets' order ([`crate::lanes`]).
    pub(crate) fn arrived_at(&self, section: usize) -> Option<crate::lanes::Stamp> {
        self.lock().arrived.get(section).copied().flatten()
    }

    /// Whether section `section`'s reader is owed, without taking it.
    #[cfg(test)]
    pub(crate) fn arrived(&self, section: usize) -> bool {
        self.arrived_at(section).is_some()
    }

    /// The sections as the server's digests leave them with nothing sent: none pending, for a test
    /// of what a write does.
    #[cfg(test)]
    pub(crate) fn settle_for_test(&self, identity: Option<Identity>) {
        let mut state = self.lock();
        state.identity = identity;
        state.pending = [false; ACCOUNT_DATA_SECTIONS];
    }

    /// Section `section` as a download leaves it, its reader owed, for a test of the reader.
    #[cfg(test)]
    pub(crate) fn owe_reader_for_test(&self, section: usize) {
        self.lock().arrived[section] = Some(crate::lanes::stamp());
    }

    /// The interface's shutdown (`0x490bd0`), its section writes: the chat and the layout always
    /// (`0x490c55`, `0x490c79`), the macros and the bindings when saved since the last
    /// (`0x490c5a`, `0x490c64`). Each restarts its section's upload timer (`0x5aff30`), unless the
    /// section is pending, where the write is skipped (`0x5afee8`).
    pub(crate) fn shut_down_ui(&self) {
        let at = Instant::now() + SAVE_DELAY;
        let mut state = self.lock();
        for s in 0..ACCOUNT_DATA_SECTIONS {
            let writes = s == LAYOUT || s == CHAT || std::mem::take(&mut state.saved[s]);
            if writes && !state.pending[s] {
                state.timers[s] = Some(at);
            }
        }
    }

    /// Whether section `section` was saved since the interface last shut down.
    #[cfg(test)]
    pub(crate) fn saved_for_test(&self, section: usize) -> bool {
        self.lock().saved[section]
    }

    /// A section's file as the server's answer lands, through the handler's own step, for a test
    /// of a reader the download reaches.
    #[cfg(test)]
    pub(crate) fn arrive_for_test(&self, identity: Identity, section: u32, file: &[u8]) {
        let body = benilla_protocol::messages::update_account_data(section, file);
        let mut state = self.lock();
        state.identity = Some(identity);
        state.arrive(&UpdateAccountData {
            section,
            size: file.len() as u32,
            stream: body[8..].to_vec(),
        });
    }

    /// Whether section `section`'s save timer is armed.
    #[cfg(test)]
    pub(crate) fn armed_for_test(&self, section: usize) -> bool {
        self.lock().timers[section].is_some()
    }

    /// `0x5afd30`, ahead of a logout request: every armed timer runs now, so what was written
    /// goes up before the session ends.
    pub(crate) fn flush(&self, commands: &NetCommands) {
        self.lock().fire(None, &mut |c| {
            let _ = commands.0.send(c);
        });
    }
}

/// The eight records' fields that matter here, and what reads the files.
pub(crate) struct State {
    /// The character the per-character sections belong to, seated by the server's digests.
    identity: Option<Identity>,
    /// The pending byte (`+0xc`): set at world entry and by a request, cleared by the check and by
    /// the section's arrival.
    pending: [bool; ACCOUNT_DATA_SECTIONS],
    /// The cached digest (`+0x1c`), the `cache.md5` file's.
    cache: [AccountDataDigest; ACCOUNT_DATA_SECTIONS],
    /// Saved here since the interface last shut down, owed the shutdown's write.
    saved: [bool; ACCOUNT_DATA_SECTIONS],
    /// The save timer (`+0x18`): when it fires.
    timers: [Option<Instant>; ACCOUNT_DATA_SECTIONS],
    /// A section the server's file replaced, owed its reader: the stamp it landed at.
    arrived: [Option<crate::lanes::Stamp>; ACCOUNT_DATA_SECTIONS],
    /// The default key bindings the binding sections are written against, read on first need.
    defaults: Option<Vec<KeyBinding>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            identity: None,
            pending: [true; ACCOUNT_DATA_SECTIONS],
            cache: [[0; 16]; ACCOUNT_DATA_SECTIONS],
            saved: [false; ACCOUNT_DATA_SECTIONS],
            timers: [None; ACCOUNT_DATA_SECTIONS],
            arrived: [None; ACCOUNT_DATA_SECTIONS],
            defaults: None,
        }
    }
}

impl State {
    /// The default bindings, `WTF\DefaultBindings.wtf` as the loader takes it.
    fn defaults(&mut self) -> &[KeyBinding] {
        self.defaults.get_or_insert_with(|| {
            crate::bindings::store::key_table(crate::ui_script::default_bindings())
        })
    }

    /// Whether section `s` has a file here: a per-character one needs the character.
    fn has_file(&self, s: usize) -> bool {
        sections::ACCOUNT_WIDE[s] || self.identity.is_some()
    }

    /// The world entry's reset (`0x5af840`) and the world leave's (`0x5afe00`): every section
    /// pending, no timer armed, the character unknown. The cached digests stay, as the reference's
    /// do when `cache.md5` will not open at the next entry (`0x5af898`).
    fn reset(&mut self) {
        let (cache, defaults) = (self.cache, self.defaults.take());
        *self = Self {
            cache,
            defaults,
            ..Self::default()
        };
    }

    /// `0x5af8e0`: the cached digests read (`0x5af880`), each section checked against the
    /// server's, the sends made, and the cache saved (`0x5af9c0`).
    fn reconcile(
        &mut self,
        server: &[AccountDataDigest; ACCOUNT_DATA_SECTIONS],
        identity: Option<Identity>,
        send: &mut dyn FnMut(ClientCommand),
    ) {
        self.identity = identity;
        read_cache(&mut self.cache);
        for (s, server) in server.iter().enumerate() {
            self.pending[s] = false;
            // No character (a rigged run): its sections are left as they are on both sides.
            if !self.has_file(s) {
                continue;
            }
            let identity = self.identity.clone();
            let local = sections::read(s, identity.as_ref(), self.defaults());
            let checked = check(&sections::digest(local.as_deref()), &self.cache[s], server);
            match checked.send {
                Transfer::Nothing => {}
                // A file that does not open sends nothing (`0x5afb09`).
                Transfer::Upload => {
                    if let Some(contents) = local {
                        send(upload(s, contents));
                    }
                }
                Transfer::Request => {
                    send(ClientCommand::RequestAccountData { section: s as u32 });
                    self.pending[s] = true;
                }
            }
            self.cache[s] = checked.cache;
        }
        write_cache(&self.cache);
    }

    /// `0x5afc60`: the section's file stored unless the size is 0, the pending byte cleared and,
    /// for a stored file, its reader owed. The section number is the server's and the reference
    /// checks no range (`0x5afd01`); one past the table is dropped here.
    fn arrive(&mut self, data: &UpdateAccountData) {
        let s = data.section as usize;
        if s >= ACCOUNT_DATA_SECTIONS {
            warn!("account data: the server sent section {s}, past the eight");
            return;
        }
        if let Some(contents) = data.contents() {
            let identity = self.identity.clone();
            if sections::write(s, identity.as_ref(), self.defaults(), &contents) {
                self.arrived[s] = Some(crate::lanes::stamp());
            }
        } else if data.size != 0 {
            warn!("account data: section {s}'s stream does not inflate to its size");
        }
        self.pending[s] = false;
    }

    /// The save timers due by `now` (every armed one for `None`): each runs `0x5afd70`, which
    /// uploads the section when its digest left the cache, caches it and saves the cache.
    fn fire(&mut self, now: Option<Instant>, send: &mut dyn FnMut(ClientCommand)) {
        for s in 0..ACCOUNT_DATA_SECTIONS {
            let due = self.timers[s].is_some_and(|at| now.is_none_or(|now| at <= now));
            if !due {
                continue;
            }
            self.timers[s] = None;
            if !self.has_file(s) {
                continue;
            }
            let identity = self.identity.clone();
            let local = sections::read(s, identity.as_ref(), self.defaults());
            let digest = sections::digest(local.as_deref());
            if digest == self.cache[s] {
                continue;
            }
            if let Some(contents) = local {
                send(upload(s, contents));
            }
            self.cache[s] = digest;
            write_cache(&self.cache);
        }
    }
}

fn upload(section: usize, contents: Vec<u8>) -> ClientCommand {
    ClientCommand::UpdateAccountData {
        section: section as u32,
        contents,
    }
}

/// `0x5af880`: `cache.md5` into the cached digests, each cleared before its 16 bytes are read; a
/// file that does not open leaves them as they were (`0x5af898`).
fn read_cache(cache: &mut [AccountDataDigest; ACCOUNT_DATA_SECTIONS]) {
    let Some(bytes) =
        crate::local_state::account_data_digests_path().and_then(|path| std::fs::read(path).ok())
    else {
        return;
    };
    for (s, digest) in cache.iter_mut().enumerate() {
        *digest = [0; 16];
        let from = (s * 16).min(bytes.len());
        let to = (from + 16).min(bytes.len());
        digest[..to - from].copy_from_slice(&bytes[from..to]);
    }
}

/// `0x5af9c0`: the cached digests to `cache.md5`, eight times 16 bytes.
fn write_cache(cache: &[AccountDataDigest; ACCOUNT_DATA_SECTIONS]) {
    let Some(path) = crate::local_state::account_data_digests_path() else {
        return;
    };
    if let Err(e) = crate::local_state::write_atomic_bytes(&path, cache.as_flattened()) {
        warn!("account data: saving {}: {e}", path.display());
    }
}

/// The server's digests: the check of every section.
fn on_digests(
    In(ev): In<SessionEvent>,
    data: Res<AccountData>,
    commands: Res<NetCommands>,
    roster: Option<Res<crate::glue_session::roster::Roster>>,
) {
    let SessionEvent::AccountDataMd5(server) = ev else {
        return;
    };
    // No state folder (a capture): the sections are the session's alone.
    if crate::local_state::home().is_none() {
        return;
    }
    let identity = roster.as_deref().and_then(crate::ui_macro::identity);
    data.lock().reconcile(&server, identity, &mut |c| {
        let _ = commands.0.send(c);
    });
}

/// A section's file from the server.
fn on_section(In(ev): In<SessionEvent>, data: Res<AccountData>) {
    let SessionEvent::UpdateAccountData(section) = ev else {
        return;
    };
    data.lock().arrive(&section);
}

/// The save timers that are due.
fn fire_timers(data: Res<AccountData>, commands: Res<NetCommands>) {
    data.lock().fire(Some(Instant::now()), &mut |c| {
        let _ = commands.0.send(c);
    });
}

/// Leaving the world: the timers dropped unrun and every section pending again.
fn on_world_leave(data: Res<AccountData>) {
    data.lock().reset();
}

pub(crate) struct AccountDataPlugin;

impl Plugin for AccountDataPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AccountData>()
            .net_handler(SessionEventKind::AccountDataMd5, on_digests)
            .net_handler(SessionEventKind::UpdateAccountData, on_section)
            .add_systems(
                Update,
                fire_timers.in_set(crate::client_state::InWorldGated),
            )
            // After the interface's shutdown, whose writes it drops unrun (`0x402048`, after
            // `ClientDestroyGame`'s interface teardown).
            .add_systems(
                OnExit(crate::client_state::ClientState::InWorld),
                on_world_leave.in_set(crate::ui_script::SessionEnd::Clears),
            );
    }
}
