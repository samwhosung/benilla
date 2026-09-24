//! Rust-driven tests of [`super::WidgetArena`]'s propagation mutators: effective visibility,
//! strata subtree-force, level delta-shift, scale propagation, reparenting, the named registry, and
//! the byte-verified alpha overwrite-cascade. Split out alongside the propagation mutators purely
//! for size — the tests exercise both `super` (create/destroy) and `super::propagation` (the
//! mutators) through the one public `WidgetArena` surface.

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

    // Hiding again is a no-op (already hidden).
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

    // Hide the middle frame: child + grand lose effective visibility, though both are `shown`.
    let changed = a.set_shown(child, false);
    assert_eq!(changed, vec![child, grand]); // pre-order
    assert!(a.frame(root).unwrap().effective_visible);
    assert!(!a.frame(child).unwrap().effective_visible);
    assert!(!a.frame(grand).unwrap().effective_visible);
    assert!(
        a.frame(grand).unwrap().shown,
        "grandchild's own shown bit is untouched"
    );

    // Re-show the middle: both come back.
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

    a.set_shown(grand, false); // grand's own shown = false
    let changed = a.set_shown(root, false); // hide the top
                                            // grand was already effectively invisible, so it does NOT re-report on the hide.
    assert_eq!(changed, vec![root, child]);

    let changed = a.set_shown(root, true); // re-show top
                                           // child comes back; grand stays hidden (its own shown is false).
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
    // root level 0; give the children distinct levels to prove the *delta* (relative offset) is
    // preserved, not the absolute value.
    a.set_frame_level(same, 5, true);
    a.set_frame_level(cross, 2, true);
    a.set_frame_strata(cross, Strata::High); // move `cross` to another strata

    // Raise root by +10 with propagation.
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
    a.set_frame_level(child, 12, true); // child two above root
                                        // Drop root to 0 (delta -10): child would be 2.
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
    // Set a marker on the child's effective scale we can detect being (not) overwritten.
    a.set_scale(child, 4.0);
    assert_eq!(a.frame(child).unwrap().effective_scale, 4.0);

    // Nudge root by less than ε: effective scale of root barely moves, so the recursion into the
    // child is pruned and the child keeps its exact prior value.
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

    // Move under the hidden parent: child loses visibility and re-inherits scale 2.0.
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
    // Making root a child of its own child would cycle — rejected, no-op.
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
    // The first writer owns the name; the duplicate still carries its own `name` field.
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
    // A new frame may reuse the slot; the old handle must not resolve to it.
    let h2 = a.create(FrameKind::Frame, None, None);
    assert!(a.frame(h).is_none());
    assert!(a.frame(h2).is_some());
}

// ── Alpha (the byte-verified overwrite-cascade, SetAlpha 0x76a690) ───────────────────────────

#[test]
fn set_alpha_overwrites_the_subtree() {
    let mut a = arena();
    let root = a.create(FrameKind::Frame, None, None);
    let child = a.create(FrameKind::Frame, None, Some(root));
    let grandchild = a.create(FrameKind::Frame, None, Some(child));
    a.set_alpha(root, 0.5);
    // The same raw value lands on every descendant frame (a flatten, not a product).
    for h in [root, child, grandchild] {
        assert_eq!(a.frame(h).unwrap().alpha, 0.5);
        assert_eq!(a.frame(h).unwrap().effective_alpha, 0.5);
    }
    // Last write wins: a child's own later SetAlpha diverges until the parent sets again…
    a.set_alpha(child, 1.0);
    assert_eq!(a.frame(root).unwrap().effective_alpha, 0.5);
    assert_eq!(a.frame(child).unwrap().effective_alpha, 1.0);
    assert_eq!(a.frame(grandchild).unwrap().effective_alpha, 1.0);
    // …and a frame created under a dimmed parent starts at 1.0 (creation doesn't re-push).
    let late = a.create(FrameKind::Frame, None, Some(root));
    assert_eq!(a.frame(late).unwrap().effective_alpha, 1.0);
}

// ── The per-kind registries ──────────────────────────────────────────────────────────────────

/// The tooltip registry is exactly the live GameTooltips — including when one dies as somebody
/// else's child, which is the case a registry maintained only at the explicit `destroy` call site
/// would miss. Three hot paths read it instead of scanning the resolve's roster, so
/// a stale entry is a dangling handle in the layout pre-pass, not a cosmetic drift.
#[test]
fn the_tooltip_registry_tracks_live_gametooltips() {
    let mut a = arena();
    assert!(a.tooltip_kinds().is_empty());

    let holder = a.create(FrameKind::Frame, None, None);
    let loose = a.create(FrameKind::GameTooltip, None, None);
    let child = a.create(FrameKind::GameTooltip, None, Some(holder));
    // A non-tooltip kind never enters the list.
    let _plain = a.create(FrameKind::Button, None, None);
    assert_eq!(a.tooltip_kinds(), &[loose, child]);

    // Destroying the PARENT takes its tooltip child with it.
    a.destroy(holder);
    assert_eq!(a.tooltip_kinds(), &[loose]);

    a.destroy(loose);
    assert!(a.tooltip_kinds().is_empty());
}
