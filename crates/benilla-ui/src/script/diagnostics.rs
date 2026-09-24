//! The **script error log** — what went wrong, kept where the player can read it (decision 1495).
//!
//! ## Why this exists
//!
//! 1305 gave script errors a *screen*: the engine dispatches every caught error to
//! `geterrorhandler()`, and FrameXML's `_ERRORMESSAGE` puts the red `ScriptErrors` dialog up. That
//! is the reference's behaviour and it stays exactly as it is. It is also, on its own, not enough,
//! and it leaves two asks: a Lua error frame that says which addons still fail, and a
//! BugGrabber-style collector that keeps their stack traces for a bug report.
//!
//! Both asks are the same three gaps, and all three are about *memory*:
//!
//! 1. **`_ERRORMESSAGE` shows a burst's FIRST message only** — its own `ScriptErrors:IsVisible()`
//!    guard, faithfully transcribed. A world entry that raises 1,113 times (1305 measured exactly
//!    that) shows one string. This is precisely why BugGrabber exists in the real world.
//! 2. **An addon can fail to load without ever raising**, and those failures had no channel at all
//!    — a manifest entry the package does not ship, a document that will not parse, a dependency
//!    cycle, a missing hard dependency, a broken `Bindings.xml`. They logged to the terminal and
//!    stopped; the addon simply was not there and the client said nothing.
//! 3. **Nothing retained anything.** [`super::UiScript::take_errors`] drains to the host log every
//!    frame, so once a frame passed, the terminal held the only copy. A player cannot read a
//!    terminal, and a bug report written from one is what B271 was debugged off.
//!
//! ## What this is, and what it deliberately is not
//!
//! A bounded, deduplicating, ordered log of everything that went wrong this session, written at
//! the two choke points every failure already passes ([`super::model::Model::record_script_error`]
//! and [`super::UiScript::report_load_failure`]) and read by whatever wants to show it.
//!
//! **"Session" means one world session, not one process.** The log lives on the `Model`, and the
//! in-game UI is rebuilt onto a fresh VM at every world entry (1290) — so a logout or a `ReloadUI`
//! starts an empty log. That is the right boundary and it self-heals: the load walk runs again on
//! the way back in and re-records whatever is still broken. What it is *not* is a place to look for
//! what happened before the reload you just did.
//!
//! It is **purely additive**: no dispatch changes, no `_ERRORMESSAGE` change, no divergence from
//! the reference's own behaviour. The reference simply never had this instrument, and *"a modern,
//! idiomatic client"* covers building one.
//!
//! It is **not** a transcription of BugGrabber, which is somebody else's addon (and not Blizzard's
//! either). It is our own equivalent, and an addon that installs its own `seterrorhandler` still
//! wins outright for the dispatch half exactly as 1195/1305 leave it.
//!
//! ## Dedupe, order, and the bound
//!
//! **Repeats collapse onto the first occurrence and bump a count.** An `OnUpdate` that raises
//! every frame is one row reading `×1113`, not 1,113 rows — without that the log is useless within
//! seconds, and unbounded besides.
//!
//! **Order is first-occurrence, oldest first.** The load-time failures are the causes and the
//! later runtime errors are usually their consequences, so a chronological read is the one that
//! explains itself. A repeat does *not* move its row to the end: a row that jumps around while you
//! read it is worse than a stale position.
//!
//! **[`DIAGNOSTIC_LOG_CAP`] distinct rows**, evicting oldest-first. `seq` is monotonic and never
//! reused, so an evicted prefix is visible as a gap rather than silently rewriting history — a
//! surface can say "showing #251-#500" honestly. The cap binds *distinct* messages, which after
//! dedupe is a much higher ceiling than it looks: 1305's 1,113-raise run produces **one** row.

use std::collections::VecDeque;

use mlua::{IntoLua, Lua, MultiValue, Value};

use super::Model;

/// How many **distinct** messages the log retains before evicting the oldest.
///
/// Sized against the worst real load we have measured rather than a round number: the 218-addon
/// corpus behind 1306, loaded at once, is the widest set of distinct failures this client has ever
/// produced. Dedupe means repeats cost nothing, so this bounds *kinds* of problem, not events.
pub const DIAGNOSTIC_LOG_CAP: usize = 256;

/// What a retained row *is* — and the split is by **consequence**, not by where it was caught,
/// because that is the only distinction the player can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// **Code ran and raised.** The addon is loaded and working apart from this; the failure may
    /// be harmless, may be one frame's bad luck, may be constant. This is the class that already
    /// reaches the red dialog.
    Error,
    /// **An addon did not load.** Definitely broken, definitely worth telling somebody about, and
    /// before 1495 invisible unless you were reading the terminal. Both the raising kind (a file
    /// scope that blew up) and the silent kind (a missing file, an unparseable document, a
    /// dependency that isn't there) land here — from the player's side they are one thing: *the
    /// addon isn't running*.
    Load,
    /// **The call was accepted and did not do what it said.** Nothing raised and nothing failed to
    /// load: an `inherits=` argument that was not a template name and got dropped, a `SetPoint`
    /// whose `relativeTo` did not resolve and re-anchored to the owner, a `SetCVar` on a name
    /// nothing registered, a saved variable that could not be serialised, a `CreateMacro` that
    /// was refused. The addon runs on, believing it got what it asked for.
    ///
    /// **This kind exists because the channel was a dead end** (decision 2135). These messages had
    /// one consumer — a `warn!` line in the host's terminal, drained and discarded every frame —
    /// so they reached neither the player (who cannot read a terminal, which is the whole
    /// argument of 1495 above) nor the addon survey (whose columns read `errors`). A warning is
    /// exactly the class an instrument is *for*: it is the failure that does not announce itself.
    Warning,
}

