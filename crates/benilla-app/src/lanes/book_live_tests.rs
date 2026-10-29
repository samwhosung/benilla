//! The skill list, the talents and the spell book as a listener reads them, and their events at
//! their packets. The getters read the live object and caches at the call: a skill row's rank off
//! its slot (`0x5ea3f0`, `0x4d3828`), a talent's rank off the known spells (`0x4f3200`), the point
//! counters off the player block (`0x519463`), the book's slots off its list (`0x4b3fe0`) and the
//! respec latch (`0x5df980`). The events fire inside the handler that raises them: a learn's
//! `SPELLS_CHANGED` and `LEARNED_SPELL_IN_TAB` (`0x4b2b5a`, `0x4b2b92`), the points watch's
//! `CHARACTER_POINTS_CHANGED` (`0x4c7098`) and the respec question's `CONFIRM_TALENT_WIPE`
//! (`0x5dfa3f`).

use std::collections::HashMap;

use benilla_formats::{SkillLineCatalog, SpellCatalog, SpellDisplay, Talent, TalentCatalog};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::field::FIELD_PLAYER_QUEST_LOG_1_1;
use benilla_protocol::messages::FIELD_PLAYER_SKILL_INFO_1_1;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
const TRAINER: u64 = 0xF130_0000_0000_0042;
/// A second trainer, held past the service range.
const FAR_TRAINER: u64 = 0xF130_0000_0000_0043;
/// `UNIT_FIELD_BYTES_0` of a male human mage.
const BYTES_0: u16 = 36;
const HUMAN_MAGE: u32 = 1 | (8 << 8);
/// `PLAYER_CHARACTER_POINTS1` and `2`, the player section's `+0xe48` and `+0xe4c`.
const POINTS_1: u16 = 1102;
const POINTS_2: u16 = 1103;
/// Slot 0's value and maximum word (`+0x84c`).
const SKILL_0_VALUES: u16 = FIELD_PLAYER_SKILL_INFO_1_1 + 1;
/// First Aid, in its own category.
const FIRST_AID: u32 = 129;
/// A spell in General, a spell of the Frost line, and the talent's two rank spells.
const SPARK: u32 = 90_001;
const FROSTBOLT: u32 = 90_002;
const FROST: u32 = 6;
const KINDLING_1: u32 = 90_011;
const KINDLING_2: u32 = 90_012;
/// Kindling's third rank, a trainer's, which supersedes the talent's own second.
const KINDLING_3: u32 = 90_013;
/// Two form spells, on the stance bar by their `MOD_SHAPESHIFT` form (`0x4b2810`).
const BEAR: u32 = 90_021;
const CAT: u32 = 90_022;
/// `UNIT_FIELD_LEVEL`.
const LEVEL: u16 = 34;

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

    /// Fields written into the player's store, with no packet and no frame: the world as a
    /// packet's handler leaves it, which the VM's copies have not heard of.
    fn write_player(&mut self, pairs: &[(u16, u32)]) {
        let world = self.app.world_mut();
        let entity = world.resource::<crate::net::GuidIndex>().0[&ME];
        world
            .get_mut::<crate::net::ObjectStore>(entity)
            .expect("the player's store")
            .0
            .merge(ObjectFields::from_pairs(pairs));
    }

    /// A spell known, as a learn's handler leaves the known spells.
    fn know(&mut self, spell_id: u32) {
        self.app
            .world_mut()
            .resource_mut::<crate::ui_action::PlayerActions>()
            .spells
            .insert(spell_id);
    }
}

