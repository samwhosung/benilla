//! `GetTradeSkillItemStats` (`0x4fdec0`): the item a recipe creates, described as one Lua string per
//! line. It is a builder of its own, not the tooltip's (`0x52b650`): fixed order, the text of each
//! line filled straight from the global string, and a requirement is wrapped in the red escape
//! (`|cffff2020`, `0x84e4e0`; `|r`, `0x844538`) instead of coloured. The vocabulary tables are
//! the tooltip's ([`crate::script::tooltip_item::names`]).

use crate::script::item_stats::{ItemTemplateView, PlayerReqState};
use crate::script::tooltip_item::names::{
    ceil_max, floor_min, full_mask, invtype_key, school_key, stat_key, CLASS_NAMES, RACE_NAMES,
};
use crate::strings::{fill, Arg};

/// What the lines read of the local player beside [`PlayerReqState`].
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PlayerFacts {
    /// Whether the active player object exists (`0x4fe9b0`): without it the lines end after the
    /// spell effects.
    pub present: bool,
    /// The player's side digit of `PVP_RANK_<rank>_<team>` (`0x5efe00`).
    pub pvp_team: i8,
    /// The player's gender for the rank title's `_FEMALE` twin (`0x612bf0`).
    pub female: bool,
    /// Whether the player knows the required spell, or a higher rank of it (`0x60c740`,
    /// `0x60c8d0`); here the exact spell alone, as the tooltip's known test is.
    pub knows_required_spell: bool,
}

/// `|cffff2020` and `|r` around `text` unless `ok` (the `%s%s%s` of each requirement line).
fn red_unless(ok: bool, text: &str) -> String {
    if ok {
        text.to_string()
    } else {
        format!("|cffff2020{text}|r")
    }
}

/// The sign character of the stat and resistance lines: `+` above 0, `-` otherwise
/// (`setle`, then `0x2b + 2 * flag`).
fn sign(v: i32) -> String {
    (if v <= 0 { '-' } else { '+' }).to_string()
}

