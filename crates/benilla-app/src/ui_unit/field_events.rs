//! The interface events of the field watches ([`crate::field_watch`]): each notice the object
//! packets wrote this frame fires its callback's signals, in wire order, after every snapshot those
//! events' handlers read is pushed ([`FieldEventPushes`]) and the unit tokens are seated
//! ([`crate::ui_aura::AuraEvents`]).

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::field_watch::FieldNotice;

/// The feeds that push what a field event's handlers read: the XP and rest values (`feed_units`),
/// the combat stats of the player and of the pet, the honor block and the watched faction. The
/// firing runs after all of them, as the reference writes a whole block before its notify pass.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FieldEventPushes;

/// Fire this frame's field notices in the order the packets wrote them.
pub(super) fn fire_field_notices(
    script: Option<NonSendMut<UiScript>>,
    mut notices: MessageReader<FieldNotice>,
    mut sink: crate::ui_action::MessageSink,
) {
    let Some(mut script) = script else {
        return;
    };
    for &notice in notices.read() {
        fire(&mut script, &mut sink, notice);
    }
}

/// One callback's signals.
fn fire(script: &mut UiScript, sink: &mut crate::ui_action::MessageSink, notice: FieldNotice) {
    match notice {
        FieldNotice::Unit { guid, id } => fire_unit(script, guid, id),
        FieldNotice::Player { id } => fire_player(script, id),
        FieldNotice::RestState { old, new } => {
            // `0x5de4e0`: the signal, then the line for the new state.
            script.fire_event("UPDATE_EXHAUSTION", vec![]);
            let line = super::rest_state_message(old, new)
                .and_then(|key| crate::ui_action::keyed_line(script, key));
            crate::ui_action::show_messages(script, sink, "ui_unit", line);
        }
        FieldNotice::CombatSkill => {
            // `0x5ed990`, in its order (`0x5ed99f`, `0x5ed9b3`, `0x5ed9c7`).
            script.fire_event("UNIT_ATTACK", vec![super::player_token()]);
            script.fire_event("UNIT_DEFENSE", vec![super::player_token()]);
            script.fire_event("UNIT_RANGED_ATTACK_POWER", vec![super::player_token()]);
        }
        FieldNotice::Weapon { slot } => {
            let event = match slot {
                // `0x5ed970` (`0x5deed9`, `0x5deee4`).
                15 | 16 => "UNIT_ATTACK",
                // `0x5deecb`.
                17 => "UNIT_RANGED_ATTACK_POWER",
                _ => return,
            };
            script.fire_event(event, vec![super::player_token()]);
        }
    }
}

/// The unit-field bridge `0x51bd50` → `0x515e50`: the event named by the field, once per token
/// naming the unit, `arg1` the token. A unit with no token flags has no watchers (`0x60c4a0`).
fn fire_unit(script: &mut UiScript, guid: u64, id: u16) {
    let Some(event) = bridge_event(id) else {
        return;
    };
    if !script.unit_has_field_watchers(guid) {
        return;
    }
    for token in script.unit_tokens_of(guid) {
        script.fire_event(event, vec![ScriptValue::Str(token)]);
    }
}

/// `0x5de4b0`: the event its registration names, `arg1 = "player"`.
fn fire_player(script: &mut UiScript, id: u16) {
    let Some(event) = player_event(id) else {
        return;
    };
    script.fire_event(event, vec![super::player_token()]);
}

/// The event-table name of a bridge id ([`crate::field_watch::BRIDGE_IDS`]), as the table the
/// static initializers write at `0x51ab30`-`0x51acb9` gives it.
fn bridge_event(id: u16) -> Option<&'static str> {
    Some(match id {
        120 | 121 => "UNIT_ATTACK_SPEED",
        122 | 165 | 166 => "UNIT_RANGEDDAMAGE",
        128..=131 => "UNIT_DAMAGE",
        144..=148 => "UNIT_STATS",
        149..=155 => "UNIT_RESISTANCES",
        159..=161 => "UNIT_ATTACK_POWER",
        162..=164 => "UNIT_RANGED_ATTACK_POWER",
        _ => return None,
    })
}

