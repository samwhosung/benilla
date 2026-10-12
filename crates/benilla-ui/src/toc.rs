//! The `.toc` manifest parser, for FrameXML, the `Blizzard_*` addons and third-party ones alike:
//! `## Key: Value` is a directive, any other `#` line a comment, and any other non-blank line a
//! file to load, in order. A BOM, `\r\n` and stray whitespace are tolerated.

/// A parsed `.toc` manifest.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Toc {
    /// `## Key: Value` pairs in file order, keys as written.
    pub directives: Vec<(String, String)>,
    /// Files to load, in order, as written; separators are normalized at load time.
    pub files: Vec<String>,
    /// Indices into [`Self::directives`] whose key had whitespace before its colon
    /// (`## LoadWith : X`). A key the reference compares with its colon (`LoadWith:`, `0x8537a8`)
    /// does not match such a line.
    gapped: Vec<usize>,
}

/// Case-insensitive ASCII prefix test, the reference's `SStrCmpI(line, key, SStrLen(key))`.
fn starts_with_ci(s: &str, prefix: &str) -> bool {
    let (s, p) = (s.as_bytes(), prefix.as_bytes());
    s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p)
}

/// The client's string-to-bool `0x6f1b30` for the words it knows, `None` for any other string. A
/// first byte of `1`-`9`, `T`, `t`, `Y` or `y` is true and `0`, `F`, `f`, `N` or `n` false (the
/// jump table `0x6f1bc0` over `0x6f1bb4`); every other string, an empty one included, is compared
/// whole and case-insensitively (`0x64a4c0`): `off` (`0x871460`) and `disabled` (`0x853758`) are
/// false, `on` (`0x87145c`) and `enabled` (`0x853764`) true.
pub(crate) fn keyword_bool(bytes: &[u8]) -> Option<bool> {
    match bytes.first() {
        Some(b'0' | b'F' | b'N' | b'f' | b'n') => Some(false),
        Some(b'1'..=b'9' | b'T' | b'Y' | b't' | b'y') => Some(true),
        _ if bytes.eq_ignore_ascii_case(b"off") || bytes.eq_ignore_ascii_case(b"disabled") => {
            Some(false)
        }
        _ if bytes.eq_ignore_ascii_case(b"on") || bytes.eq_ignore_ascii_case(b"enabled") => {
            Some(true)
        }
        _ => None,
    }
}

