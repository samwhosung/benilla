//! The region paint methods: what a Texture shows and how it is tinted, blended, cropped and
//! layered.

use mlua::{Lua, MultiValue, Table, Value};

use crate::script::object::{draw_layer_from_str, draw_layer_name, tonumber_f32};
use crate::script::texel::TexSet;
use crate::script::{BlendMode, Model, TexCoords};

use super::region_handle_of;

/// Install the paint methods into `m`.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // SetAlpha/GetAlpha: the region's own alpha, not the owner frame's; stock
    // `CastingBarFrame.lua:133` reads it back to ramp its flash. `RegionData::alpha` has the draw.
    m.set(
        "SetAlpha",
        lua.create_function(|lua, (this, alpha): (Table, f32)| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().alpha = Some(alpha.clamp(0.0, 1.0));
            Ok(())
        })?,
    )?;

    m.set(
        "GetAlpha",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model
                .region_data
                .get(&rh)
                .and_then(|d| d.alpha)
                .unwrap_or(1.0))
        })?,
    )?;

    // SetVertexColor (`0x79abd0`) reads r, g and b with a bare `lua_tonumber`, so nil or a table is
    // 0 and never raises; stock `QuestLogFrame.lua:337` can pass three nils. The alpha is staged
    // 1.0 (`0x79ad43`) but re-tested at the end (`0x79adfe`): when it is not a `lua_isnumber` the
    // colour read back at the start (`0x79ac81`) lends its alpha byte (`0x79ae0a`), so a
    // three-argument call keeps the region's alpha. Stock `ActionButton_UpdateUsable`
    // (`ActionButton.lua:274-281`) leans on it: a ring `ActionButton_ShowGrid` dimmed stays dim.
    m.set(
        "SetVertexColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Value)| {
                let a = lua.coerce_number(a).ok().flatten().map(|n| n as f32);
                let (r, g, b) = (
                    tonumber_f32(lua, &r),
                    tonumber_f32(lua, &g),
                    tonumber_f32(lua, &b),
                );
                let rh = region_handle_of(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let d = model.region_data.entry(rh).or_default();
                let kept = d.vertex_color.map_or(1.0, |c| c[3]);
                d.set_vertex_color(Some([r, g, b, a.unwrap_or(kept)]));
                // The slot `SetTextColor` writes on a FontString, so it overrides the font object.
                d.font_explicit.color = true;
                Ok(())
            },
        )?,
    )?;

    // GetVertexColor (`0x79aa50`): white when never set, and under a gradient (`0x77f8c0` answers
    // white unless the slot holds exactly one colour).
    m.set(
        "GetVertexColor",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let c = model
                .region_data
                .get(&rh)
                .and_then(|d| d.vertex_color)
                .unwrap_or([1.0, 1.0, 1.0, 1.0]);
            Ok((c[0], c[1], c[2], c[3]))
        })?,
    )?;

    // SetGradientAlpha(orientation, minR, minG, minB, minA, maxR, maxG, maxB, maxA) (`0x79b180`)
    // and SetGradient without the alphas (`0x79ae30`) write the four vertex colours (`0x77f910`),
    // the slot `SetVertexColor` writes one colour into, so each clears the other. The orientation
    // is a `lua_isstring` matched case-insensitively against HORIZONTAL and VERTICAL (`0x6f1a20`);
    // anything else raises the usage line (`0x79b126`, `0x79b54a`) and writes nothing.
    for (name, with_alpha, usage) in [
        (
            "SetGradientAlpha",
            true,
            "(\"orientation\", minR, minG, minB, minA, maxR, maxG, maxB, maxA)",
        ),
        (
            "SetGradient",
            false,
            "(\"orientation\", minR, minG, minB, maxR, maxG, maxB)",
        ),
    ] {
        m.set(
            name,
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let mut it = args.into_iter();
                let this: Table = match it.next() {
                    Some(Value::Table(t)) => t,
                    _ => return Err(mlua::Error::runtime("expected a region")),
                };
                let rh = region_handle_of(lua, &this)?;
                let vertical = match it.next() {
                    Some(Value::String(s)) => s.to_str().ok().and_then(|t| orientation(&t)),
                    // A number is a `lua_isstring` too, and names neither orientation.
                    _ => None,
                };
                let Some(vertical) = vertical else {
                    let model = lua.app_data_ref::<Model>().expect("model");
                    let who = crate::script::object::decode_id(&this)
                        .ok()
                        .and_then(|id| super::region_name_of(&model, id));
                    return Err(mlua::Error::runtime(format!(
                        "Usage: {}:{name}{usage}",
                        who.as_deref().unwrap_or("<unnamed>")
                    )));
                };
                let n = if with_alpha { 8 } else { 6 };
                let mut c = [0.0f32; 8];
                for (i, slot) in c.iter_mut().take(n).enumerate() {
                    let v = it.next();
                    // Every channel is a bare `lua_tonumber` (`0x79aef3`-`0x79b083`, `0x79b242`-
                    // `0x79b490`) except the two alphas, which are `lua_isnumber`-gated with
                    // default 1 (`0x79b30d`, `0x79b480`).
                    let raw = match v {
                        Some(v) if with_alpha && matches!(i, 3 | 7) => lua
                            .coerce_number(v)
                            .ok()
                            .flatten()
                            .map_or(1.0, |n| n as f32),
                        Some(v) => tonumber_f32(lua, &v),
                        None if with_alpha && matches!(i, 3 | 7) => 1.0,
                        None => 0.0,
                    };
                    *slot = clamp_channel(raw);
                }
                let (start, end) = if with_alpha {
                    ([c[0], c[1], c[2], c[3]], [c[4], c[5], c[6], c[7]])
                } else {
                    // SetGradient has no alpha stops: both ends are opaque.
                    ([c[0], c[1], c[2], 1.0], [c[3], c[4], c[5], 1.0])
                };
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let d = model.region_data.entry(rh).or_default();
                d.gradient = Some(crate::script::Gradient {
                    vertical,
                    start,
                    end,
                });
                d.vertex_color = None;
                Ok(())
            })?,
        )?;
    }

    m.set(
        "SetTexture",
        // The path form reads one argument (`0x770200`), the colour form (`0x770360`) up to four,
        // and extras such as `SetTexture(path, true)` are ignored: the wrap flag is 0 always
        // (`0x79bda5`). It answers 1, or nil when the path form's file does not load, as the
        // host's `Model::texture_probe` judges; with no probe the path form answers nil.
        lua.create_function(
            |lua, (this, arg, g, b, a): (Table, Value, Value, Value, Value)| {
                set_texture(lua, this, arg, g, b, a, false)
            },
        )?,
    )?;

    // SetDesaturated(flag) (`0x79c1e0`), Texture only, answers shaderSupported, which stock
    // `ItemButtonTemplate.lua:69` reads to fall back to a grey tint; the renderer greys the texel,
    // so this answers 1. The flag is `GetBoolOrDefault` (`0x6f1c10`, default 1, `0x79c27e`): no
    // argument is on, hence the `MultiValue`, nil is off, and a string goes by its first byte.
    m.set(
        "SetDesaturated",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_iter();
            let this: Table = match args.next() {
                Some(Value::Table(t)) => t,
                _ => return Ok(Value::Nil),
            };
            let on = crate::script::binding_abi::bool_or_default(args.next().as_ref(), true);
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().desaturated = on;
            // 1, not true: the reference pushes the number 1 (`0x6f3810`).
            Ok(Value::Number(1.0))
        })?,
    )?;

    // GetTexture (`0x79ba70`), Texture only, answers one value: "Solid Texture" for the colour
    // form (`0x835708`), not nil, and a path cut at its last `.` (`0x79baf0`), even a dot in a
    // directory name, as the client cuts it.
    m.set(
        "GetTexture",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_mut::<Model>().expect("model");
            let Some(data) = model.region_data.get(&rh) else {
                return Ok(None);
            };
            if data.tex.fill().is_some() {
                return Ok(Some("Solid Texture".to_string()));
            }
            Ok(data.tex.path().as_ref().map(|t| match t.rfind('.') {
                Some(i) => t[..i].to_string(),
                None => t.clone(),
            }))
        })?,
    )?;

    // SetAlphaGradient(start, length) (`0x79cc80`), a FontString's write-on reveal: answers 1
    // (`0x79cd58`) or nil (`0x79cd6d`) for whether `start` is still inside the text in chars, and
    // stock `QuestFrame.lua:558` advances until it is not.
    m.set(
        "SetAlphaGradient",
        lua.create_function(|lua, (this, start, length): (Table, f32, f32)| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let data = model.region_data.entry(rh).or_default();
            data.alpha_gradient = Some((start, length));
            let chars = data.text.as_deref().map_or(0, |t| t.chars().count());
            Ok(crate::script::binding_abi::flag(start < chars as f32))
        })?,
    )?;

    // SetBlendMode (`0x79a950`, the alphaMode enum `0x811aa8`) stores the mode as given; an
    // unknown name leaves it alone, as the enum lookup does. Only ADD draws differently here, the
    // others as straight alpha.
    m.set(
        "SetBlendMode",
        lua.create_function(|lua, (this, mode): (Table, String)| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if let Some(blend) = BlendMode::parse(&mode) {
                model.region_data.entry(rh).or_default().blend = blend;
            }
            Ok(())
        })?,
    )?;
    // GetBlendMode (`0x79a890`, table `0x87c128`): one string, the enum's spelling, "BLEND" when
    // never set (the ctor's `[+0xd0] = 2`, `0x76fc64`). The reference's nil, for a mode outside
    // its table, cannot arise from the five modes here.
    m.set(
        "GetBlendMode",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model
                .region_data
                .get(&rh)
                .map_or(BlendMode::default(), |d| d.blend)
                .name())
        })?,
    )?;

    // SetTexCoordModifiesRect/GetTexCoordModifiesRect (`0x79c080`/`0x79c120`, table `0x87c128`):
    // the setter writes `[texture+0x124]` (`0x79c113`), as does installing the texture as a
    // StatusBar's bar (`0x783220`); the getter answers 1 or nil. Stored, not applied: there a set
    // flag makes `SetTexCoord` re-derive the rect from the UV quad (`0x770462`).
    m.set(
        "SetTexCoordModifiesRect",
        lua.create_function(|lua, (this, args): (Table, MultiValue)| {
            let rh = region_handle_of(lua, &this)?;
            // `GetBoolOrDefault` (`0x6f1c10`) with default 1 (`0x79c10e`): a bare call sets it.
            let on = crate::script::binding_abi::bool_or_default(args.front(), true);
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model
                .region_data
                .entry(rh)
                .or_default()
                .tex_coord_modifies_rect = on;
            Ok(())
        })?,
    )?;
    m.set(
        "GetTexCoordModifiesRect",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model
                    .region_data
                    .get(&rh)
                    .is_some_and(|d| d.tex_coord_modifies_rect),
            ))
        })?,
    )?;

    // SetDrawLayer(layer) (`0x79a780`) reads the layer name alone, against the five-row table at
    // `0x811a80` (`0x6f18b0`); a further argument is never read, as 1.12 has no sub-level within a
    // layer. A name off the table, or no name, raises the Usage line naming the region
    // (`0x87c42c`), and the layer stays.
    m.set(
        "SetDrawLayer",
        lua.create_function(|lua, (this, layer): (Table, Value)| {
            let rh = region_handle_of(lua, &this)?;
            let dl = match &layer {
                Value::String(s) => draw_layer_from_str(&s.to_str()?),
                _ => None,
            };
            let Some(dl) = dl else {
                let name = {
                    let model = lua.app_data_ref::<Model>().expect("model");
                    crate::script::object::decode_id(&this)
                        .ok()
                        .and_then(|id| super::region_name_of(&model, id))
                };
                return Err(mlua::Error::runtime(format!(
                    "Usage: {}:SetDrawLayer(\"layer\")",
                    name.as_deref().unwrap_or("<unnamed>")
                )));
            };
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if let Some(region) = model.arena.region_mut(rh) {
                region.draw_layer = dl;
            }
            Ok(())
        })?,
    )?;

    // GetDrawLayer (Texture `0x79a6c0`, FontString `0x79c660`): the layer name alone.
    m.set(
        "GetDrawLayer",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            let dl = model
                .arena
                .region(rh)
                .map_or(crate::order::DrawLayer::Artwork, |r| r.draw_layer);
            Ok(draw_layer_name(dl))
        })?,
    )?;

    // No Texture `SetRotation`: 1.12 registers it only on PlayerModel (`0x505f00`, table
    // `0x84f1fc`), in neither region map (`0x87c128`, `0xcf5400`).

    // SetTexCoord(left, right, top, bottom), XML `<TexCoords>`: a UV sub-rect in 0..1, origin top
    // left. SetTexCoord(ULx, ULy, LLx, LLy, URx, URy, LRx, LRy): any UV quad, as stock
    // `TaxiFrame.lua:223` draws route lines, stored per corner in screen winding.
    m.set(
        "SetTexCoord",
        // Each coordinate is a bare `lua_tonumber` (`0x79beb0`), so nil or a table is 0; only the
        // count is gated, 4 or 8 (`0x79bf5d`).
        lua.create_function(|lua, (this, args): (Table, mlua::Variadic<Value>)| {
            let rest: Vec<f32> = args.iter().map(|v| tonumber_f32(lua, v)).collect();
            let rh = region_handle_of(lua, &this)?;
            let coords = match rest.len() {
                4 => Some(TexCoords::Rect([rest[0], rest[1], rest[2], rest[3]])),
                // The arguments come UL, LL, UR, LR; `TexCoords::Corners` stores TL, TR, BR, BL.
                8 => Some(TexCoords::Corners([
                    [rest[0], rest[1]], // UL → TL
                    [rest[4], rest[5]], // UR → TR
                    [rest[6], rest[7]], // LR → BR
                    [rest[2], rest[3]], // LL → BL
                ])),
                // No arguments resets to the full texture, which is not 1.12's: its gate takes 4
                // or 8.
                0 => None,
                n => {
                    return Err(mlua::Error::runtime(format!(
                        "SetTexCoord: expected 4 (edges) or 8 (corner pairs) args, got {n}"
                    )))
                }
            };
            lua.app_data_mut::<Model>()
                .expect("model")
                .region_data
                .entry(rh)
                .or_default()
                .tex_coords = coords;
            Ok(())
        })?,
    )?;

    // GetTexCoord: the four corners, the full texture when never set.
    m.set(
        "GetTexCoord",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            // Eight values in the order of `SetTexCoord`'s usage string (`0x87c538`): ULx, ULy,
            // LLx, LLy, URx, URy, LRx, LRy. The four-edge form is setter-only.
            let corners = model
                .region_data
                .get(&rh)
                .and_then(|d| d.tex_coords)
                .map_or([[0.0, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0]], |tc| {
                    match tc {
                        // Stored as TL, TR, BR, BL; Lua wants UL, LL, UR, LR.
                        crate::script::types::TexCoords::Corners(c) => [c[0], c[3], c[1], c[2]],
                        crate::script::types::TexCoords::Rect(_) => {
                            let [l, r, t, b] = tc.edges();
                            [[l, t], [l, b], [r, t], [r, b]]
                        }
                    }
                });
            Ok((
                corners[0][0],
                corners[0][1],
                corners[1][0],
                corners[1][1],
                corners[2][0],
                corners[2][1],
                corners[3][0],
                corners[3][1],
            ))
        })?,
    )?;
    Ok(())
}

