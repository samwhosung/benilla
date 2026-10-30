//! Our own cast's reports, in the order the reference signals them: the refused cast's line
//! (`ReportCastFailure 0x6e1a00`), the wire edges that become the `SPELLCAST_*` events stock
//! `CastingBarFrame.lua:6-13` registers for, named from `Spell.dbc`, and the tradeskill repeat's
//! disarm. A script call's reports are signalled inside the call ([`crate::script_calls`]), the
//! rest from their lane in the packets' order ([`fire_cast_reports`]), where the getters read the
//! cast state each packet left ([`CastLive`]). The cast lifecycle is `crate::spell`'s.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::ui_action::{CastFail, CastFailLines, MessageSink, Shown, Spells};

mod views;
pub(crate) use views::{slot_moves_here, CastLive, CastViews};

/// One edge of our own cast, queued by the spell's packet handlers, its reports and its cancels.
pub(crate) enum CastBarEdge {
    /// `SMSG_SPELL_START` with a nonzero cast time; an instant opens no bar, as in the reference.
    Start { spell_id: u32, cast_time_ms: u32 },
    /// `SPELLCAST_STOP`: our cast's own `SMSG_CAST_RESULT` popped it, or a result came with nothing
    /// in flight (`0x6e741a`, `0x6e74cb`), or a local cancel took it (`0x6e4940` with `cl` 0).
    /// Every cast gets one, an instant, a channel and an item use too; on a timed cast vmangos
    /// sends the result as the bar completes, just ahead of `SMSG_SPELL_GO`.
    Stop,
    /// `SPELLCAST_FAILED`: a refusal's tail (`0x6e224a`) or a cancel with its failed flag
    /// (`0x6e4a43`); the bar, if shown, reads "Failed".
    Failed,
    /// `SPELLCAST_INTERRUPTED`: the same for reasons 0x23 and 0x24 (`0x6e2238`, `0x6e4a24`).
    Interrupted,
    /// `SMSG_SPELL_DELAYED`: pushback extends the cast by `delay_ms`; the bar slides, never
    /// cancels.
    Delayed { delay_ms: u32 },
    /// `MSG_CHANNEL_START`, sent by vmangos to the caster only; gated by [`channel_start_name`].
    ChannelStart { spell_id: u32, duration_ms: u32 },
    /// `MSG_CHANNEL_UPDATE`, also caster-only: the time left, 0 when the channel is over.
    ChannelUpdate { remaining_ms: u32 },
}

/// One report of our own cast.
pub(crate) enum CastReport {
    /// A refusal through `ReportCastFailure 0x6e1a00`: the tradeskill repeat's check
    /// (`0x6e1a3c`), then the line and its combat-log line. Its event tail is pushed beside it.
    Refused(CastFail),
    /// A cast-bar edge.
    Bar(CastBarEdge),
    /// `0x4fd790`, the end of a tradeskill repeat, which a cancel with its failed flag runs last
    /// (`0x6e4abc`-`0x6e4ac2`).
    TradeSkillDisarm,
    /// `SignalCurrentSpellCastChanged 0x4b3250`: `ACTIONBAR_UPDATE_STATE` (`0x4e5bf0`), then
    /// `CURRENT_SPELL_CAST_CHANGED` (`0x4b325a`), the two events' only emitter. The in-flight
    /// slot's push and pop signal it (`SetInFlightSpell 0x6e4ad0`, when the slot changes), and so
    /// do the attempt's arm (`0x6e5333`), its end (`0x6e51e3`) and its arm refusal (`0x6e5069`),
    /// a cancel's word clear (`0x6e495c`), the binds (`0x6e60cb`, `0x6e615d`), the send
    /// (`0x6e589d`) and the active player's form change (`0x5de016`). The trade slot's bind
    /// (`0x6e6939`) signals it too; benilla does not build that bind (#88).
    Changed,
    /// The auto-repeat key `0xceac30` stored or cleared: `START_AUTOREPEAT_SPELL` (435) as the
    /// send stores it (`0x6e594c`, `0x6e5952`), `STOP_AUTOREPEAT_SPELL` (436) as
    /// `StopAutoRepeatSpell` clears it (`0x6ea16a`, `0x6ea170`).
    AutoRepeat(bool),
    /// A line the cast validator shows itself through `CGGameUI::DisplayError 0x496720`, outside
    /// `ReportCastFailure`: a learn rung's, or the tame rung's ahead of its report.
    Line(crate::ui_action::UiError),
}

