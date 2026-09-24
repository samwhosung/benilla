//! Cast-failure display strings — the reference's **two-layer** pipeline (decision 0427):
//!
//! 1. `HandleCastFailed 0x6e1a00` resolves the wire reason through the name-identity table
//!    `0x6e23e0` (wire order = the vmangos `SpellCastResult` enum) into
//!    `GetText("SPELL_FAILED_<name>")` — [`CAST_FAIL_KEYS`].
//! 2. A per-reason **errorId** (default `0x2c` `ERR_SPELL_FAILED_S` = `"%s"`, a pure
//!    passthrough) is handed to `CGGameUI::DisplayError 0x496720`; ~12 reasons override it,
//!    REPLACING the message entirely (their `ERR_*` string has no `%s`). This is why the
//!    screen shows "Spell is not ready yet." while `SPELL_FAILED_NOT_READY` reads
//!    "Not yet recovered", and "Not enough rage" for a rage spell's NO_POWER.
//!
//!    **This layer — and only this layer — is per-CASTER** (decision 2033). `SMSG_PET_CAST_FAILED`
//!    is not the same handler with a flag: it is `Spell_C::HandlePetCastFailed 0x6e8eb0`, a
//!    separate switch over a separate 142-byte index table, and it overrides a different ten
//!    reasons. Six of them are the `ERR_PET_SPELL_*` catalog rows, which have no other raise site
//!    in the client — "Your pet is out of range." where the player reads "Out of range." —
//!    and seven of the player's overrides are simply absent there. [`Caster`] picks the table;
//!    layers 1 and 3 are shared code in the binary and shared code here.
//!
//! 3. A per-reason **argument arm** (the second dispatch `0x6e1d8e`, 13 targets) fills that
//!    message's own `%s`/`%d` from a DBC or item name before it is displayed. The two arms whose
//!    tables benilla already loads live in [`FailArgs::fill`]; the drain fills three more before
//!    calling here (below), and the rest strip.
//!
//! Strings are never hardcoded here: every message resolves from the VM's loaded
//! `GlobalStrings.lua` by key, so localization rides for free. Suppression is faithful on
//! both mechanisms: reason `0x17` (DONT_REPORT) never reaches display (control-flow), and a
//! key absent from GlobalStrings displays as nothing (data — 0x08/0x21/0x75, happiness
//! NO_POWER).
//!
//! **The argument arms, and what is still approximate.** Filled: `0x5e` REQUIRES_SPELL_FOCUS and
//! `0x5d` REQUIRES_AREA here (decision 1313 — `0x6e1f62`/`0x6e1fad`), `0x8d`
//! PREVENTED_BY_MECHANIC (decision 1948 — `0x6e2190`, and the one arm whose word is produced
//! locally rather than read off the wire), `0x56` ONLY_SHAPESHIFT (decision 2280 — `0x6e1ff8`,
//! the one arm that reads a **mask** and joins several names), and `0x78` TOTEMS / `0x5c`
//! REAGENTS / `0x19`–`0x1b` EQUIPPED_ITEM_CLASS\* / `0x31` NEED_EXOTIC_AMMO in the drain, which
//! owns the totem/reagent pair because their fills need the item caches and the
//! query-then-redisplay cache-miss behavior ("Requires Mining Pick", decisions 0545 + 0552), and
//! the other two because they share `0x6e2380`'s subclass catalog.
//!
//! **That is every arm in the `0x6e1d8e` table, and the reasons that still read bare read bare
//! for reasons that are not "a DBC we do not load"** — decision 2292, which found that claim
//! false in all three places this paragraph used to make it — the same stale blocker that hid
//! `0x56` behind a DBC we had been loading for a month:
//!
//! - `0x30` NEED_AMMO_POUCH — **the reference leaves its own `%s` literal.** There is no arm to
//!   write: `0x6e1e3d` only clears `[caster+0xd58]` bit `0x200`, and no word is ever supplied.
//! - `0x31` NEED_EXOTIC_AMMO — modeled, and argless on this stack. `Spell::SendCastResult` fills
//!   `failureArg1` for four reasons and this is not one of them, so the arm declines.
//! - `0x84` PROSPECT_NEED_MORE and `0x90` MIN_SKILL — **unreachable on 5875, not
//!   unimplemented.** Neither is server-sent, and their only raise sites image-wide (`0x49614e`
//!   and `0x496128`, both inside the item-target validator `0x495d60`) sit behind that
//!   function's third leg, `Effect[i] == 0x7f` SPELL_EFFECT_PROSPECTING — which no shipped spell
//!   carries, pinned on the real file by `real_prospecting_effect_is_absent_from_5875`.
//!   `SkillLine.dbc`, the DBC `0x90` was said to be waiting on, has been loaded since the
//!   spellbook grew tabs.
//!
//! Stripping is a **deliberate divergence**, now byte-confirmed as one: on a
//! bad id or an absent word the reference jumps to the default arm with the pointer still on the
//! *unfilled* template, so it displays a literal `Requires %s` (the fill path's own buffer swap
//! `0x6e21d2` sits after the printf and is skipped). We show the bare stem instead:
//! "Requires" reads as terse, "Requires %s" reads as broken software (§7 — judge by the result).
//! **`0x56` is the exception, and it is the reference's own** ([`Fill::Nothing`]): its three
//! failure exits land on the function epilogue, not on that default arm, so an unfillable
//! ONLY_SHAPESHIFT displays nothing at all.
//! `0x0a`'s item-spell leg (`ERR_INVALID_ITEM_TARGET`) is unmodeled — the drain does not know
//! item-ness.
//!
//! 4. **A fourth thing leaves this function, and it is not the displayed text**
//!    ([`CastFailLine::arg_text`], decision 2285). `0x6e1a00` also calls the combat-log formatter
//!    `0x62c360`, and hands it `edi` — the **argText** buffer, which layer 2 never touches. So
//!    the eighteen re-worded reasons say one thing on screen and another in the log: "Spell is
//!    not ready yet." against "Not yet recovered". Modelling the log as "whatever the red line
//!    said" is wrong on seven rows; re-deriving it from the key table is wrong on the filled
//!    arms. Both buffers come out of the one resolution here.

