//! The cursor payload: the client's payload-mode global `[0xb4d900]` as a typed enum, and the drag
//! gestures ([`drag`]) that change it. Bags, the paper doll ([`doll`]), the action bars ([`bar`])
//! and the spellbook all transition it through [`queue_cursor_update`] and [`queue_lock_changed`],
//! so sounds, `CURSOR_UPDATE`, grid events and lock display fire from one place.

use mlua::{Lua, Value};

use super::{Model, ScriptValue};

mod bag_verbs;
mod bar;
mod doll;
mod drag;
pub(crate) mod money;
mod pet;

pub(crate) use bar::place_action;
pub(crate) use drag::{
    abandon_drag, arm_drag, maybe_start_drag, take_drag, DragGesture, DragRelease,
};

/// Sentinel bag id for the player's equipped slots, whose `slot` is a 1-based inventory slot id
/// (`GetInventorySlotInfo`'s, HeadSlot = 1). Never a Lua value, and disjoint from every real bag
/// id, -1 the bank and -2 the keyring included. Ammo (slot 0) is outside it.
pub const EQUIPMENT_BAG: i64 = -100;

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The payload
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// What the cursor carries, by the client's payload mode `[0xb4d900]`: 1 item, 2 money, 3 spell,
/// 4 pet action, 5 vendor row, 8 macro, 10 stabled pet; `Action` is the bar-slot pickup.
#[derive(Clone, Debug, PartialEq)]
pub enum CursorPayload {
    Item(CursorItem),
    Spell(CursorSpell),
    Action(CursorAction),
    Macro(CursorMacro),
    PetAction(CursorPetAction),
    /// Mode 5, a vendor row. Not an `Item`: `CursorHasItem` (`0x4895d0`) is true for modes 1 and 9
    /// only, so all thirteen stock `CursorHasItem()` gates stay shut while one is held.
    Merchant(CursorMerchantItem),
    /// Mode 10, a pet taken from the stable; its grab `0x495010` is the only writer of mode 10.
    StablePet(CursorStablePet),
    /// Mode 2, coins taken off a money frame.
    Money(CursorMoney),
}

/// Copper held on the cursor, mode 2 (`[0xb4e2f0]`, set by `PickupPlayerMoney`, `0x48abc0`). The
/// purse is not debited; the stock money frame subtracts `GetCursorMoney()` (`MoneyFrame.lua:17`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorMoney {
    pub copper: u32,
}

/// A vendor row held on the cursor, mode 5, set by `PickupMerchantItem` (`0x4fb760`). A drop on a
/// bag slot is the buy (`CMSG_BUY_ITEM_IN_SLOT`, `0x1a3`); in the reference a drop on the world,
/// ESC or the vendor closing clears it with no packet. No registered binding reports mode 5 to Lua.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorMerchantItem {
    /// The row's item entry (`0xb4b41c`), which the buy names.
    pub item_id: u32,
    /// The 0-based vendor row (`0xb4b420` = `trunc(arg) - 1`): a second `PickupMerchantItem` on the
    /// same row drops the grab.
    pub row: u32,
    /// The icon at the mouse, resolved from the display id the reference stores (`0xb4d8ec`).
    pub texture: Option<String>,
}

/// A pet held on the cursor from the stable window, mode 10, set by the grab `0x495010`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorStablePet {
    /// `[0xb4e300]`, in `PickupStablePet`'s index space: 0 the summoned pet, 1..=2 a stable slot.
    pub slot: u8,
    /// The family icon at the mouse, never empty: the grab refuses a pet without one (`0x495010`).
    pub texture: String,
}

/// An item held on the cursor: where it was picked from, so the next click routes the swap, and
/// the icon at the mouse. The server's field updates do the actual move.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorItem {
    /// The source bag id in the Lua API's numbering, or [`EQUIPMENT_BAG`].
    pub bag: i64,
    /// The 1-based source slot.
    pub slot: u32,
    pub item_id: u32,
    pub texture: Option<String>,
    pub link: Option<String>,
    /// `Some(n)` for a split of `n`, `None` for the whole stack.
    pub count: Option<u32>,
    /// Quality (0..6) at pickup, for a world drop's `DELETE_ITEM_CONFIRM`.
    pub quality: Option<u32>,
    /// The 1-based inventory slot ids the item can be equipped into, empty if none: the paper
    /// doll's place and `CursorCanGoInSlot` read it.
    pub equip_slots: Vec<u8>,
    /// Whether `PlaceAction` accepts it, its only item filter.
    pub bar_placeable: bool,
}

