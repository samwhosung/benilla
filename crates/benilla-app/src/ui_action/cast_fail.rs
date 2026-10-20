//! Cast-failure text, resolved in the three layers of the reference's handler `0x6e1a00`:
//!
//! 1. The wire reason names a `SPELL_FAILED_*` GlobalStrings key ([`CAST_FAIL_KEYS`]); a spell
//!    that teaches the pet reads the key's `_PET` variant first ([`trains_pet`]).
//! 2. An argument arm (`0x6e1d8e`) fills the text's `%s`: the DBC arms in [`FailArgs::fill`],
//!    the item and subclass arms (`0x78`, `0x5c`, `0x19`-`0x1b`, `0x31`) in the drain.
//! 3. A per-reason errorId goes to `CGGameUI::DisplayError` (`0x496720`), which formats the id's
//!    string with the text (`0x4967e9`): the default `0x2c` and most overrides are a bare `"%s"`,
//!    the rest their own line. The player's chain ([`player_error_key`]) needs the spell's record;
//!    the pet's handler `0x6e8eb0` has its own ids ([`Caster`]).
//!
//! The combat log (`0x62c360`) reads the text before the errorId formats it
//! ([`CastFailLine::arg_text`]). The `0x2f` and `0x30` arm (`0x6e1e3d`) fills nothing.
//! `0x84` and `0x90` are unreachable in 1.12: the server never sends them, and their only client
//! raise sites (`0x49614e`, `0x496128`) need the prospecting effect `0x7f`, which no shipped
//! spell has.

use std::collections::HashMap;

use benilla_formats::{
    AreaTableCatalog, ShapeshiftForm, SpellDisplay, SpellFocusCatalog, SpellMechanicCatalog,
};

use super::Caster;

/// Wire reason to its `SPELL_FAILED_*` GlobalStrings key: the reference's table `0x6e23e0`, in
/// vmangos `SpellCastResult` order (`SpellDefines.h:350`).
pub(crate) const CAST_FAIL_KEYS: [&str; 146] = [
    "SPELL_FAILED_AFFECTING_COMBAT",             // 0x00
    "SPELL_FAILED_ALREADY_AT_FULL_HEALTH",       // 0x01
    "SPELL_FAILED_ALREADY_AT_FULL_POWER",        // 0x02
    "SPELL_FAILED_ALREADY_BEING_TAMED",          // 0x03
    "SPELL_FAILED_ALREADY_HAVE_CHARM",           // 0x04
    "SPELL_FAILED_ALREADY_HAVE_SUMMON",          // 0x05
    "SPELL_FAILED_ALREADY_OPEN",                 // 0x06
    "SPELL_FAILED_AURA_BOUNCED",                 // 0x07
    "SPELL_FAILED_AUTOTRACK_INTERRUPTED",        // 0x08
    "SPELL_FAILED_BAD_IMPLICIT_TARGETS",         // 0x09
    "SPELL_FAILED_BAD_TARGETS",                  // 0x0a
    "SPELL_FAILED_CANT_BE_CHARMED",              // 0x0b
    "SPELL_FAILED_CANT_BE_DISENCHANTED",         // 0x0c
    "SPELL_FAILED_CANT_BE_PROSPECTED",           // 0x0d
    "SPELL_FAILED_CANT_CAST_ON_TAPPED",          // 0x0e
    "SPELL_FAILED_CANT_DUEL_WHILE_INVISIBLE",    // 0x0f
    "SPELL_FAILED_CANT_DUEL_WHILE_STEALTHED",    // 0x10
    "SPELL_FAILED_CANT_STEALTH",                 // 0x11
    "SPELL_FAILED_CASTER_AURASTATE",             // 0x12
    "SPELL_FAILED_CASTER_DEAD",                  // 0x13
    "SPELL_FAILED_CHARMED",                      // 0x14
    "SPELL_FAILED_CHEST_IN_USE",                 // 0x15
    "SPELL_FAILED_CONFUSED",                     // 0x16
    "SPELL_FAILED_DONT_REPORT",                  // 0x17
    "SPELL_FAILED_EQUIPPED_ITEM",                // 0x18
    "SPELL_FAILED_EQUIPPED_ITEM_CLASS",          // 0x19
    "SPELL_FAILED_EQUIPPED_ITEM_CLASS_MAINHAND", // 0x1a
    "SPELL_FAILED_EQUIPPED_ITEM_CLASS_OFFHAND",  // 0x1b
    "SPELL_FAILED_ERROR",                        // 0x1c
    "SPELL_FAILED_FIZZLE",                       // 0x1d
    "SPELL_FAILED_FLEEING",                      // 0x1e
    "SPELL_FAILED_FOOD_LOWLEVEL",                // 0x1f
    "SPELL_FAILED_HIGHLEVEL",                    // 0x20
    "SPELL_FAILED_HUNGER_SATIATED",              // 0x21
    "SPELL_FAILED_IMMUNE",                       // 0x22
    "SPELL_FAILED_INTERRUPTED",                  // 0x23
    "SPELL_FAILED_INTERRUPTED_COMBAT",           // 0x24
    "SPELL_FAILED_ITEM_ALREADY_ENCHANTED",       // 0x25
    "SPELL_FAILED_ITEM_GONE",                    // 0x26
    "SPELL_FAILED_ITEM_NOT_FOUND",               // 0x27
    "SPELL_FAILED_ITEM_NOT_READY",               // 0x28
    "SPELL_FAILED_LEVEL_REQUIREMENT",            // 0x29
    "SPELL_FAILED_LINE_OF_SIGHT",                // 0x2a
    "SPELL_FAILED_LOWLEVEL",                     // 0x2b
    "SPELL_FAILED_LOW_CASTLEVEL",                // 0x2c
    "SPELL_FAILED_MAINHAND_EMPTY",               // 0x2d
    "SPELL_FAILED_MOVING",                       // 0x2e
    "SPELL_FAILED_NEED_AMMO",                    // 0x2f
    "SPELL_FAILED_NEED_AMMO_POUCH",              // 0x30
    "SPELL_FAILED_NEED_EXOTIC_AMMO",             // 0x31
    "SPELL_FAILED_NOPATH",                       // 0x32
    "SPELL_FAILED_NOT_BEHIND",                   // 0x33
    "SPELL_FAILED_NOT_FISHABLE",                 // 0x34
    "SPELL_FAILED_NOT_HERE",                     // 0x35
    "SPELL_FAILED_NOT_INFRONT",                  // 0x36
    "SPELL_FAILED_NOT_IN_CONTROL",               // 0x37
    "SPELL_FAILED_NOT_KNOWN",                    // 0x38
    "SPELL_FAILED_NOT_MOUNTED",                  // 0x39
    "SPELL_FAILED_NOT_ON_TAXI",                  // 0x3a
    "SPELL_FAILED_NOT_ON_TRANSPORT",             // 0x3b
    "SPELL_FAILED_NOT_READY",                    // 0x3c
    "SPELL_FAILED_NOT_SHAPESHIFT",               // 0x3d
    "SPELL_FAILED_NOT_STANDING",                 // 0x3e
    "SPELL_FAILED_NOT_TRADEABLE",                // 0x3f
    "SPELL_FAILED_NOT_TRADING",                  // 0x40
    "SPELL_FAILED_NOT_UNSHEATHED",               // 0x41
    "SPELL_FAILED_NOT_WHILE_GHOST",              // 0x42
    "SPELL_FAILED_NO_AMMO",                      // 0x43
    "SPELL_FAILED_NO_CHARGES_REMAIN",            // 0x44
    "SPELL_FAILED_NO_CHAMPION",                  // 0x45
    "SPELL_FAILED_NO_COMBO_POINTS",              // 0x46
    "SPELL_FAILED_NO_DUELING",                   // 0x47
    "SPELL_FAILED_NO_ENDURANCE",                 // 0x48
    "SPELL_FAILED_NO_FISH",                      // 0x49
    "SPELL_FAILED_NO_ITEMS_WHILE_SHAPESHIFTED",  // 0x4a
    "SPELL_FAILED_NO_MOUNTS_ALLOWED",            // 0x4b
    "SPELL_FAILED_NO_PET",                       // 0x4c
    "SPELL_FAILED_NO_POWER",                     // 0x4d
    "SPELL_FAILED_NOTHING_TO_DISPEL",            // 0x4e
    "SPELL_FAILED_NOTHING_TO_STEAL",             // 0x4f
    "SPELL_FAILED_ONLY_ABOVEWATER",              // 0x50
    "SPELL_FAILED_ONLY_DAYTIME",                 // 0x51
    "SPELL_FAILED_ONLY_INDOORS",                 // 0x52
    "SPELL_FAILED_ONLY_MOUNTED",                 // 0x53
    "SPELL_FAILED_ONLY_NIGHTTIME",               // 0x54
    "SPELL_FAILED_ONLY_OUTDOORS",                // 0x55
    "SPELL_FAILED_ONLY_SHAPESHIFT",              // 0x56
    "SPELL_FAILED_ONLY_STEALTHED",               // 0x57
    "SPELL_FAILED_ONLY_UNDERWATER",              // 0x58
    "SPELL_FAILED_OUT_OF_RANGE",                 // 0x59
    "SPELL_FAILED_PACIFIED",                     // 0x5a
    "SPELL_FAILED_POSSESSED",                    // 0x5b
    "SPELL_FAILED_REAGENTS",                     // 0x5c
    "SPELL_FAILED_REQUIRES_AREA",                // 0x5d
    "SPELL_FAILED_REQUIRES_SPELL_FOCUS",         // 0x5e
    "SPELL_FAILED_ROOTED",                       // 0x5f
    "SPELL_FAILED_SILENCED",                     // 0x60
    "SPELL_FAILED_SPELL_IN_PROGRESS",            // 0x61
    "SPELL_FAILED_SPELL_LEARNED",                // 0x62
    "SPELL_FAILED_SPELL_UNAVAILABLE",            // 0x63
    "SPELL_FAILED_STUNNED",                      // 0x64
    "SPELL_FAILED_TARGETS_DEAD",                 // 0x65
    "SPELL_FAILED_TARGET_AFFECTING_COMBAT",      // 0x66
    "SPELL_FAILED_TARGET_AURASTATE",             // 0x67
    "SPELL_FAILED_TARGET_DUELING",               // 0x68
    "SPELL_FAILED_TARGET_ENEMY",                 // 0x69
    "SPELL_FAILED_TARGET_ENRAGED",               // 0x6a
    "SPELL_FAILED_TARGET_FRIENDLY",              // 0x6b
    "SPELL_FAILED_TARGET_IN_COMBAT",             // 0x6c
    "SPELL_FAILED_TARGET_IS_PLAYER",             // 0x6d
    "SPELL_FAILED_TARGET_NOT_DEAD",              // 0x6e
    "SPELL_FAILED_TARGET_NOT_IN_PARTY",          // 0x6f
    "SPELL_FAILED_TARGET_NOT_LOOTED",            // 0x70
    "SPELL_FAILED_TARGET_NOT_PLAYER",            // 0x71
    "SPELL_FAILED_TARGET_NO_POCKETS",            // 0x72
    "SPELL_FAILED_TARGET_NO_WEAPONS",            // 0x73
    "SPELL_FAILED_TARGET_UNSKINNABLE",           // 0x74
    "SPELL_FAILED_THIRST_SATIATED",              // 0x75
    "SPELL_FAILED_TOO_CLOSE",                    // 0x76
    "SPELL_FAILED_TOO_MANY_OF_ITEM",             // 0x77
    "SPELL_FAILED_TOTEMS",                       // 0x78
    "SPELL_FAILED_TRAINING_POINTS",              // 0x79
    "SPELL_FAILED_TRY_AGAIN",                    // 0x7a
    "SPELL_FAILED_UNIT_NOT_BEHIND",              // 0x7b
    "SPELL_FAILED_UNIT_NOT_INFRONT",             // 0x7c
    "SPELL_FAILED_WRONG_PET_FOOD",               // 0x7d
    "SPELL_FAILED_NOT_WHILE_FATIGUED",           // 0x7e
    "SPELL_FAILED_TARGET_NOT_IN_INSTANCE",       // 0x7f
    "SPELL_FAILED_NOT_WHILE_TRADING",            // 0x80
    "SPELL_FAILED_TARGET_NOT_IN_RAID",           // 0x81
    "SPELL_FAILED_DISENCHANT_WHILE_LOOTING",     // 0x82
    "SPELL_FAILED_PROSPECT_WHILE_LOOTING",       // 0x83
    "SPELL_FAILED_PROSPECT_NEED_MORE",           // 0x84
    "SPELL_FAILED_TARGET_FREEFORALL",            // 0x85
    "SPELL_FAILED_NO_EDIBLE_CORPSES",            // 0x86
    "SPELL_FAILED_ONLY_BATTLEGROUNDS",           // 0x87
    "SPELL_FAILED_TARGET_NOT_GHOST",             // 0x88
    "SPELL_FAILED_TOO_MANY_SKILLS",              // 0x89
    "SPELL_FAILED_TRANSFORM_UNUSABLE",           // 0x8a
    "SPELL_FAILED_WRONG_WEATHER",                // 0x8b
    "SPELL_FAILED_DAMAGE_IMMUNE",                // 0x8c
    "SPELL_FAILED_PREVENTED_BY_MECHANIC",        // 0x8d
    "SPELL_FAILED_PLAY_TIME",                    // 0x8e
    "SPELL_FAILED_REPUTATION",                   // 0x8f
    "SPELL_FAILED_MIN_SKILL",                    // 0x90
    "SPELL_FAILED_UNKNOWN",                      // 0x91
];

