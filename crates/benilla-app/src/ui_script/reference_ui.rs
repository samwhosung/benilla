//! The stock 1.12 FrameXML this client runs off the player's own patch chain, as the third
//! [`super::addons::Source`]; the parse, `<Include>` and `<Script file=>` resolution and chunk
//! naming are [`super::addons::Addon`]'s.
//!
//! `assets/ui/benilla.toc` is the one load order. An entry with a path separator comes off the
//! chain, a bare filename is a file we ship ([`is_chain_entry`]), and a name defined by both goes
//! to the later line. Without client data the chain files are absent, and the log says so once.

use std::sync::OnceLock;

use benilla_formats::Chain;
use benilla_ui::toc::Toc;
use bevy::prelude::*;

use super::addons::{Addon, Source};

/// The addon name the stock files load under. FrameXML is not an addon: it gets no
/// `ADDON_LOADED`, and [`Addon::chunk_name`] names its chunks by chain path, as the 1.12 client
/// does, so an addon's `\AddOns\` debugstack pattern never matches a FrameXML frame.
pub(super) const NAME: &str = "FrameXML";

/// Whether a manifest entry comes off the player's chain: it has a path separator. Decidable
/// because `assets/ui` is flat, which `manifest::tests` pins.
pub(super) fn is_chain_entry(entry: &str) -> bool {
    entry.contains('\\') || entry.contains('/')
}

/// The stock interface as an [`Addon`] over the chain; `files` are full chain paths.
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

/// One file's bytes off the player's patch chain, by internal path. Bytes, not a string: Lua
/// takes a chunk as stored, and a cp1252 file is not valid UTF-8.
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

