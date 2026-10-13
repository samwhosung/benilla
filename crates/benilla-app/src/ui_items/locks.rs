//! The server's half of the client's item lock. The lock is the reference's `item+0x314` bit 0,
//! held in the VM by guid ([`UiScript::lock_item`]): the cursor verbs set and lift it at their own
//! sites, and the app's sends lock their item. What lifts it from here is the server: a slot whose
//! guid changes unlocks the item that left it (`0x5ddd50` and `0x4f8e15` over the player's array,
//! the latter its backpack and keyring, `0x4f912c` over a container's slots), an item of ours
//! created or changing its stack count unlocks its entry (the walks `0x5d8590`, `0x5d94af`) or,
//! when it does not stack, itself (`0x5d85ab`, `0x5d94ca`), and the handlers without a VM queue
//! theirs in [`ItemReleases`]: a sale, a loot release.

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

/// The VM's locks between a VM and the next: the reference's bit lives on the item object
/// (`item+0x314`), which a `/reload` keeps, while ours lives in the VM it rebuilds.
/// [`fold_dying_vm_item_locks`] takes them out of the dying VM and [`seed_vm_item_locks`] seats
/// them in the new one; a world entry builds its item objects afresh and starts with none.
#[derive(Resource, Default, Debug)]
pub(crate) struct HeldItemLocks {
    pub(crate) locks: Vec<u64>,
    /// The cursor, which is engine state too: no clear on the reload's path touches an item, a
    /// spell or coins on it.
    pub(crate) cursor: benilla_ui::script::HeldCursor,
}

/// Take the dying VM's locks and cursor into [`HeldItemLocks`], as the reload's leave-world
/// sweep (`0x490c20` → `0x490a80`) leaves them. The sweep has run on this VM by now, and its
/// legs made their own lock and cursor changes in it: the loot leg's two unlocks of an item
/// source, the mail leg's attachment unlock, the auction leg's sell-slot unlock and the merchant
/// leg's held-row clear. The trade leg only cancels, and the cascade then zeroes the trade tables
/// with no `UnlockItem` (`0x4909b5` → `0x4bf0e0`), so the answer's `UnlockTradeItems` finds
/// nothing: an offered item stays locked until its seat or stack moves, the revive's unlock-all or
/// a relog, as in the reference.
///
/// Nothing else on the path writes the cursor (its setters and `ClearCursor` are the only writers
/// of `[0xb4d900]` and `[0xb4e248]`), so a held item stays held, with its lock, and an armed wrap
/// stays armed.
pub(crate) fn fold_dying_vm_item_locks(world: &mut World) {
    let Some(held) = world
        .get_non_send_resource::<UiScript>()
        .map(|s| HeldItemLocks {
            locks: s.item_locks(),
            cursor: s.held_cursor(),
        })
    else {
        return;
    };
    world.insert_resource(held);
}

