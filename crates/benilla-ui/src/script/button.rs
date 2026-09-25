//! The `Button` and `CheckButton` methods (`CSimpleButton` `0x6eeab0`, `CSimpleCheckbox`
//! `0x6eeb30`). The shown state texture latches at each state edge (`SetState` `0x779790`), not at
//! paint. `<PushedTextOffset>` is not built: a pushed label does not shift.
//! Lookup runs CheckButton's table, then Button's, then the frame table, the client's class chain,
//! so `SetChecked` is nil on a plain Button and `GetText` on a plain Frame.

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

/// A Button's texture and label slots, each an arena region created on first set.
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

    /// The three state slots go through [`ButtonState::set_state_slot`] (`0x778fd0`), which also
    /// shows the texture when it is the current state's; the others are plain fields.
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

    /// The region kind and default layer: state textures under the label, the highlight in its own
    /// layer (the draw-layer table `0x811a84`), the checked marks above the state.
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

/// The normal font's justify (`[button+0x390]`): its own `<NormalFont justifyH=>`, else its font
/// object's, else CENTER. The label adopter anchors by it.
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

/// `SetFontString`'s tail (`0x778d20`), which `SetText` also creates its label through
/// (`0x778dc0`): an unanchored label anchors LEFT, RIGHT or CENTER by the normal font's justify,
/// never its own (`+0x120`, CENTER on a fresh string), then takes the per-state font (`0x779810`).
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

/// Link the label to the normal font object and local justify, the reference's `0x779810` →
/// `0x770c60`, so its `GetFont` answers what it paints; the stock `DropDownList1` reads it
/// (`UIDropDownMenu.xml:11`). Deviation: a hovered or disabled label still reports the normal
/// font, since the state fonts are overlaid at paint and a link made here would outlive the state.
/// `font::repaint` keeps what the label set itself (its severance mask, `CSimpleFontString+0xd4`),
/// and the local justify obeys the same mask.
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
    // An unregistered name is no error here: the setter accepted it and the loader reports it.
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

/// The loader's `<NormalFont justifyH=>` and kin: `LoadXML` (`0x7788c0`) runs the `<Font>` loader
/// (`0x783c30`) on the embedded font, so the justify is its own, not inherited; no Lua verb
/// writes it. An adopted label keeps its anchor and takes the new normal justify.
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

/// Run `f` over a frame's Button state under one short write borrow. It moves no state, only an
/// edge does: re-registering a held button's clicks does not un-press it, as in the reference.
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

/// `Enable()`/`Disable()`. `Disable` (`0x77ffd0`) reaches `SetEnabled` (`0x779160`), which also
/// turns draw layer 4, HIGHLIGHT, off (`0x7791bb`) in the array `DisableDrawLayer` writes, so every
/// region in that layer goes dark, not only the HighlightTexture. `Enable()` here turns it back
/// on; the reference restores it only on a hovered button, by re-running its `<OnEnter>`
/// (`0x779183`), and never while `LockHighlight` holds (`[frame+0xf8]`).
pub(super) fn set_enabled_frame(frame: &mut crate::widget::Frame, on: bool) -> bool {
    let KindState::Button(bs) = &mut frame.kind_state else {
        return false;
    };
    bs.set_enabled(on);
    let bit = 1u8 << DrawLayer::Highlight.index();
    if on {
        frame.disabled_layers &= !bit;
    } else {
        frame.disabled_layers |= bit;
    }
    true
}

fn set_enabled(lua: &Lua, this: &Table, on: bool) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    if !set_enabled_frame(frame, on) {
        return Err(mlua::Error::runtime("not a Button"));
    }
    Ok(())
}

/// Run an engine state edge over frame `h` if it is a live Button. The reference has six, all of
/// `0x779790`'s call sites but Lua's: enable, disable, hide, press, release and drag start; the
/// cursor entering or leaving a button is not one.
pub(super) fn edge(model: &mut Model, h: FrameHandle, f: impl FnOnce(&mut ButtonState)) {
    if let Some(frame) = model.arena.frame_mut(h) {
        if let KindState::Button(bs) = &mut frame.kind_state {
            f(bs);
        }
    }
}