/// Our cast's reports in the order they were made: one lane ([`crate::lanes`]), so a refusal's line
/// precedes its `SPELLCAST_FAILED` as `0x6e1a00` signals them, and each fires at its packet's place
/// among the other lanes' items, as the spell handlers signal inside their packets.
#[derive(Resource, Default)]
pub(crate) struct CastReports(std::collections::VecDeque<(crate::lanes::Stamp, CastReport)>);

impl CastReports {
    /// Stamp `report` and queue it behind everything stamped before it.
    fn push(&mut self, report: CastReport) {
        let stamp = crate::lanes::stamp();
        let at = self.0.partition_point(|(s, _)| *s < stamp);
        self.0.insert(at, (stamp, report));
    }

    pub(crate) fn bar(&mut self, edge: CastBarEdge) {
        self.push(CastReport::Bar(edge));
    }

    /// A refusal's line; [`crate::spell::CastCore::report`] pushes its event tail after it.
    pub(crate) fn refused(&mut self, fail: CastFail) {
        self.push(CastReport::Refused(fail));
    }

    /// [`CastReport::Line`].
    pub(crate) fn line(&mut self, line: crate::ui_action::UiError) {
        self.push(CastReport::Line(line));
    }

    /// `ShowItemCastFailure 0x6e29b0`, run by the item answer of a refused line that waited on
    /// it ([`crate::items::RecordCallback::CastFailure`]): the line again, under the plain key,
    /// with no log line and no event.
    pub(crate) fn redisplay(&mut self, spell_id: u32, reason: u8, item: u32) {
        self.refused(CastFail::item_redisplay(spell_id, reason, item));
    }

    /// `SMSG_PET_CAST_FAILED` (`0x6e8eb0`): one message, no log line and no event.
    pub(crate) fn pet_refused(&mut self, spell_id: u32, reason: u8) {
        self.refused(CastFail::pet(spell_id, reason));
    }

    pub(crate) fn trade_skill_disarm(&mut self) {
        self.push(CastReport::TradeSkillDisarm);
    }

    /// `0x4b3250` ([`CastReport::Changed`]).
    pub(crate) fn changed(&mut self) {
        self.push(CastReport::Changed);
    }

    /// The auto-repeat key's edge ([`CastReport::AutoRepeat`]).
    pub(crate) fn auto_repeat(&mut self, on: bool) {
        self.push(CastReport::AutoRepeat(on));
    }

    /// The lane's head.
    fn peek(&self) -> Option<crate::lanes::Stamp> {
        self.0.front().map(|(s, _)| *s)
    }

    /// Take the head if `until` admits it.
    fn pop(&mut self, until: crate::lanes::Until) -> Option<(crate::lanes::Stamp, CastReport)> {
        until
            .admits(self.peek()?)
            .then(|| self.0.pop_front())
            .flatten()
    }

    /// Take the reports stamped since `mark` ([`crate::lanes::mark`]), in order: a call's own.
    fn take_since(&mut self, mark: crate::lanes::Mark) -> Vec<CastReport> {
        let (since, before): (std::collections::VecDeque<_>, _) = std::mem::take(&mut self.0)
            .into_iter()
            .partition(|(s, _)| s.since(mark));
        self.0 = before;
        since.into_iter().map(|(_, r)| r).collect()
    }

    /// The reports queued, in order.
    #[cfg(test)]
    pub(crate) fn reports(&self) -> Vec<&CastReport> {
        self.0.iter().map(|(_, r)| r).collect()
    }

    /// Take every report queued, in order, unfired.
    #[cfg(test)]
    pub(crate) fn take(&mut self) -> Vec<CastReport> {
        self.0.drain(..).map(|(_, r)| r).collect()
    }

    /// How many reports are queued.
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Drop every report queued, unfired.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    /// Keep the first `len` reports queued.
    #[cfg(test)]
    pub(crate) fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
    }

    /// The refusals queued, in order.
    #[cfg(test)]
    pub(crate) fn refusals(&self) -> Vec<CastFail> {
        self.0
            .iter()
            .filter_map(|(_, r)| match r {
                CastReport::Refused(f) => Some(*f),
                _ => None,
            })
            .collect()
    }

    /// Take every report queued, answering the refusals among them, in order.
    #[cfg(test)]
    pub(crate) fn take_refusals(&mut self) -> Vec<CastFail> {
        let refusals = self.refusals();
        self.0.clear();
        refusals
    }

    /// The cast-bar edges queued, in order.
    #[cfg(test)]
    pub(crate) fn edges(&self) -> Vec<&CastBarEdge> {
        self.0
            .iter()
            .filter_map(|(_, r)| match r {
                CastReport::Bar(e) => Some(e),
                _ => None,
            })
            .collect()
    }
}