use std::collections::HashMap;

use benilla_formats::{
    AreaTableCatalog, ShapeshiftForm, SpellDisplay, SpellFocusCatalog, SpellMechanicCatalog,
};

use super::Caster;

/// Wire reason → its `SPELL_FAILED_*` GlobalStrings key (the `0x6e23e0` table, byte-exact).
pub(super) const CAST_FAIL_KEYS: [&str; 146] = [
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

/// Vanilla power types (`SpellRec+0x7c`): the NO_POWER pick table `0x8118dc` and the
/// full-power `%s` fill. Health is the wire's -2.
fn power_keys(power_type: u32) -> (&'static str, &'static str) {
    match power_type {
        1 => ("ERR_OUT_OF_RAGE", "RAGE"),
        2 => ("ERR_OUT_OF_FOCUS", "FOCUS"),
        3 => ("ERR_OUT_OF_ENERGY", "ENERGY"),
        4 => ("ERR_NOT_HAPPY_ENOUGH", "HAPPINESS"),
        0xFFFFFFFE => ("ERR_OUT_OF_HEALTH", "HEALTH"),
        _ => ("ERR_OUT_OF_MANA", "MANA"),
    }
}

/// The potion/food category test (`SpellRec+0x8`) the 0x28/0x3c errorId picks key on.
fn is_potion(spell: Option<&SpellDisplay>) -> bool {
    spell.is_some_and(|d| matches!(d.category, 4 | 9))
}
fn is_food(spell: Option<&SpellDisplay>) -> bool {
    spell.is_some_and(|d| matches!(d.category, 0xA | 0xB))
}

/// The argument arms' inputs: the wire's reason-specific word ([`super::CastFail::arg`]) and the
/// DBC name tables the arms read. Both catalogs are `Option` because a client without game data
/// has neither — the arm then declines and the template strips, exactly as an unmodeled arm does.
#[derive(Default, Clone, Copy)]
pub(super) struct FailArgs<'a> {
    pub(super) arg: Option<u32>,
    /// `SpellFocusObject.dbc` (`0xc0d800`) — the `0x5e` arm's names ("Anvil", "Forge", and the
    /// Teldrassil moonwells the Crown of the Earth phials name).
    pub(super) focus: Option<&'a SpellFocusCatalog>,
    /// `AreaTable.dbc` (`0xc0e048`) — the `0x5d` arm's `AreaName`.
    pub(super) areas: Option<&'a AreaTableCatalog>,
    /// `SpellMechanic.dbc` (`0xc0d7c4`) — the `0x8d` arm's mechanic name (decision 1948).
    pub(super) mechanics: Option<&'a SpellMechanicCatalog>,
    /// `SpellShapeshiftForm.dbc` (`0xc0d76c`) — the `0x56` arm's form names, keyed by form id
    /// exactly as [`crate::ui_action::Spells::forms`] holds them (decision 2280).
    pub(super) forms: Option<&'a HashMap<u32, ShapeshiftForm>>,
}

/// What an argument arm produced — **and, when it produced nothing, which exit the reference's
/// own arm takes**, because the two unfilled outcomes are not the same outcome.
///
/// Every other arm in the `0x6e1d8e` table declines through a bounds test that jumps to the
/// shared default `0x6e21d8`, where `edi` still points at the *unfilled* `SPELL_FAILED_*`
/// template and the line is displayed anyway ([`Self::Template`] — benilla strips its tokens
/// rather than showing a raw `%s`). `0x56` alone jumps, from all three of its failure exits, to
/// the function **epilogue** `0x6e224f`
/// ([`Self::Nothing`]), past `DisplayError`, past the error sound and past the cast abort
/// `0x6e4940` alike. That is not a special case anyone chose: the `0x56` arm spends `edi` as its
/// own loop counter, so it has no text pointer left to hand the default arm.
pub(super) enum Fill {
    /// The arm resolved its word — the template's `%s` takes it.
    Filled(String),
    /// The arm declined (or benilla does not model it) and the reference still displays the
    /// template: the shared default `0x6e21d8`.
    Template,
    /// The arm declined and the reference displays **nothing**: the epilogue `0x6e224f`.
    Nothing,
}

