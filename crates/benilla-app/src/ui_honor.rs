//! The app-side **honor feed** (decision 1512) — the local player's PRIVATE honor descriptor
//! fields turned into the snapshot both Honor tabs read, the inspect-honor request/reply round
//! trip, and the `SMSG_PVP_CREDIT` award turned into its chat line and floating number.
//!
//! The display law lives in `benilla-ui`'s `script::pvp`; this is the **data** law it consumes,
//! and there are three pieces of it.
//!
//! ## 1 · The self snapshot is one descriptor read, and every field of it is PRIVATE
//!
//! The whole honor block (`PLAYER_FIELD_SESSION_KILLS` … `PLAYER_FIELD_BYTES2`) streams for
//! nobody but the local player, so there is no per-unit form of this feed and there cannot be one.
//! What a *foreign* player exposes is exactly two things — the PUBLIC current-rank byte, which
//! rides [`crate::ui_unit`]'s snapshot, and whatever `MSG_INSPECT_HONOR_STATS` chooses to answer.
//! That asymmetry is the shape of the entire arc.
//!
//! ## 2 · The two events are a field diff, and the reference watches **exactly three fields**
//!
//! The pane repaints on `PLAYER_PVP_KILLS_CHANGED` and `PLAYER_PVP_RANK_CHANGED`, which the real
//! engine fires from field watches. In the watch table (`0x467e70`, callback
//! `0x5de4b0` carrying the event id) there are **three registrations and no more**:
//!
//! | watched | fires |
//! |---|---|
//! | `PLAYER_FIELD_SESSION_KILLS` | `PLAYER_PVP_KILLS_CHANGED` (523) |
//! | `PLAYER_BYTES_3` | `PLAYER_PVP_RANK_CHANGED` (524) |
//! | `PLAYER_FIELD_BYTES2` byte 0 | `PLAYER_PVP_RANK_CHANGED` (524) |
//!
//! **Nothing watches the yesterday / this-week / last-week / lifetime / contribution fields, and
//! nothing watches `+0x102b` (the highest-lifetime rank).** That is not an omission in the client;
//! it is *why* `HonorFrame.lua` refreshes those rows on `PLAYER_ENTERING_WORLD` alone. The two
//! facts are one fact, and a feed that fired `KILLS_CHANGED` on a lifetime total — which this one
//! once did — would be firing an event the real client has no source for.
//!
//! One divergence, stated: the reference watches the **whole `PLAYER_BYTES_3` dword**, so a
//! drunkenness change fires `PLAYER_PVP_RANK_CHANGED` there too. We watch byte 3 alone. The
//! repaint that spurious fire produces is byte-identical to the one before it, so the difference
//! is unobservable; carrying the other three bytes in an honor snapshot to reproduce it would cost
//! a field nothing reads.
//!
//! ## 3 · The inspect round trip is a real one, unlike `CMSG_INSPECT`
//!
//! 0631's inspect request is fire-and-forget: the gear was already streamed and `SMSG_INSPECT`
//! echoes the guid. This one is the opposite — `MSG_INSPECT_HONOR_STATS` is the *only* source of
//! another player's honor numbers, the reply rides the **same opcode** as the request, and the
//! reference's pane gates on `HasInspectHonorData()` precisely because the data may not be here
//! yet. So: the pane asks, we resolve the inspect target's guid and send, the reply lands in
//! [`InspectHonor`], and `INSPECT_HONOR_UPDATE` tells the pane to repaint.
//!
//! **A refusal is silent by construction.** The server answers nothing when the target is gone,
//! out of the 10-yard `INSPECT_DISTANCE`, or attackable (`MiscHandler.cpp:962-977`) — there is no
//! error shape on this opcode. The pane's own `OnShow` re-asks whenever it holds nothing, which is
//! the reference's retry and the only one there is.

use bevy::prelude::*;

