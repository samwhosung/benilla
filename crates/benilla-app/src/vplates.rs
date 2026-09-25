//! **V-key nameplates** — `CGNamePlateFrame` (ctor `0x7cb250`): the toggled health-bar plates
//! over units' heads, a **2-D overlay** distinct from the world-pass overhead *names*
//! (`crate::nameplates`) they replace.
//!
//! The pinned law, transcribed:
//! - **Master toggles** (`[0xc4da34]` bit 0 enemy / bit 3 friendly; real-client boot default
//!   **OFF for both**, and since 1804 ours too — [`VPlateMode::default`]): the
//!   `ShowNameplates`/`ShowFriendNameplates` script pairs —
//!   bound to **V / Shift-V** by FrameXML `Bindings.xml` (asset-sourced default, like TAB).
//! - **Gate**: never the own unit; never `NOT_SELECTABLE` (UNIT_FIELD_FLAGS bit 25); the
//!   enemy/friendly split is **`CanAttack(localPlayer → unit)`**, NOT a reaction threshold
//!   ([`crate::target::ring::plate_is_friendly`], `0x606980` — this file shipped `rank >= 4` for
//!   a year and 1530 corrects it) —
//!   and a **player** subject must additionally pass `CanCooperate`, which is nothing but the two
//!   `FactionTemplate` faction-group masks being equal, so a player whose row carries neither
//!   side's mask (a vmangos `.gm on` character is template 35, mask 0) is enemy-category no matter
//!   how friendly they read; **max 20 yd**, hardcoded; **no
//!   occlusion** (a 2-D overlay — plates draw through walls); snap, no smoothing. Anti-overlap
//!   there *is*, since 0367 found the shared solver ([`crate::smart_rect`]) — this file's older
//!   "no anti-overlap" missed it. And the sphere is bounded by the frustum:
//!   the unit must be **in view** ([`crate::ui_pass::project_overlay`]) or the reference
//!   **destroys** the plate outright (`0x60f600`, before the 20-yard cull); it is never clamped
//!   in from off-screen, which is what ours did (1341/1344).
//!   (The friendly-totem exclusion waits on totems existing.)
//! - **Anchor**: the same overhead head point (`0x608640`, [`overhead_anchor`]) **+ 2/3 yd**
//!   (`[0x80abfc]`), projected per frame (`0x483ee0`); the plate's **TOP-CENTER** lands on the
//!   point (it hangs below — `0x509ec0`) and is edge-clamped half a plate inside the screen;
//!   **constant screen size**: the geometry globals `[0x87d9cc]=0.1` × `[0x87d9d0]=0.025` in gx
//!   screencoord units, where one unit = the screen **diagonal** `√(W²+H²)` (`0x41ad10` — and never
//!   uiScale: plates live outside the UIParent cascade). **Our basis is growth-damped** past
//!   [`PLATE_DIAG_KNEE`] — a director-pinned deviation: the real client grows
//!   plates diagonal-linear without limit (`0x7705b0`) and the director rejected that
//!   look; past 1024×768 the plate grows at half the real rate (midway between faithful and
//!   the native size).
//! - **Anatomy — not here any more.** The plate's six regions, its health-bar child, their
//!   layers and the anchors between them live with the widgets that carry them
//!   ([`benilla_ui::script::nameplate`]): the plate is a real `Button` under the
//!   `WorldFrame` and the shared frame→quad path draws it. What stays this file's is everything
//!   about the WORLD — the gate above, the anchor, the projection, the seat, and the damped
//!   basis the geometry is computed in ([`plate_basis`], [`gx_px`], [`text_px`]).
//! - **Highlight = the mouseover unit ∪ the target** (the watcher `0x606f20 → 0x607080` reads
//!   both globals): the bar's own colour brightens — a uniform [`LIT_BOOST`] lift of the fill
//!   tint, gradient untouched, nothing else changes (director-pinned form, 0184; the recorded
//!   ADD `Nameplate-Glow` rim read as hard edge lines on our linear-blending pipeline and is
//!   not drawn). The plate is itself mouse-enabled UI: hovering it makes its unit the mouseover
//!   (OnEnter `0x7cb850` → `[0xb4e2c8]` — [`PlateHover`], read straight off the widget the
//!   pointer landed on since 2159), which also lifts the model emissive, and a completed click
//!   selects through the plate ([`PlateClicks`]); the hover additionally turns the name yellow
//!   `0xFFFFFF00` (OnEnter, name-only).
//!   All decoupled from the target dim below.
//! - **Target highlight**: relative alpha — with a target, the target's plate is opaque and
//!   every other plate drops to `0x7F`; with no target all are opaque.
//! - **Mutually exclusive with the overhead name** (ShouldShowName): a unit with a live plate
//!   never also draws its floating name — [`VPlates`] is that verdict, read by the name driver.
//!   (The questgiver marker raises for a live plate too — a director-pinned DEVIATION: the
//!   reference really does sit low under a plate (`0x6076c0` arms anim 0) and the director
//!   rejected that overlap; rationale on `quest_markers::pose_markers`, 2274/2275.)
//!
//! (The skull's trivial-gray leg is the shared grey check, [`benilla_ui::script::unit_is_grey`]
//! — `0x5f0700`, the same one the tooltip/quest-range APIs read; it is vacuous on a ≥ +10
//! hostile and kept for transcription fidelity.)

use bevy::ecs::entity::EntityHashSet;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_ui::script::{unit_is_grey, PlateGeometry, PlateState};

use crate::entities::{overhead_anchor, BoneAttach, OverheadFallback};
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, NetEntity, ObjectStore, Reputations, SelfPlayer};
use crate::target::{ring_reaction, Factions, Hovered, Selection, TargetUpdate};
use benilla_world::view::WorldCamera;

/// Sharp-resampling the frame border ([`border::resample_sharp`]) so the 128×32 art reads crisp at
/// the plate's larger size instead of bilinear-magnified soft.
pub(crate) mod border;

/// The master bitmask (`[0xc4da34]`): bit 0 enemy plates, bit 3 friendly. **Both boot OFF, the
/// real client's own state** — see the derivation below. Enemy plates booted ON here from the 0167
/// director call (combat plates from the first frame) until 1804; friendly has been OFF since the
/// 0599 call, which also gives friendly speech bubbles room, because the faithful
/// plate-blocks-bubble gate would otherwise suppress them. Both stay V / Shift-V togglable.
/// `pub(crate)` so the capture harness can force plates on for the `vplates` scenario.
///
/// **This resource is the truth; the CVars are its persistence** (the [`CVAR_ENEMIES`] /
/// [`CVAR_FRIENDS`] pair, [`crate::cvars`]). That mirrors the reference's own two-store shape: the
/// engine bitmask is what the gate reads, and FrameXML keeps `NAMEPLATES_ON`/`FRIENDNAMEPLATES_ON`
/// beside it for saving (`RegisterForSave`, UIOptionsFrame.lua) — pushing changes back through
/// `ShowNameplates()`/`HideNameplates()`. 1.12 registers **no** nameplate CVar (no such string
/// exists in the binary), so the names are the LATER-era engine's, the same posture
/// as `autoLootDefault` — benilla's persistence lives in the CVar store, and a setting with
/// no 1.12 CVar takes the era name rather than inventing one.
///
/// **Both boot OFF — the reference's own state**, and verified on both of its halves, because
/// either alone would be half an answer: the engine bitmask `[0xc4da34]` starts clear, and
/// FrameXML's `UIOptionsFrame_Init` sets `NAMEPLATES_ON = nil` / `FRIENDNAMEPLATES_ON = nil` and
/// only calls `ShowNameplates()` when the SAVED value is truthy (the install's own
/// `UIOptionsFrame.lua` l.180-183, l.769-775). So a fresh 1.12 client draws no plates until you
/// press V. Enemy plates shipped ON here from 0167 until 1804 — the same class of call as the
/// overhead-name pair (`nameplates::NameConfig`): useful while the feature was being built, never
/// re-weighed as a default. The `Default` is derived now that the claim is "both false", and it is
/// still a *claim*: `cvars::tests` welds it to the registered `nameplateShowEnemies`/`Friendly`
/// defaults and asserts the pair is off in as many words, so the derive cannot drift quietly.
#[derive(Resource, Default, Clone, Copy)]
pub(crate) struct VPlateMode {
    pub(crate) enemies: bool,
    pub(crate) friends: bool,
}