impl DiagnosticKind {
    /// The one-word tag a surface prints. Stable — a surface may key colour off it.
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
    /// Monotonic, 1-based, **never reused** — the "#N" a surface shows. Survives eviction, so a
    /// gap in the numbers is an honest report that older rows were dropped.
    pub seq: u64,
    pub kind: DiagnosticKind,
    /// The message exactly as the host logged it — same string, same spelling, so a player's
    /// screenshot and our terminal line are greppable against each other.
    pub message: String,
    /// How many times this exact `(kind, message)` has occurred. Starts at 1.
    pub count: u32,
}

/// The log itself. Lives on the model; see the module header for the shape and the why.
#[derive(Debug, Default)]
pub(crate) struct DiagnosticLog {
    rows: VecDeque<Diagnostic>,
    /// The last `seq` handed out. Kept separately from `rows.len()` precisely so eviction cannot
    /// renumber anything.
    seq: u64,
}

impl DiagnosticLog {
    /// Record one failure, collapsing it onto an existing identical row if there is one.
    ///
    /// The dedupe scan is linear over at most [`DIAGNOSTIC_LOG_CAP`] rows and runs on the failure
    /// path only. The pathological caller is an `OnUpdate` raising at frame rate, where the scan
    /// hits its row and returns — cheaper by far than the `String` allocation it saves, and
    /// arithmetic beside the `mlua` error formatting that produced the message in the first place.
    pub(crate) fn record(&mut self, kind: DiagnosticKind, message: &str) {
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.kind == kind && r.message == message)
        {
            // Saturating rather than wrapping: a count that rolls over to 0 reads as "this never
            // happened", which is the one answer that is never true here.
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
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// Distinct failures **ever** recorded this session, including rows since evicted. The
    /// difference from [`Self::len`] is how many the cap ate.
    pub(crate) fn total(&self) -> u64 {
        self.seq
    }

    /// Forget everything — the player's own "I've read these" act. Deliberately does **not** reset
    /// `seq`: after a clear, the next row is still #N+1, so a screenshot taken before the clear and
    /// one taken after cannot claim the same number for different failures.
    pub(crate) fn clear(&mut self) {
        self.rows.clear();
    }
}

impl super::UiScript {
    /// Every retained row, oldest first — a clone, because the caller is the app's UI and the
    /// model is behind `app_data`. The log is capped at [`DIAGNOSTIC_LOG_CAP`] rows, so this is a
    /// bounded copy of a list nothing reads at frame rate.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.model_ref().diagnostics.rows().cloned().collect()
    }

    /// `(retained rows, distinct failures ever recorded)`. The two differ once the cap has evicted.
    pub fn diagnostic_counts(&self) -> (usize, u64) {
        let m = self.model_ref();
        (m.diagnostics.len(), m.diagnostics.total())
    }

    /// Forget the retained rows (the player's "I've read these"). See [`DiagnosticLog::clear`] for
    /// why the numbering is deliberately *not* reset.
    pub fn clear_diagnostics(&self) {
        self.model_mut().diagnostics.clear();
    }

    /// Record an addon that **did not load** for a reason that never raised — a manifest entry the
    /// package does not ship, a document that will not parse, a dependency cycle, a hard
    /// dependency that is missing, a broken `Bindings.xml`.
    ///
    /// **Deliberately does not dispatch.** These are not Lua errors: nothing raised, there is no
    /// traceback, and handing them to `geterrorhandler()` would put non-errors through
    /// `_ERRORMESSAGE` and through every addon handler that ever replaces it — a divergence from
    /// the reference for no gain, since the reference's own answer to an unparseable document is a
    /// line in `FrameXML.log` and silence on screen. What 1495 changes is that the silence is no
    /// longer *total*: the failure is retained and readable, it just does not seize the screen.
    ///
    /// The caller still logs its own line; this is the retention half, not a replacement for it.
    pub fn report_load_failure(&self, msg: &str) {
        record_load_failure(&self.lua, msg);
    }

    /// Retain one non-fatal warning the HOST caught and has already logged — a loader warning off
    /// an addon's XML, carrying the `<Addon>/<file>` prefix only the caller knows (decision 2135).
    ///
    /// The retention half alone, exactly like [`Self::report_script_error`]: the caller writes its
    /// own `warn!` line, and putting the message on [`Model::warnings`] as well would double it at
    /// the app's next per-frame drain. For a warning the ENGINE raises, the door is
    /// [`Model::record_warning`], which owns both halves.
    pub fn report_warning(&self, msg: &str) {
        record_warning(&self.lua, msg);
    }
}

