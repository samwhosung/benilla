//! Tests of [`super::WidgetArena`]'s mutators and registries.

use super::*;

fn arena() -> WidgetArena {
    WidgetArena::new()
}

// ── Effective visibility ─────────────────────────────────────────────────────────────────────

#[test]
fn hide_show_toggles_effective_visibility_and_reports_changes() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    assert!(a.frame(root).unwrap().effective_visible);

    let changed = a.set_shown(root, false);
    assert_eq!(changed, vec![root]);
    assert!(!a.frame(root).unwrap().effective_visible);

    assert!(a.set_shown(root, false).is_empty());

    let changed = a.set_shown(root, true);
    assert_eq!(changed, vec![root]);
    assert!(a.frame(root).unwrap().effective_visible);
}

#[test]
fn mid_tree_hide_blocks_a_shown_grandchild() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grand = a.create(FrameKind::Frame, None, Some(child));
    assert!(a.frame(grand).unwrap().effective_visible);

    let changed = a.set_shown(child, false);
    // Notify order: post-order, children before their parent (`0x76adee`, then `0x76adfd`).
    assert_eq!(changed, vec![grand, child]);
    assert!(a.frame(root).unwrap().effective_visible);
    assert!(!a.frame(child).unwrap().effective_visible);
    assert!(!a.frame(grand).unwrap().effective_visible);
    assert!(
        a.frame(grand).unwrap().shown,
        "grandchild's own shown bit is untouched"
    );

    let changed = a.set_shown(child, true);
    assert_eq!(changed, vec![grand, child]);
    assert!(a.frame(grand).unwrap().effective_visible);
}

#[test]
fn hidden_grandchild_stays_hidden_when_ancestor_reshows() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grand = a.create(FrameKind::Frame, None, Some(child));

    a.set_shown(grand, false);
    let changed = a.set_shown(root, false);
    // grand is already invisible, so the hide does not report it.
    assert_eq!(changed, vec![child, root]);

    let changed = a.set_shown(root, true);
    assert_eq!(changed, vec![child, root]);
    assert!(a.frame(child).unwrap().effective_visible);
    assert!(!a.frame(grand).unwrap().effective_visible);
}

// ── Strata ───────────────────────────────────────────────────────────────────────────────────

#[test]
fn set_strata_forces_whole_subtree() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grand = a.create(FrameKind::Frame, None, Some(child));

    a.set_frame_strata(root, Strata::Dialog);
    assert_eq!(a.frame(root).unwrap().strata, Strata::Dialog);
    assert_eq!(a.frame(child).unwrap().strata, Strata::Dialog);
    assert_eq!(a.frame(grand).unwrap().strata, Strata::Dialog);
}

// ── Level ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn set_level_delta_shifts_same_strata_children_only() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let same = a.create(FrameKind::Frame, None, Some(root));
    let cross = a.create(FrameKind::Frame, None, Some(root));
    // Distinct child levels show the delta is kept, not the absolute level.
    a.set_frame_level(same, 5, true);
    a.set_frame_level(cross, 2, true);
    a.set_frame_strata(cross, Strata::High);

    a.set_frame_level(root, 10, true);
    assert_eq!(a.frame(root).unwrap().level, 10);
    assert_eq!(
        a.frame(same).unwrap().level,
        15,
        "same-strata child shifted by +10"
    );
    assert_eq!(
        a.frame(cross).unwrap().level,
        2,
        "cross-strata child untouched"
    );
}

#[test]
fn level_shift_saturates_at_zero() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    a.set_frame_level(root, 10, true);
    a.set_frame_level(child, 3, true);
    // Root drops by 10, which would take the child to -7: it stops at 0.
    a.set_frame_level(root, 0, true);
    assert_eq!(a.frame(child).unwrap().level, 0);
}

