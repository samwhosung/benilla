//! The **character-select AddOns screen** (decisions 1197, 1293) — the one addon surface (the
//! in-game panel was retired at the director's call, 2026-08-14; `ReloadUI` itself stays).
//!
//! The reference is `Interface\GlueXML\AddonList.xml/.lua`, read off the player's own patch chain
//! (`patch.MPQ` — 1293 located it after 1197 believed it missing), and **this screen now draws the
//! authored layout**: the six `HelpFrame-*` plate pieces that are the whole framed panel, the
//! `UI-DialogBox-Header` title plate, the `GlueCloseButton`, the `GlueDropDownMenuTemplate`
//! control, the 19×(16+4) entry column, the `GlueScrollFrameTemplate` bar with the
//! `UI-Character-ScrollBar` track, and the four-button bottom row in the reference's own order
//! (Disable All · Enable All ··· Okay · Cancel). Geometry constants below are the authored
//! numbers, resolved top-down into the 640×512 plate. Behaviour is **re-expressed natively** —
//! written under 1234's rule, which 1602 has since retired; the native form stays as a design
//! choice, not a law.
//!
//! ## What the reference does, and what we do
//!
//! `AddonList_Update` per row: a **tri-state** checkbox (`GetAddOnEnableState` → 0/1/2, the grey
//! check drawn with `UI-CheckBox-Check-Disabled`), the title (`## Title` else the folder name)
//! coloured **gold when loadable, red when enabled-and-broken — except `DEP_DISABLED`, which reads
//! grey** (AddonList.lua's own exception), and the status token (`getglobal("ADDON_"..reason)`) in
//! `GlueFontNormalSmall` 30 units right of the title box. Titles render their `|cAARRGGBB` markup
//! as colour, exactly as every FontString does — as does every other string on this screen,
//! the tooltip included ([`crate::glue::widgets::markup_spans`]). Hovering a row raises
//! `AddonTooltip` (title + notes + `ADDON_DEPENDENCIES`) anchored to the row's left; hovering a
//! MIXED checkbox raises `ENABLED_FOR_SOME`. **A row has no hover highlight and no row-click** —
//! only the checkbox toggles (the entry template carries no `HighlightTexture` and no `OnClick`);
//! reproducing that is also what killed the hover-respawn flashing the director reported: the
//! spawned tree no longer changes under the cursor at all.
//!
//! Carried from 1197/1293: the character dropdown ("All" + the realm roster, staging
//! per-character), the `checkAddonVersion`-inverted force-load checkbox, Okay = `SaveAddOns`
//! (write the per-character enable files; the next login IS the apply point), Cancel/ESC/the X =
//! `ResetAddOns` (discard). **Not built**, per 1197's standing rejections (Script Memory
//! reaffirmed by the director, 2026-08-14): the Script Memory dial, the URL/update launch buttons
//! (`## URL` shows in the tooltip instead — information, not a trigger), the security icon.
//!
//! ## Why it reads the folder rather than the VM
//!
//! At character select no addon has been *loaded* — `load_third_party` is a world-entry step
//! (1191 §2) — so `GetNumAddOns()` would answer 0. The screen asks
//! [`crate::ui_script::addons::installed_rows`] instead, and resolves the enable bits through
//! [`crate::ui_script::addons::EnableStore`] exactly as the world-entry walk does: the same
//! discovery, the same manifest parser, the same store. Two views of one folder, never two
//! folders. Both reads happen **once at open** ([`AddonsPanel::open_for`]) — the folder walk used
//! to run once per roster character, which decision 2311 retired along with the per-character
//! `installed()` it was there to feed.

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

// ── The authored geometry (AddonList.xml, patch chain) — y resolved top-down in the plate ────────

/// `AddonListBackground` — the 640×512 `HelpFrame-*` plate, anchored CENTER +(24, 0).
const BG_W: f32 = 640.0;
const BG_H: f32 = 512.0;
const BG_CENTER_OFF_X: f32 = 24.0;
/// `MAX_ADDONS_DISPLAYED` (AddonList.lua l.2).
const MAX_ROWS: usize = 19;
/// `ADDON_BUTTON_HEIGHT` (l.1); each entry anchors TOP to the previous BOTTOM at (0, −4).
const ROW_H: f32 = 16.0;
const ROW_PITCH: f32 = ROW_H + 4.0;
/// Entry 1 sits at TOPLEFT (37, −80); entries are 520 wide.
const ROW_LEFT: f32 = 37.0;
const ROW_TOP: f32 = 80.0;
const ROW_W: f32 = 520.0;
/// The entry's title FontString: LEFT (42, 0), 220 wide; the status hangs 30 right of its box.
const TITLE_LEFT: f32 = 42.0;
const TITLE_W: f32 = 220.0;
const STATUS_LEFT: f32 = TITLE_LEFT + TITLE_W + 30.0;
/// `AddonCharacterDropDown` at TOPLEFT (0, −38); its `CharacterCreate-LabelFrame` art is three
/// 64-tall slices (25 | 115 | 25) whose top rides 17 ABOVE the 32-tall frame.
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
/// The open list: TOPLEFT to the dropdown frame's BOTTOMLEFT +(8, 22 up) → (8, 48) in the plate.
const DROP_LIST_LEFT: f32 = 8.0;
const DROP_LIST_TOP: f32 = DROP_TOP + 32.0 - 22.0;
/// `AddonListForceLoad`: a 32² checkbox whose TOP-center sits at (+50, −42); label at LEFT +36.
const FORCE_LEFT: f32 = BG_W / 2.0 + 50.0 - 16.0;
const FORCE_TOP: f32 = 42.0;
/// `AddonListScrollFrame`: TOPLEFT (49, −73), 510×390; its slider column and track hang right.
const SCROLL_TOP: f32 = 73.0;
const SCROLL_H: f32 = 390.0;
const SCROLL_RIGHT: f32 = 49.0 + 510.0;
/// The slider column between the two 16² arrows — the `<Slider>` the reference's
/// `GlueScrollBarTemplate` authors, and the surface a drag measures itself against ([`ScrollBand`]).
const BAR_TOP: f32 = SCROLL_TOP + 16.0;
const BAR_H: f32 = SCROLL_H - 32.0;
/// The knob's extent along the track (`UI-ScrollBar-Knob`, 16²).
const KNOB: f32 = 16.0;
/// The bottom row: 35 tall, 13 off the plate bottom.
const BTN_TOP: f32 = BG_H - 13.0 - 35.0;