impl FailArgs<'_> {
    /// The `%s` fill for the argument-formatted reasons this module owns, or `None` to leave the
    /// template to the strip fallback.
    ///
    /// Each of the two arms reads the **wire's** first argument word — `[ebx+8]`, the handler's own
    /// stack slot, never a re-read of `Spell.dbc` — indexes its DBC store, and `SStrPrintf`s the
    /// reason's `SPELL_FAILED_*` template. The errorId stays the default `0x2c` (`"%s"`) for both,
    /// so what the player reads IS the filled template. A single-`%s` template is what makes a
    /// plain `replace` faithful here; the reference runs a real two-pass printf, which would
    /// matter for a multi-specifier format.
    ///
    /// **`0x5e` REQUIRES_SPELL_FOCUS** (`0x6e1f62`) → `SpellFocusObject.dbc` (`0xc0d800`)
    /// `Name_Lang` at `row + 0x4 + locale*4`, so `"Requires %s"` reads "Requires Starbreeze
    /// Village Moonwell". The failing spell's own `RequiresSpellFocus` column holds the same
    /// number vmangos copied onto the wire, so we fall back to it when the word is absent — a
    /// benilla-side robustness margin, not a transcription: the client itself never reads
    /// `SpellRec+0x3c` on this path.
    ///
    /// **`0x5d` REQUIRES_AREA** (`0x6e1fad`) → `AreaTable.dbc` (`0xc0e048`) `AreaName_Lang` at
    /// `row + 0x2c + locale*4`, so `"You need to be in %s"` names the zone. This one has **no**
    /// client-side stand-in: the server derives the id from its own `spell_area` rows and nothing
    /// in `Spell.dbc` holds it, so an absent word means an unfilled message.
    fn fill(&self, caster: Caster, reason: u8, spell: Option<&SpellDisplay>) -> Fill {
        let named = |name: Option<&str>| match name {
            Some(name) => Fill::Filled(name.to_string()),
            None => Fill::Template,
        };
        match reason {
            // **`0x8d` PREVENTED_BY_MECHANIC** (`0x6e2190`) → `SpellMechanic.dbc` (`0xc0d7c4`),
            // so "Can't do that while %s" reads "Can't do that while stunned". Unlike its two
            // neighbours the word is not the wire's: this refusal is raised locally and the id
            // comes from the crowd-control ladder's own exemption scan (decisions 1941/1948).
            // A `0` or unknown id leaves the template to the strip fallback, which is also what
            // the arm does when the scan named no mechanic at all.
            0x8D => named(self.arg.and_then(|id| self.mechanics?.name(id))),
            0x5D => named(self.arg.and_then(|id| self.areas?.name(id))),
            0x5E => {
                let id = self
                    .arg
                    .filter(|&id| id != 0)
                    .or_else(|| spell.map(|d| d.requires_spell_focus).filter(|&id| id != 0));
                named(id.and_then(|id| self.focus?.name(id)))
            }
            0x56 => self.shapeshift_forms(caster, spell),
            _ => Fill::Template,
        }
    }

    /// **`0x56` ONLY_SHAPESHIFT** (`0x6e1ff8`) → `SpellShapeshiftForm.dbc` (`0xc0d76c`), so
    /// `"Must be in %s"` reads "Must be in Cat Form" — the line a bear-form druid gets for
    /// pressing Prowl. The one arm that reads a **mask** and can name several forms.
    ///
    /// Byte-exact, and unlike its neighbours it reads **no wire word at all**: `6e2021` re-reads
    /// the failing spell's own record, `6e202d: test [eax+0x2c],edx` walks `SpellRec+0x2c`
    /// (`Stances`, field 11) bit by bit, and `6e2050: mov eax,[esi+edx*4+0x8]` takes each row's
    /// `Name` at `row + 0x8 + locale*4`. The walk is over the **record array** (`6e20b3: add
    /// esi,0x38`, stride 56 = one row) with the bit index as the row index, never through the
    /// id-lookup table — and 5875's file is 32 rows carrying ids 1..32 in file order (verified on
    /// the extracted DBC), so row index `b` IS form id `b+1`. That is the same "bit b = form id
    /// b+1" law the spell tooltip's own required-form line already runs on, which is why the
    /// tooltip could read "Requires Cat Form" while the refusal read "Must be in".
    ///
    /// Joined with `", "` (`0x84480c`, `SStrCat 0x64a760`) — the reference's own separator, and
    /// again the tooltip line's. A row whose `Name` is NULL or **empty** is skipped
    /// (`6e2058: cmp BYTE PTR [eax],0x0`); 5875 ships fourteen such rows, so this is live data,
    /// not a guard against nothing.
    ///
    /// Two earned negatives. The arm never tests `AttributesEx2` bit `0x80000` — the *permissive*
    /// mask attribute the tooltip's required-form line does honour (`0x52f115`, decision 1483) —
    /// so a permissive spell that somehow reached this reason would still name its forms; the
    /// form gate, not this arm, is what keeps that unreachable. And `StancesNot` (`+0x30`) is
    /// never read here either.
    ///
    /// **Nothing to say is said as nothing** ([`Fill::Nothing`]): an empty mask, an unnamed set
    /// of forms, no spell record (`6e1ffd`), or an unloaded DBC (`6e2016`, the `[0xc0d770] == 0`
    /// test) all leave the "nothing appended" flag set at `6e20c1` and jump to the epilogue.
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
            // **The pet's arm does not join** (decision 2285). `0x6e921a` reads the same store
            // the same way — `[0xc0d76c]` at `0x6e924f`, stride `0x38` at `0x6e9285`,
            // `row+0x8+locale*4` at `0x6e9277`, the mask at `0x6e924c` — and then
            // `0x6e9282: jne 0x6e929a` leaves the loop at the FIRST non-empty name. It is a
            // break, not a continue: over `[0x6e8eb0, 0x6e93d0)` there is not one `SStrCat`
            // (`0x64a760`) call and not one reference to the separator `0x84480c`. So a
            // two-form spell the player reads as "Bear Form, Dire Bear Form" the pet reads as
            // "Bear Form".
            Caster::Pet => named.next().map(str::to_string),
            Caster::Player => {
                let all: Vec<&str> = named.collect();
                (!all.is_empty()).then(|| all.join(", "))
            }
        }
        .map_or(Fill::Nothing, Fill::Filled)
    }
}

/// The errorId every reason that overrides nothing falls through to — `0x2c`, whose text is the
/// bare `"%s"`, so what the player reads is the first layer's `SPELL_FAILED_*` string. benilla
/// short-circuits the identity substitution and displays that string directly; the id still
/// matters, because it is the id whose **record** decides the surface.
pub(super) const PASSTHROUGH: &str = "ERR_SPELL_FAILED_S";

/// One resolved cast-failure line: the text to show, and the **message id's key** that decides
/// where to show it.
///
/// The key rides along because the reference's second layer hands its errorId to
/// `CGGameUI::DisplayError`, and that function reads the record's kind — so the surface is a
/// property of the id this dispatch picked, not of "cast failures" as a category. Seventeen of
/// the overrides are red and `ERR_SPELL_FAILED_NOTUNSHEATHED` is **yellow**; before the catalog
/// (decision 1770) benilla painted all eighteen red, because nothing here knew which id it had
/// chosen by the time the line reached the frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CastFailLine {
    pub key: &'static str,
    pub text: String,
    /// **The reference's OTHER buffer** — `edi` at `0x6e21e2`, which is what the combat-log
    /// formatter `0x62c360` receives, not the displayed text (decision 2285).
    ///
    /// `0x6e1d9a: lea edi,[ebp-0x488]` runs **once, before the argument dispatch**, on the buffer
    /// `0x6e1d77` has just filled with `GetText("SPELL_FAILED_<name>")` — and the errorId-override
    /// chain (`0x6e1aab`–`0x6e1c5f`) writes only `[ebp-8]`, never `edi`. So an override changes
    /// what the SCREEN says and leaves the log on the first-layer string: a cooldown refusal
    /// toasts "Spell is not ready yet." and logs "Not yet recovered". Only `0x6e21d2` re-points
    /// `edi`, at `[ebp-0x888]`, which just three arms write — a filled argument arm (the printf
    /// output, so log == toast), `0x02` (the bare power noun), and `0x4d` (blanked).
    ///
    /// Empty means "read the last-error buffer `0xb4da40` instead" — see [`Self::logged`].
    pub arg_text: String,
}

