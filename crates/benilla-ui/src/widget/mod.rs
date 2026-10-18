//! The widget object model: the frame arena, its regions, and the reference's propagation of
//! show, strata, level, scale and alpha down the frame tree. It runs no Lua: a mutation that
//! moves visibility returns the frames that changed, for the caller to fire `OnShow`/`OnHide`.

use std::collections::HashMap;

use crate::order::{DrawLayer, Strata};

/// The effective-scale epsilon of `0x76ac90`: the client's constant `0x8029d4` (`0x34800000`,
/// about 2.384e-7), the same one the layout's `OnSizeChanged` gate uses.
pub const SCALE_EPS: f64 = crate::layout::SIZE_EPS;

// ── Handles ──────────────────────────────────────────────────────────────────────────────────

/// A generational handle to a [`Frame`]: it stops resolving once the frame is destroyed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameHandle {
    index: u32,
    generation: u32,
}

impl FrameHandle {
    /// Index and generation in one integer, for a deterministic order over a hashed set.
    #[inline]
    pub(crate) fn fingerprint_bits(self) -> u64 {
        (u64::from(self.generation) << 32) | u64::from(self.index)
    }
}

/// A generational handle to a [`Region`] (a texture or fontstring leaf).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionHandle {
    index: u32,
    generation: u32,
}

impl RegionHandle {
    /// Index and generation in one integer: the layout gate's fingerprint must see a region swap.
    #[inline]
    pub(crate) fn fingerprint_bits(self) -> u64 {
        (u64::from(self.generation) << 32) | u64::from(self.index)
    }
}

// ── The generational arena ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// `remove` bumps the slot's generation, so an old handle stops resolving.
#[derive(Clone)]
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

    fn iter(&self) -> impl Iterator<Item = (u32, u32, &T)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.value.as_ref().map(|v| (i as u32, s.generation, v)))
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = (u32, u32, &mut T)> + '_ {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(i, s)| s.value.as_mut().map(|v| (i as u32, s.generation, v)))
    }
}

// ── Widget kinds ─────────────────────────────────────────────────────────────────────────────

mod kinds;
pub(crate) use kinds::ButtonFonts;
pub use kinds::{
    minimap_radius_yd, model_key, pane_clock_step_ms, slider_fraction, slider_grab,
    slider_set_value, ArmedSequence, ButtonState, ButtonVisualState, CharacterPane,
    ColorSelectState, CopyCamera, EditAction, EditBoxState, EditOutcome, EditUnit, FrameKind,
    InsertMode, KindState, KindUpdate, MessageFrameState, MessageLine, MinimapArt, MinimapArtSlot,
    MinimapState, MinimapStatics, ModelFileFacts, ModelFog, ModelLight, ModelPlayHead, ModelState,
    PaneArm, PaneCopy, RegionKind, ScrollFrameState, ScrollingMessageState, SequenceFacts,
    ShownWrites, SliderState, StatusBarState, TooltipAnchor, TooltipState, Viewport,
    ANIM_SHUFFLE_LEFT, ANIM_SHUFFLE_RIGHT, ANIM_STAND, MINIMAP_ARROW_SCALE,
    MINIMAP_DEFAULT_ARROW_MODEL, MINIMAP_DEFAULT_MASK, MINIMAP_DEFAULT_PLAYER_MODEL,
    MINIMAP_DEFAULT_ZOOM, MINIMAP_DISC, MINIMAP_ENGINE_CHILDREN, MINIMAP_GROUP_ARROWS,
    MINIMAP_INDOOR_RADII, MINIMAP_RIM_ARROWS, MINIMAP_ZOOM_CHUNKS, MINIMAP_ZOOM_LEVELS,
    TOOLTIP_COLUMN_GAP, TOOLTIP_FADE_SECS, TOOLTIP_LINE_GAP, TOOLTIP_PLATE_PAD, TOOLTIP_WRAP_WIDTH,
    TURN_HOLD_MS,
};

// ── Frame and Region nodes ───────────────────────────────────────────────────────────────────

