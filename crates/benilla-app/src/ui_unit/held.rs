//! The snapshot of a held unit that a token names other than the player, and the per-guid feed of
//! the units a `target` chain ends on.
//!
//! The reference resolves a token to a guid (`0x515970`) and reads that guid's object, so a unit it
//! holds answers a `Unit*` getter however the token spelled it. The app pushes a snapshot
//! for each base token by name ([`UiScript::set_unit`]); a chain such as `party1target` names a
//! unit no base does, so the VM resolves it to a guid and reads that guid's snapshot, which
//! [`feed_chain_units`] pushes for the guids [`UiScript::chain_end_guids`] lists.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_formats::ChrClasses;
use benilla_ui::script::{UiScript, UnitState};

use super::{enrich_unit, faction_group, faction_group_localized, snapshot, unit_reaction};
use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, ObjectStore, Reputations, SelfPlayer};
use crate::target::Factions;
use crate::ui_script::{gate, VmMemo};

/// What a held unit's snapshot reads beside its descriptor.
pub(super) struct HeldUnits<'a> {
    pub(super) names: &'a NameCache,
    pub(super) commands: &'a NetCommands,
    pub(super) factions: Option<&'a Factions>,
    pub(super) reputations: &'a Reputations,
    pub(super) group: &'a crate::ui_party::GroupState,
    /// Our own descriptor, which reaction, attackability and the faction line read.
    pub(super) self_store: Option<&'a ObjectStore>,
    pub(super) classes: Option<&'a ChrClasses>,
    pub(super) types: CreatureTypeSources<'a>,
}

