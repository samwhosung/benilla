//! Font objects: a `<Font name=…>` or a `CreateFont` font is a Lua object, published as the global
//! of its name with its own method table. Stock FrameXML never calls this API; addons do.
//!
//! The reference's class is `CSimpleFont` (`0x87a454`). Its method table (`0x87c7c8`) has 22
//! entries (`mov edx,0x16` at `0x7a10d5`), and its lookup `0x7a1100` has no base-class fallback.
//! All are here; `CopyFontObject` is the one a FontString lacks. A named font is published by `0x783870` through `SetName 0x784150` and `CreateLuaHandle
//! 0x701bd0`, which leaves a non-nil `_G[name]` alone (`0x701cb8`).
//!
//! The live link. Every font follows at most one font (`+0x28`) and keeps the list of what follows
//! it, fonts and FontStrings alike (`+0x74`). Linking (`0x770c60`) applies every group the font
//! holds; each setter writes its group, clears its own inherit bit (`+0x2c`) and pushes what
//! changed down the list (`PushChanges 0x784180`), a font follower merging it into its own block
//! (`0x77e4b0` → `0x770910`) and pushing on in turn, a FontString applying it (`0x773530` →
//! `0x770800`). Both stop at a group the follower's mask has dropped ([`FontMask`]), so a colour
//! set on a FontString survives every later push and `SetFontObject`. A push runs only from a
//! setter or a link, never per frame.

use std::collections::HashMap;

use mlua::{Lua, Table, Value};

use super::binding_abi;
use super::object::publish_global;
use crate::justify;
use crate::widget::RegionHandle;

use super::{FontMask, FontObject, FontShadow, Model, RegionData};

/// The shared metatable of every font-object handle.
const REG_FONT_META: &str = "__benilla_font_meta";
/// The font-object method table, which is the metatable's `__index`.
const REG_FONT_METHODS: &str = "__benilla_font_methods";
/// font id → handle, so `GameFontNormal == GameFontNormal` and a re-declared `<Font>` keeps identity.
const REG_FONT_WRAPPERS: &str = "__benilla_font_wrappers";

// ── The registry ─────────────────────────────────────────────────────────────────────────────

/// One `CSimpleFont`: a named object or a widget's own (a Button's three).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FontId(u32);

/// A font, its block and its two links.
#[derive(Clone, Debug, Default)]
pub(crate) struct FontNode {
    /// The name it was declared under, as typed; `None` for a widget's own font.
    pub(crate) name: Option<String>,
    /// The block (`+0x38`): its own groups merged over what it follows.
    pub(crate) block: FontObject,
    /// The font it follows (`+0x28`).
    pub(crate) parent: Option<FontId>,
    /// The groups it still takes from [`Self::parent`] (`+0x2c`).
    pub(crate) mask: FontMask,
    /// The fonts that follow it.
    font_followers: Vec<FontId>,
    /// The FontStrings that follow it: each region whose [`RegionData::font_object`] is this font.
    region_followers: Vec<RegionHandle>,
}

/// Every font, by id, and the named ones by their case-folded name: the font hash `0x783870`
/// hashes upper-cased (`SStrHashHT 0x64b3f0`) and compares with `SStrCmpI` (`0x7838c7`).
#[derive(Clone, Debug, Default)]
pub(crate) struct Fonts {
    nodes: Vec<FontNode>,
    by_lower: HashMap<String, FontId>,
}

impl Fonts {
    /// The named font `name`, matched ignoring ASCII case.
    pub(crate) fn id(&self, name: &str) -> Option<FontId> {
        if name.bytes().any(|b| b.is_ascii_uppercase()) {
            self.by_lower.get(&name.to_ascii_lowercase()).copied()
        } else {
            self.by_lower.get(name).copied()
        }
    }

    /// The named font `name`, made empty if none is (`0x783870` with its create flag); an
    /// existing one is returned unchanged (`0x7839ab`).
    pub(crate) fn named(&mut self, name: &str) -> FontId {
        if let Some(id) = self.id(name) {
            return id;
        }
        let id = self.push_node(Some(name.to_string()));
        self.by_lower.insert(name.to_ascii_lowercase(), id);
        id
    }

