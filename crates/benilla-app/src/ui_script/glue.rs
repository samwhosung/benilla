//! The stock glue screens, behind `WOW_GLUE_STOCK=1` in a dev build: the player's own
//! `GlueXML.toc` off the patch chain, run into a glue VM ([`UiScript::new_glue`]) that stands in
//! the world while no character is in it, where the boot VM otherwise stands. The native screens
//! stay the default and, under the switch, keep driving the network unseen.
//!
//! The reference's glue builder (`0x46a7b0`) resets the Lua state on a rebuild (`0x46a87b`; the
//! cold first build, `0x46a505`, passes 0 and skips it), registers the glue functions
//! (`0x46abb0`) and the 26 glue events (`0x46a88f`), opens `Logs\GlueXML.log` (`0x46a8ce`), then
//! checks `GlueXML.toc`'s signature (`0x46a90b`). Only a valid one (`0x6f10f0` answering 3) loads
//! the toc through the shared runner (`0x6edb90`, at `0x46a9b3`), and only when the loaded files'
//! MD5 matches the signature's (`0x46a9d3`) does `FRAMES_LOADED` fire (`0x46aa34`). Signature
//! results 0–2 (the table at `0x46aaac`) log their line, missing, corrupt or modified (`0x836fb8`,
//! `0x836f98`, `0x836f78`), and load nothing; an MD5 mismatch logs no line, its frames already
//! built, and skips `FRAMES_LOADED`. Either way the client quits with
//! error 9 (`0x401560`: the exit code, then the quit post `ForceQuit` makes with code 0).
//! Deviation: no signature or digest check (`0x46a90b`, `0x46a9d3`, `0x401560(9)`), so the chain's
//! toc always loads and `FRAMES_LOADED` follows, because the check verifies Blizzard's signature
//! with a key carried in the client binary, which benilla does not ship, and a modified interface
//! file is loaded rather than refused.
//!
//! The screen comes after the build: at startup the reference fires `SET_GLUE_SCREEN` with
//! `"movie"` while the `movie` CVar is 1 (clearing it), else `"login"` (`0x402b3b`–`0x402b65`);
//! back from the world `0x402090` fires it with `"charselect"` or `"login"`. benilla plays no movie
//! yet (#968), so its boot screen is `"login"`.
//!
//! The screens' `ModelFFX` panes drive the glue booth ([`super::glue_scene`]): its scene rig, the
//! scene M2's own point lights and the `FfxGlow::GLUE_SCENE` pass, never the `ui_models` tile
//! path, where every lit batch draws black.

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use super::reference_ui;

/// The switch's variable.
const SWITCH: &str = "WOW_GLUE_STOCK";

/// Whether this run puts the stock glue screens up, read once at plugin build: a dev build with
/// `WOW_GLUE_STOCK=1`. A player build never does.
#[derive(Resource, Default, Clone, Copy)]
pub(crate) struct GlueStock(pub(crate) bool);

impl GlueStock {
    /// The switch as this process has it.
    pub(crate) fn from_env() -> Self {
        Self(
            crate::run_mode::dev_affordances()
                && std::env::var(SWITCH).ok().as_deref() == Some("1"),
        )
    }
}

/// Run condition: the stock glue screens are up, the switch on and no character in the world.
pub(crate) fn glue_ui_up(
    stock: Option<Res<GlueStock>>,
    state: Option<Res<State<crate::char_select::ClientState>>>,
) -> bool {
    stock.is_some_and(|s| s.0)
        && state.is_none_or(|s| *s.get() != crate::char_select::ClientState::InWorld)
}

/// Run condition: the switch is on, whether or not a character is in the world.
pub(crate) fn stock_glue(stock: Option<Res<GlueStock>>) -> bool {
    stock.is_some_and(|s| s.0)
}

/// Run condition: the native glue screens draw and take input, the switch off.
pub(crate) fn native_glue(stock: Option<Res<GlueStock>>) -> bool {
    !stock.is_some_and(|s| s.0)
}

