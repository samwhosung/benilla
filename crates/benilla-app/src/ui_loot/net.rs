//! The loot window's packet handlers: the [`LootState`] session and the [`LootLatch`] the loot lane
//! reads, the fishing verdicts, and the item push behind "You receive …".

use benilla_protocol::messages::ItemPushResult;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{LootCloseAtRest, LootLatch, LootMoveStart, LootState};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfGuid};
use crate::query_cache::Ask;
use crate::ui_action::{UiError, UiErrorKeys};

pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::LootResponse, on_response)
        .net_handler(K::LootError, on_error)
        .net_handler(K::LootRemoved, on_removed)
        .net_handler(K::LootMoneyNotify, on_money_notify)
        .net_handler(K::LootClearMoney, on_clear_money)
        .net_handler(K::LootReleaseResponse, on_release_response)
        .net_handler(K::LootMasterList, on_master_list)
        .net_handler(K::PlayerName, on_player_name)
        .net_handler(K::FishNotHooked, on_fish_verdict)
        .net_handler(K::FishEscaped, on_fish_verdict)
        .net_handler(K::ItemPushResult, on_item_push_result)
        .net_handler(K::Disconnected, on_session_end);
}

/// An admitted response for an object we hold selects a looted unit (`0x5ebc35` → `0x48f3a0`),
/// signalling at that place, before the window opens (`0x4c1cb0`), so the outgoing target's
/// teardown closes a window still open on it; with the targeting cursor up the select binds the
/// unit to the cast there instead (`0x4935d5` → `0x6e5b40`), whose send and signals go ahead of the
/// open. One for an object we lack shows
/// `ERR_LOOT_DIDNT_KILL` and returns (`0x5ebb33`, mask 1, `0x5ebb3f`) before the latch
/// (`0x5ebb60`) and the open. One that lands while our own
/// body moves takes the move-start close (`0x5ebc3a`-`0x5ebc51`), which the loot lane runs after
/// the copier's end, or the drain when the window waits on an item template. A close owed ahead of
/// it takes the window it found first ([`OwedAhead`]), on the lane ahead of this one's steps. The
/// copier asks each row's record here (`0x4c1e77`).
fn on_response(
    In(ev): In<SessionEvent>,
    mut set: crate::script_calls::disjoint::Pair<
        Response<'static, 'static>,
        // Absent in a harness without the cast ladder, which binds nothing.
        Option<crate::spell::PickBind<'static, 'static>>,
    >,
) {
    let SessionEvent::LootResponse {
        guid,
        loot_type,
        gold,
        items: rows,
    } = ev
    else {
        return;
    };
    let Some(latch_before) = set.p0().select(guid, loot_type) else {
        return;
    };
    if let Some(mut bind) = set.p1() {
        bind.bind();
    }
    set.p0().open(guid, loot_type, gold, rows, latch_before);
}

/// What a loot response reads and writes beside the cursor's bind.
#[derive(bevy::ecs::system::SystemParam)]
struct Response<'w, 's> {
    select: crate::target::SelectCommit<'w, 's>,
    player: Option<Res<'w, crate::player::Player>>,
    lock: Option<Res<'w, crate::player::MoverLock>>,
    owed: OwedAhead<'w, 's>,
    index: Res<'w, crate::net::GuidIndex>,
    errors: Res<'w, UiErrorKeys>,
    items: Res<'w, crate::items::Items>,
}

impl Response<'_, '_> {
    /// The response up to its select: the owed closes, the admission, the latch and the select
    /// (`0x5ebc35`); the latch it replaced, or `None` once it returned.
    fn select(&mut self, guid: u64, loot_type: u8) -> Option<Option<u64>> {
        let seam = &mut self.select.seam;
        self.owed
            .settle(&mut seam.loot, &mut seam.loot_latch, &seam.net);
        if !admit_response(
            guid,
            loot_type,
            &mut seam.loot_latch,
            &mut seam.loot,
            &seam.net,
        ) {
            return None;
        }
        if !self.index.0.contains_key(&guid) {
            debug!("net: loot response {guid:#x} for an object we lack — ERR_LOOT_DIDNT_KILL");
            self.errors.push(UiError::key("ERR_LOOT_DIDNT_KILL"));
            return None;
        }
        // A fishing bobber first arms here and still does not kneel: `LootKneel` decides the pose.
        let latch_before = seam.loot_latch.0;
        seam.loot_latch.0 = Some(guid);
        // Its signals wait for the packet's end, beside the deselects; with no targets, nowhere.
        match self.owed.dead.owed_selects() {
            Some(owed) => self.select.select_unit_in_packet(guid, owed),
            None => self.select.select_unit(guid),
        }
        Some(latch_before)
    }

    /// The rest, past the select and its bind: the window's open (`0x48f3cb` → `0x4c1cb0`), its
    /// rows' asks and the move-start close it owes.
    fn open(
        &mut self,
        guid: u64,
        loot_type: u8,
        gold: u32,
        rows: Vec<benilla_protocol::messages::LootItem>,
        latch_before: Option<u64>,
    ) {
        debug!(
            "net: loot response {guid:#x} type {loot_type} gold {gold} {} item(s)",
            rows.len()
        );
        let seam = &mut self.select.seam;
        seam.loot.open(guid, loot_type, gold, rows);
        seam.loot.note_replaced_latch(latch_before);
        seam.loot.ask_rows(&self.items, &seam.net);
        let steering_ourselves = self.lock.as_ref().is_none_or(|l| l.foreign_mover.is_none());
        if steering_ourselves
            && self
                .player
                .as_ref()
                .is_some_and(|p| p.move_flags() & crate::creature_anim::move_flags::STEERING != 0)
        {
            self.owed.move_start.0 = true;
        }
    }
}

/// The closes owed ahead of a response, which the reference runs where they are owed: our own
/// cast's GO ends its precast with the tail's close (`0x60d10b` → `0x48f200`), on the move the
/// move-start one ([`LootMoveStart`]), at rest the full one ([`LootCloseAtRest`]). They take the
/// window open then, or nothing (`0x48f23f`), never the one this response opens.
#[derive(bevy::ecs::system::SystemParam)]
struct OwedAhead<'w, 's> {
    move_start: ResMut<'w, LootMoveStart>,
    at_rest: ResMut<'w, LootCloseAtRest>,
    objects: crate::net::Objects<'w, 's>,
    dead: super::DeadUnitDeselect<'w, 's>,
}

impl OwedAhead<'_, '_> {
    /// Run the owed closes at this packet, the move-start one first, their steps queued ahead of
    /// the response's. A move-start close an earlier response in this drain owes stays for the
    /// loot lane, which runs it after that window's copier end (`0x5ebc51`). A disenchant hold's
    /// sweep (`0x48f264`) needs the VM and is owed to the lane, which runs it unless this
    /// response's copier replaces the session.
    fn settle(&mut self, loot: &mut LootState, latch: &mut LootLatch, net: &NetCommands) {
        let response_owes = loot
            .steps
            .iter()
            .any(|(_, s)| matches!(s, super::Step::Opened { .. }));
        if !response_owes && std::mem::take(&mut self.move_start.0) {
            let closed = super::close_on_move_start(loot, latch, net);
            self.dead.after_close(closed);
        }
        if std::mem::take(&mut self.at_rest.0) {
            if loot.disenchant_hold(&self.objects) {
                loot.owe_sweep();
            } else {
                let closed = super::close_interaction(loot, latch, Some(net));
                self.dead.after_close(closed);
            }
        }
    }
}

fn on_error(
    In(ev): In<SessionEvent>,
    errors: Res<UiErrorKeys>,
    mut latch: ResMut<LootLatch>,
    mut loot: ResMut<LootState>,
) {
    if let SessionEvent::LootError { guid, error } = ev {
        loot_error(guid, error, &errors, &mut latch, &mut loot);
    }
}

pub(super) fn on_removed(In(ev): In<SessionEvent>, mut window: WindowPacket) {
    if let SessionEvent::LootRemoved { slot } = ev {
        window.removed(slot);
    }
}

fn on_money_notify(
    In(ev): In<SessionEvent>,
    mut vm: crate::script_calls::HostedVm,
    chat: Res<crate::ui_chat::ChatLog>,
) {
    if let SessionEvent::LootMoneyNotify { amount } = ev {
        debug!("net: loot money {amount}");
        loot_money_notify(amount, &mut vm, &chat);
    }
}

pub(super) fn on_clear_money(In(ev): In<SessionEvent>, mut window: WindowPacket) {
    if let SessionEvent::LootClearMoney = ev {
        window.money_cleared();
    }
}

/// What the removal packets change: the window, and the close an emptied one takes.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct WindowPacket<'w, 's> {
    loot: ResMut<'w, LootState>,
    latch: ResMut<'w, LootLatch>,
    net: Res<'w, NetCommands>,
    objects: crate::net::Objects<'w, 's>,
    dead: super::DeadUnitDeselect<'w, 's>,
}

