//! The unit aura getters read the unit's aura slots off its object as they run, through the lent
//! host, as the reference's walk the fields at the call (`UnitBuff` `0x519500` from `[unit+0x110]`
//! `+0xa4`, `UnitDebuff` `0x5198f0` from `+0x124`, `GameTooltip:SetUnitBuff` `0x534ac0`), and gate
//! the buffs on the active player as it stands then (`0x5195be`-`0x5195f7`): every leg of every
//! getter reads the object, and a listener reads its own packet's auras.

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDispelTypes, SpellDisplay};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{member_status, GroupMemberEntry, PartyMemberStatsInfo};
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
/// A party member out of view, whose roster record stands in for it (`0x519735`).
const FAR: u64 = 0x23;
/// A creature no token names.
const BOAR: u64 = 0xF130_0000_0047_0004;

/// `UNIT_FIELD_CHARMEDBY`, `UNIT_FIELD_SUMMON` and `UNIT_FIELD_FLAGS`.
const CHARMEDBY: u16 = 10;
const SUMMON: u16 = 8;
const FLAGS: u16 = 46;
/// `UNIT_FIELD_AURA`, `AURAFLAGS`, `AURALEVELS` and `AURAAPPLICATIONS`.
const AURA: u16 = 47;
const AURAFLAGS: u16 = 95;
const AURALEVELS: u16 = 101;
const APPLICATIONS: u16 = 113;
/// `UNIT_FLAG_AURAS_VISIBLE`, which opens `UnitBuff`'s unit gate on any unit (`0x5195b6`).
const AURAS_VISIBLE: u32 = 0x0800_0000;

/// The spells: two buffs, one the player can cast; a Magic and a Curse debuff, the Magic one the
/// player can dispel; and a buff only an aura vision shows (`RequiredAuraVision`, `0x51989c`).
const MARK: u32 = 1126;
const FORT: u32 = 1243;
const PAIN: u32 = 589;
const AGONY: u32 = 980;
const SEEN: u32 = 26869;
/// A friendly Magic dispel the player knows, which lists Magic debuffs for `raidFilter`.
const DISPEL: u32 = 527;

