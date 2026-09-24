//! **Where interfaces come from** — the addon folder, and the walk that loads what is in it
//! (decision 1184, building 1178 step 2).
//!
//! 1178 step 1 made benilla's own interface load from a `.toc`. This is the other half of the
//! claim: the same parser, the same loader, and the same walk now run over folders that are not
//! ours. An [`Addon`] is a name, a parsed manifest, and a [`Source`] its files come from — and
//! benilla's own interface is simply the one whose source is the compiled-in tree.
//!
//! ## Where we look — ONE folder
//!
//! **`<benilla-config>/AddOns/<Name>/<Name>.toc`**, and nothing else (decision 1185). 1184 also
//! searched the WoW install's own `Interface/AddOns/` and merged the two; the director's call is
//! one root, changeable in future *by choice* but never both at once. Resolved through
//! [`crate::local_state`], the only module allowed to compute that path (0954), which is already
//! `None` under `$WOW_CAPTURE` — so a capture discovers nothing and a deterministic baseline
//! cannot depend on what somebody has installed (0008).
//!
//! What the install's folder actually holds, checked rather than assumed: twelve `Blizzard_*`
//! folders containing **only a `.pub` signature file** — their real code is inside the MPQs, which
//! we do not read for addons at all — plus whatever the machine's owner has put there. Nothing was
//! being gained by searching it.
//!
//! ## Two kinds of file in a manifest
//!
//! A `.toc` lists `.lua` **and** `.xml`, and they load differently: Lua is executed as a chunk in
//! the shared global state, FrameXML is parsed and materialized. See [`Addon::load_files`].
//!
//! ## The load order is the reference's, not a topological sort
//!
//! `AddOn_Load 0x51f240` is **recursive**: for each addon, in order —
//! **OptionalDeps first (failures ignored)** → **RequiredDeps (a failure ABORTS this addon's
//! load)** → **its own `.toc`-listed files, in listed order**. We match that shape, because a
//! pre-sorted flat list cannot express "a missing hard dependency drops exactly this addon and
//! nothing else".
//!
//! **Our own interface is not an addon for lifecycle purposes.** In the reference,
//! *FrameXML does not go through `AddOn_Load`, so it gets no `ADDON_LOADED`* — and `benilla.toc`
//! is excluded exactly as FrameXML is. Structurally, not by a filter: it loads through
//! [`super::manifest::load_ingame_ui`], while [`Walk`] only ever walks what [`discover`] found.
//! It shares the loader; it does not share the event.
//!
//! ## Per-addon `Bindings.xml`
//!
//! Loaded right after the `.toc` files and before the saved variables (`0x51f400`) — 1188 phase 4,
//! in [`Walk::load`]. Parsed by [`benilla_ui::bindings_xml`] and registered into the same
//! key-binding table the Key Bindings window edits, so an addon's binding is a row like any
//! other; the app's dispatch runs its Lua body ([`crate::bindings`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};
use benilla_ui::toc::Toc;

use super::content;

/// The folder name under each root, and the reference's own spelling.
const ADDONS_DIR: &str = "AddOns";

/// The world-entry load's VM instruction bound, per addon (decision 1306) — and the addon
/// harness's survey bound, one number for both so the measurement stays shared. MEASURED, not
/// guessed (e463649e): across the 218-addon corpus, 214 addons execute under 1M VM instructions
/// and the heaviest legitimate one (Enchantrix) reaches 4M — this is ~50x that. An addon that
/// crosses it is not slow, it is not coming back; the raise fails that addon with a distinctive
/// message and the walk loads everyone else, where before 1306 the client sat frozen on the
/// loading screen with zero diagnostics (B271's class).
pub(crate) const LOAD_INSTRUCTION_BUDGET: u64 = 200_000_000;

/// Where one interface's files come from.
///
/// The three arms are the same question asked of three stores, and keeping them one enum is what
/// lets [`Addon::load`] be written once: our own interface, a third-party folder and the player's
/// own install differ in where the bytes live, not in how they load.
pub(super) enum Source {
    /// benilla's own interface — the compiled-in tree, which a dev build shadows with `assets/ui`
    /// on disk so editing FrameXML costs no recompile ([`content`], 1175).
    Builtin,
    /// **The AddOns root**, not this addon's own folder (decision 1186). Paths handed to
    /// [`Addon::read`] are relative to it, so `Bagnon/src/main.xml` and the
    /// `BagBrother/core/core.xml` it includes are both expressible — which is the point, since a
    /// shared library addon exists to be reached from its dependents.
    Dir(PathBuf),
    /// **The player's own installed patch chain** — the reference FrameXML this client sources
    /// rather than shipping a copy of (decision 1751, [`super::reference_ui`]). Paths are full
    /// chain-internal paths (`Interface\FrameXML\ContainerFrame.xml`), so the prefix is empty and
    /// the chain resolves `/` and `\` alike, case-insensitively.
    Chain,
}

/// One loadable interface: a name, its parsed manifest, and where its files come from.
pub(super) struct Addon {
    /// The addon's folder name — and, for the builtin, `"benilla"`. This is what
    /// `GetAddOnInfo`/`IsAddOnLoaded` will key on (1188 phase 2) and what `ADDON_LOADED` carries.
    pub(super) name: String,
    /// The parsed `.toc`: the ordered file list plus every directive.
    pub(super) toc: Toc,
    source: Source,
}

impl Addon {
    /// An interface from an explicit source — [`super::reference_ui::addon`]'s constructor, and
    /// the only one that is not derived from a `.toc` on some store.
    pub(super) fn new(name: String, toc: Toc, source: Source) -> Self {
        Addon { name, toc, source }
    }

    /// benilla's own interface. Always present — it is in the binary (1175), so unlike a
    /// discovered addon it cannot be missing.
    pub(super) fn builtin() -> Self {
        let toc = content::read(super::manifest::MANIFEST)
            .map(|t| Toc::parse(&t))
            .unwrap_or_else(|| {
                error!(
                    "ui_script: {} is not in the shipped UI — no interface will load",
                    super::manifest::MANIFEST
                );
                Toc::default()
            });
        Addon {
            name: "benilla".to_string(),
            toc,
            source: Source::Builtin,
        }
    }

    /// The text of one file, by a path already resolved into the **source's** path space — the
    /// AddOns root for a `Dir`, the flat shipped tree for the builtin.
    ///
    /// The caller does the relative-path arithmetic ([`benilla_ui::loader::join_ref`]); this only
    /// reads. That split is decision 1186's: only the loader knows the include tree, and only the
    /// source knows what a resolved path is allowed to reach.
    ///
    /// **The sandbox is the AddOns folder, not one addon's subfolder.** `..` that walks above the
    /// root survives `join_ref` as a leading `..`, and [`read_under`] refuses it on the filesystem
    /// leg — so an addon can reach a sibling library addon (which is how `Bagnon` reaches
    /// `BagBrother`, and what 1184's per-addon guard wrongly blocked) but cannot reach the machine.
    /// A `Dir` miss then falls through to the player's patch chain under the install's own name
    /// for it ([`read_addon_file`], decision 2155), which is the single namespace the reference
    /// opener has — and the reason `Auctioneer`'s reach into `Blizzard_AuctionUI` resolves at all.
    ///
    /// The **builtin alone** keeps a basename fallback: its tree is flat, and a transcription that
    /// writes a Blizzard-style directory path should still find the file. A `Dir` source must not
    /// have it — a basename fallback silently rescues exactly the escaping path the guard just
    /// refused.
    /// Returns **bytes**, not text (decision 1193). A `.lua` chunk is handed to Lua as it sits on
    /// disk, and only an XML/`.toc` parse decodes — because a `read_to_string` here did not make a
    /// cp1252 locale file lose a glyph, it made the file *not exist*.
    fn read(&self, req: &str) -> Option<Vec<u8>> {
        match &self.source {
            Source::Builtin => {
                let norm = req.replace('\\', "/");
                let base = norm.rsplit('/').next().unwrap_or(&norm).to_string();
                content::read(&norm)
                    .or_else(|| content::read(&base))
                    .map(String::into_bytes)
            }
            Source::Dir(root) => read_addon_file(root, req),
            Source::Chain => super::reference_ui::read(req),
        }
    }

    /// This addon's own folder in the install's path space — the `base` its manifest entries are
    /// relative to. `""` for the builtin's flat tree, `""` for the chain (whose manifest entries
    /// are already full internal paths), and `Interface/AddOns/<Folder>` for a `Dir`.
    ///
    /// **`Interface/AddOns/`, not the bare folder name** (decision 2155). It used to be the folder
    /// alone, which made the addon's path space the AddOns root rather than the install — a second
    /// namespace the reference does not have. The visible cost was `<Script file=>`'s chunk NAME:
    /// the loader names a file chunk after the path it resolved, so an XML-loaded Lua file came out
    /// `@AtlasLoot\Core\AtlasLoot.lua` while the `.toc`-listed file beside it was
    /// `@Interface\AddOns\AtlasLoot\…`. Every Ace2/FuBar-era library finds its own addon by
    /// splitting a `debugstack` frame on `\AddOns\` (`AceDB-2.0.lua:742`,
    /// `FuBarPlugin-2.0.lua:602`), so for those files the split found nothing and the library
    /// silently keyed itself on a traceback — or, in `SSHonor_Fu`'s case, called
    /// `IsAddOnLoadOnDemand(nil)`.
    fn prefix(&self) -> String {
        match &self.source {
            Source::Builtin | Source::Chain => String::new(),
            Source::Dir(_) => format!("{ADDONS_PREFIX}{}", self.name),
        }
    }

    /// The name Lua gives a chunk from this source — **not cosmetic**, because addons parse it.
    ///
    /// `path` is the entry already resolved into the install's path space, which is the only thing
    /// the client names a file chunk after: `"@%s"` (`0x8716e0`) over the resolved path, built by
    /// `0x704bc0` for every `.lua` the loader touches. So an addon's file is
    /// `Interface\AddOns\<Folder>\<File>` and a chain file is `Interface\FrameXML\…` — the second
    /// being both what the real client names it and what keeps FrameXML *out* of the `\AddOns\`
    /// pattern the Ace2 family matches against, because FrameXML is not an addon.
    ///
    /// **One rule for both doors, which is the point** (decision 2155). `Loader::run` names a
    /// `<Script file=>` chunk by exactly this rule over exactly this space, so a manifest entry and
    /// an XML-referenced file now agree — they did not while a `Dir` addon's space was rooted at
    /// the AddOns folder, and [`Addon::prefix`] carries what that cost.
    ///
    /// The builtin keeps [`benilla_ui::script::addon_chunk_name`]: its tree is flat and internal,
    /// so there is no resolved install path to name it after.
    fn chunk_name(&self, file: &str, path: &str) -> String {
        match &self.source {
            Source::Builtin => benilla_ui::script::addon_chunk_name(&self.name, file),
            Source::Dir(_) | Source::Chain => format!("@{}", path.replace('/', "\\")),
        }
    }

    /// Load this addon's `.toc`-listed files into the VM, in listed order. Returns per-file
    /// errors, each tagged `"<Addon>/<file>: <error>"`.
    ///
    /// Errors are logged as they happen *and* returned: the app ignores the value (it has already
    /// been logged) while the tests assert it empty, which is the shape 1178 step 1 established
    /// for the builtin and the reason a broken entry can no longer reach a run behind a log line.
    fn load(&self, script: &UiScript) -> Vec<String> {
        self.load_files(script, &self.toc.files)
    }

    /// [`Addon::load`] over an explicit slice — the builtin's two-phase boot split (1051) is the
    /// only caller that needs less than the whole manifest.
    ///
    /// **A manifest lists two kinds of file and they load differently** (decision 1185). `.lua` is
    /// executed as a chunk in the shared global state; anything else is FrameXML — parsed, then
    /// materialized. 1184 sent every entry through the XML parser, which was invisible only
    /// because `benilla.toc` happens to list nothing but `.xml`: the reference's own `FrameXML.toc`
    /// opens with `GlobalStrings.lua`, and a bare Lua file is not a document.
    pub(super) fn load_files(&self, script: &UiScript, files: &[String]) -> Vec<String> {
        let mut failures = Vec::new();
        // The `<Include>` / `<Script file=>` seam. The loader hands us paths it has already
        // resolved against the including file's directory (1186), so this only reads — and the
        // sandbox lives in `read`, at the source, rather than being spread over call sites.
        let provider = |req: &str| -> Option<Vec<u8>> { self.read(req) };
        for file in files {
            // A manifest entry is relative to the addon's own folder; `read` and the loader both
            // work in the source's path space, so resolve once here and use it for both.
            let path = benilla_ui::loader::join_ref(&self.prefix(), file);
            let Some(bytes) = self.read(&path) else {
                let e = format!("{}/{file}: not found", self.name);
                // Severity follows whose manifest lied. For the builtin that is us — a client
                // bug, and the boot tests assert none. For a player's addon it is the package
                // (the director's AtlasLoot copy lists `Bossnames\BossNames.xml`; no such folder
                // ships in it), and the reference client silently skips a missing toc entry — so
                // a broken addon must not read as a client ERROR, which is gate-fatal to
                // `smoke.sh`'s zero-ERROR count (1450).
                match self.source {
                    // Ours, and the chain list is ours too: a `benilla.toc` line naming a file the
                    // player's install does not hold is our manifest being wrong about the 1.12
                    // chain, not the player's copy being wrong. (No install at all is a different
                    // thing, warned once by `reference_ui::chain`.)
                    Source::Builtin | Source::Chain => error!("ui_script: {e}"),
                    Source::Dir(_) => warn!("ui_script: {e}"),
                }
                // Retained where the player can read it (1495). This is the single commonest way
                // an addon "doesn't work" with nothing on screen — the director's own AtlasLoot
                // copy lists `Bossnames\BossNames.xml` and ships no such folder.
                script.report_load_failure(&e);
                failures.push(e);
                continue;
            };
            if is_lua(file) {
                // The same execution `<Script file=>` gets (`loader::mod.rs`): one chunk, run in
                // the one global state, in manifest order. The reference does exactly this —
                // `AddOn_Load 0x51f240` hands each listed file to `0x6edb90` regardless of kind.
                match script.run_chunk_named(&bytes, &self.chunk_name(file, &path)) {
                    Ok(()) => info!("ui_script: {}/{file} ran", self.name),
                    Err(e) => {
                        let e = format!("{}/{file}: {e}", self.name);
                        error!("ui_script: {e}");
                        // A file-scope failure is a script error, and script errors reach the
                        // player (1305): queued for the Lua error handler — `_ERRORMESSAGE`'s
                        // red dialog once BasicControls has run — never only a terminal line.
                        script.report_script_error(&e);
                        failures.push(e);
                    }
                }
                continue;
            }
            let doc = match benilla_ui::framexml::parse(&benilla_ui::source::decode(&bytes)) {
                Ok(d) => d,
                Err(e) => {
                    let e = format!("{}/{file}: {e}", self.name);
                    error!("ui_script: parsing {e}");
                    // Still log-only as far as the *dialog* goes — the reference answers an
                    // unparseable document with a FrameXML.log line and silence, and 1495 does not
                    // change that. What it changes is that the silence is no longer total.
                    script.report_load_failure(&e);
                    failures.push(e);
                    continue;
                }
            };
            // The document's OWN directory is what its relative references resolve against — a
            // manifest listing `src\main.xml` means `<Include file="templates.xml">` inside it is
            // `<Addon>/src/templates.xml`, and `..\..\Lib\lib.xml` is a sibling addon (1186). The
            // loader takes the path and does that itself, so it can also name the file a raise
            // came from (1217).
            let report = benilla_ui::loader::load_in(script, &doc, &path, &provider);
            // `FrameXML_Debug`'s trace lines, if the player turned it on (2160). Log-only, and
            // never retained: the reference files them into the same per-document record as the
            // errors, but at severity 0 (`0x6ee2bc`), and *where that record surfaces is an open
            // question*, for severity 1 as well. So they go where a trace can go without claiming a
            // surface we have not derived — and the switch that produces them is off unless an
            // addon asks.
            for t in &report.traces {
                info!("ui_script({}/{file}): {t}", self.name);
            }
            for w in &report.warnings {
                warn!("ui_script({}/{file}): {w}", self.name);
                // …and retained where a player can read it (2135). The `warn!` above is the
                // terminal's copy and it is gone the moment the line scrolls; this is the one
                // that survives to `/errors`, and the prefix is why it is recorded here rather
                // than inside the loader — only this caller knows which file the warning is from.
                script.report_warning(&format!("{}/{file}: {w}", self.name));
            }
            // **A file the document named and the provider does not have — the same rule as a
            // `.toc` line naming no file** (decision 2155, the rule 2107 unified for the walk and
            // the demand load, at the two doors it did not reach). Severity by whose manifest
            // lied, retained where a player can read it, and never a script error: the reference
            // logs `Couldn't open %s` / `Error loading %s` and carries on with nothing raised.
            // Still a `failures` entry, so our own boot tests fail on a `benilla.toc` document
            // that names a file the chain does not hold.
            for m in &report.missing_files {
                let e = format!("{}/{file}: {m}", self.name);
                match self.source {
                    Source::Builtin | Source::Chain => error!("ui_script: {e}"),
                    Source::Dir(_) => warn!("ui_script: {e}"),
                }
                script.report_load_failure(&e);
                failures.push(e);
            }
            for e in &report.errors {
                error!("ui_script({}/{file}): {e}", self.name);
                // Same contract as the Lua arm above: a `<Script file=>` chunk that raised or an
                // OnLoad that errored is a script error the player gets to see (1305).
                // `_ERRORMESSAGE`'s own IsVisible guard shows a burst's FIRST failure only,
                // which is the reference's behaviour too. Document *parse* failures stay
                // log-only, like the reference's FrameXML.log.
                script.report_script_error(&format!("{}/{file}: {e}", self.name));
                failures.push(format!("{}/{file}: {e}", self.name));
            }
            info!(
                "ui_script: {}/{file} loaded ({} frames materialized)",
                self.name, report.frames
            );
        }
        failures
    }
}