impl crate::script::UiScript {
    /// Install the host's texture-path check behind the path form of `SetTexture`'s 1-or-nil
    /// answer; without one every path answers nil.
    pub fn set_texture_probe(&mut self, probe: Box<dyn Fn(&str) -> bool>) {
        self.model_mut().texture_probe = Some(probe.into());
    }

    /// Install the host's font-path check behind `SetFont`'s 1-or-nil answer; without one every
    /// non-empty path answers 1.
    pub fn set_font_probe(&mut self, probe: Box<dyn Fn(&str) -> bool>) {
        self.model_mut().font_probe = Some(probe.into());
    }

    /// Install the host's texel-size check, which lets an axis sized 0 take its span from the art
    /// as `CSimpleTexture::GetWidth` (`0x770720`) falls back to the texels; without one such a
    /// region stays where it was.
    pub fn set_texture_size_probe(&mut self, probe: Box<crate::script::TextureSizeFn>) {
        let mut model = self.model_mut();
        model.texture_size_probe = Some(probe.into());
        // The reference has every art's size at hand (`0x44b3b0` loads it), so a probe installed
        // late re-resolves each texture that takes a span from its art, as if it had been there.
        let art_sized: Vec<u32> = model
            .region_data
            .iter()
            .filter(|(_, d)| {
                !d.anchors.is_empty() && d.size.is_none_or(|(w, h)| w == 0.0 || h == 0.0)
            })
            .filter_map(|(rh, _)| model.region_to_id.get(rh).copied())
            .collect();
        for id in art_sized {
            model.queue_invalidate(id);
        }
        // No per-node hash holds a probe's answer: a full-scope rebuild, as for a screen move.
        model.layout_scope.invalidate();
        model.touch_layout();
    }
}

