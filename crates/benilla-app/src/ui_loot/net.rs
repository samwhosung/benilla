//! The loot window's packet handlers (in the net handler table since 2319, moved
//! out of the drain's loot arm file) — the [`LootState`] session and the [`LootLatch`] the loot
//! feed ([`super`]) reads, the fishing verdicts, and the item push that prints "You receive …".
//! The group rolls are [`crate::ui_loot_roll`]'s and the inventory refusal is
//! [`crate::ui_items`]'s.

use benilla_protocol::messages::{ItemPushResult, LootItem};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{LootLatch, LootState};
use crate::net::{ClientCommand, NetCommands, NetHandlerApp, SelfGuid};
use crate::pending_item_ops::{LockTransitions, PendingItemOps};
use crate::ui_action::{UiError, UiErrorKeys};

/// Register the loot handlers — called from [`super::UiLootPlugin`]. One per kind, plus the
/// session-end listener.
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
) {
    if let SessionEvent::LootReleaseResponse { guid } = ev {
        let unlock = ItemUnlock {
            pending: &mut pending,
            lock_cleared: &mut lock_cleared,
        };
        loot_release_response(guid, &mut loot, &mut latch, unlock);
    }
}

/// **`UnlockItem 0x495420`** as the loot closes call it — on the loot guid, which unlocks
/// something only when that guid is an item we locked (a lockbox, clam or loot bag, locked at its
/// `CMSG_OPEN_ITEM` send). The unlocked slots queue in [`LockTransitions`] for the
/// container feed to fire `ITEM_LOCK_CHANGED`, like the inventory failure's clear.
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

/// An open loot window and the kneel latch die with the socket (unconditionally). A listener on
/// the session end (a second handler on the kind, after the bridge's own teardown).
fn on_session_end(
    In(_): In<SessionEvent>,
    mut loot: ResMut<LootState>,
    mut latch: ResMut<LootLatch>,
) {
    loot.clear_session();
    latch.0 = None;
}

/// The wire `loot_type` values `SMSG_LOOT_RESPONSE`'s **cold-latch** admission accepts
/// (`0x5eb94b`/`0x5eb953`/`0x5eb95b`) — vmangos `LootType`'s `PICKPOCKETING(2)` · `FISHING(3)` ·
/// `DISENCHANTING(4)`. The alphabet the bytes are drawing is *"the server started this session"*;
/// `CORPSE(1)` is the client's own, and it is the one value a cold latch refuses.
const SERVER_STARTED_LOOT: [u8; 3] = [2, 3, 4];

/// A loot window opened (`SMSG_LOOT_RESPONSE`'s normal shape) — the answer to our `CMSG_LOOT`, or
/// to anything else that opened a session: a chest, herb node, mining vein or fishing bobber all
/// reach the window through an `OPEN_LOCK` cast, never through `CMSG_LOOT` (vmangos
/// `Spell::SendLoot` → `Player::SendLoot`).
///
/// **This handler is an admission gate, not an unconditional open** (decision 1477 correcting
/// 1471). `0x5eb924`:
///
/// ```text
/// ACCEPT ⇔ (latch != 0 && latch == pkt.guid) || (latch == 0 && loot_type ∈ {2,3,4})
/// ```
///
/// Everything else is **refused** at `0x5eb963`: no window, a `CMSG_LOOT_RELEASE` bounced back for
/// the *packet's* guid, and the latch cleared — **not** guid-matched, so an unsolicited response
/// for B drops a live latch on A. The design the bytes state is a two-way handshake: type 1 means
/// "I asked for this", and the client pre-armed at its own `CMSG_LOOT` send, so a type-1 answer
/// with nothing latched is an answer to a question we never asked.
///
/// The accepted arm writes the latch verbatim (`0x5ebb60`) and — verified over the whole success
/// path — calls **neither** the base-anim recompute nor the Loot-50 force-play. A chest is already
/// kneeling by now: its arm was `SMSG_SPELL_GO` (`0x6e831b`, [`super::spells`]), one packet
/// earlier.
fn loot_response(
    guid: u64,
    loot_type: u8,
    gold: u32,
    items: Vec<LootItem>,
    loot: &mut LootState,
    latch: &mut LootLatch,
    net: &NetCommands,
) {
    // `0x5eb924`–`0x5eb95b`, transcribed.
    let accept = match latch.0 {
        Some(latched) => latched == guid,
        None => SERVER_STARTED_LOOT.contains(&loot_type),
    };
    if !accept {
        // `0x5eb963`: bounce it. On this server nothing produces a refusal — a corpse always
        // pre-arms and a chest/fishing answer carries type 2/3 — so this arm is inert today and
        // carried for the faithful shape (and for a server that answers differently).
        debug!(
            "net: loot response {guid:#x} type {loot_type} REFUSED (latch {:?})",
            latch.0
        );
        if loot_type != 0 {
            let _ = net.0.send(ClientCommand::LootRelease { guid });
        }
        latch.0 = None; // NOT guid-matched — `0x5eb9d2` clears whatever was there
        return;
    }
    // The loot window opens: fill LootState from the wire; the feed
    // ([`super`]) resolves rows + fires LOOT_OPENED next frame. `loot_type` rides along
    // for `IsFishingLoot()`.
    debug!(
        "net: loot response {guid:#x} type {loot_type} gold {gold} {} item(s)",
        items.len()
    );
    loot.open(guid, loot_type, gold, items);
    // `0x5ebb60` — the packet's guid, verbatim. Re-arming a corpse's already-matching latch is a
    // no-op; a chest re-arms what `SMSG_SPELL_GO` armed; a **fishing** bobber arms here for the
    // first time and still does not kneel, because the pose is predicate B's call, not the
    // latch's ([`super::LootKneel`]).
    latch.0 = Some(guid);
}

