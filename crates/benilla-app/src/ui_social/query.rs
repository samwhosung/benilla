//! The `/who` query builder `FriendList::SendWhoQuery` (`0x5aebb0`, from `SendWho` `0x5ad3b0`): the
//! typed filter into `CMSG_WHO`'s fields, token by token. The filter is copied up to 127 bytes and
//! a space appended (`0x5aebf2`-`0x5aec1a`), read one UTF-8 code point at a time (`sgetu8`
//! `0x41aab0`), and split at white space outside double quotes (`0x5aec90`-`0x5aecda`); a token
//! still quoted when the copy ends is dropped (`0x5af099`). Each token is tested, without case and
//! as a prefix, for the localized `WHO_TAG_NAME`, `_GUILD`, `_ZONE`, `_RACE` and `_CLASS` in that
//! order (`0x5aecd5`-`0x5aeee8`), then for a level range (`0x5aef7d`); any other is a search term,
//! and the fourth ends the scan (`0x5af02d`).

use benilla_formats::AreaTableCatalog;
use benilla_protocol::messages::WhoRequest;

use crate::ui_unit::UnitNames;

/// The five tag strings `FrameScript_GetText` answers for the builder (`0x85dce0`, `0x85dcd0`,
/// `0x85dcc0`, `0x85dcb0`, `0x85dca0`), the empty string for a missing one.
pub(crate) struct WhoTags {
    name: Vec<u8>,
    guild: Vec<u8>,
    zone: Vec<u8>,
    race: Vec<u8>,
    class: Vec<u8>,
}

/// The tag keys, in the builder's test order.
pub(crate) const WHO_TAG_KEYS: [&str; 5] = [
    "WHO_TAG_NAME",
    "WHO_TAG_GUILD",
    "WHO_TAG_ZONE",
    "WHO_TAG_RACE",
    "WHO_TAG_CLASS",
];

impl WhoTags {
    /// The tags from their texts, in [`WHO_TAG_KEYS`]' order.
    pub(crate) fn from_texts<S: AsRef<[u8]>>(texts: [S; 5]) -> Self {
        let [name, guild, zone, race, class] = texts.map(|t| t.as_ref().to_vec());
        Self {
            name,
            guild,
            zone,
            race,
            class,
        }
    }
}

/// The filter copy's buffer (`[ebp-0x128]`, `SStrCopy` bound `0x80`): 127 bytes and a NUL.
const COPY_BYTES: usize = 127;
/// The name and guild buffers (`SStrCopy` bound `0x30`, `0x5aed08`, `0x5aed49`).
const NAME_BYTES: usize = 47;
/// The zone ids the walk collects (`0x5aedc8`).
const MAX_ZONES: usize = 10;
/// The search terms; the fourth ends the scan (`0x5af02d`).
const MAX_TERMS: usize = 4;

/// `sgetu8`'s answer at a NUL (`0x5aec6a`, `0x5af099`).
const END: u32 = 0xffff_ffff;
/// `sgetu8`'s answer for a broken sequence.
const BAD: u32 = 0x8000_0000;

/// Build the query `filter` asks for. `areas` and `names` are the `AreaTable`, `ChrRaces` and
/// `ChrClasses` rows the zone, race and class tags search.
pub(crate) fn parse(
    filter: &[u8],
    tags: &WhoTags,
    areas: Option<&AreaTableCatalog>,
    names: UnitNames<'_>,
) -> WhoRequest {
    let mut copy: Vec<u8> = filter
        .iter()
        .copied()
        .take_while(|&b| b != 0)
        .take(COPY_BYTES)
        .collect();
    // Deviation: a filter of 127 bytes or more puts the copy's NUL in the name buffer's first byte
    // (`0x5aec1a`), so a name an earlier tag set is scanned as more filter. benilla ends the copy
    // at its own NUL: the run-on reads one stack buffer through another.
    copy.push(b' ');
    let mut builder = Builder {
        tags,
        areas,
        names,
        request: WhoRequest::default(),
    };
    let mut token = Vec::new();
    let mut quoted = false;
    let mut at = skip_space(&copy, 0);
    loop {
        let (c, n) = next_code_point(&copy, at);
        if c == END {
            break;
        }
        if c == u32::from(b'"') {
            quoted = !quoted;
            at += n;
            continue;
        }
        if quoted || !is_space(c) {
            // Every byte the decoder read, the one that broke a sequence too (`0x5af06f`).
            token.extend_from_slice(&copy[at..at + n]);
            at += n;
            continue;
        }
        builder.token(std::mem::take(&mut token));
        if builder.request.search_terms.len() >= MAX_TERMS {
            break;
        }
        at = skip_space(&copy, at);
    }
    builder.request
}

