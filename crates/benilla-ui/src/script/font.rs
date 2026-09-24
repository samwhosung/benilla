//! **Font objects as first-class Lua objects** — a `<Font name="GameFontNormal">` is not merely a
//! style record the loader flattens, it is an *object* the client publishes as the global
//! `GameFontNormal`, with its own method table.
//!
//! ## Why this exists
//!
//! 1.12's own FrameXML never touches the API — `grep -c FontObject` over the reference
//! FrameXML's own `.lua` is **0** — so the whole surface is addon-facing, and
//! the addon corpus (`benilla_formats::addon_corpus`, 218 addons) is where the demand is:
//!
//! | call shape | sites | note |
//! |---|---|---|
//! | `x:SetFontObject(GameFontNormal)` (bare global) | 3,180 | the form we did **not** accept |
//! | `x:SetFontObject("GameFontNormal")` (string) | 6 | the only form we did accept |
//! | `x:GetFontObject():GetTextColor()` | 65 | needs an OBJECT back, not a name |
//! | `GameTooltipHeaderText:GetFont()` and kin | 268 | `Tablet-2.0`'s header-size probe |
//! | `GameFont*:GetTextColor/GetShadowOffset/GetShadowColor` | 13 | |
//! | `CreateFont(name)` | 3 | `_Nameplates`, `!OmniCC`, `FonzAppraiser` |
//! | `f:CopyFontObject(GameFontHighlightSmall)` | 1 | Font-on-Font |
//!
//! ## The method surface, and where it came from
//!
//! The Font method table is `.data 0x87c7c8` with **22 entries** — the count read from `mov
//! edx,0x16` at `0x7a10d5`, not from a run-length scan, which merges neighbouring tables and
//! reports a bogus 54. Its lookup `0x7a1100` has **no base-class fallback**, so 22 is the entire
//! surface:
//! `GetObjectType · IsObjectType · GetName · SetFontObject · GetFontObject · CopyFontObject ·
//! SetFont · GetFont · SetAlpha · GetAlpha · SetTextColor · GetTextColor · SetShadowColor ·
//! GetShadowColor · SetShadowOffset · GetShadowOffset · SetSpacing · GetSpacing · SetJustifyH ·
//! GetJustifyH · SetJustifyV · GetJustifyV`. All implemented here **except the `Spacing` pair** —
//! see [`install`]. `CopyFontObject` is the only one of the 22 a FontString does not also have.
//!
//! The class is **`CSimpleFont`** (`__FILE__` `0x87a454`), and a named font is published by
//! `0x783870` → `SetName 0x784150` → `CreateLuaHandle 0x701bd0` → `_G[name] = handle` at
//! `0x701cb8`, **skipped when the global is already non-nil** — the same non-overwriting rule
//! [`publish_global`] applies on the frame side.
//!
//! ## Mutability — the deliberate part
//!
//! `GameFontNormal:SetFont(…)` on the real client repaints every FontString that inherits it, and
//! the mechanism is not a copy: `SetFontObject` stores a parent pointer (`+0x28`, FontString
//! `+0xd0`) **and** links the instance into the parent's intrusive dependents list (`+0x74`/`+0x78`,
//! link offset `+0x70`). Every setter ends in `vtable[+0x14]` = `NotifyDependents 0x784180`, which
//! walks that list and calls each dependent's `OnFontChanged` — `0x77e4b0` for a font (which
//! cascades) and `0x773530`→`0x770800` for a FontString (which re-drives its real setters).
//!
//! We model that with an eager push instead of an intrusive list, so no reader changes: every
//! setter here writes the [`FontObject`] record and then calls [`propagate`], which re-paints every
//! region whose [`RegionData::font_object`] names it.
//!
//! **The severance rule, corrected by the bytes.** Our first cut read `FONTINSTANCE+0x038` as an
//! "explicitly set" mask and reset it on every `SetFontObject`. Both halves were wrong. `+0x38`'s
//! low bits are a transient broadcast flag and its high bits mean "has a resolved value" — set by
//! the *inheriting* merge too (`0x7709d3`). The real "stop inheriting this property" signal is a
//! **cleared** bit in the previously-unrecorded **inheritMask at `+0x2c`** (FontString `+0xd4`,
//! per-axis justify at `+0x124`), cleared by each local setter and **never restored** — so a
//! FontString that set its own colour stays severed *across a later `SetFontObject`*.
//! [`RegionData::font_explicit`] is that mask, and nothing resets it.
//!
//! The source side gates too: a merge only copies properties the source actually holds, so a font
//! freshly minted by [`create_font`] (`mask == 0`) copies **nothing** — see [`repaint`].
//!
//! **What we deliberately do not model, now a verified divergence rather than an unknown:** a
//! `<Font inherits="OtherFont">` chain stays flattened at load (`Loader::do_font`), so mutating
//! `MasterFont` does not walk down to `GameFontNormal`. The reference links these live as well —
//! `LoadXML 0x783c30` calls the *same* `0x770c60` link function `SetFontObject` calls
//! (`0x783ce6`), and `font=` likewise (`0x783d22`). Flattening is what this arc was scoped to keep;
//! the cost is bounded and measured: **0 corpus addons mutate a declared font object at all** —
//! every mutation in 218 addons is of the addon's own [`create_font`] object — so the Font→Font
//! half has no demand behind it and the Font→FontString half has all of it.

