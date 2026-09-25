//! **The read-back column — ask the VM instead of guessing at it.**
//!
//! [`super::survey`] answers *how many* addons work and *what row* each one died on. It cannot
//! answer the next question, which is the one every fix actually turns on: **why is this
//! particular global nil?** The report's `--why` opens the error text; nothing opened the *state*
//! the error was raised against.
//!
//! So this loads **one** addon exactly the way the survey loads it, drives the same session start,
//! and then evaluates Lua of the caller's choosing against the VM that is left standing.
//!
//! ## Two properties it has to have, and both are about not lying
//!
//! **The environment is the corpus's, never the selection's.** The registry every VM is seated
//! with, and the case-folded installed set dependency resolution consults, are built from the
//! *whole folder* — exactly as [`super::survey`] builds them — so probing `KLHThreatMeter` out of
//! a 219-addon root produces the same VM its row in the full run had. Building a registry of one
//! would quietly answer a different question: `GetAddOnInfo` is how AceAddon and AceLibrary find
//! their dependencies, and against a registry of one they take the "nothing is installed" path
//! (the fault [`super::survey`]'s registry comment records at corpus scale).
//!
//! **It stops at session start, before the render and use probes.** Those two drive input into the
//! addon's own frames — hover, click, drag — and standing up the method oracle writes sixteen
//! widgets and a global into the VM. A read taken after them answers "what is true once the
//! harness has finished poking it", which is not the state a player is in and not the state an
//! error was raised in. `ADDON_LOADED` → `VARIABLES_LOADED` → `PLAYER_LOGIN` →
//! `PLAYER_ENTERING_WORLD` + the ticks is the moment this reads, and it is the moment the session
//! column's errors come from.
//!
//! ## What it is worth
//!
//! It is a **debugger, not a measurement**. An eval can mutate the VM, so nothing here prints a
//! column and no number from a probe run belongs in a record — quote [`super::survey`] for that.

use std::collections::BTreeSet;
use std::path::Path;

use benilla_ui::script::UiScript;
use benilla_ui::toc::Toc;

/// One addon, loaded and asked.
#[derive(Debug, Clone, Default)]
pub struct ProbeOutcome {
    pub name: String,
    /// Load-time failures, verbatim — the same list [`super::AddonReport::errors`] carries.
    pub load_errors: Vec<String>,
    /// What its handlers raised while the session start was driven.
    pub session_errors: Vec<String>,
    /// `(chunk, answer)` per `--eval`, in the order asked. An answer is `= <value>` or
    /// `ERROR: <message>`; the chunk that raises does not stop the ones after it.
    pub answers: Vec<(String, String)>,
}

/// Wrap a caller's chunk so a raise comes back as a value instead of killing the probe.
///
/// **`pcall` + `tostring`, and only the FIRST result.** Multi-return would need `table.getn` or
/// `select`, and this VM is deliberately 5.0-shaped — a probe that depended
/// on which of those the dialect layer publishes would be an instrument with a dialect bug in it.
/// A caller that wants more concatenates its own string, which is what `..` is for.
fn wrapped(chunk: &str) -> String {
    format!(
        "local __ok, __v = pcall(function() {chunk} end)\n\
         if __ok then return \"= \" .. tostring(__v) else return \"ERROR: \" .. tostring(__v) end"
    )
}

/// One step of a probe run, in the order the caller asked for it.
///
/// A probe used to be a list of chunks alone, which cannot ask the question half of this corpus
/// is about: an addon's map, bar and tooltip behaviour is **hover-driven**, and the state a read
/// finds with the cursor parked off-screen is not the state a player is in. Cartographer's world
/// map is the case that forced this — its area label is hidden at every zone change and shown
/// again only by `WorldMapButton`'s `OnEnter`, so `WorldMapFrameAreaLabel:IsShown()` reads `nil`
/// in a cursor-less VM whether the hover works or not. Interleaving the move with the reads is
/// what tells those two apart.
#[derive(Debug, Clone)]
pub enum Step {
    /// Lua to evaluate against the VM as it stands.
    Eval(String),
    /// Move the cursor to `(x, y)` in UI units (y-up from the bottom-left), firing the real
    /// `OnLeave`/`OnEnter` pair exactly as [`UiScript::mouse_move`] does for the app.
    Mouse(f32, f32),
    /// Advance the engine one frame of `secs` — the `OnUpdate` pump. An addon that hooks a
    /// FrameXML `OnUpdate` (Cartographer secure-hooks `WorldMapButton_OnUpdate`) raises there and
    /// nowhere else, so a probe that never ticks reports it clean. The answer is what the tick
    /// raised, or `no errors`.
    Tick(f32),
}

