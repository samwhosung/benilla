//! The item tooltip's vocabulary: the builder's key tables (`INVTYPE_*`, `ITEM_MOD_*`,
//! `SPELL_SCHOOL%d_CAP`), which [`super::render`] resolves against the player's own
//! `GlobalStrings.lua`, and its colours. Keys, not English: `INVTYPE_SHIELD` and
//! `INVTYPE_WEAPONOFFHAND` both read "Off Hand" in enUS and differ elsewhere. The race and class
//! names are `ChrRaces.dbc`'s and `ChrClasses.dbc`'s rows ([`ItemReqNames`]).

/// The reference's quality colour table, `0xc0d3c8`, read by `GetItemQualityColor` (`0x48dfb0`).
pub(super) const QUALITY_RGB: [[f32; 3]; 7] = [
    [0.616, 0.616, 0.616], // 0 Poor      9d9d9d
    [1.0, 1.0, 1.0],       // 1 Common    ffffff
    [0.118, 1.0, 0.0],     // 2 Uncommon  1eff00
    [0.0, 0.439, 0.867],   // 3 Rare      0070dd
    [0.639, 0.208, 0.933], // 4 Epic      a335ee
    [1.0, 0.502, 0.0],     // 5 Legendary ff8000
    [0.902, 0.8, 0.502],   // 6 Artifact  e6cc80
];

// The tooltip colours: white `0xc0cf60` ffffffff, red `0xc0d390` ffff2020, green `0xc0d3ac`
// ff00ff00, gold `0xc0d3e8` ffffd200, gray `0xc0d3c4` ff808080.
pub(super) const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub(super) const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
pub(super) const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];
/// The pure red `0xc0d398` (ffff0000), used by two enchant lines (`0x52ca29`, `0x52cc13`): a
/// negative enchant id in slot 0 or 1, and ITEM_ENCHANT_DISCLAIMER.
pub(super) const ENCHANT_RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
pub(super) const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];
pub(super) const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
/// The skill-difficulty ramp's rungs `0x529fa0` hands out, written by their initializers: gray
/// `0xc0cf50` ff808080 ([`GRAY`]), green `0xc0d420` ff40c040, yellow `0xc0cf18` ffffff00, orange
/// `0xc0d3a4` ffff8040 and red `0xc0d3a8` ffff2020 ([`RED`]). The green is also the colour of a
/// satisfiable `LOCKED` line.
pub(super) const LOCK_GREEN: [f32; 4] = [64.0 / 255.0, 192.0 / 255.0, 64.0 / 255.0, 1.0];
pub(super) const SKILL_YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
pub(super) const SKILL_ORANGE: [f32; 4] = [1.0, 128.0 / 255.0, 64.0 / 255.0, 1.0];

/// `0x529fa0(required, value)`: gray 100 above the requirement, green 50, yellow 25, orange from
/// the requirement itself, red below it.
pub(in crate::script) fn skill_color(required: i32, value: i32) -> [f32; 4] {
    if value >= required.wrapping_add(100) {
        GRAY
    } else if value >= required.wrapping_add(50) {
        LOCK_GREEN
    } else if value >= required.wrapping_add(25) {
        SKILL_YELLOW
    } else if value >= required {
        SKILL_ORANGE
    } else {
        RED
    }
}

/// An equipped set member's cream, `0xc0d368` (ffffff97, written at `0x529050`).
pub(super) const CREAM: [f32; 4] = [1.0, 1.0, 151.0 / 255.0, 1.0];

