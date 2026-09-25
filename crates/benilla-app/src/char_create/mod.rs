//! The character-creation screen: a Bevy-UI overlay laid out like the 1.12 `CharacterCreate.xml`,
//! driving the `"create"` preview booth, with art, captions and sounds read from the player's
//! install. This file holds the selection state, input and the wire.

mod panels;
mod parts;
mod refresh;
mod screen;

use benilla_protocol::{CharAction, CharCreateReq};
use benilla_ui::widget::EditBoxState;
use bevy::input::keyboard::KeyboardInput;

use crate::textinput::{self, HostClipboard};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::input::ButtonState;
use bevy::prelude::*;

use crate::char_select::{ClientState, Roster};
use crate::entities::CharCreate;
use crate::glue_strings::GlueStrings;
use crate::net::{CharActionResultMessage, CharPick, CharRequest};
use crate::portrait::{CreateLook, GlueLook, GluePreview};
use crate::sound::GlueSound;

/// Alliance and Horde race columns, top to bottom: `CharacterCreateEnumerateRaces` fills buttons
/// 1..8 in `GetAvailableRaces()` order and `CharacterCreate.xml` chains 1-4 down the first column,
/// 5-8 down the second, so Alliance then Horde, ascending race id. `RACE_ICON_TCOORDS`'s table
/// order is a name-to-UV lookup and never the layout order.
///
/// The Alliance half is also the race-to-side split `ui_unit::race_faction_group` answers
/// `UnitFactionGroup("player")` with.
pub(crate) const ALLIANCE: [u8; 4] = [1, 3, 4, 7]; // Human, Dwarf, Night Elf, Gnome
const HORDE: [u8; 4] = [2, 5, 6, 8]; // Orc, Scourge, Tauren, Troll
/// The reference's initial facing (`SetCharacterCreateFacing(-15)`), reset on every race switch.
const INITIAL_FACING: f32 = -15.0 * std::f32::consts::PI / 180.0;

/// The character-creation screen and its selection state.
pub(crate) struct CharCreatePlugin;

impl Plugin for CharCreatePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CreateSelection>()
            .add_systems(OnEnter(ClientState::CharCreate), screen::enter_create)
            .add_systems(OnExit(ClientState::CharCreate), screen::exit_create)
            .add_systems(Update, debug_enter)
            .add_systems(
                Update,
                (
                    screen::rescale_screen,
                    create_input,
                    debug_auto_create,
                    debug_pick,
                    debug_shot,
                    rotate_model,
                    refresh::refresh_dynamic,
                    refresh_name_box,
                    refresh::refresh_hover,
                    refresh::scroll_info,
                    refresh::scroll_drive,
                    refresh::scroll_visuals,
                    create_result,
                )
                    .chain()
                    .before(crate::glue::GlueVisuals)
                    .run_if(in_state(ClientState::CharCreate))
                    .after(benilla_world::schedule::WorldStage::Net),
            );
    }
}

/// `WOW_CHARCREATE_NAME=<name>`: a few seconds after the screen is up, fill the name and fire
/// Create once, exercising the whole create path unattended (pair with `WOW_CHARCREATE_SHOT=1`).
fn debug_auto_create(
    mut sel: ResMut<CreateSelection>,
    pick: Res<CharPick>,
    time: Res<Time>,
    mut fired: Local<bool>,
    mut armed_at: Local<Option<f32>>,
) {
    if *fired {
        return;
    }
    let Ok(name) = std::env::var("WOW_CHARCREATE_NAME") else {
        *fired = true;
        return;
    };
    let now = time.elapsed_secs();
    let start = *armed_at.get_or_insert(now);
    if now - start < 6.0 {
        return; // let the model settle and the socket park
    }
    sel.name.set_text(&name);
    sel.creating = true;
    let _ = pick.0.send(CharRequest::Create(sel.request()));
    *fired = true;
    info!("char create: auto-create fired for {:?}", sel.name.text);
}

