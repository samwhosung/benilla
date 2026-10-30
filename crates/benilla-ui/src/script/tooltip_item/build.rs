//! The builder's call and its rebuild. Every `Set*Item` resolves its item into an [`ItemBuild`] and
//! hands it to [`build_item`], the reference's `0x52b650` from its clear to its show. A fresh build
//! keeps its inputs on the tooltip (`0x52b6cc`..`0x52b70a`, `+0x380`..`+0x440`), and every cache it
//! misses takes the tooltip as the cookie of `CGTooltip::OnCacheAnswer` (`0x52e530`): the item
//! record cache (`0x52b716`, the lock's key at `0x52bf48`), the name cache (the creator at
//! `0x52e1f4`, the gift's giver at `0x52e28d`, a petition's owner at `0x52b9c5`), the petition cache
//! (`0x52b92f`) and the item-name cache of the set members (`0x52df4b`). An answer rebuilds whatever
//! the tooltip keeps by then, from scratch (`0x52e5a7`, p7 = 0: the clear and its
//! `OnTooltipCleared`, the lines, the show), and sizes it (`0x52e5ac`); a tooltip that keeps
//! nothing, since every clear zeroes the entry (`0x5300dd`), stays as it is (`0x52e556`).
//!
//! benilla's caches answer by push or through the lent host: a template through
//! [`UiScript::set_item_template`], a set and its members' names through the host's set read (the
//! VM's copy, [`UiScript::set_item_set`], with none lent), a name through the lent host's name cache
//! or else inside the views that carry its guid, and a petition inside those views, which the app
//! re-pushes once it resolves. [`answer_item_caches`]
//! settles the waits once a tick, one rebuild per answered ask.
//!
//! [`UiScript::set_item_template`]: crate::script::UiScript::set_item_template
//! [`UiScript::set_item_set`]: crate::script::UiScript::set_item_set

use mlua::{Lua, Table};

use super::names::{GRAY, WHITE};
use super::render::{self, BuilderFlags, ItemInstance};
use super::{fire_add_money, view_of};
use crate::script::object::frame_wrapper;
use crate::script::tooltip::{append_line, clear, show_or_hide_empty};
use crate::script::{ItemTemplateView, Model};
use crate::widget::FrameHandle;

/// What a `Set*Item` hands the builder, kept on the tooltip until its next clear: the reference's
/// p1, p4, p5, p6 and instance fields (`+0x380`..`+0x440`).
#[derive(Clone, Default)]
pub(crate) struct ItemBuild {
    pub(in crate::script) item_id: u32,
    pub(in crate::script) flags: BuilderFlags,
    /// The item object's or block's fields; `None` for a template source.
    pub(in crate::script) inst: Option<ItemInstance>,
    /// What shows while the template is in flight.
    pub(in crate::script) in_flight: InFlight,
    /// The stack of an item object the merchant may price, at an open merchant out of repair
    /// mode: one held in a bag, or by the player at slot index 23 or more (`0x52e376`..`0x52e40f`).
    pub(in crate::script) sell_count: Option<u32>,
}

impl ItemBuild {
    pub(in crate::script) fn new(item_id: u32) -> Self {
        Self {
            item_id,
            ..Default::default()
        }
    }
}

/// The plate while the template is in flight. Deviation: the reference's builder hides and clears
/// on a missing record (`0x52b72e`..`0x52b73f`), where a source that knows the item's name shows
/// that, because a blank plate under an on-screen name reads as broken; the template's arrival
/// then builds it whole.
#[derive(Clone, Default)]
pub(in crate::script) enum InFlight {
    /// The reference's own hide.
    #[default]
    Nothing,
    /// One line: the source's name in its colour.
    Name(String, [f32; 4]),
    /// A minimal view of what the source knows of the template (a vendor row's stat head).
    View(Box<ItemTemplateView>),
}

