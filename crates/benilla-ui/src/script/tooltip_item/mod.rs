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

use super::binding_abi::{self, flag};
use super::object::frame_handle_of;
use super::tooltip::clear_silent;
use super::{ItemTemplateView, Model};

pub(in crate::script) mod build;
mod compare;
pub(in crate::script) mod names;
pub(in crate::script) mod render;

use build::{build_item, InFlight, ItemBuild};
use names::quality_color;
use render::BuilderFlags;

/// The cached template as the build reads it at the call, its spell lines expanded and its
/// reputation line gendered against the player as it stands (`0x5075f0` at `0x52da24`,
/// `0x52d80f`); a miss is noted for the build, which asks it with the tooltip as the cookie
/// inside its call and waits for its answer (`0x52b716`, [`build::ask_tooltip_records`]).
fn view_of(lua: &Lua, item_id: u32) -> Option<ItemTemplateView> {
    let live = crate::script::item_stats::live_record(lua, item_id);
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let v = live.unwrap_or_else(|| model.item_templates.get(&item_id).cloned());
    if v.is_none() && item_id != 0 {
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
fn fire_add_money(lua: &Lua, h: crate::widget::FrameHandle, copper: i64) {
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

/// `SStrCmpI(link, prefix, len)` (`0x64a4c0`) answering 0: the link starts with `prefix`, ASCII
/// case aside; the rest of the link.
fn link_tail<'a>(link: &'a str, prefix: &str) -> Option<&'a str> {
    let head = link.as_bytes().get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix.as_bytes())
        .then(|| &link[prefix.len()..])
}

/// An `item:` link's id and its `<enchant>:<random property>` fields, each `SStrToInt`
/// (`0x64ac60`) after the next `:` (`0x5320fa`..`0x53214d`), a missing one 0; signed, as the
/// reference keeps them.
fn hyperlink_item_fields(tail: &str) -> (i32, i32, i32) {
    let mut fields = tail.split(':').map(binding_abi::s_str_to_int);
    let mut next = || fields.next().unwrap_or(0);
    (next(), next(), next())
}

/// The spell record's `castUI` (column 3, `+0xc`) through the lent host, which an `enchant:` link
/// tests against 3 (`0x532243`): `Some(None)` for an id with no record, `None` with no host to ask.
pub struct SpellCastUiQuery(pub u32);

impl super::live::LiveQuery for SpellCastUiQuery {
    type Answer = Option<u32>;
}

/// A block's bare enchant id in `slot` as its line (`0x52c9f2`..`0x52ca46`): `abs(id)` names the
/// `SpellItemEnchantment` row, and a negative id paints slots 0 and 1 red. An id with no named row
/// adds nothing.
fn block_enchant(lua: &Lua, slot: u8, id: i32) -> Option<crate::script::EnchantView> {
    let model = lua.app_data_ref::<Model>().expect("model app_data");
    let name = model.enchant_names.get(&id.unsigned_abs())?.clone();
    (id != 0).then_some(crate::script::EnchantView {
        slot,
        negative: id < 0,
        name,
        ..Default::default()
    })
}

/// A bag slot's item as the builder's live item object: the name off the link (the reference
/// builds both from `0x5d8b00`), its fields and the open gate's p6.
fn slot_instance(s: &crate::script::ContainerSlot, openable_source: bool) -> render::ItemInstance {
    render::ItemInstance {
        name: s.link.as_deref().and_then(link_name).map(str::to_string),
        roll: 0,
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
        proposed_enchant: None,
        cooldown_ms: 0,
        object_charges: s.spell_charges,
        block_charges: None,
        block_durability: (0, 0),
    }
}

