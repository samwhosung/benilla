//! The mirror-timer feed: breath / fatigue / feign-death off the wire → FrameXML events
//! (decision 0874).
//!
//! The net bridge queues [`MirrorTimerEdge`]s and the drain fires the reference client's
//! FrameScript events into the script VM — `MIRROR_TIMER_START` / `_PAUSE` / `_STOP`, the exact
//! contract stock `Interface\FrameXML\MirrorTimer.xml` (`MirrorTimer1/2/3`) registers for.
//! The bars themselves are the reference's: the frame stores the value and integrates
//! `value + scale * elapsed` every OnUpdate, so a packet every few seconds is enough to paint a
//! smooth countdown.
//!
//! **The client computes nothing here.** Breath and fatigue are server state — vmangos's
//! `Player::UpdateMirrorTimers` runs them off its own liquid checks (`IsUnderwater`,
//! `IsInHighSea`) and ships a value + a signed rate. That is why there is no local "am I
//! underwater" predicate in this module and should not be one: a second, disagreeing authority
//! is exactly how a bar ends up drifting from the drowning damage that follows it.
//!
//! The `ui_cast::CastBarFeed` pattern throughout — one queue, one drain, no per-frame work.

use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use benilla_protocol::messages::{MirrorTimerKind, MirrorTimerStart};

use crate::ui_action::Spells;
use crate::ui_unit::UnitFeed;

/// One mirror-timer edge off the wire, queued by the net bridge for the bars.
#[derive(Debug, Clone, Copy)]
pub(crate) enum MirrorTimerEdge {
    /// `SMSG_START_MIRROR_TIMER` — start, or wholly re-state, one timer. The server re-sends this
    /// on every change (direction, remaining, frozen), so it arrives repeatedly for one bar.
    Start(MirrorTimerStart),
    /// `SMSG_PAUSE_MIRROR_TIMER` — freeze/unfreeze. vmangos never sends it (it substitutes a full
    /// `Start`), but a server that does must not be ignored.
    Pause { kind: u32, paused: bool },
    /// `SMSG_STOP_MIRROR_TIMER` — that timer is over; its bar hides.
    Stop { kind: u32 },
}

/// The net bridge's mirror-timer queue (the [`crate::ui_cast::CastBarFeed`] pattern).
#[derive(Resource, Default)]
pub(crate) struct MirrorTimerFeed(pub(crate) Vec<MirrorTimerEdge>);

/// The FrameScript name the reference passes as **arg1** for each timer type — the key its
/// `MirrorTimerColors` table is indexed by, and the stem of the caption lookup below.
///
/// The client holds these as a 3-entry table indexed by the wire's `timerType`
/// (`WoW.exe`: `"EXHAUSTION"` @`0x460520`, `"BREATH"` @`0x46052c`, `"FEIGNDEATH"` @`0x460534`,
/// contiguous and in the server's `MirrorTimer::Type` order). Note the type-0 name is
/// `EXHAUSTION`, not the server's own word for it (`FATIGUE`) — the two ends disagree on the
/// name of the same timer, and it is the *client's* word that the Lua is keyed by.
fn script_name(kind: MirrorTimerKind) -> &'static str {
    match kind {
        MirrorTimerKind::Fatigue => "EXHAUSTION",
        MirrorTimerKind::Breath => "BREATH",
        MirrorTimerKind::FeignDeath => "FEIGNDEATH",
    }
}

/// The static caption for a timer with **no owning spell** — the `GlobalStrings.lua` value the
/// reference's `<NAME>_LABEL` lookup resolves to.
///
/// The 1.12 `GlobalStrings.lua` defines exactly two of the three: `BREATH_LABEL = "Breath"` and
/// `EXHAUSTION_LABEL = "Fatigue"` (each commented "Used as the label for the … status bar"), and
/// **no** `FEIGNDEATH_LABEL`. The reference's `GetGlobalString 0x703bf0` never returns NULL — it
/// pre-seeds a static empty string (`0x882748`) — so the missing one is the **empty caption**, not
/// a nil the Lua would trip over.
///
/// Inlined here rather than looked up through the VM for the same reason `CastingBar.xml` inlines
/// `FAILED`/`INTERRUPTED`: benilla loads no `GlobalStrings.lua` yet.
fn global_string_label(kind: MirrorTimerKind) -> &'static str {
    match kind {
        MirrorTimerKind::Fatigue => "Fatigue",
        MirrorTimerKind::Breath => "Breath",
        MirrorTimerKind::FeignDeath => "",
    }
}

/// The bar's caption — **arg6** of `MIRROR_TIMER_START`, and it is **not** a fixed word.
///
/// Handler `0x5e7990` and its label helper `0x5e7b10`: the client tries the **owning spell's
/// localized name first** — `Spell.dbc` `SpellRec + 0x1e0 + 4*locale`, indexed by the START
/// packet's `spellId` — and only falls back to the `"<NAME>_LABEL"` global string when there is
/// no spell (`spellId == 0`).
///
/// That correction matters in play, and 0874 had it wrong: a water-breathing effect owns the
/// breath timer for its duration (vmangos `UpdateMirrorTimers` starts the timer from
/// `GetMirrorTimerBuff(type)` and passes `buff->GetId()`), so while it is up the bar is captioned
/// with **the spell's own name**, not "Breath". The bar's colour still keys off the timer name,
/// so only the word changes.
///
/// `spell_name` is the already-resolved catalog lookup (the `ui_cast` idiom: the script VM has no
/// spell-catalog binding, so the drain resolves it — one lookup face, decision 0107). `None`
/// covers both "no owning spell" and "spell not in the catalog"; the reference's fallback chain
/// ends at the global string either way.
fn caption(kind: MirrorTimerKind, spell_name: Option<&str>) -> String {
    spell_name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| global_string_label(kind))
        .to_string()
}

