//! The in-game interface: the core, the player's own stock FrameXML as their chain's
//! `FrameXML.toc` lists it ([`reference_ui::core`]) and then its `Bindings.xml`
//! ([`load_core_bindings`]), then benilla's layer, [`LAYER_MANIFEST`], every entry of which is
//! ours and loads after the whole core.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::addons::Addon;
use super::reference_ui;

/// benilla's own interface, one layer on the stock one, relative to `assets/ui`.
pub(super) const LAYER_MANIFEST: &str = "layer.toc";

/// Whether this run loads the layer: always in a player build; a dev build boots the stock UI
/// alone under `WOW_STOCK_UI=1`.
pub(super) fn layer_enabled() -> bool {
    layer_enabled_by(
        crate::run_mode::dev_affordances(),
        std::env::var("WOW_STOCK_UI").ok().as_deref(),
    )
}

/// [`layer_enabled`] over its two facts: a dev build, and the switch's value.
pub(super) fn layer_enabled_by(dev_build: bool, stock_ui: Option<&str>) -> bool {
    !(dev_build && stock_ui == Some("1"))
}

/// Every entry in load order: the core's rows, none without an install, then the layer's.
#[cfg(test)]
pub(super) fn manifest_files() -> Vec<String> {
    let mut files = reference_ui::core()
        .map(|core| core.toc.files)
        .unwrap_or_default();
    files.extend(Addon::layer().toc.files);
    files
}

/// The entries that name a file we ship rather than one off the player's install: the layer's.
#[cfg(test)]
pub(super) fn shipped_manifest_files() -> Vec<String> {
    manifest_files()
        .into_iter()
        .filter(|f| !reference_ui::is_chain_entry(f))
        .collect()
}

/// Runs `UIParent_ManageFramePositions` after the interface has loaded, then the buff pass below.
fn bootstrap_positions(script: &UiScript) -> Vec<String> {
    if let Err(e) = script.run("UIParent_ManageFramePositions()") {
        error!("ui_script: managed-positions bootstrap: {e}");
        return vec![format!("managed-positions bootstrap: {e}")];
    }
    if let Err(e) = apply_buff_durations(script) {
        error!("ui_script: buff-duration layout: {e}");
        return vec![format!("buff-duration layout: {e}")];
    }
    Vec::new()
}

/// Runs `BuffButtons_UpdatePositions` at load, which `BuffFrame_OnLoad` never calls: until the
/// stock `UIOptionsFrame.lua`'s `VARIABLES_LOADED` arm runs it (l.206), the second buff row and
/// the debuff row keep `BuffFrame.xml`'s anchors whatever `SHOW_BUFF_DURATIONS` says.
pub(super) fn apply_buff_durations(script: &UiScript) -> Result<(), String> {
    script
        .run("if BuffButtons_UpdatePositions then BuffButtons_UpdatePositions() end")
        .map_err(|e| e.to_string())
}

/// What a screen-size change re-runs, from `extract::tick_script`'s resize arm: anchors follow the
/// screen, but not a size or seat computed from the old one. The managed pass re-wraps the open
/// bags, whose columns `updateContainerFrameAnchors` lays out by `GetScreenHeight()`.
/// Deviation: [`FULLSCREEN_QUADS_RESEAT`] re-sizes the full-screen quads, which the stock files
/// size only in their OnLoads, because a resize or `uiScale` change here keeps the loaded UI and
/// the blackout would stop short of the new screen. Host-side, not in the layer: it adapts the
/// engine's resizable window, and changes nothing the stock UI does at a fixed size.
pub(super) fn on_screen_resized(script: &UiScript) {
    let _ = script.run("if UIParent_ManageFramePositions then UIParent_ManageFramePositions() end");
    if let Err(e) = script.run(FULLSCREEN_QUADS_RESEAT) {
        error!("ui_script: full-screen quad re-seat: {e}");
    }
}