/// The player's own item of entry `item_id` as the builder's live item object, found as the
/// action bar's item word finds it (`0x532490` → `0x622270` → the walk `0x622420`, mask `0x47`
/// because `0x5dd44d` sets `[player+0x1d48]`): the worn items (positions 0..18), then each
/// equipped bag (19..22) followed by its contents, then the backpack (23..38), then the keyring
/// (81..112). The bank is out of the mask. A broken item (durability 0 with a maximum, unless
/// wrapped, `0x6224f7`..`0x62250e`) or one carrying flag `0x10` (`0x622510`) is skipped. The
/// reference also skips an object whose `[+0xe4]` bit 0 is set (`0x614ea0`); benilla holds no
/// such fact. `None` when nothing matches, where the setter pushes nil (`0x5325bc`). When the
/// cached template's block-0 charges (`+0x144`, `0x53251a`) are neither -1 nor 0, the block's
/// charges are the summed absolute block-0 charges of every copy the same walk reaches
/// (`0x622180` at `0x532553`, stored at `+0x428`); otherwise, and with no template cached
/// (`0x532518`), they stay -1 and the builder reads the object's own.
pub(super) fn owned_instance(
    lua: &Lua,
    model: &Model,
    item_id: u32,
) -> Option<render::ItemInstance> {
    const BROKEN: u32 = 0x10;
    const WRAPPED: u32 = 0x8;
    let usable = |flags: u32, durability: Option<(u32, u32)>| {
        let broken =
            flags & WRAPPED == 0 && durability.is_some_and(|(cur, max)| max > 0 && cur == 0);
        !broken && flags & BROKEN == 0
    };
    let worn_copy =
        |s: &&crate::script::InvSlotView| s.item_id == item_id && usable(s.flags, s.durability);
    let held_copy =
        |s: &&crate::script::ContainerSlot| s.item_id == item_id && usable(s.flags, s.durability);
    // Every bag slot now, in bag and slot order.
    let bags = crate::script::held::bag_items(lua, model);
    let held = |bag: i64| -> Option<render::ItemInstance> {
        bags.iter()
            .filter(|(b, _, _)| *b == bag)
            .map(|(_, _, s)| s)
            .find(held_copy)
            .map(|s| slot_instance(s, false))
    };
    // The worn slots now, by live id (`0x6228a0`): the doll's keys, and a slot's view read once,
    // only where its item is this one.
    let keys = crate::script::worn::worn_keys(lua, model);
    let views: [std::cell::OnceCell<Option<crate::script::InvSlotView>>;
        crate::script::INVENTORY_SLOT_COUNT] = Default::default();
    let worn_view = |i: usize| -> Option<&crate::script::InvSlotView> {
        keys.get(i)
            .copied()
            .flatten()
            .filter(|&(_, id)| id == item_id)?;
        views[i]
            .get_or_init(|| crate::script::worn::worn_slot(lua, model, i))
            .as_ref()
    };
    let worn = |i: usize| {
        worn_view(i)
            .filter(worn_copy)
            .map(|s| worn_instance(s, true))
    };
    // Equipment, then each equipped bag item and straight after it that bag's contents.
    let found = (1..20)
        .find_map(worn)
        .or_else(|| (1..=4_i64).find_map(|bag| worn(19 + bag as usize).or_else(|| held(bag))))
        .or_else(|| held(0))
        .or_else(|| held(super::char_stats::KEYRING_CONTAINER))?;
    let summed = model
        .item_templates
        .get(&item_id)
        .is_some_and(|v| !matches!(v.spell_charges_0, -1 | 0));
    if !summed {
        return Some(found);
    }
    let worn_charges = (1..crate::script::INVENTORY_SLOT_COUNT).filter_map(worn_view);
    let held_charges = bags
        .iter()
        .filter(|(b, _, _)| (0..=4).contains(b) || *b == super::char_stats::KEYRING_CONTAINER)
        .map(|(_, _, s)| s);
    let sum = worn_charges
        .filter(worn_copy)
        .map(|s| s.spell_charges[0])
        .chain(held_charges.filter(held_copy).map(|s| s.spell_charges[0]))
        .fold(0_i32, |sum, n| sum.wrapping_add(n.wrapping_abs()));
    Some(render::ItemInstance {
        block_charges: Some(sum),
        ..found
    })
}

/// A worn (or inspected) slot's item as the builder's item: an object of the player's own, else
/// the visible-item record (`0x533354`), which has no durability to show and whose creator guid
/// the leg copies to `+0x410` (`0x5333cd`).
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
        object_charges: s.spell_charges,
        ..Default::default()
    }
}

