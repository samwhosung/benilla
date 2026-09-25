//! Character select: the app's lifecycle state ([`ClientState`]), the roster pick policy and the
//! `CharacterSelect.xml` glue screen. The parked IO thread emits the roster ([`CharListMessage`]),
//! answered by the pending pick (reconnect), `WOW_CHAR`, or a choice on the screen, which opens on
//! the character last entered as (`lastCharacterIndex`).

mod addons;
mod dialog;
mod input;
mod refresh;
mod screen;

use benilla_protocol::{CharAction, Character};
use bevy::prelude::*;

use crate::glue_strings::GlueStrings;

use crate::net::{
    CharActionResultMessage, CharListMessage, CharPick, CharRequest, CharacterLoginFailedMessage,
    EnteredWorldMessage, LoggedOutMessage,
};

/// The app's lifecycle: which screen owns the session.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum ClientState {
    /// Parked pre-logon at the login screen, the IO thread waiting for credentials. The realm list
    /// is no state: `GlueParent.lua`'s `GlueScreenInfo` omits it, a `DIALOG` frame over any screen
    /// ([`crate::realm_select::Realms::shown`]).
    #[default]
    Login,
    /// Parked at character select, the IO thread waiting for a pick.
    CharSelect,
    /// The character-creation screen, still parked at select (the IO thread serves create and
    /// delete in place).
    CharCreate,
    /// A character is in (or entering) the world.
    InWorld,
}

/// The one "only while in the world" gate; a member's own `run_if` ANDs with it. Configured for
/// `Update` only: a set's conditions are per schedule, so a member elsewhere needs the set
/// configured there too.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InWorldGated;

/// The character-select subsystem: the state machine + the select screen.
pub(crate) struct CharSelectPlugin {
    /// The screen this session opens on: `Login` when connected, `InWorld` for a world capture,
    /// the photographed glue screen for a glue capture.
    pub(crate) start: ClientState,
}

/// Mirror the session's screen onto the engine's one-bit "is there a world" fact, in one place so
/// no transition can forget it.
fn publish_world_live(
    state: Res<State<ClientState>>,
    mut live: ResMut<benilla_world::schedule::WorldLive>,
) {
    let now = benilla_world::schedule::WorldLive(*state.get() == ClientState::InWorld);
    if *live != now {
        *live = now;
    }
}

impl Plugin for CharSelectPlugin {
    fn build(&self, app: &mut App) {
        app.insert_state(self.start)
            .configure_sets(Update, InWorldGated.run_if(in_state(ClientState::InWorld)))
            .init_resource::<Roster>()
            // Ahead of every world stage, so `WorldLive` and its falling edge are this frame's.
            .add_systems(
                Update,
                publish_world_live.before(benilla_world::schedule::WorldStage::Net),
            )
            .init_resource::<dialog::DeleteDialog>()
            .init_resource::<addons::AddonsPanel>()
            .add_systems(
                OnEnter(ClientState::CharSelect),
                (screen::enter_select, stamp_select_entry),
            )
            .add_systems(
                OnExit(ClientState::CharSelect),
                (screen::exit_select, clear_select_entry),
            )
            .add_systems(Update, (debug_glue_roundtrip, debug_logout_smoke))
            .add_systems(
                Update,
                (
                    // Runs in every state: the reconnect auto-answer and the logout edge arrive
                    // `InWorld`. `back_on_disconnect` runs after the entry: a session that died
                    // during entry queues both edges into one frame, and the dead one must win.
                    (
                        apply_roster_policy,
                        enter_on_connected,
                        back_on_logout,
                        back_on_login_refused,
                        back_on_disconnect,
                        // Last and ungated: it mirrors the pick the frame ended with, and world
                        // entry is reached from the create screen too.
                        persist_last_character,
                    )
                        .chain(),
                    (
                        screen::materialize_screen,
                        input::select_input,
                        input::rotate_model,
                        debug_select_dialog,
                        dialog::drive_delete_dialog,
                        // Ahead of the dialog driver, so a refused delete shows the frame it lands.
                        delete_result,
                        // The one `GlueDialog`: here, the refused login and the refused delete.
                        crate::glue::dialog::drive_glue_dialog,
                        debug_select_addons,
                        debug_select_walk,
                        addons::drive_addons_panel,
                        refresh::refresh_list,
                        refresh::refresh_banner_and_buttons,
                        refresh::feed_glue_preview,
                        debug_select_shot,
                    )
                        .chain()
                        .before(crate::glue::GlueVisuals)
                        // After the UI tick: the addons panel holds the VM, and a glue screen
                        // has no push the tick must see, so the chain takes the drain side.
                        .after(crate::ui_script::UiInput)
                        .run_if(in_state(ClientState::CharSelect)),
                )
                    .chain()
                    .after(benilla_world::schedule::WorldStage::Net),
            )
            // After the cover's stage, so the raise it reads is this frame's.
            .add_systems(
                Update,
                screen::hide_under_world_cover
                    .after(benilla_world::schedule::WorldStage::Present)
                    .run_if(in_state(ClientState::CharSelect)),
            );
    }
}

// ── The roster and pick policy ──────────────────────────────────────────────────────────────────

/// The account roster and the pick policy's memory.
#[derive(Resource, Default)]
pub(crate) struct Roster {
    pub(super) chars: Vec<Character>,
    /// The selected row, `CharacterSelect.selectedIndex` 0-based. Written only through
    /// [`Roster::select`].
    selected: Option<usize>,
    /// Bumped by every [`Roster::select`], changed row or not: the reference's `SelectCharacter`
    /// zeroes the facing unconditionally (`0x472950` writes `0xb4217c` above its already-built
    /// check), so a roster refresh re-selecting the same index re-squares too. Only a row click is
    /// gated, in the stock Lua ([`Roster::click_row`]).
    pub(super) select_seq: u64,
    /// The guid picked; kept in-world so a reconnect's roster is answered with it, cleared by a
    /// logout so the roster is shown.
    pub(super) pending_pick: Option<u64>,
    /// `WOW_CHAR`: auto-pick this name on the first roster only.
    env_char: Option<String>,
    /// A just-created character's name, whose row gets selected (the reference's
    /// `SELECT_LAST_CHARACTER`, keyed by name to survive the create/enum race).
    just_created: Option<String>,
    /// The realm-list entry this session connected to, refreshed with each roster.
    pub(super) realm: Option<benilla_protocol::RealmInfo>,
    env_read: bool,
}

