//! **SmartScreenRect** — the client's shared draw-time anti-overlap placement solver
//! (`UIUtil\SmartScreenRect`). Every frame each participant submits its desired screen rect; the
//! solver relocates it off the rects already claimed **this frame in its own bucket** and then
//! claims the result, so later rects dodge earlier ones. Exactly two buckets exist in 5875 and
//! they never interact: **nameplates (bucket 0)** and **combat worldtext (bucket 1)** — a plate
//! never pushes a number. This is the mechanism behind the reference's whole non-overlap feel:
//! plates bounce off each other, a late number shoves the earlier one upward, crits (whose
//! measured half-extents balloon with the scale pop) both push and get pushed harder, and a
//! pushed rect **snaps back** the frame its blocker expires because the desire is re-derived
//! from the anchor every frame — the solver keeps no cross-frame state at all.
//!
//! The law, transcribed to our Y-DOWN logical-pixel viewport (the ref computes in the Y-UP gx
//! device space `[0,G44]×[0,G48]`; every clamp below is bound-symmetric, so the mirror is exact):
//! - **Solve** (`0x5097a0`): bounded FIFO/BFS. Seed = the input rect band-clamped on-screen
//!   (`0x509bf0` mode 0); its region (3×3, `0x509d80`) picks a 4-try push-direction order
//!   ([`ORDER`], `.rdata 0x8087e0`) fixed for the whole solve. Each dequeued node tries the 4
//!   directions in region order: a push against the **first** strictly-overlapping claimed rect
//!   (`0x509220`, zero padding — touching edges do not overlap) lands the node **edge-to-edge**
//!   with the blocker (`0x5091c0/0x509300/0x509360/0x5093c0`); an *unmoved* try means the spot
//!   is free — **adopt immediately**; a moved rect is enqueued for re-checking against the whole
//!   bucket. Dequeues are capped at `(⌊W/w⌋+1)·(⌊H/h⌋+1)` (`0x509dd0`); exhaustion returns the
//!   input unmodified — an unplaceable rect draws overlapping at its anchor, never hides.
//! - **Seat tail** (shared by both seats `0x509520`/`0x509ec0`): clamp the solved rect's center-X
//!   to `[halfW, W−halfW]` and its physical-TOP edge to `[halfH, H−halfH]`, rebuild top-anchored,
//!   re-normalize on-screen (`0x509e20`), then **claim** (`0x509660`).
//! - The node-strategy skip at `0x509874` is dead in 5875 (`+0x18` is never written past the
//!   ctor's `-1`) and the `showsmartrects` CVar has no readers — the solver is unconditionally
//!   live and every node tries all four directions. Both are replicated by simply not porting
//!   them.

use std::collections::VecDeque;

use bevy::math::{Rect, Vec2};

/// The near-TOP band, gx device units (`.rdata [0x8087cc]` = 0.0375): a rect whose physical top
/// edge is within `0.0375 × diagonal` px of the screen top is "near top" (region bit 2) and its
/// push order avoids UP. One gx unit = the screen diagonal (`√(W²+H²)` px).
const DDC_NEAR_TOP: f32 = 0.0375;
/// The near-BOTTOM band (`.rdata [0x8087d0]` = 0.01875): within `0.01875 × diagonal` px of the
/// screen bottom (region bit 3) the order avoids DOWN. The X "bands" are the literal screen
/// edges (`[0x8087d4]/[0x8087d8]` = 0.0) — bits 0/1 fire only when the rect is partly off-screen.
const DDC_NEAR_BOTTOM: f32 = 0.01875;

/// The 9-region × 4-try push-direction order (`.rdata 0x8087e0`), strategy ids 0=UP 1=LEFT
/// 2=RIGHT 3=DOWN (the `0x808870` table slots, physical Y-UP directions). Every region avoids
/// pushing further off its violated edge; the center region tries UP first — the reference's
/// signature "the number jumps up".
const ORDER: [[u8; 4]; 9] = [
    [0, 2, 3, 1], // 0 center:                UP RIGHT DOWN LEFT
    [1, 0, 2, 0], // 1 near bottom:           LEFT UP RIGHT UP
    [1, 3, 2, 3], // 2 near top:              LEFT DOWN RIGHT DOWN
    [0, 1, 3, 1], // 3 off right:             UP LEFT DOWN LEFT
    [0, 2, 3, 2], // 4 off left:              UP RIGHT DOWN RIGHT
    [0, 1, 0, 1], // 5 off right + near bottom
    [0, 2, 0, 2], // 6 off left  + near bottom
    [3, 1, 3, 1], // 7 off right + near top
    [3, 2, 3, 2], // 8 off left  + near top, and the catch-all
];