/// A red that reads as "this is enabled and will not load" — `SetTextColor(1.0, 0.1, 0.1)`,
/// AddonList.lua's own value.
const BROKEN: Color = Color::srgb(1.0, 0.1, 0.1);
/// The `AddonTooltip` backdrop tint — its `OnLoad`'s `SetBackdropColor(0.09, 0.09, 0.19)`.
const TIP_FILL: Color = Color::srgb(0.09, 0.09, 0.19);
/// The tooltip's authored width.
const TIP_W: f32 = 220.0;

/// One checkbox's face — the reference's `GetAddOnEnableState` values by their meaning
/// (0 / 2 / 1 = [`Self::Off`] / [`Self::On`] / [`Self::Mixed`]). `Mixed` exists only in the
/// All view: enabled for SOME of the characters, not all of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BoxState {
    Off,
    On,
    Mixed,
}

/// What the cursor is over, for the two tooltips: the row strip raises the `AddonTooltip` body,
/// the checkbox raises `ENABLED_FOR_SOME` (only when its state is MIXED) — the reference splits
/// them the same way (the entry's `OnEnter` vs the CheckButton's).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    Row(usize),
    Check(usize),
}

/// The screen's state. Opened by the select screen's AddOns button, driven by
/// [`drive_addons_panel`], cleared on leaving character select.
///
/// `staged` is the whole point of Okay/Cancel: edits go here — one column per roster character —
/// and only Okay writes the files. The reference does the same with `SaveAddOns`/`ResetAddOns`
/// over the client's in-memory per-character records.
#[derive(Resource)]
pub(super) struct AddonsPanel {
    pub(super) open: bool,
    /// The installed list as read at open — never re-read while up, so a row's index is stable.
    /// Shared **metadata**: its own `enabled` bit is the first column's read and nothing repaints
    /// from it; the live enable state is `staged`'s.
    list: Vec<InstalledAddOn>,
    /// The realm the roster belongs to — half of every enable file's key (0997's `(realm, name)`).
    realm: String,
    /// The roster's character names at open, the dropdown's entries after "All". Empty on a fresh
    /// account; `staged` then holds one anonymous column so the checkboxes still work (below).
    chars: Vec<String>,
    /// The edited enable state: `staged[c][i]` is character `c`'s bit for `list[i]`. When `chars`
    /// is empty this is ONE anonymous column (the folder with nobody's file — everything enabled);
    /// it has no file behind it, so Okay writes nothing, exactly what the pre-1293 single-identity
    /// screen did with no character.
    staged: Vec<Vec<bool>>,
    /// What the files said at open, column for column — Okay writes only the columns that moved
    /// off this ([`Self::save_staged`]).
    baseline: Vec<Vec<bool>>,
    /// The dropdown's selection: `None` = "All", `Some(c)` = `chars[c]`. Which character's enable
    /// state the list edits — and whose staged bits feed the gate.
    view: Option<usize>,
    /// The dropdown's option list is up (drawn over the list area).
    dropdown_open: bool,
    /// The live `checkAddonVersion` mirror ("1"/absent = check ON = the force-load box UNTICKED),
    /// refreshed from the boot VM every frame by [`drive_addons_panel`] — the same live-per-query
    /// read the gate's verbs make (1292 §2.2), which is why toggling the box repaints the statuses
    /// with nothing rescanned.
    version_check: bool,
    /// First visible row (`AddonList.offset`).
    offset: usize,
    /// The scroll bar's in-flight drag: the grab [`slider_grab`] returned at the press, as a
    /// fraction of the band's own height ([`ScrollBand`]).
    ///
    /// Held here rather than read back off `Interaction::Pressed` because the panel **respawns
    /// its whole tree** on any repaint — and a drag repaints it every row it crosses. A fresh
    /// entity's `Interaction` starts at `None`, so a capture inferred from the widget would drop
    /// the moment the drag moved the list. The button's own held state (`ButtonInput`) is the
    /// only thing that survives a respawn, so that is what the drag rides on.
    drag: Option<f32>,
    /// The spawned tooltip, keyed by what it describes — **the only thing hover drives, and
    /// never a repaint trigger**. The old model set `dirty` on every hover change and respawned
    /// the whole tree, which reset the fresh entities' `Interaction` to `None`, which flipped
    /// the hover back, which respawned again — the "border flashing" the director reported.
    tip: Option<(Hover, Entity)>,
    root: Option<Entity>,
    spawned_s: f32,
    /// Set while the spawned tree no longer matches the state — one flag rather than a diff,
    /// because a 19-row list is cheaper to respawn than to reconcile. Hover is deliberately NOT
    /// part of this (above).
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
            // The registrar default: "1" = check ON (1292, byte-verified). Hand-written Default
            // for this one field — a derived `false` would paint every out-of-date row loadable
            // for the frame before the drive system's first CVar read corrects it.
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
    /// Open for this realm's roster: the folder read **once** into one metadata list, and one
    /// staged column per character off the enable store. The reference's `AddonList_OnShow` also
    /// re-reads (`AddonList_Update`), so a folder that changed under us is picked up.
    ///
    /// **The store is loaded for the whole roster, not per character** (decision 2311). A column
    /// is not "that character's file, everything else enabled": an addon the character has no row
    /// for takes what the realm's *other* characters agree on, and only falls to the manifest's
    /// `## DefaultState` when they disagree or nobody has said anything. Reading it the other way
    /// is what made a newly created character re-enable every addon in the All view — its
    /// all-enabled column turned every disabled row Mixed, and Mixed reads as on.
    pub(super) fn open_for(&mut self, realm: String, chars: Vec<String>) {
        self.list = addons::installed_rows();
        self.staged.clear();
        let store = addons::EnableStore::load(&realm, &chars);
        // A fresh account stages one anonymous column: no character means no file to key, and
        // Okay has none to write ([`Self::save_staged`] walks `chars`).
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
        // "All" is the reference's own default selection (AddonList.lua's dropdown initializer).
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

    /// Okay — the reference's `SaveAddOns`: write the per-character enable files, and only for
    /// the characters whose staged column actually moved off what their file said at open (the
    /// write is [`addons::write_enable_state`]'s merge, so an untouched character's file is not
    /// even opened). The anonymous no-roster column has no file and writes nothing.
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

    /// Are there any installed addons at all? The reference hides its button when not
    /// (`UpdateAddonButton`), and so do we — a button that opens an empty list is worse than none.
    ///
    /// **Answered once per process, not per call.** The screen asks this while it is spawning its
    /// tree, and the tree respawns on every glue-scale change — which during a window drag-resize
    /// is *every frame*. Reading the folder there means a `read_dir` plus a `.toc` parse per addon
    /// per frame for as long as the director holds the mouse down. Installing or removing an addon
    /// mid-session is not a thing that happens, so the cache never needs invalidating; the panel
    /// itself re-reads the folder on every open ([`Self::open_for`]), which is where freshness
    /// actually matters.
    pub(super) fn any_installed() -> bool {
        static ANY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ANY.get_or_init(|| !addons::installed_rows().is_empty())
    }

    /// The rows currently visible, as `(index, addon)` — enable state is the view's to answer
    /// ([`Self::box_state`]), not the metadata's.
    fn visible(&self) -> impl Iterator<Item = (usize, &InstalledAddOn)> {
        self.list
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(MAX_ROWS)
    }

    /// The highest first-row index that still fills the list.
    fn max_offset(&self) -> usize {
        self.list.len().saturating_sub(MAX_ROWS)
    }

    /// Where the knob sits, as a fraction of its travel — the value→pixel direction. Shared by the
    /// spawn (which draws the knob there) and the drag (which needs to know where it grabbed), so
    /// the two can never disagree about what the bar is showing.
    fn thumb_fraction(&self) -> f32 {
        match self.max_offset() {
            0 => 0.0,
            max => self.offset as f32 / max as f32,
        }
    }

    /// The pixel→value direction: seat the first visible row at `fraction` of the way down.
    ///
    /// Rounded to a whole row because this list scrolls by ROWS, not pixels — the reference's
    /// AddonList is a faux scroll frame over 19 fixed slots, so the knob rides in row-sized
    /// detents. `dirty` only when the row actually changes: a drag inside one detent must not
    /// respawn the tree 60 times a second.
    fn scroll_to(&mut self, fraction: f32) {
        let next = (fraction * self.max_offset() as f32).round() as usize;
        if next != self.offset {
            self.offset = next;
            self.dirty = true;
        }
    }

    /// One row's checkbox state for the current view. A single character's view is plain
    /// two-state over their column; the All view is the reference's tri-state — its
    /// `GetAddOnEnableState(nil, i)`: enabled for all / for some (the greyed check) / for none.
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

    /// Is this addon enabled *for the current view* — the bit the gate is fed? A single character
    /// reads their own column. The All view answers **"any staged character has it on"**, which
    /// 1293 stated as the natural reading of an untraced mechanism and which the bytes have
    /// since confirmed: `AddonList_Update` computes `enabled = (checkboxState > 0)` off
    /// `GetAddOnEnableState(nil, i)`, whose `1` (enabled for some) therefore counts as on
    /// (`0x51e470`). What 1293 got wrong was not this fold but what each column holds — see
    /// [`Self::open_for`] and decision 2311.
    fn effective_enabled(&self, i: usize) -> bool {
        self.box_state(i) != BoxState::Off
    }

    /// The current view lowered into the gate's adapter rows (1292): the folder's one registry
    /// with THIS view's staged enable bits. `loaded` is `false` for every row — nothing has
    /// loaded at the glue (`load_third_party` is a world-entry step, 1191 §2), so the loaded
    /// short-circuit can never fire here.
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

    /// **Why a row will not load** — the one arbiter's answer (1292), which the status column
    /// splices and the title colour keys on. `demand_only=false`: this is the glue surface
    /// (`dl=0`) — `NOT_DEMAND_LOADED` is unreachable here. `version_check` is the live CVar
    /// mirror, re-read per query like the reference (1292 §2.2).
    fn verdict(&self, i: usize) -> Verdict {
        can_load(&self.gate_rows(), i, false, self.version_check)
    }

    /// AddonList.lua's title colour, verbatim in structure: gold when loadable; red when the
    /// player has it ON and it still will not load — **unless the reason is `DEP_DISABLED`**,
    /// which reads grey like a disabled row (the Lua's own `reason ~= "DEP_DISABLED"` guard: the
    /// player already made that choice one row up, and red would blame the dependent); grey
    /// otherwise. Any `|c` markup in the title overrides this base per-span, exactly as the
    /// reference FontString renders it ([`crate::glue::widgets::markup_spans`]).
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

    /// A checkbox click for the current view. In a single character's view it is a plain toggle
    /// of their bit. In the All view it is the reference's CheckButton over the tri-state: a
    /// grey (mixed) box **counts as checked**, so clicking On *or* Mixed unchecks — disable for
    /// every character — and only a fully-unchecked box enables for all.
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

    /// Enable All / Disable All sweep the CURRENT VIEW: the view the player is editing is the
    /// view the sweep edits — "All" sweeps every column, a single character's view only theirs.
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

    /// The dropdown's current value — "All" or the viewed character's name.
    fn view_name<'a>(&'a self, strings: &'a GlueStrings) -> &'a str {
        match self.view {
            None => strings.text("ALL", "All"),
            Some(c) => self.chars.get(c).map(String::as_str).unwrap_or("?"),
        }
    }
}

/// The status column's text: the reference's `getglobal("ADDON_"..reason)` splice, run against
/// the parsed GlueStrings table — the glue side loads no VM to splice in (1197 §3), but the
/// string TABLE is loaded, so the lookup is the same one. The fallbacks are the shipped values
/// of the same tokens, pinned by `the_status_labels_are_the_reference_globalstrings_values`
/// (verified against the extracted 1.12 chain, GlueStrings.lua l.44-56).
fn status_label<'a>(strings: &'a GlueStrings, token: &'a str) -> &'a str {
    let fallback = match token {
        "DISABLED" => "Disabled",
        "INTERFACE_VERSION" => "Out of date",
        "DEP_MISSING" => "Dependency missing",
        "DEP_DISABLED" => "Dependency disabled",
        "DEP_INTERFACE_VERSION" => "Dependency out of date",
        // `BANNED`/`CORRUPT`/`INSECURE` gate on server signature state we do not model, and
        // `NOT_DEMAND_LOADED` is the in-game surface's (`demand_only=false` here) — none is
        // producible at the glue (addon_gate's module doc); the raw token is the honest fallback.
        other => other,
    };
    strings.get(&format!("ADDON_{token}")).unwrap_or(fallback)
}

/// Flip the *Load out of date AddOns* box: the `checkAddonVersion` CVar **inverted** (1292 §2,
/// byte-verified — ticked = `"0"`). A host write into the registry (2303), which is what
/// persists it and what the VM's mirror learns — nothing else to do: the statuses repaint from
/// [`drive_addons_panel`]'s per-frame mirror, no rescan, because the gate re-reads the flag per
/// query (1292 §2.2).
fn toggle_force_load(cvars: &mut crate::cvars::Cvars) {
    let checking = cvars.addon_version_check();
    cvars.set("checkAddonVersion", if checking { "0" } else { "1" });
}

/// The panel's clickable parts.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) enum AddonsAction {
    /// A row's CHECKBOX — the only thing on a row that clicks, as in the reference (the entry
    /// template has no `OnClick`; `AddonList_Enable` hangs off the CheckButton alone).
    Row(usize),
    /// The row strip itself: hover raises the `AddonTooltip`; a press does nothing.
    RowHover(usize),
    EnableAll,
    DisableAll,
    Okay,
    /// Cancel, the ESC key, and the `GlueCloseButton` X — all `AddonList_OnCancel`.
    Cancel,
    ScrollUp,
    ScrollDown,
    /// The slider column — **one surface covering knob and track alike**, because the law has no
    /// thumb hit-test to gate capture on: a press anywhere on the bar captures, and where it
    /// *grabs* is [`slider_grab`]'s to say, from geometry rather than from picking.
    ScrollBar,
    /// The dropdown control — opens/closes the option list. Also carried by the open list's
    /// full-screen backdrop, so a click that misses every option closes it (click-away).
    DropdownToggle,
    /// One dropdown option: `None` = "All", `Some(c)` = that roster character.
    DropdownPick(Option<usize>),
    /// The *Load out of date AddOns* checkbox ([`toggle_force_load`]).
    ForceLoad,
}

