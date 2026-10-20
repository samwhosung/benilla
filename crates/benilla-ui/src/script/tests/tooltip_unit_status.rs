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
        [
            "[LEVEL 10]",
            "[PVP]",
            "[SKINNABLE]",
            "[CIVILIAN]",
            "[LEADER]"
        ]
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
        [
            "[LEVEL 10]",
            "PvP-aktiviert",
            "Kürschnerbar",
            "Zivilist",
            "Anführer"
        ]
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
    assert_eq!(
        lines(&s),
        ["[LEVEL_TYPE 20 [PLAYER]]", "[PVP]", "[RESURRECTABLE]"]
    );
    let (r, g, b): (f32, f32, f32) = s.eval("return STTextLeft4:GetTextColor()").unwrap();
    assert!(
        r == 0.0 && g > 0.99 && b == 0.0,
        "`0xc0d3ac`, the plain green: {r} {g} {b}"
    );

    // Feigning: `UnitHealth` reads 0, the field does not.
    let feigning = UnitState {
        tooltip: UnitTooltipFacts {
            health: 1200,
            ..Default::default()
        },
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
        s.set_unit(
            "target",
            Some(UnitState {
                level: 30,
                ..creature()
            }),
        );
        assert_eq!(lines(&s)[0], format!("[LEVEL {tooltip}]"), "drunk {drunk}");
        assert_eq!(
            s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(),
            unit_level,
            "UnitLevel at drunk {drunk}"
        );
    }
    // Never below 1.
    let mut s = viewer_script(30, 100, attacking());
    s.set_unit(
        "target",
        Some(UnitState {
            level: 3,
            ..creature()
        }),
    );
    assert_eq!(lines(&s)[0], "[LEVEL 1]");
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), 1);
    // A unit the player cannot attack keeps its level.
    let mut s = viewer_script(30, 100, Relations::default());
    s.set_unit(
        "target",
        Some(UnitState {
            level: 30,
            ..creature()
        }),
    );
    assert_eq!(lines(&s)[0], "[LEVEL 30]");
    assert_eq!(s.eval::<i64>(r#"return UnitLevel("target")"#).unwrap(), 30);
    assert!(s.take_errors().is_empty());
}

/// The owner line follows the subtitle, white (`0x52a24c`-`0x52a266`): the owner's name in the
/// template its kind keys (`0x854140`), `UNKNOWNOBJECT` for an owner whose name is not known.
#[test]
fn the_owner_line_names_the_owner_in_its_kinds_template() {
    let mut s = viewer_script(30, 0, Relations::default());
    s.run(
        r#"
        UNITNAME_TITLE_PET = "[PET %s]"; UNITNAME_TITLE_MINION = "[MINION %s]"
        UNITNAME_TITLE_GUARDIAN = "[GUARDIAN %s]"; UNITNAME_TITLE_CREATION = "[CREATION %s]"
        UNKNOWNOBJECT = "[UNKNOWN]"
    "#,
    )
    .unwrap();
    for (kind, name, line) in [
        (OwnerTitleKind::Pet, Some("Pfedh"), "[PET Pfedh]"),
        (OwnerTitleKind::Minion, Some("Pfedh"), "[MINION Pfedh]"),
        (OwnerTitleKind::Guardian, Some("Pfedh"), "[GUARDIAN Pfedh]"),
        (OwnerTitleKind::Creation, None, "[CREATION [UNKNOWN]]"),
    ] {
        s.set_unit(
            "target",
            Some(UnitState {
                subtitle: Some("Sub".into()),
                tooltip: UnitTooltipFacts {
                    owner: Some(OwnerTitle {
                        kind,
                        name: name.map(str::to_string),
                    }),
                    ..Default::default()
                },
                ..creature()
            }),
        );
        assert_eq!(lines(&s)[..3], ["Sub", line, "[LEVEL 20]"], "{kind:?}");
        let (r, g, b): (f32, f32, f32) = s.eval("return STTextLeft3:GetTextColor()").unwrap();
        assert!(r > 0.99 && g > 0.99 && b > 0.99, "white: {r} {g} {b}");
    }
    assert!(s.take_errors().is_empty());
}

/// `UNIT_SKINNABLE` is red (`0xc0d3a8`) unless the player has a skinning spell; then the skill
/// ramp `0x529fa0` of its value against the creature's requirement, 1 to level 10, `10 · level −
/// 100` below 20, else `5 · level` (`0x529f60`-`0x529f7f`).
#[test]
fn the_skinnable_line_takes_the_skill_colour_of_the_players_skinning() {
    let rgb = |s: &mut UiScript, level: u32, skinning: Option<i32>| -> (u8, u8, u8) {
        s.set_player_req_state(PlayerReqState {
            level: 30,
            skinning,
            ..Default::default()
        });
        s.set_unit(
            "target",
            Some(UnitState {
                level,
                skinnable: true,
                flags: 0x0400_0000,
                ..creature()
            }),
        );
        assert_eq!(lines(s)[1], "[SKINNABLE]");
        let (r, g, b): (f32, f32, f32) = s.eval("return STTextLeft3:GetTextColor()").unwrap();
        let q = |c: f32| (c * 255.0).round() as u8;
        (q(r), q(g), q(b))
    };
    const RED: (u8, u8, u8) = (255, 32, 32);
    const ORANGE: (u8, u8, u8) = (255, 128, 64);
    const YELLOW: (u8, u8, u8) = (255, 255, 0);
    const GREEN: (u8, u8, u8) = (64, 192, 64);
    const GRAY: (u8, u8, u8) = (128, 128, 128);
    let mut s = viewer_script(30, 0, Relations::default());
    assert_eq!(rgb(&mut s, 30, None), RED, "no skinning spell");
    // Level 30 requires 150.
    assert_eq!(rgb(&mut s, 30, Some(149)), RED);
    assert_eq!(rgb(&mut s, 30, Some(150)), ORANGE);
    assert_eq!(rgb(&mut s, 30, Some(175)), YELLOW);
    assert_eq!(rgb(&mut s, 30, Some(200)), GREEN);
    assert_eq!(rgb(&mut s, 30, Some(250)), GRAY);
    // Level 15 requires 50, level 10 and below 1, level 20 100.
    assert_eq!(rgb(&mut s, 15, Some(49)), RED);
    assert_eq!(rgb(&mut s, 15, Some(50)), ORANGE);
    assert_eq!(rgb(&mut s, 10, Some(1)), ORANGE);
    assert_eq!(rgb(&mut s, 10, Some(101)), GRAY);
    assert_eq!(rgb(&mut s, 20, Some(99)), RED);
    assert_eq!(rgb(&mut s, 20, Some(100)), ORANGE);
    assert!(s.take_errors().is_empty());
}

/// A group member's record, found by guid on its own party or raid row whatever token shows it
/// (`0x52a02b`), decides two lines (`0x52a6e2`-`0x52a72b`, `0x52a93e`-`0x52a995`): an offline one
/// ends the tooltip after the level line with `PLAYER_OFFLINE`, in sight or not; one online but
/// out of sight gets its area's name, white, in place of every status line; one in sight neither.
#[test]
fn a_group_members_record_reads_offline_or_its_zone_under_any_token() {
    let mut s = viewer_script(30, 0, Relations::default());
    s.run(r#"PLAYER_OFFLINE = "[OFFLINE]""#).unwrap();
    let shown = |guid: u64, has_object: bool| UnitState {
        guid,
        has_object,
        pvp: true,
        flags: 0x1000,
        ..player_unit()
    };
    let row = |guid: u64, online: bool| UnitState {
        tooltip: UnitTooltipFacts {
            party: Some(PartyRecord {
                online,
                area: Some("Elwynn Forest".into()),
            }),
            ..Default::default()
        },
        ..shown(guid, false)
    };
    let seat = |s: &mut UiScript, party: [u64; 4], raid: Vec<u64>| {
        s.set_unit_guids(&UnitGuids {
            player: ME,
            party,
            raid,
            ..Default::default()
        })
    };
    seat(&mut s, [UNIT, 0, 0, 0], vec![]);
    for (online, has_object, want) in [
        (false, false, "[OFFLINE]"),
        (false, true, "[OFFLINE]"),
        (true, false, "Elwynn Forest"),
        (true, true, "[PVP]"),
    ] {
        s.set_unit("party1", Some(row(UNIT, online)));
        s.set_unit("target", Some(shown(UNIT, has_object)));
        assert_eq!(
            lines(&s),
            ["[LEVEL_TYPE 20 [PLAYER]]", want],
            "online {online}, in sight {has_object}"
        );
    }
    // A raid member off our party slots reads its raid row's record (`0x4bb0f0`).
    s.set_unit("party1", None);
    seat(&mut s, [0; 4], vec![UNIT, ME]);
    s.set_unit("raid1", Some(row(UNIT, false)));
    assert_eq!(lines(&s), ["[LEVEL_TYPE 20 [PLAYER]]", "[OFFLINE]"]);
    // Off the roster no record is found, whatever the shown snapshot carries.
    seat(&mut s, [0; 4], vec![]);
    s.set_unit(
        "target",
        Some(UnitState {
            has_object: true,
            ..row(UNIT, false)
        }),
    );
    assert_eq!(lines(&s), ["[LEVEL_TYPE 20 [PLAYER]]", "[PVP]"]);
    // The active player's own record names no zone (`0x52a953`).
    seat(&mut s, [0; 4], vec![UNIT, ME]);
    s.set_unit("raid2", Some(row(ME, true)));
    s.set_unit("target", Some(shown(ME, false)));
    assert_eq!(lines(&s), ["[LEVEL_TYPE 20 [PLAYER]]"]);
    assert!(s.take_errors().is_empty());
}

/// The game object builder tints a tooltip that has a backdrop `0xff002e5a` (`0x52aa92`-
/// `0x52aaaa`); the corpse builder does not, and its one line is white (`0x52afa7`), where the
/// object's name is gold.
#[test]
fn a_world_objects_tooltip_tints_its_backdrop_and_a_corpses_does_not() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local up = CreateFrame("Frame", "UIParent"); up:SetAllPoints()
        local tt = CreateFrame("GameTooltip", "GameTooltip", nil, "TestTooltipTemplate")
        tt:SetBackdrop({ bgFile = "Interface\\Tooltips\\UI-Tooltip-Background" })
        tt:SetBackdropColor(0.09, 0.09, 0.19)
    "#,
    )
    .unwrap();
    let backdrop = |s: &UiScript| -> (u8, u8, u8) {
        let (r, g, b): (f32, f32, f32) = s.eval("return GameTooltip:GetBackdropColor()").unwrap();
        let q = |c: f32| (c * 255.0).round() as u8;
        (q(r), q(g), q(b))
    };
    let text_rgb = |s: &UiScript| -> (u8, u8, u8) {
        let (r, g, b): (f32, f32, f32) = s
            .eval("return GameTooltipTextLeft1:GetTextColor()")
            .unwrap();
        let q = |c: f32| (c * 255.0).round() as u8;
        (q(r), q(g), q(b))
    };
    s.publish_world_mouseover(Some(1));
    assert!(s.world_tooltip_corpse("Corpse of Bob"));
    assert_eq!(
        backdrop(&s),
        (23, 23, 48),
        "the corpse leaves the backdrop alone"
    );
    assert_eq!(text_rgb(&s), (255, 255, 255), "the corpse line is white");
    s.publish_world_mouseover(Some(2));
    assert!(s.world_tooltip_gameobject("Mailbox", &[], false));
    assert_eq!(backdrop(&s), (0, 46, 90), "0xff002e5a");
    assert_eq!(text_rgb(&s), (255, 210, 0), "the object's name is gold");
    assert!(s.take_errors().is_empty());
}

