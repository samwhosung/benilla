//! **The nameplate widgets** — `CGNamePlateFrame` as the frame-system object it actually is.
//!
//! benilla drew V-plates for a year as quads pushed straight into the UI pass, which made them
//! invisible to Lua: a live run's `WorldFrame:GetChildren()` returned the two named FrameXML
//! frames and nothing else, and every 1.12 nameplate addon — all of which find plates by walking
//! exactly that list — found nothing to work with. The reference does the opposite, and this
//! module is that difference closed.
//!
//! ## What the reference builds, byte for byte
//!
//! The plate is a `CSimpleButton` subclass (ctor `0x7cb250`, `…\Ui\NamePlateFrame.cpp`, vtable
//! `0x81de50`, size `0x518`) whose **base constructor parents it to the `CGWorldFrame` singleton**
//! `ds:0xb4b2bc` (`0x7786a3` → `0x769318` → `SetParent 0x76ab10`, tail-appending at `0x76abdb`) —
//! so it is a real member of `[worldframe+0x300]`, returned by `GetChildren` and counted by
//! `GetNumChildren`, inheriting strata **0 (WORLD)** and taking level **1**.
//!
//! - **Identity:** `GetObjectType()` = `"Button"`, `IsObjectType` accepts Button/Frame/Region, and
//!   `GetName()` is **nil** — `[this+0x98]` is zeroed at `0x76c50b` and only XML `name=` ever
//!   writes it. Anonymity is load-bearing: `_Nameplates` and pfUI both reject a *named*
//!   `WorldFrame` child.
//! - **Six regions, in creation order**: Border `0x7cb2c2`, Glow `0x7cb361`, Name `0x7cb3d7`,
//!   Level `0x7cb479`, Skull `0x7cb512`, RaidIcon `0x7cb5b4`.
//! - **One child**, the health bar (`GetObjectType()` = `"StatusBar"`), whose own single region is
//!   the BarFill texture `GetStatusBarTexture()` returns. **No cast bar exists in 1.12** — a second
//!   child would be blanked by pfUI, which destructures `healthbar, castbar`.
//! - **Real anchors.** The ctor writes them through `0x767c70` — the *same* primitive and the same
//!   `[layoutObj + 4 + 4·point]` storage the Lua `SetPoint` binding ends in — so `GetPoint`,
//!   `GetLeft` and `GetWidth` answer truthfully. Name = `("BOTTOM", plate, "CENTER", 0, 0)`
//!   (`0x7cb456`), Level = `("CENTER", plate, "BOTTOMRIGHT", …)` (`0x7cb7f5`); `_Nameplates`
//!   *validates* both pairs and rejects a plate that fails them.
//! - **Levels, and this is the surprise:** the bar is put **below** the plate, not above it —
//!   `0x7cb32e mov edx,[plate+0xc4]` (=1) · `0x7cb33c dec edx` · `0x7cb33e call 0x76a4f0(bar,0,0)`,
//!   an absolute level that overrides what `SetParent` had just given it. Plate level 1, **bar
//!   level 0**. Since the draw walk's layer loop is *outer* and its frame walk *inner* (`0x765920`:
//!   layer counter `0x76593c`, frame walk `0x7659d0`–`0x7659f8`, `cmp eax,5` at `0x765a63`), the
//!   whole bar frame drains before any plate layer — which is why the border's bevels cap the
//!   fill's ends.
//! - **Layers** (`GetDrawLayer 0x79a6c0`, table `0x811a80`): Border ARTWORK, Glow **HIGHLIGHT**,
//!   Name/Level/Skull OVERLAY (the ctor re-layers Name and Level off ARTWORK at `0x7cb438`/
//!   `0x7cb4ea`), RaidIcon ARTWORK, BarFill ARTWORK on the bar. **No sub-level exists in 5875**;
//!   within a layer it is creation order.
//! - **Scripts:** the engine installs none, and every C++ override tails into the base
//!   unconditionally, so an addon's `SetScript` on OnShow/OnHide/OnUpdate/OnEnter/OnLeave/
//!   OnMouseDown/OnMouseUp/OnClick fires. `RegisterForClicks(0x500)` = Left/Right on mouse-**up**.
//! - **Pooling:** destroy (`0x608a10`) never unparents — it hides and pushes onto a free list, and
//!   reuse re-runs neither the ctor nor `SetParent`. So **the child-list index is stable for the
//!   session**, and a retired plate is still returned *and counted* (the list walk has no
//!   visibility filter). Every corpus addon's `registry[plate]` identity map rests on that.
//!
//! ## The consumer contract this has to keep
//!
//! Four independent 1.12 addons (`_Nameplates`, `CustomNameplates`, pfUI's vanilla branch,
//! ShaguTweaks' vanilla branch — all `## Interface: 11200`) destructure the two tuples
//! *positionally* and identify a plate by `regions[1]:GetTexture()` being exactly
//! `Interface\Tooltips\Nameplate-Border`. The byte-derived order and the corpus's agree element for
//! element — an independent empirical control on a byte derivation. So the composition is not ours
//! to choose; it is an ABI, and the tests transcribe each addon's own walk.
//!
//! **An addon takes the plate over**, which is the half a painter never had to respect: pfUI blanks
//! all six regions and paints its own, sets `SetAlpha(.95)` on non-targets itself, and ShaguTweaks
//! reparents the raid icon. So [`Plate::create`] writes the static properties (paths, fonts,
//! layers) exactly **once**, [`Plate::lay_out`] rewrites the window-derived anchors only when the
//! window moves, and [`Plate::drive`] writes only what actually changed this frame. The one
//! deliberate exception is the plate's own anchor, which the reference itself re-issues every dirty
//! frame from the seat `0x509ec0` (`ClearAllPoints` + `SetPoint`), wiping an addon anchor *on the
//! plate* there too — which is why every corpus addon anchors its overlay **to** the plate and
//! re-anchors the **bar**, never the plate.

