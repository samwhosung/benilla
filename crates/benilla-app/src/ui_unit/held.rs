//! The snapshot of a held unit that a token names other than the player, and of a guid the object
//! manager does not hold.
//!
//! The reference resolves a token to a guid (`0x515970`) and reads that guid's object, so a unit it
//! holds answers a `Unit*` getter however the token spelled it. The getters ask the lent host for
//! the unit as of the call ([`super::live::UnitFeedLive`]), which builds it here, and the views
//! lane builds the four tokens' snapshots off it ([`super::views`]).

use bevy::prelude::*;

use benilla_ui::script::UnitState;

use super::{
    enrich_unit, faction_group, faction_group_localized, player_reaction, snapshot, unit_reaction,
};
use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::ObjectStore;
use crate::query_cache::Ask;
use crate::target::{RelUnit, Relations};

/// A held unit's reach from the player: its squared distance between `at` and `me_at`, and
/// whether it passes `0x4944a0`'s other legs, the one `CanInspect` answers by (`0x48a2a1`): a
/// player unit (`0x4944d8`) that is the active player (`0x4944dd`) or one it can cooperate with
/// (`0x4944e4`), judged by `rel`. Cooperation is the faction group, not attackability: an
/// unflagged enemy-faction player is not inspectable, a same-faction duel opponent is.
pub(crate) fn reach_between(
    rel: &Relations,
    unit: Option<&RelUnit>,
    me: Option<&RelUnit>,
    at: Vec3,
    me_at: Vec3,
) -> benilla_ui::script::UnitReach {
    let inspectable = unit.zip(me).is_some_and(|(unit, me)| {
        unit.is_player && (unit.guid == me.guid || rel.can_cooperate(me, unit))
    });
    benilla_ui::script::UnitReach {
        dist_sq: super::dist_sq(at, me_at),
        inspectable,
    }
}

/// What a held unit's snapshot reads beside its descriptor.
pub(super) struct HeldUnits<'a> {
    pub(super) names: &'a NameCache,
    pub(super) group: &'a crate::ui_party::GroupState,
    /// Our own descriptor, which the faction line reads.
    pub(super) self_store: Option<&'a ObjectStore>,
    pub(super) unit_names: super::UnitNames<'a>,
    pub(super) types: CreatureTypeSources<'a>,
    /// The relation functions, over the object manager, and the active player they judge from.
    pub(super) rel: Relations<'a>,
    pub(super) me: Option<RelUnit>,
    /// What the unit tooltip's own lines read.
    pub(super) tooltip: super::tooltip_facts::TooltipSources<'a>,
    /// The guild cache, which the guild leg reads without asking.
    pub(super) guild: &'a crate::ui_guild::GuildState,
}

impl HeldUnits<'_> {
    /// The snapshot of the held unit `guid`, whose descriptor is `store`, under `name`, as a token
    /// naming a unit other than the player carries it, however the token spells it:
    /// `GetGuildInfo` resolves the token and reads that object's guild (`0x4c9330`), so the guild
    /// leg is every held unit's.
    pub(super) fn state_named(
        &self,
        store: &ObjectStore,
        guid: u64,
        name: Option<String>,
    ) -> UnitState {
        let unit = RelUnit::of(guid, store).with_record(store, self.names);
        let factions = self.rel.factions;
        let reaction = unit_reaction(&self.rel, &unit, self.me.as_ref());
        let mut s = snapshot(store, guid, name, reaction, self.unit_names, self.types);
        s.is_pvp_ffa = super::pvp_free_for_all(&self.rel, &unit);
        s.player_reaction = player_reaction(&self.rel, &unit, self.me.as_ref());
        s.raid_target = self.group.raid_target_index(guid);
        s.faction_group = faction_group(store, factions);
        s.faction_group_localized = faction_group_localized(store, factions);
        enrich_unit(&mut s, guid, self.names, store, factions, self.self_store);
        s.tooltip = super::tooltip_facts::tooltip_facts(store, s.creature_type, &self.tooltip);
        // `[[unit+0xe68]+0xc]` (`0x4c93a3`) against the cache; a miss is `GetGuildInfo`'s to ask.
        s.guild = crate::ui_guild::cached_unit_guild(&store.0, self.guild);
        s
    }
}

