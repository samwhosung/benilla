//! The in-game interface: its manifest, [`MANIFEST`], and the boot split that loads it in two
//! phases. An entry with a path is the reference's own file off the player's patch chain, a bare
//! filename is one we ship, and the manifest's order is the load order across both.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::addons::Addon;
use super::reference_ui;

/// The built-in interface's manifest, relative to `assets/ui`.
pub(super) const MANIFEST: &str = "benilla.toc";

/// The manifest's entries in load order, from both stores.
#[cfg(test)]
pub(super) fn manifest_files() -> Vec<String> {
    Addon::builtin().toc.files
}

/// The manifest's entries that name a file we ship rather than one off the player's install.
#[cfg(test)]
pub(super) fn shipped_manifest_files() -> Vec<String> {
    manifest_files()
        .into_iter()
        .filter(|f| !reference_ui::is_chain_entry(f))
        .collect()
}

/// Runs `UIParent_ManageFramePositions` after the manifest has loaded, then installs the load-time
/// repairs below. Never after `Fonts.xml` alone: `UIParent.lua` defines the pass.
fn bootstrap_positions(script: &UiScript) -> Vec<String> {
    if let Err(e) = script.run("UIParent_ManageFramePositions()") {
        error!("ui_script: managed-positions bootstrap: {e}");
        return vec![format!("managed-positions bootstrap: {e}")];
    }
    if let Err(e) = install_durability_reseat(script) {
        error!("ui_script: durability re-seat hook: {e}");
        return vec![format!("durability re-seat hook: {e}")];
    }
    if let Err(e) = apply_buff_durations(script) {
        error!("ui_script: buff-duration layout: {e}");
        return vec![format!("buff-duration layout: {e}")];
    }
    if let Err(e) = install_chat_plate_guard(script) {
        error!("ui_script: chat plate guard: {e}");
        return vec![format!("chat plate guard: {e}")];
    }
    Vec::new()
}

/// Deviation: the stock `FCF_ChatTabFadeFinished` (`FloatingChatFrame.lua:989-992`) nils `oldAlpha`
/// even under a live hover, so leaving and re-entering within `CHAT_FRAME_FADE_TIME` (a flick to
/// the scroll buttons, outside the hover box, does it) stops the plate fading in (l.873) for the
/// rest of the session; this keeps `oldAlpha` while `hover` is set. Redefined, not wrapped, so a
/// `ReloadUI` re-run cannot stack.
pub(super) fn install_chat_plate_guard(script: &UiScript) -> Result<(), String> {
    script.run(CHAT_PLATE_GUARD).map_err(|e| e.to_string())
}

const CHAT_PLATE_GUARD: &str = r#"
if FCF_ChatTabFadeFinished then
    function FCF_ChatTabFadeFinished(chatTab, chatFrame)
        chatTab:Hide()
        if not chatFrame.hover then
            chatFrame.oldAlpha = nil
        end
    end
end
"#;

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
/// the blackout would stop short of the new screen.
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

/// Deviation: re-runs `UIParent_ManageFramePositions` after `DurabilityFrame`'s own OnEvent. The
/// pass seats the frame 20 further in while a side glyph shows (`UIParent.lua:1759-1768`), but the
/// stock frame runs it only from OnShow/OnHide, so a glyph appearing while the frame is shown
/// leaves the shield's overhang off the screen edge. Latched, so a `ReloadUI` cannot stack it; the
/// stock handler runs first, unchanged, on the `this`/`event`/`arg1` already set for the dispatch.
pub(super) fn install_durability_reseat(script: &UiScript) -> Result<(), String> {
    script.run(DURABILITY_RESEAT).map_err(|e| e.to_string())
}

const DURABILITY_RESEAT: &str = r#"
if DurabilityFrame and not DurabilityFrame.benillaReseat then
    DurabilityFrame.benillaReseat = 1
    local refOnEvent = DurabilityFrame:GetScript("OnEvent")
    DurabilityFrame:SetScript("OnEvent", function()
        if refOnEvent then refOnEvent() end
        UIParent_ManageFramePositions()
    end)
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

