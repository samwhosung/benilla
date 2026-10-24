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
/// Beside it, the players a portrait asked the name cache for, whose callback `0x51a100` marks
/// each once its record lands (`0x51a10c`).
#[derive(Resource, Default, Debug)]
pub(crate) struct PortraitMarks {
    pending: Vec<u64>,
    asked: Vec<u64>,
}

impl PortraitMarks {
    /// `0x524e10(&guid)`.
    pub(crate) fn mark(&mut self, guid: u64) {
        if !self.pending.contains(&guid) {
            self.pending.insert(0, guid);
        }
    }

    /// `SetPortraitTexture` asked the name cache for `guid` with the callback `0x51a100`.
    pub(crate) fn ask(&mut self, guid: u64) {
        if !self.asked.contains(&guid) {
            self.asked.push(guid);
        }
    }

    /// Whether `guid` waits on the list, which the bake returns on at once (`0x524f96`).
    pub(crate) fn holds(&self, guid: u64) -> bool {
        self.pending.contains(&guid)
    }

    /// The GUIDs waiting, head first.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> &[u64] {
        &self.pending
    }

    /// Leaving the world frees every node (`0x524b70`, `0x524c60`-`0x524c89`), and the name
    /// cache drops its callbacks with its records.
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.asked.clear();
    }
}

/// What [`mark_portraits`] remembers between frames to tell a producer's run from a rerun.
#[derive(Default)]
pub(super) struct Seen {
    /// The display each unit's body was last built on: benilla's palette heal rebuilds on the
    /// same one, which is no display change.
    bodies: HashMap<Entity, Option<u32>>,
    /// A player's weapons, display and load state aside ([`DressKey`]).
    weapons: HashMap<Entity, DressKey>,
    /// Where a player's glowing hand weapons hang ([`crate::entities::HeldItems::glowing_hands`]).
    hands: HashMap<Entity, [Option<u16>; 2]>,
    /// Each item's `ITEM_FIELD_DURABILITY` as its watcher last saw it.
    durability: HashMap<Entity, Option<u32>>,
}

impl Seen {
    fn forget(&mut self, entity: Entity) {
        self.bodies.remove(&entity);
        self.weapons.remove(&entity);
        self.hands.remove(&entity);
        self.durability.remove(&entity);
    }
}

