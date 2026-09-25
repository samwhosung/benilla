//! Character select — the faithful glue screen (superseding 0193 §4's v1 overlay).
//!
//! Owns [`ClientState`], the app's lifecycle state machine: `CharSelect` is the pre-world "glue"
//! layer (the real client's GlueXML universe — login/realm/select screens), `InWorld` is the game.
//! The IO thread parks after the world handshake and emits the account roster
//! ([`CharListMessage`]); the **pick policy** here decides what answers it — the pending pick
//! (seamless reconnect), the `WOW_CHAR` env fast path, or the director's choice on
//! the screen, which opens on whoever they last entered the world as (`lastCharacterIndex`,
//! decision 1622). The pick travels the [`CharPick`] channel; `Connected` flips us `InWorld`;
//! a `/logout` round-trip ([`LoggedOutMessage`]) flips back to select with the pending pick
//! cleared, and a **lost** session flips all the way back to the login screen (
//! the reference's `DISCONNECTED_FROM_SERVER`).
//!
//! The screen is the reference's own arrangement (`CharacterSelect.xml/.lua`, extracted off the
//! patch chain): the fullscreen `UI_<Race>` glue scene with the **selected
//! character standing in it, geared from its enum record** (the glue booth), the right-column
//! character list (realm banner, ten row buttons, Create New Character), Enter World / Back /
//! Delete Character along the bottom, the rotate pair, drag-to-rotate, arrow-key cycling,
//! double-click-to-enter, and the typed-`DELETE` confirm dialog. Art/strings/sounds come off the
//! player's own client data ([`crate::glue`]) — never embedded.
//!
//! Module split: this file (state machine + roster policy + shared display constants),
//! [`screen`] (the authored layout), [`refresh`] (list/banner/booth-feed refresh), [`input`]
//! (clicks, keys, rotation, the flows), [`dialog`] (the delete confirm).

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
    /// Parked pre-logon at the login screen: the IO thread waits for credentials;
    /// [`crate::login`]'s policy decides what answers it (the env fast path, the reconnect
    /// resubmit, or the director's typed submit).
    ///
    /// **The realm list is not one of these.** 0193 planned a `RealmList` variant and 2056 built
    /// it; the reference has no such screen (`GlueParent.lua`'s `GlueScreenInfo` lists every glue
    /// screen and the realm list is not among them — it is a `frameStrata="DIALOG"` frame shown
    /// over whichever screen is up). It is [`crate::realm_select::Realms::shown`] now, and the
    /// variant is gone rather than left unconstructed.
    #[default]
    Login,
    /// Parked at character select: the select screen is up, the IO thread waits for a pick, and
    /// the in-world input surfaces (player controller, FrameXML keyboard) are gated off.
    CharSelect,
    /// The character-creation screen: still parked at select (the IO thread
    /// services create/delete in place), a sibling glue screen. Entered from the select screen's
    /// "Create New Character" button; Back returns to `CharSelect`. In-world input stays gated off
    /// (any `in_state(InWorld)` system is off here, mechanically).
    CharCreate,
    /// A character is in (or entering) the world.
    InWorld,
}

/// **The one "only while in the world" gate**. A system that has no business
/// at a glue screen — an input reader, a world-packet feed, a per-character file watcher — joins
/// this set instead of carrying its own `run_if(in_state(ClientState::InWorld))`: twenty-odd
/// copies of that condition said one thing in twenty-odd places, and the condition a set carries
/// ANDs with a member's own, so a member keeps whatever else it needs (the controller's
/// capture-mode gate, say) and the conversion is exact.
///
/// Configured once, in [`CharSelectPlugin`] — the state's home — for `Update`, the only schedule
/// a member lives in. A member in another schedule means configuring the set *there* as well
/// (a set's conditions are per schedule; an unconfigured set gates nothing), never a fresh
/// `run_if` at the site.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InWorldGated;

/// The character-select subsystem: the state machine + the select screen.
pub(crate) struct CharSelectPlugin {
    /// The screen this session opens on.
    ///
    /// A connected boot starts at [`ClientState::Login`] and the roster's arrival
    /// flips it. A world capture starts straight at [`ClientState::InWorld`] — no net thread, no
    /// picker — so the deterministic scene harness is untouched by the glue layer. A **glue**
    /// capture starts on the glue screen it photographs, which is the reason this is a state and
    /// not the bool it used to be: there turned out to be three answers, not two.
    pub(crate) start: ClientState,
}

/// Mirror the session's screen onto the engine's one-bit "is there a world" fact.
///
/// One writer, not a line at each of the dozen sites that set [`ClientState`] — a mirror a future
/// transition can forget is worse than the coupling it replaced.
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
            // The in-world gate, configured beside the state it reads (see [`InWorldGated`]).
            .configure_sets(Update, InWorldGated.run_if(in_state(ClientState::InWorld)))
            .init_resource::<Roster>()
            // **The engine's world-existence bit** (1160's wire (b)): the session owner is this
            // module, so this module tells the world whether there is one. Ordered ahead of every
            // world stage so the fact and its falling edge are this frame's, not last frame's — which
            // is the whole reason `WorldLive` is a resource and not a mirrored state.
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
                    // The policy + transitions run in BOTH states: the roster auto-answer (reconnect
                    // relogin) happens while `InWorld`, and the logout edge arrives there too.
                    // `back_on_disconnect` LAST: a session that died during entry queues both
                    // edges into one frame, and the dead one has to win.
                    (
                        apply_roster_policy,
                        enter_on_connected,
                        back_on_logout,
                        back_on_login_refused,
                        back_on_disconnect,
                        // LAST, and outside both `run_if`s: it mirrors the pick this frame ended
                        // with, and world entry is reached from the create screen too (1622).
                        persist_last_character,
                    )
                        .chain(),
                    (
                        screen::materialize_screen,
                        input::select_input,
                        input::rotate_model,
                        debug_select_dialog,
                        dialog::drive_delete_dialog,
                        // Ahead of the dialog driver, so a refused delete is on screen the frame
                        // its answer lands rather than the next.
                        delete_result,
                        // The reference's one `GlueDialog` (`crate::glue::dialog`), which on this
                        // screen carries the refused character login and the refused delete. Both
                        // glue screens run the same system over the same resource; what a press
                        // *means* is answered per-screen, and an `Error` — the only kind reachable
                        // here — needs no answer at all.
                        crate::glue::dialog::drive_glue_dialog,
                        // Before the list refresh, and before `select_input` reads a click that
                        // landed on the panel rather than the screen.
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
                        // After the UI tick: one member holds the VM (the addons panel reads
                        // the manifest through it), and every VM holder in `Update` declares
                        // its side of the tick. A glue screen has no push the
                        // tick must see, so the whole chain takes the drain side.
                        .after(crate::ui_script::UiInput)
                        .run_if(in_state(ClientState::CharSelect)),
                )
                    .chain()
                    .after(benilla_world::schedule::WorldStage::Net),
            )
            // AFTER the cover's own stage, so the raise it reads is this frame's — see the
            // system's doc for why the glue must yield the glass the moment the cover goes up.
            .add_systems(
                Update,
                screen::hide_under_world_cover
                    .after(benilla_world::schedule::WorldStage::Present)
                    .run_if(in_state(ClientState::CharSelect)),
            );
    }
}

