//! The loot getters as a listener reads them, and the loot window's events at their packets. The
//! getters read the window's cells and rows at the call (`0x4c2260`, `0x4c22a0`-`0x4c2670`,
//! `0x4c2740`) and the candidate array the list packet placed (`0x61c660`); the events fire inside
//! the handler that raises them: the copier's end (`0x4c1ee1`), a removal's (`0x4c1f77`,
//! `0x4c276f`), a close's teardown (`0x4c1dac`) and a candidate name's callback (`0x4c2fba`), and a
//! close the leave-world sweep makes waits behind what the lane still holds.

use benilla_protocol::field::FIELD_UNIT_HEALTH;
use benilla_protocol::messages::{loot_type, slot_type, ItemInfo, LootItem};
use benilla_protocol::{EntityKind, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;
use crossbeam_channel::{Receiver, Sender};

use super::unit_view_tests::{created, values, wire_packets};
use crate::net::ClientCommand;
use crate::ui_loot::{LootLatch, LootState};

const ME: u64 = 0x7;
const THRALL: u64 = 0x22;
const CORPSE: u64 = 0xF130_0000_0000_00AB;
const SECOND: u64 = 0xF130_0000_0000_00AC;
const LINEN: u32 = 2589;
const WOOL: u32 = 2592;

/// The whole client, headless, in the world.
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

    /// `chunk`'s answer through the lent host when `live`, else off the VM's copy alone.
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

    /// What went on the wire since the last call, as debug text.
    fn sent(&mut self) -> Vec<String> {
        self.sent.try_iter().map(|c| format!("{c:?}")).collect()
    }

    /// `packets` run through their handlers with no frame: the world as the handlers leave it.
    fn handle(&mut self, packets: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), packets);
        self.app.world_mut().flush();
    }

    /// Our `CMSG_LOOT` on `guid`, which arms the latch the response is admitted by (`0x5df253`).
    fn ask_loot(&mut self, guid: u64) {
        self.app.world_mut().resource_mut::<LootLatch>().0 = Some(guid);
    }
}

/// The client in the world beside two corpses, the cloths' records held, and a spy whose line for
/// each loot event and monster line carries its name and first argument.
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
            for _, e in ipairs({"LOOT_OPENED", "LOOT_SLOT_CLEARED", "LOOT_CLOSED",
                    "UPDATE_MASTER_LOOT_LIST", "CHAT_MSG_MONSTER_SAY"}) do
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
    app.world_mut().resource_mut::<crate::net::SelfGuid>().0 = Some(ME);
    {
        let mut items = app.world_mut().resource_mut::<crate::items::Items>();
        for (entry, name) in [(LINEN, "Linen Cloth"), (WOOL, "Wool Cloth")] {
            items.insert_template(
                entry,
                Some(ItemInfo {
                    quality: 1,
                    ..crate::items::test_template(name)
                }),
            );
        }
    }
    let mut c = Client { app, inbound, sent };
    c.frame(vec![vec![
        created(ME, EntityKind::Player, &[(FIELD_UNIT_HEALTH, 100)]),
        created(CORPSE, EntityKind::Unit, &[(FIELD_UNIT_HEALTH, 0)]),
        created(SECOND, EntityKind::Unit, &[(FIELD_UNIT_HEALTH, 0)]),
    ]]);
    c.frame(vec![]);
    c.seen();
    c.sent();
    c
}

fn row(slot: u8, entry: u32, kind: u8) -> LootItem {
    LootItem {
        slot,
        item_id: entry,
        count: 1,
        display_info_id: 0,
        random_property_id: 0,
        slot_type: kind,
    }
}

/// `guid`'s loot: `gold` copper and one row of each entry, in wire slots from 0.
fn response(guid: u64, gold: u32, entries: &[u32]) -> SessionEvent {
    SessionEvent::LootResponse {
        guid,
        loot_type: loot_type::CORPSE,
        gold,
        items: entries
            .iter()
            .enumerate()
            .map(|(i, &e)| row(i as u8, e, slot_type::ALLOW_LOOT))
            .collect(),
    }
}

/// A monster's line, which shows inside its own packet's handler on the chat lane.
fn boar_says(text: &str) -> SessionEvent {
    SessionEvent::Chat(benilla_protocol::messages::ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_MONSTER_SAY,
        language: 0,
        sender_guid: 0xF130_0000_0045_0003,
        target_guid: 0,
        sender_name: Some("Boar".into()),
        channel: None,
        text: text.into(),
        chat_tag: 0,
    })
}

/// The window as its getters read it: the count, then each row's name, `-` for a gap.
const READ: &str = "local n = GetNumLootItems() local s = tostring(n) \
    for i = 1, n do local _, name = GetLootSlotInfo(i) \
    s = s .. '/' .. (LootSlotIsItem(i) and name or (LootSlotIsCoin(i) and 'coin' or '-')) end \
    return s";