use crate::layout::{Anchor, Point};
use crate::order::DrawLayer;
use crate::widget::{FrameHandle, FrameKind, KindState, RegionHandle, RegionKind};

use super::model::Model;
use super::UiScript;
use super::{BlendMode, FontShadow, JustifyV, RegionData, TexCoords};

/// Region 1's art — and the identification test every corpus addon runs, which is why it is a
/// constant here rather than a caller's argument.
pub const BORDER_TEXTURE: &str = "Interface\\Tooltips\\Nameplate-Border";
/// Region 2's. `glow:IsShown()` is how pfUI reads *mouseover* (`nameplates.lua:601`).
pub const GLOW_TEXTURE: &str = "Interface\\Tooltips\\Nameplate-Glow";
/// Region 5's — the boss / out-of-range skull (`levelicon` to pfUI, `Boss` to CustomNameplates).
pub const SKULL_TEXTURE: &str = "Interface\\TargetingFrame\\UI-TargetingFrame-Skull";
/// Region 6's 4-column atlas.
pub const RAID_ICON_TEXTURE: &str = "Interface\\TargetingFrame\\UI-RaidTargetingIcons";
/// The health bar's fill — what `healthbar:GetStatusBarTexture()` hands back.
pub const BAR_FILL_TEXTURE: &str = "Interface\\TargetingFrame\\UI-TargetingFrame-BarFill";
/// `NAMEPLATE_FONT` — Friz Quadrata, the plate's own face.
pub const PLATE_FONT: &str = "Fonts\\FRIZQT__.TTF";

/// The plate's frame level, and the bar's — `plate.level − 1`, absolute and clamped ≥ 0
/// (`0x7cb32e`/`0x7cb33c`/`0x7cb33e` → `0x76a4f0`). The pair is what puts the border over the fill.
const PLATE_LEVEL: u16 = 1;
const BAR_LEVEL: u16 = 0;

/// Everything about a plate's geometry that follows from the *window* rather than from the unit,
/// in FrameXML units (the driver has already divided out the seam scale).
///
/// Applied to every pooled plate at once, and only when it changes — these are exactly the writes
/// that must not happen per frame, because an addon that re-anchors the bar would otherwise have
/// its anchor stamped over at 60 Hz.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlateGeometry {
    /// The plate frame's own size.
    pub width: f32,
    pub height: f32,
    /// The health bar, from the plate's BOTTOMLEFT.
    pub bar_off_x: f32,
    pub bar_off_y: f32,
    pub bar_width: f32,
    pub bar_height: f32,
    /// The level text / skull seat, from the plate's BOTTOMRIGHT (x is subtracted, y added).
    pub level_off_x: f32,
    pub level_off_y: f32,
    /// The skull's square side.
    pub skull_size: f32,
    /// The raid icon's square side; it hangs off the plate's LEFT edge, vertically centred.
    pub raid_size: f32,
    /// Font heights for the name and the level.
    pub name_height: f32,
    pub level_height: f32,
    /// The plate text's drop-shadow step, in the same FrameXML units — **the driver divides the
    /// seam out of it like everything else here**, because the director tuned it as a constant
    /// ONE LOGICAL PIXEL (2026-07-07) and the shared text arm multiplies whatever it finds by the
    /// seam on the way out.
    ///
    /// Left as a bare `1.0` unit by the widget port, it became `round(windowH/768 × uiScale)`
    /// drawn px — 2 px from a 1152-tall window up, 3 px at 2160 — which is exactly the "chunky"
    /// the recorded ±0.001 gx law was tuned away from, and it made the shadow the one part of a
    /// plate that was not uiScale-blind.
    pub shadow_offset: f32,
}

