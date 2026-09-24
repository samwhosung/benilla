//! The `Button`/`CheckButton` method surface — per-kind behavior over the frame arena
//! (`CSimpleButton` `0x6eeab0` / `CSimpleCheckbox` `0x6eeb30`).
//!
//! Grounded in the client's LoadXML table (`0x7788c0`): the four state textures
//! (`Normal/Pushed/Disabled/Highlight`), the `ButtonText` fontstring + `text` attribute, the
//! `OnClick` script slot (`+0x4cc`); CheckButton runs Button's loader first and adds
//! `CheckedTexture`/`DisabledCheckedTexture` + the `checked` bool (`+0x4dc`). Which texture *shows*
//! is **latched on the transition**, not resolved at paint: [`ButtonState::set_state`] is the
//! client's `SetState 0x779790` and [`settle`] is where the derived inputs reach it. Two stated v1
//! gaps: the highlight draws with normal blending (the client ADD-blends it; the quad pass has no
//! blend modes yet), and `PushedTextOffset` is not modeled. Per-state label fonts *are*: the
//! `*FontObject` trio picks which object each state inherits and `extract` re-resolves it every
//! frame, while `SetFont` writes the button's own face/size/flags over all of them
//! ([`crate::widget::ButtonFont`]).
//!
//! Method resolution: CheckButton's table is consulted first, then Button's, then the shared frame
//! table — mirroring the client's class chain, and keeping duck-typing honest (`frame.SetChecked`
//! is nil on a plain Button; `frame.GetText` is nil on a plain Frame).

use mlua::{Lua, MultiValue, ObjectLike, Table, Value};

use super::object::{as_f32, frame_handle_of};
use super::region::region_wrapper;
use super::{event, JustifyH, Model, RegionData};
use crate::justify::Justify;
use crate::order::DrawLayer;
use crate::widget::{
    ButtonFont, ButtonState, ButtonVisualState, FrameHandle, FrameKind, KindState, RegionHandle,
    RegionKind,
};

pub(super) const REG_BUTTON_METHODS: &str = "__benilla_button_methods";
pub(super) const REG_CHECKBUTTON_METHODS: &str = "__benilla_checkbutton_methods";

/// The Button state-texture/text slots (each an arena region created on first set).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Normal,
    Pushed,
    Disabled,
    Highlight,
    Checked,
    DisabledChecked,
    Text,
}

impl Slot {
    fn get(self, bs: &ButtonState) -> Option<crate::widget::RegionHandle> {
        match self {
            Slot::Normal => bs.normal,
            Slot::Pushed => bs.pushed,
            Slot::Disabled => bs.disabled,
            Slot::Highlight => bs.highlight,
            Slot::Checked => bs.checked_tex,
            Slot::DisabledChecked => bs.disabled_checked,
            Slot::Text => bs.text,
        }
    }

    /// The three STATE slots go through [`ButtonState::set_state_slot`] — the client's
    /// `0x778fd0` family, which stores the slot *and* pushes it to the shown pointer when it is
    /// the current state's. Every other slot is a plain field: the highlight, the checked pair
    /// and the label are not in the state array and have show rules of their own.
    fn set(self, bs: &mut ButtonState, rh: Option<crate::widget::RegionHandle>) {
        match self {
            Slot::Normal => bs.set_state_slot(ButtonVisualState::Normal, rh),
            Slot::Pushed => bs.set_state_slot(ButtonVisualState::Pushed, rh),
            Slot::Disabled => bs.set_state_slot(ButtonVisualState::Disabled, rh),
            Slot::Highlight => bs.highlight = rh,
            Slot::Checked => bs.checked_tex = rh,
            Slot::DisabledChecked => bs.disabled_checked = rh,
            Slot::Text => bs.text = rh,
        }
    }

    /// The slot's region kind + default draw layer: state textures under the text, the highlight
    /// in its own top layer (drawLayer table `0x811a84`), the checked marks above the state.
    fn shape(self) -> (RegionKind, DrawLayer) {
        match self {
            Slot::Normal | Slot::Pushed | Slot::Disabled => {
                (RegionKind::Texture, DrawLayer::Artwork)
            }
            Slot::Checked | Slot::DisabledChecked => (RegionKind::Texture, DrawLayer::Overlay),
            Slot::Highlight => (RegionKind::Texture, DrawLayer::Highlight),
            Slot::Text => (RegionKind::FontString, DrawLayer::Overlay),
        }
    }
}

/// The NORMAL embedded font's justify word — `[button+0x390]` (`+0x33c`, the normal `CSimpleFont`,
/// plus its `+0x54` justify cell): the element-level `<NormalFont justifyH=>` when the template
/// wrote one (a local write on that instance, severed from what it inherits), else what the
/// instance inherits from its font object, else the ctor default (CENTER). The word the label
/// adopter anchors by — decision 1996.
fn normal_font_justify(model: &Model, bs: &ButtonState) -> Justify {
    let mut word = Justify::default();
    let inherited = || {
        bs.normal_font
            .as_deref()
            .and_then(|n| model.font_object(n))
            .and_then(|fo| fo.justify_h)
    };
    if let Some(j) = bs.normal_justify_h.or_else(inherited) {
        word.set_h(j);
    }
    word
}

/// `CSimpleButton::SetFontString 0x778d20`'s tail — the ONE path a button label enters by,
/// whether `SetText` made it (`0x778dc0` allocates, then calls this) or Lua handed one over
/// (`SetFontString`): anchor it to the button **only if it has no anchor of its own** and **by the
/// NORMAL font's justify word** (`[button+0x390]`: LEFT→LEFT, RIGHT→RIGHT, else CENTER — never
/// the string's own `+0x120`, which on a fresh string is the ctor's CENTER), then apply the
/// button's per-state font to it on the spot (`0x779810`).
///
/// The word's source is the whole bug this fixes (decision 1996): a `UIMenuButtonTemplate` row
/// has no `<ButtonText>` — its label is born from the reference's `UIMenu_AddButton` →
/// `button:SetText(text)` — and its `<NormalFont inherits="GameFontNormal" justifyH="LEFT"/>` is
/// the only thing that puts the label at the row's left edge. Reading the fresh string's own
/// word instead seated every row CENTER, and with the shortcut string anchored RIGHT in a fixed
/// 104-wide row, "Macro" ran into "/macro".
fn adopt_label(model: &mut Model, owner: FrameHandle, rh: RegionHandle) {
    let point = {
        let Some(frame) = model.arena.frame(owner) else {
            return;
        };
        let KindState::Button(bs) = &frame.kind_state else {
            return;
        };
        super::region::justify_anchor_point(normal_font_justify(model, bs).0)
    };
    super::region::anchor_unanchored_at(model, rh, point);
    apply_normal_font(model, owner);
}

