//! The portrait list: `MarkPortraitDirty` (`0x524e10`) queues a unit's GUID, and each frame the
//! world frame's render (`0x483228`) drains it (`0x524cd0`), signalling `UNIT_PORTRAIT_UPDATE` and
//! then `UNIT_MODEL_CHANGED` for each drained unit (`0x524de4`, `0x524df1`), each once per token
//! naming it (`0x515e50`). The stock unit frames re-take their portrait on the first
//! (`UnitFrame.lua:20`), the character panes their model on the second.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::entities::{DressKey, Equipment, VisualAttached};
use crate::field_watch::{id, FieldNotice, NoticeLane};
use crate::net::{Guid, GuidIndex, ObjectStore};

/// `TYPEMASK_UNIT`, the mask the drain's lookup passes (`0x524d2f`).
const UNIT: u32 = 0x8;
/// `TYPEMASK_PLAYER`.
const PLAYER: u32 = 0x10;

/// Whether `store`'s `OBJECT_FIELD_TYPE` carries `mask`.
fn is(store: &ObjectStore, mask: u32) -> bool {
    store.0.object_type_mask().is_some_and(|m| m & mask != 0)
}

/// The pending list (`0xc0cdd0`, head `0xc0cdd4`), head first. A mark links a GUID the list does
/// not hold at the head (`0x524eab`-`0x524ecd`) and leaves one it holds where it is (`0x524e39`);
/// the drain walks from the head (`0x524cf2`, next at `+0x4`), so the newest mark drains first.
#[derive(Resource, Default, Debug)]
pub(crate) struct PortraitMarks(Vec<u64>);

impl PortraitMarks {
    /// `0x524e10(&guid)`.
    pub(crate) fn mark(&mut self, guid: u64) {
        if !self.0.contains(&guid) {
            self.0.insert(0, guid);
        }
    }

    /// The GUIDs waiting, head first.
    #[cfg(test)]
    pub(super) fn pending(&self) -> &[u64] {
        &self.0
    }

    /// Leaving the world frees every node (`0x524b70`, `0x524c60`-`0x524c89`).
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }
}

/// Mark each unit a producer of `0x524e10` reached since the last frame:
/// - a body built, at a create or a display change (a shapeshift, a polymorph): `0x607da0` marks
///   once the model loads, for the latch the constructor (`0x5faf14`) or `0x60abe0` (`0x60ad2e`)
///   set (`0x607e0f`), and its component build marks too (`0x5fb5d0`). A mount builds no body:
///   its field's handler `0x5ffa50` reaches no mark;
/// - a player's worn look ([`Equipment`]): a visible item (`0x5df119`), the helm and cloak flags
///   (`0x5eea22`), the guild tabard and the designer's preview (`0x5e08c9`, `0x5e081e`);
/// - a player's weapons and their enchants ([`DressKey`], `0x5df119`, `0x5eeda9`), but not a
///   weapon's model finishing its load, nor the body's display, which the build marks.
///
/// The sheath clip's `$SHL`/`$SHR` key (`0x5ffbbe`) marks from `bump_model_revision`, the key's
/// other reader.
#[allow(clippy::type_complexity)]
pub(super) fn mark_portraits(
    mut marks: ResMut<PortraitMarks>,
    built: Query<(&Guid, &ObjectStore), Added<VisualAttached>>,
    worn: Query<(&Guid, &ObjectStore), Changed<Equipment>>,
    armed: Query<(Entity, &Guid, &ObjectStore, &DressKey), Changed<DressKey>>,
    mut dropped: RemovedComponents<DressKey>,
    mut weapons: Local<HashMap<Entity, DressKey>>,
) {
    for (guid, store) in &built {
        if is(store, UNIT) {
            marks.mark(guid.0);
        }
    }
    for (guid, store) in &worn {
        if is(store, PLAYER) {
            marks.mark(guid.0);
        }
    }
    for gone in dropped.read() {
        weapons.remove(&gone);
    }
    for (entity, guid, store, dress) in &armed {
        if !is(store, PLAYER) {
            continue;
        }
        let held = DressKey {
            display_id: None,
            held_ready: true,
            ..*dress
        };
        if weapons.insert(entity, held) != Some(held) {
            marks.mark(guid.0);
        }
    }
}