/// The persisted names of the two toggles — see [`VPlateMode`]. Welded to [`crate::cvars`]'s
/// registered table by its own test, so the spelling here and the row on the Nameplates page can
/// never drift apart.
pub(crate) const CVAR_ENEMIES: &str = benilla_ui::script::CVAR_NAMEPLATE_ENEMIES;
pub(crate) const CVAR_FRIENDS: &str = benilla_ui::script::CVAR_NAMEPLATE_FRIENDS;

/// The units carrying a live plate this frame — the ShouldShowName exclusivity verdict the
/// overhead-name driver reads (a plated unit never also draws its floating name).
#[derive(Resource, Default)]
pub(crate) struct VPlates(pub(crate) EntityHashSet);

/// **The unit whose plate the pointer is inside** — the plate's own OnEnter publishing the
/// mouseover, from this side (`0x7cb850` → `[0xb4e2c8]`).
///
/// The plate is real mouse-enabled UI now (2148), so the UI pointer pass owns the cursor over it
/// and `target::hover`'s world pick correctly stands down; this is how the unit still reaches
/// [`crate::target::Hovered`]. Written by the plate driver from
/// [`benilla_ui::script::UiScript::hovered_nameplate`] — last frame's layout, which is the
/// reference's own input-vs-layout latency.
#[derive(Resource, Default)]
pub(crate) struct PlateHover(pub(crate) Option<Entity>);

/// Completed clicks on plates, waiting for the targeting chain — the reference's plate click slot
/// (`0x7cb910`), which ends in the same `SetSelection` a click on the body does.
///
/// Both a physical click and an addon's `plate:Click("LeftButton")` land here: the engine records
/// them at the one click funnel both go through.
#[derive(Resource, Default)]
pub(crate) struct PlateClicks {
    pub(crate) left: Vec<Entity>,
    pub(crate) right: Vec<Entity>,
}

/// The plate frame, gx screen-height units (`[0x87d9cc]`/`[0x87d9d0]`): 0.1 × 0.025. The border
/// SetAllPoints-fills it; everything else anchors inside it (offsets: `0x7cb250`, `0x7cb6d0`).
const PLATE_W: f32 = 0.1;
const PLATE_H: f32 = 0.025;
/// The health bar: BOTTOMLEFT ← plate BOTTOMLEFT + (0.0031, 0.003125), sized 0.0804 × 0.007025.
const BAR_OFF_X: f32 = 0.0031;
const BAR_OFF_Y: f32 = 0.003125;
const BAR_W: f32 = 0.0804;
const BAR_H: f32 = 0.007025;
/// Name text height 0.01, its BOTTOM at the plate CENTER; level height 0.0086, its CENTER at
/// plate BOTTOMRIGHT + (−0.0092, +0.0071); both carry the FontString's black drop shadow at
/// ±0.001. The skull (0.01 × 0.01) overlays the level's anchor when it replaces the number.
///
/// `LEVEL_H` is a **director-pinned deviation** (2026-07-07) from the byte 0.009: one em smaller
/// across the window range (em 11 vs 12 at the reference, −1 up through 1440p). TEXT ems compose
/// through the same damped gx basis as the frame geometry ([`text_px`] — the client-side chain:
/// live re-raster on resize (`0x5c2b50`), no fixed UI space, no staleness).
const NAME_H: f32 = 0.01;
const LEVEL_H: f32 = 0.0086;
const LEVEL_OFF_X: f32 = 0.0092;
const LEVEL_OFF_Y: f32 = 0.0071;
const SKULL_SIZE: f32 = 0.01;
/// The raid-target icon (`0x7cb250`): 0.02 × 0.02, RIGHT ← border.LEFT (0,0) — it hangs
/// off the plate's left edge, vertically centered; the 4-column atlas cell is 0.25.
const RAID_ICON_SIZE: f32 = 0.02;
/// The plate's world lift above the overhead anchor: 2/3 yd (`[0x80abfc]`).
const PLATE_LIFT: f32 = 2.0 / 3.0;
/// Max plate distance, hardcoded (no cvar): 20 yd.
const MAX_DIST_SQ: f32 = 20.0 * 20.0;
/// The non-target relative alpha while something is targeted — a **director-pinned deviation**
/// (2026-07-07) from the faithful `0x7F` (0.5), which faded the other plates too hard; raised so
/// non-target plates stay readable. Tunable: 0.5 = the byte law, 1.0 = no dim.
const DIM_ALPHA: f32 = 178.0 / 255.0;

/// The bar-fill palette — the byte-confirmed dwords (`0xcf60d0/e8/c8/dc`): pure
/// red/blue/yellow/green (NOT the ring's pale player-blue). Client-space sRGB, the [`UiQuads`]
/// convention.
const PLATE_HOSTILE: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const PLATE_NEUTRAL: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
const PLATE_FRIENDLY: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const PLATE_PLAYER: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// The plate palette selector — `0x7cbaa0`'s exact test order: hostile
/// (reaction ≤ 1) red, else a player pure blue, else friendly (≥ 4) green, else — reaction 2–3,
/// unfriendly AND neutral — yellow. (Rank 2 is yellow, not red: an earlier transcription here
/// painted it hostile.) The bar fill (and its lit brighten) tints through this.
fn plate_tint(rank: u8, is_player: bool) -> [f32; 4] {
    if rank <= 1 {
        PLATE_HOSTILE
    } else if is_player {
        PLATE_PLAYER
    } else if rank >= 4 {
        PLATE_FRIENDLY
    } else {
        PLATE_NEUTRAL
    }
}

/// The level con palette — the client's EXACT dwords (`0x7cbd50`; the softened WoW colors, not the
/// FrameXML QuestDifficultyColor approximations we shipped first): red `0xFFFF1919`, orange
/// `0xFFFF7F3F`, yellow `0xFFFFFF00`, green `0xFF3FB23F`, gray `0xFF7F7F7F`.
const CON_RED: [f32; 4] = [1.0, 25.0 / 255.0, 25.0 / 255.0, 1.0];
const CON_ORANGE: [f32; 4] = [1.0, 127.0 / 255.0, 63.0 / 255.0, 1.0];
const CON_YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
const CON_GREEN: [f32; 4] = [63.0 / 255.0, 178.0 / 255.0, 63.0 / 255.0, 1.0];
const CON_GRAY: [f32; 4] = [127.0 / 255.0, 127.0 / 255.0, 127.0 / 255.0, 1.0];

/// The client's own con-color law (`0x7cbd50` — thresholds AND the grayband table
/// `[0x81dda8]`, indexed `playerLevel/5`; it agrees with vmangos' formula at every probed level
/// but the table is the client's own): `diff = unit − player`; ≥ +5 red, +3/+4 orange,
/// −2..+2 yellow, lower → green while within `grayband` levels below, gray past it. The
/// grayband/grey test lives in ONE home ([`benilla_ui::script::unit_is_grey`], the byte-identical
/// `0x80ae98`/`0x81dda8` twins) shared with the engine tooltip's "??"/CIVILIAN gates and the
/// `GetQuestGreenRange` binding — so the plate, the tooltip, and the target frame can never
/// disagree on where grey starts.
fn con_color(pl_level: u32, unit_level: u32) -> [f32; 4] {
    let diff = i64::from(unit_level) - i64::from(pl_level);
    if diff >= 5 {
        CON_RED
    } else if diff >= 3 {
        CON_ORANGE
    } else if diff >= -2 {
        CON_YELLOW
    } else if !unit_is_grey(pl_level, unit_level) {
        CON_GREEN
    } else {
        CON_GRAY
    }
}