/// A response and a removal land with no frame between: the getters read the session at the call,
/// the removal's gap included, with no push; with no host the VM's copy, which nothing pushes,
/// holds none.
#[test]
fn the_loot_getters_read_the_session_at_the_call() {
    let mut c = client();
    c.ask_loot(CORPSE);
    c.handle(vec![response(CORPSE, 5, &[LINEN, WOOL])]);
    assert_eq!(
        c.eval::<String>(true, READ),
        "3/coin/Linen Cloth/Wool Cloth"
    );
    c.handle(vec![SessionEvent::LootRemoved { slot: 0 }]);
    assert_eq!(c.eval::<String>(true, READ), "3/coin/-/Wool Cloth");
    assert!(!c.eval::<bool>(true, "return IsFishingLoot() == 1"));
    assert_eq!(
        c.eval::<String>(false, READ),
        "0",
        "the VM's copy holds nothing"
    );
}

/// Each loot event fires at its packet's place among a chat line's of the packets around it: the
/// copier's end, a row's and the coin's removals, the release's close, and a removal after it,
/// which clears its row with no window open (`0x4c1f00` asks for no source; the close leaves the
/// rows, `0x4c1da0`) while the latch it is admitted by resolves (`0x5ebedf`).
#[test]
fn the_loot_events_fire_at_their_packets() {
    let mut c = client();
    c.ask_loot(CORPSE);
    c.frame(vec![
        vec![boar_says("a")],
        vec![response(CORPSE, 5, &[LINEN, WOOL])],
        vec![boar_says("b")],
        vec![SessionEvent::LootRemoved { slot: 1 }],
        vec![boar_says("c")],
        vec![SessionEvent::LootClearMoney],
        vec![boar_says("d")],
        vec![SessionEvent::LootReleaseResponse {
            guid: CORPSE,
            result: 1,
        }],
        vec![boar_says("e")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "LOOT_OPENED:nil",
            "CHAT_MSG_MONSTER_SAY:b",
            "LOOT_SLOT_CLEARED:3",
            "CHAT_MSG_MONSTER_SAY:c",
            "LOOT_SLOT_CLEARED:1",
            "CHAT_MSG_MONSTER_SAY:d",
            "LOOT_CLOSED:nil",
            "CHAT_MSG_MONSTER_SAY:e",
        ]
    );
    c.frame(vec![]);
    assert!(c.seen().is_empty(), "nothing fires after");

    // The next `CMSG_LOOT`'s latch, and the closed window's last row taken.
    c.ask_loot(SECOND);
    c.frame(vec![
        vec![boar_says("f")],
        vec![SessionEvent::LootRemoved { slot: 0 }],
        vec![boar_says("g")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:f",
            "LOOT_SLOT_CLEARED:2",
            "CHAT_MSG_MONSTER_SAY:g",
        ]
    );
    assert_eq!(c.eval::<i64>(true, "return GetNumLootItems()"), 0);
}

/// The list packet places the candidates and signals nothing (`0x61c550`); a candidate the name
/// cache misses answers nil and asks, and its answer signals `UPDATE_MASTER_LOOT_LIST` at its own
/// packet (`0x4c2f70`, `0x4c2fba`), once per call that missed; an unknown name signals nothing.
#[test]
fn a_candidates_name_signals_the_update_at_its_answer() {
    let mut c = client();
    c.ask_loot(CORPSE);
    c.frame(vec![
        vec![SessionEvent::LootMasterList {
            candidates: vec![THRALL],
        }],
        vec![SessionEvent::LootResponse {
            guid: CORPSE,
            loot_type: loot_type::CORPSE,
            gold: 0,
            items: vec![row(0, LINEN, slot_type::MASTER)],
        }],
    ]);
    assert_eq!(c.seen(), ["LOOT_OPENED:nil"], "the list signals nothing");
    c.sent();
    assert!(c.eval::<bool>(true, "return GetMasterLootCandidate(1) == nil"));
    assert!(c.eval::<bool>(true, "return GetMasterLootCandidate(2) == nil"));
    assert!(
        c.sent().iter().any(|s| s.contains(&format!("{THRALL}"))),
        "the miss asks the name"
    );
    c.eval::<()>(true, "GetMasterLootCandidate(1)");
    c.frame(vec![
        vec![boar_says("a")],
        vec![SessionEvent::PlayerName {
            guid: THRALL,
            name: "Thrall".into(),
            race: 2,
            gender: 0,
            class: 1,
        }],
        vec![boar_says("b")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "UPDATE_MASTER_LOOT_LIST:nil",
            "UPDATE_MASTER_LOOT_LIST:nil",
            "CHAT_MSG_MONSTER_SAY:b",
        ],
        "one callback per call that missed"
    );
    assert_eq!(
        c.eval::<String>(true, "return GetMasterLootCandidate(1)"),
        "Thrall"
    );
    c.frame(vec![vec![SessionEvent::PlayerName {
        guid: THRALL,
        name: "Thrall".into(),
        race: 2,
        gender: 0,
        class: 1,
    }]]);
    assert!(
        c.seen().is_empty(),
        "an answer nothing waits on signals nothing"
    );
}

/// A window and the leave-world sweep's close with no frame between: the open fires inside its
/// dispatch, the close shuts the session at once and its `LOOT_CLOSED` fires at the frame's
/// drain, leaving the window shut, read through the host or after.
#[test]
fn a_one_frame_open_then_close_leaves_no_stale_state() {
    let mut c = client();
    c.ask_loot(CORPSE);
    c.handle(vec![response(CORPSE, 0, &[LINEN])]);
    assert_eq!(
        c.seen(),
        ["LOOT_OPENED:nil"],
        "the open fires inside its dispatch"
    );
    crate::ui_script::sweep_test::sweep_in_place(&mut c.app);
    assert!(
        c.sent()
            .iter()
            .any(|s| s.starts_with("LootRelease") && s.contains(&format!("{CORPSE}"))),
        "the release goes out at the close"
    );
    assert_eq!(c.app.world().resource::<LootState>().source(), None);
    assert_eq!(c.app.world().resource::<LootLatch>().0, None);
    c.frame(vec![]);
    assert_eq!(c.seen(), ["LOOT_CLOSED:nil"]);
    assert_eq!(c.eval::<i64>(true, "return GetNumLootItems()"), 0);
    c.frame(vec![]);
    c.frame(vec![]);
    assert!(c.seen().is_empty(), "nothing fires after");

    // A release in the open's frame does the same at its packet.
    c.ask_loot(CORPSE);
    c.frame(vec![vec![
        response(CORPSE, 0, &[LINEN]),
        SessionEvent::LootReleaseResponse {
            guid: CORPSE,
            result: 1,
        },
    ]]);
    assert_eq!(c.seen(), ["LOOT_OPENED:nil", "LOOT_CLOSED:nil"]);
    assert_eq!(c.eval::<i64>(true, "return GetNumLootItems()"), 0);
    c.frame(vec![]);
    assert!(c.seen().is_empty(), "nothing fires after");
}

/// Two corpses looted in one frame, each response admitted by its own `CMSG_LOOT`'s latch: the
/// first's `LOOT_OPENED` listeners read the first's rows, the second's copier ends it with
/// `LOOT_CLOSED` (`0x4c1dac`), its own listeners read its rows, and the window ends as the
/// second's.
#[test]
fn a_second_corpse_in_the_frame_ends_with_its_loot() {
    let mut c = client();
    c.eval::<()>(
        true,
        &format!(
            "Rows = {{}} local read = loadstring({READ:?}) local f = CreateFrame('Frame') \
             f:RegisterEvent('LOOT_OPENED') \
             f:SetScript('OnEvent', function() table.insert(Rows, read()) end)"
        ),
    );
    c.ask_loot(CORPSE);
    c.handle(vec![response(CORPSE, 0, &[LINEN])]);
    c.ask_loot(SECOND);
    c.handle(vec![response(SECOND, 0, &[WOOL, LINEN])]);
    c.frame(vec![]);
    assert_eq!(
        c.seen(),
        ["LOOT_OPENED:nil", "LOOT_CLOSED:nil", "LOOT_OPENED:nil"]
    );
    assert_eq!(
        c.eval::<String>(true, "return table.concat(Rows, '|')"),
        "1/Linen Cloth|2/Wool Cloth/Linen Cloth",
        "each open's listeners read its own window"
    );
    assert_eq!(c.eval::<String>(true, READ), "2/Wool Cloth/Linen Cloth");
    assert_eq!(c.app.world().resource::<LootState>().source(), Some(SECOND));
    c.frame(vec![]);
    assert!(c.seen().is_empty(), "nothing fires after");
}

/// A candidate the roster holds by name but the player-name cache lacks reads nil: the getter asks
/// the cache alone (`0x4c2f70`, `0xc0e228`), never the roster, and its miss's callback signals
/// `UPDATE_MASTER_LOOT_LIST` when the name lands (`0x4c2fba`), after which it reads the name.
#[test]
fn a_roster_member_the_name_cache_lacks_reads_nil_until_its_name_lands() {
    let mut c = client();
    c.app
        .world_mut()
        .resource_mut::<crate::ui_party::GroupState>()
        .members
        .push(benilla_protocol::messages::GroupMemberEntry {
            name: "Thrall".into(),
            guid: THRALL,
            status: 0,
            flags: 0,
        });
    c.ask_loot(CORPSE);
    c.frame(vec![
        vec![SessionEvent::LootMasterList {
            candidates: vec![THRALL],
        }],
        vec![SessionEvent::LootResponse {
            guid: CORPSE,
            loot_type: loot_type::CORPSE,
            gold: 0,
            items: vec![row(0, LINEN, slot_type::MASTER)],
        }],
    ]);
    c.seen();
    assert!(
        c.eval::<bool>(true, "return GetMasterLootCandidate(1) == nil"),
        "the roster names no candidate"
    );
    c.frame(vec![vec![SessionEvent::PlayerName {
        guid: THRALL,
        name: "Thrall".into(),
        race: 2,
        gender: 0,
        class: 1,
    }]]);
    assert_eq!(c.seen(), ["UPDATE_MASTER_LOOT_LIST:nil"]);
    assert_eq!(
        c.eval::<String>(true, "return GetMasterLootCandidate(1)"),
        "Thrall"
    );
}

/// The candidate array as a step's packet left it: a `LOOT_OPENED` listener ran inside its
/// response (`0x4c1ee1`), before a later list packet in the frame cleared and refilled the array
/// (`0x61c567`), so it reads the list that came before its response.
#[test]
fn a_later_list_in_the_frame_does_not_show_through_to_an_earlier_step() {
    const GARROSH: u64 = 0x23;
    let mut c = client();
    let name = |guid, name: &str| SessionEvent::PlayerName {
        guid,
        name: name.into(),
        race: 2,
        gender: 0,
        class: 1,
    };
    c.frame(vec![
        vec![name(THRALL, "Thrall")],
        vec![name(GARROSH, "Garrosh")],
    ]);
    c.eval::<()>(
        true,
        r#"local f = CreateFrame("Frame")
        f:RegisterEvent("LOOT_OPENED")
        f:SetScript("OnEvent", function() CANDIDATE = GetMasterLootCandidate(1) end)"#,
    );
    c.ask_loot(CORPSE);
    c.frame(vec![
        vec![SessionEvent::LootMasterList {
            candidates: vec![THRALL],
        }],
        vec![SessionEvent::LootResponse {
            guid: CORPSE,
            loot_type: loot_type::CORPSE,
            gold: 0,
            items: vec![row(0, LINEN, slot_type::MASTER)],
        }],
        vec![SessionEvent::LootMasterList {
            candidates: vec![GARROSH],
        }],
    ]);
    assert_eq!(c.seen(), ["LOOT_OPENED:nil"]);
    assert_eq!(c.eval::<String>(false, "return CANDIDATE"), "Thrall");
    assert_eq!(
        c.eval::<String>(true, "return GetMasterLootCandidate(1)"),
        "Garrosh"
    );
}

/// A response with the targeting cursor up hands its unit to the cast (`0x5ebc35` → `0x48f3be` →
/// `0x4935d5` → `0x6e5b40`), which binds it inside the packet, ahead of the open (`0x48f3cb`): the
/// cast goes out in the frame with no selection sent, its signals (`0x6e60cb`, the send's
/// `0x6e589d`) fire after the line before it and ahead of `LOOT_OPENED` and the line after, and a
/// listener of the window, as every call after it, reads the cursor down.
#[test]
fn a_response_with_the_cursor_up_binds_its_unit_at_the_packet() {
    /// A living mark, which the pick-pocket's loot is from.
    const MARK: u64 = 0xF130_0000_0000_00AD;
    /// A spell the library does not know, so any unit binds under its `TARGET_FLAG_UNIT` word.
    const HEAL: u32 = 2050;
    let mut c = client();
    c.frame(vec![vec![created(
        MARK,
        EntityKind::Unit,
        &[(FIELD_UNIT_HEALTH, 100)],
    )]]);
    c.frame(vec![]);
    c.eval::<()>(
        false,
        r#"
        local g = CreateFrame("Frame")
        g:RegisterEvent("CURRENT_SPELL_CAST_CHANGED")
        g:RegisterEvent("LOOT_OPENED")
        g:SetScript("OnEvent", function()
            if event == "LOOT_OPENED" then
                table.insert(Seen, "targeting:" .. tostring(SpellIsTargeting()))
            else
                table.insert(Seen, event)
            end
        end)
        "#,
    );
    c.seen();
    c.sent();
    c.app
        .world_mut()
        .resource_mut::<crate::spell::SpellTargeting>()
        .enter(HEAL, crate::spell::CastCommit::Spell, 0x0002);
    c.ask_loot(MARK);
    c.frame(vec![
        vec![boar_says("a")],
        vec![SessionEvent::LootResponse {
            guid: MARK,
            loot_type: loot_type::PICKPOCKETING,
            gold: 0,
            items: vec![row(0, LINEN, slot_type::ALLOW_LOOT)],
        }],
        vec![boar_says("b")],
    ]);
    assert_eq!(
        c.seen(),
        [
            "CHAT_MSG_MONSTER_SAY:a",
            "CURRENT_SPELL_CAST_CHANGED",
            "CURRENT_SPELL_CAST_CHANGED",
            "LOOT_OPENED:nil",
            "targeting:nil",
            "CHAT_MSG_MONSTER_SAY:b",
        ]
    );
    let sent = c.sent();
    let casts: Vec<&String> = sent.iter().filter(|s| s.starts_with("CastSpell")).collect();
    assert_eq!(
        casts,
        [&format!(
            "CastSpell {{ spell_id: {HEAL}, target: Some({MARK}) }}"
        )],
        "{sent:?}"
    );
    assert!(
        !sent.iter().any(|s| s.starts_with("SetSelection")),
        "bound, not selected: {sent:?}"
    );
    assert!(c.eval::<bool>(true, "return SpellIsTargeting() == nil"));
}

/// A listener's close, then a removal in the same frame: `CloseLoot` at `LOOT_OPENED` runs before
/// the removal's handler (`0x537ae5`) and clears the latch, so the removal finds none
/// (`0x5ebedf`) and signals nothing.
#[test]
fn a_listeners_close_leaves_a_later_removal_unsignalled() {
    let mut c = client();
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("LOOT_OPENED")
        g:SetScript("OnEvent", function() CloseLoot() end)"#,
    );
    c.ask_loot(CORPSE);
    c.frame(vec![
        vec![response(CORPSE, 0, &[LINEN, WOOL])],
        vec![SessionEvent::LootRemoved { slot: 1 }],
    ]);
    assert_eq!(c.seen(), ["LOOT_OPENED:nil", "LOOT_CLOSED:nil"]);
}

/// Fireball, which our precast holds.
const FIREBALL: u32 = 133;

/// The corpse's window open, read in full, and a listener of the monster's line that reads the
/// window's row count as it fires, after the spy's own line; what the frames heard and sent
/// taken.
fn looting_the_corpse() -> Client {
    let mut c = client();
    c.ask_loot(CORPSE);
    c.frame(vec![vec![response(CORPSE, 0, &[LINEN])]]);
    assert_eq!(c.seen(), ["LOOT_OPENED:nil"]);
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("CHAT_MSG_MONSTER_SAY")
        g:SetScript("OnEvent", function()
            table.insert(Seen, "items:" .. GetNumLootItems())
        end)"#,
    );
    c.sent();
    c
}

/// Our precast of [`FIREBALL`], with its record held, as a send or a start left it.
fn precasting(c: &mut Client) {
    let world = c.app.world_mut();
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
            FIREBALL,
            Default::default(),
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::creature_anim::Precast {
        spell_id: FIREBALL,
        busy: true,
    });
}