    /// A new unnamed font, a widget's own.
    pub(crate) fn anonymous(&mut self) -> FontId {
        self.push_node(None)
    }

    fn push_node(&mut self, name: Option<String>) -> FontId {
        let id = FontId(u32::try_from(self.nodes.len()).expect("fewer than 2^32 fonts"));
        self.nodes.push(FontNode {
            name,
            ..FontNode::default()
        });
        id
    }

    pub(crate) fn get(&self, id: FontId) -> &FontNode {
        &self.nodes[id.0 as usize]
    }

    fn get_mut(&mut self, id: FontId) -> &mut FontNode {
        &mut self.nodes[id.0 as usize]
    }

    /// The block of the named font `name`.
    pub(crate) fn block_of(&self, name: &str) -> Option<&FontObject> {
        self.id(name).map(|id| &self.get(id).block)
    }

    /// Drop `rh` from the follower list of `id`, as a dying FontString detaches (`0x773490`).
    pub(crate) fn drop_region(&mut self, id: FontId, rh: RegionHandle) {
        let list = &mut self.get_mut(id).region_followers;
        if let Some(i) = list.iter().position(|&r| r == rh) {
            list.swap_remove(i);
        }
    }
}

// ── The handle ───────────────────────────────────────────────────────────────────────────────

/// Get or create the Lua handle of font `id`, the one table its every reader gets back.
///
/// `T[0]` holds the font's id as an integer, never a light userdata, so a font handle passed
/// where a frame is wanted is rejected rather than decoded.
pub(crate) fn wrapper(lua: &Lua, id: FontId) -> mlua::Result<Table> {
    let wrappers: Table = lua.named_registry_value(REG_FONT_WRAPPERS)?;
    if let Value::Table(t) = wrappers.raw_get::<Value>(i64::from(id.0))? {
        return Ok(t);
    }
    let t = lua.create_table()?;
    t.raw_set(0, i64::from(id.0))?;
    let meta: Table = lua.named_registry_value(REG_FONT_META)?;
    t.set_metatable(Some(meta))?;
    wrappers.raw_set(i64::from(id.0), t.clone())?;
    Ok(t)
}

/// Publish named font `id`'s handle as `_G[name]`, leaving a non-nil global alone as
/// [`publish_global`] does.
pub(crate) fn publish(lua: &Lua, id: FontId, name: &str) -> mlua::Result<()> {
    let t = wrapper(lua, id)?;
    publish_global(lua, name, &t)
}

/// The font a handle holds in `T[0]`; `None` for any other table.
pub(super) fn id_of_table(model: &Model, t: &Table) -> Option<FontId> {
    match t.raw_get::<Value>(0).ok()? {
        Value::Integer(i) => u32::try_from(i)
            .ok()
            .map(FontId)
            .filter(|id| (id.0 as usize) < model.fonts.nodes.len()),
        _ => None,
    }
}

/// The font `this` is, for a Font method's receiver.
fn id_of(lua: &Lua, this: &Table) -> mlua::Result<FontId> {
    let model = lua.app_data_ref::<Model>().expect("model");
    id_of_table(&model, this)
        .ok_or_else(|| mlua::Error::runtime("not a font object (missing T[0] font identity)"))
}

/// Which font verb an argument error names.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Verb {
    /// `SetFontObject` (`0x79ef10`, the FontString's `0x79d1a0`): nil unlinks.
    SetFontObject,
    /// `CopyFontObject` (`0x79f0e0`): nil is a usage error.
    CopyFontObject,
}

