//! **The font block** — the ten font methods a text-bearing widget re-declares in its own method
//! table, implemented once over the [`RegionData`](super::RegionData) the glyphs actually paint
//! from, and installed onto any widget's table by [`install`].
//!
//! ## Why this is a module and not a base class
//!
//! **There is no `FontInstance` class in the 1.12.1 Lua chain.** The registrar map shows 23 flat
//! `{const char* name, void* fn}` `.data` tables and a per-class `vtable[+0x8]` lookup
//! (`0x7020b0`'s dispatch) that tail-calls **exactly one** base class's lookup on a miss.
//! The six text-bearing types each *re-declare* the same font names in their own flat table; the
//! sharing happens one level **down**, in C++, where every binding is a thin type-guard shim that
//! tail-calls one shared implementation (`0x79f210` SetFont · `0x79f3b0` GetFont · `0x79f4d0` /
//! `0x79f680` Set/GetTextColor · `0x79f730` / `0x79f910` Set/GetShadowColor · `0x79f9c0` /
//! `0x79fad0` Set/GetShadowOffset · `0x79ef10` / `0x79f090` Set/GetFontObject).
//!
//! So: **which font methods a type has is a per-table fact, never derivable from "it draws text"** —
//! and this module is that shared C++ layer, with each widget's own table naming the subset it
//! really carries. Adding a name a table does not carry is exactly as wrong as missing one.
//!
//! ## The membership, read off the binary rather than assumed
//!
//! `EditBox`'s table is `.data 0x87bb68`, **48 entries** — the count read from the `mov edx,0x30` at
//! the registering site `0x799ab5`, never from a run-length scan (adjacent tables are contiguous and
//! a scan merges them into a bogus blob). Its **first sixteen entries are the font block**, in this
//! order, and each one is a shim that `call`s the shared implementation and returns **without
//! touching `eax`** — so every return arity below is the shared implementation's, verbatim:
//!
//! | # | EditBox entry | binding | tail-calls | returns |
//! |---|---|---|---|---|
//! | 0 | `SetFontObject`   | `0x797090` | `0x79ef10` | **0 values** |
//! | 1 | `GetFontObject`   | `0x797150` | `0x79f090` | **1 value** (the handle, or nil) |
//! | 2 | `SetFont`         | `0x797210` | `0x79f210` | **1 value: the NUMBER 1, or nil** |
//! | 3 | `GetFont`         | `0x7972d0` | `0x79f3b0` | **3 values** (path, height, flags) |
//! | 4 | `SetTextColor`    | `0x797390` | `0x79f4d0` | **0 values** |
//! | 5 | `GetTextColor`    | `0x797450` | `0x79f680` | **4 values** |
//! | 6 | `SetShadowColor`  | `0x797510` | `0x79f730` | **0 values** |
//! | 7 | `GetShadowColor`  | `0x7975d0` | `0x79f910` | **4 values** (`0x79f9b3 mov eax,0x4`) |
//! | 8 | `SetShadowOffset` | `0x797690` | `0x79f9c0` | **0 values** |
//! | 9 | `GetShadowOffset` | `0x797750` | `0x79fad0` | **2 values** |
//! | 10–11 | `Set/GetSpacing` | `0x797810` / `0x7978d0` | `0x79fb40` / `0x79fbe0` | *not installed — below* |
//! | 12–15 | `Set/GetJustifyH`, `Set/GetJustifyV` | `0x797990`… | `0x79fc20`… | *EditBox-side — below* |
//!
//! **`SetFont` is the trap here, and it points the opposite way to the one this codebase already
//! pinned.** `Button:SetFont` (`0x780880`) ends `xor eax,eax` and returns **nothing** — that
//! divergence has its own test. `EditBox:SetFont` does **not**: `0x7972b2 call 0x79f210` is followed
//! by `pop edi; pop esi; pop ebx; ret` with no `eax` write, so the shared implementation's single
//! value (`lua_pushnumber(1.0)` at `0x79f345`, or `lua_pushnil` at `0x79f361`) passes straight
//! through. It is the number `1`, not `true`, and not zero values.
//!
//! The table above was read off the PE `.data` section directly (with Font's 22-entry table as a
//! positive control), covering all 48 names, all 48 binding addresses and all 16 arities. The
//! control for the "nothing is discarded" claim is `SetMultiLine`'s binding, which *does*
//! emit `xor eax,eax` (`0x797e9d`) and returns 0.
//! Entry 47 ends at `0x87bce8`, the first byte of the string `"GetAltArrowKeyMode"` — the table
//! abuts its own string pool, which pins the count at 48 a second way.
//!
//! **A stated divergence, verified and deliberately left consistent rather than fixed on one table:**
//! the reference's `SetTextColor` and `SetShadowColor` read r, g and b with a **bare `lua_tonumber`**
//! (a non-number silently becomes `0.0`) and carry **no usage string at all**, so they *cannot
//! raise* — `SetTextColor()` sets opaque black. Ours take `f32`, so a missing or non-numeric channel
//! errors instead. Alpha matches (`lua_isnumber`-gated, default 1.0).
//!
//! **The collapse below has now landed, which is what was blocking this**: the FontString and
//! EditBox tables both reach these two verbs *here*, so the correction is a one-place change rather
//! than three copies drifting. It is deliberately still not made, for a reason of its own: it turns
//! a loud error into a silent opaque black, which is faithful but is a real behaviour change and
//! belongs in its own landing with its own corpus A/B. No corpus site is known to depend on it.
//!
//! ## Where it lands, and why that is the mechanism rather than an approximation
//!
//! Each shim loads **`[this+0x324]`** and hands it to the shared implementation as the target. On a
//! `CSimpleEditBox` that offset is the box's implicit FontString — the same field
//! [`EditBoxState::text_region`](crate::widget::EditBoxState::text_region) models (the EditBox's
//! analogue of `ButtonText`). So `editBox:SetFontObject(ChatFontNormal)` really does
//! paint *the box's font string*, which is exactly what this module writes: resolve the widget to
//! the region its glyphs come from, then set the same [`RegionData`](super::RegionData) fields a
//! `FontString` sets for itself.
//!
//! ## Not installed here, and the reason is not laziness
//!
//! - **`SetSpacing` / `GetSpacing`.** We model no line spacing anywhere — neither `RegionData` nor
//!   the text layout carries the field, and the line pitch is the font height — so a setter here
//!   could only store a number nobody draws. That is the silently-ignored-setter failure this
//!   codebase keeps being bitten by (1203, 1205, 1211), and the alternative failure is the good one:
//!   a missing method raises `attempt to call method 'SetSpacing' (a nil value)`, which names itself.
//!   Demand is **zero** — `grep -r ':SetSpacing(\|:GetSpacing('` over the 218-addon corpus finds
//!   nothing at all. `script::font`'s own table withholds the pair for the same reason; when spacing
//!   becomes something the renderer honours, both land together.
//! - **The justify four.** They are real EditBox entries and they *are* wired — but on the EditBox's
//!   own state rather than on its text region, because our EditBox draw law forces the region's
//!   justification (see `script::editbox::methods`). They live there for that reason, not here.
//!
//! ## Who installs this, and the duplicates it replaced
//!
//! Two tables: the `EditBox`'s (`script::editbox::methods`) and the **`FontString`'s**
//! (`script::region::text`). The FontString's used to hand-write its own copy of all ten; 1238
//! collapsed them, deleting 226 lines and carrying the correction that copy had been hiding — its
//! `SetFont` answered the boolean `true`, where the bytes above say the number `1` or nil.
//!
//! The `<Font>` object's table (`script::font`) is **not** installed from here, and that is not an
//! oversight: it writes a [`FontObject`](super::FontObject) record rather than a
//! [`RegionData`](super::RegionData), so it shares the *marshalling* ([`set_font_args`]) rather
//! than the body. Its `SetFont` had drifted the other way — `Option` arguments, so `SetFont()`
//! answered nil where `0x87c69c` raises — which is why the gate is one function now.