/// The item at `(bag, slot)` in the cursor's numbering, as a live object with no block, for the
/// sell and send-mail slots (`0x5357c3`, `0x5353a0`).
fn held_instance(lua: &Lua, model: &Model, bag: i64, slot: u32) -> Option<render::ItemInstance> {
    crate::script::held::held_slot(lua, model, bag, slot).map(|s| slot_instance(&s, true))
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
/// [`render::render_view_flagged`]), false while the template is in flight. The merchant prices
/// a copy found in a bag (`0x52e3bc`), never a worn one, which the player holds below slot index
/// 23 (`0x52e40f`).
pub(super) fn render_owned(
    lua: &Lua,
    this: &Table,
    item_id: u32,
    inst: Option<render::ItemInstance>,
) -> mlua::Result<bool> {
    let h = frame_handle_of(lua, this)?;
    let sell_count = inst.as_ref().filter(|i| i.object).and_then(|i| {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        build::container_object(lua, &model, i.guid).map(|s| s.count.max(1))
    });
    build_item(
        lua,
        this,
        h,
        ItemBuild {
            inst,
            sell_count,
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
        // The row's -1 (no inventory type) names no colour.
        Some(it) => {
            let quality = u32::try_from(it.quality).ok();
            render_by_id(lua, this, it.item_id, it.name.clone(), quality)
        }
        None => {
            let h = frame_handle_of(lua, this)?;
            clear_silent(lua, h);
            super::tooltip::show_or_hide_empty(lua, h);
            Ok(())
        }
    }
}

/// Register the item content channels into the GameTooltip kind method table.
pub(super) fn install_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:SetQuestItem(type, index) / SetQuestLogItem(type, index) (`0x533610`,
    // `0x533760`; `QuestFrameTemplates.xml:148`, `QuestLogFrame.xml:113`), no returns. `type` is
    // `GetQuestItemInfo`'s; an unknown type or index leaves the tooltip empty, as in the reference.
    m.set(
        "SetQuestItem",
        lua.create_function(|lua, (this, kind, index): (Table, Value, Value)| {
            // `is-number-or-string` on the type and `is-number` on the index, else `Invalid quest
            // item in SetQuestItem("type", index)` (`0x855338`, `0x5336c8`-`0x53373f`).
            const LINE: &str = r#"Invalid quest item in SetQuestItem("type", index)"#;
            let kind = binding_abi::string_arg(lua, kind, LINE)?;
            let index = binding_abi::index_arg(lua, index, LINE)?;
            // The panel's rows as the host holds them at the call.
            let item = crate::script::quest::state(lua).and_then(|q| {
                crate::script::quest::item_vec(&q, &kind)
                    .and_then(|v| index.checked_sub(1).and_then(|n| v.get(n)))
                    .cloned()
            });
            set_quest_item_view(lua, &this, item)
        })?,
    )?;
    m.set(
        "SetQuestLogItem",
        lua.create_function(|lua, (this, kind, index): (Table, Value, Value)| {
            // As `SetQuestItem`, with `Invalid quest item in SetQuestLogItem("type", index)`
            // (`0x85536c`, `0x533818`-`0x533894`).
            const LINE: &str = r#"Invalid quest item in SetQuestLogItem("type", index)"#;
            let kind = binding_abi::string_arg(lua, kind, LINE)?;
            let index = binding_abi::index_arg(lua, index, LINE)?;
            let item = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                crate::script::quest_log::selected_quest_item(&model, &kind, index)
            };
            set_quest_item_view(lua, &this, item)
        })?,
    )?;

    // GameTooltip:SetHyperlink(link): the chat-link tooltip (`ItemRef.lua:60`), handed the link
    // part alone, "item:<id>:..." or "enchant:<spellId>" (`0x531fd0`).
    m.set(
        "SetHyperlink",
        lua.create_function(|lua, (this, link): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            // `is-number-or-string` else `Usage: SetHyperlink(link)` (`0x53208b`, `0x8551c4`).
            let link = binding_abi::string_arg(lua, link, "Usage: SetHyperlink(link)")?;
            // Each prefix is compared at the link's start (`0x5320b1`, `0x532218`); a miss, an
            // item id at 0 or below (`0x532178`) and an enchant naming no spell of `castUI` 3
            // (`0x53222a`..`0x532247`) raise `Unknown link type` (`0x8551e0`, `0x532269`).
            let unknown = || mlua::Error::runtime("Unknown link type");
            if let Some(tail) = link_tail(&link, "item:") {
                let (id, enchant_id, roll) = hyperlink_item_fields(tail);
                // The item the shown tooltip holds hides it instead (`0x5320c7`..`0x5320e9`): the
                // shown bit (`+0xd0`) and the kept entry (`+0x398`, 0 with no item kept), tested
                // before the id is, so the same link clicked twice closes the plate.
                let holds = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    let shown = model.arena.frame(h).is_some_and(|f| f.shown);
                    let kept = model.item_builds.get(&h).map_or(0, |b| b.item_id);
                    shown && kept == id as u32
                };
                if holds {
                    super::visibility::set_shown(lua, h, false);
                    return Ok(());
                }
                if id <= 0 {
                    return Err(unknown());
                }
                // The fields fill a block (p6=1, `0x532181`): the enchant id to slot 0 (`+0x3d0`,
                // `0x5321d3`), the roll to `+0x424` (its lines expanded into slots 2..6 and its
                // suffix joined to the name by the builder, `0x52b7bf`), the unique id to `+0x420`,
                // unread. So a link shows its enchant and its roll's lines, never the placeholder
                // (`0x52cc33`). The creator and giver are zeroed (`0x5321ad`, `0x5321b3`), so a
                // link never shows a maker line.
                let mut enchants: Vec<_> = block_enchant(lua, 0, enchant_id).into_iter().collect();
                enchants.extend(roll_enchants(lua, roll as u32));
                let inst = render::ItemInstance {
                    enchants,
                    roll,
                    ..Default::default()
                };
                build_item(
                    lua,
                    &this,
                    h,
                    ItemBuild {
                        inst: Some(inst),
                        ..ItemBuild::new(id as u32)
                    },
                )?;
                return Ok(());
            }
            let tail = link_tail(&link, "enchant:").ok_or_else(unknown)?;
            // The craft window's `enchant:<spellId>` (`GetCraftItemLink`): a positive id whose
            // record's `castUI` is 3 (`0x532225`..`0x532247`), to the spell builder (`0x532259`).
            // With no host to read the record, the id alone decides.
            let spell = binding_abi::s_str_to_int(tail);
            let craft = spell > 0
                && super::live::ask(lua, &SpellCastUiQuery(spell as u32))
                    .is_none_or(|cast_ui| cast_ui == Some(3));
            if !craft {
                return Err(unknown());
            }
            super::tooltip_spell::set_spell_by_id(
                lua,
                &this,
                spell as u32,
                None,
                Default::default(),
            )
        })?,
    )?;

    // GameTooltip:SetInventoryItem(unit, slot [, nameOnly]) -> hasItem, hasCooldown, repairCost
    // (`0x532ee0`), three as `PaperDollFrame.lua:741` reads them (`0x533300`, `0x53343e`), and
    // two on the ammo slot (`0x533122`, `0x533140`). Unit-keyed through `Model::inv_slot`:
    // an inspected item (`InspectPaperDollFrame.xml:20`) has no durability, so that line does not
    // show, and its maker is the visible block's creator (`0x5333cd`). An armed shopping tooltip
    // renders the compare shape; the arm is consumed either way. repairCost is the item's cost
    // (`0x5332fb`), 0 for an inspected unit's.
    m.set(
        "SetInventoryItem",
        lua.create_function(
            |lua, (this, unit, slot, name_only): (Table, Value, Value, Value)| {
                let h = frame_handle_of(lua, &this)?;
                // `is-number-or-string` on the unit else `Usage: SetInventoryItem(unit, slot [,
                // nameOnly])` (`0x8552dc`, `0x532fa9`); then `is-number` on the slot, `tonumber`
                // less 1.0 truncated (`0x532fe2`) and the slot reader's whitelist, else `Invalid
                // inventory slot in SetInventoryItem` (`0x8552b0`, `0x53344c`).
                let unit = super::binding_abi::string_arg(
                    lua,
                    unit,
                    "Usage: SetInventoryItem(unit, slot [, nameOnly])",
                )?;
                const SLOT_LINE: &str = "Invalid inventory slot in SetInventoryItem";
                let slot0 = super::binding_abi::less_one_arg(lua, slot, SLOT_LINE)?;
                if !super::char_stats::inventory_slot_reader_accepts(slot0) {
                    return Err(mlua::Error::runtime(SLOT_LINE));
                }
                let slot = (slot0 + 1) as usize;
                // `nameOnly` (usage string `0x8552dc`) is set only by a number above 0
                // (`0x533027..0x53304c`); anything else builds the ordinary tooltip, without an
                // error. Three of the binding's four builder legs pass it on, not `0x533106`.
                let name_only = super::binding_abi::positive_number_flag(lua, name_only)?;
                // The token's resolve (`0x533061`), which raises for one it does not know, then
                // `CanReadUnitInventory` (`0x53306a`), through `Model::inv_slot`.
                super::unit::check_unit_token(&Some(unit.clone()))?;
                // The player's item object answers the cooldown query, which asks a missing
                // record with the callback `0x6e2f70` inside the call (`0x533188` → `0x5331ec`,
                // `0x6e2efb`); the ammo slot passes an entry and queries nothing (`0x53307d`).
                if slot0 != -1
                    && unit.eq_ignore_ascii_case("player")
                    && super::char_stats::active_player_held(lua)
                {
                    let pending = {
                        let model = lua.app_data_ref::<Model>().expect("model app_data");
                        crate::script::char_stats::own_inv_view(lua, &model, slot)
                    }
                    .filter(|s| s.record_pending())
                    .map(|s| s.item_id);
                    if let Some(entry) = pending {
                        super::container::ask_cooldown_record(lua, entry)?;
                    }
                }
                let (item_id, name, quality, inst, count, repair_cost, currently_equipped) = {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    // Only the active player's slots hold item objects; another player's item is
                    // a block (`0x533354`).
                    let reader = super::char_stats::inventory_reader(lua, &model, &unit);
                    let player = reader == Some(super::char_stats::InvReader::Own);
                    let armed = match super::tooltip::tip_mut(&mut model, h) {
                        Ok(t) => std::mem::take(&mut t.equipped_header_armed),
                        Err(_) => false,
                    };
                    let view = reader
                        .and_then(|r| model.inv_slot_as(lua, r, slot))
                        .filter(|s| s.item_id != 0)
                        .map(|s| {
                            (s.item_id, s.name.clone(), s.quality, s.count.max(1), {
                                // The player's own item answers the cooldown query
                                // (`0x5331ec`), unless it is charged and spent
                                // (`0x53318a`-`0x53319b`, the feed's `None`); an inspected
                                // record has none. A running one takes the p6=1 leg
                                // (`0x533267`), every other the p6=0 leg (`0x5332ad`), whose
                                // open hint a lootable box in the vault shows.
                                let cooldown = player.then(|| {
                                    crate::script::char_stats::inv_slot_cooldown(lua, &model, slot)
                                });
                                let cooldown = cooldown.flatten();
                                let running = cooldown
                                    .is_some_and(|(start, dur, on)| on && start > 0.0 && dur > 0.0);
                                // The vault and keyring are bag slots, read as the object they
                                // hold: its text id, flags and charter all reach the builder.
                                let mut inst = match player.then(|| {
                                    crate::script::char_stats::inv_container_slot(lua, &model, slot)
                                }) {
                                    Some(Some(c)) => slot_instance(&c, !running),
                                    _ => {
                                        let mut inst = worn_instance(&s, player);
                                        inst.openable_source = player && !running;
                                        inst
                                    }
                                };
                                if player {
                                    inst.cooldown_ms = super::tooltip_spell::live_remaining_ms(
                                        cooldown, model.now,
                                    );
                                }
                                inst
                            })
                        });
                    let cost = if player {
                        model.inv_repair_cost(lua, slot)
                    } else {
                        0
                    };
                    match view {
                        Some((id, name, q, count, inst)) => (id, name, q, inst, count, cost, armed),
                        // An empty slot answers nil and still pushes the other two: callers
                        // destructure all three first, and an addon may add up repairCost. The
                        // ammo slot pushes two, nil and nil (`0x533130`..`0x533140`).
                        None if slot0 == -1 => {
                            return Ok(MultiValue::from_vec(vec![Value::Nil, Value::Nil]))
                        }
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
                    ..Default::default()
                };
                let duration_ms = inst.duration_ms;
                // Template in flight: the header and the slot's own name, as `SetBagItem`'s
                // miss path.
                let in_flight = match name {
                    Some(name) => InFlight::Name(name, quality_color(quality.max(0) as u32)),
                    None => InFlight::Nothing,
                };
                // The merchant prices an object the player holds at slot index 23 or more
                // (`0x52e40f`): the vault, the bank bags and the keyring, never a worn item.
                let sold = (inst.object && inst.guid != 0 && slot0 >= 23).then_some(count);
                let live = build_item(
                    lua,
                    &this,
                    h,
                    ItemBuild {
                        flags,
                        inst: Some(inst),
                        in_flight,
                        sell_count: sold,
                        ..ItemBuild::new(item_id)
                    },
                )?;
                // The ammo slot's leg drops the builder's answer and pushes 1 and nil
                // (`0x53310e`..`0x533122`).
                if slot0 == -1 {
                    return Ok(MultiValue::from_vec(vec![Value::Integer(1), Value::Nil]));
                }
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
        lua.create_function(|lua, (this, bag, slot): (Table, Value, Value)| {
            let h = frame_handle_of(lua, &this)?;
            // Both are `lua_isnumber`-gated (`0x5346ff`, `0x53470f`); a miss returns no value at
            // all (`0x53476e`), the tooltip untouched.
            let (Some(bag), Some(slot)) = (
                crate::script::binding_abi::is_number(lua, bag),
                crate::script::binding_abi::is_number(lua, slot),
            ) else {
                return Ok(MultiValue::new());
            };
            let bag = i64::from(crate::script::binding_abi::ftol(bag));
            let slot = crate::script::binding_abi::ftol(slot) as u32;
            // The cooldown query asks a missing record with the callback `0x6e2f70` inside the
            // call (`0x53483a`, `0x6e2efb`), off its bag lookup, `0x4f93e0` (`0x534747`).
            let held = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                crate::script::held::bag_slot(lua, &model, bag, slot)
            };
            let pending = held
                .as_ref()
                .filter(|s| s.record_pending())
                .map(|s| s.item_id);
            if let Some(entry) = pending {
                super::container::ask_cooldown_record(lua, entry)?;
            }
            let (item_id, count, link, quality, inst, repair_cost) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let cost = crate::script::merchant::repair_cost_at(lua, &model, bag, slot);
                match held.as_ref().filter(|s| s.item_id != 0) {
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
                            crate::script::container::slot_cooldown(s),
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
                            cost,
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
    // `TradeFrame.xml:150`, `TradeFrame.xml:106`), no returns, both with the fields valid
    // (`0x5343be`, `0x5345e9`): the player's side the item object its slot's guid names
    // (`[0xb716d0 + slot*8]`, `0x5342c8`), the target side a block off the partner's slot arrays
    // (the entry `[0xb714f4 + slot*4]`, `0x5344ff`). An empty or out-of-range slot, or a guid
    // naming no object (`0x5342f9`), leaves the tooltip untouched. The partner's permanent
    // enchant (`[0xb715c0 + slot*4]`, `0x534521`) is not rendered: the trade view has no enchant
    // id.
    for (name, player_side) in [("SetTradePlayerItem", true), ("SetTradeTargetItem", false)] {
        m.set(
            name,
            lua.create_function(move |lua, (this, slot): (Table, Value)| {
                let h = frame_handle_of(lua, &this)?;
                // `is-number` else `Invalid trade slot in SetTradePlayerItem` (`0x8554a0`,
                // `0x5343ea`) or `… in SetTradeTargetItem` (`0x8554cc`, `0x5345f9`); `n - 1` out of
                // `[0, 7)` names no slot and does nothing (`0x5342c1`, `0x5344f0`).
                let slot0 =
                    binding_abi::less_one_arg(lua, slot, &format!("Invalid trade slot in {name}"))?;
                let cell = usize::try_from(slot0)
                    .ok()
                    .filter(|&n| n < super::trade::TRADE_SLOTS);
                let source = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    // The slot as the getters read it at the call.
                    let row =
                        cell.and_then(|n| super::trade::slot_now(lua, &model, n + 1, player_side));
                    // The seventh slot alone proposes the spell cast on it (`0x534391`,
                    // `0x534598`, the block's `+0x40c`).
                    let proposed = row
                        .as_ref()
                        .filter(|_| slot0 == 6)
                        .and_then(|s| s.enchantment.clone());
                    match player_side {
                        // The object's flags, creator and giver reach the LOCKED, wrapped and
                        // maker reads (`0x52bdb3`, `0x52b789`, `0x52e247`); the block's charges
                        // stay -1 and its lock id 0 (`0x534337`, `0x534341`).
                        true => cell
                            .and_then(|n| model.trade_offered[n].as_ref())
                            .and_then(|offered| build::live_object(lua, &model, offered.guid))
                            .map(|held| {
                                let inst = render::ItemInstance {
                                    openable_source: false,
                                    proposed_enchant: proposed,
                                    ..held.inst
                                };
                                (held.item_id, held.quality, inst)
                            }),
                        // The block carries the partner's lock id (`0x534563`), wrapped word
                        // (`0x534559`), gift creator (`0x534574`), creator (`0x53454f`), charges
                        // (`0x534595`) and durability (`0x5345b4`, `0x5345c0`).
                        false => row.filter(|i| i.item_id != 0).map(|item| {
                            let inst = render::ItemInstance {
                                name: item.name.clone(),
                                block_lock_id: item.lock_id,
                                block_wrapped: item.wrapped,
                                gift_creator: item.gift_creator.clone(),
                                gift_creator_guid: item.gift_creator_guid,
                                creator: item.creator.clone(),
                                creator_guid: item.creator_guid,
                                proposed_enchant: proposed,
                                block_charges: Some(item.charges),
                                block_durability: item.durability,
                                ..Default::default()
                            };
                            (item.item_id, item.quality, inst)
                        }),
                    }
                };
                let Some((item_id, quality, inst)) = source else {
                    return Ok(());
                };
                let in_flight = match inst.name.clone() {
                    Some(name) => InFlight::Name(name, quality_color(quality.unwrap_or(1))),
                    None => InFlight::Nothing,
                };
                build_item(
                    lua,
                    &this,
                    h,
                    ItemBuild {
                        inst: Some(inst),
                        in_flight,
                        ..ItemBuild::new(item_id)
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
        lua.create_function(|lua, (this, slot): (Table, Value)| {
            let h = frame_handle_of(lua, &this)?;
            // `is-number` else the client's `Invalid loot slot in SetInventoryItem`, its text
            // naming the other method (`0x855310`, `0x533528`).
            let slot = binding_abi::index_arg(lua, slot, "Invalid loot slot in SetInventoryItem")?;
            // The row as the window holds it at the call (`0x533551`, `0x4c22a0`).
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                super::loot::slot_now(lua, &model, slot)
                    .flatten()
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
        lua.create_function(|lua, (this, roll_id): (Table, Value)| {
            // `is-number` else `Usage: SetLootRollItem(id)` (`0x855650`, `0x53657b`).
            let roll_id = binding_abi::index_arg32(lua, roll_id, "Usage: SetLootRollItem(id)")?;
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

    compare::install_methods(lua, m)?;

    // GameTooltip:SetMerchantItem(index) / SetBuybackItem(index): template sources, as the
    // reference's `SetMerchantItem` (`0x534080`) is. `SetBuybackItem` (`0x536300`) tests its slot
    // with `lua_isnumber` before its receiver (`0x536322`, usage `0x855634`), then takes row
    // `ftol(slot) - 1` when it is below the count, compared unsigned (`0x536401`-`0x53640f`).
    for (method, buyback) in [("SetMerchantItem", false), ("SetBuybackItem", true)] {
        m.set(
            method,
            lua.create_function(move |lua, (this, index): (Value, Value)| {
                let (this, index) = if buyback {
                    let Some(slot) = lua.coerce_number(index)? else {
                        return Err(mlua::Error::runtime("Usage: SetBuybackItem(slot)"));
                    };
                    let this = crate::script::object::receiver_in(
                        lua,
                        &this,
                        crate::script::tooltip::REG_TOOLTIP_METHODS,
                    )?;
                    let Ok(index) = usize::try_from(slot as i64) else {
                        return Ok(());
                    };
                    (this, index)
                } else {
                    let this = <Table as mlua::FromLua>::from_lua(this, lua)?;
                    frame_handle_of(lua, &this)?;
                    // `is-number` else `Invalid merchant slot in SetMerchantItem` (`0x855474`,
                    // `0x5341c4`); `n - 1` below 0 names no row (`0x534166`).
                    let slot0 = binding_abi::less_one_arg(
                        lua,
                        index,
                        "Invalid merchant slot in SetMerchantItem",
                    )?;
                    let Ok(index) = usize::try_from(slot0) else {
                        return Ok(());
                    };
                    (this, index + 1)
                };
                let h = frame_handle_of(lua, &this)?;
                // The open vendor's row as of the call.
                let row = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    if !crate::script::merchant::head(lua, &model).open {
                        return Ok(());
                    }
                    if buyback {
                        index
                            .checked_sub(1)
                            .and_then(|i| crate::script::merchant::buyback_row(lua, &model, i))
                    } else {
                        crate::script::merchant::row(lua, &model, index - 1)
                    }
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
        lua.create_function(|lua, (this, index): (Table, Value)| {
            // `this` first (`0x53554f`), then `is-number` else `Usage: SetInboxItem(index)`
            // (`0x855584`, `0x53559e`).
            let h = frame_handle_of(lua, &this)?;
            let index = binding_abi::index_arg(lua, index, "Usage: SetInboxItem(index)")?;
            // `ftol(n) - 1` compared unsigned (`0x5355ce`, `0x5355d1`): 0 names no row. The row
            // as the session holds it at the call.
            let row = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                super::mail::row_now(lua, &model, index)
            };
            let Some(row) = row.filter(|r| r.item_id != 0) else {
                return Ok(Value::Nil);
            };
            let item_id = row.item_id;
            // A block source (`0x5355fa`, p6=1) carrying the attachment's roll, so no
            // placeholder, and its charges and durability (`0x53566f`, `0x53568d`, `0x535699`).
            let inst = render::ItemInstance {
                name: row.item_name,
                enchants: roll_enchants(lua, row.item_random_property_id),
                block_charges: Some(row.item_charges),
                block_durability: row.item_durability,
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
        lua.create_function(|lua, (this, kind, index): (Table, Value, Value)| {
            // `this` first, then `is-number` on arg 3, `is-number-or-string` on arg 2 else
            // `Usage: SetAuctionItem("type", index)` (`0x8555a0`, `0x535ab6`).
            let h = frame_handle_of(lua, &this)?;
            let kind =
                binding_abi::string_arg(lua, kind, r#"Usage: SetAuctionItem("type", index)"#)?;
            let index =
                binding_abi::index_arg(lua, index, r#"Usage: SetAuctionItem("type", index)"#)?;
            let held = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.auction.is_some()
            };
            let Some(list) = super::auction::list_index_of(&kind).filter(|_| held) else {
                return Ok(Value::Nil);
            };
            // `ftol(n) - 1` compared unsigned (`0x535933`, `0x535944`): 0 names no row. The
            // record is read at the call, as the getters read it.
            let (item_id, roll, name, charges) =
                match super::auction_live::live_row_at(lua, list, index) {
                    Some(r) => (r.item_id, r.random_property_id, r.name, r.charges),
                    None => (0, 0, None, -1),
                };
            if item_id == 0 {
                return Ok(Value::Nil);
            }
            // A block source (`0x5359d9`, p6=1) carrying the listing's roll, so no placeholder,
            // and its charges (`0x535a4a`).
            let inst = render::ItemInstance {
                name,
                enchants: roll_enchants(lua, roll),
                block_charges: Some(charges),
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
                    Some(it) => (it.item_id, held_instance(lua, &model, it.bag, it.slot)),
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
                    Some(it) => (it.item_id, held_instance(lua, &model, it.bag, it.slot)),
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
    // recipe's or the reagent's name alone (`render_by_id`).
    m.set(
        "SetTradeSkillItem",
        lua.create_function(
            |lua, (this, skill_index, reagent_index): (Table, Value, Value)| {
                // `this` first (`0x533b6e`).
                frame_handle_of(lua, &this)?;
                // `is-number` on the index, a row below the count (`0x533b9f`) whose spell has a
                // record (`0x533bbd`, a header's -1 has none), and a reagent number that names
                // one of the recipe's reagents (`0x533c51`..`0x533c68`), else `Invalid trade skill
                // item in SetTradeSkillItem(index [,reagent])` (`0x8553d8`, `0x533c6a`); a
                // non-number reagent is absent (`0x533be7`).
                const LINE: &str =
                    "Invalid trade skill item in SetTradeSkillItem(index [,reagent])";
                let skill_index = binding_abi::index_arg(lua, skill_index, LINE)?;
                let reagent_index = binding_abi::index_or(lua, reagent_index)?.map(|i| i as usize);
                let found = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    // A visible-row index, headers included (`0x4fca20`); a header is None.
                    super::tradeskill::recipe_at(&model, skill_index).and_then(|recipe| {
                        match reagent_index {
                            Some(ri) => ri
                                .checked_sub(1)
                                .and_then(|i| recipe.reagents.get(i))
                                .map(|r| (r.item, r.name.clone())),
                            None => Some((recipe.product_item, Some(recipe.name.clone()))),
                        }
                    })
                };
                let Some((item_id, fb_name)) = found else {
                    return Err(mlua::Error::runtime(LINE));
                };
                render_by_id(lua, &this, item_id, fb_name, None)
            },
        )?,
    )?;

    // GameTooltip:SetCraftItem(craftIndex, reagentIndex): the reagent's item
    // (`Blizzard_CraftUI.xml:33`); a craft has no product item, so both arguments are required.
    m.set(
        "SetCraftItem",
        lua.create_function(
            |lua, (this, craft_index, reagent_index): (Table, Value, Value)| {
                // `this` first (`0x533d7b`).
                frame_handle_of(lua, &this)?;
                // `is-number` on both (`0x533d88`, `0x533d9c`), a row below the count
                // (`0x533dc3`) whose spell has a record (`0x533dd9`), and a reagent number naming
                // one of its reagents (`0x533e10`..`0x533e27`), else `Invalid craft item in
                // SetCraftItem(index, reagent)` (`0x855418`, `0x533e29`).
                const LINE: &str = "Invalid craft item in SetCraftItem(index, reagent)";
                let craft_index = binding_abi::index_arg(lua, craft_index, LINE)?;
                let reagent_index = binding_abi::index_arg(lua, reagent_index, LINE)?;
                let found = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    crate::script::craft::recipe(&model, craft_index).and_then(|recipe| {
                        reagent_index
                            .checked_sub(1)
                            .and_then(|i| recipe.reagents.get(i))
                            .map(|r| (r.item, r.name.clone()))
                    })
                };
                let Some((item_id, fb_name)) = found else {
                    return Err(mlua::Error::runtime(LINE));
                };
                render_by_id(lua, &this, item_id, fb_name, None)
            },
        )?,
    )?;

    Ok(())
}