/// The lines for one created item, in the reference's order. `get` is the global-string lookup,
/// `""` for a missing key (`FrameScript_GetText`, `0x703bf0`).
pub(super) fn item_stat_lines(
    v: &ItemTemplateView,
    req: &PlayerReqState,
    me: PlayerFacts,
    get: &dyn Fn(&str) -> String,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    // Quality (`0x4fdfac`): only an item with an InventoryType; an empty string is no line.
    if v.inventory_type != 0 {
        let q = get(&format!("ITEM_QUALITY{}_DESC", v.quality));
        if !q.is_empty() {
            out.push(q);
        }
    }

    // Binding: the five-entry table `0x4ff380`; a value past it, or 0, is none.
    let bind = match v.bonding {
        1 => "ITEM_BIND_ON_PICKUP",
        2 => "ITEM_BIND_ON_EQUIP",
        3 => "ITEM_BIND_ON_USE",
        4 | 5 => "ITEM_BIND_QUEST",
        _ => "",
    };
    if !bind.is_empty() {
        let t = get(bind);
        if !t.is_empty() {
            out.push(t);
        }
    }

    // Uniqueness (`0x4fe0ba`), a container excepted; the line is pushed whatever the string holds.
    if v.max_count > 0 && v.class != 1 {
        out.push(if v.max_count == 1 {
            get("ITEM_UNIQUE")
        } else {
            fill(&get("ITEM_UNIQUE_MULTIPLE"), &[Arg::D(v.max_count.into())])
        });
    }
    if v.start_quest != 0 {
        out.push(get("ITEM_STARTS_QUEST"));
    }

    // The player's proficiency in the item's class (`0x5e1e70`, 0 for none): a miss reds the
    // subclass line, or for a weapon whose subclass has a prerequisite the class line, and the
    // subclass line too when the player lacks that one as well (`0x4fe23b`..`0x4fe288`).
    let mut slot_ok = true;
    let mut sub_ok = true;
    if let Some(&mask) = req.proficiency.get(&v.class).filter(|&&m| m != 0) {
        if mask & 1u32.wrapping_shl(v.subclass) == 0 {
            match v.proficiency_prereq {
                Some(alt) if v.class == 2 => {
                    slot_ok = false;
                    if mask & 1u32.wrapping_shl(alt) == 0 {
                        sub_ok = false;
                    }
                }
                _ => sub_ok = false,
            }
        }
    }
    let sub_name = v.sub_class_display.as_deref().filter(|n| !n.is_empty());
    if v.inventory_type == 18 {
        // A bag: the slot count and the subclass name, and no class or subclass line.
        if let Some(name) = sub_name {
            out.push(fill(
                &get("CONTAINER_SLOTS"),
                &[Arg::D(v.container_slots.into()), Arg::S(name)],
            ));
        }
    } else {
        // The class line: a projectile's `ItemClass.dbc` name, else the InventoryType's string.
        let class_line = if v.class == 6 {
            v.item_type.clone().filter(|n| !n.is_empty())
        } else {
            invtype_key(v.inventory_type)
                .map(get)
                .filter(|t| !t.is_empty())
        };
        if let Some(text) = class_line {
            out.push(red_unless(slot_ok, &text));
        }
        if let Some(name) = sub_name {
            out.push(red_unless(sub_ok, name));
        }
    }

    // Damage: the first slot alone (`0x4fe3d0`), when either bound is nonzero.
    if let Some(&(min, max, school)) = v.damages.first().filter(|d| d.0 != 0.0 || d.1 != 0.0) {
        let school_text = if school != 0 {
            format!(
                "{} ",
                school_key(school).map(|k| get(&k)).unwrap_or_default()
            )
        } else {
            String::new()
        };
        out.push(format!(
            "{} - {} {}{}",
            floor_min(min),
            ceil_max(max),
            school_text,
            get("DAMAGE")
        ));
        // A weapon's speed: the delay in hundredths of the millisecond count, truncated
        // (`0x51eb851f`, `sar 5`).
        if v.class == 2 {
            out.push(format!("{} {}", get("SPEED"), v.delay_ms / 100));
        }
    }

    if v.armor > 0 {
        out.push(format!("{} {}", v.armor, get("ARMOR")));
    }

    // Resistances: all six equal and nonzero is one line, else a line per nonzero school.
    let all_equal = v.resistances.iter().all(|&r| r == v.resistances[0]);
    if all_equal {
        if v.resistances[0] != 0 {
            let r = v.resistances[0];
            out.push(fill(
                &get("ITEM_RESIST_ALL"),
                &[Arg::S(&sign(r)), Arg::D(i64::from(r.unsigned_abs()))],
            ));
        }
    } else {
        for (i, &r) in v.resistances.iter().enumerate() {
            if r == 0 {
                continue;
            }
            let school = get(&format!("SPELL_SCHOOL{}_CAP", i + 1));
            out.push(fill(
                &get("ITEM_RESIST_SINGLE"),
                &[
                    Arg::S(&sign(r)),
                    Arg::D(i64::from(r.unsigned_abs())),
                    Arg::S(&school),
                ],
            ));
        }
    }

    // Stats, in wire order: a nonzero value of type 0, 1 or 3..=7 with a nonempty string.
    for &(kind, value) in v.stats.iter().take(10) {
        if value == 0 {
            continue;
        }
        let Some(key) = stat_key(kind) else {
            continue;
        };
        let template = get(key);
        if template.is_empty() {
            continue;
        }
        out.push(fill(
            &template,
            &[
                Arg::S(&sign(value)),
                Arg::D(i64::from(value.unsigned_abs())),
            ],
        ));
    }

    // Spell effects: the spell's name, with the charges appended whole (`0x4fe890`).
    for (name, charges) in v.spell_slots.iter().take(5) {
        let mut line = fill(&get("ITEM_SPELL_EFFECT"), &[Arg::S(name)]);
        match *charges {
            0 => {}
            -1 => line.push_str(&get("ITEM_SPELL_CHARGE_SINGLE")),
            n => line.push_str(&fill(
                &get("ITEM_SPELL_CHARGES"),
                &[Arg::D(i64::from(n.unsigned_abs()))],
            )),
        }
        out.push(line);
    }

    // Everything below needs the player (`0x4fe9b0`).
    if !me.present {
        return out;
    }

    // Race and class restrictions (`0x4fe9e8`). Each mask with at most one bit set is a single
    // race and class: one `RACE_CLASS_ONLY` line naming both, red unless the player is that race
    // and class. Otherwise a mask that leaves out a playable race, or any class, lists the ones it
    // allows, red unless the player is among them.
    let races = v.allowable_race;
    let classes = v.allowable_class;
    if races & races.wrapping_sub(1) == 0 && classes & classes.wrapping_sub(1) == 0 {
        let mut names = String::new();
        let mut ok = true;
        if let Some(&(id, name)) = RACE_NAMES
            .iter()
            .find(|&&(id, _)| races & (1i32.wrapping_shl(id - 1)) != 0)
        {
            names = format!("{name} ");
            ok &= req.race_id == id;
        }
        if let Some(&(id, name)) = CLASS_NAMES
            .iter()
            .find(|&&(id, _)| classes & (1i32.wrapping_shl(id - 1)) != 0)
        {
            names.push_str(name);
            ok &= req.class_id == id;
        }
        if !names.is_empty() {
            out.push(red_unless(
                ok,
                &fill(&get("RACE_CLASS_ONLY"), &[Arg::S(&names)]),
            ));
        }
    } else {
        let list = |ids: &[(u32, &str)], mask: i32, player: u32| -> Option<(String, bool)> {
            let allowed: Vec<(u32, &str)> = ids
                .iter()
                .copied()
                .filter(|&(id, _)| mask & (1i32.wrapping_shl(id - 1)) != 0)
                .collect();
            let text = allowed
                .iter()
                .map(|&(_, n)| n)
                .collect::<Vec<_>>()
                .join(", ");
            (!text.is_empty()).then(|| (text, allowed.iter().any(|&(id, _)| id == player)))
        };
        if races & full_mask(&RACE_NAMES) != full_mask(&RACE_NAMES) {
            if let Some((text, ok)) = list(&RACE_NAMES, races, req.race_id) {
                out.push(red_unless(
                    ok,
                    &fill(&get("ITEM_RACES_ALLOWED"), &[Arg::S(&text)]),
                ));
            }
        }
        if classes & full_mask(&CLASS_NAMES) != full_mask(&CLASS_NAMES) {
            if let Some((text, ok)) = list(&CLASS_NAMES, classes, req.class_id) {
                out.push(red_unless(
                    ok,
                    &fill(&get("ITEM_CLASSES_ALLOWED"), &[Arg::S(&text)]),
                ));
            }
        }
    }

    // Levels: the item level rides the required level's test (`0x4fef07`), and the minimum only
    // prints above 1.
    if v.required_level > 0 {
        out.push(fill(&get("ITEM_LEVEL"), &[Arg::D(v.item_level.into())]));
        if v.required_level > 1 {
            out.push(red_unless(
                req.level >= v.required_level,
                &fill(&get("ITEM_MIN_LEVEL"), &[Arg::D(v.required_level.into())]),
            ));
        }
    }

    // A required skill (`0x4ff013`). The colour test is the reference's: plain while the player's
    // total is BELOW the rank, red once it reaches it (`setg` at `0x4ff0e9`), the reverse of every
    // other line.
    if v.required_skill != 0 {
        let name = v.required_skill_name.as_deref().unwrap_or("UNKNOWN");
        let line = if v.required_skill_rank != 0 {
            fill(
                &get("ITEM_MIN_SKILL"),
                &[Arg::S(name), Arg::D(v.required_skill_rank.into())],
            )
        } else {
            fill(&get("ITEM_REQ_SKILL"), &[Arg::S(name)])
        };
        let total = req.skills.get(&v.required_skill).copied().unwrap_or(0);
        out.push(red_unless(v.required_skill_rank > total, &line));
    }

    // A required spell, when its row exists.
    if let Some(name) = v
        .required_spell_name
        .as_deref()
        .filter(|_| v.required_spell != 0)
    {
        out.push(red_unless(
            me.knows_required_spell,
            &fill(&get("ITEM_REQ_SKILL"), &[Arg::S(name)]),
        ));
    }

    // A required honor rank, named by the player's side and gender (`0x5efe60`), and a required
    // medal (`PVP_MEDAL<n>`), which the player's medal bits never hold.
    if v.required_honor_rank != 0 {
        let rank = v.required_honor_rank;
        let key = format!("PVP_RANK_{rank}_{}", me.pvp_team);
        let title = if me.female {
            Some(get(&format!("{key}_FEMALE"))).filter(|t| !t.is_empty())
        } else {
            None
        }
        .unwrap_or_else(|| get(&key));
        out.push(red_unless(
            u32::from(req.honor_rank) >= rank,
            &fill(&get("ITEM_REQ_SKILL"), &[Arg::S(&title)]),
        ));
    }
    if v.required_city_rank != 0 {
        let medal = get(&format!("PVP_MEDAL{}", v.required_city_rank));
        out.push(red_unless(
            false,
            &fill(&get("ITEM_REQ_SKILL"), &[Arg::S(&medal)]),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::script::UiScript;
    use crate::script::{TradeSkillDifficulty, TradeSkillRecipe, TradeSkillState};

    const RED: &str = "|cffff2020";

    /// The enUS values of the keys the lines read.
    fn strings() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("ITEM_QUALITY2_DESC", "Uncommon"),
            ("ITEM_BIND_ON_EQUIP", "Binds when equipped"),
            ("ITEM_BIND_ON_PICKUP", "Binds when picked up"),
            ("ITEM_UNIQUE", "Unique"),
            ("ITEM_UNIQUE_MULTIPLE", "Unique (%d)"),
            ("ITEM_STARTS_QUEST", "This Item Begins a Quest"),
            ("INVTYPE_WEAPON", "One-Hand"),
            ("INVTYPE_CHEST", "Chest"),
            ("DAMAGE", "Damage"),
            ("SPEED", "Speed"),
            ("ARMOR", "Armor"),
            ("ITEM_RESIST_ALL", "%c%d to All Resistances"),
            ("ITEM_RESIST_SINGLE", "%c%d %s Resistance"),
            ("SPELL_SCHOOL1_CAP", "Holy"),
            ("SPELL_SCHOOL2_CAP", "Fire"),
            ("SPELL_SCHOOL4_CAP", "Frost"),
            ("ITEM_MOD_STAMINA", "%c%d Stamina"),
            ("ITEM_MOD_MANA", "%c%d Mana"),
            ("ITEM_SPELL_EFFECT", "Effect: %s"),
            ("ITEM_SPELL_CHARGES", "%d Charge"),
            ("ITEM_SPELL_CHARGE_SINGLE", "One Charge"),
            ("CONTAINER_SLOTS", "%d Slot %s"),
            ("RACE_CLASS_ONLY", "%s Only."),
            ("ITEM_RACES_ALLOWED", "Races: %s"),
            ("ITEM_CLASSES_ALLOWED", "Classes: %s"),
            ("ITEM_LEVEL", "Level %d"),
            ("ITEM_MIN_LEVEL", "Requires Level %d"),
            ("ITEM_MIN_SKILL", "Requires %s (%d)"),
            ("ITEM_REQ_SKILL", "Requires %s"),
            ("PVP_RANK_5_1", "Private"),
            ("PVP_RANK_5_1_FEMALE", "Private (f)"),
            ("PVP_MEDAL1", "Protector of Stormwind"),
        ])
    }

    fn lines(v: &ItemTemplateView, req: &PlayerReqState, me: PlayerFacts) -> Vec<String> {
        let table = strings();
        item_stat_lines(v, req, me, &|k| {
            table.get(k).copied().unwrap_or("").to_string()
        })
    }

    fn human_warrior() -> (PlayerReqState, PlayerFacts) {
        (
            PlayerReqState {
                level: 20,
                class_id: 1,
                race_id: 1,
                ..Default::default()
            },
            PlayerFacts {
                present: true,
                pvp_team: 1,
                ..Default::default()
            },
        )
    }

    /// `s` in the red escape, leaked for the test's lifetime so it sits in a `&str` array.
    fn red(s: &str) -> &'static str {
        Box::leak(format!("{RED}{s}|r").into_boxed_str())
    }

    /// A one-hand fire sword, line by line, in `0x4fdec0`'s order: quality, binding, uniqueness,
    /// quest start, the class and subclass lines, damage then speed, stats, the spell with its
    /// charges appended whole.
    #[test]
    fn an_item_without_a_player_ends_after_its_spell_lines() {
        let v = ItemTemplateView {
            quality: 2,
            class: 2,
            subclass: 7,
            inventory_type: 13,
            bonding: 2,
            max_count: 1,
            start_quest: 9,
            sub_class_display: Some("Sword".into()),
            damages: vec![(3.2, 5.7, 2)],
            delay_ms: 2650,
            stats: vec![(7, 5), (0, -3), (2, 4), (4, 0)],
            spell_slots: vec![("Fireball".into(), 3), ("Frost Nova".into(), -1)],
            // Everything below needs the player.
            required_level: 10,
            allowable_race: 1,
            allowable_class: 1,
            ..Default::default()
        };
        let (req, _) = human_warrior();
        let got = lines(&v, &req, PlayerFacts::default());
        assert_eq!(
            got,
            [
                "Uncommon",
                "Binds when equipped",
                "Unique",
                "This Item Begins a Quest",
                "One-Hand",
                "Sword",
                // `floor(3.2) - ceil(5.7)` with the school and its trailing space.
                "3 - 6 Fire Damage",
                // 2650 / 100, truncated.
                "Speed 26",
                "+5 Stamina",
                "-3 Mana",
                "Effect: Fireball3 Charge",
                "Effect: Frost NovaOne Charge",
            ]
        );
    }

    /// The lines that need the player: the single race and class, the levels, the skill (with the
    /// reference's reversed colour test), a spell, an honor rank and a medal.
    #[test]
    fn the_requirement_lines_follow_the_players_state() {
        let v = ItemTemplateView {
            required_level: 30,
            item_level: 25,
            allowable_race: 1,
            allowable_class: 1,
            required_skill: 164,
            required_skill_rank: 50,
            required_skill_name: Some("Blacksmithing".into()),
            required_spell: 777,
            required_spell_name: Some("Smelting".into()),
            required_honor_rank: 5,
            required_city_rank: 1,
            ..Default::default()
        };
        let (mut req, me) = human_warrior();
        req.skills.insert(164, 40);
        req.honor_rank = 3;
        let got = lines(&v, &req, me);
        assert_eq!(
            got,
            [
                "Human Warrior Only.",
                "Level 25",
                // 20 < 30: red.
                red("Requires Level 30"),
                // The reversed test: rank 50 above the player's 40 is plain.
                "Requires Blacksmithing (50)",
                // The spell is unknown, the honor rank unmet, the medal never held.
                red("Requires Smelting"),
                red("Requires Private"),
                red("Requires Protector of Stormwind"),
            ]
        );
        // Everything met: the level and the spell plain, the skill red at the rank, the title
        // the female twin's.
        req.level = 30;
        req.skills.insert(164, 50);
        req.honor_rank = 5;
        let me = PlayerFacts {
            knows_required_spell: true,
            female: true,
            ..me
        };
        let got = lines(&v, &req, me);
        assert_eq!(
            got,
            [
                "Human Warrior Only.",
                "Level 25",
                "Requires Level 30",
                red("Requires Blacksmithing (50)"),
                "Requires Smelting",
                "Requires Private (f)",
                red("Requires Protector of Stormwind"),
            ]
        );
        // No skill row: UNKNOWN under the plain key; with no rank (0) the test `0 > total` never
        // holds, so the line is red.
        let v = ItemTemplateView {
            required_skill: 999,
            ..Default::default()
        };
        assert_eq!(lines(&v, &req, me), [red("Requires UNKNOWN")]);
    }

    /// A single race or class is wrong for a different player, and an unrestricted mask is no
    /// line; two or more set bits list what is allowed, only when a playable race or any class is
    /// left out.
    #[test]
    fn race_and_class_masks_make_the_single_or_the_list_line() {
        let (req, me) = human_warrior();
        let with = |races: i32, classes: i32| {
            lines(
                &ItemTemplateView {
                    allowable_race: races,
                    allowable_class: classes,
                    ..Default::default()
                },
                &req,
                me,
            )
        };
        // Orc and Warrior: the player is a human warrior.
        assert_eq!(with(2, 1), [red("Orc Warrior Only.")]);
        // Race alone (class mask 0): the trailing space stays.
        assert_eq!(with(1, 0), ["Human  Only."]);
        // Neither bit names a row: no line.
        assert!(with(0, 0).is_empty());
        // Everything allowed (-1): none.
        assert!(with(-1, -1).is_empty());
        // Races 1 and 2, any class: the race list, plain for a human.
        assert_eq!(with(3, -1), ["Races: Human, Orc"]);
        // The same list is red for an undead.
        let undead = PlayerReqState {
            race_id: 5,
            ..req.clone()
        };
        let got = lines(
            &ItemTemplateView {
                allowable_race: 3,
                allowable_class: -1,
                ..Default::default()
            },
            &undead,
            me,
        );
        assert_eq!(got, [red("Races: Human, Orc")]);
        // Classes 1 and 8 (warrior, mage), any race.
        assert_eq!(with(-1, 0b1000_0001), ["Classes: Warrior, Mage"]);
        // Both lists at once, in that order.
        assert_eq!(
            with(3, 0b1000_0001),
            ["Races: Human, Orc", "Classes: Warrior, Mage"]
        );
    }

    /// A weapon whose subclass has a prerequisite reds the class line, and the subclass line too
    /// when the player lacks the prerequisite as well; without one only the subclass line reds.
    #[test]
    fn a_missing_proficiency_reds_the_class_and_subclass_lines_as_the_reference_does() {
        let v = |prereq| ItemTemplateView {
            class: 2,
            subclass: 7,
            inventory_type: 13,
            sub_class_display: Some("Sword".into()),
            proficiency_prereq: prereq,
            ..Default::default()
        };
        let (mut req, me) = human_warrior();
        // No mask: no red.
        assert_eq!(lines(&v(None), &req, me), ["One-Hand", "Sword"]);
        // Axes only (subclass 0).
        req.proficiency.insert(2, 1 << 0);
        assert_eq!(lines(&v(None), &req, me), ["One-Hand", red("Sword")]);
        // The prerequisite is a subclass the player has: the class line alone.
        assert_eq!(lines(&v(Some(0)), &req, me), [red("One-Hand"), "Sword"]);
        // The prerequisite is one the player lacks too: both.
        assert_eq!(
            lines(&v(Some(4)), &req, me),
            [red("One-Hand"), red("Sword")]
        );
        // Not a weapon: the prerequisite is not consulted.
        let armor = ItemTemplateView {
            class: 4,
            subclass: 2,
            inventory_type: 5,
            proficiency_prereq: Some(0),
            sub_class_display: Some("Leather".into()),
            ..Default::default()
        };
        req.proficiency.insert(4, 1 << 1);
        assert_eq!(lines(&armor, &req, me), ["Chest", red("Leather")]);
    }

    /// Resistances: six equal and nonzero is one line, otherwise one per nonzero school; a bag has
    /// one slot line and no class lines; armor is `%d %s`.
    #[test]
    fn resistances_armor_and_bags() {
        let (req, me) = human_warrior();
        let all = ItemTemplateView {
            resistances: [-4; 6],
            armor: 12,
            ..Default::default()
        };
        assert_eq!(lines(&all, &req, me), ["12 Armor", "-4 to All Resistances"]);
        let some = ItemTemplateView {
            resistances: [3, 0, 0, -2, 0, 0],
            ..Default::default()
        };
        assert_eq!(
            lines(&some, &req, me),
            ["+3 Holy Resistance", "-2 Frost Resistance"]
        );
        let bag = ItemTemplateView {
            inventory_type: 18,
            container_slots: 6,
            sub_class_display: Some("Bag".into()),
            class: 1,
            max_count: 2,
            ..Default::default()
        };
        // A container is never "Unique", and a bag has no class or subclass line.
        assert_eq!(lines(&bag, &req, me), ["6 Slot Bag"]);
        let nameless = ItemTemplateView {
            sub_class_display: None,
            ..bag
        };
        assert!(lines(&nameless, &req, me).is_empty());
        // Uniqueness with a count, and a projectile's class name.
        let arrow = ItemTemplateView {
            class: 6,
            inventory_type: 24,
            item_type: Some("Projectile".into()),
            max_count: 3,
            ..Default::default()
        };
        assert_eq!(lines(&arrow, &req, me), ["Unique (3)", "Projectile"]);
    }

    fn recipe(spell_id: u32, product: u32) -> TradeSkillRecipe {
        TradeSkillRecipe {
            group: Some((4, 2, "Leather".into())),
            spell_id,
            name: format!("Recipe {spell_id}"),
            difficulty: TradeSkillDifficulty::Optimal,
            num_available: 1,
            icon: None,
            min_made: 1,
            max_made: 1,
            cooldown: None,
            product_item: product,
            product_inv_type: 5,
            product_item_level: 0,
            reagents: Vec::new(),
            tools: Vec::new(),
        }
    }

    /// The binding: the usage text, zero values on a header, a missing row or a recipe with no
    /// product, an uncached template asked for, and one string per line otherwise.
    #[test]
    fn the_binding_answers_zero_values_or_one_string_per_line() {
        let mut s = UiScript::new().unwrap();
        let table = strings();
        s.run(
            &table
                .iter()
                .map(|(k, v)| format!("{k} = {v:?}\n"))
                .collect::<String>(),
        )
        .unwrap();
        for call in [
            "GetTradeSkillItemStats()",
            "GetTradeSkillItemStats(nil)",
            "GetTradeSkillItemStats('x')",
            "GetTradeSkillItemStats({})",
        ] {
            let err = s.run(call).expect_err(call).to_string();
            assert!(
                err.contains("Usage: GetTradeSkillItemStats(index)"),
                "{call}: {err}"
            );
        }
        // No window: zero values.
        assert_eq!(s.arity("GetTradeSkillItemStats(1)").unwrap(), 0);
        s.set_trade_skill(Some(TradeSkillState {
            line: 165,
            line_name: "Leatherworking".into(),
            rank: 10,
            max_rank: 75,
            recipes: vec![recipe(100, 5000), recipe(101, 0)],
        }));
        // Rows: 1 the "Leather" header, 2 and 3 the recipes in name order.
        for i in [0, 1, 4, 99, -1] {
            assert_eq!(
                s.arity(&format!("GetTradeSkillItemStats({i})")).unwrap(),
                0,
                "row {i}"
            );
        }
        // A recipe with no product is no item, and asks nothing.
        assert_eq!(s.arity("GetTradeSkillItemStats(3)").unwrap(), 0);
        assert!(s.take_item_stat_asks().is_empty());
        // An uncached template: zero values, and the template is asked for.
        assert_eq!(s.arity("GetTradeSkillItemStats(2)").unwrap(), 0);
        assert_eq!(s.take_item_stat_asks(), [5000]);
        s.set_item_template(
            5000,
            ItemTemplateView {
                quality: 2,
                bonding: 2,
                class: 4,
                subclass: 2,
                inventory_type: 5,
                armor: 30,
                sub_class_display: Some("Leather".into()),
                allowable_race: -1,
                allowable_class: -1,
                ..Default::default()
            },
        );
        assert_eq!(s.arity("GetTradeSkillItemStats(2)").unwrap(), 5);
        let (a, b, c, d): (String, String, String, String) =
            s.eval("return GetTradeSkillItemStats(2)").unwrap();
        assert_eq!(
            (a.as_str(), b.as_str(), c.as_str(), d.as_str()),
            ("Uncommon", "Binds when equipped", "Chest", "Leather")
        );
        let armor: String = s
            .eval("local a, b, c, d, e = GetTradeSkillItemStats(2) return e")
            .unwrap();
        assert_eq!(armor, "30 Armor");
    }
}
