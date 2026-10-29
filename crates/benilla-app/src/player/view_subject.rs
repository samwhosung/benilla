//! What the camera orbits: our body, or a far-sight subject elsewhere. Far sight moves only the
//! eye, so the subject is published for the camera, not made a control branch like free-fly.
//!
//! The server's signal is `PLAYER_FARSIGHT` (player field 712) on our own object, and the client
//! answers it with `UpdateFarSight` (`0x5ee290`, [`FarSight`]), which moves the camera and tells
//! the server with `CMSG_FAR_SIGHT`. vmangos defers the set by 400 ms
//! (`Player::ScheduleCameraUpdate`, `Player.cpp:19136`), so the subject has streamed first, and
//! sends the clear at once to every client in range (`DirectSendPublicValueUpdate`), so only our
//! [`SelfPlayer`] store is read; it never sends `SMSG_CLEAR_FAR_SIGHT_IMMEDIATE`, which another
//! server may, and which ends far sight at once ([`on_clear_far_sight`]). Mind Control sets the
//! same field (`SpellAuras.cpp:2975`).

use std::sync::{Mutex, PoisonError};

use bevy::prelude::*;

use super::{head_height, CameraPivot};
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetEntity, ObjectStore, SelfPlayer};

/// Where the camera rig sits this frame when it is not our body; `None` is the body. It carries a
/// resolved pose because the controller, holding our own `Transform`, cannot query another's.
#[derive(Resource, Default)]
pub(crate) struct ViewSubject {
    pub(crate) remote: Option<RemoteView>,
}

/// A far-sight subject's pose, in the two forms the camera rig asks for.
#[derive(Clone, Copy)]
pub(crate) struct RemoteView {
    /// For readers of the followed unit's state: the reference's camera shake reads the followed
    /// unit (`[cam+0x88]`/`[cam+0x8c]`), never the active player (`0x468550`).
    pub(crate) entity: Entity,
    /// The subject's feet in world space, which the rig orbits.
    pub(crate) feet: Vec3,
    /// Its framing pivot height above the feet, as [`head_height`] gives it.
    pub(crate) pivot_height: f32,
}

impl RemoteView {
    /// The camera-collision boom's origin: the subject's framing pivot, standing in for the capsule
    /// top our own body uses. Rooted at our body, the boom would sweep across the world to it.
    pub(crate) fn sweep_origin(&self) -> Vec3 {
        self.feet + Vec3::Y * self.pivot_height
    }
}

/// The far-sight machine `UpdateFarSight` (`0x5ee290`) runs for the active player, written
/// through `&self` as a shared sink of the VM pass ([`crate::vm_pass::shared_sinks`]): the buff
/// cancel runs it inside its call (`0x6e70ad`); the update handler at the field's change
/// (`0x5de0d0`) and at the arrival of the object it names (`0x5fbb1a`, `0x5d552d`); the clear's
/// handler (`0x5e441d`); and [`publish_view_subject`] for the camera's lost object. Our player's
/// free takes the latch with it ([`Self::free`]).
///
/// Not built: the mover switch (`SetActiveMover` `0x6006e0` at `0x5ee35b`, `0x5ee477`,
/// `0x5ee573`), which possession already moves by the control update; the attack stop at a
/// start (`0x5ecac0` at `0x5ee445`); the targeting stop at an end (`0x6e4920` at `0x5ee545`).
#[derive(Resource, Default)]
pub(crate) struct FarSight(Mutex<FarSightState>);

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct FarSightState {
    /// `+0x1c70` bit 0: `CMSG_FAR_SIGHT` 1 sent and 0 not yet.
    on: bool,
    /// The far-sight object the camera follows (`[cam+0x88]`); `None` is the active player.
    camera: Option<u64>,
}

impl FarSight {
    fn state(&self) -> std::sync::MutexGuard<'_, FarSightState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The far-sight object the camera follows, `None` for our own player.
    pub(crate) fn camera(&self) -> Option<u64> {
        self.state().camera
    }

