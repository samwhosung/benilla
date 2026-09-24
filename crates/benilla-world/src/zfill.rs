//! **The depth-prime twin** — the reference's `M2UseZFill` ("z-fill transparent objects", default
//! ON, `0x82e748`) reproduced for fading/stealthed entities (decision 0831).
//!
//! The mechanism it reproduces: when a model instance draws translucent (`0 < A < 1` — stealth's
//! CharProc 0.3, the appear/despawn ramps, the self-avatar zoom feather), the reference's collector
//! CLONES each of its z-writing mesh commands into a twin that draws **first** (comparator key
//! `cmd+0x08` desc inside the instance's sort-key tie), **colour-masked off** (`glColorMask 0`),
//! **blend off**, **z-write ON**. The whole model's nearest-surface depth is therefore in the
//! buffer before any of its colour batches draw, each colour fragment passes the depth test only at
//! that nearest surface, and every interior/overlapped layer fails — **one blended layer
//! everywhere**. Without it, a stealthed body's overlapping parts compound `0.3` over `0.3` into
//! the darker patches of ledger B182.
//!
//! The benilla shape: [`sync_zfill_twins`] watches every fadeable part
//! ([`FadeMaterials::zfill`] carries the twin material, built next to the part's blend twin) and
//! keeps a **twin child mesh** alive on it exactly while the part's `MeshTag` alpha is translucent
//! ([`crate::mesh_tag::translucent`]). The twin shares the part's mesh, transform and tag (same
//! skinned pose, same rig slot), and its material's `specialize` masks the colour writes off and
//! forces z-write on; its ordering comes from the material's negative `Transparent3d` sort bias
//! ([`crate::model_render::ZFILL_SORT_BIAS`]).
//!
//! Two deliberate departures from the reference, both recorded in 0831:
//! - **Scope**: the reference twins every translucent mesh command always (authored-Blend glass
//!   included); we twin only while an instance alpha episode is live, so every steady-state look is
//!   untouched.
//! - **Ordering**: the reference ties one instance's commands to a single sort key and puts twins
//!   first inside the tie; Bevy sorts per-entity, so the twins ride a fixed −8 yd bias instead —
//!   transparent scene content sorting within that window behind a fading body draws after the
//!   prime and is depth-clipped where the body covers it, for the seconds the episode lasts.

use bevy::camera::visibility::RenderLayers;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::model_fade::FadeMaterials;

/// On a fadeable part: its live depth-prime twin entity. Present exactly while the part draws
/// translucent — [`sync_zfill_twins`] is the only writer.
#[derive(Component)]
pub(crate) struct ZfillTwin(Entity);

/// On a twin: the part it primes for. The mesh/tag re-sync key, and the marker that keeps the twin
/// out of every part query (writers key on [`FadeMaterials`], which a twin never carries; this side
/// of the disjointness is the `Without` in the queries below).
#[derive(Component)]
pub(crate) struct ZfillTwinOf(Entity);

/// Keep every fadeable part's depth-prime twin in sync with its episode: spawn the twin child when
/// the part's tag alpha enters the translucent band, despawn it when the tag settles (opaque or the
/// part itself despawns — a child dies with its parent), and mirror the part's mesh + tag onto a
/// live twin (a gear-change redress swaps the part's mesh in place, 0835; the rig slot rides the
/// tag, so the twin skins into the same pose as the surface it primes).
///
/// Runs in `PostUpdate`, after every Update-side tag writer (the aura author, the appear/despawn
/// ramp, the self feather) — the twin's first frame is the episode's first frame.
/// One fadeable part as [`sync_zfill_twins`] sees it: its twin material source, live tag, mesh,
/// and (maybe) the twin it already has.
type ZfillParts<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static FadeMaterials,
        &'static MeshTag,
        &'static Mesh3d,
        Option<&'static ZfillTwin>,
        // The part's render layer, when it is not on the default one. A world part carries none
        // and the twin needs none; a BOOTH part is on its booth's own layer, and a twin that did
        // not copy it would prime depth in the WORLD view (where it is invisible and useless)
        // while the booth body it exists for stayed multi-layered.
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
            continue; // no-z-write / Mod / Mod2x batch — the reference's own twin gate
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
        // The part's tag moves every fade frame (its alpha bits); mirroring it keeps the rig slot
        // — the half the twin's vertex stage actually reads — correct across a redress or a slot
        // reassignment without a dedicated edge.
        let Ok((_, _, ptag, pmesh, _, _)) = parts.get(of.0) else {
            continue; // part going away this frame — the child despawns with it
        };
        if tag.0 != ptag.0 {
            tag.0 = ptag.0;
        }
        if mesh.0 != pmesh.0 {
            mesh.0 = pmesh.0.clone();
        }
    }
}

/// The layer's instrument (`WOW_MOVE_TRACE=<path>`, tag `zfl`): one line per twin edge, so "did
/// the depth prime actually arm on this body" is answered from the trace beside the `aur` lines —
/// a stealth press prints `aur arm … target 0.30` and then one `zfl arm` per fadeable part.
fn trace(what: &str, part: Entity, tag: u32) {
    if !benilla_assets::trace::enabled() {
        return;
    }
    benilla_assets::trace::line("zfl", &format!("{what} part={part} tag={tag:#010x}"));
}

/// The depth-prime lane's own registration (decision 1163, stage zero) — `EntitiesPlugin`'s last
/// piece of the render lane.
pub fn plugin(app: &mut App) {
    // The depth-prime twins (decision 0831 — the reference's `M2UseZFill`): PostUpdate, after the
    // Update-side tag writers, so a twin arms on its episode's first frame.
    app.add_systems(PostUpdate, sync_zfill_twins);
}

#[cfg(test)]
mod tests {
    /// The activation band, pinned against the tag encoding it reads: the untagged sentinel and a
    /// full alpha field are opaque (no twin), every ramp value in between is translucent (twin),
    /// and the flag bits never leak into the decision.
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
        // alpha_bits floors a non-positive alpha at field value 1 — still armed, matching the
        // reference (only A ≤ 0 culls the batch, and that never reaches a tag).
        assert!(mesh_tag::translucent(mesh_tag::with_alpha(0, 0.0)));
        // The standalone flag bits are not payload: a highlighted-but-untagged instance is opaque.
        assert!(!mesh_tag::translucent(mesh_tag::HIGHLIGHT_BIT));
        // …and a highlighted mid-ramp instance still arms.
        assert!(mesh_tag::translucent(
            mesh_tag::with_alpha(0, 0.3) | mesh_tag::HIGHLIGHT_BIT
        ));
    }
}
