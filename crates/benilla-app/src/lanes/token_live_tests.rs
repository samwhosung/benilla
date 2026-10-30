//! The unit-token resolver (`0x515970`) reads what it reads of the objects as it runs, through the
//! lent host: the active player's object (`0x51598d`), our pet off our `UNIT_FIELD_CHARM` else
//! `UNIT_FIELD_SUMMON` (`0x515a5e`-`0x515a6e`), a group member's pet off the member's descriptor
//! (`0x4e8204`, `0x491993`), the mouseover's object (`0x515bd2`), and each `target` hop off the
//! held unit's `UNIT_FIELD_TARGET` (`0x515a1c`-`0x515a25`). A listener resolves a chain, and the
//! verbs that take a token resolve it, as its own packet left them, with no copy pushed ahead of
//! the drain.

use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{member_status, GroupMemberEntry, PartyMemberStatsInfo};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, heartbeat, placed, targets, values, wire_packets};
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
/// A member who joins mid-frame, out of view.
const NEW: u64 = 0x24;
/// `AreaTable.dbc` zones: one on our map, 0, and one on an instance's, 36.
const ELWYNN: u16 = 12;
const DEADMINES: u16 = 1581;
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
    sent: Receiver<ClientCommand>,
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

/// `SMSG_GROUP_LIST` for a raid of us, leading, and `members` in our subgroup, in that order: the
/// party slots and the raid rows.
fn raid_of(members: &[u64]) -> SessionEvent {
    SessionEvent::GroupList {
        group_type: crate::ui_party::GROUPTYPE_RAID,
        own_flags: 0,
        members: members
            .iter()
            .map(|&guid| GroupMemberEntry {
                name: if guid == MATE { "Mate" } else { "Far" }.into(),
                guid,
                status: member_status::ONLINE,
                flags: 0,
            })
            .collect(),
        leader: ME,
        loot: None,
    }
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
    let mut c = Client { app, inbound, sent };
    let roster = raid_of(&[MATE, FAR]);
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

/// Two group lists in one frame, the second swapping the party's slots: each list's
/// `PARTY_MEMBERS_CHANGED` listener trades with `party1` as its own list wrote the slots, as
/// `InitiateTrade` resolves its token inside the call (`0x48a120` → `0x515970`), not the frame's
/// last.
#[test]
fn a_listeners_trade_goes_to_party1_as_its_own_list_left_it() {
    let mut c = client();
    c.eval::<()>(
        true,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("PARTY_MEMBERS_CHANGED")
        f:SetScript("OnEvent", function() InitiateTrade("party1") end)"#,
    );
    c.sent.try_iter().for_each(drop);
    c.frame(vec![
        vec![raid_of(&[MATE, FAR])],
        vec![raid_of(&[FAR, MATE])],
    ]);
    let mut traded: Vec<u64> = c
        .sent
        .try_iter()
        .filter_map(|cmd| match cmd {
            ClientCommand::InitiateTrade { target } => Some(target),
            _ => None,
        })
        .collect();
    traded.dedup();
    assert_eq!(traded, [MATE, FAR]);
}

/// Two group lists in one frame, the second dropping [`FAR`], who is out of view: the first list's
/// listener finds `party2` friendly and cooperative through the verbs' group fallback, which tests
/// the slots at the call (`0x516961`-`0x516a1d`, `0x516aa1`-`0x516b5c` → `0x491900` → `0x4e80a0`),
/// and the second's `party2` names nobody.
#[test]
fn a_listeners_friend_test_reads_its_own_lists_slots() {
    let mut c = client();
    c.eval::<()>(
        true,
        r#"Friends = {}
        local f = CreateFrame("Frame")
        f:RegisterEvent("PARTY_MEMBERS_CHANGED")
        f:SetScript("OnEvent", function()
            table.insert(Friends, tostring(UnitIsFriend("player", "party2")) .. ","
                .. tostring(UnitCanCooperate("party2", "player")))
        end)"#,
    );
    c.frame(vec![vec![raid_of(&[MATE, FAR])], vec![raid_of(&[MATE])]]);
    let mut seen: Vec<String> = c.eval(false, "return Friends");
    seen.dedup();
    assert_eq!(seen, ["1,1", "nil,nil"]);
}

/// The world map's inputs for the map position tests: our map 0, the two zones' `AreaTable` rows,
/// and a projection that reads a position on map 0 as its thousandths and any other map as off
/// the displayed one (`0x4a7437`); and a spy noting `GetPlayerMapPosition(token)` at each of
/// `token`'s `UNIT_HEALTH`s.
fn map_spy(c: &mut Client, token: &str) {
    let row = |map_id| benilla_formats::AreaTableRow {
        map_id,
        zone_id: 0,
        explore_flag: 0,
        flags: 0,
        faction_group_mask: 0,
        exploration_level: -1,
        name: String::new(),
    };
    c.app.insert_resource(crate::area::AreaTableRes(
        benilla_formats::AreaTableCatalog::from_rows(vec![
            (u32::from(ELWYNN), row(0)),
            (u32::from(DEADMINES), row(36)),
        ]),
    ));
    c.app
        .insert_resource(benilla_world::world_map::CurrentMap(0));
    let script = &mut *c.app.world_mut().non_send_resource_mut::<UiScript>();
    script.set_world_loc_projector(Box::new(|_, map, x, y| {
        (map == 0).then_some((x / 1000.0, y / 1000.0))
    }));
    c.eval::<()>(
        true,
        &format!(
            r#"Maps = {{}}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_HEALTH")
            f:SetScript("OnEvent", function()
                if arg1 == "{token}" then
                    local x, y = GetPlayerMapPosition(arg1)
                    table.insert(Maps, string.format("%.1f,%.1f", x, y))
                end
            end)"#
        ),
    );
}

/// `SMSG_PARTY_MEMBER_STATS` for `guid`, online, at `hp` health in `zone` at `at`.
fn member_at(guid: u64, hp: u16, zone: u16, at: (i16, i16)) -> SessionEvent {
    member_with(member_status::ONLINE, guid, hp, zone, at)
}

/// [`member_at`] with the record's status byte `status`.
fn member_with(status: u8, guid: u64, hp: u16, zone: u16, at: (i16, i16)) -> SessionEvent {
    SessionEvent::PartyMemberStats {
        guid,
        full: true,
        info: Box::new(PartyMemberStatsInfo {
            status: Some(status),
            cur_hp: Some(hp),
            max_hp: Some(100),
            zone: Some(zone),
            position: Some(at),
            ..Default::default()
        }),
    }
}

/// A member who joins and moves inside one frame: each stats packet's listener reads `party3`'s
/// map position off the record that packet wrote, as `0x4a7870` reads the record at the call
/// (`0x4a78fd`, its position words at `0x4a7929` and `0x4a792d`), not the frame before's, which
/// held no such member.
#[test]
fn a_member_who_joins_mid_frame_reads_its_packets_map_position() {
    let mut c = client();
    map_spy(&mut c, "party3");
    c.frame(vec![
        vec![raid_of(&[MATE, FAR, NEW])],
        vec![member_at(NEW, 50, ELWYNN, (100, 200))],
        vec![member_at(NEW, 40, ELWYNN, (300, 400))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["0.1,0.2", "0.3,0.4"]);
}

/// A member in an instance whose position falls inside our map's rect reads (0, 0): `0x4a7870`
/// projects a record under the map its zone's `AreaTable` row names (`0x4a790c`-`0x4a7934`), which
/// is not the displayed one (`0x4a7437`), never under ours.
#[test]
fn a_member_in_an_instance_reads_no_map_position() {
    let mut c = client();
    map_spy(&mut c, "party2");
    c.frame(vec![
        vec![member_at(FAR, 20, ELWYNN, (100, 200))],
        vec![member_at(FAR, 10, DEADMINES, (100, 200))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["0.1,0.2", "0.0,0.0"]);
}

/// A member whose record's status has bit 0x20 reads (0, 0), and the same record without it reads
/// its position: `0x4a7870` leaves a record with that bit (`0x4a7906`) unread.
#[test]
fn a_member_whose_status_has_bit_0x20_reads_no_map_position() {
    let mut c = client();
    map_spy(&mut c, "party2");
    c.frame(vec![
        vec![member_with(
            member_status::ONLINE | 0x20,
            FAR,
            20,
            ELWYNN,
            (100, 200),
        )],
        vec![member_at(FAR, 10, ELWYNN, (100, 200))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["0.0,0.0", "0.1,0.2"]);
}

/// A member in view reads its object's position under our map whatever its record's zone:
/// `0x4a7870` takes a held player object first (`0x4a78ae`, typemask 0x10) and projects it under
/// the active player's map (`0x4a78e2` → `0x5ea700`), never reaching the record (`0x4a78fd`).
#[test]
fn a_member_in_view_reads_its_objects_map_position() {
    let mut c = client();
    map_spy(&mut c, "party1");
    let mate = c.app.world().resource::<crate::net::GuidIndex>().0[&MATE];
    c.app
        .world_mut()
        .entity_mut(mate)
        .insert(Transform::from_translation(
            benilla_assets::coords::wow_to_bevy([500.0, 600.0, 0.0]),
        ));
    c.frame(vec![vec![member_at(MATE, 20, DEADMINES, (100, 200))]]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["0.5,0.6"]);
}

/// A member in view whose stats packet's listener reads its map position, destroyed by a later
/// packet of the frame: the listener runs before the destroy's handler (`0x537ae5`), so
/// `0x4a7870` still finds the object at the call (`0x4a78ae`) and reads its position
/// (`0x4a78c1`).
#[test]
fn a_member_destroyed_later_in_the_frame_reads_its_objects_map_position() {
    let mut c = client();
    map_spy(&mut c, "party1");
    let mate = c.app.world().resource::<crate::net::GuidIndex>().0[&MATE];
    c.app
        .world_mut()
        .entity_mut(mate)
        .insert(Transform::from_translation(
            benilla_assets::coords::wow_to_bevy([500.0, 600.0, 0.0]),
        ));
    c.frame(vec![
        vec![member_at(MATE, 20, DEADMINES, (100, 200))],
        vec![SessionEvent::ObjectDestroyed(MATE)],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["0.5,0.6"]);
}

/// A transport whose deck stands at (3000, 4000) turned a quarter left, so a deck-local +x runs
/// along world +y; it has no clock, so no tick moves it.
const BOAT: u64 = 0xF120_0000_0050_0007;
/// `OBJECT_FIELD_TYPE` of a game object (object and game object bits), and `GAMEOBJECT_FLAGS`
/// with the transport bit, bit 3, which `0x630a80` tests.
const TYPE_GAMEOBJECT: u32 = 0x21;
const GAMEOBJECT_FLAGS: u16 = 9;
const GO_TRANSPORT: u32 = 0x8;
const FORWARD: u32 = crate::creature_anim::move_flags::FORWARD;

/// A deck's descriptor: a game object flagged a transport.
fn deck_fields() -> [(u16, u32); 2] {
    [
        (OBJECT_TYPE, TYPE_GAMEOBJECT),
        (GAMEOBJECT_FLAGS, GO_TRANSPORT),
    ]
}

/// [`BOAT`] in the world, held under its guid, armed as the transport tick leaves a deck.
fn moor(c: &mut Client) {
    let world = c.app.world_mut();
    let boat = world
        .spawn((
            crate::net::Guid(BOAT),
            crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&deck_fields())),
            crate::transport::Transport::parked_for_test(),
            Transform::from_translation(benilla_assets::coords::wow_to_bevy([3000.0, 4000.0, 0.0]))
                .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        ))
        .id();
    world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .insert(BOAT, boat);
}

/// [`MATE`]'s create block at the world's `at`, aboard `deck` when one is named.
fn mate_created(at: [f32; 3], deck: Option<(u64, [f32; 3])>) -> SessionEvent {
    let mut mate = unit(60, &summons(MATE_PET));
    mate.extend(targets(ME));
    mate.push((OBJECT_TYPE, TYPE_PLAYER));
    placed(created(MATE, EntityKind::Player, &mate), at, 0.0, deck)
}

/// The client with `party1` created at (500, 600) and its first relayed move there, by `flags`,
/// applied by the frame's tick, on a clock that steps 16 ms a frame; the spy cleared. Two frames
/// on, a move whose wire stamp has not advanced arrives 32 ms late.
fn seated(flags: u32) -> Client {
    let mut c = client();
    c.app
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(16),
        ));
    map_spy(&mut c, "party1");
    moor(&mut c);
    c.frame(vec![
        vec![mate_created([500.0, 600.0, 0.0], None)],
        vec![heartbeat(MATE, [500.0, 600.0, 0.0], flags, None)],
    ]);
    c.frame(vec![]);
    c.eval::<()>(false, "Maps = {}");
    c
}

/// A late mover's packet seats it inside its handler, where a later packet's listener reads it:
/// due by the movement clock at its arrival (`@0x618dd2`), the apply writes the mover's position
/// there (`0x7c6420` at `0x618dfc`), and `0x4a7870` reads the object's position at the call
/// (`0x4a78c1` → `0x5f1f10`).
#[test]
fn a_listener_reads_a_late_mover_where_its_packet_put_it() {
    let mut c = seated(FORWARD);
    c.frame(vec![
        vec![heartbeat(MATE, [700.0, 800.0, 0.0], FORWARD, None)],
        vec![member_at(MATE, 20, ELWYNN, (100, 200))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["0.7,0.8"]);
}

/// A spy noting `token`'s map position to a ten-thousandth, a tenth of a yard on [`map_spy`]'s
/// projection, in `Fine` at each of its `UNIT_HEALTH`s.
fn fine_map_spy(c: &mut Client, token: &str) {
    c.eval::<()>(
        true,
        &format!(
            r#"Fine = {{}}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_HEALTH")
            f:SetScript("OnEvent", function()
                if arg1 == "{token}" then
                    local x, y = GetPlayerMapPosition(arg1)
                    table.insert(Fine, string.format("%.4f,%.4f", x, y))
                end
            end)"#
        ),
    );
}

/// A late mover's packet seats it stepped on by its lateness, where a later packet's listener
/// reads it: the late start's catch-up (`0x618bb0` at `0x618e5d`) runs the mover on from the
/// packet's point for the ms the movement clock is past the fire-time, at most 500, since a due
/// fire-time's skew is at least −500 (`@0x618d49`). Forty frames after its last fire, a move whose
/// stamp has not advanced fires 500 ms before its arrival, 484 before the clock: 3.388 yd at a run
/// speed of 7, along its facing, +x.
#[test]
fn a_listener_reads_a_late_mover_stepped_on_by_its_lateness() {
    let mut c = seated(FORWARD);
    let mate = c.app.world().resource::<crate::net::GuidIndex>().0[&MATE];
    c.app
        .world_mut()
        .entity_mut(mate)
        .insert(crate::net::UnitSpeeds(benilla_protocol::MoveSpeeds {
            run: 7.0,
            ..Default::default()
        }));
    fine_map_spy(&mut c, "party1");
    for _ in 0..40 {
        c.frame(vec![]);
    }
    c.frame(vec![
        vec![heartbeat(MATE, [700.0, 800.0, 0.0], FORWARD, None)],
        vec![member_at(MATE, 20, ELWYNN, (100, 200))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Fine");
    assert_eq!(seen, ["0.7034,0.8000"]);
}

/// A standing member's packet waits for the frame's tick, so a later packet's listener reads the
/// member where it stood: the re-base puts the fire-time at or past the arrival
/// (`0x618cfc`-`0x618d41`), past the movement clock (`@0x618dd2`, `0x618dde`), and the move is
/// queued (`0x618f20` → `0x6176a0`).
#[test]
fn a_listener_reads_a_standing_member_where_it_stood() {
    let mut c = seated(0);
    c.frame(vec![
        vec![heartbeat(MATE, [700.0, 800.0, 0.0], 0, None)],
        vec![member_at(MATE, 20, ELWYNN, (100, 200))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["0.5,0.6"]);
}

/// A late mover a packet seats on a deck reads, in a later packet's listener, its deck-local
/// point through the deck as it stands, not the packet's world position: the apply re-seats the
/// mover deck-local (`0x618b30` → `0x7c6900`, at `0x618e07`), and its position reads through the
/// transport's matrix at the call (`0x5f1f10` → `0x7c4930`). A second late move re-seats it.
#[test]
fn a_listener_reads_a_late_rider_where_its_packet_seated_it_on_the_deck() {
    let mut c = seated(FORWARD);
    c.frame(vec![
        vec![heartbeat(
            MATE,
            [100.0, 200.0, 0.0],
            FORWARD,
            Some((BOAT, [500.0, 0.0, 0.0])),
        )],
        vec![member_at(MATE, 20, ELWYNN, (100, 200))],
        vec![heartbeat(
            MATE,
            [100.0, 200.0, 0.0],
            FORWARD,
            Some((BOAT, [500.0, 100.0, 0.0])),
        )],
        vec![member_at(MATE, 10, ELWYNN, (100, 200))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["3.0,4.5", "2.9,4.5"]);
}

/// A standing member's packet onto a deck waits for the frame's tick, as any idle mover's does,
/// so a later packet's listener reads the member where it stood.
#[test]
fn a_listener_reads_a_standing_rider_where_it_stood() {
    let mut c = seated(0);
    c.frame(vec![
        vec![heartbeat(
            MATE,
            [100.0, 200.0, 0.0],
            0,
            Some((BOAT, [500.0, 0.0, 0.0])),
        )],
        vec![member_at(MATE, 20, ELWYNN, (100, 200))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["0.5,0.6"]);
}

/// `create` with its living block's mover state: moving by `flags`, its stamp `stamp` on the
/// server's clock.
fn living(mut create: SessionEvent, flags: u32, stamp: u32) -> SessionEvent {
    if let SessionEvent::ObjectCreate { mover, .. } = &mut create {
        *mover = Some(benilla_protocol::MoverState {
            flags,
            pitch: 0.0,
            time: stamp,
        });
    }
    create
}

/// The client with [`MATE`] created at (500, 600), moving by `flags` and stamped 0, on a clock
/// that steps 16 ms a frame, and two frames on, a move stamped 0 at (700, 800), moving forward,
/// read by a listener after it: its create seeded the replay chain with the block's stamp and the
/// clock (`0x619320` → `0x618c30`, `@0x618c54`-`0x618c6a`, the word cleared first at `0x61936e`),
/// so the move's skew is its stamp's step, none, less its arrival's, 32 ms (`@0x618ccf`).
fn first_move_after(flags: u32) -> Vec<String> {
    let mut c = client();
    c.app
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(16),
        ));
    map_spy(&mut c, "party1");
    c.frame(vec![vec![living(
        mate_created([500.0, 600.0, 0.0], None),
        flags,
        0,
    )]]);
    c.frame(vec![]);
    c.frame(vec![
        vec![heartbeat(MATE, [700.0, 800.0, 0.0], FORWARD, None)],
        vec![member_at(MATE, 20, ELWYNN, (100, 200))],
    ]);
    c.eval(false, "return Maps")
}

/// A member's first move after a create that showed it moving is due at arrival, and seats in its
/// handler where a later packet's listener reads it: a moving mover is not re-based
/// (`@0x618ce4`), so the move fires 32 ms before its arrival, at the create's clock, before the
/// movement clock the frame between stamped (`@0x618dd2`).
#[test]
fn a_listener_reads_a_created_movers_first_move_where_it_put_it() {
    assert_eq!(first_move_after(FORWARD), ["0.7,0.8"]);
}

/// The same move after a create that showed the member standing waits for the frame's tick: the
/// schedule reads the mover's flags before the move applies, the create's, and a standing mover
/// with nothing queued re-bases by the ring's worst lateness, this move's 32 ms (`0x618cfc`-
/// `0x618d41`), to fire at its arrival, past the movement clock (`0x618dde`).
#[test]
fn a_listener_reads_a_created_stander_where_it_stood_until_its_first_move_fires() {
    assert_eq!(first_move_after(0), ["0.5,0.6"]);
}

/// A member's first move, aboard, in its create's frame waits for the frame's tick: the create
/// seeded the chain with its own stamp and clock (`0x619320` → `0x618c30`), so a move whose stamp
/// and arrival have not moved past them fires at its arrival (`@0x618ccf`), past the movement
/// clock (`@0x618dd2`), and a later packet's listener reads the member where its create stood.
#[test]
fn a_listener_reads_a_member_whose_first_move_lands_aboard_where_it_stood() {
    let mut c = client();
    map_spy(&mut c, "party1");
    moor(&mut c);
    c.frame(vec![
        vec![living(mate_created([500.0, 600.0, 0.0], None), 0, 0)],
        vec![heartbeat(
            MATE,
            [100.0, 200.0, 0.0],
            FORWARD,
            Some((BOAT, [500.0, 0.0, 0.0])),
        )],
        vec![member_at(MATE, 20, ELWYNN, (100, 200))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["0.5,0.6"]);
}

/// A member whose create block seats it on a deck reads its deck-local point through the deck in
/// a later packet's listener: the create applies at once, forced (`0x619320` → `0x618c30` at
/// `0x61941f`, its force pushed at `0x619414`), and re-seats the mover as a relayed move's
/// apply does (`0x618e07`).
#[test]
fn a_listener_reads_a_member_created_aboard_through_the_deck() {
    let mut c = seated(0);
    c.frame(vec![
        vec![mate_created(
            [100.0, 200.0, 0.0],
            Some((BOAT, [500.0, 100.0, 0.0])),
        )],
        vec![member_at(MATE, 20, ELWYNN, (100, 200))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["2.9,4.5"]);
}

/// A member created aboard a deck whose own create came earlier in the same frame reads through
/// that deck: `0x630a80` takes any held game object whose flags have the transport bit, armed by
/// the tick or not, and the deck stands where its create put it until its tick moves it.
#[test]
fn a_listener_reads_a_member_created_aboard_a_deck_created_that_frame() {
    let mut c = client();
    map_spy(&mut c, "party1");
    c.frame(vec![
        vec![placed(
            created(BOAT, EntityKind::GameObject, &deck_fields()),
            [3000.0, 4000.0, 0.0],
            std::f32::consts::FRAC_PI_2,
            None,
        )],
        vec![mate_created(
            [100.0, 200.0, 0.0],
            Some((BOAT, [500.0, 100.0, 0.0])),
        )],
        vec![member_at(MATE, 20, ELWYNN, (100, 200))],
    ]);
    let seen: Vec<String> = c.eval(false, "return Maps");
    assert_eq!(seen, ["2.9,4.5"]);
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