/// The inputs [`mark_portraits`] reads off the world.
#[allow(clippy::type_complexity)]
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct MarkSources<'w, 's> {
    built: Query<
        'w,
        's,
        (
            Entity,
            &'static Guid,
            &'static ObjectStore,
            Option<&'static crate::entities::AppliedDisplay>,
        ),
        Added<VisualAttached>,
    >,
    worn: Query<'w, 's, (&'static Guid, &'static ObjectStore), Changed<Equipment>>,
    armed: Query<
        'w,
        's,
        (
            Entity,
            &'static Guid,
            &'static ObjectStore,
            &'static DressKey,
        ),
        Changed<DressKey>,
    >,
    held: Query<
        'w,
        's,
        (
            Entity,
            &'static Guid,
            &'static ObjectStore,
            &'static crate::entities::HeldItems,
            Has<crate::creature_anim::VisualSheath>,
        ),
        Changed<crate::entities::HeldItems>,
    >,
    items: Query<'w, 's, (Entity, &'static ObjectStore), Changed<ObjectStore>>,
    stores: Query<'w, 's, &'static ObjectStore>,
    index: Res<'w, GuidIndex>,
    names: Res<'w, crate::names::NameCache>,
    gone: RemovedComponents<'w, 's, Guid>,
}

/// `TYPEMASK_ITEM`.
const ITEM: u32 = 0x2;

/// Mark each unit a producer of `0x524e10` reached since the last frame:
/// - a body built, at a create or a display change (a shapeshift, a polymorph): `0x607da0` marks
///   once the model loads, for the latch the constructor (`0x5faf14`) or `0x60abe0` (`0x60ad2e`)
///   set (`0x607e0f`), and its component build marks too (`0x5fb5d0`). A mount builds no body:
///   its field's handler `0x5ffa50` reaches no mark;
/// - a player's worn look ([`Equipment`]): a visible item (`0x5df119`), the helm and cloak flags
///   (`0x5eea22`), the guild tabard and the designer's preview (`0x5e08c9`, `0x5e081e`);
/// - a player's weapons and their enchants ([`DressKey`], `0x5df119`, `0x5eeda9`), but not a
///   weapon's model finishing its load, nor the body's display, which the build marks;
/// - a glowing hand weapon re-attached by a snap sheath (`0x60b96a` → `0x5eed00`, mark at
///   `0x5eeda9`); a ceremony's move is its key's mark;
/// - an item's durability, for its owner when the client holds the owner as a player
///   (`0x5d97f2`, `0x5d981d`, mark at `0x5d991b`);
/// - a name record landing that a portrait asked for (`0x51a10c`).
///
/// The sheath clip's `$SHL`/`$SHR` key (`0x5ffbbe`) marks from `bump_model_revision`, the key's
/// other reader; the party record's pet display (`0x5e587f`) from the notice lane.
pub(super) fn mark_portraits(
    mut marks: ResMut<PortraitMarks>,
    mut world: MarkSources,
    mut seen: Local<Seen>,
) {
    for gone in world.gone.read() {
        seen.forget(gone);
    }
    for (entity, guid, store, display) in &world.built {
        let display = display.and_then(|d| d.0);
        if is(store, UNIT) && seen.bodies.insert(entity, display) != Some(display) {
            marks.mark(guid.0);
        }
    }
    for (guid, store) in &world.worn {
        if is(store, PLAYER) {
            marks.mark(guid.0);
        }
    }
    for (entity, guid, store, dress) in &world.armed {
        if !is(store, PLAYER) {
            continue;
        }
        let held = DressKey {
            display_id: None,
            held_ready: true,
            ..*dress
        };
        if seen.weapons.insert(entity, held) != Some(held) {
            marks.mark(guid.0);
        }
    }
    for (entity, guid, store, held, ceremony) in &world.held {
        let now = held.glowing_hands();
        let was = seen.hands.insert(entity, now).unwrap_or([None; 2]);
        let moved = now.iter().zip(was).any(|(n, w)| n.is_some() && *n != w);
        if is(store, PLAYER) && moved && !ceremony {
            marks.mark(guid.0);
        }
    }
    for (entity, store) in &world.items {
        if !is(store, ITEM) {
            continue;
        }
        let now = store.0.item_durability();
        // A first create runs no watcher (`0x465d80`).
        let Some(was) = seen.durability.insert(entity, now) else {
            continue;
        };
        let owner = store.0.item_owner().unwrap_or(0);
        let held = world
            .index
            .0
            .get(&owner)
            .and_then(|e| world.stores.get(*e).ok())
            .is_some_and(|s| is(s, PLAYER));
        if was != now && held {
            marks.mark(owner);
        }
    }
    if !marks.asked.is_empty() {
        let names = &world.names;
        let landed: Vec<u64> = marks
            .asked
            .iter()
            .copied()
            .filter(|g| names.player_traits(*g).is_some())
            .collect();
        marks.asked.retain(|g| !landed.contains(g));
        for guid in landed {
            marks.mark(guid);
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
    if marks.pending.is_empty() {
        return;
    }
    marks.pending.retain(|&guid| {
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
            .init_resource::<crate::names::NameCache>()
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
        built(&mut app, e, 49);
        app.update();
        assert_eq!(notices(&mut app), pair(A), "the first build marks too");
        app.update();
        assert_eq!(notices(&mut app), [], "once");

        // The swap tears the body down; the rebuild lands frames later.
        torn_down(&mut app, e);
        app.update();
        app.update();
        assert_eq!(notices(&mut app), [], "a body still loading fires nothing");
        built(&mut app, e, 50);
        app.update();
        assert_eq!(notices(&mut app), pair(A));
        app.update();
        assert_eq!(notices(&mut app), []);
    }

    fn built(app: &mut App, unit: Entity, display: u32) {
        app.world_mut().entity_mut(unit).insert((
            VisualAttached,
            crate::entities::AppliedDisplay(Some(display)),
        ));
    }

    fn torn_down(app: &mut App, unit: Entity) {
        app.world_mut()
            .entity_mut(unit)
            .remove::<(VisualAttached, crate::entities::AppliedDisplay)>();
    }

    /// benilla's palette heal rebuilds a body on the display it had, which is no display change
    /// and runs no `0x607da0` latch: no mark.
    #[test]
    fn a_body_rebuilt_on_its_own_display_marks_nothing() {
        let mut app = app();
        let e = unit(&mut app, A, EntityKind::Player);
        built(&mut app, e, 49);
        app.update();
        notices(&mut app);
        torn_down(&mut app, e);
        app.update();
        built(&mut app, e, 49);
        app.update();
        app.update();
        assert_eq!(notices(&mut app), []);
    }

    /// Mounting moves neither the body nor its latches: the mount field's handler `0x5ffa50`
    /// reaches no mark.
    #[test]
    fn mounting_marks_nothing() {
        let mut app = app();
        let e = unit(&mut app, A, EntityKind::Player);
        built(&mut app, e, 49);
        app.update();
        notices(&mut app);
        let mounted = ObjectFields::from_pairs(&[
            (2, 0x19),
            (benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID, 14_338),
        ]);
        app.world_mut().entity_mut(e).insert(ObjectStore(mounted));
        app.update();
        app.update();
        assert_eq!(notices(&mut app), []);
    }

    /// The name callback `0x51a100` marks a player the portrait asked the name cache for once the
    /// record lands (`0x51a10c`), and the drain fires the pair for a GUID no unit answers to.
    #[test]
    fn a_name_record_landing_for_a_portrait_marks_its_player() {
        let mut app = app();
        app.world_mut().resource_mut::<PortraitMarks>().ask(B);
        app.update();
        assert_eq!(notices(&mut app), [], "no record yet");
        app.world_mut()
            .resource_mut::<crate::names::NameCache>()
            .insert_player(B, "Brisca".into(), Some((4, 1, 1)));
        app.update();
        assert_eq!(notices(&mut app), pair(B));
        app.update();
        assert_eq!(notices(&mut app), [], "the callback runs once");
    }

    /// The durability watcher `0x5d97c0` marks the item's owner when the item and its owner, a
    /// player, are both held (`0x5d97f2`, `0x5d981d`, mark at `0x5d991b`), whatever the value; a
    /// first create runs no watcher.
    #[test]
    fn a_durability_change_marks_the_items_owner() {
        let mut app = app();
        let e = unit(&mut app, A, EntityKind::Player);
        built(&mut app, e, 49);
        let item = |durability: u32, owner: u64| {
            ObjectStore(ObjectFields::from_pairs(&[
                (2, 0x3),
                (6, owner as u32),
                (7, (owner >> 32) as u32),
                (46, durability),
                (47, 100),
            ]))
        };
        const SWORD: u64 = 0x4000_0000_0000_0099;
        let sword = app.world_mut().spawn((Guid(SWORD), item(100, A))).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(SWORD, sword);
        app.update();
        notices(&mut app);
        app.world_mut().entity_mut(sword).insert(item(99, A));
        app.update();
        assert_eq!(notices(&mut app), pair(A), "worn by a hit");
        app.world_mut().entity_mut(sword).insert(item(100, A));
        app.update();
        assert_eq!(notices(&mut app), pair(A), "a repair");
        app.world_mut().entity_mut(sword).insert(item(80, MOB));
        app.update();
        assert_eq!(
            notices(&mut app),
            [],
            "an owner the client does not hold as a player"
        );
    }

    /// A snap sheath of a weapon with a glow re-attaches it through `0x60b770` → `0x5eed00`, whose
    /// `0x5eed50` re-applies the glow and marks (`0x5eeda9`); a weapon with none does not, and a
    /// ceremony's move is its key's mark (`0x5ffbbe`).
    #[test]
    fn a_glowing_weapon_snapped_to_another_point_marks_its_wielder() {
        use crate::entities::attach_id::{HAND_RIGHT, HIP_MAIN};
        use crate::entities::HeldItems;
        let mut app = app();
        let e = unit(&mut app, A, EntityKind::Player);
        built(&mut app, e, 49);
        app.world_mut()
            .entity_mut(e)
            .insert(HeldItems::hands_for_tests([Some((HIP_MAIN, 0)), None]));
        app.update();
        notices(&mut app);
        app.world_mut()
            .entity_mut(e)
            .insert(HeldItems::hands_for_tests([Some((HAND_RIGHT, 0)), None]));
        app.update();
        assert_eq!(notices(&mut app), [], "no glow");

        app.world_mut()
            .entity_mut(e)
            .insert(HeldItems::hands_for_tests([Some((HAND_RIGHT, 172)), None]));
        app.update();
        notices(&mut app);
        app.world_mut()
            .entity_mut(e)
            .insert(HeldItems::hands_for_tests([Some((HIP_MAIN, 172)), None]));
        app.update();
        assert_eq!(notices(&mut app), pair(A), "the glow moved with the snap");
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
