//! The nameplate widgets: the reference's `CGNamePlateFrame` (ctor `0x7cb250`), an anonymous
//! `Button` parented to the `WorldFrame` (`0x7786a3`) at strata WORLD, level 1. Plate addons
//! (`_Nameplates`, `CustomNameplates`, pfUI, ShaguTweaks) find it in `WorldFrame:GetChildren()`
//! and read it positionally: six regions in creation order, then one child, the health bar (1.12
//! has no cast bar). Addons take plates over, so static properties are written once, anchors
//! when the window changes and state when it moves.
//!
//! Also the four verbs that switch plates on and off, which write two bits of the engine's
//! runtime dword `[0xc4da34]` and nothing else.

use std::collections::{HashSet, VecDeque};

use mlua::Lua;

use crate::layout::{Anchor, Anchors, Len, Point};
use crate::order::DrawLayer;
use crate::widget::{FrameHandle, FrameKind, KindState, RegionHandle, RegionKind};

use super::model::Model;
use super::UiScript;
use super::{BlendMode, FontShadow, JustifyV, RegionData, TexCoords};

/// Region 1's art, and the path plate addons identify a plate by.
pub const BORDER_TEXTURE: &str = "Interface\\Tooltips\\Nameplate-Border";
/// Region 2's art; pfUI reads mouseover from `glow:IsShown()` (`nameplates.lua:601`).
pub const GLOW_TEXTURE: &str = "Interface\\Tooltips\\Nameplate-Glow";
/// Region 5's art, the boss and out-of-range skull.
pub const SKULL_TEXTURE: &str = "Interface\\TargetingFrame\\UI-TargetingFrame-Skull";
/// Region 6's art, a 4-column atlas.
pub const RAID_ICON_TEXTURE: &str = "Interface\\TargetingFrame\\UI-RaidTargetingIcons";
/// The health bar's fill, which `GetStatusBarTexture()` returns.
pub const BAR_FILL_TEXTURE: &str = "Interface\\TargetingFrame\\UI-TargetingFrame-BarFill";

/// The plate's level; the bar's is one less, set absolutely (`0x7cb33e` → `0x76a4f0`). The draw
/// walks levels outermost and each level's layers inside (`0x765920`), so the whole bar draws
/// before the border that caps it.
const PLATE_LEVEL: u16 = 1;
const BAR_LEVEL: u16 = 0;

/// The plate geometry that follows from the window rather than the unit, in FrameXML units with
/// the seam scale divided out. Written only when it changes, so an addon's bar anchor survives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlateGeometry {
    pub width: f32,
    pub height: f32,
    /// The health bar, from the plate's BOTTOMLEFT.
    pub bar_off_x: f32,
    pub bar_off_y: f32,
    pub bar_width: f32,
    pub bar_height: f32,
    /// The level text and skull seat, from the plate's BOTTOMRIGHT (x is subtracted, y added).
    pub level_off_x: f32,
    pub level_off_y: f32,
    pub skull_size: f32,
    pub raid_size: f32,
    pub name_height: f32,
    pub level_height: f32,
    /// The text's drop-shadow step, seam divided out like the rest: one logical pixel at any
    /// window size, since the text path multiplies it by the seam.
    pub shadow_offset: f32,
}

/// One plate's per-frame state: everything that can move while the plate is alive.
#[derive(Clone, Debug, PartialEq)]
pub struct PlateState {
    /// The unit's GUID and the plate's identity: a plate is bound to its unit for its life
    /// (`[unit+0xe60]`), and addons cache per-plate state on the frame.
    pub key: u64,
    /// Where the plate's top centre lands, in FrameXML units from the screen's bottom-left; the
    /// plate hangs below it (`0x509ec0`).
    pub top_centre: (f32, f32),
    /// `UNIT_FIELD_HEALTH` and `UNIT_FIELD_MAXHEALTH`, raw: `GetValue()` is the health, not a
    /// fraction (`0x78f5d0`), and `GetMinMaxValues()` is `(0, max)`; plate addons divide.
    pub health: f32,
    pub max_health: f32,
    /// The bar's reaction colour, exact: addons read the unit's reaction from `GetStatusBarColor`.
    pub bar_colour: [f32; 3],
    pub name: String,
    /// The level to show, or `None` before `UNIT_FIELD_LEVEL` arrives: an empty seat, not a skull.
    pub level: Option<u32>,
    /// Show the skull in the level's seat instead of a number.
    pub skull: bool,
    pub level_colour: [f32; 3],
    /// The raid-target mark, 0-based, or `None`.
    pub raid_icon: Option<u8>,
    /// The plate's alpha byte: 255 for the target, dimmed otherwise. Addons find the target
    /// plate by `GetAlpha() == 1`.
    pub alpha: u8,
    /// Mouseover or target: the bar brightens.
    pub lit: bool,
    /// The pointer is on the plate: the name goes yellow (`0x7cb850`) and the glow region shows.
    pub hovered: bool,
}

