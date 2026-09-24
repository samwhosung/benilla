//! Tests for the PvP and honor bindings. Several pin reference facts that look like bugs, so each
//! is written to fail against the natural reading.

use super::*;
use crate::script::{UiScript, UnitState};

/// Every number distinct, so a swapped return cannot pass: the tens digit is the block (session
/// 1x to lifetime 5x), the units digit the position in it.
fn honor_state() -> HonorState {
    HonorState {
        session_hk: 11,
        session_dk: 12,
        yesterday_hk: 21,
        yesterday_dk: 22,
        yesterday_honor: 23,
        this_week_hk: 31,
        this_week_honor: 32,
        last_week_hk: 41,
        last_week_dk: 42,
        last_week_honor: 43,
        last_week_standing: 44,
        lifetime_hk: 51,
        lifetime_dk: 52,
        highest_rank: 11,
        rank: 9,
        // 51/255 is exactly 0.2, which the engine's multiply does not produce.
        rank_bar: 51,
    }
}

/// The inspect reply's twelve, distinct in return order, so the ninth (the standing) cannot hide
/// behind the eighth.
fn inspect_state() -> InspectHonorData {
    InspectHonorData {
        guid: 0xDEAD_BEEF,
        session_hk: 61,
        session_dk: 62,
        yesterday_hk: 63,
        yesterday_honor: 64,
        this_week_hk: 65,
        this_week_honor: 66,
        last_week_hk: 67,
        last_week_honor: 68,
        last_week_standing: 69,
        lifetime_hk: 70,
        lifetime_dk: 71,
        highest_rank: 12,
        rank_bar: 255,
    }
}

/// A female Alliance player at rank 9 (visual 5).
fn player() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Benilla".into()),
        sex: 3, // female
        is_player: true,
        // Both, and they differ: `faction_group` is the live faction template, `pvp_team` the
        // race walk (`0x5efe00`) that keys every rank title.
        faction_group: Some("Alliance".into()),
        pvp_team: 1,
        pvp_rank: 9,
        ..Default::default()
    }
}

fn seated() -> UiScript {
    let mut s = UiScript::new().expect("VM");
    s.set_honor(Some(honor_state()));
    s.set_unit("player", Some(player()));
    s
}

/// The rank titles `GlobalStrings.lua` puts on `_G`: both teams, one female twin, the dishonorable
/// end and rank 19. Rank 17 is left out as the key no locale has.
fn seat_rank_globals(s: &UiScript) {
    let g = s.lua().globals();
    for (key, value) in [
        ("PVP_RANK_1_1", "Pariah"),
        ("PVP_RANK_4_1", "Dishonored"),
        ("PVP_RANK_5_0", "Scout"),
        ("PVP_RANK_5_1", "Private"),
        ("PVP_RANK_9_0", "Senior Sergeant"),
        ("PVP_RANK_9_1", "Sergeant Major"),
        ("PVP_RANK_9_1_FEMALE", "Sergeant Major (f)"),
        ("PVP_RANK_18_1", "Grand Marshal"),
        ("PVP_RANK_19_1", "Leader"),
        ("PVP_RANK_19_1_FEMALE", "Leader (f)"),
    ] {
        g.set(key, value).expect("global string");
    }
}

/// The enUS GlobalStrings `UnitPVPName`'s decoration reads.
fn seat_name_globals(s: &UiScript) {
    let g = s.lua().globals();
    for (key, value) in [
        ("UNIT_PVP_NAME", "%s %s"),
        ("PVP_RANK_CIVILIAN", "Civilian"),
        ("PVP_MEDAL1", "Guardian of Stormwind"),
    ] {
        g.set(key, value).expect("global string");
    }
}

