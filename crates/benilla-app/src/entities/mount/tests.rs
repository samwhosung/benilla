//! The mount transition's law: **nothing on the rider is destroyed by mounting**.
//!
//! The case these pin: Ice Barrier's shield visual stays on the character through a mount, and is
//! still there after the dismount. The old transition
//! `despawn_related::<Children>()`'d the whole rider; the aura's persistent kit instance died with
//! it and, because `FxAttached` lives on the *unit* and kept a dangling root while the aura never
//! left its slot, nothing ever noticed. So these fixtures hang the two things that matter off a
//! bone anchor — a held item and a spell-effect instance root — and assert they are the same
//! entities on the far side of a mount, a dismount and a re-mount.

use super::*;
use benilla_assets::{ModelJoint, ModelSkeleton};
use benilla_protocol::ObjectFields;

use crate::entities::{BoneAttach, VisualAttached};
use crate::net::ObjectStore;

/// `UNIT_FIELD_MOUNTDISPLAYID` (index 133) — the wire's one mounted signal.
const FIELD_MOUNTDISPLAYID: u16 = 133;
/// The mount's attachment-0 bone in the fixture mount rig.
const SEAT_BONE: u16 = 3;

/// A rider standing on its own feet, wearing everything a mount transition used to destroy: one
/// consumer anchor (the rig's only child), a held item under it, and a persistent spell-effect
/// instance root under it — the Ice Barrier glow, in the shape `attach_spell_fx` builds.
struct Standing {
    app: App,
    rider: Entity,
    /// The rig's consumer anchor — the entity every attachment hangs from.
    anchor: Entity,
    /// The held weapon root and the aura glow root, both under `anchor`.
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

/// The descriptor store for a unit whose mount field reads `display`.
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
    /// Move the wire field and run one pass.
    fn field(&mut self, display: u32) {
        self.app
            .world_mut()
            .entity_mut(self.rider)
            .insert(mounted_on(display));
        self.app.update();
    }

    /// Finish the mount child's build the way `attach_entity_visuals` would: mark it attached and
    /// give it the attachment-0 anchor the seat hangs from. Returns the child.
    fn mount_attaches(&mut self) -> Entity {
        let child = self.mount_child().expect("a mount child was ordered");
        // The seat joint spawns on first demand out of the mount's pose — the
        // test provides the pose, not a hand-built joint.
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

    /// The rig's current model frame.
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

    /// Everything that must survive every transition, by entity id.
    fn attachments_alive(&self) -> bool {
        let w = self.app.world();
        w.get_entity(self.anchor).is_ok()
            && w.get_entity(self.held).is_ok()
            && w.get_entity(self.glow).is_ok()
    }
}

/// **The director's report.** Mounting used to destroy the rider's whole visual, taking the
/// aura's persistent kit instance with it — and because `FxAttached` outlived the teardown holding
/// a dangling root while the aura never left its slots, the glow never came back, not even after
/// dismounting. The reference re-parents the body model onto the mount (`0x712f70`); it never
/// re-creates it. So: the same anchor, the same held item, the same glow root — now hanging under
/// the mount's seat.
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

/// The dismount half — `0x607ce0`: detach the body back onto its own frame, destroy the mount
/// model. The rider is again the rig's root, the mount child and its seat are gone, and the glow
/// the report says never returns is the same entity it always was.
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

/// A re-mount (id→id′) is `0x5ffa50`'s two calls in order: tear the old seat down first, then
/// build the new one. The old mount dies, the new one is ordered, and the rider — which is not
/// part of either half — never notices.
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

/// The field can also zero again mid-load — a cast that lands and is cancelled inside the model
/// load. `AppliedMount` never left 0, so the diff must key on the *ordered* mount too, or the
/// horse arrives riderless and stays for the unit's life.
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

/// The field can move again while the mount model is still loading. The pending child is dropped
/// for one built with the display the field actually says, and the rider — which was never torn
/// down to begin with — just keeps standing.
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

/// A mount that authors no attachment 0 is the reference's `0x60ce70` present-test miss: it logs
/// and leaves the body at the unit matrix — mounted, but unseated. It must not spin (the field is
/// stamped) and it must not tear the rider down either.
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
