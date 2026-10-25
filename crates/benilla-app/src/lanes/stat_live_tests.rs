//! The stat getters and the active player's own (money, experience, rest) read the
//! object as they run, through the lent host, as the reference's read the unit they resolve
//! (`UnitStat`, `0x518600`, `[unit+0x110]` from index 6 on, the player block at `[unit+0xe68]`)
//! and the active player's fields (`GetMoney`, `0x518160`, `+0xf70`): the live answer and the
//! frame's snapshot agree on every getter wherever the snapshot is current, and a listener reads
//! its own packet's value.

use benilla_protocol::field::{
    FIELD_PLAYER_FIELD_COINAGE, FIELD_PLAYER_FLAGS, FIELD_PLAYER_INV_SLOT_HEAD,
    FIELD_PLAYER_SKILL_INFO_1_1, FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL, FIELD_UNIT_MAXHEALTH,
};
use benilla_protocol::messages::{ItemDamage, ObjectType};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, targets, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// The target, a creature that targets us.
const WOLF: u64 = 0xF130_0000_0044_0001;
/// Our pet, `UNIT_FIELD_SUMMON`'s, summoned by us.
const PET: u64 = 0xF140_0000_0045_0002;

/// `UNIT_FIELD_SUMMON` and `UNIT_FIELD_SUMMONEDBY`.
const SUMMON: u16 = 8;
const SUMMONEDBY: u16 = 12;
/// `UNIT_FIELD_BASEATTACKTIME` (main, off) and `RANGEDATTACKTIME`.
const ATTACK_TIME: u16 = 126;
const RANGED_ATTACK_TIME: u16 = 128;
/// `UNIT_FIELD_MINDAMAGE` to `MAXOFFHANDDAMAGE`, floats.
const MIN_DAMAGE: u16 = 134;
/// `UNIT_FIELD_STAT0` and `UNIT_FIELD_RESISTANCES`.
const STAT0: u16 = 150;
const RESISTANCES: u16 = 155;
/// `UNIT_FIELD_ATTACK_POWER`, its `_MODS` (two signed halves) and `_MULTIPLIER`, then the three
/// ranged ones and the ranged damage range.
const ATTACK_POWER: u16 = 165;
const MIN_RANGED_DAMAGE: u16 = 171;
/// `PLAYER_FLAGS_RESTING` and `PLAYER_FLAGS_PARTIAL_PLAY_TIME`.
const RESTING: u32 = 0x20;
const PARTIAL_PLAY_TIME: u32 = 0x1000;
/// `PLAYER_BYTES_2`, whose byte 3 is the rest state.
const BYTES_2: u16 = 194;
/// `PLAYER_XP`, `PLAYER_NEXT_LEVEL_XP`.
const XP: u16 = 716;
/// `PLAYER_BLOCK_PERCENTAGE`, dodge, parry.
const BLOCK: u16 = 1106;
/// `PLAYER_REST_STATE_EXPERIENCE`.
const REST_POOL: u16 = 1175;
/// `PLAYER_FIELD_POSSTAT0`, `NEGSTAT0`, the resistance buff mods and the damage-done mods.
const POSSTAT0: u16 = 1177;
const NEGSTAT0: u16 = 1182;
const RESISTANCE_POS: u16 = 1187;
const RESISTANCE_NEG: u16 = 1194;
const DAMAGE_DONE_POS: u16 = 1201;
const DAMAGE_DONE_NEG: u16 = 1208;
const DAMAGE_DONE_PCT: u16 = 1215;

/// The worn weapons: a sword in the main hand, a dagger in the off hand and a shadow wand in the
/// ranged slot, each `(guid, entry, inventory slot)`.
const SWORD: (u64, u32, u8) = (item(1), 25, 15);
const DAGGER: (u64, u32, u8) = (item(2), 2092, 16);
const WAND: (u64, u32, u8) = (item(3), 5069, 17);

const fn item(low: u64) -> u64 {
    (benilla_protocol::guid::HIGH_ITEM as u64) << 48 | low
}

/// The tokens the agreement walks: us, a creature, our pet, and us again through a `target` hop.
const TOKENS: [&str; 4] = ["player", "target", "pet", "targettarget"];

