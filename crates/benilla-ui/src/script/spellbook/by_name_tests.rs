//! `CastSpellByName(name [, onSelf])` (`0x4b4ab0`) and the parse it shares with a macro's bound
//! spell (`0x4b3950` → `0x4b3a10`, read through `0x4b3bc0`), at the Lua surface.

use super::{resolve_spell_in_books, PetBookState, SpellBookState, SpellSlotView};
use crate::script::UiScript;

fn slot(spell_id: u32, name: &str, rank: Option<&str>) -> SpellSlotView {
    SpellSlotView {
        spell_id,
        name: name.into(),
        rank: rank.map(Into::into),
        ..Default::default()
    }
}

/// Fireball's three ranks in book order (`0x4b30c0`: name, then the rank's number), and Frost
/// Armor.
fn ranked() -> SpellBookState {
    SpellBookState {
        tabs: Vec::new(),
        slots: vec![
            slot(133, "Fireball", Some("Rank 1")),
            slot(143, "Fireball", Some("Rank 2")),
            slot(145, "Fireball", Some("Rank 3")),
            slot(168, "Frost Armor", Some("Rank 1")),
        ],
    }
}

/// A VM with `book` and `pet` pushed that keeps its calls for the `take_*` readers.
fn vm(book: SpellBookState, pet: PetBookState) -> UiScript {
    let mut s = UiScript::new().unwrap();
    s.keep_calls();
    s.set_spellbook(book);
    s.set_pet_book(pet);
    s
}

/// The player casts `CastSpellByName(args)` makes, as `(spell id, onSelf)`.
fn casts(s: &mut UiScript, args: &str) -> Vec<(u32, bool)> {
    s.run(&format!("CastSpellByName({args})"))
        .unwrap_or_else(|e| panic!("CastSpellByName({args}): {e}"));
    assert!(
        s.take_pet_spell_casts().is_empty(),
        "CastSpellByName({args}): no pet cast"
    );
    s.take_spell_casts()
}