/// Whether a hover edge on `h` reaches the base notify, the HIGHLIGHT layer and the Lua
/// `<OnEnter>`/`<OnLeave>`: `OnEnter`/`OnLeave` (`0x779490`/`0x7794e0`) skip it on a DISABLED
/// Button, leaving only the hover sound (`0x7794b0`). Every hover path goes through that pair
/// (the hover walk `0x766218`, `SetMouseFocus` `0x764dc0`, the hide tail `0x764cce`), and the
/// Hyperlink and NamePlate overrides chain back (`0x7cb8a5`). The hover target still moves, so
/// `GetMouseFocus()` answers a disabled button.
pub(super) fn hover_notify_runs(model: &Model, h: Option<FrameHandle>) -> bool {
    let Some(h) = h else { return true };
    match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Button(bs)) => bs.enabled(),
        _ => true,
    }
}

/// `LootButton:SetSlot`: writes [`ButtonState::loot_slot`].
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
        // `SetSlot` type-checks `this` as a LootButton (`0x4c18ee`) and raises on anything else.
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
            // A new highlight blends ADD, `SetHighlightTexture`'s default (`0x781d92`). The
            // reference applies that mode, or a third argument's, on every call and to a handed-in
            // Texture too (`0x7703f0` from `0x779050` and `0x779110`); here only a new region
            // takes it.
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
            model.touch_layout(); // a region entered the layout gate's read set
            if let Some(frame) = model.arena.frame_mut(h) {
                if let KindState::Button(bs) = &mut frame.kind_state {
                    slot.set(bs, Some(rh));
                }
            }
            // Only a new region gets an implicit anchor: a state texture fills the button
            // (`0x778f9d`/`0x7790db`), a label goes to the adopter (`0x778dc0` → `0x778d20`).
            match slot {
                Slot::Text => adopt_label(&mut model, h, rh),
                _ => super::region::implicit_creation_anchor(&mut model, rh),
            }
            rh
        }
    };
    Ok(model.region_id(rh))
}

/// Point `slot` at `rh` or at nothing, returning the different region it held for the caller to
/// free, as the slot store `0x778fd0` destroys the outgoing object.
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

/// Free the region a slot gave up, never the one that moved in.
fn free_outgoing(lua: &Lua, outgoing: Option<RegionHandle>, incoming: RegionHandle) {
    if let Some(old) = outgoing.filter(|old| *old != incoming) {
        super::region::free_region(
            &mut lua.app_data_mut::<Model>().expect("model app_data"),
            old,
        );
    }
}