/// One plate's per-frame state, as the app hands it over: everything that can move while a plate is
/// alive. Anything that cannot is written once, at creation.
#[derive(Clone, Debug, PartialEq)]
pub struct PlateState {
    /// The unit this plate belongs to — its GUID, and **the plate's identity**.
    ///
    /// The reference binds a plate to its unit for as long as the plate lives (`[unit+0xe60]`
    /// holds the pointer; the free list hands out a plate at create and takes it back at destroy),
    /// and every corpus addon depends on that: pfUI caches the name, level, colour and its own
    /// overlay ON the frame (`registry[plate]`, `nameplate.original.*`) and only refreshes what
    /// its `OnDataChanged` says moved. Handing widget *i* to whichever unit happens to sort i-th
    /// this frame would make every plate's cached state wrong on the frame two units swap places —
    /// which is why the pool is keyed by this rather than indexed by the driver's sort order.
    pub key: u64,
    /// Where the plate's **TOP-CENTRE** lands, in FrameXML units from the screen's bottom-left —
    /// the reference's own seat (`0x509ec0`: the plate hangs below the point).
    pub top_centre: (f32, f32),
    /// `UNIT_FIELD_HEALTH` and `UNIT_FIELD_MAXHEALTH`, **raw**: `healthbar:GetValue()` is
    /// `[bar+0x320]` (`0x78f5d0`), not a fraction, and `GetMinMaxValues()` answers `(0, max)`.
    /// pfUI and CustomNameplates both divide, so a fraction here would read as 1 hp out of 1.
    pub health: f32,
    pub max_health: f32,
    /// The bar's reaction colour — the four confirmed dwords. Addons reverse-engineer the unit's
    /// faction from `GetStatusBarColor`, so these have to be exact.
    pub bar_colour: [f32; 3],
    /// The unit's name.
    pub name: String,
    /// The level number to show, or `None` when the unit has no `UNIT_FIELD_LEVEL` yet.
    ///
    /// **`None` is not the skull** — that is [`Self::skull`], and conflating them is what made a
    /// unit whose level had not arrived wear a world boss's skull. The old painter kept the whole
    /// level/skull block inside `if let Some(level)`, so no level meant neither.
    pub level: Option<u32>,
    /// Show the SKULL in the level's seat instead of a number — the boss / far-out-of-range mark.
    pub skull: bool,
    /// The level text's con colour.
    pub level_colour: [f32; 3],
    /// The raid-target mark, 0-based (`col = idx & 3`, `row = idx >> 2`), or `None`.
    pub raid_icon: Option<u8>,
    /// The plate's own alpha: **exactly 1.0 for the target**, dimmed otherwise. `GetAlpha() == 1`
    /// is how pfUI and CustomNameplates detect the target plate — a contract, not a look.
    pub alpha: f32,
    /// Mouseover ∪ target: the bar's brighten.
    pub lit: bool,
    /// The plate rect is hovered right now — the name goes yellow (`0x7cb850` OnEnter, name-only)
    /// and the glow region is shown, which is the mouseover signal addons read.
    pub hovered: bool,
}

/// One pooled plate's widgets, in the reference's creation order.
#[derive(Debug)]
struct Plate {
    frame: FrameHandle,
    border: RegionHandle,
    glow: RegionHandle,
    name: RegionHandle,
    level: RegionHandle,
    skull: RegionHandle,
    raid: RegionHandle,
    bar: FrameHandle,
    fill: RegionHandle,
    /// Last frame's state, so [`Plate::drive`] writes only what moved. `None` = retired.
    last: Option<PlateState>,
    /// **The geometry this plate's anchors were written under**, or `None` while it has never been
    /// laid out — the fix for a plate that was born invisible.
    ///
    /// [`Plate::lay_out`] used to be driven by one per-frame `relayout` flag ("did the window
    /// move?"), which is true exactly once per window size. A plate the pool grew on any LATER
    /// frame — a unit walking into range with no free slot — therefore kept `width: 0`, `height: 0`
    /// and no region anchors, resolved to nothing, and drew nothing for the rest of the session.
    /// The number of plates on screen was capped at however many were up on the frame V was first
    /// pressed. Per-plate state instead of a per-frame flag makes the condition unfalsifiable: a
    /// plate whose anchors are not the current geometry's gets them, whatever the reason.
    laid_out: Option<PlateGeometry>,
}

impl Plate {
    /// The unit this plate is currently bound to, or `None` while it sits in the pool.
    fn key(&self) -> Option<u64> {
        self.last.as_ref().map(|s| s.key)
    }
}

/// The plate pool — grown on demand, **never shrunk**, in creation order.
///
/// The reference's pool is unbounded and its free list FIFO; what matters to an addon is only that
/// an index, once handed out, keeps meaning the same Lua object, and that a retired plate stays in
/// the child list (hidden). pfUI and ShaguTweaks scan `initialized + 1 .. GetNumChildren()` and
/// never reset `initialized`, so a client that reordered or compacted this list would make them
/// miss every plate created afterwards — permanently, and silently.
#[derive(Debug, Default)]
pub(crate) struct NamePlates {
    plates: Vec<Plate>,
    /// Which pool slot each live unit holds, by [`PlateState::key`] — the reference's
    /// `[unit+0xe60]`, from this side. A slot leaves this map only when its unit's plate is
    /// retired, and the slot itself never moves.
    assigned: std::collections::HashMap<u64, usize>,
    /// The reverse of the arena's own structure: which slot a frame handle is. Filled at
    /// creation and never removed (plates are never destroyed), so the pointer boundary and the
    /// click funnel can ask "is this frame a plate, and whose?" in one lookup.
    by_frame: std::collections::HashMap<FrameHandle, usize>,
    /// Completed clicks on plates, drained by the app each frame
    /// ([`UiScript::take_nameplate_clicks`]).
    clicks: Vec<NamePlateClick>,
    /// **The plate's own hit-test veto** — `0x7cba30`, the `+0x3c` override, set while a
    /// ground-targeted spell is armed ([`UiScript::set_nameplate_hit_test_veto`]).
    ///
    /// Deliberately NOT the mouse-enabled bit: the reference has two distinct mechanisms here and
    /// only one of them is visible from Lua. `0x60f830` (freelook) toggles input kind 2 on every
    /// plate, so `IsMouseEnabled()` answers `false` — while `0x7cba30` refuses the *hit test*
    /// before testing the rect and never touches the bit, so an addon asking a plate whether it
    /// takes the mouse during ground targeting is told `true`, truthfully. Folding the two into
    /// one flag would have been simpler here and wrong there.
    hit_test_vetoed: bool,
    /// The geometry every live plate is currently laid out under.
    geometry: Option<PlateGeometry>,
}