/// One pooled plate's widgets, in the reference's creation order.
#[derive(Clone, Debug)]
struct Plate {
    frame: FrameHandle,
    border: RegionHandle,
    glow: RegionHandle,
    name: RegionHandle,
    level: RegionHandle,
    skull: RegionHandle,
    raid: RegionHandle,
    bar: FrameHandle,
    /// `None` when the fill's file failed to load (`0x7831b4`).
    fill: Option<RegionHandle>,
    /// Last frame's state, so [`Plate::drive`] writes only what moved; `None` while retired.
    last: Option<PlateState>,
    /// The geometry its anchors were written under, `None` before the first lay-out. Kept per
    /// plate, so a plate the pool grows later is laid out too.
    laid_out: Option<PlateGeometry>,
}

impl Plate {
    fn key(&self) -> Option<u64> {
        self.last.as_ref().map(|s| s.key)
    }
}

/// The plate pool, grown on demand and never shrunk or reordered: addons scan only the children
/// past the count they last saw, so a compacted list would hide every later plate from them.
#[derive(Clone, Debug, Default)]
pub(crate) struct NamePlates {
    plates: Vec<Plate>,
    /// Each live unit's pool slot, by [`PlateState::key`]: the reference's `[unit+0xe60]`.
    assigned: std::collections::HashMap<u64, usize>,
    /// Frame to pool slot, never removed: plates are never destroyed.
    by_frame: std::collections::HashMap<FrameHandle, usize>,
    /// World-render shows after the WORLD batch build; these plates join the next UI tick.
    pending_paint: std::collections::HashSet<FrameHandle>,
    /// Plates this sync hid while they were in the built batch: re-shown in the same pass, they
    /// still draw from the records the batch copied (`0x772fd0`).
    retired_in_batch: std::collections::HashSet<FrameHandle>,
    /// Completed clicks, drained by [`UiScript::take_nameplate_clicks`].
    clicks: Vec<NamePlateClick>,
    /// The hit-test veto (`0x7cba30`). Not the mouse-enabled bit: it leaves `IsMouseEnabled()`
    /// true, where freelook (`0x60f830`) clears the bit.
    hit_test_vetoed: bool,
    geometry: Option<PlateGeometry>,
    /// The verbs' writes to the two script bits, drained by
    /// [`UiScript::take_nameplate_bit_writes`].
    bit_writes: PlateBitWrites,
    /// The fills of the lit plates, which draw boosted ([`lit_fill_boost`]).
    lit_fills: std::collections::HashSet<RegionHandle>,
}

impl NamePlates {
    /// The next UI update precedes the next batch build (`0x764340`, `0x764347`).
    pub(super) fn begin_update(&mut self) {
        self.pending_paint.clear();
    }

    /// Whether `frame` is a plate, whose leave publishes the empty mouseover (`0x7cb8b0`).
    pub(crate) fn is_plate(&self, frame: FrameHandle) -> bool {
        self.by_frame.contains_key(&frame)
    }

    /// Whether the `0x7cba30` veto refuses `frame`: only a plate, only while the veto stands.
    pub(crate) fn vetoes(&self, frame: FrameHandle) -> bool {
        self.hit_test_vetoed && self.by_frame.contains_key(&frame)
    }
}

/// A completed click on a plate. The reference's click slot (`0x7cb910`) selects the unit on the
/// left button (`0x4925d0`) and selects and interacts on the right (`0x492820`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamePlateClick {
    /// The unit whose plate was clicked ([`PlateState::key`]).
    pub key: u64,
    /// `"LeftButton"` or `"RightButton"`.
    pub button: String,
}

/// What a sync runs outside the model borrow: the plates' `Show`/`Hide`, whose transitions fire
/// handlers, and `OnValueChanged` for moved bars (pfUI hooks it, `nameplates.lua:393`).
#[derive(Default)]
struct SyncEffects {
    /// `(plate, shown)` in the order the sync issued them.
    visibility: Vec<(FrameHandle, bool)>,
    values: Vec<(FrameHandle, f32)>,
}

/// Bit `0x1` of `[0xc4da34]`: plates on every unit that is not friendly, set by `ShowNameplates`
/// and cleared by `HideNameplates` (setter `0x6054f0`).
pub const PLATE_BIT_ENEMY: u8 = 0x1;
/// Bit `0x8` of `[0xc4da34]`: plates on friendly units, set by `ShowFriendNameplates` and cleared
/// by `HideFriendNameplates` (setter `0x605510`).
pub const PLATE_BIT_FRIEND: u8 = 0x8;

