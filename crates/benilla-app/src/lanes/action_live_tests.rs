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
/// Frostbolt's second rank, which supersedes the first.
const FROSTBOLT_2: u32 = 205;
const CAT: u32 = 768;
const BEAR: u32 = 5487;
const ATTACK: u32 = 6603;
/// Two reactive spells (`AttributesEx4 & 0x200`) that need the defense aura state, off the bar,
/// which the login lists Overpower first.
const OVERPOWER: u32 = 7384;
const REVENGE: u32 = 6572;
/// A spell whose `ActiveIconID` shows while its own cancelable aura is on (`0x4e55f0`), and that
/// icon.
const TOGGLE: u32 = 2645;
const TOGGLE_ACTIVE: &str = "Interface\\Icons\\Spell_Active";
/// `UNIT_FIELD_AURA` and `UNIT_FIELD_AURAFLAGS`, slot 0's aura and its nibble.
const AURA_0: u16 = 47;
const AURA_FLAGS_0: u16 = 95;
/// `UNIT_FIELD_AURASTATE`, whose bit 0 is the state the two need.
const AURA_STATE: u16 = 125;
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
    let reactive = |d: &mut SpellDisplay| {
        d.attributes_ex4 = 0x200;
        d.caster_aura_state = 1;
    };
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
        row(FROSTBOLT_2, |d| {
            d.mana_cost = 60;
            d.recovery_ms = 120_000;
        }),
        row(CAT, |d| d.shapeshift_form = Some(CAT_FORM)),
        row(BEAR, |d| d.shapeshift_form = Some(BEAR_FORM)),
        // `SPELL_EFFECT_ATTACK`, which makes the slot the Attack slot (`0x4e59de`).
        row(ATTACK, |d| d.effects[0] = 78),
        row(OVERPOWER, reactive),
        row(REVENGE, reactive),
        row(TOGGLE, |d| {
            d.active_icon_id = 1;
            d.active_icon = Some(TOGGLE_ACTIVE.to_string());
        }),
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
    client_with(|_| {})
}

