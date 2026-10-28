//! The active player's bags as the container getters and verbs read them at the call: the
//! reference's look the slot up in the player's list or the bag's (`GetItemInSlot`, `0x6228a0`;
//! the bag lookup `0x4f93e0`) as they run, so a listener reads the bags its own packet left. Each
//! question goes to the lent host ([`super::live::ask`]); with none to ask, the VM's copy answers,
//! which only a harness seeds ([`super::UiScript::set_container`]).

use mlua::Lua;

use super::container::ContainerSlot;
use super::live::{ask, LiveQuery};
use super::Model;

/// The item a bag slot holds now, as `GetItemInSlot` (`0x6228a0`) or the bag's own list finds it:
/// container `bag` (0 the backpack, 1-4 the worn bags, -1 the vault, 5-10 the bank bags, -2 the
/// keyring) at 1-based `slot`. `None` is an empty slot, one past the bag's size, a bag the player
/// does not hold, and a slot whose item object has not streamed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BagSlotQuery {
    pub bag: i64,
    pub slot: u32,
}

impl LiveQuery for BagSlotQuery {
    type Answer = Option<ContainerSlot>;
}

/// Container `bag` itself now: its size and, for bags 1-10, its item's name; `None` when the player
/// holds no such bag (`0x4f93e0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BagQuery(pub i64);

impl LiveQuery for BagQuery {
    type Answer = Option<BagHead>;
}

/// What a container is, past its slots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BagHead {
    pub num_slots: u32,
    /// `GetBagName` for bags 1-10 (the bag item's own name); the backpack, the vault and the
    /// keyring have none.
    pub name: Option<String>,
}

/// Every occupied slot of every container the player holds now, `(bag, slot, item)`: for a walk
/// over the bags (the auto-equip's find by guid, a held item's damage test).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BagItemsQuery;

impl LiveQuery for BagItemsQuery {
    type Answer = Vec<(i64, u32, ContainerSlot)>;
}

/// The container slot that holds the item object of this guid now, wherever it moved, no bank
/// gate: the client's find by guid through the object manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BagGuidQuery(pub u64);

impl LiveQuery for BagGuidQuery {
    type Answer = Option<(i64, u32, ContainerSlot)>;
}

/// `HasKey()` now (`0x48ae90`): whether the player owns a `BagFamily` key item anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HasKeyQuery;

impl LiveQuery for HasKeyQuery {
    type Answer = bool;
}

/// The slot `(bag, slot)` as a container binding finds it now, through the bag lookup's and
/// `GetItemInSlot`'s bank gates ([`Model::bank_bag_shut`], [`Model::vault_shut`]): the host's,
/// else the VM's copy.
// Deviation: the backpack, the bank and the keyring are each bounded by their own slots, where
// `0x4f990f` bounds them only by the player list's count (0x71) from their bases (the bank at
// `+0x27`, `0x4f98ae`; the keyring at `+0x51`, `0x4f98c0`), a reference bug that lets their index
// read the next container's slots.
pub(crate) fn bag_slot(lua: &Lua, model: &Model, bag: i64, slot: u32) -> Option<ContainerSlot> {
    if model.vault_shut(bag) || model.bank_bag_shut(bag) {
        return None;
    }
    held_slot(lua, model, bag, slot)
}

/// The slot `(bag, slot)` with no bank gate: what the client finds by an item's own place, so an
/// item held from a bank slot is still found after the banker closes.
pub(crate) fn held_slot(lua: &Lua, model: &Model, bag: i64, slot: u32) -> Option<ContainerSlot> {
    ask(lua, &BagSlotQuery { bag, slot }).unwrap_or_else(|| {
        model
            .containers
            .get(&bag)
            .and_then(|c| c.slots.get(&slot))
            .cloned()
    })
}

/// Container `bag` as the bag lookup finds it now (`0x4f93e0`), a shut bank bag none.
pub(crate) fn bag(lua: &Lua, model: &Model, bag: i64) -> Option<BagHead> {
    if model.bank_bag_shut(bag) {
        return None;
    }
    ask(lua, &BagQuery(bag)).unwrap_or_else(|| {
        model.containers.get(&bag).map(|c| BagHead {
            num_slots: c.num_slots,
            name: c.name.clone(),
        })
    })
}

/// Whether the slot resolver `0x4f9820` finds a list for container `id`: the player's own for
/// the backpack, the bank and the keyring (`0x4f988d`-`0x4f98c3`), a bag through [`bag`] for 1-10
/// (`0x4f98c5`-`0x4f98f2`), none above.
pub(crate) fn bag_resolves(lua: &Lua, model: &Model, id: i64) -> bool {
    id <= 0 || bag(lua, model, id).is_some()
}

/// Every occupied slot the player's containers hold now ([`BagItemsQuery`]), no bank gate.
pub(crate) fn bag_items(lua: &Lua, model: &Model) -> Vec<(i64, u32, ContainerSlot)> {
    ask(lua, &BagItemsQuery).unwrap_or_else(|| {
        let mut out: Vec<(i64, u32, ContainerSlot)> = model
            .containers
            .iter()
            .flat_map(|(&bag, c)| c.slots.iter().map(move |(&slot, s)| (bag, slot, s.clone())))
            .collect();
        out.sort_by_key(|&(bag, slot, _)| (bag, slot));
        out
    })
}

/// The container slot holding the item object `guid` now ([`BagGuidQuery`]); none for guid 0.
pub(crate) fn find_guid(lua: &Lua, model: &Model, guid: u64) -> Option<(i64, u32, ContainerSlot)> {
    if guid == 0 {
        return None;
    }
    ask(lua, &BagGuidQuery(guid)).unwrap_or_else(|| {
        let mut held: Vec<(i64, u32, &ContainerSlot)> = model
            .containers
            .iter()
            .flat_map(|(&bag, c)| c.slots.iter().map(move |(&slot, s)| (bag, slot, s)))
            .filter(|(_, _, s)| s.guid == guid)
            .collect();
        held.sort_by_key(|&(bag, slot, _)| (bag, slot));
        held.first().map(|&(bag, slot, s)| (bag, slot, s.clone()))
    })
}

/// `HasKey()` now ([`HasKeyQuery`]).
pub(crate) fn has_key(lua: &Lua, model: &Model) -> bool {
    ask(lua, &HasKeyQuery).unwrap_or(model.has_key)
}

/// The bags 1 to 10 the player holds now, as the world-enter cascade's rescan finds them
/// (`0x4f91a0`).
pub(crate) fn held_bags(lua: &Lua, model: &Model) -> Vec<i64> {
    (1..=10)
        .filter(|&n| {
            ask(lua, &BagQuery(n))
                .map(|b| b.is_some())
                .unwrap_or_else(|| model.containers.contains_key(&n))
        })
        .collect()
}
