//! The group pets' unit tokens, `partypet1..4` and `raidpet1..40`: each member's pet as the
//! per-token snapshot the `Unit*` getters read. [`super::feed`] drives it from the party feed's
//! gate. Pushing fires nothing: `UNIT_PET` and the pets' own unit events are the descriptor's
//! watches and the stats packet's record diff (`crate::field_watch`, `super::records`).
//!
//! A pet the object manager holds is an ordinary live unit. One it does not hold is answered from
//! the owner's roster record, the way the reference's getters fall through: the live descriptor,
//! then the member record, then the pet record (`0x496420`). `UnitExists` answers 1 for it through
//! the roster fallback (`0x491900`), which needs no object, and `UnitLevel`, `UnitIsDead` and
//! `UnitIsGhost` have no pet leg, so they read as they do for a unit the client knows nothing of.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{UiScript, UnitState};

use crate::creature_type::CreatureTypeSources;
use crate::names::NameCache;
use crate::net::{GuidIndex, NetCommands, ObjectStore};
use crate::ui_script::gate;

use super::GroupState;
use crate::query_cache::Ask;

/// Which roster a pet token counts its owner in: the two resolve the pet's guid apart
/// (`0x4e81d0` behind the owner's online bit, `0x491960` with none).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Roster {
    Party,
    Raid,
}

/// The name cache with the handle that asks it, one parameter of [`super::feed::feed_party`],
/// which sits at Bevy's 16-parameter limit: a held pet's name is asked of the server once, as any
/// held unit's is. It derefs to the cache.
#[derive(SystemParam)]
pub(crate) struct Names<'w> {
    cache: Res<'w, NameCache>,
    commands: Res<'w, NetCommands>,
}

impl std::ops::Deref for Names<'_> {
    type Target = NameCache;

    fn deref(&self) -> &NameCache {
        &self.cache
    }
}

/// What the pets' snapshots read: the object manager, the name cache and the creature-type
/// resolver's sources.
pub(super) struct Lookup<'a, 'w, 's, 'q> {
    index: &'a GuidIndex,
    stores: &'a Query<'w, 's, &'q ObjectStore>,
    names: &'a NameCache,
    commands: &'a NetCommands,
    types: CreatureTypeSources<'a>,
    /// `Spell.dbc`, which the tooltip's owner line reads a creating spell from.
    catalog: Option<&'a benilla_formats::SpellCatalog>,
    study: crate::ui_unit::tooltip_facts::StudySources<'a>,
}

impl<'a, 'w, 's, 'q> Lookup<'a, 'w, 's, 'q> {
    pub(super) fn new(
        index: &'a GuidIndex,
        stores: &'a Query<'w, 's, &'q ObjectStore>,
        names: &'a Names,
        types: CreatureTypeSources<'a>,
        catalog: Option<&'a benilla_formats::SpellCatalog>,
        study: crate::ui_unit::tooltip_facts::StudySources<'a>,
    ) -> Self {
        Self {
            index,
            stores,
            names: &names.cache,
            commands: &names.commands,
            types,
            catalog,
            study,
        }
    }

    /// The held object's descriptor (`0x468460`).
    pub(super) fn store(&self, guid: u64) -> Option<&ObjectStore> {
        self.stores.get(*self.index.0.get(&guid)?).ok()
    }
}

/// The guid `partypetN` or `raidpetN` names for `owner`, off its descriptor while held, else its
/// record (see [`GroupState::party_pet_guid`], [`GroupState::raid_pet_guid`]).
fn pet_guid(
    group: &GroupState,
    owner: u64,
    held: Option<&ObjectStore>,
    roster: Roster,
) -> Option<u64> {
    let live = held.map(|s| &s.0);
    match roster {
        Roster::Party => group.party_pet_guid(owner, live),
        Roster::Raid => group.raid_pet_guid(owner, live),
    }
}

/// Whether a held pet of any group member has moved, for the party feed's gate. Every member is
/// asked as a raid row, whose record leg has no online test: a superset of the party slots'.
pub(super) fn store_changed(
    look: &Lookup<'_, '_, '_, '_>,
    group: &GroupState,
    own: Option<(u64, &ObjectStore)>,
    changed: &Query<(), Changed<ObjectStore>>,
) -> bool {
    let moved = |owner: u64, held: Option<&ObjectStore>| {
        pet_guid(group, owner, held, Roster::Raid)
            .and_then(|pet| look.index.0.get(&pet))
            .is_some_and(|&e| changed.get(e).is_ok())
    };
    group.members.iter().any(|m| moved(m.guid, look.store(m.guid)))
        // Our own pet is a raid row's only, a party's slots being the others'.
        || (group.group_type == super::GROUPTYPE_RAID
            && own.is_some_and(|(guid, store)| moved(guid, Some(store))))
}

