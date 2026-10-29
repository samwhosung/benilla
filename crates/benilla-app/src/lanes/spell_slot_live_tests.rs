//! The bar's legs of the spell packets, at their packet. An unlearn clears every slot standing for
//! its spell inside its handler (`UnlearnSpell 0x5e9fe0` → `ClearSpellSlots 0x4e5e20` at
//! `0x5ea253`), and a supersede clears and sets each spell slot of the old rank before its book
//! pair (`0x5e6356` → `ReplaceActionSpell 0x4e5e60`), each write sending and signalling
//! `ACTIONBAR_SLOT_CHANGED` there (`0x4e58e0`). The learn latches are written at the learn
//! (`0x4b262c`).

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::messages::{ActionButton, ACTION_KIND_SPELL};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, fall, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
const FIREBALL: u32 = 133;
/// Frostbolt's first two ranks.
const FROSTBOLT_1: u32 = 116;
const FROSTBOLT_2: u32 = 205;
/// Skinning's first rank, whose `Effect[0]` is `SPELL_EFFECT_SKINNING`.
const SKINNING: u32 = 8613;

/// What the spy records of every event it hears.
const EVENTS: &[&str] = &["ACTIONBAR_SLOT_CHANGED", "SPELLS_CHANGED", "UNIT_COMBAT"];

struct Client {
    app: App,
    inbound: Sender<SessionEvent>,
    sent: Receiver<ClientCommand>,
}

impl Client {
    /// One whole frame of the client's, with `packets` in the inbound channel, each one packet.
    fn frame(&mut self, packets: Vec<Vec<SessionEvent>>) -> Vec<String> {
        for ev in wire_packets(packets) {
            self.inbound.send(ev).expect("the inbound channel");
        }
        self.app.update();
        self.seen()
    }

    /// What the spy heard since the last call.
    fn seen(&mut self) -> Vec<String> {
        let script = self.app.world().non_send_resource::<UiScript>();
        let seen: Vec<String> = script.eval("local s = Seen Seen = {} return s").unwrap();
        seen
    }

    /// The bar's sends since the last call, as `(slot, word)`.
    fn sends(&self) -> Vec<(u8, u32)> {
        self.sent
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::SetActionButton { button, packed } => Some((button, packed)),
                _ => None,
            })
            .collect()
    }
}