/// How many items the cursor holds. A whole-stack pickup has `count: None`, so the size is read
/// from the source slot, which a pickup locks but does not empty; 1 if that slot is unreadable.
pub(super) fn held_count(model: &super::Model, item: &CursorItem) -> u32 {
    if let Some(n) = item.count {
        return n.max(1);
    }
    model
        .containers
        .get(&item.bag)
        .and_then(|c| c.slots.get(&item.slot))
        .filter(|s| s.item_id == item.item_id)
        .map_or(1, |s| s.count.max(1))
}

/// A spell payload, mode 3, from `PickupSpell` on the player's spellbook.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorSpell {
    pub book_slot: u32,
    pub book_type: String,
    pub spell_id: u32,
    pub texture: Option<String>,
    /// `Attributes & 0x40` (`SPELL_ATTR_PASSIVE`): `PlaceAction` refuses a passive (`0x4e63ad`).
    pub passive: bool,
}

/// An action-bar payload from `PickupAction`: the source slot, the action's type byte as
/// `CMSG_SET_ACTION_BUTTON` packs it (spell, macro or item) and its id.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorAction {
    pub src_slot: u32,
    pub kind: u8,
    pub action: u32,
    pub texture: Option<String>,
}

/// A pet-bar payload, mode 4, from `PickupPetAction` or the pet spellbook: the packed action word
/// copied verbatim into `[0xb4e2f8]` (`0x494f0c`) and dropped undecoded (`0x4bce00`). Pet bar
/// only: `PlaceAction` refuses it (`0x4e62e0`).
#[derive(Clone, Debug, PartialEq)]
pub struct CursorPetAction {
    /// The 1-based pet bar slot it came from, 0 from the spellbook.
    pub src_slot: u32,
    /// The packed action word, verbatim.
    pub packed: u32,
    /// `Attributes & 0x40` of a spell word, tested at the drop, not the pickup (`0x4bc9f8`).
    pub passive: bool,
    pub texture: Option<String>,
}

/// A macro payload from `PickupMacro`, mode 8: the bare macro id (`[0xb4e2fc]`), which
/// `PlaceAction` accepts (`0x4e62e0`). It has no source slot, so a refused place leaves it held.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorMacro {
    /// The 1-based macro index: 1..=18 account, 19..=36 character.
    pub index: u32,
    pub texture: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The transition events
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Queue `CURSOR_UPDATE`, once per payload transition, plus a grid event when what the bars can
/// take changes. Call it after `model.cursor` holds the new payload; an action hop keeps the grid
/// up rather than hiding and reshowing it.
pub(crate) fn queue_cursor_update(model: &mut Model) {
    model
        .pending_events
        .push(("CURSOR_UPDATE".to_string(), Vec::new()));
    // The action bar's grid shows for any payload but a pet action, which `PlaceAction` refuses,
    // or a vendor row, whose grab `0x4950f0` fires `ACTIONBAR_SHOWGRID` for mode 7 only
    // (`0x495106`, `0x49513a`). The reference fires it (`0x4e58c0`) only from the item, spell and
    // macro setters and that mode-7 grab, so coins and a stabled pet show it here and not there.
    // The pet bar's follows the pet action alone (`0x494f28`).
    let pet_held = matches!(model.cursor, Some(CursorPayload::PetAction(_)));
    let bar_held = model.cursor.is_some()
        && !pet_held
        && !matches!(model.cursor, Some(CursorPayload::Merchant(_)));
    if bar_held != model.cursor_grid_shown {
        model.cursor_grid_shown = bar_held;
        let event = if bar_held {
            "ACTIONBAR_SHOWGRID"
        } else {
            "ACTIONBAR_HIDEGRID"
        };
        model.pending_events.push((event.to_string(), Vec::new()));
    }
    if pet_held != model.pet_grid_shown {
        model.pet_grid_shown = pet_held;
        let event = if pet_held {
            "PET_BAR_SHOWGRID"
        } else {
            "PET_BAR_HIDEGRID"
        };
        model.pending_events.push((event.to_string(), Vec::new()));
    }
}

