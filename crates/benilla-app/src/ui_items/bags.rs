//! The active player's bags and worn items as the container and slot getters and verbs read them
//! at the call ([`benilla_ui::script::BagSlotQuery`] and its kin, and
//! [`benilla_ui::script::WornSlotQuery`]): the reference's look the slot up in the player's list
//! or the bag's as they run (`GetItemInSlot`, `0x6228a0`; the bag lookup `0x4f93e0`), so a
//! listener reads the bags and the doll its own packet left. The player's store and the item
//! objects are read as of the drain's place ([`crate::ui_unit::views::ViewStores`]), each slot
//! derived as it is asked for ([`super::feed::resolve_slot`]). With no player held the family
//! answers nothing: a logout's `PLAYER_LOGOUT` (`0x490c2a`) runs after the object manager is gone
//! (`0x402011` before `0x402039`), where `GetContainerNumSlots` finds no active player and returns
//! 0 (`0x4f95ab`), and only a harness's copy stands in.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_protocol::ObjectFields;
use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    BagGuidQuery, BagHead, BagItemsQuery, BagQuery, BagSlotQuery, ContainerSlot, HasKeyQuery,
    LiveReader, WeaponEnchantQuery, WornKeysQuery, WornSlotQuery,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{ObjectStore, Objects};
use crate::query_cache::Ask;

use super::{
    has_key, keyring_size, ItemObjects, BAG_SLOT_FIRST, BANK_BAG_ID_FIRST, BANK_CONTAINER,
    BANK_SLOTS, KEYRING_CONTAINER, KEYRING_SLOTS, PACK_SLOTS,
};

/// Every container id a getter can name, in the order a walk over them reads.
const CONTAINERS: [i64; 13] = [-2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

/// What the container getters read as they run.
#[derive(SystemParam)]
pub(crate) struct BagLive<'w, 's> {
    reads: BagReads<'w, 's>,
    cooldowns: Option<Res<'w, crate::spell::Cooldowns>>,
}

/// [`BagLive`] but the cooldown list, for a read in a system that holds the list to write (the
/// cast ladder's [`crate::spell::CastLadder`]), which lends both ([`Self::with`]).
#[derive(SystemParam)]
pub(crate) struct BagReads<'w, 's> {
    objects: Objects<'w, 's>,
    stores: crate::ui_unit::views::ViewStores<'w, 's>,
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    rolls: crate::items::Rolls<'w>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    names: Res<'w, crate::names::NameCache>,
    petitions: Option<Res<'w, crate::ui_petition::PetitionRecords>>,
    /// The interface clock's pair every start is mapped through, as the pushes map theirs.
    clock: Option<Res<'w, crate::ui_script::HostClock>>,
    /// `ChrClasses.dbc` field 16, the fit rule's relic flag.
    classes: Option<Res<'w, crate::chr_classes::ChrClassTable>>,
    /// `ItemSubClass.dbc`, for `GetInventoryItemCount`'s container count bit.
    sub_classes: Option<Res<'w, super::ItemSubClasses>>,
}

impl<'w, 's> BagReads<'w, 's> {
    /// The bags, their slots' cooldowns read off `cooldowns`.
    pub(crate) fn with<'a>(
        &'a self,
        cooldowns: Option<&'a crate::spell::Cooldowns>,
    ) -> Bags<'a, 'w, 's> {
        Bags {
            reads: self,
            cooldowns,
        }
    }
}

impl<'w, 's> BagLive<'w, 's> {
    fn bags(&self) -> Bags<'_, 'w, 's> {
        self.reads.with(self.cooldowns.as_deref())
    }
}

/// The bags as one read sees them: [`BagReads`] and the cooldown list.
pub(crate) struct Bags<'a, 'w, 's> {
    reads: &'a BagReads<'w, 's>,
    cooldowns: Option<&'a crate::spell::Cooldowns>,
}

impl<'w, 's> std::ops::Deref for Bags<'_, 'w, 's> {
    type Target = BagReads<'w, 's>;
    fn deref(&self) -> &Self::Target {
        self.reads
    }
}

/// One container's head and the guids its slots hold, as the lookups find them.
struct Held {
    head: BagHead,
    /// 1-based slot to guid, 0 for an empty slot.
    guids: Vec<u64>,
}