/// **THE rule for "an addon file did not load"** — one implementation, both loaders (decision
/// 2107).
///
/// The startup walk reaches it through [`super::UiScript::report_load_failure`]; `LoadAddOn`'s
/// demand load reaches it directly, because it runs inside a Lua binding and holds only `&Lua`
/// (1191 §4). That was the whole defect 2107 fixes: the two paths had **two copies** of the rule
/// and the demand-load copy classified a missing manifest entry as a script error, so an addon the
/// reference loads (logging `Couldn't open %s` and continuing) came back as a session failure here.
///
/// The reference's own shape: a failed open is **non-fatal** — `0x6edaa0` logs
/// `"Couldn't open %s"` (`0x846ff4`) to the log sink at severity 2 and returns null; every failure
/// leg "reports through the sink and returns normally, with no throw, no `longjmp` and no abort",
/// and the enclosing `.toc`/document keeps loading. Nothing on that path reaches the Lua error
/// handler, which is why this never dispatches (1495) and why 1450 made it a WARN: a broken
/// package is the *package's* defect, and ERROR means "the client is broken".
pub(crate) fn record_load_failure(lua: &Lua, msg: &str) {
    lua.app_data_mut::<Model>()
        .expect("model app_data set")
        .diagnostics
        .record(DiagnosticKind::Load, msg);
}

/// **THE rule for "something was accepted and did not do what it said"** — the retention half of
/// the warning channel, for a caller that holds `&Lua` (decision 2135).
///
/// Its `&UiScript` face is [`super::UiScript::report_warning`] and its engine-internal sibling is
/// [`Model::record_warning`]; the difference is only which of the two channels the caller still
/// owes. A host caller has already written its own log line and wants retention only; an engine
/// caller wants both.
pub(crate) fn record_warning(lua: &Lua, msg: &str) {
    lua.app_data_mut::<Model>()
        .expect("model app_data set")
        .diagnostics
        .record(DiagnosticKind::Warning, msg);
}

/// Register the error-log reads the `BenillaScriptLogFrame` polls.
///
/// **`Benilla`-prefixed because they are ours**, not 1.12's — the prefix rule as
/// `tests::reference_surface` enforces it (1254: a name we invent must not collide, a name the
/// reference owns must not be hidden). 1.12 has no error-log API to shadow, so there is nothing
/// here an addon could have meant by a bare name.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    // BenillaGetNumScriptErrors() -> retained, totalEverRecorded
    lua.globals().set(
        "BenillaGetNumScriptErrors",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok((model.diagnostics.len(), model.diagnostics.total()))
        })?,
    )?;
    // BenillaGetScriptErrorInfo(index) -> seq, kind, message, count   (1-based, oldest first;
    // nil for an out-of-range index, so a stale row index during a repaint reads as "gone"
    // rather than erroring inside the very frame that lists errors.)
    lua.globals().set(
        "BenillaGetScriptErrorInfo",
        lua.create_function(|lua, index: i64| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(row) = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.diagnostics.rows().nth(i))
            else {
                return Ok(MultiValue::new());
            };
            Ok(MultiValue::from_vec(vec![
                Value::Integer(row.seq as i64),
                lua.create_string(row.kind.tag())?.into_lua(lua)?,
                lua.create_string(&row.message)?.into_lua(lua)?,
                Value::Integer(i64::from(row.count)),
            ]))
        })?,
    )?;
    // BenillaClearScriptErrors()
    lua.globals().set(
        "BenillaClearScriptErrors",
        lua.create_function(|lua, ()| {
            lua.app_data_mut::<Model>()
                .expect("model app_data")
                .diagnostics
                .clear();
            Ok(())
        })?,
    )
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
        // 2135's whole subject. Before it, a warning existed for exactly as long as one terminal
        // line: `take_warnings` drained it and nothing kept a copy, so neither the player's
        // `/errors` window nor the addon survey could see one.
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

        // The drain is a drain; the log is a memory. A second frame does not lose the row.
        assert!(s.take_warnings().is_empty());
        assert!(s
            .diagnostics()
            .iter()
            .any(|d| d.message.contains("thisCVarDoesNotExist")));
    }

    #[test]
    fn a_text_sink_takes_bytes_rather_than_raising_on_them() {
        // Decision 2138, and 1193's rule finally applied at the boundary 1193 named. `strsub` is
        // byte-indexed by design, so a slice through a multi-byte character is a string an addon
        // really does hand to `SetText` — and mlua's `String` conversion raised on it, killing the
        // handler. A stray byte costs a glyph.
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

    #[test]
    fn clear_forgets_rows_but_not_the_numbering() {
        let mut log = DiagnosticLog::default();
        log.record(DiagnosticKind::Error, "a");
        log.record(DiagnosticKind::Error, "b");
        log.clear();
        assert_eq!(log.len(), 0);
        log.record(DiagnosticKind::Error, "c");
        assert_eq!(
            log.rows().next().unwrap().seq,
            3,
            "a number never names two different failures"
        );
    }
}
