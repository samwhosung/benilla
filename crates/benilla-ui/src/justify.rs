//! The justify enum — **one** transcription of `.rdata 0x811ad0`, shared by every table that
//! speaks it.
//!
//! ## Why this is a module and not three sets of `match` arms
//!
//! Justification is set on **three** widget tables — a `FontString` (`0x87c1d8`), a `<Font>`
//! object (`0x87c7c8`) and an `EditBox` — and each had been transcribed *independently* from the
//! same byte law. Each drifted its own way, and each looked right alone:
//!
//! | | FontString ([`crate::script::region`]) | Font ([`crate::script::font`]) | EditBox ([`crate::widget::kinds::editbox`]) | the bytes |
//! |---|---|---|---|---|
//! | `GetJustifyH`/`V` | **absent** | present | present | present (`0x79e5f0`/`0x79e7f0`) |
//! | space-padded token | rejected | `.trim()`ed, **accepted** | rejected | rejected (whole-string) |
//! | unmatched string | silently `CENTER` | silently `CENTER` | **raises** | **raises** |
//! | cross-axis token | silently `CENTER` | silently `CENTER` | **clears the axis** | clears the axis |
//! | axis storage | resolved enum → **the dword** (1239) | resolved enum | **the raw dword** | one dword |
//!
//! That is the seam class the addon-lane round trip named: halves transcribed separately from one
//! law drift where they meet, while every test on every side still passes. **The EditBox column is
//! the one that was right** — written last, straight from the bytes, and never consulted by the two
//! older tables. So this module is not new law: it is that column, lifted to where all three reach
//! it. It sits at the crate root rather than under `script` because `widget::kinds` needs it too.
//!
//! ## The law
//!
//! Storage is a single dword — `CSimpleFont+0x54`, `CSimpleFontString+0x120` — with **bits 0–2
//! horizontal and bits 3–5 vertical**. `SetJustifyH 0x79fc20` computes `(cur ^ parsed) & 0x07 ^
//! cur`, i.e. `(cur & !0x07) | (parsed & 0x07)`, replacing only its own axis (VERIFIED at
//! `0x79fc5d`–`0x79fc64`); `SetJustifyV 0x79fce0` uses mask `0x38`. The ctor default is `0x212` =
//! `CENTER | MIDDLE | 0x200`, bit 9 being outside both masks and read by neither accessor.
//!
//! Two consequences fall out of that arithmetic, and both are transcribed below:
//!
//! - **A cross-axis token is accepted and clears.** `SetJustifyH("TOP")` parses fine (`0x08`) but
//!   `0x08 & 0x07 == 0`, so it erases the horizontal justification with **no error raised**, and a
//!   following `GetJustifyH()` answers `"UNKNOWN"` — while the glyphs keep drawing **centred**.
//!   The two readers genuinely disagree, which is why [`Justify`] stores the dword rather than a
//!   resolved enum: [`Justify::name_h`] is the getter's answer and [`Justify::paint_h`] is the
//!   draw path's, and they differ only in this state.
//! - **A non-token raises.** `0x6f1990` scans all six entries with `SStrCmpI` — whole-string,
//!   case-insensitive, **no trimming** — and returns 0 on a miss, which is the caller's cue to
//!   raise `Usage: %s:SetJustifyH("justify")` (`.rdata 0x87c77c`) / `…SetJustifyV…`
//!   (`0x87c7a0`). Per `script::binding_abi`, that arm does not return: it abandons the caller's
//!   statement.
//!
//! The setter also clears the inherit bit unconditionally, whether or not the axis value changed
//! (`0x79fc7c`).

use crate::script::{JustifyH, JustifyV};

/// `.rdata 0x811ad0` — six `{u32 bits, const char* name}` entries, in image order. The order is
/// load-bearing twice over: `0x6f1990` scans it linearly, and `0x6f1a00` answers with the **first**
/// entry whose bit is set.
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

/// The horizontal axis — bits 0–2, `SetJustifyH`'s mask.
pub const H_MASK: u32 = 0x07;
/// The vertical axis — bits 3–5, `SetJustifyV`'s mask.
pub const V_MASK: u32 = 0x38;