/// `packet` ends our precast with its tail, whose loot close (`0x60d040` → `0x48f200` at
/// `0x60d10b`) runs inside it: its release goes out there and its `LOOT_CLOSED` fires there, so the
/// next packet's listener in the same frame hears it fired and reads the window shut.
fn closes_inside_its_packet(c: &mut Client, packet: SessionEvent) {
    c.frame(vec![vec![packet], vec![boar_says("b")]]);
    assert_eq!(
        c.seen(),
        ["LOOT_CLOSED:nil", "CHAT_MSG_MONSTER_SAY:b", "items:0"]
    );
    let sent = c.sent();
    assert_eq!(
        sent.iter()
            .filter(|s| s.starts_with("LootRelease") && s.contains(&format!("{CORPSE}")))
            .count(),
        1,
        "released once, in its frame: {sent:?}"
    );
    assert_eq!(c.app.world().resource::<LootState>().source(), None);
    c.frame(vec![]);
    assert!(c.seen().is_empty(), "nothing fires after");
}

/// Our cast's GO ends its precast (`0x6e7fc5`), the tail run as `0x6e4870` answers for a plain
/// record.
#[test]
fn a_go_closes_our_loot_inside_its_packet() {
    let mut c = looting_the_corpse();
    precasting(&mut c);
    closes_inside_its_packet(
        &mut c,
        SessionEvent::SpellGo {
            caster: ME,
            spell_id: FIREBALL,
            cast_flags: 0x100,
            hits: vec![],
            misses: vec![],
            target: None,
            go_target: None,
            target_mask: 0,
            source: None,
            dest: None,
            ammo_display_id: None,
            item_caster: None,
            unit_caster: ME,
        },
    );
}