/// Is this manifest entry a Lua chunk rather than a FrameXML document?
///
/// By extension, case-insensitively, and `\` is a path separator like `/` — a `.toc` is written
/// for a Windows client and `Libs\LibStub\LibStub.lua` is the normal spelling.
fn is_lua(entry: &str) -> bool {
    entry
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(entry)
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("lua"))
}

/// `root/rel`, refusing to escape `root`.
///
/// `rel` has already been lexically resolved by [`benilla_ui::loader::join_ref`], so a path that
/// stays inside is free of `.`/`..` and one that escapes carries a **leading `..`** — which is
/// exactly what this rejects, along with anything absolute or non-UTF-8-normal. The check is
/// lexical and happens before any filesystem call, so a symlinked addon folder still works and a
/// traversal never opens a file at all.
///
/// `root` is the **AddOns folder** (1186), so a sibling addon is reachable and the machine is not.
fn read_under(root: &Path, rel: &str) -> Option<Vec<u8>> {
    let rel = Path::new(rel);
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    std::fs::read(root.join(rel)).ok()
}

/// **THE file namespace — install-relative, one space for every source** (decision 2155).
///
/// `req` is a path the loader has already resolved and collapsed
/// ([`benilla_ui::loader::join_ref`]) in the client's own space: `Interface/AddOns/<Folder>/…` for
/// an addon's file, `Interface/FrameXML/…` for one that walked out of the AddOns tree. The
/// reference has exactly one such space — `0x647e60` answers a name from a hash index of the
/// install tree keyed by **install-root-relative path**, and then, attempt #4, from the MPQ chain
/// (loose *before* archive, which is the order below). Two spaces is what this client had, and it
/// cost three separate things:
///
/// - `..\Blizzard_AuctionUI\Blizzard_AuctionUITemplates.xml`, a `.toc` line in `Auctioneer` and
///   in `BeanCounter`, collapses to `Interface/AddOns/Blizzard_AuctionUI/…` — a folder that is a
///   `.pub` decoy on disk with the real files inside `patch.MPQ`. Only the chain leg has it.
/// - `..\..\FrameXML\Fonts.xml`, a `.toc` line in `JIM_toolbox` and in `SpecialTalentUI`,
///   collapses out of the AddOns tree entirely, to `Interface/FrameXML/Fonts.xml`. Same leg.
///   (Both **RESOLVE** on the real client: `0x6ede10` collapses the `..` before the open.)
/// - and the one that was invisible: with the addon space rooted at the AddOns folder, a
///   `<Script file=>` chunk was NAMED after that space — `@AtlasLoot\Core\AtlasLoot.lua` — while
///   a `.toc`-listed one was named `@Interface\AddOns\AtlasLoot\…`. See [`Addon::chunk_name`].
///
/// **The sandbox is stricter than it was, not looser.** Only a path under `Interface/AddOns/`
/// touches the filesystem at all, and [`read_under`] still refuses to escape the root once the
/// prefix is stripped; everything else can only reach the read-only archive chain, whose names
/// cannot leave `Interface\`'s own tree. An addon reaches a sibling and the client's own interface
/// files, and never the machine.
pub(crate) fn read_addon_file(root: &Path, req: &str) -> Option<Vec<u8>> {
    under_addons(req)
        .and_then(|rel| read_under(root, rel))
        .or_else(|| super::reference_ui::read(&req.replace('/', "\\")))
}

/// `req` with the `Interface/AddOns/` prefix stripped, or `None` if it does not carry one.
///
/// Case-insensitively, because the reference is a Windows client: a `.toc` writing
/// `..\..\Interface\Addons\Foo\bar.lua` names the same file there, and on a case-sensitive
/// filesystem an exact compare would silently send it to the chain-only leg.
fn under_addons(req: &str) -> Option<&str> {
    let rest = req.get(ADDONS_PREFIX.len()..)?;
    req.get(..ADDONS_PREFIX.len())
        .filter(|p| p.eq_ignore_ascii_case(ADDONS_PREFIX))
        .map(|_| rest)
}

/// Where a `Source::Dir` addon's files live in the install's path space — the base every one of
/// its manifest entries resolves against, with its folder name appended
/// ([`Addon::prefix`]). `/`-separated, because that is [`benilla_ui::loader::join_ref`]'s space.
const ADDONS_PREFIX: &str = "Interface/AddOns/";

/// **The** addon folder — `<benilla-config>/AddOns/` — or `None` when there is none to read.
///
/// **One root, never two** (decision 1185, the director's call). 1184 searched the WoW install's
/// `Interface/AddOns/` as well and merged the results. Two roots means an addon's identity depends
/// on which folder won, a name can be shadowed, and "where is this addon loaded from" stops having
/// one answer. If the install's folder ever becomes reachable it will be *instead of* this one and
/// by explicit choice, not alongside it.
///
/// It is ours for the same reason 1180 moved our state out of the install: benilla reads a WoW
/// install, it does not live in one. Resolved through [`crate::local_state`], the only module
/// allowed to compute that path (0954) — which is already `None` under `$WOW_CAPTURE`, so a
/// deterministic baseline cannot depend on what somebody has installed (0008).
///
/// `pub(crate)` since 1322: every asset an addon ships needs the same folder — the sprite
/// decoder's `Interface\AddOns\` loose-file resolve, the `SetTexture` probe, and (2103) the font
/// engine's face loader plus the `SetFont` probe all map that virtual prefix onto this root, so
/// there is exactly one answer to "where do addons live" however the question is asked. The first
/// three are wired in one place (`ui_script::lifecycle::install_addon_asset_resolvers`); the face
/// loader reads it here directly, at engine construction, because it is not a VM concern.
pub(crate) fn root() -> Option<PathBuf> {
    root_from(crate::local_state::home())
}

/// [`root`] with its one environment fact passed in, so it is testable without touching the
/// process environment.
///
/// Not tidiness: `install::candidates_from` exists for exactly this reason (1175's own flake — a
/// test that *sets* an env var poisons every other test running concurrently in the same process,
/// and the failure then moves around with scheduling).
fn root_from(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(ADDONS_DIR))
}

/// The reference's own LoadOnDemand addons, as its `Interface\\AddOns\\` folder names them — the
/// twelve `Blizzard_*` folders on a 1.12 install, each a `.pub` decoy on disk with the real files
/// inside the archive (the InspectUI header in the manifest). Probed against the chain, so a
/// chain without one simply has no such addon.
const BLIZZARD_ADDONS: [&str; 12] = [
    "Blizzard_AuctionUI",
    "Blizzard_BattlefieldMinimap",
    "Blizzard_BindingUI",
    "Blizzard_CombatText",
    "Blizzard_CraftUI",
    "Blizzard_GMSurveyUI",
    "Blizzard_InspectUI",
    "Blizzard_MacroUI",
    "Blizzard_RaidUI",
    "Blizzard_TalentUI",
    "Blizzard_TradeSkillUI",
    "Blizzard_TrainerUI",
];

/// The Blizzard addons the chain carries as LoadOnDemand registry rows — the ones the reference
/// reaches through `LoadAddOn` from `UIParentLoadAddOn` (1957). `benilla.toc` lists none of them,
/// like the reference's own `FrameXML.toc` (1967). One exclusion, by shape: an addon whose toc is
/// not LoadOnDemand would need the startup walk to read the chain, which it does not yet — that
/// one stays with its own file until its window migrates.
fn chain_addons() -> Vec<Addon> {
    BLIZZARD_ADDONS
        .iter()
        .filter_map(|name| {
            let bytes = super::reference_ui::read(&format!("Interface/AddOns/{name}/{name}.toc"))?;
            let toc = Toc::parse(&benilla_ui::source::decode(&bytes));
            toc.load_on_demand().then(|| Addon {
                name: (*name).to_string(),
                toc,
                source: Source::Chain,
            })
        })
        .collect()
}

/// Every registered addon, in the reference's own **registration order**: the archive pass first,
/// then the loose folders (decision 2175).
///
/// **Two passes, archive before loose, and the first pass wins a duplicate.**
/// `AddOn_ScanAddOnDir 0x51c760` runs
/// `0x401470` over each mounted archive's `(listfile)` (`0x648fb0`, textual line order) and only
/// then `0x42ad10`'s `FindFirstFileW` walk of the loose directories; both funnel into
/// `0x51c9b0`, which probes the name hash and **returns immediately on a hit** (`0x51ca10`), and
/// links each new record at the TAIL (`0x521ad0` mode 2 — a tail insert with no comparison of any
/// kind, so registry order *is* registration order). Measured on a stock
/// install: the twelve `Blizzard_*` `.toc`s are the only ones in any archive, so Blizzard occupies
/// registry positions 1..12 and third-party always follows.
///
/// We had it the other way round — loose first, chain appended — which put our chain rows last and
/// left a player folder named `Blizzard_TalentUI` registered *twice*, once from each source. The
/// duplicate is the half with teeth: two records share one name, and every name lookup finds only
/// the first.
///
/// The load walk barely notices (all twelve are `## LoadOnDemand: 1`, so `0x51f600` skips them),
/// but the registry order is what the Lua index space is built from, and it is what decides which
/// addon's copy of a shared global wins — the last writer, in the one shared Lua state
/// (`0x7040d0`).
fn discover() -> Vec<Addon> {
    // PASS 1 — the archive. `BLIZZARD_ADDONS` is in ascending name order, which is also the order
    // the shipped `patch.MPQ` listfile happens to carry those twelve lines in (measured). A
    // re-implementation may NOT rely on a listfile being sorted, so this is
    // our deterministic choice for a set we enumerate ourselves, not a claim about the format.
    let mut found = chain_addons();
    // PASS 2 — the loose folders, minus anything the archive already registered.
    let seen: HashSet<String> = found.iter().map(|a| a.name.to_ascii_lowercase()).collect();
    found.extend(
        discover_folder()
            .into_iter()
            .filter(|a| !seen.contains(&a.name.to_ascii_lowercase())),
    );
    found
}

/// **The reference's directory-listing order, as a sort key** — the one thing 2166 §7 left open.
///
/// The reference's loose-folder pass is `0x42ad10`'s kernel32 `FindFirstFileW`/`FindNextFileW`
/// walk, and that walk **imposes no ordering of its own**: the client never sorts, so its walk
/// order is whatever the host filesystem hands back. So a re-implementation cannot be
/// order-identical to the reference, because the reference's order is not a property of the
/// reference.
///
/// So there is no order to copy, and exactly one order worth choosing: **NTFS's**. It is the order
/// every 1.12 install actually walked in, because that is the filesystem the client shipped on, and
/// it is therefore the order the whole vanilla addon corpus was written and tested against. A
/// directory's `$I30` index is a B-tree sorted by collation rule `COLLATION_FILE_NAME`, which
/// compares UTF-16 code units after mapping each through the volume's `$UpCase` table, and breaks a
/// case-insensitive tie with a case-sensitive compare of the raw units. Documented in ntfs-3g's
/// `layout.h`/`unistr.c` (`ntfs_names_full_collate`); Microsoft's `FindNextFileW` remarks say the
/// order is not *guaranteed* but that NTFS returns names "usually in alphabetical order", and
/// Raymond Chen's *"Why do NTFS and Explorer disagree on filename sorting?"* names it an ordinal
/// compare over the format-time case table rather than a locale collation.
///
/// **This is the whole difference from the `names.sort()` it replaces, and it is not cosmetic.**
/// Byte order sorts every uppercase name before every lowercase one and puts `_` between them;
/// NTFS folds case away and puts `_` after `Z`. On the 219-addon vanilla corpus **153 of the 219
/// positions move** — `Fubar_*` interleaves with `FuBar_*` instead of following it wholesale,
/// `oRA2` climbs 31 places, `zBar` overtakes `Zorlen`, and `_LazyPig`/`_Nameplates` fall from the
/// middle to dead last. Since the client runs every addon in one Lua state and adds no versioning
/// (`0x7040d0`: which provider of a shared library wins is exactly whichever addon's files run
/// last), that reshuffle decides which copy of `Tablet-2.0` a corpus of seventy providers ends up
/// with.
///
/// The corpus corroborates the mechanism from the other side: the `!`-prefix convention
/// (`!OmniCC`) only ever bought an addon an early slot because the walk was **name-collated**
/// rather than creation-ordered — which is what NTFS does and FAT/exFAT do not.
///
/// **We sort rather than take `read_dir`'s word even on Windows**, deliberately. Raw host order
/// would be byte-identical to a reference install on NTFS and arbitrary everywhere else, and it
/// would cost three things we need: a capture baseline that cannot depend on the host
/// (`crate::local_state`, 0008), [`installed`]'s contract that back-to-back reads answer the same
/// rows in the same order, and a corpus harness whose `--diff` means anything (2166 §4 already
/// fights Lua-side nondeterminism; filesystem-side would end it). One fixed order, and it is the
/// one the corpus grew up under.
///
/// **Where this stops.** `$UpCase` is captured when a volume is formatted, so two Windows volumes
/// can disagree about non-ASCII names and the reference has no single answer there either. For
/// ASCII — every folder name in the corpus — `$UpCase` is the identity outside `a`–`z`, which is
/// exactly what [`upcase_unit`] reproduces.
pub(crate) fn sort_by_directory_order(names: &mut [String]) {
    names.sort_by_cached_key(|n| collation_key(n));
}

/// One name's `COLLATION_FILE_NAME` key: the upcased UTF-16 units, then the raw ones as the
/// tiebreak the rule specifies for names equal but for case.
fn collation_key(name: &str) -> (Vec<u16>, Vec<u16>) {
    let upper: String = name.chars().map(upcase_unit).collect();
    (
        upper.encode_utf16().collect(),
        name.encode_utf16().collect(),
    )
}

/// `$UpCase`'s mapping for one character: **1:1 or nothing**.
///
/// The table is 65536 UTF-16 entries wide, so it can only express a mapping that keeps a name the
/// same length. `char::to_uppercase` is Rust's *full* Unicode uppercase and sometimes yields more
/// than one char (`ß` → `SS`); `$UpCase` cannot, and leaves such a character alone. Taking the
/// mapping only when it is a single char is that constraint, not an approximation of it.
fn upcase_unit(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) => u,
        _ => c,
    }
}

/// The player's own addons — every folder under the AddOns root with a `<Name>.toc`, in
/// [`sort_by_directory_order`].
fn discover_folder() -> Vec<Addon> {
    let Some(root) = root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new(); // no addon folder is the normal case, not an error
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .collect();
    sort_by_directory_order(&mut names);
    names
        .into_iter()
        .filter_map(|name| {
            let dir = root.join(&name);
            // A folder with no `<Name>.toc` is not an addon. The reference ignores these too (a
            // stray `Backup/` folder is the common case), so this is not a warning.
            let text = manifest_in(&dir, &name)?;
            Some(Addon {
                name,
                toc: Toc::parse(&text),
                // The ROOT, not `dir` — every path this addon resolves is relative to the AddOns
                // folder so a sibling library addon is reachable (1186).
                source: Source::Dir(root.clone()),
            })
        })
        .collect()
}

/// `<dir>/<name>.toc`, matched case-insensitively.
///
/// The reference is a Windows client on a case-insensitive filesystem, so a folder named `MyAddon`
/// containing `myaddon.toc` is an addon there and has to be one here — on Linux an exact-name
/// probe would silently not find it.
/// A `.toc` that is not valid UTF-8 is still a manifest (decision 1193): before it was decoded
/// rather than `read_to_string`'d, five addons in a 218-addon corpus were **invisible to
/// discovery** — not broken, not reported, simply not addons — because a German `## Notes:` line
/// held one cp1252 byte.
fn manifest_in(dir: &Path, name: &str) -> Option<String> {
    let want = format!("{name}.toc");
    let entry = std::fs::read_dir(dir).ok()?.flatten().find(|e| {
        e.file_name()
            .to_str()
            .is_some_and(|f| f.eq_ignore_ascii_case(&want))
    })?;
    let bytes = std::fs::read(entry.path()).ok()?;
    Some(benilla_ui::source::decode(&bytes).into_owned())
}

/// Build the AddOn API's registry row for one discovered addon — every `.toc` directive the
/// eleven verbs can be asked about, read once here rather than re-parsed per call.
fn info_for(addon: &Addon) -> benilla_ui::script::AddOnInfo {
    let mut info = info_from_toc(&addon.name, &addon.toc);
    info.chain = matches!(addon.source, Source::Chain);
    info
}

/// [`info_for`]'s body, over the two things it actually reads.
///
/// Split out because `addon_harness` needs the same row and must not build its own: a survey that
/// seats a DIFFERENT registry shape from production is measuring a client nobody runs, which is the
/// fault 1193 was written to fix. One converter, two callers — the harness's own `Toc` goes through
/// this, so a directive added here reaches the survey for free.
pub(crate) fn info_from_toc(name: &str, toc: &Toc) -> benilla_ui::script::AddOnInfo {
    benilla_ui::script::AddOnInfo {
        name: name.to_owned(),
        title: toc.directive("Title").map(str::to_owned),
        notes: toc.directive("Notes").map(str::to_owned),
        url: toc.directive("URL").map(str::to_owned),
        // `## Secure: 1` is Blizzard's own marker; nothing a player installs carries it honestly,
        // and we do not treat it as granting anything — it only picks the glue's icon.
        secure: toc.directive("Secure").map(str::trim) == Some("1"),
        load_on_demand: toc.load_on_demand(),
        dependencies: toc.dependencies().into_iter().map(str::to_owned).collect(),
        directives: toc.directives.clone(),
        files: toc.files.clone(),
        saved_variables: toc
            .list("SavedVariables")
            .into_iter()
            .map(str::to_owned)
            .collect(),
        saved_variables_per_character: toc
            .list("SavedVariablesPerCharacter")
            .into_iter()
            .map(str::to_owned)
            .collect(),
        // Visible to `GetAddOnInfo(index)` until `SMSG_ADDON_INFO` says otherwise (2175). The
        // manifest cannot answer this: the exclusion is the SERVER's verdict on a `## Secure:`
        // record, not a directive.
        hidden: false,
        enabled: true, // an addon nobody has disabled is enabled; the file below overrides
        saved_enabled: true, // re-stamped from `enabled` at registration — see `register_addons`
        loaded: false,
        // The version gate's dword (decision 1292) — the client's own parse: leading integer,
        // 0 when the manifest is silent (and 0 is out of date, not "unknown").
        interface: toc.interface_version(),
        chain: false, // `info_for` stamps the chain rows; the harness's rows are folder addons
    }
}

