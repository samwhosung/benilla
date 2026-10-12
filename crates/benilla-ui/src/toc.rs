//! The `.toc` manifest parser, for FrameXML, the `Blizzard_*` addons and third-party ones alike:
//! `## Key: Value` is a directive, any other `#` line a comment, and any other non-blank line a
//! file to load, in order. A BOM, `\r\n` and stray whitespace are tolerated.
//!
//! The reference reads a directive by testing the line, after the spaces that follow `##`, against
//! each directive's own literal in turn (`Toc_Parse 0x51c9b0`, tests `0x51cc69`-`0x51d295`),
//! case-insensitively and as a prefix of the line, and the first literal that matches claims it. A
//! literal that ends in a colon (`LoadWith:`) needs the colon right after the key, so
//! `## LoadWith : X` is not that directive; one that does not (`Dep`) is a bare prefix of the key.
//! That table is [`Kind`], and every typed read here goes through it.

use crate::strings::string_to_bool;

/// A parsed `.toc` manifest.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Toc {
    /// Every `## Key: Value` in file order.
    pub directives: Vec<Directive>,
    /// Files to load, in order, as written; separators are normalized at load time.
    pub files: Vec<String>,
}

/// One `## Key: Value` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    /// The key with the spaces around it trimmed: the metadata key `0x51d580` stores
    /// (`GetAddOnMetadata`), where it trims the spaces before the colon (`0x51d600`).
    pub key: String,
    /// Everything after the first colon, trimmed.
    pub value: String,
    /// The line from after the spaces following `##` up to its first colon, as written: what the
    /// literals in [`Kind`] are compared against.
    head: String,
}

/// A directive the reference reads, with the literal its reader tests (`SStrCmpI` with the
/// literal's own `SStrLen`) and that literal's address. Variants are in the order the reader tests
/// them, so the first match claims a line: any other directive is dropped (`0x51d3d0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `Interface:` `0x8537fc` (test `0x51cc69`): the number at `[rec+0x1c]`.
    Interface,
    /// `Title` `0x853838` (`0x51cca6`): stored by the metadata map `0x51d580`.
    Title,
    /// `Notes` `0x8537f4` (`0x51ccbb`): metadata.
    Notes,
    /// `Author` `0x8537ec` (`0x51ccd0`): metadata.
    Author,
    /// `Version` `0x8537e4` (`0x51cce5`): metadata.
    Version,
    /// `X-` `0x8537e0` (`0x51ccfa`): metadata.
    Extra,
    /// `Revision:` `0x8537d4` (`0x51cd18`): the number at `[rec+0x20]`.
    Revision,
    /// `OptionalDep` `0x8537c8` (`0x51cd5f`): the list at `[rec+0x40]`.
    OptionalDep,
    /// `RequiredDep` `0x8537b8` (`0x51ce6e`): the list at `[rec+0x50]`.
    RequiredDep,
    /// `Dep` `0x8537b4` (`0x51ce8c`): the same list as [`Kind::RequiredDep`].
    Dep,
    /// `LoadWith:` `0x8537a8` (`0x51ceaa`): the list at `[rec+0x60]`.
    LoadWith,
    /// `SavedVariables:` `0x853798` (`0x51cfbe`): the list at `[rec+0x70]`.
    SavedVariables,
    /// `SavedVariablesPerCharacter:` `0x85377c` (`0x51d0be`): the list at `[rec+0x80]`.
    SavedVariablesPerCharacter,
    /// `DefaultState:` `0x85376c` (`0x51d1be`): `enabled` `0x853764` or `disabled` `0x853758`
    /// into `[rec+0x2b]`.
    DefaultState,
    /// `LoadOnDemand:` `0x853748` (`0x51d23c`): `0x6f1b30` into `[rec+0x2c]`.
    LoadOnDemand,
    /// `Secure:` `0x853740` (`0x51d295`): `0x6f1b30` into `[rec+0x28]`.
    Secure,
}