/// A font argument as the reference reads it (`0x79ef10`, `0x79d1a0`, `0x79f0e0`), `who` being the
/// receiver's [`super::binding_abi::object_name`]: a table must hold a font in `this`
/// (`0x87c664`), not another object (`0x87c62c`); a string, and only a string (`lua_type` 4, so not a number), names a font
/// (`0x783870`), a miss raising (`0x87c5fc`); nil is `None` for `SetFontObject`; anything else,
/// no argument included, raises the usage line (`0x87c5cc`, `0x87c8e4`).
pub(super) fn font_arg(
    lua: &Lua,
    who: &str,
    verb: Verb,
    arg: Option<&Value>,
) -> mlua::Result<Option<FontId>> {
    let v = match verb {
        Verb::SetFontObject => "SetFontObject",
        Verb::CopyFontObject => "CopyFontObject",
    };
    let model = lua.app_data_ref::<Model>().expect("model");
    match arg {
        Some(Value::Table(t)) => match t.raw_get::<Value>(0)? {
            Value::Integer(_) | Value::LightUserData(_) => {
                id_of_table(&model, t).map(Some).ok_or_else(|| {
                    mlua::Error::runtime(format!("{who}:{v}(): Wrong object type, expected font"))
                })
            }
            _ => Err(mlua::Error::runtime(format!(
                "{who}:{v}(): Couldn't find 'this' in font object"
            ))),
        },
        Some(Value::String(s)) => {
            let name = s.to_string_lossy();
            model.fonts.id(&name).map(Some).ok_or_else(|| {
                mlua::Error::runtime(format!("{who}:{v}(): Couldn't find font named {name}"))
            })
        }
        Some(Value::Nil) if verb == Verb::SetFontObject => Ok(None),
        _ => Err(mlua::Error::runtime(match verb {
            Verb::SetFontObject => format!("Usage: {who}:SetFontObject(font or \"font\" or nil)"),
            Verb::CopyFontObject => format!("Usage: {who}:CopyFontObject(font or \"font\")"),
        })),
    }
}

// ── The live link ────────────────────────────────────────────────────────────────────────────

/// `ApplyFontBlock 0x770800`: write each of `groups` that the font holds (its set bits, `0x770826`)
/// and the region's mask passes (`word & mask`, `0x770824`). An empty `CreateFont` font holds
/// nothing, so it changes nothing.
pub(crate) fn apply(d: &mut RegionData, fo: &FontObject, groups: FontMask) {
    let g = groups & d.font_mask;
    // Face, height and flags load together (`0x7714f0`, `0x770848`), the height into `+0xe4`.
    if g.has(FontMask::FONT) {
        if let Some(f) = &fo.font {
            d.font_path = Some(f.clone());
            if let Some(h) = fo.height {
                d.font_height = Some(h);
            }
            d.outline = fo.outline;
            d.monochrome = fo.monochrome;
        }
    }
    // `(~mask & word) | (font & mask)` (`0x770868`, `0x770876`).
    if g.has(FontMask::JUSTIFY) {
        if let Some(j) = fo.justify {
            let m = d.justify_mask.0;
            d.justify.0 = (d.justify.0 & !m) | (j.0 & m);
        }
    }
    if g.has(FontMask::COLOR) {
        if let Some(c) = fo.color {
            d.set_vertex_color(Some(c));
        }
    }
    if g.has(FontMask::SHADOW) {
        if let Some(s) = fo.shadow {
            d.font_shadow = Some(s);
        }
    }
    // Copied as given: a FontString's reader clamps it, as its setter `0x772240` does at the merge
    // (`0x7708fd`).
    if g.has(FontMask::SPACING) {
        if let Some(sp) = fo.spacing {
            d.spacing = sp;
        }
    }
}

/// The groups `fo` holds, its set bits (`0x100`..`0x1000` of `+0x38`).
pub(crate) fn held(fo: &FontObject) -> FontMask {
    let mut m = FontMask::NONE;
    for (g, on) in [
        (FontMask::FONT, fo.font.is_some()),
        (FontMask::JUSTIFY, fo.justify.is_some()),
        (FontMask::COLOR, fo.color.is_some()),
        (FontMask::SHADOW, fo.shadow.is_some()),
        (FontMask::SPACING, fo.spacing.is_some()),
    ] {
        if on {
            m.set(g);
        }
    }
    m
}

/// The groups of `groups` in which `a` and `b` differ: a font by name ignoring case, height and
/// spacing by 2^-22 (`0x770973`, `0x77097c`, `0x770a84`), the rest exactly.
fn differs(a: &FontObject, b: &FontObject, groups: FontMask) -> FontMask {
    let close = |x: Option<f32>, y: Option<f32>| match (x, y) {
        (Some(x), Some(y)) => (x - y).abs() < f32::from_bits(0x3480_0000),
        (None, None) => true,
        _ => false,
    };
    let mut m = FontMask::NONE;
    let font_eq = match (&a.font, &b.font) {
        (Some(x), Some(y)) => {
            x.eq_ignore_ascii_case(y)
                && close(a.height, b.height)
                && a.outline == b.outline
                && a.monochrome == b.monochrome
        }
        (None, None) => true,
        _ => false,
    };
    for (g, eq) in [
        (FontMask::FONT, font_eq),
        (FontMask::JUSTIFY, a.justify == b.justify),
        (FontMask::COLOR, a.color == b.color),
        (FontMask::SHADOW, a.shadow == b.shadow),
        (FontMask::SPACING, close(a.spacing, b.spacing)),
    ] {
        if groups.has(g) && !eq {
            m.set(g);
        }
    }
    m
}

