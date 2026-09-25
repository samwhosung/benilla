//! The character-select AddOns screen, the reference's `GlueXML\AddonList.xml/.lua` layout drawn
//! natively: per row a tri-state checkbox (`GetAddOnEnableState`), the title gold when loadable,
//! red when enabled and broken except `DEP_DISABLED` (grey), and the `ADDON_<reason>` status. A
//! row has no highlight and no click; only its checkbox toggles. Okay is `SaveAddOns` (the next
//! login applies it), Cancel, Escape and the X are `ResetAddOns`.
//!
//! Deviation: no Script Memory dial, because there is no Lua heap cap here; no URL or update
//! buttons, because launching a browser is an outward action (`## URL` shows in the tooltip); no
//! security icon, because no installed addon carries `## Secure` honestly and it would always
//! read insecure.
//!
//! No addon is loaded at the glue, so `GetNumAddOns()` would answer 0: the screen reads the folder
//! once at open through [`crate::ui_script::addons::installed_rows`] and
//! [`crate::ui_script::addons::EnableStore`], as world entry does.

use bevy::prelude::*;
use bevy::ui_render::ui_material::MaterialNode;

use benilla_ui::script::addon_gate::{can_load, GateRow, Verdict};
use benilla_ui::script::UiScript;
use benilla_ui::widget::{slider_fraction, slider_grab};

use crate::glue::art::{tc_rect, GlueArt, DIM, GOLD};
use crate::glue::backdrop::{backdrop_border, tiled_bg_node};
use crate::glue::widgets::{
    glue_button, outlined_text, overlay, ArtSwap, GlueBtnKind, GlueText, Hilight,
};
use crate::glue_strings::GlueStrings;
use crate::sound::GlueSound;
use crate::ui_script::addons::{self, InstalledAddOn};

use super::wow_font;

// ── The authored geometry (`AddonList.xml`), y resolved top-down in the plate ──────────────────

/// `AddonListBackground`, the 640×512 `HelpFrame-*` plate, anchored CENTER +(24, 0).
const BG_W: f32 = 640.0;
const BG_H: f32 = 512.0;
const BG_CENTER_OFF_X: f32 = 24.0;
/// `MAX_ADDONS_DISPLAYED` (`AddonList.lua:2`).
const MAX_ROWS: usize = 19;
/// `ADDON_BUTTON_HEIGHT` (`AddonList.lua:1`); each entry's TOP sits 4 below the previous BOTTOM.
const ROW_H: f32 = 16.0;
const ROW_PITCH: f32 = ROW_H + 4.0;
/// Entry 1 sits at TOPLEFT (37, -80); entries are 520 wide.
const ROW_LEFT: f32 = 37.0;
const ROW_TOP: f32 = 80.0;
const ROW_W: f32 = 520.0;
/// The entry's title FontString: LEFT (42, 0), 220 wide; the status hangs 30 right of its box.
const TITLE_LEFT: f32 = 42.0;
const TITLE_W: f32 = 220.0;
const STATUS_LEFT: f32 = TITLE_LEFT + TITLE_W + 30.0;
/// `AddonCharacterDropDown` at TOPLEFT (0, -38); its `CharacterCreate-LabelFrame` art is three
/// 64-tall slices (25 | 115 | 25) whose top rides 17 above the 32-tall frame.
const DROP_TOP: f32 = 38.0;
const DROP_ART_TOP: f32 = DROP_TOP - 17.0;
const DROP_ART_H: f32 = 64.0;
const DROP_SLICE_W: [f32; 3] = [25.0, 115.0, 25.0];
const DROP_W: f32 = DROP_SLICE_W[0] + DROP_SLICE_W[1] + DROP_SLICE_W[2];
/// The slices' texcoords in the 128-wide `CharacterCreate-LabelFrame`.
const DROP_TC: [[f32; 4]; 3] = [
    [0.0, 0.1953125, 0.0, 1.0],
    [0.1953125, 0.8046875, 0.0, 1.0],
    [0.8046875, 1.0, 0.0, 1.0],
];
/// The open list: TOPLEFT to the dropdown's BOTTOMLEFT +(8, 22 up), (8, 48) in the plate.
const DROP_LIST_LEFT: f32 = 8.0;
const DROP_LIST_TOP: f32 = DROP_TOP + 32.0 - 22.0;
/// `AddonListForceLoad`: a 32² checkbox whose TOP-center sits at (+50, -42); label at LEFT +36.
const FORCE_LEFT: f32 = BG_W / 2.0 + 50.0 - 16.0;
const FORCE_TOP: f32 = 42.0;
/// `AddonListScrollFrame`: TOPLEFT (49, -73), 510×390; its slider column and track hang right.
const SCROLL_TOP: f32 = 73.0;
const SCROLL_H: f32 = 390.0;
const SCROLL_RIGHT: f32 = 49.0 + 510.0;
/// The slider column between the two 16² arrows, `GlueScrollBarTemplate`'s `<Slider>`.
const BAR_TOP: f32 = SCROLL_TOP + 16.0;
const BAR_H: f32 = SCROLL_H - 32.0;
/// `UI-ScrollBar-Knob`, 16².
const KNOB: f32 = 16.0;
/// The bottom row: 35 tall, 13 off the plate bottom.
const BTN_TOP: f32 = BG_H - 13.0 - 35.0;

/// Enabled but will not load: `SetTextColor(1.0, 0.1, 0.1)` (`AddonList.lua:63`).
const BROKEN: Color = Color::srgb(1.0, 0.1, 0.1);
/// The `AddonTooltip` backdrop tint, its `OnLoad`'s `SetBackdropColor(0.09, 0.09, 0.19)`.
const TIP_FILL: Color = Color::srgb(0.09, 0.09, 0.19);
/// The tooltip's authored width.
const TIP_W: f32 = 220.0;

/// One checkbox's face, `GetAddOnEnableState`'s 0 / 2 / 1; `Mixed` (enabled for some characters)
/// exists only in the All view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BoxState {
    Off,
    On,
    Mixed,
}

/// What the cursor is over: a row raises `AddonTooltip`, a mixed checkbox `ENABLED_FOR_SOME`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    Row(usize),
    Check(usize),
}

/// The screen's state; edits stage per character and only Okay writes the files.
#[derive(Resource)]
pub(super) struct AddonsPanel {
    pub(super) open: bool,
    /// The installed list read at open, so a row's index is stable; the enable state is `staged`'s.
    list: Vec<InstalledAddOn>,
    /// The roster's realm, half of every enable file's `(realm, name)` key.
    realm: String,
    /// The roster's character names, the dropdown's entries after "All".
    chars: Vec<String>,
    /// `staged[c][i]` is character `c`'s bit for `list[i]`; with no characters, one anonymous
    /// column that Okay does not write.
    staged: Vec<Vec<bool>>,
    /// What the files said at open; Okay writes only the columns that moved off it.
    baseline: Vec<Vec<bool>>,
    /// The dropdown's selection: `None` is "All", `Some(c)` is `chars[c]`.
    view: Option<usize>,
    dropdown_open: bool,
    /// The live `checkAddonVersion`, true meaning the force-load box is unticked; read off the
    /// boot VM every frame.
    version_check: bool,
    /// First visible row (`AddonList.offset`).
    offset: usize,
    /// The scroll drag's grab, as a fraction of the band's height. Held here, not read off
    /// `Interaction::Pressed`: a drag respawns the tree, and a fresh entity's `Interaction` is
    /// `None`.
    drag: Option<f32>,
    /// The spawned tooltip, keyed by what it describes. Hover never sets `dirty`: a respawn resets
    /// `Interaction`, which would flip the hover and respawn again.
    tip: Option<(Hover, Entity)>,
    root: Option<Entity>,
    spawned_s: f32,
    /// The spawned tree no longer matches the state and is respawned whole.
    dirty: bool,
}

impl Default for AddonsPanel {
    fn default() -> Self {
        Self {
            open: false,
            list: Vec::new(),
            realm: String::new(),
            chars: Vec::new(),
            staged: Vec::new(),
            baseline: Vec::new(),
            view: None,
            dropdown_open: false,
            // The registered default, "1"; a `false` would paint out-of-date rows loadable for a
            // frame.
            version_check: true,
            offset: 0,
            drag: None,
            tip: None,
            root: None,
            spawned_s: 0.0,
            dirty: false,
        }
    }
}

