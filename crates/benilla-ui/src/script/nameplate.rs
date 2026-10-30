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

/// One plate's state as the frame's gate reads it (`0x60f600` from `0x607ef9`): the bind's for a
/// unit it gives a plate (`0x7cb6d0`), and the frame's own fields for every plate it keeps: the
/// seat (`0x608ce0`), the alpha (`0x7cb9f9`), the lit bar and the hover.
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

impl PlateState {
    /// What the bind reads of the unit.
    fn unit(&self) -> PlateUnit {
        PlateUnit {
            health: self.health,
            max_health: self.max_health,
            bar_colour: self.bar_colour,
            name: Some(self.name.clone()),
            level: self.level,
            skull: self.skull,
            level_colour: self.level_colour,
            raid_icon: self.raid_icon,
        }
    }
}

/// What a plate's bind reads of its unit (`0x7cb6d0`), and what the unit's callbacks write again
/// inside their packets, each its own part: the bar's colour (`0x7cbaa0`), the level and skull
/// (`0x7cbb40`) and the raid icon (`0x7cbe20`). The name is the bind's alone (`0x7cb70c`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlateUnit {
    /// `UNIT_FIELD_HEALTH` and `UNIT_FIELD_MAXHEALTH`, as [`PlateState::health`].
    pub health: f32,
    pub max_health: f32,
    pub bar_colour: [f32; 3],
    /// `GetUnitName`'s answer (`0x609210`); `None` for its miss, the `UNKNOWNOBJECT` text the VM
    /// holds (`0x609324`).
    pub name: Option<String>,
    pub level: Option<u32>,
    pub skull: bool,
    pub level_colour: [f32; 3],
    pub raid_icon: Option<u8>,
}

/// The unit `guid` as a plate's callback reads it as it runs, asked of the lent host; no answer
/// for no unit held under the guid (`0x468460`, mask 8). `party`, when set, is the four party
/// slots as they stood where the callback ran, for one that runs inside a handler that is still
/// writing them (`ClearParty`, `AddPartyMember`), where the host's stand as the handler left them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlateUnitQuery {
    pub guid: u64,
    pub party: Option<[u64; 4]>,
}

impl super::live::LiveQuery for PlateUnitQuery {
    type Answer = PlateUnit;
}

/// The gate `0x60f600` on `guid` as it runs inside a packet, from `InvalidateNameDesc`
/// (`0x60706a`): the unit as the bind reads it when the gate gives it a plate (`0x60f7d9`), else
/// `None`, which releases one (`0x60f742`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlateGateQuery(pub u64);

impl super::live::LiveQuery for PlateGateQuery {
    type Answer = Option<PlateUnit>;
}

/// A bound plate: its unit and the frame's fields as the last sync wrote them, each `None` until
/// the first sync after a bind inside a packet writes it.
#[derive(Clone, Debug)]
struct Shown {
    /// [`PlateState::key`], `[unit+0xe60]`'s back link.
    key: u64,
    /// The point the frame's seat anchored the plate at (`0x608ce0` → `0x509ec0`): a bind inside
    /// a packet stores its point (`0x60872f`) and leaves the anchor to the frame's seat.
    seat: Option<(f32, f32)>,
    alpha: Option<u8>,
    lit: Option<bool>,
    hovered: Option<bool>,
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
    /// The name's flash timer `+0x514`, in milliseconds: [`FLASH_MS`] when a hit arms it
    /// (`0x7cba77`), 0 at every bind (`0x7cb80e`), counted down by the plate's update; a negative
    /// timer is idle, and while it is above 0 the hover leaves the name red.
    flash_ms: i32,
    /// The bound unit and what the frame last wrote, so [`Plate::frame`] writes only what moved;
    /// `None` while retired.
    shown: Option<Shown>,
    /// `[plate+0x4e8]`, the unit last bound ([`PlateState::key`]): 0 from the constructor
    /// (`0x7cb2a6`), written by each bind (`0x7cb6e0`) and left by the retire (`0x608a10`), so a
    /// hidden pooled plate's click still names its last unit (`0x7cb91c`).
    guid: u64,
    /// `[bar+0x338]`, the unit the bar's two watchers watch (`0x7cc510`): set by each bind
    /// (`0x7cc405`) and kept by the retire, which unwatches nothing, so a retired plate's bar
    /// follows its last unit until a bind moves it (`0x7cc3e2` → `0x7cc5d0`) or the unit's
    /// object, which holds the watch list, goes; 0 for none.
    bar_unit: u64,
    /// The link of the bar's watchers on its unit's list ([`super::unit_watch`]), which orders
    /// them among the unit's other watchers (`0x467f65`-`0x467f76`).
    bar_watched_at: u64,
    /// The geometry its anchors were written under, `None` before the first lay-out. Kept per
    /// plate, so a plate the pool grows later is laid out too.
    laid_out: Option<PlateGeometry>,
}

