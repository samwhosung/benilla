//! The PvP + honor binding tests (the parent module is the unit under test).
//!
//! Several of these pin facts that look like bugs — the multiply-not-divide rank bar, the
//! backwards negative visual ranks, rank 19's rejection, the ungendered pane title, the suppressed
//! sub-5 lifetime rank. Each is verified at the bytes, and each one would pass just as happily
//! against the wrong reading if it were written loosely, so they are written tightly on purpose.

use super::*;
use crate::script::{UiScript, UnitState};

/// A snapshot whose every number is distinct, so a swapped return order cannot pass: the tens
/// digit is the block (session 1x, yesterday 2x, this week 3x, last week 4x, lifetime 5x) and
/// the units digit is the position within it.
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
        // 51 is 0.2 of 255 exactly — which the engine's multiply does NOT produce, and that
        // near-miss is the whole point of the byte chosen here.
        rank_bar: 51,
    }
}

/// The inspect reply's twelve, same trick: 6x is the session block, then one number per return
/// so the ninth (the standing) cannot hide behind the eighth.
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

/// A female Alliance player carrying rank 9 (visual 5) — the snapshot `UnitPVPRank("player")`
/// and every title lookup read.
fn player() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Benilla".into()),
        sex: 3, // female
        is_player: true,
        // Both, because a real Alliance player has both — and they are not the same field.
        // `faction_group` is `UnitFactionGroup`'s live faction template; `pvp_team` is
        // `0x5efe00`'s race walk, which is what every rank title is keyed by. The GM-mode test
        // below is the one that drives them apart.
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

/// Install the rank titles the real `GlobalStrings.lua` puts on `_G` at boot — the only place
/// `GetPVPRankInfo` reads names from (see the module doc). Both teams, one female twin, the
/// dishonorable end and rank 19, so the key construction is pinned rather than assumed.
///
/// Rank **17** is deliberately left unseated: it is the "the key exists in no locale" case the nil
/// tests need, now that 19 is a real title here.
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
        // The racial-leader rank: a real GlobalString the pane's binding still refuses.
        ("PVP_RANK_19_1", "Leader"),
        ("PVP_RANK_19_1_FEMALE", "Leader (f)"),
    ] {
        g.set(key, value).expect("global string");
    }
}

/// The GlobalStrings `UnitPVPName`'s decoration reads — the enUS templates, which is what makes
/// the two-`%s` substitution testable at all (see [`format_two_strings`]).
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

/// **The reference's own destructuring**, line for line — the highest-value assertion
/// available, because it is the exact code `HonorFrame.lua` runs. Four from last week, three
/// from yesterday, two from this week: the arities are not symmetric and getting one wrong
/// shifts a whole pane by a column.
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

/// **`GetPVPLifetimeStats` suppresses a highest-lifetime rank below 5** (`0x51a843 cmp al,5; jb`),
/// reporting `0` instead of the true byte — which is why the reference pane shows NONE for a
/// character whose best rank was one of the four dishonorable ones. The boundary is the whole test:
/// 4 vanishes, 5 survives.
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

    // Suppressed to a NUMBER, not to nil: the pane feeds it straight to `GetPVPRankInfo`, which
    // raises on a non-number.
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

/// Arity, measured at the host boundary ([`UiScript::arity`]). A getter that returned one value
/// too many would still pass the destructuring test above; the count is what catches it.
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

/// Before the first push every self getter still answers at full width, with zeros — the
/// reference feeds these straight into `format()`, so a nil would raise inside its pane.
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