/// `WOW_CHARCREATE_SHOT=1`: jump to the create screen a few seconds after boot, without a click.
fn debug_enter(
    state: Res<State<ClientState>>,
    mut next: ResMut<NextState<ClientState>>,
    time: Res<Time>,
    mut done: Local<bool>,
) {
    if *done || std::env::var("WOW_CHARCREATE_SHOT").is_err() {
        *done = true;
        return;
    }
    if time.elapsed_secs() > 3.0 && *state.get() == ClientState::CharSelect {
        next.set(ClientState::CharCreate);
        *done = true;
    }
}

/// `WOW_CHARCREATE_PICK="race,sex[,class]"`: applied once the screen is up, after its enter reset;
/// a class the race may not be is ignored.
fn debug_pick(
    state: Res<State<ClientState>>,
    catalog: Option<Res<CharCreate>>,
    mut sel: ResMut<CreateSelection>,
    mut preview: ResMut<GluePreview>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Ok(spec) = std::env::var("WOW_CHARCREATE_PICK") else {
        *done = true;
        return;
    };
    if *state.get() != ClientState::CharCreate {
        return;
    }
    let mut it = spec.split(',').map(|s| s.trim().parse::<u8>().ok());
    sel.race = it.next().flatten().unwrap_or(1).max(1);
    sel.sex = it.next().flatten().unwrap_or(0).min(1);
    sel.clamp(catalog.as_deref());
    if let Some(class) = it.next().flatten() {
        if race_classes(catalog.as_deref(), sel.race).contains(&class) {
            sel.class = class;
        } else {
            warn!(
                "char create: pick instrument class {class} invalid for race {} — keeping {}",
                sel.race, sel.class
            );
        }
    }
    // `WOW_CHARCREATE_DIALS="skin,face,hairStyle,hairColor,facialHair"`: each field optional
    // (missing leaves 0), each clamped into the (race, sex)'s range.
    if let Ok(spec) = std::env::var("WOW_CHARCREATE_DIALS") {
        let counts = dial_counts(catalog.as_deref(), sel.race, sel.sex);
        for (i, field) in spec.split(',').take(5).enumerate() {
            let Some(want) = field.trim().parse::<u8>().ok() else {
                continue;
            };
            let n = counts[i].max(1);
            if want >= n {
                warn!(
                    "char create: dial {i} value {want} past the range ({n}) for race {} sex {} — clamped",
                    sel.race, sel.sex
                );
            }
            sel.dials[i] = want.min(n - 1);
        }
    }
    preview.scene = Some(crate::portrait::GlueScene::Race(sel.race));
    preview.look = Some(GlueLook::Create(sel.look()));
    info!(
        "char create: pick instrument set race {} sex {} class {} dials {:?}",
        sel.race, sel.sex, sel.class, sel.dials
    );
    *done = true;
}

/// `WOW_CHARCREATE_SHOT_OUT=<path>`: once the screen has settled, write one PNG of the window
/// through Bevy's framebuffer readback. Pairs with `WOW_CHARCREATE_SHOT=1`.
fn debug_shot(
    mut commands: Commands,
    time: Res<Time>,
    mut entered_at: Local<Option<f32>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Ok(out) = std::env::var("WOW_CHARCREATE_SHOT_OUT") else {
        *done = true;
        return;
    };
    let start = *entered_at.get_or_insert(time.elapsed_secs());
    if time.elapsed_secs() - start < 5.0 {
        return;
    }
    use bevy::render::view::screenshot::{save_to_disk, Screenshot};
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(out.clone()));
    info!("char create: shot instrument writing {out}");
    *done = true;
}

// ── The selection state ──────────────────────────────────────────────────────────────────────────

/// What the create screen has selected. The dials are `[skin, face, hairStyle, hairColor,
/// facialHair]` indices into [`CharCreate`]'s ranges, re-clamped on every race or gender change;
/// `class` picks the starting outfit the preview wears.
#[derive(Resource, Default)]
pub(crate) struct CreateSelection {
    race: u8,
    sex: u8,
    class: u8,
    dials: [u8; 5],
    /// The typed name; the shared feed enforces letters only and the 12-character cap, pasted too.
    name: EditBoxState,
    /// Waiting on `SMSG_CHAR_CREATE`: Create is disarmed.
    creating: bool,
}