impl Plate {
    fn key(&self) -> Option<u64> {
        self.shown.as_ref().map(|s| s.key)
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
    /// The hit-test veto (`0x7cba30`). Not the mouse-enabled bit: it leaves `IsMouseEnabled()`
    /// true, where freelook (`0x60f830`) clears the bit.
    hit_test_vetoed: bool,
    geometry: Option<PlateGeometry>,
    /// The fills of the lit plates, which draw boosted ([`lit_fill_boost`]).
    lit_fills: std::collections::HashSet<RegionHandle>,
    /// Bit `0x4` of `[0xc4da34]` (`0x605550`): the next sync refreshes the level and skull of each
    /// plate it keeps (`0x60875f` → `0x7cbb40`), then the latch clears (`0x48362e`).
    levels_due: bool,
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

    /// The plate frames, for the update walk to visit: each runs [`update`] ahead of its
    /// `OnUpdate` (`0x7cb950` → `0x76b2c0`).
    pub(super) fn frames(&self) -> impl Iterator<Item = FrameHandle> + '_ {
        self.by_frame.keys().copied()
    }

    /// Whether the `0x7cba30` veto refuses `frame`: only a plate, only while the veto stands.
    pub(crate) fn vetoes(&self, frame: FrameHandle) -> bool {
        self.hit_test_vetoed && self.by_frame.contains_key(&frame)
    }
}

/// The flash a hit starts, in milliseconds (`0x7cba77`, `0x1388`).
const FLASH_MS: i32 = 5000;

/// The name colour while it flashes (`0x7cba88`, `0xffff0000`), and at rest: yellow while the glow
/// shows, white otherwise (`0x7cb89e`, `0x7cb991`, `0x7cb8ed`).
const FLASH_RED: crate::color::Rgba8 = crate::color::Rgba8::rgba(255, 0, 0, 255);
const NAME_YELLOW: crate::color::Rgba8 = crate::color::Rgba8::rgba(255, 255, 0, 255);

/// The plate's `OnUpdate` override (`0x7cb950`), the flash half: a timer at or above 0 loses
/// `ftol(elapsed * 1000)` (`0x7cb962`-`0x7cb976`), and once it is 0 or less the name takes its
/// resting colour, by whether the glow shows (`0x7cb97e`-`0x7cb9ac`). A negative timer does
/// nothing, so the bind's 0 resets the name on the first update and then idles. It runs ahead of
/// the plate's own `OnUpdate` script, which the override tail-calls (`0x76b2c0`).
pub(super) fn update(model: &mut Model, frame: FrameHandle, elapsed: f32) {
    let Some(&slot) = model.nameplates.by_frame.get(&frame) else {
        return;
    };
    let plate = &mut model.nameplates.plates[slot];
    if plate.flash_ms < 0 {
        return;
    }
    plate.flash_ms -= super::binding_abi::ftol(f64::from(elapsed) * 1000.0);
    if plate.flash_ms > 0 {
        return;
    }
    let colour = if plate
        .shown
        .as_ref()
        .is_some_and(|s| s.hovered == Some(true))
    {
        NAME_YELLOW
    } else {
        crate::color::Rgba8::WHITE
    };
    let name = plate.name;
    model
        .region_data
        .entry(name)
        .or_default()
        .set_vertex_color(Some(colour));
}

/// A plate's click as its slot (`CGNamePlateFrame::OnClick 0x7cb910`) hands it to
/// `ClickObjectByAction` (`0x7cb92a` → `0x4949f0`), which the host applies inside the click
/// ([`super::ScriptCall::PlateClick`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlateClick {
    /// The plate's unit, its guid at `[plate+0x4e8]` (`0x7cb91c`, [`PlateState::key`]).
    pub guid: u64,
    /// What the button's bit asks of `0x4949f0`.
    pub action: PlateAction,
    /// A Lua `Click()`, which passes the slot 1 (`0x7827b2`) where the mouse passes 0
    /// (`0x779280`, `0x7793a4`): no press of the mouse latched the unit, so the right button's
    /// interact classes it as it stands.
    pub scripted: bool,
    /// The gate's tag 2 (`0x6e5ba5`), asked where the click ran, for the select's bind to a
    /// standing targeting cursor and for the casts the interact tries.
    pub may_cast: bool,
}

