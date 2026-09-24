//! The **dressing room** surface (decisions 1060, 1969) — the `DressUpModel` widget's own three
//! verbs (table `0x84f190`: `Undress 0x504c00` · `Dress 0x504cd0` · `TryOn 0x504d90`) and the
//! ordered intent queue behind them and behind `PlayerModel`'s `SetUnit`/`RefreshUnit` when the
//! pane is a dressing room.
//!
//! ## Why a queue, and why the state is the app's
//!
//! In the client the widget clones the unit's live model on `SetUnit`/`Dress` (`0x5059a0`, the
//! attachment tree deep-copied) and `TryOn` overwrites one bodyslot or one of two hand lanes of
//! that clone (`0x504350`). benilla renders no FrameXML model:
//! every model pane is a booth bake the app composes from a look — the player's own visible items
//! with the tried-on ones substituted in — and the VM holds neither item templates nor the
//! player's equipment. So the verbs record *intents*, in order, and the app applies them
//! (`take_dressup_intents`): `DressUpItem` resets *then* tries on in one breath when the window
//! was closed (ref `DressUpFrame.lua:3-7`), and applying those two out of order would show the
//! player's own gear instead of the item they clicked.
//!
//! ## What each verb means, off the bytes
//!
//! - `SetUnit(unit)` / `RefreshUnit()` / `Dress()` all funnel into the same rebuild-from-the-unit
//!   worker (`0x505b50`): every substitution is gone and the model is what the player shows in
//!   the world. One intent, [`DressUpIntent::Dress`].
//! - `Undress()` → `0x504490`: clears components bodyslots `0..0xb` — every worn piece, base and
//!   tried-on alike — and touches no hand lane, so a held weapon stays. [`DressUpIntent::Undress`].
//! - `TryOn(item)`: the argument is `trunc(tonumber(arg))` (`__ftol 0x40a2b0`) handed to the item
//!   cache, so a numeric string is an id and anything else is item 0, which previews nothing. The
//!   stock `DressUpItemLink` hands it the digits it `gsub`bed out of the `|Hitem:` link. It gates
//!   on nothing — no class, level or proficiency check anywhere in the path (`DressUpFrame.lua:2-16`).

use mlua::{Lua, Table, Value};

use super::object::frame_handle_of;
use super::Model;
use crate::widget::FrameKind;

/// Registry key of the `DressUpModel` method table — its **own three** entries. `PlayerModel`'s
/// three and `Model`'s 23 come through the dispatcher's chain (`object.rs`).
pub(super) const REG_DRESSUPMODEL_METHODS: &str = "__benilla_dressupmodel_methods";

/// One queued dressing-room intent (see the module doc on ordering).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DressUpIntent {
    /// `DressUpModel:SetUnit("player")` (the open), `RefreshUnit()` and `Dress()` (the Reset
    /// button) — "wear what the player is actually wearing", dropping every substitution.
    Dress,
    /// `DressUpModel:Undress()` — every worn piece off, base and tried-on; the hands keep what
    /// they hold.
    Undress,
    /// `DressUpModel:TryOn(item)` — substitute this item id into whichever slot its
    /// `InventoryType` belongs to.
    TryOn(u32),
    /// The window was hidden — the app's own intent, derived from the frame going invisible
    /// (`UiScript::frame_visible`): there is nothing to show, so the booth empties (and stops
    /// rendering). The reference's widget keeps its state while hidden, but its next `DressUpItem`
    /// re-issues `SetUnit("player")` precisely *because* the frame was not visible, so the state it
    /// kept is never observable — dropping it is behaviour-identical and saves the bake.
    Close,
}

impl super::UiScript {
    /// Drain the dressing room's queued intents, oldest first — the app applies them in order.
    pub fn take_dressup_intents(&mut self) -> Vec<DressUpIntent> {
        std::mem::take(&mut self.model_mut().dressup_intents)
    }
}

fn queue(lua: &Lua, intent: DressUpIntent) {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .dressup_intents
        .push(intent);
}

/// `PlayerModel`'s `SetUnit`/`RefreshUnit` on a pane that is a `DressUpModel`: the rebuild that
/// drops every substitution (the module doc). A no-op on any other pane.
pub(super) fn redress_if_dressup(lua: &Lua, this: &Table) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let is_dressup = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == FrameKind::DressUpModel);
    if is_dressup {
        queue(lua, DressUpIntent::Dress);
    }
    Ok(())
}