/// Snap a logical-pixel coordinate onto the **device** pixel grid — the plate's texel alignment
/// (a benilla divergence: the reference draws at fractional device coords).
///
/// The plate rect is snapped so the sharp-resampled border art blits 1:1 and reads crisp instead
/// of bilinear-smeared. Alignment is a property of the **framebuffer**, though, and the
/// quad lane is *logical* px ([`crate::ui_pass`]: 1 world unit = 1 logical px) — so the plain
/// `round()` this shipped with quantized the plate to `scale_factor` PHYSICAL pixels: two pixels
/// of stepping on the 2× display we develop and play on, against a world that slides continuously
/// underneath. At a fractional scale (1.25/1.5, the Windows norm) it did not land on a texel
/// boundary at all — the crispness it was bought for was an accident of an integer scale factor.
///
/// Snapping on the device grid keeps the 1:1 blit, is correct at any scale factor, and halves the
/// stepping at 2×: measured over a Goldshire walk at 1440×810 (the `vpl` trace), the extra
/// displacement the snap adds to a plate's glide fell from a median 0.33 / max 0.99 logical px per
/// frame to 0.21 / 0.49. What is left is the ±½ device pixel every crisp UI pays.
///
/// **Shared with the chat bubble** ([`crate::chat_bubble`]), like [`plate_basis`]/[`gx_px`]: it is
/// the same outside-UIParent overlay family sliding over the same world, so it is the same snap
/// law. The bubble shipped on a plain logical `round()` — this function's own bug, at the one site
/// that never got the fix — which is 1398's finding.
pub(crate) fn device_snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

/// The knee of the plate's gx basis — **a director-pinned DEVIATION from the byte law**.
/// The real client's plates grow diagonal-linear without limit (`0x7705b0`:
/// ~294 px plate + em-29 name at 2560×1440, bilinear-softened) — the director
/// rejected that look as too big/thick/soft, and a hard cap at native as too small.
/// 1280 is the diagonal where the 0.1 × 0.025 frame is EXACTLY the border art's native
/// 128 × 32 px; past it the basis grows at [`PLATE_GROWTH_DAMP`] of the real rate — at every
/// window the plate lands midway between the faithful size and the native pin.
const PLATE_DIAG_KNEE: f32 = 1280.0;
/// The growth rate past the knee: ½ = the midpoint between the byte law and the 0185 native
/// pin (the director's "between what it was and what it is now"). 0 restores the 0185 pin,
/// 1 the faithful law.
const PLATE_GROWTH_DAMP: f32 = 0.5;

/// One gx screencoord unit = the screen **DIAGONAL** `√(W²+H²)` (`0x41ad10` — the device space
/// spans `[0,G44]×[0,G48]`, the live aspect basis; uiScale never enters: plates anchor
/// WorldFrame-side, outside the UIParent cascade) — growth damped past [`PLATE_DIAG_KNEE`] (the
/// director's pin).
/// Shared with the chat bubble ([`crate::chat_bubble`]) — the sibling outside-UIParent overlay,
/// same basis law so bubble text and plate text hold the same em at every window.
pub(crate) fn plate_basis(viewport: Vec2) -> f32 {
    let d = viewport.x.hypot(viewport.y);
    if d <= PLATE_DIAG_KNEE {
        d
    } else {
        PLATE_DIAG_KNEE + (d - PLATE_DIAG_KNEE) * PLATE_GROWTH_DAMP
    }
}

/// gx screencoord units → pixels over the plate basis. Frame geometry only — text takes
/// [`text_px`]. Shared with the chat bubble, like [`plate_basis`].
pub(crate) fn gx_px(v: f32, basis: f32) -> f32 {
    (v * basis).round()
}

/// Plate FontString height → the FreeType em: `min(32, round(h · basis))`. The real client's
/// law (the FontObject bridge `0x44d040` divides the gx height by G48 before `GxuFontCreate`,
/// the raster `0x5ca030` multiplies by the viewport height — netting `h·√(W²+H²)`, re-rasterized
/// live on resize, 32-capped at the atlas cell); our `basis` is that diagonal under the director's
/// [`PLATE_DIAG_KNEE`] damping. Reference-pinned at its 1152×648 window: name em 13, level 12.
/// Shared with the chat bubble (`NAMEPLATE_FONT` at the same 0.01 gx), like [`plate_basis`].
pub(crate) fn text_px(h: f32, basis: f32) -> f32 {
    (h * basis).round().min(32.0)
}

/// **The FrameXML mirror of the two toggles** — `NAMEPLATES_ON` and `FRIENDNAMEPLATES_ON`,
/// pushed into whichever VM is live.
///
/// The reference keeps the plate state in **two** levels: the engine bitmask `[0xc4da34]`, which
/// is volatile and cleared on every `EnterWorld`, and those two FrameXML globals, which are the
/// saved store. `UpdateNameplates` (`UIOptionsFrame.lua` l.768) replays the store into the
/// bitmask, from `UIParent_OnEvent`'s VARIABLES_LOADED and PLAYER_ENTERING_WORLD arms
/// (`UIParent.lua` l.234, l.367).
///
/// **benilla has only one level.** [`VPlateMode`] is the state and the [`CVAR_ENEMIES`] /
/// [`CVAR_FRIENDS`] pair is its persistence, and nothing clears it at a world entry — so on this
/// engine the replay has nothing to restore and everything to break. With the globals left nil
/// (nothing here ever wrote them) `UpdateNameplates` took its else branch and called
/// `HideNameplates()`, which writes the CVar, which IS the store: the player's setting was erased
/// at every world entry and `config.toml` lost the line as "at default".
///
/// So the globals are kept TRUE, which makes the stock replay a value-preserving no-op — and is
/// what a third-party addon reading `NAMEPLATES_ON` is owed anyway, which was 2115's whole
/// argument for loading the stock window in the first place.
pub(crate) fn push_plate_globals(script: &benilla_ui::script::UiScript, mode: VPlateMode) {
    // The reference's own truthiness for these two: the number `1`, or nil. Never `0` — a Lua
    // `0` is truthy, so `NAMEPLATES_ON = 0` would read as ON in `UpdateNameplates`'s `if`.
    let on = |b: bool| b.then_some(1i64);
    let g = script.lua().globals();
    if let Err(e) = g
        .set("NAMEPLATES_ON", on(mode.enemies))
        .and_then(|()| g.set("FRIENDNAMEPLATES_ON", on(mode.friends)))
    {
        warn!("nameplates: FrameXML globals: {e}");
    }
}

/// The globals kept in step with the mode for the life of each VM ([`push_plate_globals`]).
///
/// Behind a [`crate::ui_script::VmMemo`] (1290) because "this VM has been told" is a fact about
/// the VM, not about the process. The world-entry load seeds them earlier still — ahead of
/// `VARIABLES_LOADED`, which no `Update` system can reach — so this is the steady-state half:
/// a V press, an options checkbox, a `/console` write, an addon's `SetCVar`.
fn feed_plate_globals(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mode: Res<VPlateMode>,
    mut told: Local<crate::ui_script::VmMemo<Option<(bool, bool)>>>,
) {
    let Some(script) = script else {
        return;
    };
    let now = (mode.enemies, mode.friends);
    let told = told.get(&script);
    if *told != Some(now) {
        *told = Some(now);
        push_plate_globals(&script, *mode);
    }
}

