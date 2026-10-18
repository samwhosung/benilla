//! The `Button` and `CheckButton` methods (`CSimpleButton` `0x6eeab0`, `CSimpleCheckbox`
//! `0x6eeb30`). The shown state texture latches at each state edge (`SetState` `0x779790`), not at
//! paint, and so does the pushed label's text offset (`0x779810`).
//! Lookup runs CheckButton's table, then Button's, then the frame table, the client's class chain,
//! so `SetChecked` is nil on a plain Button and `GetText` on a plain Frame.

use mlua::{Lua, MultiValue, ObjectLike, Table, Value};

use super::font::FontId;
use super::object::frame_handle_of;
use super::region::region_wrapper;
use super::{event, Model, RegionData};
use crate::order::DrawLayer;
use crate::widget::{
    ButtonFonts, ButtonState, ButtonVisualState, FrameHandle, FrameKind, KindState, RegionHandle,
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
    /// shows the texture when it is the current state's, and the two checked slots refresh the
    /// pair (`0x7854c0` from `0x785330`/`0x785440`); the others are plain fields. The bits the
    /// store writes come back for the caller to apply.
    fn set(
        self,
        bs: &mut ButtonState,
        rh: Option<crate::widget::RegionHandle>,
    ) -> crate::widget::ShownWrites {
        match self {
            Slot::Normal => return bs.set_state_slot(ButtonVisualState::Normal, rh),
            Slot::Pushed => return bs.set_state_slot(ButtonVisualState::Pushed, rh),
            Slot::Disabled => return bs.set_state_slot(ButtonVisualState::Disabled, rh),
            Slot::Highlight => bs.highlight = rh,
            Slot::Checked => {
                bs.checked_tex = rh;
                return bs.refresh_checked(bs.checked, true);
            }
            Slot::DisabledChecked => {
                bs.disabled_checked = rh;
                return bs.refresh_checked(bs.checked, true);
            }
            Slot::Text => bs.text = rh,
        }
        crate::widget::ShownWrites::default()
    }

    /// The link's third argument (`0x77fd10`): every slot links its texture shown (the highlight
    /// `0x779139`, the label `0x778d49`) but the state and checked ones, which pass 0 and are
    /// shown by the switch that follows.
    fn links_shown(self) -> bool {
        matches!(self, Slot::Highlight | Slot::Text)
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

/// Which of the button's three embedded fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LabelFont {
    Normal,
    Highlight,
    Disabled,
}

impl LabelFont {
    fn of(self, f: ButtonFonts) -> FontId {
        match self {
            LabelFont::Normal => f.normal,
            LabelFont::Highlight => f.highlight,
            LabelFont::Disabled => f.disabled,
        }
    }
}

/// Button `h`'s three embedded fonts (normal `+0x33c`, highlight `+0x3b8`, disabled `+0x434`),
/// made empty on first use, which is the constructor's state; `None` for any frame but a Button.
pub(super) fn fonts(model: &mut Model, h: FrameHandle) -> Option<ButtonFonts> {
    match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Button(bs)) if bs.fonts.is_some() => return bs.fonts,
        Some(KindState::Button(_)) => {}
        _ => return None,
    }
    let made = ButtonFonts {
        normal: model.fonts.anonymous(),
        highlight: model.fonts.anonymous(),
        disabled: model.fonts.anonymous(),
    };
    if let Some(KindState::Button(bs)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        bs.fonts = Some(made);
    }
    Some(made)
}

/// [`fonts`] for a Lua receiver.
fn fonts_of(lua: &Lua, this: &Table) -> mlua::Result<(FrameHandle, ButtonFonts)> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let f = fonts(&mut model, h).ok_or_else(|| mlua::Error::runtime("not a Button"))?;
    Ok((h, f))
}

/// The label refresh for the loader, after a `<ButtonText>` or `<NormalText>` label has loaded
/// its own levels: the reference builds the string first (`0x7789d0` → `0x6f2780` → `0x770f40`)
/// and installs it after (`0x7789d9` → `0x778d20` → `0x778daa`), so the state's font is linked
/// over whatever font the string's own XML linked.
pub(crate) fn refresh_label_lua(lua: &Lua, wrapper: &Table) -> mlua::Result<()> {
    let h = frame_handle_of(lua, wrapper)?;
    refresh_label(&mut lua.app_data_mut::<Model>().expect("model app_data"), h);
    Ok(())
}