/// The plate verbs' writes since the last drain. Each setter `or`s or `and`s its bit into the live
/// dword in call order, so per bit the last write wins; bits `0x2` and `0x4` are the frame loop's
/// own latches and no verb reaches them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlateBitWrites {
    set: u8,
    clear: u8,
}

impl PlateBitWrites {
    fn write(&mut self, bit: u8, on: bool) {
        if on {
            self.set |= bit;
            self.clear &= !bit;
        } else {
            self.clear |= bit;
            self.set &= !bit;
        }
    }

    /// `bits` with these writes applied.
    pub fn apply(self, bits: u8) -> u8 {
        (bits | self.set) & !self.clear
    }

    /// No verb ran since the last drain.
    pub fn is_empty(self) -> bool {
        self.set | self.clear == 0
    }
}

/// The four nameplate verbs: `ShowNameplates 0x489450`, `HideNameplates 0x489460`,
/// `ShowFriendNameplates 0x489470`, `HideFriendNameplates 0x489480`. Each reads no argument
/// (`ShowNameplates(false)` still shows), returns zero values and writes its bit of `[0xc4da34]`,
/// which the engine clears at every world entry (`0x60394b`, `0x603952`). There is no getter and
/// no nameplate CVar: FrameXML keeps its own `NAMEPLATES_ON`/`FRIENDNAMEPLATES_ON` and replays
/// them through these verbs.
pub(super) fn install_verbs(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    for (name, bit, on) in [
        ("ShowNameplates", PLATE_BIT_ENEMY, true),
        ("HideNameplates", PLATE_BIT_ENEMY, false),
        ("ShowFriendNameplates", PLATE_BIT_FRIEND, true),
        ("HideFriendNameplates", PLATE_BIT_FRIEND, false),
    ] {
        g.set(
            name,
            // The empty `MultiValue` returns zero values, which is not nil.
            lua.create_function(move |lua, _: mlua::MultiValue| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.nameplates.bit_writes.write(bit, on);
                Ok(mlua::MultiValue::new())
            })?,
        )?;
    }
    Ok(())
}

impl UiScript {
    /// Drain the plate verbs' writes for the engine's `[0xc4da34]`, which the app owns.
    pub fn take_nameplate_bit_writes(&mut self) -> PlateBitWrites {
        let lua = self.lua();
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        std::mem::take(&mut model.nameplates.bit_writes)
    }

    /// Drive the pool from the app's per-frame states, one per shown plate; a unit with no state
    /// has its plate retired (hidden, never destroyed). An unchanged `geometry` costs nothing.
    pub fn sync_nameplates(&mut self, geometry: PlateGeometry, states: &[PlateState]) {
        let lua = self.lua();
        // The constructor's two lookups (`0x7cb3f9`, `0x7cb49b`), for each plate about to be built
        // and none for a built one. Done before the Model is borrowed: a lookup honours an `_G`
        // `__index` (`lua_gettable` at `0x704373`), which may call any verb.
        let mut fonts: VecDeque<Option<String>> = {
            let built = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                // No WorldFrame, no plates, and no lookups for them.
                if model.arena.lookup("WorldFrame").is_none() {
                    return;
                }
                plates_to_build(&model, states)
            };
            (0..2 * built)
                .map(|_| super::font::nameplate_font(lua))
                .collect()
        };
        let effects = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // No WorldFrame (no interface loaded), no plates: no addon walks an invented root.
            let Some(world) = model.arena.lookup("WorldFrame") else {
                return;
            };
            sync(&mut model, world, geometry, states, &mut fonts)
        };
        apply_visibility(lua, effects.visibility);
        for (bar, value) in effects.values {
            super::statusbar::fire_engine_value_changed(lua, bar, value);
        }
    }

    /// The unit whose plate is the hovered frame, the mouseover its OnEnter publishes (`0x7cb850`
    /// → `0x492890`, `[0xb4e2c8]`); an addon frame on a plate takes the hover from it.
    pub fn hovered_nameplate(&self) -> Option<u64> {
        let lua = self.lua();
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let frame = model.mouseover?;
        let slot = *model.nameplates.by_frame.get(&frame)?;
        model.nameplates.plates.get(slot)?.key()
    }

    /// Drain the completed plate clicks, physical or `plate:Click()`, for the app to select from.
    pub fn take_nameplate_clicks(&mut self) -> Vec<NamePlateClick> {
        let lua = self.lua();
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        std::mem::take(&mut model.nameplates.clicks)
    }

    /// The freelook toggle (`0x60f830`): entering camera freelook disables mouse input on every
    /// plate and leaving re-enables it (called from `0x483e80` and `0x483e70`). The disable is
    /// `EnableMouse(false)`'s (`0x76af50(2)`), whose removal tail (`0x764ba0`, flag 0, `0x76af7f`)
    /// ends a hovered plate's hover through its leave (`0x7cb8b0`).
    pub fn set_nameplate_mouse(&mut self, enabled: bool) {
        let lua = self.lua();
        let frames: Vec<FrameHandle> = {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            model.nameplates.plates.iter().map(|p| p.frame).collect()
        };
        for frame in frames {
            super::visibility::set_mouse_enabled(lua, frame, enabled);
        }
    }

    /// The plate hit-test veto (`0x7cba30`): while a ground-targeted spell is armed, clicks fall
    /// through plates to the `WorldFrame`. The caller supplies the first two terms of the
    /// reference's `IsTargeting() && (flag & 0x60) && !0x6e6180()`; the third, a test of the same
    /// word against `0x878e` whose meaning is unsettled, is not applied.
    pub fn set_nameplate_hit_test_veto(&mut self, vetoed: bool) {
        let lua = self.lua();
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.nameplates.hit_test_vetoed = vetoed;
    }

    /// Retire every live plate (`0x608a10` over the active list, as when the master toggle
    /// clears); the app's every early return owes this, since a widget stays up until hidden.
    pub fn retire_nameplates(&mut self) {
        let lua = self.lua();
        let effects = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let mut effects = SyncEffects::default();
            model.nameplates.assigned.clear();
            for i in 0..model.nameplates.plates.len() {
                Plate::retire(&mut model, i, &mut effects);
            }
            effects
        };
        apply_visibility(lua, effects.visibility);
    }

    /// How many plate widgets the pool holds.
    pub fn nameplate_pool_len(&self) -> usize {
        let lua = self.lua();
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        model.nameplates.plates.len()
    }
}