/// NAMEPLATES / FRIENDNAMEPLATES / ALLNAMEPLATES through the binding table (defaults V /
/// SHIFT-V / CTRL-V). The typing gate and 0585's modifier law live in the dispatch now.
///
/// The V and SHIFT-V halves stay benilla's **independent** toggles (the shipped behavior) rather
/// than 1.12's exclusive dance (whose NAMEPLATES body also force-hides friendly plates — a
/// recorded divergence, 0997 residue). ALLNAMEPLATES arrives with the table and takes the 1.12
/// body's semantics as-is: both on unless both already on, else both off.
///
/// **A key press mirrors into the CVar table** ([`VPlateMode`]'s doc): `set_cvar_engine` is the
/// minimap-zoom pattern — the engine's own value moved, so the table follows it AND queues the
/// change, which is what dirties `config.toml` and what makes the Nameplates page's checkboxes
/// read right the next time they are opened. The resource stays authoritative either way: with no
/// UI VM (a capture, a bare test app) the mirror is a silent no-op and V still works.
fn toggle_vplates(
    binds: Res<crate::bindings::BindingsState>,
    mut mode: ResMut<VPlateMode>,
    mut cvars: ResMut<crate::cvars::Cvars>,
) {
    use crate::bindings::cmd;
    let (was_enemies, was_friends) = (mode.enemies, mode.friends);
    if binds.fired(cmd::NAMEPLATES) {
        mode.enemies = !mode.enemies;
        info!(
            "nameplates: enemy {}",
            if mode.enemies { "ON" } else { "OFF" }
        );
    }
    if binds.fired(cmd::FRIEND_NAMEPLATES) {
        mode.friends = !mode.friends;
        info!(
            "nameplates: friendly {}",
            if mode.friends { "ON" } else { "OFF" }
        );
    }
    if binds.fired(cmd::ALL_NAMEPLATES) {
        let both = mode.enemies && mode.friends;
        mode.enemies = !both;
        mode.friends = !both;
        info!("nameplates: all {}", if !both { "ON" } else { "OFF" });
    }
    let flag = |b: bool| if b { "1" } else { "0" };
    if mode.enemies != was_enemies {
        cvars.set(CVAR_ENEMIES, flag(mode.enemies));
    }
    if mode.friends != was_friends {
        cvars.set(CVAR_FRIENDS, flag(mode.friends));
    }
}

/// The world/reaction inputs of the plate gate, bundled (the 16-param ceiling).
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled system param — the app's convention
struct PlateWorld<'w, 's> {
    units: Query<
        'w,
        's,
        (
            Entity,
            &'static NetEntity,
            &'static Guid,
            &'static Transform,
            Option<&'static ObjectStore>,
        ),
        Without<SelfPlayer>,
    >,
    self_q: Query<'w, 's, (&'static Transform, Option<&'static ObjectStore>), With<SelfPlayer>>,
    factions: Option<Res<'w, Factions>>,
    reputations: Res<'w, Reputations>,
    selection: Res<'w, Selection>,
    // The mouseover unit (the 3-D pick ∪ last frame's plate rects) — with `selection`, the
    // highlight's trigger pair (the watcher's two globals `[0xb4e2c8]`/`[0xb4e2d8]`).
    hovered: Res<'w, Hovered>,
    camera: Query<'w, 's, (&'static Camera, &'static Transform), With<WorldCamera>>,
    // The cursor, for the plate-rect hover (OnEnter — the yellow name, this frame's rects).
    window: Query<'w, 's, &'static Window, With<bevy::window::PrimaryWindow>>,
    // The pending ground-target cast — the plate's `+0x3c` hit-test veto (`0x7cba30`).
    targeting: Res<'w, crate::spell::SpellTargeting>,
}

