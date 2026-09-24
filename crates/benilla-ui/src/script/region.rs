//! The **region** side of the object model (each leaf carries the reference's own type tag): the
//! `Texture`/`FontString` wrapper cache, their shared metatable, and the region method surface.
//! Split from [`super::object`] (which keeps the frame side + `CreateFrame`) so each grows along
//! its own axis — frames grow per-kind method tables ([`super::statusbar`], [`super::button`]),
//! regions grow paint/coords methods here.

use mlua::{Lua, MultiValue, Table, Value};

use super::object::anchor_args::{parse_set_point, resolve_rel_target, UNNAMED};
use super::object::{
    anchor_bits_eq, anchor_retarget_is_structural, decode_id, id_to_lud, NamedTarget,
};
use super::region_map::{set_shared, Side};
use super::{
    Model, REG_FONTSTRING_META, REG_FONTSTRING_METHODS, REG_REGION_META, REG_REGION_METHODS,
    REG_TEXTURE_META, REG_TEXTURE_METHODS, REG_TITLE_META, REG_TITLE_METHODS, REG_WRAPPERS, SCREEN,
};
use crate::layout::Anchor;
use crate::widget::{RegionHandle, RegionKind};

/// Resolve `self` (a region wrapper) to its live [`RegionHandle`].
pub(super) fn region_handle_of(lua: &Lua, this: &Table) -> mlua::Result<RegionHandle> {
    let id = decode_id(this)?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model
        .id_to_region
        .get(&id)
        .copied()
        .ok_or_else(|| mlua::Error::runtime("stale or invalid region handle"))
}

/// Apply the font parts an **XML element** supplies — `font=`, `<FontHeight>`, `outline=` — any
/// subset of which may be absent.
///
/// **This is deliberately not the `SetFont` binding, and that separation is the point.** The
/// reference applies XML font attributes in C++ (`LoadXML`), never through the Lua method, and its
/// `SetFont` therefore *requires* both a path and a height, raising
/// `Usage: %s:SetFont("font", fontHeight [, flags])` (`0x87c69c`) without them. Our loader used to
/// call the binding with three `Option`s — `SetFont(nil, nil, "OUTLINE")` for an outline-only
/// `<FontString>` — which is why that binding could not be made faithful: one name was doing two
/// jobs, and the more lenient job won. Splitting them lets `SetFont` be the reference's `SetFont`
/// and lets the loader keep the partial application XML actually needs.
///
/// Each part supplied is an **explicit** set (`FontExplicit`), so it survives a later mutation of
/// the font object this region inherits. An empty `font=` is treated as absent — it keeps the
/// inherited face, rather than being the binding's load-failure edge.
pub(crate) fn apply_font_parts(
    lua: &Lua,
    this: &Table,
    path: Option<String>,
    height: Option<f32>,
    flags: Option<String>,
) -> mlua::Result<()> {
    let rh = region_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let d = model.region_data.entry(rh).or_default();
    if let Some(p) = path.filter(|p| !p.is_empty()) {
        d.font_path = Some(p);
        d.font_explicit.face = true;
    }
    if let Some(h) = height {
        d.font_height = Some(h);
        d.font_explicit.height = true;
    }
    if let Some(f) = flags {
        d.outline = super::Outline::flags(&f);
        d.font_explicit.outline = true;
    }
    model.touch_measure(rh);
    Ok(())
}

/// Get-or-create the wrapper table for a region id (distinct metatable — the region "tag").
pub(super) fn region_wrapper(lua: &Lua, id: u32) -> mlua::Result<Table> {
    let wrappers: Table = lua.named_registry_value(REG_WRAPPERS)?;
    if let Value::Table(t) = wrappers.get::<Value>(id)? {
        return Ok(t);
    }
    let t = lua.create_table()?;
    t.raw_set(0, Value::LightUserData(id_to_lud(id)))?;
    // **A title region gets a NARROWER metatable, chosen here rather than per lookup.**
    // `CreateTitleRegion 0x773910`'s object answers *exactly* the 19 Region methods — no
    // Show/Hide, no textures, no text — and this cache is created once per region, so picking the
    // table at construction costs nothing on the call path (dispatching inside `__index` would
    // put a model borrow and a kind lookup in front of EVERY region method call in the UI).
    let kind = {
        let model = lua.app_data_ref::<Model>().expect("model");
        model
            .id_to_region
            .get(&id)
            .and_then(|rh| model.arena.region(*rh))
            .map(|r| r.kind)
    };
    let meta: Table = lua.named_registry_value(match kind {
        Some(RegionKind::Texture) => REG_TEXTURE_META,
        Some(RegionKind::FontString) => REG_FONTSTRING_META,
        Some(RegionKind::Title) => REG_TITLE_META,
        // A wrapper for an id with no live region: the full table, which is what this cache did for
        // every region before the leaves were split. Nothing can call through it — every method
        // resolves the handle first and raises on a dead one.
        None => REG_REGION_META,
    })?;
    t.set_metatable(Some(meta))?;
    wrappers.set(id, t.clone())?;
    Ok(t)
}

