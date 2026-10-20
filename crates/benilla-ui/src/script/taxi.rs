//! The taxi-map bindings the stock `TaxiFrame.lua` runs on, over a node list the app resolves
//! and pushes (types, positions, fares and routes); `TakeTaxiNode` and `CloseTaxiMap` queue
//! intents the app drains. Positions are normalized 0..1 from the map's bottom-left, which the Lua
//! scales by the 316x352 map.

use mlua::{Lua, Value};

use super::binding_abi;
use super::Model;

/// A listed node's type (`TaxiNodeGetType`, `0x4dc4e0`): the green, white or yellow icon in the
/// reference, or no button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TaxiNodeType {
    /// The flight master's own node, "You are here".
    Current,
    /// Known and routed from the current node; clickable.
    #[default]
    Reachable,
    /// Known with its bit clear in the stored mask. Never produced: every listed node's bit is set
    /// (`0x4dc545`); the variant stays because `TaxiButtonTypes` names it.
    Distant,
    /// `NONE`: listed, but the route search never reached it (`0x4dc512`); stock `TaxiFrame.lua`
    /// hides its button.
    Unrouted,
}

impl TaxiNodeType {
    /// The type string, a `TaxiButtonTypes` key, or `NONE`.
    fn era_str(self) -> &'static str {
        match self {
            TaxiNodeType::Current => "CURRENT",
            TaxiNodeType::Reachable => "REACHABLE",
            TaxiNodeType::Distant => "DISTANT",
            TaxiNodeType::Unrouted => "NONE",
        }
    }
}

