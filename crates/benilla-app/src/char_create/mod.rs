//! The character-creation screen — the v1 glue overlay (decision 0423, phase 4).
//!
//! A disposable Bevy-UI screen (the same register as [`crate::char_select`], not GlueXML — the
//! faithful glue arc is 0193's) arranged like the reference client's `CharacterCreate.xml` and
//! driving the live preview booth ([`crate::portrait`]'s `"create"` slot): the left tower holds the
//! faction-bannered race grid, the gender pair, the per-race class grid (only the CharBaseInfo-valid
//! classes, like the ref), the five appearance dials (ranges data-derived per (race, sex) —
//! [`crate::entities::CharCreate`]; labels per-race via the ChrRaces customization tokens), and
//! Randomize; the right stack quotes the GlueStrings faction/race/class paragraphs; the model floats
//! center over the page (transparent booth); name + Accept/Back sit along the bottom. The real art,
//! captions, and click sounds come from the player's own client data ([`art`]'s `GlueArt`,
//! [`crate::glue_strings::GlueStrings`], [`crate::sound::GlueSound`]) — never embedded. Every
//! `SMSG_CHAR_CREATE` result maps to its 1.12 GlueStrings text in the status line; a success
//! re-enums (phase 1) and returns to select with the new row armed.
//!
//! The module is split by concern: [`art`] (client-data art + the frozen tables), [`parts`] (the
//! component vocabulary), [`widgets`] (the glue button shapes), [`screen`] (the authored layout),
//! [`refresh`] (the systems driving it), and this file (state, input, wire).

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

/// Alliance / Horde race columns, top to bottom — the order the reference screen shows
/// (director's screenshot, 2026-07-19). `CharacterCreate.lua`'s `CharacterCreateEnumerateRaces`
/// fills `CharacterCreateRaceButton1..8` in the order the engine's `GetAvailableRaces()` returns,
/// and `CharacterCreate.xml` chains buttons 1–4 down column A from (33,−68) with button 5 anchored
/// right of button 1 (+46) and 6–8 chained below it — so the flat engine order fills Alliance then
/// Horde, ascending race id within each. (`RACE_ICON_TCOORDS` is a name→UV lookup; its literal table
/// order never reaches layout — reading it as the button order is what got this wrong before.)
///
/// `pub(crate)` on the Alliance half because it is also the race→side split
/// `ui_unit::race_faction_group` answers `UnitFactionGroup("player")` with during world entry —
/// one home for the mapping, pinned by [`tests::race_columns_match_the_reference_screen`], rather
/// than a second copy that can disagree with this one.
pub(crate) const ALLIANCE: [u8; 4] = [1, 3, 4, 7]; // Human, Dwarf, Night Elf, Gnome
const HORDE: [u8; 4] = [2, 5, 6, 8]; // Orc, Scourge, Tauren, Troll
/// The ref's initial model facing (`SetCharacterCreateFacing(-15)`), reset on every race switch.
const INITIAL_FACING: f32 = -15.0 * std::f32::consts::PI / 180.0;

/// The character-creation subsystem (decision 0423): the screen + its selection state.
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

/// The end-to-end create instrument (`WOW_CHARCREATE_NAME=<name>`, decision 0423): a few seconds
/// after the create screen is up, fill the name and fire Create — so the whole screen → wire →
/// server → result → back-to-select path is verifiable headlessly (pair with `WOW_CHARCREATE_SHOT=1`
/// to reach the screen). Inert without the env; fires once.
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
        return; // let the model settle + the socket park
    }
    sel.name.set_text(&name);
    sel.creating = true;
    let _ = pick.0.send(CharRequest::Create(sel.request()));
    *fired = true;
    info!("char create: auto-create fired for {:?}", sel.name.text);
}

/// The screen-shot instrument (`WOW_CHARCREATE_SHOT=1`, decision 0423): jump to the create screen a
/// few seconds after boot so a live shot / eyeball reaches it without a click. Inert without the env.
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