use mlua::{Lua, Table, Value};

use super::object::as_f32;
use super::{FontShadow, Model, Outline};
use crate::widget::RegionHandle;

/// How a widget's method table resolves `this` to the region its glyphs are painted from.
///
/// A `FontString` *is* that region; an `EditBox` owns one and creates it on demand. A resolver
/// returning an error is how a method called on the wrong receiver reports it.
pub(super) type ResolveRegion = fn(&Lua, &Table) -> mlua::Result<RegionHandle>;

/// `SetFont(path, fontHeight [, flags])`'s shared **argument gate** — the entry of `0x79f210`,
/// which Font `0x7a0270`, FontString `0x79d4f0` and EditBox `0x797210` all reach.
///
/// arg 2 must pass `lua_isstring` and arg 3 `lua_isnumber` — both of which coerce, so a numeric
/// string is accepted for either — else `Usage: %s:SetFont("font", fontHeight [, flags])`
/// (`.rdata 0x87c69c`). That arm is `luaL_error`, which longjmps, so it is an `Err` here and
/// **never** a nil answer (`super::binding_abi`).
///
/// The returned path may be **empty**, which is the caller's *load-failure* edge rather than an
/// argument error: it answers nil with nothing raised, because `!OmniCC/main.lua:41`'s
/// `if not f:SetFont(saved, size) then revert end` is a font-file validity probe. The height is
/// still applied on that edge — only the face is rejected.
///
/// Shared because it was not: the `<Font>` table took `Option<String>, Option<f32>` and so answered
/// nil for `SetFont()` where the reference raises, while the FontString's copy answered the boolean
/// `true` for everything. One gate is the only way three entry points stay one routine.
pub(super) fn set_font_args(
    file: &Value,
    height: &Value,
    widget: &str,
) -> mlua::Result<(String, f32)> {
    let usage = || {
        mlua::Error::runtime(format!(
            "Usage: <{widget}>:SetFont(\"font\", fontHeight [, flags])"
        ))
    };
    let path = match file {
        Value::String(s) => s.to_str()?.to_string(),
        Value::Number(_) | Value::Integer(_) => as_f32(file).to_string(),
        _ => return Err(usage()),
    };
    let height = match height {
        Value::Number(_) | Value::Integer(_) => as_f32(height),
        Value::String(s) => s.to_str()?.parse::<f32>().map_err(|_| usage())?,
        _ => return Err(usage()),
    };
    Ok((path, height))
}

