//! Our cast's state as a listener reads it, packet by packet. The cast state is the client's
//! globals: the in-flight slot (`0xceca88`, its item at `0xcecab0`), the targeting word and its
//! record's spell (`0xceac58`), the auto-repeat key (`0xceac30`) and the attack flag
//! (`[player+0xc48]`). Each packet's handler writes them and signals inside itself:
//! `SMSG_SPELL_START` fires `SPELLCAST_START` (`0x6e7a53`) with nothing written, and our cast's
//! `SMSG_CAST_RESULT` pops the in-flight slot (`0x6e741a` → `0x6e4940` → `0x6e4ad0`) before its
//! `SPELLCAST_STOP` (`0x6e4a50`). The getters read the globals inside the call: `IsCurrentAction`
//! (`0x4e72f0` → `0x4e53a0`, the in-flight spell at `0x4e54c1`), `IsAutoRepeatAction`
//! (`0x4e7360` → `0x4e55b0` → `0x6e9fd0`), `SpellIsTargeting` (`0x6e6cd0`) and `SpellStopCasting`'s
//! test (`0x6e6e80`). So a cast start and its stop in one frame each read their own packet's state.

use std::collections::HashMap;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::{ActionButton, ACTION_KIND_SPELL};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// A cast with no target, on bar slot 1 and in the book.
const HEAL: u32 = 2050;

/// `UNIT_FIELD_FLAGS` and its player-controlled bit, `UNIT_FIELD_POWER1` and its maximum.
const FLAGS: u16 = 46;
const PLAYER_CONTROLLED: u32 = 0x8;
const MANA: u16 = 23;
const MAX_MANA: u16 = 29;

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

    /// `chunk` run with the host lent, as a key's body: the press a cast needs.
    fn press(&mut self, chunk: &str) {
        crate::script_calls::with_hosted_vm(self.app.world_mut(), |s| {
            s.expect("a VM").run_binding(chunk)
        })
        .expect("runs");
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

    /// What the spy heard since the last call, its events among `events` alone.
    fn seen(&mut self, events: &[&str]) -> Vec<String> {
        let seen: Vec<String> = self.eval(false, "local s = Seen Seen = {} return s");
        seen.into_iter()
            .filter(|l| events.iter().any(|e| l.split(':').next() == Some(e)))
            .collect()
    }

    /// The spells cast since the last call.
    fn casts(&self) -> Vec<u32> {
        self.sent
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { spell_id, .. } => Some(spell_id),
                _ => None,
            })
            .collect()
    }
}

/// The client in the world as [`ME`], Heal in the book and on bar slot 1; and a spy whose line
/// for each cast event carries what its listener read: `IsCurrentAction(1)` and
/// `SpellIsTargeting()`.
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
    app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            HEAL,
            SpellDisplay {
                id: HEAL,
                name: "Heal".into(),
                icon: Some("Interface\\Icons\\Spell_Holy_Heal".into()),
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    let script = UiScript::new().expect("a VM");
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"SPELLCAST_START", "SPELLCAST_STOP", "SPELLCAST_FAILED",
                    "CURRENT_SPELL_CAST_CHANGED", "UNIT_MANA", "SPELL_UPDATE_COOLDOWN"}) do
                f:RegisterEvent(e)
            end
            local function s(v) return tostring(v) end
            f:SetScript("OnEvent", function()
                if event == "UNIT_MANA" and arg1 ~= "player" then return end
                table.insert(Seen, event .. ":" .. s(IsCurrentAction(1)) .. ":"
                    .. s(SpellIsTargeting()))
            end)
            "#,
        )
        .unwrap();
    app.insert_non_send_resource(script);
    app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
    benilla_world::world_point::init_world_point_resources(app.world_mut());
    let world = app.world_mut();
    world.resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    world
        .resource_mut::<crate::ui_action::PlayerActions>()
        .spells
        .extend([HEAL]);
    {
        let mut slots = world.resource_mut::<crate::ui_action::ActionSlots>();
        slots.staged.insert(
            0,
            ActionButton {
                slot: 0,
                action: HEAL,
                kind: ACTION_KIND_SPELL,
            },
        );
    }
    let mut c = Client { app, inbound, sent };
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (MANA, 100),
            (MAX_MANA, 100),
            (FLAGS, PLAYER_CONTROLLED),
        ],
    )]]);
    c.frame(vec![]);
    c.frame(vec![]);
    c.seen(&[]);
    c.casts();
    c
}