/// `SPELLCAST_START`'s name argument: empty for an unknown spell, or when `AttributesEx3 & 0x4`
/// is set, where the reference pushes `""` (`0x6e7a2d`) and still fires, so the bar is a
/// full-length untitled one. The channel bar names differently ([`channel_start_name`]).
fn cast_bar_label(spells: Option<&crate::ui_action::Spells>, id: u32) -> String {
    spells
        .and_then(|s| s.catalog.get(id))
        .filter(|d| !d.no_casting_bar_text())
        .map(|d| d.name.clone())
        .unwrap_or_default()
}

/// `SPELLCAST_CHANNEL_START`'s name, the second of its `(duration, name)` (format `0x8432cc`,
/// "%d%s"), or `None` where `SpellChannelStart` (`0x6e7550`) fires no event: a zero duration, an
/// id with no `Spell.dbc` row, or `AttributesEx3 & 0x2000`. The name is the spell's own only with
/// `AttributesEx` bit 29 set, else the Lua global `CHANNELING` off the player's chain (`""` when
/// missing, and the event still fires).
///
/// This gates the event only: the move cancel reads the channel off the unit's
/// `UNIT_CHANNEL_SPELL` (`0x51514e`), which the bar never touches, so `ActiveChannel` still arms
/// for a suppressed channel.
fn channel_start_name(
    script: &UiScript,
    spells: Option<&crate::ui_action::Spells>,
    id: u32,
    duration_ms: u32,
) -> Option<String> {
    if duration_ms == 0 {
        return None;
    }
    // No row at that id, or a row that forbids the channel bar: no event.
    let display = spells
        .and_then(|s| s.catalog.get(id))
        .filter(|d| !d.no_channel_bar())?;
    let name = if display.channel_bar_own_name() {
        display.name.clone()
    } else {
        script
            .lua()
            .globals()
            .get::<String>("CHANNELING")
            .unwrap_or_default()
    };
    Some(name)
}

/// An edge's stock event: `SPELLCAST_START(name, ms)`, `SPELLCAST_CHANNEL_START(ms, name)`
/// (reversed, as `CastingBarFrame.lua:80-84` reads it), `SPELLCAST_CHANNEL_UPDATE(ms)` and the
/// argless rest. A channel update of 0 is `SPELLCAST_CHANNEL_STOP`: vmangos ends a channel that
/// way, finished or interrupted.
fn edge_event(
    edge: CastBarEdge,
    script: &UiScript,
    spells: Option<&Spells>,
) -> Option<(&'static str, Vec<ScriptValue>)> {
    match edge {
        CastBarEdge::Start {
            spell_id,
            cast_time_ms,
        } => Some((
            "SPELLCAST_START",
            vec![
                ScriptValue::Str(cast_bar_label(spells, spell_id)),
                ScriptValue::Int(i64::from(cast_time_ms)),
            ],
        )),
        CastBarEdge::Stop => Some(("SPELLCAST_STOP", vec![])),
        CastBarEdge::Failed => Some(("SPELLCAST_FAILED", vec![])),
        CastBarEdge::Interrupted => Some(("SPELLCAST_INTERRUPTED", vec![])),
        CastBarEdge::Delayed { delay_ms } => Some((
            "SPELLCAST_DELAYED",
            vec![ScriptValue::Int(i64::from(delay_ms))],
        )),
        CastBarEdge::ChannelStart {
            spell_id,
            duration_ms,
        } => {
            let name = channel_start_name(script, spells, spell_id, duration_ms);
            // Traced here, where the `0x6e7550` gate is decided; other edges trace in
            // `spell::net`.
            if *crate::net::CAST_TRACE {
                match name.as_deref() {
                    Some(label) => info!(
                        "cast-trace: CHANNEL_START — spell {spell_id} {duration_ms}ms, \
                         bar label {label:?}"
                    ),
                    None => info!(
                        "cast-trace: CHANNEL_START — spell {spell_id} {duration_ms}ms, \
                         NO BAR (0x6e7550 gate: zero duration, no Spell.dbc row, or \
                         AttributesEx3 & 0x2000)"
                    ),
                }
            }
            name.map(|name| {
                (
                    "SPELLCAST_CHANNEL_START",
                    vec![
                        ScriptValue::Int(i64::from(duration_ms)),
                        ScriptValue::Str(name),
                    ],
                )
            })
        }
        // `0x6e75f0`: nonzero re-times the bar, zero ends it, with no attribute gate; for a
        // suppressed channel the stock Lua's `IsShown()` guards absorb these.
        CastBarEdge::ChannelUpdate { remaining_ms } => {
            let over = remaining_ms == 0;
            if *crate::net::CAST_TRACE {
                if over {
                    info!("cast-trace: CHANNEL_STOP — update 0, the channel is over");
                } else {
                    info!("cast-trace: CHANNEL_UPDATE — {remaining_ms}ms left");
                }
            }
            Some(if over {
                ("SPELLCAST_CHANNEL_STOP", vec![])
            } else {
                (
                    "SPELLCAST_CHANNEL_UPDATE",
                    vec![ScriptValue::Int(i64::from(remaining_ms))],
                )
            })
        }
    }
}