impl Kind {
    /// In the reader's order.
    const ALL: [Kind; 16] = [
        Kind::Interface,
        Kind::Title,
        Kind::Notes,
        Kind::Author,
        Kind::Version,
        Kind::Extra,
        Kind::Revision,
        Kind::OptionalDep,
        Kind::RequiredDep,
        Kind::Dep,
        Kind::LoadWith,
        Kind::SavedVariables,
        Kind::SavedVariablesPerCharacter,
        Kind::DefaultState,
        Kind::LoadOnDemand,
        Kind::Secure,
    ];

    /// The literal the reader compares, from the client's `.data`.
    pub const fn literal(self) -> &'static str {
        match self {
            Kind::Interface => "Interface:",
            Kind::Title => "Title",
            Kind::Notes => "Notes",
            Kind::Author => "Author",
            Kind::Version => "Version",
            Kind::Extra => "X-",
            Kind::Revision => "Revision:",
            Kind::OptionalDep => "OptionalDep",
            Kind::RequiredDep => "RequiredDep",
            Kind::Dep => "Dep",
            Kind::LoadWith => "LoadWith:",
            Kind::SavedVariables => "SavedVariables:",
            Kind::SavedVariablesPerCharacter => "SavedVariablesPerCharacter:",
            Kind::DefaultState => "DefaultState:",
            Kind::LoadOnDemand => "LoadOnDemand:",
            Kind::Secure => "Secure:",
        }
    }
}

/// Case-insensitive ASCII prefix test, the reference's `SStrCmpI(line, key, SStrLen(key))`.
fn starts_with_ci(s: &str, prefix: &str) -> bool {
    let (s, p) = (s.as_bytes(), prefix.as_bytes());
    s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p)
}

impl Directive {
    /// Whether the reader claims this line for `kind`: the first literal in [`Kind::ALL`] order
    /// that prefixes the line. The line is the head and its colon, which is all a literal can
    /// reach but a list directive's value, and no literal does.
    fn kind(&self) -> Option<Kind> {
        let line = format!("{}:", self.head);
        Kind::ALL
            .into_iter()
            .find(|k| starts_with_ci(&line, k.literal()))
    }

    /// The value as a scalar directive reads it: past any run of colons and spaces
    /// (`0x51d260`-`0x51d26b`).
    fn scalar(&self) -> &str {
        self.value.trim_start_matches([':', ' '])
    }
}