/// Heal pressed and sent, in flight as the next frame begins.
fn cast_heal(c: &mut Client) {
    c.press("UseAction(1)");
    assert_eq!(c.casts(), [HEAL], "the cast went out");
    c.frame(vec![]);
    c.seen(&[]);
}

/// Our cast's start and its result in one frame: the start's `SPELLCAST_START` reads Heal still
/// in flight, and the result's `0x6e741a` → `0x6e4940` pops it (`0x6e4ad0` at `0x6e49d6`), then
/// signals `CURRENT_SPELL_CAST_CHANGED` (`0x4b3250`) and `SPELLCAST_STOP` (`0x6e4a50`), which read
/// it popped; a power packet between them reads the slot as the start left it.
#[test]
fn a_cast_start_and_its_stop_in_one_frame_each_read_their_own_packets_state() {
    let mut c = client();
    cast_heal(&mut c);
    c.frame(vec![
        vec![SessionEvent::SpellStart {
            caster: ME,
            spell_id: HEAL,
            cast_flags: 0,
            cast_time_ms: 2500,
            target: None,
            ammo_display_id: None,
        }],
        vec![values(ME, &[(MANA, 90)])],
        vec![SessionEvent::CastResult {
            spell_id: HEAL,
            success: true,
            reason: None,
            arg: None,
        }],
    ]);
    let seen = c.seen(&[
        "SPELLCAST_START",
        "UNIT_MANA",
        "SPELLCAST_STOP",
        "CURRENT_SPELL_CAST_CHANGED",
    ]);
    assert_eq!(
        seen,
        [
            "SPELLCAST_START:1:nil",
            "UNIT_MANA:1:nil",
            "CURRENT_SPELL_CAST_CHANGED:nil:nil",
            "SPELLCAST_STOP:nil:nil",
        ]
    );
}

/// With the frame's packets fired, the getters answer the frame's last state, live and off the
/// snapshot alike: Heal in flight, then popped.
#[test]
fn the_cast_getters_answer_the_frames_last_state_once_its_packets_fired() {
    let mut c = client();
    let chunk = "return IsCurrentAction(1), SpellIsTargeting()";
    cast_heal(&mut c);
    for live in [false, true] {
        assert_eq!(
            c.eval::<(Option<i64>, Option<i64>)>(live, chunk),
            (Some(1), None)
        );
    }
    c.frame(vec![vec![SessionEvent::CastResult {
        spell_id: HEAL,
        success: true,
        reason: None,
        arg: None,
    }]]);
    for live in [false, true] {
        assert_eq!(
            c.eval::<(Option<i64>, Option<i64>)>(live, chunk),
            (None, None)
        );
    }
}

/// A listener's call that moves no cast state leaves it its packet's view: the `SPELLCAST_START`
/// listener's `ClearTarget()` applies inside it, and its next line still reads Heal in flight.
#[test]
fn a_listeners_call_that_moves_no_cast_state_leaves_its_packets_view() {
    let mut c = client();
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("SPELLCAST_START")
        g:SetScript("OnEvent", function()
            ClearTarget()
            table.insert(Seen, "CALLED:" .. tostring(IsCurrentAction(1)))
        end)"#,
    );
    cast_heal(&mut c);
    c.frame(vec![
        vec![SessionEvent::SpellStart {
            caster: ME,
            spell_id: HEAL,
            cast_flags: 0,
            cast_time_ms: 2500,
            target: None,
            ammo_display_id: None,
        }],
        vec![SessionEvent::CastResult {
            spell_id: HEAL,
            success: true,
            reason: None,
            arg: None,
        }],
    ]);
    assert_eq!(c.seen(&["CALLED"]), ["CALLED:1"]);
}

