//! The list's flush sites and the values its inserts take, each pinned to the reference's site.

use std::time::{Duration, Instant};

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::{ItemUseSpell, SpellCooldown};

use super::Cooldowns;
use crate::spell::{CategoryPad, SpellModifiers, OP_COOLDOWN, OP_GCD};

fn spell(
    category: u32,
    recovery_ms: u32,
    category_recovery_ms: u32,
    gcd: (u32, u32),
    attributes: u32,
) -> SpellDisplay {
    SpellDisplay {
        category,
        recovery_ms,
        category_recovery_ms,
        start_recovery_category: gcd.0,
        start_recovery_ms: gcd.1,
        attributes,
        ..Default::default()
    }
}

/// Fireball-shaped: no cooldown of its own, the ordinary 133/1500 GCD.
fn fireball() -> SpellDisplay {
    spell(0, 0, 0, (133, 1500), 0x10000)
}

/// Charge-shaped: category 44 for 15 s, no GCD.
fn charge() -> SpellDisplay {
    spell(44, 0, 15_000, (0, 0), 0)
}

/// `d` in family 3 under bit 5, the bit [`mods`] modifies.
fn in_family(mut d: SpellDisplay) -> SpellDisplay {
    d.spell_family = 3;
    d.spell_family_flags = 1 << 5;
    d
}

/// A table whose op `op` adds `flat` to a family-3 bit-5 spell.
fn mods(op: u8, flat: i32) -> SpellModifiers {
    let mut m = SpellModifiers::default();
    m.set_class_family(3);
    m.set(true, 5, op, flat);
    m
}

fn player(ranged_attack_time_ms: u32) -> CategoryPad {
    CategoryPad::Player {
        ranged_attack_time_ms,
    }
}

/// `0x6e9460` flushes once at its end (`0x6e95b0`): with no pairs `0x6e94b3` jumps there, and a pair
/// whose spell `Spell.dbc` lacks inserts nothing (`0x6e94f8`) and goes on to it.
#[test]
fn a_spell_cooldown_packet_flushes_once_whatever_it_inserts() {
    let now = Instant::now();
    let none = SpellModifiers::default();
    let mut cds = Cooldowns::default();
    cds.apply_spell_cooldowns(
        std::iter::empty::<(u32, u32, Option<&SpellDisplay>)>(),
        &none,
        CategoryPad::NoPlayer,
        now,
    );
    assert_eq!(cds.generation, 1, "no pairs");
    cds.apply_spell_cooldowns([(4242, 5_000, None)], &none, CategoryPad::NoPlayer, now);
    assert_eq!(cds.generation, 2, "an unknown spell's pair");
    assert_eq!(cds.record_count(), 0, "inserts nothing");
    let fb = fireball();
    cds.apply_spell_cooldowns(
        [(133, 8_000, Some(&fb)), (116, 8_000, Some(&fb))],
        &none,
        CategoryPad::NoPlayer,
        now,
    );
    assert_eq!(cds.generation, 3, "one flush for the packet's two inserts");
}