/// The loader's handle on one embedded font, which `<NormalFont>` and kin load into (`0x778baf`,
/// `0x778bf4` → `0x783c30`).
pub(crate) fn state_font_lua(lua: &Lua, wrapper: &Table, which: LabelFont) -> mlua::Result<FontId> {
    Ok(which.of(fonts_of(lua, wrapper)?.1))
}

/// `SetFontString`'s tail (`0x778d20`), which `SetText` also creates its label through
/// (`0x778dc0`): an unanchored label anchors LEFT, RIGHT or CENTER by the normal font's justify
/// word (`[button+0x390]`, the constructor's CENTER until the font holds one), never its own
/// (`+0x120`), then the label refresh links it to the state's font (`0x778daa`).
fn adopt_label(model: &mut Model, owner: FrameHandle, rh: RegionHandle) {
    let Some(f) = fonts(model, owner) else {
        return;
    };
    let word = model.fonts.get(f.normal).block.justify.unwrap_or_default();
    super::region::anchor_unanchored_at(model, rh, super::region::justify_anchor_point(word.0));
    refresh_label(model, owner);
}

/// The label refresh (`0x779810`) at the points where the reference runs it outside a state change
/// ([`ButtonState::refresh_label`]), its offset and its font; a no-op on any frame but a Button.
pub(super) fn refresh_label(model: &mut Model, h: FrameHandle) {
    if let Some(KindState::Button(bs)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        bs.refresh_label();
        bs.take_relink();
    }
    relink_label(model, h);
}

/// The label refresh's font half (`0x779810` → `0x770c60` on the label's `+0xcc`, `0x779878`): the
/// disabled font in state 0 (`0x77985c`), else the highlight font while the HIGHLIGHT layer is on
/// (`+0x1a8`, `0x77983c`), else the normal one. The link applies every group the font holds
/// through the label's mask, so a state font that holds no colour leaves the colour the last one
/// gave, and a label's own setter outlasts every state. Here the layer is on while the cursor is
/// over the button or `LockHighlight` holds, as the highlight texture draws. Relinking the font it
/// already follows does nothing (`0x770c6a`).
fn relink_label(model: &mut Model, h: FrameHandle) {
    let (rh, state, locked) = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Button(bs)) => match bs.text {
            Some(rh) => (rh, bs.button_state(), bs.locked_highlight),
            None => return,
        },
        _ => return,
    };
    let Some(f) = fonts(model, h) else {
        return;
    };
    let to = if state == ButtonVisualState::Disabled {
        f.disabled
    } else if locked || model.mouseover == Some(h) {
        f.highlight
    } else {
        f.normal
    };
    super::font::link_region(model, rh, Some(to));
}

/// The font relink a real state change owes the label (`0x7797fc` → `0x779810`); nothing for a
/// state that held (`0x7797a9`) or a call that moved no state, such as `SetChecked`.
fn relink_after_change(model: &mut Model, h: FrameHandle) {
    let due = match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        Some(KindState::Button(bs)) => bs.take_relink(),
        _ => false,
    };
    if due {
        relink_label(model, h);
    }
}

/// `Set{Text,Highlight,Disabled}FontObject(font | "font" | nil)` (`0x780400`, `0x780700`,
/// `0x780580`): the shared `0x79ef10` on that embedded font, whose texts name the button
/// (`0x87c5fc` and kin, `<unnamed>` for none), so a miss raises and no dead name is stored. nil
/// unlinks the font and keeps what it took.
fn link_state_font(
    lua: &Lua,
    this: &Table,
    which: LabelFont,
    args: &MultiValue,
) -> mlua::Result<()> {
    let who = super::binding_abi::name_of(lua, this);
    let to = super::font::font_arg(lua, &who, super::font::Verb::SetFontObject, args.front())?;
    let (_, f) = fonts_of(lua, this)?;
    let id = which.of(f);
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    if to.is_some_and(|to| super::font::would_loop(&model, id, to)) {
        return Err(mlua::Error::runtime(format!(
            "{who}:SetFontObject(): Can't create a font object loop"
        )));
    }
    super::font::link_font(&mut model, id, to);
    Ok(())
}