/// A cache a build missed, with the tooltip as the answer's cookie.
#[derive(Clone, PartialEq)]
pub(crate) enum CacheWait {
    /// The item record cache (`0xc0e2a0`): the item itself, a lock's key, a compare's offer.
    Template(u32),
    /// A set's view the VM's copy lacks with no host lent; with one, the set reads as it runs.
    Set(u32),
    /// The item-name cache (`0xc0e2dc`): one set member still unnamed.
    MemberName { set: u32, item: u32 },
    /// The name cache (`0xc0e228`): a creator's or a giver's guid.
    Name(u64),
    /// The petition cache (`0xc0e0fc`): the charter object of this guid has no record yet.
    Petition(u64),
    /// The name cache, for the owner of the charter object of this guid.
    PetitionOwner(u64),
}

/// The set blocks a tick's waits read, each set read once a tick.
type SetsRead = std::collections::HashMap<u32, Option<crate::script::ItemSetView>>;

impl CacheWait {
    fn answered(&self, lua: &Lua, model: &Model, sets: &mut SetsRead) -> bool {
        let mut set = |id: u32| {
            sets.entry(id)
                .or_insert_with(|| crate::script::item_stats::item_set(lua, model, id))
                .clone()
        };
        match *self {
            CacheWait::Template(id) => model.item_templates.contains_key(&id),
            CacheWait::Set(id) => set(id).is_some(),
            CacheWait::MemberName { set: id, item } => set(id).is_some_and(|v| {
                v.members
                    .iter()
                    .any(|(id, name)| *id == item && name.is_some())
            }),
            CacheWait::Name(guid) => name_of(lua, model, guid).is_some(),
            CacheWait::Petition(guid) => {
                container_object(lua, model, guid).is_some_and(|s| s.petition.is_some())
            }
            CacheWait::PetitionOwner(guid) => container_object(lua, model, guid)
                .and_then(|s| s.petition)
                .is_some_and(|p| p.owner.is_some()),
        }
    }
}

/// The name cache's record for `guid`, read through the lent host (`0x55f080`); with no host free
/// to ask, off any view that carries it as a creator or a giver, which the app re-pushes once a
/// name answers.
fn name_of(lua: &Lua, model: &Model, guid: u64) -> Option<String> {
    if let Some(live) = crate::script::live::ask(lua, &crate::script::PlayerNameQuery(guid)) {
        return live;
    }
    let of = |g: u64, name: &Option<String>| (g == guid).then(|| name.clone()).flatten();
    let bags = model.containers.values().flat_map(|c| c.slots.values());
    let held = bags.filter_map(|s| {
        of(s.creator_guid, &s.creator).or_else(|| of(s.gift_creator_guid, &s.gift_creator))
    });
    // Our own worn items, each read as the walk reaches it, and the inspected players', whose
    // maker is the visible block's creator.
    let maker = |s: &crate::script::InvSlotView| {
        of(s.creator_guid, &s.creator).or_else(|| of(s.gift_creator_guid, &s.gift_creator))
    };
    let own = crate::script::worn::worn_iter(lua, model).filter_map(|s| maker(&s));
    let inspected = model
        .visible_equipment
        .values()
        .flatten()
        .flatten()
        .filter_map(maker);
    let traded = model
        .trade
        .iter()
        .flat_map(|t| t.player.slots.iter().chain(t.target.slots.iter()))
        .flatten()
        .filter_map(|s| {
            of(s.creator_guid, &s.creator).or_else(|| of(s.gift_creator_guid, &s.gift_creator))
        });
    held.chain(own).chain(inspected).chain(traded).next()
}

/// The bag slot that holds the item object `guid` now, wherever it moved; none for guid 0.
pub(super) fn container_object(
    lua: &Lua,
    model: &Model,
    guid: u64,
) -> Option<crate::script::ContainerSlot> {
    crate::script::held::find_guid(lua, model, guid).map(|(_, _, s)| s)
}

/// An item object the player holds, as the builder reads it live.
pub(super) struct HeldObject {
    pub(super) item_id: u32,
    /// `None` while the template is in flight.
    pub(super) quality: Option<u32>,
    pub(super) count: u32,
    /// The object with the fields not valid; a setter that sets them says so.
    pub(super) inst: ItemInstance,
}

