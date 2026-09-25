//! The app side of the loot window: the net handlers fill [`LootState`], [`feed_loot`] pushes the
//! resolved rows to the VM and fires `LOOT_OPENED`, `LOOT_SLOT_CLEARED`, `UPDATE_MASTER_LOOT_LIST`
//! and `LOOT_CLOSED`, and [`drain_loot`] sends what the Lua asked for.

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

/// `ItemBondingType::BIND_WHEN_PICKED_UP`, the first conjunct of the `LOOT_BIND` confirm: the
/// client compares `item_template + 0x194` against 1.
const BIND_WHEN_PICKED_UP: u32 = 1;

/// The second conjunct: uncommon or better (`0x4c28fb cmp [tmpl+0x1c], 2`).
const BIND_CONFIRM_MIN_QUALITY: u32 = 2;

/// The coin row's quality, 0, so its text is grey: `0x4c23a0`'s coin guard returns 0 at
/// `0x4c23de`, and `LootFrame_Update` has no coin case (`LootFrame.lua:81-85`).
const COIN_QUALITY: u32 = 0;
/// 1.12's `GOLD`, `SILVER` and `COPPER` (`GlobalStrings.lua:2025`, `:3465`, `:865`).
const GOLD_WORD: &str = "Gold";
const SILVER_WORD: &str = "Silver";
const COPPER_WORD: &str = "Copper";
/// Frames a push waits for its item template before it is dropped, about 2 s at 60 fps.
const RECEIVE_MAX_TRIES: u16 = 120;
/// The slots `OnItemPush` treats as the keyring (`0x491bc3`/`0x491bc8`, 0x51..=0x70): the
/// descriptor's full 32 positions, wider than the 16 vmangos uses from `KEYRING_SLOT_START` 81.
const PUSH_KEYRING_SLOTS: std::ops::RangeInclusive<u32> = 81..=112;

/// A push waiting on its item template. `OnItemPush 0x491a60` defers both of its outputs the same
/// way: on a cache miss (`0x491a93`) it re-enters from the cache callback `0x491ee0`.
struct PendingReceive {
    /// Crafted or conjured: "You create: …".
    created: bool,
    entry: u32,
    count: u32,
    from_npc: bool,
    random_property_id: u32,
    suffix_factor: u32,
    /// `showInChat` gates the line only: `ITEM_PUSH` fires (`0x491be8`) before the chat test
    /// (`0x491bf3`), so a silent push still animates.
    in_chat: bool,
    container: i64,
    tries: u16,
}

/// `ITEM_PUSH`'s `arg1`, the reference's selector at `0x491bb5`-`0x491bd6`: a wire bag other than
/// 255 gives `bag + 1` (20..=23 for an equipped bag, the ids its bar buttons carry), a keyring slot
/// gives [`KEYRING_CONTAINER`], and anything else 0, the backpack.
fn push_container(bag: u8, slot: u32) -> i64 {
    if bag != BAG_PLAYER_INVENTORY {
        return i64::from(bag) + 1;
    }
    if PUSH_KEYRING_SLOTS.contains(&slot) {
        return KEYRING_CONTAINER;
    }
    0
}

/// The open loot as the wire delivered it, plus the push queue, which outlives the window. The
/// layout is fixed at open: a looted row is a gap, as `LOOT_SLOT_CLEARED` hides its button in
/// place and `numLootItems` is read once at `OnShow` (`LootFrame.lua:22-37`, `:132`).
#[derive(Resource, Default)]
pub(crate) struct LootState {
    /// The looted object; `None` when no loot is open.
    source: Option<u64>,
    /// The coin pile in copper, 0 once taken.
    gold: u32,
    /// Whether layout position 1 is the coin pile: fixed at open, kept after the coin is taken.
    coin_slot: bool,
    /// The item rows in wire order; never shrinks while open, a taken row lands in `taken`.
    items: Vec<LootItem>,
    /// Wire slots taken by anyone (`SMSG_LOOT_REMOVED`), left as gaps.
    taken: Vec<u8>,
    receives: Vec<PendingReceive>,
    /// A removal just emptied the window, so the client releases it, as the reference does on the
    /// last slot (`0x4c2a70` → `0x48f200`); vmangos does not. Never set at open: an empty window
    /// stays up (`LOOTWINDOWOPENEMPTY`).
    auto_release: bool,
    /// Wire `loot_type` 3, for `IsFishingLoot()`: `LootFrame_OnShow`'s reel-in sound and icon.
    fishing: bool,
    /// The open window's master-loot candidates (`SMSG_LOOT_MASTER_LIST`), in wire order.
    master_candidates: Vec<u64>,
    /// The row a `LOOT_BIND_CONFIRM` is open for, the reference's `[0x847cec]`: set by a click
    /// (`0x4c2790`), cleared by the confirmed send (`0x4c281a`) and per window (`0x4c1df5`).
    /// Deviation: it is the display row, where the reference's is the item index plus one
    /// (`0x4c2885`), so an addon's `GetLootSlotInfo(arg1)` reads the row the dialog is for.
    pending_bind_confirm: Option<u32>,
    /// The list staged for the next open: vmangos sends it from inside `Player::SendLoot`
    /// (`Player.cpp:8080`), just ahead of its response. Staging keeps it out of later windows.
    pending_master_candidates: Vec<u64>,
    /// The wire `loot_type` (`0x4c2740`); a disenchant window with rows left survives movement.
    loot_type: u8,
}

/// The client's candidate array: 40 slots at `0xc4dc38`, bound-checked by the getter `0x61c660`.
const MASTER_LOOT_CANDIDATE_SLOTS: usize = 40;
/// A raid subgroup's block within it (`0x61c609`).
const MEMBERS_PER_RAID_GROUP: usize = 5;

