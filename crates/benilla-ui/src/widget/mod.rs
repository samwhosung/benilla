//! The widget object model — the frame arena and the show/hide/strata/level/scale/alpha
//! mutations, transcribed from the reference's binary-verified propagation cluster (decision 0068).
//!
//! This crate owns the *model*, not the runtime: the arena stores frames and their region leaves and
//! implements the propagation math, but it does **not** run Lua or fire `OnShow`/`OnHide` — the
//! mutations that cause visibility transitions *return the set of frames that changed* so the
//! Lua-embedding layer (the app) can fire the handlers in order.
//!
//! ## Ground truth (binary-verified against `WoW.exe`)
//!
//! - **Effective visibility** (show `0x76ae10` / hide `0x76ad50`): `effectiveVisible = shown AND
//!   (parent == null OR parent.effectiveVisible)`.
//!   A transition recurses into child frames — **a hidden mid-tree frame blocks its whole subtree** —
//!   and fires `OnShow`/`OnHide`. The recursion is *transition-gated*: a node whose effective
//!   visibility does not change stops the walk (its subtree cannot have changed).
//! - **Strata** (`set_frame_strata 0x76a470`): the **whole subtree is forced to one stratum** — the
//!   frame's strata is set, then every child frame is recursively set to the same value.
//! - **Level** (`set_frame_level 0x76a4f0`): clamp `>= 0`; `delta = new - old`; **same-strata**
//!   children shift by the same `delta` (relative offsets preserved); cross-strata children are
//!   untouched.
//! - **Effective scale** (`0x76ac90`): `effectiveScale = parentScale * ownScale`,
//!   written to `layoutScale`, **ε-gated** (skip the write + recursion if `|new - cur| < 2.384e-7`,
//!   the `_DAT_008029d4` constant — [`SCALE_EPS`]); recurse to child frames.
//! - **Alpha** (`SetAlpha 0x76a690`):
//!   an eager set-time **overwrite-cascade** — SetAlpha writes the raw value to the frame and
//!   recurses over its child frames pushing the SAME value (a flatten, like the strata subtree-
//!   force; never a draw-time ancestor product). At draw a region multiplies its own alpha by its
//!   OWNER frame's alpha only (one hop, `0x772180`/`0x77fac0`) — which is what `effective_alpha`
//!   feeds, so effective always equals own.
//! - **Named auto-publish**: a frame with a name is published to the global name table; a duplicate
//!   name does **not** overwrite the first (see [`WidgetArena::lookup`]).
//!
//! ## Alpha: the settled model (the old 1.12-vs-Era tension is resolved)
//!
//! An earlier INFERRED reading ("1.12 alpha is not tree-propagated") kept a dormant Era-style
//! propagation flag here as a hedge. `SetAlpha 0x76a690` settled it: 1.12
//! DOES reach the subtree, but by set-time overwrite (the same raw byte pushed to every
//! descendant frame's `+0xc8`), not by a live parent×child product — Era's `ignoreParentAlpha`
//! belongs to the later multiplicative model and has no 1.12 counterpart (there is no
//! `GetEffectiveAlpha` in 5875). The hedge machinery is gone; [`WidgetArena::set_alpha`] IS the
//! client's mechanism. Consequences worth knowing: a child's later own-SetAlpha diverges from its
//! parent until the parent is set again (last write wins), and a frame created under a dimmed
//! parent starts at 1.0 (creation does not re-push the parent's alpha). Scale keeps its own
//! (verified multiplicative) propagation and Era `ignore_parent_scale` opt-out, inert in 1.12.

use std::collections::HashMap;

use crate::order::{DrawLayer, Strata};

/// The scale-propagation ε — `_DAT_008029d4` = `0x34800000` ≈ 2.384e-7 (`0x76ac90`): a
/// recomputed effective scale within ε of the current one is skipped (no write, no recursion).
/// This is the *same* binary constant the layout resolver uses for its `OnSizeChanged`
/// gate, so we reuse it rather than restate the value.
pub const SCALE_EPS: f64 = crate::layout::SIZE_EPS;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Handles — generational, so a destroyed-then-reused slot never aliases a stale handle
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A generational handle to a [`Frame`] in a [`WidgetArena`]. Copyable and comparable; a handle to a
/// destroyed frame is detected (the slot's generation moved on) and reads return `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameHandle {
    index: u32,
    generation: u32,
}

/// A generational handle to a [`Region`] (a texture or fontstring leaf).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionHandle {
    index: u32,
    generation: u32,
}

impl RegionHandle {
    /// The handle's identity as one integer — index and generation packed — for the layout change
    /// gate's fingerprint (`script::layout::InputFingerprint`), which must notice a region being
    /// swapped for a different one at the same map size.
    #[inline]
    pub(crate) fn fingerprint_bits(self) -> u64 {
        (u64::from(self.generation) << 32) | u64::from(self.index)
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// A hand-rolled generational arena (no external deps — a slotmap in miniature)
// ─────────────────────────────────────────────────────────────────────────────────────────────

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// Minimal generational arena: `insert` reuses freed slots, `remove` bumps the slot's generation so
/// old handles stop resolving. Indices/generations are `u32` — plenty for a UI.
struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

impl<T> Arena<T> {
    fn new() -> Arena<T> {
        Arena {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    fn insert(&mut self, value: T) -> (u32, u32) {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.value = Some(value);
            (index, slot.generation)
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot {
                generation: 0,
                value: Some(value),
            });
            (index, 0)
        }
    }

    fn remove(&mut self, index: u32, generation: u32) -> Option<T> {
        let slot = self.slots.get_mut(index as usize)?;
        if slot.generation != generation {
            return None;
        }
        let taken = slot.value.take();
        if taken.is_some() {
            slot.generation = slot.generation.wrapping_add(1);
            self.free.push(index);
        }
        taken
    }

    fn get(&self, index: u32, generation: u32) -> Option<&T> {
        let slot = self.slots.get(index as usize)?;
        if slot.generation != generation {
            return None;
        }
        slot.value.as_ref()
    }

    fn get_mut(&mut self, index: u32, generation: u32) -> Option<&mut T> {
        let slot = self.slots.get_mut(index as usize)?;
        if slot.generation != generation {
            return None;
        }
        slot.value.as_mut()
    }

    /// Iterate live entries as `(index, generation, &value)`.
    fn iter(&self) -> impl Iterator<Item = (u32, u32, &T)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.value.as_ref().map(|v| (i as u32, s.generation, v)))
    }