/// Load `name` out of `root` the way the survey does, drive the session start, then run each
/// [`Step`] against the VM that is left.
///
/// `None` when the folder has no manifest — the same refusal [`super::survey`] makes by filtering.
pub fn probe(root: &Path, name: &str, steps: &[Step]) -> Option<ProbeOutcome> {
    let toc_path = super::manifest_path(root, name)?;
    // Decoded, not `read_to_string`'d, for the reason `survey_one` states: a cp1252 manifest read
    // as UTF-8 parses as an EMPTY toc, and an addon with no files reads as a clean pass.
    let toc = Toc::parse(&benilla_ui::source::decode(
        &std::fs::read(&toc_path).unwrap_or_default(),
    ));

    let (_, installed, registry) = super::corpus(root);

    let mut script = match UiScript::new() {
        Ok(s) => s,
        Err(e) => {
            return Some(ProbeOutcome {
                name: name.to_string(),
                load_errors: vec![format!("VM: {e}")],
                ..Default::default()
            })
        }
    };
    script.set_instruction_budget(super::ADDON_INSTRUCTION_BUDGET);
    script.set_screen_size(1024.0, 768.0);
    // `None` **saved-variable** roots: a probe must never read or write the director's real ones,
    // for the same reason the survey does not call `finish_ui_load` (1213 §4). The AddOns root is
    // passed, exactly as [`super::survey_one`] passes it — a probe VM that answers `MISSING` to
    // every `LoadAddOn` is not the VM the row came from.
    script.register_addons(registry, Some(root.to_path_buf()), None, None);
    super::seat_a_session(&mut script);
    let _ = crate::ui_script::load_default_ui(&script);

    let mut dep_order: Vec<super::LoadedDep> = Vec::new();
    super::load_dependencies(
        &mut script,
        root,
        &toc,
        &installed,
        &mut BTreeSet::new(),
        &mut dep_order,
    );

    let load_errors = super::load_addon_files(&script, root, name, &toc).errors;
    // Same stamp the survey applies, for the same reason: the registry must agree with the VM
    // about what is loaded, or `IsAddOnLoaded` and every dependency verdict answer for a session
    // that does not exist. The chain stamped itself as it loaded (2166); this is the surveyed one.
    script.mark_addon_loaded(name);
    let session_errors = super::drive_session_start(&mut script, name, &installed);

    let answers = steps
        .iter()
        .map(|step| match step {
            Step::Eval(chunk) => {
                let answer = script
                    .eval::<String>(&wrapped(chunk))
                    // A raise HERE is the wrapper failing to compile the caller's chunk — a syntax
                    // error in what they typed — not the chunk raising, which `pcall` already
                    // caught.
                    .unwrap_or_else(|e| format!("SYNTAX: {e}"));
                (chunk.clone(), answer)
            }
            Step::Tick(secs) => {
                let before = script.errors().len();
                script.tick(*secs);
                let raised: Vec<String> = script.errors().into_iter().skip(before).collect();
                (
                    format!("--tick {secs}"),
                    if raised.is_empty() {
                        "= no errors".to_string()
                    } else {
                        format!("= RAISED: {}", raised.join(" | "))
                    },
                )
            }
            Step::Mouse(x, y) => {
                // `resolve` first: the hit-test reads resolved rects, and a frame the addon
                // created or moved since the last resolve has none (`pointer`'s module doc).
                script.resolve();
                script.mouse_move(*x, *y);
                let focus = script
                    .hit_test_name(*x, *y)
                    .unwrap_or_else(|| "<nothing>".into());
                (format!("--mouse {x},{y}"), format!("= over {focus}"))
            }
        })
        .collect();

    Some(ProbeOutcome {
        name: name.to_string(),
        load_errors,
        session_errors,
        answers,
    })
}