/// The census's getters that take a unit, called with every token.
const UNIT_GETTERS: &str = "
    for i = 1, 5 do note('UnitStat ' .. i, UnitStat(unit, i)) end
    for i = 0, 6 do note('UnitResistance ' .. i, UnitResistance(unit, i)) end
    note('UnitArmor', UnitArmor(unit))
    note('UnitDamage', UnitDamage(unit))
    note('UnitRangedDamage', UnitRangedDamage(unit))
    note('UnitAttackSpeed', UnitAttackSpeed(unit))
    note('UnitAttackPower', UnitAttackPower(unit))
    note('UnitRangedAttackPower', UnitRangedAttackPower(unit))
    note('UnitAttackBothHands', UnitAttackBothHands(unit))
    note('UnitRangedAttack', UnitRangedAttack(unit))
    note('UnitDefense', UnitDefense(unit))
    note('UnitXP', UnitXP(unit))
    note('UnitXPMax', UnitXPMax(unit))";

/// The census's getters of the active player alone, called once.
const PLAYER_GETTERS: &str = "
    note('GetDodgeChance', GetDodgeChance())
    note('GetParryChance', GetParryChance())
    note('GetBlockChance', GetBlockChance())
    note('HasWandEquipped', HasWandEquipped())
    note('GetMoney', GetMoney())
    note('GetRestState', GetRestState())
    note('GetXPExhaustion', GetXPExhaustion())
    note('IsResting', IsResting())
    note('PartialPlayTime', PartialPlayTime())
    note('NoPlayTime', NoPlayTime())";

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

    /// Every census getter's answer, one line each: the unit getters for every token, then the
    /// active player's.
    fn answers(&mut self, live: bool) -> Vec<String> {
        let tokens = TOKENS
            .iter()
            .map(|t| format!("'{t}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let chunk = format!(
            "local out = {{}}
            local function note(what, ...)
                for i = 1, table.getn(arg) do arg[i] = tostring(arg[i]) end
                table.insert(out, what .. ' ' .. table.concat(arg, ','))
            end
            for _, unit in ipairs({{ {tokens} }}) do
                local function note(what, ...)
                    for i = 1, table.getn(arg) do arg[i] = tostring(arg[i]) end
                    table.insert(out, unit .. ' ' .. what .. ' ' .. table.concat(arg, ','))
                end
                {UNIT_GETTERS}
            end
            {PLAYER_GETTERS}
            return out"
        );
        self.eval(live, &chunk)
    }
}

/// A float field's bits.
fn float(x: f32) -> u32 {
    x.to_bits()
}

/// A signed field as the wire carries it.
fn signed(x: i32) -> u32 {
    x as u32
}

/// Two 16-bit halves in one field, `lo` the low.
fn halves(lo: i32, hi: i32) -> u32 {
    (lo as u16 as u32) | ((hi as u16 as u32) << 16)
}

/// One `PLAYER_SKILL_INFO` slot: the line and step, the rank and maximum, the temporary and
/// permanent bonuses.
fn skill(slot: u16, id: u32, rank: u32, max: u32, temp: i32, perm: i32) -> [(u16, u32); 3] {
    let base = FIELD_PLAYER_SKILL_INFO_1_1 + 3 * slot;
    [
        (base, id),
        (base + 1, rank | (max << 16)),
        (base + 2, halves(temp, perm)),
    ]
}

/// The records of the worn weapons: a one-handed sword, a dagger and a wand whose one damage line
/// is shadow.
fn record(entry: u32) -> benilla_protocol::messages::ItemInfo {
    let mut t = crate::items::test_template(&format!("Item {entry}"));
    match entry {
        25 => (t.class, t.subclass, t.inventory_type) = (2, 7, 13),
        2092 => (t.class, t.subclass, t.inventory_type) = (2, 15, 13),
        5069 => {
            (t.class, t.subclass, t.inventory_type) = (2, 19, 26);
            t.damages = vec![ItemDamage {
                min: 9.0,
                max: 17.0,
                school: 5,
            }];
        }
        _ => {}
    }
    t
}

fn item_create((guid, entry, _): (u64, u32, u8)) -> SessionEvent {
    SessionEvent::ItemCreate {
        guid,
        container: false,
        fields: ObjectFields::from_pairs(&[(2, 0x3), (3, entry), (6, ME as u32), (8, ME as u32)])
            .into_created(ObjectType::Item),
    }
}

