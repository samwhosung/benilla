//! The model-pane widgets' Lua methods, `Model` and `PlayerModel`. The engine holds the scene the
//! Lua API reads and writes ([`ModelState`]); the app draws it.
//!
//! Each 1.12 model-pane class has its own method table that never repeats its base's entries; a
//! derived pane reaches its base's methods on a miss (`__index` meta `0x7020b0`), never the
//! reverse, so a plain `<Model>` has no `SetUnit`:
//!
//! ```text
//! CSimpleFrame 0x778590
//! └─ CSimpleModel          0x76f870   table 0x878948 (23)   <Model>          ← here
//!    └─ CGCharacterModelBase 0x506260  table 0x84f1fc (3)   <PlayerModel>    ← here
//!       ├─ DressUpModelFrame 0x5050d0  table 0x84f190 (3)   <DressUpModel>   ← `dressup`
//!       └─ TabardModel       0x503bd0  table 0x84ee40 (10)  <TabardModel>    ← `tabard`
//! ```
//!
//! The pane's clock is the engine's, since Lua sees it through `OnUpdateModel`, fired at the top of
//! every paint (`0x76d1bc`), and `OnAnimFinished` (`0x76cdc0`). What only the file knows arrives
//! from the host once the asset is resident ([`crate::script::UiScript::set_model_facts`]).

use std::sync::Arc;

use mlua::{Lua, MultiValue, Table, Value};

use super::object::frame_handle_of;
use super::Model;
use crate::widget::{model_key, FrameHandle, KindState, ModelFileFacts, ModelLight, ModelState};

impl Model {
    /// FrameXML units per layout unit, `768 · √(a²+1)` for the screen aspect `a` (`[0x832a48]`
    /// holds `1/√(a²+1)`): the unit of `SetPosition` and of the implicit rect. `a` is 4/3 before a
    /// screen exists.
    pub(crate) fn layout_unit(&self) -> f32 {
        let (w, h) = (self.screen.width(), self.screen.height());
        let a = if h > 0.0 && w > 0.0 { w / h } else { 4.0 / 3.0 };
        768.0 * (a * a + 1.0).sqrt()
    }

    /// The implicit rect: a pane with no authored size takes its file's bounding-box extent, in
    /// layout units, as the reference's geometry overrides answer it (`0x76d080`, `0x76d0d0`).
    pub(crate) fn apply_implicit_rect(&mut self, h: FrameHandle) {
        let unit = self.layout_unit();
        let extent = {
            let Some(f) = self.arena.frame(h) else { return };
            let KindState::Model(m) = &f.kind_state else {
                return;
            };
            let Some(path) = m.path.as_deref() else {
                return;
            };
            let Some(facts) = self.model_facts.get(&model_key(path)) else {
                return;
            };
            let authored = !m.implicit_size
                && self
                    .layout_inputs
                    .get(&h)
                    .is_some_and(|i| i.width != 0.0 || i.height != 0.0);
            if authored {
                return;
            }
            facts.extent()
        };
        let (w, ht) = (extent.0 * unit, extent.1 * unit);
        let input = self.layout_inputs.entry(h).or_default();
        let changed =
            input.width.to_bits() != w.to_bits() || input.height.to_bits() != ht.to_bits();
        input.width = w;
        input.height = ht;
        if changed {
            self.touch_layout_frame(h);
        }
        if let Some(KindState::Model(m)) = self.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            m.implicit_size = true;
        }
    }

    /// A size was authored on `h` (`SetWidth`, `SetHeight`, or XML `<Size>` through them): the
    /// implicit rect yields to it for good, as the reference's override does.
    pub(crate) fn note_authored_size(&mut self, h: FrameHandle) {
        if let Some(KindState::Model(m)) = self.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            m.implicit_size = false;
        }
    }

    /// The host's facts for `path`; on a miss the path is queued, once, for the host to load.
    pub(crate) fn model_facts_for(&mut self, path: &str) -> Option<Arc<ModelFileFacts>> {
        let key = model_key(path);
        match self.model_facts.get(&key) {
            Some(facts) => Some(facts.clone()),
            None => {
                if !self.model_facts_wanted.contains(&key) {
                    self.model_facts_wanted.push(key);
                }
                None
            }
        }
    }
}

