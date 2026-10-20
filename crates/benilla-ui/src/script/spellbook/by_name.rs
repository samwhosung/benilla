//! A spell named by its text, `Name(Rank N)`, as `CastSpellByName` (`0x4b4ab0`) and a macro's
//! bound spell (`0x4b3bc0`) resolve it: `0x4b3950` splits the text, `0x4b3a10` searches both books.

use super::{PetBookState, SpellBookState, SpellSlotView};

/// `0x4b3950`'s name buffer, `SStrCopy(buf, text, 0x400)` (`0x4b3978`): 1023 bytes and a NUL.
const NAME_BUFFER: usize = 0x400;

/// `0x4b3950`'s rank buffer, `SStrCopy(rank, after, 0x200)` (`0x4b399f`).
const RANK_BUFFER: usize = 0x200;

/// A name resolved against both books: the spell, the book it came from, and its passivity, which
/// the player's cast refuses (`0x6e4bb2`) and the pet's order does not test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedSpell {
    pub spell_id: u32,
    /// The pet's book, not the player's (`0x4b3b51` writes 1, `0x4b3b3d` writes 0).
    pub pet: bool,
    pub passive: bool,
}

/// The spell `text` names, by `0x4b3950`'s split ([`split_name_rank`]) and `0x4b3a10`'s search
/// ([`find_in_books`]): what `CastSpellByName` casts and a macro's bound spell reads.
pub fn resolve_spell_in_books(
    player: &SpellBookState,
    pet: &PetBookState,
    text: &str,
) -> Option<NamedSpell> {
    resolve_named(player, pet, text.as_bytes())
}

/// [`resolve_spell_in_books`] on the bytes `lua_tostring` hands the verb (`0x4b4b0a`).
pub(super) fn resolve_named(
    player: &SpellBookState,
    pet: &PetBookState,
    text: &[u8],
) -> Option<NamedSpell> {
    let (name, rank) = split_name_rank(text);
    let (slot, pet) = find_in_books(&player.slots, &pet.slots, name, rank)?;
    Some(NamedSpell {
        spell_id: slot.spell_id,
        pet,
        passive: slot.passive,
    })
}

/// `0x4b3950`'s split of a C string, which ends at its first NUL, into the name and the rank:
///
/// - the text is copied into a 1024-byte buffer, keeping 1023 bytes (`0x4b3978`);
/// - the last `(` in the copy ends the name (`SStrChrR` `0x64a450` at `0x4b3985`, the cut at
///   `0x4b398e`), and nothing is trimmed, so `Fireball (Rank 1)` names `Fireball `;
/// - what follows is copied into a 512-byte buffer (`0x4b399f`) and walked one UTF-8 character
///   at a time (`0x41aab0`) to the first `)` (`0x4b39c6`), else to its end (`0x4b39c4`);
/// - with no `(` the rank is empty (`0x4b39a6`), as is `()`.
fn split_name_rank(text: &[u8]) -> (&[u8], &[u8]) {
    let text = &text[..text.iter().position(|&b| b == 0).unwrap_or(text.len())];
    let text = &text[..text.len().min(NAME_BUFFER - 1)];
    let Some(open) = text.iter().rposition(|&b| b == b'(') else {
        return (text, &[]);
    };
    let after = &text[open + 1..];
    let after = &after[..after.len().min(RANK_BUFFER - 1)];
    let mut at = 0;
    while let Some((code, read)) = sgetu8(&after[at..]) {
        if code == u32::from(b')') {
            return (&text[..open], &after[..at]);
        }
        at += read;
    }
    (&text[..open], after)
}