// ── State: the roster + pick policy ──────────────────────────────────────────────────────────────

/// The account roster + the pick policy's memory. `pending_pick` is the character we asked to log
/// in as — kept while in-world so a reconnect's fresh roster is auto-answered with it (decision
/// 0065's seamless relogin); cleared by a deliberate logout so the roster is *shown* instead.
#[derive(Resource, Default)]
pub(crate) struct Roster {
    pub(super) chars: Vec<Character>,
    /// The selected row (the ref's `CharacterSelect.selectedIndex`, 0-based; `None` = empty list).
    /// A fresh roster clamps it into range and defaults to the first row (the ref's law).
    /// **Write it through [`Roster::select`]**, never directly — see [`Roster::select_seq`].
    selected: Option<usize>,
    /// Bumped by every [`Roster::select`], whether or not the row changed.
    ///
    /// The ref's `SelectCharacter` zeroes the select facing **unconditionally**: `0x472950`'s
    /// `mov ds:0xb4217c, 0` sits one instruction *above* the already-built discriminator, so it
    /// dominates both legs, and the merged tail re-applies it geometrically (1533). A counter
    /// rather than change-detection on `selected`, because the engine re-squares on a
    /// *re*-selection of the same index too — which is exactly what a roster refresh does, calling
    /// it with the index it already holds.
    ///
    /// **The click is the one caller that does not reach it**, and the gate is in the stock Lua
    /// rather than in the engine — see [`Roster::click_row`] and. Of the ten Lua call
    /// sites only the two click handlers are gated, and of `0x472740`'s four C callers two are the
    /// roster teardown passing `-1` (so `0x472950` exits above the reset) — every ungated path
    /// re-squares.
    pub(super) select_seq: u64,
    /// The guid we answered the IO thread with; `Some` = a login is requested/live.
    pub(super) pending_pick: Option<u64>,
    /// `WOW_CHAR`, when explicitly set: auto-pick this name on the FIRST roster (the dev fast
    /// path past the screen). `take()`n once — a later `/logout` shows the screen normally.
    env_char: Option<String>,
    /// A just-created character's name: its row gets selected (the ref's
    /// `SELECT_LAST_CHARACTER`, keyed by name so it survives the create/enum race). Armed by
    /// [`Roster::note_created`], which consumes it against the roster **already in hand** —
    /// [`apply_roster_policy`] only has to answer the other arrival order.
    just_created: Option<String>,
    /// The auth realm-list entry this session connected to (the screen's realm banner);
    /// refreshed with each roster.
    pub(super) realm: Option<benilla_protocol::RealmInfo>,
    /// `WOW_CHAR` read latch (env read once at first policy run).
    env_read: bool,
}

impl Roster {
    /// A roster with a pick already in flight — the state world entry runs in.
    ///
    /// `#[cfg(test)]` and `pub(crate)` because the fields are `pub(super)`: a test outside this
    /// module cannot build one, and `ui_script`'s does exactly that to drive
    /// `seat_from_roster` over a real row rather than a copy of its logic.
    #[cfg(test)]
    pub(crate) fn with_pending_pick(chars: Vec<Character>, guid: u64) -> Self {
        Self {
            chars,
            pending_pick: Some(guid),
            ..Self::default()
        }
    }

    /// Note a character the create screen just made, and select its row.
    ///
    /// **The consume happens here, not on the next roster message**: the IO thread
    /// re-enumerates and emits the fresh roster *before* the create result (`net::io`), so by the
    /// time the result reaches [`crate::char_create`] the new row is normally already in
    /// [`Self::chars`] and [`apply_roster_policy`] has already run for that list. Arming a flag for
    /// "the next roster update" then waited for a message that never arrives again — the select
    /// screen came back on the previously-selected row (reproduced: create a tauren on a
    /// human-first account and the human is still standing there).
    ///
    /// The arm survives when the name isn't in hand yet, so the reverse arrival order (result
    /// first) is still answered by [`apply_roster_policy`].
    pub(crate) fn note_created(&mut self, name: String) {
        self.just_created = Some(name);
        self.select_created_by_name();
    }

    /// Select the armed just-created row **by name**, disarming only on a hit. Case-insensitive:
    /// vmangos normalizes a created name (`normalizePlayerName` — first letter upper, rest lower),
    /// so a typed `ZZBULL` enumerates back as `Zzbull` and an exact compare would miss.
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

    /// Select a row — the ref's `SelectCharacter`. Every selection goes through here so the facing
    /// reset it owes cannot be forgotten at one of the seven call sites (see [`Self::select_seq`]).
    pub(super) fn select(&mut self, row: Option<usize>) {
        self.selected = row;
        self.select_seq = self.select_seq.wrapping_add(1);
    }

    /// A click on a character row — the ref's `CharacterSelectButton_OnClick`, whose entire body is
    /// the gate `if ( id ~= CharacterSelect.selectedIndex ) then CharacterSelect_SelectCharacter(id)`
    /// (shipped `CharacterSelect.lua` l.305-310; `OnDoubleClick`, l.312-318, repeats it verbatim
    /// before entering the world, and the ten `CharSelectCharacterButtonTemplate` buttons carry
    /// `id="1".."10"` 1:1 with the character index, so `id` *is* the row).
    ///
    /// So the row you are already on is never re-selected from a click, and the facing zero
    /// [`Self::select`] owes never fires for it: the angle you dragged the character to survives
    /// clicking it again. Decision 2194, correcting 1533 — the engine function is unconditional as
    /// recorded (`0x472740`), but the click never reaches it: the binding has exactly one live call
    /// site, and the two handlers that lead to it both gate.
    pub(super) fn click_row(&mut self, row: usize) {
        if self.selected != Some(row) {
            self.select(Some(row));
        }
    }