/// Install the region method table + the shared region metatable.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    install_region_methods(lua)?;

    // SetPortraitTexture(textureRegion, unit) — the live API's **global** (ref-UnitFrame.lua:
    // `SetPortraitTexture(this.portrait, this.unit)`), distinct from the region-method
    // `SetPortraitToTexture(path)` icon crop. It binds a Texture region to a unit token; the app
    // renders that unit's model off-screen and feeds the bake back through the region's
    // [`super::QuadContent::Texture::portrait_unit`], with `circular` marking the round stencil
    // (the bake is square with an opaque backdrop; the frame-ring portraits cut the inscribed
    // circle, exactly what the app's quad shader does with the flag). `texture`/`color` drop —
    // the bake replaces them. A later `SetTexture`/`SetPortraitToTexture` clears the binding.
    lua.globals().set(
        "SetPortraitTexture",
        lua.create_function(|lua, (region, unit): (Table, Value)| {
            // `0x519ef0` gates its unit argument at `0x519fb4` with `lua_isstring` and fails into
            // `luaL_error` with this exact string (`0x8513b8`); `luaL_error` does not return, so
            // nil/absent/boolean/table abandon the caller's statement. Stock
            // `Blizzard_InspectUI.lua:49` reaches it on a re-open, which is how it was found.
            // A token that RESOLVES to nothing is a different, quiet case — it blanks the portrait
            // and returns no values. Decision 1834.
            let unit = crate::script::binding_abi::string_arg(
                lua,
                unit,
                r#"Usage: SetPortraitTexture(texture, "unit")"#,
            )?;
            let rh = region_handle_of(lua, &region)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let data = model.region_data.entry(rh).or_default();
            // CANONICAL (lowercase) on the way in, like `set_unit`'s map key: the binding
            // resolves its token through the one resolver every `Unit*` binding shares
            // (`0x519ef0` → `0x515970`), whose every compare is `_strnicmp` — so the stock
            // `MerchantFrame.lua:68`, `GuildRegistrarFrame.lua:4` and `TradeFrame.lua:41` all
            // write `"NPC"` and mean `"npc"`. The app samples the booth by this string
            // (`portrait::PortraitImages`, keyed by the lowercase slot names), and a raw `"NPC"`
            // reached it and matched nothing: an empty portrait ring on all three windows
            // (decision 2022).
            data.portrait_unit = Some(unit.to_ascii_lowercase());
            data.texture = None;
            data.fill = None;
            data.circular = true;
            Ok(())
        })?,
    )?;

    // SetPortraitToTexture(textureName, path) — the ENGINE GLOBAL, which is what 1.12 has.
    //
    // `reference/1.12-globals.tsv` marks it `engine`, and the reference's own two call sites both
    // pass a texture NAME: `ContainerFrame.lua:419` writes
    // `SetPortraitToTexture(frame:GetName().."Portrait", "…KeyRing-Bag-Icon")` and
    // `MailFrame.lua:174` writes `SetPortraitToTexture("OpenMailFrameIcon", stationeryIcon)`.
    // **That first one matters here: we SOURCE `ContainerFrame.lua` off the patch chain, so the
    // client's own file calls this global inside our VM.**
    //
    // A NAME, not a region handle — strictly what the reference's callers are attested to pass.
    // Whether the real binding also accepts a texture object has not been read from the bytes, so
    // it is not accepted here: inventing the wider signature is how a superset starts (1189), and
    // nothing needs it — both of our own callers already hold the name.
    //
    // The behaviour is the crop it was always: set the texture AND mark the region a portrait, so
    // it draws masked to its inscribed circle. The client's portraits are circular and the frame
    // ring is a thin band whose transparent corners would otherwise show the square texture's
    // edges. Distinct from `SetPortraitTexture(region, unit)`'s live model bake, so it drops any
    // live-unit binding; a later `SetTexture` clears the flag again.
    lua.globals().set(
        "SetPortraitToTexture",
        lua.create_function(|lua, (name, path): (String, String)| {
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let Some(id) = model.region_names.get(&name).copied() else {
                return Ok(());
            };
            let Some(rh) = model.id_to_region.get(&id).copied() else {
                return Ok(());
            };
            let data = model.region_data.entry(rh).or_default();
            data.texture = Some(path);
            data.circular = true;
            data.portrait_unit = None;
            Ok(())
        })?,
    )?;

    // BenillaSetBoothTexture(textureRegion, slotToken) — the **square** twin of
    // SetPortraitTexture (decision 0208 §5): the same `portrait_unit` booth-image binding
    // WITHOUT the circular mask, for a body pane whose texture region samples the booth's bake
    // edge to edge (no frame ring to mask for). Benilla-named because it is not the live API: it
    // is the pane→booth join written in Lua.
    //
    // **RETIRING.** A migrated window's file is the reference's, which declares a bare
    // `<PlayerModel>` and no texture at all, so the join moved app-side
    // (`portrait::MODEL_PANE_BOOTHS`) and the widget draws itself (decision 1810). Two callers are
    // left — the dressing room and the inspect window, both still our own files — and this goes
    // with the later of them.
    lua.globals().set(
        "BenillaSetBoothTexture",
        lua.create_function(|lua, (region, token): (Table, String)| {
            let rh = region_handle_of(lua, &region)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let data = model.region_data.entry(rh).or_default();
            data.portrait_unit = Some(token);
            data.texture = None;
            data.fill = None;
            data.circular = false;
            Ok(())
        })?,
    )?;

    // **`__index` is the method TABLE, not a dispatcher function** (decision 2310) — here and at
    // the three leaf metatables below. A Rust `__index` makes every `t.SetTexture` a Lua→Rust→Lua
    // round trip plus a named-registry string lookup; measured at ~195 ns against ~9 ns for the
    // table form, paid by every widget method access in the client. Nothing mutates these tables
    // after install (`super::region_map::install` writes into them in place, and runs inside
    // `super::object::install`), so holding the table itself cannot go stale.
    let region_meta = lua.create_table()?;
    region_meta.set(
        "__index",
        lua.named_registry_value::<Table>(REG_REGION_METHODS)?,
    )?;
    lua.set_named_registry_value(REG_REGION_META, region_meta)?;
    Ok(())
}

