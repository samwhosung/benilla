//! **The reference FrameXML this client EXECUTES off the player's own patch chain**, rather than
//! shipping a copy of it — the mechanism half of.
//!
//! ## The rule
//!
//! The end state for the in-game interface is the stock 1.12 FrameXML, run off the file the player
//! already owns. `assets/ui` is scaffolding: it retires file by file, and a migrated window means
//! *its stock XML + Lua run off the chain and our counterpart file is deleted* (1751 §2). Fidelity
//! by construction — the reference's text cannot drift from itself, and every frame name, id,
//! template and stratum an addon reaches for is right because it **is** the reference's.
//!
//! What stays ours permanently: the glue screens (GlueXML is a separate engine surface
//! even in the real client), dev-only frames, and adapter shims only while a genuine engine
//! difference forces one.
//!
//! ## Where the list lives — the manifest, not a second list in Rust
//!
//! Until 1751 this module carried its own `SOURCED` array and ran it *before* `assets/ui`, which
//! was the only ordering a Lua-only mechanism could express. Sourcing **XML** needs a real
//! position in the load order instead: stock `ContainerFrame.xml` inherits `ItemButtonTemplate`,
//! `CooldownFrameTemplate`, `SmallMoneyFrameTemplate` and `UIPanelCloseButton`, so it has to load
//! *after* the files that declare them.
//!
//! So there is exactly one ordered list of what loads and when, and it is `assets/ui/benilla.toc`
//! — the manifest that already had that job. **A manifest entry carrying a path separator is
//! sourced off the chain; a bare filename is a file we ship** ([`is_chain_entry`]). Our tree is
//! flat, so the two can never be confused, and the migration reads as what it is: the line
//! `BagFrame.xml` becomes `Interface\FrameXML\ContainerFrame.xml`, and `BagFrame.xml` is deleted.
//!
//! Everything else — the XML parse, `<Include>` / `<Script file=>` resolution against the
//! document's own directory, chunk naming, the error reporting that reaches the player — is
//! [`super::addons::Addon`]'s, unchanged. This module is only a third [`super::addons::Source`]:
//! *the player's install*.
//!
//! ## Where a reference file and our own UI still collide, and which one wins
//!
//! **Order decides, and the manifest is the order.** A name defined by both goes to whichever line
//! is later. That is the whole rule; there is no precedence machinery. A file sourced *before* our
//! own has its collisions overwritten by ours; a file sourced at the position of the window it
//! replaces owns its names outright, which is what migrating a window means.
//!
//! The load-bearing example was `PaperDollFrame.lua`, sourced far above everything for one
//! frame-agnostic button family while our own `CharacterFrame.xml` won the eighteen names it
//! collided on. Decision 1751 migrated that window, so the file arrives at its own position now
//! and there is nothing left for it to collide with — which is what a finished migration looks
//! like.
//!
//! **Nothing is stubbed silently.** A reference body that reaches for something this client does
//! not have raises, naming it — which is loud, correct, and strictly better than a no-op that
//! pretends (1203, 1205, 1211, 1230). The answer is to build the verb, or to adapt the body in one
//! of our own files and say why at the site.
//!
//! ## No install, no file
//!
//! A machine with no client data (CI, a bare checkout) simply does not get these files, and says so
//! once, loudly. It is the same condition under which `GlobalStrings` is absent, and the addon
//! survey already prints which mode it ran in for that reason. An install-less checkout cannot run
//! most meaningful UI tests anyway — the art, fonts and MPQs come from the install too — so tests
//! that need these files gate on the install like every other client-data test.

use std::sync::OnceLock;

use benilla_formats::Chain;
use benilla_ui::toc::Toc;
use bevy::prelude::*;

use super::addons::{Addon, Source};

/// The addon name the reference's own files load under — the reference's word for its interface.
///
/// It is not `Interface\AddOns\…` anything: FrameXML is not an addon, gets no `ADDON_LOADED`, and
/// an addon that derives its folder from a `debugstack` pattern (`"\\AddOns\\(.*)\\"` —
/// `benilla_ui::script::addon_chunk_name`'s reason for existing) must not match a FrameXML frame.
/// [`Addon::chunk_name`] is what keeps that true: a chain file's chunk is named after its own
/// chain path, which is exactly what the real client names it.
pub(super) const NAME: &str = "FrameXML";

/// Is this manifest entry **sourced off the player's chain**, rather than shipped by us?
///
/// The test is a path separator, and it is decidable because our own shipped tree is *flat*: every
/// `assets/ui` entry is a bare filename, and every chain entry is a full internal path
/// (`Interface\FrameXML\ContainerFrame.xml`). `manifest::tests` pins both halves so the day
/// somebody adds a subdirectory to `assets/ui` is a failing test rather than a file that silently
/// stops loading.
pub(super) fn is_chain_entry(entry: &str) -> bool {
    entry.contains('\\') || entry.contains('/')
}

/// The reference interface as an [`Addon`] whose files come off the chain — the peer of
/// [`Addon::builtin`], and the thing [`super::manifest`] hands a manifest's chain entries to.
///
/// `files` are full chain paths, so the addon's prefix is empty and each entry is already in its
/// source's path space.
pub(super) fn addon(files: Vec<String>) -> Addon {
    Addon::new(
        NAME.to_string(),
        Toc {
            directives: Vec::new(),
            files,
        },
        Source::Chain,
    )
}

/// One file's bytes, read off the player's installed patch chain by internal path.
///
/// **Bytes, not text** (1193): a `.lua` chunk goes to Lua as it sits in the archive, and only an
/// XML parse decodes — a `read_to_string` here would not make a cp1252 file lose a glyph, it would
/// make the file *not exist*.
pub(super) fn read(req: &str) -> Option<Vec<u8>> {
    let chain = chain()?;
    match chain.read(req) {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            debug!("ui_script: {req} is not in the patch chain: {e:#}");
            None
        }
    }
}

/// The player's patch chain, opened once per process and cached.
///
/// Cached because the addon survey stands up 218 VMs and this would otherwise be per-VM work. A
/// process-local chain rather than the one [`benilla_assets`] holds: the interface loads from
/// places that have no Bevy world to ask (the tests, the addon harness, a bare `UiScript`), and
/// `Chain`'s reads are `&self` and lock-free, so a second handle costs the mount and nothing else.
fn chain() -> Option<&'static Chain> {
    static CHAIN: OnceLock<Option<Chain>> = OnceLock::new();
    CHAIN
        .get_or_init(|| {
            let Some(data) = benilla_formats::wow_data() else {
                warn!(
                    "ui_script: no client data — every interface file this client SOURCES off the \
                     player's install (benilla.toc's `Interface\\…` entries) is absent, so the \
                     windows they build do not exist and addons that call their globals will raise"
                );
                return None;
            };
            match benilla_formats::open_chain(&data) {
                Ok(chain) => Some(chain),
                Err(e) => {
                    error!("ui_script: opening the patch chain to source the reference UI: {e:#}");
                    None
                }
            }
        })
        .as_ref()
}