impl Roster {
    /// A roster with a pick in flight, for tests outside this module.
    #[cfg(test)]
    pub(crate) fn with_pending_pick(chars: Vec<Character>, guid: u64) -> Self {
        Self {
            chars,
            pending_pick: Some(guid),
            ..Self::default()
        }
    }

    /// Note a character the create screen just made, and select its row now: `net::io` emits the
    /// fresh roster before the create result, so no later roster comes. If the row is not in hand
    /// yet, the arm stays for [`apply_roster_policy`].
    pub(crate) fn note_created(&mut self, name: String) {
        self.just_created = Some(name);
        self.select_created_by_name();
    }

    /// Select the armed just-created row by name, disarming only on a hit. Case-insensitive:
    /// vmangos's `normalizePlayerName` turns a typed `ZZBULL` into `Zzbull`.
    fn select_created_by_name(&mut self) {
        let Some(name) = self.just_created.as_deref() else {
            return;
        };
        if let Some(row) = self
            .chars
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
        {
            self.select(Some(row));
            self.just_created = None;
        }
    }

    /// Select a row, the reference's `SelectCharacter`; every selection goes through here so the
    /// facing reset is never skipped.
    pub(super) fn select(&mut self, row: Option<usize>) {
        self.selected = row;
        self.select_seq = self.select_seq.wrapping_add(1);
    }

    /// A click on a row: `CharacterSelectButton_OnClick` selects only when `id` differs from
    /// `selectedIndex` (`CharacterSelect.lua:305-310`, and `OnDoubleClick` at 312-318), so clicking
    /// the current row keeps its dragged facing.
    pub(super) fn click_row(&mut self, row: usize) {
        if self.selected != Some(row) {
            self.select(Some(row));
        }
    }

    pub(super) fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub(super) fn selected_char(&self) -> Option<&Character> {
        self.selected.and_then(|i| self.chars.get(i))
    }

    /// The pending pick's map, for the loading screen's art before `SMSG_LOGIN_VERIFY_WORLD`.
    pub(crate) fn pending_map(&self) -> Option<u32> {
        self.pending_row().map(|c| c.map)
    }

    /// The pending pick's level: the reference suppresses the loading tip when it arrived as 0 in
    /// `SMSG_CHAR_ENUM` (`0x5b42a0` sets `[selChar+0x10a]`); vmangos always sends a real level.
    pub(crate) fn pending_level(&self) -> Option<u8> {
        self.pending_row().map(|c| c.level)
    }

    /// The picked character's `(map, wow xyz)`, a round trip before `SMSG_LOGIN_VERIFY_WORLD`; the
    /// streamers aim at it during world entry.
    pub(crate) fn pending_entry(&self) -> Option<(u32, [f32; 3])> {
        self.pending_row()
            .map(|c| (c.map, [c.position.x, c.position.y, c.position.z]))
    }

    /// The roster row for the pick in flight: the only description of the character until the
    /// self descriptor streams in, so the in-game UI seats `UnitName("player")` from it.
    pub(crate) fn pending_row(&self) -> Option<&Character> {
        self.pending_pick
            .and_then(|g| self.chars.iter().find(|c| c.guid == g))
    }

    /// The roster position of the pick in flight, by guid rather than the live selection: what
    /// the reference persists as `lastCharacterIndex` at Enter World.
    pub(super) fn pending_index(&self) -> Option<usize> {
        let guid = self.pending_pick?;
        self.chars.iter().position(|c| c.guid == guid)
    }
}

// ── The remembered character (`lastCharacterIndex`) ───────────────────────────────

/// The 1.12 CVar the select screen remembers by: registered at `0x402d93` (name `0x82e8f8`,
/// default `"0"`, pointer at `[0x882674]`), written engine-side, named by no GlueXML.
pub(crate) const CVAR_LAST_CHARACTER: &str = "lastCharacterIndex";

/// The stored index is 0-based, `"0"` the first character: the engine's selection cell
/// `[0x83856c]` printed with `"%d"` (`0x473470` decrements the Lua's 1-based row).
/// `CVar::SaveConfig` (`0x63d980`) skips a value equal to the default, so row 0 writes no line.
fn last_character_value(row: usize) -> String {
    row.to_string()
}

/// The stored value as a row; out of range is the caller's, since the reference does not clamp.
fn last_character_row(value: &str) -> Option<usize> {
    value.trim().parse::<usize>().ok()
}

/// The remembered row for a roster of `len`: out of range falls back to the first row, not the
/// nearest (`CGlueMgr::SetSelectedCharacter`, `0x472740`).
fn remembered_row(cvars: &crate::cvars::Cvars, len: usize) -> usize {
    cvars
        .get(CVAR_LAST_CHARACTER)
        .and_then(last_character_row)
        .filter(|&row| row < len)
        .unwrap_or(0)
}

/// Mirror the character entering the world into [`CVAR_LAST_CHARACTER`], at Enter World, not on
/// a click: `CGlueMgr::EnterWorld` (`0x46b500`) writes the CVar at `0x46b5fa`. The reference
/// flushes `Config.wtf` in the same call (`0x46b6f6`); this reaches disk on exit with every CVar.
fn persist_last_character(
    roster: Res<Roster>,
    mut cvars: ResMut<crate::cvars::Cvars>,
    // The registry outlives every VM, so a row written once stays written.
    mut mirrored: Local<Option<usize>>,
) {
    let Some(row) = roster.pending_index() else {
        return;
    };
    if *mirrored == Some(row) {
        return;
    }
    cvars.set(CVAR_LAST_CHARACTER, &last_character_value(row));
    *mirrored = Some(row);
}

/// Ask the parked IO thread to log in as `guid` and remember it as pending; the probe rig picks
/// through here too, so a reconnect re-answers with its character.
pub(crate) fn send_pick(roster: &mut Roster, pick: &CharPick, guid: u64) {
    roster.pending_pick = Some(guid);
    let _ = pick.0.send(CharRequest::Enter(guid));
}