/// `0x6e9460`'s values. A zero duration takes `RecoveryTime` through op 11 (`0x6e9516`); the category
/// time, the spell's for a zero duration and else 0 (`0x6e952b`-`0x6e953b`), goes through `0x6e2b60`
/// (`0x6e9544`), so on the player's list a ranged spell's takes the ranged pad whatever the
/// duration, then op 11, and the pet's list skips the pad; the GCD takes op 21 (`0x6e9562`).
#[test]
fn a_spell_cooldown_entry_takes_op_11_the_ranged_pad_and_op_21() {
    let now = Instant::now();
    // Fire Blast with 3/3 Improved Fire Blast: 8 s less 1.5 s.
    let fire_blast = in_family(spell(0, 8_000, 0, (0, 0), 0));
    let mut cds = Cooldowns::default();
    cds.apply_spell_cooldowns(
        [(2136, 0, Some(&fire_blast))],
        &mods(OP_COOLDOWN, -1_500),
        player(0),
        now,
    );
    assert_eq!(
        cds.info(2136, 0, Some(&fire_blast), now).duration_ms,
        6_500,
        "op 11 on a zero duration's RecoveryTime"
    );

    // Throw: category 76, no DBC times, `Attributes` 0x2; a sibling reads the category timer.
    let throw = spell(76, 0, 0, (0, 0), 0x2);
    let sibling = spell(76, 0, 0, (0, 0), 0);
    let none = SpellModifiers::default();
    for (pad, ms, want) in [
        (player(2_000), 0, 2_000),
        (player(2_000), 5_000, 2_000),
        (CategoryPad::Skip, 0, 0),
        (CategoryPad::NoPlayer, 0, 0),
    ] {
        let mut cds = Cooldowns::default();
        cds.apply_spell_cooldowns([(2764, ms, Some(&throw))], &none, pad, now);
        assert_eq!(
            cds.info(999, 0, Some(&sibling), now).remaining_ms,
            want,
            "{pad:?}, a duration of {ms}"
        );
    }

    // Op 21 halves the GCD a sibling of its category reads.
    let mut halved = SpellModifiers::default();
    halved.set_class_family(3);
    halved.set(false, 5, OP_GCD, -50);
    let fb = in_family(fireball());
    let mut cds = Cooldowns::default();
    cds.apply_spell_cooldowns([(133, 8_000, Some(&fb))], &halved, player(0), now);
    assert_eq!(cds.info(116, 0, Some(&fireball()), now).duration_ms, 750);
}

/// `StartGlobalCooldown 0x6e2de0` returns early only for a zero result of op 21 (`0x6e2e3d`): a
/// negative one is inserted (`0x6e2e72`), as the insert's early-out tests the GCD time for zero
/// alone (`0x6e12db`), and flushed (`0x6e2e77`). Its GCD ends before its start, so no read shows
/// it (`0x6e15df`), but a failed cast of the spell finds the record and flushes (`0x6e1658`,
/// `0x6e167d`); a held spell's record parks.
#[test]
fn a_negative_global_cooldown_still_inserts_and_flushes() {
    let now = Instant::now();
    let m = mods(OP_GCD, -2_000);
    let fb = in_family(fireball());
    let mut cds = Cooldowns::default();
    cds.start_gcd(133, &fb, now, &m);
    assert_eq!(
        (cds.record_count(), cds.generation),
        (1, 1),
        "the record and the flush"
    );
    assert_eq!(cds.info(133, 0, Some(&fb), now).remaining_ms, 0, "no GCD");
    cds.clear_gcd(133, now);
    assert_eq!(cds.generation, 2, "a failed cast finds the record");
    let held = in_family(spell(0, 0, 0, (133, 1_500), 0x0200_0000));
    cds.start_gcd(1784, &held, now, &m);
    assert!(cds.has_on_hold_record(1784, 0, 0), "parked");
    assert_eq!(
        cds.usable_refreshes, 1,
        "with the usable refresh (0x6e2e85)"
    );
}

/// `ClearGlobalCooldown 0x6e1630` matches any record of the spell (`0x6e1658`), whatever its GCD,
/// then flushes (`0x6e167d`); a spell with no record flushes nothing (`0x6e166e`).
#[test]
fn a_failed_cast_flushes_for_any_record_of_its_spell() {
    let t0 = Instant::now();
    let mut cds = Cooldowns::default();
    cds.start_spell(100, &charge(), 0, t0, None);
    let before = cds.generation;
    cds.clear_gcd(999, t0);
    assert_eq!(cds.generation, before, "no record of the spell");
    cds.clear_gcd(100, t0);
    assert_eq!(cds.generation, before + 1, "Charge's record has no GCD");
    assert_eq!(
        cds.info(100, 0, Some(&charge()), t0).remaining_ms,
        15_000,
        "and keeps its own cooldown"
    );
}