/// The item object `guid` (`0x52b777` finds it by guid) wherever it is held, a bag slot or a worn
/// one; none for guid 0.
pub(super) fn live_object(lua: &Lua, model: &Model, guid: u64) -> Option<HeldObject> {
    if let Some(s) = container_object(lua, model, guid) {
        return Some(HeldObject {
            item_id: s.item_id,
            quality: s.quality,
            count: s.count.max(1),
            inst: super::slot_instance(&s, true),
        });
    }
    // The worn slot that holds it now (`0x6228a0`), found by the doll's keys and read alone.
    let slot = crate::script::worn::worn_keys(lua, model)
        .iter()
        .position(|k| guid != 0 && k.is_some_and(|(g, _)| g == guid))?;
    crate::script::worn::worn_slot(lua, model, slot).map(|s| HeldObject {
        item_id: s.item_id,
        quality: u32::try_from(s.quality).ok(),
        count: s.count.max(1),
        inst: ItemInstance {
            openable_source: true,
            ..super::worn_instance(&s, true)
        },
    })
}

impl ItemBuild {
    /// The inputs a rebuild runs with. The builder reads the item object live by its kept guid
    /// (p3, `0x52e560`), and the block fields as they were kept (`+0x3d0`..`+0x440`): the open
    /// gate's p6, the cooldown, the lock id, the wrapped word, the proposed enchant, the charges
    /// and the durability. A name the build lacked is read as it is now.
    ///
    /// Deviation: an object no longer held rebuilds from the instance the build read, where the
    /// reference's lookup finds no object and builds from the record and the block alone;
    /// benilla's views carry no object to read once it is gone, and the kept instance is what the
    /// plate showed.
    fn refreshed(mut self, lua: &Lua, model: &Model) -> Self {
        let Some(kept) = self.inst.take() else {
            return self;
        };
        let live = kept
            .object
            .then(|| live_object(lua, model, kept.guid))
            .flatten();
        let mut inst = match live {
            Some(held) => {
                let inst = ItemInstance {
                    openable_source: kept.openable_source,
                    cooldown_ms: kept.cooldown_ms,
                    block_lock_id: kept.block_lock_id,
                    block_wrapped: kept.block_wrapped,
                    proposed_enchant: kept.proposed_enchant,
                    block_charges: kept.block_charges,
                    block_durability: kept.block_durability,
                    ..held.inst
                };
                self.sell_count = self.sell_count.map(|_| held.count);
                inst
            }
            None => kept,
        };
        if inst.creator.is_none() && inst.creator_guid != 0 {
            inst.creator = name_of(lua, model, inst.creator_guid);
        }
        if inst.gift_creator.is_none() && inst.gift_creator_guid != 0 {
            inst.gift_creator = name_of(lua, model, inst.gift_creator_guid);
        }
        self.inst = Some(inst);
        self
    }
}

/// Records a miss of the build under way; [`build_item`] hands it to its tooltip.
pub(super) fn note_miss(model: &mut Model, wait: CacheWait) {
    model.item_cache_misses.push(wait);
}

/// Hands the misses recorded since the last call to the tooltip `h` as its waits, one callback
/// node each. The builder's asks pass the dedupe flag, so an ask already waiting with this cookie
/// adds none (`0x55bbb3`..`0x55bbe3`); the compare asks pass 0 (`0x5361d7`, `0x535f39`), so each
/// stacks another, and every node runs on the answer.
pub(super) fn keep_misses(model: &mut Model, h: FrameHandle, dedupe: bool) {
    let misses = std::mem::take(&mut model.item_cache_misses);
    if misses.is_empty() {
        return;
    }
    let waits = model.item_cache_waits.entry(h).or_default();
    let mut asks = Vec::new();
    for m in misses {
        if let CacheWait::Template(entry) = m {
            asks.push(entry);
        }
        if !dedupe || !waits.contains(&m) {
            waits.push(m);
        }
    }
    model.tooltip_record_asks.extend(asks);
}

