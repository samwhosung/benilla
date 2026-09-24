//! The world map's **arrow frames** (`0x4a8bb0`): the seven bindings the stock
//! `WorldMapFrame.lua` and `Blizzard_BattlefieldMinimap.lua` call to put the player's arrow on
//! the map.
//!
//! The reference keeps **two singletons per UI session** — the world map's and the battlefield
//! minimap's — each a stock `Model` widget (the same class `CreateFrame("Model")` makes) born as a
//! child of the frame the first `Create…` was given, loaded with `Interface\Minimap\MinimapArrow.mdx`,
//! shown, and never freed: every later `Create…` is a no-op even with a different parent. Its
//! rect is the model's bounding box (no authored size), its model is re-centred on that rect by
//! every `Update…`, and its facing is the camera-tracked object's — the player's. `Position…` is
//! literally `arrow:SetPoint(...)` with the generic binder's three error strings and two silent
//! legs; `Show…` is `arrow:Show()`/`Hide()` under the never-raising boolean coercion.
//!
//! Here the singleton is an anonymous `Model` frame this module creates through the same factory
//! `CreateFrame` uses, sized to the arrow's byte-measured footprint, and the app draws it: a
//! `Model` frame whose file is the minimap arrow extracts as [`QuadContent::ModelPane`] with the
//! path and the facing, and the app's model-pane arm paints the arrow sprite there
//! (`ui_script/extract`).

use mlua::{Lua, MultiValue, ObjectLike, Table, Value};

use super::binding_abi::bool_or_default;
use super::object::{create_frame, decode_id, frame_handle_of, frame_wrapper, point_from_str};
use super::Model;

/// The arrow's model — `0x8453c0`, and NOT `Rotating-MinimapArrow`.
pub const ARROW_MODEL: &str = "Interface\\Minimap\\MinimapArrow.mdx";

/// The arrow's rect is the file's own bounding box — the implicit rect of a size-less pane
/// (`MinimapArrow.mdx`'s `0.0262 × 0.0263` model units read as layout units,
/// `1280·extent = 33.5 × 33.7` FrameXML units at 4:3). The world-map arrow's model scale is
/// `G48 · 5/3` (`= 1.0` at 4:3, `G48 = 1/√(aspect² + 1)`) and the mini's `G48 · 10/9`, so the
/// quad holds a constant apparent size as the window's shape changes while its rect grows with
/// `√(a²+1)` (`0x4a7ad8`).
fn centre_of(facts: Option<&crate::widget::ModelFileFacts>) -> (f32, f32, f32) {
    facts.map_or((0.0, 0.0, 0.0), |f| {
        let (x, y) = f.extent();
        (x * 0.5, y * 0.5, 0.0)
    })
}

/// Which of the two singletons a binding addresses.
#[derive(Clone, Copy)]
enum Arrow {
    World,
    Mini,
}

impl Arrow {
    fn slot(self, model: &Model) -> Option<u32> {
        match self {
            Arrow::World => model.worldmap.arrow_world,
            Arrow::Mini => model.worldmap.arrow_mini,
        }
    }
    fn set_slot(self, model: &mut Model, id: u32) {
        match self {
            Arrow::World => model.worldmap.arrow_world = Some(id),
            Arrow::Mini => model.worldmap.arrow_mini = Some(id),
        }
    }
    /// The model-scale numerator: `5/3` for the world map's arrow, `10/9` for the mini's.
    fn scale_over_g48(self) -> f32 {
        match self {
            Arrow::World => 5.0 / 3.0,
            Arrow::Mini => 10.0 / 9.0,
        }
    }
    fn usage_create(self) -> &'static str {
        match self {
            Arrow::World => "Usage: CreateWorldMapArrowFrame(parent)",
            Arrow::Mini => "Usage: CreateMiniWorldMapArrowFrame(parent)",
        }
    }
    fn no_this(self) -> &'static str {
        match self {
            Arrow::World => "CreateWorldMapArrowFrame(): Couldn't find 'this' in parent object",
            Arrow::Mini => "CreateMiniWorldMapArrowFrame(): Couldn't find 'this' in parent object",
        }
    }
    fn not_a_frame(self) -> &'static str {
        match self {
            Arrow::World => "CreateWorldMapArrowFrame(): Wrong object type, expected frame",
            Arrow::Mini => "CreateMiniWorldMapArrowFrame(): Wrong object type, expected frame",
        }
    }
    fn usage_position(self) -> &'static str {
        match self {
            Arrow::World => {
                "Usage: PositionWorldMapArrowFrame(\"point\" \"frame\" [, relativePoint] [, offsetX, offsetY])"
            }
            Arrow::Mini => {
                "Usage: PositionMiniWorldMapArrowFrame(\"point\" \"frame\" [, relativePoint] [, offsetX, offsetY])"
            }
        }
    }
}

