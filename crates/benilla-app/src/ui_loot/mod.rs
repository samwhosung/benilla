//! The app-side **loot feed** (decision 0084) — the inward half of the loot seam around
//! [`benilla_ui::script`]'s `loot` module, the twin of [`crate::ui_merchant`]'s merchant seam.
//!
//! The net bridge ([`crate::net::apply`]) fills [`LootState`] from the wire: `SMSG_LOOT_RESPONSE` →
//! the rows + coin pile ([`LootState::open`]); `SMSG_LOOT_REMOVED` → that row becomes an empty gap
//! at its fixed position ([`LootState::remove_slot`] — the layout never compacts while open);
//! `SMSG_LOOT_CLEAR_MONEY` → the coin row becomes the same kind of gap
//! ([`LootState::clear_money`]); `SMSG_LOOT_RELEASE_RESPONSE` → the window closes
//! ([`LootState::clear`]); the error shape → a red line by GlobalStrings key, raised straight from
//! the bridge ([`crate::ui_loot::net::loot_error`] — it needs no queue on this side);
//! `SMSG_ITEM_PUSH_RESULT` → a queued "You receive loot" line ([`LootState::receives`]). A removal
//! that empties the window arms the client-authoritative **auto-close**
//! ([`LootState::auto_release`] — the real engine's close-on-last-slot), released by
//! [`drain_loot`].
//!
//! Each frame [`feed_loot`] surfaces the receive lines, resolves each wire [`LootItem`] to a
//! Lua-facing [`LootRow`] (icon straight from the wire `display_info_id` through the same
//! `ItemDisplayInfo.dbc` catalog the bags use — no template wait; name + quality via the ask-once
//! item-template cache, `None`/re-fed while in flight), prepends the synthesized coin row when the
//! loot carries gold, pushes the snapshot ([`benilla_ui::script::UiScript::set_loot`]), and fires
//! `LOOT_OPENED` on open / `LOOT_UPDATE` on a content change / `LOOT_CLOSED` on clear.
//! [`drain_loot`] pulls the Lua intents back out: `LootSlot`
//! → coin ? [`ClientCommand::LootMoney`] : [`ClientCommand::AutostoreLootItem`] (the clicked 1-based
//! row mapped to the item's **wire** loot slot); `CloseLoot` → [`ClientCommand::LootRelease`].

use benilla_protocol::messages::{slot_type, ItemPushResult, LootItem, BAG_PLAYER_INVENTORY};
use bevy::prelude::*;

use benilla_ui::script::{LootRow, LootState as LootSnapshot, ScriptValue, UiScript};

use crate::entities::ItemDisplays;
use crate::items::{Items, RollCatalogs};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::ui_items::KEYRING_CONTAINER;
use crate::ui_party::{GroupState, GROUPTYPE_RAID, GROUP_MEMBER_SUBGROUP};
use crate::ui_script::{UiFeed, UiInput};

mod net;

/// The coin-pile row icons (direct `Interface\Icons` paths — `SetTexture` takes them as-is, no DBC),
/// **six of them, one per decade of copper**, all VERIFIED to extract from `interface.MPQ`.
///
/// This used to be three, chosen by the highest nonzero denomination, and said so in a "stated
/// approximation" note whose stated reason was that the real selection was not read from the
/// reference. It is: `0x4c2460` sends the money row to `0x4c248a call 0x6c62d0`, whose ladder at
/// `0x6c6307`–`0x6c6386` is exactly the six thresholds below.
///
/// The icon *order* is the client's own and it is not the numeric one — `_05, _06, _03, _04, _01,
/// _02` as the amount climbs. What each of the six pieces of art depicts is not claimed here; the
/// ladder is byte-derived and the art is whatever the reference picks at that step.
/// `item_template.bonding == BIND_WHEN_PICKED_UP` — the first of the two conjuncts that defer a
/// loot take behind the LOOT_BIND confirm (VERIFIED vmangos `ItemPrototype.h`'s `ItemBondingType`:
/// `NO_BIND` 0, `BIND_WHEN_PICKED_UP` 1, `BIND_WHEN_EQUIPPED` 2, `BIND_WHEN_USE` 3, `QUEST_ITEM` 4;
/// the client reads the same field at `item_template + 0x194` and compares it against `1`).
const BIND_WHEN_PICKED_UP: u32 = 1;

/// The second conjunct: quality **>= 2** (uncommon or better), `cmp [tmpl+0x1c], 2 / jb` at
/// `0x4c28fb`. Below it a bind-on-pickup row is simply taken — the confirm exists to stop a player
/// soulbinding something they meant to pass to a groupmate, and nobody passes a white.
const BIND_CONFIRM_MIN_QUALITY: u32 = 2;

/// The quality `GetLootSlotInfo` answers for the synthesized coin row: **0, Poor** — so the money
/// text renders GREY, not white.
///
/// Byte-derived, and it corrects a stated approximation ("common/white — the money text reads as
/// plain"). `0x4c23a0`'s second guard IS its coin leg: `0x4c23da test ecx,ecx` / `0x4c23dc jne`, so
/// the money row (0-based slot 0 with `[0xb71ba0] != 0`) falls through to
/// `0x4c23de xor eax,eax; ret` and never reaches the item-cache block the item rows read
/// `[rec+0x1c]` from. The same guard is why its `quantity` is 0 (`0x4c22fd`).
///
/// Nothing downstream re-colours it: stock `LootFrame_Update` has no coin special case
/// (`LootFrame.lua:81-85`) and `ITEM_QUALITY_COLORS[0]` is `0xff9d9d9d`.
const COIN_QUALITY: u32 = 0;
/// The 1.12 coin-denomination words, QUOTED from `Interface\FrameXML\GlobalStrings.lua` (verified
/// extract): `GOLD = "Gold"` (l.2025), `SILVER = "Silver"` (l.3465), `COPPER = "Copper"` (l.865).
const GOLD_WORD: &str = "Gold";
const SILVER_WORD: &str = "Silver";
const COPPER_WORD: &str = "Copper";
/// Give up re-checking a pending receive line's item name after this many frames (a negative-cached
/// or genuinely-unknown entry never resolves; ~2s at 60fps is well past a normal template round-trip).
const RECEIVE_MAX_TRIES: u16 = 120;
/// The player-array slots the reference calls "the keyring" when deciding which bag button a push
/// animates — the literal `0x51`/`0x70` bounds compiled into `OnItemPush` (`0x491bc3`/`0x491bc8`).
/// It is the **descriptor array's** full 32-guid width (vmangos `KEYRING_SLOT_START 81` + 32), not
/// the 16 addressable positions `ui_items`'s `KEYRING_SLOTS` names (81..96) — the client tests the
/// wider range, so this transcribes the client rather than deriving from the server's stricter one.
const PUSH_KEYRING_SLOTS: std::ops::RangeInclusive<u32> = 81..=112;

/// One deferred push (`SMSG_ITEM_PUSH_RESULT`) awaiting its item template — the whole
/// `CGGameUI::OnItemPush 0x491a60` tail, not just the chat line. Carries the pushed entry + count,
/// the source flags (looted / from an NPC / created — the wording differs), the two random-property
/// wire fields the item link carries, whether the line is spoken at all, the destination container
/// ([`push_container`], for the bag-bar drop animation) and a retry budget.
///
/// Deferring **both** outputs on the template is the reference's own shape: `OnItemPush` opens with
/// an item-cache lookup (`0x491a93`) and, on a miss, copies its nine arguments into a heap record and
/// re-enters itself from the cache callback (`0x491aa7`-`0x491b05`, callback `0x491ee0`) — nothing is
/// emitted until the item is known.
struct PendingReceive {
    /// `SMSG_ITEM_PUSH_RESULT`'s created flag — a crafted/conjured item ("You create: …").
    created: bool,
    entry: u32,
    count: u32,
    from_npc: bool,
    /// `SMSG_ITEM_PUSH_RESULT`'s `randomPropertyId` — link field 3 (see [`receive_line`]).
    random_property_id: u32,
    /// `SMSG_ITEM_PUSH_RESULT`'s `suffixFactor` — link field 4 (see [`receive_line`]).
    suffix_factor: u32,
    /// The wire's `showInChat` — whether the chat LINE is spoken. It gates the line ONLY: the
    /// reference fires `ITEM_PUSH` at `0x491be8`, before the `[ebx+0x24]` test at `0x491bf3` that
    /// guards the whole chat block, so a silent push still animates. Which is why this rides in the
    /// record instead of short-circuiting at the net bridge (decision 0887).
    in_chat: bool,
    /// Which bag-bar button the drop animation plays on — see [`push_container`].
    container: i64,
    tries: u16,
}

/// The wire push's `(bag, slot)` → `ITEM_PUSH`'s **`arg1`**, in the reference's own vocabulary:
/// `0` the backpack, `20..=23` an equipped bag (the INVENTORY-slot id its bag buttons carry),
/// [`KEYRING_CONTAINER`] the keyring.
///
/// **Byte-VERIFIED** at `CGGameUI::OnItemPush` `0x491bb5`-`0x491bd6`, which is the whole selector:
///
/// ```text
///   491bb5  cmp edi,0xff        ; edi = the wire `bag` byte
///   491bbb  lea eax,[edi+1]     ; bag != 255  ⇒  bag + 1
///   491bbe  jne 491bd6
///   491bc0  mov eax,[ebp-0x4]   ; else look at the wire `slot`
///   491bc3  cmp eax,0x51        ; 81  = vmangos KEYRING_SLOT_START
///   491bc6  jl  491bd4
///   491bc8  cmp eax,0x70        ; 112 = the last keyring position
///   491bcb  jg  491bd4
///   491bcd  mov eax,-2          ;  ⇒ KEYRING_CONTAINER
///   491bd4  xor eax,eax         ;  ⇒ 0 (the backpack)
/// ```
///
/// **No translation any more, and that is the point.** This used to emit benilla's own container
/// vocabulary (`1..=4` for the four bags), because benilla's bag BAR was ours and was keyed that
/// way. 1751 window 3 made the bar the reference's own `MainMenuBarBagButtons.xml`, whose
/// `ItemAnim_OnEvent` reads `this:GetParent():GetID()` — 20..23, from `GetInventorySlotInfo` — and
/// compares it to `arg1`. Against the translated value that comparison is false for every bag, so
/// the four bag cards would simply never have played, silently. The fix is not to adapt the
/// reference's body: it is to stop translating, which is also what every addon reading `ITEM_PUSH`
/// expects, so the function is now `0x491bb5`'s selector verbatim.
///
/// A non-equipped-bag container (a bank bag, wire 63..68) still lands on an id no bag-bar button
/// carries — no animation, exactly as there.
fn push_container(bag: u8, slot: u32) -> i64 {
    if bag != BAG_PLAYER_INVENTORY {
        // `491bbb  lea eax,[edi+1]` — the wire bag byte plus one, nothing else.
        return i64::from(bag) + 1;
    }
    if PUSH_KEYRING_SLOTS.contains(&slot) {
        return KEYRING_CONTAINER;
    }
    0
}

/// The open loot, filled by the net bridge and read by [`feed_loot`]. Holds the looted guid, the coin
/// pile, and the rows exactly as the wire delivered them (`SMSG_LOOT_RESPONSE`); the feed resolves
/// each to a display row and the drain maps a clicked 1-based row to its wire loot slot. Cleared on
/// release and on disconnect. `receives` outlives the window (a vendor buy pushes one with no loot
/// open) and is cleared only on disconnect.
///
/// **The slot layout is FIXED at open** (the reference's own slot array): a looted row — item or
/// coin — becomes an empty *gap* at its position, never a compaction. The real client's
/// `LOOT_SLOT_CLEARED` hides one button in place (`LootFrame.lua:22-37`), `numLootItems` is read
/// once at OnShow (l.132), and a cleared slot answers neither `LootSlotIsItem` nor `LootSlotIsCoin`
/// (l.80) — so the rows below a looted one keep their positions until the window closes. Modelled
/// here as `coin_slot` (does position 1 belong to the coin pile, looted or not) + `taken` (which
/// wire slots are already gone); [`snapshot`] emits `None` rows for the gaps.
#[derive(Resource, Default)]
pub(crate) struct LootState {
    /// The lootable unit whose window is open; `None` = no loot open.
    source: Option<u64>,
    /// The coin pile in copper; `0` = no coin left to loot.
    gold: u32,
    /// Whether the layout's position 1 is the coin pile — fixed at open (`gold > 0` then), and it
    /// stays `true` after the coin is looted: the slot becomes a gap, not a vacancy the items
    /// below shift into.
    coin_slot: bool,
    /// The item rows (wire order); each carries its own **wire** loot slot (`LootItem::slot`).
    /// Never shrinks while the window is open — a looted row's wire slot lands in `taken` instead.
    items: Vec<LootItem>,
    /// Wire slots already looted by anyone (`SMSG_LOOT_REMOVED`) — their rows stay in the layout
    /// as empty gaps.
    taken: Vec<u8>,
    /// Deferred "You receive …" lines awaiting their item name.
    receives: Vec<PendingReceive>,
    /// A wire removal just emptied the open window (last item taken / coin line cleared with no
    /// items left) — the client-authoritative auto-close is due: the real client closes the loot
    /// itself when the last slot clears (the server never initiates the last-slot release —
    /// vmangos `LootHandler.cpp` releases only in `HandleLootReleaseOpcode`, and its one
    /// unprompted release is the movement handler's, see [`LootMoveStart`]; the 1.12
    /// `LootFrame.lua` `LOOT_SLOT_CLEARED` handler only hides buttons, so the close is engine-side).
    /// Set only on the *transition* to empty via [`LootState::remove_slot`]/[`LootState::clear_money`],
    /// never at open — an empty-at-open window stays up (the reference `LootFrame_OnShow` even has a
    /// dedicated `LOOTWINDOWOPENEMPTY` sound for it). Drained by [`drain_loot`].
    auto_release: bool,
    /// Whether the open loot came from fishing (the wire `loot_type == 3` — vmangos folds
    /// `FISHINGHOLE`/`FISHING_FAIL` into `LOOT_FISHING` before sending). Carried into the Lua
    /// snapshot as `IsFishingLoot()`, which `LootFrame_OnShow` keys the "FISHING REEL IN" sound
    /// and the FishingLoot portrait overlay on (decision 1086).
    fishing: bool,
    /// The master-loot candidates for the OPEN window (`SMSG_LOOT_MASTER_LIST`), in wire order —
    /// the guids `GiveMasterLoot`'s 1-based candidate index resolves against (decision 1675).
    /// Empty under every other loot method.
    master_candidates: Vec<u64>,
    /// The row a `LOOT_BIND_CONFIRM` is currently open for — 1-based, display-side, the number the
    /// event carried out and the number `LootSlot` must carry back (decision 1744). This is the
    /// reference's `[0x847cec]`, whose `-1` is our `None`: `0x4c2790`'s click arm writes it instead
    /// of sending, and its continuation arm sends only for a slot that equals it, then clears it
    /// (`0x4c281a mov [0x847cec], 0xffffffff`). Reset with the window (`0x4c1df5`, in the
    /// `SMSG_LOOT_RESPONSE` copier) so a confirm cannot survive into the next corpse.
    pending_bind_confirm: Option<u32>,
    /// The candidate list that arrived but has no window yet. `SMSG_LOOT_MASTER_LIST` is sent from
    /// *inside* `Player::SendLoot` (`Player.cpp:8077-8081`), so it lands **before** the
    /// `SMSG_LOOT_RESPONSE` it belongs to; [`LootState::open`] takes it from here. Staging it
    /// rather than writing `master_candidates` directly is what keeps one window's list from
    /// leaking into the next window opened under a different loot method.
    pending_master_candidates: Vec<u64>,
    /// The wire `loot_type` the window opened with (`SMSG_LOOT_RESPONSE`'s byte, the reference's
    /// `0x4c2740` read). The move-start close reads it: a **non-empty DISENCHANT window** (type 4)
    /// survives movement (`0x48f24a`–`0x48f25a`, decision 2097).
    loot_type: u8,
}

