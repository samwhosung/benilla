//! The unit-token resolver (`0x515970`) reads what it reads of the objects as it runs, through the
//! lent host: the active player's object (`0x51598d`), our pet off our `UNIT_FIELD_CHARM` else
//! `UNIT_FIELD_SUMMON` (`0x515a5e`-`0x515a6e`), a group member's pet off the member's descriptor
//! (`0x4e8204`, `0x491993`), the mouseover's object (`0x515bd2`), and each `target` hop off the
//! held unit's `UNIT_FIELD_TARGET` (`0x515a1c`-`0x515a25`). The live answer and the frame's
//! snapshot agree on every token form wherever the snapshot is current, and a listener resolves a
//! chain as its own packet left it.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{member_status, GroupMemberEntry, PartyMemberStatsInfo};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, targets, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// The target, a creature.
const WOLF: u64 = 0xF130_0000_0044_0001;
/// Our pet, `UNIT_FIELD_SUMMON`'s.
const PET: u64 = 0xF140_0000_0045_0002;
/// A raid member in view in our subgroup, and its pet.
const MATE: u64 = 0x22;
const MATE_PET: u64 = 0xF140_0000_0046_0003;
/// A raid member out of view, and the pet its record names.
const FAR: u64 = 0x23;
const FAR_PET: u64 = 0xF140_0000_0048_0005;
/// A creature beside the wolf.
const BOAR: u64 = 0xF130_0000_0047_0004;
/// A player no token names, a night elf rogue, and the rat it targets, a warrior-class creature
/// of template [`RAT_ENTRY`].
const ROGUE: u64 = 0x31;
const RAT: u64 = 0xF130_0000_0049_0006;
const RAT_ENTRY: u32 = 4075;

/// `UNIT_FIELD_SUMMON`, and `PLAYER_XP`, which `UnitXP` answers only for a token naming us.
const SUMMON: u16 = 8;
const PLAYER_XP: u16 = 716;
/// `OBJECT_FIELD_TYPE`, and the type masks of a unit and a player (object, unit, player bits).
const OBJECT_TYPE: u16 = 2;
const TYPE_UNIT: u32 = 0x9;
const TYPE_PLAYER: u32 = 0x19;
/// `UNIT_FIELD_BYTES_0`: race, class and gender bytes; `OBJECT_FIELD_ENTRY`, the template.
const BYTES_0: u16 = 36;
const ENTRY: u16 = 3;

/// Every token form the agreement walks: each base, a `target` hop off each, the chains, a member
/// out of view and its record's pet, our own raid row, and the forms naming nobody.
const TOKENS: [&str; 29] = [
    "player",
    "pet",
    "target",
    "targettarget",
    "targettargettarget",
    "targettargettargettarget",
    "playertarget",
    "pettarget",
    "pettargettarget",
    "party1",
    "party1target",
    "party2",
    "party2target",
    "partypet1",
    "partypet1target",
    "partypet2",
    "raid1",
    "raid1target",
    "raid2",
    "raid3",
    "raid3targettarget",
    "raidpet1",
    "raidpet2",
    "raidpet3",
    "mouseover",
    "npc",
    "party3",
    "raid9",
    "targetfoo",
];

/// The getters whose answer turns on the guid the token names: the unit itself by its health,
/// whether it is us (`UnitXP`, `UnitIsUnit`), its group tests, and its existence.
const GETTERS: &str = "
    local function row(t)
        return table.concat({
            tostring(UnitExists(t)), UnitHealth(t), UnitXP(t), tostring(UnitIsUnit(t, 'player')),
            tostring(UnitIsUnit(t, 'party1')), tostring(UnitIsUnit(t, 'target')),
            tostring(UnitInParty(t)), tostring(UnitInRaid(t)), tostring(UnitIsPlayer(t)),
            tostring((UnitBuff(t, 1))),
        }, ',')
    end";

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

    /// `chunk`'s answer through the lent host when `live`, else off the snapshots alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
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

    /// Every token's row of [`GETTERS`], one line each.
    fn answers(&mut self, live: bool) -> Vec<String> {
        let tokens = TOKENS
            .iter()
            .map(|t| format!("'{t}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let chunk = format!(
            "{GETTERS}
            local out = {{}}
            for _, t in ipairs({{ {tokens} }}) do table.insert(out, t .. ' ' .. row(t)) end
            return out"
        );
        self.eval(live, &chunk)
    }
}

