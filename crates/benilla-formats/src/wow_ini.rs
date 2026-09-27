//! `Wow.ini`, the client's `[WoW Config]` section. The reference loads it through the archive
//! chain at startup (`0x40249d`, `SFileLoadFile`), and a stock install ships it in
//! `interface.MPQ`; `Region` picks the realm categories ([`crate::load_realm_categories`]).

use crate::Chain;

const WOW_INI: &str = "Wow.ini";
const SECTION: &str = "WoW Config";

/// The client Region, `[WoW Config] Region` (`0x4024bd` into `[0x882730]`). 0 when the file is
/// absent (`0x4024a4` skips the read, leaving the zero-initialised `.data` word) or has no such
/// key (`0x44542c` stores 0 before the lookup).
pub fn client_region(chain: &Chain) -> u32 {
    chain
        .read(WOW_INI)
        .ok()
        .and_then(|bytes| {
            ini_value(&String::from_utf8_lossy(&bytes), SECTION, "Region").map(ini_int)
        })
        .unwrap_or(0) as u32
}

/// The value of `key` in `[section]`. Both names compare case-insensitively (`SStrCmpI` at
/// `0x445356` and `0x4453cb`).
fn ini_value<'a>(text: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut in_section = false;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = name.trim().eq_ignore_ascii_case(section);
        } else if let (true, Some((k, v))) = (in_section, line.split_once('=')) {
            if k.trim().eq_ignore_ascii_case(key) {
                return Some(v.trim());
            }
        }
    }
    None
}

/// An integer value as `0x445470` reads one: a quoted run of up to four characters packed
/// big-endian, else `SStrToInt` (`0x64ac60`), an optional `-` then decimal digits up to the first
/// non-digit, 0 when there are none.
fn ini_int(value: &str) -> i32 {
    if let Some(quoted) = value.strip_prefix('\'') {
        return quoted
            .bytes()
            .take_while(|&b| b != b'\'')
            .take(4)
            .fold(0i32, |acc, b| (acc << 8) | i32::from(b));
    }
    let (negative, digits) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
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

    #[test]
    fn the_region_is_read_from_the_config_section_only() {
        let ini = "[Other]\r\nRegion=7\r\n[WoW Config]\r\nVersion=1\r\nregion=3\r\n";
        assert_eq!(ini_value(ini, SECTION, "Region"), Some("3"));
        assert_eq!(
            ini_value("[wow config]\nRegion = 1\n", SECTION, "Region"),
            Some("1")
        );
        assert_eq!(
            ini_value("[WoW Config]\nVersion=1\n", SECTION, "Region"),
            None
        );
    }

    #[test]
    fn a_value_parses_as_the_reference_parses_it() {
        assert_eq!(ini_int("3"), 3);
        assert_eq!(ini_int("101"), 101);
        assert_eq!(ini_int("-2"), -2);
        assert_eq!(ini_int("12abc"), 12, "digits up to the first non-digit");
        assert_eq!(ini_int("abc"), 0);
        assert_eq!(ini_int("'AB'"), 0x4142, "a quoted run packs its bytes");
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
}