/// Copy the groups of `groups` that `src` holds into `dst`: [`differs`] picks what changed.
fn copy_groups(dst: &mut FontObject, src: &FontObject, groups: FontMask) {
    if groups.has(FontMask::FONT) {
        dst.font.clone_from(&src.font);
        dst.height = src.height;
        dst.outline = src.outline;
        dst.monochrome = src.monochrome;
    }
    if groups.has(FontMask::JUSTIFY) {
        dst.justify = src.justify;
    }
    if groups.has(FontMask::COLOR) {
        dst.color = src.color;
    }
    if groups.has(FontMask::SHADOW) {
        dst.shadow = src.shadow;
    }
    if groups.has(FontMask::SPACING) {
        dst.spacing = src.spacing;
    }
}

/// `FontBlockMergeMasked 0x770910` into font `id` from `src`: each of `groups` that `src` holds,
/// that `id`'s mask passes, and that differs or is not yet held (`0x7709f2`-`0x7709fc`). Answers
/// the groups taken, which `id` pushes on.
fn merge_into(model: &mut Model, id: FontId, src: &FontObject, groups: FontMask) -> FontMask {
    let node = model.fonts.get_mut(id);
    let g = groups & node.mask & held(src);
    let took = differs(&node.block, src, g);
    copy_groups(&mut node.block, src, took);
    took
}

/// `PushChanges 0x784180`: hand the groups of font `id` that `changed` names to every follower, a
/// font merging and pushing on (`0x77e4b0`), a FontString applying them (`0x773530`). Nothing
/// runs for no change, so an equal set costs one compare.
pub(crate) fn push(model: &mut Model, id: FontId, changed: FontMask) {
    if changed.is_empty() {
        return;
    }
    let node = model.fonts.get(id);
    let src = node.block.clone();
    let fonts = node.font_followers.clone();
    let regions = node.region_followers.clone();
    for f in fonts {
        let took = merge_into(model, f, &src, changed);
        push(model, f, took);
    }
    for rh in regions {
        apply_to_region(model, rh, id, &src, changed);
    }
}

/// [`apply`] onto follower `rh` of font `id`, then re-measure it when a measured group arrived.
fn apply_to_region(
    model: &mut Model,
    rh: RegionHandle,
    id: FontId,
    src: &FontObject,
    groups: FontMask,
) {
    let Some(d) = model.region_data.get_mut(&rh) else {
        return;
    };
    if d.font_object != Some(id) {
        return;
    }
    let landed = groups & d.font_mask & held(src);
    apply(d, src, groups);
    if !(landed & (FontMask::FONT | FontMask::SHADOW | FontMask::SPACING)).is_empty() {
        model.touch_measure(rh);
    }
}

/// `LinkFontFollower 0x770c60` for a FontString: follow `to` (nil only unlinks, `0x770cb8`),
/// applying every group it holds through the region's mask. The same font again does nothing
/// (`0x770c6a`). Answers whether the link moved.
pub(crate) fn link_region(model: &mut Model, rh: RegionHandle, to: Option<FontId>) -> bool {
    let d = model.region_data.entry(rh).or_default();
    let old = d.font_object;
    if old == to {
        return false;
    }
    d.font_object = to;
    if let Some(old) = old {
        model.fonts.drop_region(old, rh);
    }
    if let Some(to) = to {
        model.fonts.get_mut(to).region_followers.push(rh);
        let src = model.fonts.get(to).block.clone();
        apply_to_region(model, rh, to, &src, FontMask::ALL);
    }
    true
}