/// A spell's power type (`SpellRec+0x7c`) to its NO_POWER key (the reference's table `0x8118dc`);
/// health is -2.
fn no_power_key(power_type: u32) -> &'static str {
    match power_type {
        1 => "ERR_OUT_OF_RAGE",
        2 => "ERR_OUT_OF_FOCUS",
        3 => "ERR_OUT_OF_ENERGY",
        4 => "ERR_NOT_HAPPY_ENOUGH",
        0xFFFFFFFE => "ERR_OUT_OF_HEALTH",
        _ => "ERR_OUT_OF_MANA",
    }
}

/// The power nouns `0x02` names by the player's power byte, the table `0x6e213a`-`0x6e2156` builds.
const POWER_NOUNS: [&str; 5] = ["MANA", "RAGE", "FOCUS", "ENERGY", "HAPPINESS"];

/// The spell categories (`SpellRec+0x8`) the `0x28` and `0x3c` cooldown picks test.
fn is_potion(d: &SpellDisplay) -> bool {
    matches!(d.category, 4 | 9)
}
fn is_food(d: &SpellDisplay) -> bool {
    matches!(d.category, 0xA | 0xB)
}

/// `((SpellRec+0x18 & 0x10) | 0x300) >> 4`: 0x31 for an ability (`Attributes & 0x10`), else 0x30.
fn cooldown_key(spell: Option<&SpellDisplay>) -> &'static str {
    if spell.is_some_and(|d| d.attributes & 0x10 != 0) {
        "ERR_ABILITY_COOLDOWN"
    } else {
        "ERR_SPELL_COOLDOWN"
    }
}

/// The `_PET` flag `[ebp-0x10]` (`0x6e1b1b`-`0x6e1b3c`): the first effect (`+0xf4`) teaches the
/// pet, `LEARN_PET_SPELL` 0x39, or `LEARN_SPELL` 0x24 at implicit target 5 (`+0x148`), the pet.
fn trains_pet(d: &SpellDisplay) -> bool {
    match d.effects[0] {
        0x39 => true,
        0x24 => d.effect_implicit_target_a[0] == 5,
        _ => false,
    }
}

/// The player's errorId chain `0x6e1aab`-`0x6e1c5f`, in its order, by the key of the id each arm
/// stores; `None` keeps the default `0x2c`. `0x4d` picks its id in its argument arm.
fn player_error_key(reason: u8, d: &SpellDisplay) -> Option<&'static str> {
    Some(match reason {
        // `0x6e1ab7`: food 0x2f, potion 0x2e, else ability or spell.
        0x3C if is_food(d) => "ERR_FOOD_COOLDOWN",
        0x3C if is_potion(d) => "ERR_POTION_COOLDOWN",
        0x3C => cooldown_key(Some(d)),
        // `0x6e1b4d`: potion 0x2e, else 0x2d.
        0x28 if is_potion(d) => "ERR_POTION_COOLDOWN",
        0x28 => "ERR_ITEM_COOLDOWN",
        0x78 => "ERR_SPELL_FAILED_TOTEMS",        // 0xd1 @0x6e1b71
        0x5C => "ERR_SPELL_FAILED_REAGENTS",      // 0xd2 @0x6e1b7f
        0x18 => "ERR_SPELL_FAILED_EQUIPPED_ITEM", // 0xd4 @0x6e1b8d
        0x19..=0x1B => "ERR_SPELL_FAILED_EQUIPPED_ITEM_CLASS_S", // 0xd5 @0x6e1c58
        0x09 => "ERR_GENERIC_NO_TARGET",          // 0xb8 @0x6e1bb6
        0x59 => "ERR_SPELL_OUT_OF_RANGE",         // 0x131 @0x6e1bc7
        0x2F => "ERR_NOAMMO_S",                   // 0x133 @0x6e1bd8
        0x56 => "ERR_SPELL_FAILED_SHAPESHIFT_FORM_S", // 0xd6 @0x6e1be9
        // `0x6e1bfa`: a spell that targets items (`Targets & 0x10`, `+0x34`), else `0x2c`.
        0x0A if d.targets & 0x10 != 0 => "ERR_INVALID_ITEM_TARGET", // 0x13c @0x6e1c04
        0x41 => "ERR_SPELL_FAILED_NOTUNSHEATHED",                   // 0x140 @0x6e1c15
        0x01 => "ERR_SPELL_FAILED_ALREADY_AT_FULL_HEALTH",          // 0x18e @0x6e1c26
        0x02 => "ERR_SPELL_FAILED_ALREADY_AT_FULL_POWER_S",         // 0x18f @0x6e1c37
        0x8E => "ERR_PLAY_TIME_EXCEEDED",                           // 0x1bf @0x6e1c4c
        _ => return None,
    })
}

/// The pet's errorIds (`0x6e8eb0`): reasons above `0x8d` take the default arm, the rest the index
/// table `0x6e93d0`. Six are the `ERR_PET_SPELL_*` rows only this handler raises; `0x4d` picks its
/// id in its argument arm. The pet sends `0x01`, `0x02`, `0x09`, `0x18`, `0x28`, `0x41` and `0x8e`
/// to its generic arm, so it never says those lines; `0x17` goes there too and shows nothing only
/// because 1.12 ships no `SPELL_FAILED_DONT_REPORT`.
fn pet_error_key(reason: u8, spell: Option<&SpellDisplay>) -> Option<&'static str> {
    Some(match reason {
        0x00 => "ERR_PET_SPELL_AFFECTING_COMBAT", // 0x14c @0x6e8fab
        0x13 => "ERR_PET_SPELL_DEAD",             // 0x150 @0x6e9017
        0x32 => "ERR_PET_SPELL_NOPATH",           // 0x151 @0x6e9032
        0x33 => "ERR_PET_SPELL_NOT_BEHIND",       // 0x14e @0x6e8fe1
        0x59 => "ERR_PET_SPELL_OUT_OF_RANGE",     // 0x14d @0x6e8fc6
        0x5F => "ERR_PET_SPELL_ROOTED",           // 0x14b @0x6e8f4f
        0x65 => "ERR_PET_SPELL_TARGETS_DEAD",     // 0x14f @0x6e8ffc
        // `0x6e8f2a`: ability or spell only; unlike the player's `0x6e1ab7`, no food or potion leg.
        0x3C => cooldown_key(spell),
        0x56 => "ERR_SPELL_FAILED_SHAPESHIFT_FORM_S", // 0xd6 @0x6e92be
        _ => return None,
    })
}