/// The facts of the file `this` pane holds, if it holds one the host has loaded.
fn facts_of_pane(lua: &Lua, this: &Table) -> mlua::Result<Option<Arc<ModelFileFacts>>> {
    let path = with_model(lua, this, |m| m.path.clone())?;
    Ok(path.and_then(|p| {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .model_facts_for(&p)
    }))
}

/// Registry key of the `Model` method table (the MAXCSTACK discipline: a Lua-side root).
pub(super) const REG_MODEL_METHODS: &str = "__benilla_model_methods";

/// Registry key of the `PlayerModel` table, its own three methods only: `Model`'s 23 are reached
/// on a miss, as `0x506260` falls through to `0x76f870`.
pub(super) const REG_PLAYERMODEL_METHODS: &str = "__benilla_playermodel_methods";

/// Run `f` on a frame's Model state under one short write borrow. Errors on a receiver that is not
/// a live Model: a method taken off the table can be called on anything.
pub(super) fn with_model<T>(
    lua: &Lua,
    this: &Table,
    f: impl FnOnce(&mut ModelState) -> T,
) -> mlua::Result<T> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let frame = model
        .arena
        .frame_mut(h)
        .ok_or_else(|| mlua::Error::runtime("stale frame handle"))?;
    match &mut frame.kind_state {
        KindState::Model(m) => Ok(f(m)),
        _ => Err(mlua::Error::runtime("not a Model")),
    }
}

/// `lua_tonumber`, which the reference's coordinate and angle setters read through: a number or a
/// numeric string, anything else 0.0.
fn num(v: &Value) -> f32 {
    match v {
        Value::Number(n) => *n as f32,
        Value::Integer(i) => *i as f32,
        Value::String(s) => s.to_str().ok().and_then(|t| t.parse().ok()).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Same, as an integer (sequence and camera indices).
fn int(v: &Value) -> i32 {
    num(v) as i32
}

/// `lua_isnumber`: a number or a string that converts. Separate from [`num`] because `SetLight`
/// raises on a non-number where the coordinate setters read 0.
fn number(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => Some(*n as f32),
        Value::Integer(i) => Some(*i as f32),
        Value::String(s) => s.to_str().ok().and_then(|t| t.trim().parse().ok()),
        _ => None,
    }
}

/// `[0x8029d4]`, the client's float-zero epsilon: `SetLight`'s intensity gate, `0x71b6a0`'s
/// direction normalise and the paint's degenerate-rect test.
const FLOAT_EPS: f32 = 2.384_185_8e-7;

/// `Model:SetLight`'s usage error. The reference's (`0x878c00`) names the receiver through `%s`
/// and ends in `)`; this one names `Model` and drops the `)`.
const SET_LIGHT_USAGE: &str = "Usage: Model:SetLight(enabled[, omni, dirX, dirY, dirZ, \
     ambIntensity[, ambR, ambG, ambB], dirIntensity[, dirR, dirG, dirB]]";

/// What [`parse_set_light`] decided.
enum SetLight {
    /// A non-number where one is required: `luaL_error(SET_LIGHT_USAGE)`.
    Raise,
    /// `enabled == 0` returns (`0x76e2cb`) before the copy, so the call neither disables nor
    /// edits the light.
    NoOp,
    /// The light, copied whole over the widget's (`0x76cf30` at `0x76e777`).
    Set(ModelLight),
}

/// `Model:SetLight`'s argument walk (`0x76e1e0`) over the arguments after `self` (`a[0]` is Lua
/// index 2), into a local light whose colours start black (`0x71b4a0`), not the widget's white.
/// A colour triple is read only when its intensity is nonzero and all three are numbers; otherwise
/// it is white and the cursor does not advance, so `SetLight(1, 0, x, y, z, ambI, dirI)` reads the
/// second intensity at index 8, not 11.
fn parse_set_light(a: &[Value]) -> SetLight {
    let Some(enabled) = a.first().and_then(number) else {
        return SetLight::Raise;
    };
    if enabled as i32 == 0 {
        return SetLight::NoOp;
    }
    // Lua indices 3..=7 (omni, x, y, z, ambient intensity) are required once enabled.
    let mut head = [0.0f32; 5];
    for (i, slot) in head.iter_mut().enumerate() {
        match a.get(i + 1).and_then(number) {
            Some(v) => *slot = v,
            None => return SetLight::Raise,
        }
    }
    let omni = head[0] as i32 != 0;
    let mut vector = [head[1], head[2], head[3]];
    if !omni {
        // A direction is normalised on write (`0x71b6a0`); a position is stored raw (`0x71b650`).
        let len = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
        if len > FLOAT_EPS {
            vector = vector.map(|v| v / len);
        }
    }
    // Each colour is packed to 8 bits a channel and back (`0x76f900`), so a round trip is lossy.
    let quantize = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round()) / 255.0;
    let block = |at: usize| -> Option<[f32; 3]> {
        let c: Vec<f32> = (at..at + 3)
            .filter_map(|k| a.get(k).and_then(number))
            .collect();
        (c.len() == 3).then(|| [quantize(c[0]), quantize(c[1]), quantize(c[2])])
    };
    let ambient_i = head[4];
    let (ambient_rgb, next) = match block(6).filter(|_| ambient_i.abs() > FLOAT_EPS) {
        Some(rgb) => (rgb, 9), // the cursor advanced: Lua index 11
        None => ([1.0; 3], 6), // white, and the cursor stayed: Lua index 8
    };
    let mut light = ModelLight {
        enabled: true,
        omni,
        vector,
        ambient: ambient_rgb.map(|c| c * ambient_i),
        diffuse: [0.0; 3],
    };
    if let Some(diffuse_i) = a.get(next).and_then(number) {
        let rgb = block(next + 1)
            .filter(|_| diffuse_i.abs() > FLOAT_EPS)
            .unwrap_or([1.0; 3]);
        light.diffuse = rgb.map(|c| c * diffuse_i);
    }
    SetLight::Set(light)
}

