//! The dressing room: the `DressUpModel` verbs (table `0x84f190`) and `SetUnit`/`RefreshUnit` on a
//! dressing-room pane, each handed to the host, which applies it to the room inside the call
//! ([`super::ScriptCall::DressUp`]). The reference edits a clone of the unit's model (`0x5059a0`,
//! `0x504350`); here the app composes the look from the verbs in call order, which matters: on a
//! closed window `DressUpItem` resets, then tries on (`DressUpFrame.lua:3-7`).

use mlua::{Lua, Table, Value};

use super::binding_abi;
use super::object::frame_handle_of;
use super::Model;
use crate::widget::FrameKind;

/// Registry key of the `DressUpModel` method table, its own three; the rest come by the chain.
pub(super) const REG_DRESSUPMODEL_METHODS: &str = "__benilla_dressupmodel_methods";

/// One dressing-room verb, applied inside its call.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DressUpIntent {
    /// `SetUnit`, `RefreshUnit` and `Dress` (the Reset button): the reference's rebuild from the
    /// unit (`0x505b50`), dropping every substitution.
    Dress,
    /// `Undress()`: component slots `0..0xb` cleared and both held lanes detached and zeroed, worn
    /// and tried-on alike (`0x504490`).
    Undress,
    /// `TryOn(item)`: the item in its `InventoryType`'s slot, a held one in the lane `0x5048e8`
    /// picks, with no class, level or proficiency check (`DressUpFrame.lua:2-16`).
    TryOn(u32),
}

impl super::UiScript {
    /// The dressing-room verbs a harness's VM kept ([`Self::keep_calls`]), oldest first.
    pub fn take_dressup_intents(&mut self) -> Vec<DressUpIntent> {
        self.take_calls_where(|call| match call {
            super::ScriptCall::DressUp(intent) => Some(*intent),
            _ => None,
        })
    }
}

/// Hand `intent` to the host, which applies it before this returns.
fn submit(lua: &Lua, intent: DressUpIntent) -> mlua::Result<()> {
    super::calls::submit(lua, super::ScriptCall::DressUp(intent))
}

/// The rebuild when `PlayerModel`'s `SetUnit`/`RefreshUnit` lands on a `DressUpModel`.
pub(super) fn redress_if_dressup(lua: &Lua, this: &Table) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let is_dressup = lua
        .app_data_ref::<Model>()
        .expect("model app_data")
        .arena
        .frame(h)
        .is_some_and(|f| f.kind == FrameKind::DressUpModel);
    if is_dressup {
        submit(lua, DressUpIntent::Dress)?;
    }
    Ok(())
}

/// `trunc(tonumber(arg))`, the reference's `__ftol` (`0x40a2b0`) over `lua_tonumber`; anything
/// else is 0.
fn item_arg(lua: &Lua, v: &Value) -> i64 {
    i64::from(binding_abi::ftol(binding_abi::coerced_number(
        lua,
        Some(v.clone()),
    )))
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let m = lua.create_table()?;

    // `0x504c00`.
    m.set(
        "Undress",
        lua.create_function(|lua, this: Table| {
            frame_handle_of(lua, &this)?;
            submit(lua, DressUpIntent::Undress)
        })?,
    )?;

    // `0x504cd0`, the Reset button (`DressUpFrame.xml:182`).
    m.set(
        "Dress",
        lua.create_function(|lua, this: Table| {
            frame_handle_of(lua, &this)?;
            submit(lua, DressUpIntent::Dress)
        })?,
    )?;

    // `0x504d90` calls `0x504540(itemId)`; item 0 (a non-number or a link) previews nothing, and
    // so does a negative id here.
    m.set(
        "TryOn",
        lua.create_function(|lua, (this, item): (Table, Value)| {
            frame_handle_of(lua, &this)?;
            if let Ok(id) = u32::try_from(item_arg(lua, &item)) {
                if id != 0 {
                    submit(lua, DressUpIntent::TryOn(id))?;
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
        s.keep_calls();
        s.set_screen_size(800.0, 600.0);
        s.run(r#"dm = CreateFrame("DressUpModel", "DM", UIParent)"#)
            .unwrap();
        s
    }

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

    /// The stock `DressUpItemLink` passes the link's digits: a digit string is an id, a link is 0.
    #[test]
    fn the_verbs_hand_their_calls_over_in_order_and_try_on_reads_its_argument_like_the_client() {
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
        // The same two PlayerModel verbs on a paper-doll pane make no call.
        s.run(r#"pm = CreateFrame("PlayerModel", "PMOnly", UIParent) pm:SetUnit("player") pm:RefreshUnit()"#)
            .unwrap();
        assert!(s.take_dressup_intents().is_empty());
        // Its yaw is its own facing, read by name like every other pane's.
        s.run("DM:SetRotation(0.61)").unwrap();
        assert!((s.model_pane_facing("DM") - 0.61).abs() < 1e-6);
    }
}