/// The five dial counts for a (race, sex), or `[1; 5]` without the catalog.
fn dial_counts(catalog: Option<&CharCreate>, race: u8, sex: u8) -> [u8; 5] {
    catalog
        .and_then(|c| c.0.ranges(race, sex))
        .map(|r| [r.skin, r.face, r.hair_style, r.hair_color, r.facial_hair])
        .unwrap_or([1; 5])
}

impl CreateSelection {
    /// Reset to Human, male, its first class, dials 0.
    fn reset(&mut self, catalog: Option<&CharCreate>) {
        self.race = 1;
        self.sex = 0;
        self.class = catalog
            .and_then(|c| c.0.classes_for_race(1).first().copied())
            .unwrap_or(1);
        self.dials = [0; 5];
        self.name.set_text("");
        self.creating = false;
    }

    /// Re-clamp class and dials into the current (race, sex)'s valid ranges.
    fn clamp(&mut self, catalog: Option<&CharCreate>) {
        if let Some(c) = catalog {
            if !c.0.allows(self.race, self.class) {
                self.class =
                    c.0.classes_for_race(self.race)
                        .first()
                        .copied()
                        .unwrap_or(1);
            }
        }
        let counts = dial_counts(catalog, self.race, self.sex);
        for (d, &n) in self.dials.iter_mut().zip(&counts) {
            *d = if n == 0 { 0 } else { (*d).min(n - 1) };
        }
    }

    /// The booth's look; the class dresses the (race, class, sex) starting outfit.
    fn look(&self) -> CreateLook {
        CreateLook {
            race: self.race,
            sex: self.sex,
            class: self.class,
            skin: self.dials[0],
            face: self.dials[1],
            hair_style: self.dials[2],
            hair_color: self.dials[3],
            facial_hair: self.dials[4],
        }
    }

    fn request(&self) -> CharCreateReq {
        CharCreateReq {
            name: self.name.text.clone(),
            race: self.race,
            class: self.class,
            gender: self.sex,
            skin: self.dials[0],
            face: self.dials[1],
            hair_style: self.dials[2],
            hair_color: self.dials[3],
            facial_hair: self.dials[4],
        }
    }
}

/// One clickable control on the screen.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum CreateAction {
    Race(u8),
    Gender(u8),
    /// An index into the race's valid classes: the reference lists only those, so the slot's class
    /// shifts per race.
    ClassSlot(u8),
    /// Dial index 0..5, direction ±1.
    Dial(u8, i8),
    Randomize,
    Create,
    Back,
    /// Hold to rotate, like the reference's rotate buttons.
    RotateLeft,
    RotateRight,
    /// The model pane: drag to rotate, no click action.
    Model,
}

/// The classes the race may be, ascending class id: CharBaseInfo order, as `GetClassesForRace`
/// enumerates them.
fn race_classes(catalog: Option<&CharCreate>, race: u8) -> Vec<u8> {
    catalog
        .map(|c| c.0.classes_for_race(race))
        .unwrap_or_else(|| vec![1, 2, 3, 4, 5, 7, 8, 9, 11])
}

/// A class id's file string, the `CLASS_ICON_TCOORDS` key.
fn class_file(class: u8) -> &'static str {
    match class {
        1 => "WARRIOR",
        2 => "PALADIN",
        3 => "HUNTER",
        4 => "ROGUE",
        5 => "PRIEST",
        7 => "SHAMAN",
        8 => "MAGE",
        9 => "WARLOCK",
        11 => "DRUID",
        _ => "WARRIOR",
    }
}

// ── Input ────────────────────────────────────────────────────────────────────────────────────────