/// Seat the new VM's locks and cursor before any file or event runs on it: a `/reload`'s are the
/// dying VM's, a world entry's none. A world entry's watch starts over ([`LockWatch`]); a
/// `/reload`'s carries on.
pub(crate) fn seed_vm_item_locks(world: &mut World, script: &mut UiScript, reload: bool) {
    let held = world
        .get_resource_mut::<HeldItemLocks>()
        .map(|mut h| std::mem::take(&mut *h))
        .unwrap_or_default();
    if reload {
        script.seed_item_locks(held.locks);
        script.seed_held_cursor(held.cursor);
    } else {
        script.seed_item_locks(Vec::new());
        if let Some(mut watch) = world.get_resource_mut::<LockWatch>() {
            *watch = LockWatch::default();
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

/// The player's seats as the watchers last saw them, and the stack count of every item of ours.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Snapshot {
    seats: HashMap<Seat, u64>,
    counts: HashMap<u64, u32>,
}

/// The watched part of the player's array, in the reference's numbering: equipment and bags
/// (0-22), the bank and its bags (39-68) and the keyring (81-112) under `0x5ddcf0` (installed by
/// `0x5dd8a0`), the backpack (23-38) and the keyring again under `0x4f8db0` (installed by
/// `0x4f8cd0`). Buyback (69-80) has no watcher.
fn watched_array(store: &ObjectFields) -> impl Iterator<Item = (u8, u64)> + '_ {
    let inv = (0..23u8).map(|i| (i, store.player_inv_slot(i)));
    let pack = (0..16u8).map(|i| (23 + i, store.player_pack_slot(i)));
    let bank = (0..24u8).map(|i| (39 + i, store.player_bank_slot(i)));
    let bank_bags = (0..6u8).map(|i| (63 + i, store.player_bank_bag_slot(i)));
    let keyring = (0..32u8).map(|i| (81 + i, store.player_keyring_slot(i)));
    inv.chain(pack)
        .chain(bank)
        .chain(bank_bags)
        .chain(keyring)
        .map(|(i, guid)| (i, guid.unwrap_or(0)))
}

/// The keyring, which both watchers cover, so a seat change there unlocks twice.
const KEYRING: std::ops::RangeInclusive<u8> = 81..=112;

/// Read the watchers' view: the seats, and the stack count of each item object we own
/// (`0x5d9400`'s owner test, `0x5d9449`).
pub(crate) fn snapshot(store: &ObjectFields, objects: &Objects, me: u64) -> Snapshot {
    let mut seats: HashMap<Seat, u64> = watched_array(store)
        .map(|(i, guid)| (Seat::Player(i), guid))
        .collect();
    // A container's slots under `0x4f90d0`, for the equipped and the bank bags.
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
    let counts = objects
        .iter()
        .filter(|(_, _, f)| f.is_item() && f.item_owner() == Some(me))
        .map(|(guid, _, f)| (guid, f.item_stack_count().unwrap_or(1)))
        .collect();
    Snapshot { seats, counts }
}

/// The scopes of the inventory walk `0x622420` the entry unlocks take (`0x6221e0`): the arrival's
/// flags 0, which the walk widens to equipment, bags, backpack and keyring (`0x622439`), and the
/// stack-count write's 8, which adds the bank and its bags. Neither reaches buyback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Walk {
    Arrival,
    Stack,
}

/// The items of `entry` the walk reaches, sorted. A bag reached is walked inside (`0x62256d`). A
/// broken item, durability 0 of a nonzero maximum on an unwrapped item, and one flagged `0x10`
/// are passed over (`0x6224f7`-`0x622513`).
pub(crate) fn kin(store: &ObjectFields, objects: &Objects, walk: Walk, entry: u32) -> Vec<u64> {
    let in_scope = |i: u8| match i {
        0..=38 | 81..=112 => true,
        39..=68 => walk == Walk::Stack,
        _ => false,
    };
    let passed_over = |f: &ObjectFields| {
        let flags = f.item_flags().unwrap_or(0);
        let broken = flags & 0x8 == 0
            && f.item_max_durability().unwrap_or(0) > 0
            && f.item_durability().unwrap_or(0) == 0;
        broken || flags & 0x10 != 0
    };
    let mut out = Vec::new();
    fn walk_item(
        guid: u64,
        objects: &Objects,
        entry: u32,
        passed_over: &dyn Fn(&ObjectFields) -> bool,
        out: &mut Vec<u64>,
    ) {
        let Some(f) = objects.object(guid).filter(|f| !passed_over(f)) else {
            return;
        };
        if f.object_entry() == Some(entry) {
            out.push(guid);
        }
        let n = f.container_num_slots().unwrap_or(0).min(36) as u8;
        for j in 0..n {
            if let Some(inner) = f.container_slot(j).filter(|&g| g != 0) {
                walk_item(inner, objects, entry, passed_over, out);
            }
        }
    }
    let array: Vec<(u8, u64)> = (0..23u8)
        .map(|i| (i, store.player_inv_slot(i)))
        .chain((0..16u8).map(|i| (23 + i, store.player_pack_slot(i))))
        .chain((0..24u8).map(|i| (39 + i, store.player_bank_slot(i))))
        .chain((0..6u8).map(|i| (63 + i, store.player_bank_bag_slot(i))))
        .chain((0..32u8).map(|i| (81 + i, store.player_keyring_slot(i))))
        .filter_map(|(i, g)| Some((i, g.filter(|&g| g != 0)?)))
        .collect();
    for (i, guid) in array {
        if in_scope(i) {
            walk_item(guid, objects, entry, &passed_over, &mut out);
        }
    }
    out.sort_unstable();
    out
}

