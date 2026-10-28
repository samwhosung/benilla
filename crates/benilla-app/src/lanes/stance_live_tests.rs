//! The stance bar and the mirror timers as a listener reads them, and their events at their
//! packets. The form getters read the form list and the active player at the call: the count
//! `0xb71104` (`0x4b4590`), an entry's form byte, aura slots and usability walk (`0x4b45c0` →
//! `0x6e3d60`), and its spell's cooldown off the player's list (`0x4b49a0` → `0x6e2ea0`). The
//! mirror-timer handler `0x5e7990` signals inside itself, a start's tutorial after its signal
//! (`0x5e7a9a`, `0x5e7ab0`, `0x5e7acd`).

use std::collections::HashMap;
use std::time::Instant;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::field::{
    FIELD_UNIT_AURA, FIELD_UNIT_BYTES_1, FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH,
};
use benilla_protocol::messages::MirrorTimerStart;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// Two form spells on the stance bar by their `MOD_SHAPESHIFT` form (`0x4b2810`): Bear, form 5,
/// and a form whose spell carries the fear immunity (`AttributesEx` bit 15, aura 77 over
/// mechanic 5), form 6.
const BEAR: u32 = 90_021;
const WARDED: u32 = 90_023;
/// A third, Cat, form 1, ordered after both by its `StanceBarOrder`.
const CAT: u32 = 90_022;
/// A fear aura, mechanic 5.
const FEAR: u32 = 90_031;
/// `UNIT_FIELD_FLAGS` bit 23, fleeing.
const FLEEING: u32 = 0x0080_0000;

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

    /// A spell known, as a learn's handler leaves the known spells and the form list
    /// (`0x4b25b0`).
    fn know(&mut self, spell_id: u32) {
        let world = self.app.world_mut();
        world.resource_scope(|world, mut actions: Mut<crate::ui_action::PlayerActions>| {
            actions.spells.insert(spell_id);
            let catalog = &world.resource::<crate::ui_action::Spells>().catalog;
            crate::ui_shapeshift::form_learned(&mut actions.forms, spell_id, None, catalog);
        });
    }
}

fn spells() -> crate::ui_action::Spells {
    let form = |id: u32, name: &str, form: u32, order: i32| {
        (
            id,
            SpellDisplay {
                id,
                name: name.into(),
                icon: Some(format!("Interface\\Icons\\Spell_{id}")),
                shapeshift_form: Some(form),
                stance_bar_order: order,
                ..Default::default()
            },
        )
    };
    let (_, mut warded) = form(WARDED, "Warded Form", 6, 1);
    warded.attributes_ex = 0x8000;
    warded.effects[0] = 6;
    warded.effect_apply_aura[0] = 77;
    warded.effect_misc_value[0] = 5;
    let mut fear = SpellDisplay {
        id: FEAR,
        mechanic: 5,
        ..Default::default()
    };
    fear.effects[0] = 6;
    fear.effect_apply_aura[0] = 7;
    crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(
            [
                form(BEAR, "Bear Form", 5, 0),
                (WARDED, warded),
                form(CAT, "Cat Form", 1, 2),
                (FEAR, fear),
            ]
            .into_iter()
            .collect::<HashMap<_, _>>(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// The client in the world knowing Bear and the warded form, with the tutorial bank cleared and
/// a spy whose line for each family event carries its name and first argument, emptied.
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
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"MIRROR_TIMER_START", "MIRROR_TIMER_PAUSE", "MIRROR_TIMER_STOP",
                    "SPELL_UPDATE_COOLDOWN", "TUTORIAL_TRIGGER"}) do
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
        actions.spells.extend([BEAR, WARDED]);
        actions.initial.extend([BEAR, WARDED]);
    }
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[(FIELD_UNIT_HEALTH, 100), (FIELD_UNIT_MAXHEALTH, 100)],
    )]]);
    c.frame(vec![]);
    c.app
        .world()
        .resource::<crate::tutorial::Tutorials>()
        .lock()
        .apply_flags(&[0u8; 8]);
    c.seen();
    c
}

/// Every form entry's name and active and castable flags, joined.
const BAR: &str = "local n = {} for i = 1, GetNumShapeshiftForms() do \
                   local _, name, active, castable = GetShapeshiftFormInfo(i) \
                   n[i] = name .. ':' .. tostring(active) .. ':' .. tostring(castable) end \
                   return table.concat(n, ',')";

