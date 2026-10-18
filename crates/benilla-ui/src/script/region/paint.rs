//! The region paint methods: what a Texture shows and how it is tinted, blended, cropped and
//! layered.

use mlua::{Lua, MultiValue, Table, Value};

use crate::script::binding_abi::usage;
use crate::script::object::{draw_layer_from_str, draw_layer_name, tonumber_f32};
use crate::script::texel::TexSet;
use crate::script::{BlendMode, Model, TexCoords};

use super::region_handle_of;

/// Install the paint methods into `m`.
pub(super) fn install(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // SetAlpha/GetAlpha: the region's own alpha, not the owner frame's; stock
    // `CastingBarFrame.lua:133` reads it back to ramp its flash. `RegionData::alpha` has the draw.
    // `Texture:SetAlpha` (`0x79b580`) and `FontString:SetAlpha` (`0x79cb70`) take a
    // `lua_isnumber` or raise the usage line (`0x87c51c`), and store `__ftol(a * 255)`'s low byte,
    // unclamped (`0x79b638`, `0x79cc28`); `GetAlpha` reads it times the double 1/255 (`0x79b754`).
    m.set(
        "SetAlpha",
        lua.create_function(|lua, (this, alpha): (Table, Value)| {
            let rh = region_handle_of(lua, &this)?;
            let Some(alpha) = lua.coerce_number(alpha)? else {
                return Err(usage(lua, &this, "SetAlpha(alpha)"));
            };
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().alpha =
                Some(crate::color::wrap_alpha_byte(alpha));
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
                .map_or(1.0, crate::color::alpha_f64))
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
                let given = crate::script::object::alpha_arg(lua, &a).is_some();
                let c = crate::script::object::colour_args(lua, &r, &g, &b, &a);
                let rh = region_handle_of(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let d = model.region_data.entry(rh).or_default();
                let kept = d.vertex_color_or_white().a;
                d.set_vertex_color(Some(if given { c } else { c.with_alpha(kept) }));
                // The slot `SetTextColor` writes on a FontString, so it overrides the font object.
                d.font_mask.clear(super::super::FontMask::COLOR);
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
                .map_or(crate::color::Rgba8::WHITE, |d| d.vertex_color_or_white());
            Ok(crate::script::object::colour_values(c))
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
                    return Err(self::usage(lua, &this, &format!("{name}{usage}")));
                };
                // Every channel is a bare `lua_tonumber` (`0x79aef3`-`0x79b083`, `0x79b242`-
                // `0x79b490`) except the two alphas, which are `lua_isnumber`-gated with default
                // 1 (`0x79b30d`, `0x79b480`); each stop packs as every colour setter does
                // (`0x79b353`-`0x79b3ad`). SetGradient has no alpha stops: both ends are opaque.
                let n = if with_alpha { 8 } else { 6 };
                let v: Vec<Value> = (0..n).map(|_| it.next().unwrap_or(Value::Nil)).collect();
                let nil = Value::Nil;
                let stop = |at: usize| {
                    let alpha = if with_alpha { &v[at + 3] } else { &nil };
                    crate::script::object::colour_args(lua, &v[at], &v[at + 1], &v[at + 2], alpha)
                };
                let (start, end) = (stop(0), stop(if with_alpha { 4 } else { 3 }));
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
        // host's `Model::texture_probe` judges ([`set_texture`]).
        lua.create_function(
            |lua, (this, arg, g, b, a): (Table, Value, Value, Value, Value)| {
                set_texture(lua, this, arg, g, b, a)
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

    // IsDesaturated (`0x79c2c0`), Texture only: 1 when the shader slot (`+0x128`) holds the
    // desaturate shader (`0x770620(1)`, `0x79c354`), else nil. `SetDesaturated` answers the shader
    // supported, so this is the stored flag, which a file that loads clears ([`load_texture_name`]).
    m.set(
        "IsDesaturated",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                model.region_data.get(&rh).is_some_and(|d| d.desaturated),
            ))
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

    // SetBlendMode (`0x79a950`) looks a `lua_isstring` argument (`0x79a9e0`, a number too) up in
    // the alphaMode enum, ignoring case (`0x6f1920` over `0x811aa8`), and stores it
    // (`0x79aa13`); a miss or any other value raises the usage (`0x79aa20`, `0x87c44c`) and
    // stores nothing.
    m.set(
        "SetBlendMode",
        lua.create_function(|lua, (this, mode): (Table, Value)| {
            let rh = region_handle_of(lua, &this)?;
            let blend = match &mode {
                Value::String(_) | Value::Number(_) | Value::Integer(_) => lua
                    .coerce_string(mode.clone())?
                    .and_then(|m| BlendMode::parse(&m.to_string_lossy())),
                _ => None,
            };
            let Some(blend) = blend else {
                return Err(usage(lua, &this, "SetBlendMode(\"mode\")"));
            };
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.region_data.entry(rh).or_default().blend = blend;
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
                return Err(usage(lua, &this, "SetDrawLayer(\"layer\")"));
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
        // count is gated, `lua_gettop` less the receiver (`0x79bf58`), so a trailing nil counts.
        // Any count but 4 or 8, none included, raises the usage (`0x79bf6c`, `0x87c538`).
        lua.create_function(|lua, (this, args): (Table, mlua::Variadic<Value>)| {
            let rest: Vec<f32> = args.iter().map(|v| tonumber_f32(lua, v)).collect();
            let rh = region_handle_of(lua, &this)?;
            // The eight come UL, LL, UR, LR; `TexCoords::Corners` stores TL, TR, BR, BL.
            let coords = match rest.len() {
                4 => TexCoords::Rect([rest[0], rest[1], rest[2], rest[3]]),
                8 => TexCoords::Corners([
                    [rest[0], rest[1]], // UL → TL
                    [rest[4], rest[5]], // UR → TR
                    [rest[6], rest[7]], // LR → BR
                    [rest[2], rest[3]], // LL → BL
                ]),
                _ => return Err(usage(lua, &this, TEX_COORD_USAGE)),
            };
            lua.app_data_mut::<Model>()
                .expect("model")
                .region_data
                .entry(rh)
                .or_default()
                .tex_coords = Some(coords);
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

/// `SetTexCoord`'s usage line (`0x87c538`), after its `%s:`.
const TEX_COORD_USAGE: &str = "SetTexCoord(minX, maxX, minY, maxY) or SetTexCoord(ULx, ULy, LLx, \
                               LLy, URx, URy, LRx, LRy)";

/// The XML `alphaMode=` of a texture (`0x770176`): read with the same enum lookup as
/// `SetBlendMode` (`0x6f1920`), but an empty value skips and an unknown one is ignored, with no
/// log (`0x770185`, `0x770195`).
pub(crate) fn load_blend_mode(lua: &Lua, this: &Table, mode: &str) -> mlua::Result<()> {
    let Some(blend) = BlendMode::parse(mode) else {
        return Ok(());
    };
    let rh = region_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    model.region_data.entry(rh).or_default().blend = blend;
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
                !d.anchors.is_empty() && d.size.is_none_or(|(w, h)| w.is_zero() || h.is_zero())
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

/// What `CSimpleTexture::SetTexture(name)` (`0x770200`) came to ([`load_texture_name`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileLoad {
    /// It returned 1: the file it already shows, an empty name, or a file the probe found.
    Loaded,
    /// It returned 0: the probe did not find the file, and the texture kept what it had.
    Failed,
    /// No probe is installed (a test harness): the path is stored, with no verdict.
    Unprobed,
}

/// `CSimpleTexture::SetTexture(name)` (`0x770200`) on region `rh`, the one load behind
/// `SetTexture`'s path form, the XML `file=` and every widget's path leg. The file the texture
/// already shows answers 1 untouched (`0x770225`); anything else is judged by
/// [`texture_name_verdict`] and applied by [`apply_texture_name`].
pub(crate) fn load_texture_name(
    model: &mut Model,
    rh: crate::widget::RegionHandle,
    name: &str,
    wrap: bool,
) -> FileLoad {
    let held = model
        .region_data
        .get(&rh)
        .is_some_and(|d| !name.is_empty() && model.texels.holds_file(&d.tex, name));
    if held {
        return FileLoad::Loaded;
    }
    let verdict = texture_name_verdict(model, name);
    apply_texture_name(model, rh, name, wrap, verdict);
    verdict
}

/// What `0x770200` comes to for a name the texture does not already show: an empty name, which is
/// how a null one arrives, loads as no image (`0x770235` → `0x7702cb`, answering 1); any other is
/// the host probe's to judge, and a miss answers 0 (`0x7702ac`). The probe is asked once here, so a
/// path leg that builds its texture only for a load that succeeds asks before it builds.
pub(crate) fn texture_name_verdict(model: &Model, name: &str) -> FileLoad {
    if name.is_empty() {
        return FileLoad::Loaded;
    }
    match &model.texture_probe {
        None => FileLoad::Unprobed,
        Some(probe) if probe(name) => FileLoad::Loaded,
        Some(_) => FileLoad::Failed,
    }
}

/// `0x770200` past its probe on region `rh`. A failed load keeps the image and the shader slot
/// (`0x7702ac`); an empty name drops the image and leaves the slot (`0x7702cb`); a file writes
/// shader 0 to the slot (`0x7702c0`), clearing any desaturation, and then the image. Every caller
/// passes shader index 0 (`0x79bd89`, `0x783165`, `0x7897e5`, `0x78b7d5`, `0x778f3a`).
pub(crate) fn apply_texture_name(
    model: &mut Model,
    rh: crate::widget::RegionHandle,
    name: &str,
    wrap: bool,
    verdict: FileLoad,
) {
    if verdict == FileLoad::Failed {
        return;
    }
    let Model {
        region_data,
        texels,
        ..
    } = &mut *model;
    let data = region_data.entry(rh).or_default();
    let set = if name.is_empty() {
        TexSet::None
    } else {
        data.desaturated = false;
        if wrap {
            TexSet::WrapFile(name.to_string())
        } else {
            TexSet::File(name.to_string())
        }
    };
    // The probe has answered already, so the file is no miss.
    texels.set(&mut data.tex, set, |_| false);
    model.note_art(rh);
}

/// The XML `<Texture file=>` load of a region (`0x76fe20` → `0x770200`), with the wrap flag the
/// element's `<TexCoords>` decide.
pub(crate) fn load_texture_file(
    lua: &Lua,
    this: &Table,
    file: &str,
    wrap: bool,
) -> mlua::Result<FileLoad> {
    let rh = region_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model");
    Ok(load_texture_name(&mut model, rh, file, wrap))
}

/// `Texture:SetTexture` (`0x79bb40`). `lua_isnumber` (`0x79bbd5`, so a numeric string too) picks
/// the colour form (`0x770360`); anything else is a name for `0x770200` with the wrap flag 0
/// (`0x79bda5`): a string as it is, and every other value, nil and no argument included, as a null
/// name (`0x79bdc8`), which clears. It answers 1 unless the load returns 0 (`0x79bdac` →
/// `0x79bddc`); with no probe installed the path form answers nil.
fn set_texture(
    lua: &Lua,
    this: Table,
    arg: Value,
    g: Value,
    b: Value,
    a: Value,
) -> mlua::Result<Value> {
    let rh = region_handle_of(lua, &this)?;
    let colour_form = lua.coerce_number(arg.clone()).ok().flatten().is_some();
    let loaded = {
        let mut model = lua.app_data_mut::<Model>().expect("model");
        // SetTexture drops any portrait mask and live-unit binding.
        let data = model.region_data.entry(rh).or_default();
        data.circular = false;
        data.portrait_unit = None;
        if colour_form {
            // g and b are a bare `lua_tonumber`, and alpha is `lua_isnumber`-gated with default 1.
            // The solid writes the one texture slot (`+0xcc`), so it clears a file; neither form
            // touches the vertex colour (`+0xb8`) or the shader slot, so a tint and a grey outlive
            // the art.
            let fill = crate::script::object::colour_args(lua, &arg, &g, &b, &a);
            model.set_texel(rh, TexSet::Fill(fill));
            model.note_art(rh);
            true
        } else {
            let name = match &arg {
                Value::String(s) => s.to_str()?.to_string(),
                _ => String::new(),
            };
            load_texture_name(&mut model, rh, &name, false) == FileLoad::Loaded
        }
    };
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
