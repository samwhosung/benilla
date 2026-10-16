//! Character select, the native `CharacterSelect.xml` screen: its look, its input, its dialogs and
//! its instruments. The roster it shows, the pick and the session's edges are
//! [`crate::glue_session`]'s, which serve the stock screens too.

mod addons;
#[cfg(test)]
pub(crate) use addons::okay_after_clicks;
mod dialog;
mod input;
mod refresh;
mod screen;

use benilla_protocol::CharAction;
use bevy::prelude::*;

use crate::client_state::ClientState;
use crate::glue_session::roster::Roster;
use crate::glue_strings::GlueStrings;

use crate::net::{CharActionResultMessage, CharacterLoginFailedMessage};

/// The native character-select screen. All of it runs on the native screens alone: under
/// the stock screens the stock `CharacterSelect` stands in its place, and the session it shows is
/// [`crate::glue_session`]'s.
pub(crate) struct CharSelectPlugin;

impl Plugin for CharSelectPlugin {
    fn build(&self, app: &mut App) {
        use crate::ui_script::glue::native_glue;
        app.init_resource::<dialog::DeleteDialog>()
            .init_resource::<addons::AddonsPanel>()
            .init_resource::<refresh::SelectBackground>()
            .add_systems(
                OnEnter(ClientState::CharSelect),
                (screen::enter_select, stamp_select_entry).run_if(native_glue),
            )
            .add_systems(
                OnExit(ClientState::CharSelect),
                (
                    screen::exit_select,
                    screen::clear_select_scene,
                    clear_select_entry,
                )
                    .run_if(native_glue),
            )
            .add_systems(
                OnEnter(ClientState::InWorld),
                refresh::reset_select_background.run_if(native_glue),
            )
            .add_systems(
                Update,
                (
                    open_login_refusal,
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
                        addons::drive_addons_panel,
                        refresh::refresh_list,
                        refresh::refresh_banner_and_buttons,
                        refresh::feed_glue_preview.in_set(crate::portrait::GluePreviewFeed),
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
                    .run_if(native_glue)
                    // The session's edges first: the state and the pick a screen shows.
                    .after(crate::glue_session::edges::SessionEdges)
                    .after(benilla_world::schedule::WorldStage::Net),
            )
            // After the cover's stage, so the raise it reads is this frame's.
            .add_systems(
                Update,
                screen::hide_under_world_cover
                    .after(benilla_world::schedule::WorldStage::Present)
                    .run_if(in_state(ClientState::CharSelect))
                    .run_if(native_glue),
            );
    }
}

/// `SMSG_CHARACTER_LOGIN_FAILED` in the glue dialog, in any state: the refusal arrives in the
/// world, and the edge that takes the session back to select is [`crate::glue_session::edges`]'s.
/// A refusal the player cannot see is a select screen that does nothing. Under the stock glue the
/// words are its glue manager's (state 9).
fn open_login_refusal(
    mut msgs: MessageReader<CharacterLoginFailedMessage>,
    mut dialog: ResMut<crate::glue::dialog::GlueDialog>,
    strings: Option<Res<GlueStrings>>,
) {
    let Some(msg) = msgs.read().last().copied() else {
        return;
    };
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
    unit_names: crate::ui_unit::NameTables,
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
    let (guid, name, level, class) = (
        c.guid,
        c.name.clone(),
        c.level,
        class_name(unit_names.get(), c.class),
    );
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

/// A race's display name from `ChrRaces.dbc` at the install's locale column; `?` for an id with no
/// row, which the roster's own bytes should not carry.
pub(crate) fn race_name<'a>(names: crate::ui_unit::UnitNames<'a>, race: u8) -> &'a str {
    names.race(race).map_or("?", |(name, _)| name)
}

/// A class's display name from `ChrClasses.dbc` at the install's locale column; `?` for no row.
pub(crate) fn class_name<'a>(names: crate::ui_unit::UnitNames<'a>, class: u8) -> &'a str {
    names.class(class).map_or("?", |(name, _)| name)
}

#[cfg(test)]
mod tests {
    /// The screens' race and class words are the tables' at the install's locale; an id with no
    /// row, or no table, reads `?`.
    #[test]
    fn the_screens_name_race_and_class_from_the_tables() {
        let korean = crate::ui_unit::TestNames::korean();
        assert_eq!(race_name(korean.names(), 5), "언데드");
        assert_eq!(class_name(korean.names(), 11), "드루이드");
        assert_eq!(race_name(korean.names(), 0), "?");
        assert_eq!(class_name(korean.names(), 6), "?");
        assert_eq!(class_name(Default::default(), 1), "?");
    }

    use super::*;

    /// A refused login opens its words in the native glue dialog. Under the stock glue nothing of
    /// this screen runs (the schedule census in `game_plugins`), and the stock screen's own words
    /// are its glue manager's state 9.
    #[test]
    fn a_refusal_opens_the_native_dialog() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<crate::glue::dialog::GlueDialog>()
            .add_message::<CharacterLoginFailedMessage>()
            .add_systems(Update, open_login_refusal);
        app.world_mut()
            .write_message(CharacterLoginFailedMessage { result: 0x01 });
        app.update();
        let dialog = app.world().resource::<crate::glue::dialog::GlueDialog>();
        // No GlueStrings here, so the fallback literal; the shipped one is checked below.
        assert_eq!(
            dialog.text, "World server is down",
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
}
