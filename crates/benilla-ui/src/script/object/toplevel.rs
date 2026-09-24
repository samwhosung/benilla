//! Frame method-table cluster: the `toplevel` flag and the **raise law** — `SetToplevel`/
//! `IsToplevel`, `Raise`/`Lower`, and the engine-side worker every raise trigger runs. Split out of
//! [`super`] beside [`super::movable`], whose flag word this shares.
//!
//! This is "bring the window I just opened to the front". 82 corpus addons declare `toplevel=` in
//! XML and eleven call `SetToplevel` from Lua; our own shipped windows declare it on thirteen
//! frames. Until this module it was parsed and thrown away with a warn-once, so a benilla dialog
//! opened behind another one stayed behind it.
//!
//! ## The law
//!
//! The flag is **`[frame+0xb4]` bit `0x1`**, written by the same pure bit-setter `0x76a3c0` as
//! `movable` (`0x100`) and `resizable` (`0x200`): `if (v) flags |= mask; else flags &= ~mask;`, no
//! calls, no list touch. XML `toplevel` lands there at `0x7698ec`; Lua `SetToplevel 0x775440` at
//! `0x7754d4`. **Setting the flag raises nothing** — it only marks the frame.
//!
//! The raise itself is `CSimpleTop::Raise(frame, force) 0x7650f0`, reached from
//! `CSimpleFrame::Raise 0x76a5b0` = `this->root->Raise(this, force = 1)`:
//!
//! 1. **Resolve to the nearest TOPLEVEL self-or-ancestor** up `+0x9c` (`0x7650fb..0x765135`).
//!    No toplevel in the chain ⇒ **total no-op**. So the gate lives in the worker, not the callers:
//!    every trigger may hand it an arbitrary frame, and the frame that visibly moves may be an
//!    *ancestor* of the one that was clicked or shown.
//! 2. **`force != 0` ⇒ recompute the OVERLAPPED bit** (`0x76513e..0x765290`): resolve the frame's
//!    pending layout, then scan its stratum bucket from **its own level upward** for a frame that is
//!    not itself, is not one of its descendants (`0x767010`), and whose screen rect
//!    intersects non-emptily. The answer is stored as `+0xb4` bit `0x10`.
//! 3. **The raise, gated on that bit** (`0x7651ac..0x7651e7`): level compaction (`0x764eb0`)
//!    squeezes the free levels out of the stratum — over **every live frame of it, hidden
//!    included** — then `set_frame_level(bucket->count, propagate = 1)`.
//!
//! Three consequences the tests pin: a raise is **`level := top occupied level + 1`**, not a list
//! reshuffle; it is **occlusion-gated** — `Raise()` on a frame that overlaps nothing changes nothing
//! at all; and it **never changes the stratum** (`0x7650f0` never writes `+0xc0`), so a LOW frame can
//! never be lifted over a MEDIUM one by any number of raises. `propagate = 1` shifts same-strata
//! children by the **same delta** (`0x76a4f0` @`0x76a58a`–`0x76a595`), so the raised subtree keeps
//! its internal order; cross-strata children are untouched.
//!
//! ## How that maps onto benilla's order model, clause by clause
//!
//! Our draw order is a derived total order over the arena ([`crate::order`]), not a materialized
//! array of per-stratum buckets of intrusive level lists. Every clause above still lands, but two
//! of them land as *derivations* and one is deliberately not stored:
//!
//! - **"the stratum bucket"** = the effective-visible frames whose `strata` matches. Bucket
//!   membership is exactly the client's link-gate: a frame is in a level list only while
//!   effectively visible, which is the same condition [`crate::widget::WidgetArena::resequence_to_tail`]
//!   already mirrors for the link stamp.
//! - **"raise within the strata"** = a **frame-level bump**, and it is the honest mapping rather
//!   than a re-stamp: the binary writes `+0xc4` through `set_frame_level 0x76a4f0`, and level is the
//!   term *above* the link stamp in our key, so a bump lifts the frame over every same-strata frame
//!   regardless of when each was last shown. The arithmetic is pinned in
//!   `tests/toplevel.rs::a_raise_is_top_occupied_level_plus_one_after_compaction`.
//! - **Level compaction** is [`crate::widget::WidgetArena::compact_levels`] — an order-preserving
//!   renumber of **every frame** in one stratum, hidden ones included: `0x764eb0` walks the client's
//!   master frame list filtering on strata and level alone, and never reads `+0xd4`
//!   (decision 2104). It changes no draw order by itself; it is
//!   what keeps `level := max + 1` from ratcheting upward for the length of a session. Renumbering
//!   only the *bucket* (the visible half) is the one shape it may not have: the raise's `propagate`
//!   delta is computed in the new numbering and applied to levels that are still in the old one, so
//!   a squeeze taken out of a visible child and not out of its hidden sibling inverts a pair that
//!   `parent.level + 1` made equal — see that method's own doc for the window it broke, and for the
//!   two places ours is deliberately simpler than the bytes.
//! - **The OVERLAPPED bit is computed, never stored** — and that is *exact*, not a shortcut. The bit
//!   is written only by `0x7650f0` and the per-tick pass `0x7657d0`, and read only by `0x7650f0`,
//!   which with `force = 1` recomputes it immediately before reading it. The stored value is
//!   therefore observable **only** through the one `force = 0` call site, `0x764a20`
//!   (bucket-invalidate), which rides an already-computed bit. benilla has no bucket-invalidate
//!   call — every trigger we have is a `force = 1` one — so the per-tick maintenance `0x7657d0`
//!   would be state nothing could read. A local `bool` per raise is bit-for-bit what `force = 1`
//!   does. **If a `force = 0` trigger is ever built, the bit has to become a field and `0x7657d0` a
//!   per-tick pass; there is no other clause of this law that is unmodelled.**
//!
//! ## The screen rects the gate reads
//!
//! Step 2 needs rects, which live on [`Model::resolved`], not on the arena. `0x7650f0` starts by
//! resolving the frame's *own* pending layout (`0x76513e`: `if (T->+0x60 & 1) …0x7680e0`) and then
//! reads every other frame's cached rect as-is; [`raise`] does the same thing with the one resolve
//! we have — [`crate::script::UiScript::resolve_layout`], whose epoch gate makes it a `u64` compare
//! on the overwhelmingly common quiet call (showing a frame writes no layout input, so a Show-driven
//! raise never pays a solve unless the caller moved something in the same tick). Our solve covers
//! hidden frames too, so a frame being shown for the first time already has a rect to test. A frame
//! with **no** resolved rect (anchorless, so not on screen) overlaps nothing and raises nothing.
//!
//! ## Triggers — the census, and which ones benilla wires
//!
//! A complete rel32 + absolute-dword census finds six call sites. Ours:
//!
//! | site | what it is | benilla |
//! |---|---|---|
//! | `0x775b0a` (Lua `Frame:Raise()`) | the explicit script call | [`install`] |
//! | `0x76aeec` in `0x76ae10` | **the Show trigger** | [`raise_on_show`], run from [`crate::script::event::fire_visibility_changes`] |
//! | `0x7652d7` in begin-move `0x7652b0` | drag start | [`super::movable`]'s `start_moving` |
//! | `0x766392` in the mouse-down handler `0x7662c0` | **the click trigger** | [`crate::script::UiScript::mouse_button`]'s down arm, first thing — capture-else-hover, unguarded, ahead of the title-region swallow |
//! | `0x764a4c` (`force = 0`) | re-applies an already-computed bit `0x10` | no counterpart (see the OVERLAPPED note above) |
//! | `0x7650d6` | dead — zero references binary-wide | — |
//!
//! **`Frame:Lower()` is inert in build 5875** and inert here: `0x775b20` → `0x76a5c0` → `0x7652a0`,
//! which is `xor eax,eax; ret 4` — a verified no-op stub whose frame argument is never read.

