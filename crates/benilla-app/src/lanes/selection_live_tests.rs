//! The selection's and the interaction's globals as the getters read them, through the lent host:
//! the resolver reads `[0xb4e2d8]` for `"target"` (`0x515a86`) and `[0xb4e2d0]` for `"npc"`
//! (`0x515c00`) when it runs. A target's release tears the selection down inside its packet's
//! handler (`0x5fbb60` → `0x493910` at `0x5fbbe5`), so its `PLAYER_TARGET_CHANGED` (`0x493a73`)
//! fires at that packet's place among the other packets' events. So do the selects a packet
//! makes: the attacked and cast-at acquires (`0x6259fe`, `0x6e799c`) and the loot response's
//! (`0x48f3a0`).

use benilla_protocol::{ObjectFields, SessionEvent};
use benilla_ui::script::UiScript;
use bevy::prelude::*;

use super::unit_view_tests::{client, created, frame, health, values};

const ME: u64 = 0x7;
const WOLF: u64 = 0xF130_0000_0044_0001;
const BOAR: u64 = 0xF130_0000_0045_0002;
/// A quest-giving game object, which the latch holds while its quest panel is open.
const POSTER: u64 = 0xF110_0000_0046_0003;

/// `chunk`'s answer with the call host lent, as a listener's or a key's Lua runs.
fn hosted<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
    crate::script_calls::with_hosted_vm(app.world_mut(), |s| s.expect("a VM").eval(chunk))
        .expect("the chunk runs")
}

/// `chunk`'s answer with no host lent: the snapshot the feeds pushed.
fn unhosted<T: benilla_ui::mlua::FromLuaMulti>(app: &mut App, chunk: &str) -> T {
    crate::script_calls::eval_in_vm(app.world(), chunk)
}

/// The world's selection moved to `guid` with nothing pushed to the VM.
fn select(app: &mut App, guid: Option<u64>) {
    let world = app.world_mut();
    let target = guid.and_then(|g| world.resource::<crate::net::GuidIndex>().0.get(&g).copied());
    *world.resource_mut::<crate::target::Selection>() = crate::target::Selection {
        target,
        guid,
        ..Default::default()
    };
}

/// A getter with the host lent reads the world's target and interaction NPC as they stand, the
/// guid of each with no push between; with no host, the snapshot the VM holds. The target global
/// is returned whatever it names (`0x515a86`), and a cleared one is nobody.
#[test]
fn a_hosted_getter_reads_the_live_target_and_npc() {
    let mut app = client();
    const READ: &str = r#"return UnitHealth("target"), UnitHealth("npc"),
        UnitIsUnit("target", "npc") and 1 or 0"#;
    assert_eq!(hosted::<(u32, u32, u8)>(&mut app, READ), (100, 0, 0));
    // The VM's copies of the tokens, which no feed pushes in game, seated on the wolf.
    crate::ui_unit::seat_tokens_once(app.world_mut());
    // The boar selected and latched, the VM told nothing.
    select(&mut app, Some(BOAR));
    app.world_mut()
        .resource_mut::<crate::ui_session::InteractLatch>()
        .latch_for_test(BOAR);
    assert_eq!(hosted::<(u32, u32, u8)>(&mut app, READ), (50, 50, 1));
    assert_eq!(
        unhosted::<(u32, u32, u8)>(&mut app, READ),
        (100, 0, 0),
        "no host, the snapshot"
    );
    // Cleared: nobody, though the VM still holds the wolf.
    select(&mut app, None);
    assert!(hosted::<bool>(
        &mut app,
        r#"return UnitExists("target") == nil"#
    ));
    assert!(unhosted::<bool>(
        &mut app,
        r#"return UnitExists("target") == 1"#
    ));
}