/// A string argument as `lua_isstring` gates it: a string or a number (tags 3 and 4), the number
/// rendered as decimal text.
fn string_arg(v: &Value) -> Option<std::borrow::Cow<'_, str>> {
    match v {
        Value::String(s) => s
            .to_str()
            .ok()
            .map(|t| std::borrow::Cow::Owned(t.to_string())),
        Value::Number(n) => Some(std::borrow::Cow::Owned(n.to_string())),
        Value::Integer(i) => Some(std::borrow::Cow::Owned(i.to_string())),
        _ => None,
    }
}

/// The client's `Usage:` error, naming the receiver by `GetName()` or `<unnamed>` (`0x84c7f0`).
fn usage(lua: &Lua, this: &Table, call: &str) -> mlua::Error {
    let name = this
        .get::<mlua::Function>("GetName")
        .and_then(|f| f.call::<Option<String>>(this.clone()))
        .ok()
        .flatten()
        .unwrap_or_else(|| "<unnamed>".to_string());
    let _ = lua;
    mlua::Error::runtime(format!("Usage: {name}:{call}"))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // ── Content: the model, and clearing ────────────────────────────────────────────────────
    //
    // `SetModel` and `PlayerModel:SetUnit` are alternative contents: setting one clears the other.
    // `SetModel` raises `Usage: %s:SetModel("file")` on anything `lua_isstring` refuses, nil
    // included (`0x76d950`); `ClearModel` is the clear.
    //
    // Deviation: a path that does not resolve is stored, where the reference raises
    // `Invalid model file: %s` (`0x878b44`), because the file loads asynchronously here and the
    // call cannot know yet.
    m.set(
        "SetModel",
        lua.create_function(|lua, (this, path): (Table, Value)| {
            let path = string_arg(&path)
                .ok_or_else(|| usage(lua, &this, "SetModel(\"file\")"))?
                .to_string();
            // The reference's asset-resident test (`0x71d5a3`): the loader's Stand seed runs now,
            // or when the file lands.
            let facts = lua
                .app_data_mut::<Model>()
                .expect("model app_data")
                .model_facts_for(&path);
            with_model(lua, &this, |m| m.set_file(path, facts.as_deref()))?;
            // A size-less pane takes the file's rect as soon as the file is known.
            let h = frame_handle_of(lua, &this)?;
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .apply_implicit_rect(h);
            Ok(())
        })?,
    )?;
    m.set(
        "GetModel",
        lua.create_function(|lua, this: Table| with_model(lua, &this, |m| m.path.clone()))?,
    )?;
    m.set(
        "ClearModel",
        lua.create_function(|lua, this: Table| with_model(lua, &this, ModelState::clear_file))?,
    )?;
    // ── Animation ───────────────────────────────────────────────────────────
    //
    // Both verbs are one arm, `0x7121a0(model, -1, id, 0, ms, 1.0f, 0, 1)`: `SetSequence` with
    // `ms = 0` (`0x76dec0` → `0x76cf50`), `SetSequenceTime` with the caller's (`0x76dfc0` →
    // `0x76cf80`). It anchors the play head at `clock − trunc(ms)`, re-read every frame
    // (`0x71273d`). The id is an `AnimationData` id, not a file slot.
    m.set(
        "SetSequence",
        lua.create_function(|lua, (this, seq): (Table, Value)| {
            let seq = int(&seq);
            let facts = facts_of_pane(lua, &this)?;
            with_model(lua, &this, |m| m.arm(seq, 0, facts.as_deref()))
        })?,
    )?;
    m.set(
        "SetSequenceTime",
        lua.create_function(|lua, (this, seq, ms): (Table, Value, Value)| {
            let (seq, ms) = (int(&seq), int(&ms));
            let facts = facts_of_pane(lua, &this)?;
            with_model(lua, &this, |m| m.arm(seq, i64::from(ms), facts.as_deref()))
        })?,
    )?;

    // ── The pane's view: yaw, scale, camera, position ───────────────────────────────────────
    //
    // `SetFacing` (`0x878948[4]` → `0x76dce0`) writes the yaw at `+0x39c`, the field
    // `PlayerModel:SetRotation` also writes.
    m.set(
        "SetFacing",
        lua.create_function(|lua, (this, rad): (Table, Value)| {
            let rad = num(&rad);
            with_model(lua, &this, |m| m.facing = rad)
        })?,
    )?;
    m.set(
        "GetFacing",
        lua.create_function(|lua, this: Table| with_model(lua, &this, |m| m.facing))?,
    )?;
    m.set(
        "SetModelScale",
        lua.create_function(|lua, (this, s): (Table, Value)| {
            let s = num(&s);
            with_model(lua, &this, |m| m.scale = s)
        })?,
    )?;
    m.set(
        "GetModelScale",
        lua.create_function(|lua, this: Table| with_model(lua, &this, |m| m.scale))?,
    )?;
    // `SetCamera(n)` (`0x76e0e0` → `0x76cec0`) picks a camera by raw table index, not through
    // `cameraLookup`; past the table it installs the NULL (orthographic) camera, and until the
    // facts arrive the pane draws nothing.
    m.set(
        "SetCamera",
        lua.create_function(|lua, (this, c): (Table, Value)| {
            let c = int(&c);
            let facts = facts_of_pane(lua, &this)?;
            with_model(lua, &this, |m| m.install_camera(c, facts.as_deref()))
        })?,
    )?;
    m.set(
        "SetPosition",
        lua.create_function(|lua, (this, x, y, z): (Table, Value, Value, Value)| {
            let p = (num(&x), num(&y), num(&z));
            with_model(lua, &this, |m| m.position = p)
        })?,
    )?;
    m.set(
        "GetPosition",
        lua.create_function(|lua, this: Table| {
            let (x, y, z) = with_model(lua, &this, |m| m.position)?;
            Ok((x, y, z))
        })?,
    )?;

    // ── The scene: light and fog ────────────────────────────────────────────────────────────
    m.set(
        "SetLight",
        lua.create_function(|lua, args: MultiValue| {
            let mut it = args.into_iter();
            let this = match it.next() {
                Some(Value::Table(t)) => t,
                _ => return Err(mlua::Error::runtime("expected a Model")),
            };
            let a: Vec<Value> = it.collect();
            match parse_set_light(&a) {
                SetLight::Raise => Err(mlua::Error::runtime(SET_LIGHT_USAGE)),
                SetLight::NoOp => with_model(lua, &this, |_| ()),
                SetLight::Set(l) => with_model(lua, &this, |m| m.light = l),
            }
        })?,
    )?;
    m.set(
        "GetLight",
        lua.create_function(|lua, this: Table| {
            let l = with_model(lua, &this, |m| m.light)?;
            let mut out: Vec<Value> = vec![
                Value::Number(f64::from(u8::from(l.enabled))),
                Value::Number(f64::from(u8::from(l.omni))),
                Value::Number(f64::from(l.vector[0])),
                Value::Number(f64::from(l.vector[1])),
                Value::Number(f64::from(l.vector[2])),
            ];
            // A colour comes back as `1.0, r, g, b` if any component is above 0, else as a lone 0
            // (`0x76e953`), so the arity is 7, 10 or 13.
            for c in [l.ambient, l.diffuse] {
                if c.iter().any(|&v| v > 0.0) {
                    out.push(Value::Number(1.0));
                    out.extend(c.iter().map(|&v| Value::Number(f64::from(v))));
                } else {
                    out.push(Value::Number(0.0));
                }
            }
            Ok(MultiValue::from_vec(out))
        })?,
    )?;
    m.set(
        "SetFogColor",
        lua.create_function(
            |lua, (this, r, g, b, a): (Table, Value, Value, Value, Option<Value>)| {
                // The fifth argument is alpha, clamped like the rest but 1.0 when absent, where an
                // absent r, g or b reads 0.0: `SetFogColor(r, g, b)` is opaque.
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
                let a = a.as_ref().map_or(1.0, num);
                let packed = (q(a) << 24) | (q(num(&r)) << 16) | (q(num(&g)) << 8) | q(num(&b));
                // Setting the colour arms the fog (`0x76f059`), the only verb that does.
                with_model(lua, &this, |m| {
                    m.fog_color = packed;
                    m.fog = true;
                })
            },
        )?,
    )?;
    m.set(
        "GetFogColor",
        lua.create_function(|lua, this: Table| {
            // Always four values; never set reads `1, 1, 1, 1`, the ctor's `0xffffffff`.
            let packed = with_model(lua, &this, |m| m.fog_color)?;
            let ch = |shift: u32| f64::from((packed >> shift) & 0xff) / 255.0;
            Ok((ch(16), ch(8), ch(0), ch(24)))
        })?,
    )?;

    // `SetFogNear` (`0x76f1e0`) and `SetFogFar` (`0x76f390`) store raw, with no clamp or order
    // check (the `≥ 0` clamp is XML-only), and do not arm the fog.
    m.set(
        "SetFogNear",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let v = num(&v);
            with_model(lua, &this, |m| m.fog_near = v)
        })?,
    )?;
    m.set(
        "GetFogNear",
        lua.create_function(|lua, this: Table| with_model(lua, &this, |m| m.fog_near))?,
    )?;
    m.set(
        "SetFogFar",
        lua.create_function(|lua, (this, v): (Table, Value)| {
            let v = num(&v);
            with_model(lua, &this, |m| m.fog_far = v)
        })?,
    )?;
    m.set(
        "GetFogFar",
        lua.create_function(|lua, this: Table| with_model(lua, &this, |m| m.fog_far))?,
    )?;
    // `ClearFog` (`0x76f540`) clears the armed bit alone (`0x76f5c5`): colour, near and far stay.
    m.set(
        "ClearFog",
        lua.create_function(|lua, this: Table| with_model(lua, &this, |m| m.fog = false))?,
    )?;

    // ── AdvanceTime and ReplaceIconTexture ─────────────────────────────────────────────────

    // `AdvanceTime()` (`0x878948[14]` → `0x76eca0`) does nothing: it reads no argument, returns
    // nothing, writes no field, and its one call, `0x76cfb0`, is `mov eax,1; ret`.
    m.set(
        "AdvanceTime",
        lua.create_function(|lua, this: Table| with_model(lua, &this, |_| ()))?,
    )?;

    // `ReplaceIconTexture(path)` (`0x878948[15]` → `0x76ed70`): `0x76cfe0(0xe, path)` → `0x710ec0`
    // retextures every type-14 texture of the model instance and of its ribbon (`0x7b7950`) and
    // particle (`0x7b4d20`) emitters, so `SetModel` and `ClearModel` drop it with the instance.
    // With no instance (`[widget+0x318] == 0`) the call is dropped; with one not yet resident it is
    // replayed on load, so the pane stores the override for the host to apply.
    m.set(
        "ReplaceIconTexture",
        lua.create_function(|lua, (this, path): (Table, Value)| {
            let Some(path) = string_arg(&path) else {
                return Err(usage(lua, &this, "ReplaceIconTexture(\"texture\")"));
            };
            let path = path.to_string();
            with_model(lua, &this, |m| {
                if m.path.is_some() {
                    m.icon = Some(path);
                }
            })
        })?,
    )?;

    lua.set_named_registry_value(REG_MODEL_METHODS, m)?;
    playermodel_install(lua)
}