/// `SMSG_SPELL_FAILED_OTHER` on our cast ends its precast with `b` set (`0x6e8e8c`).
#[test]
fn a_spell_failure_closes_our_loot_inside_its_packet() {
    let mut c = looting_the_corpse();
    precasting(&mut c);
    closes_inside_its_packet(
        &mut c,
        SessionEvent::SpellFailedOther {
            caster: ME,
            spell_id: FIREBALL,
        },
    );
}

/// A failed `SMSG_CAST_RESULT` reports the failure, whose player leg ends the precast of the
/// spell with its tail (`0x6e1c7f`-`0x6e1c8e`), whatever the reason.
#[test]
fn a_failed_cast_result_closes_our_loot_inside_its_packet() {
    let mut c = looting_the_corpse();
    precasting(&mut c);
    closes_inside_its_packet(
        &mut c,
        SessionEvent::CastResult {
            spell_id: FIREBALL,
            success: false,
            reason: Some(0x23),
            arg: None,
        },
    );
}

/// The health watcher's death of our selection: `OnDied`'s deselect is refused while we are
/// charmed (`0x49359c`), so the selection is still the dead unit (`0x6047c7`) and the active
/// player's precast ends with its own spell and its tail (`0x6047e7`), inside the update packet.
#[test]
fn a_selection_that_dies_and_stays_selected_closes_our_loot_inside_its_packet() {
    /// A living mark, selected.
    const MARK: u64 = 0xF130_0000_0000_00AD;
    /// `UNIT_FIELD_CHARMEDBY`, its low dword.
    const CHARMEDBY: u16 = 0x0a;
    let mut c = looting_the_corpse();
    c.frame(vec![
        vec![created(MARK, EntityKind::Unit, &[(FIELD_UNIT_HEALTH, 100)])],
        vec![values(ME, &[(CHARMEDBY, 0x99), (CHARMEDBY + 1, 0)])],
    ]);
    let world = c.app.world_mut();
    let mark = world.resource::<crate::net::GuidIndex>().0[&MARK];
    let mut selection = world.resource_mut::<crate::target::Selection>();
    selection.guid = Some(MARK);
    selection.target = Some(mark);
    c.frame(vec![]);
    c.seen();
    c.sent();
    assert_eq!(c.app.world().resource::<LootState>().source(), Some(CORPSE));
    closes_inside_its_packet(&mut c, values(MARK, &[(FIELD_UNIT_HEALTH, 0)]));
    assert_eq!(
        c.app.world().resource::<crate::target::Selection>().guid,
        Some(MARK),
        "the charmed deselect refused"
    );
}