/// One node on the open taxi map, resolved by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaxiUiNode {
    /// `TaxiNodeName`, the DBC's localized name.
    pub name: String,
    /// `TaxiNodeGetType`.
    pub node_type: TaxiNodeType,
    /// `TaxiNodePosition`: normalized `(x, y)` on the map art, from the bottom-left.
    pub pos: (f32, f32),
    /// `TaxiNodeCost`: the fare in copper, 0 for the current node.
    pub cost: u32,
    /// The hover route's lines as `[src_x, src_y, dest_x, dest_y]` in the same space, for
    /// `TaxiGetSrcX` to `TaxiGetDestY` (`0x4dc5c0`); empty for the current node and an unrouted
    /// one.
    pub routes: Vec<[f32; 4]>,
    /// `GetNumRoutes` (`0x4dc170`): 1 with a direct path from the current node, else the route's
    /// node count less 1, so 0 for the current node and 999998 for one never routed.
    pub num_routes: u32,
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

    /// The close (`0x4dc0f0`) clears the flight master's cell but keeps the slot count
    /// (`0xbb4aa8`): the nodes, their types, routes and lines stay, and every fare reads 0 with no
    /// flight master (`0x4dc423`).
    pub fn close_taxi(&mut self) {
        if let Some(taxi) = &mut self.model_mut().taxi {
            for node in &mut taxi.nodes {
                node.cost = 0;
            }
        }
    }

    /// An open's reset before its build: no slot (`0x4db748`) and no line (`0x4db793`-`0x4db7d6`),
    /// the map art kept (`0xbb4a78`).
    pub fn reset_taxi_slots(&mut self) {
        if let Some(taxi) = &mut self.model_mut().taxi {
            taxi.nodes.clear();
        }
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

/// What `TaxiNodePosition` and `TaxiNodeCost` raise for a slot the map lacks (`0x84b008`).
const INVALID_SLOT: &str = "Invalid taxi node slot";

/// A node by 1-based Lua index: the reference compares the index less 1, unsigned, with the count
/// (`0x4dc380`, `0x4dcbc5`, `0x4dcc63`), so 0, a negative index and an empty list miss.
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
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: TaxiNodeGetType(slot)` (`0x84b080`, `0x4dcd01`).
            let i = i64::from(binding_abi::number_arg(
                lua,
                i,
                "Usage: TaxiNodeGetType(slot)",
            )?);
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(node(&model, i).map_or("NONE", |n| n.node_type.era_str()))
        })?,
    )?;

    g.set(
        "TaxiNodePosition",
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: TaxiNodeTaxiNodeLocation(slot)` (`0x84b020`, `0x4dcb97`).
            let i = i64::from(binding_abi::number_arg(
                lua,
                i,
                "Usage: TaxiNodeTaxiNodeLocation(slot)",
            )?);
            let model = lua.app_data_ref::<Model>().expect("model");
            let n =
                node(&model, i).ok_or_else(|| mlua::Error::RuntimeError(INVALID_SLOT.into()))?;
            Ok(n.pos)
        })?,
    )?;

    g.set(
        "TaxiNodeName",
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: TaxiNodeName(slot)` (`0x84afec`, `0x4dcb41`).
            let i = i64::from(binding_abi::number_arg(
                lua,
                i,
                "Usage: TaxiNodeName(slot)",
            )?);
            let model = lua.app_data_ref::<Model>().expect("model");
            // `INVALID` (`0x84afc8`) out of range (`0x4dc388`).
            Ok(node(&model, i).map_or_else(|| "INVALID".to_owned(), |n| n.name.clone()))
        })?,
    )?;

    g.set(
        "TaxiNodeCost",
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: TaxiNodeCost(slot)` (`0x84b048`, `0x4dcc35`).
            let i = i64::from(binding_abi::number_arg(
                lua,
                i,
                "Usage: TaxiNodeCost(slot)",
            )?);
            let model = lua.app_data_ref::<Model>().expect("model");
            let n =
                node(&model, i).ok_or_else(|| mlua::Error::RuntimeError(INVALID_SLOT.into()))?;
            Ok(i64::from(n.cost))
        })?,
    )?;

    // The reference computes the hovered node's route here; the app precomputes every route.
    // Deviation: returns nothing. The handler ends `mov eax,1` with nothing pushed (`0x4dcd7c`), so
    // Lua 5.0 hands back the stack top, the node argument: an accident of the handler, not an API,
    // and copying a stack-top leak would copy a bug.
    g.set(
        "TaxiNodeSetCurrent",
        lua.create_function(|lua, slot: Value| {
            // `is-number` else `Usage: TaxiNodeSetCurrent(slot)` (`0x84b0a0`, `0x4dcd51`).
            binding_abi::number_arg(lua, slot, "Usage: TaxiNodeSetCurrent(slot)")?;
            Ok(())
        })?,
    )?;

    g.set(
        "GetNumRoutes",
        // A bare `lua_tonumber` (`0x4dcede`) with no gate: a non-number reads 0, which `0x4dc170`
        // (index - 1 as unsigned) answers 0 for, as for any out-of-range node.
        lua.create_function(|lua, i: Value| {
            let i = i64::from(binding_abi::ftol(binding_abi::coerced_number(lua, Some(i))));
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(node(&model, i).map_or(0, |n| i64::from(n.num_routes)))
        })?,
    )?;

    // One coordinate of 1-based segment `hop` of slot `i`, for the stock `DrawRouteLine` loop: both
    // arguments a bare `lua_tonumber` and `_ftol`, the segment's read first (`0x4dcd9d`-`0x4dcdbb`),
    // so a non-number is 0 and misses, as any slot or segment out of range answers 0 (`0x4dca3a`).
    for (name, pick) in [
        ("TaxiGetSrcX", 0usize),
        ("TaxiGetSrcY", 1),
        ("TaxiGetDestX", 2),
        ("TaxiGetDestY", 3),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, (i, hop): (Value, Value)| {
                let index =
                    |v| i64::from(binding_abi::ftol(binding_abi::coerced_number(lua, Some(v))));
                let (hop, i) = (index(hop), index(i));
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
        lua.create_function(|lua, i: Value| {
            // `is-number` else `Usage: TakeTaxiNode(slot)` (`0x84b064`, `0x4dccb1`).
            let i = i64::from(binding_abi::number_arg(
                lua,
                i,
                "Usage: TakeTaxiNode(slot)",
            )?);
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

    // The map's continent art (`TAXIMAP<map>`, the app's pick) onto the given texture. With no
    // art loaded it returns before reading its argument (`0x4dca5c`); else the argument resolves
    // as a member function's receiver, a Texture (`0x4dca8b`-`0x4dcad9`).
    g.set(
        "SetTaxiMap",
        lua.create_function(|lua, this: Value| {
            let art = {
                let model = lua.app_data_ref::<Model>().expect("model");
                model.taxi.as_ref().map(|t| t.art.clone())
            };
            let Some(art) = art else {
                return Ok(());
            };
            let rh = super::region::this_texture(lua, &this)?;
            let mut model = lua.app_data_mut::<Model>().expect("model");
            model.set_texel(rh, crate::script::texel::TexSet::File(art));
            model.note_art(rh);
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

#[cfg(test)]
mod tests {
    use crate::script::UiScript;

    /// Deviation pin: the reference hands back its stack top (`0x4dcd7c`); `TaxiNodeSetCurrent`
    /// returns nothing, whatever the node.
    #[test]
    fn taxi_node_set_current_returns_no_values() {
        let s = UiScript::new().unwrap();
        for node in ["1", "0", "99"] {
            assert_eq!(
                s.eval::<i64>(&format!(
                    "return (function(...) return arg.n end)(TaxiNodeSetCurrent({node}))"
                ))
                .unwrap(),
                0,
                "node {node}"
            );
        }
    }
}
