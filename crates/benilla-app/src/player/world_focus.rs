//! **The game's half of 1160's wire (a) and the settle release** — what the world is told, and
//! what the game does with what the world publishes back.
//!
//! The terrain streamer used to read `player::Player` directly (where the avatar is, whether it is
//! settled) and write it back (lifting the post-snap hold when the destination's colliders
//! arrived). Both directions are the engine reaching across the line: a world renderer cannot
//! depend on a game's avatar type, and `benilla-worldview` proved the point by having to stub one.
//!
//! Inverted, it is two systems and no shared type:
//!
//! - [`publish_view_focus`] answers the world's one question — *where should I stream from* —
//!   ahead of the stream stage. The viewer with no avatar (a capture run, the world viewer)
//!   answers `ViewFocus::camera()` and needs nothing else.
//! - [`release_post_snap_hold`] reads the residency the world publishes and decides when the
//!   mover's hold ends. The *decision* is the game's; the *fact* is the world's. That split is why
//!   decision 0737's law survives the move intact — the hold still ends on residency and never on
//!   ground contact, and every mover mode still releases the same way, because there is still
//!   exactly one place that does it.

use bevy::prelude::*;

use super::{Player, SETTLE_TIMEOUT};
use benilla_world::terrain_stream::{ViewFocus, WorldLoadProgress};
use benilla_world::view::Viewer;

/// Tell the world where the avatar's body is, in the one shape the three lanes that ask actually
/// want (see [`Viewer`]). Same frame position and same gate as the focus below.
pub(super) fn publish_viewer(
    mut viewer: ResMut<Viewer>,
    player: Option<Res<Player>>,
    rig: Option<Res<super::CameraControl>>,
    screen: Option<Res<crate::loading_screen::LoadingScreen>>,
    store: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
) {
    // The viewer's *condition*, off **our own character's** descriptor block: both are whole-screen
    // effects, which is why they ride here rather than on any body in the scene — and why they read
    // `SelfPlayer` rather than the body we drive. Being drunk is a fact about you; possessing a boar
    // does not sober you up, and a boar has no drunk byte to read.
    let (drunk, ghost) = match store.single() {
        Ok(s) => (
            s.0.player_drunk_byte()
                .map_or(0.0, |b| f32::from(b.min(100)) / 100.0),
            s.0.player_is_ghost(),
        ),
        Err(_) => (0.0, false),
    };
    // The ghost A/B override (`WOW_GHOST_PROBE`) lands here rather than beside the screen pass it
    // used to pin alone: this flag is what the death light and the DeathClouds sky both read, so
    // overriding it is what makes the probe mean "the ghost world" rather than "the ghost filter".
    let ghost = crate::death::ghost_probe().unwrap_or(ghost);
    let body = match player.as_deref() {
        Some(p) if p.active && !p.detached => Viewer {
            at: Some(p.pos),
            move_flags: p.move_flags(),
            planar_speed: p.planar_speed(),
            height: p.collision_height.0,
            ..Viewer::default()
        },
        _ => Viewer::default(),
    };
    *viewer = Viewer {
        drunk,
        ghost,
        // The bare zoom feather. The mesh-side writer folds the aura factor separately, so this
        // must stay the zoom alone or the self body double-applies it.
        self_fade: rig.as_deref().map_or(1.0, super::CameraControl::self_fade),
        world_covered: screen.as_deref().is_some_and(|s| s.covering()),
        ..body
    };
}