/// What a justify argument turned out to be — the three outcomes of `0x6f1990` followed by the
/// setter's mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Set<J> {
    /// A token carrying a bit on *this* axis: the new justification.
    To(J),
    /// A token that matched the table but carries no bit on this axis — `SetJustifyH("TOP")`.
    /// The reference **clears** the axis and raises nothing; `GetJustifyH()` then answers
    /// `"UNKNOWN"` while the glyphs keep drawing centred ([`Justify::paint_h`]).
    ///
    /// Not hypothetical: 13 corpus sites call `SetJustifyV("CENTER")` meaning "middle"
    /// (`FonzAppraiser` ×12, `Roid-Macros`), which parses and then erases the vertical axis.
    Clears,
    /// No entry matched. The caller raises its own `Usage:` string; it does not coerce.
    NoMatch,
}

/// The ctor default — `CENTER | MIDDLE | 0x200`. Bit 9 is outside both axis masks and is read by
/// neither accessor nor the gx translator. `CSimpleFontString`'s **own** ctor writes it at
/// `0x770dd3`, so a FontString that has never been told otherwise reads `CENTER`/`MIDDLE`.
const CTOR_DEFAULT: u32 = 0x212;

/// The justify **dword** — `CSimpleFont+0x54`, `CSimpleFontString+0x120`, `CSimpleEditBox`'s own —
/// with bits 0–2 horizontal and bits 3–5 vertical.
///
/// Stored as the dword rather than a pair of resolved enums because **an axis with no bit set is a
/// real, reachable state** and a resolved enum cannot hold it: `SetJustifyH("TOP")` parses `0x08`,
/// contributes nothing to mask `0x07`, and erases the axis with no error raised.
///
/// The two readers then disagree, faithfully, and that is the whole reason this type exists:
/// [`Justify::name_h`] (the Lua getter) answers `"UNKNOWN"`, while [`Justify::paint_h`] (the draw
/// path) answers `CENTER`. Anything that collapses the two loses one of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Justify(pub u32);

impl Default for Justify {
    fn default() -> Self {
        Justify(CTOR_DEFAULT)
    }
}

impl Justify {
    /// Replace the horizontal bits — `(cur & !0x07) | (parsed & 0x07)`.
    pub fn set_h(&mut self, j: JustifyH) {
        self.0 = set_axis(self.0, H_MASK, bits_h(j));
    }
    /// Replace the vertical bits — `(cur & !0x38) | (parsed & 0x38)`.
    pub fn set_v(&mut self, j: JustifyV) {
        self.0 = set_axis(self.0, V_MASK, bits_v(j));
    }
    /// Erase the horizontal axis, as a cross-axis token does.
    pub fn clear_h(&mut self) {
        self.0 = set_axis(self.0, H_MASK, 0);
    }
    /// Erase the vertical axis.
    pub fn clear_v(&mut self) {
        self.0 = set_axis(self.0, V_MASK, 0);
    }

    /// `GetJustifyH`'s answer — `0x6f1a00`, so `"UNKNOWN"` for a cleared axis.
    pub fn name_h(self) -> &'static str {
        name_of(self.0, H_MASK)
    }
    /// `GetJustifyV`'s answer.
    pub fn name_v(self) -> &'static str {
        name_of(self.0, V_MASK)
    }

    /// What the glyphs actually draw at horizontally — the ui→gx translator `0x44d420`, whose one
    /// call site image-wide is `0x772693`.
    ///
    /// **It is a priority ladder, not a switch, and the fall-through is `CENTER` — not `LEFT`.**
    /// Each axis's result register is pre-set to `1` *between* the `test` and the `jcc`
    /// (`0x44d4fb mov ebx,0x1`, then `0x44d516 je`), so an all-clear axis exits with `1` still in
    /// it, and `1` is bit-identical to CENTER. A cleared axis is therefore visually
    /// indistinguishable from the ctor default.
    ///
    /// **The trap, quoted from the finding:** the gx layer's `else` arms genuinely *are* LEFT and
    /// BOTTOM, but both are unreachable from a FontString — `0x44d420` emits only `{0,1,2}` and
    /// `0x5c1c30` rejects `>= 3`. *Mapping the ui bitmask onto the gx enum with a `0` default
    /// inverts both axes.*
    pub fn paint_h(self) -> JustifyH {
        if self.0 & 0x04 != 0 {
            JustifyH::Right
        } else if self.0 & 0x01 != 0 {
            JustifyH::Left
        } else {
            JustifyH::Center // 0x02 set, or NOTHING set — the pre-set `1`
        }
    }

    /// What the glyphs draw at vertically — the same ladder, `TOP > BOTTOM > MIDDLE`, falling
    /// through to `MIDDLE`. `0x44d536`'s redundant `mov esi,0x1` on the MIDDLE arm is MSVC's own
    /// tell that `default:` and `MIDDLE` share a constant.
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