/// A report made ready for the VM: everything outside it (a line's sound, a chat line, a log
/// file's tee) is done, and what runs Lua is left, so it can run with the call host back in place.
pub(crate) enum Ready {
    /// `0x6e1a3c`: a refusal of the armed tradeskill repeat's spell ends the repeat.
    TradeSkillCastFailed(u32),
    /// `0x4fd790`: whatever repeat is armed ends.
    TradeSkillDisarm,
    /// An event and its arguments.
    Event(&'static str, Vec<ScriptValue>),
}

/// Signal `ready` in order.
pub(crate) fn fire(script: &mut UiScript, ready: Vec<Ready>) {
    for r in ready {
        match r {
            Ready::TradeSkillCastFailed(spell_id) => script.trade_skill_cast_failed(spell_id),
            Ready::TradeSkillDisarm => script.trade_skill_disarm(),
            Ready::Event(event, args) => script.fire_event(event, args),
        }
    }
}

/// What readies our cast's reports: the refused cast's line ([`CastFailLines`]) and the sink its
/// sound and log line go to.
#[derive(SystemParam)]
pub(crate) struct CastReporter<'w, 's> {
    lines: CastFailLines<'w, 's>,
    sink: MessageSink<'w>,
}

impl CastReporter<'_, '_> {
    /// `reports`, readied in order: a refusal's tradeskill check (`0x6e1a3c`, the player's first
    /// pass only), its line and its log line, an edge's event, a disarm.
    pub(crate) fn ready(
        &mut self,
        script: &UiScript,
        reports: impl IntoIterator<Item = CastReport>,
    ) -> Vec<Ready> {
        let mut out = Vec::new();
        for report in reports {
            match report {
                CastReport::Refused(fail) => {
                    if fail.caster == crate::ui_action::Caster::Player && !fail.redisplay {
                        out.push(Ready::TradeSkillCastFailed(fail.spell_id));
                    }
                    if let Some(resolved) = self.lines.resolve(script, fail) {
                        out.extend(self.line(script, resolved.shown));
                        // After the line (`0x6e21dd`), `0x6e21fc` → `LogCastFailure 0x62c360`,
                        // whose line fires its chat event inside the report.
                        if let Some(log) = resolved.log {
                            let written = self.sink.chat.write_combat_now(script, &log);
                            out.extend(written.into_iter().map(|(e, args)| Ready::Event(e, args)));
                        }
                    }
                }
                CastReport::Bar(edge) => out.extend(
                    edge_event(edge, script, self.lines.spells())
                        .map(|(event, args)| Ready::Event(event, args)),
                ),
                CastReport::TradeSkillDisarm => out.push(Ready::TradeSkillDisarm),
                CastReport::Changed => out.extend([
                    Ready::Event("ACTIONBAR_UPDATE_STATE", vec![]),
                    Ready::Event("CURRENT_SPELL_CAST_CHANGED", vec![]),
                ]),
                CastReport::AutoRepeat(true) => {
                    out.push(Ready::Event("START_AUTOREPEAT_SPELL", vec![]))
                }
                CastReport::AutoRepeat(false) => {
                    out.push(Ready::Event("STOP_AUTOREPEAT_SPELL", vec![]))
                }
                CastReport::Line(e) => {
                    let get = |key: &str| script.lua().globals().get::<String>(key).ok();
                    if let Some(text) = crate::ui_action::ui_error_text(&e, &get) {
                        out.extend(self.line(script, Shown::keyed(e.key, text)));
                    }
                }
            }
        }
        out
    }