/// A spline move on our own player ends its precast whatever it is (`0x6020e8`), the tail's
/// close read off the path it installed: moving, the move-start close, which takes a corpse's.
#[test]
fn a_monster_move_closes_our_loot_inside_its_packet() {
    let mut c = looting_the_corpse();
    closes_inside_its_packet(
        &mut c,
        SessionEvent::MonsterMove {
            guid: ME,
            transport: None,
            start: [0.0; 3],
            spline_id: 7,
            path: vec![[0.0; 3], [10.0, 0.0, 0.0]],
            facing: benilla_protocol::MonsterMoveFacing::None,
            stop: false,
            duration_ms: 1000,
            flying: false,
            run_mode: true,
        },
    );
}

/// A chest, `GAMEOBJECT_TYPE_ID` 3.
const CHEST: u64 = 0xF110_0000_0000_00AE;
/// Opening, an `OPEN_LOCK` spell.
const OPENING: u32 = 3365;

/// Our Opening's GO on the chest whose window we hold: the precast end's close takes that window
/// and its latch as the go's legs return (`0x6e7fc5`), then the player tail arms the chest as the
/// loot target (`0x6e831b` → `SetLootTarget 0x5ed5f0`), so the latch the GO arms is the one that
/// stands for the response it asks.
#[test]
fn a_go_on_the_chest_we_loot_keeps_the_latch_it_arms() {
    use benilla_protocol::messages::ObjectType;
    let mut c = client();
    c.frame(vec![vec![SessionEvent::ObjectCreate {
        guid: CHEST,
        kind: EntityKind::GameObject,
        display_id: None,
        position: [0.0; 3],
        orientation: 0.0,
        scale: 1.0,
        speeds: None,
        mover: None,
        transport_progress: None,
        transport: None,
        spline: None,
        fields: benilla_protocol::ObjectFields::from_pairs(&[(2, 0x21), (21, 3)])
            .into_created(ObjectType::GameObject),
    }]]);
    c.ask_loot(CHEST);
    c.frame(vec![vec![response(CHEST, 0, &[LINEN])]]);
    assert_eq!(c.seen(), ["LOOT_OPENED:nil"]);
    let world = c.app.world_mut();
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
            OPENING,
            benilla_formats::SpellDisplay {
                open_lock: Some(benilla_formats::OpenLock {
                    lock_type: 1,
                    effect: 0,
                }),
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::creature_anim::Precast {
        spell_id: OPENING,
        busy: true,
    });
    c.sent();
    c.frame(vec![vec![SessionEvent::SpellGo {
        caster: ME,
        spell_id: OPENING,
        cast_flags: 0x100,
        hits: vec![],
        misses: vec![],
        target: None,
        go_target: Some(CHEST),
        target_mask: 0,
        source: None,
        dest: None,
        ammo_display_id: None,
        item_caster: None,
        unit_caster: ME,
    }]]);
    assert_eq!(c.seen(), ["LOOT_CLOSED:nil"]);
    let sent = c.sent();
    assert_eq!(
        sent.iter()
            .filter(|s| s.starts_with("LootRelease") && s.contains(&format!("{CHEST}")))
            .count(),
        1,
        "the window released at the GO: {sent:?}"
    );
    assert_eq!(c.app.world().resource::<LootState>().source(), None);
    assert_eq!(
        c.app.world().resource::<LootLatch>().0,
        Some(CHEST),
        "the chest stays the loot target"
    );
}

