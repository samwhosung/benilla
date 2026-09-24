//! The model-pane family's method surfaces — `Model` and `PlayerModel`, the 3D panes an addon or a
//! FrameXML frame parks a model in.
//!
//! **The same split as [`super::minimap`] and [`super::cooldown`], and for the same reason.** The
//! engine core holds exactly the scene the Lua API reads and writes ([`ModelState`]); the pixels
//! are the app renderer's job. That is the posture the `<Minimap>` and cooldown widgets already
//! run under — a sized hole the game layer draws into.
//!
//! ## Two tables, chained — not one table with everything in it
//!
//! 1.12 registers **four** model-pane types, and each has its own Lua method table that **never
//! repeats its base's entries**; a derived pane reaches its base's verbs through the miss leg of
//! `vtable+0x8` (`__index` meta `0x7020b0`):
//!
//! ```text
//! CSimpleFrame 0x778590
//! └─ CSimpleModel          0x76f870   table 0x878948 (23)   <Model>          ← built here
//!    └─ CGCharacterModelBase 0x506260  table 0x84f1fc (3)   <PlayerModel>    ← built here
//!       ├─ DressUpModelFrame 0x5050d0  table 0x84f190 (3)   <DressUpModel>   ← `dressup` (1969)
//!       └─ TabardModel       0x503bd0  table 0x84ee40 (10)  <TabardModel>    ← `tabard` (1977)
//! ```
//!
//! Our `__index` dispatcher already walks a *slice* of registry keys per kind, so the chain is
//! `&[REG_PLAYERMODEL_METHODS, REG_MODEL_METHODS]` and neither table duplicates the other. The
//! direction is derived → base **only**: a plain `<Model>` does not acquire `SetUnit`.
//!
//! `DressUpModel` (`Undress`/`Dress`/`TryOn`) is `super::dressup` (1969) and `TabardModel` (the ten
//! tabard verbs) `super::tabard` (1977) — each built the day its stock window came onto the chain,
//! as 1134 §4 said they would be.
//!
//! ## Why this surface, in this order
//!
//! It is the wall four of the twenty most-installed 1.12 addons hit at once. pfUI's action-bar
//! module builds the pet bar's autocast shine as
//!
//! ```lua
//! f.autocast = CreateFrame("Model", nil, f)
//! f.autocast:SetModel("Interface\\Buttons\\UI-AutoCastButton.mdx")
//! f.autocast:SetSequence(0)
//! ```
//!
//! and its unit frames build portraits as `CreateFrame("PlayerModel", …)` driven by `SetUnit` +
//! `SetCamera`. pfUI is embedded in pfQuest, pfQuest-turtle and ShaguDPS as well, so one missing
//! type stopped all four dead, each of them *after* the whole rest of the UI had built.
//!
//! ## Ownership is read off the registrar, never off a string scan
//!
//! This module published `SetUnit`, `RefreshUnit` and `SetRotation` on `Model` until 2026-08-30,
//! on the strength of an isolated-string scan of `WoW.exe` finding one occurrence of each. **A
//! `strings` hit cannot settle ownership** — it answers *whether a name exists*, and `SetUnit`'s
//! single pooled string `0x84f22c` is referenced by **two** method-table entries in two different
//! tables (`PlayerModel 0x84f1fc[0]` and `GameTooltip 0x854290`). All three are `PlayerModel`'s.
//!
//! Ownership is settled by the enumeration recipe, not a `strings` hit: census `(call|jmp)
//! 0x701d80`, read the count from the registering `mov edx`, read the pairs at `base + 8*i`, and
//! settle "which table owns method M" by counting image-wide dword references to M's name VA.
//!
//! ## The clock is the engine's; the pixels are the host's
//!
//! A pane's animation state is not "a sequence index the host interprets": the reference widget
//! owns a private `CM2Scene` whose clock its own `OnUpdate` advances, arms sequences through
//! `0x7121a0` with an anchor the sampler re-reads, fires `OnUpdateModel` at the top of every
//! paint (`0x76d1bc`) and `OnAnimFinished` from the completion callback (`0x76cdc0`). All of that
//! is Lua-observable — the shipped cooldown is nothing but those two handlers scrubbing
//! `SetSequenceTime` — so it lives here:
//! [`ModelState::clock_ms`] / [`ModelState::armed`], the tick's model pass, and the bindings
//! below. What the engine does NOT have is the file: which ids it owns, how long each sequence
//! runs, whether it loops. Those are the file's **facts** ([`crate::widget::ModelFileFacts`]),
//! handed over by the host once the asset is resident ([`crate::script::UiScript::set_model_facts`])
//! — the reference's own "asset ready" edge, on which the loader's Stand seed runs.
//!
//! ## What is deliberately NOT here
//!
//! Nothing of `Model`'s own 23 — the fog near/far/clear set was the last hold-out and 2027 closed
//! it (`ClearFog 0x76f540` is `76f5c5 and [edi+0x3a4],-2`, bit 0 and nothing else, so the guess
//! that kept it out is gone; see the install below).
//!
//! Absent, and correctly so: `SetCreature` and `SetCustomRace`. Neither string exists in
//! 5875 in any form (substring scan of the whole mapped image returns 0, against a positive
//! control of 27 hits for `Creature`) — they are later-expansion names, and publishing one would
//! be decision 1189's error.
//!
//! And any interpretation of `SetLight`'s numbers — the engine core has no lighting model, so the
//! tuple is stored verbatim rather than typed into a scene semantics nobody has verified.