/// One member's pet for its token: the snapshot the getters read, or `None` for an empty slot,
/// `owner` 0, or a member with no pet the roster or the descriptor names
/// ([`GroupState::party_pet_guid`], [`GroupState::raid_pet_guid`]).
pub(super) fn member_pet(
    look: &Lookup<'_, '_, '_, '_>,
    group: &GroupState,
    owner: u64,
    held: Option<&ObjectStore>,
    roster: Roster,
) -> Option<UnitState> {
    if owner == 0 {
        return None;
    }
    let guid = pet_guid(group, owner, held, roster)?;
    let mut state = match look.store(guid) {
        // A held pet is a live unit like any: every getter reads its descriptor (`0x468460`), and
        // `UnitIsConnected` answers 1 for any object (`0x517daf`), as `snapshot` does. No
        // reaction: the pet frame reads none, as the `"pet"` token's.
        Some(store) => {
            // The unit's create looked its name up with a callback (`0x5dec12`); a frame asks only
            // a name the cache holds nothing for, as one invalidated since, and a refused one waits
            // for the next create.
            let name = look
                .names
                .resolve_unit(guid, Some(store), Ask::New(look.commands))
                .map(str::to_string);
            let mut s =
                crate::ui_unit::snapshot(store, guid, name, 0, Default::default(), look.types);
            let object = |g: u64| look.store(g);
            let sources = crate::ui_unit::tooltip_facts::TooltipSources {
                names: look.names,
                catalog: look.catalog,
                object: &object,
                study: look.study,
            };
            s.tooltip =
                crate::ui_unit::tooltip_facts::tooltip_facts(store, s.creature_type, &sources);
            s
        }
        None => record_state(
            group,
            guid,
            owner,
            group.roster_names_pet(owner, held.is_some()),
        ),
    };
    // Judged by guid alone, with no object needed.
    state.raid_target = group.raid_target_index(guid);
    Some(state)
}

/// A pet with no object, off the pet record that names it (`0x496420`); with no record, a bare
/// snapshot that carries only the guid.
///
/// The getters' record leg (`UnitHealth 0x5174d0`, `UnitHealthMax 0x5175b0`, `UnitMana 0x517670`,
/// `UnitManaMax 0x5177e0`, `UnitPowerType 0x517940`) reads the pet block's `+0xdc`, `+0xde`,
/// `+0xe0`, `+0xe2` and `+0xd8`, the two power figures divided as a live unit's. `UnitName`
/// (`0x5171da`) reads the block's name buffer whole, so an unnamed pet is the empty string, not
/// `UNKNOWNOBJECT`. `UnitIsConnected` is existence-only there (`0x517dfb`). `UnitLevel`,
/// `UnitIsDead`, `UnitIsGhost` and `UnitIsDeadOrGhost` never look at a pet record: 0, nil, nil,
/// nil, which the snapshot's defaults are.
///
/// The roster names the pet's owner with no object, on the terms it answers `UnitExists`: the
/// party half of `UnitPlayerOrPetInParty` compares guids alone (`0x4e8fa0`).
fn record_state(group: &GroupState, guid: u64, owner: u64, exists: bool) -> UnitState {
    let record = group.pet_record(guid);
    UnitState {
        exists,
        guid,
        owner: if exists { owner } else { 0 },
        name: record.map(|r| r.pet_name.clone().unwrap_or_default()),
        health: record.map_or(0, |r| u32::from(r.pet_cur_hp.unwrap_or(0))),
        max_health: record.map_or(0, |r| u32::from(r.pet_max_hp.unwrap_or(0))),
        power_type: record.map_or(0, |r| r.shown_pet_power_type()),
        power: record.map_or(0, |r| r.shown_pet_power()),
        max_power: record.map_or(0, |r| r.shown_pet_max_power()),
        is_connected: record.is_some(),
        ..Default::default()
    }
}

/// Every pet token's last pushed snapshot: `partypet1..4` and `raidpet1..40`.
#[derive(Default)]
pub(super) struct FedPets {
    pub(super) party: [Option<UnitState>; 4],
    pub(super) raid: Vec<Option<UnitState>>,
}

/// Push one pet token's snapshot when it moved.
pub(super) fn feed_token(
    script: &mut UiScript,
    gate: &gate::Gate,
    fed: &mut Option<UnitState>,
    token: &str,
    now: Option<UnitState>,
) {
    if *fed != now {
        gate.audit("feed_party", "a group pet-token snapshot");
        script.set_unit(token, now.clone());
        *fed = now;
    }
}