/// `PlayerModel`'s own three methods (table `0x84f1fc`); the rest are `Model`'s, reached on a miss.
fn playermodel_install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // `SetUnit(unit)` (`0x84f1fc[0]` → `0x505d70`) clears the `SetModel` path; the unit token is
    // resolved at render.
    m.set(
        "SetUnit",
        lua.create_function(|lua, (this, unit): (Table, Value)| {
            let unit = match &unit {
                Value::String(s) => Some(s.to_str()?.to_string()),
                _ => None,
            };
            with_model(lua, &this, |m| {
                m.unit = unit;
                m.path = None;
            })?;
            // A dress-up pane rebuilds from the unit's live model, dropping try-ons (`0x5059a0`).
            super::dressup::redress_if_dressup(lua, &this)
        })?,
    )?;

    // `RefreshUnit()` (`0x84f1fc[1]` → `0x505e40`) re-reads the pane's unit; the token already
    // resolves at render, so only a dress-up pane has work to do.
    m.set(
        "RefreshUnit",
        lua.create_function(|lua, this: Table| {
            with_model(lua, &this, |_| ())?;
            // The same worker as `SetUnit`'s (`0x505b50`).
            super::dressup::redress_if_dressup(lua, &this)
        })?,
    )?;

    // `SetRotation(rad)` (`0x84f1fc[2]` → `0x505f00` → `0x505bb0`) writes the yaw `SetFacing`
    // writes (`0x505c44`). Not built: the reference also plays a turn animation, ShuffleRight
    // (`0xc`) when the facing grows, ShuffleLeft (`0xb`) when it shrinks, else Stand (0), unless
    // already armed on bone slot 0, and holds the turn 100 ms (`[+0x3e8]`, `[+0x3ec]`, expired by
    // `0x505c50`).
    m.set(
        "SetRotation",
        lua.create_function(|lua, (this, rad): (Table, Value)| {
            let rad = num(&rad);
            with_model(lua, &this, |m| m.facing = rad)
        })?,
    )?;

    lua.set_named_registry_value(REG_PLAYERMODEL_METHODS, m)
}

