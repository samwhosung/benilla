//! The mirror timers: breath, fatigue and feign-death edges off the wire, fired as
//! `MIRROR_TIMER_START`, `_PAUSE` and `_STOP` (`UIParent.lua:97`, `MirrorTimer.lua:73-74`) on the
//! notice lane at their packet, as the handler `0x5e7990` signals inside itself. The stock bars
//! integrate `value + scale * elapsed` every `OnUpdate`, so a packet every few seconds paints a
//! smooth countdown.
//!
//! The client computes nothing: vmangos's `Player::UpdateMirrorTimers` runs the timers off its own
//! liquid checks and ships a value and a signed rate, so there is no local underwater predicate to
//! disagree with the drowning damage that follows.

use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use benilla_protocol::messages::{MirrorTimerKind, MirrorTimerStart};

use crate::ui_action::Spells;

/// One mirror-timer edge off the wire, as the notice lane carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MirrorTimerEdge {
    /// `SMSG_START_MIRROR_TIMER`: starts or wholly restates a timer; vmangos re-sends it on every
    /// change of direction, remaining time or freeze.
    Start(MirrorTimerStart),
    /// `SMSG_PAUSE_MIRROR_TIMER`: vmangos never sends it, restating with a `Start` instead.
    Pause { kind: u32, paused: bool },
    /// `SMSG_STOP_MIRROR_TIMER`: the bar hides.
    Stop { kind: u32 },
}

impl MirrorTimerEdge {
    /// The wire's `timerType`.
    fn kind(&self) -> u32 {
        match *self {
            Self::Start(start) => start.kind,
            Self::Pause { kind, .. } | Self::Stop { kind } => kind,
        }
    }
}

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

/// `MIRROR_TIMER_START`'s arg6 (handler `0x5e7990`, label helper `0x5e7b10`): the owning spell's
/// localized name (`SpellRec + 0x1e0 + 4*locale`, by the packet's `spellId`, `0x5e7b37`), else
/// `"%s_LABEL"` (`0x860540`) of the timer's name read from the strings (`0x5e7b48`-`0x5e7b66`):
/// the 1.12 `GlobalStrings.lua` defines `BREATH_LABEL` and `EXHAUSTION_LABEL` and no
/// `FEIGNDEATH_LABEL`, which `GetText` (`0x703bf0`) answers with its empty string. A
/// water-breathing buff owns the breath timer (vmangos `UpdateMirrorTimers`,
/// `GetMirrorTimerBuff`), so the bar reads its name; the colour still keys off the timer.
/// `spell_name` is `None` for no spell and for one not in the catalog.
fn caption(
    kind: MirrorTimerKind,
    spell_name: Option<&str>,
    get: &dyn Fn(&str) -> Option<String>,
) -> String {
    match spell_name {
        Some(name) => name.to_string(),
        None => get(&format!("{}_LABEL", script_name(kind))).unwrap_or_default(),
    }
}