    /// The selected row, if any.
    pub(super) fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The selected character, if any.
    pub(super) fn selected_char(&self) -> Option<&Character> {
        self.selected.and_then(|i| self.chars.get(i))
    }

    /// The pending pick's map — the loading screen resolves the *destination's* art from the
    /// roster at the entry edge, before the server's `SMSG_LOGIN_VERIFY_WORLD` snap lands.
    pub(crate) fn pending_map(&self) -> Option<u32> {
        self.pending_row().map(|c| c.map)
    }

    /// The pending pick's level — the loading screen's tip-of-the-day guard. The
    /// reference keeps a synthesised flag at `[selChar+0x10a]` that `0x5b42a0` sets iff this byte
    /// arrived as `0` in `SMSG_CHAR_ENUM`, and a set flag suppresses the tip. vmangos always sends
    /// a real level, so the arm is unreachable against our server; it is honoured because it costs
    /// one comparison and a different server is free to send a zero.
    pub(crate) fn pending_level(&self) -> Option<u8> {
        self.pending_row().map(|c| c.level)
    }

    /// The picked character's `(map, wow xyz)` — **where the world we are about to load actually
    /// is**, known from the roster row a whole server round-trip before `SMSG_LOGIN_VERIFY_WORLD`
    /// says so. The streamers aim at this during world entry; without it the only
    /// answer available before the snap is the hardcoded Northshire anchor
    /// ([`crate::SPAWN_XY`]), which is a guess that is wrong for every character who isn't a fresh
    /// human — and a wrong guess here does not merely idle, it spends the entry's IO budget
    /// decoding tiles that are dropped a moment later.
    pub(crate) fn pending_entry(&self) -> Option<(u32, [f32; 3])> {
        self.pending_row()
            .map(|c| (c.map, [c.position.x, c.position.y, c.position.z]))
    }

    /// The roster row for the pick in flight.
    ///
    /// `pub(crate)` because this row is the ONLY description of the character that exists during
    /// world entry: `Connected` flips us `InWorld` a whole server round-trip before the self
    /// descriptor streams in, and the in-game UI — plus every addon's file scope — materializes
    /// inside that window. `ui_script::load_ingame_ui_on_world_entry` seats
    /// `UnitName("player")` from here for exactly that reason.
    pub(crate) fn pending_row(&self) -> Option<&Character> {
        self.pending_pick
            .and_then(|g| self.chars.iter().find(|c| c.guid == g))
    }

    /// The **roster position** of the pick in flight — what the reference persists as
    /// `lastCharacterIndex` at Enter World. By guid rather than by the live
    /// selection, so the row it names is the one actually being entered even if the selection has
    /// since moved.
    pub(super) fn pending_index(&self) -> Option<usize> {
        let guid = self.pending_pick?;
        self.chars.iter().position(|c| c.guid == guid)
    }
}

// ── The remembered character (`lastCharacterIndex`) ───────────────────────────────

/// The CVar the select screen remembers you by — a **real 1.12 CVar**
/// (registered at `0x402d93`, name `0x82e8f8`, help "Last character selected", default `"0"`,
/// pointer cached at `[0x882674]`), and written engine-side: no shipped GlueXML names it and the
/// binary never looks it up by name.
pub(crate) const CVAR_LAST_CHARACTER: &str = "lastCharacterIndex";

/// The stored index is **0-based**, and `"0"` is the FIRST character — not "nothing remembered".
///
/// It is the engine's own selection cell `[0x83856c]` printed with `"%d"`: `SelectCharacter`
/// (`0x473470`) takes the glue Lua's 1-based row and `dec`s it, and the event back out
/// (`0x472740`) re-adds the one. So the number on disk is one *less* than the row `CharacterSelect
/// .selectedIndex` counts in, and it lines up with [`Roster::selected`] exactly.
///
/// The registered default being `"0"` is load-bearing rather than incidental: `CVar::SaveConfig`
/// (`0x63d980`) skips any value equal to its default, so a player who last entered on their first
/// character has **no such line in `Config.wtf` at all** — and reads back as row 0 anyway. Ours
/// composes the same way (`cvars::compose_file`), so `config.toml` gets the same shape for the
/// same reason.
fn last_character_value(row: usize) -> String {
    row.to_string()
}

/// The stored value → a row. `None` only for a value that is not a number at all (a hand edit);
/// out-of-range is the *caller's* business, because the reference's answer to it is not a clamp.
fn last_character_row(value: &str) -> Option<usize> {
    value.trim().parse::<usize>().ok()
}

/// The remembered row for a roster of `len` characters — the reference's whole selection rule,
/// `CGlueMgr::SetSelectedCharacter` `0x472740`:
///
/// ```text
/// selected = (stored < 0 || stored >= count) ? 0 : stored
/// ```
///
/// Out of range falls back to the **first** row, never to the nearest one: a character deleted
/// since you last played, or a realm with fewer characters, puts you at the top of the list rather
/// than beside where the old row used to be.
fn remembered_row(cvars: &crate::cvars::Cvars, len: usize) -> usize {
    cvars
        .get(CVAR_LAST_CHARACTER)
        .and_then(last_character_row)
        .filter(|&row| row < len)
        .unwrap_or(0)
}

/// Mirror the character we are **entering the world as** into [`CVAR_LAST_CHARACTER`].
///
/// **At Enter World, not at every click** — which is the difference between "the last character
/// you logged in as" and "the last row you happened to touch", and it is the reference's own
/// seam: `CGlueMgr::EnterWorld` (`0x46b500`) formats `[0x83856c]` into the CVar at `0x46b5fa`,
/// and clicks and arrow keys reach `0x472740` without ever going near it. [`Roster::pending_pick`]
/// is exactly that moment for us, so [`Roster::pending_index`] is what this reads.
///
/// The write is a host write into the registry ([`crate::cvars::Cvars::set`]), which is what
/// persists it and what the VM's mirror learns (2303) — the shape the AddOns panel's force-load
/// box uses from this same screen (1293). **One divergence, stated:** the
/// reference flushes `Config.wtf` synchronously in the same call (`0x46b6f6`), while ours reaches
/// disk on the exit edge with every other CVar (1528) — so a crash between entering the world and
/// quitting loses the memory, where the reference would not. That is the CVar store's shape, not
/// this key's, and changing it is an autosave design (1528's own "what this does NOT fix").
fn persist_last_character(
    roster: Res<Roster>,
    mut cvars: ResMut<crate::cvars::Cvars>,
    // A plain `Local` is honest here since 2303: the registry outlives every VM, so the row
    // written once stays written — the per-VM memo this used to need was betting on the seed.
    mut mirrored: Local<Option<usize>>,
) {
    let Some(row) = roster.pending_index() else {
        return; // nobody is entering the world — nothing to remember
    };
    if *mirrored == Some(row) {
        return;
    }
    cvars.set(CVAR_LAST_CHARACTER, &last_character_value(row));
    *mirrored = Some(row);
}