/// A frame node, the modelled subset of `CSimpleFrame`. The `effective_*` fields and the tree
/// links belong to the [`WidgetArena`] mutators: change state through them so it propagates.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub kind: FrameKind,
    /// Layers switched off by `DisableDrawLayer` (`0x775680`) and back on by `EnableDrawLayer`
    /// (`0x7755b0`), bit `n` for [`DrawLayer::index`] `n`: a frame-level mask, so each region
    /// keeps its own shown state underneath.
    pub disabled_layers: u8,
    /// The global name, which a later duplicate carries without owning ([`WidgetArena::lookup`]).
    pub name: Option<String>,
    pub parent: Option<FrameHandle>,
    /// Child frames in link order, oldest first (the client's child list at `+0x300`).
    pub children: Vec<FrameHandle>,
    /// Owned regions in link order, a detached one included so `destroy` frees it.
    pub regions: Vec<RegionHandle>,
    /// The title region (`CSimpleFrame+0xA8`), also in `regions`. One per frame: a second
    /// `CreateTitleRegion` returns it with its anchors cleared (`0x767ed0` → `0x767620`).
    pub title_region: Option<RegionHandle>,
    /// The draw stratum (`frameStrata +0xc0`, default MEDIUM).
    pub strata: Strata,
    /// The level within the stratum (`frameLevel +0xc4`, default 0).
    pub level: u16,
    /// This frame's alpha byte (`+0xc8`, default 255). It cascades at set time
    /// ([`WidgetArena::set_alpha`]), and at draw a region multiplies only its owner frame's
    /// (`0x772180`/`0x77fac0`).
    pub alpha: u8,
    /// The frame's own Show/Hide bit (`+0xd0`, default true).
    pub shown: bool,
    /// `shown` and every ancestor effectively visible (`+0xd4`).
    pub effective_visible: bool,
    /// The frame's own load window (`+0x114`): set by the pre-load hook (`0x7697c2`), cleared as
    /// the post-load hook starts (`0x76a2ff`). While it is set the frame's own OnShow and OnHide
    /// fire nothing (`0x76b270`/`0x76b2a0`); it still transitions and walks its children.
    pub load_window: bool,
    /// The layout object's deferral flag (`0x2` of `G+0x3c`, written only by `0x768060`): set by a
    /// hide (`0x76ad70`) and a load's start (`0x7697cc`), cleared by a show (`0x76ae58`), a load's
    /// end while visible (`0x76a310`) and an anchor read (`0x7a2fc0`). While it is set the frame's
    /// resolve waits; [`crate::script::Model`] keeps what it owes.
    pub layout_deferred: bool,
    /// `EnableMouse`: the mouse bit (bit 2) of the input mask `[frame+0xcc]`.
    pub mouse_enabled: bool,
    /// `EnableMouseWheel`, or an `<OnMouseWheel>` script at XML load (`0x76a044`): the mask's
    /// wheel bit (3), membership in the wheel walk. A frame with the script but not the bit is
    /// transparent to the wheel. Born set only for the WorldFrame (`0x481b1f`); `EnableMouseWheel`
    /// and an XML `<OnMouseWheel>` set it later.
    pub mouse_wheel_enabled: bool,
    /// The input mask's char bit (0): membership in the char walk ([`crate::script::keyboard`]),
    /// `0x76af00(0)`. `EnableKeyboard` (`0x776ec0`), XML `enableKeyboard` or a key script at XML
    /// load set it with [`Self::key_enabled`]; the EditBox ctor sets it (`0x779ced`); a script-less
    /// frame in the walk declines.
    pub char_enabled: bool,
    /// The mask's key bit (1): membership in the key-down walk, `0x76af00(1)`. The EditBox ctor
    /// (`0x779cf8`) and the WorldFrame's (`0x481b09`, which has no char bit) set it.
    pub key_enabled: bool,
    /// When the frame last joined the char bucket (`0x764aa0`, kind 0): at its link (creation
    /// visible, show, a strata or level change) or, on a linked frame, at the bit's own enable
    /// (`0x76af00`). Equal-level frames walk in this order, oldest first. A [`WidgetArena`]
    /// counter, apart from [`Self::insertion_seq`], whose draw order an enable does not touch.
    pub char_stamp: u64,
    /// The same for the key bucket (kind 1).
    pub key_stamp: u64,
    /// The same for the wheel bucket (kind 3): at the link or, on a linked frame, at the bit's
    /// own enable (`0x76af00`).
    pub wheel_stamp: u64,
    /// `SetClampedToScreen`, geometry flags `0x10`: `0x767a20` shifts the rect back on screen.
    /// Every GameTooltip is born with it here; the stock tooltips get it from their template,
    /// `GameTooltipTemplate.xml:3`.
    pub clamped_to_screen: bool,
    /// `SetHitRectInsets` or `<HitRectInsets>`: the mouse rect's `[left, right, top, bottom]`
    /// inset. Only the hit test reads it; drawing, anchoring and `GetLeft`/`GetWidth` do not.
    pub hit_rect_insets: [f32; 4],
    /// `SetMovable`: only a guard, as `StartMoving` errors without it.
    pub movable: bool,
    /// `SetResizable`: the guard `StartSizing` checks.
    pub resizable: bool,
    /// `SetMinResize` (`0x776020`; `GetMinResize` `0x775f20`): the resize drag's floor, `(width,
    /// height)` in `SetWidth` units. `0.0` is unbounded per axis (ctor `0x767680`); a negative
    /// bound clamps (`0x768710`). Only the drag reads it, never `SetWidth` or the layout.
    pub min_resize: (f32, f32),
    /// `SetMaxResize` (`0x7762a0`; `GetMaxResize` `0x7761a0`): the ceiling, same rules.
    pub max_resize: (f32, f32),
    /// `SetUserPlaced`: the layout cache keeps this frame's position. Every drag (`0x7652b0`)
    /// stamps it (`0x7652e5`), so the cache also requires `movable` or `resizable`, at write and
    /// at apply.
    pub user_placed: bool,
    /// `SetToplevel` (`0x775440`): bit `0x1` of the flag word `[frame+0xb4]` (`movable` `0x100`,
    /// `resizable` `0x200`, all set by `0x76a3c0`). It raises nothing; `CSimpleTop::Raise`
    /// (`0x7650f0`) raises the nearest toplevel self-or-ancestor of a clicked or shown frame.
    pub toplevel: bool,
    /// Flag `0x2000` of `[frame+0xb4]`: the frame is in a scroll child's subtree, so the flat draw
    /// pass skips it (`0x7659df`) and its ScrollFrame draws it; the hit test clips it to its first
    /// ancestor without the flag (`0x76b034`). [`WidgetArena::set_scroll_flags`] writes it.
    pub scroll_member: bool,
    /// Flag `0x4000`: the frame is a scroll child itself, so a recursive draw that reaches it
    /// through its parent skips it (`0x76b509`): its ScrollFrame draws it.
    pub scroll_root: bool,
    /// This frame's own scale (`ownScale +0xb8`, default 1.0).
    pub scale: f32,
    /// `layoutScale` = `parentEffective * ownScale`, ε-gated (`0x76ac90`).
    pub effective_scale: f32,
    /// A later client's `ignoreParentScale`, not 1.12: nothing sets it.
    pub ignore_parent_scale: bool,
    /// `GetID`/`SetID` and XML `id`, at `+0xb0` (`0x77531f`, `0x7753e7`): data for handlers, such
    /// as a tab index, not the script model's frame id. Default 0.
    pub wow_id: i64,
    /// The link position in the `(strata, level)` bucket, re-stamped at the tail on each relink.
    /// Drawing puts the later-linked frame on top; the hit test probes the earlier-linked first.
    pub insertion_seq: u32,
    /// The place on the list of every live frame (`root+0xcc4`), which the constructor appends to
    /// (`0x76930d` → `0x7646a0`): creation order, which arena order is not once a slot is reused.
    created: u64,
    pub kind_state: KindState,
    next_decl: u32,
}

