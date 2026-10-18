//! The `Minimap` methods and engine children: the zoom, the three textures, the two arrow models and
//! the ping over the minimap's process-wide statics ([`crate::widget::MinimapStatics`]), and the
//! nine `Model` children its ctor makes, which draw the arrows. The reference has six zoom levels
//! (`0x6da9a0`), a live index per mode (`0x86f698` outdoor, `0x86f69c` indoor) and the
//! `minimapZoom`/`minimapInsideZoom` CVars (default `"3"`) that re-seed them (`0x6d9008`).

use mlua::{Lua, Table, Value};

use super::binding_abi::{frame_name, lua_number_text, number_arg, usage};
use super::object::{decode_id, frame_handle_of};
use super::Model;
use crate::widget::{
    FrameHandle, FrameKind, KindState, MinimapArt, MinimapArtSlot, MINIMAP_ZOOM_LEVELS,
};

/// Registry key of the Minimap method table.
pub(super) const REG_MINIMAP_METHODS: &str = "__benilla_minimap_methods";

/// The member prologue the setters share: `this` must be a table (`0x847ef8`), carry a
/// framescript object (`0x847ec0`) and be a Minimap, the `IsA` test against the Minimap class
/// token `0xbc8290` (`0x4ee000`, else `0x847e98`).
fn this_minimap(lua: &Lua, this: &Value) -> mlua::Result<FrameHandle> {
    let Value::Table(t) = this else {
        return Err(mlua::Error::runtime(
            "Attempt to find 'this' in non-table object (used '.' instead of ':' ?)",
        ));
    };
    if decode_id(t).is_err() {
        return Err(mlua::Error::runtime(
            "Attempt to find 'this' in non-framescript object",
        ));
    }
    let wrong = || mlua::Error::runtime("Wrong object type for member function");
    let h = frame_handle_of(lua, t).map_err(|_| wrong())?;
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    match model.arena.frame(h).map(|f| f.kind) {
        Some(FrameKind::Minimap) => Ok(h),
        _ => Err(wrong()),
    }
}

/// The `CMinimap` ctor's nine `Model` children (`0x4edbc0`), made with the minimap as parent, so
/// at its stratum and its level plus one: five party and pet arrows (`0x4ede72`), three rim
/// arrows (`0x4edf1f`) and the player arrow (`0x4edfae`). Each is anchored CENTER on the
/// minimap's CENTER (`0x767c70(4, minimap, 4, x, y)`): the eight at offset 0 (`0x4ede8b`,
/// `0x4edf38`), model scale 0.6 (`0x4edeba`) and hidden (`0x4edece`, `0x4edf7b`); the five
/// relevelled to the minimap's level plus 2 (`0x4edee0`); the player arrow sized by its file and
/// left shown. What the ctor takes from the screen-scale cells is [`seat_engine_children`]'s.
pub(crate) fn build_engine_children(lua: &Lua, minimap: FrameHandle) {
    use crate::layout::{Anchor, Point};
    let hide = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        let Some((strata, level)) = model.arena.frame(minimap).map(|f| (f.strata, f.level)) else {
            return;
        };
        let units = model.layout_units();
        let minimap_id = model.frame_id(minimap);
        let mut children = Vec::with_capacity(crate::widget::MINIMAP_ENGINE_CHILDREN);
        for i in 0..crate::widget::MINIMAP_ENGINE_CHILDREN {
            let child = model.arena.create(FrameKind::Model, None, Some(minimap));
            model.arena.set_frame_strata(child, strata);
            let lift = if i < crate::widget::MINIMAP_GROUP_ARROWS {
                2
            } else {
                1
            };
            model
                .arena
                .set_frame_level(child, level.saturating_add(lift), false);
            let player = i + 1 == crate::widget::MINIMAP_ENGINE_CHILDREN;
            if !player {
                let input = model.layout_inputs.entry(child).or_default();
                input.anchors.set(
                    Anchor::new(Point::Center, minimap_id, Point::Center, 0.0, 0.0),
                    units,
                );
                if let Some(KindState::Model(m)) =
                    model.arena.frame_mut(child).map(|f| &mut f.kind_state)
                {
                    m.scale = crate::widget::MINIMAP_ARROW_SCALE;
                }
            }
            children.push(child);
        }
        let (arrows, player) = children.split_at(crate::widget::MINIMAP_ENGINE_CHILDREN - 1);
        if let Some(KindState::Minimap(m)) =
            model.arena.frame_mut(minimap).map(|f| &mut f.kind_state)
        {
            m.group_arrows = arrows[..crate::widget::MINIMAP_GROUP_ARROWS].to_vec();
            m.rim_arrows = arrows[crate::widget::MINIMAP_GROUP_ARROWS..].to_vec();
            m.player_arrow = player.first().copied();
            m.level_base = i32::from(level) + 3;
        }
        seat_engine_children(&mut model, minimap);
        arrows.to_vec()
    };
    for child in hide {
        super::visibility::set_shown(lua, child, false);
    }
}

