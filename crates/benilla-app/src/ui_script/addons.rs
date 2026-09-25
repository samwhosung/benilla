//! Where interfaces come from: addon discovery under [`root`] and the load walk ([`Walk::load`]).
//! An [`Addon`] is a name, a parsed `.toc` and the [`Source`] its files come from. benilla's own
//! interface is one too, with the compiled-in tree as its source, but not for the lifecycle: like
//! FrameXML it loads outside `AddOn_Load` ([`super::manifest::load_ingame_ui`]) and gets no
//! `ADDON_LOADED`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};
use benilla_ui::toc::Toc;

use super::content;

/// The addon folder's name, the reference's own spelling.
const ADDONS_DIR: &str = "AddOns";

/// The VM instruction bound for one addon's load, shared with the addon harness: about 50x the
/// corpus's heaviest (4M), so an addon that crosses it is in a runaway loop and fails alone.
pub(crate) const LOAD_INSTRUCTION_BUDGET: u64 = 200_000_000;

/// Where one interface's files come from.
pub(super) enum Source {
    /// benilla's own interface: the compiled-in tree, shadowed by `assets/ui` in a dev build.
    Builtin,
    /// The AddOns root on disk, not this addon's own folder: a request under `Interface/AddOns/`
    /// maps onto it, so a dependent can reach a shared library addon beside it.
    Dir(PathBuf),
    /// The player's patch chain ([`super::reference_ui`]), by full chain path, `/` and `\` alike.
    Chain,
}

/// One loadable interface: a name, its parsed manifest, and where its files come from.
pub(super) struct Addon {
    /// The folder name (`"benilla"` for the builtin): what the AddOn API keys on and what
    /// `ADDON_LOADED` carries.
    pub(super) name: String,
    /// The parsed `.toc`: the ordered file list plus every directive.
    pub(super) toc: Toc,
    source: Source,
}

impl Addon {
    /// An interface from an explicit source, for [`super::reference_ui::addon`].
    pub(super) fn new(name: String, toc: Toc, source: Source) -> Self {
        Addon { name, toc, source }
    }

    /// benilla's own interface, which is in the binary and so cannot be missing.
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

    /// One file's bytes, by a path already resolved into the source's path space. A `Dir` reads
    /// under the AddOns root, then the chain ([`read_addon_file`]); only the flat builtin falls
    /// back to the basename, which for a `Dir` would rescue an escaping path. Bytes, not text: a
    /// `.lua` reaches Lua as it is on disk.
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

    /// The base this addon's manifest entries resolve against: `Interface/AddOns/<Folder>` for a
    /// `Dir`, empty for the builtin's flat tree and the chain's full paths.
    fn prefix(&self) -> String {
        match &self.source {
            Source::Builtin | Source::Chain => String::new(),
            Source::Dir(_) => format!("{ADDONS_PREFIX}{}", self.name),
        }
    }

    /// The chunk name, which addons parse: `"@%s"` (`0x8716e0`) over the resolved install path,
    /// built by `0x704bc0` for every `.lua`, as for `<Script file=>`. Ace2 libraries find their
    /// addon by splitting a `debugstack` frame on `\AddOns\` (`AceDB-2.0.lua:742`). The builtin's
    /// flat tree keeps [`benilla_ui::script::addon_chunk_name`].
    fn chunk_name(&self, file: &str, path: &str) -> String {
        match &self.source {
            Source::Builtin => benilla_ui::script::addon_chunk_name(&self.name, file),
            Source::Dir(_) | Source::Chain => format!("@{}", path.replace('/', "\\")),
        }
    }

    /// Load this addon's `.toc`-listed files, in listed order. Each error is logged as it happens
    /// and also returned, tagged `"<Addon>/<file>: <error>"`, for the tests to assert empty.
    fn load(&self, script: &UiScript) -> Vec<String> {
        self.load_files(script, &self.toc.files)
    }