/// The shot instrument's race/sex/class pick (`WOW_CHARCREATE_PICK="race,sex[,class]"`): applied
/// once as soon as the create screen is up (after its enter reset), so a probe shot can capture any
/// race's scene — the ref comparisons are per-race. The optional third field picks the class, which
/// selects the starting outfit the preview wears (decision 0527) — so an A/B of the same race at two
/// classes machine-checks the dressing path. An out-of-range class for the race is ignored (the
/// race's first class stands). Inert without the env.
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
    // `clamp` seats the race's first class; the optional third field overrides it, honored only if
    // the race may actually be that class.
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
    // The appearance dials (`WOW_CHARCREATE_DIALS="skin,face,hairStyle,hairColor,facialHair"`):
    // each field optional, a missing/blank one leaves that dial at 0. Every value is clamped into
    // the (race, sex)'s real range, so a probe can name a specific look — e.g. the bald + bearded
    // combination that leaves an orc/gnome male's facial hair without a hair texture.
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

/// The shot writer (`WOW_CHARCREATE_SHOT_OUT=<path>`): once the create screen has been up a few
/// seconds (art + model settled), write one PNG of the window via Bevy's own framebuffer readback —
/// so an agent run can machine-check the screen's geometry without macOS screen-recording
/// permission. Pairs with `WOW_CHARCREATE_SHOT=1`; inert without the env.
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

/// What the create screen currently has selected. The five dials are `[skin, face, hairStyle,
/// hairColor, facialHair]` indices; the ranges come from [`CharCreate`], and are re-clamped whenever
/// race/gender changes (so a dial never points past the new race's range). `class` reaches the booth
/// too — it picks the starting outfit the preview wears (decision 0527).
#[derive(Resource, Default)]
pub(crate) struct CreateSelection {
    race: u8,
    sex: u8,
    class: u8,
    dials: [u8; 5],
    /// The typed name — a real [`EditBoxState`] (decision 0704), so it has the caret, selection,
    /// Ctrl+A and clipboard the chat box has. Letters-only and the 12-cap are enforced by the
    /// shared feed, on pasted text as well as typed.
    name: EditBoxState,
    /// A create is in flight (waiting on `SMSG_CHAR_CREATE`) — the Create button is disarmed and the
    /// status shows progress.
    creating: bool,
}

/// The five dial counts for a (race, sex), or `[1; 5]` when the catalog is missing (so the UI still
/// renders, degenerate).
fn dial_counts(catalog: Option<&CharCreate>, race: u8, sex: u8) -> [u8; 5] {
    catalog
        .and_then(|c| c.0.ranges(race, sex))
        .map(|r| [r.skin, r.face, r.hair_style, r.hair_color, r.facial_hair])
        .unwrap_or([1; 5])
}

impl CreateSelection {
    /// Reset to a valid default for the catalog (Human, male, its first class, dials 0).
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

    /// Re-clamp class + dials into the current (race, sex)'s valid ranges — after a race/gender change.
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

    /// The look to show in the booth: race/gender/class + appearance. Class dresses the preview in
    /// the (race, class, sex) starting outfit (decision 0527), so a class change re-bakes the model.
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

    /// The wire request for the current selection.
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

/// One clickable control on the screen — a single component so one query dispatches every button.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum CreateAction {
    Race(u8),
    Gender(u8),
    /// A class-grid slot (index into the selected race's valid-class list — the ref enumerates only
    /// the classes the race may be, so the slot→class mapping shifts per race).
    ClassSlot(u8),
    /// A dial spinner arrow: dial index 0..5, direction ±1.
    Dial(u8, i8),
    Randomize,
    Create,
    Back,
    /// Hold-to-rotate (the ref's rotate buttons, ±2°/frame).
    RotateLeft,
    RotateRight,
    /// The model pane (drag to rotate — no click action, but it carries the tag for the drag test).
    Model,
}

/// The classes the selected race may be, ascending class id (the CharBaseInfo file order — the
/// ref's `GetClassesForRace` enumeration).
fn race_classes(catalog: Option<&CharCreate>, race: u8) -> Vec<u8> {
    catalog
        .map(|c| c.0.classes_for_race(race))
        .unwrap_or_else(|| vec![1, 2, 3, 4, 5, 7, 8, 9, 11])
}

/// A class id's GlueStrings fileString (`CLASS_<FILE>`, `CLASS_ICON_TCOORDS` key) — a frozen enum
/// of the build.
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

