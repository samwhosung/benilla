//! The server's half of the client's item lock. The lock is the reference's `item+0x314` bit 0,
//! held in the VM by guid ([`UiScript::lock_item`]): the cursor verbs set and lift it at their own
//! sites, and the app's sends lock their item. What lifts it from the server is the watchers'
//! runs on the item lane, at their packets' places ([`crate::item_watch::Unlock`]): a seat whose
//! guid changes unlocks the item now in it (`0x5ddd50`, `0x4f8e15`, `0x4f912c`), and an item of
//! ours set up or changing its stack count unlocks its entry or itself (`0x5d8590`, `0x5d85ab`,
//! `0x5d94af`, `0x5d94ca`); a sale's refusal's on the item lane too (`0x5dd26b`); an inventory
//! refusal's on its own lane ([`super::EquipErrors`]). What is left here is the loot window's
//! release the loot lane did not run, and the locks whose objects went ([`apply_item_releases`]).

use std::collections::HashMap;

use benilla_ui::script::UiScript;
use bevy::prelude::*;

use crate::net::Objects;

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

/// The entity behind each locked item, as last seen: the lock is the object's, so one that is
/// despawned or rebuilt under the same guid drops it.
#[derive(Default, Resource)]
pub(crate) struct LockWatch {
    lock_owners: HashMap<u64, Entity>,
}

/// Run the loot window's owed unlocks, each `UnlockItem` (`0x495420`) whose `ITEM_LOCK_CHANGED`
/// fires whether or not the guid names an item we hold (`0x495450`), and forget the locks whose
/// objects went: a locked item whose object went away, or came back as a new one (a cross-map
/// worldport rebuilds them all), loses the lock silently, as it lived on the old object and the
/// new one is built unlocked (`0x5d821f`, `0x5d823e`).
pub(crate) fn apply_item_releases(
    mut vm: crate::script_calls::VmWith<Option<ResMut<'static, crate::ui_loot::LootState>>>,
    objects: Objects,
    mut watch: ResMut<LockWatch>,
) {
    vm.turn(|script, loot| {
        // Read before taking, so a frame owing nothing leaves the session unchanged for its feed.
        let owed: Vec<u64> = match loot.filter(|l| l.owes_unlocks()) {
            Some(mut loot) => loot
                .take_item_unlocks()
                .into_iter()
                .filter(|&(guid, live)| !live || objects.object(guid).is_some_and(|f| f.is_item()))
                .map(|(guid, _)| guid)
                .collect(),
            None => Vec::new(),
        };
        let Some(script) = script else {
            return;
        };
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
        for guid in owed {
            script.unlock_item(guid);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::ObjectFields;

    const A: u64 = 0x4000_0000_0000_00a1;
    const ME: u64 = 0x2a;
    const F_OBJECT_TYPE: u16 = 2;
    const F_OBJECT_ENTRY: u16 = 3;
    const F_ITEM_OWNER: u16 = 6;
    const F_ITEM_STACK_COUNT: u16 = 14;
    const ARROWS: u32 = 2512;

    fn arrows(count: u32) -> ObjectFields {
        ObjectFields::from_pairs(&[
            (F_OBJECT_TYPE, 0x3),
            (F_ITEM_OWNER, ME as u32),
            (F_ITEM_OWNER + 1, 0),
            (F_OBJECT_ENTRY, ARROWS),
            (F_ITEM_STACK_COUNT, count),
        ])
    }

    /// The release system over a world with the arrows as an item object, and a VM that counts
    /// `ITEM_LOCK_CHANGED`.
    fn arrow_world() -> App {
        let mut app = App::new();
        app.init_resource::<crate::items::Items>()
            .init_resource::<LockWatch>()
            .init_resource::<crate::net::GuidIndex>()
            .add_systems(Update, apply_item_releases);
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
        app
    }

    fn heard(app: &mut App) -> usize {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let n = script.eval::<usize>("return table.getn(SEEN)").unwrap();
        script.run("SEEN = {}").unwrap();
        n
    }

    /// A closed window's source unlocks only while it resolves as a live item object
    /// (`0x48f299`/`0x48f2ab`): a fully looted box, already destroyed, owes nothing.
    #[test]
    fn a_window_sources_unlock_needs_a_live_item() {
        const GONE: u64 = 0x4000_0000_0000_0dea;
        let mut app = arrow_world();
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
        let mut app = arrow_world();
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
        assert_eq!(heard(&mut app), 0, "no event for the rebuild");

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
        let mut app = arrow_world();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(A);
        let _ = heard(&mut app);
        let mut loot = crate::ui_loot::LootState::default();
        loot.unlock_item_source(A);
        app.insert_resource(loot);
        app.update();
        assert_eq!(heard(&mut app), 1);
        assert!(!app.world().non_send_resource::<UiScript>().item_locked(A));
    }
}
