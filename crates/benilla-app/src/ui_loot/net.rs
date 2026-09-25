//! The loot window's packet handlers: the [`LootState`] session and the [`LootLatch`] the feed
//! reads, the fishing verdicts, and the item push behind "You receive …".

use benilla_protocol::messages::{ItemPushResult, LootItem};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{LootLatch, LootState};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfGuid};
use crate::pending_item_ops::{LockTransitions, PendingItemOps};
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
        .net_handler(K::FishNotHooked, on_fish_verdict)
        .net_handler(K::FishEscaped, on_fish_verdict)
        .net_handler(K::ItemPushResult, on_item_push_result)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_response(
    In(ev): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    mut latch: ResMut<LootLatch>,
    commands: Res<NetCommands>,
) {
    if let SessionEvent::LootResponse {
        guid,
        loot_type,
        gold,
        items,
    } = ev
    {
        loot_response(
            guid, loot_type, gold, items, &mut loot, &mut latch, &commands,
        );
    }
}

fn on_error(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<UiErrorKeys>,
    mut latch: ResMut<LootLatch>,
    mut pending: ResMut<PendingItemOps>,
    mut lock_cleared: ResMut<LockTransitions>,
) {
    if let SessionEvent::LootError { guid, error } = ev {
        let unlock = ItemUnlock {
            pending: &mut pending,
            lock_cleared: &mut lock_cleared,
        };
        loot_error(guid, error, &mut errors, &mut latch, unlock);
    }
}

fn on_removed(In(ev): In<SessionEvent>, mut loot: ResMut<LootState>) {
    if let SessionEvent::LootRemoved { slot } = ev {
        loot_removed(slot, &mut loot);
    }
}

fn on_money_notify(In(ev): In<SessionEvent>) {
    if let SessionEvent::LootMoneyNotify { amount } = ev {
        loot_money_notify(amount);
    }
}

fn on_clear_money(In(ev): In<SessionEvent>, mut loot: ResMut<LootState>) {
    if let SessionEvent::LootClearMoney = ev {
        loot_clear_money(&mut loot);
    }
}

fn on_release_response(
    In(ev): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    mut latch: ResMut<LootLatch>,
    mut pending: ResMut<PendingItemOps>,
    mut lock_cleared: ResMut<LockTransitions>,
    mut dead: super::DeadUnitDeselect,
) {
    if let SessionEvent::LootReleaseResponse { guid } = ev {
        let unlock = ItemUnlock {
            pending: &mut pending,
            lock_cleared: &mut lock_cleared,
        };
        dead.after_close(loot_release_response(guid, &mut loot, &mut latch, unlock));
    }
}

/// `UnlockItem 0x495420` on the loot guid, as the loot closes call it: only an item we locked at
/// its `CMSG_OPEN_ITEM` send unlocks, and its slot queues for `ITEM_LOCK_CHANGED`.
struct ItemUnlock<'a> {
    pending: &'a mut PendingItemOps,
    lock_cleared: &'a mut LockTransitions,
}

impl ItemUnlock<'_> {
    fn unlock(self, guid: u64) {
        self.lock_cleared.0.extend(self.pending.clear_by_guid(guid));
    }
}

fn on_master_list(In(ev): In<SessionEvent>, mut loot: ResMut<LootState>) {
    if let SessionEvent::LootMasterList { candidates } = ev {
        loot_master_list(candidates, &mut loot);
    }
}

fn on_fish_verdict(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    match ev {
        SessionEvent::FishNotHooked => fish_verdict(false, &mut errors),
        SessionEvent::FishEscaped => fish_verdict(true, &mut errors),
        _ => {}
    }
}

fn on_item_push_result(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    mut loot: ResMut<LootState>,
    mut tutorials: ResMut<crate::tutorial::Tutorials>,
) {
    if let SessionEvent::ItemPushResult(p) = ev {
        item_push_result(p, &self_guid, &mut loot, &mut tutorials);
    }
}

/// The open window and the latch die with the session, unconditionally.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    mut latch: ResMut<LootLatch>,
) {
    loot.clear_session();
    latch.0 = None;
}

/// The `loot_type`s a cold latch admits (`0x5eb94b`, `0x5eb953`, `0x5eb95b`): pickpocketing,
/// fishing and disenchanting, which the server starts. `CORPSE` (1) answers our own `CMSG_LOOT`
/// or `CMSG_OPEN_ITEM` (`SpellHandler.cpp:227`).
const SERVER_STARTED_LOOT: [u8; 3] = [2, 3, 4];