impl WindowPacket<'_, '_> {
    /// `SMSG_LOOT_REMOVED` (`0x5ebeb0`): with the player's loot target resolving (`0x5ebedf`, mask
    /// 1; else `0x5ebee7` leaves), `ClearLootRow` (`0x4c1f00`), the lane's `LOOT_SLOT_CLEARED`.
    fn removed(&mut self, slot: u8) {
        debug!("net: loot slot {slot} removed");
        if !self
            .latch
            .0
            .is_some_and(|g| self.objects.entity(g).is_some())
        {
            return;
        }
        if self.loot.remove_slot(slot) {
            self.close_if_empty();
        }
    }

    /// `SMSG_LOOT_CLEAR_MONEY` (`0x5ec030` → `0x4c2750`): the coin row goes, for every looter.
    fn money_cleared(&mut self) {
        debug!("net: loot coin line cleared");
        if self.loot.clear_money() {
            self.close_if_empty();
        }
    }

    /// A window with nothing left (`0x4c2a70`) closes with release inside the packet
    /// (`0x4c1f8d`, `0x4c2785` → `0x48f200(1, 0, 0)`); the lane signals its `LOOT_CLOSED` after the
    /// removal's `LOOT_SLOT_CLEARED` and before the pushes behind it. vmangos does not release it.
    fn close_if_empty(&mut self) {
        if self.loot.source().is_none() || self.loot.has_contents(&self.objects) {
            return;
        }
        let closed = super::close_interaction(&mut self.loot, &mut self.latch, Some(&self.net));
        if let Some(guid) = closed {
            debug!("net: loot emptied — release {guid:#x}");
        }
        self.dead.after_close(closed);
    }
}

fn on_release_response(
    In(ev): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    mut latch: ResMut<LootLatch>,
    mut dead: super::DeadUnitDeselect,
    objects: crate::net::Objects,
) {
    if let SessionEvent::LootReleaseResponse { guid, result } = ev {
        let closed = loot_release_response(guid, result, &mut loot, &mut latch, &objects);
        dead.after_close(closed);
    }
}

fn on_master_list(
    In(ev): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    group: Res<crate::ui_party::GroupState>,
) {
    if let SessionEvent::LootMasterList { candidates } = ev {
        loot_master_list(&candidates, &mut loot, &group);
    }
}

/// The name answer runs the candidate lookups' callbacks waiting on the guid, after the cache
/// filed it (`0x55f426`-`0x55f452`); an empty name is the answer for an unknown guid.
fn on_player_name(In(ev): In<SessionEvent>, mut loot: ResMut<LootState>) {
    if let SessionEvent::PlayerName { guid, name, .. } = ev {
        loot.name_answered(guid, !name.is_empty());
    }
}

fn on_fish_verdict(In(ev): In<SessionEvent>, errors: Res<UiErrorKeys>) {
    match ev {
        SessionEvent::FishNotHooked => fish_verdict(false, &errors),
        SessionEvent::FishEscaped => fish_verdict(true, &errors),
        _ => {}
    }
}

fn on_item_push_result(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    mut loot: ResMut<LootState>,
    items: Res<crate::items::Items>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::ItemPushResult(p) = ev {
        // The push's lookup with the callback `0x491ee0` (`0x491b05`).
        let held = |entry: u32| {
            let held = items.template(entry, 0, Ask::Event(&commands)).is_some();
            if !held {
                items.wait(entry, crate::items::RecordCallback::Push);
            }
            held
        };
        item_push_result(p, &self_guid, &mut loot, held);
    }
}

/// The open window, the latch and the auto-loot byte die with the session, unconditionally: the
/// next player object is built with both at 0 (`0x5dd403`, `0x5dd40f`).
fn on_session_end(
    In(_): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    mut latch: ResMut<LootLatch>,
    mut byte: ResMut<super::AutoLootByte>,
) {
    loot.clear_session();
    latch.0 = None;
    byte.0 = false;
}

/// The `loot_type`s a cold latch admits (`0x5eb94b`, `0x5eb953`, `0x5eb95b`): pickpocketing,
/// fishing and disenchanting, which the server starts. `CORPSE` (1) answers our own `CMSG_LOOT`
/// or `CMSG_OPEN_ITEM` (`SpellHandler.cpp:227`).
const SERVER_STARTED_LOOT: [u8; 3] = [2, 3, 4];

/// `SMSG_LOOT_RESPONSE`'s item shape, behind the admission gate `0x5eb924`: it opens when the
/// latch holds the packet's guid, or when the latch is cold and the type is server-started.
/// Anything else is refused (`0x5eb963`): no window, a `CMSG_LOOT_RELEASE` for the packet's guid,
/// the latch cleared whatever it held, and `UnlockItem` on the packet's guid (`0x5eb9de`). An
/// admitted one for an object we hold writes the latch (`0x5ebb60`) and plays no anim; a chest
/// already knelt at its `SMSG_SPELL_GO` (`0x6e831b`).
fn admit_response(
    guid: u64,
    loot_type: u8,
    latch: &mut LootLatch,
    loot: &mut LootState,
    net: &NetCommands,
) -> bool {
    let accept = match latch.0 {
        Some(latched) => latched == guid,
        None => SERVER_STARTED_LOOT.contains(&loot_type),
    };
    if !accept {
        // Inert against vmangos: a corpse always pre-arms, a chest or fishing answer is 2 or 3.
        debug!(
            "net: loot response {guid:#x} type {loot_type} REFUSED (latch {:?})",
            latch.0
        );
        if loot_type != 0 {
            let _ = net.0.send(ClientCommand::LootRelease { guid });
        }
        latch.0 = None; // not guid-matched: `0x5eb9d2` clears whatever was there
        loot.unlock_item_here(guid);
        return false;
    }
    true
}

/// `SMSG_FISH_ESCAPED` (the skill roll failed) or `SMSG_FISH_NOT_HOOKED` (clicked before the
/// splash, or expired): a yellow toast, as the handlers `0x5e3fc5`/`0x5e3fe2` push ids
/// `0x13e`/`0x13f`, type-1 entries that `DisplayError` fires as `UI_INFO_MESSAGE` (`0x49684b`).
fn fish_verdict(escaped: bool, errors: &UiErrorKeys) {
    let key = if escaped {
        "ERR_FISH_ESCAPED"
    } else {
        "ERR_FISH_NOT_HOOKED"
    };
    debug!("net: fishing verdict {key}");
    errors.push(UiError::key(key));
}

/// The default arm is code 7's arm (`0x5eba11 ja 0x5ebabd`), so it releases; the compare is
/// unsigned, so codes 0..3 and 15..255 all land there.
const DEFAULT_ARM_RELEASES: bool = true;

/// One arm of the loot-refusal jump table: the message it pushes, and whether it falls into the
/// release tail at `0x5ebac2`.
struct LootRefusal {
    /// The `GlobalStrings` key of the id the arm hands `CGGameUI::DisplayError` (`0x496720`).
    key: &'static str,
    /// The tail zeroes the loot guid (our [`LootLatch`]), runs `UnlockItem 0x495420` and
    /// `RecomputeBaseAnim 0x5fd9e0(-1)`: it ends the kneel.
    releases: bool,
}

/// The reference's refusal jump table for codes 4..=14 (`0x5eba0b`, entries at `0x5ebc64`). The
/// four that answer an already-open window (10, 12, 13, 14) skip the release tail. Every other
/// code takes the default, `ERR_LOOT_DIDNT_KILL`: 1.12 has no string for vmangos'
/// `ALREADY_PICKPOCKETED` (15) or `NOT_WHILE_SHAPESHIFTED` (16).
fn loot_refusal(reason: u8) -> LootRefusal {
    use benilla_protocol::messages::loot_error as e;
    // A gap in the vmangos enum, but a real arm of the client's table.
    const UNNAMED_DIDNT_KILL: u8 = 7;
    let (key, releases) = match reason {
        e::TOO_FAR => ("ERR_LOOT_TOO_FAR", true),
        e::BAD_FACING => ("ERR_LOOT_BAD_FACING", true),
        e::LOCKED => ("ERR_LOOT_LOCKED", true),
        UNNAMED_DIDNT_KILL => ("ERR_LOOT_DIDNT_KILL", true),
        e::NOTSTANDING => ("ERR_LOOT_NOTSTANDING", true),
        e::STUNNED => ("ERR_LOOT_STUNNED", true),
        e::PLAYER_NOT_FOUND => ("ERR_LOOT_PLAYER_NOT_FOUND", false),
        e::PLAY_TIME_EXCEEDED => ("ERR_PLAY_TIME_EXCEEDED", true),
        e::MASTER_INV_FULL => ("ERR_LOOT_MASTER_INV_FULL", false),
        e::MASTER_UNIQUE_ITEM => ("ERR_LOOT_MASTER_UNIQUE_ITEM", false),
        e::MASTER_OTHER => ("ERR_LOOT_MASTER_OTHER", false),
        _ => ("ERR_LOOT_DIDNT_KILL", DEFAULT_ARM_RELEASES),
    };
    LootRefusal { key, releases }
}

