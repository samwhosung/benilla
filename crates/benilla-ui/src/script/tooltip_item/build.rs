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
//! source's own view, which the app re-pushes once it resolves. [`answer_item_caches`] runs once a
//! tick and settles every wait at once.
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
    /// The view the instance's names and petition were read from, re-read when they answer.
    pub(in crate::script) names: Option<NameSource>,
    /// The names and petition the last render read and lacked.
    pub(super) lacked: Lacked,
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

/// The view a source read the instance's names from: benilla's name and petition caches answer
/// inside it.
#[derive(Clone, PartialEq)]
pub(in crate::script) enum NameSource {
    /// A bag slot, the cursor's numbering (`SetBagItem`, and the sell and send-mail slots).
    Bag { bag: i64, slot: u32 },
    /// A unit's inventory slot (`SetInventoryItem`).
    Inventory { unit: String, slot: usize },
    /// A trade slot, 1-based (`SetTradePlayerItem`, `SetTradeTargetItem`).
    Trade { player_side: bool, slot: usize },
    /// The player's own item of the entry, as the action bar finds it.
    Owned,
}

/// The fields of a source's view that a cache fills.
#[derive(PartialEq)]
struct Names {
    creator: Option<String>,
    gift_creator: Option<String>,
    petition: Option<crate::script::PetitionSlotView>,
}

impl NameSource {
    /// The names the source holds now, `None` once it no longer holds the item.
    fn read(&self, model: &Model, item_id: u32) -> Option<Names> {
        match self {
            NameSource::Bag { bag, slot } => model
                .containers
                .get(bag)
                .and_then(|c| c.slots.get(slot))
                .filter(|s| s.item_id == item_id)
                .map(|s| Names {
                    creator: s.creator.clone(),
                    gift_creator: s.gift_creator.clone(),
                    petition: s.petition.clone(),
                }),
            NameSource::Inventory { unit, slot } => model
                .inv_slot(unit, *slot)
                .filter(|s| s.item_id == item_id)
                .map(|s| Names {
                    creator: s.creator,
                    gift_creator: s.gift_creator,
                    petition: None,
                }),
            NameSource::Trade { player_side, slot } => model
                .trade
                .as_ref()
                .and_then(|t| {
                    let side = if *player_side { &t.player } else { &t.target };
                    slot.checked_sub(1)
                        .and_then(|n| side.slots.get(n))?
                        .as_ref()
                })
                .filter(|s| s.item_id == item_id)
                .map(|s| Names {
                    creator: None,
                    gift_creator: s.gift_creator.clone(),
                    petition: None,
                }),
            NameSource::Owned => super::owned_instance(model, item_id).map(|i| Names {
                creator: i.creator,
                gift_creator: i.gift_creator,
                petition: i.petition,
            }),
        }
    }
}

/// Which of the cache-filled fields the render read and found empty: the creator for the maker
/// line, the giver for a gift's, the petition and its owner for a charter's.
#[derive(Clone, Copy, Default)]
pub(super) struct Lacked {
    creator: bool,
    gift_creator: bool,
    petition: bool,
}

impl Lacked {
    fn of(v: &ItemTemplateView, inst: &ItemInstance) -> Self {
        let wrapped = inst.wrapped();
        Self {
            creator: !wrapped && inst.creator.is_none(),
            gift_creator: wrapped && inst.gift_creator.is_none(),
            petition: v.flags & 0x2000 != 0
                && inst.petition.as_ref().is_none_or(|p| p.owner.is_none()),
        }
    }

    fn any(self) -> bool {
        self.creator || self.gift_creator || self.petition
    }

    /// A field the render lacked is there now.
    fn answered_by(self, now: &Names) -> bool {
        (self.creator && now.creator.is_some())
            || (self.gift_creator && now.gift_creator.is_some())
            || (self.petition && now.petition.as_ref().is_some_and(|p| p.owner.is_some()))
    }
}

/// A cache a build missed, with the tooltip as the answer's cookie.
#[derive(Clone, PartialEq)]
pub(crate) enum CacheWait {
    /// The item record cache (`0xc0e2a0`): the item itself, or a lock's key.
    Template(u32),
    /// A set's view.
    Set(u32),
    /// The item-name cache (`0xc0e2dc`): a set whose members' names were not all in.
    SetMembers { set: u32, unnamed: usize },
}