/// Each form getter follows the world at the call, with no push between: a learn's known spells
/// move the list, the form byte the active entry, lost control the castable flags, and the
/// player's list the cooldown; with no host, the VM's copy answers.
#[test]
fn the_form_getters_read_the_world_at_the_call() {
    let mut c = client();
    assert_eq!(
        c.eval::<String>(true, BAR),
        "Bear Form:nil:1,Warded Form:nil:1"
    );
    c.know(CAT);
    assert_eq!(
        c.eval::<String>(true, BAR),
        "Bear Form:nil:1,Warded Form:nil:1,Cat Form:nil:1",
        "the learn's list, in bar order"
    );
    assert_eq!(
        c.eval::<i64>(false, "return GetNumShapeshiftForms()"),
        0,
        "with no host the copy answers"
    );
    // The stance hover reads the list at the call too (`0x532a21`): the learned form's name,
    // with no record to build from.
    let hover = "local tt = TT or CreateFrame('GameTooltip', 'TT', nil, 'TestTooltipTemplate') \
                 tt:SetOwner(CreateFrame('Frame'), 'ANCHOR_NONE') tt:SetShapeshift(3) \
                 return tt:NumLines(), TTTextLeft1:GetText()";
    assert_eq!(
        c.eval::<(i64, Option<String>)>(true, hover),
        (1, Some("Cat Form".into()))
    );

    // Bear's form byte (`UNIT_FIELD_BYTES_1` byte 2): active, so castable whatever the walk says.
    c.write_player(&[(FIELD_UNIT_BYTES_1, 5 << 16)]);
    assert_eq!(
        c.eval::<String>(true, BAR),
        "Bear Form:1:1,Warded Form:nil:1,Cat Form:nil:1"
    );
    // Feared, without control: the walk's leg 2 greys every form but the immune one and the
    // active one (`0x6e3dc3`).
    c.write_player(&[
        (FIELD_UNIT_FLAGS, (1 << 3) | FLEEING),
        (FIELD_UNIT_AURA, FEAR),
    ]);
    c.app
        .world_mut()
        .resource_mut::<crate::player::OwnControlLost>()
        .0 = true;
    assert_eq!(
        c.eval::<String>(true, BAR),
        "Bear Form:1:1,Warded Form:nil:1,Cat Form:nil:nil"
    );

    // The player's list as the cooldown packet's handler leaves it: a held cooldown answers the
    // poll's own now with 0.001 and 0 (`0x6e1517`).
    let held = SpellDisplay {
        shapeshift_form: Some(1),
        attributes: 0x0200_0000,
        ..Default::default()
    };
    c.app
        .world_mut()
        .resource_mut::<crate::spell::Cooldowns>()
        .start_spell(CAT, &held, 0, Instant::now(), None);
    let (start, duration, enable, now) = c.eval::<(f64, f64, i64, f64)>(
        true,
        "local s, d, e = GetShapeshiftFormCooldown(3) return s, d, e, GetTime()",
    );
    assert_eq!((start, duration, enable), (now, 0.001, 0));
    assert_eq!(
        c.eval::<(f64, f64, i64)>(true, "return GetShapeshiftFormCooldown(1)"),
        (0.0, 0.0, 1),
        "Bear's own list is cold"
    );
    assert_eq!(
        c.eval::<(f64, f64, i64)>(true, "return GetShapeshiftFormCooldown(4)"),
        (0.0, 0.0, 1),
        "past the list"
    );
}

/// Four nils with no active player held, whatever the list holds (`0x4b4639`): the host declines
/// the entry, and nothing in game pushes the copy that answers it then.
#[test]
fn an_entry_reads_four_nils_with_no_player() {
    let mut c = client();
    let me = c.app.world().resource::<crate::net::GuidIndex>().0[&ME];
    c.app
        .world_mut()
        .entity_mut(me)
        .remove::<crate::net::SelfPlayer>();
    assert_eq!(
        c.eval::<(i64, bool)>(
            true,
            "return GetNumShapeshiftForms(), GetShapeshiftFormInfo(1) == nil"
        ),
        (2, true)
    );
}

/// `SMSG_SPELL_COOLDOWN` for the player on Bear, a packet of the cooldown lane's.
fn cooldown() -> Vec<SessionEvent> {
    vec![SessionEvent::SpellCooldowns {
        caster: ME,
        cooldowns: vec![(BEAR, 8_000)],
    }]
}