use mlua::{Lua, Table, Value};

use super::binding_abi;
use super::object::publish_global;
use crate::justify;

use super::{FontObject, FontShadow, Model, Outline, RegionData};

/// The shared metatable of every font-object handle.
const REG_FONT_META: &str = "__benilla_font_meta";
/// The font-object method table the metatable's `__index` dispatches through.
const REG_FONT_METHODS: &str = "__benilla_font_methods";
/// name → handle, so `GameFontNormal == GameFontNormal` and a re-declared `<Font>` keeps identity.
const REG_FONT_WRAPPERS: &str = "__benilla_font_wrappers";

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The handle
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Get-or-create the Lua handle for a named font object.
///
/// Identity is the **name**, not a minted id: every font object in 1.12 has one (an unnamed
/// `<Font>` is warned and dropped at parse time, and [`create_font`] requires one), and keying by
/// name is what makes a re-declared `<Font name="GameFontNormal">` update the record *in place*
/// while every `SetFontObject(GameFontNormal)` already taken keeps pointing at the same object.
///
/// `T[0]` is the name string rather than the frame/region wrapper's `LightUserData` id, so
/// [`super::object::decode_id`] rejects a font handle passed where a frame is wanted, with its own
/// message, instead of decoding a garbage id.
pub(crate) fn wrapper(lua: &Lua, name: &str) -> mlua::Result<Table> {
    let wrappers: Table = lua.named_registry_value(REG_FONT_WRAPPERS)?;
    if let Value::Table(t) = wrappers.get::<Value>(name)? {
        return Ok(t);
    }
    let t = lua.create_table()?;
    t.raw_set(0, name)?;
    let meta: Table = lua.named_registry_value(REG_FONT_META)?;
    t.set_metatable(Some(meta))?;
    wrappers.set(name, t.clone())?;
    Ok(t)
}

/// Publish `_G[name]` as this font object's handle — what makes `fs:SetFontObject(GameFontNormal)`
/// resolve to anything at all. Non-overwriting, exactly like the frame side's [`publish_global`].
///
/// Called by `Loader::do_font` for every top-level `<Font name=…>`, and by [`create_font`].
pub(crate) fn publish(lua: &Lua, name: &str) -> mlua::Result<()> {
    let t = wrapper(lua, name)?;
    publish_global(lua, name, &t)
}

/// The name behind a font handle, or an error naming what was actually passed.
fn name_of(this: &Table) -> mlua::Result<String> {
    match this.raw_get::<Value>(0)? {
        Value::String(s) => Ok(s.to_str()?.to_string()),
        _ => Err(mlua::Error::runtime(
            "not a font object (missing T[0] name identity)",
        )),
    }
}

