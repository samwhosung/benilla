//! The `.toc` manifest parser — the load list + metadata of every addon, including Blizzard's own
//! (`FrameXML.toc` and the `Blizzard_*` AddOns are ordinary manifests; third-party addons are the
//! identical mechanism — decision 0068).
//!
//! Grammar, shared by 1.12 and Era manifests (Era only adds directive *keys*, not syntax):
//! - `## Key: Value` — a directive. Keys compare case-insensitively; localized variants carry a
//!   locale suffix (`## Title-deDE: …`). Era manifests list several client builds in one
//!   `## Interface: 11507, 11508`.
//! - `#` (not followed by another `#`) — a comment.
//! - anything else non-blank — a file to load, in order (`.lua`/`.xml`, `\` or `/` separators).
//!
//! The parser is lossless about order (directives keep file order for duplicate keys; files keep
//! load order) and byte-tolerant: UTF-8 BOM, `\r\n`, and stray whitespace are the norm in shipped
//! manifests, not the exception.

/// A parsed `.toc` manifest: ordered directives + the ordered file load list.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Toc {
    /// `## Key: Value` directives in file order, keys as written (compare via [`Toc::directive`]).
    pub directives: Vec<(String, String)>,
    /// Files to load, in order, exactly as written (path separators are normalized at *load* time,
    /// not here — the manifest is quoted verbatim in errors and tooling).
    pub files: Vec<String>,
}

/// Case-insensitive ASCII prefix test — the reference's `SStrCmpI(line, key, SStrLen(key))`.
fn starts_with_ci(s: &str, prefix: &str) -> bool {
    let (s, p) = (s.as_bytes(), prefix.as_bytes());
    s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p)
}