/// Drain each [`CharListMessage`] into the roster, then answer it with the pending pick
/// (reconnect), the `WOW_CHAR` fast path (first roster only), or nothing, leaving the screen up.
fn apply_roster_policy(
    mut msgs: MessageReader<CharListMessage>,
    mut roster: ResMut<Roster>,
    pick: Res<CharPick>,
    // The remembered row, read off the registry, the store that outlives every VM.
    cvars: Res<crate::cvars::Cvars>,
    // Present only when a rig drives the run and picks its own character.
    rig: Option<Res<crate::run_mode::RigCharacter>>,
    mut exit: MessageWriter<AppExit>,
) {
    if !roster.env_read {
        roster.env_read = true;
        // `WOW_RIG` outranks `WOW_CHAR`: the rig may have to create its character first, which
        // this one-shot fast path cannot wait for.
        roster.env_char = match rig.as_deref() {
            Some(crate::run_mode::RigCharacter(name)) => {
                if let Ok(ignored) = std::env::var("WOW_CHAR") {
                    warn!("char select: WOW_RIG names {name} — ignoring WOW_CHAR={ignored}");
                }
                None
            }
            // `WOW_CHAR`, or the login smoke's optional third field.
            None => std::env::var("WOW_CHAR").ok().or_else(|| {
                std::env::var("WOW_LOGIN_SMOKE")
                    .ok()
                    .and_then(|s| crate::login::smoke_character(&s))
            }),
        };
    }
    for msg in msgs.read() {
        roster.chars = msg.characters.clone();
        roster.realm = msg.realm.clone();
        // A still-armed create selects by name, else the last row: the reference's
        // `SELECT_LAST_CHARACTER` is `SelectCharacter(numChars)`, and vmangos enumerates
        // `ORDER BY create_time, guid`. It outranks the remembered row: the reference pushes the
        // CVar's row (`0x472563`) before `CHARACTER_LIST_UPDATE`, whose `selectLast` overwrites it.
        let created = roster.just_created.is_some();
        if created {
            roster.select_created_by_name();
            if let Some(name) = roster.just_created.take() {
                warn!(
                    "char select: created {name:?} is not on the fresh roster — selecting the last row"
                );
                let last = roster.chars.len().checked_sub(1);
                roster.select(last);
            }
        }
        if roster.chars.is_empty() {
            roster.select(None);
        } else if !created {
            // Re-applied on every roster: the reference reads the CVar in the list rebuild
            // (`0x4724d0` to `0x472740`), not once at startup.
            let row = remembered_row(&cvars, roster.chars.len());
            let who = roster.chars[row].name.clone();
            info!("char select: {CVAR_LAST_CHARACTER} selects row {row} ({who})");
            roster.select(Some(row));
        }
        // `WOW_CHARSELECT_PICK=<name>` selects that row and stays on the screen.
        if let Ok(name) = std::env::var("WOW_CHARSELECT_PICK") {
            match roster
                .chars
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(&name))
            {
                Some(row) => {
                    info!("char select: WOW_CHARSELECT_PICK={name} — selecting row {row}");
                    roster.select(Some(row));
                }
                None => warn!("char select: WOW_CHARSELECT_PICK={name} not on this account"),
            }
        }
        if let Some(guid) = roster.pending_pick {
            // A reconnect, or a pick that raced a dying socket: re-answer without the screen.
            send_pick(&mut roster, &pick, guid);
        } else if let Some(name) = roster.env_char.take() {
            match roster
                .chars
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&name))
            {
                Some(c) => {
                    let guid = c.guid;
                    info!("char select: WOW_CHAR={name} — fast path");
                    send_pick(&mut roster, &pick, guid);
                }
                None => {
                    warn!("char select: WOW_CHAR={name} not on this account — showing roster");
                    // A driverless run cannot pick another row, so it fails rather than idles.
                    if std::env::var_os("WOW_LOGIN_SMOKE").is_none()
                        && crate::run_mode::fatal_when_driverless(&format!(
                            "WOW_CHAR={name} is not on this account"
                        ))
                    {
                        exit.write(AppExit::error());
                    }
                }
            }
        }
    }
}

/// `Connected` ([`EnteredWorldMessage`]): the world owns the session.
fn enter_on_connected(
    mut msgs: MessageReader<EnteredWorldMessage>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if msgs.read().next().is_some() {
        next.set(ClientState::InWorld);
    }
}

/// The session died: back to the login screen, as `GlueParent.lua`'s `DISCONNECTED_FROM_SERVER`
/// does. Chained after [`enter_on_connected`]: a stream dying during entry drains `Connected` and
/// `Disconnected` in one frame, and the dead session must be the last word.
fn back_on_disconnect(
    mut msgs: MessageReader<crate::net::DisconnectedMessage>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if !msgs.read().any(|m| m.session_over) {
        return;
    }
    // With no reconnect coming, a kept pick would auto-answer the next roster.
    roster.pending_pick = None;
    next.set(ClientState::Login);
}

/// `SMSG_CHARACTER_LOGIN_FAILED`: back to select with the refusal in the glue dialog. The IO
/// thread announces the entry with the pick, so this undoes a half-built one. The pick must be
/// cleared, or the relist behind the refusal is auto-answered and the refused login loops.
fn back_on_login_refused(
    mut msgs: MessageReader<CharacterLoginFailedMessage>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
    mut dialog: ResMut<crate::glue::dialog::GlueDialog>,
    strings: Option<Res<GlueStrings>>,
) {
    let Some(msg) = msgs.read().last().copied() else {
        return;
    };
    roster.pending_pick = None;
    next.set(ClientState::CharSelect);
    let empty = GlueStrings::default();
    let strings = strings.as_deref().unwrap_or(&empty);
    dialog.open_error(char_login_refusal_text(strings, msg.result));
}

/// The refusal byte as the reference's words, its jump table `0x5aae08`: the byte is a 1-based
/// reason index (`0x5aad70` does `dec eax; cmp eax,5; ja <default>`), mapping `1..=6` onto status
/// codes `0x3e 0x3f 0x40 0x42 0x43 0x44`, skipping `CHAR_LOGIN_FAILED` (`0x41`), the default arm.
/// vmangos sends `1` for all its refusal guards, so it always reads "World server is down".
pub(crate) fn char_login_refusal_text(strings: &GlueStrings, result: u8) -> &str {
    let (key, fallback): (&str, &str) = match result {
        1 => ("CHAR_LOGIN_NO_WORLD", "World server is down"),
        2 => (
            "CHAR_LOGIN_DUPLICATE_CHARACTER",
            "A character with that name already exists",
        ),
        3 => (
            "CHAR_LOGIN_NO_INSTANCES",
            "No instance servers are available",
        ),
        4 => (
            "CHAR_LOGIN_DISABLED",
            "Login for that race, class, or character is currently disabled.",
        ),
        5 => ("CHAR_LOGIN_NO_CHARACTER", "Character not found"),
        6 => (
            "CHAR_LOGIN_LOCKED_FOR_TRANSFER",
            "Your character is currently locked as part of the paid character transfer process.",
        ),
        // `0` and `7` up: the default arm, the only way to `CHAR_LOGIN_FAILED`.
        _ => ("CHAR_LOGIN_FAILED", "Login failed"),
    };
    strings.text(key, fallback)
}

