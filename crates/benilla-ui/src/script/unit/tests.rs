//! The `Unit*` binding tests (the parent module is the unit under test).

use crate::script::{PartyState, PlayerRecord, UiScript, UnitState};

fn player() -> UnitState {
    UnitState {
        exists: true,
        name: Some("Benilla".into()),
        health: 72,
        max_health: 100,
        level: 12,
        power_type: 1, // rage
        power: 35,
        max_power: 100,
        dead: false,
        reaction: 0,
        race: Some("Night Elf".into()),
        race_file: Some("NightElf".into()),
        class: Some("Warrior".into()),
        class_file: Some("WARRIOR".into()),
        sex: 3, // female
        ..Default::default()
    }
}

#[test]
fn unit_reaction_reports_the_scale_value_or_nil() {
    let mut s = UiScript::new().unwrap();
    // Neutral target (UnitReaction 4) → the integer; the name-plate palette indexes it.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            reaction: 4,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitReaction("target", "player")"#)
            .unwrap(),
        4
    );
    // reaction 0 (unresolved) reports as nil — the API's "can't tell".
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            reaction: 0,
            ..Default::default()
        }),
    );
    assert!(s
        .eval::<bool>(r#"return UnitReaction("player", "target") == nil"#)
        .unwrap());
}

/// The target plate's faction decision (TargetFrame_CheckFaction, as `UnitFrame_Update`
/// composes it): a player-controlled target picks red (mutually hostile) or blue (else) and
/// NEVER the reaction green; only an NPC reads the reaction swatch. This pins the predicate
/// composition the plate branch depends on — a friendly *player* must resolve to blue, which is
/// exactly the green-vs-blue bug the branch fixes.
#[test]
fn the_name_plate_faction_branch_picks_player_red_blue_over_the_reaction_swatch() {
    let mut s = UiScript::new().unwrap();
    // The plate's decision, verbatim from the XML Update (a chunk whose if-branches each return).
    let plate = r#"
            local u = "target"
            if UnitIsPlayer(u) then
                if UnitIsEnemy(u, "player") then return "red" else return "blue" end
            else
                return UnitReaction(u, "player") and "reaction" or "reaction-blue"
            end
        "#;
    let decide = |s: &mut UiScript| s.eval::<String>(plate).unwrap();

    // A friendly PLAYER (reaction 5) → blue, not the reaction green.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            is_player: true,
            reaction: 5,
            ..Default::default()
        }),
    );
    assert_eq!(decide(&mut s), "blue");
    // A hostile PLAYER (reaction 2) → red.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            is_player: true,
            reaction: 2,
            ..Default::default()
        }),
    );
    assert_eq!(decide(&mut s), "red");
    // A friendly NPC (same reaction 5, not a player) → the reaction swatch (green), unchanged.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            is_player: false,
            reaction: 5,
            ..Default::default()
        }),
    );
    assert_eq!(decide(&mut s), "reaction");
    // UnitIsPlayer itself: 1 for a player, nil otherwise.
    assert!(s
        .eval::<bool>(r#"return UnitIsPlayer("target") == nil"#)
        .unwrap());
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            is_player: true,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsPlayer("target")"#).unwrap(),
        1
    );
}

#[test]
fn unit_level_reads_minus_one_when_the_level_cant_be_told() {
    use crate::script::PlayerReqState;
    let mut s = UiScript::new().unwrap();
    s.set_player_req_state(PlayerReqState {
        level: 3,
        ..Default::default()
    });
    // A hostile 10+ levels up (reaction 2, level 13 vs player 3) → −1: the target frame's
    // skull branch (TargetFrame_CheckLevel's `level <= 0`).
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            level: 13,
            reaction: 2,
            ..Default::default()
        }),
    );
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), -1);
    // One level short of the band (12 vs 3) → the raw number.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            level: 12,
            reaction: 2,
            ..Default::default()
        }),
    );
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), 12);
    // The same 10-up gap on a NEUTRAL (reaction 4) tells its level — the gate is hostile-only.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            level: 13,
            reaction: 4,
            ..Default::default()
        }),
    );
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), 13);
    // A world boss (creature rank 3) hides its level regardless of the gap.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            level: 5,
            reaction: 4,
            rank: 3,
            ..Default::default()
        }),
    );
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), -1);
    // A PLAYER never reads −1 (rank/reaction notwithstanding).
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            level: 60,
            reaction: 2,
            is_player: true,
            ..Default::default()
        }),
    );
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), 60);
    // A raw level 0 (unstreamed) returns VERBATIM — never −1 (`0x517fc0` pushes the
    // raw field; only the boss/hostile legs substitute). Same skull outcome via `<= 0`.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            level: 0,
            reaction: 2,
            ..Default::default()
        }),
    );
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), 0);
}

#[test]
fn corpse_can_attack_and_green_range_bindings() {
    use crate::script::PlayerReqState;
    let mut s = UiScript::new().unwrap();
    // UnitIsCorpse: a pure TYPEID_CORPSE object check (`0x5161c0`) — a DEAD unit is NOT a corpse…
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            dead: true,
            ..Default::default()
        }),
    );
    assert!(s
        .eval::<bool>(r#"return UnitIsCorpse("target") == nil"#)
        .unwrap());
    // …only a resolved corpse world object is.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            corpse_object: true,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsCorpse("target")"#).unwrap(),
        1
    );
    // UnitCanAttack reads the app-fed 0x606980 verdict off the non-player token, both
    // argument orders; false → nil.
    assert!(s
        .eval::<bool>(r#"return UnitCanAttack("player", "target") == nil"#)
        .unwrap());
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            can_attack: true,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitCanAttack("player", "target")"#)
            .unwrap(),
        1
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitCanAttack("target", "player")"#)
            .unwrap(),
        1
    );
    assert!(s
        .eval::<bool>(r#"return UnitIsCorpse("target") == nil"#)
        .unwrap());
    // GetQuestGreenRange: the grey band for the player's level — 4 at level 3, 9 at 46,
    // 12 clamped past the table (the byte-verified GREY_BAND).
    for (pl, want) in [(3u32, 4i64), (46, 9), (120, 12)] {
        s.set_player_req_state(PlayerReqState {
            level: pl,
            ..Default::default()
        });
        assert_eq!(
            s.eval::<i64>("return GetQuestGreenRange()").unwrap(),
            want,
            "green range at level {pl}"
        );
    }
}