/// The two button bits `0x4949f0` acts on (`0x494a15`, `0x494a2c`); any other does nothing
/// (`0x494a3e`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlateAction {
    /// Bit 1, the left button: `SelectObjectByClick` (`0x494a22` → `0x4925d0`).
    Select,
    /// Bit 4, the right button: `InteractWithObjectByClick` (`0x494a39` → `0x492820`).
    Interact,
}

/// What a sync runs outside the model borrow, in the order it issued them: the plates' `Show`
/// and `Hide`, whose transitions fire handlers, and `OnValueChanged` for moved bars (pfUI hooks
/// it, `nameplates.lua:393`). A bind's bar fires inside it, ahead of the plate's show
/// (`0x608707` before `0x60871e`).
#[derive(Default)]
struct SyncEffects(Vec<Effect>);

enum Effect {
    Shown(FrameHandle, bool),
    Value(FrameHandle, f32),
}

impl SyncEffects {
    fn shown(&mut self, frame: FrameHandle, shown: bool) {
        self.0.push(Effect::Shown(frame, shown));
    }

    fn values(&mut self, bar: FrameHandle, fires: impl IntoIterator<Item = Option<f32>>) {
        self.0
            .extend(fires.into_iter().flatten().map(|v| Effect::Value(bar, v)));
    }

    /// Run them, each a full transition or handler.
    fn apply(self, lua: &Lua) {
        for effect in self.0 {
            match effect {
                Effect::Shown(frame, shown) => super::visibility::set_shown(lua, frame, shown),
                Effect::Value(bar, v) => super::statusbar::fire_engine_value_changed(lua, bar, v),
            }
        }
    }
}

/// Bit `0x1` of `[0xc4da34]`: plates on every unit that is not friendly, set by `ShowNameplates`
/// and cleared by `HideNameplates` (setter `0x6054f0`).
pub const PLATE_BIT_ENEMY: u8 = 0x1;
/// Bit `0x8` of `[0xc4da34]`: plates on friendly units, set by `ShowFriendNameplates` and cleared
/// by `HideFriendNameplates` (setter `0x605510`).
pub const PLATE_BIT_FRIEND: u8 = 0x8;

/// One plate verb's write of its bit of `[0xc4da34]`, which the host applies inside the call
/// ([`super::ScriptCall::Nameplates`]): the setter `or`s or `and`s the bit into the live dword.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlateBitWrite {
    pub bit: u8,
    pub on: bool,
}

impl PlateBitWrite {
    /// `bits` with this write applied.
    pub fn apply(self, bits: u8) -> u8 {
        if self.on {
            bits | self.bit
        } else {
            bits & !self.bit
        }
    }
}

/// The plate writes a harness's VM kept ([`UiScript::take_nameplate_bit_writes`]), folded in call
/// order, so per bit the last write wins; bits `0x2` and `0x4` are the frame loop's own latches
/// and no verb reaches them.
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

    /// No verb ran since the last take.
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
                super::calls::submit(
                    lua,
                    super::ScriptCall::Nameplates(PlateBitWrite { bit, on }),
                )?;
                Ok(mlua::MultiValue::new())
            })?,
        )?;
    }
    Ok(())
}

impl UiScript {
    /// The plate writes a harness's VM kept ([`Self::keep_calls`]), folded in call order.
    pub fn take_nameplate_bit_writes(&mut self) -> PlateBitWrites {
        let mut writes = PlateBitWrites::default();
        for write in self.take_calls_where(|c| match c {
            super::ScriptCall::Nameplates(write) => Some(*write),
            _ => None,
        }) {
            writes.write(write.bit, write.on);
        }
        writes
    }