/// Point the ButtonText at the button's NORMAL font — object and local justify — so the label's
/// *query* surface answers what its paint already shows. The live link the reference's per-state
/// applier `0x779810` → `0x770c60` installs: the instance's field block flows onto the label
/// behind the label's own severance mask.
///
/// Before this, the two disagreed. `extract` resolves the per-state font object every frame and
/// overlays it onto a CLONE of the region's data (`extract.rs` l.114-122), so the label painted
/// correctly while `region_data.font_object` stayed `None` — and a dropdown row's
/// `GetFontObject()` answered **nil**, `GetFont()` answered **nil, nil**. Nothing errored: a
/// FontString with no font of its own is a legal state, so this was invisible in exactly the way
/// 1205's silent-drop class predicts.
///
/// The measured consequence is smaller than it first looks, and that is stated rather than implied.
/// The corpus's 65 `GetFontObject` sites are on FontStrings the addon created and linked itself
/// (Dewdrop-2.0 calls `text:SetFontObject(GameFontHighlightSmall)` one line before it reads the
/// object back), so none of them were reaching this. What WAS reaching it is the reference's own
/// `DropDownList1` OnLoad, which derives `UIDROPDOWNMENU_DEFAULT_TEXT_HEIGHT` from
/// `DropDownList1Button1NormalText:GetFont()` and got nil.
///
/// **Normal only, deliberately.** The highlight and disabled objects are transient states that
/// extract overlays at paint time; linking either here would make a resting button report its hover
/// font. The remaining divergence, stated: while a button is disabled or hovered, the reference's
/// label reports that state's font and ours still reports the normal one. The paint is unaffected
/// either way, and no corpus site reads a label's font mid-state.
///
/// `font::repaint` honours the severance mask (`CSimpleFontString+0xd4`), so a `<FontHeight>` or
/// `SetTextColor` the label set for itself survives the link, and the
/// local justify goes behind the same mask.
fn apply_normal_font(model: &mut Model, owner: FrameHandle) {
    let (rh, name, local_justify) = {
        let Some(frame) = model.arena.frame(owner) else {
            return;
        };
        let KindState::Button(bs) = &frame.kind_state else {
            return;
        };
        let Some(rh) = bs.text else {
            return;
        };
        (rh, bs.normal_font.clone(), bs.normal_justify_h)
    };
    // An unregistered name is not an error here: `SetTextFontObject` already accepted it, and the
    // loader's own log-and-continue rule (0068) owns the reporting.
    let fo = name.as_deref().and_then(|n| model.font_object(n).cloned());
    let d = model.region_data.entry(rh).or_default();
    match (&name, fo) {
        (None, _) => d.font_object = None,
        (Some(_), None) => {}
        (Some(name), Some(fo)) => {
            d.font_object = Some(name.clone());
            super::font::repaint(d, &fo);
        }
    }
    if let Some(j) = local_justify {
        if !d.font_explicit.justify_h {
            d.justify.set_h(j);
        }
    }
    model.touch_measure(rh);
}

/// Which of the button's three embedded font instances a `<…Font>` element writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LabelFont {
    Normal,
    Highlight,
    Disabled,
}

/// `<NormalFont justifyH=>` / `<HighlightFont justifyH=>` / `<DisabledFont justifyH=>` — the
/// loader-facing write of a per-state embedded font's own justify (`CSimpleButton::LoadXML
/// 0x7788c0` → the `<Font>` loader `0x783c30` on `+0x33c`/`+0x3b8`/`+0x434`: a `<Font>`-typed
/// element, so the attribute lands on the instance itself, severed from whatever it inherits).
/// No 1.12 Lua verb writes this — the API trio sets the OBJECT each instance inherits, never its
/// local fields — which is why it is crate-internal rather than a method. A label already
/// adopted keeps its anchor (the reference decided that at adoption) while its own justify word
/// follows the normal instance, as the live link propagates the write.
pub(crate) fn set_label_font_justify_h_lua(
    lua: &Lua,
    wrapper: &Table,
    which: LabelFont,
    j: JustifyH,
) -> mlua::Result<()> {
    let owner = frame_handle_of(lua, wrapper)?;
    with_button(lua, wrapper, |bs| match which {
        LabelFont::Normal => bs.normal_justify_h = Some(j),
        LabelFont::Highlight => bs.highlight_justify_h = Some(j),
        LabelFont::Disabled => bs.disabled_justify_h = Some(j),
    })?;
    if which == LabelFont::Normal {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        apply_normal_font(&mut model, owner);
    }
    Ok(())
}

/// Run `f` over a frame's Button state under one short write borrow.
///
/// **It no longer settles a state machine afterwards, because there is no longer one to settle**
/// (decision 2134). The button's state is a latch the engine's own edges write; a Lua write moves
/// it only when the write IS an edge (`Enable`/`Disable`, `SetButtonState`), and those call the
/// edge themselves. `RegisterForClicks` and the texture setters no longer disturb the state at
/// all — which is the reference's behaviour: re-registering a held button's clicks does not
/// un-press it.
fn with_button<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut ButtonState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::Button(bs) => Ok(f(bs)),
        _ => Err(mlua::Error::runtime("not a Button")),
    }
}

/// `Enable()` / `Disable()` — and the second thing they do, which is not the state texture.
///
/// `Disable 0x77ffd0` ends `0x78009a call [vtbl+0x90](0)`, reaching the shared helper
/// `0x779160`; the constructor reaches the same helper with `1` (`0x778766`), which is what puts
/// a fresh button in NORMAL. That helper does **two** things:
///
/// - `[vtbl+0x9c](state, 0)` = `SetButtonState 0x779790` — [`ButtonState::set_enabled`]'s half,
///   whose two early-outs (nothing happens if the button is already in the state being asked for)
///   are the helper's own; and
/// - `0x7791bb push 4; call 0x76a730` — the per-layer enable for layer **4 = HIGHLIGHT**, written
///   into the very `[frame+0x198]` array that `Enable/DisableDrawLayer` writes and that
///   `0x76b3a0` reads back at draw time.
///
/// So a disabled button's highlight is switched off **at the layer**, which takes every region
/// the frame owns there with it — not only the HighlightTexture. That is why the highlight is no
/// longer gated on `enabled` inside [`ButtonState::region_visible`]: one mechanism, in the place
/// the client keeps it, instead of a second rule that agreed with it on the common case and
/// disagreed on `<Layer level="HIGHLIGHT">` art the button did not put there itself.
///
/// **The restore is NOT symmetric, and this comment used to claim it was** — *"one array and one
/// writer, so an addon's `DisableDrawLayer("HIGHLIGHT")` is undone by the next `Enable()`"*. Both
/// halves are false (`0x779160`). The indexed form
/// `[reg + 4*reg + 0x198]` has **three** sites image-wide — `0x76a717` (=1), `0x76a737` (=0) and
/// `DrawLayer 0x76b3a6`'s read — so there are **two** writers; and the enable arm calls
/// *neither* helper. It reaches the ON one only one hop away and only when the button is the
/// frame under the cursor:
///
/// ```text
/// 779183  mov ecx,[esi+0xa0]   ; the CSimpleTop singleton
/// 779189  cmp esi,[ecx+0x7c]   ; am I the hover target?
/// 77918c  jne 0x7791ce         ;   no -> return, layer untouched
/// 779192  call [edx+0x4c]      ; = 0x779490 -> 0x76b6a0 -> 0x76b6c9 push 4; call 0x76a710
/// ```
///
/// — i.e. `Enable()` on a hovered button restores the layer **by re-running the frame's Lua
/// `<OnEnter>`**, and on a non-hovered one restores nothing (the next hover-enter does it). Both
/// restore paths are gated on the LockHighlight flag `[frame+0xf8]`, and the disable arm's
/// `0x7791bb call 0x76a730(4)` is the one site in the image that is *not* — so
/// `LockHighlight()` → `Disable()` → `Enable()` leaves the layer off in the reference, and only
/// `UnlockHighlight()`+`LockHighlight()` or an explicit `EnableDrawLayer` brings it back.
///
/// **Ours is the symmetric bit-flip**, which draws the same in the ordinary case — a highlight is
/// only visible while hovered, and every hover-enter sets it — and diverges in two script-visible
/// ways: we do not fire the `<OnEnter>` the reference synthesises when you `Enable()` a hovered
/// button, and we restore a locked-then-disabled highlight the reference leaves dark. Named here
/// rather than fixed, because the faithful shape needs the layer restore moved onto the hover
/// path where the reference keeps it.
fn set_enabled(lua: &Lua, this: &Table, on: bool) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    // The state half is [`ButtonState::set_enabled`], whose two early-outs are the client's:
    // `Enable()` on a button that is not DISABLED does nothing at all, and neither does
    // `Disable()` on one that already is.
    match &mut frame.kind_state {
        KindState::Button(bs) => bs.set_enabled(on),
        _ => return Err(mlua::Error::runtime("not a Button")),
    };
    let bit = 1u8 << DrawLayer::Highlight.index();
    if on {
        frame.disabled_layers &= !bit;
    } else {
        frame.disabled_layers |= bit;
    }
    Ok(())
}

