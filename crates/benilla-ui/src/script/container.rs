//! The container bindings (decision 0068 T2) — the 1.12 bag verbs, every one of them a plain
//! top-level global (measured on the corpus: `GetContainerItemLink`,
//! `GetContainerNumSlots` and `GetContainerItemInfo` are the three most-wanted engine verbs of
//! all, and Bagnon's data engine is built on exactly these). Same two-way seam as
//! [`super::action`]: the app pushes a **container snapshot** per bag id
//! ([`UiScript::set_container`] — slots already resolved to icon/count/quality by the app's
//! item stores; the engine holds no item knowledge), and `UseContainerItem` queues an outbound
//! **intent** the app drains into the wire ([`UiScript::take_container_uses`]).
//!
//! Bag ids are the live API's space: `0` the backpack, `1..4` the equipped bags (bank later).
//! Slots are 1-based. One deliberate divergence from the 1.14 documentation:
//! `containerInfo.iconFileID` carries our icon **texture path** (`Interface\Icons\…`), not a
//! numeric FileDataID — 5875 has no FileDataIDs, and every measured consumer feeds the value
//! straight to `SetTexture`, which takes both shapes in the live client.

use mlua::{Lua, Table, Value};

use super::cursor::{self, CursorItem, CursorPayload};
use super::Model;

/// The charter/petition lines an item tooltip prints under its name — the item tooltip's own
/// emission order, line 3 (`0x854d7c..0x854dd0`): *"(petition/guild-charter items) Title /
/// Creator / Num-signatures (white; title wraps) — a resolved petition object; keys
/// `PETITION_*`/`GUILD_CHARTER_*`"*.
///
/// The two key families are picked by [`Self::is_charter`], the same record bit that picks
/// `GetPetitionInfo`'s first return — so a charter reads *"Guild Name:"* / *"Guild Master:"* and a
/// plain petition reads *"Petition:"* / *"Created by"*.
///
/// **The third line, the signature count, is deliberately NOT here.** The law names it, but neither
/// the reference nor the packet says where the number comes from: the petition record carries no
/// count, and the only candidate — the item's `ITEM_FIELD_ENCHANTMENT` slot-0 *charges* dword,
/// which vmangos's own source documents as "the on-item signature count" — has its write
/// **commented out** server-side, so it is always zero and the guess could never be falsified here.
/// Omitted rather than invented; the line is one field away the day someone pins the source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PetitionSlotView {
    /// The record's charter bit: `true` picks the `GUILD_CHARTER_*` keys, `false` the `PETITION_*`
    /// ones.
    pub is_charter: bool,
    /// The proposed guild's (or petition's) name.
    pub title: String,
    /// The owner's name, app-resolved through the name cache. `None` while that query is in
    /// flight — the line is withheld, as the creator line is, rather than printed with a hole.
    pub owner: Option<String>,
}

/// One occupied bag slot, resolved by the app (icon from ItemDisplayInfo, count/quality from the
/// item object + template). Plain data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainerSlot {
    /// Icon texture path (`Interface\Icons\…`); `None` while the template answer is in flight.
    pub texture: Option<String>,
    pub count: u32,
    /// Item quality 0..6; `None` while unresolved (the API reports `nil`).
    pub quality: Option<u32>,
    /// The item's template entry (`containerInfo.itemID`); 0 while unresolved.
    pub item_id: u32,
    /// An `|Hitem:…|h[Name]|h` link once the name is known; Bagnon's search reads it.
    pub link: Option<String>,
    pub locked: bool,
    /// The 1-based live-API inventory slot ids this item could be EQUIPPED into (empty = not
    /// equippable) — the app-resolved "fit rule" decision 0208 phase 1b's cursor arc needs
    /// (`cursor::CursorItem::equip_slots` captures it at pickup). Derived from the item
    /// template's `inventoryType` via `ui_items::find_equip_slot`; the engine holds no item
    /// knowledge of its own.
    pub equip_slots: Vec<u8>,
    /// Whether this item may be placed on an ACTION-BAR slot — app-resolved from the template
    /// exactly like [`Self::equip_slots`] (the engine holds no item knowledge of its own).
    /// `PlaceAction`'s only item filter, byte-read: an on-use spell OR equippable
    /// (`ItemInfo::placeable_on_action_bar`, `PlaceAction 0x4e62e0`).
    pub bar_placeable: bool,
    /// The instance's live durability `(current, max)` — `ITEM_FIELD_DURABILITY`/`MAXDURABILITY`
    /// off the streamed item object; `None` for indestructible items (max 0) or while the create
    /// hasn't landed. The real-instance tooltip's "Durability X / Y" line reads it (a template
    /// hover keeps the template's full max/max).
    pub durability: Option<(u32, u32)>,
    /// The item's running use-cooldown as `(start_ms on the GetTime clock, duration_ms,
    /// enabled)` — the same app-computed triple [`super::ActionState::cooldown`] carries;
    /// `None` = cold. Stored at push ([`super::UiScript::set_container`]) so
    /// `GetContainerItemCooldown` answers the reference's `(start, duration, enable)`.
    pub cooldown: Option<(i64, u32, bool)>,
    /// Right-clicking reads this item (an instance carrying `ITEM_FIELD_ITEM_TEXT_ID` — a mail
    /// permanent copy). `GetContainerItemInfo`'s `isReadable`; the bag hover shows the Inspect
    /// magnifier off it (ref ContainerFrame.lua l.638 `this.readable → ShowInspectCursor()`),
    /// and the tooltip's WRITTEN_BY/READABLE gates key on it.
    pub readable: bool,
    /// The RESOLVED `ITEM_FIELD_CREATOR` name (app: ask-once name cache) — the tooltip's
    /// "Written by %s" (a letter) / "<Made by %s>" (anything crafted) line. `None` = authorless
    /// or the name query is in flight (no line; the re-push repaints the hover when it lands).
    pub creator: Option<String>,
    /// The instance's `ITEM_FIELD_FLAGS` — the tooltip's openable lock sub-gate reads
    /// UNLOCKED `0x4`, the wrapped-gift arm WRAPPED `0x8` (see
    /// [`super::char_stats::InvSlotView::flags`], the doll twin).
    pub flags: u32,
    /// `0x5da2c0` — **the instance is runtime-bound**: `ITEM_FIELD_FLAGS & 1` (soulbound), or a
    /// live enchant slot naming a `SpellItemEnchantment` row that binds. App-resolved off the raw
    /// descriptor ([`crate::items::already_bound`] in benilla-app — the same predicate the enchant
    /// cursor's bind question asks), because the binding half needs a DBC join and the engine
    /// holds no item knowledge of its own. The tooltip's bind line overrides to **Soulbound**
    /// on it; `false` on any source with no streamed item object.
    pub already_bound: bool,
    /// The instance's enchant slots, resolved by the app ([`super::EnchantView`]) and in
    /// enchant-slot order: it joins the id through `SpellItemEnchantment.dbc`'s name column and
    /// hands over the row's name plus the three facts the line law needs to place and paint it.
    /// Empty = unenchanted, or the enchant DBC never loaded.
    pub enchants: Vec<super::EnchantView>,
    /// **The instance's remaining LIFETIME in milliseconds** — a duration-limited item counting
    /// down to its own destruction (a conjured stone, a holiday gift, a timed quest item). Its
    /// only feed is `SMSG_ITEM_TIME_UPDATE`, parked as an absolute deadline and recomputed on
    /// read ([`crate::items::Countdowns::lifetime_remaining_display_ms`] in benilla-app), floored to
    /// the whole second so the snapshot moves once a second rather than every frame.
    /// `None` = no timer, which is nearly every item.
    pub duration_ms: Option<u64>,
    /// The petition this item names, when it is a signable charter — **line 3 of the tooltip's
    /// emission law**, between the NAME and the green `ITEM_SIGNABLE` line.
    ///
    /// `None` for every ordinary item, and also for a charter whose petition record has not
    /// arrived: the hover is what ISSUES that query, so the first hover of an unopened charter
    /// shows the name and the green line, and the two guild lines appear on the repaint. Exactly
    /// [`Self::creator`]'s shape one field up, for exactly its reason.
    pub petition: Option<PetitionSlotView>,
}