/// The glue screen `SET_GLUE_SCREEN` names, `GlueParent.lua`'s `GlueScreenInfo` key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GlueScreen {
    Login,
    CharSelect,
}

impl GlueScreen {
    /// The key, as the reference's literals spell it (`0x82e3e0`, `0x82e3e8`).
    pub(crate) fn key(self) -> &'static str {
        match self {
            GlueScreen::Login => "login",
            GlueScreen::CharSelect => "charselect",
        }
    }
}

/// Load the chain's `GlueXML.toc` into `script`, a glue VM, and fire `FRAMES_LOADED`: the glue
/// builder's steps from the toc on (`0x46a9b3`, `0x46aa34`). Returns every failure; a loader error
/// is tagged `"GlueXML/<file>: <error>"`, a handler that raised during the walk is one too. The
/// load's record is left for [`finish_glue_load_log`].
pub(super) fn load_glue(script: &mut UiScript) -> Vec<String> {
    // The builder's sweep before the checks and the toc (`0x46a8eb`).
    super::addons::sweep_loose_interface();
    let mut failures = match reference_ui::glue() {
        Some(glue) => {
            let mut toc = benilla_ui::status::Status::default();
            let failures = glue.load_files_into(script, &glue.toc.files, &mut toc);
            let mut log = benilla_ui::status::Status::default();
            let banner = benilla_ui::status::toc_banner(reference_ui::GLUE_TOC);
            toc.close_into(&mut log, script.framexml_debug(), banner);
            script.report_load_status(log);
            failures
        }
        None => {
            // The runner's `"Couldn't open %s"` (`0x846ff4`) at severity 2; no glue frame exists.
            let e = format!(
                "{}: not found — the stock glue screens are not built",
                reference_ui::GLUE_TOC
            );
            error!("ui_script: {e}");
            script.report_load_failure(&e);
            let mut log = benilla_ui::status::Status::default();
            log.report(
                benilla_ui::status::FAILURE,
                benilla_ui::status::missing(reference_ui::GLUE_TOC, false),
            );
            script.report_load_status(log);
            vec![e]
        }
    };
    if super::manifest::layer_enabled() {
        failures.extend(load_glue_layer(script));
    }
    script.fire_event("FRAMES_LOADED", vec![]);
    failures.extend(
        script
            .errors()
            .into_iter()
            .map(|e| format!("a handler raised during the glue load: {e}")),
    );
    failures
}

/// benilla's one glue file, its realmlist control over the stock `AccountLogin`, which loads after
/// every `GlueXML.toc` row and before `FRAMES_LOADED`, as the in-game layer loads after
/// `FrameXML.toc` ([`super::manifest::LAYER_MANIFEST`]); `WOW_STOCK_UI=1` drops it with that layer.
pub(super) const GLUE_LAYER: &str = "AccountLoginRealmlist.xml";

/// Load [`GLUE_LAYER`] off the shipped tree; returns its failures.
fn load_glue_layer(script: &UiScript) -> Vec<String> {
    let files = vec![GLUE_LAYER.to_string()];
    let layer = super::addons::Addon::new(
        "benilla".to_string(),
        benilla_ui::toc::Toc {
            directives: Vec::new(),
            files: files.clone(),
        },
        super::addons::Source::Builtin,
    );
    layer.load_files(script, &files)
}

/// Drain the glue load's record into `Logs\GlueXML.log`, as the builder does after
/// `FRAMES_LOADED` (`0x46aa61`-`0x46aa94`).
pub(super) fn finish_glue_load_log(script: &mut UiScript) {
    script.finish_ui_load_log();
    super::load_log::write_glue(script.take_load_log_writes());
}