/// A breath timer's start, a fatigue start, a pause and a stop, each between two cooldown
/// packets: each signals inside its handler, a start's tutorial after its signal, so they
/// interleave with the cooldown lane's as the packets came, not ahead of them from a frame-end
/// feed. A type with no bar raises nothing.
#[test]
fn the_mirror_timers_fire_at_their_packets_places() {
    let mut c = client();
    let start = |kind| {
        vec![SessionEvent::MirrorTimerStart(MirrorTimerStart {
            kind,
            remaining_ms: 60_000,
            duration_ms: 60_000,
            scale: -1,
            paused: false,
            spell_id: 0,
        })]
    };
    c.frame(vec![
        cooldown(),
        start(1),
        cooldown(),
        start(0),
        cooldown(),
        vec![SessionEvent::MirrorTimerPause {
            kind: 1,
            paused: true,
        }],
        vec![SessionEvent::MirrorTimerStop { kind: 3 }],
        cooldown(),
        vec![SessionEvent::MirrorTimerStop { kind: 1 }],
        cooldown(),
    ]);
    assert_eq!(
        c.seen(),
        [
            "SPELL_UPDATE_COOLDOWN:nil",
            "MIRROR_TIMER_START:BREATH",
            // Tutorial 0x1c, published as its id plus one.
            "TUTORIAL_TRIGGER:29",
            "SPELL_UPDATE_COOLDOWN:nil",
            "MIRROR_TIMER_START:EXHAUSTION",
            "TUTORIAL_TRIGGER:27",
            "SPELL_UPDATE_COOLDOWN:nil",
            "MIRROR_TIMER_PAUSE:BREATH",
            "SPELL_UPDATE_COOLDOWN:nil",
            "MIRROR_TIMER_STOP:BREATH",
            "SPELL_UPDATE_COOLDOWN:nil",
        ]
    );
}

/// The capture's demo seat with no player held: the list the capture seeds
/// ([`crate::ui_script::DEMO_STANCES`]), in bar order, each entry off the copy the demo feed
/// seeds, by its spell.
#[test]
fn the_demo_seat_shows_its_forms() {
    let mut c = client();
    let me = c.app.world().resource::<crate::net::GuidIndex>().0[&ME];
    let world = c.app.world_mut();
    world.entity_mut(me).remove::<crate::net::SelfPlayer>();
    let stance = |id: u32, form: u32, order: i32| {
        (
            id,
            SpellDisplay {
                id,
                shapeshift_form: Some(form),
                stance_bar_order: order,
                ..Default::default()
            },
        )
    };
    let [battle, defensive, berserker] = crate::ui_script::DEMO_STANCES;
    world.insert_resource(crate::ui_action::Spells {
        catalog: SpellCatalog::from_displays(
            [
                stance(berserker, 19, 2),
                stance(battle, 17, 0),
                stance(defensive, 18, 1),
            ]
            .into_iter()
            .collect::<HashMap<_, _>>(),
        ),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    let mut actions = world.resource_mut::<crate::ui_action::PlayerActions>();
    actions.spells.clear();
    actions.spells.extend(crate::ui_script::DEMO_STANCES);
    actions.forms.clear();
    actions.forms.extend(crate::ui_script::DEMO_STANCES);
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(crate::ui_script::demo_unit_feed)
        .expect("the demo feed runs");
    assert_eq!(
        c.eval::<String>(true, BAR),
        "Battle Stance:1:1,Defensive Stance:nil:nil,Berserker Stance:nil:1"
    );
}

/// The active player's destroy runs its release, which stops all three mirror timers, running or
/// not (`0x4674a0` → `0x5e9aa0`, `0x5e9b55` → `0x5e9b90`).
#[test]
fn the_players_release_stops_every_mirror_timer() {
    let mut c = client();
    c.frame(vec![
        vec![SessionEvent::MirrorTimerStart(MirrorTimerStart {
            kind: 1,
            remaining_ms: 60_000,
            duration_ms: 60_000,
            scale: -1,
            paused: false,
            spell_id: 0,
        })],
        cooldown(),
        vec![SessionEvent::ObjectDestroyed(ME)],
    ]);
    assert_eq!(
        c.seen(),
        [
            "MIRROR_TIMER_START:BREATH",
            "TUTORIAL_TRIGGER:29",
            "SPELL_UPDATE_COOLDOWN:nil",
            "MIRROR_TIMER_STOP:EXHAUSTION",
            "MIRROR_TIMER_STOP:BREATH",
            "MIRROR_TIMER_STOP:FEIGNDEATH",
        ]
    );
}