/// Ask the parked IO thread to log in as `guid` (the pick channel) and remember it as pending.
/// `pub(crate)` for the probe rig, which picks a character it may have had to
/// *create* first — going through here is what keeps `pending_pick` truthful, so 0065's reconnect
/// re-answers with the rigged body rather than showing the roster.
pub(crate) fn send_pick(roster: &mut Roster, pick: &CharPick, guid: u64) {
    roster.pending_pick = Some(guid);
    let _ = pick.0.send(CharRequest::Enter(guid));
}

/// Drain each [`CharListMessage`] into the roster, then decide what answers it: the pending pick
/// (reconnect), the `WOW_CHAR` fast path (first roster only), or nothing — the screen waits for
/// the director. A shown roster clamps the selection into range and defaults it to the first row
/// (the ref's `UpdateCharacterList`).
fn apply_roster_policy(
    mut msgs: MessageReader<CharListMessage>,
    mut roster: ResMut<Roster>,
    pick: Res<CharPick>,
    // The remembered row — read off the registry rather than the VM's table
    // because the registry is the store that outlives every VM (2303), and because reading it
    // here keeps this system send-able. It is consulted exactly once per process: `selected` is
    // `None` only before the first roster lands.
    cvars: Res<crate::cvars::Cvars>,
    // Is the character pick already spoken for? Present only when a rig is driving this run
    // (decision 1174's always-present run fact) — absent in every ordinary run, which is the
    // player answer and the one this screen was written for.
    rig: Option<Res<crate::run_mode::RigCharacter>>,
    mut exit: MessageWriter<AppExit>,
) {
    if !roster.env_read {
        roster.env_read = true;
        // `WOW_RIG` with a body outranks `WOW_CHAR`: the rig picks its own derived character, and
        // it may have to CREATE it first, which this fast path (a one-shot `take()` that gives up
        // on a name it can't find) structurally cannot wait for.
        roster.env_char = match rig.as_deref() {
            Some(crate::run_mode::RigCharacter(name)) => {
                if let Ok(ignored) = std::env::var("WOW_CHAR") {
                    warn!("char select: WOW_RIG names {name} — ignoring WOW_CHAR={ignored}");
                }
                None
            }
            // `WOW_CHAR`, or the login smoke's optional third field: the seat
            // exists because `WOW_CHAR` *was* also an unattended marker, so using it to reach the
            // world switched the very branch a session test was trying to exercise. 1769 severed
            // that — `WOW_CHAR` now only means "skip the click" — and the seat stays as the
            // smoke's own way in. Same fast path either way.
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
        // A still-armed create (the result hasn't landed yet — the reverse of the order `net::io`
        // actually sends in) selects its row here: by name first, else the LAST row, which is the
        // ref's literal `SELECT_LAST_CHARACTER` → `SelectCharacter(numChars)` and lands on the same
        // character regardless — vmangos enumerates `ORDER BY create_time, guid`, so the new one is
        // last. Otherwise clamp into range, first row default.
        // **`SELECT_LAST_CHARACTER` outranks the remembered row** — the reference's own
        // precedence, and the order is why: the C side restores the CVar and pushes it into Lua
        // (`0x472563` → `UPDATE_SELECTED_CHARACTER`) *before* firing `CHARACTER_LIST_UPDATE`, and
        // `UpdateCharacterList`'s deferred `selectLast` flag then overwrites it.
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
            // The remembered character, re-applied on **every** roster — the
            // reference reads the CVar in the char-list rebuild itself (`0x4724d0` → `0x472740`),
            // not once at startup, so the screen always opens on whoever you last entered the
            // world as. Out of range falls back to the first row; see `remembered_row`.
            let row = remembered_row(&cvars, roster.chars.len());
            // Greppable, and it names the character rather than only the index: "the first row
            // happened to be right" and "the memory worked" are the same picture on screen, and
            // this line is the only thing that tells them apart in a log.
            let who = roster.chars[row].name.clone();
            info!("char select: {CVAR_LAST_CHARACTER} selects row {row} ({who})");
            roster.select(Some(row));
        }
        // `WOW_CHARSELECT_PICK=<name>` — **select** that row and stay on the screen, the
        // deliberate opposite of `WOW_CHAR`'s enter-the-world fast path. Without it the screen is
        // only ever reachable on its default first row, which makes every "what does this
        // character look like at select?" check hostage to the account's create order (it cost
        // this session's item-glow verification a run). An unknown name leaves the default.
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
            // We already chose this session (in-world reconnect, or a pick raced a dying socket):
            // re-answer without showing the screen.
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
                    // A driverless run can never pick a different row itself — parked here it
                    // burns its whole wall-clock. Same home as the login arm's verdict, so the
                    // marker a leg runner greps for is written in exactly one place (1371, 1769).
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

/// `Connected` (bridged as [`EnteredWorldMessage`]) → the world owns the session.
fn enter_on_connected(
    mut msgs: MessageReader<EnteredWorldMessage>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if msgs.read().next().is_some() {
        next.set(ClientState::InWorld);
    }
}

/// **The session died** → all the way back to the login screen, which is where the
/// reference's `GlueParent.lua` puts it: `SetGlueScreen("login")` on `DISCONNECTED_FROM_SERVER`,
/// with [`crate::login`] raising the "Disconnected from server" dialog over it.
///
/// Chained **after** [`enter_on_connected`] on purpose. Both edges can land in the same frame — the
/// IO thread emits `Connected` before its read loop starts, so a stream that dies during entry
/// queues `Connected` and `Disconnected` back to back and the app drains both at once. That is the
/// race the report caught: the entry half won, and the client walked into a world whose session had
/// already been torn down — a camera with no avatar, `SelfGuid` set, an entry awaiting a snap that
/// could never arrive. Ordering it last makes the dead session the last word by construction.
fn back_on_disconnect(
    mut msgs: MessageReader<crate::net::DisconnectedMessage>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
) {
    if !msgs.read().any(|m| m.session_over) {
        return;
    }
    // The pending pick is the reconnect's memory. With no reconnect coming it would only
    // auto-answer the *next* roster — sending the player straight back into the world they were
    // just thrown out of, without ever seeing the screen.
    roster.pending_pick = None;
    next.set(ClientState::Login);
}

/// The server **refused** the character we picked (`SMSG_CHARACTER_LOGIN_FAILED`) → back to the
/// glue layer, pick cleared, and the refusal said out loud.
///
/// The entry it undoes was optimistic: the IO thread announces the connection in the same breath
/// as the pick (so the destination's tiles start streaming a round-trip early), and
/// the refusal arrives after. So this runs the logout's transition on a world that was only ever
/// half-built — the cover comes down with it ([`crate::loading_screen`]), and the IO thread's
/// relist puts the roster back underneath.
///
/// **Clearing the pick is the load-bearing half.** `pending_pick` is 0065's reconnect memory, and
/// the relist behind this refusal produces exactly the roster it auto-answers: left set, the
/// client would re-enter the character the server just refused, be refused again, and loop — from
/// behind a black screen, since each pass raises the cover afresh.
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

/// The refusal byte, in the client's own words — the reference's jump table `0x5aae08`,
/// transcribed.
///
/// **The byte is a 1-based reason index, not a status code**, and that is the whole reason this
/// table exists rather than an offset. `ClientServices::OnCharLoginResult 0x5aad70` reads it as
/// `movzx eax,byte[ebp+8]; dec eax; cmp eax,5; ja <default>` and jumps through a six-entry table,
/// which maps `1..=6` onto the `CHAR_LOGIN_*` status codes `0x3e, 0x3f, 0x40, 0x42, 0x43, 0x44` —
/// **skipping `0x41`**, `CHAR_LOGIN_FAILED`, which is reachable only as the default. So the
/// tempting `0x3d + byte` is right for four rows and wrong for the rest, and `0` and anything past
/// `6` are "Login failed" rather than an out-of-bounds read: the `ja` guard precedes the table.
/// (The strings are the shipped `GlueStrings.lua:158-166`, quoted here only as the
/// graceful-absence fallback.)
///
/// Both emulators speak this dialect. vmangos sends a bare `1` for all three of its refusal
/// guards (`PlayerLoading() || GetPlayer() || !guid.IsPlayer()`), so **every** refusal from our
/// own local server reads "World server is down" — the reference would say exactly that too, and
/// saying anything better here would be benilla inventing a diagnosis the client cannot make.
/// mangos-classic's live enum is `CharLoginFailReasons` `0x01..=0x08`; its `ResponseCodes`
/// `CHAR_LOGIN_*` block, whose values would NOT survive this table, is commented out.
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
        // `0`, and `7` up (mangos-classic's `LOCKED_BY_BILLING` and `FAILED` among them): the
        // switch's default arm, which is the only way `CHAR_LOGIN_FAILED` is ever reached.
        _ => ("CHAR_LOGIN_FAILED", "Login failed"),
    };
    strings.text(key, fallback)
}

