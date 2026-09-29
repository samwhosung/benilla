//! V-key nameplates, `CGNamePlateFrame` (ctor `0x7cb250`): health-bar plates drawn as a 2-D
//! overlay that replaces a unit's overhead name. This file owns the world side (gate, anchor,
//! projection, seat, basis); the widgets live in [`benilla_ui::script::nameplate`].
//!
//! - Toggles: the `[0xc4da34]` bitmask, bit 0 enemy and bit 3 friendly, both off at boot.
//! - Gate: never the own unit or a `NOT_SELECTABLE` one; enemy or friendly by `CanAttack` from the
//!   player (`0x606980`), and a player subject must also pass `CanCooperate` (equal faction-group
//!   masks); 20 yd; no occlusion; a unit not projected into view loses its plate (`0x60f600`).
//! - Anchor: the overhead head point (`0x608640`) plus 2/3 yd (`[0x80abfc]`), projected every
//!   frame (`0x483ee0`), the plate's top centre on it (`0x509ec0`), sized in gx units of the
//!   screen diagonal (`0x41ad10`), outside uiScale. Deviation: the basis is damped past
//!   [`PLATE_DIAG_KNEE`], because the reference's unbounded growth (`0x7705b0`) draws plates too
//!   large and soft at high resolutions.
//! - Highlight: the mouseover or the target (`0x606f20` into `0x607080`) lights the plate.
//!   Deviation: the bar fill brightens by `LIT_BOOST` in place of the reference's ADD
//!   `Nameplate-Glow` rim, which reads as hard edge lines under benilla's linear blending.
//!   Hovering a plate makes its unit the mouseover (`0x7cb850`) and turns the name yellow; a
//!   click on it selects.
//! - With a target, every other plate dims ([`DIM_ALPHA`]).

use bevy::ecs::entity::EntityHashSet;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_ui::script::{
    unit_is_grey, PlateGeometry, PlateState, PLATE_BIT_ENEMY, PLATE_BIT_FRIEND,
};

use crate::entities::{overhead_anchor, BoneAttach, OverheadFallback};
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, NetEntity, ObjectStore, Reputations, SelfPlayer};
use crate::target::{ring_reaction, Factions, Hovered, Selection, TargetUpdate};
use benilla_world::view::WorldCamera;

/// The plate border, sharp-resampled so its 128x32 art stays crisp when magnified.
pub(crate) mod border;

/// The two script bits of `[0xc4da34]`, bit `0x1` enemy plates and bit `0x8` friendly: engine
/// state, written only by the four plate verbs ([`apply_plate_verbs`]) and cleared at every world
/// entry ([`clear_at_world_entry`]). Nothing persists it: FrameXML replays its own
/// `NAMEPLATES_ON`/`FRIENDNAMEPLATES_ON` saved variables through the verbs
/// (`UIOptionsFrame.lua:768-780`, `UIParent.lua:234`, `:367`).
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct VPlateMode {
    pub(crate) enemies: bool,
    pub(crate) friends: bool,
}

impl VPlateMode {
    fn bits(self) -> u8 {
        let bit = |on: bool, b: u8| if on { b } else { 0 };
        bit(self.enemies, PLATE_BIT_ENEMY) | bit(self.friends, PLATE_BIT_FRIEND)
    }

    fn from_bits(bits: u8) -> Self {
        Self {
            enemies: bits & PLATE_BIT_ENEMY != 0,
            friends: bits & PLATE_BIT_FRIEND != 0,
        }
    }
}

/// The units carrying a live plate this frame; the overhead-name driver draws no name for them
/// (the reference's ShouldShowName).
#[derive(Resource, Default)]
pub(crate) struct VPlates(pub(crate) EntityHashSet);

/// The unit whose plate the pointer is inside, from last frame's layout: the plate's OnEnter
/// publishing the mouseover (`0x7cb850` into `[0xb4e2c8]`), since the world pick stands down
/// over UI.
#[derive(Resource, Default)]
pub(crate) struct PlateHover(pub(crate) Option<Entity>);