impl CastFailLine {
    /// A line the second layer left on its default errorId — the drain builds these for the three
    /// argument arms it owns (`0x78` TOTEMS, `0x5c` REAGENTS, `0x19`-`0x1b` EQUIPPED_ITEM_CLASS*),
    /// whose fills need the item caches. A filled arm's two buffers hold the same printf output.
    pub(super) fn passthrough(text: String) -> Self {
        Self {
            key: PASSTHROUGH,
            arg_text: text.clone(),
            text,
        }
    }

    /// What `0x62c360` prints as the reason — `0x6e21e2`'s one-byte test: the argText buffer when
    /// it is non-empty, else `0x4968a0`'s global last-error cell `0xb4da40`, which `DisplayError`
    /// has just filled with the text it displayed (`0x496802`).
    pub(super) fn logged(&self) -> &str {
        if self.arg_text.is_empty() {
            &self.text
        } else {
            &self.arg_text
        }
    }

    /// The `0x4d` arm's `mov BYTE PTR [ebp-0x888],0` (`0x6e20fd` / `0x6e2113`): the argText
    /// buffer is emptied and `edi` re-pointed at it, so the combat log falls through to the
    /// last-error cell and reads what the screen reads. The one override whose two surfaces
    /// agree, and it agrees by being blanked rather than by being equal. (Moot on the pet's
    /// path, which never reaches a log formatter at all — it is applied there only so the two
    /// casters' lines carry the same shape.)
    fn arm_blanked(self) -> Self {
        Self {
            arg_text: String::new(),
            ..self
        }
    }

    fn fill(self, name: &str) -> Self {
        Self {
            text: self.text.replace("%s", name),
            ..self
        }
    }
}

