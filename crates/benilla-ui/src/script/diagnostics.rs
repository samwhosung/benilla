//! The engine's record of this session's script errors, addon load failures and warnings, for the
//! host (the world-entry load-failure lines, the addon harness) and dev instruments; no Lua global
//! reads it. The layer's `/errors` window keeps its own log off the Lua error handler, as a 1.12
//! addon does. Dispatch and `_ERRORMESSAGE` are unchanged by it.
//!
//! A repeat bumps its row's count and keeps its place, rows are oldest first, and past
//! [`DIAGNOSTIC_LOG_CAP`] distinct messages the oldest is evicted; `seq` is never reused, so an
//! eviction shows as a gap. The log lives on the `Model`, and every world entry and `ReloadUI`
//! builds a fresh VM, so each starts an empty log.

use std::collections::VecDeque;

use mlua::Lua;

use super::Model;

/// Distinct messages retained before the oldest is evicted, sized for the whole 218-addon corpus
/// loaded at once.
pub const DIAGNOSTIC_LOG_CAP: usize = 256;

/// What a row reports, split by its consequence to the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// Code ran and raised; the addon is otherwise loaded.
    Error,
    /// An addon did not load, whether its file scope raised or a file, document or dependency
    /// was missing or broken.
    Load,
    /// A call was accepted and did not do what it said (a dropped `inherits=`, an unresolved
    /// `SetPoint` target, a `SetCVar` on an unregistered name, a refused `CreateMacro`).
    Warning,
}

impl DiagnosticKind {
    /// The one-word tag a surface prints; stable, as a surface may key colour off it.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Load => "load",
            Self::Warning => "warn",
        }
    }
}

/// One retained row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The "#N" a surface shows: 1-based, monotonic and never reused.
    pub seq: u64,
    pub kind: DiagnosticKind,
    /// The message exactly as the host logged it.
    pub message: String,
    /// Occurrences of this `(kind, message)`, from 1.
    pub count: u32,
}

/// The log, held on the model.
#[derive(Debug, Default)]
pub(crate) struct DiagnosticLog {
    rows: VecDeque<Diagnostic>,
    /// The last `seq` handed out; kept apart from `rows.len()` so eviction never renumbers.
    seq: u64,
}

impl DiagnosticLog {
    /// Record one failure, or bump the count of its identical row.
    pub(crate) fn record(&mut self, kind: DiagnosticKind, message: &str) {
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.kind == kind && r.message == message)
        {
            row.count = row.count.saturating_add(1);
            return;
        }
        self.seq += 1;
        if self.rows.len() == DIAGNOSTIC_LOG_CAP {
            self.rows.pop_front();
        }
        self.rows.push_back(Diagnostic {
            seq: self.seq,
            kind,
            message: message.to_string(),
            count: 1,
        });
    }

    /// Every retained row, oldest first.
    pub(crate) fn rows(&self) -> impl Iterator<Item = &Diagnostic> {
        self.rows.iter()
    }

    /// Retained rows.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// Distinct failures recorded this session, evicted rows included.
    #[cfg(test)]
    pub(crate) fn total(&self) -> u64 {
        self.seq
    }
}

impl super::UiScript {
    /// Every retained row, oldest first, cloned out of the model.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.model_ref().diagnostics.rows().cloned().collect()
    }

    /// Retain an addon load failure that never raised (a missing file, an unparseable document, a
    /// dependency cycle or missing dependency, a broken `Bindings.xml`). It is not dispatched to
    /// `geterrorhandler()`: the reference logs such a failure and shows nothing. The caller still
    /// writes its own log line.
    pub fn report_load_failure(&self, msg: &str) {
        record_load_failure(&self.lua, msg);
    }

    /// Whether `FrameXML_Debug` is above 0, which puts every banner in the load log.
    pub fn framexml_debug(&self) -> bool {
        self.model_ref().framexml_debug.get() > 0
    }

    /// Merge one closed level of a UI load (the core's `.toc`, an add-on) into the load's record.
    pub fn report_load_status(&self, status: crate::status::Status) {
        self.model_mut().load_log.ui.merge(status);
    }

    /// The UI load is over: its record is drained (`0x490187`), after any `LoadAddOn` block it
    /// ran.
    pub fn finish_ui_load_log(&self) {
        let mut model = self.model_mut();
        let lines = model.load_log.ui.take_lines();
        model
            .load_log
            .writes
            .push(crate::status::LogWrite::Rewrite(lines));
    }

    /// The drains made since the last call, in order, for the host to write.
    pub fn take_load_log_writes(&mut self) -> Vec<crate::status::LogWrite> {
        std::mem::take(&mut self.model_mut().load_log.writes)
    }

    /// Retain a warning the host has already logged (a loader warning with its `<Addon>/<file>`
    /// prefix). Queuing it on `Model::warnings` too would log it twice; an engine-raised warning
    /// goes through `Model::record_warning`, which does both.
    pub fn report_warning(&self, msg: &str) {
        record_warning(&self.lua, msg);
    }
}

