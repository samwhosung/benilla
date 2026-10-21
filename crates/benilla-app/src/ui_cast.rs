//! Our own cast's reports, in the order the reference signals them: the refused cast's line
//! (`ReportCastFailure 0x6e1a00`), the wire edges that become the `SPELLCAST_*` events stock
//! `CastingBarFrame.lua:6-13` registers for, named from `Spell.dbc`, and the tradeskill repeat's
//! disarm. A script call's reports are signalled inside the call ([`crate::script_calls`]), the
//! rest by the frame's feed ([`feed_cast_reports`]), which also pushes the VM's cast state. The
//! cast lifecycle is `crate::spell`'s.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::spell::CastState;
use crate::ui_action::{CastFail, CastFailLines, MessageSink, Shown, Spells};
use crate::ui_unit::UnitFeed;

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
}

/// Our cast's reports in the order they were made: one queue, so a refusal's line precedes its
/// `SPELLCAST_FAILED` as `0x6e1a00` signals them.
#[derive(Resource, Default)]
pub(crate) struct CastReports(pub(crate) Vec<CastReport>);

impl CastReports {
    pub(crate) fn bar(&mut self, edge: CastBarEdge) {
        self.0.push(CastReport::Bar(edge));
    }

    /// A refusal's line; [`crate::spell::CastCore::report`] pushes its event tail after it.
    pub(crate) fn refused(&mut self, fail: CastFail) {
        self.0.push(CastReport::Refused(fail));
    }

    /// `SMSG_PET_CAST_FAILED` (`0x6e8eb0`): one message, no log line and no event.
    pub(crate) fn pet_refused(&mut self, spell_id: u32, reason: u8) {
        self.refused(CastFail::pet(spell_id, reason));
    }

    pub(crate) fn trade_skill_disarm(&mut self) {
        self.0.push(CastReport::TradeSkillDisarm);
    }

    /// `0x4b3250` ([`CastReport::Changed`]).
    pub(crate) fn changed(&mut self) {
        self.0.push(CastReport::Changed);
    }

    /// The auto-repeat key's edge ([`CastReport::AutoRepeat`]).
    pub(crate) fn auto_repeat(&mut self, on: bool) {
        self.0.push(CastReport::AutoRepeat(on));
    }

    /// The refusals queued, in order.
    #[cfg(test)]
    pub(crate) fn refusals(&self) -> Vec<CastFail> {
        self.0
            .iter()
            .filter_map(|r| match r {
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
            .filter_map(|r| match r {
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
                        out.extend(self.line(resolved.shown));
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
            }
        }
        out
    }

    /// The refused lines whose item record has answered since (`0x6e29b0`).
    pub(crate) fn redisplays(&mut self, script: &UiScript) -> Vec<Ready> {
        let shown = self.lines.redisplays(script);
        shown.into_iter().filter_map(|s| self.line(s)).collect()
    }

    /// A line's world half done, its event readied.
    pub(crate) fn line(&mut self, shown: Shown) -> Option<Ready> {
        shown
            .sink(&mut self.sink, "ui_action")
            .map(|(event, args)| Ready::Event(event, args))
    }
}

/// The reports a script call makes, signalled inside it ([`crate::script_calls`]): the by-key
/// refusals its verb raised (an item not ready, a slot that cannot be used), then its cast's
/// reports in order. What was queued before the call stays for the frame's feed.
#[derive(SystemParam)]
pub(crate) struct CallReports<'w, 's> {
    reports: ResMut<'w, CastReports>,
    ui_errors: Res<'w, crate::ui_action::UiErrorKeys>,
    reporter: CastReporter<'w, 's>,
}

/// Where a call's reports start in each queue.
#[derive(Clone, Copy)]
pub(crate) struct ReportMark(usize, usize);

impl CallReports<'_, '_> {
    pub(crate) fn mark(&self) -> ReportMark {
        ReportMark(self.ui_errors.len(), self.reports.0.len())
    }

    /// The reports queued since `mark`, taken off their queues and readied.
    pub(crate) fn ready_since(&mut self, script: &UiScript, mark: ReportMark) -> Vec<Ready> {
        let ReportMark(errors, reports) = mark;
        let start = errors.min(self.ui_errors.len());
        let errors = self.ui_errors.take_from(start);
        let mut ready: Vec<Ready> = errors
            .iter()
            .filter_map(|e| {
                crate::ui_action::ui_error_text(e, &|key| {
                    script.lua().globals().get::<String>(key).ok()
                })
                .map(|t| Shown::keyed(e.key, t))
            })
            .filter_map(|shown| self.reporter.line(shown))
            .collect();
        let start = reports.min(self.reports.0.len());
        let reports: Vec<_> = self.reports.0.drain(start..).collect();
        ready.extend(self.reporter.ready(script, reports));
        ready
    }
}

/// Signal the reports made outside any script call, and push the cast state the frame's packets
/// and cancels left ([`CastState`]), which `SpellStopCasting` (`0x6e6e80`) and the bar's checked
/// tests read: a queued strike and the spell awaiting its click count as in flight, so the first
/// Esc stops it and `ToggleGameMenu` skips `ClearTarget()` (`UIParent.lua:1489-1492`). The active
/// player's form change is signalled here too, off its descriptor edge (`OnShapeshiftFormChanged
/// 0x5de010` → `0x5de016`), after the book's push, whose checked ring it lights.
pub(crate) fn feed_cast_reports(
    // The reports and the cast state, which the host's casts write.
    mut vm: crate::script_calls::VmWith<(
        ResMut<'static, CastReports>,
        CastReporter<'static, 'static>,
        CastState<'static, 'static>,
    )>,
    mut edges: MessageReader<crate::net::FieldChanged>,
    me: Query<Entity, With<crate::net::SelfPlayer>>,
) {
    let me = me.iter().next();
    let form_changed = edges.read().any(|e| {
        e.unit_field(benilla_protocol::field::FIELD_UNIT_BYTES_1)
            && Some(e.entity) == me
            && (e.old >> 16) as u8 != (e.new >> 16) as u8
    });
    // The view and the reports read as an engine turn, then signalled with the host lent: a
    // refusal's handler may cast again.
    let ready = vm.turn(|script, (mut reports, mut reporter, cast_state)| {
        if form_changed {
            reports.0.insert(0, CastReport::Changed);
        }
        let script = script?;
        script.set_cast_view(cast_state.view());
        let mut ready = reporter.redisplays(script);
        ready.extend(reporter.ready(script, std::mem::take(&mut reports.0)));
        Some(ready)
    });
    let Some(ready) = ready else {
        return;
    };
    vm.run(|script| {
        if let Some(script) = script {
            fire(script, ready);
        }
    });
}

/// The report feed, drained before the VM ticks so an edge and its first `OnUpdate` share a
/// frame, and after the local cancel so a move or Esc kills the bar on the next tick.
pub(crate) struct UiCastPlugin;

impl Plugin for UiCastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CastReports>().add_systems(
            Update,
            // After the spell's local cancel, whose edges this drains the same frame.
            feed_cast_reports
                .in_set(UnitFeed)
                .after(crate::spell::LocalCancel),
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