/// `SMSG_LOOT_RESPONSE`'s error shape. Only for the held loot target (`0x5eb936`-`0x5eb93e`): the
/// arm's message by key, as `DisplayError(stringId)` takes its surface and voice from the record,
/// and a releasing arm drops the target and runs `UnlockItem 0x495420` on the packet's guid (the
/// tail `0x5ebac2`, `0x5ebae0`). A refusal for any other guid, or with no target held, takes the
/// refusal leg (`0x5eb944` → `0x5eb963`): no line, the target cleared whatever it held
/// (`0x5eb9d2`) and the guid unlocked (`0x5eb9de`).
fn loot_error(
    guid: u64,
    error: u8,
    errors: &UiErrorKeys,
    latch: &mut LootLatch,
    loot: &mut LootState,
) {
    if latch.0 != Some(guid) {
        debug!("net: loot error {error} on {guid:#x}, not the loot target — dropped");
        latch.0 = None;
        loot.unlock_item_here(guid);
        return;
    }
    let LootRefusal { key, releases } = loot_refusal(error);
    debug!("net: loot error {error} on {guid:#x} → {key} (releases: {releases})");
    errors.push(UiError::key(key));
    if releases {
        latch.0 = None;
        loot.unlock_item_here(guid);
    }
}

/// `SMSG_LOOT_MONEY_NOTIFY` (`0x5ebf20`), our share of a group's coin: `LOOT_MONEY_SPLIT` as a
/// `CHAT_MSG_MONEY` line (`0x5ec011` → `AddChatLine 0x49a870`), queued behind the lines of the
/// packets before it; the coin row drops on `SMSG_LOOT_CLEAR_MONEY` and the purse rides the
/// coinage field. vmangos sends none for a solo loot (`LootHandler.cpp:331`).
fn loot_money_notify(
    amount: u32,
    vm: &mut crate::script_calls::HostedVm,
    chat: &crate::ui_chat::ChatLog,
) {
    let text = vm.run(|script| super::calls::share_line(script?, amount));
    if let Some(text) = text {
        chat.push_event(crate::ui_chat::ChatEvent::text_only(
            crate::ui_chat::ChatEventKind::Money,
            text,
        ));
    }
}

/// `SMSG_LOOT_RELEASE_RESPONSE` (`0x5ec090`): acted on only with a nonzero result (`0x5ec0b6`)
/// and the latched guid (`0x5ec0bd`-`0x5ec0ce`), so a release for an old window, which vmangos
/// sends when a new `CMSG_LOOT` replaces it (`Player.cpp:7694-7695`), closes nothing, and one
/// after our own close finds the latch already clear. An accepted one clears the latch
/// (`0x5ec0d4`) and takes the shared close without a send (`0x5ec0f3`), which unlocks an item
/// source (`0x48f200(cl=0, dl=0)`, [`super::close_interaction`]): a lockbox closed with loot left
/// keeps its slot, as vmangos destroys only a fully looted item (`LootHandler.cpp:558`). The
/// closed source is returned.
fn loot_release_response(
    guid: u64,
    result: u8,
    loot: &mut LootState,
    latch: &mut LootLatch,
    objects: &crate::net::Objects,
) -> Option<u64> {
    if result == 0 || latch.0 != Some(guid) {
        debug!(
            "net: loot release {guid:#x} (result {result}) ignored, latch {:?}",
            latch.0
        );
        return None;
    }
    debug!("net: loot released {guid:#x}");
    latch.0 = None;
    // The handler's close has `dl` clear (`0x5ec0ef`), so a disenchant window with contents is
    // swept instead, by its step on the loot lane.
    if loot.disenchant_hold(objects) {
        loot.owe_sweep();
        return None;
    }
    super::close_interaction(loot, latch, None)
}

/// `SMSG_LOOT_MASTER_LIST` (`0x61c550`): the candidate array, placed off the roster as this packet
/// finds it. vmangos sends it just ahead of the response it belongs to (`Player.cpp:8080`).
fn loot_master_list(candidates: &[u64], loot: &mut LootState, group: &crate::ui_party::GroupState) {
    debug!("net: master-loot candidates: {} eligible", candidates.len());
    loot.set_master_candidates(candidates, group);
}

/// `OnItemPush 0x491a60`'s arm (`0x491b56`/`0x491b61`): only our own push animates and its own
/// lines; another member's takes the `0x491d9f` arm. `showInChat` is a later gate
/// (`0x491bf3`/`0x491db1`), carried into [`LootState::push_receive`].
fn is_our_push(p: &ItemPushResult, self_guid: &SelfGuid) -> bool {
    self_guid.0 == Some(p.player_guid)
}

