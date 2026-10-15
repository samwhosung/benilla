//! The client's locale: which of a localized DBC string's eight columns it reads. Start-up
//! (`0x402350`) reads `Wow.ini`'s `[WoW Config]` `Language` then `Country` (`0x4024d6`,
//! `0x4024f7`), formats them `"%s%s"` into five bytes (`0x402513`, so four characters at most),
//! packs those bytes into the locale tag (`0x40251b`-`0x402537`, `[0xc2a2a4]`), and stores as the
//! locale index the position of the name in [`LOCALE_NAMES`] the string equals, any case
//! (`0x402540`-`0x402562`, `[0xc0e080]`). Nothing else writes either word. With no `Wow.ini` or
//! either key missing both stay 0; a name the table lacks (enGB) still sets the tag, and the index
//! stays 0, enUS's column.

/// The locale names in index order, the table at `0x8558a4`.
pub const LOCALE_NAMES: [&str; 8] = [
    "enUS", "koKR", "frFR", "deDE", "zhCN", "zhTW", "esES", "xxYY",
];

/// The locale index and tag start-up derives from `Wow.ini`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Locale {
    index: u8,
    tag: u32,
}

impl Locale {
    /// The locale of a start-up whose `Wow.ini` names both keys: `language` then `country`, as
    /// the bytes of their first values.
    pub fn from_language_country(language: &[u8], country: &[u8]) -> Self {
        // `SStrPrintf(buf, 5, "%s%s", Language, Country)`: the concatenation cut to four bytes.
        let joined: Vec<u8> = language.iter().chain(country).copied().take(4).collect();
        // Each byte sign-extended (`movsx`) and added to the tag shifted left a byte.
        let tag = joined.iter().fold(0u32, |acc, &b| {
            (acc << 8).wrapping_add(i32::from(b as i8) as u32)
        });
        // `SStrCmpI` against each name, whole strings compared (length `0x7fffffff`).
        let index = LOCALE_NAMES
            .iter()
            .position(|name| name.as_bytes().eq_ignore_ascii_case(&joined))
            .unwrap_or(0);
        Self {
            index: index as u8,
            tag,
        }
    }

    /// The locale at `index` in [`LOCALE_NAMES`], its tag that name packed; `None` past the table.
    pub fn from_index(index: usize) -> Option<Self> {
        let name = LOCALE_NAMES.get(index)?.as_bytes();
        Some(Self::from_language_country(&name[..2], &name[2..]))
    }

    /// The locale index, `[0xc0e080]`: the column a localized string is read from.
    pub fn index(self) -> usize {
        usize::from(self.index)
    }

    /// The name at the index, as `GetLocale` answers it (`0x48d8b0`).
    pub fn name(self) -> &'static str {
        LOCALE_NAMES[self.index()]
    }

    /// The packed configured string, `[0xc2a2a4]`; 0 when `Wow.ini` named no locale.
    pub fn tag(self) -> u32 {
        self.tag
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_is_the_names_position_in_the_table() {
        let de = Locale::from_language_country(b"de", b"DE");
        assert_eq!(de.index(), 3);
        assert_eq!(de.name(), "deDE");
        assert_eq!(de.tag(), u32::from_be_bytes(*b"deDE"));
        assert_eq!(Locale::from_language_country(b"zh", b"TW").index(), 5);
        assert_eq!(Locale::from_language_country(b"xx", b"YY").index(), 7);
    }

    /// `SStrCmpI`: the case of either key does not matter, and the tag keeps the bytes as read.
    #[test]
    fn the_names_compare_without_case() {
        let fr = Locale::from_language_country(b"FR", b"fr");
        assert_eq!(fr.index(), 2);
        assert_eq!(fr.tag(), u32::from_be_bytes(*b"FRfr"));
    }

    /// enGB is no name in the table, so its index stays 0 and reads enUS's column; its tag is still
    /// the packed string.
    #[test]
    fn a_name_the_table_lacks_reads_column_zero() {
        let gb = Locale::from_language_country(b"en", b"GB");
        assert_eq!(gb.index(), 0);
        assert_eq!(gb.name(), "enUS");
        assert_eq!(gb.tag(), u32::from_be_bytes(*b"enGB"));
        assert_eq!(Locale::from_language_country(b"", b"").index(), 0);
        assert_eq!(Locale::from_language_country(b"", b"").tag(), 0);
        assert_eq!(Locale::default(), Locale { index: 0, tag: 0 });
    }

    /// The five-byte buffer cuts the joined string at four characters before the compare: a long
    /// language still matches on its first four, and the split between the keys does not matter.
    #[test]
    fn the_joined_string_is_cut_at_four_bytes() {
        assert_eq!(Locale::from_language_country(b"deDE", b"").index(), 3);
        assert_eq!(Locale::from_language_country(b"d", b"eDEx").index(), 3);
        assert_eq!(Locale::from_language_country(b"English", b"US").index(), 0);
        assert_eq!(
            Locale::from_language_country(b"English", b"US").tag(),
            u32::from_be_bytes(*b"Engl")
        );
        assert_eq!(
            Locale::from_language_country(b"en", b"").tag(),
            u32::from_be_bytes([0, 0, b'e', b'n'])
        );
    }

    /// A byte past 0x7f is sign-extended before the add (`movsx`), so it borrows from the bytes
    /// above it.
    #[test]
    fn a_high_byte_is_sign_extended_into_the_tag() {
        let tag = Locale::from_language_country(&[b'a', 0xe9], b"").tag();
        assert_eq!(tag, ((u32::from(b'a')) << 8).wrapping_add(0xffff_ffe9));
    }

    #[test]
    fn from_index_round_trips_every_name() {
        for (i, name) in LOCALE_NAMES.iter().enumerate() {
            let l = Locale::from_index(i).unwrap();
            assert_eq!((l.index(), l.name()), (i, *name));
        }
        assert_eq!(Locale::from_index(8), None);
    }
}