/// The tokens the agreement walks: every base a unit here answers to, a `target` hop, and a
/// group member out of view.
const TOKENS: [&str; 7] = [
    "player",
    "target",
    "targettarget",
    "pet",
    "party1",
    "partypet1",
    "party2",
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

    /// Every census getter's answer for every token, one line each: `UnitBuff` and `UnitDebuff`
    /// at every index of each half and past it, with and without `raidFilter`, and the tooltip's
    /// `SetUnitBuff` and `SetUnitDebuff` lines.
    fn answers(&mut self) -> Vec<String> {
        let tokens = TOKENS
            .iter()
            .map(|t| format!("'{t}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let chunk = format!(
            "local out = {{}}
            local tt = AuraTip
            local function lines()
                local t = {{}}
                for i = 1, tt:NumLines() do
                    table.insert(t, getglobal('AuraTipTextLeft' .. i):GetText() or '')
                end
                return table.concat(t, '|')
            end
            for _, unit in ipairs({{ {tokens} }}) do
                local function note(what, ...)
                    for i = 1, table.getn(arg) do arg[i] = tostring(arg[i]) end
                    table.insert(out, unit .. ' ' .. what .. ' ' .. table.concat(arg, ','))
                end
                for i = 0, 5 do
                    for _, raid in ipairs({{ 'nil', '1' }}) do
                        local filter = raid == '1' and 1 or nil
                        note('UnitBuff ' .. i .. ' ' .. raid, UnitBuff(unit, i, filter))
                        note('UnitDebuff ' .. i .. ' ' .. raid, UnitDebuff(unit, i, filter))
                        tt:SetOwner(AuraOwner, 'ANCHOR_NONE')
                        tt:SetUnitBuff(unit, i, filter)
                        note('SetUnitBuff ' .. i .. ' ' .. raid, lines())
                        tt:SetOwner(AuraOwner, 'ANCHOR_NONE')
                        tt:SetUnitDebuff(unit, i, filter)
                        note('SetUnitDebuff ' .. i .. ' ' .. raid, lines())
                    end
                end
            end
            return out"
        );
        self.eval(true, &chunk)
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

/// Slot `slot` holding `spell` at `stacks`, cast at `level`, shown and cancelable on the buff half
/// (`AURAFLAGS` `0x9`), shown on the debuff half (`0x8`).
fn aura(slot: u16, spell: u32, stacks: u32, level: u32) -> Vec<(u16, u32)> {
    let nibble: u32 = if slot < 32 { 0x9 } else { 0x8 };
    vec![
        (AURA + slot, spell),
        (AURAFLAGS + slot / 8, nibble << (4 * (slot % 8))),
        (AURALEVELS + slot / 4, level << (8 * (slot % 4))),
        (APPLICATIONS + slot / 4, (stacks - 1) << (8 * (slot % 4))),
    ]
}

/// Several slots' pairs merged, the shared dwords or-ed together.
fn auras(slots: &[(u16, u32, u32, u32)]) -> Vec<(u16, u32)> {
    let mut by_field: std::collections::BTreeMap<u16, u32> = Default::default();
    for &(slot, spell, stacks, level) in slots {
        for (field, v) in aura(slot, spell, stacks, level) {
            *by_field.entry(field).or_default() |= v;
        }
    }
    by_field.into_iter().collect()
}

/// The spells' rows and their dispel classes.
fn spells() -> crate::ui_action::Spells {
    let row = |id: u32, name: &str, f: fn(&mut SpellDisplay)| {
        let mut d = SpellDisplay {
            id,
            name: name.into(),
            icon: Some(format!("Interface\\Icons\\Spell_{id}")),
            ..Default::default()
        };
        f(&mut d);
        (id, d)
    };
    let catalog = SpellCatalog::from_displays(HashMap::from([
        row(MARK, "Mark of the Wild", |_| {}),
        row(FORT, "Power Word: Fortitude", |_| {}),
        row(PAIN, "Shadow Word: Pain", |d| d.dispel = 1),
        row(AGONY, "Curse of Agony", |d| d.dispel = 2),
        row(SEEN, "Amorous", |d| d.required_aura_vision = 1),
        row(DISPEL, "Dispel Magic", |d| {
            (d.effects[1], d.effect_misc_value[1]) = (38, 1);
            d.effect_implicit_target_a[1] = 21;
        }),
    ]))
    .with_dispel_types(SpellDispelTypes::from_names(HashMap::from([
        (1, "Magic".to_string()),
        (2, "Curse".to_string()),
    ])));
    crate::ui_action::Spells {
        catalog,
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// Every aura kind on one unit: a buff, a stacked buff the player can cast, a vision-hidden buff,
/// a Magic debuff the player can dispel and a stacked Curse.
fn every_kind() -> Vec<(u16, u32)> {
    every_kind_at(1, 1)
}

/// [`every_kind`] with the first buff at `mark` stacks and the Magic debuff at `pain`.
fn every_kind_at(mark: u32, pain: u32) -> Vec<(u16, u32)> {
    auras(&[
        (0, MARK, mark, 40),
        (1, FORT, 3, 52),
        (2, SEEN, 1, 1),
        (32, PAIN, pain, 60),
        (33, AGONY, 2, 58),
    ])
}

/// `UNIT_FLAG_PLAYER_CONTROLLED`, which a player and its pet carry.
const PLAYER_CONTROLLED: u32 = 0x8;
/// `UNIT_FIELD_CREATEDBY`, the pet's owner the relation reads.
const CREATEDBY: u16 = 14;

/// A pet of `owner`'s: created by it, player-controlled.
fn pet_of(owner: u64) -> Vec<(u16, u32)> {
    let mut pairs = unit(&every_kind());
    pairs.extend([
        (CREATEDBY, owner as u32),
        (CREATEDBY + 1, (owner >> 32) as u32),
        (FLAGS, PLAYER_CONTROLLED),
    ]);
    pairs
}

/// `OBJECT_FIELD_TYPE`, and the type masks of a unit and a player (object, unit, player bits).
const OBJECT_TYPE: u16 = 2;
const TYPE_UNIT: u32 = 0x9;
const TYPE_PLAYER: u32 = 0x19;

/// A unit's descriptor at 100 health with `extra`.
fn unit(extra: &[(u16, u32)]) -> Vec<(u16, u32)> {
    let mut pairs = vec![
        (OBJECT_TYPE, TYPE_UNIT),
        (FIELD_UNIT_HEALTH, 100),
        (FIELD_UNIT_MAXHEALTH, 100),
    ];
    pairs.extend_from_slice(extra);
    pairs
}

/// `UNIT_FIELD_SUMMON` naming `guid`.
fn summons(guid: u64) -> [(u16, u32); 2] {
    [(SUMMON, guid as u32), (SUMMON + 1, (guid >> 32) as u32)]
}

/// The client in the world as [`ME`] with [`PET`], in a party with [`MATE`] (in view, with
/// [`MATE_PET`]) and [`FAR`] (out of view), [`WOLF`] selected and targeting us, [`BOAR`] at its
/// side; every unit wearing [`every_kind`], and a spy on `UNIT_AURA` that reads each unit's first
/// buff and first debuff.
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
    app.insert_resource(spells());
    let script = UiScript::new().expect("a VM");
    crate::ui_script::test_ui::load_tooltip_template(&script);
    script
        .run(
            r#"
            AuraTip = CreateFrame("GameTooltip", "AuraTip", nil, "TestTooltipTemplate")
            AuraOwner = CreateFrame("Frame")
            Seen = {}
            local f = CreateFrame("Frame")
            f:RegisterEvent("UNIT_AURA")
            f:SetScript("OnEvent", function()
                local _, stacks = UnitBuff(arg1, 1)
                local _, debuff_stacks, dispel = UnitDebuff(arg1, 1)
                table.insert(Seen, arg1 .. ":" .. tostring(stacks) .. ":"
                    .. tostring(debuff_stacks) .. ":" .. tostring(dispel) .. ":"
                    .. UnitHealth(arg1))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    // The player knows Fortitude and a friendly Magic dispel, `raidFilter`'s two books.
    if let Some(mut actions) = app
        .world_mut()
        .get_resource_mut::<crate::ui_action::PlayerActions>()
    {
        actions.spells.extend([FORT, DISPEL]);
        // The dispel's type in the book's list, as its learn appends it (`0x4b26a2`).
        actions.dispels.push(1);
    }
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    let roster = SessionEvent::GroupList {
        group_type: 0,
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
            cur_hp: Some(100),
            max_hp: Some(100),
            auras: Some(vec![(0, MARK as u16), (1, FORT as u16), (2, SEEN as u16)]),
            auras_negative: Some(vec![(32, PAIN as u16), (33, AGONY as u16)]),
            ..Default::default()
        }),
    };
    let kinds = every_kind();
    let mut me = unit(&kinds);
    me.extend(summons(PET));
    me.extend([(FLAGS, PLAYER_CONTROLLED), (OBJECT_TYPE, TYPE_PLAYER)]);
    let mut mate = unit(&kinds);
    mate.extend(summons(MATE_PET));
    mate.extend([(FLAGS, PLAYER_CONTROLLED), (OBJECT_TYPE, TYPE_PLAYER)]);
    let mut wolf = unit(&kinds);
    wolf.extend(targets(ME));
    c.frame(vec![
        vec![roster],
        vec![far],
        vec![created(ME, EntityKind::Player, &me)],
        vec![created(PET, EntityKind::Unit, &pet_of(ME))],
        vec![created(MATE, EntityKind::Player, &mate)],
        vec![created(MATE_PET, EntityKind::Unit, &pet_of(MATE))],
        vec![created(WOLF, EntityKind::Unit, &wolf)],
        vec![created(BOAR, EntityKind::Unit, &unit(&kinds))],
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
    c.eval::<()>(false, "Seen = {}");
    c
}

/// Every census getter answers off the live object for every token, through the slots and the
/// gate the walk takes ([`crate::ui_aura::UnitAuraSource`]).
#[test]
fn every_aura_getter_answers_off_the_live_object() {
    let mut c = client();
    let live = c.answers();
    assert_eq!(live.len(), TOKENS.len() * 6 * 2 * 4);
    // The fixture reaches every leg: a stacked buff, the vision test, the unit gate, the
    // dispel class, `raidFilter` on both halves, the roster record and the tooltip.
    for line in [
        "player UnitBuff 2 nil Interface\\Icons\\Spell_1243,3",
        "player UnitBuff 3 nil nil",
        "player UnitBuff 1 1 Interface\\Icons\\Spell_1243,3",
        "player UnitDebuff 1 nil Interface\\Icons\\Spell_589,1,Magic",
        "player UnitDebuff 2 nil Interface\\Icons\\Spell_980,2,Curse",
        "player UnitDebuff 1 1 Interface\\Icons\\Spell_589,1,Magic",
        "player UnitDebuff 2 1 nil",
        "player SetUnitBuff 3 nil Amorous",
        "target UnitBuff 1 nil nil",
        "target UnitDebuff 2 nil Interface\\Icons\\Spell_980,2,Curse",
        "target SetUnitBuff 1 nil Mark of the Wild",
        "party2 UnitBuff 2 nil Interface\\Icons\\Spell_1243,1",
        "party2 UnitDebuff 2 nil Interface\\Icons\\Spell_980,1,Curse",
        "partypet1 UnitDebuff 1 nil Interface\\Icons\\Spell_589,1,Magic",
    ] {
        assert!(live.iter().any(|l| l == line), "{line:?} among {live:#?}");
    }
}

/// A store a packet moved after the frame's feeds reads as the object holds it, through the
/// host, for the token's own unit and through a `target` hop.
#[test]
fn an_aura_getter_reads_the_object_as_it_runs() {
    let mut c = client();
    c.write_store(WOLF, &every_kind_at(1, 4));
    c.write_store(ME, &every_kind_at(5, 1));
    let chunk = "local _, wolf = UnitDebuff('target', 1)
        local _, mine = UnitBuff('targettarget', 1)
        return wolf, mine";
    let live: (i64, i64) = c.eval(true, chunk);
    assert_eq!(live, (4, 5));
}

/// A unit no token names, so no list of the frame holds its auras, reads them the moment a verb
/// in the chunk names it: `TargetLastTarget` selects inside its call (`0x489b00`), and
/// `UnitDebuff` walks the object it then resolves (`0x51955e`, `0x519979`).
#[test]
fn a_unit_targeted_in_the_chunk_reads_its_auras_there() {
    let mut c = client();
    // The boar the last target, which no token names.
    c.app
        .world_mut()
        .resource_mut::<crate::target::Selection>()
        .last = Some(BOAR);
    c.frame(vec![]);
    let chunk = "TargetLastTarget() return UnitDebuff('target', 2)";
    let picked: (Option<String>, Option<i64>, Option<String>) = c.eval(true, chunk);
    assert_eq!(
        picked,
        (
            Some("Interface\\Icons\\Spell_980".into()),
            Some(2),
            Some("Curse".into())
        )
    );
}

/// Two packets in one frame: the first moves our debuff's stacks, the second charms us. The
/// `UNIT_AURA` listener of the first reads our buffs, as `UnitBuff`'s gate reads the active
/// player's `UNIT_FIELD_CHARMEDBY` at the call (`0x5195f1`), still clear at that packet; and the
/// stacks that packet wrote.
#[test]
fn a_unit_aura_listener_reads_the_gate_as_its_own_packet_left_us() {
    let mut c = client();
    c.frame(vec![
        vec![values(ME, &every_kind_at(1, 2))],
        vec![values(
            ME,
            &[
                (CHARMEDBY, BOAR as u32),
                (CHARMEDBY + 1, (BOAR >> 32) as u32),
                (FIELD_UNIT_HEALTH, 50),
            ],
        )],
    ]);
    let mut seen: Vec<String> = c.eval(false, "return Seen");
    seen.dedup();
    assert_eq!(seen, ["player:1:2:Magic:100"]);
    // Charmed at the frame's end: our buffs are hidden from us.
    let now: Option<String> = c.eval(true, "return (UnitBuff('player', 1))");
    assert_eq!(now, None);
}

/// Two packets in one frame, each moving the stacks of every token's unit: each `UNIT_AURA`
/// listener reads its own packet's, not the frame's last.
#[test]
fn each_unit_aura_listener_reads_its_own_packets_auras() {
    let mut c = client();
    c.write_store(WOLF, &[(FLAGS, AURAS_VISIBLE)]);
    c.frame(vec![]);
    c.eval::<()>(false, "Seen = {}");
    let packet = |stacks: u32| {
        [WOLF, PET, MATE]
            .map(|g| values(g, &every_kind_at(stacks, stacks)))
            .into()
    };
    c.frame(vec![packet(2), packet(4)]);
    let mut seen: Vec<String> = c.eval(false, "return Seen");
    seen.sort();
    assert_eq!(
        seen,
        [
            "party1:2:2:Magic:100",
            "party1:4:4:Magic:100",
            "pet:2:2:Magic:100",
            "pet:4:4:Magic:100",
            "target:2:2:Magic:100",
            "target:4:4:Magic:100",
        ]
    );
}

/// The aura getters' per-call cost with the host lent: 10k calls of each,
/// on a unit wearing [`every_kind`].
///
/// Run with `cargo nextest run -p benilla-app --lib aura_getter_cost --run-ignored only
/// --no-capture`.
#[test]
#[ignore = "a timing, not a check"]
fn aura_getter_cost() {
    let mut c = client();
    for call in [
        "UnitBuff('player', 1)",
        "UnitBuff('player', 3)",
        "UnitDebuff('target', 2)",
        "UnitDebuff('party2', 1)",
        "UnitBuff('player', 1, 1)",
    ] {
        let chunk = format!("for n = 1, 10000 do {call} end");
        let mut times = Vec::new();
        for _ in 0..30 {
            let t = std::time::Instant::now();
            c.eval::<()>(true, &chunk);
            times.push(t.elapsed().as_secs_f64() * 1e9 / 10_000.0);
        }
        times.sort_by(f64::total_cmp);
        let live = times[times.len() / 2];
        eprintln!("{call}: {live:.0} ns a call live");
    }
}