/// `SMSG_LOOT_RESPONSE`'s item shape, behind the admission gate `0x5eb924`: it opens when the
/// latch holds the packet's guid, or when the latch is cold and the type is server-started.
/// Anything else is refused (`0x5eb963`): no window, a `CMSG_LOOT_RELEASE` for the packet's guid,
/// and the latch cleared whatever it held. An open writes the latch (`0x5ebb60`) and plays no
/// anim; a chest already knelt at its `SMSG_SPELL_GO` (`0x6e831b`).
fn loot_response(
    guid: u64,
    loot_type: u8,
    gold: u32,
    items: Vec<LootItem>,
    loot: &mut LootState,
    latch: &mut LootLatch,
    net: &NetCommands,
) {
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
        return;
    }
    debug!(
        "net: loot response {guid:#x} type {loot_type} gold {gold} {} item(s)",
        items.len()
    );
    loot.open(guid, loot_type, gold, items);
    // A fishing bobber first arms here and still does not kneel: `LootKneel` decides the pose.
    latch.0 = Some(guid);
}

/// `SMSG_FISH_ESCAPED` (the skill roll failed) or `SMSG_FISH_NOT_HOOKED` (clicked before the
/// splash, or expired): a yellow toast, as the handlers `0x5e3fc5`/`0x5e3fe2` push ids
/// `0x13e`/`0x13f`, type-1 entries that `DisplayError` fires as `UI_INFO_MESSAGE` (`0x49684b`).
fn fish_verdict(escaped: bool, errors: &mut UiErrorKeys) {
    let key = if escaped {
        "ERR_FISH_ESCAPED"
    } else {
        "ERR_FISH_NOT_HOOKED"
    };
    debug!("net: fishing verdict {key}");
    errors.0.push(UiError::key(key));
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

/// `SMSG_LOOT_RESPONSE`'s error shape: the arm's message by key, as `DisplayError(stringId)` takes
/// its surface and voice from the record; a releasing arm drops the latch and runs
/// `UnlockItem 0x495420` on the packet's guid (the tail `0x5ebac2`). There is no admission gate
/// here: a guid-mismatched error still shows and keeps the latch, where the reference refuses it
/// at `0x5eb963`, clearing the latch and showing nothing.
fn loot_error(
    guid: u64,
    error: u8,
    errors: &mut UiErrorKeys,
    latch: &mut LootLatch,
    unlock: ItemUnlock,
) {
    let LootRefusal { key, releases } = loot_refusal(error);
    debug!("net: loot error {error} on {guid:#x} → {key} (releases: {releases})");
    errors.0.push(UiError::key(key));
    if releases {
        // Guid-matched: the tail's store is unconditional, but only a matching latch reaches it.
        latch.clear_for(guid);
        unlock.unlock(guid);
    }
}

/// `SMSG_LOOT_REMOVED`: a row was taken, by anyone; the feed fires `LOOT_SLOT_CLEARED` for it.
fn loot_removed(slot: u8, loot: &mut LootState) {
    debug!("net: loot slot {slot} removed");
    loot.remove_slot(slot);
}

/// `SMSG_LOOT_MONEY_NOTIFY`, our share of a group's coin: only logged, as the coin row drops on
/// `SMSG_LOOT_CLEAR_MONEY` and the purse rides the coinage field. vmangos sends none for a solo
/// loot (`LootHandler.cpp:331`).
fn loot_money_notify(amount: u32) {
    debug!("net: loot money {amount}");
}

/// `SMSG_LOOT_CLEAR_MONEY`: the coin row goes, for every looter.
fn loot_clear_money(loot: &mut LootState) {
    debug!("net: loot coin line cleared");
    loot.clear_money();
}

/// `SMSG_LOOT_RELEASE_RESPONSE`, idempotent after our own close. The latch clear is guid-matched,
/// as the reference's (`0x5ec0d4`), so an old window's release keeps a newer request's latch.
/// It unlocks an opened item (`0x5ec090` → `0x48f200(cl=0, dl=0)` → `UnlockItem 0x495420`): the
/// only clear a lockbox closed with loot left gets, as vmangos destroys only a fully looted item
/// (`LootHandler.cpp:558`). The open window takes the shared close without a send (`0x5ec0f3`);
/// the closed source is returned.
fn loot_release_response(
    guid: u64,
    loot: &mut LootState,
    latch: &mut LootLatch,
    unlock: ItemUnlock,
) -> Option<u64> {
    debug!("net: loot released {guid:#x}");
    latch.clear_for(guid);
    unlock.unlock(guid);
    super::close_interaction(loot, latch, None)
}

/// `SMSG_LOOT_MASTER_LIST`, staged: it lands just ahead of the response it belongs to.
fn loot_master_list(candidates: Vec<u64>, loot: &mut LootState) {
    debug!("net: master-loot candidates: {} eligible", candidates.len());
    loot.set_master_candidates(candidates);
}

/// `OnItemPush 0x491a60`'s outermost gate (`0x491b56`/`0x491b61`): only our own push prints or
/// animates. Another member's push takes the `0x491d9f` arm and prints `LOOT_ITEM` with their
/// name, which is not built, so a foreign push is dropped. `showInChat` is a later gate
/// (`0x491bf3`/`0x491db1`), carried into [`LootState::push_receive`].
fn is_our_push(p: &ItemPushResult, self_guid: &SelfGuid) -> bool {
    self_guid.0 == Some(p.player_guid)
}

/// `SMSG_ITEM_PUSH_RESULT`: queues our own push for its chat line and its bag-bar animation.
fn item_push_result(
    p: ItemPushResult,
    self_guid: &SelfGuid,
    loot: &mut LootState,
    tutorials: &mut crate::tutorial::Tutorials,
) {
    debug!(
        "net: item push {} x{} → bag {} slot {:#x}",
        p.item_entry, p.count, p.bag_slot, p.item_slot
    );
    if !is_our_push(&p, self_guid) {
        return;
    }
    tutorials.item_received(p.item_entry, p.bag_slot, p.item_slot);
    loot.push_receive(&p);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    use crate::pending_item_ops::{LockTransitions, PendingItemOps};

    /// An [`ItemUnlock`] over no locks, for a loot source that is not an item.
    macro_rules! no_item {
        () => {
            ItemUnlock {
                pending: &mut PendingItemOps::default(),
                lock_cleared: &mut LockTransitions::default(),
            }
        };
    }

    #[test]
    fn fish_verdict_keys_resolve_in_the_real_global_strings() {
        let mut errors = UiErrorKeys::default();
        fish_verdict(false, &mut errors);
        fish_verdict(true, &mut errors);
        assert_eq!(
            errors.0,
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
            let mut errors = UiErrorKeys::default();
            let mut latch = LootLatch(Some(CORPSE));
            loot_error(CORPSE, code, &mut errors, &mut latch, no_item!());
            assert_eq!(latch.0, None, "code {code} should release");
            assert_eq!(errors.0.len(), 1, "code {code} shows exactly one line");
        }
        for code in [10u8, 12, 13, 14] {
            let mut errors = UiErrorKeys::default();
            let mut latch = LootLatch(Some(CORPSE));
            loot_error(CORPSE, code, &mut errors, &mut latch, no_item!());
            assert_eq!(
                latch.0,
                Some(CORPSE),
                "code {code} answers an OPEN window — the kneel stays"
            );
        }
    }

    /// The reference's behaviour: the tail's store is unconditional, but the error leg is reached
    /// only through `0x5eb93e`, which already required a matching latch.
    #[test]
    fn the_error_legs_clear_is_guid_matched() {
        let mut errors = UiErrorKeys::default();
        let mut latch = LootLatch(Some(CHEST));
        loot_error(CORPSE, 8, &mut errors, &mut latch, no_item!());
        assert_eq!(latch.0, Some(CHEST));
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
        let (net, rx) = net();
        let mut loot = LootState::default();
        let mut latch = LootLatch::default();
        assert_eq!(latch.0, None, "no CMSG_LOOT was sent, so nothing armed it");

        loot_response(CHEST, 2, 0, Vec::new(), &mut loot, &mut latch, &net);
        assert_eq!(latch.0, Some(CHEST), "the open window is the loot session");
        assert_eq!(loot.source(), Some(CHEST), "…and the window opened");
        assert!(
            rx.try_recv().is_err(),
            "an accepted response bounces nothing"
        );

        loot_release_response(CHEST, &mut loot, &mut latch, no_item!());
        assert_eq!(latch.0, None, "the release ends the session");
    }

    /// The `CMSG_LOOT` send armed the same guid: the match branch (`0x5eb93e`).
    #[test]
    fn a_matching_latch_admits_any_loot_type() {
        let (net, _rx) = net();
        let mut loot = LootState::default();
        let mut latch = LootLatch(Some(CORPSE)); // the `CMSG_LOOT` send
        loot_response(CORPSE, 1, 0, Vec::new(), &mut loot, &mut latch, &net);
        assert_eq!(latch.0, Some(CORPSE));
        assert_eq!(loot.source(), Some(CORPSE));
    }

    /// The refusal arm (`0x5eb963`): a type-1 answer on a cold latch answers nothing we asked.
    #[test]
    fn a_cold_latch_refuses_a_corpse_typed_response_and_bounces_it() {
        let (net, rx) = net();
        let mut loot = LootState::default();
        let mut latch = LootLatch::default();

        loot_response(CORPSE, 1, 0, Vec::new(), &mut loot, &mut latch, &net);
        assert_eq!(
            loot.source(),
            None,
            "no window for an unasked-for corpse answer"
        );
        assert_eq!(latch.0, None, "…and nothing latched");
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CORPSE),
            "the refusal releases the object the PACKET named"
        );
    }

    /// The refusal's clear is not guid-matched (`0x5eb9d2`): a response for B drops a latch on A.
    #[test]
    fn a_refusal_drops_whatever_latch_was_live_not_just_a_matching_one() {
        let (net, rx) = net();
        let mut loot = LootState::default();
        let mut latch = LootLatch(Some(CORPSE));

        loot_response(CHEST, 1, 0, Vec::new(), &mut loot, &mut latch, &net);
        assert_eq!(latch.0, None, "A's latch is dropped by B's refusal");
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::LootRelease { guid }) if guid == CHEST),
            "…and it is B that gets released"
        );
    }

    /// An item guid (`HIGHGUID_ITEM` 0x4000 in the high word): a lockbox.
    const LOCKBOX: u64 = 0x4000_0000_0000_0007;

    /// A world with the lockbox at bag 0 slot 3 locked, as its `CMSG_OPEN_ITEM` send leaves it.
    fn opened_lockbox_world() -> World {
        let mut world = World::new();
        world.init_resource::<LootState>();
        world.insert_resource(LootLatch(Some(LOCKBOX)));
        world.init_resource::<UiErrorKeys>();
        world.init_resource::<LockTransitions>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>();
        let mut pending = PendingItemOps::default();
        pending.add([(0, 3, LOCKBOX, 1)]);
        world.insert_resource(pending);
        world
    }

    /// With loot left, no field update ever resolves the lock: the release is the clear.
    #[test]
    fn a_release_for_the_opened_item_unlocks_its_slot() {
        let mut world = opened_lockbox_world();
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse { guid: LOCKBOX },
            )
            .expect("the handler runs as a one-shot system");
        assert!(
            !world.resource::<PendingItemOps>().contains(0, 3),
            "the lockbox is no longer grey and locked"
        );
        assert_eq!(
            world.resource::<LockTransitions>().0,
            vec![(0, 3)],
            "…and the container feed fires ITEM_LOCK_CHANGED for it"
        );
    }

    #[test]
    fn a_release_for_another_guid_leaves_the_lock() {
        let mut world = opened_lockbox_world();
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse { guid: CORPSE },
            )
            .expect("the handler runs as a one-shot system");
        assert!(world.resource::<PendingItemOps>().contains(0, 3));
        assert!(world.resource::<LockTransitions>().0.is_empty());
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
                SessionEvent::LootReleaseResponse { guid: CORPSE },
            )
            .expect("the handler runs as a one-shot system");
        assert_eq!(world.resource::<LootState>().source(), None);
        assert_eq!(world.resource::<LootLatch>().0, None);
        assert!(rx.try_recv().is_err(), "the server's release is not echoed");
        let asked: Vec<u64> = world
            .resource_mut::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>()
            .drain()
            .map(|d| d.0)
            .collect();
        assert_eq!(asked, [CORPSE]);

        // A second release finds nothing open and asks nothing.
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse { guid: CORPSE },
            )
            .expect("the handler runs as a one-shot system");
        assert!(world
            .resource_mut::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>()
            .drain()
            .next()
            .is_none());
    }

    /// Only a releasing arm reaches `UnlockItem 0x495420`, through the tail `0x5ebac2`.
    #[test]
    fn a_releasing_loot_error_on_the_opened_item_unlocks_it() {
        let mut world = opened_lockbox_world();
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
        assert!(world.resource::<PendingItemOps>().contains(0, 3));
        // TOO_FAR (4) does.
        world
            .run_system_once_with(
                on_error,
                SessionEvent::LootError {
                    guid: LOCKBOX,
                    error: 4,
                },
            )
            .expect("the handler runs as a one-shot system");
        assert!(!world.resource::<PendingItemOps>().contains(0, 3));
        assert_eq!(world.resource::<LockTransitions>().0, vec![(0, 3)]);
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
        item_push_result(
            push(ME, false),
            &me,
            &mut loot,
            &mut crate::tutorial::Tutorials::default(),
        );
        assert_eq!(loot.pending_receive_count(), 1);
    }
}