/// Another lane's listener ahead of the stop reads the slot as the frame began: a cooldown
/// packet's `SPELL_UPDATE_COOLDOWN` (`0x6e95b0`), first in the frame, reads Heal in flight, and the
/// stop's listeners after it read it popped.
#[test]
fn a_cooldown_listener_ahead_of_the_stop_reads_the_cast_in_flight() {
    let mut c = client();
    cast_heal(&mut c);
    c.frame(vec![
        vec![SessionEvent::SpellCooldowns {
            caster: ME,
            cooldowns: vec![(HEAL, 5_000)],
        }],
        vec![SessionEvent::CastResult {
            spell_id: HEAL,
            success: true,
            reason: None,
            arg: None,
        }],
    ]);
    assert_eq!(
        c.seen(&["SPELL_UPDATE_COOLDOWN", "SPELLCAST_STOP"]),
        ["SPELL_UPDATE_COOLDOWN:1:nil", "SPELLCAST_STOP:nil:nil"]
    );
}

/// A listener's cast holds through the packets after its own: the stop's listener casts Heal
/// again, and the next packet, which moves the form byte, reads it in flight, as the slot the
/// call wrote is the live one.
#[test]
fn a_listeners_cast_keeps_its_slot_through_the_next_packets_view() {
    use benilla_protocol::field::FIELD_UNIT_BYTES_1;
    let mut c = client();
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("SPELLCAST_STOP")
        g:SetScript("OnEvent", function()
            if not Recast then Recast = true UseAction(1) end
        end)"#,
    );
    cast_heal(&mut c);
    c.frame(vec![
        vec![SessionEvent::CastResult {
            spell_id: HEAL,
            success: true,
            reason: None,
            arg: None,
        }],
        vec![values(ME, &[(MANA, 90), (FIELD_UNIT_BYTES_1, 1 << 16)])],
    ]);
    assert_eq!(c.casts(), [HEAL], "the listener's cast went out");
    assert_eq!(
        c.seen(&["SPELLCAST_STOP", "UNIT_MANA"]),
        ["SPELLCAST_STOP:nil:nil", "UNIT_MANA:1:nil"]
    );
}

/// Our cast's failed `SMSG_CAST_RESULT` on the lane: a power packet ahead of it reads Heal in
/// flight, and the refusal (`0x6e1a00`) pops it (`0x6e2218`) before its own listeners,
/// `CURRENT_SPELL_CAST_CHANGED` then `SPELLCAST_FAILED` (`0x6e2238`), which read it popped.
#[test]
fn a_failed_cast_result_pops_the_slot_at_its_place_in_the_drain() {
    let mut c = client();
    cast_heal(&mut c);
    c.frame(vec![
        vec![values(ME, &[(MANA, 90)])],
        vec![SessionEvent::CastResult {
            spell_id: HEAL,
            success: false,
            reason: Some(0x0b),
            arg: None,
        }],
    ]);
    assert_eq!(
        c.seen(&[
            "UNIT_MANA",
            "SPELLCAST_STOP",
            "SPELLCAST_FAILED",
            "CURRENT_SPELL_CAST_CHANGED",
        ]),
        [
            "UNIT_MANA:1:nil",
            "CURRENT_SPELL_CAST_CHANGED:nil:nil",
            "SPELLCAST_FAILED:nil:nil",
        ]
    );
}