fn spells() -> crate::ui_action::Spells {
    let row = |id: u32, name: &str, rank: Option<&str>| {
        (
            id,
            SpellDisplay {
                id,
                name: name.into(),
                rank: rank.map(Into::into),
                icon: Some(format!("Interface\\Icons\\Spell_{id}")),
                ..Default::default()
            },
        )
    };
    let form = |id: u32, name: &str, form: u32| {
        let (id, mut d) = row(id, name, None);
        d.shapeshift_form = Some(form);
        (id, d)
    };
    crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(
            [
                row(SPARK, "Spark", None),
                row(FROSTBOLT, "Frostbolt", Some("Rank 1")),
                row(KINDLING_1, "Kindling", Some("Rank 1")),
                row(KINDLING_2, "Kindling", Some("Rank 2")),
                row(KINDLING_3, "Kindling", Some("Rank 3")),
                form(BEAR, "Bear Form", 5),
                form(CAT, "Cat Form", 1),
            ]
            .into_iter()
            .collect::<HashMap<_, _>>(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// The mage's one tab, Fire, with one two-rank talent.
fn talents() -> crate::ui_talent::Talents {
    let tab = benilla_formats::TalentTabInfo {
        id: 41,
        name: "Fire".into(),
        race_mask: 511,
        class_mask: 1 << 7,
        background: "MageFire".into(),
        icon: None,
    };
    let mut ranks = [0; benilla_formats::MAX_TALENT_RANK];
    ranks[..2].copy_from_slice(&[KINDLING_1, KINDLING_2]);
    let kindling = Talent {
        id: 10,
        tab: 41,
        row: 0,
        col: 0,
        ranks,
        prereq_talents: [0; 3],
        prereq_ranks: [0; 3],
        required_spell: 0,
        exceptional: false,
    };
    crate::ui_talent::Talents {
        catalog: TalentCatalog::from_rows(vec![tab], vec![kindling]),
    }
}

/// First Aid, shown in its category; Frostbolt on the Frost line; Kindling's second rank
/// superseded by its third.
fn skill_lines() -> crate::ui_spellbook::SkillLines {
    let row = |skill_id: u32, forward_spell_id: u32| benilla_formats::SlaInfo {
        skill_id,
        req_skill_value: 0,
        forward_spell_id,
        trivial_low: 0,
        trivial_high: 0,
    };
    crate::ui_spellbook::SkillLines {
        catalog: SkillLineCatalog::from_abilities([
            (FROSTBOLT, row(FROST, 0)),
            (KINDLING_2, row(0, KINDLING_3)),
        ])
        .with_line_names([(FIRST_AID, "First Aid"), (FROST, "Frost")])
        .with_race_class_flags([(FIRST_AID, 0), (FROST, 0)])
        .with_category(0, "Secondary Skills", 1),
    }
}

/// A trainer's create `x` yards from the player along one axis.
fn trainer_at(guid: u64, x: f32) -> SessionEvent {
    let mut ev = created(guid, EntityKind::Unit, &[(FIELD_UNIT_HEALTH, 100)]);
    if let SessionEvent::ObjectCreate { position, .. } = &mut ev {
        *position = [x, 0.0, 0.0];
    }
    ev
}

/// [`arrived`] knowing Spark, the spy emptied.
fn client() -> Client {
    let mut c = arrived(&[SPARK]);
    c.seen();
    c
}

/// The client in the world as a human mage knowing `known`, five talent points, First Aid at 10
/// of 75; and a spy whose line for each family event carries its name and first argument, with
/// what the arrival fired.
fn arrived(known: &[u32]) -> Client {
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
    app.insert_resource(talents());
    app.insert_resource(skill_lines());
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"SPELLS_CHANGED", "LEARNED_SPELL_IN_TAB",
                    "CHARACTER_POINTS_CHANGED", "CONFIRM_TALENT_WIPE", "SPELL_UPDATE_COOLDOWN",
                    "UPDATE_SHAPESHIFT_FORMS", "TUTORIAL_TRIGGER", "UPDATE_LFG_TYPES"}) do
                f:RegisterEvent(e)
            end
            f:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(arg1))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    let world = app.world_mut();
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    {
        // As `SMSG_INITIAL_SPELLS` leaves the book, for the arrival to learn again.
        let mut actions = world.resource_mut::<crate::ui_action::PlayerActions>();
        actions.spells.extend(known.iter().copied());
        actions.initial.extend(known.iter().copied());
    }
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (BYTES_0, HUMAN_MAGE),
            (POINTS_1, 5),
            (FIELD_PLAYER_SKILL_INFO_1_1, FIRST_AID),
            (SKILL_0_VALUES, 10 | (75 << 16)),
        ],
    )]]);
    // The trainer within the service range, the other past it.
    c.frame(vec![
        vec![trainer_at(TRAINER, 3.0)],
        vec![trainer_at(FAR_TRAINER, 10.0)],
    ]);
    c.frame(vec![]);
    c
}

