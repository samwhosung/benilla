//! The ground-targeting AoE reticle: the terrain-projected decal under a location cast's cursor.
//! Only a targeting word passing `TargetingWantsLocation`'s `& 0x60` draws one; a bag click or a
//! world GameObject click arms the same cursor and draws none.
//!
//! The reference projects a decal for an area spell: the picked ground point ± r horizontally and
//! ± 2.0 vertically (`0x4837b0`), axis-aligned, through the selection ring's ground-decal projector
//! (`0x6d7330 → 0x6d6fa0 → 0x6d7480`, [`benilla_world::decal`]). The texture carries the colour,
//! `Spell-Shadow-Acceptable.blp` in range and `Spell-Shadow-Unacceptable.blp` out of it, under a
//! white vertex colour. The reference also rotates a GameObject preview model for an
//! object-placement spell (effect `0x51`); that model is not built.
//!
//! The radius is `ground_cast_radius` (`GetCurrentCastRadius 0x6e6350`, clamped to 20.0 in
//! `0x4820f0`), with no spell mod applied. Out of range forces it to 0.0, and 0.0 draws at the
//! 1.3888889 default. With no world hit nothing is drawn: the
//! reference resets its draw state every hover pass. Over a unit the decal lands on the ground
//! behind it, since a dest-only word's pick skips the object trace (`0x480e7b`).
//!
//! The reference's second pass (`0x483727`) also projects onto liquid (flags `0x0f0000`); the
//! decal surfaces have no liquid yet, so the reticle vanishes over water.

use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use crate::spell::{ground_cast_radius, SpellTargeting, TargetingWants};
use crate::target::{PickOcclusion, WorldCursor};
use crate::ui_action::Spells;
use benilla_world::decal::{DecalFrame, WorldDecal};
use benilla_world::particles::buffer::EffectVertex;
use benilla_world::view::WorldCamera;

/// The footprint for a zero radius, out of range or rowless (the reference's literal for
/// `[0xb4b3b0] == 0.0`).
const RETICLE_DEFAULT_RADIUS: f32 = 1.388_889;
/// The box's vertical half-height (`0x4837b0`), fixed, not radius-scaled like the ring's.
const RETICLE_VERT: f32 = 2.0;

/// The two state textures; the draw waits until they are loaded.
#[derive(Resource)]
pub(super) struct ReticleAssets {
    acceptable: Handle<Image>,
    unacceptable: Handle<Image>,
}

/// The reticle: its projection, cached against a key and pushed onto the effect stream each frame.
#[derive(Resource, Default)]
pub(super) struct ReticleState {
    verts: Vec<EffectVertex>,
    key: ReticleKey,
    acceptable: bool,
    shown: bool,
}

/// The projection's rebuild inputs: a still cursor costs one compare.
#[derive(Default, PartialEq, Clone, Copy)]
struct ReticleKey {
    center: Vec3,
    radius: f32,
    surfaces: usize,
}

pub(super) fn setup_reticle(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(ReticleAssets {
        acceptable: asset_server
            .load::<Image>("mpq://interface/spellshadow/spell-shadow-acceptable.blp"),
        unacceptable: asset_server
            .load::<Image>("mpq://interface/spellshadow/spell-shadow-unacceptable.blp"),
    });
    commands.init_resource::<ReticleState>();
}

/// Places, sizes and colours the reticle while a location cast awaits its click. Runs after the
/// targeting cursor drive: `WorldCursor.unable` is the frame's range verdict, one read for the
/// cursor and the decal, as in the reference.
///
/// Both of `0x4820f0`'s guards, in order: `IsTargeting 0x6e48a0`, then `TargetingWantsLocation
/// 0x6e6320`. Either false returns before the draw state, which the hover handler reset to 3, do
/// not draw (`0x481840`), so a lock or enchant word (`0x4000`, `0x0010`) draws no decal. The
/// reference's pick is word-gated (`0x481050`) and ours is not, so this consumer carries the guard,
/// through [`SpellTargeting::spell_for`].
pub(super) fn update_reticle(
    targeting: Res<SpellTargeting>,
    occlusion: Res<PickOcclusion>,
    cursor: Res<WorldCursor>,
    spells: Option<Res<Spells>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    decals: WorldDecal,
    mut state: ResMut<ReticleState>,
) {
    let state = &mut *state;
    let (Some(spell_id), Some(point)) = (
        targeting.spell_for(TargetingWants::Location),
        occlusion.point,
    ) else {
        // Not targeting, a word that wants no location, or no world hit: nothing is drawn.
        state.shown = false;
        return;
    };
    let acceptable = !cursor.unable;
    // Out of range forces radius 0.0, which draws at the default.
    let radius = if acceptable {
        let level = self_store
            .single()
            .ok()
            .and_then(|s| s.0.unit_level())
            .unwrap_or(1);
        ground_cast_radius(spells.as_deref(), spell_id, level)
    } else {
        0.0
    };
    let radius = if radius > 0.0 {
        radius
    } else {
        RETICLE_DEFAULT_RADIUS
    };
    let key = ReticleKey {
        center: point,
        radius,
        surfaces: decals.receiver_count(),
    };
    if !(state.shown && key == state.key && state.acceptable == acceptable) {
        state.verts.clear();
        state.key = key;
        state.acceptable = acceptable;
        // Box to [0,1]² UVs, the reference's 0.5 UV bias. The vertical fade is the ring's, so a
        // draped wall piece dims rather than smears.
        let frame = DecalFrame {
            center: point,
            sin: 0.0,
            cos: 1.0,
            min_x: -radius,
            max_x: radius,
            min_z: -radius,
            max_z: radius,
            min_y: -RETICLE_VERT,
            max_y: RETICLE_VERT,
        };
        decals.project(
            &mut state.verts,
            &frame,
            |p| ((RETICLE_VERT - p.y.abs()) / RETICLE_VERT).clamp(0.0, 1.0),
            |x, z| frame.rect_uv(x, z),
        );
    }
    state.shown = !state.verts.is_empty();
}