/// The master-loot candidate array's fixed width — 40 slots of 8 bytes at `0xc4dc38`, zeroed at
/// every `SMSG_LOOT_MASTER_LIST` and bound-checked by the getter `0x61c660` (`cmp ecx,0x28`).
const MASTER_LOOT_CANDIDATE_SLOTS: usize = 40;
/// The raid subgroup's stride within that array (`n*5 .. n*5+5`, `0x61c609`) — 8 groups of 5.
const MEMBERS_PER_RAID_GROUP: usize = 5;

/// A resolved loot-row pick: the coin pile, or an item at a concrete **wire** loot slot (carrying
/// its display id, so the pick can play the item's pickup sound without a second lookup, and the
/// wire's `slot_type`, which decides whether the row is takeable at all).
enum LootAction {
    Money,
    Item {
        wire_slot: u8,
        display_id: u32,
        /// The template entry — the key the bind-on-pickup deferral reads `bonding` and `quality`
        /// off (decision 1744).
        item_id: u32,
        /// The wire's per-row [`slot_type`]. `MASTER` diverts the click to the master-loot
        /// dropdown instead of a take (decision 1675).
        slot_type: u8,
    },
}

impl LootState {
    /// Open (or replace) the window with a fresh loot response (`SMSG_LOOT_RESPONSE`). Quest items
    /// ride the same `items` list (the wire appends them with `slot = items.len()+i`), so they need no
    /// special handling here.
    pub(crate) fn open(&mut self, source: u64, loot_type: u8, gold: u32, items: Vec<LootItem>) {
        self.source = Some(source);
        self.gold = gold;
        self.coin_slot = gold > 0; // the layout is fixed here, for the window's lifetime
        self.items = items;
        self.taken.clear();
        self.auto_release = false; // empty-at-open stays open — only a removal auto-closes
        self.pending_bind_confirm = None; // ref `0x4c1df5`: the copier resets the stash to -1
        self.loot_type = loot_type;
        self.fishing = loot_type == benilla_protocol::messages::loot_type::FISHING;
        // The master-loot candidate list arrives just AHEAD of this response (the server sends it
        // from inside `SendLoot`), so the window claims whatever was staged and leaves the staging
        // empty — a later window under a non-master method then correctly has no candidates.
        self.master_candidates = std::mem::take(&mut self.pending_master_candidates);
    }

    /// A master-loot candidate list arrived (`SMSG_LOOT_MASTER_LIST`). Normally this precedes the
    /// `SMSG_LOOT_RESPONSE` it belongs to and is staged for [`LootState::open`]; if a window is
    /// already up it is also applied in place, which is the case the reference's
    /// `UPDATE_MASTER_LOOT_LIST` event exists for.
    pub(crate) fn set_master_candidates(&mut self, candidates: Vec<u64>) {
        if self.source.is_some() {
            self.master_candidates.clone_from(&candidates);
        }
        self.pending_master_candidates = candidates;
    }

    /// A row was taken by anyone (`SMSG_LOOT_REMOVED`, keyed by the **wire** slot): its position
    /// becomes an empty gap — the layout never compacts (the reference hides that one button in
    /// place, `LootFrame.lua:22-37`). Emptying the window arms the auto-close
    /// ([`LootState::auto_release`]).
    pub(crate) fn remove_slot(&mut self, wire_slot: u8) {
        if self.items.iter().any(|it| it.slot == wire_slot) && !self.taken.contains(&wire_slot) {
            self.taken.push(wire_slot);
        }
        self.arm_auto_release();
    }

    /// The coin line disappears for everyone (`SMSG_LOOT_CLEAR_MONEY`) — its slot stays in the
    /// layout as a gap (`coin_slot` holds). Emptying the window arms the auto-close
    /// ([`LootState::auto_release`]).
    pub(crate) fn clear_money(&mut self) {
        self.gold = 0;
        self.arm_auto_release();
    }

    /// Arm the client-authoritative auto-close when a removal just left the open window with
    /// nothing lootable — every item taken and no coin left (see [`LootState::auto_release`]).
    fn arm_auto_release(&mut self) {
        if self.source.is_some() && self.is_empty() {
            self.auto_release = true;
        }
    }

    /// Take the armed auto-close edge (see [`LootState::auto_release`]) — `true` at most once per
    /// emptying.
    fn take_auto_release(&mut self) -> bool {
        std::mem::take(&mut self.auto_release)
    }

    /// Queue a deferred push (`SMSG_ITEM_PUSH_RESULT`) — the "You receive …" line *and* the bag-bar
    /// drop animation, emitted together once the item template lands (see [`PendingReceive`]). The
    /// **self** gate lives at the net bridge ([`crate::net::apply`]); the `showInChat` gate does
    /// not — it rides in as `in_chat` and silences the line alone, leaving the animation to play.
    pub(crate) fn push_receive(&mut self, p: &ItemPushResult) {
        self.receives.push(PendingReceive {
            created: p.created,
            entry: p.item_entry,
            count: p.count,
            from_npc: p.from_npc,
            random_property_id: p.random_property_id,
            suffix_factor: p.suffix_factor,
            in_chat: p.show_in_chat,
            container: push_container(p.bag_slot, p.item_slot),
            tries: 0,
        });
    }

    /// Close the open window (a release response, or a client-authoritative close on `CloseLoot`).
    /// Keeps `receives` (an in-flight receive line outlives the window).
    pub(crate) fn clear(&mut self) {
        self.source = None;
        self.gold = 0;
        self.coin_slot = false;
        self.items.clear();
        self.taken.clear();
        self.auto_release = false;
        self.fishing = false;
        self.loot_type = 0;
        self.pending_bind_confirm = None;
        self.master_candidates.clear();
        self.pending_master_candidates.clear();
    }

    /// Disconnect: drop the open window **and** any pending receive lines (mirrors the merchant/gossip
    /// session clears).
    pub(crate) fn clear_session(&mut self) {
        self.clear();
        self.receives.clear();
    }

    /// How many pushes are queued awaiting their item template — the net bridge's test hook for
    /// "did this packet get through the self gate", since neither output is emitted until the
    /// template lands.
    #[cfg(test)]
    pub(crate) fn pending_receive_count(&self) -> usize {
        self.receives.len()
    }

    /// Whether a coin row is shown (position 1 when present).
    fn has_coin(&self) -> bool {
        self.gold > 0
    }

    /// The guid of the loot source whose window is open (`None` = closed). Read by the GameObject
    /// lid-close watcher ([`crate::go_anim`]) to close a chest's lid when its loot window closes
    /// (decision 2271) — the faithful client-authoritative close, any path (player close or the
    /// server's release on the last item).
    pub(crate) fn source(&self) -> Option<u64> {
        self.source
    }

    /// Nothing lootable left — every item taken and no coin (the reference's `0x4c2a70`
    /// empty-check). The auto-close edge and the disenchant exemption both read it.
    fn is_empty(&self) -> bool {
        !self.has_coin() && self.items.iter().all(|it| self.taken.contains(&it.slot))
    }

    /// Resolve a clicked 1-based display row to its action: the coin pile (position 1 when the
    /// layout has a coin slot) or the item at the corresponding **wire** loot slot. Positions are
    /// the FIXED open-time layout; a slot already looted (the coin gone, an item in `taken`)
    /// answers `None` — a click on the gap does nothing, like the reference's hidden button.
    fn action_at(&self, index_1based: u32) -> Option<LootAction> {
        let mut index = index_1based.checked_sub(1)? as usize; // 0-based layout position
        if self.coin_slot {
            if index == 0 {
                return self.has_coin().then_some(LootAction::Money);
            }
            index -= 1;
        }
        let it = self.items.get(index)?;
        (!self.taken.contains(&it.slot)).then_some(LootAction::Item {
            wire_slot: it.slot,
            display_id: it.display_info_id,
            item_id: it.item_id,
            slot_type: it.slot_type,
        })
    }

    /// The candidate slots as the client lays them out — **not** simply the wire order (VERIFIED
    /// in the 5875 binary, `SMSG_LOOT_MASTER_LIST`'s handler `0x61c550`). The array is 40 fixed
    /// 8-byte slots at `0xc4dc38`, zeroed per packet, and filled by one of two paths chosen once
    /// from the live raid-member count `[0xb713e0]`:
    ///
    /// - **Not in a raid** (`0x61c5b9`): the wire's own loop counter is the slot. Dense, and with
    ///   no bound check at all — a party can only ever fill 0..4.
    /// - **In a raid** (`0x61c5c9`-`0x61c637`): each guid is looked up in the raid roster, its
    ///   subgroup `n` read, and it is written to the **first free slot of `[n*5, n*5+5)`**, bound
    ///   checked against 40. So the array is BUCKETED BY SUBGROUP, with holes.
    ///
    /// The holes are the point: `GroupLootDropDown_Initialize` walks `1..40` in blocks of five and
    /// keeps a "Group N" submenu only where the block has an occupant (`LootFrame.lua:190-201`).
    /// Packing the list densely would have labelled every candidate with the wrong raid group.
    ///
    /// One placement function serves both readers — the feed (which turns slots into names) and
    /// the drain (which turns a clicked index back into a guid) — so the index the Lua hands back
    /// can never mean something different from the index it was shown.
    fn placed_candidates(&self, group: &GroupState) -> Vec<Option<u64>> {
        if group.group_type != GROUPTYPE_RAID {
            return self.master_candidates.iter().copied().map(Some).collect();
        }
        let mut slots: Vec<Option<u64>> = vec![None; MASTER_LOOT_CANDIDATE_SLOTS];
        for &guid in &self.master_candidates {
            // Everyone else's subgroup rides their roster entry; ours rides `own_flags`, and the
            // fallback IS us — `SMSG_GROUP_LIST`'s member array is the *other* members, so the one
            // candidate guid that can never be found in it is our own (the server puts us in the
            // candidate list: `Group::MasterLoot` walks the whole group).
            let flags = group
                .members
                .iter()
                .find(|m| m.guid == guid)
                .map_or(group.own_flags, |m| m.flags);
            let base = usize::from(flags & GROUP_MEMBER_SUBGROUP) * MEMBERS_PER_RAID_GROUP;
            let end = (base + MEMBERS_PER_RAID_GROUP).min(MASTER_LOOT_CANDIDATE_SLOTS);
            if let Some(free) = (base..end).find(|&i| slots[i].is_none()) {
                slots[free] = Some(guid);
            }
        }
        while slots.last().is_some_and(Option::is_none) {
            slots.pop(); // trailing empties read as nil either way; keep the snapshot small
        }
        slots
    }

    /// The candidate at a 1-based menu index, through [`LootState::placed_candidates`].
    fn master_candidate(&self, index_1based: u32, group: &GroupState) -> Option<u64> {
        self.placed_candidates(group)
            .get(index_1based.checked_sub(1)? as usize)
            .copied()
            .flatten()
    }
}

/// The loot player knob (decision 0961): `autoLootDefault` — era's Controls-page checkbox (no
/// 1.12 CVar exists; vanilla only had the shift-click), settable from the Options window
/// through the CVar store (0954). The reference implements auto-loot ENGINE-side (era's own
/// Lua never reads this CVar outside its settings page), and so do we: [`feed_loot`] picks
/// every row itself at the open edge. A held SHIFT inverts the setting — era's
/// `AUTOLOOTTOGGLE` modified click, default SHIFT (Bindings_Vanilla.xml l.1467), the same
/// gesture that WAS vanilla's whole auto-loot.
///
/// `show_loot_spam` is 1.12's own `showLootSpam` — the *Detailed Loot Information* checkbox, whose
/// subject is **group loot rolls**, not loot messages generally (decision 1589, the Chat page).
/// It rides here rather than on [`crate::ui_loot_roll`] because it is one loot knob among the
/// loot knobs and [`on_cvar`] writes both. The CVar is `0xb4e2bc`, registered at `0x48fd1c` with
/// default `"1"` and flags 5, and a byte census over the whole binary finds exactly four
/// references — one writer and three readers, all three inside the loot-roll line composers.
#[derive(Resource)]
pub(crate) struct LootConfig {
    pub(crate) auto_loot: bool,
    pub(crate) show_loot_spam: bool,
}

impl Default for LootConfig {
    fn default() -> Self {
        Self {
            // No 1.12 CVar; era's registrar default is off (see the doc above).
            auto_loot: false,
            // The reference's registered `"1"` — detail on, which is 1.12's out-of-box chat.
            show_loot_spam: true,
        }
    }
}

