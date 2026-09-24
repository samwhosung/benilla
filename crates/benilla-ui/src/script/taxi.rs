//! The taxi-map bindings (decision 0484) — the Era-shaped flight-master surface driving a faithful
//! port of the real 1.12 `TaxiFrame` (extracted from the patch chain:
//! `Interface\FrameXML\TaxiFrame.{xml,lua}`). Same two-way seam as [`super::trainer`]: the app
//! pushes a **taxi snapshot** ([`UiScript::set_taxi`] — the known-node mask, DBC positions, route
//! and cost computation already resolved app-side to a flat node list), and the Lua
//! `TakeTaxiNode`/`CloseTaxiMap` calls queue outbound **intents** the app drains
//! ([`UiScript::take_taxi_node`] / [`UiScript::take_taxi_close`]). The engine holds no taxi
//! knowledge — a node is name/type/position/cost/route-segments, all app-resolved.
//!
//! ## The Era API shape (matched to the real `TaxiFrame.lua`)
//!
//! The reference window's `.lua` runs verbatim on these bindings: `NumTaxiNodes`,
//! `TaxiNodeGetType(i) → "CURRENT"/"REACHABLE"/"DISTANT"/"NONE"`, `TaxiNodePosition(i) → x, y`
//! (normalized 0..1, **BOTTOMLEFT** origin — the Lua multiplies by the 316×352 map size and anchors
//! from the map's BOTTOMLEFT), `TaxiNodeName(i)`, `TaxiNodeCost(i)` (copper),
//! `GetNumRoutes(i)` + `TaxiGetSrcX/Y(i, hop)`/`TaxiGetDestX/Y(i, hop)` (the hover route's
//! per-hop segment endpoints, same normalized space), `TakeTaxiNode(i)`, `CloseTaxiMap()`,
//! `SetTaxiMap(texture)` (assigns the continent art onto the given Texture region),
//! `TaxiNodeSetCurrent(i)` (a no-op here: the real client computes the hovered node's route arrays
//! behind it; the app precomputes every node's route, so there is nothing to arm), and
//! `UnitOnTaxi(unit)`. Indices are 1-based into the pushed node list.
//!
//! Node classification, projection, and the route metric are the app's job — and are INTERIM
//! behind decision 0484's in-flight §5 (I1/I2); this seam only carries the results.

use mlua::{Lua, Table};

use super::region::region_handle_of;
use super::Model;

/// A visible node's icon state (`TaxiNodeGetType`) — the reference maps these to the green
/// (current) / white (reachable) / yellow (distant) icons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TaxiNodeType {
    /// The flight master's own node ("You are here" — green).
    Current,
    /// Known, and a route from the current node exists (white; clickable).
    #[default]
    Reachable,
    /// Known, but no route connects it to the current node (yellow). **Never produced**: the real
    /// client's DISTANT classification is a dead branch (byte-verified — decision 0496 §TU-3; an
    /// unroutable node simply doesn't render), so the app drops such nodes instead. The variant
    /// stays because the reference Lua's `TaxiButtonTypes` table names it — a faithful surface
    /// with no live writer.
    Distant,
}

impl TaxiNodeType {
    /// The Era type string (`TaxiFrame.lua`'s `TaxiButtonTypes` keys).
    fn era_str(self) -> &'static str {
        match self {
            TaxiNodeType::Current => "CURRENT",
            TaxiNodeType::Reachable => "REACHABLE",
            TaxiNodeType::Distant => "DISTANT",
        }
    }
}

/// One node on the open taxi map — everything the window shows for it, app-resolved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaxiUiNode {
    /// `TaxiNodeName` (the DBC's localized name, e.g. `"Stormwind, Elwynn"`).
    pub name: String,
    /// `TaxiNodeGetType`. A node the reference would type `"NONE"` (hidden) is simply not pushed.
    pub node_type: TaxiNodeType,
    /// `TaxiNodePosition` — normalized `(x, y)` on the map art, 0..1, BOTTOMLEFT origin.
    pub pos: (f32, f32),
    /// `TaxiNodeCost` — the route's fare in copper (0 for the current node / an unroutable one).
    pub cost: u32,
    /// The hover route's per-hop segments, `[src_x, src_y, dest_x, dest_y]` each, in the same
    /// normalized space (`GetNumRoutes` / `TaxiGetSrcX…DestY`). Empty for current/distant nodes.
    pub routes: Vec<[f32; 4]>,
}