/// `sgetu8` (`0x41aab0`): the code point at the head of `s` and the bytes it read, `None` at the
/// end (its -1). A lead byte of `10xxxxxx`, `FE` or `FF` reads `0x80000000` in one byte; a byte
/// that is no continuation reads `0x80000000` too, counted with the bytes before it; an overlong
/// form decodes as it stands, so `C0 A9` is `)`.
fn sgetu8(s: &[u8]) -> Option<(u32, usize)> {
    const BAD: u32 = 0x8000_0000;
    let lead = *s.first()?;
    let (code, extra) = match lead {
        0x00..=0x7f => return Some((u32::from(lead), 1)),
        0xc0..=0xdf => (lead & 0x1f, 1),
        0xe0..=0xef => (lead & 0x0f, 2),
        0xf0..=0xf7 => (lead & 0x07, 3),
        0xf8..=0xfb => (lead & 0x03, 4),
        0xfc..=0xfd => (lead & 0x01, 5),
        0x80..=0xbf | 0xfe..=0xff => return Some((BAD, 1)),
    };
    let mut code = u32::from(code);
    for read in 1..=extra {
        // The string's NUL inside a sequence is the -1 (`0x41ab6b`).
        let byte = *s.get(read)?;
        if byte & 0xc0 != 0x80 {
            return Some((BAD, read + 1));
        }
        code = code << 6 | u32::from(byte & 0x3f);
    }
    Some((code, extra + 1))
}

/// `0x4b3a10`: the player's book (`0x4b3a2d`), then the pet's (`0x4b3aad`), each from its last
/// slot down (`0x4b3a37`, `0x4b3ac0`), for the first spell whose name equals `name` and, when
/// `rank` is not empty (`0x4b3a74`-`0x4b3a7f`), whose rank equals `rank`, both compared whole and
/// ignoring ASCII case (`SStrCmpI` `0x64a4c0`, at `0x4b3a6b` and `0x4b3a99`). It has no passive
/// test, and an empty name finds nothing (`0x4b3a1e`-`0x4b3a27`). The book order (`0x4b30c0`:
/// name, then the rank's number) puts a name's highest rank last, so a bare name takes it.
fn find_in_books<'a>(
    player: &'a [SpellSlotView],
    pet: &'a [SpellSlotView],
    name: &[u8],
    rank: &[u8],
) -> Option<(&'a SpellSlotView, bool)> {
    if name.is_empty() {
        return None;
    }
    let named = |s: &&SpellSlotView| {
        s.name.as_bytes().eq_ignore_ascii_case(name)
            && (rank.is_empty()
                || s.rank
                    .as_deref()
                    .unwrap_or_default()
                    .as_bytes()
                    .eq_ignore_ascii_case(rank))
    };
    player
        .iter()
        .rev()
        .find(named)
        .map(|s| (s, false))
        .or_else(|| pet.iter().rev().find(named).map(|s| (s, true)))
}

#[cfg(test)]
mod tests {
    use super::{sgetu8, split_name_rank};

    /// `0x41aab0`'s reads, one class of lead each.
    #[test]
    fn sgetu8_reads_as_the_reference_does() {
        assert_eq!(sgetu8(b""), None);
        assert_eq!(sgetu8(b")x"), Some((0x29, 1)));
        assert_eq!(sgetu8("é".as_bytes()), Some((0xe9, 2)));
        assert_eq!(sgetu8("€".as_bytes()), Some((0x20ac, 3)));
        assert_eq!(sgetu8(b"\xc0\xa9"), Some((0x29, 2)), "overlong");
        assert_eq!(
            sgetu8(b"\xa9)"),
            Some((0x8000_0000, 1)),
            "a stray continuation"
        );
        assert_eq!(sgetu8(b"\xff"), Some((0x8000_0000, 1)));
        assert_eq!(
            sgetu8(b"\xc3)"),
            Some((0x8000_0000, 2)),
            "the `)` is read with it"
        );
        assert_eq!(sgetu8(b"\xe2\x82"), None, "the end inside a sequence");
    }

    #[test]
    fn a_bad_sequence_swallows_the_byte_after_it() {
        assert_eq!(
            split_name_rank(b"Fireball(Rank 1\xc3)x)"),
            (&b"Fireball"[..], &b"Rank 1\xc3)x"[..])
        );
    }

    /// The rank copy keeps 511 bytes (`0x4b399f`), so a `)` past them is never reached.
    #[test]
    fn the_rank_buffer_holds_511_bytes() {
        let mut text = b"Fireball(".to_vec();
        text.extend(std::iter::repeat_n(b'r', 511));
        text.push(b')');
        let (name, rank) = split_name_rank(&text);
        assert_eq!(name, b"Fireball");
        assert_eq!(rank.len(), 511);
        assert!(rank.iter().all(|&b| b == b'r'));
    }
}
