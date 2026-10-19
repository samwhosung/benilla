//! The unit tooltip's status lines after the level line (`0x52a840`-`0x52a9f3`) and the level
//! text's drunk reduction (`0x52a384`-`0x52a3f4`): which install string each line reads, and the
//! gates that read the active player.

use std::rc::Rc;

use super::common::script;
use crate::script::*;

const ME: u64 = 0x10;
const UNIT: u64 = 0x0000_0000_0000_0020;

/// The host's relation functions over a fixed set of verdicts, the player toward [`UNIT`].
#[derive(Default)]
struct Relations {
    attack: bool,
    assist: bool,
}

impl UnitRelations for Relations {
    fn holds(&self, guid: u64) -> bool {
        guid == ME || guid == UNIT
    }
    fn reaction(&self, _: u64, _: u64) -> Option<u8> {
        Some(3)
    }
    fn can_attack(&self, a: u64, b: u64) -> Option<bool> {
        Some((a, b) == (ME, UNIT) && self.attack)
    }
    fn can_assist(&self, a: u64, b: u64) -> Option<bool> {
        Some((a, b) == (ME, UNIT) && self.assist)
    }
    fn can_cooperate(&self, _: u64, _: u64) -> Option<bool> {
        Some(false)
    }
    fn is_pvp(&self, _: u64) -> Option<bool> {
        Some(false)
    }
}