/// `0x524cd0`, after the frame's script work, as the world frame's render calls it: from the head,
/// each GUID whose unit is held but not ready stays (`0x524d42`-`0x524d8e`), and every other
/// leaves the list and signals `UNIT_PORTRAIT_UPDATE` (0xb8), then `UNIT_MODEL_CHANGED` (0xb9),
/// through the token fan-out. Ready is a body built ([`VisualAttached`]), benilla's one latch for
/// the reference's latches, compositor and model tests. A GUID no unit answers to fires at once
/// (`0x524d40 je 0x524d90`). The pair rides the notice lane, so it fires at the next frame's drain.
pub(super) fn drain_portraits(
    mut marks: ResMut<PortraitMarks>,
    index: Res<GuidIndex>,
    units: Query<(&ObjectStore, Has<VisualAttached>)>,
    mut lane: ResMut<NoticeLane>,
) {
    if marks.0.is_empty() {
        return;
    }
    marks.0.retain(|&guid| {
        // The lookup with type mask 8 (`0x524d37`): a unit, or nothing.
        let loading = index
            .0
            .get(&guid)
            .and_then(|e| units.get(*e).ok())
            .is_some_and(|(store, built)| is(store, UNIT) && !built);
        if !loading {
            lane.write_batch([
                FieldNotice::Signal {
                    guid,
                    id: id::UNIT_PORTRAIT_UPDATE,
                },
                FieldNotice::Signal {
                    guid,
                    id: id::UNIT_MODEL_CHANGED,
                },
            ]);
        }
        loading
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::{EntityKind, ObjectFields};
    use bevy::ecs::system::RunSystemOnce;

    const A: u64 = 0x0000_0000_0000_0011;
    const B: u64 = 0x0000_0000_0000_0012;
    const MOB: u64 = 0xF130_0000_0000_0031;

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<PortraitMarks>()
            .init_resource::<NoticeLane>()
            .init_resource::<GuidIndex>()
            .add_message::<crate::creature_anim::SheathSwapMessage>()
            .add_systems(
                Update,
                (
                    super::super::bump_model_revision,
                    mark_portraits,
                    drain_portraits,
                )
                    .chain(),
            );
        app
    }

    fn unit(app: &mut App, guid: u64, kind: EntityKind) -> Entity {
        let mask = if kind == EntityKind::Player {
            0x19
        } else {
            0x9
        };
        let e = app
            .world_mut()
            .spawn((
                Guid(guid),
                ObjectStore(ObjectFields::from_pairs(&[(2, mask)])),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(guid, e);
        e
    }

    fn notices(app: &mut App) -> Vec<FieldNotice> {
        app.world_mut()
            .resource_mut::<NoticeLane>()
            .drain()
            .collect()
    }

    fn pair(guid: u64) -> [FieldNotice; 2] {
        [
            FieldNotice::Signal {
                guid,
                id: id::UNIT_PORTRAIT_UPDATE,
            },
            FieldNotice::Signal {
                guid,
                id: id::UNIT_MODEL_CHANGED,
            },
        ]
    }

    /// A display change (a shapeshift, a polymorph) rebuilds the body and latches the unit
    /// (`0x60abe0` sets `+0xccc` and `+0xcd0`); once the model loads, `0x607da0` clears the latch
    /// and marks (`0x607e0f`), and the drain signals the pair once. While the body is down the unit
    /// is held but not ready, and nothing fires.
    #[test]
    fn a_display_change_fires_both_events_once_the_new_body_is_built() {
        let mut app = app();
        let e = unit(&mut app, A, EntityKind::Player);
        app.world_mut().entity_mut(e).insert(VisualAttached);
        app.update();
        assert_eq!(notices(&mut app), pair(A), "the first build marks too");
        app.update();
        assert_eq!(notices(&mut app), [], "once");

        // The swap tears the body down; the rebuild lands frames later.
        app.world_mut().entity_mut(e).remove::<VisualAttached>();
        app.update();
        app.update();
        assert_eq!(notices(&mut app), [], "a body still loading fires nothing");
        app.world_mut().entity_mut(e).insert(VisualAttached);
        app.update();
        assert_eq!(notices(&mut app), pair(A));
        app.update();
        assert_eq!(notices(&mut app), []);
    }

    /// The drain walks from the head and a mark links at the head, so the newest unit drains first;
    /// a unit marked again keeps its place. A held unit not ready stays on the list, firing nothing,
    /// while the others drain past it (`0x524df6`); a GUID no unit answers to drains at once
    /// (`0x524d40 je 0x524d90`).
    #[test]
    fn the_drain_runs_newest_first_and_keeps_a_unit_still_loading() {
        let mut app = app();
        let a = unit(&mut app, A, EntityKind::Player);
        unit(&mut app, B, EntityKind::Unit);
        app.world_mut().entity_mut(a).insert(VisualAttached);
        app.update();
        notices(&mut app);

        {
            let mut marks = app.world_mut().resource_mut::<PortraitMarks>();
            marks.mark(A);
            marks.mark(MOB);
            marks.mark(B);
            marks.mark(A);
        }
        app.world_mut()
            .run_system_once(drain_portraits)
            .expect("drain");
        let mut want = Vec::from(pair(MOB));
        want.extend(pair(A));
        assert_eq!(notices(&mut app), want, "B is held and still loading");
        assert_eq!(app.world().resource::<PortraitMarks>().pending(), [B]);
    }

    /// Equipping marks the wearer every time (`0x5e2810` → `0x5dee30` → `0x5df119`), and so do a
    /// helm or cloak toggle (`0x5eea22`) and the guild tabard's reapply (`0x5e08c9`), all of which
    /// move [`Equipment`]; a weapon or its enchant moves [`DressKey`] (`0x5df119`, `0x5eeda9`), and
    /// the sheath clip's `$SHL`/`$SHR` key marks too (`0x5ffbbe`). A weapon's model finishing its
    /// load is no mark.
    #[test]
    fn a_change_of_dress_marks_the_wearer() {
        use crate::entities::ItemModelKind;
        let mut app = app();
        let e = unit(&mut app, A, EntityKind::Player);
        let dress = |held_ready: bool, sword: u32| DressKey {
            display_id: Some(49),
            held: [Some((sword, ItemModelKind::Weapon, 0)), None, None],
            held_ready,
        };
        app.world_mut().entity_mut(e).insert((
            VisualAttached,
            Equipment::default(),
            dress(false, 1234),
        ));
        app.update();
        assert_eq!(notices(&mut app), pair(A));

        app.world_mut().entity_mut(e).insert(dress(true, 1234));
        app.update();
        assert_eq!(notices(&mut app), [], "the sword's model landing");

        app.world_mut().entity_mut(e).insert(dress(true, 5678));
        app.update();
        assert_eq!(notices(&mut app), pair(A), "a new sword");

        app.world_mut().entity_mut(e).insert(Equipment {
            helm: 77,
            ..Default::default()
        });
        app.update();
        assert_eq!(notices(&mut app), pair(A), "a helm");

        app.world_mut()
            .write_message(crate::creature_anim::SheathSwapMessage {
                entity: e,
                slot: 0,
                drawing: true,
            });
        app.update();
        assert_eq!(notices(&mut app), pair(A), "the draw's key");
    }
}