/// The seat watchers' unlocks between two looks: a seat whose guid changed unlocks the item that
/// left it (`0x5ddd50`, `0x4f8e15`, `0x4f912c`), twice on the keyring, which both array watchers
/// cover; a bag's seats go with it unseen. Sorted, so the event stream is reproducible.
pub(crate) fn seat_unlocks(was: &Snapshot, now: &Snapshot) -> Vec<u64> {
    let mut left: Vec<(Seat, u64)> = was
        .seats
        .iter()
        .filter(|&(_, &old)| old != 0)
        .filter(|&(seat, &old)| match now.seats.get(seat) {
            Some(&new) => new != old,
            None => matches!(seat, Seat::Player(_)),
        })
        .map(|(&seat, &old)| (seat, old))
        .collect();
    left.sort_unstable_by_key(|&(_, g)| g);
    left.into_iter()
        .flat_map(|(seat, old)| {
            let twice = matches!(seat, Seat::Player(i) if KEYRING.contains(&i));
            std::iter::repeat_n(old, if twice { 2 } else { 1 })
        })
        .collect()
}

/// The items of ours whose stack count changed between two looks (`0x5d9400`), sorted; one that
/// appeared is a create, the arrival watcher's.
pub(crate) fn restacked(was: &Snapshot, now: &Snapshot) -> Vec<u64> {
    let mut out: Vec<u64> = now
        .counts
        .iter()
        .filter(|&(g, &c)| was.counts.get(g).is_some_and(|&w| w != c))
        .map(|(&g, _)| g)
        .collect();
    out.sort_unstable();
    out
}

/// The watchers' last look. The arrival unlock reads the world latch (`[0xb4b424]` at `0x5d84ff`),
/// set in the world-entry cascade (`0x4908ce`) and cleared in the world sweep (`0x490a8d`), so a
/// world entry's interface takes a baseline and releases nothing for what it finds; a `/reload`
/// clears and sets the latch inside one call, so the look outlives the VM it replaces, and a
/// create or a restack the reload's frame brings is compared as any other. `arrivals` are item
/// objects created since, waiting on their template as the reference's create waits for its
/// answer (`0x5d8346` → `0x5d8390`).
#[derive(Default, Resource)]
pub(crate) struct LockWatch {
    seen: Option<Snapshot>,
    templates: u64,
    arrivals: Vec<u64>,
    /// The entity behind each locked item, as last seen: the lock is the object's, so one that
    /// is despawned or rebuilt under the same guid drops it.
    lock_owners: HashMap<u64, Entity>,
}