impl AddonsPanel {
    /// Open for this realm's roster: read the folder, and one staged column per character, as
    /// `AddonList_OnShow` re-reads. The store loads for the whole roster: an addon a character has
    /// no row for takes what the other characters agree on, else the manifest's `## DefaultState`.
    pub(super) fn open_for(&mut self, realm: String, chars: Vec<String>) {
        self.list = addons::installed_rows();
        self.staged.clear();
        let store = addons::EnableStore::load(&realm, &chars);
        let columns: Vec<Option<&str>> = if chars.is_empty() {
            vec![None]
        } else {
            chars.iter().map(|c| Some(c.as_str())).collect()
        };
        for character in columns {
            self.staged.push(
                self.list
                    .iter()
                    .map(|a| store.enabled_for(&a.name, a.default_state, character))
                    .collect(),
            );
        }
        self.baseline = self.staged.clone();
        self.realm = realm;
        self.chars = chars;
        // "All" is the reference's default selection.
        self.view = None;
        self.dropdown_open = false;
        self.offset = 0;
        self.tip = None;
        self.open = true;
        self.dirty = true;
    }

    pub(super) fn close(&mut self) {
        self.open = false;
        self.list.clear();
        self.chars.clear();
        self.staged.clear();
        self.baseline.clear();
        self.view = None;
        self.dropdown_open = false;
        self.tip = None;
        self.dirty = true;
    }

    /// Okay, `SaveAddOns`: write the enable file of each character whose column changed.
    fn save_staged(&self) {
        for (c, name) in self.chars.iter().enumerate() {
            if self.staged.get(c) == self.baseline.get(c) {
                continue;
            }
            let id = (self.realm.clone(), name.clone());
            let states: Vec<(String, bool)> = self
                .list
                .iter()
                .zip(self.staged[c].iter())
                .map(|(a, &on)| (a.name.clone(), on))
                .collect();
            addons::write_enable_state(Some(&id), &states);
        }
    }

    /// Whether any addon is installed (`UpdateAddonButton` hides the button otherwise). Cached for
    /// the process: the screen respawns every frame of a drag-resize.
    pub(super) fn any_installed() -> bool {
        static ANY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ANY.get_or_init(|| !addons::installed_rows().is_empty())
    }

    fn visible(&self) -> impl Iterator<Item = (usize, &InstalledAddOn)> {
        self.list
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(MAX_ROWS)
    }

    fn max_offset(&self) -> usize {
        self.list.len().saturating_sub(MAX_ROWS)
    }

    /// Where the knob sits, as a fraction of its travel; shared by the spawn and the drag.
    fn thumb_fraction(&self) -> f32 {
        match self.max_offset() {
            0 => 0.0,
            max => self.offset as f32 / max as f32,
        }
    }

    /// Seat the first visible row at `fraction` of the way down, rounded to a row: the reference's
    /// list is a faux scroll frame over 19 fixed slots.
    fn scroll_to(&mut self, fraction: f32) {
        let next = (fraction * self.max_offset() as f32).round() as usize;
        if next != self.offset {
            self.offset = next;
            self.dirty = true;
        }
    }

    /// One row's checkbox for the current view; the All view is `GetAddOnEnableState(nil, i)`'s
    /// tri-state.
    fn box_state(&self, i: usize) -> BoxState {
        match self.view {
            Some(c) => {
                if self
                    .staged
                    .get(c)
                    .is_some_and(|col| col.get(i) == Some(&true))
                {
                    BoxState::On
                } else {
                    BoxState::Off
                }
            }
            None => {
                let on = self
                    .staged
                    .iter()
                    .filter(|col| col.get(i) == Some(&true))
                    .count();
                if on == 0 {
                    BoxState::Off
                } else if on == self.staged.len() {
                    BoxState::On
                } else {
                    BoxState::Mixed
                }
            }
        }
    }

    /// The enable bit the gate is fed: in the All view, on for any character, since
    /// `AddonList_Update` takes `enabled = (checkboxState > 0)` (`AddonList.lua:48`, `0x51e470`).
    fn effective_enabled(&self, i: usize) -> bool {
        self.box_state(i) != BoxState::Off
    }

    /// The current view as the gate's rows; nothing is loaded at the glue.
    fn gate_rows(&self) -> Vec<GateRow<'_>> {
        self.list
            .iter()
            .enumerate()
            .map(|(i, a)| GateRow {
                name: &a.name,
                enabled: self.effective_enabled(i),
                interface: a.interface,
                load_on_demand: a.load_on_demand,
                loaded: false,
                dependencies: a.dependencies.iter().map(String::as_str).collect(),
            })
            .collect()
    }

    /// Why a row will not load. The glue surface passes `demand_only=false`, so
    /// `NOT_DEMAND_LOADED` is unreachable; the version check is re-read per query.
    fn verdict(&self, i: usize) -> Verdict {
        can_load(&self.gate_rows(), i, false, self.version_check)
    }

    /// The title colour (`AddonList.lua:60-66`): gold when loadable, red when enabled and still
    /// not loading unless the reason is `DEP_DISABLED`, grey otherwise. `|c` markup overrides it.
    fn title_colour(&self, i: usize) -> Color {
        let verdict = self.verdict(i);
        if verdict.loadable() {
            GOLD
        } else if self.effective_enabled(i) && verdict.token().as_deref() != Some("DEP_DISABLED") {
            BROKEN
        } else {
            DIM
        }
    }

    /// A checkbox click. In the All view a mixed box counts as checked, so clicking On or Mixed
    /// disables for every character and only an unchecked box enables for all.
    fn click_row(&mut self, i: usize) {
        match self.view {
            Some(c) => {
                if let Some(slot) = self.staged.get_mut(c).and_then(|col| col.get_mut(i)) {
                    *slot = !*slot;
                    self.dirty = true;
                }
            }
            None => {
                let target = self.box_state(i) == BoxState::Off;
                for col in &mut self.staged {
                    if let Some(slot) = col.get_mut(i) {
                        *slot = target;
                    }
                }
                self.dirty = true;
            }
        }
    }

    /// Enable All and Disable All sweep the current view's columns.
    fn set_all(&mut self, on: bool) {
        match self.view {
            Some(c) => {
                if let Some(col) = self.staged.get_mut(c) {
                    col.iter_mut().for_each(|s| *s = on);
                }
            }
            None => {
                for col in &mut self.staged {
                    col.iter_mut().for_each(|s| *s = on);
                }
            }
        }
        self.dirty = true;
    }

    fn view_name<'a>(&'a self, strings: &'a GlueStrings) -> &'a str {
        match self.view {
            None => strings.text("ALL", "All"),
            Some(c) => self.chars.get(c).map(String::as_str).unwrap_or("?"),
        }
    }
}

/// The status text, `getglobal("ADDON_"..reason)` over the parsed GlueStrings; the fallbacks are
/// the shipped values (`GlueStrings.lua:44-56`).
fn status_label<'a>(strings: &'a GlueStrings, token: &'a str) -> &'a str {
    let fallback = match token {
        "DISABLED" => "Disabled",
        "INTERFACE_VERSION" => "Out of date",
        "DEP_MISSING" => "Dependency missing",
        "DEP_DISABLED" => "Dependency disabled",
        "DEP_INTERFACE_VERSION" => "Dependency out of date",
        // `BANNED`, `CORRUPT`, `INSECURE` and `NOT_DEMAND_LOADED` are not produced at the glue.
        other => other,
    };
    strings.get(&format!("ADDON_{token}")).unwrap_or(fallback)
}

/// Flip the "Load out of date AddOns" box, the `checkAddonVersion` CVar inverted (ticked is
/// `"0"`); the statuses repaint from [`drive_addons_panel`]'s per-frame read.
fn toggle_force_load(cvars: &mut crate::cvars::Cvars) {
    let checking = cvars.addon_version_check();
    cvars.set("checkAddonVersion", if checking { "0" } else { "1" });
}

/// The panel's clickable parts.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum AddonsAction {
    /// A row's checkbox, the only thing on a row that clicks (`AddonList_Enable`).
    Row(usize),
    /// The row strip: hover raises the `AddonTooltip`; a press does nothing.
    RowHover(usize),
    EnableAll,
    DisableAll,
    Okay,
    /// Cancel, Escape and the `GlueCloseButton` X, all `AddonList_OnCancel`.
    Cancel,
    ScrollUp,
    ScrollDown,
    /// The slider column, one surface for knob and track: a press anywhere captures, and
    /// [`slider_grab`] says where it grabs.
    ScrollBar,
    /// The dropdown control; also the open list's full-screen backdrop, so a missed click closes
    /// it.
    DropdownToggle,
    /// One dropdown option: `None` is "All", `Some(c)` a roster character.
    DropdownPick(Option<usize>),
    /// The "Load out of date AddOns" checkbox.
    ForceLoad,
}