mod layout;
mod paint;
mod text;

fn install_region_methods(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // GetName() → this region's global name, or nil when it was declared anonymously — the pair
    // the frame side already answers. Real FrameXML round-trips a region through its name wherever
    // it can't hold a reference to itself: `ComboFrame.lua`'s shine chain hands `frame:GetName()`
    // to a fade `finishedFunc` and `getglobal`s it back ("hack since a frame can't have a
    // reference to itself in it" — its own comment).
    //
    // Resolution is [`region_name_of`], shared with `IsObjectType`'s `Usage:` text so the two can
    // never disagree about what this region is called.
    set_shared(lua, &m, Side::Region, "GetName", |lua, this: Table| {
        let id = decode_id(&this)?;
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        match region_name_of(&model, id) {
            Some(n) => Ok(Value::String(lua.create_string(&n)?)),
            None => Ok(Value::Nil),
        }
    })?;

    // ── GetObjectType / IsObjectType: the last two of the Region map (1244 §4 closed) ───────────
    //
    // 1244 shipped four of the six missing Region-map members and deliberately left these two
    // DISPATCHED rather than guessed, because every interesting detail is one a plausible
    // implementation gets wrong. Every one of those details is below, verified at the bytes.
    //
    // `GetObjectType` is a per-class `.data` `const char*` read through `vtable[+0x1c]` — Texture
    // `0x773480` → `"Texture"`, FontString `0x7735d0` → `"FontString"` — pushed with
    // `lua_pushstring`, exactly one value, extra arguments ignored with no arity check.
    set_shared(
        lua,
        &m,
        Side::Region,
        "GetObjectType",
        |lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(region_type_name(&model, rh))
        },
    )?;

    // `IsObjectType(name)` — binding `0x7a1290`. Four traps, all verified, all here:
    //
    //  · **Case-INSENSITIVE, whole-string.** `vtable[+0x18]` → `SStrCmpI 0x64a4c0` → `_strnicmp`,
    //    which folds both operands before comparing and breaks at the first NUL on either side —
    //    so no prefix or substring match either. (The *method-name* lookup `0x702000` uses the
    //    case-SENSITIVE sibling; the two are easy to conflate and behave differently.)
    //  · **A short, hardcoded chain — and there is no root type.** Texture answers `"Texture"` and
    //    `"Region"`; FontString answers `"FontString"` and `"Region"`. **1.12.1 has no
    //    `"LayoutFrame"`, `"ScriptObject"` or `"Object"` type at all** — those strings exist only
    //    inside `__FILE__` paths and allocator tags — so `tex:IsObjectType("LayoutFrame")` is nil.
    //    That is the single most likely thing to invent from knowing later clients.
    //  · **A hit is the NUMBER 1 and a miss is nil — never a boolean**, read off the pushed tags
    //    (`lua_pushnumber` tag 3 vs `lua_pushnil` tag 0; tag 1 is never written), and exactly one
    //    value on both paths.
    //  · **A bad argument RAISES.** The gate is `lua_isstring`, which accepts strings and NUMBERS
    //    only; anything else (missing, nil, boolean, table, function, userdata) hits
    //    `luaL_error(L, "Usage: %s:IsObjectType(\"TYPE\")")` with `%s` the region's name or
    //    `<unnamed>`, and that longjmps rather than returning. A number is ACCEPTED and stringified
    //    in place, so `tex:IsObjectType(5)` compares against `"5"` and quietly answers nil — we
    //    format it and compare for real rather than short-circuiting, though no type name is
    //    numeric so the answer is nil either way.
    set_shared(
        lua,
        &m,
        Side::Region,
        "IsObjectType",
        |lua, (this, want): (Table, Value)| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let want = match &want {
                Value::String(s) => s.to_str()?.to_string(),
                Value::Number(n) => n.to_string(),
                Value::Integer(i) => i.to_string(),
                _ => {
                    let who = region_name_of(&model, decode_id(&this)?)
                        .unwrap_or_else(|| "<unnamed>".to_string());
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {who}:IsObjectType(\"TYPE\")"
                    )));
                }
            };
            let leaf = region_type_name(&model, rh);
            let hit = want.eq_ignore_ascii_case(leaf) || want.eq_ignore_ascii_case("Region");
            Ok(if hit { Value::Number(1.0) } else { Value::Nil })
        },
    )?;

    // SetParent(frame) — **a Texture/FontString really does have this**, and we were the ones
    // missing it. `SetParent` lives in the REGION method table (`0x7a1550`); Texture's class lookup
    // falls back to Region's at `0x79c650` and FontString's at `0x79ee50`, so both reach it.
    // `FuBar_FuXPFu.lua:210`'s `self.Spark:SetParent(self.XPBar)` is not a broken addon.
    //
    // Four contract traps, each spelled out because each is a plausible implementation's silent
    // divergence:
    //
    //  · **The argument must be a FRAME.** `0x7a16ea` runs `IsA(FrameTag)` on it, and a Texture or
    //    Region argument raises `"…Wrong parent object type, expected frame"` (`0x87cb78`). Ours
    //    raises too — a re-parent that quietly did nothing is the silent-drop class of 1203/1205.
    //  · **A missing argument is NOT the nil form.** TNONE fails the `== LUA_TNIL` test and falls
    //    through to `"…Couldn't find region named '%s'"` (`0x87cb48`), so `tex:SetParent()` raises
    //    while `tex:SetParent(nil)` detaches. That is why this reads a `MultiValue`: mlua cannot
    //    tell an absent argument from an explicit nil any other way.
    //  · **Anchors are untouched.** The re-link moves draw-layer/region-list membership only; a
    //    re-parented Texture still anchors to whatever `SetPoint` named — which is why FuXPFu
    //    re-points its spark afterwards, and why our anchors (which store the resolved target id)
    //    need no fixing up at all.
    //  · **Zero return values**, on every path.
    //
    // The mechanism half — full re-link, layer and sub-level preserved, `nil` = orphaned but not
    // destroyed — is [`crate::widget::WidgetArena::set_region_owner`]'s doc.
    set_shared(
        lua,
        &m,
        Side::Region,
        "SetParent",
        |lua, args: mlua::MultiValue| {
            let mut it = args.into_iter();
            let Some(Value::Table(this)) = it.next() else {
                return Err(mlua::Error::runtime("SetParent: expected a region"));
            };
            let rh = region_handle_of(lua, &this)?;
            let Some(parent) = it.next() else {
                return Err(mlua::Error::runtime(
                    "SetParent(): Couldn't find region named '' (no argument)",
                ));
            };
            let wrong_type =
                || mlua::Error::runtime("SetParent(): Wrong parent object type, expected frame");
            // `_G[name]` before the guard, and **without** `$parent` expansion — the reparent
            // bindings call `0x76c760` directly (`super::object::NamedTarget`).
            let named = match &parent {
                Value::String(s) => Some(match s.to_str() {
                    Ok(n) => super::object::prefetch_named_target(lua, n.as_ref(), None),
                    Err(_) => NamedTarget::unreadable(),
                }),
                _ => None,
            };
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let new_owner = match &parent {
                Value::Nil => None,
                // A region/font-object wrapper decodes to an id that is not a frame's (or to no id
                // at all) — both land on the same "expected frame" the reference raises.
                Value::Table(t) => Some(
                    decode_id(t)
                        .ok()
                        .and_then(|id| model.id_to_frame.get(&id).copied())
                        .ok_or_else(wrong_type)?,
                ),
                // A name resolves through `_G[name]` + the narrow **Frame** tag check
                // (`[0xcf0c10]`), as every other frame-target argument does — the reference's
                // `0x76c760`; see `object::NamedTarget`.
                Value::String(_) => {
                    let nt = named.as_ref().expect("a String argument is prefetched");
                    let hit = super::object::resolve_named_target(&model, nt)
                        .and_then(|id| model.id_to_frame.get(&id).copied());
                    Some(hit.ok_or_else(|| {
                        mlua::Error::runtime(format!(
                            "SetParent(): Couldn't find region named '{}'",
                            nt.name
                        ))
                    })?)
                }
                _ => return Err(wrong_type()),
            };
            if model.arena.set_region_owner(rh, new_owner) {
                // An un-anchored region draws relative to its owner, and an anchored one resolves
                // against the owner's rect and effective scale (`layout.rs`'s region sweep) — the
                // owner is a layout input, so a re-link is a layout change.
                model.touch_layout();
            }
            Ok(())
        },
    )?;

    // Region-level visibility — the real VisibleRegion Show/Hide on Textures/FontStrings (the
    // ref kit hides tab slices, cooldown swipes, money coins…). A hidden region draws nothing;
    // IsVisible additionally requires the owner frame's effective visibility, mirroring the
    // frame-side pair.
    m.set(
        "Show",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().hidden = false;
            Ok(())
        })?,
    )?;

    m.set(
        "Hide",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().hidden = true;
            Ok(())
        })?,
    )?;

    m.set(
        "IsShown",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let shown = !model.region_data.get(&rh).is_some_and(|d| d.hidden);
            Ok(if shown { Value::Integer(1) } else { Value::Nil })
        })?,
    )?;

    m.set(
        "IsVisible",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let shown = !model.region_data.get(&rh).is_some_and(|d| d.hidden);
            let owner_visible = model
                .arena
                .region(rh)
                .and_then(|r| model.arena.frame(r.owner))
                .is_some_and(|f| f.effective_visible);
            Ok(if shown && owner_visible {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;

    // The three clusters this file was split into (0716's budget). Order is immaterial —
    // every one of them only writes into the same method table.
    paint::install(lua, &m)?;
    text::install(lua, &m)?;
    layout::install(lua, &m)?;

    // ── The title region's own, narrower table ──────────────────────────────────────────────────
    //
    // 1250 §5 recorded this as a named divergence: our region metatable was shared, so a title
    // region answered `SetTexture`/`Show`/`Hide` where the reference raises `attempt to call
    // method`. Inert — the kind never draws — but a **superset in PRESENCE**, and 1189 is the
    // record of what a superset costs when an addon feature-detects.
    //
    // Closed by copying exactly the Region map (the 19 names 1244/1245 landed and assert as a set)
    // out of the full table. It is a copy rather than a second install because the two must never
    // disagree about what, say, `GetPoint` does — one implementation, two visibilities.
    //
    // **The Texture/FontString superset is NOT closed here, deliberately.** They still share one
    // table, so a Texture answers `SetText` and a FontString answers `SetTexture`. Splitting those
    // needs the per-table membership facts, and the naive partition is WRONG: `paint.rs` installs
    // `SetDrawLayer`/`SetVertexColor`/`SetAlpha`/`SetAlphaGradient`, all of which FontString's own
    // method table (`0xcf5400`) legitimately has. A wrong split REMOVES verbs addons use, which is
    // worse than the superset it fixes — so that half waits for the membership read (1238's shape).
    let title = lua.create_table()?;
    for name in super::REGION_MAP_METHODS {
        let f: Value = m.get(name)?;
        title.set(name, f)?;
    }
    // ── The two LEAF tables ──────────────────────────────────────────────────────────────────
    //
    // Texture's map is `0x87c128` (22 entries, lookup `0x79c620`), FontString's is `0xcf5400` (32,
    // lookup `0x79ee20`); both tail-call the Region map and stop there — no third table. Until now
    // ours was ONE table for both, so a Texture answered `SetText` and a FontString answered
    // `SetTexture`: a superset in both directions.
    //
    // **Partitioned first, then pruned name by name.** The 1244/1245 split only decided which
    // leaf could SEE each name, because getting a split wrong REMOVES verbs addons use, which is
    // worse than the superset it fixes. The five that were in NEITHER client map were left in
    // place as a separate question per name, and all five have since been answered:
    // `GetStringHeight` (1251), `SetRotation` and `SetPortraitToTexture` (the latter a 1.12
    // GLOBAL, and it lives there now), and `SetFormattedText` and `SetSize` (2142). Both leaves
    // are the client's own lists now.
    //
    // Copied out of the full table rather than installed twice, so one implementation stands behind
    // both visibilities — and note that the shared names use the IDENTICAL `const char*` in the
    // client's two tables (e.g. `GetDrawLayer` at `0x87c41c`), so de-duplicating by name would
    // drop one side.
    for (key, extra) in [
        (REG_TEXTURE_METHODS, &super::TEXTURE_ONLY_METHODS[..]),
        (REG_FONTSTRING_METHODS, &super::FONTSTRING_ONLY_METHODS[..]),
    ] {
        let leaf = lua.create_table()?;
        for name in super::REGION_MAP_METHODS
            .iter()
            .chain(super::REGION_LEAF_SHARED.iter())
            .chain(extra.iter())
        {
            let f: Value = m.get(*name)?;
            leaf.set(*name, f)?;
        }
        lua.set_named_registry_value(key, leaf)?;
    }
    for (meta_key, methods_key) in [
        (REG_TEXTURE_META, REG_TEXTURE_METHODS),
        (REG_FONTSTRING_META, REG_FONTSTRING_METHODS),
    ] {
        let meta = lua.create_table()?;
        meta.set("__index", lua.named_registry_value::<Table>(methods_key)?)?;
        lua.set_named_registry_value(meta_key, meta)?;
    }

    let title_meta = lua.create_table()?;
    title_meta.set("__index", title.clone())?;
    lua.set_named_registry_value(REG_TITLE_METHODS, title)?;
    lua.set_named_registry_value(REG_TITLE_META, title_meta)?;

    lua.set_named_registry_value(REG_REGION_METHODS, m)?;
    Ok(())
}

/// The layout [`super::layout::Handle`] a region anchors to by default: its **owner frame**'s id
/// (minted if needed), or [`SCREEN`] if the region has somehow lost its owner.
/// This region's global name, or `None` when it was declared anonymously.
///
/// Scans the region-name registry rather than mirroring the name onto the region: that registry is
/// the single authority (the widget arena deliberately holds none), and a second copy is one more
/// thing to drift. Linear in NAMED regions, and every caller is human-rate.
pub(super) fn region_name_of(model: &Model, id: u32) -> Option<String> {
    model
        .region_names
        .iter()
        .find(|&(_, &v)| v == id)
        .map(|(k, _)| k.clone())
}

/// Publish a region's global name into the **region-name registry** — the table a sibling's
/// `SetPoint`/`SetAllPoints` resolves a named `relativeTo` through
/// ([`super::object::layout_methods`]). First-wins, the same rule frames follow.
///
/// `CreateTexture(name)` does this for itself; the paths that *don't* are the typed widgets' own
/// sub-textures, which are created by a setter (`SetThumbTexture`, `SetColorWheelTexture`, …) and
/// named afterwards by the XML loader. Without this they land in `_G` and nowhere else, and a
/// `relativeTo="ColorPickerWheel"` on a sibling silently falls back to the parent frame — which is
/// exactly how the colour picker's value strip first ended up 224 px to the right of where the
/// reference puts it, anchored to the window instead of to the wheel.
pub(crate) fn publish_region_name(lua: &Lua, name: &str, region: &Table) {
    let Ok(id) = super::object::decode_id(region) else {
        return;
    };
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    model.region_names.entry(name.to_string()).or_insert(id);
}

/// The string `GetObjectType()` answers for this region, and the leaf `IsObjectType` matches.
///
/// The reference reads a per-class `.data` `const char*` through `vtable[+0x1c]`; ours reads the
/// arena's [`RegionKind`], which is the same fact stored once. A handle whose region has been
/// destroyed answers `"Region"` — the base every leaf also matches, so an identity question about
/// a dead handle degrades to the truthful half rather than naming a leaf it no longer is.
pub(super) fn region_type_name(model: &Model, rh: RegionHandle) -> &'static str {
    match model.arena.region(rh).map(|r| r.kind) {
        Some(RegionKind::Texture) => "Texture",
        Some(RegionKind::FontString) => "FontString",
        // The title region's own type name — it is a Region and nothing more (`CreateTitleRegion
        // 0x773910`).
        Some(RegionKind::Title) => "Region",
        None => "Region",
    }
}

/// Free one region and **every trace of its identity** — the arena slot, its paint, its resolved
/// rect, its name registration, and the stable id both directions.
///
/// Anything that had fetched its wrapper now resolves to a stale-handle error, which is the honest
/// answer for an object the widget destroyed. That matters because the client really does destroy:
/// `CSimpleButton::SetFontString 0x778d20` runs the *scalar deleting* destructor on the label it
/// replaces (`old->vtable[0](1)`, returning the storage to the FontString pool), so a caller
/// holding the old string is holding a freed object — orphaning it instead would leave a live,
/// parentless label that still answers every getter, which is a different and quieter bug.
///
/// One law, two callers ([`super::simplehtml`]'s block free and the button's label swap); it lives
/// here because the region side owns region lifetime, and because two hand-written copies of a
/// five-map teardown is how one of them ends up forgetting a map.
pub(crate) fn free_region(model: &mut Model, rh: RegionHandle) {
    model.region_data.remove(&rh);
    model.region_resolved.remove(&rh);
    if let Some(id) = model.region_to_id.remove(&rh) {
        model.id_to_region.remove(&id);
        // A NAMED region also holds a slot in the name registry — anonymous ones (every SimpleHTML
        // block, most labels) do not, which is why the first caller of this law never needed it.
        model.region_names.retain(|_, v| *v != id);
    }
    model.arena.destroy_region(rh);
    // A death is the archetypal STRUCTURAL change — it takes a node out of the layout roster and
    // its reverse edges with it, which is exactly what a per-node ledger cannot describe
    // (decision 1388).
    model.touch_layout();
}

pub(super) fn region_owner_id(model: &mut Model, rh: RegionHandle) -> u32 {
    match model.arena.region(rh).map(|r| r.owner) {
        Some(owner) => model.frame_id(owner),
        None => SCREEN,
    }
}

/// The client's **creation-path implicit anchor** (decision 1310): a per-region-type post-step
/// the real engine runs immediately after a region's LoadXML returns (`0x7701c0` texture /
/// `0x771480` fontstring — the same two fire from the Button state-texture and ButtonText paths,
/// and from Lua `CreateTexture`/`CreateFontString` only on a template-registry hit). Condition:
/// the region has a parent AND every one of its nine anchor slots is empty — any anchor from any
/// source suppresses it. Then:
///
/// - a **Texture** gets `SetAllPoints(parent)` — two corner anchors, TOPLEFT→TOPLEFT and
///   BOTTOMRIGHT→BOTTOMRIGHT at (0,0). Two opposing corners pin all four edges, so an authored
///   `<Size>` is **structurally unread** (the resolver law) — which is why the reference's
///   stack-split plate authors a vestigial 256×32 and renders 172×96 (B180).
/// - a **FontString** gets ONE middle-row `SetPoint` chosen by its live justify word
///   (`[this+0x120] & 7`: 1 → LEFT→LEFT, 4 → RIGHT→RIGHT, else CENTER→CENTER, offsets (0,0)) —
///   and its `<Size>` stays live (single anchor + W/H sizes the opposite edges).
///   [`RegionData::justify`] *is* that word — `SetJustifyH` writes it and a font-object link
///   merges into it behind the explicit mask — so reading it here is reading `+0x120`.
/// - a **Title region** gets nothing (verified negative), and a templateless Lua region is never
///   routed here at all: it stays rect-less and does not render.
///
/// These are ordinary anchors once installed: a later same-point `SetPoint` replaces only its own
/// slot and the other implicit corner survives (verified) — callers that re-apply authored anchors
/// over a materialized region must `ClearAllPoints` first, exactly as the reference's XML path
/// avoids the mix by running this step *after* `<Anchors>` load.
pub(crate) fn implicit_creation_anchor(model: &mut Model, rh: RegionHandle) {
    let Some((kind, owner)) = model.arena.region(rh).map(|r| (r.kind, r.owner)) else {
        return;
    };
    let owner_id = model.frame_id(owner);
    let data = model.region_data.entry(rh).or_default();
    if !data.anchors.is_empty() {
        return;
    }
    use crate::layout::Point;
    match kind {
        RegionKind::Texture => {
            data.anchors = vec![
                Anchor::new(Point::TopLeft, owner_id, Point::TopLeft, 0.0, 0.0),
                Anchor::new(Point::BottomRight, owner_id, Point::BottomRight, 0.0, 0.0),
            ];
        }
        RegionKind::FontString => {
            let point = justify_anchor_point(data.justify.0);
            data.anchors = vec![Anchor::new(point, owner_id, point, 0.0, 0.0)];
        }
        RegionKind::Title => return,
    }
    model.touch_layout();
}

/// The middle-row point a justify word selects — the compare chain the FontString creation
/// post-step (`0x771480`) and the Button label adopter (`CSimpleButton::SetFontString 0x778d20`)
/// share: `& 7`, then equality against LEFT (1) and RIGHT (4); every other value — the CENTER bit
/// and a cleared axis alike — falls to CENTER. What differs between the two callers is only WHOSE
/// word is read: the post-step reads the string's own (`+0x120`), the adopter reads the button's
/// normal font's (`+0x390`) — see [`super::button`]'s `adopt_label` (decision 1996).
pub(crate) fn justify_anchor_point(word: u32) -> crate::layout::Point {
    use crate::layout::Point;
    match word & crate::justify::H_MASK {
        0x01 => Point::Left,
        0x04 => Point::Right,
        _ => Point::Center,
    }
}

/// One middle-row anchor `point → the owner's same point, (0,0)`, installed only when the region
/// has no anchor of its own — the nine-slot scan every implicit-anchor site runs first, so any
/// anchor from any source suppresses it. [`implicit_creation_anchor`]'s FontString arm is this
/// with the region's own justify word; the Button adopter is this with the button's.
pub(crate) fn anchor_unanchored_at(
    model: &mut Model,
    rh: RegionHandle,
    point: crate::layout::Point,
) {
    let Some(owner) = model.arena.region(rh).map(|r| r.owner) else {
        return;
    };
    let owner_id = model.frame_id(owner);
    let data = model.region_data.entry(rh).or_default();
    if !data.anchors.is_empty() {
        return;
    }
    data.anchors = vec![Anchor::new(point, owner_id, point, 0.0, 0.0)];
    model.touch_layout();
}

/// [`implicit_creation_anchor`] behind a wrapper table — the loader-facing form (the loader holds
/// region wrappers, not handles), same seam as [`apply_font_parts`].
pub(crate) fn implicit_creation_anchor_lua(lua: &Lua, wrapper: &Table) -> mlua::Result<()> {
    let rh = region_handle_of(lua, wrapper)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    implicit_creation_anchor(&mut model, rh);
    Ok(())
}

/// The two things the shared ladder needs off a **region** receiver, read under one short `Model`
/// borrow that is dropped before the `_G` read (`super::object::anchor_args`'s contract, and the
/// frame twin's `frame_ladder_context`): the name the reference puts in its error strings, and the
/// name a leading `$parent` expands to.
///
/// A region's `$parent` is its **owner** frame (the region's own `+0x9c`), so the walk starts
/// there rather than one link higher. Its own name comes back out of `Model::region_names` — the
/// registry is name→id, and this is the only place that wants the inverse, so it is a scan on the
/// **error path** rather than a second map kept in step.
pub(super) fn region_ladder_context(lua: &Lua, rh: RegionHandle) -> (String, String) {
    let mut model = lua.app_data_mut::<Model>().expect("model");
    let id = model.region_id(rh);
    let who = model
        .region_names
        .iter()
        .find(|(_, &v)| v == id)
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| UNNAMED.to_string());
    let owner = model.arena.region(rh).map(|r| r.owner);
    let base = super::object::parent_token_base(&model, owner);
    (who, base)
}

