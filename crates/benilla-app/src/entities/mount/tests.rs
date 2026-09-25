//! The mount transition re-seats the rider and destroys nothing on it: the held item and the aura
//! glow hung off its bone anchor stay the same entities through a mount, a dismount and a re-mount.

use super::*;
use benilla_assets::{ModelJoint, ModelSkeleton};
use benilla_protocol::ObjectFields;

use crate::entities::{BoneAttach, VisualAttached};
use crate::net::ObjectStore;

/// `UNIT_FIELD_MOUNTDISPLAYID` (index 133), the wire's one mounted signal.
const FIELD_MOUNTDISPLAYID: u16 = 133;
/// The mount's attachment-0 bone in the fixture mount rig.
const SEAT_BONE: u16 = 3;

/// A standing rider: one consumer anchor (the rig's only child) with a held item and a persistent
/// spell-effect root under it, in the shape `attach_spell_fx` builds.
struct Standing {
    app: App,
    rider: Entity,
    anchor: Entity,
    held: Entity,
    glow: Entity,
}

/// A one-bone rig buffer rooted at `frame`, with `anchor` registered as bone 0's consumer anchor.
fn rig_at(frame: Entity, anchor: Entity) -> RigPose {
    let skeleton = ModelSkeleton {
        joints: vec![ModelJoint {
            parent: -1,
            local_translation: Vec3::ZERO,
            billboard: None,
            parent_arm: None,
        }],
        ..Default::default()
    };
    let mut rig = RigPose::new(frame, &skeleton);
    rig.anchors.push((0, anchor));
    rig
}

fn mounted_on(display: u32) -> ObjectStore {
    ObjectStore(ObjectFields::from_pairs(&[(FIELD_MOUNTDISPLAYID, display)]))
}