/// What the `CMinimap` ctor takes from the screen-scale cells: `base = G48 · 5/3` (`0x4edc42`,
/// `0x4edc47`), about 1 at 4:3; the eight arrow frames' width and height, the disc ([`disc`],
/// through slots 5 and 6 at `0x4ede9e`, `0x4edeaf`); the player arrow's offset of
/// 0.001 layout units each way (`0x4edfd1`) and its model scale `base` (`0x4edfec`). The reference
/// fixes these at the build; a change of cells rebuilds the interface, so the ctor runs again
/// with the new `G48`, and [`super::UiScript`]'s live resize re-seats them here instead.
pub(crate) fn seat_engine_children(model: &mut Model, minimap: FrameHandle) {
    use crate::layout::{Anchor, Point};
    let Some(KindState::Minimap(state)) = model.arena.frame(minimap).map(|f| &f.kind_state) else {
        return;
    };
    let arrows: Vec<FrameHandle> = state
        .group_arrows
        .iter()
        .chain(&state.rim_arrows)
        .copied()
        .collect();
    let player = state.player_arrow;
    let units = model.layout_units();
    let unit = model.layout_unit();
    // The player arrow's scale is the product as stored (`0x4edfe0`, `0x4edfec`).
    let base = units.k2() * (5.0 / 3.0);
    let disc = disc(units);
    let side = disc * unit;
    for h in arrows {
        let input = model.layout_inputs.entry(h).or_default();
        let changed = input.width.to_bits() != side.to_bits();
        input.width = side;
        input.height = side;
        model.invalidate_frame(h, changed);
    }
    if let Some(h) = player {
        let minimap_id = model.frame_id(minimap);
        let offset = 0.001 * unit;
        let input = model.layout_inputs.entry(h).or_default();
        let moved = input.anchors.set(
            Anchor::new(Point::Center, minimap_id, Point::Center, offset, offset),
            units,
        );
        model.invalidate_frame(h, moved);
        if let Some(KindState::Model(m)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            m.scale = base;
        }
    }
    if let Some(KindState::Minimap(m)) = model.arena.frame_mut(minimap).map(|f| &mut f.kind_state) {
        m.base = base;
        m.disc = disc;
    }
}

/// The disc, `[0xbc7970]`, in layout units: the ctor keeps `G48 · 5/3` on the x87 stack unstored
/// (`0x4edc42`, `0x4edc47`) and multiplies it by 0.11 (`[0x807a44]`, `0x4edca9`) before the one
/// store (`0x4edcaf`). `G48 · 5/3` of two floats is exact in a double, and the product with 0.11
/// rounds to the 53 bits the cells are worked at, then to the float.
pub(crate) fn disc(units: crate::layout::LayoutUnits) -> f32 {
    let base = f64::from(units.k2()) * f64::from(5.0f32 / 3.0);
    (base * f64::from(crate::widget::MINIMAP_DISC)) as f32
}

/// `LoadArrowModel` (`0x4ee170`): the minimap's level plus 3 becomes the rim arrows' level base
/// (`0x4ee1a3`), and the file goes on the eight arrow frames, the five party and pet arrows
/// playing `GroupArrow` (165, `0x4ee1de`) and the three rim arrows `Arrow` (166, `0x4ee22b`). The
/// model is made with `CreateModelFromPath` (`0x707350`), which puts the error cube in for a file
/// that does not load, so this answers 1 for any name; benilla's pane has no cube yet and draws
/// nothing for such a file (#330).
fn load_arrow_model(model: &mut Model, minimap: FrameHandle, path: &str) -> bool {
    let level = model.arena.frame(minimap).map_or(0, |f| i32::from(f.level));
    let (group, rim) = match model.arena.frame_mut(minimap).map(|f| &mut f.kind_state) {
        Some(KindState::Minimap(m)) => {
            m.level_base = level + 3;
            (m.group_arrows.clone(), m.rim_arrows.clone())
        }
        _ => return true,
    };
    for (frames, sequence) in [(group, GROUP_ARROW), (rim, LANDMARK_ARROW)] {
        for h in frames {
            model.set_pane_model(h, path.to_string());
            arm(model, h, sequence);
        }
    }
    true
}

