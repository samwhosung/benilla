//! The server's half of the client's item lock. The lock is the reference's `item+0x314` bit 0,
//! held in the VM by guid ([`UiScript::lock_item`]): the cursor verbs set and lift it at their own
//! sites, and the app's sends lock their item. What lifts it from here is the server: a slot whose
//! guid changes unlocks the item that left it (`0x5ddd50` over the player's array, `0x4f8e15` and
//! `0x4f912c` over a bag's), an item of ours that arrives or changes its stack count unlocks its
//! entry (`0x5d85ab`, `0x5d94ca`), and the handlers without a VM queue theirs in
//! [`ItemReleases`]: a refused operation, a sale, a loot release.

use std::collections::HashMap;

use benilla_protocol::ObjectFields;
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::items::Items;
use crate::net::{NetCommands, Objects};

/// One server-side release, run into the VM by [`apply_item_releases`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Release {
    /// `UnlockItem` (`0x495420`) on one guid: its `ITEM_LOCK_CHANGED` fires whether or not the
    /// guid names an item we hold (`0x495450`).
    Item(u64),
    /// A stackable item's arrival or stack-count change (`0x5d8590`, `0x5d94af`): every item of
    /// ours with its entry unlocks, bar the trade, mail and auction attachments, and one
    /// `ITEM_LOCK_CHANGED` fires ([`UiScript::unlock_entry_items`]).
    Entry(Vec<u64>),
}

/// The releases the handlers without a VM queue, run ahead of this frame's inventory feeds.
#[derive(Resource, Default, Debug)]
pub(crate) struct ItemReleases(pub(crate) Vec<Release>);

impl ItemReleases {
    /// `UnlockItem` on `guid`; a zero guid does nothing (`0x49542d`).
    pub(crate) fn unlock(&mut self, guid: u64) {
        if guid != 0 {
            self.0.push(Release::Item(guid));
        }
    }

    /// Run every queued release into the VM, in order.
    pub(crate) fn apply(&mut self, script: &mut UiScript) {
        for release in self.0.drain(..) {
            match release {
                Release::Item(guid) => script.unlock_item(guid),
                Release::Entry(guids) => script.unlock_entry_items(&guids),
            }
        }
    }
}

/// Where an item sits, as the reference's watchers key it: an index of the player's own array
/// (`[player+0x1d3c]`), or a slot of a container object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Seat {
    Player(u8),
    Bag(u64, u8),
}

/// One item as the watchers see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Item {
    entry: u32,
    count: u32,
    stackable: bool,
}

/// The player's items as the watchers last saw them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Snapshot {
    seats: HashMap<Seat, u64>,
    /// Each item's entry, stack count and whether its template stacks past 1 (`[rec+0x60]`), for
    /// the items whose objects and templates have arrived: the arrival unlock waits for the
    /// template (`0x5d8346` defers to `0x5d8390`).
    items: HashMap<u64, Item>,
}

/// The player's array in the reference's numbering: equipment and bags (0-22), the backpack
/// (23-38), the bank (39-62), its bags (63-68), buyback (69-80) and the keyring (81-112).
fn player_array(store: &ObjectFields) -> impl Iterator<Item = (u8, u64)> + '_ {
    let inv = (0..23u8).map(|i| (i, store.player_inv_slot(i)));
    let pack = (0..16u8).map(|i| (23 + i, store.player_pack_slot(i)));
    let bank = (0..24u8).map(|i| (39 + i, store.player_bank_slot(i)));
    let bank_bags = (0..6u8).map(|i| (63 + i, store.player_bank_bag_slot(i)));
    let buyback = (0..12u8).map(|i| (69 + i, store.player_buyback_slot(i)));
    let keyring = (0..32u8).map(|i| (81 + i, store.player_keyring_slot(i)));
    inv.chain(pack)
        .chain(bank)
        .chain(bank_bags)
        .chain(buyback)
        .chain(keyring)
        .map(|(i, guid)| (i, guid.unwrap_or(0)))
}

