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
//! benilla's caches answer by push: a template through [`UiScript::set_item_template`], a set and
//! its members' names through [`UiScript::set_item_set`], and a name or a petition inside the
//! views that carry its guid, which the app re-pushes once it resolves. [`answer_item_caches`]
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
    /// `SetBagItem`'s stack, priced at an open merchant out of repair mode.
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
    /// A set's view, which benilla's app pushes on ask.
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

impl CacheWait {
    fn answered(&self, model: &Model) -> bool {
        match *self {
            CacheWait::Template(id) => model.item_templates.contains_key(&id),
            CacheWait::Set(id) => model.item_sets.contains_key(&id),
            CacheWait::MemberName { set, item } => model.item_sets.get(&set).is_some_and(|v| {
                v.members
                    .iter()
                    .any(|(id, name)| *id == item && name.is_some())
            }),
            CacheWait::Name(guid) => name_of(model, guid).is_some(),
            CacheWait::Petition(guid) => {
                container_object(model, guid).is_some_and(|s| s.petition.is_some())
            }
            CacheWait::PetitionOwner(guid) => container_object(model, guid)
                .and_then(|s| s.petition.as_ref())
                .is_some_and(|p| p.owner.is_some()),
        }
    }
}

/// A name the app resolved for `guid`, read off any view that carries it as a creator or a giver.
/// benilla's name cache lives in the app, which re-pushes those views once a name answers.
fn name_of(model: &Model, guid: u64) -> Option<String> {
    let of = |g: u64, name: &Option<String>| (g == guid).then(|| name.clone()).flatten();
    let bags = model.containers.values().flat_map(|c| c.slots.values());
    let held = bags.filter_map(|s| {
        of(s.creator_guid, &s.creator).or_else(|| of(s.gift_creator_guid, &s.gift_creator))
    });
    let worn = model.inventory_slots.iter().flatten().filter_map(|s| {
        of(s.creator_guid, &s.creator).or_else(|| of(s.gift_creator_guid, &s.gift_creator))
    });
    let traded = model
        .trade
        .iter()
        .flat_map(|t| t.player.slots.iter().chain(t.target.slots.iter()))
        .flatten()
        .filter_map(|s| of(s.gift_creator_guid, &s.gift_creator));
    held.chain(worn).chain(traded).next()
}

/// The bag slot that holds the item object `guid`, wherever it moved.
fn container_object(model: &Model, guid: u64) -> Option<&crate::script::ContainerSlot> {
    model
        .containers
        .values()
        .flat_map(|c| c.slots.values())
        .find(|s| s.guid == guid)
}

/// The item object `guid` as the builder reads it live (`0x52b777` finds it by guid), and its
/// stack, wherever it is held: a bag slot or a worn one.
fn live_object(model: &Model, guid: u64) -> Option<(ItemInstance, u32)> {
    if let Some(s) = container_object(model, guid) {
        return Some((super::slot_instance(s, false), s.count.max(1)));
    }
    model
        .inventory_slots
        .iter()
        .flatten()
        .find(|s| s.guid == guid)
        .map(|s| (super::worn_instance(s, true), s.count.max(1)))
}

impl ItemBuild {
    /// The inputs a rebuild runs with. The builder reads the item object live by its kept guid
    /// (p3, `0x52e560`), and the block fields as they were kept (`+0x3d0`..`+0x440`): the open
    /// gate's p6 and the cooldown. A name the build lacked is read as it is now.
    ///
    /// Deviation: an object no longer held, or a source that keeps no guid (the player's own
    /// trade slot), rebuilds from the instance the build read, where the reference's lookup finds
    /// no object and builds from the record and the block alone; benilla's views carry no object
    /// to read once it is gone, and the kept instance is what the plate showed.
    fn refreshed(mut self, model: &Model) -> Self {
        let Some(kept) = self.inst.take() else {
            return self;
        };
        let live = (kept.object && kept.guid != 0)
            .then(|| live_object(model, kept.guid))
            .flatten();
        let mut inst = match live {
            Some((mut inst, count)) => {
                inst.openable_source = kept.openable_source;
                inst.cooldown_ms = kept.cooldown_ms;
                inst.block_lock_id = kept.block_lock_id;
                inst.block_wrapped = kept.block_wrapped;
                self.sell_count = self.sell_count.map(|_| count);
                inst
            }
            None => kept,
        };
        if inst.creator.is_none() && inst.creator_guid != 0 {
            inst.creator = name_of(model, inst.creator_guid);
        }
        if inst.gift_creator.is_none() && inst.gift_creator_guid != 0 {
            inst.gift_creator = name_of(model, inst.gift_creator_guid);
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
    for m in misses {
        if !dedupe || !waits.contains(&m) {
            waits.push(m);
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
    let view = view_of(lua, build.item_id);
    let live = match &view {
        Some(v) => render::render_view_flagged(lua, this, v, build.flags, build.inst.as_ref())?,
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
    let sell = build.sell_count.zip(view.as_ref().map(|v| v.sell_price));
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        keep_misses(&mut model, h, true);
        model.item_builds.insert(h, build);
    }
    // The sell price, after the lines (`0x52e376`): `OnTooltipAddMoney` with the stack's price,
    // or ITEM_UNSELLABLE at price 0 (`0x854a74`, pushed at `0x52e4a3`).
    if let Some((count, price)) = sell {
        let (merchant_open, repairing) = {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            (model.merchant.is_some(), model.repair_mode)
        };
        if merchant_open && !repairing {
            if price > 0 {
                fire_add_money(lua, h, u64::from(price) * u64::from(count));
            } else if let Some(t) = crate::strings::global(lua, "ITEM_UNSELLABLE") {
                append_line(lua, this, (t, WHITE), None, false)?;
            }
        }
    }
    show_or_hide_empty(lua, h);
    Ok(live)
}

/// `CGTooltip::OnCacheAnswer` (`0x52e530`) for every answer pushed since the last tick: each
/// callback node an answer settles rebuilds its tooltip from the inputs it keeps then, so two
/// answers are two rebuilds. A tooltip that keeps none takes the answer and stays as it is.
pub(in crate::script) fn answer_item_caches(lua: &Lua) -> Vec<mlua::Error> {
    let due: Vec<(u32, FrameHandle, usize)> = {
        let mut guard = lua.app_data_mut::<Model>().expect("model app_data");
        let model = &mut *guard;
        if model.item_cache_waits.is_empty() {
            return Vec::new();
        }
        let tips: Vec<FrameHandle> = model.item_cache_waits.keys().copied().collect();
        let mut due = Vec::new();
        for h in tips {
            let Some(mut waits) = model.item_cache_waits.remove(&h) else {
                continue;
            };
            let before = waits.len();
            waits.retain(|w| !w.answered(model));
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
                    .map(|b| b.refreshed(&model))
            };
            // Nothing kept (`0x52e556`): a clear in between, or a rebuild's own hide.
            let Some(build) = build else { break };
            let rebuilt = frame_wrapper(lua, id).and_then(|this| build_item(lua, &this, h, build));
            if let Err(e) = rebuilt {
                errors.push(e);
            }
        }
    }
    errors
}
