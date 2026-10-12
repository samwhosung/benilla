//! The loot window's packet handlers: the [`LootState`] session and the [`LootLatch`] the feed
//! reads, the fishing verdicts, and the item push behind "You receive …".

use benilla_protocol::messages::ItemPushResult;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{LootLatch, LootMoveStart, LootState};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfGuid};
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

/// An admitted response selects a looted unit (`0x5ebc35` → `0x48f3a0`) before the window opens
/// (`0x4c1cb0`), so the outgoing target's teardown closes a window still open on it. One that lands
/// while our own body moves takes the move-start close (`0x5ebc3a`-`0x5ebc51`), which the feed runs
/// after it fires `LOOT_OPENED`, or the drain when the window waits on an item template.
fn on_response(
    In(ev): In<SessionEvent>,
    mut select: crate::target::SelectCommit,
    player: Option<Res<crate::player::Player>>,
    mut move_start: ResMut<LootMoveStart>,
) {
    let SessionEvent::LootResponse {
        guid,
        loot_type,
        gold,
        items,
    } = ev
    else {
        return;
    };
    let seam = &mut select.seam;
    if !admit_response(
        guid,
        loot_type,
        &mut seam.loot_latch,
        &mut seam.loot,
        &seam.net,
    ) {
        return;
    }
    debug!(
        "net: loot response {guid:#x} type {loot_type} gold {gold} {} item(s)",
        items.len()
    );
    select.select_unit(guid);
    select.seam.loot.open(guid, loot_type, gold, items);
    if player.is_some_and(|p| {
        p.foreign_mover.is_none()
            && p.move_flags() & crate::creature_anim::move_flags::STEERING != 0
    }) {
        move_start.0 = true;
    }
}