/// [`client`] with `more` run on its world before our player's create.
fn client_with(more: impl FnOnce(&mut World)) -> Client {
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
    {
        let mut actions = world.resource_mut::<crate::ui_action::PlayerActions>();
        actions
            .spells
            .extend([FROSTBOLT, CAT, BEAR, ATTACK, OVERPOWER, REVENGE]);
        actions.initial = vec![OVERPOWER, REVENGE];
    }
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
    more(world);
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
    let seen: Vec<String> = c
        .seen(&[
            "ACTIONBAR_UPDATE_USABLE",
            "SPELL_UPDATE_USABLE",
            "UNIT_MANA",
        ])
        .iter()
        .map(|l| usable(l))
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

/// The usable pair a spy line carries: its event and `IsUsableAction(1)`.
fn usable(l: &str) -> String {
    l.split(':').take(3).collect::<Vec<_>>().join(":")
}

/// `EditMacro`'s worker ends in the bar's usable cache refresh (`0x4f132e` into `0x4e5c00`),
/// inside the call: a pair that moved with no refresh signals `ACTIONBAR_UPDATE_USABLE` before the
/// call returns, so the chunk's next line has heard it, and no later frame signals it again.
#[test]
fn an_edit_macro_refreshes_the_usable_cache_inside_its_call() {
    let mut c = client();
    c.eval::<()>(true, "CreateMacro('m', 1, '/say hi', 1)");
    c.write_store(&[(MANA, 10)]);
    let heard: Vec<String> = c.eval(
        true,
        "Seen = {} EditMacro(1, 'n') local s = Seen Seen = {} return s",
    );
    let heard: Vec<String> = heard
        .iter()
        .filter(|l| l.starts_with("ACTIONBAR_UPDATE_USABLE"))
        .map(|l| usable(l))
        .collect();
    assert_eq!(heard, ["ACTIONBAR_UPDATE_USABLE:nil:1"], "inside the call");
    c.frame(vec![]);
    assert!(c.seen(&["ACTIONBAR_UPDATE_USABLE"]).is_empty());
}

/// Two packets in one frame: the potion's stack count, whose watcher's `RaiseBagUpdate`
/// refreshes the bar's usable cache ahead of its `BAG_UPDATE` (`0x4f9370` → `0x4f9374`), then a
/// power packet. The run's refresh reads the pair a quiet write moved and signals at the run,
/// between the two packets' events.
#[test]
fn a_bag_updates_usable_refresh_signals_at_its_run() {
    let mut c = client();
    c.eval::<()>(
        false,
        "local f = CreateFrame('Frame') f:RegisterEvent('BAG_UPDATE') \
         f:SetScript('OnEvent', function() table.insert(Seen, event) end)",
    );
    c.write_store(&[(MANA, 10)]);
    c.frame(vec![
        vec![values(POTION_GUID, &[(STACK_COUNT, 1)])],
        vec![values(ME, &[(MANA, 100)])],
    ]);
    let seen: Vec<String> = c
        .seen(&["ACTIONBAR_UPDATE_USABLE", "BAG_UPDATE", "UNIT_MANA"])
        .iter()
        .map(|l| usable(l))
        .collect();
    assert_eq!(
        seen,
        [
            "ACTIONBAR_UPDATE_USABLE:nil:1",
            "BAG_UPDATE",
            "ACTIONBAR_UPDATE_USABLE:1:nil",
            "UNIT_MANA:1:nil",
        ]
    );
}

/// `PlaceAction`'s write stores the slot's pair inside the call (`0x4e62e0` → `0x4e58e0`,
/// `0x4e590a`), ahead of its `ACTIONBAR_SLOT_CHANGED` (`0x4e5998`): the listener reads the placed
/// spell lit, and the slot the pickup cleared grey.
#[test]
fn a_placed_slots_listener_reads_the_pair_its_write_stored() {
    let mut c = client();
    let heard: Vec<String> = c.eval(
        true,
        "Seen = {} PickupAction(1) PlaceAction(8) local s = Seen Seen = {} return s",
    );
    let slots: Vec<&String> = heard
        .iter()
        .filter(|l| l.starts_with("ACTIONBAR_SLOT_CHANGED"))
        .collect();
    assert_eq!(
        slots,
        [
            "ACTIONBAR_SLOT_CHANGED:1:nil:0",
            "ACTIONBAR_SLOT_CHANGED:8:1:0"
        ]
    );
}

/// A ration the cache has no record of, on bar slot 8, three of them ours.
const RATION_GUID: u64 = 0x4000_0000_0000_00F2;
const RATION: u32 = 4540;
const RATION_DISPLAY: u32 = 6413;
const RATION_ICON: &str = "Interface\\Icons\\INV_Misc_Food_11";

/// Our arrival's slot pass counts the ration slot through the item cache with the bar's callback
/// (`0x5deb00` → `0x4e5d60` → `0x4e6d20`, `0x4e6d69`), which the record's answer runs inside its
/// packet (`0x55bf39` → `0x4e5b80` → `0x4e5c70`): the slot's `ACTIONBAR_SLOT_CHANGED` fires between
/// the packets around the answer, and every getter reads the slot's icon, consumable test and count
/// at its call (`0x4e6a50`, `0x4e5250`, `0x4e6c70`): the first packet's listener reads the count our
/// arrival stored before the rations reached the pack, and the last the answer's. The record is the
/// cache's as the frame's packets left it until each packet's events fire inside its dispatch, so
/// both read the ration's icon.
#[test]
fn a_bar_items_record_repaints_its_slot_at_the_answer_and_the_getters_read_it_live() {
    let mut c = client_with(|world| {
        crate::items::test_spawn_item(
            world,
            RATION_GUID,
            ObjectFields::from_pairs(&[
                (OBJECT_TYPE, 0x3),
                (3, RATION),
                (6, ME as u32),
                (7, (ME >> 32) as u32),
                (STACK_COUNT, 3),
            ])
            .into_created(benilla_protocol::messages::ObjectType::Item),
            false,
        );
        world
            .resource_mut::<crate::ui_action::ActionSlots>()
            .buttons
            .insert(
                7,
                ActionButton {
                    slot: 7,
                    action: RATION,
                    kind: benilla_protocol::messages::ACTION_KIND_ITEM,
                },
            );
        world.insert_resource(crate::entities::ItemDisplays::icons_for_tests(
            benilla_formats::ItemDisplayCatalog::from_displays(HashMap::from([(
                RATION_DISPLAY,
                benilla_formats::ItemDisplay {
                    icon: Some(RATION_ICON.to_string()),
                    ..Default::default()
                },
            )])),
        ));
    });
    c.write_store(&[
        (PACK_SLOT_1 + 2, RATION_GUID as u32),
        (PACK_SLOT_1 + 3, (RATION_GUID >> 32) as u32),
    ]);
    c.eval::<()>(
        false,
        "local f = CreateFrame('Frame') f:RegisterEvent('UNIT_MANA') \
         f:RegisterEvent('ACTIONBAR_SLOT_CHANGED') \
         f:SetScript('OnEvent', function() \
             if event == 'ACTIONBAR_SLOT_CHANGED' and arg1 ~= 8 then return end \
             table.insert(Seen, 'RATION:' .. event .. ':' .. tostring(GetActionTexture(8)) .. ':' \
                 .. tostring(IsConsumableAction(8)) .. ':' .. GetActionCount(8)) end)",
    );
    c.seen(&[]);
    // A cold record reads the resolver's question mark at the call; the arrival's count asked for
    // it, and the getter, finding it pending, asks nothing more.
    let icon: String = c.eval(true, "return GetActionTexture(8)");
    assert_eq!(icon, crate::ui_action::MISSING_ITEM_ICON);
    let asked = crate::query_cache::queries(&c._sent)
        .into_iter()
        .filter(|q| matches!(q, ClientCommand::ItemQuery { entry, .. } if *entry == RATION))
        .count();
    assert_eq!(asked, 1);
    // Tough rations: one on-use block whose charges -1 make the slot show its count.
    let mut info = crate::items::test_template("Ration");
    info.display_info_id = RATION_DISPLAY;
    info.spells = vec![benilla_protocol::messages::ItemSpellEntry {
        index: 0,
        spell_id: 433,
        trigger: 0,
        charges: -1,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    }];
    c.frame(vec![
        vec![values(ME, &[(MANA, 90)])],
        vec![SessionEvent::ItemTemplate {
            entry: RATION,
            info: Some(Box::new(info)),
        }],
        vec![values(ME, &[(MANA, 100)])],
    ]);
    let ration: Vec<String> = c
        .seen(&["RATION"])
        .into_iter()
        .map(|l| l.trim_start_matches("RATION:").to_string())
        .collect();
    assert_eq!(
        ration,
        [
            format!("UNIT_MANA:{RATION_ICON}:1:0"),
            format!("ACTIONBAR_SLOT_CHANGED:{RATION_ICON}:1:3"),
            format!("UNIT_MANA:{RATION_ICON}:1:3"),
        ]
    );
}

/// The active player's aura add and removal each notify the bar's slots that stand for the spell
/// (`0x612440`, `0x61237e` → `0x4e5de0`): a toggle's `ACTIONBAR_SLOT_CHANGED` fires inside each
/// aura packet, between its neighbours' events, after the add's line and ahead of the removal's
/// (`0x62b480`), and its listener's `GetActionTexture` reads that packet's aura (`0x4e6bbd`): the
/// active icon at the add, the spell's own at the removal.
#[test]
fn an_aura_packet_notifies_its_spells_slots_between_its_neighbours() {
    let mut c = client_with(|world| {
        world
            .resource_mut::<crate::ui_action::ActionSlots>()
            .buttons
            .insert(
                7,
                ActionButton {
                    slot: 7,
                    action: TOGGLE,
                    kind: benilla_protocol::messages::ACTION_KIND_SPELL,
                },
            );
    });
    c.eval::<()>(
        false,
        "AURAADDEDSELFHELPFUL = 'You gain %s.' AURAREMOVEDSELF = '%s fades from you.' \
         local f = CreateFrame('Frame') f:RegisterEvent('UNIT_MANA') \
         f:RegisterEvent('ACTIONBAR_SLOT_CHANGED') f:RegisterEvent('CHAT_MSG_SPELL_PERIODIC_SELF_BUFFS') \
         f:RegisterEvent('CHAT_MSG_SPELL_AURA_GONE_SELF') \
         f:SetScript('OnEvent', function() \
             if event == 'ACTIONBAR_SLOT_CHANGED' and arg1 ~= 8 then return end \
             if event == 'UNIT_MANA' and arg1 ~= 'player' then return end \
             table.insert(Seen, 'TOGGLE:' .. event .. ':' .. tostring(GetActionTexture(8))) end)",
    );
    c.seen(&[]);
    c.frame(vec![
        vec![values(ME, &[(MANA, 90)])],
        vec![values(ME, &[(AURA_0, TOGGLE), (AURA_FLAGS_0, 0xb)])],
        vec![values(ME, &[(AURA_0, 0), (AURA_FLAGS_0, 0)])],
        vec![values(ME, &[(MANA, 100)])],
    ]);
    let own = format!("Interface\\Icons\\Spell_{TOGGLE}");
    assert_eq!(
        c.seen(&["TOGGLE"]),
        [
            format!("TOGGLE:UNIT_MANA:{own}"),
            format!("TOGGLE:CHAT_MSG_SPELL_PERIODIC_SELF_BUFFS:{TOGGLE_ACTIVE}"),
            format!("TOGGLE:ACTIONBAR_SLOT_CHANGED:{TOGGLE_ACTIVE}"),
            format!("TOGGLE:ACTIONBAR_SLOT_CHANGED:{own}"),
            format!("TOGGLE:CHAT_MSG_SPELL_AURA_GONE_SELF:{own}"),
            format!("TOGGLE:UNIT_MANA:{own}"),
        ]
    );
}

/// `GameTooltip:SetAction` runs the slot's cooldown query at the call (`0x5323a6` → `0x4e6ca0`),
/// so a hover inside the packet that armed a two-minute cooldown builds the remaining time and
/// answers 1 (`0x532597`, `0x5325a9`), where the frame's push still holds none.
#[test]
fn set_action_reads_the_slots_cooldown_at_the_call() {
    let mut c = client_with(|world| {
        world
            .resource_mut::<crate::ui_action::PlayerActions>()
            .spells
            .insert(FROSTBOLT_2);
        world
            .resource_mut::<crate::ui_action::ActionSlots>()
            .buttons
            .insert(
                7,
                ActionButton {
                    slot: 7,
                    action: FROSTBOLT_2,
                    kind: benilla_protocol::messages::ACTION_KIND_SPELL,
                },
            );
    });
    crate::ui_script::test_ui::load_tooltip_template(c.app.world().non_send_resource::<UiScript>());
    c.eval::<()>(
        false,
        "local a = CreateFrame('Button', 'B') a:SetPoint('CENTER', 0, 0) a:SetWidth(10) \
         a:SetHeight(10) CreateFrame('GameTooltip', 'TT', nil, 'TestTooltipTemplate')",
    );
    let hover = "TT:SetOwner(B, 'ANCHOR_RIGHT') return TT:SetAction(8)";
    assert_eq!(c.eval::<Option<i64>>(true, hover), None, "nothing running");
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::SpellCooldowns {
            caster: ME,
            cooldowns: vec![(FROSTBOLT_2, 120_000)],
        }],
    );
    assert_eq!(
        c.eval::<Option<i64>>(true, hover),
        Some(1),
        "live: the armed cooldown"
    );
    assert_eq!(
        c.eval::<Option<i64>>(false, hover),
        None,
        "the frame's push holds none"
    );
}