/// `[0xb4e2d0]` is the latch whatever it holds (`0x515c00`), and a getter looks the guid up
/// itself: `UnitExists` finds a quest-giving game object (mask 1, `0x515fd9`) whose slot 22
/// answers 0 (`0x515fee`), so nil, while `UnitName` names it by its record (`0x517146` →
/// `0x5f8100`), as `QuestFrame.lua:63` shows it, and a text reader's item by its cached template
/// (`0x517159`), `UNKNOWNOBJECT` on a miss.
#[test]
fn the_npc_latched_on_an_object_is_named_and_exists_for_nobody() {
    const POSTER_ENTRY: u32 = 31;
    const LETTER: u64 = 0x4000_0000_0000_0047;
    const LETTER_ENTRY: u32 = 5520;
    let mut app = client();
    let world = app.world_mut();
    world
        .resource_mut::<crate::go_templates::GameObjectTemplates>()
        .insert(POSTER_ENTRY, 2, "Wanted Poster".into(), &[0; 24]);
    let poster = world
        .spawn((
            crate::net::Guid(POSTER),
            crate::net::ObjectStore(crate::go_templates::test_fields(POSTER_ENTRY)),
        ))
        .id();
    let letter = world
        .spawn((
            crate::net::Guid(LETTER),
            crate::net::ObjectStore(ObjectFields::from_pairs(&[(2, 0x3), (3, LETTER_ENTRY)])),
        ))
        .id();
    let mut index = world.resource_mut::<crate::net::GuidIndex>();
    index.0.insert(POSTER, poster);
    index.0.insert(LETTER, letter);
    let latch = |app: &mut App, guid: u64| {
        app.world_mut()
            .resource_mut::<crate::ui_session::InteractLatch>()
            .latch_for_test(guid);
    };
    const READ: &str = r#"return UnitName("npc"), UnitExists("npc") == nil,
        UnitIsUnit("npc", "target") == nil"#;
    latch(&mut app, POSTER);
    assert_eq!(
        hosted::<(String, bool, bool)>(&mut app, READ),
        ("Wanted Poster".into(), true, true)
    );
    latch(&mut app, LETTER);
    assert_eq!(
        hosted::<(String, bool, bool)>(&mut app, READ),
        ("Unknown Being".into(), true, true),
        "no template yet"
    );
    app.world_mut()
        .resource_mut::<crate::items::Items>()
        .insert_template(
            LETTER_ENTRY,
            Some(crate::items::test_template("A Sealed Letter")),
        );
    assert_eq!(
        hosted::<(String, bool, bool)>(&mut app, READ),
        ("A Sealed Letter".into(), true, true)
    );
    // Control: a unit latched is the NPC.
    latch(&mut app, BOAR);
    assert_eq!(hosted::<u32>(&mut app, r#"return UnitHealth("npc")"#), 50);
}

/// The target's destroy, between two of our health packets: its `PLAYER_TARGET_CHANGED` fires at
/// the destroy's place, after the first packet's `UNIT_HEALTH` and ahead of the second's, and its
/// listener reads the selection the release left, nobody.
#[test]
fn a_destroyed_targets_change_fires_at_the_destroy() {
    let mut app = client();
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local g = CreateFrame("Frame")
            g:RegisterEvent("PLAYER_TARGET_CHANGED")
            g:SetScript("OnEvent", function()
                table.insert(Seen, event .. ":" .. tostring(UnitExists("target")))
            end)
            "#,
        )
        .unwrap();
    assert_eq!(
        frame(
            &mut app,
            vec![
                values(ME, &health(70)),
                SessionEvent::ObjectDestroyed(WOLF),
                values(ME, &health(60)),
            ]
        ),
        [
            "UNIT_HEALTH:player:70",
            "PLAYER_TARGET_CHANGED:nil",
            "UNIT_HEALTH:player:60"
        ]
    );
    assert_eq!(
        app.world().resource::<crate::target::Selection>().guid,
        None
    );
}

/// A reconnectable loss keeps the client in the world but drops every object with no release:
/// the selection ends there, as at the world's teardown, which returns before the swing stop and
/// the send (`0x4939df`), and the VM that survives it hears the change, as a release's teardown
/// signals (`0x493a73`). The target the reconnect streams in again is not selected, so nothing
/// names it as `"target"`.
#[test]
fn a_reconnect_does_not_reseat_the_lost_target() {
    let _lock = crate::local_state::test_env::ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _decl = crate::local_state::test_env::EnvGuard::set("WOW_UNATTENDED", "1");
    let mut app = client();
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local g = CreateFrame("Frame")
            g:RegisterEvent("PLAYER_TARGET_CHANGED")
            g:SetScript("OnEvent", function() table.insert(Seen, event) end)
            "#,
        )
        .unwrap();
    assert_eq!(
        frame(
            &mut app,
            vec![SessionEvent::Disconnected {
                reason: "lost".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        ),
        ["PLAYER_TARGET_CHANGED"],
        "the surviving VM hears the change"
    );
    let s = *app.world().resource::<crate::target::Selection>();
    assert_eq!(
        (s.guid, s.target, s.lit),
        (None, None, None),
        "ended at the loss"
    );
    assert_eq!(s.last, Some(WOLF));
    frame(
        &mut app,
        vec![created(
            WOLF,
            benilla_protocol::EntityKind::Unit,
            &health(100),
        )],
    );
    assert_eq!(
        app.world().resource::<crate::target::Selection>().guid,
        None
    );
    assert!(hosted::<bool>(
        &mut app,
        r#"return UnitExists("target") == nil"#
    ));
}

/// Nothing selected and the VM told so, with a spy on the selection's change and the boar's line:
/// each heard event, the change and the window with the target they read, `UnitHealth("target")`.
/// The line's listener is not asked: a lane's firing reads the world the frame left until each
/// packet's lanes fire inside the dispatch.
fn unselected_with_a_spy(app: &mut App) {
    select(app, None);
    frame(app, vec![]);
    app.world_mut()
        .non_send_resource::<UiScript>()
        .run(
            r#"
            local g = CreateFrame("Frame")
            g:RegisterEvent("PLAYER_TARGET_CHANGED")
            g:RegisterEvent("CHAT_MSG_MONSTER_SAY")
            g:RegisterEvent("LOOT_OPENED")
            g:SetScript("OnEvent", function()
                if event == "CHAT_MSG_MONSTER_SAY" then
                    table.insert(Seen, event)
                else
                    table.insert(Seen, event .. ":" .. tostring(UnitHealth("target")))
                end
            end)
            Seen = {}
            "#,
        )
        .unwrap();
}

/// The boar's line, which its own packet's handler shows on the chat lane.
fn boar_says() -> SessionEvent {
    SessionEvent::Chat(benilla_protocol::messages::ChatMessage {
        chat_type: benilla_protocol::messages::CHAT_MSG_MONSTER_SAY,
        language: 0,
        sender_guid: BOAR,
        target_guid: 0,
        sender_name: Some("Boar".into()),
        channel: None,
        text: "grr".into(),
        chat_tag: 0,
    })
}

/// The boar's line, its swing at us with nothing selected, and its line again: the swing's handler
/// selects the boar (`0x6259c9`-`0x6259fe`), so the change's `PLAYER_TARGET_CHANGED` fires at the
/// swing's place, between the lines' listeners, and reads the boar as the target.
#[test]
fn an_attackers_acquire_signals_at_its_swing() {
    let mut app = client();
    unselected_with_a_spy(&mut app);
    let swing = SessionEvent::AttackerState(benilla_protocol::messages::AttackerState {
        attacker: BOAR,
        victim: ME,
        hit_info: 0x2,
        damage: 3,
        victim_state: 1,
        absorb: 0,
        resist: 0,
        blocked: 0,
        melee_spell_id: 0,
        school: 0,
    });
    assert_eq!(
        frame(&mut app, vec![boar_says(), swing, boar_says()]),
        [
            "CHAT_MSG_MONSTER_SAY",
            "PLAYER_TARGET_CHANGED:50",
            "CHAT_MSG_MONSTER_SAY"
        ]
    );
    assert_eq!(
        app.world().resource::<crate::target::Selection>().guid,
        Some(BOAR)
    );
}

/// The boar's line, its cast at us with nothing selected, and its line again: the start's handler
/// selects the caster as its last act (`0x6e7963`-`0x6e799c`), so `PLAYER_TARGET_CHANGED` fires at
/// the start's place, between the lines' listeners.
#[test]
fn a_casters_acquire_signals_at_its_start() {
    const FROSTBOLT: u32 = 116;
    let mut app = client();
    app.insert_resource(crate::ui_action::Spells {
        catalog: benilla_formats::SpellCatalog::from_displays(std::collections::HashMap::from([(
            FROSTBOLT,
            benilla_formats::SpellDisplay::default(),
        )])),
        ..crate::ui_action::Spells::empty_for_tests()
    });
    unselected_with_a_spy(&mut app);
    let start = SessionEvent::SpellStart {
        caster: BOAR,
        spell_id: FROSTBOLT,
        cast_flags: 0,
        cast_time_ms: 3000,
        target: Some(ME),
        ammo_display_id: None,
    };
    assert_eq!(
        frame(&mut app, vec![boar_says(), start, boar_says()]),
        [
            "CHAT_MSG_MONSTER_SAY",
            "PLAYER_TARGET_CHANGED:50",
            "CHAT_MSG_MONSTER_SAY"
        ]
    );
}

/// The boar's line, then a loot response for the boar: the response selects the looted unit
/// (`0x5ebc35` → `0x48f3a0`) before the window opens (`0x4c1cb0`), so `PLAYER_TARGET_CHANGED`
/// fires at the response's place, after the line's listener and ahead of `LOOT_OPENED`.
#[test]
fn the_loot_responses_select_signals_at_the_response() {
    let mut app = client();
    unselected_with_a_spy(&mut app);
    let response = SessionEvent::LootResponse {
        guid: BOAR,
        // Pickpocketing: a server-started window, admitted with no latch.
        loot_type: 2,
        gold: 0,
        items: Vec::new(),
    };
    assert_eq!(
        frame(&mut app, vec![boar_says(), response]),
        [
            "CHAT_MSG_MONSTER_SAY",
            "PLAYER_TARGET_CHANGED:50",
            "LOOT_OPENED:50"
        ]
    );
}

/// The boar dead, selected and its loot open, then its line, the server's release of the loot and
/// its line again: the release closes the window (`0x5ec0f3` → `0x48f200`), whose last step
/// deselects a dead unit (`0x48f369` → `0x493910`), signalling inside it (`0x493a73`), so
/// `PLAYER_TARGET_CHANGED` fires at the release's place, between the lines' listeners.
#[test]
fn a_dead_units_loot_close_signals_its_deselect_at_the_packet() {
    let mut app = client();
    frame(&mut app, vec![values(BOAR, &health(0))]);
    unselected_with_a_spy(&mut app);
    select(&mut app, Some(BOAR));
    let world = app.world_mut();
    world
        .resource_mut::<crate::ui_loot::LootState>()
        .open(BOAR, 1, 0, Vec::new());
    world.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(BOAR);
    let release = SessionEvent::LootReleaseResponse {
        guid: BOAR,
        result: 1,
    };
    assert_eq!(
        frame(&mut app, vec![boar_says(), release, boar_says()]),
        [
            "CHAT_MSG_MONSTER_SAY",
            "PLAYER_TARGET_CHANGED:0",
            "CHAT_MSG_MONSTER_SAY"
        ]
    );
    assert_eq!(
        app.world().resource::<crate::target::Selection>().guid,
        None
    );
}

/// `CloseLoot()` on a dead unit's window: the close's last step deselects it (`0x4c2ed6` →
/// `0x48f200` → `0x48f369`), whose `PLAYER_TARGET_CHANGED` (`0x493a73`) runs its listeners inside
/// the call, before the line after it.
#[test]
fn close_loot_on_a_dead_unit_signals_its_deselect_inside_the_call() {
    let mut app = client();
    frame(&mut app, vec![values(BOAR, &health(0))]);
    unselected_with_a_spy(&mut app);
    select(&mut app, Some(BOAR));
    let world = app.world_mut();
    world
        .resource_mut::<crate::ui_loot::LootState>()
        .open(BOAR, 1, 0, Vec::new());
    world.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(BOAR);
    let seen: Vec<String> = hosted(
        &mut app,
        r#"CloseLoot() table.insert(Seen, "after") return Seen"#,
    );
    assert_eq!(seen, ["PLAYER_TARGET_CHANGED:0", "after"]);
    assert_eq!(
        app.world().resource::<crate::target::Selection>().guid,
        None
    );
}