use benilla_ui::script::{HonorState, InspectHonorData, ScriptValue, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::{UiFeed, VmMemo};

/// The inspect-honor reply we currently hold, or `None` before one lands.
///
/// Keyed by nothing but its own `player_guid`: the reply carries whose it is, and the pane is
/// keyed by a unit token, so the app is where the two are matched. One slot rather than a map —
/// the reference's `HasInspectHonorData` is a single latch, and a window that can only inspect one
/// player at a time needs no more.
#[derive(Resource, Default)]
pub(crate) struct InspectHonor(pub(crate) Option<benilla_protocol::messages::InspectHonorStats>);

/// What the last push told this VM, so the feed pushes and fires only on real change.
///
/// Behind a [`VmMemo`] (1290/1291) for the reason every other feed's is: a `/reload` replaces the
/// VM without despawning the world, and a memory of what the *old* VM was told would leave the new
/// one with an empty pane and no event ever coming.
#[derive(Resource, Default)]
struct HonorFeedState {
    vm: VmMemo<HonorFeedMemo>,
}

/// The per-VM change bases — the self snapshot and the inspect reply's guid.
#[derive(Default)]
struct HonorFeedMemo {
    last: Option<HonorState>,
    last_inspect: Option<u64>,
}

/// The honor pane's packet handler (decision 1512; in the net handler table since 2313). The
/// honor arc's other inbound message, the award (`PvpCredit`), is a chat line and a floating
/// number and stays with the chat family.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::InspectHonor;
    use crate::net::NetHandlerApp;

    /// Register the handler — called from [`super::UiHonorPlugin`].
    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::InspectHonorStats, on_inspect_stats);
    }

    /// The inspect reply REPLACES whatever is held, including for a different player: the
    /// reference's latch is a single slot, and a pane still showing the last target's kills is
    /// the failure keeping the old one produces.
    fn on_inspect_stats(In(ev): In<SessionEvent>, mut inspect: ResMut<InspectHonor>) {
        if let SessionEvent::InspectHonorStats(stats) = ev {
            inspect.0 = Some(stats);
        }
    }
}

pub(crate) struct UiHonorPlugin;

impl Plugin for UiHonorPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<InspectHonor>()
            .init_resource::<HonorFeedState>()
            .add_systems(Update, feed_honor.in_set(UiFeed));
    }
}

/// Read the honor block off our own descriptor, or `None` while none of it has streamed.
///
/// **`None` and all-zeroes are the same thing on screen, and that is the point of the gate, not a
/// flaw in it.** This doc used to say the reference "paints a player whose fields have not arrived
/// as blank, not as zero". It does not: `0x51a4b0`–`0x51a7c0`'s "absent → 0.0" tails are about the
/// absent player OBJECT, and once the object exists every one of those bindings reads a
/// descriptor array that is allocated and zeroed, so a field the server never sent reads `0`.
/// Our bindings answer the same zeros — `honor(lua).unwrap_or_default()` — so what the gate
/// actually decides is only *when the first snapshot is pushed*, and with it when
/// `PLAYER_PVP_KILLS_CHANGED`/`PLAYER_PVP_RANK_CHANGED` first fire. It cannot make a row differ
/// from the reference.
///
/// That is worth stating because this gate was the other candidate cause of report B378, and it
/// is ruled out by exactly this: a missing snapshot and a zeroed one paint the same pane. The
/// cause was the rank title's team digit (decision 2227).
fn honor_snapshot(store: &ObjectStore) -> Option<HonorState> {
    let f = &store.0;
    let session = f.player_session_kills();
    let yesterday = f.player_yesterday_kills();
    let last_week = f.player_last_week_kills();
    let this_week = f.player_this_week_kills();
    // The rank byte is PUBLIC and arrives with the unit block, so it alone is a poor presence
    // test; the honor block proper is what we wait for.
    session?;
    Some(HonorState {
        session_hk: session.map_or(0, |(hk, _)| hk),
        session_dk: session.map_or(0, |(_, dk)| dk),
        yesterday_hk: yesterday.map_or(0, |(hk, _)| hk),
        yesterday_dk: yesterday.map_or(0, |(_, dk)| dk),
        yesterday_honor: f.player_yesterday_contribution().unwrap_or(0),
        this_week_hk: this_week.map_or(0, |(hk, _)| hk),
        this_week_honor: f.player_this_week_contribution().unwrap_or(0),
        last_week_hk: last_week.map_or(0, |(hk, _)| hk),
        last_week_dk: last_week.map_or(0, |(_, dk)| dk),
        last_week_honor: f.player_last_week_contribution().unwrap_or(0),
        last_week_standing: f.player_last_week_rank().unwrap_or(0),
        lifetime_hk: f.player_lifetime_honorable_kills().unwrap_or(0),
        lifetime_dk: f.player_lifetime_dishonorable_kills().unwrap_or(0),
        // The HIGHEST lifetime rank (PRIVATE, `PLAYER_FIELD_BYTES` byte 3) …
        highest_rank: f.player_honor_rank().unwrap_or(0),
        // … and the CURRENT one (PUBLIC, `PLAYER_BYTES_3` byte 3). Two bytes, two fields, and
        // they are equal for anyone who has never ranked down — which is exactly why reading one
        // for the other would have shipped green (decision 1512).
        rank: f.player_pvp_rank().unwrap_or(0),
        rank_bar: f.player_honor_rank_bar().unwrap_or(0),
    })
}