    /// The frame's gate over the pool (`0x60f600` from `0x607ef9`), one state per unit it keeps
    /// a plate on: a unit with no plate is bound (`0x6086e0` → `0x7cb6d0`), a unit with one has
    /// only the frame's fields moved, the seat, the alpha, the lit bar and the hover, and its
    /// level too while the latch asks ([`Self::latch_nameplate_levels`]); a unit with no state
    /// has its plate retired (hidden, never destroyed). What a bound plate shows of its unit
    /// moves inside the packets, at its callbacks ([`Self::nameplate_health`] and the rest). An
    /// unchanged `geometry` costs nothing.
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
            fonts_for(lua, built)
        };
        let effects = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // No WorldFrame (no interface loaded), no plates: no addon walks an invented root.
            let Some(world) = model.arena.lookup("WorldFrame") else {
                return;
            };
            sync(&mut model, world, geometry, states, &mut fonts)
        };
        effects.apply(lua);
    }

    /// Set the level latch (bit `0x4` of `[0xc4da34]`, `0x605550`): the active player's level
    /// moved (`0x5edb93`), or its drunkenness (`0x5e2b42`), so the next sync refreshes each kept
    /// plate's level and skull, whose colour, threshold and shown level read them.
    pub fn latch_nameplate_levels(&mut self) {
        self.model_mut().nameplates.levels_due = true;
    }

    /// The bar's watcher `0x7cc570` on a change of `guid`'s `UNIT_FIELD_HEALTH` or
    /// `UNIT_FIELD_MAXHEALTH`, inside its packet, on each plate whose bar watches the unit, in the
    /// order their watches were linked, with no other watcher among them
    /// ([`Self::unit_health_watchers`]).
    pub fn nameplate_health(&mut self, guid: u64, health: f32, max: f32) {
        let watch = super::HealthWatch {
            bar: Some((health, max)),
            tooltip: None,
        };
        self.unit_health_watchers(guid, watch, |_| {});
    }

    /// One plate bar's watcher `0x7cc570`: the range (`0x7cc5a8` → `0x783270`), which re-applies
    /// the held value, then the value (`0x7cc5be`), each firing `OnValueChanged` where it moves.
    pub(super) fn bar_watch(&mut self, bar: FrameHandle, health: f32, max: f32) {
        let lua = self.lua();
        for write in [BarWrite::Range(max), BarWrite::Value(health)] {
            let fired = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                match model.arena.frame_mut(bar).map(|f| &mut f.kind_state) {
                    Some(KindState::StatusBar(sb)) => match write {
                        BarWrite::Range(max) => sb.set_min_max(0.0, max),
                        BarWrite::Value(v) => sb.set_value(v),
                    },
                    _ => None,
                }
            };
            if let Some(v) = fired {
                super::statusbar::fire_engine_value_changed(lua, bar, v);
            }
        }
    }

    /// `RefreshNameDesc`'s plate leg (`0x607080` → `0x7cbaa0`) on `guid`'s plate, if it has one
    /// (`0x60708e`), inside its packet: the bar's colour from the unit's reaction as it stands
    /// ([`PlateUnitQuery`]). Its callers: the health watcher (`0x60471b`), the reaction's refresh
    /// of a unit, its summon and its charm (`0x606e3c`, `0x606e8a`, `0x606ecf`) and the active
    /// player's sweep (`0x606fa2`).
    pub fn refresh_nameplate_colour(&mut self, guid: u64) {
        self.refresh_bound(PlateUnitQuery { guid, party: None }, |model, i, unit| {
            Plate::colour(model, i, unit.bar_colour)
        });
    }

    /// [`Self::refresh_nameplate_colour`] from a party slot writer, the reaction read with the
    /// slots `party` as they stood there: `AddPartyMember`'s on the member it seats (`0x4e83dd`)
    /// and `ClearParty`'s on each it empties, ahead of the slot's zeroing (`0x4e8568`, `0x4e858c`).
    pub fn refresh_nameplate_colour_in_party(&mut self, guid: u64, party: [u64; 4]) {
        let query = PlateUnitQuery {
            guid,
            party: Some(party),
        };
        self.refresh_bound(query, |model, i, unit| {
            Plate::colour(model, i, unit.bar_colour)
        });
    }

    /// `OnLevelChanged`'s plate leg (`0x60cdd0` at `0x60ce5b` → `0x7cbb40`) on `guid`'s plate, if
    /// it has one (`0x60ce58`), inside its packet: the level and skull as they stand.
    pub fn refresh_nameplate_level(&mut self, guid: u64) {
        self.refresh_bound(PlateUnitQuery { guid, party: None }, Plate::level);
    }

    /// `f` on the plate bound to the query's unit, with the unit as the lent host reads it now.
    fn refresh_bound(
        &mut self,
        query: PlateUnitQuery,
        f: impl FnOnce(&mut Model, usize, &PlateUnit),
    ) {
        let guid = query.guid;
        let lua = self.lua();
        if bound_slot(&lua.app_data_ref::<Model>().expect("model app_data"), guid).is_none() {
            return;
        }
        let Some(unit) = super::live::ask(lua, &query) else {
            return;
        };
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if let Some(i) = bound_slot(&model, guid) {
            f(&mut model, i, &unit);
        }
    }

    /// `0x608a90` on each unit with a plate: its plate's raid icon from the board's slot for it as
    /// it stands (`0x608aa2` → `0x4bb190`, `0x608ab3` → `0x7cbe20`), as the raid target handler
    /// runs it on each unit whose slot moved (`0x4ba28a`, `0x4ba308`, `0x4ba32f`) and the board's
    /// clear on each it held (`0x4ba5cb`), ahead of `RAID_TARGET_UPDATE`. A unit whose slot did
    /// not move reads the icon it shows.
    pub fn refresh_nameplate_raid_icons(&mut self) {
        let units: Vec<u64> = self
            .model_ref()
            .nameplates
            .assigned
            .keys()
            .copied()
            .collect();
        for guid in units {
            self.refresh_bound(PlateUnitQuery { guid, party: None }, |model, i, unit| {
                Plate::raid_icon(model, i, unit.raid_icon)
            });
        }
    }

    /// The gate `0x60f600` on `guid` inside its packet, from `InvalidateNameDesc` (`0x607050` at
    /// `0x60706a`), as the lent host judges it ([`PlateGateQuery`]): a unit it gives a plate and
    /// has none is bound (`0x6086e0`) and shown, its seat left for the frame's layout
    /// (`0x608ce0`); one it keeps a plate on has its level and skull refreshed while the latch is
    /// set (`0x60875f` → `0x7cbb40`), as the frame's gate does; one it refuses has its plate
    /// released (`0x60f742`). With no host to ask, nothing moves.
    pub fn gate_nameplate(&mut self, guid: u64) {
        let lua = self.lua();
        let Some(verdict) = super::live::ask(lua, &PlateGateQuery(guid)) else {
            return;
        };
        let Some(unit) = verdict else {
            self.release_nameplate(guid);
            return;
        };
        let lua = self.lua();
        let built = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.arena.lookup("WorldFrame").is_none() || model.nameplates.geometry.is_none() {
                return;
            }
            if model.nameplates.assigned.contains_key(&guid) {
                if let Some(i) = bound_slot(&model, guid).filter(|_| model.nameplates.levels_due) {
                    Plate::level(&mut model, i, &unit);
                }
                return;
            }
            usize::from(free_slot(&model).is_none())
        };
        let mut fonts = fonts_for(lua, built);
        let name = unit
            .name
            .clone()
            .unwrap_or_else(|| self.unknown_object_text());
        let lua = self.lua();
        let effects = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let (Some(world), Some(geometry)) =
                (model.arena.lookup("WorldFrame"), model.nameplates.geometry)
            else {
                return;
            };
            let mut effects = SyncEffects::default();
            let i = take_slot(&mut model, world, guid, &mut fonts);
            if model.nameplates.plates[i].laid_out != Some(geometry) {
                Plate::lay_out(&mut model, i, geometry);
            }
            Plate::bind(&mut model, i, guid, &unit, name, &mut effects);
            effects
        };
        effects.apply(lua);
    }

    /// `ReleaseNamePlate` (`0x608a10`) on `guid`'s plate, if it has one, inside its packet: hidden
    /// and returned to the pool, its guid and its bar's watch kept. The name and creature answers
    /// release it ahead of their name update (`0x5fb86e`, `0x6046c1`).
    pub fn release_nameplate(&mut self, guid: u64) {
        let lua = self.lua();
        let effects = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let mut effects = SyncEffects::default();
            if let Some(i) = model.nameplates.assigned.remove(&guid) {
                Plate::retire(&mut model, i, &mut effects);
            }
            effects
        };
        effects.apply(lua);
    }

    /// The unit `guid` leaves the world inside its packet: its release frees its plate
    /// (`CGUnit_C::Release` at `0x5fbba9`, the destructor at `0x5fb620`), and its object, which
    /// holds the bar and tooltip watches, goes with them, so neither follows it again.
    pub fn nameplate_unit_gone(&mut self, guid: u64) {
        self.release_nameplate(guid);
        let mut model = self.model_mut();
        let tooltips: Vec<_> = model
            .unit_watches
            .linked_tooltips()
            .filter(|&h| super::tooltip_unit::shows(&model, h, guid))
            .collect();
        model.unit_watches.gone(guid, &tooltips);
        for plate in &mut model.nameplates.plates {
            if plate.bar_unit == guid {
                plate.bar_unit = 0;
            }
        }
    }

    /// A hit on the unit `key`: `FlashNamePlate` (`0x6122e0` → `0x7cba70`) starts the flash on the
    /// unit's plate, if it has one (`[unit+0xe60]`, `0x6122e6`): the timer to 5000 ms and the
    /// name red, whatever the hover. Called inside the packet for a health drop (`0x6046f0` at
    /// `0x604764`), a direct spell hit (`0x62cd80` at `0x62ce43`) and environmental damage
    /// (`0x62aac0` at `0x62abac`).
    pub fn flash_nameplate(&mut self, key: u64) {
        let lua = self.lua();
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let Some(&slot) = model.nameplates.assigned.get(&key) else {
            return;
        };
        let plate = &mut model.nameplates.plates[slot];
        if plate.shown.is_none() {
            return;
        }
        plate.flash_ms = FLASH_MS;
        let name = plate.name;
        model
            .region_data
            .entry(name)
            .or_default()
            .set_vertex_color(Some(FLASH_RED));
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

    /// Apply `click` through the host lent now, as the plate's slot does inside its click
    /// ([`super::ScriptCall::PlateClick`]): for a test of the click's legs without the widget.
    pub fn submit_plate_click(&mut self, click: PlateClick) {
        submit(self.lua(), click);
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
        effects.apply(lua);
    }

    /// How many plate widgets the pool holds.
    pub fn nameplate_pool_len(&self) -> usize {
        let lua = self.lua();
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        model.nameplates.plates.len()
    }
}