fn spells() -> crate::ui_action::Spells {
    let row = |id: u32, effect: u32| {
        let mut d = SpellDisplay {
            id,
            name: format!("Spell {id}"),
            icon: Some(format!("Interface\\Icons\\Spell_{id}")),
            ..Default::default()
        };
        d.effects[0] = effect;
        (id, d)
    };
    crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([
            row(FIREBALL, 0),
            row(FROSTBOLT_1, 0),
            row(FROSTBOLT_2, 0),
            row(SKINNING, benilla_formats::SPELL_EFFECT_SKINNING),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// The client in the world as [`ME`], knowing Fireball and Frostbolt's first rank, with Fireball
/// in slot 1 and Frostbolt in slot 2, shown; a spy whose line for each [`EVENTS`] carries what a
/// listener reads of slots 1 and 2 as it fires.
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
    let list: Vec<String> = EVENTS.iter().map(|e| format!("{e:?}")).collect();
    script
        .run(&format!(
            r#"
            Seen = {{}}
            local f = CreateFrame("Frame")
            for _, e in ipairs({{{}}}) do f:RegisterEvent(e) end
            local function slot(s) return tostring(GetActionTexture(s)) end
            f:SetScript("OnEvent", function()
                if event == "UNIT_COMBAT" then table.insert(Seen, event) return end
                table.insert(Seen, event .. ":" .. tostring(arg1) .. ":" .. slot(1) .. ":"
                    .. slot(2))
            end)
            "#,
            list.join(", ")
        ))
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    let world = app.world_mut();
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    world
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .extend([FIREBALL, FROSTBOLT_1]);
    {
        let mut slots = world.resource_mut::<crate::ui_action::ActionSlots>();
        for (slot, action) in [(0, FIREBALL), (1, FROSTBOLT_1)] {
            slots.staged.insert(
                slot,
                ActionButton {
                    slot,
                    action,
                    kind: ACTION_KIND_SPELL,
                },
            );
        }
    }
    let mut c = Client { app, inbound, sent };
    c.frame(vec![vec![created(ME, EntityKind::Player, &[])]]);
    c.frame(vec![]);
    c.seen();
    c.sends();
    c
}

fn removed(spell_id: u32) -> SessionEvent {
    SessionEvent::SpellRemoved { spell_id }
}

fn superceded(old_spell_id: u32, new_spell_id: u32) -> SessionEvent {
    SessionEvent::SpellSuperceded {
        old_spell_id,
        new_spell_id,
    }
}

/// The icon a listener reads for `spell`'s slot.
fn icon(spell: u32) -> String {
    format!("Interface\\Icons\\Spell_{spell}")
}

/// With no frame after the unlearn, the bar holds no Fireball: the slot is cleared and sent as 0
/// inside the handler, and the VM shows it cleared, so a listener reads it so at the packet.
#[test]
fn a_listener_inside_an_unlearns_packet_reads_the_slot_cleared() {
    let mut c = client();
    assert!(c
        .app
        .world()
        .resource::<crate::ui_action::ActionSlots>()
        .buttons
        .contains_key(&0));
    crate::net::handlers::dispatch(c.app.world_mut(), vec![removed(FIREBALL)]);
    assert!(!c
        .app
        .world()
        .resource::<crate::ui_action::ActionSlots>()
        .buttons
        .contains_key(&0));
    assert_eq!(c.sends(), [(0, 0)], "the clear's send, at the packet");
    crate::lanes::fire_in_order(c.app.world_mut());
    let seen = c.seen();
    assert_eq!(
        seen.last().map(String::as_str),
        Some(format!("ACTIONBAR_SLOT_CHANGED:1:nil:{}", icon(FROSTBOLT_1)).as_str()),
        "{seen:?}"
    );
}

/// A supersede clears and sets Frostbolt's slot, each with its send and its signal, ahead of the
/// book pair's `SPELLS_CHANGED`; the set's listener reads the new rank on the bar. The feed after
/// the packets adds nothing.
#[test]
fn a_supersedes_listener_reads_the_new_rank_on_the_bar() {
    let mut c = client();
    let seen = c.frame(vec![vec![superceded(FROSTBOLT_1, FROSTBOLT_2)]]);
    let on_bar = format!("{}:{}", icon(FIREBALL), icon(FROSTBOLT_2));
    assert_eq!(seen.len(), 4, "{seen:?}");
    assert!(seen[0].starts_with("ACTIONBAR_SLOT_CHANGED:2:"), "{seen:?}");
    assert_eq!(
        seen[1..],
        [
            format!("ACTIONBAR_SLOT_CHANGED:2:{on_bar}"),
            format!("SPELLS_CHANGED:nil:{on_bar}"),
            format!("SPELLS_CHANGED:nil:{on_bar}"),
        ]
    );
    assert_eq!(
        c.sends(),
        [(1, 0), (1, FROSTBOLT_2)],
        "two packets, clear then set"
    );
    let slots = c.app.world().resource::<crate::ui_action::ActionSlots>();
    assert_eq!(slots.buttons[&1].action, FROSTBOLT_2);
    assert!(c.frame(vec![]).is_empty());
}

/// The unlearn's `ACTIONBAR_SLOT_CHANGED` fires at its packet's place, after its book's
/// `SPELLS_CHANGED` and between a fall's `UNIT_COMBAT` on each side; the identity feed signals the
/// slot no second time.
#[test]
fn actionbar_slot_changed_fires_between_neighbouring_packets_events() {
    let mut c = client();
    let seen = c.frame(vec![vec![fall(5)], vec![removed(FIREBALL)], vec![fall(5)]]);
    let after = format!("nil:{}", icon(FROSTBOLT_1));
    assert_eq!(
        seen,
        [
            "UNIT_COMBAT".to_string(),
            format!("SPELLS_CHANGED:nil:{after}"),
            format!("ACTIONBAR_SLOT_CHANGED:1:{after}"),
            "UNIT_COMBAT".to_string(),
        ]
    );
    assert!(c.frame(vec![]).is_empty());
}

/// The registrar latches Skinning at the learn's packet (`0x4b262c`), so a call inside that packet
/// reads it, and the unlearn clears it at its own (`0x4b2c8b`).
#[test]
fn a_learned_abilitys_latch_reads_at_the_learn_packet() {
    let mut c = client();
    let latch = |c: &Client| {
        c.app
            .world()
            .resource::<crate::ui_action::LearnedAbilities>()
            .skinning
    };
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::SpellLearned { spell_id: SKINNING }],
    );
    assert_eq!(latch(&c), Some(SKINNING));
    crate::net::handlers::dispatch(c.app.world_mut(), vec![removed(SKINNING)]);
    assert_eq!(latch(&c), None);
}