impl CacheWait {
    fn answered(&self, model: &Model) -> bool {
        match *self {
            CacheWait::Template(id) => model.item_templates.contains_key(&id),
            CacheWait::Set(id) => model.item_sets.contains_key(&id),
            CacheWait::SetMembers { set, unnamed } => model
                .item_sets
                .get(&set)
                .is_some_and(|v| unnamed_members(v) < unnamed),
        }
    }
}

/// How many of a set's members still have no name.
pub(super) fn unnamed_members(v: &crate::script::ItemSetView) -> usize {
    v.members.iter().filter(|(_, n)| n.is_none()).count()
}

/// Records a miss of the build under way; [`build_item`] hands it to its tooltip.
pub(super) fn note_miss(model: &mut Model, wait: CacheWait) {
    model.item_cache_misses.push(wait);
}

/// Hands the misses recorded since the last call to the tooltip `h` as its waits.
pub(super) fn keep_misses(model: &mut Model, h: FrameHandle) {
    let misses = std::mem::take(&mut model.item_cache_misses);
    if misses.is_empty() {
        return;
    }
    let waits = model.item_cache_waits.entry(h).or_default();
    for m in misses {
        if !waits.contains(&m) {
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
    mut build: ItemBuild,
) -> mlua::Result<bool> {
    clear(lua, h);
    // What an earlier render outside a build noted is no miss of this one.
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .item_cache_misses
        .clear();
    let view = view_of(lua, build.item_id);
    let live = match &view {
        Some(v) => {
            let live = render::render_view_flagged(lua, this, v, build.flags, build.inst.as_ref())?;
            build.lacked = match (&build.names, &build.inst) {
                (Some(_), Some(inst)) => Lacked::of(v, inst),
                _ => Lacked::default(),
            };
            live
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
    let sell = build.sell_count.zip(view.as_ref().map(|v| v.sell_price));
    {
        let mut model = lua.app_data_mut::<Model>().expect("model app_data");
        keep_misses(&mut model, h);
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
/// tooltip a cache answered rebuilds from the inputs it keeps, once however many answered. One
/// that keeps none takes the answer and stays as it is.
pub(in crate::script) fn answer_item_caches(lua: &Lua) -> Vec<mlua::Error> {
    let due: Vec<(u32, FrameHandle)> = {
        let mut guard = lua.app_data_mut::<Model>().expect("model app_data");
        let model = &mut *guard;
        if model.item_cache_waits.is_empty() && model.item_builds.is_empty() {
            return Vec::new();
        }
        let mut tips: Vec<FrameHandle> = model.item_cache_waits.keys().copied().collect();
        tips.extend(
            model
                .item_builds
                .iter()
                .filter(|(h, b)| b.lacked.any() && !model.item_cache_waits.contains_key(h))
                .map(|(h, _)| *h),
        );
        let mut due = Vec::new();
        for h in tips {
            let mut answered = false;
            if let Some(mut waits) = model.item_cache_waits.remove(&h) {
                let before = waits.len();
                waits.retain(|w| !w.answered(model));
                answered = waits.len() != before;
                if !waits.is_empty() {
                    model.item_cache_waits.insert(h, waits);
                }
            }
            let Some(build) = model.item_builds.get(&h) else {
                continue;
            };
            let now = build
                .lacked
                .any()
                .then(|| build.names.as_ref()?.read(model, build.item_id))
                .flatten();
            answered |= now.as_ref().is_some_and(|n| build.lacked.answered_by(n));
            if !answered {
                continue;
            }
            // The rebuild reads the names the source holds now.
            if let Some(now) = now {
                if let Some(inst) = model.item_builds.get_mut(&h).and_then(|b| b.inst.as_mut()) {
                    inst.creator = now.creator;
                    inst.gift_creator = now.gift_creator;
                    inst.petition = now.petition;
                }
            }
            due.push((model.frame_id(h), h));
        }
        due
    };
    let mut errors = Vec::new();
    for (id, h) in due {
        let build = {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            model.item_builds.get(&h).cloned()
        };
        let Some(build) = build else { continue };
        let rebuilt = frame_wrapper(lua, id).and_then(|this| build_item(lua, &this, h, build));
        if let Err(e) = rebuilt {
            errors.push(e);
        }
    }
    errors
}
