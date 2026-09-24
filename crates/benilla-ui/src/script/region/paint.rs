//! The region paint methods: what a Texture shows and how it is tinted, blended, cropped and
//! layered.

use mlua::{Lua, MultiValue, Table, Value};

use crate::script::object::{as_f32, draw_layer_from_str, draw_layer_name};
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
    // 0 and never raises; stock `QuestLogFrame.lua:337` can pass three nils. A missing alpha is 1
    // here; the reference keeps the region's current alpha (read back at `0x79ac81`, copied over
    // at `0x79adfe`/`0x79ae0a`), so there a three-argument call cannot restore opacity.
    m.set(
        "SetVertexColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<f32>)| {
                let (r, g, b) = (as_f32(&r), as_f32(&g), as_f32(&b));
                let rh = region_handle_of(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let d = model.region_data.entry(rh).or_default();
                d.vertex_color = Some([r, g, b, a.unwrap_or(1.0)]);
                // The slot `SetTextColor` writes on a FontString, so it overrides the font object.
                d.font_explicit.color = true;
                Ok(())
            },
        )?,
    )?;

    // GetVertexColor (`0x79aa50`): white when never set.
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

    // SetGradientAlpha(orientation, r1, g1, b1, a1, r2, g2, b2, a2) and SetGradient without the
    // alphas: a two-stop gradient the client generates into the texture slot (`+0xcc`); here it is
    // stored whole and drawn as its midpoint. Any orientation but "VERTICAL", in any case, is
    // horizontal.
    for (name, with_alpha) in [("SetGradientAlpha", true), ("SetGradient", false)] {
        m.set(
            name,
            lua.create_function(move |lua, args: mlua::MultiValue| {
                let mut it = args.into_iter();
                let this: Table = match it.next() {
                    Some(Value::Table(t)) => t,
                    _ => return Err(mlua::Error::runtime("expected a region")),
                };
                let orientation = match it.next() {
                    Some(Value::String(s)) => s.to_str()?.to_string(),
                    // A missing or non-string orientation is horizontal.
                    _ => String::new(),
                };
                let n = if with_alpha { 8 } else { 6 };
                let mut c = [0.0f32; 8];
                for slot in c.iter_mut().take(n) {
                    *slot = it.next().as_ref().map(as_f32).unwrap_or(0.0);
                }
                let (start, end) = if with_alpha {
                    ([c[0], c[1], c[2], c[3]], [c[4], c[5], c[6], c[7]])
                } else {
                    // SetGradient has no alpha stops: both ends are opaque.
                    ([c[0], c[1], c[2], 1.0], [c[3], c[4], c[5], 1.0])
                };
                let rh = region_handle_of(lua, &this)?;
                let mut model = lua.app_data_mut::<Model>().expect("model");
                let d = model.region_data.entry(rh).or_default();
                d.gradient = Some(crate::script::Gradient {
                    vertical: orientation.eq_ignore_ascii_case("VERTICAL"),
                    start,
                    end,
                });
                Ok(())
            })?,
        )?;
    }

    m.set(
        "SetTexture",
        // The path form reads one argument (`0x770200`), the colour form (`0x770360`) up to four,
        // and extras such as `SetTexture(path, true)` are ignored. `SetTexture` (`0x79bb40`)
        // answers 1, or nil when the path form's file does not load, as the host's
        // `Model::texture_probe` judges; with no probe the path form answers nil.
        lua.create_function(
            |lua, (this, arg, g, b, a): (Table, Value, Value, Value, Value)| {
                let rh = region_handle_of(lua, &this)?;
                // Scoped so the model borrow ends before the layout touch; the path arm drops it
                // mid-way to call the host probe.
                let (loaded, derived) = {
                    let mut model = lua.app_data_mut::<Model>().expect("model");
                    // SetTexture drops any portrait mask and live-unit binding.
                    let data = model.region_data.entry(rh).or_default();
                    data.circular = false;
                    data.portrait_unit = None;
                    // SetTexture clears the desaturation: `0x770200` writes the shader slot
                    // (`+0x128`) from an argument the binding (`0x79bb40`) always passes NULL. The
                    // same path returns first (`0x770225`) and keeps it, nil or "" clear it, and
                    // the colour form (`0x770360`) never writes it.
                    let same_path = matches!((&arg, &data.texture),
                    (Value::String(s), Some(cur)) if s.to_str().is_ok_and(|s| *s == **cur));
                    let colour_form = matches!(&arg, Value::Number(_) | Value::Integer(_));
                    if !same_path && !colour_form {
                        data.desaturated = false;
                    }
                    // An anchored region with an axis sized 0 takes that span from its art, so a
                    // new texture moves it; an anchorless region never resolves, so for it this
                    // is only a paint.
                    let derived = !data.anchors.is_empty()
                        && data.size.is_none_or(|(w, h)| w == 0.0 || h == 0.0);
                    // Both forms write the one texture slot (`+0xcc`), a file or an 8×8 solid, so
                    // each clears the other; neither touches the vertex colour (`+0xb8`), so a
                    // tint outlives its art.
                    let loaded = match &arg {
                        // "" clears, as nil does (stock `QuestLogFrame.lua:166`), and answers nil
                        // like a failed load; the reference's answer for "" is untraced.
                        Value::String(s) if s.to_str()?.is_empty() => {
                            data.texture = None;
                            data.fill = None;
                            false
                        }
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
                            let mut model = lua.app_data_mut::<Model>().expect("model");
                            let had_probe = model.texture_probe.is_some();
                            if resolvable {
                                let data = model.region_data.entry(rh).or_default();
                                data.texture = Some(path);
                                data.fill = None;
                            }
                            resolvable && had_probe
                        }
                        // The colour form, the only one to read the trailing three; a non-number
                        // there takes the default a missing one does.
                        Value::Number(_) | Value::Integer(_) => {
                            let chan = |v: &Value, dflt: f32| match v {
                                Value::Number(_) | Value::Integer(_) => as_f32(v),
                                Value::String(s) => s
                                    .to_str()
                                    .ok()
                                    .and_then(|s| s.parse::<f32>().ok())
                                    .unwrap_or(dflt),
                                _ => dflt,
                            };
                            data.fill =
                                Some([as_f32(&arg), chan(&g, 0.0), chan(&b, 0.0), chan(&a, 1.0)]);
                            data.texture = None;
                            true
                        }
                        // nil clears, so the region draws nothing, and answers 1 (`0x79bb40`).
                        Value::Nil => {
                            data.texture = None;
                            data.fill = None;
                            true
                        }
                        _ => false,
                    };
                    (loaded, derived)
                };
                // Only the art-sized shape can move; touching the layout on every repaint would
                // reopen the resolve's change gate each frame.
                if derived {
                    lua.app_data_mut::<Model>()
                        .expect("model")
                        .touch_layout_region(rh);
                }
                Ok(if loaded {
                    Value::Number(1.0)
                } else {
                    Value::Nil
                })
            },
        )?,
    )?;

    // SetDesaturated(flag) (`0x79c1e0`), Texture only, answers shaderSupported, which stock
    // `ItemButtonTemplate.lua:69` reads to fall back to a grey tint; the renderer greys the texel,
    // so this answers 1. The flag is `GetBoolOrDefault` (`0x6f1c10`, default 1, jump table
    // `0x6f1ce8`): no argument is on, hence the `MultiValue`, nil is off, a number truncating to 0
    // is off (`0x6f3620`, `0x40a2b0`), and a table, function or userdata is on. Every string is on
    // here; the reference's string arm (`0x6f1c51`, against `0x871460` and `0x853758`) is not.
    m.set(
        "SetDesaturated",
        lua.create_function(|lua, args: MultiValue| {
            let mut args = args.into_iter();
            let this: Table = match args.next() {
                Some(Value::Table(t)) => t,
                _ => return Ok(Value::Nil),
            };
            let on = match args.next() {
                None => true,
                Some(Value::Nil) => false,
                Some(Value::Boolean(b)) => b,
                Some(Value::Integer(i)) => i != 0,
                Some(Value::Number(n)) => n.trunc() != 0.0,
                Some(_) => true,
            };
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
            if data.fill.is_some() {
                return Ok(Some("Solid Texture".to_string()));
            }
            Ok(data.texture.as_ref().map(|t| match t.rfind('.') {
                Some(i) => t[..i].to_string(),
                None => t.clone(),
            }))
        })?,
    )?;

    // SetAlphaGradient(start, length), a FontString's write-on reveal: answers whether `start` is
    // still inside the text in chars, and stock `QuestFrame.lua:558` advances until it is not.
    m.set(
        "SetAlphaGradient",
        lua.create_function(|lua, (this, start, length): (Table, f32, f32)| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let data = model.region_data.entry(rh).or_default();
            data.alpha_gradient = Some((start, length));
            let chars = data.text.as_deref().map_or(0, |t| t.chars().count());
            Ok(start < chars as f32)
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
    // the setter is the only writer of `[texture+0x124]`, the getter answers 1 or nil. Stored, not
    // applied: there a set flag makes `SetTexCoord` re-derive the rect from the UV quad
    // (`0x770462`).
    m.set(
        "SetTexCoordModifiesRect",
        lua.create_function(|lua, (this, arg): (Table, Value)| {
            let rh = region_handle_of(lua, &this)?;
            // `GetBoolOrDefault` (`0x6f1c10`) with default 1 (`0x79c105`), but a missing argument
            // arrives here as nil and reads false, where the reference sets the flag.
            let on = crate::script::binding_abi::bool_or_default(Some(&arg), false);
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

    m.set(
        "SetDrawLayer",
        lua.create_function(|lua, (this, layer, sub): (Table, String, Option<i64>)| {
            let rh = region_handle_of(lua, &this)?;
            let dl = draw_layer_from_str(&layer)
                .ok_or_else(|| mlua::Error::runtime(format!("unknown draw layer '{layer}'")))?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            if let Some(region) = model.arena.region_mut(rh) {
                region.draw_layer = dl;
                if let Some(s) = sub {
                    region.sub_level = s.clamp(i64::from(i8::MIN), i64::from(i8::MAX)) as i8;
                }
            }
            Ok(())
        })?,
    )?;

    // GetDrawLayer (Texture `0x79a6c0`, FontString `0x79c660`): the layer name alone, since
    // 1.12's pair is layer-only; the sub-level `SetDrawLayer` accepts is not 1.12's.
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
            let rest: Vec<f32> = args.iter().map(as_f32).collect();
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
    pub fn set_texture_probe(&mut self, probe: crate::script::TextureProbe) {
        self.model_mut().texture_probe = Some(probe);
    }

    /// Install the host's font-path check behind `SetFont`'s 1-or-nil answer; without one every
    /// non-empty path answers 1.
    pub fn set_font_probe(&mut self, probe: crate::script::FontProbe) {
        self.model_mut().font_probe = Some(probe);
    }

    /// Install the host's texel-size check, which lets an axis sized 0 take its span from the art
    /// as `CSimpleTexture::GetWidth` (`0x770720`) falls back to the texels; without one such a
    /// region stays where it was.
    pub fn set_texture_size_probe(&mut self, probe: crate::script::TextureSizeProbe) {
        self.model_mut().texture_size_probe = Some(probe);
    }
}