/// The position past the white space at `at` (`0x5aec2b`-`0x5aec58`, `0x5af03d`-`0x5af068`).
fn skip_space(copy: &[u8], mut at: usize) -> usize {
    loop {
        let (c, n) = next_code_point(copy, at);
        if !is_space(c) {
            return at;
        }
        at += n;
    }
}

/// `sgetu8` (`0x41aab0`): the code point at `at` and the bytes read. A NUL lead, or a NUL where a
/// continuation belongs, is [`END`]; a stray continuation, or a continuation that is not
/// `10xxxxxx`, is [`BAD`], the bad byte counted. Leads up to `0xfd` take up to five
/// continuations, with no check for overlong forms. The copy's end reads as its NUL.
fn next_code_point(copy: &[u8], at: usize) -> (u32, usize) {
    let byte = |i: usize| u32::from(copy.get(at + i).copied().unwrap_or(0));
    let lead = byte(0);
    let (more, mut point) = match lead {
        0 => return (END, 0),
        0x01..=0x7f => return (lead, 1),
        0xc0..=0xdf => (1, lead & 0x1f),
        0xe0..=0xef => (2, lead & 0xf),
        0xf0..=0xf7 => (3, lead & 7),
        0xf8..=0xfb => (4, lead & 3),
        0xfc..=0xfd => (5, lead & 1),
        _ => return (BAD, 1),
    };
    for k in 1..=more {
        let c = byte(k);
        if c == 0 {
            return (END, k);
        }
        if c & 0xc0 != 0x80 {
            return (BAD, k + 1);
        }
        point = point << 6 | (c & 0x3f);
    }
    (point, more + 1)
}

/// `iswspace` (`0x74006f`) of a code point's low 16 bits: `_iswctype` (`0x7458e4`) answers 0 for
/// `0xffff`, reads the C table `_pwctype` (`0x831720`) below `0x100`, where only tab to carriage
/// return and the space carry `_SPACE` (so not `U+00A0`), and above it asks
/// `___crtGetStringTypeW` (`0x413ed3`), which calls `GetStringTypeW(CT_CTYPE1, &unit, 1, &type)`
/// through `0x8824a0` and tests `C1_SPACE`.
fn is_space(c: u32) -> bool {
    match c as u16 {
        0xffff => false,
        w @ 0..=0xff => matches!(w, 0x09..=0x0d | 0x20),
        // Deviation: `C1_SPACE` from `GetStringTypeW` is the Windows system's own table, which
        // no file of the client holds; benilla answers Unicode's `White_Space` for those units,
        // and nothing for a lone surrogate.
        w => char::from_u32(u32::from(w)).is_some_and(char::is_whitespace),
    }
}

/// `iswdigit` (`0x740050`) of a byte sign-extended to 16 bits (`movsx ax`): a byte of `0x80` or
/// more is `U+FF80` to `U+FFFF`, none of them a digit.
fn is_wide_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

/// `_isdigit` (`0x73fc0f`) of the byte after a range's digits or its leading `-` (`0x5aefd9`),
/// sign-extended: a byte of `0x80` or more reads the 128 bytes before the ctype table, where the
/// digit bit is set for `0xf7`, `0xfb` and `0xfd` (the decimal point `0x831714` and the low bytes
/// of `0x83171c` and `0x831720`).
fn is_digit(b: u8) -> bool {
    b.is_ascii_digit() || matches!(b, 0xf7 | 0xfb | 0xfd)
}

/// `SStrToInt` (`0x64ac60`): an optional `-`, then decimal digits in wrapping 32-bit arithmetic,
/// 0 when no digit follows.
fn str_to_int(s: &[u8]) -> u32 {
    let (negative, digits) = match s.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, s),
    };
    let value = digits
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0u32, |v, &d| {
            v.wrapping_mul(10).wrapping_add(u32::from(d - b'0'))
        });
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

/// `SStrCmpI` (`0x64a4c0`) over the tag's length: does `token` begin with `tag`, `A`-`Z` folded?
/// An empty tag begins every token.
fn starts_with_tag(token: &[u8], tag: &[u8]) -> bool {
    token.len() >= tag.len() && token[..tag.len()].eq_ignore_ascii_case(tag)
}