/// Whether font `to` is `id` or follows it, however far: linking `id` to it would close a loop
/// (`0x79f045`).
pub(crate) fn would_loop(model: &Model, id: FontId, to: FontId) -> bool {
    let mut at = Some(to);
    // Bounded by the font count: no chain can be longer without already looping.
    for _ in 0..=model.fonts.nodes.len() {
        match at {
            Some(f) if f == id => return true,
            Some(f) => at = model.fonts.get(f).parent,
            None => return false,
        }
    }
    true
}

/// [`link_region`] for a font: follow `to`, merging every group it holds through `id`'s mask
/// (`0x77e4b0` with all five marked changed) and pushing what that changed. A caller rules out a
/// loop first ([`would_loop`]).
pub(crate) fn link_font(model: &mut Model, id: FontId, to: Option<FontId>) {
    let old = model.fonts.get(id).parent;
    if old == to {
        return;
    }
    if let Some(old) = old {
        let list = &mut model.fonts.get_mut(old).font_followers;
        if let Some(i) = list.iter().position(|&f| f == id) {
            list.swap_remove(i);
        }
    }
    model.fonts.get_mut(id).parent = to;
    if let Some(to) = to {
        model.fonts.get_mut(to).font_followers.push(id);
        let src = model.fonts.get(to).block.clone();
        let took = merge_into(model, id, &src, FontMask::ALL);
        push(model, id, took);
    }
}

/// One setter's write on font `id` (`0x79f210`, `0x79f4d0` and kin): `f` writes `group`, the
/// font's own bit of it is cleared for good (`0x79f33a`, `0x79f669`, …), and what changed is
/// pushed. A value equal to the held one changes nothing (`0x79f65a`).
pub(crate) fn set<R>(
    model: &mut Model,
    id: FontId,
    group: FontMask,
    f: impl FnOnce(&mut FontObject) -> R,
) -> R {
    let node = model.fonts.get_mut(id);
    let before = node.block.clone();
    let out = f(&mut node.block);
    node.mask.clear(group);
    let changed = differs(&before, &node.block, group);
    push(model, id, changed);
    out
}

/// Replace font `id`'s block whole and push what that changed; its mask and parent stay.
pub(crate) fn replace_block(model: &mut Model, id: FontId, block: FontObject) {
    let changed = differs(&model.fonts.get(id).block, &block, FontMask::ALL);
    model.fonts.get_mut(id).block = block;
    push(model, id, changed);
}

/// Run `f` on the receiver's block as [`set`] does: every setter's one write path.
fn edit<R>(
    lua: &Lua,
    this: &Table,
    group: FontMask,
    f: impl FnOnce(&mut FontObject) -> R,
) -> mlua::Result<R> {
    let id = id_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    Ok(set(&mut model, id, group, f))
}

/// A copy of the receiver's block.
fn read(lua: &Lua, this: &Table) -> mlua::Result<FontObject> {
    let id = id_of(lua, this)?;
    let model = lua.app_data_ref::<Model>().expect("model");
    Ok(model.fonts.get(id).block.clone())
}

// ── install: the method table, the metatable and CreateFont ──────────────────────────────────