/// Marked stand-ins for every key the status lines and the level line read, so a line can only
/// match by reading its own key.
fn seed(s: &mut UiScript) {
    s.run(
        r#"
        TOOLTIP_UNIT_LEVEL            = "[LEVEL %s]"
        TOOLTIP_UNIT_LEVEL_CLASS      = "[LEVEL_CLASS %s %s]"
        TOOLTIP_UNIT_LEVEL_TYPE       = "[LEVEL_TYPE %s %s]"
        TOOLTIP_UNIT_LEVEL_CLASS_TYPE = "[LEVEL_CLASS_TYPE %s %s %s]"
        PLAYER = "[PLAYER]"
        PVP_ENABLED       = "[PVP]"
        RESURRECTABLE     = "[RESURRECTABLE]"
        UNIT_SKINNABLE    = "[SKINNABLE]"
        PVP_RANK_CIVILIAN = "[CIVILIAN]"
        PVP_RANK_LEADER   = "[LEADER]"
        local a = CreateFrame("Button", "StatusHost"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        CreateFrame("GameTooltip", "ST", nil, "TestTooltipTemplate")
    "#,
    )
    .unwrap();
}

/// A script with the active player seated at level `level`, drunk by `drunk`, and the relation
/// functions over `rel`.
fn viewer_script(level: u32, drunk: u8, rel: Relations) -> UiScript {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    seed(&mut s);
    s.set_player_req_state(PlayerReqState {
        level,
        ..Default::default()
    });
    s.set_unit(
        "player",
        Some(UnitState {
            exists: true,
            has_object: true,
            is_player: true,
            guid: ME,
            level,
            drunk,
            ..Default::default()
        }),
    );
    s.seat_player_guid(ME);
    s.set_unit_relations(Some(Rc::new(rel)));
    s
}

/// The tooltip's lines from 2 on, after a `SetUnit("target")`.
fn lines(s: &UiScript) -> Vec<String> {
    s.run(r#"ST:SetOwner(StatusHost, "ANCHOR_RIGHT"); assert(ST:SetUnit("target") == 1)"#)
        .unwrap();
    let mut out = Vec::new();
    for i in 2..=30 {
        let text: Option<String> = s
            .eval(&format!("return STTextLeft{i} and STTextLeft{i}:GetText()"))
            .unwrap();
        match text {
            Some(t) => out.push(t),
            None => break,
        }
    }
    out
}

fn creature() -> UnitState {
    UnitState {
        exists: true,
        has_object: true,
        guid: UNIT,
        name: Some("Raptor".into()),
        level: 20,
        reaction: 3,
        player_reaction: 3,
        ..Default::default()
    }
}

fn player_unit() -> UnitState {
    UnitState {
        is_player: true,
        name: Some("Pfedh".into()),
        ..creature()
    }
}

/// Every status line is the install's string under its key (`0x703bf0` on `PVP_ENABLED`
/// `0x8548f8`, `UNIT_SKINNABLE` `0x8548b4`, `PVP_RANK_CIVILIAN` `0x8548d4`, `PVP_RANK_LEADER`
/// `0x8548c4`), so a rewrite of the global, or a German install, reaches the line.
#[test]
fn the_status_lines_read_the_install_strings() {
    let mut s = viewer_script(30, 0, Relations::default());
    s.set_unit(
        "target",
        Some(UnitState {
            flags: 0x1000 | 0x0400_0000,
            pvp: true,
            skinnable: true,
            civilian: true,
            racial_leader: true,
            reaction: 2,
            level: 10,
            ..creature()
        }),
    );
    assert_eq!(
        lines(&s),
        ["[LEVEL 10]", "[PVP]", "[SKINNABLE]", "[CIVILIAN]", "[LEADER]"]
    );
    s.run(
        r#"
        PVP_ENABLED = "PvP-aktiviert"; UNIT_SKINNABLE = "Kürschnerbar"
        PVP_RANK_CIVILIAN = "Zivilist"; PVP_RANK_LEADER = "Anführer"
    "#,
    )
    .unwrap();
    assert_eq!(
        lines(&s),
        ["[LEVEL 10]", "PvP-aktiviert", "Kürschnerbar", "Zivilist", "Anführer"]
    );
    assert!(s.take_errors().is_empty());
}

/// The PvP line shows on either of two bits (`0x52a84b`-`0x52a878`): a player's contested flag,
/// `PLAYER_FLAGS` bit 8, or the PvP unit flag, bit 12, for any unit; the contested bit counts on a
/// player alone. A party member out of sight has no object, and no status line at all.
#[test]
fn the_pvp_line_reads_the_contested_flag_or_the_unit_flag() {
    let mut s = viewer_script(30, 0, Relations::default());
    let pvp = |s: &mut UiScript, u: UnitState| {
        s.set_unit("target", Some(u));
        lines(s).iter().any(|l| l == "[PVP]")
    };
    let contested = UnitState {
        player_flags: 0x100,
        ..player_unit()
    };
    assert!(pvp(&mut s, contested.clone()), "a contested player");
    let flagged = UnitState {
        flags: 0x1000,
        ..player_unit()
    };
    assert!(pvp(&mut s, flagged), "the unit flag on a player");
    let creature_flagged = UnitState {
        flags: 0x1000,
        ..creature()
    };
    assert!(pvp(&mut s, creature_flagged), "the unit flag on a creature");
    let creature_contested = UnitState {
        player_flags: 0x100,
        ..creature()
    };
    assert!(
        !pvp(&mut s, creature_contested),
        "the contested bit is read off a player only (`0x52a851`)"
    );
    // A party member out of sight carries the roster's PvP bit, which no status line reads.
    let absent = UnitState {
        has_object: false,
        pvp: true,
        flags: 0x1000,
        ..contested
    };
    assert!(!pvp(&mut s, absent), "no object, no PvP line (`0x52a840`)");
    assert!(s.take_errors().is_empty());
}

/// `RESURRECTABLE`, green, follows the PvP line for a player whose raw health is 0 or less
/// (`0x52a8a4`, `0x52a8ac`) when the active player may assist it (`0x6066f0` at `0x52a8bc`). A
/// feigning hunter's raw health stands, so its zero `UnitHealth` does not count.
#[test]
fn a_dead_player_the_player_may_assist_reads_resurrectable() {
    let dead = UnitState {
        dead: true,
        flags: 0x1000,
        ..player_unit()
    };
    let assisting = Relations {
        assist: true,
        ..Default::default()
    };
    let mut s = viewer_script(30, 0, assisting);
    s.set_unit("target", Some(dead.clone()));
    assert_eq!(lines(&s), ["[LEVEL_TYPE 20 [PLAYER]]", "[PVP]", "[RESURRECTABLE]"]);
    let (r, g, b): (f32, f32, f32) = s.eval("return STTextLeft4:GetTextColor()").unwrap();
    assert!(
        r == 0.0 && g > 0.99 && b == 0.0,
        "`0xc0d3ac`, the plain green: {r} {g} {b}"
    );

    // Feigning: `UnitHealth` reads 0, the field does not.
    let feigning = UnitState {
        tooltip: UnitTooltipFacts { health: 1200 },
        ..dead.clone()
    };
    s.set_unit("target", Some(feigning));
    assert!(!lines(&s).iter().any(|l| l == "[RESURRECTABLE]"));

    // A creature corpse, and a player the player may not assist, read nothing.
    s.set_unit(
        "target",
        Some(UnitState {
            dead: true,
            ..creature()
        }),
    );
    assert!(!lines(&s).iter().any(|l| l == "[RESURRECTABLE]"));
    let mut s = viewer_script(30, 0, Relations::default());
    s.set_unit("target", Some(dead));
    assert!(!lines(&s).iter().any(|l| l == "[RESURRECTABLE]"));
    assert!(s.take_errors().is_empty());
}

/// A drunk player sees a lower level on a unit it can attack: the tooltip takes
/// `trunc(min(drunk, 100) · 0.05 + 0.5)` off it, to no less than 1 (`0x52a3c6`-`0x52a3f4`), and
/// `UnitLevel` the same by its own product, `drunk · 5.0 · 0.01` (`0x5180c8`-`0x5180ec`), which at
/// 53 bits reads one lower at a drunkenness of 10, 30, 50, 70 or 90.
#[test]
fn a_drunk_player_sees_a_lower_level_on_a_unit_it_can_attack() {
    let attacking = || Relations {
        attack: true,
        ..Default::default()
    };
    for (drunk, tooltip, unit_level) in [
        (0u8, 30, 30),
        (9, 30, 30),
        (10, 29, 30),
        (11, 29, 29),
        (50, 27, 28),
        (100, 25, 25),
        (255, 25, 25),
    ] {
        let mut s = viewer_script(30, drunk, attacking());
        s.set_unit("target", Some(UnitState { level: 30, ..creature() }));
        assert_eq!(lines(&s)[0], format!("[LEVEL {tooltip}]"), "drunk {drunk}");
        assert_eq!(
            s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(),
            unit_level,
            "UnitLevel at drunk {drunk}"
        );
    }
    // Never below 1.
    let mut s = viewer_script(30, 100, attacking());
    s.set_unit("target", Some(UnitState { level: 3, ..creature() }));
    assert_eq!(lines(&s)[0], "[LEVEL 1]");
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), 1);
    // A unit the player cannot attack keeps its level.
    let mut s = viewer_script(30, 100, Relations::default());
    s.set_unit("target", Some(UnitState { level: 30, ..creature() }));
    assert_eq!(lines(&s)[0], "[LEVEL 30]");
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), 30);
    assert!(s.take_errors().is_empty());
}