    /// `UpdateFarSight` (`0x5ee290`) for the active player, whose `PLAYER_FARSIGHT` is `field`,
    /// with `held` the client's object lookup (`0x5ee2ef`). A field naming no held object puts
    /// the camera back on the player (`0x5ee334`) and answers 0 (`0x5ee360`). A held object the
    /// camera is not on starts far sight: `CMSG_FAR_SIGHT` 1 unless it is on already (`0x5ee39c`,
    /// `0x5ee3d8`), and the camera on the object (`0x5ee483`). Anything else, no field or the
    /// camera already on its object (`0x5ee396`), ends it: 0 when it was on (`0x5ee4c4`,
    /// `0x5ee500`), and the camera on the player (`0x5ee54c`). Both answer 1.
    pub(crate) fn update(
        &self,
        field: Option<u64>,
        held: impl Fn(u64) -> bool,
        net: &NetCommands,
    ) -> bool {
        let mut st = self.state();
        let object = field.filter(|&guid| held(guid));
        if field.is_some() && object.is_none() {
            st.camera = None;
            return false;
        }
        match object {
            Some(guid) if st.camera != Some(guid) => {
                if !st.on {
                    let _ = net.0.send(ClientCommand::FarSight { engage: true });
                    st.on = true;
                }
                st.camera = Some(guid);
            }
            _ => {
                if st.on {
                    st.on = false;
                    let _ = net.0.send(ClientCommand::FarSight { engage: false });
                }
                st.camera = None;
            }
        }
        true
    }

    /// `StartFarSight` (`0x5ee270`): the update while far sight is off.
    pub(crate) fn start(&self, field: Option<u64>, held: impl Fn(u64) -> bool, net: &NetCommands) {
        if !self.state().on {
            self.update(field, held, net);
        }
    }

    /// `EndFarSight` (`0x5ee590`): the update while far sight is on.
    pub(crate) fn end(&self, field: Option<u64>, held: impl Fn(u64) -> bool, net: &NetCommands) {
        if self.state().on {
            self.update(field, held, net);
        }
    }

    /// Whether far sight is on (`+0x1c70` bit 0).
    pub(crate) fn on(&self) -> bool {
        self.state().on
    }

    /// Our player freed with every object, at a world's end or a cross-map load (`0x467800`): the
    /// bit goes with it, and the next player's constructor zeroes it (`0x5dd33d`), so the next
    /// world owes the server no `CMSG_FAR_SIGHT` 0 and sends its first start's 1.
    pub(crate) fn free(&self) {
        *self.state() = FarSightState::default();
    }
}

/// Runs the one far-sight caller outside a packet and a script call, the world frame's render
/// (`0x483031`, `0x483054`-`0x483061`): with far sight on and the camera's object gone, the
/// update. Then it resolves the camera's object into a pose, before the controller runs; an object
/// not held leaves the camera on the body.
pub(super) fn publish_view_subject(
    mut subject: ResMut<ViewSubject>,
    far_sight: Res<FarSight>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    index: Res<GuidIndex>,
    // The pivot height takes the raw `OBJECT_FIELD_SCALE_X`, not the eased render scale.
    poses: Query<(&Transform, Option<&CameraPivot>, Option<&NetEntity>)>,
    net: Res<NetCommands>,
) {
    let field = self_store
        .iter()
        .next()
        .and_then(|store| store.0.player_farsight());
    let held = |guid: u64| index.0.contains_key(&guid);
    if far_sight.camera().is_some_and(|guid| !held(guid)) && far_sight.on() {
        far_sight.update(field, held, &net);
    }

    subject.remote = far_sight
        .camera()
        .and_then(|guid| index.0.get(&guid).copied())
        .and_then(|entity| Some((entity, poses.get(entity).ok()?)))
        .map(|(entity, (t, pivot, net))| RemoteView {
            entity,
            feet: t.translation,
            pivot_height: head_height(pivot, net.map_or(1.0, |n| n.scale)),
        });
}

/// `SMSG_CLEAR_FAR_SIGHT_IMMEDIATE` (`0x20d`): the handler's jump table (`0x5e417e`, entry 5)
/// leads to `0x5e43f8`, which looks up the active player (`0x5e440e`) and runs `EndFarSight` on it
/// (`0x5ee590` at `0x5e441d`), reading no body. The field still names the object, so the view
/// stays home until it changes.
fn on_clear_far_sight(
    In(ev): In<benilla_protocol::SessionEvent>,
    far_sight: Res<FarSight>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    index: Res<GuidIndex>,
    net: Res<NetCommands>,
) {
    let (benilla_protocol::SessionEvent::FarSightCleared, Some(store)) =
        (ev, self_store.iter().next())
    else {
        return;
    };
    let held = |guid: u64| index.0.contains_key(&guid);
    far_sight.end(store.0.player_farsight(), held, &net);
}

