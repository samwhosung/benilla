//! **The in-game interface** — its manifest, and the boot split that loads it in two phases.
//!
//! The *how* of loading any interface lives in [`super::addons`]: an [`Addon`] is a name, a parsed
//! `.toc`, and a source its files come from (decision 1184). What is specific to the default UI,
//! and therefore still here, is the **seam at index 0** — see [`load_default_ui`] — and the
//! **two sources one manifest names**, see [`load_manifest`].
//!
//! The manifest itself is not here either. It is [`MANIFEST`] — `assets/ui/benilla.toc`, an
//! ordinary addon manifest read by the ordinary `.toc` parser ([`benilla_ui::toc`]), exactly as a
//! third-party addon's is (decision 1178). Until then it was a hand-ordered `&[&str]` in this file,
//! which meant our own interface loaded by a private door and the addon path was untested by
//! construction.
//!
//! ## One ordered list, two stores (decision 1751)
//!
//! The end state for this interface is the stock 1.12 FrameXML run off the player's own install;
//! `assets/ui` is scaffolding that retires file by file ([`super::reference_ui`], whose header is
//! the rule). A manifest entry carrying a **path** is sourced off the chain, a **bare filename** is
//! one we ship — so the migration of a window is one line changing in `benilla.toc` plus the
//! deletion of our copy, and the manifest stays the single ordered truth of what loads when. That
//! ordering is the point: stock `ContainerFrame.xml` inherits four templates our earlier files
//! declare, so "source the reference first" — the only order a Lua-only mechanism could express —
//! is not a position it can load at.

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::addons::Addon;
use super::reference_ui;

/// The built-in interface's manifest, relative to `assets/ui`. Its `## Interface:`/`## Title:`
/// directives are what `GetAddOnInfo` will read once the AddOn API lands (1178 step 4); nothing
/// consumes them yet.
pub(super) const MANIFEST: &str = "benilla.toc";

/// The manifest's file list, in load order — a convenience over [`Addon::builtin`] for the tests
/// and the content sweep, which want the names rather than a loader. **Both stores**: filter with
/// [`reference_ui::is_chain_entry`] for one or the other.
#[cfg(test)]
pub(super) fn manifest_files() -> Vec<String> {
    Addon::builtin().toc.files
}

/// The manifest's entries that name a file **we ship** — everything [`reference_ui`] does not
/// source off the player's install. This is what a check about `assets/ui` wants.
#[cfg(test)]
pub(super) fn shipped_manifest_files() -> Vec<String> {
    manifest_files()
        .into_iter()
        .filter(|f| !reference_ui::is_chain_entry(f))
        .collect()
}

/// Run decision 0272's load-time `UIParent_ManageFramePositions()` pass.
///
/// Only meaningful once the frames that table names exist, so the font-registry-only load
/// ([`load_font_registry`]) skips it. It is defined in the stock `UIParent.lua`, which is in the deferred
/// half; calling it after `Fonts.xml` alone is a nil-global error, not a no-op.
///
/// The ref applies `UIPARENT_MANAGED_FRAME_POSITIONS` once at load, then re-fires from the bottom
/// bars' OnShow/OnHide. Every frame the table names exists by the time this runs, so this is that
/// load-time application; the stance bar's show/hide handles the rest at runtime.
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