/// The compaction moves each frame of a shifted level through `0x76a4f0` without propagation
/// (`0x764faf`/`0x764fb6`), walking the list of every frame in creation order (`0x764f80`), so a
/// shifted visible frame relinks in that order, while a hidden one and an unshifted one keep
/// their link. Creation order is not arena order once a destroyed frame's slot is reused.
#[test]
fn compaction_relinks_the_shifted_visible_frames_in_creation_order() {
    let mut a = arena();
    let low = a.create(FrameKind::Frame, None, None);
    let doomed = a.create(FrameKind::Frame, None, None);
    let first = a.create(FrameKind::Frame, None, None);
    a.destroy(doomed);
    let second = a.create(FrameKind::Frame, None, None); // reuses `doomed`'s slot, before `first`
    let hidden = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(first));
    a.set_shown(hidden, false);
    for h in [first, second, hidden] {
        a.set_frame_level(h, 3, false);
    }
    a.set_frame_level(child, 6, false);
    // `first` linked last, so it is the newer link of the two.
    a.set_shown(first, false);
    a.set_shown(first, true);
    let seq = |a: &WidgetArena, h| a.frame(h).unwrap().insertion_seq;
    assert!(seq(&a, second) < seq(&a, first));
    let (low_seq, hidden_seq) = (seq(&a, low), seq(&a, hidden));

    assert_eq!(
        a.compact_levels(Strata::Medium),
        3,
        "{{0, 3, 6}} packs to [0, 3)"
    );
    for h in [first, second, hidden] {
        assert_eq!(a.frame(h).unwrap().level, 1);
    }
    assert_eq!(
        a.frame(child).unwrap().level,
        2,
        "the child moves on its own level's account, not by its parent's delta"
    );
    assert!(
        seq(&a, first) < seq(&a, second),
        "the shifted level's visible frames relink in creation order"
    );
    assert_eq!(seq(&a, low), low_seq, "an unshifted frame keeps its link");
    assert_eq!(seq(&a, hidden), hidden_seq, "a hidden frame is in no list");
}

/// Compaction (`0x764eb0`) reads the stratum's bound `bucket+0x8` (`0x764ec2`) and examines only
/// the levels below it (`0x764f4c`, `0x764fcf`). The bound is the highest visible link plus one,
/// so a frame hidden above it keeps its level, and one hidden below it is packed with the rest.
#[test]
fn compaction_examines_only_the_levels_below_the_bound() {
    let mut a = arena();
    let low = a.create(FrameKind::Frame, None, None);
    let below = a.create(FrameKind::Frame, None, None);
    let above = a.create(FrameKind::Frame, None, None);
    let panel = a.create(FrameKind::Frame, None, None);
    a.set_shown(below, false);
    a.set_shown(above, false);
    a.set_frame_level(below, 3, false);
    a.set_frame_level(above, 6, false);
    a.set_frame_level(panel, 5, false); // the only high link: the bound is 6

    assert_eq!(
        a.compact_levels(Strata::Medium),
        3,
        "{{0, 3, 5}} pack to [0, 3); level 6 is not below the bound"
    );
    let levels = [low, below, panel, above].map(|h| a.frame(h).unwrap().level);
    assert_eq!(levels, [0, 1, 2, 6]);

    // The compaction set the bound to its count: the stratum's next raise lands at 3.
    assert_eq!(a.compact_levels(Strata::Medium), 3);
    assert_eq!(a.frame(above).unwrap().level, 6);
}

/// The bound grows on a visible link only (`0x764780`): a show, or a level or stratum change of a
/// visible frame. A hidden frame's level never reaches it.
#[test]
fn the_bound_grows_on_a_visible_link_only() {
    let mut a = arena();
    let f = a.create(FrameKind::Frame, None, None);
    let g = a.create(FrameKind::Frame, None, None);
    a.set_shown(f, false);
    a.set_frame_level(f, 9, false);
    assert_eq!(a.compact_levels(Strata::Medium), 1, "only g's level 0");
    assert_eq!(a.frame(f).unwrap().level, 9);

    a.set_shown(f, true);
    assert_eq!(
        a.compact_levels(Strata::Medium),
        2,
        "f linked at 9: {{0, 9}}"
    );
    assert_eq!(a.frame(f).unwrap().level, 1);

    a.set_frame_level(g, 7, false);
    a.set_frame_strata(f, Strata::Dialog);
    assert_eq!(
        a.compact_levels(Strata::Dialog),
        1,
        "f's relink grows the new stratum's bound; the old one is unchanged"
    );
    assert_eq!(a.compact_levels(Strata::Medium), 1);
    assert_eq!(a.frame(g).unwrap().level, 0);
}