/// A confirmed `/logout` → back to the glue layer, pick cleared (the follow-up roster must be
/// shown, not auto-answered).
///
/// The in-world UI's input latches are released by [`crate::ui_script::end_ui_session`], on the
/// `OnExit(InWorld)` edge this transition causes: they stick because `feed_ui_input` stops running
/// outside `InWorld`, which is a fact about the edge and not about *why* we left it — so both this
/// path and the disconnect above get it from one place (1290).
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

/// Surface a refused delete in the glue dialog; a success needs nothing here, since `net::io`
/// re-enumerates before the result and the row is already gone.
///
/// **A refusal is reachable, and was silent.** vmangos refuses to delete a guild master
/// (`HandleCharDeleteOpcode`: `GetGuildByLeader` → `CHAR_DELETE_FAILED`), and this used to be a
/// `warn!` under a comment calling the refusal unreachable — so the player pressed Delete, typed
/// DELETE, and watched nothing happen.
///
/// The reference's shape, read off `WoW.exe`: `SMSG_CHAR_DELETE 0x5b45c0` hands the result byte to
/// `0x5ab0e0`, which passes it **straight through** as the operation status
/// (`SetStatus(byte == 0x39, byte)`); `CGlueMgr::Update`'s `screenState == 6` arm (`0x46c14e`)
/// then, on a completed-and-failed operation, fires `OPEN_STATUS_DIALOG("OKAY", text)`
/// (`0x46c177` pushes `"OKAY"` `0x837084`) with the status text the key table `0x85cae8` names —
/// the one-button dialog [`crate::glue::dialog::GlueDialog::open_error`] is.
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

/// A `SMSG_CHAR_DELETE` result byte → the sentence the refusal dialog says, or `None` for the
/// success (`0x39`), which the refreshed roster says instead.
///
/// The keys are the `CHAR_DELETE_*` block of the reference's status-key table `0x85cae8`
/// (`0x38..=0x3b`), numbered exactly as vmangos's `ResponseCodes` (`SharedDefines.h`). vmangos
/// sends only two of them: `0x39` on success and `0x3a` for a guild master; its three other
/// refusals (a character still loaded, one not found, one on another account) send nothing at all.
///
/// **The default arm is benilla's, not the reference's.** The reference indexes the whole 83-row
/// table with the byte, so an out-of-block byte would read some other family's sentence (and one
/// past `0x52` would read `"(%i)"`); that table is not transcribed here, and no server we speak to
/// sends such a byte, so anything unnamed says the block's own generic failure.
pub(crate) fn char_delete_refusal_text(strings: &GlueStrings, code: u8) -> Option<&str> {
    let (key, fallback): (&str, &str) = match code {
        benilla_protocol::messages::CHAR_DELETE_SUCCESS => return None,
        0x38 => ("CHAR_DELETE_IN_PROGRESS", "Deleting character"),
        0x3B => (
            "CHAR_DELETE_FAILED_LOCKED_FOR_TRANSFER",
            "Your character is currently locked as part of the paid character transfer process.",
        ),
        // `0x3a` — the guild-master refusal — and the default arm above.
        _ => ("CHAR_DELETE_FAILED", "Character deletion failed"),
    };
    Some(strings.text(key, fallback))
}