/// One of the bar watcher's two writes (`0x7cc570`).
#[derive(Clone, Copy)]
enum BarWrite {
    Range(f32),
    Value(f32),
}

/// Each plate bar watching `guid` ([`Plate::bar_unit`]), a retired one's included, by its link.
pub(super) fn bars_watching(model: &Model, guid: u64) -> Vec<(u64, FrameHandle)> {
    model
        .nameplates
        .plates
        .iter()
        .filter(|p| p.bar_unit == guid)
        .map(|p| (p.bar_watched_at, p.bar))
        .collect()
}

/// The slot of the plate bound to `guid` (`[unit+0xe60]`), if it has one.
fn bound_slot(model: &Model, guid: u64) -> Option<usize> {
    let &i = model.nameplates.assigned.get(&guid)?;
    model.nameplates.plates[i].shown.is_some().then_some(i)
}

/// The lowest free slot. Deviation: the reference's free list is FIFO (`0x6087f0`, `0x608a5a`);
/// either keeps every index stable, all an addon can observe of the pool's order.
fn free_slot(model: &Model) -> Option<usize> {
    (0..model.nameplates.plates.len()).find(|&i| model.nameplates.plates[i].shown.is_none())
}

/// `0x6087f0` for `guid`: the lowest free slot, else a plate built at the tail, assigned to it.
fn take_slot(
    model: &mut Model,
    world: FrameHandle,
    guid: u64,
    fonts: &mut VecDeque<Option<String>>,
) -> usize {
    let i = free_slot(model).unwrap_or_else(|| {
        let plate = Plate::create(model, world, fonts);
        model.nameplates.plates.push(plate);
        model.nameplates.plates.len() - 1
    });
    model.nameplates.assigned.insert(guid, i);
    i
}