/// Parse the enable-state file — one `<AddOnName>: enabled|disabled` per line, the reference's own
/// `AddOns.txt` format (confirmed against a real 1.12 install, not remembered).
///
/// Unknown names are ignored rather than dropped-and-rewritten: a file listing an addon that is
/// not installed right now belongs to an addon that will be again, and silently forgetting the
/// player's choice on every uninstall is the behaviour nobody wants. Only `disabled` disables — a
/// malformed line leaves the addon enabled, which is the safe direction.
fn parse_enable_state(text: &str) -> Vec<(String, bool)> {
    text.lines()
        .filter_map(|line| {
            let (name, state) = line.split_once(':')?;
            let name = name.trim();
            (!name.is_empty()).then(|| {
                (
                    name.to_string(),
                    !state.trim().eq_ignore_ascii_case("disabled"),
                )
            })
        })
        .collect()
}

/// Render the enable state back out in the reference's format.
fn render_enable_state(states: &[(String, bool)]) -> String {
    let mut out = String::new();
    for (name, enabled) in states {
        out.push_str(name);
        out.push_str(if *enabled {
            ": enabled\n"
        } else {
            ": disabled\n"
        });
    }
    out
}

/// Where this character's enable state lives, or `None` with no identity yet / no install.
pub(crate) fn enable_state_path(identity: Option<&(String, String)>) -> Option<PathBuf> {
    let (realm, character) = identity?;
    crate::local_state::addons_state_path(realm, character)
}

/// One installed addon as the **AddOns screens** need it — no VM, no load (decision 1197).
///
/// The glue screen runs before any UI VM has addons in it (`load_third_party` is a world-entry
/// step), so the AddOns list cannot ask `GetAddOnInfo`. It asks the same *folder* instead, through
/// the same discovery and the same manifest parser, so the two views cannot disagree about what is
/// installed. Same shape as the reference's `GetAddOnInfo` return, minus what only a live session
/// knows (`loaded`).
#[derive(Clone, Debug)]
pub(crate) struct InstalledAddOn {
    pub(crate) name: String,
    pub(crate) title: Option<String>,
    pub(crate) notes: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) dependencies: Vec<String>,
    /// `## Interface` as the version gate reads it (decision 1292): the leading integer, `0`
    /// when absent — and now ENFORCED by the load walk when `checkAddonVersion` is on
    /// (`0x51e876`), superseding 1191 §6's report-only interim.
    pub(crate) interface: u32,
    /// `## LoadOnDemand: 1` — shown as a status hint rather than a checkbox state, because a
    /// LoadOnDemand addon is not "off", it is waiting for a `LoadAddOn` call (1191 §6).
    /// Read by the char-select AddOns screen's rows (`char_select::addons`).
    pub(crate) load_on_demand: bool,
    /// `## DefaultState:` — what this addon is for a character with no opinion, and the tie-break
    /// when the realm's characters disagree.
    ///
    /// **This row carries no enable bit**, deliberately (decision 2311). It used to, read out of
    /// one character's file with "absent means enabled" baked in — which is the bug: an absent
    /// row is not enabled, it is [`EnableStore::enabled_for`]'s question, and the store needs the
    /// realm's whole character list to answer it. Metadata here, state there.
    pub(crate) default_state: bool,
}

impl InstalledAddOn {
    /// The list's display name — `## Title` when it has one, else the folder name. The reference's
    /// own `if (title) … else name` (`AddonList_Update`).
    ///
    /// (An `out_of_date()` convenience lived here too until 1293 moved the glue screen onto the
    /// gate — the version compare has exactly one home now, `addon_gate::can_load`'s check 6,
    /// and a screen-side copy is the per-asker drift 1292 §Rejected names.)
    pub(crate) fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }
}

/// Every installed addon **in the glue list's own order** — `## Title`-sorted, case-insensitively,
/// with the folder name as the fallback — and `character`'s enable state applied.
///
/// The AddOns screens' whole data source. `identity` is `(realm, character)`; `None` reads the
/// folder with nobody's enable file, which is the "no character picked yet" case and shows
/// everything as enabled.
///
/// **The order is the reference's, and it is not the folder's** (decision 2175). The glue's
/// `AddonList_Update` walks `GetAddOnInfo(i)` for `i = 1..GetNumAddOns()`, and glue `0x46d460`
/// resolves that index through the same `0x51df00` the in-game binding does — the flat array
/// `SMSG_ADDON_INFO` builds, sorted by comparator `0x51deb0` on `AddOn_GetTitle 0x51df20` with
/// `SStrCmpI`. So the rows a player sees are in title order, not folder order, and this screen
/// showed folder order because it reads the folder.
///
/// The array's *membership* needs nothing here: it excludes the addons the server hid, which on a
/// stock install is Blizzard's twelve, and this walk is the player's own folder only — they were
/// never in it.
pub(crate) fn installed_rows() -> Vec<InstalledAddOn> {
    // The player's own folder only: the glue's list is what a player can toggle, and the
    // reference's Blizzard addons are not in it.
    let mut rows: Vec<InstalledAddOn> = discover_folder()
        .into_iter()
        .map(|addon| InstalledAddOn {
            default_state: addon.toc.default_state(),
            title: addon.toc.directive("Title").map(str::to_owned),
            notes: addon.toc.directive("Notes").map(str::to_owned),
            url: addon.toc.directive("URL").map(str::to_owned),
            dependencies: addon
                .toc
                .dependencies()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            interface: addon.toc.interface_version(),
            load_on_demand: addon.toc.load_on_demand(),
            name: addon.name,
        })
        .collect();
    // `SStrCmpI` folds `'A'..'Z'` by `+0x20` on both operands (`0x64a4c0` → `_strnicmp 0x414310`)
    // — ASCII only, which is what `to_ascii_lowercase` is. Stable, where the reference's `qsort`
    // is not: equal keys are undefined there, so any order is conformant.
    rows.sort_by_key(|a| a.display_title().to_ascii_lowercase());
    rows
}

// ── The enable store — the reference's `ADDONSTATELIST` ──────────────────────────────────────────

/// **One node per character on the realm's character list, each holding the explicit rows that
/// character's `AddOns.txt` carried** — the reference's `ADDONSTATELIST` (anchor `0xbe1bd0`, head
/// `ds:0xbe1bd8`), which `AddOnList_LoadCharacter 0x51ebe0` fills one node per character at
/// char-list population.
///
/// The node set is the **char-list, in wire order, rebuilt whole** (decision 2316): the per-record
/// callback `0x472300` is handed to an enumerator that walks every `SMSG_CHAR_ENUM` record with no
/// filter and no early-out, and its driver destroys every existing node first (`0x51f0b0(NULL)`).
///
/// **A node with no file is EMPTY, not absent**, and the difference is the whole point: an empty
/// node contributes no opinion to the aggregate, but it is still a character whose enable bit has
/// to be answered — and the reference answers it from the *other* characters, never with a bare
/// "enabled". That was the bug this type was written for (decision 2311): benilla resolved an
/// absent row as enabled, so creating a character re-enabled every addon the player had just
/// turned off, on every character at once.
#[derive(Default)]
pub(crate) struct EnableStore {
    /// `(character, explicit rows)` in character-list order; the map is the node's enable hash,
    /// keyed lowercased because every compare in the reference is `SStrCmpI`.
    nodes: Vec<(String, HashMap<String, bool>)>,
}

impl EnableStore {
    /// Load a node per character, each from its own `AddOns.txt`
    /// (`benilla-config/addons/<Realm>-<Char>.txt`, decision 1191 §7). A character with no file
    /// still gets its node.
    pub(crate) fn load(realm: &str, characters: &[String]) -> Self {
        let nodes = characters
            .iter()
            .map(|character| {
                let id = (realm.to_string(), character.clone());
                let rows = enable_state_path(Some(&id))
                    .and_then(|p| std::fs::read(p).ok())
                    .map(|b| parse_enable_state(&benilla_ui::source::decode(&b)))
                    .unwrap_or_default();
                let hash = rows
                    .into_iter()
                    // Last line wins, like the reference's hash insert (`0x51eeef`).
                    .map(|(name, on)| (name.to_ascii_lowercase(), on))
                    .collect();
                (character.clone(), hash)
            })
            .collect();
        Self { nodes }
    }

    /// `0x51e470(addon, NULL, useDefault = 0)` — the **explicit-only** aggregate. A node with no
    /// entry for this addon is not counted at all (`0x51e5df je 0x51e60a`).
    ///
    /// `Some(v)` is the reference's 2/0 (every counted node agrees on `v`); `None` folds its two
    /// *undecided* returns together — the mixed `1` and the `total == 0` epilogue — because both
    /// resolve the same way one level up, through `DefaultState`.
    fn aggregate(&self, addon: &str) -> Option<bool> {
        let key = addon.to_ascii_lowercase();
        let mut total = 0usize;
        let mut on = 0usize;
        for (_, hash) in &self.nodes {
            if let Some(&v) = hash.get(&key) {
                total += 1;
                on += usize::from(v);
            }
        }
        match (total, on) {
            (0, _) => None,
            (_, 0) => Some(false),
            (t, o) if o == t => Some(true),
            _ => None,
        }
    }

    /// **The bit a character actually gets** — `0x51e470(addon, character, useDefault = 1)`
    /// lowered to a bool (a single-character query returns only 0 or 2).
    ///
    /// Three cases, and the reference distinguishes the last two (decision 2316, which corrects
    /// 2311's reading of them as one):
    ///
    /// * **their file has an explicit row** → that row, and nothing else is consulted;
    /// * **they have a node but no row for this addon** → [`Self::aggregate`], the explicit-only
    ///   fold over the character list (`0x51e5f0`'s self-recursion), falling to `## DefaultState`
    ///   only where that is undecided;
    /// * **no node carries their name at all** → `## DefaultState`, *not* the aggregate. The walk
    ///   compares the name at every node and advances past each mismatch
    ///   (`0x51e55a jne 0x51e611`, bypassing the stop-check at `0x51e60a`), so it exhausts with
    ///   `total == 0` and takes that epilogue. A character the list does not carry inherits
    ///   nothing. `None` — nobody picked yet — is this case too.
    ///
    /// That third arm is unreachable from our own callers: every panel column is a node, and
    /// [`store_nodes`] seats the loading character. It is written faithfully anyway, because the
    /// next caller would otherwise find the wrong branch sitting here and have no way to know.
    pub(crate) fn enabled_for(
        &self,
        addon: &str,
        default_state: bool,
        character: Option<&str>,
    ) -> bool {
        let Some(node) = character.and_then(|c| self.node(c)) else {
            return default_state;
        };
        match node.get(&addon.to_ascii_lowercase()) {
            Some(&explicit) => explicit,
            None => self.aggregate(addon).unwrap_or(default_state),
        }
    }

    fn node(&self, character: &str) -> Option<&HashMap<String, bool>> {
        self.nodes
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(character))
            .map(|(_, hash)| hash)
    }
}

/// The store's node set for a load: the realm's character list, with the loading character
/// appended when the caller had no list to give (a test, a capture, a `.go`-style entry) — so a
/// one-identity caller still gets its own file honoured, and nothing else.
fn store_nodes(identity: Option<&(String, String)>, roster: &[String]) -> Vec<String> {
    let mut names = roster.to_vec();
    if let Some((_, character)) = identity {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(character)) {
            names.push(character.clone());
        }
    }
    names
}

/// Write a character's enable state — the AddOns screen's write (decision 1197) and, since 2139,
/// the in-world logout write too.
///
/// **Merges rather than replaces, because that is what the reference's writer structurally IS.**
/// `0x51ef20` walks the in-memory enable **hash** and emits one `"%s: %s\r\n"` line per entry
/// (`0x853968`), and that hash is built by the reader `0x51ebe0` from `AddOns.txt` itself — which
/// creates entries for every line, both states, strdup'ing each name as written. So a row for an
/// addon that is not installed right now was loaded, is never removed, and is written straight back
/// out; and the spelling that survives is the FILE's, not any folder's, because the folder list
/// never enters the hash at all.
///
/// Both halves matter and both were divergent on the logout path, which used to rebuild the file
/// from `addon_enable_states()` — the installed-folder registry. One logout erased
/// `Uninstalled: disabled` outright and rewrote `myaddon:` as `MyAddon:`. The uninstall case is the
/// one with teeth: a player who removes a folder for a week loses the choice they made about it.
pub(crate) fn write_enable_state(identity: Option<&(String, String)>, states: &[(String, bool)]) {
    let Some(path) = enable_state_path(identity) else {
        return; // no character picked, or no state folder — nothing to write to
    };
    let mut merged: Vec<(String, bool)> = std::fs::read(&path)
        .ok()
        .map(|b| parse_enable_state(&benilla_ui::source::decode(&b)))
        .unwrap_or_default();
    for (name, on) in states {
        match merged
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some(row) => row.1 = *on,
            None => merged.push((name.clone(), *on)),
        }
    }
    match crate::local_state::write_atomic(&path, &render_enable_state(&merged)) {
        Ok(()) => info!("addons: wrote {} ({} rows)", path.display(), merged.len()),
        Err(e) => warn!("addons: cannot write {}: {e}", path.display()),
    }
}

/// Write the enable state back — the reference's own last shutdown step (`0x490c88`, after the
/// saved-variables files), so a `DisableAddOn` from Lua survives the session.
///
/// **The same writer the AddOns screen uses** (2139). This used to render
/// `addon_enable_states()` — the installed-folder registry — over the whole file, which is not the
/// shape of `0x51ef20`: the reference emits the enable *hash*, and that hash is the file's own
/// contents plus this session's toggles. Two things followed from the difference, both reproduced
/// before the change: a row for an addon not installed this session was dropped, and every name
/// was rewritten into its folder's case rather than the spelling the file already had.
pub(super) fn save_enable_state(script: &UiScript, identity: Option<&(String, String)>) {
    let states = script.addon_enable_states();
    if states.is_empty() {
        return; // nothing was ever registered — a glue-only run or a capture; nothing to merge in
    }
    write_enable_state(identity, &states);
}

/// Write every loaded addon's declared saved variables — the reference's own shutdown step
/// (`0x490c83`, right after the flat file and just before `AddOns.txt`).
///
/// **There is no autosave and no dirty bit**, deliberately: the reference has neither (`ds:0xb4b3f4`
/// has three references image-wide, and the write gate is the record's *loaded* byte). An addon
/// that never loaded this session has no globals to write, and writing it would blank a file it
/// never read.
///
/// An addon declaring nothing gets no file. The reference *deletes* one in that case; we simply do
/// not write, because our folder is a place a player looks and a stale file there is confusing
/// either way — recorded rather than silently different.
pub(super) fn save_addon_variables(script: &mut UiScript, identity: Option<&(String, String)>) {
    let account = crate::local_state::addon_saved_account_dir();
    let character = identity.and_then(|(r, c)| crate::local_state::addon_saved_character_dir(r, c));
    for (name, account_names, character_names) in script.addon_saved_variable_sets() {
        for (dir, names) in [(&account, &account_names), (&character, &character_names)] {
            if names.is_empty() {
                continue;
            }
            let Some(dir) = dir else { continue };
            let path = dir.join(format!("{name}.lua"));
            if script.saved_file_held(&path) {
                // It failed to load this session: its globals are the addon's defaults, and
                // writing them would replace the player's file with them.
                warn!(
                    "ui_script: {} did not load this session — left as it is",
                    path.display()
                );
                continue;
            }
            let mut body = SAVED_HEADER.as_bytes().to_vec();
            body.extend(script.saved_variables_bytes_for(names));
            match crate::local_state::write_atomic_bytes(&path, &body) {
                Ok(()) => info!("ui_script: wrote {}", path.display()),
                Err(e) => warn!("ui_script: cannot write {}: {e}", path.display()),
            }
        }
    }
    for w in script.take_warnings() {
        warn!("ui_script: saved variables: {w}");
    }
}

/// The per-addon file's header. The reference writes none (its files open with a bare blank line
/// from the `"\r\n"`-per-variable prefix); ours says what the file is, because a visible folder
/// invites a look — the same call [`crate::ui_saved`] made for the flat file.
const SAVED_HEADER: &str = "\
-- benilla per-addon saved variables (decision 1188 phase 3).
-- Written at logout/exit from the live globals; executed as a Lua chunk at addon load.
";