    /// A line's world half done, its event readied: a chat line's too, which `DisplayError`
    /// writes in place (`0x49a870`).
    pub(crate) fn line(&mut self, script: &UiScript, shown: Shown) -> Option<Ready> {
        shown
            .sink_now(script, &mut self.sink, "ui_action")
            .map(|(event, args)| Ready::Event(event, args))
    }
}

/// The reports a script call makes, signalled inside it ([`crate::script_calls`]): the by-key
/// refusals its verb raised (an item not ready, a slot that cannot be used), then its cast's
/// reports in order. What was queued before the call stays on its lane.
#[derive(SystemParam)]
pub(crate) struct CallReports<'w, 's> {
    reports: ResMut<'w, CastReports>,
    ui_errors: Res<'w, crate::ui_action::UiErrorKeys>,
    reporter: CastReporter<'w, 's>,
}

/// Where a call's reports start: the clock its refusal keys and its cast reports are stamped
/// after.
#[derive(Clone, Copy)]
pub(crate) struct ReportMark(crate::lanes::Mark, crate::lanes::Mark);

impl CallReports<'_, '_> {
    pub(crate) fn mark(&self) -> ReportMark {
        let now = crate::lanes::mark();
        ReportMark(now, now)
    }

    /// A mark at the clock's `at`, for an engine writer that marked where its step began: the
    /// cast reports it made since then are its own, and it raises no refusal keys.
    pub(crate) fn mark_at(&self, at: crate::lanes::Mark) -> ReportMark {
        ReportMark(crate::lanes::mark(), at)
    }