/// The panel root (despawned whole on close).
#[derive(Component)]
struct AddonsUi;

/// The scroll bar's travel band — the node spanning [`BAR_TOP`]`..`[`BAR_TOP`]`+`[`BAR_H`], with
/// the knob riding inside it as a child.
///
/// The drag reads its `ComputedNode`/`UiGlobalTransform` rather than re-deriving where the bar
/// landed on screen: this panel is centered by a flex parent at a window-dependent scale, so any
/// hand-rolled screen-rect math here would be a second copy of the layout, wrong the first time
/// the window resizes. Normalizing the cursor into this node's own box is scale-free by
/// construction.
#[derive(Component)]
pub(super) struct ScrollBand;

/// Spawn/despawn the panel, run its flows, and repaint when anything it shows has changed.
///
/// Ordered before the select screen's own click handling so a click that lands on the panel is
/// never also read as a click on the screen behind it.
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
        // **The scroll bar warps on the PRESS**, and only it: the reference's `CSimpleSlider`
        // OnMouseDown (`0x789ca0`, 45 bytes, zero branches) warps the value from any press inside
        // the hit rect — there is no thumb hit-test in the class. Every *button*
        // in this panel fires on the RELEASE, over the button that took the press (1533).
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
                // The reference's dropdown button click (`ToggleDropDownMenu` +
                // `PlaySound("igMainMenuOptionCheckBoxOn")`).
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
                // No VM (a bare test world / a capture): nothing to write to and nothing the
                // walk would read differently — the box is inert, honestly.
                toggle_force_load(&mut cvars);
            }
        }
    }
    // `AddonList_OnKeyDown`: ESCAPE cancels, ENTER accepts.
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

    // ── the scroll bar's drag (B274) ──────────────────────────────────────────────────────────
    // Benilla's one slider law, reached from the glue lane: the press grabs ([`slider_grab`] —
    // on the knob it keeps the grabbed point, off it the knob's centre warps under the cursor),
    // and press and every move after it share the one absolute cursor→value map
    // ([`slider_fraction`]). Absolute, so it cannot drift off the cursor over a long drag; and
    // the capture rides the held BUTTON, not the widget, so it survives the repaint that every
    // crossed row triggers and keeps tracking with the cursor off the bar entirely.
    let cursor = window
        .single()
        .ok()
        .and_then(|w| w.physical_cursor_position());
    if !mouse.pressed(MouseButton::Left) || cursor.is_none() {
        // Release, or the cursor left the window — the widget lane abandons a capture on the
        // same two conditions.
        panel.drag = None;
    }
    if let (Some(cursor), Ok((node, xf))) = (cursor, band.single()) {
        // Everything below is a fraction of the band's own height, which makes it free of the
        // window scale, the panel's centering, and the retina factor all at once.
        if let Some(local) = node.normalize_point(*xf, cursor) {
            let cursor_n = local.y + 0.5;
            let thumb_n = KNOB / BAR_H;
            if bar_pressed && panel.drag.is_none() {
                let lead_n = panel.thumb_fraction() * (1.0 - thumb_n);
                panel.drag = Some(slider_grab(cursor_n, lead_n, thumb_n));
            }
            if let Some(grab) = panel.drag {
                // `None` = a knob with nowhere to go; the bar is not spawned in that case.
                if let Some(f) = slider_fraction(cursor_n, grab, 1.0, thumb_n) {
                    panel.scroll_to(f);
                }
            }
        }
    }

    // The live `checkAddonVersion` mirror (1293): read per frame like the gate's own per-query
    // read (1292 §2.2), so the ForceLoad click above — or any other writer — repaints the
    // statuses this same frame with nothing rescanned. Absent VM or table = the registrar
    // default: check ON.
    let version_check = script
        .as_deref()
        .and_then(|s| s.cvar("checkAddonVersion"))
        .is_none_or(|v| v != "0");
    if panel.version_check != version_check {
        panel.version_check = version_check;
        panel.dirty = true;
    }

    if close_and_save {
        // `AddonList_OnOk` / `AddonList_OnCancel` play the realm dialog's pair.
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
        // Fresh entities — hover, tooltips and highlights reconcile against them next frame.
        return;
    }

    // ── hover: the tooltip, with the tree left alone ──────────────────────────────────────────
    // (The sheens are `crate::glue::glue_hilights`' — this panel used to drive its own, which is
    // one of the four copies that let the realm list ship with none.)

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
    // What the tooltip should say for this hover — `None` collapses "nothing hovered" and "a
    // checkbox that is not mixed" (the reference's CheckButton only carries a tooltip then).
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
            // Anchor to the row strip — the reference's own
            // `AddonTooltip:SetPoint("TOPRIGHT", this, "TOPLEFT", -14, 0)`.
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