/// Accept any of the **three** forms the real binding takes and return the registry name, with
/// `None` for the nil form (clear the link).
///
/// The reference's own usage string settles the shape — `.rdata 0x87c5cc` reads verbatim
/// `Usage: %s:SetFontObject(font or "font" or nil)`. So all three are real: the object
/// (3,180 of the corpus's 3,186 sites), the name string (our own shipped `assets/ui` plus 6 corpus
/// sites), and nil.
///
/// Anything else — a frame, a number, a missing argument — is an **error**, and in the reference
/// too: every rejection there is `luaL_error 0x6f4940`, which longjmps and aborts the call rather
/// than no-opping. A font that quietly fails to apply is the silent-drop class of 1203/1205/1211.
pub(super) fn resolve(verb: &str, v: &Value) -> mlua::Result<Option<String>> {
    match v {
        Value::Nil => Ok(None),
        Value::Table(t) => name_of(t).map(Some).map_err(|_| {
            mlua::Error::runtime(format!(
                "{verb}: the argument is a table but not a font object \
                 (a frame or region cannot be a font)"
            ))
        }),
        Value::String(s) => Ok(Some(s.to_str()?.to_string())),
        other => Err(mlua::Error::runtime(format!(
            "{verb}: expected a font object, a font name, or nil, got {}",
            other.type_name()
        ))),
    }
}