impl Bags<'_, '_, '_> {
    /// The player's store as of the drain's place, `None` while no player is held.
    fn player(&self) -> Option<&ObjectStore> {
        self.stores.get(self.objects.player()?).ok()
    }

    /// Container `bag` as the bag lookup finds it (`0x4f93e0`): the player's own arrays for the
    /// backpack, the vault and the keyring, a bag item's container object for 1-10; `None` for
    /// an empty bag slot or an id past them.
    fn held(&self, me: &ObjectFields, held: &dyn ItemObjects, bag: i64) -> Option<Held> {
        let own = |n: u8, at: &dyn Fn(u8) -> Option<u64>| -> Vec<u64> {
            (0..n).map(|i| at(i).unwrap_or(0)).collect()
        };
        let bag_item = |bag_guid: Option<u64>| -> Option<Held> {
            let bag_guid = bag_guid.filter(|&g| g != 0)?;
            // A bag whose object has not streamed is held with no slots.
            let (entry, num_slots, guids) = match held.object(bag_guid) {
                Some(f) => {
                    let n = f.container_num_slots().unwrap_or(0);
                    let guids = (0..n.min(36) as u8)
                        .map(|j| f.container_slot(j).unwrap_or(0))
                        .collect();
                    (f.object_entry().unwrap_or(0), n, guids)
                }
                None => (0, 0, Vec::new()),
            };
            // `GetBagName` only reads (`0x4fa75e`).
            let name = (entry != 0)
                .then(|| self.items.template(entry, bag_guid, Ask::Read))
                .flatten()
                .map(|t| t.name.clone());
            Some(Held {
                head: BagHead { num_slots, name },
                guids,
            })
        };
        match bag {
            0 => Some(Held {
                head: BagHead {
                    num_slots: u32::from(PACK_SLOTS),
                    name: None,
                },
                guids: own(PACK_SLOTS, &|i| me.player_pack_slot(i)),
            }),
            1..=4 => bag_item(me.player_inv_slot(BAG_SLOT_FIRST + bag as u8 - 1)),
            BANK_CONTAINER => Some(Held {
                head: BagHead {
                    num_slots: u32::from(BANK_SLOTS),
                    name: None,
                },
                guids: own(BANK_SLOTS, &|i| me.player_bank_slot(i)),
            }),
            5..=10 => bag_item(me.player_bank_bag_slot((bag - BANK_BAG_ID_FIRST) as u8)),
            // Sized by the level ladder stock `GetKeyRingSize` (`ContainerFrame.lua:773`) reads
            // off `UnitLevel` and the server shares; the slots past it, which the server refuses,
            // hold nothing.
            KEYRING_CONTAINER => {
                let size = keyring_size(me.unit_level().unwrap_or(1));
                let n = size.min(u32::from(KEYRING_SLOTS)) as u8;
                Some(Held {
                    head: BagHead {
                        num_slots: size,
                        name: None,
                    },
                    guids: own(n, &|i| me.player_keyring_slot(i)),
                })
            }
            _ => None,
        }
    }

    /// The item `guid` as its slot shows it; `None` for an empty slot or an unstreamed object.
    fn slot(&self, me: &ObjectFields, held: &dyn ItemObjects, guid: u64) -> Option<ContainerSlot> {
        // The class's relic flag; without client data it is false, an ordinary ranged slot.
        let has_relic_slot = me.unit_class().is_some_and(|c| {
            self.classes
                .as_deref()
                .is_some_and(|t| t.0.has_relic_slot(u32::from(c)))
        });
        let clock = self.clock.as_deref().map(|c| (c.0.anchor, c.0.ui_now));
        super::feed::resolve_slot(
            guid,
            held,
            &self.items,
            self.icons.as_deref(),
            self.rolls.get(),
            self.cooldowns
                .zip(clock)
                .map(|(c, (now, ui_now))| (c, now, ui_now)),
            self.spells.as_deref().map(|s| &s.catalog),
            &self.names,
            self.petitions.as_deref(),
            has_relic_slot,
        )
    }

    /// Every occupied slot of every container the player holds, `(bag, slot, guid)`, in the
    /// walk's order.
    fn occupied(&self, me: &ObjectFields, held: &dyn ItemObjects) -> Vec<(i64, u32, u64)> {
        CONTAINERS
            .iter()
            .filter_map(|&bag| Some((bag, self.held(me, held, bag)?)))
            .flat_map(|(bag, h)| {
                h.guids
                    .into_iter()
                    .enumerate()
                    .filter(|&(_, g)| g != 0)
                    .map(move |(j, g)| (bag, j as u32 + 1, g))
            })
            .collect()
    }

    /// `ask` answered off the player's bags as of here; `false` for not one of the family's.
    fn answer_held(&self, me: &ObjectFields, ask: &mut LiveAsk<'_>) -> bool {
        let objects = self.stores.item_objects(&self.objects);
        let objects: &dyn ItemObjects = &objects;
        ask.answer(|q: &BagSlotQuery| {
            Some(self.held(me, objects, q.bag).and_then(|h| {
                let i = usize::try_from(q.slot.checked_sub(1)?).ok()?;
                self.slot(me, objects, *h.guids.get(i)?)
            }))
        }) || ask.answer(|q: &BagQuery| Some(self.held(me, objects, q.0).map(|h| h.head)))
            || ask.answer(|_: &BagItemsQuery| {
                Some(
                    self.occupied(me, objects)
                        .into_iter()
                        .filter_map(|(bag, slot, g)| Some((bag, slot, self.slot(me, objects, g)?)))
                        .collect(),
                )
            })
            || ask.answer(|q: &BagGuidQuery| {
                Some(
                    self.occupied(me, objects)
                        .into_iter()
                        .find(|&(_, _, g)| q.0 != 0 && g == q.0)
                        .and_then(|(bag, slot, g)| Some((bag, slot, self.slot(me, objects, g)?))),
                )
            })
            || ask.answer(|_: &HasKeyQuery| Some(has_key(me, objects, &self.items)))
            || ask.answer(|q: &WornSlotQuery| {
                Some(crate::ui_char::worn_slot_view(
                    me,
                    objects,
                    &self.items,
                    self.icons.as_deref(),
                    self.rolls.get(),
                    &self.names,
                    self.sub_classes.as_deref(),
                    self.classes.as_deref().map(|t| &t.0),
                    q.0,
                ))
            })
            || ask.answer(|_: &WornKeysQuery| Some(crate::ui_char::worn_keys(me, objects)))
            || ask
                .answer(|_: &WeaponEnchantQuery| Some(crate::ui_char::weapon_enchants(me, objects)))
    }
}