/// What an XML `file=` load came to ([`load_texture_file`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileLoad {
    /// The host's texture probe found the file.
    Loaded,
    /// The probe did not: `0x770200` returned 0 and the texture kept what it had (`0x7702ac`).
    Failed,
    /// No probe is installed (a test harness): the path is stored, with no verdict.
    Unprobed,
}

/// The XML `<Texture file=>` load of a region (`0x76fe20` → `0x770200`): the path form of
/// [`set_texture`] with the wrap flag the element's `<TexCoords>` decide.
pub(crate) fn load_texture_file(
    lua: &Lua,
    this: &Table,
    file: &str,
    wrap: bool,
) -> mlua::Result<FileLoad> {
    let probed = lua
        .app_data_ref::<Model>()
        .expect("model")
        .texture_probe
        .is_some();
    let path = Value::String(lua.create_string(file)?);
    let answer = set_texture(
        lua,
        this.clone(),
        path,
        Value::Nil,
        Value::Nil,
        Value::Nil,
        wrap,
    )?;
    Ok(match (probed, answer) {
        (false, _) => FileLoad::Unprobed,
        (true, Value::Nil) => FileLoad::Failed,
        (true, _) => FileLoad::Loaded,
    })
}

/// `Texture:SetTexture` (`0x79bb40`) and the XML `<Texture file=>` load (`0x76fe20`), the two
/// callers of `0x770200` that differ in its wrap argument: the binding passes 0 (`0x79bda5`), the
/// loader passes 1 only when the element's `<TexCoords>` leave [0, 1] (`0x77012d`).
pub(crate) fn set_texture(
    lua: &Lua,
    this: Table,
    arg: Value,
    g: Value,
    b: Value,
    a: Value,
    wrap: bool,
) -> mlua::Result<Value> {
    let rh = region_handle_of(lua, &this)?;
    // `lua_isnumber` (`0x79bbd5`) picks the colour form, a numeric string included.
    let colour_form = lua.coerce_number(arg.clone()).ok().flatten().is_some();
    // Scoped so the model borrow ends before the layout touch; the path arm drops it
    // mid-way to call the host probe.
    let loaded = {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        {
            let Model {
                region_data,
                texels,
                ..
            } = &mut *model;
            // SetTexture drops any portrait mask and live-unit binding.
            let data = region_data.entry(rh).or_default();
            data.circular = false;
            data.portrait_unit = None;
            // SetTexture clears the desaturation: `0x770200` writes the shader slot
            // (`+0x128`) from an argument the binding (`0x79bb40`) always passes NULL. The
            // same file returns first (`0x770225`) and keeps it, nil or "" clear it, and
            // the colour form (`0x770360`) never writes it.
            let same_file = matches!(&arg, Value::String(s)
                    if s.to_str().is_ok_and(|s| texels.holds_file(&data.tex, &s)));
            if !same_file && !colour_form {
                data.desaturated = false;
            }
        }
        // Both forms write the one texture slot (`+0xcc`), a file or an 8×8 solid, so
        // each clears the other; neither touches the vertex colour (`+0xb8`), so a
        // tint outlives its art.
        let (set, loaded) = match &arg {
            // The colour form (`lua_isnumber`, `0x79bbd5`, so a numeric string too): g
            // and b are a bare `lua_tonumber`, and alpha is `lua_isnumber`-gated with
            // default 1.
            _ if colour_form => {
                let alpha = lua
                    .coerce_number(a.clone())
                    .ok()
                    .flatten()
                    .map_or(1.0, |n| n as f32);
                let fill = [
                    tonumber_f32(lua, &arg),
                    tonumber_f32(lua, &g),
                    tonumber_f32(lua, &b),
                    alpha,
                ];
                (Some(TexSet::Fill(fill)), true)
            }
            // "" clears, as nil does (stock `QuestLogFrame.lua:166`), and answers nil
            // like a failed load; the reference's answer for "" is untraced.
            Value::String(s) if s.to_str()?.is_empty() => (Some(TexSet::None), false),
            // Ask the probe before writing: on a failed load the reference returns 0
            // and keeps the texture it had (`0x770288`, `0x77028e`-`0x7702b2`). With
            // no probe the path is stored, though the answer is nil.
            Value::String(s) => {
                let path = s.to_str()?.to_string();
                drop(model);
                let resolvable = {
                    let model = lua.app_data_ref::<Model>().expect("model");
                    model
                        .texture_probe
                        .as_ref()
                        .is_none_or(|probe| probe(&path))
                };
                model = lua.app_data_mut::<Model>().expect("model");
                let had_probe = model.texture_probe.is_some();
                if resolvable {
                    // Resolvable already: the probe need not run again.
                    let Model {
                        region_data,
                        texels,
                        ..
                    } = &mut *model;
                    let data = region_data.entry(rh).or_default();
                    let set = if wrap {
                        TexSet::WrapFile(path)
                    } else {
                        TexSet::File(path)
                    };
                    texels.set(&mut data.tex, set, |_| false);
                }
                (None, resolvable && had_probe)
            }
            // nil clears, so the region draws nothing, and answers 1 (`0x79bb40`).
            Value::Nil => (Some(TexSet::None), true),
            _ => (None, false),
        };
        if let Some(set) = set {
            model.set_texel(rh, set);
        }
        loaded
    };
    // The draw only (`0x77fdf0`); an art-sized texture now holds a state its next
    // reaching resolve need not give back.
    lua.app_data_mut::<Model>().expect("model").note_art(rh);
    Ok(if loaded {
        Value::Number(1.0)
    } else {
        Value::Nil
    })
}

/// A gradient orientation token (`0x6f1a20` over `0x811b00`): `Some(true)` for VERTICAL,
/// `Some(false)` for HORIZONTAL, in any case; `None` for anything else.
pub(crate) fn orientation(token: &str) -> Option<bool> {
    if token.eq_ignore_ascii_case("VERTICAL") {
        Some(true)
    } else if token.eq_ignore_ascii_case("HORIZONTAL") {
        Some(false)
    } else {
        None
    }
}

/// A gradient channel's clamp to `0..=1` (`0x79af05`-`0x79af2d`): the two `fcom`s fall through an
/// unordered compare, so NaN lands on 1.
fn clamp_channel(v: f32) -> f32 {
    if v.is_nan() {
        1.0
    } else {
        v.clamp(0.0, 1.0)
    }
}
