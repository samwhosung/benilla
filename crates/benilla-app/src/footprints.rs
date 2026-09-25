//! Footprint decals on snow and sand, through the shared decal projector
//! ([`benilla_world::decal`]). A print is projected once when its foot plants and its triangles
//! replay with the current alpha until it fades, as the reference's per-frame draw `0x69a3e0` does.
//!
//! - Surface: `TerrainType.Flags & 1` (`0x699eb6`, Snow and Sand) on the unit's one resolved
//!   surface (`CGUnit+0xc60`), which the decal, the spray and `$FSD` share (`0x5fc06e`,
//!   `0x5fc20f`, `0x62341d`); indoors it is the building's floor.
//! - Trigger: the per-foot tags `$xL*`/`$xR*` of [`AnimSoundEvent`], placed at the event's
//!   authored offset through the live bone (`0x7196df`); `$FSD` is sound-only.
//! - Ink and size: `CreatureModelData.FootprintTextureID` to `FootprintTextures.dbc`, Length and
//!   Width in inches (1/36 yd, `0x5fc310`, `0x607a00`) times the wire `OBJECT_FIELD_SCALE_X` alone
//!   (`0x469f10`); `0xFFFFFFFF` is printless. A mount prints its own ink at the rider's scale. The
//!   texture is a left foot; right prints mirror (`0x5fc07f`).
//! - Caps: ring pools of 64 for the local player and 512 for everyone else (`0xca05f0`).
//! - Suppressed by hover, stealth (not death), a player ghost and more than 50 yd from the camera
//!   eye ([`footfall_culls`], the local player's feet included); water does not suppress it.
//! - The reference's `showfootprints` cvar (default on, `0x5fc023`) skips the decal block alone,
//!   so the footstep shake and the spray still run; it is not wired here, so prints are always on.
//!
//! Draw state, as the reference's: src-alpha blend, no depth write, unlit, white vertex RGB with
//! the fade in alpha; the ink's darkness is the texture's.
//! Untraced: the forward-axis sign, the uv1 64x8 edge-fade ramp (not modelled) and the order
//! against the blob shadow (our rung sits below it).

use std::collections::VecDeque;

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::creature_anim::{
    footfall_culls, footfall_side, move_flags, AnimSoundEvent, MovementState,
};
use crate::entities::Creatures;
use crate::net::{Embodied, NetEntity, ObjectStore};
use crate::sound::footsteps::Footsteps;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::decal::{DecalFrame, WorldDecal};
use benilla_world::particles::buffer::{begin_effect_frame, EffectVertex};
use benilla_world::schedule::WorldStage;
use benilla_world::view::WorldCamera;

/// 6000 ms (`0x69a3e0`).
const LIFETIME: f32 = 6.0;
/// The local player's ring: the head of the reference's 576-slot table at `0xca05f0`.
const OWN_CAP: usize = 64;
/// Everyone else's ring, the table's tail.
const SHARED_CAP: usize = 512;
/// Half-height of the projection slab about the foot: reaches the ground under a lifted foot
/// bone without painting a terrace below.
const SLAB_HALF_HEIGHT: f32 = 1.0;

/// One live print; `verts` carry alpha 1.0 and the fade multiplies in at push time.
struct Print {
    verts: Vec<EffectVertex>,
    spawned: f32,
    texture: AssetId<Image>,
    /// The draw's sort anchor.
    anchor: Vec3,
}

/// The two ring pools, oldest first; with one lifetime, spawn order is expiry order.
#[derive(Resource, Default)]
struct Footprints {
    own: VecDeque<Print>,
    shared: VecDeque<Print>,
}

/// The entity every print draw names as its owner, since prints have no entity.
#[derive(Resource)]
struct FootprintLane(Entity);

/// `FootprintTextures.dbc` id to its ink texture.
#[derive(Resource, Default)]
struct FootprintInk(HashMap<u32, Handle<Image>>);

pub(crate) struct FootprintsPlugin;