/// One enchant slot as the tooltip renders it (`0x52c9f9`–`0x52ca23`). The app
/// resolves the DBC row; every rule below is the engine's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnchantView {
    /// `ITEM_FIELD_ENCHANTMENT` slot: **0** permanent · **1** temporary · **2..6** the
    /// random-property suffix. It decides the colour band — only 0 and 1 are ever coloured.
    pub slot: u8,
    /// The enchant id was NEGATIVE. `abs(id)` names the row either way — the sign's only effect is
    /// to paint slots 0/1 pure-red instead of green (`0x52ca29–0x52ca49`).
    pub negative: bool,
    /// The `SpellItemEnchantment` row's name, verbatim (`"Agility +15"`, `"Crusader"`) — the
    /// reference copies the string with no format at all (`0x52ca8b–0x52caa1`).
    pub name: String,
    /// The slot's `ITEM_FIELD_ENCHANTMENT` charges dword — nonzero appends " (N Charges)".
    pub charges: u32,
    /// Milliseconds left on a TEMPORARY enchant, from `SMSG_ITEM_ENCHANT_TIME_UPDATE` (the item's
    /// own duration field is never read for this). `Some` replaces the plain name with the
    /// countdown phrasing; `None` = no timer, or expired.
    pub remaining_ms: Option<u64>,
}

/// One `ItemRandomProperties` row as the engine renders it — the **random-suffix roll** a dropped,
/// linked, auctioned or mailed item carries. The app resolves the DBC (both of
/// them: the roll's table and the `SpellItemEnchantment` names its five ids land on) and pushes the
/// whole table once at load; a tooltip source supplies only the id, exactly as in the reference.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RandomPropertyView {
    /// The roll's suffix — `"of the Monkey"`. The reference joins it onto the item name through
    /// `ITEM_SUFFIX_TEMPLATE` (`"%s %s"`) in its one name formatter `0x5d8b00`; benilla does that
    /// join app-side, where every display name is composed, so this is carried for reference and
    /// for the surfaces that name an item the app never composed.
    pub suffix: String,
    /// The enchant lines the roll grants, already named and seated in enchant slots **2..6** —
    /// the five dwords the builder copies out of the suffix row (`0x52b7e0–0x52b7fb`). Slots 2..6
    /// are always white, whatever the sign.
    pub enchants: Vec<EnchantView>,
}

/// One bag's snapshot: its name, capacity, and occupied slots (1-based).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContainerState {
    /// `GetBagName` (the bag item's own name; the backpack is the client-local "Backpack").
    pub name: Option<String>,
    pub num_slots: u32,
    pub slots: std::collections::HashMap<u32, ContainerSlot>,
}

/// One queued backpack pick/place/swap/split: move the item from `(src_bag, src_slot)` to
/// `(dst_bag, dst_slot)` (live-API space, 1-based slots). The app maps a backpack-internal move
/// (both bags 0) onto `CMSG_SWAP_INV_ITEM` player-array slots. `count`: `None` = a whole-stack
/// move/swap (including a same-item merge — the wire tops the stack up itself); `Some(n)` = a
/// split placement (`SplitContainerItem`) the app maps onto `CMSG_SPLIT_ITEM` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerMove {
    pub src_bag: i64,
    pub src_slot: u32,
    pub dst_bag: i64,
    pub dst_slot: u32,
    pub count: Option<u32>,
}

/// One queued **auto-store into a bag** — `PutItemInBag`'s third leg and the whole of
/// `PutItemInBackpack` (`crate::script::cursor::bag_verbs`): take the item at `(src_bag, src_slot)`
/// and put it anywhere inside container `dst_bag`.
///
/// **There is no destination slot, and that is the finding, not an omission**: the reference's
/// `CMSG_AUTOSTORE_BAG_ITEM 0x10B` carries `(srcbag, srcslot, dstbag)` and nothing else, and its
/// split sibling `CMSG_SPLIT_ITEM 0x10E` carries the literal `0xFF` where a slot would go. The
/// server picks. `count`: `None` = the whole stack (AUTOSTORE); `Some(n)` = a split carry, which
/// the drain sends as SPLIT instead — the same fork the reference makes on `[0xb4b40c]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BagAutoStore {
    pub src_bag: i64,
    pub src_slot: u32,
    /// Live-API container id (0 = backpack, 1..=4 an equipped bag, 5..=10 a bank bag).
    pub dst_bag: i64,
    pub count: Option<u32>,
}

impl super::UiScript {
    /// Drain the auto-stores `PutItemInBag`/`PutItemInBackpack` queued since the last call.
    pub fn take_bag_autostores(&mut self) -> Vec<BagAutoStore> {
        std::mem::take(&mut self.model_mut().bag_autostores)
    }

    /// Push (or remove, with `None`) one bag's snapshot, keyed by live-API bag id (0 = backpack).
    /// Each slot's cooldown arrives with its absolute start already on the `GetTime` clock (ms) —
    /// storing is a pure unit conversion, the same seam shape as
    /// [`super::UiScript::set_action_state`] — so `GetContainerItemCooldown` answers absolute
    /// `(start, duration, enable)` without drift.
    pub fn set_container(&mut self, bag: i64, state: Option<ContainerState>) {
        let mut model = self.model_mut();
        model.container_cooldowns.retain(|&(b, _), _| b != bag);
        match state {
            Some(s) => {
                for (&slot, cs) in &s.slots {
                    let Some((start_ms, duration_ms, enabled)) = cs.cooldown else {
                        continue;
                    };
                    let abs = (
                        start_ms as f64 / 1000.0,
                        f64::from(duration_ms) / 1000.0,
                        enabled,
                    );
                    model.container_cooldowns.insert((bag, slot), abs);
                }
                model.containers.insert(bag, s);
            }
            None => {
                model.containers.remove(&bag);
            }
        }
    }

    /// Push the app's answer to `HasKey()` — whether the player owns any `BagFamily` KEYS item
    /// anywhere the reference's own search reaches. The keyring UI's one gate.
    pub fn set_has_key(&mut self, has_key: bool) {
        self.model_mut().has_key = has_key;
    }

    /// Drain the `(bag, slot)` pairs queued by `UseContainerItem` since the last call.
    pub fn take_container_uses(&mut self) -> Vec<(i64, u32)> {
        std::mem::take(&mut self.model_mut().container_uses)
    }

    /// Drain the pick/place/swap moves queued by `PickupContainerItem` since the last call.
    pub fn take_container_moves(&mut self) -> Vec<ContainerMove> {
        std::mem::take(&mut self.model_mut().container_moves)
    }

    /// Drain the `(bag, slot)` sources `AutoEquipCursorItem` queued (decision 0208 phase 1b) —
    /// the app resolves each to wire bag/slot and sends `CMSG_AUTOEQUIP_ITEM`.
    pub fn take_container_autoequips(&mut self) -> Vec<(i64, u32)> {
        std::mem::take(&mut self.model_mut().container_autoequips)
    }

    /// Drain the `(bag, slot)` repair clicks the repair-mode pickup intercept queued — the app
    /// resolves each to its item guid and sends `CMSG_REPAIR_ITEM`.
    pub fn take_container_repairs(&mut self) -> Vec<(i64, u32)> {
        std::mem::take(&mut self.model_mut().container_repairs)
    }

    /// Drain the `(bag, slot, count)` destroys `DeleteCursorItem` queued (`count == 0` = the
    /// whole stack) — the app resolves each to its item guid and sends `CMSG_DESTROYITEM`.
    pub fn take_container_destroys(&mut self) -> Vec<(i64, u32, u32)> {
        std::mem::take(&mut self.model_mut().container_destroys)
    }

    /// **Arm the gift-wrap cursor** on the paper at `(bag, slot)` — the app's call when a
    /// right-click takes the reference's begin-wrap arm (`ItemInfo::begins_gift_wrap`).
    ///
    /// Three acts, exactly the three `0x5edea0` performs: the paper's slot reads **locked**, the
    /// displayed cursor becomes **mode 2** (`Interface\Cursor\Cast.blp` — the same art the cast
    /// cursor uses, table `0x853b88` slot 2), and **nothing goes on the wire**. What completes it
    /// is the next left-click on a container slot.
    pub fn arm_gift_wrap(&mut self, bag: i64, slot: u32) {
        let mut model = self.model_mut();
        model.pending_wrap = Some(PendingWrap { bag, slot });
        model.ui_cursor = Some(UiCursorMode::Cast);
        model.ui_cursor_dirty = true;
        cursor::queue_lock_changed(&mut model, bag, slot);
    }