impl NamePlates {
    /// Does the `0x7cba30` veto refuse `frame` this point? True only for a plate, and only while
    /// the veto stands.
    pub(crate) fn vetoes(&self, frame: FrameHandle) -> bool {
        self.hit_test_vetoed && self.by_frame.contains_key(&frame)
    }
}

/// A completed click on a plate — what the reference's own `CGNamePlateFrame` click slot
/// (`0x7cb910`) turns into a selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamePlateClick {
    /// The unit whose plate was clicked ([`PlateState::key`]).
    pub key: u64,
    /// `"LeftButton"` / `"RightButton"` — the plate registers for both, on mouse-UP only
    /// (`RegisterForClicks(0x500)` at `0x7cb637`).
    pub button: String,
}

/// What a sync produced that has to be fired **outside** the model borrow: the frames whose
/// effective visibility flipped (their `OnShow`/`OnHide`), and the bars whose value moved (their
/// `OnValueChanged`, which pfUI hooks — `nameplates.lua:393` — and reads `this:GetParent()` in).
#[derive(Default)]
struct SyncEffects {
    visibility: Vec<FrameHandle>,
    values: Vec<(FrameHandle, f32)>,
}

impl UiScript {
    /// Drive the plate pool from the app's per-frame verdict: `states[i]` is the i-th **shown**
    /// plate, and every pooled plate past `states.len()` is retired (hidden, never destroyed).
    ///
    /// `geometry` is the window-derived half; passing an unchanged value costs nothing.
    pub fn sync_nameplates(&mut self, geometry: PlateGeometry, states: &[PlateState]) {
        let lua = self.lua();
        let effects = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // No WorldFrame, no plates: a VM with no interface loaded (a bare test app, the addon
            // harness) has nowhere to parent them, and inventing a root would put plates somewhere
            // an addon's walk could never find them anyway.
            let Some(world) = model.arena.lookup("WorldFrame") else {
                return;
            };
            sync(&mut model, world, geometry, states)
        };
        super::event::fire_visibility_changes(lua, effects.visibility);
        for (bar, value) in effects.values {
            super::statusbar::fire_engine_value_changed(lua, bar, value);
        }
    }

    /// **The unit whose plate the pointer is inside**, or `None` — the reference's plate OnEnter
    /// (`0x7cb850`) publishing the mouseover global `[0xb4e2c8]`, read from the app's side instead
    /// of pushed.
    ///
    /// The app needs this because a plate is real UI here: with the plate mouse-enabled, the UI
    /// pointer pass owns the cursor over it and `target::hover`'s world pick correctly stands
    /// down — so the mouseover has to come from the frame that took it. Exactly the hovered frame,
    /// never an ancestor: an addon frame parented to a plate takes the hover in the reference too,
    /// and its OnEnter is the one that fires.
    pub fn hovered_nameplate(&self) -> Option<u64> {
        let lua = self.lua();
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let frame = model.mouseover?;
        let slot = *model.nameplates.by_frame.get(&frame)?;
        model.nameplates.plates.get(slot)?.key()
    }

    /// Drain the completed plate clicks — the app turns each into a selection, which is what the
    /// reference's own click slot does. Both a physical click and Lua's `plate:Click("LeftButton")`
    /// arrive here, because both go through the one click funnel.
    pub fn take_nameplate_clicks(&mut self) -> Vec<NamePlateClick> {
        let lua = self.lua();
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        std::mem::take(&mut model.nameplates.clicks)
    }

    /// **The mouselook toggle** (`0x60f830`): entering camera freelook disables mouse input on
    /// every plate, leaving re-enables it — the reference walks its own intrusive plate list at
    /// `ds:0xc4d92c` doing exactly this, called from `0x483e80` (enter) and `0x483e70` (leave).
    ///
    /// Without it a right-drag that starts over a plate would be a plate click instead of a camera
    /// turn, and the plates would keep taking a pointer that is no longer on screen.
    pub fn set_nameplate_mouse(&mut self, enabled: bool) {
        let lua = self.lua();
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let frames: Vec<FrameHandle> = model.nameplates.plates.iter().map(|p| p.frame).collect();
        for frame in frames {
            model.arena.set_mouse_enabled(frame, enabled);
        }
    }

    /// **The plate hit-test veto** (`0x7cba30`, the plate's `+0x3c` override — the one of the six
    /// that can refuse *before* testing the rect): while a ground-targeted spell is armed the
    /// plates stop taking the mouse and the click falls through to the `WorldFrame`, so the reticle
    /// can be placed through a plate.
    ///
    /// The reference's predicate is `IsTargeting() && TargetingWantsLocation(flag & 0x60) &&
    /// !0x6e6180()`. The caller supplies the first two verbatim — benilla's `TargetingWants::
    /// Location` **is** that `& 0x60` mask. The third is another mask predicate over the same
    /// pending-spell word (`& 0x878e`) whose
    /// meaning is not settled; a word that reaches our targeting cursor with location bits is a
    /// pure ground target (the resolver binds or refuses a unit word before then), so it is left
    /// unmodelled rather than guessed at.
    pub fn set_nameplate_hit_test_veto(&mut self, vetoed: bool) {
        let lua = self.lua();
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.nameplates.hit_test_vetoed = vetoed;
    }

    /// **Retire every live plate** — hide them all and return them to the pool, without touching
    /// the geometry they are laid out under.
    ///
    /// The app's every early return owes this: a painter that stopped drawing left no plates
    /// behind, but widgets stay until something hides them, so a V press that turns plates off, a
    /// frame with no camera, or a world exit would otherwise leave the last frame's plates standing
    /// on screen. It is `0x608a10` applied to the whole active list, which is what the reference
    /// does when the master toggle clears.
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
        super::event::fire_visibility_changes(lua, effects.visibility);
    }

    /// How many plate widgets the pool holds — the plate driver's census, and the tests'.
    pub fn nameplate_pool_len(&self) -> usize {
        let lua = self.lua();
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        model.nameplates.plates.len()
    }
}