/// `HonorFrame.lua`'s own destructuring, line for line; the arities differ per period.
#[test]
fn the_reference_destructuring_lands_every_value_in_the_right_slot() {
    let s = seated();

    assert_eq!(
        s.eval::<(i64, i64)>("local hk, dk = GetPVPSessionStats() return hk, dk")
            .unwrap(),
        (11, 12)
    );
    assert_eq!(
        s.eval::<(i64, i64, i64)>(
            "local hk, dk, contribution = GetPVPYesterdayStats() return hk, dk, contribution"
        )
        .unwrap(),
        (21, 22, 23)
    );
    assert_eq!(
        s.eval::<(i64, i64)>(
            "local hk, contribution = GetPVPThisWeekStats() return hk, contribution"
        )
        .unwrap(),
        (31, 32)
    );
    assert_eq!(
        s.eval::<(i64, i64, i64, i64)>(
            "local hk, dk, contribution, rank = GetPVPLastWeekStats() \
             return hk, dk, contribution, rank"
        )
        .unwrap(),
        (41, 42, 43, 44),
        "the fourth return is last week's STANDING"
    );
    assert_eq!(
        s.eval::<(i64, i64, i64)>(
            "local hk, dk, highestRank = GetPVPLifetimeStats() return hk, dk, highestRank"
        )
        .unwrap(),
        (51, 52, 11)
    );
}

/// `0x51a843 cmp al,5; jb`: 4 vanishes, 5 survives.
#[test]
fn a_lifetime_rank_below_five_is_reported_as_zero() {
    let mut s = seated();
    let third = |s: &UiScript| {
        s.eval::<i64>("local _, _, highest = GetPVPLifetimeStats() return highest")
            .unwrap()
    };
    for (highest, reported) in [(0u8, 0i64), (1, 0), (4, 0), (5, 5), (6, 6), (18, 18)] {
        s.set_honor(Some(HonorState {
            highest_rank: highest,
            ..honor_state()
        }));
        assert_eq!(third(&s), reported, "highest lifetime rank {highest}");
    }

    // A number, not nil: the pane passes it to `GetPVPRankInfo`, which raises on a non-number.
    s.set_honor(Some(HonorState {
        highest_rank: 3,
        ..honor_state()
    }));
    assert_eq!(s.arity("GetPVPLifetimeStats()").unwrap(), 3);
    assert!(s
        .eval::<bool>(
            "local _, _, highest = GetPVPLifetimeStats() return type(highest) == 'number'"
        )
        .unwrap());
}

/// A getter returning one value too many passes the destructuring test; the count catches it.
#[test]
fn every_getter_returns_exactly_the_reference_arity() {
    let s = seated();
    for (call, width) in [
        ("GetPVPSessionStats()", 2),
        ("GetPVPYesterdayStats()", 3),
        ("GetPVPThisWeekStats()", 2),
        ("GetPVPLastWeekStats()", 4),
        ("GetPVPLifetimeStats()", 3),
        ("GetPVPRankProgress()", 1),
        ("GetPVPRankInfo(9)", 2),
        ("GetPVPRankInfo(0)", 2),
        ("UnitPVPRank('player')", 1),
    ] {
        assert_eq!(s.arity(call).unwrap(), width, "{call} arity");
    }
}

/// The reference feeds these straight into `format()`, where a nil would raise.
#[test]
fn the_self_getters_answer_zeroed_before_the_first_push() {
    let s = UiScript::new().expect("VM");
    assert_eq!(
        s.eval::<(i64, i64, i64, i64)>("return GetPVPLastWeekStats()")
            .unwrap(),
        (0, 0, 0, 0)
    );
    assert_eq!(s.eval::<f64>("return GetPVPRankProgress()").unwrap(), 0.0);
}