/// Tell the world where to stream from, once per frame, before the stream stage reads it — and,
/// when that answer is not the body, put the body on the settle hold for as long as it is not.
pub(super) fn publish_view_focus(
    mut focus: ResMut<ViewFocus>,
    mut player: Option<ResMut<Player>>,
    roster: Option<Res<crate::char_select::Roster>>,
    cinematic: Option<Res<crate::cinematic::Cinematic>>,
    mut was_flying: Local<bool>,
) {
    let entry = roster
        .as_deref()
        .and_then(crate::char_select::Roster::pending_entry);
    // A cinematic flies the eye away from the body — a Tauren's intro opens 1741 yards out — so
    // the stream has to follow the *camera* for its duration or the shot crosses unstreamed
    // terrain. That is what `detached` already means here (the eye is off the body; the zone
    // authority stays on it), so a fly-by borrows free-fly's answer rather than inventing a third
    // mode. The server does the mirror-image thing on its side: while a cinematic runs it
    // re-anchors object visibility to its own copy of the flying camera.
    let flying = cinematic.as_deref().is_some_and(|c| c.is_playing());
    // **A detached focus stops keeping the body's own ground resident — so the body goes on the
    // hold for as long as that lasts.** Free-fly never had to answer for this: `control` skips the
    // whole controlled branch while `detached`, so nothing simulates the body there. A cinematic
    // does, and the body stands with gravity on while the tiles under it unload. Measured on the
    // probe: `.debug play cinematic 41` over a settled body at z=59.4 dropped it to z=-62 within
    // four seconds, the server yanking it back, and it never sent a movement packet again.
    //
    // This is not a new fact — it is the settle hold's own ("the ground under this body is not
    // there"), reached from the other side: after a teleport the body's world has not arrived
    // *yet*, and here it is *leaving*. So it takes the same hold, and the release stays exactly
    // where decision 0737 put it — [`release_post_snap_hold`], on residency about the body's own
    // tile, which cannot become true again until the shot ends and the focus comes home. Nothing
    // here ever clears the hold: starting one is the game's business, ending one is the world's.
    //
    // `world_stale` rides with it because it is equally true — the resident world is the flying
    // camera's, not this body's — and because it is what keeps the stall backstop off. Without it
    // a camera that settles mid-shot goes quiet, the 6 s no-progress budget expires, and gravity
    // comes back on partway through a 102-second intro: the exact fall this prevents, delayed.
    // Armed on the RISING EDGE, never re-asserted per frame — the sentence above is a rule, not a
    // flourish. A shot whose camera happens to stay on the body's own tile makes that tile resident
    // and the release ends the hold at once, correctly; re-arming every frame would fight it and
    // leave `settling` oscillating under the loading screen's clear-gate and the zone-channel walk,
    // both of which read it.
    if flying && !*was_flying {
        if let Some(p) = player.as_deref_mut().filter(|p| p.active) {
            p.settling = true;
            p.world_stale = true;
            info!("cinematic: body held — the stream follows the camera off its ground");
        }
    }
    *was_flying = flying;
    // The pacing bit: spawn caps apply only to a live avatar standing in a settled world. Through
    // entry, a teleport and a world swap the loading cover is absorbing the burst, and a cap there
    // would only lengthen the reveal.
    *focus = match player.as_deref() {
        Some(p) if p.active => {
            let wow = benilla_assets::coords::bevy_to_wow(p.pos);
            let paced = !p.settling && !p.world_stale;
            if p.detached || flying {
                ViewFocus::detached(wow, paced)
            } else {
                ViewFocus::body(wow, paced)
            }
        }
        // No avatar: the picked character's row for the entry window, else
        // whatever the camera can see.
        _ => match entry {
            Some((map, pos)) => ViewFocus::entry(map, pos),
            None => ViewFocus::camera(),
        },
    };
}

