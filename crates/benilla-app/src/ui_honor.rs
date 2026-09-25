//! The honor feed: our own honor descriptor fields become the snapshot the Honor tabs read, and
//! the inspect-honor reply is matched to the inspected player. The honor block streams only to
//! its owner, so another player's numbers come only from `MSG_INSPECT_HONOR_STATS`, which vmangos
//! refuses by not answering (`MiscHandler.cpp:962-972`); the stock pane re-asks from its `OnShow`
//! while it holds nothing, the only retry.
//!
//! The reference fires the pane's events from three field watches (`0x467e70`, callback
//! `0x5de4b0`): `PLAYER_FIELD_SESSION_KILLS` fires `PLAYER_PVP_KILLS_CHANGED`, and
//! `PLAYER_BYTES_3` and `PLAYER_FIELD_BYTES2` byte 0 fire `PLAYER_PVP_RANK_CHANGED`. Nothing
//! watches the weekly, lifetime or highest-rank fields, which `HonorFrame.lua:10-11` repaints only
//! on `PLAYER_ENTERING_WORLD`.
//!
//! Deviation: the reference watches the whole `PLAYER_BYTES_3` dword, so a drunkenness change also
//! fires `PLAYER_PVP_RANK_CHANGED`; we watch byte 3 alone, because that repaint is identical.

use bevy::prelude::*;

use benilla_ui::script::{HonorState, InspectHonorData, ScriptValue, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_script::{UiFeed, VmMemo};

/// The inspect-honor reply held: one slot, like the reference's `HasInspectHonorData` latch.
#[derive(Resource, Default)]
pub(crate) struct InspectHonor(pub(crate) Option<benilla_protocol::messages::InspectHonorStats>);

/// What the last push told this VM, keyed on the VM so a `/reload`'s fresh VM is told again.
#[derive(Resource, Default)]
struct HonorFeedState {
    vm: VmMemo<HonorFeedMemo>,
}

/// The per-VM change bases: the last snapshot and the held reply's guid.
#[derive(Default)]
struct HonorFeedMemo {
    last: Option<HonorState>,
    last_inspect: Option<u64>,
}

/// The inspect-honor reply's handler; the honor award, `SMSG_PVP_CREDIT`, is the chat family's.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::InspectHonor;
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        app.net_handler(SessionEventKind::InspectHonorStats, on_inspect_stats);
    }

    /// A reply replaces whatever is held, whoever it is for: the reference's latch is one slot.
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

/// Read the honor block off our own descriptor, `None` until it streams. That decides only when
/// the first push and its events happen: the reference reads a field it was never sent as 0
/// (`0x51a4b0`-`0x51a7c0`), and so do our bindings.
fn honor_snapshot(store: &ObjectStore) -> Option<HonorState> {
    let f = &store.0;
    let session = f.player_session_kills();
    let yesterday = f.player_yesterday_kills();
    let last_week = f.player_last_week_kills();
    let this_week = f.player_this_week_kills();
    // Presence is tested on a private field: the public rank byte streams with the unit block.
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
        // The highest lifetime rank, private `PLAYER_FIELD_BYTES` byte 3.
        highest_rank: f.player_honor_rank().unwrap_or(0),
        // The current rank, public `PLAYER_BYTES_3` byte 3: another field, equal to the highest
        // until the player ranks down, so a swap would pass unnoticed.
        rank: f.player_pvp_rank().unwrap_or(0),
        rank_bar: f.player_honor_rank_bar().unwrap_or(0),
    })
}

/// `(kills changed, rank changed)` between two snapshots, from the three watched fields only; a
/// first push fires both. The watched fields are listed so that a new field fires nothing.
fn events_for(before: Option<&HonorState>, after: &HonorState) -> (bool, bool) {
    let Some(b) = before else {
        return (true, true);
    };
    // `PLAYER_FIELD_SESSION_KILLS`, both halves.
    let kills = |h: &HonorState| (h.session_hk, h.session_dk);
    // `PLAYER_BYTES_3` byte 3 and `PLAYER_FIELD_BYTES2` byte 0; not `highest_rank`, unwatched.
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
    // The reference's latch clears only at `0x4c6f70`, from a `NotifyInspect` naming another
    // player, and on `ClearInspectPlayer`. Deviation: we clear it whenever the inspected token's
    // guid moves, because our window re-resolves its token each frame and both tabs read it, so
    // the reference's rule would let the two tabs show different players. `ClearInspectPlayer`
    // drops the token, which clears the slot here too.
    let inspected = inspect_target
        .token
        .as_deref()
        .and_then(|t| crate::ui_unit::player_token_guid(t, &selection, &group));
    if inspect_honor
        .0
        .as_ref()
        .is_some_and(|reply| Some(reply.player_guid) != inspected)
    {
        // Dropped from the resource, so `HasInspectHonorData` and this store agree.
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
        // Fired on a clear too, so the pane stops showing the previous player's numbers.
        script.fire_event("INSPECT_HONOR_UPDATE", Vec::<ScriptValue>::new());
    }

    // --- the pane's request ------------------------------------------------------------------
    // `RequestInspectHonorData()` takes no argument: it asks about the inspected token's guid now,
    // so a re-target before the Honor tab opens asks about the new player.
    let requests = script.take_inspect_honor_requests();
    if requests > 0 {
        match inspected {
            Some(guid) => {
                debug!("honor: inspect-honor request -> {guid:#x}");
                let _ = commands
                    .0
                    .send(ClientCommand::InspectHonorStats { target: guid });
            }
            // Nothing to ask about; the pane asks again from its next `OnShow`.
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

    #[test]
    fn each_event_fires_only_for_its_own_half() {
        let base = state();

        let mut killed = base;
        killed.session_hk += 1;
        assert_eq!(events_for(Some(&base), &killed), (true, false));

        let mut ranked = base;
        ranked.rank = 9;
        assert_eq!(events_for(Some(&base), &ranked), (false, true));

        // The rank bar is `PLAYER_FIELD_BYTES2` byte 0, a rank event.
        let mut barred = base;
        barred.rank_bar = 200;
        assert_eq!(events_for(Some(&base), &barred), (false, true));

        // Unwatched in the reference (`0x467e70`), so these fire nothing.
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

    /// Otherwise a pane whose numbers arrived before it existed would never paint.
    #[test]
    fn the_first_push_fires_both() {
        assert_eq!(events_for(None, &state()), (true, true));
    }
}