/// `Set<State>Texture(texture | "path" | nil)`, forked on the argument's type (`0x781970`). The
/// object and nil legs never touch the slot's own region, so they run before [`ensure_slot`]
/// would create one. The `(r, g, b [, a])` form is not 1.12's, which reads a number as a path
/// (`0x781b23`).
fn set_slot_texture(lua: &Lua, this: &Table, slot: Slot, args: &MultiValue) -> mlua::Result<()> {
    match args.front() {
        // A Texture object is itself installed into the slot (`0x781b0b` → `0x778fd0`).
        Some(Value::Table(t)) => {
            let rh = super::region::region_handle_of(lua, t)?;
            let h = frame_handle_of(lua, this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // Any other widget raises (`0x781ab1`, message `0x87a1e0`).
            if model.arena.region(rh).map(|r| r.kind) != Some(RegionKind::Texture) {
                return Err(mlua::Error::runtime("Wrong object type, expected texture"));
            }
            let outgoing = swap_slot(&mut model, h, slot, Some(rh))?;
            drop(model);
            free_outgoing(lua, outgoing, rh);
            return Ok(());
        }
        // nil clears the slot and destroys the old region (`0x781b5a` → `0x778fd0`): unhooked but
        // alive, it would draw in every state. No argument is not nil (`0x781b4f`) and clears
        // nothing.
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
        // `""` clears the texture, which `QuestLogFrame.lua:165` relies on.
        Some(Value::String(s)) if s.to_str()?.is_empty() => {
            data.texture = None;
            data.fill = None;
        }
        Some(Value::String(s)) => {
            data.texture = Some(s.to_str()?.to_string());
            data.fill = None;
        }
        // The colour form is a solid texture, not a tint: it and the path clear each other.
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

/// `Get<State>Texture`: the region wrapper, or nil while unset.
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

    // SetText/GetText go to the ButtonText fontstring, as the `text` attribute does (`0x778dc0`).
    m.set(
        "SetText",
        lua.create_function(|lua, (this, text): (Table, Option<Value>)| {
            let text = super::binding_abi::text_arg(lua, text)?;
            // A nil returns at once (`0x778dcc`): no clear, no label created. The guard is the
            // button's own; `FontString:SetText(nil)` clears (`0x771d80`).
            let Some(text) = text else { return Ok(()) };
            let id = ensure_slot(lua, &this, Slot::Text)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let rh = *model.id_to_region.get(&id).expect("text region id");
            model.region_data.entry(rh).or_default().text = Some(text);
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;
    // GetText (`0x780e10`): nil for no label, and for an empty one (`0x780ec5`).
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

    // SetFontString(fontString) (`0x780a60` → `0x778d20`) raises, naming the button, on a
    // non-table (`0x87a100`), nil included, so Lua cannot clear the label; on a non-widget
    // (`0x87a160`); on a non-FontString (`0x87a124`). The same label is a no-op; otherwise the old
    // one is destroyed and the new one moves to the button at ARTWORK (`0x77fd10`) and is adopted.
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
            // Two checks in the reference's order, a widget at all (`0x780b27`), then a FontString
            // (`0x780b90`), so a Frame or a Texture gets the second message.
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
                // Destroyed, not orphaned (`0x778d3c`).
                super::region::free_region(&mut model, dead);
            }
            model.arena.set_region_owner(rh, Some(owner));
            if let Some(r) = model.arena.region_mut(rh) {
                r.draw_layer = DrawLayer::Artwork;
            }
            adopt_label(&mut model, owner, rh);
            // The layout's read set moved, and `GetTextWidth` reads the new label's extents.
            model.touch_layout();
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;

    // GetTextWidth/GetTextHeight (`0x782290`/`0x782390`) forward to the label's extents (vtable
    // `0x1c`/`0x20`). The width is the natural, unwrapped one: the laid-out width would feed a
    // size-to-text caller (`MoneyFrame.lua:202`) its own output. 0 before measurement and with no
    // label, where the reference dereferences `+0x338` and what it does on null is untraced.
    for (name, region_getter) in [
        ("GetTextWidth", "GetStringWidth"),
        // 1.12 has no `GetStringHeight`; `0x782390` reads the label's height.
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

    // The per-state label fonts, and `<NormalFont>` and kin through the loader. Each takes a font
    // object, its name or nil, stored by name: extract re-resolves the current state's font every
    // frame, so a state change or a later edit to the object reaches the label.
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

    // SetFont(file, height [, flags]) (`0x780880`) sets all three embedded fonts and returns
    // nothing, discarding the shared `0x79f210`'s load result. It never touches the label
    // (`+0x338`), so creates none: extract applies the stored font to whatever label exists. Its
    // checks are 5.0's coercing `lua_isstring`/`lua_isnumber`, else a usage error (`0x87c69c`).
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
            },
        )?,
    )?;
    // GetFont() (`0x7809a0` → `0x79f3b0`) → file, height, flags off the normal font, else the
    // object it inherits (`<NormalFont>`, `SetTextFontObject`); always three values.
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

    // The per-state label colours: a set one paints over the state font's colour at extract.
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
    // GetTextColor() (`0x781100`) → r, g, b, a, four values, off the normal font: its own colour,
    // else the inherited object's (`<NormalFont>`, `SetTextFontObject`), else white.
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

    // LockHighlight (`0x782840`) and UnlockHighlight: the highlight shows regardless of hover.
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
    // IsEnabled() (`0x7800b0`) → the number 1 or 0 from the state at `+0x328`, never a boolean:
    // 0 is truthy in Lua, and the stock UI tests `== 0` (`FriendsFrame.lua:404`) and `== 1`
    // (`StaticPopup.lua:713`).
    m.set(
        "IsEnabled",
        lua.create_function(|lua, this: Table| {
            with_button(lua, &this, |bs| i64::from(bs.enabled()))
        })?,
    )?;

    // RegisterForClicks(...) (`0x782490`) replaces the registered set rather than adding to it
    // (`0x779730` stores the mask), matching the names in any case (`0x64a4c0`); [`wants_click`]
    // reads it.
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

    // SetButtonState(state [, lock]) (`0x780270`): DISABLED, NORMAL or PUSHED in any case
    // (`0x780390`), else a usage error; `lock` is `GetBoolOrDefault` with default 0 (`0x78032c`),
    // so the `1` of `MainMenuBarMicroButtons.lua:22` sets it. While locked, presses and releases
    // leave the state alone; unlocked, the next release un-pushes a scripted push.
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
            // The latched state (`0x780180` reads `+0x328`): a held button answers PUSHED because
            // its press wrote it, which the chat scroll repeat reads (`ChatFrame.lua:1588`).
            with_button(lua, &this, |bs| match bs.button_state() {
                ButtonVisualState::Disabled => "DISABLED",
                ButtonVisualState::Pushed => "PUSHED",
                ButtonVisualState::Normal => "NORMAL",
            })
        })?,
    )?;

    // Click([button]) (`0x7826c0`): the physical click's path, toggle and `OnClick` alike.
    m.set(
        "Click",
        lua.create_function(|lua, (this, button): (Table, Option<String>)| {
            let h = frame_handle_of(lua, &this)?;
            let id = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.frame_id(h)
            };
            let btn = button.unwrap_or_else(|| "LeftButton".to_string());
            // A released click, flagged as scripted, which only a LootButton reads.
            click_button(lua, id, &btn, false, true);
            Ok(())
        })?,
    )?;

    lua.set_named_registry_value(REG_BUTTON_METHODS, m)?;

    // CheckButton's own table, consulted before Button's.
    let c = lua.create_table()?;
    texture_pair(lua, &c, "Checked", Slot::Checked)?;
    texture_pair(lua, &c, "DisabledChecked", Slot::DisabledChecked)?;
    c.set(
        "SetChecked",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            // `SetChecked` (`0x799bf0`) reads its argument through `0x6f1c10` with default 1
            // (`0x799c77`), not Lua truthiness: a number truncates toward zero (`0x40a2b0`), so
            // `SetChecked(0)` unchecks, and the stock UI passes `"true"`/`"false"`
            // (`SpellBookFrame.lua:296-303`). Here a missing argument unchecks and a string checks
            // only as `"true"` or a nonzero number, where the reference checks for a missing one
            // and reads a string by its first byte, so `"yes"` checks there.
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
        // GetChecked() → the number 1 or nil, never a boolean: `UIOptionsFrame.xml:310` saves
        // `tostring(this:GetChecked())` and `BuffFrame.lua:71` compares it with `"1"`.
        "GetChecked",
        lua.create_function(|lua, this: Table| {
            let checked = with_button(lua, &this, |bs| bs.checked)?;
            Ok(crate::script::binding_abi::flag(checked))
        })?,
    )?;
    lua.set_named_registry_value(REG_CHECKBUTTON_METHODS, c)?;

    Ok(())
}