/// Run one of the engine's own state edges over frame `h`, if it is a live Button.
///
/// The reference's edge set is exactly seven `SetButtonState 0x779790` call sites and **six of
/// them are engine-side**: Enable, Disable, the hide
/// notify, mouse-down, mouse-up and the drag-threshold crossing. There is **no enter or leave
/// edge** — which is why the pointer path no longer calls anything here when the cursor crosses a
/// button's boundary (decision 2134).
///
/// Harmless on a non-Button handle, and on a stale one.
pub(super) fn edge(model: &mut Model, h: FrameHandle, f: impl FnOnce(&mut ButtonState)) {
    if let Some(frame) = model.arena.frame_mut(h) {
        if let KindState::Button(bs) = &mut frame.kind_state {
            f(bs);
        }
    }
}

/// Does a hover edge on frame `h` reach the **base** enter/leave notify — the HIGHLIGHT layer and
/// the Lua `<OnEnter>`/`<OnLeave>` script slots — or is it swallowed on the way there?
///
/// `CSimpleButton::OnEnter 0x779490` and `OnLeave 0x7794e0` both open on the state word, and the
/// branch skips the *whole* body except the sound:
///
/// ```text
/// 779493  mov eax,[esi+0x328]
/// 779499  test eax,eax
/// 77949b  je 0x7794b0          ; DISABLED -> skip the base notify AND the label restyle
/// 77949d  call 0x76b6a0        ; CSimpleFrame::OnEnter — highlight layer on, <OnEnter> at +0x140
/// ```
///
/// — so **a DISABLED Button runs neither script**, and `0x7794b0`'s hover sound is the only thing
/// past the branch; the base pair's
/// script slots are `+0x140`/`+0x148`. Every path that delivers a hover
/// edge dispatches through the object's own vtable, so there is no way around the guard: the hover
/// walk (`0x766218`–`0x76623c`, `[vt+0x50]` then `[vt+0x4c]`), `SetMouseFocus 0x764dc0`
/// (`0x764ddd push 0`), and the hide/removal tail (`0x764cce mov edx,[edi]; push 1;
/// call [edx+0x50]`). `CheckButton` inherits both slots; the Hyperlink and NamePlate buttons
/// override them and tail straight back into this pair (`0x7cb8a5`), so the guard covers the
/// family.
///
/// The hover **target** still moves — the walk reassigns `[root+0x7c]` *before* it fires either
/// notify — so `GetMouseFocus()` answers a disabled button; it is the scripts alone that stay
/// quiet. This is what makes AtlasLoot's unset QuickLook buttons safe on the reference: their
/// `<OnEnter>` guards with `if this:IsEnabled() then`, which is a NUMBER `1`/`0` and therefore
/// always truthy (`IsEnabled 0x7800b0`), so the body would index a nil
/// `QuickLooks[n]` — the reference never runs it at all.
///
/// `None` (the cursor over no frame) and every non-Button kind notify normally.
pub(super) fn hover_notify_runs(model: &Model, h: Option<FrameHandle>) -> bool {
    let Some(h) = h else { return true };
    match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Button(bs)) => bs.enabled(),
        _ => true,
    }
}

/// Write [`ButtonState::loot_slot`] — `LootButton:SetSlot`'s whole body, kept here beside the
/// other `ButtonState` writers rather than reaching into the arena from `script::loot`.
pub(super) fn set_loot_slot(lua: &Lua, this: &Table, slot: Option<u32>) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match (&frame.kind, &mut frame.kind_state) {
        (crate::widget::FrameKind::LootButton, KindState::Button(bs)) => {
            bs.loot_slot = slot;
            Ok(())
        }
        // The reference's own guard: `SetSlot` type-checks `this` against the LootButton tag via
        // vtable slot 4 (`0x4c18ee`), so calling it on a plain Button raises rather than writing a
        // field nothing would ever read.
        _ => Err(mlua::Error::runtime(
            "SetSlot: 'this' is not a LootButton widget",
        )),
    }
}

/// Get-or-create the region behind `slot`; returns its id.
fn ensure_slot(lua: &Lua, this: &Table, slot: Slot) -> mlua::Result<u32> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let existing = match &model
        .arena
        .frame(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?
        .kind_state
    {
        KindState::Button(bs) => slot.get(bs),
        _ => return Err(mlua::Error::runtime("not a Button")),
    };
    let rh = match existing {
        Some(rh) => rh,
        None => {
            let (kind, layer) = slot.shape();
            let rh = model
                .arena
                .create_region(h, kind, layer, 0)
                .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
            // The highlight draws ADD by contract (the live API's SetHighlightTexture default
            // blend; highlight art is authored for it — under straight alpha it reads as a
            // dark box over the icon).
            model.region_data.insert(
                rh,
                RegionData {
                    blend: if slot == Slot::Highlight {
                        crate::script::BlendMode::Add
                    } else {
                        crate::script::BlendMode::default()
                    },
                    ..Default::default()
                },
            );
            model.touch_layout(); // a region entered the layout gate's read set (decision 0740)
            if let Some(frame) = model.arena.frame_mut(h) {
                if let KindState::Button(bs) = &mut frame.kind_state {
                    slot.set(bs, Some(rh));
                }
            }
            // A freshly built slot region gets its creation-path implicit anchor (decision 1310)
            // — an EXISTING slot region is never touched here; the get half of get-or-create
            // changes no geometry. Which anchor depends on the slot: the reference's C++ string
            // setters SetAllPoints a fresh state texture outright (`0x778f9d`/`0x7790db` — fresh
            // means zero anchors, so the conditional form is equivalent), while a fresh LABEL is
            // handed to the adopter (`SetText 0x778dc0` → `0x778d20`), which anchors it by the
            // button's normal font and links it (decision 1996). The XML loader re-derives after
            // applying authored `<Anchors>` (see `loader/widgets.rs`).
            match slot {
                Slot::Text => adopt_label(&mut model, h, rh),
                _ => super::region::implicit_creation_anchor(&mut model, rh),
            }
            rh
        }
    };
    Ok(model.region_id(rh))
}

/// Point `slot` at `rh` (or at nothing) and hand back whatever it held before, when that is a
/// different region. The reference's slot store `0x778fd0` ends in the outgoing object's
/// vtable-slot-0 dtor, so the caller frees it.
fn swap_slot(
    model: &mut Model,
    h: FrameHandle,
    slot: Slot,
    rh: Option<RegionHandle>,
) -> mlua::Result<Option<RegionHandle>> {
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    let KindState::Button(bs) = &mut frame.kind_state else {
        return Err(mlua::Error::runtime("not a Button"));
    };
    let outgoing = slot.get(bs).filter(|old| Some(*old) != rh);
    slot.set(bs, rh);
    model.touch_layout();
    Ok(outgoing)
}

/// Free the region a slot just gave up — never the one that just moved in.
fn free_outgoing(lua: &Lua, outgoing: Option<RegionHandle>, incoming: RegionHandle) {
    if let Some(old) = outgoing.filter(|old| *old != incoming) {
        super::region::free_region(
            &mut lua.app_data_mut::<Model>().expect("model app_data"),
            old,
        );
    }
}