/// The sizing arithmetic of `WorldMapFrame_OnLoad` (`WorldMapFrame.lua:19-28`) and
/// `CinematicFrame_OnLoad` (`CinematicFrame.lua:6-21`), each existence-guarded: the glue screens
/// have neither. Below 4:3 the bars go back to `CinematicFrame.xml`'s declared 1024 x 128, where
/// the stock OnLoad leaves them.
const FULLSCREEN_QUADS_RESEAT: &str = r#"
if BlackoutWorld then
    local width = GetScreenWidth()
    local height = GetScreenHeight()
    if ( width / height < 4 / 3 ) then
        width = width * 1.25
        height = height * 1.25
    end
    BlackoutWorld:SetWidth( width )
    BlackoutWorld:SetHeight( height )
end
if UpperBlackBar and LowerBlackBar then
    local width = GetScreenWidth()
    local height = GetScreenHeight()
    local barWidth, blackBarHeight = 1024, 128
    if ( width / height > 4 / 3 ) then
        local desiredHeight = width / 2
        if ( desiredHeight > height ) then
            desiredHeight = height
        end
        barWidth = width
        blackBarHeight = ( height - desiredHeight ) / 2
    end
    UpperBlackBar:SetHeight( blackBarHeight )
    UpperBlackBar:SetWidth( barWidth )
    LowerBlackBar:SetHeight( blackBarHeight )
    LowerBlackBar:SetWidth( barWidth )
end
"#;

/// Runs a UI load in the counted sound-suppression scope, as `CGGameUI::Initialize` (`0x48fbf0`,
/// called at login `0x48f681` and `/reloadui` `0x495669`) runs its own: enter (`0x458f50`) at
/// `0x48fbfa`, leave (`0x458f60`) at `0x49016d`, and `PlaySoundByName` (`0x458030`) drops every
/// call in between. `PLAYER_LOGIN` fires inside it only on `/reloadui` (`0x490168`); a fresh login
/// fires it from the player's create (`0x5deb60` to `0x4908c0`), a cascade with its own scope
/// (`0x4908d5` to `0x490a56`). The stock `TargetFrame_OnHide` at load is one sound it swallows.
pub(super) fn silenced_ui_load<S: std::borrow::Borrow<UiScript>, R>(
    script: &mut S,
    body: impl FnOnce(&mut S) -> R,
) -> R {
    script.borrow().push_sound_suppression();
    let out = body(script);
    script.borrow().pop_sound_suppression();
    out
}

/// Loads the core, then the layer unless [`layer_enabled`] says no, then the load-time repairs:
/// [`load_ingame_ui`] without the addons, for the tests and [`crate::addon_harness`]. Returns every
/// failure; a loader error is tagged `"<Addon>/<file>: <error>"`.
pub(crate) fn load_default_ui(script: &UiScript) -> Vec<String> {
    // The client's CVar table first: the stock `UIOptionsFrame.xml` reads CVars in its OnLoads,
    // and a bare `UiScript::new()` has only `benilla-ui`'s own. A re-register only refreshes
    // defaults.
    script.register_cvars(crate::cvars::registered_pairs());
    let mut failures = silenced_ui_load(&mut &*script, |script| {
        let mut failures = load_core(script);
        if layer_enabled() {
            failures.extend(load_layer(script));
        }
        failures.extend(bootstrap_positions(script));
        failures
    });
    // A raise one call below the loader's own dispatch (an OnLoad that shows a frame whose OnShow
    // raises) lands in the VM's error list, not the loader's report; it is a load failure too.
    failures.extend(
        script
            .errors()
            .into_iter()
            .map(|e| format!("a handler raised during the load walk: {e}")),
    );
    failures
}