/// Queue `ITEM_LOCK_CHANGED(bag, slot)` for a source slot that a pickup locks or a put-down frees.
pub(crate) fn queue_lock_changed(model: &mut Model, bag: i64, slot: u32) {
    model.pending_events.push((
        "ITEM_LOCK_CHANGED".to_string(),
        vec![ScriptValue::Int(bag), ScriptValue::Int(i64::from(slot))],
    ));
}

/// Queue `DELETE_ITEM_CONFIRM(name, quality)` for an item dropped on the world. The payload stays
/// held: the popup's `DeleteCursorItem` or `ClearCursor` clears it.
fn queue_delete_item_confirm(model: &mut Model, item: &CursorItem) {
    let name = item_link_name(item.link.as_deref());
    let quality = item.quality.unwrap_or(0);
    model.pending_events.push((
        "DELETE_ITEM_CONFIRM".to_string(),
        vec![ScriptValue::Str(name), ScriptValue::Int(i64::from(quality))],
    ));
}

/// An item link's name, from its `|h[Name]|h` segment; empty for no link or another shape.
pub(super) fn item_link_name(link: Option<&str>) -> String {
    link.and_then(|l| l.split_once('['))
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(name, _)| name.to_string())
        .unwrap_or_default()
}

/// `ClearCursor()` (`0x495190`): drops any payload and frees an item's source slot, then signals
/// `CURSOR_UPDATE` from its shared tail (`0x49529b`) on every call, an empty cursor included.
pub(crate) fn clear_cursor(model: &mut Model) {
    // Any clear first cancels an armed gift wrap, whatever its parameters (`0x495190` opens with
    // `0x5edf10`): the wrapping paper unlocks and the cursor mode returns to the base.
    if let Some(wrap) = model.pending_wrap.take() {
        queue_lock_changed(model, wrap.bag, wrap.slot);
        model.ui_cursor = None;
        model.ui_cursor_dirty = true;
    }
    // Coins go back to nothing, as the purse was never debited; a vendor row clears with no
    // packet (`0x49525f`).
    let taken = model.cursor.take();
    queue_cursor_update(model);
    if let Some(CursorPayload::Item(item)) = taken {
        queue_lock_changed(model, item.bag, item.slot);
    }
}

/// The sell fork's clear, `SetCursorItem(0, …)` (`0x494b60`), shared by `PickupMerchantItem`
/// (`0x4fb760`) and the interact ladder's vendor arm (`0x5df5d0`): unlike [`clear_cursor`] it
/// leaves the source slot locked, grey until the server removes the item (the mode-1 arm skips
/// `0x495420`). Any other payload is no sale and stays held (`GetCursorItem`, `0x494c60`).
pub(crate) fn take_cursor_item_for_sale(model: &mut Model) -> Option<CursorItem> {
    let item = match model.cursor.take() {
        Some(CursorPayload::Item(item)) => item,
        other => {
            model.cursor = other;
            return None;
        }
    };
    queue_cursor_update(model);
    Some(item)
}

/// What is under the cursor in the world this frame: the reference's click-time pick state
/// `[this+0x350]` (`0x481f60`). The app feeds it; tests and captures leave it `Nothing`.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum WorldPick {
    /// State 0: no world hit (the sky).
    #[default]
    Nothing,
    /// State 1: terrain, a WMO or a doodad, but no unit or GameObject.
    Terrain,
    /// State 2: a unit or GameObject.
    Object,
}

/// A left click on the world with a payload held, press and release both over the world and no
/// drag (`0x495300`); returns whether it was consumed. Over an `Object` (`0x492ce0`) nothing
/// drops and the click selects. Over `Terrain` (`0x492c90`) or `Nothing` (`0x492d30`) an item
/// raises `DELETE_ITEM_CONFIRM(name, quality)` (`0x5e0320`) and stays held for the popup to clear;
/// a spell, action, macro or pet action clears. Coins, a vendor row and a stabled pet stay held
/// here, where the reference puts a vendor row down on every leg and clears any non-item payload
/// over `Nothing` (`0x492e04`).
///
/// Deviation: on `Terrain` the reference keeps a spell, action, macro or pet action on the left
/// button; we clear them, because otherwise a spell has no left-click way off the cursor.
pub(crate) fn world_drop_click(model: &mut Model) -> bool {
    match (&model.cursor, model.world_pick) {
        (Some(CursorPayload::Item(item)), WorldPick::Terrain | WorldPick::Nothing) => {
            let item = item.clone();
            queue_delete_item_confirm(model, &item);
            true
        }
        (
            Some(
                CursorPayload::Spell(_)
                | CursorPayload::Action(_)
                | CursorPayload::Macro(_)
                | CursorPayload::PetAction(_),
            ),
            WorldPick::Terrain | WorldPick::Nothing,
        ) => {
            clear_cursor(model);
            true
        }
        _ => false,
    }
}