/// A lock line tinted by a known opener's margin takes the skill ramp `0x529fa0`'s colour.
#[test]
fn a_skill_tinted_lock_line_draws_the_ramps_colour() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local up = CreateFrame("Frame", "UIParent"); up:SetAllPoints()
        CreateFrame("GameTooltip", "GameTooltip", nil, "TestTooltipTemplate")
    "#,
    )
    .unwrap();
    let rgb = |s: &mut UiScript, value: i32| -> (u8, u8, u8) {
        let lines = [(
            "Requires Herbalism".to_string(),
            TooltipTint::Skill {
                required: 100,
                value,
            },
        )];
        s.publish_world_mouseover(Some(value as u64 + 1));
        assert!(s.world_tooltip_gameobject("Peacebloom", &lines, false));
        let (r, g, b): (f32, f32, f32) = s
            .eval("return GameTooltipTextLeft2:GetTextColor()")
            .unwrap();
        let q = |c: f32| (c * 255.0).round() as u8;
        (q(r), q(g), q(b))
    };
    assert_eq!(rgb(&mut s, 99), (255, 32, 32));
    assert_eq!(rgb(&mut s, 100), (255, 128, 64));
    assert_eq!(rgb(&mut s, 125), (255, 255, 0));
    assert_eq!(rgb(&mut s, 150), (64, 192, 64));
    assert_eq!(rgb(&mut s, 200), (128, 128, 128));
    assert!(s.take_errors().is_empty());
}