    /// Iterate live entries as `(index, generation, &mut value)`.
    fn iter_mut(&mut self) -> impl Iterator<Item = (u32, u32, &mut T)> + '_ {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(i, s)| s.value.as_mut().map(|v| (i as u32, s.generation, v)))
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Widget kinds
// ─────────────────────────────────────────────────────────────────────────────────────────────

mod kinds;
pub use kinds::{
    model_key, slider_fraction, slider_grab, slider_set_value, ArmedSequence, ButtonFont,
    ButtonState, ButtonVisualState, ColorSelectState, EditAction, EditBoxState, EditOutcome,
    EditUnit, FrameKind, InsertMode, KindState, MessageFrameState, MessageLine, MinimapState,
    ModelFileFacts, ModelFog, ModelLight, ModelPlayHead, ModelState, RegionKind, ScrollFrameState,
    ScrollingMessageState, SequenceFacts, SliderState, StatusBarState, TooltipAnchor, TooltipState,
    MINIMAP_DEFAULT_ARROW_MODEL, MINIMAP_DEFAULT_MASK, MINIMAP_DEFAULT_PLAYER_MODEL,
    MINIMAP_DEFAULT_ZOOM, MINIMAP_ENGINE_CHILDREN, MINIMAP_ZOOM_LEVELS, TOOLTIP_DOUBLE_GAP,
    TOOLTIP_FADE_SECS, TOOLTIP_LINE_GAP, TOOLTIP_PAD, TOOLTIP_WRAP_WIDTH,
};

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Frame + Region nodes
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// A frame node in the arena — the modeled subset of `CSimpleFrame`.
///
/// The invariant-bearing fields (`effective_visible`, `effective_scale`, `effective_alpha`) and the
/// structural fields (`parent`, `children`, `regions`) are maintained by the [`WidgetArena`]
/// mutators; treat them as **read-only** and change state through
/// [`WidgetArena::set_shown`]/[`set_frame_strata`](WidgetArena::set_frame_strata)/etc. so propagation
/// stays correct. (Not to be confused with [`crate::layout::LayoutInput`], which is the *layout resolver's*
/// per-frame anchor/size input — a separate concern.)
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// The widget subtype.
    pub kind: FrameKind,
    /// Draw layers switched OFF on this frame — bit `n` = [`DrawLayer::index`] `n`.
    ///
    /// `Frame:EnableDrawLayer(layer)` / `DisableDrawLayer(layer)`, present on the Frame
    /// method table `0x878ec0` at `0x7755b0` / `0x775680`, sitting between `IsToplevel` and `Show`.
    /// (Read off the `{const char*, void*}` pair bytes; the same walk reproduces the Button table
    /// at `0x879d00` exactly.)
    ///
    /// A disabled layer hides every region the frame owns in it, and the region's own `Show`/`Hide`
    /// is untouched underneath — re-enabling the layer brings back exactly what was showing before,
    /// which is why this is a frame-level mask rather than a sweep over the regions. Both corpus
    /// consumers (pfUI, MoveAnything) use it to strip Blizzard artwork off a frame they are
    /// reskinning without disturbing what the owner does to its own regions.
    pub disabled_layers: u8,
    /// The frame's name (its global identifier), if any. See [`WidgetArena::lookup`] for the
    /// non-overwriting publish rule.
    pub name: Option<String>,
    /// The parent frame, or `None` for a top-level frame (attached to the screen root / `UIParent`).
    pub parent: Option<FrameHandle>,
    /// Child frames in **insertion order** (the client's `+0x300` child list order) — this order is
    /// the draw-order tiebreak within a `(strata, level)` bucket for equal-insertion children.
    pub children: Vec<FrameHandle>,
    /// Owned region leaves (textures/fontstrings), in creation order. A region detached by
    /// `SetParent(nil)` stays in this list so [`WidgetArena::destroy`] still frees it — see
    /// [`Region::detached`], which is what actually takes it out of the draw.
    pub regions: Vec<RegionHandle>,
    /// This frame's **title region** — the drag handle at `CSimpleFrame+0xA8`, or `None`.
    ///
    /// **One per frame**, which is what makes `CreateTitleRegion` idempotent: a second call returns
    /// the SAME object after clearing its anchors, so calling it on an XML-declared title region
    /// silently wipes them (`0x767ed0`→`0x767620`, ClearAllPoints). It is also in
    /// [`Frame::regions`], so the arena still frees it.
    pub title_region: Option<RegionHandle>,
    /// The draw stratum (`frameStrata +0xc0`, default MEDIUM).
    pub strata: Strata,
    /// The level within the stratum (`frameLevel +0xc4`, default 0).
    pub level: u16,
    /// This frame's own alpha, 0.0..=1.0 (`alpha +0xc8`, default 1.0).
    pub alpha: f32,
    /// The alpha a renderer should apply — always == [`Frame::alpha`] (see the module's alpha
    /// section: 1.12 cascades at set time, so there is no separate draw-time product). Kept as its
    /// propagated product (see the module tension note).
    pub effective_alpha: f32,
    /// `shown` bit (`shown +0xd0`, default true) — the frame's own Show/Hide state.
    pub shown: bool,
    /// `effectiveVisible +0xd4` — `shown AND parent-chain visible`. Maintained by propagation.
    pub effective_visible: bool,
    /// Whether the frame accepts the mouse (`EnableMouse`). Default **false** — WoW's `enableMouse`
    /// default; only mouse-enabled *and* effective-visible frames capture the cursor in
    /// [`crate::order::hit_test`]. (Keyboard focus / `EnableKeyboard` is a separate flag, out of
    /// scope in this crate for now.)
    pub mouse_enabled: bool,
    /// Whether the frame accepts the **wheel** (`EnableMouseWheel` / XML `enableMouseWheel`) — a
    /// flag of its own in the reference, and separate from [`Self::mouse_enabled`] there and here
    /// (decision 1198).
    ///
    /// Default **false**, WoW's own default. The wheel dispatch bubbles to the nearest *enabled*
    /// ancestor carrying an `OnMouseWheel`, which is how a scroll region hands the wheel to the
    /// window behind it while keeping its handler installed — a frame with a handler it has not
    /// enabled is deliberately transparent to the wheel.
    pub mouse_wheel_enabled: bool,
    /// Whether the frame is **keyboard-enabled** (`EnableKeyboard` `0x776f90` / XML
    /// `enableKeyboard`) — the kind-0/kind-1 bucket membership `0x76af00` writes as
    /// `[frame+0xcc] |= 1<<kind`.
    ///
    /// Default **false**, the client's own default. **This is the field key delivery gates on**:
    /// [`crate::script::keyboard`]'s walk builds its candidate set from
    /// `effective_visible && keyboard_enabled`, then orders it strata 8→0 / level high→low /
    /// oldest-registration-first (built 2026-08-14, decision 1319). `0x76af00` never touches the
    /// handler slots, so the flag and a handler are separable: `EnableKeyboard(true)` on a
    /// script-less frame "puts it in the walk where it is called and declines — transparent to
    /// everything downstream", so **being enabled is not being a handler**.
    pub keyboard_enabled: bool,
    /// Clamp-to-screen (`SetClampedToScreen` / XML `clampedToScreen` — the client's geometry
    /// flags **bit4**, applied inside rect assembly `0x767a20`): the layout
    /// resolve shifts this frame's assembled rect back inside the screen, size preserved.
    /// Default **true for GameTooltip frames**, false otherwise: the reference tooltip observably
    /// clamps (the minimap zone-text hover's ANCHOR_LEFT plate hangs down from the screen top
    /// instead of leaving the window) though its XML never sets the attribute — the class supplies
    /// the flag — and the rule here is absolute: no tooltip ever leaves the window (decision 0352).
    pub clamped_to_screen: bool,
    /// `SetHitRectInsets` / XML `<HitRectInsets>` — how far the frame's **mouse** rect is inset
    /// from its resolved rect, as `[left, right, top, bottom]` (default all 0, i.e. the hit rect
    /// IS the frame rect). Only [`crate::order::hit_test`] reads it: drawing, anchoring, and
    /// `GetLeft`/`GetWidth` all stay on the un-inset rect. The reference uses it wherever a
    /// button's art fills less than its frame — the micro buttons are 29×58 frames whose art
    /// occupies only the lower ~40 px, and a `top="18"` inset is what stops that empty header
    /// from swallowing hover and clicks aimed past the bar.
    pub hit_rect_insets: [f32; 4],
    /// `SetMovable` / XML `movable` — whether `StartMoving()` may drag this frame. Default
    /// **false**, and the flag is a *guard*, not a behaviour: nothing moves until a script calls
    /// `StartMoving`, and `StartMoving` without this bit set raises instead of moving
    /// ([`crate::script`]'s `object::movable` cluster).
    pub movable: bool,
    /// `SetResizable` / XML `resizable` — the sizing twin of [`Frame::movable`], guarding the
    /// `StartSizing` half of the same family. Default false. benilla stores and reports the flag;
    /// no resize *drag* is built (`StopMovingOrSizing` already covers the stop side of both).
    pub resizable: bool,
    /// `SetMinResize` / `SetMaxResize` — the interactive-resize bounds as `(width, height)`
    /// (`0x776020` / `0x7762a0`, with `GetMinResize 0x775f20` / `GetMaxResize 0x7761a0`; all four
    /// on the Frame method table `0x878ec0`, stored in the client at `CLayoutFrame+0x5c..+0x68`
    /// as a `CRect`-shaped `{minY, minX, maxY, maxX}`).
    ///
    /// **`0.0` is the client's own "unbounded" sentinel, on each field independently** (1505) — not a
    /// flag, not a negative, not a `None`: the `CLayoutFrame` ctor `0x767680` zeroes all four, the
    /// getters hand back two plain numbers (`0, 0` on a frame nobody bounded, never `nil`), and
    /// the clamp's first test on every axis is `bound == 0.0 → skip`. A **negative** bound is
    /// therefore live and clamps normally (`0x768710`).
    ///
    /// The values are lengths in the same space as the explicit width/height, so they compare
    /// directly against [`crate::layout::LayoutInput`]'s — the binding runs the byte-identical
    /// logical→internal transform `SetWidth`'s does.
    ///
    /// A window kit's opening lines are `SetResizable(true)` + these two; Quiver's
    /// `SideEffectMakeMoveable` calls `SetMinResize` on every module frame it builds and died on
    /// the nil method (part of B267).
    ///
    /// **Only the interactive drag reads them** — VERIFIED, not an omission: the setters store raw
    /// and do not even mark the layout dirty, `CLayoutFrame::SetWidth`/`SetHeight` never consult
    /// them, and no layout-resolve path does either. A frame sized 100 wide with
    /// `SetMinResize(400, 400)` stays 100 wide until the first drag tick snaps it into range.
    pub min_resize: (f32, f32),
    /// The upper twin of [`Frame::min_resize`] — see its doc, including the `0.0` sentinel.
    pub max_resize: (f32, f32),
    /// `SetUserPlaced` — the client's "the user placed this frame; persist its position across
    /// sessions" bit. Default false, readable through `IsUserPlaced`, and consumed by the layout
    /// cache ([`crate::script::layout_cache`]) — which is where persisting a frame's position
    /// belongs, rather than with the drag that moved it.
    ///
    /// **Necessary, not sufficient.** Every drag entry stamps this bit unconditionally
    /// (`0x7652b0` @`0x7652e5`), so an addon that drags a stock frame once stamps it too; the
    /// cache's own filter is this bit AND [`Frame::movable`]`|`[`Frame::resizable`], at both the
    /// write and the apply, which is what lets the stamp fall off again (decision 2193).
    pub user_placed: bool,
    /// `SetToplevel` / XML `toplevel` — flag word `[frame+0xb4]` **bit `0x1`**, the same word as
    /// [`Frame::movable`] (`0x100`) and [`Frame::resizable`] (`0x200`), written by the same pure
    /// bit-setter `0x76a3c0`. Default false.
    ///
    /// The flag only **marks**: setting it performs no raise (`SetToplevel 0x775440` is
    /// arg-marshal + `0x76a3c0` and returns). What reads it is the raise worker
    /// `CSimpleTop::Raise 0x7650f0`, which resolves the nearest toplevel *self-or-ancestor* and
    /// raises **that** — so a frame with this bit clear is never the subject of a raise, and a
    /// frame with it set is what a click/show anywhere in its subtree lifts. The law, the gate and
    /// the arithmetic live in [`crate::script::object::toplevel`].
    pub toplevel: bool,
    /// This frame's own scale (`ownScale +0xb8`, default 1.0).
    pub scale: f32,
    /// `layoutScale` = `parentEffective * ownScale`, ε-gated (`effective_scale 0x76ac90`).
    pub effective_scale: f32,
    /// Era `ignoreParentScale` opt-out — inert in 1.12 (no frame sets it; extension point).
    pub ignore_parent_scale: bool,
    /// The Lua-visible numeric id (`GetID`/`SetID`, XML `id=` — the client's `+0xb4`): an
    /// app-meaning-free label frames carry for their handlers (a dropdown row's list position, a
    /// tab index). Default 0. Distinct from the layout/wrapper id ([`Model`](crate::script::Model)'s
    /// bijection) — this one is data, not identity.
    pub wow_id: i64,
    /// The frame's **live link position** in its `(strata, level)` bucket — the client's intrusive
    /// list order, not a creation index. Seeded at creation and re-stamped to the bucket's tail by
    /// [`WidgetArena::resequence_to_tail`] on every relink the reference performs: becoming
    /// effectively visible, and a *changing* strata or level on an already-visible frame.
    ///
    /// It orders **two** sweeps that disagree about which end wins. Drawing takes the LATER-linked
    /// frame on top; the HIT sweep probes the EARLIER-linked frame first and stops there
    /// (decision 1816). See [`crate::order::hit_test`], and the note on [`WidgetArena`].
    pub insertion_seq: u32,
    /// Per-kind behavior state (StatusBar value model, …); [`KindState::None`] for plain kinds.
    pub kind_state: KindState,
    /// Next declaration index handed to a new region of this frame (region draw-order tiebreak).
    next_decl: u32,
}

