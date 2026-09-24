//! The item tooltip's display vocabulary — the builder's own **key** tables (`INVTYPE_*`,
//! `ITEM_MOD_*`, `SPELL_SCHOOL%d_CAP`), the DBC-sourced subclass/class/race names, and the
//! byte-verified color constants the render law paints with. Pure data; the law itself is
//! [`super::render`], which resolves every key here against the player's own `GlobalStrings.lua`.
//!
//! **Keys, not sentences, and the difference is not cosmetic.** `INVTYPE_SHIELD` and
//! `INVTYPE_WEAPONOFFHAND` both read "Off Hand" in enUS and are separately localizable
//! everywhere else; a table that stored the English could not tell them apart, and this one had
//! them collapsed into a single arm until 2045's sweep.
//!
//! **What stays a Rust literal here, and why it is not the same thing**: [`CLASS_NAMES`] and
//! [`RACE_NAMES`] name rows the reference reads out of DBC records (ChrClasses, ChrRaces), never
//! out of `GlobalStrings.lua`. Those belong to the DBC-feed question, not to this one — and the
//! subclass names that used to sit beside them have gone the whole way there: the type cell and
//! the bag line both read `ItemSubClass.dbc`'s own DisplayName off the app-resolved view now,
//! rather than a hand-typed copy of it.

/// The client's 7-entry quality→color table (`0xc0d3c8` behind
/// `GetItemQualityColor 0x48dfb0`): Poor gray, Common white, Uncommon green, Rare blue, Epic
/// purple, Legendary orange, Artifact gold.
pub(super) const QUALITY_RGB: [[f32; 3]; 7] = [
    [0.616, 0.616, 0.616], // 0 Poor      9d9d9d
    [1.0, 1.0, 1.0],       // 1 Common    ffffff
    [0.118, 1.0, 0.0],     // 2 Uncommon  1eff00
    [0.0, 0.439, 0.867],   // 3 Rare      0070dd
    [0.639, 0.208, 0.933], // 4 Epic      a335ee
    [1.0, 0.502, 0.0],     // 5 Legendary ff8000
    [0.902, 0.8, 0.502],   // 6 Artifact  e6cc80
];

// The tooltip color constants: white `0xc0cf60=ffffffff`, red `0xc0d390=ffff2020` (255,32,32),
// green `0xc0d3ac=ff00ff00`, gold `0xc0d3e8=ffffd200` (255,210,0), gray `0xc0d3c4=ff808080`.
pub(super) const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
pub(super) const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
pub(super) const RED: [f32; 4] = [1.0, 32.0 / 255.0, 32.0 / 255.0, 1.0];
/// The tooltip's OTHER red — `0xc0d398 = ffff0000`, a pure red distinct from the (255,32,32) the
/// requirement lines wear. Two lines use it, both in the enchant family: a **negative** enchant id
/// in slot 0/1, and ITEM_ENCHANT_DISCLAIMER (`0x52ca29`, `0x52cc13`).
pub(super) const ENCHANT_RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
pub(super) const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];
pub(super) const GRAY: [f32; 4] = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
/// The owned set member's pale cream — byte-read `0xc0d368 = ffffff97` (writer `0x529050`).
pub(super) const CREAM: [f32; 4] = [1.0, 1.0, 151.0 / 255.0, 1.0];

/// InventoryType → the slot line's GlobalString **key** — the builder's own 30-entry pointer
/// table at `0x83ddb0`, indexed by `[record+0x2c]` directly (`0x52c103: mov ecx,[ecx*4+0x83ddb0]`),
/// dumped entry by entry rather than matched by English.
///
/// **Index 0 and index 29 are the pre-seeded empty string `0x882748`** — a non-equip item and an
/// out-of-range type name nothing, which is the `None` here. 18 (bag) and 27 (quiver) *do* have
/// entries (`INVTYPE_BAG`/`INVTYPE_QUIVER`) but are unreachable: both are containers, and a
/// container takes the CONTAINER_SLOTS line above this call instead.
///
/// **Four of these keys ship no value, and that is the reference's behaviour, not a gap.**
/// `INVTYPE_AMMO` (24), `INVTYPE_THROWN` (25), `INVTYPE_RANGEDRIGHT` (26) and `INVTYPE_QUIVER`
/// (27) are in the exe's table but absent from `GlobalStrings.lua`, so `FrameScript_GetText`
/// hands the builder an empty left cell. We had written "Projectile", "Thrown" and "Ranged" into
/// those arms — the invention class decision 2045 says a text-matching tripwire cannot catch.
///
/// **But an arrow's cell is not empty, and this table is not why.** A class-6 item never reaches
/// here at all: the builder forks to `ItemClass.dbc`'s own name for it one instruction earlier
/// (`0x52c0bc`), so ammunition reads "Projectile | Arrow" — the word 2080 deleted, restored by
/// the mechanism that actually produces it rather than by the key that does not. `INVTYPE_RANGED`
/// (15) likewise **does** ship, so a bow reads "Ranged | Bow"; it is 25 and 26 — thrown weapons
/// and guns/crossbows/wands, all ItemClass 2 — that genuinely draw the type word alone.
pub(super) fn invtype_key(t: u32) -> Option<&'static str> {
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