/// A fishing verdict with no loot window (`SMSG_FISH_ESCAPED` / `SMSG_FISH_NOT_HOOKED`, both
/// empty-bodied): the **yellow** toast by GlobalStrings key — `ERR_FISH_ESCAPED`
/// ("Your fish got away!") when the skill roll failed on the click, `ERR_FISH_NOT_HOOKED`
/// ("No fish are hooked.") when the bobber expired or was clicked before the splash. Yellow, not
/// red: the reference handlers (`0x5e3fc5`/`0x5e3fe2` → `DisplayError` ids `0x13e`/`0x13f`) are
/// **type-1** registry entries, which fire `UI_INFO_MESSAGE` (`DisplayError`'s type-1 arm
/// `0x49684b`), correcting 1086's shipped guess.
fn fish_verdict(escaped: bool, errors: &mut UiErrorKeys) {
    let key = if escaped {
        "ERR_FISH_ESCAPED"
    } else {
        "ERR_FISH_NOT_HOOKED"
    };
    debug!("net: fishing verdict {key}");
    errors.0.push(UiError::key(key));
}

/// The **default** arm releases.
///
/// The out-of-range branch is not merely *adjacent* to code 7's arm, it **is** code 7's arm:
///
/// ```text
/// 5eba11: 0f 87 a6 00 00 00   ja 0x5ebabd   ; 0x5eba17 + 0xa6 = 0x5ebabd
/// 5ebabd: 68 83 00 00 00      push 0x83     ; 5 bytes, ending exactly at 0x5ebac2
/// 5ebac2: e8 59 ac ea ff      call 0x496720 ; the release tail, reached by FALL-THROUGH
/// ```
///
/// Jump-table slot 3 (`0x5ebc70`) holds the same `bd ba 5e 00`, a dword that occurs exactly once
/// in the image. So every code the table does not name shows `ERR_LOOT_DIDNT_KILL` **and** ends
/// the kneel — and the default's reach is wider than the enum suggests: `0x5eba07` is a `movzx`
/// and the `ja` is **unsigned**, so 0..3 and 15..255 all land here.
const DEFAULT_ARM_RELEASES: bool = true;

