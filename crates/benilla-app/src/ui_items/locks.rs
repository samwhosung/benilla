//! The server's half of the client's item lock. The lock is the reference's `item+0x314` bit 0,
//! held in the VM by guid ([`UiScript::lock_item`]): the cursor verbs set and lift it at their own
//! sites, and the app's sends lock their item. What lifts it from the server is the watchers'
//! runs on the item lane, at their packets' places ([`crate::item_watch::Unlock`]): a seat whose
//! guid changes unlocks the item now in it (`0x5ddd50`, `0x4f8e15`, `0x4f912c`), and an item of
//! ours set up or changing its stack count unlocks its entry or itself (`0x5d8590`, `0x5d85ab`,
//! `0x5d94af`, `0x5d94ca`); a sale's refusal's on the item lane too (`0x5dd26b`); an inventory
//! refusal's on its own lane ([`super::EquipErrors`]); a loot close's and the copier's ending's on
//! the loot lane ([`crate::ui_loot::close_interaction`]). The lock goes silently with its object
//! where the object is freed, on the item lane at the free's place ([`crate::item_watch::Freed`]);
//! what is left here is the lock's life across a `/reload`.

use benilla_ui::script::UiScript;
use bevy::prelude::*;

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
/// dying VM's, a world entry's none, as its item objects are built afresh.
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
    }
}