/// Written to fail against `byte / 255.0`: the constant is checked at its bits, and 51 and 255
/// must not land on 0.2 and 1.0.
#[test]
fn the_rank_bar_multiplies_by_the_f32_reciprocal_and_never_clamps() {
    // The four bytes at `0x8026c8`.
    assert_eq!(f64::from(f32::from_bits(0x3B80_8081)), RANK_BAR_SCALE);
    assert_ne!(RANK_BAR_SCALE, 1.0 / 255.0, "the f32 is NOT 1/255");

    let mut s = seated();
    let bar = |s: &UiScript| s.eval::<f64>("return GetPVPRankProgress()").unwrap();

    assert_eq!(bar(&s), 51.0 * RANK_BAR_SCALE);
    assert_ne!(bar(&s), 0.2, "a divisor would land exactly on 0.2");
    assert_ne!(bar(&s), 51.0 / 255.0);

    for byte in [0u8, 1, 51, 128, 254, 255] {
        s.set_honor(Some(HonorState {
            rank_bar: byte,
            ..honor_state()
        }));
        assert_eq!(bar(&s), f64::from(byte) * RANK_BAR_SCALE, "byte {byte}");
    }

    s.set_honor(Some(HonorState {
        rank_bar: 0,
        ..honor_state()
    }));
    assert_eq!(bar(&s), 0.0);
    s.set_honor(Some(HonorState {
        rank_bar: 255,
        ..honor_state()
    }));
    assert!(bar(&s) > 1.0, "255 * K = 1.0000000091389835, unclamped");
    assert_ne!(bar(&s), 1.0);

    // The inspect twin runs the identical kernel over the reply's byte (`0x51ab04`).
    s.set_inspect_honor(Some(inspect_state())); // rank_bar 255
    assert_eq!(
        s.eval::<f64>("return GetInspectPVPRankProgress()").unwrap(),
        255.0 * RANK_BAR_SCALE
    );
}

/// Runs the panes' own `NONE` fallback, not just the nil.
#[test]
fn rank_zero_names_nothing_and_the_pane_falls_back_to_none() {
    let s = seated();
    seat_rank_globals(&s);
    s.lua().globals().set("NONE", "None").expect("NONE");

    let (name, number) = s
        .eval::<(Option<String>, i64)>("local n, r = GetPVPRankInfo(0) return n, r")
        .unwrap();
    assert_eq!(name, None, "no PVP_RANK_0_* exists");
    assert_eq!(number, 0, "and the badge (rankNumber > 0) stays off");

    assert_eq!(
        s.eval::<String>(
            "local name = GetPVPRankInfo(0) if not name then name = NONE end return name"
        )
        .unwrap(),
        "None"
    );
}

/// `GetPVPRankInfo` passes gender 0 (`0x51aa0f`), so a female character gets the default title
/// with the `_FEMALE` twin right there on `_G`.
#[test]
fn the_pane_title_is_keyed_by_team_and_is_never_gendered() {
    let mut s = seated();
    seat_rank_globals(&s);

    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Sergeant Major"
    );
    assert_eq!(
        s.eval::<String>("return PVP_RANK_9_1_FEMALE").unwrap(),
        "Sergeant Major (f)",
        "the twin is on _G — the binding declines it, it does not miss it"
    );

    s.set_unit("player", Some(UnitState { sex: 2, ..player() }));
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Sergeant Major"
    );

    s.set_unit(
        "player",
        Some(UnitState {
            sex: 2,
            faction_group: Some("Horde".into()),
            pvp_team: 0,
            ..player()
        }),
    );
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Senior Sergeant"
    );
}