/// **The rank bar is a MULTIPLY by a slightly-wrong f32 reciprocal, and it does not clamp.**
///
/// Written to fail against the natural `byte / 255.0` reading rather than to describe the shape:
/// the constant is asserted at its bits, every byte is compared to the exact product, and the two
/// values a divisor would round prettier (51 → 0.2, 255 → 1.0) are asserted **not** to be those
/// numbers. A full bar comes out *above* 1.0, which is the clamp's absence made visible.
#[test]
fn the_rank_bar_multiplies_by_the_f32_reciprocal_and_never_clamps() {
    // The four bytes at `0x8026c8`, and the fact that makes this test necessary at all.
    assert_eq!(f64::from(f32::from_bits(0x3B80_8081)), RANK_BAR_SCALE);
    assert_ne!(RANK_BAR_SCALE, 1.0 / 255.0, "the f32 is NOT 1/255");

    let mut s = seated();
    let bar = |s: &UiScript| s.eval::<f64>("return GetPVPRankProgress()").unwrap();

    // 51 is the fixture's byte, and 51/255 would be exactly 0.2. The multiply is not.
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

    // The ends. Zero is the one input the two readings agree on; a full bar OVERSHOOTS, and
    // nothing clamps it back.
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

/// **Rank 0 names nothing.** There is no `PVP_RANK_0_*` GlobalString, and both reference panes
/// depend on that nil to fall back to `NONE` — so this runs the fallback the way the pane
/// writes it, not just the nil.
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

/// The title key: the team digit (0 Horde / 1 Alliance) off the player's faction group — and
/// **the pane's title is NOT gendered**, which is the half a plausible implementation gets wrong.
///
/// `GetPVPRankInfo` hands `0x703bf0` gender 0 (`0x51aa0f`) and takes the fast path, so a female
/// character sees the male/default title in her own honor pane while the `_FEMALE` twin sits right
/// there on `_G`. The seated player is female and a twin exists for rank 9 precisely so that this
/// test fails the moment somebody "fixes" the resolver to prefer it.
#[test]
fn the_pane_title_is_keyed_by_team_and_is_never_gendered() {
    let mut s = seated();
    seat_rank_globals(&s);

    // Female Alliance → the BASE key, with `PVP_RANK_9_1_FEMALE` seated and ignored.
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Sergeant Major"
    );
    assert_eq!(
        s.eval::<String>("return PVP_RANK_9_1_FEMALE").unwrap(),
        "Sergeant Major (f)",
        "the twin is on _G — the binding declines it, it does not miss it"
    );

    // Male Alliance → the same answer, which is the point.
    s.set_unit("player", Some(UnitState { sex: 2, ..player() }));
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Sergeant Major"
    );

    // Horde is team 0 — an entirely different word list, which is why an unknown side must
    // not be guessed at.
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

/// The **Rust-side** title lookup the app's `SMSG_PVP_CREDIT` line uses shares the key
/// construction with the binding and **deliberately differs on gender**: the credit formatter
/// resolves through `0x612bf0` with the local player's gender selector, the pane's binding passes
/// 0. One function each, and the pair of them is asserted apart here so neither drifts into the
/// other.
///
/// The credit packet carries the INTERNAL rank, which is why its number goes straight in with no
/// visual conversion — and unlike the binding this path applies **no range check**, so rank 19
/// names "Leader" here and `(nil, 0)` there.
#[test]
fn the_rust_side_title_lookup_is_gendered_where_the_binding_is_not() {
    let s = seated();
    seat_rank_globals(&s);

    // The app names a side and a sex outright — no player snapshot involved.
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

    // Gendered means PREFERRED, not required: a rank with no twin falls back to the base key.
    assert_eq!(s.pvp_rank_title(5, 1, true).as_deref(), Some("Private"));

    // No range check on this path — rank 19 is a name here and a refusal in the binding.
    assert_eq!(s.pvp_rank_title(19, 1, false).as_deref(), Some("Leader"));
    assert_eq!(s.pvp_rank_title(19, 1, true).as_deref(), Some("Leader (f)"));
    assert!(s.eval::<bool>("return GetPVPRankInfo(19) == nil").unwrap());

    // Same nil semantics either side: rank 0, an absent key, and an empty one.
    assert_eq!(s.pvp_rank_title(0, 1, false), None);
    assert_eq!(s.pvp_rank_title(17, 1, false), None, "no PVP_RANK_17_1");
    s.lua().globals().set("PVP_RANK_17_1", "").expect("global");
    assert_eq!(
        s.pvp_rank_title(17, 1, false),
        None,
        "empty reads as absent"
    );
}

/// A missing GlobalString — the bare VM with no install behind it — reads as **nil**, never as
/// an empty string and never as a raise: the panes' `if not rankName` fallback is the only
/// thing standing between that and a blank title.
#[test]
fn a_missing_rank_global_reads_nil_not_empty() {
    let s = seated(); // no rank globals seated at all
    assert!(s.eval::<bool>("return GetPVPRankInfo(9) == nil").unwrap());
    // …and so does an empty one.
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

/// **The team digit is NOT the faction group** — report B378 at the binding level.
///
/// `UnitFactionGroup` reads the unit's live `UNIT_FIELD_FACTIONTEMPLATE` (`0x516630`) and the rank
/// title's team digit reads the unit's RACE (`0x5efe00`), so a vmangos GM — template 35, group
/// mask 0 — has no side and keeps his rank. Both `0x5efe00` surfaces are asserted here: the
/// binding's key and `UnitPVPName`'s decoration (`0x5efe60`, the same walk).
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
            // `.gm on` — the template names nothing …
            faction_group: None,
            faction_group_localized: None,
            // … and the race still names Alliance.
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
    // The control: a unit whose RACE resolves to nothing is still −1, and −1 still misses.
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

/// The internal→visual conversion across its whole range — **and the negative half runs the
/// opposite way from the server's `visualRank`**, which is the assertion that matters: rank 1 is
/// −4 and rank 4 is −1, because `0x51aa38` subtracts 5 rather than negating. vmangos's
/// `HonorMgr.cpp:991` gives 1 → −1 and 4 → −4; the binding's second return is the client's, and
/// this pins it so nobody restores the server's arithmetic here.
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
        // The server's form, stated as the thing this is NOT.
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

    // …and the binding reports the same number, which is what indexes the badge texture: only a
    // positive one is drawn, so all four dishonorable ranks hide it.
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

/// **The range gate is `[1, 18]`, so rank 19 — "Leader" — is refused**, GlobalString and all.
///
/// The consequence is real and visible: the engine's own badge table has fifteen entries because
/// rank 19 → `PvPRank15` is reachable from the world-text kill toast (`0x6c7f10`, index
/// `rank − 5`), never from this binding, which is why FrameXML only ever names fourteen.
///
/// Both failure edges answer `(nil, 0)` — **two** values, never one — because the panes destructure
/// a pair and then branch on the nil.
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

    // The refusal is the GATE, not a missing key: `PVP_RANK_19_1` is right there.
    assert_eq!(s.eval::<String>("return PVP_RANK_19_1").unwrap(), "Leader");
    // …and the ends of the accepted range do answer.
    assert!(s.eval::<bool>("return GetPVPRankInfo(1) ~= nil").unwrap());
    assert!(s.eval::<bool>("return GetPVPRankInfo(18) ~= nil").unwrap());
}