    /// [`Addon::load`] over an explicit slice, for the builtin's two-phase boot. A manifest lists
    /// both kinds of file (the reference's `FrameXML.toc` opens with `GlobalStrings.lua`): a `.lua`
    /// runs as a chunk in the shared state, anything else is parsed as FrameXML and materialized.
    pub(super) fn load_files(&self, script: &UiScript, files: &[String]) -> Vec<String> {
        let mut failures = Vec::new();
        // The `<Include>` / `<Script file=>` provider; `read` is the sandbox.
        let provider = |req: &str| -> Option<Vec<u8>> { self.read(req) };
        for file in files {
            // Resolved once into the source's path space, for `read` and the loader alike.
            let path = benilla_ui::loader::join_ref(&self.prefix(), file);
            let Some(bytes) = self.read(&path) else {
                let e = format!("{}/{file}: not found", self.name);
                // Severity follows whose manifest is wrong. Ours (the builtin, or a `benilla.toc`
                // line the player's chain lacks) is an ERROR. A player's addon is the package's
                // fault, which the reference skips silently, and an ERROR line fails `smoke.sh`.
                match self.source {
                    Source::Builtin | Source::Chain => error!("ui_script: {e}"),
                    Source::Dir(_) => warn!("ui_script: {e}"),
                }
                // Retained for the player: the commonest way an addon fails with nothing on screen.
                script.report_load_failure(&e);
                failures.push(e);
                continue;
            };
            if is_lua(file) {
                // What `<Script file=>` gets: one chunk in the one global state, in manifest
                // order, as `AddOn_Load 0x51f240` hands every listed file to `0x6edb90`.
                match script.run_chunk_named(&bytes, &self.chunk_name(file, &path)) {
                    Ok(()) => info!("ui_script: {}/{file} ran", self.name),
                    Err(e) => {
                        let e = format!("{}/{file}: {e}", self.name);
                        error!("ui_script: {e}");
                        // A script error: it reaches the player through the Lua error handler.
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
                    // No dialog, as the reference only logs it (FrameXML.log); still retained.
                    script.report_load_failure(&e);
                    failures.push(e);
                    continue;
                }
            };
            // The loader resolves relative references against the document's own directory, and
            // names the file a raise came from.
            let report = benilla_ui::loader::load_in(script, &doc, &path, &provider);
            // `FrameXML_Debug` traces go to the log only: the reference files them at severity 0
            // (`0x6ee2bc`) in a per-document record whose surface is untraced.
            for t in &report.traces {
                info!("ui_script({}/{file}): {t}", self.name);
            }
            for w in &report.warnings {
                warn!("ui_script({}/{file}): {w}", self.name);
                // Retained for `/errors` too, here because only this caller knows the file.
                script.report_warning(&format!("{}/{file}: {w}", self.name));
            }
            // A file the document names and the provider lacks is a `.toc` line naming no file:
            // never a script error, as the reference logs `Couldn't open %s` and carries on, but a
            // `failures` entry, so our boot tests catch one in a `benilla.toc` document.
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
                // A script error, as in the Lua arm; `_ERRORMESSAGE`'s IsVisible guard shows only
                // a burst's first, as in the reference.
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

/// Whether a manifest entry is a Lua chunk: by extension, case-insensitively, with `\` a
/// separator like `/` (a `.toc` is written for a Windows client).
fn is_lua(entry: &str) -> bool {
    entry
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(entry)
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("lua"))
}

/// `root/rel`, refusing to escape `root`. `rel` is already resolved by
/// [`benilla_ui::loader::join_ref`], so an escape shows as a leading `..`, refused with anything
/// absolute before any filesystem call; a symlinked addon folder still works.
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

/// The one file namespace, install-relative, as in the reference: `0x647e60` answers a name from
/// the install tree's hash index, then the archive chain, after `0x6ede10` collapses `..`. So an
/// addon's `..\Blizzard_AuctionUI\…` reaches the archive behind a `.pub`-only folder. Only a path
/// under `Interface/AddOns/` touches the filesystem, and never above the AddOns root.
pub(crate) fn read_addon_file(root: &Path, req: &str) -> Option<Vec<u8>> {
    under_addons(req)
        .and_then(|rel| read_under(root, rel))
        .or_else(|| super::reference_ui::read(&req.replace('/', "\\")))
}

/// `req` with the `Interface/AddOns/` prefix stripped, or `None` without it; matched
/// case-insensitively, as the reference is a Windows client (`Interface\Addons\` is the same path).
fn under_addons(req: &str) -> Option<&str> {
    let rest = req.get(ADDONS_PREFIX.len()..)?;
    req.get(..ADDONS_PREFIX.len())
        .filter(|p| p.eq_ignore_ascii_case(ADDONS_PREFIX))
        .map(|_| rest)
}

/// A `Dir` addon's base before its folder name ([`Addon::prefix`]), `/`-separated as `join_ref` is.
const ADDONS_PREFIX: &str = "Interface/AddOns/";

/// The addon folder, `<benilla-config>/AddOns/`, or `None` (always under `$WOW_CAPTURE`); addon
/// textures and fonts resolve onto it too. Deviation: the reference reads the install's
/// `Interface\AddOns\`; benilla reads an install and does not live in one, so its addons live in
/// its own state folder, and only there.
pub(crate) fn root() -> Option<PathBuf> {
    root_from(crate::local_state::home())
}

/// [`root`] with its one environment fact passed in, so a test need not set the environment.
fn root_from(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(ADDONS_DIR))
}

/// The reference's twelve `Blizzard_*` LoadOnDemand addons: on a 1.12 install each folder holds
/// only a `.pub`, with the files in the archive. Probed against the chain.
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

/// The Blizzard addons the chain carries as LoadOnDemand registry rows, for `UIParentLoadAddOn`;
/// a toc that is not LoadOnDemand is skipped, as the startup walk does not read the chain.
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

/// Every registered addon in registration order, the archive pass before the loose folders and a
/// name's first registration winning: `AddOn_ScanAddOnDir 0x51c760` walks each archive's
/// `(listfile)` in line order (`0x401470`, `0x648fb0`) before the `FindFirstFileW` walk
/// (`0x42ad10`), and `0x51c9b0` returns on a name-hash hit (`0x51ca10`) and tail-inserts
/// (`0x521ad0`). That order is the Lua index space, and in the one shared Lua state (`0x7040d0`)
/// it decides whose copy of a global wins.
fn discover() -> Vec<Addon> {
    // The archive pass, in `BLIZZARD_ADDONS` order: the shipped `patch.MPQ` listfile's too.
    let mut found = chain_addons();
    // The loose pass, minus the names the archive registered.
    let seen: HashSet<String> = found.iter().map(|a| a.name.to_ascii_lowercase()).collect();
    found.extend(
        discover_folder()
            .into_iter()
            .filter(|a| !seen.contains(&a.name.to_ascii_lowercase())),
    );
    found
}

/// Sort folder names into NTFS directory order. The reference's loose pass (`0x42ad10`,
/// `FindFirstFileW`) sorts nothing, so its order is the host filesystem's, and the addon corpus was
/// written against NTFS: a directory's `$I30` index is ordered by `COLLATION_FILE_NAME`, UTF-16
/// units compared through `$UpCase`, a tie broken by the raw units. With every addon in one Lua
/// state (`0x7040d0`), this order decides which copy of a shared library wins. Sorted on every
/// host, Windows included, so a capture and the addon harness are deterministic.
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

/// `$UpCase`'s mapping for one character, 1:1 or none: the table maps each of the 65536 UTF-16
/// units to one unit, so a character whose uppercase is longer (`ß` to `SS`) stays as it is.
fn upcase_unit(c: char) -> char {
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(u), None) => u,
        _ => c,
    }
}

