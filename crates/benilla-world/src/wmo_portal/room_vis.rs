//! **Is a claimed WMO room visible this frame?** — the one answer two elections share.
//!
//! The reference elects an object standing in a WMO group with its building (`0x6834e0`, inside
//! the WMO-group render path): if the group isn't rendered this frame, the object is pass 2 —
//! neither drawn nor ticked. Two of our systems ask exactly that question of a body's
//! [`UnitWmoRoom`] claim: the anim-LOD gate (`creature_anim::lod`, decision 0739 — parks the
//! pose) and the body draw election (`crate::exterior_cull`, decision 1475 — hides the root).
//! One function answers both, so the two can never drift apart (the drift is how 0448's park and
//! 0648's draw ended up disagreeing for a month).

use bevy::prelude::*;

use benilla_assets::{WmoGroupNav, WmoModel, WmoPortalRef};

use super::{UnitWmoRoom, WmoPortalInstance, WmoRoom};

/// The room leg's resolve chain: unit claim → placement instance → model → PVS bits. Fail-open
/// at every seam (no claim, despawned placement, still-loading model) — a lookup miss must never
/// hide or park a drawable body.
pub fn room_pvs_visible(
    room: Option<&UnitWmoRoom>,
    instances: &Query<&WmoPortalInstance>,
    wmos: &Assets<WmoModel>,
) -> bool {
    let Some(WmoRoom { instance, group }) = room.and_then(|r| r.room()) else {
        return true;
    };
    let Ok(inst) = instances.get(instance) else {
        return true;
    };
    let Some(model) = wmos.get(&inst.handle) else {
        return true;
    };
    room_visible(&inst.visible, &model.group_nav, &model.portal_refs, group)
}

/// Is the claimed group — or any group one portal hop from it — in the PVS? The one-hop union is
/// the doorway-straddle guard: a body can extend past its feet's room only through a portal
/// opening, so the neighbour set bounds everything of the unit that could be on screen. Indices
/// out of range read visible (fail-open, [`super::WmoGroupVis::drawn_by`]'s convention); a group
/// with no portal refs at all can only be seen with the camera inside it (the flood cannot reach
/// it), which the direct bit already answered.
fn room_visible(visible: &[bool], nav: &[WmoGroupNav], refs: &[WmoPortalRef], group: u16) -> bool {
    let vis = |g: usize| visible.get(g).copied().unwrap_or(true);
    if vis(group as usize) {
        return true;
    }
    let Some(n) = nav.get(group as usize) else {
        return true;
    };
    let Some(hops) = refs.get(n.ref_start as usize..(n.ref_start as usize + n.ref_count as usize))
    else {
        return true;
    };
    hops.iter().any(|r| vis(r.group as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A [`WmoGroupNav`] whose only meaningful fields are its portal-ref slice — these tests
    /// never touch flags/bounds.
    fn nav(ref_start: u16, ref_count: u16) -> WmoGroupNav {
        WmoGroupNav {
            flags: 0,
            bbox_min: [0.0; 3],
            bbox_max: [0.0; 3],
            ref_start,
            ref_count,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: benilla_formats::NO_GROUP_LIQUID,
        }
    }

    fn pref(group: u16) -> WmoPortalRef {
        WmoPortalRef {
            portal: 0,
            group,
            side: 1,
        }
    }

    /// The room predicate's whole truth table: direct bit, the one-hop straddle guard, the
    /// all-dark verdict, the sealed room, and both fail-open seams (group past every table, a
    /// ref slice past the refs vec).
    #[test]
    fn room_visible_covers_the_hop_guard_and_fails_open() {
        // Groups 0 ↔ 1 share one portal; group 2 is sealed (no refs).
        let navs = vec![nav(0, 1), nav(1, 1), nav(2, 0)];
        let refs = vec![pref(1), pref(0)];
        assert!(
            room_visible(&[true, false, false], &navs, &refs, 0),
            "direct PVS bit"
        );
        assert!(
            room_visible(&[true, false, false], &navs, &refs, 1),
            "own bit dark but the neighbour lit — the doorway-straddle guard keeps it live"
        );
        assert!(
            !room_visible(&[false, false, true], &navs, &refs, 0),
            "own room and every hop dark ⇒ not visible"
        );
        assert!(
            !room_visible(&[true, true, false], &navs, &refs, 2),
            "a sealed room's own bit decides — no hops exist to save it"
        );
        assert!(
            room_visible(&[false], &navs, &refs, 9),
            "a group past the PVS table reads visible (fail-open)"
        );
        assert!(
            room_visible(&[false], &[nav(7, 2)], &refs, 0),
            "a ref slice past the refs vec reads visible (fail-open)"
        );
    }
}
