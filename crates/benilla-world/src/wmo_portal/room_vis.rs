//! Is a claimed WMO room visible this frame? The reference elects an object standing in a WMO
//! group with its building (`0x6834e0`, in the WMO-group render path): when the group is not
//! rendered, the object is neither drawn nor ticked. The anim-LOD gate (`creature_anim::lod`) and
//! the body draw election (`crate::exterior_cull`) both ask it of a body's [`UnitWmoRoom`], and one
//! function answers both so they cannot disagree.

use bevy::prelude::*;

use benilla_assets::{WmoGroupNav, WmoModel, WmoPortalRef};

use super::{UnitWmoRoom, WmoPortalInstance, WmoRoom};

/// Whether a body's claimed room is visible this frame. Every seam (no claim, a despawned
/// placement, a loading model) fails open: a lookup miss must never hide or park a body.
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

/// Is the claimed group, or any group one portal hop from it, in the PVS? The hop is the
/// doorway-straddle guard: a body extends past its room only through a portal opening. An index
/// out of range reads visible, as in [`super::WmoGroupVis::drawn_by`].
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

    /// A [`WmoGroupNav`] whose only meaningful fields are its portal-ref slice.
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

    /// Direct bit, the straddle guard, all dark, the sealed room, and both fail-open seams.
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