/// Build the font-object method table and metatable, and register `CreateFont`.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.set_named_registry_value(REG_FONT_WRAPPERS, lua.create_table()?)?;

    let m = lua.create_table()?;

    // ── identity ────────────────────────────────────────────────────────────────────────────
    m.set(
        "GetObjectType",
        lua.create_function(|_, _this: Table| Ok("Font"))?,
    )?;
    // `1` or nil, never a boolean, like the Region's (`0x7a1290`): the Font's own `IsObjectType`
    // (`0x79fe60`) answers a number. Its argument handling is untraced; mlua's coercion applies.
    m.set(
        "IsObjectType",
        lua.create_function(|_, (_this, ty): (Table, String)| {
            Ok(binding_abi::flag(ty.eq_ignore_ascii_case("font")))
        })?,
    )?;
    // GetName(): the declared name, nil for a widget's own font.
    m.set(
        "GetName",
        lua.create_function(|lua, this: Table| {
            let id = id_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.fonts.get(id).name.clone())
        })?,
    )?;

    // ── the font-object-on-font-object pair ─────────────────────────────────────────────────
    // SetFontObject(font | "font" | nil) (`0x7a0030` → `0x79ef10`): follow that font, raising on
    // a loop (`0x79f045`, `0x87c8b0`); nil unlinks and keeps the block.
    m.set(
        "SetFontObject",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let who = super::binding_abi::name_of(lua, &this);
            let to = font_arg(lua, &who, Verb::SetFontObject, args.front())?;
            let id = id_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if to.is_some_and(|to| would_loop(&model, id, to)) {
                return Err(mlua::Error::runtime(format!(
                    "{who}:SetFontObject(): Can't create a font object loop"
                )));
            }
            link_font(&mut model, id, to);
            Ok(())
        })?,
    )?;
    // CopyFontObject(font | "font") (`0x7a01b0` → `0x79f0e0`): nil is a usage error. The copy is
    // the source's block taken whole and pushed; what `0x7841d0` does to the mask and the parent
    // is not modelled.
    m.set(
        "CopyFontObject",
        lua.create_function(|lua, (this, args): (Table, mlua::MultiValue)| {
            let who = super::binding_abi::name_of(lua, &this);
            let src = font_arg(lua, &who, Verb::CopyFontObject, args.front())?
                .expect("CopyFontObject refuses nil");
            let id = id_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let paint = model.fonts.get(src).block.clone();
            replace_block(&mut model, id, paint);
            Ok(())
        })?,
    )?;
    // GetFontObject() (`0x7a00f0` → `0x79f090`): the font this one follows, or nil.
    m.set(
        "GetFontObject",
        lua.create_function(|lua, this: Table| {
            let id = id_of(lua, &this)?;
            let parent = lua
                .app_data_ref::<Model>()
                .expect("model")
                .fonts
                .get(id)
                .parent;
            match parent {
                Some(p) => Ok(Value::Table(wrapper(lua, p)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // ── face ────────────────────────────────────────────────────────────────────────────────
    // SetFont(path, height [, flags]) → 1, or nil when the font fails to load, which does not raise
    // and writes nothing: an empty path, one the host's probe refuses, or a height of 0 or less.
    m.set(
        "SetFont",
        lua.create_function(
            |lua, (this, file, height, flags): (Table, Value, Value, Value)| {
                // The argument gate every kind shares (`0x79f210`); a bad path or height raises
                // (`0x87c69c`), and anything but a string or a number is no flags.
                let super::font_block::FontArgs {
                    path,
                    height,
                    flags,
                    monochrome,
                } = super::font_block::set_font_args(lua, &this, &file, &height, &flags)?;
                if !super::font_block::instance_loads(lua, &path, height) {
                    return Ok(Value::Nil);
                }
                edit(lua, &this, FontMask::FONT, |fo| {
                    fo.font = Some(path);
                    fo.outline = flags;
                    fo.monochrome = monochrome;
                    fo.height = Some(height);
                })?;
                Ok(Value::Number(1.0))
            },
        )?,
    )?;
    // GetFont() → path, height, flags. An unset Font answers nil, 0, "": the constructor
    // `0x783a40` zeroes the height and stores a NULL path (`0x41e3a0`), which pushes as nil.
    m.set(
        "GetFont",
        lua.create_function(|lua, this: Table| {
            let fo = read(lua, &this)?;
            let path = match fo.font {
                Some(p) => Value::String(lua.create_string(&p)?),
                None => Value::Nil,
            };
            Ok((
                path,
                fo.height.unwrap_or(0.0),
                fo.outline.flags_text(fo.monochrome),
            ))
        })?,
    )?;

    // ── colour, and the alpha that is its fourth channel ────────────────────────────────────
    // Shape C on r, g, b (`SetTextColor 0x79f4d0`): nil or a non-number is 0, never a raise.
    m.set(
        "SetTextColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Value)| {
                // Alpha is staged 1.0 (`0x79f59b`) and never read back, so a bare colour call
                // resets it; packed as every colour setter packs.
                let c = super::object::colour_args(lua, &r, &g, &b, &a);
                edit(lua, &this, FontMask::COLOR, |fo| fo.color = Some(c))
            },
        )?,
    )?;
    m.set(
        "GetTextColor",
        lua.create_function(|lua, this: Table| {
            let c = read(lua, &this)?
                .color
                .unwrap_or(crate::color::Rgba8::WHITE);
            Ok(super::object::colour_values(c))
        })?,
    )?;
    // SetAlpha/GetAlpha are the text colour's alpha: a FontInstance has one packed colour
    // (`+0x58`) and no alpha of its own. `SetAlpha` (`0x79f420`) takes a `lua_isnumber` or raises
    // the usage line (`0x79f47f`), and stores `__ftol(a * 255)`'s low byte at `+0x5b`, unclamped
    // (`0x79f449`-`0x79f464`); `GetAlpha` reads it times the float at `0x81c6b4` (`0x79f4b4`).
    m.set(
        "SetAlpha",
        lua.create_function(|lua, (this, a): (Table, Value)| {
            let Some(a) = lua.coerce_number(a)? else {
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:SetAlpha(alpha)",
                    super::binding_abi::name_of(lua, &this)
                )));
            };
            let a = crate::color::wrap_alpha_byte(a);
            edit(lua, &this, FontMask::COLOR, |fo| {
                let c = fo.color.unwrap_or(crate::color::Rgba8::WHITE);
                fo.color = Some(c.with_alpha(a));
            })
        })?,
    )?;
    m.set(
        "GetAlpha",
        lua.create_function(|lua, this: Table| {
            Ok(read(lua, &this)?
                .color
                .map_or(crate::color::channel(255), |c| crate::color::channel(c.a)))
        })?,
    )?;

    // ── shadow ──────────────────────────────────────────────────────────────────────────────
    // An unset shadow is the constructor's transparent black (`0x783a72`), which `GetShadowColor`
    // reads (`0x79f910`) and `SetShadowOffset` keeps (`0x79fa4a`-`0x79fa92`).
    m.set(
        "SetShadowColor",
        lua.create_function(
            // Shape C on r, g, b (`Font:SetShadowColor 0x79f730`): never raises.
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Value)| {
                let color = super::object::colour_args(lua, &r, &g, &b, &a);
                edit(lua, &this, FontMask::SHADOW, |fo| {
                    let offset = fo.shadow.map_or([0.0, 0.0], |s| s.offset);
                    fo.shadow = Some(FontShadow { offset, color });
                })
            },
        )?,
    )?;
    m.set(
        "GetShadowColor",
        lua.create_function(|lua, this: Table| {
            let c = read(lua, &this)?
                .shadow
                .map_or(crate::color::Rgba8::TRANSPARENT, |s| s.color);
            Ok(super::object::colour_values(c))
        })?,
    )?;
    m.set(
        "SetShadowOffset",
        lua.create_function(|lua, (this, x, y): (Table, f32, f32)| {
            edit(lua, &this, FontMask::SHADOW, |fo| {
                let color = fo
                    .shadow
                    .map_or(crate::color::Rgba8::TRANSPARENT, |s| s.color);
                fo.shadow = Some(FontShadow {
                    offset: [x, y],
                    color,
                });
            })
        })?,
    )?;
    m.set(
        "GetShadowOffset",
        lua.create_function(|lua, this: Table| {
            let o = read(lua, &this)?.shadow.map_or([0.0, 0.0], |s| s.offset);
            Ok((o[0], o[1]))
        })?,
    )?;

    // ── line spacing ────────────────────────────────────────────────────────────────────────
    // The shared pair (`0x7a09f0` → `0x79fb40`, `0x7a0ab0` → `0x79fbe0`): `+0x50` as given, a
    // negative included; a FontString that inherits it clamps it.
    m.set(
        "SetSpacing",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let sp = super::font_block::spacing_arg(lua, &this, &v)?;
            edit(lua, &this, FontMask::SPACING, |fo| fo.spacing = Some(sp))
        })?,
    )?;
    m.set(
        "GetSpacing",
        lua.create_function(|lua, this: Table| Ok(read(lua, &this)?.spacing.unwrap_or(0.0)))?,
    )?;

    // ── justification ───────────────────────────────────────────────────────────────────────
    // Parsing is [`crate::justify`]'s, shared with the FontString pair: `SStrCmpI` over the whole
    // string, no trim. `None` means the font does not specify the axis (inheritance), not a
    // cleared axis; a fresh object reads CENTER/MIDDLE, the constructor default `0x212`. The other
    // axis's token is a write of no bits, zeroing the axis, with the bit cleared and the push
    // (`0x79fc5d`-`0x79fc84`).
    m.set(
        "SetJustifyH",
        lua.create_function(|lua, (this, j): (Table, Value)| {
            id_of(lua, &this)?;
            let parsed = match binding_abi::optional_string(lua, &j).map(|j| justify::parse_h(&j)) {
                Some(p) if p != justify::Set::NoMatch => p,
                _ => return Err(justify::usage_h(&binding_abi::name_of(lua, &this))),
            };
            edit(lua, &this, FontMask::JUSTIFY, |fo| {
                let word = fo.justify.get_or_insert_with(Default::default);
                match parsed {
                    justify::Set::To(jh) => word.set_h(jh),
                    _ => word.clear_h(),
                }
            })
        })?,
    )?;
    m.set(
        "GetJustifyH",
        lua.create_function(|lua, this: Table| {
            Ok(read(lua, &this)?.justify.unwrap_or_default().name_h())
        })?,
    )?;
    m.set(
        "SetJustifyV",
        lua.create_function(|lua, (this, j): (Table, Value)| {
            id_of(lua, &this)?;
            let parsed = match binding_abi::optional_string(lua, &j).map(|j| justify::parse_v(&j)) {
                Some(p) if p != justify::Set::NoMatch => p,
                _ => return Err(justify::usage_v(&binding_abi::name_of(lua, &this))),
            };
            edit(lua, &this, FontMask::JUSTIFY, |fo| {
                let word = fo.justify.get_or_insert_with(Default::default);
                match parsed {
                    justify::Set::To(jv) => word.set_v(jv),
                    _ => word.clear_v(),
                }
            })
        })?,
    )?;
    m.set(
        "GetJustifyV",
        lua.create_function(|lua, this: Table| {
            Ok(read(lua, &this)?.justify.unwrap_or_default().name_v())
        })?,
    )?;

    let meta = lua.create_table()?;
    // `__index` is the method table itself, not a function: a table index costs ~9 ns against
    // ~200 ns through a Rust dispatcher, and nothing mutates the table after this point.
    meta.set("__index", m.clone())?;
    lua.set_named_registry_value(REG_FONT_METHODS, m)?;
    lua.set_named_registry_value(REG_FONT_META, meta)?;

    lua.globals()
        .set("CreateFont", lua.create_function(create_font)?)?;
    Ok(())
}