/// `0x6f1990` — a linear scan of all six entries with `SStrCmpI`: **whole-string,
/// case-insensitive, and without trimming**, so `"left "` is a miss.
pub fn parse_bits(s: &str) -> Option<u32> {
    TOKENS
        .iter()
        .find(|(_, name)| s.eq_ignore_ascii_case(name))
        .map(|(bits, _)| *bits)
}

/// `0x6f1a00` — the **first** entry whose bit is set within `mask`, else the literal `UNKNOWN`.
pub fn name_of(bits: u32, mask: u32) -> &'static str {
    TOKENS
        .iter()
        .find(|(b, _)| bits & mask & b != 0)
        .map_or(UNKNOWN, |(_, name)| *name)
}

/// Replace one axis's bits with `parsed`'s, masked to that axis — `0x79fc5d`'s
/// `(cur ^ parsed) & mask ^ cur`, i.e. `(cur & !mask) | (parsed & mask)`. This is the operation
/// that makes a cross-axis token *clear* rather than raise.
pub fn set_axis(cur: u32, mask: u32, parsed: u32) -> u32 {
    (cur & !mask) | (parsed & mask)
}

/// `SetJustifyH`'s argument, resolved against mask `0x07`.
pub fn parse_h(s: &str) -> Set<JustifyH> {
    match parse_bits(s) {
        None => Set::NoMatch,
        Some(0x01) => Set::To(JustifyH::Left),
        Some(0x02) => Set::To(JustifyH::Center),
        Some(0x04) => Set::To(JustifyH::Right),
        Some(_) => Set::Clears,
    }
}

/// `SetJustifyV`'s argument, resolved against mask `0x38`.
pub fn parse_v(s: &str) -> Set<JustifyV> {
    match parse_bits(s) {
        None => Set::NoMatch,
        Some(0x08) => Set::To(JustifyV::Top),
        Some(0x10) => Set::To(JustifyV::Middle),
        Some(0x20) => Set::To(JustifyV::Bottom),
        Some(_) => Set::Clears,
    }
}

/// `GetJustifyH`'s answer for a resolved horizontal justification, through the same table.
pub fn name_h(j: JustifyH) -> &'static str {
    name_of(bits_h(j), H_MASK)
}

/// `GetJustifyV`'s answer for a resolved vertical justification, through the same table.
pub fn name_v(j: JustifyV) -> &'static str {
    name_of(bits_v(j), V_MASK)
}

/// The `Usage:` string `SetJustifyH` raises on a miss (`.rdata 0x87c77c`). The `%s` is spelled as
/// the widget type, matching the shipped convention in `script::font_block`.
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
        // `SStrCmpI` compares the whole string; the client does not trim, and neither do we —
        // the Font-object path used to, which is the divergence this module exists to end.
        assert_eq!(parse_h("left "), Set::NoMatch);
        assert_eq!(parse_h(" left"), Set::NoMatch);
        assert_eq!(parse_h("leftmost"), Set::NoMatch);
        assert_eq!(parse_h(""), Set::NoMatch);
    }

    #[test]
    fn an_unmatched_string_does_not_coerce_to_center() {
        // The whole point of the raise: the reference tells the addon it passed nonsense, where we
        // used to silently answer CENTER on both tables.
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
        // `0x6f1a00`'s two documented edges, neither reachable through our resolved enums but both
        // part of the law being transcribed: no bit in the axis answers the literal UNKNOWN, and a
        // dword with two bits set answers the *first* table entry rather than the largest.
        assert_eq!(name_of(0x00, H_MASK), "UNKNOWN");
        assert_eq!(name_of(0x38, H_MASK), "UNKNOWN"); // vertical bits only
        assert_eq!(name_of(0x05, H_MASK), "LEFT"); // LEFT | RIGHT
        assert_eq!(name_of(0x30, V_MASK), "MIDDLE"); // MIDDLE | BOTTOM
                                                     // The ctor default `0x212` reads CENTER/MIDDLE, and bit 9 is invisible to both axes.
        assert_eq!(name_of(0x212, H_MASK), "CENTER");
        assert_eq!(name_of(0x212, V_MASK), "MIDDLE");
    }
}