/// Glue-flow smoke (`WOW_GLUE_ROUNDTRIP=1`): once a real roster is up, bounce
/// CharSelect → CharCreate → **Back** → CharSelect and exit — so the return-to-select rebuild is
/// provable headlessly from the logs. Runs ungated (it crosses states); inert without the env.
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

/// The logout-boundary smoke (`WOW_LOGOUT_SMOKE=1`, meant with the `WOW_CHAR` fast path): once
/// seated in the world, linger, request the `/logout` round-trip, confirm the return to
/// CharSelect, linger again (the glue theme should be the only thing audible over the logs'
/// world-teardown lines), **enter the world a second time**, and exit. Inert without the env.
///
/// The re-entry leg is not decoration. Since decision 0777 the world is *released* on the way out
/// (`terrain_stream::release_world`), so the second entry is a materially different code path from
/// the first — it streams a map the streamer has already torn down once — and a teardown that
/// forgets to reset its own bookkeeping fails exactly here and nowhere else. The world-audio
/// boundary this smoke was written for is still checked on the way through.
///
/// **It ends the way a player ends a session** — by closing the window, not by writing an
/// `AppExit` of its own. That is not cosmetic: the close is the *latest*
/// announcement Bevy makes (`exit_on_all_closed` in `PostUpdate`, a frame after the request), so
/// it is the only exit that actually tests whether the shutdown tail is reachable. Exiting by a
/// hand-written `AppExit` from `Update` skipped that question for this smoke's whole life, and the
/// bug it would have caught — every saved variable, every addon file and the camera pose lost on
/// every window close — shipped underneath it.
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
        // **The residency reading belongs HERE, not on arrival.** It used to print on the first
        // frame at CharSelect and report "25 tiles resident (must be 0)" every single run — a
        // permanent, alarming, wrong number. `release_world` runs ~150 ms later (it hangs off the
        // world-live falling edge, not the state edge), so the probe was reading the count before
        // the thing it is checking had happened, and reporting the world it had just left.
        //
        // It cost more than a wrong line: 2277 wrote the reading up as a live contradiction of
        // 0777's release-world claim, on the strength of it reproducing identically on an older
        // commit — which it did, because an instrument that measures too early does that reliably.
        // A number that is always wrong teaches everyone to skip the line (0777's own lesson about
        // tripwires nobody trips, from the other end).
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
            // **The suppressor reading is the point of the second entry, not a decoration on it**.
            // This leg has crossed the boundary on every run since 2277 and
            // could only ever report that it *happened* — tiles, UI rebuilds, error counts — none
            // of which a character who re-entered unable to move would disturb. `scripts/smoke.sh`
            // fails on anything but `none`, and separately reports whether the run's logout was
            // the rooted kind at all: a GM probe gets vmangos's instant path, which is B306's
            // precondition missing, so a green here is not a B306 regression on its own.
            info!(
                "logout-smoke: re-entered — {} tiles resident, suppressors: {}, done",
                streamer.residency().1,
                player.movement_suppressors()
            );
            *phase = 5;
        }
        5 => {
            info!("logout-smoke: done");
            // The player's exit: request the window close and let `bevy_window` take it from
            // there. A run with no window (there is none in the headless harness) has no such
            // path, so it says so and falls back rather than hanging until the timeout.
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

/// The select-screen **walk** instrument (`WOW_CHARSELECT_WALK=<period_s>[:<enter_after_s>]`) —
/// the thing a headless run had no way to do at all: *click a row*. It steps the selection one row
/// per `period_s` through the whole roster (the ref's arrow-key cycle, in code), starting from the
/// row the screen opened on, and logs each step with its elapsed stamp — so a booth/scene timeline
/// (`WOW_BOOTH_LOG=1`, `WOW_STREAM_TRACE`) is attributable to the selection that caused it. With
/// `enter_after_s`, it then presses **Enter World** that long after the last step, which is the
/// only headless way to land on the world-entry edge from a screen that was actually *used*
/// (`WOW_CHAR`'s fast path answers the first roster and never renders a second selection).
///
/// Built for the two reports this file's booth feed owns — the cold-selection model swap and the
/// entry-edge stall — and kept, because every future "what does the screen do when you click"
/// question needs exactly this and nothing more. Inert without the env.
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
        return; // roster not in yet — the walk starts at the first real screen
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
        // The last row waits `enter_after` (not `period`) before the entry, so the screen the
        // stall is measured from is a settled one.
        *next_at = Some(
            now + if *row == roster.chars.len() {
                enter_after.unwrap_or(period)
            } else {
                period
            },
        );
        return;
    }
    // The roster is walked. Either enter the world on the last selection, or stop.
    *done = true;
    if enter_after.is_none() {
        return;
    }
    if let Some((guid, name)) = roster.selected_char().map(|c| (c.guid, c.name.clone())) {
        info!("char select: WALK t={now:.3} → ENTER WORLD as {name}");
        send_pick(&mut roster, &pick, guid);
    }
}

/// When the select screen came up: `Time::elapsed_secs()` at `OnEnter(CharSelect)`, gone again
/// at `OnExit`. The one clock the screen's "a few seconds after the screen is up" instruments
/// ([`debug_select_dialog`], [`debug_select_addons`], [`debug_select_shot`]) measure from, in
/// place of a `Local` each stamped on its own first run. Those three run
/// under `in_state(CharSelect)` in `Update`, so the resource is always there when they read it.
#[derive(Resource, Clone, Copy)]
struct CharSelectEnteredAt(f32);

impl CharSelectEnteredAt {
    /// Seconds the screen has been up.
    fn elapsed(&self, time: &Time) -> f32 {
        time.elapsed_secs() - self.0
    }
}

/// `OnEnter(CharSelect)`: stamp the screen's entry.
fn stamp_select_entry(mut commands: Commands, time: Res<Time>) {
    commands.insert_resource(CharSelectEnteredAt(time.elapsed_secs()));
}

/// `OnExit(CharSelect)`: the screen is down, so is its clock.
fn clear_select_entry(mut commands: Commands) {
    commands.remove_resource::<CharSelectEnteredAt>();
}