/// Run the plates' `Show`/`Hide` a sync issued, each a full transition with its handlers.
fn apply_visibility(lua: &mlua::Lua, verbs: Vec<(FrameHandle, bool)>) {
    for (frame, shown) in verbs {
        super::visibility::set_shown(lua, frame, shown);
    }
}

/// Record a click on `id` if it is a live plate; every button click comes here.
pub(super) fn note_click(lua: &mlua::Lua, id: u32, button: &str) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let Some(frame) = model.id_to_frame.get(&id).copied() else {
        return;
    };
    let Some(&slot) = model.nameplates.by_frame.get(&frame) else {
        return;
    };
    let Some(key) = model.nameplates.plates.get(slot).and_then(Plate::key) else {
        return;
    };
    model.nameplates.clicks.push(NamePlateClick {
        key,
        button: button.to_string(),
    });
}

/// How many new plates [`sync`] builds for `states`: the units without a plate, less the free
/// slots, which are the retired ones and those the departing units are about to free.
fn plates_to_build(model: &Model, states: &[PlateState]) -> usize {
    let np = &model.nameplates;
    let mut free = np.plates.iter().filter(|p| p.last.is_none()).count();
    free += np
        .assigned
        .iter()
        .filter(|(k, &i)| !states.iter().any(|s| s.key == **k) && np.plates[i].last.is_some())
        .count();
    let arriving: HashSet<u64> = states
        .iter()
        .map(|s| s.key)
        .filter(|k| !np.assigned.contains_key(k))
        .collect();
    arriving.len().saturating_sub(free)
}

/// The per-frame drive: retire departed units, bind the rest, lay out, write what changed.
fn sync(
    model: &mut Model,
    world: FrameHandle,
    geometry: PlateGeometry,
    states: &[PlateState],
    fonts: &mut VecDeque<Option<String>>,
) -> SyncEffects {
    let mut effects = SyncEffects::default();
    let relayout = model.nameplates.geometry != Some(geometry);
    model.nameplates.geometry = Some(geometry);

    model.nameplates.retired_in_batch.clear();
    // Retire first, so a departing unit's slot is free for an arriving one, as the reference
    // destroys then creates through one free list in the same tick.
    let gone: Vec<u64> = model
        .nameplates
        .assigned
        .keys()
        .copied()
        .filter(|k| !states.iter().any(|s| s.key == *k))
        .collect();
    for key in gone {
        if let Some(i) = model.nameplates.assigned.remove(&key) {
            Plate::retire(model, i, &mut effects);
        }
    }

    for state in states {
        let slot = match model.nameplates.assigned.get(&state.key) {
            Some(&i) => i,
            None => {
                // The lowest free slot, else a new one at the tail. Deviation: the reference's
                // free list is FIFO; either keeps every index stable, all an addon can observe.
                let free = (0..model.nameplates.plates.len())
                    .find(|i| model.nameplates.plates[*i].last.is_none());
                let i = free.unwrap_or_else(|| {
                    let plate = Plate::create(model, world, fonts);
                    model.nameplates.plates.push(plate);
                    model.nameplates.plates.len() - 1
                });
                model.nameplates.assigned.insert(state.key, i);
                i
            }
        };
        if model.nameplates.plates[slot].laid_out != Some(geometry) {
            Plate::lay_out(model, slot, geometry);
        }
        Plate::drive(model, slot, world, state, &mut effects);
    }

    // A geometry change reaches retired plates too: addons still read them in the child list.
    if relayout {
        for i in 0..model.nameplates.plates.len() {
            if model.nameplates.plates[i].laid_out != Some(geometry) {
                Plate::lay_out(model, i, geometry);
            }
        }
    }
    effects
}