/// The displayed line for a failed cast — `None` = the reference shows nothing. `get` is the
/// VM's GlobalStrings lookup (an absent or empty key resolves to `None`, the data-suppression
/// face). Reasons beyond the table print their code — our debug affordance, not a ref string.
pub(super) fn cast_fail_text(
    caster: Caster,
    reason: u8,
    spell: Option<&SpellDisplay>,
    args: FailArgs<'_>,
    get: &dyn Fn(&str) -> Option<String>,
) -> Option<CastFailLine> {
    let get_text = |key: &str| get(key).filter(|s| !s.is_empty());
    // `[ebp-0x488]` — the FIRST-layer string, `SStrCopy`d at `0x6e1d77` and pointed at by `edi`
    // at `0x6e1d9a`, both of which run before the argument dispatch and neither of which any
    // errorId override touches. It is therefore what the combat log reads for every reason no
    // argument arm claims, *including* the eighteen the second layer re-words on screen
    // ([`CastFailLine::arg_text`], decision 2285). Empty when 5875 ships no such key.
    let first_layer = CAST_FAIL_KEYS
        .get(usize::from(reason))
        .and_then(|k| get_text(k))
        .map(|t| strip_tokens(&t))
        .unwrap_or_default();
    let get_display = |key: &'static str| {
        get_text(key).map(|text| CastFailLine {
            key,
            text,
            arg_text: first_layer.clone(),
        })
    };
    // The errorId overrides — the replaced-message reasons. **Which table is asked is the
    // caster's** (decision 2033): the reference does not flag one handler, it ships two, and they
    // disagree on ten reasons. Everything past this match — the `SPELL_FAILED_*` vocabulary, the
    // argument arms, the strip fallback — is shared, exactly as it is in the binary.
    match caster {
        // `Spell_C::HandlePetCastFailed 0x6e8eb0`: `cmp reason,0x8d; ja default`, then a 142-byte
        // index table (`0x6e93d0`) over 15 jump targets (`0x6e9394`). Six of the ten overrides are
        // the `ERR_PET_SPELL_*` catalog rows, which exist for this handler and nothing else —
        // "Your pet is dead." where the player reads "You are dead".
        Caster::Pet => match reason {
            0x00 => return get_display("ERR_PET_SPELL_AFFECTING_COMBAT"), // 0x14c @0x6e8fab
            0x13 => return get_display("ERR_PET_SPELL_DEAD"),             // 0x150 @0x6e9017
            0x32 => return get_display("ERR_PET_SPELL_NOPATH"),           // 0x151 @0x6e9032
            0x33 => return get_display("ERR_PET_SPELL_NOT_BEHIND"),       // 0x14e @0x6e8fe1
            0x59 => return get_display("ERR_PET_SPELL_OUT_OF_RANGE"),     // 0x14d @0x6e8fc6
            0x5F => return get_display("ERR_PET_SPELL_ROOTED"),           // 0x14b @0x6e8f4f
            0x65 => return get_display("ERR_PET_SPELL_TARGETS_DEAD"),     // 0x14f @0x6e8ffc
            // `0x6e8f2a` computes its id: `((SpellRec+0x18 & 0x10) | 0x300) >> 4`, which is only
            // ever `0x31` or `0x30`. The player's four-way (`0x6e1aab`) tests the spell CATEGORY
            // first for food and potion; the pet's does not test it at all, because a pet eats
            // and drinks nothing.
            0x3C => {
                return get_display(if spell.is_some_and(|d| d.attributes & 0x10 != 0) {
                    "ERR_ABILITY_COOLDOWN"
                } else {
                    "ERR_SPELL_COOLDOWN"
                });
            }
            // `0x6e8f6a`: health (`SpellRec+0x7c` == `-2`) takes `0x123`, everything else indexes
            // `[0x8118f0 + 4*power]`. Those five dwords are byte-identical to the player's table
            // at `0x8118dc` (`0x11f 0x120 0x121 0x122 0x168`), so this is the SAME pick, not a
            // pet-specific one — which is why [`power_keys`] serves both and a hunter pet's focus
            // ability reads "Not enough focus" either way.
            0x4D => {
                let power = spell.map_or(0, |d| d.power_type);
                return get_display(power_keys(power).0).map(CastFailLine::arm_blanked);
            }
            // What is deliberately absent is as load-bearing as what is here: the pet's index
            // table sends `0x01`/`0x02` (already at full health/power), `0x09` (no target),
            // `0x18` (equipped item), `0x28` (item cooldown), `0x41` (unsheathed) and `0x8e`
            // (play time) to the generic arm, so a pet never says any of those seven lines.
            //
            // `0x17` DONT_REPORT is the one that looks like a divergence and is not: the player's
            // handler hides it by control flow, the pet's routes it to the generic arm — but
            // `SPELL_FAILED_DONT_REPORT` has no string in 5875's `GlobalStrings.lua`, so the
            // passthrough below draws nothing and the two agree on screen.
            //
            // `0x56` ONLY_SHAPESHIFT has its own arm here (`0x6e921a`, errorId `0xd6`), and it
            // is the one place the two casters' form lines actually differ: it names the FIRST
            // matching form and stops, where the player's joins them all. [`FailArgs::fill`] owns
            // that fork, below — the line still resolves through the shared layer, because
            // `ERR_SPELL_FAILED_SHAPESHIFT_FORM_S` is the bare `"%s"` exactly as the passthrough
            // `0x2c` is, and the message catalog gives ids `0x2c` and `0xd6` the same surface.
            // What is NOT shared is the log: `0x6e8eb0` never calls `0x62c360` at all.
            _ => {}
        },
        // `Spell_C::HandleCastFailed 0x6e1a00` (`0x6e1aab`–`0x6e1c5f`).
        Caster::Player => match reason {
            0x01 => return get_display("ERR_SPELL_FAILED_ALREADY_AT_FULL_HEALTH"),
            0x02 => {
                let t = get_display("ERR_SPELL_FAILED_ALREADY_AT_FULL_POWER_S")?;
                let power = spell.map_or(0, |d| d.power_type);
                let name = get_text(power_keys(power).1).unwrap_or_default();
                // `0x6e211f` copies the localized power NOUN — and nothing else — into
                // `[ebp-0x888]`, then points `edi` there. So the screen reads the sentence and
                // the combat log reads the bare word: "You fail to cast Rejuvenation: Mana."
                return Some(CastFailLine {
                    arg_text: name.clone(),
                    ..t.fill(&name)
                });
            }
            0x09 => return get_display("ERR_GENERIC_NO_TARGET"),
            0x17 => return None, // DONT_REPORT: control-flow hidden (jumps past DisplayError)
            0x18 => return get_display("ERR_SPELL_FAILED_EQUIPPED_ITEM"),
            0x28 => {
                return get_display(if is_potion(spell) {
                    "ERR_POTION_COOLDOWN"
                } else {
                    "ERR_ITEM_COOLDOWN"
                });
            }
            0x3C => {
                let key = if is_food(spell) {
                    "ERR_FOOD_COOLDOWN"
                } else if is_potion(spell) {
                    "ERR_POTION_COOLDOWN"
                } else if spell.is_some_and(|d| d.attributes & 0x10 != 0) {
                    "ERR_ABILITY_COOLDOWN"
                } else {
                    "ERR_SPELL_COOLDOWN"
                };
                return get_display(key);
            }
            0x41 => return get_display("ERR_SPELL_FAILED_NOTUNSHEATHED"),
            0x4D => {
                let power = spell.map_or(0, |d| d.power_type);
                return get_display(power_keys(power).0).map(CastFailLine::arm_blanked);
            }
            0x59 => return get_display("ERR_SPELL_OUT_OF_RANGE"),
            0x8E => return get_display("ERR_PLAY_TIME_EXCEEDED"),
            _ => {}
        },
    }
    // The passthrough layer: errorId 0x2c ("%s") displays GetText(SPELL_FAILED_<name>) as-is.
    let Some(key) = CAST_FAIL_KEYS.get(usize::from(reason)) else {
        return Some(CastFailLine::passthrough(format!(
            "Spell failed ({reason:#04x})"
        )));
    };
    let text = get_text(key)?;
    // On this layer the two buffers are one string: errorId `0x2c` is the bare `"%s"`, so what
    // `DisplayError` formats IS `[ebp-0x488]`, the same buffer `edi` still points at.
    let line = CastFailLine::passthrough;
    // The argument arms (`0x6e1d8e`): fill the template's `%s` from the reason's own DBC.
    match args.fill(caster, reason, spell) {
        Fill::Filled(name) => return Some(line(text.replace("%s", &name))),
        // The arm's own exit is the epilogue, not the shared default — nothing is displayed, and
        // nothing is logged either (`0x6e224f` is past `0x62c360` too). See [`Fill`].
        Fill::Nothing => return None,
        Fill::Template => {}
    }
    // An arm we don't model (or one whose lookup missed) — strip the tokens so the stem reads
    // clean ("Missing reagent: %s" → "Missing reagent"), never a raw % on screen.
    Some(line(strip_tokens(&text)))
}

