//! The taxi-map bindings the stock `TaxiFrame.lua` runs on, over a node list the app resolves
//! and pushes (types, positions, fares and routes); `TakeTaxiNode` and `CloseTaxiMap` queue
//! intents the app drains. Positions are normalized 0..1 from the map's bottom-left, which the Lua
//! scales by the 316x352 map.

use mlua::{Lua, Table};

use super::region::region_handle_of;
use super::Model;

/// A visible node's icon state (`TaxiNodeGetType`): green, white or yellow in the reference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TaxiNodeType {
    /// The flight master's own node, "You are here".
    Current,
    /// Known and routable from the current node; clickable.
    #[default]
    Reachable,
    /// Known but unroutable. Never produced: the reference's DISTANT branch is dead and an
    /// unroutable node does not render; the variant stays because `TaxiButtonTypes` names it.
    Distant,
}

impl TaxiNodeType {
    /// The type string, a `TaxiButtonTypes` key.
    fn era_str(self) -> &'static str {
        match self {
            TaxiNodeType::Current => "CURRENT",
            TaxiNodeType::Reachable => "REACHABLE",
            TaxiNodeType::Distant => "DISTANT",
        }
    }
}

/// One node on the open taxi map, resolved by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaxiUiNode {
    /// `TaxiNodeName`, the DBC's localized name.
    pub name: String,
    /// `TaxiNodeGetType`; a node the reference types `"NONE"` is not pushed.
    pub node_type: TaxiNodeType,
    /// `TaxiNodePosition`: normalized `(x, y)` on the map art, from the bottom-left.
    pub pos: (f32, f32),
    /// `TaxiNodeCost`: the fare in copper, 0 for the current node.
    pub cost: u32,
    /// The hover route's hops as `[src_x, src_y, dest_x, dest_y]` in the same space, for
    /// `GetNumRoutes` and `TaxiGetSrcX` to `TaxiGetDestY`; empty for the current node.
    pub routes: Vec<[f32; 4]>,
}

/// The open taxi map: the continent art and the visible nodes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaxiUiState {
    /// The map art `SetTaxiMap` assigns (e.g. `Interface\TaxiFrame\TAXIMAP1`).
    pub art: String,
    /// The visible nodes, in the app's order (Lua indexes them 1-based).
    pub nodes: Vec<TaxiUiNode>,
}

impl super::UiScript {
    /// Push (or clear, with `None`) the open taxi map's snapshot.
    pub fn set_taxi(&mut self, state: Option<TaxiUiState>) {
        self.model_mut().taxi = state;
    }

    /// Whether the player is riding a taxi, for `UnitOnTaxi("player")`.
    pub fn set_on_taxi(&mut self, riding: bool) {
        self.model_mut().taxi_riding = riding;
    }

    /// Drain the 1-based indices `TakeTaxiNode` queued; the app sends the activate for each.
    pub fn take_taxi_node(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.model_mut().taxi_takes)
    }

    /// Whether `CloseTaxiMap` was called since the last drain; it sends no packet, as the server
    /// holds no window session for the map.
    pub fn take_taxi_close(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().taxi_close)
    }
}

/// A node by 1-based Lua index.
fn node(model: &Model, i: i64) -> Option<&TaxiUiNode> {
    let nodes = &model.taxi.as_ref()?.nodes;
    usize::try_from(i)
        .ok()?
        .checked_sub(1)
        .and_then(|i| nodes.get(i))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "NumTaxiNodes",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.taxi.as_ref().map_or(0, |t| t.nodes.len()) as i64)
        })?,
    )?;

    // "NONE" out of range, a button the reference hides.
    g.set(
        "TaxiNodeGetType",
        lua.create_function(|lua, i: i64| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(node(&model, i).map_or("NONE", |n| n.node_type.era_str()))
        })?,
    )?;

    g.set(
        "TaxiNodePosition",
        lua.create_function(|lua, i: i64| {
            let model = lua.app_data_ref::<Model>().expect("model");
            let (x, y) = node(&model, i).map_or((0.0, 0.0), |n| n.pos);
            Ok((x, y))
        })?,
    )?;

    g.set(
        "TaxiNodeName",
        lua.create_function(|lua, i: i64| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(node(&model, i).map_or(String::new(), |n| n.name.clone()))
        })?,
    )?;

    g.set(
        "TaxiNodeCost",
        lua.create_function(|lua, i: i64| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(node(&model, i).map_or(0, |n| n.cost) as i64)
        })?,
    )?;

    // The reference computes the hovered node's route here; the app precomputes every route.
    g.set(
        "TaxiNodeSetCurrent",
        lua.create_function(|_, _: i64| Ok(()))?,
    )?;

    g.set(
        "GetNumRoutes",
        lua.create_function(|lua, i: i64| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(node(&model, i).map_or(0, |n| n.routes.len()) as i64)
        })?,
    )?;

    // One coordinate of 1-based hop `hop`, for the reference's `DrawRouteLine` loop.
    for (name, pick) in [
        ("TaxiGetSrcX", 0usize),
        ("TaxiGetSrcY", 1),
        ("TaxiGetDestX", 2),
        ("TaxiGetDestY", 3),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, (i, hop): (i64, i64)| {
                let model = lua.app_data_ref::<Model>().expect("model");
                let coord = node(&model, i)
                    .zip(usize::try_from(hop).ok().and_then(|h| h.checked_sub(1)))
                    .and_then(|(n, h)| n.routes.get(h))
                    .map_or(0.0, |seg| seg[pick]);
                Ok(coord)
            })?,
        )?;
    }

    g.set(
        "TakeTaxiNode",
        lua.create_function(|lua, i: i64| {
            if let Ok(i) = usize::try_from(i) {
                lua.app_data_mut::<Model>()
                    .expect("model")
                    .taxi_takes
                    .push(i);
            }
            Ok(())
        })?,
    )?;

    g.set(
        "CloseTaxiMap",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>().expect("model").taxi_close = true;
            Ok(())
        })?,
    )?;

    // The open map's continent art (`TAXIMAP<map>`, the app's pick) onto the given texture.
    g.set(
        "SetTaxiMap",
        lua.create_function(|lua, this: Table| {
            let rh = region_handle_of(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            let art = model.taxi.as_ref().map(|t| t.art.clone());
            if let Some(art) = art {
                model.region_data.entry(rh).or_default().texture = Some(art);
            }
            Ok(())
        })?,
    )?;

    // `0x517a40`: the number 1 or nil, never a boolean; an argument neither string nor number
    // raises the reference's usage (`0x517a48`). Only the player is tracked, as the stock UI asks
    // about no other unit.
    g.set(
        "UnitOnTaxi",
        lua.create_function(|lua, unit: mlua::Value| {
            let unit =
                crate::script::binding_abi::string_arg(lua, unit, r#"Usage: UnitOnTaxi("unit")"#)?;
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(crate::script::binding_abi::flag(
                unit.eq_ignore_ascii_case("player") && model.taxi_riding,
            ))
        })?,
    )?;

    Ok(())
}