/// The player's own addons: every folder under the AddOns root with a `<Name>.toc`, in
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
            // A folder with no `<Name>.toc` is not an addon; the reference skips it silently too.
            let text = manifest_in(&dir, &name)?;
            Some(Addon {
                name,
                toc: Toc::parse(&text),
                // The root, not `dir`, so a sibling library addon is reachable.
                source: Source::Dir(root.clone()),
            })
        })
        .collect()
}

/// `<dir>/<name>.toc`, matched case-insensitively as on the reference's Windows filesystem
/// (`MyAddon/myaddon.toc` is an addon there). Decoded, not read as UTF-8: one cp1252 byte in a
/// `## Notes:` line must not make the addon vanish.
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

/// The AddOn API's registry row for one discovered addon.
fn info_for(addon: &Addon) -> benilla_ui::script::AddOnInfo {
    let mut info = info_from_toc(&addon.name, &addon.toc);
    info.chain = matches!(addon.source, Source::Chain);
    info
}

/// [`info_for`]'s body, shared with `addon_harness` so its survey seats the registry row
/// production does.
pub(crate) fn info_from_toc(name: &str, toc: &Toc) -> benilla_ui::script::AddOnInfo {
    benilla_ui::script::AddOnInfo {
        name: name.to_owned(),
        title: toc.directive("Title").map(str::to_owned),
        notes: toc.directive("Notes").map(str::to_owned),
        url: toc.directive("URL").map(str::to_owned),
        // `## Secure: 1` grants nothing; it only sets `GetAddOnInfo`'s security answer.
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
        // Visible to `GetAddOnInfo(index)` until `SMSG_ADDON_INFO` hides it: the server's verdict
        // on a `## Secure:` record, not a directive.
        hidden: false,
        enabled: true,       // until the enable state says otherwise
        saved_enabled: true, // re-stamped from `enabled` at registration
        loaded: false,
        // The version gate's dword: the leading integer, 0 when absent (out of date, not unknown).
        interface: toc.interface_version(),
        chain: false, // `info_for` stamps the chain rows; the harness's rows are folder addons
    }
}

/// Parse `AddOns.txt`, the reference's format: one `<AddOnName>: enabled|disabled` per line. Only
/// `disabled` disables, so a malformed line leaves the addon enabled.
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

/// One installed addon as the AddOns screens need it, read from the folder with no VM, as the glue
/// runs before any VM holds addons.
#[derive(Clone, Debug)]
pub(crate) struct InstalledAddOn {
    pub(crate) name: String,
    pub(crate) title: Option<String>,
    pub(crate) notes: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) dependencies: Vec<String>,
    /// `## Interface` as the version gate reads it: the leading integer, 0 when absent. The load
    /// walk enforces it when `checkAddonVersion` is on (`0x51e876`).
    pub(crate) interface: u32,
    /// `## LoadOnDemand: 1`: shown as a status, not a checkbox state, as such an addon is not off
    /// but waiting for `LoadAddOn`.
    pub(crate) load_on_demand: bool,
    /// `## DefaultState:`, which [`EnableStore::enabled_for`] falls back to; no enable bit here.
    pub(crate) default_state: bool,
}

impl InstalledAddOn {
    /// The list's display name: `## Title`, else the folder name (`AddonList_Update`).
    pub(crate) fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }
}

/// Every installed addon in the glue list's order, by `## Title` (else the folder name): glue
/// `0x46d460` resolves `GetAddOnInfo(i)` through `0x51df00`'s array, sorted by `0x51deb0` on
/// `AddOn_GetTitle 0x51df20` with `SStrCmpI`. The Blizzard addons the server hides are not here.
pub(crate) fn installed_rows() -> Vec<InstalledAddOn> {
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
    // `SStrCmpI` folds `'A'..'Z'` by `+0x20` (`0x64a4c0`, `_strnicmp 0x414310`): ASCII only.
    // Stable, where the reference's `qsort` leaves equal keys in any order.
    rows.sort_by_key(|a| a.display_title().to_ascii_lowercase());
    rows
}

// ── The enable store: the reference's `ADDONSTATELIST` ──

/// The reference's `ADDONSTATELIST` (`0xbe1bd0`): a node per character in `SMSG_CHAR_ENUM` order,
/// rebuilt whole (`0x51f0b0` clears it, then callback `0x472300` fills a node per record through
/// `AddOnList_LoadCharacter 0x51ebe0`), each holding that character's explicit `AddOns.txt` rows.
/// A character with no file is an empty node, whose enable bit is answered from the others.
#[derive(Default)]
pub(crate) struct EnableStore {
    /// `(character, explicit rows)` in character-list order; names lowercased, as every compare in
    /// the reference is `SStrCmpI`.
    nodes: Vec<(String, HashMap<String, bool>)>,
}