/// Which of the two reference events a change between two snapshots deserves — the module doc's
/// three-row watch table, written out. A first push is both: the pane has never painted, and
/// either event repaints it.
///
/// **Most of `HonorState` fires nothing at all.** The weekly figures, the two lifetime totals and
/// the highest-lifetime rank are unwatched in the real client, so they ride the pane's world-entry
/// repaint and move here in silence. Listing the watched fields positively — rather than
/// "everything that is not a rank byte", which is what this did before — is what
/// keeps a field added later from inventing an event for itself.
fn events_for(before: Option<&HonorState>, after: &HonorState) -> (bool, bool) {
    let Some(b) = before else {
        return (true, true);
    };
    // `PLAYER_FIELD_SESSION_KILLS`, both halves — the one field behind `PLAYER_PVP_KILLS_CHANGED`.
    let kills = |h: &HonorState| (h.session_hk, h.session_dk);
    // `PLAYER_BYTES_3` byte 3 and `PLAYER_FIELD_BYTES2` byte 0 — the two behind
    // `PLAYER_PVP_RANK_CHANGED`. NOT `highest_rank`: that is `+0x102b`, and nothing watches it.
    let ranks = |h: &HonorState| (h.rank, h.rank_bar);
    (kills(b) != kills(after), ranks(b) != ranks(after))
}