/// Our descriptor: worn weapons, item-raised stats under a buff and a debuff on each, a cursed
/// shadow resistance, signed attack power mods, the weapon and defense skills (the dagger's under
/// a temporary malus), the avoidance chances, money, experience and rest.
fn me() -> Vec<(u16, u32)> {
    let mut f = vec![
        (FIELD_UNIT_HEALTH, 900),
        (FIELD_UNIT_MAXHEALTH, 1000),
        (FIELD_UNIT_LEVEL, 30),
        (SUMMON, PET as u32),
        (SUMMON + 1, (PET >> 32) as u32),
        (ATTACK_TIME, 2600),
        (ATTACK_TIME + 1, 1800),
        (RANGED_ATTACK_TIME, 1500),
        (ATTACK_POWER, 310),
        (ATTACK_POWER + 1, halves(45, -20)),
        (ATTACK_POWER + 2, float(0.1)),
        (ATTACK_POWER + 3, 120),
        (ATTACK_POWER + 4, halves(10, -35)),
        (ATTACK_POWER + 5, float(-0.05)),
        (MIN_RANGED_DAMAGE, float(11.5)),
        (MIN_RANGED_DAMAGE + 1, float(19.25)),
        (FIELD_PLAYER_FIELD_COINAGE, 123_456),
        (XP, 4_200),
        (XP + 1, 9_000),
        (REST_POOL, 1_500),
        (BYTES_2, 1 << 24),
        (FIELD_PLAYER_FLAGS, RESTING | PARTIAL_PLAY_TIME),
        (BLOCK, float(5.0)),
        (BLOCK + 1, float(7.25)),
        (BLOCK + 2, float(4.5)),
        (DAMAGE_DONE_POS, 12),
        (DAMAGE_DONE_NEG, signed(-4)),
        (DAMAGE_DONE_PCT, float(1.1)),
        (DAMAGE_DONE_POS + 5, 30),
        (DAMAGE_DONE_NEG + 5, signed(-8)),
        (DAMAGE_DONE_PCT + 5, float(1.25)),
    ];
    for (i, d) in [41.0f32, 62.5, 30.0, 44.0].into_iter().enumerate() {
        f.push((MIN_DAMAGE + i as u16, float(d)));
    }
    for (i, (stat, pos, neg)) in [
        (80, 15, -5),
        (55, 0, -12),
        (90, 20, 0),
        (40, 0, 0),
        (-3, 2, -9),
    ]
    .into_iter()
    .enumerate()
    {
        let i = i as u16;
        f.extend([
            (STAT0 + i, signed(stat)),
            (POSSTAT0 + i, signed(pos)),
            (NEGSTAT0 + i, signed(neg)),
        ]);
    }
    for (i, (res, pos, neg)) in [
        (1450, 200, -150),
        (10, 10, 0),
        (0, 0, 0),
        (25, 0, 0),
        (5, 15, -10),
        (-20, 0, -20),
        (0, 0, 0),
    ]
    .into_iter()
    .enumerate()
    {
        let i = i as u16;
        f.extend([
            (RESISTANCES + i, signed(res)),
            (RESISTANCE_POS + i, signed(pos)),
            (RESISTANCE_NEG + i, signed(neg)),
        ]);
    }
    for (guid, _, seat) in [SWORD, DAGGER, WAND] {
        let field = FIELD_PLAYER_INV_SLOT_HEAD + 2 * u16::from(seat);
        f.extend([(field, guid as u32), (field + 1, (guid >> 32) as u32)]);
    }
    // Swords, Daggers under a temporary malus, Wands, Defense.
    f.extend(skill(0, 43, 140, 150, 5, 0));
    f.extend(skill(1, 173, 120, 150, -130, 0));
    f.extend(skill(2, 228, 90, 150, 0, 3));
    f.extend(skill(3, 95, 145, 150, 7, 0));
    f
}

/// A creature's public stat block: its resistances, damage, swing and attack power.
fn wolf() -> Vec<(u16, u32)> {
    let mut f = vec![
        (FIELD_UNIT_HEALTH, 800),
        (FIELD_UNIT_MAXHEALTH, 800),
        (FIELD_UNIT_LEVEL, 31),
        (ATTACK_TIME, 2000),
        (MIN_DAMAGE, float(50.0)),
        (MIN_DAMAGE + 1, float(70.0)),
        (ATTACK_POWER, 178),
        (ATTACK_POWER + 1, halves(12, -4)),
        (RESISTANCES, 900),
        (RESISTANCES + 2, signed(-15)),
    ];
    f.extend(targets(ME));
    f
}