/// **A stated repair of a reference defect, installed rather than edited in** (decision 1998):
/// the chat plate's hover fade must survive a quick exit and re-entry.
///
/// The stock `FCF_OnUpdate` (FloatingChatFrame.lua l.809-987) keeps per-window state across
/// ticks: `hover` (the mouse is over the window), `oldAlpha` (the alpha the plate returns to, and
/// the gate the fade-in needs — `oldAlpha < DEFAULT_CHATFRAME_ALPHA`), and `hasBeenFaded`. Two of
/// its arms disagree about who owns them. The leave arm clears `hover` only inside the textures'
/// fade-out condition (l.913-918); the tabs' fade-out is queued with `FCF_ChatTabFadeFinished`
/// as its `finishedFunc` (l.931/973), which fires `CHAT_FRAME_FADE_TIME` (0.15 s) later and sets
/// `oldAlpha = nil` (l.991) — unconditionally. Re-enter the window inside that 0.15 s and the
/// re-entry's hover-start arm has already run (it keeps `oldAlpha`, still valid); then the tab's
/// fade completes, `oldAlpha` goes nil under a live hover, the plate arm (`chatFrame.oldAlpha
/// and chatFrame.oldAlpha < DEFAULT_CHATFRAME_ALPHA`, l.873) never passes again, and every later
/// leave skips the arm that would clear `hover` — so the hover-start re-read of `oldAlpha` (l.907)
/// never runs either. The tab keeps fading in; the plate never does, for the rest of the session.
/// `FCF_SetWindowAlpha` (the tab menu's opacity slider) reseats `oldAlpha` and is the one way
/// out, which is the shape the director reported: an existing window shows no plate on hover, a
/// new window (opened at `DEFAULT_CHATFRAME_ALPHA`, no fade needed) shows one, and "setting the
/// background to zero once" makes the hover work from then on. The gesture that arms it is
/// ordinary: the scroll buttons sit 32 units outside the window's left edge
/// (`FCF_SetButtonSide`, l.1038) and the hover box reaches only 5 (`MouseIsOver(chatFrame, 45,
/// -10, -5, 5)`), so a flick from the text to a scroll button and back does it.
///
/// The repair is the smallest one: the tab's finished callback leaves `oldAlpha` alone while the
/// window is hovered (`hover` set), and behaves as the reference's when it is not. The Lua is the
/// reference's own and the engine verbs it uses (`GetCursorPosition`, a texture's `GetAlpha`, the
/// OnUpdate `elapsed`) are settled, so the trap is inferred to be 1.12's as well — a client-side
/// A/B is the director's to run (`./run-ref-client.sh`; the record has the script). Installed from
/// Rust for the durability hook's reasons above: `assets/ui` does not grow (1779), and a repair of
/// a reference defect is not a `ContainerFrameAdapters`-class engine-difference shim (1751 §2).
///
/// Re-stated rather than wrapped (the reference body is two lines), so re-running it after a
/// `ReloadUI` re-defines the same function instead of stacking. Guarded on the function's
/// presence: the font-registry-only load has no chat files.
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

/// **Apply `SHOW_BUFF_DURATIONS` to the buff bar once, at load** (1751 window 18).
///
/// The reference's `BuffFrame_OnLoad` seats the duration FontStrings and nothing else — the row
/// pitch and the debuff row's anchor come from `BuffButtons_UpdatePositions`, which it never calls.
/// In 1.12 that is applied by `UIOptionsFrame.lua`, the file that owns the setting; we have no
/// counterpart to it (our row lives on `OptionsFrame.xml`, whose `applyFunc` fires on a CHANGE),
/// so without this the bar ships laid out for durations-OFF while the setting says on — a 10px
/// pitch error on every buff and a debuff row anchored to the wrong frame.
///
/// Here rather than in a FrameXML file for the same two reasons as the durability hook above:
/// `assets/ui` does not grow (1779), and this is our stand-in for a file the reference has and we
/// do not, which makes Rust the durable home.
///
/// Guarded, because the font-registry-only load has no buff bar.
pub(super) fn apply_buff_durations(script: &UiScript) -> Result<(), String> {
    script
        .run("if BuffButtons_UpdatePositions then BuffButtons_UpdatePositions() end")
        .map_err(|e| e.to_string())
}

