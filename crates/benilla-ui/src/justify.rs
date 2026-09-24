//! The justify dword and its token table (`.rdata 0x811ad0`), shared by the `FontString`
//! (`0x87c1d8`), `Font` (`0x87c7c8`) and `EditBox` method tables. Each setter replaces only its
//! own axis, and also clears the inherit bit whether or not the value changed (`0x79fc7c`).

use crate::script::{JustifyH, JustifyV};

/// `.rdata 0x811ad0`, six `{u32 bits, const char* name}` entries in image order. The order
/// matters: `0x6f1990` scans linearly and `0x6f1a00` answers the first entry with its bit set.
const TOKENS: [(u32, &str); 6] = [
    (0x01, "LEFT"),
    (0x02, "CENTER"),
    (0x04, "RIGHT"),
    (0x08, "TOP"),
    (0x10, "MIDDLE"),
    (0x20, "BOTTOM"),
];

/// The literal `0x6f1a00` answers when no bit in the requested axis is set (`.data 0x838044`).
pub const UNKNOWN: &str = "UNKNOWN";

/// The horizontal axis, bits 0–2: the mask of `SetJustifyH 0x79fc20`.
pub const H_MASK: u32 = 0x07;
/// The vertical axis, bits 3–5: the mask of `SetJustifyV 0x79fce0`.
pub const V_MASK: u32 = 0x38;

/// A justify argument after `0x6f1990` and the setter's mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Set<J> {
    /// A token with a bit on this axis.
    To(J),
    /// A token with no bit on this axis (`SetJustifyH("TOP")`): the reference clears the axis and
    /// raises nothing; corpus addons call `SetJustifyV("CENTER")` and hit this.
    Clears,
    /// No entry matched: the caller raises its `Usage:` string, never coerces.
    NoMatch,
}

/// `CENTER | MIDDLE | 0x200`, as `CSimpleFontString`'s ctor writes it (`0x770dd3`); bit 9 is in
/// neither axis and no reader looks at it.
const CTOR_DEFAULT: u32 = 0x212;

/// The justify dword (`CSimpleFont+0x54`, `CSimpleFontString+0x120`, `CSimpleEditBox`'s own),
/// bits 0–2 horizontal and 3–5 vertical. Kept raw because an axis with no bit set is reachable,
/// and there the getter answers `"UNKNOWN"` while the draw path centres.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Justify(pub u32);

impl Default for Justify {
    fn default() -> Self {
        Justify(CTOR_DEFAULT)
    }
}

impl Justify {
    pub fn set_h(&mut self, j: JustifyH) {
        self.0 = set_axis(self.0, H_MASK, bits_h(j));
    }
    pub fn set_v(&mut self, j: JustifyV) {
        self.0 = set_axis(self.0, V_MASK, bits_v(j));
    }
    /// Erase the horizontal axis, as a cross-axis token does.
    pub fn clear_h(&mut self) {
        self.0 = set_axis(self.0, H_MASK, 0);
    }
    pub fn clear_v(&mut self) {
        self.0 = set_axis(self.0, V_MASK, 0);
    }

    /// `GetJustifyH`'s answer (`0x79e5f0`) through `0x6f1a00`: `"UNKNOWN"` for a cleared axis.
    pub fn name_h(self) -> &'static str {
        name_of(self.0, H_MASK)
    }
    /// `GetJustifyV`'s answer (`0x79e7f0`).
    pub fn name_v(self) -> &'static str {
        name_of(self.0, V_MASK)
    }

    /// Where the glyphs draw horizontally: the translator `0x44d420` (called only from
    /// `0x772693`) is a ladder whose result is pre-set to `1`, CENTER (`0x44d4fb`, before the
    /// `je` at `0x44d516`), so a cleared axis draws centred. The gx enum's `else` arms, LEFT and
    /// BOTTOM, are unreachable (`0x44d420` emits only 0 to 2, `0x5c1c30` rejects 3 and up):
    /// mapping the bitmask onto that enum with a `0` default inverts both axes.
    pub fn paint_h(self) -> JustifyH {
        if self.0 & 0x04 != 0 {
            JustifyH::Right
        } else if self.0 & 0x01 != 0 {
            JustifyH::Left
        } else {
            JustifyH::Center // 0x02 set, or no bit set: the pre-set `1`
        }
    }

    /// The same ladder vertically, `TOP > BOTTOM > MIDDLE`, falling through to MIDDLE
    /// (`0x44d536`).
    pub fn paint_v(self) -> JustifyV {
        if self.0 & 0x08 != 0 {
            JustifyV::Top
        } else if self.0 & 0x20 != 0 {
            JustifyV::Bottom
        } else {
            JustifyV::Middle
        }
    }
}

fn bits_h(j: JustifyH) -> u32 {
    match j {
        JustifyH::Left => 0x01,
        JustifyH::Center => 0x02,
        JustifyH::Right => 0x04,
    }
}

fn bits_v(j: JustifyV) -> u32 {
    match j {
        JustifyV::Top => 0x08,
        JustifyV::Middle => 0x10,
        JustifyV::Bottom => 0x20,
    }
}

