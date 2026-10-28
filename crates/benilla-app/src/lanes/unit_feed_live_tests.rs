//! The unit getters no unit snapshot carries, as a listener reads them, and their events at their
//! packets. The relation verbs run the engine's functions over the units they resolve inside the
//! call (`0x5167e0` → `0x6061e0`), the range verbs measure the unit then (`0x48ba00`,
//! `0x48a26f`), and the language walk tests each `Languages.dbc` row then (`0x49fb30` →
//! `0x5ec720`), `GetDefaultLanguage` the player's race (`0x49fcd0` → `0x5ec890`). The far-sight
//! watch signals inside its values block (`0x5de0d0`), and the spell registrar's language leg
//! inside the learn (`0x4b2641` → `0x49b970`).

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;
use crate::target::relations::fixture::{factions, HUMAN, ORC};

const ME: u64 = 0x7;
const WOLF: u64 = 0xF130_0000_0044_0001;
/// `UNIT_FIELD_FACTIONTEMPLATE`, `UNIT_FIELD_BYTES_0` (the race in byte 0), `PLAYER_FARSIGHT`
/// and the first `PLAYER_SKILL_INFO` entry's skill id.
const TEMPLATE: u16 = 35;
const BYTES_0: u16 = 36;
const FARSIGHT: u16 = 712;
const SKILL_1: u16 = 718;
/// Two spells that teach Common (`Effect_1` 39, misc 7), on skill line 98, and a plain one whose
/// cooldown packet is another lane's.
const COMMON: u32 = 90_101;
const COMMON_TOO: u32 = 90_102;
const PLAIN: u32 = 90_103;

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

    /// `chunk`'s answer through the lent host when `live`, else off the VM's copies alone.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(&mut self, live: bool, chunk: &str) -> T {
        if live {
            crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
                s.expect("a VM").eval::<T>(chunk).expect("runs")
            })
        } else {
            crate::script_calls::eval_in_vm(self.app.world(), chunk)
        }
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        self.eval(false, "local s = Seen Seen = {} return s")
    }

    /// Fields written into `guid`'s store, with no packet and no frame: the world as a packet's
    /// handler leaves it, which the VM's copies have not heard of.
    fn write(&mut self, guid: u64, pairs: &[(u16, u32)]) {
        let world = self.app.world_mut();
        let entity = world.resource::<crate::net::GuidIndex>().0[&guid];
        world
            .get_mut::<crate::net::ObjectStore>(entity)
            .expect("the unit's store")
            .0
            .merge(ObjectFields::from_pairs(pairs));
    }

    /// `guid`'s unit placed `yards` along one axis, so d² is exactly yards².
    fn place(&mut self, guid: u64, yards: f32) {
        let world = self.app.world_mut();
        let entity = world.resource::<crate::net::GuidIndex>().0[&guid];
        world
            .entity_mut(entity)
            .insert(Transform::from_xyz(yards, 0.0, 0.0));
    }
}