/// A region leaf — a texture or fontstring belonging to one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    /// Texture or fontstring.
    pub kind: RegionKind,
    /// The frame this region belongs to.
    pub owner: FrameHandle,
    /// The draw layer within the owner (BACKGROUND..HIGHLIGHT).
    pub draw_layer: DrawLayer,
    /// `textureSubLevel` — ordering within the draw layer (extension point beyond 1.12; see
    /// [`crate::order`]).
    pub sub_level: i8,
    /// Declaration index within the owner frame — the final within-layer draw tiebreak.
    pub decl_seq: u32,
    /// `Region:SetParent(nil)` — the region is **orphaned and unrendered, not destroyed** (the
    /// re-link virtual `0x77fd10` with a null parent unlinks from the old parent's draw layer and
    /// region list and stores nothing in their place).
    ///
    /// Modelled as a flag rather than as an owner-less region: [`Frame::regions`] keeps the entry
    /// so [`WidgetArena::destroy`] still frees the slab slot, and
    /// [`crate::order::traversal`] skips it, so it emits no quad. A later
    /// `SetParent(frame)` clears it and re-links for real
    /// ([`WidgetArena::set_region_owner`]).
    pub detached: bool,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The arena
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// The frame arena: the store of all frames and regions, the name registry, and the propagation
/// mutators.
///
/// The **screen root** (`CSimpleTop` / `UIParent`) is *not* a node here — a frame with
/// `parent == None` is a top-level frame anchored to the always-visible, unit-scale screen root.
///
/// **Link-stamp / draw order.** Each frame gets a monotonic `insertion_seq` at creation, used as the
/// draw-order tiebreak **below the draw layer** within a `(strata, level)` bucket (decision 0884 —
/// the layer outranks the frame; see [`crate::order`]). Like the client, a frame is re-stamped to
/// its bucket's **tail** whenever it *becomes visible* (show `0x76ae10` re-ADDS to the intrusive
/// level list) or a visible frame changes strata/level (both setters remove-then-add). Show order
/// IS draw order within a bucket — how the reference's late-shown
/// `MiniMapTrackingFrame` draws over the earlier-declared `MinimapBackdrop` ring despite the XML
/// declaring it first (decision 0557; the mutators live in [`propagation`]).
///
/// The client's own list is a *head* insert walked tail→head, i.e. FIFO by link time with the
/// newest-linked frame emitted last — order-equivalent to this ascending counter, which is why the
/// counter stays (0884's §5 refuted the intuition that a head insert draws *first*). A same-value
/// `SetFrameLevel` early-outs before re-stamping, matching `0x76a4f0`'s `je` at `0x76a509`.
pub struct WidgetArena {
    frames: Arena<Frame>,
    regions: Arena<Region>,
    names: HashMap<String, FrameHandle>,
    next_insertion: u32,
    /// The draw list's fingerprint cache ([`crate::order::traversal`]).
    pub(crate) order_cache: crate::order::OrderCache,
    /// Monotonic count of Minimap-kind frames ever created — an O(1) "a new Minimap widget
    /// exists" signal, so the per-frame state feed (`set_minimap_inside`'s caller) re-pushes
    /// exactly when one appears instead of walking every frame to find out.
    minimap_created: u64,
    /// Frames whose kind carries **engine-side per-tick behavior** — ScrollingMessageFrame /
    /// MessageFrame (the line fades) and Cooldown (the flash-finished hide). The tick's registry
    /// (decision 1446, the `minimap_created` disposition at list size): the per-frame advance
    /// walks these few dozen instead of the whole arena, which it used to do TWICE per tick — a
    /// corpus UI is thousands of frames. Appended at creation (kinds never change after);
    /// `destroy` removes its own handle, so the list never carries dead entries.
    ticked_kinds: Vec<FrameHandle>,
    /// Every live **GameTooltip**, same registry shape as [`Self::ticked_kinds`] and for the same
    /// reason one layer over: three hot paths (the auto-size pre-pass at the top of every layout
    /// resolve, the per-tick fade advance, and the unit-push hook) all begin "find the tooltips",
    /// and all three did it by walking the resolve's whole frame roster — ~4000 entries at a
    /// corpus UI, to reach the two or three that exist. Appended at creation (kinds never change);
    /// `destroy` removes its own handle, so the list never carries dead entries.
    tooltip_kinds: Vec<FrameHandle>,
    /// Every live **Minimap**, same registry shape as [`Self::ticked_kinds`] and for the same
    /// reason: three state feeds (containment, the two zoom indices, and the player-arrow facing)
    /// all begin "find the Minimaps", and the facing is *per frame*. The ping's own lesson
    /// (decision 1596) was that a per-widget push must not walk the ~3–4k-frame arena every frame
    /// to reach the one widget that exists. Appended at creation (kinds never change); `destroy`
    /// removes its own handle.
    minimap_kinds: Vec<FrameHandle>,
}