/// A confirmed `/logout`: back to select, the pick cleared so the next roster is shown.
fn back_on_logout(
    mut msgs: MessageReader<LoggedOutMessage>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if msgs.read().next().is_some() {
        roster.pending_pick = None;
        next.set(ClientState::CharSelect);
    }
}

/// Surface a refused delete in the glue dialog (vmangos refuses a guild master). The reference's
/// `SMSG_CHAR_DELETE` handler (`0x5b45c0`) passes the byte through as the status (`0x5ab0e0`), and
/// on failure `CGlueMgr::Update` (`0x46c14e`) opens the one-button `OKAY` dialog (`0x46c177`
/// pushes `"OKAY"`, `0x837084`) with the key table `0x85cae8`'s text. A success needs nothing:
/// `net::io` re-enumerates first.
fn delete_result(
    mut msgs: MessageReader<CharActionResultMessage>,
    mut dialog: ResMut<crate::glue::dialog::GlueDialog>,
    strings: Option<Res<GlueStrings>>,
) {
    let empty = GlueStrings::default();
    let strings = strings.as_deref().unwrap_or(&empty);
    for msg in msgs.read() {
        if msg.action != CharAction::Delete {
            continue;
        }
        if let Some(text) = char_delete_refusal_text(strings, msg.code) {
            info!(
                "char select: delete refused (code {:#04x}) — {text}",
                msg.code
            );
            dialog.open_error(text);
        }
    }
}

/// A `SMSG_CHAR_DELETE` result byte as the refusal sentence, `None` for success (`0x39`). The keys
/// are the `CHAR_DELETE_*` block (`0x38..=0x3b`) of the status-key table `0x85cae8`, numbered as
/// vmangos's `ResponseCodes`; vmangos sends only `0x39` and `0x3a` (guild master). Any other byte
/// reads `CHAR_DELETE_FAILED`; the reference indexes its whole 83-row table with it, which is not
/// transcribed here, and no server sends one.
pub(crate) fn char_delete_refusal_text(strings: &GlueStrings, code: u8) -> Option<&str> {
    let (key, fallback): (&str, &str) = match code {
        benilla_protocol::messages::CHAR_DELETE_SUCCESS => return None,
        0x38 => ("CHAR_DELETE_IN_PROGRESS", "Deleting character"),
        0x3B => (
            "CHAR_DELETE_FAILED_LOCKED_FOR_TRANSFER",
            "Your character is currently locked as part of the paid character transfer process.",
        ),
        // `0x3a`, the guild-master refusal, and every unnamed byte.
        _ => ("CHAR_DELETE_FAILED", "Character deletion failed"),
    };
    Some(strings.text(key, fallback))
}

/// Glue-flow smoke (`WOW_GLUE_ROUNDTRIP=1`): once a roster is up, go to CharCreate, Back to
/// CharSelect, and exit. Ungated, since it crosses states.
fn debug_glue_roundtrip(
    roster: Res<Roster>,
    state: Res<State<ClientState>>,
    mut next: ResMut<NextState<ClientState>>,
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
    mut phase: Local<u8>,
    mut mark: Local<f32>,
) {
    if std::env::var("WOW_GLUE_ROUNDTRIP").is_err() {
        return;
    }
    let now = time.elapsed_secs();
    match *phase {
        0 if !roster.chars.is_empty() && *state.get() == ClientState::CharSelect => {
            info!(
                "glue-roundtrip: initial roster = {} char(s) → entering CharCreate",
                roster.chars.len()
            );
            next.set(ClientState::CharCreate);
            (*phase, *mark) = (1, now);
        }
        1 if *state.get() == ClientState::CharCreate && now - *mark > 1.5 => {
            info!("glue-roundtrip: in CharCreate → Back to CharSelect");
            next.set(ClientState::CharSelect);
            (*phase, *mark) = (2, now);
        }
        2 if *state.get() == ClientState::CharSelect && now - *mark > 1.5 => {
            info!(
                "glue-roundtrip: back at CharSelect, roster = {} char(s) — done",
                roster.chars.len()
            );
            exit.write(AppExit::Success);
            *phase = 3;
        }
        _ => {}
    }
}

/// The logout smoke (`WOW_LOGOUT_SMOKE=1`, with `WOW_CHAR`): once in the world, `/logout`, return
/// to CharSelect, enter the world again and close the window. The second entry streams a map
/// `terrain_stream::release_world` tore down. It ends by a window close, not an `AppExit`, since
/// only the close reaches the shutdown tail that saves variables and addon files.
fn debug_logout_smoke(
    state: Res<State<ClientState>>,
    player: Res<crate::player::Player>,
    commands: Res<crate::net::NetCommands>,
    mut roster: ResMut<Roster>,
    pick: Res<CharPick>,
    streamer: Res<benilla_world::terrain_stream::TerrainStreamer>,
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
    mut close: MessageWriter<bevy::window::WindowCloseRequested>,
    windows: Query<Entity, With<bevy::window::PrimaryWindow>>,
    mut phase: Local<u8>,
    mut mark: Local<f32>,
) {
    if std::env::var("WOW_LOGOUT_SMOKE").is_err() {
        return;
    }
    let now = time.elapsed_secs();
    match *phase {
        0 if *state.get() == ClientState::InWorld && player.active => {
            info!("logout-smoke: seated in world — lingering");
            (*phase, *mark) = (1, now);
        }
        1 if now - *mark > 3.0 => {
            info!("logout-smoke: requesting logout");
            let _ = commands.0.send(crate::net::ClientCommand::Logout);
            *phase = 2;
        }
        2 if *state.get() == ClientState::CharSelect => {
            info!("logout-smoke: back at character select — lingering");
            (*phase, *mark) = (3, now);
        }
        // Residency is read here, not on arrival: `release_world` runs about 150 ms after the
        // state edge, on the world-live falling edge.
        3 if now - *mark > 4.0 => match roster.chars.first().map(|c| (c.guid, c.name.clone())) {
            Some((guid, name)) => {
                info!(
                    "logout-smoke: {} tiles resident after the release (must be 0)",
                    streamer.residency().1
                );
                info!("logout-smoke: re-entering the world as {name}");
                send_pick(&mut roster, &pick, guid);
                (*phase, *mark) = (4, now);
            }
            None => {
                warn!("logout-smoke: empty roster — cannot test re-entry");
                *phase = 5;
            }
        },
        4 if *state.get() == ClientState::InWorld && player.active && now - *mark > 3.0 => {
            // The suppressor reading catches a character that re-entered unable to move;
            // `scripts/smoke.sh` fails on anything but `none`. A GM account's logout is instant,
            // not rooted, so it does not exercise the rooted path.
            info!(
                "logout-smoke: re-entered — {} tiles resident, suppressors: {}, done",
                streamer.residency().1,
                player.movement_suppressors()
            );
            *phase = 5;
        }
        5 => {
            info!("logout-smoke: done");
            // Close the window as a player does; a headless run has none and exits directly.
            match windows.single() {
                Ok(window) => {
                    close.write(bevy::window::WindowCloseRequested { window });
                }
                Err(_) => {
                    warn!("logout-smoke: no primary window — exiting by AppExit instead");
                    exit.write(AppExit::Success);
                }
            }
            *phase = 6;
        }
        _ => {}
    }
}

