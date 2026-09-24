//! The engine item-tooltip renderer: every item hover renders through one line law, as 8 of the
//! reference's 9 `Set*Item` bindings share the builder `0x52b650`. The methods register into the
//! GameTooltip kind table beside the widget verbs ([`super::tooltip`]). Every family's order,
//! gate and colour is the reference's, and every sentence is a key resolved off the player's own
//! `GlobalStrings.lua` at render time. Red (`0xc0d390`, `0xffff2020`) marks unmet requirements,
//! LOCKED, broken durability and "Already known"; the name never recolors.
//!
//! Stock FrameXML seats the shopping plates (`MerchantFrame.xml:63-80`,
//! `Blizzard_AuctionUI.lua:1078`). There is no hover compare: `SHOW_COMPARE_TOOLTIP` has no fire
//! site (`0x49211a` and `0x49630d` never produce event 377), so `PaperDollFrame.lua`'s listener
//! for it is dead code.

use mlua::{Lua, MultiValue, Table, Value};

use super::object::frame_handle_of;
use super::tooltip::{append_line, clear_content, fire_cleared, show_or_hide_empty};
use super::{ItemTemplateView, Model};

mod names;
mod render;

use names::{quality_color, GRAY, WHITE};
use render::{render_view, BuilderFlags};

/// The body both compare bindings run (`0x536080`, `0x535d70`): walk `0x809200[InventoryType]`'s
/// slots, skipping empty ones uncounted and ones whose worn item's class differs (`0x5d9f30`),
/// until `left` matches have passed; then run `SetInventoryItem` there with the header latch armed,
/// as `0x536080` calls `0x52b650` with p4 = 0, p5 = 1. The number 1, or nil on a miss, which leaves
/// the tooltip untouched.
fn compare_against_worn(
    lua: &Lua,
    this: &Table,
    h: crate::widget::FrameHandle,
    offered: &ItemTemplateView,
    mut left: f64,
) -> mlua::Result<Value> {
    let found = {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        let mut hit = None;
        for &slot in equip_slots_for(offered.inventory_type) {
            let Some(worn) = model
                .inv_slot("player", slot as usize)
                .filter(|s| s.item_id != 0)
            else {
                continue; // an empty slot is not counted
            };
            let same_class = model
                .item_templates
                .get(&worn.item_id)
                .is_some_and(|w| w.class == offered.class);
            if !same_class {
                continue;
            }
            if left <= 0.0 {
                hit = Some(slot as usize);
                break;
            }
            left -= 1.0;
        }
        hit
    };
    let Some(slot) = found else {
        return Ok(Value::Nil);
    };
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        if let Ok(t) = super::tooltip::tip_mut(&mut model, h) {
            t.equipped_header_armed = true;
        }
    }
    this.get::<mlua::Function>("SetInventoryItem")?
        .call::<mlua::MultiValue>((this.clone(), "player", slot))?;
    Ok(Value::Integer(1))
}

/// The cached template; a miss records the ask, and the hover's re-enter loop repaints on arrival.
fn view_of(lua: &Lua, item_id: u32) -> Option<ItemTemplateView> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let v = model.item_templates.get(&item_id).cloned();
    if v.is_none() && item_id != 0 {
        model.item_stat_asks.insert(item_id);
    }
    v
}

/// A random-suffix roll's enchant lines: the reference copies the suffix row's five ids into
/// enchant slots 2..6 (`0x52b7bf`) for a block source with no item object.
fn roll_enchants(lua: &Lua, random_property_id: u32) -> Vec<crate::script::EnchantView> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    model
        .random_properties
        .get(&random_property_id)
        .map(|v| v.enchants.clone())
        .unwrap_or_default()
}

fn link_name(link: &str) -> Option<&str> {
    let start = link.find('[')? + 1;
    let end = link[start..].find(']')? + start;
    Some(&link[start..end])
}

/// Fire `OnTooltipAddMoney(copper)`: the engine computes, FrameXML's `SetTooltipMoney` draws.
fn fire_add_money(lua: &Lua, h: crate::widget::FrameHandle, copper: u64) {
    let id = {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        model.frame_id(h)
    };
    if let Err(e) = super::event::fire_widget_handler(
        lua,
        id,
        "OnTooltipAddMoney",
        vec![Value::Number(copper as f64)],
    ) {
        lua.app_data_mut::<Model>()
            .expect("model app_data")
            .record_script_error(e.to_string());
    }
}