/// Gate + draw, every frame: decide which units carry a plate (into [`VPlates`], the
/// name-exclusivity verdict), seat each one, and hand the result to the widget layer as
/// [`PlateState`] — at constant screen size over the projected anchor + 2/3 yd. Runs after
/// the script extract), after the targeting chain (it reads the frame's selection verdict).
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn drive_vplates(
    mode: Res<VPlateMode>,
    mut plates: ResMut<VPlates>,
    mut plate_hover: ResMut<PlateHover>,
    mut plate_clicks: ResMut<PlateClicks>,
    // Camera freelook, for the mouselook toggle `0x60f830`: plates stop taking the mouse while
    // the pointer is driving the camera.
    rig: Res<crate::player::CameraControl>,
    world: PlateWorld,
    names: Res<NameCache>,
    net_commands: Res<NetCommands>,
    // The widget layer this drives. `None` in a VM-less run (a capture with the
    // interface off, a bare test app) — the gate below then costs one early return.
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    // The seam this frame's px↔unit conversion runs at (`windowH/768 × uiScale`).
    ui_scale: Res<crate::ui_script::UiScaleCvar>,
    // The overhead-anchor inputs ([`overhead_anchor`]).
    anchor_q: (
        Query<&BoneAttach>,
        Query<&benilla_world::rig_anim::RigPose>,
        Query<&OverheadFallback>,
        Query<&GlobalTransform>,
        Query<(), With<crate::entities::mount::MountChild>>,
    ),
    // The frame's claim bucket 0 (nameplates) — cleared and rebuilt every pass; worldtext owns
    // bucket 1 in `combat_text` and the two never interact (a plate never pushes a number).
    mut bucket: Local<crate::smart_rect::SmartBucket>,
    // The raid-target board — the plate's raid-icon child reads it.
    group: Res<crate::ui_party::GroupState>,
    // Whether this VM's plates have been told about freelook (the `0x60f830` edge).
    mut mouse_told: Local<crate::ui_script::VmMemo<Option<bool>>>,
) {
    plates.0.clear();
    plate_hover.0 = None;
    bucket.clear();
    // **Every early return has to RETIRE the plates first, and that is new with 2148.** A painter
    // could stop drawing and the plates were gone with the frame's quads; widgets stay until they
    // are hidden, so a V press that turns plates off — or a camera-less frame, or a world exit —
    // would otherwise leave the last frame's plates standing on screen forever. `sync` with no
    // states is exactly the reference's own answer: `0x608a10` on every live plate, hiding each and
    // returning it to the pool.
    let retire_all = |script: Option<NonSendMut<benilla_ui::script::UiScript>>| {
        if let Some(mut script) = script {
            script.retire_nameplates();
        }
    };
    if !mode.enemies && !mode.friends {
        retire_all(script);
        return;
    }
    let (Ok((cam, cam_pose)), Ok((self_tf, self_store))) =
        (world.camera.single(), world.self_q.single())
    else {
        retire_all(script);
        return;
    };
    let Some(mut script) = script else {
        return;
    };
    let cam_tf = GlobalTransform::from(*cam_pose);
    let Some(viewport) = cam.logical_viewport_size() else {
        script.retire_nameplates();
        return;
    };
    let basis = plate_basis(viewport);
    let gx = |v: f32| gx_px(v, basis);
    let window = world.window.single().ok();
    // **The mouselook toggle** (`0x60f830`, called from `0x483e80`/`0x483e70`): plates stop taking
    // the mouse while the camera is in freelook, so a right-drag that starts over a plate turns the
    // camera instead of clicking the plate, and the plates are not holding a pointer that has left
    // the screen. Written on the EDGE, not per frame — the reference's own is two call sites on the
    // freelook transitions, and the memo is keyed to the VM because that is what it is memory about
    // (1290). A fresh VM's plates are born with the bit, so the memo starting empty is right.
    // Which unit's plate the pointer is inside (last frame's layout), and the completed clicks
    // waiting on the targeting chain. Both come from the plate widgets themselves, which is the
    // reference's own arrangement: the plate's OnEnter publishes the mouseover and its click slot
    // ends in `SetSelection`.
    let looking = rig.is_looking();
    if *mouse_told.get(&script) != Some(looking) {
        *mouse_told.get(&script) = Some(looking);
        script.set_nameplate_mouse(!looking);
    }
    // **The plate's OWN veto** (`0x7cba30`, the `+0x3c` override), which is a different mechanism
    // from the freelook toggle above and has to stay one: while a ground-targeted spell is armed a
    // plate refuses the hit test *before* the rect, so the reticle can be placed through it — and
    // it does that without touching the mouse-enabled bit, so `IsMouseEnabled()` still answers
    // truthfully to an addon. `TargetingWants::Location` is `0x6e6320`'s `flag & 0x60` verbatim.
    // Written every frame rather than on an edge: it is a plain flag read at hit-test time, not a
    // walk over the plate list, so there is no edge worth memoising.
    script.set_nameplate_hit_test_veto(
        world
            .targeting
            .wants(crate::spell::targeting::TargetingWants::Location),
    );
    let hovered_key = script.hovered_nameplate();
    let clicked = script.take_nameplate_clicks();
    let my_level = self_store.and_then(|s| s.0.unit_level()).unwrap_or(1);
    let has_target = world.selection.target.is_some();

    let (pw, ph) = (gx(PLATE_W), gx(PLATE_H));
    let scale = window.map_or(1.0, |w| w.scale_factor());
    // The px↔unit seam. Every number below is computed in window px exactly as it always was —
    // the projection, the seat, the solve, the device snap — and divided through this once, at
    // the boundary, because the widget layer speaks FrameXML units.
    let seam = window.map_or(1.0, |w| {
        crate::ui_script::seam_scale(w.height(), ui_scale.0)
    });
    let mut states: Vec<PlateState> = Vec::new();

    // The seat PRIORITY (`0x608870`/`0x6089a0` + the per-frame walk `0x608ce0`): plates seat in
    // squared-distance order from the fixed gx point (0.4, 0.3) — the 4:3 screen center, a
    // literal constant in the ref, converted here over the diagonal basis with the Y mirrored
    // out of the ref's Y-up device space. The first-seated plate never moves; each later one is
    // solved off the plates already claimed this frame (bucket 0) — the reference's bouncing,
    // never-overlapping plates. Gate + project first, then seat in sorted order. The TRUE
    // diagonal, not the 0185-damped `basis`: the point is positional law, not plate size.
    let diag = viewport.length();
    let sort_pt = Vec2::new(0.4 * diag, viewport.y - 0.3 * diag);
    let mut cands = Vec::new();
    for (entity, net, guid, tf, store) in &world.units {
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        // NOT_SELECTABLE never gets a plate — `0x60f600`'s gate 2 (`0x60f622 shr ecx,0x19` /
        // `0x60f628 jne 0x60f740` → pool + hide + clear `[unit+0xe60]`), unconditional, and re-run
        // every tick because `0x60f600`'s caller `0x607ef9` sits inside CGUnit's OnUpdate
        // (`vtable+0x38` = `0x607ed0`). So the flag arriving on a plated unit takes its plate away
        // on the next tick, which falls out of this per-frame gate for free.
        //
        // **This suppression is load-bearing, not cosmetic**: a plate hover
        // publishes the mouseover *directly* — `0x7cb850 OnEnter` → `0x7cb869 call 0x492890`, with
        // none of the `IsSelectable` grading the world hover gets at `0x482982`. If a flagged unit
        // ever kept its plate, hovering that plate would hand it a name tooltip the reference never
        // shows. The plate gate is the only thing standing there.
        if store.is_some_and(|s| s.0.unit_flags() & (1 << 25) != 0) {
            continue;
        }
        // A DEAD unit shows no plate — the per-tick gate `0x60f600`'s FIRST, unconditional
        // test: signed `UNIT_FIELD_HEALTH ≤ 0`, absent = 0 (the
        // zero-init descriptor). A poll, not a death callback; the lootable bit is never
        // consulted. Feign death keeps health > 0, so it clears THIS leg — the gate that takes a
        // feigning body's plate away is the hostile-only one below.
        if store.is_none_or(|s| s.0.unit_health().unwrap_or(0) == 0) {
            continue;
        }
        // …and the SAME per-tick gate's second leg (`0x60f62e` — corrects this file's older
        // "ghost stays plated" note): `bytes_1 byte3 & 3` (ghost | creep) denies the plate — a
        // released ghost (health 1) carries none.
        if store.is_some_and(|s| s.0.unit_is_ghost_visual()) {
            continue;
        }
        // 20 yd, hardcoded, from the player.
        if (tf.translation - self_tf.translation).length_squared() > MAX_DIST_SQ {
            continue;
        }
        // A unit with a CREATEDBY owner but no SUMMONEDBY owner, carrying `UNIT_FIELD_FLAGS`
        // bit 9 (`0x200`), gets no plate in EITHER category (`0x60f70b`–`0x60f72d`, byte-verified
        // — the same CHARMEDBY→SUMMONEDBY→CREATEDBY triple the standalone owner accessor
        // `0x611670` walks in the same order). The bit's vanilla NAME is unrecorded; the number
        // is what the binary tests, so that is what we test.
        if store.is_some_and(|s| {
            s.0.unit_created_by().is_some_and(|g| g != 0)
                && s.0.unit_summoned_by().is_none_or(|g| g == 0)
                && s.0.unit_flags() & 0x200 != 0
        }) {
            continue;
        }
        // **The enemy/friendly split is `CanAttack`, run PLAYER → UNIT** — not a reaction-rank
        // threshold, which is what this shipped with and what 1530 corrects. The two halves of a
        // plate genuinely run the reaction in opposite directions: the CATEGORY here asks "can I
        // attack it?" (and for a rep-slot faction that question is answered by the AT-WAR bit,
        // never by the standing), while the bar COLOUR below asks the unit for its reaction toward
        // us — where the standing IS the input. A not-at-war neutral-standing NPC is therefore a
        // friendly-category plate with a yellow bar, exactly as the reference draws it.
        let rank = ring_reaction(
            world.factions.as_deref(),
            &world.reputations,
            store,
            self_store,
        );
        let is_player = net.kind == EntityKind::Player;
        let friendly = crate::target::ring::plate_is_friendly(
            world.factions.as_deref(),
            &world.reputations,
            store,
            self_store,
            is_player,
        );
        if friendly && !mode.friends || !friendly && !mode.enemies {
            continue;
        }
        // The gate's dead-looking leg, which sits on the HOSTILE side of that same split
        // (`0x60f72f test al,al; jne` → friendly goes to the creature-type check `0x60f750`;
        // hostile falls into `0x60f733 shr eax,5; test al,1` → hide). So a feigning hunter's
        // plate vanishes for the enemies he is hiding from, which is the point of the spell,
        // while a friendly one keeps his.
        if !friendly && store.is_some_and(|s| s.0.unit_reads_dead()) {
            continue;
        }
        // Project the plate point: the overhead anchor + 2/3 yd, per frame, no smoothing — and
        // honour the projector's ACCEPT verdict ([`crate::ui_pass::project_overlay`]): behind the
        // camera or outside the viewport, there is no plate. That is the reference's own law, and
        // a hard one — `0x60f600` **destroys** the plate frame on a false verdict, before it ever
        // reaches the seat.
        //
        // Ours instead ran an off-screen point into the seat, whose clamp translates a rect from
        // anywhere bodily onto the screen: **30% of every drawn plate** was a unit nobody could
        // see, pinned to a border, claiming bucket space and shoving the plates of units you can
        // (1341 — two thirds of them while standing still, and 90% of the reported jitter).
        let anchor = overhead_anchor(
            entity,
            tf,
            &anchor_q.0,
            &anchor_q.1,
            &anchor_q.2,
            &anchor_q.3,
            &anchor_q.4,
        );
        let Some(screen) =
            crate::ui_pass::project_overlay(cam, &cam_tf, anchor + Vec3::Y * PLATE_LIFT, viewport)
        else {
            continue;
        };
        cands.push((
            screen.distance_squared(sort_pt),
            screen,
            anchor,
            rank,
            is_player,
            entity,
            guid,
            store,
        ));
    }
    cands.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, screen, anchor, rank, is_player, entity, guid, store) in cands {
        plates.0.insert(entity);

        // The target-highlight relative alpha: target (or nobody targeted) opaque, others 0x7F.
        let alpha = if !has_target || world.selection.target == Some(entity) {
            1.0
        } else {
            DIM_ALPHA
        };
        // The bar tint: the NAMEPLATE palette in `0x7cbaa0`'s exact order.
        let tint = plate_tint(rank, is_player);
        // The frame SEAT (`0x509ec0`): the plate's TOP-CENTER lands on the projected
        // point — the plate HANGS BELOW head + 2/3 yd — and the point is edge-clamped half a
        // plate inside every screen border (`SetPoint(TOP ← root.BOTTOMLEFT, clampedX/Y)`).
        // The highlight is a 2-D hover over THIS rect (the frame's OnEnter — yellow name). `pw`/`ph`
        // (the plate's logical size) are hoisted above the loop — the border resample keys off them.
        // Geometry trace for the vplates capture (`WOW_VPLATE_TRACE=1`): the exact plate rects
        // this frame, in logical px — the machine-side check the capture PNG can't give
        // (fill/border/text hues overlap under zoom). The `vpl` lines of the shared trace below
        // are read back by `scripts/vplsum.py`.
        static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let trace = *TRACE.get_or_init(|| std::env::var("WOW_VPLATE_TRACE").as_deref() == Ok("1"));
        // The full seat (`0x509ec0`): the desired rect TOP-anchored on the raw projected point
        // (the plate hangs below head + 2/3 yd), then the bucket-0 seat law — normalize, SOLVE
        // off the plates already claimed this frame, clamp the resolved center-X/top half a
        // plate inside every border, rebuild ([`crate::smart_rect`]). The old bare edge-clamp
        // was this law minus the solve.
        let desired = Rect::new(
            screen.x - pw * 0.5,
            screen.y,
            screen.x + pw * 0.5,
            screen.y + ph,
        );
        let solved = bucket.resolve(desired, viewport);
        // The rect snaps to the device pixel grid ([`device_snap`]) — the LEFT edge, not the
        // centre: an odd width would put a snapped centre's edges back between texels. The CLAIM
        // takes the snapped rect, so later plates dodge exactly what is drawn.
        let top = device_snap(solved.min.y, scale);
        let left = device_snap(solved.min.x, scale);
        let plate = Rect::new(left, top, left + pw, top + ph);
        // **The jitter decomposition** (`WOW_MOVE_TRACE` tag `vpl`, one line per plate per frame):
        // the world anchor, the camera pose that projected it, the raw projected point, the
        // solver's answer, and the snapped rect — so "the plates are jittery when moving" is
        // attributed to one of the four (a moving anchor, a noisy camera, a flipping solve, the
        // pixel snap) from numbers rather than from the eye. It rides the shared trace — tag-
        // filtered, one line per plate — and not the `WOW_VPLATE_TRACE` eprintlns beside it, whose
        // four unbuffered writes per plate per frame would distort the frame pacing the question
        // is about (the 0880 lesson).
        if benilla_assets::trace::enabled_for("vpl") {
            let (cp, cf) = (cam_pose.translation, cam_pose.forward());
            benilla_assets::trace::line(
                "vpl",
                &format!(
                    "e={} vp=({:.0},{:.0}) anchor=[{:.4},{:.4},{:.4}] cam=[{:.4},{:.4},{:.4}] \
                     fwd=[{:.4},{:.4},{:.4}] scr=({:.3},{:.3}) solved=({:.3},{:.3}) plate=({:.1},{:.1})",
                    entity.index(),
                    viewport.x,
                    viewport.y,
                    anchor.x,
                    anchor.y,
                    anchor.z,
                    cp.x,
                    cp.y,
                    cp.z,
                    cf.x,
                    cf.y,
                    cf.z,
                    screen.x,
                    screen.y,
                    solved.min.x,
                    solved.min.y,
                    plate.min.x,
                    plate.min.y,
                ),
            );
        }
        bucket.claim(plate);
        // The plate's own hover, from last frame's layout: the widget layer answers which unit's
        // plate the pointer is inside, and the driver resolves it to this frame's entity below.
        let hover = hovered_key == Some(guid.0);
        // The highlight trigger — the watcher's OR over the two globals: this unit is the
        // MOUSEOVER (the 3-D body pick, or the plate's own hover — [`PlateHover`], which the
        // widget layer answered above) or the current TARGET. `hover` joins in directly so the
        // bar brighten never lags the yellow name by the frame the mouseover takes to publish.
        let lit =
            hover || world.hovered.target == Some(entity) || world.selection.target == Some(entity);
        if trace {
            eprintln!(
                "vplate-trace: viewport={viewport:?} screen=({:.2},{:.2}) plate=({:.1},{:.1})..({:.1},{:.1}) lit={lit}",
                screen.x, screen.y, plate.min.x, plate.min.y, plate.max.x, plate.max.y
            );
        }
        // ── The plate's state, handed to the widget layer ─────────────────────────────────
        //
        // Everything above is unchanged: the gate, the anchor, the projection's ACCEPT verdict,
        // the distance-sorted seat and the bucket solve are all about the WORLD, and they stay
        // the app's. What changed in decision 2148 is only what happens with the answer — it used
        // to become six quads, and it now becomes one [`PlateState`] for a real widget, which the
        // shared frame→quad path draws and an addon can read, hook, restyle or take over.
        //
        // The seam scale (`windowH/768 × uiScale`) converts our window px into the FrameXML units
        // the widget layer speaks. Dividing it out here is deliberate and is a DIVERGENCE worth
        // naming: the reference's plates are outside the `uiScale` cascade (uiScale is
        // `UIParent`'s own frame scale there, and the WorldFrame is a sibling root), while
        // benilla folds uiScale into one global seam. Until that seam grows a
        // per-cascade answer, the driver compensates, so a plate's PIXELS stay uiScale-blind the
        // way the reference's are.
        // The raid-target board: 1-based here, 0-based for the atlas cell.
        let mark = group.raid_target_index(guid.0);
        let x_units = (plate.min.x + plate.max.x) * 0.5 / seam;
        let y_units = (viewport.y - plate.min.y) / seam;
        if hovered_key == Some(guid.0) {
            plate_hover.0 = Some(entity);
        }
        for click in &clicked {
            if click.key == guid.0 {
                match click.button.as_str() {
                    "RightButton" => plate_clicks.right.push(entity),
                    _ => plate_clicks.left.push(entity),
                }
            }
        }
        states.push(PlateState {
            // The plate's identity is the UNIT's, for the life of the plate — the reference's
            // `[unit+0xe60]` binding, which every addon's per-plate cache rests on.
            key: guid.0,
            top_centre: (x_units, y_units),
            // RAW health and its max — `healthbar:GetValue()` is `[bar+0x320]`, not a fraction
            // (`0x78f5d0`). pfUI and CustomNameplates both divide, and a fraction here would read
            // as full health.
            health: store.and_then(|s| s.0.unit_health()).unwrap_or(0) as f32,
            max_health: store
                .and_then(|s| s.0.unit_max_health())
                .unwrap_or(1)
                .max(1) as f32,
            bar_colour: [tint[0], tint[1], tint[2]],
            name: names
                .resolve_unit(guid.0, store, &net_commands)
                .map(str::to_owned)
                .unwrap_or_default(),
            // The skull's two legs (`0x7cbb40`): a WORLD BOSS — creature-
            // classification rank 3, through the client's own getter (`gated_rank`, decision
            // 0782, which is why a MIND-CONTROLLED boss shows its number) — unconditionally; or a
            // hostile ≥ 10 levels up that isn't trivial-grey (`0x5f0700`, the shared check,
            // vacuous on a ≥ +10 hostile and kept for transcription fidelity).
            //
            // **A unit with no level yet is not a boss.** The two facts travel separately, because
            // folding them into one `Option` made a missing `UNIT_FIELD_LEVEL` read as a skull —
            // the old painter kept the whole block inside `if let Some(level)`, so no level meant
            // no number AND no skull, and that is the behaviour restored here.
            level: store.and_then(|s| s.0.unit_level()),
            skull: store.and_then(|s| s.0.unit_level()).is_some_and(|level| {
                crate::names::gated_rank(
                    store
                        .and_then(|s| s.0.object_entry())
                        .and_then(|e| names.creature_record(e)),
                    store,
                ) == 3
                    || (rank <= 1 && level >= my_level + 10 && !unit_is_grey(my_level, level))
            }),
            level_colour: {
                let c = store
                    .and_then(|s| s.0.unit_level())
                    .map_or([1.0, 1.0, 1.0, 1.0], |level| con_color(my_level, level));
                [c[0], c[1], c[2]]
            },
            // The 0434 §6 board, 0-based for the atlas (`col = idx & 3`, `row = idx >> 2`).
            raid_icon: (mark >= 1).then(|| mark - 1),
            alpha,
            lit,
            hovered: hover,
        });
    }

    // The window-derived half, once per frame — the widget layer rewrites the static anchors only
    // when this actually moves, which is what keeps an addon's own re-anchoring alive between
    // resizes.
    script.sync_nameplates(
        PlateGeometry {
            width: pw / seam,
            height: ph / seam,
            bar_off_x: gx(BAR_OFF_X) / seam,
            bar_off_y: gx(BAR_OFF_Y) / seam,
            bar_width: gx(BAR_W) / seam,
            bar_height: gx(BAR_H) / seam,
            level_off_x: gx(LEVEL_OFF_X) / seam,
            level_off_y: gx(LEVEL_OFF_Y) / seam,
            skull_size: gx(SKULL_SIZE) / seam,
            raid_size: gx(RAID_ICON_SIZE) / seam,
            name_height: text_px(NAME_H, basis) / seam,
            level_height: text_px(LEVEL_H, basis) / seam,
            // A constant ONE LOGICAL PIXEL of drop shadow, in the widget layer's units — the
            // director's 2026-07-07 tuning, which the shared text arm would otherwise scale by
            // this same seam and draw 2 px thick from a 1152-tall window up.
            shadow_offset: 1.0 / seam,
        },
        &states,
    );
}