impl Default for WidgetArena {
    fn default() -> WidgetArena {
        WidgetArena::new()
    }
}

/// Whether a widget of this kind is **mouse-enabled by its constructor**, needing neither an
/// `enableMouse` attribute nor an auto-enabling `<Scripts>` handler to be clickable.
///
/// One list, because there are two readers: this arena, and `benilla-app`'s `frame_flag_gate`
/// sweep, which compares our frames against a model of the reference's XML. That sweep used to
/// carry its own copy with a comment claiming it was "deliberately the SAME list" — it was not,
/// and it drifted silently the first time this one was edited. A wrong entry should be wrong in
/// one place.
///
/// **Read off the client's ctors.** `[frame+0xcc]` is a four-bit input mask — char, keyboard,
/// MOUSE, wheel — zeroed by the base `CSimpleFrame` ctor `0x769090` and set by exactly five
/// subclass ctors:
/// Button `0x778771`, CheckButton (through Button), Slider `0x789467`, ColorSelect `0x78b2b5`,
/// and EditBox `0x779ced`, which takes char+keyboard+mouse together because it must take a click
/// to focus. `Minimap` is an engine frame born `0x4` (`0x4edc38`), and ours is a `FrameKind`.
///
/// **`ScrollFrame` and `ScrollingMessageFrame` are NOT here, and that was the hard one.** They
/// were, on the reasoning that a scroll frame "takes the wheel in its ctor" — the wheel is a
/// different bit, and no ctor writes it at all. Against that stood a recorded observation:
/// `ChatFrame1` is a `ScrollingMessageFrame`, and something has to make a 1.12 chat link
/// clickable. The follow-up round settled it: nothing about the frame does. The engine synthesises
/// one mouse-enabled `CSimpleHyperlinkButton` child PER LINK SPAN (`0x7a3240`, born through the
/// `CSimpleButton` ctor) and routes the hyperlink scripts back to the parent. We hold spans as
/// rects rather than child frames, so `script::pointer`'s hit test carries the same law as a
/// disjunct: the chat window is a candidate over a link and inert everywhere else.
pub fn mouse_enabled_by_ctor(kind: FrameKind) -> bool {
    matches!(
        kind,
        FrameKind::Button
            | FrameKind::CheckButton
            | FrameKind::EditBox
            // `CGWorldFrame`'s ctor enables key + mouse + wheel (`0x481b09`/`0x481b14`/`0x481b1f`);
            // the hit is the world's, not the UI's (decision 1983).
            | FrameKind::WorldFrame
            // A Slider's thumb must be draggable: every scrollbar is a UIPanelScrollBarTemplate
            // Slider that declares no `enableMouse` yet is draggable in-game (decision 0250).
            | FrameKind::Slider
            | FrameKind::ColorSelect
            | FrameKind::Minimap
            // `LootButton` extends `CSimpleButton` and inherits its ctor's bit (decision 1799).
            | FrameKind::LootButton
    )
}

