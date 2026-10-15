//! `Wow.ini`, the client's `[WoW Config]` section. The reference loads it through the archive
//! chain at startup (`0x40249d` → `0x444460`, whose `SFileLoadFile` at `0x444507` opens with flags
//! 0; the extra flags `[0x865b40]` are cleared by `0x648be0(0)` at `0x40221f`, so no loose copy),
//! and a stock install ships it in `interface.MPQ`; `Region` picks the realm categories
//! ([`crate::load_realm_categories`]), `Language` and `Country` the locale ([`crate::Locale`]).

use crate::{Chain, Locale};

const WOW_INI: &str = "Wow.ini";
const SECTION: &str = "WoW Config";

/// The client Region, `[WoW Config] Region` (`0x4024bd` into `[0x882730]`). 0 when the file is
/// absent (`0x4024a4` skips the read, leaving the zero-initialised `.data` word) or has no such
/// key (`0x44542c` stores 0 before the lookup).
pub fn client_region(chain: &Chain) -> u32 {
    chain
        .read(WOW_INI)
        .ok()
        .and_then(|bytes| ini_value(&bytes, SECTION, "Region").map(ini_int))
        .unwrap_or(0) as u32
}

/// The client locale from `[WoW Config]` `Language` and `Country` (`0x4024c2`-`0x402562`). The
/// default, index 0 and tag 0, when the file is absent (`0x4024a4`) or either key has no value
/// (`0x4024dd`, `0x4024fe`): both jump past the tag and the index.
pub(crate) fn client_locale(chain: &Chain) -> Locale {
    chain
        .read(WOW_INI)
        .map(|bytes| locale_from_ini(&bytes))
        .unwrap_or_default()
}

/// The locale a `Wow.ini`'s text names. Each value is copied into 16 bytes first (`0x4024c4`,
/// `0x4024e5`, `SStrCopy` at `0x445612`), a cut the four-byte format below makes moot.
fn locale_from_ini(text: &[u8]) -> Locale {
    match (
        ini_value(text, SECTION, "Language"),
        ini_value(text, SECTION, "Country"),
    ) {
        (Some(language), Some(country)) => Locale::from_language_country(language, country),
        _ => Locale::default(),
    }
}

/// The first value of `key` in `[section]`, as the reader `0x444540` splits the file and the
/// lookup `0x4452c0` answers index 0; `None` when the key is absent or its value is empty.
///
/// A line is a `//` comment, a `[section]` (cut at `]`; with no `]` the line is dropped and the
/// section before it stays current), or `key=value`, cut at the first `=` and at the end of the
/// line (CR or LF), with no trimming either side (`0x4445e0`-`0x4446b9`). A value splits into
/// several at each `,`, and one opening with `"` runs to the next `"` instead (`0x444710`); an
/// empty value is no value at all (`0x444732`). A line before any section has none and is dropped
/// (`0x44471e`). Section and key names compare without case (`SStrCmpI` at `0x445356` and
/// `0x4453cb`); the first line naming the key answers.
fn ini_value<'a>(text: &'a [u8], section: &str, key: &str) -> Option<&'a [u8]> {
    // The walk stops at a NUL (`0x4446c8`).
    let text = text.split(|&b| b == 0).next().unwrap_or_default();
    let mut current: Option<&[u8]> = None;
    for line in text.split(|&b| b == b'\r' || b == b'\n') {
        if line.is_empty() || line.starts_with(b"//") {
            continue;
        }
        if let Some(rest) = line.strip_prefix(b"[") {
            if let Some(end) = rest.iter().position(|&b| b == b']') {
                current = Some(&rest[..end]);
            }
            continue;
        }
        let Some(eq) = line.iter().position(|&b| b == b'=') else {
            continue;
        };
        let (k, value) = (&line[..eq], &line[eq + 1..]);
        let in_section = current.is_some_and(|s| s.eq_ignore_ascii_case(section.as_bytes()));
        if !in_section || !k.eq_ignore_ascii_case(key.as_bytes()) || value.is_empty() {
            continue;
        }
        return Some(match value.strip_prefix(b"\"") {
            Some(quoted) => quoted.split(|&b| b == b'"').next().unwrap_or_default(),
            None => value.split(|&b| b == b',').next().unwrap_or_default(),
        });
    }
    None
}

