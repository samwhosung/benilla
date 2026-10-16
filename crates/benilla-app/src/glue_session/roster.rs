//! The account's roster and the pick, as both glue screen sets stand on them: the roster the parked
//! IO thread emits ([`CharListMessage`]), the character last entered as (`lastCharacterIndex`),
//! the run's own answer to a roster (the reconnect's re-answer, `WOW_CHAR`, `WOW_RIG`,
//! `WOW_CHARSELECT_PICK`) and the pick that asks the IO thread to enter the world.

use benilla_protocol::Character;
use bevy::prelude::*;

use crate::net::{CharListMessage, CharPick, CharRequest};

// ── The roster and pick policy ──────────────────────────────────────────────────────────────────

/// The account roster and the pick policy's memory.
#[derive(Resource, Default)]
pub(crate) struct Roster {
    pub(crate) chars: Vec<Character>,
    /// The selected row, `CharacterSelect.selectedIndex` 0-based. Written only through
    /// [`Roster::select`].
    selected: Option<usize>,
    /// Bumped by every [`Roster::select`], changed row or not: the reference's `SelectCharacter`
    /// zeroes the facing unconditionally (`0x472950` writes `0xb4217c` above its already-built
    /// check), so a roster refresh re-selecting the same index re-squares too. Only a row click is
    /// gated, in the stock Lua ([`Roster::click_row`]).
    pub(crate) select_seq: u64,
    /// The guid picked; kept in-world so a reconnect's roster is answered with it, cleared by a
    /// logout so the roster is shown.
    pub(crate) pending_pick: Option<u64>,
    /// `WOW_CHAR`: auto-pick this name on the first roster only.
    env_char: Option<String>,
    /// A just-created character's name, whose row gets selected (the reference's
    /// `SELECT_LAST_CHARACTER`, keyed by name to survive the create/enum race).
    just_created: Option<String>,
    /// The realm-list entry this session connected to, refreshed with each roster.
    pub(crate) realm: Option<benilla_protocol::RealmInfo>,
    env_read: bool,
}

impl Roster {
    /// A rename the server accepted, applied to the record with `guid` as the glue's applier
    /// renames its own (`0x4725f0`): the new name and the rename flag clear. The in-world files
    /// are named from this roster ([`crate::ui_macro::identity`]).
    pub(crate) fn rename(&mut self, guid: u64, name: &str) {
        if let Some(c) = self.chars.iter_mut().find(|c| c.guid == guid) {
            c.name = name.to_string();
            c.flags &= !benilla_protocol::CHARACTER_FLAG_RENAME;
        }
    }

    /// A roster the screen holds with no pick, for tests outside this module.
    #[cfg(test)]
    pub(crate) fn with_chars(chars: Vec<Character>) -> Self {
        Self {
            chars,
            ..Self::default()
        }
    }

    /// A roster with a pick in flight, for tests outside this module.
    #[cfg(test)]
    pub(crate) fn with_pending_pick(chars: Vec<Character>, guid: u64) -> Self {
        Self {
            chars,
            pending_pick: Some(guid),
            ..Self::default()
        }
    }

    /// The roster with the run's environment already read, so a test's roster policy answers
    /// with no `WOW_CHAR` of the process's.
    #[cfg(test)]
    pub(crate) fn env_settled(mut self) -> Self {
        self.env_read = true;
        self
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
    pub(crate) fn select(&mut self, row: Option<usize>) {
        self.selected = row;
        self.select_seq = self.select_seq.wrapping_add(1);
    }

    /// A click on a row: `CharacterSelectButton_OnClick` selects only when `id` differs from
    /// `selectedIndex` (`CharacterSelect.lua:305-310`, and `OnDoubleClick` at 312-318), so clicking
    /// the current row keeps its dragged facing.
    pub(crate) fn click_row(&mut self, row: usize) {
        if self.selected != Some(row) {
            self.select(Some(row));
        }
    }

    pub(crate) fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub(crate) fn selected_char(&self) -> Option<&Character> {
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
    pub(crate) fn pending_index(&self) -> Option<usize> {
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
pub(super) fn persist_last_character(
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
pub(super) fn apply_roster_policy(
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
    app.insert_state(crate::client_state::ClientState::CharSelect)
        .init_resource::<Roster>()
        .insert_resource(CharPick(pick.into()))
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
            .insert_resource(CharPick(tx.into()))
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
            cause: benilla_protocol::RosterCause::Park,
            epoch: 0,
            generation: 0,
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
            .insert_resource(CharPick(tx.into()))
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
            cause: benilla_protocol::RosterCause::Park,
            epoch: 0,
            generation: 0,
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
            .insert_resource(CharPick(tx.into()))
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