impl Toc {
    /// Parse manifest text. Never fails: unrecognizable lines are file entries by definition of
    /// the grammar, and an empty input is an empty manifest.
    pub fn parse(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut toc = Toc::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(directive) = line.strip_prefix("##") {
                // `##` alone (or without a colon) is treated as a comment, matching the client's
                // tolerance for malformed directives.
                if let Some((key, value)) = directive.split_once(':') {
                    let key = key.trim();
                    if !key.is_empty() {
                        toc.directives
                            .push((key.to_string(), value.trim().to_string()));
                    }
                }
            } else if !line.starts_with('#') {
                toc.files.push(line.to_string());
            }
        }
        toc
    }

    /// The directive whose key matches `key` case-insensitively — **the LAST one, when a manifest
    /// writes the key twice**.
    ///
    /// This read the first for as long as the doc here said "shipped manifests don't duplicate
    /// keys", which two of them do: `MoveAnything.toc` has `## Notes` on lines 3 and 5, and
    /// `AtlasLoot.toc` has `## Author` on lines 6 and 9. The reference hashes each directive into
    /// the metadata map at `[rec+0x98]` (`0x51d580`) and, on an existing entry, **frees the old
    /// value and stores the new** (`0x51d77b`) — a hash insert, so the last line written wins.
    ///
    /// Note the asymmetry with [`Self::list`] one function down, and that both halves were wrong
    /// here in the same direction: a **scalar** replaces, a **list** appends.
    pub fn directive(&self, key: &str) -> Option<&str> {
        self.directives
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// `## Interface:` as client build numbers. Era manifests list several (`11507, 11508`);
    /// 1.12's lists one (`11200`). Unparseable entries are skipped, not errors.
    pub fn interface_versions(&self) -> Vec<u32> {
        self.directive("Interface")
            .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
            .unwrap_or_default()
    }

    /// `## Interface:` **as the 1.12 client reads it** (decision 1292): the leading integer of the
    /// value, and a manifest with no `## Interface` line (or a non-numeric one) is `0`.
    /// `Toc_Parse 0x51c9b0` stores `SStrToInt` of the value at `[rec+0x1c]`, the record ctor
    /// leaves it `0`, and the version gate compares that single dword — so an Era manifest's
    /// `11507, 11508` reads as `11507`, exactly as the reference would read it.
    /// [`Toc::interface_versions`] stays for display; the GATE runs on this.
    pub fn interface_version(&self) -> u32 {
        self.directive("Interface")
            .map(|v| {
                let digits: String = v.trim().chars().take_while(char::is_ascii_digit).collect();
                digits.parse().unwrap_or(0)
            })
            .unwrap_or(0)
    }

    /// A list directive (`SavedVariables`, `OptionalDeps`, `Dependencies`, …), tokenized the way
    /// the reference tokenizes it. Missing directive = empty list.
    ///
    /// **Two departures from the obvious read, both byte-derived, both of which were losing real
    /// data** (decision 2126):
    ///
    /// * **A repeated directive APPENDS.** The reference `strdup`s each item (`0x64a620`) into a
    ///   `0x40`-granular array at `[rec+0x80]` with a running count at `[rec+0x7c]`, so a second
    ///   `## SavedVariables:` line continues the first rather than being ignored. Reading only the
    ///   first line cost `CT_RaidAssist` 18 of
    ///   its 25 saved globals — it declares them across four lines and got seven, so raid
    ///   positions, menu state, boss timers, debuff templates, the loot method and the squelch
    ///   list were never written and never restored.
    /// * **A space separates as surely as a comma.** The tokenizer is `SStrTokenize 0x64ae50` over
    ///   the delimiter SET `" ,"` at `0x8537c4` — readable in the image's own `.rdata`, sitting
    ///   between the `RequiredDep` and `OptionalDep` literals. `SpecialTalentUI` writes
    ///   `## SavedVariables: SpecialTalentPlannedSaved SpecialTalentFrameSaved` and we registered
    ///   one "global" whose name contained a space.
    pub fn list(&self, key: &str) -> Vec<&str> {
        self.list_where(|k| k.eq_ignore_ascii_case(key))
    }

    /// [`Self::list`] over every directive whose key satisfies `pick`, in file order.
    fn list_where(&self, mut pick: impl FnMut(&str) -> bool) -> Vec<&str> {
        self.directives
            .iter()
            .filter(|(k, _)| pick(k))
            .flat_map(|(_, v)| v.split([',', ' ']).map(str::trim))
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Hard dependencies — **every directive whose key STARTS WITH `Dep` or `RequiredDep`**.
    ///
    /// The three dependency keys are the only ones in the reference's directive table that carry
    /// **no colon**: the `.rdata` cluster reads
    /// `SavedVariablesPerCharacter:\0 SavedVariables:\0 LoadWith:\0 \0\0 Dep\0 RequiredDep\0 " ,"\0\0 OptionalDep\0 Revision:\0`,
    /// and a whole-key compare against `"Dep"` could never match a line reading `Dependencies:`.
    /// So the compare is a PREFIX compare — `SStrCmpI(line, key, SStrLen(key))` at
    /// `0x51cd4e`-`0x51cd5f` — and `## Dependencies`, `## RequiredDependencies` and
    /// `## Dependency` all land here (decision 2126).
    ///
    /// Both keys feed the SAME array in the reference (`[rec+0x50]`), so a manifest writing both
    /// gets their union rather than whichever this function happened to test first.
    pub fn dependencies(&self) -> Vec<&str> {
        self.list_where(|k| starts_with_ci(k, "RequiredDep") || starts_with_ci(k, "Dep"))
    }

    /// Soft dependencies — every directive whose key starts with `OptionalDep`, on the same
    /// prefix rule as [`Self::dependencies`].
    ///
    /// **They load FIRST and their failures are ignored** — `AddOn_Load 0x51f240`'s own order,
    /// quoted in 1191 §2 — which is what lets `## OptionalDeps: FuBar, Ace2` mean "if Ace2 is
    /// installed, its libraries are already global by the time my files run". **130 corpus addons
    /// declare them**, and the whole FuBar family leans on exactly that: `FuBar_BagFu`'s `.toc`
    /// lists `FuBarPlugin-2.0.lua` BEFORE `AceLibrary.lua`, which only works because the `Ace2`
    /// addon went first.
    ///
    /// The long form `## OptionalDependencies:` — which 4 corpus addons write — used to be
    /// refused here, on the reasoning that it was "verified for the required half and merely
    /// plausible for this one, left open". The premise was wrong: the reference
    /// matches all three dependency keys by the same colon-less prefix, so the long form was never
    /// a separate question.
    pub fn optional_dependencies(&self) -> Vec<&str> {
        self.list_where(|k| starts_with_ci(k, "OptionalDep"))
    }

    /// `## LoadOnDemand: 1` (Era loader; absent in 1.12 manifests).
    pub fn load_on_demand(&self) -> bool {
        self.directive("LoadOnDemand").map(str::trim) == Some("1")
    }

    /// `## DefaultState:` — what this addon's enable state is for a character who has never
    /// expressed one. The reference's `[rec+0x2b]`, stored by `Toc_Parse` at `0x51d204` from the
    /// two literals `"enabled"` (`0x853764`) → `1` and `"disabled"` (`0x853758`) → `0`.
    ///
    /// It is load-bearing well beyond a manifest that writes it: the enable query `0x51e470`
    /// falls back to this byte whenever the characters disagree, and whenever *none* of them has
    /// an opinion at all — which is every addon on a fresh install.
    ///
    /// **A manifest that does not write the line is enabled** — which is what makes a folder
    /// dropped into `AddOns/` just work, and is the record's initial byte, read at the bytes and
    /// not inferred from the two literals' fall-through (decision 2316). The ctor `0x520550` seeds
    /// this one field to 1 explicitly — `0x5205b9 mov [esi+0x2b],al` with `eax = 1`, where its five
    /// neighbours take `bl = 0` — over an allocation that does zero-fill, which is exactly what
    /// made "the ctor zeroes it" read as true.
    ///
    /// Two consequences this function depends on, both verified: a value matching **neither**
    /// literal leaves the 1 (`0x51d21a jne`, no store — there is no "unrecognised means disabled"),
    /// and a duplicated directive is **last-wins**, which is [`Self::directive`]'s own rule.
    pub fn default_state(&self) -> bool {
        !self
            .directive("DefaultState")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("disabled"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_directives_files_and_comments() {
        let toc = Toc::parse(
            "\u{feff}## Interface: 11507, 11508\r\n\
             ## Title: Probe |cff00ff00Addon|r\r\n\
             ## SavedVariables: ProbeDB, ProbeSettings\r\n\
             ## X-Custom: anything: with colons\r\n\
             # a comment line\r\n\
             ##\r\n\
             ## malformed directive without colon\r\n\
             \r\n\
             Libs\\LibStub\\LibStub.lua\r\n\
             Core.lua\r\n\
             Frames.xml\r\n",
        );
        assert_eq!(toc.interface_versions(), vec![11507, 11508]);
        assert_eq!(toc.directive("title"), Some("Probe |cff00ff00Addon|r"));
        assert_eq!(toc.list("SavedVariables"), vec!["ProbeDB", "ProbeSettings"]);
        // Values keep everything after the first colon.
        assert_eq!(toc.directive("X-Custom"), Some("anything: with colons"));
        assert_eq!(
            toc.files,
            vec!["Libs\\LibStub\\LibStub.lua", "Core.lua", "Frames.xml"]
        );
        // Comments and malformed directives contribute nothing.
        assert_eq!(toc.directives.len(), 4);
    }

    /// **`## DefaultState:` and its three non-obvious answers**: an absent line is *enabled*
    /// because the record's ctor seeds `[rec+0x2b]` to 1, a value matching neither literal leaves
    /// that 1 rather than meaning disabled, and a duplicated line is last-wins.
    ///
    /// Only `disabled` disables, in other words — which is the same asymmetry `AddOns.txt`'s own
    /// value grammar has, and worth a falsifier because "unrecognised means off" is the reading
    /// anyone would write by hand.
    #[test]
    fn default_state_is_enabled_unless_the_manifest_says_disabled() {
        let d = |body: &str| Toc::parse(body).default_state();
        assert!(d("## Interface: 11200\n"), "absent: the ctor's own 1");
        assert!(d("## DefaultState: enabled\n"));
        assert!(!d("## DefaultState: disabled\n"));
        assert!(
            !d("## DefaultState:    DISABLED   \n"),
            "trimmed, case-folded"
        );
        assert!(
            d("## DefaultState: maybe\n"),
            "neither literal: no store, the 1 stands"
        );
        assert!(
            d("## DefaultState:\n"),
            "empty value is not `disabled` either"
        );
        // Last-wins, the reference's hash insert — not first-wins, and not an append.
        assert!(!d("## DefaultState: enabled\n## DefaultState: disabled\n"));
        assert!(d("## DefaultState: disabled\n## DefaultState: enabled\n"));
    }

    #[test]
    fn vanilla_1_12_shape() {
        let toc = Toc::parse(
            "## Interface: 11200\n\
             ## Title: FrameXML\n\
             ## Secure: 1\n\
             GlobalStrings.lua\n\
             Fonts.xml\n",
        );
        assert_eq!(toc.interface_versions(), vec![11200]);
        assert_eq!(toc.directive("Secure"), Some("1"));
        assert!(!toc.load_on_demand());
        assert_eq!(toc.files.len(), 2);
    }

    /// **A scalar directive REPLACES on a repeat; a list directive APPENDS** — the reference's two
    /// stores, and this file had both backwards (decision 2126). `MoveAnything.toc` writes
    /// `## Notes` twice and `AtlasLoot.toc` writes `## Author` twice, so the scalar half is not
    /// hypothetical either.
    #[test]
    fn a_scalar_directive_replaces_and_a_list_directive_appends() {
        let toc = Toc::parse(
            "## RequiredDeps: LibA, LibB\n\
             ## Title: first\n\
             ## Title: second\n\
             ## LoadOnDemand: 1\n",
        );
        assert_eq!(toc.dependencies(), vec!["LibA", "LibB"]);
        assert_eq!(toc.directive("Title"), Some("second"));
        assert!(toc.load_on_demand());
    }

    /// **`CT_RaidAssist`'s own manifest, in shape** — 25 saved globals across four lines, of which
    /// we registered the first seven. The rest (raid positions, menu state, boss timers, debuff
    /// templates, the loot method, the squelch list) were never written and never restored.
    ///
    /// Plus `SpecialTalentUI`'s: two names separated by a SPACE, which we read as one global whose
    /// name contained a space. The tokenizer is `SStrTokenize 0x64ae50` over the delimiter set
    /// `" ,"` at `0x8537c4`.
    #[test]
    fn a_repeated_list_directive_accumulates_across_lines_and_splits_on_space() {
        let toc = Toc::parse(
            "## SavedVariables: A, B\n\
             ## SavedVariables: C, D\n\
             ## SavedVariablesPerCharacter: E F\n",
        );
        assert_eq!(toc.list("SavedVariables"), vec!["A", "B", "C", "D"]);
        assert_eq!(toc.list("SavedVariablesPerCharacter"), vec!["E", "F"]);
    }

    /// **The three dependency keys are matched by PREFIX**, because they are the only directives in
    /// the reference's table with no colon: the `.rdata` cluster is
    /// `… LoadWith:\0 \0\0 Dep\0 RequiredDep\0 " ,"\0\0 OptionalDep\0 Revision:\0 …`, and a
    /// whole-key compare against `"Dep"` could never match a line reading `Dependencies:`.
    ///
    /// Four corpus addons write the long optional form (`FuBar_CRDelayFu`, `FuBar_PetInFu`,
    /// `FuBar_ToFu`, `FuBar_WeaponRebuffFu`), which this file used to refuse on the reasoning that
    /// the long form was "merely plausible" for the optional half. It was never a separate
    /// question.
    #[test]
    fn the_dependency_keys_match_by_prefix_and_both_required_spellings_union() {
        let toc = Toc::parse(
            "## Dependencies: LibA\n\
             ## RequiredDependencies: LibB\n\
             ## OptionalDependencies: LibC, LibD\n\
             ## OptionalDeps: LibE\n",
        );
        assert_eq!(toc.dependencies(), vec!["LibA", "LibB"]);
        assert_eq!(
            toc.optional_dependencies(),
            vec!["LibC", "LibD", "LibE"],
            "the long and short optional spellings are the same key"
        );
        // And the prefixes do not bleed into each other: `OptionalDependencies` does not start
        // with `Dep`, so it never lands in the required list.
        assert!(!toc.dependencies().contains(&"LibC"));
    }

    #[test]
    fn empty_and_degenerate_inputs() {
        assert_eq!(Toc::parse(""), Toc::default());
        let toc = Toc::parse("# only a comment\n\n");
        assert!(toc.directives.is_empty() && toc.files.is_empty());
    }
}