fn spells() -> crate::ui_action::Spells {
    let language = |id: u32| {
        let mut d = SpellDisplay {
            id,
            name: format!("Language {id}"),
            ..Default::default()
        };
        d.effects[0] = 39;
        d.effect_misc_value[0] = 7;
        (id, d)
    };
    let plain = SpellDisplay {
        id: PLAIN,
        name: "Plain".into(),
        ..Default::default()
    };
    crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(
            [language(COMMON), language(COMMON_TOO), (PLAIN, plain)]
                .into_iter()
                .collect::<HashMap<_, _>>(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// The client in the world as a Human knowing nothing, its skill block holding line 98, targeting
/// an Orc-template wolf; a spy whose line for each family event carries its name.
fn client() -> Client {
    arrived(&[]).0
}

/// [`client`] knowing `known` as it arrives, with what the spy heard of its arrival.
fn arrived(known: &[u32]) -> (Client, Vec<String>) {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    app.insert_resource(bevy::ecs::error::DefaultErrorHandler(
        bevy::ecs::error::ignore,
    ));
    let (tx, sent) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetCommands(tx));
    let (inbound, rx) = crossbeam_channel::unbounded();
    app.insert_resource(crate::net::NetEvents::for_test(rx));
    app.update();
    app.insert_resource(spells())
        .insert_resource(factions())
        .insert_resource(crate::ui_spellbook::SkillLines {
            catalog: benilla_formats::SkillLineCatalog::from_spell_lines([
                (COMMON, 98),
                (COMMON_TOO, 98),
            ]),
        })
        .insert_resource(crate::ui_unit::LanguagesRes(
            benilla_formats::Languages::from_rows(&[(1, "Orcish"), (7, "Common")]),
        ))
        .insert_resource(crate::ui_unit::DefaultLanguagesRes(
            benilla_formats::DefaultLanguages::from_rows(&[(1, "Common"), (2, "Orcish")]),
        ));
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"LANGUAGE_LIST_CHANGED", "PLAYER_FARSIGHT_FOCUS_CHANGED",
                    "SPELL_UPDATE_COOLDOWN", "PLAYER_ENTERING_WORLD"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function() table.insert(Seen, event) end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    {
        // As `SMSG_INITIAL_SPELLS` leaves the book, for the arrival to learn again.
        let mut actions = app
            .world_mut()
            .resource_mut::<crate::ui_action::PlayerActions>();
        actions.spells.extend(known.iter().copied());
        actions.initial.extend(known.iter().copied());
    }
    let mut c = Client { app, inbound, sent };
    let unit = [(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)];
    c.frame(vec![vec![
        created(
            ME,
            EntityKind::Player,
            &[
                unit[0],
                unit[1],
                (TEMPLATE, HUMAN),
                (BYTES_0, 1),
                (SKILL_1, 98),
            ],
        ),
        created(WOLF, EntityKind::Unit, &[unit[0], unit[1], (TEMPLATE, ORC)]),
    ]]);
    let world = c.app.world_mut();
    let wolf = world.resource::<crate::net::GuidIndex>().0[&WOLF];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(wolf),
        guid: Some(WOLF),
        ..Default::default()
    };
    c.place(ME, 0.0);
    c.place(WOLF, 5.0);
    c.frame(vec![]);
    let seen = c.seen();
    (c, seen)
}

/// The relation verbs judge the units as the world holds them at the call, with no push between:
/// the wolf's template moves from Orc to Human, and its reaction toward us and our attack on it
/// follow; with no host lent, nothing in game seats a copy, so they answer nil.
#[test]
fn the_relation_verbs_read_the_world_at_the_call() {
    let mut c = client();
    let verbs = r#"return UnitReaction("target", "player"), UnitCanAttack("player", "target"),
                   UnitIsFriend("player", "target")"#;
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>, Option<i64>)>(true, verbs),
        (Some(2), Some(1), None),
        "an Orc wolf is hostile"
    );
    c.write(WOLF, &[(TEMPLATE, HUMAN)]);
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>, Option<i64>)>(true, verbs),
        (Some(5), None, Some(1)),
        "the template as the handler left it"
    );
    assert_eq!(
        c.eval::<Option<i64>>(false, r#"return UnitReaction("target", "player")"#),
        None,
        "no host, no copy"
    );
}

/// The range verbs measure the unit at the call: the wolf walks from 5 to 20 yards and back, and
/// `CheckInteractDistance` and `CanInspect` follow with no push between.
#[test]
fn the_range_verbs_measure_the_unit_at_the_call() {
    let mut c = client();
    let near = r#"return CheckInteractDistance("target", 3)"#;
    assert_eq!(c.eval::<Option<i64>>(true, near), Some(1));
    c.place(WOLF, 20.0);
    assert_eq!(c.eval::<Option<i64>>(true, near), None, "beyond 10 yards");
    assert_eq!(
        c.eval::<Option<i64>>(true, r#"return CheckInteractDistance("target", 4)"#),
        Some(1),
        "within 30"
    );
    // A creature is no inspect target at any distance (`0x4944d8`); we are.
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>)>(
            true,
            r#"return CanInspect("target"), CanInspect("player")"#
        ),
        (None, Some(1))
    );
}

/// The language walk and the default language read the player at the call: a learn's known
/// spells add Common, and a race byte the handler wrote names its own tongue.
#[test]
fn the_language_getters_read_the_player_at_the_call() {
    let mut c = client();
    let walk = "return GetNumLaguages(), GetLanguageByIndex(1)";
    assert_eq!(c.eval::<(i64, Option<String>)>(true, walk), (0, None));
    c.app
        .world_mut()
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .insert(COMMON);
    assert_eq!(
        c.eval::<(i64, Option<String>)>(true, walk),
        (1, Some("Common".into())),
        "the learn's spell, its line in the skill block"
    );
    c.write(ME, &[(SKILL_1, 0)]);
    assert_eq!(
        c.eval::<i64>(true, "return GetNumLaguages()"),
        0,
        "no skill line, no language (`0x5ec720`)"
    );
    let default = "return GetDefaultLanguage()";
    assert_eq!(c.eval::<String>(true, default), "Common");
    c.write(ME, &[(BYTES_0, 2)]);
    assert_eq!(c.eval::<String>(true, default), "Orcish", "the race now");
}