impl WidgetArena {
    /// A new, empty arena in **1.12 mode** (alpha does not propagate).
    pub fn new() -> WidgetArena {
        WidgetArena {
            frames: Arena::new(),
            regions: Arena::new(),
            names: HashMap::new(),
            next_insertion: 0,
            minimap_created: 0,
            order_cache: Default::default(),
            ticked_kinds: Vec::new(),
            tooltip_kinds: Vec::new(),
            minimap_kinds: Vec::new(),
        }
    }

    /// Monotonic count of Minimap widgets ever created (see the field note).
    pub fn minimap_created(&self) -> u64 {
        self.minimap_created
    }

    /// The frames whose kind the host must advance each tick (see the field note).
    pub fn ticked_kinds(&self) -> &[FrameHandle] {
        &self.ticked_kinds
    }

    /// The live GameTooltip frames (see the field note).
    pub fn tooltip_kinds(&self) -> &[FrameHandle] {
        &self.tooltip_kinds
    }

    /// The live Minimap frames (see the field note).
    pub fn minimap_kinds(&self) -> &[FrameHandle] {
        &self.minimap_kinds
    }

    // ── Read access ────────────────────────────────────────────────────────────────────────────

    /// Borrow a frame, or `None` if the handle is stale/unknown.
    pub fn frame(&self, h: FrameHandle) -> Option<&Frame> {
        self.frames.get(h.index, h.generation)
    }