/// One claim bucket: the screen rects already placed this frame, in seat order. Each producer
/// owns its bucket (`Local`), clears it at the top of its frame pass ([`Self::clear`] =
/// `0x509500`), and seats through [`Self::resolve`] + [`Self::claim`]. Seat order is priority:
/// the first-claimed rect never moves, later ones push off it.
#[derive(Default)]
pub(crate) struct SmartBucket(Vec<Rect>);

impl SmartBucket {
    /// The per-frame reset (`0x509500`): count to zero, capacity kept.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    /// The shared seat law minus the claim: normalize on-screen → solve → clamp the resolved
    /// (center-X, top-edge) → rebuild top-anchored → normalize. Callers [`Self::claim`] the
    /// result (the plate seat snaps to whole pixels in between — a recorded benilla divergence,
    /// not ref law).
    pub(crate) fn resolve(&self, desired: Rect, viewport: Vec2) -> Rect {
        let (w, h) = (desired.width(), desired.height());
        let (half_w, half_h) = (w * 0.5, h * 0.5);
        let input = normalize(desired, viewport);
        let solved = self.solve(input, viewport);
        // The resolve wrapper's output pair (`0x509720`): resolved center-X + resolved physical
        // TOP edge (Y-down: min.y), each clamped half-extent inside (`max()` keeps an
        // over-wide rect centered instead of NaN-flipping the clamp bounds).
        let cx =
            ((solved.min.x + solved.max.x) * 0.5).clamp(half_w, (viewport.x - half_w).max(half_w));
        let top = solved
            .min
            .y
            .clamp(half_h, (viewport.y - half_h).max(half_h));
        normalize(Rect::new(cx - half_w, top, cx + half_w, top + h), viewport)
    }

    /// Append to the bucket (`0x509660`) — subsequent rects this frame dodge it.
    pub(crate) fn claim(&mut self, rect: Rect) {
        self.0.push(rect);
    }

    /// The bounded-BFS relocation core (`0x5097a0`). Returns the first collision-free placement
    /// in region-preference order, or `input` unchanged when the bound exhausts.
    fn solve(&self, input: Rect, viewport: Vec2) -> Rect {
        let (seed, flags) = clamp_to_bands(input, viewport);
        let region = region_index(flags);
        // `0x509dd0`: how many rects of this size tile the screen — the dequeue cap. A
        // degenerate size saturates the cast; such a rect never strictly overlaps and adopts
        // on its first try.
        let bound = ((viewport.x / input.width()).trunc() as i64 + 1)
            .saturating_mul((viewport.y / input.height()).trunc() as i64 + 1);
        let mut queue = VecDeque::from([seed]);
        for _ in 0..bound {
            let Some(node) = queue.pop_front() else {
                return input; // worklist dry (every branch went off-screen) — keep the anchor
            };
            // The per-try off-screen skip (`0x509bf0` mode≠0) tests the NODE's rect, which is
            // constant across its four tries — hoisted to one test per node.
            if band_flags(node, viewport) != 0 {
                continue;
            }
            for strategy in ORDER[region as usize] {
                let candidate = push(&self.0, node, strategy);
                if candidate == node {
                    return candidate; // unmoved = overlaps nothing — adopt
                }
                queue.push_back(candidate);
            }
        }
        input // bound exhausted — draw overlapping at the anchor
    }
}