/// `Get{Text,Highlight,Disabled}FontObject()` (`0x7804c0`, `0x7807c0`, `0x780640` → `0x79f090`):
/// the font that embedded font follows, or nil.
fn state_font_object(lua: &Lua, this: &Table, which: LabelFont) -> mlua::Result<Value> {
    let (_, f) = fonts_of(lua, this)?;
    let parent = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .fonts
        .get(which.of(f))
        .parent;
    match parent {
        Some(p) => Ok(Value::Table(super::font::wrapper(lua, p)?)),
        None => Ok(Value::Nil),
    }
}

/// `Set{Text,Highlight,Disabled}TextColor(r, g, b [, a])` (`0x778e20`, `0x778ec0`, `0x778e70`):
/// the embedded font's colour, its own bit cleared and the change pushed to the label.
fn set_state_color(
    lua: &Lua,
    this: &Table,
    which: LabelFont,
    (r, g, b, a): (Value, Value, Value, Value),
) -> mlua::Result<()> {
    // Shape C on the channels (`Button:SetTextColor 0x780ee0`, `2=C 3=C 4=C 5=B`), packed as
    // every colour setter packs.
    let c = super::object::colour_args(lua, &r, &g, &b, &a);
    let (_, f) = fonts_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    super::font::set(&mut model, which.of(f), super::FontMask::COLOR, |fo| {
        fo.color = Some(c);
    });
    Ok(())
}

/// `Get{Text,Highlight,Disabled}TextColor()` (`0x781100`, `0x781800`, `0x781480`): four numbers,
/// the embedded font's colour (`+0x58`), its own or what its font pushed, else the constructor's
/// white; each byte times the float at `0x81c6b4`.
fn state_color(lua: &Lua, this: &Table, which: LabelFont) -> mlua::Result<mlua::MultiValue> {
    let (_, f) = fonts_of(lua, this)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let c = model
        .fonts
        .get(which.of(f))
        .block
        .color
        .unwrap_or(crate::color::Rgba8::WHITE);
    Ok(super::object::colour_values(c))
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

/// [`with_button`] for a state machine call that writes shown bits: the writes land in the regions
/// once the button's borrow is released.
fn with_button_shown(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut ButtonState) -> crate::widget::ShownWrites,
) -> mlua::Result<()> {
    let writes = with_button(lua, this, f)?;
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    super::region::apply_shown_writes(&mut model, writes);
    relink_after_change(&mut model, h);
    Ok(())
}

/// `Enable()`/`Disable()`. `Disable` (`0x77ffd0`) reaches `SetEnabled` (`0x779160`), which also
/// turns draw layer 4, HIGHLIGHT, off (`0x7791bb`) in the array `DisableDrawLayer` writes, so every
/// region in that layer goes dark, not only the HighlightTexture. `Enable()` here turns it back
/// on; the reference restores it only on a hovered button, by re-running its `<OnEnter>`
/// (`0x779183`), and never while `LockHighlight` holds (`[frame+0xf8]`).
fn set_enabled(lua: &Lua, this: &Table, on: bool) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    let writes = match &mut frame.kind_state {
        KindState::Button(bs) => bs.set_enabled(on),
        _ => return Err(mlua::Error::runtime("not a Button")),
    };
    let bit = 1u8 << DrawLayer::Highlight.index();
    if on {
        frame.disabled_layers &= !bit;
    } else {
        frame.disabled_layers |= bit;
    }
    super::region::apply_shown_writes(&mut model, writes);
    relink_after_change(&mut model, h);
    Ok(())
}

/// Run an engine state edge over frame `h` if it is a live Button. The reference has six, all of
/// `0x779790`'s call sites but Lua's: enable, disable, hide, press, release and drag start; the
/// cursor entering or leaving a button is not one.
pub(super) fn edge(
    model: &mut Model,
    h: FrameHandle,
    f: impl FnOnce(&mut ButtonState) -> crate::widget::ShownWrites,
) {
    let writes = match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        Some(KindState::Button(bs)) => f(bs),
        _ => return,
    };
    super::region::apply_shown_writes(model, writes);
    relink_after_change(model, h);
}