/// Completed plate clicks, physical or an addon's `plate:Click`, waiting for the targeting chain:
/// the reference's click slot (`0x7cb910`) ends in the same `SetSelection` as a body click.
#[derive(Resource, Default)]
pub(crate) struct PlateClicks {
    pub(crate) left: Vec<Entity>,
    pub(crate) right: Vec<Entity>,
}

/// The plate frame in gx units (`[0x87d9cc]`, `[0x87d9d0]`); child offsets from `0x7cb250` and
/// `0x7cb6d0`.
const PLATE_W: f32 = 0.1;
const PLATE_H: f32 = 0.025;
/// The health bar: BOTTOMLEFT at the plate's BOTTOMLEFT plus the offset.
const BAR_OFF_X: f32 = 0.0031;
const BAR_OFF_Y: f32 = 0.003125;
const BAR_W: f32 = 0.0804;
const BAR_H: f32 = 0.007025;
/// The name's BOTTOM sits at the plate CENTER; the level's CENTER at plate BOTTOMRIGHT plus
/// (-0.0092, +0.0071); the skull overlays the level. Deviation: `LEVEL_H` is 0.0086, one em under
/// the reference's 0.009, because the level number read too large.
const NAME_H: f32 = 0.01;
const LEVEL_H: f32 = 0.0086;
const LEVEL_OFF_X: f32 = 0.0092;
const LEVEL_OFF_Y: f32 = 0.0071;
const SKULL_SIZE: f32 = 0.01;
/// The raid-target icon (`0x7cb250`): its RIGHT on the border's LEFT, hanging off the plate's
/// left edge.
const RAID_ICON_SIZE: f32 = 0.02;
/// `[0x80abfc]`.
const PLATE_LIFT: f32 = 2.0 / 3.0;
/// 20 yd, hardcoded in the reference.
const MAX_DIST_SQ: f32 = 20.0 * 20.0;
/// Deviation: the reference dims non-target plates to `0x7F`; raised because that fades them too
/// far to read.
const DIM_ALPHA: f32 = 178.0 / 255.0;

/// The bar-fill palette, the dwords at `0xcf60d0`/`e8`/`c8`/`dc`, in sRGB.
const PLATE_HOSTILE: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const PLATE_NEUTRAL: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
const PLATE_FRIENDLY: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const PLATE_PLAYER: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// `0x7cbaa0`'s test order: reaction 1 or less red, else a player blue, else 4 or more green,
/// else (2 and 3) yellow.
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

/// The level con palette, `0x7cbd50`'s dwords, not FrameXML's `QuestDifficultyColor`.
const CON_RED: [f32; 4] = [1.0, 25.0 / 255.0, 25.0 / 255.0, 1.0];
const CON_ORANGE: [f32; 4] = [1.0, 127.0 / 255.0, 63.0 / 255.0, 1.0];
const CON_YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
const CON_GREEN: [f32; 4] = [63.0 / 255.0, 178.0 / 255.0, 63.0 / 255.0, 1.0];
const CON_GRAY: [f32; 4] = [127.0 / 255.0, 127.0 / 255.0, 127.0 / 255.0, 1.0];

/// The con colour (`0x7cbd50`): +5 or more red, +3/+4 orange, -2..+2 yellow, then green until
/// the shared grey test ([`unit_is_grey`], table `[0x81dda8]`) says gray.
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

/// Snap a logical-pixel coordinate onto the device pixel grid, so the sharp-resampled border
/// blits 1:1 at any scale factor. Deviation: the reference draws at fractional device
/// coordinates; the snap keeps the magnified border crisp. Shared with the chat bubble.
pub(crate) fn device_snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

