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
