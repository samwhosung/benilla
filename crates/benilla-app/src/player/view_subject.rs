//! What the camera orbits: our body, or a far-sight subject elsewhere. Far sight moves only the
//! eye, so the subject is published for the camera, not made a control branch like free-fly.
//!
//! The server's only signal is `PLAYER_FARSIGHT` (player field 712) on our own object: non-zero
//! looks through that guid, zero comes home. vmangos defers the set by 400 ms
//! (`Player::ScheduleCameraUpdate`, `Player.cpp:19136`), so the subject has streamed first, and
//! sends the clear at once to every client in range (`DirectSendPublicValueUpdate`), so only our
//! [`SelfPlayer`] store is read; it never sends `SMSG_CLEAR_FAR_SIGHT_IMMEDIATE`. Mind Control
//! sets the same field (`SpellAuras.cpp:2975`).

use bevy::prelude::*;

use super::{head_height, CameraPivot};
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore, SelfPlayer};

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

/// Resolves our `PLAYER_FARSIGHT` into a pose, before the controller runs; a subject not yet
/// streamed leaves the camera on the body.
pub(super) fn publish_view_subject(
    mut subject: ResMut<ViewSubject>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    index: Res<GuidIndex>,
    // The pivot height takes the raw `OBJECT_FIELD_SCALE_X`, not the eased render scale.
    poses: Query<(
        &Transform,
        Option<&CameraPivot>,
        Option<&crate::net::NetEntity>,
    )>,
    net: Res<NetCommands>,
    mut engaged: Local<Option<u64>>,
) {
    let anchor = self_store
        .iter()
        .next()
        .and_then(|store| store.0.player_farsight());
    subject.remote = anchor
        .and_then(|guid| index.0.get(&guid).copied())
        .and_then(|entity| Some((entity, poses.get(entity).ok()?)))
        .map(|(entity, (t, pivot, net))| RemoteView {
            entity,
            feet: t.translation,
            pivot_height: head_height(pivot, net.map_or(1.0, |n| n.scale)),
        });

    // `CMSG_FAR_SIGHT` on the field's edge: 1 as the view attaches and 0 as it releases, the
    // reference's two sites (`0x5ee290`). It follows the field, not the resolve: a release sent
    // while the subject streams in would tear down the server's stream around it.
    if *engaged != anchor {
        if anchor.is_some() {
            let _ = net.0.send(ClientCommand::FarSight { engage: true });
        } else {
            let _ = net.0.send(ClientCommand::FarSight { engage: false });
        }
        *engaged = anchor;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_or_unstreamed_far_sight_both_resolve_to_the_body() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<ViewSubject>()
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

        // A subject standing 100 yards away, at scale 1.
        let subject = app
            .world_mut()
            .spawn((
                Transform::from_xyz(100.0, 0.0, 0.0),
                CameraPivot {
                    height_local: 2.0,
                    swim_drop_local: 0.0,
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
        assert_eq!(
            votes(&rx),
            [true],
            "the vote is keyed off the FIELD, not off resolving the object — voting 'released' \
             here would tear down the server's stream around the object we are waiting for"
        );
        app.update();
        assert!(
            votes(&rx).is_empty(),
            "and it fires on the edge, not per frame"
        );

        // Streamed: the subject's feet, and its pivot as the sweep origin's height.
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(0x1234, subject);
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

        // Cleared to zero: home.
        set(&mut app, 0);
        app.update();
        assert!(
            app.world().resource::<ViewSubject>().remote.is_none(),
            "a zeroed field is the ONLY teardown signal there is — no CLEAR_FAR_SIGHT_IMMEDIATE"
        );
        assert_eq!(votes(&rx), [false], "and the release votes 0, once");
    }
}