fn hyperlink_enchant_spell(link: &str) -> Option<u32> {
    let at = link.find("enchant:")? + 8;
    let tail = &link[at..];
    let end = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    tail[..end].parse().ok().filter(|&id: &u32| id != 0)
}

fn hyperlink_item_fields(link: &str) -> Option<(u32, u32, u32)> {
    let at = link.find("item:")? + 5;
    let tail = &link[at..];
    let end = tail.find("|h").unwrap_or(tail.len());
    let mut fields = tail[..end]
        .split(':')
        .map(|f| f.trim().parse().unwrap_or(0));
    let item_id: u32 = fields.next()?;
    let enchant_id = fields.next().unwrap_or(0);
    let random_property_id = fields.next().unwrap_or(0);
    (item_id != 0).then_some((item_id, enchant_id, random_property_id))
}

/// The id-keyed render; on a miss, the ask and a name-only line, as `SetBagItem`'s miss path.
fn render_by_id(
    lua: &Lua,
    this: &Table,
    item_id: u32,
    fb_name: Option<String>,
    fb_q: Option<u32>,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        clear_content(&mut model, h);
    }
    fire_cleared(lua, h);
    match view_of(lua, item_id) {
        Some(v) => {
            render_view(lua, this, &v, BuilderFlags::default(), None)?;
        }
        None => {
            if let Some(name) = fb_name {
                append_line(
                    lua,
                    this,
                    (name, quality_color(fb_q.unwrap_or(1))),
                    None,
                    false,
                )?;
            }
        }
    }
    show_or_hide_empty(lua, h);
    Ok(())
}

fn set_quest_item_view(
    lua: &Lua,
    this: &Table,
    item: Option<crate::script::quest::QuestItemView>,
) -> mlua::Result<()> {
    match item {
        Some(it) => render_by_id(lua, this, it.item_id, it.name.clone(), Some(it.quality)),
        None => {
            let h = frame_handle_of(lua, this)?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            super::tooltip::show_or_hide_empty(lua, h);
            Ok(())
        }
    }
}

/// An InventoryType's compare candidate slots, as `GetInventorySlotInfo` ids: the reference's
/// mask table `0x809200[InventoryType]`.
fn equip_slots_for(inventory_type: u32) -> &'static [u32] {
    match inventory_type {
        1 => &[1],             // head
        2 => &[2],             // neck
        3 => &[3],             // shoulder
        4 => &[4],             // shirt
        5 | 20 => &[5],        // chest / robe
        6 => &[6],             // waist
        7 => &[7],             // legs
        8 => &[8],             // feet
        9 => &[9],             // wrist
        10 => &[10],           // hands
        11 => &[11, 12],       // finger
        12 => &[13, 14],       // trinket
        16 => &[15],           // back
        19 => &[19],           // tabard
        13 => &[16, 17],       // one-hand
        21 => &[16],           // main hand
        14 | 22 | 23 => &[17], // shield / off-hand weapon / held
        // Two-hand: main hand only, though a two-hander displaces both: `0x809200[17]` is
        // `0x8000`, bit 15.
        17 => &[16],
        15 | 25 | 26 | 28 => &[18], // bow / thrown / wand-gun / relic
        _ => &[],
    }
}