#[derive(Component)]
struct AddonsUi;

/// The scroll bar's travel band, the knob its child. The drag normalizes the cursor into this
/// node's own box, so it is free of the window scale and centering.
#[derive(Component)]
pub(super) struct ScrollBand;

/// Spawn or despawn the panel, run its flows, and repaint when anything it shows changed.
pub(super) fn drive_addons_panel(
    mut commands: Commands,
    mut panel: ResMut<AddonsPanel>,
    art: Res<GlueArt>,
    assets: Res<AssetServer>,
    strings: Option<Res<GlueStrings>>,
    script: Option<NonSendMut<UiScript>>,
    mut cvars: ResMut<crate::cvars::Cvars>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    mut sounds: MessageWriter<GlueSound>,
    clicks: Res<crate::glue::GlueClicks>,
    hovers: Query<(Entity, &AddonsAction, Ref<Interaction>)>,
    band: Query<(&ComputedNode, &UiGlobalTransform), With<ScrollBand>>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    if !panel.open {
        if let Some(root) = panel.root.take() {
            commands.entity(root).despawn();
        }
        panel.tip = None;
        panel.drag = None;
        wheel.clear();
        return;
    }

    // ── the flows ─────────────────────────────────────────────────────────────────────────────
    let mut close_and_save = false;
    let mut close_and_discard = false;
    let mut bar_pressed = false;
    for (entity, action, interaction) in &hovers {
        // The scroll bar acts on the press: `CSimpleSlider`'s OnMouseDown (`0x789ca0`) warps from
        // any press in its hit rect, with no thumb hit-test. Every button fires on release.
        let click = if *action == AddonsAction::ScrollBar {
            interaction.is_changed() && *interaction == Interaction::Pressed
        } else {
            clicks.hit(entity)
        };
        if !click {
            continue;
        }
        match *action {
            AddonsAction::Row(i) => panel.click_row(i),
            AddonsAction::RowHover(_) => {}
            AddonsAction::EnableAll => panel.set_all(true),
            AddonsAction::DisableAll => panel.set_all(false),
            AddonsAction::Okay => close_and_save = true,
            AddonsAction::Cancel => close_and_discard = true,
            AddonsAction::ScrollUp => scroll(&mut panel, -1),
            AddonsAction::ScrollDown => scroll(&mut panel, 1),
            AddonsAction::ScrollBar => bar_pressed = true,
            AddonsAction::DropdownToggle => {
                // `ToggleDropDownMenu` and `PlaySound("igMainMenuOptionCheckBoxOn")`.
                sounds.write(GlueSound("igMainMenuOptionCheckBoxOn"));
                panel.dropdown_open = !panel.dropdown_open;
                panel.dirty = true;
            }
            AddonsAction::DropdownPick(view) => {
                sounds.write(GlueSound("igMainMenuOptionCheckBoxOn"));
                panel.view = view;
                panel.dropdown_open = false;
                panel.dirty = true;
            }
            AddonsAction::ForceLoad => {
                toggle_force_load(&mut cvars);
            }
        }
    }
    // `AddonList_OnKeyDown`: Escape cancels, Enter accepts.
    if keys.just_pressed(KeyCode::Escape) {
        close_and_discard = true;
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) {
        close_and_save = true;
    }
    for ev in wheel.read() {
        if ev.y != 0.0 {
            scroll(&mut panel, if ev.y > 0.0 { -1 } else { 1 });
        }
    }

    // ── the scroll bar's drag ──────────────────────────────────────────────────────────
    // The press grabs ([`slider_grab`]), then every move maps the cursor absolutely
    // ([`slider_fraction`]). The capture rides the held button, so it survives each respawn.
    let cursor = window
        .single()
        .ok()
        .and_then(|w| w.physical_cursor_position());
    if !mouse.pressed(MouseButton::Left) || cursor.is_none() {
        panel.drag = None;
    }
    if let (Some(cursor), Ok((node, xf))) = (cursor, band.single()) {
        // Fractions of the band's height, free of scale, centering and the retina factor.
        if let Some(local) = node.normalize_point(*xf, cursor) {
            let cursor_n = local.y + 0.5;
            let thumb_n = KNOB / BAR_H;
            if bar_pressed && panel.drag.is_none() {
                let lead_n = panel.thumb_fraction() * (1.0 - thumb_n);
                panel.drag = Some(slider_grab(cursor_n, lead_n, thumb_n));
            }
            if let Some(grab) = panel.drag {
                // `None` is a knob with nowhere to go; the bar is not spawned then.
                if let Some(f) = slider_fraction(cursor_n, grab, 1.0, thumb_n) {
                    panel.scroll_to(f);
                }
            }
        }
    }

    // `checkAddonVersion` read per frame, as the gate reads it per query; absent, the registered
    // default "1".
    let version_check = script
        .as_deref()
        .and_then(|s| s.cvar("checkAddonVersion"))
        .is_none_or(|v| v != "0");
    if panel.version_check != version_check {
        panel.version_check = version_check;
        panel.dirty = true;
    }

    if close_and_save {
        // `AddonList_OnOk` and `AddonList_OnCancel` play the realm dialog's pair.
        sounds.write(GlueSound("gsLoginChangeRealmOK"));
        panel.save_staged();
        panel.close();
        return;
    }
    if close_and_discard {
        sounds.write(GlueSound("gsLoginChangeRealmCancel"));
        panel.close();
        return;
    }

    // ── the tree ──────────────────────────────────────────────────────────────────────────────
    let s = crate::glue::screen_scale(window.single().ok());
    let stale = panel.root.is_some() && (panel.spawned_s != s || panel.dirty);
    if stale {
        if let Some(root) = panel.root.take() {
            commands.entity(root).despawn();
        }
        panel.tip = None;
    }
    if panel.root.is_none() {
        let empty = GlueStrings::default();
        let strings = strings.as_deref().unwrap_or(&empty);
        panel.root = Some(spawn_panel(
            &mut commands,
            &art,
            &assets,
            strings,
            &panel,
            s,
        ));
        panel.spawned_s = s;
        panel.dirty = false;
        // Hover and tooltips reconcile against the fresh entities next frame.
        return;
    }

    // ── hover: the tooltip, with the tree left alone ──────────────────────────────────────────

    let hover = hovers.iter().find_map(|(_, a, i)| {
        if !matches!(*i, Interaction::Hovered | Interaction::Pressed) {
            return None;
        }
        match a {
            AddonsAction::RowHover(r) => Some(Hover::Row(*r)),
            AddonsAction::Row(r) => Some(Hover::Check(*r)),
            _ => None,
        }
    });
    // A checkbox carries a tooltip only when mixed.
    let want = match hover {
        Some(h @ Hover::Row(_)) => Some(h),
        Some(h @ Hover::Check(i)) if panel.box_state(i) == BoxState::Mixed => Some(h),
        _ => None,
    };
    if panel.tip.map(|(h, _)| h) != want {
        if let Some((_, e)) = panel.tip.take() {
            commands.entity(e).despawn();
        }
        if let Some(h) = want {
            let row = match h {
                Hover::Row(i) | Hover::Check(i) => i,
            };
            // `AddonTooltip:SetPoint("TOPRIGHT", this, "TOPLEFT", -14, 0)` on the row strip.
            let strip = hovers
                .iter()
                .find_map(|(e, a, _)| (*a == AddonsAction::RowHover(row)).then_some(e));
            if let (Some(strip), Some(addon)) = (strip, panel.list.get(row)) {
                let empty = GlueStrings::default();
                let strings = strings.as_deref().unwrap_or(&empty);
                let mixed = panel.box_state(row) == BoxState::Mixed;
                let mut tip = Entity::PLACEHOLDER;
                commands.entity(strip).with_children(|p| {
                    tip = spawn_tooltip(p, &art, &assets, strings, addon, h, mixed, s);
                });
                panel.tip = Some((h, tip));
            }
        }
    }
}

/// Move the first visible row (`AddonList.offset`) by `delta`, clamped.
fn scroll(panel: &mut AddonsPanel, delta: i32) {
    let max = panel.max_offset();
    let next = (panel.offset as i32 + delta).clamp(0, max as i32) as usize;
    if next != panel.offset {
        panel.offset = next;
        panel.dirty = true;
    }
}