/// Our pet: summoned by us, level 29, with what the server sends its owner.
fn pet() -> Vec<(u16, u32)> {
    vec![
        (FIELD_UNIT_HEALTH, 500),
        (FIELD_UNIT_MAXHEALTH, 500),
        (FIELD_UNIT_LEVEL, 29),
        (SUMMONEDBY, ME as u32),
        (SUMMONEDBY + 1, (ME >> 32) as u32),
        (ATTACK_TIME, 2000),
        (STAT0 + 1, 25),
        (RESISTANCES, 600),
        (ATTACK_POWER, 78),
        (ATTACK_POWER + 1, halves(30, -10)),
    ]
}

/// The client in the world as [`ME`], wearing the weapons, with [`PET`], and [`WOLF`] selected,
/// targeting us; a VM whose spy records `UNIT_STATS` on `"player"` (with the agility, the wand
/// and the ranged skill), `PLAYER_MONEY` and `PLAYER_XP_UPDATE` with what each one's getter reads
/// as it fires.
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
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"UNIT_STATS", "PLAYER_MONEY", "PLAYER_XP_UPDATE"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                if event == "UNIT_STATS" then
                    if arg1 == "player" then
                        local _, effective = UnitStat("player", 2)
                        local wand, ranged = HasWandEquipped(), UnitRangedAttack("player")
                        table.insert(Seen, event .. ":" .. effective .. ":" .. tostring(wand)
                            .. ":" .. ranged)
                    end
                elseif event == "PLAYER_MONEY" then
                    table.insert(Seen, event .. ":" .. GetMoney())
                else
                    table.insert(Seen, event .. ":" .. UnitXP("player"))
                end
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    {
        let mut items = app.world_mut().resource_mut::<crate::items::Items>();
        for (_, entry, _) in [SWORD, DAGGER, WAND] {
            items.insert_template(entry, Some(record(entry)));
        }
    }
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    let mut packets: Vec<Vec<SessionEvent>> = [SWORD, DAGGER, WAND]
        .into_iter()
        .map(|w| vec![item_create(w)])
        .collect();
    packets.push(vec![created(ME, EntityKind::Player, &me())]);
    packets.push(vec![created(PET, EntityKind::Unit, &pet())]);
    packets.push(vec![created(WOLF, EntityKind::Unit, &wolf())]);
    c.frame(packets);
    let world = c.app.world_mut();
    let wolf = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(wolf),
        guid: Some(WOLF),
        ..Default::default()
    };
    c.frame(vec![]);
    c.frame(vec![]);
    c.eval::<()>(false, "Seen = {}");
    c
}

/// Wherever the frame's snapshots are current, every census getter answers the same off the live
/// object as off the snapshot, for us (buffed by items and auras, debuffed), a creature, our pet
/// and us through a `target` hop: the fields the snapshots hold are the ones the live read takes
/// ([`crate::ui_char::StatsObject`]).
#[test]
fn every_stat_getter_answers_the_same_live_as_off_the_snapshot() {
    let mut c = client();
    let snapshot = c.answers(false);
    let live = c.answers(true);
    assert_eq!(snapshot, live, "snapshot vs live");
    // The fixture reaches every gate: our buffs and gear, the creature's and the pet's own arms.
    for line in [
        "player UnitStat 5 -3,0,2,-9",
        "player UnitResistance 5 0,0,0,-20",
        "player UnitArmor 1400,1450,1450,200,-150",
        "player UnitAttackSpeed 2.6000001234934,1.8000000854954",
        "player UnitAttackBothHands 140,5,120,-120",
        "player UnitRangedAttack 93,0",
        "player UnitDefense 145,7",
        "player UnitRangedDamage 1.5000000712462,11.5,19.25,30,-8,1.25",
        "target UnitResistance 2 0,0,0,0",
        "target UnitDamage 50,70,0,0,0,0,1",
        "target UnitDefense 0,0",
        "pet UnitDefense 145,0",
        "pet UnitAttackPower 78,30,-10",
        "targettarget UnitDefense 145,7",
        "GetDodgeChance 7.25",
        "HasWandEquipped 1",
        "GetMoney 123456",
        "GetRestState 1,Rested,2",
        "GetXPExhaustion 3000",
        "IsResting 1",
        "PartialPlayTime 1",
    ] {
        assert!(live.iter().any(|l| l == line), "{line:?} among {live:#?}");
    }
}

