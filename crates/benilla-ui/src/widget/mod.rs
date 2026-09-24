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

struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// `remove` bumps the slot's generation, so an old handle stops resolving.
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
    /// This frame's own alpha, 0.0..=1.0 (`alpha +0xc8`, default 1.0).
    pub alpha: f32,
    /// Always equal to [`Frame::alpha`]: alpha cascades at set time, and at draw a region
    /// multiplies only its owner frame's alpha (`0x772180`/`0x77fac0`).
    pub effective_alpha: f32,
    /// The frame's own Show/Hide bit (`+0xd0`, default true).
    pub shown: bool,
    /// `shown` and every ancestor effectively visible (`+0xd4`).
    pub effective_visible: bool,
    /// `EnableMouse`: the mouse bit (bit 2) of the input mask `[frame+0xcc]`.
    pub mouse_enabled: bool,
    /// `EnableMouseWheel`, or an `<OnMouseWheel>` script at XML load: the mask's wheel bit (3).
    /// A frame with the script but not the bit is transparent to the wheel.
    pub mouse_wheel_enabled: bool,
    /// `EnableKeyboard` (`0x776ec0`), XML `enableKeyboard` or a key script at XML load: the
    /// mask's char and key bits (`0x76af00`), which key delivery walks
    /// ([`crate::script::keyboard`]); a script-less frame there declines.
    pub keyboard_enabled: bool,
    /// `SetClampedToScreen`, geometry flags bit 4: `0x767a20` shifts the rect back on screen.
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
    pub kind_state: KindState,
    next_decl: u32,
}

/// A region leaf: a texture or font string owned by one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub kind: RegionKind,
    pub owner: FrameHandle,
    pub draw_layer: DrawLayer,
    /// A later client's `textureSubLevel`; 1.12 has no sub-level (`0x76a860`).
    pub sub_level: i8,
    /// The index within the owner frame, the last within-layer draw tiebreak.
    pub decl_seq: u32,
    /// `Region:SetParent(nil)`: unlinked and not drawn, but not destroyed (`0x77fd10` with a null
    /// parent). The owner's `regions` keeps it so `destroy` frees it; `SetParent(frame)` relinks.
    pub detached: bool,
}

// ── The arena ────────────────────────────────────────────────────────────────────────────────

/// The frame arena: frames, regions, the name registry and the propagation mutators. The screen
/// root is not a node: a frame with no parent hangs off it. Within a bucket the client's list is a
/// head insert walked tail to head, the same order as the ascending [`Frame::insertion_seq`].
pub struct WidgetArena {
    frames: Arena<Frame>,
    regions: Arena<Region>,
    names: HashMap<String, FrameHandle>,
    next_insertion: u32,
    /// The draw list's fingerprint cache ([`crate::order::traversal`]).
    pub(crate) order_cache: crate::order::OrderCache,
    /// Minimap frames ever created, so the state feed notices a new one without a walk.
    minimap_created: u64,
    /// The live frames the host ticks (message fades, model scene clocks), so the tick skips the
    /// rest of the arena. Added at creation, as kinds never change; `destroy` removes its own.
    ticked_kinds: Vec<FrameHandle>,
    /// The live GameTooltips, kept the same way, for the layout pre-pass, fade tick and unit push.
    tooltip_kinds: Vec<FrameHandle>,
    /// The live Minimaps, kept the same way, for the containment, zoom and arrow-facing feeds.
    minimap_kinds: Vec<FrameHandle>,
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
/// mouse-enabled child per span (`0x7a3240`), which `script::pointer` models as rects.
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
            minimap_created: 0,
            order_cache: Default::default(),
            ticked_kinds: Vec::new(),
            tooltip_kinds: Vec::new(),
            minimap_kinds: Vec::new(),
        }
    }

    /// Minimap frames ever created, a cheap signal that a new one exists.
    pub fn minimap_created(&self) -> u64 {
        self.minimap_created
    }

    /// The frames the host advances each tick.
    pub fn ticked_kinds(&self) -> &[FrameHandle] {
        &self.ticked_kinds
    }

    pub fn tooltip_kinds(&self) -> &[FrameHandle] {
        &self.tooltip_kinds
    }

    pub fn minimap_kinds(&self) -> &[FrameHandle] {
        &self.minimap_kinds
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
        }
        let seq = self.next_insertion;
        if let Some(f) = self.frame_mut(h) {
            f.insertion_seq = seq;
            self.next_insertion += 1;
        }
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

        let insertion_seq = self.next_insertion;
        self.next_insertion += 1;

        let shown = true;
        let scale = 1.0f32;
        // 1.0 whatever the parent's: only `SetAlpha` pushes alpha down the tree.
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
            // The WorldFrame ctor alone writes stratum 0, WORLD; the rest start MEDIUM.
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
            // The WorldFrame ctor takes the wheel (`0x481b1f`); the reference's ScrollFrame and
            // ScrollingMessageFrame ctors do not, and a stock one takes it from `<OnMouseWheel>`.
            mouse_wheel_enabled: matches!(
                kind,
                FrameKind::ScrollingMessageFrame | FrameKind::ScrollFrame | FrameKind::WorldFrame
            ),
            // No kind is keyboard-enabled at creation here; the reference's EditBox
            // (`0x779ced`/`0x779cf8`) and WorldFrame (`0x481b09`) ctors enable key input.
            keyboard_enabled: false,
            clamped_to_screen: matches!(kind, FrameKind::GameTooltip),
            hit_rect_insets: [0.0; 4],
            movable: false,
            resizable: false,
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
                // `CGCharacterModelBase` extends `CSimpleModel`, adding only a turn-animation pair,
                // which is not modelled.
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
            self.names.entry(n).or_insert(handle);
        }
        if matches!(kind, FrameKind::EditBox) {
            self.build_editbox_engine_regions(handle);
        }
        if matches!(kind, FrameKind::Minimap) {
            self.minimap_kinds.push(handle);
            // The `CMinimap` ctor's last act, so a `CreateFrame("Minimap")` has its nine children
            // before its `OnLoad` too.
            self.build_minimap_engine_children(handle);
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

    /// Build a Minimap's nine engine `Model` children, the ninth its player arrow. They are born
    /// anonymous and without a model file; `CMinimap::LoadXML` (`0x4ee2b0`) assigns the paths.
    fn build_minimap_engine_children(&mut self, minimap: FrameHandle) {
        let mut ninth = None;
        for _ in 0..kinds::MINIMAP_ENGINE_CHILDREN {
            ninth = Some(self.create(FrameKind::Model, None, Some(minimap)));
        }
        if let Some(KindState::Minimap(m)) = self.frame_mut(minimap).map(|f| &mut f.kind_state) {
            m.player_arrow = ninth;
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

    /// Free one region, as only `CSimpleHTML::SetText` does in the client, pool-freeing the last
    /// parse's blocks (`0x78adc6`). The caller drops the region's script-model state too, and never
    /// frees a region some state holds by handle.
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

mod propagation;

#[cfg(test)]
mod tests;