/// **What a screen-size change re-runs** — called from `extract::tick_script`'s resize arm, the
/// frame [`UiScript::set_screen_size`] reports a change on. Anchors follow the new screen rect by
/// themselves; what does not is a size or seat somebody COMPUTED from the old one.
///
/// The bottom-stack manage pass (decision 1499): the open-bag stack starts a fresh column when
/// the current one would run off the top, and that decision is made from `GetScreenHeight()` at
/// layout time. Without this, dragging the window smaller leaves the bag columns wrapped for the
/// old height until the next bag opens. Existence-guarded: the pass is defined by `UIParent.xml`,
/// which is in-game UI, and the resize arm also runs on the glue screens.
///
/// **And the three full-screen quads the stock files size once** — a stated repair of a reference
/// gap, in the same posture as [`install_durability_reseat`]: nothing of Blizzard's is edited.
/// `WorldMapFrame_OnLoad` sizes `BlackoutWorld` and `CinematicFrame_OnLoad` sizes
/// `UpperBlackBar`/`LowerBlackBar` from `GetScreenWidth()`/`GetScreenHeight()` at load, and no
/// stock handler touches them again (no `DISPLAY_SIZE_CHANGED` listener) — the reference could
/// afford that because its screen changed only across a restart. 2242 fixed the load edge; this
/// is the other edge: after a window resize or a `uiScale` change the blackout stopped short of
/// the new screen and the world showed through beside the map. [`FULLSCREEN_QUADS_RESEAT`] is the
/// two OnLoads' own arithmetic re-run, so a quad after a resize is exactly what a fresh load at
/// the new size would have made. A stateless chunk rather than an installed hook, so a `ReloadUI`
/// has nothing to re-install and nothing can stack.
pub(super) fn on_screen_resized(script: &UiScript) {
    let _ = script.run("if UIParent_ManageFramePositions then UIParent_ManageFramePositions() end");
    if let Err(e) = script.run(FULLSCREEN_QUADS_RESEAT) {
        error!("ui_script: full-screen quad re-seat: {e}");
    }
}

/// `WorldMapFrame.lua` `WorldMapFrame_OnLoad` l.19-28 and `CinematicFrame.lua`
/// `CinematicFrame_OnLoad` l.5-19, verbatim in their arithmetic, each existence-guarded (the
/// glue screens have neither). One addition to the letterbox: the stock OnLoad does nothing below
/// 4:3, which leaves the bars at `CinematicFrame.xml`'s declared `1024 x 128` — so a resize to
/// below 4:3 puts that declared size back, rather than keeping a wide screen's recompute.
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

/// **A stated divergence from the reference, installed rather than edited in** (1751 window 4).
///
/// The reference seats `DurabilityFrame` 20 further in when one of its three side glyphs is up
/// (`UIParent.lua` l.1759-1768), and recomputes that only when something calls
/// `UIParent_ManageFramePositions`. Stock `DurabilityFrame.xml` calls it from `OnShow`/`OnHide`
/// only — so the case where the frame is **already shown** and a side glyph *then* appears leaves
/// the seat stale, and the shield glyph's ~17-unit overhang hangs off the screen edge until some
/// unrelated frame happens to trigger the next pass. That is the reference's own bug and the
/// director caught it on their screen; it is not ours to reproduce faithfully.
///
/// So the fix rides on the frame's `OnEvent` — which is exactly the recompute the glyphs change on
/// — and it is installed **here** rather than written into anything of Blizzard's. Two reasons for
/// that home rather than a FrameXML one: `assets/ui` does not grow (1779), and this is the wrong
/// side of the line for a `ContainerFrameAdapters`-class shim anyway — that clause is for a genuine
/// engine difference (1751 §2), and this is a deliberate repair of a reference defect. Rust is also
/// the durable home: our `UIParent.xml` was a transcription awaiting its own window (it migrated
/// with 1988), and a hook parked there would have been homeless that day.
///
/// Idempotent by its own latch, so a `ReloadUI` cannot stack wrappers. The reference's handler
/// still runs first and unchanged: `this`, `event` and `arg1` are globals the engine has already
/// set for the dispatch, so calling it through the captured reference is the same call.
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