/// InventoryType to the slot line's key: the builder's 30-entry table `0x83ddb0`, indexed by
/// `[record+0x2c]` (`0x52c103`); entries 0 and 29 are the empty string `0x882748`, here `None`. A
/// bag (18) takes CONTAINER_SLOTS instead, and class 6 its class name (`0x52c0bc`); 24-27 ship no
/// `GlobalStrings.lua` value, so a thrown weapon (25) or a gun (26) has an empty slot cell.
pub(in crate::script) fn invtype_key(t: u32) -> Option<&'static str> {
    Some(match t {
        1 => "INVTYPE_HEAD",
        2 => "INVTYPE_NECK",
        3 => "INVTYPE_SHOULDER",
        4 => "INVTYPE_BODY",
        5 => "INVTYPE_CHEST",
        6 => "INVTYPE_WAIST",
        7 => "INVTYPE_LEGS",
        8 => "INVTYPE_FEET",
        9 => "INVTYPE_WRIST",
        10 => "INVTYPE_HAND",
        11 => "INVTYPE_FINGER",
        12 => "INVTYPE_TRINKET",
        13 => "INVTYPE_WEAPON",
        14 => "INVTYPE_SHIELD",
        15 => "INVTYPE_RANGED",
        16 => "INVTYPE_CLOAK",
        17 => "INVTYPE_2HWEAPON",
        19 => "INVTYPE_TABARD",
        20 => "INVTYPE_ROBE",
        21 => "INVTYPE_WEAPONMAINHAND",
        22 => "INVTYPE_WEAPONOFFHAND",
        23 => "INVTYPE_HOLDABLE",
        24 => "INVTYPE_AMMO",
        25 => "INVTYPE_THROWN",
        26 => "INVTYPE_RANGEDRIGHT",
        28 => "INVTYPE_RELIC",
        _ => return None,
    })
}

/// The damage bias `[0x808120]`, 0.9999899864196777: an integral max is not bumped, and unlike a
/// true `ceil()` a value within ~1e-5 above an integer is not rounded up.
const DAMAGE_BIAS: f32 = f32::from_bits(0x3f7f_ff58);

/// The builder's `floor(DamageMin)`: subtract the bias unless positive, then truncate toward zero
/// (`0x52c253..0x52c276`).
pub(in crate::script) fn floor_min(m: f32) -> i32 {
    (if m > 0.0 { m } else { m - DAMAGE_BIAS }) as i32
}

/// The builder's `ceil(DamageMax)`: add the bias if positive, then truncate (`0x52c26e..0x52c28c`).
pub(in crate::script) fn ceil_max(m: f32) -> i32 {
    (if m > 0.0 { m + DAMAGE_BIAS } else { m }) as i32
}

/// A school's name key, `SPELL_SCHOOL%d_CAP`, the one key the builder composes (`0x84e4cc`,
/// pushed at `0x52c2a8` and `0x52c8d1`). School 0 has none: a physical weapon takes the
/// school-less `DAMAGE_TEMPLATE` arm.
pub(in crate::script) fn school_key(s: u32) -> Option<String> {
    (1..=6).contains(&s).then(|| format!("SPELL_SCHOOL{s}_CAP"))
}

/// Stat type to its `ITEM_MOD_*` line template (`"%c%d Agility"`); the builder's 8-way jump table
/// `0x52e510` skips 2 (keys at `0x52c6eb..0x52c777`).
pub(in crate::script) fn stat_key(t: u32) -> Option<&'static str> {
    Some(match t {
        0 => "ITEM_MOD_MANA",
        1 => "ITEM_MOD_HEALTH",
        3 => "ITEM_MOD_AGILITY",
        4 => "ITEM_MOD_STRENGTH",
        5 => "ITEM_MOD_INTELLECT",
        6 => "ITEM_MOD_SPIRIT",
        7 => "ITEM_MOD_STAMINA",
        _ => return None,
    })
}

/// The rows an item's race and class lines walk, at the install's locale, each `(id, name)` in file
/// order: `ChrRaces` (`0xc0ded8`) without its hidden rows, and `ChrClasses` (`0xc0deec`).
#[derive(Clone, Debug, Default)]
pub(crate) struct ItemReqNames {
    pub(crate) races: Vec<(u32, String)>,
    pub(crate) classes: Vec<(u32, String)>,
}

impl crate::script::UiScript {
    /// Push the `ChrRaces` and `ChrClasses` rows an item's race and class lines walk, each
    /// `(id, name)` at the install's locale, in file order.
    pub fn set_item_req_names(&mut self, races: Vec<(u32, String)>, classes: Vec<(u32, String)>) {
        self.model_mut().item_req_names = ItemReqNames { races, classes };
    }
}