impl EnableStore {
    /// A node per character, each from its own `AddOns.txt`
    /// (`benilla-config/addons/<Realm>-<Char>.txt`); a character with no file still gets one.
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

    /// `0x51e470(addon, NULL, useDefault = 0)`, the explicit-only aggregate over the nodes with a
    /// row (`0x51e5df je 0x51e60a`): `Some(v)` when all say `v` (the reference's 2 or 0), `None`
    /// when they are mixed or there are none.
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

    /// The bit a character gets, `0x51e470(addon, character, useDefault = 1)` (0 or 2 as a bool):
    /// * an explicit row in their file wins;
    /// * a node with no row takes [`Self::aggregate`] (`0x51e5f0`'s self-recursion), then
    ///   `## DefaultState` where that is undecided;
    /// * no node at all, or no character, takes `## DefaultState`: the walk passes each
    ///   mismatching node (`0x51e55a jne 0x51e611`) and ends with `total == 0`. From our callers a
    ///   named character always has a node ([`store_nodes`]).
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

/// The store's nodes for a load: the realm's character list, plus the loading character when the
/// caller had no list to give (a test, a capture), so its own file is still read.
fn store_nodes(identity: Option<&(String, String)>, roster: &[String]) -> Vec<String> {
    let mut names = roster.to_vec();
    if let Some((_, character)) = identity {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(character)) {
            names.push(character.clone());
        }
    }
    names
}