/// Deviation: the reference's plates grow with the diagonal without limit (`0x7705b0`), which
/// reads too big and soft; past this diagonal, where the frame is the border art's native
/// 128x32 px, the basis grows at [`PLATE_GROWTH_DAMP`] of that rate.
const PLATE_DIAG_KNEE: f32 = 1280.0;
/// 0 pins the native size, 1 is the reference's rate.
const PLATE_GROWTH_DAMP: f32 = 0.5;

/// One gx unit in pixels: the screen diagonal (`0x41ad10`), never uiScale, damped past
/// [`PLATE_DIAG_KNEE`]. Shared with the chat bubble so its text holds the plate's em.
pub(crate) fn plate_basis(viewport: Vec2) -> f32 {
    let d = viewport.x.hypot(viewport.y);
    if d <= PLATE_DIAG_KNEE {
        d
    } else {
        PLATE_DIAG_KNEE + (d - PLATE_DIAG_KNEE) * PLATE_GROWTH_DAMP
    }
}

/// gx units to pixels for frame geometry; text takes [`text_px`].
pub(crate) fn gx_px(v: f32, basis: f32) -> f32 {
    (v * basis).round()
}

/// A plate FontString height to its em, `min(32, round(h * basis))`: the reference's
/// `0x44d040` and `0x5ca030` net `h` times the diagonal, capped at the 32 px atlas cell, and
/// re-rasterize on every resize (`0x5c2b50`).
pub(crate) fn text_px(h: f32, basis: f32) -> f32 {
    (h * basis).round().min(32.0)
}

/// Every frame: fold the plate verbs' writes into the bits, in the order the VM made them.
pub(crate) fn apply_plate_verbs(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    mut mode: ResMut<VPlateMode>,
) {
    let Some(mut script) = script else {
        return;
    };
    let writes = script.take_nameplate_bit_writes();
    if writes.is_empty() {
        return;
    }
    let now = VPlateMode::from_bits(writes.apply(mode.bits()));
    if *mode != now {
        info!(
            "nameplates: enemy {}, friendly {}",
            if now.enemies { "ON" } else { "OFF" },
            if now.friends { "ON" } else { "OFF" }
        );
        *mode = now;
    }
}

/// The world-entry clear of both script bits (`0x60394b`, `0x603952`, in `0x6033c0`), which the
/// entry setup `0x401570` runs at `0x401639`, after the UI load at `0x401602` (`InitializeGame`,
/// whose `UI_Init` fires `VARIABLES_LOADED` at `0x4900b2`): the load's replay is cleared, and
/// the one at `PLAYER_ENTERING_WORLD` decides. A `ReloadUI()` never reaches it.
pub(crate) fn clear_at_world_entry(world: &mut World) {
    if let Some(mut script) = world.get_non_send_resource_mut::<benilla_ui::script::UiScript>() {
        let _ = script.take_nameplate_bit_writes();
    }
    if let Some(mut mode) = world.get_resource_mut::<VPlateMode>() {
        *mode = VPlateMode::default();
    }
}

/// The `config.toml` names benilla once kept the two bits under, with the FrameXML global each
/// carries over to. 1.12 has no nameplate CVar; the setting is FrameXML's saved variables.
const LEGACY_CVARS: [(&str, &str); 2] = [
    ("nameplateShowEnemies", "NAMEPLATES_ON"),
    ("nameplateShowFriends", "FRIENDNAMEPLATES_ON"),
];