/// `0x4b3950` cuts the copy at the last `(` (`0x64a450`, at `0x4b3985`; the cut at `0x4b398e`)
/// and trims nothing, so a space before the parenthesis stays in the name and matches no spell;
/// the rank runs to the first `)` after it (`0x4b39c6`) or to the end (`0x4b39c4`), and `0x4b3a10`
/// compares both whole, ignoring case (`SStrCmpI` `0x64a4c0`). An empty rank matches any rank
/// (`0x4b3a74`-`0x4b3a7f`), and the search runs from the last slot down (`0x4b3a37`), so a bare
/// name takes the book's last, highest, rank.
#[test]
fn cast_spell_by_name_parses_the_name_as_the_reference_does() {
    let mut s = vm(ranked(), PetBookState::default());
    for (arg, want) in [
        (r#""Fireball""#, Some(145)),
        (r#""fireball""#, Some(145)),
        (r#""Fireball(Rank 1)""#, Some(133)),
        (r#""FIREBALL(rank 2)""#, Some(143)),
        (r#""Fireball()""#, Some(145)),
        // No `)`: the rank runs to the end.
        (r#""Fireball(Rank 1""#, Some(133)),
        // The first `)` ends the rank.
        (r#""Fireball(Rank 1)x)""#, Some(133)),
        (r#""Fireball(Rank 1) ""#, Some(133)),
        // The last `(` ends the name.
        (r#""Fireball(Rank 1)(Rank 2)""#, None),
        // No trimming anywhere.
        (r#""Fireball (Rank 1)""#, None),
        (r#"" Fireball""#, None),
        (r#""Fireball ""#, None),
        (r#""Fireball( Rank 1 )""#, None),
        (r#""Fireball(Rank 1 )""#, None),
        (r#""(Rank 1)""#, None),
        (r#""""#, None),
    ] {
        assert_eq!(
            casts(&mut s, arg),
            want.map(|id| vec![(id, false)]).unwrap_or_default(),
            "CastSpellByName({arg})"
        );
    }
}

/// The verb reads its text as a C string (`lua_tostring`, `0x4b4b0a`), so it ends at the first
/// NUL; the copy keeps 1023 bytes (`SStrCopy(buf, text, 0x400)`, `0x4b3978`), so a `(` past them
/// is gone; and the rank is walked by UTF-8 character (`0x41aab0`, at `0x4b39bc`), which decodes
/// the overlong `C0 A9` as `)` and ends the rank there.
#[test]
fn the_name_is_a_c_string_in_a_kilobyte_buffer_walked_by_utf8_character() {
    let mut s = vm(ranked(), PetBookState::default());
    assert_eq!(casts(&mut s, r#""Fireball\0(Rank 1)""#), vec![(145, false)]);
    assert_eq!(
        casts(&mut s, r#""Fireball(Rank 1\192\169""#),
        vec![(133, false)]
    );
    // `Fireball(Rank 1)`, padding to 1023 bytes and beyond, then a last `(`: the copy drops it.
    assert_eq!(
        casts(
            &mut s,
            r#""Fireball(Rank 1)" .. string.rep("x", 1100) .. "(Rank 2)""#
        ),
        vec![(133, false)]
    );
    // Within the copy, the same last `(` names a spell no book holds.
    assert_eq!(
        casts(
            &mut s,
            r#""Fireball(Rank 1)" .. string.rep("x", 900) .. "(Rank 2)""#
        ),
        vec![]
    );
}

/// `0x4b3a10` falls back to the pet's book (`0x4b3aad`, from its last slot), and a pet match
/// (`0x4b3b51`) goes to `0x4b3300`'s pet fork, `CMSG_PET_ACTION` (`0x4b34ce`), as
/// `CastSpell(id, "pet")` does, with `onSelf` swapping the target for the player (`0x4b4afa`).
#[test]
fn cast_spell_by_name_casts_a_name_only_the_pets_book_holds_as_a_pet_spell() {
    let pet = PetBookState {
        token: Some("DEMON".into()),
        slots: vec![
            slot(6358, "Seduction", Some("Rank 1")),
            slot(19647, "Spell Lock", Some("Rank 2")),
            slot(168, "Frost Armor", None),
        ],
    };
    let mut s = vm(ranked(), pet);
    s.run(r#"CastSpellByName("Seduction") CastSpellByName("spell lock(rank 2)", 1)"#)
        .unwrap();
    assert_eq!(s.take_pet_spell_casts(), vec![(6358, false), (19647, true)]);
    assert!(s.take_spell_casts().is_empty());
    // The player's book is searched first (`0x4b3a2d`).
    assert_eq!(casts(&mut s, r#""Frost Armor""#), vec![(168, false)]);
    // A rank the pet's spell lacks matches nothing in either book.
    s.run(r#"CastSpellByName("Seduction(Rank 2)")"#).unwrap();
    assert!(s.take_pet_spell_casts().is_empty());
}

/// `0x4b3a10` has no passive test, so a passive is matched like any spell: a passive in the pet's
/// book goes out through the pet fork (`0x4b34ce`), which tests nothing either, and one in the
/// player's book ends the search there and is refused by `TryCastSpell` (`0x6e4bb2`), silently,
/// even with a castable spell of that name earlier in the book or in the pet's.
#[test]
fn a_passive_is_matched_by_name_like_any_spell() {
    let mut player = ranked();
    player.slots.push(SpellSlotView {
        passive: true,
        ..slot(18459, "Fireball", Some("Rank 3"))
    });
    let pet = PetBookState {
        token: Some("PET".into()),
        slots: vec![
            SpellSlotView {
                passive: true,
                ..slot(3025, "Avoidance", None)
            },
            slot(133, "Fireball", Some("Rank 3")),
        ],
    };
    let mut s = vm(player, pet);
    s.run(r#"CastSpellByName("Avoidance")"#).unwrap();
    assert_eq!(s.take_pet_spell_casts(), vec![(3025, false)]);
    assert_eq!(casts(&mut s, r#""Fireball""#), vec![]);
    assert_eq!(casts(&mut s, r#""Fireball(Rank 3)""#), vec![]);
    assert_eq!(casts(&mut s, r#""Fireball(Rank 2)""#), vec![(143, false)]);
}

/// The text must be a string or a number (`lua_isstring`, `0x4b4abc`), else the verb raises
/// `Usage: CastSpellByName(name [, onSelf])` (`0x846adc`); a number is its text.
#[test]
fn cast_spell_by_name_takes_a_string_or_a_number() {
    let mut s = vm(ranked(), PetBookState::default());
    for bad in [
        "CastSpellByName()",
        "CastSpellByName(nil)",
        "CastSpellByName({})",
    ] {
        let err = s.run(bad).expect_err(bad).to_string();
        assert!(
            err.contains("Usage: CastSpellByName(name [, onSelf])"),
            "{bad}: {err}"
        );
    }
    let mut book = ranked();
    book.slots.push(slot(7, "1", None));
    s.set_spellbook(book);
    assert_eq!(casts(&mut s, "1"), vec![(7, false)]);
}

/// A macro's bound spell resolves as the verb does (`0x4b3bc0` → `0x4b3950`), the book it came
/// from included.
#[test]
fn the_bound_spell_resolves_through_the_same_parse() {
    let pet = PetBookState {
        token: None,
        slots: vec![slot(6358, "Seduction", Some("Rank 1"))],
    };
    let player = ranked();
    let resolve =
        |text: &str| resolve_spell_in_books(&player, &pet, text).map(|n| (n.spell_id, n.pet));
    assert_eq!(resolve("Fireball"), Some((145, false)));
    assert_eq!(resolve("Fireball(Rank 1"), Some((133, false)));
    assert_eq!(resolve("Fireball (Rank 1)"), None);
    assert_eq!(resolve("Fireball(Rank 1)(Rank 2)"), None);
    assert_eq!(resolve("Seduction()"), Some((6358, true)));
}
