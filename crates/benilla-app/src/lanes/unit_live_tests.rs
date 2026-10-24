//! The `Unit*` getters read a held unit's own fields off its object as they run, through the lent
//! host, as the reference's read the object they resolve (`UnitHealth`, `0x5174d0`): the live
//! answer and the frame's snapshot agree on every getter wherever the snapshot is current, and the
//! object answers where it is not.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::GroupMemberEntry;
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, targets, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// The target, a creature that targets us.
const WOLF: u64 = 0xF130_0000_0044_0001;
/// Our pet, `UNIT_FIELD_SUMMON`'s.
const PET: u64 = 0xF140_0000_0045_0002;
/// A party member in view, and its pet.
const MATE: u64 = 0x22;
const MATE_PET: u64 = 0xF140_0000_0046_0003;
/// `UNIT_FIELD_SUMMON`, the guid the `"pet"` token reads.
const SUMMON: u16 = 8;

/// The tokens the agreement walks: every base a unit here answers to, and a `target` hop.
const TOKENS: [&str; 6] = [
    "player",
    "target",
    "targettarget",
    "pet",
    "party1",
    "partypet1",
];

/// The one-token getters of the `Unit*` family.
const GETTERS: [&str; 39] = [
    "UnitExists",
    "UnitIsVisible",
    "UnitIsTapped",
    "UnitIsTappedByPlayer",
    "UnitIsPartyLeader",
    "UnitName",
    "UnitHealth",
    "UnitHealthMax",
    "UnitLevel",
    "UnitIsCorpse",
    "UnitIsTrivial",
    "UnitIsDead",
    "UnitIsGhost",
    "UnitIsDeadOrGhost",
    "UnitIsCivilian",
    "UnitPlayerControlled",
    "UnitIsPlayer",
    "UnitIsPlusMob",
    "UnitAffectingCombat",
    "UnitPlayerOrPetInParty",
    "UnitPlayerOrPetInRaid",
    "UnitInRaid",
    "UnitInParty",
    "GetRaidTargetIndex",
    "UnitIsConnected",
    "UnitIsPVP",
    "UnitIsPVPFreeForAll",
    "UnitFactionGroup",
    "UnitRace",
    "UnitClass",
    "UnitHasRelicSlot",
    "UnitSex",
    "UnitCreatureType",
    "UnitClassification",
    "UnitPowerType",
    "UnitMana",
    "UnitManaMax",
    "UnitIsCharmed",
    "UnitXP",
];

/// The whole client, headless, in the world with its frame run.
struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    _sent: Receiver<ClientCommand>,
}