impl HeldUnits<'_> {
    /// The snapshot of the held unit `guid`, whose descriptor is `store`, as a token naming a unit
    /// other than the player carries it. It has no guild leg: nothing asks a chain's unit for
    /// one, and the lazy guild cache sends a query on a miss.
    pub(super) fn state(&self, store: &ObjectStore, guid: u64) -> UnitState {
        let name = self
            .names
            .resolve_unit(guid, Some(store), self.commands)
            .map(str::to_string);
        let reaction = unit_reaction(self.factions, self.reputations, store, self.self_store);
        let mut s = snapshot(store, guid, name, reaction, self.classes, self.types);
        s.raid_target = self.group.raid_target_index(guid);
        s.faction_group = faction_group(store, self.factions);
        s.faction_group_localized = faction_group_localized(store, self.factions);
        // `CanAttack` (`0x606980`); `UnitCanAttack` gates the target frame's level colour.
        s.can_attack = crate::target::can_attack(
            Some(store),
            self.factions,
            self.reputations,
            self.self_store,
        );
        enrich_unit(
            &mut s,
            guid,
            self.names,
            store,
            self.factions,
            self.self_store,
        );
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

/// Push the snapshot of each held unit a `target` chain can end on, by guid, so `UnitName`,
/// `UnitHealth` and the other getters answer for `party1target`, `raid3target` and `pettarget`
/// (`0x515970` resolves the token, then each getter reads the guid's object, `0x468460`). The
/// guids are the resolver's own inputs ([`UiScript::chain_end_guids`]), which the aura feed pushes
/// each frame, so this runs after it. A guid the object manager does not hold, or holds as
/// anything but a unit, gets no snapshot, so the getters answer nobody for it. The reference also
/// answers a group member with no object from its roster record (`0x496400`), which a chain's end
/// does not get here. A token without a hop reads its own push, not these.
pub(super) fn feed_chain_units(
    script: Option<NonSendMut<UiScript>>,
    tables: super::SnapshotTables,
    // The object manager's lookup (`0x468460`) on its own, not through [`super::UnitTokens`],
    // whose pet-bar read would order this feed against the pet feed it does not depend on.
    index: Option<Res<crate::net::GuidIndex>>,
    stores: Query<&ObjectStore>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    group: Res<crate::ui_party::GroupState>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    changed: Query<(), (Changed<ObjectStore>, Without<crate::items::ItemObject>)>,
    mut removed: RemovedComponents<Guid>,
    mut memo: Local<VmMemo<ChainMemo>>,
) {
    let Some(mut script) = script else {
        return;
    };
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
    let reps_changed = reputations.is_changed();
    let factions_changed = factions.as_ref().is_some_and(|r| r.is_changed());
    removed.clear();
    let gate = gate::Gate::new(
        vm_reset
            || names_moved
            || ends_moved
            || stores_changed
            || stores_removed
            || group_changed
            || reps_changed
            || factions_changed,
    );
    if gate.skip() {
        return;
    }

    let held = HeldUnits {
        names: &names,
        commands: &commands,
        factions: factions.as_deref(),
        reputations: &reputations,
        group: &group,
        self_store: self_q.iter().next(),
        classes: tables.classes(),
        types: tables.types(&names),
    };
    let built: HashMap<u64, UnitState> = ends
        .iter()
        .filter_map(|&guid| {
            let entity = *index.as_ref()?.0.get(&guid)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::CreatureRecord;
    use crate::net::GuidIndex;
    use crate::target::Selection;
    use crate::ui_party::{GroupState, GROUPTYPE_RAID};
    use benilla_protocol::messages::{member_status, GroupMemberEntry};
    use benilla_protocol::ObjectFields;

    const ME: u64 = 0x10;
    /// party1 and raid2, party2 and raid3, party3 and raid4.
    const A: u64 = 0x1001;
    const B: u64 = 0x1002;
    const C: u64 = 0x1003;
    /// Our target, which targets party1.
    const MOB: u64 = 0xF130_0000_4500_0001;
    /// What party1 targets.
    const BOSS: u64 = 0xF130_0000_4600_0001;
    /// What raid3 targets, and a unit that targets nobody.
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

    fn spawn(app: &mut App, guid: u64, store: ObjectStore) -> Entity {
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
    /// `GONE`; raid3 (`B`) targets `ADD`, our pet `FOE`, and `C` nobody. `LONER` is held and
    /// targets `EXTRA`. The group is a raid of us, `A`, `B` and `C`, all in our subgroup, so the
    /// three are also party slots 1 to 3. The aura feed, whose resolver inputs the chain feed
    /// reads, and the chain feed run.
    fn app() -> App {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(crate::char_select::ClientState::InWorld)
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
    /// `raid3target` and `pettarget` read the unit at the end of the chain and any break in the
    /// chain reads nobody. No token names `ADD` or `FOE` but a chain.
    #[test]
    fn the_getters_answer_for_the_unit_a_chain_ends_on() {
        let mut app = app();
        app.update();
        for (token, want, hp) in [
            ("party1target", "Boss", 4000),
            ("raid2target", "Boss", 4000),
            ("PARTY1TARGET", "Boss", 4000),
            ("raid3target", "Add", 250),
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
            r#"UnitIsUnit("raid2target", "party1target") == 1"#
        ));
        assert!(holds(
            &app,
            r#"UnitIsUnit("raid2target", "raid3target") == nil"#
        ));
        // `BOSS` targets `GONE`, which nobody holds; `C` targets nobody; party4 is nobody.
        for token in [
            "party1targettarget",
            "raid4target",
            "party4target",
            "raid9target",
        ] {
            assert_eq!(name(&app, token), None, "UnitName({token})");
            assert_eq!(health(&app, token), 0, "UnitHealth({token})");
            assert!(
                holds(&app, &format!("UnitExists({token:?}) == nil")),
                "UnitExists({token})"
            );
        }
        // The plain tokens read their own push and nothing else: this world pushes none, so
        // `raid2` answers nil though a chain ends on it, and a token pushed by name wins.
        assert_eq!(name(&app, "raid2"), None);
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

        // `ADD` leaves the object manager, and `raid3target` with it. The chain is still
        // `raid3`'s target, so the resolver's guid holds; the snapshot is what goes.
        assert_eq!(name(&app, "raid3target").as_deref(), Some("Add"));
        let add = app.world_mut().resource_mut::<GuidIndex>().0.remove(&ADD);
        app.world_mut().despawn(add.unwrap());
        app.update();
        assert_eq!(name(&app, "raid3target"), None);
        assert_eq!(health(&app, "raid3target"), 0);
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