#[cfg(test)]
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use benilla_ui::script::UiScript;

    /// **Every saved UI global we declare defaults to what the reference's own file declares**
    /// — the FrameXML half of the "a default is the reference's" standard that
    /// `cvars::REGISTERED`'s [`crate::cvars::Reference`] column holds for the CVar half.
    ///
    /// Two stores carry a player's settings in this client and the standard has to cover both.
    /// CVars are the engine's, and their table now states the reference's value per row. The other
    /// store is FrameXML's own: the `RegisterForSave`'d globals — *Instant Quest Text*, *Show Buff
    /// Durations*, *Lock Action Bar* — whose default is a plain assignment in whichever of our
    /// `assets/ui` files still owns that window. Nothing tied those to anything, and two had
    /// drifted: `QUEST_FADING_DISABLE` and `SHOW_BUFF_DURATIONS` both shipped `"1"` where 1.12
    /// ships `"0"`, each a reasonable call on its own day (2026-07-17 and 0255) and neither
    /// visible as a *divergence from the reference* without opening its file.
    ///
    /// The reference's declarations are read off the player's own chain rather than copied here,
    /// so this cannot rot the way a transcribed list would: `UIOptionsFrame.lua`'s
    /// `UIOptionsFrame_Init` assigns every options-panel uvar its factory value, and that file is
    /// the authority. A name we persist that the reference declares **somewhere else**
    /// (`SHOW_OFFLINE_GUILD_MEMBERS` lives in `FriendsFrame.lua`) or not at all (our own
    /// `TRAINER_FILTER_*`) is reported as uncovered, not failed — and the covered count is
    /// asserted, so the day our last options window migrates and this covers nothing, it says so
    /// instead of passing vacuously.
    ///
    /// **This test retires as `assets/ui` does** (1751): a migrated window runs the reference's
    /// own file, whose assignment IS the reference's value, and the question stops existing.
    ///
    /// Skips without client data, like every other test that reads the install.
    #[test]
    fn our_saved_ui_globals_default_to_the_references_own_values() {
        let _data = benilla_formats::wow_data_or_skip!();

        // The reference's own factory assignments, parsed out of the file that makes them. Only
        // `NAME = "literal"` / `NAME = number` at the head of a line — an assignment guarded by an
        // `if`, or one that copies another global, is not a factory default and must not be read
        // as one.
        let src = String::from_utf8_lossy(
            &super::read("Interface\\FrameXML\\UIOptionsFrame.lua")
                .expect("the reference's own UIOptionsFrame.lua"),
        )
        .into_owned();
        let mut theirs = std::collections::BTreeMap::new();
        for line in src.lines() {
            let line = line.trim();
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() || !name.chars().all(|c| super::is_word(c) && !c.is_lowercase()) {
                continue; // a uvar is SHOUTED; anything else is a local, a field or a comparison
            }
            let value = value.trim().trim_end_matches(';').trim();
            let literal = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'));
            let Some(v) = literal.or_else(|| value.parse::<i64>().ok().map(|_| value)) else {
                continue; // not a literal — an expression, so not a factory default
            };
            theirs.insert(name.to_string(), v.to_string());
        }
        assert!(
            theirs.len() > 20,
            "parsed only {} declarations out of UIOptionsFrame.lua — the parse is broken, not the \
             reference",
            theirs.len()
        );

        let mut s = UiScript::new().expect("VM");
        s.set_screen_size(1024.0, 768.0);
        seat_a_player(&mut s);
        let failures = super::super::manifest::load_default_ui(&s);
        assert!(failures.is_empty(), "the default UI: {failures:#?}");

        let (mut checked, mut uncovered, mut wrong) = (0usize, Vec::new(), Vec::new());
        for name in s.saved_variable_names() {
            let Some(want) = theirs.get(&name) else {
                uncovered.push(name);
                continue;
            };
            // Through `tostring`, because the reference is inconsistent about it itself:
            // `AUTO_QUEST_WATCH` is declared as the number 1 and every other uvar as a string.
            let got = s
                .eval::<String>(&format!("return tostring({name})"))
                .unwrap_or_else(|e| panic!("{name} is registered for save but unreadable: {e}"));
            checked += 1;
            if &got != want {
                wrong.push(format!("{name}: ours {got:?}, the reference's {want:?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "saved UI globals that do not default to the reference's own value — either match it, \
             or make the divergence explicit at the assignment the way `cvars::REGISTERED` does:\n  \
             {}",
            wrong.join("\n  "),
        );
        assert!(
            checked >= 9,
            "only {checked} of our saved globals are declared in the reference's \
             UIOptionsFrame.lua (uncovered: {uncovered:?}) — if a window migrated, lower this; if \
             the parse broke, fix it",
        );
    }

    /// Strip what is not code, before any call census over a FrameXML file.
    ///
    /// Both `Name(` and `:Name(` counted calls inside comments until decision 1800: the round that
    /// built `PickupMerchantItem` also asked about `ShowInventorySellCursor`, which
    /// [`chain_gap_report`] had named as `PaperDollFrame.xml`'s last engine gap — and the answer
    /// was that stock `PaperDollFrame.lua:754-756` has the call **commented out**, all three
    /// lines. A real binding, never called, blocking a window that was not blocked.
    ///
    /// Line-based and deliberately simple: Lua `--` to end of line (but not `--[[`, which opens a
    /// block), Lua `--[[ … ]]` blocks, and XML `<!-- … -->` blocks. It does not track string
    /// literals, so a `"--"` inside a string truncates that line — which can only ever cause an
    /// UNDER-report, the safe direction for every reader of it.
    /// Blank the contents of every Lua string literal (`"…"`, `'…'`, `[[…]]`), keeping the quotes,
    /// so a pattern like `"/([^%s]+)%s(.*)"` cannot read as a call to `s`.
    fn strip_strings(text: &str) -> String {
        let b: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c == '"' || c == '\'' {
                out.push(c);
                i += 1;
                while i < b.len() && b[i] != c && b[i] != '\n' {
                    if b[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i < b.len() && b[i] == c {
                    out.push(c);
                    i += 1;
                }
                continue;
            }
            if c == '[' && i + 1 < b.len() && b[i + 1] == '[' {
                out.push_str("[[");
                i += 2;
                while i + 1 < b.len() && !(b[i] == ']' && b[i + 1] == ']') {
                    i += 1;
                }
                if i + 1 < b.len() {
                    out.push_str("]]");
                    i += 2;
                }
                continue;
            }
            out.push(c);
            i += 1;
        }
        out
    }

    fn strip_comments(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut in_xml = false;
        let mut in_lua_block = false;
        for line in text.lines() {
            let mut rest = line;
            let mut kept = String::new();
            loop {
                if in_xml {
                    match rest.find("-->") {
                        Some(i) => {
                            in_xml = false;
                            rest = &rest[i + 3..];
                        }
                        None => break,
                    }
                } else if in_lua_block {
                    match rest.find("]]") {
                        Some(i) => {
                            in_lua_block = false;
                            rest = &rest[i + 2..];
                        }
                        None => break,
                    }
                } else {
                    let xml = rest.find("<!--");
                    let lua = rest.find("--");
                    match (xml, lua) {
                        (Some(x), Some(l)) if x <= l => {
                            kept.push_str(&rest[..x]);
                            in_xml = true;
                            rest = &rest[x + 4..];
                        }
                        (_, Some(l)) => {
                            kept.push_str(&rest[..l]);
                            if rest[l..].starts_with("--[[") {
                                in_lua_block = true;
                                rest = &rest[l + 4..];
                            } else {
                                // A plain `--` comment runs to end of line.
                                rest = "";
                                break;
                            }
                        }
                        (Some(x), None) => {
                            kept.push_str(&rest[..x]);
                            in_xml = true;
                            rest = &rest[x + 4..];
                        }
                        (None, None) => break,
                    }
                }
            }
            if !in_xml && !in_lua_block {
                kept.push_str(rest);
            }
            out.push_str(&kept);
            out.push('\n');
        }
        out
    }

    /// **A chain entry really loads off the player's install, and order really decides.**
    ///
    /// The two halves of this module's rule, asserted rather than described, because nothing else
    /// would notice if either flipped: a chain entry that silently resolved to nothing would leave
    /// its globals nil (the failure mode is a window that does not exist, not an error), and the
    /// collision direction is invisible until an addon calls the wrong body.
    ///
    /// Skips without client data, like every other test that reads the install.
    #[test]
    fn a_chain_entry_loads_and_the_later_line_owns_the_collision() {
        let _data = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new().expect("VM");
        s.set_screen_size(1024.0, 768.0);

        seat_a_player(&mut s);
        let failures = super::super::manifest::load_default_ui(&s);
        assert!(failures.is_empty(), "the default UI: {failures:#?}");

        // The item-button family the `ContainerFrame.lua` line is there for — nothing but the
        // sourced file defines these.
        for name in [
            "ContainerFrameItemButton_OnEnter",
            "ContainerFrameItemButton_OnClick",
            "ContainerFrameItemButton_OnLoad",
            "ContainerFrameItemButton_OnUpdate",
            "KeyRingItemButton_OnClick",
        ] {
            assert!(
                s.eval::<bool>(&format!("return type({name}) == \"function\""))
                    .unwrap(),
                "{name} must come from the sourced reference file"
            );
        }
        // …and its constants, which the corpus reads directly.
        assert_eq!(s.eval::<i64>("return NUM_BAG_FRAMES").unwrap(), 4);
        assert_eq!(s.eval::<i64>("return NUM_CONTAINER_FRAMES").unwrap(), 12);
        // `PaperDollFrame.lua` used to be a manifest line of its own, sourced far above everything
        // for exactly this family. It arrives through stock `PaperDollFrame.xml`'s own
        // `<Script file=>` now, at the character window's position — so this
        // assertion also proves a chain `.xml` really brings its `.lua`.
        assert!(s
            .eval::<bool>("return type(PaperDollItemSlotButton_OnLoad) == \"function\"")
            .unwrap());

        // **The 18-name overlap this test used to assert the winner of is GONE.** Our
        // `CharacterFrame.xml` redefined 18 of `PaperDollFrame.lua`'s 29 functions and won them all
        // by loading later; that file is deleted and the reference's own bodies are the only ones.
        // The check that replaces it is the swap's, not the collision's: the live bodies are the
        // reference's, and ours are not merely shadowed but absent.
        assert!(
            s.eval::<bool>(
                "return type(PaperDollFrame_SetLevel) == \"function\" \
                 and type(CHARACTERFRAME_SUBFRAMES) == \"table\" \
                 and table.getn(CHARACTERFRAME_SUBFRAMES) == 5 \
                 and BenillaPaperDollSlot_OnLoad == nil"
            )
            .unwrap(),
            "the character sheet's bodies must be the reference's own now"
        );

        // Order still decides, and it is still the whole rule — so it is asserted directly rather
        // than through whichever window happens to collide this month. Two chunks, the same name,
        // and the later one stands; `publish_global`'s non-overwriting rule (`0x701bd0`) applies to
        // FRAMES, never to a plain Lua global, and confusing the two has produced confident wrong
        // diagnoses before.
        s.run("function _order_probe() return 1 end").unwrap();
        s.run("function _order_probe() return 2 end").unwrap();
        assert_eq!(
            s.eval::<i64>("return _order_probe()").unwrap(),
            2,
            "the later definition of a colliding name is the live one"
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    /// **The migration readiness probe** — which stock FrameXML file could be swapped in *today*,
    /// asked of the running loader rather than guessed from a source scan.
    ///
    /// ```text
    /// cargo test -p benilla-app --lib chain_readiness_report -- --ignored --nocapture
    /// ```
    ///
    /// 1751 is a long migration — 88 manifest entries, three of them chain entries at the time this
    /// was written — and the expensive question at every step is *which window is ready*. Picking by
    /// eye means reading a stock file, listing the globals it calls, and grepping each one; that is
    /// slow, and it is wrong in both directions. It over-reports (a name that exists in a comment
    /// greps as present — `framexml-file-demand.py` states that crudeness about itself) and it
    /// under-reports the things a grep cannot see at all: an XML element type the loader does not
    /// build, a script handler nothing dispatches, an attribute silently dropped (1739 measured 151
    /// of those), a template inherited before its definer.
    ///
    /// So the probe does not analyse. It **loads the file** — the whole shipped manifest first, into
    /// a fresh VM, exactly as a real run does, and then the candidate off the chain on top — and
    /// reports what the loader and the VM actually said. That is ground truth: the same machinery
    /// that would run it for real, answering the same question, with no model of the engine in
    /// between that could be out of date.
    ///
    /// **What a clean line does and does not mean.** It means the file *loads* — every element
    /// built, every template resolved, every load-time body ran without raising. It does not mean
    /// the window *works*: a verb that only a click reaches is not exercised by loading, and neither
    /// is anything behind an event. Clean is "start here", not "done"; the window's own test module
    /// and the director's eye are what finish it (§7).
    ///
    /// **Loading on top of the manifest, not instead of it**, because that is the position a
    /// migrated file occupies — every template it inherits is declared by an earlier entry, and
    /// asking whether `ContainerFrame.xml` loads *alone* only measures that it has predecessors.
    ///
    /// ## The false positive this method has, and how to recognise it
    ///
    /// A candidate whose frame NAMES our own shipped file already owns produces failures that are
    /// artefacts of the probe, not of the window. `publish_global` is deliberately non-overwriting
    /// (`0x701bd0`), so the second frame to claim a name gets a wrapper that `_G` never points at —
    /// and any reference body using the `getglobal(this:GetName())` idiom then reads a DIFFERENT
    /// table than the `this` it just wrote to.
    ///
    /// That is exactly what the money frames look like: stock `TradeFrame.xml` reports
    /// `MoneyFrame_Update: attempt to index local 'info'`, because `MoneyFrame_SetType` set
    /// `this.info` on the new frame and `MoneyFrame_Update` read it back off OUR TradeFrame's
    /// same-named one. Delete our counterpart — which is what migrating the window does — and the
    /// collision goes with it. The same shape covers `MailFrame` and `QuestLogFrame`.
    ///
    /// **So a failure inside a name our own manifest also declares is suspect and has to be
    /// re-measured with the counterpart removed.** A failure naming something nothing of ours
    /// declares (`CreateFrame: unknown frame type 'LootButton'`, `attempt to call global
    /// 'UnitFrame_Initialize'`) is real. The probe does not tell the two apart for you; the
    /// question to ask of every line is "does our tree already own this name?".
    ///
    /// Ignored because it stands up ~90 fresh VMs and each one loads the entire interface; it is an
    /// instrument you run when choosing the next window, not a gate.
    #[test]
    #[ignore = "instrument: run by hand when choosing the next window to migrate"]
    fn chain_readiness_report() {
        let _data = benilla_formats::wow_data_or_skip!();

        // The reference's OWN order, off the chain — never a hand-kept list here. A file's position
        // in it is also the answer to "where does its manifest line go", so the report prints it.
        let toc = String::from_utf8_lossy(
            &super::read("Interface\\FrameXML\\FrameXML.toc").expect("the reference's own toc"),
        )
        .into_owned();
        let stock: Vec<String> = toc
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#') && l.ends_with(".xml"))
            .map(str::to_string)
            .collect();

        let migrated: Vec<String> = super::super::addons::Addon::builtin()
            .toc
            .files
            .iter()
            .filter(|f| super::is_chain_entry(f))
            .map(|f| {
                f.rsplit(['\\', '/'])
                    .next()
                    .unwrap_or(f.as_str())
                    .to_string()
            })
            .collect();

        println!(
            "\n=== 1751 migration readiness — {} stock windows ===",
            stock.len()
        );
        println!(
            "{:>3}  {:<32} what stops it (empty = loads clean)",
            "pos", "file"
        );

        let mut clean = Vec::new();
        for (i, name) in stock.iter().enumerate() {
            let pos = i + 1;
            if migrated.iter().any(|m| m == name) {
                println!("{pos:>3}  {name:<32} — already migrated");
                continue;
            }
            let mut s = UiScript::new().expect("VM");
            s.set_screen_size(1024.0, 768.0);
            seat_a_player(&mut s);
            let base = super::super::manifest::load_default_ui(&s);
            assert!(base.is_empty(), "the shipped manifest itself: {base:#?}");
            s.resolve();
            let before = s.errors().len();

            let path = format!("Interface\\FrameXML\\{name}");
            let addon = super::addon(vec![path.clone()]);
            let mut said = addon.load_files(&s, std::slice::from_ref(&path));
            s.resolve();
            said.extend(s.errors().into_iter().skip(before));

            if said.is_empty() {
                clean.push((pos, name.clone()));
                println!("{pos:>3}  {name:<32} CLEAN");
            } else {
                // One line per distinct complaint, deduped and truncated: the same missing verb
                // reported by twelve frames is one fact, and the tail of a Lua traceback is noise.
                let mut seen: Vec<String> = Vec::new();
                for e in said {
                    let one = e.lines().next().unwrap_or("").trim().to_string();
                    let one = if one.len() > 140 {
                        format!("{}…", &one[..140])
                    } else {
                        one
                    };
                    if !one.is_empty() && !seen.contains(&one) {
                        seen.push(one);
                    }
                }
                println!("{pos:>3}  {name:<32} {} issue(s)", seen.len());
                for one in seen.iter().take(6) {
                    println!("         · {one}");
                }
                if seen.len() > 6 {
                    println!("         · … and {} more", seen.len() - 6);
                }
            }
        }

        println!("\n=== loads clean today: {} ===", clean.len());
        for (pos, name) in &clean {
            println!("  {pos:>3}  {name}");
        }
    }

    /// **The readiness probe's companion: not "does it load" but "what would I have to BUILD".**
    ///
    /// ```text
    /// cargo test -p benilla-app --lib chain_gap_report -- --ignored --nocapture
    /// ```
    ///
    /// [`chain_readiness_report`] answers one question well and is silent on the next one. A window
    /// it calls CLEAN can still be a week of work (its verbs are only reached by a click, which
    /// loading never makes), and a window it reports failing may be blocked on a single name. When
    /// the migration ran out of drop-in windows, "which of these is actually cheap" became the
    /// question, and the probe could not answer it.
    ///
    /// So this one reads the calls instead of running them. For every stock window not yet in the
    /// manifest it collects the `Name(` sites across the file and its `.lua`, subtracts what the
    /// file defines itself and what this client already has, and splits the remainder against
    /// **the reference's own `_G`** (`reference/1.12-globals.tsv`, captured from the running
    /// client):
    ///
    /// * `engine=` — the reference has it as an engine binding and we do not. **Real work**, and
    ///   the only column worth planning from.
    /// * `fx=` — the reference has it as a FrameXML function. Cheap by comparison: it lives in
    ///   some stock file, and the name beside it says which, so sourcing that file may be the whole
    ///   fix. `GetText` looked like an engine binding for an hour and turned out to be
    ///   `LocaleProperties.lua`; this column is that lesson, mechanised.
    /// * A name in NEITHER is dropped, and that is the load-bearing filter: 1.12's widget methods
    ///   do not live in `_G`, so `SetText(` and `Hide(` and their two hundred siblings would
    ///   otherwise drown the report. Anything the reference's own global table does not carry is
    ///   not a global.
    ///
    /// **What "already has" means, and why it is asked of a LOADED VM.** An earlier hand-rolled
    /// version of this compared against a bare `UiScript::new()`, which is the ENGINE surface
    /// alone — so every FrameXML function our own interface defines (`ShowUIPanel`,
    /// `StaticPopup_Visible`, `UpdateMicroButtons`, …) read as missing, and the `fx=` column was
    /// mostly noise. Here the manifest is loaded first and `_G` is read after, so the answer is
    /// what this client *actually* answers to.
    ///
    /// **Four columns, because a window can be blocked four ways and this could once see one.**
    /// `engine=` and `fx=` read bare `Name(` globals. `method=` reads `:Name(` calls against the
    /// method surface this engine actually exposes. `LOAD:` is the stock file loaded on top of our
    /// manifest in a fresh VM — the same pass [`chain_readiness_report`] makes, run here so the
    /// answer is in one table.
    ///
    /// That last one is 1801, and it is the same mistake as the third column one step later.
    /// `<LootButton>` and `<TaxiRouteFrame>` are element TAGS: no census of `Name(` or `:Name(`
    /// can reach them, so `LootFrame.xml` sat in this report's "needs NO engine work" list while
    /// the readiness probe was printing `4 issue(s)` for it in a different table. Both tables were
    /// right. Joining them was left to whoever read them, and I read it wrong.
    ///
    /// Until 1798 the method sites were **silently dropped**: a name in neither the engine nor the
    /// FrameXML half of `1.12-globals.tsv` was assumed to be a widget method and skipped, on the
    /// reasoning that widget methods are not in `_G`. True, and it meant the report could not see
    /// a widget method we had *not built*. `MerchantFrame.xml` read `0 engine` and
    /// [`chain_readiness_report`] read CLEAN while the stock row's `<OnEnter>` called
    /// `ShoppingTooltip1:SetMerchantCompareItem(...)`, which this engine did not have then (1802
    /// built it) — so the file loaded, every check passed, and hovering a vendor row would have
    /// raised in play. Both instruments were right about what they measure. Neither measured the
    /// window.
    ///
    /// **A remaining `<?>` in the `fx=` column is usually a LoadOnDemand addon**, not something to
    /// build. `ClassTrainerFrame_Show`, `CraftFrame_Show`, `MacroFrame_SaveMacro`,
    /// `InspectFrame_Show`, `TalentFrame_Toggle` and their siblings live in `Blizzard_*` addons, so
    /// no amount of scanning the extracted **FrameXML** finds them. They arrive when that addon does,
    /// exactly like an `fx=` name with a home.
    ///
    /// **They are NOT unreachable, and this doc used to say they were.** It claimed the install
    /// ships them "packed as `.pub`", which is a misreading of the loose
    /// `Interface\AddOns\<name>\` folder — that really does hold only a `.pub` signature file.
    /// The addon's real `.xml`/`.lua`/`.toc` are inside **`patch.MPQ`**, and `Chain` mounts MPQs, so
    /// `reference_ui::read` reaches them and `is_chain_entry` accepts the path. Verified by reading
    /// `Interface\AddOns\Blizzard_MacroUI\Blizzard_MacroUI.xml` and `Blizzard_InspectUI`'s twin
    /// straight out of the archive. Every one of those windows is buildable today.
    ///
    /// One crudeness remains, stated because it decides how to read the output: neither scan can
    /// see a name reached through `getglobal`, so both can under-report.
    ///
    /// They no longer over-report on comments. Both counted commented-out calls until 1800 — 1.12
    /// comments out whole blocks, and `PaperDollFrame.lua:754-756`'s `ShowInventorySellCursor` is
    /// three commented lines that this report named as that window's last engine gap. A real
    /// binding, never called, blocking a window that was not blocked. [`strip_comments`] takes
    /// Lua `--`/`--[[ ]]` and XML `<!-- -->` out first; it does not track string literals, so it
    /// can only ever under-report, which is the safe direction here. The `method=` column is also receiver-blind: it asks
    /// "does ANY widget type answer to this name", not "does *this* receiver", so a method that
    /// exists on the wrong type still reads as present. It is for ranking work, not for proving a
    /// window done — [`chain_readiness_report`] and the window's own tests are that, and the
    /// paragraph above is what those two are worth on their own.
    #[test]
    #[ignore = "instrument: run by hand when choosing what to build next"]
    fn chain_gap_report() {
        let _data = benilla_formats::wow_data_or_skip!();

        // The reference's own global table, with each name's origin.
        let tsv = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../reference/1.12-globals.tsv"
        );
        let text = std::fs::read_to_string(tsv).expect("the reference surface");
        let mut origin: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let mut f = line.split('\t');
            if let (Some(name), Some(_kind), Some(from)) = (f.next(), f.next(), f.next()) {
                origin.insert(name, from);
            }
        }

        // What this client answers to with its whole interface up — engine bindings AND every
        // global our own FrameXML defines.
        let mut s = UiScript::new().expect("VM");
        s.set_screen_size(1024.0, 768.0);
        seat_a_player(&mut s);
        let failures = super::super::manifest::load_default_ui(&s);
        assert!(failures.is_empty(), "the shipped manifest: {failures:#?}");
        let have: std::collections::HashSet<String> = s
            .eval::<Vec<String>>(
                "local t = {} for k in pairs(_G) do table.insert(t, k) end return t",
            )
            .expect("dump _G")
            .into_iter()
            .collect();

        // `:Name(` — a method call, receiver unknown.
        let called_methods = |text: &str| -> std::collections::HashSet<String> {
            let b: Vec<char> = text.chars().collect();
            let mut out = std::collections::HashSet::new();
            let mut i = 1;
            while i < b.len() {
                // `::` is not a method call, and neither is a `:` inside a word.
                if b[i - 1] == ':' && (i < 2 || b[i - 2] != ':') && b[i].is_ascii_alphabetic() {
                    let mut j = i;
                    while j < b.len() && super::is_word(b[j]) {
                        j += 1;
                    }
                    let mut k = j;
                    while k < b.len() && b[k] == ' ' {
                        k += 1;
                    }
                    if k < b.len() && b[k] == '(' {
                        out.insert(b[i..j].iter().collect::<String>());
                    }
                    i = j;
                    continue;
                }
                i += 1;
            }
            out
        };

        // Whether this engine's widgets answer to a method name — **asked**, not enumerated. A
        // widget's methods come through an `__index` FUNCTION, so there is no table to walk; the
        // only way to know is to look the name up on a real widget. Receiver-blind by design (a
        // `:Name(` site does not say what it is called on), so this answers "does ANY widget type
        // answer to this name", which is the question that catches a method we never built.
        let answers = |s: &UiScript, names: &[String]| -> std::collections::HashSet<String> {
            let list = names
                .iter()
                .map(|n| format!("{n:?}"))
                .collect::<Vec<_>>()
                .join(",");
            s.eval::<Vec<String>>(&format!(
                r#"
                local want = {{{list}}}
                local probes = {{ GameTooltip }}
                local types = {{
                    "Frame", "Button", "CheckButton", "LootButton", "StatusBar", "EditBox",
                    "ScrollFrame",
                    "Slider", "ColorSelect", "MessageFrame", "ScrollingMessageFrame",
                    "SimpleHTML", "Model", "PlayerModel", "DressUpModel", "TabardModel",
                    "Minimap", "MovieFrame",
                }}
                for i = 1, table.getn(types) do
                    local ok, w = pcall(function()
                        return CreateFrame(types[i], "BenillaGapProbe" .. i, UIParent)
                    end)
                    if ok and w then table.insert(probes, w) end
                end
                local host = CreateFrame("Frame", "BenillaGapProbeHost", UIParent)
                table.insert(probes, host:CreateTexture())
                table.insert(probes, host:CreateFontString())
                local out = {{}}
                for i = 1, table.getn(want) do
                    local name, found = want[i], false
                    for j = 1, table.getn(probes) do
                        local ok, v = pcall(function() return probes[j][name] end)
                        if ok and type(v) == "function" then found = true break end
                    end
                    if found then table.insert(out, name) end
                end
                return out
            "#
            ))
            .expect("probe the widget method surface")
            .into_iter()
            .collect()
        };
        // The instrument's own tripwire: if the probe stops working, the `method=` column goes
        // silently empty — which is precisely the failure mode 1798 exists to end.
        let control: Vec<String> = ["SetPoint", "SetMerchantItem", "BenillaNotAMethod"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let got = answers(&s, &control);
        assert!(
            got.contains("SetPoint")
                && got.contains("SetMerchantItem")
                && !got.contains("BenillaNotAMethod"),
            "the method probe is not working — it answered {got:?} for {control:?}"
        );

        let migrated: std::collections::HashSet<String> = super::super::addons::Addon::builtin()
            .toc
            .files
            .iter()
            .filter(|f| super::is_chain_entry(f))
            .filter_map(|f| f.rsplit(['\\', '/']).next().map(str::to_string))
            .collect();

        let toc = String::from_utf8_lossy(
            &super::read("Interface\\FrameXML\\FrameXML.toc").expect("the reference's own toc"),
        )
        .into_owned();
        let stock: Vec<String> = toc
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#') && l.ends_with(".xml"))
            .map(str::to_string)
            .collect();

        // The `.lua` files a stock `.xml` SOURCES through `<Script file=>`. A window's own sourced
        // code is part of the window: its functions are not gaps, and they are where most of its
        // `fx=` names would otherwise be looked for.
        let sourced_luas = |xml_leaf: &str| -> Vec<String> {
            let Some(b) = super::read(&format!("Interface\\FrameXML\\{xml_leaf}")) else {
                return Vec::new();
            };
            let text = String::from_utf8_lossy(&b).into_owned();
            let mut out = Vec::new();
            for (i, _) in text.match_indices("<Script") {
                let rest = &text[i..];
                let Some(end) = rest.find("/>").or_else(|| rest.find('>')) else {
                    continue;
                };
                let tag = &rest[..end];
                if let Some(fi) = tag.find("file=\"") {
                    let after = &tag[fi + 6..];
                    if let Some(q) = after.find('"') {
                        let leaf = after[..q].rsplit(['\\', '/']).next().unwrap_or("");
                        if leaf.ends_with(".lua") {
                            out.push(leaf.to_string());
                        }
                    }
                }
            }
            out
        };

        // `function Name(` across the whole corpus, so an fx gap can name the file that holds it.
        //
        // Each toc `.xml` is scanned together with **every `.lua` it SOURCES**, not just the
        // `X.lua` its own name suggests. Guessing missed the common case: `ActionBarFrame.xml`
        // sources `ActionButton.lua`, there is no `ActionBarFrame.lua`, and `ActionButton.lua` is
        // not a toc line of its own — so the whole `ActionButton_*` family read `<?>`, which says
        // "nothing defines this" about five functions a stock file defines and brings with it.
        // The difference matters for how the column is read: a name with a home ARRIVES when that
        // file does; a `<?>` is something to build.
        let mut home: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for f in &stock {
            let mut cands = vec![f.clone(), format!("{}.lua", &f[..f.len() - 4])];
            cands.extend(sourced_luas(f));
            for cand in cands {
                let Some(bytes) = super::read(&format!("Interface\\FrameXML\\{cand}")) else {
                    continue;
                };
                for line in String::from_utf8_lossy(&bytes).lines() {
                    if let Some(rest) = line.trim_start().strip_prefix("function ") {
                        let name: String = rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        if !name.is_empty() {
                            home.entry(name).or_insert_with(|| cand.clone());
                        }
                    }
                }
            }
        }

        let called = |text: &str| -> std::collections::HashSet<String> {
            let b: Vec<char> = text.chars().collect();
            let mut out = std::collections::HashSet::new();
            let mut i = 0;
            while i < b.len() {
                // A GLOBAL call: uppercase, not inside a word, and **not preceded by `.` or
                // `:`**. Without that last test `info.UpdateFunc(` and `dialog.OnAccept(` read as
                // globals — `.` is not a word character — and a dozen `StaticPopupDialogs` /
                // `MoneyTypeInfo` FIELD names showed up as `<?>` gaps. `:` belongs to the method
                // scan below; `.` belongs to nobody, since a field call arrives with its table.
                let after_field = i > 0 && (b[i - 1] == '.' || b[i - 1] == ':');
                if b[i].is_ascii_uppercase()
                    && !after_field
                    && (i == 0 || !super::is_word(b[i - 1]))
                {
                    let mut j = i;
                    while j < b.len() && super::is_word(b[j]) {
                        j += 1;
                    }
                    let mut k = j;
                    while k < b.len() && b[k] == ' ' {
                        k += 1;
                    }
                    if k < b.len() && b[k] == '(' {
                        out.insert(b[i..j].iter().collect::<String>());
                    }
                    i = j;
                    continue;
                }
                i += 1;
            }
            out
        };

        println!("\n=== 1751 gap report — what each unmigrated window would cost ===");
        #[allow(clippy::type_complexity)] // (blockers, file, engine, fx, method, load errors)
        let mut rows: Vec<(
            usize,
            String,
            Vec<String>,
            Vec<String>,
            Vec<String>,
            Vec<String>,
        )> = Vec::new();
        for f in &stock {
            if migrated.contains(f) {
                continue;
            }
            // The window IS its xml plus every `.lua` it sources — `ActionBarFrame.xml` sources
            // `ActionButton.lua`, and the whole `ActionButton_*` family is that window's own code,
            // not a dependency on somebody else's. Gathering only `X.xml` + `X.lua` reported five
            // of its own functions as gaps.
            let mut text = String::new();
            let mut parts = vec![f.clone(), format!("{}.lua", &f[..f.len() - 4])];
            parts.extend(sourced_luas(f));
            for cand in parts {
                if let Some(b) = super::read(&format!("Interface\\FrameXML\\{cand}")) {
                    text.push_str(&String::from_utf8_lossy(&b));
                }
            }
            let text = strip_comments(&text);
            // …and the names the file declares as LOCALS, which a bare call resolves to. The
            // idiom that needs it is the reference's own dispatch shape:
            //
            //     local OnAccept = StaticPopupDialogs[dialog.which].OnAccept
            //     if ( OnAccept ) then dontHide = OnAccept(dialog.data, dialog.data2) end
            //
            // The call is bare, so the global scan sees `OnAccept(` and reports a gap for a name
            // that is a table field one line up. `OnAccept`, `OnCancel`, `OnShow`, `OnHide` and
            // the three `EditBoxOn*` all arrived that way. A `local` declaration in the same file
            // is proof enough: nothing else could be meant.
            let locals: std::collections::HashSet<String> = text
                .lines()
                .filter_map(|l| l.trim_start().strip_prefix("local "))
                .map(|r| {
                    r.chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect::<String>()
                })
                .filter(|n| !n.is_empty())
                .collect();
            let own: std::collections::HashSet<String> = text
                .lines()
                .filter_map(|l| l.trim_start().strip_prefix("function "))
                .map(|r| {
                    r.chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect::<String>()
                })
                .collect();
            let (mut eng, mut fx) = (Vec::new(), Vec::new());
            let mut names: Vec<String> = called(&text)
                .into_iter()
                .filter(|c| !own.contains(c) && !locals.contains(c) && !have.contains(c))
                .collect();
            names.sort();
            for c in names {
                match origin.get(c.as_str()).copied() {
                    Some("engine") => eng.push(c),
                    Some(_) => {
                        let h = home.get(&c).cloned().unwrap_or_else(|| "?".into());
                        fx.push(format!("{c}<{h}>"));
                    }
                    None => {} // not a global at all — the `:Name(` scan below is what sees it
                }
            }
            // The method half. `own`/`have` do not apply: a method is never a global, so the only
            // question is whether this engine's widgets answer to the name.
            // …and does it LOAD? A window can be blocked on a global, on a widget method, or on a
            // widget TYPE — and the two scans above see only the first two. `<LootButton>` and
            // `<TaxiRouteFrame>` are element tags: nothing in a `Name(` or `:Name(` census can
            // reach them, and `LootFrame.xml` sat in this report's "needs NO engine work" list
            // while `chain_readiness_report` was printing `4 issue(s)` for it in another table.
            //
            // Both tables were right. Joining them was left to whoever read them, and I got it
            // wrong (1801). So this one runs the load itself — the same fresh-VM-plus-manifest
            // pass the readiness probe does — and reports it in the same row.
            // **A `LOAD:` line for a window we ALSO ship is suspect**, and the reason is the same
            // one `chain_readiness_report` carries: `publish_global` is non-overwriting
            // (`0x701bd0`), so loading the stock file on top of our identically-named one leaves
            // every colliding frame's global pointing at OUR frame while the stock file's handlers
            // run against THEIRS. A field set on `this` in an OnLoad is then invisible to
            // `getglobal(name)`, and the failure looks like a bug in whatever read it back.
            //
            // Worked example, because it cost an hour: stock `TradeFrame.xml` reported
            // `MoneyFrame.xml:525: attempt to index local 'info'`. Nothing was wrong with our
            // MoneyFrame — `MoneyFrame_SetType` had set `this.info` correctly, and
            // `getglobal(this:GetName())` answered a DIFFERENT table, because our own
            // `TradeFrame.xml` already owned `TradeRecipientMoneyFrame`.
            //
            // So the column is marked, not trusted. A row whose file we do not ship is a real
            // load failure; a row whose file we do ship needs the swap attempted to know.
            let ours_too = !migrated.contains(f)
                && std::fs::metadata(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("assets/ui")
                        .join(f),
                )
                .is_ok();
            let loads = {
                let mut probe = UiScript::new().expect("VM");
                probe.set_screen_size(1024.0, 768.0);
                seat_a_player(&mut probe);
                let base = super::super::manifest::load_default_ui(&probe);
                assert!(base.is_empty(), "the shipped manifest itself: {base:#?}");
                probe.resolve();
                let before = probe.errors().len();
                let path = format!("Interface\\FrameXML\\{f}");
                let addon = super::addon(vec![path.clone()]);
                let mut said = addon.load_files(&probe, std::slice::from_ref(&path));
                probe.resolve();
                said.extend(probe.errors().into_iter().skip(before));
                said
            };

            let mut asked: Vec<String> = called_methods(&text).into_iter().collect();
            asked.sort();
            let known = answers(&s, &asked);
            let mut meth: Vec<String> = asked.into_iter().filter(|m| !known.contains(m)).collect();
            meth.sort();
            // A suspect LOAD line does not count as a blocker — it is a question, not an answer.
            let load_blockers = if ours_too { 0 } else { loads.len() };
            let loads: Vec<String> = loads
                .into_iter()
                .map(|e| {
                    let one = e.replace('\n', " ");
                    let mark = if ours_too {
                        " (ours too — suspect)"
                    } else {
                        ""
                    };
                    format!("{mark}   {}", &one[..one.len().min(150)])
                })
                .collect();
            rows.push((
                eng.len() + meth.len() + load_blockers,
                f.clone(),
                eng,
                fx,
                meth,
                loads,
            ));
        }
        rows.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        for (n, f, eng, fx, meth, loads) in &rows {
            println!("{n:>3} blocker(s)  {f}");
            if !eng.is_empty() {
                println!("            engine: {}", eng.join(" "));
            }
            if !meth.is_empty() {
                println!("            method: {}", meth.join(" "));
            }
            for e in loads {
                println!("            LOAD:{e}");
            }
            if !fx.is_empty() {
                println!("            fx:     {}", fx.join(" "));
            }
        }
        // "Free" means free of ALL THREE. A window blocked on a widget method, or on a widget type
        // its XML declares, is exactly as blocked as one missing a global — and the `fx=` column
        // is deliberately NOT counted, because a FrameXML function another stock file defines
        // arrives with that file rather than needing to be built.
        let free: Vec<&String> = rows.iter().filter(|r| r.0 == 0).map(|r| &r.1).collect();
        println!(
            "\n=== {} windows are UNBLOCKED — no missing global, no missing method, and the \
             stock file loads clean on top of our manifest ===",
            free.len()
        );
        for f in free {
            println!("  {f}");
        }
    }

    /// **Every function of ours that shadows one the player's own chain already defines.**
    ///
    /// A window that reads unblocked in [`chain_gap_report`] can still fail to swap, and the third
    /// reason found (after a missing widget type and a missing widget method) is this one: our
    /// `assets/ui` file defines a global the reference defines too, our manifest loads a CHAIN file
    /// that also defines it, and whichever lands second wins. `PartyFrame.xml` was the worked
    /// example, and it is worth keeping in the past tense because it is what this check was built
    /// to catch: our `UnitFrames.xml` redefined `UnitFrame_OnEvent`/`UnitFrame_Update` nine
    /// manifest lines after stock `UnitFrame.lua` defined them, so a stock party row built by the
    /// reference's own `UnitFrame_Initialize` called OUR update and indexed a field its rows do not
    /// carry. It loads clean and raises on the first event.
    ///
    /// Shadowing is not automatically wrong — where we ship a file the reference would have
    /// shipped, defining its names is the whole job. It is wrong precisely when **both** copies
    /// load, which is what this reports: a name ours defines that a chain entry in our own
    /// manifest also defines.
    ///
    /// **Two halves, both exact, because both sides DECLARE rather than mention.** The function
    /// half is above. The frame half asks the other question a swap has to answer: *can this stock
    /// window be added at all* — which is different from "does it load", because a stock window we
    /// do not ship under its own name usually has a counterpart of ours under a different one, and
    /// both would declare the same frames.
    ///
    /// That half used to double as the map nothing else held — `FloatingChatFrame.xml` was our
    /// `ChatFrame.xml`, `MainMenuBarMicroButtons.xml` our `MicroMenu.xml`, `StaticPopup.xml` our
    /// `UiPanels.xml`'s dialog half, `PlayerFrame.xml`/`TargetFrame.xml`/`PetFrame.xml` our one
    /// `UnitFrames.xml`. Every one of those is the reference's own file now (1751; the micro row
    /// last, 1987), so the frame half names nothing today. It stays because a stock window
    /// declaring a frame one of ours still holds is the first thing a swap has to rule out —
    /// such a pair can load individually and cannot load side by side.
    ///
    /// Run it before attempting a swap. It predicts which ones will fail without attempting them.
    #[test]
    #[ignore = "instrument: run by hand before attempting a window swap"]
    fn shadowed_reference_functions() {
        let _data = benilla_formats::wow_data_or_skip!();

        // The FRAME NAMES a document declares — `name="X"` on a widget element, minus the
        // `$parent`-relative and `virtual` template forms, which name nothing globally.
        //
        // The sibling of the function check below and exactly as exact, because both sides
        // DECLARE rather than mention. This is the version of the "frame names" idea that works:
        // an earlier attempt scanned names a file *referenced* and was pure noise (it found `UI`
        // and missed the case it was built for), because a reference can live in any file. A
        // declaration cannot.
        //
        // What it catches, in the case it was built for: our `UnitFrames.xml` declared `PetFrame`,
        // and so does the stock `PetFrame.xml`, so adding that stock file alongside ours would have
        // declared the name twice. (That pair is resolved — ours is deleted — but the check is not
        // about those two files; every window still ahead of 1751 has the same collision waiting.)
        // Several stock windows we do not ship under their own name have an equivalent of ours
        // under a different one, and `chain_gap_report` calls every one of them unblocked —
        // truthfully, because the stock file WOULD load; it just cannot load *beside* ours.
        let declares = |text: &str| -> Vec<String> {
            let mut out = Vec::new();
            for (i, _) in text.match_indices("name=\"") {
                let rest = &text[i + 6..];
                let Some(end) = rest.find('"') else { continue };
                let name = &rest[..end];
                if name.starts_with('$') || name.is_empty() {
                    continue;
                }
                // A `virtual="true"` element is a template: its name is a registry key, not a
                // frame, and two files may legitimately hold the same template name only if one
                // replaces the other — which is the same question, so they are reported too.
                out.push(name.to_string());
            }
            out
        };

        // `function Name(a, b, c)` → the name and its PARAMETER LIST. Arity is the third way our
        // files and the reference's can disagree about a name, and the one that fails most
        // quietly: `TextStatusBar_Initialize()` takes no argument in 1.12 and acts on `this`, ours
        // took an optional bar, and `UnitFrames.xml` calls it with one. Swapping that file stopped
        // initialising the unit-frame bars with no error and no missing global — the numerals
        // simply never appeared.
        //
        // **Both directions are silent, and that is the point.** Lua drops extra arguments without
        // complaint, so neither an over- nor an under-supplied call raises; they differ only in
        // WHO loses information:
        //
        //   * ours WIDER  — our callers pass the extra argument and the reference's version drops
        //     it. Breaks when OUR file is swapped out (the `TextStatusBar` case).
        //   * ours NARROWER — the reference's callers pass more than ours takes and ours drops it.
        //     Breaks when a STOCK file is added beside ours and calls our version.
        //
        // The report names the direction because it says which swap the difference is waiting for,
        // not because one of them is safe.
        let params_in = |text: &str| -> Vec<(String, usize)> {
            let mut out = Vec::new();
            for line in text.lines() {
                let Some(rest) = line.trim_start().strip_prefix("function ") else {
                    continue;
                };
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if name.is_empty() {
                    continue;
                }
                let Some(open) = rest.find('(') else { continue };
                let Some(close) = rest[open..].find(')') else {
                    continue;
                };
                let args = rest[open + 1..open + close].trim();
                // `...` is a vararg, which has no fixed arity to compare.
                let n = if args.is_empty() || args == "..." {
                    0
                } else {
                    args.split(',').count()
                };
                out.push((name, n));
            }
            out
        };

        let defined_in = |text: &str| -> Vec<String> {
            text.lines()
                .filter_map(|l| l.trim_start().strip_prefix("function "))
                .map(|r| {
                    r.chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect::<String>()
                })
                .filter(|n| !n.is_empty())
                .collect()
        };

        // Everything the manifest pulls OFF THE CHAIN, and every name each of those defines —
        // including the `.lua` a chain `.xml` sources, which is where most of them live.
        let toc = &super::super::addons::Addon::builtin().toc.files;
        // Load order: a manifest entry at its line, a reached addon's file after everything.
        //
        // **Every manifest entry, not only the chain half.** The map is read twice — once for a
        // chain file's own seat, and once for OURS, to decide which of two definitions stands
        // (`ours_wins` below). Keyed on the chain alone it had no entry for any file of ours, so
        // the second read was an index into a map that could not contain it and the whole
        // instrument panicked with `no entry found for key` — on the first of our files that
        // shares a name with a stock one, which is the only case it exists to report.
        let chain = gated_chain_entries();
        let pos: std::collections::HashMap<&String, usize> = toc
            .iter()
            .enumerate()
            .map(|(k, f)| (f, k))
            .chain(
                chain
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| !toc.contains(f))
                    .map(|(k, f)| (f, toc.len() + k)),
            )
            .collect();
        let mut chain_home: std::collections::HashMap<String, (String, usize)> =
            std::collections::HashMap::new();
        let mut chain_frames: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for entry in &chain {
            let leaf = entry.rsplit(['\\', '/']).next().unwrap_or(entry);
            let mut cands = vec![entry.clone()];
            if let Some(stem) = entry.strip_suffix(".xml") {
                cands.push(format!("{stem}.lua"));
            }
            for cand in cands {
                let Some(b) = super::read(&cand.replace('\\', "/")) else {
                    continue;
                };
                let text = String::from_utf8_lossy(&b).into_owned();
                for name in defined_in(&text) {
                    let at = pos[entry];
                    chain_home.entry(name).or_insert_with(|| {
                        (cand.rsplit('/').next().unwrap_or(leaf).to_string(), at)
                    });
                }
            }
        }

        // …against everything OUR files define.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
        let mut hits: Vec<(String, String, String, bool)> = Vec::new();
        for entry in toc.iter().filter(|f| !super::is_chain_entry(f)) {
            let Ok(text) = std::fs::read_to_string(dir.join(entry)) else {
                continue;
            };
            for name in defined_in(&text) {
                if let Some((home, at)) = chain_home.get(&name) {
                    // Load order settles it: the later definition is the one that stands.
                    let ours_wins = pos[entry] > *at;
                    hits.push((name, entry.clone(), home.clone(), ours_wins));
                }
            }
        }
        hits.sort();
        hits.dedup();
        // The frame names every STOCK window declares — read off the reference's own toc, not off
        // our manifest's chain entries. Scanning only what we already load was the first attempt
        // and it answered 0 by construction: a stock file we do not load is exactly the one whose
        // names could collide, and it was never opened.
        let ref_toc = String::from_utf8_lossy(
            &super::read("Interface\\FrameXML\\FrameXML.toc").expect("the reference's own toc"),
        )
        .into_owned();
        let mut ref_arity: std::collections::HashMap<String, (usize, String)> =
            std::collections::HashMap::new();
        for line in ref_toc.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') || !line.ends_with(".xml") {
                continue;
            }
            for cand in [line.to_string(), format!("{}.lua", &line[..line.len() - 4])] {
                let Some(b) = super::read(&format!("Interface/FrameXML/{cand}")) else {
                    continue;
                };
                let text = String::from_utf8_lossy(&b).into_owned();
                if cand.ends_with(".xml") {
                    for name in declares(&text) {
                        chain_frames.entry(name).or_insert_with(|| line.to_string());
                    }
                }
                for (name, n) in params_in(&text) {
                    ref_arity.entry(name).or_insert((n, cand.clone()));
                }
            }
        }
        assert!(
            chain_frames.contains_key("GameTooltip") && chain_frames.contains_key("PetFrame"),
            "the stock frame-name scan found nothing recognisable ({} names) — an empty answer \
             here reads as \"no collisions\", which is what the first version of this reported \
             for the wrong reason",
            chain_frames.len()
        );

        // …and the frame-name half, over the stock files the manifest does NOT already take off
        // the chain. A file of ours whose stock counterpart we already load is a swap that has
        // happened; this is about the ones that have not.
        let mut frame_hits: Vec<(String, String, String)> = Vec::new();
        for entry in toc.iter().filter(|f| !super::is_chain_entry(f)) {
            let Ok(text) = std::fs::read_to_string(dir.join(entry)) else {
                continue;
            };
            for name in declares(&text) {
                if let Some(home) = chain_frames.get(&name) {
                    // **The LEAF, compared whole — `ends_with` was a mask.** `home` is a bare
                    // `FrameXML.toc` line (`OptionsFrame.xml`) and the manifest carries full chain
                    // paths, so a suffix test made `Interface\FrameXML\UIOptionsFrame.xml` answer
                    // "we already load OptionsFrame.xml". It does not: they are two different
                    // windows, and that one substring silently emptied this table of every
                    // collision the VIDEO window has with ours — the exact set the instrument
                    // exists to print before a swap.
                    let already = toc
                        .iter()
                        .filter(|f| super::is_chain_entry(f))
                        .any(|f| f.rsplit(['\\', '/']).next() == Some(home.as_str()));
                    // A template's name is a registry key rather than a frame, but two files
                    // holding one is the same question, so it is reported the same way.
                    if !already {
                        frame_hits.push((name, entry.clone(), home.clone()));
                    }
                }
            }
        }
        frame_hits.sort();
        frame_hits.dedup();

        println!("\n=== names ours redefines that a CHAIN entry already defines ===");
        println!("{:<36} {:<28} {:<34} winner", "name", "ours", "chain");
        for (name, ours, home, ours_wins) in &hits {
            let w = if *ours_wins { "OURS" } else { "the chain's" };
            println!("{name:<36} {ours:<28} {home:<34} {w}");
        }

        // Grouped by our file, because that is the unit of work: a window cannot swap while OUR
        // file is still standing on the names its stock counterpart needs.
        let mut by_file: std::collections::BTreeMap<&String, usize> =
            std::collections::BTreeMap::new();
        for (_, ours, _, _) in &hits {
            *by_file.entry(ours).or_default() += 1;
        }
        println!(
            "\n=== {} collisions, across {} of our files ===",
            hits.len(),
            by_file.len()
        );
        for (f, n) in &by_file {
            println!("  {n:>3}  {f}");
        }

        // The ARITY half: a name we define that the reference also defines, with a different
        // parameter count. Not a collision — the two need never both load for this to bite — so it
        // is its own section rather than a column.
        let mut arity: Vec<(String, String, usize, usize, String)> = Vec::new();
        for entry in toc.iter().filter(|f| !super::is_chain_entry(f)) {
            let Ok(text) = std::fs::read_to_string(dir.join(entry)) else {
                continue;
            };
            for (name, ours_n) in params_in(&text) {
                if let Some((ref_n, home)) = ref_arity.get(&name) {
                    if *ref_n != ours_n {
                        arity.push((name, entry.clone(), ours_n, *ref_n, home.clone()));
                    }
                }
            }
        }
        arity.sort();
        arity.dedup();
        println!(
            "\n=== {} signatures of ours differ in ARITY from the reference's ===",
            arity.len()
        );
        println!("{:<34} {:<26} ours ref  direction", "name", "ours");
        for (name, ours, a, b, home) in &arity {
            // Which swap this one is waiting for — see the note above; both are silent.
            let dir = if a > b {
                "ours WIDER  — bites when OUR file goes"
            } else {
                "ours NARROWER — bites when THEIRS arrives"
            };
            println!("{name:<34} {ours:<26} {a:>4} {b:>3}  {dir} — ref in {home}");
        }

        // The frame half, grouped the other way — by the STOCK file, because that is the unit of
        // the question it answers: "can this stock window be added?"
        let mut by_stock: std::collections::BTreeMap<&String, std::collections::BTreeSet<&String>> =
            std::collections::BTreeMap::new();
        for (_, ours, home) in &frame_hits {
            by_stock.entry(home).or_default().insert(ours);
        }
        println!(
            "\n=== {} FRAME-NAME collisions: {} stock windows we do not load already have their \
             names declared by a file of ours ===",
            frame_hits.len(),
            by_stock.len()
        );
        for (stock, ours) in &by_stock {
            let n = frame_hits.iter().filter(|(_, _, h)| h == *stock).count();
            let mine: Vec<&str> = ours.iter().map(|s| s.as_str()).collect();
            println!("  {n:>3}  {stock:<34} vs {}", mine.join(", "));
        }
    }

    /// **Every global a MIGRATED window calls is answered by the loaded interface.**
    ///
    /// The gate the loot window and the character sheet both wanted and neither had.
    /// [`chain_readiness_report`] asks "does the stock file LOAD"; [`chain_gap_report`] asks "what
    /// would I have to BUILD before migrating it". Neither asks the question that actually bites
    /// after a swap: *the file is on the chain now — does everything it calls exist?* A missing
    /// FrameXML function is invisible to both, because loading a file never runs the body that
    /// calls it: `LootFrame.xml` shipped load-clean and raised at `LootFrame.lua:85` the first
    /// time it met real data, and stock `CharacterFrame.xml` would have raised on the first tab
    /// HOVER, because `MicroButtonTooltipText` did not exist here (it does now — the stock
    /// `MainMenuBarMicroButtons.xml` declares it, off the chain since 1987).
    ///
    /// So: for every chain `.xml` in the manifest, census the bare `Name(` call sites across it
    /// and its `.lua`, subtract what those two define themselves, keep the names the reference's
    /// own `_G` carries (`reference/1.12-globals.tsv` — anything else is a local, a widget method,
    /// or a table field, and 1.12's widget methods are not globals), and require every survivor to
    /// be non-nil in a VM with the whole shipped manifest up.
    ///
    /// **A gate rather than an instrument** (the `assert` is the point): for a window we have
    /// already migrated, the answer must be zero, and a missing name is a raise waiting for a
    /// player's first click on it.
    ///
    /// Three limits, stated because they decide what a green run is worth. It cannot see a name
    /// reached through `getglobal` (both censuses share that blind spot). It cannot see a WIDGET
    /// method — those are not in `_G`, which is exactly why `chain_gap_report` grew a separate
    /// `method=` column (1798). And "the name exists" is not "the body is right": arity and
    /// semantics are decision 1793's problem and the window's own tests', not this one's.
    #[test]
    fn every_global_a_migrated_window_calls_is_answered() {
        let _data = benilla_formats::wow_data_or_skip!();

        // The reference's own global table — the filter that keeps this from drowning in locals.
        let tsv = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../reference/1.12-globals.tsv"
        );
        let text = std::fs::read_to_string(tsv).expect("the reference surface");
        let reference: std::collections::HashSet<&str> = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.split('\t').next())
            .collect();

        let mut s = UiScript::new().expect("VM");
        s.set_screen_size(1024.0, 768.0);
        seat_a_player(&mut s);
        let failures = super::super::manifest::load_default_ui(&s);
        assert!(failures.is_empty(), "the shipped manifest: {failures:#?}");
        // The whole interface is the manifest AND every LoadOnDemand addon it reaches (1967):
        // what `MacroFrame_SaveMacro` answers to is `Blizzard_MacroUI.lua`, loaded on the first
        // `ShowMacroFrame`, and a call into it from ActionBarFrame.lua is answered exactly then.
        for name in reached_addons() {
            super::super::test_ui::seat_chain_addon(&mut s, &name);
            s.run(&format!("UIParentLoadAddOn(\"{name}\")")).unwrap();
            assert!(
                s.eval::<bool>(&format!("return IsAddOnLoaded(\"{name}\") == 1"))
                    .unwrap(),
                "{name}: reached, seated, and did not load: {:?}",
                s.errors()
            );
        }
        let have: std::collections::HashSet<String> = s
            .eval::<Vec<String>>(
                "local t = {} for k in pairs(_G) do table.insert(t, k) end return t",
            )
            .expect("dump _G")
            .into_iter()
            .collect();

        // **The gaps this gate found on the day it was written, each still open, each named with
        // the window that reaches it and the click that would.** They are listed rather than
        // silenced: a KNOWN entry here is a defect we have and have not fixed, not a tolerance —
        // and the assertion below refuses an entry that no longer describes one, so fixing a gap
        // forces its line out (`frame_flag_gate`'s rule, applied to a second gate).
        //
        // None of them belongs to the character sheet, which is the window that prompted this and
        // reads clean. Each belongs to whichever window's migration left it, and each is one
        // binding or one sourced file away.
        const KNOWN: &[(&str, &str, &str)] = &[
            (
                "ContainerFrame.xml",
                "KeyRingButtonIDToInvSlotID",
                "an engine binding (`1.12-globals.tsv`). `ContainerFrame.lua:617` hovers a KEYRING \
                 slot with it, so the raise needs the keyring open and a key hovered. Ours drives \
                 keyring tooltips through `ContainerFrameAdapters.xml`'s wrapper (0765), which is \
                 why nothing has hit it — the wrapper answers first for our own rows.",
            ),
            (
                "SkillFrame.xml",
                "BuySkillTier",
                "a 5875 binding (`0x4d3e50`: marshals and delegates to a C++ \
                 method/net-send) of the pre-1.12 skill-point purchase UI. the detail bar's LearnSkillButton calls it, and \
                 that button shows only while `UnitCharacterPoints`'s second value or a row's \
                 step/rank cost is non-zero — which no 1.12 server sends. Unreachable until the \
                 skill-point wire exists; not built (1956).",
            ),
            (
                "SkillFrame.xml",
                "AddSkillUp",
                "a 5875 binding (`0x4d3c30`: marshals and delegates to a C++ \
                 method/net-send) of the pre-1.12 skill-point purchase UI. the detail bar's RightArrow calls it, and \
                 that button shows only while `UnitCharacterPoints`'s second value or a row's \
                 step/rank cost is non-zero — which no 1.12 server sends. Unreachable until the \
                 skill-point wire exists; not built (1956).",
            ),
            (
                "SkillFrame.xml",
                "RemoveSkillUp",
                "a 5875 binding (`0x4d3c70`: marshals and delegates to a C++ \
                 method/net-send) of the pre-1.12 skill-point purchase UI. the detail bar's LeftArrow calls it, and \
                 that button shows only while `UnitCharacterPoints`'s second value or a row's \
                 step/rank cost is non-zero — which no 1.12 server sends. Unreachable until the \
                 skill-point wire exists; not built (1956).",
            ),
            (
                "DurabilityFrame.xml",
                "UpdateInventoryAlertStatus",
                "an engine binding. `DurabilityFrame.lua:81` calls it from the armor guy's own \
                 update; our `inventory_alerts` snapshot is recomputed on every inventory push \
                 instead, so the recompute exists and only the Lua verb that forces one does not.",
            ),
            (
                "Blizzard_GMSurveyUI.xml",
                "GMSurveyAnswerSubmit",
                "one of the GM survey's four engine verbs, none built: the survey window opens on \
                 GMSURVEY_DISPLAY, which the stock HelpFrame.lua registers and nothing fires — the \
                 trigger is ticket status 3 on SMSG_GMTICKET_GETTICKET, which vmangos never sends \
                 (1889; the producer gate carries the event). Gated since the addon became a reached \
                 LoadOnDemand row (1967).",
            ),
            (
                "Blizzard_GMSurveyUI.xml",
                "GMSurveyCommentSubmit",
                "one of the GM survey's four engine verbs, none built: the survey window opens on \
                 GMSURVEY_DISPLAY, which the stock HelpFrame.lua registers and nothing fires — the \
                 trigger is ticket status 3 on SMSG_GMTICKET_GETTICKET, which vmangos never sends \
                 (1889; the producer gate carries the event). Gated since the addon became a reached \
                 LoadOnDemand row (1967).",
            ),
            (
                "Blizzard_GMSurveyUI.xml",
                "GMSurveyQuestion",
                "one of the GM survey's four engine verbs, none built: the survey window opens on \
                 GMSURVEY_DISPLAY, which the stock HelpFrame.lua registers and nothing fires — the \
                 trigger is ticket status 3 on SMSG_GMTICKET_GETTICKET, which vmangos never sends \
                 (1889; the producer gate carries the event). Gated since the addon became a reached \
                 LoadOnDemand row (1967).",
            ),
            (
                "Blizzard_GMSurveyUI.xml",
                "GMSurveySubmit",
                "one of the GM survey's four engine verbs, none built: the survey window opens on \
                 GMSURVEY_DISPLAY, which the stock HelpFrame.lua registers and nothing fires — the \
                 trigger is ticket status 3 on SMSG_GMTICKET_GETTICKET, which vmangos never sends \
                 (1889; the producer gate carries the event). Gated since the addon became a reached \
                 LoadOnDemand row (1967).",
            ),
            (
                "StaticPopup.xml",
                "ReplaceTradeEnchant",
                "a registered 1.12 binding (`0x48d330`) whose body is not yet known; it is built \
                 once it is (1960). Reached by TRADE_REPLACE_ENCHANT's Accept, an event this engine does not fire yet.",
            ),
        ];

        let mut missing: Vec<(String, String)> = Vec::new();
        for entry in &gated_chain_entries() {
            // `GlobalStrings.lua` is 4000 lines of `NAME = "…";` and nothing else — it calls no
            // global at all. What it DOES contain is every format specifier and every English
            // sentence in the interface, and this scanner's `name(` shape reads `%d (`, `%s (` and
            // "rank (" out of those literals as calls. It became a manifest entry with 1848 (the
            // reference's own first line, which ours had been loading out of band); skipping it is
            // not a workaround for that, it is the scanner declining to parse prose.
            if entry.ends_with("GlobalStrings.lua") {
                continue;
            }
            let leaf = entry.rsplit(['\\', '/']).next().unwrap_or(entry);
            let mut text = String::new();
            let mut cands = vec![entry.replace('\\', "/")];
            if let Some(stem) = entry.strip_suffix(".xml") {
                cands.push(format!("{stem}.lua").replace('\\', "/"));
            }
            for cand in &cands {
                if let Some(b) = super::read(cand) {
                    text.push_str(&String::from_utf8_lossy(&b));
                    text.push('\n');
                }
            }
            let text = strip_strings(&strip_comments(&text));
            let defines: std::collections::HashSet<String> = text
                .lines()
                .filter_map(|l| l.trim_start().strip_prefix("function "))
                .map(|r| r.chars().take_while(|c| super::is_word(*c)).collect())
                .collect();

            // `name(` at a call position: any identifier not preceded by `.` or `:` (a field or a
            // method) and followed by `(`.
            //
            // **Not capitalised-only.** An earlier cut of this filtered on a leading capital, on
            // the reasoning that 1.12's globals are named that way — most are, and the ones that
            // are not are the ones a stat tooltip is built out of: `strupper`, `strsub`, `abs`,
            // `max`, `floor`, `format`, `getglobal`. `reference/1.12-globals.tsv` is the filter
            // that actually belongs here, and it does not care about case.
            // Names the file binds LOCALLY — `local X`, `local function X`, a `for` loop's
            // variables — are not globals however they are called: the stock StaticPopup.lua
            // reads a dialog's handlers into locals (`local OnAccept = …; OnAccept(…)`) and
            // ChatFrame.lua walks `SlashCmdList` with `for index, value in …; value(msg)`.
            let mut locals: std::collections::HashSet<String> = std::collections::HashSet::new();
            for line in text.lines() {
                let l = line.trim_start();
                let rest = if let Some(r) = l.strip_prefix("local function ") {
                    Some(r)
                } else if let Some(r) = l.strip_prefix("local ") {
                    Some(r)
                } else {
                    l.strip_prefix("for ")
                };
                if let Some(rest) = rest {
                    for name in rest
                        .split(['=', ' ', '\t'])
                        .take_while(|w| *w != "in" && *w != "=" && !w.starts_with('('))
                        .flat_map(|w| w.split(','))
                        .map(|w| w.trim())
                        .filter(|w| !w.is_empty())
                    {
                        let name: String =
                            name.chars().take_while(|c| super::is_word(*c)).collect();
                        if !name.is_empty() {
                            locals.insert(name);
                        }
                    }
                }
            }

            let b: Vec<char> = text.chars().collect();
            let mut called: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut i = 0;
            while i < b.len() {
                if (b[i].is_ascii_alphabetic() || b[i] == '_')
                    && (i == 0 || !super::is_word(b[i - 1]))
                    && (i == 0 || (b[i - 1] != '.' && b[i - 1] != ':'))
                {
                    let mut j = i;
                    while j < b.len() && super::is_word(b[j]) {
                        j += 1;
                    }
                    let mut k = j;
                    while k < b.len() && b[k].is_whitespace() {
                        k += 1;
                    }
                    if k < b.len() && b[k] == '(' {
                        called.insert(b[i..j].iter().collect());
                    }
                    i = j;
                    continue;
                }
                i += 1;
            }

            let mut gaps: Vec<&String> = called
                .iter()
                .filter(|n| !defines.contains(*n))
                .filter(|n| !locals.contains(*n))
                .filter(|n| reference.contains(n.as_str()))
                .filter(|n| !have.contains(*n))
                .collect();
            gaps.sort();
            for n in gaps {
                missing.push((leaf.to_string(), n.clone()));
            }
        }
        missing.sort();

        let news: Vec<String> = missing
            .iter()
            .filter(|(f, n)| !KNOWN.iter().any(|(kf, kn, _)| kf == f && kn == n))
            .map(|(f, n)| format!("{f} calls {n}, which nothing answers to"))
            .collect();
        assert!(
            news.is_empty(),
            "a MIGRATED window calls a global this client does not have — load-clean and dead on \
             the first click that reaches it:\n  {}",
            news.join("\n  ")
        );

        // …and the other direction: a KNOWN entry whose gap has been closed is documentation
        // claiming a defect we do not have, so it must go with the fix.
        let stale: Vec<String> = KNOWN
            .iter()
            .filter(|(kf, kn, _)| !missing.iter().any(|(f, n)| f == kf && n == kn))
            .map(|(kf, kn, why)| format!("{kf} / {kn} — claimed: {why}"))
            .collect();
        assert!(
            stale.is_empty(),
            "{} KNOWN entr(y/ies) name a gap that is closed — delete them:\n  {}",
            stale.len(),
            stale.join("\n  ")
        );
    }

    /// **A chain `.xml` that does not source its own `.lua` needs TWO manifest lines**, and
    /// getting it wrong is silent.
    ///
    /// Most stock windows pull their code in with `<Script file="X.lua"/>`, so naming the `.xml`
    /// brings both. A few do not — `TextStatusBar.xml` and `MoneyInputFrame.xml` declare only a
    /// template, and the reference's own toc lists their `.lua` on the preceding line (l.32-33,
    /// l.11-12). Name the `.xml` alone and the template loads against nothing: every global that
    /// file was supposed to define reads nil, every guarded call becomes a no-op, and there is no
    /// error anywhere.
    ///
    /// Both were live. `TextStatusBar` was caught by three tests that happened to assert on the
    /// numerals; `MoneyInputFrame` was caught only by sweeping for the shape afterwards, and its
    /// manifest header had claimed for months to bring "the ten `MoneyInputFrame_*` verbs" while a
    /// full-manifest probe answered nil for all of them.
    ///
    /// A gate rather than an instrument, because the answer should always be zero and the failure
    /// mode is invisible.
    #[test]
    fn every_chain_xml_brings_its_own_lua() {
        let _data = benilla_formats::wow_data_or_skip!();
        let toc = &super::super::addons::Addon::builtin().toc.files;
        let listed: std::collections::HashSet<&str> = toc
            .iter()
            .map(|f| f.rsplit(['\\', '/']).next().unwrap_or(f))
            .collect();

        let mut orphans = Vec::new();
        for entry in &gated_chain_entries() {
            let leaf = entry.rsplit(['\\', '/']).next().unwrap_or(entry);
            let Some(stem) = leaf.strip_suffix(".xml") else {
                continue;
            };
            let lua = format!("{stem}.lua");
            // No sibling in the archive means there is nothing to miss.
            if super::read(&format!("Interface/FrameXML/{lua}")).is_none() {
                continue;
            }
            let xml = super::read(&entry.replace('\\', "/"))
                .unwrap_or_else(|| panic!("{entry}: not in the chain"));
            let text = String::from_utf8_lossy(&xml);
            let sourced = text
                .to_ascii_lowercase()
                .contains(&format!("file=\"{}\"", lua.to_ascii_lowercase()));
            if !sourced && !listed.contains(lua.as_str()) {
                orphans.push(format!(
                    "{leaf} does not source {lua}, and {lua} is not a manifest entry"
                ));
            }
        }
        assert!(
            orphans.is_empty(),
            "a chain window whose code never loads — silent, every global it defines reads nil:\n  {}",
            orphans.join("\n  ")
        );
    }

    /// A path is a chain entry; a bare name is ours. The one-line rule the manifest rests on.
    #[test]
    fn a_separator_is_what_makes_an_entry_the_players_own_file() {
        assert!(super::is_chain_entry(
            "Interface\\FrameXML\\ContainerFrame.xml"
        ));
        assert!(super::is_chain_entry(
            "Interface/FrameXML/ContainerFrame.xml"
        ));
        assert!(!super::is_chain_entry("BagFrame.xml"));
        assert!(!super::is_chain_entry("ScrollTemplates.xml"));
    }
    /// Every global function and virtual template name a manifest entry declares.
    ///
    /// Line-based for `function NAME(`, tag-based for `name="X" … virtual="true"` — the two
    /// shapes FrameXML actually uses. A chain `.xml` that sources its code with
    /// `<Script file="X.lua"/>` contributes that file's functions too, because the manifest
    /// line brings both (`every_chain_xml_brings_its_own_lua`).
    fn declared_by(entry: &str) -> std::collections::BTreeSet<String> {
        fn attr(tag: &str, key: &str) -> Option<String> {
            let pat = format!("{key}=\"");
            let mut from = 0;
            while let Some(i) = tag[from..].find(&pat) {
                let at = from + i;
                let before_ok = at == 0
                    || tag[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_whitespace());
                let rest = &tag[at + pat.len()..];
                if before_ok {
                    return rest.find('"').map(|j| rest[..j].to_string());
                }
                from = at + pat.len();
            }
            None
        }
        fn harvest(text: &str, out: &mut std::collections::BTreeSet<String>) {
            for line in text.lines() {
                if let Some(rest) = line.trim_start().strip_prefix("function ") {
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() && rest[name.len()..].trim_start().starts_with('(') {
                        out.insert(name);
                    }
                }
            }
            for chunk in text.split('<').skip(1) {
                let Some(end) = chunk.find('>') else { continue };
                let tag = &chunk[..end];
                if tag.contains("virtual=\"true\"") {
                    if let Some(n) = attr(tag, "name") {
                        out.insert(n);
                    }
                }
            }
        }

        let mut out = std::collections::BTreeSet::new();
        let text = if super::is_chain_entry(entry) {
            let Some(bytes) = super::read(&entry.replace('\\', "/")) else {
                return out;
            };
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/ui")
                .join(entry);
            match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => return out,
            }
        };
        harvest(&text, &mut out);

        // A chain .xml's own `<Script file="X.lua"/>` — same folder as the .xml.
        if super::is_chain_entry(entry) && entry.to_ascii_lowercase().ends_with(".xml") {
            let dir = {
                let p = entry.replace('\\', "/");
                p.rsplit_once('/')
                    .map(|(d, _)| d.to_string())
                    .unwrap_or_default()
            };
            for chunk in text.split('<').skip(1) {
                let Some(end) = chunk.find('>') else { continue };
                let tag = &chunk[..end];
                if !tag.trim_start().starts_with("Script") {
                    continue;
                }
                let Some(file) = attr(tag, "file") else {
                    continue;
                };
                if !file.to_ascii_lowercase().ends_with(".lua") {
                    continue;
                }
                if let Some(bytes) = super::read(&format!("{dir}/{}", file.replace('\\', "/"))) {
                    harvest(&String::from_utf8_lossy(&bytes), &mut out);
                }
            }
        }
        out
    }

    /// **A name of ours that a LATER chain entry redeclares is dead code, and nothing says so.**
    ///
    /// The manifest's law is "later line wins": template registration is a `HashMap::insert` and a
    /// global function assignment is an overwrite. So the moment a window migrates and its chain
    /// entry lands BELOW one of our files, every function and template that file shares with the
    /// reference stops running — silently, with our copy still on disk, still commented, still
    /// read by the next session as if it were live.
    ///
    /// That is not a tidiness problem. Our copies DIVERGE from the reference deliberately, and the
    /// divergence is what dies: our retired `UiPanels.xml` carried a `PanelTemplates_TabResize`
    /// with a benilla-only `return tabWidth` that our tab settle read, and the reference's
    /// returns nothing (both are gone — 1988, 1993).
    ///
    /// The reverse direction — ours seated BELOW the chain's, so we silently override the
    /// reference — is a real category too, and a wider audit than this gate.
    #[test]
    fn nothing_we_ship_is_shadowed_by_a_later_chain_entry() {
        let _data = benilla_formats::wow_data_or_skip!();
        let toc = &super::super::addons::Addon::builtin().toc.files;
        let mut ours: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut shadowed: Vec<String> = Vec::new();
        for entry in toc.iter() {
            let names = declared_by(entry);
            if super::is_chain_entry(entry) {
                for n in names {
                    if let Some(file) = ours.remove(&n) {
                        shadowed.push(format!("{n}  (ours in {file}, the chain's in {entry})"));
                    }
                }
            } else {
                for n in names {
                    ours.insert(n, entry.clone());
                }
            }
        }
        shadowed.sort();
        assert!(
            shadowed.is_empty(),
            "dead copies — declared by one of ours, then overwritten by a later chain entry:\n  {}",
            shadowed.join("\n  ")
        );
    }

    /// **A frame that inherits a template the manifest never loads is a WARNING, and warnings are
    /// invisible.** The frame is still built — bare. It gets none of the template's regions, none
    /// of its children, and none of its `<Scripts>`, so whatever that `<OnLoad>` was going to
    /// initialise silently stays nil.
    ///
    /// This shipped. `Blizzard_MacroUI.xml`'s `MacroPopupScrollFrame` inherits FrameXML's
    /// `ClassTrainerListScrollFrameTemplate`; the manifest's own header for that window NAMES that
    /// dependency and then never lists the file declaring it. So the icon picker's scroll frame
    /// came up with no `<OnLoad>`, `ScrollFrame_OnLoad` never ran, `this.offset` was never seeded,
    /// and the reference's `FauxScrollFrame_GetOffset` — `return frame.offset`, with no `or 0`
    /// fallback of the kind our deleted copy had — handed `MacroPopupFrame_Update` a nil to
    /// multiply. Clicking "Change Name/Icon" raised.
    ///
    /// Static rather than a load probe: it needs no VM, no client state and no player, so it
    /// answers for every entry including the ones a running load would never reach.
    #[test]
    fn every_template_the_manifest_inherits_is_declared_by_the_manifest() {
        let _data = benilla_formats::wow_data_or_skip!();
        let toc = &super::super::addons::Addon::builtin().toc.files;

        let mut declared: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut wanted: Vec<(String, String)> = Vec::new();
        for entry in toc.iter() {
            // **`<Include>` counts.** The reference's own `FrameXML.toc` lists only two of its
            // `*Templates.xml` files; the rest are pulled in by the window that needs them
            // (`HonorFrame.xml` -> `HonorFrameTemplates.xml`, and so on), and the loader follows
            // that against the including document's own directory (1186). A walk that reads only
            // the manifest's own lines reports thirteen templates missing that are not — which is
            // exactly what the first run of this gate did.
            let mut text = String::new();
            for src in entry_sources(entry) {
                text.push_str(&src);
                text.push('\n');
            }
            if text.is_empty() {
                continue;
            }
            for chunk in text.split('<').skip(1) {
                let Some(end) = chunk.find('>') else { continue };
                let tag = &chunk[..end];
                if tag.contains("virtual=\"true\"") {
                    if let Some(n) = tag_attr(tag, "name") {
                        declared.insert(n);
                    }
                }
                if let Some(list) = tag_attr(tag, "inherits") {
                    for name in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        wanted.push((name.to_string(), entry.clone()));
                    }
                }
            }
        }

        let mut missing: Vec<String> = wanted
            .into_iter()
            .filter(|(n, _)| !declared.contains(n))
            .map(|(n, e)| format!("{n}  (inherited in {e})"))
            .collect();
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "the manifest inherits templates it never loads — the frames are built BARE, with no \
             scripts, and nothing errors:\n  {}",
            missing.join("\n  ")
        );
    }

    /// One `name="…"`-style attribute out of a raw tag, matched only at a token boundary.
    fn tag_attr(tag: &str, key: &str) -> Option<String> {
        let pat = format!("{key}=\"");
        let mut from = 0;
        while let Some(i) = tag[from..].find(&pat) {
            let at = from + i;
            let boundary = at == 0
                || tag[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_whitespace());
            let rest = &tag[at + pat.len()..];
            if boundary {
                return rest.find('"').map(|j| rest[..j].to_string());
            }
            from = at + pat.len();
        }
        None
    }

    /// A manifest entry's source text and everything it `<Include>`s, transitively.
    ///
    /// The chain for a path, `assets/ui` for a bare name — and an include resolves against the
    /// INCLUDING document's own directory in its own source's path space, which is the rule the
    /// loader follows (1186).
    /// The reference's LoadOnDemand Blizzard addons this interface REACHES — every `Blizzard_*`
    /// name a `UIParentLoadAddOn("…")` literal names in our own files or in a manifest chain
    /// entry's sources (the reference's `*_LoadUI` loaders in UIParent.xml, the options window's
    /// combat-text load). They have no manifest row, exactly as the reference's `FrameXML.toc`
    /// has none, so this is how the gates know which addon files are part of the shipped
    /// interface (1967). An addon nothing loads is an unbuilt window, not a migrated one.
    fn reached_addons() -> Vec<String> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
        let mut out: Vec<String> = Vec::new();
        for entry in &super::super::addons::Addon::builtin().toc.files {
            let texts = if super::is_chain_entry(entry) {
                entry_sources(entry)
            } else {
                std::fs::read_to_string(dir.join(entry))
                    .into_iter()
                    .collect()
            };
            for text in texts {
                let text = strip_comments(&text);
                for (i, _) in text.match_indices("UIParentLoadAddOn(\"Blizzard_") {
                    let rest = &text[i + "UIParentLoadAddOn(\"".len()..];
                    if let Some(end) = rest.find('"') {
                        let name = rest[..end].to_string();
                        if !out.contains(&name) {
                            out.push(name);
                        }
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Every chain file the shipped interface loads, in load order: the manifest's chain entries,
    /// then each reached addon's files (`Interface\AddOns\<name>\<file>`, the addon's own toc
    /// order) — LoadOnDemand loads after everything. The set every gate over chain files walks
    /// (1967); an addon an opener names that the chain does not carry is a finding, not a skip.
    fn gated_chain_entries() -> Vec<String> {
        let mut out: Vec<String> = super::super::addons::Addon::builtin()
            .toc
            .files
            .iter()
            .filter(|f| super::is_chain_entry(f))
            .cloned()
            .collect();
        for name in reached_addons() {
            let bytes =
                super::read(&format!("Interface/AddOns/{name}/{name}.toc")).unwrap_or_else(|| {
                    panic!("{name}: reached by UIParentLoadAddOn, not on the chain")
                });
            let toc = benilla_ui::toc::Toc::parse(&benilla_ui::source::decode(&bytes));
            for file in &toc.files {
                out.push(format!(
                    "Interface\\AddOns\\{name}\\{}",
                    file.replace('/', "\\")
                ));
            }
        }
        out
    }

    fn entry_sources(entry: &str) -> Vec<String> {
        fn read_one(path: &str, chain: bool) -> Option<String> {
            if chain {
                let bytes = super::read(&path.replace('\\', "/"))?;
                return Some(String::from_utf8_lossy(&bytes).into_owned());
            }
            std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("assets/ui")
                    .join(path),
            )
            .ok()
        }
        let chain = super::is_chain_entry(entry);
        let dir = {
            let p = entry.replace('\\', "/");
            p.rsplit_once('/').map(|(d, _)| d.to_string())
        };
        let mut out = Vec::new();
        let mut queue = vec![entry.to_string()];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        while let Some(path) = queue.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            let Some(text) = read_one(&path, chain) else {
                continue;
            };
            for chunk in text.split('<').skip(1) {
                let Some(end) = chunk.find('>') else { continue };
                let tag = &chunk[..end];
                // `<Include>` brings a sibling document; `<Script file=>` brings the code, and
                // that is where nearly every `RegisterEvent` lives — a reader that follows only
                // the first sees a window's frames without its handlers.
                let kind = tag.trim_start();
                if !kind.starts_with("Include") && !kind.starts_with("Script") {
                    continue;
                }
                if let Some(file) = tag_attr(tag, "file") {
                    let next = match &dir {
                        Some(d) => format!("{d}/{}", file.replace('\\', "/")),
                        None => file.replace('\\', "/"),
                    };
                    queue.push(next);
                }
            }
            out.push(text);
        }
        out
    }

    /// **Every faux list must declare its own `<OnVerticalScroll>`, and no file may still write
    /// `frame.updateFunc`.**
    ///
    /// `FauxScrollFrame_OnVerticalScroll` is the ONLY thing that writes `frame.offset`, and it runs
    /// from a handler the OWNER declares — the reference has no `updateFunc` field, which is what
    /// our retired kit used. A window that inherits `FauxScrollFrameTemplate` without that handler
    /// loads clean, shows its bar, moves its thumb, and never scrolls its list. Nothing errors.
    ///
    /// A gate rather than a test per window, because what makes it silent is structural: a list
    /// only scrolls when something drives it, and most window tests never do.
    #[test]
    fn every_faux_scroll_frame_declares_its_own_on_vertical_scroll() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ui");
        let mut unwired: Vec<String> = Vec::new();
        let mut stale: Vec<String> = Vec::new();
        for file in std::fs::read_dir(&dir).expect("assets/ui").flatten() {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "xml") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).unwrap_or_default();

            for line in text.lines() {
                let code = line.trim_start();
                if code.starts_with("--") || code.starts_with("<!--") {
                    continue;
                }
                if code.contains(".updateFunc") && code.contains('=') && !code.contains("==") {
                    stale.push(format!("{name}: {}", code.trim()));
                }
            }

            // Each `<ScrollFrame … inherits="…FauxScrollFrameTemplate">` up to its close.
            let mut from = 0;
            while let Some(i) = text[from..].find("<ScrollFrame ") {
                let start = from + i;
                let Some(gt) = text[start..].find('>') else {
                    break;
                };
                let tag = &text[start..start + gt];
                from = start + gt;
                if !tag.contains("FauxScrollFrameTemplate") {
                    continue;
                }
                // A VIRTUAL template is not a list. `BenillaAuctionScrollTemplate` is the auction
                // window's shared shape and legitimately carries no handler: its four instances
                // each repaint a different pane, so each declares its own. What must be wired is
                // the instance.
                if tag.contains("virtual=\"true\"") {
                    continue;
                }
                let Some(end) = text[start..].find("</ScrollFrame>") else {
                    continue;
                };
                let body = &text[start..start + end];
                if !body.contains("OnVerticalScroll") {
                    let who = tag_attr(tag, "name").unwrap_or_else(|| "?".into());
                    unwired.push(format!("{name}: {who}"));
                }
            }
        }
        assert!(
            unwired.is_empty(),
            "a faux list with no <OnVerticalScroll> — its bar moves and the list never follows:\n  {}",
            unwired.join("\n  ")
        );
        assert!(
            stale.is_empty(),
            "`frame.updateFunc` is our retired kit's field; the reference has none:\n  {}",
            stale.join("\n  ")
        );
    }
    /// `text` with every whitespace run that precedes a `.` removed — a method chain rustfmt
    /// broke across lines reads as one call again.
    fn glue_chains(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut pending = String::new();
        for c in text.chars() {
            if c.is_whitespace() {
                pending.push(c);
            } else {
                if c != '.' {
                    out.push_str(&pending);
                }
                pending.clear();
                out.push(c);
            }
        }
        out.push_str(&pending);
        out
    }

    /// **A stock file listening for an event nothing produces is silent on both sides.**
    ///
    /// This is 1819's shape: `ui_unit.rs` fired the Classic Era power pair while the reference's
    /// frames registered the 1.12 per-resource names, so no mana bar could live-update — and
    /// nothing anywhere said so, because an event name is a plain string at both ends. 1818 is the
    /// same seam one API over. The *arity* half of that class already has an instrument (the shape
    /// gate, 1842/1843/1845); this is the event half, which had none.
    ///
    /// A CENSUS with a declared set, not a hard zero: most of these are features we have not built,
    /// and listing them is the point. What must not happen is a NEW one appearing — that means a
    /// window migration just brought a listener nothing feeds, which is exactly how 1819 arrived.
    ///
    /// **Constructed names have to be declared**, because a literal scan cannot see them:
    /// `ui_unit.rs` builds the power events with `format!("UNIT_{}", power_token(...))`, and a gate
    /// blind to that reports nine false positives — which is what its first run did.
    #[test]
    fn every_event_a_chain_file_registers_has_a_producer() {
        let _data = benilla_formats::wow_data_or_skip!();

        // Names benilla builds at runtime rather than writing as literals. Each is a family, with
        // the site that constructs it — an entry here is a promise that something fires it.
        const CONSTRUCTED: &[&str] = &[
            // `ui_unit.rs`: `format!("UNIT_{}", power_token(ty))` and its `UNIT_MAX…` twin, over
            // `power_token`'s five resources (`unit/mod.rs`).
            "UNIT_MANA",
            "UNIT_RAGE",
            "UNIT_FOCUS",
            "UNIT_ENERGY",
            "UNIT_HAPPINESS",
            "UNIT_MAXMANA",
            "UNIT_MAXRAGE",
            "UNIT_MAXFOCUS",
            "UNIT_MAXENERGY",
            "UNIT_MAXHAPPINESS",
        ];

        // Registered by a chain file we load, produced by nothing. Each is a feature we have not
        // built; none is 1819-shaped, because no PAIR is split (a half-fired pair is the tell).
        const UNPRODUCED: &[(&str, &str)] = &[
            // ── The stock `UIParent.lua`'s own listeners (1988) — every one is a dialog or a
            // notice the reference's engine raises for a condition benilla's session does not
            // reach yet. Each names the arm that would fire.
            (
                "ADDON_ACTION_FORBIDDEN",
                "UIParent.lua — the protected-action refusal; benilla has no protected-call \
                 taint model, so nothing can raise it",
            ),
            (
                "MACRO_ACTION_FORBIDDEN",
                "UIParent.lua — the macro half of ADDON_ACTION_FORBIDDEN, same reason",
            ),
            (
                "AUTOEQUIP_BIND_CONFIRM",
                "UIParent.lua — the bind-on-equip confirm for an AUTOEQUIP (right-click) path; \
                 benilla's equip path fires the EQUIP_BIND_CONFIRM sibling only",
            ),
            (
                "EQUIP_BIND_CONFIRM",
                "UIParent.lua — the bind-on-equip confirm; benilla's inventory feed does not \
                 derive the server's confirm ask yet",
            ),
            (
                "USE_BIND_CONFIRM",
                "UIParent.lua — the bind-on-use confirm, the same gap from the use path",
            ),
            (
                "BILLING_NAG_DIALOG",
                "UIParent.lua — the subscription-time nag; vmangos never sends it",
            ),
            (
                "IGR_BILLING_NAG_DIALOG",
                "UIParent.lua — the internet-cafe billing nag, likewise never sent",
            ),
            (
                "GOSSIP_ENTER_CODE",
                "UIParent.lua — the code-entry gossip option (a door with a combination); \
                 benilla's gossip feed carries no code-entry option kind yet",
            ),
            (
                "MEMORY_EXHAUSTED",
                "UIParent.lua — the client's own out-of-memory dialog; benilla's allocator \
                 failure is a Rust abort, not a Lua event",
            ),
            (
                "MEMORY_RECOVERED",
                "UIParent.lua — the other half of MEMORY_EXHAUSTED",
            ),
            (
                "PLAYER_SKINNED",
                "UIParent.lua — the corpse-skinned notice; benilla's loot feed does not derive it",
            ),
            (
                "TRADE_REQUEST",
                "UIParent.lua — the trade ASK dialog, and the one entry on this list that is \
                 UNPRODUCEABLE rather than unbuilt: the 5875 client registers the event and \
                 signals it from NOWHERE (a whole-image census: no signal site passes its id, \
                 `0x11d`), so StaticPopupDialogs[\"TRADE\"] is \
                 dead code THERE too. benilla wired the dialog up once and took it back out — \
                 decision 1764. Producing this would be a divergence, not a fix",
            ),
            (
                "TRADE_REPLACE_ENCHANT",
                "UIParent.lua — the enchant-replacement confirm inside a trade; benilla's trade \
                 feed does not derive it",
            ),
            (
                "CLOSE_WORLD_MAP",
                "WorldMapFrame.lua — the engine-side close the reference fires when the map is \
                 shut from outside its own frame; benilla closes the map through the frame's own \
                 hide path only (1980)",
            ),
            ("DISPLAY_SIZE_CHANGED", "the four paperdoll files"),
            (
                "GMSURVEY_DISPLAY",
                "HelpFrame.lua — the post-ticket survey. A real 1.12 event (fired at \
                 `0x5e797b`, id 538) whose whole UI is the LoadOnDemand `Blizzard_GMSurveyUI`; \
                 we have neither the producer nor the addon on the chain, and the ticket \
                 flow works without it",
            ),
            ("ITEM_TEXT_TRANSLATION", "ItemTextFrame.lua"),
            ("PET_UI_CLOSE", "PetPaperDollFrame.lua"),
            ("PET_UI_UPDATE", "PetPaperDollFrame.lua"),
            ("PLAYER_DAMAGE_DONE_MODS", "PaperDollFrame.lua"),
            (
                "SHOW_COMPARE_TOOLTIP",
                "PaperDollFrame.lua — the second `TRADE_REQUEST` (decision 1764): event 377 is \
                 registered in 5875 and signalled from NOWHERE (zero fire sites in the whole \
                 image), so this listener is dead code THERE \
                 too. benilla fired it from 0283 until 2202, then drove the plates itself on a \
                 shift-held hover until 2210; both were supersets. Nothing in this engine seats a \
                 shopping plate now — the reference's own callers do (`MerchantFrame.xml:63-80`, \
                 the auction rows), which is the whole of the compare in 1.12.1. Producing this \
                 event would be a divergence, not a fix",
            ),
            ("SYSMSG", "UIErrorsFrame.lua"),
            ("UNIT_DEFENSE", "PetPaperDollFrame.lua"),
            (
                "UNIT_QUEST_LOG_CHANGED",
                "QuestLogFrame.lua — a party member's quest-log fields changing (the reference \
                 fires it off the unit's PLAYER_QUEST_LOG_* descriptor updates); benilla's unit \
                 feed does not derive it yet (1944)",
            ),
            (
                "UNIT_MODEL_CHANGED",
                "four files — the paperdoll model refresh",
            ),
            ("UNIT_PORTRAIT_UPDATE", "three files — the portrait refresh"),
            (
                "ZONE_UNDER_ATTACK",
                "ChatFrame.lua — the reference's `SMSG_ZONE_UNDER_ATTACK` line (\"%s is under \
                 attack!\"); the wire handler is not built (1948)",
            ),
        ];

        let mut fired: std::collections::HashSet<String> =
            CONSTRUCTED.iter().map(|s| (*s).to_string()).collect();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("crates");
        let caps = |lit: &str| {
            !lit.is_empty()
                && lit
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
        };
        // The two literal shapes an indirect fire's file can carry: `"EVENT", vec!` and a match
        // arm `=> "EVENT",` — the event-name table a `fire_event(name_of(kind), …)` reads.
        let scan_arms = |text: &str, fired: &mut std::collections::HashSet<String>| {
            const SHAPE: &str = "\", vec!";
            for (i, _) in text.match_indices(SHAPE) {
                let before = &text[..i];
                let Some(q) = before.rfind('"') else { continue };
                let lit = &before[q + 1..];
                if caps(lit) {
                    fired.insert(lit.to_string());
                }
            }
            let lines: Vec<&str> = text.lines().map(str::trim).collect();
            for (i, t) in lines.iter().enumerate() {
                let (t, arm_value) = match t.find("=> \"") {
                    Some(k) => (t[k + 3..].trim_end_matches(','), true),
                    None => (*t, false),
                };
                let Some(lit) = t.strip_prefix('"').and_then(|x| x.strip_suffix('"')) else {
                    continue;
                };
                if arm_value && caps(lit) {
                    fired.insert(lit.to_string());
                    continue;
                }
                let arm = i > 0
                    && lines[i - 1].ends_with('{')
                    && lines.get(i + 1).is_some_and(|n| n.starts_with('}'));
                if arm && caps(lit) {
                    fired.insert(lit.to_string());
                }
            }
        };
        // An indirect fire — `fire_event(EXECUTE_CHAT_LINE, …)`, `fire_event(event_name(kind), …)`
        // — names a const or a function; the literal lives where THAT is defined, which may be
        // another file (1948: `ui_chat::event::event_name` answers for `frames::route`'s fire).
        let mut indirect: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut texts: Vec<String> = Vec::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let text = std::fs::read_to_string(&p).unwrap_or_default();
                    // rustfmt splits a long chain at its dots (`model\n.pending_events\n.push((`), so
                    // the three shapes are matched with the whitespace before each `.` removed.
                    let text = glue_chains(&text);
                    // The engine's deferred lane (`pending_events.push((name, args))`,
                    // `cursor.rs`) is a fire too — the pet grid pair rides it (1953).
                    const CALLS: [&str; 3] = [
                        concat!("fire_event", "("),
                        concat!("fire_event_into", "("),
                        concat!("pending_events.push", "(("),
                    ];
                    let mut fires_indirectly = false;
                    for call in CALLS {
                        let mut from = 0;
                        while let Some(i) = text[from..].find(call) {
                            let at = from + i + call.len();
                            from = at;
                            let rest = text[at..].trim_start();
                            let rest = rest.strip_prefix("lua,").map_or(rest, str::trim_start);
                            if let Some(body) = rest.strip_prefix('"') {
                                if let Some(end) = body.find('"') {
                                    fired.insert(body[..end].to_string());
                                }
                            } else {
                                fires_indirectly = true;
                                let ident: String = rest
                                    .chars()
                                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                                    .collect();
                                if !ident.is_empty() {
                                    indirect.insert(ident);
                                }
                            }
                        }
                    }
                    if fires_indirectly {
                        scan_arms(&text, &mut fired);
                    }
                    texts.push(text);
                }
            }
        }
        for text in &texts {
            for ident in &indirect {
                let decl = format!("const {ident}: &str = \"");
                if let Some(k) = text.find(&decl) {
                    let body = &text[k + decl.len()..];
                    if let Some(end) = body.find('"') {
                        fired.insert(body[..end].to_string());
                    }
                }
                if text.contains(&format!("fn {ident}(")) {
                    scan_arms(text, &mut fired);
                }
            }
        }

        let mut dead: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for entry in &gated_chain_entries() {
            for text in entry_sources(entry) {
                let mut from = 0;
                while let Some(i) = text[from..].find("RegisterEvent(") {
                    let at = from + i + "RegisterEvent(".len();
                    from = at;
                    let rest = text[at..].trim_start();
                    if let Some(body) = rest.strip_prefix('"') {
                        if let Some(end) = body.find('"') {
                            let ev = &body[..end];
                            if !fired.contains(ev) {
                                dead.entry(ev.to_string()).or_insert_with(|| entry.clone());
                            }
                        }
                    }
                }
            }
        }

        let expected: std::collections::HashSet<&str> =
            UNPRODUCED.iter().map(|(e, _)| *e).collect();
        let surprises: Vec<String> = dead
            .iter()
            .filter(|(e, _)| !expected.contains(e.as_str()))
            .map(|(e, f)| format!("{e}  (registered in {f})"))
            .collect();
        assert!(
            surprises.is_empty(),
            "a chain file listens for an event NOTHING fires, and it is not one of the known gaps \
             — this is 1819 arriving: a window migration brought a listener with no producer:\n  {}",
            surprises.join("\n  ")
        );

        let fixed: Vec<&str> = UNPRODUCED
            .iter()
            .map(|(e, _)| *e)
            .filter(|e| !dead.contains_key(*e))
            .collect();
        assert!(
            fixed.is_empty(),
            "these now HAVE a producer — take them out of UNPRODUCED so the list keeps meaning \
             what it says:\n  {fixed:?}"
        );
    }

    /// Seat a player before a probe loads the manifest — **what the live client always does.**
    ///
    /// The in-game UI materializes on world entry (1051), so a player always exists by the time
    /// the manifest loads, and the stock macro window's character tab formats `UnitName("player")`
    /// into its label inside its own `OnLoad`. A manifest load with no player is a state the
    /// client never reaches — and one a probe reaches by default, where it raises
    /// `bad argument #2 to 'format'` and looks exactly like a load failure.
    ///
    /// This was five identical copies of the same six-line comment and the same seven-line seed,
    /// inlined at every probe in this file — and the sixth, `chain_gap_report`'s, did not have it,
    /// which is why that instrument could not run at all. One function is harder to forget.
    fn seat_a_player(s: &mut UiScript) {
        s.set_unit(
            "player",
            Some(benilla_ui::script::UnitState {
                exists: true,
                name: Some("Probefour".into()),
                level: 60,
                ..Default::default()
            }),
        );
    }

    /// **Every event benilla fires must be an event the 1.12 client HAS** — the 1818/1819 seam,
    /// in the one direction nothing was checking.
    ///
    /// [`every_event_a_chain_file_registers_has_a_producer`] runs the other way: a stock file
    /// listens, does anything fire it. This asks whether a name we fire is a 1.12 name at all.
    /// Firing a Classic Era event is invisible to every other gate — our own halves agree with
    /// each other, the name is spelled correctly, and Lua that never registers it never notices.
    /// That is exactly how `UNIT_POWER_UPDATE` (1819) survived, and how the three this test found
    /// on its first run did: `BAG_UPDATE_DELAYED` (Era-only; 1.12 has `BAG_UPDATE` alone),
    /// `LOOT_UPDATE` and `UPDATE_LOOT_ROLL` (both invented here), each fired into a room with
    /// nobody in it.
    ///
    /// **The oracle is the reference binary's own string table.** An event the client can
    /// dispatch is a NUL-terminated string in `WoW.exe`; a name that is not there is a name the
    /// client cannot dispatch. That is a stronger oracle than the FrameXML corpus, which only
    /// shows what the stock UI happens to consume — and which this repo has only four of the
    /// LoadOnDemand addons of, so a corpus grep alone flags real events like `CRAFT_UPDATE`.
    ///
    /// Test files are skipped: `script/tests/events.rs` fires synthetic names (`E3`) at the
    /// dispatcher on purpose, and a test's own scaffolding is not a product surface.
    #[test]
    fn every_event_we_fire_is_an_event_the_reference_has() {
        let data = benilla_formats::wow_data_or_skip!();
        let exe = data.parent().expect("install root").join("WoW.exe");
        let Ok(bytes) = std::fs::read(&exe) else {
            eprintln!("skipping: no WoW.exe at {exe:?}");
            return;
        };

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root")
            .join("crates");
        let mut fired: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                // A test's own synthetic events are not a surface we ship.
                if p.to_string_lossy().contains("test") {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                // Split so this file cannot match its OWN walker — it did on the first run, and
                // reported four fragments of this function as ghost events.
                const CALL: &str = concat!("fire_event", "(");
                let mut from = 0;
                while let Some(i) = text[from..].find(CALL) {
                    let at = from + i + CALL.len();
                    from = at;
                    let rest = text[at..].trim_start();
                    if let Some(body) = rest.strip_prefix('"') {
                        if let Some(end) = body.find('"') {
                            fired
                                .entry(body[..end].to_string())
                                .or_insert_with(|| p.display().to_string());
                        }
                    }
                }
            }
        }
        assert!(
            fired.len() > 100,
            "the walker found only {} fired events — it stopped matching, which would make this \
             gate silently vacuous",
            fired.len()
        );

        let ghosts: Vec<String> = fired
            .iter()
            // A `BENILLA_`-prefixed name declares itself ours and cannot be mistaken for a 1.12
            // one — the same discipline `BENILLA_ALLOW_OWN_UI` uses. `BENILLA_QUEST_PROGRESS` is
            // the live example: an engine event our quest log registers, which exists because the
            // shipped 1.12 auto-watch chain is broken at the `QUEST_WATCH_UPDATE` arg seam. The
            // prefix is what makes an invented event honest instead of a mistake.
            .filter(|(ev, _)| !ev.starts_with("BENILLA_"))
            .filter(|(ev, _)| {
                let needle: Vec<u8> = ev.bytes().chain(std::iter::once(0)).collect();
                !bytes.windows(needle.len()).any(|w| w == needle)
            })
            .map(|(ev, at)| format!("{ev}  (fired from {at})"))
            .collect();
        assert!(
            ghosts.is_empty(),
            "benilla fires {} event(s) the 1.12 client does not have — a Classic Era name, or one \
             invented here. Nothing in the stock UI can ever listen for these:\n  {}",
            ghosts.len(),
            ghosts.join("\n  ")
        );
    }
}