/// The far-sight packet's handler.
pub(super) fn register(app: &mut App) {
    use crate::net::NetHandlerApp;
    app.net_handler(
        benilla_protocol::SessionEventKind::FarSightCleared,
        on_clear_far_sight,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit's network identity.
    fn unit_identity() -> crate::net::NetEntity {
        crate::net::NetEntity {
            kind: benilla_protocol::EntityKind::Unit,
            display_id: None,
            scale: 1.0,
        }
    }

    /// Far sight started on `guid` as its create starts it (`0x5fbb1a`), the update handler's.
    fn started_on(app: &App, guid: u64) {
        let world = app.world();
        let index = world.resource::<GuidIndex>();
        world.resource::<FarSight>().start(
            Some(guid),
            |g| index.0.contains_key(&g),
            world.resource::<NetCommands>(),
        );
    }

    #[test]
    fn an_unset_or_unstreamed_far_sight_both_resolve_to_the_body() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<ViewSubject>()
            .init_resource::<FarSight>()
            .init_resource::<GuidIndex>()
            .insert_resource(NetCommands(tx))
            .add_systems(Update, publish_view_subject);
        // The `CMSG_FAR_SIGHT` votes, as engage booleans.
        let votes = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<bool> {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::FarSight { engage } => Some(engage),
                    _ => None,
                })
                .collect()
        };

        // A unit standing 100 yards away, at scale 1.
        let subject = app
            .world_mut()
            .spawn((
                Transform::from_xyz(100.0, 0.0, 0.0),
                CameraPivot {
                    height_local: 2.0,
                    swim_drop_local: 0.0,
                },
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
            ))
            .id();

        // No far sight: the body.
        let me = app
            .world_mut()
            .spawn((
                SelfPlayer,
                Transform::default(),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[])),
            ))
            .id();
        app.update();
        assert!(
            app.world().resource::<ViewSubject>().remote.is_none(),
            "no far sight set → the camera stays on the body"
        );
        assert!(votes(&rx).is_empty(), "no field, no vote");

        // Far sight set, the object not streamed: still the body, not the origin.
        const FARSIGHT: u16 = 712;
        let set = |app: &mut App, lo: u32| {
            *app.world_mut().get_mut::<ObjectStore>(me).unwrap() =
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (FARSIGHT, lo),
                    (FARSIGHT + 1, 0),
                ]));
        };
        set(&mut app, 0x1234);
        app.update();
        assert!(
            app.world().resource::<ViewSubject>().remote.is_none(),
            "far sight naming an unstreamed object → the body, never the world origin"
        );
        assert!(
            votes(&rx).is_empty(),
            "a field naming no held object takes the end (`0x5de15d`), which sends nothing while \
             far sight is off"
        );

        // Streamed: the pass starts nothing, as the arrival is the create's (`0x5fbb1a`).
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(0x1234, subject);
        app.update();
        assert!(
            app.world().resource::<ViewSubject>().remote.is_none(),
            "the pass starts no far sight"
        );
        assert!(votes(&rx).is_empty(), "and tells the server nothing");

        // Started: the subject's feet, and its pivot as the sweep origin's height.
        started_on(&app, 0x1234);
        assert_eq!(votes(&rx), [true], "the start tells the server once");
        app.update();
        let remote = app
            .world()
            .resource::<ViewSubject>()
            .remote
            .expect("a streamed far-sight object resolves");
        assert_eq!(remote.feet, Vec3::new(100.0, 0.0, 0.0));
        assert_eq!(
            remote.sweep_origin(),
            Vec3::new(100.0, 2.0, 0.0),
            "the boom sweeps from the SUBJECT's head, not ours — otherwise it is cast across the \
             world and stops at the first wall in between"
        );
        app.update();
        assert!(votes(&rx).is_empty(), "once, not per frame");

        // Cleared to zero: the field's change is its handler's (`0x5de0d0`), so the frame's pass
        // leaves the view where it is.
        set(&mut app, 0);
        app.update();
        assert!(votes(&rx).is_empty(), "the pass runs no end of its own");
    }

    /// Cancelling the buff of a far-sight spell (`AttributesEx` 0x2000 without 0x4: Sentry Totem,
    /// 6495) runs `UpdateFarSight` (`0x6e70ad` → `0x5ee290`). With the camera on the field's object
    /// it ends far sight: `CMSG_FAR_SIGHT` 0 (`0x5ee500`) and the camera back on the player
    /// (`0x5ee54c`); with the camera elsewhere it starts it again, 1 (`0x5ee3d8`) and the camera on
    /// the object (`0x5ee483`). Either way it answers 1, so the cancel sends no `CMSG_CANCEL_AURA`
    /// (`0x6e70b4`).
    #[test]
    fn a_far_sight_buff_cancel_toggles_the_view_and_tells_the_server() {
        const SENTRY_TOTEM: u32 = 6495;
        const TOTEM: u64 = 0xF130_0000_0000_0042;
        const FARSIGHT: u16 = 712;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        app.insert_resource(NetCommands(tx));
        app.world_mut().insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(
                    SENTRY_TOTEM,
                    benilla_formats::SpellDisplay {
                        id: SENTRY_TOTEM,
                        attributes_ex: 0x2000,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let totem = app
            .world_mut()
            .spawn((Transform::from_xyz(50.0, 0.0, 0.0), unit_identity()))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(TOTEM, totem);
        app.world_mut().spawn((
            SelfPlayer,
            Transform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (FARSIGHT, TOTEM as u32),
                (FARSIGHT + 1, (TOTEM >> 32) as u32),
            ])),
        ));
        let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<String> {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::FarSight { engage } => {
                        Some(format!("far sight {}", engage as u8))
                    }
                    ClientCommand::CancelAura { spell_id } => Some(format!("cancel {spell_id}")),
                    _ => None,
                })
                .collect()
        };
        let publish = |app: &mut App| {
            app.world_mut()
                .run_system_cached(publish_view_subject)
                .unwrap();
            app.world()
                .resource::<ViewSubject>()
                .remote
                .map(|r| r.entity)
        };
        let cancel = |app: &mut App| {
            app.world_mut()
                .run_system_cached(|aura: crate::ui_aura::AuraSteps| {
                    aura.cancel(None, SENTRY_TOTEM)
                })
                .unwrap();
        };

        started_on(&app, TOTEM);
        assert_eq!(
            publish(&mut app),
            Some(totem),
            "the totem's arrival engaged it (`0x5fbb1a`)"
        );
        assert_eq!(sent(&rx), ["far sight 1"]);

        cancel(&mut app);
        assert_eq!(
            sent(&rx),
            ["far sight 0"],
            "the view ends, and no aura cancel"
        );
        assert_eq!(publish(&mut app), None, "the camera is back on the player");
        assert_eq!(sent(&rx), Vec::<String>::new());

        cancel(&mut app);
        assert_eq!(
            sent(&rx),
            ["far sight 1"],
            "and a second cancel looks again"
        );
        assert_eq!(publish(&mut app), Some(totem));
    }

    /// `SMSG_CLEAR_FAR_SIGHT_IMMEDIATE` (`0x20d`) leads through the handler's jump table
    /// (`0x5e417e`, entry 5) to `0x5e43f8`, which runs `EndFarSight` (`0x5ee590`) on the active
    /// player, the body unread: with far sight on, `CMSG_FAR_SIGHT` 0 and the camera back on the
    /// player at once, though the field still names the object; with it off, nothing.
    #[test]
    fn the_clear_far_sight_packet_ends_the_view_at_once() {
        const EYE: u64 = 0xF130_0000_0000_0043;
        const FARSIGHT: u16 = 712;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        app.insert_resource(NetCommands(tx));
        let eye = app
            .world_mut()
            .spawn((Transform::from_xyz(50.0, 0.0, 0.0), unit_identity()))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(EYE, eye);
        app.world_mut().spawn((
            SelfPlayer,
            Transform::default(),
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (FARSIGHT, EYE as u32),
                (FARSIGHT + 1, (EYE >> 32) as u32),
            ])),
        ));
        let votes = |rx: &crossbeam_channel::Receiver<ClientCommand>| -> Vec<bool> {
            rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::FarSight { engage } => Some(engage),
                    _ => None,
                })
                .collect()
        };
        let publish = |app: &mut App| {
            app.world_mut()
                .run_system_cached(publish_view_subject)
                .unwrap();
            app.world()
                .resource::<ViewSubject>()
                .remote
                .map(|r| r.entity)
        };
        let clear = |app: &mut App| {
            crate::net::handlers::dispatch(
                app.world_mut(),
                vec![benilla_protocol::SessionEvent::FarSightCleared],
            );
        };
        started_on(&app, EYE);
        assert_eq!(publish(&mut app), Some(eye));
        assert_eq!(votes(&rx), [true]);

        clear(&mut app);
        assert_eq!(votes(&rx), [false], "the server is told at once");
        assert_eq!(publish(&mut app), None, "the camera is back on the player");
        assert_eq!(
            publish(&mut app),
            None,
            "and stays there while the field still names the object"
        );
        clear(&mut app);
        assert!(votes(&rx).is_empty(), "with far sight off, nothing");
    }
}
