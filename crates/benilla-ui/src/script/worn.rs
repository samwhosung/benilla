//! The active player's worn items as the slot getters read them at the call: the reference's look
//! the slot up in the player's item list as they run (`GetItemInSlot`, `0x6228a0`;
//! `GetInventoryItemLink` at `0x4c8cd3`), so a listener reads the doll its own packet left. Each
//! question goes to the lent host ([`super::live::ask`]); with none to ask, the VM's copy answers,
//! which only a harness seats ([`super::UiScript::set_inventory_slots`]).

use mlua::Lua;

use super::char_stats::{InvSlotView, INVENTORY_SLOT_COUNT};
use super::live::{ask, LiveQuery};
use super::Model;

/// The bank bag slots' live-API ids, the player list's 63..=68.
const BANK_BAG_IDS: std::ops::RangeInclusive<usize> = 64..=69;

/// The active player's item at live-API id `slot` now: its object in the player's item list, and
/// for the ammo slot 0 `PLAYER_AMMO_ID` and its record. 0 the ammo, 1..=23 the doll and the
/// equipped bags, 64..=69 the bank bags, with no bank gate; `None` is an empty slot, an object not
/// streamed, or an id past those bands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WornSlotQuery(pub usize);

impl LiveQuery for WornSlotQuery {
    type Answer = Option<InvSlotView>;
}

/// [`WornSlotQuery`] now: the host's, else the VM's copy.
pub(crate) fn worn_slot(lua: &Lua, model: &Model, slot: usize) -> Option<InvSlotView> {
    ask(lua, &WornSlotQuery(slot)).unwrap_or_else(|| {
        if BANK_BAG_IDS.contains(&slot) {
            model
                .bank_bag_slots
                .get(slot - BANK_BAG_IDS.start())?
                .clone()
        } else {
            model.inventory_slots.get(slot)?.clone()
        }
    })
}

/// Each slot 0..=23's item object guid and item id now, read off the player's list and the
/// objects with no view built: what a walk over the doll matches on before it reads the one slot
/// it wants ([`worn_slot`]). The ammo slot is `PLAYER_AMMO_ID` with guid 0; `None` is a slot
/// [`WornSlotQuery`] finds empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WornKeysQuery;

/// [`WornKeysQuery`]'s answer, by live id.
pub type WornKeys = [Option<(u64, u32)>; INVENTORY_SLOT_COUNT];

impl LiveQuery for WornKeysQuery {
    type Answer = WornKeys;
}

/// [`WornKeysQuery`] now: the host's, else the VM's copy.
pub(crate) fn worn_keys(lua: &Lua, model: &Model) -> WornKeys {
    ask(lua, &WornKeysQuery).unwrap_or_else(|| {
        std::array::from_fn(|i| {
            model.inventory_slots[i]
                .as_ref()
                .map(|s| (s.guid, s.item_id))
        })
    })
}

/// Every occupied slot 0..=23 now, in live-id order, each read as the walk reaches it.
pub(crate) fn worn_iter<'a>(
    lua: &'a Lua,
    model: &'a Model,
) -> impl Iterator<Item = InvSlotView> + 'a {
    (0..INVENTORY_SLOT_COUNT).filter_map(move |i| worn_slot(lua, model, i))
}