/// `PLAYER_FIELD_BYTES`, the combo points in byte 1 and the bar toggles in byte 2, and
/// `PLAYER_FIELD_COMBO_TARGET`, a guid pair.
const FIELD_BYTES: u16 = 1222;
const COMBO_TARGET: u16 = 714;

/// The active player's combo pair, bar toggles and faction masks are read off its object as the
/// call runs (`GetComboPoints` `0x51a1c3`-`0x51a234`, `GetActionBarToggles` `0x4e768c`,
/// `GetZonePVPInfo` `0x48d5d6`-`0x48d613`): what a handler wrote shows to the hosted getter with no
/// frame between, and the VM's copy, which no feed pushes, stays as it was across the frame.
#[test]
fn the_player_scalars_read_as_written() {
    let mut c = client();
    crate::script_calls::with_hosted_vm(c.app.world_mut(), |s| {
        let s = s.expect("a VM");
        s.set_realm_pvp(true);
        // An Alliance zone: friendly to the Human template (friend mask 2), hostile to the Orc
        // one (enemy mask 10).
        s.set_zone_texts(benilla_ui::script::ZoneTexts {
            pvp_zone: Some(benilla_ui::script::ZonePvp {
                faction_group_mask: 2,
                faction_name: "Alliance".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
    });
    let chunk = "return GetComboPoints(), (GetZonePVPInfo()), GetActionBarToggles()";
    type Scalars = (
        i64,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
    );
    let human = (0, Some("friendly".to_string()), None, None, None, None);
    assert_eq!(c.eval::<Scalars>(true, chunk), human);
    // A rogue (class 4, `UNIT_FIELD_BYTES_0` byte 1) of the Orc template, three points banked on
    // the wolf, the target, and the second and third bars on.
    c.write(
        ME,
        &[
            (BYTES_0, 1 | 4 << 8),
            (TEMPLATE, ORC),
            (FIELD_BYTES, 0b0110 << 16 | 3 << 8),
            (COMBO_TARGET, WOLF as u32),
            (COMBO_TARGET + 1, (WOLF >> 32) as u32),
        ],
    );
    let written = (3, Some("hostile".to_string()), None, Some(1), Some(1), None);
    assert_eq!(c.eval::<Scalars>(true, chunk), written);
    c.frame(vec![]);
    assert_eq!(c.eval::<Scalars>(true, chunk), written);
    assert_eq!(
        c.eval::<Scalars>(false, chunk),
        (0, None, None, None, None, None),
        "no feed pushes the scalars: the VM's copies hold nothing"
    );
}

/// `SMSG_SPELL_COOLDOWN` for us on [`PLAIN`], a packet of the cooldown lane's.
fn cooldown() -> Vec<SessionEvent> {
    vec![SessionEvent::SpellCooldowns {
        caster: ME,
        cooldowns: vec![(PLAIN, 8_000)],
    }]
}

/// Far sight set, then cleared, between two cooldown packets: each values block signals inside
/// itself, so the two signals interleave with the cooldown lane's as the packets came; a frame-end
/// diff would find no change at all.
#[test]
fn the_far_sight_watch_signals_at_its_block() {
    let mut c = client();
    let eye = 0xF130_0000_0099_0001u64;
    let anchor = |guid: u64| {
        vec![values(
            ME,
            &[(FARSIGHT, guid as u32), (FARSIGHT + 1, (guid >> 32) as u32)],
        )]
    };
    c.frame(vec![anchor(eye), cooldown(), anchor(0)]);
    assert_eq!(
        c.seen(),
        [
            "PLAYER_FARSIGHT_FOCUS_CHANGED",
            "SPELL_UPDATE_COOLDOWN",
            "PLAYER_FARSIGHT_FOCUS_CHANGED"
        ]
    );
}

/// Each learn of a language spell signals inside its handler, after the cooldown packet before it,
/// the second too though it teaches a language already understood (`0x4b2659`); a spell of no
/// language signals nothing.
#[test]
fn a_language_learn_signals_at_its_packet() {
    let mut c = client();
    let learn = |spell_id| vec![SessionEvent::SpellLearned { spell_id }];
    c.frame(vec![
        cooldown(),
        learn(COMMON),
        learn(PLAIN),
        learn(COMMON_TOO),
    ]);
    assert_eq!(
        c.seen(),
        [
            "SPELL_UPDATE_COOLDOWN",
            "LANGUAGE_LIST_CHANGED",
            "LANGUAGE_LIST_CHANGED"
        ]
    );
}

/// The signal is the spell registrar's alone, never a diff of the known languages: a removal
/// fires nothing (`0x4b2c50`), nor does a skill line arriving after its spell, and a learn of a
/// language already known fires again (`0x4b2659`).
#[test]
fn only_a_language_learn_signals_never_a_removal_or_a_skill_line() {
    let mut c = client();
    let learn = |spell_id| vec![SessionEvent::SpellLearned { spell_id }];
    c.frame(vec![learn(COMMON)]);
    assert_eq!(c.seen(), ["LANGUAGE_LIST_CHANGED"]);
    c.frame(vec![vec![SessionEvent::SpellRemoved { spell_id: COMMON }]]);
    assert_eq!(c.seen(), Vec::<String>::new(), "an unlearn");
    c.frame(vec![vec![values(ME, &[(SKILL_1, 0)])]]);
    c.frame(vec![vec![values(ME, &[(SKILL_1, 98)])]]);
    assert_eq!(c.seen(), Vec::<String>::new(), "a skill line");
    c.frame(vec![learn(COMMON)]);
    c.frame(vec![learn(COMMON)]);
    assert_eq!(c.seen(), ["LANGUAGE_LIST_CHANGED", "LANGUAGE_LIST_CHANGED"]);
}

/// A unit created in the frame reads its real distance before the scene's propagation has run:
/// the host composes its position from its `Transform` at the call, so the create's own packet
/// left it where it stands, not at the origin.
#[test]
fn a_unit_created_this_frame_reads_its_real_distance() {
    const LYNX: u64 = 0xF130_0000_0046_0003;
    let mut app = super::unit_view_tests::client();
    let mut create = created(LYNX, EntityKind::Unit, &[(FIELD_UNIT_HEALTH, 10)]);
    if let SessionEvent::ObjectCreate { position, .. } = &mut create {
        *position = [12.0, 0.0, 0.0];
    }
    super::unit_view_tests::frame(&mut app, vec![create]);
    let world = app.world_mut();
    let lynx = world.resource::<crate::net::GuidIndex>().0[&LYNX];
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target: Some(lynx),
        guid: Some(LYNX),
        ..Default::default()
    };
    let reach = crate::script_calls::with_hosted_vm(app.world_mut(), |s| {
        s.expect("a VM")
            .eval::<(Option<i64>, Option<i64>)>(
                r#"return CheckInteractDistance("target", 3), CheckInteractDistance("target", 4)"#,
            )
            .expect("runs")
    });
    assert_eq!(reach, (None, Some(1)), "12 yards: beyond 10, within 30");
}

/// The arrival's replay registers the known spells (`0x5dea96`-`0x5deab9` → `0x5e9c20` →
/// `0x4b25b0`), each language spell's leg signalling, ahead of the world-enter cascade's
/// `PLAYER_ENTERING_WORLD`: two language spells, two signals, a plain spell none.
#[test]
fn the_arrival_signals_each_language_spell_before_entering_the_world() {
    let (_, seen) = arrived(&[COMMON, PLAIN, COMMON_TOO]);
    assert_eq!(
        seen,
        [
            "LANGUAGE_LIST_CHANGED",
            "LANGUAGE_LIST_CHANGED",
            "PLAYER_ENTERING_WORLD"
        ]
    );
    let (_, seen) = arrived(&[PLAIN]);
    assert_eq!(seen, ["PLAYER_ENTERING_WORLD"]);
}

/// With no active player held, `GetNumLaguages` returns no value (`0x49fb5e` → `0x49fb60`), and
/// the walk finds nothing.
#[test]
fn the_language_count_is_no_value_with_no_player() {
    let mut c = client();
    let me = c.app.world().resource::<crate::net::GuidIndex>().0[&ME];
    c.app
        .world_mut()
        .entity_mut(me)
        .remove::<crate::net::SelfPlayer>();
    assert_eq!(
        c.eval::<(usize, usize)>(
            true,
            "return table.getn({GetNumLaguages()}), table.getn({GetLanguageByIndex(1)})"
        ),
        (0, 0)
    );
}

/// The unit a token resolves to is read at the call (`0x515970`, then `0x468460`): a raid mark
/// set by guid after the frame's push (`0x4bb4f1`) answers for `"target"` and for a chain ending on
/// the same unit, with no push between; with no host lent, the push answers.
#[test]
fn the_unit_a_token_names_is_read_at_the_call() {
    let mut c = client();
    // The wolf's `UNIT_FIELD_TARGET` is us, and ours the wolf, so `targettargettarget` ends on it.
    c.write(WOLF, &[(16, ME as u32), (17, 0)]);
    c.write(ME, &[(16, WOLF as u32), (17, (WOLF >> 32) as u32)]);
    c.frame(vec![]);
    let chunk = r#"return GetRaidTargetIndex("target"), GetRaidTargetIndex("targettargettarget")"#;
    type Answer = (Option<i64>, Option<i64>);
    assert_eq!(c.eval::<Answer>(true, chunk), (None, None));
    c.app
        .world_mut()
        .resource_mut::<crate::ui_party::GroupState>()
        .raid_targets[2] = WOLF;
    assert_eq!(
        c.eval::<Answer>(true, chunk),
        (Some(3), Some(3)),
        "the mark as it stands"
    );
    assert_eq!(
        c.eval::<Answer>(false, chunk),
        (None, None),
        "no host, the frame's push"
    );
}

/// The by-name walk offers the units as they stand at the call (`0x493aa0`, `0x468380` with
/// `0x493d30`): a player whose name lands after the frame is picked by `PromoteByName`, which
/// sends `CMSG_GROUP_SET_LEADER` for it (`0x48a86a`).
#[test]
fn the_by_name_walk_offers_the_units_at_the_call() {
    const ALLY: u64 = 0x9;
    let mut c = client();
    c.frame(vec![vec![created(
        ALLY,
        EntityKind::Player,
        &[
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (TEMPLATE, HUMAN),
        ],
    )]]);
    c.place(ALLY, 3.0);
    c.sent.try_iter().for_each(drop);
    c.app
        .world_mut()
        .resource_mut::<crate::names::NameCache>()
        .insert_player(ALLY, "Ally".into(), None);
    c.eval::<()>(true, r#"PromoteByName("Ally")"#);
    let leads: Vec<u64> = c
        .sent
        .try_iter()
        .filter_map(|cmd| match cmd {
            ClientCommand::GroupSetLeader { guid } => Some(guid),
            _ => None,
        })
        .collect();
    assert_eq!(leads, [ALLY]);
}

/// The cost of one getter call on `"target"`, with the host lent and off the snapshot: object
/// getters (`UnitHealth`, `UnitName`, `UnitLevel`), a state getter (`GetRaidTargetIndex`), a guid
/// compare (`UnitIsUnit`) and a bare Lua call (`type`) for the loop's own cost. A measurement, run
/// by hand: `cargo test -p benilla-app --lib getter_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn getter_cost() {
    let mut c = client();
    const N: u32 = 20_000;
    for getter in [
        "UnitHealth",
        "GetRaidTargetIndex",
        "UnitName",
        "UnitLevel",
        "UnitIsUnit",
        "type",
    ] {
        for live in [true, false] {
            let chunk = format!(r#"for i = 1, {N} do {getter}("target", "target") end"#);
            let start = std::time::Instant::now();
            c.eval::<()>(live, &chunk);
            let ns = start.elapsed().as_secs_f64() * 1e9 / f64::from(N);
            eprintln!("{getter}(\"target\") live={live}: {ns:.0} ns a call");
        }
    }
}

/// A transfer releases no target, so the cascade's `SelectTarget(0)` (`0x490927`) clears whatever
/// the guid still names, with its send, ahead of `PLAYER_ENTERING_WORLD` (`0x490965`) and signalled
/// once; the port itself sends and signals nothing.
#[test]
fn the_arrival_clears_the_target_before_entering_the_world() {
    let mut c = client();
    c.eval::<()>(
        true,
        "local f = CreateFrame('Frame') f:RegisterEvent('PLAYER_TARGET_CHANGED') \
         f:SetScript('OnEvent', function() table.insert(Seen, event) end)",
    );
    c.frame(vec![]);
    let _ = c.seen();
    let selected = |c: &Client| c.app.world().resource::<crate::target::Selection>().guid;
    assert_eq!(selected(&c), Some(WOLF));
    c.frame(vec![vec![SessionEvent::Worldport {
        map_id: 1,
        position: [0.0; 3],
        orientation: 0.0,
        needs_ack: true,
    }]]);
    assert_eq!(selected(&c), Some(WOLF), "the port releases no target");
    assert_eq!(c.seen(), Vec::<String>::new());
    let _ = c.sent.try_iter().count();
    let unit = [(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)];
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[
            unit[0],
            unit[1],
            (TEMPLATE, HUMAN),
            (BYTES_0, 1),
            (SKILL_1, 98),
        ],
    )]]);
    c.frame(vec![]);
    assert_eq!(c.seen(), ["PLAYER_TARGET_CHANGED", "PLAYER_ENTERING_WORLD"]);
    assert_eq!(selected(&c), None);
    assert!(c
        .sent
        .try_iter()
        .any(|s| matches!(s, ClientCommand::SetSelection { guid: 0 })));
}