/// Two packets in one frame, each moving our agility, our money and our experience: each
/// listener reads its own packet's value as it fires (`UnitStat` reads `UNIT_FIELD_STAT1` off the
/// object, `0x518689`; `GetMoney` the coinage, `0x5181ae`; `UnitXP` `PLAYER_XP`, `0x5173c9`), not
/// the frame's last.
#[test]
fn each_stat_listener_reads_its_own_packets_value() {
    let mut c = client();
    let packet = |agility: u32, money: u32, xp: u32| {
        vec![values(
            ME,
            &[
                (STAT0 + 1, agility),
                (FIELD_PLAYER_FIELD_COINAGE, money),
                (XP, xp),
            ],
        )]
    };
    c.frame(vec![packet(60, 1_000, 5_000), packet(70, 2_000, 6_000)]);
    let seen: Vec<String> = c.eval(false, "return Seen");
    assert_eq!(
        seen,
        [
            "UNIT_STATS:60:1:93",
            "PLAYER_XP_UPDATE:5000",
            "PLAYER_MONEY:1000",
            "UNIT_STATS:70:1:93",
            "PLAYER_XP_UPDATE:6000",
            "PLAYER_MONEY:2000",
        ]
    );
}

/// A listener of an earlier packet reads the weapons it wore then, though a later packet of the
/// frame unequips the wand and destroys it: `HasWandEquipped` reads the ranged slot's item off the
/// object manager as the call runs (`0x4c99c2`), and the item is still held at its packet.
#[test]
fn an_earlier_listener_reads_the_weapon_a_later_packet_destroys() {
    let mut c = client();
    let ranged = FIELD_PLAYER_INV_SLOT_HEAD + 2 * u16::from(WAND.2);
    c.frame(vec![
        vec![values(ME, &[(STAT0 + 1, 60)])],
        vec![values(ME, &[(ranged, 0), (ranged + 1, 0)])],
        vec![SessionEvent::ObjectDestroyed(WAND.0)],
    ]);
    let seen: Vec<String> = c.eval(false, "return Seen");
    assert_eq!(seen, ["UNIT_STATS:60:1:93"]);
    let now: (Option<i64>, i64) =
        c.eval(true, "return HasWandEquipped(), UnitRangedAttack('player')");
    assert_eq!(now, (None, 0), "the wand is gone at the frame's end");
}

/// With a host lent and no player held, the active player's getters answer as the reference's do
/// for an absent player, 0 or nil (`GetMoney` `0x5181db`, `IsResting` `0x516ee8`, `GetDodgeChance`
/// `0x516f42`), never the copy an earlier session left in the VM.
#[test]
fn with_no_player_held_the_player_getters_answer_nothing() {
    let mut c = client();
    let chunk = "return GetMoney(), GetDodgeChance(), HasWandEquipped(), UnitXP('player'),
        GetXPExhaustion(), IsResting(), PartialPlayTime(), (GetRestState())";
    type Answers = (
        i64,
        f64,
        Option<i64>,
        i64,
        Option<f64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    );
    let held: Answers = c.eval(true, chunk);
    assert_eq!(
        held,
        (
            123_456,
            7.25,
            Some(1),
            4_200,
            Some(3000.0),
            Some(1),
            Some(1),
            Some(1)
        )
    );
    // Our object gone, as in the logout's frames, with the VM's copies and the seat left behind.
    let world = c.app.world_mut();
    let me = world
        .resource_mut::<crate::net::GuidIndex>()
        .0
        .remove(&ME)
        .expect("our object");
    world.despawn(me);
    let gone: Answers = c.eval(true, chunk);
    assert_eq!(gone, (0, 0.0, None, 0, None, None, None, None));
}

/// The stat getters' per-call cost with the host lent and off the snapshots: 10k calls of each of
/// a paper doll's getters for us, wearing the three weapons.
///
/// Run with `cargo nextest run -p benilla-app --lib stat_getter_cost --run-ignored only
/// --no-capture`.
#[test]
#[ignore = "a timing, not a check"]
fn stat_getter_cost() {
    let mut c = client();
    for call in [
        "UnitStat('player', 2)",
        "UnitArmor('player')",
        "UnitDefense('player')",
        "UnitAttackBothHands('player')",
        "UnitRangedDamage('player')",
        "UnitStat('pet', 2)",
        "GetDodgeChance()",
        "GetMoney()",
        "UnitXP('player')",
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
