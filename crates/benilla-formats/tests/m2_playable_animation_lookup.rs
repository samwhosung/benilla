//! The M2 `playableAnimationLookup`, which resolves an animation id the model lacks: 203 rows in
//! every shipped M2, sized to `AnimationData.dbc`'s playable set.

use benilla_formats::{open_chain, parse_m2_playable_animation_lookup};

#[test]
fn humanmale_playable_animation_lookup_matches_the_byte_verified_shape() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open chain");
    let bytes = chain
        .read("character\\human\\male\\humanmale.m2")
        .expect("read m2");
    let pal = parse_m2_playable_animation_lookup(&bytes).expect("parse playable animation lookup");

    assert_eq!(pal.len(), 203);

    // HumanMale authors Stand, Death, WalkBackwards, Walk and Run, so they map to themselves.
    for id in [0u16, 1, 3, 4, 5] {
        let row = pal[id as usize];
        assert_eq!(row.resolved_id, id, "row {id} should be identity");
        assert_eq!(row.dir_flags, 0, "row {id} should carry no dir-flags code");
    }

    // Row 6 packs `0x00030001`, Death with dir-flags 3, which the DBC Fallback walk (`0x711bf0`)
    // over row 6 (Fallback 1, Flags 0x28) reproduces.
    assert_eq!(pal[6].resolved_id, 1, "row 6 -> Death, the DBC-walk proof");
    assert_eq!(pal[6].dir_flags, 3, "row 6's direction/variant code");

    assert_eq!(pal[32].resolved_id, 16, "row 32 substitutes AttackUnarmed");
}

/// The stealth clips, 119 StealthWalk and 120 StealthStand: a player model authors both, and where
/// a creature authors neither the baked lookup steps them to Walk and Stand, so a prowling druid
/// cat walks as usual in the reference too.
#[test]
fn the_stealth_clips_are_authored_by_players_and_absent_from_the_druid_cat() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = open_chain(&data).expect("open chain");
    let pal = |path: &str| {
        parse_m2_playable_animation_lookup(&chain.read(path).expect("read m2")).expect("parse pal")
    };

    for model in [
        "character\\human\\male\\humanmale.m2",
        "character\\nightelf\\female\\nightelffemale.m2",
    ] {
        let p = pal(model);
        assert_eq!(p[119].resolved_id, 119, "{model} authors StealthWalk");
        assert_eq!(p[120].resolved_id, 120, "{model} authors StealthStand");
    }

    let cat = pal("creature\\druidcat\\druidcat.m2");
    assert_eq!(cat[119].resolved_id, 4, "cat StealthWalk -> Walk");
    assert_eq!(cat[120].resolved_id, 0, "cat StealthStand -> Stand");
}