/// The player's patch chain, opened once per process and shared by every VM. Process-local, not
/// the one [`benilla_assets`] holds: tests, the addon harness and a bare `UiScript` load the
/// interface with no Bevy world to ask.
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

    /// Every `RegisterForSave`'d UI global defaults to the value the reference's own
    /// `UIOptionsFrame_Init` assigns, the FrameXML half of the rule `cvars::REGISTERED` holds for
    /// CVars. A name the reference declares elsewhere (`SHOW_OFFLINE_GUILD_MEMBERS`,
    /// `FriendsFrame.lua:13`) is uncovered, not failed; the covered count is asserted.
    #[test]
    fn our_saved_ui_globals_default_to_the_references_own_values() {
        let _data = benilla_formats::wow_data_or_skip!();

        // Only `NAME = "literal"` or `NAME = number` at a line's head: a guarded or copied
        // assignment is not a factory default.
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
                continue; // a uvar is upper case; anything else is a local, a field or a comparison
            }
            let value = value.trim().trim_end_matches(';').trim();
            let literal = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'));
            let Some(v) = literal.or_else(|| value.parse::<i64>().ok().map(|_| value)) else {
                continue; // an expression, not a factory default
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
            // Through `tostring`: the reference declares `AUTO_QUEST_WATCH` as the number 1
            // (`UIOptionsFrame.lua:122`) and the rest as strings.
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

    /// Blanks the contents of every Lua string literal (`"…"`, `'…'`, `[[…]]`), keeping the
    /// quotes, so a pattern like `"/([^%s]+)%s(.*)"` cannot read as a call to `s`.
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
                                // A plain `--` comment runs to end of line. Strings are not
                                // tracked, so a `"--"` in one also cuts the line: this can
                                // hide a call, never invent one.
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

    /// A chain entry that resolved to nothing would leave its globals nil with no error, and a
    /// collision's winner is invisible until an addon calls the wrong body; both are asserted.
    #[test]
    fn a_chain_entry_loads_and_the_later_line_owns_the_collision() {
        let _data = benilla_formats::wow_data_or_skip!();
        let mut s = UiScript::new().expect("VM");
        s.set_screen_size(1024.0, 768.0);

        seat_a_player(&mut s);
        let failures = super::super::manifest::load_default_ui(&s);
        assert!(failures.is_empty(), "the default UI: {failures:#?}");

        // Only the stock `ContainerFrame.lua` defines these.
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
        // Its constants, which addons read directly.
        assert_eq!(s.eval::<i64>("return NUM_BAG_FRAMES").unwrap(), 4);
        assert_eq!(s.eval::<i64>("return NUM_CONTAINER_FRAMES").unwrap(), 12);
        // `PaperDollFrame.lua` arrives through `PaperDollFrame.xml`'s `<Script file=>`: a chain
        // `.xml` brings its `.lua`.
        assert!(s
            .eval::<bool>("return type(PaperDollItemSlotButton_OnLoad) == \"function\"")
            .unwrap());

        // The character sheet runs the stock bodies alone, with no `BenillaPaperDollSlot_OnLoad`.
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

        // Of two definitions of a plain Lua global the later stands; `publish_global`'s
        // non-overwriting rule (`0x701bd0`) applies to frames only.
        s.run("function _order_probe() return 1 end").unwrap();
        s.run("function _order_probe() return 2 end").unwrap();
        assert_eq!(
            s.eval::<i64>("return _order_probe()").unwrap(),
            2,
            "the later definition of a colliding name is the live one"
        );
        assert!(s.errors().is_empty(), "{:#?}", s.errors());
    }

    /// Which unmigrated stock FrameXML window loads clean on top of the shipped manifest, each
    /// asked of the real loader in a fresh VM. Clean means it loads, not that it works: nothing
    /// behind a click or an event runs. A failure in a frame name a file of ours also declares is
    /// suspect: `publish_global` does not overwrite (`0x701bd0`), so `_G` keeps our frame while
    /// the stock handlers write to theirs; re-measure with ours removed. Run with `--ignored
    /// --nocapture`.
    #[test]
    #[ignore = "instrument: run by hand when choosing the next window to migrate"]
    fn chain_readiness_report() {
        let _data = benilla_formats::wow_data_or_skip!();

        // The stock toc's order, off the chain; a file's position there is where its manifest line
        // goes, so the report prints it.
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
                // One line per distinct complaint: a verb twelve frames miss is one fact.
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

    /// What each unmigrated stock window would cost to build. Its calls (the `.xml` and every
    /// `.lua` it sources, comments stripped), minus what it defines and what the loaded interface
    /// has, split against the reference's `_G` (`reference/1.12-globals.tsv`):
    ///
    /// * `engine`: a reference engine binding we lack, the real work.
    /// * `fx`: a FrameXML function, which arrives with the file named beside it; a `<?>` is
    ///   usually a LoadOnDemand `Blizzard_*` addon's, which the chain reads out of `patch.MPQ`.
    /// * `method`: a `:Name(` call no widget of ours answers to, whatever the receiver.
    /// * `LOAD`: what loading the stock file on top of the manifest raised.
    ///
    /// A name in neither half of the reference's `_G` is dropped: 1.12's widget methods are not
    /// globals. A name reached through `getglobal` is invisible. Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "instrument: run by hand when choosing what to build next"]
    fn chain_gap_report() {
        let _data = benilla_formats::wow_data_or_skip!();

        // The reference's `_G`, each name with its origin (`engine`, `framexml`, `lua`).
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

        // What this client answers to with the whole interface up, engine and FrameXML alike.
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

        // `:Name(`: a method call, receiver unknown.
        let called_methods = |text: &str| -> std::collections::HashSet<String> {
            let b: Vec<char> = text.chars().collect();
            let mut out = std::collections::HashSet::new();
            let mut i = 1;
            while i < b.len() {
                // `::` is not a method call.
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

        // Asked of a real widget of each type, not enumerated: a widget's methods come through an
        // `__index` function, so there is no table to walk.
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
        // A broken probe would empty the `method` column silently.
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

        // The `.lua` files a stock `.xml` sources through `<Script file=>`: the window's own code.
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

        // `function Name(` in every stock `.xml` and each `.lua` it sources (`ActionBarFrame.xml`
        // sources `ActionButton.lua`), so an `fx` gap can name the file that holds it.
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
                // A global call: capitalised, not inside a word, and not after `.` (a field call,
                // which arrives with its table) or `:` (the method scan's).
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
            // The window is its `.xml` plus every `.lua` it sources.
            let mut text = String::new();
            let mut parts = vec![f.clone(), format!("{}.lua", &f[..f.len() - 4])];
            parts.extend(sourced_luas(f));
            for cand in parts {
                if let Some(b) = super::read(&format!("Interface\\FrameXML\\{cand}")) {
                    text.push_str(&String::from_utf8_lossy(&b));
                }
            }
            let text = strip_comments(&text);
            // A bare call can name a local: `StaticPopup.lua:1853` reads a dialog's `OnAccept`
            // into one and calls it.
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
                    None => {} // not a global; the `:Name(` scan below sees it
                }
            }
            // A method is never a global, so `own` and `have` do not apply to it. The load pass
            // sees what neither census can, a missing widget type (`<LootButton>`). A window we
            // also ship is suspect there: `publish_global` does not overwrite (`0x701bd0`), so
            // `_G` keeps our frame while the stock handlers write to theirs.
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
            // A suspect LOAD line is not counted as a blocker.
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
        // Unblocked: no missing global, method or load. `fx` names arrive with their own file, so
        // they do not count.
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

    /// Every function a file of ours defines that a chain entry also defines, and which one stands
    /// by load order; every frame name a stock window we do not load shares with a file of ours
    /// (such a pair cannot load side by side); every function of ours whose parameter count
    /// differs from the reference's. Run before attempting a window swap.
    #[test]
    #[ignore = "instrument: run by hand before attempting a window swap"]
    fn shadowed_reference_functions() {
        let _data = benilla_formats::wow_data_or_skip!();

        // Every `name="X"` a document declares, minus the `$parent`-relative ones.
        let declares = |text: &str| -> Vec<String> {
            let mut out = Vec::new();
            for (i, _) in text.match_indices("name=\"") {
                let rest = &text[i + 6..];
                let Some(end) = rest.find('"') else { continue };
                let name = &rest[..end];
                if name.starts_with('$') || name.is_empty() {
                    continue;
                }
                // A template's name is a registry key, not a frame, but two files holding one is
                // the same collision, so templates count too.
                out.push(name.to_string());
            }
            out
        };

        // `function Name(a, b)` to its name and parameter count. Lua never raises on an argument
        // count, so a difference is silent: ours wider breaks when our file goes, ours narrower
        // when a stock caller arrives.
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

        // Every function each chain entry and its same-name `.lua` define.
        let toc = &super::super::addons::Addon::builtin().toc.files;
        // Load order: each manifest entry at its line, ours included (`ours_wins` reads both
        // sides), then each reached addon's files after everything.
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

        // Against everything our files define.
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
        // Frame names and arities from every window in the stock toc, loaded or not: one we do
        // not load is the one whose names can collide.
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

        // The frame-name half, over stock windows the manifest does not already load.
        let mut frame_hits: Vec<(String, String, String)> = Vec::new();
        for entry in toc.iter().filter(|f| !super::is_chain_entry(f)) {
            let Ok(text) = std::fs::read_to_string(dir.join(entry)) else {
                continue;
            };
            for name in declares(&text) {
                if let Some(home) = chain_frames.get(&name) {
                    // The leaf compared whole: a suffix test would take `UIOptionsFrame.xml` for
                    // `OptionsFrame.xml`, a different window.
                    let already = toc
                        .iter()
                        .filter(|f| super::is_chain_entry(f))
                        .any(|f| f.rsplit(['\\', '/']).next() == Some(home.as_str()));
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

        // Grouped by our file, the unit of work.
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

        // The arity half: it bites without both files loading, so it is a section of its own.
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
            // Which swap the difference bites on; both directions are silent.
            let dir = if a > b {
                "ours WIDER  — bites when OUR file goes"
            } else {
                "ours NARROWER — bites when THEIRS arrives"
            };
            println!("{name:<34} {ours:<26} {a:>4} {b:>3}  {dir} — ref in {home}");
        }

        // The frame half, grouped by stock window: can this one be added?
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

    /// Every global a migrated window calls exists once the shipped interface is up. Loading never
    /// runs the body that calls it, so a missing one raises on the first click that reaches it.
    /// Bare `name(` calls in each chain file and its `.lua`, minus its own functions and locals,
    /// count when the reference's `_G` has them (`reference/1.12-globals.tsv`). Blind to names
    /// reached through `getglobal` and to widget methods; it checks existence, not the body.
    #[test]
    fn every_global_a_migrated_window_calls_is_answered() {
        let _data = benilla_formats::wow_data_or_skip!();

        // The reference's `_G`, which filters out locals, fields and widget methods.
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
        // The interface includes every LoadOnDemand addon it reaches: `ActionButton.lua` calls
        // `MacroFrame_SaveMacro`, which `Blizzard_MacroUI.lua` defines once `ShowMacroFrame`
        // loads it.
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

        // Open gaps, each with the window and the click that reach it. The assertion below
        // refuses an entry whose gap has closed.
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
            // `GlobalStrings.lua` only assigns strings; it calls nothing.
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

            // A call is `name(` not after `.` or `:`, in any case: `floor` and `format` are 1.12
            // globals too. Names bound locally (`local X`, `local function X`, a `for` loop's
            // variables) are not globals: `StaticPopup.lua:1853` calls a dialog's `OnAccept`
            // through a local, and `ChatFrame.lua:2170` calls each `SlashCmdList` entry as
            // `value(msg)`.
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

        // The other direction: a `KNOWN` entry whose gap has closed goes with the fix.
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

    /// A chain `.xml` that does not source its own `.lua` needs the `.lua` as a manifest line too,
    /// as the stock toc lists `MoneyInputFrame.lua` and `TextStatusBar.lua` (lines 11 and 32).
    /// Without it every global the file should define reads nil, and nothing errors.
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
    /// Every global function and virtual template a manifest entry declares, including the
    /// functions of each `.lua` a chain `.xml` sources.
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

        // A chain `.xml`'s `<Script file=>`, resolved in the `.xml`'s own folder.
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

    /// The later line wins (a template is a `HashMap::insert`, a function an overwrite), so a
    /// function or template of ours that a later chain entry redeclares is dead while still on
    /// disk. Ours overriding a stock one from a later line is not checked here.
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

    /// An inherited template nothing declares is only a warning: the frame is built bare, with none
    /// of the template's regions, children or `<Scripts>`, so its `<OnLoad>` state stays nil.
    #[test]
    fn every_template_the_manifest_inherits_is_declared_by_the_manifest() {
        let _data = benilla_formats::wow_data_or_skip!();
        let toc = &super::super::addons::Addon::builtin().toc.files;

        let mut declared: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut wanted: Vec<(String, String)> = Vec::new();
        for entry in toc.iter() {
            // `<Include>` counts: the stock toc lists two `*Templates.xml`; the rest arrive with
            // the window that includes them, as `HonorFrame.xml` does `HonorFrameTemplates.xml`.
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

    /// The LoadOnDemand `Blizzard_*` addons the interface reaches: each one a
    /// `UIParentLoadAddOn("…")` literal names in our files or a chain entry's sources
    /// (`UIParent.lua`'s `*_LoadUI` loaders, `UIOptionsFrame.lua`'s combat-text load). Like the
    /// stock toc, the manifest has no row for them.
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
    /// then each reached addon's files in its own toc order.
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
                // `<Include>` brings a sibling document and `<Script file=>` the code, where
                // nearly every `RegisterEvent` lives; both resolve in the entry's own folder.
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

    /// A faux list follows its bar only through `FauxScrollFrame_OnVerticalScroll`
    /// (`UIPanelTemplates.lua:228`), called from the owner's own `<OnVerticalScroll>`; 1.12 has no
    /// `updateFunc` field. Without the handler the bar moves, the list never does, and nothing
    /// errors.
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
                // A virtual template is not a list; each instance declares its own handler.
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
    /// `text` with every whitespace run before a `.` removed, so a chain rustfmt split across lines
    /// reads as one call.
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

    /// Every event a chain file registers is fired by something here or listed in `UNPRODUCED`. An
    /// event name is a plain string at both ends, so a listener nothing feeds is silent; a new one
    /// means a window migration brought it.
    #[test]
    fn every_event_a_chain_file_registers_has_a_producer() {
        let _data = benilla_formats::wow_data_or_skip!();

        // Names built at runtime, which a literal scan cannot see; each entry is a promise that
        // something fires it.
        const CONSTRUCTED: &[&str] = &[
            // `ui_unit.rs` builds `UNIT_{}` and `UNIT_MAX{}` over `power_token`'s five resources.
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

        // Registered by a chain file we load and fired by nothing here; each says why.
        const UNPRODUCED: &[(&str, &str)] = &[
            // ── The stock `UIParent.lua`'s listeners first, then the other files' ──
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
        // The event-name table an indirect fire reads: `"EVENT", vec!`, a match arm
        // `=> "EVENT",`, or a lone `"EVENT"` as a block arm's value.
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
        // An indirect fire names a const or a function, whose literal may live in another file
        // (`ui_chat::event::event_name`, for the fire in `ui_chat::frames`).
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
                    // rustfmt splits a long chain at its dots, so the three shapes are matched
                    // with the whitespace before each `.` removed.
                    let text = glue_chains(&text);
                    // The engine's deferred lane, `pending_events.push((name, args))`, fires too.
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

    /// Seats a player before a probe loads the manifest, as the live client always has one by
    /// then: the stock macro window formats `UnitName("player")` into a tab label in its `OnLoad`,
    /// and without a player that raises like a load failure.
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

    /// Every event benilla fires exists in the 1.12 client: an event it can dispatch is a
    /// NUL-terminated string in `WoW.exe`, and a name missing there no stock listener can ever
    /// receive. Test files are skipped; they fire synthetic names (`E3`) on purpose.
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
                // Split so this file cannot match its own walker.
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
            // A `BENILLA_` prefix marks an event as ours, never mistaken for a 1.12 name.
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