impl Frame {
    /// Its place on the list of every live frame (`root+0xcc4`), creation order.
    pub(crate) fn created(&self) -> u64 {
        self.created
    }
}

/// A region leaf: a texture or font string owned by one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub kind: RegionKind,
    pub owner: FrameHandle,
    pub draw_layer: DrawLayer,
    /// The engine's own order within a layer, which only the EditBox caret sets, to draw over the
    /// text; 1.12 gives Lua and XML no sub-level (`0x76a860`, `0x79a780`).
    pub sub_level: i8,
    /// The index within the owner frame, the last within-layer draw tiebreak.
    pub decl_seq: u32,
    /// `Region:SetParent(nil)`: unlinked and not drawn, but not destroyed (`0x77fd10` with a null
    /// parent). The owner's `regions` keeps it so `destroy` frees it; `SetParent(frame)` relinks.
    pub detached: bool,
    /// The region's own name, the expanded string `SetName` keeps at `[+0x98]` (`0x76c650`); `None`
    /// while anonymous. Only the `_G` entry is first-come (`0x701bd0`), so two regions can hold
    /// one name, and `GetName` answers each its own.
    pub name: Option<String>,
    /// The layout object's deferral bit, as [`Frame::layout_deferred`]: set when the region stops
    /// drawing (`0x77fc82`) and at its owner's load (`0x769807`), cleared when it starts
    /// (`0x77fce6`) and by an anchor read.
    pub layout_deferred: bool,
}

