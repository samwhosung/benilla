//! The group pets' unit tokens, `partypet1..4` and `raidpet1..40`: each member's pet as the
//! per-token snapshot the `Unit*` getters read, and the `UNIT_PET` edge that tells a party frame
//! its member's pet came, went or changed. [`super::feed`] drives it from the party feed's gate.
//!
//! A pet the object manager holds is an ordinary live unit. One it does not hold is answered from
//! the owner's roster record, the way the reference's getters fall through: the live descriptor,
//! then the member record, then the pet record (`0x496420`). `UnitExists` answers 1 for it through
//! the roster fallback (`0x491900`), which needs no object, and `UnitLevel`, `UnitIsDead` and
//! `UnitIsGhost` have no pet leg, so they read as they do for a unit the client knows nothing of.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript, UnitState};

use crate::names::NameCache;
use crate::net::{FieldEdges, GuidIndex, NetCommands, ObjectStore};
use crate::ui_script::gate;

use super::GroupState;

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
pub(super) struct Names<'w> {
    cache: Res<'w, NameCache>,
    commands: Res<'w, NetCommands>,
}

impl std::ops::Deref for Names<'_> {
    type Target = NameCache;

    fn deref(&self) -> &NameCache {
        &self.cache
    }
}

/// What the pets' snapshots read: the object manager and the name cache.
pub(super) struct Lookup<'a, 'w, 's, 'q> {
    index: &'a GuidIndex,
    stores: &'a Query<'w, 's, &'q ObjectStore>,
    names: &'a NameCache,
    commands: &'a NetCommands,
}