/// `LoadPlayerModel` (`0x4ee260`): `SetModel` on the player arrow (slot 0x94, `0x76cce0`), which
/// clears it for an empty name; 0 when the arrow is left with no model (`0x4ee286`).
fn load_player_model(model: &mut Model, minimap: FrameHandle, path: &str) -> bool {
    let Some(h) = (match model.arena.frame(minimap).map(|f| &f.kind_state) {
        Some(KindState::Minimap(m)) => m.player_arrow,
        _ => None,
    }) else {
        return false;
    };
    if path.is_empty() {
        if let Some(KindState::Model(m)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
            m.clear_file();
        }
        return false;
    }
    model.set_pane_model(h, path.to_string());
    true
}

/// `SimpleModelPlaySequence` (`0x76cf50`): `id` from its start, against the facts the pane's file
/// has, or replayed when they land.
fn arm(model: &mut Model, h: FrameHandle, id: u16) {
    let path = match model.arena.frame(h).map(|f| &f.kind_state) {
        Some(KindState::Model(m)) => m.path.clone(),
        _ => return,
    };
    let facts = path.and_then(|p| model.model_facts_for(&p));
    if let Some(KindState::Model(m)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        m.arm(i32::from(id), 0, facts.as_deref());
    }
}

/// `AnimationData.dbc`'s arrow sequences, which light one quad of the arrow model each: 165
/// `GroupArrow`, 166 `Arrow`, 167 `CorpseArrow`, 168 `GuideArrow`.
pub const GROUP_ARROW: u16 = 0xa5;
pub const LANDMARK_ARROW: u16 = 0xa6;
pub const CORPSE_ARROW: u16 = 0xa7;
pub const GUIDE_ARROW: u16 = 0xa8;

/// The post-load half of `CMinimap::LoadXML` (`0x4ee2b0`), which runs `CSimpleFrame`'s post-load
/// first (`0x4ee2c0`), so after the `<Frames>`, the `OnLoad` and the `OnShow`: `minimapArrowModel`
/// to the eight arrow frames, `minimapPlayerModel` to the player arrow, each defaulting when absent.
/// A model that does not load warns as the reference logs (`0x84c73c`, `0x84c6fc`).
pub(crate) fn apply_model_attrs(
    lua: &Lua,
    this: &Table,
    arrow: &str,
    player: &str,
) -> mlua::Result<Vec<&'static str>> {
    let h = frame_handle_of(lua, this)?;
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let mut failed = Vec::new();
    if !load_arrow_model(&mut model, h, arrow) {
        failed.push("Invalid minimapArrowModel in Minimap.xml");
    }
    if !load_player_model(&mut model, h, player) {
        failed.push("Invalid minimapPlayerModel in Minimap.xml");
    }
    Ok(failed)
}

/// The model setters' one body (`SetArrowModel` `0x4ee980`, `SetPlayerModel` `0x4eeab0`): the member
/// prologue, `lua_isstring(2)` or `Usage: %s:<name>("file")`, the loader, and
/// `%s:<name>(): Couldn't load the file %s` when it answers 0.
fn set_model(
    lua: &Lua,
    this: &Value,
    path: Value,
    name: &str,
    load: fn(&mut Model, FrameHandle, &str) -> bool,
) -> mlua::Result<()> {
    let h = this_minimap(lua, this)?;
    let path = match path {
        Value::String(s) => s.to_string_lossy(),
        Value::Integer(i) => lua_number_text(i as f64),
        Value::Number(n) => lua_number_text(n),
        _ => {
            let this = this.as_table().expect("the prologue took a table");
            return Err(usage(lua, this, &format!("{name}(\"file\")")));
        }
    };
    let loaded = load(
        &mut lua.app_data_mut::<Model>().expect("model app_data"),
        h,
        &path,
    );
    if !loaded {
        let who = frame_name(&lua.app_data_ref::<Model>().expect("model app_data"), h);
        return Err(mlua::Error::runtime(format!(
            "{who}:{name}(): Couldn't load the file {path}"
        )));
    }
    Ok(())
}