/// A plate setting still in `config.toml` from before it moved to FrameXML's saved variables:
/// each global and whether it was on.
#[derive(Default)]
pub(crate) struct LegacyPlateSettings(Vec<(&'static str, bool)>);

/// Take the legacy plate settings out of `config.toml`, once: the next save drops them, and from
/// then on the saved-variables file carries them.
pub(crate) fn take_legacy_settings(cvars: &mut crate::cvars::Cvars) -> LegacyPlateSettings {
    LegacyPlateSettings(
        LEGACY_CVARS
            .iter()
            .filter_map(|&(cvar, global)| {
                let value = cvars.retire_file_entry(cvar)?;
                Some((global, value.trim() == "1"))
            })
            .collect(),
    )
}

/// Seat the legacy settings over the saved-variables chunk's values, before `VARIABLES_LOADED`,
/// as the number 1 or nil (a Lua 0 is truthy). Until now the setting was the CVar, which
/// outranked the file.
pub(crate) fn seat_legacy_settings(
    script: &benilla_ui::script::UiScript,
    legacy: &LegacyPlateSettings,
) {
    let g = script.lua().globals();
    for &(global, on) in &legacy.0 {
        if let Err(e) = g.set(global, on.then_some(1i64)) {
            warn!("nameplates: carrying {global} over: {e}");
        }
    }
}

/// The world and reaction inputs of the plate gate, bundled under the 16-param ceiling.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled system param
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
    // With `selection`, the highlight's pair: the watcher's globals `[0xb4e2c8]`/`[0xb4e2d8]`.
    hovered: Res<'w, Hovered>,
    camera: Query<'w, 's, (&'static Camera, &'static Transform), With<WorldCamera>>,
    window: Query<'w, 's, &'static Window, With<bevy::window::PrimaryWindow>>,
    // The pending ground-target cast, for the plate's hit-test veto (`0x7cba30`).
    targeting: Res<'w, crate::spell::SpellTargeting>,
}

/// Every frame: gate the units, seat each plate and hand the result to the widget layer as
/// [`PlateState`]s. Runs after the targeting chain, whose selection it reads.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn drive_vplates(
    mode: Res<VPlateMode>,
    mut plates: ResMut<VPlates>,
    mut plate_hover: ResMut<PlateHover>,
    mut plate_clicks: ResMut<PlateClicks>,
    rig: Res<crate::player::CameraControl>,
    world: PlateWorld,
    names: Res<NameCache>,
    net_commands: Res<NetCommands>,
    // `None` in a run with no UI VM.
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    ui_scale: Res<crate::ui_script::UiScaleCvar>,
    anchor_q: (
        Query<&BoneAttach>,
        Query<&benilla_world::rig_anim::RigPose>,
        Query<&OverheadFallback>,
        Query<&GlobalTransform>,
        Query<(), With<crate::entities::mount::MountChild>>,
    ),
    // Claim bucket 0, the plates', rebuilt every frame; combat text owns bucket 1.
    mut bucket: Local<crate::smart_rect::SmartBucket>,
    group: Res<crate::ui_party::GroupState>,
    mut mouse_told: Local<crate::ui_script::VmMemo<Option<bool>>>,
) {
    plates.0.clear();
    plate_hover.0 = None;
    bucket.clear();
    // Every early return retires the plates first, or the widgets stay on screen; the reference
    // hides each live plate and returns it to the pool (`0x608a10`).
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
    // Plates stop taking the mouse in freelook (`0x60f830`, called from `0x483e80`/`0x483e70` on
    // the transitions), written on the edge per VM.
    let looking = rig.is_looking();
    if *mouse_told.get(&script) != Some(looking) {
        *mouse_told.get(&script) = Some(looking);
        script.set_nameplate_mouse(!looking);
    }
    // While a ground-targeted spell is armed (`0x6e6320`'s `flag & 0x60`), plates refuse the hit
    // test (`0x7cba30`, the `+0x3c` override) without clearing the mouse-enabled bit, so
    // `IsMouseEnabled()` is unchanged.
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
    // Everything below is in window px, divided by this once for the widget layer's units.
    let seam = window.map_or(1.0, |w| {
        crate::ui_script::seam_scale(w.height(), ui_scale.0)
    });
    let mut states: Vec<PlateState> = Vec::new();

    // Plates seat in squared-distance order from the gx point (0.4, 0.3), Y mirrored from the
    // reference's Y-up space (`0x608870`, `0x6089a0`, walked by `0x608ce0`); each later plate is
    // solved off those already claimed. The undamped diagonal: this is position, not size.
    let diag = viewport.length();
    let sort_pt = Vec2::new(0.4 * diag, viewport.y - 0.3 * diag);
    let mut cands = Vec::new();
    for (entity, net, guid, tf, store) in &world.units {
        if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            continue;
        }
        // `NOT_SELECTABLE` (flags bit 25) never gets a plate, re-tested every tick (`0x60f622`,
        // from CGUnit's OnUpdate `0x607ed0`). A plate hover publishes the mouseover with no
        // selectability check (`0x7cb869`, unlike the world hover at `0x482982`), so this gate is
        // what keeps such a unit's tooltip away.
        if store.is_some_and(|s| s.0.unit_flags() & (1 << 25) != 0) {
            continue;
        }
        // `0x60f600`'s first test: health 0 or less (absent reads 0) has no plate. Feign death
        // keeps health, so it passes here.
        if store.is_none_or(|s| s.0.unit_health().unwrap_or(0) == 0) {
            continue;
        }
        // `0x60f62e`: `bytes_1` byte 3 `& 3` (ghost or creep) has no plate.
        if store.is_some_and(|s| s.0.unit_is_ghost_visual()) {
            continue;
        }
        if (tf.translation - self_tf.translation).length_squared() > MAX_DIST_SQ {
            continue;
        }
        // A CREATEDBY owner, no SUMMONEDBY owner and flags bit 9 (`0x200`): no plate in either
        // category (`0x60f70b`-`0x60f72d`); the bit's 1.12 name is unknown.
        if store.is_some_and(|s| {
            s.0.unit_created_by().is_some_and(|g| g != 0)
                && s.0.unit_summoned_by().is_none_or(|g| g == 0)
                && s.0.unit_flags() & 0x200 != 0
        }) {
            continue;
        }
        // The category is `CanAttack` from the player (a rep faction answers by its at-war bit);
        // the bar colour is the unit's reaction toward us (by standing). A not-at-war neutral NPC
        // is a friendly plate with a yellow bar.
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
        // Only on the hostile side, a dead-looking unit has no plate (`0x60f733`): a feigning
        // hunter's plate hides from enemies and stays for friends.
        if !friendly && store.is_some_and(|s| s.0.unit_reads_dead()) {
            continue;
        }
        // Behind the camera or outside the viewport there is no plate: `0x60f600` destroys it
        // before the seat, never clamping it in from off-screen.
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

        let alpha = if !has_target || world.selection.target == Some(entity) {
            1.0
        } else {
            DIM_ALPHA
        };
        let tint = plate_tint(rank, is_player);
        // `WOW_VPLATE_TRACE=1` prints each plate rect in logical px.
        static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let trace = *TRACE.get_or_init(|| std::env::var("WOW_VPLATE_TRACE").as_deref() == Ok("1"));
        // The seat (`0x509ec0`): the plate hangs from its top centre on the projected point, is
        // solved off the plates already claimed, then clamped half a plate inside the screen.
        let desired = Rect::new(
            screen.x - pw * 0.5,
            screen.y,
            screen.x + pw * 0.5,
            screen.y + ph,
        );
        let solved = bucket.resolve(desired, viewport);
        // Snap the left edge, not the centre, which an odd width would put between texels; the
        // claim takes the snapped rect, so later plates dodge what is drawn.
        let top = device_snap(solved.min.y, scale);
        let left = device_snap(solved.min.x, scale);
        let plate = Rect::new(left, top, left + pw, top + ph);
        // Trace tag `vpl`, one line per plate per frame: anchor, camera, projected point, solve
        // and snapped rect, so plate jitter can be attributed to one of them.
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
        let hover = hovered_key == Some(guid.0);
        // The mouseover or the target lights the bar; `hover` joins directly so the brighten does
        // not lag the yellow name by the frame the mouseover takes to publish.
        let lit =
            hover || world.hovered.target == Some(entity) || world.selection.target == Some(entity);
        if trace {
            eprintln!(
                "vplate-trace: viewport={viewport:?} screen=({:.2},{:.2}) plate=({:.1},{:.1})..({:.1},{:.1}) lit={lit}",
                screen.x, screen.y, plate.min.x, plate.min.y, plate.max.x, plate.max.y
            );
        }
        // ── The plate's state, handed to the widget layer ──
        //
        // The reference's plates sit outside the uiScale cascade, but benilla folds uiScale into
        // one global seam, so dividing by it here keeps plate pixels uiScale-blind.
        // The raid-target index is 1-based here, 0-based for the atlas cell.
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
            // A plate stays bound to its unit for its life, as the reference's `[unit+0xe60]`;
            // addons cache per plate on it.
            key: guid.0,
            top_centre: (x_units, y_units),
            // Raw health, not a fraction: `healthbar:GetValue()` is `[bar+0x320]` (`0x78f5d0`),
            // and addons divide it by the max.
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
            // The skull (`0x7cbb40`): rank 3 through `gated_rank` (so a charmed boss shows its
            // level), or a hostile 10 or more levels up that is not grey (`0x5f0700`, vacuous
            // there but transcribed). A unit with no level yet shows neither.
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
            // 0-based for the atlas: `col = idx & 3`, `row = idx >> 2`.
            raid_icon: (mark >= 1).then(|| mark - 1),
            alpha,
            lit,
            hovered: hover,
        });
    }

    // The widget layer rewrites the static anchors only when this moves, so an addon's own
    // re-anchoring survives between resizes.
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
            // Deviation: a constant one-logical-pixel shadow, not the reference's 0.001 gx, which
            // draws 2 px thick and too heavy at larger windows. Divided by the seam, which the
            // shared text arm would otherwise scale it by.
            shadow_offset: 1.0 / seam,
        },
        &states,
    );
}