/// The damage block's bias constant — `[0x808120]`, byte-exact `0.9999899864196777`
/// (`0x3f7fff58`). Neither `0.5` nor `1.0`: the epsilon is what stops an exactly-integral max
/// from being bumped by one, and a true `ceil()` differs from this expression for any value
/// within ~1e-5 above an integer.
const DAMAGE_BIAS: f32 = f32::from_bits(0x3f7f_ff58);

/// `floor(DamageMin)` as the builder computes it — `fsub [0x808120]` iff the value is **not**
/// greater than 0, then `__ftol`'s truncate-toward-zero (`0x52c253`–`0x52c276`).
pub(super) fn floor_min(m: f32) -> i32 {
    (if m > 0.0 { m } else { m - DAMAGE_BIAS }) as i32
}

/// `ceil(DamageMax)` — the mirror: `fadd [0x808120]` iff the value **is** greater than 0, then
/// the same truncating conversion (`0x52c26e`–`0x52c28c`).
pub(super) fn ceil_max(m: f32) -> i32 {
    (if m > 0.0 { m + DAMAGE_BIAS } else { m }) as i32
}

/// A damage/resistance school's name key — `SPELL_SCHOOL%d_CAP`, the one string the builder
/// composes at runtime rather than naming outright (`0x84e4cc`, pushed at `0x52c2a8` for the
/// damage line and `0x52c8d1` for the resistance line).
///
/// The index is the school itself: `SPELL_SCHOOL0_CAP` is "Physical" and 1..6 are
/// Holy/Fire/Nature/Frost/Shadow/Arcane. **School 0 answers `None`** because the reference
/// doesn't name it either — a physical weapon takes the school-less `DAMAGE_TEMPLATE` arm rather
/// than printing the word.
pub(super) fn school_key(s: u32) -> Option<String> {
    (1..=6).contains(&s).then(|| format!("SPELL_SCHOOL{s}_CAP"))
}

/// Stat-mod type → its `ITEM_MOD_*` key. Not a *name*: the key resolves to the whole line
/// template, sign hole and all (`ITEM_MOD_AGILITY = "%c%d Agility"`), which is why 2 has no arm —
/// the builder's 8-way jump table `0x52e510` skips it (keys byte-read at
/// `0x52c6eb..0x52c777`).
pub(super) fn stat_key(t: u32) -> Option<&'static str> {
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

/// Class id → display name (vanilla playable ids; the `ITEM_CLASSES_ALLOWED` list).
pub(super) const CLASS_NAMES: [(u32, &str); 9] = [
    (1, "Warrior"),
    (2, "Paladin"),
    (3, "Hunter"),
    (4, "Rogue"),
    (5, "Priest"),
    (7, "Shaman"),
    (8, "Mage"),
    (9, "Warlock"),
    (11, "Druid"),
];

/// Race id → display name (vanilla playable ids; the `ITEM_RACES_ALLOWED` list).
pub(super) const RACE_NAMES: [(u32, &str); 8] = [
    (1, "Human"),
    (2, "Orc"),
    (3, "Dwarf"),
    (4, "Night Elf"),
    (5, "Undead"),
    (6, "Tauren"),
    (7, "Gnome"),
    (8, "Troll"),
];

/// A playable-ids mask covering every listed id — a mask covering all of them shows no line.
pub(super) fn full_mask(ids: &[(u32, &str)]) -> i32 {
    ids.iter().fold(0i32, |m, &(id, _)| m | (1 << (id - 1)))
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