/// A unit's descriptor at `hp` health with `extra`.
fn unit(hp: u32, extra: &[(u16, u32)]) -> Vec<(u16, u32)> {
    let mut pairs = vec![
        (OBJECT_TYPE, TYPE_UNIT),
        (FIELD_UNIT_HEALTH, hp),
        (FIELD_UNIT_MAXHEALTH, 100),
    ];
    pairs.extend_from_slice(extra);
    pairs
}

/// `UNIT_FIELD_SUMMON` naming `guid`.
fn summons(guid: u64) -> [(u16, u32); 2] {
    [(SUMMON, guid as u32), (SUMMON + 1, (guid >> 32) as u32)]
}

/// The client in the world as [`ME`] (90 health, some XP) with [`PET`] (70), in a raid with
/// [`MATE`] (60, in view, with [`MATE_PET`], 50) and [`FAR`] (out of view, its record naming
/// [`FAR_PET`]), all one subgroup; [`WOLF`] (80) selected and hovered. The chains: the wolf targets
/// the mate, the mate us, we the wolf, our pet the boar (40), the mate's pet the wolf. A spy on
/// `UNIT_HEALTH` of `"target"` notes what the chains name as it fires.
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
    let script = UiScript::new().expect("a VM");
    script
        .run(&format!(
            r#"
            {GETTERS}
            Seen = {{}}
            DeepSeen = {{}}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_HEALTH")
            PetSeen = {{}}
            local pets = CreateFrame("Frame")
            pets:RegisterEvent("UNIT_HEALTH")
            pets:SetScript("OnEvent", function()
                if string.find(arg1, "pet") then
                    table.insert(PetSeen, arg1 .. ":" .. UnitHealth(arg1))
                end
            end)
            f:SetScript("OnEvent", function()
                if arg1 ~= "target" then return end
                if UnitHealth("target") == 33 then
                    table.insert(DeepSeen, tostring(UnitHealth("targettargettargettarget")))
                end
                for _, t in ipairs({{ "targettarget", "targettargettarget", "party1target" }}) do
                    table.insert(Seen, UnitHealth("target") .. " " .. t .. " " .. row(t))
                end
            end)
            "#
        ))
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    {
        let mut names = app.world_mut().resource_mut::<crate::names::NameCache>();
        names.insert_player(ME, "Me".into(), None);
        names.insert_player(MATE, "Mate".into(), None);
        names.insert_player(ROGUE, "Shade".into(), None);
        names.insert_creature(
            RAT_ENTRY,
            Some(crate::names::CreatureRecord {
                name: "Rat".into(),
                subname: None,
                creature_type: 8,
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
    crate::ui_unit::TestNames::english().insert(&mut app);
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    let roster = SessionEvent::GroupList {
        group_type: crate::ui_party::GROUPTYPE_RAID,
        own_flags: 0,
        members: [("Mate", MATE), ("Far", FAR)]
            .map(|(name, guid)| GroupMemberEntry {
                name: name.into(),
                guid,
                status: member_status::ONLINE,
                flags: 0,
            })
            .into(),
        leader: ME,
        loot: None,
    };
    let far = SessionEvent::PartyMemberStats {
        guid: FAR,
        full: true,
        info: Box::new(PartyMemberStatsInfo {
            status: Some(member_status::ONLINE),
            cur_hp: Some(30),
            max_hp: Some(100),
            pet_guid: Some(FAR_PET),
            ..Default::default()
        }),
    };
    let mut me = unit(90, &summons(PET));
    me.extend(targets(WOLF));
    me.extend([(PLAYER_XP, 1234), (OBJECT_TYPE, TYPE_PLAYER)]);
    let mut rat = unit(20, &targets(ME));
    rat.extend([(ENTRY, RAT_ENTRY), (BYTES_0, 1 << 8)]);
    // Night elf (4), rogue (4), female (1).
    let mut rogue = unit(75, &targets(RAT));
    rogue.extend([(BYTES_0, 4 | 4 << 8 | 1 << 16), (OBJECT_TYPE, TYPE_PLAYER)]);
    let mut mate = unit(60, &summons(MATE_PET));
    mate.extend(targets(ME));
    mate.push((OBJECT_TYPE, TYPE_PLAYER));
    c.frame(vec![
        vec![roster],
        vec![far],
        vec![created(ME, EntityKind::Player, &me)],
        vec![created(PET, EntityKind::Unit, &unit(70, &targets(BOAR)))],
        vec![created(MATE, EntityKind::Player, &mate)],
        vec![created(
            MATE_PET,
            EntityKind::Unit,
            &unit(50, &targets(WOLF)),
        )],
        vec![created(WOLF, EntityKind::Unit, &unit(80, &targets(MATE)))],
        vec![created(BOAR, EntityKind::Unit, &unit(40, &[]))],
        vec![created(ROGUE, EntityKind::Player, &rogue)],
        vec![created(RAT, EntityKind::Unit, &rat)],
    ]);
    let world = c.app.world_mut();
    let wolf = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(wolf),
        guid: Some(WOLF),
        ..Default::default()
    };
    c.frame(vec![]);
    c.frame(vec![]);
    c.eval::<()>(false, "Seen = {} PetSeen = {} DeepSeen = {}");
    // The VM's copy of our experience, which no feed pushes: `UnitXP` reads it through the host.
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        s.expect("a VM").set_player_xp(1234, 0)
    });
    c
}