/// The shared body of `Set<State>Texture(texture | "path" | nil)` / `(r, g, b [, a])`.
///
/// The reference forks on `lua_type(L, 2)` into four legs (`0x781970`).
/// **Two of them never touch the
/// slot's own region at all**, so they are resolved *before* [`ensure_slot`] — which would
/// otherwise lazily build a region for a call whose entire purpose is to replace or drop one.
fn set_slot_texture(lua: &Lua, this: &Table, slot: Slot, args: &MultiValue) -> mlua::Result<()> {
    match args.front() {
        // **A widget object installs THAT region into the slot** (`0x781b0b`: `push the Texture
        // object; push idx; call 0x778fd0`) — it does not load a path into the slot's own region.
        // Three corpus addons build a highlight exactly this way and every one of them was a
        // silent no-op here: Bongos' bar-drag button (`bar.lua:64-71`), _Nameplates' mouseover
        // glow (`_Nameplates.lua:217-222`) and Quiver's option rows (`Quiver.bundle.lua:8949-53`)
        // each `CreateTexture` on the button, colour it, `SetAllPoints`, and hand it over.
        Some(Value::Table(t)) => {
            let rh = super::region::region_handle_of(lua, t)?;
            let h = frame_handle_of(lua, this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // The reference type-checks the handed object and raises rather than installing a
            // FontString into a texture slot (`0x781ab1`, `0x87a1e0`).
            if model.arena.region(rh).map(|r| r.kind) != Some(RegionKind::Texture) {
                return Err(mlua::Error::runtime("Wrong object type, expected texture"));
            }
            let outgoing = swap_slot(&mut model, h, slot, Some(rh))?;
            drop(model);
            free_outgoing(lua, outgoing, rh);
            return Ok(());
        }
        // **nil CLEARS the slot** (`0x781b5a`: `push 0; push idx; call 0x778fd0`, whose tail runs
        // the outgoing object's dtor). Clearing the pointer alone would be worse than doing
        // nothing: `ButtonState::region_visible` shows any region that is not one of the button's
        // own slots unconditionally, so an unhooked state texture would start drawing in *every*
        // state instead of none. `TheoryCraft/TheoryCraftUI.lua:215-217` strips a talent-rank
        // button's normal/pushed/highlight art with three of these.
        //
        // A call with NO argument is left alone: `LUA_TNONE` is not `LUA_TNIL`, and the
        // reference's own nil test is `0x781b4f call 0x6f3400 == 0`.
        Some(Value::Nil) => {
            let h = frame_handle_of(lua, this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let outgoing = swap_slot(&mut model, h, slot, None)?;
            drop(model);
            if let Some(old) = outgoing {
                super::region::free_region(
                    &mut lua.app_data_mut::<Model>().expect("model app_data"),
                    old,
                );
            }
            return Ok(());
        }
        _ => {}
    }

    let id = ensure_slot(lua, this, slot)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let rh = *model.id_to_region.get(&id).expect("slot region id");
    let data = model.region_data.entry(rh).or_default();
    match args.front() {
        // `Set<State>Texture("")` CLEARS — the live API's blank form (ref QuestLogFrame.lua:165
        // clears the quest rows' +/- slot with exactly this), same as SetTexture(nil)/("") on a
        // plain region.
        Some(Value::String(s)) if s.to_str()?.is_empty() => {
            data.texture = None;
            data.fill = None;
        }
        Some(Value::String(s)) => {
            data.texture = Some(s.to_str()?.to_string());
            data.fill = None;
        }
        // The colour form generates a solid texture into the same slot the path form loads into
        // ([`RegionData::fill`]) — never a tint, so each clears the other.
        Some(v @ (Value::Number(_) | Value::Integer(_))) => {
            let arg = |i: usize| args.get(i).map(as_f32);
            data.fill = Some([
                as_f32(v),
                arg(1).unwrap_or(0.0),
                arg(2).unwrap_or(0.0),
                arg(3).unwrap_or(1.0),
            ]);
            data.texture = None;
        }
        _ => {}
    }
    Ok(())
}

/// The shared body of `Get<State>Texture` — the region wrapper, or nil while unset.
fn get_slot_texture(lua: &Lua, this: &Table, slot: Slot) -> mlua::Result<Value> {
    let rh = with_button(lua, this, |bs| slot.get(bs))?;
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        rh.map(|rh| model.region_id(rh))
    };
    match id {
        Some(id) => Ok(Value::Table(region_wrapper(lua, id)?)),
        None => Ok(Value::Nil),
    }
}

/// Register one `Set<X>Texture`/`Get<X>Texture` pair on `m`.
fn texture_pair(lua: &Lua, m: &Table, name: &str, slot: Slot) -> mlua::Result<()> {
    m.set(
        format!("Set{name}Texture"),
        lua.create_function(move |lua, (this, args): (Table, MultiValue)| {
            set_slot_texture(lua, &this, slot, &args)
        })?,
    )?;
    m.set(
        format!("Get{name}Texture"),
        lua.create_function(move |lua, this: Table| get_slot_texture(lua, &this, slot))?,
    )?;
    Ok(())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    texture_pair(lua, &m, "Normal", Slot::Normal)?;
    texture_pair(lua, &m, "Pushed", Slot::Pushed)?;
    texture_pair(lua, &m, "Disabled", Slot::Disabled)?;
    texture_pair(lua, &m, "Highlight", Slot::Highlight)?;

    // SetText/GetText target the ButtonText fontstring (`0x778dc0`: the `text` attr routes there).
    m.set(
        "SetText",
        lua.create_function(|lua, (this, text): (Table, Option<Value>)| {
            let text = super::binding_abi::text_arg(lua, text)?;
            // `CSimpleButton::SetText 0x778dc0` opens `if (!text) return;` (`0x778dcc`) — a nil
            // never reaches the label at all, so
            // it neither clears the text nor lazily creates the FontString. That guard is the
            // BUTTON's own: a `FontString:SetText(nil)` is not a no-op, it truncates the cell
            // (`0x771d80`, decision 2110). Below the guard the button is a pass-through to
            // `[button+0x338]->0x771d80`.
            let Some(text) = text else { return Ok(()) };
            let id = ensure_slot(lua, &this, Slot::Text)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let rh = *model.id_to_region.get(&id).expect("text region id");
            model.region_data.entry(rh).or_default().text = Some(text);
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;
    // GetText — like the FontString's own (`region::text`, decision 2110), an **empty label reads
    // back nil**: `Button:GetText 0x780e10` carries the same first-byte substitution at `0x780ec5`,
    // over three nil conditions rather than two (no label region, a NULL cell, an empty cell).
    // Its EditBox neighbour deliberately does not — the law is per binding.
    m.set(
        "GetText",
        lua.create_function(|lua, this: Table| {
            let rh = with_button(lua, &this, |bs| bs.text)?;
            let text = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                rh.and_then(|rh| model.region_data.get(&rh))
                    .and_then(|d| d.text.clone())
                    .filter(|t| !t.is_empty())
            };
            match text {
                Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    m.set(
        "GetFontString",
        lua.create_function(|lua, this: Table| get_slot_texture(lua, &this, Slot::Text))?,
    )?;

    // SetFontString(fontString) — ADOPT a caller-made FontString as this Button's label.
    //
    // End to end (decision 1505): the binding `0x780a60` is gates + a delegate to
    // `CSimpleButton::SetFontString 0x778d20`, which is the SAME function `SetText`'s lazy creation
    // path funnels through — so adopting and creating share their whole tail, and only the
    // allocation differs.
    //
    // The idiom it exists for: build the label yourself so you can style and place it, then hand
    // it to the button so `SetText` and the per-state font machinery drive it. Quiver's
    // `Component/Select.wow.lua` builds every dropdown option row that way, and with the method
    // nil the row died on its first line — the second of B267's three walls.
    //
    // **It raises, in three distinct ways, and never silently no-ops on a bad argument** — the
    // first shape I built here was a lenient no-op and the bytes say otherwise:
    //
    //  · a **missing** argument, `nil`, a number, a string or a boolean → `lua_type(L,2) != TABLE`
    //    → `Usage: %s:SetFontString(fontstring)` (`0x87a100`). Missing and nil take the identical
    //    leg (`index2adr` answers NULL → type −1), so `SetFontString(nil)` **cannot clear the
    //    label from Lua**; the C++ clear exists but no binding reaches it.
    //  · a **table that is not a widget** → `%s:SetFontString(): Couldn't find 'this' in
    //    fontstring` (`0x87a160`).
    //  · a **Frame or a Texture** — anything that is not a FontString → `%s:SetFontString():
    //    Wrong object type, expected fontstring` (`0x87a124`), gated by an `IsA` against the
    //    FontString token, which a Texture fails.
    //
    // `%s` is the BUTTON's name (or `<unnamed>`), never the argument's. Zero return values.
    //
    // What `0x778d20` then does, all four clauses VERIFIED and all four here:
    //
    //  · **`new == old` is a total no-op** — the first compare, before anything is touched.
    //  · **the previous label is DESTROYED**, not orphaned (`old->vtable[0](1)`, the scalar
    //    deleting destructor, returning the storage to the FontString pool). So a button whose
    //    `SetText` already lazily made one does not leak a stray string behind the new label.
    //  · **the string is RE-PARENTED to the button and its draw layer forced to ARTWORK**
    //    (`0x77fd10(parent, 2, 1)` — layer id 2 in the client's own `.rdata 0x811a80` name table).
    //    Always, whatever it was parented to or drawn in before.
    //  · **it is anchored only if it has NO anchors of its own** (a scan of all nine
    //    `anchorPoints` slots): LEFT/RIGHT/CENTER by the justify bits of the **button's normal
    //    font** (`[button+0x390]`), to the matching point on the button — [`adopt_label`], the
    //    same `& 7` → LEFT(1)/RIGHT(4)/else-CENTER chain the FontString post-step runs, over the
    //    other word. (It used to reuse the post-step and read the string's own word; the two
    //    only agree when the normal font has no justify — decision 1996.)
    //  · **the button's per-state font is applied immediately** (`0x779810`) — the same
    //    [`adopt_label`] links the label to the normal font, so its query surface answers.
    m.set(
        "SetFontString",
        lua.create_function(|lua, args: MultiValue| {
            let mut it = args.into_iter();
            let this = match it.next() {
                Some(Value::Table(t)) => t,
                _ => return Err(mlua::Error::runtime("expected a button")),
            };
            let who = {
                let h = frame_handle_of(lua, &this)?;
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .arena
                    .frame(h)
                    .and_then(|f| f.name.clone())
                    .unwrap_or_else(|| "<unnamed>".to_string())
            };
            let Some(Value::Table(fs)) = it.next() else {
                return Err(mlua::Error::runtime(format!(
                    "Usage: {who}:SetFontString(fontstring)"
                )));
            };
            // The two remaining legs are DIFFERENT questions and the reference asks them in this
            // order: first "is this a framescript object at all" (`0x780b27`), then "is it a
            // FontString" (`0x780b90`'s `IsA` against the FontString token). A Frame or a Texture
            // passes the first and fails the second — so resolving straight to a region handle
            // would collapse both onto the first message and mislabel every widget argument.
            let rh = {
                let not_an_object = || {
                    mlua::Error::runtime(format!(
                        "{who}:SetFontString(): Couldn't find 'this' in fontstring"
                    ))
                };
                let id = super::object::decode_id(&fs).map_err(|_| not_an_object())?;
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let known_widget =
                    model.id_to_frame.contains_key(&id) || model.id_to_region.contains_key(&id);
                if !known_widget {
                    return Err(not_an_object());
                }
                match model.id_to_region.get(&id).copied() {
                    Some(rh)
                        if model.arena.region(rh).map(|r| r.kind)
                            == Some(RegionKind::FontString) =>
                    {
                        rh
                    }
                    _ => {
                        return Err(mlua::Error::runtime(format!(
                            "{who}:SetFontString(): Wrong object type, expected fontstring"
                        )))
                    }
                }
            };
            let old = with_button(lua, &this, |bs| bs.text)?;
            if old == Some(rh) {
                return Ok(());
            }
            let owner = frame_handle_of(lua, &this)?;
            with_button(lua, &this, |bs| bs.text = Some(rh))?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(dead) = old {
                // DESTROYED, not orphaned — the shared region-lifetime law, whose doc carries the
                // reason (`0x778d3c`'s scalar deleting destructor).
                super::region::free_region(&mut model, dead);
            }
            model.arena.set_region_owner(rh, Some(owner));
            if let Some(r) = model.arena.region_mut(rh) {
                r.draw_layer = DrawLayer::Artwork;
            }
            adopt_label(&mut model, owner, rh);
            // A new label, a new owner and a possible new anchor — the layout's read set moved,
            // and the string's extents are what the button's own `GetTextWidth` reports.
            model.touch_layout();
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;

    // GetTextWidth / GetTextHeight — the Button's OWN text-extent readers (`0x782290` /
    // `0x782390`); both are present on Button's own method table, `GetStringWidth` is not.
    // Both are thin forwards onto the label FontString's own extent vtable slots
    // (`0x1c` / `0x20`), which is exactly what this delegation is.
    //
    // **Who asks.** `Bagnon_Forever/database/ui.lua:61` sizes its character-switch dropdown from
    // `button:GetTextWidth() + 40` over every saved character — with the method nil the whole
    // dropdown died on the first row, so the director could not switch characters in Bagnon at all.
    // The idiom generalises: it is how a 1.12 kit fits a button to its label, and the reference's
    // own `MoneyFrame.lua` l.202 (`SetWidth(GetTextWidth() + iconWidth)`) is the same shape.
    //
    // **Which measurement**, since a FontString carries two. The label's `GetStringWidth` — the
    // NATURAL, unwrapped extent — never the laid-out `GetWidth`. A button label does not wrap, so
    // the two agree in the ordinary case; where they differ, serving the laid-out width would hand
    // every "size the button to its text" caller its own previous output as its next input, which
    // is decision 0997's measured feedback loop (the macro window's tab that changed width every
    // frame). The unwrapped extent is the one that settles.
    //
    // `0` before the host has measured the string (a frame's latency, as everywhere else), and `0`
    // for a Button with no label at all — the reference dereferences its FontString pointer `+0x338`
    // here where `SetFont`/`GetFont` deliberately do not, and what a null one does is NOT byte-read,
    // so this answers the harmless number rather than raising on a guess.
    for (name, region_getter) in [
        ("GetTextWidth", "GetStringWidth"),
        // `GetHeight`, not a `GetStringHeight` — 1.12 has no such method (byte-verified absent
        // in every encoding), and the reference's own `Button:GetTextHeight 0x782390` is this
        // same call on the embedded FontString rather than a separate API.
        ("GetTextHeight", "GetHeight"),
    ] {
        m.set(
            name,
            lua.create_function(move |lua, this: Table| {
                let label = get_slot_texture(lua, &this, Slot::Text)?;
                match label {
                    Value::Table(t) => t.call_method::<f32>(region_getter, ()),
                    _ => Ok(0.0),
                }
            })?,
        )?;
    }

    // The per-state label fonts (the 1.12 API trio; XML `<NormalFont>/<HighlightFont>/
    // <DisabledFont>` route here through the loader). Stored as font-object NAMES — extract
    // re-points the ButtonText to the current state's object each frame, so Enable/Disable and
    // hover swap the label's paint with no Lua involvement (the client's own behavior:
    // UIPanelButtonTemplate's gold/white/gray label states).
    //
    // Each takes the font OBJECT, its name, or nil, like `FontString:SetFontObject` — across the
    // trio the corpus splits 5 object-form to 4 string-form, so accepting one shape only would
    // break about half the callers, and nil clears the state back to the default. Because the
    // state is stored as a NAME and re-resolved at every `extract`, a later mutation of that font
    // object reaches these labels for free.
    m.set(
        "SetTextFontObject",
        lua.create_function(|lua, (this, font): (Table, Value)| {
            let name = super::font::resolve("SetTextFontObject", &font)?;
            let owner = frame_handle_of(lua, &this)?;
            with_button(lua, &this, |bs| bs.normal_font = name)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            apply_normal_font(&mut model, owner);
            Ok(())
        })?,
    )?;
    m.set(
        "SetHighlightFontObject",
        lua.create_function(|lua, (this, font): (Table, Value)| {
            let name = super::font::resolve("SetHighlightFontObject", &font)?;
            with_button(lua, &this, |bs| bs.highlight_font = name)
        })?,
    )?;
    m.set(
        "SetDisabledFontObject",
        lua.create_function(|lua, (this, font): (Table, Value)| {
            let name = super::font::resolve("SetDisabledFontObject", &font)?;
            with_button(lua, &this, |bs| bs.disabled_font = name)
        })?,
    )?;

    // SetFont(file, height [, flags]) / GetFont() — the Button's own font, `0x780880`/`0x79f3b0`.
    // `_LazyPig/LazyPigMenu.lua:214` calls it straight on a `CreateFrame("Button", …)`, and it is
    // that addon's only blocker.
    //
    // Three contract details, each a plausible implementation's silent divergence:
    //
    //  · **It returns ZERO values.** The shared impl `0x79f210` pushes `1`/nil (a real font-load
    //    probe on a Font object — `!OmniCC` uses it), but `Button:SetFont` DISCARDS it
    //    (`xor eax,eax`), so a failed font load is undetectable from a Button. Returning `true`
    //    here would look harmless and would hand an addon a probe the client does not have.
    //  · **It never touches the label.** `SetFont`/`GetFont` do not dereference the FontString
    //    pointer `+0x338` at all, so styling a bare `CreateFrame("Button")` with no `<ButtonText>`
    //    is no error, no crash and **no lazy creation** — which is why the style is stored on the
    //    button ([`ButtonState::font`]) and `extract` applies it to whatever label exists. A later
    //    `SetText` (which *does* lazily create one, `0x778dc0`) then picks it up, so either call
    //    order works, exactly as the reference's does.
    //  · **It retunes all three embedded fonts** (normal `+0x33c`, disabled `+0x434`, highlight
    //    `+0x3b8`) and `GetFont` reads back only the normal one — see [`ButtonFont`] for why one
    //    record is the faithful shape for three identical writes.
    //
    // The argument gate is the shared impl's: arg2 `lua_isstring` and arg3 `lua_isnumber`, else
    // `luaL_error("Usage: %s:SetFont(\"font\", fontHeight [, flags])")` (`0x87c69c`). Both
    // predicates are the *coercing* ones in 5.0 — `lua_isstring` takes a number, `lua_isnumber`
    // takes a numeric string — so the leniency is transcribed rather than tightened.
    m.set(
        "SetFont",
        lua.create_function(
            |lua, (this, file, height, flags): (Table, Value, Value, Option<String>)| {
                let usage = || {
                    mlua::Error::runtime(
                        "Usage: <Button>:SetFont(\"font\", fontHeight [, flags])".to_string(),
                    )
                };
                let path = match &file {
                    Value::String(s) => s.to_str()?.to_string(),
                    Value::Number(_) | Value::Integer(_) => as_f32(&file).to_string(),
                    _ => return Err(usage()),
                };
                let height = match &height {
                    Value::Number(_) | Value::Integer(_) => as_f32(&height),
                    Value::String(s) => s.to_str()?.parse::<f32>().map_err(|_| usage())?,
                    _ => return Err(usage()),
                };
                let flags = super::Outline::flags(flags.as_deref().unwrap_or(""))
                    .as_str()
                    .to_string();
                with_button(lua, &this, |bs| {
                    bs.font = Some(ButtonFont {
                        path,
                        height,
                        flags,
                    })
                })
                // …and nothing is returned: `with_button` yields `()`.
            },
        )?,
    )?;
    // GetFont() → file, height, flags — **3 values**, read off the NORMAL embedded font. Unset
    // locally, that font still resolves through what the normal state inherits
    // (`<NormalFont>`/`SetTextFontObject`), which is how a `GameMenuButtonTemplate` button answers
    // GameFontNormal's face before anything calls `SetFont`. Nil path / nil height when neither
    // exists, still 3 values.
    m.set(
        "GetFont",
        lua.create_function(|lua, this: Table| {
            let (own, inherits) =
                with_button(lua, &this, |bs| (bs.font.clone(), bs.normal_font.clone()))?;
            let (path, height, flags) = match own {
                Some(f) => (Some(f.path), Some(f.height), f.flags),
                None => {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    let fo = inherits.and_then(|n| model.font_object(&n));
                    (
                        fo.and_then(|f| f.font.clone()),
                        fo.and_then(|f| f.height),
                        fo.map(|f| f.outline)
                            .unwrap_or_default()
                            .as_str()
                            .to_string(),
                    )
                }
            };
            let path = match path {
                Some(p) => Value::String(lua.create_string(&p)?),
                None => Value::Nil,
            };
            Ok((path, height, flags))
        })?,
    )?;

    // The per-state label COLORS (Button:SetTextColor + the Highlight/Disabled pair): the state's
    // color, when set, repaints the ButtonText over the state font object's paint at extract —
    // the dropdown kit's rows use all three (info.textR/G/B rows, isTitle's NORMAL-yellow and
    // notClickable's HIGHLIGHT-white recolors of a disabled row).
    m.set(
        "SetTextColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                // Shape C on the channels (`Button:SetTextColor 0x780ee0`, `2=C 3=C 4=C 5=B`).
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                with_button(lua, &this, |bs| {
                    bs.normal_color = Some([r, g, b, a.unwrap_or(1.0)])
                })
            },
        )?,
    )?;
    // GetTextColor() → r, g, b, a — **FOUR values** (`0x781100`, table `0x879d00`, argc 1, arity 4,
    // kinds `(number,number,number,number)`), the same shape as the FontString/Font-object twins
    // (`0x79d840` table `0x87c1d8`, `0x79f680`) this codebase already answers. Three is the
    // plausible wrong answer, and the shapes table calls the name `name-not-unique` precisely
    // because it is registered on seven tables; this is the Button one.
    //
    // **Read like `GetFont` right above, not like `SetTextColor` right above that.** The reference
    // reads the colour back off the button's NORMAL embedded `CSimpleFont`, and an unset local
    // colour there still resolves through what the normal state INHERITS
    // (`<NormalFont>`/`SetTextFontObject`) — which is how a `GameMenuButtonTemplate` button answers
    // `GameFontNormal`'s colour before anything calls `SetTextColor`. Falling back to white would
    // be wrong in exactly the case the corpus cares about: reading a stock button's colour to
    // restore it after a temporary recolour. White is the last resort, matching every other colour
    // getter here (`GetVertexColor`, `FontString:GetTextColor`) — the untinted default a region
    // with no colour anywhere draws at.
    //
    // Completeness rather than a live break: the census found no corpus site with a Button
    // receiver (every measured `GetTextColor` call is on a FontString or a font object). It is here
    // because the reference registers it and the widget shape gate can then cover it.
    m.set(
        "GetTextColor",
        lua.create_function(|lua, this: Table| {
            let (own, inherits) =
                with_button(lua, &this, |bs| (bs.normal_color, bs.normal_font.clone()))?;
            let c = own.unwrap_or_else(|| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                inherits
                    .and_then(|n| model.font_object(&n))
                    .and_then(|f| f.color)
                    .unwrap_or([1.0, 1.0, 1.0, 1.0])
            });
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;
    m.set(
        "SetHighlightTextColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                // Shape C on the channels (`Button:SetTextColor 0x780ee0`, `2=C 3=C 4=C 5=B`).
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                with_button(lua, &this, |bs| {
                    bs.highlight_color = Some([r, g, b, a.unwrap_or(1.0)])
                })
            },
        )?,
    )?;
    m.set(
        "SetDisabledTextColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                // Shape C on the channels (`Button:SetTextColor 0x780ee0`, `2=C 3=C 4=C 5=B`).
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                with_button(lua, &this, |bs| {
                    bs.disabled_color = Some([r, g, b, a.unwrap_or(1.0)])
                })
            },
        )?,
    )?;

    // LockHighlight/UnlockHighlight — pin the HighlightTexture on regardless of hover (ref
    // CButton::LockHighlight; the dropdown kit's checked rows stay lit).
    m.set(
        "LockHighlight",
        lua.create_function(|lua, this: Table| {
            with_button(lua, &this, |bs| bs.locked_highlight = true)
        })?,
    )?;
    m.set(
        "UnlockHighlight",
        lua.create_function(|lua, this: Table| {
            with_button(lua, &this, |bs| bs.locked_highlight = false)
        })?,
    )?;

    m.set(
        "Enable",
        lua.create_function(|lua, this: Table| set_enabled(lua, &this, true))?,
    )?;
    m.set(
        "Disable",
        lua.create_function(|lua, this: Table| set_enabled(lua, &this, false))?,
    )?;
    // IsEnabled() → the NUMBER 1 or the NUMBER 0 — never a boolean, and never nil.
    //
    // The Button method table at
    // `0x879d10` gives `IsEnabled 0x7800b0`, which reads the three-valued STATE at `[obj+0x328]`
    // (0 DISABLED / 1 NORMAL / 2 PUSHED), does `setne cl` for "not disabled", and pushes that as a
    // NUMBER. So the reference's own `IsEnabled() == 0` and `== 1` tests (FriendsFrame.lua l.404,
    // StaticPopup.lua l.713) are live code.
    //
    // **This answered a Lua boolean until now, and the difference is not cosmetic, because 0 is
    // TRUTHY in Lua.** A caller writing the reference's `== 0` got false forever; a caller writing
    // `IsEnabled() > 0` got "attempt to compare number with boolean" — which is where pfUI's
    // `api/ui-widgets.lua:812` icon-fade died, taking the widget module with it. The divergence was
    // known and worked around by hand in three of our own files (FriendsFrame.xml's
    // `guildButtonDisabled` read BOTH spellings and said so); those workarounds go with this
    // change, which is 1719's bar — a prior call revisited because a new measurement arrived.
    //
    // Slider used to carry its own `IsEnabled` (slider.rs), left alone here because "whether the
    // slider widget shares the contract is unverified". It does not share it — it does not HAVE
    // one: 1.12 registers `Enable`/`Disable`/`IsEnabled` on this table alone, and the Slider's
    // LoadXML `0x789580` has no `enabled` attribute either, so ours was a superset. The trio came
    // off the Slider rather than being brought into line with this contract; see the note at its
    // old site in `slider.rs`.
    m.set(
        "IsEnabled",
        lua.create_function(|lua, this: Table| {
            with_button(lua, &this, |bs| i64::from(bs.enabled()))
        })?,
    )?;

    // RegisterForClicks(...): replaces the button's registered-click set outright (the live API's
    // contract — not additive) with its varargs, verbatim strings (`"LeftButtonUp"`,
    // `"RightButtonDown"`, …). [`UiScript::mouse_button`] consults the set via [`wants_click`] to
    // decide whether a press or a release reaches `OnClick`.
    m.set(
        "RegisterForClicks",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let mut set = std::collections::HashSet::new();
            for v in args.iter() {
                if let Value::String(s) = v {
                    set.insert(s.to_str()?.to_string());
                }
            }
            with_button(lua, &this, |bs| bs.registered_clicks = set)
        })?,
    )?;

    // **`SetButtonState(state[, lock]) 0x780270` — TWO arguments**, and the second is not
    // cosmetic. It reads three stack
    // indices and returns none; the state maps case-insensitively over exactly
    // `{"DISABLED", "NORMAL", "PUSHED"}` (`0x780390`, `SStrCmpI`) and an unrecognised string
    // raises `Usage: %s:SetButtonState("state", lock)`; the flag is `GetBoolOrDefault` with
    // **default 0** (`0x78032c push 0`), whose number arm truncates through `_ftol`, so the
    // literal `1` that `MainMenuBarMicroButtons.lua` passes arms it.
    //
    // **The lock is the whole of decision 2134.** While it is set the button ignores the mouse
    // press/release transitions; while it is clear, the next release un-pushes a scripted push —
    // which is what Tablet-2.0's rows depend on, since the library pushes them and never calls
    // `SetButtonState("NORMAL")` anywhere.
    //
    // DISABLED really is accepted here — the mapper takes it — even though the 1.12 FrameXML
    // never passes it.
    m.set(
        "SetButtonState",
        lua.create_function(|lua, (this, state, lock): (Table, String, MultiValue)| {
            let new = if state.eq_ignore_ascii_case("PUSHED") {
                ButtonVisualState::Pushed
            } else if state.eq_ignore_ascii_case("NORMAL") {
                ButtonVisualState::Normal
            } else if state.eq_ignore_ascii_case("DISABLED") {
                ButtonVisualState::Disabled
            } else {
                return Err(mlua::Error::runtime(format!(
                    "Usage: SetButtonState(\"state\", lock) — unknown state '{state}'"
                )));
            };
            let first = lock.into_iter().next();
            let locked = super::binding_abi::bool_or_default(first.as_ref(), false);
            with_button(lua, &this, |bs| bs.set_button_state(new, locked))
        })?,
    )?;
    m.set(
        "GetButtonState",
        lua.create_function(|lua, this: Table| {
            // ONE variable, read back (`0x780180` reads `[obj+0x328]`). This used to OR in the
            // live mouse-held press, because the state was derived and the press lived outside
            // the widget; now the press *wrote* the state at its edge, so a mouse-held button
            // answers "PUSHED" for the same reason the reference does — and cannot disagree with
            // the art the way a re-derived answer could (decision 2134). The chat scroll buttons'
            // hold-repeat (ref `MessageFrameScrollButton_OnUpdate`) still reads what it needs.
            with_button(lua, &this, |bs| match bs.button_state() {
                ButtonVisualState::Disabled => "DISABLED",
                ButtonVisualState::Pushed => "PUSHED",
                ButtonVisualState::Normal => "NORMAL",
            })
        })?,
    )?;

    // Click([button]) — the programmatic click: same path as a physical one (toggle-then-OnClick).
    m.set(
        "Click",
        lua.create_function(|lua, (this, button): (Table, Option<String>)| {
            let h = frame_handle_of(lua, &this)?;
            let id = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.frame_id(h)
            };
            let btn = button.unwrap_or_else(|| "LeftButton".to_string());
            // A programmatic Click() always emulates a completed (released) click.
            // A programmatic Click() — `scripted = true`, which only a LootButton reads.
            click_button(lua, id, &btn, false, true);
            Ok(())
        })?,
    )?;

    lua.set_named_registry_value(REG_BUTTON_METHODS, m)?;

    // CheckButton's own table (consulted before Button's — the class chain).
    let c = lua.create_table()?;
    texture_pair(lua, &c, "Checked", Slot::Checked)?;
    texture_pair(lua, &c, "DisabledChecked", Slot::DisabledChecked)?;
    c.set(
        "SetChecked",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            // Numeric coercion, NOT Lua truthiness (decision 0227; `SetChecked 0x799bf0` →
            // `0x6f1c10`): a number goes through `lua_tonumber` then a truncate-to-int (`fistp`,
            // round-toward-zero, `0x40a2b0`) and the C++ setter tests `!= 0`. So `SetChecked(0)`
            // UNchecks (0 is Lua-truthy — only a numeric read gets this right) and `SetChecked(1)`
            // checks; the type-dispatched helper also honors the keyword strings the reference
            // passes ("true"/"false" — SpellBookFrame.lua l.296-303), while a non-keyword string
            // falls to tonumber (→ 0 → unchecked). nil/other → false; booleans honored as-is for
            // this codebase's own Era-style callers.
            let checked = match v {
                Value::Boolean(b) => b,
                Value::Integer(i) => i != 0,
                Value::Number(n) => n.trunc() != 0.0,
                Value::String(s) => s.to_str().ok().is_some_and(|s| {
                    let t = s.trim();
                    t.eq_ignore_ascii_case("true")
                        || t.parse::<f64>().is_ok_and(|n| n.trunc() != 0.0)
                }),
                _ => false,
            };
            with_button(lua, &this, |bs| bs.checked = checked)
        })?,
    )?;
    c.set(
        // GetChecked() -> the NUMBER 1 or nil, never a boolean (1830). Its neighbour
        // `IsEnabled` two entries up was corrected for the same reason under 1719, whose comment
        // records that a boolean here killed pfUI's widget module outright; this is that call
        // finished. The reference proves the shape in its own source rather than only at the
        // bytes: stock `UIOptionsFrame.xml:310` saves a checkbox as
        // `tostring(this:GetChecked())` and stock `BuffFrame.lua:71` reads it back as `== "1"`,
        // a round-trip that only closes on the number.
        "GetChecked",
        lua.create_function(|lua, this: Table| {
            let checked = with_button(lua, &this, |bs| bs.checked)?;
            Ok(crate::script::binding_abi::flag(checked))
        })?,
    )?;
    lua.set_named_registry_value(REG_CHECKBUTTON_METHODS, c)?;

    Ok(())
}

/// Whether frame `h` should fire `OnClick` for the input transition `name`
/// (`"<Button>ButtonUp"`/`"<Button>ButtonDown"`, e.g. `"RightButtonDown"`) — the
/// [`UiScript::mouse_button`](super::UiScript::mouse_button) gate. A Button/CheckButton consults
/// its `RegisterForClicks` set (case-insensitively — the live API is forgiving of case); any
/// other kind with an `OnClick` fires exactly as it always has, before `RegisterForClicks`
/// existed: on release only (`name` ending `"Up"`), for any button, never on press.
pub(super) fn wants_click(model: &Model, h: FrameHandle, name: &str) -> bool {
    match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Button(bs)) => bs
            .registered_clicks
            .iter()
            .any(|s| s.eq_ignore_ascii_case(name)),
        _ => name.ends_with("Up"),
    }
}