/// The set the plate drive runs in — the overhead-name driver orders after it (the
/// ShouldShowName exclusivity: a plated unit draws no floating name).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct VPlateSet;

/// V-key nameplates: the toggles + the per-frame gate/draw.
pub(crate) struct VPlatesPlugin;

/// The two V-plate toggles' change callback — the bitmask's two bits, flags
/// like every other checkbox. Lowercased here like every arm; the consts carry the registered
/// spelling.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut mode: ResMut<VPlateMode>) {
    if ev.is(CVAR_ENEMIES) {
        mode.enemies = ev.flag();
    } else if ev.is(CVAR_FRIENDS) {
        mode.friends = ev.flag();
    }
}

impl Plugin for VPlatesPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        app.init_resource::<VPlateMode>()
            .init_resource::<VPlates>()
            .init_resource::<PlateHover>()
            .init_resource::<PlateClicks>()
            .add_systems(
                Update,
                (
                    toggle_vplates,
                    // After the targeting chain (selection/hover verdicts) — which is
                    // `.after(WorldStage::Input)`, so the camera this projects through is THIS
                    // frame's, the freshest data a plate can be built from.
                    //
                    // **And the paint is this frame's too, since decision 2168.** The UI pass is
                    // two systems: the tick/resolve half stays ahead of `WorldStage::Input`
                    // (the hit test feeds `PointerOverUi`, which the camera reads), and the QUAD
                    // half — `ui_script::extract::paint_script` — runs after this driver. 2148
                    // shipped with the whole pass ahead of the camera, so the anchors written here
                    // were drawn by the NEXT frame's extract: the seat reached the paint 16 ms
                    // late, every frame, which is the director's "way more jittered when the
                    // creature is moving" (measured both ways on the `vpl` trace, 2026-09-10 —
                    // median driver→paint gap 16.0 ms before, 0.0 ms after).
                    drive_vplates.after(TargetUpdate),
                )
                    .chain()
                    .in_set(VPlateSet),
            )
            // **Outside [`VPlateSet`] deliberately.** Its only ordering need is to be ahead of the
            // script tick, and three sets order *after* `VPlateSet` while `drive_vplates` runs
            // after the targeting chain — pulling the whole set in front of `UiInput` to carry one
            // system would rewire all of that. A V press can therefore reach the globals a frame late,
            // which costs nothing: their only readers are `UpdateNameplates` at the two world-entry
            // events and an addon that calls it, never a per-frame path.
            .add_systems(Update, feed_plate_globals.in_set(crate::ui_script::UiFeed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client's own con law (`0x7cbd50`): thresholds at level 30 (grayband[6] = 7 → green
    /// down to 23, gray at 22), the exact softened dwords, and the low-level edge (grayband[0] =
    /// 4, so a level-1 never grays anything).
    #[test]
    fn con_color_matches_the_byte_table() {
        assert_eq!(con_color(30, 35), CON_RED);
        assert_eq!(con_color(30, 33), CON_ORANGE);
        assert_eq!(con_color(30, 28), CON_YELLOW);
        assert_eq!(
            con_color(30, 23),
            CON_GREEN,
            "gap 7 = grayband[6], still green"
        );
        assert_eq!(con_color(30, 22), CON_GRAY, "gap 8 crosses the band");
        assert_eq!(con_color(1, 1), CON_YELLOW);
        assert_ne!(
            con_color(5, 1),
            CON_GRAY,
            "grayband[1] = 4 ≥ the max gap at 5"
        );
        // The exact dwords (0xFFFF1919 / 0xFFFF7F3F / 0xFF3FB23F): byte-pinned, not the
        // FrameXML QuestDifficultyColor approximations.
        assert_eq!(CON_RED[1], 25.0 / 255.0);
        assert_eq!(CON_ORANGE[2], 63.0 / 255.0);
        assert_eq!(CON_GREEN[1], 178.0 / 255.0);
    }

    /// The snap is on the DEVICE grid, not the logical one. At the 2× scale we develop on, a
    /// logical `round()` moved the plate two physical pixels at a time; this moves it one, which
    /// is the smallest step that still lands the border blit on a texel boundary. Pinned so the
    /// `scale` argument can't be "simplified" away back into `round()`.
    #[test]
    fn the_plate_snaps_on_the_device_grid_not_the_logical_one() {
        // 2× display: the grid is every half logical pixel, and every snapped value is a whole
        // number of PHYSICAL pixels.
        for (v, want) in [(10.0, 10.0), (10.2, 10.0), (10.3, 10.5), (10.6, 10.5)] {
            let got = device_snap(v, 2.0);
            assert_eq!(got, want, "{v} at 2×");
            assert_eq!((got * 2.0).fract(), 0.0, "{got} is a whole physical pixel");
        }
        // 1×: unchanged from the old law.
        assert_eq!(device_snap(10.4, 1.0), 10.0);
        assert_eq!(device_snap(10.6, 1.0), 11.0);
        // 1.5× (the Windows norm), where a logical round() was never texel-aligned at all.
        assert_eq!((device_snap(10.4, 1.5) * 1.5).fract(), 0.0);
        assert_eq!((device_snap(10.9, 1.5) * 1.5).fract(), 0.0);
    }

    /// Plate text ems ride the gx DIAGONAL like the frame geometry, growth-damped past the
    /// director's [`PLATE_DIAG_KNEE`] — name em 13 at the reference's 1152×648 window (measured),
    /// level em 11 (director-pinned one under the byte 0.009); at larger windows the em lands midway
    /// between the byte law and the 0185 native pin (2560×1440: byte law 29, pin 13 → ours 21). The
    /// raw em still carries the client's 32 atlas-cell cap.
    #[test]
    fn plate_text_sizes_take_the_damped_diagonal_basis() {
        let ref43 = plate_basis(Vec2::new(1024.0, 768.0));
        let refwin = plate_basis(Vec2::new(1152.0, 648.0));
        assert_eq!(text_px(NAME_H, ref43), 13.0);
        assert_eq!(text_px(LEVEL_H, ref43), 11.0);
        assert_eq!(text_px(NAME_H, refwin), 13.0);
        assert_eq!(text_px(LEVEL_H, refwin), 11.0);
        let big = plate_basis(Vec2::new(2560.0, 1440.0));
        assert_eq!(text_px(NAME_H, big), 21.0, "midway between 29 and 13");
        assert_eq!(text_px(NAME_H, 10_000.0), 32.0, "atlas-cell cap, raw law");
    }

    /// **A key press moves the CVar too** — the half a settings page cannot see for itself. V
    /// flips the resource (the bitmask) AND mirrors into the table as an ENGINE write, which is
    /// what dirties `config.toml` and what the Nameplates page's checkbox reads next time it
    /// opens. Without the mirror, plates toggled by key would silently revert at every launch and
    /// the window would show the wrong state.
    ///
    /// The table is seeded at `"0"/"0"` because that is what the app seeds it with: both plate
    /// CVars boot at the reference's OFF since 1804 ([`VPlateMode::default`]), so the first V of a
    /// session turns plates ON. The mirror is direction-blind, and the second press below is here
    /// to say so.
    #[test]
    fn the_v_key_mirrors_into_the_cvar_table() {
        use crate::bindings::{cmd, BindingsState};
        use crate::cvars::Cvars;
        let mut app = App::new();
        app.add_systems(Update, toggle_vplates)
            .init_resource::<VPlateMode>()
            .init_resource::<Cvars>()
            .insert_resource(BindingsState::test_fired(&[cmd::NAMEPLATES]));
        app.update();
        assert!(app.world().resource::<VPlateMode>().enemies, "V turns on");
        let moved = |app: &mut App| {
            app.world_mut()
                .resource_mut::<Cvars>()
                .take_events()
                .into_iter()
                .map(|e| (e.name, e.new))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            moved(&mut app),
            vec![(CVAR_ENEMIES.to_string(), "1".to_string())],
            "the write is an accepted move, so the config dirties and the mirror learns it"
        );
        assert_eq!(
            app.world().resource::<Cvars>().get(CVAR_FRIENDS),
            Some("0"),
            "untouched"
        );

        // And back: the same key mirrors the OFF as a host write too.
        app.world_mut()
            .insert_resource(BindingsState::test_fired(&[cmd::NAMEPLATES]));
        app.update();
        assert!(!app.world().resource::<VPlateMode>().enemies, "V turns off");
        assert_eq!(
            moved(&mut app),
            vec![(CVAR_ENEMIES.to_string(), "0".to_string())]
        );

        // Shift-V is the other bit, and only the other bit.
        app.world_mut()
            .insert_resource(BindingsState::test_fired(&[cmd::FRIEND_NAMEPLATES]));
        app.update();
        assert!(app.world().resource::<VPlateMode>().friends);
        assert_eq!(
            moved(&mut app),
            vec![(CVAR_FRIENDS.to_string(), "1".to_string())]
        );

        // A frame with nothing fired writes nothing at all.
        app.world_mut().insert_resource(BindingsState::default());
        app.update();
        assert!(moved(&mut app).is_empty());
    }

    /// **The FrameXML mirror is owed to every VM** — the mode's two bits reach
    /// `NAMEPLATES_ON`/`FRIENDNAMEPLATES_ON` as the reference's `1`-or-nil, follow a change, and
    /// are handed to a rebuilt VM (a `/reload`) without one.
    #[test]
    fn the_plate_globals_follow_the_mode_and_survive_a_rebuilt_vm() {
        let read = |app: &mut App| {
            let s = app
                .world_mut()
                .non_send_resource_mut::<benilla_ui::script::UiScript>();
            (
                s.lua().globals().get::<Option<i64>>("NAMEPLATES_ON").ok(),
                s.lua()
                    .globals()
                    .get::<Option<i64>>("FRIENDNAMEPLATES_ON")
                    .ok(),
            )
        };
        let mut app = App::new();
        app.add_systems(Update, feed_plate_globals)
            .init_resource::<VPlateMode>()
            .insert_non_send_resource(benilla_ui::script::UiScript::new().unwrap());

        app.update();
        assert_eq!(read(&mut app), (Some(None), Some(None)), "both off ⇒ nil");

        app.world_mut().resource_mut::<VPlateMode>().enemies = true;
        app.update();
        assert_eq!(
            read(&mut app),
            (Some(Some(1)), Some(None)),
            "the reference's own truthiness: the NUMBER 1, never a truthy `0`"
        );

        // `ReloadUI()`: a fresh VM, and the mode did not move. The memo is keyed on the VM's
        // session (1290), so the new one is told again rather than inheriting the old one's claim.
        app.insert_non_send_resource(benilla_ui::script::UiScript::new().unwrap());
        assert_eq!(
            read(&mut app),
            (Some(None), Some(None)),
            "a fresh VM knows nothing"
        );
        app.update();
        assert_eq!(
            read(&mut app),
            (Some(Some(1)), Some(None)),
            "…and is handed the mode again"
        );
    }

    /// The palette selector follows `0x7cbaa0`'s exact test order — notably reaction 2
    /// (unfriendly) is YELLOW with 3, not hostile red, and a hostile player is red before the
    /// player-blue test.
    #[test]
    fn plate_tint_matches_the_byte_order() {
        assert_eq!(plate_tint(1, false), PLATE_HOSTILE);
        assert_eq!(plate_tint(1, true), PLATE_HOSTILE, "hostile beats player");
        assert_eq!(plate_tint(2, false), PLATE_NEUTRAL, "unfriendly is yellow");
        assert_eq!(plate_tint(3, false), PLATE_NEUTRAL);
        assert_eq!(plate_tint(3, true), PLATE_PLAYER, "player beats neutral");
        assert_eq!(plate_tint(4, false), PLATE_FRIENDLY);
        assert_eq!(plate_tint(6, true), PLATE_PLAYER, "player beats friendly");
    }

    /// The plate geometry law through the gx DIAGONAL unit, growth-damped past
    /// [`PLATE_DIAG_KNEE`]: at 1024×768 (diag exactly 1280) the 0.1 × 0.025 frame = 128 × 32 px
    /// — the border texture's native size, the bar ≈ 103 × 9 px. Past the knee the frame lands
    /// midway between the byte law and native (1080p: byte law 220, native 128 → ours 174).
    /// Below the knee the diagonal rules unchanged (a smaller window shrinks the plate,
    /// `0x41ad10`).
    #[test]
    fn plate_geometry_damps_past_the_native_knee() {
        let ref43 = plate_basis(Vec2::new(1024.0, 768.0));
        assert_eq!(gx_px(PLATE_W, ref43), 128.0);
        assert_eq!(gx_px(PLATE_H, ref43), 32.0);
        assert_eq!(gx_px(BAR_W, ref43), 103.0);
        assert_eq!(gx_px(BAR_H, ref43), 9.0);
        assert_eq!(
            gx_px(PLATE_W, plate_basis(Vec2::new(1920.0, 1080.0))),
            174.0,
            "midway between 220 and 128"
        );
        assert_eq!(gx_px(PLATE_W, plate_basis(Vec2::new(800.0, 600.0))), 100.0);
    }
}