/// A failed `SMSG_CAST_RESULT`'s report clears the GCD the send armed and fires its pair
/// (`0x6e1d89` → `0x6e1630`, the pair at `0x6e167d`) before it pops the slot (`0x6e2218`): the
/// pair's `SPELL_UPDATE_COOLDOWN` reads Heal still in flight, and the pop's
/// `CURRENT_SPELL_CAST_CHANGED` and `SPELLCAST_FAILED` after it read it popped.
#[test]
fn a_failed_results_gcd_clear_reads_the_cast_still_in_flight() {
    let mut c = client();
    c.app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            HEAL,
            SpellDisplay {
                id: HEAL,
                name: "Heal".into(),
                icon: Some("Interface\\Icons\\Spell_Holy_Heal".into()),
                start_recovery_category: 133,
                start_recovery_ms: 1500,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    c.frame(vec![]);
    c.seen(&[]);
    cast_heal(&mut c);
    c.frame(vec![vec![SessionEvent::CastResult {
        spell_id: HEAL,
        success: false,
        reason: Some(0x0b),
        arg: None,
    }]]);
    assert_eq!(
        c.seen(&[
            "SPELL_UPDATE_COOLDOWN",
            "CURRENT_SPELL_CAST_CHANGED",
            "SPELLCAST_FAILED",
        ]),
        [
            "SPELL_UPDATE_COOLDOWN:1:nil",
            "CURRENT_SPELL_CAST_CHANGED:nil:nil",
            "SPELLCAST_FAILED:nil:nil",
        ]
    );
}

/// `CancelCast 0x6e4940` drops the targeting word (`0x6e4955`) where it pops the slot
/// (`0x6e49d6`), at the failed report's pop (`0x6e2218`), after the GCD clear's pair
/// (`0x6e167d`): with the cursor up for Heal, the pair's `SPELL_UPDATE_COOLDOWN` reads the slot and
/// the word both still up.
#[test]
fn a_failed_results_gcd_clear_reads_the_targeting_word_still_up() {
    let mut c = client();
    let heal = SpellDisplay {
        id: HEAL,
        name: "Heal".into(),
        start_recovery_category: 133,
        start_recovery_ms: 1500,
        ..Default::default()
    };
    let world = c.app.world_mut();
    world.resource_mut::<crate::spell::Cooldowns>().start_gcd(
        HEAL,
        &heal,
        std::time::Instant::now(),
        &crate::spell::SpellModifiers::default(),
    );
    world.resource_mut::<crate::spell::SpellTargeting>().enter(
        HEAL,
        crate::spell::CastCommit::Spell,
        0x0002,
    );
    c.frame(vec![]);
    c.seen(&[]);
    c.frame(vec![vec![SessionEvent::CastResult {
        spell_id: HEAL,
        success: false,
        reason: Some(0x0b),
        arg: None,
    }]]);
    let seen = c.seen(&["SPELL_UPDATE_COOLDOWN", "CURRENT_SPELL_CAST_CHANGED"]);
    assert_eq!(
        seen.first().map(String::as_str),
        Some("SPELL_UPDATE_COOLDOWN:1:1"),
        "{seen:?}"
    );
    assert_eq!(
        seen.last().map(String::as_str),
        Some("CURRENT_SPELL_CAST_CHANGED:nil:nil"),
        "{seen:?}"
    );
}

/// A reagent the refusal below names, whose record the item cache lacks until its answer.
const SHARD: u32 = 6265;

/// The reagent line's strings, and a spy on the lines, each `UI_ERROR_MESSAGE` as `LINE:<text>`.
fn spy_lines(c: &mut Client) {
    c.eval::<()>(
        false,
        r#"SPELL_FAILED_REAGENTS = "Missing reagent: %s"
        ERR_SPELL_FAILED_REAGENTS = "%s"
        local g = CreateFrame("Frame")
        g:RegisterEvent("UI_ERROR_MESSAGE")
        g:SetScript("OnEvent", function() table.insert(Seen, "LINE:" .. arg1) end)"#,
    );
}

/// Our cast of `spell_id` refused for a missing reagent, its item word `arg`.
fn refuse_for_a_reagent(c: &mut Client, spell_id: u32, arg: Option<u32>) {
    c.frame(vec![vec![SessionEvent::CastResult {
        spell_id,
        success: false,
        reason: Some(0x5c),
        arg,
    }]]);
}

/// [`client`] with [`spy_lines`]; then our cast refused for [`SHARD`], which the cache lacks: the
/// reporter's lookup asks for it and leaves past the line and its log line (`0x6e1ea4`,
/// `0x6e1eab`).
fn refused_for_a_reagent() -> Client {
    let mut c = client();
    spy_lines(&mut c);
    refuse_for_a_reagent(&mut c, HEAL, Some(SHARD));
    assert!(
        c.seen(&["LINE"]).is_empty(),
        "no line while the record is asked"
    );
    c
}

/// The answer for [`SHARD`]: its record, or the server's refusal.
fn shard_answer(name: Option<&str>) -> SessionEvent {
    SessionEvent::ItemTemplate {
        entry: SHARD,
        info: name.map(|n| Box::new(crate::items::test_template(n))),
    }
}

/// The record's answer runs the refusal's callback (`ShowItemCastFailure 0x6e29b0`) inside its
/// packet (`ItemCacheReceive 0x55bdb0`): the line fires between the power packets around it.
#[test]
fn a_reagent_line_shows_at_its_records_answer() {
    let mut c = refused_for_a_reagent();
    c.frame(vec![
        vec![values(ME, &[(MANA, 90)])],
        vec![shard_answer(Some("Soul Shard"))],
        vec![values(ME, &[(MANA, 80)])],
    ]);
    assert_eq!(
        c.seen(&["UNIT_MANA", "LINE"]),
        [
            "UNIT_MANA:nil:nil",
            "LINE:Missing reagent: Soul Shard",
            "UNIT_MANA:nil:nil",
        ]
    );
    c.frame(vec![]);
    assert!(c.seen(&["LINE"]).is_empty(), "the callback ran once");
}

/// A refusal runs the callback too, whose lookup finds no record: `UNKNOWN` (`0x6e2a49`).
#[test]
fn a_refused_reagent_record_shows_unknown_at_its_answer() {
    let mut c = refused_for_a_reagent();
    c.frame(vec![
        vec![values(ME, &[(MANA, 90)])],
        vec![shard_answer(None)],
        vec![values(ME, &[(MANA, 80)])],
    ]);
    assert_eq!(
        c.seen(&["UNIT_MANA", "LINE"]),
        [
            "UNIT_MANA:nil:nil",
            "LINE:Missing reagent: UNKNOWN",
            "UNIT_MANA:nil:nil",
        ]
    );
}

/// With no frame after them, the getters read through the host the cast state each packet left,
/// inside the call (`0x4e53a0`, `0x6e6e80`): Heal in flight after its start, popped after its
/// result, where the VM's copy, which no feed pushes, still holds it in flight; and
/// `SpellStopCasting` finds nothing to stop.
#[test]
fn the_cast_getters_read_each_packets_state_at_the_call() {
    let mut c = client();
    cast_heal(&mut c);
    let chunk = "return IsCurrentAction(1), SpellIsTargeting()";
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::SpellStart {
            caster: ME,
            spell_id: HEAL,
            cast_flags: 0,
            cast_time_ms: 2500,
            target: None,
            ammo_display_id: None,
        }],
    );
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>)>(true, chunk),
        (Some(1), None)
    );
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::CastResult {
            spell_id: HEAL,
            success: true,
            reason: None,
            arg: None,
        }],
    );
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>)>(true, chunk),
        (None, None)
    );
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>)>(false, chunk),
        (Some(1), None),
        "the VM's copy"
    );
    c.press("Stopped = SpellStopCasting()");
    assert_eq!(c.eval::<Option<i64>>(false, "return Stopped"), None);
}

