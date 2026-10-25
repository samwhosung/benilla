//! The action bar's getters as a listener reads them, packet by packet. `IsUsableAction`
//! answers the bar's usable cache (`0x4e73d0` → `0x4e5230`, `0xbc6b60`/`0xbc67a0`), which the
//! field watchers refresh inside their packet (`0x604876` → `0x4b31c0` → `0x4e5c00`), so its
//! `ACTIONBAR_UPDATE_USABLE` and the `UNIT_MANA` after it read that packet's pair. Two getters
//! read the active player's object inside the call: `IsCurrentAction`'s form arm the form byte
//! (`0x4e558b`, `[unit+0x110]+0x212`) and `IsEquippedAction` the worn slots (`0x4e5310` →
//! `0x622270` at mask 1). The bonus bar's offset is stored where the form watcher runs
//! (`0x5de01b` → `0x4e4fc0`), off that packet's form. The live answer and the frame's snapshot
//! agree on every getter wherever the snapshot is current.

use std::collections::HashMap;

use benilla_formats::{ShapeshiftForm, SpellCatalog, SpellDisplay};
use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
use benilla_protocol::messages::ActionButton;
use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;

const ME: u64 = 0x7;
/// A sword the player carries, worn or not as the test puts it.
const SWORD_GUID: u64 = 0x4000_0000_0000_00F0;
const SWORD: u32 = 2025;
/// A potion in backpack slot 1, ours (`ITEM_FIELD_OWNER`), the last of its stack.
const POTION_GUID: u64 = 0x4000_0000_0000_00F1;
const POTION: u32 = 118;

/// The spells on the bar: a Frostbolt at 50 mana, the druid's Cat and Bear forms, and Attack.
const FROSTBOLT: u32 = 116;
const CAT: u32 = 768;
const BEAR: u32 = 5487;
const ATTACK: u32 = 6603;
/// The form bytes of Cat and Bear, and the bonus bars their rows page to.
const CAT_FORM: u32 = 1;
const BEAR_FORM: u32 = 5;

/// `OBJECT_FIELD_TYPE` and a player's type mask, `UNIT_FIELD_FLAGS` and its player-controlled bit.
const OBJECT_TYPE: u16 = 2;
const TYPE_PLAYER: u32 = 0x19;
const FLAGS: u16 = 46;
const PLAYER_CONTROLLED: u32 = 0x8;
/// `UNIT_FIELD_POWER1` and `UNIT_FIELD_MAXPOWER1`, mana for power type 0.
const MANA: u16 = 23;
const MAX_MANA: u16 = 29;
/// `UNIT_FIELD_BYTES_1`, whose byte 2 is the form.
const BYTES_1: u16 = 138;
/// `PLAYER_FIELD_INV_SLOT_HEAD` + 2 × 15, the main hand's guid, and the head's.
const MAIN_HAND: u16 = 486 + 2 * 15;
const HEAD: u16 = 486;
/// `PLAYER_FIELD_PACK_SLOT_1`, backpack slot 1's guid.
const PACK_SLOT_1: u16 = 532;
/// `ITEM_FIELD_STACK_COUNT`.
const STACK_COUNT: u16 = 14;