    /// The reports queued since `mark`, taken off their queues and readied.
    pub(crate) fn ready_since(&mut self, script: &UiScript, mark: ReportMark) -> Vec<Ready> {
        let ReportMark(errors, reports) = mark;
        let errors = self.ui_errors.take_since(errors);
        let mut ready: Vec<Ready> = errors
            .iter()
            .filter_map(|e| {
                crate::ui_action::ui_error_text(e, &|key| {
                    script.lua().globals().get::<String>(key).ok()
                })
                .map(|t| Shown::keyed(e.key, t))
            })
            .filter_map(|shown| self.reporter.line(script, shown))
            .collect();
        let reports = self.reports.take_since(reports);
        ready.extend(self.reporter.ready(script, reports));
        ready
    }
}

/// The cast lane's `fire_until` ([`crate::lanes`]): each report the lane holds while `until`
/// admits it, readied and signalled at its place with the host lent, as the spell handlers signal
/// inside their packets (`0x6e1a00`, `0x6e7a2d`, `0x6e7550`); the lane stops after a report whose
/// firing stamped anything, so what it pushed fires next. A refusal's handler may cast again.
fn fire_cast_reports(
    In(until): In<crate::lanes::Until>,
    mut vm: crate::script_calls::VmWith<(
        ResMut<'static, CastReports>,
        CastReporter<'static, 'static>,
    )>,
) {
    loop {
        let Some((stamp, report)) = vm.own().0.pop(until) else {
            return;
        };
        let ((), moved) = crate::lanes::firing(stamp, || {
            let ready = vm.turn(|script, (_, mut reporter)| {
                script.map(|script| reporter.ready(script, [report]))
            });
            if let Some(ready) = ready {
                vm.run(|script| {
                    if let Some(script) = script {
                        fire(script, ready);
                    }
                });
            }
        });
        if moved {
            return;
        }
    }
}

/// The session's end drops the reports no VM will hear, as the chat lane drops its lines: the
/// Lua state they were for is replaced at the session end.
fn end_session_reports(mut reports: ResMut<CastReports>) {
    reports.clear();
}

/// Every report queued, fired alone, for a test that drives the reports without the other lanes.
#[cfg(test)]
pub(crate) fn signal_reports(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    let until = crate::lanes::Until::all();
    while world
        .resource::<CastReports>()
        .peek()
        .is_some_and(|s| until.admits(s))
    {
        world
            .run_system_once_with(fire_cast_reports, until)
            .unwrap();
    }
}

/// The cast lane and the cast views' log.
pub(crate) struct UiCastPlugin;

impl Plugin for UiCastPlugin {
    fn build(&self, app: &mut App) {
        use crate::lanes::LaneApp;
        views::add_cast_views_lane(app);
        app.init_resource::<CastReports>()
            .add_lane(
                "cast reports",
                |world| world.get_resource::<CastReports>()?.peek(),
                fire_cast_reports,
                None,
            )
            .add_systems(
                OnExit(crate::client_state::ClientState::InWorld),
                end_session_reports.in_set(crate::ui_script::SessionEnd::Clears),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spell 22810, "Opening - No Text", carries `SPELL_ATTR_EX3_NO_CASTING_BAR_TEXT`.
    #[test]
    fn the_no_casting_bar_text_attribute_blanks_the_label() {
        use benilla_formats::{SpellCatalog, SpellDisplay};
        let named = |name: &str, attributes_ex3: u32| SpellDisplay {
            name: name.to_string(),
            attributes_ex3,
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [
                    (6478, named("Opening", 0)),
                    (22810, named("Opening - No Text", 0x4)),
                ]
                .into_iter()
                .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        assert_eq!(cast_bar_label(Some(&spells), 6478), "Opening");
        assert_eq!(
            cast_bar_label(Some(&spells), 22810),
            "",
            "the event still fires carrying its duration — a full-length UNTITLED bar, \
             not a hidden one (0x6e7a33 pushes the empty string, then fires as usual)"
        );
        // An unknown id, and no client data at all.
        assert_eq!(cast_bar_label(Some(&spells), 133), "");
        assert_eq!(cast_bar_label(None, 6478), "");
    }

    /// The attribute columns are those of the shipped 5875 rows.
    #[test]
    fn the_channel_bar_says_channeling_unless_the_spell_may_name_itself() {
        use benilla_formats::{SpellCatalog, SpellDisplay};
        let row = |name: &str, attributes_ex: u32, attributes_ex3: u32| SpellDisplay {
            name: name.to_string(),
            attributes_ex,
            attributes_ex3,
            ..Default::default()
        };
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [
                    // Blizzard rank 1: bit 29 clear.
                    (10, row("Blizzard", 0x1000_008c, 0)),
                    // Fishing: bit 29 set, one of nine rows that name themselves.
                    (7620, row("Fishing", 0x2100_4004, 0x0800_0000)),
                    // Blood Siphon: `AttributesEx3 & 0x2000` suppresses the event.
                    (24322, row("Blood Siphon", 0x2001_808c, 0x2000)),
                ]
                .into_iter()
                .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        // The word is the Lua global, set by hand so the gates run without client data.
        let script = UiScript::new().expect("VM");
        script.eval::<()>("CHANNELING = \"Channeling\"").unwrap();

        let name_of = |id: u32, ms: u32| -> Option<String> {
            channel_start_name(&script, Some(&spells), id, ms)
        };
        assert_eq!(
            name_of(10, 8_000).as_deref(),
            Some("Channeling"),
            "Blizzard has AttributesEx bit 29 clear, so the bar reads the generic word"
        );
        assert_eq!(
            name_of(7620, 20_000).as_deref(),
            Some("Fishing"),
            "bit 29 set takes Name[locale] instead"
        );
        assert_eq!(
            name_of(24322, 8_000),
            None,
            "AttributesEx3 & 0x2000 fires NO event — a suppressed bar, not a blank one"
        );
        assert_eq!(name_of(10, 0), None, "0x6e7574: duration <= 0 -> no event");
        assert_eq!(
            name_of(133, 8_000),
            None,
            "0x6e7590: no SpellRec -> no event"
        );
        assert_eq!(
            channel_start_name(&script, None, 10, 8_000),
            None,
            "no client data is the same no-record leg"
        );
    }

    /// `CHANNELING` is `GlobalStrings.lua:479`. Skips without client data.
    #[test]
    fn the_channeling_word_comes_off_the_players_own_chain() {
        let _data = benilla_formats::wow_data_or_skip!();
        use benilla_formats::{SpellCatalog, SpellDisplay};
        let script = UiScript::new().expect("VM");
        crate::ui_script::test_ui::load_ui(&script, r"Interface\FrameXML\GlobalStrings.lua");
        let spells = crate::ui_action::Spells {
            catalog: SpellCatalog::from_displays(
                [(
                    10,
                    SpellDisplay {
                        name: "Blizzard".into(),
                        attributes_ex: 0x1000_008c,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        };
        assert_eq!(
            channel_start_name(&script, Some(&spells), 10, 8_000).as_deref(),
            Some("Channeling"),
        );
    }
}