/// Marked stand-ins for the studied-creature block's keys; school 5's name is blank.
fn seed_studied(s: &mut UiScript) {
    s.run(
        r#"
        DAMAGE_TEMPLATE        = "[DMG %d-%d]"
        SINGLE_DAMAGE_TEMPLATE = "[DMG %d]"
        ARMOR_TEMPLATE         = "[ARMOR %d]"
        HP_TEMPLATE            = "[HP %d]"
        RESISTANCE_TEMPLATE    = "[RES %d %s]"
        RESISTANCE0_NAME       = "[R0]"
        RESISTANCE1_NAME       = "[R1]"
        RESISTANCE2_NAME       = "[R2]"
        RESISTANCE3_NAME       = "[R3]"
        RESISTANCE4_NAME       = "[R4]"
        RESISTANCE5_NAME       = ""
        RESISTANCE6_NAME       = "[R6]"
        PET_DIET_TEMPLATE      = "[DIET %s]"
        TAMEABLE               = "[TAMEABLE]"
        NOT_TAMEABLE           = "[NOT_TAMEABLE]"
        PET_SPELLS_TEMPLATE    = "[SPELLS %s]"
    "#,
    )
    .unwrap();
}

fn studied_target(s: &mut UiScript, dynamic_flags: u32, health: u32, studied: StudiedCreature) {
    s.set_unit(
        "target",
        Some(UnitState {
            dynamic_flags,
            // Skinnable, so the block's place before `UNIT_SKINNABLE` shows.
            flags: 0x0400_0000,
            skinnable: true,
            tooltip: UnitTooltipFacts {
                health,
                studied: Some(Box::new(studied)),
                ..Default::default()
            },
            ..creature()
        }),
    );
}