// ── Scale ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn scale_propagates_multiplicatively() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grand = a.create(FrameKind::Frame, None, Some(child));

    a.set_scale(root, 2.0);
    a.set_scale(child, 3.0);
    assert_eq!(a.frame(root).unwrap().effective_scale, 2.0);
    assert_eq!(a.frame(child).unwrap().effective_scale, 6.0); // 2 * 3
    assert_eq!(a.frame(grand).unwrap().effective_scale, 6.0); // 2 * 3 * 1

    a.set_scale(grand, 0.5);
    assert_eq!(a.frame(grand).unwrap().effective_scale, 3.0); // 6 * 0.5
}

#[test]
fn scale_epsilon_gate_skips_subthreshold_change() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    a.set_scale(child, 4.0);
    assert_eq!(a.frame(child).unwrap().effective_scale, 4.0);

    // A sub-ε change to the root prunes the recursion, so the child keeps its exact value.
    let tiny = 1.0 + (SCALE_EPS as f32) / 4.0;
    a.set_scale(root, tiny);
    assert_eq!(
        a.frame(child).unwrap().effective_scale,
        4.0,
        "sub-ε parent change must not disturb the subtree"
    );
}

// ── Reparenting ──────────────────────────────────────────────────────────────────────────────

#[test]
fn reparent_reinherits_visibility_and_scale() {
    let mut a = arena();
    let hidden = a.create(FrameKind::Frame, None, None);
    a.set_shown(hidden, false);
    a.set_scale(hidden, 2.0);
    let visible = a.create(FrameKind::Frame, None, None);
    a.set_scale(visible, 3.0);

    let child = a.create(FrameKind::Frame, None, Some(visible));
    assert!(a.frame(child).unwrap().effective_visible);
    assert_eq!(a.frame(child).unwrap().effective_scale, 3.0);

    let changed = a.set_parent(child, Some(hidden));
    assert_eq!(changed, vec![child]);
    assert!(!a.frame(child).unwrap().effective_visible);
    assert_eq!(a.frame(child).unwrap().effective_scale, 2.0);
    assert_eq!(
        a.frame(visible).unwrap().children,
        Vec::<FrameHandle>::new()
    );
    assert_eq!(a.frame(hidden).unwrap().children, vec![child]);
}

#[test]
fn reparent_cycle_is_rejected() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let changed = a.set_parent(root, Some(child));
    assert!(changed.is_empty());
    assert_eq!(a.frame(root).unwrap().parent, None);
    assert_eq!(a.frame(child).unwrap().parent, Some(root));
}

// ── Named registry ──────────────────────────────────────────────────────────────────────────

#[test]
fn named_registry_is_non_overwriting() {
    let mut a = arena();
    let first = a.create(FrameKind::Frame, Some("MyFrame".into()), None);
    let second = a.create(FrameKind::Frame, Some("MyFrame".into()), None);
    assert_ne!(first, second);
    assert_eq!(a.lookup("MyFrame"), Some(first));
    assert_eq!(a.frame(second).unwrap().name.as_deref(), Some("MyFrame"));
}

#[test]
fn destroy_unpublishes_and_recurses() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, Some("Root".into()), None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let region = a
        .create_region(child, RegionKind::Texture, DrawLayer::Artwork, 0)
        .unwrap();
    assert_eq!(a.lookup("Root"), Some(root));

    a.destroy(root);
    assert!(a.frame(root).is_none());
    assert!(a.frame(child).is_none(), "subtree destroyed");
    assert!(a.region(region).is_none(), "regions destroyed");
    assert_eq!(a.lookup("Root"), None, "name unpublished");
}

#[test]
fn generational_handle_detects_reuse() {
    let mut a = arena();
    let h = a.create(FrameKind::Frame, None, None);
    a.destroy(h);
    let h2 = a.create(FrameKind::Frame, None, None);
    assert!(a.frame(h).is_none());
    assert!(a.frame(h2).is_some());
}