/// The select walk (`WOW_CHARSELECT_WALK=<period_s>[:<enter_after_s>]`): select each row in turn,
/// one per `period_s`, logging each step with its time; with `enter_after_s`, press Enter World
/// that long after the last step.
fn debug_select_walk(
    mut roster: ResMut<Roster>,
    pick: Res<CharPick>,
    time: Res<Time>,
    mut plan: Local<Option<(f32, Option<f32>)>>,
    mut next_at: Local<Option<f32>>,
    mut row: Local<usize>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let (period, enter_after) = *plan.get_or_insert_with(|| {
        let Ok(spec) = std::env::var("WOW_CHARSELECT_WALK") else {
            return (f32::INFINITY, None);
        };
        let (p, e) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
        (
            p.trim().parse::<f32>().unwrap_or(3.0),
            e.trim().parse::<f32>().ok(),
        )
    });
    if !period.is_finite() {
        *done = true;
        return;
    }
    if roster.chars.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    let at = *next_at.get_or_insert(now + period);
    if now < at {
        return;
    }
    if *row < roster.chars.len() {
        let i = *row;
        *row += 1;
        let c = &roster.chars[i];
        info!(
            "char select: WALK t={now:.3} → row {i} ({}, race {}, class {})",
            c.name, c.race, c.class
        );
        roster.select(Some(i));
        // The last row waits `enter_after`, so the entry starts from a settled screen.
        *next_at = Some(
            now + if *row == roster.chars.len() {
                enter_after.unwrap_or(period)
            } else {
                period
            },
        );
        return;
    }
    *done = true;
    if enter_after.is_none() {
        return;
    }
    if let Some((guid, name)) = roster.selected_char().map(|c| (c.guid, c.name.clone())) {
        info!("char select: WALK t={now:.3} → ENTER WORLD as {name}");
        send_pick(&mut roster, &pick, guid);
    }
}

/// When the select screen came up, present only while it is; the screen's timed instruments
/// measure from it.
#[derive(Resource, Clone, Copy)]
struct CharSelectEnteredAt(f32);

impl CharSelectEnteredAt {
    fn elapsed(&self, time: &Time) -> f32 {
        time.elapsed_secs() - self.0
    }
}

fn stamp_select_entry(mut commands: Commands, time: Res<Time>) {
    commands.insert_resource(CharSelectEnteredAt(time.elapsed_secs()));
}

fn clear_select_entry(mut commands: Commands) {
    commands.remove_resource::<CharSelectEnteredAt>();
}

/// `WOW_CHARSELECT_DIALOG=<typed>`: open the delete dialog for the selection 4 s after the screen
/// is up, with `<typed>` pre-typed, for a shot.
fn debug_select_dialog(
    roster: Res<Roster>,
    mut dialog: ResMut<dialog::DeleteDialog>,
    time: Res<Time>,
    entered_at: Res<CharSelectEnteredAt>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Ok(typed) = std::env::var("WOW_CHARSELECT_DIALOG") else {
        *done = true;
        return;
    };
    if entered_at.elapsed(&time) < 4.0 {
        return;
    }
    let Some(c) = roster.selected_char() else {
        return;
    };
    let (guid, name, level, class) = (c.guid, c.name.clone(), c.level, class_name(c.class));
    dialog.open_for(guid, name, level, class);
    dialog.typed.set_text(&typed);
    info!("char select: dialog instrument opened the delete confirm");
    *done = true;
}

/// `WOW_CHARSELECT_ADDONS=1`: open the AddOns panel 4 s after the screen is up, as its button
/// does, for a shot.
fn debug_select_addons(
    roster: Res<Roster>,
    mut panel: ResMut<addons::AddonsPanel>,
    time: Res<Time>,
    entered_at: Res<CharSelectEnteredAt>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    if std::env::var("WOW_CHARSELECT_ADDONS").is_err() {
        *done = true;
        return;
    }
    if entered_at.elapsed(&time) < 4.0 {
        return;
    }
    if roster.chars.is_empty() {
        return;
    }
    let realm = roster
        .realm
        .as_ref()
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "Realm".into());
    let chars = roster.chars.iter().map(|c| c.name.clone()).collect();
    panel.open_for(realm, chars);
    info!("char select: addons instrument opened the AddOn List");
    *done = true;
}

/// Default delay of the select shot, seconds from the screen coming up.
const SELECT_SHOT_AT: f32 = 8.0;

/// The select shot (`WOW_CHARSELECT_SHOT_OUT=<path>`): one PNG of the window by framebuffer
/// readback, [`SELECT_SHOT_AT`] seconds after the screen is up, or `WOW_CHARSELECT_SHOT_AT`.
fn debug_select_shot(
    mut commands: Commands,
    time: Res<Time>,
    entered_at: Res<CharSelectEnteredAt>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Ok(out) = std::env::var("WOW_CHARSELECT_SHOT_OUT") else {
        *done = true;
        return;
    };
    let at = std::env::var("WOW_CHARSELECT_SHOT_AT")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(SELECT_SHOT_AT);
    if entered_at.elapsed(&time) < at {
        return;
    }
    use bevy::render::view::screenshot::{save_to_disk, Screenshot};
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(out.clone()));
    info!("char select: shot instrument writing {out}");
    *done = true;
}

