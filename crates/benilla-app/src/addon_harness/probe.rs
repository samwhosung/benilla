//! The read-back probe: load one addon as the survey does, drive the session start, then run
//! the caller's Lua, cursor moves and ticks against the VM left standing.
//!
//! The registry and installed set are built from the whole folder, as the survey builds them:
//! against a registry of one, AceAddon and AceLibrary find no dependencies. It reads the state
//! after `PLAYER_ENTERING_WORLD` and the ticks, before the render and use probes touch the VM.
//! An eval can mutate the VM, so a probe is a debugger; the survey is the measurement.

use std::collections::BTreeSet;
use std::path::Path;

use benilla_ui::script::UiScript;
use benilla_ui::toc::Toc;

/// One addon, loaded and asked.
#[derive(Debug, Clone, Default)]
pub struct ProbeOutcome {
    pub name: String,
    /// Load-time failures, verbatim, as [`super::AddonReport::errors`] carries them.
    pub load_errors: Vec<String>,
    /// What its handlers raised while the session start was driven.
    pub session_errors: Vec<String>,
    /// `(chunk, answer)` per step in order; an answer is `= <value>` or `ERROR: <message>`, and a
    /// raise does not stop the steps after it.
    pub answers: Vec<(String, String)>,
}

/// Wrap a chunk in `pcall` + `tostring` so a raise comes back as a value; only the first result
/// is returned, so the wrapper needs nothing beyond Lua 5.0.
fn wrapped(chunk: &str) -> String {
    format!(
        "local __ok, __v = pcall(function() {chunk} end)\n\
         if __ok then return \"= \" .. tostring(__v) else return \"ERROR: \" .. tostring(__v) end"
    )
}

/// One step of a probe run. Much addon state is hover-driven, so moves interleave with reads.
#[derive(Debug, Clone)]
pub enum Step {
    /// Lua to evaluate against the VM as it stands.
    Eval(String),
    /// Move the cursor to `(x, y)` in UI units (y-up from the bottom-left), firing the real
    /// `OnLeave`/`OnEnter` pair as [`UiScript::mouse_move`] does for the app.
    Mouse(f32, f32),
    /// Advance one `OnUpdate` frame of `secs`; the answer is what the tick raised, or `no errors`.
    Tick(f32),
}

/// Load `name` out of `root` the way the survey does, drive the session start, then run each
/// [`Step`] against the VM that is left; `None` when the folder has no manifest.
pub fn probe(root: &Path, name: &str, steps: &[Step]) -> Option<ProbeOutcome> {
    let toc_path = super::manifest_path(root, name)?;
    // Decoded: a cp1252 manifest read as UTF-8 parses as an empty toc and a false clean pass.
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
    // No saved-variable roots, so a probe never touches a player's saved variables; the AddOns
    // root is passed as the survey passes it, so `LoadAddOn` resolves.
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
    // The registry must agree with the VM, or `IsAddOnLoaded` answers for another session; the
    // dependency chain stamped itself as it loaded.
    script.mark_addon_loaded(name);
    let session_errors = super::drive_session_start(&mut script, name, &installed);

    let answers = steps
        .iter()
        .map(|step| match step {
            Step::Eval(chunk) => {
                let answer = script
                    .eval::<String>(&wrapped(chunk))
                    // Only a syntax error in the chunk reaches here; `pcall` caught any raise.
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
                // Resolve first: the hit-test reads resolved rects, which a new or moved frame
                // lacks until then.
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