/// One arm of the loot-refusal jump table: the message it displays, and whether it falls through
/// the **release tail**.
///
/// Both halves are the reference's, read off the same eleven-entry table — each arm is a
/// `push <stringId>` that either falls into the tail at `0x5ebac2` or jumps past it, so "what does
/// it say" and "does the kneel end" are one decision in the bytes and one decision here.
struct LootRefusal {
    /// The `GlobalStrings` key of the id the arm hands `CGGameUI::DisplayError` (`0x496720`).
    key: &'static str,
    /// Whether the arm falls into the release tail at `0x5ebac2` — zero the pending-loot guid
    /// (`[player+0x1d28/+0x1d2c]`), `UnlockItem 0x495420`, `RecomputeBaseAnim 0x5fd9e0(-1)`.
    /// That guid **is** benilla's [`LootLatch`], so this is exactly what ends the
    /// client-predicted loot kneel.
    releases: bool,
}

/// The refusal a `SMSG_LOOT_RESPONSE` error code produces — **the reference's own jump table**,
/// transcribed.
///
/// The error leg is entered on `lootType == 0` (`0x5eb9f4`), reads the code (`0x5eba02`) and
/// dispatches `0x5eba0b add eax,-4; cmp eax,0xa; ja <default>; jmp [4*eax + 0x5ebc64]`. The
/// eleven-entry table read out of the PE gives:
///
/// | wire code | stringId | key | releases |
/// |---|---|---|---|
/// | 4 `TOO_FAR` | `0x82` (130) | `ERR_LOOT_TOO_FAR` | yes |
/// | 5 `BAD_FACING` | `0x84` (132) | `ERR_LOOT_BAD_FACING` | yes |
/// | 6 `LOCKED` | `0x81` (129) | `ERR_LOOT_LOCKED` | yes |
/// | 7 (unnamed server-side) | `0x83` (131) | `ERR_LOOT_DIDNT_KILL` | yes |
/// | 8 `NOTSTANDING` | `0x85` (133) | `ERR_LOOT_NOTSTANDING` | yes |
/// | 9 `STUNNED` | `0x86` (134) | `ERR_LOOT_STUNNED` | yes |
/// | 10 `PLAYER_NOT_FOUND` | `0x19b` (411) | `ERR_LOOT_PLAYER_NOT_FOUND` | **no** |
/// | 11 `PLAY_TIME_EXCEEDED` | `0x1bf` (447) | `ERR_PLAY_TIME_EXCEEDED` | yes |
/// | 12 `MASTER_INV_FULL` | `0x1cd` (461) | `ERR_LOOT_MASTER_INV_FULL` | **no** |
/// | 13 `MASTER_UNIQUE_ITEM` | `0x1ce` (462) | `ERR_LOOT_MASTER_UNIQUE_ITEM` | **no** |
/// | 14 `MASTER_OTHER` | `0x1cf` (463) | `ERR_LOOT_MASTER_OTHER` | **no** |
///
/// **The four that do not release are the four that arrive at an already-open window** — the
/// master looter's three `CMSG_LOOT_MASTER_GIVE` refusals and the dead/absent
/// recipient. The corpse stays open and the character stays kneeling, which is why the reference
/// jumps past the tail for exactly these.
///
/// **Everything outside 4..=14 takes the default arm**, `ERR_LOOT_DIDNT_KILL` — including code
/// `0` (`DIDNT_KILL` itself, which reaches its own sentence only through the default) and vmangos's
/// `ALREADY_PICKPOCKETED` (15) / `NOT_WHILE_SHAPESHIFTED` (16). 1.12 ships **no** string for an
/// already-picked pocket or a shapeshifted refusal; the client says "You don't have permission to
/// loot that corpse." for both, and inventing a better sentence is not fidelity.
fn loot_refusal(reason: u8) -> LootRefusal {
    use benilla_protocol::messages::loot_error as e;
    // `7` has no vmangos name — it is a gap in the server enum — but it is a real arm of the
    // client's table, and a *releasing* one, which is why it cannot simply fall to the default.
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

/// The server refused to open the loot window (`SMSG_LOOT_RESPONSE`'s error shape — didn't kill
/// it, too far, not standing, …): the message by **GlobalStrings key**, never a sentence of ours,
/// and the kneel dropped only on the arms that release ([`loot_refusal`]).
///
/// The key route is not a style preference. The reference displays every one of these through
/// `DisplayError(stringId)`, so the *surface* (red `UIErrorsFrame`, from the catalog row's `+0x04`)
/// and the *voice line* (`+0x0c` — four of these speak in the player's own race and gender) are
/// properties of the message, and a hardcoded English string throws all three away along with
/// localization. Before this, benilla composed its own eight sentences here and six of them said
/// something the client never says.
///
/// The arms that release run the reference's release tail `0x5ebac2`, which also calls
/// `UnlockItem 0x495420` on the packet's guid: a refused lockbox open drops the item's pending
/// lock. For a corpse or chest guid there is no item, and nothing unlocks.
///
/// **A gap this deliberately does not close** (named here rather than left to a later bug
/// report):
///
/// 1. **A guid-MISMATCHED error takes a different arm in the reference.** The error leg is
///    reachable only past the admission gate, so the reference only ever *displays* a refusal for
///    the session it asked about; a mismatched one falls to `0x5eb963`, which clears the latch
///    unconditionally, shows **nothing**, and (because `lootType == 0`) sends nothing. benilla has
///    no admission gate on the error shape — it displays the line and keeps the latch. Reachable
///    only under the corpse-switch race, and it belongs with the gate (1477), not with the text.
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
        // The latch armed at the `CMSG_LOOT` send drops (guid-matched — see [`LootLatch`]), or the
        // character would kneel forever at a corpse whose window never opened.
        latch.clear_for(guid);
        unlock.unlock(guid);
    }
}