/// `[0x832a48]` — the Y half-extent of the reference's layout space, `1/√(A² + 1)` for the
/// screen's aspect `A`; `0.6` at 4:3.
fn g48(model: &Model) -> f32 {
    let (w, h) = (model.screen.width(), model.screen.height());
    let a = if h > 0.0 { w / h } else { 4.0 / 3.0 };
    1.0 / (a * a + 1.0).sqrt()
}

/// The arrow's model scale for this screen, and the footprint that scale gives it.
fn arrow_scale(model: &Model, which: Arrow) -> f32 {
    g48(model) * which.scale_over_g48()
}

/// `lua_isstring`: a string, or a number (which `lua_tostring` would print).
fn is_string(v: &Value) -> bool {
    matches!(v, Value::String(_) | Value::Integer(_) | Value::Number(_))
}

fn as_string(v: &Value) -> mlua::Result<String> {
    match v {
        Value::String(s) => Ok(s.to_str()?.to_string()),
        Value::Integer(i) => Ok(i.to_string()),
        Value::Number(n) => Ok(super::binding_abi::lua_number_text(*n)),
        _ => Ok(String::new()),
    }
}

/// `lua_isnumber`: a number, or a string that parses as one.
fn as_number(lua: &Lua, v: &Value) -> Option<f64> {
    lua.coerce_number(v.clone()).ok().flatten()
}

fn create(lua: &Lua, which: Arrow, parent: Value) -> mlua::Result<()> {
    let Value::Table(t) = parent else {
        return Err(mlua::Error::runtime(which.usage_create()));
    };
    if decode_id(&t).is_err() {
        return Err(mlua::Error::runtime(which.no_this()));
    }
    if frame_handle_of(lua, &t).is_err() {
        return Err(mlua::Error::runtime(which.not_a_frame()));
    }
    {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        if which.slot(&model).is_some() {
            return Ok(()); // the singleton exists: the parent is not even read (`0x4a7a80`)
        }
    }
    let wrapper = create_frame(
        lua,
        ("Model".to_string(), None, Some(Value::Table(t)), None),
    )?;
    let id = decode_id(&wrapper)?;
    let scale = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        arrow_scale(&model, which)
    };
    // `SetModel("Interface\Minimap\MinimapArrow.mdx")` in C++ (`0x4a7a80` → `0x76c8e0`): the
    // same file set every pane takes, seeded with the file's facts when the host has them
    // (the arrow's Stand loops its 3.333 s with no bone keyed, so nothing
    // moves; the arm is the reference's, not a look). No authored size: the widget's rect is
    // the file's bounding box, and `0x4a7b20`'s `SetPosition(½·GetWidth,
    // ½·GetHeight, 0)` — the geometry override's bbox extent, in layout units — centres the
    // model on it.
    let facts = lua
        .app_data_mut::<Model>()
        .expect("model app_data")
        .model_facts_for(ARROW_MODEL);
    super::modelframe::with_model(lua, &wrapper, |m| {
        m.set_file(ARROW_MODEL.to_string(), facts.as_deref());
        m.scale = scale;
        m.position = centre_of(facts.as_deref());
    })?;
    let h = frame_handle_of(lua, &wrapper)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    model.apply_implicit_rect(h);
    which.set_slot(&mut model, id);
    Ok(())
}

/// The singleton's wrapper, or `None` while it does not exist.
fn arrow_wrapper(lua: &Lua, which: Arrow) -> mlua::Result<Option<Table>> {
    let id = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        which.slot(&model)
    };
    id.map(|id| frame_wrapper(lua, id)).transpose()
}

/// `UpdateWorldMapArrowFrames()` (`0x4a8d10`/`0x4a7c20`): for each existing arrow, re-centre the
/// model on its rect and copy the camera-tracked object's facing — the player's, which the app
/// pushes with the world-map feed — into its rotation about +Z. No player → neither facing
/// moves.
fn update(lua: &Lua) -> mlua::Result<()> {
    for which in [Arrow::World, Arrow::Mini] {
        let Some(wrapper) = arrow_wrapper(lua, which)? else {
            continue;
        };
        let (facing, tracked, facts) = {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            (
                model.worldmap.player_facing,
                model.worldmap.player_uv.is_some(),
                model.model_facts_for(ARROW_MODEL),
            )
        };
        super::modelframe::with_model(lua, &wrapper, |m| {
            m.position = centre_of(facts.as_deref());
            if tracked {
                m.facing = facing;
            }
        })?;
    }
    Ok(())
}