/// The hard on-screen clamp (`0x509e20`): size-preserving translate, per-edge, the ref's
/// physical order bottom → top → left → right (so an oversize rect pins at the top / right).
fn normalize(r: Rect, viewport: Vec2) -> Rect {
    let mut r = r;
    if r.max.y > viewport.y {
        let d = r.max.y - viewport.y;
        r.min.y -= d;
        r.max.y -= d;
    }
    if r.min.y < 0.0 {
        let d = -r.min.y;
        r.min.y += d;
        r.max.y += d;
    }
    if r.min.x < 0.0 {
        let d = -r.min.x;
        r.min.x += d;
        r.max.x += d;
    }
    if r.max.x > viewport.x {
        let d = r.max.x - viewport.x;
        r.min.x -= d;
        r.max.x -= d;
    }
    r
}

/// The region-flag word (`0x509bf0`), Y-down: bit0 off LEFT edge, bit1 off RIGHT edge, bit2
/// physical top edge inside the near-TOP band, bit3 physical bottom edge inside the near-BOTTOM
/// band. Bands scale with the screen diagonal (gx device units).
fn band_flags(r: Rect, viewport: Vec2) -> u8 {
    let diag = viewport.length();
    let mut flags = 0;
    if r.min.x < 0.0 {
        flags |= 1;
    }
    if r.max.x > viewport.x {
        flags |= 2;
    }
    if r.min.y < DDC_NEAR_TOP * diag {
        flags |= 4;
    }
    if r.max.y > viewport.y - DDC_NEAR_BOTTOM * diag {
        flags |= 8;
    }
    flags
}

/// `0x509bf0` mode 0: the flags PLUS a size-preserving clamp of each offending edge back to its
/// band boundary (X to the screen edge, Y to the band edge) — the solve's seed rect.
///
/// **The offending edge is ASSIGNED the boundary and its opposite derived from the size** — the
/// law's own spelling (`bit2 → top = Y_hi; bottom = Y_hi − height`), and not the equivalent
/// translate-by-the-gap this shipped with, because the two are not equivalent in `f32` and the
/// difference is a whole plate jumping 19 px for one frame (decision 2172).
///
/// The seed this returns is immediately re-tested by [`band_flags`] inside [`SmartBucket::solve`]
/// (the per-node skip, `0x509bf0` mode 1), and the boundary test is a strict `<`/`>`, so a seed
/// sitting EXACTLY on the boundary passes. `r.min.y + (band - r.min.y)` does not land exactly on
/// `band`: for most inputs it does, and for the ones where the subtraction loses a bit it lands
/// one ULP below — the seed then re-fires its own flag, every try is skipped, the worklist empties
/// and the solve returns the UNCLAMPED input, which the seat tail clamps to half a plate from the
/// screen top instead. Measured on a Goldshire walk (the `vpl` trace): a plate riding the near-top
/// band at 27.54 px dropped to 9.0 for exactly one frame and came back, three times in ten
/// seconds — at `scr.y = 8.064` and nowhere either side of it, which is the signature of a
/// rounding tie rather than of any law.
fn clamp_to_bands(r: Rect, viewport: Vec2) -> (Rect, u8) {
    let diag = viewport.length();
    let flags = band_flags(r, viewport);
    let (w, h) = (r.width(), r.height());
    let mut r = r;
    if flags & 1 != 0 {
        r.min.x = 0.0;
        r.max.x = w;
    }
    if flags & 2 != 0 {
        r.max.x = viewport.x;
        r.min.x = viewport.x - w;
    }
    if flags & 4 != 0 {
        r.min.y = DDC_NEAR_TOP * diag;
        r.max.y = r.min.y + h;
    }
    if flags & 8 != 0 {
        r.max.y = viewport.y - DDC_NEAR_BOTTOM * diag;
        r.min.y = r.max.y - h;
    }
    (r, flags)
}

/// The 3×3 region index — the exact `0x509d80` decision tree, byte-read for all 16 flag words
/// (the degenerate multi-flag combos included; region 8 is not a catch-all): **bit0
/// (off left) dominates and ignores bit1 beneath it;
/// bit2 (near top) is next and ignores bit3 beneath it; then bit1; then bit3.** So off-both-X
/// (flags 3) is region 4, spanning-both-bands (flags 12) is region 2 — never reachable by a
/// plate or a number, but exact.
fn region_index(flags: u8) -> u8 {
    if flags & 1 != 0 {
        if flags & 4 != 0 {
            8
        } else if flags & 8 != 0 {
            6
        } else {
            4
        }
    } else if flags & 4 != 0 {
        if flags & 2 != 0 {
            7
        } else {
            2
        }
    } else if flags & 2 != 0 {
        if flags & 8 != 0 {
            5
        } else {
            3
        }
    } else if flags & 8 != 0 {
        1
    } else {
        0
    }
}