/// The Lua action ids the bar holds, and one past them that holds nothing.
const SLOTS: [u32; 7] = [1, 2, 3, 4, 5, 6, 7];

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

    /// What the spy heard since the last call, its events among `events` alone.
    fn seen(&mut self, events: &[&str]) -> Vec<String> {
        let seen: Vec<String> = self.eval(false, "local s = Seen Seen = {} return s");
        seen.into_iter()
            .filter(|l| events.iter().any(|e| l.split(':').next() == Some(e)))
            .collect()
    }

    /// Every census getter's answer for every slot, one line each, and the bonus bar's offset.
    fn answers(&mut self, live: bool) -> Vec<String> {
        let slots = SLOTS.map(|s| s.to_string()).join(", ");
        let chunk = format!(
            "local out = {{}}
            local function note(what, ...)
                for i = 1, table.getn(arg) do arg[i] = tostring(arg[i]) end
                table.insert(out, what .. ' ' .. table.concat(arg, ','))
            end
            for _, s in ipairs({{ {slots} }}) do
                note(s .. ' HasAction', HasAction(s))
                note(s .. ' GetActionTexture', GetActionTexture(s))
                note(s .. ' GetActionCount', GetActionCount(s))
                note(s .. ' GetActionCooldown', GetActionCooldown(s))
                note(s .. ' GetActionText', GetActionText(s))
                note(s .. ' GetActionAutocast', GetActionAutocast(s))
                note(s .. ' IsUsableAction', IsUsableAction(s))
                note(s .. ' IsCurrentAction', IsCurrentAction(s))
                note(s .. ' IsAutoRepeatAction', IsAutoRepeatAction(s))
                note(s .. ' IsAttackAction', IsAttackAction(s))
                note(s .. ' IsConsumableAction', IsConsumableAction(s))
                note(s .. ' IsEquippedAction', IsEquippedAction(s))
                note(s .. ' IsActionInRange', IsActionInRange(s))
                note(s .. ' ActionHasRange', ActionHasRange(s))
            end
            note('GetBonusBarOffset', GetBonusBarOffset())
            return out"
        );
        self.eval(live, &chunk)
    }

    /// The player's store, written in place with no packet or frame, as a packet's handler leaves
    /// it before the feeds have pushed.
    fn write_store(&mut self, pairs: &[(u16, u32)]) {
        let world = self.app.world_mut();
        let entity = world.resource::<crate::net::GuidIndex>().0[&ME];
        let mut store = world
            .get_mut::<crate::net::ObjectStore>(entity)
            .expect("a store");
        store.0.merge(ObjectFields::from_pairs(pairs));
    }
}

/// The spells' rows and the two forms' rows.
fn spells() -> crate::ui_action::Spells {
    let row = |id: u32, f: fn(&mut SpellDisplay)| {
        let mut d = SpellDisplay {
            id,
            name: format!("Spell {id}"),
            icon: Some(format!("Interface\\Icons\\Spell_{id}")),
            ..Default::default()
        };
        f(&mut d);
        (id, d)
    };
    let catalog = SpellCatalog::from_displays(HashMap::from([
        row(FROSTBOLT, |d| d.mana_cost = 50),
        row(CAT, |d| d.shapeshift_form = Some(CAT_FORM)),
        row(BEAR, |d| d.shapeshift_form = Some(BEAR_FORM)),
        // `SPELL_EFFECT_ATTACK`, which makes the slot the Attack slot (`0x4e59de`).
        row(ATTACK, |d| d.effects[0] = 78),
    ]));
    let form = |bonus_bar: u32| ShapeshiftForm {
        bonus_bar,
        ..Default::default()
    };
    crate::ui_action::Spells {
        catalog,
        forms: HashMap::from([(CAT_FORM, form(1)), (BEAR_FORM, form(3))]),
        ..crate::ui_action::Spells::empty_for_tests()
    }
}