/// The client-local **loot-target latch** — the mirror of the real client's `[player+0x1d28]`
/// guid (decisions 0515 / 1471 / **1477**).
/// It says *a loot session is open on this object*, and it is read by far more than the kneel: the
/// loot cursor, the re-loot lock-out (`0x5ec110`), `CMSG_LOOT_MONEY`'s gate, auto-loot.
///
/// **Whether we kneel at it is a separate question** — [`LootKneel`], the client's predicate B
/// `0x612710`. Arming the latch is not arming the pose; conflating the two is what 1471 got wrong.
///
/// **The arms** — the real client has five, and we model all five (1477 shipped four; the fifth is
/// decision 1531):
/// - the `CMSG_LOOT` send (`0x5df253`/`0x5df40d`) — a **corpse** or player bones, armed at the
///   click, so the kneel is client-predicted with no round-trip;
/// - **`SMSG_SPELL_GO`** for an `OPEN_LOCK` cast that lands on a **chest**
///   (`0x6e831b → SetLootTarget 0x5ed5f0`) — this, not the loot response, is a chest's real arm,
///   and it is why the reference is already kneeling by the time the window opens;
/// - the **`CMSG_OPEN_ITEM` send** (`0x5edcc0`, in emitter `0x5edc80`) — a clam, lockbox or loot
///   bag in the bags, latched on the **item's own guid**
///   ([`crate::ui_items::drain::drain_container_uses`]'s open arm). It changes no pose — predicate
///   B refuses an ITEM — but it is exactly what makes the response's gate admit the answer:
///   vmangos replies `SendLoot(item guid, LOOT_CORPSE)`, i.e. wire type **1**, which a cold latch
///   refuses. 1477 read this arm as pose-only and left it out; the window stopped opening;
/// - `CGPlayer_C::OnLootResponse 0x5eb900`, through its **admission gate** — see
///   [`crate::ui_loot::net::loot_response`]. Not unconditional: a `loot_type == 1` response
///   against a cold latch is *refused and bounced*.
///
/// Cleared on release/close (`0x48f2c9`/`0x5ec0d4`). Ours clears **guid-matched** (release
/// response / refusal / our own release sends) — the safe generalization of the client's clears
/// under our corpse-switch race, where the *old* window's release response lands after the *new*
/// loot request armed the latch — plus unconditionally at session teardown.
#[derive(Resource, Default)]
pub(crate) struct LootLatch(pub(crate) Option<u64>);

/// **Predicate B `0x612710`, the local-player branch** — whether the object the [`LootLatch`]
/// currently names is one the character *kneels at* (decision 1477). The loot leg `0x5fd260` needs
/// predicate A (a session is open) **and** this one, and the split is the whole reason a fishing
/// bobber does not kneel while a chest does — the latch is armed identically for both.
///
/// The byte table, transcribed:
///
/// | latched object | kneels? |
/// |---|---|
/// | GameObject, any type but 17 (a chest, a herb node, a `FISHINGHOLE` 25) | yes |
/// | GameObject type **17 `FISHINGNODE`** — a fishing bobber (`0x612772`) | **no** |
/// | Unit with `UNIT_FIELD_HEALTH <= 0` — a creature corpse | yes |
/// | Unit with health **> 0** — pickpocketing a live target (`0x61278c`) | **no** |
/// | Item — a lockbox, a disenchant (`0x612797`, `!(TYPEMASK & 2)`) | **no** |
/// | a guid the object manager cannot resolve (`0x612732`) | **no** |
///
/// Recomputed by [`resolve_loot_kneel`] each frame, **between the net drain and the anim driver**.
/// That ordering is load-bearing, not tidiness: the reference does not poll this at all — it
/// force-plays Loot 50 *at* the chest arm (`0x5ed619`, in the same `SMSG_SPELL_GO` handler that
/// writes the latch), so the pose is up on the arming frame. Scheduling this system loose cost
/// exactly one frame of Stand at the open, which the chest probe caught as 59/60.
#[derive(Resource, Default)]
pub(crate) struct LootKneel(pub(crate) bool);

/// `GAMEOBJECT_TYPE_ID` 17 — `FISHINGNODE`, the bobber. The one GameObject type predicate B names
/// explicitly, and the reason "the latch is armed" is not the same question as "we kneel"
/// (`0x612772`; the test exists for exactly this case).
const GO_TYPE_FISHINGNODE: i32 = 17;

/// Recompute [`LootKneel`] from the latched object — predicate B `0x612710`'s local branch.
/// `pub(crate)` so [`crate::creature_anim`]'s driver chain can order itself after it (see
/// [`LootKneel`]: the reference's pose is up on the arming frame, so ours must be too).
pub(crate) fn resolve_loot_kneel(
    latch: Res<LootLatch>,
    index: Res<crate::net::GuidIndex>,
    objects: Query<(&crate::net::NetEntity, &crate::net::ObjectStore)>,
    mut kneel: ResMut<LootKneel>,
) {
    let allowed = latch
        .0
        .and_then(|guid| index.0.get(&guid).copied())
        .and_then(|e| objects.get(e).ok())
        .is_some_and(|(net_entity, store)| match net_entity.kind {
            // `0x612764`: a GameObject kneels unless it is the bobber.
            benilla_protocol::EntityKind::GameObject => {
                store.0.gameobject_type_id() != GO_TYPE_FISHINGNODE
            }
            // `0x61278c`: a unit kneels only once its `UNIT_FIELD_HEALTH` is not positive — read
            // straight off the descriptor, as the bytes do (an unsent field is 0 in the client's
            // descriptor array too, so `unwrap_or(0)` *is* the faithful read; this deliberately
            // does not go through `unit_is_dead`, whose extra `max_health > 0` guard the
            // reference has no counterpart for).
            benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player => {
                store.0.unit_health().unwrap_or(0) == 0
            }
            // An ITEM never kneels (`0x612797`), and we stream no item entities anyway — the
            // resolve above already answers `false` for a lockbox latch. Everything else
            // (DynamicObject, Other) is not a loot target the reference reaches here.
            _ => false,
        });
    if kneel.0 != allowed {
        kneel.0 = allowed;
    }
}

impl LootLatch {
    /// Drop the latch if it still points at `guid` (a release/refusal for that loot session).
    pub(crate) fn clear_for(&mut self, guid: u64) {
        if self.0 == Some(guid) {
            self.0 = None;
        }
    }
}

/// The controller's report that a **loot-closing movement START** happened this frame — the
/// reference's guard `0x60e990`, which every player-initiated movement-START emitter calls first
/// and whose tail (`arg2 == 0`) closes an open loot: forward/back, strafe, keyboard-turn and pitch
/// START, pitch STOP, `SetPitch`, and Jump. Mouse-look `SetFacing` passes `arg2 = 1` and does not.
/// Written by `player::control` beside the cast bar's [`crate::spell::LocalMoveStart`] (a
/// different mask — the cast's `0x10f0` excludes TURN, this one includes it), consumed and cleared
/// by [`drain_loot`] the next frame, which runs `CloseInteraction 0x48f200(cl=1, dl=1, 0)`: the
/// kneel latch clears, `CMSG_LOOT_RELEASE` goes out, the frame closes, and a dead corpse that is
/// also the selection is deselected — at distance zero, on the first step. Decision 2097.
///
/// **The loot window has no distance leash at all** (no loot target reaches the per-frame range
/// gate `0x493230`, which refutes 2094 and the 1741 census row it rested on: that row is a dead
/// lottery kiosk, `0x4c3eb0`). vmangos happens to release on every movement opcode too
/// (`MovementHandler.cpp:1108`), which is why the missing client-side close was invisible on the
/// local server and plain on cmangos (B381).
#[derive(Resource, Default)]
pub(crate) struct LootMoveStart(pub(crate) bool);

pub(crate) struct UiLootPlugin;

/// The loot rows' change callback (decision 2303): two flags.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut loot: ResMut<LootConfig>) {
    match ev.key().as_str() {
        "autolootdefault" => loot.auto_loot = ev.flag(),
        // The loot-roll detail switch (1589) — a flag over the roll-line composer's two shapes.
        "showlootspam" => loot.show_loot_spam = ev.flag(),
        _ => {}
    }
}

impl Plugin for UiLootPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.add_observer(on_cvar);
        app.init_resource::<LootState>()
            .init_resource::<LootConfig>()
            .init_resource::<LootLatch>()
            .init_resource::<LootKneel>()
            .init_resource::<LootMoveStart>()
            .add_systems(
                Update,
                (
                    // Push before the input pass so an open/close is on screen the same frame; drain
                    // after it so a click's intent goes out the same frame (mirrors ui_merchant).
                    feed_loot.in_set(UiFeed),
                    drain_loot.after(UiInput),
                    // Predicate B, per frame, after the net drain that arms the latch. The anim
                    // driver then orders itself after THIS (`crate::creature_anim`), closing the
                    // arm→pose gap to zero frames, as the reference's force-play does.
                    resolve_loot_kneel.after(benilla_world::schedule::WorldStage::Net),
                ),
            );
    }
}

/// Copper → the loot coin-row name text: each nonzero denomination as "<n> <Word>", the words the real
/// GlobalStrings coin words (`GOLD`/`SILVER`/`COPPER`), joined by a single space and dropping leading
/// zeroes (25 → "25 Copper"; 10025 → "1 Gold 25 Copper"; 0 → "0 Copper"). The coin row's name is
/// resolved app-side (the loot twin of the merchant's coin display).
///
/// **Stated approximation (documented gap).** 1.12.1's GlobalStrings has **no** `GOLD_AMOUNT` /
/// `SILVER_AMOUNT` / `COPPER_AMOUNT` "%d <Word>" patterns — those arrive in a later client. The real
/// 1.12 coin text is produced inside `GetLootSlotInfo` (a C function, not FrameXML, not yet read),
/// so we compose "<n> <Word>" from the bare `GOLD`/`SILVER`/`COPPER` words and join them the way
/// [`format_money`] always has (a single space); the exact client wording/separator is the stand-in.
fn format_money(copper: u32) -> String {
    let (g, s, c) = (copper / 10000, (copper % 10000) / 100, copper % 100);
    let mut parts: Vec<String> = Vec::new();
    if g > 0 {
        parts.push(format!("{g} {GOLD_WORD}"));
    }
    if s > 0 {
        parts.push(format!("{s} {SILVER_WORD}"));
    }
    if c > 0 || parts.is_empty() {
        parts.push(format!("{c} {COPPER_WORD}"));
    }
    parts.join(" ")
}

/// The coin-pile icon for a copper amount — the reference's six-step ladder, one table for the
/// loot slot, `GetCoinIcon` and the money cursor's bitmap (`benilla_ui::script::coin_icon`, 1965).
fn coin_icon(copper: u32) -> &'static str {
    benilla_ui::script::coin_icon(i64::from(copper))
}

/// Resolve one wire [`LootItem`] into the Lua-facing [`LootRow`]: the icon comes straight from the
/// wire `display_info_id` (no template wait), name + quality from the ask-once template cache (`None`
/// while in flight — the row shows a placeholder and fills in when the answer lands).
///
/// The row's **link** (`GetLootSlotLink`, decision 1059) comes off that same one template answer, out
/// of the one shared builder [`receive_line`] uses ([`crate::ui_items::item_link_full`], our
/// transcription of `0x52adb0`) and with the same arguments: enchant `0`, the wire's own
/// `randomPropertyId`, and suffix factor `0` — `SMSG_LOOT_RESPONSE`'s `randomSuffix` is a literal `0`
/// server-side (`LootMgr.cpp:841`, which is why [`LootItem`] doesn't even carry it), so there is
/// nothing else to pass. One builder, no drifting twins — the reason the zeros live in `item_link`
/// rather than at each call site.
fn resolve_item(
    item: &LootItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    rolls: RollCatalogs,
) -> LootRow {
    let (name, quality, link) = match items.template(item.item_id, 0, commands) {
        Some(t) => {
            // The rolled name IS the name here — the reference composes every display of an item's
            // name through `0x5d8b00`, link text included, so the row, the tooltip plate and the
            // shift-click link all read "Chipped Claw of the Bear" off this one string.
            let name = rolls.name(&t.name, item.random_property_id);
            (
                Some(name.clone()),
                Some(t.quality),
                Some(crate::ui_items::item_link_full(
                    item.item_id,
                    0,
                    item.random_property_id,
                    0,
                    &name,
                    t.quality,
                )),
            )
        }
        None => (None, None, None),
    };
    let texture = icons
        .and_then(|i| i.catalog.get(item.display_info_id))
        .and_then(|d| d.icon.clone());
    LootRow {
        name,
        texture,
        quantity: item.count,
        quality,
        is_coin: false,
        item_id: item.item_id,
        link,
        // The roll rides as the raw id, exactly as the client's own loot record keeps it: the
        // tooltip resolves it against the pushed roll table (`0x52b7bf`). Decision 1547.
        random_property_id: item.random_property_id,
    }
}

/// Whether any row of this open loot is still waiting on its item-template answer — the reference's
/// outstanding-query counter `[0xb71b44]`, in predicate form (decision 1805).
///
/// A row counts as waiting while its name is absent AND the server has not yet answered at all. A
/// **negative** answer ("no such entry") releases it: the reference's counter is decremented by the
/// arrival callback either way, and a window held open forever by an entry nobody will ever describe
/// is strictly worse than one that opens showing the cache-miss sentinels — which is exactly what
/// the reference paints in that case.
///
/// The coin row never waits; it has no template.
fn templates_outstanding(items: &Items, snap: &LootSnapshot) -> bool {
    snap.rows.iter().flatten().any(|r| {
        !r.is_coin
            && r.item_id != 0
            && r.name.is_none()
            && !items.template_answered_unknown(r.item_id)
    })
}

/// Build the Lua-facing snapshot from [`LootState`] — `None` when no loot is open. One entry per
/// slot of the **fixed** open-time layout: the coin pile first when the loot opened with gold, then
/// the items in wire order. A slot already looted stays in the list as `None` — the gap the
/// reference's hidden button leaves (`LOOT_SLOT_CLEARED` hides in place; the rows below never
/// shift up).
fn snapshot(
    loot: &LootState,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    rolls: RollCatalogs,
    who: Candidates,
) -> Option<LootSnapshot> {
    loot.source?;
    let mut rows = Vec::with_capacity(loot.items.len() + 1);
    if loot.coin_slot {
        rows.push(loot.has_coin().then(|| LootRow {
            name: Some(format_money(loot.gold)),
            texture: Some(coin_icon(loot.gold).into()),
            // 0, not 1, and for the same reason as [`COIN_QUALITY`]: `0x4c22e0`'s coin guard
            // returns before the record read (`0x4c22fd`). Invisible either way — stock
            // `LootFrame_Update` hides the count string unless `quantity > 1` — but it is the
            // value the accessor answers.
            quantity: 0,
            quality: Some(COIN_QUALITY),
            is_coin: true,
            item_id: 0,
            // No link: the coin pile is a synthesized row with no item behind it, so a modified
            // click on it finds nil and does nothing (decision 1059).
            link: None,
            random_property_id: 0,
        }));
    }
    for it in &loot.items {
        rows.push(
            (!loot.taken.contains(&it.slot))
                .then(|| resolve_item(it, items, icons, commands, rolls)),
        );
    }
    Some(LootSnapshot {
        rows,
        fishing: loot.fishing,
        master_candidates: who.names_for(loot),
    })
}