#[cfg(test)]
pub(in crate::ui_party) mod tests {
    use super::*;
    use crate::net::{FieldChanged, Guid};
    use benilla_protocol::messages::{
        member_status, GroupMemberEntry, ObjectFields, PartyMemberStatsInfo,
    };

    /// Unit descriptor field indices, build 5875: the pair a member's `SUMMON` holds, and a pet's
    /// health, maximum health and pet number.
    const SUMMON: u16 = 8;
    const HEALTH: u16 = 22;
    const MAXHEALTH: u16 = 28;
    const PETNUMBER: u16 = 139;

    pub(in crate::ui_party) const ME: u64 = 0x10;

    /// The `i`-th party member, and its pet.
    pub(in crate::ui_party) fn member(i: u64) -> u64 {
        0x1000 + i
    }
    fn pet(i: u64) -> u64 {
        0xF140_0000_0000_0000 + i
    }

    fn store(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(pairs))
    }

    fn summon(guid: u64) -> [(u16, u32); 2] {
        [(SUMMON, guid as u32), (SUMMON + 1, (guid >> 32) as u32)]
    }

    /// The frames of a group feed running [`super::super::feed::feed_party`] on a VM whose one
    /// spy frame logs the unit events pets raise, as `"EVENT:token"`.
    pub(in crate::ui_party) fn app_with(script: UiScript) -> App {
        let mut app = App::new();
        app.add_message::<FieldChanged>()
            .init_resource::<GroupState>()
            .init_resource::<crate::ui_party::calls::GroupEdits>()
            .init_resource::<GuidIndex>()
            .init_resource::<NameCache>()
            .init_resource::<crate::ui_guild::GuildState>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>();
        let (tx, rx) = crossbeam_channel::unbounded();
        // Kept alive for the run: a dropped receiver would fail the name queries.
        std::mem::forget(rx);
        app.insert_resource(NetCommands(tx));
        benilla_world::world_point::init_world_point_resources(app.world_mut());
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                for _, e in ipairs({ "UNIT_PET", "UNIT_HEALTH", "UNIT_MAXHEALTH", "UNIT_MANA",
                                     "UNIT_NAME_UPDATE" }) do
                    f:RegisterEvent(e)
                end
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, event .. ":" .. tostring(arg1))
                end)
                "#,
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.add_systems(Update, super::super::feed::feed_party);
        app
    }

    pub(in crate::ui_party) fn app() -> App {
        app_with(UiScript::new().unwrap())
    }

    /// A party of `n`, each `online` on the wire and in its record, the record naming pet `i`.
    pub(in crate::ui_party) fn party(app: &mut App, n: u64, online: bool) {
        let status = if online {
            member_status::ONLINE
        } else {
            member_status::OFFLINE
        };
        let list = (1..=n)
            .map(|i| GroupMemberEntry {
                name: format!("M{i}"),
                guid: member(i),
                status,
                flags: 0,
            })
            .collect();
        let mut group = app.world_mut().resource_mut::<GroupState>();
        group.apply_list(0, 0, list, ME, None, Some(ME));
        for i in 1..=n {
            group.apply_stats(
                member(i),
                true,
                PartyMemberStatsInfo {
                    status: Some(status),
                    cur_hp: Some(2000),
                    max_hp: Some(3000),
                    pet_guid: Some(pet(i)),
                    pet_name: Some("Whelp".into()),
                    pet_cur_hp: Some(400),
                    pet_max_hp: Some(500),
                    pet_power_type: Some(0),
                    pet_cur_power: Some(30),
                    pet_max_power: Some(60),
                    ..Default::default()
                },
            );
        }
    }

    /// `SMSG_GROUP_LIST` re-sent with only the members' status bytes moved: members `1..` of the
    /// party or raid, one per status.
    fn relist(app: &mut App, group_type: u8, statuses: &[u8]) {
        let list = statuses
            .iter()
            .enumerate()
            .map(|(i, &status)| GroupMemberEntry {
                name: format!("M{}", i + 1),
                guid: member(i as u64 + 1),
                status,
                flags: 0,
            })
            .collect();
        app.world_mut().resource_mut::<GroupState>().apply_list(
            group_type,
            0,
            list,
            ME,
            None,
            Some(ME),
        );
    }

    /// Hold `guid` in the object manager.
    pub(in crate::ui_party) fn stream(app: &mut App, guid: u64, pairs: &[(u16, u32)]) -> Entity {
        let e = app.world_mut().spawn((Guid(guid), store(pairs))).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(guid, e);
        e
    }

    fn set_fields(app: &mut App, e: Entity, pairs: &[(u16, u32)]) {
        app.world_mut()
            .entity_mut(e)
            .get_mut::<ObjectStore>()
            .unwrap()
            .0
            .merge(ObjectFields::from_pairs(pairs));
    }

    /// Run a frame and take the pet events it fired; `UNIT_PET`, or a unit event naming a pet.
    pub(in crate::ui_party) fn frame(app: &mut App) -> Vec<String> {
        app.update();
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.resolve();
        let seen: Vec<String> = s.eval("return SEEN").unwrap();
        s.run("SEEN = {}").unwrap();
        seen.into_iter()
            .filter(|e| e.starts_with("UNIT_PET:") || e.contains("pet"))
            .collect()
    }

    /// A number the VM answers, for the getters.
    fn num(app: &mut App, code: &str) -> f64 {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval(&format!("return {code}"))
            .unwrap()
    }

    /// A string or nil the VM answers.
    fn text(app: &mut App, code: &str) -> Option<String> {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval(&format!("return {code}"))
            .unwrap()
    }

    /// A predicate answers the number 1 or nil, never a boolean.
    pub(in crate::ui_party) fn flag(app: &mut App, code: &str) -> Option<f64> {
        app.world_mut()
            .non_send_resource::<UiScript>()
            .eval(&format!("return {code}"))
            .unwrap()
    }

    /// A pet the object manager holds is a live unit: `UnitExists` and `UnitIsVisible` answer 1,
    /// and the getters read its descriptor and its cached name.
    #[test]
    fn a_held_party_pet_reads_its_own_descriptor() {
        let mut app = app();
        party(&mut app, 1, true);
        stream(&mut app, member(1), &summon(pet(1)));
        stream(
            &mut app,
            pet(1),
            &[(HEALTH, 900), (MAXHEALTH, 1000), (PETNUMBER, 4)],
        );
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_pet(4, "Fang".into());
        frame(&mut app);

        assert_eq!(flag(&mut app, "UnitExists('partypet1')"), Some(1.0));
        assert_eq!(flag(&mut app, "UnitIsVisible('partypet1')"), Some(1.0));
        assert_eq!(flag(&mut app, "UnitIsConnected('partypet1')"), Some(1.0));
        assert_eq!(
            text(&mut app, "UnitName('partypet1')").as_deref(),
            Some("Fang")
        );
        assert_eq!(num(&mut app, "UnitHealth('partypet1')"), 900.0);
        assert_eq!(num(&mut app, "UnitHealthMax('partypet1')"), 1000.0);
        assert_eq!(
            flag(&mut app, "UnitExists('partypet2')"),
            None,
            "no second member"
        );
        assert_eq!(
            flag(&mut app, "UnitExists('party1')"),
            Some(1.0),
            "the member is unchanged"
        );
    }

    /// A pet the object manager does not hold answers from its online owner's record, as the
    /// getters fall through the party record to the pet record (`0x496420`): `UnitExists` is 1
    /// through the roster fallback with no object, `UnitIsVisible` is nil, and `UnitLevel` has no
    /// pet leg.
    #[test]
    fn an_unheld_party_pet_reads_its_online_owners_record() {
        let mut app = app();
        party(&mut app, 1, true);
        // A rage pet: the record's power is raw, and divides at the read as a live unit's does.
        app.world_mut().resource_mut::<GroupState>().apply_stats(
            member(1),
            false,
            PartyMemberStatsInfo {
                pet_power_type: Some(1),
                pet_cur_power: Some(570),
                pet_max_power: Some(1000),
                ..Default::default()
            },
        );
        frame(&mut app);

        assert_eq!(flag(&mut app, "UnitExists('partypet1')"), Some(1.0));
        assert_eq!(
            flag(&mut app, "UnitIsVisible('partypet1')"),
            None,
            "no object"
        );
        assert_eq!(
            text(&mut app, "UnitName('partypet1')").as_deref(),
            Some("Whelp")
        );
        assert_eq!(num(&mut app, "UnitHealth('partypet1')"), 400.0);
        assert_eq!(num(&mut app, "UnitHealthMax('partypet1')"), 500.0);
        assert_eq!(num(&mut app, "UnitPowerType('partypet1')"), 1.0);
        assert_eq!(num(&mut app, "UnitMana('partypet1')"), 57.0);
        assert_eq!(num(&mut app, "UnitManaMax('partypet1')"), 100.0);
        assert_eq!(
            num(&mut app, "UnitLevel('partypet1')"),
            0.0,
            "`UnitLevel` has no pet leg"
        );
        assert_eq!(
            flag(&mut app, "UnitIsConnected('partypet1')"),
            Some(1.0),
            "existence alone (`0x517dfb`)"
        );
        assert_eq!(
            flag(&mut app, "UnitPlayerOrPetInParty('partypet1')"),
            Some(1.0),
            "the roster names it by guid alone (`0x4e8fa0`)"
        );
        for dead in ["UnitIsDead", "UnitIsGhost", "UnitIsDeadOrGhost"] {
            assert_eq!(
                flag(&mut app, &format!("{dead}('partypet1')")),
                None,
                "{dead}"
            );
        }
    }

    /// The reported shape: the record names the member's old pet from a stats packet; in view they
    /// call another and walk out of range before its name is known. The snapshot takes the new
    /// pet's name from `0x609210` (`0x5f0a63`), which misses and answers `UNKNOWNOBJECT`
    /// (`0x609324`), and copies that (`0x5f0a70`): the old pet's name is not kept for the new one.
    #[test]
    fn a_member_leaving_view_with_an_unnamed_new_pet_names_it_unknownobject() {
        let mut app = app();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(r#"UNKNOWNOBJECT = "Unknown""#)
            .unwrap();
        party(&mut app, 1, true);
        let owner = stream(&mut app, member(1), &summon(pet(2)));
        let new_pet = stream(
            &mut app,
            pet(2),
            &[(HEALTH, 900), (MAXHEALTH, 1000), (PETNUMBER, 7)],
        );
        frame(&mut app);
        assert_eq!(
            text(&mut app, "UnitName('partypet1')").as_deref(),
            Some("Unknown"),
            "held, its name not yet answered"
        );

        let fields = |app: &App, e: Entity| app.world().get::<ObjectStore>(e).unwrap().clone();
        let (store, pet_store) = (fields(&app, owner), fields(&app, new_pet));
        app.world_mut()
            .resource_scope(|world, mut group: Mut<GroupState>| {
                super::super::net::member_deactivated(
                    member(1),
                    &mut group,
                    Some(&store),
                    |g| (g == pet(2)).then_some(&pet_store),
                    world.resource::<NameCache>(),
                    world.get_non_send_resource::<UiScript>(),
                    world.resource::<NetCommands>(),
                );
            });
        for (guid, e) in [(member(1), owner), (pet(2), new_pet)] {
            app.world_mut().despawn(e);
            app.world_mut().resource_mut::<GuidIndex>().0.remove(&guid);
        }
        frame(&mut app);
        assert_eq!(
            text(&mut app, "UnitName('partypet1')").as_deref(),
            Some("Unknown"),
            "out of view, the record's copy of the miss, not the old pet's \"Whelp\""
        );
        assert_eq!(num(&mut app, "UnitHealth('partypet1')"), 900.0);
    }

    /// A pet is found only behind its owner's online bit (`0x4e8227`, `0x4e8884`): an offline
    /// member's record names nobody.
    #[test]
    fn an_offline_owners_party_pet_is_nobody() {
        let mut app = app();
        party(&mut app, 1, false);
        frame(&mut app);

        assert_eq!(flag(&mut app, "UnitExists('partypet1')"), None);
        assert_eq!(text(&mut app, "UnitName('partypet1')"), None);
        assert_eq!(num(&mut app, "UnitHealth('partypet1')"), 0.0);
        assert_eq!(flag(&mut app, "UnitIsConnected('partypet1')"), None);
    }

    /// The feed pushes every pet token's move and fires nothing: a record's new or dismissed pet, a
    /// held member's `SUMMON`, a held pet's health. Their `UNIT_PET` and unit events are the stats
    /// packet's record diff (`super::super::records::record_notices`) and the descriptor's
    /// watches (`crate::field_watch`).
    #[test]
    fn the_pet_tokens_are_pushed_and_the_feed_fires_nothing() {
        let mut app = app();
        party(&mut app, 1, true);
        assert!(frame(&mut app).is_empty(), "a first look");
        app.world_mut().resource_mut::<GroupState>().apply_stats(
            member(1),
            false,
            PartyMemberStatsInfo {
                pet_guid: Some(pet(9)),
                pet_cur_hp: Some(250),
                ..Default::default()
            },
        );
        assert!(frame(&mut app).is_empty(), "a record's new pet");
        assert_eq!(num(&mut app, "UnitHealth('partypet1')"), 250.0);
        app.world_mut()
            .resource_mut::<GroupState>()
            .stats
            .get_mut(&member(1))
            .unwrap()
            .pet_guid = None;
        assert!(frame(&mut app).is_empty(), "dismissed");
        assert_eq!(flag(&mut app, "UnitExists('partypet1')"), None);

        let owner = stream(&mut app, member(1), &summon(pet(1)));
        let held = stream(&mut app, pet(1), &[(HEALTH, 900), (MAXHEALTH, 1000)]);
        assert!(frame(&mut app).is_empty(), "held");
        set_fields(&mut app, held, &[(HEALTH, 700)]);
        assert!(frame(&mut app).is_empty(), "a held pet's health");
        assert_eq!(num(&mut app, "UnitHealth('partypet1')"), 700.0);
        set_fields(&mut app, owner, &summon(0));
        assert!(frame(&mut app).is_empty(), "a held member's summon");
        assert_eq!(flag(&mut app, "UnitExists('partypet1')"), None);
    }

    /// The reported shape: a member with a pet logs off out of view, and the server sends the list
    /// again with their status alone moved. The list rewrites the record's online bit
    /// (`0x4e8361`-`0x4e837b`), so the pet is nobody at once (`0x4e8227`) and comes back with the
    /// next list; no list reaches the stats diff (`0x5e5510`'s callers are `0x5e54a8` and `0x5f0c1c`), so
    /// neither fires `UNIT_PET`.
    #[test]
    fn a_status_only_list_takes_an_out_of_view_members_pet_and_fires_no_unit_pet() {
        let mut app = app();
        party(&mut app, 2, true);
        frame(&mut app);
        assert_eq!(flag(&mut app, "UnitExists('partypet1')"), Some(1.0));

        relist(
            &mut app,
            0,
            &[member_status::OFFLINE, member_status::ONLINE],
        );
        let offline = frame(&mut app);
        assert!(
            !offline.iter().any(|e| e.starts_with("UNIT_PET:")),
            "{offline:?}"
        );
        assert_eq!(flag(&mut app, "UnitExists('partypet1')"), None);
        assert_eq!(text(&mut app, "UnitName('partypet1')"), None);
        assert_eq!(num(&mut app, "UnitHealth('partypet1')"), 0.0);
        assert_eq!(
            flag(&mut app, "UnitExists('partypet2')"),
            Some(1.0),
            "the member still listed online keeps theirs"
        );
        assert_eq!(
            app.world().resource::<GroupState>().stats[&member(1)].pet_guid,
            Some(pet(1)),
            "the list keeps the rest of the record"
        );

        relist(&mut app, 0, &[member_status::ONLINE, member_status::ONLINE]);
        let online = frame(&mut app);
        assert!(
            !online.iter().any(|e| e.starts_with("UNIT_PET:")),
            "{online:?}"
        );
        assert_eq!(flag(&mut app, "UnitExists('partypet1')"), Some(1.0));
        assert_eq!(
            text(&mut app, "UnitName('partypet1')").as_deref(),
            Some("Whelp")
        );
        assert_eq!(num(&mut app, "UnitHealth('partypet1')"), 400.0);
    }

    /// A raid's rows take the same list: the record's bit moves, the offline row's pet does not
    /// exist (`0x4bb040`) and its party slot's is nobody, and no `UNIT_PET` fires for either token.
    #[test]
    fn a_status_only_raid_list_takes_an_offline_rows_pet_and_fires_no_unit_pet() {
        let mut app = app();
        party(&mut app, 2, true);
        let (on, off) = (member_status::ONLINE, member_status::OFFLINE);
        relist(&mut app, super::super::GROUPTYPE_RAID, &[on, on]);
        frame(&mut app);
        assert_eq!(flag(&mut app, "UnitExists('raidpet2')"), Some(1.0));

        relist(&mut app, super::super::GROUPTYPE_RAID, &[on, off]);
        let offline = frame(&mut app);
        assert!(
            !offline.iter().any(|e| e.starts_with("UNIT_PET:")),
            "{offline:?}"
        );
        assert_eq!(flag(&mut app, "UnitExists('raidpet2')"), None);
        assert_eq!(num(&mut app, "UnitHealth('raidpet2')"), 0.0);
        assert_eq!(flag(&mut app, "UnitExists('partypet2')"), None);
        assert_eq!(
            flag(&mut app, "UnitExists('raidpet1')"),
            Some(1.0),
            "the row listed online keeps its pet"
        );

        relist(&mut app, super::super::GROUPTYPE_RAID, &[on, on]);
        let online = frame(&mut app);
        assert!(
            !online.iter().any(|e| e.starts_with("UNIT_PET:")),
            "{online:?}"
        );
        assert_eq!(flag(&mut app, "UnitExists('raidpet2')"), Some(1.0));
        assert_eq!(flag(&mut app, "UnitExists('partypet2')"), Some(1.0));
    }

    /// A raid's pets are its rows' (`0x491960`): the record leg has no online test at the
    /// resolver, but `UnitExists`' roster fallback (`0x4bb040`) has one, so an offline member's
    /// pet resolves to a guid the client never holds and does not exist.
    #[test]
    fn raid_pets_follow_their_rows_and_an_offline_rows_pet_does_not_exist() {
        let mut app = app();
        party(&mut app, 2, true);
        app.world_mut().resource_mut::<GroupState>().apply_list(
            super::super::GROUPTYPE_RAID,
            0,
            (1..=2)
                .map(|i| GroupMemberEntry {
                    name: format!("M{i}"),
                    guid: member(i),
                    status: member_status::ONLINE,
                    flags: 0,
                })
                .collect(),
            ME,
            None,
            Some(ME),
        );
        // Row 2 has gone offline.
        app.world_mut()
            .resource_mut::<GroupState>()
            .stats
            .get_mut(&member(2))
            .unwrap()
            .status = Some(member_status::OFFLINE);
        let first = frame(&mut app);
        assert!(
            !first.iter().any(|e| e.starts_with("UNIT_PET:")),
            "{first:?}"
        );

        assert_eq!(flag(&mut app, "UnitExists('raidpet1')"), Some(1.0));
        assert_eq!(
            text(&mut app, "UnitName('raidpet1')").as_deref(),
            Some("Whelp")
        );
        assert_eq!(num(&mut app, "UnitHealth('raidpet1')"), 400.0);
        assert_eq!(
            flag(&mut app, "UnitExists('raidpet2')"),
            None,
            "an offline row's pet"
        );
        assert_eq!(num(&mut app, "UnitHealth('raidpet2')"), 0.0);
        assert_eq!(
            flag(&mut app, "UnitIsUnit('raidpet1', 'partypet1')"),
            Some(1.0),
            "a row and its party slot name one pet"
        );
    }

    /// The stock party frame, loaded whole off the player's own chain, shows a member's pet frame
    /// once `UnitExists("partypet1")` answers (`PartyMemberFrame.lua:73`), and hides it on the
    /// `UNIT_PET` that names the member (`PartyMemberFrame.lua:189`).
    #[test]
    fn the_stock_party_frame_shows_and_hides_its_pet_frame() {
        benilla_formats::wow_data_or_skip!();
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        let failures = crate::ui_script::load_default_ui(&script);
        assert!(failures.is_empty(), "load failures: {failures:?}");
        let mut app = app_with(script);
        assert_eq!(
            text(&mut app, "SHOW_PARTY_PETS").as_deref(),
            Some("1"),
            "the stock default the pet frame gates on"
        );
        assert_eq!(flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"), None);

        party(&mut app, 1, true);
        frame(&mut app);
        assert_eq!(
            flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"),
            Some(1.0),
            "an online member with a pet the client does not hold"
        );
        assert_eq!(
            flag(&mut app, "PartyMemberFrame2PetFrame:IsShown()"),
            None,
            "and no other slot's"
        );
        // The frame's portrait binds the token the portrait booths bake for it.
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.resolve();
        let bound: Vec<String> = script
            .extract()
            .into_iter()
            .filter_map(|q| match q.content {
                benilla_ui::script::QuadContent::Texture {
                    portrait_unit: Some(unit),
                    ..
                } => Some(unit),
                _ => None,
            })
            .filter(|unit| unit.starts_with("partypet"))
            .collect();
        assert_eq!(
            bound,
            ["partypet1"],
            "the pet frame draws the bake of its own token"
        );

        // The pet is dismissed by a stats packet: the token is pushed, which hides nothing, and
        // the record diff's `UNIT_PET` names the member (`0x5e584f`), on which the frame hides.
        let old = app.world().resource::<GroupState>().stats[&member(1)].clone();
        app.world_mut().resource_mut::<GroupState>().apply_stats(
            member(1),
            false,
            PartyMemberStatsInfo {
                pet_guid: Some(0),
                ..Default::default()
            },
        );
        let new = app.world().resource::<GroupState>().stats[&member(1)].clone();
        frame(&mut app);
        assert_eq!(
            flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"),
            Some(1.0),
            "the push alone"
        );
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_unit_guids(&benilla_ui::script::UnitGuids {
                player: ME,
                party: [member(1), 0, 0, 0],
                ..Default::default()
            });
            for notice in super::super::records::record_notices(member(1), &old, &new) {
                crate::ui_unit::field_events::fire_unit_notice(&mut script, notice);
            }
        }
        assert_eq!(flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"), None);
        let errors = app.world().non_send_resource::<UiScript>().errors();
        assert!(errors.is_empty(), "script errors: {errors:?}");
    }

    /// The resolver's inputs for the creature-type tests: Cat Form's row, and entry 69's template
    /// a Demon (an imp), so a pet's type is told from the race's Humanoid.
    fn seed_creature_types(app: &mut App) {
        app.insert_resource(crate::target::CreatureTypes::korean_for_tests());
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.forms.insert(
            CAT_FORM,
            benilla_formats::ShapeshiftForm {
                creature_type: 1,
                ..Default::default()
            },
        );
        app.insert_resource(spells);
        app.world_mut().resource_mut::<NameCache>().insert_creature(
            69,
            Some(crate::names::CreatureRecord {
                name: "Imp".into(),
                subname: None,
                creature_type: 3,
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

    /// `UNIT_FIELD_BYTES_0` and `_1`, `OBJECT_FIELD_ENTRY`, a Night Elf and Cat Form.
    const BYTES_0: u16 = 36;
    const BYTES_1: u16 = 138;
    const OBJECT_FIELD_ENTRY: u16 = 3;
    const NIGHT_ELF: u32 = 4;
    const CAT_FORM: u32 = 1;

    /// `UnitCreatureType` on the party's tokens goes through the one resolver (`0x605570`): a
    /// held member in Cat Form is a Beast and an unshifted one a Humanoid by race, a held pet
    /// answers its template's type, and a member the object manager does not hold answers nil,
    /// as the token resolver finds no object for it (`0x51a2b1`).
    #[test]
    fn party_tokens_read_the_creature_type_through_the_resolver() {
        let mut app = app();
        seed_creature_types(&mut app);
        party(&mut app, 2, true);
        let fields = |form: u32| {
            [
                &[(BYTES_0, NIGHT_ELF), (BYTES_1, form << 16)][..],
                &summon(pet(1)),
            ]
            .concat()
        };
        let druid = stream(&mut app, member(1), &fields(CAT_FORM));
        stream(&mut app, pet(1), &[(OBJECT_FIELD_ENTRY, 69)]);
        frame(&mut app);

        let creature_type =
            |app: &mut App, token: &str| text(app, &format!("UnitCreatureType('{token}')"));
        assert_eq!(creature_type(&mut app, "party1").as_deref(), Some("야수"));
        assert_eq!(
            creature_type(&mut app, "partypet1").as_deref(),
            Some("악마"),
            "the pet's template"
        );
        assert_eq!(
            creature_type(&mut app, "party2"),
            None,
            "a member the object manager does not hold"
        );

        set_fields(&mut app, druid, &[(BYTES_1, 0)]);
        frame(&mut app);
        assert_eq!(
            creature_type(&mut app, "party1").as_deref(),
            Some("인간형"),
            "out of form: the race's type"
        );
    }

    /// The raid rows read the same resolver, our own row off our descriptor.
    #[test]
    fn raid_tokens_read_the_creature_type_through_the_resolver() {
        let mut app = app();
        seed_creature_types(&mut app);
        party(&mut app, 1, true);
        app.world_mut().resource_mut::<GroupState>().apply_list(
            super::super::GROUPTYPE_RAID,
            0,
            vec![GroupMemberEntry {
                name: "M1".into(),
                guid: member(1),
                status: member_status::ONLINE,
                flags: 0,
            }],
            ME,
            None,
            Some(ME),
        );
        let me = app
            .world_mut()
            .spawn((
                crate::net::SelfPlayer,
                Guid(ME),
                store(&[(BYTES_0, NIGHT_ELF), (BYTES_1, CAT_FORM << 16)]),
            ))
            .id();
        stream(
            &mut app,
            member(1),
            &[(BYTES_0, NIGHT_ELF), (BYTES_1, CAT_FORM << 16)],
        );
        frame(&mut app);

        let creature_type =
            |app: &mut App, token: &str| text(app, &format!("UnitCreatureType('{token}')"));
        assert_eq!(
            creature_type(&mut app, "raid2").as_deref(),
            Some("야수"),
            "our own row, after the member's"
        );
        assert_eq!(
            creature_type(&mut app, "raid1").as_deref(),
            Some("야수"),
            "a held member's row"
        );
        set_fields(&mut app, me, &[(BYTES_1, 0)]);
        frame(&mut app);
        assert_eq!(
            creature_type(&mut app, "raid2").as_deref(),
            Some("인간형"),
            "out of form: our race's type"
        );
    }
}