/// Bit-exact equality for a region's explicit size — the layout gate's own lens
/// (`InputFingerprint::input` feeds `f32::to_bits`), so a setter's no-op detection and the gate
/// can never disagree; see [`anchor_bits_eq`].
pub(super) fn size_bits_eq(a: Option<(f32, f32)>, b: Option<(f32, f32)>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some((aw, ah)), Some((bw, bh))) => {
            aw.to_bits() == bw.to_bits() && ah.to_bits() == bh.to_bits()
        }
        _ => false,
    }
}

/// `Region:SetPoint(point [, relativeTo [, relativePoint]] [, x, y])` — the *same* binding as the
/// frame's (`0x7a2540` is registered once, on `CScriptRegion`), so the argument ladder and every
/// one of its raises come from [`super::object::anchor_args`]; this side supplies only what the
/// reference reads off a region — the owner as the layout parent, and
/// [`super::RegionData::anchors`] as the store.
pub(super) fn region_set_point(lua: &Lua, this: &Table, args: &MultiValue) -> mlua::Result<()> {
    let rh = region_handle_of(lua, this)?;
    let (who, base) = region_ladder_context(lua, rh);
    // The `_G` read inside the ladder runs with no model guard alive — see `region_ladder_context`.
    let p = parse_set_point(lua, args, &who, &base)?;

    let mut model = lua.app_data_mut::<Model>().expect("model");
    let me = model.region_id(rh);
    let owner = region_owner_id(&mut model, rh);
    let rel_to_id = resolve_rel_target(&model, &p.target, &who, "SetPoint", me, owner)?;
    let (point, rel_point, x, y) = (p.point, p.rel_point, p.x, p.y);

    let data = model.region_data.entry(rh).or_default();
    let new = Anchor::new(point, rel_to_id, rel_point, x, y);
    // Same no-op law as the frame twin (`layout_methods::set_point`): idempotent only when the
    // bit-identical anchor already holds the tail and no earlier entry carries this point.
    let same_at_tail = data.anchors.last().is_some_and(|a| anchor_bits_eq(a, &new))
        && !data.anchors[..data.anchors.len() - 1]
            .iter()
            .any(|a| a.point == point);
    if !same_at_tail {
        // The frame twin's law (decision 1388): re-pointing the same anchor at the same target is
        // a VALUE change and names its node; anything that moves the target set is structural.
        // This is the castbar spark's and every combat-text string's per-frame write.
        // …and a retarget names its node too (decision 1625) — see the frame twin.
        let structural = anchor_retarget_is_structural(&data.anchors, &new);
        let old_targets: Option<Vec<u32>> =
            structural.then(|| data.anchors.iter().map(|a| a.relative_to).collect());
        data.anchors.retain(|a| a.point != point);
        data.anchors.push(new);
        match old_targets {
            None => model.touch_layout_region(rh),
            Some(old) => {
                let new_targets: Vec<u32> = model.region_data[&rh]
                    .anchors
                    .iter()
                    .map(|a| a.relative_to)
                    .collect();
                model.touch_layout_retarget_region(rh, &old, &new_targets);
            }
        }
    }
    Ok(())
}