/// The shot instrument's delete-dialog dial (`WOW_CHARSELECT_DIALOG=<typed>`): open the
/// typed-confirm dialog for the selected character a few seconds after the screen is up, with
/// `<typed>` pre-typed (may be empty) — so the dialog's geometry (the ChatInputBorder edit box,
/// the caret bar) is capturable headlessly. Pair with `WOW_CHARSELECT_SHOT_OUT`; inert without
/// the env; fires once.
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
        return; // roster not in yet — keep waiting
    };
    let (guid, name, level, class) = (c.guid, c.name.clone(), c.level, class_name(c.class));
    dialog.open_for(guid, name, level, class);
    dialog.typed.set_text(&typed);
    info!("char select: dialog instrument opened the delete confirm");
    *done = true;
}

/// The shot instrument's AddOns dial (`WOW_CHARSELECT_ADDONS=1`): open the AddOns panel a few
/// seconds after the screen is up, exactly as the screen's own button would — so the reference
/// AddonList layout is exercised and capturable headlessly (pair with
/// `WOW_CHARSELECT_SHOT_OUT`). Inert without the env; fires once.
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
        return; // roster not in yet — keep waiting
    }
    // The same open the AddOns button performs (`input::select_input`): realm + full roster.
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

/// Default delay before the select-screen shot fires (seconds from the screen coming up) — long
/// enough for the art, the glue scene and the geared model to settle.
const SELECT_SHOT_AT: f32 = 8.0;

/// The select-screen shot instrument (`WOW_CHARSELECT_SHOT_OUT=<path>`): once the
/// screen has been up a few seconds (art + scene + model settled), write one PNG of the window via
/// Bevy's own framebuffer readback — machine-checkable geometry without macOS screen-recording
/// permission. Inert without the env.
///
/// `WOW_CHARSELECT_SHOT_AT=<secs>` moves the shot (default [`SELECT_SHOT_AT`], measured from the
/// first frame the screen is up). The knob exists because the fixed 8 s lands *inside* the
/// create round-trip when the run is `WOW_CHARCREATE_SHOT` + `WOW_CHARCREATE_NAME`: the B119 retest
/// wants the screen a few seconds AFTER the new character lands, and it captured the frame 19 ms
/// after the create result — before the booth had re-baked the new selection.
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

// ── 1.12 display constants (frozen facts of the build; display only) ────────────────────────────

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

/// A `Character` with only its identity filled in — enough for any test that cares about *which*
/// row, not what stands on the stage. `pub(crate)` so [`crate::cvars`]'s end-to-end round trip can
/// build a roster without a second copy of this list drifting from the real one.
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