/// Read the watchers' view off the self descriptor and the item objects; `stackable` answers
/// from the template cache, `None` while the template is unanswered.
pub(crate) fn snapshot(
    store: &ObjectFields,
    objects: &Objects,
    stackable: impl Fn(u32, u64) -> Option<bool>,
) -> Snapshot {
    let mut seats: HashMap<Seat, u64> = player_array(store)
        .map(|(i, guid)| (Seat::Player(i), guid))
        .collect();
    // The equipped bags' and the bank bags' contents, keyed by the container object.
    let bags: Vec<u64> = (19..23u8)
        .chain(63..69)
        .filter_map(|i| seats.get(&Seat::Player(i)).copied())
        .filter(|&g| g != 0)
        .collect();
    for bag in bags {
        let Some(fields) = objects.object(bag) else {
            continue;
        };
        let n = fields.container_num_slots().unwrap_or(0).min(36) as u8;
        for j in 0..n {
            seats.insert(Seat::Bag(bag, j), fields.container_slot(j).unwrap_or(0));
        }
    }
    let items = seats
        .values()
        .filter(|&&g| g != 0)
        .filter_map(|&guid| {
            let fields = objects.object(guid)?;
            let entry = fields.object_entry()?;
            let item = Item {
                entry,
                count: fields.item_stack_count().unwrap_or(1),
                stackable: stackable(entry, guid)?,
            };
            Some((guid, item))
        })
        .collect();
    Snapshot { seats, items }
}

/// The releases between two looks. A seat whose guid changed unlocks the item that left it
/// (`0x5ddd50`, `0x4f8e15`, `0x4f912c`); a bag's seats go with it unseen. An item that arrived
/// (`0x5d8440` into `0x5d85ab`) or whose stack count changed (`0x5d9400` into `0x5d94ca`) unlocks
/// its whole entry when its template stacks past 1 (`[rec+0x60] > 1`, `0x5d857b`, `0x5d947e`),
/// and itself otherwise. Sorted, so the event stream is reproducible.
pub(crate) fn releases(was: &Snapshot, now: &Snapshot) -> Vec<Release> {
    let mut left: Vec<u64> = was
        .seats
        .iter()
        .filter(|&(_, &old)| old != 0)
        .filter(|&(seat, &old)| match now.seats.get(seat) {
            Some(&new) => new != old,
            // A bag that left takes its seats with it: their watchers never see a write.
            None => matches!(seat, Seat::Player(_)),
        })
        .map(|(_, &old)| old)
        .collect();
    left.sort_unstable();
    let mut out: Vec<Release> = left.into_iter().map(Release::Item).collect();
    let mut moved: Vec<(u64, Item)> = now
        .items
        .iter()
        .filter(|&(guid, item)| was.items.get(guid).is_none_or(|w| w.count != item.count))
        .map(|(&guid, &item)| (guid, item))
        .collect();
    moved.sort_unstable_by_key(|&(guid, _)| guid);
    for (guid, item) in moved {
        if item.stackable {
            let mut kin: Vec<u64> = now
                .items
                .iter()
                .filter(|&(_, i)| i.entry == item.entry)
                .map(|(&g, _)| g)
                .collect();
            kin.sort_unstable();
            out.push(Release::Entry(kin));
        } else {
            out.push(Release::Item(guid));
        }
    }
    out
}

/// The watchers' last look, per VM: a fresh VM, like the reference's game UI coming up
/// (`0x4908ce`, which the arrival unlock reads at `0x5d84ff`), takes a baseline and releases
/// nothing for what it finds.
#[derive(Default)]
pub(crate) struct LockWatch(Option<Snapshot>, u64);