/// `Region:GetWidth()`/`GetHeight()` — **the virtual size getters**, per region class.
///
/// The Lua bindings are not field reads. `GetWidth 0x7a1e00` ends `ff 52 1c` and
/// `GetHeight 0x7a2030` ends `ff 52 20`: both dispatch through the
/// receiver's own **geometry vtable** — slots `+0x1c`/`+0x20` — which is the identical call the
/// rect resolver makes (`0x767579`). So a region's Lua-visible size and the size its rect is built
/// from are one number BY CONSTRUCTION on the reference, and this function is how they are one
/// number here (decision 1670; the resolver half is `layout::content_span` +
/// `layout::FONTSTRING_MIN_SPAN`, and both are read from here).
///
/// | class | law |
/// |---|---|
/// | plain frame / title region | the flat authored field, `0x768420` / `0x768410`. No content. |
/// | `CSimpleTexture` | `0x770720` / `0x770790`: authored when **not exactly `0.0`**, else the art's own texel extent, else `0.0`. |
/// | `CSimpleFontString` | `0x772930` / `0x772a60`: authored when not `0.0`, else the cached measure — and then **floored at one FrameXML unit**. |
///
/// Two details of the FontString row are worth spelling out, because both were wrong here before
/// and neither is guessable from the name:
///
/// * **The authored value WINS.** The old code preferred the measure and fell back to the
///   authored size;
///   the reference's `jp` at `0x77294a` skips the measure entirely when the authored value is
///   non-zero. Per axis, not per region — `<Size x="290" y="0"/>` takes 290 from the author and
///   the height from the text.
/// * **The width is the NATURAL, unwrapped extent** — `0x772890`'s `[fs+0xfc]`, measured with no
///   wrap constraint, the very cell `GetStringWidth 0x79e510` returns. The *height* is the other
///   cell, `0x7729b0`'s `[fs+0x100]`, which IS the wrapped line count × line height. So a wrapped
///   string reports a width wider than its own box and a height taller than one line, and that
///   asymmetry is the client's.
pub(super) fn measured_wh(lua: &Lua, this: &Table) -> mlua::Result<(f32, f32)> {
    let rh = region_handle_of(lua, this)?;
    // Same-tick measure when a host font engine is installed — see `region::text`'s `natural_w`.
    // A no-op for a Texture (not a FontString) and for an already-current measure.
    super::measure::ensure_measured(lua, rh);
    let model = lua.app_data_ref::<Model>().expect("model");
    Ok(virtual_span(&model, rh))
}