/// One race or class line of an item: its template's key, what fills its `%s`, and whether the
/// player meets it.
pub(in crate::script) struct ReqLine {
    pub(in crate::script) key: &'static str,
    pub(in crate::script) text: String,
    pub(in crate::script) ok: bool,
}

/// Whether `mask` holds a row's bit, `1 << (id - 1)` (`0x52ce68`-`0x52ce70`).
fn allows(mask: i32, id: u32) -> bool {
    mask & 1i32.wrapping_shl(id.wrapping_sub(1)) != 0
}

/// An item's race and class lines, the same in both builders (`0x52ce14`..`0x52d2c9`,
/// `0x4fe9e8`..`0x4fef07`). Masks of at most one bit each (`0x52ce24`, `0x52ce3c`) give one
/// `RACE_CLASS_ONLY` line: `"%s "` of the first race row in the mask, then the first class row's
/// name (`0x52ce89`, `0x52cf12`), met when the player is both, and no line with neither row.
/// Otherwise each table gives its own line, the races first ([`list_line`]).
pub(in crate::script) fn race_class_lines(
    names: &ItemReqNames,
    races: i32,
    classes: i32,
    race: u32,
    class: u32,
) -> Vec<ReqLine> {
    if races & races.wrapping_sub(1) != 0 || classes & classes.wrapping_sub(1) != 0 {
        return [
            list_line("ITEM_RACES_ALLOWED", &names.races, races, race),
            list_line("ITEM_CLASSES_ALLOWED", &names.classes, classes, class),
        ]
        .into_iter()
        .flatten()
        .collect();
    }
    let mut text = String::new();
    let mut ok = true;
    if let Some((id, name)) = names.races.iter().find(|(id, _)| allows(races, *id)) {
        text = format!("{name} ");
        ok &= race == *id;
    }
    if let Some((id, name)) = names.classes.iter().find(|(id, _)| allows(classes, *id)) {
        text.push_str(name);
        ok &= class == *id;
    }
    if text.is_empty() {
        return Vec::new();
    }
    vec![ReqLine {
        key: "RACE_CLASS_ONLY",
        text,
        ok,
    }]
}

/// One table's list line: none for a mask that holds every row (`0x52cfa0`-`0x52d045`) or names
/// none of them (`0x52d123`), else the names joined by `", "` (`0x84480c`), met when the player's
/// own row is among them (`0x52d0fc`).
fn list_line(key: &'static str, rows: &[(u32, String)], mask: i32, player: u32) -> Option<ReqLine> {
    let allowed: Vec<&(u32, String)> = rows.iter().filter(|(id, _)| allows(mask, *id)).collect();
    if allowed.len() == rows.len() {
        return None;
    }
    let text = allowed
        .iter()
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if text.is_empty() {
        return None;
    }
    let ok = allowed.iter().any(|(id, _)| *id == player);
    Some(ReqLine { key, text, ok })
}

/// The enUS rows, for the tests of what prints them.
#[cfg(test)]
pub(in crate::script) fn stock_req_names() -> ItemReqNames {
    fn owned(rows: &[(u32, &str)]) -> Vec<(u32, String)> {
        rows.iter().map(|&(id, n)| (id, n.to_string())).collect()
    }
    ItemReqNames {
        races: owned(&[
            (1, "Human"),
            (2, "Orc"),
            (3, "Dwarf"),
            (4, "Night Elf"),
            (5, "Undead"),
            (6, "Tauren"),
            (7, "Gnome"),
            (8, "Troll"),
        ]),
        classes: owned(&[
            (1, "Warrior"),
            (2, "Paladin"),
            (3, "Hunter"),
            (4, "Rogue"),
            (5, "Priest"),
            (7, "Shaman"),
            (8, "Mage"),
            (9, "Warlock"),
            (11, "Druid"),
        ]),
    }
}

pub(super) fn quality_color(q: u32) -> [f32; 4] {
    let c = QUALITY_RGB.get(q as usize).unwrap_or(&QUALITY_RGB[1]);
    [c[0], c[1], c[2], 1.0]
}

pub(super) fn req_color(ok: bool) -> [f32; 4] {
    if ok {
        WHITE
    } else {
        RED
    }
}