/// The client in the world as [`ME`] at 100 mana, in no form, the sword carried but not worn;
/// the bar holding Frostbolt, Cat Form, Bear Form, the sword and Attack in slots 1-5; and a spy
/// whose line for each event carries what its listener read of the bar: Frostbolt's usable pair
/// and the sword's equipped flag, the two forms' checked state, the bonus bar's offset.
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
    script
        .run(
            r#"
            Seen = {}
            local f = CreateFrame("Frame")
            for _, e in ipairs({"ACTIONBAR_UPDATE_USABLE", "SPELL_UPDATE_USABLE", "UNIT_MANA",
                    "ACTIONBAR_UPDATE_STATE", "UPDATE_BONUS_ACTIONBAR"}) do
                f:RegisterEvent(e)
            end
            local function s(v) return tostring(v) end
            f:SetScript("OnEvent", function()
                if event == "UNIT_MANA" and arg1 ~= "player" then return end
                local usable, oom = IsUsableAction(1)
                table.insert(Seen, event .. ":" .. s(usable) .. ":" .. s(oom) .. ":"
                    .. s(IsEquippedAction(4)) .. ":" .. s(IsCurrentAction(2)) .. ":"
                    .. s(IsCurrentAction(3)) .. ":" .. s(GetBonusBarOffset()))
            end)
            local slots = CreateFrame("Frame")
            slots:RegisterEvent("ACTIONBAR_SLOT_CHANGED")
            slots:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. arg1 .. ":" .. s(IsUsableAction(arg1)) .. ":"
                    .. GetActionCount(arg1))
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
        .extend([FROSTBOLT, CAT, BEAR, ATTACK]);
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(SWORD, Some(crate::items::test_template("Sword")));
    world
        .resource_mut::<crate::items::Items>()
        .insert_template(POTION, Some(crate::items::test_template("Potion")));
    {
        let mut slots = world.resource_mut::<crate::ui_action::ActionSlots>();
        for (slot, action, kind) in [
            (0, FROSTBOLT, benilla_protocol::messages::ACTION_KIND_SPELL),
            (1, CAT, benilla_protocol::messages::ACTION_KIND_SPELL),
            (2, BEAR, benilla_protocol::messages::ACTION_KIND_SPELL),
            (3, SWORD, benilla_protocol::messages::ACTION_KIND_ITEM),
            (4, ATTACK, benilla_protocol::messages::ACTION_KIND_SPELL),
            (6, POTION, benilla_protocol::messages::ACTION_KIND_ITEM),
        ] {
            slots
                .buttons
                .insert(slot, ActionButton { slot, action, kind });
        }
        slots.dirty = true;
    }
    // The sword: entry, stack 1, durability 30 of 40, so the worn walk counts it.
    crate::items::test_spawn_item(
        world,
        SWORD_GUID,
        ObjectFields::from_pairs(&[(3, SWORD), (14, 1), (46, 30), (47, 40)]),
        false,
    );
    // The potion: an item (`OBJECT_FIELD_TYPE` 0x3) of ours, two in the stack.
    crate::items::test_spawn_item(
        world,
        POTION_GUID,
        ObjectFields::from_pairs(&[
            (OBJECT_TYPE, 0x3),
            (3, POTION),
            (6, ME as u32),
            (7, (ME >> 32) as u32),
            (STACK_COUNT, 2),
        ])
        .into_created(benilla_protocol::messages::ObjectType::Item),
        false,
    );
    let mut c = Client {
        app,
        inbound,
        _sent: sent,
    };
    c.frame(vec![vec![created(
        ME,
        EntityKind::Player,
        &[
            (OBJECT_TYPE, TYPE_PLAYER),
            (FIELD_UNIT_HEALTH, 100),
            (FIELD_UNIT_MAXHEALTH, 100),
            (MANA, 100),
            (MAX_MANA, 100),
            (FLAGS, PLAYER_CONTROLLED),
            (PACK_SLOT_1, POTION_GUID as u32),
            (PACK_SLOT_1 + 1, (POTION_GUID >> 32) as u32),
        ],
    )]]);
    c.frame(vec![]);
    c.frame(vec![]);
    c.seen(&[]);
    c
}

/// The main hand holding the sword, or nothing.
fn wield(on: bool) -> [(u16, u32); 2] {
    let guid = if on { SWORD_GUID } else { 0 };
    [
        (MAIN_HAND, guid as u32),
        (MAIN_HAND + 1, (guid >> 32) as u32),
    ]
}

/// `UNIT_FIELD_BYTES_1` with `form` in byte 2.
fn form(form: u32) -> [(u16, u32); 1] {
    [(BYTES_1, form << 16)]
}

/// Wherever the frame's snapshots are current, every census getter answers the same live as off
/// the snapshot, for every slot: at load, in each form, with the sword worn and short of mana.
#[test]
fn every_action_getter_answers_the_same_live_as_off_the_snapshot() {
    let mut c = client();
    let agree = |c: &mut Client, why: &str| -> Vec<String> {
        let snapshot = c.answers(false);
        let live = c.answers(true);
        assert_eq!(snapshot.len(), SLOTS.len() * 14 + 1, "{why}");
        let differ: Vec<(&String, &String)> =
            snapshot.iter().zip(&live).filter(|(s, l)| s != l).collect();
        assert!(differ.is_empty(), "{why}: snapshot vs live: {differ:#?}");
        live
    };
    let at_load = agree(&mut c, "at load");
    c.frame(vec![vec![values(ME, &form(CAT_FORM))]]);
    let cat = agree(&mut c, "in cat form");
    c.frame(vec![vec![values(ME, &form(BEAR_FORM))]]);
    agree(&mut c, "in bear form");
    c.frame(vec![
        vec![values(ME, &form(0))],
        vec![values(ME, &wield(true))],
    ]);
    let worn = agree(&mut c, "the sword worn");
    c.frame(vec![vec![values(ME, &[(MANA, 10)])]]);
    let short = agree(&mut c, "short of mana");
    // The fixture reaches every leg it means to: the usable pair, the form arm, the equipped
    // walk, the attack slot and the bonus bar.
    for (answers, line) in [
        (&at_load, "1 IsUsableAction 1,nil"),
        (&at_load, "2 IsCurrentAction nil"),
        (&at_load, "4 IsEquippedAction nil"),
        (&at_load, "5 IsAttackAction 1"),
        (&at_load, "6 HasAction nil"),
        (&cat, "2 IsCurrentAction 1"),
        (&cat, "3 IsCurrentAction nil"),
        (&cat, "GetBonusBarOffset 1"),
        (&worn, "4 IsEquippedAction 1"),
        (&short, "1 IsUsableAction nil,1"),
    ] {
        assert!(
            answers.iter().any(|l| l == line),
            "{line:?} among {answers:#?}"
        );
    }
}