/// `SMSG_ITEM_PUSH_RESULT`: queues our own push for its bag-bar animation, its chat line and its
/// tutorials, and another member's for its line, at its packet when its record is `held`, else at
/// the record's answer.
fn item_push_result(
    p: ItemPushResult,
    self_guid: &SelfGuid,
    loot: &mut LootState,
    held: impl Fn(u32) -> bool,
) {
    debug!(
        "net: item push {} x{} → bag {} slot {:#x}",
        p.item_entry, p.count, p.bag_slot, p.item_slot
    );
    if !is_our_push(&p, self_guid) {
        let held = held(p.item_entry);
        // Another member's: no line for an item pushed but not created (`0x491d9f`-`0x491dab`),
        // or one not for the chat (`0x491db1`).
        if (p.from_npc && !p.created) || !p.show_in_chat {
            return;
        }
        loot.push_receive_other(&p, held);
        return;
    }
    loot.push_receive(&p, held(p.item_entry));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// A loot session with no window, whose owed unlocks a test may ignore.
    macro_rules! no_item {
        () => {
            &mut LootState::default()
        };
    }

    #[test]
    fn fish_verdict_keys_resolve_in_the_real_global_strings() {
        let errors = UiErrorKeys::default();
        fish_verdict(false, &errors);
        fish_verdict(true, &errors);
        assert_eq!(
            errors.snapshot(),
            vec![
                UiError::key("ERR_FISH_NOT_HOOKED"),
                UiError::key("ERR_FISH_ESCAPED"),
            ]
        );

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).expect(key);
        assert_eq!(g("ERR_FISH_NOT_HOOKED"), "No fish are hooked.");
        assert_eq!(g("ERR_FISH_ESCAPED"), "Your fish got away!");
    }

    /// The ids the reference's jump table (`0x5ebc64`) pushes: a plausible wrong key fails.
    #[test]
    fn every_refusal_names_the_message_id_the_reference_pushes() {
        const TABLE: &[(u8, u16)] = &[
            (4, 0x82),
            (5, 0x84),
            (6, 0x81),
            (7, 0x83),
            (8, 0x85),
            (9, 0x86),
            (10, 0x19b),
            (11, 0x1bf),
            (12, 0x1cd),
            (13, 0x1ce),
            (14, 0x1cf),
            // The default arm: 0 is `DIDNT_KILL`, 15 and 16 have no 1.12 string, 1 is an enum gap.
            (0, 0x83),
            (1, 0x83),
            (15, 0x83),
            (16, 0x83),
            (200, 0x83),
        ];
        for &(code, id) in TABLE {
            let key = loot_refusal(code).key;
            let record = benilla_ui::messages::by_key(key)
                .unwrap_or_else(|| panic!("code {code} named {key}, which is not a catalog row"));
            assert_eq!(
                record.id, id,
                "code {code} → {key} (id {}), but the reference pushes {id:#x}",
                record.id
            );
            // A red `UIErrorsFrame` line: the record's `+0x04`.
            assert_eq!(
                record.kind,
                benilla_ui::messages::MsgKind::Error,
                "{key} is not a red line"
            );
        }

        // Four refusals are voiced in the player's race and gender: their `type_tag` (`+0x0c`) is
        // a `VocalUIEnum` line id, not the `0x44` cue sentinel.
        let voiced = |key: &str| {
            usize::from(benilla_ui::messages::by_key(key).expect(key).type_tag)
                < benilla_formats::VOCAL_UI_LINES
        };
        for key in [
            "ERR_LOOT_DIDNT_KILL",
            "ERR_LOOT_BAD_FACING",
            "ERR_LOOT_LOCKED",
            "ERR_LOOT_TOO_FAR",
        ] {
            assert!(voiced(key), "{key} lost its voice line");
        }
        // The rest are cue-tagged.
        for key in ["ERR_LOOT_NOTSTANDING", "ERR_LOOT_MASTER_OTHER"] {
            assert!(!voiced(key), "{key} unexpectedly carries a voice line");
        }
    }

    #[test]
    fn refusal_keys_resolve_to_the_real_1_12_sentences() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).expect(key);

        for (code, want) in [
            (4u8, "You are too far away to loot that corpse."),
            (5, "You must be facing the corpse to loot it."),
            (6, "Someone is already looting that corpse."),
            (7, "You don't have permission to loot that corpse."),
            (8, "You need to be standing up to loot something!"),
            (9, "You can't loot anything while stunned!"),
            (10, "Player not found"),
            (11, "Maximum play time exceeded"),
            (12, "That player's inventory is full"),
            (13, "Player has too many of that item already"),
            (14, "Can't assign item to that player"),
            // The default arm, including the two vmangos codes 1.12 has no string for.
            (0, "You don't have permission to loot that corpse."),
            (15, "You don't have permission to loot that corpse."),
            (16, "You don't have permission to loot that corpse."),
        ] {
            assert_eq!(g(loot_refusal(code).key), want, "wire code {code}");
        }
    }

    /// Codes 10 and 12..=14 answer an open window and jump past the release tail `0x5ebac2`.
    #[test]
    fn only_the_window_less_refusals_drop_the_kneel() {
        // The releasing arms plus the default, which is code 7's arm (`ja 0x5ebabd`).
        for code in [4u8, 5, 6, 7, 8, 9, 11, 0, 1, 2, 3, 15, 16, 200, 255] {
            let errors = UiErrorKeys::default();
            let mut latch = LootLatch(Some(CORPSE));
            loot_error(CORPSE, code, &errors, &mut latch, no_item!());
            assert_eq!(latch.0, None, "code {code} should release");
            assert_eq!(errors.len(), 1, "code {code} shows exactly one line");
        }
        for code in [10u8, 12, 13, 14] {
            let errors = UiErrorKeys::default();
            let mut latch = LootLatch(Some(CORPSE));
            loot_error(CORPSE, code, &errors, &mut latch, no_item!());
            assert_eq!(
                latch.0,
                Some(CORPSE),
                "code {code} answers an OPEN window — the kneel stays"
            );
        }
    }

    /// The error leg is reached only through `0x5eb93e`, a matching target; a refusal for another
    /// guid takes the refusal leg (`0x5eb963`), which shows nothing and clears the target whatever
    /// it held (`0x5eb9d2`).
    #[test]
    fn a_refusal_for_another_guid_clears_the_target_and_shows_nothing() {
        let errors = UiErrorKeys::default();
        let mut latch = LootLatch(Some(CHEST));
        loot_error(CORPSE, 8, &errors, &mut latch, no_item!());
        assert_eq!(latch.0, None);
        assert!(errors.snapshot().is_empty());
    }

    /// A GameObject guid: a chest.
    const CHEST: u64 = 0xF110_0000_0000_1234;
    /// A creature guid: a corpse.
    const CORPSE: u64 = 0xF130_0000_0000_00AB;

    /// A `NetCommands` and its receiver, to read what a handler sent.
    fn net() -> (
        crate::net::NetCommands,
        crossbeam_channel::Receiver<ClientCommand>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (crate::net::NetCommands(tx), rx)
    }

    /// No `CMSG_LOOT` armed the latch; a chest's answer is wire type 2 (`0x5eb94b`-`0x5eb95b`).
    #[test]
    fn a_cold_latch_admits_a_server_started_loot_and_arms_on_it() {
        let (mut world, rx) = tabbed_world();
        assert_eq!(
            world.resource::<LootLatch>().0,
            None,
            "no CMSG_LOOT was sent, so nothing armed it"
        );

        respond(&mut world, CHEST, 2);
        assert_eq!(
            world.resource::<LootLatch>().0,
            Some(CHEST),
            "the open window is the loot session"
        );
        assert_eq!(
            world.resource::<LootState>().source(),
            Some(CHEST),
            "…and the window opened"
        );
        assert!(
            rx.try_recv().is_err(),
            "an accepted response bounces nothing"
        );

        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse {
                    guid: CHEST,
                    result: 1,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert_eq!(
            world.resource::<LootLatch>().0,
            None,
            "the release ends the session"
        );
    }

    /// The `CMSG_LOOT` send armed the same guid: the match branch (`0x5eb93e`).
    #[test]
    fn a_matching_latch_admits_any_loot_type() {
        let (mut world, _rx) = tabbed_world();
        world.resource_mut::<LootLatch>().0 = Some(CORPSE); // the `CMSG_LOOT` send
        respond(&mut world, CORPSE, 1);
        assert_eq!(world.resource::<LootLatch>().0, Some(CORPSE));
        assert_eq!(world.resource::<LootState>().source(), Some(CORPSE));
    }

    /// The refusal arm (`0x5eb963`): a type-1 answer on a cold latch answers nothing we asked.
    #[test]
    fn a_cold_latch_refuses_a_corpse_typed_response_and_bounces_it() {
        let (mut world, rx) = tabbed_world();

        respond(&mut world, CORPSE, 1);
        assert_eq!(
            world.resource::<LootState>().source(),
            None,
            "no window for an unasked-for corpse answer"
        );
        assert_eq!(
            world.resource::<LootLatch>().0,
            None,
            "…and nothing latched"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE),
            "the refusal releases the object the PACKET named"
        );
    }

    /// The refusal's clear is not guid-matched (`0x5eb9d2`): a response for B drops a latch on A.
    #[test]
    fn a_refusal_drops_whatever_latch_was_live_not_just_a_matching_one() {
        let (mut world, rx) = tabbed_world();
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);

        respond(&mut world, CHEST, 1);
        assert_eq!(
            world.resource::<LootLatch>().0,
            None,
            "A's latch is dropped by B's refusal"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CHEST),
            "…and it is B that gets released"
        );
        assert_eq!(
            world.resource::<LootState>().queued_unlocks(),
            [CHEST],
            "and `UnlockItem` runs on B's guid, an item or not (`0x5eb9de`), at the packet"
        );
    }

    /// An item guid (`HIGHGUID_ITEM` 0x4000 in the high word): a lockbox.
    const LOCKBOX: u64 = 0x4000_0000_0000_0007;

    /// A world with a loot window open on the lockbox, latched, as its `CMSG_OPEN_ITEM` and the
    /// answer leave it.
    fn opened_lockbox_world() -> World {
        let mut world = World::new();
        let mut loot = LootState::default();
        loot.open(LOCKBOX, 1, 0, Vec::new());
        world.insert_resource(loot);
        world.insert_resource(LootLatch(Some(LOCKBOX)));
        world.init_resource::<UiErrorKeys>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::target::OwedSelects>();
        world
    }

    /// With loot left, no field update releases the lockbox: the close does, twice, the close's
    /// own `UnlockItem` (`0x48f2bd`) and the frame teardown's (`0x4c1d36`).
    #[test]
    fn a_release_closing_the_opened_item_unlocks_it_twice() {
        let mut world = opened_lockbox_world();
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse {
                    guid: LOCKBOX,
                    result: 1,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert_eq!(
            world
                .resource_mut::<LootState>()
                .take_queued_unlocks()
                .into_iter()
                .map(|(g, _)| g)
                .collect::<Vec<_>>(),
            [LOCKBOX, LOCKBOX]
        );
        // With no window left, a second release unlocks nothing (`0x48f23f`).
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse {
                    guid: LOCKBOX,
                    result: 1,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert!(world
            .resource_mut::<LootState>()
            .take_queued_unlocks()
            .into_iter()
            .map(|(g, _)| g)
            .collect::<Vec<_>>()
            .is_empty());
    }

    /// A window on a corpse closes with no unlock: only an item source takes the two
    /// (`0x48f2ab`, `0x4c1d27`).
    #[test]
    fn closing_a_window_on_anything_but_an_item_unlocks_nothing() {
        let mut loot = LootState::default();
        let mut latch = LootLatch(Some(CORPSE));
        loot.open(CORPSE, 1, 0, Vec::new());
        assert_eq!(
            super::super::close_interaction(&mut loot, &mut latch, None),
            Some(CORPSE)
        );
        assert!(loot
            .take_queued_unlocks()
            .into_iter()
            .map(|(g, _)| g)
            .collect::<Vec<_>>()
            .is_empty());
    }

    /// A window replacing one open on an item unlocks the old source once, the teardown's
    /// (`0x5ebc35` → `0x4c1d36`).
    #[test]
    fn a_window_replacing_an_items_unlocks_the_old_source() {
        let mut loot = LootState::default();
        loot.open(LOCKBOX, 1, 0, Vec::new());
        assert!(
            loot.take_queued_unlocks()
                .into_iter()
                .map(|(g, _)| g)
                .collect::<Vec<_>>()
                .is_empty(),
            "the first window replaces nothing"
        );
        loot.open(CORPSE, 1, 0, Vec::new());
        assert_eq!(
            loot.take_queued_unlocks()
                .into_iter()
                .map(|(g, _)| g)
                .collect::<Vec<_>>(),
            [LOCKBOX]
        );
        loot.open(CHEST, 2, 0, Vec::new());
        assert!(
            loot.take_queued_unlocks()
                .into_iter()
                .map(|(g, _)| g)
                .collect::<Vec<_>>()
                .is_empty(),
            "a corpse source owes nothing"
        );
    }

    /// The server's release (`0x5ec090` → `0x5ec0f3`) closes the window through the shared close,
    /// sending nothing back, and its last step deselects the dead unit it closed on.
    #[test]
    fn a_release_response_closes_the_window_and_deselects_a_dead_unit() {
        let (net, rx) = net();
        let mut world = opened_lockbox_world();
        world.insert_resource(net);
        let corpse = world
            .spawn((
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                // `UNIT_FIELD_HEALTH` (22) at 0.
                crate::net::ObjectStore(benilla_protocol::messages::ObjectFields::from_pairs(&[(
                    22, 0,
                )])),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(CORPSE, corpse);
        world
            .resource_mut::<LootState>()
            .open(CORPSE, 1, 0, Vec::new());
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);

        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse {
                    guid: CORPSE,
                    result: 1,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert_eq!(world.resource::<LootState>().source(), None);
        assert_eq!(world.resource::<LootLatch>().0, None);
        assert!(rx.try_recv().is_err(), "the server's release is not echoed");
        assert_eq!(crate::target::take_owed_deselects(&mut world), [CORPSE]);

        // A second release finds nothing open and asks nothing.
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse {
                    guid: CORPSE,
                    result: 1,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert!(crate::target::take_owed_deselects(&mut world).is_empty());
    }

    /// Chained loot: A's window is open and `CMSG_LOOT` for B latched B, so the release vmangos
    /// sends for A on the way (`Player.cpp:7694-7695`) is not the latch and closes nothing
    /// (`0x5ec0bd`-`0x5ec0ce`); B's response then replaces the window.
    #[test]
    fn a_release_for_the_window_a_new_loot_request_replaces_closes_nothing() {
        let mut world = opened_lockbox_world();
        world
            .resource_mut::<LootState>()
            .open(CORPSE, 1, 0, Vec::new());
        world.resource_mut::<LootState>().take_queued_unlocks();
        world.resource_mut::<LootLatch>().0 = Some(CHEST); // the `CMSG_LOOT` for B
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse {
                    guid: CORPSE,
                    result: 1,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert_eq!(
            world.resource::<LootState>().source(),
            Some(CORPSE),
            "A's window stays up for B's response to replace"
        );
        assert_eq!(
            world.resource::<LootLatch>().0,
            Some(CHEST),
            "B stays latched"
        );
    }

    /// The release's close has `dl` clear (`0x5ec0ef`): a disenchant window with rows left owes
    /// its sweep instead of closing; the handler's own latch clear (`0x5ec0d4`) still runs.
    #[test]
    fn a_release_on_a_disenchant_window_with_rows_left_owes_its_sweep() {
        let mut world = opened_lockbox_world();
        // The disenchanted item, which `LootHasContents` resolves (`0x4c2a8c`).
        let item = world
            .spawn(crate::net::ObjectStore(
                benilla_protocol::messages::ObjectFields::from_pairs(&[(2, 0x3)]),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(LOCKBOX, item);
        world.resource_mut::<LootState>().open(
            LOCKBOX,
            benilla_protocol::messages::loot_type::DISENCHANTING,
            0,
            vec![benilla_protocol::messages::LootItem {
                slot: 0,
                item_id: 117,
                count: 1,
                display_info_id: 1117,
                random_property_id: 0,
                slot_type: 0,
            }],
        );
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse {
                    guid: LOCKBOX,
                    result: 1,
                },
            )
            .expect("the handler runs as a one-shot system");
        let loot = world.resource::<LootState>();
        assert_eq!(loot.source(), Some(LOCKBOX));
        assert!(loot.sweep_owed);
        assert_eq!(world.resource::<LootLatch>().0, None);
    }

    /// A zero result is ignored even for the latched guid (`0x5ec0b6`).
    #[test]
    fn a_release_with_result_zero_closes_nothing() {
        let mut world = opened_lockbox_world();
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse {
                    guid: LOCKBOX,
                    result: 0,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert_eq!(world.resource::<LootState>().source(), Some(LOCKBOX));
        assert_eq!(world.resource::<LootLatch>().0, Some(LOCKBOX));
    }

    /// A live creature: `TYPEMASK_OBJECT | TYPEMASK_UNIT`, so it is the Tab target.
    const KOBOLD: u64 = 0xF130_0000_0600_00CD;
    /// A creature flagged `UNIT_FLAG_NOT_SELECTABLE`.
    const UNSELECTABLE: u64 = 0xF130_0000_0700_00EF;

    const OBJECT_FIELD_TYPE: u16 = 2;
    const OBJECT_FIELD_ENTRY: u16 = 3;
    /// `OBJECT_FIELD_TYPE` masks: `TYPEMASK_OBJECT` with `UNIT`, `PLAYER` or `GAMEOBJECT`.
    const TYPE_UNIT: u32 = 0x09;
    const TYPE_PLAYER: u32 = 0x19;
    const TYPE_GAMEOBJECT: u32 = 0x21;
    /// `UNIT_FLAG_NOT_SELECTABLE`.
    const NOT_SELECTABLE: u32 = 1 << 25;
    const KOBOLD_ENTRY: u32 = 6;
    const CORPSE_ENTRY: u32 = 257;

    /// Us, the Tab target selected, and a corpse, a chest and an unselectable unit beside it, all
    /// streamed: the issue's spot, with what [`on_response`] reads and the selection commit writes.
    fn tabbed_world() -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let mut world = World::new();
        let rx = seat_tabbed(&mut world);
        (world, rx)
    }

    /// [`tabbed_world`]'s seating, into any world.
    fn seat_tabbed(world: &mut World) -> crossbeam_channel::Receiver<ClientCommand> {
        use crate::net::{Guid, GuidIndex, NetEntity, ObjectStore, SelfPlayer};
        use benilla_protocol::field::{FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH};
        use benilla_protocol::messages::{ObjectFields, ObjectType};
        use benilla_protocol::EntityKind::{GameObject, Unit};
        world.init_resource::<crate::items::Items>();
        let unit = |kind, type_mask, pairs: &[(u16, u32)]| {
            let mut fields = vec![(OBJECT_FIELD_TYPE, type_mask)];
            fields.extend_from_slice(pairs);
            (
                NetEntity {
                    kind,
                    display_id: None,
                    scale: 1.0,
                },
                ObjectStore(ObjectFields::from_pairs(&fields).into_created(match kind {
                    GameObject => ObjectType::GameObject,
                    _ => ObjectType::Unit,
                })),
            )
        };
        let (net, rx) = net();
        world.insert_resource(net);
        world.init_resource::<LootState>();
        world.init_resource::<LootLatch>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::PendingCast>();
        world.init_resource::<crate::ui_cast::CastReports>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<crate::creature_anim::AttackStopInFlight>();
        world.init_resource::<crate::creature_anim::UnitEdits>();
        world.init_resource::<bevy::ecs::message::Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<GuidIndex>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<crate::target::AssistAttack>();
        world.init_resource::<crate::target::Selection>();
        world.init_resource::<crate::spell::SpellTargeting>();
        world.init_resource::<crate::spell::CursorPicks>();
        world.init_resource::<crate::spell::targeting::EnchantConfirmItem>();
        world.init_resource::<crate::target::OwedSelects>();
        world.init_resource::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>();
        world.init_resource::<crate::player::Player>();
        world.init_resource::<super::super::LootMoveStart>();
        world.init_resource::<super::super::LootCloseAtRest>();
        world.init_resource::<super::super::AutoLootByte>();
        world.init_resource::<UiErrorKeys>();
        world.spawn((
            SelfPlayer,
            Guid(1),
            unit(Unit, TYPE_PLAYER, &[(FIELD_UNIT_HEALTH, 100)]),
        ));
        for (guid, bundle) in [
            (
                KOBOLD,
                unit(
                    Unit,
                    TYPE_UNIT,
                    &[(OBJECT_FIELD_ENTRY, KOBOLD_ENTRY), (FIELD_UNIT_HEALTH, 100)],
                ),
            ),
            (
                CORPSE,
                unit(
                    Unit,
                    TYPE_UNIT,
                    &[(OBJECT_FIELD_ENTRY, CORPSE_ENTRY), (FIELD_UNIT_HEALTH, 0)],
                ),
            ),
            (CHEST, unit(GameObject, TYPE_GAMEOBJECT, &[])),
            (
                UNSELECTABLE,
                unit(
                    Unit,
                    TYPE_UNIT,
                    &[(FIELD_UNIT_HEALTH, 0), (FIELD_UNIT_FLAGS, NOT_SELECTABLE)],
                ),
            ),
        ] {
            let e = world.spawn((Guid(guid), bundle)).id();
            world.resource_mut::<GuidIndex>().0.insert(guid, e);
        }
        let kobold = world.resource::<GuidIndex>().0[&KOBOLD];
        let mut selection = world.resource_mut::<crate::target::Selection>();
        selection.target = Some(kobold);
        selection.guid = Some(KOBOLD);
        rx
    }

    /// A response for `guid` of `loot_type` with no rows, through the registered handler.
    fn respond(world: &mut World, guid: u64, loot_type: u8) {
        world
            .run_system_once_with(
                on_response,
                SessionEvent::LootResponse {
                    guid,
                    loot_type,
                    gold: 0,
                    items: Vec::new(),
                },
            )
            .expect("the handler runs as a one-shot system");
    }

    /// `0x5ebc35` → `0x48f3a0` → `SetSelection 0x493540` on the looted unit's guid.
    #[test]
    fn a_loot_window_opening_on_a_unit_selects_it() {
        let (mut world, rx) = tabbed_world();
        world.resource_mut::<LootLatch>().0 = Some(CORPSE); // the `CMSG_LOOT` send
        respond(&mut world, CORPSE, 1);
        assert_eq!(world.resource::<LootState>().source(), Some(CORPSE));
        let selection = world.resource::<crate::target::Selection>();
        assert_eq!(
            selection.guid,
            Some(CORPSE),
            "the looted body is the target"
        );
        assert_eq!(
            selection.last,
            Some(KOBOLD),
            "…through SetSelection's stamp"
        );
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(sent[..], [ClientCommand::SetSelection { guid }] if guid == CORPSE),
            "{sent:?}"
        );

        // The setter's dedup: a second window on the selected body sends nothing.
        respond(&mut world, CORPSE, 1);
        assert!(rx.try_recv().is_err());
    }

    /// `0x48f3a0` tests `TYPEMASK_UNIT`; `SetSelection`'s `IsSelectable` refuses the flagged unit;
    /// a refused response never reaches `0x5ebc35`.
    #[test]
    fn a_chest_an_unselectable_unit_and_a_refusal_leave_the_target() {
        for (guid, latched, loot_type) in [
            (CHEST, true, 1),
            (UNSELECTABLE, true, 1),
            (CORPSE, false, 1),
        ] {
            let (mut world, rx) = tabbed_world();
            world.resource_mut::<LootLatch>().0 = latched.then_some(guid);
            respond(&mut world, guid, loot_type);
            assert_eq!(
                world.resource::<crate::target::Selection>().guid,
                Some(KOBOLD),
                "{guid:#x}"
            );
            assert!(
                !rx.try_iter()
                    .any(|c| matches!(c, ClientCommand::SetSelection { .. })),
                "{guid:#x}"
            );
        }
    }

    /// The selection comes before the window (`0x48f3be`, then `0x4c1cb0`): the outgoing target's
    /// teardown closes the window still open on it, its release ahead of the new selection.
    #[test]
    fn the_selection_tears_down_the_old_targets_window_before_the_new_one_opens() {
        let (mut world, rx) = tabbed_world();
        world
            .resource_mut::<LootState>()
            .open(KOBOLD, 2, 0, Vec::new());
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(&mut world, CORPSE, 1);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::LootRelease { guid: a }, ClientCommand::SetSelection { guid: b }]
                    if a == KOBOLD && b == CORPSE
            ),
            "{sent:?}"
        );
        assert_eq!(world.resource::<LootState>().source(), Some(CORPSE));
        // [`LootLatch`]'s deviation: the reference's teardown zeroes it here (`0x48f2c9`).
        assert_eq!(world.resource::<LootLatch>().0, Some(CORPSE));
    }

    /// `SetSelection`'s switch while swinging (`0x493a08`, `0x4938a1`): the swing at the Tab target
    /// stops, and none opens on the body.
    #[test]
    fn looting_while_swinging_at_the_tab_target_stops_the_swing() {
        use crate::net::SelfPlayer;
        let (mut world, rx) = tabbed_world();
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(&world)
            .expect("us");
        world
            .entity_mut(me)
            .insert(crate::creature_anim::Engaged(KOBOLD));
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(&mut world, CORPSE, 1);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AttackStop, ClientCommand::SetSelection { guid }] if guid == CORPSE
            ),
            "{sent:?}"
        );
    }

    /// `StartAttack`'s target gate (`0x5ecc16`-`0x5ecc29`): a body is no swing target even when
    /// hostile, so looting one mid-swing stops and selects, and swings at nothing.
    #[test]
    fn looting_a_hostile_body_while_swinging_never_swings_at_it() {
        use crate::net::{ObjectStore, SelfPlayer};
        use benilla_protocol::field::{FIELD_UNIT_FACTIONTEMPLATE, FIELD_UNIT_HEALTH};
        use benilla_protocol::messages::{ObjectFields, ObjectType};
        const UNIT_FIELD_BYTES_0: u16 = 36;
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let (factions, stormwind, reps) = crate::target::stormwind_fixture(&mut chain, 0);
        let me_store = ObjectStore(
            ObjectFields::from_pairs(&[
                (OBJECT_FIELD_TYPE, TYPE_PLAYER),
                (FIELD_UNIT_HEALTH, 100),
                (FIELD_UNIT_FACTIONTEMPLATE, stormwind),
                (UNIT_FIELD_BYTES_0, crate::target::HUMAN_WARRIOR),
            ])
            .into_created(ObjectType::Player),
        );
        let creature = |template, health| {
            ObjectStore(
                ObjectFields::from_pairs(&[
                    (OBJECT_FIELD_TYPE, TYPE_UNIT),
                    (FIELD_UNIT_HEALTH, health),
                    (FIELD_UNIT_FACTIONTEMPLATE, template),
                ])
                .into_created(ObjectType::Unit),
            )
        };
        let rel = crate::target::Relations {
            factions: Some(&factions),
            reputations: &reps,
            me: None,
            roster: None,
            units: &crate::target::relations::NoUnits,
        };
        let me = crate::target::RelUnit::of(1, &me_store);
        let hostile = (1u32..4096)
            .find(|&t| rel.can_attack(&me, &crate::target::RelUnit::of(2, &creature(t, 100))))
            .expect("a template a Stormwind human can attack");

        let (mut world, rx) = tabbed_world();
        world.insert_resource(factions);
        world.insert_resource(reps);
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(&world)
            .expect("us");
        world
            .entity_mut(me)
            .insert((me_store, crate::creature_anim::Engaged(KOBOLD)));
        let corpse = world.resource::<crate::net::GuidIndex>().0[&CORPSE];
        world.entity_mut(corpse).insert(creature(hostile, 0));
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(&mut world, CORPSE, 1);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AttackStop, ClientCommand::SetSelection { guid }] if guid == CORPSE
            ),
            "{sent:?}"
        );
    }

    /// The stock unit frames over `TargetFrame`, for [`stock_app`].
    const TARGET_UI: &[&str] = &[
        "Interface\\FrameXML\\Fonts.xml",
        "Interface\\FrameXML\\GlobalStrings.lua",
        "Interface\\FrameXML\\UIParent.xml",
        "Interface\\FrameXML\\BasicControls.xml",
        "Interface\\FrameXML\\MoneyFrame.lua",
        "Interface\\FrameXML\\MoneyFrame.xml",
        "Interface\\FrameXML\\GameTooltip.xml",
        "Interface\\FrameXML\\UIDropDownMenu.xml",
        "Interface\\FrameXML\\TextStatusBar.lua",
        "Interface\\FrameXML\\TextStatusBar.xml",
        "Interface\\FrameXML\\BuffFrame.xml",
        "Interface\\FrameXML\\CombatFeedback.xml",
        "Interface\\FrameXML\\UnitPopup.xml",
        "Interface\\FrameXML\\UnitFrame.xml",
        "Interface\\FrameXML\\PlayerFrame.xml",
        "Interface\\FrameXML\\PartyFrame.xml",
        "Interface\\FrameXML\\TargetFrame.xml",
        // `StaticPopup_Hide`, which UIParent's `PLAYER_UNGHOST` arm calls (`UIParent.lua:251-257`)
        // at a live arrival.
        "Interface\\FrameXML\\StaticPopup.xml",
    ];

    /// [`tabbed_world`] under the stock `ui` files, with the token seats and both creatures named.
    fn stock_app(ui: &[&[&str]]) -> App {
        use benilla_ui::script::UiScript;
        let mut s = UiScript::new().expect("VM");
        s.set_screen_size(1024.0, 768.0);
        for f in crate::ui_script::test_ui::production_order(ui) {
            crate::ui_script::test_ui::load_ui(&s, f);
        }

        let mut app = App::new();
        seat_tabbed(app.world_mut());
        let mut names = crate::names::NameCache::default();
        for (entry, name) in [
            (KOBOLD_ENTRY, "Kobold Vermin"),
            (CORPSE_ENTRY, "Kobold Worker"),
        ] {
            names.insert_creature(
                entry,
                Some(crate::names::CreatureRecord {
                    name: name.into(),
                    subname: None,
                    creature_type: 7,
                    pet_family: 0,
                    rank: 0,
                    type_flags: 0,
                    pet_spell_list: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
        }
        app.insert_resource(names)
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::ui_guild::GuildState>()
            .insert_non_send_resource(s);
        crate::ui_unit::add_token_seats(&mut app);
        app
    }

    /// The name on the stock `TargetFrame`, empty while it is hidden.
    fn target_frame_name(app: &App) -> String {
        let s = app
            .world()
            .non_send_resource::<benilla_ui::script::UiScript>();
        assert!(s.errors().is_empty(), "script errors: {:?}", s.errors());
        s.eval("return TargetFrame:IsShown() and TargetFrame.name:GetText() or ''")
            .expect("the target frame's name")
    }

    /// The issue's symptom on the stock frames: the Tab target is on `TargetFrame` when the body's
    /// window opens, and the target's name reads the body there (`TargetFrame.lua:63`,
    /// `UnitFrame.lua:25`).
    #[test]
    fn the_stock_target_frame_names_the_looted_body() {
        benilla_formats::wow_data_or_skip!();
        let mut app = stock_app(&[TARGET_UI]);

        app.update();
        target_changed(&mut app);
        assert_eq!(target_frame_name(&app), "Kobold Vermin", "the Tab target");

        app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(app.world_mut(), CORPSE, 1);
        app.update();
        target_changed(&mut app);
        assert_eq!(target_frame_name(&app), "Kobold Worker", "the looted body");
    }

    /// The select's own `PLAYER_TARGET_CHANGED` (`0x49389c`), which no host carries in this
    /// harness, once the frame's pushes seat the target.
    fn target_changed(app: &mut App) {
        app.world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .fire_event("PLAYER_TARGET_CHANGED", vec![]);
    }

    /// What [`super::super::drain_loot`] reads beside [`seat_tabbed`]'s, with our body's movement
    /// word at `player`'s.
    fn seat_drain(world: &mut World, player: crate::player::Player) {
        world.insert_resource(player);
        super::super::seat_loot_steps(world);
    }

    /// The deselects asked for since the last call.
    fn deselects(world: &mut World) -> Vec<u64> {
        crate::target::take_owed_deselects(world)
    }

    /// A response on the corpse with `player` seated, then one drain.
    fn open_while(
        player: crate::player::Player,
        lock: crate::player::MoverLock,
    ) -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut world, rx) = tabbed_world();
        seat_drain(&mut world, player);
        world.insert_resource(lock);
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(&mut world, CORPSE, 1);
        world
            .run_system_once(super::super::drain_loot)
            .expect("the drain runs as a one-shot system");
        (world, rx)
    }

    /// The response's tail (`0x5ebc3a`-`0x5ebc51`): a bit of our movement word's low byte, forward,
    /// back, strafe, turn or pitch, takes the move-start close on the window it just opened, the
    /// release after the selection and the dead body's deselect last (`0x48f369`).
    #[test]
    fn a_window_that_opens_while_moving_closes_as_a_movement_start_does() {
        for bit in (0..8).map(|b| 1u32 << b) {
            let (mut world, rx) = open_while(
                crate::player::Player::with_move_flags(bit),
                Default::default(),
            );
            assert_eq!(world.resource::<LootState>().source(), None, "{bit:#x}");
            assert_eq!(world.resource::<LootLatch>().0, None, "{bit:#x}");
            let sent: Vec<_> = rx.try_iter().collect();
            assert!(
                matches!(
                    sent[..],
                    [ClientCommand::SetSelection { guid: a }, ClientCommand::LootRelease { guid: b }]
                        if a == CORPSE && b == CORPSE
                ),
                "{bit:#x}: {sent:?}"
            );
            assert_eq!(deselects(&mut world), [CORPSE], "{bit:#x}");
        }
    }

    /// Only that byte, and only our body's: walking, falling or swimming alone leaves the window
    /// open, as does a step of the creature we possess.
    #[test]
    fn a_window_opened_falling_swimming_or_possessing_stays_open() {
        use crate::creature_anim::move_flags as f;
        use crate::player::{MoverLock, Player};
        let possessing = MoverLock {
            foreign_mover: Some(KOBOLD),
            ..MoverLock::default()
        };
        for (what, player, lock) in [
            ("still", Player::with_move_flags(0), MoverLock::default()),
            (
                "walk mode",
                Player::with_move_flags(f::WALK_MODE),
                MoverLock::default(),
            ),
            (
                "falling",
                Player::with_move_flags(f::FALLING),
                MoverLock::default(),
            ),
            (
                "swimming",
                Player::with_move_flags(f::SWIMMING),
                MoverLock::default(),
            ),
            (
                "possessing",
                Player::with_move_flags(f::FORWARD),
                possessing,
            ),
        ] {
            let (mut world, rx) = open_while(player, lock);
            assert_eq!(
                world.resource::<LootState>().source(),
                Some(CORPSE),
                "{what}"
            );
            let sent: Vec<_> = rx.try_iter().collect();
            assert!(
                matches!(sent[..], [ClientCommand::SetSelection { guid }] if guid == CORPSE),
                "{what}: {sent:?}"
            );
            assert!(deselects(&mut world).is_empty(), "{what}");
        }
    }

    /// The close our own cast's GO owes (`0x60d040` at `0x6e7fc5`, its tail `0x60d10b`): at rest
    /// the full one, on the move (`& 0x200f`, falling included) the move-start one, which a
    /// response's own tail (`0x5ebc40`, the low byte alone) does not owe while falling.
    #[derive(Clone, Copy, Debug)]
    enum Owed {
        AtRest,
        MoveStart,
    }

    impl Owed {
        const BOTH: [(Owed, u32); 2] = [
            (Owed::AtRest, 0),
            (Owed::MoveStart, crate::creature_anim::move_flags::FALLING),
        ];

        fn owe(self, world: &mut World) {
            match self {
                Owed::AtRest => world.resource_mut::<super::super::LootCloseAtRest>().0 = true,
                Owed::MoveStart => world.resource_mut::<super::super::LootMoveStart>().0 = true,
            }
        }
    }

    fn drain(world: &mut World) {
        world
            .run_system_once(super::super::drain_loot)
            .expect("the drain runs as a one-shot system");
    }

    fn releases(rx: &crossbeam_channel::Receiver<ClientCommand>) -> Vec<u64> {
        rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::LootRelease { guid } => Some(guid),
                _ => None,
            })
            .collect()
    }

    /// A gather's or an opening's GO runs its close inside its own packet (`0x60d10b` →
    /// `0x48f200`), where no window is open (`0x48f23f`), so it closes nothing. The chest's
    /// response behind it opens a window the frame's drain keeps, unreleased.
    #[test]
    fn a_close_the_go_ahead_owed_never_takes_the_window_its_response_opens() {
        for (owed, flags) in Owed::BOTH {
            let (mut world, rx) = tabbed_world();
            seat_drain(&mut world, crate::player::Player::with_move_flags(flags));
            world.resource_mut::<LootLatch>().0 = Some(CHEST);
            owed.owe(&mut world);
            respond(&mut world, CHEST, 1);
            drain(&mut world);
            assert_eq!(
                world.resource::<LootState>().source(),
                Some(CHEST),
                "{owed:?}"
            );
            assert_eq!(world.resource::<LootLatch>().0, Some(CHEST), "{owed:?}");
            assert!(releases(&rx).is_empty(), "{owed:?}");
        }
    }

    /// The same close with a window open at the GO closes that one, released and its dead body
    /// deselected (`0x48f369`), and the next response opens on a clean slate.
    #[test]
    fn a_close_the_go_ahead_owed_takes_the_window_open_then() {
        for (owed, flags) in Owed::BOTH {
            let (mut world, rx) = tabbed_world();
            seat_drain(&mut world, crate::player::Player::with_move_flags(flags));
            world.resource_mut::<LootLatch>().0 = Some(CORPSE);
            respond(&mut world, CORPSE, 1);
            world.init_resource::<crate::names::NameCache>();
            super::super::feed_loot_once(&mut world);
            deselects(&mut world);
            world.resource_mut::<LootLatch>().0 = Some(CHEST);
            owed.owe(&mut world);
            respond(&mut world, CHEST, 1);
            drain(&mut world);
            assert_eq!(
                world.resource::<LootState>().source(),
                Some(CHEST),
                "{owed:?}"
            );
            assert_eq!(releases(&rx), [CORPSE], "{owed:?}");
            assert_eq!(deselects(&mut world), [CORPSE], "{owed:?}");
        }
    }

    /// A close owed after the response, by a GO behind it or a step, still takes its window in
    /// the drain.
    #[test]
    fn a_close_owed_after_the_response_takes_its_window() {
        for (owed, flags) in Owed::BOTH {
            let (mut world, rx) = tabbed_world();
            seat_drain(&mut world, crate::player::Player::with_move_flags(flags));
            world.resource_mut::<LootLatch>().0 = Some(CHEST);
            respond(&mut world, CHEST, 1);
            owed.owe(&mut world);
            drain(&mut world);
            assert_eq!(world.resource::<LootState>().source(), None, "{owed:?}");
            assert_eq!(world.resource::<LootLatch>().0, None, "{owed:?}");
            assert_eq!(releases(&rx), [CHEST], "{owed:?}");
        }
    }

    /// Two responses on the run in one drain: each one's own tail (`0x5ebc51`) closes its own
    /// window after its copier, so each source is released once, the first not again by the
    /// second's handler.
    #[test]
    fn two_responses_on_the_run_in_one_drain_release_each_window_once() {
        let mut app = App::new();
        let rx = seat_tabbed(app.world_mut());
        seat_drain(
            app.world_mut(),
            crate::player::Player::with_move_flags(crate::creature_anim::move_flags::FORWARD),
        );
        app.init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::names::NameCache>()
            .insert_non_send_resource(benilla_ui::script::UiScript::new().expect("VM"))
            .add_systems(
                Update,
                (super::super::loot_frame, super::super::drain_loot).chain(),
            );
        app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(app.world_mut(), CORPSE, 1);
        app.world_mut().resource_mut::<LootLatch>().0 = Some(CHEST);
        respond(app.world_mut(), CHEST, 1);
        app.update();
        assert_eq!(releases(&rx), [CORPSE, CHEST]);
        assert_eq!(app.world().resource::<LootState>().source(), None);
    }

    /// With the byte the click stored set, the copier `0x4c1cb0` sweeps the window (`0x4c1fa0`)
    /// before the response's movement close (`0x5ebc51`), so a body auto-looted on the run is
    /// taken, then released.
    #[test]
    fn auto_loot_on_the_run_takes_the_rows_before_the_release() {
        const TOUGH_JERKY: u32 = 117;
        let mut app = App::new();
        let rx = seat_tabbed(app.world_mut());
        seat_drain(
            app.world_mut(),
            crate::player::Player::with_move_flags(crate::creature_anim::move_flags::FORWARD),
        );
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .insert_template(
                TOUGH_JERKY,
                Some(crate::items::test_template("Tough Jerky")),
            );
        app.insert_resource(super::super::AutoLootByte(true))
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::names::NameCache>()
            .insert_non_send_resource(benilla_ui::script::UiScript::new().expect("VM"))
            .add_systems(
                Update,
                (super::super::loot_frame, super::super::drain_loot).chain(),
            );

        app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
        app.world_mut()
            .run_system_once_with(
                on_response,
                SessionEvent::LootResponse {
                    guid: CORPSE,
                    loot_type: 1,
                    gold: 0,
                    items: vec![benilla_protocol::messages::LootItem {
                        slot: 0,
                        item_id: TOUGH_JERKY,
                        count: 1,
                        display_info_id: 1117,
                        random_property_id: 0,
                        slot_type: 0,
                    }],
                },
            )
            .expect("the handler runs as a one-shot system");
        app.update();
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::SetSelection { .. },
                    ClientCommand::AutostoreLootItem { slot: 0 },
                    ClientCommand::LootRelease { guid },
                ] if guid == CORPSE
            ),
            "{sent:?}"
        );
    }

    /// A copier waiting on a record ends at the answer (`0x4c2afc`), ahead of a release in the
    /// same frame: its sweep belongs to its own window while it is open, so a window the release
    /// closed since sweeps nothing and signals `LOOT_OPENED` as an unswept end does.
    #[test]
    fn a_sweep_owed_to_a_window_closed_since_sends_nothing() {
        const LINEN: u32 = 2589;
        let mut app = App::new();
        let rx = seat_tabbed(app.world_mut());
        seat_drain(app.world_mut(), crate::player::Player::with_move_flags(0));
        app.insert_resource(super::super::AutoLootByte(true))
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::names::NameCache>()
            .insert_non_send_resource(benilla_ui::script::UiScript::new().expect("VM"))
            .add_systems(
                Update,
                (super::super::loot_frame, super::super::drain_loot).chain(),
            );
        app.world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .run(
                "OPENS = 0 local f = CreateFrame('Frame') f:RegisterEvent('LOOT_OPENED') \
                 f:SetScript('OnEvent', function() OPENS = OPENS + 1 end)",
            )
            .unwrap();
        let world = app.world_mut();
        world.resource_mut::<LootLatch>().0 = Some(CORPSE);
        world
            .run_system_once_with(
                on_response,
                SessionEvent::LootResponse {
                    guid: CORPSE,
                    loot_type: 1,
                    gold: 0,
                    items: vec![benilla_protocol::messages::LootItem {
                        slot: 0,
                        item_id: LINEN,
                        count: 1,
                        display_info_id: 1,
                        random_property_id: 0,
                        slot_type: 0,
                    }],
                },
            )
            .expect("the handler runs as a one-shot system");
        world
            .resource_mut::<crate::items::Items>()
            .insert_template(LINEN, Some(crate::items::test_template("Linen Cloth")));
        world.resource_scope(|world, mut loot: Mut<LootState>| {
            loot.row_answered(world.resource::<crate::items::Items>())
        });
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse {
                    guid: CORPSE,
                    result: 1,
                },
            )
            .expect("the handler runs as a one-shot system");
        rx.try_iter().for_each(drop);
        app.update();
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(sent.is_empty(), "{sent:?}");
        let opens = app
            .world_mut()
            .non_send_resource_mut::<benilla_ui::script::UiScript>()
            .eval::<i64>("return OPENS")
            .unwrap();
        assert_eq!(opens, 1);
    }

    /// The issue's spot on the stock frames: a body's window that opens on the run is shown by
    /// `LOOT_OPENED` and hidden by `LOOT_CLOSED` in the same frame (`LootFrame.lua:13-16`,
    /// `:53-55`), its one release
    /// sent, and `TargetFrame` lets the body go; standing, the window and the target stay.
    #[test]
    fn the_stock_loot_frame_closes_on_a_body_looted_on_the_run() {
        use crate::creature_anim::move_flags as f;
        use benilla_ui::script::UiScript;
        benilla_formats::wow_data_or_skip!();
        for (flags, stays) in [(f::FORWARD, false), (0, true)] {
            let mut app = stock_app(&[
                crate::ui_script::test_ui::LOOT_UI,
                TARGET_UI,
                &["Interface\\FrameXML\\LootFrame.xml"],
            ]);
            let (tx, rx) = crossbeam_channel::unbounded();
            app.insert_resource(crate::net::NetCommands(tx));
            seat_drain(
                app.world_mut(),
                crate::player::Player::with_move_flags(flags),
            );
            app.init_resource::<super::super::LootConfig>().add_systems(
                Update,
                (
                    super::super::loot_frame,
                    super::super::drain_loot,
                    crate::target::fire_owed_selects_lane,
                )
                    .chain(),
            );
            app.world()
                .non_send_resource::<UiScript>()
                .run(
                    "LOOT_EVENTS = {} local f = CreateFrame('Frame') \
                     f:RegisterEvent('LOOT_OPENED') f:RegisterEvent('LOOT_CLOSED') \
                     f:SetScript('OnEvent', function() tinsert(LOOT_EVENTS, event) end)",
                )
                .unwrap();
            app.update();

            app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
            respond(app.world_mut(), CORPSE, 1);
            // One frame: the reference opens and closes in the response's handler.
            app.update();
            let (events, shown): (String, bool) = app
                .world()
                .non_send_resource::<UiScript>()
                .eval("return table.concat(LOOT_EVENTS, ','), LootFrame:IsShown() == 1")
                .unwrap();
            for _ in 0..2 {
                app.update();
            }
            target_changed(&mut app);
            let releases = rx
                .try_iter()
                .filter(|c| matches!(c, ClientCommand::LootRelease { guid } if *guid == CORPSE))
                .count();
            if stays {
                assert_eq!(events, "LOOT_OPENED");
                assert!(shown, "standing, the window stays");
                assert_eq!(releases, 0);
                assert_eq!(target_frame_name(&app), "Kobold Worker");
            } else {
                assert_eq!(events, "LOOT_OPENED,LOOT_CLOSED");
                assert!(
                    !shown,
                    "on the run, the window closes in the frame it opens"
                );
                assert_eq!(releases, 1, "released once");
                assert_eq!(target_frame_name(&app), "", "and the body is let go");
            }
        }
    }

    /// Only a releasing arm reaches `UnlockItem 0x495420`, through the tail `0x5ebac2`.
    #[test]
    fn a_releasing_loot_error_on_the_opened_item_unlocks_it() {
        let mut world = opened_lockbox_world();
        world.resource_mut::<LootState>().clear();
        // PLAYER_NOT_FOUND (10) does not reach the tail: nothing unlocks.
        world
            .run_system_once_with(
                on_error,
                SessionEvent::LootError {
                    guid: LOCKBOX,
                    error: 10,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert!(world.resource::<LootState>().queued_unlocks().is_empty());
        // TOO_FAR (4) does, at the packet.
        world
            .run_system_once_with(
                on_error,
                SessionEvent::LootError {
                    guid: LOCKBOX,
                    error: 4,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert_eq!(world.resource::<LootState>().queued_unlocks(), [LOCKBOX]);
    }

    const ME: u64 = 0x0000_0000_0000_002A;
    const THEM: u64 = 0x0000_0000_0000_00FF;

    fn push(player_guid: u64, show_in_chat: bool) -> ItemPushResult {
        ItemPushResult {
            player_guid,
            from_npc: false,
            created: false,
            show_in_chat,
            bag_slot: 0xFF,
            item_slot: 0,
            item_entry: 2589,
            suffix_factor: 0,
            random_property_id: 0,
            count: 1,
        }
    }

    /// Another member's push takes `0x491d9f`: no line for an item pushed but not created
    /// (`0x491d9f`-`0x491dab`), nor with `showInChat` clear (`0x491db1`); any other queues for its
    /// line.
    #[test]
    fn another_members_push_queues_for_its_line() {
        let me = SelfGuid(Some(ME));
        let queued = |from_npc: bool, created: bool, show_in_chat: bool| {
            let mut loot = LootState::default();
            item_push_result(
                ItemPushResult {
                    from_npc,
                    created,
                    ..push(THEM, show_in_chat)
                },
                &me,
                &mut loot,
                |_| true,
            );
            loot.pending_receive_count()
        };
        assert_eq!(queued(false, false, true), 1, "looted");
        assert_eq!(queued(false, true, true), 1, "created");
        assert_eq!(queued(true, true, true), 1, "created and pushed");
        assert_eq!(queued(true, false, true), 0, "pushed, not created");
        assert_eq!(queued(false, false, false), 0, "not for the chat");
    }

    #[test]
    fn only_our_own_pushes_reach_the_receive_queue() {
        let me = SelfGuid(Some(ME));
        assert!(is_our_push(&push(ME, true), &me));
        assert!(!is_our_push(&push(THEM, true), &me));
        // No guid before login: no active player to match.
        assert!(!is_our_push(&push(ME, true), &SelfGuid(None)));
        // `showInChat` is not this gate: a silent push still queues and animates.
        assert!(is_our_push(&push(ME, false), &me));
        let mut loot = LootState::default();
        item_push_result(push(ME, false), &me, &mut loot, |_| true);
        assert_eq!(loot.pending_receive_count(), 1);
    }
}