/// One rim arrow the landmark selection kept, an arrow record (`0x4c` bytes, `0x6daa20`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimapRimArrow {
    /// The bearing to its target in radians, from north toward west (`+0x40`).
    pub bearing: f32,
    /// The static record it came from (`+0x48`): 0 the quest marker, 1 the gossip marker, 2 the
    /// corpse, −2 an `AreaPOI.dbc` row.
    pub slot: i32,
    /// `+0x44`: set for the gossip marker and the corpse, the arrows that show indoors.
    pub indoors: bool,
}

/// An arrow frame's model position and facing for `bearing`, as the render's rotate loops set them
/// (`0x4ed452`, `0x4ed4f7`): `(C − A·sin b, C + A·cos b, 0)` with `C` half the stored disc
/// (`[0xbc7970] · 0.5`, `0x4ed466`, `0x4ed472`) and `A` 0.8 of the stored half disc (`[0x807a24]`,
/// `[0xbc8274]`, `0x4ed458`), in layout units from the frame's bottom-left. `fsincos` stores its
/// sine and cosine to floats (`0x4ed454`, `0x4ed456`); `A`, `C` and each product and sum stay on
/// the stack, at the cells' 53 bits, until each coordinate's store (`0x4ed488`, `0x4ed497`).
fn place_arrow(model: &mut Model, h: FrameHandle, disc: f32, bearing: f32) {
    // Half a float is exact, so `C` is also `[0xbc8274]` as stored (`0x4edce5`).
    let c = f64::from(disc) * 0.5;
    let a = f64::from(0.8f32) * c;
    let (sin, cos) = f64::from(bearing).sin_cos();
    let (sin, cos) = (f64::from(sin as f32), f64::from(cos as f32));
    if let Some(KindState::Model(m)) = model.arena.frame_mut(h).map(|f| &mut f.kind_state) {
        m.position = ((c - a * sin) as f32, (c + a * cos) as f32, 0.0);
        m.facing = bearing;
    }
}