/// The WoW UI font, straight off the patch chain (Bevy's TTF loader over the `mpq://` source).
pub(crate) fn wow_font(assets: &AssetServer) -> Handle<Font> {
    assets.load("mpq://Fonts/FRIZQT__.ttf")
}

// ── 1.12 display names ──────────────────────────────────────────────────────────────────────────

pub(crate) fn race_name(race: u8) -> &'static str {
    match race {
        1 => "Human",
        2 => "Orc",
        3 => "Dwarf",
        4 => "Night Elf",
        5 => "Undead",
        6 => "Tauren",
        7 => "Gnome",
        8 => "Troll",
        _ => "?",
    }
}

pub(crate) fn class_name(class: u8) -> &'static str {
    match class {
        1 => "Warrior",
        2 => "Paladin",
        3 => "Hunter",
        4 => "Rogue",
        5 => "Priest",
        7 => "Shaman",
        8 => "Mage",
        9 => "Warlock",
        11 => "Druid",
        _ => "?",
    }
}

/// A `Character` with only its identity filled in.
#[cfg(test)]
pub(crate) fn test_character(guid: u64, name: &str) -> Character {
    Character {
        guid,
        name: name.to_string(),
        race: 1,
        class: 1,
        gender: 0,
        skin: 0,
        face: 0,
        hair_style: 0,
        hair_color: 0,
        facial_hair: 0,
        level: 1,
        zone: 0,
        map: 0,
        position: benilla_protocol::wire::Vector3d {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        flags: 0,
        equipment: [benilla_protocol::CharEnumItem::default(); 19],
        pet_display_id: 0,
        pet_level: 0,
        pet_family: 0,
    }
}

/// The roster policy and its CVar mirror without the screen, for tests over a real CVar host.
#[cfg(test)]
pub(crate) fn add_test_systems(app: &mut App, pick: crossbeam_channel::Sender<CharRequest>) {
    app.insert_state(ClientState::CharSelect)
        .init_resource::<Roster>()
        .insert_resource(CharPick(pick))
        .add_message::<CharListMessage>()
        .add_systems(
            Update,
            (apply_roster_policy, persist_last_character).chain(),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::test_character as character;

    /// A socket dying mid-entry (a displacement kick) drains `Connected` and `Disconnected` in one
    /// frame; the ordering alone makes the disconnect win.
    #[test]
    fn a_session_lost_during_entry_beats_the_entry() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::CharSelect)
            .init_resource::<Roster>()
            .init_resource::<crate::ui_script::PlayerUiHover>()
            .init_resource::<crate::ui_script::UiKeyboardCapture>()
            .add_message::<EnteredWorldMessage>()
            .add_message::<crate::net::DisconnectedMessage>()
            .add_systems(Update, (enter_on_connected, back_on_disconnect).chain());
        app.world_mut().resource_mut::<Roster>().pending_pick = Some(7);

        app.world_mut().write_message(EnteredWorldMessage {
            billing_time_rested: 0,
            tutorial_flags: None,
        });
        app.world_mut()
            .write_message(crate::net::DisconnectedMessage {
                reason: "disconnected: world stream closed: failed to fill whole buffer".into(),
                end: benilla_protocol::SessionEnd::Lost,
                session_over: true,
            });
        app.update();
        app.update(); // `StateTransition` applies the pending state at the next frame

        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::Login,
            "the dead session must be the last word — the reference's DISCONNECTED_FROM_SERVER \
             puts the client back on the account screen, not into a world with no session",
        );
        assert_eq!(
            app.world().resource::<Roster>().pending_pick,
            None,
            "and the reconnect's pending pick goes with it, or the next roster walks straight \
             back into the world the player was just thrown out of",
        );
    }

    /// A teardown whose session is not over (a logout, or a reconnect) still enters the world.
    #[test]
    fn a_teardown_that_is_not_the_end_leaves_the_entry_alone() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::CharSelect)
            .init_resource::<Roster>()
            .init_resource::<crate::ui_script::PlayerUiHover>()
            .init_resource::<crate::ui_script::UiKeyboardCapture>()
            .add_message::<EnteredWorldMessage>()
            .add_message::<crate::net::DisconnectedMessage>()
            .add_systems(Update, (enter_on_connected, back_on_disconnect).chain());

        app.world_mut().write_message(EnteredWorldMessage {
            billing_time_rested: 0,
            tutorial_flags: None,
        });
        app.world_mut()
            .write_message(crate::net::DisconnectedMessage {
                reason: "logged out".into(),
                end: benilla_protocol::SessionEnd::LoggedOut,
                session_over: false,
            });
        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::InWorld,
        );
    }

    /// `Connected` is emitted with `CMSG_PLAYER_LOGIN`, and vmangos refuses before touching the
    /// database, so the refusal lands in the same drain and must be the last word.
    #[test]
    fn a_refused_login_beats_the_entry_it_revokes() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(ClientState::CharSelect)
            .init_resource::<Roster>()
            .init_resource::<crate::ui_script::PlayerUiHover>()
            .init_resource::<crate::ui_script::UiKeyboardCapture>()
            .add_message::<EnteredWorldMessage>()
            .add_message::<CharacterLoginFailedMessage>()
            .init_resource::<crate::glue::dialog::GlueDialog>()
            .add_systems(Update, (enter_on_connected, back_on_login_refused).chain());
        app.world_mut().resource_mut::<Roster>().pending_pick = Some(7);

        app.world_mut().write_message(EnteredWorldMessage {
            billing_time_rested: 0,
            tutorial_flags: None,
        });
        app.world_mut()
            .write_message(CharacterLoginFailedMessage { result: 0x01 });
        app.update();
        app.update(); // `StateTransition` applies the pending state at the next frame

        assert_eq!(
            *app.world().resource::<State<ClientState>>().get(),
            ClientState::CharSelect,
            "the refusal must win — the reference leaves the player on the select screen it \
             never actually took them off",
        );
        assert_eq!(
            app.world().resource::<Roster>().pending_pick,
            None,
            "and the pick goes with it: the relist behind the refusal is auto-answered with \
             `pending_pick`, so keeping it would re-enter the character just refused, forever",
        );
        // No GlueStrings here, so the fallback literal; the shipped one is checked below.
        assert_eq!(
            app.world()
                .resource::<crate::glue::dialog::GlueDialog>()
                .text,
            "World server is down",
            "a refusal the player cannot see is the bug this whole path exists to end",
        );
    }

    /// Every refusal byte resolves to the sentence 1.12 ships, off the player's chain. `4` is
    /// `CHAR_LOGIN_DISABLED`: the reference's table skips `CHAR_LOGIN_FAILED` (0x41).
    #[test]
    fn every_refusal_byte_resolves_in_the_real_glue_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let strings = crate::glue_strings::table_from_chain(&mut chain);

        assert_eq!(char_login_refusal_text(&strings, 1), "World server is down");
        assert_eq!(
            char_login_refusal_text(&strings, 4),
            "Login for that race, class, or character is currently disabled.",
        );
        assert_eq!(char_login_refusal_text(&strings, 5), "Character not found");

        // Both ends of the reference's `dec eax; cmp eax,5; ja` guard reach the one default.
        let default = char_login_refusal_text(&strings, 0);
        assert_eq!(default, "Login failed");
        for byte in [7u8, 8, 0x3f, 0x43, 0xff] {
            assert_eq!(char_login_refusal_text(&strings, byte), default);
        }

        // Every named byte must resolve to a real key, or a matching fallback hides a missing one.
        let map = crate::glue_strings::table_from_chain(&mut chain).into_map();
        for key in [
            "CHAR_LOGIN_NO_WORLD",
            "CHAR_LOGIN_DUPLICATE_CHARACTER",
            "CHAR_LOGIN_NO_INSTANCES",
            "CHAR_LOGIN_DISABLED",
            "CHAR_LOGIN_NO_CHARACTER",
            "CHAR_LOGIN_LOCKED_FOR_TRANSFER",
            "CHAR_LOGIN_FAILED",
        ] {
            assert!(map.contains_key(key), "{key} is not in the shipped table");
        }
    }

    /// A refused delete (vmangos's guild-master `CHAR_DELETE_FAILED`, 0x3a) opens the one-button
    /// glue dialog; a success opens nothing.
    #[test]
    fn a_refused_delete_raises_the_glue_dialog() {
        use crate::glue::dialog::{DialogKind, GlueDialog};
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_message::<CharActionResultMessage>()
            .init_resource::<GlueDialog>()
            .add_systems(Update, delete_result);

        app.world_mut().write_message(CharActionResultMessage {
            action: CharAction::Delete,
            code: benilla_protocol::messages::CHAR_DELETE_SUCCESS,
        });
        app.update();
        assert!(
            !app.world().resource::<GlueDialog>().is_open(),
            "a success is answered by the roster, not a dialog"
        );

        app.world_mut().write_message(CharActionResultMessage {
            action: CharAction::Delete,
            code: 0x3A,
        });
        app.update();
        let dialog = app.world().resource::<GlueDialog>();
        assert_eq!(dialog.kind, Some(DialogKind::Error), "the OKAY dialog");
        // No GlueStrings here, so the fallback literal; the shipped one is checked below.
        assert_eq!(dialog.text, "Character deletion failed");
    }

    /// The delete refusals resolve to the sentences 1.12 ships, each off a real key.
    #[test]
    fn every_delete_result_resolves_in_the_real_glue_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let strings = crate::glue_strings::table_from_chain(&mut chain);

        assert_eq!(char_delete_refusal_text(&strings, 0x39), None);
        assert_eq!(
            char_delete_refusal_text(&strings, 0x3A),
            Some("Character deletion failed")
        );
        assert_eq!(
            char_delete_refusal_text(&strings, 0x3B),
            Some(
                "Your character is currently locked as part of the paid character transfer \
                 process."
            )
        );
        let map = strings.into_map();
        for key in [
            "CHAR_DELETE_IN_PROGRESS",
            "CHAR_DELETE_FAILED",
            "CHAR_DELETE_FAILED_LOCKED_FOR_TRANSFER",
        ] {
            assert!(map.contains_key(key), "{key} is not in the shipped table");
        }
    }

    /// A created character is selected against the roster already in hand: `net::io` emits the
    /// fresh roster before the create result.
    #[test]
    fn a_created_character_is_selected_from_the_roster_in_hand() {
        let mut roster = Roster {
            chars: vec![
                character(1, "Kerwind"),
                character(2, "Xero"),
                character(3, "Zzbullone"), // appended by the re-enum
            ],
            selected: Some(0),
            ..default()
        };
        roster.note_created("Zzbullone".to_string());
        assert_eq!(roster.selected, Some(2), "the new row must be selected");
        assert!(roster.just_created.is_none(), "and the arm consumed");
    }

    /// The name key is case-insensitive: vmangos's `normalizePlayerName` recases a created name.
    #[test]
    fn the_created_name_matches_the_servers_normalized_spelling() {
        let mut roster = Roster {
            chars: vec![character(1, "Kerwind"), character(2, "Zzbullone")],
            selected: Some(0),
            ..default()
        };
        roster.note_created("ZZBULLONE".to_string()); // as typed
        assert_eq!(roster.selected, Some(1));
    }

    // ── The remembered character (`lastCharacterIndex`) ───────────────────────────

    /// Run [`apply_roster_policy`] over one roster with `stored` as `lastCharacterIndex` (`None`
    /// for never set); returns the row the screen opens on.
    fn roster_policy_over(
        stored: Option<&str>,
        names: &[&str],
        preselected: Option<usize>,
    ) -> Option<usize> {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Roster>()
            .insert_resource(CharPick(tx))
            .insert_resource(match stored {
                Some(v) => crate::cvars::Cvars::with_value(CVAR_LAST_CHARACTER, v),
                None => crate::cvars::Cvars::default(),
            })
            .add_message::<CharListMessage>()
            .add_message::<AppExit>()
            .add_systems(Update, apply_roster_policy);
        if let Some(row) = preselected {
            app.world_mut().resource_mut::<Roster>().select(Some(row));
        }
        app.world_mut().write_message(CharListMessage {
            characters: names
                .iter()
                .enumerate()
                .map(|(i, n)| character(i as u64 + 1, n))
                .collect(),
            realm: None,
        });
        app.update();
        app.world().resource::<Roster>().selected()
    }

    /// The screen opens on the character last logged in as; the stored value is 0-based.
    #[test]
    fn the_roster_opens_on_the_remembered_character() {
        assert_eq!(
            roster_policy_over(
                Some("2"),
                &["Kerwind", "Xero", "Zzbullone", "Wartwof"],
                None
            ),
            Some(2),
            "a config.toml remembering the third character must select it, not row one",
        );
    }

    /// Nothing stored and `"0"`, the registered default and the first character, both land on
    /// row 0.
    #[test]
    fn nothing_remembered_and_a_stored_zero_are_both_the_first_row() {
        assert_eq!(
            roster_policy_over(None, &["Kerwind", "Xero"], None),
            Some(0)
        );
        assert_eq!(
            roster_policy_over(Some("0"), &["Kerwind", "Xero"], None),
            Some(0),
        );
    }

    /// An out-of-range index goes to the first row, not the nearest (`0x472740`).
    #[test]
    fn a_remembered_row_past_the_end_falls_back_to_the_first() {
        assert_eq!(
            roster_policy_over(Some("9"), &["Kerwind", "Xero"], None),
            Some(0),
            "the reference clamps to 0, not to the last row — `(idx >= count) ? 0 : idx`",
        );
    }

    /// A corrupt value selects the first row.
    #[test]
    fn an_unparseable_remembered_value_is_the_first_row() {
        assert_eq!(
            roster_policy_over(Some("Kerwind"), &["Kerwind", "Xero"], None),
            Some(0),
        );
    }

    /// Every roster re-applies the stored row (the reference reads it in the list rebuild,
    /// `0x4724d0`), so a selection never entered as does not survive a re-enum.
    #[test]
    fn a_re_enumerated_roster_returns_to_the_remembered_character() {
        assert_eq!(
            roster_policy_over(Some("0"), &["Kerwind", "Xero", "Zzbullone"], Some(2)),
            Some(0),
            "row 2 was only ever clicked; the roster rebuild goes back to the remembered row 0",
        );
    }

    /// Clicking the current row keeps the dragged facing (`CharacterSelectButton_OnClick` gates on
    /// `id ~= selectedIndex`); the facing reset rides `select_seq`.
    #[test]
    fn re_clicking_the_selected_row_keeps_the_facing() {
        let mut roster = Roster {
            chars: vec![character(1, "Kerwind"), character(2, "Xero")],
            ..Roster::default()
        };
        roster.click_row(0);
        let squared = roster.select_seq;
        roster.click_row(0);
        assert_eq!(
            roster.select_seq, squared,
            "a click on the row already selected must not re-select — the dragged angle survives",
        );
        roster.click_row(1);
        assert_eq!(
            roster.select_seq,
            squared + 1,
            "…while a click on a DIFFERENT row selects, and squares the character it brings up",
        );
    }

    /// A just-created character outranks the remembered row: `UpdateCharacterList`'s deferred
    /// `selectLast` overwrites the restored index.
    #[test]
    fn a_created_character_outranks_the_remembered_one() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Roster>()
            .insert_resource(CharPick(tx))
            .insert_resource(crate::cvars::Cvars::with_value(CVAR_LAST_CHARACTER, "0"))
            .add_message::<CharListMessage>()
            .add_message::<AppExit>()
            .add_systems(Update, apply_roster_policy);
        // The create result lands before the fresh roster, the order `apply_roster_policy` answers.
        app.world_mut()
            .resource_mut::<Roster>()
            .note_created("Zzbullone".into());
        app.world_mut().write_message(CharListMessage {
            characters: vec![
                character(1, "Kerwind"),
                character(2, "Xero"),
                character(3, "Zzbullone"),
            ],
            realm: None,
        });
        app.update();
        assert_eq!(
            app.world().resource::<Roster>().selected(),
            Some(2),
            "SELECT_LAST_CHARACTER outranks the remembered row 0",
        );
    }

    /// The stored value is 0-based.
    #[test]
    fn the_stored_index_is_zero_based_and_round_trips() {
        for row in [0usize, 1, 4, 9] {
            assert_eq!(last_character_row(&last_character_value(row)), Some(row));
        }
        assert_eq!(
            last_character_value(0),
            "0",
            "row 0 IS the registrar default"
        );
        assert_eq!(
            last_character_row("0"),
            Some(0),
            "0 is the first row, not 'none'"
        );
        assert_eq!(last_character_row(""), None);
    }

    /// The write is at Enter World, not at selection (`0x46b500`).
    #[test]
    fn only_entering_the_world_writes_the_cvar() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Roster>()
            .init_resource::<crate::cvars::Cvars>()
            .insert_resource(CharPick(tx))
            .add_systems(Update, persist_last_character);
        {
            let mut roster = app.world_mut().resource_mut::<Roster>();
            roster.chars = vec![
                character(1, "Kerwind"),
                character(2, "Xero"),
                character(3, "Zz"),
            ];
            roster.select(Some(2));
            roster.select(Some(1));
        }
        app.update();
        assert!(
            !app.world().resource::<crate::cvars::Cvars>().has_events(),
            "a selection alone must NOT be remembered — the reference writes nothing here",
        );

        app.world_mut().resource_mut::<Roster>().pending_pick = Some(2);
        app.update();
        let moved: Vec<(String, String)> = app
            .world_mut()
            .resource_mut::<crate::cvars::Cvars>()
            .take_events()
            .into_iter()
            .map(|e| (e.name, e.new))
            .collect();
        assert_eq!(
            moved,
            vec![(CVAR_LAST_CHARACTER.to_string(), "1".to_string())],
            "guid 2 sits at row 1, and the row is what the registry took",
        );
        // Steady frames stay quiet; the next VM is seeded from the registry.
        app.update();
        let cvars = app.world().resource::<crate::cvars::Cvars>();
        assert!(!cvars.has_events());
        assert!(cvars
            .vm_seed()
            .iter()
            .any(|r| r.name == CVAR_LAST_CHARACTER && r.value == "1"));
    }

    /// Result first, roster after: the arm is answered by the roster policy, and a name not found
    /// falls back to the last row (`SELECT_LAST_CHARACTER`).
    #[test]
    fn an_unanswerable_create_stays_armed() {
        let mut roster = Roster {
            chars: vec![character(1, "Kerwind")],
            selected: Some(0),
            ..default()
        };
        roster.note_created("Zzbulltwo".to_string());
        assert_eq!(
            roster.just_created.as_deref(),
            Some("Zzbulltwo"),
            "no row to select yet — the arm must survive for the roster policy"
        );
        assert_eq!(roster.selected, Some(0), "and nothing is mis-selected");
    }
}