/// Pushes the shown reticle onto the effect stream: vertex colour `0xffffffff`, alpha blend (the
/// reference's blend mode 2), no fog.
pub(super) fn push_reticle(
    assets: Option<Res<ReticleAssets>>,
    state: Option<Res<ReticleState>>,
    cam: Query<Entity, With<WorldCamera>>,
    mut draw: benilla_world::particles::buffer::WorldEffectDraw,
) {
    let (Some(assets), Some(state)) = (assets, state) else {
        return;
    };
    let Ok(cam) = cam.single() else { return };
    if !state.shown || state.verts.is_empty() {
        return;
    }
    let texture = if state.acceptable {
        assets.acceptable.id()
    } else {
        assets.unacceptable.id()
    };
    let mut batch = draw
        .batch(cam, texture)
        .anchored(state.key.center)
        // The pre-water decal band, the reference's pass at `0x4836c5` (flags `0x200122`).
        .rung(
            benilla_world::sky_order::Rung::RETICLE,
            benilla_world::sky_order::Rung::DECAL_RASTER,
        );
    batch.extend(state.verts.iter().map(|v| EffectVertex {
        pos: v.pos,
        uv: v.uv,
        color: [1.0, 1.0, 1.0, v.color[3]],
    }));
    batch.tris();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::CastCommit;
    use avian3d::prelude::Collider;
    use benilla_world::collision::GroundDecalSurface;
    use bevy::ecs::system::RunSystemOnce;

    /// `0x4820f0`'s second guard. The ground is a real trimesh, so a DEST word draws and "nothing
    /// drawn" is a verdict, not an empty scene.
    #[test]
    fn only_a_location_word_draws_a_decal() {
        let drawn_for = |word: u16| {
            let mut world = World::new();
            world.init_resource::<ReticleState>();
            world.init_resource::<WorldCursor>();
            world.init_resource::<SpellTargeting>();
            world.insert_resource(PickOcclusion {
                distance: 10.0,
                point: Some(Vec3::ZERO),
            });
            world
                .resource_mut::<SpellTargeting>()
                .enter(2120, CastCommit::Spell, word);
            // A flat 100×100 yd quad at y = 0 in world space (the marked colliders' identity-pose
            // contract), inside the ±2.0 slab and wider than any radius.
            let q = 50.0;
            world.spawn((
                GroundDecalSurface,
                Collider::trimesh(
                    vec![
                        Vec3::new(-q, 0.0, -q),
                        Vec3::new(q, 0.0, -q),
                        Vec3::new(q, 0.0, q),
                        Vec3::new(-q, 0.0, q),
                    ],
                    vec![[0, 1, 2], [0, 2, 3]],
                ),
            ));
            world.run_system_once(update_reticle).unwrap();
            world.resource::<ReticleState>().shown
        };

        // Blizzard's bare DEST word.
        assert!(drawn_for(0x0040), "a DEST word draws its reticle");
        // SOURCE|DEST is still `& 0x60`.
        assert!(drawn_for(0x0060), "a SOURCE|DEST word draws its reticle");
        // Lock, Opening (lock and GameObject), item and GameObject words.
        for word in [0x4000, 0x4800, 0x0010, 0x0800] {
            assert!(
                !drawn_for(word),
                "word {word:#06x} wants no location — no decal"
            );
        }
    }
}