/// Every token form names its unit live, as the resolver reads the objects and the group rows
/// inside the call (`0x515970`): each base, a hop off each, the chains, a member out of view and
/// the pet its record names, our own raid row and its pet, and nobody.
#[test]
fn every_token_form_names_its_unit_live() {
    let mut c = client();
    let live = c.answers(true);
    assert_eq!(live.len(), TOKENS.len());
    // The fixture reaches every leg: each base, a hop off each, a three-hop chain, a member out
    // of view and the pet its record names, our own raid row and its pet, and nobody.
    for line in [
        "player 1,90,1234,1,nil,nil,1,1,1,nil",
        "pet 1,70,0,nil,nil,nil,nil,nil,nil,nil",
        "target 1,80,0,nil,nil,1,nil,nil,nil,nil",
        "targettarget 1,60,0,nil,1,nil,1,1,1,nil",
        "targettargettarget 1,90,1234,1,nil,nil,1,1,1,nil",
        "targettargettargettarget 1,80,0,nil,nil,1,nil,nil,nil,nil",
        "playertarget 1,80,0,nil,nil,1,nil,nil,nil,nil",
        "pettarget 1,40,0,nil,nil,nil,nil,nil,nil,nil",
        "pettargettarget nil,0,0,nil,nil,nil,nil,nil,nil,nil",
        "party1target 1,90,1234,1,nil,nil,1,1,1,nil",
        "party2target nil,0,0,nil,nil,nil,nil,nil,nil,nil",
        "partypet1 1,50,0,nil,nil,nil,nil,nil,nil,nil",
        "partypet1target 1,80,0,nil,nil,1,nil,nil,nil,nil",
        "raid1target 1,90,1234,1,nil,nil,1,1,1,nil",
        "raid3 1,90,1234,1,nil,nil,1,1,1,nil",
        "raid3targettarget 1,60,0,nil,1,nil,1,1,1,nil",
        "raidpet3 1,70,0,nil,nil,nil,nil,nil,nil,nil",
        "party3 nil,0,0,nil,nil,nil,nil,nil,nil,nil",
        "targetfoo nil,0,0,nil,nil,nil,nil,nil,nil,nil",
    ] {
        assert!(live.iter().any(|l| l == line), "{line:?} among {live:#?}");
    }
    let far = |t: &str| {
        live.iter()
            .find(|l| l.starts_with(&format!("{t} ")))
            .cloned()
    };
    assert!(
        far("party2").is_some_and(|l| l.contains(",30,")),
        "{:?}",
        far("party2")
    );
    assert!(
        far("partypet2").is_some_and(|l| l.starts_with("partypet2 1,")),
        "the record's pet: {:?}",
        far("partypet2")
    );
}