fn stand() -> Standing {
    let mut app = App::new();
    app.add_systems(Update, reseat_mounts);

    let rider = app
        .world_mut()
        .spawn((
            NetEntity {
                kind: EntityKind::Player,
                display_id: Some(42),
                scale: 1.0,
            },
            mounted_on(0),
            VisualAttached,
            AppliedMount(0),
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    let anchor = app
        .world_mut()
        .spawn((Transform::default(), Visibility::default(), ChildOf(rider)))
        .id();
    let held = app
        .world_mut()
        .spawn((Transform::default(), Visibility::default(), ChildOf(anchor)))
        .id();
    let glow = app
        .world_mut()
        .spawn((Transform::default(), Visibility::default(), ChildOf(anchor)))
        .id();
    let rig = rig_at(rider, anchor);
    app.world_mut().entity_mut(rider).insert(rig);
    app.update();
    Standing {
        app,
        rider,
        anchor,
        held,
        glow,
    }
}

impl Standing {
    fn field(&mut self, display: u32) {
        self.app
            .world_mut()
            .entity_mut(self.rider)
            .insert(mounted_on(display));
        self.app.update();
    }

    /// Finish the mount child's build as `attach_entity_visuals` would: attached, with the
    /// attachment-0 point the seat hangs from.
    fn mount_attaches(&mut self) -> Entity {
        let child = self.mount_child().expect("a mount child was ordered");
        // The seat joint spawns on demand from the mount's pose, so the test gives only the pose.
        let pose =
            benilla_world::testing::test_rig_pose(child, &[Vec3::ZERO; SEAT_BONE as usize + 1]);
        self.app.world_mut().entity_mut(child).insert((
            VisualAttached,
            pose,
            BoneAttach {
                points: [(0u16, (SEAT_BONE, Vec3::Y))].into_iter().collect(),
                markers: Default::default(),
            },
        ));
        self.app.update();
        child
    }

    fn mount_child(&self) -> Option<Entity> {
        self.app
            .world()
            .entity(self.rider)
            .get::<MountChild>()
            .map(|c| c.0)
    }

    fn frame(&self) -> Entity {
        self.app
            .world()
            .entity(self.rider)
            .get::<RigPose>()
            .unwrap()
            .joints_root
    }

    fn parent_of(&self, e: Entity) -> Option<Entity> {
        self.app.world().entity(e).get::<ChildOf>().map(|c| c.0)
    }

    fn applied(&self) -> u32 {
        self.app
            .world()
            .entity(self.rider)
            .get::<AppliedMount>()
            .map_or(0, |a| a.0)
    }

    fn attachments_alive(&self) -> bool {
        let w = self.app.world();
        w.get_entity(self.anchor).is_ok()
            && w.get_entity(self.held).is_ok()
            && w.get_entity(self.glow).is_ok()
    }
}

/// The reference re-parents the body model onto the mount (`0x712f70`) and never re-creates it:
/// the same anchor, held item and glow root, now under the mount's seat.
#[test]
fn mounting_re_seats_the_rig_and_destroys_nothing_on_the_rider() {
    let mut s = stand();
    s.field(2404);
    assert_eq!(s.applied(), 0, "still standing while the mount model loads");
    assert_eq!(s.frame(), s.rider, "…on its own frame");
    assert!(s.attachments_alive(), "…and nothing was torn down to wait");

    s.mount_attaches();
    assert_eq!(s.applied(), 2404);
    assert!(
        s.attachments_alive(),
        "the anchor, the held item and the aura glow all survive the mount"
    );
    let seat = s.frame();
    assert_ne!(seat, s.rider, "the rig re-rooted onto a seat anchor");
    assert_eq!(
        s.parent_of(s.anchor),
        Some(seat),
        "the rig's anchor moved onto the seat",
    );
    assert_eq!(
        (s.parent_of(s.held), s.parent_of(s.glow)),
        (Some(s.anchor), Some(s.anchor)),
        "and everything hanging off it rode along, unmoved",
    );
}

/// Dismount (`0x607ce0`) detaches the body onto its own frame and destroys the mount model.
#[test]
fn dismounting_puts_the_rig_back_on_its_own_frame_and_keeps_the_glow() {
    let mut s = stand();
    s.field(2404);
    let child = s.mount_attaches();
    let seat = s.frame();

    s.field(0);
    assert_eq!(s.applied(), 0);
    assert_eq!(s.frame(), s.rider, "the body is back at the unit matrix");
    assert_eq!(s.parent_of(s.anchor), Some(s.rider));
    assert!(
        s.mount_child().is_none(),
        "the `[unit+0xdc]` handle cleared"
    );
    assert!(
        s.app.world().get_entity(child).is_err() && s.app.world().get_entity(seat).is_err(),
        "the mount model and its seat are destroyed",
    );
    assert!(s.attachments_alive(), "the rider's own visual is untouched");
}

/// A re-mount is `0x5ffa50`'s two calls in order: tear the old seat down, then build the new one.
#[test]
fn a_re_mount_tears_the_old_seat_down_before_it_builds_the_new_one() {
    let mut s = stand();
    s.field(2404);
    let first = s.mount_attaches();

    s.field(2405);
    assert!(
        s.app.world().get_entity(first).is_err(),
        "the old mount model is destroyed first (`0x607ce0`)",
    );
    assert_eq!(s.frame(), s.rider, "…the body detached onto its own frame");
    assert_eq!(s.applied(), 0);

    s.app.update(); // the build leg orders the new model
    s.mount_attaches();
    assert_eq!(s.applied(), 2405);
    assert_eq!(s.parent_of(s.anchor), Some(s.frame()));
    assert!(s.attachments_alive());
}

/// A field that zeroes mid-load leaves `AppliedMount` at 0, so the diff also keys on the ordered
/// mount; otherwise the horse arrives riderless and stays for the unit's life.
#[test]
fn a_mount_ordered_and_then_cancelled_mid_load_is_dropped() {
    let mut s = stand();
    s.field(2404);
    let pending = s.mount_child().expect("ordered");
    s.field(0);
    assert!(
        s.app.world().get_entity(pending).is_err(),
        "the ordered-but-unseated mount is destroyed",
    );
    assert!(s.mount_child().is_none());
    assert_eq!(s.frame(), s.rider);
    s.app.update();
    assert!(s.mount_child().is_none(), "and it stays gone — no churn");
}

#[test]
fn a_field_that_moves_mid_load_drops_the_pending_mount() {
    let mut s = stand();
    s.field(2404);
    let pending = s.mount_child().expect("ordered");
    s.field(2405);
    assert!(
        s.app.world().get_entity(pending).is_err(),
        "the child built for the display we left is dropped",
    );
    s.app.update();
    let replacement = s.mount_child().expect("re-ordered");
    assert_eq!(
        s.app
            .world()
            .entity(replacement)
            .get::<NetEntity>()
            .unwrap()
            .display_id,
        Some(2405),
    );
}

/// A mount with no attachment 0 fails `0x60ce70`'s present test: the reference logs and leaves the
/// body at the unit matrix. The field is still stamped, so the diff cannot churn.
#[test]
fn a_mount_without_a_seat_leaves_the_body_at_the_unit_matrix() {
    let mut s = stand();
    s.field(2404);
    let child = s.mount_child().expect("ordered");
    s.app.world_mut().entity_mut(child).insert((
        VisualAttached,
        BoneAttach {
            points: Default::default(),
            markers: Default::default(),
        },
    ));
    s.app.update();
    assert_eq!(s.applied(), 2404, "stamped — the diff cannot churn");
    assert_eq!(s.frame(), s.rider);
    assert!(s.attachments_alive());
}