/// [`measured_wh`]'s body, on a borrowed model — the form the engine's own callers want.
pub(super) fn virtual_span(model: &Model, rh: RegionHandle) -> (f32, f32) {
    let Some(d) = model.region_data.get(&rh) else {
        return (0.0, 0.0);
    };
    let (aw, ah) = d.size.unwrap_or((0.0, 0.0));
    match model.arena.region(rh).map(|r| r.kind) {
        Some(RegionKind::FontString) => {
            // The key carries the owner's effective_scale ([`RegionData::measure_key`]) — the same
            // recipe the request loop stamps, or every read under a SetScale'd owner reports
            // stale. Unlike the layout sweep, the getter DOES key-check: the sweep holds a
            // last-known box so a line whose text just changed does not flicker for the frame the
            // re-measure is in flight, and a getter that did the same would hand a caller the
            // previous string's metric (the whisper header's latched inset).
            let scale = model
                .arena
                .region(rh)
                .and_then(|r| model.arena.frame(r.owner))
                .map(|f| f.effective_scale)
                .unwrap_or(1.0);
            let m = d.measured.filter(|m| m.key == d.measure_key(scale));
            // **The floor applies to a KNOWN extent, and a pending measure is not one.** On the
            // reference every extent is known — the getter measures inline — so `0.0` never comes
            // back and the question never arises. Ours can be *waiting*, which is not a size but
            // the absence of an answer. **Where that state still lives is a VM with no measurer
            // installed** (`script::measure`'s is optional): the app seats one at the load edge
            // (2028), so no in-app caller observes a pending measure any more, and the three
            // convergence drivers this used to name went with their windows —
            // `BenillaGossipRow_Resize` (gossip, 1751), our tab fit (1993/2028) and our quest
            // panel (1944). Flooring a pending measure to one unit hands a measurer-less caller a
            // number where it should read "not yet"; the surviving `OnUpdate` fits
            // (`OptionsScroll_Fit`, `BenillaScroll_ResizeChild`) guard on RECTS, not on this.
            //
            // A genuinely EMPTY string is a different thing: its extent is known and it is zero, so
            // it floors — which is the case the reference's floor exists for. The layout sweep
            // floors unconditionally because a rect has to exist on every frame and nothing reads
            // it as a sentinel (decision 1664); the difference between the two is exactly this
            // pending state, which the reference does not have.
            //
            // "Known" is the same test the layout sweep applies: **empty text needs no
            // round-trip** — nothing ever measures an empty string, and its extent is known
            // without asking. So `""` is a known zero (⇒ one unit) while a pending measure on real
            // text is no answer at all (⇒ `0.0`), and the two are only indistinguishable if you
            // look at `measured` alone.
            let floor = super::layout::FONTSTRING_MIN_SPAN;
            let known = |from_measure: fn(&crate::script::types::MeasuredText) -> f32| {
                if d.text.as_deref().is_none_or(str::is_empty) {
                    Some(0.0)
                } else {
                    m.as_ref().map(from_measure)
                }
            };
            let w = if aw != 0.0 {
                aw.max(floor)
            } else {
                known(|m| m.natural_w).map_or(0.0, |v| v.max(floor))
            };
            let h = if ah != 0.0 {
                ah.max(floor)
            } else {
                known(|m| m.h).map_or(0.0, |v| v.max(floor))
            };
            (w, h)
        }
        Some(RegionKind::Texture) => {
            let texel = (aw == 0.0 || ah == 0.0)
                .then(|| super::layout::content_span(d, model.texture_size_probe.as_ref()))
                .flatten();
            (
                if aw != 0.0 {
                    aw
                } else {
                    texel.map_or(0.0, |t| t.0)
                },
                if ah != 0.0 {
                    ah
                } else {
                    texel.map_or(0.0, |t| t.1)
                },
            )
        }
        // A title region is a bare `CScriptRegion` — geometry vtable `0x81c9c8`, whose `+0x1c`
        // is the unoverridden `0x768420`. The flat field, like a frame.
        _ => (aw, ah),
    }
}