/// The texture setters' one body (`SetMaskTexture` `0x4ee4a0`): the member prologue, then
/// `lua_isstring(2)` (`0x4ee532`), whose failure raises `Usage: %s:<name>("file")` (`0x84c7fc`)
/// with the frame's name or `<unnamed>`. Then `TextureCreate` on `lua_tostring(2)` (`0x4ee5e4`),
/// its result stored over the old handle (`0x4ee5e9`) and, when the load logged at severity 2 or
/// more (`0x4ee5ee`), `%s:<name>(): Couldn't load the file %s` (`0x84c7c0`, `0x4ee623`) after the
/// store. An empty name loads nothing and logs nothing. With no host probe the file is taken.
fn set_art(
    lua: &Lua,
    this: &Value,
    path: Value,
    slot: MinimapArtSlot,
    name: &str,
) -> mlua::Result<()> {
    let h = this_minimap(lua, this)?;
    let path = match path {
        Value::String(s) => s.to_string_lossy(),
        Value::Integer(i) => lua_number_text(i as f64),
        Value::Number(n) => lua_number_text(n),
        _ => {
            let this = this.as_table().expect("the prologue took a table");
            return Err(usage(lua, this, &format!("{name}(\"file\")")));
        }
    };
    let art = if path.is_empty() {
        MinimapArt::Null
    } else {
        match super::region::texture_name_verdict(
            &lua.app_data_ref::<Model>().expect("model app_data"),
            &path,
        ) {
            super::region::FileLoad::Failed => MinimapArt::Missing,
            _ => MinimapArt::File(path.clone()),
        }
    };
    let failed = art == MinimapArt::Missing;
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .minimap
        .set_art(slot, art);
    if failed {
        let who = frame_name(&lua.app_data_ref::<Model>().expect("model app_data"), h);
        return Err(mlua::Error::runtime(format!(
            "{who}:{name}(): Couldn't load the file {path}"
        )));
    }
    Ok(())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    m.set(
        "GetZoom",
        // The indoor index while inside a WMO, else the outdoor one (`0x6da980`); no `this`.
        lua.create_function(|lua, _: mlua::MultiValue| {
            Ok(lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .minimap
                .active_zoom())
        })?,
    )?;
    m.set(
        "SetZoom",
        lua.create_function(|lua, (_this, zoom): (Value, Value)| {
            // No `this` lookup; argument 2 passes `lua_isnumber` (`0x4eec68`) or raises (`0x4eec8d`).
            let level = number_arg(lua, zoom, "Usage: SetZoom(level)")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(cvar) = model.minimap.set_zoom_index(level) {
                // The mode's CVar takes the new level, which is what survives a restart.
                let text = model.minimap.active_zoom().to_string();
                super::cvars::set_from_engine(&mut model, cvar, text);
            }
            Ok(())
        })?,
    )?;
    m.set(
        "SetMaskTexture",
        lua.create_function(|lua, (this, path): (Value, Value)| {
            set_art(lua, &this, path, MinimapArtSlot::Mask, "SetMaskTexture")
        })?,
    )?;
    m.set(
        // The landmark atlas `0xbc827c` (`0x4ee640`: store `0x4ee789`, texts `0x84c850`, `0x84c820`),
        // drawn as a fixed 8×8 grid whatever its size.
        "SetIconTexture",
        lua.create_function(|lua, (this, path): (Value, Value)| {
            set_art(lua, &this, path, MinimapArtSlot::Icons, "SetIconTexture")
        })?,
    )?;
    m.set(
        // The dot atlas `0xbc822c` (`0x4ee7e0`: store `0x4ee929`, texts `0x84c8a4`, `0x84c874`),
        // drawn as a fixed 4×4 grid of quarters.
        "SetBlipTexture",
        lua.create_function(|lua, (this, path): (Value, Value)| {
            set_art(lua, &this, path, MinimapArtSlot::Blips, "SetBlipTexture")
        })?,
    )?;
    m.set(
        "SetArrowModel",
        lua.create_function(|lua, (this, path): (Value, Value)| {
            set_model(lua, &this, path, "SetArrowModel", load_arrow_model)
        })?,
    )?;
    m.set(
        "SetPlayerModel",
        lua.create_function(|lua, (this, path): (Value, Value)| {
            set_model(lua, &this, path, "SetPlayerModel", load_player_model)
        })?,
    )?;
    m.set(
        "GetZoomLevels",
        // The reference's constant (`0x6da9a0`), with no `this` lookup (`0x4eebe0`).
        lua.create_function(|_, _: mlua::MultiValue| Ok(MINIMAP_ZOOM_LEVELS))?,
    )?;
    m.set(
        "PingLocation",
        // `0x4eeca0`: nothing without the player object (`0x4eecdf`). Arguments 2 and 3 count only
        // when both pass `lua_isnumber` (`0x4eed0b`, `0x4eed1f`), and only then is `this` looked
        // up (`0x4eed25`); otherwise both offsets stay 0 and the ping lands on the player. The
        // first is negated (`0x4eedeb`) and divided by the frame's given height (slot 0x20,
        // `0x4eee31`), the second by its width (slot 0x1c, `0x4eee21`): fractions of its own size,
        // whatever its scale. Each times the zoom diameter (`0x4eecf8`) offsets the player's point,
        // world X by the second and world Y by the first (`0x4eee48`), with no disc test. The
        // party send (`0x4eef72`) goes to the host; the ping then applies at once (`0x4eefb5`).
        lua.create_function(|lua, args: mlua::MultiValue| {
            let player = lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .minimap
                .player;
            let Some(player) = player else {
                return Ok(());
            };
            let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Nil);
            let (x, y) = (lua.coerce_number(arg(1))?, lua.coerce_number(arg(2))?);
            let (mut first, mut second) = (0.0_f64, 0.0_f64);
            if let (Some(x), Some(y)) = (x, y) {
                let h = this_minimap(lua, &arg(0))?;
                let (w, ht) = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model
                        .layout_inputs
                        .get(&h)
                        .map_or((0.0, 0.0), |i| (i.width, i.height))
                };
                first = -x / f64::from(ht);
                second = y / f64::from(w);
            }
            super::tick::verb(lua, |model| {
                let d = 2.0 * f64::from(model.minimap.radius_yd());
                let world = (
                    (f64::from(player.0) + second * d) as f32,
                    (f64::from(player.1) + first * d) as f32,
                );
                model.minimap_pings_sent.push(world);
                if let Some((a, b)) = model.minimap.apply_ping(world) {
                    model.raise("MINIMAP_PING", super::ping_args("player", a, b));
                }
            });
            Ok(())
        })?,
    )?;
    m.set(
        "GetPingPosition",
        // Two numbers recomputed from the stored point at every call, `(0, 0)` with no player
        // (`0x4eefd0`); the stock `Minimap_OnUpdate` multiplies them for 5 s, so a nil would error
        // every frame. No `this` lookup.
        lua.create_function(|lua, _: mlua::MultiValue| {
            Ok(lua
                .app_data_ref::<Model>()
                .expect("model app_data")
                .minimap
                .ping_position())
        })?,
    )?;
    lua.set_named_registry_value(REG_MINIMAP_METHODS, m)?;
    Ok(())
}