/// One checkbox as the `CheckButton` draws it: the `UI-CheckBox-Up` box always, the check overlaid
/// (`-Check` when on, `-Check-Disabled` when mixed, `TriStateCheckbox_SetState`'s state 1), and the
/// ADD highlight on hover.
fn checkbox_button<A: Component>(
    parent: &mut ChildSpawnerCommands,
    art: &GlueArt,
    action: A,
    state: BoxState,
    node: Node,
) {
    let mut b = parent.spawn((action, Button, node));
    match &art.checkbox {
        Some(c) => {
            b.insert((
                ImageNode::new(c.up.clone()),
                ArtSwap {
                    up: c.up.clone(),
                    down: c.down.clone(),
                },
            ));
            b.with_children(|b| {
                match state {
                    BoxState::On => {
                        b.spawn((ImageNode::new(c.checked.clone()), overlay()));
                    }
                    BoxState::Mixed => match &art.check_disabled {
                        Some(grey) => {
                            b.spawn((ImageNode::new(grey.clone()), overlay()));
                        }
                        None => {
                            b.spawn((
                                ImageNode {
                                    color: DIM,
                                    ..ImageNode::new(c.checked.clone())
                                },
                                overlay(),
                            ));
                        }
                    },
                    BoxState::Off => {}
                }
                if let Some(hi) = &c.hi {
                    b.spawn((
                        Hilight,
                        Visibility::Hidden,
                        MaterialNode(hi.clone()),
                        overlay(),
                    ));
                }
            });
        }
        None => {
            b.insert(BackgroundColor(match state {
                BoxState::On => GOLD,
                // Art-less only: halfway between GOLD and DIM.
                BoxState::Mixed => Color::srgb(0.75, 0.64, 0.25),
                BoxState::Off => DIM,
            }));
        }
    }
}