/// A chain end the object manager does not hold, as the getters read a guid with no object: the
/// player-name cache by guid whatever the guid is (`0x5171eb`), which vmangos answers only under a
/// player's guid (`QueryHandler.cpp:48-60`), so a creature or a pet reads `UNKNOWNOBJECT`; the
/// raid mark by guid (`0x4bb4f1`);
/// nothing else. Off the roster it does not exist (`0x491900`); on it, the VM reads the row's push.
/// The name is read without asking: the lookup asks when `UnitName` reads it, inside its call
/// ([`benilla_ui::script::RecordAsk::PlayerName`]).
pub(crate) fn unheld_state(
    names: &NameCache,
    group: &crate::ui_party::GroupState,
    guid: u64,
    tables: super::UnitNames<'_>,
) -> UnitState {
    let mut s = UnitState {
        guid,
        name: names.resolve_player(guid, Ask::Read).map(str::to_string),
        raid_target: group.raid_target_index(guid),
        ..Default::default()
    };
    traits_from_name_cache(&mut s, names, guid, tables);
    s
}

/// What `UnitRace`, `UnitClass` and `UnitSex` read of a unit with no object: the name cache record
/// of its guid, looked up with no callback, so nothing is asked (`0x5182bc`): the race at `+0x138`
/// (`0x5182d9`), the class at `+0x140` (`0x518429`) and the sex at `+0x13c` (`0x517f80`) through
/// `{2, 3, 1, 6}` (`0x808be4`). A miss leaves them unset: `nil, nil`, and `UnitSex` 2 (`0x517f95`).
pub(crate) fn traits_from_name_cache(
    s: &mut UnitState,
    names: &NameCache,
    guid: u64,
    tables: super::UnitNames<'_>,
) {
    let Some((race, class, gender)) = names.player_traits(guid) else {
        return;
    };
    let race = tables.race(race);
    let class = tables.class(class);
    s.race = race.map(|(n, _)| n.to_string());
    s.race_file = race.map(|(_, f)| f.to_string());
    s.class = class.map(|(n, _)| n.to_string());
    s.class_file = class.map(|(_, f)| f.to_string());
    // The table has no bound; past its four entries there is nothing to read, so it stays unset.
    s.sex = [2, 3, 1, 6].get(usize::from(gender)).copied().unwrap_or(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::CreatureRecord;
    use crate::net::{Guid, GuidIndex, NetCommands, Reputations, SelfPlayer};
    use crate::target::Selection;
    use crate::ui_party::{GroupState, GROUPTYPE_RAID};
    use benilla_protocol::messages::{member_status, GroupMemberEntry};
    use benilla_protocol::ObjectFields;
    use benilla_ui::script::UiScript;

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
    /// three are also party slots 1 to 3. The resolver feed, whose inputs the chain feed reads,
    /// and the chain feed run.
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
            .init_resource::<crate::ui_guild::GuildState>()
            .insert_resource(NetCommands(tx))
            .init_resource::<crate::ui_session::InteractLatch>()
            .add_plugins(crate::ui_aura::UiAuraPlugin);
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
                        pet_spell_list: 0,
                        civilian: false,
                        racial_leader: false,
                        display_id: 0,
                    }),
                );
            }
        }
        let mut mine = player(MOB, 900);
        // `UNIT_FIELD_SUMMON`, the pet `"pet"` names (`0x515a6e`).
        mine.0.merge(ObjectFields::from_pairs(&[
            (8, MY_PET as u32),
            (9, (MY_PET >> 32) as u32),
        ]));
        let me = spawn(&mut app, ME, mine);
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
        app.init_resource::<crate::ui_pet::PetBar>();
        app
    }

    fn script(app: &App) -> &UiScript {
        app.world().non_send_resource::<UiScript>()
    }

    /// `chunk`'s answer with the host lent, as the getters run in game: the units, the tokens and
    /// the selection read as of the call. The name asks the getters made are asked as the host
    /// asks them inside the call ([`benilla_ui::script::RecordAsk::PlayerName`]).
    fn lent<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
        use benilla_ui::script::{RecordAsk, ScriptCall};
        let (answer, calls) = crate::ui_unit::live::eval_lent_objects::<T>(app.world_mut(), chunk);
        let world = app.world();
        for call in calls {
            if let ScriptCall::RecordAsk(RecordAsk::PlayerName(guid)) = call {
                let commands = world.resource::<NetCommands>();
                let _ = world
                    .resource::<NameCache>()
                    .resolve_player(guid, Ask::Event(commands));
            }
        }
        answer
    }

    fn name(app: &mut App, token: &str) -> Option<String> {
        lent(app, &format!("return UnitName({token:?})"))
    }

    fn number(app: &mut App, call: &str, token: &str) -> i64 {
        lent(app, &format!("return {call}({token:?})"))
    }

    fn health(app: &mut App, token: &str) -> i64 {
        number(app, "UnitHealth", token)
    }

    /// Whether the Lua expression holds.
    fn holds(app: &mut App, expr: &str) -> bool {
        lent(app, &format!("return {expr}"))
    }

    /// The getters answer for a chain through the feeds that run in the client: the resolver
    /// feed's inputs name the guid, and this feed's snapshot of it answers, so `party1target`,
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
                name(&mut app, token).as_deref(),
                Some(want),
                "UnitName({token})"
            );
            assert_eq!(health(&mut app, token), hp, "UnitHealth({token})");
            assert_eq!(number(&mut app, "UnitHealthMax", token), 5000, "{token}");
            assert_eq!(number(&mut app, "UnitLevel", token), 60, "{token}");
            assert!(
                holds(&mut app, &format!("UnitExists({token:?}) == 1")),
                "UnitExists({token})"
            );
        }
        assert!(holds(
            &mut app,
            r#"UnitIsUnit("raid1target", "party1target") == 1"#
        ));
        assert!(holds(
            &mut app,
            r#"UnitIsUnit("raid1target", "raid2target") == nil"#
        ));
        // `BOSS` targets `GONE`, which nobody holds: no record and off the roster, so 0 and nil
        // ([`an_unheld_chain_end_answers_what_the_client_knows_of_it`] has its name).
        assert_eq!(health(&mut app, "party1targettarget"), 0);
        assert!(holds(
            &mut app,
            r#"UnitExists("party1targettarget") == nil"#
        ));
        // `C` targets nobody; party4 is nobody.
        for token in ["raid3target", "party4target", "raid9target"] {
            assert_eq!(name(&mut app, token), None, "UnitName({token})");
            assert_eq!(health(&mut app, token), 0, "UnitHealth({token})");
            assert!(
                holds(&mut app, &format!("UnitExists({token:?}) == nil")),
                "UnitExists({token})"
            );
        }
        // A group token reads the unit its row names at the call, as a chain does. `target` reads
        // the unit live too, past a push of its own; with no host lent, the push answers.
        assert_eq!(name(&mut app, "raid1").as_deref(), Some("Alia"));
        assert_eq!(name(&mut app, "target").as_deref(), Some("Mob"));
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
        assert_eq!(name(&mut app, "target").as_deref(), Some("Mob"));
        let unlent: Option<String> = script(&app).eval(r#"return UnitName("target")"#).unwrap();
        assert_eq!(unlent.as_deref(), Some("Pushed"));
    }

    /// `UnitIsPVPFreeForAll` (`0x516540`) on a held unit: `UNIT_FIELD_FLAGS` 0x8
    /// (`0x5165ad`-`0x5165bf`), then the player controlling it (`0x606170` at `0x5165c3`, its
    /// charmer, else its creator, else itself, when that is a player) and that player's
    /// `PLAYER_FLAGS` 0x80 (`0x5165cc`-`0x5165d5`). A creature charmed by a free-for-all player
    /// answers 1 with no player flags of its own; a player without bit 0x8 answers nil though
    /// flagged; a creature that answers to nobody is no player and answers nil.
    #[test]
    fn unit_is_pvp_free_for_all_reads_the_controlling_players_flag() {
        const FLAGS: u16 = benilla_protocol::field::FIELD_UNIT_FLAGS;
        const CHARMEDBY: u16 = 10;
        const PLAYER_FLAGS: u16 = 190;
        let mut app = app();
        let set = |app: &mut App, guid: u64, pairs: &[(u16, u32)]| {
            let e = app.world().resource::<GuidIndex>().0[&guid];
            let mut store = app.world_mut().get_mut::<ObjectStore>(e).unwrap();
            store.0.merge(ObjectFields::from_pairs(pairs));
        };
        // `MOB` (`playertarget`) is charmed by `A` (`targettarget`), free-for-all; `BOSS`
        // (`party1target`) is player-controlled with no owner.
        set(&mut app, A, &[(FLAGS, 0x8), (PLAYER_FLAGS, 0x80)]);
        set(
            &mut app,
            MOB,
            &[
                (FLAGS, 0x8),
                (CHARMEDBY, A as u32),
                (CHARMEDBY + 1, (A >> 32) as u32),
            ],
        );
        set(&mut app, BOSS, &[(FLAGS, 0x8), (PLAYER_FLAGS, 0x80)]);
        app.update();
        let ffa = |app: &mut App, token: &str| {
            holds(app, &format!("UnitIsPVPFreeForAll({token:?}) == 1"))
        };
        assert!(ffa(&mut app, "targettarget"), "a flagged player");
        assert!(
            ffa(&mut app, "playertarget"),
            "a creature its charmer flags"
        );
        assert!(
            !ffa(&mut app, "party1target"),
            "a creature, its own controller"
        );
        set(&mut app, A, &[(FLAGS, 0)]);
        app.update();
        assert!(!ffa(&mut app, "targettarget"), "a player without bit 0x8");
        assert!(
            ffa(&mut app, "playertarget"),
            "the charmer's own bit 0x8 is not asked"
        );
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
        app.init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>();
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.keep_calls();
            script.run(r#"UNKNOWNOBJECT = "Unknown""#).unwrap();
            // Level 60, so the level 0 of a unit with no object would read grey.
            script.set_player_req_state(benilla_ui::script::PlayerReqState {
                level: 60,
                ..Default::default()
            });
        }
        // A Troll (8) Hunter (3), female (1), as the name query answered.
        app.world_mut().resource_mut::<NameCache>().insert_player(
            STRANGER,
            "Wanderer".into(),
            Some((8, 3, 1)),
        );
        let tables = crate::ui_unit::TestNames::english();
        app.insert_resource(crate::chr_classes::ChrRaceTable(tables.races))
            .insert_resource(crate::chr_classes::ChrClassTable(tables.classes));
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
        let nil = |app: &mut App, getters: &[&str]| {
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
        assert_eq!(name(&mut app, chain).as_deref(), Some("Unknown"));
        app.update();
        assert_eq!(asked(&rx), vec![GONE], "the read's miss is asked");
        assert_eq!(name(&mut app, chain).as_deref(), Some("Unknown"));
        assert_eq!(health(&mut app, chain), 0);
        nil(
            &mut app,
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
        assert_eq!(number(&mut app, "GetRaidTargetIndex", chain), 3);
        assert_eq!(
            asked(&rx),
            Vec::<u64>::new(),
            "a pending name is asked once"
        );

        // A player out of view: the cached name, race, class and sex (`0x5182d9`, `0x518429`,
        // `0x517f80`), and still no unit.
        retarget_boss(&mut app, STRANGER);
        assert_eq!(name(&mut app, chain).as_deref(), Some("Wanderer"));
        assert_eq!(asked(&rx), Vec::<u64>::new(), "a cached name is not asked");
        nil(&mut app, &["UnitExists", "UnitIsPlayer", "UnitPVPName"]);
        let pair = |app: &mut App, getter: &str| -> (Option<String>, Option<String>) {
            lent(app, &format!("return {getter}({chain:?})"))
        };
        assert_eq!(
            pair(&mut app, "UnitRace"),
            (Some("Troll".into()), Some("Troll".into()))
        );
        assert_eq!(
            pair(&mut app, "UnitClass"),
            (Some("Hunter".into()), Some("HUNTER".into()))
        );
        assert_eq!(number(&mut app, "UnitSex", chain), 3);

        // A party member out of view: what `party4` reads, off the record.
        let mut entries: Vec<GroupMemberEntry> =
            [A, B, C, D].into_iter().map(group_entry).collect();
        entries[3].name = "Dara".into();
        // The name the list's lookup asked for, answered (`0x4e8477`).
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(D, "Dara".into(), None);
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
        assert!(holds(&mut app, &format!("UnitExists({chain:?}) == 1")));
        assert!(holds(&mut app, &format!("UnitIsPlayer({chain:?}) == 1")));
        assert!(holds(&mut app, &format!("UnitIsConnected({chain:?}) == 1")));
        assert_eq!(name(&mut app, chain).as_deref(), Some("Dara"));
        assert_eq!(health(&mut app, chain), 1234);
        assert_eq!(number(&mut app, "UnitHealthMax", chain), 2000);
        assert_eq!(number(&mut app, "UnitLevel", chain), 41);
        nil(&mut app, &["UnitIsVisible", "UnitPVPName"]);
        assert_eq!(name(&mut app, chain), name(&mut app, "party4"));

        // Its pet: the pet record, and on the roster while the owner's record is online.
        retarget_boss(&mut app, D_PET);
        assert!(holds(&mut app, &format!("UnitExists({chain:?}) == 1")));
        assert_eq!(name(&mut app, chain).as_deref(), Some("Grub"));
        assert_eq!(health(&mut app, chain), 77);
        assert_eq!(number(&mut app, "UnitHealthMax", chain), 99);
        nil(&mut app, &["UnitIsVisible", "UnitIsPlayer"]);
    }

    /// The getters follow the world at the call: a descriptor that moves, a chain that gains an
    /// end with no descriptor moving, and a unit that leaves the object manager.
    #[test]
    fn the_getters_follow_the_world() {
        let mut app = app();
        app.update();
        assert_eq!(health(&mut app, "party1target"), 4000);

        // A descriptor moves.
        let boss = app.world().resource::<GuidIndex>().0[&BOSS];
        app.world_mut()
            .entity_mut(boss)
            .insert(creature(GONE, 1000));
        app.update();
        assert_eq!(health(&mut app, "party1target"), 1000);

        // Selecting `LONER` gives `targettarget` a unit that no chain ended on, and moves no
        // descriptor and no name.
        assert_eq!(name(&mut app, "targettarget").as_deref(), Some("Alia"));
        let loner = app.world().resource::<GuidIndex>().0[&LONER];
        app.insert_resource(Selection {
            target: Some(loner),
            guid: Some(LONER),
            ..Default::default()
        });
        app.update();
        assert_eq!(name(&mut app, "targettarget").as_deref(), Some("Extra"));
        assert_eq!(health(&mut app, "targettarget"), 45);

        // `ADD` leaves the object manager, and `raid2target` with it. The chain is still
        // `raid2`'s target, so the resolver's guid holds; the unit is what goes.
        assert_eq!(name(&mut app, "raid2target").as_deref(), Some("Add"));
        let add = app.world_mut().resource_mut::<GuidIndex>().0.remove(&ADD);
        app.world_mut().despawn(add.unwrap());
        app.update();
        // The name would be the name cache's (`0x5171eb`), which a chain's end does not read yet.
        assert_eq!(health(&mut app, "raid2target"), 0);
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
        // The verbs ask the relation functions of the lent host, as of the call.
        let lent = |app: &mut App, expr: &str| -> bool { holds(app, expr) };
        app.insert_resource(fx::factions());
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
            lent(&mut app, r#"UnitCanAssist("player", "party1target") == 1"#),
            "a friendly, PvP-flagged creature"
        );
        assert!(
            lent(&mut app, r#"UnitCanAssist("player", "raid2target") == nil"#),
            "a friendly creature that is not PvP-flagged"
        );
        // The other way round the creature is `this` and the player the argument: the player is
        // player-controlled, so no PvP flag is asked, and `ADD` assists us though we cannot it.
        assert!(
            lent(&mut app, r#"UnitCanAssist("raid2target", "player") == 1"#),
            "a friendly creature toward the player"
        );
        // A unit nobody holds is no unit: nil.
        assert!(lent(
            &mut app,
            r#"UnitCanAssist("player", "party1targettarget") == nil"#
        ));
        // The verdict follows a descriptor that moves: `ADD` takes the flag.
        put(
            &mut app,
            ADD,
            with(&[(TEMPLATE, 1), (FLAGS, 0x1000)], 0, 0x09),
        );
        app.update();
        assert!(lent(
            &mut app,
            r#"UnitCanAssist("player", "raid2target") == 1"#
        ));
        // `UNIT_FLAG_NOT_SELECTABLE` (bit 25) refuses it.
        put(
            &mut app,
            ADD,
            with(&[(TEMPLATE, 1), (FLAGS, 0x1000 | 0x0200_0000)], 0, 0x09),
        );
        app.update();
        assert!(lent(
            &mut app,
            r#"UnitCanAssist("player", "raid2target") == nil"#
        ));
    }

    /// `GetGuildInfo` (`0x4c9330`) resolves any token (`0x515940`) and reads that object's guild,
    /// `[[unit+0xe68]+0xc]` (`0x4c93a3`), from the cache (`0x4c93d2`), so a held player a chain
    /// ends on answers its guild as a base token does; a record that lands later answers on the
    /// next frame. `targettarget` here is a chain: no feed in this world pushes it by name.
    #[test]
    fn get_guild_info_answers_for_the_player_a_chain_ends_on() {
        /// `PLAYER_GUILDID` and `PLAYER_GUILDRANK`.
        const GUILD_ID: u16 = 191;
        const GUILD_RANK: u16 = 192;
        let mut app = app();
        let a = app.world().resource::<GuidIndex>().0[&A];
        app.world_mut()
            .get_mut::<ObjectStore>(a)
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(&[(GUILD_ID, 7), (GUILD_RANK, 2)]));
        app.update();
        let info = |app: &mut App, token: &str| -> (Option<String>, i64) {
            let (name, _, rank): (Option<String>, Option<String>, i64) =
                lent(app, &format!("return GetGuildInfo({token:?})"));
            (name, rank)
        };
        assert_eq!(info(&mut app, "targettarget"), (None, 0), "not cached yet");
        app.world_mut()
            .resource_mut::<crate::ui_guild::GuildState>()
            .land_for_test(7, "Probe Guild");
        app.update();
        for token in ["targettarget", "playertargettarget"] {
            let want = (Some("Probe Guild".to_string()), 2);
            assert_eq!(info(&mut app, token), want, "GetGuildInfo({token})");
        }
    }
}