/// Run the frame's releases ahead of the inventory feeds: the watchers compare the incoming guid
/// with the one still in the descriptor (`0x5ddd42`), so a handler reads the slot as it was.
#[allow(clippy::too_many_arguments)] // the input set
pub(crate) fn apply_item_releases(
    script: Option<NonSendMut<UiScript>>,
    mut inv: crate::items::Inventory,
    created: Query<&crate::net::Guid, Added<crate::items::ItemObject>>,
    me: Res<crate::net::SelfGuid>,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut queued: ResMut<ItemReleases>,
    // The loot window's closes and refusals owe theirs on the session.
    loot: Option<ResMut<crate::ui_loot::LootState>>,
    mut watch: ResMut<LockWatch>,
) {
    // Read before taking, so a frame owing nothing leaves the session unchanged for its feed.
    if let Some(mut loot) = loot.filter(|l| l.owes_unlocks()) {
        queued.0.extend(
            loot.take_item_unlocks()
                .into_iter()
                .filter_map(|(guid, live)| {
                    let resolves = !live || inv.objects.object(guid).is_some_and(|f| f.is_item());
                    resolves.then_some(Release::Item(guid))
                }),
        );
    }
    let Some(mut script) = script else {
        queued.0.clear();
        return;
    };
    let watch = &mut *watch;
    let (Some(store), Some(me)) = (inv.self_store.iter().next(), me.0) else {
        *watch = LockWatch::default();
        queued.apply(&mut script);
        return;
    };
    // The arrival unlock fires from the item's create (`0x5d8300`), never from a seat a moved bag
    // brings into view; the VM's first look owns what already exists.
    if watch.seen.is_some() {
        watch.arrivals.extend(created.iter().map(|g| g.0));
    }
    let moved = inv.changes.moved() || !inv.self_changed.is_empty();
    // A template answer moves no item object, so the frame it lands has its own trigger.
    let templates = items.template_epoch();
    let answered = watch.templates != templates;
    watch.templates = templates;
    let mut out = Vec::new();
    // Whether an entry stacks past 1 (`[rec+0x60] > 1`, `0x5d857b`, `0x5d947e`); `None` while the
    // template is unanswered.
    let stackable = |entry: u32, guid: u64| {
        items
            .template(entry, guid, &commands)
            .map(|t| t.stackable > 1)
    };
    let objects = &inv.objects;
    let release = |guid: u64, entry: u32, walk: Walk, stacks: bool| {
        if stacks {
            Release::Entry(kin(&store.0, objects, walk, entry))
        } else {
            Release::Item(guid)
        }
    };
    if moved || watch.seen.is_none() {
        let now = snapshot(&store.0, objects, me);
        if let Some(was) = watch.seen.replace(now) {
            let now = watch.seen.as_ref().expect("just stored");
            out.extend(seat_unlocks(&was, now).into_iter().map(Release::Item));
            for guid in restacked(&was, now) {
                let entry = objects.object(guid).and_then(ObjectFields::object_entry);
                // An unanswered template unlocks the item alone (`0x5d947c`).
                let stacks = entry.and_then(|e| stackable(e, guid)).unwrap_or(false);
                out.push(release(guid, entry.unwrap_or(0), Walk::Stack, stacks));
            }
        }
    }
    // A locked item whose object went away, or came back as a new one (a cross-map worldport
    // rebuilds them all), loses the lock silently: it lived on the old object, and the new one
    // is built unlocked (`0x5d821f`, `0x5d823e`).
    let mut owners = HashMap::new();
    for guid in script.item_locks() {
        let now = objects.entity(guid);
        let was = watch.lock_owners.get(&guid).copied();
        match now {
            Some(e) if was.is_none_or(|w| w == e) => {
                owners.insert(guid, e);
            }
            _ => script.forget_item_lock(guid),
        }
    }
    watch.lock_owners = owners;
    if moved || answered {
        let mut waiting = Vec::new();
        for guid in std::mem::take(&mut watch.arrivals) {
            // Ours only (`0x5d851f`); a vanished object owes nothing.
            let Some(f) = objects.object(guid).filter(|f| f.item_owner() == Some(me)) else {
                continue;
            };
            let Some(entry) = f.object_entry() else {
                waiting.push(guid);
                continue;
            };
            match stackable(entry, guid) {
                Some(stacks) => out.push(release(guid, entry, Walk::Arrival, stacks)),
                None => waiting.push(guid),
            }
        }
        watch.arrivals = waiting;
    }
    queued.0.splice(0..0, out);
    queued.apply(&mut script);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(seats: &[(Seat, u64)], counts: &[(u64, u32)]) -> Snapshot {
        Snapshot {
            seats: seats.iter().copied().collect(),
            counts: counts.iter().copied().collect(),
        }
    }

    const A: u64 = 0x4000_0000_0000_00a1;
    const B: u64 = 0x4000_0000_0000_00b2;
    const C: u64 = 0x4000_0000_0000_00c3;
    const BAG: u64 = 0x4000_0000_0000_0b06;
    const ME: u64 = 0x2a;

    #[test]
    fn a_seat_that_changes_guid_unlocks_the_item_that_left() {
        let was = snap(&[(Seat::Player(23), A), (Seat::Player(24), 0)], &[]);
        let now = snap(&[(Seat::Player(23), 0), (Seat::Player(24), A)], &[]);
        assert_eq!(
            seat_unlocks(&was, &now),
            [A],
            "the move: only the vacated seat's old guid, the filled seat had none"
        );
        let swapped = snap(&[(Seat::Player(23), B), (Seat::Player(24), A)], &[]);
        let was = snap(&[(Seat::Player(23), A), (Seat::Player(24), B)], &[]);
        assert_eq!(seat_unlocks(&was, &swapped), [A, B], "a swap unlocks both");
        // The keyring sits under both array watchers (`0x5ddcf0`, `0x4f8db0`).
        let was = snap(&[(Seat::Player(81), A)], &[]);
        let now = snap(&[(Seat::Player(81), 0)], &[]);
        assert_eq!(seat_unlocks(&was, &now), [A, A], "a key unlocks twice");
    }

    #[test]
    fn a_bag_that_leaves_takes_its_seats_silently() {
        let was = snap(&[(Seat::Player(19), BAG), (Seat::Bag(BAG, 0), A)], &[]);
        let now = snap(&[(Seat::Player(19), 0)], &[]);
        assert_eq!(seat_unlocks(&was, &now), [BAG]);
    }

    #[test]
    fn a_restack_is_a_count_change_of_an_item_seen_before() {
        let was = snap(&[], &[(A, 5)]);
        let now = snap(&[], &[(A, 3), (B, 2)]);
        assert_eq!(
            restacked(&was, &now),
            [A],
            "B appeared: a create, not a restack"
        );
        assert!(restacked(&now, &now).is_empty());
    }

    /// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid pair.
    const F_PACK_SLOT_1: u16 = 532;
    /// `PLAYER_FIELD_INV_SLOT_HEAD`, slot 0's guid pair; slot 19 is the first bag.
    const F_INV_SLOT_1: u16 = benilla_protocol::field::FIELD_PLAYER_INV_SLOT_HEAD;
    /// `PLAYER_FIELD_BANK_SLOT_1`.
    const F_BANK_SLOT_1: u16 = 564;
    const F_OBJECT_TYPE: u16 = 2;
    const F_OBJECT_ENTRY: u16 = 3;
    const F_ITEM_OWNER: u16 = 6;
    const F_ITEM_STACK_COUNT: u16 = 14;
    const F_ITEM_DURABILITY: u16 = 46;
    const F_ITEM_MAX_DURABILITY: u16 = 47;
    const F_CONTAINER_NUM_SLOTS: u16 = 48;
    const F_CONTAINER_SLOT_1: u16 = 50;
    const ARROWS: u32 = 2512;

    fn guid_pairs(first: u16, slots: &[u64]) -> Vec<(u16, u32)> {
        slots
            .iter()
            .enumerate()
            .flat_map(|(i, &g)| {
                let f = first + 2 * i as u16;
                [(f, g as u32), (f + 1, (g >> 32) as u32)]
            })
            .collect()
    }

    fn pack(slots: &[u64]) -> ObjectFields {
        ObjectFields::from_pairs(&guid_pairs(F_PACK_SLOT_1, slots))
    }

    /// An item of ours: type item, owner the player.
    fn ours(extra: &[(u16, u32)]) -> ObjectFields {
        let mut pairs = vec![
            (F_OBJECT_TYPE, 0x3),
            (F_ITEM_OWNER, ME as u32),
            (F_ITEM_OWNER + 1, 0),
        ];
        pairs.extend_from_slice(extra);
        ObjectFields::from_pairs(&pairs)
    }

    fn arrows(count: u32) -> ObjectFields {
        ours(&[(F_OBJECT_ENTRY, ARROWS), (F_ITEM_STACK_COUNT, count)])
    }

    /// The release system over a world with arrows in backpack slot 1, its baseline taken.
    fn arrow_world() -> (App, Entity) {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Items>()
            .init_resource::<ItemReleases>()
            .init_resource::<LockWatch>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(crate::net::SelfGuid(Some(ME)))
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

    fn set_store(app: &mut App, me: Entity, fields: ObjectFields) {
        *app.world_mut()
            .get_mut::<crate::net::ObjectStore>(me)
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

    /// Swap in a fresh VM the way a `/reload` does: the dying VM's locks folded, the new one
    /// seeded with them before anything runs on it.
    fn reload_vm(app: &mut App) {
        let world = app.world_mut();
        fold_dying_vm_item_locks(world);
        let mut script = UiScript::new().unwrap();
        seed_vm_item_locks(world, &mut script, true);
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('ITEM_LOCK_CHANGED') \
                 f:SetScript('OnEvent', function() table.insert(SEEN, event) end)",
            )
            .unwrap();
        world.insert_non_send_resource(script);
    }

    /// A `/reload` keeps the world latch set (`[0xb4b424]`, cleared at `0x490a8d` and set again
    /// at `0x4908ce` inside the one call), so a restack and a create landing on the reload's
    /// frame unlock into the new VM, not into its baseline; a world entry's VM starts over.
    #[test]
    fn a_reload_frame_swallows_no_restack_or_create() {
        let (mut app, me) = arrow_world();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(A);
        reload_vm(&mut app);
        restream(&mut app, A, arrows(199));
        crate::items::test_spawn_item(app.world_mut(), B, arrows(20), false);
        set_store(&mut app, me, pack(&[A, B]));
        app.update();
        assert_eq!(
            heard(&mut app),
            2,
            "the restack's and the create's entry unlocks"
        );
        assert!(!app.world().non_send_resource::<UiScript>().item_locked(A));

        // A world entry's seed: the new VM's first look is a baseline again.
        let mut fresh = UiScript::new().unwrap();
        seed_vm_item_locks(app.world_mut(), &mut fresh, false);
        fresh
            .run(
                "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('ITEM_LOCK_CHANGED') \
                 f:SetScript('OnEvent', function() table.insert(SEEN, event) end)",
            )
            .unwrap();
        app.world_mut().insert_non_send_resource(fresh);
        restream(&mut app, A, arrows(150));
        app.update();
        assert_eq!(heard(&mut app), 0, "the world entry's baseline");
    }

    /// A move frees the seat it left (`0x5ddd50`); a created item of a stacking entry unlocks
    /// its entry (`0x5d8590`), one event each.
    #[test]
    fn a_move_and_a_create_each_unlock() {
        let (mut app, me) = arrow_world();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(A);
        let _ = heard(&mut app);
        set_store(&mut app, me, pack(&[0, A]));
        app.update();
        assert_eq!(heard(&mut app), 1, "slot 1 lost A");
        assert!(!app.world().non_send_resource::<UiScript>().item_locked(A));

        crate::items::test_spawn_item(app.world_mut(), B, arrows(20), false);
        set_store(&mut app, me, pack(&[B, A]));
        app.update();
        assert_eq!(heard(&mut app), 1, "B created: one entry unlock");
    }

    /// Buyback has no watcher (`0x5dd8a0` skips 69-80): a sale's item reaching it, and leaving it,
    /// unlocks nothing there; the backpack seat it left does.
    #[test]
    fn buyback_seats_are_not_watched() {
        const F_BUYBACK_1: u16 = 624;
        let (mut app, me) = arrow_world();
        let mut pairs = guid_pairs(F_PACK_SLOT_1, &[0]);
        pairs.extend(guid_pairs(F_BUYBACK_1, &[A]));
        set_store(&mut app, me, ObjectFields::from_pairs(&pairs));
        app.update();
        assert_eq!(heard(&mut app), 1, "the backpack seat alone");
        let mut pairs = guid_pairs(F_PACK_SLOT_1, &[0]);
        pairs.extend(guid_pairs(F_BUYBACK_1, &[0]));
        set_store(&mut app, me, ObjectFields::from_pairs(&pairs));
        app.update();
        assert_eq!(heard(&mut app), 0, "the buyback seat emptying");
    }

    /// A bag equipped from the backpack brings its seats into view, and nothing in it arrives:
    /// the arrival unlock is the item's create (`0x5d8300`), not a seat appearing.
    #[test]
    fn a_bag_moved_into_view_brings_no_arrival() {
        let (mut app, me) = arrow_world();
        crate::items::test_spawn_item(
            app.world_mut(),
            BAG,
            ours(
                &[(F_CONTAINER_NUM_SLOTS, 4)]
                    .into_iter()
                    .chain(guid_pairs(F_CONTAINER_SLOT_1, &[C]))
                    .collect::<Vec<_>>(),
            ),
            true,
        );
        crate::items::test_spawn_item(app.world_mut(), C, arrows(5), false);
        set_store(&mut app, me, pack(&[A, BAG]));
        app.update();
        let _ = heard(&mut app); // the two creates' own unlocks
        let mut pairs = guid_pairs(F_PACK_SLOT_1, &[A, 0]);
        pairs.extend(guid_pairs(F_INV_SLOT_1 + 2 * 19, &[BAG]));
        set_store(&mut app, me, ObjectFields::from_pairs(&pairs));
        app.update();
        assert_eq!(heard(&mut app), 1, "the backpack seat's unlock alone");
    }

    /// The arrival's walk skips the bank and the stack-count write's reaches it (`0x622420`
    /// flags 0 and 8); both pass over a broken item and one flagged `0x10`.
    #[test]
    fn the_entry_walks_scope_and_skip_as_the_reference_does() {
        use bevy::ecs::system::RunSystemOnce;
        const BROKEN: u64 = 0x4000_0000_0000_0d01;
        const FLAGGED: u64 = 0x4000_0000_0000_0d02;
        let (mut app, me) = arrow_world();
        for (guid, extra) in [
            (B, vec![]),
            (C, vec![]),
            (
                BROKEN,
                vec![(F_ITEM_MAX_DURABILITY, 50), (F_ITEM_DURABILITY, 0)],
            ),
            (FLAGGED, vec![(21, 0x10)]),
        ] {
            let mut f = vec![(F_OBJECT_ENTRY, ARROWS), (F_ITEM_STACK_COUNT, 1)];
            f.extend(extra);
            crate::items::test_spawn_item(app.world_mut(), guid, ours(&f), false);
        }
        let mut pairs = guid_pairs(F_PACK_SLOT_1, &[A, BROKEN, FLAGGED]);
        pairs.extend(guid_pairs(F_BANK_SLOT_1, &[B]));
        pairs.extend(guid_pairs(F_INV_SLOT_1, &[C]));
        set_store(&mut app, me, ObjectFields::from_pairs(&pairs));
        let walks = app
            .world_mut()
            .run_system_once(
                |q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
                 objects: Objects| {
                    let store = &q.single().unwrap().0;
                    (
                        kin(store, &objects, Walk::Arrival, ARROWS),
                        kin(store, &objects, Walk::Stack, ARROWS),
                    )
                },
            )
            .unwrap();
        assert_eq!(walks.0, [A, C], "no bank, nothing broken or flagged");
        assert_eq!(walks.1, [A, B, C], "the bank too");
    }

    /// A closed window's source unlocks only while it resolves as a live item object
    /// (`0x48f299`/`0x48f2ab`): a fully looted box, already destroyed, owes nothing.
    #[test]
    fn a_window_sources_unlock_needs_a_live_item() {
        const GONE: u64 = 0x4000_0000_0000_0dea;
        let (mut app, _) = arrow_world();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(A);
        let _ = heard(&mut app);
        let mut loot = crate::ui_loot::LootState::default();
        loot.unlock_item_source(GONE);
        loot.unlock_item_source(A);
        app.insert_resource(loot);
        app.update();
        assert_eq!(heard(&mut app), 1, "A's alone");
        assert!(!app.world().non_send_resource::<UiScript>().item_locked(A));
    }

    /// The lock is the item object's: one rebuilt under the same guid (a cross-map worldport)
    /// comes back unlocked (`0x5d821f`, `0x5d823e`), and one despawned takes its lock with it,
    /// with no event either way.
    #[test]
    fn a_rebuilt_or_despawned_item_loses_its_lock_silently() {
        let (mut app, _) = arrow_world();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(A);
        app.update();
        let _ = heard(&mut app);
        let old = app.world().resource::<crate::net::GuidIndex>().0[&A];
        app.world_mut().despawn(old);
        crate::items::test_spawn_item(app.world_mut(), A, arrows(200), false);
        app.update();
        assert!(!app.world().non_send_resource::<UiScript>().item_locked(A));
        let _ = heard(&mut app); // the new object's own arrival unlock (`0x5d8590`)

        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(A);
        app.update();
        let _ = heard(&mut app);
        let e = app.world().resource::<crate::net::GuidIndex>().0[&A];
        app.world_mut().despawn(e);
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .remove(&A);
        app.update();
        assert!(!app.world().non_send_resource::<UiScript>().item_locked(A));
        assert_eq!(heard(&mut app), 0, "no event for the drop itself");
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
