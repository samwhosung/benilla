//! The mirror-timer feed: breath, fatigue and feign-death edges off the wire, fired as
//! `MIRROR_TIMER_START`, `_PAUSE` and `_STOP` (`UIParent.lua:97`, `MirrorTimer.lua:73-74`). The
//! stock bars integrate `value + scale * elapsed` every `OnUpdate`, so a packet every few seconds
//! paints a smooth countdown.
//!
//! The client computes nothing: vmangos's `Player::UpdateMirrorTimers` runs the timers off its own
//! liquid checks and ships a value and a signed rate, so there is no local underwater predicate to
//! disagree with the drowning damage that follows.

use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use benilla_protocol::messages::{MirrorTimerKind, MirrorTimerStart};

use crate::ui_action::Spells;
use crate::ui_unit::UnitFeed;

/// One mirror-timer edge off the wire, queued by the net bridge for the bars.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MirrorTimerEdge {
    /// `SMSG_START_MIRROR_TIMER`: starts or wholly restates a timer; vmangos re-sends it on every
    /// change of direction, remaining time or freeze.
    Start(MirrorTimerStart),
    /// `SMSG_PAUSE_MIRROR_TIMER`: vmangos never sends it, restating with a `Start` instead.
    Pause { kind: u32, paused: bool },
    /// `SMSG_STOP_MIRROR_TIMER`: the bar hides.
    Stop { kind: u32 },
}

/// The net bridge's mirror-timer queue.
#[derive(Resource, Default)]
pub(crate) struct MirrorTimerFeed(pub(crate) Vec<MirrorTimerEdge>);

/// The timer's arg1 and `MirrorTimerColors` key, from the client's 3-entry table by wire type
/// (`"EXHAUSTION"` `0x460520`, `"BREATH"` `0x46052c`, `"FEIGNDEATH"` `0x460534`). Type 0 is
/// `EXHAUSTION` on the client and `FATIGUE` on the server; the Lua keys on the client's word.
fn script_name(kind: MirrorTimerKind) -> &'static str {
    match kind {
        MirrorTimerKind::Fatigue => "EXHAUSTION",
        MirrorTimerKind::Breath => "BREATH",
        MirrorTimerKind::FeignDeath => "FEIGNDEATH",
    }
}

/// A spell-less timer's caption, the reference's `<NAME>_LABEL` lookup: the 1.12
/// `GlobalStrings.lua` defines `BREATH_LABEL` and `EXHAUSTION_LABEL` and no `FEIGNDEATH_LABEL`,
/// which `GetGlobalString` (`0x703bf0`) answers with its static empty string (`0x882748`), never
/// nil. These are the enUS values inlined; the reference reads the install's.
fn global_string_label(kind: MirrorTimerKind) -> &'static str {
    match kind {
        MirrorTimerKind::Fatigue => "Fatigue",
        MirrorTimerKind::Breath => "Breath",
        MirrorTimerKind::FeignDeath => "",
    }
}

/// `MIRROR_TIMER_START`'s arg6 (handler `0x5e7990`, label helper `0x5e7b10`): the owning spell's
/// localized name (`SpellRec + 0x1e0 + 4*locale`, by the packet's `spellId`), else the
/// `<NAME>_LABEL` string. A water-breathing buff owns the breath timer (vmangos
/// `UpdateMirrorTimers`, `GetMirrorTimerBuff`), so the bar reads its name; the colour still keys
/// off the timer. `spell_name` is `None` for no spell and for one not in the catalog.
fn caption(kind: MirrorTimerKind, spell_name: Option<&str>) -> String {
    spell_name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| global_string_label(kind))
        .to_string()
}