/// **The second argument**, which the reference's own panes never pass and which is therefore easy
/// to miss entirely: a NUMBER is the team digit itself, a STRING is a unit token, and absent means
/// the local player.
#[test]
fn get_pvp_rank_info_takes_a_second_argument_three_different_ways() {
    let mut s = seated(); // an ALLIANCE player
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

    // Absent → the local player's side.
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9))").unwrap(),
        "Sergeant Major"
    );
    // A NUMBER is the digit, with no unit resolved at all — including a digit the player is not.
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9, 0))").unwrap(),
        "Senior Sergeant"
    );
    assert_eq!(
        s.eval::<String>("return (GetPVPRankInfo(9, 1))").unwrap(),
        "Sergeant Major"
    );
    // Lua's `isnumber` accepts a numeric string, so "0" is a DIGIT and not a token.
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
    // A team digit no GlobalString matches simply misses.
    assert!(s
        .eval::<bool>("return GetPVPRankInfo(9, 7) == nil")
        .unwrap());

    // A STRING is a unit token — the FOREIGN unit's side, not the player's.
    assert_eq!(
        s.eval::<String>(r#"return (GetPVPRankInfo(9, "target"))"#)
            .unwrap(),
        "Senior Sergeant",
        "Thrall is Horde, so his ladder names rank 9"
    );

    // A token that names nothing, and a token naming a non-player, both fall to team 0 — the
    // engine's uninitialised team register, not a failure.
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

    // A unit that resolves but has NO side is −1, which misses the key — a different answer from
    // the not-found 0 above, and the one the panes render as NONE.
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

    // An unrecognised token raises, as it does for every unit binding.
    assert!(s
        .eval::<Option<String>>(r#"return (GetPVPRankInfo(9, "wombat"))"#)
        .is_err());
}

/// The first argument's `Usage:` gate — `0x51a930` tests `lua_isnumber` and **raises**, abandoning
/// the caller's statement. It is not a `(nil, 0)` edge, and the two are not interchangeable.
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
    // A numeric string is a number to Lua, so it does NOT raise.
    assert!(s
        .eval::<Option<String>>(r#"return (GetPVPRankInfo("9"))"#)
        .is_ok());
}

/// `UnitPVPRank` must answer for a **foreign** unit: `PLAYER_BYTES_3` is PUBLIC, and the
/// reference's inspect pane calls it as `UnitPVPRank("target")`. An unknown token reads 0.
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
    // A creature reads 0 because it has no PLAYER descriptor block to decode a rank byte from —
    // the engine's own path to the same answer. The binding does NOT re-gate on `is_player`; see
    // the binding for why our `is_player` is not the engine's type mask.
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
    // A non-string argument RAISES (`0x51a8ac`) — it does not answer 0. A number is a string to
    // Lua, so it resolves the token "5" and raises for a different reason: an unknown token.
    assert!(s.eval::<i64>("return UnitPVPRank()").is_err());
    assert!(s.eval::<i64>("return UnitPVPRank({})").is_err());
}

