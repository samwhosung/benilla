//! The account's roster and the pick, as the session stands on them: the roster the parked IO
//! thread emits ([`CharListMessage`]), the run's own answer to a roster (the reconnect's re-answer,
//! `WOW_CHAR`, `WOW_CHARSELECT_PICK`) and the pick that asks the IO thread to enter the world.

use benilla_protocol::Character;
use bevy::prelude::*;

use crate::net::{CharListMessage, CharPick, CharRequest};
use crate::ui_script::glue::{StockGlueProbe, StockVm};

// ── The roster and pick policy ──────────────────────────────────────────────────────────────────

/// What the run's own pick presses on the select screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PickAct {
    /// Select the row and stay on the screen (`WOW_CHARSELECT_PICK`).
    Select,
    /// Select the row and press Enter World (`WOW_CHAR`).
    Enter,
}

/// The run's own pick, by guid, until the select screen stands to take it ([`enter_run_pick`]).
#[derive(Resource, Default)]
pub(crate) struct RunPick(Option<(u64, PickAct)>);

#[cfg(test)]
impl RunPick {
    /// The entry armed for `guid`, as the roster policy arms `WOW_CHAR`.
    pub(crate) fn armed(guid: u64) -> Self {
        RunPick(Some((guid, PickAct::Enter)))
    }

    /// The selection armed for `guid`, as the roster policy arms `WOW_CHARSELECT_PICK`.
    pub(crate) fn selecting(guid: u64) -> Self {
        RunPick(Some((guid, PickAct::Select)))
    }
}

/// The account roster and the pick policy's memory.
#[derive(Resource, Default)]
pub(crate) struct Roster {
    pub(crate) chars: Vec<Character>,
    /// The guid picked; kept in-world so a reconnect's roster is answered with it, cleared by a
    /// logout so the roster is shown.
    pub(crate) pending_pick: Option<u64>,
    /// `WOW_CHAR`: auto-pick this name on the first roster only.
    env_char: Option<String>,
    /// The realm-list entry this session connected to, refreshed with each roster.
    pub(crate) realm: Option<benilla_protocol::RealmInfo>,
    env_read: bool,
    /// The last race whose intro eye was looked up for a new character's entry, with what it found.
    entry_eye: Option<(u8, Option<[f32; 3]>)>,
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

    /// The pending pick's map, for the loading screen's art before `SMSG_LOGIN_VERIFY_WORLD`.
    pub(crate) fn pending_map(&self) -> Option<u32> {
        self.pending_row().map(|c| c.map)
    }

    /// Whether the pending pick's record has its new-character byte (`+0x10a`) set: the list parse
    /// sets it for a wire level of 0 (`0x5b44e8`-`0x5b44f9`), and the reference then enters at the
    /// race's intro eye (`0x46c203`) and picks no loading tip (`0x46b671`); vmangos never sends
    /// level 0.
    pub(crate) fn pending_new_character(&self) -> bool {
        self.pending_row().is_some_and(|c| c.new_character)
    }

    /// The race of a pending new character whose intro eye is yet to be looked up.
    fn unresolved_entry_race(&self) -> Option<u8> {
        let c = self.pending_row().filter(|c| c.new_character)?;
        (self.entry_eye.map(|(race, _)| race) != Some(c.race)).then_some(c.race)
    }

    /// Look up, once per race, the eye `0x46c2c0` writes over a new character's entry position
    /// (`0x46c214`); a miss is kept, as the reference leaves the position and moves on.
    pub(crate) fn resolve_entry_eye(&mut self, eye: impl FnOnce(u8) -> Option<[f32; 3]>) {
        if let Some(race) = self.unresolved_entry_race() {
            self.entry_eye = Some((race, eye(race)));
        }
    }

    /// The picked character's `(map, wow xyz)`, a round trip before `SMSG_LOGIN_VERIFY_WORLD`; the
    /// streamers aim at it during world entry. `ClientInitializeGame` opens the map around the
    /// record's position (`0x46c1ef`-`0x46c236`), or around the race's intro eye for a new
    /// character the lookup found one for.
    pub(crate) fn pending_entry(&self) -> Option<(u32, [f32; 3])> {
        self.pending_row().map(|c| {
            let eye = self
                .entry_eye
                .filter(|(race, _)| c.new_character && *race == c.race)
                .and_then(|(_, eye)| eye);
            (
                c.map,
                eye.unwrap_or([c.position.x, c.position.y, c.position.z]),
            )
        })
    }

    /// The roster row for the pick in flight: the only description of the character until the
    /// self descriptor streams in, so the in-game UI seats `UnitName("player")` from it.
    pub(crate) fn pending_row(&self) -> Option<&Character> {
        self.pending_pick
            .and_then(|g| self.chars.iter().find(|c| c.guid == g))
    }
}

/// The 1.12 CVar the select screen remembers the character entered as by: registered at
/// `0x402d93` (name `0x82e8f8`, default `"0"`, pointer at `[0x882674]`), written by `EnterWorld`
/// (`0x46b5fa`), named by no GlueXML.
pub(crate) const CVAR_LAST_CHARACTER: &str = "lastCharacterIndex";