/// `Position…("point", "frame" [, relativePoint] [, offsetX, offsetY])` (`0x4a8d20`/`0x4a8f20`):
/// silent with no arrow; args 1 and 2 must be strings (a number is one) else the usage raise;
/// the point through the same table `SetPoint` reads, else `Unknown frame point`; the frame by
/// name — `$parent` expanded against the arrow's parent, then `_G` — else `Couldn't find frame
/// named '%s'`, and the arrow itself `Error: %s is anchored to itself`; a non-string third
/// argument means `relativePoint = point` and zero offsets with no error; offsets only when both
/// are numbers.
fn position(lua: &Lua, which: Arrow, args: [Value; 5]) -> mlua::Result<()> {
    let Some(arrow) = arrow_wrapper(lua, which)? else {
        return Ok(());
    };
    let [point, frame, rel, x, y] = args;
    if !is_string(&point) || !is_string(&frame) {
        return Err(mlua::Error::runtime(which.usage_position()));
    }
    let point_name = as_string(&point)?;
    if point_from_str(&point_name).is_none() {
        return Err(mlua::Error::runtime("Unknown frame point"));
    }
    let frame_name = as_string(&frame)?;
    let resolved = if let Some(rest) = frame_name.strip_prefix("$parent") {
        // The first NAMED ancestor of the arrow — its parent frame, or that frame's own parent.
        let parent_name: Option<String> = arrow
            .call_method::<Option<Table>>("GetParent", ())?
            .and_then(|p| {
                p.call_method::<Option<String>>("GetName", ())
                    .ok()
                    .flatten()
            });
        parent_name.map(|p| format!("{p}{rest}"))
    } else {
        Some(frame_name.clone())
    };
    let target: Option<Table> = resolved
        .as_deref()
        .and_then(|n| lua.globals().get::<Option<Table>>(n).ok().flatten())
        .filter(|t| frame_handle_of(lua, t).is_ok());
    let Some(target) = target else {
        return Err(mlua::Error::runtime(format!(
            "Couldn't find frame named '{}'",
            resolved.unwrap_or(frame_name)
        )));
    };
    if decode_id(&target)? == decode_id(&arrow)? {
        return Err(mlua::Error::runtime(format!(
            "Error: {} is anchored to itself",
            resolved.unwrap_or(frame_name)
        )));
    }
    let (rel_point, ox, oy) = if is_string(&rel) {
        let r = as_string(&rel)?;
        if point_from_str(&r).is_none() {
            return Err(mlua::Error::runtime("Unknown frame point"));
        }
        match (as_number(lua, &x), as_number(lua, &y)) {
            (Some(x), Some(y)) => (r, x, y),
            _ => (r, 0.0, 0.0),
        }
    } else {
        (point_name.clone(), 0.0, 0.0)
    };
    arrow.call_method::<()>(
        "SetPoint",
        (point_name, Value::Table(target), rel_point, ox, oy),
    )
}