/// Run the frame's releases ahead of the inventory feeds: the watchers compare the incoming guid
/// with the one still in the descriptor (`0x5ddd42`), so a handler reads the slot as it was.
pub(crate) fn apply_item_releases(
    script: Option<NonSendMut<UiScript>>,
    mut inv: crate::items::Inventory,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut queued: ResMut<ItemReleases>,
    // The loot window's closes and refusals owe theirs on the session.
    loot: Option<ResMut<crate::ui_loot::LootState>>,
    mut memo: Local<crate::ui_script::VmMemo<LockWatch>>,
) {
    // Read before taking, so a frame owing nothing leaves the session unchanged for its feed.
    if let Some(mut loot) = loot.filter(|l| l.owes_unlocks()) {
        queued
            .0
            .extend(loot.take_item_unlocks().into_iter().map(Release::Item));
    }
    let Some(mut script) = script else {
        queued.0.clear();
        return;
    };
    let watch = memo.get(&script);
    let Some(store) = inv.self_store.iter().next() else {
        watch.0 = None;
        queued.apply(&mut script);
        return;
    };
    let moved = inv.changes.moved() || !inv.self_changed.is_empty();
    let mut out = Vec::new();
    // A template answer moves no item object, so the frame it lands has its own trigger.
    let templates = items.template_epoch();
    let answered = watch.1 != templates;
    watch.1 = templates;
    if moved || answered || watch.0.is_none() {
        let now = snapshot(&store.0, &inv.objects, |entry, guid| {
            items
                .template(entry, guid, &commands)
                .map(|t| t.stackable > 1)
        });
        if let Some(was) = watch.0.replace(now) {
            out = releases(&was, watch.0.as_ref().expect("just stored"));
        }
    }
    queued.0.splice(0..0, out);
    queued.apply(&mut script);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(seats: &[(Seat, u64)], items: &[(u64, u32, u32, bool)]) -> Snapshot {
        Snapshot {
            seats: seats.iter().copied().collect(),
            items: items
                .iter()
                .map(|&(g, entry, count, stackable)| {
                    let item = Item {
                        entry,
                        count,
                        stackable,
                    };
                    (g, item)
                })
                .collect(),
        }
    }

    const A: u64 = 0x4000_0000_0000_00a1;
    const B: u64 = 0x4000_0000_0000_00b2;
    const BAG: u64 = 0x4000_0000_0000_0b06;

    #[test]
    fn a_seat_that_changes_guid_unlocks_the_item_that_left() {
        let was = snap(
            &[(Seat::Player(23), A), (Seat::Player(24), 0)],
            &[(A, 1, 1, false)],
        );
        let now = snap(
            &[(Seat::Player(23), 0), (Seat::Player(24), A)],
            &[(A, 1, 1, false)],
        );
        assert_eq!(
            releases(&was, &now),
            [Release::Item(A)],
            "the move: only the vacated seat's old guid, the filled seat had none"
        );
        let swapped = snap(&[(Seat::Player(23), B), (Seat::Player(24), A)], &[]);
        let was = snap(&[(Seat::Player(23), A), (Seat::Player(24), B)], &[]);
        assert_eq!(
            releases(&was, &swapped),
            [Release::Item(A), Release::Item(B)],
            "a swap unlocks both"
        );
    }

    #[test]
    fn a_bag_that_leaves_takes_its_seats_silently() {
        let was = snap(&[(Seat::Player(19), BAG), (Seat::Bag(BAG, 0), A)], &[]);
        let now = snap(&[(Seat::Player(19), 0)], &[]);
        assert_eq!(releases(&was, &now), [Release::Item(BAG)]);
    }

    #[test]
    fn an_arrival_or_a_restack_unlocks_its_entry_when_it_stacks() {
        let was = snap(&[], &[(A, 7, 5, true)]);
        let now = snap(&[], &[(A, 7, 3, true), (B, 7, 2, true)]);
        let kin = Release::Entry(vec![A, B]);
        assert_eq!(
            releases(&was, &now),
            [kin.clone(), kin],
            "the source's restack and the split's new stack, each the whole entry"
        );
        let was = snap(&[], &[(A, 9, 1, false)]);
        let now = snap(&[], &[(A, 9, 1, false), (B, 9, 1, false)]);
        assert_eq!(
            releases(&was, &now),
            [Release::Item(B)],
            "a template that does not stack unlocks only the arrival"
        );
        assert!(releases(&now, &now).is_empty(), "no change, no release");
    }

    /// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = 532;
    const F_OBJECT_ENTRY: u16 = 3;
    const F_ITEM_STACK_COUNT: u16 = 14;
    const ARROWS: u32 = 2512;

    fn pack(slots: &[u64]) -> ObjectFields {
        let pairs: Vec<(u16, u32)> = slots
            .iter()
            .enumerate()
            .flat_map(|(i, &g)| {
                let f = F_PACK_SLOT_1 + 2 * i as u16;
                [(f, g as u32), (f + 1, (g >> 32) as u32)]
            })
            .collect();
        ObjectFields::from_pairs(&pairs)
    }

    fn arrows(count: u32) -> ObjectFields {
        ObjectFields::from_pairs(&[(F_OBJECT_ENTRY, ARROWS), (F_ITEM_STACK_COUNT, count)])
    }

    /// The release system over a world with arrows in backpack slot 1, its baseline taken.
    fn arrow_world() -> (App, Entity) {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Items>()
            .init_resource::<ItemReleases>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, apply_item_releases);
        let mut items = app.world_mut().resource_mut::<Items>();
        items.insert_template(
            ARROWS,
            Some(benilla_protocol::messages::ItemInfo {
                stackable: 200,
                ..crate::items::test_template("Rough Arrow")
            }),
        );
        let me = app
            .world_mut()
            .spawn((crate::net::SelfPlayer, crate::net::ObjectStore(pack(&[A]))))
            .id();
        crate::items::test_spawn_item(app.world_mut(), A, arrows(200), false);
        let script = UiScript::new().unwrap();
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('ITEM_LOCK_CHANGED') \
                 f:SetScript('OnEvent', function() table.insert(SEEN, event) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.update();
        (app, me)
    }

    fn heard(app: &mut App) -> usize {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let n = script.eval::<usize>("return table.getn(SEEN)").unwrap();
        script.run("SEEN = {}").unwrap();
        n
    }

    fn restream(app: &mut App, guid: u64, fields: ObjectFields) {
        let e = app.world().resource::<crate::net::GuidIndex>().0[&guid];
        *app.world_mut()
            .get_mut::<crate::net::ObjectStore>(e)
            .unwrap() = crate::net::ObjectStore(fields);
    }

    /// The baseline is silent; a shot's stack-count write unlocks the arrows' entry with one
    /// `ITEM_LOCK_CHANGED` (`0x5d94af`, `0x5d94b9`), an auto-shot timer's only per-shot signal,
    /// and a frame with no change releases nothing.
    #[test]
    fn a_stack_count_write_unlocks_its_entry_once() {
        let (mut app, _) = arrow_world();
        assert_eq!(heard(&mut app), 0, "the VM's first look is the baseline");
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(A);
        let _ = heard(&mut app);
        restream(&mut app, A, arrows(199));
        app.update();
        assert_eq!(heard(&mut app), 1);
        assert!(!app.world().non_send_resource::<UiScript>().item_locked(A));
        app.update();
        assert_eq!(heard(&mut app), 0);
    }

    /// A move frees the seat it left: the item that left unlocks (`0x5ddd50`), and an arrival of
    /// a stacking entry unlocks its entry (`0x5d85ab`), one event each.
    #[test]
    fn a_move_and_an_arrival_each_unlock() {
        let (mut app, me) = arrow_world();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(A);
        let _ = heard(&mut app);
        *app.world_mut()
            .get_mut::<crate::net::ObjectStore>(me)
            .unwrap() = crate::net::ObjectStore(pack(&[0, A]));
        app.update();
        assert_eq!(heard(&mut app), 1, "slot 1 lost A");
        assert!(!app.world().non_send_resource::<UiScript>().item_locked(A));

        crate::items::test_spawn_item(app.world_mut(), B, arrows(20), false);
        *app.world_mut()
            .get_mut::<crate::net::ObjectStore>(me)
            .unwrap() = crate::net::ObjectStore(pack(&[B, A]));
        app.update();
        assert_eq!(heard(&mut app), 1, "B arrived: one entry unlock");
    }

    /// The loot window's owed unlocks run here, ahead of the feeds.
    #[test]
    fn the_loot_windows_owed_unlocks_run() {
        let (mut app, _) = arrow_world();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(A);
        let _ = heard(&mut app);
        let mut loot = crate::ui_loot::LootState::default();
        loot.unlock_item(A);
        app.insert_resource(loot);
        app.update();
        assert_eq!(heard(&mut app), 1);
        assert!(!app.world().non_send_resource::<UiScript>().item_locked(A));
    }
}