fn spawn_panel(
    commands: &mut Commands,
    art: &GlueArt,
    assets: &AssetServer,
    strings: &GlueStrings,
    panel: &AddonsPanel,
    s: f32,
) -> Entity {
    let px = |v: f32| Val::Px(v * s);
    let font = wow_font(assets);
    let abs = |left: f32, top: f32, w: f32, h: f32| Node {
        position_type: PositionType::Absolute,
        left: px(left),
        top: px(top),
        width: px(w),
        height: px(h),
        ..default()
    };

    commands
        .spawn((
            AddonsUi,
            GlobalZIndex(1200), // over the select screen's 1100, like the delete dialog
            // The reference's full-screen BACKGROUND layer, black at 0.75; it also keeps a stray
            // click off the screen behind.
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75)),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
        ))
        .with_children(|overlay_ui| {
            let mut boxed = overlay_ui.spawn(Node {
                width: px(BG_W),
                height: px(BG_H),
                left: px(BG_CENTER_OFF_X),
                ..default()
            });
            boxed.with_children(|b| {
                // ── the HelpFrame plate: six pieces tiling 640×512 ─────────────────────────
                match &art.help_frame {
                    Some(hf) => {
                        for (img, l, t, w, h) in [
                            (&hf.tl, 0.0, 0.0, 256.0, 256.0),
                            (&hf.top, 256.0, 0.0, 256.0, 256.0),
                            (&hf.tr, 512.0, 0.0, 128.0, 256.0),
                            (&hf.bl, 0.0, 256.0, 256.0, 256.0),
                            (&hf.bottom, 256.0, 256.0, 256.0, 256.0),
                            (&hf.br, 512.0, 256.0, 128.0, 256.0),
                        ] {
                            b.spawn((ImageNode::new(img.clone()), abs(l, t, w, h)));
                        }
                        // The ARTWORK divider under the top band: the top pieces' TexCoords
                        // y 0.12109375-0.234375, 29 tall at y 50.
                        const BAND_TC: [f32; 4] = [0.0, 1.0, 0.121_093_75, 0.234_375];
                        for (i, (img, l, w)) in [
                            (&hf.tl, 0.0, 256.0),
                            (&hf.top, 256.0, 256.0),
                            (&hf.tr, 512.0, 128.0),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            b.spawn((
                                ImageNode {
                                    image: img.clone(),
                                    rect: Some(tc_rect(hf.sizes[i], BAND_TC)),
                                    ..default()
                                },
                                abs(l, 50.0, w, 29.0),
                            ));
                        }
                    }
                    None => {
                        b.spawn((
                            BackgroundColor(Color::srgba(0.05, 0.05, 0.08, 0.95)),
                            overlay(),
                        ));
                    }
                }

                // ── the header plate (`UI-DialogBox-Header` 256×64 at TOP (-12, +12)) and
                // `ADDON_LIST` ───────────────────────────────────────────────────────────────
                if let Some((header, _)) = &art.dialog_header {
                    b.spawn((
                        ImageNode::new(header.clone()),
                        abs((BG_W - 256.0) / 2.0 - 12.0, -12.0, 256.0, 64.0),
                    ));
                }
                outlined_text(
                    b,
                    Node {
                        position_type: PositionType::Absolute,
                        left: px((BG_W - 256.0) / 2.0 - 12.0),
                        width: px(256.0),
                        top: px(2.0),
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    (),
                    (),
                    GlueText {
                        text: strings.text("ADDON_LIST", "AddOn List"),
                        size: 12.0, // GlueFontNormalSmall
                        color: GOLD,
                        wrap: false,
                    },
                    &font,
                    s,
                );

                // ── the close X (`GlueCloseButton` at TOPRIGHT (-42, -3)), Cancel ──────────
                {
                    let mut x = b.spawn((
                        AddonsAction::Cancel,
                        Button,
                        abs(BG_W - 42.0 - 32.0, 3.0, 32.0, 32.0),
                    ));
                    if let Some(cb) = &art.close_btn {
                        x.insert((
                            ImageNode::new(cb.up.clone()),
                            ArtSwap {
                                up: cb.up.clone(),
                                down: cb.down.clone(),
                            },
                        ));
                        if let Some(hi) = &cb.hi {
                            x.with_children(|x| {
                                x.spawn((
                                    Hilight,
                                    Visibility::Hidden,
                                    MaterialNode(hi.clone()),
                                    overlay(),
                                ));
                            });
                        }
                    } else {
                        x.with_children(|x| {
                            outlined_text(
                                x,
                                Node {
                                    left: px(10.0),
                                    top: px(6.0),
                                    ..default()
                                },
                                (),
                                (),
                                GlueText {
                                    text: "X",
                                    size: 14.0,
                                    color: GOLD,
                                    wrap: false,
                                },
                                &font,
                                s,
                            );
                        });
                    }
                }

                // ── "Configure Addons For:" and `AddonCharacterDropDown` ─────────────────────
                outlined_text(
                    b,
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(20.0),
                        top: px(DROP_TOP - 14.0),
                        ..default()
                    },
                    (),
                    (),
                    GlueText {
                        text: strings.text("CONFIGURE_MODS_FOR", "Configure Addons For:"),
                        size: 12.0, // GlueFontNormalSmall
                        color: GOLD,
                        wrap: false,
                    },
                    &font,
                    s,
                );
                {
                    let mut drop = b.spawn((
                        AddonsAction::DropdownToggle,
                        Button,
                        abs(0.0, DROP_ART_TOP, DROP_W, DROP_ART_H),
                    ));
                    drop.with_children(|d| {
                        if let Some((sheet, size)) = &art.label_frame {
                            let mut left = 0.0;
                            for (w, tc) in DROP_SLICE_W.iter().zip(DROP_TC) {
                                d.spawn((
                                    ImageNode {
                                        image: sheet.clone(),
                                        rect: Some(tc_rect(*size, tc)),
                                        ..default()
                                    },
                                    abs(left, 0.0, *w, DROP_ART_H),
                                ));
                                left += w;
                            }
                        } else {
                            d.spawn((
                                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.10)),
                                abs(10.0, 20.0, DROP_W - 20.0, 24.0),
                            ));
                        }
                        // The selected value, `GlueFontHighlightSmall`, right-aligned by the arrow.
                        outlined_text(
                            d,
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(10.0),
                                width: px(DROP_W - 43.0 - 10.0),
                                top: px(0.0),
                                height: px(DROP_ART_H),
                                justify_content: JustifyContent::FlexEnd,
                                align_items: AlignItems::Center,
                                ..default()
                            },
                            (),
                            (),
                            GlueText {
                                text: panel.view_name(strings),
                                size: 12.0,
                                color: Color::WHITE,
                                wrap: false,
                            },
                            &font,
                            s,
                        );
                        // The 24² arrow (`UI-ChatIcon-ScrollDown-*`, `UI-Common-MouseHilight`).
                        let mut arrow = d.spawn((
                            AddonsAction::DropdownToggle,
                            Button,
                            abs(DROP_W - 16.0 - 24.0, 18.0, 24.0, 24.0),
                        ));
                        match (&art.dropdown_arrow_up, &art.dropdown_arrow_down) {
                            (Some(up), down) => {
                                arrow.insert(ImageNode::new(up.clone()));
                                if let Some(down) = down {
                                    arrow.insert(ArtSwap {
                                        up: up.clone(),
                                        down: down.clone(),
                                    });
                                }
                                if let Some(hi) = &art.mouse_hilight {
                                    arrow.with_children(|a| {
                                        a.spawn((
                                            Hilight,
                                            Visibility::Hidden,
                                            MaterialNode(hi.clone()),
                                            overlay(),
                                        ));
                                    });
                                }
                            }
                            _ => {
                                arrow.with_children(|a| {
                                    outlined_text(
                                        a,
                                        Node::default(),
                                        (),
                                        (),
                                        GlueText {
                                            text: "v",
                                            size: 12.0,
                                            color: GOLD,
                                            wrap: false,
                                        },
                                        &font,
                                        s,
                                    );
                                });
                            }
                        }
                    });
                }

                // ── "Load out of date AddOns" (`AddonListForceLoad`): ticking erases the
                // `INTERFACE_VERSION` refusal, so those rows repaint loadable ────────────────
                checkbox_button(
                    b,
                    art,
                    AddonsAction::ForceLoad,
                    if panel.version_check {
                        BoxState::Off
                    } else {
                        BoxState::On
                    },
                    abs(FORCE_LEFT, FORCE_TOP, 32.0, 32.0),
                );
                outlined_text(
                    b,
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(FORCE_LEFT + 36.0),
                        top: px(FORCE_TOP),
                        height: px(32.0),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    (),
                    (),
                    GlueText {
                        text: strings.text("ADDON_FORCE_LOAD", "Load out of date AddOns"),
                        size: 12.0,
                        color: GOLD,
                        wrap: false,
                    },
                    &font,
                    s,
                );

                // ── the rows ────────────────────────────────────────────────────────────────
                for (slot, (index, addon)) in panel.visible().enumerate() {
                    let top = ROW_TOP + slot as f32 * ROW_PITCH;
                    let state = panel.box_state(index);
                    let status = panel.verdict(index).token();
                    let colour = panel.title_colour(index);

                    // The row strip: tooltip hover only, no click and no highlight.
                    b.spawn((
                        AddonsAction::RowHover(index),
                        Button,
                        abs(ROW_LEFT, top, ROW_W, ROW_H),
                    ))
                    .with_children(|r| {
                        // Title, clipped at its 220-wide box so it cannot run into the status.
                        outlined_text(
                            r,
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(TITLE_LEFT),
                                width: px(TITLE_W),
                                height: px(ROW_H),
                                align_items: AlignItems::Center,
                                overflow: Overflow::clip(),
                                ..default()
                            },
                            (),
                            (),
                            GlueText {
                                text: addon.display_title(),
                                size: 15.0, // GlueFontNormal
                                color: colour,
                                wrap: false,
                            },
                            &font,
                            s,
                        );
                        // Status, `GlueFontNormalSmall` gold, 30 right of the title box.
                        if let Some(token) = status.as_deref() {
                            outlined_text(
                                r,
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(STATUS_LEFT),
                                    height: px(ROW_H),
                                    align_items: AlignItems::Center,
                                    ..default()
                                },
                                (),
                                (),
                                GlueText {
                                    text: status_label(strings, token),
                                    size: 12.0,
                                    color: GOLD,
                                    wrap: false,
                                },
                                &font,
                                s,
                            );
                        }
                    });
                    // A 32² CheckButton at the entry's LEFT +5, spawned after the strip so it
                    // wins the pick where they overlap.
                    checkbox_button(
                        b,
                        art,
                        AddonsAction::Row(index),
                        state,
                        abs(ROW_LEFT + 5.0, top + (ROW_H - 32.0) / 2.0, 32.0, 32.0),
                    );
                }

                // ── the scrollbar, only when the list overflows (`GlueScrollFrame_Update`) ───
                if panel.max_offset() > 0 {
                    if let Some((track, size)) = &art.char_scrollbar {
                        for (l, t, w, h, tc) in [
                            // Top 31×256 at scrollframe TOPRIGHT (-2, +5).
                            (
                                SCROLL_RIGHT - 2.0,
                                SCROLL_TOP - 5.0,
                                31.0,
                                256.0,
                                [0.0, 0.484_375, 0.0, 1.0],
                            ),
                            // Middle, spanning to the bottom piece (TexCoords y .75-1).
                            (
                                SCROLL_RIGHT - 2.0,
                                SCROLL_TOP - 5.0 + 256.0,
                                31.0,
                                (SCROLL_TOP + SCROLL_H + 2.0 - 106.0) - (SCROLL_TOP - 5.0 + 256.0),
                                [0.0, 0.484_375, 0.75, 1.0],
                            ),
                            // Bottom 31×106 at scrollframe BOTTOMRIGHT (-2, -2).
                            (
                                SCROLL_RIGHT - 2.0,
                                SCROLL_TOP + SCROLL_H + 2.0 - 106.0,
                                31.0,
                                106.0,
                                [0.515_625, 1.0, 0.0, 0.414_062_5],
                            ),
                        ] {
                            b.spawn((
                                ImageNode {
                                    image: track.clone(),
                                    rect: Some(tc_rect(*size, tc)),
                                    ..default()
                                },
                                abs(l, t, w, h),
                            ));
                        }
                    }
                    // The slider column: TOPLEFT at scrollframe TOPRIGHT +(6, -16).
                    let bar_left = SCROLL_RIGHT + 6.0;
                    let bar_top = BAR_TOP;
                    let bar_h = BAR_H;
                    if let Some(sc) = &art.scroll {
                        for (action, up, top) in [
                            (AddonsAction::ScrollUp, &sc.up_btn, bar_top - 16.0),
                            (AddonsAction::ScrollDown, &sc.down_btn, bar_top + bar_h),
                        ] {
                            b.spawn((
                                action,
                                Button,
                                ImageNode {
                                    image: up.up.clone(),
                                    rect: Some(tc_rect(up.size, crate::glue::art::SCROLL_BTN_TC)),
                                    ..default()
                                },
                                abs(bar_left, top, 16.0, 16.0),
                            ))
                            .with_children(|btn| {
                                btn.spawn((
                                    Hilight,
                                    Visibility::Hidden,
                                    MaterialNode(up.hi.clone()),
                                    overlay(),
                                ));
                            });
                        }
                        // The band is the slider, pressable end to end; the knob is decoration.
                        b.spawn((
                            AddonsAction::ScrollBar,
                            ScrollBand,
                            Button,
                            abs(bar_left, bar_top, KNOB, bar_h),
                        ))
                        .with_children(|band| {
                            band.spawn((
                                // The knob must not block the press: a node's default
                                // `FocusPolicy` is `Block`, which would leave only the track
                                // draggable.
                                bevy::ui::FocusPolicy::Pass,
                                ImageNode {
                                    image: sc.knob.0.clone(),
                                    rect: Some(tc_rect(sc.knob.1, crate::glue::art::SCROLL_BTN_TC)),
                                    ..default()
                                },
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(0.0),
                                    top: px(panel.thumb_fraction() * (bar_h - KNOB)),
                                    width: px(KNOB),
                                    height: px(KNOB),
                                    ..default()
                                },
                            ));
                        });
                    } else {
                        for (action, caption, top) in [
                            (AddonsAction::ScrollUp, "-", bar_top - 16.0),
                            (AddonsAction::ScrollDown, "+", bar_top + bar_h),
                        ] {
                            b.spawn((
                                action,
                                Button,
                                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.10)),
                                Node {
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    ..abs(bar_left, top, 16.0, 16.0)
                                },
                            ))
                            .with_children(|sb| {
                                outlined_text(
                                    sb,
                                    Node::default(),
                                    (),
                                    (),
                                    GlueText {
                                        text: caption,
                                        size: 11.0,
                                        color: GOLD,
                                        wrap: false,
                                    },
                                    &font,
                                    s,
                                );
                            });
                        }
                    }
                }

                // ── the bottom row: Disable All, Enable All (`AddonListButtonTemplate` 160×35
                // from BOTTOMLEFT +16), Okay, Cancel (125×35 from BOTTOMRIGHT -46) ──────────
                for (action, token, fallback, left, w, kind) in [
                    (
                        AddonsAction::DisableAll,
                        "DISABLE_ALL_ADDONS",
                        "Disable All",
                        16.0,
                        160.0,
                        GlueBtnKind::List,
                    ),
                    (
                        AddonsAction::EnableAll,
                        "ENABLE_ALL_ADDONS",
                        "Enable All",
                        176.0,
                        160.0,
                        GlueBtnKind::List,
                    ),
                    (
                        AddonsAction::Okay,
                        "OKAY",
                        "Okay",
                        BG_W - 46.0 - 125.0 + 8.0 - 125.0,
                        125.0,
                        GlueBtnKind::Dialog,
                    ),
                    (
                        AddonsAction::Cancel,
                        "CANCEL",
                        "Cancel",
                        BG_W - 46.0 - 125.0,
                        125.0,
                        GlueBtnKind::Dialog,
                    ),
                ] {
                    b.spawn(abs(left, BTN_TOP, w, 35.0)).with_children(|slot| {
                        glue_button(
                            slot,
                            art,
                            &font,
                            action,
                            strings.text(token, fallback),
                            w,
                            35.0,
                            kind,
                            s,
                        );
                    });
                }

                // ── the dropdown's open list (`UIDropDownListTemplate`) over a full-screen
                // click-away backdrop ────────────────────────────────────────────────────────
                if panel.dropdown_open {
                    b.spawn((
                        AddonsAction::DropdownToggle,
                        Button,
                        GlobalZIndex(1205),
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Vw(-100.0),
                            top: Val::Vh(-100.0),
                            width: Val::Vw(300.0),
                            height: Val::Vh(300.0),
                            ..default()
                        },
                    ));
                    let mut list = b.spawn((
                        GlobalZIndex(1210),
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(DROP_LIST_LEFT),
                            top: px(DROP_LIST_TOP),
                            flex_direction: FlexDirection::Column,
                            padding: UiRect::new(px(5.0), px(10.0), px(15.0), px(15.0)),
                            ..default()
                        },
                    ));
                    let framed = art.dialog_bg.is_some() && art.dialog_border.is_some();
                    if !framed {
                        list.insert(BackgroundColor(Color::srgba(0.05, 0.05, 0.08, 0.97)));
                    }
                    list.with_children(|list| {
                        if framed {
                            list.spawn((
                                tiled_bg_node(
                                    art.dialog_bg.clone().unwrap(),
                                    32.0,
                                    s,
                                    Color::WHITE,
                                ),
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(11.0),
                                    right: px(12.0),
                                    top: px(12.0),
                                    bottom: px(11.0),
                                    ..default()
                                },
                            ));
                            backdrop_border(
                                list,
                                art.dialog_border.as_ref().unwrap(),
                                32.0,
                                Color::WHITE,
                            );
                        }
                        // "All", then the roster (`AddonListCharacterDropDown_Initialize`).
                        for view in std::iter::once(None).chain((0..panel.chars.len()).map(Some)) {
                            let name = match view {
                                None => strings.text("ALL", "All"),
                                Some(c) => panel.chars[c].as_str(),
                            };
                            let current = view == panel.view;
                            let mut row = list.spawn((
                                AddonsAction::DropdownPick(view),
                                Button,
                                Node {
                                    height: px(ROW_H),
                                    min_width: px(120.0),
                                    align_items: AlignItems::Center,
                                    ..default()
                                },
                            ));
                            row.with_children(|o| {
                                // The current selection carries the template's 24² check.
                                if current {
                                    if let Some(c) = &art.checkbox {
                                        o.spawn((
                                            ImageNode::new(c.checked.clone()),
                                            Node {
                                                position_type: PositionType::Absolute,
                                                left: px(0.0),
                                                top: px((ROW_H - 24.0) / 2.0),
                                                width: px(24.0),
                                                height: px(24.0),
                                                ..default()
                                            },
                                        ));
                                    }
                                }
                                outlined_text(
                                    o,
                                    Node {
                                        left: px(27.0),
                                        ..default()
                                    },
                                    (),
                                    (),
                                    GlueText {
                                        text: name,
                                        size: 12.0, // GlueFontHighlightSmall
                                        color: Color::WHITE,
                                        wrap: false,
                                    },
                                    &font,
                                    s,
                                );
                                if let Some(hi) = &art.quest_hilight {
                                    o.spawn((
                                        Hilight,
                                        Visibility::Hidden,
                                        MaterialNode(hi.clone()),
                                        overlay(),
                                    ));
                                }
                            });
                        }
                    });
                }
            });
        })
        .id()
}