/// A clicked row resolved: the coin pile, or an item at its wire loot slot.
enum LootAction {
    Money,
    Item {
        wire_slot: u8,
        display_id: u32,
        item_id: u32,
        /// `MASTER` opens the master-loot dropdown instead of a take.
        slot_type: u8,
    },
}

impl LootState {
    /// Opens or replaces the window; quest items ride the same rows at `slot = items.len() + i`.
    pub(crate) fn open(&mut self, source: u64, loot_type: u8, gold: u32, items: Vec<LootItem>) {
        self.source = Some(source);
        self.gold = gold;
        self.coin_slot = gold > 0; // fixed for the window's lifetime
        self.items = items;
        self.taken.clear();
        self.auto_release = false; // an empty window at open stays open
        self.pending_bind_confirm = None; // `0x4c1df5`: the copier resets the stash
        self.loot_type = loot_type;
        self.fishing = loot_type == benilla_protocol::messages::loot_type::FISHING;
        self.master_candidates = std::mem::take(&mut self.pending_master_candidates);
    }

    /// Stages a list for the next open; an open window takes it at once, as a refresh.
    pub(crate) fn set_master_candidates(&mut self, candidates: Vec<u64>) {
        if self.source.is_some() {
            self.master_candidates.clone_from(&candidates);
        }
        self.pending_master_candidates = candidates;
    }

    /// `SMSG_LOOT_REMOVED`: the row becomes a gap; an emptied window arms the auto-close.
    pub(crate) fn remove_slot(&mut self, wire_slot: u8) {
        if self.items.iter().any(|it| it.slot == wire_slot) && !self.taken.contains(&wire_slot) {
            self.taken.push(wire_slot);
        }
        self.arm_auto_release();
    }

    /// `SMSG_LOOT_CLEAR_MONEY`: the coin row becomes a gap; an emptied window arms the auto-close.
    pub(crate) fn clear_money(&mut self) {
        self.gold = 0;
        self.arm_auto_release();
    }

    fn arm_auto_release(&mut self) {
        if self.source.is_some() && self.is_empty() {
            self.auto_release = true;
        }
    }

    fn take_auto_release(&mut self) -> bool {
        std::mem::take(&mut self.auto_release)
    }

    /// Queues a push the net handler has already checked is ours.
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

    /// Closes the window; queued pushes outlive it.
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

    /// Session end: drops the window and the queued pushes.
    pub(crate) fn clear_session(&mut self) {
        self.clear();
        self.receives.clear();
    }

    /// Queued pushes, for the net handler's self-gate test.
    #[cfg(test)]
    pub(crate) fn pending_receive_count(&self) -> usize {
        self.receives.len()
    }

    fn has_coin(&self) -> bool {
        self.gold > 0
    }

    /// The open loot's guid; the chest lid watcher in [`crate::go_anim`] reads it too.
    pub(crate) fn source(&self) -> Option<u64> {
        self.source
    }

    /// Nothing lootable left: the reference's empty check `0x4c2a70`.
    fn is_empty(&self) -> bool {
        !self.has_coin() && self.items.iter().all(|it| self.taken.contains(&it.slot))
    }

    /// A 1-based display row of the fixed layout; a gap answers `None`, like the hidden button.
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

    /// The candidate slots as `0x61c550` lays them out: wire order in a party (`0x61c5b9`); in a
    /// raid, each guid in the first free slot of its subgroup's block of five
    /// (`0x61c5c9`-`0x61c637`), holes kept for `GroupLootDropDown_Initialize`'s "Group N" submenus
    /// (`LootFrame.lua:197-213`). The feed and the drain share it, so an index means one candidate.
    fn placed_candidates(&self, group: &GroupState) -> Vec<Option<u64>> {
        if group.group_type != GROUPTYPE_RAID {
            return self.master_candidates.iter().copied().map(Some).collect();
        }
        let mut slots: Vec<Option<u64>> = vec![None; MASTER_LOOT_CANDIDATE_SLOTS];
        for &guid in &self.master_candidates {
            // A guid missing from the roster is ours: `SMSG_GROUP_LIST` lists only the others,
            // and `Group::MasterLoot` includes us.
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
            slots.pop(); // trailing empties read as nil either way
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

/// The loot CVars. `autoLootDefault` is not a 1.12 CVar: 1.12 auto-loots only on a shift-click.
/// [`feed_loot`] sweeps the rows engine-side at the open, as the reference does, and a held Shift
/// inverts the setting. `showLootSpam` is 1.12's Detailed Loot Information checkbox (`0xb4e2bc`,
/// registered at `0x48fd1c`, default `"1"`), read only by the loot-roll line composers.
#[derive(Resource)]
pub(crate) struct LootConfig {
    pub(crate) auto_loot: bool,
    pub(crate) show_loot_spam: bool,
}

impl Default for LootConfig {
    fn default() -> Self {
        Self {
            // Off leaves 1.12's shift-click as the only auto-loot.
            auto_loot: false,
            // The reference's registered `"1"`.
            show_loot_spam: true,
        }
    }
}

/// The loot-target latch, the reference's `[player+0x1d28]` guid: a loot session is open on this
/// object; kneeling is [`LootKneel`]'s question. Armed at the `CMSG_LOOT` send
/// (`0x5df253`/`0x5df40d`), at `SMSG_SPELL_GO` for an `OPEN_LOCK` on a chest (`0x6e831b`), by an
/// admitted response (`0x5eb900`), and at the `CMSG_OPEN_ITEM` send (`0x5edcc0`), which is what
/// admits vmangos' type-1 answer for the item.
///
/// Deviation: our close paths clear it guid-matched, where `CloseInteraction` clears it
/// unconditionally (`0x48f2c9`), so closing an old window keeps a newer `CMSG_LOOT`'s latch.
#[derive(Resource, Default)]
pub(crate) struct LootLatch(pub(crate) Option<u64>);

/// Whether the character kneels at the latched object, the reference's predicate B `0x612710`:
/// yes for a corpse or any GameObject but a fishing bobber, no for a live unit, an item or an
/// unresolved guid. Recomputed each frame between the net drain and the anim driver, as the
/// reference force-plays the pose at the arm itself (`0x5ed619`).
#[derive(Resource, Default)]
pub(crate) struct LootKneel(pub(crate) bool);

/// `GAMEOBJECT_TYPE_ID` 17, `FISHINGNODE`: the one GameObject type that does not kneel
/// (`0x612772`).
const GO_TYPE_FISHINGNODE: i32 = 17;

/// Recomputes [`LootKneel`]; `pub(crate)` so [`crate::creature_anim`]'s driver can order after it.
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
            // `0x61278c`: a unit kneels once its health reads 0 off the descriptor, where an
            // unsent field is 0 too; not `unit_is_dead`, whose `max_health` guard it lacks.
            benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player => {
                store.0.unit_health().unwrap_or(0) == 0
            }
            // An item never kneels (`0x612797`); we stream none, so a lockbox latch never resolves.
            _ => false,
        });
    if kneel.0 != allowed {
        kneel.0 = allowed;
    }
}

