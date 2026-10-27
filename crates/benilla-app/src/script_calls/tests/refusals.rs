//! A verb's refusal shows inside its call: `DisplayError` (`0x496720`) signals
//! `UI_ERROR_MESSAGE` and returns before the verb does, so the chunk's next line reads the line.

use super::*;

/// What the chunk's line after the verb saw of the lines shown: the red lines
/// ([`record_reports`]) and the chat surface's system lines.
fn shown_in_call(f: &mut Frame, lua: &str) -> String {
    run(
        f,
        r#"local tap = CreateFrame("Frame")
        tap:RegisterEvent("CHAT_MSG_SYSTEM")
        tap:SetScript("OnEvent", function() table.insert(LINES, arg1) end)"#,
    );
    run(f, &format!(r#"{lua} SEEN = table.concat(LINES, ",")"#));
    eval(f, "return SEEN")
}

/// The VM knows the player by name, as a chat line needs.
fn in_world(f: &mut Frame) {
    f.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .set_unit(
            "player",
            Some(benilla_ui::script::UnitState {
                exists: true,
                name: Some("Probe".into()),
                ..Default::default()
            }),
        );
}

/// `SendChatMessage`'s group gate (`0x49f351` → `0x496720`, message `0x48`) and its dead check
/// (`0x49f5df`, `0xef`) refuse inside the call.
#[test]
fn a_chat_lines_refusal_shows_inside_its_call() {
    let mut f = frame(true);
    in_world(&mut f);
    // No party slot held in the world, which the gate reads at the call.
    f.app
        .world_mut()
        .resource_mut::<crate::ui_party::GroupState>()
        .members
        .clear();
    assert_eq!(
        shown_in_call(&mut f, r#"SendChatMessage("hi", "PARTY")"#),
        "ERR_NOT_IN_GROUP"
    );
    let mut f = frame(true);
    in_world(&mut f);
    let world = f.app.world_mut();
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .get_mut::<ObjectStore>(me)
        .unwrap()
        .0
        .merge(benilla_protocol::ObjectFields::from_pairs(&[(HEALTH, 0)]));
    assert_eq!(
        shown_in_call(&mut f, r#"SendChatMessage("hi", "SAY")"#),
        "ERR_CHAT_WHILE_DEAD"
    );
}

/// `DoEmote`'s stand-still arm (`0x5ef5e0` → `0x496720`, message `0x139`) refuses a moving,
/// self-controlled player inside the call.
#[test]
fn an_emote_refused_for_moving_shows_inside_its_call() {
    const WAVE: u32 = 101;
    const WAVE_EMOTE: u32 = 3;
    let mut f = frame(true);
    let world = f.app.world_mut();
    world.insert_resource(crate::sound::EmoteSounds(
        benilla_formats::EmoteSoundCatalog::default()
            .with_text_emote("WAVE", WAVE, WAVE_EMOTE)
            // `EmoteFlags 0x4000`, the stand-still flag `0x47db40` reports on.
            .with_row(WAVE_EMOTE, 0, 0x4000),
    ));
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(crate::creature_anim::MovementState {
            flags: crate::creature_anim::move_flags::FORWARD,
            ..Default::default()
        });
    assert_eq!(
        shown_in_call(&mut f, r#"DoEmote("WAVE")"#),
        "ERR_NOEMOTEWHILERUNNING"
    );
    assert!(
        !f.rx
            .try_iter()
            .any(|c| matches!(c, ClientCommand::TextEmote { .. })),
        "no emote is sent"
    );
}

/// A probe's emote line runs `DoEmote`'s engine half (`0x5ef560`), its refusal shown inside it,
/// with the host lent, as inside `DoEmote`'s call: the line's listener's call applies.
#[test]
fn a_probe_emote_lines_refusal_runs_its_listener_with_the_host_lent() {
    const WAVE: u32 = 101;
    const WAVE_EMOTE: u32 = 3;
    let mut f = frame(true);
    let world = f.app.world_mut();
    world.insert_resource(crate::sound::EmoteSounds(
        benilla_formats::EmoteSoundCatalog::default()
            .with_text_emote("WAVE", WAVE, WAVE_EMOTE)
            .with_row(WAVE_EMOTE, 0, 0x4000),
    ));
    let me = world.resource::<GuidIndex>().0[&ME];
    world
        .entity_mut(me)
        .insert(crate::creature_anim::MovementState {
            flags: crate::creature_anim::move_flags::FORWARD,
            ..Default::default()
        });
    probe_line_world(world);
    world.insert_resource(crate::ui_chat::commands::SlashCommands::build(
        |name| match name {
            "EMOTE1_CMD1" => Some("/wave".to_string()),
            "EMOTE1_TOKEN" => Some("WAVE".to_string()),
            _ => None,
        },
        |token| (token == "WAVE").then_some(WAVE),
    ));
    run(
        &mut f,
        r#"local roll = CreateFrame("Frame")
        roll:RegisterEvent("UI_ERROR_MESSAGE")
        roll:SetScript("OnEvent", function() RandomRoll(1, 100) end)"#,
    );
    f.rx.try_iter().for_each(drop);
    let world = f.app.world_mut();
    world
        .non_send_resource_mut::<UiScript>()
        .push_chat_input("/wave".into());
    crate::ui_chat::drain_chat_input_once(world);
    assert_eq!(errors(&f), ["ERR_NOEMOTEWHILERUNNING"]);
    let wire: Vec<String> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
                ClientCommand::TextEmote { .. } => Some("TextEmote".to_string()),
                _ => None,
            })
            .collect();
    assert_eq!(wire, ["RandomRoll"]);
}

/// `SetRaidTarget` on a unit it may not mark (`0x4bbfd2` → `0x496720`, message `0x1c5`), an enemy
/// player, refuses inside the call.
#[test]
fn a_raid_mark_refused_shows_inside_its_call() {
    let mut f = frame(true);
    // `OLD`, selected, a player of the enemy's template.
    with_factions(&mut f, FOE, &[(2, 0x19)]);
    f.app
        .world_mut()
        .non_send_resource_mut::<UiScript>()
        .seat_selection(Some(benilla_ui::script::SeatedSelection {
            guid: OLD,
            held: true,
            target: 0,
            target_held: false,
        }));
    let seen = shown_in_call(&mut f, r#"SetRaidTarget("target", 1)"#);
    assert_eq!(seen, "ERR_INVALID_RAID_TARGET");
}

/// `JoinChannelByName` past ten slots: the join body's claim (`0x49ede8` → `0x49b9c5`) shows
/// `ERR_TOO_MANY_CHAT_CHANNELS` (message `0x199`) inside the call, and the body sends the join
/// after it all the same (`0x49edf3`, `0x49ee02`), so the line's listener's packet goes out first.
#[test]
fn a_join_past_ten_channels_shows_its_refusal_inside_the_call_before_the_send() {
    let mut f = frame(true);
    in_world(&mut f);
    for i in 1..=10 {
        run(&mut f, &format!("JoinChannelByName('Chan{i}')"));
    }
    f.rx.try_iter().for_each(drop);
    run(
        &mut f,
        r#"local roll = CreateFrame("Frame")
        roll:RegisterEvent("CHAT_MSG_SYSTEM")
        roll:SetScript("OnEvent", function() RandomRoll(1, 100) end)"#,
    );
    assert_eq!(
        shown_in_call(&mut f, "JoinChannelByName('Eleventh')"),
        "ERR_TOO_MANY_CHAT_CHANNELS"
    );
    let wire: Vec<String> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
                ClientCommand::JoinChannel { name, .. } => Some(format!("JoinChannel {name}")),
                _ => None,
            })
            .collect();
    assert_eq!(wire, ["RandomRoll", "JoinChannel Eleventh"]);
}

/// The attack validator's refusal shows inside the call, after what the validator sent before
/// it: with the targeting cursor up, the nearest enemy its scan picks is bound to the cast
/// (`0x4941e6`), the friend stays selected, and the final gate refuses it (`0x613171`), so the
/// cast goes out ahead of the line's listener's packet, and the chunk's next line reads the line.
#[test]
fn an_attacks_refusal_shows_inside_its_call_after_the_cast_it_bound() {
    let mut f = frame(true);
    with_factions(&mut f, FRIEND, &[]);
    spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
    with_the_cursor_up(&mut f);
    run(
        &mut f,
        r#"local roll = CreateFrame("Frame")
        roll:RegisterEvent("UI_ERROR_MESSAGE")
        roll:SetScript("OnEvent", function() RandomRoll(1, 100) end)"#,
    );
    f.rx.try_iter().for_each(drop);
    run_pressed(&mut f, r#"AttackTarget() SEEN = table.concat(LINES, ",")"#);
    assert_eq!(
        eval::<String>(&mut f, "return SEEN"),
        "ERR_INVALID_ATTACK_TARGET"
    );
    let wire: Vec<String> =
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { target, .. } => Some(format!("CastSpell {target:x?}")),
                ClientCommand::RandomRoll { .. } => Some("RandomRoll".to_string()),
                ClientCommand::AttackSwing { .. } => Some("AttackSwing".to_string()),
                _ => None,
            })
            .collect();
    assert_eq!(
        wire,
        [format!("CastSpell {:x?}", Some(MOB)), "RandomRoll".into()]
    );
}

/// `SendChatMessage`'s raid gates (`0x49f379`, `0x49f3a4`), chat rows like the party gate: a raid
/// line with no raid is `ERR_NOT_IN_RAID` (`0x19c`), and a raid warning from a plain member
/// `ERR_NOT_LEADER` (`0x4b`), each inside the call.
#[test]
fn a_raid_lines_refusal_shows_inside_its_call() {
    let mut f = frame(true);
    in_world(&mut f);
    assert_eq!(
        shown_in_call(&mut f, r#"SendChatMessage("hi", "RAID")"#),
        "ERR_NOT_IN_RAID"
    );
    let mut f = frame(true);
    in_world(&mut f);
    // The world's raid, as the gate reads it at the call: Bob leads, we hold rank 0.
    {
        let mut group = f
            .app
            .world_mut()
            .resource_mut::<crate::ui_party::GroupState>();
        group.group_type = crate::ui_party::GROUPTYPE_RAID;
        group.leader = NEW;
        group.rebuild_raid(ME);
    }
    assert_eq!(
        shown_in_call(&mut f, r#"SendChatMessage("hi", "RAID_WARNING")"#),
        "ERR_NOT_LEADER"
    );
}

/// The party verbs' own refusals, `DisplayError` (`0x496720`) inside the binding: `InviteByName`
/// of our own name (`0x48a485`), `SetLootMethod("master")` with no name (`0x4e9453`), and
/// `DoReadyCheck` while one runs (`0x4bb20c`).
#[test]
fn a_party_verbs_own_refusal_shows_inside_its_call() {
    let mut f = frame(true);
    {
        let world = f.app.world_mut();
        let mut script = world.non_send_resource_mut::<UiScript>();
        // The player's object is held (`0x48a460`), so the invite compares names.
        script.set_unit_guids(&benilla_ui::script::UnitGuids {
            player: ME,
            ..Default::default()
        });
        script.set_unit(
            "player",
            Some(benilla_ui::script::UnitState {
                exists: true,
                guid: ME,
                name: Some("Probe".into()),
                ..Default::default()
            }),
        );
        // The frame seats Bob in `party1`; we lead, as the gates read the world's group.
        world.resource_mut::<crate::ui_party::GroupState>().leader = ME;
        // Our name as `GetUnitName` reads it off the name cache at the call (`0x48a473`).
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(ME, "Probe".into(), None);
    }
    assert_eq!(
        shown_in_call(&mut f, r#"InviteByName("probe")"#),
        "ERR_INVITE_SELF"
    );
    run(&mut f, "LINES = {}");
    assert_eq!(
        shown_in_call(&mut f, r#"SetLootMethod("master")"#),
        "ERR_SPECIFY_MASTER_LOOTER"
    );
    run(&mut f, "DoReadyCheck() LINES = {}");
    assert_eq!(
        shown_in_call(&mut f, "DoReadyCheck()"),
        "ERR_READY_CHECK_IN_PROGRESS"
    );
}