/// A studied beast (`UNIT_DYNAMIC_FLAGS` bit 4, `0x52a8ea`) reads `0x529a40`'s block between
/// `RESURRECTABLE` and `UNIT_SKINNABLE`: the damage with the minimum truncated and the maximum
/// rounded up (`0x529a55`-`0x529a7b`), the armor school's slot, the raw health, each other school
/// above 0 with a name (`0x529b83`, `0x529baa`), the diet and abilities joined by `", "`
/// (`0x84480c`), a rank in `" (%s)"` (`0x854820`).
#[test]
fn a_studied_creature_reads_the_beast_lore_block() {
    let mut s = viewer_script(30, 0, Relations::default());
    seed_studied(&mut s);
    let studied = StudiedCreature {
        min_damage: 10.7,
        max_damage: 15.2,
        resistances: [300, 0, 25, 0, -5, 40, 12],
        armor_school: Some(0),
        armor: 300,
        diet: vec!["Meat".into(), "Fish".into()],
        tameable: Some(vec![
            PetAbility {
                name: "Claw".into(),
                rank: "Rank 1".into(),
            },
            PetAbility {
                name: "Dash".into(),
                rank: String::new(),
            },
        ]),
    };
    studied_target(&mut s, 0x10, 1234, studied.clone());
    let l = lines(&s);
    let block = [
        "[DMG 10-16]",
        "[ARMOR 300]",
        "[HP 1234]",
        "[RES 25 [R2]]",
        "[RES 12 [R6]]",
        "[DIET Meat, Fish]",
        "[TAMEABLE]",
        "[SPELLS Claw (Rank 1), Dash]",
        "[SKINNABLE]",
    ];
    assert!(l.ends_with(&block.map(String::from)), "{l:?}");

    // The flag gates the block.
    studied_target(&mut s, 0, 1234, studied);
    let l = lines(&s);
    assert!(!l.iter().any(|t| t.starts_with("[DMG")), "{l:?}");
    assert!(s.take_errors().is_empty());
}

/// Equal ends take `SINGLE_DAMAGE_TEMPLATE` (`0x529a8a`), armor not above 0 reads 0
/// (`0x529aef`), with no school flagged none is skipped, an empty diet adds no line, and a
/// creature that is not tameable ends on `NOT_TAMEABLE` (`0x529eaa`).
#[test]
fn an_untameable_studied_creature_ends_on_not_tameable() {
    let mut s = viewer_script(30, 0, Relations::default());
    seed_studied(&mut s);
    studied_target(
        &mut s,
        0x10,
        80,
        StudiedCreature {
            min_damage: 12.0,
            max_damage: 11.5,
            resistances: [7, 0, 0, 0, 0, 0, 0],
            armor_school: None,
            armor: -3,
            diet: Vec::new(),
            tameable: None,
        },
    );
    let l = lines(&s);
    let block = [
        "[DMG 12]",
        "[ARMOR 0]",
        "[HP 80]",
        "[RES 7 [R0]]",
        "[NOT_TAMEABLE]",
        "[SKINNABLE]",
    ];
    assert!(l.ends_with(&block.map(String::from)), "{l:?}");
    assert!(s.take_errors().is_empty());
}