/// Silk, whose record the client lacks.
const SILK: u32 = 4306;

/// A response that lands while we run, on a body with a row whose record the client lacks: its
/// copier returns with the fetch outstanding (`0x4c1ea6`), signalling nothing, and the response's
/// tail closes the window all the same (`0x5ebc3a`-`0x5ebc51`), inside its packet: released there,
/// `LOOT_CLOSED` there, and the teardown cancels the row's fetch (`0x4c1d4f`), so the record's
/// answer behind it in the frame copies nothing and `LOOT_OPENED` never fires.
#[test]
fn a_response_on_the_run_waiting_on_a_record_closes_inside_its_packet() {
    let mut c = client();
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("CHAT_MSG_MONSTER_SAY")
        g:SetScript("OnEvent", function()
            table.insert(Seen, "items:" .. GetNumLootItems())
        end)"#,
    );
    c.app
        .world_mut()
        .insert_resource(crate::player::Player::with_move_flags(
            crate::creature_anim::move_flags::FORWARD,
        ));
    c.ask_loot(CORPSE);
    c.frame(vec![
        vec![response(CORPSE, 0, &[SILK])],
        vec![SessionEvent::ItemTemplate {
            entry: SILK,
            info: Some(Box::new(crate::items::test_template("Silk Cloth"))),
        }],
        vec![boar_says("b")],
    ]);
    assert_eq!(
        c.seen(),
        ["LOOT_CLOSED:nil", "CHAT_MSG_MONSTER_SAY:b", "items:0"]
    );
    let sent = c.sent();
    assert_eq!(
        sent.iter()
            .filter(|s| s.starts_with("LootRelease") && s.contains(&format!("{CORPSE}")))
            .count(),
        1,
        "released once, at the response: {sent:?}"
    );
    assert!(
        !sent.iter().any(|s| s.starts_with("AutostoreLootItem")),
        "{sent:?}"
    );
    assert_eq!(c.app.world().resource::<LootState>().source(), None);
    c.frame(vec![]);
    assert!(c.seen().is_empty(), "nothing fires after");
}