/// One loot-window row was taken, by anyone (`SMSG_LOOT_REMOVED`) — the UI clears that row.
fn loot_removed(slot: u8, loot: &mut LootState) {
    // A row was taken (by anyone): drop it; the feed repaints via LOOT_UPDATE.
    debug!("net: loot slot {slot} removed");
    loot.remove_slot(slot);
}

/// Our share of the loot's coin pile (`SMSG_LOOT_MONEY_NOTIFY`), answering our `CMSG_LOOT_MONEY`.
fn loot_money_notify(amount: u32) {
    // Our share of the coin pile — informational; the coin row drops on CLEAR_MONEY and
    // the purse rides the ordinary COINAGE flush. Solo looting rarely sends this.
    debug!("net: loot money {amount}");
}

/// The coin line disappears for every current looter (`SMSG_LOOT_CLEAR_MONEY`).
fn loot_clear_money(loot: &mut LootState) {
    // The coin line disappears for everyone → drop the coin row.
    debug!("net: loot coin line cleared");
    loot.clear_money();
}

/// The loot window closes (`SMSG_LOOT_RELEASE_RESPONSE`), answering our `CMSG_LOOT_RELEASE`.
/// Idempotent — a client-side close already cleared. The latch clear is **guid-matched**: under
/// the corpse-switch race (loot B requested while A was open) the old window's release response
/// must not drop the latch the new request just armed.
///
/// **An item loot unlocks here.** The handler (`0x5ec090`) ends in `0x48f200(cl=0, dl=0)`, whose
/// `48f299` leg calls `UnlockItem 0x495420` when the loot object is an ITEM (`0x48f200` gates
/// the unlock on `dl == 0`). It is the only clear an opened lockbox closed with loot left
/// ever gets: vmangos' `DoLootRelease` destroys the item only once it is fully looted, so its
/// slot never changes and [`PendingItemOps::resolve`] has nothing to see.
fn loot_release_response(
    guid: u64,
    loot: &mut LootState,
    latch: &mut LootLatch,
    unlock: ItemUnlock,
) {
    debug!("net: loot released {guid:#x}");
    loot.clear();
    latch.clear_for(guid);
    unlock.unlock(guid);
}

/// The master-loot candidate list (`SMSG_LOOT_MASTER_LIST`) — who the master looter
/// may hand a row to. It arrives from inside the server's `SendLoot`, so it lands just AHEAD of the
/// `SMSG_LOOT_RESPONSE` it belongs to; `LootState` stages it and the open claims it.
fn loot_master_list(candidates: Vec<u64>, loot: &mut LootState) {
    debug!("net: master-loot candidates: {} eligible", candidates.len());
    loot.set_master_candidates(candidates);
}