/// The constructor's two lookups (`0x7cb3f9`, `0x7cb49b`) for each of `built` plates, name then
/// level, done before the Model is borrowed: a lookup honours an `_G` `__index` (`lua_gettable`
/// at `0x704373`), which may call any verb.
fn fonts_for(lua: &Lua, built: usize) -> VecDeque<Option<String>> {
    (0..2 * built)
        .map(|_| super::font::nameplate_font(lua))
        .collect()
}

/// A plate's click slot (`0x7cb910`) ahead of the button's own click: `0x4949f0` on the plate's
/// unit (`0x7cb92a`), applied inside the click, so the plate's `OnClick` (`0x7cb936` →
/// `0x779540`) and the line after a `Click()` read what it did. Every run of the slot comes here, a
/// press's (`0x779285`) or a release's (`0x7793a9`), the mouse's or `Click()`'s, on an enabled
/// plate or not: `RunClick`'s gate (`0x779552`) is past it, and on a retired plate, whose guid is
/// still its last unit's. A frame that is no plate, a plate never bound, or any button but the
/// left and the right, does nothing.
pub(super) fn click(lua: &mlua::Lua, frame: FrameHandle, button: &str, scripted: bool) {
    let action = match button {
        "LeftButton" => PlateAction::Select,
        "RightButton" => PlateAction::Interact,
        _ => return,
    };
    let guid = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let Some(&slot) = model.nameplates.by_frame.get(&frame) else {
            return;
        };
        match model.nameplates.plates.get(slot).map(|p| p.guid) {
            Some(guid) if guid != 0 => guid,
            _ => return,
        }
    };
    let may_cast = super::protected::select_may_cast(lua);
    submit(
        lua,
        PlateClick {
            guid,
            action,
            scripted,
            may_cast,
        },
    );
}

/// Hand `click` to the host, which applies it before this returns.
fn submit(lua: &mlua::Lua, click: PlateClick) {
    if let Err(e) = super::calls::submit(lua, super::ScriptCall::PlateClick(click)) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_lua_error(&e);
    }
}

/// How many new plates [`sync`] builds for `states`: the units without a plate, less the free
/// slots, which are the retired ones and those the departing units are about to free.
fn plates_to_build(model: &Model, states: &[PlateState]) -> usize {
    let np = &model.nameplates;
    let mut free = np.plates.iter().filter(|p| p.shown.is_none()).count();
    free += np
        .assigned
        .iter()
        .filter(|(k, &i)| !states.iter().any(|s| s.key == **k) && np.plates[i].shown.is_some())
        .count();
    let arriving: HashSet<u64> = states
        .iter()
        .map(|s| s.key)
        .filter(|k| !np.assigned.contains_key(k))
        .collect();
    arriving.len().saturating_sub(free)
}