impl crate::script::UiScript {
    /// The scene state of the model pane named `name`, for the host that draws it, which knows the
    /// pane by its stock name (`CharacterModelFrame`).
    pub fn model_pane(&self, name: &str) -> Option<ModelState> {
        self.model_ref()
            .arena
            .iter_frames()
            .find_map(|(_, f)| match (&f.name, &f.kind_state) {
                (Some(n), KindState::Model(m)) if n == name => Some(m.clone()),
                _ => None,
            })
    }

    /// A named pane's yaw in radians, 0.0 before the pane exists; the stock `Model_OnLoad` sets
    /// 0.61 (`UIParent.lua:1422`).
    pub fn model_pane_facing(&self, name: &str) -> f32 {
        self.model_pane(name).map_or(0.0, |m| m.facing)
    }

    /// The host's facts for a model file, read off the loaded asset: every pane holding it runs the
    /// reference's load completion ([`ModelState::seed_from_facts`]) and takes its implicit rect.
    pub fn set_model_facts(&mut self, path: &str, facts: ModelFileFacts) {
        let key = model_key(path);
        let facts = Arc::new(facts);
        let mut model = self.model_mut();
        model.model_facts.insert(key.clone(), facts.clone());
        model.model_facts_wanted.retain(|k| *k != key);
        // The panes holding this file: the waiters the reference links on its streaming drain
        // (`0x71d640`).
        let handles: Vec<_> = model
            .arena
            .iter_frames()
            .filter_map(|(h, f)| match &f.kind_state {
                KindState::Model(m) if m.path.as_deref().is_some_and(|p| model_key(p) == key) => {
                    Some(h)
                }
                _ => None,
            })
            .collect();
        for h in handles {
            if let Some(KindState::Model(m)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
                m.seed_from_facts(&facts);
            }
            model.apply_implicit_rect(h);
        }
    }

