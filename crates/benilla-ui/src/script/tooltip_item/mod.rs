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

use super::binding_abi::flag;
use super::object::frame_handle_of;
use super::tooltip::clear_silent;
use super::{ItemTemplateView, Model};

pub(in crate::script) mod build;
pub(in crate::script) mod names;
pub(in crate::script) mod render;

use build::{build_item, InFlight, ItemBuild};
use names::quality_color;
use render::BuilderFlags;

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

/// A compare's offered template; a miss asks with the tooltip as the answer's cookie (`0x5361ee`),
/// so the answer rebuilds whatever item the tooltip keeps then.
fn offered_view(
    lua: &Lua,
    h: crate::widget::FrameHandle,
    item_id: u32,
) -> Option<ItemTemplateView> {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .item_cache_misses
        .clear();
    let v = view_of(lua, item_id);
    build::keep_misses(
        &mut lua.app_data_mut::<Model>().expect("model app_data"),
        h,
        false,
    );
    v
}

/// The cached template; a miss records the ask and waits for its answer (`0x52b716`).
fn view_of(lua: &Lua, item_id: u32) -> Option<ItemTemplateView> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let v = model.item_templates.get(&item_id).cloned();
    if v.is_none() && item_id != 0 {
        model.item_stat_asks.insert(item_id);
        build::note_miss(&mut model, build::CacheWait::Template(item_id));
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
            .record_lua_error(&e);
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

/// A bag slot's item as the builder's live item object: the name off the link (the reference
/// builds both from `0x5d8b00`), its fields and the open gate's p6.
fn slot_instance(s: &crate::script::ContainerSlot, openable_source: bool) -> render::ItemInstance {
    render::ItemInstance {
        name: s.link.as_deref().and_then(link_name).map(str::to_string),
        durability: s.durability,
        creator: s.creator.clone(),
        gift_creator: s.gift_creator.clone(),
        creator_guid: s.creator_guid,
        gift_creator_guid: s.gift_creator_guid,
        guid: s.guid,
        has_text: s.readable,
        flags: s.flags,
        already_bound: s.already_bound,
        petition: s.petition.clone(),
        enchants: s.enchants.clone(),
        openable_source,
        duration_ms: s.duration_ms,
        object: true,
        block_lock_id: 0,
        block_wrapped: false,
        cooldown_ms: 0,
    }
}

/// The player's own item of entry `item_id` as the builder's live item object, found as the
/// action bar's item word finds it (`0x532490` → `0x622270` → the walk `0x622420`, mask `0x47`
/// because `0x5dd44d` sets `[player+0x1d48]`): the worn items (positions 0..18), then each
/// equipped bag (19..22) followed by its contents, then the backpack (23..38), then the keyring
/// (81..112). The bank is out of the mask. A broken item (durability 0 with a maximum, unless
/// wrapped, `0x6224f7`..`0x62250e`) or one carrying flag `0x10` (`0x622510`) is skipped. The
/// reference also skips an object whose `[+0xe4]` bit 0 is set (`0x614ea0`); benilla holds no
/// such fact. `None` when nothing matches, where the setter pushes nil (`0x5325bc`).
pub(super) fn owned_instance(model: &Model, item_id: u32) -> Option<render::ItemInstance> {
    const BROKEN: u32 = 0x10;
    const WRAPPED: u32 = 0x8;
    let usable = |flags: u32, durability: Option<(u32, u32)>| {
        let broken =
            flags & WRAPPED == 0 && durability.is_some_and(|(cur, max)| max > 0 && cur == 0);
        !broken && flags & BROKEN == 0
    };
    let worn = |s: &crate::script::InvSlotView| {
        (s.item_id == item_id && usable(s.flags, s.durability)).then(|| worn_instance(s, true))
    };
    let held = |bag: i64| -> Option<render::ItemInstance> {
        let c = model.containers.get(&bag)?;
        let mut slots: Vec<_> = c.slots.iter().collect();
        slots.sort_by_key(|(n, _)| **n);
        slots
            .into_iter()
            .map(|(_, s)| s)
            .find(|s| s.item_id == item_id && usable(s.flags, s.durability))
            .map(|s| slot_instance(s, false))
    };
    // Equipment, then each equipped bag item and straight after it that bag's contents.
    for s in model.inventory_slots.iter().take(20).skip(1).flatten() {
        if let Some(i) = worn(s) {
            return Some(i);
        }
    }
    for bag in 1..=4_i64 {
        if let Some(i) = model
            .inventory_slots
            .get(19 + bag as usize)
            .and_then(Option::as_ref)
            .and_then(worn)
        {
            return Some(i);
        }
        if let Some(i) = held(bag) {
            return Some(i);
        }
    }
    held(0).or_else(|| held(super::char_stats::KEYRING_CONTAINER))
}

/// A worn (or inspected) slot's item as the builder's item: an object of the player's own, else
/// the visible-item record (`0x533354`), which has no durability or creator to show.
pub(super) fn worn_instance(s: &crate::script::InvSlotView, object: bool) -> render::ItemInstance {
    render::ItemInstance {
        // App-composed, with the suffix off `ITEM_FIELD_RANDOM_PROPERTIES_ID`.
        name: s.name.clone(),
        durability: s.durability,
        creator: s.creator.clone(),
        gift_creator: s.gift_creator.clone(),
        creator_guid: s.creator_guid,
        gift_creator_guid: s.gift_creator_guid,
        guid: if object { s.guid } else { 0 },
        flags: s.flags,
        already_bound: s.already_bound,
        // A charter has `InventoryType` 0 and is never equipped.
        petition: None,
        enchants: s.enchants.clone(),
        duration_ms: s.duration_ms,
        object,
        ..Default::default()
    }
}

/// The item at `(bag, slot)` in the cursor's numbering, as a live object with no block, for the
/// sell and send-mail slots (`0x5357c3`, `0x5353a0`).
fn held_instance(model: &Model, bag: i64, slot: u32) -> Option<render::ItemInstance> {
    model
        .containers
        .get(&bag)
        .and_then(|c| c.slots.get(&slot))
        .map(|s| slot_instance(s, true))
}

/// The id-keyed render; on a miss, the ask and a name-only line, as `SetBagItem`'s miss path. The
/// engine's own item hovers reach it from Rust (the action bar's, `SetTrainerService`,
/// `SetCraftSpell`): 1.12's `Set*Item` bindings all name a container, so no Lua method takes an id.
pub(super) fn render_by_id(
    lua: &Lua,
    this: &Table,
    item_id: u32,
    fb_name: Option<String>,
    fb_q: Option<u32>,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    let in_flight = match fb_name {
        Some(name) => InFlight::Name(name, quality_color(fb_q.unwrap_or(1))),
        None => InFlight::Nothing,
    };
    build_item(
        lua,
        this,
        h,
        ItemBuild {
            in_flight,
            ..ItemBuild::new(item_id)
        },
    )
    .map(|_| ())
}

/// The player's own item of entry `item_id` through the builder, `inst` the object
/// [`owned_instance`] found, if any; answers the builder's return value (see
/// [`render::render_view_flagged`]), false while the template is in flight.
pub(super) fn render_owned(
    lua: &Lua,
    this: &Table,
    item_id: u32,
    inst: Option<render::ItemInstance>,
) -> mlua::Result<bool> {
    let h = frame_handle_of(lua, this)?;
    build_item(
        lua,
        this,
        h,
        ItemBuild {
            inst,
            ..ItemBuild::new(item_id)
        },
    )
}

/// A block source with no name to wait on (a loot row, a roll): `inst`, its own name in the
/// row's quality while the template is in flight.
fn build_block(
    lua: &Lua,
    this: &Table,
    h: crate::widget::FrameHandle,
    item_id: u32,
    inst: render::ItemInstance,
    quality: Option<u32>,
) -> mlua::Result<bool> {
    let in_flight = match inst.name.clone() {
        Some(name) => InFlight::Name(name, quality_color(quality.unwrap_or(1))),
        None => InFlight::Nothing,
    };
    build_item(
        lua,
        this,
        h,
        ItemBuild {
            inst: Some(inst),
            in_flight,
            ..ItemBuild::new(item_id)
        },
    )
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
            clear_silent(lua, h);
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
                crate::script::quest_log::selected_quest_item(&model, &kind, index)
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
            // The reference's format is `"%s|Hitem:%d:%d:%d:%d|h[%s]|h%s"` (`0x8549c8`), and
            // `SetHyperlink` (`0x532181`) parses it into a block (p6=1): the enchant id to slot 0
            // (`+0x3d0`), the roll to `+0x424` (expanded into slots 2..6 by `0x52b7bf`), the
            // unique id to `+0x420`, unread; a missing field is 0. So a link shows its roll's
            // lines, never the placeholder (`0x52cc33`). The name is the link's own, built with
            // the suffix joined (`0x5d8b00`). The enchant id is parsed, not rendered: naming it
            // needs its `SpellItemEnchantment` row, and no link benilla composes carries one.
            // The block's creator and giver are zeroed (`0x5321ad`, `0x5321b3`), so a link never
            // shows a maker line.
            let name = link_name(&link).map(str::to_string);
            let inst = render::ItemInstance {
                name: name.clone(),
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            let in_flight = match name {
                Some(name) => InFlight::Name(name, quality_color(1)),
                None => InFlight::Nothing,
            };
            build_item(
                lua,
                &this,
                h,
                ItemBuild {
                    inst: Some(inst),
                    in_flight,
                    ..ItemBuild::new(id)
                },
            )?;
            Ok(())
        })?,
    )?;

    // GameTooltip:SetInventoryItem(unit, slot [, nameOnly]) -> hasItem, hasCooldown, repairCost
    // (`0x532ee0`), three as `PaperDollFrame.lua:741` reads them; the arity is that consumer's,
    // not byte-read as `SetBagItem`'s two is (`0x534985`). Unit-keyed through `Model::inv_slot`:
    // an inspected item (`InspectPaperDollFrame.xml:20`) has no durability or creator, so those
    // lines do not show, as in the reference. An armed shopping tooltip renders the compare
    // shape; the arm is consumed either way. repairCost is the item's cost (`0x5332fb`), 0 for an
    // inspected unit's.
    m.set(
        "SetInventoryItem",
        lua.create_function(
            |lua, (this, unit, slot, name_only): (Table, String, usize, Value)| {
                let h = frame_handle_of(lua, &this)?;
                // `nameOnly` (usage string `0x8552dc`) is set only by a number above 0
                // (`0x533027..0x53304c`); anything else builds the ordinary tooltip, without an
                // error. Three of the binding's four builder legs pass it on, not `0x533106`.
                let name_only = super::binding_abi::positive_number_flag(lua, name_only)?;
                let (item_id, name, quality, inst, repair_cost, currently_equipped) = {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    let armed = match super::tooltip::tip_mut(&mut model, h) {
                        Ok(t) => std::mem::take(&mut t.equipped_header_armed),
                        Err(_) => false,
                    };
                    let view = model
                        .inv_slot(&unit, slot)
                        .filter(|s| s.item_id != 0)
                        .map(|s| {
                            (s.item_id, s.name.clone(), s.quality, {
                                // Only the player's own doll holds item objects; an
                                // inspected unit's item is a block (`0x532ee0`). `0x532ee0`
                                // has p6=0 legs too (`0x533106`, `0x5332ad`), but nothing
                                // openable is equippable.
                                let player = unit.eq_ignore_ascii_case("player");
                                let mut inst = worn_instance(&s, player);
                                // The player's own item answers the cooldown query
                                // (`0x5331ec`), unless it is charged and spent
                                // (`0x53318a`-`0x53319b`, the feed's `None`); an inspected
                                // record has none.
                                if player {
                                    inst.cooldown_ms = super::tooltip_spell::live_remaining_ms(
                                        model.inv_slot_cooldown(slot),
                                        model.now,
                                    );
                                }
                                inst
                            })
                        });
                    let cost = if unit.eq_ignore_ascii_case("player") {
                        model.inv_repair_cost(slot)
                    } else {
                        0
                    };
                    match view {
                        Some((id, name, q, inst)) => (id, name, q, inst, cost, armed),
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
                let duration_ms = inst.duration_ms;
                // Template in flight: the header and the slot's own name, as `SetBagItem`'s
                // miss path.
                let in_flight = match name {
                    Some(name) => InFlight::Name(name, quality_color(quality.max(0) as u32)),
                    None => InFlight::Nothing,
                };
                let live = build_item(
                    lua,
                    &this,
                    h,
                    ItemBuild {
                        flags,
                        inst: Some(inst),
                        in_flight,
                        ..ItemBuild::new(item_id)
                    },
                )?;
                Ok(MultiValue::from_vec(vec![
                    Value::Integer(1),
                    // hasCooldown is the builder's `[ebp-0x38]`, set by four lines: LOCKED, the
                    // temporary-enchant countdown, the item's duration (`0x52ce0d`) and
                    // ITEM_COOLDOWN_TIME (`0x52e140`). A template still in flight draws only the
                    // name, but the slot's own expiry still counts.
                    flag(live || duration_ms.is_some()),
                    Value::Integer(i64::from(repair_cost)),
                ]))
            },
        )?,
    )?;

    // GameTooltip:SetBagItem(bag, slot) -> hasCooldown, repairCost: the real-instance hover and
    // the one money source: with the merchant open and repair off (`0x52e376`), the engine fires
    // `OnTooltipAddMoney(SellPrice × stack)`, or prints ITEM_UNSELLABLE at price 0 (`0x854a74`,
    // pushed at `0x52e4a3`). repairCost is the item's cost (`0x534975`), shown in repair mode.
    m.set(
        "SetBagItem",
        lua.create_function(|lua, (this, bag, slot): (Table, i64, u32)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, count, link, quality, inst, repair_cost) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                let cost = model.repair_costs.bags.get(&(bag, slot)).copied();
                match model
                    .containers
                    .get(&bag)
                    .and_then(|c| c.slots.get(&slot))
                    .filter(|s| s.item_id != 0)
                {
                    // `0x534620` takes the p6=1 leg iff the cooldown query `0x6e2ed0` reports
                    // enable, start and duration all non-zero; that boolean is the inverse of the
                    // openable gate. `hasCooldown` is the builder's own return. A charged
                    // template whose copy has no charge left skips the query for the p6=0 leg
                    // (`0x5347e3`-`0x5347f4`); the feed leaves such a slot's cooldown `None`.
                    Some(s) => {
                        let has_cd = s
                            .cooldown
                            .is_some_and(|(start, dur, en)| en && start > 0 && dur > 0);
                        // `start + duration - now` (`0x534882`..`0x53488c`), into `+0x408`.
                        let mut inst = slot_instance(s, !has_cd);
                        inst.cooldown_ms = super::tooltip_spell::live_remaining_ms(
                            model.container_cooldowns.get(&(bag, slot)).copied(),
                            model.now,
                        );
                        (
                            s.item_id,
                            s.count.max(1),
                            s.link.clone(),
                            s.quality.unwrap_or(1),
                            // p6 = 0 iff no cooldown runs; mid-cooldown ITEM_COOLDOWN_TIME takes
                            // the openable line's place.
                            inst,
                            cost.unwrap_or(0),
                        )
                    }
                    None => return Ok(MultiValue::from_vec(vec![Value::Nil])),
                }
            };
            let duration_ms = inst.duration_ms;
            // The template in flight shows the slot's name ([`InFlight`]).
            let in_flight = match link.as_deref().and_then(link_name) {
                Some(name) => InFlight::Name(name.to_string(), quality_color(quality)),
                None => InFlight::Nothing,
            };
            let live = build_item(
                lua,
                &this,
                h,
                ItemBuild {
                    inst: Some(inst),
                    in_flight,
                    sell_count: Some(count),
                    ..ItemBuild::new(item_id)
                },
            )?;
            Ok(MultiValue::from_vec(vec![
                // The builder's `[ebp-0x38]`: LOCKED, a countdown, the item's duration line, or a
                // running cooldown (ITEM_COOLDOWN_TIME, `0x52e140`).
                flag(live || duration_ms.is_some()),
                Value::Integer(i64::from(repair_cost)),
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
                let inst = render::ItemInstance {
                    name: item.name.clone(),
                    // The target side's block carries the partner's lock id (`0x534563`), wrapped
                    // word (`0x534559`) and gift creator (`0x534574`); the player's lock id is
                    // zero (`0x534341`), but its slot is a live item object, whose flags the
                    // LOCKED and wrapped gates read (`0x5342ed`, `0x5343da`, `0x52b789`).
                    block_lock_id: if player_side { 0 } else { item.lock_id },
                    block_wrapped: !player_side && item.wrapped,
                    gift_creator: item.gift_creator.clone(),
                    gift_creator_guid: item.gift_creator_guid,
                    object: item.object_flags.is_some(),
                    flags: item.object_flags.unwrap_or(0),
                    ..Default::default()
                };
                let in_flight = match item.name {
                    Some(name) => InFlight::Name(name, quality_color(item.quality.unwrap_or(1))),
                    None => InFlight::Nothing,
                };
                build_item(
                    lua,
                    &this,
                    h,
                    ItemBuild {
                        inst: Some(inst),
                        in_flight,
                        ..ItemBuild::new(item.item_id)
                    },
                )?;
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
            // The row's app-composed name has the suffix joined, as `0x5d8b00` does; the lines
            // come from the roll id, the reference's `+0x424` (`0x52b7bf`).
            let inst = render::ItemInstance {
                name: row.name.clone(),
                enchants: roll_enchants(lua, row.random_property_id),
                ..Default::default()
            };
            build_block(lua, &this, h, row.item_id, inst, row.quality)?;
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
            let inst = render::ItemInstance {
                name: entry.name.clone(),
                enchants: roll_enchants(lua, entry.random_property_id),
                ..Default::default()
            };
            build_block(lua, &this, h, entry.item_id, inst, entry.quality)?;
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
                let Some(offered) = offered_view(lua, h, item_id) else {
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
                // `lua_isstring` then `lua_tostring` (`0x535e28`, `0x535e50`).
                let Some(kind) = crate::script::binding_abi::optional_string(lua, &kind) else {
                    return Err(mlua::Error::runtime(
                        "Usage: SetAuctionCompareItem(\"type\", index [, offset])",
                    ));
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
                let Some(offered) = offered_view(lua, h, item_id) else {
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
                // Nothing to arm: the vendor tab's row seats `ShoppingTooltip1/2` itself
                // (`MerchantFrame.xml:67-80`), and the buyback tab seats none. Template in
                // flight: the row's own stat head as a minimal view ([`InFlight`]).
                let head = row.stats.unwrap_or_default();
                let in_flight = InFlight::View(Box::new(ItemTemplateView {
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
                }));
                build_item(
                    lua,
                    &this,
                    h,
                    ItemBuild {
                        in_flight,
                        ..ItemBuild::new(row.item_id)
                    },
                )?;
                Ok(())
            })?,
        )?;
    }

    // GameTooltip:SetInboxItem(index): the mail item hover (`MailFrame.lua:218`,
    // `MailFrame.lua:470`). A row with no item draws nothing and answers nil; stock calls it only
    // when there is one. The answer is the builder's return value, 1 or nil (`0x5356bb`).
    m.set(
        "SetInboxItem",
        lua.create_function(|lua, (this, index): (Table, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, roll, name) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                let Some(mail) = &model.mail else {
                    return Ok(Value::Nil);
                };
                match mail.inbox.get(index.saturating_sub(1)) {
                    Some(r) => (r.item_id, r.item_random_property_id, r.item_name.clone()),
                    None => (0, 0, None),
                }
            };
            if item_id == 0 {
                return Ok(Value::Nil);
            }
            // A block source (`0x5355fa`, p6=1) carrying the attachment's roll: no placeholder.
            let inst = render::ItemInstance {
                name,
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            let live = build_item(
                lua,
                &this,
                h,
                ItemBuild {
                    inst: Some(inst),
                    ..ItemBuild::new(item_id)
                },
            )?;
            Ok(flag(live))
        })?,
    )?;

    // GameTooltip:SetAuctionItem(type, index): an auction row's hover, `type` "list", "bidder" or
    // "owner". No creator, gift or open line: the reference zeroes the guid arguments
    // (`0x535a6d`, `0x535a73`), so the item-object gate those lines hang off fails. It answers the
    // builder's return value, 1 or nil; a miss answers nil.
    m.set(
        "SetAuctionItem",
        lua.create_function(|lua, (this, kind, index): (Table, String, usize)| {
            let h = frame_handle_of(lua, &this)?;
            let (item_id, roll, name) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                let Some(auction) = &model.auction else {
                    return Ok(Value::Nil);
                };
                let Some(list) = super::auction::list_index_of(&kind) else {
                    return Ok(Value::Nil);
                };
                match auction.lists[list].rows.get(index.saturating_sub(1)) {
                    Some(r) => (r.item_id, r.random_property_id, r.name.clone()),
                    None => (0, 0, None),
                }
            };
            if item_id == 0 {
                return Ok(Value::Nil);
            }
            // A block source (`0x5359d9`, p6=1) carrying the listing's roll: no placeholder.
            let inst = render::ItemInstance {
                name,
                enchants: roll_enchants(lua, roll),
                ..Default::default()
            };
            let live = build_item(
                lua,
                &this,
                h,
                ItemBuild {
                    inst: Some(inst),
                    ..ItemBuild::new(item_id)
                },
            )?;
            Ok(flag(live))
        })?,
    )?;

    // GameTooltip:SetAuctionSellItem(): the sell slot's hover (`Blizzard_AuctionUI.xml:1791`); an
    // empty slot does nothing and answers nil. The reference resolves the live sell-slot object
    // (`[0xb72608]`, `[0xb7260c]`, `0x5357b4`), so its creator line can show; this renders the
    // template alone. It answers the builder's return value, 1 or nil (`0x5357da`).
    m.set(
        "SetAuctionSellItem",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            // The slot's item is a live object (`0x5357b4`, `0x5353a0`), so its LOCKED gate,
            // expiry and temporary enchants follow the bag slot it came from.
            let (item_id, inst) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match model.auction_sell_item.as_ref() {
                    Some(it) => (it.item_id, held_instance(&model, it.bag, it.slot)),
                    None => (0, None),
                }
            };
            if item_id == 0 {
                return Ok(Value::Nil);
            }
            let live = build_item(
                lua,
                &this,
                h,
                ItemBuild {
                    inst,
                    ..ItemBuild::new(item_id)
                },
            )?;
            Ok(flag(live))
        })?,
    )?;

    // GameTooltip:SetSendMailItem(): the attached item's hover (`MailFrame.xml:952`); an empty
    // slot does nothing and answers nil. It answers the builder's return value, 1 or nil
    // (`0x53548a`).
    m.set(
        "SetSendMailItem",
        lua.create_function(|lua, this: Table| {
            let h = frame_handle_of(lua, &this)?;
            // The slot's item is a live object (`0x5357b4`, `0x5353a0`), so its LOCKED gate,
            // expiry and temporary enchants follow the bag slot it came from.
            let (item_id, inst) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match model.mail_send_item.as_ref() {
                    Some(it) => (it.item_id, held_instance(&model, it.bag, it.slot)),
                    None => (0, None),
                }
            };
            if item_id == 0 {
                return Ok(Value::Nil);
            }
            let live = build_item(
                lua,
                &this,
                h,
                ItemBuild {
                    inst,
                    ..ItemBuild::new(item_id)
                },
            )?;
            Ok(flag(live))
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