/// The go's item leg takes the item block's category only when positive, else the spell's
/// (`0x6e8523`-`0x6e8534`): Flask of Port's block carries 0 and its spell 59, so the flask and a
/// sibling of category 59 read the record.
#[test]
fn an_item_block_of_category_0_records_the_spells_category() {
    let now = Instant::now();
    let port = spell(59, 0, 10_000, (0, 0), 0);
    let block = ItemUseSpell {
        spell_id: 11008,
        cooldown_ms: -1,
        category: 0,
        category_cooldown_ms: -1,
    };
    let mut cds = Cooldowns::default();
    cds.start_item(2593, &block, Some(&port), now);
    assert_eq!(
        cds.info_item(11008, 2593, 0, Some(&port), now).remaining_ms,
        10_000,
        "the flask"
    );
    let sibling = spell(59, 0, 0, (0, 0), 0);
    assert_eq!(
        cds.info_item(11009, 2594, 0, Some(&sibling), now)
            .remaining_ms,
        10_000,
        "another drink of category 59"
    );
}

fn wire(
    spell_id: u16,
    item_id: u16,
    category: u16,
    spell_ms: u32,
    category_ms: u32,
) -> SpellCooldown {
    SpellCooldown {
        spell_id,
        item_id,
        category,
        spell_cd_ms: spell_ms,
        category_cd_ms: category_ms,
    }
}

/// `StartCooldown 0x6e2c60`, which the initial spells seed through, calls no flush.
#[test]
fn an_initial_cooldown_flushes_nothing() {
    let now = Instant::now();
    let last_stand = spell(0, 600_000, 0, (0, 0), 0);
    let mut cds = Cooldowns::default();
    cds.seed_initial(
        &wire(12975, 0, 0, 240_000, 0),
        Some(&last_stand),
        None,
        &SpellModifiers::default(),
        CategoryPad::NoPlayer,
        now,
    );
    assert_eq!(cds.record_count(), 1);
    assert_eq!(cds.generation, 0);
}

/// `ReadInitialSpells 0x5e6510` splits bit 31 off the category word as `held`
/// (`0x5e6632`-`0x5e664b`) and seeds through `StartCooldown 0x6e2c60`: the time is the larger of the
/// base and the remainder through op 11 (`0x6e2d31`-`0x6e2d40`), started so it ends at the remainder
/// (`0x6e2d4e`); the category side the same through `0x6e2b60` (`0x6e2d80`-`0x6e2d94`); the item
/// slot's own times are the bases (`0x6e2d23`, `0x6e2d66`); a held record parks (`0x6e2dab`).
#[test]
fn an_initial_cooldown_resumes_its_sweep_and_parks_when_held() {
    let now = Instant::now();
    let none = SpellModifiers::default();
    let seed = |cds: &mut Cooldowns,
                cd: SpellCooldown,
                d: &SpellDisplay,
                block: Option<&benilla_protocol::messages::ItemSpellEntry>,
                m: &SpellModifiers| {
        cds.seed_initial(&cd, Some(d), block, m, CategoryPad::NoPlayer, now);
    };

    // Last Stand, 10 minutes with 4 left: started 6 minutes ago.
    let last_stand = spell(0, 600_000, 0, (0, 0), 0);
    let mut cds = Cooldowns::default();
    seed(
        &mut cds,
        wire(12975, 0, 0, 240_000, 0),
        &last_stand,
        None,
        &none,
    );
    let read = cds.info(12975, 0, Some(&last_stand), now);
    assert_eq!((read.duration_ms, read.remaining_ms), (600_000, 240_000));
    assert_eq!(now.duration_since(read.start), Duration::from_secs(360));

    // Op 11 on the larger: 8 minutes, still ending at the remainder.
    let modded = in_family(spell(0, 600_000, 0, (0, 0), 0));
    let mut cds = Cooldowns::default();
    seed(
        &mut cds,
        wire(12975, 0, 0, 240_000, 0),
        &modded,
        None,
        &mods(OP_COOLDOWN, -120_000),
    );
    let read = cds.info(12975, 0, Some(&modded), now);
    assert_eq!((read.duration_ms, read.remaining_ms), (480_000, 240_000));

    // A potion's category: 2 minutes with 1 left, read by a sibling of category 4.
    let potion = spell(4, 0, 120_000, (0, 0), 0);
    let mut cds = Cooldowns::default();
    seed(&mut cds, wire(439, 0, 4, 0, 60_000), &potion, None, &none);
    let read = cds.info(440, 0, Some(&spell(4, 0, 0, (0, 0), 0)), now);
    assert_eq!((read.duration_ms, read.remaining_ms), (120_000, 60_000));

    // An item slot's own 20 s is the base over the spell's nothing.
    let block = benilla_protocol::messages::ItemSpellEntry {
        index: 0,
        spell_id: 11008,
        trigger: 0,
        charges: 0,
        cooldown_ms: 20_000,
        category: 0,
        category_cooldown_ms: -1,
    };
    let drink = spell(0, 0, 0, (0, 0), 0);
    let mut cds = Cooldowns::default();
    seed(
        &mut cds,
        wire(11008, 2593, 0, 5_000, 0),
        &drink,
        Some(&block),
        &none,
    );
    let read = cds.info(11008, 2593, Some(&drink), now);
    assert_eq!((read.duration_ms, read.remaining_ms), (20_000, 5_000));

    // vmangos's permanent cooldown: 1 ms and bit 31 (`Player.cpp:3499-3500`). The record parks,
    // and the bit is no category time.
    let parked = spell(12, 10_000, 5_000, (0, 0), 0x0200_0000);
    let mut cds = Cooldowns::default();
    seed(
        &mut cds,
        wire(900, 0, 12, 1, 0x8000_0000),
        &parked,
        None,
        &none,
    );
    assert!(cds.has_on_hold_record(900, 0, 12), "held");
    assert!(!cds.info(900, 0, Some(&parked), now).enabled);
    assert_eq!(
        cds.info(901, 0, Some(&spell(12, 0, 0, (0, 0), 0)), now)
            .remaining_ms,
        0,
        "no category timer from the marker"
    );
}