impl Plugin for FootprintsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Footprints>()
            .add_systems(Startup, load_ink.after(AssetSet::Open))
            // Present: after animation and transforms settle the foot bones.
            .add_systems(Update, spawn_footprints.in_set(WorldStage::Present))
            .add_systems(PostUpdate, push_footprints.after(begin_effect_frame));
    }
}

fn load_ink(
    mut commands: Commands,
    assets: Option<Res<WorldAssets>>,
    asset_server: Res<AssetServer>,
) {
    let lane = commands.spawn(Name::new("footprint-lane")).id();
    commands.insert_resource(FootprintLane(lane));
    let Some(assets) = assets else { return };
    let table = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_footprint_textures(&mut chain)
    };
    match table {
        Ok(table) => {
            let ink = table
                .into_iter()
                .map(|(id, path)| {
                    // DBC paths are extensionless, with backslashes.
                    let url = format!("mpq://{}.blp", path.replace('\\', "/"));
                    (id, asset_server.load::<Image>(url))
                })
                .collect();
            commands.insert_resource(FootprintInk(ink));
        }
        Err(e) => warn!("footprints: FootprintTextures.dbc failed to load: {e:#}"),
    }
}

/// The root unit's reads (the rider, for a mount).
type RootState = (
    Has<Embodied>,
    Option<&'static NetEntity>,
    Option<&'static ObjectStore>,
    Option<&'static MovementState>,
);