/// Move the first visible row by `delta`, clamped — the reference's `AddonList.offset`.
fn scroll(panel: &mut AddonsPanel, delta: i32) {
    let max = panel.max_offset();
    let next = (panel.offset as i32 + delta).clamp(0, max as i32) as usize;
    if next != panel.offset {
        panel.offset = next;
        panel.dirty = true;
    }
}

/// One checkbox as the reference `CheckButton` draws it: the `UI-CheckBox-Up` box always (the
/// pressed swap is [`ArtSwap`]'s), the check OVERLAID — gold (`-Check`) when on, grey
/// (`-Check-Disabled`) when mixed (`TriStateCheckbox_SetState`'s state-1 texture) — and the ADD
/// highlight lit on hover ([`HoverLit`]). The old face swapped the box art for the check art,
/// which is why the director's screenshot showed floating checks with no boxes. Plain-fill
/// fallback without art.
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
                // Ours: halfway between the art-less GOLD and DIM, so a mixed box still reads
                // as its own state.
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
            // The reference's own full-screen BACKGROUND layer: black at 0.75 over the glue
            // scene, which is also what keeps a stray click off the screen behind (picking
            // follows z-order).
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
                // The plate's authored CENTER offset (+24, 0).
                left: px(BG_CENTER_OFF_X),
                ..default()
            });
            boxed.with_children(|b| {
                // ── the HelpFrame plate: six pieces tiling 640×512, frame + top band + dark
                // inset + bottom band all baked into the art ─────────────────────────────────
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
                        // The ARTWORK divider strip under the top band: the same three top
                        // pieces' authored sub-band (TexCoords y 0.12109375–0.234375), 29 tall
                        // at y 50.
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

                // ── the header plate + title (`UI-DialogBox-Header` 256×64 at TOP (−12, +12),
                // `ADDON_LIST` GlueFontNormalSmall 14 under its top) ─────────────────────────
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

                // ── the close X (`GlueCloseButton` at TOPRIGHT (−42, −3)) — Cancel ──────────
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

                // ── "Configure Addons For:" + the dropdown (`AddonCharacterDropDown`,
                // `GlueDropDownMenuTemplate`) ────────────────────────────────────────────────
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
                        // The three `CharacterCreate-LabelFrame` slices (25 | 115 | 25 of the
                        // 128-wide art) — the same sheet the create screen's dial rows use.
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
                        // The selected value (white, `GlueFontHighlightSmall`) right-aligned
                        // beside the arrow — the template's own text seat.
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
                        // The 24² arrow (`UI-ChatIcon-ScrollDown-*` + `UI-Common-MouseHilight`).
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

                // ── "Load out of date AddOns" (`AddonListForceLoad`: a 32² checkbox at TOP
                // (+50, −42), label at LEFT +36) — the `checkAddonVersion` CVar INVERTED
                // (ticked = "0"; 1292 §2.1: force-load actively ERASES the refusal, so ticking
                // repaints every INTERFACE_VERSION row as plain loadable) ────────────────────
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
                    // The one arbiter (1292): status and colour both come from the gate over
                    // the CURRENT VIEW's staged states.
                    let status = panel.verdict(index).token();
                    let colour = panel.title_colour(index);

                    // The row strip: tooltip hover only — no click, no highlight (the
                    // reference entry has neither; see the module doc on the flashing).
                    b.spawn((
                        AddonsAction::RowHover(index),
                        Button,
                        abs(ROW_LEFT, top, ROW_W, ROW_H),
                    ))
                    .with_children(|r| {
                        // Title — `GlueFontNormal`, 220 wide, colour markup rendered
                        // ([`crate::glue::widgets::markup_spans`], run by every `GlueText`);
                        // clipped at the authored box so a long title
                        // cannot run into the status column.
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
                        // Status — `GlueFontNormalSmall` (gold), 30 right of the title box:
                        // the reference's `getglobal("ADDON_"..reason)` splice.
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
                    // The checkbox: a 32² CheckButton at the entry's LEFT +5, spawned after
                    // the strip so it wins the pick where they overlap.
                    checkbox_button(
                        b,
                        art,
                        AddonsAction::Row(index),
                        state,
                        abs(ROW_LEFT + 5.0, top + (ROW_H - 32.0) / 2.0, 32.0, 32.0),
                    );
                }

                // ── the scrollbar, shown only when the list overflows (the reference's
                // `GlueScrollFrame_Update` hides it on the same condition): the
                // `UI-Character-ScrollBar` decorative track + the `GlueScrollBarTemplate`
                // column (16² arrows, the knob riding the offset fraction) ──────────────────
                if panel.max_offset() > 0 {
                    if let Some((track, size)) = &art.char_scrollbar {
                        for (l, t, w, h, tc) in [
                            // Top 31×256 at scrollframe TOPRIGHT (−2, +5).
                            (
                                SCROLL_RIGHT - 2.0,
                                SCROLL_TOP - 5.0,
                                31.0,
                                256.0,
                                [0.0, 0.484_375, 0.0, 1.0],
                            ),
                            // Middle, spanning to the bottom piece (TexCoords y .75–1).
                            (
                                SCROLL_RIGHT - 2.0,
                                SCROLL_TOP - 5.0 + 256.0,
                                31.0,
                                (SCROLL_TOP + SCROLL_H + 2.0 - 106.0) - (SCROLL_TOP - 5.0 + 256.0),
                                [0.0, 0.484_375, 0.75, 1.0],
                            ),
                            // Bottom 31×106 at scrollframe BOTTOMRIGHT (−2, −2).
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
                    // The slider column: TOPLEFT at scrollframe TOPRIGHT +(6, −16).
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
                        // The band IS the slider: pressable end to end, with the knob a
                        // decorative child riding at the value fraction. Splitting them into two
                        // pickable surfaces would put a thumb hit-test back into the capture
                        // decision, which the class does not have (`slider_grab`).
                        b.spawn((
                            AddonsAction::ScrollBar,
                            ScrollBand,
                            Button,
                            abs(bar_left, bar_top, KNOB, bar_h),
                        ))
                        .with_children(|band| {
                            band.spawn((
                                // The knob must NOT capture the press it sits under: a UI node
                                // with no `FocusPolicy` defaults to `Block`, and a blocking knob
                                // would swallow every press aimed at it — the one gesture this
                                // whole path exists to serve — leaving only the bare track
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

                // ── the bottom row, the reference's own order and geometry: Disable All ·
                // Enable All (`AddonListButtonTemplate` 160×35 from BOTTOMLEFT +16) ··· Okay ·
                // Cancel (`GlueDialogButtonTemplate` 125×35 from BOTTOMRIGHT −46) ───────────
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

                // ── the dropdown's open list (`UIDropDownListTemplate`: the DialogBox
                // backdrop, 16-tall rows, the gold check on the current selection, the
                // `UI-QuestTitleHighlight` hover), plus a full-screen click-away backdrop
                // underneath it in z ────────────────────────────────────────────────────────
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
                        // "All" first, then the roster in its own order — the reference's
                        // `AddonListCharacterDropDown_Initialize` shape.
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

/// The hover tooltip — the reference's `AddonTooltip` (a 220-wide `UI-Tooltip-Border` box, fill
/// `(0.09, 0.09, 0.19)`, title + notes + `ADDON_DEPENDENCIES` in that order) anchored TOPRIGHT to
/// the row's TOPLEFT at (−14, 0), spawned as the row strip's child so the anchor is structural.
/// `## URL` rides as an extra line (1197: information, not a launch button). A MIXED checkbox
/// hover shows `ENABLED_FOR_SOME` alone — the reference's GlueTooltip split, on the same box.
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
    // (text, size, colour) per line; empties dropped below.
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

    /// A panel over `chars` staged columns (each starting at the list's own `enabled` bits),
    /// viewing All — the open-time default. Tests that want a single character's view set
    /// `p.view = Some(c)` themselves.
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

    /// The single-character convenience: one column, All view — over one column the All view IS
    /// that column (any-on and all-on coincide), so `p.staged[0][i]` drives everything.
    fn panel(list: Vec<InstalledAddOn>) -> AddonsPanel {
        panel_for(1, list)
    }

    /// **Every label this panel can show is the value its reference token carries**.
    ///
    /// The splice is the reference's own (`getglobal("ADDON_"..reason)`), run against the parsed
    /// GlueStrings table; what this pins is the FALLBACKS — the strings shown with no client
    /// data — quoted from the extracted 1.12 chain (GlueStrings.lua l.44-56). An empty table
    /// exercises exactly that path.
    ///
    /// This test exists because a comment here once named a token `INCOMPATIBLE`, **which does
    /// not exist** — the real one is `ADDON_INTERFACE_VERSION`. A wrong token name is invisible
    /// until somebody splices it.
    #[test]
    fn the_status_labels_are_the_reference_globalstrings_values() {
        let strings = GlueStrings::default(); // no chain in a test — the pinned fallbacks answer
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

    /// The status column's rule — THE GATE's verdict (1292): `AddOn_CanLoad`'s byte-verified
    /// check order, consulted over the current view's staged states.
    ///
    /// The ordering is the part worth pinning: a row the player turned OFF reads `DISABLED` and
    /// nothing else, even when its dependency is also missing — the gate's check 3 precedes the
    /// dependency loop, and the alternative tells a player to go fix a dependency for an addon
    /// they deliberately switched off.
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

        // Force-load ERASES the refusal (1292 §2.1: reason 7 written, then reset to 0) — with
        // the box ticked an out-of-date row is byte-indistinguishable from a current one, so the
        // screen shows nothing there, faithfully.
        p.version_check = false;
        assert_eq!(p.verdict(4).token(), None);
        assert_eq!(p.verdict(5).token(), None);
        p.version_check = true;

        // Turning the library off makes its dependent unloadable, and the dependent says so.
        p.staged[0][1] = false;
        assert_eq!(p.verdict(1).token().as_deref(), Some("DISABLED"));
        assert_eq!(p.verdict(2).token().as_deref(), Some("DEP_DISABLED"));

        // ...and a row the player turned off reports only that, even with a broken dependency.
        p.staged[0][3] = false;
        assert_eq!(p.verdict(3).token().as_deref(), Some("DISABLED"));
    }

    /// AddonList.lua's title colour, including its one exception: an enabled row that will not
    /// load reads RED — **unless the reason is `DEP_DISABLED`**, where the Lua's own
    /// `reason ~= "DEP_DISABLED"` guard sends it to grey (the player already made that choice
    /// on the dependency's row; the first build here painted it red and the director's shot
    /// showed the mismatch).
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

    /// The All view's gate input is OUR reading (1293): "enabled = any staged character has it on".
    #[test]
    fn the_all_view_feeds_the_gate_any_character_on() {
        let mut p = panel_for(
            2,
            vec![addon("Lib", &[], 11200), addon("Needs", &["Lib"], 11200)],
        );
        p.staged[0][0] = false; // Char0 turned Lib off; Char1 has not

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

    /// The All view's box is the reference's tri-state (`GetAddOnEnableState`: 0/1/2), computed
    /// across every staged column; a single character's view is plain two-state over theirs.
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

        // A single character's view never shows Mixed — it is their own bit, both ways.
        p.view = Some(1);
        assert_eq!(p.box_state(0), BoxState::Off);
        p.staged[1][0] = true;
        assert_eq!(p.box_state(0), BoxState::On);
    }

    /// An All-view click is the reference's CheckButton over the tri-state: a grey (mixed) box
    /// COUNTS AS CHECKED, so clicking it — or a fully-checked one — unchecks (disable for every
    /// character), and only a fully-unchecked box enables for all.
    #[test]
    fn an_all_view_click_follows_the_references_checkbutton() {
        let mut p = panel_for(3, vec![addon("A", &[], 11200)]);
        p.staged[1][0] = false; // mixed

        p.click_row(0); // mixed = checked → click unchecks
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

        // In a single character's view the same click is a plain toggle of THEIR bit.
        p.view = Some(2);
        p.click_row(0);
        assert!(p.staged[2][0] && !p.staged[0][0] && !p.staged[1][0]);
    }

    /// Okay/Cancel is a **staged** edit: nothing the panel does touches an enable file until
    /// Okay. `Cancel` is the reference's `ResetAddOns`, and `close` is what implements it.
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

    /// **Okay fans out per character** (1293 — the reference's `SaveAddOns` over the dropdown's
    /// roster): every character whose staged column moved off its open-time baseline gets their
    /// own merge-write, and an untouched character's file is not even created. Hermetic under
    /// `BENILLA_HOME` like the walk's own folder tests (`ui_script/addons.rs`).
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

        // Distinct edits: Alice loses Alpha, Bob loses Beta, Carol touches nothing.
        p.staged[0][0] = false;
        p.staged[1][1] = false;
        p.save_staged();

        // Read back through the same store the world-entry walk resolves against — the two views
        // of one folder that 1197 §3 demands can never disagree.
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
        // And Carol, who has no file at all, is NOT "everything enabled": she takes what the
        // others agree on, which here is nothing — Alpha and Beta are each on for one and off
        // for the other — so both fall to their manifests' `## DefaultState` (2311).
        assert_eq!(read("Carol"), vec![true, true]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **Creating a character does not re-enable everything** — the director's report,
    /// 2026-09-17: *"after creating a new char all addons are enabled again, even after disabling
    /// all of them just before"*.
    ///
    /// The walk is theirs, verbatim: Disable All in the All view, Okay, create a character, open
    /// the list again. What used to happen is the whole bug in one line — the new character had
    /// no `AddOns.txt`, benilla read an absent row as **enabled**, and that all-true column turned
    /// every row in the All view from Off to Mixed, which paints a check and reads as on
    /// (`effective_enabled`). One click there would then have written the all-enabled state back
    /// onto every other character's file: a one-way ratchet.
    ///
    /// The reference does not do this: its enable query `0x51e470` counts only nodes with an
    /// **explicit** entry, so a character with no file contributes no opinion and inherits the
    /// aggregate — here, the unanimous `disabled` the player just saved. Decision 2311.
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

        // The roster before the create, and the player's Disable All + Okay over it.
        let mut p = AddonsPanel::default();
        p.open_for(
            "TestRealm".into(),
            vec!["Onemage".into(), "Onerogue".into()],
        );
        p.set_all(false);
        p.save_staged();
        p.close();

        // …then a character is created, so the roster the panel opens over has one more name and
        // that name has no enable file at all.
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

    /// The other side of the same law: a new character does **not** inherit a disable the roster
    /// never agreed on. One character off, one on, and the newcomer takes the manifest's
    /// `## DefaultState` — so this is not "new characters copy whoever is strictest".
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

    /// The force-load box IS the `checkAddonVersion` CVar inverted, and a toggle is a host
    /// write into the registry (2303) — that is the whole persistence story (the write dirties
    /// the config and reaches the VM's mirror through the outbox); the panel itself only
    /// repaints from its per-frame mirror, rescanning nothing (1292 §2.2).
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

    /// **B274** — the knob is draggable, and the drag is the client's own law.
    ///
    /// This models the press/move arithmetic the drive system runs, in the same band-normalized
    /// units: `cursor_n` is the cursor as a fraction of the bar's height, `thumb_n` the knob's
    /// share of it. Only the cursor read and the `ComputedNode` lookup are left out — everything
    /// that decides where the list lands is here.
    ///
    /// What it pins is the property the reporter is missing (a grab moves the list at all) plus
    /// the two that make it feel right: **offset-preserving** on the knob (grab its bottom edge,
    /// and the list does not jump before you have moved), and **absolute, not accumulated**, so a
    /// long drag cannot walk away from the cursor.
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

        // Press the knob at its BOTTOM edge, at rest (knob at the top of the track).
        let grab = slider_grab(thumb_n, 0.0, thumb_n);
        assert_eq!(grab, thumb_n, "grabbing the knob's bottom keeps that point");
        let f = slider_fraction(thumb_n, grab, 1.0, thumb_n).expect("the bar has travel");
        p.scroll_to(f);
        assert_eq!(p.offset, 0, "the press alone must not jump the list");

        // Drag to the middle of the track; the grabbed point stays under the cursor, so the
        // knob's LEADING edge lands half a knob above it.
        let f = slider_fraction(0.5 + thumb_n * 0.5, grab, 1.0, thumb_n).unwrap();
        p.scroll_to(f);
        assert_eq!(p.offset, max / 2, "mid-track = mid-list");

        // Past the bottom end: pinned, never past it (the clamp `SetValue` does in the client).
        let f = slider_fraction(4.0, grab, 1.0, thumb_n).unwrap();
        p.scroll_to(f);
        assert_eq!(p.offset, max);
        assert_eq!(p.visible().count(), MAX_ROWS, "the last page is still full");

        // Absolute, not accumulated: returning the cursor to where it pressed returns the list
        // to where it started, however far it wandered in between.
        let f = slider_fraction(thumb_n, grab, 1.0, thumb_n).unwrap();
        p.scroll_to(f);
        assert_eq!(p.offset, 0);
    }

    /// A press on the TRACK — off the knob — warps the knob's centre under the cursor and drags
    /// on from there, one gesture. 1.12 does this (`0x789ca0`, and the director's own requirement
    /// in 0989); the glue bar runs the same law as every in-game one.
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

    /// The two directions are inverses at every stop: draw the knob where the offset says
    /// ([`AddonsPanel::thumb_fraction`]), read the offset back out of that position
    /// ([`AddonsPanel::scroll_to`]), and land on the same row. A bar that fails this creeps a row
    /// per grab.
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
