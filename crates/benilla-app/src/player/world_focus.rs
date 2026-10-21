//! What the game tells the world, and what it does with what the world publishes back:
//! [`publish_view_focus`] says where to stream from, ahead of the stream stage,
//! [`release_post_snap_hold`] ends the mover's settle hold on the residency the world publishes,
//! and [`publish_caster_outdoors`] asks the world whether each caster stands outdoors. The hold
//! ends on residency, never on ground contact, the same way for every mover mode.

use bevy::prelude::*;

use super::{Player, SETTLE_TIMEOUT};
use benilla_world::terrain_stream::{ViewFocus, WorldLoadProgress};
use benilla_world::view::Viewer;

/// Tells the world where the avatar's body is ([`Viewer`]), with the focus's position and gate.
pub(super) fn publish_viewer(
    mut viewer: ResMut<Viewer>,
    player: Option<Res<Player>>,
    rig: Option<Res<super::CameraControl>>,
    screen: Option<Res<crate::loading_screen::LoadingScreen>>,
    store: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
) {
    // Drunk and ghost are whole-screen effects of our own character, so they read `SelfPlayer`,
    // not the body we drive: possessing a boar does not sober you up.
    let (drunk, ghost) = match store.single() {
        Ok(s) => (
            s.0.player_drunk_byte()
                .map_or(0.0, |b| f32::from(b.min(100)) / 100.0),
            s.0.player_is_ghost(),
        ),
        Err(_) => (0.0, false),
    };
    // `WOW_GHOST_PROBE` overrides the flag the death light and the DeathClouds sky both read.
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
        // The zoom feather alone: the mesh side folds in the aura factor, so it must not be here.
        self_fade: rig.as_deref().map_or(1.0, super::CameraControl::self_fade),
        world_covered: screen.as_deref().is_some_and(|s| s.covering()),
        ..body
    };
}