impl<'a, 'w, 's, 'q> Lookup<'a, 'w, 's, 'q> {
    pub(super) fn new(
        index: &'a GuidIndex,
        stores: &'a Query<'w, 's, &'q ObjectStore>,
        names: &'a Names,
    ) -> Self {
        Self {
            index,
            stores,
            names: &names.cache,
            commands: &names.commands,
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

/// One member's pet: the guid its token names and the snapshot the getters read, or `None` for a
/// member with no pet the roster or the descriptor names.
pub(super) fn member_pet(
    look: &Lookup<'_, '_, '_, '_>,
    group: &GroupState,
    owner: u64,
    held: Option<&ObjectStore>,
    roster: Roster,
) -> Option<(u64, UnitState)> {
    let guid = pet_guid(group, owner, held, roster)?;
    let mut state = match look.store(guid) {
        // A held pet is a live unit like any: every getter reads its descriptor (`0x468460`), and
        // `UnitIsConnected` answers 1 for any object (`0x517daf`), as `snapshot` does. No
        // reaction: the pet frame reads none, as the `"pet"` token's.
        Some(store) => {
            let name = look
                .names
                .resolve_unit(guid, Some(store), look.commands)
                .map(str::to_string);
            crate::ui_unit::snapshot(store, guid, name, 0, None)
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
    Some((guid, state))
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

/// What one pet token last pushed: the owner and the pet guid it named, `UNIT_PET`'s trigger, and
/// the snapshot, [`fire_transitions`](crate::ui_unit::fire_transitions)' per-field diff.
#[derive(Default)]
pub(super) struct FedPet {
    owner: u64,
    pet: Option<u64>,
    state: Option<UnitState>,
}

/// Every pet token's memo: `partypet1..4` and `raidpet1..40`.
#[derive(Default)]
pub(super) struct FedPets {
    pub(super) party: [FedPet; 4],
    pub(super) raid: Vec<FedPet>,
}

/// Push one pet token's snapshot and fire what moved, in the reference's order (`0x5e5720`):
/// `UNIT_PET` for the owner's token, then the pet's own `UNIT_*` events for its own.
///
/// `UNIT_PET` fires when the same owner's pet guid changes: a pet arriving, going or being
/// replaced, off the record or the descriptor alike. An owner that changes is the roster's edge
/// (`PARTY_MEMBERS_CHANGED`), which the party frame answers by rereading, and the first look is
/// no edge. `owner_token` is `None` where the owner's own feed fires it.
pub(super) fn feed_token(
    script: &mut UiScript,
    gate: &gate::Gate,
    edges: &FieldEdges,
    fed: &mut FedPet,
    token: &str,
    owner_token: Option<&str>,
    owner: u64,
    now: Option<(u64, UnitState)>,
) {
    let (pet, state) = match now {
        Some((guid, state)) => (Some(guid), Some(state)),
        None => (None, None),
    };
    let moved = owner != 0 && fed.owner == owner && fed.pet != pet;
    let changed = fed.state != state;
    if changed {
        gate.audit("feed_party", "a group pet-token snapshot");
        script.set_unit(token, state.clone());
    }
    if let Some(owner_token) = owner_token.filter(|_| moved) {
        gate.audit("feed_party", "a group pet edge");
        script.fire_event("UNIT_PET", vec![ScriptValue::Str(owner_token.to_string())]);
    }
    if let Some(cur) = state.as_ref().filter(|_| changed) {
        crate::ui_unit::fire_transitions(script, token, fed.state.as_ref(), cur, edges);
    }
    *fed = FedPet { owner, pet, state };
}

#[cfg(test)]
mod tests {
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

    const ME: u64 = 0x10;

    /// The `i`-th party member, and its pet.
    fn member(i: u64) -> u64 {
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
    fn app_with(script: UiScript) -> App {
        let mut app = App::new();
        app.add_message::<FieldChanged>()
            .init_resource::<GroupState>()
            .init_resource::<GuidIndex>()
            .init_resource::<NameCache>()
            .init_resource::<crate::ui_chat::ChatLog>();
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

    fn app() -> App {
        app_with(UiScript::new().unwrap())
    }

    /// A party of `n`, each `online` on the wire and in its record, the record naming pet `i`.
    fn party(app: &mut App, n: u64, online: bool) {
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

    /// Hold `guid` in the object manager.
    fn stream(app: &mut App, guid: u64, pairs: &[(u16, u32)]) -> Entity {
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
    fn frame(app: &mut App) -> Vec<String> {
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
    fn flag(app: &mut App, code: &str) -> Option<f64> {
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

    /// `PARTY_MEMBERS_CHANGED` and first sight announce nothing about pets: `UNIT_PET` fires when
    /// the same member's pet guid moves, off the record.
    #[test]
    fn unit_pet_fires_with_the_owners_token_when_the_records_pet_changes() {
        let mut app = app();
        party(&mut app, 1, true);
        let first = frame(&mut app);
        assert!(
            !first.iter().any(|e| e.starts_with("UNIT_PET:")),
            "the first look is no edge: {first:?}"
        );
        assert!(frame(&mut app).is_empty(), "a steady frame is silent");

        // A new pet.
        app.world_mut().resource_mut::<GroupState>().apply_stats(
            member(1),
            false,
            PartyMemberStatsInfo {
                pet_guid: Some(pet(9)),
                ..Default::default()
            },
        );
        let changed = frame(&mut app);
        assert_eq!(
            changed.iter().filter(|e| *e == "UNIT_PET:party1").count(),
            1,
            "{changed:?}"
        );
        // Dismissed.
        app.world_mut()
            .resource_mut::<GroupState>()
            .stats
            .get_mut(&member(1))
            .unwrap()
            .pet_guid = None;
        assert!(frame(&mut app).contains(&"UNIT_PET:party1".to_string()));
        assert_eq!(
            flag(&mut app, "UnitExists('partypet1')"),
            None,
            "and the token clears"
        );
        // Back.
        app.world_mut()
            .resource_mut::<GroupState>()
            .stats
            .get_mut(&member(1))
            .unwrap()
            .pet_guid = Some(pet(1));
        assert!(frame(&mut app).contains(&"UNIT_PET:party1".to_string()));
        assert_eq!(flag(&mut app, "UnitExists('partypet1')"), Some(1.0));
    }

    /// The same edge off a held member's descriptor (`SUMMON`), where no stats packet moves.
    #[test]
    fn unit_pet_fires_when_a_held_members_summon_changes() {
        let mut app = app();
        party(&mut app, 1, true);
        let owner = stream(&mut app, member(1), &summon(pet(1)));
        frame(&mut app);

        set_fields(&mut app, owner, &summon(pet(2)));
        assert!(frame(&mut app).contains(&"UNIT_PET:party1".to_string()));
        set_fields(&mut app, owner, &summon(0));
        assert!(frame(&mut app).contains(&"UNIT_PET:party1".to_string()));
        assert_eq!(flag(&mut app, "UnitExists('partypet1')"), None);
    }

    /// A held pet's health moving fires the unit event with the pet's own token, which its frame's
    /// bar listens to. The pet's store is the only input that moved, so its gate input carries it.
    #[test]
    fn unit_health_fires_with_the_pets_token_when_its_health_changes() {
        let mut app = app();
        party(&mut app, 1, true);
        stream(&mut app, member(1), &summon(pet(1)));
        let held = stream(&mut app, pet(1), &[(HEALTH, 900), (MAXHEALTH, 1000)]);
        frame(&mut app);
        assert!(frame(&mut app).is_empty(), "a steady frame is silent");

        set_fields(&mut app, held, &[(HEALTH, 700)]);
        assert_eq!(frame(&mut app), ["UNIT_HEALTH:partypet1"]);
        assert_eq!(num(&mut app, "UnitHealth('partypet1')"), 700.0);
        set_fields(&mut app, held, &[(MAXHEALTH, 1200)]);
        assert_eq!(frame(&mut app), ["UNIT_MAXHEALTH:partypet1"]);
    }

    /// The same off the record, where a stats packet moves it.
    #[test]
    fn unit_health_fires_with_the_pets_token_when_the_records_pet_health_changes() {
        let mut app = app();
        party(&mut app, 1, true);
        frame(&mut app);

        app.world_mut().resource_mut::<GroupState>().apply_stats(
            member(1),
            false,
            PartyMemberStatsInfo {
                pet_cur_hp: Some(250),
                ..Default::default()
            },
        );
        assert_eq!(frame(&mut app), ["UNIT_HEALTH:partypet1"]);
        assert_eq!(num(&mut app, "UnitHealth('partypet1')"), 250.0);
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

        // A raid pet's arrival is announced to the row's token as well as the slot's.
        app.world_mut()
            .resource_mut::<GroupState>()
            .stats
            .get_mut(&member(1))
            .unwrap()
            .pet_guid = Some(pet(9));
        let changed = frame(&mut app);
        assert!(
            changed.contains(&"UNIT_PET:party1".to_string()),
            "{changed:?}"
        );
        assert!(
            changed.contains(&"UNIT_PET:raid1".to_string()),
            "{changed:?}"
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

        // The pet is dismissed: `UNIT_PET` names the member, and the frame hides.
        app.world_mut()
            .resource_mut::<GroupState>()
            .stats
            .get_mut(&member(1))
            .unwrap()
            .pet_guid = None;
        frame(&mut app);
        assert_eq!(flag(&mut app, "PartyMemberFrame1PetFrame:IsShown()"), None);
        let errors = app.world().non_send_resource::<UiScript>().errors();
        assert!(errors.is_empty(), "script errors: {errors:?}");
    }
}