/// Spawns and caches a print for each gated per-foot plant.
fn spawn_footprints(
    mut events: MessageReader<AnimSoundEvent>,
    time: Res<Time>,
    // Global: a mount's tags come from a child whose local Transform is seat-relative.
    units: Query<(&NetEntity, &GlobalTransform)>,
    parents: Query<&ChildOf>,
    roots: Query<RootState>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    footsteps: Option<Res<Footsteps>>,
    creatures: Option<Res<Creatures>>,
    ink: Option<Res<FootprintInk>>,
    world: benilla_world::world_point::WorldPoint,
    decals: WorldDecal,
    mut prints: ResMut<Footprints>,
) {
    if events.is_empty() {
        return;
    }
    let (Some(footsteps), Some(creatures), Some(ink)) = (footsteps, creatures, ink) else {
        return;
    };
    let Ok(eye) = camera.single().map(|t| t.translation()) else {
        return;
    };
    let now = time.elapsed_secs();
    for ev in events.read() {
        let Some(side) = footfall_side(&ev.ident) else {
            continue;
        };
        let Ok((net, transform)) = units.get(ev.entity) else {
            continue;
        };
        // Ink and size from the event's model, scale and gates from the root unit (`0x607920`).
        let Some(params) = net.display_id.and_then(|d| creatures.footprint(d)) else {
            continue;
        };
        let Some(texture) = ink.0.get(&params.texture_id) else {
            continue;
        };
        let mut root = ev.entity;
        while let Ok(child_of) = parents.get(root) {
            root = child_of.parent();
        }
        let (is_self, root_net, store, movement) = match roots.get(root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if movement.is_some_and(|m| m.flags & move_flags::HOVER != 0) {
            continue;
        }
        if let Some(store) = store {
            if store.0.unit_is_stealthed() || store.0.player_is_ghost() {
                continue;
            }
        }
        // The fired key's own point, not a first-match marker lookup (`0x7130e0`), which a model
        // authoring a side tag twice would resolve to the wrong foot.
        let foot = ev.pos.unwrap_or_else(|| transform.translation());
        if footfall_culls(eye, foot) {
            continue;
        }
        // The unit's surface, not a per-foot sample (`CGUnit+0xc60`, module doc).
        let terrain = world.terrain_type(
            &footsteps.0,
            benilla_world::world_point::Subject::Unit(ev.entity),
            transform.translation(),
        );
        if !terrain.is_some_and(|t| footsteps.0.terrain_leaves_footprints(t)) {
            continue;
        }
        // The facing maps to the frame's -z axis, so v = 0 is the toe; the reference's sign is
        // untraced, and a backwards print flips one sign here.
        let yaw = transform
            .to_scale_rotation_translation()
            .1
            .to_euler(EulerRot::YXZ)
            .0;
        let scale = root_net.unwrap_or(net).scale.max(0.0);
        let (half_len, half_wid) = (params.length * scale * 0.5, params.width * scale * 0.5);
        if half_len <= 0.0 || half_wid <= 0.0 {
            continue;
        }
        let frame = DecalFrame {
            center: foot,
            sin: yaw.sin(),
            cos: yaw.cos(),
            min_x: -half_wid,
            max_x: half_wid,
            min_z: -half_len,
            max_z: half_len,
            min_y: -SLAB_HALF_HEIGHT,
            max_y: SLAB_HALF_HEIGHT,
        };
        let mirror = side == b'R';
        let mut verts = Vec::new();
        let projected = decals.project(
            &mut verts,
            &frame,
            |_| 1.0,
            |x, z| {
                let [u, v] = frame.rect_uv(x, z);
                [if mirror { 1.0 - u } else { u }, v]
            },
        );
        if !projected {
            continue; // no ground under the foot
        }
        let n_verts = verts.len();
        let (pool, cap) = if is_self {
            (&mut prints.own, OWN_CAP)
        } else {
            (&mut prints.shared, SHARED_CAP)
        };
        if pool.len() >= cap {
            pool.pop_front(); // unconditional ring rotation
        }
        pool.push_back(Print {
            verts,
            spawned: now,
            texture: texture.id(),
            anchor: foot,
        });
        debug!(
            "footprint: {} ink {} at ({:.2}, {:.2}, {:.2}), {} verts ({} own + {} shared live)",
            ev.ident.map(char::from).iter().collect::<String>(),
            params.texture_id,
            foot.x,
            foot.y,
            foot.z,
            n_verts,
            prints.own.len(),
            prints.shared.len(),
        );
    }
}

/// The reference's fade (`0x69a3e0`): `t = 1 - age/6 s`, alpha byte `min(127, floor(255 t))`,
/// so a 3 s hold at half opacity, then linear to 0, with no fade-in.
fn fade(age: f32) -> f32 {
    let t = 1.0 - age / LIFETIME;
    if t <= 0.0 {
        return 0.0;
    }
    (255.0 * t).floor().min(127.0) / 255.0
}

/// Retires expired prints and replays the live ones with the fade in vertex alpha.
fn push_footprints(
    time: Res<Time>,
    cam: Query<Entity, With<WorldCamera>>,
    lane: Option<Res<FootprintLane>>,
    mut draw: benilla_world::particles::buffer::WorldEffectDraw,
    mut prints: ResMut<Footprints>,
) {
    let now = time.elapsed_secs();
    let prints = &mut *prints;
    for pool in [&mut prints.own, &mut prints.shared] {
        while pool.front().is_some_and(|p| now - p.spawned >= LIFETIME) {
            pool.pop_front();
        }
    }
    if prints.own.is_empty() && prints.shared.is_empty() {
        return;
    }
    let Ok(cam) = cam.single() else { return };
    let Some(lane) = lane else { return };
    for print in prints.own.iter().chain(prints.shared.iter()) {
        let alpha = fade(now - print.spawned);
        let mut batch = draw
            .batch(cam, print.texture)
            .anchored(print.anchor)
            .rung(
                benilla_world::sky_order::Rung::FOOTPRINT,
                benilla_world::sky_order::Rung::DECAL_RASTER,
            )
            .owner(lane.0);
        batch.extend(print.verts.iter().map(|v| EffectVertex {
            color: [v.color[0], v.color[1], v.color[2], v.color[3] * alpha],
            ..*v
        }));
        batch.tris();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_matches_the_reference_ramp() {
        assert_eq!(fade(0.0), 127.0 / 255.0);
        assert_eq!(fade(1.0), 127.0 / 255.0);
        assert_eq!(fade(3.0), 127.0 / 255.0);
        // t = 0.25, floor(63.75) = 63.
        assert_eq!(fade(4.5), 63.0 / 255.0);
        // t = 0.002 reaches 0 before death.
        assert_eq!(fade(5.988), 0.0);
        assert_eq!(fade(6.0), 0.0);
        assert_eq!(fade(7.0), 0.0);
    }
}