/// Whether frame `h` is registered for `button` (`"LeftButton"`, `"RightButton"`, …) in **either**
/// variant — the press-visual gate, which is a strictly weaker test than [`wants_click`]'s.
///
/// `CButton::OnMouseDown 0x779210` runs the two as separate tests in one pass, and the difference
/// between them is the whole of this:
///
/// ```text
/// 0x779238  eax = event.buttonMask
/// 0x779243  ecx = eax << 8 | eax
/// 0x77924b  test [this+0x330], ecx   ; registered EITHER WAY -> keep going, else nothing at all
/// 0x779256  hit-test the point       ; else nothing
/// 0x77926b  test event.buttonMask, [this+0x330]  ; the DOWN bits alone -> fire OnClick
/// 0x7792ad  push 2 ; SetState(PUSHED)            ; unconditional past the gates above
/// ```
///
/// So `[this+0x330]` is one dword holding the `"…ButtonDown"` registrations in byte 0 and the
/// `"…ButtonUp"` registrations in byte 1, and the mask `m | m << 8` asks "this button, either
/// variant". **The pushed art is not conditional on the click firing, or on the handler doing
/// anything** — it is conditional only on the button being registered for that mouse button at
/// all, which is why a right-click on an action, spellbook or pet slot lights up even when the
/// right-click does nothing (they all register `RightButtonUp`), and why a right-click on a plain
/// `LeftButtonUp` button does not.
pub(super) fn wants_press_visual(model: &Model, h: FrameHandle, button: &str) -> bool {
    match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Button(bs)) => bs.registered_clicks.iter().any(|s| {
            s.strip_suffix("Up")
                .or_else(|| s.strip_suffix("Down"))
                .is_some_and(|b| b.eq_ignore_ascii_case(button))
        }),
        _ => false,
    }
}