/// The tables and files `0x46c2c0` reads to find a new character's intro eye.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct IntroEyes<'w> {
    races: Option<Res<'w, crate::chr_classes::ChrRaceTable>>,
    cinematics: Option<Res<'w, crate::cinematic::Cinematics>>,
    assets: Option<Res<'w, benilla_assets::WorldAssets>>,
}

impl IntroEyes<'_> {
    /// The eye the race's intro shot opens on, read off the patch chain on the spot as the
    /// reference loads the shot's model (`0x46c36c`); `None` where the tables or the file are
    /// missing.
    fn eye(&self, race: u8) -> Option<[f32; 3]> {
        use benilla_assets::LockRecover;
        let (races, cinematics, assets) = (
            self.races.as_deref()?,
            self.cinematics.as_deref()?,
            self.assets.as_deref()?,
        );
        let mut chain = assets.chain.lock_recover();
        benilla_formats::first_login_eye(&mut chain, &cinematics.0, &races.0, race)
    }
}

/// Ask the parked IO thread to log in as `guid` and remember it as pending, so a reconnect
/// re-answers with its character. A new character's entry opens at its race's intro eye
/// (`0x46c203`-`0x46c214`), looked up here, where the entry begins.
pub(crate) fn send_pick(roster: &mut Roster, pick: &CharPick, guid: u64, eyes: &IntroEyes) {
    roster.pending_pick = Some(guid);
    roster.resolve_entry_eye(|race| eyes.eye(race));
    let _ = pick.0.send(CharRequest::Enter(guid));
}

/// Drain each [`CharListMessage`] into the roster, then answer it with the pending pick
/// (reconnect), or arm the run's own pick for the select screen to take ([`enter_run_pick`]):
/// `WOW_CHAR` on the first roster only, `WOW_CHARSELECT_PICK` on every one.
pub(super) fn apply_roster_policy(
    mut msgs: MessageReader<CharListMessage>,
    mut roster: ResMut<Roster>,
    pick: Res<CharPick>,
    eyes: IntroEyes,
    mut run_pick: ResMut<RunPick>,
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
            None => std::env::var("WOW_CHAR").ok(),
        };
    }
    for msg in msgs.read() {
        roster.chars = msg.characters.clone();
        roster.realm = msg.realm.clone();
        // `WOW_CHARSELECT_PICK=<name>` selects that row and stays on the screen, through the
        // screen's own selection once it holds this roster.
        if let Ok(name) = std::env::var("WOW_CHARSELECT_PICK") {
            match roster
                .chars
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&name))
            {
                Some(c) => {
                    info!("char select: WOW_CHARSELECT_PICK={name} — selecting it");
                    run_pick.0 = Some((c.guid, PickAct::Select));
                }
                None => warn!("char select: WOW_CHARSELECT_PICK={name} not on this account"),
            }
        }
        if let Some(guid) = roster.pending_pick {
            // A reconnect, or a pick that raced a dying socket: re-answer without the screen.
            send_pick(&mut roster, &pick, guid, &eyes);
        } else if let Some(name) = roster.env_char.take() {
            match roster
                .chars
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&name))
            {
                Some(c) => {
                    info!("char select: WOW_CHAR={name} — fast path, by Enter World");
                    run_pick.0 = Some((c.guid, PickAct::Enter));
                }
                None => {
                    warn!("char select: WOW_CHAR={name} not on this account — showing roster");
                    // A driverless run cannot pick another row, so it fails rather than idles.
                    if crate::run_mode::fatal_when_driverless(&format!(
                        "WOW_CHAR={name} is not on this account"
                    )) {
                        exit.write(AppExit::error());
                    }
                }
            }
        }
    }
}

/// Enter the world as `row` (0-based) on the select screen, the way a player does: the row
/// selected and Enter World pressed (`CharacterSelect.lua`'s
/// `CharacterSelectButton_OnDoubleClick`), whose `EnterWorld` reaches `CGlueMgr::EnterWorld`
/// (`0x46b500`): `lastCharacterIndex` written and saved, state 8, then the pick.
pub(crate) fn press_enter_world(vm: StockVm, row: usize) {
    let n = row + 1;
    vm.press(&format!(
        "CharacterSelect_SelectCharacter({n}) CharSelectEnterWorldButton:Click()"
    ));
}

/// Select `row` (0-based) on the select screen, as a click on it does
/// (`CharacterSelectButton_OnClick` → `CharacterSelect_SelectCharacter`), whose `SelectCharacter`
/// reaches the engine's selection (`0x473470`).
fn press_select(vm: StockVm, row: usize) {
    vm.press(&format!("CharacterSelect_SelectCharacter({})", row + 1));
}