/// Push the self snapshot and the inspect reply, fire what moved, and drain the pane's request.
fn feed_honor(
    script: Option<NonSendMut<UiScript>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    inspect_target: Res<crate::ui_inspect::InspectTarget>,
    selection: Res<crate::target::Selection>,
    group: Res<crate::ui_party::GroupState>,
    mut inspect_honor: ResMut<InspectHonor>,
    mut state: ResMut<HonorFeedState>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    let memo = state.vm.get(&script);

    // --- the self snapshot -------------------------------------------------------------------
    if let Ok(store) = self_store.single() {
        if let Some(fresh) = honor_snapshot(store) {
            if memo.last.as_ref() != Some(&fresh) {
                let (kills, ranks) = events_for(memo.last.as_ref(), &fresh);
                script.set_honor(Some(fresh));
                memo.last = Some(fresh);
                if kills {
                    script.fire_event("PLAYER_PVP_KILLS_CHANGED", vec![]);
                }
                if ranks {
                    script.fire_event("PLAYER_PVP_RANK_CHANGED", vec![]);
                }
            }
        }
    }

    // --- the inspect reply -------------------------------------------------------------------
    //
    // **A reply is only valid while it is about the player currently being inspected.** The
    // reference's `HasInspectHonorData()` is the latch its pane's `OnShow` gates on: hold stale
    // data and inspecting a SECOND player repaints the FIRST one's kills, with no request ever
    // sent and nothing on screen to say so.
    //
    // **The real client's latch is invalidated by exactly one thing, and it is not this one**:
    // the slot is a single un-keyed store, and `0x4c6f70` — reached
    // from `NotifyInspect` — is its only GUID writer *and* its only invalidator. A `NotifyInspect`
    // naming a different player zeroes both flags; the same player is a no-op; there is no timeout;
    // and `ClearInspectPlayer` (the stock `InspectFrame_OnHide`) clears it outright.
    //
    // **We invalidate on the inspected TOKEN's guid moving instead, deliberately.** Our inspect
    // window re-resolves its token every frame so the paper doll follows a re-target (0631), and
    // the honor page reads that same token rather than the reference's hardcoded `"target"` — so
    // matching the reference's latch exactly would let one window show two different players'
    // data at once, on its two tabs. The reference cannot notice because its two pages disagree
    // about whose player they show in the first place. Ours agree, and this comparison is what
    // keeps them agreeing.
    //
    // (`ClearInspectPlayer` is covered too, one level up: it drops the token, `inspected` reads
    // `None`, and the mismatch below clears the slot.)
    let inspected = inspect_target
        .token
        .as_deref()
        .and_then(|t| crate::ui_unit::player_token_guid(t, &selection, &group));
    if inspect_honor
        .0
        .as_ref()
        .is_some_and(|reply| Some(reply.player_guid) != inspected)
    {
        // Dropped from the RESOURCE, not merely filtered on the way out: a reply nobody may read
        // is not data we are keeping, and leaving it here would make `HasInspectHonorData` and
        // this store disagree about what is held.
        inspect_honor.0 = None;
    }
    let held = inspect_honor.0.as_ref().map(|r| r.player_guid);
    if memo.last_inspect != held {
        script.set_inspect_honor(inspect_honor.0.as_ref().map(|r| {
            let (session_hk, session_dk) = r.session_kills();
            InspectHonorData {
                guid: r.player_guid,
                session_hk,
                session_dk,
                yesterday_hk: r.yesterday_hk,
                yesterday_honor: r.yesterday_honor,
                this_week_hk: r.this_week_hk,
                this_week_honor: r.this_week_honor,
                last_week_hk: r.last_week_hk,
                last_week_honor: r.last_week_honor,
                last_week_standing: r.last_week_rank,
                lifetime_hk: r.lifetime_hk,
                lifetime_dk: r.lifetime_dhk,
                highest_rank: r.highest_rank,
                rank_bar: r.rank_bar,
            }
        }));
        memo.last_inspect = held;
        // Fired on a *clear* as well as on an arrival: the pane's handler re-reads
        // `GetInspectHonorData`, and a window left showing the previous player's kills is the
        // failure a silent clear produces.
        script.fire_event("INSPECT_HONOR_UPDATE", Vec::<ScriptValue>::new());
    }

    // --- the pane's request ------------------------------------------------------------------
    //
    // `RequestInspectHonorData()` takes no argument — the engine holds the inspected player, so
    // the app resolves it, exactly as `NotifyInspect`'s token is resolved. The token is re-read
    // now rather than remembered from the notify, so a re-target between opening the window and
    // opening its Honor tab asks about whoever is actually being inspected.
    let requests = script.take_inspect_honor_requests();
    if requests > 0 {
        match inspected {
            Some(guid) => {
                debug!("honor: inspect-honor request -> {guid:#x}");
                let _ = commands
                    .0
                    .send(ClientCommand::InspectHonorStats { target: guid });
            }
            // No inspect target (or it is not streamed): nothing to ask about. The pane will ask
            // again from its next `OnShow`, which is the reference's only retry.
            None => debug!("honor: inspect-honor request with no resolvable target — not sent"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> HonorState {
        HonorState {
            session_hk: 3,
            lifetime_hk: 900,
            rank: 8,
            highest_rank: 9,
            rank_bar: 128,
            ..Default::default()
        }
    }

    /// The event split is the whole point of [`events_for`]: a kill must not claim a rank change,
    /// and a rank must not claim a kill. An addon that registers only one of the two is what makes
    /// the distinction observable.
    #[test]
    fn each_event_fires_only_for_its_own_half() {
        let base = state();

        let mut killed = base;
        killed.session_hk += 1;
        assert_eq!(events_for(Some(&base), &killed), (true, false));

        let mut ranked = base;
        ranked.rank = 9;
        assert_eq!(events_for(Some(&base), &ranked), (false, true));

        // The bar moving is a rank event too — it is the rank's own progress, and the reference's
        // pane redraws the bar from the same handler as the title.
        let mut barred = base;
        barred.rank_bar = 200;
        assert_eq!(events_for(Some(&base), &barred), (false, true));

        // The unwatched half: a weekly figure, a lifetime total and the highest-lifetime rank all
        // move in silence, because the real client registers no watch on any of them (`0x467e70` —
        // the module doc's table). This is the assertion the earlier implementation failed: it
        // fired `KILLS_CHANGED` for all three.
        for mutate in [
            (|h: &mut HonorState| h.last_week_standing = 42) as fn(&mut HonorState),
            |h: &mut HonorState| h.lifetime_hk = 5_000,
            |h: &mut HonorState| h.yesterday_honor = 900,
            |h: &mut HonorState| h.highest_rank = 18,
        ] {
            let mut quiet = base;
            mutate(&mut quiet);
            assert_eq!(
                events_for(Some(&base), &quiet),
                (false, false),
                "an unwatched field must fire nothing"
            );
        }
    }

    /// A first push has to fire both, or a pane whose numbers arrived before it existed never
    /// paints — the `ui_unit` "first resolve counts as a transition" rule.
    #[test]
    fn the_first_push_fires_both() {
        assert_eq!(events_for(None, &state()), (true, true));
    }
}