    /// Mutably borrow a frame. Prefer the dedicated mutators for state that propagates
    /// (shown/strata/level/scale/alpha/parent) — direct edits bypass propagation.
    pub fn frame_mut(&mut self, h: FrameHandle) -> Option<&mut Frame> {
        self.frames.get_mut(h.index, h.generation)
    }

    /// Borrow a region, or `None` if the handle is stale/unknown.
    pub fn region(&self, h: RegionHandle) -> Option<&Region> {
        self.regions.get(h.index, h.generation)
    }

    /// Mutably borrow a region.
    pub fn region_mut(&mut self, h: RegionHandle) -> Option<&mut Region> {
        self.regions.get_mut(h.index, h.generation)
    }

    /// Iterate every live frame as `(handle, &Frame)` (arena order — [`crate::order::traversal`]
    /// sorts into draw order).
    pub fn iter_frames(&self) -> impl Iterator<Item = (FrameHandle, &Frame)> + '_ {
        self.frames
            .iter()
            .map(|(index, generation, f)| (FrameHandle { index, generation }, f))
    }

    /// Iterate every live frame as `(handle, &mut Frame)` — for per-kind state the host advances each
    /// tick (the ScrollingMessageFrame fade). Structural fields stay the arena's to mutate through its
    /// propagation setters; this is for [`Frame::kind_state`] only.
    pub fn iter_frames_mut(&mut self) -> impl Iterator<Item = (FrameHandle, &mut Frame)> + '_ {
        self.frames
            .iter_mut()
            .map(|(index, generation, f)| (FrameHandle { index, generation }, f))
    }

    /// Move `h` to the **tail** of its `(strata, level)` draw bucket — the client's own live-list
    /// maintenance: show `0x76ae10` re-ADDS a frame to its level's intrusive list on the
    /// hidden→visible transition, and `set_frame_strata 0x76a470` /
    /// `set_frame_level 0x76a4f0` remove-then-add a visible frame — an intrusive-list add appends.
    /// Called by the [`propagation`] mutators on exactly those transitions (module doc).
    pub(crate) fn resequence_to_tail(&mut self, h: FrameHandle) {
        // The seq lives in an 18-bit field of the packed `ZKey`; unbounded show-bumping could
        // exhaust it over a marathon session, so at the cap renumber every frame in its current
        // order (order-preserving, so no visible change) instead of tripping ZKey's assert.
        if self.next_insertion >= (1 << crate::order::INSERTION_BITS) {
            let mut order: Vec<FrameHandle> =
                self.iter_frames().map(|(handle, _)| handle).collect();
            order.sort_by_key(|&fh| self.frame(fh).map_or(0, |f| f.insertion_seq));
            for (i, fh) in order.iter().enumerate() {
                if let Some(f) = self.frame_mut(*fh) {
                    f.insertion_seq = i as u32;
                }
            }
            self.next_insertion = order.len() as u32;
        }
        let seq = self.next_insertion;
        if let Some(f) = self.frame_mut(h) {
            f.insertion_seq = seq;
            self.next_insertion += 1;
        }
    }

    /// Resolve a name to the frame that published it, or `None`. Publishing is **non-overwriting**:
    /// the first frame created with a given name owns it; a later duplicate keeps its own `name`
    /// field but does not become the lookup target (matching the client's auto-publish rule,
    /// `0x701bd0`; decision 0068).
    pub fn lookup(&self, name: &str) -> Option<FrameHandle> {
        self.names.get(name).copied()
    }

    // ── Create / destroy ────────────────────────────────────────────────────────────────────────

    /// Create a frame of `kind`, with an optional `name` and `parent`. A `parent` handle that is not
    /// live is treated as `None` (top-level). The new frame starts `shown`, strata MEDIUM, level 0,
    /// scale 1.0, alpha 1.0; its `effective_visible`/`effective_scale`/`effective_alpha` are computed
    /// from the parent immediately. If `name` is given and not already published, it is published.
    ///
    /// (Strata/level inheritance from the parent at create time is a *loader/CreateFrame* concern,
    /// not the ctor's — the client's ctor `0x769090` writes MEDIUM/0 unconditionally; we match that
    /// and leave inheritance to the layer above.)
    pub fn create(
        &mut self,
        kind: FrameKind,
        name: Option<String>,
        parent: Option<FrameHandle>,
    ) -> FrameHandle {
        // Only keep a parent that is actually live.
        let parent = parent.filter(|&p| self.frame(p).is_some());
        let parent_visible = parent.is_none_or(|p| self.frame(p).unwrap().effective_visible);
        let parent_scale = parent.map_or(1.0, |p| self.frame(p).unwrap().effective_scale);

        let insertion_seq = self.next_insertion;
        self.next_insertion += 1;

        let shown = true;
        let scale = 1.0f32;
        // Alpha starts at 1.0 regardless of the parent's — creation does not re-push a dimmed
        // parent's value (only SetAlpha cascades; module alpha section).
        let alpha = 1.0f32;
        let effective_alpha = alpha;

        let frame = Frame {
            disabled_layers: 0,
            kind,
            name: name.clone(),
            parent,
            children: Vec::new(),
            regions: Vec::new(),
            title_region: None,
            // The WorldFrame's constructor is the one writer of stratum 0 (`WORLD`, below
            // `BACKGROUND`); every other kind starts at the base ctor's MEDIUM (1984).
            strata: if kind == FrameKind::WorldFrame {
                Strata::World
            } else {
                Strata::default()
            },
            level: 0,
            alpha,
            effective_alpha,
            shown,
            effective_visible: shown && parent_visible,
            mouse_enabled: mouse_enabled_by_ctor(kind),
            // The two kinds whose ctor takes the WHEEL rather than the mouse generally — the same
            // by-construction argument as above, narrowed to the flag it is actually about
            // (decision 1198): a ScrollingMessageFrame and a ScrollFrame are wheel-interactive
            // with no attribute, and nothing else is.
            mouse_wheel_enabled: matches!(
                kind,
                FrameKind::ScrollingMessageFrame | FrameKind::ScrollFrame | FrameKind::WorldFrame
            ),
            // Nothing is keyboard-enabled by construction: `0x76af00` is only ever reached from the
            // XML attribute or an explicit call, never a ctor.
            keyboard_enabled: false,
            // A tooltip clamps to the screen by construction (its XML never sets the attribute,
            // yet the reference plate observably never leaves the window — the class supplies
            // geometry flags bit4; decision 0352's law: no tooltip off-screen, ever).
            clamped_to_screen: matches!(kind, FrameKind::GameTooltip),
            // No frame is born with an inset hit rect — `<HitRectInsets>`/SetHitRectInsets is the
            // only source (the client's ctor zeroes the four floats too).
            hit_rect_insets: [0.0; 4],
            // No kind is born movable/resizable/user-placed/toplevel: every one of the four is
            // opt-in from XML or a script, and a frame nobody opted in stays where its anchors put
            // it, at the level it was created with.
            movable: false,
            resizable: false,
            // All four zeroed, which IS the client's unbounded state — `CLayoutFrame`'s ctor
            // `0x767680` does exactly this (`xor esi,esi` into `+0x5c..+0x68`), and `0.0` is the
            // sentinel the clamp tests rather than a separate flag.
            min_resize: (0.0, 0.0),
            max_resize: (0.0, 0.0),
            user_placed: false,
            toplevel: false,
            scale,
            effective_scale: parent_scale * scale,
            ignore_parent_scale: false,
            wow_id: 0,
            insertion_seq,
            kind_state: match kind {
                FrameKind::StatusBar => KindState::StatusBar(StatusBarState::default()),
                // `CLootButton` and `CSimpleCheckbox` both extend `CSimpleButton` and both add
                // exactly one field, which `ButtonState` carries for them (`loot_slot`/`checked`).
                FrameKind::Button | FrameKind::CheckButton | FrameKind::LootButton => {
                    KindState::Button(ButtonState::default())
                }
                FrameKind::EditBox => KindState::EditBox(EditBoxState::default()),
                FrameKind::ScrollingMessageFrame => {
                    KindState::ScrollingMessage(kinds::ScrollingMessageState::default())
                }
                FrameKind::MessageFrame => KindState::Message(kinds::MessageFrameState::default()),
                FrameKind::ScrollFrame => KindState::Scroll(kinds::ScrollFrameState::default()),
                FrameKind::Slider => KindState::Slider(kinds::SliderState::default()),
                FrameKind::ColorSelect => {
                    KindState::ColorSelect(kinds::ColorSelectState::default())
                }
                // Both model panes share one state: `CGCharacterModelBase` EXTENDS `CSimpleModel`,
                // so a `<PlayerModel>` carries every `CSimpleModel` member and adds only the
                // turn-animation pair we do not model (`ModelState`'s doc).
                FrameKind::Model
                | FrameKind::PlayerModel
                | FrameKind::DressUpModel
                | FrameKind::TabardModel => KindState::Model(kinds::ModelState::default()),
                FrameKind::Minimap => KindState::Minimap(kinds::MinimapState::default()),
                FrameKind::GameTooltip => KindState::Tooltip(kinds::TooltipState::default()),
                _ => KindState::None,
            },
            next_decl: 0,
        };

        if matches!(kind, FrameKind::Minimap) {
            self.minimap_created += 1;
        }
        // The model panes ride the same registry: their scene clock advances per tick and their
        // completion edge is read there (decision 2007).
        let ticked = matches!(
            kind,
            FrameKind::ScrollingMessageFrame
                | FrameKind::MessageFrame
                | FrameKind::Model
                | FrameKind::PlayerModel
                | FrameKind::DressUpModel
                | FrameKind::TabardModel
        );
        let (index, generation) = self.frames.insert(frame);
        let handle = FrameHandle { index, generation };
        if ticked {
            self.ticked_kinds.push(handle);
        }
        if matches!(kind, FrameKind::GameTooltip) {
            self.tooltip_kinds.push(handle);
        }
        if let Some(p) = parent {
            self.frame_mut(p)
                .expect("live parent")
                .children
                .push(handle);
        }
        if let Some(n) = name {
            // Non-overwriting: first writer wins.
            self.names.entry(n).or_insert(handle);
        }
        if matches!(kind, FrameKind::EditBox) {
            self.build_editbox_engine_regions(handle);
        }
        if matches!(kind, FrameKind::Minimap) {
            self.minimap_kinds.push(handle);
            // The `CMinimap` ctor's own last act, so no observer ever sees a Minimap without its
            // nine (see [`MINIMAP_ENGINE_CHILDREN`]). It sits here rather than in the loader
            // because the client builds them in the *element factory* — `CreateFrame("Minimap")`
            // from Lua gets them too, and gets them before its OnLoad runs.
            self.build_minimap_engine_children(handle);
        }
        handle
    }

    /// Build the **five engine-owned regions of a fresh EditBox**, in the ctor's own order — the
    /// same posture as [`Self::build_minimap_engine_children`] below, and for the same reason: the
    /// client builds them in `CSimpleEditBox`'s ctor, so a `CreateFrame("EditBox")` with no XML
    /// behind it has them too, before its OnLoad runs.
    ///
    /// All five, byte-verified:
    ///
    /// | # | region | ctor site | built by |
    /// |---|---|---|---|
    /// | 1 | text FontString `E+0x328` | `0x779bee` | `0x770d30(E, 2, 1)` |
    /// | 2-4 | selection-highlight quads `E+0x350/0x354/0x358` | `0x779c41`–`0x779c72` | `0x76fc40(E, 2, 0)` |
    /// | 5 | caret `E+0x368` | `0x779c86`–`0x779cac` | `0x76fc40(E, 3, 1)` |
    ///
    /// **Why the ORDER is the whole point.** `GetRegions 0x773f60` walks `[frame+0x1b8]` — one flat
    /// creation-ordered list, oldest first, with **no filter** (hidden regions are returned and
    /// counted), and insertion is at the TAIL. All five reach
    /// that list through `0x77fd10`, the single linker. So on a real client the authored `<Layers>`
    /// regions start at index **6**, and an addon may index them there.
    ///
    /// pfUI's `skins/blizzard/friends.lua` l.379 is the report:
    /// `local _,_,_,_,_,left,right = GuildControlPopupFrameEditBox:GetRegions()` — it skips five and
    /// takes the box's two `UI-ChatInputBorder` textures. Before this, benilla returned only the
    /// authored regions, so `left` was nil and the whole guild-control skin died on it.
    ///
    /// The four quads are created blank — no texture, no fill — so they emit nothing; benilla's
    /// caret and selection highlight are still painted host-side. That gap is named on the
    /// [`EditBoxState`](kinds::EditBoxState) fields rather than papered over.
    fn build_editbox_engine_regions(&mut self, eb: FrameHandle) {
        // The text region keeps the OVERLAY/0 layer it has always shipped with; the ctor's own
        // 2/1 is recorded on the field and deferred there (a draw-order change to every EditBox
        // is not a rider on a region-list fix).
        let text = self.create_region(eb, RegionKind::FontString, DrawLayer::Overlay, 0);
        let mut selection = [None; 3];
        for slot in &mut selection {
            *slot = self.create_region(eb, RegionKind::Texture, DrawLayer::Artwork, 0);
        }
        let caret = self.create_region(eb, RegionKind::Texture, DrawLayer::Overlay, 1);
        if let Some(KindState::EditBox(s)) = self.frame_mut(eb).map(|f| &mut f.kind_state) {
            s.text_region = text;
            s.selection_regions = selection;
            s.caret_region = caret;
        }
    }

    /// Build the nine engine-owned `Model` children of a fresh Minimap and remember the ninth as
    /// its player arrow. They are created **anonymous and file-less**: the ctor only constructs
    /// them, and the model paths are `CMinimap::LoadXML 0x4ee2b0`'s to assign from the two
    /// attributes (the loader's minimap pass) — which is why a `CreateFrame("Minimap")` with no XML
    /// behind it has nine blank Models here, exactly as the reference does.
    fn build_minimap_engine_children(&mut self, minimap: FrameHandle) {
        let mut ninth = None;
        for _ in 0..kinds::MINIMAP_ENGINE_CHILDREN {
            ninth = Some(self.create(FrameKind::Model, None, Some(minimap)));
        }
        if let Some(KindState::Minimap(m)) = self.frame_mut(minimap).map(|f| &mut f.kind_state) {
            m.player_arrow = ninth;
        }
    }

    /// Destroy a frame and its whole subtree (child frames and all regions), unpublishing any names
    /// it owns and detaching it from its parent's child list. Stale handles are ignored.
    pub fn destroy(&mut self, h: FrameHandle) {
        let Some(frame) = self.frame(h) else {
            return;
        };
        let children = frame.children.clone();
        let regions = frame.regions.clone();
        let parent = frame.parent;
        let name = frame.name.clone();
        // Each (recursive) destroy removes its own handle — the tick registry stays dead-free.
        self.ticked_kinds.retain(|&t| t != h);
        self.tooltip_kinds.retain(|&t| t != h);
        self.minimap_kinds.retain(|&t| t != h);

        for c in children {
            self.destroy(c);
        }
        for r in regions {
            self.regions.remove(r.index, r.generation);
        }
        if let Some(p) = parent {
            if let Some(pf) = self.frame_mut(p) {
                pf.children.retain(|&c| c != h);
            }
        }
        if let Some(n) = name {
            if self.names.get(&n) == Some(&h) {
                self.names.remove(&n);
            }
        }
        self.frames.remove(h.index, h.generation);
    }

    /// Create a region leaf owned by `owner`. Returns `None` if `owner` is stale.
    pub fn create_region(
        &mut self,
        owner: FrameHandle,
        kind: RegionKind,
        draw_layer: DrawLayer,
        sub_level: i8,
    ) -> Option<RegionHandle> {
        let decl_seq = {
            let f = self.frame_mut(owner)?;
            let d = f.next_decl;
            f.next_decl += 1;
            d
        };
        let (index, generation) = self.regions.insert(Region {
            kind,
            owner,
            draw_layer,
            sub_level,
            decl_seq,
            detached: false,
        });
        let handle = RegionHandle { index, generation };
        self.frame_mut(owner)
            .expect("live owner")
            .regions
            .push(handle);
        Some(handle)
    }

    /// **Free one region leaf** — unlink it from its owner's region list and release the slab slot,
    /// so the handle stops resolving and the region stops drawing. Returns whether it was live.
    ///
    /// There is no `Region:Destroy` in the widget API and none is wanted: this exists for the one
    /// engine-owned lifetime in the client, `CSimpleHTML::SetText`, which pool-frees the blocks of
    /// the previous parse before building the new ones (the pool allocate at `0x78adc6` has a
    /// matching free, and the CONTENTNODE list at `+0x340` is what names the objects to free).
    /// Without it a second `SetText` leaves the first parse's
    /// FontStrings on screen, stacked behind the new ones.
    ///
    /// **The caller owns the rest of the region's identity.** The arena holds only structure;
    /// paint (`RegionData`), the stable id maps, the resolved-rect cache and any name publish live
    /// on the script model, and a caller that frees a region without dropping those leaves a
    /// resolvable id pointing at a dead handle. [`crate::script`]'s SimpleHTML block teardown is
    /// the one caller and does the whole set. For the same reason this must never be pointed at a
    /// region some *state* still holds by handle (a button's texture slot, a tooltip line, an
    /// EditBox's text region): freeing those would leave that state dangling.
    pub fn destroy_region(&mut self, h: RegionHandle) -> bool {
        let Some(region) = self.region(h) else {
            return false;
        };
        let owner = region.owner;
        if let Some(f) = self.frame_mut(owner) {
            f.regions.retain(|&r| r != h);
            if f.title_region == Some(h) {
                f.title_region = None;
            }
        }
        self.regions.remove(h.index, h.generation).is_some()
    }
}

// The propagation mutators (visibility/strata/level/scale/alpha/reparenting) — split out for size;
// see `propagation`'s module doc.
mod propagation;

#[cfg(test)]
mod tests;