/// Write a character's enable state, from the AddOns screen and at logout, merged into the file:
/// the reference's writer `0x51ef20` emits its enable hash a line per entry (`0x853968`), built by
/// `0x51ebe0` from the file, so an uninstalled addon's row survives and names keep their spelling.
pub(crate) fn write_enable_state(identity: Option<&(String, String)>, states: &[(String, bool)]) {
    let Some(path) = enable_state_path(identity) else {
        return; // no character picked, or no state folder
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

/// Write the enable state back at shutdown, the reference's last step (`0x490c88`, after the
/// saved-variables files), through the AddOns screen's merging writer.
pub(super) fn save_enable_state(script: &UiScript, identity: Option<&(String, String)>) {
    let states = script.addon_enable_states();
    if states.is_empty() {
        return; // nothing registered: a glue-only run or a capture
    }
    write_enable_state(identity, &states);
}

/// Write every loaded addon's declared saved variables at shutdown (`0x490c83`, after the flat
/// file, before `AddOns.txt`). No autosave and no dirty bit, as in the reference, whose write gate
/// is the record's loaded byte: an addon that never loaded is not written. An addon declaring
/// nothing is left alone, where the reference deletes its file.
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
                // It failed to load this session, so its globals are the addon's defaults, and
                // writing them would replace the player's file.
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

/// The per-addon file's header. Deviation: the reference writes none (its file opens with a blank
/// line); ours names the file, because players open this folder.
const SAVED_HEADER: &str = "\
-- benilla per-addon saved variables.
-- Written at logout/exit from the live globals; executed as a Lua chunk at addon load.
";

/// Load every discovered third-party addon at world entry (`UI_Init 0x48fbf0` → `0x51f600`),
/// returning every load error tagged by addon. `&mut` because each `ADDON_LOADED` fires
/// ([`UiScript::fire_event`]) before the next addon's files run, as in the reference.
pub(super) fn load_third_party(
    script: &mut UiScript,
    identity: Option<&(String, String)>,
    roster: &[String],
    version_check: bool,
) -> Vec<String> {
    let addons = discover();
    // Registered even when empty: `GetNumAddOns()` must answer 0, not a previous session's list.
    let mut infos: Vec<_> = addons.iter().map(info_for).collect();
    // Chain rows read their files off the player's patch chain.
    script.set_addon_chain_reader(Box::new(super::reference_ui::read));
    // Each enable bit through the reference's query: the character's explicit row, else what the
    // realm's other characters agree on, else the manifest's `## DefaultState`.
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
        // `## LoadOnDemand: 1` waits for `LoadAddOn`: `0x51f600` loads only records whose
        // LoadOnDemand byte is 0.
        if addon.toc.load_on_demand() {
            info!(
                "ui_script: {} is LoadOnDemand — not loaded (no LoadAddOn() yet)",
                addon.name
            );
            continue;
        }
        // Re-armed per addon, so one runaway cannot fail every addon after it; a dependency
        // chain loads under its dependent's arming, as in the harness's survey.
        script.set_instruction_budget(LOAD_INSTRUCTION_BUDGET);
        let _ = state.load(script, &addons, &addon.name);
        let spent = script.instructions_used();
        if spent > 1_000_000 {
            // A heavy load gets a line: this counter is what the budget was chosen from.
            info!(
                "ui_script: {} spent {spent} VM instructions loading",
                addon.name
            );
        }
    }
    state.failures
}

/// The recursive load's bookkeeping, for [`Walk::load`].
#[derive(Default)]
struct Walk {
    loaded: HashSet<String>,
    /// Addons whose load failed, so a later dependent gets the same answer without a rerun.
    failed: HashSet<String>,
    /// The current dependency chain, for cycle detection and for naming the cycle when it happens.
    loading: Vec<String>,
    failures: Vec<String>,
    /// Addons the player turned off, lowercased; empty means every addon is enabled.
    disabled: HashSet<String>,
    /// `checkAddonVersion`: when on, an addon whose `## Interface` is not exactly the client's is
    /// skipped like a disabled one, and a dependent gets `Err` (`DEP_INTERFACE_VERSION`).
    version_check: bool,
}

impl Walk {
    /// Load one addon by name; `Err` means it did not load, which a hard dependent propagates. The
    /// order is `AddOn_Load 0x51f240`'s: optional dependencies (failures ignored), required ones (a
    /// failure aborts only this addon), the `.toc` files in order (`0x51f3fa`), `Bindings.xml`
    /// (`0x51f400`), the account then per-character saved variables (`0x51f4b5`, `0x51f53b`), then
    /// `ADDON_LOADED` (event 429, `0x51f5ad`), whose handlers see the restored values. The
    /// reference then loads the addons whose `## LoadWith` names this one; this walk does not.
    fn load(&mut self, script: &mut UiScript, all: &[Addon], name: &str) -> Result<(), ()> {
        // Key on the addon's own spelling: a `.toc` may name a dependency in any case, and keying
        // on the caller's would load the same addon twice.
        let Some(addon) = all.iter().find(|a| a.name.eq_ignore_ascii_case(name)) else {
            return Err(()); // not installed: the caller decides whether that is fatal
        };
        let key = addon.name.as_str();
        if self.loaded.contains(key) {
            return Ok(());
        }
        if self.failed.contains(key) {
            return Err(());
        }
        // Disabled is the player's choice, not a failure: nothing is pushed to `failures`, and a
        // dependent still gets `Err` (the reference's `DEP_DISABLED`).
        if self.disabled.contains(&key.to_ascii_lowercase()) {
            info!("ui_script: {key} is disabled — not loaded");
            self.failed.insert(addon.name.clone());
            return Err(());
        }
        // The version gate, `AddOn_CanLoad` check 6: after the enable state, before the dependency
        // loop, an exact `==` (a missing `## Interface` is 0). Not a failure either: the AddOns
        // screens show it, and the Load out of date AddOns checkbox loads it anyway.
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

        // Optional dependencies first: a failure is ignored, a present one still loads first.
        for dep in addon.toc.optional_dependencies() {
            let _ = self.load(script, all, dep);
        }
        // Hard dependencies: a failure aborts this addon and nothing else.
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
            // Retained for the player, who can usually fix it by installing the dependency.
            script.report_load_failure(&e);
            self.failures.push(e);
            self.failed.insert(addon.name.clone());
            return Err(());
        }

        self.failures.extend(addon.load(script));
        // `Bindings.xml` (`0x51f400`), after the files whose functions a binding calls, through
        // the addon's own sandboxed reader; absent is normal.
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
        // The saved variables, account then per-character (`0x51f4b5`, `0x51f53b`): after the
        // files set their defaults, before `ADDON_LOADED`.
        script.load_addon_saved_variables(&addon.name);
        self.loaded.insert(addon.name.clone());
        script.mark_addon_loaded(&addon.name);
        // `arg1` is the addon's own folder name, whatever case a dependent used. Marked loaded
        // first: the reference sets `[rec+0x18]` before `0x51f5ad`, so a handler's
        // `IsAddOnLoaded` about itself is true.
        script.fire_event("ADDON_LOADED", vec![ScriptValue::Str(addon.name.clone())]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    /// NTFS `$I30` order, not byte order: case folds away and `_` (0x5F) sorts after `Z` (0x5A).
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
                // `!` (0x21) sorts ahead of every letter.
                "!OmniCC",
                // The digit `2` (0x32) before `G` (0x47).
                "Ace2",
                "AceGUI",
                // Case folded: `E` < `T` decides it, not `u` vs `B`.
                "Fubar_EmoteFu",
                "FuBar_TinyTipFu",
                "oRA2",
                "zBar",
                "Zorlen",
                "_LazyPig",
            ]
        );

        // Byte order disagrees, so this fails if the key becomes `str`'s `Ord`.
        let mut plain = names.clone();
        plain.sort();
        assert_ne!(plain, names, "NTFS collation is not byte order");
    }

    /// The `COLLATION_FILE_NAME` tie: names equal but for case compare the raw units, uppercase
    /// (0x41) before lowercase (0x61), so a case-sensitive host still sorts deterministically.
    #[test]
    fn names_equal_but_for_case_break_the_tie_uppercase_first() {
        let mut names: Vec<String> = ["fubar", "FuBar", "FUBAR"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        sort_by_directory_order(&mut names);
        assert_eq!(names, vec!["FUBAR", "FuBar", "fubar"]);
    }

    /// `$UpCase` maps one unit to one: `ß` stays, where `to_uppercase` gives `SS`.
    #[test]
    fn the_upcase_fold_is_one_to_one_like_the_table_it_models() {
        assert_eq!(upcase_unit('a'), 'A');
        assert_eq!(upcase_unit('_'), '_');
        assert_eq!(upcase_unit('ß'), 'ß');
        assert_eq!(upcase_unit('é'), 'É');
    }

    /// Glue's `0x51df00` array is `## Title`-sorted by `0x51deb0`, case-insensitively. Folder names
    /// and titles are different permutations, and `Middle` is the case control.
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
        // No `## Title`: sorts under its folder name.
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

    /// `AddOn_ScanAddOnDir 0x51c760` scans the archives (`0x51c777`) before the loose folders
    /// (`0x51c78f`), and `0x51c9b0` returns on a name already registered (`0x51ca10`).
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

        let first_loose = names.iter().position(|n| *n == "Loose").expect("Loose");
        assert_eq!(
            first_loose,
            chain.len(),
            "the loose folder follows all {} archive rows: {names:?}",
            chain.len()
        );

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

    /// Every chain row is LoadOnDemand and chain-sourced, the eight windows the interface opens
    /// through them are rows, and `benilla.toc` lists none, like the reference's `FrameXML.toc`.
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

    /// A sibling library addon is reachable (`Bagnon/src/main.xml` includes
    /// `..\..\BagBrother\core\core.xml`), the machine is not; paths go through `join_ref` first.
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

        assert_eq!(addon.prefix(), "Interface/AddOns/Probe");
        let src = "Interface/AddOns/Probe/src";

        assert_eq!(
            addon.read(&join_ref(src, "own.txt")).as_deref(),
            Some(&b"yes"[..])
        );
        assert_eq!(
            addon
                .read(&join_ref(src, "..\\..\\ProbeLib\\core\\lib.xml"))
                .as_deref(),
            Some(&b"sibling"[..]),
            "a shared library addon must be reachable from the addon that includes it"
        );

        // Above the AddOns root touches no file: the collapsed path leaves `Interface/AddOns/` and
        // can only ask the archive chain. `secret.txt` beside the root is unreachable.
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
        // `join_ref` re-roots a leading `/` at the base; with none it is a bare relative name, a
        // chain lookup that misses.
        assert_eq!(join_ref("", "/etc/hosts"), "etc/hosts");
        assert!(addon.read(&join_ref("", "/etc/hosts")).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A `.toc`-listed `Bindings.xml` is walked as FrameXML, each `<Binding>` an unknown frame
    /// type, which the reference logs and carries on from (`0x6ee356`).
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

    /// A `.toc` line resolves against the addon folder, `<Script file=X>` against
    /// `dirname(referrer)` (`0x6ee07b`), and both chunks are `"@%s"` of the path (`0x704bc0`).
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
        // Both chunks carry the full install path that `\AddOns\` is split out of.
        for g in ["LISTED", "VIAXML"] {
            let frame = script.eval::<String>(&format!("return {g}")).unwrap();
            assert!(
                frame.contains("Interface\\AddOns\\Probe\\"),
                "{g} is named after the install path, not the AddOns root: {frame}"
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

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

    /// End to end through `framexml::parse`, the loader and the VM: a `.toc`, an XML frame, an
    /// `<Include>`d file, a sibling library addon and Lua that calls a client-API global.
    #[test]
    fn a_third_party_addon_loads_from_a_folder_with_no_rust() {
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-e2e-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("ProbeAddon");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(tmp.join("ProbeLib/core")).unwrap();
        // Bagnon's shape: the manifest names one file in a subfolder with a backslash path, which
        // reaches a sibling file by bare name and a shared library addon by `..\..`.
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
        // The library includes a file of its own by bare name, so the base follows the include.
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
            "a bare-name <Include> resolves against the including file's directory (src/), not \
             the addon root, which is where Bagnon's `templates.xml` is found"
        );
        assert_eq!(
            script.eval::<bool>("return ProbeLibLoaded == true").ok(),
            Some(true),
            "`..\\..\\ProbeLib\\core\\lib.xml` reaches a sibling addon, the shared-library \
             pattern a per-addon sandbox would block"
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

    #[test]
    fn a_dependency_named_in_another_case_loads_once() {
        let tmp = std::env::temp_dir().join(format!(
            "benilla-addon-case-key-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // `Bump.xml` counts its own runs, so a second load is observable.
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

    /// `local_state::home()` is `None` under `$WOW_CAPTURE`, so `None` here is the capture case.
    #[test]
    fn there_is_one_addon_root_and_it_is_ours() {
        assert_eq!(
            root_from(Some(PathBuf::from("/state"))),
            Some(PathBuf::from("/state/AddOns"))
        );
        assert_eq!(root_from(None), None, "a capture run has no addon root");
    }

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

    /// A folder with no `.toc` is not an addon; one whose `.toc` differs only in case is.
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

    // ───────────────────────────── lifecycle events ─────────────────────────────

    /// A frame that appends `"<event>:<arg1>"` to `EventLog` for each event it registers, loaded as
    /// an addon file so the events travel a real addon's `<OnEvent>` path.
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

    fn event_log(script: &UiScript) -> Vec<String> {
        script
            .eval::<Vec<String>>("return EventLog")
            .expect("EventLog")
    }

    /// Addons guard their handler with `if arg1 == "MyAddon" then`, so a wrong `arg1` looks like
    /// the event never arriving.
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
        // File-scope Lua runs before the event; its marker proves the order.
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

    /// FrameXML gets no `ADDON_LOADED` in the reference, and our interface loads like it, through
    /// [`super::manifest`] and never the walk; this loads both ways and reads the log.
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
        // The builtin, loaded as production loads it: its own manifest, not the walk.
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

    /// `UI_Init 0x48fbf0` loads the addons at `0x4900a3` (each `ADDON_LOADED` at `0x51f5ad`) and
    /// fires `VARIABLES_LOADED` at `0x4900b2`; `PLAYER_LOGIN` comes after, on a fresh login from
    /// the player's create (`0x5deb60` → `0x4908c0`), on a `/reload` from `0x490168`.
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

        // Hermetic: the state folder is the tempdir, so discovery finds only this addon.
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

    // ────────────────────────── the AddOn API and enable state ──────────────────────────

    /// An AddOns root under a temp `benilla-config`, `BENILLA_HOME` on it while the guard lives.
    fn hermetic_root(tag: &str) -> (PathBuf, crate::local_state::test_env::EnvGuard) {
        // The pid keeps two test binaries running at once from wiping each other's tree.
        let tmp =
            std::env::temp_dir().join(format!("benilla-addon-api-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("benilla-config");
        std::fs::create_dir_all(home.join("AddOns")).unwrap();
        let guard =
            crate::local_state::test_env::EnvGuard::set("BENILLA_HOME", home.to_str().unwrap());
        (home, guard)
    }

    /// `AddOn_CanLoad` check 6: exact `==`, a missing `## Interface` is 0 and out of date, and a
    /// gated addon's dependent is blocked; `checkAddonVersion` off loads the same folder in full.
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
            "main.lua\n", // no ## Interface: parses as 0, out of date
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

        // Force-load: the same folder, with the checkbox's other state.
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

    /// [`write_addon`] with raw bytes, for files that are not UTF-8.
    fn write_addon_bytes(home: &Path, name: &str, toc: &[u8], files: &[(&str, &[u8])]) {
        let dir = home.join("AddOns").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.toc")), toc).unwrap();
        for (f, body) in files {
            std::fs::write(dir.join(f), body).unwrap();
        }
    }

    /// A cp1252 `.toc` is still a manifest, a BOM'd `.lua` runs, and a cp1252 `.lua` is found. Lua
    /// 5.0 strings are bytes and the reference hands `luaL_loadbuffer` the file unmodified, so a
    /// cp1252 literal keeps its byte length.
    #[test]
    fn an_addon_whose_files_are_not_utf8_still_loads() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (home, _guard) = hermetic_root("encoding");
        // `## Title:` holds a cp1252 a-umlaut (0xE4).
        let mut toc = b"## Interface: 11200\n## Title: Sch\xE4tze\nlocale.lua\nboot.lua\n".to_vec();
        toc.splice(0..0, [0xEFu8, 0xBB, 0xBF]); // and a BOM on the manifest too
        write_addon_bytes(
            &home,
            "Umlaut",
            &toc,
            &[
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

        // Discovery found it. One, not thirteen: `SMSG_ADDON_INFO` answers status 2 for the
        // chain's twelve secure addons, and the Lua index space drops them.
        seat_stock_addon_reply(&mut script);
        assert_eq!(script.eval::<i64>("return GetNumAddOns()").ok(), Some(1));
        assert_eq!(
            script
                .eval::<String>("return GetAddOnMetadata('Umlaut', 'Title')")
                .ok(),
            Some("Sch\u{e4}tze".to_string()),
            "the manifest's cp1252 title decoded to the glyph its author typed, not to nothing"
        );
        assert_eq!(
            script.eval::<bool>("return UmlautLoaded == true").ok(),
            Some(true),
            "a BOM'd .lua ran — the three-byte mark was stripped, not handed to the lexer"
        );
        // The cp1252 literal is still bytes, as in the reference.
        assert_eq!(
            script.eval::<i64>("return UmlautLen").ok(),
            Some(7),
            "`Sch\\xE4tze` is 7 bytes in the file and must be 7 bytes in Lua — transcoding the \
             chunk to UTF-8 would make it 8 and silently move every string.sub in the addon"
        );
    }

    /// A row for an addon not installed now survives the AddOns screen's write.
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

    /// Seat the `SMSG_ADDON_INFO` reply hiding the secure addons, as world entry does: the Lua
    /// index space does not exist until the server answers, as in the reference.
    fn seat_stock_addon_reply(script: &mut UiScript) {
        let hidden: Vec<String> = benilla_protocol::messages::STOCK_SECURE_ADDONS
            .iter()
            .map(|a| a.name.to_string())
            .collect();
        script.note_addon_info_reply(&hidden);
    }

    /// The in-game `GetAddOnInfo` (`0x48e390`) answers seven values, `name, title, notes, enabled,
    /// loadable, reason, security`; glue's (`0x46d460`) answers eight, with `url` at slot 4.
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

        // One, not thirteen: the chain's twelve secure addons are hidden by the reply.
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
                "1".into(),   // enabled: slot 4 in-game, where glue's binding has url
                "1".into(),   // loadable
                "nil".into(), // no reason: it loads
                "INSECURE".into(),
                // No eighth value in-game (glue's `newVersion`).
                "nil".into(),
            ])
        );
        // AceLibrary's guard, `local name, _, _, enabled, loadable = GetAddOnInfo(major)`: with
        // `url` in slot 4, `enabled` would be nil and Ace would silently skip its dependency.
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

    /// An addon the enable file does not mention loads, so a dropped-in folder just works.
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
        // "On" is absent from the file.
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

    /// `LoadAddOn` is synchronous: `UIParentLoadAddOn` uses the addon's frames on the next line.
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

        // Discovered and described, not run; one row, the chain's twelve hidden by the reply.
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

    /// A LoadOnDemand options package on a dependency the startup walk loaded (`/msbt`'s shape):
    /// without the walk's `mark_addon_loaded` stamp the gate answers `DEP_NOT_DEMAND_LOADED`.
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

        assert_eq!(
            script.eval::<bool>("return IsAddOnLoaded('Base') == 1").ok(),
            Some(true),
            "a startup-loaded addon reads as loaded, else a dependent answers DEP_NOT_DEMAND_LOADED"
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

    /// A demand load treats a `.toc` line naming a missing file as the startup walk does: the
    /// reference logs `Couldn't open %s` and carries on (`0x6edaa0`).
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
            // `Locale-koKR.lua` is listed and not written.
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
        // A warning, never a script error: the addon harness and `smoke.sh` both count errors.
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
        assert!(
            script
                .diagnostics()
                .iter()
                .any(|d| d.message.contains("Holey/Locale-koKR.lua")),
            "the miss is retained in the diagnostic log"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// The reason tokens are the reference's own: `UIParent.lua:642` looks up
    /// `getglobal("ADDON_"..reason)` for the label, so an invented token renders as nil.
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
            // Loaded at startup, as it is not LoadOnDemand: the success case.
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

    /// `AddOn_CanLoad` check 3 is `0x51e470(name, character, useDefault = 1)`. `Contested` is the
    /// control: the roster disagrees about it, so the newcomer gets its `## DefaultState`.
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
        // Two characters with files: both turned `Shunned` off, they disagree about `Contested`.
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

    /// A name on no node passes every node (`0x51e55a jne 0x51e611`) and takes the
    /// `DefaultState ? 2 : 0` epilogue; `Known`, with a node and no row, is the control.
    #[test]
    fn an_unknown_character_takes_the_manifest_default_not_the_aggregate() {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _c = crate::local_state::test_env::EnvGuard::unset("WOW_CAPTURE");
        let (home, _h) = hermetic_root("unknown-char");
        // `OptIn` ships `DefaultState: disabled`, so the epilogue's two outcomes differ.
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

        // `Stranger` is on no node: each addon answers its own manifest default.
        assert!(store.enabled_for("Shunned", true, Some("Stranger")));
        assert!(!store.enabled_for("OptIn", false, Some("Stranger")));
        // `None` (nobody picked yet) is the same epilogue.
        assert!(store.enabled_for("Shunned", true, None));
        assert!(!store.enabled_for("OptIn", false, None));
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

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
        // Registry order: the chain's Blizzard rows, then the folder's two (`0x51c777` before
        // `0x51c78f`, tail-inserted), one line per registry row.
        let mut expected = String::new();
        for a in chain_addons() {
            expected.push_str(&format!("{}: enabled\n", a.name));
        }
        expected.push_str("Drop: disabled\nKeep: enabled\n");
        assert_eq!(
            written, expected,
            "the reference's own one-line-per-addon format"
        );

        let mut next = UiScript::new().unwrap();
        let _ = load_third_party(&mut next, Some(&id), &[], true);
        assert_eq!(
            next.eval::<bool>("return DropRan == nil").ok(),
            Some(true),
            "the disable survived the session"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    // ───────────────────────────── saved variables ─────────────────────────────

    /// The account file, then the per-character one (`0x51f4b5`, `0x51f53b`), then `ADDON_LOADED`
    /// (`0x51f5ad`): the file-scope default is set, overwritten, then read by the handler.
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

    /// The reference writer's grammar (`0x7043f0`/`0x704480`): `NAME = value`, bracketed keys, TAB
    /// indent, a trailing comma on every entry, a bracketed key for a list entry too (`[1] = 1,`),
    /// and the file split by scope. Asserted as bytes, not by reloading.
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
        assert!(!account.contains("GramChar"), "account file: {account}");
        let per_char = std::fs::read_to_string(home.join("saved/Realm-Char/Gram.lua")).unwrap();
        assert!(
            per_char.ends_with("GramChar = 5\n"),
            "per-char file: {per_char}"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// `PLAYER_LOGOUT` fires before the writes (`0x490c2a` before `0x490c7e`/`0x490c83`), so a
    /// value its handler sets reaches the file.
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

    /// The writer `0x51ef20` emits the enable hash the reader `0x51ebe0` built from the file, an
    /// entry per line as written: a row survives an uninstall, and a name keeps its spelling.
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
        assert_eq!(
            rows.iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("Uninstalled"))
                .map(|(_, on)| *on),
            Some(false),
            "a row for an addon not installed this session must survive the write: {after}"
        );
        assert!(
            after.contains("myaddon:"),
            "the file's spelling is what the reference writes back: {after}"
        );
        assert_eq!(
            rows.iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("MyAddon"))
                .map(|(_, on)| *on),
            Some(false),
            "the DisableAddOn must still be persisted: {after}"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    /// `AddOn_Load 0x51f240` has no re-entrancy guard (the visiting byte `[UIADDON+0x2d]` lives in
    /// `AddOn_CanLoad 0x51e780`) but stamps the loaded byte before the dependency loops
    /// (`0x51f313 mov byte [rec+0x18],1`), so the re-entered frame exits at `0x51f2d6`/`0x51f2db`
    /// with 1. A cycle loads both with no reason token, `ADDON_LOADED` fires B then A, and
    /// `IsAddOnLoaded('Ping')` is 1 while Pong's files run.
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

        // Neither ran at startup: the walk skips LoadOnDemand.
        assert_eq!(
            script
                .eval::<bool>("return PingRan == nil and PongRan == nil")
                .ok(),
            Some(true)
        );

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
        assert_eq!(
            script.eval::<i64>("return PingSeenLoaded").ok(),
            Some(1),
            "inside a cycle an addon is flagged loaded before its own files run"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }
}
