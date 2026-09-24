//! Pins the M2 **PlayableAnimationLookup** table parse (decision 0082 — missing-animation-clip
//! resolution) against a real build-5875 model. `nPlayableAnimationLookup` is a fixed 203 across
//! the entire retail 1.12.1 M2 corpus, measured; row 6 is HumanMale's decisive example
//! (`playableAnimationLookup[6] = 0x00030001`). Skips when the gitignored
//! client data isn't present.

use benilla_formats::{open_chain, parse_m2_playable_animation_lookup};

#[test]
fn humanmale_playable_animation_lookup_matches_the_byte_verified_shape() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open chain");
    let bytes = chain
        .read("character\\human\\male\\humanmale.m2")
        .expect("read m2");
    let pal = parse_m2_playable_animation_lookup(&bytes).expect("parse playable animation lookup");

    // `nPlayableAnimationLookup` is a fixed 203 across the retail corpus, measured — the array is
    // sized to `AnimationData.dbc`'s playable set, identically for every
    // model regardless of its own sequence count.
    assert_eq!(pal.len(), 203);

    // Identity entries: HumanMale actually plays Stand(0)/Death(1)/WalkBackwards(3)/Walk(4)/Run(5)
    // itself, so each row maps back to its own id with no direction code.
    for id in [0u16, 1, 3, 4, 5] {
        let row = pal[id as usize];
        assert_eq!(row.resolved_id, id, "row {id} should be identity");
        assert_eq!(row.dir_flags, 0, "row {id} should carry no dir-flags code");
    }

    // PATH 2's DBC Fallback-column walk (`0x711bf0`): row 6 packs `0x00030001` — resolved id 1
    // (Death), dir-flags code 3 — computed by hand-replaying the DBC Fallback walk (row 6:
    // Fallback=1, Flags=0x28) and shown bit-for-bit identical to this baked entry. The single
    // strongest real-asset anchor for the whole mechanism.
    assert_eq!(pal[6].resolved_id, 1, "row 6 -> Death, the DBC-walk proof");
    assert_eq!(pal[6].dir_flags, 3, "row 6's direction/variant code");

    // A genuine fallback entry away from the DBC-walk showcase row: HumanMale plays every attack
    // (2H/1H/unarmed all present), so pick a row known to fall back rather than resolve to itself —
    // row 32 substitutes AttackUnarmed(16) for whatever id 32 requests.
    assert_eq!(pal[32].resolved_id, 16, "row 32 substitutes AttackUnarmed");
}

/// The **prowl clips** the stealth gait branch asks for (`creature_anim::select`'s `STEALTH_WALK` /
/// `STEALTH_STAND`, `[[110]+0x213]&2` branches): a player model authors both, and a
/// creature model may author NEITHER — in which case the same baked lookup the real client indexes
/// steps 119 down to Walk and 120 to Stand. That asymmetry is why the selector's stealth branch needs
/// no model-capability check of its own, and why a prowling druid cat shows its ordinary walk on the
/// reference client too.
#[test]
fn the_stealth_clips_are_authored_by_players_and_absent_from_the_druid_cat() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open chain");
    let pal = |path: &str| {
        parse_m2_playable_animation_lookup(&chain.read(path).expect("read m2")).expect("parse pal")
    };

    // Rogue/druid bodies: StealthWalk(119) and StealthStand(120) resolve to themselves.
    for model in [
        "character\\human\\male\\humanmale.m2",
        "character\\nightelf\\female\\nightelffemale.m2",
    ] {
        let p = pal(model);
        assert_eq!(p[119].resolved_id, 119, "{model} authors StealthWalk");
        assert_eq!(p[120].resolved_id, 120, "{model} authors StealthStand");
    }

    // Cat form authors neither — the baked walk lands on Walk / Stand.
    let cat = pal("creature\\druidcat\\druidcat.m2");
    assert_eq!(cat[119].resolved_id, 4, "cat StealthWalk -> Walk");
    assert_eq!(cat[120].resolved_id, 0, "cat StealthStand -> Stand");
}
