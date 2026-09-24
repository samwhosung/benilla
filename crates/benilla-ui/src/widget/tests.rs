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
    assert_eq!(changed, vec![child, grand]); // pre-order
    assert!(a.frame(root).unwrap().effective_visible);
    assert!(!a.frame(child).unwrap().effective_visible);
    assert!(!a.frame(grand).unwrap().effective_visible);
    assert!(
        a.frame(grand).unwrap().shown,
        "grandchild's own shown bit is untouched"
    );

    let changed = a.set_shown(child, true);
    assert_eq!(changed, vec![child, grand]);
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
    assert_eq!(changed, vec![root, child]);

    let changed = a.set_shown(root, true);
    assert_eq!(changed, vec![root, child]);
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
    a.set_frame_level(child, 12, true);
    // Root drops by 10, taking the child from 12 to 2.
    a.set_frame_level(root, 0, true);
    assert_eq!(a.frame(child).unwrap().level, 2);
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