impl LootLatch {
    /// Drops the latch only if it still names `guid`.
    pub(crate) fn clear_for(&mut self, guid: u64) {
        if self.0 == Some(guid) {
            self.0 = None;
        }
    }
}

/// A movement start that closes the loot: the reference's guard `0x60e990`, called by every
/// movement-start emitter, turning included, but not by mouse-look facing. No loot target reaches
/// the range gate `0x493230`, so there is no distance leash. vmangos also releases on movement
/// (`MovementHandler.cpp:1104`), but the close is the client's.
#[derive(Resource, Default)]
pub(crate) struct LootMoveStart(pub(crate) bool);

pub(crate) struct UiLootPlugin;

/// Applies a loot CVar change.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut loot: ResMut<LootConfig>) {
    match ev.key().as_str() {
        "autolootdefault" => loot.auto_loot = ev.flag(),
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
                    // Feed before the input pass and drain after it: an open shows, and a click
                    // sends, the same frame. The drain's deselect lands before the selection
                    // writers read it, and their teardown's close finds this frame's window.
                    feed_loot.in_set(UiFeed),
                    drain_loot
                        .after(UiInput)
                        .before(crate::target::TargetUpdate),
                    // After the net drain and the selection writers, which arm and drop the
                    // latch; the anim driver orders after this.
                    resolve_loot_kneel
                        .after(benilla_world::schedule::WorldStage::Net)
                        .after(crate::target::TargetUpdate),
                ),
            );
    }
}

/// The coin row's name: each nonzero denomination as "<n> <Word>", joined by spaces. The
/// reference's composer `0x6c6260` emits the same "%d %s" parts but separates them with a newline
/// (`0x835144`).
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

/// The coin row's icon: the reference's six-step ladder (`0x4c2460` → `0x6c62d0`), shared with
/// `GetCoinIcon` and the money cursor.
fn coin_icon(copper: u32) -> &'static str {
    benilla_ui::script::coin_icon(i64::from(copper))
}

/// One wire row for the Lua: the icon from the wire display id, the rest from the item template.
/// The link's suffix factor is 0, as the wire's `randomSuffix` always is (`LootMgr.cpp:842`).
fn resolve_item(
    item: &LootItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
    rolls: RollCatalogs,
) -> LootRow {
    let (name, quality, link) = match items.template(item.item_id, 0, commands) {
        Some(t) => {
            // The rolled name: the reference composes every display of it through `0x5d8b00`.
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
        // The raw id, as the client's loot record keeps it; the tooltip resolves it (`0x52b7bf`).
        random_property_id: item.random_property_id,
    }
}

/// Whether a row still waits on its item template: the reference's pending-query counter
/// `[0xb71b44]`. Deviation: a negative answer releases the row, where the reference's callback
/// `0x4c2ac0` returns on a failed query and the window never opens, because an entry the server
/// cannot describe would otherwise keep the loot shut for good.
fn templates_outstanding(items: &Items, snap: &LootSnapshot) -> bool {
    snap.rows.iter().flatten().any(|r| {
        !r.is_coin
            && r.item_id != 0
            && r.name.is_none()
            && !items.template_answered_unknown(r.item_id)
    })
}

/// The Lua snapshot: one entry per slot of the fixed layout, coin first, a looted slot `None`.
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
            // 0: `0x4c22e0`'s coin guard returns before the record read (`0x4c22fd`).
            quantity: 0,
            quality: Some(COIN_QUALITY),
            is_coin: true,
            item_id: 0,
            // No item behind it: a modified click finds nil.
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

/// A candidate's name sources: the roster for everyone else, the name cache for our own guid,
/// which vmangos lists as a candidate (`Group.cpp:928-937`) but never sends as a member.
#[derive(Clone, Copy)]
struct Candidates<'a> {
    group: &'a GroupState,
    names: &'a NameCache,
}

impl Candidates<'_> {
    /// The candidate slots as names, an empty slot and an unresolved name both `None`: the
    /// binding `0x4c2f10` pushes nil on a name-cache miss (`0x4c2f91`), and the cache callback
    /// `0x4c2fb0` fires `UPDATE_MASTER_LOOT_LIST` when the name lands.
    fn names_for(&self, loot: &LootState) -> Vec<Option<String>> {
        loot.placed_candidates(self.group)
            .into_iter()
            .map(|slot| slot.and_then(|guid| self.name(guid)))
            .collect()
    }

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