// ── Alpha ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn set_alpha_overwrites_the_subtree() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grandchild = a.create(FrameKind::Frame, None, Some(child));
    a.set_alpha(root, 0.5);
    for h in [root, child, grandchild] {
        assert_eq!(a.frame(h).unwrap().alpha, 0.5);
        assert_eq!(a.frame(h).unwrap().effective_alpha, 0.5);
    }
    // Last write wins: the child's own SetAlpha holds until the parent sets again.
    a.set_alpha(child, 1.0);
    assert_eq!(a.frame(root).unwrap().effective_alpha, 0.5);
    assert_eq!(a.frame(child).unwrap().effective_alpha, 1.0);
    assert_eq!(a.frame(grandchild).unwrap().effective_alpha, 1.0);
    // A frame created under a dimmed parent starts at 1.0.
    let late = a.create(FrameKind::Frame, None, Some(root));
    assert_eq!(a.frame(late).unwrap().effective_alpha, 1.0);
}

// ── The per-kind registries ──────────────────────────────────────────────────────────────────

/// Includes a tooltip destroyed as another frame's child.
#[test]
fn the_tooltip_registry_tracks_live_gametooltips() {
    let mut a = arena();
    assert!(a.tooltip_kinds().is_empty());

    let holder = a.create(FrameKind::Frame, None, None);
    let loose = a.create(FrameKind::GameTooltip, None, None);
    let child = a.create(FrameKind::GameTooltip, None, Some(holder));
    let _plain = a.create(FrameKind::Button, None, None);
    assert_eq!(a.tooltip_kinds(), &[loose, child]);

    a.destroy(holder);
    assert_eq!(a.tooltip_kinds(), &[loose]);

    a.destroy(loose);
    assert!(a.tooltip_kinds().is_empty());
}

// ── The region-removed hook (frame vtable `+0x2c`) ───────────────────────────────────────────

/// Freeing, reparenting or detaching a region runs its parent's hook, and each kind that keeps a
/// region in a slot lets go of it: StatusBar `0x783110`, Slider `0x789790`, ColorSelect
/// `0x78b530`, CheckButton `0x785230` over Button `0x778cb0`.
#[test]
fn a_region_leaving_its_parent_empties_the_kind_slot_that_held_it() {
    let mut a = arena();
    let other = a.create(FrameKind::Frame, None, None);
    let bar = a.create(FrameKind::StatusBar, None, None);
    let slider = a.create(FrameKind::Slider, None, None);
    let wheel = a.create(FrameKind::ColorSelect, None, None);
    let check = a.create(FrameKind::CheckButton, None, None);
    let tex = |a: &mut WidgetArena, f| {
        a.create_region(f, RegionKind::Texture, DrawLayer::Artwork, 0)
            .unwrap()
    };
    let (b, t, w, c, n) = (
        tex(&mut a, bar),
        tex(&mut a, slider),
        tex(&mut a, wheel),
        tex(&mut a, check),
        tex(&mut a, check),
    );
    match &mut a.frame_mut(bar).unwrap().kind_state {
        KindState::StatusBar(s) => s.bar = Some(b),
        _ => unreachable!(),
    }
    match &mut a.frame_mut(slider).unwrap().kind_state {
        KindState::Slider(s) => s.thumb = Some(t),
        _ => unreachable!(),
    }
    match &mut a.frame_mut(wheel).unwrap().kind_state {
        KindState::ColorSelect(s) => s.value_thumb = Some(w),
        _ => unreachable!(),
    }
    match &mut a.frame_mut(check).unwrap().kind_state {
        KindState::Button(s) => {
            s.checked_tex = Some(c);
            s.set_state_slot(ButtonVisualState::Normal, Some(n));
        }
        _ => unreachable!(),
    }
    // Freed, reparented, detached, freed, and the shown normal texture freed.
    assert!(a.destroy_region(b));
    assert!(a.set_region_owner(t, Some(other)));
    assert!(a.set_region_owner(w, None));
    assert!(a.destroy_region(c));
    assert!(a.destroy_region(n));
    assert!(
        matches!(&a.frame(bar).unwrap().kind_state, KindState::StatusBar(s) if s.bar.is_none())
    );
    assert!(
        matches!(&a.frame(slider).unwrap().kind_state, KindState::Slider(s) if s.thumb.is_none())
    );
    assert!(matches!(&a.frame(wheel).unwrap().kind_state,
        KindState::ColorSelect(s) if s.value_thumb.is_none()));
    let KindState::Button(s) = &a.frame(check).unwrap().kind_state else {
        unreachable!()
    };
    assert_eq!((s.checked_tex, s.normal), (None, None));
}