/// The frame's gate over the pool: retire departed units, bind arriving ones, lay out, and move
/// the frame's fields of every kept plate, with its level while the latch asks.
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

    let levels_due = std::mem::take(&mut model.nameplates.levels_due);
    for state in states {
        let kept = model.nameplates.assigned.get(&state.key).copied();
        let slot = kept.unwrap_or_else(|| take_slot(model, world, state.key, fonts));
        if model.nameplates.plates[slot].laid_out != Some(geometry) {
            Plate::lay_out(model, slot, geometry);
        }
        match kept {
            // `0x6086e0` on a unit with a plate: the level while the latch asks (`0x60875f` →
            // `0x7cbb40`), and the point stored for the seat.
            Some(_) if levels_due => Plate::level(model, slot, &state.unit()),
            Some(_) => {}
            // A new plate's bind (`0x608707`), then its show (`0x60871e`). WORLD batches precede
            // the world callback's allocations (`0x765896`, `0x7658e2`).
            None => {
                let frame = model.nameplates.plates[slot].frame;
                if !model.nameplates.retired_in_batch.contains(&frame) {
                    model.nameplates.pending_paint.insert(frame);
                }
                Plate::bind(
                    model,
                    slot,
                    state.key,
                    &state.unit(),
                    state.name.clone(),
                    &mut effects,
                );
            }
        }
        Plate::frame(model, slot, world, state);
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
            flash_ms: 0,
            shown: None,
            guid: 0,
            bar_unit: 0,
            bar_watched_at: 0,
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

        // The plate's size; its position is the frame's seat, in `frame`.
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

    /// The bind `0x7cb6d0` of plate `i` to the unit `key`, at the frame's gate or inside a packet:
    /// the guid (`0x7cb6e0`), the name white (`0x7cb701`) with `GetUnitName`'s text (`0x7cb70c`),
    /// the bar bound to the unit and set from it (`0x7cb724` → `0x7cc3d0`), the flash timer 0
    /// (`0x7cb80e`), the bar's colour (`0x7cb818`), the level (`0x7cb820`) and the raid icon
    /// (`0x7cb837`); then the show (`0x60871e`). The seat is the frame's ([`Self::frame`]).
    fn bind(
        model: &mut Model,
        i: usize,
        key: u64,
        unit: &PlateUnit,
        name: String,
        effects: &mut SyncEffects,
    ) {
        let binds = model.unit_watches.link();
        let p = &mut model.nameplates.plates[i];
        p.guid = key;
        p.flash_ms = 0;
        p.shown = Some(Shown {
            key,
            seat: None,
            alpha: None,
            lit: None,
            hovered: None,
        });
        let (frame, bar, name_rh, fill) = (p.frame, p.bar, p.name, p.fill);

        model
            .region_data
            .entry(name_rh)
            .or_default()
            .set_vertex_color(Some(crate::color::Rgba8::WHITE));
        // The name goes through `SetText` with its expand flag: `GetUnitName` takes the one stack
        // argument and leaves the `push 1` ahead of it for the call (`0x7cb706`-`0x7cb718`).
        let text = model.localized_text(name);
        model.region_data.entry(name_rh).or_default().text = Some(text);
        model.touch_measure(name_rh);
        // A new string clears the valid bit and invalidates (`0x771ea4`).
        model.clear_region_valid(name_rh);

        // The bar's bind unwatches its last unit and watches this one (`0x7cc3e2`, `0x7cc416`),
        // then sets its range and its value (`0x7cc42c`, `0x7cc442`), each firing where it moves.
        let p = &mut model.nameplates.plates[i];
        p.bar_unit = key;
        p.bar_watched_at = binds;
        let fires = super::statusbar::set_health(model, bar, unit.max_health, unit.health);
        effects.values(bar, fires);

        // The lit boost is the frame's to set again ([`Self::frame`]).
        if let Some(fill) = fill {
            model.nameplates.lit_fills.remove(&fill);
        }
        Plate::colour(model, i, unit.bar_colour);
        Plate::level(model, i, unit);
        Plate::raid_icon(model, i, unit.raid_icon);
        effects.shown(frame, true);
    }

    /// The frame's own fields of plate `i`, each only when it moved: the seat (`0x608ce0` →
    /// `0x509ec0`), the alpha (`OnUpdate`, `0x7cb9f9`), the lit bar and the hover.
    fn frame(model: &mut Model, i: usize, world: FrameHandle, state: &PlateState) {
        let p = &model.nameplates.plates[i];
        let (frame, glow, name, fill) = (p.frame, p.glow, p.name, p.fill);
        let Some(shown) = p.shown.clone() else {
            return;
        };
        let world_id = model.frame_id(world);

        // Re-seated whenever it moves, as the reference does (`0x509ec0`).
        if shown.seat != Some(state.top_centre) {
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
        if shown.alpha != Some(state.alpha) {
            model.arena.set_alpha(frame, state.alpha);
        }
        // The fill's reaction colour brightens while lit (`LIT_BOOST`); the boost rides the draw
        // ([`lit_fill_boost`]), as it can pass 1, which the packed colour cannot hold.
        if shown.lit != Some(state.lit) {
            if let Some(fill) = fill {
                if state.lit {
                    model.nameplates.lit_fills.insert(fill);
                } else {
                    model.nameplates.lit_fills.remove(&fill);
                }
            }
        }
        if shown.hovered != Some(state.hovered) {
            // The enter and leave notifies (`0x7cb883`, `0x7cb8d9`) set the name's colour only
            // with the timer at 0 or less; a flash outlasts the hover.
            if model.nameplates.plates[i].flash_ms <= 0 {
                let c = if state.hovered {
                    NAME_YELLOW
                } else {
                    crate::color::Rgba8::WHITE
                };
                model
                    .region_data
                    .entry(name)
                    .or_default()
                    .set_vertex_color(Some(c));
            }
            // The glow really shows, as addons read mouseover from it; it only paints nothing.
            model.set_region_hidden(glow, !state.hovered);
        }
        model.nameplates.plates[i].shown = Some(Shown {
            seat: Some(state.top_centre),
            alpha: Some(state.alpha),
            lit: Some(state.lit),
            hovered: Some(state.hovered),
            ..shown
        });
    }

    /// `SetBarColor` (`0x7cbaa0`): the fill's reaction colour, exact, as addons read the unit's
    /// reaction from `GetStatusBarColor`.
    fn colour(model: &mut Model, i: usize, c: [f32; 3]) {
        if let Some(fill) = model.nameplates.plates[i].fill {
            model.region_data.entry(fill).or_default().vertex_color =
                Some(crate::color::Rgba8::from_f32([c[0], c[1], c[2], 1.0]));
        }
    }

    /// `UpdateLevel` (`0x7cbb40`): the level text in its colour, or the skull in its seat.
    fn level(model: &mut Model, i: usize, unit: &PlateUnit) {
        let p = &model.nameplates.plates[i];
        let (level, skull) = (p.level, p.skull);
        match (unit.level, unit.skull) {
            // The skull flag wins the seat, whatever the level.
            (_, true) => {
                model.set_region_hidden(level, true);
                model.set_region_hidden(skull, false);
            }
            (Some(n), false) => {
                let c = unit.level_colour;
                // The level goes through `SetText` with its expand flag (`0x7cbcca`).
                let shown = model.localized_text(n.to_string());
                let data = model.region_data.entry(level).or_default();
                data.text = Some(shown);
                data.set_vertex_color(Some(crate::color::Rgba8::from_f32([c[0], c[1], c[2], 1.0])));
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

    /// `0x7cbe20`: the raid icon's atlas cell, column `idx & 3` and row `idx >> 2`, or hidden.
    fn raid_icon(model: &mut Model, i: usize, icon: Option<u8>) {
        let raid = model.nameplates.plates[i].raid;
        match icon {
            Some(idx) => {
                let (u0, v0) = (f32::from(idx & 3) * 0.25, f32::from(idx >> 2) * 0.25);
                let data = model.region_data.entry(raid).or_default();
                data.tex_coords = Some(TexCoords::Rect([u0, u0 + 0.25, v0, v0 + 0.25]));
                model.set_region_hidden(raid, false);
            }
            None => model.set_region_hidden(raid, true),
        }
    }

    /// Retire a plate: hide it, never destroy it (`0x608a10`), so it keeps its child-list index and
    /// comes back as the same Lua object addons key their per-plate state by. Its guid stays: the
    /// retire never writes `+0x4e8`, and its bar still watches the unit.
    fn retire(model: &mut Model, i: usize, effects: &mut SyncEffects) {
        if model.nameplates.plates[i].shown.is_none() {
            return;
        }
        let frame = model.nameplates.plates[i].frame;
        if !model.nameplates.pending_paint.remove(&frame) {
            model.nameplates.retired_in_batch.insert(frame);
        }
        effects.shown(frame, false);
        model.nameplates.plates[i].shown = None;
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