/// The callback `0x6e2f70` on an awaited record: its arrival re-queries `0x6e2ed0` (`0x6e2f80`) and
/// flushes the pair and the bags while the item's on-use spell reads running or held
/// (`0x6e2f85`-`0x6e2f8e`), once, as the callback is spent; a refusal runs it with no record, which
/// returns (`0x6e2f78`).
#[test]
fn an_awaited_record_flushes_its_running_cooldown_once_it_lands() {
    use benilla_protocol::messages::ItemInfo;
    const HEARTHSTONE: u32 = 6948;
    const LINEN: u32 = 2589;
    const REFUSED: u32 = 4242;
    let on_use = |spell_id| ItemInfo {
        use_spell: Some(ItemUseSpell {
            spell_id,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        }),
        ..crate::items::test_template("Item")
    };
    let now = Instant::now();
    let (tx, _rx) = crossbeam_channel::unbounded();
    let net = crate::net::NetCommands(tx);
    let mut items = crate::items::Items::default();
    let _ = items.template(REFUSED, 0, crate::query_cache::Ask::New(&net));
    let mut cds = Cooldowns::default();
    cds.apply_wire_item_cooldown(HEARTHSTONE, 8690, now);
    cds.apply_wire_item_cooldown(REFUSED, 8690, now);
    for entry in [HEARTHSTONE, LINEN, REFUSED] {
        cds.await_item_record(entry);
    }
    let edges = |cds: &Cooldowns| (cds.generation, cds.bag_flushes);
    let before = edges(&cds);
    cds.answer_awaited_records(&items, None, now);
    assert_eq!(edges(&cds), before, "nothing landed");

    items.insert_template(LINEN, Some(on_use(7934)));
    items.insert_template(REFUSED, None);
    cds.answer_awaited_records(&items, None, now);
    assert_eq!(edges(&cds), before, "nothing running, and a refusal");
    items.insert_template(REFUSED, Some(on_use(8690)));
    cds.answer_awaited_records(&items, None, now);
    assert_eq!(edges(&cds), before, "the refusal spent its callback");
    items.insert_template(HEARTHSTONE, Some(on_use(8690)));
    cds.answer_awaited_records(&items, None, now);
    assert_eq!(
        edges(&cds),
        (before.0 + 1, before.1 + 1),
        "the hearthstone's record, on its running cooldown"
    );
    cds.answer_awaited_records(&items, None, now);
    assert_eq!(edges(&cds), (before.0 + 1, before.1 + 1), "once");
}