/// The next login in the same run, as B: `logout` runs the session's end first, else the session
/// dropped with no logout. Then B's login, B's book, which knows only Frostbolt's second rank, and
/// B's bar, in vmangos's order (`Player.cpp:19253`, `:19256`). A's bar held the first rank, which
/// A knew. Returns the bar's sends and B's staged words.
fn next_login(logout: bool) -> (Vec<(u8, u32)>, Vec<ActionButton>, ActionButton) {
    use benilla_formats::{SkillLineCatalog, SlaInfo};
    let mut c = client();
    // A's bar as A's login brought it.
    let a_bar: Vec<ActionButton> = {
        let slots = c.app.world().resource::<crate::ui_action::ActionSlots>();
        slots.buttons.values().copied().collect()
    };
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::ActionButtons { buttons: a_bar }],
    );
    let ability = |forward_spell_id| SlaInfo {
        skill_id: 6,
        req_skill_value: 0,
        forward_spell_id,
        trivial_low: 0,
        trivial_high: 0,
    };
    c.app.insert_resource(crate::ui_spellbook::SkillLines {
        catalog: SkillLineCatalog::from_abilities([
            (FROSTBOLT_1, ability(FROSTBOLT_2)),
            (FROSTBOLT_2, ability(0)),
        ]),
    });
    if logout {
        c.app
            .world_mut()
            .run_schedule(OnExit(crate::client_state::ClientState::InWorld));
    }
    let b_bar = ActionButton {
        slot: 5,
        action: FIREBALL,
        kind: ACTION_KIND_SPELL,
    };
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![
            SessionEvent::Connected {
                self_guid: 0x8,
                name: "Bee".into(),
                billing: Default::default(),
                tutorial_flags: None,
                addon_info: None,
            },
            SessionEvent::SpellBook {
                spell_ids: vec![FIREBALL, FROSTBOLT_2],
                cooldowns: Vec::new(),
            },
            SessionEvent::ActionButtons {
                buttons: vec![b_bar],
            },
        ],
    );
    let slots = c.app.world().resource::<crate::ui_action::ActionSlots>();
    assert!(slots.buttons.is_empty(), "B's bar waits for B's arrival");
    let bar = slots.staged.values().copied().collect();
    (c.sends(), bar, b_bar)
}

/// A worldport resends the book ahead of the bar (vmangos `MovementHandler.cpp:112` →
/// `Player.cpp:19253`, `:19256`). The book's rank pass reaches only a staging that waits for our
/// player's arrival, so it never sends the last staging, which the arrival loaded and the bar may
/// have moved on from; the worldport's own bar packet gets the pass.
#[test]
fn a_worldports_book_leaves_the_loaded_staging_alone() {
    use benilla_formats::{SkillLineCatalog, SlaInfo};
    let mut c = client();
    // This login's bar packet, then the arrival that loads it.
    let bar: Vec<ActionButton> = {
        let slots = c.app.world().resource::<crate::ui_action::ActionSlots>();
        slots.buttons.values().copied().collect()
    };
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::ActionButtons {
            buttons: bar.clone(),
        }],
    );
    c.app
        .world_mut()
        .run_system_cached(crate::ui_action::recount::arrival_slot_pass)
        .expect("the arrival's slot pass runs");
    c.sends();
    // Frostbolt's second rank trained, which supersedes the first in the book.
    let ability = |forward_spell_id| SlaInfo {
        skill_id: 6,
        req_skill_value: 0,
        forward_spell_id,
        trivial_low: 0,
        trivial_high: 0,
    };
    c.app.insert_resource(crate::ui_spellbook::SkillLines {
        catalog: SkillLineCatalog::from_abilities([
            (FROSTBOLT_1, ability(FROSTBOLT_2)),
            (FROSTBOLT_2, ability(0)),
        ]),
    });
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::SpellBook {
            spell_ids: vec![FIREBALL, FROSTBOLT_2],
            cooldowns: Vec::new(),
        }],
    );
    assert_eq!(c.sends(), [], "the book's pass waits for the bar");
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::ActionButtons { buttons: bar }],
    );
    assert_eq!(c.sends(), [(1, FROSTBOLT_2)], "the bar's pass moves it");
}

/// A relog in the same run: the game entry empties A's bar (`0x48f5a4` → `0x4e4f50`) and the
/// book's rank pass waits for B's bar, so neither moves or sends A's slots.
#[test]
fn a_relogs_rank_pass_never_reaches_the_last_characters_bar() {
    let (sent, bar, b_bar) = next_login(true);
    assert_eq!(sent, [], "nothing for A's slots");
    assert_eq!(bar, [b_bar]);
}

/// The session dropped with no logout, so no session end ran: the game entry alone empties A's
/// bar.
#[test]
fn a_login_after_a_dropped_session_never_reaches_the_last_characters_bar() {
    let (sent, bar, b_bar) = next_login(false);
    assert_eq!(sent, [], "nothing for A's slots");
    assert_eq!(bar, [b_bar]);
}