/// **The UI load's sound-suppression bracket — the one site both load paths go through.**
///
/// `CGGameUI::Initialize 0x48fbf0` brackets ITSELF in the counted suppression scope —
/// `0x48fbfa call 0x458f50` on entry, `0x49016d call 0x458f60` on exit — across the TOC walk,
/// Bindings.xml, the AddOns, the saved variables and the world-enter cascade it calls at
/// `0x490168` (`PLAYER_LOGIN`), so both of its callers (login `0x48f681` and `/reloadui`
/// `0x495669`) load without a sound. The only reader of that depth is `PlaySoundByName
/// 0x458030`, which drops the call outright.
///
/// This is the mechanism, not a workaround for one noisy handler: stock `TargetFrame_OnHide`
/// really does fire at load (the frame ships shown and its OnLoad hides it) and really does call
/// `PlaySound("INTERFACESOUND_LOSTTARGETUNIT")`. The engine throws it away. Decision 1033 reached
/// the right rule from the director's ear; this is the binary agreeing.
///
/// One function rather than a push/pop pair at each caller because the production edge
/// ([`super::load_ingame_ui_on_world_entry`]) spent its life unbracketed while the tests' whole-
/// manifest load was — every `/reload` played the lost-target click. Generic over the borrow so
/// the `&UiScript` test loader and the owning production edge share it.
pub(super) fn silenced_ui_load<S: std::borrow::Borrow<UiScript>, R>(
    script: &mut S,
    body: impl FnOnce(&mut S) -> R,
) -> R {
    script.borrow().push_sound_suppression();
    let out = body(script);
    script.borrow().pop_sound_suppression();
    out
}

/// Load benilla's own default UI — every file [`MANIFEST`] names — through the engine-free loader.
/// This is our content (MIT/Apache), committed and **compiled into the binary**
/// ([`super::content`], decision 1175); a dev build still prefers the copy on disk, so editing a
/// FrameXML file costs no recompile. Textures (`Interface\…`) still resolve at render through the
/// MPQ `sprite_texture` path; the loader only needs the XML/Lua text.
///
/// Returns every loader error, tagged `"<Addon>/<file>: <error>"` — the app ignores the value (each
/// is already logged as it happens) and [`shipped_xml_tests`] asserts it empty. Before that
/// assertion a broken entry — a bad file name, a frame that collides with a later window's, a
/// template referenced before its definer — reached a real run with nothing but a log line. Capture
/// runs cannot cover it either: they skip this function entirely unless `WOW_CAPTURE_UI=1`.
///
/// **Split across the boot boundary (1051).** `Fonts.xml` — the manifest's first entry, zero frames
/// materialized — is the font-object registry the glyph atlas bakes its plan from, and our native
/// glue screens share that one atlas, so it must exist before the login screen. Everything after it
/// is in-game UI and loads at world entry ([`load_ingame_ui`]). This whole-manifest entry point
/// stays for the tests, which assert over the complete shipped set — production now loads in two
/// phases, so production has no caller for the whole-manifest form — the tests do, and so does
/// the addon harness ([`crate::addon_harness`]), which needs our entire interface under each
/// surveyed addon.
pub(crate) fn load_default_ui(script: &UiScript) -> Vec<String> {
    // **The client's CVar table first, because the interface reads CVars AT LOAD** (decision
    // 2115). The app registers `crate::cvars::REGISTERED` at startup, long before world entry, so
    // in the running client this call finds every name already there and only refreshes its
    // default — it never clobbers a live value ([`benilla_ui::script::UiScript::register_cvars`]).
    // What it buys is that a **probe** VM is the same client: `UiScript::new()` carries only
    // `benilla-ui`'s own CVars, and the stock `UIOptionsFrame.xml`'s two camera dropdowns read
    // theirs inside their own `OnLoad`
    // (`getglobal("OPTION_TOOLTIP_CAMERA"..UIDropDownMenu_GetSelectedID(this))`, which is nil and
    // then a concat error when `GetCVar("cameraSmoothStyle")` answers nil). Both of those CVars
    // have been registered here with real consumers since 1493/1502; the probes simply never had
    // them, and 218 tests found that out the hour this row went on the manifest.
    script.register_cvars(crate::cvars::registered_pairs());
    // Silent, the way the client's own load is — see [`silenced_ui_load`].
    let mut failures = silenced_ui_load(&mut &*script, |script| {
        let mut failures = load_manifest(script, &Addon::builtin().toc.files);
        failures.extend(bootstrap_positions(script));
        failures
    });
    // **A handler that raised DURING the walk is a load failure.** The loader's own report holds
    // the raises it dispatched itself (a `<Script file=>` chunk, an OnLoad); a raise one call
    // deeper — an OnLoad that `Show()`s a frame whose OnShow indexes a global its file has not
    // loaded yet — lands in the VM's error list instead, and until 2001 nothing read that list
    // here: the stance bar's load-order defect shipped with the manifest reporting clean and a
    // WARN line the smoke does not fail on. The errors stay in the VM (the player's dialog and
    // the retained log still get them); this is the walk's own verdict growing the row.
    failures.extend(
        script
            .errors()
            .into_iter()
            .map(|e| format!("a handler raised during the load walk: {e}")),
    );
    failures
}