/// Ask the item cache for every record a tooltip kept a wait on, each with the tooltips'
/// callback, which the answer runs in its place among the entry's (`0x52b722`, `0x52eda4`); the
/// caller holds no borrow of the model.
pub(in crate::script) fn ask_tooltip_records(lua: &Lua) {
    let asks = std::mem::take(
        &mut lua
            .app_data_mut::<Model>()
            .expect("model app_data")
            .tooltip_record_asks,
    );
    for entry in asks {
        let ask = crate::script::RecordAsk::Item {
            entry,
            owner: 0,
            callback: crate::script::ItemCallback::Tooltip,
        };
        if let Err(e) = crate::script::calls::ask(lua, ask) {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .record_lua_error(&e);
        }
    }
}

/// The builder (`0x52b650`) as a `Set*Item` runs it: the clear (`0x52b67e`), the kept inputs
/// (`0x52b6cc`), the lines, the sell price, and the show or, with no line, the hide whose clear
/// drops the kept inputs again. Answers the builder's return value.
pub(in crate::script) fn build_item(
    lua: &Lua,
    this: &Table,
    h: FrameHandle,
    build: ItemBuild,
) -> mlua::Result<bool> {
    clear(lua, h);
    // What an earlier render outside a build noted is no miss of this one.
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .item_cache_misses
        .clear();
    // With no active player the build hides and returns, keeping nothing (`0x52b699` →
    // `0x52b6af`, `0x52b6b5`).
    if crate::script::requirements::no_player(lua) {
        show_or_hide_empty(lua, h);
        return Ok(false);
    }
    let view = view_of(lua, build.item_id);
    let live = match &view {
        Some(v) => {
            match render::render_view_flagged(lua, this, v, build.flags, build.inst.as_ref())? {
                Some(live) => live,
                // A petition miss returns before the sell price and the show (`0x52b94e`); the
                // build is kept for the answer's rebuild.
                None => {
                    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                    keep_misses(&mut model, h, true);
                    model.item_builds.insert(h, build);
                    drop(model);
                    ask_tooltip_records(lua);
                    return Ok(false);
                }
            }
        }
        None => {
            match &build.in_flight {
                InFlight::Nothing => {}
                InFlight::Name(name, color) => {
                    if build.flags.currently_equipped {
                        if let Some(t) = crate::strings::global(lua, "CURRENTLY_EQUIPPED") {
                            append_line(lua, this, (t, GRAY), None, false)?;
                        }
                    }
                    let color = if build.flags.name_only { WHITE } else { *color };
                    append_line(lua, this, (name.clone(), color), None, false)?;
                }
                InFlight::View(v) => {
                    render::render_view(lua, this, v, build.flags, None)?;
                }
            }
            false
        }
    };
    let sell = build.sell_count.zip(view.as_ref()).map(|(count, v)| {
        let model = lua.app_data_ref::<Model>().expect("model app_data");
        // An item object's charges and repair cost (`0x52e426` tests the object).
        let object = build.inst.as_ref().filter(|i| i.object);
        let charges = object.map(|i| i.object_charges[0]);
        let cost = object.map_or(0, |i| {
            crate::script::merchant::item_repair_cost(lua, &model, i.guid)
        });
        let amount = sell_amount(v.sell_price, v.spell_charges_0, charges, cost, count);
        (v.sell_price, amount)
    });
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        keep_misses(&mut model, h, true);
        model.item_builds.insert(h, build);
    }
    // The sell price, after the lines (`0x52e376`): `OnTooltipAddMoney` with the amount, or
    // ITEM_UNSELLABLE at price 0 (`0x854a74`, pushed at `0x52e4a3`).
    if let Some((price, amount)) = sell {
        let (merchant_open, repairing) = {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            (
                crate::script::merchant::head(lua, &model).open,
                model.repair_mode,
            )
        };
        if merchant_open && !repairing {
            if price > 0 {
                fire_add_money(lua, h, amount);
            } else if let Some(t) = crate::strings::global(lua, "ITEM_UNSELLABLE") {
                append_line(lua, this, (t, WHITE), None, false)?;
            }
        }
    }
    show_or_hide_empty(lua, h);
    ask_tooltip_records(lua);
    Ok(live)
}