/// Each getter follows the world at the call, with no push between: the known spells a learn
/// leaves move the book and the talent's rank, the player block's words the points and the skill
/// row, and the respec latch `CheckTalentMasterDist`; with no host, the VM's copies answer.
#[test]
fn the_getters_read_the_world_at_the_call() {
    let mut c = client();
    let book = "local n = {} for i = 1, 3 do n[i] = GetSpellName(i, 'spell') or '-' end \
                return table.concat(n, ','), GetNumSpellTabs()";
    assert_eq!(c.eval::<(String, i64)>(true, book), ("Spark,-,-".into(), 1));
    c.know(FROSTBOLT);
    c.know(KINDLING_1);
    assert_eq!(
        c.eval::<(String, i64)>(true, book),
        ("Kindling,Spark,Frostbolt".into(), 2),
        "General's two by name, then the Frost tab"
    );
    assert_eq!(
        c.eval::<(String, i64)>(false, book),
        ("-,-,-".into(), 0),
        "with no host the copy answers"
    );
    let talent = "local _, _, _, _, rank, max = GetTalentInfo(1, 1) \
                  local _, _, spent = GetTalentTabInfo(1) return rank, max, spent";
    assert_eq!(c.eval::<(i64, i64, i64)>(true, talent), (1, 2, 1));
    c.know(KINDLING_2);
    assert_eq!(c.eval::<(i64, i64, i64)>(true, talent), (2, 2, 2));

    c.write_player(&[(POINTS_1, 3), (POINTS_2, 1)]);
    assert_eq!(
        c.eval::<(i64, i64)>(true, "return UnitCharacterPoints('player')"),
        (3, 1)
    );
    let skill = "local name, _, _, rank, _, _, max = GetSkillLineInfo(2) return name, rank, max";
    assert_eq!(
        c.eval::<(String, i64, i64)>(true, skill),
        ("First Aid".into(), 10, 75)
    );
    c.write_player(&[(SKILL_0_VALUES, 20 | (150 << 16))]);
    assert_eq!(
        c.eval::<(String, i64, i64)>(true, skill),
        ("First Aid".into(), 20, 150),
        "the slot's words, the list's rows unchanged"
    );

    let dist = "return CheckTalentMasterDist()";
    assert_eq!(c.eval::<Option<i64>>(true, dist), None);
    c.app
        .world_mut()
        .resource_mut::<crate::ui_talent_wipe::TalentWipeState>()
        .ask(TRAINER, 10_000);
    assert_eq!(c.eval::<Option<i64>>(true, dist), Some(1));
}

/// `SMSG_SPELL_COOLDOWN` for the player on Spark, a packet of the cooldown lane's.
fn cooldown() -> Vec<SessionEvent> {
    vec![SessionEvent::SpellCooldowns {
        caster: ME,
        cooldowns: vec![(SPARK, 8_000)],
    }]
}

