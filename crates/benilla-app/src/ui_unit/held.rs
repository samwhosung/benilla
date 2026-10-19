//! The snapshot of a held unit that a token names other than the player, and the per-guid feed of
//! the units a `target` chain ends on.
//!
//! The reference resolves a token to a guid (`0x515970`) and reads that guid's object, so a unit it
//! holds answers a `Unit*` getter however the token spelled it. The app pushes a snapshot
//! for each base token by name ([`UiScript::set_unit`]); a chain such as `party1target` names a
//! unit no base does, so the VM resolves it to a guid and reads that guid's snapshot, which
//! [`feed_chain_units`] pushes for the guids [`UiScript::chain_end_guids`] lists.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use benilla_ui::script::{NameCandidate, UiScript, UnitState};

use super::{
    enrich_unit, faction_group, faction_group_localized, player_reaction, snapshot, unit_reaction,
};
use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::target::{Candidacy, NameSearch, RelUnit, Relations};
use crate::ui_script::{gate, VmMemo};

/// What a held unit's snapshot reads beside its descriptor.
pub(super) struct HeldUnits<'a> {
    pub(super) names: &'a NameCache,
    pub(super) commands: &'a NetCommands,
    pub(super) group: &'a crate::ui_party::GroupState,
    /// Our own descriptor, which the faction line reads.
    pub(super) self_store: Option<&'a ObjectStore>,
    pub(super) unit_names: super::UnitNames<'a>,
    pub(super) types: CreatureTypeSources<'a>,
    /// The relation functions, over the object manager, and the active player they judge from.
    pub(super) rel: Relations<'a>,
    pub(super) me: Option<RelUnit>,
}

impl HeldUnits<'_> {
    /// The snapshot of the held unit `guid`, whose descriptor is `store`, as a token naming a unit
    /// other than the player carries it. It has no guild leg, so `GetGuildInfo` on a chain answers
    /// nothing, where the reference resolves the token and reads the object's guild (`0x4c9330`).
    pub(super) fn state(&self, store: &ObjectStore, guid: u64) -> UnitState {
        // The unit's create looked its name up with a callback (`0x5dec12`); a frame asks only
        // a name the cache holds nothing for, as one invalidated since, and a refused one waits
        // for the next create.
        let name = self
            .names
            .resolve_unit(guid, Some(store), Ask::New(self.commands))
            .map(str::to_string);
        self.state_named(store, guid, name)
    }

    /// [`Self::state`] under a name the caller looked up, which sends no name query.
    fn state_named(&self, store: &ObjectStore, guid: u64, name: Option<String>) -> UnitState {
        let unit = RelUnit::of(guid, store).with_record(store, self.names);
        let factions = self.rel.factions;
        let reaction = unit_reaction(&self.rel, &unit, self.me.as_ref());
        let mut s = snapshot(store, guid, name, reaction, self.unit_names, self.types);
        s.player_reaction = player_reaction(&self.rel, &unit, self.me.as_ref());
        s.raid_target = self.group.raid_target_index(guid);
        s.faction_group = faction_group(store, factions);
        s.faction_group_localized = faction_group_localized(store, factions);
        enrich_unit(&mut s, guid, self.names, store, factions, self.self_store);
        s
    }
}

/// [`feed_chain_units`]' per-VM memory.
#[derive(Default)]
pub(super) struct ChainMemo {
    names_generation: gate::Watch,
    /// The guids the last build covered, sorted.
    ends: Vec<u64>,
    /// What each was last pushed as.
    pushed: HashMap<u64, UnitState>,
}