fn on_error(
    In(ev): In<SessionEvent>,
    mut errors: ResMut<UiErrorKeys>,
    mut latch: ResMut<LootLatch>,
    mut loot: ResMut<LootState>,
) {
    if let SessionEvent::LootError { guid, error } = ev {
        loot_error(guid, error, &mut errors, &mut latch, &mut loot);
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
    mut dead: super::DeadUnitDeselect,
) {
    if let SessionEvent::LootReleaseResponse { guid } = ev {
        dead.after_close(loot_release_response(guid, &mut loot, &mut latch));
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
/// the latch cleared whatever it held, and `UnlockItem` on the packet's guid (`0x5eb9de`). An
/// admitted one writes the latch (`0x5ebb60`) and plays no anim; a chest already knelt at its
/// `SMSG_SPELL_GO` (`0x6e831b`).
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
        loot.unlock_item(guid);
        return false;
    }
    // A fishing bobber first arms here and still does not kneel: `LootKneel` decides the pose.
    latch.0 = Some(guid);
    true
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
/// `UnlockItem 0x495420` on the packet's guid (the tail `0x5ebac2`, `0x5ebae0`). There is no admission gate
/// here: a guid-mismatched error still shows and keeps the latch, where the reference refuses it
/// at `0x5eb963`, clearing the latch and showing nothing.
fn loot_error(
    guid: u64,
    error: u8,
    errors: &mut UiErrorKeys,
    latch: &mut LootLatch,
    loot: &mut LootState,
) {
    let LootRefusal { key, releases } = loot_refusal(error);
    debug!("net: loot error {error} on {guid:#x} → {key} (releases: {releases})");
    errors.0.push(UiError::key(key));
    if releases {
        // Guid-matched: the tail's store is unconditional, but only a matching latch reaches it.
        latch.clear_for(guid);
        loot.unlock_item(guid);
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
/// The open window takes the shared close without a send (`0x5ec0f3`), which unlocks an item
/// source (`0x48f200(cl=0, dl=0)`, [`super::close_interaction`]): a lockbox closed with loot left
/// keeps its slot, as vmangos destroys only a fully looted item (`LootHandler.cpp:558`). The
/// closed source is returned.
fn loot_release_response(guid: u64, loot: &mut LootState, latch: &mut LootLatch) -> Option<u64> {
    debug!("net: loot released {guid:#x}");
    latch.clear_for(guid);
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

    /// A loot session with no window, whose owed unlocks a test may ignore.
    macro_rules! no_item {
        () => {
            &mut LootState::default()
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
                SessionEvent::LootReleaseResponse { guid: CHEST },
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
            world
                .resource_mut::<LootState>()
                .take_item_unlocks()
                .into_iter()
                .map(|(g, _)| g)
                .collect::<Vec<_>>(),
            [CHEST],
            "and `UnlockItem` runs on B's guid, an item or not (`0x5eb9de`)"
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
        world.init_resource::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>();
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
                SessionEvent::LootReleaseResponse { guid: LOCKBOX },
            )
            .expect("the handler runs as a one-shot system");
        assert_eq!(
            world
                .resource_mut::<LootState>()
                .take_item_unlocks()
                .into_iter()
                .map(|(g, _)| g)
                .collect::<Vec<_>>(),
            [LOCKBOX, LOCKBOX]
        );
        // With no window left, a second release unlocks nothing (`0x48f23f`).
        world
            .run_system_once_with(
                on_release_response,
                SessionEvent::LootReleaseResponse { guid: LOCKBOX },
            )
            .expect("the handler runs as a one-shot system");
        assert!(world
            .resource_mut::<LootState>()
            .take_item_unlocks()
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
            .take_item_unlocks()
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
            loot.take_item_unlocks()
                .into_iter()
                .map(|(g, _)| g)
                .collect::<Vec<_>>()
                .is_empty(),
            "the first window replaces nothing"
        );
        loot.open(CORPSE, 1, 0, Vec::new());
        assert_eq!(
            loot.take_item_unlocks()
                .into_iter()
                .map(|(g, _)| g)
                .collect::<Vec<_>>(),
            [LOCKBOX]
        );
        loot.open(CHEST, 2, 0, Vec::new());
        assert!(
            loot.take_item_unlocks()
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
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<bevy::ecs::message::Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<GuidIndex>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<crate::target::AssistAttack>();
        world.init_resource::<crate::target::Selection>();
        world.init_resource::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>();
        world.init_resource::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>();
        world.init_resource::<crate::player::Player>();
        world.init_resource::<super::super::LootMoveStart>();
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
        let hostile = (1u32..4096)
            .find(|&t| {
                crate::target::can_attack(
                    Some(&creature(t, 100)),
                    Some(&factions),
                    &reps,
                    Some(&me_store),
                )
            })
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
    ];

    /// [`tabbed_world`] under the stock `ui` files, with the unit feed and both creatures named.
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
        crate::ui_unit::add_unit_feed(&mut app);
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
    /// window opens, and the unit feed then names the body there (`TargetFrame.lua:63`,
    /// `UnitFrame.lua:25`).
    #[test]
    fn the_stock_target_frame_names_the_looted_body() {
        benilla_formats::wow_data_or_skip!();
        let mut app = stock_app(&[TARGET_UI]);

        app.update();
        assert_eq!(target_frame_name(&app), "Kobold Vermin", "the Tab target");

        app.world_mut().resource_mut::<LootLatch>().0 = Some(CORPSE);
        respond(app.world_mut(), CORPSE, 1);
        app.update();
        assert_eq!(target_frame_name(&app), "Kobold Worker", "the looted body");
    }

    /// What [`super::super::drain_loot`] reads beside [`seat_tabbed`]'s, with our body's movement
    /// word at `player`'s.
    fn seat_drain(world: &mut World, player: crate::player::Player) {
        world.insert_resource(player);
        world.init_resource::<crate::ui_party::GroupState>();
        world.init_resource::<crate::items::Items>();
        world.init_resource::<bevy::ecs::message::Messages<crate::sound::LootPickupSound>>();
    }

    /// The deselects asked for since the last call.
    fn deselects(world: &mut World) -> Vec<u64> {
        world
            .resource_mut::<bevy::ecs::message::Messages<crate::target::DeselectGuid>>()
            .drain()
            .map(|d| d.0)
            .collect()
    }

    /// A response on the corpse with `player` seated, then one drain.
    fn open_while(
        player: crate::player::Player,
    ) -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let (mut world, rx) = tabbed_world();
        seat_drain(&mut world, player);
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
            let (mut world, rx) = open_while(crate::player::Player::with_move_flags(bit));
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
        use crate::player::Player;
        let mut possessing = Player::with_move_flags(f::FORWARD);
        possessing.foreign_mover = Some(KOBOLD);
        for (what, player) in [
            ("still", Player::with_move_flags(0)),
            ("walk mode", Player::with_move_flags(f::WALK_MODE)),
            ("falling", Player::with_move_flags(f::FALLING)),
            ("swimming", Player::with_move_flags(f::SWIMMING)),
            ("possessing", possessing),
        ] {
            let (mut world, rx) = open_while(player);
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

    /// The copier `0x4c1cb0` sweeps an auto-loot window (`0x4c1fa0`) before the response's
    /// movement close (`0x5ebc51`), so a body auto-looted on the run is taken, then released.
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
        app.insert_resource(super::super::LootConfig {
            auto_loot: true,
            ..Default::default()
        })
        .init_resource::<crate::ui_chat::ChatLog>()
        .init_resource::<crate::names::NameCache>()
        .init_resource::<ButtonInput<KeyCode>>()
        .insert_non_send_resource(benilla_ui::script::UiScript::new().expect("VM"))
        .add_systems(
            Update,
            (super::super::feed_loot, super::super::drain_loot).chain(),
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
            app.init_resource::<super::super::LootConfig>()
                .init_resource::<ButtonInput<KeyCode>>()
                .add_systems(
                    Update,
                    (
                        super::super::feed_loot,
                        super::super::drain_loot,
                        crate::target::click::clear_target_requests,
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
        assert!(world
            .resource_mut::<LootState>()
            .take_item_unlocks()
            .into_iter()
            .map(|(g, _)| g)
            .collect::<Vec<_>>()
            .is_empty());
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
        assert_eq!(
            world
                .resource_mut::<LootState>()
                .take_item_unlocks()
                .into_iter()
                .map(|(g, _)| g)
                .collect::<Vec<_>>(),
            [LOCKBOX]
        );
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