use std::sync::Arc;

use mlua::{Lua, MultiValue, Table, Value};

use super::object::frame_handle_of;
use super::Model;
use crate::widget::{model_key, FrameHandle, KindState, ModelFileFacts, ModelLight, ModelState};

impl Model {
    /// FrameXML units per **layout unit** — `768 · √(a²+1)` for the screen's aspect `a`
    /// (`G48` [`0x832a48`] = `1/√(a²+1)`, the root scale against the `768`-tall FrameXML
    /// space). `SetPosition`'s space, and the unit a size-less pane's implicit rect is measured
    /// in. `4/3` before a screen exists.
    pub(crate) fn layout_unit(&self) -> f32 {
        let (w, h) = (self.screen.width(), self.screen.height());
        let a = if h > 0.0 && w > 0.0 { w / h } else { 4.0 / 3.0 };
        768.0 * (a * a + 1.0).sqrt()
    }

    /// The **implicit rect**: a model pane that authored no size takes its
    /// file's bounding-box extent, in layout units, as its size — the reference's geometry
    /// overrides (`0x76d080`/`0x76d0d0`) answer the bbox whenever no size is authored, and its
    /// layout consumes them like any size. Written into the layout input when the facts are
    /// known; a pane with an authored size, or no file, or no facts yet, is left alone.
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

    /// A size was AUTHORED on `h` (`SetWidth`/`SetHeight`, the XML `<Size>` through
    /// them): an implicit rect no longer applies to it, for good — the reference's override
    /// yields to any authored size.
    pub(crate) fn note_authored_size(&mut self, h: FrameHandle) {
        if let Some(KindState::Model(m)) = self.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            m.implicit_size = false;
        }
    }

    /// The facts the host has handed over for `path`, or `None` — in which case the path is
    /// queued for the host to load ([`crate::script::UiScript::model_facts_wanted`]), once.
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

/// The facts of the file `this` pane currently holds (`None` for an empty pane or a file the
/// host has not loaded yet).
fn facts_of_pane(lua: &Lua, this: &Table) -> mlua::Result<Option<Arc<ModelFileFacts>>> {
    let path = with_model(lua, this, |m| m.path.clone())?;
    Ok(path.and_then(|p| {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .model_facts_for(&p)
    }))
}

/// Registry key of the `Model` method table (the MAXCSTACK discipline: Lua-side root, named key).
pub(super) const REG_MODEL_METHODS: &str = "__benilla_model_methods";

/// Registry key of the `PlayerModel` method table — its **own three** entries only. The other 23
/// names a `<PlayerModel>` answers come from [`REG_MODEL_METHODS`] through the dispatcher's chain,
/// exactly as the client's `0x506260` reaches `0x76f870` on a miss.
pub(super) const REG_PLAYERMODEL_METHODS: &str = "__benilla_playermodel_methods";