/// The core: every row of the chain's `FrameXML.toc`, in its order, each file in document order,
/// then the chain's `Bindings.xml`. A chain without the toc builds no stock interface: the runner
/// reports `"Couldn't open %s"` (`0x846ff4`) at severity 2 (`0x6edc3f`) and returns 0, which
/// `UI_Init` never reads (`0x48fff2`), going on to `Bindings.xml` and the addons. benilla goes on
/// the same way and reports it as a load failure, an `ERROR` line and a row in `/errors`.
pub(super) fn load_core(script: &UiScript) -> Vec<String> {
    let Some(core) = reference_ui::core() else {
        let e = format!(
            "{}: not found — the stock interface is not built",
            reference_ui::TOC
        );
        error!("ui_script: {e}");
        script.report_load_failure(&e);
        // Into the UI load's own record, before any banner (`0x6edc3f`).
        let mut log = benilla_ui::status::Status::default();
        log.report(
            benilla_ui::status::FAILURE,
            benilla_ui::status::missing(reference_ui::TOC, false),
        );
        script.report_load_status(log);
        let mut failures = vec![e];
        failures.extend(load_core_bindings(script));
        return failures;
    };
    let mut toc = benilla_ui::status::Status::default();
    let mut failures = core.load_files_into(script, &core.toc.files, &mut toc);
    let mut log = benilla_ui::status::Status::default();
    let banner = benilla_ui::status::toc_banner(reference_ui::TOC);
    toc.close_into(&mut log, script.framexml_debug(), banner);
    script.report_load_status(log);
    failures.extend(load_core_bindings(script));
    failures
}

/// The stock key-binding commands, `UI_Init`'s next step after the toc walk: the chain's
/// `Interface\FrameXML\Bindings.xml` (`0x842f9c`), when the chain has it (`0x48fff9`), through
/// the loader addons' files go through (`0x4b6f70`, called at `0x490018` here and `0x51f443` for
/// an addon). Every command benilla has is a row of this file, its body the file's own Lua.
pub(super) fn load_core_bindings(script: &UiScript) -> Vec<String> {
    let Some(bytes) = reference_ui::read(reference_ui::BINDINGS) else {
        warn!(
            "ui_script: {} is not in the patch chain — no key binding has a command",
            reference_ui::BINDINGS
        );
        return Vec::new();
    };
    match benilla_ui::bindings_xml::parse(&benilla_ui::source::decode(&bytes)) {
        Ok(bindings) => {
            script.register_bindings(&bindings);
            info!(
                "bindings: {} commands registered from {}",
                bindings.len(),
                reference_ui::BINDINGS
            );
            Vec::new()
        }
        // `"Couldn't parse XML in %s"` (`0x846fd8`) at severity 2 (`0x4b700c`).
        Err(e) => {
            let e = format!("{}: {e}", reference_ui::BINDINGS);
            error!("ui_script: {e}");
            script.report_load_failure(&e);
            vec![e]
        }
    }
}

/// The defaults, set 0, off the player's chain: `WTF\DefaultBindings.wtf` (`0x846e44`), which
/// `0x4b62b0` reads line by line (`0x4b6140`); empty when the chain lacks it.
pub(crate) fn default_bindings() -> Vec<benilla_ui::script::keybind::KeyBinding> {
    match reference_ui::read(reference_ui::DEFAULT_BINDINGS) {
        Some(bytes) => {
            benilla_ui::script::keybind::parse_bindings_wtf(&String::from_utf8_lossy(&bytes))
        }
        None => {
            warn!(
                "ui_script: {} is not in the patch chain — every command ships unbound",
                reference_ui::DEFAULT_BINDINGS
            );
            Vec::new()
        }
    }
}

/// The chain's `Bindings.xml` text, for the tests that hold benilla against it.
#[cfg(test)]
pub(crate) fn stock_bindings_file() -> Option<String> {
    reference_ui::read(reference_ui::BINDINGS)
        .map(|bytes| benilla_ui::source::decode(&bytes).into_owned())
}

/// The stock bindings on a VM a test built by hand: the defaults loaded live, as a first login's
/// account set is, and the core's commands.
#[cfg(test)]
pub(crate) fn load_stock_bindings(script: &mut UiScript) {
    script.set_default_bindings(default_bindings());
    script.load_binding_set(1);
    assert!(
        load_core_bindings(script).is_empty(),
        "the stock Bindings.xml loads"
    );
}