/// What the merchant pays (`0x52e418`..`0x52e473`), in copper, the `"%d"` the money handler
/// gets: the record's sell price, which for an item object scales by its block-0 charges over the
/// template's when those are below -1 (`imul`, `idiv`, `0x52e432`..`0x52e446`), drops to 1 when
/// the repair cost (`0x5da330`) reaches it or else loses the cost (`0x52e451`..`0x52e45e`), and
/// multiplies by a stack above 1 (`0x52e466`..`0x52e471`). Without an object it is the price.
pub(super) fn sell_amount(
    price: u32,
    template_charges: i32,
    object_charges: Option<i32>,
    repair_cost: u32,
    count: u32,
) -> i64 {
    let Some(charges) = object_charges else {
        return i64::from(price);
    };
    let mut amount = price as i32;
    if template_charges < -1 {
        amount = charges.wrapping_mul(amount) / template_charges;
    }
    let cost = repair_cost as i32;
    if cost >= amount {
        amount = 1;
    } else if cost != 0 {
        amount -= cost;
    }
    if count as i32 > 1 {
        amount = (count as i32).wrapping_mul(amount);
    }
    i64::from(amount)
}

/// `CGTooltip::OnCacheAnswer` (`0x52e530`) for every answer pushed since the last tick, or with
/// `only`, for the item record cache's answer of that entry alone, at its packet: each callback
/// node an answer settles rebuilds its tooltip from the inputs it keeps then, so two answers are
/// two rebuilds. A tooltip that keeps none takes the answer and stays as it is.
pub(in crate::script) fn answer_item_caches(lua: &Lua, only: Option<u32>) -> Vec<mlua::Error> {
    let due: Vec<(u32, FrameHandle, usize)> = {
        let mut guard = lua.app_data_mut::<Model>().expect("model app_data");
        let model = &mut *guard;
        if model.item_cache_waits.is_empty() {
            return Vec::new();
        }
        let tips: Vec<FrameHandle> = model.item_cache_waits.keys().copied().collect();
        let mut due = Vec::new();
        let mut sets = SetsRead::new();
        for h in tips {
            let Some(mut waits) = model.item_cache_waits.remove(&h) else {
                continue;
            };
            let before = waits.len();
            let settles = |w: &CacheWait| only.is_none_or(|e| *w == CacheWait::Template(e));
            waits.retain(|w| !(settles(w) && w.answered(lua, model, &mut sets)));
            let answers = before - waits.len();
            if !waits.is_empty() {
                model.item_cache_waits.insert(h, waits);
            }
            if answers > 0 {
                due.push((model.frame_id(h), h, answers));
            }
        }
        due
    };
    let mut errors = Vec::new();
    for (id, h, answers) in due {
        for _ in 0..answers {
            let build = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .item_builds
                    .get(&h)
                    .cloned()
                    .map(|b| b.refreshed(lua, &model))
            };
            // Nothing kept (`0x52e556`): a clear in between, or a rebuild's own hide.
            let Some(build) = build else { break };
            let rebuilt = frame_wrapper(lua, id).and_then(|this| build_item(lua, &this, h, build));
            if let Err(e) = rebuilt {
                errors.push(e);
            }
            // The callback sizes the plate after its builder's show (`0x52e5ae`), so lines an
            // `OnShow` added count.
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            crate::script::tooltip::size_tooltip(&mut model, h);
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    /// The charge scale truncates toward zero (`idiv`, `0x52e444`), a pool of -1 or more is no
    /// scale (`0x52e435`), a spent item still fetches 1 (`0x52e453`), and a source with no object
    /// takes the price alone (`0x52e42b`).
    #[test]
    fn the_charge_scale_truncates_and_no_object_takes_the_price() {
        assert_eq!(super::sell_amount(7, -3, Some(-2), 0, 1), 4);
        assert_eq!(super::sell_amount(7, -1, Some(0), 0, 1), 7);
        assert_eq!(super::sell_amount(7, -3, Some(0), 0, 1), 1);
        assert_eq!(super::sell_amount(7, -3, None, 9, 5), 7);
    }
}