use mlua::{Lua, Table, Value};

use crate::layout::Rect;
use crate::script::{Model, UiScript};
use crate::widget::FrameHandle;

use super::frame_handle_of;

/// Populate `m`'s toplevel/raise methods (see the module doc).
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // SetToplevel(flag) / IsToplevel() — flag word bit 0x1 (`0x775440`/`0x76a3c0`). A pure flag
    // write; the raise is entirely separate. The argument is an **optional** boolean defaulting to
    // TRUE (`0x775440` marshals it that way) — unlike `SetMovable`'s, so this takes a `Value` and
    // reads absence as true rather than leaning on mlua's nil-is-false bool conversion.
    m.set(
        "SetToplevel",
        lua.create_function(|lua, (this, flag): (Table, Option<Value>)| {
            let on = match flag {
                None => true,
                Some(v) => !matches!(v, Value::Nil | Value::Boolean(false)),
            };
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model")
                .arena
                .set_toplevel(h, on);
            Ok(())
        })?,
    )?;
    m.set(
        "IsToplevel",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(model.arena.is_toplevel(h)))
        })?,
    )?;
    // Raise() — `0x775a50` → `0x76a5b0` → `0x7650f0(this, force = 1)`. Note it is legal on any
    // frame: the worker substitutes the nearest toplevel self-or-ancestor and does nothing at all
    // when there is none, so an addon calling it on a plain frame gets the reference's silence, not
    // an error.
    m.set(
        "Raise",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            raise(&mut model, h);
            Ok(())
        })?,
    )?;
    // Lower() — `0x775b20` → `0x76a5c0` → `0x7652a0` = `xor eax,eax; ret 4`. A VERIFIED no-op stub
    // in build 5875 (the frame argument is never read, and the call is direct, so this is
    // unconditionally what runs). Present because absence is a different behaviour — a nil-call
    // error where the reference silently does nothing.
    m.set("Lower", lua.create_function(|_, _this: Table| Ok(()))?)?;
    Ok(())
}