/// The roster policy and its CVar mirror, with none of the screen they normally run under — the
/// two systems the remembered selection is made of ([`apply_roster_policy`] reads it,
/// [`persist_last_character`] writes it), wired into a test app.
///
/// `pub(crate)` so [`crate::cvars`] can drive them over a **real** CVar host: the seam where a
/// queued engine write becomes a line in `config.toml` lives entirely in that module, and this
/// module's own tests stub it. `CharSelectPlugin` itself is not usable there — it wants the whole
/// glue screen, its art and its sounds.
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

    /// **A session that dies during world entry does not leave the client in the world**.
    ///
    /// The IO thread emits `Connected` *before* its read loop starts, so a socket that dies mid-
    /// entry — which is exactly what a displacement kick looks like, a bare EOF and nothing else —
    /// queues `Connected` and `Disconnected` back to back, and the drain (`try_iter`) hands the app
    /// both in one frame. Both edges then fire in the same `Update`. Whichever writes `NextState`
    /// last wins, and before this the entry did: the client flipped `InWorld` against a world the
    /// same frame's teardown had already emptied, and stood there in a camera with no avatar.
    ///
    /// The guard is the *ordering*, which nothing else can catch: each system is correct alone, the
    /// build is green either way, and the failure needs two logins racing on one account to
    /// reproduce by hand.
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

        // One frame carrying both halves of the race, in the order the drain produces them.
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

    /// The same frame with the session *not* over (a clean logout's teardown, or an unattended run
    /// keeping 0065's seamless reconnect) still enters the world — the guard above must not have
    /// turned every disconnect into a bounce to the login screen.
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

    /// **A refused character login takes the entry back** — and does it in the same frame the
    /// entry was announced in.
    ///
    /// The IO thread sends `CMSG_PLAYER_LOGIN` and emits `Connected` in the same breath (the
    /// entry's head start), so when the server refuses immediately — which is the
    /// vmangos guard's whole shape: `PlayerLoading() || GetPlayer() || !guid.IsPlayer()` answers
    /// before it touches the database — `Connected` and `SMSG_CHARACTER_LOGIN_FAILED` reach the
    /// app in ONE drain. Both edges then fire in one `Update`, and the refusal has to be the last
    /// word, exactly as a lost session does one test up.
    ///
    /// Without this the client flips `InWorld` against a world the server has just declined to
    /// give it: no snap, no objects, and a loading cover armed for a snap that will never come —
    /// which is the shape the whole fix exists to end.
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
        // And the player is told. No GlueStrings in this App, so this is the fallback literal —
        // the shipped sentence for vmangos's `1` is asserted against the real chain below.
        assert_eq!(
            app.world()
                .resource::<crate::glue::dialog::GlueDialog>()
                .text,
            "World server is down",
            "a refusal the player cannot see is the bug this whole path exists to end",
        );
    }

    /// **Every refusal byte resolves to the sentence 1.12 actually ships**, read off the player's
    /// own chain — the char-create screen's regression (2045/2052) applied to the table this
    /// screen owns. Skips without client data.
    ///
    /// The two rows worth writing down. `1` is the only byte vmangos ever sends, and it reads
    /// "World server is down" — not the "duplicate character" its own guard actually means, which
    /// is the server's dialect and not ours to improve on. And `4` is where the tempting
    /// `0x3d + byte` arithmetic breaks: the reference's table skips `CHAR_LOGIN_FAILED` (0x41),
    /// so `4` is `CHAR_LOGIN_DISABLED` and not the "Login failed" an offset would give.
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

        // The default arm, from both ends of the guard the reference's `dec eax; cmp eax,5; ja`
        // draws — and it must be the same sentence, since there is only one default.
        let default = char_login_refusal_text(&strings, 0);
        assert_eq!(default, "Login failed");
        for byte in [7u8, 8, 0x3f, 0x43, 0xff] {
            assert_eq!(char_login_refusal_text(&strings, byte), default);
        }

        // Every named byte must resolve to a REAL key — a fallback that happens to match would
        // hide a missing key forever.
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

    /// **A refused delete is said, not swallowed.** vmangos refuses to delete a guild master with
    /// `CHAR_DELETE_FAILED` (0x3a); the screen used to `warn!` into the log and show nothing, so
    /// the confirmed delete just silently did not happen. The reference raises its one-button
    /// glue dialog with the `CHAR_DELETE_*` status text (`CGlueMgr::Update`'s `screenState == 6`
    /// arm). A success raises nothing — the refreshed roster is the answer.
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
        // No GlueStrings in this App: the fallback literal. The shipped sentence is asserted
        // against the real chain below.
        assert_eq!(dialog.text, "Character deletion failed");
    }

    /// The delete refusals resolve to the sentences 1.12 ships, off the player's own chain — and
    /// every key named is a REAL key, so a matching fallback cannot hide a missing one. Skips
    /// without client data.
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

    /// B119 — a created character is selected against the roster **already in hand**. `net::io`
    /// re-enumerates and emits the fresh roster BEFORE the create result, so by the time
    /// `note_created` runs the row exists and the roster message is spent; a version that only armed
    /// a flag for "the next roster update" left the previously-selected row standing (reproduced
    /// live: create a tauren on a human-first account, the human stays on the stage).
    #[test]
    fn a_created_character_is_selected_from_the_roster_in_hand() {
        let mut roster = Roster {
            chars: vec![
                character(1, "Kerwind"),
                character(2, "Xero"),
                character(3, "Zzbullone"), // the fresh row, appended by the re-enum
            ],
            selected: Some(0),
            ..default()
        };
        roster.note_created("Zzbullone".to_string());
        assert_eq!(roster.selected, Some(2), "the new row must be selected");
        assert!(roster.just_created.is_none(), "and the arm consumed");
    }

    /// The name key is case-insensitive: vmangos normalizes a created name
    /// (`normalizePlayerName` — first letter upper, rest lower), so the string the create screen
    /// typed is NOT what the enum sends back.
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

    /// Drive the REAL [`apply_roster_policy`] over one roster message, with `config.toml` already
    /// holding `stored` for `lastCharacterIndex` (`None` = a launch that has never entered a
    /// world). Returns the row the screen opens on.
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

    /// **The report**: the screen must open on the character you last logged in as, as the
    /// reference does. The stored value is 0-based — the engine's own selection cell, one less
    /// than the row the glue Lua counts in — so `"2"` IS row 2.
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

    /// A client that has never entered a world stores nothing — and `"0"` is not "nothing", it is
    /// the FIRST character (the registrar default, which is exactly why the key is absent from a
    /// `Config.wtf` whose player last played their first character). Both land on row 0, and it
    /// matters that they land there for the reference's reason.
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

    /// The remembered character was deleted (or this realm simply has fewer): the reference's
    /// `0x472740` sends an out-of-range index to the **first** row — never to the nearest one.
    #[test]
    fn a_remembered_row_past_the_end_falls_back_to_the_first() {
        assert_eq!(
            roster_policy_over(Some("9"), &["Kerwind", "Xero"], None),
            Some(0),
            "the reference clamps to 0, not to the last row — `(idx >= count) ? 0 : idx`",
        );
    }

    /// A hand-edited or corrupt value is not a crash and not a wrong row: the first row.
    #[test]
    fn an_unparseable_remembered_value_is_the_first_row() {
        assert_eq!(
            roster_policy_over(Some("Kerwind"), &["Kerwind", "Xero"], None),
            Some(0),
        );
    }

    /// **Every** roster re-applies the stored row, not just the session's first — the reference
    /// reads the CVar inside the char-list rebuild (`0x4724d0`), so a live selection that was
    /// never entered as does not survive a re-enum. This is the half that makes the memory mean
    /// "who you last **logged in** as" rather than "the last row you clicked".
    #[test]
    fn a_re_enumerated_roster_returns_to_the_remembered_character() {
        assert_eq!(
            roster_policy_over(Some("0"), &["Kerwind", "Xero", "Zzbullone"], Some(2)),
            Some(0),
            "row 2 was only ever clicked; the roster rebuild goes back to the remembered row 0",
        );
    }

    /// **The report**: drag the character round on the select screen, then click the row it is
    /// already standing on, and it snapped square again — the reference keeps the angle.
    /// `CharacterSelectButton_OnClick`'s whole body is `if ( id ~= CharacterSelect.selectedIndex )`,
    /// so that click never reaches the engine's unconditional facing zero at all (2194, correcting
    /// 1533 — which verified the engine function and never asked what calls it). The facing reset
    /// rides `select_seq`, so "did it re-square" is exactly "did the counter move".
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

    /// …but a just-created character still wins, which is the reference's precedence and the
    /// order it comes in: the C side pushes the restored index into Lua first, and
    /// `UpdateCharacterList`'s deferred `selectLast` flag overwrites it (B119 stays fixed).
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
        // The create result landed before the fresh roster (the order `net::io` does NOT use —
        // the one `apply_roster_policy` exists to answer).
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

    /// The base, in the one place it lives, and the value's own spelling for the first character.
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

    /// **The write is at Enter World, not at selection** — the reference's `0x46b500` formats the
    /// CVar from the index it is about to enter as, while clicks and arrow keys reach `0x472740`
    /// and never touch it. So "last logged in", exactly as reported.
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
            roster.select(Some(2)); // clicked around the list…
            roster.select(Some(1));
        }
        app.update();
        assert!(
            !app.world().resource::<crate::cvars::Cvars>().has_events(),
            "a selection alone must NOT be remembered — the reference writes nothing here",
        );

        // …and now Enter World, on the row that was selected.
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
        // Steady frames stay quiet, and the next VM is seeded from the registry — which is what
        // replaced the per-VM memo this system used to carry (1290's property, now held by the
        // store outliving the VM, 2303).
        app.update();
        let cvars = app.world().resource::<crate::cvars::Cvars>();
        assert!(!cvars.has_events());
        assert!(cvars
            .vm_seed()
            .iter()
            .any(|r| r.name == CVAR_LAST_CHARACTER && r.value == "1"));
    }

    /// The reverse arrival order (result first, roster after) stays armed and is answered by the
    /// roster policy — and a name that still can't be found falls back to the LAST row, which is the
    /// ref's literal `SELECT_LAST_CHARACTER` → `SelectCharacter(numChars)` and the same character
    /// anyway (vmangos enumerates `ORDER BY create_time, guid`).
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