/// Whether a hover edge on `h` reaches the base notify, the HIGHLIGHT layer and the Lua
/// `<OnEnter>`/`<OnLeave>`: `OnEnter`/`OnLeave` (`0x779490`/`0x7794e0`) skip it on a DISABLED
/// Button, leaving only the hover sound (`0x7794b0`). Every hover path goes through that pair
/// (the hover walk `0x766218`, `SetMouseFocus` `0x764dc0`, the hide tail `0x764cce`), and the
/// Hyperlink and NamePlate overrides chain back (`0x7cb8a5`). The hover target still moves, so
/// `GetMouseFocus()` answers a disabled button.
pub(super) fn hover_notify_runs(model: &Model, h: Option<FrameHandle>) -> bool {
    h.is_none_or(|h| is_enabled(model, h))
}

/// Whether `h` is a Button out of the DISABLED state (`+0x328`); any other frame answers true.
pub(super) fn is_enabled(model: &Model, h: FrameHandle) -> bool {
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
            // Born with its shown bit clear, as `0x76fc40` zeroes it; the link writes the slot's
            // own (`0x77fd10`), and the store shows a current state's texture (`0x779037`).
            model.region_data.insert(
                rh,
                RegionData {
                    hidden: !slot.links_shown(),
                    ..RegionData::default()
                },
            );
            model.touch_layout_region(rh);
            let writes = match model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
                Some(KindState::Button(bs)) => slot.set(bs, Some(rh)),
                _ => Default::default(),
            };
            super::region::apply_shown_writes(&mut model, writes);
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

/// Install `incoming` (a Texture object) into `slot`, or clear it, as the slot store `0x778fd0`
/// does: the same texture is a no-op (`0x778fe7`), the displaced one is destroyed (`0x77900a`), and
/// the incoming one moves into the button on the slot's own draw layer (`0x779017`, `0x779139`
/// for the highlight) before it is stored.
fn install_slot(
    lua: &Lua,
    this: &Table,
    slot: Slot,
    incoming: Option<RegionHandle>,
) -> mlua::Result<bool> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    install_slot_model(&mut model, h, slot, incoming)
}

/// The loader's install of the texture a state-texture element built through the `<Layers>`
/// adder (`0x6f26f0`), never through a Lua setter: `Button::LoadXML` hands each new region to the
/// slot store (`0x77890e`, `0x778940`, `0x778972`; the highlight's `0x7789a9`; the checked pair's
/// `0x7851dd`, `0x78520e`), which destroys whatever the slot held, an inherited template's
/// included, so a handle still held on it goes dead (`0x701cd0` clears its `[0]`). The highlight's
/// store sets the region's own blend (`0x7789a4`), so it keeps it.
pub(super) fn install_loaded(
    model: &mut Model,
    h: FrameHandle,
    tag: &str,
    rh: RegionHandle,
) -> mlua::Result<()> {
    let slot = match tag {
        "NormalTexture" => Slot::Normal,
        "PushedTexture" => Slot::Pushed,
        "DisabledTexture" => Slot::Disabled,
        "HighlightTexture" => Slot::Highlight,
        "CheckedTexture" => Slot::Checked,
        "DisabledCheckedTexture" => Slot::DisabledChecked,
        _ => return Err(mlua::Error::runtime(format!("not a Button texture: {tag}"))),
    };
    install_slot_model(model, h, slot, Some(rh)).map(|_| ())
}