impl Plate {
    /// Build one plate: the Button, its six regions in the reference's order, and the health bar.
    /// Written once, so a region an addon blanks stays blank.
    fn create(
        model: &mut Model,
        world: FrameHandle,
        fonts: &mut VecDeque<Option<String>>,
    ) -> Plate {
        // Anonymous, as the reference's name is zeroed (`0x76c50b`): plate addons reject a named
        // `WorldFrame` child.
        let frame = model.arena.create(FrameKind::Button, None, Some(world));
        let strata = model
            .arena
            .frame(world)
            .map(|f| f.strata)
            .unwrap_or_default();
        model.arena.set_frame_strata(frame, strata);
        model.arena.set_frame_level(frame, PLATE_LEVEL, true);
        // Mouse-enabled from birth by `CSimpleButton`'s ctor (`0x7786a3`), and registered for
        // both buttons on the up edge (`RegisterForClicks(0x500)`, `0x7cb637`); the arena's
        // Button default is the left alone.
        if let Some(KindState::Button(bs)) = model.arena.frame_mut(frame).map(|f| &mut f.kind_state)
        {
            bs.registered_clicks = ["LeftButtonUp", "RightButtonUp"]
                .into_iter()
                .map(str::to_string)
                .collect();
        }

        // The ctor's blend modes, one `0x7703f0` call per texture: BLEND, but the glow is ADD
        // (`0x7cb36a`), which [`is_unpainted_glow`] depends on.
        let border = texture(
            model,
            frame,
            DrawLayer::Artwork,
            BORDER_TEXTURE,
            BlendMode::Blend,
        );
        let glow = texture(
            model,
            frame,
            DrawLayer::Highlight,
            GLOW_TEXTURE,
            BlendMode::Add,
        );
        // Each FontString looks `NAMEPLATE_FONT` up itself, name then level (`0x7cb3f9`,
        // `0x7cb49b`), at construction only.
        let name = font_string(model, frame, JustifyV::Bottom, fonts.pop_front().flatten());
        let level = font_string(model, frame, JustifyV::Middle, fonts.pop_front().flatten());
        let skull = texture(
            model,
            frame,
            DrawLayer::Overlay,
            SKULL_TEXTURE,
            BlendMode::Blend,
        );
        let raid = texture(
            model,
            frame,
            DrawLayer::Artwork,
            RAID_ICON_TEXTURE,
            BlendMode::Blend,
        );
        // The order above is the ABI addons walk and, as 5875 has no sub-level, the draw order
        // within a layer.

        // The one child, below the plate's level so the border draws over the fill.
        let bar = model.arena.create(FrameKind::StatusBar, None, Some(frame));
        model.arena.set_frame_strata(bar, strata);
        model.arena.set_frame_level(bar, BAR_LEVEL, true);
        // The fill is `SetStatusBarTexture`'s path leg on layer 2 (`0x7cb31a`-`0x7cb329`), so it
        // is installed, flagged to cut its rect and pinned to the bar like any bar texture; a file
        // that fails to load leaves the bar without one. The bar's ctor (`0x7cc340` → `0x782d80`)
        // sets no range and no value.
        super::statusbar::set_bar_path_model(model, bar, BAR_FILL_TEXTURE, DrawLayer::Artwork)
            .expect("live bar");
        let fill = match model.arena.frame(bar).map(|f| &f.kind_state) {
            Some(KindState::StatusBar(sb)) => sb.bar,
            _ => None,
        };

        for rh in [glow, skull, raid] {
            model.set_region_hidden(rh, true);
        }

        // Born retired and hidden: addons tell a live plate from a pooled one by `IsShown()`.
        // Silent, since nothing can have set a handler on a frame not yet handed out.
        model.set_shown_silent(frame, false);

        model
            .nameplates
            .by_frame
            .insert(frame, model.nameplates.plates.len());

        Plate {
            frame,
            border,
            glow,
            name,
            level,
            skull,
            raid,
            bar,
            fill,
            last: None,
            laid_out: None,
        }
    }