/// Loads every [`MANIFEST`] entry at once, then the load-time repairs. Production splits this
/// load at boot ([`load_font_registry`], [`load_ingame_ui`]), so the callers are the tests and
/// [`crate::addon_harness`]. Returns every failure; a loader error is tagged
/// `"<Addon>/<file>: <error>"`.
pub(crate) fn load_default_ui(script: &UiScript) -> Vec<String> {
    // The client's CVar table first: the stock `UIOptionsFrame.xml` reads CVars in its OnLoads,
    // and a bare `UiScript::new()` has only `benilla-ui`'s own. A re-register only refreshes
    // defaults.
    script.register_cvars(crate::cvars::registered_pairs());
    let mut failures = silenced_ui_load(&mut &*script, |script| {
        let mut failures = load_manifest(script, &Addon::builtin().toc.files);
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

/// Loads a slice of [`MANIFEST`] entries, each from its own store and one at a time, so the
/// manifest's order is the load order across both.
fn load_manifest(script: &UiScript, files: &[String]) -> Vec<String> {
    let builtin = Addon::builtin();
    let reference = reference_ui::addon(
        files
            .iter()
            .filter(|f| reference_ui::is_chain_entry(f))
            .cloned()
            .collect(),
    );
    let mut failures = Vec::new();
    for file in files {
        let from = if reference_ui::is_chain_entry(file) {
            &reference
        } else {
            &builtin
        };
        failures.extend(from.load_files(script, std::slice::from_ref(file)));
    }
    failures
}

/// The font-object registry alone, `Fonts.xml`, loaded at `Startup`.
/// Deviation: it loads before the login screen because benilla's glue screens share its glyph
/// atlas; the reference's glue has its own `GlueFonts.xml`. The font objects declared outside it
/// (FRIZQT 13, 14 and 20, un-outlined) repeat combinations it has, so the atlas plan loses nothing.
pub(crate) fn load_font_registry(script: &UiScript) -> Vec<String> {
    load_manifest(
        script,
        Addon::builtin().toc.files.get(..1).unwrap_or_default(),
    )
}

/// The in-game UI, every entry after `Fonts.xml`, then every third-party addon, on entering the
/// world. The reference loads both in `CGGameUI::Initialize` (`0x48fbf0`), reached only from world
/// entry (`0x401570` from `0x46c236`), its addons at `0x4900a3` (`0x51f600`).
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
    // Bounded, addons included: a chunk that never returns fails as a load error instead of
    // freezing the loading screen. The caller disarms the budget once the edge is done.
    script.set_instruction_budget(super::addons::LOAD_INSTRUCTION_BUDGET);
    let mut failures = load_manifest(
        script,
        Addon::builtin().toc.files.get(1..).unwrap_or_default(),
    );
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

    /// `Fonts.xml` must stay entry 0: [`load_font_registry`] loads entry 0 at startup and
    /// [`load_ingame_ui`] the rest.
    #[test]
    fn the_manifest_is_a_toc_that_starts_with_the_font_registry() {
        let toc = Addon::builtin().toc;
        assert_eq!(toc.interface_versions(), vec![11200]);
        assert_eq!(toc.directive("Title"), Some("benilla"));
        assert_eq!(
            toc.files.first().map(String::as_str),
            Some("Interface\\FrameXML\\Fonts.xml"),
            "the font registry is the manifest's first entry — the loader splits there"
        );
    }

    /// A shipped `.xml` the manifest does not list never loads, with no error to say so.
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

    /// Every chain entry the manifest names is in the player's 1.12 patch chain.
    #[test]
    fn every_chain_entry_resolves_off_the_players_install() {
        let _data = benilla_formats::wow_data_or_skip!();
        let chain: Vec<String> = manifest_files()
            .into_iter()
            .filter(|f| reference_ui::is_chain_entry(f))
            .collect();
        for entry in &chain {
            assert!(
                reference_ui::read(entry).is_some(),
                "benilla.toc sources {entry} off the patch chain, which does not hold it"
            );
        }
    }

    /// The stock pass (`UIParent.lua:1592-1775`) indexes these frames unguarded, so a file that
    /// runs it at load must follow every file it reads, as the reference's toc orders them.
    #[test]
    fn every_load_time_runner_of_the_managed_pass_follows_what_it_reads() {
        let files = manifest_files();
        let at = |leaf: &str| {
            files
                .iter()
                .position(|f| f.ends_with(&format!("\\{leaf}")))
                .unwrap_or_else(|| panic!("the manifest lists {leaf}"))
        };
        // What the pass reads by name (`UIParent.lua:1598-1775`), and the file that declares it.
        const READS: &[(&str, &str)] = &[
            (
                "MultiBarLeft / MultiBarRight / MultiBarBottomLeft",
                "MultiActionBars.xml",
            ),
            (
                "PetActionBarFrame + SlidingActionBarTexture0/1",
                "PetActionBarFrame.xml",
            ),
            ("ReputationWatchBar", "ReputationFrame.xml"),
            (
                "MainMenuExpBar / MainMenuBarMaxLevelBar / MainMenuBar",
                "MainMenuBar.xml",
            ),
            ("CastingBarFrame", "CastingBarFrame.xml"),
            ("QuestTimerFrame", "QuestTimerFrame.xml"),
            ("QuestWatchFrame", "QuestLogFrame.xml"),
            ("DurabilityFrame + its three glyphs", "DurabilityFrame.xml"),
            ("MinimapCluster", "Minimap.xml"),
            (
                "ChatFrame1 / ChatFrame2 (+ FCF_DockUpdate)",
                "FloatingChatFrame.xml",
            ),
            // The shapeshift arm (l.1705-1732): the stance bar's file declares these and also runs
            // the pass, so it must precede the other runner.
            (
                "ShapeshiftBarLeft / Middle / Right",
                "BonusActionBarFrame.xml",
            ),
        ];
        // Who runs it at load: an OnLoad, or the OnShow of a frame its OnLoad shows.
        const RUNNERS: &[(&str, &str)] = &[
            (
                "ShapeshiftBar_OnLoad → Show → OnShow",
                "BonusActionBarFrame.xml",
            ),
            ("WorldStateAlwaysUpFrame OnLoad", "WorldStateFrame.xml"),
        ];
        for (what, runner) in RUNNERS {
            for (name, read) in READS {
                if read == runner {
                    continue; // its own declarations precede its own OnLoad
                }
                assert!(
                    at(read) < at(runner),
                    "{runner} ({what}) loads at {} but reads {name}, declared by {read} at {} — \
                     the pass raises at load and never seats the frame it was run for. Move the \
                     runner below the read, as the reference's toc has it.",
                    at(runner),
                    at(read)
                );
            }
        }
    }

    #[test]
    fn nothing_declares_a_uiparent_child_before_uiparent_itself_loads() {
        let files = manifest_files();
        let at = files
            .iter()
            .position(|f| f == r"Interface\FrameXML\UIParent.xml")
            .expect("the manifest lists UIParent.xml");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
        for early in &files[..at] {
            // A chain entry is a stock file off the player's install, not one on disk here.
            if reference_ui::is_chain_entry(early) {
                continue;
            }
            let text = std::fs::read_to_string(dir.join(early)).unwrap();
            assert!(
                !text.contains(r#"parent="UIParent""#),
                "{early} loads before UIParent.xml (position {at}) but declares a \
                 UIParent child — the loader would warn and silently drop it. Move \
                 UIParent.xml up, or the declaration down."
            );
        }
    }
}