/// An integer value as `0x445470` reads one: a quoted run of up to four characters packed
/// big-endian, else `SStrToInt` (`0x64ac60`), an optional `-` then decimal digits up to the first
/// non-digit, 0 when there are none.
fn ini_int(value: &[u8]) -> i32 {
    if let Some(quoted) = value.strip_prefix(b"'") {
        return quoted
            .iter()
            .take_while(|&&b| b != b'\'')
            .take(4)
            .fold(0i32, |acc, &b| (acc << 8) | i32::from(b));
    }
    let (negative, digits) = match value.strip_prefix(b"-") {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    let n = digits
        .iter()
        .take_while(|d| d.is_ascii_digit())
        .fold(0i32, |acc, d| {
            acc.wrapping_mul(10).wrapping_add(i32::from(d - b'0'))
        });
    if negative {
        n.wrapping_neg()
    } else {
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(ini: &'a str, key: &str) -> Option<&'a str> {
        ini_value(ini.as_bytes(), SECTION, key).map(|v| std::str::from_utf8(v).unwrap())
    }

    #[test]
    fn the_region_is_read_from_the_config_section_only() {
        let ini = "[Other]\r\nRegion=7\r\n[WoW Config]\r\nVersion=1\r\nregion=3\r\n";
        assert_eq!(value(ini, "Region"), Some("3"));
        assert_eq!(value("[wow config]\nRegion=1", "Region"), Some("1"));
        assert_eq!(value("[WoW Config]\nVersion=1\n", "Region"), None);
        assert_eq!(
            value("Region=1\n[WoW Config]\n", "Region"),
            None,
            "no section yet"
        );
    }

    /// `0x4445e0`-`0x4446b9`: the key runs to the `=` and the value to the line's end, untrimmed.
    #[test]
    fn neither_key_nor_value_is_trimmed() {
        assert_eq!(value("[WoW Config]\nRegion = 1\n", "Region"), None);
        assert_eq!(value("[WoW Config]\nRegion= 1 \n", "Region"), Some(" 1 "));
        assert_eq!(value("[WoW Config]\n Country=DE\n", "Country"), None);
        assert_eq!(value("[WoW Config]\nCountry=a=b\n", "Country"), Some("a=b"));
    }

    #[test]
    fn comments_and_unclosed_sections_are_skipped() {
        let ini = "[WoW Config]\n//Country=FR\n[Broken\nCountry=DE\n";
        assert_eq!(
            value(ini, "Country"),
            Some("DE"),
            "the open section stays current"
        );
        assert_eq!(value("[WoW Config] x\nCountry=DE", "Country"), Some("DE"));
        assert_eq!(
            value("[WoW Config]\nA=1\0\nCountry=DE", "Country"),
            None,
            "a NUL ends the file"
        );
    }

    /// `0x444710`: a value splits at commas unless quoted, and an empty one is no value, so a later
    /// line naming the key answers.
    #[test]
    fn the_first_value_is_cut_at_a_comma_or_a_quote() {
        assert_eq!(value("[WoW Config]\nCountry=DE,FR", "Country"), Some("DE"));
        assert_eq!(
            value("[WoW Config]\nCountry=\"D,E\",FR", "Country"),
            Some("D,E")
        );
        assert_eq!(value("[WoW Config]\nCountry=,FR", "Country"), Some(""));
        assert_eq!(
            value("[WoW Config]\nCountry=\nCountry=FR", "Country"),
            Some("FR")
        );
        assert_eq!(
            value("[WoW Config]\nCountry=US\nCountry=FR", "Country"),
            Some("US")
        );
    }

    fn locale_of(ini: &str) -> Locale {
        locale_from_ini(ini.as_bytes())
    }

    /// Language then Country, the arguments `0x402513` formats (`0x402507`, `0x402503`); either
    /// missing leaves the default, tag and index 0.
    #[test]
    fn the_locale_joins_language_and_country() {
        let de = locale_of("[WoW Config]\r\nCountry=DE\r\nLanguage=de\r\n");
        assert_eq!((de.index(), de.tag()), (3, u32::from_be_bytes(*b"deDE")));
        let gb = locale_of("[WoW Config]\r\nCountry=GB\r\nLanguage=en\r\n");
        assert_eq!((gb.index(), gb.tag()), (0, u32::from_be_bytes(*b"enGB")));
        assert_eq!(locale_of("[WoW Config]\nCountry=DE\n"), Locale::default());
        assert_eq!(locale_of("[WoW Config]\nLanguage=de\n"), Locale::default());
    }

    #[test]
    fn a_value_parses_as_the_reference_parses_it() {
        assert_eq!(ini_int(b"3"), 3);
        assert_eq!(ini_int(b"101"), 101);
        assert_eq!(ini_int(b"-2"), -2);
        assert_eq!(ini_int(b"12abc"), 12, "digits up to the first non-digit");
        assert_eq!(ini_int(b"abc"), 0);
        assert_eq!(ini_int(b"'AB'"), 0x4142, "a quoted run packs its bytes");
    }

    /// The install carries a `Wow.ini` in its archives, and its Region names categories.
    #[test]
    fn the_install_names_a_region_with_categories() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        assert!(chain.contains(WOW_INI), "Wow.ini is not in the patch chain");
        let region = client_region(&chain);
        assert_ne!(region, 0, "the shipped Wow.ini names a Region");
        let categories =
            crate::load_realm_categories(&mut chain, region).expect("Cfg_Categories.dbc");
        assert!(!categories.is_empty(), "Region {region} has no categories");
    }

    /// The install's `Wow.ini` names a locale, and the chain carries the one it names: the tag is
    /// the language then the country, read off the file's own lines.
    #[test]
    fn the_chain_carries_the_installs_locale() {
        let data = crate::wow_data_or_skip!();
        let chain = crate::open_chain(&data).expect("open chain");
        let text = String::from_utf8(chain.read(WOW_INI).expect("Wow.ini")).expect("ASCII");
        let line = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
                .expect("the shipped file names the key")
                .to_string()
        };
        let joined = format!("{}{}", line("Language"), line("Country"));
        let packed = joined
            .bytes()
            .take(4)
            .fold(0u32, |acc, b| (acc << 8) | u32::from(b));
        assert_ne!(packed, 0);
        assert_eq!(chain.locale().tag(), packed, "{joined}");
        let index = crate::locale::LOCALE_NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(&joined))
            .unwrap_or(0);
        assert_eq!(chain.locale().index(), index, "{joined}");
    }
}