/// The name box's caret blink clock (the ref's `blinkSpeed`, f32 default 0.5 s — `0x77a790`,
/// period `E+0x370` / accumulator `E+0x374`), reset on every keystroke
/// so the caret is solid while you type.
///
/// Its own resource rather than a [`CreateSelection`] field on purpose: ticking it there would trip
/// that resource's change detection every frame and defeat `refresh_dynamic`'s `is_changed` gate,
/// re-running the whole dial/panel/icon refresh 60× a second.
fn create_input(
    buttons: Query<(Entity, &CreateAction)>,
    clicks: Res<crate::glue::GlueClicks>,
    mut keyboard: MessageReader<KeyboardInput>,
    keys: Res<ButtonInput<KeyCode>>,
    catalog: Option<Res<CharCreate>>,
    mut sel: ResMut<CreateSelection>,
    // The host pasteboard + the window handle its Wayland backend needs (decision 0702).
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

    // Every control on this screen is a Button, and a Button fires on the RELEASE, over the
    // button that took the press (1533, `crate::glue::glue_clicks`). The name box needs no
    // press-to-focus: the screen has one field and it is always focused while up.
    for (entity, action) in &buttons {
        if !clicks.hit(entity) {
            continue;
        }
        match *action {
            CreateAction::Race(r) => {
                // The ref plays the click always, switches only on a real change — and a switch
                // resets the class to the race's first and the facing to −15°.
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
                // The ref plays the click always, and re-dresses the model only on a real change:
                // `SelectClass` (`0x470f50`) → `cc_apply_sections` re-applies equipment, because the
                // class picks the starting outfit the preview wears (decision 0527).
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

    // Name typing + Enter/Esc.
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

/// Set every dial to a random valid index (a tiny xorshift, seeded off a per-run counter — the
/// screen has no need for a real RNG dependency; captures never hit Randomize).
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

/// Paint the name box from its [`EditBoxState`] — the display segments, the selection highlight and
/// the caret at the cursor — through the shared [`crate::glue::widgets::paint_glue_field`], so it
/// draws exactly like the login boxes (decision 0704). Only the five name-box row items carry a
/// `GlueFieldPart`, so requiring it is enough to pick them out of every other `DynText`.
fn refresh_name_box(
    sel: Res<CreateSelection>,
    mut parts: Query<(
        &crate::glue::widgets::GlueFieldPart,
        Option<&mut Text>,
        &mut Visibility,
    )>,
) {
    // One field, no focus model: it is focused whenever the screen is up.
    crate::glue::widgets::paint_glue_field(&sel.name, true, parts.iter_mut());
}

/// Rotate the preview: drag on the model pane (the ref's full-frame mouse rotation: `facing +=
/// Δcursor·CHARACTER_ROTATION_CONSTANT` — dragging right *increases* the facing, same sign as the
/// right rotate button), or hold a rotate button (the ref's ±2°-per-frame
/// `RotateLeft/Right_OnUpdate`; left decrements the facing).
///
/// The drag constant is the select screen's, because in the reference it is literally the same
/// number: `CHARACTER_ROTATION_CONSTANT = 0.6` is declared once in `CharacterSelect.lua` and read
/// by both screens' `OnUpdate`. This screen carried a bare `0.01` rad/px (0.573°/px) instead — a
/// 4.5 % drift from the screen next door, against a comment that already claimed 0.6 (1533).
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
            // The fresh roster already arrived (`net::io` re-enumerates and emits it BEFORE the
            // result), so `note_created` selects the new row against the list already in hand —
            // arming a flag for "the next roster update" waited for a message that never comes
            // again, and the select screen came back on the old row (B119).
            roster.note_created(sel.name.text.clone());
            next.set(ClientState::CharSelect);
        } else if let Ok(mut text) = status.single_mut() {
            text.0 = char_result_text(&strings, msg.code).to_string();
        }
    }
}

/// A `SMSG_CHAR_CREATE` result byte → its **GlueStrings key**, resolved off the player's own
/// `GlueStrings.lua` with a built-in caption as the fallback — `login::world_refusal_text`'s shape,
/// and the reference's own (a key first, a literal only when the chain lacks it).
///
/// The codes are vmangos's `ResponseCodes`, anchored at `CHAR_CREATE_SUCCESS = 0x2E`; every key
/// below was derived by matching our old text against the shipped file rather than by name, then
/// corrected where the *semantics* disagreed with the match. Three things that hiding behind
/// literals had concealed (decision 2045):
///
/// - **`0x36` was missing a sentence.** Our copy of `CHAR_CREATE_SERVER_QUEUE` stopped at
///   "…temporarily disabled." where the shipped string continues "Please try again during off peak
///   hours." A re-typed string is a string nobody diffed.
/// - **`0x50` was invented.** "You cannot use consecutive spaces in a name" has no 1.12
///   counterpart — there is no `CHAR_NAME_CONSECUTIVE_SPACES` in this chain — so it takes the
///   default arm, `CHAR_CREATE_INVALID_NAME`, exactly as any code the table does not name does.
///   Composing a better sentence for a code the client has no string for is what 2035 was about.
/// - **`0x4B` named the wrong key.** It resolved `CHAR_CREATE_NAME_IN_USE` because that key's text
///   *equals* `CHAR_NAME_RESERVED`'s in enUS — "That name is unavailable" — but the code is the
///   reserved-name one. Identical text, different key: a locale that words them apart would have
///   shown the wrong sentence, and no amount of text comparison could ever have found it.
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
    /// **Every char-create result resolves to the sentence 1.12 actually ships**, read off the
    /// player's own chain — `GlueStrings.lua` with `GlueLocalization.lua`'s `Localize()` patch over
    /// it, assembled by the loader's own helper so this cannot assert a sentence the running client
    /// would not show (2052). The regression for three bugs that literals had hidden (decision
    /// 2045). Skips without client data.
    #[test]
    fn every_char_create_result_resolves_in_the_real_glue_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let strings = crate::glue_strings::table_from_chain(&mut chain);

        // `0x36` used to stop at "…temporarily disabled."; the shipped string carries a second
        // sentence, and a re-typed string is one nobody diffed.
        assert_eq!(
            char_result_text(&strings, 0x36),
            "This server is currently queued and new character creation is temporarily disabled. \
             Please try again during off peak hours."
        );

        // `0x50` is INVENTED text: 1.12 has no `CHAR_NAME_CONSECUTIVE_SPACES`, so the code takes
        // the default arm like any other the table does not name. If a chain ever *does* carry
        // such a key this assert is what will say so.
        assert_eq!(
            char_result_text(&strings, 0x50),
            char_result_text(&strings, 0xFE),
            "0x50 has no 1.12 string and must fall to the default arm, not invent one"
        );
        assert_eq!(char_result_text(&strings, 0xFE), "Invalid character name");

        // Every named code must resolve to a REAL key — the fallback caption never showing is the
        // whole point, since a fallback that matches hides a missing key forever.
        for code in [
            0x2Eu8, 0x2F, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x45, 0x46, 0x47, 0x48,
            0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F,
        ] {
            let shown = char_result_text(&strings, code);
            assert!(!shown.is_empty(), "code {code:#04x} shows nothing");
        }
    }

    /// **`0x4B` is the reserved-name code, and text alone can never prove it.**
    ///
    /// It used to resolve `CHAR_CREATE_NAME_IN_USE`, whose enUS value is *identical* to
    /// `CHAR_NAME_RESERVED`'s — "That name is unavailable" — so no comparison of the displayed
    /// string could distinguish right from wrong. This asserts the key by *name* against the
    /// shipped file, which is the only thing that can. A locale that words the two apart is
    /// exactly the case this protects.
    #[test]
    fn the_reserved_name_code_names_the_reserved_key_not_the_in_use_one() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let map = crate::glue_strings::table_from_chain(&mut chain).into_map();

        // Both keys exist and agree in enUS — which is why the mix-up was invisible.
        let reserved = map.get("CHAR_NAME_RESERVED").expect("CHAR_NAME_RESERVED");
        let in_use = map
            .get("CHAR_CREATE_NAME_IN_USE")
            .expect("CHAR_CREATE_NAME_IN_USE");
        assert_eq!(
            reserved, in_use,
            "if these ever differ, the mix-up becomes visible"
        );

        // Prove we read the RESERVED one: a table with only that key altered must move 0x4B.
        let mut probe = map.clone();
        probe.insert("CHAR_NAME_RESERVED".into(), "RESERVED-SENTINEL".into());
        let strings = GlueStrings::from_map(probe);
        assert_eq!(char_result_text(&strings, 0x4B), "RESERVED-SENTINEL");
        // …and 0x31, the genuine in-use code, must NOT have moved.
        assert_eq!(char_result_text(&strings, 0x31), in_use.as_str());
    }

    use super::*;

    /// The race grid's column order, pinned against the reference screen (director's screenshot,
    /// 2026-07-19): Alliance reads Human · Dwarf · Night Elf · Gnome, Horde reads Orc · Scourge ·
    /// Tauren · Troll. This is a *regression* test with history: the columns were previously ordered
    /// off `RACE_ICON_TCOORDS`'s table order, which is a name→UV lookup that never reaches layout —
    /// the real order is whatever `GetAvailableRaces()` enumerates into buttons 1–8, which the XML
    /// chains 1–4 down column A and 5–8 down column B.
    #[test]
    fn race_columns_match_the_reference_screen() {
        assert_eq!(ALLIANCE, [1, 3, 4, 7], "Human, Dwarf, Night Elf, Gnome");
        assert_eq!(HORDE, [2, 5, 6, 8], "Orc, Scourge, Tauren, Troll");
        // Together the columns are exactly the eight playable races, no repeats.
        let mut all: Vec<u8> = ALLIANCE.iter().chain(&HORDE).copied().collect();
        all.sort_unstable();
        assert_eq!(all, (1..=8).collect::<Vec<u8>>());
        // Each column ascends by race id — the engine's per-faction enumeration order.
        assert!(ALLIANCE.windows(2).all(|w| w[0] < w[1]));
        assert!(HORDE.windows(2).all(|w| w[0] < w[1]));
    }

    /// The booth look carries the class, so a class click re-dresses the model (decision 0527).
    /// Guards the regression this test was written for: `CreateSelection.class` was set by the
    /// class buttons but never reached `GluePreview`, so the starting outfit never changed.
    #[test]
    fn look_carries_the_class() {
        let mut sel = CreateSelection {
            race: 1,
            sex: 0,
            class: 1,
            ..default()
        };
        let warrior = sel.look();
        sel.class = 8; // mage — a different starting outfit (robe, not the recruit set)
        let mage = sel.look();
        assert_eq!(warrior.class, 1);
        assert_eq!(mage.class, 8);
        assert_ne!(
            warrior, mage,
            "the booth look must differ by class, or the preview cannot re-dress"
        );
    }

    /// **The chosen race / gender / class icon is LOCK-HIGHLIGHTED** — the reference's own verb:
    /// `SetCharacterRace`, `SetCharacterClass` and `SetCharacterGender` each call
    /// `button:LockHighlight()` on the one that is chosen and `UnlockHighlight()` on the rest
    /// (`CharacterCreate.lua` l.171/254/326). In 1.12 that lock *is* the whole selected visual:
    /// `CharacterCreateIconButtonTemplate` has its `<CheckedTexture>` commented out, leaving the
    /// ADD `ButtonHilight-Square` and the HIGHLIGHT-layer `$parentHighlightText` — both of which
    /// the lock is what lights.
    ///
    /// **The regression this exists for** (director, 2026-09-17: *"it doesn't show that mage is
    /// selected"*): 2072 moved the sheen to one owner and gave this screen's visuals query a
    /// `&mut LockHighlight` term — but [`crate::glue::widgets::icon_button`], the only spawn site
    /// for all eighteen of these icons, inserted no such component. A `&mut T` term is a filter,
    /// so the query matched **nothing**: no selected sheen and no icon name on race, gender or
    /// class, for ten days. Every existing sheen test hand-spawned the flag, so the consumer was
    /// covered and the producer was not — hence this one builds the icons with the real
    /// `icon_button` and runs the real system.
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
            // Human, female, MAGE — the director's own case. With no catalog loaded
            // `race_classes` is the full list, so mage (8) is slot 6.
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

        // …and the selection moving takes the lock with it, rather than lighting a second icon.
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