/// The targeting word and the auto-repeat key read at the call too (`0x6e6cd0`, `0x6e9fd0`), each
/// written with no push after it: the cursor up for Heal and Heal repeating. `SpellStopTargeting`
/// tests the word at the call and ends it inside it (`0x6e6e30` → `0x6e4900`).
#[test]
fn the_targeting_word_and_the_auto_repeat_key_read_at_the_call() {
    let mut c = client();
    let world = c.app.world_mut();
    world.resource_mut::<crate::spell::SpellTargeting>().enter(
        HEAL,
        crate::spell::CastCommit::Spell,
        0x0002,
    );
    world.resource_mut::<crate::spell::AutoRepeatActive>().0 = Some(HEAL);
    let chunk = "return SpellIsTargeting(), IsAutoRepeatAction(1)";
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>)>(true, chunk),
        (Some(1), Some(1))
    );
    assert_eq!(
        c.eval::<(Option<i64>, Option<i64>)>(false, chunk),
        (None, None),
        "the VM's copy"
    );
    c.press("Stopped = SpellStopTargeting()");
    assert_eq!(c.eval::<Option<i64>>(false, "return Stopped"), Some(1));
    assert!(!c
        .app
        .world()
        .resource::<crate::spell::SpellTargeting>()
        .active());
}