    /// Write the window-derived sizes and anchors, on the first drive and a geometry change. The
    /// reference's are real anchors too, through `SetPoint`'s own store (`0x767c70`).
    fn lay_out(model: &mut Model, i: usize, g: PlateGeometry) {
        let p = &model.nameplates.plates[i];
        let (frame, bar) = (p.frame, p.bar);
        let (border, glow, name, level, skull, raid) =
            (p.border, p.glow, p.name, p.level, p.skull, p.raid);
        let plate_id = model.frame_id(frame);

        // The plate's size; its position is per-frame, in `drive`.
        let input = model.layout_inputs.entry(frame).or_default();
        input.width = crate::layout::Len::Script(g.width);
        input.height = crate::layout::Len::Script(g.height);

        for rh in [border, glow] {
            model.region_data.entry(rh).or_default().anchors = Anchors::from([
                Anchor::new(Point::TopLeft, plate_id, Point::TopLeft, 0.0, 0.0),
                Anchor::new(Point::BottomRight, plate_id, Point::BottomRight, 0.0, 0.0),
            ]);
        }

        let bar_input = model.layout_inputs.entry(bar).or_default();
        bar_input.anchors = Anchors::from([Anchor::new(
            Point::BottomLeft,
            plate_id,
            Point::BottomLeft,
            g.bar_off_x,
            g.bar_off_y,
        )]);
        bar_input.width = crate::layout::Len::Script(g.bar_width);
        bar_input.height = crate::layout::Len::Script(g.bar_height);

        // The name's (`0x7cb456`) and level's (`0x7cb7f5`) anchors, which `_Nameplates` checks.
        let name_data = model.region_data.entry(name).or_default();
        name_data.anchors = Anchors::from([Anchor::new(
            Point::Bottom,
            plate_id,
            Point::Center,
            0.0,
            0.0,
        )]);
        name_data.font_height = Some(Len::Script(g.name_height));

        for rh in [name, level] {
            model.region_data.entry(rh).or_default().font_shadow = Some(FontShadow {
                offset: [
                    crate::layout::Len::Script(g.shadow_offset),
                    crate::layout::Len::Script(-g.shadow_offset),
                ],
                color: crate::color::Rgba8::BLACK,
            });
        }

        let level_data = model.region_data.entry(level).or_default();
        level_data.anchors = Anchors::from([Anchor::new(
            Point::Center,
            plate_id,
            Point::BottomRight,
            -g.level_off_x,
            g.level_off_y,
        )]);
        level_data.font_height = Some(Len::Script(g.level_height));
        model.touch_measure(level);

        let skull_data = model.region_data.entry(skull).or_default();
        skull_data.anchors = Anchors::from([Anchor::new(
            Point::Center,
            plate_id,
            Point::BottomRight,
            -g.level_off_x,
            g.level_off_y,
        )]);
        skull_data.size = Some((Len::Script(g.skull_size), Len::Script(g.skull_size)));

        let raid_data = model.region_data.entry(raid).or_default();
        raid_data.anchors =
            Anchors::from([Anchor::new(Point::Right, plate_id, Point::Left, 0.0, 0.0)]);
        raid_data.size = Some((Len::Script(g.raid_size), Len::Script(g.raid_size)));

        // Once per geometry change: each node it wrote is invalidated, as the plate setup's
        // `SetPoint`/`SetAllPoints` with the invalidate flag do (`0x7cb456`..`0x7cb806`).
        model.invalidate_frame(frame, true);
        model.invalidate_frame(bar, true);
        for rh in [border, glow, name, level, skull, raid] {
            model.invalidate_region(rh, true);
        }
        model.nameplates.plates[i].laid_out = Some(g);
    }