/// Record a completed click on `id` if that frame is a live plate — [`super::button::click_button`]
/// calls this for every button click, physical or scripted.
pub(super) fn note_click(lua: &mlua::Lua, id: u32, button: &str) {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let Some(frame) = model.id_to_frame.get(&id).copied() else {
        return;
    };
    let Some(&slot) = model.nameplates.by_frame.get(&frame) else {
        return;
    };
    // A pooled (retired) plate has no unit, and clicking one is not possible anyway — it is hidden.
    let Some(key) = model.nameplates.plates.get(slot).and_then(Plate::key) else {
        return;
    };
    model.nameplates.clicks.push(NamePlateClick {
        key,
        button: button.to_string(),
    });
}

/// The whole per-frame drive: grow, re-lay-out if the window moved, write what changed, retire the
/// tail.
fn sync(
    model: &mut Model,
    world: FrameHandle,
    geometry: PlateGeometry,
    states: &[PlateState],
) -> SyncEffects {
    let mut effects = SyncEffects::default();
    let relayout = model.nameplates.geometry != Some(geometry);
    model.nameplates.geometry = Some(geometry);

    // Retire first, so a unit that left this frame frees its slot for one that arrived — the
    // reference's destroy-then-create through the same free list, in the same tick.
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
                // The lowest free slot, else a new one at the tail. Lowest-free rather than the
                // reference's FIFO free list: both keep every existing index meaning what it did,
                // which is the only property an addon can observe.
                let free = (0..model.nameplates.plates.len())
                    .find(|i| model.nameplates.plates[*i].last.is_none());
                let i = free.unwrap_or_else(|| {
                    let plate = Plate::create(model, world, &mut effects);
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

    // A geometry change reaches the retired plates too: they are still in the child list, an addon
    // can still read them, and they must not come back next frame laid out for the old window.
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
    /// Build one plate: the Button under the WorldFrame, its six regions in the reference's
    /// creation order, and the health-bar child with its fill.
    ///
    /// Everything here is written **once**. An addon that blanks a region (pfUI's `DisableObject`
    /// does `SetTexture("")`) must find it still blank on the next frame.
    fn create(model: &mut Model, world: FrameHandle, effects: &mut SyncEffects) -> Plate {
        // Anonymous, and that is a requirement rather than an omission (module doc).
        let frame = model.arena.create(FrameKind::Button, None, Some(world));
        let strata = model
            .arena
            .frame(world)
            .map(|f| f.strata)
            .unwrap_or_default();
        model.arena.set_frame_strata(frame, strata);
        model.arena.set_frame_level(frame, PLATE_LEVEL, true);
        // The plate takes the mouse, which it is born with: `CSimpleButton`'s ctor writes
        // `[+0xcc] = 0x4` (`0x7786a3`), so the reference's plate needs no `EnableMouse` of its own
        // and neither does ours. Hovering it publishes the mouseover (`0x7cb850` → `0x492890`) and
        // a completed click selects through the button's click slot (`0x7cb910`); both reach the
        // app through [`UiScript::hovered_nameplate`] and [`UiScript::take_nameplate_clicks`].
        // `EnableMouse`/`IsMouseEnabled` are in the corpus's own call set, and pfUI's vanilla
        // click-through block drives them.
        //
        // **And it takes BOTH buttons, on the UP edge**: `RegisterForClicks(0x500)` at `0x7cb637`
        // (`0x779730` → `[this+0x330] = 0x500` = LeftButtonUp | RightButtonUp). The arena's Button
        // default is `{"LeftButtonUp"}` alone, so without this a physical right-click on a plate
        // fired no click at all — [`super::button::wants_click`] refused the release, the funnel
        // never ran, and since 2233 took the camera off a press that lands on a plate there was no
        // other path left: right-clicking a nameplate did nothing. The reference's own slot
        // (`0x7cb910`) takes mask 1 → `0x4925d0` select and mask 4 → `0x492820` select+interact.
        if let Some(KindState::Button(bs)) = model.arena.frame_mut(frame).map(|f| &mut f.kind_state)
        {
            bs.registered_clicks = ["LeftButtonUp", "RightButtonUp"]
                .into_iter()
                .map(str::to_string)
                .collect();
        }

        // The blend modes are the ctor's own, one `0x7703f0` call per texture: everything is
        // BLEND(2) except the **glow, which is ADD(3)** (`push 3` at `0x7cb36a` → `0x7cb374`).
        // Getting that one wrong is not a subtlety — see [`is_unpainted_glow`].
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
        let name = font_string(model, frame, JustifyV::Bottom);
        let level = font_string(model, frame, JustifyV::Middle);
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
        // ↑ THE ORDER. Six regions, and this sequence is the ABI (module doc).

        // The one child, below the plate's own level so the border draws over the fill.
        let bar = model.arena.create(FrameKind::StatusBar, None, Some(frame));
        model.arena.set_frame_strata(bar, strata);
        model.arena.set_frame_level(bar, BAR_LEVEL, true);
        let fill = model
            .arena
            .create_region(bar, RegionKind::Texture, DrawLayer::Artwork, 0)
            .expect("live bar");
        let fill_data = model.region_data.entry(fill).or_default();
        fill_data.texture = Some(BAR_FILL_TEXTURE.to_string());
        // Not overridden by the ctor — the `CSimpleTexture` ctor's own `[+0xd0] = 2` stands
        // (`0x76fc40`). Written explicitly because the default is a FACT here, not an
        // omission.
        fill_data.blend = BlendMode::Blend;
        if let Some(KindState::StatusBar(sb)) =
            model.arena.frame_mut(bar).map(|f| &mut f.kind_state)
        {
            sb.bar = Some(fill);
            sb.min = 0.0;
            sb.max = 1.0;
            sb.value = 0.0;
        }

        // The skull and the raid icon are the two regions that only sometimes show; the glow is
        // shown on hover alone. The rest are always up while the plate is.
        for rh in [glow, skull, raid] {
            model.region_data.entry(rh).or_default().hidden = true;
        }

        // Born retired: a fresh plate belongs to no unit yet, and `IsShown()` is what an addon
        // reads to tell a live plate from a pooled one.
        effects
            .visibility
            .extend(model.arena.set_shown(frame, false));

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

    /// (Re)lay the static anchors — everything that follows from the window's size rather than the
    /// unit. Called on the first drive and on a geometry change, never per frame.
    fn lay_out(model: &mut Model, i: usize, g: PlateGeometry) {
        let p = &model.nameplates.plates[i];
        let (frame, bar) = (p.frame, p.bar);
        let (border, glow, name, level, skull, raid) =
            (p.border, p.glow, p.name, p.level, p.skull, p.raid);
        let plate_id = model.frame_id(frame);

        // The plate's own size; its position is per-frame and lives in `drive`.
        let input = model.layout_inputs.entry(frame).or_default();
        input.width = g.width;
        input.height = g.height;

        // The border fills the plate, and so does the glow — a rim over the whole frame.
        for rh in [border, glow] {
            model.region_data.entry(rh).or_default().anchors = vec![
                Anchor::new(Point::TopLeft, plate_id, Point::TopLeft, 0.0, 0.0),
                Anchor::new(Point::BottomRight, plate_id, Point::BottomRight, 0.0, 0.0),
            ];
        }

        // The health bar: BOTTOMLEFT + (off_x, off_y), explicit size.
        let bar_input = model.layout_inputs.entry(bar).or_default();
        bar_input.anchors = vec![Anchor::new(
            Point::BottomLeft,
            plate_id,
            Point::BottomLeft,
            g.bar_off_x,
            g.bar_off_y,
        )];
        bar_input.width = g.bar_width;
        bar_input.height = g.bar_height;

        // The name — ("BOTTOM", plate, "CENTER", 0, 0), the pair `_Nameplates` validates.
        let name_data = model.region_data.entry(name).or_default();
        name_data.anchors = vec![Anchor::new(
            Point::Bottom,
            plate_id,
            Point::Center,
            0.0,
            0.0,
        )];
        name_data.font_height = Some(g.name_height);

        // The level — ("CENTER", plate, "BOTTOMRIGHT", −x, +y), the other validated pair.
        for rh in [name, level] {
            model.region_data.entry(rh).or_default().font_shadow = Some(FontShadow {
                offset: [g.shadow_offset, -g.shadow_offset],
                color: [0.0, 0.0, 0.0, 1.0],
            });
        }

        let level_data = model.region_data.entry(level).or_default();
        level_data.anchors = vec![Anchor::new(
            Point::Center,
            plate_id,
            Point::BottomRight,
            -g.level_off_x,
            g.level_off_y,
        )];
        level_data.font_height = Some(g.level_height);
        level_data.font_explicit.height = true;
        model.touch_measure(level);

        // The skull replaces the level number, in the level's own seat.
        let skull_data = model.region_data.entry(skull).or_default();
        skull_data.anchors = vec![Anchor::new(
            Point::Center,
            plate_id,
            Point::BottomRight,
            -g.level_off_x,
            g.level_off_y,
        )];
        skull_data.size = Some((g.skull_size, g.skull_size));

        // The raid icon hangs off the LEFT edge, vertically centred.
        let raid_data = model.region_data.entry(raid).or_default();
        raid_data.anchors = vec![Anchor::new(Point::Right, plate_id, Point::Left, 0.0, 0.0)];
        raid_data.size = Some((g.raid_size, g.raid_size));

        // Anchor TARGETS moved (from nothing to the plate) on the first lay-out, so this is the
        // conservative touch — the only one in this module, and it runs once per geometry change.
        model.touch_layout();
        model.nameplates.plates[i].laid_out = Some(g);
    }

    /// The per-frame write: position, health, texts, colours, shown flags, alpha — **and nothing
    /// else**, each only when it moved.
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

        // The seat, re-issued whenever it moves, exactly like the reference's own (`0x509ec0`:
        // ClearAllPoints then SetPoint) — which is why an addon anchor on the plate does not
        // survive there either. A moved OFFSET keeps the graph's edges, so the precise touch
        // applies; the first seat adds an edge and takes the retarget.
        if last
            .as_ref()
            .is_none_or(|l| l.top_centre != state.top_centre)
        {
            let fresh = model
                .layout_inputs
                .get(&frame)
                .is_none_or(|i| i.anchors.is_empty());
            let input = model.layout_inputs.entry(frame).or_default();
            input.anchors = vec![Anchor::new(
                Point::Top,
                world_id,
                Point::BottomLeft,
                state.top_centre.0,
                state.top_centre.1,
            )];
            if fresh {
                model.touch_layout_retarget_frame(frame, &[], &[world_id]);
            } else {
                model.touch_layout_frame(frame);
            }
        }

        if last.is_none() {
            effects
                .visibility
                .extend(model.arena.set_shown(frame, true));
        }
        if last.as_ref().is_none_or(|l| l.alpha != state.alpha) {
            model.arena.set_alpha(frame, state.alpha);
        }

        // The bar reports RAW health (module doc). Its fill carries the reaction colour, lifted by
        // a uniform brighten while the unit is the mouseover or the target — 0184's form of the
        // highlight, in place of the additive `Nameplate-Glow` rim.
        if last
            .as_ref()
            .is_none_or(|l| l.health != state.health || l.max_health != state.max_health)
        {
            if let Some(KindState::StatusBar(sb)) =
                model.arena.frame_mut(bar).map(|f| &mut f.kind_state)
            {
                sb.min = 0.0;
                sb.max = state.max_health;
                sb.value = state.health.clamp(0.0, state.max_health);
            }
            effects.values.push((bar, state.health));
        }
        if last
            .as_ref()
            .is_none_or(|l| l.bar_colour != state.bar_colour || l.lit != state.lit)
        {
            let boost = if state.lit { LIT_BOOST } else { 1.0 };
            let c = state.bar_colour;
            model.region_data.entry(fill).or_default().vertex_color =
                Some([c[0] * boost, c[1] * boost, c[2] * boost, 1.0]);
        }

        // The name: white, yellow while the plate itself is hovered.
        if last.as_ref().is_none_or(|l| l.name != state.name) {
            model.region_data.entry(name).or_default().text = Some(state.name.clone());
            model.touch_measure(name);
            model.touch_layout_region(name);
        }
        if last.as_ref().is_none_or(|l| l.hovered != state.hovered) {
            let c = if state.hovered {
                [1.0, 1.0, 0.0, 1.0]
            } else {
                [1.0, 1.0, 1.0, 1.0]
            };
            model.region_data.entry(name).or_default().vertex_color = Some(c);
            // `glow:IsShown()` IS the mouseover signal (pfUI `nameplates.lua:601`). The region is
            // shown for real; that it paints nothing is 0184's deviation, not a missing region.
            model.region_data.entry(glow).or_default().hidden = !state.hovered;
        }

        // Level text and skull are mutually exclusive, in one seat.
        if last.as_ref().is_none_or(|l| {
            l.level != state.level || l.skull != state.skull || l.level_colour != state.level_colour
        }) {
            match (state.level, state.skull) {
                // The skull wins the seat, and it is the SKULL FLAG that puts it there.
                (_, true) => {
                    model.region_data.entry(level).or_default().hidden = true;
                    model.region_data.entry(skull).or_default().hidden = false;
                }
                (Some(n), false) => {
                    let c = state.level_colour;
                    let data = model.region_data.entry(level).or_default();
                    data.text = Some(n.to_string());
                    data.vertex_color = Some([c[0], c[1], c[2], 1.0]);
                    data.hidden = false;
                    model.region_data.entry(skull).or_default().hidden = true;
                    model.touch_measure(level);
                    model.touch_layout_region(level);
                }
                // No level yet and no skull: the seat stays EMPTY, the old painter's behaviour.
                (None, false) => {
                    model.region_data.entry(level).or_default().hidden = true;
                    model.region_data.entry(skull).or_default().hidden = true;
                }
            }
        }

        // The raid mark: the 4-column atlas cell, shown only while the unit carries one.
        if last.as_ref().is_none_or(|l| l.raid_icon != state.raid_icon) {
            match state.raid_icon {
                Some(idx) => {
                    let (u0, v0) = (f32::from(idx & 3) * 0.25, f32::from(idx >> 2) * 0.25);
                    let data = model.region_data.entry(raid).or_default();
                    data.tex_coords = Some(TexCoords::Rect([u0, u0 + 0.25, v0, v0 + 0.25]));
                    data.hidden = false;
                }
                None => model.region_data.entry(raid).or_default().hidden = true,
            }
        }

        model.nameplates.plates[i].last = Some(state.clone());
    }

    /// Retire a plate: **hide it, never destroy it** (`0x608a10` — unlink from the active list,
    /// push onto the free list, `Hide`). It keeps its slot in `WorldFrame`'s child list, keeps its
    /// index, and comes back as the same Lua object, which is the contract every addon's
    /// `registry[plate]` identity map rests on.
    fn retire(model: &mut Model, i: usize, effects: &mut SyncEffects) {
        if model.nameplates.plates[i].last.is_none() {
            return;
        }
        let frame = model.nameplates.plates[i].frame;
        effects
            .visibility
            .extend(model.arena.set_shown(frame, false));
        model.nameplates.plates[i].last = None;
    }
}

/// One of the plate's texture regions, with its path and its blend mode — created in the ABI's
/// order by the caller. The mode is not decoration: the ctor sets one on every texture it makes
/// (`0x7703f0`, storing `[texture+0xd0]`), and the glow's is the one that differs — see
/// [`is_unpainted_glow`].
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
    let data = model.region_data.entry(rh).or_default();
    data.texture = Some(path.to_string());
    data.blend = blend;
    rh
}

/// One of the plate's two FontStrings. Both are OVERLAY — the ctor re-layers them off ARTWORK
/// (`0x7cb438`, `0x7cb4ea`) — and both take `NAMEPLATE_FONT` with the plate's black 1 px drop
/// shadow; the heights arrive with the geometry.
///
/// The black drop shadow is written in [`Plate::lay_out`] instead of here: its step is a
/// WINDOW-derived number ([`PlateGeometry::shadow_offset`]), not a create-time constant.
///
/// `justify_v` is the one benilla-side compensation here, and it is the old painter's, kept: the
/// plate's name is seated by its **ink**, not by the line box. Our shaper's box is ascent-heavy
/// for Friz, so a box-seated `BOTTOM` anchor drops the name ~8 px — half of it over the plate —
/// which is the director's old "the level is not aligned" in its other half. `Bottom` puts the
/// text where the reference's own metrics put it.
fn font_string(model: &mut Model, frame: FrameHandle, justify_v: JustifyV) -> RegionHandle {
    let rh = model
        .arena
        .create_region(frame, RegionKind::FontString, DrawLayer::Overlay, 0)
        .expect("live plate");
    let data = model.region_data.entry(rh).or_default();
    data.font_path = Some(PLATE_FONT.to_string());
    data.font_explicit.face = true;
    data.justify.set_v(justify_v);
    rh
}

/// **Is this frame a V-plate?** — the seat question, asked of the OWNER.
///
/// A plate is a WorldFrame overlay: the driver snaps its rect to the DEVICE pixel grid
/// (`vplates::device_snap`, 0188/1398) because it slides continuously over the world, and
/// everything drawn inside it has to be rigid to that rect. The renderer's UI seat snap quantizes
/// a text block's top on the coarser LOGICAL grid, so with both laws in force a plate's name and
/// level pop a whole logical pixel every second step the border takes — two quantizers on one
/// sliding object, and the "janky text walking up to a mob" the director reported the day after
/// decision 2148 moved these strings out of a painter that never snapped them (its host-measure
/// rects were degenerate, and a degenerate rect skips the snap — the carve-out the port silently
/// stopped selecting).
///
/// **Asked of the frame rather than written on our six regions**, because it is the frame that
/// slides: pfUI and ShaguTweaks blank the stock regions and hang their *own* FontStrings on the
/// plate, and those are drawn inside the same rect and want the same answer. (A string an addon
/// puts on the plate's health-BAR child is one level further down and still takes the UI grid —
/// the named residual, and the shape of the fix if it ever bites is an ancestor walk.)
pub(super) fn is_world_seated(model: &Model, frame: FrameHandle) -> bool {
    model.nameplates.by_frame.contains_key(&frame)
}

/// **The plate's glow region paints nothing here** — the region is real, shown, textured and ADD,
/// and the quad walk skips it ([`super::extract`]). Decision 0184, and it is the director's call
/// rather than a gap we could close by trying harder.
///
/// The reference draws this region `(SRC_ALPHA, ONE)` over the health bar: an additive
/// reaction-tinted lift of the bar's cavity. 0183 built exactly that and the director rejected it
/// on sight — *"you should not change the gradient or anything — just make the color of the yellow
/// bar brighter"* — so 0184 replaced the rim with [`LIT_BOOST`], and the paint has been the bar's
/// own brighten ever since. That is what this predicate keeps true now that the plate is a widget
/// and its regions go through the shared frame→quad path instead of a painter that simply never
/// emitted a glow quad.
///
/// **Why the mode matters even though nothing is drawn.** `Nameplate-Glow.blp` is DXT1 with **no
/// alpha channel** — every one of its 4096 texels is alpha 255 — and 80.5% of it is exactly
/// `rgb(0,0,0)`, the surround; the signal is a grey rounded bar (rim 140, interior ~46) in rows
/// 20-26 that only reads as a glow when it is *added*. Blitted with straight `BLEND` it is
/// `dst·(1−1) + black·1` — **an opaque black rectangle over the whole plate**, which is precisely
/// what shipped when the widget port left the mode at the ctor default (director report,
/// 2026-09-10). So `GetBlendMode()` answers `"ADD"` truthfully, an addon that re-textures the
/// region gets its own art painted normally, and only the reference's own glow art on an ADD
/// region is the one thing this engine declines to draw.
pub(super) fn is_unpainted_glow(data: &RegionData) -> bool {
    data.blend == BlendMode::Add && data.texture.as_deref() == Some(GLOW_TEXTURE)
}

/// The LIT (mouseover ∪ target) bar brighten — a uniform multiplicative lift of the fill tint,
/// gradient untouched (the recorded additive `Nameplate-Glow` rim read as hard edge lines on
/// our linear-blending pipeline, and the director pinned the brighten instead).
///
/// Quad colours are client-space sRGB (`srgb_quad_color` linearizes them), so this multiplies
/// ENCODED texels ~1:1 — the gamma-space modulate the client's FFP would do. **255/215 is the
/// largest clean value**: the fill texture's encoded peak (215) lands exactly on white, no row
/// clips, and every row scales by the same visible factor. (A first cut of 1.45 assumed a
/// linear-space multiply and flattened six rows against white — the very gradient change the
/// director banned.)
const LIT_BOOST: f32 = 255.0 / 215.0;