/// Two packets in one frame: the first moves the wolf's health, the second turns it onto us. The
/// first's `UNIT_HEALTH` listener resolves the chains off the wolf's `UNIT_FIELD_TARGET` as that
/// packet left it, the mate, not the frame's last: `0x515970` reads the field inside the call.
#[test]
fn a_listener_resolves_the_chains_as_its_own_packet_left_them() {
    let mut c = client();
    c.frame(vec![
        vec![values(WOLF, &[(FIELD_UNIT_HEALTH, 33)])],
        vec![values(WOLF, &targets(ME))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Seen");
    assert_eq!(
        seen,
        [
            "33 targettarget 1,60,0,nil,1,nil,1,1,1,nil",
            "33 targettargettarget 1,90,1234,1,nil,nil,1,1,1,nil",
            "33 party1target 1,90,1234,1,nil,nil,1,1,1,nil",
        ]
    );
    // The frame's end: the wolf (now at 33) targets us, and we the wolf.
    let now: Vec<String> = c.eval(
        true,
        &format!("{GETTERS} return {{ row('targettarget'), row('targettargettarget') }}"),
    );
    assert_eq!(
        now,
        [
            "1,90,1234,1,nil,nil,1,1,1,nil",
            "1,33,0,nil,nil,1,nil,nil,nil,nil",
        ]
    );
}

/// Two packets in one frame: the first moves our pet's health, the second summons another. The
/// first's `UNIT_HEALTH` fires under the tokens its own packet's state gives the pet: the watchers
/// it still carries (`0x60c520`), and `0x515e50` lists its tokens through `0x515c50` as it
/// signals, reading our `UNIT_FIELD_SUMMON` there (`0x515cb9`) and our raid row's pet
/// (`0x491960`), not the frame's last.
#[test]
fn a_pet_event_fires_under_its_own_packets_tokens() {
    let mut c = client();
    c.frame(vec![
        vec![values(PET, &[(FIELD_UNIT_HEALTH, 55)])],
        vec![values(ME, &summons(BOAR))],
    ]);
    let seen: Vec<String> = c.eval(false, "return PetSeen");
    assert_eq!(seen, ["pet:55", "raidpet3:55"]);
    let now: (i64, Option<i64>) = c.eval(
        true,
        "return UnitHealth('pet'), UnitIsUnit('pet', 'raidpet3')",
    );
    assert_eq!(now, (40, Some(1)), "the boar is our pet at the frame's end");
}

/// A unit a chain ends on that no snapshot holds is named off its object as the getter runs: the
/// name cache for its name (`0x51710c` → `GetUnitName 0x609210`) and its descriptor's race, class
/// and gender bytes, as for any held unit.
#[test]
fn a_chain_end_no_snapshot_holds_is_named_off_its_object() {
    let mut c = client();
    // The boar, the last target, now targets the rat no token names; no snapshot of the rat
    // exists, nor a by-name candidate's, which only players are.
    c.write_store(BOAR, &targets(RAT));
    c.app
        .world_mut()
        .resource_mut::<crate::target::Selection>()
        .last = Some(BOAR);
    let got: Vec<String> = c.eval(
        true,
        "TargetLastTarget()
        local race, race_file = UnitRace('targettarget')
        local class, class_file = UnitClass('targettarget')
        return { tostring(UnitName('targettarget')), tostring(race), tostring(race_file),
            tostring(class), tostring(class_file), UnitSex('targettarget'),
            UnitHealth('targettarget') }",
    );
    assert_eq!(got, ["Rat", "nil", "nil", "Warrior", "WARRIOR", "2", "20"]);
}

/// Two packets in one frame: the first turns the wolf onto the rogue and moves the rogue, the
/// second destroys the rat the rogue targets and turns the wolf back. The first's listener reads
/// the rat as that packet left it, held, though the object manager no longer holds it when the
/// listener runs: the resolver's hop off it and the getters' lookup of it read the stores as of
/// the listener's packet.
#[test]
fn a_listener_reads_a_unit_a_later_packet_destroys() {
    let mut c = client();
    c.frame(vec![
        vec![
            values(
                WOLF,
                &[
                    (FIELD_UNIT_HEALTH, 33),
                    targets(ROGUE)[0],
                    targets(ROGUE)[1],
                ],
            ),
            values(ROGUE, &[(FIELD_UNIT_HEALTH, 70)]),
        ],
        vec![
            SessionEvent::ObjectsRemoved(vec![RAT]),
            values(WOLF, &targets(MATE)),
        ],
    ]);
    let seen: Vec<String> = c.eval(
        false,
        "local out = {} for _, l in ipairs(Seen) do
            if string.find(l, '^33 targettargettarget') then table.insert(out, l) end
        end return out",
    );
    assert_eq!(
        seen,
        ["33 targettargettarget 1,20,0,nil,nil,nil,nil,nil,nil,nil"]
    );
    let deep: Vec<String> = c.eval(false, "return DeepSeen");
    assert_eq!(
        deep,
        ["90"],
        "the rat's target, us, off the rat as the packet left it"
    );
}

/// A selection a chunk makes is the target its next line resolves through: `TargetUnit` commits
/// inside its call (`0x4899d0` → `SetSelection 0x493540`), and `0x515970` reads the new target's
/// `UNIT_FIELD_TARGET` as it resolves.
#[test]
fn a_chunk_that_targets_resolves_the_new_targets_target() {
    let mut c = client();
    let got: (String, String, i64, i64, Option<i64>) = c.eval(
        true,
        "TargetUnit('party1')
        return tostring(UnitName('targettarget')), tostring(UnitName('raid3')),
            UnitHealth('targettarget'), UnitXP('targettarget'),
            UnitIsUnit('targettargettarget', 'party1')",
    );
    assert_eq!(
        (got.0.as_str(), got.1.as_str()),
        ("Me", "Me"),
        "the mate targets us"
    );
    assert_eq!((got.2, got.3, got.4), (90, 1234, None));
    let wolf: i64 = c.eval(
        true,
        "TargetUnit('party1') return UnitHealth('targettargettarget')",
    );
    assert_eq!(wolf, 80, "we target the wolf");
}

/// The resolver's per-call cost with the host lent and off the snapshots: 10k calls of a getter
/// that reads nothing but the guid (`UnitInParty`, `0x516290`), and of `UnitHealth`, for `target`
/// and `targettarget`.
///
/// Run with `cargo nextest run -p benilla-app --lib token_resolve_cost --run-ignored only
/// --no-capture`.
#[test]
#[ignore = "a timing, not a check"]
fn token_resolve_cost() {
    let mut c = client();
    for call in [
        "UnitInParty('target')",
        "UnitInParty('targettarget')",
        "UnitHealth('target')",
        "UnitHealth('targettarget')",
    ] {
        let chunk = format!("for n = 1, 10000 do {call} end");
        let mut median = |live: bool| {
            let mut times = Vec::new();
            for _ in 0..30 {
                let t = std::time::Instant::now();
                c.eval::<()>(live, &chunk);
                times.push(t.elapsed().as_secs_f64() * 1e9 / 10_000.0);
            }
            times.sort_by(f64::total_cmp);
            times[times.len() / 2]
        };
        let (live, snapshot) = (median(true), median(false));
        eprintln!("{call}: {live:.0} ns a call live, {snapshot:.0} ns off the snapshot");
    }
}