/// Drain the queue into the script VM, one FrameScript event per edge.
///
/// A `kind` the client has no bar for is **dropped**. The reference does fire the event for one —
/// its type→name switch (`0x5e7ae0`) answers `"UNKNOWN"` for anything outside 0..2 — but
/// `MirrorTimerColors["UNKNOWN"]` is nil, so `MirrorTimer_Show` errors at the colour read
/// *before* `dialog:Show()` and no bar ever appears. Dropping it is the same observable without
/// the Lua error, and vanilla never sends one anyway (the server's `NUM_CLIENT_TIMERS` gate keeps
/// its fourth timer, `ENVIRONMENTAL`, off the wire entirely).
fn feed_mirror_timers(
    script: Option<NonSendMut<UiScript>>,
    mut feed: ResMut<MirrorTimerFeed>,
    spells: Option<Res<Spells>>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    let Some(mut script) = script else {
        // No VM (a capture/headless run): drop the edges rather than let them pile up unbounded.
        feed.0.clear();
        return;
    };
    // The owning spell's name, resolved here because the script VM has no spell-catalog binding
    // (the `ui_cast` idiom, decision 0107). `0` = no spell, which is the common case.
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
        // The handler's two tutorial arms (`0x5e7ab0` type 0, `0x5e7acd` type 1; 1976).
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
            // **This edge reaches a reference body that raises**, and it is left that way on
            // purpose (1751 window 7). `MirrorTimerFrame_OnEvent` reads `arg1` as the timer name
            // and then as a number (`arg1 > 0`), comparing a string to a number — 1.12's own bug.
            // vmangos never sends the packet: it substitutes a full START and says so in
            // `Player::SendMirrorTimers`, which is presumably how the bug survived. If a server
            // ever does send one, the raise is the correct loud failure (1203) and the repair
            // belongs in an adapter over the reference's body, not in a second vocabulary here.
            MirrorTimerEdge::Pause { paused, .. } => (
                "MIRROR_TIMER_PAUSE",
                vec![name, ScriptValue::Int(i64::from(paused))],
            ),
            MirrorTimerEdge::Stop { .. } => ("MIRROR_TIMER_STOP", vec![name]),
        };
        // One line per edge. The bars live inside the script VM, so from outside it a mirror
        // timer is otherwise unobservable — and "did the server start a breath timer *here*?" is
        // the first question of any drowning / fatigue / liquid-hazard probe.
        debug!("net: mirror timer {event} {edge:?}");
        script.fire_event(event, args);
    }
}

/// The mirror-timer UI seam: the queue + its drain, ordered like the cast bar's — before the VM
/// ticks, so an edge and its first OnUpdate land on the same frame.
/// The mirror timers' packet handlers (decision 0874; in the net handler table since 2313):
/// breath / fatigue / feign-death. Pure queue handlers — every meaning (which bar, what colour,
/// what caption, how fast it drains) is resolved at the UI seam in this module, and the
/// countdown itself is the FrameXML's own OnUpdate integration.
mod net {
    use benilla_protocol::{SessionEvent, SessionEventKind};
    use bevy::prelude::*;

    use super::{MirrorTimerEdge, MirrorTimerFeed};
    use crate::net::NetHandlerApp;

    /// Register the handlers — called from [`super::UiMirrorPlugin`].
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

    /// The client's arg1 names, in the server's `MirrorTimer::Type` order — and the fact that
    /// type 0 is `EXHAUSTION` on the client but `FATIGUE` on the server. Getting this wrong is
    /// silent: `MirrorTimerColors[timer]` would be nil and the bar's colour read would error.
    #[test]
    fn arg1_is_the_clients_name_not_the_servers() {
        assert_eq!(script_name(MirrorTimerKind::Fatigue), "EXHAUSTION");
        assert_eq!(script_name(MirrorTimerKind::Breath), "BREATH");
        assert_eq!(script_name(MirrorTimerKind::FeignDeath), "FEIGNDEATH");
    }

    /// With no owning spell the caption is the 1.12 `GlobalStrings.lua` value the `<NAME>_LABEL`
    /// lookup resolves to — and the feign-death one is empty because that global does not exist
    /// (the reference's `GetGlobalString` hands back a static empty string, never nil).
    #[test]
    fn a_spell_less_timer_captions_from_the_global_string() {
        assert_eq!(caption(MirrorTimerKind::Fatigue, None), "Fatigue");
        assert_eq!(caption(MirrorTimerKind::Breath, None), "Breath");
        assert_eq!(caption(MirrorTimerKind::FeignDeath, None), "");
    }

    /// The correction 0874 got wrong (`0x5e7b10`): the client tries the OWNING
    /// SPELL's localized name first and only falls back to the global string. A water-breathing
    /// effect owns the breath timer while it is up, so the bar reads with the spell's name.
    #[test]
    fn an_owning_spell_captions_the_bar_with_its_own_name() {
        assert_eq!(
            caption(MirrorTimerKind::Breath, Some("Water Breathing")),
            "Water Breathing"
        );
        // An id the catalog can't resolve falls back exactly as a spell-less timer does.
        assert_eq!(caption(MirrorTimerKind::Breath, None), "Breath");
        assert_eq!(caption(MirrorTimerKind::Breath, Some("")), "Breath");
    }
}