/// Whether `h` fires `OnClick` for the transition `name` (e.g. `"RightButtonDown"`): a Button
/// checks its `RegisterForClicks` set, ignoring case; any other kind fires on release only.
pub(super) fn wants_click(model: &Model, h: FrameHandle, name: &str) -> bool {
    match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Button(bs)) => bs
            .registered_clicks
            .iter()
            .any(|s| s.eq_ignore_ascii_case(name)),
        _ => name.ends_with("Up"),
    }
}

/// Whether `h` is registered for `button` (`"LeftButton"`, …) as Up or Down, the press-art gate.
/// `OnMouseDown` (`0x779210`) tests `m | m << 8` against `[this+0x330]` (Down in byte 0, Up in
/// byte 1, `0x77924b`) before the Down-only `OnClick` test (`0x77926b`), so any registration for
/// that button pushes the art, click or not.
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

/// The click shared by the input path and `Click()`: a disabled Button fires nothing, a CheckButton
/// toggles before `OnClick` runs (`0x785550`), then `OnClick` gets the button name and `down`, true
/// only for a click a `…ButtonDown` registration fired. The second argument is not 1.12's, whose
/// `OnClick` gets the name alone (`0x779540`). `scripted` marks a Lua `Click()` (`0x7826c0` passes
/// 1, the mouse `0x779280`/`0x7793a4` pass 0): a LootButton then does nothing at all, not even its
/// `OnClick` (`0x4c182b`); every other kind ignores it.
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
            // Deviation: the slot is read before the handler, where the reference reads it after
            // (`[esi+0x4dc]`), because reading first keeps the borrow simple and no stock handler
            // re-slots its row. Shift, ctrl or alt suppresses the take (`0x4c183a`/`0x4c1848`/
            // `0x4c1856`).
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
            // A Button fires only while enabled;
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
            // any other kind with an `OnClick` just fires.
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
    // A nameplate click selects its unit, from the pointer or `Click()` alike (the plate's
    // override `0x7cb910` chains the base), after the handler so an erroring hook cannot eat it. A
    // press selects nothing: the plate registers Up clicks only (`0x7cb637`).
    if !down {
        super::nameplate::note_click(lua, id, button);
    }
    // The take runs whatever the handler did (`0x4c1867`): a row whose `OnClick` errored still
    // loots.
    if let Some(slot) = take_loot {
        // The slot is 0-based; the take queue speaks the 1-based row.
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .loot_picks
            .push(slot + 1);
    }
}