/// The argument arms' inputs: the wire's argument word ([`super::CastFail::arg`]) and the DBC
/// name tables; an arm whose table is missing declines.
#[derive(Default, Clone, Copy)]
pub(super) struct FailArgs<'a> {
    pub(super) arg: Option<u32>,
    /// `SpellFocusObject.dbc` (`0xc0d800`), for `0x5e`.
    pub(super) focus: Option<&'a SpellFocusCatalog>,
    /// `AreaTable.dbc` (`0xc0e048`), for `0x5d`.
    pub(super) areas: Option<&'a AreaTableCatalog>,
    /// `SpellMechanic.dbc` (`0xc0d7c4`), for `0x8d`.
    pub(super) mechanics: Option<&'a SpellMechanicCatalog>,
    /// `SpellShapeshiftForm.dbc` (`0xc0d76c`), for `0x56`, keyed by form id.
    pub(super) forms: Option<&'a HashMap<u32, ShapeshiftForm>>,
    /// The name a world arm found ([`super::cast_lines`]): the subclass for `0x19`-`0x1b`
    /// (`0x6e1db7`), the projectile for `0x31` (`0x6e1e54`), the item for `0x5c` and `0x78`
    /// (`0x6e1e7f`).
    pub(super) found: Option<&'a str>,
    /// The item-cache callback `0x6e29b0`, which formats the plain key, never the `_PET` one.
    pub(super) redisplay: bool,
    /// The active player's power type, `UNIT_FIELD_BYTES_0` byte 3, for `0x02`; `None` with no
    /// player.
    pub(super) power: Option<u8>,
}

/// What an argument arm produced, and when nothing, which exit the reference takes: every player
/// arm but `0x56` declines to the shared default `0x6e21d8`, which still displays the template,
/// while `0x56` spends `edi` as its loop counter and exits to the epilogue `0x6e224f`, past the
/// display, the log and the cast abort `0x6e4940`. Every pet arm declines past its display.
pub(super) enum Fill {
    /// The name for the template's `%s`.
    Filled(String),
    /// Declined; the reference displays the unfilled template (`0x6e21d8`).
    Template,
    /// Declined; the reference displays nothing (`0x6e224f`).
    Nothing,
}

impl FailArgs<'_> {
    /// The `%s` fill for the arms this module owns; `replace` matches the reference's printf
    /// because each template has one `%s`. `0x5e` (`0x6e1f62`) and `0x5d` (`0x6e1fad`) read the
    /// wire's argument word, never `Spell.dbc`, and name a `SpellFocusObject` or `AreaTable` row.
    /// Deviation: the player's `0x5e` with no word falls back to the spell's own
    /// `RequiresSpellFocus`, which the reference never reads here, because it is the id vmangos
    /// sends (`Spell.cpp:4426`). `0x5d` has no such stand-in: vmangos takes its area from its own
    /// `spell_area` table (`Spell.cpp:4429`), which `Spell.dbc` does not carry. The pet's arm keeps
    /// the reference's read: a short packet, which vmangos's always is (`Packets/Pet.cpp:127`),
    /// leaves the word 0 (`0x6e9161`), and with no row 0 in `SpellFocusObject.dbc` the line is
    /// silent (`0x6e9385`).
    fn fill(&self, caster: Caster, reason: u8, spell: Option<&SpellDisplay>) -> Fill {
        let named = |name: Option<&str>| match (name, caster) {
            (Some(name), _) => Fill::Filled(name.to_string()),
            (None, Caster::Player) => Fill::Template,
            // The pet's id arms leave at `0x6e9385` on a bad id or a missing row, its subclass
            // arm at `0x6e90d2`: no line.
            (None, Caster::Pet) => Fill::Nothing,
        };
        match reason {
            // `0x8d` (`0x6e2190`), "Can't do that while stunned": the word is the local
            // crowd-control ladder's mechanic id, not the wire's.
            0x8D => named(self.arg.and_then(|id| self.mechanics?.name(id))),
            0x5D => named(self.arg.and_then(|id| self.areas?.name(id))),
            0x5E => {
                let word = self.arg.unwrap_or(0);
                let id = match caster {
                    Caster::Player if word == 0 => spell.map_or(0, |d| d.requires_spell_focus),
                    _ => word,
                };
                named(self.focus.and_then(|f| f.name(id)))
            }
            0x56 => self.shapeshift_forms(caster, spell),
            // The world arms; the pet's table has the subclass arm alone (`0x6e904d`).
            0x19..=0x1B => named(self.found),
            0x31 | 0x5C | 0x78 if caster == Caster::Player => named(self.found),
            _ => Fill::Template,
        }
    }

    /// `0x56` ONLY_SHAPESHIFT (`0x6e1ff8`), "Must be in Cat Form": no wire word, but the spell's
    /// own `Stances` mask (`SpellRec+0x2c`), each set bit `b` naming the DBC's row `b`, which in
    /// 1.12's file is form id `b+1`. Empty names are skipped (1.12 ships fourteen) and the rest
    /// joined with `", "` (`0x84480c`). It never tests `StancesNot`, nor the permissive
    /// `AttributesEx2` bit `0x80000` the tooltip honours (`0x52f115`). No spell, no DBC or no
    /// name is [`Fill::Nothing`].
    fn shapeshift_forms(&self, caster: Caster, spell: Option<&SpellDisplay>) -> Fill {
        let (Some(forms), Some(stances)) = (self.forms, spell.map(|d| d.stances)) else {
            return Fill::Nothing;
        };
        let mut named = (0..u32::BITS)
            .filter(|b| stances & (1 << b) != 0)
            .filter_map(|b| forms.get(&(b + 1)))
            .map(|f| f.name.as_str())
            .filter(|n| !n.is_empty());
        match caster {
            // The pet's arm (`0x6e921a`) reads the same rows but leaves the loop at the first
            // name (`0x6e9282`); it never joins.
            Caster::Pet => named.next().map(str::to_string),
            Caster::Player => {
                let all: Vec<&str> = named.collect();
                (!all.is_empty()).then(|| all.join(", "))
            }
        }
        .map_or(Fill::Nothing, Fill::Filled)
    }
}

/// The default errorId `0x2c`, whose string is 1.12's bare `"%s"`, formatted through its key as
/// every id is; this id's record decides the surface.
pub(super) const PASSTHROUGH: &str = "ERR_SPELL_FAILED_S";

/// One resolved cast-failure line: the text, and the errorId's key, whose catalog record picks
/// the surface (`DisplayError` reads its kind) and the voice; `ERR_SPELL_FAILED_NOTUNSHEATHED` is
/// the one yellow override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CastFailLine {
    pub key: &'static str,
    pub text: String,
    /// The text the errorId formats, which the combat log (`0x62c360`) reads too (`edi` at
    /// `0x6e21e2`): the first-layer string (`0x6e1d77`), or what an arm wrote over it
    /// (`0x6e21d2`); empty means the log reads the displayed text ([`Self::logged`]).
    pub arg_text: String,
}

impl CastFailLine {
    /// A line on the default errorId, both buffers holding the same text, as after a filled arm.
    pub(super) fn passthrough(text: String) -> Self {
        Self {
            key: PASSTHROUGH,
            arg_text: text.clone(),
            text,
        }
    }

    /// What the combat log prints (`0x6e21e2`): the argText buffer, or when it is empty the
    /// last-error cell `0xb4da40`, which holds the displayed text (`0x496802`).
    pub(super) fn logged(&self) -> &str {
        if self.arg_text.is_empty() {
            &self.text
        } else {
            &self.arg_text
        }
    }
}

/// The line for a failed cast, `None` where the reference leaves before `DisplayError` and the
/// log; `get` is the VM's GlobalStrings lookup. An empty [`CastFailLine::text`] shows nothing and
/// still logs. Deviation: a reason past the table prints its hex code, so an unknown reason is
/// visible.
pub(super) fn cast_fail_text(
    caster: Caster,
    reason: u8,
    spell: Option<&SpellDisplay>,
    args: FailArgs<'_>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<CastFailLine> {
    let get_text = |key: &str| get(key).filter(|s| !s.is_empty());
    let player = caster == Caster::Player;
    // DONT_REPORT jumps past DisplayError (`0x6e1cfa`).
    if player && reason == 0x17 {
        return None;
    }
    let Some(&key) = CAST_FAIL_KEYS.get(usize::from(reason)) else {
        return Some(CastFailLine::passthrough(format!(
            "Spell failed ({reason:#04x})"
        )));
    };
    // The errorId `[ebp-0x8]`, `0x2c` until an arm stores one (`0x6e1a45`). With no record the
    // player's chain and its `_PET` flag are skipped (`0x6e1a7c`), and the pet's handler leaves
    // (`0x6e8f04`).
    let (mut error, pet_text) = match (caster, spell) {
        (Caster::Player, Some(d)) => (
            player_error_key(reason, d),
            !args.redisplay && trains_pet(d),
        ),
        (Caster::Player, None) => (None, false),
        (Caster::Pet, None) => return None,
        (Caster::Pet, Some(_)) => (pet_error_key(reason, spell), false),
    };
    // The first layer `[ebp-0x488]`: `<key>_PET` when flagged and not empty (`0x6e1d54`), else the
    // key's own string; empty when 1.12 ships no such key.
    let first = pet_text
        .then(|| get_text(&format!("{key}_PET")))
        .flatten()
        .or_else(|| get_text(key))
        .unwrap_or_default();
    // The argument arms (`0x6e1d8e`).
    let text = match reason {
        // `0x6e211f`: the noun of the active player's own power (`[[player+0x110]+0x7b]`) alone,
        // leaving past the display and the log on a byte of 5 or more (`0x6e2131`) or an empty
        // noun (`0x6e2176`).
        0x02 if player => get_text(POWER_NOUNS.get(usize::from(args.power?))?)?,
        // `0x6e20eb`, and the pet's `0x6e8f6a`: health takes `0x123`, the rest the five ids of
        // `0x8118dc` (the pet's `0x8118f0` holds the same), with an empty text (`0x6e20fd`).
        0x4D => {
            error = Some(no_power_key(spell.map_or(0, |d| d.power_type)));
            String::new()
        }
        _ => match args.fill(caster, reason, spell) {
            Fill::Filled(name) => first.replace("%s", &name),
            Fill::Template => strip_tokens(&first),
            // The epilogue `0x6e224f` skips the log too.
            Fill::Nothing => return None,
        },
    };
    // `DisplayError` formats the id's string, its key's live global (`0x4967d7`), with the text
    // (`0x4967e9`); an absent or empty one shows nothing (`0x4945b0`), and the log still reads the
    // text (`0x6e21e2`).
    let key = error.unwrap_or(PASSTHROUGH);
    let shown = get_text(key)
        .map(|format| format.replace("%s", &text))
        .unwrap_or_default();
    Some(CastFailLine {
        key,
        text: shown,
        arg_text: text,
    })
}