/// A learn, a points change and a respec question in one frame, each between two cooldown
/// packets: each signals inside its handler, so its events interleave with the cooldown lane's as
/// the packets came, not ahead of them from a frame-end feed.
#[test]
fn the_familys_events_fire_at_their_packets_places() {
    let mut c = client();
    c.frame(vec![
        cooldown(),
        vec![SessionEvent::SpellLearned {
            spell_id: FROSTBOLT,
        }],
        cooldown(),
        vec![SessionEvent::ObjectValues {
            guid: ME,
            fields: ObjectFields::from_pairs(&[(POINTS_1, 4)]),
        }],
        cooldown(),
        vec![SessionEvent::TalentWipeConfirm {
            trainer: TRAINER,
            cost: 10_000,
        }],
        cooldown(),
    ]);
    assert_eq!(
        c.seen(),
        [
            "SPELL_UPDATE_COOLDOWN:nil",
            // The sort's signal, then the Frost tab's flash, second after General (`0x4b2b92`).
            "SPELLS_CHANGED:nil",
            "LEARNED_SPELL_IN_TAB:2",
            "SPELL_UPDATE_COOLDOWN:nil",
            // A point spent: the change of the first counter (`0x4c7086`).
            "CHARACTER_POINTS_CHANGED:-1",
            "SPELL_UPDATE_COOLDOWN:nil",
            "CONFIRM_TALENT_WIPE:10000",
            "SPELL_UPDATE_COOLDOWN:nil",
        ]
    );
}

/// A removal signals the book's sort only for a spell it held (`0x4b2f2f`), and a rank-up
/// signals the old rank's removal, then the new one's add with its tab (`0x4b2f50`).
#[test]
fn an_unlearn_and_a_rank_up_signal_the_book_at_their_packets() {
    let mut c = client();
    c.know(KINDLING_1);
    c.frame(vec![
        vec![SessionEvent::SpellRemoved {
            spell_id: FROSTBOLT,
        }],
        cooldown(),
        vec![SessionEvent::SpellSuperceded {
            old_spell_id: KINDLING_1,
            new_spell_id: KINDLING_2,
        }],
        cooldown(),
        vec![SessionEvent::SpellRemoved { spell_id: SPARK }],
    ]);
    assert_eq!(
        c.seen(),
        [
            "SPELL_UPDATE_COOLDOWN:nil",
            "SPELLS_CHANGED:nil",
            "SPELLS_CHANGED:nil",
            "LEARNED_SPELL_IN_TAB:1",
            "SPELL_UPDATE_COOLDOWN:nil",
            "SPELLS_CHANGED:nil",
        ]
    );
}

/// The arrival's replay lists each form spell with its own `UPDATE_SHAPESHIFT_FORMS`
/// (`0x5dea8b` → `0x4b2904`), then sorts the book and signals it (`0x5deac2`).
#[test]
fn the_arrival_lists_its_forms_then_signals_the_book() {
    let mut c = arrived(&[SPARK, BEAR, CAT]);
    let seen = c.seen();
    assert_eq!(
        seen[..3],
        [
            "UPDATE_SHAPESHIFT_FORMS:nil",
            "UPDATE_SHAPESHIFT_FORMS:nil",
            "SPELLS_CHANGED:nil"
        ],
        "{seen:?}"
    );
    assert_eq!(
        seen.iter()
            .filter(|l| l.starts_with("UPDATE_SHAPESHIFT_FORMS"))
            .count(),
        2,
        "one per form spell, none from the stance feed: {seen:?}"
    );
}