    /// **Cancel an armed gift wrap and nothing else** — disarm, unlock the paper, reset the
    /// cursor. Deliberately narrower than `ClearCursor`, which also drops any held payload:
    /// the reference records that a right-click "uses the item and cancels the wrap," not by
    /// which of the two routes, so this touches only the half it names.
    pub fn cancel_gift_wrap(&mut self) -> Option<PendingWrap> {
        let mut model = self.model_mut();
        let wrap = model.pending_wrap.take()?;
        cursor::queue_lock_changed(&mut model, wrap.bag, wrap.slot);
        model.ui_cursor = None;
        model.ui_cursor_dirty = true;
        Some(wrap)
    }

    /// Is a gift wrap armed, and on which slot? Read by the app to decide whether a right-click
    /// is cancelling one.
    pub fn gift_wrap_armed(&self) -> Option<PendingWrap> {
        self.model_ref().pending_wrap
    }

    /// Drain the `(giftBag, giftSlot, itemBag, itemSlot)` quads a completed wrap queued — the app
    /// resolves each pair to the wire's bag/slot addressing and sends `CMSG_WRAP_ITEM`.
    pub fn take_container_wraps(&mut self) -> Vec<(i64, u32, i64, u32)> {
        std::mem::take(&mut self.model_mut().container_wraps)
    }

    /// The mode the last FrameXML cursor call armed — `None` after a `ResetCursor`. This is the
    /// *value* of the most recent write; whether a write happened at all is
    /// [`Self::take_cursor_write`], and that is what the app acts on.
    pub fn ui_cursor(&self) -> Option<UiCursorMode> {
        self.model_ref().ui_cursor
    }

    /// Drain the pending cursor write: `Some(mode)` = a `Show*Cursor` armed `mode`, `Some(None)` =
    /// a `ResetCursor` asked for the base mode, `None` = **no FrameXML cursor call happened**, so
    /// the sticky mode stands untouched.
    ///
    /// The three-state return is the point. A UI element with no cursor handler must leave the
    /// cursor exactly as it was — that is how an armed spell keeps its cast cursor while the mouse
    /// crosses a spellbook button, and reading a two-state latch instead is what made it snap to
    /// Point.
    #[allow(clippy::option_option)]
    pub fn take_cursor_write(&mut self) -> Option<Option<UiCursorMode>> {
        let mut model = self.model_mut();
        std::mem::take(&mut model.ui_cursor_dirty).then_some(model.ui_cursor)
    }
}

/// **The armed gift wrap** — which piece of wrapping paper the next container click will spend.
///
/// The reference holds this as globals beside the displayed-cursor mode rather than as a cursor
/// *payload*; the distinction is load-bearing and is why this is its own type instead of a
/// [`cursor::CursorPayload`] variant (see [`super::Model::pending_wrap`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PendingWrap {
    /// The paper's container id, in the same bag space every cursor verb speaks.
    pub bag: i64,
    /// The paper's 1-based slot within that container.
    pub slot: u32,
}

/// A **displayed-cursor override** the FrameXML cursor family arms: the
/// single "displayed mode" (`0xbe2c2c`) the real client swaps to while a UI element wants a non-base
/// cursor, restored to the base mode by `ResetCursor`. The app maps each to the matching
/// `Interface\Cursor\*` art over the world classifier's Point.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiCursorMode {
    /// Buy(3) — the coin/pouch: selling a bag item to a vendor (`ShowContainerSellCursor`, no
    /// affordability gate), or a merchant/buyback item you can afford (`Show*SellCursor`).
    Buy,
    /// UnableBuy(23) — the grayed coin: a merchant/buyback item you can't afford (`Show*SellCursor`,
    /// player coin < price).
    UnableBuy,
    /// Inspect(7) — the magnifier the Ctrl-hover shows over an item (`ShowInspectCursor`).
    Inspect,
    /// Point(1) — `SetCursor("POINT_CURSOR")`, and what a `ResetCursor` resolves to.
    Point,
    /// Cast(2) — `SetCursor("CAST_CURSOR")`: the lit cast cursor a unit frame shows while a spell
    /// that CAN bind that unit is armed (`UnitFrame_OnEnter`).
    Cast,
    /// UnableCast(22) — `SetCursor("CAST_ERROR_CURSOR")`: the greyed twin, for a unit the armed
    /// spell cannot bind, and for `UnitFrame_OnLeave` while still targeting.
    CastError,
}

/// The pick/place/swap gesture behind `PickupContainerItem` (amended by 0218;
/// unit-testable without Lua). Mirrors the real client's single entry point:
/// - an empty cursor over a resolved, UNLOCKED slot picks it up (a locked slot refuses — the real
///   client's refusal; today only the app's `locked` flag drives this, the engine's own
///   pending-move lock lands with slice 2);
/// - holding, a click on the SAME slot cancels (a split carry included);
/// - holding the WHOLE stack (`count: None`), a click elsewhere queues the move and CLEARS —
///   empty, same-item (the wire merges), and different-item (the wire swaps: the displaced item
///   lands where the held one came from) all alike. The displaced item never hops onto the
///   cursor: 0216 §2 shipped that exchange off 0091's gloss; the director's eye caught it and
///   the bytes refuted it — no `SetCursorItem` on the place branch
///   (`0x5e0c40`/`0x4f9b30`), `ClearCursor(0)`, one put-down sound. Bag
///   placements are server-authoritative; only the ACTION bar
///   hops its displaced payload (client-authoritative — the slice-4 note in 0218).
/// - holding a SPLIT carry (`count: Some(n)`), a click onto an empty/unresolved/same-item
///   destination queues the split move and clears; onto a DIFFERENT item it's a no-op (kept —
///   you can't swap a partial stack).
/// - a spell/action payload refuses a bag slot outright (no-op, kept).
///
/// Returns whether the caller should repaint (the source-slot lock or the held payload changed).
fn pickup_container_item(model: &mut super::Model, bag: i64, slot: u32) -> bool {
    // **The gift-wrap completion, ahead of the whole gesture**. `0x4f9b30` is the
    // *only* entry to the wrap sender `0x5edfc0` image-wide — which is why the paper doll cannot
    // finish a wrap and an equipped item is therefore unwrappable — and it takes this branch
    // before it looks at the cursor at all.
    if let Some(wrap) = model.pending_wrap {
        // An empty or unresolved slot **bails and leaves the wrap armed** — the reference's own
        // edge. Nothing is spent and the cursor keeps its mode, so the next click can still land
        // on a real item.
        if !model
            .containers
            .get(&bag)
            .and_then(|c| c.slots.get(&slot))
            .is_some_and(|s| s.item_id != 0)
        {
            return false;
        }
        // **The paper has to still be there.** `0x5edfef` bails when the stashed gift GUID no
        // longer resolves, and — the edge worth transcribing — it then sends *and clears*
        // nothing: the wrap stays armed and the paper's slot stays locked. So this returns
        // without touching either.
        if !model
            .containers
            .get(&wrap.bag)
            .and_then(|c| c.slots.get(&wrap.slot))
            .is_some_and(|s| s.item_id != 0)
        {
            return false;
        }
        // No eligibility test of any kind. The wrap arm reads each item's `ITEM_FIELD_CONTAINED`
        // and nothing else — not flags, not stack count, not bind state — and the client ships six
        // `ERR_CANT_WRAP_*` strings for the server to answer with (`SMSG_INVENTORY_CHANGE_FAILURE`
        // reasons 43-48). Same law as the openable click: gating locally would eat
        // the click in silence and cost the player their error line.
        model.pending_wrap = None;
        model.container_wraps.push((wrap.bag, wrap.slot, bag, slot));
        // The paper unlocks and the cursor resets **immediately**, not on the server's answer:
        // `0x5edfc0` calls `0x5eded0` at `0x5ee0aa`, right after the send. The target is never
        // locked at all.
        cursor::queue_lock_changed(model, wrap.bag, wrap.slot);
        model.ui_cursor = None;
        model.ui_cursor_dirty = true;
        return true;
    }
    match model.cursor.take() {
        None => {
            let picked = model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .filter(|s| s.item_id != 0 && !s.locked)
                .map(|s| CursorItem {
                    bag,
                    slot,
                    item_id: s.item_id,
                    texture: s.texture.clone(),
                    link: s.link.clone(),
                    quality: s.quality,
                    count: None,
                    bar_placeable: s.bar_placeable,
                    equip_slots: s.equip_slots.clone(),
                });
            match picked {
                Some(item) => {
                    model.cursor = Some(CursorPayload::Item(item));
                    cursor::queue_cursor_update(model);
                    cursor::queue_lock_changed(model, bag, slot);
                    true
                }
                None => false,
            }
        }
        Some(CursorPayload::Item(held)) if held.bag == bag && held.slot == slot => {
            cursor::queue_cursor_update(model);
            cursor::queue_lock_changed(model, held.bag, held.slot);
            true
        }
        Some(CursorPayload::Item(held)) => {
            // Cloned out from under `model.containers` so the moves below can borrow `model`
            // mutably; a slot present but unresolved (item_id == 0, an in-flight template
            // answer) reads as absent, same as a truly empty slot.
            let dest = model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .cloned()
                .filter(|d| d.item_id != 0);
            match (held.count, dest) {
                // A split carry onto a DIFFERENT item can't swap a partial stack — no-op, kept.
                (Some(_), Some(d)) if d.item_id != held.item_id => {
                    model.cursor = Some(CursorPayload::Item(held));
                    false
                }
                // Every other placement queues its move and clears: empty and same-item take the
                // carry's count through to the wire (a split placement or a merge the server
                // tops up); a whole-stack place onto a different item is the plain SWAP — the
                // displaced item lands where the held one came from, and the cursor empties
                // (the director's eye over 0216 §2's hop).
                (count, _) => {
                    queue_move(model, &held, bag, slot, count);
                    cursor::queue_cursor_update(model);
                    cursor::queue_lock_changed(model, held.bag, held.slot);
                    true
                }
            }
        }
        // Mode 5 — **the buy**. A held vendor row dropped into a container slot is the whole
        // point of the merchant cursor: `0x4f9efa` tests `[0xb4d900] == 5` right here and calls
        // `0x5e1f30` = `CMSG_BUY_ITEM_IN_SLOT 0x1a3`, `{vendorGuid, itemEntry, bagGuid, bagSlot,
        // count = 1}`. The cursor clears whether or not the server honours it.
        //
        // Deliberately NOT gated on the destination being empty. The reference does not look at
        // the slot at all — the server decides, and answers `SMSG_BUY_FAILED` when it will not.
        // What we DO refuse is a stale row: the reference's two container drop consumers
        // dereference the vendor row without re-checking it against a list that may have been
        // rewritten by a fresh `SMSG_LIST_INVENTORY`, which is a latent null deref there
        // (`0x4f9f35`, `0x4c7ea5`). We treat a row that no longer resolves as a refusal — that is
        // the third consumer `0x4c7300`'s own behaviour, and the one of the three that is right.
        Some(CursorPayload::Merchant(held)) => {
            let entry = model
                .merchant
                .as_ref()
                .and_then(|m| m.items.get(held.row as usize))
                .filter(|it| it.item_id == held.item_id)
                .map(|it| it.item_id);
            if let Some(entry) = entry {
                model.merchant_slot_buys.push((bag, slot, entry));
            }
            cursor::queue_cursor_update(model);
            true
        }
        Some(
            other @ (CursorPayload::Spell(_)
            | CursorPayload::Action(_)
            | CursorPayload::Macro(_)
            | CursorPayload::PetAction(_)
            // Mode 10 — a stabled pet refuses a bag/doll slot exactly as the
            // spell/action family does, and stays on the cursor for the stable window to take.
            | CursorPayload::StablePet(_)
            // Mode 2 (1962) — coins have no slot to land in; a money frame's DropFunc takes them.
            | CursorPayload::Money(_)),
        ) => {
            model.cursor = Some(other);
            false
        }
    }
}