/// `SplitContainerItem(bag, slot, count)`: picks up `count` of an unlocked stack onto an empty
/// cursor. A count of the whole stack or more is a plain pickup, since vmangos refuses to split a
/// whole stack or more (`Player.cpp:11139`, `:11147`). Returns whether anything was picked up.
pub(crate) fn split_container_item(model: &mut Model, bag: i64, slot: u32, count: i64) -> bool {
    if model.cursor.is_some() || count < 1 {
        return false;
    }
    let Some(s) = model.containers.get(&bag).and_then(|c| c.slots.get(&slot)) else {
        return false;
    };
    if s.item_id == 0 || s.locked {
        return false;
    }
    let n = count.min(i64::from(s.count)) as u32;
    let whole = n >= s.count;
    let item = CursorItem {
        bag,
        slot,
        item_id: s.item_id,
        texture: s.texture.clone(),
        link: s.link.clone(),
        quality: s.quality,
        count: if whole { None } else { Some(n) },
        bar_placeable: s.bar_placeable,
        equip_slots: s.equip_slots.clone(),
    };
    model.cursor = Some(CursorPayload::Item(item));
    queue_cursor_update(model);
    queue_lock_changed(model, bag, slot);
    true
}

/// `DeleteCursorItem()`, the delete popup's accept: a held item queues its `(bag, slot, count)`
/// destroy, count 0 for the whole stack, and clears; anything else is a no-op.
pub(crate) fn delete_cursor_item(model: &mut Model) {
    let Some(CursorPayload::Item(item)) = &model.cursor else {
        return;
    };
    let item = item.clone();
    model.cursor = None;
    model
        .container_destroys
        .push((item.bag, item.slot, item.count.unwrap_or(0)));
    queue_cursor_update(model);
    queue_lock_changed(model, item.bag, item.slot);
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// UiScript accessors
// ─────────────────────────────────────────────────────────────────────────────────────────────

impl super::UiScript {
    /// The held item, if the payload is one.
    pub fn cursor_item(&self) -> Option<CursorItem> {
        match self.model_ref().cursor.clone() {
            Some(CursorPayload::Item(item)) => Some(item),
            _ => None,
        }
    }

    /// The cursor payload, any arm.
    pub fn cursor_payload(&self) -> Option<CursorPayload> {
        self.model_ref().cursor.clone()
    }

    /// Feed the world pick under the cursor this frame ([`WorldPick`]).
    pub fn set_world_pick(&mut self, pick: WorldPick) {
        self.model_mut().world_pick = pick;
    }

    /// [`take_cursor_item_for_sale`], for the app's interact ladder.
    pub fn take_cursor_item_for_sale(&mut self) -> Option<CursorItem> {
        take_cursor_item_for_sale(&mut self.model_mut())
    }

    /// `ClearCursor()` from Rust, which the app calls on a right-click over empty world: both
    /// reference legs there (`0x492c90`, `0x492d30`) call `ClearCursor(1, 1)` (`0x495190`).
    pub fn clear_cursor_payload(&mut self) {
        clear_cursor(&mut self.model_mut());
    }

    /// Test-only: set the payload directly, bypassing the Lua bindings.
    #[cfg(test)]
    pub(crate) fn set_cursor_for_test(&mut self, payload: CursorPayload) {
        self.model_mut().cursor = Some(payload);
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The targeting cursor's item pick and the enchant confirms
// ─────────────────────────────────────────────────────────────────────────────────────────────
impl super::UiScript {
    /// Arm or disarm the targeting cursor's item half (`TargetingWantsItem`, `0x6e6330`): while
    /// armed, a bag or paper-doll click queues its `(bag, slot)` as a pick instead of picking up,
    /// as the reference's two pickups do (`0x4f9b30` at `0x4f9c54`, `0x4c7300` at `0x4c76df`).
    pub fn set_item_pick_armed(&mut self, armed: bool) {
        self.model_mut().item_pick_armed = armed;
    }

    /// Push what `SpellCanTargetUnit` answers: whether the armed targeting cast can take a unit.
    pub fn set_spell_can_target_unit(&mut self, can: bool) {
        self.model_mut().spell_can_target_unit = can;
    }

    /// Drain the `(bag, slot)` picks since the last call; a paper-doll pick reports as
    /// [`EQUIPMENT_BAG`] and its 1-based inventory slot.
    pub fn take_item_picks(&mut self) -> Vec<(i64, u32)> {
        std::mem::take(&mut self.model_mut().item_picks)
    }

    /// Drain the enchant-confirm answers since the last call; both answer the pick the app parked.
    pub fn take_enchant_confirms(&mut self) -> Vec<EnchantConfirm> {
        std::mem::take(&mut self.model_mut().enchant_confirms)
    }
}

/// A Yes on an enchant-apply confirm (`StaticPopup.lua:1245`, `:1257`); the app holds the item
/// guid it answers for (the reference's `0xb4e3c0`/`0xb4e3c4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnchantConfirm {
    /// `BindEnchant()` (`0x48d2e0`): reruns the enchant-apply gate `0x495d60` as already
    /// confirmed, so it may raise the replace popup next rather than cast.
    Bind,
    /// `ReplaceEnchant()` (`0x48d300`): binds the parked guid through `0x6e5b40`, skipping the
    /// gate. There is no replace packet: the popup only gates the same cast.
    Replace,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// Lua globals
// ─────────────────────────────────────────────────────────────────────────────────────────────

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `BindEnchant()`, `ReplaceEnchant()`: queue an intent for the app; nothing is sent from here.
    for (name, answer) in [
        ("BindEnchant", EnchantConfirm::Bind),
        ("ReplaceEnchant", EnchantConfirm::Replace),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                model.enchant_confirms.push(answer);
                Ok(())
            })?,
        )?;
    }

    // `GetCursorInfo()` is not a 1.12 verb. It answers ("item", id, link), ("spell", book slot,
    // book, spell id), ("action", slot), ("macro", index) or ("petaction", slot), padded with nil
    // to four values; any other payload, or none, is four nils.
    g.set(
        "GetCursorInfo",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let str_or_nil = |lua: &Lua, s: &Option<String>| -> mlua::Result<Value> {
                match s {
                    Some(s) => Ok(Value::String(lua.create_string(s)?)),
                    None => Ok(Value::Nil),
                }
            };
            match &model.cursor {
                Some(CursorPayload::Item(c)) => Ok((
                    Value::String(lua.create_string("item")?),
                    Value::Integer(i64::from(c.item_id)),
                    str_or_nil(lua, &c.link)?,
                    Value::Nil,
                )),
                Some(CursorPayload::Spell(s)) => Ok((
                    Value::String(lua.create_string("spell")?),
                    Value::Integer(i64::from(s.book_slot)),
                    Value::String(lua.create_string(&s.book_type)?),
                    Value::Integer(i64::from(s.spell_id)),
                )),
                Some(CursorPayload::Action(a)) => Ok((
                    Value::String(lua.create_string("action")?),
                    Value::Integer(i64::from(a.src_slot)),
                    Value::Nil,
                    Value::Nil,
                )),
                Some(CursorPayload::Macro(m)) => Ok((
                    Value::String(lua.create_string("macro")?),
                    Value::Integer(i64::from(m.index)),
                    Value::Nil,
                    Value::Nil,
                )),
                Some(CursorPayload::PetAction(p)) => Ok((
                    Value::String(lua.create_string("petaction")?),
                    Value::Integer(i64::from(p.src_slot)),
                    Value::Nil,
                    Value::Nil,
                )),
                Some(CursorPayload::StablePet(_)) => {
                    Ok((Value::Nil, Value::Nil, Value::Nil, Value::Nil))
                }
                Some(CursorPayload::Money(_)) => {
                    Ok((Value::Nil, Value::Nil, Value::Nil, Value::Nil))
                }
                Some(CursorPayload::Merchant(_)) => {
                    Ok((Value::Nil, Value::Nil, Value::Nil, Value::Nil))
                }
                None => Ok((Value::Nil, Value::Nil, Value::Nil, Value::Nil)),
            }
        })?,
    )?;

    g.set(
        "CursorHasItem",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(matches!(model.cursor, Some(CursorPayload::Item(_))))
        })?,
    )?;
    g.set(
        "CursorHasSpell",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(matches!(model.cursor, Some(CursorPayload::Spell(_))))
        })?,
    )?;
    g.set(
        "ClearCursor",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            clear_cursor(&mut model);
            Ok(())
        })?,
    )?;

    g.set(
        "SplitContainerItem",
        lua.create_function(|lua, (bag, slot, count): (i64, u32, i64)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(split_container_item(&mut model, bag, slot, count))
        })?,
    )?;
    g.set(
        "DeleteCursorItem",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            delete_cursor_item(&mut model);
            Ok(())
        })?,
    )?;

    doll::install(lua)?;
    bag_verbs::install(lua)?;
    bar::install(lua)?;
    pet::install(lua)?;
    money::install(lua)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CursorAction, CursorItem, CursorPayload, CursorSpell};
    use crate::script::UiScript;

    /// A backpack holding a five-stack in slot 1.
    fn one_item_backpack() -> crate::script::container::ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            crate::script::container::ContainerSlot {
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
                count: 5,
                quality: Some(3),
                item_id: 117,
                link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
                locked: false,
                equip_slots: Vec::new(),
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        crate::script::container::ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    /// `SetCursorItem(0, …)` (`0x494b60`) skips `ClearCursor`'s unlock (`0x495420`) and takes only
    /// a held item.
    #[test]
    fn the_sell_clear_keeps_the_source_slot_locked() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));

        // Drain the pickup's events so only the sell clear's remain.
        s.eval::<()>("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some(), "the pickup did not take");
        s.model_mut().pending_events.clear();

        let sold = s.take_cursor_item_for_sale().expect("mode 1 is a sale");
        assert_eq!((sold.bag, sold.slot), (0, 1));
        assert!(
            s.cursor_payload().is_none(),
            "the cursor kept the sold item"
        );

        let fired: Vec<String> = s
            .model_ref()
            .pending_events
            .iter()
            .map(|(e, _)| e.clone())
            .collect();
        assert!(
            fired.iter().any(|e| e == "CURSOR_UPDATE"),
            "the sell clear must fire CURSOR_UPDATE — got {fired:?}"
        );
        assert!(
            !fired.iter().any(|e| e == "ITEM_LOCK_CHANGED"),
            "the sell clear must NOT un-lock the source slot (that is `ClearCursor`'s job, not \
             `SetCursorItem(0, …)`'s) — got {fired:?}"
        );

        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
            passive: false,
        }));
        assert!(
            s.take_cursor_item_for_sale().is_none(),
            "a spell is not a sale"
        );
        assert!(
            matches!(s.cursor_payload(), Some(CursorPayload::Spell(_))),
            "asking for a sale dropped a non-item payload"
        );
    }

    #[test]
    fn get_cursor_info_and_has_checks_per_arm() {
        let mut s = UiScript::new().unwrap();

        assert!(s
            .eval::<bool>("local k = GetCursorInfo() return k == nil")
            .unwrap());
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(!s.eval::<bool>("return CursorHasSpell()").unwrap());

        s.set_cursor_for_test(CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: Some("|Hitem:117|h[Tough Jerky]|h".into()),
            count: None,
            quality: Some(3),
            equip_slots: Vec::new(),
        }));
        let (kind, id, link) = s
            .eval::<(String, i64, String)>("local k, id, link = GetCursorInfo() return k, id, link")
            .unwrap();
        assert_eq!(
            (kind.as_str(), id, link.as_str()),
            ("item", 117, "|Hitem:117|h[Tough Jerky]|h")
        );
        assert!(s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(!s.eval::<bool>("return CursorHasSpell()").unwrap());

        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 3,
            book_type: "spell".into(),
            spell_id: 133,
            texture: None,
        }));
        let (kind, slot, book, spell_id) = s
            .eval::<(String, i64, String, i64)>(
                "local k, slot, book, id = GetCursorInfo() return k, slot, book, id",
            )
            .unwrap();
        assert_eq!(
            (kind.as_str(), slot, book.as_str(), spell_id),
            ("spell", 3, "spell", 133)
        );
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(s.eval::<bool>("return CursorHasSpell()").unwrap());

        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 12,
            kind: 0,
            action: 133,
            texture: None,
        }));
        let (kind, slot) = s
            .eval::<(String, i64)>("local k, slot = GetCursorInfo() return k, slot")
            .unwrap();
        assert_eq!((kind.as_str(), slot), ("action", 12));
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(!s.eval::<bool>("return CursorHasSpell()").unwrap());
    }

    #[test]
    fn clear_cursor_widens_to_any_arm() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
            passive: false,
            book_slot: 1,
            book_type: "spell".into(),
            spell_id: 1,
            texture: None,
        }));
        assert!(s.cursor_payload().is_some());
        s.run("ClearCursor()").unwrap();
        assert!(s.cursor_payload().is_none());
    }

    /// The clear's shared tail (`0x49529b`) signals on every call: with nothing held, one
    /// `CURSOR_UPDATE` and no grid event.
    #[test]
    fn clear_cursor_with_nothing_held_still_signals() {
        let s = UiScript::new().unwrap();
        let before = s.model_ref().pending_events.len();
        s.run("ClearCursor()").unwrap();
        let fired: Vec<String> = s.model_ref().pending_events[before..]
            .iter()
            .map(|(e, _)| e.clone())
            .collect();
        assert_eq!(fired, vec!["CURSOR_UPDATE"]);
    }

    /// Clearing a spell on terrain is the deviation documented on `world_drop_click`.
    #[test]
    fn world_drop_terrain_and_nothing_both_clear_a_spell_payload() {
        use super::{world_drop_click, WorldPick};
        let mut s = UiScript::new().unwrap();
        for pick in [WorldPick::Terrain, WorldPick::Nothing] {
            s.set_cursor_for_test(CursorPayload::Spell(CursorSpell {
                passive: false,
                book_slot: 3,
                book_type: "spell".into(),
                spell_id: 133,
                texture: None,
            }));
            s.model_mut().world_pick = pick;
            assert!(
                world_drop_click(&mut s.model_mut()),
                "{pick:?}: the dismiss consumes the click"
            );
            assert!(
                s.cursor_payload().is_none(),
                "{pick:?}: the spell clears silently"
            );
        }

        let item = CursorPayload::Item(CursorItem {
            bar_placeable: true,
            bag: 0,
            slot: 1,
            item_id: 117,
            texture: None,
            link: None,
            count: None,
            quality: None,
            equip_slots: Vec::new(),
        });
        s.set_cursor_for_test(item.clone());
        s.model_mut().world_pick = WorldPick::Terrain;
        assert!(
            world_drop_click(&mut s.model_mut()),
            "terrain: the item pops the popup"
        );
        assert!(s.cursor_item().is_some(), "and stays held");
        s.model_mut().world_pick = WorldPick::Object;
        assert!(
            !world_drop_click(&mut s.model_mut()),
            "object: nothing drops at all"
        );
        assert!(s.cursor_item().is_some());
    }

    #[test]
    fn delete_cursor_item_queues_destroy_and_clears() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());

        s.run("DeleteCursorItem()").unwrap();
        assert!(s.cursor_item().is_none(), "the payload clears");
        assert_eq!(s.take_container_destroys(), vec![(0, 1, 0)]);
        assert!(s.take_container_destroys().is_empty(), "drained");
    }

    #[test]
    fn delete_cursor_item_is_a_no_op_off_the_item_arm() {
        let mut s = UiScript::new().unwrap();
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0,
            action: 1,
            texture: None,
        }));
        s.run("DeleteCursorItem()").unwrap();
        assert!(
            s.cursor_payload().is_some(),
            "non-item payload is untouched"
        );
        assert!(s.take_container_destroys().is_empty());
    }

    #[test]
    fn split_of_the_whole_stack_degrades_to_a_plain_pickup() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(one_item_backpack())); // slot 1: a 5-stack
        assert!(s
            .eval::<bool>("return SplitContainerItem(0, 1, 5)")
            .unwrap());
        let held = s.cursor_item().expect("picked up");
        assert_eq!(held.count, None, "count>=stack ⇒ whole-stack pickup");
    }
}