/// `trunc(tonumber(arg))` — `__ftol 0x40a2b0` over `lua_tonumber`: a number or numeric string
/// truncates toward zero; anything else is 0.
fn item_arg(v: &Value) -> i64 {
    match v {
        Value::Integer(i) => *i,
        Value::Number(n) => n.trunc() as i64,
        Value::String(s) => s
            .to_str()
            .ok()
            .and_then(|t| t.trim().parse::<f64>().ok())
            .map_or(0, |n| n.trunc() as i64),
        _ => 0,
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // `Undress()` — `0x84f190` -> `0x504c00` -> `0x504490`.
    m.set(
        "Undress",
        lua.create_function(|lua, this: Table| {
            frame_handle_of(lua, &this)?;
            queue(lua, DressUpIntent::Undress);
            Ok(())
        })?,
    )?;

    // `Dress()` — `0x504cd0`, the Reset button (`DressUpFrame.xml:182`).
    m.set(
        "Dress",
        lua.create_function(|lua, this: Table| {
            frame_handle_of(lua, &this)?;
            queue(lua, DressUpIntent::Dress);
            Ok(())
        })?,
    )?;

    // `TryOn(item)` — `0x504d90` -> `0x504540(itemId)`. Item 0 (a non-number, a link string) looks
    // up nothing in the client's cache and previews nothing; so does a negative one here.
    m.set(
        "TryOn",
        lua.create_function(|lua, (this, item): (Table, Value)| {
            frame_handle_of(lua, &this)?;
            if let Ok(id) = u32::try_from(item_arg(&item)) {
                if id != 0 {
                    queue(lua, DressUpIntent::TryOn(id));
                }
            }
            Ok(())
        })?,
    )?;

    lua.set_named_registry_value(REG_DRESSUPMODEL_METHODS, m)
}

#[cfg(test)]
mod tests {
    use super::DressUpIntent;
    use crate::script::UiScript;

    fn room() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(800.0, 600.0);
        s.run(r#"dm = CreateFrame("DressUpModel", "DM", UIParent)"#)
            .unwrap();
        s
    }

    /// The widget answers its own three, PlayerModel's three and Model's, in a chain five deep —
    /// and a plain PlayerModel does not answer the three (the chain runs derived → base only).
    #[test]
    fn a_dress_up_model_is_a_player_model_plus_three_and_the_chain_runs_one_way() {
        let s = room();
        s.run(r#"pm = CreateFrame("PlayerModel", "PMOnly", UIParent)"#)
            .unwrap();
        for verb in [
            "TryOn",
            "Dress",
            "Undress",
            "SetUnit",
            "RefreshUnit",
            "SetRotation",
            "SetCamera",
        ] {
            assert_eq!(
                s.eval::<String>(&format!("return type(DM.{verb})"))
                    .unwrap(),
                "function",
                "DressUpModel answers {verb}"
            );
        }
        for verb in ["TryOn", "Dress", "Undress"] {
            assert_eq!(
                s.eval::<String>(&format!("return type(PMOnly.{verb})"))
                    .unwrap(),
                "nil",
                "a PlayerModel must NOT answer {verb}"
            );
        }
        assert_eq!(
            s.eval::<String>("return DM:GetObjectType()").unwrap(),
            "DressUpModel"
        );
        for t in ["DressUpModel", "PlayerModel", "Model", "Frame", "Region"] {
            assert!(
                s.eval::<bool>(&format!("return DM:IsObjectType(\"{t}\") == 1"))
                    .unwrap(),
                "IsObjectType({t})"
            );
        }
    }

    /// The verbs queue intents in call order; `SetUnit`/`RefreshUnit` re-dress on THIS kind only;
    /// `TryOn`'s argument is `trunc(tonumber(arg))`, so the stock file's digit string is an id and
    /// a link string is item 0, which queues nothing.
    #[test]
    fn the_verbs_queue_intents_in_order_and_try_on_reads_its_argument_like_the_client() {
        let mut s = room();
        s.run(
            r#"
            DM:SetUnit("player")
            DM:TryOn("117")
            DM:TryOn(1234.7)
            DM:Undress()
            DM:RefreshUnit()
            DM:Dress()
            DM:TryOn("|cffffffff|Hitem:117|h[Tough Jerky]|h|r")
            DM:TryOn(0)
            DM:TryOn(nil)
            DM:TryOn(-5)
            "#,
        )
        .unwrap();
        assert_eq!(
            s.take_dressup_intents(),
            vec![
                DressUpIntent::Dress,
                DressUpIntent::TryOn(117),
                DressUpIntent::TryOn(1234),
                DressUpIntent::Undress,
                DressUpIntent::Dress,
                DressUpIntent::Dress,
            ]
        );
        // The same two PlayerModel verbs on a paper-doll pane queue nothing.
        s.run(r#"pm = CreateFrame("PlayerModel", "PMOnly", UIParent) pm:SetUnit("player") pm:RefreshUnit()"#)
            .unwrap();
        assert!(s.take_dressup_intents().is_empty());
        // Its yaw is its own facing, read by name like every other pane's.
        s.run("DM:SetRotation(0.61)").unwrap();
        assert!((s.model_pane_facing("DM") - 0.61).abs() < 1e-6);
    }
}