/// Clicks, name typing, Enter and Escape. The name caret blinks at the reference's `blinkSpeed`,
/// 0.5 s (`0x77a790`).
fn create_input(
    buttons: Query<(Entity, &CreateAction)>,
    clicks: Res<crate::glue::GlueClicks>,
    mut keyboard: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    catalog: Option<Res<CharCreate>>,
    mut sel: ResMut<CreateSelection>,
    // The host pasteboard and the window handle its Wayland backend needs.
    mut clipboard: NonSendMut<HostClipboard>,
    raw_handle: Query<&bevy::window::RawHandleWrapper, With<bevy::window::PrimaryWindow>>,
    time: Res<Time>,
    mut preview: ResMut<GluePreview>,
    pick: Res<CharPick>,
    mut next: ResMut<NextState<ClientState>>,
    mut sounds: MessageWriter<GlueSound>,
    mut rng: Local<u64>,
) {
    let mods = textinput::mods_now(&keys);
    let wl = textinput::wayland_display(raw_handle.iter().next());
    // The create screen has one field and no focus model, so it is always focused while up.
    textinput::tick_caret(&mut sel.name, true, time.delta_secs());
    let cat = catalog.as_deref();
    let mut changed_look = false;
    let mut do_create = false;

    // A Button fires on the release over the button that took the press (`glue::glue_clicks`).
    for (entity, action) in &buttons {
        if !clicks.hit(entity) {
            continue;
        }
        match *action {
            CreateAction::Race(r) => {
                // The reference always clicks; a real switch resets the class to the race's first
                // and the facing to -15°.
                sounds.write(GlueSound("gsCharacterCreationClass"));
                if sel.race != r {
                    sel.race = r;
                    sel.class = race_classes(cat, r).first().copied().unwrap_or(1);
                    sel.clamp(cat);
                    preview.yaw = INITIAL_FACING;
                    changed_look = true;
                }
            }
            CreateAction::Gender(g) => {
                sounds.write(GlueSound("gsCharacterCreationClass"));
                if sel.sex != g {
                    sel.sex = g;
                    sel.clamp(cat);
                    changed_look = true;
                }
            }
            CreateAction::ClassSlot(slot) => {
                // The reference always clicks and re-dresses only on a real change (`0x470f50`).
                if let Some(&class) = race_classes(cat, sel.race).get(slot as usize) {
                    sounds.write(GlueSound("gsCharacterCreationClass"));
                    if sel.class != class {
                        sel.class = class;
                        changed_look = true;
                    }
                }
            }
            CreateAction::Dial(dial, dir) => {
                sounds.write(GlueSound("gsCharacterCreationLook"));
                cycle_dial(&mut sel, cat, dial as usize, dir);
                changed_look = true;
            }
            CreateAction::Randomize => {
                sounds.write(GlueSound("gsCharacterCreationLook"));
                randomize(&mut sel, cat, &mut rng);
                changed_look = true;
            }
            CreateAction::Create => do_create = true,
            CreateAction::Back => {
                sounds.write(GlueSound("gsCharacterCreationCancel"));
                next.set(ClientState::CharSelect);
            }
            CreateAction::RotateLeft | CreateAction::RotateRight | CreateAction::Model => {}
        }
    }

    for ev in keyboard.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        if textinput::feed_key(
            &mut sel.name,
            ev,
            mods,
            &mut clipboard,
            wl,
            textinput::CharFilter::Letters,
        ) == textinput::FieldKey::Consumed
        {
            continue;
        }
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) {
        do_create = true;
    }
    if keys.just_pressed(KeyCode::Escape) {
        sounds.write(GlueSound("gsCharacterCreationCancel"));
        next.set(ClientState::CharSelect);
    }

    if do_create && !sel.creating {
        sel.creating = true;
        sounds.write(GlueSound("gsCharacterCreationCreateChar"));
        let _ = pick.0.send(CharRequest::Create(sel.request()));
    }
    if changed_look {
        preview.scene = Some(crate::portrait::GlueScene::Race(sel.race));
        preview.look = Some(GlueLook::Create(sel.look()));
    }
}

/// Cycle one dial by `dir`, wrapping within `0..count`.
fn cycle_dial(sel: &mut CreateSelection, catalog: Option<&CharCreate>, dial: usize, dir: i8) {
    let count = dial_counts(catalog, sel.race, sel.sex)[dial].max(1) as i32;
    let cur = sel.dials[dial] as i32;
    sel.dials[dial] = (cur + dir as i32).rem_euclid(count) as u8;
}

/// Set every dial to a random valid index, from a small xorshift.
fn randomize(sel: &mut CreateSelection, catalog: Option<&CharCreate>, rng: &mut u64) {
    let counts = dial_counts(catalog, sel.race, sel.sex);
    for (d, &n) in sel.dials.iter_mut().zip(&counts) {
        *rng ^= *rng << 13;
        *rng ^= *rng >> 7;
        *rng ^= *rng << 17;
        *rng = rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        *d = if n == 0 { 0 } else { (*rng % n as u64) as u8 };
    }
}