/// Run `f` over a frame's Model state under one short write borrow. Errors if `this` is not a live
/// Model (unreachable through the kind dispatcher, but the method table is a plain Lua value — a
/// caller can fish it out and misapply it).
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

/// A Lua number-ish → f32, `nil` and non-numbers → 0.0.
///
/// The widget's setters are all coordinates and angles, and the corpus passes them through
/// arithmetic that can produce a nil (`C.bars[...].icon_size / 25` when the config key is absent).
/// The reference marshals through `lua_tonumber`, which is this.
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

/// `lua_isnumber` — a number, or a string that converts, and **nothing else**. Distinct from
/// [`num`] on purpose: `SetLight` *raises* on a non-number where the coordinate setters coerce,
/// so the two questions cannot share one helper.
fn number(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => Some(*n as f32),
        Value::Integer(i) => Some(*i as f32),
        Value::String(s) => s.to_str().ok().and_then(|t| t.trim().parse().ok()),
        _ => None,
    }
}

/// `[0x8029d4]` — the client's "is this float zero" epsilon, shared by `SetLight`'s intensity
/// gate, `0x71b6a0`'s direction normalise and the paint's degenerate-rect test.
const FLOAT_EPS: f32 = 2.384_185_8e-7;

/// `Model:SetLight`'s usage string (`0x878c00`), which the binding raises verbatim.
const SET_LIGHT_USAGE: &str = "Usage: Model:SetLight(enabled[, omni, dirX, dirY, dirZ, \
     ambIntensity[, ambR, ambG, ambB], dirIntensity[, dirR, dirG, dirB]]";

/// What [`parse_set_light`] decided.
enum SetLight {
    /// A non-number where the binding requires one — `luaL_error(SET_LIGHT_USAGE)`.
    Raise,
    /// **Trap 1.** `enabled == 0` returns at `76e2cb` *before* the local light is copied into the
    /// widget, so the call writes nothing at all: it neither disables a light nor edits one.
    NoOp,
    /// The light to copy wholesale over the widget's (`76e777 0x76cf30`, `rep movsd 0x1b`).
    Set(ModelLight),
}

/// `Model:SetLight` `0x76e1e0`'s argument walk, over the arguments **after
/// `self`** — so `a[0]` is the reference's Lua index 2.
///
/// The binding builds a **local** `CGLight` from `0x71b4a0` — type 1, every colour ZERO, which is
/// not the widget ctor's white — fills it, and copies the whole thing over the widget's at the
/// end. So an omitted colour block lands the *binding's* black, never the ctor's white.
///
/// **Trap 2** is the walking cursor: the ambient colour triple is read only when its intensity is
/// nonzero **and** all three components are numbers; when either fails, the colour stays white
/// and the cursor does *not* advance past it — so the second intensity is read at index 8 rather
/// than 11, and `SetLight(1, 0, x, y, z, ambI, dirI)` is a legal seven-argument form.
fn parse_set_light(a: &[Value]) -> SetLight {
    let Some(enabled) = a.first().and_then(number) else {
        return SetLight::Raise;
    };
    if enabled as i32 == 0 {
        return SetLight::NoOp;
    }
    // Indices 3..=7 — omni, x, y, z, ambIntensity — are all mandatory once the light is enabled.
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
        // `0x71b6a0` normalises a DIRECTION on write (`71b6c5`-`71b705`); the position setter
        // `0x71b650` stores raw.
        let len = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
        if len > FLOAT_EPS {
            vector = vector.map(|v| v / len);
        }
    }
    // The colour blocks: each packed to 8 bits per channel and unpacked again (`0x76f900`), so a
    // Set→Get round trip is lossy exactly the way `SetFogColor`'s is.
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

/// **Shape A's gate** (decision 1717's taxonomy, for a *string* position): the client's
/// `lua_isstring` accepts tags 3|4 — a string **or a number** — and nothing else. A number is
/// rendered to decimal text and used as the path, which is why this coerces rather than matching
/// `Value::String` alone.
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