/// The font a new plate or chat bubble takes: `FrameScript_GetText` on `NAMEPLATE_FONT`
/// (`0x703bf0`; plates `0x7cb3ee`/`0x7cb490`, bubbles `0x4b0b36`). Its `_G[key]` arm (`0x703c12` →
/// `0x704350`) gates on `lua_isstring` (`0x6f3510`) and reads `lua_tostring` (`0x6f3690`), so a
/// string or a number answers its text and any other value the shared "". `None` is that empty
/// path, which `SetFont` rejects (`0x44d05c`, `0x44d040` answers 0) and leaves the FontString with
/// no font.
pub(super) fn nameplate_font(lua: &Lua) -> Option<String> {
    let value = lua.globals().get::<Value>("NAMEPLATE_FONT").ok()?;
    let text = lua.coerce_string(value).ok()??;
    Some(text.to_string_lossy()).filter(|s| !s.is_empty())
}

/// `CreateFont(name)`: mint a font object, publish it as the global `name` and return it; addons
/// use both. An existing name returns that object unchanged and unrepublished (`0x7839ab`).
///
/// The reference's gate (`0x706048`, `lua_isstring`) takes a number as its `%.14g` text or any
/// string, the empty one included, and raises `Usage: CreateFont("name")` (`0x872f40`) on anything
/// else. A fresh object holds nothing, so a link to it applies nothing ([`apply`]).
fn create_font(lua: &Lua, name: Option<Value>) -> mlua::Result<Table> {
    let name = name
        .and_then(|v| binding_abi::optional_string(lua, &v))
        .ok_or_else(|| mlua::Error::runtime(r#"Usage: CreateFont("name")"#))?;
    let id = lua
        .app_data_mut::<Model>()
        .expect("model")
        .fonts
        .named(&name);
    let t = wrapper(lua, id)?;
    publish_global(lua, &name, &t)?;
    Ok(t)
}