/// Every [`LAYER_MANIFEST`] entry, in order, after the whole core: the layer loads as more of the
/// stock load, with no `ADDON_LOADED` and no registry row, as FrameXML does (`0x48ffed`).
fn load_layer(script: &UiScript) -> Vec<String> {
    let layer = Addon::layer();
    layer.load_files(script, &layer.toc.files)
}

/// The in-game UI, the core, then the layer unless [`layer_enabled`] says no, then every
/// third-party addon, on entering the world. The reference loads both in `CGGameUI::Initialize`
/// (`0x48fbf0`), reached only from world entry (`0x401570` from `0x46c236`): `FrameXML.toc` at
/// `0x48ffed`, its addons at `0x4900a3` (`0x51f600`).
///
/// `identity`, `(realm, character)`, names the character's AddOn enable-state file, as the
/// reference keys `AddOns.txt` per character; `roster`, the realm's characters, decides an addon
/// this one has no row for. `version_check` is the persisted `checkAddonVersion`, since this VM
/// has no CVar table yet at load.
pub(crate) fn load_ingame_ui(
    script: &mut UiScript,
    identity: Option<&(String, String)>,
    roster: &[String],
    version_check: bool,
) -> Vec<String> {
    load_ingame_ui_with(script, identity, roster, version_check, layer_enabled())
}