    /// The screen aspect changed: re-derive every implicit rect, whose unit scales with `√(a²+1)`.
    pub(crate) fn reapply_implicit_rects(&mut self) {
        let mut model = self.model_mut();
        let panes: Vec<FrameHandle> = model
            .arena
            .ticked_kinds()
            .iter()
            .copied()
            .filter(|&h| {
                model.arena.frame(h).is_some_and(
                    |f| matches!(&f.kind_state, KindState::Model(m) if m.implicit_size),
                )
            })
            .collect();
        for h in panes {
            model.apply_implicit_rect(h);
        }
    }

    /// Drain the model files panes named with no facts yet, as [`crate::widget::model_key`]s; the
    /// host loads each once and answers through [`Self::set_model_facts`].
    pub fn model_facts_wanted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().model_facts_wanted)
    }

    /// Whether the engine holds facts for `path`'s file.
    pub fn has_model_facts(&self, path: &str) -> bool {
        self.model_ref().model_facts.contains_key(&model_key(path))
    }

    /// The paint list: every visible model pane whose file's facts are known, with its clock and
    /// play head, in the arena's registry order, which is stable across frames.
    pub fn visible_model_panes(&self) -> Vec<ModelPaneFrame> {
        let model = self.model_ref();
        model
            .arena
            .ticked_kinds()
            .iter()
            .filter_map(|&h| {
                let f = model.arena.frame(h)?;
                if !f.effective_visible {
                    return None;
                }
                let KindState::Model(m) = &f.kind_state else {
                    return None;
                };
                let path = m.path.as_deref()?;
                let facts = model.model_facts.get(&model_key(path))?;
                // A pane whose camera is still pending paints nothing, not even its
                // `OnUpdateModel` (`0x76d5f0`).
                if m.camera_pending.is_some() {
                    return None;
                }
                Some(ModelPaneFrame {
                    handle: h,
                    clock_ms: m.clock_ms,
                    play: m.play_head(facts),
                })
            })
            .collect()
    }
}

/// One row of [`UiScript::visible_model_panes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelPaneFrame {
    pub handle: crate::widget::FrameHandle,
    /// The pane's scene clock in ms, which also drives the file's global sequences.
    pub clock_ms: u64,
    /// The armed sequence's play head; `None` draws the rest pose.
    pub play: Option<crate::widget::ModelPlayHead>,
}