/// A cursor over a frame's child list for [`WidgetArena::next_child`]: the child last returned and
/// where it stood, so a walk that a handler interrupts resumes from the list as it is now.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChildWalk {
    at: usize,
    last: Option<FrameHandle>,
}

// ── The arena ────────────────────────────────────────────────────────────────────────────────

/// The frame arena: frames, regions, the name registry and the propagation mutators. The screen
/// root is not a node: a frame with no parent hangs off it. Within a bucket the client's list is a
/// head insert walked tail to head, the same order as the ascending [`Frame::insertion_seq`].
#[derive(Clone)]
pub struct WidgetArena {
    frames: Arena<Frame>,
    regions: Arena<Region>,
    names: HashMap<String, FrameHandle>,
    next_insertion: u32,
    /// The next [`Frame::char_stamp`] / [`Frame::key_stamp`] / [`Frame::wheel_stamp`].
    next_kb_stamp: u64,
    /// Each stratum's level bound, the reference's `bucket+0x8`: one above the highest level a
    /// visible frame has linked at since the last compaction set it. It starts at 0 and only a
    /// visible link grows it ([`Self::link_bound`]); nothing else lowers it but the compaction.
    level_bound: [u32; Strata::ALL.len()],
    /// The next [`Frame::created`].
    next_created: u64,
    /// How many times the link stamps were renumbered at the cap, so a reader holding stamps can
    /// tell they are still comparable with the live ones.
    renumbers: u32,
    /// While the update walk runs, every frame linked into a bucket since it opened, in link order
    /// ([`Self::open_link_log`]).
    link_log: Option<Vec<FrameHandle>>,
    /// The draw list's fingerprint cache ([`crate::order::traversal`]).
    pub(crate) order_cache: crate::order::OrderCache,
    /// The live model panes, so the host's passes over them (the paint list, the implicit rects)
    /// skip the rest of the arena. Added at creation, as kinds never change; `destroy` removes its
    /// own.
    model_kinds: Vec<FrameHandle>,
    /// The live frames whose kind has a [`KindUpdate`], kept the same way: the update walk reaches
    /// each one shown, with an `OnUpdate` or without.
    walked_kinds: Vec<FrameHandle>,
    /// The live GameTooltips, kept the same way, for the layout's cursor pre-pass, the headless
    /// sizing and the unit push.
    tooltip_kinds: Vec<FrameHandle>,
    /// The live Minimaps, kept the same way, for the containment, zoom and arrow-facing feeds.
    minimap_kinds: Vec<FrameHandle>,
    /// The live EditBoxes, kept the same way, for the resize re-seat (`reseat_resized`).
    editbox_kinds: Vec<FrameHandle>,
}