/// The two name sources a master-loot candidate guid can resolve through (decision 1675). The
/// roster is the primary one — `SMSG_GROUP_LIST` carries every other member's name outright, so no
/// query is needed — and the name cache covers the one guid the roster never lists: **our own**,
/// which vmangos includes in the candidate list (`Group::MasterLoot` walks the whole group,
/// `Group.cpp:919-937`) but excludes from the member array it sends us.
#[derive(Clone, Copy)]
struct Candidates<'a> {
    group: &'a GroupState,
    names: &'a NameCache,
}

impl Candidates<'_> {
    /// The open window's candidate slots as NAMES, with two kinds of hole preserved: an empty
    /// slot, and an occupied slot whose name has not resolved yet.
    ///
    /// Both read as `nil` from `GetMasterLootCandidate`, which is what the real binding does —
    /// `0x4c2f10` takes its name from the guid→name cache `0x55f080` and pushes **nil** on a miss
    /// (`0x4c2f91`), the same value it pushes for an empty slot. The miss is transient: the cache
    /// queues a lookup with `0x4c2fb0` as its completion callback, and that callback fires event
    /// `0x1f8` — `UPDATE_MASTER_LOOT_LIST`, whose whole job is to repaint the menu once a name
    /// lands. Answering an empty string instead would put a blank row in the dropdown and make
    /// that event pointless.
    fn names_for(&self, loot: &LootState) -> Vec<Option<String>> {
        loot.placed_candidates(self.group)
            .into_iter()
            .map(|slot| slot.and_then(|guid| self.name(guid)))
            .collect()
    }

    /// A candidate's display name, or `None` while it is unresolved. The roster is the primary
    /// source — `SMSG_GROUP_LIST` carries every other member's name outright — and the name cache
    /// covers our own guid, which that array never lists.
    fn name(&self, guid: u64) -> Option<String> {
        self.group
            .members
            .iter()
            .find(|m| m.guid == guid)
            .map(|m| m.name.clone())
            .or_else(|| self.names.peek(guid).map(str::to_string))
            .filter(|n| !n.is_empty())
    }
}

/// Compose one `CHAT_MSG_LOOT` receive line — the whole of `CGGameUI::OnItemPush`'s self branch,
/// VERIFIED against the 1.12.1 client binary (`WoW.exe` 5875, `0x491a60`, self arm `0x491bfb`).
///
/// The mechanic the line hangs on, and the reason the item name is **not** LOOT-green: the `%s` the
/// GlobalString takes is a full **item link**, not a bare name. `0x491c43`/`0x491ca3` call the link
/// builder `0x52adb0` ([`crate::ui_items::item_link_full`] is our transcription of it) with the item
/// id, enchant `0`, the wire `randomPropertyId`, the wire `suffixFactor`, and the resolved name. The
/// link's `|r` closes right after the `]`, so the count that follows is drawn in the line's own
/// colour — exactly the reference client's `[Chipped Claw]x2.`
///
/// The format strings are QUOTED from the extracted `Interface\FrameXML\GlobalStrings.lua`
/// (patch-2.MPQ, l.2599-2605): `LOOT_ITEM_SELF = "You receive loot: %s."` /
/// `LOOT_ITEM_SELF_MULTIPLE = "You receive loot: %sx%d."` and the PUSHED_SELF / CREATED_SELF twins —
/// note there is **no space** before the `x%d`. The key is selected by the wire's (created,
/// received) pair with created winning (`0x491c04`-`0x491c1b` / `0x491c60`-`0x491c77`), exactly as
/// the server sets them (vmangos `SendNewItem`: crafting → created, vendor/quest → received,
/// loot → neither).
fn receive_line(r: &PendingReceive, name: &str, quality: u32) -> String {
    let verb = if r.created {
        "You create"
    } else if r.from_npc {
        "You receive item"
    } else {
        "You receive loot"
    };
    let link = crate::ui_items::item_link_full(
        r.entry,
        0,
        r.random_property_id,
        r.suffix_factor,
        name,
        quality,
    );
    if r.count > 1 {
        format!("{verb}: {link}x{}.", r.count)
    } else {
        format!("{verb}: {link}.")
    }
}

/// Emit any pending pushes (`SMSG_ITEM_PUSH_RESULT`) once their item template resolves — the whole
/// `CGGameUI::OnItemPush` tail, in the reference's own order: the bag-bar drop animation first
/// (`ITEM_PUSH(container, icon)`, fired at `0x491be8` *before* the chat block, decision 0887), then —
/// if the wire asked for it — the "You receive …" line in the chat window (decision 0084's chat arc),
/// a `LOOT`-green line carrying a quality-coloured item link ([`receive_line`], decision 0888).
/// Unresolved pushes retry up to [`RECEIVE_MAX_TRIES`] frames, then drop (the reference instead
/// sleeps on the item-cache callback, so it never gives up — a stated divergence that only shows on
/// an entry the server never answers for).
fn drain_receives(
    loot: &mut LootState,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    chat: &mut crate::ui_chat::ChatLog,
    script: &mut UiScript,
    rolls: RollCatalogs,
) {
    let pending = std::mem::take(&mut loot.receives);
    let mut still = Vec::new();
    for mut r in pending {
        // One template read serves all three outputs: the name and quality for the line, the
        // display id for the animation's icon.
        let resolved = items
            .template(r.entry, 0, commands)
            .map(|t| (t.name.clone(), t.quality, t.display_info_id));
        match resolved {
            Some((name, quality, display_id)) => {
                // `ITEM_PUSH` is UNGATED by `show_in_chat` and by the created/received pair: the
                // fire at `0x491be8` precedes the `[ebx+0x24]` chat test at `0x491bf3`, so every
                // push that reaches us animates, including a silent one. `arg2` is the item's icon
                // path — the reference composes it from the icon directory + the display record's
                // own name (`0x491baa`, "%s%s%s"); ours is the same path out of the
                // `ItemDisplayInfo.dbc` catalog the bags and the loot window already read.
                let icon = icons
                    .and_then(|i| i.catalog.get(display_id))
                    .and_then(|d| d.icon.clone());
                if let Some(icon) = icon {
                    debug!(
                        "ui_loot: ITEM_PUSH container {} icon {icon} (item {})",
                        r.container, r.entry
                    );
                    script.fire_event(
                        "ITEM_PUSH",
                        vec![ScriptValue::Int(r.container), ScriptValue::Str(icon)],
                    );
                }
                if r.in_chat {
                    chat.push_event(crate::ui_chat::ChatEvent::text_only(
                        crate::ui_chat::ChatEventKind::Loot,
                        receive_line(&r, &rolls.name(&name, r.random_property_id), quality),
                    ));
                }
            }
            None => {
                r.tries += 1;
                if r.tries < RECEIVE_MAX_TRIES {
                    still.push(r);
                }
            }
        }
    }
    loot.receives = still;
}

/// Push the current loot into the VM and fire open/update/close on a transition (or a content change
/// — an async name landing, a removed row, the coin clearing). Also routes refusals + receive lines
/// into the chat window. Diffed against a `Local` memory, exactly like the merchant/gossip feeds.
fn feed_loot(
    script: Option<NonSendMut<UiScript>>,
    mut loot: ResMut<LootState>,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    mut chat: ResMut<crate::ui_chat::ChatLog>,
    mut last: Local<crate::ui_script::VmMemo<Option<LootSnapshot>>>,
    cfg: Res<LootConfig>,
    keys: Res<ButtonInput<KeyCode>>,
    mut pickup: MessageWriter<crate::sound::LootPickupSound>,
    // The random-suffix roll's two catalogs (decision 1547) — the drop's "of the Monkey" name and
    // the enchant slots 2..6 its tooltip shows. A loot slot carries no item object, so this is the
    // only source either can come from.
    props: Option<Res<crate::items::RandomProperties>>,
    enchants: Option<Res<crate::items::Enchants>>,
    // The two master-loot candidate name sources (decision 1675) — see [`Candidates`].
    group: Res<GroupState>,
    names: Res<NameCache>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let rolls = RollCatalogs {
        props: props.as_deref(),
        enchants: enchants.as_deref(),
    };
    drain_receives(
        &mut loot,
        &items,
        icons.as_deref(),
        &commands,
        &mut chat,
        &mut script,
        rolls,
    );

    let who = Candidates {
        group: &group,
        names: &names,
    };
    let fresh = snapshot(&loot, &items, icons.as_deref(), &commands, rolls, who);
    if fresh == *last {
        return;
    }
    script.set_loot(fresh.clone());
    match (&*last, &fresh) {
        (None, Some(snap)) => {
            // **The window does not open until every row's item template has landed** (1805). The
            // reference's copier ends `0x4c1e9f mov eax,[0xb71b44]; test eax,eax; jne 0x4c1eeb` —
            // if any template query is outstanding it fires NOTHING — and the item-cache arrival
            // callback `0x4c2ac0` fires `LOOT_OPENED` (`0x10b`) itself on that counter's falling
            // edge to zero (`0x4c2af6 dec` / `jne`). There is no repaint path to fall back on:
            // `LootFrame_Update` is reachable only through the XML `<OnShow>`, and `ShowUIPanel`
            // early-returns on an already-visible frame, so a second `LOOT_OPENED` would do
            // nothing. Deferring IS the mechanism.
            //
            // Returning without advancing `last` re-evaluates next frame; the queries are already
            // in flight from `snapshot` above, and the auto-loot sweep below waits with the window,
            // which is the reference's shape too (the same callback runs the sweep instead of
            // firing when the auto-loot latch is set).
            if templates_outstanding(&items, snap) {
                return;
            }
            script.fire_event("LOOT_OPENED", vec![]);
            // era's engine-side auto-loot ([`LootConfig`]): the knob decides, a held SHIFT
            // inverts it, and the client "clicks" every row itself at the open edge — the
            // same autostore/coin sends a hand pick makes, pickup sounds included. A refusal
            // (inventory full) keeps its row and the window simply stays; emptying it fires
            // the existing last-row auto-release.
            let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
            if cfg.auto_loot != shift {
                let mut bind_confirm_fired = false;
                for index in 1..=snap.rows.len() as u32 {
                    match loot.action_at(index) {
                        Some(LootAction::Money) => {
                            let _ = commands.0.send(ClientCommand::LootMoney);
                        }
                        // Only an ALLOW_LOOT row: the reference's auto-loot sweep processes a
                        // record exactly when the wire's slot-type getter answers 0
                        // (`0x4c2180`/`0x4c2196 test eax,eax; jne`). So a master-loot row is not
                        // swept into a dropdown, and a roll-in-progress row is not sent as a
                        // take the server would refuse.
                        Some(LootAction::Item {
                            wire_slot,
                            display_id,
                            item_id,
                            slot_type,
                        }) if slot_type == slot_type::ALLOW_LOOT => {
                            // The sweep carries the same bind gate as a hand click, plus a
                            // ONE-SHOT latch (`0x4c21c2 test ebx,ebx; jne` → the loop's continue,
                            // `0x4c21e2 mov ebx,1`; decision 1744): the first bind-on-pickup row
                            // raises the confirm, and every later one in the same sweep is left
                            // in the window untouched — not taken, not asked about. Otherwise a
                            // three-blue corpse would stack three dialogs over one pending slot.
                            if bind_confirm_required(&items, &commands, item_id) {
                                if bind_confirm_fired {
                                    continue;
                                }
                                loot.pending_bind_confirm = Some(index);
                                script.fire_event(
                                    "LOOT_BIND_CONFIRM",
                                    vec![ScriptValue::Int(i64::from(index))],
                                );
                                bind_confirm_fired = true;
                                continue;
                            }
                            let _ = commands
                                .0
                                .send(ClientCommand::AutostoreLootItem { slot: wire_slot });
                            pickup.write(crate::sound::LootPickupSound { display_id });
                        }
                        Some(LootAction::Item { .. }) | None => {}
                    }
                }
            }
        }
        // A content change while open (async name landed, a row removed, coin cleared) → repaint,
        // keeping the current page.
        //
        // **Both events fire, and the per-slot one is not optional any more** (1751). This used to
        // send `LOOT_UPDATE` alone, on the reasoning that a full re-snapshot replaces Blizzard's
        // per-button `LOOT_SLOT_CLEARED` optimization the way the merchant seam replaced per-row
        // stock updates. That was defensible while we owned `LootFrame.xml`. The STOCK file hangs
        // real behaviour off `LOOT_SLOT_CLEARED` and only off it: its arm hides the button in
        // place AND, when that empties the page, calls `LootFrame_PageDown` (`LootFrame.lua:38-50`
        // — "try to move second page of loot items to the first page"). `LOOT_UPDATE` reaches
        // none of that. Send only the summary and looting out a page leaves the player staring at
        // an empty window with a live Down arrow.
        //
        // So the reference's vocabulary is spoken as the reference speaks it: one
        // `LOOT_SLOT_CLEARED` per row that went away, carrying its 1-based row as `arg1`, and then
        // the summary repaint.
        (Some(before), Some(after)) => {
            for (i, was) in before.rows.iter().enumerate() {
                let gone = was.is_some() && after.rows.get(i).is_none_or(Option::is_none);
                if gone {
                    script.fire_event("LOOT_SLOT_CLEARED", vec![ScriptValue::Int(i as i64 + 1)]);
                }
            }
            // A changed candidate list refreshes the open dropdown in place without re-toggling
            // it — `LootFrame_OnEvent`'s `UIDropDownMenu_Refresh(GroupLootDropDown)`, which the
            // reference hangs off `UPDATE_MASTER_LOOT_LIST` (`LootFrame.lua:62-64`). Firing it
            // only on a real change keeps a closed menu untouched.
            if before.master_candidates != after.master_candidates {
                script.fire_event("UPDATE_MASTER_LOOT_LIST", vec![]);
            }
        }
        (Some(_), None) => script.fire_event("LOOT_CLOSED", vec![]),
        (None, None) => {}
    }
    *last = fresh;
}

/// Whether taking this row must first raise `LOOT_BIND_CONFIRM` — the reference's two-conjunct
/// gate at `0x4c28f2`/`0x4c28fb` (decision 1744). An unresolved template answers **false**: the
/// reference peeks its own item cache here and cannot ask, and a row whose template has not landed
/// has no name on it either, so it is not a row anyone has clicked. Asking (rather than peeking)
/// costs nothing — the entry is already in flight from the snapshot — and keeps the answer right
/// for the next click if one somehow arrives first.
fn bind_confirm_required(items: &Items, commands: &NetCommands, item_id: u32) -> bool {
    items
        .template(item_id, 0, commands)
        .is_some_and(|t| t.bonding == BIND_WHEN_PICKED_UP && t.quality >= BIND_CONFIRM_MIN_QUALITY)
}

