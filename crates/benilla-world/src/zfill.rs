//! The depth-prime twin: the reference's `M2UseZFill` ("z-fill transparent objects", default on,
//! `0x82e748`). For a translucent instance (`0 < A < 1`: stealth, the appear and despawn ramps,
//! the self-avatar feather) its collector clones each z-writing mesh command into a twin drawn
//! first (comparator key `cmd+0x08` descending in the instance's tie), colour masked off, blend
//! off, z-write on, so overlapping parts blend as one layer, not `0.3` over `0.3`.
//! [`sync_zfill_twins`] keeps such a twin child, sharing mesh, transform and tag, on each fadeable
//! part while its `MeshTag` alpha is translucent.
//!
//! Deviation: the reference twins every translucent command, authored-blend glass included; here
//! twins exist only during an instance alpha episode, so every steady-state look is untouched.
//!
//! Deviation: Bevy sorts per entity, so twins lead by a fixed −8 yd sort bias
//! ([`crate::model_render::ZFILL_SORT_BIAS`]) instead of by the instance's sort-key tie;
//! transparent content within that window behind a fading body is depth-clipped where the body
//! covers it, for the episode.

use bevy::camera::visibility::RenderLayers;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::model_fade::FadeMaterials;

/// On a fadeable part: its live twin, present exactly while the part draws translucent.
#[derive(Component)]
pub(crate) struct ZfillTwin(Entity);

/// On a twin: the part it primes, and the marker that keeps twins out of part queries.
#[derive(Component)]
pub(crate) struct ZfillTwinOf(Entity);

/// One fadeable part as [`sync_zfill_twins`] sees it: twin material, live tag, mesh, and its twin
/// if it has one.
type ZfillParts<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static FadeMaterials,
        &'static MeshTag,
        &'static Mesh3d,
        Option<&'static ZfillTwin>,
        // A booth part's own render layer, which its twin must copy to prime the booth's view.
        Option<&'static RenderLayers>,
    ),
    Without<ZfillTwinOf>,
>;

pub(crate) fn sync_zfill_twins(
    mut commands: Commands,
    parts: ZfillParts,
    mut twins: Query<(&ZfillTwinOf, &mut MeshTag, &mut Mesh3d), Without<FadeMaterials>>,
) {
    for (part, fm, tag, mesh, twin, layers) in &parts {
        let Some(mat) = fm.zfill.as_ref() else {
            continue; // no-z-write, Mod or Mod2x batch: the reference's own twin gate
        };
        let active = crate::mesh_tag::translucent(tag.0);
        match twin {
            None if active => {
                let mut twin = commands.spawn((
                    Mesh3d(mesh.0.clone()),
                    MeshMaterial3d(mat.clone()),
                    MeshTag(tag.0),
                    Transform::IDENTITY,
                    ZfillTwinOf(part),
                    ChildOf(part),
                ));
                if let Some(layers) = layers {
                    twin.insert(layers.clone());
                }
                let t = twin.id();
                commands.entity(part).insert(ZfillTwin(t));
                trace("arm", part, tag.0);
            }
            Some(t) if !active => {
                commands.entity(t.0).despawn();
                commands.entity(part).remove::<ZfillTwin>();
                trace("release", part, tag.0);
            }
            _ => {}
        }
    }
    for (of, mut tag, mut mesh) in &mut twins {
        // Mirroring the part's tag and mesh keeps the rig slot right across a redress or reslot.
        let Ok((_, _, ptag, pmesh, _, _)) = parts.get(of.0) else {
            continue; // the part is going away; the child despawns with it
        };
        if tag.0 != ptag.0 {
            tag.0 = ptag.0;
        }
        if mesh.0 != pmesh.0 {
            mesh.0 = pmesh.0.clone();
        }
    }
}

/// Trace tag `zfl` (`WOW_MOVE_TRACE=<path>`): one line per twin arm or release.
fn trace(what: &str, part: Entity, tag: u32) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    benilla_assets::trace::line("zfl", &format!("{what} part={part} tag={tag:#010x}"));
}

/// The depth-prime lane's registration.
pub fn plugin(app: &mut App) {
    // PostUpdate, after every Update-side tag writer, so a twin arms on its episode's first frame.
    app.add_systems(PostUpdate, sync_zfill_twins);
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_twin_arms_exactly_on_the_translucent_band() {
        use crate::mesh_tag;
        assert!(!mesh_tag::translucent(0), "untagged ⇒ opaque, no twin");
        assert!(
            !mesh_tag::translucent(mesh_tag::with_alpha(0, 1.0)),
            "opaque ramp end: released"
        );
        for alpha in [0.02, 0.3, 0.5, 0.9] {
            assert!(
                mesh_tag::translucent(mesh_tag::with_alpha(0, alpha)),
                "mid-ramp {alpha} must arm the twin"
            );
        }
        // `alpha_bits` floors a non-positive alpha at 1, still armed: in the reference only A ≤ 0
        // culls the batch, and that never reaches a tag.
        assert!(mesh_tag::translucent(mesh_tag::with_alpha(0, 0.0)));
        // The standalone flag bits are not payload: a highlighted-but-untagged instance is opaque.
        assert!(!mesh_tag::translucent(mesh_tag::HIGHLIGHT_BIT));
        // A highlighted mid-ramp instance still arms.
        assert!(mesh_tag::translucent(
            mesh_tag::with_alpha(0, 0.3) | mesh_tag::HIGHLIGHT_BIT
        ));
    }
}