/// `SStrStrI` (`0x64b540`): does `text` occur in `name`, `A`-`Z` folded? An empty name holds
/// nothing, and an empty text is in any other.
fn contains(name: &[u8], text: &[u8]) -> bool {
    !name.is_empty() && (0..name.len()).any(|i| starts_with_tag(&name[i..], text))
}

/// `SStrCopy` into a 48-byte buffer: the first 47 bytes.
fn bounded(text: &[u8]) -> Vec<u8> {
    text[..text.len().min(NAME_BYTES)].to_vec()
}

struct Builder<'a> {
    tags: &'a WhoTags,
    areas: Option<&'a AreaTableCatalog>,
    names: UnitNames<'a>,
    request: WhoRequest,
}

impl Builder<'_> {
    /// One token at the white space that ends it (`0x5aecda`).
    fn token(&mut self, token: Vec<u8>) {
        let tags = self.tags;
        if starts_with_tag(&token, &tags.name) {
            self.request.player_name = bounded(&token[tags.name.len()..]);
        } else if starts_with_tag(&token, &tags.guild) {
            self.request.guild_name = bounded(&token[tags.guild.len()..]);
        } else if starts_with_tag(&token, &tags.zone) {
            self.zone(&token[tags.zone.len()..]);
        } else if starts_with_tag(&token, &tags.race) {
            let rows = self.names.races.into_iter().flat_map(|t| t.names_by_id());
            add_matches(&mut self.request.race_mask, rows, &token[tags.race.len()..]);
        } else if starts_with_tag(&token, &tags.class) {
            let rows = self.names.classes.into_iter().flat_map(|t| t.names_by_id());
            add_matches(
                &mut self.request.class_mask,
                rows,
                &token[tags.class.len()..],
            );
        } else if !self.levels(&token) {
            self.request.search_terms.push(token);
        }
    }

    /// The zone walk (`0x5aed8f`-`0x5aee0f`): every `AreaTable` row with no parent (`+8`,
    /// `0x5aedc1`), in record order, whose name contains `text`, while the list holds fewer than
    /// 10; the list carries over from one zone tag to the next, and one left empty holds zone 0
    /// (`0x5aee08`).
    fn zone(&mut self, text: &[u8]) {
        let zones = &mut self.request.zones;
        if let Some(areas) = self.areas {
            for (id, row) in areas.rows_in_order() {
                if zones.len() >= MAX_ZONES {
                    break;
                }
                if row.zone_id == 0 && contains(row.name.as_bytes(), text) {
                    zones.push(id);
                }
            }
        }
        if zones.is_empty() {
            zones.push(0);
        }
    }

    /// A level range (`0x5aef7d`-`0x5aeff2`), or `false` for a term. A token that begins with a
    /// digit sets both levels to its number, then 100 for the maximum after a `-` (`0x5aefc4`) and
    /// the number after that dash; one that begins with `-` and a digit sets only the maximum.
    /// Nothing is clamped.
    fn levels(&mut self, token: &[u8]) -> bool {
        let byte = |i: usize| token.get(i).copied().unwrap_or(0);
        let request = &mut self.request;
        let mut at = 1;
        let range = if is_wide_digit(byte(0)) {
            let n = str_to_int(token);
            (request.level_min, request.level_max) = (n, n);
            while is_wide_digit(byte(at)) {
                at += 1;
            }
            if byte(at) == b'-' {
                at += 1;
                request.level_max = 100;
            }
            true
        } else if byte(0) == b'-' {
            false
        } else {
            return false;
        };
        if is_digit(byte(at)) {
            request.level_max = str_to_int(&token[at..]);
            return true;
        }
        range
    }
}

/// A race or class tag (`0x5aee53`-`0x5aeec1`, `0x5aef00`-`0x5aef70`): the first of its kind clears
/// the all-ones mask, then every row whose localized name contains `text` adds `1 << (id mod 32)`
/// (`shl edx,cl`, `0x5aeeab`).
fn add_matches<'n>(mask: &mut u32, rows: impl Iterator<Item = (u32, &'n str)>, text: &[u8]) {
    if *mask == u32::MAX {
        *mask = 0;
    }
    for (id, name) in rows {
        if contains(name.as_bytes(), text) {
            *mask |= 1 << (id & 31);
        }
    }
}

#[cfg(test)]
mod tests;