/// `0x6f1b30` as a `.toc` boolean reads it: anything [`keyword_bool`] does not know is false.
pub fn string_to_bool(s: &str) -> bool {
    keyword_bool(s.as_bytes()).unwrap_or(false)
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
                if let Some((key, value)) = directive.split_once(':') {
                    let trimmed = key.trim();
                    if !trimmed.is_empty() {
                        if key.trim_end().len() != key.len() {
                            toc.gapped.push(toc.directives.len());
                        }
                        toc.directives
                            .push((trimmed.to_string(), value.trim().to_string()));
                    }
                }
            } else if !line.starts_with('#') {
                toc.files.push(line.to_string());
            }
        }
        toc
    }

    /// The directive matching `key` case-insensitively, the last one when a key repeats: the
    /// reference's hash insert into `[rec+0x98]` (`0x51d580`) replaces the old value (`0x51d77b`).
    pub fn directive(&self, key: &str) -> Option<&str> {
        self.directives
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// `## Interface:` as build numbers, for display; Era manifests list several (`11507, 11508`).
    pub fn interface_versions(&self) -> Vec<u32> {
        self.directive("Interface")
            .map(|v| v.split(',').filter_map(|n| n.trim().parse().ok()).collect())
            .unwrap_or_default()
    }

    /// `## Interface:` as the version gate reads it: `Toc_Parse 0x51c9b0` stores `SStrToInt` of
    /// the value at `[rec+0x1c]` (else the ctor's `0`), so `11507, 11508` reads `11507`.
    pub fn interface_version(&self) -> u32 {
        self.directive("Interface")
            .map(|v| {
                let digits: String = v.trim().chars().take_while(char::is_ascii_digit).collect();
                digits.parse().unwrap_or(0)
            })
            .unwrap_or(0)
    }

    /// A list directive (`SavedVariables`, `OptionalDeps`, …) as the reference tokenizes it: a
    /// repeated line appends (`0x64a620` into the array at `[rec+0x80]`, count at `[rec+0x7c]`),
    /// and a space separates like a comma (`SStrTokenize 0x64ae50`, delimiters `" ,"` at
    /// `0x8537c4`).
    pub fn list(&self, key: &str) -> Vec<&str> {
        self.list_where(|k| k.eq_ignore_ascii_case(key))
    }

    fn list_where(&self, mut pick: impl FnMut(&str) -> bool) -> Vec<&str> {
        self.directives
            .iter()
            .filter(|(k, _)| pick(k))
            .flat_map(|(_, v)| v.split([',', ' ']).map(str::trim))
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Hard dependencies: every directive whose key starts with `Dep` or `RequiredDep`. The
    /// reference stores these keys without a colon and prefix-compares them
    /// (`0x51cd4e`-`0x51cd5f`), so `## Dependencies` and `## RequiredDependencies` both land in
    /// its one array at `[rec+0x50]`.
    pub fn dependencies(&self) -> Vec<&str> {
        self.list_where(|k| starts_with_ci(k, "RequiredDep") || starts_with_ci(k, "Dep"))
    }

    /// Soft dependencies: every directive whose key starts with `OptionalDep`, the long form
    /// included. `AddOn_Load 0x51f240` loads them first and ignores their failures.
    pub fn optional_dependencies(&self) -> Vec<&str> {
        self.list_where(|k| starts_with_ci(k, "OptionalDep"))
    }

    /// `## LoadWith:`, the addons whose load loads this one. `Toc_Parse` compares the 9 bytes
    /// `LoadWith:` (`0x8537a8`, `SStrCmpI` with `SStrLen`, `0x51ceaa`) against the line once the
    /// spaces after `##` are skipped (`0x51cc3a`-`0x51cc56`), so the colon must follow the key
    /// directly: `## LoadWith : X` is not this directive. The value tokenizes like the other lists
    /// (`0x51cef7`), into the array at `[rec+0x60]`.
    pub fn load_with(&self) -> Vec<&str> {
        self.directives
            .iter()
            .enumerate()
            .filter(|(i, (k, _))| k.eq_ignore_ascii_case("LoadWith") && !self.gapped.contains(i))
            .flat_map(|(_, (_, v))| v.split([',', ' ']).map(str::trim))
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// `## LoadOnDemand:`, which stock 1.12 addons such as `Blizzard_TalentUI` carry as `1`. The
    /// value goes through `0x6f1b30` (called at `0x51d26f`, stored at `0x51d27c`), so `true`,
    /// `yes`, `on` and `enabled` count as well.
    pub fn load_on_demand(&self) -> bool {
        self.directive("LoadOnDemand").is_some_and(string_to_bool)
    }

    /// `## DefaultState:`, the byte at `[rec+0x2b]` the enable query `0x51e470` falls back to when
    /// characters disagree or none has chosen. `Toc_Parse` stores `"enabled"` (`0x853764`) as 1 and
    /// `"disabled"` (`0x853758`) as 0 at `0x51d204` and any other value not at all (`0x51d21a`);
    /// the ctor `0x520550` seeds 1 (`0x5205b9`), so only `disabled` disables.
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
        assert_eq!(toc.list("SavedVariables"), vec!["A", "B", "C", "D"]);
        assert_eq!(toc.list("SavedVariablesPerCharacter"), vec!["E", "F"]);
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

    /// `Toc_Parse` compares the 9 bytes `LoadWith:` (`0x51ceaa`): a longer key, or a space before
    /// the colon, is not it, where `Dep` is a bare prefix.
    #[test]
    fn load_with_is_a_list_of_its_own_key() {
        let toc = Toc::parse(
            "## LoadWith: KLHThreatMeter, Other\n\
             ## loadwith: Third\n\
             ## LoadWith : Spaced\n\
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

    #[test]
    fn empty_and_degenerate_inputs() {
        assert_eq!(Toc::parse(""), Toc::default());
        let toc = Toc::parse("# only a comment\n\n");
        assert!(toc.directives.is_empty() && toc.files.is_empty());
    }
}