/// The event-table name of an id `0x5dd8a0` registers `0x5de4b0` with.
fn player_event(id: u16) -> Option<&'static str> {
    Some(match id {
        149 => "UNIT_RESISTANCES",
        189 => "PLAYER_XP_UPDATE",
        201 => "PLAYER_DAMAGE_DONE_MODS",
        372 => "UPDATE_FACTION",
        406 => "UPDATE_EXHAUSTION",
        523 => "PLAYER_PVP_KILLS_CHANGED",
        524 => "PLAYER_PVP_RANK_CHANGED",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    const ME: u64 = 0x07;
    const PET: u64 = 0xF140_0000_0000_0077;
    const MOB: u64 = 0xF130_0000_0000_0001;

    /// The event table, id to name (`reference/events.tsv`).
    fn event_table() -> std::collections::HashMap<u16, String> {
        let tsv = concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference/events.tsv");
        let text = std::fs::read_to_string(tsv).expect("reference/events.tsv");
        text.lines()
            .skip(1)
            .filter_map(|l| {
                let mut f = l.split('\t');
                Some((f.next()?.parse().ok()?, f.next()?.to_string()))
            })
            .collect()
    }

    /// Each watched id fires the event the reference's table names for it.
    #[test]
    fn each_id_fires_the_event_the_table_names() {
        let table = event_table();
        for id in crate::field_watch::BRIDGE_IDS {
            assert_eq!(
                bridge_event(id),
                table.get(&id).map(String::as_str),
                "bridge id {id}"
            );
        }
        for id in [149, 189, 201, 372, 406, 523, 524] {
            assert_eq!(
                player_event(id),
                table.get(&id).map(String::as_str),
                "player id {id}"
            );
        }
    }

    /// A world whose VM records every event the notices fire, with `guids` seated.
    fn world(guids: benilla_ui::script::UnitGuids) -> App {
        let mut app = App::new();
        app.add_message::<FieldNotice>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>();
        let mut script = UiScript::new().unwrap();
        script
            .run(
                r#"
                SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterAllEvents()
                f:SetScript("OnEvent", function()
                    table.insert(SEEN, event .. " " .. tostring(arg1))
                end)
            "#,
            )
            .unwrap();
        script.set_unit_guids(&guids);
        app.insert_non_send_resource(script);
        app
    }

    fn fired(app: &mut App, notices: &[FieldNotice]) -> Vec<String> {
        app.world_mut()
            .resource_mut::<Messages<FieldNotice>>()
            .write_batch(notices.iter().copied());
        app.world_mut().run_system_once(fire_field_notices).unwrap();
        let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
        s.resolve();
        let out = s.eval::<Vec<String>>("return SEEN").unwrap();
        s.run("SEEN = {}").unwrap();
        out
    }

    fn seated() -> benilla_ui::script::UnitGuids {
        benilla_ui::script::UnitGuids {
            player: ME,
            pet: PET,
            target: ME,
            mouseover: MOB,
            ..Default::default()
        }
    }

    /// The bridge fires once per token naming the unit, in the fan-out's order; a unit with no
    /// token flags fires nothing, the mouseover alone included.
    #[test]
    fn the_bridge_fires_once_per_token_naming_the_unit() {
        let mut app = world(seated());
        assert_eq!(
            fired(
                &mut app,
                &[
                    FieldNotice::Unit { guid: ME, id: 144 },
                    FieldNotice::Unit { guid: ME, id: 145 },
                    FieldNotice::Unit { guid: PET, id: 128 },
                    FieldNotice::Unit { guid: MOB, id: 144 },
                ]
            ),
            [
                "UNIT_STATS player",
                "UNIT_STATS target",
                "UNIT_STATS player",
                "UNIT_STATS target",
                "UNIT_DAMAGE pet",
            ]
        );
    }

    /// Each callback's own signals, in the order the notices came.
    #[test]
    fn each_callback_fires_its_own_signals_in_notice_order() {
        let mut app = world(seated());
        assert_eq!(
            fired(
                &mut app,
                &[
                    FieldNotice::RestState { old: 1, new: 2 },
                    FieldNotice::Player { id: 189 },
                    FieldNotice::CombatSkill,
                    FieldNotice::Weapon { slot: 16 },
                    FieldNotice::Weapon { slot: 17 },
                    FieldNotice::Player { id: 406 },
                ]
            ),
            [
                "UPDATE_EXHAUSTION nil",
                "PLAYER_XP_UPDATE player",
                "UNIT_ATTACK player",
                "UNIT_DEFENSE player",
                "UNIT_RANGED_ATTACK_POWER player",
                "UNIT_ATTACK player",
                "UNIT_RANGED_ATTACK_POWER player",
                "UPDATE_EXHAUSTION player",
            ]
        );
    }
}
