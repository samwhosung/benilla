//! The **ground-targeting AoE reticle** — the terrain-projected decal a **location** cast's
//! cursor drags across the world. Only a word that passes
//! `TargetingWantsLocation`'s `& 0x60` has one: the other two seams (a bag click, a world
//! GameObject click) arm the same cursor and draw no decal at all — see [`update_reticle`]'s guard.
//!
//! For every ordinary area spell (Blizzard, Flamestrike — no object-placement effect), the
//! reference draws a **projected decal**, not a model: box = the picked ground point ± r in the
//! horizontal plane, **± 2.0 vertically** (`0x4837b0`); textures
//! `Interface\SpellShadow\Spell-Shadow-Acceptable.blp` (in range) /
//! `…-Unacceptable.blp` (out), vertex colour `0xffffffff` — the *texture* carries the
//! green/red, not a tint. It projects through **the same ground-decal projector as the
//! selection ring** (`0x6d7330 → 0x6d6fa0 → 0x6d7480` — [`benilla_world::decal`]), axis-aligned (no
//! rotation term; the effect-0x51 placement rotate orients a GameObject *preview model*, a
//! machine we don't carry — that family of spells is a named residual).
//!
//! **Radius**: `r = min(GetCurrentCastRadius, 20.0)` (`0x6e6350`, clamp `0x4820f0`):
//! per-effect `radius + casterLevel × perLevel` over **EffectRadiusIndex[0] and [1] only**
//! (slot 2 is never read), max with candidate-1 winning ties/NaN. **Out of range forces the
//! radius to 0.0** — the decal shrinks to the 1.3888889 default *and* turns red. `r == 0` (no
//! radius rows — a dest spell with no area) also draws at the default. Spell-mod op 6
//! (SPELLMOD_RADIUS) is not folded in: the tables are live (`crate::spell::mods`), this consumer
//! is not wired to them (the same residual as the range gate).
//!
//! **States**: in range → Acceptable at `r`; out of range → Unacceptable at the default size;
//! cursor over sky / no world hit → **nothing is drawn** (the ref resets its draw state every
//! hover pass before the pick — the "frozen at the last point" reading was refuted at the
//! bytes). Over a unit the decal draws on the ground behind it (the pick can't see units while
//! a dest-only word is up — its flags skip the object trace at `0x480e7b`).
//!
//! Named gap (shared with the blob shadow): the ref's second projection pass (`0x483727`) takes
//! **liquid** surfaces (flags `0x0f0000`) — our [`GroundDecalSurface`] set has no liquid yet, so
//! the reticle vanishes over water instead of floating on it.

use bevy::prelude::*;

use crate::net::{ObjectStore, SelfPlayer};
use crate::spell::{ground_cast_radius, SpellTargeting, TargetingWants};
use crate::target::{PickOcclusion, WorldCursor};
use crate::ui_action::Spells;
use benilla_world::decal::{DecalFrame, WorldDecal};
use benilla_world::particles::buffer::EffectVertex;
use benilla_world::view::WorldCamera;

/// The default footprint when the radius is 0 — out of range, or a rowless spell
/// (`[0xb4b3b0] == 0.0 → 1.3888889`, the ref's literal).
const RETICLE_DEFAULT_RADIUS: f32 = 1.388_889;
/// The projection box's vertical slab: ± 2.0 around the picked point (`0x4837b0` — fixed, not
/// radius-scaled like the ring's).
const RETICLE_VERT: f32 = 2.0;

/// The two state textures (the render-side residency gate withholds the draw until loaded).
#[derive(Resource)]
pub(super) struct ReticleAssets {
    acceptable: Handle<Image>,
    unacceptable: Handle<Image>,
}

/// The single reticle record — the ring's resource pattern (projection cached against a key,
/// pushed onto the effect stream per frame).
#[derive(Resource, Default)]
pub(super) struct ReticleState {
    verts: Vec<EffectVertex>,
    key: ReticleKey,
    acceptable: bool,
    shown: bool,
}