/// A form spell learned signals the form list (`0x4b2904`) ahead of the book's sort, its tab and
/// The Spellbook's tutorial 0x27 (`0x4b2b5a`-`0x4b2ba1`), all at its packet; unlearned, the list
/// (`0x4b2e4e`) ahead of the sort (`0x4b2f3f`).
#[test]
fn a_form_spell_signals_the_form_list_ahead_of_the_book() {
    let mut c = client();
    c.app
        .world()
        .resource::<crate::tutorial::Tutorials>()
        .lock()
        .apply_flags(&[0u8; 8]);
    c.frame(vec![
        cooldown(),
        vec![SessionEvent::SpellLearned { spell_id: BEAR }],
        cooldown(),
    ]);
    assert_eq!(
        c.seen(),
        [
            "SPELL_UPDATE_COOLDOWN:nil",
            "UPDATE_SHAPESHIFT_FORMS:nil",
            "SPELLS_CHANGED:nil",
            "LEARNED_SPELL_IN_TAB:1",
            "TUTORIAL_TRIGGER:40",
            "SPELL_UPDATE_COOLDOWN:nil",
        ]
    );
    assert_eq!(
        c.eval::<i64>(true, "return GetNumShapeshiftForms()"),
        1,
        "the list as the learn left it"
    );
    c.frame(vec![
        cooldown(),
        vec![SessionEvent::SpellRemoved { spell_id: BEAR }],
        cooldown(),
    ]);
    assert_eq!(
        c.seen(),
        [
            "SPELL_UPDATE_COOLDOWN:nil",
            "UPDATE_SHAPESHIFT_FORMS:nil",
            "SPELLS_CHANGED:nil",
            "SPELL_UPDATE_COOLDOWN:nil",
        ]
    );
}

/// A level change on the active player signals the book at its packet (`0x6045b0` →
/// `0x5edb7b`), then the looking-for-group type recount's `UPDATE_LFG_TYPES` (`0x5edb8a` →
/// `0x4e8b0e`).
#[test]
fn a_level_change_signals_the_book_at_its_packet() {
    let mut c = client();
    c.frame(vec![
        cooldown(),
        vec![SessionEvent::ObjectValues {
            guid: ME,
            fields: ObjectFields::from_pairs(&[(LEVEL, 2)]),
        }],
        cooldown(),
    ]);
    assert_eq!(
        c.seen(),
        [
            "SPELL_UPDATE_COOLDOWN:nil",
            "SPELLS_CHANGED:nil",
            "UPDATE_LFG_TYPES:nil",
            "SPELL_UPDATE_COOLDOWN:nil",
        ]
    );
}

/// A change of a quest-log slot of the active player runs the slot watcher `0x5ddd80`, whose
/// quest-list rebuild signals `UPDATE_LFG_TYPES` at the packet (`0x5dde98` → `0x4e8c70`).
#[test]
fn a_quest_log_slot_change_signals_the_lfg_types_at_its_packet() {
    let mut c = client();
    c.frame(vec![
        cooldown(),
        vec![SessionEvent::ObjectValues {
            guid: ME,
            fields: ObjectFields::from_pairs(&[(FIELD_PLAYER_QUEST_LOG_1_1, 5)]),
        }],
        cooldown(),
    ]);
    assert_eq!(
        c.seen(),
        [
            "SPELL_UPDATE_COOLDOWN:nil",
            "UPDATE_LFG_TYPES:nil",
            "SPELL_UPDATE_COOLDOWN:nil",
        ]
    );
}

/// A talent's rank counts a later rank known in its place (`0x4f32f5`, `0x60c8d0`): the talent's
/// second rank ranked past at a trainer, which drops it from the book, still reads rank 2.
#[test]
fn a_talent_ranked_past_at_a_trainer_keeps_its_rank() {
    let mut c = client();
    c.know(KINDLING_1);
    c.frame(vec![
        vec![SessionEvent::SpellSuperceded {
            old_spell_id: KINDLING_1,
            new_spell_id: KINDLING_2,
        }],
        vec![SessionEvent::SpellSuperceded {
            old_spell_id: KINDLING_2,
            new_spell_id: KINDLING_3,
        }],
    ]);
    let talent = "local _, _, _, _, rank = GetTalentInfo(1, 1) \
                  local _, _, spent = GetTalentTabInfo(1) return rank, spent";
    assert_eq!(c.eval::<(i64, i64)>(true, talent), (2, 2));
}