    /// The per-frame write, each property only when it moved.
    fn drive(
        model: &mut Model,
        i: usize,
        world: FrameHandle,
        state: &PlateState,
        effects: &mut SyncEffects,
    ) {
        let p = &model.nameplates.plates[i];
        let (frame, bar) = (p.frame, p.bar);
        let (glow, name, level, skull, raid, fill) =
            (p.glow, p.name, p.level, p.skull, p.raid, p.fill);
        let last = p.last.clone();
        let world_id = model.frame_id(world);

        // Re-seated whenever it moves, as the reference does (`0x509ec0`).
        if last
            .as_ref()
            .is_none_or(|l| l.top_centre != state.top_centre)
        {
            let input = model.layout_inputs.entry(frame).or_default();
            input.anchors = Anchors::from([Anchor::new(
                Point::Top,
                world_id,
                Point::BottomLeft,
                state.top_centre.0,
                state.top_centre.1,
            )]);
            // `SetPoint` with the invalidate flag (`0x50a044`).
            model.invalidate_frame(frame, true);
        }

        if last.is_none() {
            // WORLD batches precede the world callback's allocations (`0x765896`, `0x7658e2`).
            if !model.nameplates.retired_in_batch.contains(&frame) {
                model.nameplates.pending_paint.insert(frame);
            }
            effects.visibility.push((frame, true));
        }
        if last.as_ref().is_none_or(|l| l.alpha != state.alpha) {
            model.arena.set_alpha(frame, state.alpha);
        }

        // Raw health on the bar; the fill's reaction colour brightens while lit (`LIT_BOOST`).
        // The assignment drives it once (`0x7cc42c`); after that one watcher per field, HEALTH and
        // MAXHEALTH (`0x7cc53b`, `0x7cc562`), runs for each that changed (`0x4655bb`).
        let runs = match &last {
            None => 1,
            Some(l) => {
                usize::from(l.health != state.health)
                    + usize::from(l.max_health != state.max_health)
            }
        };
        for _ in 0..runs {
            let fires = super::statusbar::set_health(model, bar, state.max_health, state.health);
            effects
                .values
                .extend(fires.into_iter().flatten().map(|v| (bar, v)));
        }
        if last
            .as_ref()
            .is_none_or(|l| l.bar_colour != state.bar_colour || l.lit != state.lit)
        {
            // The boost rides the draw ([`lit_fill_boost`]): it can pass 1, which the packed
            // colour cannot hold.
            if let Some(fill) = fill {
                if state.lit {
                    model.nameplates.lit_fills.insert(fill);
                } else {
                    model.nameplates.lit_fills.remove(&fill);
                }
                let c = state.bar_colour;
                model.region_data.entry(fill).or_default().vertex_color =
                    Some(crate::color::Rgba8::from_f32([c[0], c[1], c[2], 1.0]));
            }
        }

        if last.as_ref().is_none_or(|l| l.name != state.name) {
            // The name goes through `SetText` with its expand flag: `GetUnitName` takes the one
            // stack argument and leaves the `push 1` ahead of it for the call (`0x7cb706`-`0x7cb718`).
            let shown = model.localized_text(state.name.clone());
            model.region_data.entry(name).or_default().text = Some(shown);
            model.touch_measure(name);
            // A new string clears the valid bit and invalidates (`0x771ea4`).
            model.clear_region_valid(name);
        }
        if last.as_ref().is_none_or(|l| l.hovered != state.hovered) {
            let c = if state.hovered {
                crate::color::Rgba8::rgba(255, 255, 0, 255)
            } else {
                crate::color::Rgba8::WHITE
            };
            model
                .region_data
                .entry(name)
                .or_default()
                .set_vertex_color(Some(c));
            // The glow really shows, as addons read mouseover from it; it only paints nothing.
            model.set_region_hidden(glow, !state.hovered);
        }

        if last.as_ref().is_none_or(|l| {
            l.level != state.level || l.skull != state.skull || l.level_colour != state.level_colour
        }) {
            match (state.level, state.skull) {
                // The skull flag wins the seat, whatever the level.
                (_, true) => {
                    model.set_region_hidden(level, true);
                    model.set_region_hidden(skull, false);
                }
                (Some(n), false) => {
                    let c = state.level_colour;
                    // The level goes through `SetText` with its expand flag (`0x7cbcca`).
                    let shown = model.localized_text(n.to_string());
                    let data = model.region_data.entry(level).or_default();
                    data.text = Some(shown);
                    data.set_vertex_color(Some(crate::color::Rgba8::from_f32([
                        c[0], c[1], c[2], 1.0,
                    ])));
                    model.set_region_hidden(level, false);
                    model.set_region_hidden(skull, true);
                    model.touch_measure(level);
                    model.clear_region_valid(level);
                }
                // No level yet and no skull: the seat stays empty.
                (None, false) => {
                    model.set_region_hidden(level, true);
                    model.set_region_hidden(skull, true);
                }
            }
        }

        if last.as_ref().is_none_or(|l| l.raid_icon != state.raid_icon) {
            match state.raid_icon {
                Some(idx) => {
                    let (u0, v0) = (f32::from(idx & 3) * 0.25, f32::from(idx >> 2) * 0.25);
                    let data = model.region_data.entry(raid).or_default();
                    data.tex_coords = Some(TexCoords::Rect([u0, u0 + 0.25, v0, v0 + 0.25]));
                    model.set_region_hidden(raid, false);
                }
                None => model.set_region_hidden(raid, true),
            }
        }

        model.nameplates.plates[i].last = Some(state.clone());
    }

