//! `CGItem_C::Deactivate`'s head, which a release and a destroy run on every item and bag
//! (`0x464920` → `[vtable+0x4]` at `0x46496f`): what still names the item lets it go, inside the
//! object packet's handler. Its interface tail is the item lane's run ([`crate::item_watch`]).

use super::Scene;

/// `CGItem_C::Deactivate`'s head (`0x5d8660`, slot 1 of the item's vtable `0x80ada8` and the
/// bag's `0x80ac88`): what still names `item` lets it go, in its order, each signalling at the
/// packet's place ahead of the tail. The loot window looting it (`[0xb71b48]`, `0x5d867f`) closes
/// with its release (`0x48f200(1, 0, 0)` at `0x5d869b`); the pending record's cast from it
/// (`0x6e3d40` at `0x5d86ab`) is cancelled (`0x6e4940(1, 0, 0x1c)` at `0x5d86c4`); the cursor
/// holding it (`0x494c60` at `0x5d86d4`) lets it go (`0x495190(1, 1)` at `0x5d86e9`); the
/// reader on it (`[0xbc3fa0]`, `0x5d86f3`) closes (`0x4e32e0` of a zero guid at `0x5d8713`); and
/// a record not yet arrived (`+0x314` bit 1 clear, `0x5d8718`) drops this item's set-up callback
/// `0x5d8390` from its entry's pending record (`0x55c0c0` at `0x5d8733`), so the next object
/// under the guid sets up off its own request alone. The callback waits only until its record
/// lands, which is the bit, so the wait's presence stands for it. The owner's pending count it
/// lowers (`0x5d875d`) has no reader here.
pub(super) fn head(sc: &mut Scene, item: u64) {
    if let (Some(loot), Some(latch)) = (sc.loot.as_deref_mut(), sc.loot_latch.as_deref_mut()) {
        if loot.source() == Some(item) {
            crate::ui_loot::end_loot_window_on_its_item(loot, latch, &sc.net);
        }
    }
    let player = sc.self_guid.0.and_then(|me| sc.index.0.get(&me).copied());
    sc.item_cast.run(item, &sc.net, player);
    sc.item_lane
        .push(crate::item_watch::Run::clearing_cursor(item));
    sc.item_text.run(item);
    let entry = sc
        .index
        .0
        .get(&item)
        .and_then(|&e| sc.stores.get(e).ok())
        .and_then(|s| s.0.object_entry());
    if let Some(entry) = entry {
        sc.items
            .cancel_wait(entry, crate::items::RecordCallback::SetUp(item));
    }
}