#[test]
fn target_unit_queues_the_token_for_the_app_to_resolve() {
    use crate::script::SelectionRequest;
    let mut s = UiScript::new().unwrap();
    // Nothing queued until a call lands.
    assert!(s.take_selection_requests().is_empty());
    // Each call queues its raw token, in order; a nil token is ignored.
    s.eval::<()>(r#"TargetUnit("player")"#).unwrap();
    s.eval::<()>(r#"TargetUnit("target")"#).unwrap();
    s.eval::<()>(r#"TargetUnit(nil)"#).unwrap();
    assert_eq!(
        s.take_selection_requests(),
        vec![
            SelectionRequest::Unit("player".into()),
            SelectionRequest::Unit("target".into()),
        ]
    );
    // The drain is a take — a second read is empty.
    assert!(s.take_selection_requests().is_empty());
}

/// The three verbs share ONE queue, and the queue keeps their call order — which is the whole
/// reason it is one queue (`0x489a40` is one function for all of them, and a macro can observe
/// the order). A nil `AssistUnit` argument is dropped like `TargetUnit`'s, and
/// `TargetLastEnemy()` names no unit at all.
#[test]
fn the_selection_queue_carries_all_three_verbs_in_call_order() {
    use crate::script::SelectionRequest;
    let mut s = UiScript::new().unwrap();
    s.eval::<()>(r#"TargetUnit("party1")"#).unwrap();
    s.eval::<()>(r#"AssistUnit("target")"#).unwrap();
    s.eval::<()>(r#"AssistUnit(nil)"#).unwrap();
    s.eval::<()>("TargetLastEnemy()").unwrap();
    assert_eq!(
        s.take_selection_requests(),
        vec![
            SelectionRequest::Unit("party1".into()),
            SelectionRequest::Assist("target".into()),
            SelectionRequest::LastEnemy,
        ]
    );
    assert!(s.take_selection_requests().is_empty());
}

/// `TargetNearestFriend`'s reverse flag, read exactly as `0x6f1c10(idx 1, default 0)` does:
/// absent or nil is forward, and 1.12's own `Bindings.xml` note — *"1 (or \"true\") means
/// reverse!"* — is why a boolean and a number both count. A numeric `0` is forward.
#[test]
fn target_nearest_friend_queues_its_reverse_flag() {
    let mut s = UiScript::new().unwrap();
    assert!(s.take_target_nearest_friend_requests().is_empty());
    s.eval::<()>("TargetNearestFriend()").unwrap();
    s.eval::<()>("TargetNearestFriend(1)").unwrap();
    s.eval::<()>("TargetNearestFriend(true)").unwrap();
    s.eval::<()>("TargetNearestFriend(0)").unwrap();
    s.eval::<()>("TargetNearestFriend(nil)").unwrap();
    assert_eq!(
        s.take_target_nearest_friend_requests(),
        vec![false, true, true, false, false]
    );
    assert!(s.take_target_nearest_friend_requests().is_empty());
}

#[test]
fn set_unit_is_read_by_the_bindings() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));

    assert_eq!(s.eval::<i64>(r#"return UnitExists("player")"#).unwrap(), 1);
    assert_eq!(
        s.eval::<String>(r#"return UnitName("player")"#).unwrap(),
        "Benilla"
    );
    assert_eq!(s.eval::<i64>(r#"return UnitHealth("player")"#).unwrap(), 72);
    assert_eq!(
        s.eval::<i64>(r#"return UnitHealthMax("player")"#).unwrap(),
        100
    );
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("player")"#).unwrap(), 12);
    assert!(s
        .eval::<bool>(r#"return UnitIsDead("player") == nil"#)
        .unwrap());
}

#[test]
fn absent_token_reports_not_existing_with_zero_numbers() {
    let s = UiScript::new().unwrap();
    // **nil, not `false`** — the family's false leg. `== nil` is the comparison a
    // boolean would invert, so it is the one worth asserting.
    assert!(s
        .eval::<bool>(r#"return UnitExists("target") == nil"#)
        .unwrap());
    assert_eq!(s.eval::<i64>(r#"return UnitHealth("target")"#).unwrap(), 0);
    assert_eq!(
        s.eval::<i64>(r#"return UnitHealthMax("target")"#).unwrap(),
        0
    );
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), 0);
    assert!(s
        .eval::<bool>(r#"return UnitName("target") == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return UnitIsDead("target") == nil"#)
        .unwrap());
}

#[test]
fn a_dead_unit_reports_dead_and_zero_health() {
    let mut s = UiScript::new().unwrap();
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            name: None,
            health: 0,
            max_health: 3200,
            level: 30,
            dead: true,
            ..Default::default()
        }),
    );
    assert_eq!(s.eval::<i64>(r#"return UnitExists("target")"#).unwrap(), 1);
    assert_eq!(s.eval::<i64>(r#"return UnitIsDead("target")"#).unwrap(), 1);
    assert_eq!(s.eval::<i64>(r#"return UnitHealth("target")"#).unwrap(), 0);
    // Name unknown (no name-query yet) → `UNKNOWNOBJECT`, never nil: the unit RESOLVED, and the
    // reference pushes nil for a zero GUID only (the shape is pinned below). A bare
    // VM carries no GlobalStrings, so this is the binary's own literal.
    assert_eq!(
        s.eval::<String>(r#"return UnitName("target")"#).unwrap(),
        "Unknown Being"
    );
}

/// `UnitName`'s value 1 is nil in exactly two cases — a zero GUID, and the `"player"` fast path
/// over an empty local-name buffer — and a STRING everywhere else: the cached name, or
/// `FrameScript_GetText("UNKNOWNOBJECT")` for a unit that resolved but whose name the cache has not
/// answered (`0x517020`; `0x609324`). The stock stable window concatenates the answer on
/// `UNIT_PET` (`PetStable.lua:129`), the instant a called pet's name is still in flight — the
/// director's `attempt to concatenate a nil value` dialog.
#[test]
fn unitname_reads_unknownobject_for_a_resolved_unit_whose_name_is_in_flight() {
    let mut s = UiScript::new().unwrap();
    let pending = UnitState {
        exists: true,
        has_object: true,
        name: None,
        level: 58,
        guid: 0xF140_0000_0000_0001,
        ..Default::default()
    };
    s.set_unit("pet", Some(pending.clone()));

    // No GlobalStrings loaded: the literal the binary falls back to (`0x860fa4`).
    assert_eq!(
        s.eval::<String>(r#"return UnitName("pet")"#).unwrap(),
        "Unknown Being"
    );
    // With the stock global seated, its (localizable) value — read out of the VM, not baked in.
    s.run(r#"UNKNOWNOBJECT = "Unknown""#).unwrap();
    assert_eq!(
        s.eval::<String>(r#"return UnitName("pet")"#).unwrap(),
        "Unknown"
    );
    // An EMPTY global is the same miss as an absent one (`0x609324`'s empty check).
    s.run(r#"UNKNOWNOBJECT = """#).unwrap();
    assert_eq!(
        s.eval::<String>(r#"return UnitName("pet")"#).unwrap(),
        "Unknown Being"
    );
    // Still two returns, the second nil (1840).
    assert_eq!(
        s.eval::<i64>(r#"local t = {UnitName("pet")}; return table.getn(t)"#)
            .unwrap(),
        1,
        "a trailing nil is not counted by getn"
    );
    assert!(s
        .eval::<bool>(r#"local _, realm = UnitName("pet"); return realm == nil"#)
        .unwrap());

    // The name lands: the string, and nothing else changes.
    let mut named = pending.clone();
    named.name = Some("Snarl".into());
    s.set_unit("pet", Some(named));
    assert_eq!(
        s.eval::<String>(r#"return UnitName("pet")"#).unwrap(),
        "Snarl"
    );

    // The `"player"` fast path never reaches the resolver: an empty local-name buffer is nil.
    s.set_unit("player", Some(pending));
    assert!(s
        .eval::<bool>(r#"return UnitName("player") == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return UnitName("PLAYER") == nil"#)
        .unwrap());

    // A token nothing resolves is GUID 0 → nil (the shape `absent_token_…` pins too).
    assert!(s
        .eval::<bool>(r#"return UnitName("target") == nil"#)
        .unwrap());
}

#[test]
fn power_bindings_read_the_active_type() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player())); // rage 35/100
                                          // ONE value. The Era `(type, "RAGE")` pair does not exist in 5875 — `0x517940` pushes a
                                          // number at every one of its four live `ret`s and never a string.
    assert_eq!(
        s.eval::<i64>(r#"return UnitPowerType("player")"#).unwrap(),
        1
    );
    assert_eq!(s.eval::<i64>(r#"return UnitMana("player")"#).unwrap(), 35);
    assert_eq!(
        s.eval::<i64>(r#"return UnitManaMax("player")"#).unwrap(),
        100
    );
    // **The 1.12 verbs take one argument and report the ACTIVE power**, whatever it is — this
    // unit is a rage user and `UnitMana` reads 35 rage, which looks wrong and is exactly right
    // (`UnitFrame.lua`: `local currValue = UnitMana(unit)` for every power type). Asking about a
    // specific type is `UnitPowerType(unit)` first; there is no second argument to pass, and the
    // Era pair that had one is gone (decision 1190's beyond-1.12 list, 1188 phase 5).
    assert!(
        s.eval::<bool>("return UnitPower == nil and UnitPowerMax == nil")
            .unwrap(),
        "the Era spellings must not linger beside the 1.12 ones"
    );
    // An absent unit: the NUMBER 0 — the same value Mana has, and never nil. Load-bearing rather
    // than tidy: stock `UnitFrame.lua:122` writes `ManaBarColor[UnitPowerType(unitFrame.unit)]`,
    // and a nil there indexes nothing.
    assert_eq!(
        s.eval::<i64>(r#"return UnitPowerType("target")"#).unwrap(),
        0
    );
}

#[test]
fn get_money_reads_the_pushed_purse() {
    let mut s = UiScript::new().unwrap();
    // No feed yet: 0 copper.
    assert_eq!(s.eval::<i64>("return GetMoney()").unwrap(), 0);
    s.set_money(123_456);
    assert_eq!(s.eval::<i64>("return GetMoney()").unwrap(), 123_456);
}

#[test]
fn unit_xp_reads_the_player_globals_only_for_the_player_token() {
    let mut s = UiScript::new().unwrap();
    // No feed yet: 0/0.
    assert_eq!(s.eval::<i64>(r#"return UnitXP("player")"#).unwrap(), 0);
    assert_eq!(s.eval::<i64>(r#"return UnitXPMax("player")"#).unwrap(), 0);
    s.set_player_xp(4200, 6000);
    assert_eq!(s.eval::<i64>(r#"return UnitXP("player")"#).unwrap(), 4200);
    assert_eq!(
        s.eval::<i64>(r#"return UnitXPMax("player")"#).unwrap(),
        6000
    );
    // Any non-player token reports 0 (no creature/other unit exposes XP — the live shape).
    assert_eq!(s.eval::<i64>(r#"return UnitXP("target")"#).unwrap(), 0);
    assert_eq!(s.eval::<i64>(r#"return UnitXPMax("target")"#).unwrap(), 0);
}

#[test]
fn set_unit_none_removes_the_token() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));
    assert_eq!(s.eval::<i64>(r#"return UnitExists("player")"#).unwrap(), 1);
    s.set_unit("player", None);
    assert!(s
        .eval::<bool>(r#"return UnitExists("player") == nil"#)
        .unwrap());
    assert_eq!(s.eval::<i64>(r#"return UnitHealth("player")"#).unwrap(), 0);
}

#[test]
fn party_frame_predicates_report_1_or_nil() {
    let mut s = UiScript::new().unwrap();
    s.set_unit(
        "party1",
        Some(UnitState {
            exists: true,
            is_connected: true,
            is_afk: true,
            is_dnd: false,
            pvp: true,
            is_pvp_ffa: false,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsConnected("party1")"#)
            .unwrap(),
        1
    );
    assert_eq!(s.eval::<i64>(r#"return UnitIsAFK("party1")"#).unwrap(), 1);
    assert!(s
        .eval::<bool>(r#"return UnitIsDND("party1") == nil"#)
        .unwrap());
    assert_eq!(s.eval::<i64>(r#"return UnitIsPVP("party1")"#).unwrap(), 1);
    assert!(s
        .eval::<bool>(r#"return UnitIsPVPFreeForAll("party1") == nil"#)
        .unwrap());

    // Flip every flag: the false/true cases aren't just "always true".
    s.set_unit(
        "party1",
        Some(UnitState {
            exists: true,
            is_connected: false,
            is_afk: false,
            is_dnd: true,
            pvp: false,
            is_pvp_ffa: true,
            ..Default::default()
        }),
    );
    assert!(s
        .eval::<bool>(r#"return UnitIsConnected("party1") == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return UnitIsAFK("party1") == nil"#)
        .unwrap());
    assert_eq!(s.eval::<i64>(r#"return UnitIsDND("party1")"#).unwrap(), 1);
    assert!(s
        .eval::<bool>(r#"return UnitIsPVP("party1") == nil"#)
        .unwrap());
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsPVPFreeForAll("party1")"#)
            .unwrap(),
        1
    );

    // An absent token reports nil regardless of any flag's zero value.
    assert!(s
        .eval::<bool>(r#"return UnitIsConnected("party2") == nil"#)
        .unwrap());
}

#[test]
fn unit_race_class_sex_report_the_snapshot_or_the_absent_shape() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));
    assert_eq!(
        s.eval::<(String, String)>(r#"return UnitRace("player")"#)
            .unwrap(),
        ("Night Elf".into(), "NightElf".into())
    );
    assert_eq!(
        s.eval::<(String, String)>(r#"return UnitClass("player")"#)
            .unwrap(),
        ("Warrior".into(), "WARRIOR".into())
    );
    assert_eq!(s.eval::<i64>(r#"return UnitSex("player")"#).unwrap(), 3);
    // An absent token: the two string pairs go nil, nil — but **`UnitSex` answers the number 2**,
    // not nil. It is a numeric getter, and `0x517f9f` pushes the constant double 2.0 on the
    // unresolved leg exactly as `UnitLevel`/`UnitMana`/`GetMoney` push 0.0 on theirs (decision
    // 2118). The shapes table has no nil alternative for this row at all.
    assert!(s
        .eval::<bool>(r#"local a, b = UnitRace("target") return a == nil and b == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"local a, b = UnitClass("target") return a == nil and b == nil"#)
        .unwrap());
    assert_eq!(s.eval::<i64>(r#"return UnitSex("target")"#).unwrap(), 2);
    // A snapshot whose race/class haven't resolved yet (feed pending): same nils.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            ..Default::default()
        }),
    );
    assert!(s
        .eval::<bool>(r#"local a, b = UnitRace("target") return a == nil and b == nil"#)
        .unwrap());
    // Same for a seated body whose sex byte has not streamed: the number 2, never nil.
    assert_eq!(s.eval::<i64>(r#"return UnitSex("target")"#).unwrap(), 2);
}

/// `UnitFactionGroup` returns the (english, localized) pair the PvP-icon law reads, and `nil, nil`
/// for a unit with no side — the state the reference's `if ( factionGroup and … )` gate exists
/// for. `TogglePVP` queues one toggle per call.
#[test]
fn faction_group_pair_and_the_pvp_toggle() {
    let mut s = UiScript::new().unwrap();
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            faction_group: Some("Horde".into()),
            ..Default::default()
        }),
    );
    let (english, localized) = s
        .eval::<(String, String)>(r#"return UnitFactionGroup("target")"#)
        .unwrap();
    // With no localized name fed, the second return falls back to the English one rather than nil:
    // FactionGroup.dbc leaves `Name0` empty for some rows, and nil here would blank a tooltip the
    // reference fills.
    assert_eq!((english.as_str(), localized.as_str()), ("Horde", "Horde"));

    // **The two halves are DIFFERENT fields and this is the assertion that says so.** They are
    // FactionGroup.dbc's `InternalName` and `Name0`; only on enUS do they hold the same word, which
    // is why returning one twice went unnoticed. The first is concatenated into a texture path by
    // every stock consumer (`"…\UI-PVP-"..factionGroup` — PlayerFrame.lua:68, TargetFrame.lua:198,
    // PartyMemberFrame.lua:125; `"…\Battleground-"..` — BattlefieldFrame.lua:195), so a localized
    // string there names a file that does not exist. This fixture is a deDE-shaped client.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            faction_group: Some("Horde".into()),
            faction_group_localized: Some("Horde-Allianz".into()),
            ..Default::default()
        }),
    );
    let (english, localized) = s
        .eval::<(String, String)>(r#"return UnitFactionGroup("target")"#)
        .unwrap();
    assert_eq!(
        (english.as_str(), localized.as_str()),
        ("Horde", "Horde-Allianz"),
        "the FIRST return is the English InternalName — a texture path is built from it"
    );

    // No side (a Monster/neutral template, or a unit whose template hasn't streamed).
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            faction_group: None,
            ..Default::default()
        }),
    );
    assert!(s
        .eval::<bool>(r#"local a, b = UnitFactionGroup("target") return a == nil and b == nil"#)
        .unwrap());
    // An absent unit answers the same way. (This read `"focus"` until the unrecognised-token raise
    // landed — a token 1.12 does not have at all, so it now raises rather than being "absent". The
    // question the line means to ask needs a RECOGNISED token that names nothing.)
    assert!(s
        .eval::<bool>(r#"local a = UnitFactionGroup("party4") return a == nil"#)
        .unwrap());

    assert_eq!(s.take_pvp_toggles(), 0, "nothing queued yet");
    s.run("TogglePVP() TogglePVP()").unwrap();
    assert_eq!(
        s.take_pvp_toggles(),
        2,
        "one send per call, never collapsed"
    );
    assert_eq!(s.take_pvp_toggles(), 0, "the drain empties the queue");
}

/// `UnitClassification` (byte-verified table `0x850424`): five words indexed by the
/// gated rank, a STRING for every input — including an absent token, which the binary answers
/// `"normal"` for (its unresolved path loads index 0), never nil.
#[test]
fn unit_classification_reads_the_five_word_table() {
    let mut s = UiScript::new().unwrap();

    for (rank, word) in [
        (0, "normal"),
        (1, "elite"),
        (2, "rareelite"),
        (3, "worldboss"),
        (4, "rare"),
    ] {
        s.set_unit(
            "target",
            Some(UnitState {
                exists: true,
                rank,
                ..UnitState::default()
            }),
        );
        assert_eq!(
            s.eval::<String>(r#"return UnitClassification("target")"#)
                .unwrap(),
            word,
            "rank {rank}"
        );
    }

    // No snapshot at all, and a missing argument: still "normal", never nil. The target frame's
    // border branch compares this against string literals, so a nil here would be a Lua error the
    // reference cannot produce.
    s.set_unit("target", None);
    assert_eq!(
        s.eval::<String>(r#"return UnitClassification("target")"#)
            .unwrap(),
        "normal",
        "an absent unit is normal, not nil"
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitClassification()"#).unwrap(),
        "normal",
        "a missing token is normal, not nil"
    );
}

// ── `UnitAffectingCombat` and `TargetByName` ────────────────────────────────────────────────────

/// `UnitAffectingCombat` → the number 1 / nil, and **one arm serves both "not in combat" and "no
/// such unit"** (`0x517e48` and `0x517e5c` jump to the same `0x517e73`). That indistinguishability
/// is the behaviour, not a shortcut: an addon must not be able to probe existence with this verb.
#[test]
fn unit_affecting_combat_is_one_or_nil_and_hides_the_missing_unit() {
    let mut s = UiScript::new().unwrap();
    let mut hot = player();
    hot.in_combat = true;
    s.set_unit("player", Some(hot));
    s.set_unit("target", Some(player())); // exists, peaceful

    assert_eq!(
        s.eval::<i64>(r#"return UnitAffectingCombat("player")"#)
            .unwrap(),
        1,
        "the number 1, never a boolean"
    );
    assert!(s
        .eval::<bool>(r#"return UnitAffectingCombat("target") == nil"#)
        .unwrap());
    assert!(
        s.eval::<bool>(r#"return UnitAffectingCombat("party3") == nil"#)
            .unwrap(),
        "an unresolvable token is the SAME nil a peaceful unit gives"
    );
    // **A number is accepted and stringified — and that is exactly why it RAISES.** The gate
    // `0x6f3510` is number-OR-string, so `5` is coerced via `%.14g` to the token `"5"`; `"5"` then
    // matches none of the resolver's nine prefixes and falls into
    // `luaL_error("Unknown unit name: %s")`. This assertion read `== nil` and "it does not raise"
    // until corrected: the acceptance was right, the conclusion was not.
    assert!(
        s.run("UnitAffectingCombat(5)").is_err(),
        "a number is coerced to a token that matches nothing, so it raises"
    );
}

/// A missing or wrong-typed argument **raises** — `0x6f4940` is `luaL_error` and does not return,
/// so the caller's statement is abandoned rather than continuing with a nil.
#[test]
fn unit_affecting_combat_raises_on_a_bad_argument() {
    let s = UiScript::new().unwrap();
    for call in ["UnitAffectingCombat()", "UnitAffectingCombat({})"] {
        let err = s
            .eval::<mlua::Value>(&format!("return {call}"))
            .unwrap_err();
        assert!(
            format!("{err}").contains(r#"Usage: UnitAffectingCombat("unit")"#),
            "{call} must raise the usage line, got {err}"
        );
    }
}

/// `TargetByName(name [, exactMatch])` queues the name **and the second argument** — the flag the
/// slash command has no way to supply, which turns the resolver's longest-common-prefix tier off.
#[test]
fn target_by_name_queues_the_name_and_the_exact_flag() {
    let mut s = UiScript::new().unwrap();
    assert!(s.take_target_by_name_requests().is_empty());

    s.run(r#"TargetByName("Rag")"#).unwrap();
    s.run(r#"TargetByName("Ragnaros", 1)"#).unwrap();
    s.run(r#"TargetByName("Ragnaros", 0)"#).unwrap();
    s.run(r#"TargetByName("Ragnaros", true)"#).unwrap();
    s.run("TargetByName(5)").unwrap(); // a number stringifies, as `0x6f3690` does
    assert_eq!(
        s.take_target_by_name_requests(),
        vec![
            ("Rag".to_string(), false),
            ("Ragnaros".to_string(), true),
            ("Ragnaros".to_string(), false),
            ("Ragnaros".to_string(), true),
            ("5".to_string(), false),
        ]
    );
    assert!(s.take_target_by_name_requests().is_empty());

    // A missing/wrong-typed name raises (`0x489d69 call 0x6f3510` → `0x489de1`).
    let err = s.eval::<mlua::Value>("return TargetByName()").unwrap_err();
    assert!(
        format!("{err}").contains(r#"Usage: TargetByName("name")"#),
        "got {err}"
    );
}

/// **`UnitIsCharmed` — the number `1`, the nil, and the asymmetry.**
///
/// `KLHThreatMeter\Code\KTM_My.lua:533` is the corpus line blocked on it; seven other addons name
/// it. The binding (`0x516cf0`) reads `UNIT_FIELD_CHARMEDBY != 0` — a 64-bit non-zero test on
/// fields 10/11, **not** a `UNIT_FIELD_FLAGS` bit, which is what a boolean-shaped unit question
/// invites you to assume.
#[test]
fn unit_is_charmed_answers_one_or_nil_and_only_for_the_charmed_side() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));
    let mut mc = player();
    mc.charmed = true;
    s.set_unit("target", Some(mc));

    // A hit is the NUMBER 1 — `lua_pushnumber` writes tag 3; tag 1 (boolean) is never written on
    // either arm, so `== true` in an addon must NOT match.
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsCharmed("target")"#).unwrap(),
        1
    );
    assert_eq!(
        s.eval::<String>(r#"return type(UnitIsCharmed("target"))"#)
            .unwrap(),
        "number",
        "the reference pushes a number, not a boolean"
    );
    assert_eq!(
        s.arity(r#"UnitIsCharmed("target")"#).unwrap(),
        1,
        "exactly one value on the hit path"
    );

    // An uncharmed unit and an absent one both answer nil — and nil, not `false` and not `0`.
    for token in ["player", "party1"] {
        assert!(
            s.eval::<Option<i64>>(&format!(r#"return UnitIsCharmed("{token}")"#))
                .unwrap()
                .is_none(),
            "{token} is not charmed, so the answer is nil"
        );
        assert_eq!(
            s.eval::<String>(&format!(r#"return type(UnitIsCharmed("{token}"))"#))
                .unwrap(),
            "nil",
            "…nil rather than false or 0"
        );
    }

    // **The asymmetry.** The field is "who charms me", never "whom I charm", so the charmer reads
    // nil while its victim reads 1 — here the player is doing the charming and is not charmed.
    assert!(
        s.eval::<Option<i64>>(r#"return UnitIsCharmed("player")"#)
            .unwrap()
            .is_none(),
        "a charmER is not charmed; UNIT_FIELD_CHARM is never read by this binding"
    );
}

/// **`UnitIsPlusMob` reads a FLAG BIT, not the creature rank** (`0x516d40`).
///
/// Decision 2209. The name says "elite" and its table neighbour `UnitClassification` answers off
/// the gated rank, so the natural implementation is `rank > 0`. The binary's is not: it takes
/// `UNIT_FIELD_FLAGS` and tests bit 6 (`UNIT_FLAG_PLUS_MOB 0x40`), never calling the rank getter
/// `0x605620`). The two normally
/// agree because the server derives the bit from the rank — so **rare** answers 1 here, not just
/// elite — and they part on the unit whose creature-cache record has not arrived, which still
/// carries its own flags while its rank reads 0.
///
/// That divergence is the whole reason this is a test and not a reading: a `rank > 0`
/// implementation passes every ordinary case and is wrong exactly where the flag is the only
/// thing the client has.
#[test]
fn unit_is_plus_mob_reads_the_flag_bit_and_not_the_rank() {
    let with = |flags: u32, rank: u32| UnitState {
        exists: true,
        has_object: true,
        flags,
        rank,
        ..Default::default()
    };
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));

    // The ordinary agreeing cases: the server sets the bit on `!IsPet() && rank > 0`, so elite,
    // rare-elite, world boss AND rare all carry it, and a normal mob does not.
    for (rank, word) in [
        (1u32, "elite"),
        (2, "rareelite"),
        (3, "worldboss"),
        (4, "rare"),
    ] {
        s.set_unit("target", Some(with(0x40, rank)));
        assert_eq!(
            s.eval::<i64>(r#"return UnitIsPlusMob("target")"#).unwrap(),
            1,
            "a {word} carries UNIT_FLAG_PLUS_MOB, so the answer is the number 1"
        );
    }
    s.set_unit("target", Some(with(0, 0)));
    assert!(
        s.eval::<Option<i64>>(r#"return UnitIsPlusMob("target")"#)
            .unwrap()
            .is_none(),
        "a normal mob answers nil"
    );

    // ── The two legs a rank read gets wrong ──
    // The flag WITHOUT a rank: an unstreamed creature, whose cache record has not arrived, still
    // has its own descriptor. `rank > 0` would call it normal; the client says 1.
    s.set_unit("target", Some(with(0x40, 0)));
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsPlusMob("target")"#).unwrap(),
        1,
        "the bit alone decides — no creature-cache rank is consulted"
    );
    // A rank WITHOUT the flag — a player's pet, which the server excludes (`!IsPet()`).
    s.set_unit("target", Some(with(0x8, 2)));
    assert!(
        s.eval::<Option<i64>>(r#"return UnitIsPlusMob("target")"#)
            .unwrap()
            .is_none(),
        "no bit, no plus mob — a neighbouring flag (player-controlled) must not read as one"
    );

    // The token grammar: quiet on nil/absent (one of 1834's thirteen with no `lua_isstring`
    // gate), quiet on a recognised token naming nothing, and a raise on an unrecognised one —
    // the body reaches the shared resolver through `0x515940`.
    assert!(s.run("UnitIsPlusMob()").is_ok(), "nil token stays quiet");
    assert!(
        s.run("UnitIsPlusMob(nil)").is_ok(),
        "…and so does an explicit nil"
    );
    assert!(
        s.eval::<Option<i64>>(r#"return UnitIsPlusMob("party5")"#)
            .unwrap()
            .is_none(),
        "recognised but naming nothing — nil, not a raise"
    );
    let err = s.run(r#"UnitIsPlusMob("bogus")"#).unwrap_err().to_string();
    assert!(
        err.contains("Unknown unit name: bogus"),
        "an unrecognised token raises the resolver's own text: {err}"
    );
}

/// **Unit tokens fold case, because the client's resolver does.**
///
/// `0x515970` compares every one of its literals with `SStrCmpI` → `_strnicmp`, whose fold is
/// `'A'..'Z' += 0x20`; not one of its ten compares reaches the case-sensitive sibling. So this is
/// a NARROWING fix — the real client resolves these and we did not — rather than the superset 1189
/// had to take back out.
///
/// `Accountant.lua:107` is the corpus line that paid for it: `UnitFactionGroup("Player")`, capital
/// P, whose nil made `Accountant_SaveData[realm][faction]` a nil table index at l.192 and ended the
/// addon's session. Roughly ten addons pass `Player`, `PLAYER`, `Target`, `Pet`, `PARTY`, `NPC`,
/// `Mouseover` or `"Raid"..i`.
#[test]
fn a_unit_token_resolves_whatever_its_case() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));
    let mut tgt = player();
    tgt.name = Some("Hogger".into());
    s.set_unit("target", Some(tgt));

    // Every spelling names the same unit — the getter, the number and the predicate alike.
    for spelling in ["player", "Player", "PLAYER", "PlAyEr"] {
        assert_eq!(
            s.eval::<String>(&format!(r#"return UnitName("{spelling}")"#))
                .unwrap(),
            "Benilla",
            "UnitName(\"{spelling}\") must resolve"
        );
        assert_eq!(
            s.eval::<i64>(&format!(r#"return UnitLevel("{spelling}")"#))
                .unwrap(),
            12
        );
        assert_eq!(
            s.eval::<i64>(&format!(r#"return UnitExists("{spelling}")"#))
                .unwrap(),
            1
        );
    }
    for spelling in ["target", "Target", "TARGET"] {
        assert_eq!(
            s.eval::<String>(&format!(r#"return UnitName("{spelling}")"#))
                .unwrap(),
            "Hogger"
        );
    }

    // **The two-unit call, which the map's fold does not cover on its own.**
    // `pick_unit_token` chooses "whichever arg is not the player"; with a case-sensitive test,
    // `("Player", "target")` reads as a non-player FIRST arg and the call answers about the wrong
    // unit — a wrong answer rather than a missing one.
    assert!(
        s.eval::<bool>(
            r#"return UnitIsEnemy("Player", "target") == UnitIsEnemy("player", "target")"#
        )
        .unwrap(),
        "the directional pick folds case too"
    );

    // An unseated but RECOGNISED token is still absent whatever its case — folding must not invent
    // units. (`bogus` moved out of this list when the unrecognised-token raise landed: it is not
    // "absent", it is not a unit token at all, and the client says so.)
    for spelling in ["Party1", "PARTY1", "raid9", "MouseOver"] {
        assert!(
            !s.eval::<bool>(&format!(r#"return UnitExists("{spelling}")"#))
                .unwrap(),
            "{spelling} was never seated"
        );
    }
}

/// The fold is on the way **IN** as well as out: a feed that pushes `"Target"` must not create a
/// second entry shadowing `"target"`, which is what a lookup-only fold would allow.
#[test]
fn seating_a_token_folds_its_key_too() {
    let mut s = UiScript::new().unwrap();
    let mut a = player();
    a.name = Some("First".into());
    s.set_unit("Target", Some(a));
    assert_eq!(
        s.eval::<String>(r#"return UnitName("target")"#).unwrap(),
        "First",
        "a token seated as \"Target\" is readable as \"target\""
    );

    // …and clearing it through the other spelling really clears it, rather than leaving a shadow.
    s.set_unit("TARGET", None);
    assert!(
        s.eval::<bool>(r#"return UnitExists("target") == nil"#)
            .unwrap(),
        "removal folds too — no shadowed entry survives"
    );
}

/// **An unrecognised token RAISES; a recognised one that names nothing is a quiet nil.**
///
/// `0x515970` falls off the end of its nine compares into `luaL_error(L, "Unknown unit name: %s")`,
/// which longjmps and never returns. Ours answered nil for everything unknown — 1203's shape
/// pointed the other way, a failure the client reports and we swallowed.
///
/// The split is THREE-way, and the two quiet legs are the ones a "raise on nil" implementation gets
/// wrong.
#[test]
fn an_unrecognised_unit_token_raises_and_a_recognised_empty_one_does_not() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));

    // RAISE: nothing the resolver's compares match.
    for bogus in ["bogus", "focus", "boss1", "arena1"] {
        let err = s
            .run(&format!(r#"UnitName("{bogus}")"#))
            .expect_err(&format!("{bogus} must raise"))
            .to_string();
        assert!(
            err.contains(&format!("Unknown unit name: {bogus}")),
            "the raise carries the client's own text and the offending token: {err}"
        );
    }
    // `npc` is the SOLE full-string compare, so a suffix on it matches nothing at all.
    assert!(s.run(r#"UnitName("npctarget")"#).is_err());
    // …while `npc` itself is recognised, folded like everything else.
    assert!(s.run(r#"UnitName("NPC")"#).is_ok());

    // QUIET NIL, leg 1: a recognised token naming nothing here.
    for absent in [
        "party5",
        "raid17",
        "pet",
        "mouseover",
        "partypet2",
        "raidpet3",
    ] {
        assert!(
            s.eval::<Option<String>>(&format!(r#"return UnitName("{absent}")"#))
                .unwrap()
                .is_none(),
            "{absent} is recognised and absent — nil, not a raise"
        );
    }
    // QUIET NIL, leg 2: a recognised PREFIX with a junk suffix. The compares are prefix tests that
    // stop at either NUL, so `playerfoo` matches `player` and never reaches the raise.
    for junk in ["playerfoo", "raidx", "petx", "targetish"] {
        assert!(
            s.eval::<Option<String>>(&format!(r#"return UnitName("{junk}")"#))
                .unwrap()
                .is_none(),
            "{junk} matches a PREFIX, so it is a quiet nil — not a raise. This is the leg an \
             implementation that raises on \"did not resolve\" gets wrong."
        );
    }
    // QUIET NIL, leg 3: the empty string — which passes the `lua_isstring` gate (it IS a string)
    // and dies quietly in the resolver's own NULL/empty guard.
    assert!(s.run(r#"UnitName("")"#).is_ok());

    // …but an ABSENT or nil argument is NOT leg 3, and this line used to assert that it was.
    // `UnitName 0x517020` gates its token position at `0x517048` and raises
    // `Usage: UnitName("unit")` (`0x850ee0`); `luaL_error` does not return. The comment this
    // replaces said the per-binding gates were "not uniform … only two poles verified", which was
    // true when it was written — all 83 entries of the table at `0x850438` gate and raise in 53
    // cases, and only 13 unit-token bindings are quiet.
    assert!(s.run("UnitName()").is_err(), "absent argument raises");
    assert!(s.run("UnitName(nil)").is_err(), "nil argument raises");
    // A NUMBER passes the gate — `lua_isstring` admits tag 3 — and is handed to the resolver as
    // "5", which then raises the family's OTHER message rather than the `Usage:` one.
    assert!(
        s.run("UnitName(5)").is_err(),
        "a number reaches the resolver"
    );

    // The thirteen that stay quiet, three of them here: no gate at all, so nil is fine.
    for quiet in ["UnitExists", "UnitIsVisible", "UnitClassification"] {
        assert!(
            s.run(&format!("{quiet}()")).is_ok(),
            "{quiet} is one of the 13 with no gate — nil must stay quiet"
        );
    }

    // A two-unit call gates BOTH arguments.
    assert!(s.run(r#"UnitIsUnit("player", "bogus")"#).is_err());
    assert!(s.run(r#"UnitIsUnit("bogus", "player")"#).is_err());
    assert!(s.run(r#"UnitIsUnit("player", "party3")"#).is_ok());
}

/// **A multibyte unit token must not take the client down.**
///
/// The prefix test used to slice the token as a `&str` — `token[..p.len()]` — which PANICS when the
/// token is multibyte UTF-8 and the prefix length lands mid-character ("byte index N is not a char
/// boundary"). Any addon passing a non-ASCII token would have crashed the process rather than
/// getting the `Unknown unit name` raise the client gives.
///
/// The client compares with `_strnicmp` over BYTES and folds ASCII only, so byte comparison is both
/// the safe form and the faithful one — a token whose bytes differ above 0x7F is simply not one of
/// the nine prefixes.
#[test]
fn a_multibyte_unit_token_raises_rather_than_panicking() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));

    // Each of these has a multibyte character positioned so that a byte-index slice at one of the
    // prefix lengths (3 for `pet`, 4 for `raid`, 5 for `party`, 6 for `player`/`target`) would land
    // inside it.
    for token in ["pé", "раid", "playér", "targét", "мышь", "日本語のトークン"] {
        let err = s
            .run(&format!(r#"UnitName("{token}")"#))
            .expect_err("a non-ASCII token matches no prefix, so it raises")
            .to_string();
        assert!(
            err.contains("Unknown unit name"),
            "it must RAISE, not panic and not answer: {err}"
        );
    }
    // …and an ASCII token that merely shares a prefix's leading bytes still behaves.
    assert!(s
        .eval::<Option<String>>(r#"return UnitName("playerfoo")"#)
        .unwrap()
        .is_none());
    assert_eq!(
        s.eval::<String>(r#"return UnitName("player")"#).unwrap(),
        "Benilla"
    );
}

/// **`UnitIsVisible` is object presence, and it is NOT `UnitExists`** — the pair an out-of-range
/// party member separates, and the branch pfUI takes seven times.
///
/// `0x516030` is 57 bytes with one branch:
/// `ClntObjMgrObjectPtr(resolve(token), TYPEMASK_UNIT) != NULL`. No field read, no comparison
/// beyond `test eax,eax`, no float opcode — so no distance, no radius, no visibility flag. The
/// range test is the *server's*: the out-of-range demotion unlinks the object from the very
/// manager index this query searches.
///
/// The state that backs it is [`UnitState::has_object`], which `ui_party::feed`'s
/// `member_unit_state` has always branched on — its `store: Option<&ObjectStore>` argument, whose
/// own comment names `0x468460`. This test is what stops the two fields being conflated back
/// together, because in every *in-range* case they agree and only the out-of-range one tells them
/// apart.
#[test]
fn unit_is_visible_is_object_presence_not_existence() {
    let mut s = UiScript::new().unwrap();

    // In range: the object is held, both answer yes.
    s.set_unit(
        "party1",
        Some(UnitState {
            exists: true,
            has_object: true,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsVisible("party1")"#).unwrap(),
        1,
        "a held object is visible — and the return is the NUMBER 1, never a boolean"
    );

    // **Out of range: the roster still has them, the object manager does not.** This is the row
    // that matters; `exists` MUST stay true here, because that is what UnitExists's own GUID
    // fallback answers, and folding the two fields into one would silently break it.
    s.set_unit(
        "party1",
        Some(UnitState {
            exists: true,
            has_object: false,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<Option<i64>>(r#"return UnitIsVisible("party1")"#)
            .unwrap(),
        None,
        "no object -> nil, even though the token still exists"
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitExists("party1")"#).unwrap(),
        1,
        "...and UnitExists must NOT have moved with it — the roster fallback is the difference"
    );

    // pfUI's actual line (`unitframes.lua:1873`), which chooses a flat portrait over a 3D one.
    assert!(
        s.eval::<bool>(
            r#"return (not UnitIsVisible("party1") or not UnitIsConnected("party1")) and true or false"#
        )
        .unwrap(),
        "the portrait branch fires for an out-of-range member"
    );

    // A token with no snapshot at all: nil, via the same "no snapshot" path every predicate uses.
    assert_eq!(
        s.eval::<Option<i64>>(r#"return UnitIsVisible("party4")"#)
            .unwrap(),
        None
    );
}

/// **The tapped pair, and the two ways it goes wrong.**
///
/// `UnitIsTapped 0x519c90` / `UnitIsTappedByPlayer 0x519d00` are a masked-byte pair — 108 bytes
/// each, differing only in the `UNIT_DYNAMIC_FLAGS` mask (`0x4`/`0x8`, UpdateField 143) and the
/// `Usage:` string. Each is `object present && (flags & mask)` and nothing else: no ownership, no
/// GUID compare, no party/raid or health conjunct.
///
/// Asserted here because both are easy to get wrong in a way that still looks right:
///
/// - **Shape A, not shape C.** `UnitIsVisible` beside them has *no* `Usage:` raise; these two do.
///   Inheriting the neighbour's shape would fail quietly, in the permissive direction.
/// - **The pair is read as a conjunction.** `tapped && not tappedByPlayer` — someone *else's* kill
///   — is the condition addons actually draw, and either bit alone says nothing useful.
#[test]
fn the_tapped_pair_is_two_masks_of_one_field_and_raises_on_a_bad_argument() {
    let mut s = UiScript::new().unwrap();
    let set = |s: &mut UiScript, tapped, by_player| {
        s.set_unit(
            "target",
            Some(UnitState {
                exists: true,
                has_object: true,
                tapped,
                tapped_by_player: by_player,
                ..Default::default()
            }),
        );
    };

    // Untapped: both nil.
    set(&mut s, false, false);
    assert_eq!(
        s.eval::<Option<i64>>(r#"return UnitIsTapped("target")"#)
            .unwrap(),
        None
    );

    // Tapped by someone ELSE — the grey-bar case, pfUI `api/unitframes.lua:2012`.
    set(&mut s, true, false);
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsTapped("target")"#).unwrap(),
        1,
        "the return is the NUMBER 1, never a boolean"
    );
    assert_eq!(
        s.eval::<Option<i64>>(r#"return UnitIsTappedByPlayer("target")"#)
            .unwrap(),
        None
    );
    assert!(
        s.eval::<bool>(
            r#"return (UnitIsTapped("target") and not UnitIsTappedByPlayer("target")) and true or false"#
        )
        .unwrap(),
        "someone else's kill — the conjunction addons actually draw"
    );

    // Tapped by ME: both set, so the grey-bar branch must NOT fire.
    set(&mut s, true, true);
    assert!(
        !s.eval::<bool>(
            r#"return (UnitIsTapped("target") and not UnitIsTappedByPlayer("target")) and true or false"#
        )
        .unwrap(),
        "my own tap is not greyed"
    );

    // Object presence is a conjunct of BOTH: an out-of-range unit is neither, whatever the bits
    // say — and the bits cannot even be read, because there is no descriptor to read them from.
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            has_object: false,
            tapped: true,
            tapped_by_player: false,
            ..Default::default()
        }),
    );
    // (The feed can never build that combination — `ui_unit::snapshot` sets all three together —
    // but the binding is asserted against it anyway, because a future feed that forgot would
    // otherwise report a tapped unit the object manager does not hold.)
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsTapped("target")"#).unwrap(),
        1,
        "the binding reads the field it is given; the conjunct is enforced at the FEED, and this \
         assertion records which of the two owns it"
    );

    // SHAPE A. `UnitIsVisible` next door has no `Usage:` raise; these do — and **nil is in the
    // list**, which is the half that does not come free: the shared `check_unit_token` lets a nil
    // through by design (right for `UnitExists` and `UnitIsVisible`, wrong for these two), so the
    // gate has to live in the binding. This assertion is what found that; the original spelling
    // used `print`, which is not defined in our VM and so was quietly testing nil all along.
    for bad in ["{}", "true", "nil", "", "function() end"] {
        assert!(
            s.run(&format!("UnitIsTapped({bad})")).is_err(),
            "UnitIsTapped({bad}) must raise — shape A, not the neighbour's shape C"
        );
        assert!(
            s.run(&format!("UnitIsTappedByPlayer({bad})")).is_err(),
            "UnitIsTappedByPlayer({bad}) must raise too — the pair shares its gate"
        );
    }
}

/// **`UnitIsPartyLeader` is two legs ORed, and the solo case answers 1.**
///
/// `0x516210` is
///
/// ```text
/// o = ObjPtr(resolve(t), TYPEMASK_PLAYER)
/// (o != NULL && (o.PLAYER_FLAGS & 0x1)) || resolve(t) == g_groupLeaderGuid
/// ```
///
/// and it is **not** derivable from `IsPartyLeader()` + `GetPartyLeaderIndex()` however arranged —
/// the two legs cover disjoint failures. Each leg is asserted with the other one dead, because
/// either alone looks sufficient until the case it cannot reach.
#[test]
fn unit_is_party_leader_ors_two_legs_and_answers_one_when_solo() {
    let mut s = UiScript::new().unwrap();
    const ME: u64 = 0x1111;
    const THEM: u64 = 0x2222;

    // ── LEG 1 alone: the descriptor flag, with the leader GUID matching nobody.
    //    This is the leg that answers for a STRANGER who leads their own party — something no
    //    comparison against our group's leader could ever express.
    s.set_party(PartyState {
        leader_guid: 0x9999,
        own_guid: 0,
        ..Default::default()
    });
    s.set_unit(
        "target",
        Some(UnitState {
            exists: true,
            has_object: true,
            guid: THEM,
            group_leader: true,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsPartyLeader("target")"#)
            .unwrap(),
        1,
        "the server's PLAYER_FLAGS bit answers on its own"
    );

    // ── LEG 2 alone: no descriptor flag, but the resolved GUID IS our leader. This is the
    //    out-of-range member — the client holds no object, so leg 1 has nothing to read.
    s.set_party(PartyState {
        leader_guid: THEM,
        own_guid: 0,
        ..Default::default()
    });
    s.set_unit(
        "party1",
        Some(UnitState {
            exists: true,
            has_object: false,
            guid: THEM,
            group_leader: false,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsPartyLeader("party1")"#)
            .unwrap(),
        1,
        "the GUID compare answers where there is no descriptor to read a flag from"
    );

    // ── Neither leg: a grouped member who does not lead.
    s.set_unit(
        "party2",
        Some(UnitState {
            exists: true,
            has_object: true,
            guid: ME,
            group_leader: false,
            ..Default::default()
        }),
    );
    assert_eq!(
        s.eval::<Option<i64>>(r#"return UnitIsPartyLeader("party2")"#)
            .unwrap(),
        None
    );

    // ── **NO ZERO GUARD.** Ungrouped, the cached leader is 0 and an unresolvable-but-non-raising
    //    token resolves to 0 — so they match and the answer is 1. `IsPartyLeader 0x4e9130`
    //    short-circuits on a `0:0` leader and `0x516210` does not; that asymmetry is the finding,
    //    and answering nil here would be the divergence, however much it reads like a bug.
    s.set_party(PartyState::default());
    assert_eq!(
        s.eval::<i64>("return UnitIsPartyLeader(nil)").unwrap(),
        1,
        "solo + unresolvable token: 0 == 0, and the reference answers 1"
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitIsPartyLeader("target")"#)
            .unwrap(),
        1,
        "...and so does a token with no snapshot, by the same route"
    );

    // A bad token still raises through the shared resolver — shape C with a shape-A tail.
    assert!(s.run(r#"UnitIsPartyLeader("notatoken")"#).is_err());
}

/// `UnitHasRelicSlot` — the number 1 or nil, per token, never a boolean.
///
/// This shipped **absent** for months on the belief that the relic slot post-dates 1.12, which is
/// false. Stock `PaperDollFrame.lua` calls it unconditionally at l.429 and l.580,
/// so while it was missing the character sheet raised `attempt to call global` for every class —
/// which is why the nil-global case is asserted here too, not just the answer.
#[test]
fn unit_has_relic_slot_answers_one_or_nil() {
    let mut s = UiScript::new().unwrap();

    let mut druid = player();
    druid.class = Some("Druid".into());
    druid.class_file = Some("DRUID".into());
    druid.has_relic_slot = true;
    s.set_unit("player", Some(druid));

    let mut warrior = player();
    warrior.has_relic_slot = false;
    s.set_unit("target", Some(warrior));

    // The truthy leg is the NUMBER 1 (`lua_pushnumber`, `0x519ec8`) — a caller comparing it to 1
    // is stock idiom, and a boolean would silently fail that.
    assert_eq!(
        s.eval::<String>(r#"return tostring(UnitHasRelicSlot("player"))"#)
            .unwrap(),
        "1"
    );
    assert_eq!(
        s.eval::<String>(r#"return type(UnitHasRelicSlot("player"))"#)
            .unwrap(),
        "number"
    );
    // The false leg is nil, never `false` (`lua_pushnil`, `0x519edb`).
    assert_eq!(
        s.eval::<String>(r#"return type(UnitHasRelicSlot("target"))"#)
            .unwrap(),
        "nil"
    );
    // No `"player"` fast path in the reference, so every token is answered the same way — the
    // stock inspect path at `PaperDollFrame.lua:429` passes the inspected unit, not "player".
    assert_eq!(
        s.eval::<String>(r#"return type(UnitHasRelicSlot("party1"))"#)
            .unwrap(),
        "nil"
    );
    // And it EXISTS — the regression that actually bit. A nil global here takes the whole
    // character sheet down for every class, not just the three that answer 1.
    assert_eq!(
        s.eval::<String>("return type(UnitHasRelicSlot)").unwrap(),
        "function"
    );
}

/// **The two-token predicates gate BOTH positions** — decision 1836, closing the half 1834 left
/// open on purpose.
///
/// 1834 applied the `lua_isstring` gate only where a partial census named a single-token binding,
/// because a census that finds "a gate somewhere in this body" cannot say *which* argument carries
/// it. The complete 83-row table (`0x850438`) resolves it: these seven each carry **two**
/// `lua_isstring` sites, so either argument being nil raises.
#[test]
fn a_two_token_predicate_raises_on_either_nil_argument() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));
    s.set_unit("target", Some(player()));

    for verb in [
        "UnitIsUnit",
        "UnitIsEnemy",
        "UnitIsFriend",
        "UnitCanCooperate",
        "UnitCanAttack",
        "UnitReaction",
    ] {
        assert!(
            s.run(&format!(r#"{verb}("player", "target")"#)).is_ok(),
            "{verb} with both tokens is fine"
        );
        assert!(
            s.run(&format!(r#"{verb}("player")"#)).is_err(),
            "{verb} with the SECOND argument absent must raise"
        );
        assert!(
            s.run(&format!(r#"{verb}(nil, "target")"#)).is_err(),
            "{verb} with the FIRST argument nil must raise"
        );
        assert!(s.run(&format!("{verb}()")).is_err(), "{verb} with neither");
    }

    // `GetRaidTargetIndex` is the one of six previously-unclassified names that takes a token, and
    // it is gated. Its usage string names the argument UNQUOTED — the reference's own spelling.
    assert!(s.run(r#"GetRaidTargetIndex("player")"#).is_ok());
    assert!(s.run("GetRaidTargetIndex()").is_err());

    // …while the other five take no argument at all and have no gate. `GetTimeToWellRested` is a
    // three-instruction stub in the reference that always pushes nil; computing a value there
    // would be divergence, not a feature.
    for none in [
        "GetQuestGreenRange",
        "GetRestState",
        "GetXPExhaustion",
        "GetTimeToWellRested",
        "GetBillingTimeRested",
    ] {
        assert!(s.run(&format!("{none}()")).is_ok(), "{none} takes no token");
    }
}

// ── The family's return SHAPE, pinned per binding ───────────────────────────────────────────────

/// **Every `Unit*` predicate answers the number `1` or `nil` — never a Lua boolean** (decision
/// 2043; the law is 1830's, made for the widget predicates a family earlier).
///
/// The reference cannot produce a boolean here. Its pushes are `lua_pushnumber 0x6f3810` (tag 3,
/// the double `1.0`) and `lua_pushnil 0x6f37f0` (tag 0); `lua_pushboolean 0x6f39f0` has seven call
/// sites image-wide and not one is a registered binding body, so **tag 1 is never written**
/// (`UnitExists 0x515fb0`, `UnitIsVisible 0x516030`, `UnitIsTapped 0x519c90`, `UnitAffectingCombat
/// 0x517e10`, `UnitPlayerControlled 0x516410`, `UnitInRaid 0x516350`, `UnitIsCharmed 0x516cf0` and
/// `UnitHasRelicSlot 0x519e50` each confirm it individually).
///
/// **Truthiness is exactly why this needs a test rather than a reading.** `if UnitIsDead(u)` and
/// `if not UnitIsDead(u)` are identical under either shape, and mlua's `bool` conversion maps
/// `Integer(1)` to `true` as happily as it maps `Boolean(true)` — so a test that reads the value
/// as a Rust `bool` passes under the bug. Only `type(...)`, `== 1` and `== nil` separate them, and
/// `== nil` is the one that *inverts*: a boolean `false` is not `nil`, so a caller comparing
/// against `nil` concludes the opposite of the truth.
///
/// The table is the whole registered family, so a new predicate added outside the seam fails here
/// rather than shipping its own shape.
#[test]
fn every_unit_predicate_is_one_or_nil_and_never_a_boolean() {
    // (binding, the call as an addon writes it, a UnitState that makes it TRUE)
    let table: Vec<(&str, &str, UnitState)> = vec![
        (
            "UnitExists",
            r#"UnitExists("target")"#,
            UnitState {
                exists: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsVisible",
            r#"UnitIsVisible("target")"#,
            UnitState {
                exists: true,
                has_object: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsDead",
            r#"UnitIsDead("target")"#,
            UnitState {
                exists: true,
                dead: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsGhost",
            r#"UnitIsGhost("target")"#,
            UnitState {
                exists: true,
                ghost: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsDeadOrGhost",
            r#"UnitIsDeadOrGhost("target")"#,
            UnitState {
                exists: true,
                dead: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsCorpse",
            r#"UnitIsCorpse("target")"#,
            UnitState {
                exists: true,
                corpse_object: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsPlayer",
            r#"UnitIsPlayer("target")"#,
            UnitState {
                exists: true,
                is_player: true,
                ..Default::default()
            },
        ),
        (
            "UnitPlayerControlled",
            r#"UnitPlayerControlled("target")"#,
            UnitState {
                exists: true,
                player_controlled: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsPlusMob",
            r#"UnitIsPlusMob("target")"#,
            UnitState {
                exists: true,
                flags: 0x40,
                ..Default::default()
            },
        ),
        (
            "UnitIsCharmed",
            r#"UnitIsCharmed("target")"#,
            UnitState {
                exists: true,
                charmed: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsTapped",
            r#"UnitIsTapped("target")"#,
            UnitState {
                exists: true,
                tapped: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsTappedByPlayer",
            r#"UnitIsTappedByPlayer("target")"#,
            UnitState {
                exists: true,
                tapped_by_player: true,
                ..Default::default()
            },
        ),
        (
            "UnitAffectingCombat",
            r#"UnitAffectingCombat("target")"#,
            UnitState {
                exists: true,
                in_combat: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsConnected",
            r#"UnitIsConnected("target")"#,
            UnitState {
                exists: true,
                is_connected: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsAFK",
            r#"UnitIsAFK("target")"#,
            UnitState {
                exists: true,
                is_afk: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsDND",
            r#"UnitIsDND("target")"#,
            UnitState {
                exists: true,
                is_dnd: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsPVP",
            r#"UnitIsPVP("target")"#,
            UnitState {
                exists: true,
                pvp: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsPVPFreeForAll",
            r#"UnitIsPVPFreeForAll("target")"#,
            UnitState {
                exists: true,
                is_pvp_ffa: true,
                ..Default::default()
            },
        ),
        (
            "UnitHasRelicSlot",
            r#"UnitHasRelicSlot("target")"#,
            UnitState {
                exists: true,
                has_relic_slot: true,
                ..Default::default()
            },
        ),
        (
            "UnitCanAttack",
            r#"UnitCanAttack("player", "target")"#,
            UnitState {
                exists: true,
                can_attack: true,
                ..Default::default()
            },
        ),
        (
            "UnitIsEnemy",
            r#"UnitIsEnemy("player", "target")"#,
            UnitState {
                exists: true,
                reaction: 1,
                ..Default::default()
            },
        ),
        (
            "UnitIsFriend",
            r#"UnitIsFriend("player", "target")"#,
            UnitState {
                exists: true,
                reaction: 5,
                ..Default::default()
            },
        ),
        (
            "UnitCanCooperate",
            r#"UnitCanCooperate("player", "target")"#,
            UnitState {
                exists: true,
                is_player: true,
                reaction: 5,
                ..Default::default()
            },
        ),
        (
            "UnitIsUnit",
            r#"UnitIsUnit("target", "target")"#,
            UnitState {
                exists: true,
                guid: 42,
                ..Default::default()
            },
        ),
    ];

    for (name, call, live) in table {
        // ---- the TRUE leg: the number 1, one value, and NOT a boolean ----
        let mut s = UiScript::new().unwrap();
        s.set_unit("player", Some(player()));
        s.set_unit("target", Some(live));

        assert_eq!(
            s.eval::<String>(&format!("return type({call})")).unwrap(),
            "number",
            "{name} pushes a boolean where the reference pushes tag 3 (lua_pushnumber)"
        );
        assert_eq!(
            s.eval::<i64>(&format!("return {call}")).unwrap(),
            1,
            "{name}'s true leg is the number 1"
        );
        assert!(
            s.eval::<bool>(&format!("return {call} == 1")).unwrap(),
            "{name}: the corpus idiom `== 1` must match on the true leg"
        );
        assert!(
            !s.eval::<bool>(&format!("return {call} == true")).unwrap(),
            "{name}: `== true` must NEVER match — the reference cannot push tag 1"
        );
        assert_eq!(
            s.arity(call).unwrap(),
            1,
            "{name} returns exactly one value on the true leg"
        );

        // ---- the FALSE leg: nil, one value, and NOT `false` ----
        // The token is cleared rather than re-flagged, so this also covers the "no such unit" arm
        // the reference shares with a false predicate.
        let mut s = UiScript::new().unwrap();
        s.set_unit("player", Some(player()));
        s.set_unit("target", None);

        assert!(
            s.eval::<bool>(&format!("return {call} == nil")).unwrap(),
            "{name}'s false leg is nil — a boolean `false` is not nil, and a caller comparing \
             against nil would read the opposite of the truth"
        );
        assert!(
            !s.eval::<bool>(&format!("return {call} == false")).unwrap(),
            "{name}: `== false` must never match"
        );
        assert_eq!(
            s.arity(call).unwrap(),
            1,
            "{name} returns exactly one value on the false leg too — nil, never zero values"
        );
    }
}

/// The corpus idiom, spelled the way an addon spells it, on a live token and a dead one.
///
/// This is the shape a `bool` breaks and truthiness hides: `if UnitExists("target") == 1` is
/// simply never true under a boolean, and `if UnitExists("target") == nil` is never true either —
/// the branch silently picks the wrong arm in both directions at once.
#[test]
fn the_equals_one_idiom_branches_correctly_on_a_live_and_a_dead_token() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));
    s.set_unit("target", Some(player()));

    assert_eq!(
        s.eval::<String>(
            r#"if UnitExists("target") == 1 then return "live" else return "dead" end"#
        )
        .unwrap(),
        "live"
    );

    s.set_unit("target", None);
    assert_eq!(
        s.eval::<String>(
            r#"if UnitExists("target") == 1 then return "live" else return "dead" end"#
        )
        .unwrap(),
        "dead"
    );
    assert_eq!(
        s.eval::<String>(
            r#"if UnitExists("target") == nil then return "gone" else return "here" end"#
        )
        .unwrap(),
        "gone",
        "the `== nil` half — the comparison a boolean INVERTS"
    );
}

/// `UnitOnTaxi` (`0x517a40`) is the family's one predicate registered outside `unit/`, over the
/// taxi module's own ride flag — which is how it stayed a `bool` when its 22 siblings were fixed.
/// Same shape, and the same `Usage:` raise its gate at `0x517a48` carries.
#[test]
fn unit_on_taxi_is_one_or_nil_and_raises_the_reference_usage() {
    let mut s = UiScript::new().unwrap();
    s.set_unit("player", Some(player()));

    assert!(s
        .eval::<bool>(r#"return UnitOnTaxi("player") == nil"#)
        .unwrap());
    s.set_on_taxi(true);
    assert_eq!(
        s.eval::<String>(r#"return type(UnitOnTaxi("player"))"#)
            .unwrap(),
        "number"
    );
    assert_eq!(s.eval::<i64>(r#"return UnitOnTaxi("player")"#).unwrap(), 1);
    assert!(!s
        .eval::<bool>(r#"return UnitOnTaxi("player") == true"#)
        .unwrap());
    // Any other token is nil, not false — only our own player is tracked.
    assert!(s
        .eval::<bool>(r#"return UnitOnTaxi("target") == nil"#)
        .unwrap());

    let err = s.eval::<mlua::Value>("return UnitOnTaxi()").unwrap_err();
    assert!(
        err.to_string().contains(r#"Usage: UnitOnTaxi("unit")"#),
        "the gate's own message, not mlua's type error: {err}"
    );
}

/// **The four `"player"` verbs read the seeded record, and nothing can blank it** (2261/2263).
///
/// The reference's `"player"` arm never reaches the resolver: `0x51708c` reads `0xc27d88`, a copy
/// of the char-enum row written once at the Enter World commit and never cleared anywhere in the
/// image. So the name survives everything that can happen to the *snapshot* — which is the whole
/// of decision 2260's bug: a name-cache miss for our own guid produced a nameless player push, it
/// replaced the roster seat, and the verb went nil in the world. That push is modelled here
/// exactly, and it must now be invisible to this verb.
#[test]
fn the_player_record_outlives_every_snapshot() {
    let mut s = UiScript::new().unwrap();

    // Before any Enter World commit — the buffer's one empty state, and the reference's nil.
    assert!(s
        .eval::<bool>(r#"return UnitName("player") == nil"#)
        .unwrap());

    // The world-entry load seeds it from the roster row, before a single addon file runs.
    s.set_player_record(PlayerRecord {
        name: "Nelprifour".into(),
        race: Some(("Night Elf".into(), "NightElf".into())),
        class: Some(("Priest".into(), "PRIEST".into())),
        sex: 2,
    });
    assert_eq!(
        s.eval::<String>(r#"return UnitName("player")"#).unwrap(),
        "Nelprifour",
        "the buffer answers with no player snapshot at all — the reference runs addon OnUpdate \
         for many frames with no local player object and this verb still answers"
    );

    // 2260's push, verbatim: the descriptor landed, the name cache missed for our own guid.
    let nameless = UnitState {
        exists: true,
        has_object: true,
        name: None,
        level: 5,
        ..Default::default()
    };
    s.set_unit("player", Some(nameless));
    assert_eq!(
        s.eval::<String>(r#"return UnitName("player")"#).unwrap(),
        "Nelprifour",
        "a nameless snapshot must not reach this verb — that push is the reported bug"
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitRace("player")"#).unwrap(),
        "Night Elf",
        "the same holds for the other three fields of the record (2263)"
    );
    assert_eq!(
        s.eval::<String>(r#"local _, t = UnitClass("player"); return t"#)
            .unwrap(),
        "PRIEST",
        "UnitClass's second return is the UPPERCASE token"
    );
    assert_eq!(s.eval::<i64>(r#"return UnitSex("player")"#).unwrap(), 2);
    // …and the snapshot is still the snapshot for everything else. `UnitLevel` is the boundary:
    // the record carries a level byte and the client's accessor for it (`0x5abe00`) is DEAD, so
    // this one reads the descriptor even for `"player"`.
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("player")"#).unwrap(), 5);

    // The token going away entirely (the logout despawn frames) does not empty the buffer either.
    s.set_unit("player", None);
    assert_eq!(
        s.eval::<String>(r#"return UnitName("player")"#).unwrap(),
        "Nelprifour"
    );
    assert!(
        s.eval::<bool>(r#"return UnitExists("player") == nil"#)
            .unwrap(),
        "the name and the unit are different questions — which is exactly what the buffer buys"
    );

    // An empty seed is refused, not stored: nothing in the reference ever empties this record.
    s.set_player_record(PlayerRecord::default());
    assert_eq!(
        s.eval::<String>(r#"return UnitName("player")"#).unwrap(),
        "Nelprifour"
    );
    assert_eq!(
        s.eval::<String>(r#"return UnitRace("player")"#).unwrap(),
        "Night Elf"
    );

    // A NAMED push does keep it in step — a second login as somebody else (1290's per-login VM
    // is the usual route, but a push must never leave the two disagreeing).
    let mut other = player();
    other.name = Some("Onewarrior".into());
    s.set_unit("player", Some(other));
    assert_eq!(
        s.eval::<String>(r#"return UnitName("player")"#).unwrap(),
        "Onewarrior"
    );
    assert_eq!(
        s.eval::<String>(r#"local _, t = UnitClass("player"); return t"#)
            .unwrap(),
        "WARRIOR",
        "a push that carries the field updates it; one that does not leaves it standing"
    );
    assert_eq!(
        s.eval::<i64>(r#"return UnitSex("player")"#).unwrap(),
        3,
        "player() is female — the record follows the push"
    );

    // Still the family's two returns, the second nil (1840).
    assert_eq!(
        s.eval::<i64>(r#"local t = {UnitName("player")}; return table.getn(t)"#)
            .unwrap(),
        1
    );
    // …and the case fold on the token holds on the fast path too.
    assert_eq!(
        s.eval::<String>(r#"return UnitName("PLAYER")"#).unwrap(),
        "Onewarrior"
    );
}

/// **The unset record's four answers, which are not uniform**.
///
/// The state is "no Enter World has been committed in this process" — unreachable from Lua in the
/// reference, because the four verbs only exist inside `UI_Init`'s table and only an Enter World
/// commit gets there. It is reachable *here* (a bare VM, a capture, a test world with no pick), so
/// it is pinned to what the bytes do rather than left to whatever falls out:
///
/// - `UnitName` → nil, via `lua_pushstring(NULL)` at `0x517095` tail-jumping into `lua_pushnil`;
/// - `UnitRace`/`UnitClass` → `nil, nil`, via the DBC bound and NULL-row arms — race 0 and class 0
///   have no row, and those arms reach `pushnil` **without** falling back to the unit resolver;
/// - `UnitSex` → the number **2**, because `0x517ef9`'s accessor feeds `fild [4*eax+0x808be4]`
///   over `{2,3,1,6}` with **no bounds check at all**. It is the one of the four with no validity
///   guard, and "male" is what an all-zero record means to it.
#[test]
fn an_unseeded_player_record_answers_the_references_four_ways() {
    let s = UiScript::new().unwrap();
    assert!(s
        .eval::<bool>(r#"return UnitName("player") == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return UnitRace("player") == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"local _, t = UnitRace("player"); return t == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"return UnitClass("player") == nil"#)
        .unwrap());
    assert!(s
        .eval::<bool>(r#"local _, t = UnitClass("player"); return t == nil"#)
        .unwrap());
    assert_eq!(
        s.eval::<i64>(r#"return UnitSex("player")"#).unwrap(),
        2,
        "the one of the four with no bounds check — an all-zero record reads male, not nil"
    );
    // And the fast path is the TOKEN, not a prefix: `"playerfoo"` is recognised but resolves to
    // nothing, so it goes to the resolver and answers the resolver's nil.
    assert!(s
        .eval::<bool>(r#"return UnitName("playerfoo") == nil"#)
        .unwrap());
    assert_eq!(
        s.eval::<i64>(r#"return UnitSex("playerfoo")"#).unwrap(),
        2,
        "…which for UnitSex is the same 2, by the binding's own unresolved-token default"
    );
}