/// `Show…([shown])` (`0x4a9120`/`0x4a9170`): silent with no arrow; the never-raising boolean
/// coercion with a default of true (absent → show, `nil` → hide).
fn show(lua: &Lua, which: Arrow, args: MultiValue) -> mlua::Result<()> {
    let Some(arrow) = arrow_wrapper(lua, which)? else {
        return Ok(());
    };
    // An ABSENT argument is not nil at the C API (`LUA_TNONE`, which the binding's nil test does
    // not match), so the bare call shows; an explicit nil hides; anything else is the boolean
    // coercion with its true default.
    let on = match args.front() {
        None => true,
        Some(Value::Nil) => false,
        Some(v) => bool_or_default(Some(v), true),
    };
    arrow.call_method::<()>(if on { "Show" } else { "Hide" }, ())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();
    for (which, create_name, position_name, show_name) in [
        (
            Arrow::World,
            "CreateWorldMapArrowFrame",
            "PositionWorldMapArrowFrame",
            "ShowWorldMapArrowFrame",
        ),
        (
            Arrow::Mini,
            "CreateMiniWorldMapArrowFrame",
            "PositionMiniWorldMapArrowFrame",
            "ShowMiniWorldMapArrowFrame",
        ),
    ] {
        g.set(
            create_name,
            lua.create_function(move |lua, parent: Value| create(lua, which, parent))?,
        )?;
        g.set(
            position_name,
            lua.create_function(
                move |lua, (p, f, r, x, y): (Value, Value, Value, Value, Value)| {
                    position(lua, which, [p, f, r, x, y])
                },
            )?,
        )?;
        g.set(
            show_name,
            lua.create_function(move |lua, args: MultiValue| show(lua, which, args))?,
        )?;
    }
    g.set(
        "UpdateWorldMapArrowFrames",
        lua.create_function(|lua, ()| update(lua))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    /// The arrow file's facts, as the host hands them over (2007/2015): the header box the
    /// implicit rect and the re-centring read.
    fn arrow_facts(s: &mut UiScript) {
        s.set_model_facts(
            ARROW_MODEL,
            crate::widget::ModelFileFacts {
                sequences: vec![crate::widget::SequenceFacts {
                    anim_id: 0,
                    duration_ms: 3333,
                    looping: true,
                }],
                bbox: ([-0.0127, -0.0118, 0.0], [0.0135, 0.0145, 0.0]),
                cameras: 0,
            },
        );
    }

    fn vm() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.run(
            r#"WM = CreateFrame("Frame", "WM") WM:SetWidth(1024); WM:SetHeight(768) WM:SetPoint("CENTER", 0, 0)
               Detail = CreateFrame("Frame", "WorldMapDetailFrame", WM)
               Detail:SetWidth(1002); Detail:SetHeight(668) Detail:SetPoint("CENTER", 0, 0)"#,
        )
        .unwrap();
        s
    }

    /// The three raise strings, then the singleton: an anonymous Model child of the parent,
    /// loaded with the arrow, born shown, sized to its footprint; a second create a no-op.
    #[test]
    fn create_validates_its_parent_and_makes_one_arrow_per_session() {
        let mut s = vm();
        for (call, want) in [
            (
                "CreateWorldMapArrowFrame(5)",
                "Usage: CreateWorldMapArrowFrame(parent)",
            ),
            (
                "CreateWorldMapArrowFrame({})",
                "CreateWorldMapArrowFrame(): Couldn't find 'this' in parent object",
            ),
            (
                "CreateWorldMapArrowFrame(WM:CreateTexture())",
                "CreateWorldMapArrowFrame(): Wrong object type, expected frame",
            ),
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains(want), "{call}: {err}");
        }
        s.run("CreateWorldMapArrowFrame(WM)").unwrap();
        let before = s
            .eval::<i64>("return table.getn({WM:GetChildren()})")
            .unwrap();
        s.run("CreateWorldMapArrowFrame(Detail)").unwrap();
        let after = s
            .eval::<i64>("return table.getn({WM:GetChildren()})")
            .unwrap();
        assert_eq!(
            before, after,
            "a second create is a no-op, whatever the parent"
        );
        let (kind, shown, w) = s
            .eval::<(String, bool, f32)>(
                "local a = ({WM:GetChildren()})[2] \
                 return a:GetObjectType(), a:IsShown(), a:GetWidth()",
            )
            .unwrap();
        assert_eq!(kind, "Model");
        assert!(shown, "born shown");
        assert_eq!(w, 0.0, "no authored size, no facts yet: no rect");
        // The file lands: the rect is its box in layout units — 0.0262 × 1280 at 4:3 — and the
        // model is centred on it (`0x4a7b20`, half the box, in layout units).
        arrow_facts(&mut s);
        // The C++ re-centres on create and on every `UpdateWorldMapArrowFrames` (`0x4a7b20`);
        // a file that lands after the create is picked up by the next update.
        s.run("UpdateWorldMapArrowFrames()").unwrap();
        s.resolve();
        let w: f32 = s.eval("return ({WM:GetChildren()})[2]:GetWidth()").unwrap();
        assert!((w - 0.0262 * 1280.0).abs() < 0.05, "the box at 4:3: {w}");
        let pos = {
            let model = s.lua().app_data_ref::<Model>().expect("model");
            let id = model.worldmap.arrow_world.expect("the singleton");
            let fh = model.id_to_frame.get(&id).copied().expect("live");
            match &model.arena.frame(fh).expect("frame").kind_state {
                crate::widget::KindState::Model(m) => m.position,
                _ => panic!("not a Model"),
            }
        };
        assert!(
            (pos.0 - 0.0131).abs() < 1e-5 && (pos.1 - 0.01315).abs() < 1e-5 && pos.2 == 0.0,
            "centred on the box: {pos:?}"
        );
        assert_eq!(
            s.arity("CreateWorldMapArrowFrame(WM)").unwrap(),
            0,
            "zero values"
        );
    }

    /// `Position…` anchors the arrow like SetPoint, with the reference's three strings and its
    /// two silent legs; `Show…` follows the boolean coercion; `Update…` copies the facing.
    #[test]
    fn position_show_and_update_drive_the_arrow() {
        let mut s = vm();
        // No arrow yet: every one of the three is silent, whatever the arguments.
        s.run("PositionWorldMapArrowFrame(1, 2) ShowWorldMapArrowFrame('x') UpdateWorldMapArrowFrames()")
            .unwrap();
        s.run("CreateWorldMapArrowFrame(WM)").unwrap();
        // The file's facts: a size-less pane has no rect to anchor until its box is known.
        arrow_facts(&mut s);
        s.run("arrow = ({WM:GetChildren()})[2]").unwrap();
        for (call, want) in [
            (
                "PositionWorldMapArrowFrame(nil, 'WorldMapDetailFrame')",
                "Usage: PositionWorldMapArrowFrame",
            ),
            (
                "PositionWorldMapArrowFrame('NOWHERE', 'WorldMapDetailFrame')",
                "Unknown frame point",
            ),
            (
                "PositionWorldMapArrowFrame('CENTER', 'NoSuchFrame')",
                "Couldn't find frame named 'NoSuchFrame'",
            ),
            (
                "PositionWorldMapArrowFrame('CENTER', 'WorldMapDetailFrame', 'BOGUS', 1, 2)",
                "Unknown frame point",
            ),
        ] {
            let err = s.run(call).unwrap_err().to_string();
            assert!(err.contains(want), "{call}: {err}");
        }
        s.run("PositionWorldMapArrowFrame('CENTER', 'WorldMapDetailFrame', 'TOPLEFT', 100, -50)")
            .unwrap();
        s.resolve();
        let (cx, cy, dl, dt) = s
            .eval::<(f32, f32, f32, f32)>(
                "local cx, cy = arrow:GetCenter() \
                 return cx, cy, WorldMapDetailFrame:GetLeft(), WorldMapDetailFrame:GetTop()",
            )
            .unwrap();
        assert!(
            (cx - (dl + 100.0)).abs() < 0.01 && (cy - (dt - 50.0)).abs() < 0.01,
            "{cx} {cy}"
        );
        // A non-string third argument: relativePoint = point, offsets ignored, no error.
        s.run("PositionWorldMapArrowFrame('TOPLEFT', 'WorldMapDetailFrame', nil, 100, -50)")
            .unwrap();
        s.resolve();
        let (l, t, dl, dt) = s
            .eval::<(f32, f32, f32, f32)>(
                "return arrow:GetLeft(), arrow:GetTop(), WorldMapDetailFrame:GetLeft(), WorldMapDetailFrame:GetTop()",
            )
            .unwrap();
        assert!((l - dl).abs() < 0.01 && (t - dt).abs() < 0.01, "{l} {t}");
        // The name resolver expands $parent against the arrow's parent.
        s.run("PositionWorldMapArrowFrame('CENTER', '$parent')")
            .unwrap();

        s.run("ShowWorldMapArrowFrame(nil)").unwrap();
        assert!(
            !s.eval::<bool>("return arrow:IsShown()").unwrap(),
            "nil hides"
        );
        s.run("ShowWorldMapArrowFrame()").unwrap();
        assert!(
            s.eval::<bool>("return arrow:IsShown()").unwrap(),
            "absent shows"
        );
        s.run("ShowWorldMapArrowFrame('off')").unwrap();
        assert!(!s.eval::<bool>("return arrow:IsShown()").unwrap());
        s.run("ShowWorldMapArrowFrame(1)").unwrap();
        assert!(s.eval::<bool>("return arrow:IsShown()").unwrap());

        s.set_world_map_feed(None, Some((0.5, 0.5)), 1.25, None, Vec::new(), Vec::new());
        s.run("UpdateWorldMapArrowFrames()").unwrap();
        let facing = s.eval::<f32>("return arrow:GetFacing()").unwrap();
        assert!(
            (facing - 1.25).abs() < 1e-6,
            "the tracked object's facing: {facing}"
        );
        s.set_world_map_feed(None, None, 2.0, None, Vec::new(), Vec::new());
        s.run("UpdateWorldMapArrowFrames()").unwrap();
        assert!(
            (s.eval::<f32>("return arrow:GetFacing()").unwrap() - 1.25).abs() < 1e-6,
            "no tracked object: the facing does not move"
        );
    }
}