/// Whether the reporter `0x6e1a00` reaches its slot pop `0x6e2201`/`0x6e2218` for a player
/// failure, the one thing the handler `0x6e7330` reads back from it (`0x6e7408`). It leaves
/// through the epilogue `0x6e224f` before the pop in exactly these cases:
/// - reason `0x17` (`0x6e1ce4`, `0x6e1cfa`), every spell;
/// - `0x56` with no spell, no form or no name (`0x6e1ffd`, `0x6e2016`, `0x6e20c6`), the same
///   [`Fill::Nothing`] the display takes;
/// - `0x5c` and `0x78` when the item cache has no record for the first word (`0x6e1eab`; the
///   miss asks, and the callback `0x6e29b0` redisplays without touching the cast).
///
/// `0x02` leaves at `0x6e212b`, `0x6e2134`, `0x6e216d` and `0x6e2176` for a power byte past 4 or
/// an empty noun; a 1.12 player's power type is 0 to 4 and `MANA` to `HAPPINESS` all ship, so it
/// never leaves. `0x84` leaves on a miss of its second word (`0x6e1efc`), which no 1.12 server
/// sends (module note). Every other path, an empty or absent text included, runs on to the pop.
///
/// `item_cached` is the item cache's record test for an entry, asking the server on a miss as
/// `GetRecord` does.
pub(crate) fn report_reaches_pop(
    reason: u8,
    arg: Option<u32>,
    spell: Option<&SpellDisplay>,
    forms: Option<&HashMap<u32, ShapeshiftForm>>,
    item_cached: impl FnOnce(u32) -> bool,
) -> bool {
    match reason {
        0x17 => false,
        0x56 => {
            let args = FailArgs {
                forms,
                ..Default::default()
            };
            !matches!(args.fill(Caster::Player, reason, spell), Fill::Nothing)
        }
        0x5c | 0x78 => arg.is_some_and(item_cached),
        _ => true,
    }
}