/// The client's own `Usage:` error, with the receiver's name interpolated the way it interpolates
/// it — `GetName()`, or `<unnamed>` when the widget has none (`0x84c7f0`).
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
    // `SetModel` and `PlayerModel:SetUnit` are the two ways a pane gets content and they are
    // alternatives, not layers: setting one clears the other, so `GetModel` after a `SetUnit`
    // cannot answer a stale path from three frames ago. Only the `SetModel` arm is a `Model` verb —
    // the unit arm is `PlayerModel`'s (`playermodel_install` below), which is why the paper-doll
    // and dress-up panes are `<PlayerModel>`s and every corpus `<Model>` drives the path arm.
    // **`SetModel(nil)` RAISES. It is not a clear** — `ClearModel` is the clear, and it is a
    // separate entry in the same table. This binding said the opposite until 2026-08-30 ("the
    // documented clear, and how the corpus writes 'no model'"), which was invented: the byte read
    // (`0x76d950`, shape A) gates the argument with `lua_isstring` and raises
    // `Usage: %s:SetModel("file")` on nil, absent, boolean, table, function, userdata and thread.
    // Measured before changing it — all 5 corpus call sites pass a string literal and our own
    // FrameXML never calls it — so nothing was relying on the leniency.
    //
    // **What is NOT reproduced, deliberately:** the reference's *second* raise,
    // `Invalid model file: %s` (`0x878b44`), for a path that does not resolve — its model load is
    // synchronous, so it knows. We render no FrameXML models at all, so we cannot evaluate that
    // condition, and a raise whose predicate we have to guess is worse than a named omission
    // (1134 §4). A path that would be invalid is simply stored.
    m.set(
        "SetModel",
        lua.create_function(|lua, (this, path): (Table, Value)| {
            let path = string_arg(&path)
                .ok_or_else(|| usage(lua, &this, "SetModel(\"file\")"))?
                .to_string();
            // The file's facts, if the host has handed them over — the reference's "asset
            // resident" test (`71d5a3`), which decides whether the loader's Stand seed runs
            // now or when the file lands ([`ModelState::set_file`]).
            let facts = lua
                .app_data_mut::<Model>()
                .expect("model app_data")
                .model_facts_for(&path);
            with_model(lua, &this, |m| m.set_file(path, facts.as_deref()))?;
            // A size-less pane takes the file's rect the moment the file is known (2015).
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
    // Both verbs are the same arm, `0x7121a0(model, -1, id, 0, ms, 1.0f, 0, 1)` — `SetSequence`
    // with `ms = 0` (`0x76dec0` → `0x76cf50`), `SetSequenceTime` with the caller's `ms`
    // (`0x76dfc0` → `0x76cf80`). The arm interrupts whatever plays, resolves the id through the
    // file's `animationLookup`, and bakes the anchor `cursor_lo = sceneClock − trunc(ms)` that
    // the sampler re-reads every frame (`0x71273d`). The id is an `AnimationData` id, not a
    // file slot; one the file does not own stops the playing track and arms nothing.
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
    // `SetFacing` is `Model`'s yaw setter (`0x878948[4]` -> `0x76dce0`, writing `+0x39c`).
    // `SetRotation` writes THE SAME FIELD but is `PlayerModel`'s, not a second name here — see
    // `playermodel_install`. A `<Model>` that wants its pane turned calls this one.
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
    // `SetCamera(n)` = `0x76e0e0` -> `0x76cec0`: select the file's camera by **RAW table index**
    // (`cameraLookup` is not on this path). With the file's facts in hand the index is resolved
    // now — an index past the table's count installs the NULL camera, which is the orthographic
    // leg; without them it is deferred, and the pane draws nothing until it resolves.
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
    //
    // Typed since decision 2027, after the reference: the widget's embedded `CGLight` and the four
    // fog fields, with `SetLight`'s argument walk (`0x76e1e0`) and both of its traps.
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
            // Each colour block comes back as `1.0, r, g, b` when ANY component is `> 0`, and as
            // a lone `0` otherwise (`76e91f`/`76e92f`/`76e93f je 0x76e953` -> `push 1.0`) — which
            // is what makes the arity 7 | 10 | 13 rather than a fixed 13.
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
                // FIVE arguments, and the fifth is the alpha — the same numeric path and the same
                // `[0,1]` clamp as the components, not a flag. It is **guarded with a default of
                // `1.0`** where r/g/b are read unconditionally and yield `0.0` when absent, so
                // `SetFogColor(r, g, b)` sets alpha 1.0. Getting that backwards renders the fog
                // invisible.
                let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
                let a = a.as_ref().map_or(1.0, num);
                let packed = (q(a) << 24) | (q(num(&r)) << 16) | (q(num(&g)) << 8) | q(num(&b));
                // `76f059 or [edi+0x3a4],1` — setting the colour ARMS the fog. That is the only
                // arming verb (with the XML `<FogColor>` child); there is no `SetFogEnabled`.
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
            // FOUR values, always — the packed dword unpacked. Never set reads `1, 1, 1, 1`,
            // because the `CSimpleModel` ctor's terminal write is `0xffffffff`.
            let packed = with_model(lua, &this, |m| m.fog_color)?;
            let ch = |shift: u32| f64::from((packed >> shift) & 0xff) / 255.0;
            Ok((ch(16), ch(8), ch(0), ch(24)))
        })?,
    )?;

    // `SetFogNear 0x76f1e0` / `SetFogFar 0x76f390` store the number RAW — no clamp, no ordering
    // check (`76f282`/`76f432 fstp`); the `≥ 0` clamp exists only on the XML attribute path. The
    // getters read the same fields back. Nothing here arms the fog.
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
    // `ClearFog 0x76f540`: `76f5c5 and [edi+0x3a4],-2` — **bit 0 and nothing else**. The colour,
    // the near and the far all survive, so a later `SetFogColor` re-arms the same ramp. A clear
    // that also reset them would read as knowledge and be wrong.
    m.set(
        "ClearFog",
        lua.create_function(|lua, this: Table| with_model(lua, &this, |m| m.fog = false))?,
    )?;

    // ── The two verbs that touch no pane state ──────────────────────────────────────────────

    // `AdvanceTime()` — `0x878948[14]` -> `0x76eca0`. **It does nothing, and that is the verified
    // behaviour, not a stub.** The function takes no Lua argument (no `lua_gettop`, no `tonumber`,
    // no `tostring` anywhere in `[0x76eca0, 0x76ed65)`), returns no value, and its one reachable
    // call is `0x76cfb0`, whose entire body is `mov eax,1; ret`. It writes no `CSimpleModel` field
    // and no global. So there is no clock to advance here and no omission to name: a receiver
    // check and zero returns IS the reference.
    //
    // Present because the shipped chain calls it three times — `Cooldown.lua`, and the glue
    // character screens' `<OnUpdateModel>` on `<ModelFFX>`, which inherits this very table — and
    // an addon that hooks one of those will call it too.
    m.set(
        "AdvanceTime",
        lua.create_function(|lua, this: Table| with_model(lua, &this, |_| ()))?,
    )?;

    // `ReplaceIconTexture(path)` — `0x878948[15]` -> `0x76ed70`. **A material swap, not a content
    // setter**: `0x76cfe0(0xe, path)` -> `0x710ec0` replaces the refcounted handle in
    // `[CM2Model+0xa4][i]` for every M2 texture whose `type == 14`, plus the same-typed slots on
    // the ribbon (`0x7b7950`) and particle (`0x7b4d20`) emitters. It sets no pane content, clears
    // no unit, and stores nothing on the `0x3dc`-byte widget — the override lives on the CM2Model
    // and dies with it when `SetModel`/`ClearModel` releases the instance.
    //
    // The reference's two "not ready" cases behave OPPOSITELY: with a `CM2Model` present but its
    // data not resident the call is queued on `[cm2+0x3c]` and replayed; with **no CM2Model at
    // all** (`[widget+0x318] == 0`) it is **dropped and never replayed**. Since decision 2007 a
    // pane with a file has an instance the host renders, so the override is STORED on the pane
    // (`ModelState::icon`, the queued-and-replayed case — the host applies it whenever the file
    // is resident) and a pane with no file drops it, as the client does. The stock
    // `MainMenuBarBagButtons.lua` is the caller: `ItemAnim_OnEvent` puts the pushed item's icon
    // on `ForcedBackpackItem.mdx`, whose one batch carries no texture of its own.
    //
    // The argument gate is shape A: `lua_isstring` (tags 3|4) then `lua_tostring`, raising
    // `Usage: %s:ReplaceIconTexture("texture")` on anything else.
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