/// **`UnitPVPName`'s three legs** (`0x609370`), which is the whole binding: the decorated player
/// name, the civilian prefix, and the plain name.
///
/// The template is `UNIT_PVP_NAME` off `_G` and it is filled rank-FIRST — `add esp,0x14` proves two
/// varargs and enUS ships `"%s %s"`. The title here **is** gendered (by this unit), which is the
/// opposite of the pane's and the reason the two lookups are separate functions.
#[test]
fn unit_pvp_name_decorates_a_ranked_player_and_falls_back_three_ways() {
    let mut s = seated();
    seat_rank_globals(&s);
    seat_name_globals(&s);

    // Leg A — the seated player is a female Alliance rank 9, so the FEMALE twin wins here.
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Sergeant Major (f) Benilla"
    );
    // …male takes the base key, and the order is rank then name either way.
    s.set_unit("player", Some(UnitState { sex: 2, ..player() }));
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Sergeant Major Benilla"
    );

    // No range check on this path: the racial-leader rank really does render.
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

    // Leg A′ — the city-protector medal, appended on its own line.
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

    // Leg C — an unranked player is just a name, and so is a plain creature.
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

    // Leg B — the civilian prefix, on the same `0x612550` gate the tooltip's CIVILIAN line uses:
    // PvP-flagged, hostile and grey to the player. A friendly civilian is still just a name.
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

    // The two nil edges, and the raise.
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

/// With no install behind the VM there is no template and no title, and the decoration is install
/// data: we answer the **plain name**. (The engine would snprintf through an empty format and hand
/// back an empty string — a stated divergence, not an oversight.)
#[test]
fn unit_pvp_name_without_the_globalstrings_answers_the_plain_name() {
    let s = seated(); // nothing seated on _G
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Benilla"
    );
    // The template alone is not enough — the title has to resolve too.
    s.lua()
        .globals()
        .set("UNIT_PVP_NAME", "%s %s")
        .expect("global");
    assert_eq!(
        s.eval::<String>(r#"return UnitPVPName("player")"#).unwrap(),
        "Benilla"
    );
}

/// The template is *filled*, not assumed: a locale that punctuates `UNIT_PVP_NAME` differently
/// comes out differently, and a `%%` is a literal per cent. Two varargs is the engine's own limit
/// (`add esp,0x14`), so a third specifier has nothing to consume.
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

/// The inspect latch: false before any reply, true while one is held, false again once the app
/// clears it — which is what makes the reference's `OnShow` re-request for the next player.
#[test]
fn has_inspect_honor_data_latches_on_the_push_and_clears_with_it() {
    let mut s = seated();
    let held = || r#"return HasInspectHonorData() and true or false"#;

    assert!(!s.eval::<bool>(held()).unwrap());
    s.set_inspect_honor(Some(inspect_state()));
    assert!(s.eval::<bool>(held()).unwrap());

    // `0x4c95e0` pushes the NUMBER 1, not a boolean — one value either way, and the truthy arm is
    // `1` rather than `true`. (It is `lua_pushnumber(1.0)`; in a VM whose numbers are all
    // doubles there is no observable integer/float distinction, and `tostring` proves it.)
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

/// **With no reply held `GetInspectHonorData` still answers twelve — zeros.** `0x4c9620` is
/// UNGATED: it never consults the has-data flag, and the sixteen globals it reads are
/// zero-initialised BSS. A short return would leave the pane painting `SetText(nil)` — a blank row
/// — where the real client paints a `0` row, and the two look different on screen.
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
    // The bar reads its own zeroed byte, for the same reason.
    assert_eq!(
        s.eval::<f64>("return GetInspectPVPRankProgress()").unwrap(),
        0.0
    );

    // And after the app drops the reply it is zeros again — our `Option` is the latch AND the
    // data, where the engine keeps them apart and would answer the previous target's numbers
    // here (`0x4c6f70` re-keying zeroes the two flags and leaves the sixteen slots alone).
    s.set_inspect_honor(Some(inspect_state()));
    s.set_inspect_honor(None);
    assert_eq!(
        s.eval::<Vec<i64>>("return {GetInspectHonorData()}")
            .unwrap(),
        vec![0; 12]
    );
}

/// The two intent queues drain to the app and reset — and the honor query **does not queue twice**:
/// `0x4c80a0` bails while one is in flight (`pending`) and bails again once data is held
/// (`hasData`), which is what stops a pane shown/hidden/shown sending duplicates. The PvP toggle
/// has no such latch and genuinely does send twice.
#[test]
fn the_intent_queues_drain_and_the_honor_query_refuses_to_double_up() {
    let mut s = seated();
    assert_eq!(s.take_inspect_honor_requests(), 0);

    // Two calls, ONE query: the second sees `pending`.
    s.eval::<()>("RequestInspectHonorData() RequestInspectHonorData()")
        .unwrap();
    assert_eq!(s.take_inspect_honor_requests(), 1);
    assert_eq!(s.take_inspect_honor_requests(), 0);

    // Still latched after the drain — the app has sent, nothing has replied.
    s.eval::<()>("RequestInspectHonorData()").unwrap();
    assert_eq!(s.take_inspect_honor_requests(), 0, "still in flight");

    // The reply clears `pending` — and immediately sets `hasData`, so the next ask is refused for
    // the *other* reason.
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