/// `OnItemPush`'s self line (`0x491bfb`): its `%s` is an item link (`0x52adb0`, at `0x491c43`),
/// so a count after it takes the line's own colour. The strings are `GlobalStrings.lua:2599-2605`,
/// with no space before `x%d`; created wins over received (`0x491c04`-`0x491c1b`).
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

/// Emits each push whose template has landed, in `OnItemPush`'s order: `ITEM_PUSH` first
/// (`0x491be8`), then the chat line if the wire asked for it. Deviation: a push is dropped after
/// [`RECEIVE_MAX_TRIES`] frames, where the reference waits on the item-cache callback with no
/// timeout, because an entry the server never describes would otherwise stay queued.
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
        let resolved = items
            .template(r.entry, 0, commands)
            .map(|t| (t.name.clone(), t.quality, t.display_info_id));
        match resolved {
            Some((name, quality, display_id)) => {
                // Ungated by `in_chat`. `arg2` is the icon path the reference builds from the
                // display record (`0x491baa`).
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

/// Emits the queued pushes, then pushes the loot snapshot and fires the loot events on a change.
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
    // The random-suffix catalogs: a loot slot has no item object to carry the roll.
    props: Option<Res<crate::items::RandomProperties>>,
    enchants: Option<Res<crate::items::Enchants>>,
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
            // No window until every template has landed: the copier fires nothing while a query
            // is pending (`0x4c1e9f`), and the cache callback `0x4c2ac0` fires `LOOT_OPENED` (or
            // the auto-loot sweep) at the last answer. Not advancing `last` retries next frame.
            if templates_outstanding(&items, snap) {
                return;
            }
            script.fire_event("LOOT_OPENED", vec![]);
            // Auto-loot (`LootConfig`), inverted by a held Shift: every row gets a hand pick's
            // sends, and emptying the window auto-releases it.
            let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
            if cfg.auto_loot != shift {
                let mut bind_confirm_fired = false;
                for index in 1..=snap.rows.len() as u32 {
                    match loot.action_at(index) {
                        Some(LootAction::Money) => {
                            let _ = commands.0.send(ClientCommand::LootMoney);
                        }
                        // Only `ALLOW_LOOT` rows: the reference's sweep skips any other slot type
                        // (`0x4c2180`/`0x4c2196`).
                        Some(LootAction::Item {
                            wire_slot,
                            display_id,
                            item_id,
                            slot_type,
                        }) if slot_type == slot_type::ALLOW_LOOT => {
                            // A hand click's bind gate plus a one-shot latch (`0x4c21c2`,
                            // `0x4c21e2`): later bind-on-pickup rows stay, untaken and unasked.
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
        // One `LOOT_SLOT_CLEARED` per row that went, with its 1-based row: the stock handler hides
        // that button and pages down when the page empties (`LootFrame.lua:22-50`).
        (Some(before), Some(after)) => {
            for (i, was) in before.rows.iter().enumerate() {
                let gone = was.is_some() && after.rows.get(i).is_none_or(Option::is_none);
                if gone {
                    script.fire_event("LOOT_SLOT_CLEARED", vec![ScriptValue::Int(i as i64 + 1)]);
                }
            }
            // Refreshes an open dropdown in place (`LootFrame.lua:62-64`).
            if before.master_candidates != after.master_candidates {
                script.fire_event("UPDATE_MASTER_LOOT_LIST", vec![]);
            }
        }
        (Some(_), None) => script.fire_event("LOOT_CLOSED", vec![]),
        (None, None) => {}
    }
    *last = fresh;
}

/// Whether a take must raise `LOOT_BIND_CONFIRM` first (`0x4c28f2`/`0x4c28fb`): bind on pickup
/// and uncommon or better. An unresolved template answers false, as the reference's cache peek.
fn bind_confirm_required(items: &Items, commands: &NetCommands, item_id: u32) -> bool {
    items
        .template(item_id, 0, commands)
        .is_some_and(|t| t.bonding == BIND_WHEN_PICKED_UP && t.quality >= BIND_CONFIRM_MIN_QUALITY)
}

/// `CloseInteraction 0x48f200` past its early returns, the one close every loot close shares: the
/// latch drops (`0x48f2c9`), the release goes out when `release` is given (the `cl` argument,
/// `0x48f2da`) and the frame closes (`0x48f33d`). Returns the closed source for
/// [`DeadUnitDeselect`], the close's last step.
pub(crate) fn close_interaction(
    loot: &mut LootState,
    latch: &mut LootLatch,
    release: Option<&NetCommands>,
) -> Option<u64> {
    let source = loot.source?;
    latch.clear_for(source);
    if let Some(commands) = release {
        let _ = commands.0.send(ClientCommand::LootRelease { guid: source });
    }
    loot.clear();
    Some(source)
}

/// `CloseInteraction`'s last step (`0x48f34f`–`0x48f369`): a closed source that is a unit, a
/// player included, with `UNIT_FIELD_HEALTH` at or below 0 is deselected if it is still the
/// selection (`0x493910(guid, 1)`). It reads the health, never the loot type, so a skinned corpse
/// loses the selection and a live pickpocket target keeps it; an unstreamed source is left alone.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DeadUnitDeselect<'w, 's> {
    index: Res<'w, crate::net::GuidIndex>,
    objects: Query<
        'w,
        's,
        (
            &'static crate::net::NetEntity,
            &'static crate::net::ObjectStore,
        ),
    >,
    asks: MessageWriter<'w, crate::target::DeselectGuid>,
}

impl DeadUnitDeselect<'_, '_> {
    pub(crate) fn after_close(&mut self, closed: Option<u64>) {
        let Some(source) = closed else {
            return;
        };
        let dead_unit = self
            .index
            .0
            .get(&source)
            .and_then(|&e| self.objects.get(e).ok())
            .is_some_and(|(net_entity, store)| {
                matches!(
                    net_entity.kind,
                    benilla_protocol::EntityKind::Unit | benilla_protocol::EntityKind::Player
                ) && (store.0.unit_health().unwrap_or(0) as i32) <= 0 // signed, `jg` at `0x48f360`
            });
        if dead_unit {
            self.asks.write(crate::target::DeselectGuid(source));
        }
    }
}

/// `CloseInteraction 0x48f200(cl=1, dl=1, 0)` on a movement start. A disenchant window with rows
/// left (`0x48f24a`) and an item's loot (`0x48f2ab`) survive; anything else takes the shared close.
fn close_on_move_start(
    loot: &mut LootState,
    latch: &mut LootLatch,
    commands: &NetCommands,
) -> Option<u64> {
    use benilla_protocol::guid;
    use benilla_protocol::messages::loot_type;
    let source = loot.source()?;
    if loot.loot_type == loot_type::DISENCHANTING && !loot.is_empty() {
        return None;
    }
    if guid::is_item(source) {
        return None;
    }
    debug!("ui_loot: movement started with {source:#x} open — release");
    close_interaction(loot, latch, Some(commands))
}

/// Sends what the Lua asked for, as the take dispatcher `0x4c2790(slot, flag)`: a row click
/// (`BenillaTakeLootSlot`, flag 0) and the `LOOT_BIND` confirm (`LootSlot`, flag 1) stay apart,
/// so a second click on a bind-on-pickup row asks again.
fn drain_loot(
    script: Option<NonSendMut<UiScript>>,
    mut loot: ResMut<LootState>,
    mut latch: ResMut<LootLatch>,
    commands: Res<NetCommands>,
    mut pickup: MessageWriter<crate::sound::LootPickupSound>,
    group: Res<GroupState>,
    items: Res<Items>,
    // The move-start close runs before the VM check: it needs no Lua.
    mut move_start: ResMut<LootMoveStart>,
    mut dead: DeadUnitDeselect,
) {
    if std::mem::take(&mut move_start.0) {
        dead.after_close(close_on_move_start(&mut loot, &mut latch, &commands));
    }
    let Some(mut script) = script else {
        return;
    };
    for index in script.take_loot_picks() {
        match loot.action_at(index) {
            Some(LootAction::Money) => {
                // No sound here: the coinage watcher (`sound::money`) plays it as the purse rises.
                debug!("ui_loot: loot coin (row {index})");
                let _ = commands.0.send(ClientCommand::LootMoney);
            }
            // A master-loot row opens the candidate dropdown instead: the dispatcher branches on
            // the slot type (`0x4c28a9`), and the Lua `OnClick` has already stashed the anchor.
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
                // The bind-on-pickup deferral (`0x4c28f2`-`0x4c2920`): stash the row, fire the
                // event, send nothing, not even the pickup sound.
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
                // At the click, before any answer (`0x4c2926`): the item's ItemGroupSounds kit 0.
                pickup.write(crate::sound::LootPickupSound { display_id });
            }
            None => debug!("ui_loot: BenillaTakeLootSlot({index}) out of range — ignored"),
        }
    }

    // `LootSlot`, the confirm arm (`0x4c27c0`): it sends only for the pending row and clears the
    // stash on the send (`0x4c281a`), so a doubled OnAccept cannot loot twice.
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
            // The row went under the dialog: bail without clearing the stash, as `0x4c27d7` does.
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
    // `GiveMasterLoot(slot, candidateIndex)`, both 1-based; one that does not resolve is dropped.
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
        // Only a master row: the sender `0x4c2940` re-reads the slot type and bails unless it is 2.
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
    // `CloseLoot` (`0x4c2ec0` → `0x48f200(cl=1, dl=0)`), which stock `LootFrame_OnHide` calls.
    if script.take_loot_close() {
        let closed = close_interaction(&mut loot, &mut latch, Some(&commands));
        if let Some(guid) = closed {
            debug!("ui_loot: release loot {guid:#x}");
        }
        dead.after_close(closed);
    }
    // The last-row auto-close (`0x4c1f8d`, `0x4c2785` → `0x48f200(cl=1, dl=0)`). The feed then
    // fires `LOOT_CLOSED`, and the `CloseLoot()` from `OnHide` finds no source.
    if loot.take_auto_release() {
        let closed = close_interaction(&mut loot, &mut latch, Some(&commands));
        if let Some(guid) = closed {
            debug!("ui_loot: loot emptied — auto-release {guid:#x}");
        }
        dead.after_close(closed);
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

    fn nobody() -> (GroupState, NameCache) {
        (GroupState::default(), NameCache::default())
    }

    /// Descriptor field indices the predicate-B table reads.
    const F_GO_TYPE_ID: u16 = 21;
    const F_UNIT_HEALTH: u16 = 22;

    /// Predicate B's answer for one latched object; `None` latches a guid that does not resolve.
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

    #[test]
    fn predicate_b_decides_which_loot_targets_are_knelt_at() {
        // A GameObject other than the bobber: a chest (3), a fishing hole (25).
        assert!(kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 3)]
        ))));
        assert!(kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 25)]
        ))));
        // `0x612772`: type 17, `FISHINGNODE`.
        assert!(!kneels_at(Some((
            EntityKind::GameObject,
            &[(F_GO_TYPE_ID, 17)]
        ))));
        // `0x61278c`: a corpse kneels, a live (pickpocketed) target does not.
        assert!(kneels_at(Some((EntityKind::Unit, &[(F_UNIT_HEALTH, 0)]))));
        assert!(!kneels_at(Some((EntityKind::Unit, &[(F_UNIT_HEALTH, 1)]))));
        // `0x612732`: an unresolved guid, which an item latch is for us.
        assert!(!kneels_at(None));
    }

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

    // ── The soulbind confirm ──────────────────────────────────────────────────
    //
    // vmangos `item_template` rows: Felstriker a BoP epic, Flurry Axe a BoE epic, Tough Jerky a
    // plain white; 9999 is a synthetic white BoP, the quality control.
    const FELSTRIKER: u32 = 12590;
    const FLURRY_AXE: u32 = 871;
    const TOUGH_JERKY: u32 = 117;
    const WHITE_BOP: u32 = 9999;
    /// Brutality Blade, a second BoP epic, for the sweep's one-dialog latch.
    const SECOND_BOP: u32 = 18832;

    /// `rows` open on a corpse with every template landed, then `lua` and one drain.
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
            .init_resource::<crate::net::GuidIndex>()
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

    /// `0x4c2ac0` fires `LOOT_OPENED` once, at the last answer; here a negative one opens it too.
    #[test]
    fn the_window_waits_for_every_item_template() {
        for answer in [
            Some(ItemInfo {
                quality: 1,
                ..crate::items::test_template("Thin Cloth Gloves")
            }),
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
                // Scheduled, not `run_system_once`: the `Local` memo must persist across frames.
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

            // A second pass changes nothing: the deferral is not a one-shot.
            app.update();
            assert_eq!(opens(&mut app), 0);

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

    fn sent(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<ClientCommand> {
        rx.try_iter().collect()
    }

    fn pending_confirm(app: &App) -> Option<u32> {
        app.world().resource::<LootState>().pending_bind_confirm
    }

    /// The click arm's deferral (`0x4c28f2`-`0x4c2920`).
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

    /// The confirm arm (`0x4c27c0`).
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

    /// The flag-1 arm opens with `cmp edi,[0x847cec]`, the pending row.
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

    /// The display row on both halves (the deviation on `LootState::pending_bind_confirm`).
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

        // The accept, with the same number, reaches the right wire slot.
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

    /// `0x4c27d7`: the continuation returns on an emptied record.
    #[test]
    fn an_accept_for_a_row_that_was_taken_away_sends_nothing() {
        let (mut app, rx) = drain_with(0, vec![item(2, FELSTRIKER, 1)], "BenillaTakeLootSlot(1)");
        assert_eq!(pending_confirm(&app), Some(1));

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

    /// `0x4c1df5`: the response copier resets the stash.
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

    /// The sweep's one-shot latch (`0x4c21c2`, `0x4c21e2`), over two BoP epics and a white.
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

    /// The same row stamped `MASTER`, as vmangos sends every row under master loot.
    fn master_item(slot: u8, entry: u32, count: u32) -> LootItem {
        LootItem {
            slot_type: slot_type::MASTER,
            ..item(slot, entry, count)
        }
    }

    #[test]
    fn the_candidate_list_arrives_before_its_window_and_does_not_outlive_it() {
        let mut loot = LootState::default();
        // A default GroupState is a party (`group_type` 0): the flat placement path.
        let party = GroupState::default();

        loot.set_master_candidates(vec![0xA, 0xB, 0xC]);
        assert_eq!(
            loot.master_candidate(1, &party),
            None,
            "no window, no candidates"
        );

        loot.open(0x42, loot_type::CORPSE, 0, vec![master_item(0, 117, 1)]);
        assert_eq!(loot.master_candidate(1, &party), Some(0xA));
        assert_eq!(loot.master_candidate(3, &party), Some(0xC));
        assert_eq!(loot.master_candidate(4, &party), None, "past the end");
        assert_eq!(
            loot.master_candidate(0, &party),
            None,
            "0 is not a Lua index"
        );

        // A refresh while the window is up applies in place.
        loot.set_master_candidates(vec![0xA, 0xB]);
        assert_eq!(loot.master_candidate(2, &party), Some(0xB));
        assert_eq!(loot.master_candidate(3, &party), None, "the list shrank");

        loot.clear();
        loot.open(0x43, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert_eq!(
            loot.master_candidate(1, &party),
            None,
            "a fresh window starts empty"
        );
    }

    /// `0x61c550`'s raid placement: each guid in the first free slot of its subgroup's block.
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
        // The wire order interleaves the subgroups; placement ignores it.
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

        // In a party the same list is flat, in wire order.
        let party = GroupState {
            members: raid.members.clone(),
            ..GroupState::default()
        };
        assert_eq!(loot.master_candidate(1, &party), Some(0xC));
        assert_eq!(loot.master_candidate(4, &party), Some(0xB));
        assert_eq!(loot.master_candidate(5, &party), None);
    }

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

    /// Our own push, shown in chat, with no random property.
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
        loot.open(
            0x42,
            loot_type::CORPSE,
            1234,
            vec![item(0, 117, 1), item(1, 2589, 5)],
        );
        assert!(loot.source.is_some());
        // Row 1 is the coin, rows 2 and 3 the items; the helper's display id is `1000 + entry`.
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

    #[test]
    fn remove_slot_leaves_a_gap_at_the_fixed_position() {
        let mut loot = LootState::default();
        loot.open(
            0x42,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1), item(1, 2589, 5), item(2, 4306, 2)],
        );
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
        assert!(loot.action_at(1).is_none(), "the looted coin slot is a gap");
        assert!(matches!(
            loot.action_at(2),
            Some(LootAction::Item { wire_slot: 0, .. })
        ));
    }

    #[test]
    fn auto_release_arms_only_on_the_transition_to_empty() {
        let mut loot = LootState::default();
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
        // An empty window at open stays up (`LOOTWINDOWOPENEMPTY`).
        loot.open(0x42, loot_type::CORPSE, 0, vec![]);
        assert!(!loot.take_auto_release());
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
        // Loot B requested while A was open: A's release response keeps B's latch.
        let mut latch = LootLatch(Some(0xB));
        latch.clear_for(0xA);
        assert_eq!(latch.0, Some(0xB), "a stale release leaves the new latch");
        latch.clear_for(0xB);
        assert_eq!(latch.0, None, "the matching release drops it");
    }

    #[test]
    fn fishing_loot_type_sets_and_clears_the_flag() {
        let mut loot = LootState::default();
        loot.open(0x42, loot_type::FISHING, 0, vec![item(0, 117, 1)]);
        assert!(loot.fishing);
        loot.clear();
        assert!(!loot.fishing);
        loot.open(0x42, loot_type::CORPSE, 0, vec![item(0, 117, 1)]);
        assert!(!loot.fishing);
        // A fishing open replaced by another, with no clear between, drops it too.
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

    /// The count after the link's `|r` takes the line's own colour.
    #[test]
    fn receive_line_carries_a_quality_colored_item_link() {
        // Common, single: `LOOT_ITEM_SELF`.
        assert_eq!(
            receive_line(&pending(2589, 1, false, false), "Linen Cloth", 1),
            "You receive loot: |cffffffff|Hitem:2589:0:0:0|h[Linen Cloth]|h|r."
        );
        // Poor, stacked: `LOOT_ITEM_SELF_MULTIPLE`, with no space before `x2`.
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

    /// `|Hitem:id:0:randomPropertyId:suffixFactor|h`, `0x52adb0`'s argument order.
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

    /// `0x5d8b00` joins the suffix by `ITEM_SUFFIX_TEMPLATE`, for the row (`0x4c2550`) and tooltip.
    #[test]
    fn a_rolled_drop_reads_its_suffix_in_the_row_the_link_and_the_lines() {
        use benilla_formats::{RandomProperty, RandomPropertyCatalog};
        // "of the Monkey" (row 584 of the shipped table): Agility +7, Stamina +7.
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
        assert_eq!(rolls.name("Bloodrazor", 0), "Bloodrazor");
        // The roll's lines land in slots 2..6, the suffix band the renderer draws white.
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

    /// `0x52ad90` clamps a quality outside the table to 1, white.
    #[test]
    fn receive_line_clamps_an_out_of_range_quality_to_white() {
        assert_eq!(
            receive_line(&pending(1, 1, false, false), "Odd Thing", 9),
            "You receive loot: |cffffffff|Hitem:1:0:0:0|h[Odd Thing]|h|r."
        );
    }

    /// [`push_container`] against the selector at `0x491bb5`-`0x491bd6`, arm by arm.
    #[test]
    fn push_container_maps_the_wire_destination_onto_a_bag_bar_button() {
        // An equipped bag: wire 19..22 give `bag + 1`, 20..23, the ids its buttons carry.
        assert_eq!(push_container(19, 0), 20);
        assert_eq!(push_container(22, 5), 23);
        // `bag == 255` in the keyring window, 0x51..=0x70.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 81), KEYRING_CONTAINER);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 96), KEYRING_CONTAINER);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 112), KEYRING_CONTAINER);
        // Anywhere else is the backpack: its slots 23..38, and 80 and 113 just outside the keyring.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 23), 0);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 80), 0);
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, 113), 0);
        // A stack merge reports no slot (`0xFFFF_FFFF`): the backpack.
        assert_eq!(push_container(BAG_PLAYER_INVENTORY, u32::MAX), 0);
        // A bank bag (wire 63..68) lands on an id no bag-bar button carries.
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

    /// The reference's six-step ladder (`0x6c6307`-`0x6c6386`), both sides of every boundary.
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

        // Looting the coin turns row 1 into a gap; the item keeps its position.
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
        // Four rows a page, three when there are more than four (`LootFrame.lua:70-73`, `:112`).
        let num_items = 5u32;
        let per_page = if num_items > 4 { 3 } else { 4 };
        let pages = num_items.div_ceil(per_page);
        assert_eq!(per_page, 3);
        assert_eq!(pages, 2);
        let page1: Vec<u32> = (1..=per_page).filter(|&i| i <= num_items).collect();
        let page2: Vec<u32> = (per_page + 1..=2 * per_page)
            .filter(|&i| i <= num_items)
            .collect();
        assert_eq!(page1, vec![1, 2, 3]);
        assert_eq!(page2, vec![4, 5]);
        // Four items: one page of four.
        assert_eq!(if 4u32 > 4 { 3 } else { 4 }, 4);
    }

    /// A creature corpse and a live creature, both streamed.
    const CORPSE: u64 = 0xF130_0000_0000_0042;
    const LIVE: u64 = 0xF130_0000_0000_0043;

    /// `drain_loot` scheduled, with [`CORPSE`] streamed at health 0 and [`LIVE`] at 100.
    fn close_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_message::<crate::sound::LootPickupSound>()
            .add_message::<crate::target::DeselectGuid>()
            .init_resource::<LootState>()
            .init_resource::<LootLatch>()
            .init_resource::<LootMoveStart>()
            .init_resource::<GroupState>()
            .init_resource::<Items>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, drain_loot);
        for (guid, health) in [(CORPSE, 0), (LIVE, 100)] {
            let e = app
                .world_mut()
                .spawn((
                    crate::net::NetEntity {
                        kind: EntityKind::Unit,
                        display_id: None,
                        scale: 1.0,
                    },
                    crate::net::ObjectStore(ObjectFields::from_pairs(&[(F_UNIT_HEALTH, health)])),
                ))
                .id();
            app.world_mut()
                .resource_mut::<crate::net::GuidIndex>()
                .0
                .insert(guid, e);
        }
        (app, rx)
    }

    /// The deselects asked for since the last call.
    fn deselects(app: &mut App) -> Vec<u64> {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>()
            .drain()
            .map(|d| d.0)
            .collect()
    }

    /// Opens a window as an admitted response does: the rows and the latch.
    fn open_loot(app: &mut App, guid: u64, kind: u8, gold: u32, rows: Vec<LootItem>) {
        app.world_mut()
            .resource_mut::<LootState>()
            .open(guid, kind, gold, rows);
        app.world_mut().resource_mut::<LootLatch>().0 = Some(guid);
    }

    /// Every close reaches `CloseInteraction`'s last step (`0x48f34f`–`0x48f369`): `CloseLoot`
    /// (`0x4c2ed6`), the empty-window close after the last row (`0x4c1f8d`) or the coin
    /// (`0x4c2785`), each deselecting a dead unit and only a dead unit.
    #[test]
    fn every_loot_close_deselects_a_dead_unit() {
        let (mut app, rx) = close_app();
        let released = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<u64> {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::LootRelease { guid } => Some(guid),
                    _ => None,
                })
                .collect()
        };
        app.insert_non_send_resource(UiScript::new().unwrap());
        let close_loot = |app: &mut App| {
            app.world_mut()
                .non_send_resource_mut::<UiScript>()
                .run("CloseLoot()")
                .unwrap();
            app.update();
        };

        // `CloseLoot`, as the close button, Escape or any other hide reaches it.
        open_loot(
            &mut app,
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
        );
        close_loot(&mut app);
        assert_eq!(released(&rx), [CORPSE]);
        assert_eq!(app.world().resource::<LootLatch>().0, None);
        assert_eq!(
            deselects(&mut app),
            [CORPSE],
            "CloseLoot deselects the corpse"
        );

        // The last row taken.
        open_loot(
            &mut app,
            CORPSE,
            loot_type::CORPSE,
            0,
            vec![item(0, 117, 1)],
        );
        app.world_mut().resource_mut::<LootState>().remove_slot(0);
        app.update();
        assert_eq!(released(&rx), [CORPSE]);
        assert_eq!(
            deselects(&mut app),
            [CORPSE],
            "the last row's close deselects"
        );

        // The coin taken from a coin-only window.
        open_loot(&mut app, CORPSE, loot_type::CORPSE, 250, vec![]);
        app.world_mut().resource_mut::<LootState>().clear_money();
        app.update();
        assert_eq!(released(&rx), [CORPSE]);
        assert_eq!(deselects(&mut app), [CORPSE], "the coin's close deselects");

        // A live pickpocket target keeps the selection.
        open_loot(
            &mut app,
            LIVE,
            loot_type::PICKPOCKETING,
            0,
            vec![item(0, 117, 1)],
        );
        close_loot(&mut app);
        assert_eq!(released(&rx), [LIVE]);
        assert!(
            deselects(&mut app).is_empty(),
            "a live unit is not deselected"
        );

        // With nothing open, a second `CloseLoot` sends and asks nothing.
        close_loot(&mut app);
        assert!(released(&rx).is_empty());
        assert!(deselects(&mut app).is_empty());
    }

    /// `CloseInteraction` on a move start, with no server help and no VM.
    #[test]
    fn a_movement_start_closes_and_releases_the_open_loot() {
        const BOBBER: u64 = 0xF110_0000_0000_0011;
        const LOCKBOX: u64 = 0x4000_0000_0000_0007;

        // No UiScript mounted: the move-start leg must not depend on the VM.
        let (mut app, rx) = close_app();
        let open = |app: &mut App, guid: u64, kind: u8, rows: Vec<LootItem>| {
            open_loot(app, guid, kind, 0, rows);
        };
        let step = |app: &mut App| {
            app.world_mut().resource_mut::<LootMoveStart>().0 = true;
            app.update();
        };

        open(&mut app, CORPSE, loot_type::CORPSE, vec![item(0, 117, 1)]);
        app.update();
        assert_eq!(app.world().resource::<LootState>().source(), Some(CORPSE));
        assert!(
            rx.try_recv().is_err(),
            "nothing goes out without a move start"
        );

        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert_eq!(app.world().resource::<LootLatch>().0, None);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE),
            "the move-start close owes the wire the release"
        );
        assert!(rx.try_recv().is_err(), "and sends it once");
        assert_eq!(deselects(&mut app), vec![CORPSE]);

        // Skinning comes as wire type 2, as pick-pocketing does (`Player.cpp:8122`); the
        // deselect reads the health (`0x48f35d`), so the skinned corpse is deselected ...
        open(
            &mut app,
            CORPSE,
            loot_type::PICKPOCKETING,
            vec![item(0, 117, 1)],
        );
        step(&mut app);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE));
        assert_eq!(
            deselects(&mut app),
            vec![CORPSE],
            "a skinned corpse loses it"
        );
        // ... and a live pickpocket target is not.
        open(
            &mut app,
            LIVE,
            loot_type::PICKPOCKETING,
            vec![item(0, 117, 1)],
        );
        step(&mut app);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == LIVE));
        assert!(deselects(&mut app).is_empty(), "a live target keeps it");

        // A fishing bobber is a GameObject: the same path, minus the unit deselect.
        open(&mut app, BOBBER, loot_type::FISHING, vec![]);
        step(&mut app);
        assert_eq!(app.world().resource::<LootState>().source(), None);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == BOBBER));
        assert!(deselects(&mut app).is_empty());

        // A lockbox survives movement, unreleased.
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