/// [`resolve`] for the verbs that genuinely refuse nil — `CopyFontObject` is the one
/// (`0x7a01b0`: it value-copies and re-parents, so there is nothing to copy *from*).
fn resolve_required(verb: &str, v: &Value) -> mlua::Result<String> {
    resolve(verb, v)?.ok_or_else(|| {
        mlua::Error::runtime(format!(
            "{verb}: expected a font object or a font name, got nil"
        ))
    })
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The live link: font object → the regions that inherit it
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Copy a font object's paint onto one region, **skipping every property that region severed by
/// setting for itself** ([`RegionData::font_explicit`]).
///
/// **A property the source does not hold copies nothing.** The reference gates every merge on the
/// source's own has-a-value mask (`+0x38`'s high bits, set by the merge `0x770910`): a font freshly
/// minted by `CreateFont` has `mask == 0`, so `SetFontObject`-ing it onto a FontString leaves that
/// string exactly as it was — no blanking, no fallback to a default. `_Nameplates` relies on
/// nothing here, but `!OmniCC`'s probe font would otherwise wipe a label it was pointed at.
///
/// Residue: [`Outline`] has no "unset" state distinct from `NONE`, so an outline is still written
/// unconditionally. Every shipped `<Font>` flattens from `MasterFont`, which declares no outline,
/// so the two readings coincide for all of them; a `CreateFont` object pointed at an outlined
/// FontString is the one case that differs, and it has no caller.
pub(crate) fn repaint(d: &mut RegionData, fo: &FontObject) {
    let ex = d.font_explicit;
    if !ex.face {
        if let Some(f) = &fo.font {
            d.font_path = Some(f.clone());
        }
    }
    if !ex.height {
        if let Some(h) = fo.height {
            d.font_height = Some(h);
        }
    }
    if !ex.outline {
        d.outline = fo.outline;
    }
    if !ex.shadow {
        if let Some(s) = fo.shadow {
            d.font_shadow = Some(s);
        }
    }
    if !ex.color {
        if let Some(c) = fo.color {
            d.vertex_color = Some(c);
        }
    }
    if !ex.justify_h {
        if let Some(j) = fo.justify_h {
            d.justify.set_h(j);
        }
    }
    if !ex.justify_v {
        if let Some(j) = fo.justify_v {
            d.justify.set_v(j);
        }
    }
}

/// Re-paint every region that inherits `name` — the `SetFont`/`SetTextColor`/… propagation the real
/// client gets for free from its live `parentFontObject`. Linear in regions, on a human-rate path
/// (a font-object setter is an addon config action, never a frame path).
///
/// Per-state **button** label fonts need no equivalent: they are stored as names and re-resolved at
/// every `extract`, so a mutated font object reaches them on the next frame by construction.
pub(crate) fn propagate(model: &mut Model, name: &str) {
    let Some(fo) = model.font_object(name).cloned() else {
        return;
    };
    for d in model.region_data.values_mut() {
        if d.font_object.as_deref() == Some(name) {
            repaint(d, &fo);
        }
    }
    // A fan-out cannot name its regions one by one without a second pass; a font-object edit is
    // a human-rate action, so the measure ledger goes conservative and the next sweep walks the
    // roster once, exactly as every sweep did before the ledger existed.
    model.touch_measure_all();
}

/// Run `f` over the named font object's record, then propagate the change to its dependants.
/// The one write path every setter below goes through.
fn edit<R>(lua: &Lua, this: &Table, f: impl FnOnce(&mut FontObject) -> R) -> mlua::Result<R> {
    let name = name_of(this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let out = f(model
        .font_objects_by_lower
        .entry(name.to_ascii_lowercase())
        .or_default());
    propagate(&mut model, &name);
    Ok(out)
}

/// Read the named font object's record (a clone — the records are small and this is human-rate).
fn read(lua: &Lua, this: &Table) -> mlua::Result<FontObject> {
    let name = name_of(this)?;
    let model = lua.app_data_ref::<Model>().expect("model");
    Ok(model.font_object(&name).cloned().unwrap_or_default())
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// install — the method table, the metatable, and CreateFont
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Build the font-object method table and metatable, and register the `CreateFont` global.
///
/// **Implemented** (20 of the reference table's 22): `GetObjectType`, `IsObjectType`, `GetName`,
/// `SetFontObject`, `GetFontObject`, `CopyFontObject`, `SetFont`, `GetFont`, `SetAlpha`,
/// `GetAlpha`, `SetTextColor`, `GetTextColor`, `SetShadowColor`, `GetShadowColor`,
/// `SetShadowOffset`, `GetShadowOffset`, `SetJustifyH`, `GetJustifyH`, `SetJustifyV`,
/// `GetJustifyV`.
///
/// **Deliberately absent: `SetSpacing`/`GetSpacing`.** We model no line spacing anywhere — neither
/// [`RegionData`] nor the text layout has the field, and the line pitch is the font height — so a
/// `SetSpacing` here could only store a number nobody draws. That is precisely the silently-ignored
/// setter this codebase keeps being bitten by (1203, 1205, 1211), and the alternative failure is
/// the good one: a missing method raises `attempt to call method 'SetSpacing' (a nil value)`, which
/// names itself. Demand behind it is **zero** — no call on any font-object global anywhere in the
/// 218-addon corpus, and 1.12's own FrameXML never calls the font API at all. When spacing becomes
/// a thing the renderer honours, the pair lands with it.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.set_named_registry_value(REG_FONT_WRAPPERS, lua.create_table()?)?;

    let m = lua.create_table()?;

    // ── identity ────────────────────────────────────────────────────────────────────────────
    m.set(
        "GetObjectType",
        lua.create_function(|_, _this: Table| Ok("Font"))?,
    )?;
    // **`1`/`nil`, never a Lua boolean** (decision 2118's law; 2142 found this copy). The
    // reference's Font object has its OWN `IsObjectType` (`0x79fe60`, table `0x87c7c8`) distinct
    // from the Region base's (`0x7a1290`, `0x87c9b8`), and the shapes table types both
    // `(nil) | (number)`. This one answered a Rust `bool` — the fourth copy of a house rule that
    // three neighbours got right, which is exactly the drift 2118 consolidated `flag` to stop, and
    // it was invisible until the shape gate learned to probe the Font object at all.
    //
    // The ARGUMENT contract is a separate question and is not claimed here: `region.rs`'s twin
    // documents the reference's `Usage: %s:IsObjectType("TYPE")` raise and its number-stringifying
    // arm from a byte read of `0x7a1290`; nobody has read `0x79fe60`'s, so this copy keeps mlua's
    // own coercion rather than pretending to that finding.
    m.set(
        "IsObjectType",
        lua.create_function(|_, (_this, ty): (Table, String)| {
            Ok(binding_abi::flag(ty.eq_ignore_ascii_case("font")))
        })?,
    )?;
    m.set(
        "GetName",
        lua.create_function(|_, this: Table| name_of(&this))?,
    )?;

    // ── the font-object-on-font-object pair ─────────────────────────────────────────────────
    // SetFontObject(other) — adopt another object's whole paint. On a Font (rather than a
    // FontString) the reference's live-parent distinction is invisible to us because we flatten
    // Font→Font chains at load (module doc); this is therefore the same copy CopyFontObject makes,
    // and both are spelled out separately so a later Font→Font link has two named places to land.
    // It takes nil like its FontString twin — with no Font→Font link to sever, that is a no-op.
    m.set(
        "SetFontObject",
        lua.create_function(|lua, (this, other): (Table, Value)| {
            match resolve("SetFontObject", &other)? {
                Some(_) => copy_from(lua, &this, &other, "SetFontObject"),
                None => Ok(()),
            }
        })?,
    )?;
    // CopyFontObject(other) — `FonzAppraiser`'s pattern: mint with CreateFont, copy a shipped
    // object's paint, then override the face (`gui.lua:27-30`). Unlike `SetFontObject` it
    // **rejects nil** (`0x7a01b0` value-copies and re-parents, so there is nothing to copy from).
    m.set(
        "CopyFontObject",
        lua.create_function(|lua, (this, other): (Table, Value)| {
            resolve_required("CopyFontObject", &other)?;
            copy_from(lua, &this, &other, "CopyFontObject")
        })?,
    )?;
    // GetFontObject() — a Font object has no parent link in our model (chains are flattened at
    // load), so this is honestly nil rather than a self-reference that would read as a link.
    m.set(
        "GetFontObject",
        lua.create_function(|_, _this: Table| Ok(Value::Nil))?,
    )?;

    // ── face ────────────────────────────────────────────────────────────────────────────────
    // SetFont(path, height [, flags]) → **the number 1, or nil** — the reference's exact return
    // shape (1 value either way; a font file that fails to load yields nil, never an error).
    // `!OmniCC/main.lua:41` uses it as a *font-file validity probe* (`if not
    // OmniCCFont:SetFont(saved, size) then revert end`), so it is not decorative. We answer 1 for
    // any non-empty path — face availability is the renderer's concern here, the atlas falls back
    // per face — and nil for an empty/absent one, so a probe with a blank saved variable reverts.
    m.set(
        "SetFont",
        lua.create_function(
            |lua, (this, file, height, flags): (Table, Value, Value, Option<String>)| {
                // The same gate the FontString and EditBox tables enter — one `0x79f210`, three
                // entry points. This side used to take `Option`s, so `SetFont()` answered **nil**
                // where the reference raises `0x87c69c`; the FontString's copy answered the boolean
                // `true` for everything. Neither was the shared routine's contract.
                let (path, height) = super::font_block::set_font_args(&file, &height, "Font")?;
                let ok = !path.is_empty();
                edit(lua, &this, |fo| {
                    if ok {
                        fo.font = Some(path);
                    }
                    fo.height = Some(height);
                    if let Some(f) = flags {
                        // The LUA flags spelling ("OUTLINE"/"THICKOUTLINE"), not the XML
                        // attribute's ("NORMAL"/"THICK") — this read the XML one, so every
                        // `GameFontNormal:SetFont(f, h, "OUTLINE")` silently cleared the outline.
                        fo.outline = Outline::flags(&f);
                    }
                })?;
                Ok(if ok { Value::Number(1.0) } else { Value::Nil })
            },
        )?,
    )?;
    // GetFont() → path, height, flags. `Tablet-2.0.lua:289`'s `_, headerSize =
    // GameTooltipHeaderText:GetFont()` is the corpus's single biggest font-object read (268 sites),
    // and it does arithmetic on the second value.
    //
    // **An unset Font answers `(nil, 0, "")`, all three ctor-determined** — `0x783a40` writes
    // `[esi+0x48]` and `[esi+0x4c]` from a zeroed register, and its `0x41e3a0(NULL)` stores the
    // shared empty record whose `char*` is NULL, which `lua_pushstring` turns into nil. So the
    // height is the NUMBER zero, not nil.
    m.set(
        "GetFont",
        lua.create_function(|lua, this: Table| {
            let fo = read(lua, &this)?;
            let path = match fo.font {
                Some(p) => Value::String(lua.create_string(&p)?),
                None => Value::Nil,
            };
            Ok((path, fo.height.unwrap_or(0.0), fo.outline.as_str()))
        })?,
    )?;

    // ── colour, and the alpha that is its fourth channel ────────────────────────────────────
    // Shape C on the three channels (`SetTextColor 0x79f4d0`, `2=C 3=C 4=C 5=B`): a nil or
    // non-number is 0.0, never a raise (1973).
    m.set(
        "SetTextColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                edit(lua, &this, |fo| {
                    let keep = fo.color.map_or(1.0, |c| c[3]);
                    fo.color = Some([r, g, b, a.unwrap_or(keep)]);
                })
            },
        )?,
    )?;
    m.set(
        "GetTextColor",
        lua.create_function(|lua, this: Table| {
            let c = read(lua, &this)?.color.unwrap_or([1.0, 1.0, 1.0, 1.0]);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;
    // SetAlpha/GetAlpha on a FontInstance are its text colour's alpha channel, not a separate slot:
    // a FontInstance has exactly one colour word (`FONTINSTANCE+0x058 textColor`, a packed
    // CImVector) and no alpha field of its own, so there is nowhere else for an alpha to live. Zero
    // corpus sites; implemented because a duck-typed `if f.SetAlpha` should find it and because the
    // semantics follow from the storage rather than from a guess.
    m.set(
        "SetAlpha",
        lua.create_function(|lua, (this, a): (Table, f32)| {
            edit(lua, &this, |fo| {
                let c = fo.color.unwrap_or([1.0, 1.0, 1.0, 1.0]);
                fo.color = Some([c[0], c[1], c[2], a]);
            })
        })?,
    )?;
    m.set(
        "GetAlpha",
        lua.create_function(|lua, this: Table| Ok(read(lua, &this)?.color.map_or(1.0, |c| c[3])))?,
    )?;

    // ── shadow ──────────────────────────────────────────────────────────────────────────────
    // Either half may be set before the other, so both start from whatever is there (or a zero
    // offset / opaque black) rather than refusing a shadow that is only half-declared.
    m.set(
        "SetShadowColor",
        lua.create_function(
            // Shape C on r, g, b (`Font:SetShadowColor 0x79f730`, `2=C 3=C 4=C 5=B`) — bare
            // `lua_tonumber`, no gate, never raises.
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (
                    crate::script::object::as_f32(&r),
                    crate::script::object::as_f32(&g),
                    crate::script::object::as_f32(&b),
                );
                edit(lua, &this, |fo| {
                    let offset = fo.shadow.map_or([0.0, 0.0], |s| s.offset);
                    fo.shadow = Some(FontShadow {
                        offset,
                        color: [r, g, b, a.unwrap_or(1.0)],
                    });
                })
            },
        )?,
    )?;
    m.set(
        "GetShadowColor",
        lua.create_function(|lua, this: Table| {
            let c = read(lua, &this)?
                .shadow
                .map_or([0.0, 0.0, 0.0, 1.0], |s| s.color);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;
    m.set(
        "SetShadowOffset",
        lua.create_function(|lua, (this, x, y): (Table, f32, f32)| {
            edit(lua, &this, |fo| {
                let color = fo.shadow.map_or([0.0, 0.0, 0.0, 1.0], |s| s.color);
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

    // ── justification ───────────────────────────────────────────────────────────────────────
    //
    // The token table, the whole-string match, the raise on a miss and the cross-axis clear are
    // [`crate::justify`]'s, shared with the FontString table's identical pair. This side used to
    // `.trim()` its argument and the other did not — one law, two transcriptions, and at most one
    // of them could be right (the bytes say no trim: `SStrCmpI` compares the whole string).
    //
    // `justify_h`'s `Option` here is *inheritance*, not the reference's cleared axis: `None` means
    // "this font object does not specify justification", which is what the resolve at
    // [`resolve_font_object`] and `extract.rs` test. A fresh object therefore reads CENTER/MIDDLE
    // through `unwrap_or_default`, which is also the client's ctor default `0x212`.
    m.set(
        "SetJustifyH",
        lua.create_function(|lua, (this, j): (Table, String)| {
            let jh = match justify::parse_h(&j) {
                justify::Set::To(jh) => jh,
                justify::Set::Clears => return Ok(()),
                justify::Set::NoMatch => return Err(justify::usage_h("Font")),
            };
            edit(lua, &this, |fo| fo.justify_h = Some(jh))
        })?,
    )?;
    m.set(
        "GetJustifyH",
        lua.create_function(|lua, this: Table| {
            Ok(justify::name_h(
                read(lua, &this)?.justify_h.unwrap_or_default(),
            ))
        })?,
    )?;
    m.set(
        "SetJustifyV",
        lua.create_function(|lua, (this, j): (Table, String)| {
            let jv = match justify::parse_v(&j) {
                justify::Set::To(jv) => jv,
                justify::Set::Clears => return Ok(()),
                justify::Set::NoMatch => return Err(justify::usage_v("Font")),
            };
            edit(lua, &this, |fo| fo.justify_v = Some(jv))
        })?,
    )?;
    m.set(
        "GetJustifyV",
        lua.create_function(|lua, this: Table| {
            Ok(justify::name_v(
                read(lua, &this)?.justify_v.unwrap_or_default(),
            ))
        })?,
    )?;

    let meta = lua.create_table()?;
    // **`__index` is the method TABLE, not a dispatcher function**. A Rust
    // `__index` turns every `fo.GetFont` — a plain table index in the source — into a Lua→Rust→Lua
    // round trip plus a named-registry string lookup; measured at ~200 ns against ~9 ns for the
    // table form, on a path every widget call in the client begins with. The table is mutated in
    // place by nothing after this point, so pointing at it cannot go stale.
    meta.set("__index", m.clone())?;
    lua.set_named_registry_value(REG_FONT_METHODS, m)?;
    lua.set_named_registry_value(REG_FONT_META, meta)?;

    lua.globals()
        .set("CreateFont", lua.create_function(create_font)?)?;
    Ok(())
}

/// The body behind `SetFontObject`/`CopyFontObject` on a Font: take the *other* object's resolved
/// paint wholesale.
fn copy_from(lua: &Lua, this: &Table, other: &Value, verb: &str) -> mlua::Result<()> {
    let src = resolve_required(verb, other)?;
    let paint = {
        let model = lua.app_data_ref::<Model>().expect("model");
        model.font_object(&src).cloned().ok_or_else(|| {
            mlua::Error::runtime(format!(
                "{verb}: no font object named '{src}' is registered"
            ))
        })?
    };
    edit(lua, this, |fo| *fo = paint)
}

/// `CreateFont(name)` — mint a font object at runtime and publish it as the global `name`, the
/// twin of a `<Font name=…>` declaration. Returns the handle.
///
/// Both halves are load-bearing in the corpus and neither can be dropped: `_Nameplates.lua:149` and
/// `FonzAppraiser/mods/gui/gui.lua:27` keep the **return**, while `!OmniCC/main.lua:40-41` throws it
/// away and reads the **global** on the very next line.
///
/// A name that already names a font object returns the existing one **unchanged, silently** — no
/// error, no re-publication (`0x7839ab`). We had chosen that as the non-destructive reading before
/// the bytes were asked; they agree.
///
/// The empty string is accepted (the reference's `lua_isstring` gate takes strings *and* numbers,
/// and unlike the XML path does not require a non-empty name); only a missing/nil argument is an
/// error. A fresh object holds nothing, so `SetFontObject`-ing it copies nothing — see [`repaint`].
fn create_font(lua: &Lua, name: Option<String>) -> mlua::Result<Table> {
    let name = name.ok_or_else(|| {
        mlua::Error::runtime("CreateFont: a font name is required (it becomes a global)")
    })?;
    {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        model
            .font_objects_by_lower
            .entry(name.to_ascii_lowercase())
            .or_default();
    }
    let t = wrapper(lua, &name)?;
    publish_global(lua, &name, &t)?;
    Ok(t)
}