/// Queue one pick/place/swap/split move (the shared tail of every clearing branch above).
fn queue_move(
    model: &mut super::Model,
    held: &CursorItem,
    dst_bag: i64,
    dst_slot: u32,
    count: Option<u32>,
) {
    model.container_moves.push(ContainerMove {
        src_bag: held.bag,
        src_slot: held.slot,
        dst_bag,
        dst_slot,
        count,
    });
}

/// Register the container verbs — **top-level globals, because that is where 1.12 puts them**.
/// Every name below is `function engine` in `reference/1.12-globals.tsv` as a
/// bare global: `GetContainerNumSlots`, never `C_Container.GetContainerNumSlots`. The
/// `C_Container` namespace 1187 reached for while chasing an Era addon is a *Dragonflight*
/// reorganisation, and an addon that feature-detects it concludes it is on Dragonflight.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // ContainerIDToInventoryID(containerID) → the equipment slot the bag is worn in.
    //
    // **Two linear arms, one signed compare, and nothing else** (`0x4f94e0`, 124 bytes). Written
    // as the reference's
    // own three instructions — `t = id - 1`, `if t < 4` (SIGNED), then `+20` or `+60` — rather
    // than as the two closed forms `id+19` / `id+59`, because the wrap at the `i32` edge is then
    // reproduced rather than approximated.
    //
    // | containerID | slot |
    // |---|---|
    // | **−2 (keyring)** | **17** |
    // | −1 | 18 |
    // | **0 (backpack)** | **19** |
    // | 1 · 2 · 3 · 4 | 20 · 21 · 22 · 23 |
    // | **5** (first bank bag) | **64** |
    // | 6 … 10 | 65 … 69 |
    //
    // **There is no special case for 0 or for −2** — the "backpack is 0, keyring is −2"
    // convention is the *caller's*, and this arithmetic merely happens to land on 19 and 17.
    // **And there is no range check of any kind**: the only guard in the function is the up-front
    // type test, so an out-of-range id is not clamped, not rejected, and never nil — it returns
    // `id+19` or `id+59` as an ordinary number, and whatever the receiving binding does with an
    // invalid slot is that binding's business.
    lua.globals().set(
        "ContainerIDToInventoryID",
        lua.create_function(|lua, id: Value| {
            let id = super::binding_abi::number_arg(
                lua,
                id,
                "Usage: ContainerIDToInventoryID(containerID)",
            )?;
            // `0x4f951b dec eax` · `0x4f951f cmp eax,4` · `0x4f9524 jl` — the compare is signed
            // and lands on the `id <= 4` arm.
            let t = id.wrapping_sub(1);
            Ok(i64::from(if t < 4 {
                t.wrapping_add(20)
            } else {
                t.wrapping_add(60)
            }))
        })?,
    )?;

    // SetBagPortaitTexture(texture, containerID) — `0x4fa4f0` (374 B), the reference's own
    // spelling, typo included, and CARVED. The bag window's portrait:
    // `ContainerFrame_GenerateFrame` sets it for every container except the keyring, which takes
    // `SetPortraitToTexture` with a fixed path one line above (ContainerFrame.lua l.418-423).
    // benilla runs that file off the player's chain (1751), so this is a live call site.
    //
    // Three arms, and the FIRST is the one nobody would have guessed:
    //
    // 1 · **`id == 0` does nothing at all.** `0x4fa5b8 dec edi` / `0x4fa5b9 js 0x4fa65d` returns
    //     before the clear at `0x4fa5c8` — no texture is set and none is cleared. It is not an
    //     oversight: `ContainerFrame.xml:67` gives `$parentPortrait` no `file=`, and the backpack
    //     takes a different ARTWORK background (`UI-BackpackBackground`) that covers the portrait
    //     slot outright. (`Button-Backpack-Up` is nowhere on this path — its only stock use is the
    //     main bar's own button, MainMenuBarBagButtons.xml:79.)
    // 2 · **`id` in 1..=10** clears the texture first, then sets
    //     `Interface\Icons\<ItemDisplayInfo.InventoryIcon_1>` for the bag in that inventory slot
    //     — `StringLookups.dbc` row 3 measured as `"Interface\Icons"`, `.TGA` stripped,
    //     `INV_Misc_QuestionMark` on a display-info miss. Ids 5..10 are additionally gated on the
    //     bank being open. **An empty slot leaves the texture CLEARED** — there is no fallback art.
    // 3 · **`id >= 11` raises** `"Invalid slot in SetBagPortaitTexture"`.
    //
    // Our icon comes from the same place the item's own button icon does (`GetInventoryItemTexture`
    // reads the item template's icon), so the DBC walk above is the reference's route to the value
    // this client already has resolved; the observable — which icon lands on the portrait — is the
    // same one.
    lua.globals().set(
        "SetBagPortaitTexture",
        lua.create_function(|lua, (region, container): (Table, i64)| {
            // Arm 3, before anything is touched.
            if container >= 11 {
                return Err(mlua::Error::runtime("Invalid slot in SetBagPortaitTexture"));
            }
            // Arm 1: the backpack is a silent return, not a clear.
            if container <= 0 {
                return Ok(());
            }
            // `ContainerIDToInventoryID`'s arithmetic, deliberately re-expressed rather than
            // called: that binding is a Lua global an addon may replace, and the engine's own
            // portrait read does not go through Lua.
            let t = container.wrapping_sub(1);
            let inv = if t < 4 {
                t.wrapping_add(20)
            } else {
                t.wrapping_add(60)
            };
            let path = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                usize::try_from(inv)
                    .ok()
                    .and_then(|slot| model.inv_slot("player", slot))
                    .and_then(|v| v.icon.clone())
            };
            let rh = crate::script::region::region_handle_of(lua, &region)?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let data = model.region_data.entry(rh).or_default();
            // Arm 2. `path` is `None` for an empty slot, and that IS the answer: the reference
            // clears unconditionally and only then tries to set.
            data.texture = path;
            data.circular = true;
            data.portrait_unit = None;
            Ok(())
        })?,
    )?;

    lua.globals().set(
        "GetContainerNumSlots",
        lua.create_function(|lua, bag: i64| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.containers.get(&bag).map_or(0, |c| c.num_slots))
        })?,
    )?;

    lua.globals().set(
        "GetBagName",
        lua.create_function(|lua, bag: i64| {
            let name = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.containers.get(&bag).and_then(|c| c.name.clone())
            };
            match name {
                Some(n) => Ok(Value::String(lua.create_string(&n)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // The reference's `GetContainerItemCooldown(bag, slot)` — the bag twin of
    // `GetActionCooldown`, identical conventions: `GetTime`-clock `(start, duration, enable)`,
    // enable 0 = an on-hold record (parked, full duration), and the cold-at-expiry guard so an
    // event-driven re-feed can never replay the finish flash.
    lua.globals().set(
        "GetContainerItemCooldown",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let now: f64 = lua.globals().get("__benilla_now").unwrap_or(0.0);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match model.container_cooldowns.get(&(bag, slot)) {
                Some(&(start, duration, enabled)) if start + duration > now || !enabled => {
                    (start, duration, i32::from(enabled))
                }
                _ => (0.0, 0.0, 1),
            })
        })?,
    )?;

    lua.globals().set(
        "GetContainerItemLink",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .containers
                    .get(&bag)
                    .and_then(|c| c.slots.get(&slot))
                    .and_then(|s| s.link.clone())
            };
            match link {
                Some(l) => Ok(Value::String(lua.create_string(&l)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // `GetContainerItemInfo(bag, slot)` → **`texture, itemCount, locked, quality, readable`**
    // — the 1.12 shape, five values.
    //
    // It used to return a single 1.14-shaped `containerInfo` TABLE, inherited from decision 1187's
    // reach for the Classic Era surface. That was wrong in a way neither instrument could see:
    // **36 corpus addons call this and every one of them uses the 1.12 shape. Not one uses the
    // table.** 34 destructure (`local texture, itemCount = GetContainerItemInfo(bag, slot)`), and
    // the four that assign a single name take the *first return* — `local texture = …`, or
    // `GetContainerItemInfo(bag, i) ~= nil` as an occupancy test. All 36 were getting
    // `texture = <table>` and, where they destructured, `itemCount = nil`. They load clean, so the
    // harness scores them as passes, and they misbehave silently in play.
    //
    // (Decision 1199 §1 says "only one uses the table shape" — that came from a first reading of
    // the call sites and a recount disproves it. The corrected number makes the case stronger, not
    // weaker; the record is a point-in-time snapshot and this is where the true count lives.)
    //
    // The shipped 1.12 FrameXML settles the shape: `ContainerFrame.lua:241` reads exactly these
    // five names in exactly this order.
    //
    // **A signature is part of the API.** Decision 1198 §3 made that argument about *names*; this
    // is the same argument one level down, and it is the more dangerous half, because a wrong name
    // fails loudly and a wrong shape does not.
    //
    // `nil` for an empty or unknown slot — the reference returns no values there, and a caller's
    // `if texture then` reads the same either way.
    lua.globals().set(
        "GetContainerItemInfo",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let (info, held_here) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let held = matches!(
                    &model.cursor,
                    Some(CursorPayload::Item(c)) if c.bag == bag && c.slot == slot
                ) ||
                // An armed gift wrap locks its paper for as long as it stands (`0x4953e0` at
                // `0x5edeb5`) — the same dimming a held item's source slot shows, and released by
                // the send or by any cancel rather than by the server.
                    matches!(
                        &model.pending_wrap,
                        Some(w) if w.bag == bag && w.slot == slot
                    );
                (
                    model
                        .containers
                        .get(&bag)
                        .and_then(|c| c.slots.get(&slot))
                        .cloned(),
                    held,
                )
            };
            let Some(s) = info else {
                return Ok(mlua::MultiValue::new());
            };
            Ok(mlua::MultiValue::from_vec(vec![
                match &s.texture {
                    Some(p) => Value::String(lua.create_string(p.as_str())?),
                    None => Value::Nil,
                },
                Value::Integer(i64::from(s.count)),
                // The picked-up source slot reads locked (the real client dims it while the cursor
                // carries the item) — derived from the cursor, not a mutated snapshot, so the
                // app's per-frame re-push cannot wipe it.
                Value::Boolean(s.locked || held_here),
                match s.quality {
                    Some(q) => Value::Integer(i64::from(q)),
                    None => Value::Nil,
                },
                Value::Boolean(s.readable),
            ]))
        })?,
    )?;

    // `BenillaGetContainerItemID(bag, slot)` — the item id behind a slot, for OUR OWN FrameXML.
    //
    // 1.12 has no such verb: an addon there takes the id out of `GetContainerItemLink`'s
    // `|Hitem:12345:…` payload. Our tooltip and delete paths want the id directly, so it rides the
    // `Benilla` host-bridge prefix — the sanctioned escape hatch for a verb only our own
    // transcription calls, and the one `reference_surface` covers by prefix rather than by
    // exception. An addon that wants it parses the link, exactly as it would on the real client.
    lua.globals().set(
        "BenillaGetContainerItemID",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // `nil` for an unresolved id, not `0` — a slot can carry an entry whose template has
            // not landed yet, and `0` is a number a caller will happily index a table with.
            Ok(model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .map(|s| s.item_id)
                .filter(|&id| id != 0)
                .map(i64::from))
        })?,
    )?;

    lua.globals().set(
        "UseContainerItem",
        lua.create_function(|lua, (bag, slot, _rest): (i64, u32, mlua::MultiValue)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.container_uses.push((bag, slot));
            Ok(())
        })?,
    )?;

    // The left-click drag gesture: pick up from an occupied slot, or place/swap when holding.
    // Returns whether the caller should repaint (the source-slot lock changed) — the OnClick calls
    // the bag's `_Update` on true so the picked slot dims / un-dims immediately (no server round
    // trip for the local cursor state).
    lua.globals().set(
        "PickupContainerItem",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // Two intercepts live IN the pickup path, in the reference's own order (decision
            // 0923 re-read `PickupContainerItem 0x4f9b30` whole; the targeting rung is at
            // `4f9c54` and the repair test at `4f9c7b`, so targeting wins — a poisoning click at
            // a repair vendor binds the poison, it does not repair):
            //
            //   4f9c54  call 0x6e48a0        ; IsTargeting
            //   4f9c5d  call 0x6e6330        ; TargetingWantsItem (word & 0x4010)
            //   4f9c6d  call 0x495d60        ; bind THIS item — then return, nothing picked up
            //
            // The held-payload check precedes BOTH (`4f9c38`: a non-empty cursor jumps to the
            // place/swap arm long before either), so a click while carrying an item is a place —
            // transcribed by the `cursor.is_none()` gate. The word is one-shot: the app clears it
            // on completion, cancel, or close.
            if model.item_pick_armed && model.cursor.is_none() {
                model.item_picks.push((bag, slot));
                return Ok(false);
            }
            // Repair mode (`0x4f9c7b`): while the repair cursor is
            // armed, the click means "repair this item" — queued for the app to send — and
            // nothing is picked up. The mode STICKS across clicks (only HideRepairCursor /
            // merchant-close clears it).
            if model.repair_mode {
                model.container_repairs.push((bag, slot));
                return Ok(false);
            }
            Ok(pickup_container_item(&mut model, bag, slot))
        })?,
    )?;

    // The cursor globals (`GetCursorInfo`/`CursorHasItem`/`CursorHasSpell`/`ClearCursor`/
    // `SplitContainerItem`/`DeleteCursorItem`) live in [`super::cursor`], the one payload seam
    // every surface routes through — top-level there for exactly the same reason.

    // ShowContainerSellCursor(bag, slot) — arm the pouch cursor for a sellable hover (5875
    // `0x4fa460`: Buy(3) only when the slot actually holds an item —
    // an empty slot leaves the cursor unchanged; no Unable twin, no SellPrice check — selling
    // never grays), and the reference's own `IsTargeting` bail at its first instruction.
    lua.globals().set(
        "ShowContainerSellCursor",
        lua.create_function(|lua, (bag, slot): (i64, u32)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let occupied = model
                .containers
                .get(&bag)
                .and_then(|c| c.slots.get(&slot))
                .is_some();
            // `IsTargeting` bails at the function's FIRST instruction (`0x4fa469`) — an armed spell
            // suppresses the sell cursor outright. Now
            // that the mode is sticky, this gate has to be here rather than app-side: a write of
            // Buy would otherwise stamp over the cast cursor and there would be no per-frame world
            // write to put it back.
            if occupied && !model.spell_targeting {
                model.ui_cursor = Some(UiCursorMode::Buy);
                model.ui_cursor_dirty = true;
            }
            Ok(())
        })?,
    )?;
    // ShowInspectCursor() — the Ctrl-hover magnifier (5875 `0x48ac60`: an unconditional
    // `CursorSetMode(Inspect=7)`). Shared across surfaces (the merchant window's Ctrl-hover, a bag
    // item's Ctrl-hover, the generic UIParent item hover), so it lives here beside `ResetCursor`
    // rather than in any one seam. It takes no arguments and reads no state.
    lua.globals().set(
        "ShowInspectCursor",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.ui_cursor = Some(UiCursorMode::Inspect);
            model.ui_cursor_dirty = true;
            Ok(())
        })?,
    )?;
    // HasKey() — "does this player own a key at all?" (5875 `0x48ae90`), the gate the main bar
    // reads to decide whether the keyring exists in the UI. The reference pushes the NUMBER 1 on a
    // hit and nil otherwise, not a boolean, and FrameXML only ever tests it for truth — so the
    // shape is reproduced exactly rather than normalized to a bool. The search itself (BagFamily
    // == 9 across equipment/bags/backpack/bank/keyring) is the app's, like every other item fact.
    lua.globals().set(
        "HasKey",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(if model.has_key {
                Value::Integer(1)
            } else {
                Value::Nil
            })
        })?,
    )?;
    // SetCursor(name) — the reference's `0x489490`: a name->mode table (POINT/CAST/BUY/ATTACK =
    // 1..4, each with an `*_ERROR` twin at +20) into `CursorSetMode`, falling back to a custom
    // bitmap for an unknown name. Only the modes FrameXML actually names are mapped here; anything
    // else is ignored rather than guessed, and a **no-arg call is a `ResetCursor`**, which is the
    // reference's own documented shape.
    //
    // Its one shipped caller is the unit-frame hover pair (`UnitFrame_OnEnter`/`OnLeave`), which is
    // the ONLY lit/grey cursor split over a UI element in 1.12 — and it is authored in Lua, not in
    // C++.
    lua.globals().set(
        "SetCursor",
        lua.create_function(|lua, name: Option<String>| {
            let mode = match name.as_deref() {
                None => None, // no-arg == ResetCursor
                Some("POINT_CURSOR") => Some(UiCursorMode::Point),
                Some("CAST_CURSOR") => Some(UiCursorMode::Cast),
                Some("CAST_ERROR_CURSOR") => Some(UiCursorMode::CastError),
                Some("BUY_CURSOR") => Some(UiCursorMode::Buy),
                Some("BUY_ERROR_CURSOR") => Some(UiCursorMode::UnableBuy),
                // An unmapped name: the reference would load a custom bitmap. We have no such art,
                // and silently painting the wrong stock cursor would be worse than leaving the
                // sticky mode alone, so this is a no-op rather than a guess.
                Some(_) => return Ok(()),
            };
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.ui_cursor = mode;
            model.ui_cursor_dirty = true;
            Ok(())
        })?,
    )?;
    // ResetCursor — displayed mode back to the base (the world classifier's) mode (5875
    // `0x48ac70` → `0x523d30`): clear the whole override, whichever `Show*Cursor` armed it.
    lua.globals().set(
        "ResetCursor",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.ui_cursor = None;
            model.ui_cursor_dirty = true;
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ContainerMove, ContainerSlot, ContainerState, PendingWrap, UiCursorMode};
    use crate::script::UiScript;

    fn backpack() -> ContainerState {
        let mut slots = std::collections::HashMap::new();
        slots.insert(
            1,
            ContainerSlot {
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Food_16".into()),
                count: 5,
                quality: Some(1),
                item_id: 117,
                link: Some("|cffffffff|Hitem:117|h[Tough Jerky]|h|r".into()),
                locked: false,
                equip_slots: Vec::new(),
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        // An in-flight slot: the create arrived, the template answer hasn't.
        slots.insert(3, ContainerSlot::default());
        // A readable letter (a mail permanent copy — instance item-text): isReadable true.
        slots.insert(
            4,
            ContainerSlot {
                item_id: 8383,
                count: 1,
                readable: true,
                ..Default::default()
            },
        );
        ContainerState {
            name: Some("Backpack".into()),
            num_slots: 16,
            slots,
        }
    }

    /// **The gift-wrap state machine**, all four of its edges. Armed by the app's
    /// right-click on a wrapper, the arm locks the paper and paints cursor mode 2 and sends
    /// nothing; the next LEFT-click on a container slot spends it; an empty slot bails and leaves
    /// it armed; and any `ClearCursor` cancels it.
    #[test]
    fn an_armed_gift_wrap_spends_on_the_next_container_click() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        // The arm: nothing on the wire, the paper reads locked, the cursor is mode 2, and the
        // held-item predicate stays FALSE — this is a cursor mode, not a payload.
        s.arm_gift_wrap(0, 1);
        assert_eq!(s.gift_wrap_armed(), Some(PendingWrap { bag: 0, slot: 1 }));
        assert!(s.take_container_wraps().is_empty(), "arming sends nothing");
        assert_eq!(s.take_cursor_write(), Some(Some(UiCursorMode::Cast)));
        assert!(
            s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "the paper dims while the wrap stands"
        );
        assert!(
            !s.eval::<bool>("return CursorHasItem()").unwrap(),
            "the wrap writes no payload, so every stock CursorHasItem() gate stays shut"
        );
        // A click on an EMPTY slot bails and leaves the wrap armed — the reference's own edge.
        s.eval::<()>("PickupContainerItem(0, 9)").unwrap();
        assert_eq!(s.gift_wrap_armed(), Some(PendingWrap { bag: 0, slot: 1 }));
        assert!(s.take_container_wraps().is_empty());
        // A click on a real item spends it: the paper's pair leads, as the wire does.
        s.eval::<()>("PickupContainerItem(0, 4)").unwrap();
        assert_eq!(s.take_container_wraps(), vec![(0, 1, 0, 4)]);
        assert_eq!(s.gift_wrap_armed(), None);
        assert_eq!(
            s.take_cursor_write(),
            Some(None),
            "the cursor resets on the send, not on the server's answer"
        );
        assert!(
            s.cursor_payload().is_none(),
            "the completing click is spent on the wrap, not on picking the target up"
        );
    }

    /// Any `ClearCursor` cancels an armed wrap — the reference tests it FIRST inside
    /// `ClearCursor 0x495190` and does so ungated by either parameter, so all 70 of its call
    /// sites cancel.
    #[test]
    fn clear_cursor_cancels_an_armed_gift_wrap() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.arm_gift_wrap(0, 1);
        let _ = s.take_cursor_write();
        s.eval::<()>("ClearCursor()").unwrap();
        assert_eq!(s.gift_wrap_armed(), None);
        assert_eq!(s.take_cursor_write(), Some(None), "and the mode goes back");
        assert!(
            !s.eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
                .unwrap(),
            "the paper unlocks with the cancel — the lock never waited on a server"
        );
        // …and the next container click is an ordinary pickup again.
        s.eval::<()>("PickupContainerItem(0, 1)").unwrap();
        assert!(s.take_container_wraps().is_empty());
        assert!(s.cursor_payload().is_some(), "a plain pickup, as before");
    }

    #[test]
    fn container_snapshot_reads() {
        let mut s = UiScript::new().unwrap();
        // No bag pushed: capacity 0, info nil.
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetContainerItemInfo(0, 1) == nil")
            .unwrap());

        s.set_container(0, Some(backpack()));
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(0)").unwrap(), 16);
        assert_eq!(
            s.eval::<String>("return GetBagName(0)").unwrap(),
            "Backpack"
        );
        // The 1.12 five-value shape: texture, itemCount, locked, quality,
        // readable — the names `ContainerFrame.lua:241` destructures into, in its order.
        let (icon, count, quality) = s
            .eval::<(String, i64, i64)>(
                "local texture, itemCount, locked, quality = GetContainerItemInfo(0, 1)\n\
                 return texture, itemCount, quality",
            )
            .unwrap();
        assert_eq!(icon, "Interface\\Icons\\INV_Misc_Food_16");
        assert_eq!((count, quality), (5, 1));
        // The item id is NOT one of the five — 1.12 has no verb for it, and an addon takes it out
        // of the link. Ours rides the host-bridge prefix.
        assert_eq!(
            s.eval::<i64>("return BenillaGetContainerItemID(0, 1)")
                .unwrap(),
            117
        );
        assert!(s
            .eval::<bool>("return GetContainerItemLink(0, 1) ~= nil")
            .unwrap());
        // isReadable mirrors the slot's readable bit (the letter in 4, not the jerky in 1).
        assert!(!s
            .eval::<bool>("local _, _, _, _, readable = GetContainerItemInfo(0, 1) return readable")
            .unwrap());
        assert!(s
            .eval::<bool>("local _, _, _, _, readable = GetContainerItemInfo(0, 4) return readable")
            .unwrap());

        // **The in-flight slot**: an entry exists but its template has not landed, so texture and
        // quality are nil while itemCount is real. This is the state that made the five-value
        // shape's occupancy test subtle — the reference's own `if texture then`
        // would read this as empty, which is why our own FrameXML tests `texture or itemCount`.
        assert!(s
            .eval::<bool>(
                "local texture, itemCount, locked, quality = GetContainerItemInfo(0, 3)\n\
                 return texture == nil and quality == nil and itemCount ~= nil",
            )
            .unwrap());
        assert!(s
            .eval::<bool>("return BenillaGetContainerItemID(0, 3) == nil")
            .unwrap());
        // Empty slot: nil.
        assert!(s
            .eval::<bool>("return GetContainerItemInfo(0, 2) == nil")
            .unwrap());
    }

    #[test]
    fn use_container_item_queues_intents() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("UseContainerItem(0, 1)").unwrap();
        s.run("UseContainerItem(0, 3, 'target')").unwrap();
        assert_eq!(s.take_container_uses(), vec![(0, 1), (0, 3)]);
        assert!(s.take_container_uses().is_empty(), "drained");
    }

    #[test]
    fn pickup_then_place_swaps_and_clears_cursor() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack())); // slot 1 = Tough Jerky (resolved), slot 3 = in-flight
        assert!(s.cursor_item().is_none());
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());

        // Pick up slot 1 → cursor holds it, the source slot reads locked, no move queued yet.
        assert!(s.eval::<bool>("return PickupContainerItem(0, 1)").unwrap());
        let held = s.cursor_item().expect("cursor holds the picked item");
        assert_eq!((held.bag, held.slot, held.item_id), (0, 1, 117));
        assert_eq!(
            held.texture.as_deref(),
            Some("Interface\\Icons\\INV_Misc_Food_16")
        );
        assert!(s.eval::<bool>("return CursorHasItem()").unwrap());
        assert!(s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
            .unwrap());
        assert!(s.take_container_moves().is_empty());

        // GetCursorInfo reports the item.
        let (kind, id) = s
            .eval::<(String, i64)>("local k, id = GetCursorInfo() return k, id")
            .unwrap();
        assert_eq!((kind.as_str(), id), ("item", 117));

        // Place onto slot 5 → a move (0,1)->(0,5) queues, cursor clears, source un-locks.
        assert!(s.eval::<bool>("return PickupContainerItem(0, 5)").unwrap());
        assert!(s.cursor_item().is_none());
        assert!(!s.eval::<bool>("return CursorHasItem()").unwrap());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 5,
                count: None,
            }]
        );
        // `local _, _, locked` — the 1.12 five-value shape, the same destructuring the positive
        // assertion above uses. This read `local i = … return i.isLocked` until 2171: a leftover
        // of the 1.14 `containerInfo` TABLE that 1187 reached for and 1199 corrected in the
        // binding without correcting it here. It passed anyway, because 5.1's string metatable
        // made indexing the `texture` string a silent `nil` and `nil` is a passing `!bool`. Taking
        // the metatable away — 1.12 has none — is what turned a test that asserted nothing into a
        // test that raises.
        assert!(!s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
            .unwrap());
    }

    /// The sibling of the test above onto an OCCUPIED, DIFFERENT-item slot: the plain SWAP
    /// (superseding 0216 §2's hop — byte-verified: no `SetCursorItem` on the place
    /// branch; the wire swaps and the cursor empties, exactly like an empty destination). The
    /// displaced item never lands on the cursor.
    #[test]
    fn pickup_place_onto_occupied_different_item_swaps_and_clears() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack(); // slot 1 = item 117 (A)
        state.slots.insert(
            5,
            ContainerSlot {
                duration_ms: None,
                petition: None,
                already_bound: false,
                bar_placeable: true,
                durability: None,
                texture: Some("Interface\\Icons\\INV_Misc_Gem_01".into()),
                count: 1,
                quality: Some(2),
                item_id: 200, // item B, a different id
                link: Some("|Hitem:200|h[Shiny Gem]|h".into()),
                locked: false,
                equip_slots: Vec::new(),
                cooldown: None,
                readable: false,
                creator: None,
                flags: 0,
                enchants: Vec::new(),
            },
        );
        s.set_container(0, Some(state));

        assert!(s.eval::<bool>("return PickupContainerItem(0, 1)").unwrap());
        assert_eq!(s.cursor_item().unwrap().item_id, 117);

        // Place A onto occupied slot 5 (item B) → move (1→5) queues, the cursor EMPTIES (the
        // server's swap lands B in slot 1), and the source un-locks.
        assert!(s.eval::<bool>("return PickupContainerItem(0, 5)").unwrap());
        assert!(
            s.cursor_item().is_none(),
            "a swap clears the cursor — the displaced item never hops on"
        );
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 5,
                count: None,
            }]
        );
        // The 1.12 five-value shape — see the sibling test above for what this used to read and
        // why it passed while asserting nothing (2171).
        assert!(!s
            .eval::<bool>("local _, _, locked = GetContainerItemInfo(0, 1) return locked")
            .unwrap());
    }

    /// Placing onto a SAME-item_id destination merges (the wire tops the stack up itself) —
    /// still a plain clear-on-move.
    #[test]
    fn pickup_place_onto_same_item_merges_and_clears() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack();
        state.slots.insert(
            5,
            ContainerSlot {
                durability: None,
                item_id: 117, // same id as slot 1
                count: 2,
                ..Default::default()
            },
        );
        s.set_container(0, Some(state));

        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.eval::<bool>("return PickupContainerItem(0, 5)").unwrap());
        assert!(
            s.cursor_item().is_none(),
            "same-item merge clears the cursor"
        );
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 5,
                count: None,
            }]
        );
    }

    #[test]
    fn pickup_refuses_a_locked_slot() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack();
        state.slots.get_mut(&1).unwrap().locked = true;
        s.set_container(0, Some(state));

        assert!(!s.eval::<bool>("return PickupContainerItem(0, 1)").unwrap());
        assert!(s.cursor_item().is_none());
    }

    #[test]
    fn split_container_item_carries_a_partial_stack() {
        let mut s = UiScript::new().unwrap();
        let mut state = backpack(); // slot 1: a 5-stack of item 117
        state.slots.insert(
            7,
            ContainerSlot {
                durability: None,
                item_id: 117,
                count: 1,
                ..Default::default()
            },
        );
        state.slots.insert(
            9,
            ContainerSlot {
                durability: None,
                item_id: 999, // a different item
                count: 1,
                ..Default::default()
            },
        );
        s.set_container(0, Some(state));

        assert!(s
            .eval::<bool>("return SplitContainerItem(0, 1, 3)")
            .unwrap());
        let held = s.cursor_item().expect("the split carry");
        assert_eq!(
            (held.bag, held.slot, held.item_id, held.count),
            (0, 1, 117, Some(3))
        );

        // Placed onto an empty slot: the move carries the split count, and clears.
        assert!(s.eval::<bool>("return PickupContainerItem(0, 2)").unwrap());
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 2,
                count: Some(3),
            }]
        );

        // Re-split, then place onto a DIFFERENT item: no-op, the split carry is kept.
        s.run("SplitContainerItem(0, 1, 3)").unwrap();
        assert!(!s.eval::<bool>("return PickupContainerItem(0, 9)").unwrap());
        let held = s.cursor_item().expect("kept — can't swap a partial stack");
        assert_eq!(held.count, Some(3));
        assert!(s.take_container_moves().is_empty());

        // Placed onto a SAME-item slot: the server merges — the move queues, cursor clears.
        assert!(s.eval::<bool>("return PickupContainerItem(0, 7)").unwrap());
        assert!(s.cursor_item().is_none());
        assert_eq!(
            s.take_container_moves(),
            vec![ContainerMove {
                src_bag: 0,
                src_slot: 1,
                dst_bag: 0,
                dst_slot: 7,
                count: Some(3),
            }]
        );
    }

    #[test]
    fn split_of_the_whole_stack_is_a_plain_pickup() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack())); // slot 1: a 5-stack
        assert!(s
            .eval::<bool>("return SplitContainerItem(0, 1, 5)")
            .unwrap());
        assert_eq!(s.cursor_item().unwrap().count, None);
        // Split of MORE than the stack clamps the same way.
        s.run("ClearCursor()").unwrap();
        assert!(s
            .eval::<bool>("return SplitContainerItem(0, 1, 99)")
            .unwrap());
        assert_eq!(s.cursor_item().unwrap().count, None);
    }

    #[test]
    fn pickup_same_slot_cancels_no_move() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        assert!(s.eval::<bool>("return PickupContainerItem(0, 1)").unwrap());
        assert!(s.cursor_item().is_some());
        // Click the same slot again → cancel: cursor clears, nothing queued.
        assert!(s.eval::<bool>("return PickupContainerItem(0, 1)").unwrap());
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
    }

    #[test]
    fn pickup_empty_slot_holds_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        // Slot 2 is empty, slot 3 is in-flight (unresolved, item_id 0) — neither is pickable.
        assert!(!s.eval::<bool>("return PickupContainerItem(0, 2)").unwrap());
        assert!(s.cursor_item().is_none());
        assert!(!s.eval::<bool>("return PickupContainerItem(0, 3)").unwrap());
        assert!(s.cursor_item().is_none());
    }

    #[test]
    fn clear_cursor_drops_the_held_item() {
        let mut s = UiScript::new().unwrap();
        s.set_container(0, Some(backpack()));
        s.run("PickupContainerItem(0, 1)").unwrap();
        assert!(s.cursor_item().is_some());
        s.run("ClearCursor()").unwrap();
        assert!(s.cursor_item().is_none());
        assert!(s.take_container_moves().is_empty());
    }

    #[test]
    fn removing_a_bag_empties_it() {
        let mut s = UiScript::new().unwrap();
        s.set_container(2, Some(backpack()));
        s.set_container(2, None);
        assert_eq!(s.eval::<i64>("return GetContainerNumSlots(2)").unwrap(), 0);
    }

    /// `GetContainerItemCooldown` — the `GetActionCooldown` conventions on the bag twin: the
    /// absolute-start triple stored verbatim in `GetTime` seconds, the cold `(0, 0, 1)` shape
    /// for no-cooldown slots, the stale-refeed guard at expiry, and the re-push replacing a
    /// bag's prior triples.
    #[test]
    fn container_item_cooldown_reads_the_gettime_triple() {
        let mut s = UiScript::new().unwrap();
        s.tick(100.0); // an arbitrary clock epoch
        let mut state = backpack();
        // Slot 1's item is 4 s into a 10 s use-cooldown: started at GetTime 96.
        state.slots.get_mut(&1).unwrap().cooldown = Some((96_000, 10_000, true));
        s.set_container(0, Some(state));

        // The pushed absolute start reads back verbatim in seconds.
        assert!(s
            .eval::<bool>(
                "local s, d, e = GetContainerItemCooldown(0, 1)\n\
                 return s == 96 and d == 10 and e == 1"
            )
            .unwrap());
        // A slot with no cooldown reads cold.
        assert!(s
            .eval::<bool>(
                "local s, d, e = GetContainerItemCooldown(0, 3)\n\
                 return s == 0 and d == 0 and e == 1"
            )
            .unwrap());
        // Past expiry, the same stored triple reads cold — the no-replayed-flash guard.
        s.tick(7.0);
        assert!(s
            .eval::<bool>(
                "local s, d, e = GetContainerItemCooldown(0, 1)\n\
                 return s == 0 and d == 0 and e == 1"
            )
            .unwrap());
        // A re-push without the cooldown clears the stored triple.
        s.set_container(0, Some(backpack()));
        assert!(s
            .eval::<bool>(
                "local s, d = GetContainerItemCooldown(0, 1)\n\
                 return s == 0 and d == 0"
            )
            .unwrap());
    }

    // ── `ContainerIDToInventoryID` (`0x4f94e0`) ─────────────────────────────────────

    /// Two linear arms and no range check. **−2 → 17 and 0 → 19 are not special cases** — they are
    /// ordinary points on the `id <= 4` line that the caller-side "keyring is −2 / backpack is 0"
    /// convention happens to land on, which is why they are asserted beside the bag slots rather
    /// than as exceptions.
    #[test]
    fn container_id_to_inventory_id_is_two_lines_with_no_clamp() {
        let s = UiScript::new().unwrap();
        let slot = |id: &str| {
            s.eval::<i64>(&format!("return ContainerIDToInventoryID({id})"))
                .unwrap()
        };
        // The `id <= 4` line: keyring, the odd −1, the backpack, the four worn bags.
        assert_eq!(slot("-2"), 17, "keyring — an ordinary point, not a case");
        assert_eq!(slot("-1"), 18);
        assert_eq!(slot("0"), 19, "backpack — likewise");
        assert_eq!(
            (slot("1"), slot("2"), slot("3"), slot("4")),
            (20, 21, 22, 23)
        );
        // The `id >= 5` line: the bank bags.
        assert_eq!(slot("5"), 64, "the jump — +59, not +19");
        assert_eq!((slot("6"), slot("10")), (65, 69));
        // No range check, no clamp, and never nil: out of range is an ordinary number.
        assert_eq!(
            slot("99"),
            158,
            "the value `SetInventoryItem` would receive"
        );
        assert_eq!(slot("-100"), -81, "negatives run off the line too");
        assert_eq!(
            s.arity("ContainerIDToInventoryID(99)").unwrap(),
            1,
            "one value on every non-raising path"
        );
        // `0x40a2b0` truncates toward zero — a C cast, not `floor`.
        assert_eq!(slot("2.9"), 21, "2.9 -> 2");
        assert_eq!(slot("-2.9"), 17, "-2.9 -> -2, NOT -3");
    }

    /// A missing or non-number argument **raises** — the only guard in the function is the type
    /// test, and `0x6f4940` does not return.
    #[test]
    fn container_id_to_inventory_id_raises_on_a_bad_argument() {
        let s = UiScript::new().unwrap();
        for call in ["ContainerIDToInventoryID()", "ContainerIDToInventoryID({})"] {
            let err = s
                .eval::<mlua::Value>(&format!("return {call}"))
                .unwrap_err();
            assert!(
                format!("{err}").contains("Usage: ContainerIDToInventoryID(containerID)"),
                "{call} must raise, got {err}"
            );
        }
    }
}