/// Is this push **ours**? The outermost gate in `CGGameUI::OnItemPush` (1.12.1 `WoW.exe` 5875,
/// `0x491a60`): `0x491b56`/`0x491b61` compare the packet guid against the active player's and fork,
/// and the self arm is the only one that prints "You receive …" *or* animates a bag button. A group
/// member's push takes the `0x491d9f` arm and prints `LOOT_ITEM` ("%s receives loot: %s.") with
/// *their* name — a line we do not build yet, so a foreign push is dropped rather than mislabelled
/// ours.
///
/// The wire's **`showInChat`** used to be folded in here, which read correctly while the chat line
/// was this packet's only output. It is a *later*, narrower gate — `0x491bf3` (self) / `0x491db1`
/// (other) skip the chat formatter alone, after the `ITEM_PUSH` fire at `0x491be8` — so it now
/// rides into [`LootState::push_receive`] as `PendingReceive::in_chat` and silences the line
/// without touching the animation. vmangos always sends 1, so it is inert against
/// our server either way; it is the client's own gate, kept where the client keeps it.
fn is_our_push(p: &ItemPushResult, self_guid: &SelfGuid) -> bool {
    self_guid.0 == Some(p.player_guid)
}

/// An item landed in our bags — looted or received from an NPC (`SMSG_ITEM_PUSH_RESULT`); drives
/// the "You receive loot: …" chat line (gated by [`prints_receive_line`]) **and** the bag-bar drop
/// animation, which is NOT so gated — hence the whole packet going through, and the
/// self check being the only thing that can stop a push here. The reference's
/// `CGGameUI::OnItemPush 0x491a60` emits both from this one packet: it returns early only on a guid
/// mismatch, and tests `showInChat` further down, after the `ITEM_PUSH` fire.
fn item_push_result(
    p: ItemPushResult,
    self_guid: &SelfGuid,
    loot: &mut LootState,
    tutorials: &mut crate::tutorial::Tutorials,
) {
    // Solo loot never gets a MONEY_NOTIFY from this server (vmangos comments it out;
    // the purse rides the ordinary COINAGE field flush) — so this push line is the
    // one reliable "it landed" signal, surfaced as the "You receive loot/item" line.
    debug!(
        "net: item push {} x{} → bag {} slot {:#x}",
        p.item_entry, p.count, p.bag_slot, p.item_slot
    );
    if !is_our_push(&p, self_guid) {
        return;
    }
    // The item-received handler's tutorial sites (1976): resolved on the tutorial feed.
    tutorials.item_received(p.item_entry, p.bag_slot, p.item_slot);
    loot.push_receive(&p);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    use crate::pending_item_ops::{LockTransitions, PendingItemOps};

    /// An [`ItemUnlock`] over an empty lock set — for the tests whose loot source is no item.
    macro_rules! no_item {
        () => {
            ItemUnlock {
                pending: &mut PendingItemOps::default(),
                lock_cleared: &mut LockTransitions::default(),
            }
        };
    }

    /// Both fish-verdict keys queue as **yellow** (type-1 / `UI_INFO_MESSAGE`) entries — the
    /// `DisplayError` arm `0x49684b` — and both resolve to the exact 1.12
    /// strings in the shipped `GlobalStrings.lua` (the equip-error test's runtime pattern —
    /// a typo'd key would silently swallow the toast). Skips without client data.
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

    /// **The refusal table is the reference's, by message id.** Each key must be the catalog row
    /// whose id the binary's own jump table pushes to `DisplayError` (`0x5ebc64`: `4→0x82 ·
    /// 5→0x84 · 6→0x81 · 7→0x83 · 8→0x85 · 9→0x86 · 10→0x19b · 11→0x1bf · 12→0x1cd · 13→0x1ce ·
    /// 14→0x1cf`, default `0x83`).
    ///
    /// Asserting the **id** rather than the spelling is the point: the key text is only a name for
    /// a number the client pushes, and the catalog is generated from that same table (decision
    /// 1770). A key that reads plausibly but names the wrong row cannot survive this.
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
            // The default arm: code 0 is `DIDNT_KILL` itself, 15 `ALREADY_PICKPOCKETED` and 16
            // `NOT_WHILE_SHAPESHIFTED` have no string of their own, and 1/2/3 are server-enum gaps.
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
            // Every one of these is a red `UIErrorsFrame` line, never a chat line — the record's
            // `+0x04`, which is what benilla threw away by composing its own sentence.
            assert_eq!(
                record.kind,
                benilla_ui::messages::MsgKind::Error,
                "{key} is not a red line"
            );
        }

        // **The four commonest refusals are SPOKEN**, in the player's own race and gender — their
        // `type_tag` (`+0x0c`) is a `VocalUIEnum` line id rather than the `0x44` "play the cue"
        // sentinel. A hardcoded sentence has no record, so it had no voice either; this is the
        // third thing the key route gets back, after the wording and the surface.
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
        // …and the rest are cue-tagged, so the split is asserted from both sides.
        for key in ["ERR_LOOT_NOTSTANDING", "ERR_LOOT_MASTER_OTHER"] {
            assert!(!voiced(key), "{key} unexpectedly carries a voice line");
        }
    }

    /// **The regression for the invented English.** Every refusal key must resolve in the shipped
    /// 1.12 `GlobalStrings.lua` to the sentence the real client shows. benilla used to compose
    /// these itself and six of the eight said something 1.12 never says — "You are too far away to
    /// loot that." for a string that ends "that corpse.", "You can't loot that from there." for
    /// "You must be facing the corpse to loot it.", and a wholly invented "Those pockets are
    /// already empty." for a code that has no string at all. Skips without client data.
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
            // The default arm, including the two vmangos codes 1.12 has no sentence for.
            (0, "You don't have permission to loot that corpse."),
            (15, "You don't have permission to loot that corpse."),
            (16, "You don't have permission to loot that corpse."),
        ] {
            assert_eq!(g(loot_refusal(code).key), want, "wire code {code}");
        }
    }

    /// **The four refusals that arrive at an already-open window do not end the kneel.** The
    /// reference's arms for 10 and the master trio jump *past* the release tail at `0x5ebac2`; the
    /// rest fall into it. A master looter refused a hand-off keeps kneeling at the open corpse.
    #[test]
    fn only_the_window_less_refusals_drop_the_kneel() {
        // The named releasing arms, plus the default — `ja 0x5ebabd` lands *on* code 7's arm and
        // falls through the tail, and the compare is unsigned, so 0..3 and 15..255 all release.
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

    /// The **error leg's** clear stays guid-matched, and this is not the deviation 0515 took it
    /// for: it is behaviourally *identical* to the reference.
    ///
    /// The tail's own store is unconditional, but the tail cannot be reached on a mismatch.
    /// `0x5eb9f1` has exactly one predecessor image-wide — `0x5eb93e je`, which already required
    /// latch ≠ 0 **and** both GUID dwords equal — and it has no fall-through predecessor
    /// (`0x5eb9ee` is `ret 8`); the other predecessors of `0x5eb9f4` all require the loot type in
    /// `{2,3,4}` and so leave at `0x5eb9f8 jne`. A mismatched error never gets here at all.
    ///
    /// Read this beside [`a_refusal_drops_whatever_latch_was_live_not_just_a_matching_one`], which
    /// asserts the opposite for a **different arm**: the admission gate `0x5eb963`/`0x5eb9d2`,
    /// which *is* reached on a mismatch and does clear unconditionally. Not a contradiction — a
    /// distinction the handler draws, and the reason the mismatch case is a separate open gap
    /// (see [`loot_error`]).
    #[test]
    fn the_error_legs_clear_is_guid_matched() {
        let mut errors = UiErrorKeys::default();
        let mut latch = LootLatch(Some(CHEST));
        loot_error(CORPSE, 8, &mut errors, &mut latch, no_item!());
        assert_eq!(latch.0, Some(CHEST));
    }

    /// A GameObject guid (HIGHGUID_GAMEOBJECT in the high dword) — a chest.
    const CHEST: u64 = 0xF110_0000_0000_1234;
    /// A creature guid — a corpse.
    const CORPSE: u64 = 0xF130_0000_0000_00AB;

    /// A `NetCommands` plus its receiver, so a test can read what the handler actually sent.
    fn net() -> (
        crate::net::NetCommands,
        crossbeam_channel::Receiver<ClientCommand>,
    ) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (crate::net::NetCommands(tx), rx)
    }

    /// **B84 / decisions 1471 + 1477.** A chest never sends `CMSG_LOOT`, so the corpse branch's
    /// arm-at-the-send never fires for it. `SMSG_SPELL_GO` is its real arm; the response's
    /// own arm is the second half, and on a **cold** latch it admits only the server-started
    /// types 2/3/4 (`0x5eb94b`–`0x5eb95b`). A chest's answer is wire type 2, so it opens and arms.
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

        // The ordinary close path still ends it — the latch is guid-matched, and a chest guid is
        // no different from a corpse one there.
        loot_release_response(CHEST, &mut loot, &mut latch, no_item!());
        assert_eq!(latch.0, None, "the release ends the session");
    }

    /// The corpse path is untouched: the send already armed the latch with the same guid, so the
    /// response takes the **GUID-match** branch (`0x5eb93e`) and its re-arm is a no-op.
    #[test]
    fn a_matching_latch_admits_any_loot_type() {
        let (net, _rx) = net();
        let mut loot = LootState::default();
        let mut latch = LootLatch(Some(CORPSE)); // the `CMSG_LOOT` send, client-predicted
        loot_response(CORPSE, 1, 0, Vec::new(), &mut loot, &mut latch, &net);
        assert_eq!(latch.0, Some(CORPSE));
        assert_eq!(loot.source(), Some(CORPSE));
    }

    /// **The refusal arm (`0x5eb963`).** `loot_type == 1` means "you asked for
    /// this" — and a cold latch says we did not. The real client opens no window, bounces a
    /// `CMSG_LOOT_RELEASE` for the *packet's* guid, and clears. 1471 shipped an unconditional
    /// arm, which is exactly this case's divergence.
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

    /// The refuse arm's clear is **not** guid-matched (`0x5eb9d2`): an unsolicited response for B
    /// drops a live latch on A, and releases B. Faithfully odd, and byte-verified.
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

    /// A lockbox's item guid (HIGHGUID_ITEM is 0x4000 in the high word).
    const LOCKBOX: u64 = 0x4000_0000_0000_0007;

    /// A World with what the release / error handlers write, and the lockbox's open-item lock
    /// already armed at bag 0 slot 3 — `ui_items::drain`'s `CMSG_OPEN_ITEM` arm.
    fn opened_lockbox_world() -> World {
        let mut world = World::new();
        world.init_resource::<LootState>();
        world.insert_resource(LootLatch(Some(LOCKBOX)));
        world.init_resource::<UiErrorKeys>();
        world.init_resource::<LockTransitions>();
        let mut pending = PendingItemOps::default();
        pending.add([(0, 3, LOCKBOX, 1)]);
        world.insert_resource(pending);
        world
    }

    /// **The release unlocks an opened lockbox.** `SMSG_LOOT_RELEASE_RESPONSE` → `0x5ec090` →
    /// `0x48f200(cl=0, dl=0)`, whose `48f299` leg calls `UnlockItem 0x495420` when the loot
    /// object is an ITEM (`0x48f200` gates the unlock on `dl == 0`). Closing a lockbox's window
    /// with loot left destroys nothing server-side (vmangos `DoLootRelease` destroys only a fully
    /// looted item), so no field update ever resolves the lock — the release is the clear.
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

    /// The control: a release naming some other object (a corpse closing) leaves the lockbox's
    /// lock exactly where it was.
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

    /// The error leg's release tail `0x5ebac2` calls the same `UnlockItem 0x495420` on the
    /// packet's guid — on the arms that release, and only those.
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

    /// `OnItemPush`'s outermost gate: the push must be **ours** to produce anything at all. A party
    /// member's drop must NOT print as "You receive loot" — the real client takes its other-player
    /// arm and names them instead.
    #[test]
    fn only_our_own_pushes_reach_the_receive_queue() {
        let me = SelfGuid(Some(ME));
        assert!(is_our_push(&push(ME, true), &me));
        assert!(!is_our_push(&push(THEM, true), &me));
        // Before login lands a guid there is no active player to match against.
        assert!(!is_our_push(&push(ME, true), &SelfGuid(None)));
        // `showInChat` is NOT this gate: a silent push still queues, and still
        // animates — it only loses its chat line, downstream in `drain_receives`.
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