/// `0x6f1990`: `SStrCmpI` over the six entries, whole-string and untrimmed, so `"left "` misses.
pub fn parse_bits(s: &str) -> Option<u32> {
    TOKENS
        .iter()
        .find(|(_, name)| s.eq_ignore_ascii_case(name))
        .map(|(bits, _)| *bits)
}

/// `0x6f1a00`: the first entry whose bit is set within `mask`, else the literal `UNKNOWN`.
pub fn name_of(bits: u32, mask: u32) -> &'static str {
    TOKENS
        .iter()
        .find(|(b, _)| bits & mask & b != 0)
        .map_or(UNKNOWN, |(_, name)| *name)
}

/// The setters' `(cur ^ parsed) & mask ^ cur` (`SetJustifyH` at `0x79fc5d`–`0x79fc64`), which is
/// why a cross-axis token clears rather than raises.
pub fn set_axis(cur: u32, mask: u32, parsed: u32) -> u32 {
    (cur & !mask) | (parsed & mask)
}

/// `SetJustifyH`'s argument, against mask `0x07`.
pub fn parse_h(s: &str) -> Set<JustifyH> {
    match parse_bits(s) {
        None => Set::NoMatch,
        Some(0x01) => Set::To(JustifyH::Left),
        Some(0x02) => Set::To(JustifyH::Center),
        Some(0x04) => Set::To(JustifyH::Right),
        Some(_) => Set::Clears,
    }
}

/// `SetJustifyV`'s argument, against mask `0x38`.
pub fn parse_v(s: &str) -> Set<JustifyV> {
    match parse_bits(s) {
        None => Set::NoMatch,
        Some(0x08) => Set::To(JustifyV::Top),
        Some(0x10) => Set::To(JustifyV::Middle),
        Some(0x20) => Set::To(JustifyV::Bottom),
        Some(_) => Set::Clears,
    }
}

/// `GetJustifyH`'s answer for a resolved justification.
pub fn name_h(j: JustifyH) -> &'static str {
    name_of(bits_h(j), H_MASK)
}

pub fn name_v(j: JustifyV) -> &'static str {
    name_of(bits_v(j), V_MASK)
}

/// The `Usage:` string `SetJustifyH` raises on a miss (`.rdata 0x87c77c`), the `%s` spelled as
/// the widget type.
pub fn usage_h(widget: &str) -> mlua::Error {
    mlua::Error::runtime(format!("Usage: <{widget}>:SetJustifyH(\"justify\")"))
}

/// The `Usage:` string `SetJustifyV` raises on a miss (`.rdata 0x87c7a0`).
pub fn usage_v(widget: &str) -> mlua::Error {
    mlua::Error::runtime(format!("Usage: <{widget}>:SetJustifyV(\"justify\")"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_match_is_case_insensitive_and_whole_string() {
        assert_eq!(parse_h("LEFT"), Set::To(JustifyH::Left));
        assert_eq!(parse_h("left"), Set::To(JustifyH::Left));
        assert_eq!(parse_h("LeFt"), Set::To(JustifyH::Left));
        // `SStrCmpI` compares the whole string, untrimmed.
        assert_eq!(parse_h("left "), Set::NoMatch);
        assert_eq!(parse_h(" left"), Set::NoMatch);
        assert_eq!(parse_h("leftmost"), Set::NoMatch);
        assert_eq!(parse_h(""), Set::NoMatch);
    }

    #[test]
    fn an_unmatched_string_does_not_coerce_to_center() {
        assert_eq!(parse_h("MIDDLE_LEFT"), Set::NoMatch);
        assert_eq!(parse_v("CENTERED"), Set::NoMatch);
    }

    #[test]
    fn a_cross_axis_token_matches_but_carries_no_bit_for_this_axis() {
        for v in ["TOP", "MIDDLE", "BOTTOM"] {
            assert_eq!(parse_h(v), Set::Clears, "SetJustifyH({v:?})");
        }
        for h in ["LEFT", "CENTER", "RIGHT"] {
            assert_eq!(parse_v(h), Set::Clears, "SetJustifyV({h:?})");
        }
    }

    #[test]
    fn every_token_round_trips_through_the_one_table() {
        assert_eq!(name_h(JustifyH::Left), "LEFT");
        assert_eq!(name_h(JustifyH::Center), "CENTER");
        assert_eq!(name_h(JustifyH::Right), "RIGHT");
        assert_eq!(name_v(JustifyV::Top), "TOP");
        assert_eq!(name_v(JustifyV::Middle), "MIDDLE");
        assert_eq!(name_v(JustifyV::Bottom), "BOTTOM");
    }

    #[test]
    fn the_formatter_is_first_bit_set_within_the_axis_else_unknown() {
        // `0x6f1a00`: no bit in the axis answers UNKNOWN, two bits answer the first table entry.
        assert_eq!(name_of(0x00, H_MASK), "UNKNOWN");
        assert_eq!(name_of(0x38, H_MASK), "UNKNOWN"); // vertical bits only
        assert_eq!(name_of(0x05, H_MASK), "LEFT"); // LEFT | RIGHT
        assert_eq!(name_of(0x30, V_MASK), "MIDDLE"); // MIDDLE | BOTTOM
        assert_eq!(name_of(0x212, H_MASK), "CENTER"); // the ctor default; bit 9 is in no axis
        assert_eq!(name_of(0x212, V_MASK), "MIDDLE");
    }
}