/// Drains the queue into the VM, one event per edge.
///
/// Deviation: a type with no bar is dropped, because no bar appears either way: the reference
/// fires it as `"UNKNOWN"` (`0x5e7ae0`) and `MirrorTimer_Show` errors on its nil colour before
/// `Show()`. vmangos's `NUM_CLIENT_TIMERS` keeps its fourth timer off the wire.
fn feed_mirror_timers(
    script: Option<NonSendMut<UiScript>>,
    mut feed: ResMut<MirrorTimerFeed>,
    spells: Option<Res<Spells>>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    let Some(mut script) = script else {
        // No VM (a capture or headless run): drop the edges so the queue stays bounded.
        feed.0.clear();
        return;
    };
    // The VM has no spell-catalog binding, so the owning spell's name resolves here; 0 is none.
    let spell_name = |id: u32| -> Option<String> {
        (id != 0)
            .then(|| spells.as_ref()?.catalog.get(id).map(|d| d.name.clone()))
            .flatten()
    };
    for edge in feed.0.drain(..) {
        let raw = match edge {
            MirrorTimerEdge::Start(start) => start.kind,
            MirrorTimerEdge::Pause { kind, .. } | MirrorTimerEdge::Stop { kind } => kind,
        };
        let Some(kind) = MirrorTimerKind::from_wire(raw) else {
            continue;
        };
        // The handler's two tutorial arms: type 0 (`0x5e7ab0`) and type 1 (`0x5e7acd`).
        if matches!(edge, MirrorTimerEdge::Start(_)) {
            match kind {
                MirrorTimerKind::Fatigue => {
                    if let Some(t) = tutorials.as_mut() {
                        t.write(crate::tutorial::TutorialEvent::trigger(
                            crate::tutorial::id::FATIGUE,
                        ));
                    }
                }
                MirrorTimerKind::Breath => {
                    if let Some(t) = tutorials.as_mut() {
                        t.write(crate::tutorial::TutorialEvent::trigger(
                            crate::tutorial::id::BREATH,
                        ));
                    }
                }
                _ => {}
            }
        }
        let name = ScriptValue::Str(script_name(kind).into());
        let (event, args): (&str, Vec<ScriptValue>) = match edge {
            MirrorTimerEdge::Start(start) => (
                "MIRROR_TIMER_START",
                vec![
                    name,
                    ScriptValue::Int(i64::from(start.remaining_ms)),
                    ScriptValue::Int(i64::from(start.duration_ms)),
                    ScriptValue::Int(i64::from(start.scale)),
                    ScriptValue::Int(i64::from(start.paused)),
                    ScriptValue::Str(caption(kind, spell_name(start.spell_id).as_deref())),
                ],
            ),
            // The stock handler raises on this: it matches `arg1` as the timer name, then tests
            // `arg1 > 0` (`MirrorTimer.lua:84-88`), 1.12's own bug. vmangos never sends it
            // (`Player::SendMirrorTimers`); a repair belongs over the stock body, not here.
            MirrorTimerEdge::Pause { paused, .. } => (
                "MIRROR_TIMER_PAUSE",
                vec![name, ScriptValue::Int(i64::from(paused))],
            ),
            MirrorTimerEdge::Stop { .. } => ("MIRROR_TIMER_STOP", vec![name]),
        };
        // One log line per edge: the bars live inside the VM, so this is their only outside trace.
        debug!("net: mirror timer {event} {edge:?}");
        script.fire_event(event, args);
    }
}

/// The mirror timers' packet handlers, which only queue. The drain runs before the VM ticks, so
/// an edge and its first `OnUpdate` land on the same frame.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{MirrorTimerEdge, MirrorTimerFeed};
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::MirrorTimerStart, on_edge)
            .net_handler(K::MirrorTimerPause, on_edge)
            .net_handler(K::MirrorTimerStop, on_edge);
    }

    fn on_edge(In(ev): In<SessionEvent>, mut feed: ResMut<MirrorTimerFeed>) {
        let edge = match ev {
            SessionEvent::MirrorTimerStart(start) => MirrorTimerEdge::Start(start),
            SessionEvent::MirrorTimerPause { kind, paused } => {
                MirrorTimerEdge::Pause { kind, paused }
            }
            SessionEvent::MirrorTimerStop { kind } => MirrorTimerEdge::Stop { kind },
            _ => return,
        };
        feed.0.push(edge);
    }
}

pub(crate) struct UiMirrorPlugin;

impl Plugin for UiMirrorPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<MirrorTimerFeed>()
            .add_systems(Update, feed_mirror_timers.in_set(UnitFeed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wrong name leaves `MirrorTimerColors[timer]` nil and the bar's colour read errors.
    #[test]
    fn arg1_is_the_clients_name_not_the_servers() {
        assert_eq!(script_name(MirrorTimerKind::Fatigue), "EXHAUSTION");
        assert_eq!(script_name(MirrorTimerKind::Breath), "BREATH");
        assert_eq!(script_name(MirrorTimerKind::FeignDeath), "FEIGNDEATH");
    }

    /// Feign death has no label global, so its caption is empty.
    #[test]
    fn a_spell_less_timer_captions_from_the_global_string() {
        assert_eq!(caption(MirrorTimerKind::Fatigue, None), "Fatigue");
        assert_eq!(caption(MirrorTimerKind::Breath, None), "Breath");
        assert_eq!(caption(MirrorTimerKind::FeignDeath, None), "");
    }

    /// `0x5e7b10` tries the owning spell's name before the global string.
    #[test]
    fn an_owning_spell_captions_the_bar_with_its_own_name() {
        assert_eq!(
            caption(MirrorTimerKind::Breath, Some("Water Breathing")),
            "Water Breathing"
        );
        // An id the catalog cannot resolve falls back as a spell-less timer does.
        assert_eq!(caption(MirrorTimerKind::Breath, None), "Breath");
        assert_eq!(caption(MirrorTimerKind::Breath, Some("")), "Breath");
    }
}