    /// Retire a plate: hide it, never destroy it (`0x608a10`), so it keeps its child-list index and
    /// comes back as the same Lua object addons key their per-plate state by.
    fn retire(model: &mut Model, i: usize, effects: &mut SyncEffects) {
        if model.nameplates.plates[i].last.is_none() {
            return;
        }
        let frame = model.nameplates.plates[i].frame;
        if !model.nameplates.pending_paint.remove(&frame) {
            model.nameplates.retired_in_batch.insert(frame);
        }
        effects.visibility.push((frame, false));
        model.nameplates.plates[i].last = None;
    }
}

/// A newly shown plate and its children miss the WORLD batches already built (`0x765896`).
pub(super) fn awaits_batch(model: &Model, target: crate::order::ZTarget) -> bool {
    if model.nameplates.pending_paint.is_empty() {
        return false;
    }
    let mut frame = match target {
        crate::order::ZTarget::Frame(frame) => Some(frame),
        crate::order::ZTarget::Region(region) => model.arena.region(region).map(|r| r.owner),
    };
    while let Some(owner) = frame {
        if model.nameplates.pending_paint.contains(&owner) {
            return true;
        }
        frame = model.arena.frame(owner).and_then(|f| f.parent);
    }
    false
}

/// One of the plate's textures, with its path and the blend mode the ctor sets (`0x7703f0`).
fn texture(
    model: &mut Model,
    frame: FrameHandle,
    layer: DrawLayer,
    path: &str,
    blend: BlendMode,
) -> RegionHandle {
    let rh = model
        .arena
        .create_region(frame, RegionKind::Texture, layer, 0)
        .expect("live plate");
    model.set_texel(rh, super::texel::TexSet::File(path.to_string()));
    let data = model.region_data.entry(rh).or_default();
    data.blend = blend;
    rh
}

/// One of the plate's two FontStrings, OVERLAY as the ctor re-layers them (`0x7cb438`,
/// `0x7cb4ea`), with the `NAMEPLATE_FONT` lookup's answer: `None` sets no font, as `SetFont`
/// rejects the empty path (`0x44d05c`). `justify_v` seats the name by its ink: our shaper's Friz
/// line box is ascent-heavy and would drop it ~8 px below where the reference's metrics put it.
fn font_string(
    model: &mut Model,
    frame: FrameHandle,
    justify_v: JustifyV,
    font: Option<String>,
) -> RegionHandle {
    let rh = model
        .arena
        .create_region(frame, RegionKind::FontString, DrawLayer::Overlay, 0)
        .expect("live plate");
    let data = model.region_data.entry(rh).or_default();
    // The engine's own load (`0x7714f0`) clears no inherit bit, so a later `SetFontObject`
    // repaints the plate's text as any FontString's.
    data.font_path = font;
    data.justify.set_v(justify_v);
    rh
}

/// Whether a text's owner is a plate or a chat bubble. Their rects snap to device pixels as they
/// slide, and their text must follow that rect, not the UI's coarser grid, or it jumps. Asked of
/// the frame so an addon's strings on one get it too; a string on the plate's health bar does not.
pub(super) fn is_world_seated(model: &Model, frame: FrameHandle) -> bool {
    model.nameplates.by_frame.contains_key(&frame) || super::chat_bubble::is_bubble(model, frame)
}

/// Whether this is the plate's own glow art on its ADD mode, which the quad walk
/// ([`super::extract`]) skips. Deviation: the reference adds this rim over the bar; we brighten
/// the bar ([`LIT_BOOST`]), because the rim reads as hard edge lines in our linear-blending
/// pipeline. The art has no alpha and is mostly black, so as BLEND it would be an opaque box.
pub(super) fn is_unpainted_glow(data: &RegionData) -> bool {
    data.blend == BlendMode::Add && data.tex.path().as_deref() == Some(GLOW_TEXTURE)
}

/// The draw-time multiply of plate fill `rh`'s tint: [`LIT_BOOST`] while its plate is lit.
pub(super) fn lit_fill_boost(model: &Model, rh: RegionHandle) -> Option<f32> {
    model
        .nameplates
        .lit_fills
        .contains(&rh)
        .then_some(LIT_BOOST)
}

/// The lit bar's brighten, a multiply of the fill tint in encoded sRGB like the client's modulate:
/// 255/215 takes the fill texture's encoded peak exactly to white, so no gradient row clips.
const LIT_BOOST: f32 = 255.0 / 215.0;