/// Our cast's GO, which ends a precast of `spell_id` with its tail, hitting `hits`.
fn our_go(spell_id: u32, hits: Vec<u64>) -> SessionEvent {
    SessionEvent::SpellGo {
        caster: ME,
        spell_id,
        cast_flags: 0x100,
        hits,
        misses: vec![],
        target: None,
        go_target: None,
        target_mask: 0,
        source: None,
        dest: None,
        ammo_display_id: None,
        item_caster: None,
        unit_caster: ME,
    }
}

/// Where the first sent command starting with each of `heads` went, in `sent`.
fn sent_at(sent: &[String], head: &str) -> Option<usize> {
    sent.iter().position(|s| s.starts_with(head))
}

/// An auto-looted body: the copier ends inside the response with the sweep (`0x4c1ed3` →
/// `AutoLootAll 0x4c1fa0`), its coin and row requests sent there (`0x4c1ffc`, `0x4c2936`), so a
/// packet behind it in the frame that closes the window finds them sent: a GO that ends our precast
/// releases after them (`0x60d10b`), and the server's release answer, which vmangos sends whenever
/// we move (`MovementHandler.cpp:1104-1110`), takes a window already swept.
#[test]
fn an_auto_loot_response_sweeps_inside_its_packet_ahead_of_a_later_close() {
    for (what, later) in [
        ("a GO", our_go(FIREBALL, vec![])),
        (
            "a release answer",
            SessionEvent::LootReleaseResponse {
                guid: CORPSE,
                result: 1,
            },
        ),
    ] {
        let mut c = client();
        precasting(&mut c);
        c.app
            .world_mut()
            .resource_mut::<crate::ui_loot::AutoLootByte>()
            .0 = true;
        c.ask_loot(CORPSE);
        c.frame(vec![vec![response(CORPSE, 500, &[LINEN])], vec![later]]);
        let sent = c.sent();
        let (money, row) = (
            sent_at(&sent, "LootMoney"),
            sent_at(&sent, "AutostoreLootItem"),
        );
        assert!(money.is_some() && row.is_some(), "{what}: swept: {sent:?}");
        if let Some(release) = sent_at(&sent, "LootRelease") {
            assert!(
                money < Some(release) && row < Some(release),
                "{what}: {sent:?}"
            );
        }
        assert_eq!(
            c.app.world().resource::<LootState>().source(),
            None,
            "{what}"
        );
    }
}

/// `SMSG_STANDSTATE_UPDATE` applies in its packet (`0x603e50` → `0x6127b0` → `0x5f0790`): a sit
/// closes our loot there (`0x5f07d5`), so a later packet's listener in the frame reads the window
/// shut, and the stand behind it, which closes nothing (`0x5f07a9`), does not hide the sit.
#[test]
fn a_server_sit_closes_our_loot_inside_its_packet() {
    let mut c = looting_the_corpse();
    c.frame(vec![
        vec![SessionEvent::StandStateUpdate { state: 1 }],
        vec![boar_says("b")],
        vec![SessionEvent::StandStateUpdate { state: 0 }],
    ]);
    assert_eq!(
        c.seen(),
        ["LOOT_CLOSED:nil", "CHAT_MSG_MONSTER_SAY:b", "items:0"]
    );
    let sent = c.sent();
    assert_eq!(
        sent.iter()
            .filter(|s| s.starts_with("LootRelease") && s.contains(&format!("{CORPSE}")))
            .count(),
        1,
        "released once, at the sit: {sent:?}"
    );
}

/// A living mark our GO hits.
const MARK: u64 = 0xF130_0000_0000_00AD;
/// Cheap Shot, whose GO starts the swing (`AttributesEx2 & 0x100000`, `0x6e83c0`).
const CHEAP_SHOT: u32 = 1833;

/// The chest's window open, a living [`MARK`] beside it, and our precast of [`CHEAP_SHOT`], whose
/// GO starts the swing at what it hits; what the frames heard and sent taken. The window is a
/// chest's, which the start's select teardown does not close (`0x493959`).
fn cheap_shot_beside_the_open_chest() -> Client {
    use benilla_protocol::messages::ObjectType;
    /// `UNIT_FIELD_MAXHEALTH`.
    const MAXHEALTH: u16 = 28;
    let mut c = client();
    c.frame(vec![
        vec![created(
            MARK,
            EntityKind::Unit,
            &[(FIELD_UNIT_HEALTH, 100), (MAXHEALTH, 100)],
        )],
        vec![SessionEvent::ObjectCreate {
            guid: CHEST,
            kind: EntityKind::GameObject,
            display_id: None,
            position: [0.0; 3],
            orientation: 0.0,
            scale: 1.0,
            speeds: None,
            mover: None,
            transport_progress: None,
            transport: None,
            spline: None,
            fields: benilla_protocol::ObjectFields::from_pairs(&[(2, 0x21), (21, 3)])
                .into_created(ObjectType::GameObject),
        }],
    ]);
    c.ask_loot(CHEST);
    c.frame(vec![vec![response(CHEST, 0, &[LINEN])]]);
    let world = c.app.world_mut();
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
            CHEAP_SHOT,
            benilla_formats::SpellDisplay {
                attributes_ex2: 0x0010_0000,
                ..Default::default()
            },
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::creature_anim::Precast {
        spell_id: CHEAP_SHOT,
        busy: true,
    });
    c.seen();
    c.sent();
    c
}