/// `AddonTooltip`: title, notes and `ADDON_DEPENDENCIES` in a 220-wide box, a child of the row
/// strip. Deviation: `## URL` adds a line in place of the reference's launch button, because
/// launching a browser is an outward action. A mixed checkbox shows `ENABLED_FOR_SOME` alone.
fn spawn_tooltip(
    parent: &mut ChildSpawnerCommands,
    art: &GlueArt,
    assets: &AssetServer,
    strings: &GlueStrings,
    addon: &InstalledAddOn,
    hover: Hover,
    mixed: bool,
    s: f32,
) -> Entity {
    let px = |v: f32| Val::Px(v * s);
    let font = wow_font(assets);
    // (text, size, colour) per line; empties are dropped.
    let mut lines: Vec<(String, f32, Color)> = Vec::new();
    match hover {
        Hover::Row(_) => {
            lines.push((addon.display_title().to_string(), 15.0, GOLD));
            if let Some(notes) = &addon.notes {
                lines.push((notes.clone(), 12.0, Color::WHITE));
            }
            if !addon.dependencies.is_empty() {
                lines.push((
                    format!(
                        "{}{}",
                        strings.text("ADDON_DEPENDENCIES", "Dependencies: "),
                        addon.dependencies.join(", ")
                    ),
                    12.0,
                    GOLD,
                ));
            }
            if let Some(url) = &addon.url {
                lines.push((url.clone(), 12.0, Color::WHITE));
            }
        }
        Hover::Check(_) => {
            if mixed {
                lines.push((
                    strings
                        .text(
                            "ENABLED_FOR_SOME",
                            "This addon is only enabled for some characters.",
                        )
                        .to_string(),
                    12.0,
                    Color::WHITE,
                ));
            }
        }
    }
    let mut tip = parent.spawn((
        GlobalZIndex(1220),
        Node {
            position_type: PositionType::Absolute,
            left: px(-(TIP_W + 14.0)),
            top: px(0.0),
            width: px(TIP_W),
            flex_direction: FlexDirection::Column,
            row_gap: px(2.0),
            padding: UiRect::all(px(10.0)),
            ..default()
        },
    ));
    let framed = art.tooltip_bg.is_some() && art.tooltip_border.is_some();
    if !framed {
        tip.insert(BackgroundColor(TIP_FILL.with_alpha(0.95)));
    }
    tip.with_children(|t| {
        if framed {
            t.spawn((
                tiled_bg_node(art.tooltip_bg.clone().unwrap(), 16.0, s, TIP_FILL),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(5.0),
                    right: px(5.0),
                    top: px(5.0),
                    bottom: px(5.0),
                    ..default()
                },
            ));
            backdrop_border(t, art.tooltip_border.as_ref().unwrap(), 16.0, Color::WHITE);
        }
        for (text, size, colour) in &lines {
            outlined_text(
                t,
                Node::default(),
                (),
                (),
                GlueText {
                    text,
                    size: *size,
                    color: *colour,
                    wrap: true,
                },
                &font,
                s,
            );
        }
    });
    tip.id()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addon(name: &str, deps: &[&str], iface: u32) -> InstalledAddOn {
        InstalledAddOn {
            name: name.into(),
            title: None,
            notes: None,
            url: None,
            dependencies: deps.iter().map(|d| (*d).to_string()).collect(),
            interface: iface,
            load_on_demand: false,
            default_state: true,
        }
    }

    /// A panel over `chars` staged columns, each starting at the manifest defaults, viewing All.
    fn panel_for(chars: usize, list: Vec<InstalledAddOn>) -> AddonsPanel {
        let staged: Vec<Vec<bool>> = (0..chars.max(1))
            .map(|_| list.iter().map(|a| a.default_state).collect())
            .collect();
        AddonsPanel {
            open: true,
            realm: "TestRealm".into(),
            chars: (0..chars).map(|c| format!("Char{c}")).collect(),
            baseline: staged.clone(),
            staged,
            list,
            ..Default::default()
        }
    }

    /// One column, whose All view is that column, so `p.staged[0][i]` drives everything.
    fn panel(list: Vec<InstalledAddOn>) -> AddonsPanel {
        panel_for(1, list)
    }

    /// Every fallback label is its token's shipped value (`GlueStrings.lua:44-56`).
    #[test]
    fn the_status_labels_are_the_reference_globalstrings_values() {
        let strings = GlueStrings::default(); // no chain, so the fallbacks answer
        for (token, want) in [
            ("DISABLED", "Disabled"),
            ("DEP_DISABLED", "Dependency disabled"),
            ("DEP_MISSING", "Dependency missing"),
            ("INTERFACE_VERSION", "Out of date"),
            ("DEP_INTERFACE_VERSION", "Dependency out of date"),
        ] {
            assert_eq!(
                status_label(&strings, token),
                want,
                "ADDON_{token}'s fallback must be the shipped value verbatim"
            );
        }
    }

    /// The status is the gate's verdict in `AddOn_CanLoad`'s check order: a row turned off reads
    /// `DISABLED` even with a missing dependency, since that check precedes the dependency loop.
    #[test]
    fn the_status_column_reports_the_gates_reason_in_the_references_precedence() {
        let mut p = panel(vec![
            addon("Solo", &[], 11200),
            addon("Lib", &[], 11200),
            addon("Needs", &["Lib"], 11200),
            addon("Orphan", &["Nowhere"], 11200),
            addon("Old", &[], 11100),
            addon("Silent", &[], 0),
        ]);

        assert_eq!(
            p.verdict(0).token(),
            None,
            "nothing wrong: no status, gold title"
        );
        assert_eq!(
            p.verdict(2).token(),
            None,
            "its dependency is installed and enabled"
        );
        assert_eq!(p.verdict(3).token().as_deref(), Some("DEP_MISSING"));
        assert_eq!(p.verdict(4).token().as_deref(), Some("INTERFACE_VERSION"));
        assert_eq!(
            p.verdict(5).token().as_deref(),
            Some("INTERFACE_VERSION"),
            "a manifest with NO `## Interface` parses as 0 and IS out of date — the record ctor \
             leaves [rec+0x1c]=0 and the gate compares it like any other value (decision 1292, \
             byte-verified; supersedes 1191 §6's silent-is-current reading)"
        );

        // Force-load erases the refusal (reason 7 written, then reset to 0), so a ticked box
        // leaves an out-of-date row indistinguishable from a current one.
        p.version_check = false;
        assert_eq!(p.verdict(4).token(), None);
        assert_eq!(p.verdict(5).token(), None);
        p.version_check = true;

        p.staged[0][1] = false;
        assert_eq!(p.verdict(1).token().as_deref(), Some("DISABLED"));
        assert_eq!(p.verdict(2).token().as_deref(), Some("DEP_DISABLED"));

        p.staged[0][3] = false;
        assert_eq!(p.verdict(3).token().as_deref(), Some("DISABLED"));
    }

    /// An enabled row that will not load is red, except for `DEP_DISABLED`, which is grey.
    #[test]
    fn the_title_colour_follows_addonlist_luas_rule_with_the_dep_disabled_exception() {
        let mut p = panel(vec![
            addon("Lib", &[], 11200),
            addon("Needs", &["Lib"], 11200),
            addon("Old", &[], 11100),
        ]);
        assert_eq!(p.title_colour(0), GOLD, "loadable = gold");
        assert_eq!(p.title_colour(2), BROKEN, "enabled + out of date = red");

        p.staged[0][0] = false;
        assert_eq!(p.title_colour(0), DIM, "player-disabled = grey");
        assert_eq!(
            p.title_colour(1),
            DIM,
            "enabled but DEP_DISABLED = grey, the Lua's own exception — NOT red"
        );
    }

    /// The All view feeds the gate "on for any character" (`checkboxState > 0`).
    #[test]
    fn the_all_view_feeds_the_gate_any_character_on() {
        let mut p = panel_for(
            2,
            vec![addon("Lib", &[], 11200), addon("Needs", &["Lib"], 11200)],
        );
        p.staged[0][0] = false; // Char0 turned Lib off, Char1 has not

        assert_eq!(
            p.verdict(0).token(),
            None,
            "any-on: the All view still counts Lib enabled"
        );
        assert_eq!(p.verdict(1).token(), None);

        p.view = Some(0);
        assert_eq!(p.verdict(0).token().as_deref(), Some("DISABLED"));
        assert_eq!(p.verdict(1).token().as_deref(), Some("DEP_DISABLED"));

        p.view = Some(1);
        assert_eq!(p.verdict(0).token(), None, "Char1's view is untouched");
    }

    #[test]
    fn the_all_view_box_is_a_tri_state_over_every_character() {
        let mut p = panel_for(3, vec![addon("A", &[], 11200)]);
        assert_eq!(p.box_state(0), BoxState::On, "enabled for all three");

        p.staged[1][0] = false;
        assert_eq!(
            p.box_state(0),
            BoxState::Mixed,
            "enabled for SOME (state 1)"
        );

        p.staged[0][0] = false;
        p.staged[2][0] = false;
        assert_eq!(p.box_state(0), BoxState::Off, "disabled for all");

        p.view = Some(1);
        assert_eq!(p.box_state(0), BoxState::Off);
        p.staged[1][0] = true;
        assert_eq!(p.box_state(0), BoxState::On);
    }

    /// In the All view a mixed box counts as checked, so a click disables for every character.
    #[test]
    fn an_all_view_click_follows_the_references_checkbutton() {
        let mut p = panel_for(3, vec![addon("A", &[], 11200)]);
        p.staged[1][0] = false; // mixed

        p.click_row(0);
        assert!(
            p.staged.iter().all(|col| !col[0]),
            "mixed → disabled for ALL characters"
        );

        p.click_row(0);
        assert!(
            p.staged.iter().all(|col| col[0]),
            "unchecked → enabled for ALL characters"
        );

        p.click_row(0);
        assert!(p.staged.iter().all(|col| !col[0]), "checked → back off");

        p.view = Some(2);
        p.click_row(0);
        assert!(p.staged[2][0] && !p.staged[0][0] && !p.staged[1][0]);
    }

    /// No enable file is touched until Okay; Cancel (`ResetAddOns`) is `close`.
    #[test]
    fn edits_are_staged_and_cancel_discards_them() {
        let mut p = panel(vec![addon("A", &[], 11200), addon("B", &[], 11200)]);
        assert_eq!(p.staged, vec![vec![true, true]]);
        p.staged[0][0] = false;
        assert!(
            p.list[0].default_state,
            "the read-in list is never mutated — only `staged` is, which is what makes Cancel free"
        );
        p.close();
        assert!(!p.open);
        assert!(
            p.staged.is_empty(),
            "a cancelled edit leaves nothing behind"
        );
    }

    /// Okay writes each changed character's file and creates none for an untouched one.
    #[test]
    fn okay_writes_every_changed_characters_file_and_only_those() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp =
            std::env::temp_dir().join(format!("benilla-charsel-fanout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("benilla-config");
        for name in ["Alpha", "Beta"] {
            let dir = home.join("AddOns").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.toc")), "## Interface: 11200\n").unwrap();
        }
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _h =
            crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());

        let mut p = AddonsPanel::default();
        p.open_for(
            "TestRealm".into(),
            vec!["Alice".into(), "Bob".into(), "Carol".into()],
        );
        assert!(p.view.is_none(), "the reference's default selection is All");
        assert_eq!(p.list.len(), 2, "discovery found the two hermetic addons");
        assert_eq!(p.staged.len(), 3, "one staged column per roster character");
        assert_eq!(p.staged, p.baseline);

        p.staged[0][0] = false;
        p.staged[1][1] = false;
        p.save_staged();

        // Read back through the store world entry resolves against.
        let roster = ["Alice".to_string(), "Bob".to_string(), "Carol".to_string()];
        let store = addons::EnableStore::load("TestRealm", &roster);
        let rows = addons::installed_rows();
        let read = |who: &str| -> Vec<bool> {
            rows.iter()
                .map(|a| store.enabled_for(&a.name, a.default_state, Some(who)))
                .collect()
        };
        assert_eq!(
            read("Alice"),
            vec![false, true],
            "Alice's file carries HER edit"
        );
        assert_eq!(read("Bob"), vec![true, false], "Bob's carries HIS");
        let carol = ("TestRealm".to_string(), "Carol".to_string());
        assert!(
            !addons::enable_state_path(Some(&carol)).unwrap().exists(),
            "an unchanged column writes no file — only the diffs against the open-time baseline"
        );
        // Carol, with no file, takes what the others agree on; they split on both, so each falls
        // to its manifest's `## DefaultState`.
        assert_eq!(read("Carol"), vec![true, true]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Disable All, Okay, then create a character: the list reopens all disabled. The reference's
    /// enable query `0x51e470` counts only explicit entries, so a character with no file inherits
    /// the aggregate.
    #[test]
    fn a_new_character_inherits_the_disable_it_did_not_ask_for() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp =
            std::env::temp_dir().join(format!("benilla-charsel-newchar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("benilla-config");
        for name in ["Alpha", "Beta"] {
            let dir = home.join("AddOns").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.toc")), "## Interface: 11200\n").unwrap();
        }
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _h =
            crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());

        let mut p = AddonsPanel::default();
        p.open_for(
            "TestRealm".into(),
            vec!["Onemage".into(), "Onerogue".into()],
        );
        p.set_all(false);
        p.save_staged();
        p.close();

        // A created character, with no enable file.
        p.open_for(
            "TestRealm".into(),
            vec!["Onemage".into(), "Onerogue".into(), "Freshling".into()],
        );
        assert!(p.view.is_none(), "reopens on All, the reference's default");
        assert_eq!(p.staged.len(), 3);
        assert_eq!(
            p.staged[2],
            vec![false, false],
            "the new character inherits the unanimous disable, not a blank slate"
        );
        for i in 0..p.list.len() {
            assert_eq!(
                p.box_state(i),
                BoxState::Off,
                "row {i} reads OFF in the All view — Mixed here is the bug, and it paints a check"
            );
            assert!(!p.effective_enabled(i), "…so nothing is loadable either");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A new character does not inherit a disable the roster split on: it takes the manifest's
    /// `## DefaultState`.
    #[test]
    fn a_new_character_inherits_nothing_when_the_roster_disagrees() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp =
            std::env::temp_dir().join(format!("benilla-charsel-split-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("benilla-config");
        for name in ["Alpha", "Beta"] {
            let dir = home.join("AddOns").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(format!("{name}.toc")),
                // Beta ships `## DefaultState: disabled`, so the tie-break is visible.
                if name == "Beta" {
                    "## Interface: 11200\n## DefaultState: disabled\n"
                } else {
                    "## Interface: 11200\n"
                },
            )
            .unwrap();
        }
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _h =
            crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());

        let mut p = AddonsPanel::default();
        p.open_for(
            "TestRealm".into(),
            vec!["Onemage".into(), "Onerogue".into()],
        );
        p.view = Some(0);
        p.set_all(false); // the mage turns both off; the rogue keeps the defaults
        p.view = Some(1);
        p.set_all(true);
        p.save_staged();
        p.close();

        p.open_for(
            "TestRealm".into(),
            vec!["Onemage".into(), "Onerogue".into(), "Freshling".into()],
        );
        assert_eq!(
            p.staged[2],
            vec![true, false],
            "split roster → each row falls to its own `## DefaultState`"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The force-load box is the `checkAddonVersion` CVar inverted, written into the registry.
    #[test]
    fn the_force_load_box_flips_the_cvar_in_the_registry() {
        let mut cvars = crate::cvars::Cvars::default();
        assert!(
            cvars.addon_version_check(),
            "the registrar default: check ON"
        );

        toggle_force_load(&mut cvars);
        assert_eq!(
            cvars.get("checkAddonVersion"),
            Some("0"),
            "tick: check ON (\"1\") flips to \"0\" — box ticked = version gate open"
        );
        assert!(!cvars.addon_version_check());
        assert_eq!(
            cvars.take_events().len(),
            1,
            "a host write is an accepted move — the config dirties and the mirror learns it"
        );

        toggle_force_load(&mut cvars);
        assert_eq!(
            cvars.get("checkAddonVersion"),
            Some("1"),
            "untick: back to the registrar default"
        );
    }

    /// The drive system's press and move arithmetic in band-normalized units: a knob grab keeps
    /// its offset, and the drag is absolute, so it cannot walk away from the cursor.
    #[test]
    fn the_scroll_knob_drags_the_list_under_the_cursor() {
        let thumb_n = KNOB / BAR_H;
        // 39 addons over 19 slots = 20 scroll positions.
        let mut p = panel(
            (0..MAX_ROWS * 2 + 1)
                .map(|i| addon(&format!("A{i}"), &[], 11200))
                .collect(),
        );
        let max = p.max_offset();
        assert_eq!(max, MAX_ROWS + 1);

        // Press the resting knob at its bottom edge.
        let grab = slider_grab(thumb_n, 0.0, thumb_n);
        assert_eq!(grab, thumb_n, "grabbing the knob's bottom keeps that point");
        let f = slider_fraction(thumb_n, grab, 1.0, thumb_n).expect("the bar has travel");
        p.scroll_to(f);
        assert_eq!(p.offset, 0, "the press alone must not jump the list");

        // Mid-track: the grabbed point stays under the cursor, the leading edge half a knob up.
        let f = slider_fraction(0.5 + thumb_n * 0.5, grab, 1.0, thumb_n).unwrap();
        p.scroll_to(f);
        assert_eq!(p.offset, max / 2, "mid-track = mid-list");

        // Past the bottom end: pinned, the clamp `SetValue` does.
        let f = slider_fraction(4.0, grab, 1.0, thumb_n).unwrap();
        p.scroll_to(f);
        assert_eq!(p.offset, max);
        assert_eq!(p.visible().count(), MAX_ROWS, "the last page is still full");

        // Absolute: returning the cursor to the press point returns the list to its start.
        let f = slider_fraction(thumb_n, grab, 1.0, thumb_n).unwrap();
        p.scroll_to(f);
        assert_eq!(p.offset, 0);
    }

    /// A press on the bare track warps the knob's centre under the cursor and drags on from there
    /// (`0x789ca0`).
    #[test]
    fn a_press_on_the_bare_track_warps_the_knob_under_the_cursor() {
        let thumb_n = KNOB / BAR_H;
        let mut p = panel(
            (0..MAX_ROWS * 2 + 1)
                .map(|i| addon(&format!("A{i}"), &[], 11200))
                .collect(),
        );
        let max = p.max_offset();

        // Press well below the resting knob, three quarters down the bar.
        let grab = slider_grab(0.75, 0.0, thumb_n);
        assert_eq!(grab, thumb_n * 0.5, "off the knob = grab it by its centre");
        let f = slider_fraction(0.75, grab, 1.0, thumb_n).unwrap();
        p.scroll_to(f);
        assert_eq!(
            p.offset,
            ((0.75 - thumb_n * 0.5) / (1.0 - thumb_n) * max as f32).round() as usize,
            "the press itself moves the list — the knob's centre goes to the cursor"
        );
        assert!(p.dirty, "and the move repaints");
    }

    /// [`AddonsPanel::thumb_fraction`] and [`AddonsPanel::scroll_to`] are inverses at every stop,
    /// or a bar creeps a row per grab.
    #[test]
    fn the_knob_position_and_the_row_it_means_are_inverses() {
        let mut p = panel(
            (0..MAX_ROWS + 7)
                .map(|i| addon(&format!("A{i}"), &[], 11200))
                .collect(),
        );
        for row in 0..=p.max_offset() {
            p.offset = row;
            let drawn = p.thumb_fraction();
            p.offset = usize::MAX; // so a no-op `scroll_to` cannot pass by accident
            p.scroll_to(drawn);
            assert_eq!(p.offset, row, "knob at {drawn} must read back as row {row}");
        }
    }

    /// A list that fits has no bar at all, and nothing may divide by its zero travel.
    #[test]
    fn a_list_that_fits_has_no_scroll_positions() {
        let mut p = panel(
            (0..MAX_ROWS)
                .map(|i| addon(&format!("A{i}"), &[], 11200))
                .collect(),
        );
        assert_eq!(p.max_offset(), 0);
        assert_eq!(p.thumb_fraction(), 0.0);
        p.scroll_to(1.0);
        assert_eq!(p.offset, 0, "nowhere to scroll to");
        assert!(!p.dirty);
        assert_eq!(
            slider_fraction(0.5, 0.0, 1.0, 1.0),
            None,
            "a knob as long as its track reports no travel rather than dividing by zero"
        );
    }

    /// Scrolling clamps at both ends and never moves a list that fits.
    #[test]
    fn the_offset_clamps_to_the_list() {
        let mut short = panel(
            (0..5)
                .map(|i| addon(&format!("A{i}"), &[], 11200))
                .collect(),
        );
        assert_eq!(short.max_offset(), 0);
        scroll(&mut short, 1);
        assert_eq!(short.offset, 0, "a list that fits does not scroll");

        let mut long = panel(
            (0..MAX_ROWS + 4)
                .map(|i| addon(&format!("A{i}"), &[], 11200))
                .collect(),
        );
        assert_eq!(long.max_offset(), 4);
        scroll(&mut long, 10);
        assert_eq!(long.offset, 4, "clamped at the bottom, not past it");
        assert_eq!(long.visible().count(), MAX_ROWS);
        scroll(&mut long, -100);
        assert_eq!(long.offset, 0);
    }
}