/// A store a packet moved after the frame's pushes reads as the object holds it, through the
/// host: the form byte for `IsCurrentAction`, the main hand for `IsEquippedAction`; the snapshot
/// still holds the push.
#[test]
fn the_object_getters_read_the_player_as_they_run() {
    let mut c = client();
    c.write_store(&form(CAT_FORM));
    c.write_store(&wield(true));
    let chunk = "return IsCurrentAction(2), IsEquippedAction(4)";
    let live: (Option<i64>, Option<i64>) = c.eval(true, chunk);
    assert_eq!(live, (Some(1), Some(1)));
    let snapshot: (Option<i64>, Option<i64>) = c.eval(false, chunk);
    assert_eq!(snapshot, (None, None));
}

/// Two power packets in one frame, the first leaving the player short of Frostbolt's 50 mana and
/// the second filling it again: each packet's power watcher refreshes the bar's usable cache
/// (`0x604876` → `0x4b31c0`), its `ACTIONBAR_UPDATE_USABLE` and `SPELL_UPDATE_USABLE` fire there,
/// and the `UNIT_MANA` its bridge signals next reads the same pair, not the frame's last.
#[test]
fn a_usable_listener_reads_its_own_packets_power() {
    let mut c = client();
    c.frame(vec![
        vec![values(ME, &[(MANA, 30)])],
        vec![values(ME, &[(MANA, 100)])],
    ]);
    let usable = |l: &String| l.split(':').take(3).collect::<Vec<_>>().join(":");
    let seen: Vec<String> = c
        .seen(&[
            "ACTIONBAR_UPDATE_USABLE",
            "SPELL_UPDATE_USABLE",
            "UNIT_MANA",
        ])
        .iter()
        .map(usable)
        .collect();
    assert_eq!(
        seen,
        [
            "ACTIONBAR_UPDATE_USABLE:nil:1",
            "SPELL_UPDATE_USABLE:nil:1",
            "UNIT_MANA:nil:1",
            "ACTIONBAR_UPDATE_USABLE:1:nil",
            "SPELL_UPDATE_USABLE:1:nil",
            "UNIT_MANA:1:nil",
        ]
    );
}

/// Two form packets in one frame: each form watcher's `ACTIONBAR_UPDATE_STATE` (`0x5de016`) reads
/// `IsCurrentAction` against its own packet's form byte, and its `UPDATE_BONUS_ACTIONBAR`
/// (`0x5de01b`) the offset that form's row pages to.
#[test]
fn a_form_listener_reads_its_own_packets_form() {
    let mut c = client();
    c.frame(vec![
        vec![values(ME, &form(CAT_FORM))],
        vec![values(ME, &form(BEAR_FORM))],
    ]);
    let checked = |l: &String| {
        let f: Vec<&str> = l.split(':').collect();
        format!("{}:{}:{}:{}", f[0], f[4], f[5], f[6])
    };
    let seen: Vec<String> = c
        .seen(&["ACTIONBAR_UPDATE_STATE", "UPDATE_BONUS_ACTIONBAR"])
        .iter()
        .map(checked)
        .collect();
    assert_eq!(
        seen,
        [
            "ACTIONBAR_UPDATE_STATE:1:nil:0",
            "UPDATE_BONUS_ACTIONBAR:1:nil:1",
            "ACTIONBAR_UPDATE_STATE:nil:1:1",
            "UPDATE_BONUS_ACTIONBAR:nil:1:3",
        ]
    );
}