/// The run's own pick ([`RunPick`]): once the select screen stands with the roster loaded into it
/// (`0x4724d0`), the row is selected ([`press_select`]) or pressed into the
/// world through the screen's Enter World ([`press_enter_world`]), never sent past it.
pub(crate) fn enter_run_pick(
    roster: Res<Roster>,
    mut run_pick: ResMut<RunPick>,
    stock: StockGlueProbe,
) {
    let Some((guid, act)) = run_pick.0 else {
        return;
    };
    let Some(vm) = Some(stock.vm()).filter(|vm| vm.frame_visible("CharacterSelect")) else {
        return;
    };
    let Some(row) = roster.chars.iter().position(|c| c.guid == guid) else {
        run_pick.0 = None;
        return;
    };
    let loaded = vm.text(&format!("return (GetCharacterInfo({}))", row + 1));
    if loaded.as_deref() != Some(roster.chars[row].name.as_str()) {
        return;
    }
    run_pick.0 = None;
    match act {
        PickAct::Select => press_select(vm, row),
        PickAct::Enter => press_enter_world(vm, row),
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
        new_character: false,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(mut c: Character, map: u32, xyz: [f32; 3]) -> Character {
        c.map = map;
        c.position = benilla_protocol::wire::Vector3d {
            x: xyz[0],
            y: xyz[1],
            z: xyz[2],
        };
        c
    }

    /// World entry opens the map around the record's position, and for a record with `+0x10a` set
    /// around the race's intro eye instead (`0x46c203`-`0x46c214`, which writes the eye over the
    /// local copy of `+0x44`-`+0x4c`); a record that sets none, or a race the lookup misses,
    /// keeps its own position (`0x46c4e6` returns 0 and leaves it).
    #[test]
    fn a_new_characters_entry_is_the_intro_eye_and_any_other_keeps_its_position() {
        let own = [10.0, 20.0, 30.0];
        let eye = [-5041.0, -824.0, 541.0];
        let mut new = at(test_character(7, "New"), 1, own);
        new.race = 3;
        new.new_character = true;
        let old = at(test_character(8, "Old"), 1, own);

        let mut roster = Roster::with_pending_pick(vec![new.clone(), old.clone()], 7);
        assert_eq!(roster.pending_entry(), Some((1, own)), "before the lookup");
        let mut asked = Vec::new();
        roster.resolve_entry_eye(|race| {
            asked.push(race);
            Some(eye)
        });
        assert_eq!(roster.pending_entry(), Some((1, eye)));
        roster.resolve_entry_eye(|_| panic!("the race is looked up once"));
        assert_eq!(asked, [3]);

        // The pick moving to a record that is not new takes the record's position again.
        roster.pending_pick = Some(8);
        roster.resolve_entry_eye(|_| panic!("a record without +0x10a asks nothing"));
        assert_eq!(roster.pending_entry(), Some((1, own)));

        // A lookup that found nothing leaves the position and is not repeated.
        let mut roster = Roster::with_pending_pick(vec![new], 7);
        roster.resolve_entry_eye(|_| None);
        assert_eq!(roster.pending_entry(), Some((1, own)));
        roster.resolve_entry_eye(|_| panic!("a miss is not retried"));
    }

    /// The pick the roster policy re-answers after a reconnect begins the entry, so a new
    /// character's intro eye is asked for there as it is for a pick from the screen
    /// ([`send_pick`]); with no tables to answer, the lookup is a miss and the position stands.
    #[test]
    fn a_re_answered_pick_of_a_new_character_looks_up_its_eye() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut new = test_character(7, "Seven");
        new.race = 3;
        new.new_character = true;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(Roster::with_pending_pick(Vec::new(), 7).env_settled())
            .insert_resource(CharPick(tx.into()))
            .init_resource::<RunPick>()
            .add_message::<CharListMessage>()
            .add_systems(Update, apply_roster_policy);
        app.world_mut().write_message(CharListMessage {
            characters: vec![new],
            realm: None,
            cause: benilla_protocol::RosterCause::Asked,
            epoch: 1,
        });
        app.update();

        assert!(rx.try_recv().is_ok(), "the kept pick is sent again");
        assert_eq!(
            app.world().resource::<Roster>().entry_eye,
            Some((3, None)),
            "the race was looked up once, and found nothing without its tables"
        );
    }

    /// The eye is the race's, found once: another new character of another race looks up its own.
    #[test]
    fn each_race_is_looked_up_for_its_own_eye() {
        let mut gnome = test_character(1, "Gnome");
        gnome.race = 7;
        gnome.new_character = true;
        let mut troll = test_character(2, "Troll");
        troll.race = 8;
        troll.new_character = true;
        let mut roster = Roster::with_pending_pick(vec![gnome, troll], 1);
        roster.resolve_entry_eye(|race| Some([f32::from(race), 0.0, 0.0]));
        assert_eq!(roster.pending_entry().map(|e| e.1[0]), Some(7.0));
        roster.pending_pick = Some(2);
        roster.resolve_entry_eye(|race| Some([f32::from(race), 0.0, 0.0]));
        assert_eq!(roster.pending_entry().map(|e| e.1[0]), Some(8.0));
    }
}