/// [`load_ingame_ui`] with the layer decided by the caller.
pub(super) fn load_ingame_ui_with(
    script: &mut UiScript,
    identity: Option<&(String, String)>,
    roster: &[String],
    version_check: bool,
    layer: bool,
) -> Vec<String> {
    // Bounded, addons included: a chunk that never returns fails as a load error instead of
    // freezing the loading screen. The caller disarms the budget once the edge is done.
    script.set_instruction_budget(super::addons::LOAD_INSTRUCTION_BUDGET);
    let mut failures = load_core(script);
    if layer {
        failures.extend(load_layer(script));
    }
    failures.extend(bootstrap_positions(script));
    // Each addon's `ADDON_LOADED` fires as that addon finishes, as the reference does (`0x51f5ad`).
    failures.extend(super::addons::load_third_party(
        script,
        identity,
        roster,
        version_check,
    ));
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_script::reference_ui::fixture;

    /// A chain whose `FrameXML.toc` lists `rows`, each a `.lua` that appends its own letter to
    /// `LOAD_LOG`, and one `.xml` whose `<Script file=>` does the same.
    fn lay_letters(toc: &str) -> fixture::Guard {
        const B: &[u8] = b"LOAD_LOG = (LOAD_LOG or '') .. 'b'";
        const A: &[u8] = b"LOAD_LOG = (LOAD_LOG or '') .. 'a'";
        const X: &[u8] = br#"<Ui><Script file="x.lua"/></Ui>"#;
        const XL: &[u8] = b"LOAD_LOG = (LOAD_LOG or '') .. 'x'";
        fixture::lay(&[
            (reference_ui::TOC, Some(toc.as_bytes())),
            (r"Interface\FrameXML\b.lua", Some(B)),
            (r"Interface\FrameXML\a.lua", Some(A)),
            (r"Interface\FrameXML\x.xml", Some(X)),
            (r"Interface\FrameXML\x.lua", Some(XL)),
        ])
    }

    /// The core's load list is the chain's `FrameXML.toc`: every file line, in its order, under the
    /// toc's own directory; directives, comments and blank lines load nothing.
    #[test]
    fn the_cores_load_list_is_the_chain_tocs_rows_in_order() {
        let _chain =
            lay_letters("## Interface: 11200\r\n# a comment\r\nb.lua\r\n\r\nx.xml\r\na.lua\r\n");
        let core = reference_ui::core().expect("the laid toc");
        assert_eq!(
            core.toc.files,
            [
                r"Interface\FrameXML\b.lua",
                r"Interface\FrameXML\x.xml",
                r"Interface\FrameXML\a.lua"
            ]
        );
    }

    /// The load follows the toc: the same files under a reordered toc load in the new order, each
    /// `.xml` running its own `<Script file=>` in place.
    #[test]
    fn a_reordered_toc_reorders_the_load() {
        for (toc, want) in [
            ("b.lua\nx.xml\na.lua\n", "bxa"),
            ("a.lua\nb.lua\nx.xml\n", "abx"),
        ] {
            let _chain = lay_letters(toc);
            let s = UiScript::new().unwrap();
            let failures = load_core(&s);
            assert!(failures.is_empty(), "{toc:?}: {failures:#?}");
            assert_eq!(
                s.eval::<String>("return LOAD_LOG").unwrap(),
                want,
                "{toc:?}"
            );
        }
    }

    /// A chain without `FrameXML.toc` builds no stock interface and says so, once, as a load
    /// failure: the reference's runner reports `"Couldn't open %s"` at severity 2 (`0x6edc3f`).
    #[test]
    fn a_chain_without_the_toc_builds_no_core_and_says_so() {
        let _chain = fixture::lay(&[(reference_ui::TOC, None)]);
        assert!(reference_ui::core().is_none());
        let s = UiScript::new().unwrap();
        let failures = load_core(&s);
        assert_eq!(failures.len(), 1, "{failures:#?}");
        assert!(
            failures[0].contains(reference_ui::TOC) && failures[0].contains("not found"),
            "{failures:?}"
        );
        assert!(
            s.diagnostics().iter().any(|d| {
                d.kind == benilla_ui::script::diagnostics::DiagnosticKind::Load
                    && d.message.contains(reference_ui::TOC)
            }),
            "the player's /errors carries it"
        );
    }

    /// The core never reads our tree: a toc row that names one of the layer's files loads the
    /// chain's file of that name or reports it missing, never ours.
    #[test]
    fn the_core_never_loads_a_file_of_ours() {
        let ours = Addon::layer().toc.files;
        let row = ours
            .iter()
            .find(|f| f.as_str() == "ScrollTemplates.xml")
            .expect("the layer ships ScrollTemplates.xml");
        let toc = format!("{row}\n");
        let _chain = fixture::lay(&[
            (reference_ui::TOC, Some(toc.as_bytes())),
            (&format!(r"Interface\FrameXML\{row}"), None),
        ]);
        let s = UiScript::new().unwrap();
        let failures = load_core(&s);
        assert!(
            failures.len() == 1 && failures[0].contains("not found"),
            "{failures:#?}"
        );
        assert!(
            !s.eval::<bool>("return BenillaScrollBar_Step ~= nil")
                .unwrap(),
            "the core loaded the layer's own ScrollTemplates.xml"
        );
    }

    /// Against the player's own install: the core's load list is their `FrameXML.toc`'s file
    /// lines, read here byte by byte, in order, and every one resolves off their chain.
    #[test]
    fn the_cores_load_list_is_the_players_own_toc() {
        let _data = benilla_formats::wow_data_or_skip!();
        let bytes = reference_ui::read(reference_ui::TOC).expect("the player's FrameXML.toc");
        let text = String::from_utf8_lossy(&bytes);
        let want: Vec<String> = text
            .trim_start_matches('\u{feff}')
            .split("\r\n")
            .flat_map(|l| l.split('\n'))
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| format!(r"Interface\FrameXML\{l}"))
            .collect();
        let core = reference_ui::core().expect("the player's FrameXML.toc");
        assert_eq!(core.toc.files, want);
        assert!(want.len() >= 90, "only {} rows parsed", want.len());
        for row in &core.toc.files {
            assert!(
                reference_ui::read(row).is_some(),
                "FrameXML.toc lists {row}, which the chain does not hold"
            );
        }
    }

    /// Order shows: the frames registered for one event run in registration order (`SignalEvent`,
    /// `0x703e50`, walks its tail-appended list), so the toc's row order is their handlers' order.
    /// `ExhaustionTick` (`MainMenuBar.lua:13`) and `PlayerFrame` (`PlayerFrame.lua:17`) both take
    /// `PLAYER_UPDATE_RESTING`, and the toc lists `MainMenuBar.xml` above `PlayerFrame.xml`.
    #[test]
    fn two_stock_frames_on_one_event_run_in_the_tocs_order() {
        let _data = benilla_formats::wow_data_or_skip!();
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let toc = reference_ui::read(reference_ui::TOC).expect("the player's FrameXML.toc");
        let toc = String::from_utf8_lossy(&toc);
        let at = |leaf: &str| {
            toc.lines()
                .position(|l| l.trim() == leaf)
                .unwrap_or_else(|| panic!("FrameXML.toc lists {leaf}"))
        };
        assert!(
            at("MainMenuBar.xml") < at("PlayerFrame.xml"),
            "the stock toc's order"
        );
        let (mut s, failures) =
            super::super::layer_tests::production_load_with("event-order", false, "", |_| {});
        assert!(failures.is_empty(), "load failures: {failures:#?}");
        s.run(
            r#"EVENT_ORDER = {}
            for _, name in ipairs({ "PlayerFrame", "ExhaustionTick" }) do
                local who = name
                getglobal(who):SetScript("OnEvent", function() table.insert(EVENT_ORDER, who) end)
            end"#,
        )
        .unwrap();
        s.fire_event("PLAYER_UPDATE_RESTING", vec![]);
        let order: Vec<String> = s.eval("return EVENT_ORDER").unwrap();
        assert_eq!(order, ["ExhaustionTick", "PlayerFrame"]);
    }

    /// A shipped `.xml` the layer does not list never loads, with no error to say so.
    #[test]
    fn the_manifest_lists_every_shipped_file_and_nothing_else() {
        let mut listed = shipped_manifest_files();
        let mut shipped: Vec<String> = super::super::content::shipped_files()
            .filter(|f| f.ends_with(".xml"))
            .map(str::to_owned)
            .collect();
        listed.sort();
        shipped.sort();
        assert_eq!(listed, shipped);
    }

    /// [`reference_ui::is_chain_entry`] reads a path separator as a chain entry, so a shipped file
    /// in a subdirectory would be looked for on the player's install instead.
    #[test]
    fn every_file_we_ship_is_a_bare_name_so_a_path_can_only_mean_the_chain() {
        for name in super::super::content::shipped_files() {
            assert!(
                !reference_ui::is_chain_entry(name),
                "assets/ui is flat, but ships {name}: a separator makes it a file off the install"
            );
        }
    }

    /// A patched chain's `Bindings.xml`, rows reordered, a body edited, and the loader's skips.
    const PATCHED_BINDINGS: &[u8] = br#"<Bindings>
    <Binding name="TOGGLEBACKPACK" header="INTERFACE">Probe("bag")</Binding>
    <Binding name="JUMP" header="MOVEMENT">Probe("edited jump")</Binding>
    <Binding name="TOGGLESTATS" hidden="true" debug="true">Probe("debug")</Binding>
    <Binding name="EMPTYROW"></Binding>
    <Bindings><Binding name="NESTEDROW">Probe("nested")</Binding></Bindings>
    <Binding name="ITUNES_PLAYPAUSE" header="ITUNES_REMOTE" platform="mac">Probe("mac")</Binding>