impl Toc {
    /// Parse manifest text; never fails, since any unrecognized line is a file entry.
    pub fn parse(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut toc = Toc::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(directive) = line.strip_prefix("##") {
                // `##` with no colon is a comment, as the client tolerates it.
                if let Some((head, value)) = directive.trim_start_matches(' ').split_once(':') {
                    let key = head.trim();
                    if !key.is_empty() {
                        toc.directives.push(Directive {
                            key: key.to_string(),
                            value: value.trim().to_string(),
                            head: head.to_string(),
                        });
                    }
                }
            } else if !line.starts_with('#') {
                toc.files.push(line.to_string());
            }
        }
        toc
    }

    /// The metadata key `key` (`GetAddOnMetadata`, `Title`, `Notes`), case-insensitively, the last
    /// one when a key repeats: the reference's hash insert into `[rec+0x98]` (`0x51d580`) replaces
    /// the old value (`0x51d77b`). Not for the typed directives, which [`Kind`] reads.
    pub fn directive(&self, key: &str) -> Option<&str> {
        self.directives
            .iter()
            .rev()
            .find(|d| d.key.eq_ignore_ascii_case(key))
            .map(|d| d.value.as_str())
    }

    /// What `GetAddOnMetadata` can answer: the lines of the kinds that reach the metadata map
    /// (`0x51d580`, called from `0x51d3c6`) -- `Title`, `Notes`, `Author`, `Version` and `X-*` --
    /// as `(key, value)` in file order. The typed directives jump to `0x51d3d0` without it, so
    /// `GetAddOnMetadata(addon, "Interface")` is nil. The value is read past any colons and
    /// spaces (`0x51d610`).
    pub fn metadata(&self) -> Vec<(String, String)> {
        let kept = [
            Kind::Title,
            Kind::Notes,
            Kind::Author,
            Kind::Version,
            Kind::Extra,
        ];
        self.directives
            .iter()
            .filter(|d| d.kind().is_some_and(|k| kept.contains(&k)))
            .map(|d| (d.key.clone(), d.scalar().to_string()))
            .collect()
    }

    /// The directives the reader claims for `kind`, in file order.
    fn of(&self, kind: Kind) -> impl Iterator<Item = &Directive> {
        self.directives
            .iter()
            .filter(move |d| d.kind() == Some(kind))
    }

    /// A scalar directive's value, the last line winning.
    fn scalar(&self, kind: Kind) -> Option<&str> {
        self.of(kind).last().map(Directive::scalar)
    }

    /// A list directive's names, every line appending, a space separating like a comma
    /// (`SStrTokenize 0x64ae50`, delimiters `" ,"` at `0x8537c4`).
    fn list(&self, kinds: &[Kind]) -> Vec<&str> {
        self.directives
            .iter()
            .filter(|d| d.kind().is_some_and(|k| kinds.contains(&k)))
            .flat_map(|d| d.value.split([',', ' ']).map(str::trim))
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// `## Interface:` as build numbers, for display; Era manifests list several (`11507, 11508`).
    pub fn interface_versions(&self) -> Vec<u32> {
        self.scalar(Kind::Interface)
            .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
            .unwrap_or_default()
    }

    /// `## Interface:` as the version gate reads it: `Toc_Parse 0x51c9b0` stores `SStrToInt` of
    /// the value at `[rec+0x1c]` (else the ctor's `0`), so `11507, 11508` reads `11507`.
    pub fn interface_version(&self) -> u32 {
        self.scalar(Kind::Interface)
            .map(|v| {
                let digits: String = v.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().unwrap_or(0)
            })
            .unwrap_or(0)
    }

    /// `## SavedVariables:`: the account-wide globals.
    pub fn saved_variables(&self) -> Vec<&str> {
        self.list(&[Kind::SavedVariables])
    }

    /// `## SavedVariablesPerCharacter:`.
    pub fn saved_variables_per_character(&self) -> Vec<&str> {
        self.list(&[Kind::SavedVariablesPerCharacter])
    }

    /// Hard dependencies: the lines whose key starts with `Dep` or `RequiredDep`, which the
    /// reader stores in one array at `[rec+0x50]` (`0x51ce6e`, `0x51ce8c`), so `## Dependencies`
    /// and `## RequiredDependencies` both land there.
    pub fn dependencies(&self) -> Vec<&str> {
        self.list(&[Kind::RequiredDep, Kind::Dep])
    }

    /// Soft dependencies: every key that starts with `OptionalDep`, the long form included.
    /// `AddOn_Load 0x51f240` loads them first and ignores their failures.
    pub fn optional_dependencies(&self) -> Vec<&str> {
        self.list(&[Kind::OptionalDep])
    }

    /// `## LoadWith:`, the addons whose load loads this one (`0x51ceaa`), into the array at
    /// `[rec+0x60]`.
    pub fn load_with(&self) -> Vec<&str> {
        self.list(&[Kind::LoadWith])
    }

    /// `## LoadOnDemand:`, which stock 1.12 addons such as `Blizzard_TalentUI` carry as `1`. The
    /// value goes through `0x6f1b30` (called at `0x51d26f`, stored at `0x51d27c`), so `true`,
    /// `yes`, `on` and `enabled` count as well; the last line wins.
    pub fn load_on_demand(&self) -> bool {
        self.scalar(Kind::LoadOnDemand).is_some_and(string_to_bool)
    }

    /// `## Secure:`, the byte at `[rec+0x28]` (`0x51d2cc`), also through `0x6f1b30` (`0x51d2bf`).
    /// It grants nothing by itself: the client sends every such addon in `CMSG_AUTH_SESSION`'s
    /// addon block, and only the server's answer makes one SECURE.
    pub fn secure(&self) -> bool {
        self.scalar(Kind::Secure).is_some_and(string_to_bool)
    }

    /// `## DefaultState:`, the byte at `[rec+0x2b]` the enable query `0x51e470` falls back to when
    /// characters disagree or none has chosen. `Toc_Parse` stores `"enabled"` (`0x853764`) as 1 and
    /// `"disabled"` (`0x853758`) as 0 at `0x51d204` and `0x51d223`, line by line, and any other
    /// value not at all (`0x51d21a`); the ctor `0x520550` seeds 1 (`0x5205b9`).
    pub fn default_state(&self) -> bool {
        let mut on = true;
        for d in self.of(Kind::DefaultState) {
            let v = d.scalar().trim();
            if v.eq_ignore_ascii_case("enabled") {
                on = true;
            } else if v.eq_ignore_ascii_case("disabled") {
                on = false;
            }
        }
        on
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
        assert_eq!(toc.saved_variables(), vec!["ProbeDB", "ProbeSettings"]);
        // Values keep everything after the first colon.
        assert_eq!(toc.directive("X-Custom"), Some("anything: with colons"));
        assert_eq!(
            toc.files,
            vec!["Libs\\LibStub\\LibStub.lua", "Core.lua", "Frames.xml"]
        );
        // Comments and malformed directives contribute nothing.
        assert_eq!(toc.directives.len(), 4);
    }

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
        // Last-wins, the reference's hash insert.
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

    #[test]
    fn a_repeated_list_directive_accumulates_across_lines_and_splits_on_space() {
        let toc = Toc::parse(
            "## SavedVariables: A, B\n\
             ## SavedVariables: C, D\n\
             ## SavedVariablesPerCharacter: E F\n",
        );
        assert_eq!(toc.saved_variables(), vec!["A", "B", "C", "D"]);
        assert_eq!(toc.saved_variables_per_character(), vec!["E", "F"]);
    }

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
        // `OptionalDependencies` does not start with `Dep`.
        assert!(!toc.dependencies().contains(&"LibC"));
    }

    /// Every literal the reader tests (`0x51cc69`-`0x51d295`): one ending in a colon needs it right
    /// after the key, so a space before the colon drops the line; one that does not is a bare
    /// prefix, case-insensitive, and the colon may sit anywhere after it.
    #[test]
    fn each_directive_is_read_by_its_own_literal() {
        for kind in Kind::ALL {
            let lit = kind.literal();
            let kind_of = |line: &str| Toc::parse(line).directives[0].kind();
            let name = lit.trim_end_matches(':');
            let colon = if lit.ends_with(':') { "" } else { ":" };
            assert_eq!(kind_of(&format!("## {lit}{colon} v")), Some(kind), "{lit}");
            assert_eq!(
                kind_of(&format!(
                    "##   {}: v",
                    lit.to_ascii_uppercase().trim_end_matches(':')
                )),
                Some(kind),
                "{lit}: case and the spaces after ## do not matter"
            );
            if lit.ends_with(':') {
                assert_eq!(
                    kind_of(&format!("## {name} : v")),
                    None,
                    "{lit} needs its colon"
                );
                assert_eq!(
                    kind_of(&format!("## {name}s: v")),
                    None,
                    "{lit} is not a prefix"
                );
            } else {
                assert_eq!(kind_of(&format!("## {name} : v")), Some(kind), "{lit}");
                assert_eq!(kind_of(&format!("## {name}Suffix: v")), Some(kind), "{lit}");
            }
        }
        // The first literal that matches claims the line.
        let first = |line: &str| Toc::parse(line).directives[0].kind();
        assert_eq!(first("## Dependencies: A"), Some(Kind::Dep));
        assert_eq!(first("## RequiredDeps: A"), Some(Kind::RequiredDep));
        assert_eq!(first("## OptionalDeps: A"), Some(Kind::OptionalDep));
        assert_eq!(first("## DefaultState: x"), Some(Kind::DefaultState));
        assert_eq!(
            first("## SavedVariablesPerCharacter: A"),
            Some(Kind::SavedVariablesPerCharacter)
        );
        assert_eq!(first("## X-Website: x"), Some(Kind::Extra));
        assert_eq!(
            first("## Includes: x"),
            None,
            "any other directive is dropped"
        );
    }

    /// The same rule through each typed read, for a line with a space before its colon.
    #[test]
    fn a_colon_literal_directive_with_a_gap_reads_as_absent() {
        let toc = Toc::parse(
            "## Interface : 11200\n\
             ## LoadWith : A\n\
             ## SavedVariables : B\n\
             ## SavedVariablesPerCharacter : C\n\
             ## LoadOnDemand : 1\n\
             ## Secure : 1\n\
             ## DefaultState : disabled\n",
        );
        assert_eq!(toc.interface_version(), 0);
        assert!(toc.load_with().is_empty() && toc.saved_variables().is_empty());
        assert!(toc.saved_variables_per_character().is_empty());
        assert!(!toc.load_on_demand() && !toc.secure());
        assert!(toc.default_state(), "the ctor's 1 stands");
        // A prefix literal, and a metadata key, take the gap in stride.
        let toc = Toc::parse("## Dependencies : A\n## Title : T\n## Version : 1\n");
        assert_eq!(toc.dependencies(), vec!["A"]);
        assert_eq!(toc.directive("Title"), Some("T"));
    }

    #[test]
    fn a_scalar_skips_colons_and_spaces_before_its_value() {
        let toc = Toc::parse("## Interface:: 11200\n## LoadOnDemand:: : on\n## Secure:: Y\n");
        assert_eq!(toc.interface_version(), 11200);
        assert!(toc.load_on_demand() && toc.secure());
    }

    #[test]
    fn secure_reads_the_clients_string_to_bool() {
        for yes in ["1", "true", "Yes", "on", "enabled"] {
            assert!(Toc::parse(&format!("## Secure: {yes}\n")).secure(), "{yes}");
        }
        for no in ["0", "false", "off", "", "secure"] {
            assert!(
                !Toc::parse(&format!("## Secure: {no}\n")).secure(),
                "{no:?}"
            );
        }
        assert!(!Toc::parse("## Title: x\n").secure());
    }

    #[test]
    fn load_with_is_a_list_of_its_own_key() {
        let toc = Toc::parse(
            "## LoadWith: KLHThreatMeter, Other\n\
             ## loadwith: Third\n\
             ## LoadWithAll: Nope\n\
             ## Dependencies: LibA\n",
        );
        assert_eq!(toc.load_with(), vec!["KLHThreatMeter", "Other", "Third"]);
        assert_eq!(toc.dependencies(), vec!["LibA"]);
    }

    /// `0x6f1b30`: the first byte decides for `0`-`9`, `F`, `N`, `T`, `Y` and their lowercase; the
    /// other words are compared whole.
    #[test]
    fn load_on_demand_reads_the_clients_string_to_bool() {
        let lod = |v: &str| Toc::parse(&format!("## LoadOnDemand: {v}\n")).load_on_demand();
        for yes in [
            "1", "9", "true", "True", "Yes", "yes", "T", "on", "ON", "enabled", "Enabled",
        ] {
            assert!(lod(yes), "{yes} is true");
        }
        for no in [
            "0", "false", "no", "No", "off", "disabled", "", "maybe", "onn", "enable", "x",
        ] {
            assert!(!lod(no), "{no:?} is false");
        }
        assert!(
            !Toc::parse("## Title: x\n").load_on_demand(),
            "absent is false"
        );
    }

    /// Only the lines the reader hands the metadata map (`0x51d580`) are metadata.
    #[test]
    fn metadata_is_what_the_reader_stores_in_its_map() {
        let toc = Toc::parse(
            "## Interface: 11200\n## Title : T\n## Notes: N\n## Author: A\n## Version: 1\n\
             ## X-Website:: http://x\n## SavedVariables: V\n## Dependencies: D\n## Includes: I\n",
        );
        let keys: Vec<(String, String)> = toc.metadata();
        assert_eq!(
            keys,
            vec![
                ("Title".into(), "T".into()),
                ("Notes".into(), "N".into()),
                ("Author".into(), "A".into()),
                ("Version".into(), "1".into()),
                ("X-Website".into(), "http://x".into()),
            ]
        );
    }

    #[test]
    fn empty_and_degenerate_inputs() {
        assert_eq!(Toc::parse(""), Toc::default());
        let toc = Toc::parse("# only a comment\n\n");
        assert!(toc.directives.is_empty() && toc.files.is_empty());
    }
}