/// A refusal of ours with no item word reads -1 (`0x6e736e`), whose ask no answer ever matches
/// (`0x55bdf7`, `0x55be5d`), and one with a word of 0 asks nothing (`0x55ba42`): neither shows a
/// line, now or at an answer, though Heal's reagent is cached and missing from our bags.
#[test]
fn a_reagent_refusal_with_no_item_word_shows_no_line() {
    let mut c = client();
    let mut reagents = [(0, 0); 8];
    reagents[0] = (SHARD, 1);
    c.app.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(HashMap::from([(
            HEAL,
            SpellDisplay {
                id: HEAL,
                name: "Heal".into(),
                reagents,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    c.app
        .world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(SHARD, Some(crate::items::test_template("Soul Shard")));
    spy_lines(&mut c);
    for arg in [None, Some(0)] {
        refuse_for_a_reagent(&mut c, HEAL, arg);
        c.frame(vec![vec![shard_answer(Some("Soul Shard"))]]);
        assert!(c.seen(&["LINE"]).is_empty(), "{arg:?}");
    }
    assert!(
        c.app
            .world_mut()
            .resource_mut::<crate::items::Items>()
            .take_callbacks(0)
            .is_empty(),
        "a word of 0 keeps no callback"
    );
}

/// The reagent arm reads the item word alone, never the spell record (`0x6e1e7f`): a refusal of
/// a spell the catalog lacks shows its line at the record's answer under `0xd2`, which the
/// callback picks off the reason (`0x6e29fe`), and with the record cached under the default id
/// `0x2c`, as the reporter skips the record's chain (`0x6e1a7c`).
#[test]
fn a_reagent_line_shows_for_a_spell_with_no_record() {
    const UNLISTED: u32 = 99_999;
    let mut c = client();
    spy_lines(&mut c);
    c.eval::<()>(
        false,
        r#"ERR_SPELL_FAILED_S = "S:%s" ERR_SPELL_FAILED_REAGENTS = "R:%s""#,
    );
    refuse_for_a_reagent(&mut c, UNLISTED, Some(SHARD));
    assert!(c.seen(&["LINE"]).is_empty(), "the record is asked");
    c.frame(vec![vec![shard_answer(Some("Soul Shard"))]]);
    assert_eq!(c.seen(&["LINE"]), ["LINE:R:Missing reagent: Soul Shard"]);
    refuse_for_a_reagent(&mut c, UNLISTED, Some(SHARD));
    assert_eq!(c.seen(&["LINE"]), ["LINE:S:Missing reagent: Soul Shard"]);
}