/// One strategy try (`0x808870[strategy]` → `0x509220(dir)` + the axis-pair shift): scan the
/// claimed rects in seat order; against the FIRST strict overlap, translate the node edge-to-edge
/// past the blocker in the strategy's direction (0=UP 1=LEFT 2=RIGHT 3=DOWN, physical). No
/// overlap → the node comes back unchanged (the adopt signal).
fn push(claimed: &[Rect], r: Rect, strategy: u8) -> Rect {
    let Some(c) = claimed
        .iter()
        .find(|c| r.max.x > c.min.x && r.min.x < c.max.x && r.max.y > c.min.y && r.min.y < c.max.y)
    else {
        return r;
    };
    let mut r = r;
    match strategy {
        0 => {
            // UP: physical bottom (max.y) lands on the blocker's physical top (min.y).
            let d = r.max.y - c.min.y;
            r.min.y -= d;
            r.max.y -= d;
        }
        1 => {
            let d = r.max.x - c.min.x;
            r.min.x -= d;
            r.max.x -= d;
        }
        2 => {
            let d = c.max.x - r.min.x;
            r.min.x += d;
            r.max.x += d;
        }
        _ => {
            // DOWN: physical top (min.y) lands on the blocker's physical bottom (max.y).
            let d = c.max.y - r.min.y;
            r.min.y += d;
            r.max.y += d;
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    const VP: Vec2 = Vec2::new(1024.0, 768.0);

    fn seat(bucket: &mut SmartBucket, desired: Rect) -> Rect {
        let r = bucket.resolve(desired, VP);
        bucket.claim(r);
        r
    }

    /// **A plate riding the near-top band seats at the band edge, at EVERY approach height.**
    ///
    /// The seed clamp puts an offending edge on its band boundary and the per-node skip then
    /// re-tests that same rect against the same boundary with a strict compare, so the clamp has
    /// to land ON it, exactly. Translating by the gap instead lands one ULP below for some inputs;
    /// the seed then skips itself, the worklist empties, `solve` returns the UNCLAMPED input and
    /// the seat tail parks it half a plate from the screen top — 19 px above where the previous
    /// frame drew it, for one frame, and back.
    ///
    /// The walk is the live one that found it: a 73×18 plate at the probe's 640×360 viewport,
    /// approaching the band from above a tenth of a pixel at a time. `scr.y = 8.064` is the value
    /// that popped on the director's screen; the sweep is here so the next such value cannot.
    #[test]
    fn a_plate_in_the_near_top_band_never_pops_to_the_seat_clamp() {
        const VP2: Vec2 = Vec2::new(640.0, 360.0);
        let band = DDC_NEAR_TOP * VP2.length();
        for step in 0..2800 {
            #[allow(clippy::cast_precision_loss)]
            let top = step as f32 * 0.01;
            let desired = Rect::new(172.0, top, 245.0, top + 18.0);
            let got = SmartBucket::default().resolve(desired, VP2);
            let want = if top < band { band } else { top };
            assert!(
                (got.min.y - want).abs() < 1e-3,
                "a plate seeded at y={top} seats at {} — the band edge is {band} \
                 (a 19px pop means the seed skipped itself)",
                got.min.y
            );
        }
    }

    /// A free rect adopts unmoved on its first try — the every-frame fast path.
    #[test]
    fn free_rect_stays_put() {
        let mut b = SmartBucket::default();
        let r = Rect::new(500.0, 380.0, 540.0, 400.0);
        assert_eq!(seat(&mut b, r), r);
    }

    /// Center region: the second of two identical rects is pushed UP (Y-down: smaller y),
    /// landing edge-to-edge — bottom on the first one's top, zero gap.
    #[test]
    fn center_pushes_up_abutting() {
        let mut b = SmartBucket::default();
        let r = Rect::new(500.0, 380.0, 540.0, 400.0);
        seat(&mut b, r);
        let pushed = seat(&mut b, r);
        assert_eq!(pushed.max.y, 380.0, "bottom abuts the blocker's top");
        assert_eq!(pushed.min.y, 360.0, "size preserved");
        assert_eq!((pushed.min.x, pushed.max.x), (500.0, 540.0), "x untouched");
    }

    /// A third co-anchored rect goes SIDEWAYS, not twice-up: BFS explores the seed's children
    /// in region order (UP first), but the UP child still overlaps the second rect while the
    /// RIGHT child is free — fewest-pushes wins, the reference's "side, up, all sorts".
    #[test]
    fn third_goes_right_not_twice_up() {
        let mut b = SmartBucket::default();
        let r = Rect::new(500.0, 380.0, 540.0, 400.0);
        seat(&mut b, r);
        seat(&mut b, r);
        let third = seat(&mut b, r);
        assert_eq!(third.min.x, 540.0, "left edge abuts the blocker's right");
        assert_eq!((third.min.y, third.max.y), (380.0, 400.0), "one push only");
    }

    /// Near the top of the screen (region 2) UP is avoided: the first try is LEFT.
    #[test]
    fn near_top_pushes_left() {
        let mut b = SmartBucket::default();
        // Top band at 1024×768: 0.0375 × 1280 = 48 px — a rect whose top edge is above y=48.
        let r = Rect::new(500.0, 10.0, 540.0, 30.0);
        let first = seat(&mut b, r);
        let pushed = seat(&mut b, r);
        assert_eq!(
            pushed.max.x, first.min.x,
            "right edge abuts the blocker's left"
        );
        assert_eq!(
            (pushed.min.y, pushed.max.y),
            (first.min.y, first.max.y),
            "y untouched — never pushed further toward the top"
        );
    }

    /// The seed band-clamp (`0x509bf0` mode 0): a rect inside the near-top band is pulled down
    /// to the band edge before solving, and its region (from the PRE-clamp flags) still avoids UP.
    #[test]
    fn seed_clamps_to_band() {
        let b = SmartBucket::default();
        let r = Rect::new(500.0, 10.0, 540.0, 30.0);
        let seated = b.resolve(r, VP);
        assert_eq!(seated.min.y, 48.0, "top edge on the 0.0375·diag band");
    }

    /// Two overlapping full-screen rects exhaust the bound and the second returns at its anchor
    /// (unplaceable draws overlapping, never hides).
    #[test]
    fn exhaustion_keeps_original() {
        let mut b = SmartBucket::default();
        let r = Rect::new(0.0, 0.0, 1024.0, 768.0);
        seat(&mut b, r);
        // Every push lands fully off-screen; the off-screen node skip dries the worklist.
        let second = b.resolve(r, VP);
        assert_eq!(second.width(), 1024.0);
        assert_eq!(second.height(), 768.0);
    }

    /// The blocker-expiry snap-back: the solver is stateless across frames, so the same desire
    /// against an empty bucket lands at the anchor again.
    #[test]
    fn snaps_back_when_blocker_gone() {
        let mut b = SmartBucket::default();
        let r = Rect::new(500.0, 380.0, 540.0, 400.0);
        seat(&mut b, r);
        assert_ne!(seat(&mut b, r), r, "pushed while blocked");
        b.clear();
        assert_eq!(seat(&mut b, r), r, "back at the anchor next frame");
    }

    /// The full 16-word `0x509d80` tree — every combination, byte-derived (not just 9 rows, one
    /// per region).
    #[test]
    fn region_index_table() {
        let expected = [0, 4, 3, 4, 2, 8, 7, 8, 1, 6, 5, 6, 2, 8, 7, 8];
        for (flags, &region) in expected.iter().enumerate() {
            assert_eq!(region_index(flags as u8), region, "flags {flags:#06b}");
        }
    }

    /// Strict overlap: touching edges do NOT collide (zero padding).
    #[test]
    fn touching_edges_do_not_overlap() {
        let mut b = SmartBucket::default();
        seat(&mut b, Rect::new(500.0, 380.0, 540.0, 400.0));
        let below = Rect::new(500.0, 400.0, 540.0, 420.0);
        assert_eq!(seat(&mut b, below), below);
    }
}