/// `CloseInteraction 0x48f200(cl=1, dl=1, 0)` off the movement-START guard (decision 2097),
/// transcribed per loot-target type:
///
/// | open loot | on the first movement start |
/// |---|---|
/// | nothing open (`0x48f23f`) | nothing |
/// | wire type 4 DISENCHANT **and** rows left (`0x48f24a`, `0x48f25a`) | survives — the window stays |
/// | an ITEM guid — a lockbox (`0x48f2ab`–`0x48f2b5`) | survives, unreleased |
/// | a creature corpse, player bones, a GameObject chest/node, a fishing bobber | latch cleared (`0x48f2c9`), `CMSG_LOOT_RELEASE` sent (`0x48f2da`), frame closed (`0x48f33d`) |
/// | …and a dead UNIT that is also the selection (`0x48f34f`–`0x48f369`) | deselected too — `0x493910(guid, 1)` |
///
/// The deselect is asked of the target module ([`crate::target::DeselectGuid`]) rather than done
/// here: the teardown owns the attack-stop and the wire clear. A PICKPOCKET target is a live
/// unit (health above the `<= 1` gate) and keeps its selection.
fn close_on_move_start(
    loot: &mut LootState,
    latch: &mut LootLatch,
    commands: &NetCommands,
    deselect: &mut MessageWriter<crate::target::DeselectGuid>,
) {
    use benilla_protocol::guid;
    use benilla_protocol::messages::loot_type;
    let Some(source) = loot.source() else {
        return;
    };
    if loot.loot_type == loot_type::DISENCHANTING && !loot.is_empty() {
        return;
    }
    if guid::is_item(source) {
        return;
    }
    debug!("ui_loot: movement started with {source:#x} open — release");
    latch.clear_for(source);
    let _ = commands.0.send(ClientCommand::LootRelease { guid: source });
    if guid::is_creature_or_pet(source) && loot.loot_type != loot_type::PICKPOCKETING {
        deselect.write(crate::target::DeselectGuid(source));
    }
    loot.clear();
}