/// Tells the world where to stream from each frame, before the stream stage; a cinematic, which
/// streams from the camera, also puts the body on the settle hold.
pub(super) fn publish_view_focus(
    mut focus: ResMut<ViewFocus>,
    mut player: Option<ResMut<Player>>,
    roster: Option<Res<crate::glue_session::roster::Roster>>,
    cinematic: Option<Res<crate::cinematic::Cinematic>>,
    mut was_flying: Local<bool>,
) {
    let entry = roster
        .as_deref()
        .and_then(crate::glue_session::roster::Roster::pending_entry);
    // A cinematic flies the eye away from the body (a Tauren intro opens 1741 yd out), so the
    // stream follows the camera, as in free-fly.
    let flying = cinematic.as_deref().is_some_and(|c| c.is_playing());
    // Unlike free-fly, where `control` skips the body, a cinematic simulates it while its tiles
    // unload, so it takes the settle hold, which only residency of its own tile ends. `world_stale`
    // keeps the stall backstop from turning gravity back on mid-shot. Armed on the rising edge
    // only, so a shot that stays on the body's tile lets the release end the hold.
    if flying && !*was_flying {
        if let Some(p) = player.as_deref_mut().filter(|p| p.active) {
            p.settling = true;
            p.world_stale = true;
            info!("cinematic: body held — the stream follows the camera off its ground");
        }
    }
    *was_flying = flying;
    // Spawn caps pace only a live avatar in a settled world; through entry, a teleport or a world
    // swap a cap would only lengthen the load.
    *focus = match player.as_deref() {
        Some(p) if p.active => {
            let wow = benilla_assets::coords::bevy_to_wow(p.pos);
            let paced = !p.settling && !p.world_stale;
            if flying {
                ViewFocus::cinematic(wow, paced)
            } else if p.detached {
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

/// Ends the post-snap hold when the destination's world has arrived. Runs after the stream stage,
/// so `colliders_pending` is this frame's count.
pub(super) fn release_post_snap_hold(
    mut player: ResMut<Player>,
    progress: Option<Res<WorldLoadProgress>>,
    time: Res<Time>,
    mut last_counters: Local<Option<[usize; 6]>>,
    net_cmds: Option<Res<crate::net::NetCommands>>,
) {
    let Some(p) = progress else { return };
    // Residency counts only for the tile under the body. On a mismatch (the teleport's snap frame,
    // a free-fly eye) the release and the stale-clear are refused and only the backstop runs.
    let focus_matches = p.focus_tile.is_some_and(|t| {
        let wow = benilla_assets::coords::bevy_to_wow(player.pos);
        let (tx, ty) = benilla_formats::world_to_tile(wow[0], wow[1]);
        t == (tx as i32, ty as i32)
    });
    // Residency means this map's own tile (a WMO-only map's one building) is spawned, which a
    // swap reaches only after draining every tile of the map we left.
    if p.focus_resident && p.total > 0 && focus_matches {
        player.world_stale = false;
    }
    // Any counter moving is the destination still arriving; tracked every frame, so the first
    // settling frame has a baseline.
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
    // The release needs scene and colliders, never ground contact. The timeout is a stall budget:
    // the deadline is pushed while the resident world is still the map we left and while the
    // stream advances, so only a stream with no progress for the whole budget times out.
    let now = time.elapsed_secs();
    if p.presentable() && !player.world_stale && focus_matches {
        player.end_settle(true, now);
    } else if player.world_stale || progressed {
        player.settle_deadline = now + SETTLE_TIMEOUT;
    } else if now >= player.settle_deadline {
        player.end_settle(false, now);
    }
    // The worldport ack, the reference's post-load `0xDC`, goes at either end of the hold: the
    // server keeps an unacked transfer out of world until logout.
    if !player.settling && player.owes_worldport_ack {
        if let Some(net) = net_cmds.as_deref() {
            player.owes_worldport_ack = false;
            let _ = net.0.send(crate::net::ClientCommand::WorldportAck);
            info!("worldport: ack sent at settle release");
        }
    }
}

/// The active player's answer to `CWorld::IsEntityOutdoors 0x6706f0`, which the spell validator
/// asks through `0x612cc0` for its place rungs; true with no body, as for a null entity
/// (`0x6706fa`).
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PlayerOutdoors(pub(crate) bool);

impl Default for PlayerOutdoors {
    fn default() -> Self {
        Self(true)
    }
}

/// The pet bar's unit's answer to `CWorld::IsEntityOutdoors 0x6706f0`, with its guid: the one
/// caster but the active player, the bar's presses passing it to the attempt (`0x4bd385`), whose
/// place rungs ask that unit's own node through `0x612cc0`. `None` while the bar's unit has no
/// body.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct BarUnitOutdoors(pub(crate) Option<(u64, bool)>);

/// Asks the entity outdoors test of the world for each caster after this frame's re-place:
/// [`PlayerOutdoors`] where our own body stands (free-fly freezes it in place, so it still
/// answers), and [`BarUnitOutdoors`] where the pet bar's unit stands, at its world position, as a
/// mounted unit's `Transform` is seat-relative.
pub(crate) fn publish_caster_outdoors(
    test: benilla_world::wmo_portal::EntityOutdoors,
    player: Option<Res<Player>>,
    mut outdoors: ResMut<PlayerOutdoors>,
    bar: Option<Res<crate::ui_pet::PetBar>>,
    index: Option<Res<crate::net::GuidIndex>>,
    bodies: Query<&GlobalTransform>,
    mut unit_outdoors: ResMut<BarUnitOutdoors>,
) {
    let now = match player.as_deref() {
        // `pos` is another unit's while we drive it, and still the last mover's until the new
        // one's pose is adopted (`reseat`): our own body's answer holds.
        Some(p) if p.active && (p.foreign_mover.is_some() || p.reseat) => None,
        Some(p) if p.active => Some(test.at(p.pos)),
        // No body: a null entity reads outdoors (`0x6706fa`).
        _ => Some(true),
    };
    if let Some(now) = now.filter(|&now| outdoors.0 != now) {
        outdoors.0 = now;
    }
    let unit = bar
        .map(|b| b.spells.pet_guid)
        .filter(|&guid| guid != 0)
        .and_then(|guid| {
            let body = bodies.get(*index.as_deref()?.0.get(&guid)?).ok()?;
            Some((guid, test.at(body.translation())))
        });
    if unit_outdoors.0 != unit {
        unit_outdoors.0 = unit;
    }
}

/// One placed building for the entity outdoors test, with the terrain it is asked against: a
/// group of `flags` whose 10-yd floor spans WoW x and y -5 to 5 at z 30, placed at the world's
/// origin.
#[cfg(test)]
pub(crate) fn insert_test_room(world: &mut World, flags: u32) {
    use benilla_assets::{AdtTile, WmoGroupNav, WmoModel};
    use benilla_world::terrain_stream::TerrainStreamer;
    use benilla_world::wmo_portal::WmoPortalInstance;

    let floor = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        [[a[0], a[1], 30.0], [b[0], b[1], 30.0], [c[0], c[1], 30.0]]
    };
    let tris = vec![vec![
        floor([-5.0, -5.0], [5.0, -5.0], [5.0, 5.0]),
        floor([-5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]),
    ]];
    let (bounds, all) = benilla_assets::collision_tri_bounds(&tris);
    let room = WmoModel {
        group_nav: vec![WmoGroupNav {
            flags,
            bbox_min: [-5.0, -5.0, 30.0],
            bbox_max: [5.0, 5.0, 34.0],
            ref_start: 0,
            ref_count: 0,
            area_table_id: 0,
            fog_indices: [0; 4],
            group_liquid: benilla_formats::NO_GROUP_LIQUID,
        }],
        group_collision_grids: benilla_assets::collision_tri_grids(&tris),
        group_collision_tris: tris,
        group_collision_bounds: bounds,
        collision_bounds: all,
        ..WmoModel::default()
    };
    let mut wmos = Assets::<WmoModel>::default();
    let handle = wmos.add(room);
    world.insert_resource(wmos);
    world.init_resource::<Assets<AdtTile>>();
    world.init_resource::<TerrainStreamer>();
    world.spawn(WmoPortalInstance {
        handle,
        world_from_local: bevy::math::Affine3A::IDENTITY,
        name_set: 0,
        visible: vec![true],
        interior_fog: vec![false],
        liquid_visited: vec![false],
        flooded: vec![None],
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// The tile under `Player::default()`: the `focus_tile` when focus and avatar agree.
    fn own_tile() -> (i32, i32) {
        let wow = benilla_assets::coords::bevy_to_wow(Player::default().pos);
        let (tx, ty) = benilla_formats::world_to_tile(wow[0], wow[1]);
        (tx as i32, ty as i32)
    }

    /// The release system registered, so its `Local` baseline survives frames (`run_system_once`
    /// would reset it), a hand-driven clock, and progress about the avatar's own tile.
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

    /// A re-arm would flicker `settling` under the loading screen's clear gate and the zone-channel
    /// walk, which both read it.
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

        // The world ends the hold mid-cinematic, the camera's tile being the body's.
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

    #[test]
    fn a_slow_but_advancing_stream_never_times_out() {
        let mut app = app();
        app.world_mut().resource_mut::<Player>().world_stale = false;
        // Five budgets of wall clock, with a counter moving every frame.
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

    #[test]
    fn a_genuinely_stalled_stream_still_times_out() {
        let mut app = app();
        app.world_mut().resource_mut::<Player>().world_stale = false;
        // Frozen counters: the first frame baselines the `Local` (a change), then the budget runs.
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

    #[test]
    fn a_stale_world_pushes_the_deadline_before_the_stall_budget_starts() {
        let mut app = app();
        app.world_mut().resource_mut::<Player>().world_stale = true;
        // Two budgets of stale, frozen frames; `focus_resident` stays false, so stale stays set.
        for _ in 0..(2.0 * SETTLE_TIMEOUT) as usize {
            step(&mut app, 1.0, |p| {
                p.total = 0;
                p.scene_ready = false;
                p.focus_resident = false;
            });
            assert!(settling(&mut app), "released over the departed map's floor");
        }
        // The destination becomes resident, then stalls: the budget starts here.
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

    /// Spawned but unbaked geometry is a hole like an unattached collider, so it holds the settle.
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

    /// On a teleport's snap frame the focus can still describe the departure tile as resident.
    #[test]
    fn residency_about_another_tile_neither_releases_nor_clears_stale() {
        let mut app = app();
        app.world_mut().resource_mut::<Player>().world_stale = true;
        // A resident, quiet world described for a tile the avatar is not on.
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

    /// The ack is the reference's post-load `0xDC`, so it rides the release, never the snap.
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

    /// vmangos keeps an unacked far teleport out of world, dropping every packet, until logout.
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

    /// The outdoors answer is `0x6706f0` asked where our own body stands: indoors in a room's
    /// interior group, outdoors in an `EXTERIOR` (0x8) group or off the building, and outdoors
    /// with no body, as for a null entity (`0x6706fa`). The body answers in free-fly, which
    /// freezes it and publishes no viewer, and wherever the viewer is. While we drive another
    /// unit, or have not yet adopted our own body's pose after one, `pos` is not our body's, so
    /// the last answer holds.
    #[test]
    fn the_outdoors_answer_is_asked_where_the_body_stands() {
        use benilla_assets::coords::wow_to_bevy;

        // One frame of the system over the test room, from the answer `last`.
        let ask = |flags: u32, player: Player, viewer: Viewer, last: bool| {
            let mut app = App::new();
            insert_test_room(app.world_mut(), flags);
            app.insert_resource(player)
                .insert_resource(viewer)
                .insert_resource(PlayerOutdoors(last))
                .init_resource::<BarUnitOutdoors>()
                .add_systems(Update, publish_caster_outdoors);
            app.update();
            app.world().resource::<PlayerOutdoors>().0
        };
        let (inside, outside) = (
            wow_to_bevy([1.0, 2.0, 30.0]),
            wow_to_bevy([40.0, 2.0, 30.0]),
        );
        let body = |pos: Vec3| Player {
            active: true,
            pos,
            ..Player::default()
        };
        // `publish_viewer`'s body: our position, unless free-flying.
        let seen = |pos: Vec3| Viewer {
            at: Some(pos),
            ..Viewer::default()
        };

        assert!(
            !ask(0, body(inside), seen(inside), true),
            "an interior group"
        );
        assert!(
            ask(0x8, body(inside), seen(inside), false),
            "an exterior group"
        );
        assert!(
            ask(0, body(outside), seen(outside), false),
            "off the building"
        );
        assert!(
            ask(0, Player::default(), Viewer::default(), false),
            "no body"
        );
        let flying = Player {
            detached: true,
            ..body(inside)
        };
        assert!(
            !ask(0, flying, Viewer::default(), true),
            "free-fly: the frozen body, with no viewer"
        );
        assert!(
            !ask(0, body(inside), seen(outside), true),
            "the body, not the viewer"
        );
        let driving = Player {
            foreign_mover: Some(0xF130_0000_0000_0043),
            ..body(outside)
        };
        assert!(
            !ask(0, driving, seen(outside), false),
            "driving another unit holds the answer"
        );
        let reseating = Player {
            reseat: true,
            ..body(outside)
        };
        assert!(
            !ask(0, reseating, seen(outside), false),
            "so does the frame before our own body's pose is adopted"
        );
    }
}