impl Client {
    /// One frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
    }

    /// Every getter's answer for every token, as one line each: through the lent host when `live`,
    /// else off the snapshots alone, as a VM with no host reads them.
    fn answers(&mut self, live: bool) -> Vec<String> {
        let chunk = format!(
            "local out = {{}}
            for _, token in ipairs({{ {tokens} }}) do
                for _, name in ipairs({{ {getters} }}) do
                    local got = {{ pcall(getglobal(name), token) }}
                    for i = 1, table.getn(got) do got[i] = tostring(got[i]) end
                    table.insert(out, token .. ' ' .. name .. ' ' .. table.concat(got, ','))
                end
            end
            return out",
            tokens = quoted(&TOKENS),
            getters = quoted(&GETTERS),
        );
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<Vec<String>>(&chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), &chunk)
        }
    }

    /// `guid`'s store, written in place with no packet or frame, as a packet's handler leaves it
    /// before the feeds have pushed.
    fn write_store(&mut self, guid: u64, pairs: &[(u16, u32)]) {
        let world = self.app.world_mut();
        let entity = world.resource::<crate::net::GuidIndex>().0[&guid];
        let mut store = world
            .get_mut::<crate::net::ObjectStore>(entity)
            .expect("a store");
        store
            .0
            .merge(benilla_protocol::ObjectFields::from_pairs(pairs));
    }
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("'{n}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The client in the world as [`ME`] with [`PET`], [`MATE`] in our party, and [`WOLF`] selected,
/// targeting us.
fn client() -> Client {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    app.insert_non_send_resource(UiScript::new().expect("a VM"));
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    let unit = |hp: u32| {
        vec![
            (FIELD_UNIT_HEALTH, hp),
            (FIELD_UNIT_MAXHEALTH, 100),
            (FIELD_UNIT_LEVEL, 12),
        ]
    };
    let roster = SessionEvent::GroupList {
        group_type: 0,
        own_flags: 0,
        members: vec![GroupMemberEntry {
            name: "Mate".into(),
            guid: MATE,
            status: 1,
            flags: 0,
        }],
        leader: ME,
        loot: None,
    };
    let mut me = unit(90);
    me.extend([(SUMMON, PET as u32), (SUMMON + 1, (PET >> 32) as u32)]);
    let mut mate = unit(60);
    mate.extend([
        (SUMMON, MATE_PET as u32),
        (SUMMON + 1, (MATE_PET >> 32) as u32),
    ]);
    let mut wolf = unit(80);
    wolf.extend(targets(ME));
    c.frame(vec![
        vec![roster],
        vec![created(ME, EntityKind::Player, &me)],
        vec![created(PET, EntityKind::Unit, &unit(70))],
        vec![created(MATE, EntityKind::Player, &mate)],
        vec![created(MATE_PET, EntityKind::Unit, &unit(50))],
        vec![created(WOLF, EntityKind::Unit, &wolf)],
    ]);
    let world = c.app.world_mut();
    let wolf = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(wolf),
        guid: Some(WOLF),
        ..Default::default()
    };
    c.frame(vec![]);
    c
}

/// Wherever the frame's snapshots are current, every getter answers the same off the live object
/// as off the snapshot, for every token: the object's fields the snapshot holds are the ones it
/// reads ([`crate::ui_unit::unit_object`]), with nothing a token's own build changes after.
#[test]
fn every_getter_answers_the_same_live_as_off_the_snapshot() {
    let mut c = client();
    c.frame(vec![
        vec![values(WOLF, &[(FIELD_UNIT_HEALTH, 40)])],
        vec![values(MATE, &[(FIELD_UNIT_HEALTH, 30)])],
        vec![values(MATE_PET, &[(FIELD_UNIT_HEALTH, 25)])],
        vec![values(PET, &[(FIELD_UNIT_HEALTH, 35)])],
        vec![values(ME, &[(FIELD_UNIT_HEALTH, 20)])],
    ]);
    let snapshot = c.answers(false);
    let live = c.answers(true);
    assert_eq!(snapshot.len(), TOKENS.len() * GETTERS.len());
    for token in TOKENS {
        assert!(
            snapshot
                .iter()
                .any(|l| l.starts_with(&format!("{token} UnitExists true,1"))),
            "{token} names a unit: {snapshot:?}"
        );
    }
    let differ: Vec<(&String, &String)> =
        snapshot.iter().zip(&live).filter(|(s, l)| s != l).collect();
    assert!(differ.is_empty(), "snapshot vs live: {differ:?}");
}

/// A store a packet moved after the frame's pushes reads as the object holds it, through the host,
/// for the token's own unit and through a `target` hop; the snapshot still holds the push.
#[test]
fn a_getter_reads_the_object_as_it_runs() {
    let mut c = client();
    c.write_store(WOLF, &[(FIELD_UNIT_HEALTH, 15)]);
    c.write_store(ME, &[(FIELD_UNIT_HEALTH, 5)]);
    let chunk = "return UnitHealth('target'), UnitHealth('targettarget'), UnitHealth('player')";
    let live: (i64, i64, i64) = crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM").eval(chunk).expect("runs")
    });
    assert_eq!(live, (15, 5, 5));
    let snapshot: (i64, i64, i64) = crate::script_calls::eval_in_vm(c.app.world(), chunk);
    assert_eq!(snapshot, (80, 90, 90));
}