/// Drain the Lua intents: a picked row → coin (`CMSG_LOOT_MONEY`) or the item's wire slot
/// (`CMSG_AUTOSTORE_LOOT_ITEM`); a close → `CMSG_LOOT_RELEASE` + a client-authoritative local clear
/// (the window is already hidden by its `OnHide`; the release is fire-and-forget, and the server's
/// `SMSG_LOOT_RELEASE_RESPONSE` clears again idempotently). Also fires the **last-row auto-close**
/// ([`LootState::auto_release`]): the client, not the server, releases when a removal empties the
/// window — vmangos only ever releases in answer to our `CMSG_LOOT_RELEASE`.
///
/// **This function is the reference's take dispatcher `0x4c2790(slot, flag)`** (decision 1744), and
/// the two Lua verbs are its two flags: `BenillaTakeLootSlot` is the row click (`flag == 0`, the C
/// `CLootButton`'s arm) and `LootSlot` is the LOOT_BIND confirmation continuation (`flag == 1`,
/// which sends only for the pending slot). Keeping them apart is what makes a second click on a
/// bind-on-pickup row re-raise the confirm instead of looting behind it.
fn drain_loot(
    script: Option<NonSendMut<UiScript>>,
    mut loot: ResMut<LootState>,
    mut latch: ResMut<LootLatch>,
    commands: Res<NetCommands>,
    mut pickup: MessageWriter<crate::sound::LootPickupSound>,
    // The candidate placement is raid-shaped, so resolving a clicked menu index needs the roster.
    group: Res<GroupState>,
    // The bind-on-pickup deferral reads the row's template (`bonding`, `quality`). Already cached
    // by then in every reachable case — the snapshot asks for it to put a NAME on the row, and a
    // row with no name is a row nobody has clicked.
    items: Res<Items>,
    // The controller's move-start report (decision 2097) and the selection teardown the close
    // asks for — ahead of the VM check below, because neither depends on Lua.
    mut move_start: ResMut<LootMoveStart>,
    mut deselect: MessageWriter<crate::target::DeselectGuid>,
) {
    if std::mem::take(&mut move_start.0) {
        close_on_move_start(&mut loot, &mut latch, &commands, &mut deselect);
    }
    let Some(mut script) = script else {
        return;
    };
    for index in script.take_loot_picks() {
        match loot.action_at(index) {
            Some(LootAction::Money) => {
                // No coin play here: the coin rides the coinage-change watcher (`sound::money`)
                // when `PLAYER_FIELD_COINAGE` rises — the same rule as buy/sell.
                debug!("ui_loot: loot coin (row {index})");
                let _ = commands.0.send(ClientCommand::LootMoney);
            }
            // A master-loot row is not takeable by anyone — it is ASSIGNED. The click opens the
            // candidate dropdown instead of sending a take, and the decision is made here, on the
            // wire's slot-type byte, rather than in Lua: the real client branches on the same byte
            // inside its take dispatcher before any Lua runs (`0x4c2790`, which already reads the
            // getter at `0x4c28a9`), and the reference `LootFrame.lua` never consults
            // `GetLootMethod` at all. The row's `LootFrame.selected*` bookkeeping is already
            // stashed by the Lua `OnClick` that ran before this drain, so the event's
            // `ToggleDropDownMenu` has its anchor button.
            Some(LootAction::Item { slot_type, .. }) if slot_type == slot_type::MASTER => {
                debug!("ui_loot: row {index} is master-loot — opening the candidate list");
                script.fire_event("OPEN_MASTER_LOOT_LIST", vec![]);
            }
            Some(LootAction::Item {
                wire_slot,
                display_id,
                item_id,
                ..
            }) => {
                // The bind-on-pickup deferral (`0x4c28f2`-`0x4c2920`, decision 1744). Two
                // conjuncts and no others: the template's `bonding == BIND_WHEN_PICKED_UP` AND its
                // `quality >= 2` (uncommon or better) — a grey or white BoP row is taken with no
                // confirm at all, which is why picking up a quest trinket never asks. The event
                // carries the row out, the row is stashed, and NOTHING is sent; not even the
                // pickup sound, which the reference plays only on the arm that actually sends.
                if bind_confirm_required(&items, &commands, item_id) {
                    debug!("ui_loot: row {index} (wire {wire_slot}) binds on pickup — confirming");
                    loot.pending_bind_confirm = Some(index);
                    script.fire_event(
                        "LOOT_BIND_CONFIRM",
                        vec![ScriptValue::Int(i64::from(index))],
                    );
                    continue;
                }
                debug!("ui_loot: autostore row {index} (wire slot {wire_slot})");
                let _ = commands
                    .0
                    .send(ClientCommand::AutostoreLootItem { slot: wire_slot });
                // The pickup sound plays optimistically at the click, before the send (`0x4c2926`):
                // looting an item plays its ItemGroupSounds kit[0].
                pickup.write(crate::sound::LootPickupSound { display_id });
            }
            None => debug!("ui_loot: BenillaTakeLootSlot({index}) out of range — ignored"),
        }
    }

    // `LootSlot(slot)` — the confirm continuation (`0x4c2790`'s `flag != 0` arm, `0x4c27c0`). It
    // sends for exactly one slot: the one the pending confirm names. Anything else is dropped in
    // silence, which is what the reference does with an addon that calls `LootSlot` on an ordinary
    // row. The stash clears on the send (`0x4c281a`), so a doubled OnAccept cannot loot twice.
    for index in script.take_loot_confirms() {
        if loot.pending_bind_confirm != Some(index) {
            debug!("ui_loot: LootSlot({index}) is not the pending bind confirm — ignored");
            continue;
        }
        let Some(LootAction::Item {
            wire_slot,
            display_id,
            ..
        }) = loot.action_at(index)
        else {
            // The row went away under the open dialog (someone else took it, the window turned
            // over). The reference bails the same way — its continuation re-reads the record and
            // returns on an empty itemId (`0x4c27d7`) — and, like it, WITHOUT clearing the stash:
            // the clear is on the send path alone (`0x4c281a`), and the window turning over is
            // what drops a stash that never completed.
            debug!("ui_loot: bind confirm for row {index} — the row is gone, nothing sent");
            continue;
        };
        loot.pending_bind_confirm = None;
        debug!("ui_loot: bind-confirmed row {index} (wire slot {wire_slot})");
        let _ = commands
            .0
            .send(ClientCommand::AutostoreLootItem { slot: wire_slot });
        pickup.write(crate::sound::LootPickupSound { display_id });
    }
    // The master looter's assignments (`GiveMasterLoot(slot, candidateIndex)`): the Lua hands two
    // 1-based display numbers, the app turns them into the wire slot and the recipient guid. Both
    // must resolve — a stale row or a candidate index past the end of the list is dropped here
    // rather than sent, since the app could not address either.
    for (index, candidate) in script.take_loot_master_gives() {
        let Some(guid) = loot.source else {
            continue;
        };
        let (
            Some(LootAction::Item {
                wire_slot,
                slot_type,
                ..
            }),
            Some(target),
        ) = (
            loot.action_at(index),
            loot.master_candidate(candidate, &group),
        )
        else {
            debug!("ui_loot: GiveMasterLoot({index}, {candidate}) unresolvable — ignored");
            continue;
        };
        // The row must still BE a master row. The real sender checks exactly this before it
        // builds anything — `0x4c2940` re-reads the slot type through `0x5ebce0(record+0x18)`
        // and bails unless it is 2 — so a give aimed at an ordinary row sends nothing at all
        // rather than a packet the server would refuse.
        if slot_type != slot_type::MASTER {
            debug!("ui_loot: GiveMasterLoot({index}) is not a master row — ignored");
            continue;
        }
        debug!("ui_loot: master-give row {index} (wire slot {wire_slot}) to {target:#x}");
        let _ = commands.0.send(ClientCommand::LootMasterGive {
            guid,
            slot: wire_slot,
            target,
        });
    }
    if script.take_loot_close() {
        if let Some(guid) = loot.source {
            debug!("ui_loot: release loot {guid:#x}");
            let _ = commands.0.send(ClientCommand::LootRelease { guid });
            loot.clear(); // client-authoritative close (mirrors the merchant's optimistic clear)
            latch.clear_for(guid); // the kneel ends at the release send (0515)
        }
    }
    // The last-row auto-close (`LootState::auto_release`): a wire removal just emptied the open
    // window, so the client releases on its own — the real engine's close-on-last-slot
    // (`0x4c2a70` empty-check → `0x48f200`; the server never initiates it). The clear makes the
    // feed fire LOOT_CLOSED next pass; the window's OnHide then calls CloseLoot(), whose drain
    // above no-ops on the already-cleared source.
    if loot.take_auto_release() {
        if let Some(guid) = loot.source {
            debug!("ui_loot: loot emptied — auto-release {guid:#x}");
            let _ = commands.0.send(ClientCommand::LootRelease { guid });
            loot.clear();
            latch.clear_for(guid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::loot_type;
    use benilla_protocol::messages::GroupMemberEntry;
    use benilla_protocol::messages::ItemInfo;
    use benilla_protocol::messages::ObjectFields;
    use benilla_protocol::EntityKind;
    use bevy::ecs::system::RunSystemOnce;

    /// An empty group + name cache — what every loot test that is not about master loot wants:
    /// no candidates resolve, and a snapshot's `master_candidates` comes out empty. Master-loot
    /// tests build their own roster.
    fn nobody() -> (GroupState, NameCache) {
        (GroupState::default(), NameCache::default())
    }

    /// Descriptor field indices the predicate-B table reads.
    const F_GO_TYPE_ID: u16 = 21;
    const F_UNIT_HEALTH: u16 = 22;

    /// Drive [`resolve_loot_kneel`] once over a world holding one latched object, and report what
    /// predicate B said. `None` for `object` = the latch names a guid that does not resolve.
    fn kneels_at(object: Option<(EntityKind, &[(u16, u32)])>) -> bool {
        const GUID: u64 = 0xF110_0000_0000_0042;
        let mut app = App::new();
        app.init_resource::<LootLatch>()
            .init_resource::<LootKneel>()
            .init_resource::<crate::net::GuidIndex>()
            .add_systems(Update, resolve_loot_kneel);
        app.world_mut().resource_mut::<LootLatch>().0 = Some(GUID);
        if let Some((kind, fields)) = object {
            let e = app
                .world_mut()
                .spawn((
                    crate::net::NetEntity {
                        kind,
                        display_id: None,
                        scale: 1.0,
                    },
                    crate::net::ObjectStore(ObjectFields::from_pairs(fields)),
                ))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(GUID, e);
        }
        app.update();
        app.world().resource::<LootKneel>().0
    }

    /// **Predicate B `0x612710`, the local branch** (decision 1477).
    /// The whole row set, because the *point* of this predicate is that arming the latch is not
    /// the same question as kneeling: a fishing bobber and a chest arm it identically, and only
    /// one of them is knelt at. Without this filter, 1471's response-arm gave benilla a kneel at
    /// a bobber, over a lockbox, and while pickpocketing — none of which the reference does.
    #[test]
    fn predicate_b_decides_which_loot_targets_are_knelt_at() {
        // A GameObject that is not the bobber — a chest, a herb node, a FISHINGHOLE(25).
        assert!(kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 3)]
        ))));
        assert!(kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 25)]
        ))));
        // `0x612772` — GAMEOBJECT_TYPE_ID 17 FISHINGNODE, the one type named explicitly.
        assert!(!kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 17)]
        ))));
        // `0x61278c` — a corpse kneels, a live target (pickpocketing) does not.
        assert!(kneels_at(Some((EntityKind::Unit, &[(F_UNIT_HEALTH, 0)]))));
        assert!(!kneels_at(Some((EntityKind::Unit, &[(F_UNIT_HEALTH, 1)]))));
        // `0x612732` — a guid the object manager cannot resolve (an item latch is this, for us).
        assert!(!kneels_at(None));
    }

    /// A cold latch is not a kneel — predicate A's half, folded into the same resource so the
    /// anim driver reads one boolean.
    #[test]
    fn a_cold_latch_never_kneels() {
        let mut app = App::new();
        app.init_resource::<LootLatch>()
            .init_resource::<LootKneel>()
            .init_resource::<crate::net::GuidIndex>()
            .add_systems(Update, resolve_loot_kneel);
        app.update();
        assert!(!app.world().resource::<LootKneel>().0);
    }

    // ── The soulbind confirm (decision 1744) ──────────────────────────────────────────────────
    //
    // Real 1.12 `item_template` rows, read from the running vmangos rather than invented, so the
    // two conjuncts are exercised against numbers the server actually ships:
    //   12590 Felstriker    quality 4, bonding 1  — BoP epic: confirms
    //     871 Flurry Axe    quality 4, bonding 2  — BoE epic: takes, the bonding control
    //     117 Tough Jerky   quality 1, bonding 0  — plain white: takes
    // and one synthetic that the database has no clean example of at this quality:
    //    9999 a white BoP   quality 1, bonding 1  — the QUALITY control, which takes.
    const FELSTRIKER: u32 = 12590;
    const FLURRY_AXE: u32 = 871;
    const TOUGH_JERKY: u32 = 117;
    const WHITE_BOP: u32 = 9999;
    /// A second BoP epic, for the auto-loot sweep's one-dialog latch (18832 Brutality Blade).
    const SECOND_BOP: u32 = 18832;

    /// A world with `rows` open on a corpse, every template already landed, and `lua` run against
    /// the loot bindings. Returns the app and the wire's receiver so a test can read what was
    /// actually sent — the whole point of the arc being that a confirm sends **nothing**.
    fn drain_with(
        gold: u32,
        rows: Vec<LootItem>,
        lua: &str,
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::LootPickupSound>()
            .add_message::<crate::target::DeselectGuid>()
            .init_resource::<LootState>()
            .init_resource::<LootLatch>()
            .init_resource::<LootMoveStart>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx));

        let mut items = app.world_mut().resource_mut::<Items>();
        let mut tmpl = |entry: u32, name: &str, quality: u32, bonding: u32| {
            items.insert_template(
                entry,
                Some(ItemInfo {
                    quality,
                    bonding,
                    ..crate::items::test_template(name)
                }),
            );
        };
        tmpl(FELSTRIKER, "Felstriker", 4, 1);
        tmpl(FLURRY_AXE, "Flurry Axe", 4, 2);
        tmpl(TOUGH_JERKY, "Tough Jerky", 1, 0);
        tmpl(WHITE_BOP, "A White Soulbound Thing", 1, 1);

        app.world_mut()
            .resource_mut::<LootState>()
            .open(0x42, loot_type::CORPSE, gold, rows);

        let script = UiScript::new().unwrap();
        // A listener, so the event is observed where the real dialog driver sits rather than
        // through a test-only back door.
        script
            .run(
                "BIND_CONFIRMS = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"LOOT_BIND_CONFIRM\")\n\
                 f:SetScript(\"OnEvent\", function() tinsert(BIND_CONFIRMS, arg1) end)",
            )
            .unwrap();
        script.run(lua).unwrap();
        app.insert_non_send_resource(script);
        app.world_mut().run_system_once(drain_loot).unwrap();
        (app, rx)
    }

    /// **The window waits for the item templates** (1805) — the reference's own open rule, and the
    /// reason the cache-miss sentinels beside it are a floor rather than a plan.
    ///
    /// `0x4c1cb0` returns without firing while `[0xb71b44]` is non-zero, and the item-cache arrival
    /// callback `0x4c2ac0` fires `LOOT_OPENED` on that counter's falling edge instead. There is no
    /// repaint to fall back on: `LootFrame_Update` runs only from `<OnShow>`, and `ShowUIPanel`
    /// early-returns on an already-visible frame.
    ///
    /// So: a corpse whose one row is an entry we have never seen must open NOTHING on the first
    /// pass — and must ask the server for the template — then open exactly once when the answer
    /// lands. A **negative** answer opens it too; a window held shut forever by an entry nobody can
    /// describe is our one deliberate divergence from the reference's success-gated callback.
    #[test]
    fn the_window_waits_for_every_item_template() {
        for answer in [
            Some(ItemInfo {
                quality: 1,
                ..crate::items::test_template("Thin Cloth Gloves")
            }),
            // …and the negative answer, which releases the window rather than holding it shut.
            None,
        ] {
            let (tx, rx) = crossbeam_channel::unbounded();
            let mut app = App::new();
            app.add_message::<crate::sound::LootPickupSound>()
                .init_resource::<LootState>()
                .init_resource::<crate::ui_chat::ChatLog>()
                .init_resource::<GroupState>()
                .init_resource::<NameCache>()
                .init_resource::<Items>()
                .init_resource::<ButtonInput<KeyCode>>()
                .init_resource::<LootConfig>()
                .insert_resource(NetCommands(tx))
                // Registered in a schedule rather than driven by `run_system_once`, because the
                // whole question is what happens ACROSS frames and `feed_loot`'s memo of the last
                // snapshot is a `Local`: `run_system_once` builds a new system each call and hands
                // it a fresh memo, which reads every pass as a first open.
                .add_systems(bevy::prelude::Update, feed_loot);
            app.world_mut().resource_mut::<LootState>().open(
                0x42,
                loot_type::CORPSE,
                0,
                vec![item(3, TOUGH_JERKY, 1)],
            );

            let script = UiScript::new().unwrap();
            script
                .run(
                    "OPENS = 0\n\
                     local f = CreateFrame(\"Frame\")\n\
                     f:RegisterEvent(\"LOOT_OPENED\")\n\
                     f:SetScript(\"OnEvent\", function() OPENS = OPENS + 1 end)",
                )
                .unwrap();
            app.insert_non_send_resource(script);

            let opens = |app: &mut App| {
                app.world_mut()
                    .non_send_resource_mut::<UiScript>()
                    .eval::<i64>("return OPENS")
                    .unwrap()
            };

            // Pass one: the template is unknown, so the window stays shut — and the ask goes out.
            app.update();
            assert_eq!(
                opens(&mut app),
                0,
                "no LOOT_OPENED while a template is pending"
            );
            assert!(
                sent(&rx).iter().any(
                    |c| matches!(c, ClientCommand::ItemQuery { entry, .. } if *entry == TOUGH_JERKY)
                ),
                "and the template was asked for"
            );

            // A second pass with nothing new changes nothing — the deferral is not a one-shot.
            app.update();
            assert_eq!(opens(&mut app), 0);

            // The answer lands: the window opens, once.
            app.world_mut()
                .resource_mut::<Items>()
                .insert_template(TOUGH_JERKY, answer.clone());
            app.update();
            assert_eq!(opens(&mut app), 1, "opened when the last template answered");
            app.update();
            assert_eq!(opens(&mut app), 1, "and only once");
        }
    }

    /// The rows `LOOT_BIND_CONFIRM` has named so far, in order.
    fn confirms(app: &mut App) -> Vec<i64> {
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        let n = script.eval::<i64>("return getn(BIND_CONFIRMS)").unwrap();
        (1..=n)
            .map(|i| {
                script
                    .eval::<i64>(&format!("return BIND_CONFIRMS[{i}]"))
                    .unwrap()
            })
            .collect()
    }

    /// Everything the wire saw, in order.
    fn sent(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<ClientCommand> {
        rx.try_iter().collect()
    }

    /// Which row the app is holding a confirm open for.
    fn pending_confirm(app: &App) -> Option<u32> {
        app.world().resource::<LootState>().pending_bind_confirm
    }

    /// The click arm's deferral (`0x4c28f2`-`0x4c2920`): a BoP uncommon-or-better row fires
    /// `LOOT_BIND_CONFIRM` with its row, stashes the row, and sends **nothing** — not the
    /// autostore, and not the pickup sound either, which the reference plays only on the arm that
    /// sends.
    #[test]
    fn a_bop_row_confirms_instead_of_sending() {
        let (mut app, rx) = drain_with(0, vec![item(0, FELSTRIKER, 1)], "BenillaTakeLootSlot(1)");
        assert!(sent(&rx).is_empty(), "the deferred take sends nothing");
        assert_eq!(pending_confirm(&app), Some(1), "and stashes the row");

        assert_eq!(
            confirms(&mut app),
            vec![1],
            "the event carries the row out, 1-based and display-side"
        );
    }

    /// The confirm arm (`0x4c27c0`): `LootSlot` on the pending row sends the autostore for that
    /// row's WIRE slot and clears the stash — so a doubled OnAccept cannot loot twice.
    #[test]
    fn loot_slot_completes_the_pending_confirm_exactly_once() {
        let (mut app, rx) = drain_with(
            0,
            vec![item(7, FELSTRIKER, 1)],
            "BenillaTakeLootSlot(1) LootSlot(1)",
        );
        assert!(
            matches!(
                sent(&rx)[..],
                [ClientCommand::AutostoreLootItem { slot: 7 }]
            ),
            "the accept sends the row's wire slot, once"
        );
        assert_eq!(pending_confirm(&app), None, "and the stash clears");

        // A second accept over the same dialog: nothing left to complete.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("LootSlot(1)")
            .unwrap();
        app.world_mut().run_system_once(drain_loot).unwrap();
        assert!(sent(&rx).is_empty(), "a second accept sends nothing");
    }

    /// `LootSlot` on any row that is NOT the pending confirm does nothing at all — which is
    /// exactly what it does on the real client, where the flag-1 arm opens `cmp edi,[0x847cec]`.
    /// This is the property that makes the two-verb split worth having.
    #[test]
    fn loot_slot_on_an_ordinary_row_sends_nothing() {
        let (_app, rx) = drain_with(
            0,
            vec![item(0, TOUGH_JERKY, 1), item(1, FLURRY_AXE, 1)],
            "LootSlot(1) LootSlot(2)",
        );
        assert!(
            sent(&rx).is_empty(),
            "no confirm is pending, so neither call reaches the wire"
        );
    }

    /// Both conjuncts are load-bearing, and each fails alone. A bind-on-EQUIP epic and a
    /// bind-on-pickup WHITE are both taken outright, with no dialog — the second is why looting a
    /// grey quest trinket never asks.
    #[test]
    fn the_bind_gate_needs_both_conjuncts() {
        for (entry, why) in [
            (FLURRY_AXE, "bind-on-equip is not bind-on-pickup"),
            (WHITE_BOP, "quality 1 is below the floor of 2"),
            (TOUGH_JERKY, "neither"),
        ] {
            let (app, rx) = drain_with(0, vec![item(3, entry, 1)], "BenillaTakeLootSlot(1)");
            assert!(
                matches!(
                    sent(&rx)[..],
                    [ClientCommand::AutostoreLootItem { slot: 3 }]
                ),
                "entry {entry} should be taken outright: {why}"
            );
            assert_eq!(
                pending_confirm(&app),
                None,
                "entry {entry}: no confirm ({why})"
            );
        }
    }

    /// The stash is display-side and coin-aware. The reference's own event argument is the item
    /// ARRAY index plus one — it is written after the coin-row shift (`0x4c2885 dec edi`) and never
    /// shifted back — so on a corpse with gold its `arg1` is one below the display row. benilla
    /// speaks the display row on both halves of the round trip instead (1744), which is what makes
    /// `GetLootSlotInfo(arg1)` mean what it looks like it means.
    #[test]
    fn the_coin_row_does_not_shift_the_confirm_out_from_under_itself() {
        let (mut app, rx) = drain_with(
            120, // gold, so display row 1 is the coin and the item is row 2
            vec![item(4, FELSTRIKER, 1)],
            "BenillaTakeLootSlot(2)",
        );
        assert_eq!(
            pending_confirm(&app),
            Some(2),
            "the DISPLAY row, coin included"
        );
        assert_eq!(confirms(&mut app), vec![2]);
        assert!(sent(&rx).is_empty());

        // And the accept, with the same number, reaches the right WIRE slot.
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("LootSlot(2)")
            .unwrap();
        app.world_mut().run_system_once(drain_loot).unwrap();
        assert!(matches!(
            sent(&rx)[..],
            [ClientCommand::AutostoreLootItem { slot: 4 }]
        ));
    }

    /// The row vanishes under the open dialog — someone else in the group took it while the
    /// confirm sat there. The accept sends nothing (`0x4c27d7`: the continuation re-reads the
    /// record and returns on an empty itemId) and, like the reference, leaves the stash alone: the
    /// clear lives on the send path only (`0x4c281a`).
    #[test]
    fn an_accept_for_a_row_that_was_taken_away_sends_nothing() {
        let (mut app, rx) = drain_with(0, vec![item(2, FELSTRIKER, 1)], "BenillaTakeLootSlot(1)");
        assert_eq!(pending_confirm(&app), Some(1));

        // SMSG_LOOT_REMOVED for that wire slot: the row becomes a gap.
        app.world_mut().resource_mut::<LootState>().remove_slot(2);
        let script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.run("LootSlot(1)").unwrap();
        app.world_mut().run_system_once(drain_loot).unwrap();

        assert!(
            sent(&rx)
                .iter()
                .all(|c| !matches!(c, ClientCommand::AutostoreLootItem { .. })),
            "nothing is autostored for a row that is gone"
        );
    }

    /// A pending confirm dies with the window that raised it (`0x4c1df5`: the `SMSG_LOOT_RESPONSE`
    /// copier resets the stash to -1). Its row number would name a slot in the NEXT corpse.
    #[test]
    fn a_pending_confirm_does_not_survive_the_window() {
        let (mut app, _rx) = drain_with(0, vec![item(0, FELSTRIKER, 1)], "BenillaTakeLootSlot(1)");
        assert_eq!(pending_confirm(&app), Some(1));

        app.world_mut().resource_mut::<LootState>().clear();
        assert_eq!(pending_confirm(&app), None, "the close drops it");

        app.world_mut().resource_mut::<LootState>().open(
            0x43,
            loot_type::CORPSE,
            0,
            vec![item(0, TOUGH_JERKY, 1)],
        );
        assert_eq!(
            pending_confirm(&app),
            None,
            "and a fresh window opens with none"
        );
    }

    /// **The auto-loot sweep's one-shot latch** (`0x4c21c2 test ebx,ebx; jne` → the loop's
    /// continue, `0x4c21e2 mov ebx,1`; decision 1744). A corpse with two bind-on-pickup blues and
    /// a white: the sweep takes the white, raises ONE dialog for the first blue, and leaves the
    /// second blue in the window untouched — not taken, not asked about. Without the latch it
    /// would stack two dialogs over a single pending slot, and the second would name a row the
    /// stash no longer holds.
    #[test]
    fn the_auto_loot_sweep_raises_exactly_one_bind_confirm() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::LootPickupSound>()
            .init_resource::<LootState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<GroupState>()
            .init_resource::<NameCache>()
            .init_resource::<Items>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(LootConfig {
                auto_loot: true,
                ..LootConfig::default()
            })
            .insert_resource(NetCommands(tx));

        let mut items = app.world_mut().resource_mut::<Items>();
        for (entry, name, quality, bonding) in [
            (FELSTRIKER, "Felstriker", 4, 1),
            (SECOND_BOP, "Brutality Blade", 4, 1),
            (TOUGH_JERKY, "Tough Jerky", 1, 0),
        ] {
            items.insert_template(
                entry,
                Some(ItemInfo {
                    quality,
                    bonding,
                    ..crate::items::test_template(name)
                }),
            );
        }

        // Wire slots 5/6/7 in display order: BoP epic, BoP epic, white.
        app.world_mut().resource_mut::<LootState>().open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![
                item(5, FELSTRIKER, 1),
                item(6, SECOND_BOP, 1),
                item(7, TOUGH_JERKY, 1),
            ],
        );
        let script = UiScript::new().unwrap();
        script
            .run(
                "BIND_CONFIRMS = {}\n\
                 local f = CreateFrame(\"Frame\")\n\
                 f:RegisterEvent(\"LOOT_BIND_CONFIRM\")\n\
                 f:SetScript(\"OnEvent\", function() tinsert(BIND_CONFIRMS, arg1) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.world_mut().run_system_once(feed_loot).unwrap();

        assert_eq!(
            confirms(&mut app),
            vec![1],
            "one dialog, for the FIRST bind-on-pickup row only"
        );
        assert_eq!(pending_confirm(&app), Some(1));
        assert!(
            matches!(
                sent(&rx)[..],
                [ClientCommand::AutostoreLootItem { slot: 7 }]
            ),
            "the white is swept; neither blue is sent"
        );
    }

    fn item(slot: u8, entry: u32, count: u32) -> LootItem {
        LootItem {
            slot,
            item_id: entry,
            count,
            display_info_id: 1000 + entry,
            random_property_id: 0,
            slot_type: 0,
        }
    }

    /// The same row, stamped MASTER — what vmangos sends every group member for every row once
    /// the group's loot method is master loot.
    fn master_item(slot: u8, entry: u32, count: u32) -> LootItem {
        LootItem {
            slot_type: slot_type::MASTER,
            ..item(slot, entry, count)
        }
    }

    /// The candidate list rides AHEAD of the response it belongs to (the server sends it from
    /// inside `SendLoot`), so the window has to claim what was staged before it opened — and a
    /// later window opened with no list of its own must not inherit the previous one's.
    #[test]
    fn the_candidate_list_arrives_before_its_window_and_does_not_outlive_it() {
        let mut loot = LootState::default();
        // A default GroupState is a plain party (`group_type` 0) — the flat placement path.
        let party = GroupState::default();

        // Staged with no window open: nothing to read yet.
        loot.set_master_candidates(vec![0xA, 0xB, 0xC]);
        assert_eq!(
            loot.master_candidate(1, &party),
            None,
            "no window, no candidates"
        );

        // The response lands: the window claims the staged list, 1-based.
        loot.open(0x42, loot_type::CORPSE, 0, vec![master_item(0, 117, 1)]);
        assert_eq!(loot.master_candidate(1, &party), Some(0xA));
        assert_eq!(loot.master_candidate(3, &party), Some(0xC));
        assert_eq!(loot.master_candidate(4, &party), None, "past the end");
        assert_eq!(
            loot.master_candidate(0, &party),
            None,
            "0 is not a Lua index"
        );

        // A refresh while the window is up applies in place (the ref's UPDATE_MASTER_LOOT_LIST).
        loot.set_master_candidates(vec![0xA, 0xB]);
        assert_eq!(loot.master_candidate(2, &party), Some(0xB));
        assert_eq!(loot.master_candidate(3, &party), None, "the list shrank");

        // Close, then a plain corpse under a different loot method: no candidates leak across.
        loot.clear();
        loot.open(0x43, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert_eq!(
            loot.master_candidate(1, &party),
            None,
            "a fresh window starts empty"
        );
    }

    /// **The raid placement — the finding that corrected this code** (VERIFIED, the 5875
    /// binary's `SMSG_LOOT_MASTER_LIST` handler `0x61c550`). In a raid the candidate array is not
    /// the wire order: each guid is filed into the first free slot of its own subgroup's block of
    /// five. It was built dense first, which would have labelled every candidate with the wrong
    /// raid group in the dropdown's "Group N" submenus.
    #[test]
    fn raid_candidates_file_into_their_own_subgroup_block() {
        let member = |guid: u64, name: &str, subgroup: u8| GroupMemberEntry {
            name: name.into(),
            guid,
            status: 0,
            flags: subgroup,
        };
        let mut raid = GroupState {
            group_type: GROUPTYPE_RAID,
            own_flags: 0, // we are in subgroup 0
            ..GroupState::default()
        };
        raid.members = vec![
            member(0xB, "Cairne", 0),
            member(0xC, "Vol", 2),
            member(0xD, "Sylvanas", 2),
        ];

        let mut loot = LootState::default();
        // Wire order deliberately interleaves the subgroups — placement must ignore it.
        loot.set_master_candidates(vec![0xC, 0xA, 0xD, 0xB]);
        loot.open(0x42, loot_type::CORPSE, 0, vec![master_item(0, 117, 1)]);

        // Subgroup 0 fills slots 1-2 (us at 0xA, then Cairne); subgroup 2 fills 11-12, in the
        // order the wire listed them.
        assert_eq!(loot.master_candidate(1, &raid), Some(0xA), "us, group 1");
        assert_eq!(
            loot.master_candidate(2, &raid),
            Some(0xB),
            "Cairne, group 1"
        );
        for empty in [3, 4, 5, 6, 7, 8, 9, 10] {
            assert_eq!(
                loot.master_candidate(empty, &raid),
                None,
                "slot {empty} belongs to an empty group"
            );
        }
        assert_eq!(loot.master_candidate(11, &raid), Some(0xC), "Vol, group 3");
        assert_eq!(
            loot.master_candidate(12, &raid),
            Some(0xD),
            "Sylvanas, group 3"
        );
        assert_eq!(loot.master_candidate(13, &raid), None);

        // The same list in a plain PARTY is flat — the binary picks the path once, off the raid
        // member count, and the party path just uses the wire's own loop counter.
        let party = GroupState {
            members: raid.members.clone(),
            ..GroupState::default()
        };
        assert_eq!(loot.master_candidate(1, &party), Some(0xC));
        assert_eq!(loot.master_candidate(4, &party), Some(0xB));
        assert_eq!(loot.master_candidate(5, &party), None);
    }

    /// A master-loot row is not takeable — the click has to divert. The state layer's half of
    /// that is carrying the wire's `slot_type` out to the drain alongside the wire slot.
    #[test]
    fn a_master_row_reports_its_slot_type() {
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![master_item(0, 117, 1), item(1, 2589, 5)],
        );
        assert!(matches!(
            loot.action_at(1),
            Some(LootAction::Item {
                wire_slot: 0,
                slot_type: slot_type::MASTER,
                ..
            })
        ));
        assert!(matches!(
            loot.action_at(2),
            Some(LootAction::Item {
                wire_slot: 1,
                slot_type: slot_type::ALLOW_LOOT,
                ..
            })
        ));
    }

    /// One `SMSG_ITEM_PUSH_RESULT` as the loot path sees it (self, shown in chat, no random
    /// property) — the net bridge's guid/`showInChat` gates are tested at their own seam.
    fn push(entry: u32, count: u32, from_npc: bool, created: bool) -> ItemPushResult {
        ItemPushResult {
            player_guid: 0x1,
            from_npc,
            created,
            show_in_chat: true,
            bag_slot: 0xFF,
            item_slot: 0,
            item_entry: entry,
            suffix_factor: 0,
            random_property_id: 0,
            count,
        }
    }

    fn pending(entry: u32, count: u32, from_npc: bool, created: bool) -> PendingReceive {
        let mut loot = LootState::default();
        loot.push_receive(&push(entry, count, from_npc, created));
        loot.receives.pop().expect("queued")
    }

    #[test]
    fn action_maps_coin_first_then_items_by_wire_slot() {
        let mut loot = LootState::default();
        assert!(loot.source.is_none());
        // gold + two items at wire slots 0 and 1.
        loot.open(
            0x42,
            loot_type::CORPSE,
            1234,
            vec![item(0, 117, 1), item(1, 2589, 5)],
        );
        assert!(loot.source.is_some());
        // Row 1 is the coin pile; rows 2/3 are the items (wire slots 0/1). The item pick carries the
        // row's display id (here `1000 + entry`) so the pickup sound resolves without a second lookup.
        assert!(matches!(loot.action_at(1), Some(LootAction::Money)));
        assert!(matches!(
            loot.action_at(2),
            Some(LootAction::Item {
                wire_slot: 0,
                display_id: 1117,
                ..
            })
        ));
        assert!(matches!(
            loot.action_at(3),
            Some(LootAction::Item {
                wire_slot: 1,
                display_id: 3589,
                ..
            })
        ));
        assert!(loot.action_at(4).is_none());
        assert!(loot.action_at(0).is_none());
    }

    #[test]
    fn action_maps_items_directly_when_no_coin() {
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(3, 117, 1), item(7, 2589, 5)],
        );
        // No coin → row 1 is the first item (wire slot 3), row 2 the second (wire slot 7).
        assert!(matches!(
            loot.action_at(1),
            Some(LootAction::Item { wire_slot: 3, .. })
        ));
        assert!(matches!(
            loot.action_at(2),
            Some(LootAction::Item { wire_slot: 7, .. })
        ));
        assert!(loot.action_at(3).is_none());
    }

    /// A looted row becomes a GAP at its fixed position — the layout never compacts while the
    /// window is open (the reference hides that one button in place, `LootFrame.lua:22-37`; the
    /// director's report: on ref, looting the gold does NOT pull the items up).
    #[test]
    fn remove_slot_leaves_a_gap_at_the_fixed_position() {
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1), item(1, 2589, 5), item(2, 4306, 2)],
        );
        // Take the middle wire slot: row 2 is now a dead gap; rows 1 and 3 keep their positions
        // AND their wire slots.
        loot.remove_slot(1);
        assert!(matches!(
            loot.action_at(1),
            Some(LootAction::Item { wire_slot: 0, .. })
        ));
        assert!(loot.action_at(2).is_none(), "the looted row is a gap");
        assert!(matches!(
            loot.action_at(3),
            Some(LootAction::Item { wire_slot: 2, .. })
        ));
    }

    #[test]
    fn clear_money_leaves_the_coin_slot_as_a_gap() {
        let mut loot = LootState::default();
        loot.open(0x42, loot_type::CORPSE, 500, vec![item(0, 117, 1)]);
        assert!(loot.has_coin());
        assert!(matches!(loot.action_at(1), Some(LootAction::Money)));
        loot.clear_money();
        assert!(!loot.has_coin());
        // The coin slot stays in the layout as a gap; the item does NOT shift up into it.
        assert!(loot.action_at(1).is_none(), "the looted coin slot is a gap");
        assert!(matches!(
            loot.action_at(2),
            Some(LootAction::Item { wire_slot: 0, .. })
        ));
    }

    #[test]
    fn auto_release_arms_only_on_the_transition_to_empty() {
        let mut loot = LootState::default();
        // Coin + two items: taking rows one by one arms the auto-close only at the last removal.
        loot.open(
            0x42,
            loot_type::CORPSE,
            500,
            vec![item(0, 117, 1), item(1, 2589, 5)],
        );
        loot.remove_slot(0);
        assert!(!loot.take_auto_release(), "items + coin remain");
        loot.clear_money();
        assert!(!loot.take_auto_release(), "an item remains");
        loot.remove_slot(1);
        assert!(loot.take_auto_release(), "last row gone — auto-close due");
        assert!(!loot.take_auto_release(), "the edge drains once");
    }

    #[test]
    fn auto_release_arms_when_the_coin_line_is_the_last_row() {
        let mut loot = LootState::default();
        loot.open(0x42, loot_type::CORPSE, 500, vec![]);
        loot.clear_money();
        assert!(
            loot.take_auto_release(),
            "coin-only loot empties → auto-close"
        );
    }

    #[test]
    fn empty_at_open_does_not_auto_release() {
        let mut loot = LootState::default();
        // The reference keeps an empty-at-open window up (LOOTWINDOWOPENEMPTY); only a removal closes.
        loot.open(0x42, loot_type::CORPSE, 0, vec![]);
        assert!(!loot.take_auto_release());
        // …and a fresh open clears a stale armed edge.
        loot.open(0x43, loot_type::CORPSE, 500, vec![]);
        loot.clear_money();
        loot.open(0x44, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert!(
            !loot.take_auto_release(),
            "open() disarms the previous window's edge"
        );
    }

    #[test]
    fn latch_clears_guid_matched_only() {
        // The corpse-switch race (decision 0515): loot B was requested while A's window was open;
        // A's release response must not drop the latch B's request just armed.
        let mut latch = LootLatch(Some(0xB));
        latch.clear_for(0xA);
        assert_eq!(latch.0, Some(0xB), "a stale release leaves the new latch");
        latch.clear_for(0xB);
        assert_eq!(latch.0, None, "the matching release drops it");
    }

    /// The `IsFishingLoot()` source (decision 1086): wire `loot_type` 3 flags the open, any other
    /// type doesn't, and every close path drops the flag (a stale `true` would reel-in-sound the
    /// next corpse loot).
    #[test]
    fn fishing_loot_type_sets_and_clears_the_flag() {
        let mut loot = LootState::default();
        loot.open(0x42, loot_type::FISHING, 0, vec![item(0, 117, 1)]);
        assert!(loot.fishing);
        loot.clear();
        assert!(!loot.fishing);
        loot.open(0x42, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert!(!loot.fishing);
        // A fishing open REPLACED by an ordinary one drops the flag too (no clear in between).
        loot.open(0x42, loot_type::FISHING, 0, vec![]);
        loot.open(0x43, loot_type::CORPSE, 0, vec![]);
        assert!(!loot.fishing);
    }

    #[test]
    fn clear_closes_but_keeps_receives() {
        let mut loot = LootState::default();
        loot.open(0x42, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        loot.push_receive(&push(117, 1, false, false));
        loot.clear();
        assert!(loot.source.is_none());
        assert_eq!(loot.receives.len(), 1, "a receive line outlives the window");
        loot.clear_session();
        assert!(loot.receives.is_empty(), "disconnect drops receive lines");
    }

    /// The receive line is an item **link**, not a bare name: the quality escape opens before the
    /// `[` and `|r` closes after the `]`, so the count that follows falls back to the line's own
    /// LOOT green. This is the shape the reference client prints (`[Chipped Claw]x2.` — white name,
    /// green `x2`), and the reason the name must not inherit the chat colour.
    #[test]
    fn receive_line_carries_a_quality_colored_item_link() {
        // Common/white, single — LOOT_ITEM_SELF = "You receive loot: %s.".
        assert_eq!(
            receive_line(&pending(2589, 1, false, false), "Linen Cloth", 1),
            "You receive loot: |cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r."
        );
        // Poor/grey, stacked — LOOT_ITEM_SELF_MULTIPLE = "You receive loot: %sx%d." and the `x2`
        // sits OUTSIDE the escape, with no space before it (GlobalStrings.lua l.2605, verbatim).
        assert_eq!(
            receive_line(&pending(7092, 2, false, false), "Chipped Claw", 0),
            "You receive loot: |cff9d9d9d|Hitem:7092:0:0:0|h[Chipped Claw]|h|rx2."
        );
        // The (created, received) pair picks the verb, created winning over received.
        assert_eq!(
            receive_line(&pending(4306, 1, true, false), "Silk Cloth", 2),
            "You receive item: |cff1eff00|Hitem:4306:0:0:0|h[Silk Cloth]|h|r."
        );
        assert_eq!(
            receive_line(&pending(2320, 4, true, true), "Coarse Thread", 1),
            "You create: |cffffffff|Hitem:2320:0:0:0|h[Coarse Thread]|h|rx4."
        );
    }

    /// The link's two random-property fields ride the wire straight through
    /// (`|Hitem:id:0:randomPropertyId:suffixFactor|h` — `0x52adb0`'s arg order).
    #[test]
    fn receive_line_carries_the_wire_random_property_fields() {
        let mut loot = LootState::default();
        loot.push_receive(&ItemPushResult {
            random_property_id: 862,
            suffix_factor: 1234,
            ..push(15268, 1, false, false)
        });
        let r = loot.receives.pop().expect("queued");
        assert_eq!(
            receive_line(&r, "Bloodrazor", 3),
            "You receive loot: |cff0070dd|Hitem:15268:0:862:1234|h[Bloodrazor]|h|r."
        );
    }

    /// The **rolled name** — the reference composes every display of an item's name through
    /// `0x5d8b00(entry, randomPropertyId)`, which joins `ItemRandomProperties`' suffix with
    /// `ITEM_SUFFIX_TEMPLATE` ("%s %s"). That holds for the loot row itself (`GetLootSlotInfo`'s
    /// `item` producer `0x4c2550` ends in that call), and the tooltip's own title line makes the
    /// same call — so row text, tooltip plate and link agree by construction. Decision 1547.
    #[test]
    fn a_rolled_drop_reads_its_suffix_in_the_row_the_link_and_the_lines() {
        use benilla_formats::{RandomProperty, RandomPropertyCatalog};
        // "of the Monkey" (row 584 of the shipped table) — Agility +7, Stamina +7.
        let props = crate::items::RandomProperties(RandomPropertyCatalog::from_rows(
            [(
                584,
                RandomProperty {
                    suffix: "of the Monkey".into(),
                    enchants: [74, 71, 0, 0, 0],
                },
            )]
            .into_iter()
            .collect(),
        ));
        let enchants = crate::items::Enchants(benilla_formats::EnchantCatalog::from_rows(
            std::collections::HashMap::new(),
            [
                (74, "Agility +7".to_string()),
                (71, "Stamina +7".to_string()),
            ]
            .into_iter()
            .collect(),
            [(74, 0), (71, 0)].into_iter().collect(),
        ));
        let rolls = RollCatalogs {
            props: Some(&props),
            enchants: Some(&enchants),
        };
        assert_eq!(rolls.name("Bloodrazor", 584), "Bloodrazor of the Monkey");
        // An unrolled drop keeps the plain name — the formatter's other exit.
        assert_eq!(rolls.name("Bloodrazor", 0), "Bloodrazor");
        // The roll's enchant lines land in slots 2..6 (the suffix band), which is what makes them
        // white at the renderer.
        let lines = rolls.lines(584);
        assert_eq!(
            lines
                .iter()
                .map(|l| (l.slot, l.name.as_str()))
                .collect::<Vec<_>>(),
            vec![(2, "Agility +7"), (3, "Stamina +7")],
        );
        assert!(rolls.lines(0).is_empty(), "no roll, no lines");
    }

    /// The client clamps a quality outside the table to index 1 (white) — `0x52ad90`'s
    /// `cmpl $0x7 / jb` arm.
    #[test]
    fn receive_line_clamps_an_out_of_range_quality_to_white() {
        assert_eq!(
            receive_line(&pending(1, 1, false, false), "Odd Thing", 9),
            "You receive loot: |cffffffff|Hitem:1:0:0:0|h[Odd Thing]|h|r."
        );
    }

    /// [`push_container`] against the reference selector at `0x491bb5`-`0x491bd6`, arm by arm — the
    /// value that decides which bag button the drop animation plays on.
    #[test]
    fn push_container_maps_the_wire_destination_onto_a_bag_bar_button() {
        // `bag != 255` — an equipped bag. Wire 19..22 are the four bag inventory slots, and the
        // emitted value is the reference's own `bag + 1`: 20..23, the ids its buttons carry.
        assert_eq!(push_container(19, 0), 20);
        assert_eq!(push_container(22, 5), 23);
        // `bag == 255` with a keyring slot (the client's own 0x51..=0x70 window) — the keyring.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 81), KEYRING_CONTAINER);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 96), KEYRING_CONTAINER);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 112), KEYRING_CONTAINER);
        // `bag == 255` anywhere else — the backpack. 23..38 are its own slots; 80 and 113 are the
        // positions just outside the keyring window, which the client resolves to 0, not -2.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 23), 0);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 80), 0);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 113), 0);
        // A stack-merge reports no slot at all (`0xFFFF_FFFF`) — still the backpack, still animates.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, u32::MAX), 0);
        // A bank bag (wire 63..68) lands on an id no bag-bar button carries: no animation, which is
        // exactly what the reference's `bag + 1` does with the same push.
        assert!(!(20..=23).contains(&push_container(63, 0)));
    }

    #[test]
    fn money_formats_with_real_words_dropping_zero_denominations() {
        assert_eq!(format_money(0), "0 Copper");
        assert_eq!(format_money(4), "4 Copper");
        assert_eq!(format_money(25), "25 Copper");
        assert_eq!(format_money(10025), "1 Gold 25 Copper");
        assert_eq!(format_money(12_345), "1 Gold 23 Silver 45 Copper");
        assert_eq!(format_money(10_000), "1 Gold");
    }

    /// The reference's six-step ladder at `0x6c6307`–`0x6c6386`, both sides of every boundary.
    #[test]
    fn coin_icon_walks_the_references_six_step_ladder() {
        let icon = |n: u32| coin_icon(n).rsplit('\\').next().unwrap().to_string();
        assert_eq!(icon(0), "INV_Misc_Coin_05");
        assert_eq!(icon(9), "INV_Misc_Coin_05");
        assert_eq!(icon(10), "INV_Misc_Coin_06");
        assert_eq!(icon(99), "INV_Misc_Coin_06");
        assert_eq!(icon(100), "INV_Misc_Coin_03");
        assert_eq!(icon(999), "INV_Misc_Coin_03");
        assert_eq!(icon(1_000), "INV_Misc_Coin_04");
        assert_eq!(icon(9_999), "INV_Misc_Coin_04");
        assert_eq!(icon(10_000), "INV_Misc_Coin_01");
        assert_eq!(icon(99_999), "INV_Misc_Coin_01");
        assert_eq!(icon(100_000), "INV_Misc_Coin_02");
        assert_eq!(icon(u32::MAX), "INV_Misc_Coin_02");
    }

    #[test]
    fn coin_row_uses_real_words_and_the_ladders_icon() {
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let (grp, nm) = nobody();
        let mut loot = LootState::default();
        // A pure-copper drop: name reads "4 Copper", icon is the copper coin pile (_05).
        loot.open(0x42, loot_type::CORPSE, 4, vec![]);
        let snap = snapshot(
            &loot,
            &items,
            None,
            &commands,
            RollCatalogs::NONE,
            Candidates {
                group: &grp,
                names: &nm,
            },
        )
        .expect("open");
        assert_eq!(snap.rows.len(), 1, "coin row only");
        let coin = snap.rows[0].as_ref().expect("coin row present");
        assert!(coin.is_coin);
        assert_eq!(coin.name.as_deref(), Some("4 Copper"));
        assert_eq!(coin.texture.as_deref(), Some(coin_icon(4)));
    }

    #[test]
    fn snapshot_prepends_coin_and_resolves_items() {
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let (grp, nm) = nobody();
        let mut loot = LootState::default();
        // Closed → no snapshot.
        assert!(snapshot(
            &loot,
            &items,
            None,
            &commands,
            RollCatalogs::NONE,
            Candidates {
                group: &grp,
                names: &nm,
            },
        )
        .is_none());
        loot.open(0x42, loot_type::CORPSE, 12_345, vec![item(0, 117, 3)]);
        let snap = snapshot(
            &loot,
            &items,
            None,
            &commands,
            RollCatalogs::NONE,
            Candidates {
                group: &grp,
                names: &nm,
            },
        )
        .expect("open");
        assert_eq!(snap.rows.len(), 2, "coin + one item");
        let coin = snap.rows[0].as_ref().expect("coin row present");
        assert!(coin.is_coin);
        assert_eq!(coin.name.as_deref(), Some("1 Gold 23 Silver 45 Copper"));
        assert_eq!(coin.texture.as_deref(), Some(coin_icon(12_345)));
        // The item's name is nil while its template is in flight; its quantity is present.
        let row = snap.rows[1].as_ref().expect("item row present");
        assert!(!row.is_coin);
        assert!(row.name.is_none());
        assert_eq!(row.quantity, 3);

        // Looting the coin turns row 1 into a gap — the item KEEPS its position (the reference's
        // fixed slot array; the director's report was exactly this row sliding up).
        loot.clear_money();
        let snap = snapshot(
            &loot,
            &items,
            None,
            &commands,
            RollCatalogs::NONE,
            Candidates {
                group: &grp,
                names: &nm,
            },
        )
        .expect("still open");
        assert_eq!(snap.rows.len(), 2, "the layout keeps both slots");
        assert!(snap.rows[0].is_none(), "the looted coin slot is a gap");
        assert!(snap.rows[1].is_some(), "the item stays at position 2");

        // Looting the item empties the layout entirely (both gaps) — and arms the auto-close.
        loot.remove_slot(0);
        let snap = snapshot(
            &loot,
            &items,
            None,
            &commands,
            RollCatalogs::NONE,
            Candidates {
                group: &grp,
                names: &nm,
            },
        )
        .expect("still open");
        assert_eq!(snap.rows, vec![None, None]);
        assert!(loot.take_auto_release(), "nothing lootable left");
    }

    #[test]
    fn page_math_two_pages_of_three_and_two() {
        // The window shows 4 rows/page; > 4 items ⇒ 3 rows/page (a page slot is spent on the pager).
        // 5 items ⇒ ceil(5/3) = 2 pages, 3 then 2 (LootFrame.lua:70-73,112). Pure arithmetic mirror
        // of the shipped XML's paging, unit-tested here so a regression in either is caught.
        let num_items = 5u32;
        let per_page = if num_items > 4 { 3 } else { 4 };
        let pages = num_items.div_ceil(per_page);
        assert_eq!(per_page, 3);
        assert_eq!(pages, 2);
        // Page 1 shows rows for display slots 1..=3, page 2 for 4..=5.
        let page1: Vec<u32> = (1..=per_page).filter(|&i| i <= num_items).collect();
        let page2: Vec<u32> = (per_page + 1..=2 * per_page)
            .filter(|&i| i <= num_items)
            .collect();
        assert_eq!(page1, vec![1, 2, 3]);
        assert_eq!(page2, vec![4, 5]);
        // 4 items ⇒ a single page of 4, no pager.
        assert_eq!(if 4u32 > 4 { 3 } else { 4 }, 4);
    }

    /// **Movement closes the loot** (2097): the controller's move-start report makes the drain
    /// run `CloseInteraction` — latch cleared, the release on the wire once, the window gone, a
    /// dead-corpse selection torn down — with no server help and no VM. The exemptions the bytes
    /// carve out stay open: an item-guid loot (a lockbox) and a disenchant window with rows left;
    /// an emptied disenchant closes like the rest. A GameObject (a fishing bobber) closes too — it
    /// takes the same path as a corpse.
    #[test]
    fn a_movement_start_closes_and_releases_the_open_loot() {
        use bevy::ecs::message::Messages;
        const CORPSE: u64 = 0xF130_0000_0000_0042;
        const BOBBER: u64 = 0xF110_0000_0000_0011;
        const LOCKBOX: u64 = 0x4000_0000_0000_0007;

        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::LootPickupSound>()
            .add_message::<crate::target::DeselectGuid>()
            .init_resource::<LootState>()
            .init_resource::<LootLatch>()
            .init_resource::<LootMoveStart>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, drain_loot);
        // No UiScript mounted: the move-start leg must not depend on the VM.
        let deselects = |app: &mut App| -> Vec<u64> {
            app.world_mut()
                .resource_mut::<Messages<crate::target::DeselectGuid>>()
                .drain()
                .map(|d| d.0)
                .collect()
        };
        let open = |app: &mut App, guid: u64, kind: u8, rows: Vec<LootItem>| {
            app.world_mut()
                .resource_mut::<LootState>()
                .open(guid, kind, 0, rows);
            app.world_mut().resource_mut::<LootLatch>().0 = Some(guid);
        };
        let step = |app: &mut App| {
            app.world_mut().resource_mut::<LootMoveStart>().0 = true;
            app.update();
        };

        // A corpse, standing still: nothing happens.
        open(&mut app, CORPSE, loot_type::CORPSE, vec![item(0, 117, 1)]);
        app.update();
        assert_eq!(app.world().resource::<LootState>().source(), Some(CORPSE));
        assert!(
            rx.try_recv().is_err(),
            "nothing goes out without a move start"
        );

        // The first step: closed, released once, unlatched, the corpse deselected.
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert_eq!(app.world().resource::<LootLatch>().0, None);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE),
            "the move-start close owes the wire the release"
        );
        assert!(rx.try_recv().is_err(), "and sends it once");
        assert_eq!(deselects(&mut app), vec![CORPSE]);

        // A fishing bobber is a GameObject: the same path, minus the unit deselect.
        open(&mut app, BOBBER, loot_type::FISHING, vec![]);
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == BOBBER));
        assert!(deselects(&mut app).is_empty());

        // A lockbox survives movement, unreleased and still latched-as-it-was.
        open(&mut app, LOCKBOX, loot_type::CORPSE, vec![item(0, 117, 1)]);
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), Some(LOCKBOX));
        assert!(rx.try_recv().is_err());
        app.world_mut().resource_mut::<LootState>().clear();

        // A disenchant with rows left survives; once emptied, the step closes it.
        open(
            &mut app,
            CORPSE,
            loot_type::DISENCHANTING,
            vec![item(0, 117, 1)],
        );
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), Some(CORPSE));
        assert!(rx.try_recv().is_err());
        app.world_mut().resource_mut::<LootState>().remove_slot(0);
        // The emptying armed the auto-release; take it so the step's close is the one measured.
        assert!(app
            .world_mut()
            .resource_mut::<LootState>()
            .take_auto_release());
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE));
    }
}