/// The projection's rebuild inputs — a still cursor costs a compare.
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

/// Place/size/state the reticle each frame while a **location** cast awaits its click. Runs
/// after the targeting cursor drive in the target chain: `WorldCursor.unable` IS the frame's
/// range verdict (the one `CheckGroundPointInRange` caller — the cursor and the decal state are
/// the same read in the ref).
///
/// **Both of `0x4820f0`'s guards, in its order**: `4820f9 IsTargeting 0x6e48a0`
/// **and** `482106 TargetingWantsLocation 0x6e6320` — either false and it returns having never
/// touched the draw state, which the hover handler already reset to `3` = *do not draw*
/// (`481840`, every pass). The *word's* mask decides, not the mere fact of targeting: a lock or
/// an enchant word (`0x4000` / `0x0010`) fails `& 0x60`, so no decal is drawn for one. The
/// reference does not even reach here for such a word — `0x481050`'s targeting arm builds the
/// pick flags from the word alone, and one without `0x60` gets no bit `0x1`, so the pick reports
/// state 2 (object → `0x4828d0`) or 0, never state 1. Our picks are not word-gated, so this
/// consumer carries the guard; asking [`SpellTargeting::spell_for`] rather than `spell()` is what
/// keeps the next seam from forgetting it.
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
        // Not targeting, targeting a word that wants no location, or the pick hit nothing (sky):
        // nothing is drawn — the ref resets the draw state on every hover pass before the pick.
        state.shown = false;
        return;
    };
    let acceptable = !cursor.unable;
    // Out of range forces radius 0.0 (the ref writes the global before the state fork), and a
    // zero radius draws at the literal default.
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
        // Axis-aligned box (no rotation term), the fixed ±2.0 vertical slab, box → [0,1]² UVs
        // (the ref's 0.5 UV bias is exactly this center mapping). Vertical fade mirrors the
        // ring's treatment so a draped wall piece dims instead of smearing at full strength.
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

/// Push the shown reticle onto the effect stream — vertex colour white (the ref writes
/// `0xffffffff`; the state picks the TEXTURE), standard alpha blend (the ref's GxRs blend-mode
/// 2), fog off, at the ring's decal rung.
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
        // Sort rung from the pre-water decal band — the reference draws this pass at `0x4836c5`
        // with flags `0x200122`, before the water; its liquid-receiver twin at `0x483727`
        // is the half that draws after it, and has no counterpart here until liquid becomes a
        // receiving surface. Raster margin from the family's shared coplanarity constant.
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

    /// **`0x4820f0`'s second guard**. One armed cursor, three seams, one decal:
    /// only a word that answers `TargetingWantsLocation`'s `& 0x60` may draw. Before this, the
    /// reticle asked `IsTargeting` alone, so arming Pick Lock (`0x4000`), Opening (`0x4800`) or an
    /// enchant (`0x0010`) dropped a green AoE circle on the ground under a cursor whose click is a
    /// hover-pick — director-reported, and true of the item seam since 0928.
    ///
    /// The ground is a real trimesh so "nothing drawn" is a verdict and not an empty scene: the
    /// same fixture with a DEST word must draw, or this test proves nothing.
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
            // A flat 100×100 yd ground quad at y = 0, world-space verts (the marked colliders'
            // identity-pose contract) — inside the ±2.0 slab and wide enough for any radius.
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

        // Blizzard's bare DEST word — the seam the decal belongs to.
        assert!(drawn_for(0x0040), "a DEST word draws its reticle");
        // SOURCE|DEST is still `& 0x60`.
        assert!(drawn_for(0x0060), "a SOURCE|DEST word draws its reticle");
        // The three words the director saw a grenade circle under.
        for word in [0x4000, 0x4800, 0x0010, 0x0800] {
            assert!(
                !drawn_for(word),
                "word {word:#06x} wants no location — no decal"
            );
        }
    }
}