/// The credit line's lookup resolves through `0x612bf0` with the local player's gender, where the
/// binding passes 0, and has no range check, so rank 19 names "Leader" here.
#[test]
fn the_rust_side_title_lookup_is_gendered_where_the_binding_is_not() {
    let s = seated();
    seat_rank_globals(&s);

    assert_eq!(
        s.pvp_rank_title(9, 0, false).as_deref(),
        Some("Senior Sergeant")
    );
    assert_eq!(
        s.pvp_rank_title(9, 1, true).as_deref(),
        Some("Sergeant Major (f)"),
        "the credit line IS gendered"
    );
    // …and the binding, for that same female Alliance player, is not.
    assert_ne!(
        s.pvp_rank_title(9, 1, true),
        s.eval::<Option<String>>("return (GetPVPRankInfo(9))")
            .unwrap()
    );
    assert_eq!(
        s.pvp_rank_title(9, 1, false),
        s.eval::<Option<String>>("return (GetPVPRankInfo(9))")
            .unwrap(),
        "same key construction once the gender is out of it"
    );

    // Gendered means preferred: a rank with no twin falls back to the base key.
    assert_eq!(s.pvp_rank_title(5, 1, true).as_deref(), Some("Private"));

    assert_eq!(s.pvp_rank_title(19, 1, false).as_deref(), Some("Leader"));
    assert_eq!(s.pvp_rank_title(19, 1, true).as_deref(), Some("Leader (f)"));
    assert!(s.eval::<bool>("return GetPVPRankInfo(19) == nil").unwrap());

    assert_eq!(s.pvp_rank_title(0, 1, false), None);
    assert_eq!(s.pvp_rank_title(17, 1, false), None, "no PVP_RANK_17_1");
    s.lua().globals().set("PVP_RANK_17_1", "").expect("global");
    assert_eq!(
        s.pvp_rank_title(17, 1, false),
        None,
        "empty reads as absent"
    );
}

/// The panes' `if not rankName` fallback needs a nil, not an empty string or a raise.
#[test]
fn a_missing_rank_global_reads_nil_not_empty() {
    let s = seated(); // no rank globals seated at all
    assert!(s.eval::<bool>("return GetPVPRankInfo(9) == nil").unwrap());
    s.lua().globals().set("PVP_RANK_9_1", "").expect("global");
    assert!(s.eval::<bool>("return GetPVPRankInfo(9) == nil").unwrap());
    // A player with no side names nothing either, rather than picking a list.
    let mut s = s;
    s.set_unit(
        "player",
        Some(UnitState {
            pvp_team: -1,
            ..player()
        }),
    );
    seat_rank_globals(&s);
    assert!(s.eval::<bool>("return GetPVPRankInfo(9) == nil").unwrap());
}

/// `UnitFactionGroup` reads the live faction template (`0x516630`) and the title's team digit the
/// race (`0x5efe00`), so a vmangos GM (template 35, no side) keeps his title, in both the key and
/// `UnitPVPName`'s decoration (`0x5efe60`).
#[test]
fn a_sideless_player_still_has_a_team_digit_because_his_race_has_one() {
    let mut s = seated();
    seat_rank_globals(&s);
    s.lua()
        .globals()
        .set("UNIT_PVP_NAME", "%s %s")
        .expect("global");
    s.set_unit(
        "player",
        Some(UnitState {
            // `.gm on`: the template names nothing…
            faction_group: None,
            faction_group_localized: None,
            // …and the race still names Alliance.
            pvp_team: 1,
            sex: 2,
            pvp_rank: 18,
            ..player()
        }),
    );
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(18))").unwrap(),
        "Grand Marshal",
        "the sideless template must not reach the key"
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Grand Marshal Benilla",
        "and the name decoration reads the same digit"
    );
    // The control: a unit whose race has no side is -1, and -1 misses.
    s.set_unit(
        "player",
        Some(UnitState {
            faction_group: Some("Alliance".into()),
            pvp_team: -1,
            pvp_rank: 18,
            ..player()
        }),
    );
    assert!(
        s.eval::<bool>("return GetPVPRankInfo(18) == nil").unwrap(),
        "a side on the template cannot stand in for a missing team digit either"
    );
}

/// `0x51aa38` subtracts 5 rather than negating, so rank 1 is -4 and rank 4 is -1, the opposite of
/// vmangos's `visualRank` (`HonorMgr.cpp:991`).
#[test]
fn the_visual_rank_arithmetic_runs_backwards_through_the_dishonorable_ranks() {
    for (internal, visual) in [
        (1i64, -4i64),
        (2, -3),
        (3, -2),
        (4, -1),
        (5, 1),
        (9, 5),
        (18, 14),
    ] {
        assert_eq!(visual_rank(internal), visual, "internal {internal}");
        // The server's form, which this is not.
        let servers = if internal > 4 {
            internal - 4
        } else {
            -internal
        };
        assert_eq!(
            servers == visual,
            internal >= 5,
            "the two forms agree above rank 4 and disagree below it"
        );
    }

    let s = seated();
    seat_rank_globals(&s);
    for (internal, visual) in [(1i64, -4i64), (4, -1), (5, 1), (18, 14)] {
        assert_eq!(
            s.eval::<i64>(&format!(
                "local _, number = GetPVPRankInfo({internal}) return number"
            ))
            .unwrap(),
            visual,
            "GetPVPRankInfo({internal})"
        );
    }
}