impl super::UiScript {
    /// The arrow half of the minimap render (`0x4ec950`), on every Minimap's children. When the
    /// arrow list was rebuilt (`0x6daa20` answered 1, `0x4ed2e2`), each of the three rim frames
    /// with a record shows, at the level base plus its record's slot (`0x4ed344`) and playing its
    /// art (`0x4ed353`), and the rest hide; with the indoor byte set only a record with `+0x44`
    /// shows (`0x4ed308`). Every frame each record's frame is aimed (`0x4ed404`). Each party and
    /// pet frame shows while its record has an arrow and hides otherwise (`0x4ed3c0`), and is
    /// aimed (`0x4ed4ba`) at its bearing in `group`; its `GroupArrow` was armed once, by the model
    /// load (`0x4ee1de`).
    pub fn set_minimap_arrows(
        &mut self,
        rim: &[MinimapRimArrow],
        rebuilt: bool,
        indoors: bool,
        group: &[Option<f32>; crate::widget::MINIMAP_GROUP_ARROWS],
    ) {
        let mut shows: Vec<(FrameHandle, bool)> = Vec::new();
        {
            let mut model = self.model_mut();
            let minimaps = model.arena.minimap_kinds().to_vec();
            for mm in minimaps {
                let Some(KindState::Minimap(state)) = model.arena.frame(mm).map(|f| &f.kind_state)
                else {
                    continue;
                };
                let state = state.clone();
                if rebuilt {
                    for (i, &h) in state.rim_arrows.iter().enumerate() {
                        let rec = rim.get(i).filter(|r| !indoors || r.indoors);
                        shows.push((h, rec.is_some()));
                        let Some(rec) = rec else { continue };
                        let level = (state.level_base + rec.slot).clamp(0, i32::from(u16::MAX));
                        model.arena.set_frame_level(h, level as u16, false);
                        let art = match rec.slot {
                            2 => CORPSE_ARROW,
                            -2 => LANDMARK_ARROW,
                            _ => GUIDE_ARROW,
                        };
                        arm(&mut model, h, art);
                    }
                }
                for (i, &h) in state.rim_arrows.iter().enumerate().take(rim.len()) {
                    place_arrow(&mut model, h, state.disc, rim[i].bearing);
                }
                for (i, &h) in state.group_arrows.iter().enumerate() {
                    shows.push((h, group[i].is_some()));
                    if let Some(bearing) = group[i] {
                        place_arrow(&mut model, h, state.disc, bearing);
                    }
                }
            }
        }
        for (h, shown) in shows {
            let now = self.model_ref().arena.frame(h).is_some_and(|f| f.shown);
            if now != shown {
                super::visibility::set_shown(&self.lua, h, shown);
            }
        }
    }
}

#[cfg(test)]
impl super::UiScript {
    /// Engine child `i` (1-based, `GetChildren` order) of the named Minimap: its model pane state.
    pub(crate) fn minimap_child(
        &self,
        minimap: &str,
        i: usize,
    ) -> Option<crate::widget::ModelState> {
        let model = self.model_ref();
        let h = model.arena.lookup(minimap)?;
        let child = *model.arena.frame(h)?.children.get(i.checked_sub(1)?)?;
        match model.arena.frame(child).map(|f| &f.kind_state) {
            Some(KindState::Model(m)) => Some(m.clone()),
            _ => None,
        }
    }
}