/// Paint the name box through the shared `paint_glue_field`, so it draws like the login boxes.
fn refresh_name_box(
    sel: Res<CreateSelection>,
    mut parts: Query<(
        &crate::glue::widgets::GlueFieldPart,
        Option<&mut Text>,
        &mut Visibility,
    )>,
) {
    crate::glue::widgets::paint_glue_field(&sel.name, true, parts.iter_mut());
}

/// Rotate the preview by dragging the model pane (dragging right increases the facing) or holding
/// a rotate button (left decrements it). The drag uses the select screen's constant: the reference
/// declares `CHARACTER_ROTATION_CONSTANT = 0.6` once in `CharacterSelect.lua` for both screens.
fn rotate_model(
    panes: Query<(&Interaction, &CreateAction)>,
    motion: Res<AccumulatedMouseMotion>,
    time: Res<Time>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut preview: ResMut<GluePreview>,
) {
    let window = window.single().ok();
    for (interaction, action) in &panes {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match action {
            CreateAction::Model if motion.delta.x != 0.0 => {
                preview.yaw += crate::glue::drag_yaw(motion.delta.x, window);
            }
            CreateAction::RotateLeft => preview.yaw -= crate::glue::ROTATE_RATE * time.delta_secs(),
            CreateAction::RotateRight => {
                preview.yaw += crate::glue::ROTATE_RATE * time.delta_secs()
            }
            _ => {}
        }
    }
}

// ── Result ───────────────────────────────────────────────────────────────────────────────────────

/// Surface each create result in the status line; a success returns to select with the row armed.
fn create_result(
    mut msgs: MessageReader<CharActionResultMessage>,
    mut sel: ResMut<CreateSelection>,
    mut roster: ResMut<Roster>,
    mut next: ResMut<NextState<ClientState>>,
    mut status: Query<&mut Text, With<parts::StatusLine>>,
    strings: Res<GlueStrings>,
) {
    for msg in msgs.read() {
        if msg.action != CharAction::Create {
            continue;
        }
        sel.creating = false;
        info!(
            "char create: result {:#04x} — {}",
            msg.code,
            char_result_text(&strings, msg.code)
        );
        if msg.code == benilla_protocol::messages::CHAR_CREATE_SUCCESS {
            // `net::io` re-enumerates and emits the fresh roster before the result, so the new
            // row is selected against the list already in hand; no later roster update comes.
            roster.note_created(sel.name.text.clone());
            next.set(ClientState::CharSelect);
        } else if let Ok(mut text) = status.single_mut() {
            text.0 = char_result_text(&strings, msg.code).to_string();
        }
    }
}