/// Both failure edges answer two values, `(nil, 0)`, since the panes destructure a pair.
#[test]
fn the_range_gate_refuses_rank_zero_and_rank_nineteen_alike() {
    let s = seated();
    seat_rank_globals(&s);

    for rank in [0i64, 19, 20, -1, 255] {
        let (name, number) = s
            .eval::<(Option<String>, i64)>(&format!(
                "local n, r = GetPVPRankInfo({rank}) return n, r"
            ))
            .unwrap();
        assert_eq!(name, None, "rank {rank} names nothing");
        assert_eq!(number, 0, "rank {rank} numbers 0");
        assert_eq!(
            s.arity(&format!("GetPVPRankInfo({rank})")).unwrap(),
            2,
            "rank {rank} still answers two values"
        );
    }

    // The refusal is the gate, not a missing key: `PVP_RANK_19_1` is seated.
    assert_eq!(s.eval::<String>("return PVP_RANK_19_1").unwrap(), "Leader");
    assert!(s.eval::<bool>("return GetPVPRankInfo(1) ~= nil").unwrap());
    assert!(s.eval::<bool>("return GetPVPRankInfo(18) ~= nil").unwrap());
}

#[test]
fn get_pvp_rank_info_takes_a_second_argument_three_different_ways() {
    let mut s = seated(); // an Alliance player
    seat_rank_globals(&s);
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Thrall".into()),
            is_player: true,
            faction_group: Some("Horde".into()),
            pvp_team: 0,
            pvp_rank: 14,
            ..Default::default()
        }),
    );

    // Absent: the local player's side.
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Sergeant Major"
    );
    // A number is the digit, with no unit resolved, even one the player is not.
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9, 0))").unwrap(),
        "Senior Sergeant"
    );
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9, 1))").unwrap(),
        "Sergeant Major"
    );
    // `lua_isnumber` accepts a numeric string, so "0" is a digit, not a token.
    assert_eq!(
        s.eval::<String>(r#"return (GetPVPRankInfo(9, "0"))"#)
            .unwrap(),
        "Senior Sergeant"
    );
    // …and truncates toward zero, like every other numeric argument.
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9, 0.9))").unwrap(),
        "Senior Sergeant"
    );
    assert!(s
        .eval::<bool>("return GetPVPRankInfo(9, 7) == nil")
        .unwrap());

    // A string is a unit token: the foreign unit's side, not the player's.
    assert_eq!(
        s.eval::<String>(r#"return (GetPVPRankInfo(9, "target"))"#)
            .unwrap(),
        "Senior Sergeant",
        "Thrall is Horde, so his ladder names rank 9"
    );

    // A token naming nothing or a non-player falls to team 0, the register's initial value.
    assert_eq!(
        s.eval::<String>(r#"return (GetPVPRankInfo(9, "mouseover"))"#)
            .unwrap(),
        "Senior Sergeant"
    );
    s.set_unit(
        "mouseover",
        Some(UnitState {
            exists: true,
            name: Some("Timber Wolf".into()),
            pvp_team: 1,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<String>(r#"return (GetPVPRankInfo(9, "mouseover"))"#)
            .unwrap(),
        "Senior Sergeant",
        "a non-player is gated out before its side is read"
    );

    // A unit that resolves with no side is -1 and misses the key, unlike the not-found 0 above.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            is_player: true,
            pvp_team: -1,
            ..Default::default()
        }),
    );
    assert!(s
        .eval::<bool>(r#"return GetPVPRankInfo(9, "target") == nil"#)
        .unwrap());

    assert!(s
        .eval::<Option<String>>(r#"return (GetPVPRankInfo(9, "wombat"))"#)
        .is_err());
}

/// `0x51a930` tests `lua_isnumber` and raises: not a `(nil, 0)` edge.
#[test]
fn get_pvp_rank_info_raises_without_a_numeric_first_argument() {
    let s = seated();
    for call in [
        "GetPVPRankInfo()",
        "GetPVPRankInfo({})",
        "GetPVPRankInfo(nil)",
    ] {
        let err = s
            .eval::<Option<String>>(&format!("return ({call})"))
            .expect_err(call);
        assert!(
            format!("{err}").contains("Usage: GetPVPRankInfo(rank [, unit])"),
            "{call}: {err}"
        );
    }
    // A numeric string is a number to Lua, so it does not raise.
    assert!(s
        .eval::<Option<String>>(r#"return (GetPVPRankInfo("9"))"#)
        .is_ok());
}

/// `PLAYER_BYTES_3` is public, and the reference's inspect pane calls `UnitPVPRank("target")`.
#[test]
fn unit_pvp_rank_answers_for_a_foreign_unit() {
    let mut s = seated();
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Thrall".into()),
            is_player: true,
            faction_group: Some("Horde".into()),
            pvp_team: 0,
            pvp_rank: 14,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitPVPRank("target")"#).unwrap(),
        14
    );
    assert_eq!(s.eval::<i64>(r#"return UnitPVPRank("player")"#).unwrap(), 9);
    // A creature reads 0 because it has no player descriptor block to decode a rank from; the
    // binding does not re-gate on `is_player`.
    s.set_unit(
        "mouseover",
        Some(UnitState {
            exists: true,
            name: Some("Timber Wolf".into()),
            is_player: false,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitPVPRank("mouseover")"#).unwrap(),
        0
    );
    s.set_unit("mouseover", None);
    assert_eq!(
        s.eval::<i64>(r#"return UnitPVPRank("mouseover")"#).unwrap(),
        0,
        "no snapshot reads 0, like every other numeric Unit* getter"
    );
    // A non-string argument raises (`0x51a8ac`) rather than answering 0.
    assert!(s.eval::<i64>("return UnitPVPRank()").is_err());
    assert!(s.eval::<i64>("return UnitPVPRank({})").is_err());
}

/// `UnitPVPName`'s three legs (`0x609370`); the title fills `UNIT_PVP_NAME` rank first and is
/// gendered by this unit, unlike the pane's.
#[test]
fn unit_pvp_name_decorates_a_ranked_player_and_falls_back_three_ways() {
    let mut s = seated();
    seat_rank_globals(&s);
    seat_name_globals(&s);

    // Leg A: the seated player is a female Alliance rank 9, so the `_FEMALE` twin wins.
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Sergeant Major (f) Benilla"
    );
    s.set_unit("player", Some(UnitState { sex: 2, ..player() }));
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Sergeant Major Benilla"
    );

    s.set_unit(
        "player",
        Some(UnitState {
            sex: 2,
            pvp_rank: 19,
            ..player()
        }),
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Leader Benilla",
        "rank 19 names here and is refused by GetPVPRankInfo"
    );

    // Leg A′: the city-protector medal, on its own line.
    s.set_unit(
        "player",
        Some(UnitState {
            sex: 2,
            pvp_medal: 1,
            ..player()
        }),
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Sergeant Major Benilla\nGuardian of Stormwind"
    );

    // Leg C: an unranked player is just a name, and so is a plain creature.
    s.set_unit(
        "player",
        Some(UnitState {
            pvp_rank: 0,
            ..player()
        }),
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Benilla"
    );
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: Some("Timber Wolf".into()),
            level: 5,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("target")"#).unwrap(),
        "Timber Wolf"
    );

    // Leg B: the civilian prefix, on the tooltip's `0x612550` gate: PvP-flagged, hostile and grey
    // to the player.
    s.set_player_req_state(crate::script::PlayerReqState {
        level: 30,
        ..Default::default()
    });
    let civilian = |reaction: u8, pvp: bool| UnitState {
        exists: true,
        name: Some("Innkeeper Renee".into()),
        level: 5,
        civilian: true,
        pvp,
        reaction,
        ..Default::default()
    };
    s.set_unit("target", Some(civilian(2, true)));
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("target")"#).unwrap(),
        "Civilian Innkeeper Renee"
    );
    s.set_unit("target", Some(civilian(5, true)));
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("target")"#).unwrap(),
        "Innkeeper Renee",
        "a FRIENDLY civilian is not a dishonorable kill"
    );
    s.set_unit("target", Some(civilian(2, false)));
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("target")"#).unwrap(),
        "Innkeeper Renee",
        "…nor an unflagged one"
    );

    assert!(s
        .eval::<bool>(r#"return UnitPVPName("mouseover") == nil"#)
        .unwrap());
    s.set_unit(
        "mouseover",
        Some(UnitState {
            exists: true,
            name: None,
            ..Default::default()
        }),
    );
    assert!(
        s.eval::<bool>(r#"return UnitPVPName("mouseover") == nil"#)
            .unwrap(),
        "a snapshot whose name has not resolved is nil, not an empty decoration"
    );
    assert!(s.eval::<String>("return UnitPVPName()").is_err());
}

/// No install: the plain name, where the reference would format an empty string.
#[test]
fn unit_pvp_name_without_the_globalstrings_answers_the_plain_name() {
    let s = seated(); // nothing seated on _G
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Benilla"
    );
    // The template alone is not enough: the title has to resolve too.
    s.lua()
        .globals()
        .set("UNIT_PVP_NAME", "%s %s")
        .expect("global");
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Benilla"
    );
}

/// Two varargs is the engine's limit (`add esp,0x14`), so a third `%s` has nothing to consume.
#[test]
fn the_two_string_template_is_substituted_not_hardcoded() {
    assert_eq!(format_two_strings("%s %s", "Rank", "Name"), "Rank Name");
    assert_eq!(format_two_strings("%s, %s!", "Rank", "Name"), "Rank, Name!");
    assert_eq!(format_two_strings("%s%s", "Rank", "Name"), "RankName");
    assert_eq!(format_two_strings("100%% %s %s", "R", "N"), "100% R N");
    assert_eq!(
        format_two_strings("no specifiers", "R", "N"),
        "no specifiers"
    );
    assert_eq!(format_two_strings("%s %s %s", "R", "N"), "R N ");
    assert_eq!(format_two_strings("%d %s %s", "R", "N"), "%d R N");
    assert_eq!(format_two_strings("trailing %", "R", "N"), "trailing %");

    let s = seated();
    seat_rank_globals(&s);
    seat_name_globals(&s);
    s.lua()
        .globals()
        .set("UNIT_PVP_NAME", "<%s> %s")
        .expect("global");
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "<Sergeant Major (f)> Benilla"
    );
}

#[test]
fn has_inspect_honor_data_latches_on_the_push_and_clears_with_it() {
    let mut s = seated();
    let held = || r#"return HasInspectHonorData() and true or false"#;

    assert!(!s.eval::<bool>(held()).unwrap());
    s.set_inspect_honor(Some(inspect_state()));
    assert!(s.eval::<bool>(held()).unwrap());

    // `0x4c95e0` pushes the number 1 (`lua_pushnumber(1.0)`), not a boolean.
    assert_eq!(
        s.eval::<String>("return type(HasInspectHonorData())")
            .unwrap(),
        "number"
    );
    assert_eq!(
        s.eval::<String>("return tostring(HasInspectHonorData())")
            .unwrap(),
        "1"
    );

    s.set_inspect_honor(None);
    assert!(!s.eval::<bool>(held()).unwrap());
}

/// The reference's own twelve-wide destructure, and the standing sitting ninth.
#[test]
fn get_inspect_honor_data_returns_the_twelve_in_the_reference_order() {
    let mut s = seated();
    s.set_inspect_honor(Some(inspect_state()));

    assert_eq!(s.arity("GetInspectHonorData()").unwrap(), 12);
    let got = s
        .eval::<Vec<i64>>(
            "local sessionHK, sessionDK, yesterdayHK, yesterdayHonor, thisweekHK, \
             thisweekHonor, lastweekHK, lastweekHonor, lastweekStanding, lifetimeHK, \
             lifetimeDK, lifetimeRank = GetInspectHonorData() \
             return {sessionHK, sessionDK, yesterdayHK, yesterdayHonor, thisweekHK, \
             thisweekHonor, lastweekHK, lastweekHonor, lastweekStanding, lifetimeHK, \
             lifetimeDK, lifetimeRank}",
        )
        .unwrap();
    assert_eq!(got, [61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 12]);

    assert_eq!(
        s.eval::<f64>("return GetInspectPVPRankProgress()").unwrap(),
        255.0 * RANK_BAR_SCALE,
        "the reply's own rankBar byte, not the player's"
    );
}

/// `0x4c9620` is ungated and reads zero-initialised slots; a short return would paint blank rows
/// where the reference paints zeros.
#[test]
fn get_inspect_honor_data_answers_twelve_zeros_when_no_reply_is_held() {
    let mut s = seated();
    assert_eq!(
        s.arity("GetInspectHonorData()").unwrap(),
        12,
        "ungated: twelve on every path"
    );
    assert_eq!(
        s.eval::<Vec<i64>>("return {GetInspectHonorData()}")
            .unwrap(),
        vec![0; 12]
    );
    assert!(s
        .eval::<bool>("return (GetInspectHonorData()) == 0")
        .unwrap());
    assert_eq!(
        s.eval::<f64>("return GetInspectPVPRankProgress()").unwrap(),
        0.0
    );

    // Zeros again after a clear; the reference would still hold the previous target's numbers
    // (`0x4c6f70` zeroes only the flags).
    s.set_inspect_honor(Some(inspect_state()));
    s.set_inspect_honor(None);
    assert_eq!(
        s.eval::<Vec<i64>>("return {GetInspectHonorData()}")
            .unwrap(),
        vec![0; 12]
    );
}

/// `0x4c80a0` refuses a query while one is in flight and once data is held; `TogglePVP` has no
/// latch.
#[test]
fn the_intent_queues_drain_and_the_honor_query_refuses_to_double_up() {
    let mut s = seated();
    assert_eq!(s.take_inspect_honor_requests(), 0);

    // Two calls, one query: the second sees `pending`.
    s.eval::<()>("RequestInspectHonorData() RequestInspectHonorData()")
        .unwrap();
    assert_eq!(s.take_inspect_honor_requests(), 1);
    assert_eq!(s.take_inspect_honor_requests(), 0);

    // Still latched after the drain: sent, not yet answered.
    s.eval::<()>("RequestInspectHonorData()").unwrap();
    assert_eq!(s.take_inspect_honor_requests(), 0, "still in flight");

    // The reply clears `pending` and sets `hasData`, which refuses the next ask instead.
    s.set_inspect_honor(Some(inspect_state()));
    s.eval::<()>("RequestInspectHonorData()").unwrap();
    assert_eq!(s.take_inspect_honor_requests(), 0, "data already held");

    // Dropping the inspected player clears both, and the next ask goes out.
    s.set_inspect_honor(None);
    s.eval::<()>("RequestInspectHonorData()").unwrap();
    assert_eq!(s.take_inspect_honor_requests(), 1);

    // The binding answers zero Lua values on every one of those paths (`0x4c9610`).
    assert_eq!(s.arity("RequestInspectHonorData()").unwrap(), 0);

    s.eval::<()>("TogglePVP() TogglePVP()").unwrap();
    assert_eq!(s.take_pvp_toggles(), 2, "no latch here — two packets");
    assert_eq!(s.take_pvp_toggles(), 0);
}