/// The plate driver's set; the overhead-name driver runs after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct VPlateSet;

/// V-key nameplates: the toggles and the per-frame gate and draw.
pub(crate) struct VPlatesPlugin;

impl Plugin for VPlatesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<VPlateMode>()
            .init_resource::<VPlates>()
            .init_resource::<PlateHover>()
            .init_resource::<PlateClicks>()
            .add_systems(
                Update,
                (
                    // A drain of the verbs the tick ran (a binding body, `UpdateNameplates`).
                    apply_plate_verbs.after(crate::ui_script::UiInput),
                    // After the targeting chain, itself after `WorldStage::Input`, so this projects
                    // through this frame's camera; `ui_script::extract::paint_script` runs after
                    // it, so the plates paint the same frame.
                    drive_vplates.after(TargetUpdate),
                )
                    .chain()
                    .in_set(VPlateSet),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x7cbd50` at level 30 (grayband[6] = 7: green down to 23, gray at 22) and the low edge.
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
        // 0xFFFF1919, 0xFFFF7F3F, 0xFF3FB23F.
        assert_eq!(CON_RED[1], 25.0 / 255.0);
        assert_eq!(CON_ORANGE[2], 63.0 / 255.0);
        assert_eq!(CON_GREEN[1], 178.0 / 255.0);
    }

    #[test]
    fn the_plate_snaps_on_the_device_grid_not_the_logical_one() {
        // At 2x every snapped value is a whole number of physical pixels.
        for (v, want) in [(10.0, 10.0), (10.2, 10.0), (10.3, 10.5), (10.6, 10.5)] {
            let got = device_snap(v, 2.0);
            assert_eq!(got, want, "{v} at 2×");
            assert_eq!((got * 2.0).fract(), 0.0, "{got} is a whole physical pixel");
        }
        assert_eq!(device_snap(10.4, 1.0), 10.0);
        assert_eq!(device_snap(10.6, 1.0), 11.0);
        assert_eq!((device_snap(10.4, 1.5) * 1.5).fract(), 0.0);
        assert_eq!((device_snap(10.9, 1.5) * 1.5).fract(), 0.0);
    }

    /// Name em 13 at 1152x648 as measured in the reference; at 2560x1440 the reference's 29 and
    /// the native 13 damp to 21.
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

    /// An app with the drain and a bare VM: the four verbs are all it needs.
    fn verb_app() -> App {
        let mut app = App::new();
        app.add_systems(Update, apply_plate_verbs)
            .init_resource::<VPlateMode>()
            .insert_non_send_resource(benilla_ui::script::UiScript::new().unwrap());
        app
    }

    fn run(app: &mut App, lua: &str) {
        app.world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .run(lua)
            .unwrap();
        app.update();
    }

    fn mode(app: &App) -> (bool, bool) {
        let m = app.world().resource::<VPlateMode>();
        (m.enemies, m.friends)
    }

    fn global(app: &App, name: &str) -> Option<i64> {
        app.world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .lua()
            .globals()
            .get::<Option<i64>>(name)
            .unwrap()
    }

    /// Both bits start clear: a fresh 1.12 client draws no plates until something shows them.
    #[test]
    fn the_verbs_move_the_bits_in_call_order() {
        let mut app = verb_app();
        app.update();
        assert_eq!(mode(&app), (false, false), "boot: both clear");
        run(&mut app, "ShowNameplates()");
        assert_eq!(mode(&app), (true, false));
        run(&mut app, "ShowFriendNameplates() HideNameplates()");
        assert_eq!(mode(&app), (false, true), "each verb its own bit");
        run(
            &mut app,
            "ShowNameplates() HideNameplates() HideFriendNameplates() ShowFriendNameplates()",
        );
        assert_eq!(mode(&app), (false, true), "the last write per bit stands");
    }

    /// The entry clears the bits after the UI load (`0x401639` after `0x401602`), so the
    /// `VARIABLES_LOADED` replay does not survive it and `PLAYER_ENTERING_WORLD`'s decides.
    #[test]
    fn world_entry_clears_both_bits_and_the_loads_replay_with_them() {
        let mut app = verb_app();
        run(&mut app, "ShowNameplates() ShowFriendNameplates()");
        assert_eq!(mode(&app), (true, true));
        // The load's replay, not yet drained when the entry clears.
        app.world()
            .non_send_resource::<benilla_ui::script::UiScript>()
            .run("ShowNameplates()")
            .unwrap();
        clear_at_world_entry(app.world_mut());
        app.update();
        assert_eq!(mode(&app), (false, false), "both clear after the entry");
        run(&mut app, "ShowFriendNameplates()");
        assert_eq!(
            mode(&app),
            (false, true),
            "and the next verb moves them again"
        );
    }

    /// The V, Shift-V and Ctrl-V commands run the stock bodies (`Bindings.xml:516-550`): V and
    /// Shift-V each show their own kind and hide the other, and turn their own kind off only
    /// when it is the one showing; Ctrl-V shows both unless both show. Each flips the verb and
    /// FrameXML's global together, so the saved variable carries what the key did, and none
    /// writes a CVar.
    #[test]
    fn the_v_keys_run_the_stock_bodies() {
        benilla_formats::wow_data_or_skip!();
        let mut app = verb_app();
        crate::ui_script::load_stock_bindings(
            &mut app
                .world_mut()
                .non_send_resource_mut::<benilla_ui::script::UiScript>(),
        );
        let key = |app: &mut App, command: &str| {
            assert!(app
                .world()
                .non_send_resource::<benilla_ui::script::UiScript>()
                .execute_binding(command, true)
                .unwrap());
            app.update();
        };
        let state = |app: &App| {
            (
                mode(app),
                global(app, "NAMEPLATES_ON"),
                global(app, "FRIENDNAMEPLATES_ON"),
            )
        };

        key(&mut app, "NAMEPLATES");
        assert_eq!(state(&app), ((true, false), Some(1), None), "V: enemies on");
        key(&mut app, "FRIENDNAMEPLATES");
        assert_eq!(
            state(&app),
            ((false, true), None, Some(1)),
            "Shift-V: friends on, enemies off"
        );
        key(&mut app, "NAMEPLATES");
        assert_eq!(
            state(&app),
            ((true, false), Some(1), None),
            "V with friends showing: enemies on, friends off"
        );
        key(&mut app, "NAMEPLATES");
        assert_eq!(state(&app), ((false, false), None, None), "V again: off");
        key(&mut app, "ALLNAMEPLATES");
        assert_eq!(
            state(&app),
            ((true, true), Some(1), Some(1)),
            "Ctrl-V: both on"
        );
        key(&mut app, "NAMEPLATES");
        assert_eq!(
            state(&app),
            ((true, false), Some(1), None),
            "V with both on turns friends off"
        );
        key(&mut app, "ALLNAMEPLATES");
        key(&mut app, "FRIENDNAMEPLATES");
        assert_eq!(
            state(&app),
            ((false, true), None, Some(1)),
            "Shift-V with both on turns enemies off"
        );
        key(&mut app, "ALLNAMEPLATES");
        key(&mut app, "ALLNAMEPLATES");
        assert_eq!(
            state(&app),
            ((false, false), None, None),
            "Ctrl-V with both on: both off"
        );
        assert!(
            app.world_mut()
                .non_send_resource_mut::<benilla_ui::script::UiScript>()
                .take_cvar_changes()
                .is_empty(),
            "no CVar: nothing reaches config.toml"
        );
    }

    /// A player's plate setting from `config.toml` is carried into the saved-variables globals
    /// once, over what the file said, and leaves `config.toml`.
    #[test]
    fn a_legacy_config_setting_carries_over_once() {
        use crate::cvars::Cvars;
        let mut cvars = Cvars::with_value("nameplateShowFriends", "1");
        let legacy = take_legacy_settings(&mut cvars);
        assert_eq!(legacy.0, vec![("FRIENDNAMEPLATES_ON", true)]);
        assert!(
            take_legacy_settings(&mut cvars).0.is_empty(),
            "taken once: the file no longer holds it"
        );
        assert!(
            !cvars
                .orphans()
                .iter()
                .any(|(k, _)| k == "nameplateShowFriends"),
            "and the next save drops it"
        );

        let s = benilla_ui::script::UiScript::new().unwrap();
        // The saved-variables chunk said off; the CVar was the store, so it wins.
        s.run("FRIENDNAMEPLATES_ON = nil NAMEPLATES_ON = 1")
            .unwrap();
        seat_legacy_settings(&s, &legacy);
        assert_eq!(
            s.eval::<Option<i64>>("return FRIENDNAMEPLATES_ON").unwrap(),
            Some(1),
            "the number 1, never a truthy `0`"
        );
        assert_eq!(
            s.eval::<Option<i64>>("return NAMEPLATES_ON").unwrap(),
            Some(1),
            "a global with no legacy entry is left as the file set it"
        );

        let mut off = Cvars::with_value("NamePlateShowEnemies", "0");
        let legacy = take_legacy_settings(&mut off);
        s.run("NAMEPLATES_ON = 1").unwrap();
        seat_legacy_settings(&s, &legacy);
        assert_eq!(
            s.eval::<Option<i64>>("return NAMEPLATES_ON").unwrap(),
            None,
            "an entry of 0 is off, whatever its spelling"
        );
    }

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

    /// At 1024x768 (diagonal 1280) the frame is the border's native 128x32 px; at 1080p the
    /// reference's 220 and the native 128 damp to 174.
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