/// A `SMSG_CHAR_CREATE` result code (vmangos `ResponseCodes`, `CHAR_CREATE_SUCCESS = 0x2E`) to its
/// GlueStrings key, read off the player's chain, with a built-in caption only when the key is
/// missing. `0x50` has no 1.12 string and takes the default arm; `0x4B` is `CHAR_NAME_RESERVED`,
/// whose enUS text equals `CHAR_CREATE_NAME_IN_USE`'s.
fn char_result_text<'a>(strings: &'a GlueStrings, code: u8) -> &'a str {
    let (key, fallback): (&str, &'a str) = match code {
        0x2E => ("CHAR_CREATE_SUCCESS", "Character created"),
        0x2F => ("CHAR_CREATE_ERROR", "Error creating character"),
        0x30 => ("CHAR_CREATE_FAILED", "Character creation failed"),
        0x31 => ("CHAR_CREATE_NAME_IN_USE", "That name is unavailable"),
        0x32 => (
            "CHAR_CREATE_DISABLED",
            "Creation of that race and/or class is currently disabled.",
        ),
        0x33 => (
            "CHAR_CREATE_PVP_TEAMS_VIOLATION",
            "You cannot have both a Horde and an Alliance character on the same PvP server",
        ),
        0x34 => (
            "CHAR_CREATE_SERVER_LIMIT",
            "You already have the maximum number of characters allowed on this realm.",
        ),
        0x35 => (
            "CHAR_CREATE_ACCOUNT_LIMIT",
            "You already have the maximum number of characters allowed on this account.",
        ),
        0x36 => (
            "CHAR_CREATE_SERVER_QUEUE",
            "This server is currently queued and new character creation is temporarily disabled. \
             Please try again during off peak hours.",
        ),
        0x37 => (
            "CHAR_CREATE_ONLY_EXISTING",
            "Only players who already have characters on this realm are currently allowed to \
             create characters.",
        ),
        0x45 => ("CHAR_NAME_NO_NAME", "Enter a name for your character"),
        0x46 => ("CHAR_NAME_TOO_SHORT", "Names must be at least 2 characters"),
        0x47 => (
            "CHAR_NAME_TOO_LONG",
            "Names must be no more than 12 characters",
        ),
        0x48 => (
            "CHAR_NAME_INVALID_CHARACTER",
            "Names can only contain letters",
        ),
        0x49 => (
            "CHAR_NAME_MIXED_LANGUAGES",
            "Names must contain only one language",
        ),
        0x4A => ("CHAR_NAME_PROFANE", "That name contains profanity"),
        0x4B => ("CHAR_NAME_RESERVED", "That name is unavailable"),
        0x4C => (
            "CHAR_NAME_INVALID_APOSTROPHE",
            "You cannot use an apostrophe as the first or last character of your name",
        ),
        0x4D => (
            "CHAR_NAME_MULTIPLE_APOSTROPHES",
            "You can only have one apostrophe",
        ),
        0x4E => (
            "CHAR_NAME_THREE_CONSECUTIVE",
            "You cannot use the same letter three times consecutively",
        ),
        0x4F => (
            "CHAR_NAME_INVALID_SPACE",
            "You cannot use a space as the first or last character of your name",
        ),
        _ => ("CHAR_CREATE_INVALID_NAME", "Invalid character name"),
    };
    strings.text(key, fallback)
}

#[cfg(test)]
mod tests {
    /// The chain is assembled by the loader's own helper, `GlueLocalization.lua`'s `Localize()`
    /// over `GlueStrings.lua`, so this asserts what the running client shows.
    #[test]
    fn every_char_create_result_resolves_in_the_real_glue_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let strings = crate::glue_strings::table_from_chain(&mut chain);

        // The shipped `0x36` string has a second sentence.
        assert_eq!(
            char_result_text(&strings, 0x36),
            "This server is currently queued and new character creation is temporarily disabled. \
             Please try again during off peak hours."
        );

        // 1.12 has no `CHAR_NAME_CONSECUTIVE_SPACES`, so `0x50` takes the default arm.
        assert_eq!(
            char_result_text(&strings, 0x50),
            char_result_text(&strings, 0xFE),
            "0x50 has no 1.12 string and must fall to the default arm, not invent one"
        );
        assert_eq!(char_result_text(&strings, 0xFE), "Invalid character name");