/// The unmodeled-arm fallback's token strip — "Missing reagent: %s" → "Missing reagent", a
/// deliberate divergence from the reference's literal `%s` (module docs). Applied to **both**
/// buffers, so a declined arm never puts a raw token in the combat log either. A string with no
/// token is returned unchanged.
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

    /// The shipped 1.12 GlobalStrings entries the tests rest on (extracted values).
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
            ("ERR_POTION_COOLDOWN", "Item is not ready yet."),
            ("ERR_OUT_OF_MANA", "Not enough mana"),
            ("ERR_OUT_OF_RAGE", "Not enough rage"),
            (
                // Verbatim from 5875 — it is NOT "You have nothing to attack with.", which is
                // what this fixture claimed until the log leg made the difference observable.
                "ERR_SPELL_FAILED_NOTUNSHEATHED",
                "You have to be unsheathed to do that!",
            ),
            // The pet's own six, verbatim from the shipped file — and NO `ERR_PET_SPELL_NOPATH`,
            // because 5875 ships none, which is a fact these tests rest on.
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
        ])
    }

    /// **The six lines that exist for the pet and nobody else.** Each is a row the client raises
    /// only from `Spell_C::HandlePetCastFailed 0x6e8eb0`, and each is checked against what the
    /// *player* gets for the same wire reason — because "our pet reads the same string we do" is
    /// exactly the shape of the defect decision 2033 corrects, and a one-sided assert would not
    /// have caught it.
    #[test]
    fn the_pet_speaks_its_own_six_refusals() {
        let m = gs();
        let g = getter(&m);
        let say = |caster, reason| {
            cast_fail_text(caster, reason, None, FailArgs::default(), &g).map(|l| l.text)
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

    /// **NOPATH is silent for the pet and spoken for the player**, which reads like a bug and is
    /// the reference: the pet arm raises errorId `0x151`, whose key `ERR_PET_SPELL_NOPATH` has no
    /// string in 5875's `GlobalStrings.lua`, so `DisplayError` shows nothing. The player's `0x32`
    /// takes the passthrough and reads "No path available".
    #[test]
    fn the_pets_nopath_is_silent_because_5875_ships_no_string() {
        let m = gs();
        let g = getter(&m);
        assert_eq!(
            cast_fail_text(Caster::Pet, 0x32, None, FailArgs::default(), &g),
            None
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x32, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "No path available"
        );
    }

    /// The pet's `0x3c` computes its id from one bit — `((SpellRec+0x18 & 0x10) | 0x300) >> 4` —
    /// so it has only the ability/spell split. The player's tests the spell CATEGORY first and has
    /// a food and a potion leg. A pet eats nothing, so a potion-category spell that says "Item is
    /// not ready yet." for us says "Spell is not ready yet." for it.
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
        assert_eq!(text(Caster::Player, &potion), "Item is not ready yet.");
        assert_eq!(text(Caster::Pet, &potion), "Spell is not ready yet.");
        // The one leg they share, so the collapse is a missing branch and not a missing table.
        assert_eq!(text(Caster::Player, &ability), "Ability is not ready yet.");
        assert_eq!(text(Caster::Pet, &ability), "Ability is not ready yet.");
    }

    /// The pet's index table sends seven of the player's overrides to its generic arm, so those
    /// lines are the player's alone. Checked on the two with the sharpest tell: `0x09`, where the
    /// text changes, and `0x41`, where the **surface** changes too — the player's override is the
    /// one yellow cast failure in the game, and the pet's passthrough is red.
    #[test]
    fn the_pet_does_not_take_the_players_overrides() {
        use benilla_ui::messages::{kind_of, MsgKind};
        let m = gs();
        let g = getter(&m);
        let line = |caster, reason| {
            cast_fail_text(caster, reason, None, FailArgs::default(), &g).expect("a line")
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

    /// `0x4d` NO_POWER is the override the two handlers **agree** on, and that is a byte fact
    /// rather than an assumption: the pet's table at `0x8118f0` holds the same five dwords as the
    /// player's at `0x8118dc`. So [`power_keys`] serves both, and a hunter pet's focus ability
    /// reads the focus line on either path.
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

    /// `0x17` DONT_REPORT looks like a divergence between the two handlers and is not: the
    /// player's hides it by control flow, the pet's routes it to the generic arm — but
    /// `SPELL_FAILED_DONT_REPORT` has no string in 5875, so both draw nothing. Worth a test
    /// because the two mechanisms are different and only the *outcome* is shared, so a future
    /// change to either one should have to notice.
    #[test]
    fn dont_report_is_silent_on_both_paths_for_two_different_reasons() {
        let m = gs();
        let g = getter(&m);
        assert!(!m.contains_key("SPELL_FAILED_DONT_REPORT"));
        for caster in [Caster::Player, Caster::Pet] {
            assert_eq!(
                cast_fail_text(caster, 0x17, None, FailArgs::default(), &g),
                None
            );
        }
    }

    /// **The one cast failure that is not a red line.** Reason `0x41` overrides to
    /// `ERR_SPELL_FAILED_NOTUNSHEATHED` (id 320), and that record's kind is `1` — the yellow
    /// `UI_INFO_MESSAGE`, not the red `UI_ERROR_MESSAGE` its seventeen override neighbours take.
    ///
    /// benilla painted it red until decision 1770, and could not have done otherwise: the drain
    /// fired one event for every resolved cast-failure line, because by the time a line got there
    /// nothing knew which errorId this dispatch had chosen. Carrying the key out is what makes the
    /// surface answerable, and this test is the answer.
    ///
    /// Its two yellow siblings, `ERR_FISH_NOT_HOOKED` (318) and `ERR_FISH_ESCAPED` (319), sit one
    /// and two ids below it and were hand-traced correctly by an earlier round — which is exactly
    /// how a hand-swept surface goes wrong: two of the three neighbours seen, the third not.
    #[test]
    fn the_unsheathed_refusal_is_yellow_and_its_neighbours_are_red() {
        use benilla_ui::messages::{kind_of, MsgKind};
        let m = gs();
        let g = getter(&m);

        let line =
            cast_fail_text(Caster::Player, 0x41, None, FailArgs::default(), &g).expect("a line");
        assert_eq!(line.key, "ERR_SPELL_FAILED_NOTUNSHEATHED");
        assert_eq!(kind_of(line.key), MsgKind::Info);

        // An override that IS red, and a passthrough that falls to errorId 0x2c.
        let red =
            cast_fail_text(Caster::Player, 0x59, None, FailArgs::default(), &g).expect("a line");
        assert_eq!(red.key, "ERR_SPELL_OUT_OF_RANGE");
        assert_eq!(kind_of(red.key), MsgKind::Error);

        let through =
            cast_fail_text(Caster::Player, 0x43, None, FailArgs::default(), &g).expect("a line");
        assert_eq!(through.key, PASSTHROUGH);
        assert_eq!(kind_of(through.key), MsgKind::Error);
    }

    /// **`0x56` ONLY_SHAPESHIFT, the arm's three shapes** (decision 2280): one form, several
    /// joined with the reference's `", "`, and a row whose `Name` the DBC leaves empty.
    ///
    /// The bit→id law is the reference's record walk read as a lookup — row index `b` is form id
    /// `b+1` — so Prowl's `Stances = 0x1` names form 1 and Maul's `0x90` names forms 5 and 8, in
    /// **mask order**, which is the order the loop visits them in and not the order anything
    /// sorts them into.
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
        // `6e2058: cmp BYTE PTR [eax],0x0` — an empty name is skipped, not joined as a gap.
        assert_eq!(
            text(0x90 | 0x100).as_deref(),
            Some("Must be in Bear Form, Dire Bear Form"),
        );
        assert_eq!(text(0x101).as_deref(), Some("Must be in Cat Form"));
    }

    /// **The `0x56` arm's failure exit is the function epilogue, not the strip fallback** — an
    /// unfillable ONLY_SHAPESHIFT displays NOTHING (`6e20c6 jne 0x6e224f`, past `DisplayError`),
    /// where every other declining arm still shows its stripped stem. Four ways in: no spell
    /// record (`6e1ffd`), an unloaded DBC (`6e2016`), an empty mask, and a mask naming only
    /// unnamed rows.
    ///
    /// The control is the neighbour: `0x5c` REAGENTS declines through the SHARED default and so
    /// still reads "Missing reagent". If this ever regresses, the tell is the words "Must be in"
    /// alone on screen — which is exactly how the defect was reported.
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
        // The control: the arm next door still strips rather than falling silent.
        assert_eq!(
            cast_fail_text(Caster::Player, 0x5C, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "Missing reagent"
        );
    }

    /// **The pet's `0x56` names ONE form** (`0x6e921a`; decision 2285 corrected 2280, which had
    /// assumed the two casters produced the same line). `0x6e9282: jne 0x6e929a` leaves the loop
    /// at the first non-empty name — a break, not a continue, and the earned negative is that the
    /// whole pet handler contains no `SStrCat` and no reference to the separator.
    ///
    /// The single-form case is asserted beside it deliberately: it is where the two agree, and a
    /// test written only on that case is exactly how the wrong claim passed its own test.
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
        // One form: the two agree, and the surface is the same despite the different errorId.
        assert_eq!(text(Caster::Player, 0x1), text(Caster::Pet, 0x1));
        assert_eq!(
            text(Caster::Pet, 0x1).as_deref(),
            Some("Must be in Cat Form")
        );
        // Nothing to name is silence on both handlers, by two different routes.
        assert_eq!(text(Caster::Pet, 0x0), None);
    }

    /// **The combat log reads the reference's OTHER buffer** ([`CastFailLine::logged`] —
    /// `edi` at `0x6e21e2`), which decision 2285 exists to correct. Four classes, and the whole
    /// point is that "log what the red line said" and "re-derive from the key table" each get
    /// some of them wrong.
    #[test]
    fn the_combat_log_reads_the_argtext_buffer_not_the_displayed_line() {
        let m = gs();
        let g = getter(&m);
        let line = |reason, spell| {
            cast_fail_text(Caster::Player, reason, spell, FailArgs::default(), &g).expect("a line")
        };

        // 1b · an errorId override with NO argument arm: `edi` never moved off the first-layer
        // buffer, so the screen and the log disagree. This is the row 2280 got wrong.
        let plain = spell(0, 0, 0);
        let cooldown = line(0x3C, Some(&plain));
        assert_eq!(cooldown.text, "Spell is not ready yet.");
        assert_eq!(cooldown.logged(), "Not yet recovered");

        let no_target = line(0x09, None);
        assert_eq!(no_target.text, "You have no target.");
        assert_eq!(no_target.logged(), "No target");

        let unsheathed = line(0x41, None);
        assert_eq!(unsheathed.text, "You have to be unsheathed to do that!");
        assert_eq!(unsheathed.logged(), "You must be unsheathed");

        // 1a · no override, no arm: one string in both buffers.
        let ammo = line(0x43, None);
        assert_eq!(ammo.logged(), ammo.text);

        // 4 · `0x4d` BLANKS the buffer (`0x6e20fd`), so the log falls through to the last-error
        // cell and reads the screen's wording — the override whose two surfaces agree.
        let rage = spell(1, 0, 0);
        let power = line(0x4D, Some(&rage));
        assert_eq!(power.text, "Not enough rage");
        assert_eq!(power.logged(), "Not enough rage");

        // A declined arm strips in BOTH buffers — never a raw token in the log either.
        let reagents = line(0x5C, None);
        assert_eq!(reagents.logged(), "Missing reagent");
        assert!(!reagents.logged().contains('%'));
    }

    /// The `0x02` arm (`0x6e211f`) copies only the localized power NOUN into the argText buffer,
    /// so the screen reads a sentence and the log reads one word. The reference's own quirk, and
    /// the one row where a "the log says what the screen says" model is visibly wrong.
    #[test]
    fn already_at_full_power_logs_the_bare_power_noun() {
        let mut m = gs();
        m.insert(
            "ERR_SPELL_FAILED_ALREADY_AT_FULL_POWER_S",
            "You are already at full %s.",
        );
        m.insert("RAGE", "Rage");
        let g = getter(&m);
        let rage = spell(1, 0, 0);
        let line =
            cast_fail_text(Caster::Player, 0x02, Some(&rage), FailArgs::default(), &g).unwrap();
        assert_eq!(line.text, "You are already at full Rage.");
        assert_eq!(line.logged(), "Rage");
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

    /// Passthrough reads the SPELL_FAILED string; the errorId overrides REPLACE it: 0x59
    /// shows the perioded ERR string, 0x3c the cooldown family (never "Not yet recovered"),
    /// 0x09 the generic no-target line.
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
            cast_fail_text(Caster::Player, 0x59, None, FailArgs::default(), &g)
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
            cast_fail_text(Caster::Player, 0x09, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "You have no target."
        );
        // 0x3c: plain spell → spell cooldown; Attr&0x10 → ability; potion category → potion.
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
            "Item is not ready yet."
        );
    }

    /// NO_POWER picks the power family off the SPELL's power type — the warrior's rage
    /// ability reads "Not enough rage", never the generic power line.
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

    /// Both suppression faces: 0x17 control-flow hidden, 0x08 data-hidden (key absent from
    /// GlobalStrings); an off-table reason keeps the debug hex; an unfilled %s template
    /// strips to its stem.
    #[test]
    fn suppression_hex_fallback_and_template_strip() {
        let m = gs();
        let g = getter(&m);
        assert_eq!(
            cast_fail_text(Caster::Player, 0x17, None, FailArgs::default(), &g),
            None
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x08, None, FailArgs::default(), &g),
            None
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

    /// The RUNTIME leg, end to end on the real data: the shipped `GlobalStrings.lua` executed
    /// into a real VM (the boot's `load_global_strings` path), then the drain's exact lookup —
    /// the leg whose absence shipped a fold where every red line silently vanished (the VM had
    /// no GlobalStrings at all; the fake-getter tests above couldn't see it). Skips without
    /// client data.
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
            cast_fail_text(Caster::Player, 0x59, None, FailArgs::default(), &g)
                .unwrap()
                .text,
            "Out of range."
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x3C, None, FailArgs::default(), &g)
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
        // The environment gate's pair (decision 1056) — both are plain passthroughs, so what the
        // player reads IS the GlobalStrings value. A typo'd key here would degrade a real refusal
        // to a dead-looking button, which is what this test exists to catch.
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
        // The data-suppression face on the real file: the absent keys show nothing.
        assert_eq!(
            cast_fail_text(Caster::Player, 0x08, None, FailArgs::default(), &g),
            None
        );
        assert_eq!(
            cast_fail_text(Caster::Player, 0x21, None, FailArgs::default(), &g),
            None
        );

        // The pet's table against the player's own shipped file (decision 2033). Both halves
        // matter: the six rows resolve to the pet wording, and `ERR_PET_SPELL_NOPATH` resolves to
        // NOTHING — the claim the `SILENT_IN_5875` entry rests on, checked here against the real
        // `GlobalStrings.lua` rather than against a fixture that could simply have omitted it.
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
                cast_fail_text(Caster::Pet, reason, None, FailArgs::default(), &g)
                    .map(|l| l.text)
                    .as_deref(),
                Some(pet),
                "pet {reason:#04x}"
            );
            assert_eq!(
                cast_fail_text(Caster::Player, reason, None, FailArgs::default(), &g)
                    .map(|l| l.text)
                    .as_deref(),
                Some(player),
                "player {reason:#04x}"
            );
        }
        assert_eq!(
            cast_fail_text(Caster::Pet, 0x32, None, FailArgs::default(), &g),
            None,
            "5875 ships no ERR_PET_SPELL_NOPATH, so the reference shows nothing"
        );
        assert!(
            g("PET_SPELL_NOPATH").is_some(),
            "and the near-miss key that made the old map look right IS shipped — which is the \
             whole trap"
        );

        // B255, end to end on the real data: the argument arms against the real DBCs and the real
        // GlobalStrings templates. Without the fill these read as the bare stems "Requires" and
        // "You need to be in" — which is exactly what shipped.
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
        };
        // **0x8d PREVENTED_BY_MECHANIC** (decision 1948) — the crowd-control ladder's renamed
        // refusal, and the one argument arm whose word is produced locally rather than read off
        // the wire. The names are lower-case adjectives in the shipped data, which is what makes
        // them read as the tail of the sentence.
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
        // An unknown or absent mechanic strips to the bare stem rather than showing a raw `%s`.
        assert!(!cast_fail_text(Caster::Player, 0x8D, None, args(999), &g)
            .unwrap()
            .text
            .contains('%'));

        // 0x5e REQUIRES_SPELL_FOCUS: the Crown of the Earth phials' own refusal. Focus 12 is the
        // Starbreeze Village moonwell — using the Jade Phial at any *other* pool is the report.
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
        // 0x5d REQUIRES_AREA: area 1657 is Darnassus.
        assert_eq!(
            cast_fail_text(Caster::Player, 0x5D, None, args(1657), &g)
                .unwrap()
                .text,
            "You need to be in Darnassus"
        );
        // The fallbacks. An id the DBC doesn't name, and a wire word the server never sent, both
        // decline the arm and fall through to the strip — never a raw `%s` on screen.
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
        // **0x56 ONLY_SHAPESHIFT on the real chain — the reported defect** (decision 2280): the
        // real `Spell.dbc` `Stances` column, the real `SpellShapeshiftForm.dbc` names and the
        // real `"Must be in %s"`. Prowl in bear form is the report; Maul is the multi-form join,
        // which a single-bit test could not tell apart from "name the lowest bit".
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
        // Prowl, all three ranks — `Stances = 0x1`, the one the spellbook tooltip already read as
        // "Requires Cat Form" while this line read the bare stem.
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

        // 0x5e alone has a client-side stand-in: the failing spell's own `RequiresSpellFocus`
        // column is the very number the server copied onto the wire, so an absent word still
        // fills. (Spell 4976 "Filling" — the Crystal Phial's — carries focus 11.)
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
    }
}