</Bindings>"#;

    /// The core's commands are the chain's `Bindings.xml`, loaded after the toc's files (which
    /// see none) as `UI_Init` does (`0x48ffed`, then `0x490018`): its order, its headers and its
    /// bodies are what registers and runs, the defaults are the chain's `DefaultBindings.wtf`, and
    /// a debug, an empty, a nested and a Mac row are no commands.
    #[test]
    fn the_cores_commands_are_the_chain_bindings_xml() {
        const TOC_LUA: &[u8] =
            b"TOC_SAW = GetNumBindings() function Probe(n) PROBED = (PROBED or '') .. n end";
        let _chain = fixture::lay(&[
            (reference_ui::TOC, Some(b"a.lua\r\n")),
            (r"Interface\FrameXML\a.lua", Some(TOC_LUA)),
            (reference_ui::BINDINGS, Some(PATCHED_BINDINGS)),
            (
                reference_ui::DEFAULT_BINDINGS,
                Some(b"bind J JUMP\r\nbind B TOGGLEBACKPACK\r\nbind CTRL-Y TOGGLESTATS\r\n"),
            ),
        ]);
        let mut s = UiScript::new().unwrap();
        s.set_default_bindings(default_bindings());
        s.load_binding_set(1);
        let failures = load_core(&s);
        assert!(failures.is_empty(), "{failures:#?}");
        assert_eq!(s.eval::<i64>("return TOC_SAW").unwrap(), 0);
        let rows: Vec<Vec<String>> = s
            .eval(
                "local out = {} \
                 for i = 1, GetNumBindings() do out[i] = { GetBinding(i) } end \
                 return out",
            )
            .unwrap();
        let row = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            rows,
            [
                row(&["HEADER_INTERFACE"]),
                row(&["TOGGLEBACKPACK", "B"]),
                row(&["HEADER_MOVEMENT"]),
                row(&["JUMP", "J"]),
            ]
        );
        assert!(s.execute_binding("JUMP", true).unwrap());
        assert_eq!(s.eval::<String>("return PROBED").unwrap(), "edited jump");
        for skipped in ["TOGGLESTATS", "EMPTYROW", "NESTEDROW", "ITUNES_PLAYPAUSE"] {
            assert!(
                !s.execute_binding(skipped, true).unwrap(),
                "{skipped} is no command"
            );
        }
        assert_eq!(
            s.eval::<String>(r#"return GetBindingAction("CTRL-Y")"#)
                .unwrap(),
            "TOGGLESTATS",
            "a default key on a command the file never declared is still the key table's"
        );
    }

    /// A chain without `Bindings.xml` loads no command and fails nothing (`0x48fff9` skips it).
    #[test]
    fn a_chain_without_bindings_xml_declares_no_command() {
        let _chain = fixture::lay(&[
            (reference_ui::TOC, Some(b"")),
            (reference_ui::BINDINGS, None),
        ]);
        let s = UiScript::new().unwrap();
        assert!(load_core(&s).is_empty());
        assert_eq!(s.eval::<i64>("return GetNumBindings()").unwrap(), 0);
    }

    /// The install's own file read as the loader reads it: 219 commands once the nine debug rows
    /// are skipped, the Mac rows among them for the Mac build alone; the six `MOVEVIEW*` rows sit
    /// in an XML comment.
    #[test]
    fn the_installs_bindings_xml_reads_as_the_loader_reads_it() {
        benilla_formats::wow_data_or_skip!();
        let binds = benilla_ui::bindings_xml::parse(&stock_bindings_file().unwrap()).unwrap();
        assert_eq!(binds.len(), 219);
        assert_eq!(binds.iter().filter(|b| b.run_on_up).count(), 94);
        assert_eq!(binds.iter().filter(|b| b.header.is_some()).count(), 13);
        assert_eq!(binds.iter().filter(|b| b.hidden).count(), 3);
        assert_eq!(binds.iter().filter(|b| b.platform.is_some()).count(), 5);
        let names: Vec<&str> = binds.iter().map(|b| b.name.as_str()).collect();
        for gone in ["TOGGLESTATS", "RESETPERFORMANCEVALUES", "MOVEVIEWIN"] {
            assert!(!names.contains(&gone), "{gone}");
        }
        for kept in [
            "MOVEANDSTEER",
            "PITCHUP",
            "TURNORACTION",
            "TOGGLEUI",
            "RAIDTARGETNONE",
        ] {
            assert!(names.contains(&kept), "{kept}");
        }
        assert_eq!(binds[0].header.as_deref(), Some("MOVEMENT"));

        // Registered on this client, the PC build: every row but the five Mac ones, so the list
        // walks 211 visible commands and 12 headers, `ITUNES_REMOTE` riding on a Mac row.
        let mut s = UiScript::new().unwrap();
        load_stock_bindings(&mut s);
        assert_eq!(s.eval::<i64>("return GetNumBindings()").unwrap(), 223);
        assert!(!s.execute_binding("ITUNES_PLAYPAUSE", true).unwrap());
    }
}