/// End the post-snap hold when the destination's world has arrived — decision 0737, reading the
/// streamer's published residency instead of being written by it.
///
/// Runs after the stream stage so `colliders_pending` is this frame's count, which is the same
/// freshness `finish_colliders` heading the streaming chain was always there to give it.
pub(super) fn release_post_snap_hold(
    mut player: ResMut<Player>,
    progress: Option<Res<WorldLoadProgress>>,
    time: Res<Time>,
    mut last_counters: Local<Option<[usize; 6]>>,
    net_cmds: Option<Res<crate::net::NetCommands>>,
) {
    let Some(p) = progress else { return };
    // **The facts must be about the ground under our own feet** (B263 round 3).
    // `WorldLoadProgress` names the tile it describes; a mismatch means the streamer's focus and
    // the avatar diverged — the stale-focus snap frame this guard exists for, or a detached
    // free-fly eye — and residency published for another tile must never unfreeze this body. On a
    // mismatch the resident release and the stale-clear below are both refused; the stall backstop
    // still runs, so a genuinely wedged mismatch costs a logged 6 s timeout, never a silent fall.
    let focus_matches = p.focus_tile.is_some_and(|t| {
        let wow = benilla_assets::coords::bevy_to_wow(player.pos);
        let (tx, ty) = benilla_formats::world_to_tile(wow[0], wow[1]);
        t == (tx as i32, ty as i32)
    });
    // The streamer is the only authority on *which map* the colliders under the avatar belong to.
    // Residency here means this map's own tile (or, on a WMO-only map, its one building) is
    // spawned — reachable only after a swap has drained every tile of the map we left.
    if p.focus_resident && p.total > 0 && focus_matches {
        player.world_stale = false;
    }
    // Did the stream move since last frame? Any counter changing — a tile spawned, a placement
    // up, a collider queued or attached, a retained region baked — is the destination still
    // arriving. Tracked every frame
    // (not just while settling) so the first settling frame compares against a real baseline.
    let counters = [
        p.ready,
        p.total,
        p.colliders_pending,
        p.merge_pending,
        p.placements_pending,
        p.gx_pending,
    ];
    let progressed = last_counters.replace(counters) != Some(counters);
    if !player.settling {
        return;
    }
    // The release ends on scene AND colliders, never on ground contact — feet-on-ground dragged
    // the whole mover-mode matrix into a loading decision, and a flyer or swimmer never touched
    // it. The timeout is a STALL budget, twice over (0710's fail-closed law, extended by B263 /
    // decision 1303): the deadline is pushed while the resident colliders still belong to the map
    // we just left, AND while the destination's own stream is visibly advancing. As a fixed load
    // budget it was 0.01 s from firing on a fast machine (a Stormwind arrival used 5.99 s of the
    // 6.00), and on a slower one it fired mid-stream — gravity on, the city's collider still in
    // the build queue, and the body fell to the canyon under the Valley of Heroes with the cover
    // still up. Only a stream that has made NO progress for the whole budget — missing data, dead
    // IO — can time out now, which is the case the backstop was always for.
    let now = time.elapsed_secs();
    if p.presentable() && !player.world_stale && focus_matches {
        player.end_settle(true, now);
    } else if player.world_stale || progressed {
        player.settle_deadline = now + SETTLE_TIMEOUT;
    } else if now >= player.settle_deadline {
        player.end_settle(false, now);
    }
    // **Pay the worldport ack the moment the hold ends** — on either end, the
    // resident release or the stall timeout (a dead stream must still complete the transfer, or
    // the server holds us out-of-world until logout). This is the real client's post-load `0xDC`,
    // re-expressed: its blocking load's "done" is our release.
    if !player.settling && player.owes_worldport_ack {
        if let Some(net) = net_cmds.as_deref() {
            player.owes_worldport_ack = false;
            let _ = net.0.send(crate::net::ClientCommand::WorldportAck);
            info!("worldport: ack sent at settle release");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// The tile under `Player::default()`'s position — what the streamer would publish as
    /// `focus_tile` when its focus and the avatar agree (the ordinary, correctly-ordered frame).
    fn own_tile() -> (i32, i32) {
        let wow = benilla_assets::coords::bevy_to_wow(Player::default().pos);
        let (tx, ty) = benilla_formats::world_to_tile(wow[0], wow[1]);
        (tx as i32, ty as i32)
    }

    /// A test app with the release system registered (a registered system keeps its `Local`
    /// baseline across frames, which `run_system_once` would reset) and a hand-driven clock.
    /// The published progress names the avatar's own tile — each test then describes residency
    /// facts that are at least *about* the right place (the mismatch test overrides it).
    fn app() -> App {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .insert_resource(WorldLoadProgress {
                focus_tile: Some(own_tile()),
                ..WorldLoadProgress::default()
            })
            .insert_resource(Player {
                settling: true,
                settle_deadline: SETTLE_TIMEOUT,
                ..Player::default()
            })
            .add_systems(Update, release_post_snap_hold);
        app
    }

    /// One frame: advance the clock, mutate the streamer's published progress, run the release.
    fn step(app: &mut App, dt: f32, tweak: impl FnOnce(&mut WorldLoadProgress)) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(dt));
        tweak(&mut app.world_mut().resource_mut::<WorldLoadProgress>());
        app.update();
    }

    fn settling(app: &mut App) -> bool {
        app.world().resource::<Player>().settling
    }

    /// **A cinematic puts the body on the settle hold, once.** The live defect this pins: with the
    /// stream following the flying camera, the tiles under a standing body unload and it falls
    /// through the world — measured on the probe at `.debug play cinematic 41`, z=59.4 to z=-62 in
    /// four seconds, with the server yanking it back and no movement packet sent again.
    ///
    /// `world_stale` rides along because without it the stall backstop expires the moment the
    /// camera stops streaming new ground, and gravity comes back partway through the shot.
    #[test]
    fn a_flying_cinematic_holds_the_body_and_marks_its_world_stale() {
        let mut app = App::new();
        app.insert_resource(ViewFocus::default())
            .insert_resource(Player {
                active: true,
                settling: false,
                world_stale: false,
                ..Player::default()
            })
            .insert_resource(crate::cinematic::Cinematic::playing_for_test())
            .add_systems(Update, publish_view_focus);
        app.update();

        let p = app.world().resource::<Player>();
        assert!(
            p.settling,
            "the body kept gravity while its ground unloaded"
        );
        assert!(
            p.world_stale,
            "the stall backstop was left free to expire mid-shot"
        );
    }

    /// The other half of the same rule: **nothing here ever ends a hold**, and nothing re-arms one.
    /// The release is the world's, and a shot whose camera stays on the body's own
    /// tile makes that tile resident — so the release ends the hold correctly, and a per-frame
    /// re-arm would fight it and leave `settling` flickering under the two systems that read it
    /// (the loading screen's clear gate and the zone-channel walk).
    #[test]
    fn the_hold_is_armed_on_the_edge_and_never_re_armed() {
        let mut app = App::new();
        app.insert_resource(ViewFocus::default())
            .insert_resource(Player {
                active: true,
                ..Player::default()
            })
            .insert_resource(crate::cinematic::Cinematic::playing_for_test())
            .add_systems(Update, publish_view_focus);
        app.update();
        assert!(app.world().resource::<Player>().settling);

        // The world releases the hold (the camera's tile IS this body's), still mid-cinematic.
        app.world_mut().resource_mut::<Player>().settling = false;
        app.world_mut().resource_mut::<Player>().world_stale = false;
        app.update();
        let p = app.world().resource::<Player>();
        assert!(
            !p.settling,
            "the publish re-armed a hold the world had ended"
        );
        assert!(
            !p.world_stale,
            "and re-staled a world the streamer had cleared"
        );
    }

    /// No cinematic, no hold — the ordinary frame is untouched.
    #[test]
    fn an_ordinary_frame_arms_nothing() {
        let mut app = App::new();
        app.insert_resource(ViewFocus::default())
            .insert_resource(Player {
                active: true,
                ..Player::default()
            })
            .insert_resource(crate::cinematic::Cinematic::default())
            .add_systems(Update, publish_view_focus);
        app.update();
        let p = app.world().resource::<Player>();
        assert!(!p.settling);
        assert!(!p.world_stale);
    }

    /// B263: a stream that keeps arriving keeps the hold, however long it takes.
    /// The old fixed load budget released gravity at 6 s into a live Stormwind arrival — measured
    /// 0.01 s from firing even on a fast machine — and the body fell through the not-yet-collided
    /// city to the canyon under the Valley of Heroes, impact heard under the loading screen.
    #[test]
    fn a_slow_but_advancing_stream_never_times_out() {
        let mut app = app();
        app.world_mut().resource_mut::<Player>().world_stale = false;
        // 5× the budget of wall-clock, with some counter moving every frame — a slow machine
        // streaming a big city, five times slower than the budget ever allowed for.
        for i in 0..(5.0 * SETTLE_TIMEOUT) as usize {
            step(&mut app, 1.0, |p| {
                p.total = 2000;
                p.ready = i; // tiles/placements landing one at a time
                p.colliders_pending = 300 + i % 7;
                p.scene_ready = false;
            });
            assert!(
                settling(&mut app),
                "the hold gave up at ~{i}s with the stream still visibly advancing"
            );
        }
    }

    /// The backstop's one remaining target: a stream that makes NO progress for the whole budget
    /// (missing data, dead IO) still releases, so a broken world can never hold the screen forever.
    #[test]
    fn a_genuinely_stalled_stream_still_times_out() {
        let mut app = app();
        app.world_mut().resource_mut::<Player>().world_stale = false;
        // Frozen counters, scene never presentable. First frame baselines the Local (counts as
        // change), then the budget runs undisturbed.
        for _ in 0..=(SETTLE_TIMEOUT + 2.0) as usize {
            step(&mut app, 1.0, |p| {
                p.total = 2000;
                p.ready = 500;
                p.colliders_pending = 300;
                p.scene_ready = false;
            });
        }
        assert!(
            !settling(&mut app),
            "a dead stream must not hold the body (and the screen) forever"
        );
    }

    /// 0710's fail-closed law is untouched: while the resident world is still the departed map's,
    /// frozen counters push the deadline rather than spending it.
    #[test]
    fn a_stale_world_pushes_the_deadline_before_the_stall_budget_starts() {
        let mut app = app();
        app.world_mut().resource_mut::<Player>().world_stale = true;
        // Twice the budget of stale, frozen frames: no release (focus_resident stays false so
        // nothing clears the stale flag).
        for _ in 0..(2.0 * SETTLE_TIMEOUT) as usize {
            step(&mut app, 1.0, |p| {
                p.total = 0;
                p.scene_ready = false;
                p.focus_resident = false;
            });
            assert!(settling(&mut app), "released over the departed map's floor");
        }
        // The destination becomes resident and the stream then stalls: the budget starts HERE.
        app.world_mut().resource_mut::<Player>().world_stale = false;
        for _ in 0..=(SETTLE_TIMEOUT + 2.0) as usize {
            step(&mut app, 1.0, |p| {
                p.total = 2000;
                p.ready = 500;
                p.colliders_pending = 300;
                p.scene_ready = false;
                p.focus_resident = true;
            });
        }
        assert!(
            !settling(&mut app),
            "the stall budget never started counting"
        );
    }

    /// The ordinary end: scene presentable and colliders quiet releases at once — even on the very
    /// frame the last counter moved, so residency is never delayed by its own arrival.
    #[test]
    fn residency_releases_on_the_frame_it_lands() {
        let mut app = app();
        app.world_mut().resource_mut::<Player>().world_stale = false;
        step(&mut app, 1.0, |p| {
            p.total = 2000;
            p.ready = 1999;
            p.colliders_pending = 3;
            p.scene_ready = false;
        });
        assert!(settling(&mut app));
        step(&mut app, 0.1, |p| {
            p.ready = 2000;
            p.colliders_pending = 0;
            p.scene_ready = true;
        });
        assert!(!settling(&mut app), "presentable world, hold still on");
    }

    /// Decision 1498: geometry that has SPAWNED but not yet baked is a hole in the world exactly
    /// like an unattached collider, so it holds the post-snap freeze too. The retained pass took
    /// the static world off the entity path (1429), and from then on "the placement spawned" and
    /// "the building is on screen" stopped being the same fact — the hold, like the reveal, keys
    /// on the second.
    #[test]
    fn an_unbaked_region_holds_the_settle() {
        let mut app = app();
        app.world_mut().resource_mut::<Player>().world_stale = false;
        step(&mut app, 1.0, |p| {
            p.total = 2000;
            p.ready = 2000;
            p.colliders_pending = 0;
            p.scene_ready = true;
            p.gx_pending = 4;
        });
        assert!(settling(&mut app), "spawned but undrawn is not presentable");
        step(&mut app, 0.1, |p| p.gx_pending = 0);
        assert!(!settling(&mut app), "baked — the world is really there");
    }

    /// B263 round 3: residency published for ANOTHER tile never releases the hold
    /// and never clears the stale flag — the live defect was the focus publish racing the teleport
    /// snap, so on the snap frame the streamer described the DEPARTURE city as fully resident and
    /// the hold released into free fall at the destination. The facts now name their tile; facts
    /// about somewhere else are not facts about the ground under this body.
    #[test]
    fn residency_about_another_tile_neither_releases_nor_clears_stale() {
        let mut app = app();
        app.world_mut().resource_mut::<Player>().world_stale = true;
        // A fully-resident, quiet world — but described for a tile the avatar is not standing on
        // (the departure side of a same-map teleport, one frame stale).
        let elsewhere = {
            let (tx, ty) = own_tile();
            Some((tx + 8, ty))
        };
        for _ in 0..3 {
            step(&mut app, 0.05, |p| {
                p.focus_tile = elsewhere;
                p.total = 2000;
                p.ready = 2000;
                p.colliders_pending = 0;
                p.scene_ready = true;
                p.focus_resident = true;
            });
            assert!(settling(&mut app), "released on another tile's residency");
            assert!(
                app.world().resource::<Player>().world_stale,
                "the stale flag cleared on another tile's residency"
            );
        }
        // The same facts, now about the right tile: stale clears and the hold ends at once.
        step(&mut app, 0.05, |p| p.focus_tile = Some(own_tile()));
        assert!(!settling(&mut app), "matching residency must still release");
    }

    /// Decision 1340: the arrival debts — the deferred worldport ack and the near-teleport
    /// position report — are paid on the frame the hold ends, and exactly once. The ack is the
    /// real client's post-load `0xDC`, so it must ride the release, never the snap — and go
    /// out exactly once.
    #[test]
    fn the_resident_release_pays_the_worldport_ack_once() {
        let mut app = app();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        {
            let mut player = app.world_mut().resource_mut::<Player>();
            player.world_stale = false;
            player.owes_worldport_ack = true;
        }
        // Still streaming: settling holds, nothing is sent.
        step(&mut app, 0.1, |p| {
            p.total = 2000;
            p.ready = 1500;
            p.colliders_pending = 5;
            p.scene_ready = false;
        });
        assert!(settling(&mut app));
        assert!(
            rx.try_recv().is_err(),
            "the ack went out before the release"
        );
        // Residency lands: the hold ends and the ack goes out.
        step(&mut app, 0.1, |p| {
            p.ready = 2000;
            p.colliders_pending = 0;
            p.scene_ready = true;
        });
        assert!(!settling(&mut app));
        let sent: Vec<crate::net::ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(sent[..], [crate::net::ClientCommand::WorldportAck]),
            "exactly one worldport ack at the release, got {sent:?}"
        );
        // Paid once: further quiet frames send nothing more.
        step(&mut app, 0.1, |_| {});
        assert!(rx.try_recv().is_err(), "the ack was paid twice");
    }

    /// The stall timeout pays the ack too: a dead stream must still complete the transfer —
    /// vmangos keeps an unacked far teleport out-of-world (dropping every packet) until logout,
    /// so a release with no ack would strand the session, silently.
    #[test]
    fn a_timeout_release_still_pays_the_ack() {
        let mut app = app();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        {
            let mut player = app.world_mut().resource_mut::<Player>();
            player.world_stale = false;
            player.owes_worldport_ack = true;
        }
        // Frozen counters, scene never presentable: first frame baselines, then the budget runs.
        for _ in 0..=(SETTLE_TIMEOUT + 2.0) as usize {
            step(&mut app, 1.0, |p| {
                p.total = 2000;
                p.ready = 500;
                p.colliders_pending = 300;
                p.scene_ready = false;
            });
        }
        assert!(!settling(&mut app), "the stall backstop never fired");
        let sent: Vec<crate::net::ClientCommand> = rx.try_iter().collect();
        assert!(
            matches!(sent[..], [crate::net::ClientCommand::WorldportAck]),
            "the timeout release must still ack the transfer, got {sent:?}"
        );
    }
}