impl Bags<'_, '_, '_> {
    /// `ask` answered off the bags; `false` for not one of the family's.
    fn answer(&self, ask: &mut LiveAsk<'_>) -> bool {
        // No player declines, and a harness's copy answers.
        self.player()
            .is_some_and(|store| self.answer_held(&store.0, ask))
    }
}

impl crate::script_calls::live::LiveFamily for BagLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        self.bags().answer(ask)
    }
}

/// An engine turn's or a host apply's read of the bags, with no host to ask
/// ([`benilla_ui::script::UiScript::reading`]).
impl LiveReader for BagLive<'_, '_> {
    fn read_live(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        self.bags().answer(ask)
    }
}

impl LiveReader for Bags<'_, '_, '_> {
    fn read_live(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        Bags::answer(self, ask)
    }
}

/// `chunk` on the world's VM with the bags lent as its reads, as an engine turn lends them
/// ([`benilla_ui::script::UiScript::reading`]).
#[cfg(test)]
pub(crate) fn eval_with_bags<T: benilla_ui::mlua::FromLuaMulti>(
    world: &mut World,
    chunk: &str,
) -> T {
    let mut state = bevy::ecs::system::SystemState::<BagLive>::new(world);
    let mut script = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("a VM");
    let out = {
        let mut bags = state.get(world);
        script
            .reading(&mut bags, |s| s.eval::<T>(chunk))
            .unwrap_or_else(|e| panic!("{chunk}: {e}"))
    };
    world.insert_non_send_resource(script);
    out
}

#[cfg(test)]
#[path = "bags_cooldown_tests.rs"]
mod cooldown_tests;

#[cfg(test)]
#[path = "bags_link_tests.rs"]
mod link_tests;