/// Load every discovered third-party addon, honouring `## Dependencies:` the way the reference
/// does. Returns every load error, tagged by addon.
///
/// Called at world entry, after the built-in interface's own files — the reference loads addons
/// from `UI_Init 0x48fbf0` → `0x51f600`, which is the same seam our in-game UI materializes at
/// (1051).
///
/// **`&mut` is what lets `ADDON_LOADED` fire in the right place.** [`UiScript::fire_event`] needs
/// it, and the alternative — collecting names and firing them all after the walk — is a different
/// behaviour, not a shortcut: the reference fires each addon's event before the *next* addon's
/// files run, so a deferred batch would let B's file-scope code run ahead of A's handler.
pub(super) fn load_third_party(
    script: &mut UiScript,
    identity: Option<&(String, String)>,
    roster: &[String],
    version_check: bool,
) -> Vec<String> {
    let addons = discover();
    // Register the AddOn API's view even when the list is empty: `GetNumAddOns()` must answer 0
    // rather than answer whatever a previous session left, and an addon that asks before any
    // exist is asking a real question.
    let mut infos: Vec<_> = addons.iter().map(info_for).collect();
    // The chain-sourced rows read through the reference's own store (1957).
    script.set_addon_chain_reader(Box::new(super::reference_ui::read));
    // Every addon's enable bit, through the reference's own query (decision 2311): this
    // character's explicit `AddOns.txt` row when they have one, else what the realm's other
    // characters agree on, else the manifest's `## DefaultState`. Reading an absent row as a
    // bare "enabled" is what re-enabled every addon the moment a character was created.
    let store = EnableStore::load(
        identity
            .map(|(realm, _)| realm.as_str())
            .unwrap_or_default(),
        &store_nodes(identity, roster),
    );
    let character = identity.map(|(_, c)| c.as_str());
    for (info, addon) in infos.iter_mut().zip(addons.iter()) {
        info.enabled = store.enabled_for(&info.name, addon.toc.default_state(), character);
    }
    let disabled_names: HashSet<String> = infos
        .iter()
        .filter(|i| !i.enabled)
        .map(|i| i.name.to_ascii_lowercase())
        .collect();
    script.register_addons(
        infos,
        root(),
        crate::local_state::addon_saved_account_dir(),
        identity.and_then(|(r, c)| crate::local_state::addon_saved_character_dir(r, c)),
    );
    if addons.is_empty() {
        return Vec::new();
    }
    info!(
        "ui_script: {} addon(s) found: {}",
        addons.len(),
        addons
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut state = Walk {
        disabled: disabled_names,
        version_check,
        ..Walk::default()
    };
    for addon in &addons {
        // `## LoadOnDemand: 1` means "not at startup" — the reference's `0x51f600` loads only
        // records whose LoadOnDemand byte is 0. Without `LoadAddOn()` (1178 step 4) that means
        // never, which is why it is said out loud rather than silently skipped.
        if addon.toc.load_on_demand() {
            info!(
                "ui_script: {} is LoadOnDemand — not loaded (no LoadAddOn() yet)",
                addon.name
            );
            continue;
        }
        // Re-arm the load bound PER ADDON (decision 1306): the budget is a per-addon fact (the
        // corpus was measured per addon), and without the reset one runaway would spend the
        // whole allowance and fail every addon after it for somebody else's loop. A dependency
        // chain loads under its dependent's arming, which is the same accounting the harness's
        // per-survey arming gives it.
        script.set_instruction_budget(LOAD_INSTRUCTION_BUDGET);
        let _ = state.load(script, &addons, &addon.name);
        let spent = script.instructions_used();
        if spent > 1_000_000 {
            // Heavy is worth a line: this counter is how the budget was chosen, and a report
            // like B271's needs exactly this number without a harness run.
            info!(
                "ui_script: {} spent {spent} VM instructions loading",
                addon.name
            );
        }
    }
    state.failures
}

/// The recursive load's bookkeeping — see [`Walk::load`].
#[derive(Default)]
struct Walk {
    loaded: HashSet<String>,
    /// Addons whose load already failed, so a second dependent gets the same answer without
    /// re-running (and without re-reporting) the failure.
    failed: HashSet<String>,
    /// The current dependency chain, for cycle detection and for naming the cycle when it happens.
    loading: Vec<String>,
    failures: Vec<String>,
    /// Addons the player has turned off, lowercased. **Passed in rather than read back out of the
    /// VM**: the walk's inputs stay explicit, so recursion cannot depend on registration having
    /// happened first, and an empty set means "everything enabled" — which is exactly what an
    /// absent enable-state file means too.
    disabled: HashSet<String>,
    /// The `checkAddonVersion` gate (decision 1292), resolved by the caller from the persisted
    /// CVar — passed in for the same explicitness reason as `disabled`. When on, an addon whose
    /// `## Interface` is not exactly the client's is skipped like a disabled one (the
    /// reference's `AddOn_CanLoad` check 6, before the dependency loop), and a dependent gets
    /// `Err` — its `DEP_INTERFACE_VERSION`.
    version_check: bool,
}

impl Walk {
    /// Load one addon by name, its dependencies first. `Err` means "this addon did not load", and
    /// is what a hard dependency's failure propagates.
    ///
    /// The order inside is `AddOn_Load 0x51f240`'s:
    ///
    /// > OptionalDeps (failures ignored) → RequiredDeps (a failure aborts) → **this addon's own
    /// > `.toc`-listed files, in listed order** (`0x51f3fa`) → `Bindings.xml` (`0x51f400`) → the
    /// > account SavedVariables file (`0x51f4b5`) → the per-character file (`0x51f53b`) →
    /// > **`ADDON_LOADED` (event 429) at `0x51f5ad`** → the reverse-`LoadWith` dependents.
    ///
    /// All of it is built (`Bindings.xml` is 1188 phase 4, the two saved files phase 3), and the
    /// two middle steps are why the event is fired at the very end rather than beside the load.
    /// That position is the mechanism, not a detail: a saved value overwrites the addon's own
    /// file-scope default, and `ADDON_LOADED` handlers are specified to see the *restored* value.
    fn load(&mut self, script: &mut UiScript, all: &[Addon], name: &str) -> Result<(), ()> {
        // Resolve to the addon's OWN spelling before anything is keyed on it. A `.toc` may name a
        // dependency in any case (`## Dependencies: probeaddon` against a folder `ProbeAddon`),
        // and lookup here is case-insensitive — so keying the sets on the caller's spelling would
        // let two differently-cased dependents each miss the "already loaded" check and load the
        // same addon twice, running every one of its files a second time.
        let Some(addon) = all.iter().find(|a| a.name.eq_ignore_ascii_case(name)) else {
            return Err(()); // not installed — the caller decides whether that is fatal
        };
        let key = addon.name.as_str();
        if self.loaded.contains(key) {
            return Ok(());
        }
        if self.failed.contains(key) {
            return Err(());
        }
        // **Disabled is a player's choice, not a failure** — it is skipped silently and nothing is
        // pushed onto `failures`. A dependent still gets `Err`, which is the reference's own
        // `DEP_DISABLED`: an addon whose hard dependency the player turned off cannot load either.
        if self.disabled.contains(&key.to_ascii_lowercase()) {
            info!("ui_script: {key} is disabled — not loaded");
            self.failed.insert(addon.name.clone());
            return Err(());
        }
        // **The version gate** (decision 1292; `AddOn_CanLoad` check 6, in the checks' own order —
        // after the enable state, before the dependency loop). Exact `==` against the client's
        // build; a missing `## Interface` parses as 0 and is out of date. Not a `failures` entry
        // for the same reason disabled is not: the state is the player's to see on the AddOns
        // screens (`ADDON_INTERFACE_VERSION`), and the *Load out of date AddOns* checkbox — the
        // `checkAddonVersion` CVar this flag carries — is the reference's own escape.
        if self.version_check
            && addon.toc.interface_version() != benilla_ui::script::addon_gate::CLIENT_INTERFACE
        {
            info!(
                "ui_script: {key} is out of date (## Interface: {}, client {}) — not loaded \
                 (the AddOns screen's 'Load out of date AddOns' loads it anyway)",
                addon.toc.interface_version(),
                benilla_ui::script::addon_gate::CLIENT_INTERFACE
            );
            self.failed.insert(addon.name.clone());
            return Err(());
        }
        if self.loading.iter().any(|n| n == key) {
            let chain = self.loading.join(" → ");
            let e = format!("{key}: dependency cycle ({chain} → {key})");
            error!("ui_script: {e}");
            script.report_load_failure(&e);
            self.failures.push(e);
            self.failed.insert(addon.name.clone());
            return Err(());
        }
        self.loading.push(addon.name.clone());

        // Optional dependencies first, and a failure is genuinely ignored — that is what makes
        // them optional. It still orders the load when the dependency IS present.
        for dep in addon.toc.optional_dependencies() {
            let _ = self.load(script, all, dep);
        }
        // Hard dependencies. A failure aborts THIS addon and nothing else.
        let mut blocked = None;
        for dep in addon.toc.dependencies() {
            if self.load(script, all, dep).is_err() {
                blocked = Some(dep.to_string());
                break;
            }
        }

        self.loading.pop();
        if let Some(dep) = blocked {
            let e = format!(
                "{}: required dependency {dep} is missing or failed",
                addon.name
            );
            error!("ui_script: {e}");
            // The one failure a player can usually FIX themselves — install the dependency — and
            // until 1495 the only place it was said was the terminal.
            script.report_load_failure(&e);
            self.failures.push(e);
            self.failed.insert(addon.name.clone());
            return Err(());
        }

        self.failures.extend(addon.load(script));
        // ── `Bindings.xml` (1188 phase 4) ── the verified position `0x51f400`: after this addon's
        // own files (whose functions a binding body calls), before its saved variables. Read
        // through the addon's own reader, so the AddOns-root sandbox (1186) covers it like every
        // other file it loads; absent is the normal case and silent.
        let bindings_xml = benilla_ui::loader::join_ref(&addon.prefix(), "Bindings.xml");
        if let Some(bytes) = addon.read(&bindings_xml) {
            match benilla_ui::bindings_xml::parse(&benilla_ui::source::decode(&bytes)) {
                Ok(bindings) => script.register_addon_bindings(&addon.name, &bindings),
                Err(e) => {
                    let e = format!("{}/Bindings.xml: {e}", addon.name);
                    error!("ui_script: {e}");
                    script.report_load_failure(&e);
                    self.failures.push(e);
                }
            }
        }
        // The two saved-variables files, account then per-character — the verified position
        // (`0x51f4b5`, `0x51f53b`): after the addon's own files assigned their defaults, before
        // `ADDON_LOADED` whose handlers must see the restored value.
        script.load_addon_saved_variables(&addon.name);
        self.loaded.insert(addon.name.clone());
        script.mark_addon_loaded(&addon.name);
        // `ADDON_LOADED`, `arg1` = the addon's own folder name — the spelling the addon knows
        // itself by, not the caller's (a dependent may name it in any case). Marked loaded first,
        // because a handler is free to ask `IsAddOnLoaded` about itself and the honest answer is
        // yes: the reference sets `[rec+0x18]` before `0x51f5ad`.
        //
        // **The builtin cannot reach this**, which is the rule rather than a filter: FrameXML does
        // not go through `AddOn_Load` and gets no `ADDON_LOADED` (same source), and our
        // `benilla.toc` is loaded by `manifest::load_ingame_ui` while this walk only ever sees
        // what `discover()` found under the AddOns root.
        script.fire_event("ADDON_LOADED", vec![ScriptValue::Str(addon.name.clone())]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    /// **The walk order is NTFS's, not byte order** — the fact 2166 §7 left open, pinned here.
    ///
    /// The reference's loose pass is `FindFirstFileW` and sorts nothing
    /// (`0x42ad10`), so its order is the host filesystem's; NTFS's `$I30`
    /// collation is the one we adopt, because it is the order the entire vanilla corpus was
    /// written against. Two rules distinguish it from `str`'s own `Ord`, and this asserts both:
    /// **case folds away** (so a lowercase-initial name sorts among its letter's block rather
    /// than after every uppercase name) and **`_` sorts after `Z`** (0x5F > 0x5A) rather than
    /// between the cases.
    ///
    /// The names are the corpus's own, and each pair is one that actually moved when this
    /// replaced `names.sort()`.
    #[test]
    fn the_walk_orders_names_the_way_ntfs_lists_a_directory() {
        let mut names: Vec<String> = [
            "_LazyPig",
            "Zorlen",
            "zBar",
            "oRA2",
            "FuBar_TinyTipFu",
            "Fubar_EmoteFu",
            "!OmniCC",
            "Ace2",
            "AceGUI",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        sort_by_directory_order(&mut names);

        assert_eq!(
            names,
            vec![
                // `!` (0x21) is ahead of everything — which is why the corpus's `!`-prefix
                // convention ever worked, and evidence the real walk is name-collated.
                "!OmniCC",
                // `Ace2` before `AceGUI`: the digit `2` (0x32) beats `G` (0x47), case-blind.
                "Ace2",
                "AceGUI",
                // Case folded: `Fubar_EmoteFu` lands next to its `FuBar_*` siblings instead of
                // after every one of them. `E` < `T` decides it, not `u` vs `B`.
                "Fubar_EmoteFu",
                "FuBar_TinyTipFu",
                // Lowercase initials sort in their own letter's block, not after `Z`.
                "oRA2",
                "zBar",
                "Zorlen",
                // `_` (0x5F) sorts AFTER every letter, so this is last rather than mid-list.
                "_LazyPig",
            ]
        );

        // Byte order — what we did before — disagrees on all three rules. Asserted so the test
        // fails if someone "simplifies" the key back to `str`'s `Ord`.
        let mut plain = names.clone();
        plain.sort();
        assert_ne!(plain, names, "NTFS collation is not byte order");
    }

    /// The tie `COLLATION_FILE_NAME` specifies: names equal but for case fall back to a
    /// **case-sensitive** compare of the raw units, so uppercase (`A` 0x41) precedes lowercase
    /// (`a` 0x61). NTFS itself cannot hold both — it is case-insensitive — but a case-sensitive
    /// host filesystem can, and without the tiebreak the sort would be stable-on-`read_dir` and
    /// therefore not deterministic at all, which is the property [`installed`] depends on.
    #[test]
    fn names_equal_but_for_case_break_the_tie_uppercase_first() {
        let mut names: Vec<String> = ["fubar", "FuBar", "FUBAR"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        sort_by_directory_order(&mut names);
        assert_eq!(names, vec!["FUBAR", "FuBar", "fubar"]);
    }

    /// `$UpCase` is 65536 UTF-16 entries wide, so it can only express a length-preserving
    /// mapping: `ß` has no single-unit uppercase and the table leaves it alone, where Rust's full
    /// `to_uppercase` would expand it to `SS` and sort it under `S`.
    #[test]
    fn the_upcase_fold_is_one_to_one_like_the_table_it_models() {
        assert_eq!(upcase_unit('a'), 'A');
        assert_eq!(upcase_unit('_'), '_');
        assert_eq!(upcase_unit('ß'), 'ß');
        assert_eq!(upcase_unit('é'), 'É');
    }

    /// **The AddOns screen's rows are in the glue list's order — `## Title`, case-insensitively,
    /// with the folder name as the fallback** (decision 2175).
    ///
    /// The reference's `AddonList_Update` walks `GetAddOnInfo(i)` for `i = 1..GetNumAddOns()`, and
    /// glue `0x46d460` resolves that index through the same `0x51df00` array the in-game binding
    /// does — `## Title`-sorted by comparator `0x51deb0`. This screen reads the folder instead
    /// (1197, so the list works before any VM has addons in it), which is why it showed folder
    /// order until now.
    ///
    /// The three folder names and the three titles are deliberately *different permutations*, and
    /// `Middle` is the case control: a byte-wise sort would answer `Middle, aardvark, zebra`.
    #[test]
    fn the_addons_screen_lists_in_title_order_not_folder_order() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("listorder");
        write_addon(
            &home,
            "Alpha",
            "## Interface: 11200\n## Title: zebra\n",
            &[],
        );
        write_addon(
            &home,
            "Mike",
            "## Interface: 11200\n## Title: Middle\n",
            &[],
        );
        write_addon(
            &home,
            "Zulu",
            "## Interface: 11200\n## Title: aardvark\n",
            &[],
        );
        // No `## Title` at all — sorts under its folder name, the comparator's own fallback.
        write_addon(&home, "bravo", "## Interface: 11200\n", &[]);

        let rows = installed_rows();
        assert_eq!(
            rows.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
            vec!["Zulu", "bravo", "Mike", "Alpha"],
            "titles: {:?}",
            rows.iter()
                .map(InstalledAddOn::display_title)
                .collect::<Vec<_>>()
        );
    }

    /// **The archive pass registers before the loose one, and wins a duplicate** (decision 2175).
    ///
    /// `AddOn_ScanAddOnDir 0x51c760` runs `0x401470` over each mounted archive's `(listfile)` at
    /// `0x51c777` and only then `0x42ad10`'s `FindFirstFileW` walk at `0x51c78f`; both funnel into
    /// `0x51c9b0`, whose name-hash probe **returns immediately on a hit** (`0x51ca10`), and the
    /// list is tail-inserted with no comparison of any kind (`0x521ad0` mode 2). So registry order
    /// is registration order, and a name in both sources is registered once, by the archive.
    ///
    /// We had it the other way round. The duplicate is the half with teeth: before this, a player
    /// folder named `Blizzard_TalentUI` produced TWO records with one name, and every name lookup
    /// found only the first.
    #[test]
    fn the_archive_pass_registers_first_and_wins_a_duplicate() {
        let _data = benilla_formats::wow_data_or_skip!();
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("scanorder");
        let chain = chain_addons();
        if chain.is_empty() {
            eprintln!("skipping: the chain carries no Blizzard addon rows");
            return;
        }
        // A loose folder that COLLIDES with an archive name, and one that does not.
        let collide = chain[0].name.clone();
        write_addon(
            &home,
            &collide,
            "## Interface: 11200\n## Title: Impostor\n",
            &[],
        );
        write_addon(&home, "Loose", "## Interface: 11200\n", &[]);

        let found = discover();
        let names: Vec<&str> = found.iter().map(|a| a.name.as_str()).collect();

        // Every archive row precedes every loose one.
        let first_loose = names.iter().position(|n| *n == "Loose").expect("Loose");
        assert_eq!(
            first_loose,
            chain.len(),
            "the loose folder follows all {} archive rows: {names:?}",
            chain.len()
        );

        // The collision is registered ONCE, and by the archive — the loose `## Title: Impostor`
        // never reaches the registry.
        assert_eq!(
            names.iter().filter(|n| **n == collide).count(),
            1,
            "{collide} is registered once: {names:?}"
        );
        let row = found.iter().find(|a| a.name == collide).unwrap();
        assert!(
            matches!(row.source, Source::Chain),
            "the archive's copy won"
        );
        assert_ne!(
            row.toc.directive("Title"),
            Some("Impostor"),
            "the loose manifest did not overwrite the archive's"
        );
    }

    /// The chain's Blizzard LoadOnDemand addons are registry rows (1957): every row is
    /// LoadOnDemand and read off the chain, the eight windows the interface opens through them
    /// are among the rows, and the manifest lists no addon at all — the reference's own
    /// `FrameXML.toc` has none, every Blizzard addon loads on demand (1967).
    #[test]
    fn the_chain_carries_blizzards_load_on_demand_addons_as_registry_rows() {
        let _data = benilla_formats::wow_data_or_skip!();
        let rows = chain_addons();
        assert!(
            rows.iter().any(|a| a.name == "Blizzard_TrainerUI"),
            "the trainer addon is a row: {:?}",
            rows.iter().map(|a| &a.name).collect::<Vec<_>>()
        );
        for a in &rows {
            assert!(a.toc.load_on_demand(), "{} is LoadOnDemand", a.name);
            assert!(
                matches!(a.source, Source::Chain),
                "{} comes off the chain",
                a.name
            );
            assert!(
                info_for(a).chain,
                "{}'s registry row is marked chain-sourced",
                a.name
            );
        }
        for name in [
            "Blizzard_AuctionUI",
            "Blizzard_CraftUI",
            "Blizzard_InspectUI",
            "Blizzard_MacroUI",
            "Blizzard_RaidUI",
            "Blizzard_TalentUI",
            "Blizzard_TradeSkillUI",
            "Blizzard_TrainerUI",
        ] {
            assert!(
                rows.iter().any(|a| a.name == name),
                "{name} is a row the interface opens on demand"
            );
        }
        assert!(
            !Addon::builtin()
                .toc
                .files
                .iter()
                .any(|f| f.replace('/', "\\").starts_with("Interface\\AddOns\\")),
            "benilla.toc lists a Blizzard addon eagerly; the reference loads every one on demand"
        );
        // The glue's list is the player's folder alone.
        assert!(installed_rows()
            .iter()
            .all(|a| !a.name.starts_with("Blizzard_")));
    }

    use super::*;

    /// Build an addon over a temp AddOns root, so the walk can be tested without an install.
    fn dir_addon(root: &Path, name: &str, toc: &str) -> Addon {
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("{name}.toc")), toc).unwrap();
        Addon {
            name: name.to_string(),
            toc: Toc::parse(toc),
            source: Source::Dir(root.to_path_buf()),
        }
    }

    /// **The sandbox is the AddOns folder** (decision 1186): a sibling addon is reachable, the
    /// machine is not — now expressed as *only a path under `Interface/AddOns/` touches the
    /// filesystem at all* (2155), which is the same rule one level up and strictly tighter.
    ///
    /// 1184 drew the line at each addon's own folder, which reads as the safer choice and breaks
    /// the single most common structure in the ecosystem — a shared library addon exists precisely
    /// to be included by its dependents (`Bagnon/src/main.xml` reaches
    /// `..\..\BagBrother\core\core.xml`, and BagBrother's own `.toc` lists no files at all).
    ///
    /// The escape cases go through [`benilla_ui::loader::join_ref`] first, exactly as the loader
    /// feeds them, because that is what turns an escape into the leading `..` the guard sees.
    #[test]
    fn the_sandbox_is_the_addons_root_not_one_addon() {
        use benilla_ui::loader::join_ref;
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-escape-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let root = tmp.join("AddOns");
        std::fs::create_dir_all(root.join("Probe/src")).unwrap();
        std::fs::create_dir_all(root.join("ProbeLib/core")).unwrap();
        std::fs::write(tmp.join("secret.txt"), "no").unwrap();
        std::fs::write(root.join("Probe/src/own.txt"), "yes").unwrap();
        std::fs::write(root.join("ProbeLib/core/lib.xml"), "sibling").unwrap();
        let addon = Addon {
            name: "Probe".into(),
            toc: Toc::default(),
            source: Source::Dir(root),
        };

        // **Every path here is install-relative now** (2155) — the base is
        // `Interface/AddOns/<Folder>`, which is what `prefix()` hands the loader.
        assert_eq!(addon.prefix(), "Interface/AddOns/Probe");
        let src = "Interface/AddOns/Probe/src";

        // Its own file, and a sibling library addon reached the way a real addon reaches one.
        assert_eq!(
            addon.read(&join_ref(src, "own.txt")).as_deref(),
            Some(&b"yes"[..])
        );
        assert_eq!(
            addon
                .read(&join_ref(src, "..\\..\\ProbeLib\\core\\lib.xml"))
                .as_deref(),
            Some(&b"sibling"[..]),
            "a shared library addon must be reachable — this is what 1184 wrongly blocked"
        );

        // Above the AddOns root touches no file at all: the collapsed path leaves
        // `Interface/AddOns/`, so it never reaches `read_under` and can only ask the archive chain
        // — which is stricter than the leading-`..` refusal this replaced, because a `..` that
        // lands back INSIDE the install (`Interface/FrameXML/…`) is now a chain name rather than a
        // filesystem join. `secret.txt` sits beside the AddOns root and is unreachable either way.
        assert!(addon.read(&join_ref(src, "../../../secret.txt")).is_none());
        assert!(addon
            .read(&join_ref("Interface/AddOns/Probe", "..\\secret.txt"))
            .is_none());
        assert!(
            addon
                .read(&join_ref(src, "../../../../../../../../etc/passwd"))
                .is_none(),
            "over-consuming `..` cannot walk out of the install either"
        );
        // A leading `/` is not "the filesystem root" — `join_ref` re-roots it at the base it is
        // given, and with no base that is a bare relative name, which is not under
        // `Interface/AddOns/` and so is a chain lookup that misses.
        assert_eq!(join_ref("", "/etc/hosts"), "etc/hosts");
        assert!(addon.read(&join_ref("", "/etc/hosts")).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **A `.toc` that lists its own `Bindings.xml` loads without a script error** (decision 2191)
    /// — MonkeyDev's manifest, the director's live report: three red dialogs reading
    /// `CreateFrame(<Binding> name="MONKEYDEV_STEPUP"): unknown frame type`.
    ///
    /// The `.toc` line runner hands every non-`.lua` entry to the one file loader, so the file is
    /// walked as FrameXML and each `<Binding>` is a frame element of no registered type. The
    /// reference logs `"Unknown frame type: %s"` for each and carries on (`0x6ee356`); it raises
    /// nothing, and the file's reading AS bindings is `0x51f400`'s, which happens anyway.
    #[test]
    fn a_toc_listed_bindings_xml_is_not_a_script_error() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-tocbindings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let root = tmp.join("AddOns");
        std::fs::create_dir_all(root.join("Probe")).unwrap();
        std::fs::write(
            root.join("Probe/Bindings.xml"),
            r#"<Bindings>
  <Binding name="PROBE_STEPUP" header="PROBE">ProbeStep_Inc()</Binding>
  <Binding name="PROBE_STEPDOWN">ProbeStep_Dec()</Binding>
</Bindings>"#,
        )
        .unwrap();
        std::fs::write(root.join("Probe/Probe.lua"), "PROBE_RAN = 1").unwrap();
        let addon = Addon {
            name: "Probe".into(),
            toc: Toc::parse("## Interface: 11200\nBindings.xml\nProbe.lua\n"),
            source: Source::Dir(root),
        };

        let script = UiScript::new().unwrap();
        let failures = addon.load(&script);
        assert!(
            failures.is_empty(),
            "a listed Bindings.xml costs log lines, not script errors: {failures:?}"
        );
        assert!(
            script.eval::<bool>("return PROBE_RAN == 1").unwrap(),
            "and the manifest carries on to the next line"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **A `.toc`-listed `.lua` and an XML-referenced one get the SAME chunk name** — the defect
    /// decision 2155 found, asserted from the outside because it is only visible from there.
    ///
    /// Every Ace2-era library finds the addon it belongs to by splitting a `debugstack` frame on
    /// `\AddOns\` — `AceDB-2.0.lua:742`'s `".-\n.-\\AddOns\\(.-)\\.*"`,
    /// `FuBarPlugin-2.0.lua:602`'s `string.find(debugstack(6,1,0), "\\AddOns\\(.*)\\")`. While a
    /// `Dir` addon's path space was the AddOns folder, a `<Script file=>` chunk was named
    /// `@AtlasLoot\Core\AtlasLoot.lua` and that split found **nothing**: `SSHonor_Fu` called
    /// `IsAddOnLoadOnDemand(nil)` and AtlasLoot's AceDB keyed itself on a raw traceback. 80 of the
    /// 219-addon corpus ship at least one `<Script file=>`.
    ///
    /// The reference names both the same way: a `.toc` line resolves against
    /// `Interface\AddOns\<Addon>\` and a bare `<Script file=X>` against `dirname(referrer)`
    /// (`0x6ee07b`) — the same directory — and the chunk name is `"@%s"` over the resolved path in
    /// both cases (`0x704bc0`).
    #[test]
    fn an_xml_referenced_lua_is_named_like_a_toc_listed_one() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-chunkname-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let root = tmp.join("AddOns");
        std::fs::create_dir_all(root.join("Probe/Core")).unwrap();
        std::fs::write(
            root.join("Probe/listed.lua"),
            "LISTED = debugstack(1, 1, 0)",
        )
        .unwrap();
        std::fs::write(
            root.join("Probe/Core/viaxml.lua"),
            "VIAXML = debugstack(1, 1, 0)",
        )
        .unwrap();
        std::fs::write(
            root.join("Probe/Core/doc.xml"),
            r#"<Ui><Script file="viaxml.lua"/></Ui>"#,
        )
        .unwrap();
        let toc = "## Interface: 11200\nlisted.lua\nCore\\doc.xml\n";
        let addon = Addon {
            name: "Probe".into(),
            toc: Toc::parse(toc),
            source: Source::Dir(root),
        };

        let script = UiScript::new().unwrap();
        assert!(addon.load(&script).is_empty());

        // The exact pattern the libraries run, on each file's own traceback.
        let folder = |global: &str| -> Option<String> {
            script
                .eval::<Option<String>>(&format!(
                    "local _,_,f = string.find({global} or \"\", \"\\\\AddOns\\\\(.-)\\\\\") return f"
                ))
                .unwrap()
        };
        assert_eq!(
            folder("LISTED").as_deref(),
            Some("Probe"),
            "a manifest-listed file has always been named right"
        );
        assert_eq!(
            folder("VIAXML").as_deref(),
            Some("Probe"),
            "and an XML-referenced one must be named the same way — this was nil"
        );
        // …and literally, so this cannot pass for some other reason: both chunks carry the
        // client's own full virtual path, which is the thing `\AddOns\` is being split out of.
        for g in ["LISTED", "VIAXML"] {
            let frame = script.eval::<String>(&format!("return {g}")).unwrap();
            assert!(
                frame.contains("Interface\\AddOns\\Probe\\"),
                "{g} is named after the install path, not the AddOns root: {frame}"
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A missing hard dependency drops exactly the dependent, and an optional one drops nothing.
    ///
    /// This is the behaviour a flat topological sort cannot express, and the reason the walk is
    /// recursive like `AddOn_Load 0x51f240` rather than a pre-sort.
    #[test]
    fn a_missing_required_dep_drops_only_its_dependent() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-deps-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let all = vec![
            dir_addon(&tmp, "Alone", "## Interface: 11200\n"),
            dir_addon(&tmp, "NeedsGhost", "## Dependencies: Ghost\n"),
            dir_addon(&tmp, "WantsGhost", "## OptionalDeps: Ghost\n"),
        ];
        let mut script = UiScript::new().unwrap();
        let mut w = Walk::default();
        for a in &all {
            let _ = w.load(&mut script, &all, &a.name);
        }
        assert!(w.loaded.contains("Alone"), "an independent addon loads");
        assert!(
            w.loaded.contains("WantsGhost"),
            "a MISSING OPTIONAL dependency must not block its dependent"
        );
        assert!(
            !w.loaded.contains("NeedsGhost"),
            "a missing REQUIRED dependency must block its dependent"
        );
        assert_eq!(
            w.failures.len(),
            1,
            "and reports exactly that one: {:?}",
            w.failures
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **1178's falsifier, as far as step 2 can carry it**: a real third-party addon — a `.toc`, a
    /// frame in XML, a second file reached by `<Include>`, and Lua that calls a client-API global
    /// our own interface uses — loads from a folder, with no Rust written for it.
    ///
    /// This is the check the unit tests above structurally cannot give. They exercise the walk
    /// with empty manifests; this one goes through `framexml::parse` → `loader::load` → the VM, so
    /// it is the first thing that would fail if per-addon file scoping, the `<Include>` provider,
    /// or the shared global namespace were wrong. `ADDON_LOADED` is covered by
    /// [`addon_loaded_carries_the_addons_own_name_and_fires_after_its_files`]; what remains of the
    /// falsifier is saved variables (1188 phase 3).
    #[test]
    fn a_third_party_addon_loads_from_a_folder_with_no_rust() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-e2e-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("ProbeAddon");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(tmp.join("ProbeLib/core")).unwrap();
        // **Bagnon's shape, deliberately** — the manifest names ONE file in a subfolder with a
        // backslash path, and that file reaches a sibling by bare name and a shared library addon
        // by `..\..`. Every part of this is what the real addon does, and every part of it failed
        // before 1186.
        std::fs::write(
            dir.join("ProbeAddon.toc"),
            "## Interface: 11200\n## Title: Probe\nsrc\\main.xml\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/main.xml"),
            r#"<Ui>
  <Include file="templates.xml"/>
  <Include file="..\..\ProbeLib\core\lib.xml"/>
  <Script file="core.lua"/>
  <Frame name="ProbeAddonFrame" parent="UIParent">
    <Size><AbsDimension x="100" y="50"/></Size>
    <Anchors><Anchor point="CENTER"/></Anchors>
    <Scripts><OnLoad>ProbeAddonLoaded = GetTime() ~= nil and ProbeAddonGreeting == 'hello'</OnLoad></Scripts>
  </Frame>
</Ui>"#,
        )
        .unwrap();
        std::fs::write(dir.join("src/core.lua"), "ProbeAddonGreeting = 'hello'\n").unwrap();
        std::fs::write(
            dir.join("src/templates.xml"),
            "<Ui><Script>ProbeAddonInclude = true</Script></Ui>",
        )
        .unwrap();
        // The sibling library addon, reached across the AddOns root — and it includes a file of
        // its OWN by bare name, so the base has to follow the include down a level rather than
        // staying on the includer's.
        std::fs::write(
            tmp.join("ProbeLib/core/lib.xml"),
            "<Ui><Include file=\"deep.xml\"/><Script>ProbeLibLoaded = true</Script></Ui>",
        )
        .unwrap();
        std::fs::write(
            tmp.join("ProbeLib/core/deep.xml"),
            "<Ui><Script>ProbeLibDeep = true</Script></Ui>",
        )
        .unwrap();

        let addon = Addon {
            name: "ProbeAddon".into(),
            toc: Toc::parse(&std::fs::read_to_string(dir.join("ProbeAddon.toc")).unwrap()),
            source: Source::Dir(tmp.clone()),
        };
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        let failures = addon.load(&script);
        assert!(failures.is_empty(), "addon load errors: {failures:#?}");

        assert_eq!(
            script.eval::<bool>("return ProbeAddonInclude == true").ok(),
            Some(true),
            "a bare-name <Include> resolved against the INCLUDING FILE's directory (src/), not \
             the addon root — Bagnon's `templates.xml` missed entirely before 1186"
        );
        assert_eq!(
            script.eval::<bool>("return ProbeLibLoaded == true").ok(),
            Some(true),
            "`..\\..\\ProbeLib\\core\\lib.xml` reached a SIBLING addon — the shared-library \
             pattern 1184's per-addon sandbox blocked"
        );
        assert_eq!(
            script.eval::<bool>("return ProbeLibDeep == true").ok(),
            Some(true),
            "and the sibling's own bare-name <Include> resolved against ITS folder, so the base \
             follows the include tree down rather than staying on the includer"
        );
        assert_eq!(
            script
                .eval::<bool>("return ProbeAddonGreeting == 'hello'")
                .ok(),
            Some(true),
            "a <Script file=> ran, resolved the same relative way as an <Include>"
        );
        assert_eq!(
            script.eval::<bool>("return ProbeAddonLoaded == true").ok(),
            Some(true),
            "the frame's OnLoad ran, reached a client-API global (GetTime), AND saw the value the \
             manifest's earlier .lua file set — so the two kinds load into one shared state, in \
             manifest order"
        );
        assert_eq!(
            script
                .eval::<bool>("return getglobal('ProbeAddonFrame') ~= nil")
                .ok(),
            Some(true),
            "the addon's frame materialized under the same global namespace ours use"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// An addon named in two different cases by two dependents loads **once**.
    ///
    /// Dependency lookup is case-insensitive (a `.toc` may spell a dependency however it likes),
    /// so the loaded/failed sets have to be keyed on the addon's own spelling. Keyed on the
    /// caller's, each differently-cased dependent misses the "already loaded" check and runs every
    /// one of the shared addon's files again — re-registering its templates and re-materializing
    /// its frames on top of themselves.
    #[test]
    fn a_dependency_named_in_another_case_loads_once() {
        let tmp = std::env::temp_dir().join(format!(
            "benilla-addon-case-key-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Shared.xml increments a counter, so "loaded twice" is observable rather than inferred.
        let shared = tmp.join("Shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("Shared.toc"), "## Interface: 11200\nBump.xml\n").unwrap();
        std::fs::write(
            shared.join("Bump.xml"),
            "<Ui><Script>SharedLoads = (SharedLoads or 0) + 1</Script></Ui>",
        )
        .unwrap();
        let all = vec![
            Addon {
                name: "Shared".into(),
                toc: Toc::parse(&std::fs::read_to_string(shared.join("Shared.toc")).unwrap()),
                source: Source::Dir(tmp.clone()),
            },
            dir_addon(&tmp, "UpperDep", "## Dependencies: SHARED\n"),
            dir_addon(&tmp, "LowerDep", "## Dependencies: shared\n"),
        ];
        let mut script = UiScript::new().unwrap();
        let mut w = Walk::default();
        for a in &all {
            let _ = w.load(&mut script, &all, &a.name);
        }
        assert!(w.failures.is_empty(), "no failures: {:?}", w.failures);
        assert_eq!(
            script.eval::<i64>("return SharedLoads").ok(),
            Some(1),
            "the shared dependency's files ran exactly once"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A dependency cycle is reported once and does not recurse forever.
    #[test]
    fn a_dependency_cycle_is_caught() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-cycle-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let all = vec![
            dir_addon(&tmp, "Ping", "## Dependencies: Pong\n"),
            dir_addon(&tmp, "Pong", "## Dependencies: Ping\n"),
        ];
        let mut script = UiScript::new().unwrap();
        let mut w = Walk::default();
        for a in &all {
            let _ = w.load(&mut script, &all, &a.name);
        }
        assert!(w.loaded.is_empty(), "neither side of a cycle loads");
        assert!(
            w.failures.iter().any(|f| f.contains("cycle")),
            "the cycle is named: {:?}",
            w.failures
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// There is exactly ONE addon root and it is ours (decision 1185) — and a capture has none,
    /// so a baseline cannot depend on what is installed on the machine that runs it (0008).
    ///
    /// The capture arm is `local_state::home()`'s, already hermetic under `$WOW_CAPTURE`, which is
    /// why passing `None` here IS the capture case rather than a stand-in for it.
    #[test]
    fn there_is_one_addon_root_and_it_is_ours() {
        assert_eq!(
            root_from(Some(PathBuf::from("/state"))),
            Some(PathBuf::from("/state/AddOns"))
        );
        assert_eq!(root_from(None), None, "a capture run has no addon root");
    }

    /// A manifest's `.lua` entries run as chunks; everything else is FrameXML.
    ///
    /// The classifier, not the load — `\` is a path separator in a `.toc` written for Windows, and
    /// the extension compare is case-insensitive for the same reason discovery's is.
    #[test]
    fn lua_entries_are_told_apart_from_framexml() {
        assert!(is_lua("Core.lua"));
        assert!(is_lua("Libs\\LibStub\\LibStub.LUA"));
        assert!(is_lua("deep/nested/file.Lua"));
        assert!(!is_lua("Frames.xml"));
        assert!(!is_lua("Bindings.XML"));
        assert!(!is_lua("README"));
        assert!(!is_lua("weird.lua.xml"));
    }

    /// An addon folder with no matching `.toc` is not an addon, and one whose `.toc` differs only
    /// in case still is — the reference is a case-insensitive filesystem and a real addon shipped
    /// as `MyAddon/myaddon.toc` has to load on Linux too.
    #[test]
    fn discovery_matches_the_manifest_case_insensitively() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-case-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("Cased")).unwrap();
        std::fs::create_dir_all(tmp.join("Bare")).unwrap();
        std::fs::write(tmp.join("Cased/cased.TOC"), "## Interface: 11200\n").unwrap();
        std::fs::write(tmp.join("Bare/notes.txt"), "not an addon").unwrap();
        assert!(manifest_in(&tmp.join("Cased"), "Cased").is_some());
        assert!(manifest_in(&tmp.join("Bare"), "Bare").is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ───────────────────────────── lifecycle events (1188 phase 1) ─────────────────────────────

    /// A frame that appends `"<event>:<arg1>"` to a global for every event it is registered for —
    /// the witness the lifecycle tests read. Written as an addon file rather than injected into
    /// the VM, so the events travel the same `<OnEvent>` path a real addon's would.
    fn recorder_xml(events: &[&str]) -> String {
        let registers: String = events
            .iter()
            .map(|e| format!("this:RegisterEvent(\"{e}\");"))
            .collect();
        format!(
            r#"<Ui>
  <Frame name="EventProbeFrame" parent="UIParent">
    <Scripts>
      <OnLoad>{registers}</OnLoad>
      <OnEvent>table.insert(EventLog, event .. ":" .. tostring(arg1));</OnEvent>
    </Scripts>
  </Frame>
</Ui>"#
        )
    }

    /// Read the witness back as a `Vec<String>`.
    fn event_log(script: &UiScript) -> Vec<String> {
        script
            .eval::<Vec<String>>("return EventLog")
            .expect("EventLog")
    }

    /// **`ADDON_LOADED` reaches a third-party addon, carries its own name, and fires after its
    /// files** — 1188 phase 1's acceptance test, and the half of 1178's falsifier that said
    /// *"today it loads and reads; it does not receive"*.
    ///
    /// The `arg1` half is not cosmetic: every addon in the ecosystem opens with
    /// `if arg1 == "MyAddon" then` and does nothing at all otherwise, so a wrong or missing arg1
    /// is indistinguishable from the event never arriving.
    #[test]
    fn addon_loaded_carries_the_addons_own_name_and_fires_after_its_files() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-loaded-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("EventProbe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("EventProbe.toc"),
            "## Interface: 11200\nprobe.lua\nprobe.xml\n",
        )
        .unwrap();
        // File-scope Lua, which the reference runs BEFORE the event. Recording the marker here and
        // asserting it below is what proves the ordering rather than merely the delivery.
        std::fs::write(
            dir.join("probe.lua"),
            "EventLog = {}\ntable.insert(EventLog, \"files-ran\")\n",
        )
        .unwrap();
        std::fs::write(dir.join("probe.xml"), recorder_xml(&["ADDON_LOADED"])).unwrap();

        let all = vec![Addon {
            name: "EventProbe".into(),
            toc: Toc::parse(&std::fs::read_to_string(dir.join("EventProbe.toc")).unwrap()),
            source: Source::Dir(tmp.clone()),
        }];
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        let mut w = Walk::default();
        let _ = w.load(&mut script, &all, "EventProbe");
        assert!(w.failures.is_empty(), "load errors: {:?}", w.failures);

        assert_eq!(
            event_log(&script),
            vec!["files-ran", "ADDON_LOADED:EventProbe"],
            "the addon's own files run first, THEN ADDON_LOADED with its own folder name as arg1 \
             (`AddOn_Load 0x51f240`: files 0x51f3fa, event 0x51f5ad)"
        );
    }

    /// **`benilla` never appears in an `ADDON_LOADED`.**
    ///
    /// In the reference, FrameXML does not go through `AddOn_Load` and gets no such event, and
    /// our own interface is FrameXML's counterpart. An addon that watches
    /// `ADDON_LOADED` to detect *another* addon would otherwise see a name no reference client
    /// ever sends.
    ///
    /// The guard is structural — the builtin loads through [`super::manifest`] and never enters
    /// [`Walk`] — so this asserts the structure holds rather than that a filter fires: it loads
    /// the builtin's own manifest entries the way production does, alongside a real walk.
    #[test]
    fn the_builtin_interface_never_fires_addon_loaded() {
        let tmp = std::env::temp_dir().join(format!(
            "benilla-addon-builtin-silent-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("EventProbe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("EventProbe.toc"),
            "## Interface: 11200\nprobe.xml\n",
        )
        .unwrap();
        std::fs::write(dir.join("probe.xml"), recorder_xml(&["ADDON_LOADED"])).unwrap();

        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        script.run("EventLog = {}").unwrap();

        let all = vec![Addon {
            name: "EventProbe".into(),
            toc: Toc::parse(&std::fs::read_to_string(dir.join("EventProbe.toc")).unwrap()),
            source: Source::Dir(tmp.clone()),
        }];
        let mut w = Walk::default();
        let _ = w.load(&mut script, &all, "EventProbe");
        // The builtin, loaded the way production loads it — through its own manifest, not the walk.
        let builtin = Addon::builtin();
        let _ = builtin.load_files(&script, builtin.toc.files.get(..1).unwrap_or_default());

        let log = event_log(&script);
        assert!(
            !log.iter().any(|e| e.contains("benilla")),
            "benilla must never appear in an ADDON_LOADED: {log:?}"
        );
        assert_eq!(
            log,
            vec!["ADDON_LOADED:EventProbe"],
            "exactly the one third-party addon announced itself"
        );
    }

    /// **The three UI-init events fire in the reference's order:** every addon's `ADDON_LOADED`,
    /// then `VARIABLES_LOADED`, then `PLAYER_LOGIN`.
    ///
    /// Byte-verified straight-line order inside `UI_Init 0x48fbf0` — `0x4900a3` loads the addons
    /// (each `0x51f5ad`), `0x4900b2` fires `VARIABLES_LOADED`, `0x490168` enters the cascade that
    /// fires `PLAYER_LOGIN`. This drives the production functions in the production order rather
    /// than re-stating it: [`super::load_third_party`] then [`super::super::finish_ui_load`].
    ///
    /// It is the ordering an addon depends on — restore state on `ADDON_LOADED`, and by
    /// `PLAYER_LOGIN` everything saved is in place.
    #[test]
    fn the_ui_init_events_fire_in_the_reference_order() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-order-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("benilla-config");
        let dir = home.join("AddOns").join("EventProbe");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("EventProbe.toc"),
            "## Interface: 11200\nprobe.xml\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("probe.xml"),
            recorder_xml(&["ADDON_LOADED", "VARIABLES_LOADED", "PLAYER_LOGIN"]),
        )
        .unwrap();

        // Hermetic: point the whole state folder at the tempdir, so discovery finds exactly this
        // addon and the saved-variables read cannot touch the machine's real file.
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let _h =
            crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());

        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        script.run("EventLog = {}").unwrap();
        let failures = load_third_party(&mut script, None, &[], true);
        assert!(failures.is_empty(), "load errors: {failures:?}");
        crate::ui_script::finish_ui_load(&mut script);

        assert_eq!(
            event_log(&script),
            vec![
                "ADDON_LOADED:EventProbe",
                "VARIABLES_LOADED:nil",
                "PLAYER_LOGIN:nil",
            ],
            "every non-LoadOnDemand addon's ADDON_LOADED precedes VARIABLES_LOADED, which \
             precedes PLAYER_LOGIN"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ────────────────────────── the AddOn API + enable state (1188 phase 2) ──────────────────────

    /// Write a whole AddOns root under a temp `benilla-config`, and point `BENILLA_HOME` at it.
    /// Returns the guards, which must be held for the duration of the test.
    fn hermetic_root(tag: &str) -> (PathBuf, crate::local_state::test_env::EnvGuard) {
        // **The pid is load-bearing**, not decoration: two `benilla_app` test binaries can run at
        // once (a concurrent session, or `--all-targets`), and a fixed path plus the
        // `remove_dir_all` below means each wipes the other's tree mid-test. `ui_saved.rs` keys
        // its temp the same way for the same reason.
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-api-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("benilla-config");
        std::fs::create_dir_all(home.join("AddOns")).unwrap();
        let guard =
            crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());
        (home, guard)
    }

    /// **The version gate holds the startup walk, and force-load opens it** (decision 1292):
    /// the byte-verified `AddOn_CanLoad` check 6 — exact `==`, missing `## Interface` = 0 = out
    /// of date, a dependent of a gated addon blocked like a dependent of a disabled one — and
    /// the `checkAddonVersion` flag (the *Load out of date AddOns* checkbox inverted) loading
    /// the very same folder in full.
    #[test]
    fn the_version_gate_holds_the_walk_and_force_load_opens_it() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (home, _guard) = hermetic_root("version-gate");
        write_addon(
            &home,
            "Fresh",
            "## Interface: 11200\nmain.lua\n",
            &[("main.lua", "FreshRan = true\n")],
        );
        write_addon(
            &home,
            "Old",
            "## Interface: 11100\nmain.lua\n",
            &[("main.lua", "OldRan = true\n")],
        );
        write_addon(
            &home,
            "Silent",
            "main.lua\n", // no ## Interface at all — parses as 0, out of date (not "unknown")
            &[("main.lua", "SilentRan = true\n")],
        );
        write_addon(
            &home,
            "NeedsOld",
            "## Interface: 11200\n## Dependencies: Old\nmain.lua\n",
            &[("main.lua", "NeedsOldRan = true\n")],
        );

        let mut script = UiScript::new().unwrap();
        let failures = load_third_party(&mut script, None, &[], true);
        assert!(
            failures.iter().any(|f| f.contains("NeedsOld")),
            "the gated dependency is the dependent's failure: {failures:?}"
        );
        assert_eq!(
            script.eval::<bool>("return FreshRan == true").ok(),
            Some(true)
        );
        assert_eq!(
            script
                .eval::<bool>("return OldRan == nil and SilentRan == nil")
                .ok(),
            Some(true),
            "out-of-date and interface-less addons are held by the gate"
        );
        assert_eq!(
            script.eval::<bool>("return NeedsOldRan == nil").ok(),
            Some(true),
            "…and so is their dependent (DEP_INTERFACE_VERSION territory)"
        );

        // Force-load: the same folder, the checkbox's other state — everything loads.
        let mut open = UiScript::new().unwrap();
        let failures = load_third_party(&mut open, None, &[], false);
        assert!(failures.is_empty(), "force-load load errors: {failures:?}");
        assert_eq!(
            open.eval::<bool>(
                "return OldRan == true and SilentRan == true and NeedsOldRan == true"
            )
            .ok(),
            Some(true),
            "'Load out of date AddOns' loads the very same folder in full"
        );
    }

    fn write_addon(home: &Path, name: &str, toc: &str, files: &[(&str, &str)]) {
        let dir = home.join("AddOns").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
        for (f, body) in files {
            std::fs::write(dir.join(f), body).unwrap();
        }
    }

    /// [`write_addon`] with **raw bytes**, because the files this crate must survive are not text.
    fn write_addon_bytes(home: &Path, name: &str, toc: &[u8], files: &[(&str, &[u8])]) {
        let dir = home.join("AddOns").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
        for (f, body) in files {
            std::fs::write(dir.join(f), body).unwrap();
        }
    }

    /// **An addon whose files are not valid UTF-8 loads anyway** (decision 1193) — the corpus's
    /// single largest blocker, and one nobody would have guessed.
    ///
    /// Three separate failures, all of which used to be silent-ish and all of which are here:
    ///
    /// 1. a **cp1252 `.toc`** made the addon *invisible to discovery* — `manifest_in` read it with
    ///    `read_to_string`, got `None`, and a folder with no readable manifest is not an addon. 5
    ///    of a real 218-addon corpus vanished this way, and the harness scored them as clean
    ///    passes because an unparsed manifest lists no files to fail on.
    /// 2. a **BOM'd `.lua`** reached the lexer with `EF BB BF` in front and died on
    ///    `unexpected symbol`. 160 corpus files carry one.
    /// 3. a **cp1252 `.lua`** read as absent, so the loader reported "not found" for a file that
    ///    was right there. `AceAddon-2.0.lua` is one, embedded in ~30 addons.
    ///
    /// The literal survives as **bytes**, not as decoded text: Lua 5.0 strings are byte strings
    /// and the reference hands `luaL_loadbuffer` the file unmodified, so `string.len` on a cp1252
    /// literal must answer what it answers there. That assertion is the one that would fail if
    /// somebody "helpfully" transcoded chunks to UTF-8 later.
    #[test]
    fn an_addon_whose_files_are_not_utf8_still_loads() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (home, _guard) = hermetic_root("encoding");
        // `## Notes:` holds a cp1252 a-umlaut (0xE4) — one byte, and the whole addon disappeared.
        let mut toc = b"## Interface: 11200\n## Title: Sch\xE4tze\nlocale.lua\nboot.lua\n".to_vec();
        toc.splice(0..0, [0xEFu8, 0xBB, 0xBF]); // ...and a BOM on the manifest too (14 in corpus).
        write_addon_bytes(
            &home,
            "Umlaut",
            &toc,
            &[
                // A cp1252 locale file: the German/French half of the vanilla ecosystem.
                ("locale.lua", &b"UmlautWord = \"Sch\xE4tze\"\n"[..]),
                // A BOM'd chunk: valid UTF-8, three bytes the lexer cannot start on.
                (
                    "boot.lua",
                    &b"\xEF\xBB\xBFUmlautLoaded = true\nUmlautLen = string.len(UmlautWord)\n"[..],
                ),
            ],
        );

        let mut script = UiScript::new().unwrap();
        let failures = load_third_party(&mut script, None, &[], true);
        assert!(failures.is_empty(), "load errors: {failures:?}");

        // Discovery saw it at all — the `.toc` decoded rather than read as absent.
        // **One**, not one-plus-the-chain (decision 2175). The registry holds the chain's twelve
        // Blizzard rows too, but the Lua index space is a different set: `SMSG_ADDON_INFO` answers
        // `status = 2` for every secure addon and the array rebuild drops them, which is why the
        // reference's own AddOns list shows the player's addons and none of Blizzard's.
        seat_stock_addon_reply(&mut script);
        assert_eq!(script.eval::<i64>("return GetNumAddOns()").ok(), Some(1));
        assert_eq!(
            script
                .eval::<String>("return GetAddOnMetadata('Umlaut', 'Title')")
                .ok(),
            Some("Sch\u{e4}tze".to_string()),
            "the manifest's cp1252 title decoded to the glyph its author typed, not to nothing"
        );
        // Both chunks ran.
        assert_eq!(
            script.eval::<bool>("return UmlautLoaded == true").ok(),
            Some(true),
            "a BOM'd .lua ran — the three-byte mark was stripped, not handed to the lexer"
        );
        // ...and the cp1252 literal is still BYTES, which is the reference's semantics.
        assert_eq!(
            script.eval::<i64>("return UmlautLen").ok(),
            Some(7),
            "`Sch\\xE4tze` is 7 bytes in the file and must be 7 bytes in Lua — transcoding the \
             chunk to UTF-8 would make it 8 and silently move every string.sub in the addon"
        );
    }

    /// The AddOns screen's write **merges** rather than replaces (decision 1197).
    ///
    /// A name in the file that is not installed right now belongs to an addon that will be again.
    /// Rewriting the file from the installed list alone forgets the player's choice on every
    /// uninstall — the same rule `parse_enable_state` already documents for the read side, and the
    /// one that has to hold on the write side or the read's carefulness is wasted.
    #[test]
    fn the_addons_screen_write_merges_with_what_is_already_on_disk() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (home, _guard) = hermetic_root("screenwrite");
        let id = ("Realm".to_string(), "Char".to_string());
        let path = enable_state_path(Some(&id)).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Gone: disabled\nStays: enabled\n").unwrap();

        write_enable_state(Some(&id), &[("Stays".into(), false), ("New".into(), true)]);

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("Gone: disabled"),
            "an uninstalled addon's choice survives — {written:?}"
        );
        assert!(
            written.contains("Stays: disabled"),
            "the edit applied — {written:?}"
        );
        assert!(
            written.contains("New: enabled"),
            "a new row appended — {written:?}"
        );
        let _ = home;
    }

    /// What [`crate::ui_script::lifecycle::load_ingame_ui_on_world_entry`] seats before the load
    /// (decision 2175): the `SMSG_ADDON_INFO` reply, hiding the secure addons the way a real
    /// server's does.
    ///
    /// A test that registers a registry and then asks an *index* question needs this, because the
    /// Lua index space does not exist until the server answers — the reference's own behaviour,
    /// not a fixture convenience. The hidden set is the same list production pairs the reply
    /// against, so a test and a live login agree by construction.
    fn seat_stock_addon_reply(script: &mut UiScript) {
        let hidden: Vec<String> = benilla_protocol::messages::STOCK_SECURE_ADDONS
            .iter()
            .map(|a| a.name.to_string())
            .collect();
        script.note_addon_info_reply(&hidden);
    }

    /// **`GetAddOnInfo` returns the manifest's own `## Title` and `## Notes`** — 1188 phase 2's
    /// first acceptance test, and the reason the registry carries directives rather than a name.
    ///
    /// The return shape is the **in-game** one — `name, title, notes, enabled, loadable, reason,
    /// security`, SEVEN values — because that is the VM this runs in.
    ///
    /// **The client registers `GetAddOnInfo` twice, as two different functions**, and this test
    /// used to assert the wrong one. It was written off `Interface\GlueXML\AddonList.lua`, which
    /// describes glue's `0x46d460`: eight values, `url` at slot 4, `newVersion` at 8. The in-game
    /// binding `0x48e390` answers seven, with **`enabled`** at slot 4 and no `url` at all.
    ///
    /// Its own doc comment named the failure it then failed to catch — *"a row that shifts by one
    /// puts an addon's notes in its URL field, which nothing would notice"*. The row was shifted at
    /// slot 4 the whole time, and the test asserted the shift. That is what a falsifier copied from
    /// the wrong reference buys: it pins the bug.
    ///
    /// What it cost: `local name, _, _, enabled, loadable = GetAddOnInfo(major)` is AceLibrary's
    /// and AceAddon's shape, reached by 70 corpus folders. They read `url` as `enabled`, and since
    /// `## URL` is rare that is nil for nearly every addon, so `if enabled and loadable` refused to
    /// load the dependency. Silent — a nil where a flag belongs is legal Lua.
    ///
    /// The whole tuple is still asserted rather than the fields the phase names, for the original
    /// and still-correct reason.
    #[test]
    fn get_addon_info_returns_the_manifests_own_title_and_notes() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("info");
        write_addon(
            &home,
            "Probe",
            "## Interface: 11200\n## Title: Probe Title\n## Notes: What it does\n## URL: http://example\n## Version: 1.2\n",
            &[],
        );
        let mut script = UiScript::new().unwrap();
        let _ = load_third_party(&mut script, None, &[], true);

        // **One**, not one-plus-the-chain (decision 2175). The registry holds the chain's twelve
        // Blizzard rows too, but the Lua index space is a different set: `SMSG_ADDON_INFO` answers
        // `status = 2` for every secure addon and the array rebuild drops them, which is why the
        // reference's own AddOns list shows the player's addons and none of Blizzard's.
        seat_stock_addon_reply(&mut script);
        assert_eq!(script.eval::<i64>("return GetNumAddOns()").ok(), Some(1));
        assert_eq!(
            script
                .eval::<Vec<String>>(
                    "local n,t,no,e,l,r,s,extra = GetAddOnInfo(1) \
                     return { n, t, no, tostring(e), tostring(l), tostring(r), s, \
                              tostring(extra) }"
                )
                .ok(),
            Some(vec![
                "Probe".into(),
                "Probe Title".into(),
                "What it does".into(),
                "1".into(), // enabled — slot 4 in-game, NOT url (which glue's binding returns)
                "1".into(), // loadable
                "nil".into(), // no reason — it loads
                "INSECURE".into(),
                // The in-game binding returns SEVEN values, so there is no eighth. Asserted as
                // absent rather than trimmed off the query: glue's `newVersion` sat here, and a
                // future edit that re-adds an eighth return has to face this line.
                "nil".into(),
            ])
        );
        // AceLibrary's and AceAddon's literal shape, which is how 70 corpus folders reach this
        // verb: `local name, _, _, enabled, loadable = GetAddOnInfo(major)`. Asserted as the
        // GUARD they actually write, not as a tuple, because the guard is what silently failed —
        // with `url` in slot 4 and no `## URL` in the manifest, `enabled` was nil and Ace declined
        // to load its own dependency without erroring.
        assert_eq!(
            script
                .eval::<bool>(
                    "local _, _, _, enabled, loadable = GetAddOnInfo('Probe') \
                     if enabled and loadable then return true else return false end"
                )
                .ok(),
            Some(true),
            "Ace's `if enabled and loadable` must pass for an enabled, loadable addon"
        );

        // Both spellings of the argument, and the raw directives.
        assert_eq!(
            script
                .eval::<String>("return (GetAddOnInfo('probe'))")
                .ok()
                .as_deref(),
            Some("Probe"),
            "index OR name, case-insensitively — the reference's verbs all take either"
        );
        assert_eq!(
            script
                .eval::<String>("return GetAddOnMetadata(1, 'Version')")
                .ok()
                .as_deref(),
            Some("1.2")
        );
        assert_eq!(
            script
                .eval::<bool>("return IsAddOnLoaded('Probe') == 1")
                .ok(),
            Some(true)
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// **Disabling an addon in the enable-state file stops it loading** — phase 2's second
    /// acceptance test.
    ///
    /// The file is the reference's own `AddOns.txt` format, `<Name>: enabled|disabled` per line,
    /// confirmed against a real 1.12 install. It also checks the direction that matters more in
    /// practice: an addon the file does not mention loads, so dropping a folder in just works.
    #[test]
    fn a_disabled_addon_does_not_load_and_an_unlisted_one_does() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("disable");
        write_addon(
            &home,
            "Off",
            "## Interface: 11200\nran.lua\n",
            &[("ran.lua", "OffRan = true")],
        );
        write_addon(
            &home,
            "On",
            "## Interface: 11200\nran.lua\n",
            &[("ran.lua", "OnRan = true")],
        );
        // "On" is deliberately absent from the file.
        std::fs::create_dir_all(home.join("addons")).unwrap();
        std::fs::write(home.join("addons/Realm-Char.txt"), "Off: disabled\n").unwrap();

        let mut script = UiScript::new().unwrap();
        let id = ("Realm".to_string(), "Char".to_string());
        let failures = load_third_party(&mut script, Some(&id), &[], true);

        assert!(
            failures.is_empty(),
            "a disabled addon is a player's choice, never a load failure: {failures:?}"
        );
        assert_eq!(
            script.eval::<bool>("return OffRan == nil").ok(),
            Some(true),
            "the disabled addon's files must not have run"
        );
        assert_eq!(
            script.eval::<bool>("return OnRan == true").ok(),
            Some(true),
            "an addon the file never mentions is enabled — a dropped-in folder just works"
        );
        // And the API agrees with the loader about what happened.
        assert_eq!(
            script
                .eval::<Vec<String>>(
                    "local _,_,_,_,l,r = GetAddOnInfo('Off') return { tostring(l), tostring(r) }"
                )
                .ok(),
            Some(vec!["nil".into(), "DISABLED".into()]),
            "not loadable, with the reference's own reason token"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// **A LoadOnDemand addon loads on `LoadAddOn` and not before** — phase 2's third acceptance
    /// test, and the one that needs the loader to run from inside a Lua binding.
    ///
    /// `LoadAddOn` is called *from Lua*, synchronously, and its side effects are asserted on the
    /// next line — exactly how `UIParentLoadAddOn` uses it. A deferred implementation would fail
    /// here, which is the point.
    #[test]
    fn a_load_on_demand_addon_loads_only_when_asked() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("lod");
        write_addon(
            &home,
            "Demand",
            "## Interface: 11200\n## Title: Demand\n## LoadOnDemand: 1\nlate.lua\nlate.xml\n",
            &[
                ("late.lua", "DemandRan = true"),
                (
                    "late.xml",
                    "<Ui><Frame name=\"DemandFrame\" parent=\"UIParent\"><Scripts>\
                     <OnEvent>DemandEvent = arg1</OnEvent></Scripts></Frame>\
                     <Script>DemandFrame:RegisterEvent(\"ADDON_LOADED\")</Script></Ui>",
                ),
            ],
        );
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        let _ = load_third_party(&mut script, None, &[], true);

        // Discovered and described, but NOT run.
        // **One**, not one-plus-the-chain (decision 2175). The registry holds the chain's twelve
        // Blizzard rows too, but the Lua index space is a different set: `SMSG_ADDON_INFO` answers
        // `status = 2` for every secure addon and the array rebuild drops them, which is why the
        // reference's own AddOns list shows the player's addons and none of Blizzard's.
        seat_stock_addon_reply(&mut script);
        assert_eq!(script.eval::<i64>("return GetNumAddOns()").ok(), Some(1));
        assert_eq!(
            script
                .eval::<bool>("return IsAddOnLoadOnDemand(1) == 1")
                .ok(),
            Some(true)
        );
        assert_eq!(
            script.eval::<bool>("return DemandRan == nil").ok(),
            Some(true),
            "a LoadOnDemand addon must not run at startup"
        );
        assert_eq!(
            script
                .eval::<bool>("return IsAddOnLoaded('Demand') == nil")
                .ok(),
            Some(true)
        );

        // ...then loads synchronously, from Lua, and is usable on the very next statement.
        assert_eq!(
            script
                .eval::<Vec<String>>(
                    "local loaded, reason = LoadAddOn('Demand') \
                     return { tostring(loaded), tostring(reason), tostring(DemandRan), \
                              tostring(getglobal('DemandFrame') ~= nil) }"
                )
                .ok(),
            Some(vec!["1".into(), "nil".into(), "true".into(), "true".into()]),
            "LoadAddOn returns loaded=1 and its files have ALREADY run when it returns — the \
             reference's UIParentLoadAddOn uses the addon's frames on the next line"
        );
        assert_eq!(
            script
                .eval::<bool>("return IsAddOnLoaded('Demand') == 1")
                .ok(),
            Some(true)
        );
        // A demand load fires ADDON_LOADED too, at the same position: after the files.
        assert_eq!(
            script.eval::<String>("return DemandEvent").ok().as_deref(),
            Some("Demand"),
            "the addon's own frame, registered by its own XML, saw its own ADDON_LOADED"
        );
        // A second load is a no-op that still answers success, as the reference does.
        assert_eq!(
            script
                .eval::<String>("local l = LoadAddOn('Demand') return tostring(l)")
                .ok()
                .as_deref(),
            Some("1")
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// **The `/msbt` shape** (decision 2102): a LoadOnDemand addon whose `## Dependencies:` names
    /// an addon the STARTUP walk already loaded.
    ///
    /// This is the whole demand-load path a player actually meets — Mik's Scrolling Battle Text
    /// ships its options as a separate `## LoadOnDemand: 1` package with
    /// `## Dependencies: MikScrollingBattleText`, and `/msbt` reaches it through
    /// `UIParentLoadAddOn`, whose failure message is the red
    /// `Couldn't load %s: <ADDON_ + reason>` dialog. The dependency being **already loaded** is the
    /// half nothing covered: a dependency the gate finds unloaded and not LoadOnDemand is
    /// `DEP_NOT_DEMAND_LOADED`, so the startup walk's `mark_addon_loaded` stamp is what stands
    /// between this and a dialog. (The addon harness answered `DEP_NOT_DEMAND_LOADED` here for
    /// exactly that reason until 2102 gave it the same stamp.)
    #[test]
    fn a_load_on_demand_addon_loads_on_top_of_an_already_loaded_dependency() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("lod-dep");
        write_addon(
            &home,
            "Base",
            "## Interface: 11200\n## Title: Base\nbase.lua\n",
            &[("base.lua", "BaseRan = true")],
        );
        write_addon(
            &home,
            "BaseOptions",
            "## Interface: 11200\n## Title: Base Options\n## Dependencies: Base\n             ## LoadOnDemand: 1\nopts.lua\n",
            &[("opts.lua", "OptsRan = BaseRan")],
        );
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        let _ = load_third_party(&mut script, None, &[], true);

        // The dependency loaded at startup and the registry knows it — the stamp this rests on.
        assert_eq!(
            script.eval::<bool>("return IsAddOnLoaded('Base') == 1").ok(),
            Some(true),
            "a startup-loaded addon must read as loaded, or every LoadOnDemand dependent of it              answers DEP_NOT_DEMAND_LOADED"
        );
        assert_eq!(
            script.eval::<bool>("return OptsRan == nil").ok(),
            Some(true),
            "and the LoadOnDemand package has not run"
        );

        // `UIParentLoadAddOn`'s own two lines, in order: load, then use what it created.
        assert_eq!(
            script
                .eval::<Vec<String>>(
                    "local loaded, reason = LoadAddOn('BaseOptions') \
                     return { tostring(loaded), tostring(reason), tostring(OptsRan), \
                              tostring(IsAddOnLoaded('BaseOptions')) }"
                )
                .ok(),
            Some(vec!["1".into(), "nil".into(), "true".into(), "1".into()]),
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// **A `.toc` line naming a file the package does not ship does not stop a demand load**
    /// (decision 2107) — 1450's rule, applied by `LoadAddOn` and not only by the startup walk.
    ///
    /// The reference logs `Couldn't open %s` and carries on
    /// (`0x6edaa0`). Ours sent the miss to the **script-error** channel
    /// instead, and the corpus paid for it the moment `LoadAddOn` became reachable at scale:
    /// FuBar's `LoadLoadOnDemandPlugins` demand-loads 55 plugins whose `.toc`s each list an
    /// `AmmoFuLocale-koKR.lua` none of them ships, and every one of them scored a session
    /// failure for a file the real client shrugs at. 150 → 89 on the vanilla corpus.
    #[test]
    fn a_missing_manifest_file_does_not_fail_a_demand_load() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("lod-missing-file");
        write_addon(
            &home,
            "Holey",
            "## Interface: 11200\n## Title: Holey\n## LoadOnDemand: 1\n\
             Locale-koKR.lua\nreal.lua\n",
            // `Locale-koKR.lua` is listed and NOT written — the FuBar plugin shape exactly.
            &[("real.lua", "HoleyRan = true")],
        );
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        let _ = load_third_party(&mut script, None, &[], true);

        assert_eq!(
            script
                .eval::<Vec<String>>(
                    "local loaded, reason = LoadAddOn('Holey') \
                     return { tostring(loaded), tostring(reason), tostring(HoleyRan), \
                              tostring(IsAddOnLoaded('Holey')) }"
                )
                .ok(),
            Some(vec!["1".into(), "nil".into(), "true".into(), "1".into()]),
            "the addon loads, its remaining files run, and it reads as loaded"
        );
        // The miss is a WARNING on the host channel, never a script error — the harness's session
        // column and `smoke.sh`'s zero-ERROR count both read the error channel, and 1450's whole
        // point is that a player's broken package cannot redden either.
        assert!(
            script.take_errors().is_empty(),
            "a missing manifest entry must not reach the script-error channel"
        );
        let warnings = script.take_warnings();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("Holey/Locale-koKR.lua") && w.contains("not found")),
            "…but it must still be said out loud: {warnings:?}"
        );
        // …and retained where the player can read it (1495), as the walk's misses are.
        assert!(
            script
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("Holey/Locale-koKR.lua")),
            "the miss is retained in the diagnostic log"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// The reason tokens are the reference's own, for the cases a caller branches on.
    ///
    /// They are not free-form strings: the reference splices them into
    /// `getglobal("ADDON_"..reason)` to find a localized label, so a token we invent renders as a
    /// nil global in somebody's addon manager.
    #[test]
    fn load_addon_answers_with_the_references_reason_tokens() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("reasons");
        write_addon(&home, "Plain", "## Interface: 11200\n", &[]);
        write_addon(
            &home,
            "Off",
            "## Interface: 11200\n## LoadOnDemand: 1\n",
            &[],
        );
        std::fs::create_dir_all(home.join("addons")).unwrap();
        std::fs::write(home.join("addons/Realm-Char.txt"), "Off: disabled\n").unwrap();

        let mut script = UiScript::new().unwrap();
        let id = ("Realm".to_string(), "Char".to_string());
        let _ = load_third_party(&mut script, Some(&id), &[], true);

        for (call, want) in [
            ("LoadAddOn('NoSuchAddon')", "MISSING"),
            ("LoadAddOn('Off')", "DISABLED"),
            // Loaded at startup because it is not LoadOnDemand, so this is the success case; the
            // NOT_DEMAND_LOADED arm needs an addon that is neither loaded nor LoadOnDemand, which
            // cannot happen once it is enabled — asserted through the disabled one above instead.
            ("LoadAddOn('Plain')", "nil"),
        ] {
            assert_eq!(
                script
                    .eval::<String>(&format!("local _, r = {call} return tostring(r)"))
                    .ok()
                    .as_deref(),
                Some(want),
                "{call}"
            );
        }
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// **A character with no enable file does not load what the realm has unanimously turned
    /// off** — the world-entry half of the director's 2026-09-17 report (decision 2311).
    ///
    /// The glue list showing a new character's addons as off would be worth nothing if the world
    /// then loaded them anyway, so the load walk resolves through the same [`EnableStore`]: the
    /// reference's `AddOn_CanLoad` check 3 is `0x51e470(name, character, useDefault = 1)`, which
    /// for a character with no explicit entry takes the explicit-only aggregate over the realm's
    /// other characters.
    ///
    /// The control is the second addon: one character left it on, so the roster does **not** agree
    /// about it, and the newcomer gets its manifest's `## DefaultState` instead — enabled. Without
    /// that half the test would pass on a client that simply disabled everything for new
    /// characters.
    #[test]
    fn a_character_with_no_file_inherits_the_rosters_unanimous_disable() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("inherit");
        write_addon(
            &home,
            "Shunned",
            "## Interface: 11200\nran.lua\n",
            &[("ran.lua", "ShunnedRan = true")],
        );
        write_addon(
            &home,
            "Contested",
            "## Interface: 11200\nran.lua\n",
            &[("ran.lua", "ContestedRan = true")],
        );
        // Two characters who have played and saved: both turned `Shunned` off, they disagree
        // about `Contested`.
        for (who, contested) in [("Onemage", false), ("Onerogue", true)] {
            let id = ("Realm".to_string(), who.to_string());
            write_enable_state(
                Some(&id),
                &[("Shunned".into(), false), ("Contested".into(), contested)],
            );
        }
        let roster = [
            "Onemage".to_string(),
            "Onerogue".to_string(),
            "Freshling".to_string(),
        ];

        // …and the character just created, who has no file of their own.
        let fresh = ("Realm".to_string(), "Freshling".to_string());
        assert!(
            !enable_state_path(Some(&fresh)).unwrap().exists(),
            "the premise: a newly created character has written nothing"
        );
        let mut script = UiScript::new().unwrap();
        let _ = load_third_party(&mut script, Some(&fresh), &roster, true);

        assert_eq!(
            script.eval::<bool>("return ShunnedRan == nil").ok(),
            Some(true),
            "the unanimous disable reaches the character who never expressed one"
        );
        assert_eq!(
            script.eval::<bool>("return ContestedRan == true").ok(),
            Some(true),
            "…and a contested addon falls to its `## DefaultState`, which is enabled"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// **A name the character list does not carry inherits nothing** — the third arm of
    /// `0x51e470(addon, character, useDefault = 1)`, and the one 2311 got wrong (decision 2316).
    ///
    /// 2311 read "no explicit row" and "no node" as one case and sent both to the aggregate. The
    /// reference splits them: the walk compares the name at every node and advances past each
    /// mismatch (`0x51e55a jne 0x51e611`, bypassing the stop-check), so an unknown name exhausts
    /// the list with `total == 0` and takes the `DefaultState ? 2 : 0` epilogue. Only a character
    /// that *has* a node inherits.
    ///
    /// Unreachable from our own callers — every panel column is a node, and `store_nodes` seats
    /// the loading character — so this is the falsifier standing in for the caller that does not
    /// exist yet. `Known`, who has a node and no row, is the control that keeps the two arms
    /// honestly distinguishable: both addons are unanimous, so an implementation that collapsed
    /// the cases would answer `false` for `Stranger` too.
    #[test]
    fn an_unknown_character_takes_the_manifest_default_not_the_aggregate() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("unknown-char");
        // One unanimous `disabled`, and one addon whose manifest ships `DefaultState: disabled`
        // so the epilogue's two outcomes are told apart rather than both reading `true`.
        write_addon(&home, "Shunned", "## Interface: 11200\n", &[]);
        write_addon(
            &home,
            "OptIn",
            "## Interface: 11200\n## DefaultState: disabled\n",
            &[],
        );
        for who in ["Onemage", "Onerogue"] {
            let id = ("Realm".to_string(), who.to_string());
            write_enable_state(
                Some(&id),
                &[("Shunned".into(), false), ("OptIn".into(), true)],
            );
        }
        let roster = [
            "Onemage".to_string(),
            "Onerogue".to_string(),
            "Known".to_string(),
        ];
        let store = EnableStore::load("Realm", &roster);

        // `Known` has a node and no rows: both addons inherit their unanimous aggregate.
        assert!(!store.enabled_for("Shunned", true, Some("Known")));
        assert!(store.enabled_for("OptIn", false, Some("Known")));

        // `Stranger` is on no node: neither aggregate reaches them — each addon answers with its
        // own manifest default, which is the opposite verdict in both cases.
        assert!(store.enabled_for("Shunned", true, Some("Stranger")));
        assert!(!store.enabled_for("OptIn", false, Some("Stranger")));
        // …and `None` (nobody picked yet) is the same epilogue.
        assert!(store.enabled_for("Shunned", true, None));
        assert!(!store.enabled_for("OptIn", false, None));
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// Enable state survives the session: `DisableAddOn` from Lua is written back in the
    /// reference's own `AddOns.txt` format, and read back as a disable next time.
    #[test]
    fn the_enable_state_round_trips_through_the_file() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("roundtrip");
        write_addon(&home, "Keep", "## Interface: 11200\n", &[]);
        write_addon(
            &home,
            "Drop",
            "## Interface: 11200\nran.lua\n",
            &[("ran.lua", "DropRan = true")],
        );
        let id = ("Realm".to_string(), "Char".to_string());

        let mut script = UiScript::new().unwrap();
        let _ = load_third_party(&mut script, Some(&id), &[], true);
        script.run("DisableAddOn('Drop')").unwrap();
        save_enable_state(&script, Some(&id));

        let written = std::fs::read_to_string(home.join("addons/Realm-Char.txt")).unwrap();
        // **The chain's Blizzard rows first, then the folder's two** — registry order, which since
        // decision 2175 is the reference's own: the archive pass registers before the loose one
        // (`0x51c777` then `0x51c78f`), and the list is tail-inserted with no sort. Every registry
        // row still gets a line; the format is the reference's one-line-per-addon.
        let mut expected = String::new();
        for a in chain_addons() {
            expected.push_str(&format!("{}: enabled\n", a.name));
        }
        expected.push_str("Drop: disabled\nKeep: enabled\n");
        assert_eq!(
            written, expected,
            "the reference's own one-line-per-addon format"
        );

        // A fresh session reads it back and the addon stays off.
        let mut next = UiScript::new().unwrap();
        let _ = load_third_party(&mut next, Some(&id), &[], true);
        assert_eq!(
            next.eval::<bool>("return DropRan == nil").ok(),
            Some(true),
            "the disable survived the session"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    // ───────────────────────────── saved variables (1188 phase 3) ─────────────────────────────

    /// **An addon sets a saved variable, the session ends, a new session sees it** — 1188 phase 3's
    /// acceptance test, end to end through the production path.
    ///
    /// It also pins the two orderings that make the mechanism work, both byte-verified:
    /// *account file then per-character file* (`0x51f4b5`, `0x51f53b`), so a per-character value
    /// wins; and *files → saved variables → `ADDON_LOADED`* (`0x51f5ad`), so the addon's own
    /// file-scope default is assigned first, overwritten second, and read by the handler third.
    /// Reverse either and the saved value can never win — which is a bug that looks exactly like
    /// "settings do not persist".
    #[test]
    fn an_addons_saved_variables_survive_the_session() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("saved");
        write_addon(
            &home,
            "Keeper",
            "## Interface: 11200\n## SavedVariables: KeeperDB\n\
             ## SavedVariablesPerCharacter: KeeperChar\nkeeper.lua\nkeeper.xml\n",
            &[
                // File-scope defaults, exactly where the reference puts them.
                (
                    "keeper.lua",
                    "KeeperDB = { count = 0 }\nKeeperChar = 'default'\n",
                ),
                (
                    "keeper.xml",
                    "<Ui><Frame name=\"KeeperFrame\"><Scripts>\
                     <OnEvent>KeeperSawAtEvent = KeeperDB.count</OnEvent></Scripts></Frame>\
                     <Script>KeeperFrame:RegisterEvent(\"ADDON_LOADED\")</Script></Ui>",
                ),
            ],
        );
        let id = ("Realm".to_string(), "Char".to_string());

        // ── session one: defaults, then the addon changes them ──
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        let failures = load_third_party(&mut script, Some(&id), &[], true);
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(
            script.eval::<i64>("return KeeperSawAtEvent").ok(),
            Some(0),
            "first run: no file yet, so ADDON_LOADED sees the file-scope default"
        );
        script
            .run("KeeperDB.count = 7 KeeperDB.note = 'hi' KeeperChar = 'mine'")
            .unwrap();
        crate::ui_script::shutdown_ui_state(&mut script, Some(&id), true);

        // ── session two: a fresh VM reads them back ──
        let mut next = UiScript::new().unwrap();
        next.set_screen_size(1024.0, 768.0);
        let failures = load_third_party(&mut next, Some(&id), &[], true);
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(
            next.eval::<i64>("return KeeperDB.count").ok(),
            Some(7),
            "the saved value overwrote the addon's own file-scope default"
        );
        assert_eq!(
            next.eval::<String>("return KeeperDB.note").ok().as_deref(),
            Some("hi"),
            "a table round-trips whole, not just the scalar that changed"
        );
        assert_eq!(
            next.eval::<String>("return KeeperChar").ok().as_deref(),
            Some("mine"),
            "the per-character file loaded too"
        );
        assert_eq!(
            next.eval::<i64>("return KeeperSawAtEvent").ok(),
            Some(7),
            "ADDON_LOADED handlers see the RESTORED value — the whole reason the event is last"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// The written bytes are the recorded grammar (decision 1128's, itself byte-verified against
    /// `0x7043f0`/`0x704480`): `NAME = value`, bracketed keys, TAB indent, a trailing comma on
    /// every entry, and the file split by scope. `["t"]`'s `[1] = 1,` is the reference's shape for
    /// a list too — its writer emits a bracketed key for every table shape and never a bare
    /// positional entry — and it reloads in index order because of what 1.12's *parser* does
    /// with it (decision 2111).
    ///
    /// Asserted as bytes rather than by re-reading, because "it round-trips through our own
    /// loader" is exactly the check that passes for a private format. The reference's own client
    /// has to be able to read this.
    #[test]
    fn the_saved_file_bytes_match_the_recorded_grammar() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("grammar");
        write_addon(
            &home,
            "Gram",
            "## Interface: 11200\n## SavedVariables: GramDB\n\
             ## SavedVariablesPerCharacter: GramChar\ngram.lua\n",
            &[("gram.lua", "GramDB = {}\nGramChar = 0\n")],
        );
        let id = ("Realm".to_string(), "Char".to_string());
        let mut script = UiScript::new().unwrap();
        let _ = load_third_party(&mut script, Some(&id), &[], true);
        script
            .run("GramDB = { ['on'] = true, ['n'] = 2, ['s'] = 'a\\\"b', ['t'] = { 1 } } GramChar = 5")
            .unwrap();
        crate::ui_script::shutdown_ui_state(&mut script, Some(&id), true);

        let account = std::fs::read_to_string(home.join("saved/Gram.lua")).unwrap();
        let body = account.lines().skip(2).collect::<Vec<_>>().join("\n");
        assert_eq!(
            body,
            "GramDB = {\n\
             \t[\"n\"] = 2,\n\
             \t[\"on\"] = true,\n\
             \t[\"s\"] = \"a\\\"b\",\n\
             \t[\"t\"] = {\n\
             \t\t[1] = 1,\n\
             \t},\n\
             }",
            "keys always bracketed and quoted, TAB indent per level, trailing comma on every \
             entry, `\\\"` escaped — the recorded grammar\nGOT:\n{account}"
        );
        // Scope split: the per-character global is in the per-character file and nowhere else.
        assert!(!account.contains("GramChar"), "account file: {account}");
        let per_char = std::fs::read_to_string(home.join("saved/Realm-Char/Gram.lua")).unwrap();
        assert!(
            per_char.ends_with("GramChar = 5\n"),
            "per-char file: {per_char}"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// `PLAYER_LOGOUT` fires **before** the writes — an addon's last chance to mutate a saved
    /// global, and worthless if it fires after.
    ///
    /// The reference's own tail is `PLAYER_LEAVING_WORLD` → `PLAYER_LOGOUT` → the writes
    /// (`0x490c2a` before `0x490c7e`/`0x490c83`). This asserts the observable consequence rather
    /// than the call order: a value set *in the handler* has to reach the file.
    #[test]
    fn player_logout_fires_before_the_write() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("logout");
        write_addon(
            &home,
            "Last",
            "## Interface: 11200\n## SavedVariables: LastDB\nlast.lua\nlast.xml\n",
            &[
                ("last.lua", "LastDB = 'unset'"),
                (
                    "last.xml",
                    "<Ui><Frame name=\"LastFrame\"><Scripts>\
                     <OnEvent>if event == \"PLAYER_LOGOUT\" then LastDB = 'written at logout' end</OnEvent>\
                     </Scripts></Frame>\
                     <Script>LastFrame:RegisterEvent(\"PLAYER_LOGOUT\")</Script></Ui>",
                ),
            ],
        );
        let id = ("Realm".to_string(), "Char".to_string());
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        let _ = load_third_party(&mut script, Some(&id), &[], true);
        crate::ui_script::shutdown_ui_state(&mut script, Some(&id), true);

        let written = std::fs::read_to_string(home.join("saved/Last.lua")).unwrap();
        assert!(
            written.contains("LastDB = \"written at logout\""),
            "the value the PLAYER_LOGOUT handler set must reach the file — if the write ran \
             first this reads 'unset':\n{written}"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// **The logout write MERGES, and it keeps the file's own spelling** (decision 2139).
    ///
    /// The reference's writer `0x51ef20` emits the in-memory enable hash, and the reader
    /// `0x51ebe0` builds that hash from `AddOns.txt` — an entry per line, both states, each name
    /// strdup'd as written. The installed-folder list never enters it. So a row survives an
    /// uninstall, and the spelling that survives is the file's.
    ///
    /// Ours rebuilt the file from `addon_enable_states()` instead, and one logout erased
    /// `Uninstalled: disabled` and rewrote `myaddon:` as `MyAddon:`.
    #[test]
    fn the_logout_write_keeps_rows_it_did_not_put_there() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("logout-merge");
        write_addon(
            &home,
            "MyAddon",
            "## Interface: 11200\nmain.lua\n",
            &[("main.lua", "MyAddonRan = true")],
        );
        let id = ("Realm".to_string(), "Char".to_string());
        let p = crate::local_state::addons_state_path("Realm", "Char").unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "Uninstalled: disabled\nmyaddon: enabled\n").unwrap();

        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        let _ = load_third_party(&mut script, Some(&id), &[], true);
        script.eval::<()>("DisableAddOn('MyAddon')").unwrap();
        save_enable_state(&script, Some(&id));

        let after = std::fs::read_to_string(&p).unwrap();
        let rows = parse_enable_state(&after);
        // The uninstalled addon's row is still there, still disabled — the half with teeth.
        assert_eq!(
            rows.iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("Uninstalled"))
                .map(|(_, on)| *on),
            Some(false),
            "a row for an addon not installed this session must survive the write: {after}"
        );
        // The file's own spelling survives; the folder's case does not overwrite it.
        assert!(
            after.contains("myaddon:"),
            "the file's spelling is what the reference writes back: {after}"
        );
        // …and this session's toggle still landed on it.
        assert_eq!(
            rows.iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("MyAddon"))
                .map(|(_, on)| *on),
            Some(false),
            "the DisableAddOn must still be persisted: {after}"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// **A LoadOnDemand dependency cycle LOADS BOTH and answers `(1, nil)`** (decision 2139) —
    /// and before the fix this call took the whole process down with SIGABRT.
    ///
    /// `AddOn_Load 0x51f240` has no re-entrancy guard: every live site of the visiting byte
    /// `[UIADDON+0x2d]` is inside `AddOn_CanLoad 0x51e780`, none in the loader. What bounds the
    /// recursion is the *loaded* byte, stamped **before** the dependency loops —
    /// `0x51f313 mov byte [rec+0x18],1`, a flag-neutral store between `0x51f311 test eax,eax` and
    /// `0x51f317 jbe`, therefore unconditional — so the re-entered frame takes the already-loaded
    /// early-out at `0x51f2d6`/`0x51f2db` and returns 1. We stamped it after the recursion, so
    /// nothing terminated: Rust-level recursion, stack overflow, `signal 6`, uncatchable by
    /// `pcall`.
    ///
    /// The three assertions below are the reference's own observable consequences, and the last
    /// two are the ones that would betray a guard bolted on somewhere else instead:
    /// **no reason token** (a cycle produces none — the reference has no such token),
    /// `ADDON_LOADED` fires **B then A**, and `IsAddOnLoaded('Ping')` answers 1 throughout Pong's
    /// execution, because Ping is flagged loaded before its own files run.
    #[test]
    fn a_load_on_demand_dependency_cycle_loads_both_sides() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("lod-cycle");
        write_addon(
            &home,
            "Ping",
            "## Interface: 11200\n## LoadOnDemand: 1\n## Dependencies: Pong\nping.lua\n",
            &[(
                "ping.lua",
                "PingRan = true Order = (Order or '') .. 'Ping,'",
            )],
        );
        write_addon(
            &home,
            "Pong",
            "## Interface: 11200\n## LoadOnDemand: 1\n## Dependencies: Ping\npong.lua\n",
            &[(
                "pong.lua",
                "PongRan = true Order = (Order or '') .. 'Pong,' \
                 PingSeenLoaded = IsAddOnLoaded('Ping')",
            )],
        );
        let mut script = UiScript::new().unwrap();
        script.set_screen_size(1024.0, 768.0);
        script
            .eval::<()>(
                "AddonOrder = '' \
             CycleWatch = CreateFrame('Frame') \
             CycleWatch:RegisterEvent('ADDON_LOADED') \
             CycleWatch:SetScript('OnEvent', function() \
                 AddonOrder = AddonOrder .. arg1 .. ',' end)",
            )
            .unwrap();
        let _ = load_third_party(&mut script, None, &[], true);

        // Neither ran at startup — the walk skips LoadOnDemand, which is why a cycle among them
        // is never observed until something demand-loads one.
        assert_eq!(
            script
                .eval::<bool>("return PingRan == nil and PongRan == nil")
                .ok(),
            Some(true)
        );

        // The call that used to abort the process.
        assert_eq!(
            script
                .eval::<Vec<String>>(
                    "local loaded, reason = LoadAddOn('Ping') \
                     return { tostring(loaded), tostring(reason) }"
                )
                .ok(),
            Some(vec!["1".into(), "nil".into()]),
            "a cycle loads and answers (1, nil) — the reference has no cycle reason token"
        );

        // Both sides ran, each `.toc` list exactly once.
        assert_eq!(
            script.eval::<String>("return Order").ok().as_deref(),
            Some("Pong,Ping,"),
            "the dependency's files run first, and neither list runs twice"
        );
        // `ADDON_LOADED` fires B then A.
        assert_eq!(
            script.eval::<String>("return AddonOrder").ok().as_deref(),
            Some("Pong,Ping,")
        );
        // The stamp is ahead of the files, so the cycle inverts the declared order: Ping reads as
        // loaded for the whole of Pong's execution. This is the reference's behaviour and it is
        // the mechanism, not a side effect.
        assert_eq!(
            script.eval::<i64>("return PingSeenLoaded").ok(),
            Some(1),
            "inside a cycle an addon is flagged loaded before its own files run"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }
}