/// Two packets in one frame, the first wielding the sword and the second taking it off, each
/// moving the mana too: each `UNIT_MANA` listener reads `IsEquippedAction` off its own packet's
/// main hand (`0x622270` walks the worn slots at the call).
#[test]
fn an_equipped_listener_reads_its_own_packets_main_hand() {
    let mut c = client();
    let wield_at = |on: bool, mana: u32| {
        let mut pairs = wield(on).to_vec();
        pairs.push((MANA, mana));
        vec![values(ME, &pairs)]
    };
    c.frame(vec![wield_at(true, 90), wield_at(false, 80)]);
    let worn = |l: &String| {
        let f: Vec<&str> = l.split(':').collect();
        format!("{}:{}", f[0], f[3])
    };
    let seen: Vec<String> = c.seen(&["UNIT_MANA"]).iter().map(worn).collect();
    assert_eq!(seen, ["UNIT_MANA:1", "UNIT_MANA:nil"]);
}

/// Drinking the last potion: the destroy's recount (`0x4e5c70`) stores the slot's count and, in
/// its slot write `0x4e58e0`, the slot's usable pair (`0x4e5903`-`0x4e5914`), before its
/// `ACTIONBAR_SLOT_CHANGED` (`0x4e5998`): the listener reads the button greyed, not lit.
#[test]
fn a_slot_changed_listener_reads_the_pair_its_recount_stored() {
    let mut c = client();
    c.frame(vec![vec![values(POTION_GUID, &[(STACK_COUNT, 1)])]]);
    assert_eq!(
        c.seen(&["ACTIONBAR_SLOT_CHANGED"]),
        ["ACTIONBAR_SLOT_CHANGED:7:1:1"],
        "one potion left: lit"
    );
    c.frame(vec![vec![SessionEvent::ObjectsRemoved(vec![POTION_GUID])]]);
    assert_eq!(
        c.seen(&["ACTIONBAR_SLOT_CHANGED"]),
        ["ACTIONBAR_SLOT_CHANGED:7:nil:0"],
        "the last one drunk: greyed in its own signal"
    );
}

/// The worn walk resolves each item as of the drain's place, as the getter's `0x468460` finds the
/// object at the call: a helm a later packet of the frame destroys is still worn to the listener
/// of an earlier one.
#[test]
fn an_equipped_listener_finds_an_item_a_later_packet_destroys() {
    let mut c = client();
    c.frame(vec![vec![values(
        ME,
        &[
            (HEAD, SWORD_GUID as u32),
            (HEAD + 1, (SWORD_GUID >> 32) as u32),
        ],
    )]]);
    c.seen(&[]);
    c.frame(vec![
        vec![values(ME, &[(MANA, 90)])],
        vec![SessionEvent::ObjectsRemoved(vec![SWORD_GUID])],
    ]);
    let worn = |l: &String| {
        let f: Vec<&str> = l.split(':').collect();
        format!("{}:{}", f[0], f[3])
    };
    let seen: Vec<String> = c.seen(&["UNIT_MANA"]).iter().map(worn).collect();
    assert_eq!(seen, ["UNIT_MANA:1"]);
}

/// The bar's getters' per-call cost with the host lent and off the snapshots: 10k calls of each.
///
/// Run with `cargo nextest run -p benilla-app --lib action_getter_cost --run-ignored only
/// --no-capture`.
#[test]
#[ignore = "a timing, not a check"]
fn action_getter_cost() {
    let mut c = client();
    c.frame(vec![vec![values(ME, &wield(true))]]);
    for call in [
        "IsUsableAction(1)",
        "IsCurrentAction(2)",
        "IsEquippedAction(4)",
        "IsEquippedAction(1)",
        "GetActionCooldown(1)",
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
    // A watcher's refresh, which each power, health or aura-state packet of ours now runs, over a
    // full bar of 120 Frostbolts.
    {
        let world = c.app.world_mut();
        let mut slots = world.resource_mut::<crate::ui_action::ActionSlots>();
        for slot in 0..120 {
            slots.buttons.insert(
                slot,
                ActionButton {
                    slot,
                    action: FROSTBOLT,
                    kind: benilla_protocol::messages::ACTION_KIND_SPELL,
                },
            );
        }
        slots.dirty = true;
    }
    c.frame(vec![]);
    let mut times = Vec::new();
    for _ in 0..200 {
        let t = std::time::Instant::now();
        crate::ui_action::run_watcher_refresh_once(c.app.world_mut());
        times.push(t.elapsed().as_secs_f64() * 1e6);
    }
    times.sort_by(f64::total_cmp);
    eprintln!(
        "a watcher's refresh, 120 slots: {:.0} us",
        times[times.len() / 2]
    );
}