/// Register the item content channels into the GameTooltip kind method table.
pub(super) fn install_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:BenillaSetItemById(itemId [, fallbackName, fallbackQuality]). Not a 1.12 verb,
    // whose `Set*Item` bindings all name a container: sibling modules' item hovers (the action
    // bar's, `SetTrainerService`, `SetCraftSpell`) reach `render_by_id` by this name through the
    // wrapper table, and the prefix keeps it off the 1.12 surface.
    m.set(
        "BenillaSetItemById",
        lua.create_function(
            |lua, (this, item_id, fb_name, fb_q): (Table, u32, Option<String>, Option<u32>)| {
                render_by_id(lua, &this, item_id, fb_name, fb_q)
            },
        )?,
    )?;

    // GameTooltip:SetQuestItem(type, index) / SetQuestLogItem(type, index) (`0x533610`,
    // `0x533760`; `QuestFrameTemplates.xml:148`, `QuestLogFrame.xml:113`), no returns. `type` is
    // `GetQuestItemInfo`'s; an unknown type or index leaves the tooltip empty, as in the reference.
    m.set(
        "SetQuestItem",
        lua.create_function(|lua, (this, kind, index): (Table, String, usize)| {
            let item = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.quest.as_ref().and_then(|q| {
                    crate::script::quest::item_vec(q, &kind)
                        .and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                        .cloned()
                })
            };
            set_quest_item_view(lua, &this, item)
        })?,
    )?;
    m.set(
        "SetQuestLogItem",
        lua.create_function(|lua, (this, kind, index): (Table, String, usize)| {
            let item = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.selected_quest_detail().and_then(|d| {
                    let v = match kind.as_str() {
                        "choice" => Some(&d.choices),
                        "reward" => Some(&d.rewards),
                        _ => None,
                    };
                    v.and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                        .cloned()
                })
            };
            set_quest_item_view(lua, &this, item)
        })?,
    )?;

    // GameTooltip:SetHyperlink(link): the chat-link tooltip (`ItemRef.lua:60`). Takes an item
    // link, escaped or a bare "item:<id>", or an enchant link; any other link does nothing.
    m.set(
        "SetHyperlink",
        lua.create_function(|lua, (this, link): (Table, String)| {
            // The only other link it takes, the craft window's `|Henchant:<spellId>|h[name]|h`
            // (`GetCraftItemLink`): the spell's own tooltip, behind its producer's castUI gate
            // (`0x532243`).
            if let Some(spell) = hyperlink_enchant_spell(&link) {
                return super::tooltip_spell::set_spell_by_id(
                    lua,
                    &this,
                    spell,
                    link_name(&link).map(str::to_string),
                    Default::default(),
                    None,
                );
            }
            let Some((id, _enchant_id, roll)) = hyperlink_item_fields(&link) else {
                return Ok(());
            };
            let h = frame_handle_of(lua, &this)?;
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // The reference's format is `"%s|Hitem:%d:%d:%d:%d|h[%s]|h%s"` (`0x8549c8`), and
            // `SetHyperlink` (`0x532181`) parses it into a block (p6=1): the enchant id to slot 0
            // (`+0x3d0`), the roll to `+0x424` (expanded into slots 2..6 by `0x52b7bf`), the
            // unique id to `+0x420`, unread; a missing field is 0. So a link shows its roll's
            // lines, never the placeholder (`0x52cc33`). The name is the link's own, built with
            // the suffix joined (`0x5d8b00`). The enchant id is parsed, not rendered: naming it
            // needs its `SpellItemEnchantment` row, and no link benilla composes carries one.
            let inst = render::ItemInstance {
                name: link_name(&link).map(str::to_string),
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            match view_of(lua, id) {
                Some(v) => {
                    render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?;
                }
                None => {
                    if let Some(name) = link_name(&link) {
                        append_line(
                            lua,
                            &this,
                            (name.to_string(), quality_color(1)),
                            None,
                            false,
                        )?;
                    }
                }
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetInventoryItem(unit, slot [, nameOnly]) -> hasItem, hasCooldown, repairCost
    // (`0x532ee0`), three as `PaperDollFrame.lua:741` reads them; the arity is that consumer's,
    // not byte-read as `SetBagItem`'s two is (`0x534985`). Unit-keyed through `Model::inv_slot`:
    // an inspected item (`InspectPaperDollFrame.xml:20`) has no durability or creator, so those
    // lines do not show, as in the reference. An armed shopping tooltip renders the compare
    // shape; the arm is consumed either way. repairCost is 0, as `SetBagItem`'s is.
    m.set(
        "SetInventoryItem",
        lua.create_function(
            |lua, (this, unit, slot, name_only): (Table, String, usize, Value)| {
                let h = frame_handle_of(lua, &this)?;
                // `nameOnly` (usage string `0x8552dc`) is set only by a number above 0
                // (`0x533027..0x53304c`); anything else builds the ordinary tooltip, without an
                // error. Three of the binding's four builder legs pass it on, not `0x533106`.
                let name_only = super::binding_abi::positive_number_flag(lua, name_only)?;
                let (item_id, name, quality, inst, currently_equipped) = {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    let armed = match super::tooltip::tip_mut(&mut model, h) {
                        Ok(t) => std::mem::take(&mut t.equipped_header_armed),
                        Err(_) => false,
                    };
                    let view = model
                        .inv_slot(&unit, slot)
                        .filter(|s| s.item_id != 0)
                        .map(|s| {
                            (
                                s.item_id,
                                s.name.clone(),
                                s.quality,
                                render::ItemInstance {
                                    // App-composed, with the suffix off
                                    // `ITEM_FIELD_RANDOM_PROPERTIES_ID`.
                                    name: s.name.clone(),
                                    durability: s.durability,
                                    creator: s.creator.clone(),
                                    has_text: false,
                                    flags: s.flags,
                                    already_bound: s.already_bound,
                                    // A charter has `InventoryType` 0 and is never equipped.
                                    petition: None,
                                    enchants: s.enchants.clone(),
                                    // `0x532ee0` has p6=0 legs too (`0x533106`, `0x5332ad`), but
                                    // nothing openable is equippable.
                                    openable_source: false,
                                    duration_ms: s.duration_ms,
                                },
                            )
                        });
                    match view {
                        Some((id, name, q, inst)) => (id, name, q, inst, armed),
                        // An empty slot answers nil and still pushes the other two: callers
                        // destructure all three first, and an addon may add up repairCost.
                        None => {
                            return Ok(MultiValue::from_vec(vec![
                                Value::Nil,
                                Value::Nil,
                                Value::Integer(0),
                            ]))
                        }
                    }
                };
                let flags = BuilderFlags {
                    currently_equipped,
                    name_only,
                };
                {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    clear_content(&mut model, h);
                }
                fire_cleared(lua, h);
                match view_of(lua, item_id) {
                    Some(v) => render_view(lua, &this, &v, flags, Some(&inst))?,
                    None => {
                        // Template in flight: the header and the slot's own name, as
                        // `SetBagItem`'s miss path.
                        if flags.currently_equipped {
                            if let Some(t) = crate::strings::global(lua, "CURRENTLY_EQUIPPED") {
                                append_line(lua, &this, (t, GRAY), None, false)?;
                            }
                        }
                        if let Some(name) = name {
                            let color = if flags.name_only {
                                WHITE
                            } else {
                                quality_color(quality.max(0) as u32)
                            };
                            append_line(lua, &this, (name, color), None, false)?;
                        }
                    }
                }
                show_or_hide_empty(lua, h);
                Ok(MultiValue::from_vec(vec![
                    Value::Integer(1),
                    // hasCooldown is the builder's `[ebp-0x38]`, set by four lines:
                    // LOCKED_WITH_ITEM, the temporary-enchant countdown, the item's duration
                    // (`0x52ce0d`) and ITEM_COOLDOWN_TIME. Only the duration is answered here.
                    if inst.duration_ms.is_some() {
                        Value::Boolean(true)
                    } else {
                        Value::Nil
                    },
                    Value::Integer(0),
                ]))
            },
        )?,
    )?;

    // GameTooltip:SetBagItem(bag, slot) -> hasCooldown, repairCost: the real-instance hover and
    // the one money source: with the merchant open and repair off (`0x52e376`), the engine fires
    // `OnTooltipAddMoney(SellPrice × stack)`, or prints ITEM_UNSELLABLE at price 0 (`0x854a74`,
    // pushed at `0x52e4a3`). repairCost is 0: the per-item repair cost is not fed. The reference
    // always pushes a number there (`0x534975`), and stock guards on `> 0`.
    m.set(
        "SetBagItem",
        lua.create_function(|lua, (this, bag, slot): (Table, i64, u32)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, count, has_cd, link, quality, inst) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match model
                    .containers
                    .get(&bag)
                    .and_then(|c| c.slots.get(&slot))
                    .filter(|s| s.item_id != 0)
                {
                    // `0x534620` takes the p6=1 leg iff the cooldown query `0x6e2ed0` reports
                    // enable, start and duration all non-zero; that one boolean is both the
                    // `hasCooldown` return and the inverse of the openable gate.
                    Some(s) => {
                        let has_cd = s
                            .cooldown
                            .is_some_and(|(start, dur, en)| en && start > 0 && dur > 0);
                        (
                            s.item_id,
                            s.count.max(1),
                            has_cd,
                            s.link.clone(),
                            s.quality.unwrap_or(1),
                            render::ItemInstance {
                                // The link's name: the reference builds both from `0x5d8b00`.
                                name: s.link.as_deref().and_then(link_name).map(str::to_string),
                                durability: s.durability,
                                creator: s.creator.clone(),
                                has_text: s.readable,
                                flags: s.flags,
                                already_bound: s.already_bound,
                                petition: s.petition.clone(),
                                enchants: s.enchants.clone(),
                                // p6 = 0 iff no cooldown runs; mid-cooldown the reference shows
                                // ITEM_COOLDOWN_TIME instead, which is not built.
                                openable_source: !has_cd,
                                duration_ms: s.duration_ms,
                            },
                        )
                    }
                    None => return Ok(MultiValue::from_vec(vec![Value::Nil])),
                }
            };
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            match view_of(lua, item_id) {
                Some(v) => {
                    render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?;
                    let (merchant_open, repairing) = {
                        let model = lua.app_data_mut::<Model>().expect("model app_data");
                        (model.merchant.is_some(), model.repair_mode)
                    };
                    if merchant_open && !repairing {
                        if v.sell_price > 0 {
                            fire_add_money(lua, h, u64::from(v.sell_price) * u64::from(count));
                        } else if let Some(t) = crate::strings::global(lua, "ITEM_UNSELLABLE") {
                            append_line(lua, &this, (t, WHITE), None, false)?;
                        }
                    }
                }
                None => {
                    // Deviation: while the template is in flight the reference leaves the tooltip
                    // empty (`0x52b6a3`); this shows the slot's name, because a blank plate under
                    // an on-screen name reads as broken. The re-enter loop repaints on arrival.
                    if let Some(name) = link.as_deref().and_then(link_name) {
                        append_line(
                            lua,
                            &this,
                            (name.to_string(), quality_color(quality)),
                            None,
                            false,
                        )?;
                    }
                }
            }
            show_or_hide_empty(lua, h);
            Ok(MultiValue::from_vec(vec![
                // The builder's `[ebp-0x38]`: a running cooldown or the item's duration line.
                Value::Boolean(has_cd || inst.duration_ms.is_some()),
                Value::Integer(0),
            ]))
        })?,
    )?;

    // GameTooltip:SetTradePlayerItem(slot) / SetTradeTargetItem(slot) (`0x5341e0`, `0x534410`;
    // `TradeFrame.xml:150`, `TradeFrame.xml:106`), no returns: the slot's item, the target side
    // off the partner's slot guids (`[0xb715a0 + slot*4]`). An empty or out-of-range slot leaves
    // the tooltip untouched. The enchant line is not rendered: the trade view has no enchant id.
    for (name, player_side) in [("SetTradePlayerItem", true), ("SetTradeTargetItem", false)] {
        m.set(
            name,
            lua.create_function(move |lua, (this, slot): (Table, usize)| {
                let h = frame_handle_of(lua, &this)?;
                let item = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model.trade.as_ref().and_then(|t| {
                        let side = if player_side { &t.player } else { &t.target };
                        slot.checked_sub(1)
                            .and_then(|n| side.slots.get(n))
                            .and_then(|s| s.clone())
                    })
                };
                let Some(item) = item.filter(|i| i.item_id != 0) else {
                    return Ok(());
                };
                {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    clear_content(&mut model, h);
                }
                fire_cleared(lua, h);
                let inst = render::ItemInstance {
                    name: item.name.clone(),
                    ..Default::default()
                };
                match view_of(lua, item.item_id) {
                    Some(v) => render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?,
                    None => {
                        if let Some(name) = item.name.clone() {
                            append_line(
                                lua,
                                &this,
                                (name, quality_color(item.quality.unwrap_or(1))),
                                None,
                                false,
                            )?;
                        }
                    }
                }
                show_or_hide_empty(lua, h);
                Ok(())
            })?,
        )?;
    }

    // GameTooltip:SetLootItem(slot): the loot-row hover (`0x533470`, `LootFrame.xml:42`, behind
    // `LootSlotIsItem`). A block source (`0x533564`, p6=1): a loot slot is no item object, so a
    // rolled suffix's enchants reach the builder through the block, never through
    // `ITEM_FIELD_ENCHANTMENT`. `slot` is the 1-based row; coin and cleared rows have no tooltip.
    m.set(
        "SetLootItem",
        lua.create_function(|lua, (this, slot): (Table, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let row = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .loot
                    .as_ref()
                    .and_then(|l| slot.checked_sub(1).and_then(|n| l.rows.get(n)))
                    .and_then(|r| r.clone())
                    .filter(|r| !r.is_coin && r.item_id != 0)
            };
            let Some(row) = row else {
                return Ok(());
            };
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // The row's app-composed name has the suffix joined, as `0x5d8b00` does; the lines
            // come from the roll id, the reference's `+0x424` (`0x52b7bf`).
            let inst = render::ItemInstance {
                name: row.name.clone(),
                enchants: roll_enchants(lua, row.random_property_id),
                ..Default::default()
            };
            match view_of(lua, row.item_id) {
                Some(v) => render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?,
                // Template in flight: the row's name, as `SetBagItem`'s miss path.
                None => {
                    if let Some(name) = row.name.clone() {
                        append_line(
                            lua,
                            &this,
                            (name, quality_color(row.quality.unwrap_or(1))),
                            None,
                            false,
                        )?;
                    }
                }
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetLootRollItem(rollId): the roll window's hover (`0x5364a0`,
    // `LootFrame.xml:343`). Like `SetLootItem`: p6=1 with `+0x424` = `[roll+0x20]`, the roll's
    // randomPropertyId, every enchant slot zero and no item object, so the suffix-row copy
    // (`0x52b7bf`) is the only enchant source.
    m.set(
        "SetLootRollItem",
        lua.create_function(|lua, (this, roll_id): (Table, u32)| {
            let h = frame_handle_of(lua, &this)?;
            let entry = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .loot_rolls
                    .rolls
                    .iter()
                    .find(|r| r.roll_id == roll_id)
                    .cloned()
                    .filter(|r| r.item_id != 0)
            };
            let Some(entry) = entry else {
                return Ok(());
            };
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            let inst = render::ItemInstance {
                name: entry.name.clone(),
                enchants: roll_enchants(lua, entry.random_property_id),
                ..Default::default()
            };
            match view_of(lua, entry.item_id) {
                Some(v) => render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?,
                None => {
                    if let Some(name) = entry.name.clone() {
                        append_line(
                            lua,
                            &this,
                            (name, quality_color(entry.quality.unwrap_or(1))),
                            None,
                            false,
                        )?;
                    }
                }
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetMerchantCompareItem(index [, offset]) (`0x536080`, method-table entry 43):
    // the shopping tooltip beside a vendor row. `offset` counts candidate slots, 1-based: set in
    // `0x809200[InventoryType]`, occupied, and holding an item of the offered class (never
    // subclass), so a worn shield makes offset 2 nil for a one-hand weapon and a worn wand compares
    // against a bow. One value on every non-raising path: the number 1 (`lua_pushnumber`) or nil.
    // A bad `this` or a non-number index raises; a missing `offset` is 1, one of 0 or less is nil.
    // An uncached template answers nil and fires the query, as in the reference.
    m.set(
        "SetMerchantCompareItem",
        lua.create_function(
            |lua, (this, index, offset): (Table, Value, Option<Value>)| {
                let h = frame_handle_of(lua, &this)?;
                let as_number = |v: &Value| -> Option<f64> {
                    match v {
                        Value::Integer(i) => Some(*i as f64),
                        Value::Number(n) => Some(*n),
                        // `lua_isnumber` accepts a numeric string, here as everywhere.
                        Value::String(s) => s.to_str().ok().and_then(|s| s.parse::<f64>().ok()),
                        _ => None,
                    }
                };
                // The one argument whose absence raises, with the reference's usage text.
                let Some(idx) = as_number(&index) else {
                    return Err(mlua::Error::runtime(
                        "Usage: SetMerchantCompareItem(\"slot\" [, offset])",
                    ));
                };
                // Re-based in f64, then truncated toward zero, as the reference does: 0.5 is row 1.
                let row = (idx - 1.0).trunc();
                if row.is_nan() || row < 0.0 || row > f64::from(u32::MAX) {
                    return Ok(Value::Nil);
                }
                // `offset` is truncated before it is re-based; absent, it is 1.
                let left = match offset.as_ref().and_then(as_number) {
                    Some(n) if n.is_nan() => return Ok(Value::Nil),
                    Some(n) => n.trunc() - 1.0,
                    None => 0.0,
                };
                if left < 0.0 {
                    return Ok(Value::Nil);
                }

                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let row = row as usize;
                let Some(item_id) = ({
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model
                        .merchant
                        .as_ref()
                        .and_then(|m| m.items.get(row))
                        .map(|it| it.item_id)
                }) else {
                    return Ok(Value::Nil);
                };
                let Some(offered) = view_of(lua, item_id) else {
                    return Ok(Value::Nil);
                };

                compare_against_worn(lua, &this, h, &offered, left)
            },
        )?,
    )?;

    // GameTooltip:SetAuctionCompareItem("type", index [, offset]) (`0x535d70`): the vendor
    // compare's body (`0x53603e` and `0x5362d4` both pass p4 = 0, p5 = 1) over an auction list.
    // `type` is `lua_isstring`-gated (`0x535e28`); a non-string type or non-number index raises
    // the reference's usage, and a bad list, row or uncached template is nil. The number 1 on
    // success, which `Blizzard_AuctionUI.lua:1081` gates each shopping tooltip on.
    m.set(
        "SetAuctionCompareItem",
        lua.create_function(
            |lua, (this, kind, index, offset): (Table, Value, Value, Option<Value>)| {
                let h = frame_handle_of(lua, &this)?;
                let as_number = |v: &Value| -> Option<f64> {
                    match v {
                        Value::Integer(i) => Some(*i as f64),
                        Value::Number(n) => Some(*n),
                        Value::String(s) => s.to_str().ok().and_then(|s| s.parse::<f64>().ok()),
                        _ => None,
                    }
                };
                let kind = match &kind {
                    Value::String(s) => s.to_str()?.to_string(),
                    Value::Integer(i) => i.to_string(),
                    Value::Number(n) => n.to_string(),
                    _ => {
                        return Err(mlua::Error::runtime(
                            "Usage: SetAuctionCompareItem(\"type\", index [, offset])",
                        ))
                    }
                };
                let Some(idx) = as_number(&index) else {
                    return Err(mlua::Error::runtime(
                        "Usage: SetAuctionCompareItem(\"type\", index [, offset])",
                    ));
                };
                let row = (idx - 1.0).trunc();
                if row.is_nan() || row < 0.0 || row > f64::from(u32::MAX) {
                    return Ok(Value::Nil);
                }
                let left = match offset.as_ref().and_then(as_number) {
                    Some(n) if n.is_nan() => return Ok(Value::Nil),
                    Some(n) => n.trunc() - 1.0,
                    None => 0.0,
                };
                if left < 0.0 {
                    return Ok(Value::Nil);
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let row = row as usize;
                let Some(item_id) = ({
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    let list = super::auction::list_index_of(&kind);
                    model
                        .auction
                        .as_ref()
                        .zip(list)
                        .and_then(|(a, l)| a.lists[l].rows.get(row))
                        .map(|r| r.item_id)
                }) else {
                    return Ok(Value::Nil);
                };
                let Some(offered) = view_of(lua, item_id) else {
                    return Ok(Value::Nil);
                };
                compare_against_worn(lua, &this, h, &offered, left)
            },
        )?,
    )?;

    // GameTooltip:SetMerchantItem(index) / SetBuybackItem(index): template sources, as the
    // reference's `SetMerchantItem` (`0x534080`) is.
    for (method, buyback) in [("SetMerchantItem", false), ("SetBuybackItem", true)] {
        m.set(
            method,
            lua.create_function(move |lua, (this, index): (Table, usize)| {
                let h = frame_handle_of(lua, &this)?;
                let row = {
                    let model = lua.app_data_mut::<Model>().expect("model app_data");
                    let Some(merchant) = &model.merchant else {
                        return Ok(());
                    };
                    let list = if buyback {
                        &merchant.buyback
                    } else {
                        &merchant.items
                    };
                    list.get(index.saturating_sub(1)).cloned()
                };
                let Some(row) = row else { return Ok(()) };
                {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    clear_content(&mut model, h);
                }
                fire_cleared(lua, h);
                match view_of(lua, row.item_id) {
                    Some(v) => {
                        render_view(lua, &this, &v, BuilderFlags::default(), None)?;
                        // Nothing to arm: the vendor tab's row seats `ShoppingTooltip1/2` itself
                        // (`MerchantFrame.xml:67-80`), and the buyback tab seats none.
                        if buyback {}
                    }
                    None => {
                        // Template in flight: the row's own stat head as a minimal view, the
                        // same deviation as `SetBagItem`'s miss path.
                        let head = row.stats.unwrap_or_default();
                        let v = ItemTemplateView {
                            name: row.name.clone().unwrap_or_default(),
                            quality: head.quality,
                            class: head.class,
                            subclass: head.subclass,
                            inventory_type: head.inventory_type,
                            damages: if head.dmg_max > 0.0 {
                                vec![(head.dmg_min, head.dmg_max, head.dmg_type)]
                            } else {
                                Vec::new()
                            },
                            delay_ms: head.delay_ms,
                            armor: head.armor,
                            block: head.block,
                            ..Default::default()
                        };
                        render_view(lua, &this, &v, BuilderFlags::default(), None)?;
                    }
                }
                show_or_hide_empty(lua, h);
                Ok(())
            })?,
        )?;
    }

    // GameTooltip:SetInboxItem(index): the mail item hover (`MailFrame.lua:218`,
    // `MailFrame.lua:470`). A row with no item does nothing; stock calls it only when there is one.
    m.set(
        "SetInboxItem",
        lua.create_function(|lua, (this, index): (Table, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, roll, name) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                let Some(mail) = &model.mail else {
                    return Ok(());
                };
                match mail.inbox.get(index.saturating_sub(1)) {
                    Some(r) => (r.item_id, r.item_random_property_id, r.item_name.clone()),
                    None => (0, 0, None),
                }
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // A block source (`0x5355fa`, p6=1) carrying the attachment's roll: no placeholder.
            let inst = render::ItemInstance {
                name,
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            if let Some(v) = view_of(lua, item_id) {
                render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?;
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetAuctionItem(type, index): an auction row's hover, `type` "list", "bidder" or
    // "owner". No creator, gift or open line: the reference zeroes the guid arguments
    // (`0x535a6d`, `0x535a73`), so the item-object gate those lines hang off fails.
    m.set(
        "SetAuctionItem",
        lua.create_function(|lua, (this, kind, index): (Table, String, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, roll, name) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                let Some(auction) = &model.auction else {
                    return Ok(());
                };
                let Some(list) = super::auction::list_index_of(&kind) else {
                    return Ok(());
                };
                match auction.lists[list].rows.get(index.saturating_sub(1)) {
                    Some(r) => (r.item_id, r.random_property_id, r.name.clone()),
                    None => (0, 0, None),
                }
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            // A block source (`0x5359d9`, p6=1) carrying the listing's roll: no placeholder.
            let inst = render::ItemInstance {
                name,
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            if let Some(v) = view_of(lua, item_id) {
                render_view(lua, &this, &v, BuilderFlags::default(), Some(&inst))?;
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetAuctionSellItem(): the sell slot's hover (`Blizzard_AuctionUI.xml:1791`); an
    // empty slot does nothing. The reference resolves the live sell-slot object (`[0xb72608]`,
    // `[0xb7260c]`, `0x5357b4`), so its creator line can show; this renders the template alone.
    m.set(
        "SetAuctionSellItem",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let item_id = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .auction_sell_item
                    .as_ref()
                    .map(|it| it.item_id)
                    .unwrap_or(0)
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            if let Some(v) = view_of(lua, item_id) {
                render_view(lua, &this, &v, BuilderFlags::default(), None)?;
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetSendMailItem(): the attached item's hover (`MailFrame.xml:952`); an empty
    // slot does nothing.
    m.set(
        "SetSendMailItem",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            let item_id = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                model
                    .mail_send_item
                    .as_ref()
                    .map(|it| it.item_id)
                    .unwrap_or(0)
            };
            if item_id == 0 {
                return Ok(());
            }
            {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                clear_content(&mut model, h);
            }
            fire_cleared(lua, h);
            if let Some(v) = view_of(lua, item_id) {
                render_view(lua, &this, &v, BuilderFlags::default(), None)?;
            }
            show_or_hide_empty(lua, h);
            Ok(())
        })?,
    )?;

    // GameTooltip:SetTradeSkillItem(skillIndex [, reagentIndex]): the reagent's item, or without
    // `reagentIndex` the recipe's product. A product id of 0 or an uncached template shows the
    // recipe's or the reagent's name alone (`render_by_id`); a bad index does nothing.
    m.set(
        "SetTradeSkillItem",
        lua.create_function(
            |lua, (this, skill_index, reagent_index): (Table, usize, Option<usize>)| {
                let found = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    // A visible-row index, headers included (`0x4fca20`); a header is None.
                    let Some(recipe) = super::tradeskill::recipe_at(&model, skill_index) else {
                        return Ok(());
                    };
                    match reagent_index {
                        Some(ri) => ri
                            .checked_sub(1)
                            .and_then(|i| recipe.reagents.get(i))
                            .map(|r| (r.item, r.name.clone())),
                        None => Some((recipe.product_item, Some(recipe.name.clone()))),
                    }
                };
                let Some((item_id, fb_name)) = found else {
                    return Ok(());
                };
                render_by_id(lua, &this, item_id, fb_name, None)
            },
        )?,
    )?;

    // GameTooltip:SetCraftItem(craftIndex, reagentIndex): the reagent's item
    // (`Blizzard_CraftUI.xml:33`); a craft has no product item, so both arguments are required. A
    // bad index does nothing.
    m.set(
        "SetCraftItem",
        lua.create_function(
            |lua, (this, craft_index, reagent_index): (Table, usize, usize)| {
                let found = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    let Some(c) = &model.craft else {
                        return Ok(());
                    };
                    let Some(recipe) = craft_index.checked_sub(1).and_then(|i| c.recipes.get(i))
                    else {
                        return Ok(());
                    };
                    reagent_index
                        .checked_sub(1)
                        .and_then(|i| recipe.reagents.get(i))
                        .map(|r| (r.item, r.name.clone()))
                };
                let Some((item_id, fb_name)) = found else {
                    return Ok(());
                };
                render_by_id(lua, &this, item_id, fb_name, None)
            },
        )?,
    )?;

    Ok(())
}
