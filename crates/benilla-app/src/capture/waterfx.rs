//! The `waterfx` capture viewer for water foam: a server-less rig of one wading dummy over a
//! synthetic 4.1667-yd wet-cell lattice with a flat backdrop, driven through the shipped emitter.
//!
//! `WOW_CAPTURE=waterfx` with knobs: `WOW_WFX_MODE` (`ring`|`wake`|`turn`), `WOW_WFX_SPEED` (yd/s),
//! `WOW_WFX_HEAD` (wake heading, WoW degrees, 0 = +X), `WOW_WFX_AGE` (seconds of motion before the
//! shot), `WOW_WFX_DEPTH` (yd below the surface; past ~0.8 also fires the step-in one-shot), and
//! camera `WOW_WFX_AZ`/`EL`/`DIST`. Not a golden scenario.
//!
//! `WOW_WFX_AT=x,y,z` wades the dummy in the real streamed liquid instead (`z` the surface height,
//! which `benilla-formats --example water_here` prints; `WOW_MAP` picks the map), the only rig that
//! shows bank clipping and sorting against neighbouring water chunks.

use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_protocol::EntityKind;

use super::FxViewState;
use crate::net::NetEntity;
use benilla_world::liquid::{FoamPatch, WaterChunkInfo};

/// Which foam behaviour the viewer exercises.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum WfxMode {
    /// Standing unit: the pulsing ring.
    Ring,
    /// Unit translating along the heading: the trailing wake, ending at the rig centre.
    Wake,
    /// Unit turning in place: full-size rings (the `& 0x30` state).
    Turn,
}

/// The `waterfx` viewer request, built by [`crate::capture`] from the knobs; its presence turns on
/// the rig.
#[derive(Resource)]
pub(crate) struct WaterFxView {
    pub(crate) mode: WfxMode,
    /// Translation speed for [`WfxMode::Wake`] (yd/s, along [`Self::heading`]).
    pub(crate) speed: f32,
    /// Which way a [`WfxMode::Wake`] walks, as a WoW yaw in radians (0 = +X), to lay it along a
    /// bank.
    pub(crate) heading: f32,
    /// Seconds the unit moves or stands before the shot.
    pub(crate) age: f32,
    /// Rig centre in WoW coords `(x, y, surface_z)`, where the unit is at shot time.
    pub(crate) center: [f32; 3],
    /// Feet depth below the surface (yd); must land inside the wading gate.
    pub(crate) depth: f32,
    /// Wade in the real streamed liquid at [`Self::center`] (`WOW_WFX_AT`): no backdrop, no
    /// fixture water.
    pub(crate) live: bool,
}

/// Marks the rig's entities (spawn-once guard).
#[derive(Component)]
pub(crate) struct WaterFxDummy;

/// The MCLQ wet-cell edge (yd).
const CELL: f32 = 33.333_332 / 8.0;