/// A new VM's copy of the bar is the bar's own, its counts as the last recount stored them: the
/// tables outlive a VM (`0x4e4f50` runs only at the game entry), so a stack that moved with no
/// recount reads the stored count after a reload too, and no slot signals.
#[test]
fn a_new_vm_is_seeded_with_the_bars_words_and_stored_counts() {
    let mut c = client();
    let potions = c.app.world().resource::<crate::net::GuidIndex>().0[&POTION_GUID];
    c.app
        .world_mut()
        .get_mut::<crate::net::ObjectStore>(potions)
        .expect("the potions")
        .0
        .merge(ObjectFields::from_pairs(&[(STACK_COUNT, 5)]));
    c.app
        .world_mut()
        .insert_non_send_resource(UiScript::new().expect("a VM"));
    c.eval::<()>(
        false,
        "Seen = {} local f = CreateFrame('Frame') f:RegisterEvent('ACTIONBAR_SLOT_CHANGED') \
         f:SetScript('OnEvent', function() table.insert(Seen, event) end)",
    );
    c.frame(vec![]);
    let chunk = "return HasAction(1), HasAction(6), GetActionCount(7), GetActionTexture(1)";
    let seeded: (Option<i64>, Option<i64>, i64, String) = c.eval(false, chunk);
    assert_eq!(
        seeded,
        (
            Some(1),
            None,
            2,
            format!("Interface\\Icons\\Spell_{FROSTBOLT}")
        )
    );
    let live: (Option<i64>, Option<i64>, i64, String) = c.eval(true, chunk);
    assert_eq!(live, seeded);
    assert!(c.seen(&["ACTIONBAR_SLOT_CHANGED"]).is_empty());
}