/// Load a slice of [`MANIFEST`] entries, **each from its own store** (decision 1751).
///
/// A bare filename is a file we ship and comes from [`Addon::builtin`]; a path is the reference's
/// own file and comes off the player's installed chain ([`reference_ui`]). The dispatch is
/// per-entry rather than per-run so the manifest's order is the load order verbatim — the whole
/// reason the list is a manifest and not two lists.
///
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

/// The font-object registry alone (`Fonts.xml`), loaded at `Startup` — see [`load_default_ui`].
///
/// Verified lossless for the atlas: the full manifest and this file alone both yield the **same 19
/// distinct `(font, height, outline)` combinations**. The three font objects defined outside it
/// (`GameFontNormalMed1` 13, `OptionsFontHighlightMedium` 14, `OptionsFontHighlightHuge` 20) are
/// un-outlined and their heights are already declared here, so they add nothing to the bake plan.
pub(crate) fn load_font_registry(script: &UiScript) -> Vec<String> {
    load_manifest(
        script,
        Addon::builtin().toc.files.get(..1).unwrap_or_default(),
    )
}

/// The in-game UI — everything after the font registry — loaded on entering the world, and then
/// **every third-party addon** ([`super::addons`], decision 1184).
///
/// The reference does the same at `CGGameUI::Initialize 0x48fbf0`, reached only from world entry
/// (`0x401570` ← `0x46c236`), and loads its addons from that same function (`0x4900a3` →
/// `0x51f600`); its glue screens run GlueXML with their own `GlueFonts.xml` registry, which is why
/// the reference has no equivalent of our shared-atlas coupling (1051).
///
/// Addons load **after** the built-in interface, not interleaved with it: an addon may reference
/// our templates and globals (that is the point of 1178's seam), and nothing of ours may depend on
/// an addon.
///
/// `identity` is `(realm, character)`, which names this character's AddOn enable-state file — the
/// reference keys `AddOns.txt` per character too — and `roster` is every character on that realm's
/// list, which is the enable store's node set (decision 2311: an addon this character has no row
/// for is resolved from what the *other* characters said, never from a bare "enabled"). `None`
/// with an empty roster is the no-pick case: every addon falls to its own `## DefaultState`.
///
/// `version_check` is the persisted `checkAddonVersion` — the *Load out of date AddOns* toggle,
/// inverted — resolved by the caller because at load time this VM's own CVar table does not
/// exist yet (registration is a per-VM `Update` seed, decision 1291); the persisted value is the
/// truth the reference's live read would land on, since the session edge folds the dying VM's
/// table into it before any rebuild reads it.
pub(crate) fn load_ingame_ui(
    script: &mut UiScript,
    identity: Option<&(String, String)>,
    roster: &[String],
    version_check: bool,
) -> Vec<String> {
    // The whole load edge runs bounded (decision 1306): the reference files sourced off the
    // player's own chain, our builtin, and every addon (which re-arms per addon in
    // `load_third_party`). A chunk that never returns fails as a load error instead of freezing
    // the client on the loading screen; the caller disarms once the edge is done
    // (`lifecycle::load_ingame_ui_on_world_entry`), so the session's steady state runs unhooked.
    script.set_instruction_budget(super::addons::LOAD_INSTRUCTION_BUDGET);
    let mut failures = load_manifest(
        script,
        Addon::builtin().toc.files.get(1..).unwrap_or_default(),
    );
    failures.extend(bootstrap_positions(script));
    // `&mut` from here down: each addon's `ADDON_LOADED` fires as that addon finishes, which is
    // the reference's own interleaving (`0x51f5ad`, per addon) rather than a batch at the end.
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

    /// The manifest parses as a `.toc`, declares the build it targets, and splits where the loader
    /// splits it. `Fonts.xml` first is not tidiness: [`load_font_registry`] takes entry 0 and
    /// [`load_ingame_ui`] takes the rest, so a reordering here silently moves a real file across
    /// the boot boundary (1051) — into the glue screens' phase, or out of the atlas bake plan.
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

    /// The manifest and `assets/ui` describe the same interface, both ways.
    ///
    /// An entry naming a file we do not ship is a log line per entry and an empty screen (what
    /// `content::tests::every_manifest_entry_is_compiled_in` catches). The other direction is the
    /// one nothing caught before: a FrameXML file added to `assets/ui` and never listed here is
    /// simply never loaded, and the symptom is a window that does not exist rather than an error.
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

    /// **The shipped tree is FLAT, and that is what makes the two stores tellable apart.**
    ///
    /// [`reference_ui::is_chain_entry`] decides where a manifest entry comes from by asking
    /// whether it carries a path separator (1751). That is only decidable while every file we
    /// ship is a bare name — the day somebody adds `assets/ui/templates/Foo.xml`, its manifest
    /// entry would be read as a chain path, and the symptom would be a window that silently does
    /// not exist rather than an error. This is that day's failing test.
    #[test]
    fn every_file_we_ship_is_a_bare_name_so_a_path_can_only_mean_the_chain() {
        for name in super::super::content::shipped_files() {
            assert!(
                !reference_ui::is_chain_entry(name),
                "assets/ui is flat by construction, but ships {name} — a manifest entry with a                  separator is read as a file to source off the player's install"
            );
        }
    }

    /// Every `Interface\…` entry the manifest names is really in the 1.12 chain, and is really
    /// **not** something we also ship under that basename.
    ///
    /// Skips without client data, like every other test that reads the install.
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

    /// **Nothing may declare `parent="UIParent"` before `UIParent.xml` has loaded.**
    ///
    /// A parent name is resolved at LOAD, and a name that does not exist yet is not an error: the
    /// loader warns and silently falls back to the enclosing frame. So this ordering mistake does
    /// not fail, it *half-works* — the frame keeps drawing, keeps answering `IsShown`, and simply
    /// never joins the cascade `UIParent:Hide()` walks. That is precisely how it would be missed.
    ///
    /// It nearly was: our `UIParent.xml` sat below `UiPanels.xml` until decision 1734, so restoring
    /// `StaticPopup1`/`StaticPopup2`'s parents there would have written two declarations that did
    /// nothing at all. The reference's own order is the fix (FrameXML.toc: BasicControls.xml l.6,
    /// UIParent.xml l.8), and this keeps it.
    /// **Every load-time runner of `UIParent_ManageFramePositions` follows every frame the pass
    /// reads.** The stock pass (`UIParent.lua:1592-1775`) indexes a dozen HUD frames unguarded —
    /// `ReputationWatchBar`, `QuestTimerFrame`, `QuestWatchFrame`, `MinimapCluster`,
    /// `DurabilityFrame`, the bars — and several stock files run it from an OnLoad or the OnShow
    /// of a frame their OnLoad shows. In the reference's toc the readers all precede the runners;
    /// ours had the stance bar (a runner, through `ShapeshiftBar_OnLoad` → `Show` → `OnShow`)
    /// three hundred lines above `ReputationFrame.xml` (a read), so every login raised at
    /// `UIParent.lua:1618` and the shelf was never seated — the director's sliver above Defensive
    /// Stance (decision 2001). The pairs below are the reference's own dependency, read off the
    /// pass's body; a new runner or a new read joins the table, not a comment.
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
            // The shapeshift-appearance arm (l.1705-1732): a runner's file can be a READ too —
            // the stance bar declares these and runs the pass, so it precedes the other runner.
            (
                "ShapeshiftBarLeft / Middle / Right",
                "BonusActionBarFrame.xml",
            ),
        ];
        // Who runs it at LOAD: an OnLoad, or the OnShow of a frame its OnLoad shows.
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
            // A chain entry is the player's file, not one on this machine — and the reference's
            // own load order is what this test exists to preserve, so it cannot be the thing that
            // violates it. `every_chain_entry_resolves_off_the_players_install` covers those.
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