impl Default for WidgetArena {
    fn default() -> WidgetArena {
        WidgetArena::new()
    }
}

/// Whether this kind's constructor enables the mouse, so it takes clicks with no `enableMouse`
/// and no mouse script: the base ctor `0x769090` zeroes the input mask `[frame+0xcc]`, and Button
/// (`0x778771`, CheckButton through it), Slider (`0x789467`), ColorSelect (`0x78b2b5`), EditBox
/// (`0x779ced`..`0x779d03`, with char and key) and Minimap (`0x4edc38`) set its mouse bit. The
/// ScrollFrame and ScrollingMessageFrame ctors set none; a chat link takes clicks through a
/// mouse-enabled Button child per span (`0x7a3240`), which `script::hyperlink` builds.
pub fn mouse_enabled_by_ctor(kind: FrameKind) -> bool {
    matches!(
        kind,
        FrameKind::Button
            | FrameKind::CheckButton
            | FrameKind::EditBox
            // `CGWorldFrame`'s ctor enables key, mouse, wheel (`0x481b09`/`0x481b14`/`0x481b1f`).
            | FrameKind::WorldFrame
            | FrameKind::Slider
            | FrameKind::ColorSelect
            | FrameKind::Minimap
            // `LootButton` extends `CSimpleButton` and inherits its ctor's bit.
            | FrameKind::LootButton
    )
}

impl WidgetArena {
    pub fn new() -> WidgetArena {
        WidgetArena {
            frames: Arena::new(),
            regions: Arena::new(),
            names: HashMap::new(),
            next_insertion: 0,
            next_kb_stamp: 0,
            level_bound: [0; Strata::ALL.len()],
            next_created: 0,
            renumbers: 0,
            link_log: None,
            order_cache: Default::default(),
            model_kinds: Vec::new(),
            walked_kinds: Vec::new(),
            tooltip_kinds: Vec::new(),
            minimap_kinds: Vec::new(),
            editbox_kinds: Vec::new(),
        }
    }

    /// The live model panes.
    pub fn model_kinds(&self) -> &[FrameHandle] {
        &self.model_kinds
    }

    /// The frames whose kind has an update of its own in the walk ([`FrameKind::kind_update`]).
    pub fn walked_kinds(&self) -> &[FrameHandle] {
        &self.walked_kinds
    }

    pub fn tooltip_kinds(&self) -> &[FrameHandle] {
        &self.tooltip_kinds
    }

    pub fn minimap_kinds(&self) -> &[FrameHandle] {
        &self.minimap_kinds
    }

    pub fn editbox_kinds(&self) -> &[FrameHandle] {
        &self.editbox_kinds
    }

    // ── Read access ────────────────────────────────────────────────────────────────────────────

    pub fn frame(&self, h: FrameHandle) -> Option<&Frame> {
        self.frames.get(h.index, h.generation)
    }

    /// Direct edits skip propagation: shown, strata, level, scale, alpha and parent go through
    /// the mutators.
    pub fn frame_mut(&mut self, h: FrameHandle) -> Option<&mut Frame> {
        self.frames.get_mut(h.index, h.generation)
    }

    pub fn region(&self, h: RegionHandle) -> Option<&Region> {
        self.regions.get(h.index, h.generation)
    }

    pub fn region_mut(&mut self, h: RegionHandle) -> Option<&mut Region> {
        self.regions.get_mut(h.index, h.generation)
    }