/// [`install_slot`] on the model.
fn install_slot_model(
    model: &mut Model,
    h: FrameHandle,
    slot: Slot,
    incoming: Option<RegionHandle>,
) -> mlua::Result<bool> {
    if !matches!(
        model.arena.frame(h).map(|f| &f.kind_state),
        Some(KindState::Button(_))
    ) {
        return Err(mlua::Error::runtime("not a Button"));
    }
    let writes = std::cell::RefCell::new(crate::widget::ShownWrites::default());
    let changed = super::region::install_into_slot(
        model,
        h,
        incoming,
        slot.shape().1,
        slot.links_shown(),
        false,
        |ks| match ks {
            KindState::Button(bs) => slot.get(bs),
            _ => None,
        },
        |ks, rh| {
            if let KindState::Button(bs) = ks {
                *writes.borrow_mut() = slot.set(bs, rh);
            }
        },
    )?;
    super::region::apply_shown_writes(model, writes.into_inner());
    Ok(changed)
}

/// `Set<State>Texture(texture | "path" | nil)`, forked on the argument's type (`0x781970`). The
/// object and nil legs never touch the slot's own region, so they run before [`ensure_slot`]
/// would create one. There is no colour form: a number takes the path leg through `lua_isstring`
/// (`0x781b23`) and loads as its own decimal name, so `(1, 1, 1, 0)` is the file `"1"`; anything
/// else, no argument included, raises `Usage: <name>:<usage>` (`0x87a1b4`).
fn set_slot_texture(
    lua: &Lua,
    this: &Table,
    slot: Slot,
    usage: &str,
    args: &MultiValue,
) -> mlua::Result<()> {
    // The highlight's third argument names its blend, ADD without one or on a name that does not
    // parse (`0x781d92`, `0x781db3` → `0x6f1920`); every other state has none.
    let blend = (slot == Slot::Highlight).then(|| match args.get(1) {
        Some(Value::String(n)) => n
            .to_str()
            .ok()
            .and_then(|n| crate::script::BlendMode::parse(&n))
            .unwrap_or(crate::script::BlendMode::Add),
        _ => crate::script::BlendMode::Add,
    });
    // A Texture object is itself installed into the slot (`0x781b0b` → `0x778fd0`; the highlight's
    // `0x781eb7` → `0x779110`, which also sets its blend, `0x779144`, unless it is the same
    // texture, `0x779122`). Any other widget raises (`0x781ab1`, message `0x87a1e0`; the
    // highlight's `0x87a28c`). nil clears the slot and destroys the old region (`0x781b5a` →
    // `0x778fd0`): unhooked but alive, it would draw in every state. A string or a number is a
    // path (`0x781b33`, `lua_tostring` converting a number as `%.14g`); no argument is not nil
    // (`0x781b4f`) and raises the usage.
    let method = usage.split('(').next().unwrap_or(usage);
    let path = match super::region::slot_arg(lua, this, args, usage, method, true)? {
        super::region::SlotArg::Object(rh) => {
            if install_slot(lua, this, slot, Some(rh))? {
                if let Some(blend) = blend {
                    lua.app_data_mut::<Model>()
                        .expect("model app_data")
                        .region_data
                        .entry(rh)
                        .or_default()
                        .blend = blend;
                }
            }
            return Ok(());
        }
        super::region::SlotArg::Clear => return install_slot(lua, this, slot, None).map(|_| ()),
        super::region::SlotArg::Path(path) => path,
    };
    // `0x770200` answers 0 for a name no candidate resolves and keeps what the region held
    // (`0x7702ac`). The slot's region made for this call is then destroyed without a blend or a
    // slot (`0x7790ce`, `0x7790fa`; `0x778f88`, `0x778fb7`), while an existing one (`0x77907c`) goes
    // on to the highlight's blend.
    // Deviation: the disabled-checked leg (`0x785380`) tests and reloads the checked slot
    // (`[+0x4e0]`, `0x785388`/`0x7853a5`), a copy of the checked leg's (`0x785270`), so in 1.12.1
    // a check box with a checked texture repaints that one and never gets its disabled-checked
    // texture from a path. benilla tests and reloads the slot the method names.
    let existing = with_button(lua, this, |bs| slot.get(bs))?.is_some();
    // A slot built for this call is judged before it is built, the probe asked once.
    let verdict = if existing {
        None
    } else {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        match crate::script::texture_name_verdict(&model, &path) {
            crate::script::FileLoad::Failed => return Ok(()),
            v => Some(v),
        }
    };
    let id = ensure_slot(lua, this, slot)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let rh = *model.id_to_region.get(&id).expect("slot region id");
    // `""` clears the texture, which `QuestLogFrame.lua:165` relies on; a file that loads clears
    // the desaturation, and one that fails keeps the image and the grey (`0x778f3a` → `0x770200`).
    match verdict {
        Some(v) => crate::script::apply_texture_name(&mut model, rh, &path, false, v),
        None => {
            crate::script::load_texture_name(&mut model, rh, &path, false);
        }
    }
    let data = model.region_data.entry(rh).or_default();
    // The highlight applies its blend on every path call, to the old region as to a new one
    // (`0x779082`, `0x7790e6`), the old one even when its texture did not load. A checked
    // texture made here is ADD (`0x7852f8`, `0x785408`), a reloaded one keeps its blend
    // (`0x785295`); the state textures keep the constructor's BLEND (`0x778f10`).
    if let Some(blend) = blend {
        data.blend = blend;
    } else if !existing && matches!(slot, Slot::Checked | Slot::DisabledChecked) {
        data.blend = crate::script::BlendMode::Add;
    }
    model.note_art(rh);
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
    let method = format!("Set{name}Texture");
    // The highlight setter's own line names its blend argument (`0x87a240`), unclosed as in the
    // reference; the others share `0x87a1b4`.
    let usage = if slot == Slot::Highlight {
        format!("{method}(texture or \"texture\" or nil [, \"blendmode\")")
    } else {
        format!("{method}(texture or \"texture\" or nil)")
    };
    m.set(
        method.as_str(),
        lua.create_function(move |lua, (this, args): (Table, MultiValue)| {
            set_slot_texture(lua, &this, slot, &usage, &args)
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
            // The label's `SetText` (`0x778e10` into `0x771d80`) runs the separator pass.
            let text = model.localized_text(text);
            let data = model.region_data.entry(rh).or_default();
            let was_shown = data.text.as_deref().is_some_and(|t| !t.is_empty());
            data.text = Some(text);
            model.touch_measure(rh);
            model.touch_layout_text(rh, was_shown);
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
                super::binding_abi::frame_name(&lua.app_data_ref::<Model>().expect("model"), h)
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
            super::region::link_region(&mut model, rh, owner, DrawLayer::Artwork, true);
            adopt_label(&mut model, owner, rh);
            // A new owner, so a new scale for its node, and `GetTextWidth` reads its extents.
            model.touch_layout_region(rh);
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

    // The three embedded fonts, one verb family each: the font object, the text colour.
    for (which, set_obj, get_obj, set_color, get_color) in [
        (
            LabelFont::Normal,
            "SetTextFontObject",
            "GetTextFontObject",
            "SetTextColor",
            "GetTextColor",
        ),
        (
            LabelFont::Highlight,
            "SetHighlightFontObject",
            "GetHighlightFontObject",
            "SetHighlightTextColor",
            "GetHighlightTextColor",
        ),
        (
            LabelFont::Disabled,
            "SetDisabledFontObject",
            "GetDisabledFontObject",
            "SetDisabledTextColor",
            "GetDisabledTextColor",
        ),
    ] {
        m.set(
            set_obj,
            lua.create_function(move |lua, (this, args): (Table, MultiValue)| {
                link_state_font(lua, &this, which, &args)
            })?,
        )?;
        m.set(
            get_obj,
            lua.create_function(move |lua, this: Table| state_font_object(lua, &this, which))?,
        )?;
        m.set(
            set_color,
            lua.create_function(
                move |lua, (this, r, g, b, a): (Table, Value, Value, Value, Value)| {
                    set_state_color(lua, &this, which, (r, g, b, a))
                },
            )?,
        )?;
        m.set(
            get_color,
            lua.create_function(move |lua, this: Table| state_color(lua, &this, which))?,
        )?;
    }

    // SetFont(file, height [, flags]) (`0x780880`) sets the normal, disabled and highlight fonts in
    // that order (`0x780927`, `0x78094e`, `0x78096e`), each through the shared `0x79f210`, and
    // returns nothing. The label takes it by the push, as it follows one of the three; with no
    // label there is nothing to push to. The arguments pass the shared gate, which names the
    // button in its usage line.
    m.set(
        "SetFont",
        lua.create_function(
            |lua, (this, file, height, flags): (Table, Value, Value, Value)| {
                let super::font_block::FontArgs {
                    path,
                    height,
                    flags,
                    monochrome,
                } = super::font_block::set_font_args(lua, &this, &file, &height, &flags)?;
                // Each embedded font's load fails alike, writing nothing (`0x79f30f` → `0x79f361`).
                if !super::font_block::instance_loads(lua, &path, height) {
                    return Ok(());
                }
                let (_, f) = fonts_of(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                for id in [f.normal, f.disabled, f.highlight] {
                    super::font::set(&mut model, id, super::FontMask::FONT, |fo| {
                        fo.font = Some(path.clone());
                        fo.height = Some(height);
                        fo.outline = flags;
                        fo.monochrome = monochrome;
                    });
                }
                Ok(())
            },
        )?,
    )?;
    // GetFont() (`0x7809a0` → `0x79f3b0`) → file, height, flags off the normal font alone
    // (`0x780a41`): its own, or what it follows pushed; nil, 0, "" when it holds none.
    m.set(
        "GetFont",
        lua.create_function(|lua, this: Table| {
            let (_, f) = fonts_of(lua, &this)?;
            let block = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.fonts.get(f.normal).block.clone()
            };
            let path = match block.font {
                Some(p) => Value::String(lua.create_string(&p)?),
                None => Value::Nil,
            };
            Ok((
                path,
                block.height.unwrap_or(0.0),
                block.outline.flags_text(block.monochrome),
            ))
        })?,
    )?;

    // LockHighlight (`0x782840`) and UnlockHighlight: the highlight shows regardless of hover,
    // then the label refresh (`0x495a98`).
    for (name, on) in [("LockHighlight", true), ("UnlockHighlight", false)] {
        m.set(
            name,
            lua.create_function(move |lua, this: Table| {
                let h = frame_handle_of(lua, &this)?;
                with_button(lua, &this, |bs| bs.locked_highlight = on)?;
                refresh_label(&mut lua.app_data_mut::<Model>().expect("model app_data"), h);
                Ok(())
            })?,
        )?;
    }

    // SetPushedTextOffset(x, y) (`0x782030`): two numbers (`lua_isnumber`, `0x7820c2`/`0x7820d3`),
    // else `Usage: <name>:SetPushedTextOffset(x, y)` (`0x87a308`); each stored in layout units
    // as an anchor offset is (`0x7820eb`-`0x78212b`). The label moves only at its next refresh.
    m.set(
        "SetPushedTextOffset",
        lua.create_function(|lua, (this, x, y): (Table, Value, Value)| {
            let num = |v: Value| lua.coerce_number(v).ok().flatten();
            let (Some(x), Some(y)) = (num(x), num(y)) else {
                frame_handle_of(lua, &this)?;
                return Err(super::binding_abi::usage(
                    lua,
                    &this,
                    "SetPushedTextOffset(x, y)",
                ));
            };
            let units = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .layout_units();
            with_button(lua, &this, |bs| {
                bs.pushed_text_offset = [units.store(x as f32), units.store(y as f32)];
            })
        })?,
    )?;
    // GetPushedTextOffset() (`0x782180`) → x, y, read back through the layout scale
    // (`0x782237`-`0x78227a`): 1.28 each way on a fresh button at 4:3.
    m.set(
        "GetPushedTextOffset",
        lua.create_function(|lua, this: Table| {
            let [x, y] = with_button(lua, &this, |bs| bs.pushed_text_offset)?;
            let units = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .layout_units();
            Ok((units.load(x), units.load(y)))
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
            with_button_shown(lua, &this, |bs| bs.set_button_state(new, locked))
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

    // Click([button]) (`0x7826c0`): the physical click's path, toggle and `OnClick` alike, with
    // the argument mapped to the name the click fires ([`click_arg_name`]).
    m.set(
        "Click",
        lua.create_function(|lua, (this, button): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            let id = {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.frame_id(h)
            };
            // A released click, flagged as scripted, which only a LootButton reads.
            click_button(lua, id, click_arg_name(&button), false, true);
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
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            // `SetChecked` (`0x799bf0`) reads its argument through `GetBoolOrDefault` (`0x6f1c10`)
            // with default 1 (`0x799c77`), not Lua truthiness: no argument, `""`, a table or
            // `"yes"` check, `nil`, `0` and `"0"` uncheck. The stock UI passes `"true"`/`"false"`
            // (`SpellBookFrame.lua:296-303`).
            let checked = super::binding_abi::bool_or_default(args.front(), true);
            with_button_shown(lua, &this, |bs| bs.refresh_checked(checked, false))
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

/// `Click`'s argument as `0x7826c0` maps it, then named as the base click names its mask
/// (`0x779540`, table `0x77960c`). A string is compared ignoring case (`0x64a4c0`) with
/// `LeftButton`, `MiddleButton` and `RightButton` (1, 2, 4); any other string, `""` and
/// `"Button4"` included, is 0, which fires `UNKNOWN`. A number passes `lua_isstring` (`0x6f3510`)
/// and its decimal text matches none, so it is `UNKNOWN` too. No argument, nil, a boolean, a
/// table or a function is `LeftButton` (`0x782766`).
fn click_arg_name(arg: &Value) -> &'static str {
    match arg {
        Value::String(s) => {
            let b = s.as_bytes();
            ["LeftButton", "MiddleButton", "RightButton"]
                .into_iter()
                .find(|n| b.eq_ignore_ascii_case(n.as_bytes()))
                .unwrap_or("UNKNOWN")
        }
        Value::Integer(_) | Value::Number(_) => "UNKNOWN",
        _ => "LeftButton",
    }
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

/// The state half of the release click gate (`0x7792fc`): a Button clicks only while PUSHED, which
/// the press art sets and a drag start, a hide or a lost press clears; any other kind has no state.
pub(super) fn release_may_click(model: &Model, h: FrameHandle) -> bool {
    match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Button(bs)) => bs.button_state() == ButtonVisualState::Pushed,
        _ => true,
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

/// The click shared by the input path and `Click()`: a CheckButton toggles first, enabled or not
/// (`0x785550`), a disabled Button fires nothing more, then `OnClick` gets the button name alone
/// as `arg1` (`0x779540` fires it with the format `"%s"`); `down`, true only for a click a
/// `…ButtonDown` registration fired, reaches the nameplate's select alone. The mouse never gets
/// here for a disabled button: its press slot returns first (`0x77922e`), its release is gated on
/// PUSHED. `scripted` marks a Lua `Click()`
/// (`0x7826c0` passes 1, the mouse `0x779280`/`0x7793a4` pass 0): a LootButton then does nothing at
/// all, not even its `OnClick` (`0x4c182b`); every other kind ignores it.
pub(super) fn click_button(lua: &Lua, id: u32, button: &str, down: bool, scripted: bool) {
    // A hyperlink button's click slot (`+0x94`, `0x7a3510`) is its text frame's
    // `OnHyperlinkClick`, in place of the Button's `OnClick` firer.
    let target = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .id_to_frame
        .get(&id)
        .copied();
    if target.is_some_and(|h| super::hyperlink::click(lua, h, button)) {
        return;
    }
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
        let (fire, writes) = match &mut frame.kind_state {
            // A CheckButton's click slot (`0x785550`) toggles (`0x785566`) before the base click
            // (`0x779540`), which runs nothing for a disabled button (`0x779552`): the toggle has
            // no state gate, so `Click()` flips a disabled box silently.
            KindState::Button(bs) => {
                let writes = if is_check {
                    bs.refresh_checked(!bs.checked, false)
                } else {
                    Default::default()
                };
                (bs.enabled(), writes)
            }
            // any other kind with an `OnClick` just fires.
            _ => (true, Default::default()),
        };
        super::region::apply_shown_writes(&mut model, writes);
        fire
    };
    if !fire {
        return;
    }
    let btn = match lua.create_string(button) {
        Ok(s) => Value::String(s),
        Err(_) => return,
    };
    if let Err(e) = event::fire_widget_handler(lua, id, "OnClick", vec![btn]) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_lua_error(&e);
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