/// The GO's precast end releases our loot at `0x6e7fc5`, ahead of its deferred attack start
/// (`0x6e8402` → `0x6131a0`), whose validator selects the hit (`0x613100`), so the release goes out
/// before the start's sends.
#[test]
fn a_gos_release_goes_out_before_its_deferred_attack_starts_sends() {
    let mut c = cheap_shot_beside_the_open_chest();
    c.frame(vec![vec![our_go(CHEAP_SHOT, vec![MARK])]]);
    let sent = c.sent();
    let release = sent_at(&sent, "LootRelease");
    let select = sent
        .iter()
        .position(|s| *s == format!("SetSelection {{ guid: {MARK} }}"));
    assert!(release.is_some() && select.is_some(), "{sent:?}");
    assert!(release < select, "{sent:?}");
    if let Some(swing) = sent_at(&sent, "AttackSwing") {
        assert!(release < Some(swing), "{sent:?}");
    }
}

/// The GO's precast end signals `LOOT_CLOSED` inside its close (`0x6e7fc5` → `0x4c1dac`), ahead of
/// its deferred attack start's select (`0x6e8402` → `0x6131a0` → `0x613100`), whose
/// `PLAYER_TARGET_CHANGED` (`0x49389c`) a listener hears after it.
#[test]
fn a_gos_loot_close_signals_before_its_deferred_attack_starts_select() {
    let mut c = cheap_shot_beside_the_open_chest();
    c.eval::<()>(
        false,
        r#"local g = CreateFrame("Frame")
        g:RegisterEvent("PLAYER_TARGET_CHANGED")
        g:SetScript("OnEvent", function() table.insert(Seen, event) end)"#,
    );
    c.frame(vec![vec![our_go(CHEAP_SHOT, vec![MARK])]]);
    let seen = c.seen();
    let closed = seen.iter().position(|s| s == "LOOT_CLOSED:nil");
    let selected = seen.iter().position(|s| s == "PLAYER_TARGET_CHANGED");
    assert!(closed.is_some() && selected.is_some(), "{seen:?}");
    assert!(closed < selected, "{seen:?}");
}

/// A sting whose column 38 names the next spell (`modalNextSpell`).
const STING: u32 = 1978;
/// What the sting chains, a plain spell.
const CHAINED: u32 = 5019;

/// A failed result of our precast spell ends the precast inside its report, releasing our loot
/// (`0x6e73ac` → `0x6e1a00`, `0x6e1c8e`), ahead of the `modalNextSpell` chain's cast
/// (`0x6e74aa` → `0x6e5a90`), which reason 0x17 leaves in reach (`0x6e1ce4`).
#[test]
fn a_cast_results_release_goes_out_before_its_chained_cast() {
    let mut c = looting_the_corpse();
    let world = c.app.world_mut();
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([
            (
                STING,
                benilla_formats::SpellDisplay {
                    modal_next_spell: CHAINED,
                    ..Default::default()
                },
            ),
            (CHAINED, benilla_formats::SpellDisplay::default()),
        ])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    world
        .resource_mut::<crate::spell::PendingCast>()
        .push(STING, None, std::time::Instant::now());
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::creature_anim::Precast {
        spell_id: STING,
        busy: true,
    });
    c.seen();
    c.sent();
    c.frame(vec![vec![SessionEvent::CastResult {
        spell_id: STING,
        success: false,
        reason: Some(0x17),
        arg: None,
    }]]);
    let sent = c.sent();
    let (release, cast) = (sent_at(&sent, "LootRelease"), sent_at(&sent, "CastSpell"));
    assert!(release.is_some() && cast.is_some(), "{sent:?}");
    assert!(release < cast, "{sent:?}");
}

/// Auto Shot, our auto-repeat spell.
const AUTO_SHOT: u32 = 75;

/// A failed result of our auto-repeat spell ends its precast inside the report, releasing our loot
/// (`0x6e1a00`, `0x6e1c8e` → `0x60d10b`), ahead of the report's auto-repeat stop, whose
/// `CMSG_CANCEL_AUTO_REPEAT_SPELL` goes out after it (`0x6e1cea` → `0x6ea080`, `0x6ea0c6`); a
/// reason but 0x17 reaches the stop (`0x6e1ce4`).
#[test]
fn a_failed_auto_shots_release_goes_out_before_its_auto_repeat_stop() {
    let mut c = looting_the_corpse();
    let world = c.app.world_mut();
    world.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
            AUTO_SHOT,
            Default::default(),
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    world.insert_resource(crate::spell::AutoRepeatActive(Some(AUTO_SHOT)));
    let me = world.resource::<crate::net::GuidIndex>().0[&ME];
    world.entity_mut(me).insert(crate::creature_anim::Precast {
        spell_id: AUTO_SHOT,
        busy: true,
    });
    c.seen();
    c.sent();
    c.frame(vec![vec![SessionEvent::CastResult {
        spell_id: AUTO_SHOT,
        success: false,
        reason: Some(0x23),
        arg: None,
    }]]);
    let sent = c.sent();
    let (release, stop) = (
        sent_at(&sent, "LootRelease"),
        sent_at(&sent, "CancelAutoRepeat"),
    );
    assert!(release.is_some() && stop.is_some(), "{sent:?}");
    assert!(release < stop, "{sent:?}");
}