/// The respec question needs its trainer held (`0x5df9be`) and within `d² <= [0xc4c28c]` of the
/// player (`0x5dfa0d`); else no latch and no `CONFIRM_TALENT_WIPE`.
#[test]
fn the_respec_question_needs_its_trainer_in_range() {
    let mut c = client();
    let dist = "return CheckTalentMasterDist()";
    for trainer in [FAR_TRAINER, 0xF130_0000_0000_0099] {
        c.frame(vec![vec![SessionEvent::TalentWipeConfirm {
            trainer,
            cost: 10_000,
        }]]);
        assert_eq!(c.seen(), Vec::<String>::new(), "{trainer:#x}");
        assert_eq!(c.eval::<Option<i64>>(true, dist), None, "{trainer:#x}");
    }
    c.frame(vec![vec![SessionEvent::TalentWipeConfirm {
        trainer: TRAINER,
        cost: 10_000,
    }]]);
    assert_eq!(c.seen(), ["CONFIRM_TALENT_WIPE:10000"]);
    assert_eq!(c.eval::<Option<i64>>(true, dist), Some(1));
}

/// The learn line's strings and a spy for its `CHAT_MSG_SYSTEM`, beside the family's.
fn hear_learn_lines(c: &mut Client) {
    c.app
        .world_mut()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            ERR_LEARN_SPELL_S = "You have learned a new spell: %s."
            local g = CreateFrame("Frame")
            g:RegisterEvent("CHAT_MSG_SYSTEM")
            g:SetScript("OnEvent", function() table.insert(Seen, event .. ":" .. arg1) end)
            "#,
        )
        .unwrap();
}

/// The registrar shows a learn's line after its form list's edge (`0x4b2904`) and ahead of the
/// book's signals (`0x4b29b7`, then `0x4b2b5a`); a rank-up's between its old rank's removal
/// (`0x4b2f5b` → `0x4b2f3f`) and its new rank's add (`0x4b2f68`).
#[test]
fn a_learns_line_falls_between_the_form_list_and_the_book() {
    let mut c = client();
    hear_learn_lines(&mut c);
    c.app
        .world()
        .resource::<crate::tutorial::Tutorials>()
        .lock()
        .apply_flags(&[0u8; 8]);
    c.frame(vec![vec![SessionEvent::SpellLearned { spell_id: BEAR }]]);
    assert_eq!(
        c.seen(),
        [
            "UPDATE_SHAPESHIFT_FORMS:nil",
            "CHAT_MSG_SYSTEM:You have learned a new spell: Bear Form.",
            "SPELLS_CHANGED:nil",
            "LEARNED_SPELL_IN_TAB:1",
            "TUTORIAL_TRIGGER:40",
        ]
    );
    c.know(KINDLING_1);
    c.frame(vec![vec![SessionEvent::SpellSuperceded {
        old_spell_id: KINDLING_1,
        new_spell_id: KINDLING_2,
    }]]);
    assert_eq!(
        c.seen(),
        [
            "SPELLS_CHANGED:nil",
            "CHAT_MSG_SYSTEM:You have learned a new spell: Kindling (Rank 2).",
            "SPELLS_CHANGED:nil",
            "LEARNED_SPELL_IN_TAB:1",
        ]
    );
}

/// A rank-up reaches the registrar ahead of its player test (`0x5e6363`, then `0x5e6385`), so it
/// shows its line with no player held; a learn joins the initial list and shows none
/// (`0x5e61fb`).
#[test]
fn a_rank_up_shows_its_line_with_no_player_held() {
    let mut c = client();
    hear_learn_lines(&mut c);
    let world = c.app.world_mut();
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).remove::<crate::net::SelfPlayer>();
    c.know(KINDLING_1);
    c.frame(vec![
        vec![SessionEvent::SpellLearned {
            spell_id: FROSTBOLT,
        }],
        vec![SessionEvent::SpellSuperceded {
            old_spell_id: KINDLING_1,
            new_spell_id: KINDLING_2,
        }],
    ]);
    assert_eq!(
        c.seen(),
        ["CHAT_MSG_SYSTEM:You have learned a new spell: Kindling (Rank 2)."]
    );
}