/// Once the scene is armed, stands up the backdrop, a 12x12-cell lattice (~50 yd square) and the
/// dummy, and sets [`FxViewState::attached_at`] as the age clock's zero.
pub(crate) fn spawn(
    mut commands: Commands,
    view: Option<Res<WaterFxView>>,
    state: Option<ResMut<FxViewState>>,
    existing: Query<(), With<WaterFxDummy>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    time: Res<Time>,
) {
    let (Some(view), Some(mut state)) = (view, state) else {
        return;
    };
    if !state.armed || !existing.is_empty() {
        return;
    }
    let [cx, cy, surf] = view.center;

    if view.live {
        spawn_dummy(&mut commands, &view, cx, cy, surf);
        state.attached_at = Some(time.elapsed_secs());
        info!("waterfx: rig armed in LIVE water at ({cx}, {cy}, {surf})");
        return;
    }

    // A flat backdrop 0.15 yd under the surface, so the additive foam reads against a stable tone.
    commands.spawn((
        WaterFxDummy,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(120.0, 120.0).build())),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.16, 0.22, 0.26),
            unlit: true,
            ..default()
        })),
        Transform::from_translation(wow_to_bevy([cx, cy, surf - 0.15])),
    ));

    // The components the terrain streamer attaches to real liquid, so the shipped path runs.
    let n = 12;
    let half = n as f32 * CELL * 0.5;
    let (x0, y0) = (cx - half, cy - half);
    let mut positions = Vec::new();
    for iy in 0..=n {
        for ix in 0..=n {
            positions.push([x0 + ix as f32 * CELL, y0 + iy as f32 * CELL, surf]);
        }
    }
    commands.spawn((
        WaterFxDummy,
        WaterChunkInfo::new(
            // An outdoor lake: ADT still water, which `liquid_at` answers outdoors.
            benilla_world::liquid::LiquidSource::AdtChunk,
            benilla_formats::LiquidKind::Still,
            [n + 1, n + 1],
            positions,
            vec![true; n * n],
        ),
        FoamPatch,
        Transform::IDENTITY,
    ));

    spawn_dummy(&mut commands, &view, cx, cy, surf);
    state.attached_at = Some(time.elapsed_secs());
    info!(
        "waterfx: rig armed (mode {}, speed {}, depth {}, age {})",
        match view.mode {
            WfxMode::Ring => "ring",
            WfxMode::Wake => "wake",
            WfxMode::Turn => "turn",
        },
        view.speed,
        view.depth,
        view.age
    );
}

/// The wading dummy: a streamed unit with no display and its visual pre-attached, so it gets no
/// fallback cube. A wake starts far enough back to end at the rig centre after `age` seconds.
fn spawn_dummy(commands: &mut Commands, view: &WaterFxView, cx: f32, cy: f32, surf: f32) {
    let (start_x, start_y) = if view.mode == WfxMode::Wake {
        let back = view.speed * view.age;
        (
            cx - back * view.heading.cos(),
            cy - back * view.heading.sin(),
        )
    } else {
        (cx, cy)
    };
    commands.spawn((
        WaterFxDummy,
        NetEntity {
            kind: EntityKind::Unit,
            display_id: None,
            scale: 1.0,
        },
        // Set here, not by `entities::publish_world_units`, which runs before this
        // `WorldStage::Present` spawn and would cost the rig its first frame of foam.
        benilla_world::world_unit::WorldUnit {
            wades: true,
            scale: 1.0,
            height: crate::entities::CollisionHeight::default().0,
            // No model box; outdoors the exterior cull stands down anyway.
            bound: None,
        },
        crate::entities::VisualAttached,
        Transform::from_translation(wow_to_bevy([start_x, start_y, surf - view.depth])),
    ));
}

/// Moves the dummy (wake) or spins it (turn) until the age elapses; the emitter reads the motion
/// through its normal velocity and yaw proxies.
pub(crate) fn drive(
    view: Option<Res<WaterFxView>>,
    state: Option<Res<FxViewState>>,
    time: Res<Time>,
    mut units: Query<&mut Transform, (With<WaterFxDummy>, With<NetEntity>)>,
) {
    let (Some(view), Some(state)) = (view, state) else {
        return;
    };
    let Some(t0) = state.attached_at else {
        return;
    };
    if time.elapsed_secs() - t0 >= view.age {
        return; // hold still for the shot
    }
    for mut t in &mut units {
        match view.mode {
            WfxMode::Wake => {
                // bevy = (-wow.y, wow.z, -wow.x), so WoW heading (cos h, sin h) is Bevy
                // (-sin h, 0, -cos h).
                let step = view.speed * time.delta_secs();
                t.translation.x -= step * view.heading.sin();
                t.translation.z -= step * view.heading.cos();
            }
            WfxMode::Turn => {
                t.rotate_y(2.0 * time.delta_secs());
            }
            WfxMode::Ring => {}
        }
    }
}