/// Put `screen` up: `SET_GLUE_SCREEN` with its key as `arg1` (`0x46b840`, event 0).
pub(super) fn set_glue_screen(script: &mut UiScript, screen: GlueScreen) {
    script.fire_event(
        "SET_GLUE_SCREEN",
        vec![ScriptValue::Str(screen.key().to_string())],
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The glue functions the stock GlueXML calls that benilla has not built yet: each one's call
    /// raises "attempt to call global `X' (a nil value)", at the load or on the login screen. The
    /// list only shrinks: a name built, or no longer reached, comes off it.
    const UNBUILT_CALLED: &[&str] = &[];

    /// What the player's own files make the reference report too: their `tos.html` uses `<H4>`,
    /// which 1.12's SimpleHTML does not know (`"Frame %s: Unknown element type: %s"`,
    /// `0x87a95c`, and the walk goes on).
    const INSTALL_WARNINGS: &[&str] = &["Frame TOSText: Unknown element type: H4"];

    /// The frames each stock glue screen and dialog is, which the load must build.
    const SCREENS: [&str; 8] = [
        "GlueParent",
        "AccountLogin",
        "CharacterSelect",
        "CharacterCreate",
        "RealmList",
        "GlueDialog",
        "RealmWizard",
        "AddonList",
    ];

    /// The name a "nil value" call error names, in either of Lua's two spellings of it.
    fn nil_call(error: &str) -> Option<&str> {
        let (_, rest) = error.split_once("attempt to call ")?;
        let (kind, rest) = rest.split_once(' ')?;
        if !matches!(kind, "global" | "field" | "method") {
            return None;
        }
        let rest = rest.strip_prefix('`').or_else(|| rest.strip_prefix('\''))?;
        let (name, rest) = rest.split_once('\'')?;
        rest.starts_with(" (a nil value)").then_some(name)
    }

    /// The glue state's functions, `reference/glue_surface.tsv`'s names.
    fn glue_surface() -> BTreeSet<String> {
        let path = format!(
            "{}/../../reference/glue_surface.tsv",
            env!("CARGO_MANIFEST_DIR")
        );
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {path}: {e}"))
            .lines()
            .skip(1)
            .filter_map(|l| l.split('\t').next().map(str::to_string))
            .collect()
    }

    #[test]
    fn a_nil_call_names_its_function() {
        assert_eq!(
            nil_call("x.lua:1: attempt to call global `GetBuildInfo' (a nil value)"),
            Some("GetBuildInfo")
        );
        assert_eq!(
            nil_call("x.lua:1: attempt to call field 'Foo' (a nil value)"),
            Some("Foo")
        );
        assert_eq!(
            nil_call("x.lua:1: attempt to index global `X' (a nil value)"),
            None
        );
        assert_eq!(nil_call("x.lua:1: bad argument #1"), None);
    }

    /// The stock GlueXML loads into the glue VM, `FRAMES_LOADED` fires and the login screen goes
    /// up, and a few frames run: every screen's frame exists, and the only Lua errors are calls
    /// of the glue functions [`UNBUILT_CALLED`] names, each of which is still unbuilt and still
    /// called. Any other error, an XML problem or a missing widget method, fails here.
    #[test]
    fn the_stock_glue_loads_with_only_unbuilt_verbs_raising() {
        let _ = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new_glue().unwrap();
        s.set_screen_size(1024.0, 768.0);
        let mut errors = load_glue(&mut s);
        // `load_glue` reports the walk's handler errors; the rest are the screen's and the ticks'.
        let _ = s.take_errors();
        set_glue_screen(&mut s, GlueScreen::Login);
        for _ in 0..3 {
            s.tick(0.1);
        }
        s.resolve();
        errors.extend(s.take_errors());
        // Every row the session retained: the load's failures and warnings, and what raised.
        errors.extend(s.diagnostics().into_iter().map(|d| d.message));

        let surface = glue_surface();
        let mut called = BTreeSet::new();
        let other: Vec<&String> = errors
            .iter()
            .filter(|e| {
                if let Some(name) = nil_call(e).filter(|n| UNBUILT_CALLED.contains(n)) {
                    called.insert(name);
                    return false;
                }
                if INSTALL_WARNINGS.contains(&e.as_str()) {
                    return false;
                }
                true
            })
            .collect();
        assert!(
            other.is_empty(),
            "the glue load raised {} error(s) or warning(s) that are not a call of an unbuilt \
             glue function on UNBUILT_CALLED:\n{}",
            other.len(),
            other
                .iter()
                .map(|e| e.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
        let listed: BTreeSet<&str> = UNBUILT_CALLED.iter().copied().collect();
        let stale: Vec<&&str> = listed.difference(&called).collect();
        assert!(
            stale.is_empty(),
            "UNBUILT_CALLED names no longer called unbuilt; take them off: {stale:?}"
        );
        for name in UNBUILT_CALLED {
            assert!(
                surface.contains(*name),
                "{name} is not a glue-state function"
            );
            assert!(
                s.eval::<bool>(&format!("return {name} == nil")).unwrap(),
                "{name} is built; take it off UNBUILT_CALLED"
            );
        }

        for name in SCREENS {
            assert!(
                s.eval::<bool>(&format!(
                    "return type({name}) == 'table' and {name}.GetObjectType ~= nil"
                ))
                .unwrap(),
                "the glue load built no frame {name}"
            );
        }
        assert_eq!(
            s.eval::<(bool, bool)>(
                "return AccountLogin:IsShown() == 1, CharacterSelect:IsShown() == 1"
            )
            .unwrap(),
            (true, false),
            "SET_GLUE_SCREEN \"login\" shows the login screen alone"
        );
    }

    /// The create screen's three info panels raise their icon frame a level in its `OnLoad`
    /// (`CharacterCreate.xml:238`, `:399`, `:560`), above the ScrollFrame beside it. The scroll
    /// child draws in that ScrollFrame's HIGHLIGHT callback (`0x787220`), so the description text
    /// draws under the icon it runs past, not at the scroll child's own level, which ties the icon
    /// and would put the text on top.
    #[test]
    fn the_create_panels_icons_cover_their_scrolled_text() {
        let _ = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new_glue().unwrap();
        s.set_screen_size(1024.0, 768.0);
        let _ = load_glue(&mut s);
        set_glue_screen(&mut s, GlueScreen::Login);
        // Shown alone: the screen's art and text need no create catalog.
        s.run("AccountLogin:Hide() CharacterCreate:Show()").unwrap();
        let _ = s.take_errors();
        s.resolve();
        let quads = s.extract();
        let owned = |name: &str| {
            quads
                .iter()
                .filter(|q| s.quad_owner_name(q.target).as_deref() == Some(name))
                .collect::<Vec<_>>()
        };
        for panel in ["Faction", "Race", "Class"] {
            let icon = owned(&format!("CharacterCreate{panel}IconFrame"));
            let icon = icon
                .iter()
                .find(|q| matches!(q.content, benilla_ui::script::QuadContent::Texture { .. }))
                .unwrap_or_else(|| panic!("the {panel} icon draws"));
            let text = owned(&format!("CharacterCreate{panel}ScrollChild"));
            let text: Vec<_> = text
                .iter()
                .filter(|q| matches!(q.content, benilla_ui::script::QuadContent::Text { .. }))
                .collect();
            assert!(!text.is_empty(), "the {panel} panel's text draws");
            let icon_rect = icon.rect.expect("the icon is placed");
            assert!(
                text.iter().any(|q| q
                    .rect
                    .is_some_and(|r| r.left < icon_rect.right && r.top > icon_rect.bottom)),
                "some {panel} text runs under the icon's corner"
            );
            for q in text {
                assert!(q.z < icon.z, "the {panel} text draws under the raised icon");
            }
        }
    }

    /// The stock credits screen (`CreditsFrame.xml`). Its two fades are a black `<Color>` under a
    /// vertical alpha `<Gradient>`, opaque at the scroll frame's edge and clear inward, so the
    /// credits text fades instead of being cut. Its borders are file art in the BACKGROUND layer
    /// beside the solid black bars, and the layer batch's sort (`0x7731a0`) draws them over.
    #[test]
    fn the_stock_credits_fades_ramp_and_its_borders_cover_the_bars() {
        use benilla_ui::script::QuadContent;
        let _ = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new_glue().unwrap();
        s.set_screen_size(1024.0, 768.0);
        let _ = load_glue(&mut s);
        set_glue_screen(&mut s, GlueScreen::Login);
        s.run("AccountLogin:Hide() CreditsFrame:Show()").unwrap();
        let _ = s.take_errors();
        s.resolve();
        let quads = s.extract();
        let black = Some([0.0, 0.0, 0.0, 1.0]);

        let (clear, opaque) = ([0.0, 1.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.0]);
        let mut fades: Vec<_> = quads
            .iter()
            .filter_map(|q| match &q.content {
                QuadContent::Texture {
                    path: None,
                    color,
                    gradient: Some(g),
                    ..
                } if *color == black => Some((q.rect.expect("a fade is placed"), *g)),
                _ => None,
            })
            .collect();
        fades.sort_by(|a, b| a.0.bottom.total_cmp(&b.0.bottom));
        assert_eq!(fades.len(), 2, "both fades draw");
        assert_eq!(
            fades[0].1,
            [clear, clear, opaque, opaque],
            "the bottom fade: opaque along the bottom, clear at its top"
        );
        assert_eq!(
            fades[1].1,
            [opaque, opaque, clear, clear],
            "the top fade: opaque along the top, clear at its bottom"
        );

        let owned_by_credits = |q: &&benilla_ui::script::ExtractedQuad| {
            s.quad_owner_name(q.target).as_deref() == Some("CreditsFrame")
        };
        let bars: Vec<u128> = quads
            .iter()
            .filter(owned_by_credits)
            .filter(|q| {
                matches!(&q.content, QuadContent::Texture { path: None, color, gradient: None, .. }
                    if *color == black)
            })
            .map(|q| q.z)
            .collect();
        let borders: Vec<u128> = quads
            .iter()
            .filter(owned_by_credits)
            .filter(|q| {
                matches!(&q.content, QuadContent::Texture { path: Some(p), .. }
                    if p.ends_with("Credits\\Border1"))
            })
            .map(|q| q.z)
            .collect();
        assert_eq!((bars.len(), borders.len()), (2, 2));
        assert!(
            borders.iter().all(|b| bars.iter().all(|bar| b > bar)),
            "the borders draw over the black bars: {borders:?} over {bars:?}"
        );
    }

    /// `GlueParent` fills the screen root, 768 tall and `768·a` wide (`0x41ad10`, from `0x46ab40`),
    /// up to 16:9; past that `GlueParent_OnLoad` centres it 16:9 wide on the root's height.
    #[test]
    fn glue_parent_fills_the_root_up_to_sixteen_by_nine() {
        let _ = benilla_formats::wow_data_or_skip!();
        let rect = |aspect: f32| {
            let mut s = UiScript::new_glue().unwrap();
            s.set_screen_size(768.0 * aspect, 768.0);
            let _ = load_glue(&mut s);
            s.resolve();
            s.eval::<(f32, f32, f32, f32)>(
                "return GlueParent:GetLeft(), GlueParent:GetRight(), GlueParent:GetBottom(), \
                 GlueParent:GetTop()",
            )
            .unwrap()
        };
        let near = |a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)| {
            [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)]
                .iter()
                .all(|(x, y)| (x - y).abs() < 0.01)
        };
        for aspect in [4.0 / 3.0, 16.0 / 10.0, 16.0 / 9.0] {
            let got = rect(aspect);
            assert!(
                near(got, (0.0, 768.0 * aspect, 0.0, 768.0)),
                "{aspect}: {got:?}"
            );
        }
        let wide = 21.0 / 9.0;
        let bar = (768.0 * wide - 768.0 * 16.0 / 9.0) / 2.0;
        let got = rect(wide);
        assert!(
            near(got, (bar, 768.0 * wide - bar, 0.0, 768.0)),
            "21:9: {got:?}"
        );
    }
}