/// Install the ten shared font-block methods onto `m`, reading `this` through `resolve`.
///
/// The caller names the subset: this installs exactly the ten the module doc tabulates, and a
/// widget whose real table lacks one of them must not use this installer for it.
pub(super) fn install(
    lua: &Lua,
    m: &Table,
    resolve: ResolveRegion,
    widget: &'static str,
) -> mlua::Result<()> {
    // ── the font object, and the live link to it ────────────────────────────────────────────
    // SetFontObject(font | "font" | nil) → 0 values. All three argument forms the reference's own
    // usage string names (`.rdata 0x87c5cc`: `Usage: %s:SetFontObject(font or "font" or nil)`).
    // `Dewdrop-2.0.lua:1675` — `editBox:SetFontObject(ChatFontNormal)`, two lines after
    // `CreateFrame("EditBox", nil, editBoxFrame)` — passes the OBJECT, and that one line is
    // replicated into 63 of the corpus's 218 addons.
    //
    // A frame, a number, or an unknown name is an ERROR, never a silent no-op: every rejection in
    // the reference is `luaL_error 0x6f4940`, which longjmps and aborts the call (1203/1205/1211's
    // silent-drop class).
    m.set(
        "SetFontObject",
        lua.create_function(move |lua, (this, font): (Table, Value)| {
            let name = super::font::resolve("SetFontObject", &font)?;
            let rh = resolve(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            // The nil form severs the link and leaves the paint standing — the reference stores a
            // null parent, and nothing re-reads or clears the resolved values.
            let Some(name) = name else {
                model.region_data.entry(rh).or_default().font_object = None;
                return Ok(());
            };
            let Some(fo) = model.font_object(&name).cloned() else {
                return Err(mlua::Error::runtime(format!(
                    "SetFontObject: no font object named '{name}' is registered"
                )));
            };
            let d = model.region_data.entry(rh).or_default();
            d.font_object = Some(name);
            // The severance mask is deliberately NOT reset. The real "stop inheriting
            // this property" signal is a CLEARED bit in the inheritMask (`FONTINSTANCE+0x2c`),
            // cleared by each local setter and never restored — so a property the widget set for
            // itself stays severed across a later `SetFontObject`.
            super::font::repaint(d, &fo);
            model.touch_measure(rh);
            Ok(())
        })?,
    )?;
    // GetFontObject() → exactly 1 value: the font OBJECT last resolved, or nil. The object, never
    // its name — `Dewdrop-2.0.lua:2181` indexes the result immediately.
    m.set(
        "GetFontObject",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let name = {
                let model = lua.app_data_ref::<Model>().expect("model");
                model
                    .region_data
                    .get(&rh)
                    .and_then(|d| d.font_object.clone())
                    .filter(|n| model.font_object(n).is_some())
            };
            match name {
                Some(n) => Ok(Value::Table(super::font::wrapper(lua, &n)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // ── the face ────────────────────────────────────────────────────────────────────────────
    // SetFont(path, height [, flags]) → **the NUMBER 1, or nil** — one value either way, never a
    // boolean and never zero values (the module doc's table; `0x7972b2` does not clobber `eax` the
    // way `Button:SetFont` does). The shared impl gates arg2 on `lua_isstring` and arg3 on
    // `lua_isnumber`, raising `Usage: %s:SetFont("font", fontHeight [, flags])` (`0x87c69c`)
    // otherwise — both accept a numeric string, since `lua_isstring`/`lua_isnumber` coerce.
    //
    // nil is the *load-failure* answer, not an argument error: `!OmniCC/main.lua:41`'s
    // `if not Font:SetFont(saved, size) then revert end` is a font-file validity probe, so an empty
    // path must come back falsey rather than raising.
    m.set(
        "SetFont",
        lua.create_function(
            move |lua, (this, file, height, flags): (Table, Value, Value, Option<String>)| {
                let (path, height) = set_font_args(&file, &height, widget)?;
                let rh = resolve(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                // The verdict is the host's when there is a host: nil is a *load* failure
                // (`0x5c1ae0` under the `0x44d040` font-factory cache), so a path naming no file
                // — an addon's TTF the AddOns folder does not hold — has to come back falsey, and
                // only the store knows. With no probe installed, a non-empty path is 1: a VM with
                // no font backend has nothing for a load to fail against (decision 2103).
                let ok =
                    !path.is_empty() && model.font_probe.as_ref().is_none_or(|probe| probe(&path));
                let d = model.region_data.entry(rh).or_default();
                // Every argument supplied is an EXPLICIT set: it must survive a later mutation of
                // the font object this region inherits.
                if ok {
                    d.font_path = Some(path);
                    d.font_explicit.face = true;
                }
                d.font_height = Some(height);
                d.font_explicit.height = true;
                if let Some(f) = flags {
                    // The LUA flags spelling ("OUTLINE"/"THICKOUTLINE"), not the XML attribute's
                    // ("NORMAL"/"THICK") — the shared parse `0x6f1a90` is a case-insensitive
                    // SUBSTRING scan, which is why "THICKOUTLINE" yields both bits.
                    d.outline = Outline::flags(&f);
                    d.font_explicit.outline = true;
                }
                model.touch_measure(rh);
                Ok(if ok { Value::Number(1.0) } else { Value::Nil })
            },
        )?,
    )?;
    // GetFont() → 3 values: path, height, flagsString (`mov eax,3` at `0x79f407`). The flags string
    // is `""` when there are none — built into a zeroed static buffer at `0xceea60` — never nil.
    //
    // **The HEIGHT slot is a number even on a FontString that was never given a font**, and that is
    // not the obvious reading: the FontString reads back *through* its resolved `CGxFont`, so it
    // looks as though a NULL one should nil all three. It does not. `0x7727b0`'s fifth instruction
    // is an *unconditional* `fld [esi+0xe4]`, and its `+0xe0` test sits behind a branch `GetFont`
    // never takes (`0x79d499 push 0`) — so slot 2 never touches the `CGxFont` at all and always
    // pushes a double. We answer 0 there: `+0xe4` has **no constructor writer** in the reference,
    // so its value on this path is a recycled float that nothing can reproduce, and 0 is what the
    // Font object's own ctor-determined `+0x48` gives (decision 2129).
    //
    // A nil here is not a cosmetic difference. `aux-addon/tabs/search/frame.lua:481` computes
    // `aux.select(2, child:GetFont()) + arg1*2` in its font-resize wheel handler — `aux.select`,
    // the addon's own (`util.lua:26`), which is why that line survives 2171 taking the global
    // `select` away.
    m.set(
        "GetFont",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let d = model.region_data.get(&rh);
            let path = match d.and_then(|d| d.font_path.clone()) {
                Some(p) => Value::String(lua.create_string(&p)?),
                None => Value::Nil,
            };
            let height = d.and_then(|d| d.font_height).unwrap_or(0.0);
            let flags = d.map(|d| d.outline).unwrap_or_default().as_str();
            Ok((path, height, flags))
        })?,
    )?;

    // ── the text colour ─────────────────────────────────────────────────────────────────────
    // SetTextColor(r, g, b [, a]) → 0 values; alpha defaults to 1.0 (`lua_isnumber(L,5)`-gated,
    // `0x3f800000`). A FontString has no texel of its own, so its vertex colour IS the colour it
    // draws — the same `+0xb8` slot `SetVertexColor` writes. The three channels are SHAPE C
    // (`FontString:SetTextColor 0x79d9c0`, `2=C 3=C 4=C 5=B`):
    // a bare `lua_tonumber`, so a nil or a non-number is 0.0 and the call never raises — the stock
    // trainer/trade-skill rows' OnLeave hands it `this.r, this.g, this.b` before anything set them
    // (1973).
    m.set(
        "SetTextColor",
        lua.create_function(
            move |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                let rh = resolve(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let d = model.region_data.entry(rh).or_default();
                d.vertex_color = Some([r, g, b, a.unwrap_or(1.0)]);
                // The shared impl clears the inheritMask's colour bit on whatever instance it ran
                // on (`0x79dbd0`'s `and [edi+0xd4],0xfffffffb`), so an explicit colour survives a
                // later repaint from the font object.
                d.font_explicit.color = true;
                Ok(())
            },
        )?,
    )?;
    // GetTextColor() → 4 values, r, g, b, a. Never set = the white every region draws at.
    m.set(
        "GetTextColor",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let c = model
                .region_data
                .get(&rh)
                .and_then(|d| d.vertex_color)
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;

    // ── the shadow ──────────────────────────────────────────────────────────────────────────
    // **`GetShadowColor` returns FOUR values, not three** (`0x79f9b3`, `mov eax,0x4`). Three is the
    // plausible wrong answer and it silently drops the alpha that
    // `FuBar_NavigatorFu/NavigatorFu.lua:31` round-trips. `GetShadowOffset` returns two, in UI
    // units (the same units as `SetWidth`/`SetPoint`), not device pixels.
    //
    // Either half may be set before the other, so each starts from whatever is already there —
    // shadow colour and offset share one inherit slot in the reference too.
    m.set(
        "SetShadowColor",
        lua.create_function(
            // Shape C on r, g, b (`FontString:SetShadowColor 0x79dd40`, `2=C 3=C 4=C 5=B`) — the
            // same law this module's header already states for its `SetTextColor` sibling, which
            // 1973 closed there and not here.
            move |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (
                    super::object::as_f32(&r),
                    super::object::as_f32(&g),
                    super::object::as_f32(&b),
                );
                let rh = resolve(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let d = model.region_data.entry(rh).or_default();
                let offset = d.font_shadow.map_or([0.0, 0.0], |s| s.offset);
                d.font_shadow = Some(FontShadow {
                    offset,
                    color: [r, g, b, a.unwrap_or(1.0)],
                });
                d.font_explicit.shadow = true;
                Ok(())
            },
        )?,
    )?;
    m.set(
        "GetShadowColor",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let c = model
                .region_data
                .get(&rh)
                .and_then(|d| d.font_shadow)
                .map_or([0.0, 0.0, 0.0, 1.0], |s| s.color);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;
    // SetShadowOffset(x, y) → 0 values, and **both arguments are required** — the shared impl
    // raises `Usage: %s:SetShadowOffset(x, y)` (`0x87c6e8`) rather than defaulting the missing one.
    m.set(
        "SetShadowOffset",
        lua.create_function(move |lua, (this, x, y): (Table, f32, f32)| {
            let rh = resolve(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let d = model.region_data.entry(rh).or_default();
            let color = d.font_shadow.map_or([0.0, 0.0, 0.0, 1.0], |s| s.color);
            d.font_shadow = Some(FontShadow {
                offset: [x, y],
                color,
            });
            d.font_explicit.shadow = true;
            Ok(())
        })?,
    )?;
    m.set(
        "GetShadowOffset",
        lua.create_function(move |lua, this: Table| {
            let rh = resolve(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let o = model
                .region_data
                .get(&rh)
                .and_then(|d| d.font_shadow)
                .map_or([0.0, 0.0], |s| s.offset);
            Ok((o[0], o[1]))
        })?,
    )?;

    Ok(())
}