/// `PlayerModel`'s **own three** verbs (table `0x84f1fc`) — nothing else. Everything a
/// `<PlayerModel>` else answers is `Model`'s, reached through the dispatcher's chain.
fn playermodel_install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // `SetUnit(unit)` — `0x84f1fc[0]` -> `0x505d70`. The pane's other content arm, and `SetModel`'s
    // alternative: each clears the other, so `GetModel` after a `SetUnit` cannot answer a stale
    // path. Ours stores the unit TOKEN and resolves it at render.
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
            // On the dressing room's pane the same call rebuilds the model from the unit — the
            // reference's `0x5059a0` duplicates the live model, substitutions and all gone — which
            // is app state here (`super::dressup`, 1969).
            super::dressup::redress_if_dressup(lua, &this)
        })?,
    )?;

    // `RefreshUnit()` — `0x84f1fc[1]` -> `0x505e40`. Re-reads the unit the pane already shows; for
    // us a no-op with a live receiver check, because the pane holds the TOKEN and resolves it at
    // render, so there is no cached appearance here to invalidate. Present because the reference's
    // own `DressUpFrame`/`PaperDollFrame` call it (3 sites) and addons that hook them will too.
    m.set(
        "RefreshUnit",
        lua.create_function(|lua, this: Table| {
            with_model(lua, &this, |_| ())?;
            // the same worker as SetUnit's (`0x505b50`)
            super::dressup::redress_if_dressup(lua, &this)
        })?,
    )?;

    // `SetRotation(rad)` — `0x84f1fc[2]` -> the Lua glue `0x505f00` -> the worker `0x505bb0`, whose
    // final instruction is `0x505c44 mov [esi+0x39c], eax`: **the same yaw field `SetFacing`
    // writes**, so the Lua-observable effect of the two verbs is identical and `GetFacing` reads
    // either back. That equality is the whole reason this can be one line.
    //
    // What `0x505bb0` does BESIDES the yaw write is deliberately not modeled, and is worth naming
    // because it is real: it picks a turn animation from the sign of the change (`0xc`
    // ShuffleRight when the current facing is **<** the argument, `0xb` ShuffleLeft when **>**,
    // `0` Stand on equality or NaN), plays it unless that id is already armed on bone slot 0,
    // and then
    // UNCONDITIONALLY sets `[+0x3e8] = 1` and `[+0x3ec] = now_ms + 100` — a 100 ms turn hold that
    // the per-paint `0x505c50` expires. Every one of those is invisible to Lua (no getter reads
    // them). The app's `<Model>` renderer (`benilla-app` `ui_models`, 2013/2019/2027) draws its
    // pane off the file's own clock and does not model a turn hold, so storing the pair here
    // would be state nobody writes and nobody reads. The addresses are the pin if it ever does.
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
    /// A named model pane's scene state, or `None` if no live frame carries that name as a
    /// `Model`/`PlayerModel`.
    ///
    /// **The read side of the pane, for a host that draws it.** The app keeps one off-screen body
    /// bake per window (this module's header: the scene is ours, the pixels are the host's), and
    /// every frame it has to ask the pane the reference drives which way it is turned. Before
    /// decision 1751 that came from benilla-named globals a file of ours called
    /// (`BenillaPaperDollModel_SetFacing` and its four siblings, one scalar per window on
    /// [`Model`]); the stock FrameXML instead calls
    /// `Model_OnLoad`/`Model_RotateLeft`/`Model_RotateRight`/`Model_OnUpdate`, which write
    /// `PlayerModel:SetRotation` — i.e. this state, on the pane itself. So the host reads the pane.
    ///
    /// By NAME rather than by handle because that is what the host knows: the character sheet's
    /// bake belongs to `CharacterModelFrame`, and since 1751 that name is the reference's own. A
    /// linear scan, because a name index would be a second store to keep true and a loaded
    /// interface holds single-digit model panes.
    pub fn model_pane(&self, name: &str) -> Option<ModelState> {
        self.model_ref()
            .arena
            .iter_frames()
            .find_map(|(_, f)| match (&f.name, &f.kind_state) {
                (Some(n), KindState::Model(m)) if n == name => Some(m.clone()),
                _ => None,
            })
    }

    /// A named model pane's yaw in radians — [`Self::model_pane`]'s one hot field, and `0.0` for a
    /// pane that does not exist yet (its window's file not loaded, or its `OnLoad` not run).
    ///
    /// `0.0` rather than an `Option` because every caller is a per-frame booth mirror whose only
    /// other answer would be "keep the last one", and a pane that has not loaded has no last one.
    /// The reference's own default, 0.61, is authored in `Model_OnLoad`, so a real pane only reads
    /// 0.0 before it loads.
    pub fn model_pane_facing(&self, name: &str) -> f32 {
        self.model_pane(name).map_or(0.0, |m| m.facing)
    }

    /// The host hands over what it knows about a model **file**: its sequences
    /// and bounds, read off the loaded asset. Every pane holding that file then runs the
    /// reference's residency completion — the loader's Stand seed for a pane that was waiting,
    /// the ownership check for an arm that was queued ([`ModelState::seed_from_facts`]) — and
    /// its clock starts answering ([`ModelState::play_head`]).
    pub fn set_model_facts(&mut self, path: &str, facts: ModelFileFacts) {
        let key = model_key(path);
        let facts = Arc::new(facts);
        let mut model = self.model_mut();
        model.model_facts.insert(key.clone(), facts.clone());
        model.model_facts_wanted.retain(|k| *k != key);
        // A one-off walk per file, not per frame: the panes that named this file are the
        // waiters the reference links on the streaming drain (`0x71d640`).
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

    /// The screen's aspect moved: every implicit rect is measured in layout units, which scale
    /// with `√(a²+1)`, so each one is re-derived. An authored size is untouched.
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

    /// The model files panes have named that no facts have arrived for — drained: the host
    /// loads each once and answers through [`Self::set_model_facts`]. Keys are
    /// [`crate::widget::model_key`]s (case-folded, `/`-separated, no extension).
    pub fn model_facts_wanted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().model_facts_wanted)
    }

    /// Does the engine hold facts for `path`'s file — the host's "already answered" test.
    pub fn has_model_facts(&self, path: &str) -> bool {
        self.model_ref().model_facts.contains_key(&model_key(path))
    }

    /// **The paint list**: every effectively-visible model pane holding a file whose facts the
    /// engine has — with its scene clock and, when something is armed, its play head. The host's
    /// renderer reads this once per frame instead of the extract carrying a
    /// cursor that moves every tick (`QuadContent::ModelPane`'s doc says why). Handle order —
    /// the arena's registry order, stable across frames.
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
                // The reference's draw gate (`76d5f0 cmp [this+0x320],-1 ; jne`): a pane whose
                // camera question is still open paints NOTHING — not the model, not its
                // `OnUpdateModel`. Reachable from Lua by `SetCamera(n)` on a pane whose file has
                // not landed yet.
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

/// One visible model pane as the host's renderer sees it this frame —
/// [`UiScript::visible_model_panes`]'s row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelPaneFrame {
    pub handle: crate::widget::FrameHandle,
    /// The pane's private scene clock, ms ([`ModelState::clock_ms`]) — the global-sequence
    /// clock of everything the pane draws.
    pub clock_ms: u64,
    /// Where the armed sequence stands, or `None` while nothing is armed (the file draws at its
    /// rest pose).
    pub play: Option<crate::widget::ModelPlayHead>,
}