/// Push the snapshot of each unit a `target` chain can end on, by guid, so `UnitName`,
/// `UnitHealth` and the other getters answer for `party1target`, `raid3target` and `pettarget`
/// (`0x515970` resolves the token, then each getter reads the guid's object, `0x468460`). The
/// guids are the resolver's own inputs ([`UiScript::chain_end_guids`]), which the aura feed pushes
/// each frame, so this runs after it. A held unit's snapshot reads its descriptor. A guid the
/// object manager does not hold gets a no-object entry: its name from the name cache, asked on a
/// miss as `UnitName`'s read asks it (`0x5171eb`, `0x55f080` on `0xc0e228`, else `UNKNOWNOBJECT`
/// at `0x517216`), here when the end appears rather than when a script reads it, and its raid mark, which `GetRaidTargetIndex` reads by guid (`0x4bb4f1`); the
/// getters that need an object answer it nil. The VM reads a group row's own push over it, which
/// carries the records and the roster's `UnitExists` (`0x496400`, `0x496420`, `0x491900`). A guid
/// held as anything but a unit gets no snapshot. A token without a hop reads its own push, not
/// these.
pub(super) fn feed_chain_units(
    script: Option<NonSendMut<UiScript>>,
    tables: super::SnapshotTables,
    // The object manager's lookup (`0x468460`) on its own, not through [`super::UnitTokens`],
    // whose pet-bar read would order this feed against the pet feed it does not depend on.
    index: Option<Res<crate::net::GuidIndex>>,
    stores: Query<&ObjectStore>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    world: crate::target::RelationWorld,
    group: Res<crate::ui_party::GroupState>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    changed: Query<(), (Changed<ObjectStore>, Without<crate::items::ItemObject>)>,
    mut removed: RemovedComponents<Guid>,
    mut memo: Local<VmMemo<ChainMemo>>,
) {
    let Some(mut script) = script else {
        return;
    };
    // The names `UnitName` read off a unit with no object and missed, asked as its lookup with a
    // callback asks (`0x5171eb`), whatever the gate below says.
    for guid in script.take_unit_name_asks() {
        let _ = names.resolve_player(guid, Ask::Event(&commands));
    }
    let (memo, vm_reset) = memo.get_reset(&script);
    let ends = script.chain_end_guids();

    // The gate: every input read below, despawns included (invisible to `Changed`), and the
    // guids themselves, which move with the selection, the hover, the group and every held
    // unit's target.
    let names_moved = memo.names_generation.moved(names.generation());
    let ends_moved = memo.ends != ends;
    let stores_changed = !changed.is_empty();
    let stores_removed = !removed.is_empty();
    let group_changed = group.is_changed();
    let relations_changed = world.inputs_changed();
    removed.clear();
    let gate = gate::Gate::new(
        vm_reset
            || names_moved
            || ends_moved
            || stores_changed
            || stores_removed
            || group_changed
            || relations_changed,
    );
    if gate.skip() {
        return;
    }

    let held = HeldUnits {
        names: &names,
        commands: &commands,
        group: &group,
        self_store: self_q.iter().next(),
        unit_names: tables.unit_names(),
        types: tables.types(&names),
        rel: world.get(),
        me: world.me(),
    };
    let built: HashMap<u64, UnitState> = ends
        .iter()
        .filter_map(|&guid| {
            let Some(&entity) = index.as_ref().and_then(|i| i.0.get(&guid)) else {
                return Some((guid, unheld_state(&names, &group, guid)));
            };
            let store = stores.get(entity).ok().filter(|s| s.is_unit())?;
            Some((guid, held.state(store, guid)))
        })
        .collect();

    memo.pushed.retain(|guid, _| {
        let keep = built.contains_key(guid);
        if !keep {
            gate.audit("feed_chain_units", "a chain end's clear");
            script.set_unit_by_guid(*guid, None);
        }
        keep
    });
    for (guid, state) in built {
        if memo.pushed.get(&guid) != Some(&state) {
            gate.audit("feed_chain_units", "a chain end's snapshot");
            script.set_unit_by_guid(guid, Some(state.clone()));
            memo.pushed.insert(guid, state);
        }
    }
    memo.ends = ends;
}

/// A chain end the object manager does not hold, as the getters read a guid with no object: the
/// player-name cache by guid whatever the guid is (`0x5171eb`), which vmangos answers only under a
/// player's guid (`QueryHandler.cpp:48-60`), so a creature or a pet reads `UNKNOWNOBJECT`; the
/// raid mark by guid (`0x4bb4f1`);
/// nothing else. Off the roster it does not exist (`0x491900`); on it, the VM reads the row's push.
/// The name is read without asking: the lookup asks when `UnitName` reads it, and the VM hands
/// the miss back ([`UiScript::take_unit_name_asks`]).
fn unheld_state(names: &NameCache, group: &crate::ui_party::GroupState, guid: u64) -> UnitState {
    UnitState {
        guid,
        name: names.resolve_player(guid, Ask::Read).map(str::to_string),
        raid_target: group.raid_target_index(guid),
        ..Default::default()
    }
}

/// [`feed_name_candidates`]' per-VM memory.
#[derive(Default)]
pub(crate) struct CandidateMemo {
    /// What each pushed candidate was built from: the name it was pushed under, and whether its
    /// entry's creature record had answered. A creature's name is its record, but a pet's name
    /// and its record (the creature type the snapshot reads) come back apart.
    pushed: HashMap<u64, (String, bool)>,
    /// This frame's candidates, kept between frames so the walk allocates nothing.
    seen: HashSet<u64>,
}