/// The open taxi map: the continent art and the visible nodes. Pushed whole by the app; `None`
/// means no taxi map is open.
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

    /// Whether our own player is currently riding a taxi (`UnitOnTaxi("player")` — the action-bar
    /// dim and the reference's flight-state checks read it).
    pub fn set_on_taxi(&mut self, riding: bool) {
        self.model_mut().taxi_riding = riding;
    }

    /// Drain the **1-based node indices** `TakeTaxiNode` queued since the last call (the app maps
    /// each back to its node id and sends the activate packet).
    pub fn take_taxi_node(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.model_mut().taxi_takes)
    }

    /// Whether `CloseTaxiMap` was called since the last drain (and clear the flag). The window
    /// closed client-side; the app clears its taxi state (no packet — the server holds no
    /// open-window session for the map).
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

/// Register the taxi globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // → how many nodes the open map shows (0 when closed).
    g.set(
        "NumTaxiNodes",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(model.taxi.as_ref().map_or(0, |t| t.nodes.len()) as i64)
        })?,
    )?;

    // → the node's icon-state string; "NONE" out of range (the reference hides that button).
    g.set(
        "TaxiNodeGetType",
        lua.create_function(|lua, i: i64| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(node(&model, i).map_or("NONE", |n| n.node_type.era_str()))
        })?,
    )?;

    // → the node's normalized map position (x, y; BOTTOMLEFT origin).
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

    // → the fare to fly there, in copper (`SetTooltipMoney` renders it).
    g.set(
        "TaxiNodeCost",
        lua.create_function(|lua, i: i64| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(node(&model, i).map_or(0, |n| n.cost) as i64)
        })?,
    )?;

    // The real client arms the hovered node's route computation here; the app pushes every node's
    // route precomputed, so this is a faithful no-op (kept so the reference Lua runs verbatim).
    g.set(
        "TaxiNodeSetCurrent",
        lua.create_function(|_, _: i64| Ok(()))?,
    )?;

    // → the hover route's hop count for node `i`.
    g.set(
        "GetNumRoutes",
        lua.create_function(|lua, i: i64| {
            let model = lua.app_data_ref::<Model>().expect("model");
            Ok(node(&model, i).map_or(0, |n| n.routes.len()) as i64)
        })?,
    )?;

    // → one endpoint coordinate of route hop `hop` (1-based) of node `i` — the four accessors the
    // reference's `DrawRouteLine` loop reads.
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

    // Queue the click — the app maps the index to a node id and sends the activate packet.
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

    // SetTaxiMap(textureRegion) — assign the open map's continent art onto the given Texture
    // region (the engine picked `TAXIMAP<map>`, app-side; the region draws it like any SetTexture).
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

    // `UnitOnTaxi(unit)` (`0x517a40`) → **the number 1 or nil**, never a Lua boolean — the unit
    // predicate family's one return shape (decisions 1830, 2043; the push law is
    // `crate::script::binding_abi::flag`). It lives here rather than beside its 22 siblings
    // because the ride flag it reads is this module's, not `UnitState`'s — which is exactly how it
    // came to be the one predicate still answering a `bool` after the family was fixed.
    //
    // Only our own player is tracked (the reference UI only ever asks about `"player"`); any other
    // token reads nil.
    //
    // The token argument is `Value`, not `String`, so the miss is the reference's own message: the
    // binding carries an `lua_isstring` gate at `0x517a48` whose failure arm is
    // `luaL_error("Usage: UnitOnTaxi(\"unit\")")`. A Rust `String` parameter raised mlua's
    // own type-conversion error there instead.
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