/// The click behavior shared by the input path and `Click()`: gated on `enabled`; a CheckButton
/// **toggles before OnClick fires** (the documented widget contract — a handler reading
/// `self:GetChecked()` sees the new state); then `OnClick(self, button, down)` — `down` is `true`
/// only for a press that fired via a `"<Button>ButtonDown"` registration, `false` for every
/// release-fired or programmatic click (the Era signature: `down` mirrors which transition fired).
///
/// `scripted` says the click came from Lua `:Click()` rather than from hardware — the reference's
/// second `OnClick` argument (`0` from `0x779280`/`0x7793a4`, `1` from the `Button:Click` binding
/// `0x7826c0`). Only [`crate::widget::FrameKind::LootButton`] reads it, and it reads it as a hard
/// gate: `0x4c182b` returns before the base call, so **a scripted click on a loot row does
/// nothing at all — not even run the row's own Lua `OnClick`**. Every other kind ignores it, which
/// is the reference's shape too (the base `0x779540` never looks at the flag).
pub(super) fn click_button(lua: &Lua, id: u32, button: &str, down: bool, scripted: bool) {
    let mut take_loot = None;
    let fire = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let Some(&h) = model.id_to_frame.get(&id) else {
            return;
        };
        let is_loot = model
            .arena
            .frame(h)
            .is_some_and(|f| f.kind == crate::widget::FrameKind::LootButton);
        if is_loot {
            if scripted {
                return;
            }
            // Read the take BEFORE the handler runs, but queue it after: the reference reads
            // `[esi+0x4dc]` after the base call, and a handler that re-slots its own row mid-click
            // is not a case the shipped UI produces — reading first keeps the borrow simple and
            // the two cannot disagree. The modifier gate is `0x4c183a`/`0x4c1848`/`0x4c1856`:
            // shift, ctrl OR alt suppresses, and it is the whole reason the shipped
            // `LootFrameItem_OnClick` never calls a take itself.
            let (shift, ctrl, alt) = model.modifiers;
            if !shift && !ctrl && !alt {
                take_loot = model.arena.frame(h).and_then(|f| match &f.kind_state {
                    KindState::Button(bs) => bs.loot_slot,
                    _ => None,
                });
            }
        }
        let Some(frame) = model.arena.frame_mut(h) else {
            return;
        };
        let is_check = frame.kind == FrameKind::CheckButton;
        match &mut frame.kind_state {
            // A Button/CheckButton click is gated on its enabled flag…
            KindState::Button(bs) => {
                if bs.enabled() {
                    if is_check {
                        bs.checked = !bs.checked;
                    }
                    true
                } else {
                    false
                }
            }
            // …every other kind with an OnClick just fires (plain frames can carry one too).
            _ => true,
        }
    };
    if !fire {
        return;
    }
    let btn = match lua.create_string(button) {
        Ok(s) => Value::String(s),
        Err(_) => return,
    };
    if let Err(e) = event::fire_widget_handler(lua, id, "OnClick", vec![btn, Value::Boolean(down)])
    {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
    // **A nameplate's click is the engine's too** (decision 2148): the reference's plate overrides
    // the button click slot (`0x7cb910`) *and* chains the base, so its unit is selected whether the
    // click came from the pointer or from Lua's own `Click()` — this funnel is both. Recorded after
    // the handler, like the loot take below and for the same reason: an addon hook that errors
    // cannot silently eat the selection. A press (`down`) records nothing — the plate registers for
    // `LeftButtonUp | RightButtonUp` only (`RegisterForClicks(0x500)` at `0x7cb637`).
    if !down {
        super::nameplate::note_click(lua, id, button);
    }
    // …and then the take, unconditionally on the handler's outcome. `0x4c1867` is not guarded by
    // anything the Lua side did: the base call at `0x4c1833` returns void and its result is never
    // tested. A row whose `OnClick` errored still loots, which is the reference's behaviour and
    // the reason a broken addon hook cannot silently eat your loot.
    if let Some(slot) = take_loot {
        // The 0-based store, back to the 1-based display row the take queue speaks.
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .loot_picks
            .push(slot + 1);
    }
}