/// `CSimpleTop::Raise(frame, force = 1)` — `0x7650f0`, the worker every trigger runs. Returns
/// whether a toplevel self-or-ancestor was found at all (the binary's `bool` return); a `true` with
/// nothing moved is the occlusion gate declining, which is the common and correct case.
///
/// The three numbered steps of the module doc, in order.
pub(in crate::script) fn raise(model: &mut Model, frame: FrameHandle) -> bool {
    // 1. The nearest TOPLEVEL self-or-ancestor. None ⇒ total no-op.
    let Some(t) = nearest_toplevel(model, frame) else {
        return false;
    };
    // 2. force = 1: resolve the pending layout, then recompute OVERLAPPED (a local — see the
    //    module doc's note on why storing the bit would be unobservable state).
    //    Any `OnSizeChanged` that resolve produces is QUEUED, not fired: this path holds a bare
    //    `&mut Model` (no Lua reachable from a raise), so the host's next `UiScript::resolve` —
    //    which runs every frame — drains it one tick later. The alternative, threading a `&Lua`
    //    through the raise, would put a Lua handler in the middle of a strata compaction.
    UiScript::resolve_layout(model);
    if !overlapped(model, t) {
        return true;
    }
    // 3. The raise: compact the stratum's occupied levels, then land one above the top.
    let Some(strata) = model.arena.frame(t).map(|f| f.strata) else {
        return true;
    };
    let count = model.arena.compact_levels(strata);
    model.arena.set_frame_level(t, count, true);
    true
}

/// The Show trigger — `0x76ae10` @`0x76aee0`: on a genuine `+0xd4`
/// false→true transition, **test the frame's own toplevel bit** and raise if it is set.
///
/// The own-bit test is load-bearing and is why this is not just [`raise`]: the worker would walk up
/// to a toplevel *ancestor*, so calling it unconditionally would lift a whole window every time any
/// non-toplevel child of it became visible. The binary tests `[esi+0xb4], 0x1` first, and only then
/// calls `0x76a5b0`.
pub(in crate::script) fn raise_on_show(model: &mut Model, h: FrameHandle) {
    if model.arena.is_toplevel(h) {
        raise(model, h);
    }
}

/// Step 1 — walk `+0x9c` from the frame up, returning the first node with the toplevel bit
/// (`0x7650fb..0x765135`). Loop-guarded like the arena's own ancestry walk; a well-formed tree
/// never loops.
fn nearest_toplevel(model: &Model, frame: FrameHandle) -> Option<FrameHandle> {
    // The bound is read once, not per hop: a chain longer than the frame count is a cycle, which
    // `set_parent`'s own guard already makes unreachable — this is defence, not a hot path.
    let bound = model.arena.iter_frames().count();
    let mut cur = Some(frame);
    let mut guard = 0usize;
    while let Some(h) = cur {
        let f = model.arena.frame(h)?;
        if f.toplevel {
            return Some(h);
        }
        cur = f.parent;
        guard += 1;
        if guard > bound {
            break;
        }
    }
    None
}

/// Step 2's predicate — the scan `0x7650f0` runs inline and `0x765a90` runs standalone: within `t`'s
/// own stratum bucket, from **`t`'s own level upward**, is there a frame that is not `t`, is not one
/// of `t`'s descendants, and whose screen rect meets `t`'s?
///
/// Every clause matters, and each is a separate test:
/// - **`level >= t.level` only.** A toplevel frame that overlaps solely something *below* it is
///   already on top of it, so there is nothing to raise over.
/// - **Excludes descendants (`0x767010`).** A window overlaps its own children by construction;
///   without this every toplevel frame with content would raise on every trigger.
/// - **non-empty intersection.** Frames that merely share an edge do not overlap — the intersection
///   has to have positive width *and* height, so a zero-area touch is not an occlusion.
fn overlapped(model: &Model, t: FrameHandle) -> bool {
    let Some(tf) = model.arena.frame(t) else {
        return false;
    };
    let (strata, level) = (tf.strata, tf.level);
    let Some(t_rect) = model.resolved.get(&t).copied() else {
        // No resolved rect ⇒ the frame is not on screen ⇒ it occludes nothing.
        return false;
    };
    model.arena.iter_frames().any(|(h, f)| {
        h != t
            && f.effective_visible
            && f.strata == strata
            && f.level >= level
            && !model.arena.is_ancestor(t, h)
            && model
                .resolved
                .get(&h)
                .is_some_and(|r| rects_overlap(t_rect, *r))
    })
}

/// `rect_intersect_nonempty` (`0x768320` / `0x766c70` / `0x766bc0`) — reusing the one axis-aligned
/// intersection this crate already has and asking whether what comes out has area.
fn rects_overlap(a: Rect, b: Rect) -> bool {
    let i = crate::script::clip::intersect_rect(a, b);
    i.width() > 0.0 && i.height() > 0.0
}