/// Deviation: a declined arm's template loses its tokens ("Missing reagent: %s" reads "Missing
/// reagent") where the reference shows the literal `%s`, because a raw token reads as broken.
/// Applied to both buffers.
fn strip_tokens(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    text.replace("%s", "")
        .replace("%d", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches([' ', ':', '.', '(', ')'])
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Shipped 1.12 `GlobalStrings.lua` values.
    fn gs() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("SPELL_FAILED_NO_AMMO", "Out of ammo"),
            ("SPELL_FAILED_OUT_OF_RANGE", "Out of range"),
            ("SPELL_FAILED_TOO_CLOSE", "Target too close"),
            ("SPELL_FAILED_NOT_READY", "Not yet recovered"),
            ("SPELL_FAILED_REAGENTS", "Missing reagent: %s"),
            ("SPELL_FAILED_ONLY_SHAPESHIFT", "Must be in %s"),
            ("ERR_SPELL_OUT_OF_RANGE", "Out of range."),
            ("ERR_GENERIC_NO_TARGET", "You have no target."),
            ("ERR_SPELL_COOLDOWN", "Spell is not ready yet."),
            ("ERR_ABILITY_COOLDOWN", "Ability is not ready yet."),
            ("ERR_POTION_COOLDOWN", "You cannot drink any more yet."),
            ("ERR_FOOD_COOLDOWN", "You are too full to eat more now."),
            ("ERR_ITEM_COOLDOWN", "Item is not ready yet."),
            ("SPELL_FAILED_ITEM_NOT_READY", "Item is not ready yet"),
            ("ERR_OUT_OF_MANA", "Not enough mana"),
            ("ERR_OUT_OF_RAGE", "Not enough rage"),
            (
                // Verbatim from the shipped file.
                "ERR_SPELL_FAILED_NOTUNSHEATHED",
                "You have to be unsheathed to do that!",
            ),
            // The pet's six; 1.12 ships no `ERR_PET_SPELL_NOPATH`.
            ("ERR_PET_SPELL_AFFECTING_COMBAT", "Your pet is in combat."),
            ("ERR_PET_SPELL_DEAD", "Your pet is dead."),
            (
                "ERR_PET_SPELL_NOT_BEHIND",
                "Your pet must be behind its target.",
            ),
            ("ERR_PET_SPELL_OUT_OF_RANGE", "Your pet is out of range."),
            ("ERR_PET_SPELL_ROOTED", "Your pet is unable to move."),
            ("ERR_PET_SPELL_TARGETS_DEAD", "Your pet\'s target is dead."),
            ("SPELL_FAILED_AFFECTING_COMBAT", "You are in combat"),
            ("SPELL_FAILED_CASTER_DEAD", "You are dead"),
            ("SPELL_FAILED_NOPATH", "No path available"),
            ("SPELL_FAILED_NOT_BEHIND", "You must be behind your target"),
            ("SPELL_FAILED_ROOTED", "You are unable to move"),
            ("SPELL_FAILED_TARGETS_DEAD", "Your target is dead"),
            ("SPELL_FAILED_BAD_IMPLICIT_TARGETS", "No target"),
            ("SPELL_FAILED_NOT_UNSHEATHED", "You must be unsheathed"),
            ("ERR_OUT_OF_FOCUS", "Not enough focus"),
            // The default errorId and the chain's other messages.
            ("ERR_SPELL_FAILED_S", "%s"),
            ("ERR_SPELL_FAILED_TOTEMS", "%s"),
            ("ERR_SPELL_FAILED_REAGENTS", "%s"),
            (
                "ERR_SPELL_FAILED_EQUIPPED_ITEM",
                "Must have the proper item equipped",
            ),
            ("ERR_SPELL_FAILED_EQUIPPED_ITEM_CLASS_S", "%s"),
            ("ERR_SPELL_FAILED_SHAPESHIFT_FORM_S", "%s"),
            ("ERR_NOAMMO_S", "%s"),
            (
                "ERR_INVALID_ITEM_TARGET",
                "That item is not a valid target.",
            ),
            (
                "ERR_SPELL_FAILED_ALREADY_AT_FULL_HEALTH",
                "You are already at full Health.",
            ),
            ("ERR_PLAY_TIME_EXCEEDED", "Maximum play time exceeded"),
            ("SPELL_FAILED_TOTEMS", "Requires %s"),
            (
                "SPELL_FAILED_EQUIPPED_ITEM",
                "Must have the proper item equipped",
            ),
            (
                "SPELL_FAILED_EQUIPPED_ITEM_CLASS",
                "Must have a %s equipped",
            ),
            (
                "SPELL_FAILED_EQUIPPED_ITEM_CLASS_MAINHAND",
                "Must have a %s equipped in the main hand",
            ),
            (
                "SPELL_FAILED_EQUIPPED_ITEM_CLASS_OFFHAND",
                "Must have a %s equipped in the offhand",
            ),
            (
                "SPELL_FAILED_NEED_AMMO",
                "Ammo needs to be in the paper doll ammo slot before it can be fired",
            ),
            ("SPELL_FAILED_NEED_EXOTIC_AMMO", "Requires exotic ammo: %s"),
            ("SPELL_FAILED_BAD_TARGETS", "Invalid target"),
            (
                "SPELL_FAILED_LEVEL_REQUIREMENT",
                "You are not high enough level",
            ),
            (
                "SPELL_FAILED_LEVEL_REQUIREMENT_PET",
                "Your pet is not high enough level",
            ),
            (
                "SPELL_FAILED_SPELL_UNAVAILABLE",
                "The spell is not available to you",
            ),
            (
                "SPELL_FAILED_SPELL_UNAVAILABLE_PET",
                "That ability is not available to your pet",
            ),
        ])
    }

    /// Any record, so the player's chain runs (`0x6e1a7c`).
    fn record() -> SpellDisplay {
        SpellDisplay::default()
    }

    /// The player's chain `0x6e1aab`-`0x6e1c5f` arm by arm, in its order: each reason's message
    /// key, pinned to the errorId immediate the arm stores, through the catalog.
    #[test]
    fn the_players_chain_stores_each_arms_error_id() {
        let mut m = gs();
        m.insert(
            "ERR_SPELL_FAILED_ALREADY_AT_FULL_POWER_S",
            "You are already at full %s.",
        );
        m.insert("MANA", "Mana");
        let g = getter(&m);
        let forms = forms([(1, "Cat Form")]);
        let cat_only = SpellDisplay {
            stances: 0x1,
            ..record()
        };
        let item_target = SpellDisplay {
            targets: 0x10,
            ..record()
        };
        let line = |reason: u8, d: &SpellDisplay| {
            let args = FailArgs {
                forms: Some(&forms),
                found: Some("Name"),
                power: Some(0),
                ..FailArgs::default()
            };
            cast_fail_text(Caster::Player, reason, Some(d), args, &g)
                .unwrap_or_else(|| panic!("{reason:#04x} shows a line"))
                .key
        };
        for (reason, d, id, at) in [
            (0x3C, spell(0, 0xA, 0), 0x2f, "0x6e1aea"),
            (0x3C, spell(0, 4, 0), 0x2e, "0x6e1ae1"),
            (0x3C, spell(0, 0, 0x10), 0x31, "0x6e1ace"),
            (0x3C, record(), 0x30, "0x6e1ace"),
            (0x28, spell(0, 9, 0), 0x2e, "0x6e1b63"),
            (0x28, record(), 0x2d, "0x6e1b5a"),
            (0x78, record(), 0xd1, "0x6e1b71"),
            (0x5C, record(), 0xd2, "0x6e1b7f"),
            (0x18, record(), 0xd4, "0x6e1b8d"),
            (0x19, record(), 0xd5, "0x6e1c58"),
            (0x1A, record(), 0xd5, "0x6e1c58"),
            (0x1B, record(), 0xd5, "0x6e1c58"),
            (0x09, record(), 0xb8, "0x6e1bb6"),
            (0x59, record(), 0x131, "0x6e1bc7"),
            (0x2F, record(), 0x133, "0x6e1bd8"),
            (0x56, cat_only, 0xd6, "0x6e1be9"),
            (0x0A, item_target, 0x13c, "0x6e1c04"),
            (0x0A, record(), 0x2c, "0x6e1bfe"),
            (0x41, record(), 0x140, "0x6e1c15"),
            (0x01, record(), 0x18e, "0x6e1c26"),
            (0x02, record(), 0x18f, "0x6e1c37"),
            (0x8E, record(), 0x1bf, "0x6e1c4c"),
            // Every other reason keeps the default (`0x6e1a45`).
            (0x43, record(), 0x2c, "0x6e1a45"),
        ] {
            let key = line(reason, &d);
            assert_eq!(
                benilla_ui::messages::by_key(key).map(|r| r.id),
                Some(id),
                "{reason:#04x} stores {id:#x} at {at}, the catalog's {key}"
            );
        }
    }

    /// `0x2f` is errorId `0x133`, `ERR_NOAMMO_S`, a bare `"%s"` of the reason's own text: its
    /// argument arm (`0x6e1e3d`) fills nothing, so the line is `SPELL_FAILED_NEED_AMMO`'s, and
    /// the record's error-speech id 1 voices it.
    #[test]
    fn need_ammo_shows_its_own_text_under_err_noammo_s() {
        let m = gs();
        let g = getter(&m);
        let line = cast_fail_text(
            Caster::Player,
            0x2F,
            Some(&record()),
            FailArgs::default(),
            &g,
        )
        .expect("a line");
        assert_eq!(line.key, "ERR_NOAMMO_S");
        assert_eq!(
            line.text,
            "Ammo needs to be in the paper doll ammo slot before it can be fired"
        );
        assert_eq!(line.logged(), line.text);
        assert_eq!(
            benilla_ui::messages::by_key(line.key).map(|r| r.type_tag),
            Some(0x01)
        );
    }

    /// `0x0a` tests `Targets & 0x10` (`0x6e1bfa`): an item-targeted spell says
    /// `ERR_INVALID_ITEM_TARGET`, logging the reason's own text; any other keeps "Invalid target".
    #[test]
    fn a_bad_item_target_reads_err_invalid_item_target() {
        let m = gs();
        let g = getter(&m);
        let line = |d: &SpellDisplay| {
            cast_fail_text(Caster::Player, 0x0A, Some(d), FailArgs::default(), &g).expect("a line")
        };
        let enchant = SpellDisplay {
            targets: 0x10,
            ..record()
        };
        let on_item = line(&enchant);
        assert_eq!(on_item.key, "ERR_INVALID_ITEM_TARGET");
        assert_eq!(on_item.text, "That item is not a valid target.");
        assert_eq!(on_item.logged(), "Invalid target");
        let on_unit = line(&SpellDisplay {
            targets: 0x2,
            ..record()
        });
        assert_eq!(on_unit.key, PASSTHROUGH);
        assert_eq!(on_unit.text, "Invalid target");
    }

    /// A refusal with no `Spell.dbc` record skips the chain (`0x6e1a7c`): the default errorId shows
    /// the reason's own text.
    #[test]
    fn with_no_record_the_player_keeps_the_default_error_id() {
        let m = gs();
        let g = getter(&m);
        for (reason, text) in [
            (0x59, "Out of range"),
            (0x09, "No target"),
            (0x41, "You must be unsheathed"),
            (0x3C, "Not yet recovered"),
        ] {
            let line = cast_fail_text(Caster::Player, reason, None, FailArgs::default(), &g)
                .expect("a line");
            assert_eq!(line.key, PASSTHROUGH, "{reason:#04x}");
            assert_eq!(line.text, text, "{reason:#04x}");
        }
    }

    /// The `_PET` flag (`0x6e1b1b`-`0x6e1b3c`): a first effect of `LEARN_PET_SPELL` (0x39), or
    /// `LEARN_SPELL` (0x24) at target 5, reads `<key>_PET` first and keeps it when not empty
    /// (`0x6e1d22`-`0x6e1d54`), for the screen and the log alike.
    #[test]
    fn pet_training_spells_read_the_pet_variant_first() {
        let m = gs();
        let g = getter(&m);
        let line = |reason: u8, d: &SpellDisplay| {
            cast_fail_text(Caster::Player, reason, Some(d), FailArgs::default(), &g)
                .expect("a line")
        };
        let teach = SpellDisplay {
            effects: [0x39, 0, 0],
            ..record()
        };
        let mut train = SpellDisplay {
            effects: [0x24, 0, 0],
            ..record()
        };
        train.effect_implicit_target_a[0] = 5;
        let mut learn = SpellDisplay {
            effects: [0x24, 0, 0],
            ..record()
        };
        learn.effect_implicit_target_a[0] = 1;

        let unavailable = line(0x63, &teach);
        assert_eq!(
            unavailable.text,
            "That ability is not available to your pet"
        );
        assert_eq!(unavailable.logged(), unavailable.text);
        assert_eq!(line(0x29, &train).text, "Your pet is not high enough level");
        // A learn spell aimed elsewhere, and a reason with no `_PET` key, read the plain key.
        assert_eq!(line(0x29, &learn).text, "You are not high enough level");
        assert_eq!(line(0x43, &teach).text, "Out of ammo");
        // The pet's handler `0x6e8eb0` has no `_PET` lookup.
        assert_eq!(
            cast_fail_text(Caster::Pet, 0x63, Some(&teach), FailArgs::default(), &g)
                .unwrap()
                .text,
            "The spell is not available to you"
        );
    }

    /// The item-cache callback `0x6e29b0` formats the plain key (`0x6e29da`), so a `_PET`
    /// string an addon defines reaches the first pass only.
    #[test]
    fn the_item_redisplay_reads_the_plain_key() {
        let mut m = gs();
        m.insert("SPELL_FAILED_REAGENTS_PET", "Your pet lacks %s");
        let g = getter(&m);
        let teach = SpellDisplay {
            effects: [0x39, 0, 0],
            ..record()
        };
        let text = |redisplay| {
            let args = FailArgs {
                found: Some("Soul Shard"),
                redisplay,
                ..FailArgs::default()
            };
            cast_fail_text(Caster::Player, 0x5C, Some(&teach), args, &g)
                .unwrap()
                .text
        };
        assert_eq!(text(false), "Your pet lacks Soul Shard");
        assert_eq!(text(true), "Missing reagent: Soul Shard");
    }

    /// The world arms' names fill the reason's text, shown under the chain's errorId: `0x78`
    /// `0xd1`, `0x5c` `0xd2`, `0x19`-`0x1b` `0xd5`; `0x31` has no arm in the chain. The pet's
    /// `0x19`-`0x1b` show under `0x2c` (`0x6e9102`).
    #[test]
    fn the_world_arms_fill_the_chains_messages() {
        let m = gs();
        let g = getter(&m);
        let line = |caster, reason: u8, name: &str| {
            let args = FailArgs {
                found: Some(name),
                ..FailArgs::default()
            };
            let l = cast_fail_text(caster, reason, Some(&record()), args, &g).expect("a line");
            (l.key, l.text)
        };
        for (reason, name, key, text) in [
            (
                0x78,
                "Mining Pick",
                "ERR_SPELL_FAILED_TOTEMS",
                "Requires Mining Pick",
            ),
            (
                0x5C,
                "Soul Shard",
                "ERR_SPELL_FAILED_REAGENTS",
                "Missing reagent: Soul Shard",
            ),
            (
                0x19,
                "Wand",
                "ERR_SPELL_FAILED_EQUIPPED_ITEM_CLASS_S",
                "Must have a Wand equipped",
            ),
            (
                0x1A,
                "Dagger",
                "ERR_SPELL_FAILED_EQUIPPED_ITEM_CLASS_S",
                "Must have a Dagger equipped in the main hand",
            ),
            (
                0x1B,
                "Shield",
                "ERR_SPELL_FAILED_EQUIPPED_ITEM_CLASS_S",
                "Must have a Shield equipped in the offhand",
            ),
            (0x31, "Arrow", PASSTHROUGH, "Requires exotic ammo: Arrow"),
        ] {
            assert_eq!(
                line(Caster::Player, reason, name),
                (key, text.to_string()),
                "{reason:#04x}"
            );
        }
        assert_eq!(
            line(Caster::Pet, 0x19, "Wand"),
            (PASSTHROUGH, "Must have a Wand equipped".to_string())
        );
    }

    /// The pet's `0x56` shows its first form under errorId `0xd6` (`0x6e92be`).
    #[test]
    fn the_pets_only_shapeshift_line_is_error_id_0xd6() {
        let m = gs();
        let g = getter(&m);
        let forms = forms([(1, "Cat Form")]);
        let d = SpellDisplay {
            stances: 0x1,
            ..record()
        };
        let args = FailArgs {
            forms: Some(&forms),
            ..FailArgs::default()
        };
        let line = cast_fail_text(Caster::Pet, 0x56, Some(&d), args, &g).expect("a line");
        assert_eq!(line.key, "ERR_SPELL_FAILED_SHAPESHIFT_FORM_S");
        assert_eq!(line.text, "Must be in Cat Form");
    }

    #[test]
    fn the_pet_speaks_its_own_six_refusals() {
        let m = gs();
        let g = getter(&m);
        let say = |caster, reason| {
            cast_fail_text(caster, reason, Some(&record()), FailArgs::default(), &g).map(|l| l.text)
        };
        for (reason, pet, player) in [
            (0x00, "Your pet is in combat.", "You are in combat"),
            (0x13, "Your pet is dead.", "You are dead"),
            (
                0x33,
                "Your pet must be behind its target.",
                "You must be behind your target",
            ),
            (0x59, "Your pet is out of range.", "Out of range."),
            (
                0x5F,
                "Your pet is unable to move.",
                "You are unable to move",
            ),
            (0x65, "Your pet\'s target is dead.", "Your target is dead"),
        ] {
            assert_eq!(
                say(Caster::Pet, reason).as_deref(),
                Some(pet),
                "pet {reason:#04x}"
            );
            assert_eq!(
                say(Caster::Player, reason).as_deref(),
                Some(player),
                "player {reason:#04x}"
            );
        }
    }

    /// The pet's `0x32` raises `0x151`, `ERR_PET_SPELL_NOPATH`, whose empty format shows nothing;
    /// the player's is a passthrough.
    #[test]
    fn the_pets_nopath_is_silent_because_5875_ships_no_string() {
        let m = gs();
        let g = getter(&m);
        let nopath = cast_fail_text(Caster::Pet, 0x32, Some(&record()), FailArgs::default(), &g)
            .expect("the display runs");
        assert_eq!(nopath.key, "ERR_PET_SPELL_NOPATH");
        assert_eq!(nopath.text, "");
        assert_eq!(
            cast_fail_text(Caster::Player, 0x32, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "No path available"
        );
    }

    #[test]
    fn the_pet_has_no_food_or_potion_cooldown_leg() {
        let m = gs();
        let g = getter(&m);
        let potion = spell(0, 4, 0);
        let ability = spell(0, 0, 0x10);
        let text = |caster, d| {
            cast_fail_text(caster, 0x3C, Some(d), FailArgs::default(), &g)
                .unwrap()
                .text
        };
        assert_eq!(
            text(Caster::Player, &potion),
            "You cannot drink any more yet."
        );
        assert_eq!(text(Caster::Pet, &potion), "Spell is not ready yet.");
        // The ability leg is shared.
        assert_eq!(text(Caster::Player, &ability), "Ability is not ready yet.");
        assert_eq!(text(Caster::Pet, &ability), "Ability is not ready yet.");
    }

    /// `0x41` also changes surface: the player's override is yellow, the pet's passthrough red.
    #[test]
    fn the_pet_does_not_take_the_players_overrides() {
        use benilla_ui::messages::{kind_of, MsgKind};
        let m = gs();
        let g = getter(&m);
        let line = |caster, reason| {
            cast_fail_text(caster, reason, Some(&record()), FailArgs::default(), &g)
                .expect("a line")
        };

        assert_eq!(line(Caster::Player, 0x09).text, "You have no target.");
        assert_eq!(line(Caster::Pet, 0x09).text, "No target");

        let mine = line(Caster::Player, 0x41);
        assert_eq!(mine.key, "ERR_SPELL_FAILED_NOTUNSHEATHED");
        assert_eq!(kind_of(mine.key), MsgKind::Info);
        let its = line(Caster::Pet, 0x41);
        assert_eq!(its.key, PASSTHROUGH);
        assert_eq!(its.text, "You must be unsheathed");
        assert_eq!(kind_of(its.key), MsgKind::Error);
    }

    /// The pet's table `0x8118f0` holds the player's five ids (`0x8118dc`).
    #[test]
    fn the_pets_no_power_pick_is_the_players() {
        let m = gs();
        let g = getter(&m);
        let focus = spell(2, 0, 0);
        for caster in [Caster::Player, Caster::Pet] {
            assert_eq!(
                cast_fail_text(caster, 0x4D, Some(&focus), FailArgs::default(), &g)
                    .unwrap()
                    .text,
                "Not enough focus"
            );
        }
    }

    /// The player's handler hides `0x17` by control flow; the pet's passes it through to a string
    /// 1.12 does not ship.
    #[test]
    fn dont_report_is_silent_on_both_paths_for_two_different_reasons() {
        let m = gs();
        let g = getter(&m);
        assert!(!m.contains_key("SPELL_FAILED_DONT_REPORT"));
        let line = |caster| cast_fail_text(caster, 0x17, Some(&record()), FailArgs::default(), &g);
        assert_eq!(line(Caster::Player), None, "past the display (`0x6e1cfa`)");
        assert_eq!(
            line(Caster::Pet).map(|l| l.text).as_deref(),
            Some(""),
            "an empty format"
        );
    }

    /// `ERR_SPELL_FAILED_NOTUNSHEATHED` (id 320) has kind 1, the yellow `UI_INFO_MESSAGE`.
    #[test]
    fn the_unsheathed_refusal_is_yellow_and_its_neighbours_are_red() {
        use benilla_ui::messages::{kind_of, MsgKind};
        let m = gs();
        let g = getter(&m);

        let line = cast_fail_text(
            Caster::Player,
            0x41,
            Some(&record()),
            FailArgs::default(),
            &g,
        )
        .expect("a line");
        assert_eq!(line.key, "ERR_SPELL_FAILED_NOTUNSHEATHED");
        assert_eq!(kind_of(line.key), MsgKind::Info);

        // A red override, and a passthrough on errorId `0x2c`.
        let red = cast_fail_text(
            Caster::Player,
            0x59,
            Some(&record()),
            FailArgs::default(),
            &g,
        )
        .expect("a line");
        assert_eq!(red.key, "ERR_SPELL_OUT_OF_RANGE");
        assert_eq!(kind_of(red.key), MsgKind::Error);

        let through =
            cast_fail_text(Caster::Player, 0x43, None, FailArgs::default(), &g).expect("a line");
        assert_eq!(through.key, PASSTHROUGH);
        assert_eq!(kind_of(through.key), MsgKind::Error);
    }

    /// Prowl's `Stances = 0x1` names form 1, Maul's `0x90` forms 5 and 8, in mask order.
    #[test]
    fn only_shapeshift_names_the_forms_the_stance_mask_asks_for() {
        let m = gs();
        let g = getter(&m);
        let forms = forms([
            (1, "Cat Form"),
            (5, "Bear Form"),
            (8, "Dire Bear Form"),
            (9, ""),
        ]);
        let text = |stances: u32| {
            let d = SpellDisplay {
                stances,
                ..Default::default()
            };
            cast_fail_text(
                Caster::Player,
                0x56,
                Some(&d),
                FailArgs {
                    forms: Some(&forms),
                    ..FailArgs::default()
                },
                &g,
            )
            .map(|l| l.text)
        };
        assert_eq!(text(0x1).as_deref(), Some("Must be in Cat Form"));
        assert_eq!(
            text(0x90).as_deref(),
            Some("Must be in Bear Form, Dire Bear Form"),
        );
        // An empty name is skipped (`0x6e2058`).
        assert_eq!(
            text(0x90 | 0x100).as_deref(),
            Some("Must be in Bear Form, Dire Bear Form"),
        );
        assert_eq!(text(0x101).as_deref(), Some("Must be in Cat Form"));
    }

    /// `0x56` declines to the epilogue (`0x6e224f`) and shows nothing; other arms show the stem.
    #[test]
    fn an_unfillable_only_shapeshift_says_nothing_at_all() {
        let m = gs();
        let g = getter(&m);
        let forms = forms([(1, "Cat Form"), (9, "")]);
        let line = |spell: Option<&SpellDisplay>, args| {
            cast_fail_text(Caster::Player, 0x56, spell, args, &g)
        };
        let with = FailArgs {
            forms: Some(&forms),
            ..FailArgs::default()
        };
        let empty_mask = SpellDisplay::default();
        let unnamed = SpellDisplay {
            stances: 0x100,
            ..Default::default()
        };
        let cat = SpellDisplay {
            stances: 0x1,
            ..Default::default()
        };
        assert_eq!(line(None, with), None, "no spell record");
        assert_eq!(line(Some(&cat), FailArgs::default()), None, "no DBC loaded");
        assert_eq!(line(Some(&empty_mask), with), None, "an empty stance mask");
        assert_eq!(
            line(Some(&unnamed), with),
            None,
            "a mask naming only unnamed rows"
        );
        // The control: `0x5c` declines to its stripped stem.
        assert_eq!(
            cast_fail_text(Caster::Player, 0x5C, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "Missing reagent"
        );
    }

    /// The pet's arm (`0x6e921a`) breaks at the first named form (`0x6e9282`).
    #[test]
    fn the_pets_only_shapeshift_names_the_first_form_and_never_joins() {
        let m = gs();
        let g = getter(&m);
        let forms = forms([(1, "Cat Form"), (5, "Bear Form"), (8, "Dire Bear Form")]);
        let text = |caster, stances: u32| {
            let d = SpellDisplay {
                stances,
                ..Default::default()
            };
            cast_fail_text(
                caster,
                0x56,
                Some(&d),
                FailArgs {
                    forms: Some(&forms),
                    ..FailArgs::default()
                },
                &g,
            )
            .map(|l| l.text)
        };
        // Maul's mask: the player joins, the pet stops at the first.
        assert_eq!(
            text(Caster::Player, 0x90).as_deref(),
            Some("Must be in Bear Form, Dire Bear Form")
        );
        assert_eq!(
            text(Caster::Pet, 0x90).as_deref(),
            Some("Must be in Bear Form")
        );
        assert_eq!(text(Caster::Player, 0x1), text(Caster::Pet, 0x1));
        assert_eq!(
            text(Caster::Pet, 0x1).as_deref(),
            Some("Must be in Cat Form")
        );
        assert_eq!(text(Caster::Pet, 0x0), None);
    }

    #[test]
    fn the_combat_log_reads_the_argtext_buffer_not_the_displayed_line() {
        let m = gs();
        let g = getter(&m);
        let line = |reason, spell| {
            cast_fail_text(Caster::Player, reason, spell, FailArgs::default(), &g).expect("a line")
        };

        // An override with no arm: the screen and the log differ.
        let plain = spell(0, 0, 0);
        let cooldown = line(0x3C, Some(&plain));
        assert_eq!(cooldown.text, "Spell is not ready yet.");
        assert_eq!(cooldown.logged(), "Not yet recovered");

        let no_target = line(0x09, Some(&plain));
        assert_eq!(no_target.text, "You have no target.");
        assert_eq!(no_target.logged(), "No target");

        let unsheathed = line(0x41, Some(&plain));
        assert_eq!(unsheathed.text, "You have to be unsheathed to do that!");
        assert_eq!(unsheathed.logged(), "You must be unsheathed");

        // No override, no arm: one string.
        let ammo = line(0x43, None);
        assert_eq!(ammo.logged(), ammo.text);

        // `0x4d` blanks the buffer, so the log reads the screen's line.
        let rage = spell(1, 0, 0);
        let power = line(0x4D, Some(&rage));
        assert_eq!(power.text, "Not enough rage");
        assert_eq!(power.logged(), "Not enough rage");

        // A declined arm strips both buffers.
        let reagents = line(0x5C, None);
        assert_eq!(reagents.logged(), "Missing reagent");
        assert!(!reagents.logged().contains('%'));
    }

    /// `0x6e211f` names the active player's own power (`[[player+0x110]+0x7b]`), whatever the
    /// spell's, and leaves past the display and the log on a byte of 5 or more (`0x6e2131`) or
    /// an empty noun (`0x6e2176`).
    #[test]
    fn already_at_full_power_names_the_players_own_power() {
        let mut m = gs();
        m.insert(
            "ERR_SPELL_FAILED_ALREADY_AT_FULL_POWER_S",
            "You are already at full %s.",
        );
        m.insert("MANA", "Mana");
        m.insert("RAGE", "Rage");
        m.insert("ENERGY", "Energy");
        let g = getter(&m);
        // A mana-typed spell, as an item's restore is.
        let mana_spell = spell(0, 0, 0);
        let line = |power| {
            let args = FailArgs {
                power,
                ..FailArgs::default()
            };
            cast_fail_text(Caster::Player, 0x02, Some(&mana_spell), args, &g)
        };
        let rage = line(Some(1)).expect("a line");
        assert_eq!(rage.text, "You are already at full Rage.");
        assert_eq!(rage.logged(), "Rage");
        assert_eq!(
            line(Some(3)).map(|l| l.text).as_deref(),
            Some("You are already at full Energy.")
        );
        assert_eq!(line(Some(5)), None, "a byte past happiness");
        assert_eq!(
            line(Some(2)),
            None,
            "an empty noun: FOCUS has no string here"
        );
        assert_eq!(line(None), None, "no active player");
    }

    /// `DisplayError` formats every id through its key's live global (`0x4967d7`), the default
    /// `0x2c` too, so an addon's `ERR_SPELL_FAILED_S` changes the line and not the log.
    #[test]
    fn the_default_message_formats_through_its_key() {
        let mut m = gs();
        m.insert("ERR_SPELL_FAILED_S", "Failed: %s");
        let g = getter(&m);
        let line = cast_fail_text(
            Caster::Player,
            0x5F,
            Some(&record()),
            FailArgs::default(),
            &g,
        )
        .expect("a line");
        assert_eq!(line.text, "Failed: You are unable to move");
        assert_eq!(line.logged(), "You are unable to move");
    }

    /// An empty format shows nothing (`0x4945b0` drops an empty text), and the log still writes
    /// the text (`0x6e21e2`-`0x6e21fc` run whatever `0x496720` showed).
    #[test]
    fn an_empty_format_shows_nothing_and_still_logs() {
        let mut m = gs();
        m.insert("ERR_SPELL_FAILED_S", "");
        m.remove("ERR_SPELL_OUT_OF_RANGE");
        let g = getter(&m);
        for (reason, logged) in [(0x5F, "You are unable to move"), (0x59, "Out of range")] {
            let line = cast_fail_text(
                Caster::Player,
                reason,
                Some(&record()),
                FailArgs::default(),
                &g,
            )
            .expect("the log's line");
            assert_eq!(line.text, "", "{reason:#04x}");
            assert_eq!(line.logged(), logged, "{reason:#04x}");
        }
    }

    /// The pet's handler leaves with no record (`0x6e8f04`), and when an arm's lookup misses: the
    /// id arms at `0x6e9385`, the subclass arm at `0x6e90d2`. The player's declined arms keep
    /// their template.
    #[test]
    fn the_pet_shows_nothing_without_a_record_or_a_name() {
        let mut m = gs();
        m.insert("SPELL_FAILED_REQUIRES_AREA", "You need to be in %s");
        m.insert("SPELL_FAILED_REQUIRES_SPELL_FOCUS", "Requires %s");
        m.insert(
            "SPELL_FAILED_PREVENTED_BY_MECHANIC",
            "Can't do that while %s",
        );
        let g = getter(&m);
        let say = |caster, reason, spell: Option<&SpellDisplay>| {
            cast_fail_text(caster, reason, spell, FailArgs::default(), &g).map(|l| l.text)
        };
        assert_eq!(say(Caster::Pet, 0x59, None), None, "no record");
        for reason in [0x19, 0x1A, 0x1B, 0x5D, 0x5E, 0x8D] {
            assert_eq!(
                say(Caster::Pet, reason, Some(&record())),
                None,
                "pet {reason:#04x}"
            );
        }
        assert_eq!(
            say(Caster::Player, 0x5D, Some(&record())).as_deref(),
            Some("You need to be in")
        );
        assert_eq!(
            say(Caster::Player, 0x19, Some(&record())).as_deref(),
            Some("Must have a equipped")
        );
    }

    fn forms<const N: usize>(
        rows: [(u32, &str); N],
    ) -> HashMap<u32, benilla_formats::ShapeshiftForm> {
        rows.into_iter()
            .map(|(id, name)| {
                (
                    id,
                    benilla_formats::ShapeshiftForm {
                        name: name.to_string(),
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    fn getter<'a>(
        map: &'a HashMap<&'static str, &'static str>,
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| map.get(k).map(|s| (*s).to_string())
    }

    fn spell(power_type: u32, category: u32, attributes: u32) -> SpellDisplay {
        SpellDisplay {
            power_type,
            category,
            attributes,
            ..Default::default()
        }
    }

    /// The `0x6e23e0` table's anchors.
    #[test]
    fn the_key_table_holds_the_verified_anchors() {
        assert_eq!(CAST_FAIL_KEYS.len(), 146);
        assert_eq!(CAST_FAIL_KEYS[0x43], "SPELL_FAILED_NO_AMMO");
        assert_eq!(CAST_FAIL_KEYS[0x59], "SPELL_FAILED_OUT_OF_RANGE");
        assert_eq!(CAST_FAIL_KEYS[0x76], "SPELL_FAILED_TOO_CLOSE");
    }

    #[test]
    fn overrides_replace_and_passthrough_reads() {
        let m = gs();
        let g = getter(&m);
        assert_eq!(
            cast_fail_text(Caster::Player, 0x43, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "Out of ammo"
        );
        assert_eq!(
            cast_fail_text(
                Caster::Player,
                0x59,
                Some(&record()),
                FailArgs::default(),
                &g
            )
            .unwrap()
            .text,
            "Out of range."
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x76, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "Target too close"
        );
        assert_eq!(
            cast_fail_text(
                Caster::Player,
                0x09,
                Some(&record()),
                FailArgs::default(),
                &g
            )
            .unwrap()
            .text,
            "You have no target."
        );
        // 0x3c: a plain spell, an ability (`Attributes & 0x10`), a potion (category 4).
        let plain = spell(0, 0, 0);
        assert_eq!(
            cast_fail_text(Caster::Player, 0x3C, Some(&plain), FailArgs::default(), &g)
                .unwrap()
                .text,
            "Spell is not ready yet."
        );
        let ability = spell(1, 0, 0x10);
        assert_eq!(
            cast_fail_text(
                Caster::Player,
                0x3C,
                Some(&ability),
                FailArgs::default(),
                &g
            )
            .unwrap()
            .text,
            "Ability is not ready yet."
        );
        let potion = spell(0, 4, 0);
        assert_eq!(
            cast_fail_text(Caster::Player, 0x3C, Some(&potion), FailArgs::default(), &g)
                .unwrap()
                .text,
            "You cannot drink any more yet."
        );
    }

    #[test]
    fn no_power_reads_the_spells_power_family() {
        let m = gs();
        let g = getter(&m);
        let rage = spell(1, 0, 0);
        assert_eq!(
            cast_fail_text(Caster::Player, 0x4D, Some(&rage), FailArgs::default(), &g)
                .unwrap()
                .text,
            "Not enough rage"
        );
        let mana = spell(0, 0, 0);
        assert_eq!(
            cast_fail_text(Caster::Player, 0x4D, Some(&mana), FailArgs::default(), &g)
                .unwrap()
                .text,
            "Not enough mana"
        );
    }

    /// `0x17` is hidden by control flow and `0x08` by an unshipped key; `0x92` is off the table.
    #[test]
    fn suppression_hex_fallback_and_template_strip() {
        let m = gs();
        let g = getter(&m);
        assert_eq!(
            cast_fail_text(Caster::Player, 0x17, None, FailArgs::default(), &g),
            None
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x08, None, FailArgs::default(), &g)
                .map(|l| l.text)
                .as_deref(),
            Some("")
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x92, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "Spell failed (0x92)"
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x5C, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "Missing reagent"
        );
    }

    /// The shipped `GlobalStrings.lua` run in a real VM and read by the drain's lookup.
    #[test]
    fn the_real_boot_resolves_the_real_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        assert_eq!(
            cast_fail_text(Caster::Player, 0x43, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "Out of ammo"
        );
        assert_eq!(
            cast_fail_text(
                Caster::Player,
                0x59,
                Some(&record()),
                FailArgs::default(),
                &g
            )
            .unwrap()
            .text,
            "Out of range."
        );
        assert_eq!(
            cast_fail_text(
                Caster::Player,
                0x3C,
                Some(&record()),
                FailArgs::default(),
                &g
            )
            .unwrap()
            .text,
            "Spell is not ready yet."
        );
        let rage = spell(1, 0, 0);
        assert_eq!(
            cast_fail_text(Caster::Player, 0x4D, Some(&rage), FailArgs::default(), &g)
                .unwrap()
                .text,
            "Not enough rage"
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x50, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "Cannot use while swimming"
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x58, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "Can only use while swimming"
        );
        // Keys 1.12 does not ship show nothing.
        for unshipped in [0x08, 0x21] {
            assert_eq!(
                cast_fail_text(Caster::Player, unshipped, None, FailArgs::default(), &g)
                    .map(|l| l.text)
                    .as_deref(),
                Some(""),
                "{unshipped:#04x}"
            );
        }

        // The pet's six and its unshipped NOPATH key, against the shipped file.
        for (reason, pet, player) in [
            (0x00, "Your pet is in combat.", "You are in combat"),
            (0x13, "Your pet is dead.", "You are dead"),
            (
                0x33,
                "Your pet must be behind its target.",
                "You must be behind your target",
            ),
            (0x59, "Your pet is out of range.", "Out of range."),
            (
                0x5F,
                "Your pet is unable to move.",
                "You are unable to move",
            ),
            (0x65, "Your pet\'s target is dead.", "Your target is dead"),
        ] {
            assert_eq!(
                cast_fail_text(
                    Caster::Pet,
                    reason,
                    Some(&record()),
                    FailArgs::default(),
                    &g
                )
                .map(|l| l.text)
                .as_deref(),
                Some(pet),
                "pet {reason:#04x}"
            );
            assert_eq!(
                cast_fail_text(Caster::Pet, reason, None, FailArgs::default(), &g),
                None,
                "pet {reason:#04x} with no record (`0x6e8f04`)"
            );
            assert_eq!(
                cast_fail_text(
                    Caster::Player,
                    reason,
                    Some(&record()),
                    FailArgs::default(),
                    &g
                )
                .map(|l| l.text)
                .as_deref(),
                Some(player),
                "player {reason:#04x}"
            );
        }
        assert_eq!(
            cast_fail_text(Caster::Pet, 0x32, Some(&record()), FailArgs::default(), &g)
                .map(|l| l.text)
                .as_deref(),
            Some(""),
            "5875 ships no ERR_PET_SPELL_NOPATH, so the reference shows nothing"
        );
        assert!(
            g("PET_SPELL_NOPATH").is_some(),
            "and the near-miss key that made the old map look right IS shipped — which is the \
             whole trap"
        );

        // The argument arms against the real DBCs and templates.
        let focus =
            benilla_formats::load_spell_focus_catalog(&mut chain).expect("SpellFocusObject");
        let areas = benilla_formats::load_area_table_catalog(&mut chain).expect("AreaTable");
        let mechanics =
            benilla_formats::load_spell_mechanic_catalog(&mut chain).expect("SpellMechanic");
        let forms =
            benilla_formats::load_shapeshift_forms(&mut chain).expect("SpellShapeshiftForm.dbc");
        let args = |arg: u32| FailArgs {
            arg: Some(arg),
            focus: Some(&focus),
            areas: Some(&areas),
            mechanics: Some(&mechanics),
            forms: Some(&forms),
            ..FailArgs::default()
        };
        assert_eq!(
            cast_fail_text(Caster::Player, 0x8D, None, args(12), &g)
                .unwrap()
                .text,
            "Can't do that while stunned"
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x8D, None, args(5), &g)
                .unwrap()
                .text,
            "Can't do that while fleeing"
        );
        // An unknown mechanic strips to the stem.
        assert!(!cast_fail_text(Caster::Player, 0x8D, None, args(999), &g)
            .unwrap()
            .text
            .contains('%'));

        assert_eq!(
            cast_fail_text(Caster::Player, 0x5E, None, args(12), &g)
                .unwrap()
                .text,
            "Requires Starbreeze Village Moonwell"
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x5E, None, args(1), &g)
                .unwrap()
                .text,
            "Requires Anvil"
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x5D, None, args(1657), &g)
                .unwrap()
                .text,
            "You need to be in Darnassus"
        );
        // An unnamed id and an absent word both decline to the stem.
        assert_eq!(
            cast_fail_text(Caster::Player, 0x5E, None, args(999_999), &g)
                .unwrap()
                .text,
            "Requires"
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x5D, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "You need to be in"
        );
        // `0x56` on the real `Spell.dbc`; Maul's two forms tell a join from naming the lowest bit.
        let catalog = benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc");
        let shifted = |spell_id: u32| {
            let d = catalog.get(spell_id).expect("a shipped spell");
            cast_fail_text(
                Caster::Player,
                0x56,
                Some(d),
                FailArgs {
                    forms: Some(&forms),
                    ..FailArgs::default()
                },
                &g,
            )
            .map(|l| l.text)
        };
        // Prowl's three ranks, `Stances = 0x1`.
        for prowl in [5215, 6783, 9913] {
            assert_eq!(
                shifted(prowl).as_deref(),
                Some("Must be in Cat Form"),
                "Prowl {prowl}"
            );
        }
        assert_eq!(
            shifted(6807).as_deref(),
            Some("Must be in Bear Form, Dire Bear Form"),
            "Maul's 0x90 names both bear rows, in mask order"
        );
        assert_eq!(
            shifted(1715).as_deref(),
            Some("Must be in Battle Stance, Berserker Stance"),
            "Hamstring's 0x50000 skips the Defensive row between them"
        );
        // A spell with no stance requirement reaches the epilogue, not the stem.
        assert_eq!(
            shifted(22812),
            None,
            "Barkskin carries an empty Stances mask"
        );

        // With no word, `0x5e` falls back to the spell's focus; spell 4976, the Crystal Phial's
        // "Filling", has focus 11.
        let filling = SpellDisplay {
            requires_spell_focus: 11,
            ..Default::default()
        };
        assert_eq!(
            cast_fail_text(
                Caster::Player,
                0x5E,
                Some(&filling),
                FailArgs {
                    focus: Some(&focus),
                    ..FailArgs::default()
                },
                &g
            )
            .unwrap()
            .text,
            "Requires Shadowglen Moonwell"
        );
        // The pet's arm reads the word, 0 on a short packet (`0x6e9161`), and never the spell.
        // 5875's file has no row 0, so the map's slot is null and the line silent (`0x6e9385`).
        assert_eq!(focus.name(0), None, "SpellFocusObject.dbc has no row 0");
        let pet = |args: FailArgs| {
            cast_fail_text(Caster::Pet, 0x5E, Some(&filling), args, &g).map(|l| l.text)
        };
        assert_eq!(
            pet(FailArgs {
                focus: Some(&focus),
                ..FailArgs::default()
            }),
            None,
            "the pet's short packet names no focus"
        );
        assert_eq!(pet(args(1)).as_deref(), Some("Requires Anvil"));

        // The chain and the `_PET` flag on shipped rows and strings.
        let said = |reason: u8, spell_id: u32| {
            let d = catalog.get(spell_id).expect("a shipped spell");
            cast_fail_text(Caster::Player, reason, Some(d), FailArgs::default(), &g)
                .map(|l| (l.key, l.text))
                .expect("a line")
        };
        // Cower 1747 is `LEARN_SPELL` at the pet, Fire Shield 2949 `LEARN_PET_SPELL`.
        assert_eq!(
            said(0x63, 1747).1,
            "That ability is not available to your pet"
        );
        assert_eq!(said(0x29, 2949).1, "Your pet is not high enough level");
        // Enchant Bracer - Minor Health 7418 and Disenchant 13262 target items (`Targets` 0x10);
        // Pick Lock 1804 has `Targets` 0 and keeps the plain line.
        for item_spell in [7418, 13262] {
            assert_eq!(
                said(0x0A, item_spell),
                (
                    "ERR_INVALID_ITEM_TARGET",
                    "That item is not a valid target.".to_string()
                ),
                "spell {item_spell}"
            );
        }
        assert_eq!(
            said(0x0A, 1804),
            (PASSTHROUGH, "Invalid target".to_string())
        );
        // Auto Shot 75 out of arrows.
        assert_eq!(
            said(0x2F, 75),
            (
                "ERR_NOAMMO_S",
                "Ammo needs to be in the paper doll ammo slot before it can be fired".to_string()
            )
        );
    }
}