    /// Every live frame, in arena order, not draw order.
    pub fn iter_frames(&self) -> impl Iterator<Item = (FrameHandle, &Frame)> + '_ {
        self.frames
            .iter()
            .map(|(index, generation, f)| (FrameHandle { index, generation }, f))
    }

    /// Every live frame, mutably, for [`Frame::kind_state`].
    pub fn iter_frames_mut(&mut self) -> impl Iterator<Item = (FrameHandle, &mut Frame)> + '_ {
        self.frames
            .iter_mut()
            .map(|(index, generation, f)| (FrameHandle { index, generation }, f))
    }

    /// Move `h` to its bucket's tail, as the client's level-list add does: show `0x76ae10` on
    /// becoming visible, and strata `0x76a470` or level `0x76a4f0` on a visible frame.
    pub(crate) fn resequence_to_tail(&mut self, h: FrameHandle) {
        // The stamp is `INSERTION_BITS` wide in the packed `ZKey`: at the cap, renumber every
        // frame in its current order, which changes no draw order.
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
            self.renumbers += 1;
        }
        let seq = self.next_insertion;
        let stamp = self.next_kb_stamp;
        self.next_kb_stamp += 1;
        let mut linked = None;
        if let Some(f) = self.frame_mut(h) {
            f.insertion_seq = seq;
            f.char_stamp = stamp;
            f.key_stamp = stamp;
            f.wheel_stamp = stamp;
            linked = Some((f.strata, f.level));
            self.next_insertion += 1;
            if let Some(log) = &mut self.link_log {
                log.push(h);
            }
        }
        if let Some((strata, level)) = linked {
            self.link_bound(strata, level);
        }
    }

    /// A visible frame linked into `strata` at `level`: the stratum's bound grows to one above it
    /// (`0x764882`-`0x764887`, inside the bucket add `0x764780`, which the level setter reaches
    /// only for a visible frame, `0x76a541`). A hidden frame's level never reaches it.
    fn link_bound(&mut self, strata: Strata, level: u16) {
        let bound = &mut self.level_bound[strata.index() as usize];
        *bound = (*bound).max(u32::from(level) + 1);
    }

    /// Start recording every link into a bucket, for the update walk (`0x765650`), which must see
    /// the frames a handler links while it runs.
    pub(crate) fn open_link_log(&mut self) {
        self.link_log = Some(Vec::new());
    }

    /// The frames linked since the last call, in link order, the log staying open.
    pub(crate) fn take_linked(&mut self) -> Vec<FrameHandle> {
        self.link_log
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Stop recording links.
    pub(crate) fn close_link_log(&mut self) {
        self.link_log = None;
    }

    /// How many times the link stamps have been renumbered; stamps read under one count compare.
    pub(crate) fn renumbers(&self) -> u32 {
        self.renumbers
    }

    /// The first frame created with `name`; a later duplicate never takes it over (`0x701bd0`).
    pub fn lookup(&self, name: &str) -> Option<FrameHandle> {
        self.names.get(name).copied()
    }

    // ── Create / destroy ────────────────────────────────────────────────────────────────────────

    /// Create a frame as the client's ctor `0x769090` does, whatever the parent: shown, MEDIUM,
    /// level 0, scale and alpha 1.0. A dead `parent` makes a top-level frame.
    pub fn create(
        &mut self,
        kind: FrameKind,
        name: Option<String>,
        parent: Option<FrameHandle>,
    ) -> FrameHandle {
        let parent = parent.filter(|&p| self.frame(p).is_some());
        let parent_visible = parent.is_none_or(|p| self.frame(p).unwrap().effective_visible);
        let parent_scale = parent.map_or(1.0, |p| self.frame(p).unwrap().effective_scale);
        // The ctor's `SetParent` (`0x769318`) copies the parent's scroll membership (`0x76ab92`).
        let scroll_member = parent.is_some_and(|p| self.frame(p).unwrap().scroll_member);

        let insertion_seq = self.next_insertion;
        self.next_insertion += 1;
        let kb_stamp = self.next_kb_stamp;
        self.next_kb_stamp += 1;
        let created = self.next_created;
        self.next_created += 1;

        let shown = true;
        let scale = 1.0f32;
        // Opaque whatever the parent's: only `SetAlpha` pushes alpha down the tree.
        let alpha = 255u8;

        let frame = Frame {
            disabled_layers: 0,
            kind,
            name: name.clone(),
            parent,
            children: Vec::new(),
            regions: Vec::new(),
            title_region: None,
            // The WorldFrame ctor alone writes stratum 0, WORLD; the rest start MEDIUM.
            strata: if kind == FrameKind::WorldFrame {
                Strata::World
            } else {
                Strata::default()
            },
            level: 0,
            alpha,
            shown,
            effective_visible: shown && parent_visible,
            load_window: false,
            // The ctor zeroes the layout flags (`0x767680`); the load sets the bit.
            layout_deferred: false,
            mouse_enabled: mouse_enabled_by_ctor(kind),
            // The WorldFrame ctor takes the wheel (`0x481b1f`); the ScrollFrame and
            // ScrollingMessageFrame ctors do not (`0x786a40`, `0x787670`), and a frame takes it
            // from an XML `<OnMouseWheel>` (`0x76a044`) or `EnableMouseWheel`.
            mouse_wheel_enabled: kind == FrameKind::WorldFrame,
            // The EditBox ctor enables char and key input (`0x779ced`, `0x779cf8`), the
            // WorldFrame's key input alone (`0x481b09`); every other kind is born inert.
            char_enabled: kind == FrameKind::EditBox,
            key_enabled: matches!(kind, FrameKind::EditBox | FrameKind::WorldFrame),
            char_stamp: kb_stamp,
            key_stamp: kb_stamp,
            wheel_stamp: kb_stamp,
            clamped_to_screen: matches!(kind, FrameKind::GameTooltip),
            hit_rect_insets: [0.0; 4],
            movable: false,
            resizable: false,
            min_resize: (0.0, 0.0),
            max_resize: (0.0, 0.0),
            user_placed: false,
            toplevel: false,
            scroll_member,
            scroll_root: false,
            scale,
            effective_scale: parent_scale * scale,
            ignore_parent_scale: false,
            wow_id: 0,
            insertion_seq,
            created,
            kind_state: match kind {
                FrameKind::StatusBar => KindState::StatusBar(StatusBarState::default()),
                // `CLootButton` and `CSimpleCheckbox` extend `CSimpleButton` by one field each,
                // which `ButtonState` carries (`loot_slot`, `checked`).
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
                // `CGCharacterModelBase` (`0x505680`) extends `CSimpleModel` with its subject, its
                // turn latch and its own light.
                FrameKind::PlayerModel | FrameKind::DressUpModel | FrameKind::TabardModel => {
                    KindState::Model(kinds::ModelState::character())
                }
                k if k.is_model() => KindState::Model(kinds::ModelState::default()),
                FrameKind::Minimap => KindState::Minimap(kinds::MinimapState::default()),
                FrameKind::GameTooltip => KindState::Tooltip(kinds::TooltipState::default()),
                _ => KindState::None,
            },
            next_decl: 0,
        };

        let model_kind = kind.is_model();
        let visible = frame.effective_visible;
        let (strata, level) = (frame.strata, frame.level);
        let (index, generation) = self.frames.insert(frame);
        let handle = FrameHandle { index, generation };
        if visible {
            self.link_bound(strata, level);
            if let Some(log) = &mut self.link_log {
                log.push(handle);
            }
        }
        if model_kind {
            self.model_kinds.push(handle);
        }
        if kind.kind_update().is_some() {
            self.walked_kinds.push(handle);
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
            self.names.entry(n).or_insert(handle);
        }
        if matches!(kind, FrameKind::EditBox) {
            self.editbox_kinds.push(handle);
            self.build_editbox_engine_regions(handle);
        }
        if matches!(kind, FrameKind::Minimap) {
            self.minimap_kinds.push(handle);
        }
        handle
    }

    /// Build the five regions the `CSimpleEditBox` ctor gives every EditBox, in its order: the
    /// text FontString `E+0x328` (`0x779bee`, `0x770d30(E, 2, 1)`), three selection quads
    /// `E+0x350/0x354/0x358` (`0x779c41`..`0x779c72`, `0x76fc40(E, 2, 0)`) and the caret
    /// `E+0x368` (`0x779c86`..`0x779cac`, `0x76fc40(E, 3, 1)`). `GetRegions` (`0x773f60`) lists
    /// them first, hidden or not, so authored `<Layers>` regions start at index 6. The quads carry
    /// no texture: caret and selection paint host-side.
    fn build_editbox_engine_regions(&mut self, eb: FrameHandle) {
        // OVERLAY here; the reference ctor puts the text in layer 2, ARTWORK (`0x770d30(E, 2, 1)`).
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

    /// Destroy a frame and its subtree, with their regions and the names they own.
    pub fn destroy(&mut self, h: FrameHandle) {
        let Some(frame) = self.frame(h) else {
            return;
        };
        let children = frame.children.clone();
        let regions = frame.regions.clone();
        let parent = frame.parent;
        let name = frame.name.clone();
        // Each recursive call removes its own handle, so the registries hold no dead frame.
        self.model_kinds.retain(|&t| t != h);
        self.walked_kinds.retain(|&t| t != h);
        self.tooltip_kinds.retain(|&t| t != h);
        self.minimap_kinds.retain(|&t| t != h);
        self.editbox_kinds.retain(|&t| t != h);

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

    pub fn create_region(
        &mut self,
        owner: FrameHandle,
        kind: RegionKind,
        draw_layer: DrawLayer,
        sub_level: i8,
    ) -> Option<RegionHandle> {
        let decl_seq = self.take_decl(owner)?;
        // The link takes the owner's bit (`0x77fd1e`), and the shown region's start clears it on a
        // visible owner out of its load (`0x77fcb0`).
        let layout_deferred = self
            .frame(owner)
            .is_some_and(|f| f.layout_deferred && !(f.effective_visible && !f.load_window));
        let (index, generation) = self.regions.insert(Region {
            kind,
            owner,
            draw_layer,
            sub_level,
            decl_seq,
            detached: false,
            name: None,
            layout_deferred,
        });
        let handle = RegionHandle { index, generation };
        self.frame_mut(owner)
            .expect("live owner")
            .regions
            .push(handle);
        Some(handle)
    }

    /// The next declaration seq on `owner`, the region's link position. At the draw key's cap the
    /// frame's regions renumber from 0 in link order (`regions` ascends by seq), so a frame that
    /// rebuilds regions for ever, a backdrop set on every show, keeps its order and never
    /// outgrows the key.
    pub(crate) fn take_decl(&mut self, owner: FrameHandle) -> Option<u32> {
        if self.frame(owner)?.next_decl >= crate::order::DECL_CAP {
            let regions = self.frame(owner)?.regions.clone();
            let mut next = 0;
            for rh in regions {
                if let Some(r) = self.region_mut(rh) {
                    r.decl_seq = next;
                    next += 1;
                }
            }
            self.frame_mut(owner)?.next_decl = next;
        }
        let f = self.frame_mut(owner)?;
        let d = f.next_decl;
        f.next_decl += 1;
        Some(d)
    }

    /// Free one region, as `CSimpleHTML::SetText` deletes the last parse's blocks (`0x78a580`)
    /// and a backdrop's teardown its nine pieces (`0x77f4d0`). The caller drops the region's
    /// script-model state too, and never frees a region some state holds by handle.
    pub fn destroy_region(&mut self, h: RegionHandle) -> bool {
        let Some(region) = self.region(h) else {
            return false;
        };
        let owner = region.owner;
        if let Some(f) = self.frame_mut(owner) {
            f.kind_state.region_removed(h);
            f.regions.retain(|&r| r != h);
            if f.title_region == Some(h) {
                f.title_region = None;
            }
        }
        self.regions.remove(h.index, h.generation).is_some()
    }
}

mod propagation;

#[cfg(test)]
mod tests;