/// One edge's signal, as the handler raises it (`0x5e79cc`, `0x5e7a11`, `0x5e7a9a`), the owning
/// spell's name read off `spells`. An edge of a type with no bar never reaches the lane
/// ([`net`]).
pub(crate) fn fire_edge(script: &mut UiScript, edge: MirrorTimerEdge, spells: Option<&Spells>) {
    let Some(kind) = MirrorTimerKind::from_wire(edge.kind()) else {
        return;
    };
    // The VM has no spell-catalog binding, so the owning spell's name resolves here; 0 is none.
    let spell_name = |id: u32| -> Option<String> {
        (id != 0)
            .then(|| spells?.catalog.get(id).map(|d| d.name.clone()))
            .flatten()
    };
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
                ScriptValue::Str(caption(
                    kind,
                    spell_name(start.spell_id).as_deref(),
                    &|key| benilla_ui::strings::global(script.lua(), key),
                )),
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

/// `StopMirrorTimers` (`0x5e9b90`), which the active player's release runs (`0x5e9b55`): a
/// `MIRROR_TIMER_STOP` for each of types 0 to 2 in turn (`0x5e9ba5`), with no test of whether one
/// runs.
pub(crate) fn stop_all(lane: &mut crate::field_watch::NoticeLane) {
    for kind in 0..3 {
        lane.write(crate::field_watch::FieldNotice::MirrorTimer(
            MirrorTimerEdge::Stop { kind },
        ));
    }
}

/// The mirror timers' packet handlers, which put each edge on the notice lane at its packet.
mod net {
    use benilla_protocol::messages::MirrorTimerKind;
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::MirrorTimerEdge;
    use crate::field_watch::{FieldNotice, NoticeLane};
    use crate::net::NetHandlerApp;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::MirrorTimerStart, on_edge)
            .net_handler(K::MirrorTimerPause, on_edge)
            .net_handler(K::MirrorTimerStop, on_edge);
    }

    /// `0x5e7990`: the edge's signal, then for a start of type 0 or 1 its tutorial, 0x1a or 0x1c
    /// (`0x5e7ab0`, `0x5e7acd`), after the signal.
    ///
    /// Deviation: a type with no bar is dropped, because no bar appears either way: the reference
    /// fires it as `"UNKNOWN"` (`0x5e7ae0`) and `MirrorTimer_Show` errors on its nil colour before
    /// `Show()`. vmangos's `NUM_CLIENT_TIMERS` keeps its fourth timer off the wire.
    fn on_edge(In(ev): In<SessionEvent>, mut lane: ResMut<NoticeLane>) {
        let edge = match ev {
            SessionEvent::MirrorTimerStart(start) => MirrorTimerEdge::Start(start),
            SessionEvent::MirrorTimerPause { kind, paused } => {
                MirrorTimerEdge::Pause { kind, paused }
            }
            SessionEvent::MirrorTimerStop { kind } => MirrorTimerEdge::Stop { kind },
            _ => return,
        };
        let Some(kind) = MirrorTimerKind::from_wire(edge.kind()) else {
            return;
        };
        lane.write(FieldNotice::MirrorTimer(edge));
        let tutorial = match (edge, kind) {
            (MirrorTimerEdge::Start(_), MirrorTimerKind::Fatigue) => crate::tutorial::id::FATIGUE,
            (MirrorTimerEdge::Start(_), MirrorTimerKind::Breath) => crate::tutorial::id::BREATH,
            _ => return,
        };
        lane.write(FieldNotice::Tutorial { id: tutorial });
    }
}

pub(crate) struct UiMirrorPlugin;

impl Plugin for UiMirrorPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
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

    /// Marked stand-ins: the caption is the install's string under the timer's `_LABEL` key.
    fn labels(key: &str) -> Option<String> {
        match key {
            "EXHAUSTION_LABEL" => Some("[EXHAUSTION]".into()),
            "BREATH_LABEL" => Some("[BREATH]".into()),
            _ => None,
        }
    }

    /// `"%s_LABEL"` of the timer's name (`0x5e7b48`-`0x5e7b66`); feign death has no label global,
    /// so its caption is empty.
    #[test]
    fn a_spell_less_timer_captions_from_the_global_string() {
        assert_eq!(
            caption(MirrorTimerKind::Fatigue, None, &labels),
            "[EXHAUSTION]"
        );
        assert_eq!(caption(MirrorTimerKind::Breath, None, &labels), "[BREATH]");
        assert_eq!(caption(MirrorTimerKind::FeignDeath, None, &labels), "");
    }

    /// `0x5e7b10` takes the owning spell's record's name as it is (`0x5e7b37`) before the global
    /// string.
    #[test]
    fn an_owning_spell_captions_the_bar_with_its_own_name() {
        assert_eq!(
            caption(MirrorTimerKind::Breath, Some("Water Breathing"), &labels),
            "Water Breathing"
        );
        // An id the catalog cannot resolve falls back as a spell-less timer does.
        assert_eq!(caption(MirrorTimerKind::Breath, None, &labels), "[BREATH]");
    }
}