/// Retain an addon file that did not load: the one rule for the startup walk and for `LoadAddOn`,
/// which holds only `&Lua`. The reference's failed open is non-fatal: `0x6edaa0` logs
/// `Couldn't open %s` (`0x846ff4`) at severity 2 and loading goes on, never reaching the Lua
/// error handler.
pub(crate) fn record_load_failure(lua: &Lua, msg: &str) {
    lua.app_data_mut::<Model>()
        .expect("model app_data set")
        .diagnostics
        .record(DiagnosticKind::Load, msg);
}

/// Retain a warning its caller, holding `&Lua`, has already logged; `Model::record_warning` also
/// queues it for the host log.
pub(crate) fn record_warning(lua: &Lua, msg: &str) {
    lua.app_data_mut::<Model>()
        .expect("model app_data set")
        .diagnostics
        .record(DiagnosticKind::Warning, msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_collapse_onto_the_first_row_and_count() {
        let mut log = DiagnosticLog::default();
        for _ in 0..1113 {
            log.record(DiagnosticKind::Error, "OnUpdate: boom");
        }
        assert_eq!(log.len(), 1, "1,113 raises are ONE row — the whole point");
        let row = log.rows().next().unwrap();
        assert_eq!(row.count, 1113);
        assert_eq!(row.seq, 1);
        assert_eq!(log.total(), 1);
    }

    #[test]
    fn a_warning_reaches_the_retained_log_and_the_host_drain() {
        let mut s = crate::script::UiScript::new().expect("vm");
        s.run(r#"SetCVar("thisCVarDoesNotExist", "1")"#).unwrap();

        let host = s.take_warnings();
        assert!(
            host.iter().any(|w| w.contains("thisCVarDoesNotExist")),
            "the host's per-frame drain still gets it: {host:?}"
        );
        let kept = s.diagnostics();
        let row = kept
            .iter()
            .find(|d| d.message.contains("thisCVarDoesNotExist"))
            .unwrap_or_else(|| panic!("…and it is RETAINED: {kept:#?}"));
        assert_eq!(row.kind, DiagnosticKind::Warning, "under its own kind");

        assert!(s.take_warnings().is_empty());
        assert!(s
            .diagnostics()
            .iter()
            .any(|d| d.message.contains("thisCVarDoesNotExist")));
    }

    #[test]
    fn a_text_sink_takes_bytes_rather_than_raising_on_them() {
        // `strsub` is byte-indexed, so an addon can hand `SetText` half a UTF-8 character; that
        // costs a glyph, never a raise.
        let s = crate::script::UiScript::new().expect("vm");
        s.run(
            r#"
            f = CreateFrame("Frame", "BytesFrame")
            t = f:CreateFontString("BytesText")
            -- "a—b" is 5 bytes; cutting at 2 leaves the em dash's lead byte alone.
            local sliced = strsub("a\226\128\148b", 1, 2)
            t:SetText(sliced)
            e = CreateFrame("EditBox", "BytesBox")
            e:SetText(sliced)
        "#,
        )
        .expect("a sliced multi-byte string must not raise");
        assert!(s.errors().is_empty(), "{:?}", s.errors());
        let got: Option<String> = s.eval("return BytesText:GetText()").unwrap();
        assert_eq!(
            got.as_deref(),
            Some("a\u{fffd}"),
            "the broken byte became one replacement glyph, and the call completed"
        );
    }

    #[test]
    fn kind_is_part_of_identity() {
        let mut log = DiagnosticLog::default();
        log.record(DiagnosticKind::Error, "same text");
        log.record(DiagnosticKind::Load, "same text");
        assert_eq!(
            log.len(),
            2,
            "the same string means different things per kind"
        );
    }

    #[test]
    fn order_is_first_occurrence_and_a_repeat_does_not_move_its_row() {
        let mut log = DiagnosticLog::default();
        log.record(DiagnosticKind::Load, "first");
        log.record(DiagnosticKind::Error, "second");
        log.record(DiagnosticKind::Load, "first");
        let seen: Vec<_> = log.rows().map(|r| r.message.as_str()).collect();
        assert_eq!(seen, ["first", "second"]);
        assert_eq!(log.rows().next().unwrap().count, 2);
    }

    #[test]
    fn eviction_is_oldest_first_and_seq_never_rewinds() {
        let mut log = DiagnosticLog::default();
        for i in 0..DIAGNOSTIC_LOG_CAP + 10 {
            log.record(DiagnosticKind::Error, &format!("e{i}"));
        }
        assert_eq!(log.len(), DIAGNOSTIC_LOG_CAP);
        let first = log.rows().next().unwrap();
        assert_eq!(first.message, "e10", "the oldest ten were evicted");
        assert_eq!(
            first.seq, 11,
            "seq is monotonic: the gap IS the honest report that rows were dropped"
        );
        assert_eq!(log.total() as usize, DIAGNOSTIC_LOG_CAP + 10);
    }
}