        for code in [
            0x2Eu8, 0x2F, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x45, 0x46, 0x47, 0x48,
            0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
        ] {
            let shown = char_result_text(&strings, code);
            assert!(!shown.is_empty(), "code {code:#04x} shows nothing");
        }
    }

    /// The two keys share their enUS text, so the key is asserted by name through a sentinel.
    #[test]
    fn the_reserved_name_code_names_the_reserved_key_not_the_in_use_one() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let map = crate::glue_strings::table_from_chain(&mut chain).into_map();

        let reserved = map.get("CHAR_NAME_RESERVED").expect("CHAR_NAME_RESERVED");
        let in_use = map
            .get("CHAR_CREATE_NAME_IN_USE")
            .expect("CHAR_CREATE_NAME_IN_USE");
        assert_eq!(
            reserved, in_use,
            "if these ever differ, the mix-up becomes visible"
        );

        let mut probe = map.clone();
        probe.insert("CHAR_NAME_RESERVED".into(), "RESERVED-SENTINEL".into());
        let strings = GlueStrings::from_map(probe);
        assert_eq!(char_result_text(&strings, 0x4B), "RESERVED-SENTINEL");
        assert_eq!(char_result_text(&strings, 0x31), in_use.as_str());
    }

    use super::*;

    #[test]
    fn race_columns_match_the_reference_screen() {
        assert_eq!(ALLIANCE, [1, 3, 4, 7], "Human, Dwarf, Night Elf, Gnome");
        assert_eq!(HORDE, [2, 5, 6, 8], "Orc, Scourge, Tauren, Troll");
        let mut all: Vec<u8> = ALLIANCE.iter().chain(&HORDE).copied().collect();
        all.sort_unstable();
        assert_eq!(all, (1..=8).collect::<Vec<u8>>());
        assert!(ALLIANCE.windows(2).all(|w| w[0] < w[1]));
        assert!(HORDE.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn look_carries_the_class() {
        let mut sel = CreateSelection {
            race: 1,
            sex: 0,
            class: 1,
            ..default()
        };
        let warrior = sel.look();
        sel.class = 8; // mage, a different starting outfit
        let mage = sel.look();
        assert_eq!(warrior.class, 1);
        assert_eq!(mage.class, 8);
        assert_ne!(
            warrior, mage,
            "the booth look must differ by class, or the preview cannot re-dress"
        );
    }

    /// `SetCharacterRace`, `SetCharacterClass` and `SetCharacterGender` `LockHighlight()` the
    /// chosen button (`CharacterCreate.lua`); with the template's `<CheckedTexture>` commented out,
    /// that lock is the whole selected visual. Built with the real `icon_button`, which must insert
    /// the `LockHighlight` the refresh query needs.
    #[test]
    fn the_chosen_icons_are_lock_highlighted() {
        use crate::glue::art::GlueArt;
        use crate::glue::widgets::{icon_button, LockHighlight};

        fn spawn_icons(mut commands: Commands, art: Res<GlueArt>) {
            let font = Handle::<Font>::default();
            commands.spawn(Node::default()).with_children(|p| {
                for race in ALLIANCE {
                    icon_button(
                        p,
                        &font,
                        CreateAction::Race(race),
                        None::<parts::DynIcon>,
                        None,
                        None::<parts::DynText>,
                        "",
                        &art,
                        1.0,
                    );
                }
                for sex in 0..2u8 {
                    icon_button(
                        p,
                        &font,
                        CreateAction::Gender(sex),
                        None::<parts::DynIcon>,
                        None,
                        None::<parts::DynText>,
                        "",
                        &art,
                        1.0,
                    );
                }
                for slot in 0..8u8 {
                    icon_button(
                        p,
                        &font,
                        CreateAction::ClassSlot(slot),
                        None::<parts::DynIcon>,
                        None,
                        None::<parts::DynText>,
                        "",
                        &art,
                        1.0,
                    );
                }
            });
        }

        let mut app = App::new();
        app.init_resource::<GlueArt>()
            // Human, female, mage: without a catalog `race_classes` is the full list, so slot 6.
            .insert_resource(CreateSelection {
                race: 1,
                sex: 1,
                class: 8,
                ..default()
            })
            .add_systems(Startup, spawn_icons)
            .add_systems(Update, refresh::refresh_hover);
        app.update();

        let locked: Vec<CreateAction> = app
            .world_mut()
            .query::<(&CreateAction, &LockHighlight)>()
            .iter(app.world())
            .filter(|(_, l)| l.0)
            .map(|(a, _)| *a)
            .collect();
        assert_eq!(
            locked.len(),
            3,
            "exactly one race, one gender and one class icon is locked — got {locked:?}"
        );
        assert!(locked.contains(&CreateAction::Race(1)), "Human");
        assert!(locked.contains(&CreateAction::Gender(1)), "female");
        assert!(
            locked.contains(&CreateAction::ClassSlot(6)),
            "mage is slot 6 of [1,2,3,4,5,7,8,9,11] — and it is the mage icon the director saw \
             unmarked"
        );

        // Moving the selection moves the lock, never lights a second icon.
        app.world_mut().resource_mut::<CreateSelection>().class = 1; // warrior, slot 0
        app.update();
        let locked: Vec<CreateAction> = app
            .world_mut()
            .query::<(&CreateAction, &LockHighlight)>()
            .iter(app.world())
            .filter(|(_, l)| l.0)
            .map(|(a, _)| *a)
            .collect();
        assert!(locked.contains(&CreateAction::ClassSlot(0)));
        assert!(!locked.contains(&CreateAction::ClassSlot(6)));
    }
}