/// Push every unit `TargetByName` can pick, by guid, so the binding resolves its name against
/// them and shows the pick as `"target"` to the rest of its chunk, as `0x489d60` commits before it
/// returns: the by-name scan's candidates (typemask 8, a cached name, no query sent) with their
/// distance each frame, and each one's snapshot when it is new, its descriptor, name or creature
/// record moved, or an input every snapshot reads did; another name's answer rebuilds nothing.
/// The snapshot has no guild leg, as a chain end's; it is a view for the rest of a chunk, which
/// [`super::feed_units`] replaces with the committed selection's next frame.
/// A move of our own descriptor does not rebuild the rest, so a reaction can lag our own faction
/// change until the unit's next update. Without an active player object nothing is a candidate,
/// as the reference resolves nothing then (`0x493ae0`).
pub(crate) fn feed_name_candidates(
    script: Option<NonSendMut<UiScript>>,
    tables: super::SnapshotTables,
    // [`GlobalTransform`], as [`crate::target::PlayerLookup`] reads it: `Transform` would put
    // this feed in an undeclared order against every system that moves a unit.
    units: Query<(&Guid, &GlobalTransform, Option<Ref<ObjectStore>>)>,
    self_q: Query<(&Guid, &GlobalTransform, Option<&ObjectStore>), With<SelfPlayer>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    world: crate::target::RelationWorld,
    group: Res<crate::ui_party::GroupState>,
    mut memo: Local<VmMemo<CandidateMemo>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (memo, vm_reset) = memo.get_reset(&script);
    let rebuild = vm_reset || group.is_changed() || world.inputs_changed();
    let me = self_q.single().ok();
    let held = HeldUnits {
        names: &names,
        commands: &commands,
        group: &group,
        self_store: me.and_then(|(_, _, s)| s),
        unit_names: tables.unit_names(),
        types: tables.types(&names),
        rel: world.get(),
        me: world.me(),
    };
    memo.seen.clear();
    let origin = me.map(|(_, tf, _)| tf.translation());
    let self_guid = me.map(|(g, _, _)| g.0);
    for (guid, tf, store) in origin.iter().flat_map(|_| &units) {
        let guid = guid.0;
        let Candidacy::Named(name) = NameSearch::AnyUnit.candidacy(&names, guid) else {
            continue;
        };
        memo.seen.insert(guid);
        let dist2 = origin.map_or(0.0, |o| tf.translation().distance_squared(o));
        let record = store
            .as_ref()
            .and_then(|s| s.0.object_entry())
            .filter(|&e| e != 0)
            .is_some_and(|e| names.creature_record(e).is_some());
        let fresh = rebuild
            || store.as_ref().is_some_and(|s| s.is_changed())
            || memo
                .pushed
                .get(&guid)
                .is_none_or(|(n, r)| n != name || *r != record);
        if !fresh {
            script.set_name_candidate_dist2(guid, dist2);
            continue;
        }
        let state = match store.as_deref() {
            // The name as the target feed reads it off the descriptor, from the cache alone.
            Some(store) => {
                let named = names
                    .resolve_unit(guid, Some(store), Ask::Read)
                    .map(str::to_string);
                held.state_named(store, guid, named)
            }
            None => UnitState {
                exists: true,
                has_object: true,
                is_connected: true,
                guid,
                name: Some(name.to_string()),
                is_player: benilla_protocol::guid::is_player(guid),
                ..Default::default()
            },
        };
        let unit = store
            .as_deref()
            .map(|s| (RelUnit::of(guid, s).with_record(s, &names), s));
        script.set_name_candidate(
            guid,
            Some(NameCandidate {
                name: name.to_string(),
                dist2,
                state,
                selectable: crate::target::is_selectable(store.as_deref(), self_guid),
                assistable_alive: crate::target::assistable_alive(
                    &held.rel,
                    held.me.as_ref(),
                    unit.as_ref().map(|(u, s)| (u, *s)),
                ),
            }),
        );
        memo.pushed.insert(guid, (name.to_string(), record));
    }
    let seen = &memo.seen;
    memo.pushed.retain(|guid, _| {
        let keep = seen.contains(guid);
        if !keep {
            script.set_name_candidate(*guid, None);
        }
        keep
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::CreatureRecord;
    use crate::net::{GuidIndex, Reputations};
    use crate::target::Selection;
    use crate::ui_party::{GroupState, GROUPTYPE_RAID};
    use benilla_protocol::messages::{member_status, GroupMemberEntry};
    use benilla_protocol::ObjectFields;

    const ME: u64 = 0x10;
    /// party1 and raid1, party2 and raid2, party3 and raid3; we are raid4.
    const A: u64 = 0x1001;
    const B: u64 = 0x1002;
    const C: u64 = 0x1003;
    /// Our target, which targets party1.
    const MOB: u64 = 0xF130_0000_4500_0001;
    /// What party1 targets.
    const BOSS: u64 = 0xF130_0000_4600_0001;
    /// What raid2 targets, and a unit that targets nobody.
    const ADD: u64 = 0xF130_0000_4700_0001;
    const MY_PET: u64 = 0xF140_0000_0000_0010;
    /// What our pet targets.
    const FOE: u64 = 0xF130_0000_4800_0001;
    /// What `BOSS` targets, and nobody holds.
    const GONE: u64 = 0xF130_0000_4900_0001;
    /// Held from the start, and named by no token until it is selected.
    const LONER: u64 = 0xF130_0000_4A00_0001;
    /// What `LONER` targets.
    const EXTRA: u64 = 0xF130_0000_4B00_0001;

    /// `OBJECT_FIELD_TYPE`, `UNIT_FIELD_TARGET`, `_HEALTH`, `_MAXHEALTH` and `_LEVEL`.
    const OBJECT_TYPE: u16 = 2;
    const TARGET: u16 = 16;
    const HEALTH: u16 = 22;
    const MAXHEALTH: u16 = 28;
    const LEVEL: u16 = 34;

    fn unit(kind: u32, target: u64, health: u32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[
            (OBJECT_TYPE, kind),
            (TARGET, target as u32),
            (TARGET + 1, (target >> 32) as u32),
            (HEALTH, health),
            (MAXHEALTH, 5000),
            (LEVEL, 60),
        ]))
    }

    fn player(target: u64, health: u32) -> ObjectStore {
        unit(0x19, target, health)
    }

    fn creature(target: u64, health: u32) -> ObjectStore {
        unit(0x09, target, health)
    }

    /// Streams `guid` in. A creature's descriptor carries its `OBJECT_FIELD_ENTRY` (field 3), the
    /// key its name and record are cached under; these guids spell it in their entry bits.
    fn spawn(app: &mut App, guid: u64, mut store: ObjectStore) -> Entity {
        if let Some(entry) = benilla_protocol::guid::entry(guid) {
            store.0.merge(ObjectFields::from_pairs(&[(3, entry)]));
        }
        let e = app.world_mut().spawn((Guid(guid), store)).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(guid, e);
        e
    }

    fn group_entry(guid: u64) -> GroupMemberEntry {
        GroupMemberEntry {
            name: format!("M{guid:x}"),
            guid,
            status: member_status::ONLINE,
            flags: 0,
        }
    }

    /// The world: we target `MOB`, which targets party1 (`A`), who targets `BOSS`, which targets
    /// `GONE`; raid2 (`B`) targets `ADD`, our pet `FOE`, and `C` nobody. `LONER` is held and
    /// targets `EXTRA`. The group is a raid of us, `A`, `B` and `C`, all in our subgroup, so the
    /// three are also party slots 1 to 3. The aura feed, whose resolver inputs the chain feed
    /// reads, and the chain feed run.
    fn app() -> App {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(crate::client_state::ClientState::InWorld)
            .init_resource::<Selection>()
            .init_resource::<GuidIndex>()
            .init_resource::<Reputations>()
            .init_resource::<NameCache>()
            .init_resource::<GroupState>()
            .insert_resource(NetCommands(tx))
            .add_plugins(crate::ui_aura::UiAuraPlugin)
            .add_systems(Update, feed_chain_units.after(crate::ui_aura::AuraEvents));
        app.insert_non_send_resource(UiScript::new().unwrap());
        {
            let mut names = app.world_mut().resource_mut::<NameCache>();
            for (guid, name) in [(ME, "Meg"), (A, "Alia"), (B, "Bram"), (C, "Cole")] {
                names.insert_player(guid, name.to_string(), None);
            }
            for (guid, name) in [
                (MOB, "Mob"),
                (BOSS, "Boss"),
                (ADD, "Add"),
                (FOE, "Foe"),
                (LONER, "Loner"),
                (EXTRA, "Extra"),
            ] {
                names.insert_creature(
                    benilla_protocol::guid::entry(guid).unwrap(),
                    Some(CreatureRecord {
                        name: name.to_string(),
                        subname: None,
                        creature_type: 0,
                        pet_family: 0,
                        rank: 0,
                        type_flags: 0,
                        civilian: false,
                        racial_leader: false,
                        display_id: 0,
                    }),
                );
            }
        }
        let me = spawn(&mut app, ME, player(MOB, 900));
        app.world_mut().entity_mut(me).insert(SelfPlayer);
        spawn(&mut app, A, player(BOSS, 800));
        spawn(&mut app, B, player(ADD, 700));
        spawn(&mut app, C, player(0, 600));
        let mob = spawn(&mut app, MOB, creature(A, 5000));
        spawn(&mut app, BOSS, creature(GONE, 4000));
        spawn(&mut app, ADD, creature(0, 250));
        spawn(&mut app, MY_PET, creature(FOE, 300));
        spawn(&mut app, FOE, creature(0, 120));
        spawn(&mut app, LONER, creature(EXTRA, 90));
        spawn(&mut app, EXTRA, creature(0, 45));
        app.world_mut().resource_mut::<GroupState>().apply_list(
            GROUPTYPE_RAID,
            0,
            vec![group_entry(A), group_entry(B), group_entry(C)],
            ME,
            None,
            Some(ME),
        );
        app.insert_resource(Selection {
            target: Some(mob),
            guid: Some(MOB),
            ..Default::default()
        });
        let mut bar = crate::ui_pet::PetBar::default();
        bar.spells.pet_guid = MY_PET;
        app.insert_resource(bar);
        app
    }

    fn script(app: &App) -> &UiScript {
        app.world().non_send_resource::<UiScript>()
    }

    fn name(app: &App, token: &str) -> Option<String> {
        script(app)
            .eval(&format!("return UnitName({token:?})"))
            .unwrap()
    }

    fn number(app: &App, call: &str, token: &str) -> i64 {
        script(app)
            .eval(&format!("return {call}({token:?})"))
            .unwrap()
    }

    fn health(app: &App, token: &str) -> i64 {
        number(app, "UnitHealth", token)
    }

    /// Whether the Lua expression holds.
    fn holds(app: &App, expr: &str) -> bool {
        script(app).eval(&format!("return {expr}")).unwrap()
    }

    /// The getters answer for a chain through the feeds that run in the client: the aura feed's
    /// resolver inputs name the guid, and this feed's snapshot of it answers, so `party1target`,
    /// `raid2target` and `pettarget` read the unit at the end of the chain and any break in the
    /// chain reads nobody. No token names `ADD` or `FOE` but a chain.
    #[test]
    fn the_getters_answer_for_the_unit_a_chain_ends_on() {
        let mut app = app();
        app.update();
        for (token, want, hp) in [
            ("party1target", "Boss", 4000),
            ("raid1target", "Boss", 4000),
            ("PARTY1TARGET", "Boss", 4000),
            ("raid2target", "Add", 250),
            ("pettarget", "Foe", 120),
            ("playertarget", "Mob", 5000),
            ("targettarget", "Alia", 800),
            ("targettargettarget", "Boss", 4000),
        ] {
            assert_eq!(
                name(&app, token).as_deref(),
                Some(want),
                "UnitName({token})"
            );
            assert_eq!(health(&app, token), hp, "UnitHealth({token})");
            assert_eq!(number(&app, "UnitHealthMax", token), 5000, "{token}");
            assert_eq!(number(&app, "UnitLevel", token), 60, "{token}");
            assert!(
                holds(&app, &format!("UnitExists({token:?}) == 1")),
                "UnitExists({token})"
            );
        }
        assert!(holds(
            &app,
            r#"UnitIsUnit("raid1target", "party1target") == 1"#
        ));
        assert!(holds(
            &app,
            r#"UnitIsUnit("raid1target", "raid2target") == nil"#
        ));
        // `BOSS` targets `GONE`, which nobody holds: no record and off the roster, so 0 and nil
        // ([`an_unheld_chain_end_answers_what_the_client_knows_of_it`] has its name).
        assert_eq!(health(&app, "party1targettarget"), 0);
        assert!(holds(&app, r#"UnitExists("party1targettarget") == nil"#));
        // `C` targets nobody; party4 is nobody.
        for token in ["raid3target", "party4target", "raid9target"] {
            assert_eq!(name(&app, token), None, "UnitName({token})");
            assert_eq!(health(&app, token), 0, "UnitHealth({token})");
            assert!(
                holds(&app, &format!("UnitExists({token:?}) == nil")),
                "UnitExists({token})"
            );
        }
        // The plain tokens read their own push and nothing else: this world pushes none, so
        // `raid1` answers nil though a chain ends on it, and a token pushed by name wins.
        assert_eq!(name(&app, "raid1"), None);
        assert_eq!(name(&app, "target"), None);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_unit(
                "target",
                Some(UnitState {
                    exists: true,
                    name: Some("Pushed".into()),
                    ..Default::default()
                }),
            );
        assert_eq!(name(&app, "target").as_deref(), Some("Pushed"));
    }

    /// A chain ending on a guid nobody holds answers what the client knows of it, each getter
    /// by its no-object leg: `UnitName` the name cache by guid, which asks a miss as it reads it,
    /// once while pending, and answers `UNKNOWNOBJECT` (`0x5171eb`, `0x517216`);
    /// `GetRaidTargetIndex` the mark by guid (`0x4bb4f1`); a group member or its pet out of view their records and the roster's
    /// `UnitExists` (`0x496400`, `0x496420`, `0x491900`), as their own tokens read them; and nil
    /// from each getter that needs the object (`0x515ff5`, `0x516150`, `0x519da8`, `0x517310`).
    #[test]
    fn an_unheld_chain_end_answers_what_the_client_knows_of_it() {
        /// A player nobody holds, whose name the cache holds.
        const STRANGER: u64 = 0x2001;
        /// Party slot 4 and raid row 4, out of view, and the pet its record names.
        const D: u64 = 0x1004;
        const D_PET: u64 = 0xF140_0000_0000_1004;
        let mut app = app();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<crate::ui_chat::ChatLog>();
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        app.add_systems(Update, crate::ui_party::feed_party);
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.run(r#"UNKNOWNOBJECT = "Unknown""#).unwrap();
            // Level 60, so the level 0 of a unit with no object would read grey.
            script.set_player_req_state(benilla_ui::script::PlayerReqState {
                level: 60,
                ..Default::default()
            });
        }
        app.world_mut().resource_mut::<NameCache>().insert_player(
            STRANGER,
            "Wanderer".into(),
            None,
        );
        let asked = |rx: &crossbeam_channel::Receiver<crate::net::ClientCommand>| -> Vec<u64> {
            rx.try_iter()
                .filter_map(|c| match c {
                    crate::net::ClientCommand::NameQuery { guid } => Some(guid),
                    _ => None,
                })
                .collect()
        };
        let retarget_boss = |app: &mut App, to: u64| {
            let boss = app.world().resource::<GuidIndex>().0[&BOSS];
            app.world_mut().entity_mut(boss).insert(creature(to, 4000));
            app.update();
        };
        let chain = "party1targettarget";
        let nil = |app: &App, getters: &[&str]| {
            for getter in getters {
                assert!(
                    holds(app, &format!("{getter}({chain:?}) == nil")),
                    "{getter}({chain})"
                );
            }
        };
        app.update();

        // `GONE`, a creature: the player-name cache by its guid, which no answer fills, asked
        // when `UnitName` reads it and not before.
        assert_eq!(asked(&rx), Vec::<u64>::new(), "nothing has read the name");
        assert_eq!(name(&app, chain).as_deref(), Some("Unknown"));
        app.update();
        assert_eq!(asked(&rx), vec![GONE], "the read's miss is asked");
        assert_eq!(name(&app, chain).as_deref(), Some("Unknown"));
        assert_eq!(health(&app, chain), 0);
        nil(
            &app,
            &[
                "UnitExists",
                "UnitIsVisible",
                "UnitIsPlayer",
                "UnitIsTrivial",
                "UnitPVPName",
                "UnitIsConnected",
                "GetRaidTargetIndex",
            ],
        );
        // Marked, by guid alone; the rebuild asks the pending name no more.
        app.world_mut().resource_mut::<GroupState>().raid_targets[2] = GONE;
        app.update();
        assert_eq!(number(&app, "GetRaidTargetIndex", chain), 3);
        assert_eq!(
            asked(&rx),
            Vec::<u64>::new(),
            "a pending name is asked once"
        );

        // A player out of view: the cached name, and still no unit.
        retarget_boss(&mut app, STRANGER);
        assert_eq!(name(&app, chain).as_deref(), Some("Wanderer"));
        assert_eq!(asked(&rx), Vec::<u64>::new(), "a cached name is not asked");
        nil(&app, &["UnitExists", "UnitIsPlayer", "UnitPVPName"]);

        // A party member out of view: what `party4` reads, off the record.
        let mut entries: Vec<GroupMemberEntry> =
            [A, B, C, D].into_iter().map(group_entry).collect();
        entries[3].name = "Dara".into();
        {
            let mut group = app.world_mut().resource_mut::<GroupState>();
            group.apply_list(GROUPTYPE_RAID, 0, entries, ME, None, Some(ME));
            group.apply_stats(
                D,
                true,
                benilla_protocol::messages::PartyMemberStatsInfo {
                    status: Some(member_status::ONLINE),
                    cur_hp: Some(1234),
                    max_hp: Some(2000),
                    level: Some(41),
                    pet_guid: Some(D_PET),
                    pet_name: Some("Grub".into()),
                    pet_cur_hp: Some(77),
                    pet_max_hp: Some(99),
                    ..Default::default()
                },
            );
        }
        retarget_boss(&mut app, D);
        assert!(holds(&app, &format!("UnitExists({chain:?}) == 1")));
        assert!(holds(&app, &format!("UnitIsPlayer({chain:?}) == 1")));
        assert!(holds(&app, &format!("UnitIsConnected({chain:?}) == 1")));
        assert_eq!(name(&app, chain).as_deref(), Some("Dara"));
        assert_eq!(health(&app, chain), 1234);
        assert_eq!(number(&app, "UnitHealthMax", chain), 2000);
        assert_eq!(number(&app, "UnitLevel", chain), 41);
        nil(&app, &["UnitIsVisible", "UnitPVPName"]);
        assert_eq!(name(&app, chain), name(&app, "party4"));

        // Its pet: the pet record, and on the roster while the owner's record is online.
        retarget_boss(&mut app, D_PET);
        assert!(holds(&app, &format!("UnitExists({chain:?}) == 1")));
        assert_eq!(name(&app, chain).as_deref(), Some("Grub"));
        assert_eq!(health(&app, chain), 77);
        assert_eq!(number(&app, "UnitHealthMax", chain), 99);
        nil(&app, &["UnitIsVisible", "UnitIsPlayer"]);
    }

    /// Each input of the feed's gate reaches the getters: a descriptor that moves, a chain that
    /// gains an end with no descriptor moving, and a unit that leaves the object manager.
    #[test]
    fn the_feed_follows_the_world() {
        let mut app = app();
        app.update();
        assert_eq!(health(&app, "party1target"), 4000);

        // A descriptor moves.
        let boss = app.world().resource::<GuidIndex>().0[&BOSS];
        app.world_mut()
            .entity_mut(boss)
            .insert(creature(GONE, 1000));
        app.update();
        assert_eq!(health(&app, "party1target"), 1000);

        // Selecting `LONER` gives `targettarget` a unit that no chain ended on, and moves no
        // descriptor and no name.
        assert_eq!(name(&app, "targettarget").as_deref(), Some("Alia"));
        let loner = app.world().resource::<GuidIndex>().0[&LONER];
        app.insert_resource(Selection {
            target: Some(loner),
            guid: Some(LONER),
            ..Default::default()
        });
        app.update();
        assert_eq!(name(&app, "targettarget").as_deref(), Some("Extra"));
        assert_eq!(health(&app, "targettarget"), 45);

        // `ADD` leaves the object manager, and `raid2target` with it. The chain is still
        // `raid2`'s target, so the resolver's guid holds; the snapshot is what goes.
        assert_eq!(name(&app, "raid2target").as_deref(), Some("Add"));
        let add = app.world_mut().resource_mut::<GuidIndex>().0.remove(&ADD);
        app.world_mut().despawn(add.unwrap());
        app.update();
        // The name would be the name cache's (`0x5171eb`), which a chain's end does not read yet.
        assert_eq!(health(&app, "raid2target"), 0);
    }

    /// `UnitCanAssist` (`0x516bb0`) over a chain's end asks `CanAssist` (`0x6066f0`) of the units
    /// the tokens name, in their order: the player's template toward the unit's friendly, the
    /// unit not unselectable, and, for a unit that is not player-controlled, PvP-flagged.
    #[test]
    fn unit_can_assist_judges_a_chain_end_by_the_model() {
        use crate::spell::targeting::corpse_fixture as fx;
        const TEMPLATE: u16 = 35;
        const FLAGS: u16 = 46;
        let mut app = app();
        app.insert_resource(fx::factions()).add_systems(
            Update,
            crate::ui_unit::relations::feed_unit_relations.before(crate::ui_aura::AuraEvents),
        );
        let with = |extra: &[(u16, u32)], target: u64, kind: u32| {
            let mut pairs = vec![
                (OBJECT_TYPE, kind),
                (TARGET, target as u32),
                (TARGET + 1, (target >> 32) as u32),
                (HEALTH, 4000),
                (MAXHEALTH, 5000),
                (LEVEL, 60),
            ];
            pairs.extend_from_slice(extra);
            ObjectStore(ObjectFields::from_pairs(&pairs))
        };
        let put = |app: &mut App, guid: u64, store: ObjectStore| {
            let e = app.world().resource::<GuidIndex>().0[&guid];
            app.world_mut().entity_mut(e).insert(store);
        };
        // We are a human; `BOSS` (party1's target) a human creature, PvP-flagged; `ADD`
        // (raid2's target) a human creature that is not.
        // A player carries the player-controlled bit, which makes `IsPvP` the unit's test.
        put(
            &mut app,
            ME,
            with(&[(TEMPLATE, 1), (FLAGS, 0x8)], MOB, 0x19),
        );
        put(
            &mut app,
            BOSS,
            with(&[(TEMPLATE, 1), (FLAGS, 0x1000)], GONE, 0x09),
        );
        put(&mut app, ADD, with(&[(TEMPLATE, 1)], 0, 0x09));
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_unit(
                "player",
                Some(UnitState {
                    exists: true,
                    guid: ME,
                    ..Default::default()
                }),
            );
        app.update();
        assert!(
            holds(&app, r#"UnitCanAssist("player", "party1target") == 1"#),
            "a friendly, PvP-flagged creature"
        );
        assert!(
            holds(&app, r#"UnitCanAssist("player", "raid2target") == nil"#),
            "a friendly creature that is not PvP-flagged"
        );
        // The other way round the creature is `this` and the player the argument: the player is
        // player-controlled, so no PvP flag is asked, and `ADD` assists us though we cannot it.
        assert!(
            holds(&app, r#"UnitCanAssist("raid2target", "player") == 1"#),
            "a friendly creature toward the player"
        );
        // A unit nobody holds is no unit: nil.
        assert!(holds(
            &app,
            r#"UnitCanAssist("player", "party1targettarget") == nil"#
        ));
        // The verdict follows a descriptor that moves: `ADD` takes the flag.
        put(
            &mut app,
            ADD,
            with(&[(TEMPLATE, 1), (FLAGS, 0x1000)], 0, 0x09),
        );
        app.update();
        assert!(holds(
            &app,
            r#"UnitCanAssist("player", "raid2target") == 1"#
        ));
        // `UNIT_FLAG_NOT_SELECTABLE` (bit 25) refuses it.
        put(
            &mut app,
            ADD,
            with(&[(TEMPLATE, 1), (FLAGS, 0x1000 | 0x0200_0000)], 0, 0x09),
        );
        app.update();
        assert!(holds(
            &app,
            r#"UnitCanAssist("player", "raid2target") == nil"#
        ));
    }

    /// The chain feed reads the resolver inputs the aura feed pushes in the same frame, so the
    /// declared order puts the feed after it: read off the schedule graph.
    #[test]
    fn the_chain_feed_runs_after_the_aura_feed_it_reads() {
        use crate::game_plugins::schedule_tests::{census, headless_client, SyncPoints};

        let mut app = headless_client();
        let c = census(&mut app, Update, SyncPoints::Declared);
        if !c.systems.values().any(|s| s.name.contains("benilla_app::")) {
            eprintln!("skipped: this build carries no type names");
            return;
        }
        let one = |suffix: &str| {
            let mut hits = c.systems.iter().filter(|(_, s)| s.name.ends_with(suffix));
            match (hits.next(), hits.next()) {
                (Some((k, _)), None) => *k,
                _ => panic!("exactly one system named `…{suffix}` in Update"),
            }
        };
        let aura = one("::ui_aura::feed_auras");
        for feed in [
            "::ui_unit::held::feed_chain_units",
            "::targeting::feed_targeting_to_vm",
        ] {
            assert!(
                c.dependencies.contains(&(aura, one(feed))),
                "{feed} is declared after feed_auras"
            );
        }
    }
}