/// Our create's arrival empties the reactive list and its registrar runs take the initial spells in
/// again in wire order (`0x5dea86`, `0x5dea96`-`0x5deab9` → `0x4b27b3`), so a refresh raises their
/// words in that order (`0x4b31d7`-`0x4b3226`): Overpower, listed first, before Revenge, whose id
/// is lower.
#[test]
fn the_arrival_lists_the_initial_reactive_spells_in_wire_order() {
    let mut c = client();
    c.eval::<()>(
        false,
        "local f = CreateFrame('Frame') f:RegisterEvent('COMBAT_TEXT_UPDATE') \
         f:SetScript('OnEvent', function() \
             table.insert(Seen, event .. ':' .. arg1 .. ':' .. arg2) end)",
    );
    c.frame(vec![vec![values(ME, &[(AURA_STATE, 1)])]]);
    assert_eq!(
        c.seen(&["COMBAT_TEXT_UPDATE"]),
        [
            "COMBAT_TEXT_UPDATE:SPELL_ACTIVE:Spell 7384",
            "COMBAT_TEXT_UPDATE:SPELL_ACTIVE:Spell 6572",
        ]
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

/// `SMSG_SUPERCEDED_SPELL` gives a bar macro that cast the old rank the new one in its record
/// (`0x5e6356` → `0x4e5e60`, `0x4e5ec4`), with no signal; `GetActionCooldown` resolves the slot
/// through that record at the call (`0x4e6ca0` → `0x4e5a50` → `0x4e5ba0`) and queries the new
/// rank's cooldown (`0x6e2ea0`). So a getter inside the packet that rewrote the record reads the
/// new rank's 10 s, where the frame's push still names the old rank.
#[test]
fn a_superseded_macro_slots_cooldown_reads_the_new_rank_inside_its_packet() {
    use crate::ui_macro::{Bound, BoundSpell};
    let mut c = client();
    {
        let world = c.app.world_mut();
        crate::ui_macro::bind_for_test(
            &mut world.non_send_resource_mut::<UiScript>(),
            &[(1, Bound::from(BoundSpell::Spell(FROSTBOLT)))],
        );
        world
            .resource_mut::<crate::ui_action::PlayerActions>()
            .spells
            .insert(FROSTBOLT_2);
        crate::ui_action::hold_for_test(world, 6, benilla_protocol::messages::ACTION_KIND_MACRO, 1);
    }
    c.frame(vec![vec![SessionEvent::SpellCooldowns {
        caster: ME,
        cooldowns: vec![(FROSTBOLT_2, 10_000)],
    }]]);
    c.frame(vec![]);
    let chunk = "local _, d = GetActionCooldown(6) return d";
    assert_eq!(c.eval::<f64>(true, chunk), 0.0, "the macro casts rank 1");
    crate::net::handlers::dispatch(
        c.app.world_mut(),
        vec![SessionEvent::SpellSuperceded {
            old_spell_id: FROSTBOLT,
            new_spell_id: FROSTBOLT_2,
        }],
    );
    assert_eq!(c.eval::<f64>(true, chunk), 10.0, "live: the new rank's");
    assert_eq!(
        c.eval::<f64>(false, chunk),
        0.0,
        "the frame's push names rank 1"
    );
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
