//! The combat log's message families: each family's GlobalString stem, its keying and the slots
//! its templates consume, in the order the enUS template takes them. The switches that pick one
//! from a packet's figures are [`super::outcome`].

use super::{Family, Keying, Slot};

// ────────────────────────────────── the families ──────────────────────────────────────

/// Declare a four-way family: its key stem and its slots in the order the enUS template consumes
/// them, with that template and its `GlobalStrings.lua` line as the doc.
macro_rules! family {
    ($name:ident, $stem:literal, [$($slot:ident),* $(,)?], $doc:literal) => {
        #[doc = $doc]
        pub(crate) const $name: Family = Family {
            stem: $stem,
            keying: Keying::Quad,
            tail: "",
            slots: &[$(Slot::$slot),*],
        };
    };
}

/// Declare a two-way family, keyed on the subject alone, with an optional `tail`.
macro_rules! duo {
    ($name:ident, $stem:literal, $me:literal, $other:literal, [$($slot:ident),* $(,)?], $doc:literal) => {
        duo!($name, $stem, $me, $other, tail "", [$($slot),*], $doc);
    };
    ($name:ident, $stem:literal, $me:literal, $other:literal, tail $tail:literal, [$($slot:ident),* $(,)?], $doc:literal) => {
        #[doc = $doc]
        pub(crate) const $name: Family = Family {
            stem: $stem,
            keying: Keying::Duo { me: $me, other: $other },
            tail: $tail,
            slots: &[$(Slot::$slot),*],
        };
    };
}

/// Declare a family with no variant: the key is the whole name.
macro_rules! single {
    ($name:ident, $key:literal, [$($slot:ident),* $(,)?], $doc:literal) => {
        #[doc = $doc]
        pub(crate) const $name: Family = Family {
            stem: $key,
            keying: Keying::Single,
            tail: "",
            slots: &[$(Slot::$slot),*],
        };
    };
}

// ── melee: the swing that landed ───────────────────────────────────────────────────────
family!(
    COMBATHIT,
    "COMBATHIT",
    [Attacker, Victim, Amount],
    "`\"%s hits %s for %d.\"` (GlobalStrings.lua:777)"
);
family!(
    COMBATHITCRIT,
    "COMBATHITCRIT",
    [Attacker, Victim, Amount],
    "`\"%s crits %s for %d.\"` (:771)"
);
family!(
    COMBATHITSCHOOL,
    "COMBATHITSCHOOL",
    [Attacker, Victim, Amount, School],
    "`\"%s hits %s for %d %s damage.\"` (:779)"
);
family!(
    COMBATHITCRITSCHOOL,
    "COMBATHITCRITSCHOOL",
    [Attacker, Victim, Amount, School],
    "`\"%s crits %s for %d %s damage.\"` (:773)"
);

// ── melee: the swing that did not ──────────────────────────────────────────────────────
family!(
    MISSED,
    "MISSED",
    [Attacker, Victim],
    "`\"%s misses %s.\"` (:2698)"
);
family!(
    VSDODGE,
    "VSDODGE",
    [Attacker, Victim],
    "`\"%s attacks. %s dodges.\"` (:5400)"
);
family!(
    VSPARRY,
    "VSPARRY",
    [Attacker, Victim],
    "`\"%s attacks. %s parries.\"` (:5421)"
);
family!(
    VSBLOCK,
    "VSBLOCK",
    [Attacker, Victim],
    "`\"%s attacks. %s blocks.\"` (:5394)"
);
family!(
    VSABSORB,
    "VSABSORB",
    [Attacker, Victim],
    "`\"%s attacks. %s absorbs all the damage.\"` (:5391)"
);
family!(
    VSRESIST,
    "VSRESIST",
    [Attacker, Victim],
    "`\"%s attacks. %s resists all the damage.\"` (:5424)"
);
family!(
    VSIMMUNE,
    "VSIMMUNE",
    [Attacker, Victim],
    "`\"%s attacks but %s is immune.\"` (:5418)"
);
family!(
    VSEVADE,
    "VSEVADE",
    [Attacker, Victim],
    "`\"%s attacks. %s evades.\"` (:5415)"
);
family!(
    VSDEFLECT,
    "VSDEFLECT",
    [Attacker, Victim],
    "`\"%s attacks. %s deflects.\"` (:5397)"
);

// ── a spell's direct damage ────────────────────────────────────────────────────────────
family!(
    SPELLLOG,
    "SPELLLOG",
    [Attacker, Spell, Victim, Amount],
    "`\"%s's %s hits %s for %d.\"` (:3878)"
);
family!(
    SPELLLOGCRIT,
    "SPELLLOGCRIT",
    [Attacker, Spell, Victim, Amount],
    "`\"%s's %s crits %s for %d.\"` (:3870)"
);
family!(
    SPELLLOGSCHOOL,
    "SPELLLOGSCHOOL",
    [Attacker, Spell, Victim, Amount, School],
    "`\"%s's %s hits %s for %d %s damage.\"` (:3880)"
);
family!(
    SPELLLOGCRITSCHOOL,
    "SPELLLOGCRITSCHOOL",
    [Attacker, Spell, Victim, Amount, School],
    "`\"%s's %s crits %s for %d %s damage.\"` (:3872)"
);

// ── a spell that did not land ──────────────────────────────────────────────────────────
family!(
    SPELLMISS,
    "SPELLMISS",
    [Attacker, Spell, Victim],
    "`\"%s's %s missed %s.\"` (:3886)"
);
family!(
    SPELLRESIST,
    "SPELLRESIST",
    [Attacker, Spell, Victim],
    "`\"%s's %s was resisted by %s.\"` (:3911)"
);
family!(
    SPELLDODGED,
    "SPELLDODGED",
    [Attacker, Spell, Victim],
    "`\"%s's %s was dodged by %s.\"` (:3835)"
);
family!(
    SPELLPARRIED,
    "SPELLPARRIED",
    [Attacker, Spell, Victim],
    "`\"%s's %s was parried by %s.\"` (:3890)"
);
family!(
    SPELLBLOCKED,
    "SPELLBLOCKED",
    [Attacker, Spell, Victim],
    "`\"%s's %s was blocked by %s.\"` (:3817)"
);
family!(
    SPELLEVADED,
    "SPELLEVADED",
    [Attacker, Spell, Victim],
    "`\"%s's %s was evaded by %s.\"` (:3845)"
);
family!(
    SPELLIMMUNE,
    "SPELLIMMUNE",
    [Attacker, Spell, Victim],
    "`\"%s's %s fails. %s is immune.\"` (:3859)"
);
family!(
    SPELLDEFLECTED,
    "SPELLDEFLECTED",
    [Attacker, Spell, Victim],
    "`\"%s's %s was deflected by %s.\"` (:3829)"
);
family!(
    SPELLLOGABSORB,
    "SPELLLOGABSORB",
    [Attacker, Spell, Victim],
    "`\"%s's %s is absorbed by %s.\"` (:3866)"
);
family!(
    SPELLREFLECT,
    "SPELLREFLECT",
    [Attacker, Spell, Victim],
    "`\"%s's %s is reflected back by %s.\"` (:3907)"
);

// ── heals and power ────────────────────────────────────────────────────────────────────
family!(
    HEALED,
    "HEALED",
    [Attacker, Spell, Victim, Amount],
    "`\"%s's %s heals %s for %d.\"` (:2129)"
);
family!(
    HEALEDCRIT,
    "HEALEDCRIT",
    [Attacker, Spell, Victim, Amount],
    "`\"%s's %s critically heals %s for %d.\"` (:2125)"
);
family!(
    POWERGAIN,
    "POWERGAIN",
    [Victim, Amount, Power, Attacker, Spell],
    "`\"%s gains %d %s from %s's %s.\"` (:3091)"
);
family!(SPELLPOWERLEECH, "SPELLPOWERLEECH",
    [Attacker, Spell, Amount, Power, Victim, Attacker, Amount2, Power2],
    "`\"%s's %s drains %d %s from %s. %s gains %d %s.\"` (:3904) — the only family that spells the \
     subject TWICE, and the second one is the same name as the first (the drainer is the gainer).");

// ── periodic ticks ─────────────────────────────────────────────────────────────────────
family!(
    PERIODICAURADAMAGE,
    "PERIODICAURADAMAGE",
    [Victim, Amount, School, Attacker, Spell],
    "`\"%s suffers %d %s damage from %s's %s.\"` (:3001)"
);
family!(
    PERIODICAURAHEAL,
    "PERIODICAURAHEAL",
    [Victim, Amount, Attacker, Spell],
    "`\"%s gains %d health from %s's %s.\"` (:3005)"
);

// ── a damage shield firing back ────────────────────────────────────────────────────────
family!(
    DAMAGESHIELD,
    "DAMAGESHIELD",
    [Attacker, Amount, School, Victim],
    "`\"%s reflects %d %s damage to %s.\"` (:884) — the SUBJECT is the shield's BEARER and the \
     object is whoever struck them, which is the reverse of the packet's own field names."
);

// ── a unit dying ───────────────────────────────────────────────────────────────────────
duo!(
    UNITDIES,
    "UNITDIES",
    "SELF",
    "OTHER",
    [Attacker],
    "`\"You die.\"` (:4426) / `\"%s dies.\"` (:4425) — the death reflex's ordinary line. The SUBJECT \
     is the unit that died; there is no second endpoint."
);
single!(
    UNITDESTROYEDOTHER,
    "UNITDESTROYEDOTHER",
    [Attacker],
    "`\"%s is destroyed.\"` (:4424) — what a SUMMONED thing does instead of dying. There is no \
     `…SELF` twin: you are never a summon."
);
single!(
    SELFKILLOTHER,
    "SELFKILLOTHER",
    [Attacker],
    "`\"You have slain %s!\"` (:3408) — `SMSG_PARTYKILLLOG` when the killer is YOU. The reference \
     pushes (victim, killer) and the string consumes only the first, so the subject is the VICTIM."
);
single!(
    PARTYKILLOTHER,
    "PARTYKILLOTHER",
    [Attacker, Victim],
    "`\"%s is slain by %s!\"` (:2988) — the same packet when the killer is a party member. \
     (victim, killer), in that order."
);

// ── an aura landing, stacking, leaving, being dispelled or stolen ──────────────────────
duo!(
    AURAADDED_HARMFUL,
    "AURAADDED",
    "SELF",
    "OTHER",
    tail "HARMFUL",
    [Attacker, Spell],
    "`\"You are afflicted by %s.\"` (:102) / `\"%s is afflicted by %s.\"` (:100) — UNIT first."
);
duo!(
    AURAADDED_HELPFUL,
    "AURAADDED",
    "SELF",
    "OTHER",
    tail "HELPFUL",
    [Attacker, Spell],
    "`\"You gain %s.\"` (:103) / `\"%s gains %s.\"` (:101)"
);
duo!(
    AURAAPPLICATIONADDED_HARMFUL,
    "AURAAPPLICATIONADDED",
    "SELF",
    "OTHER",
    tail "HARMFUL",
    [Attacker, Spell, Amount],
    "`\"You are afflicted by %s (%d).\"` (:106) / `\"%s is afflicted by %s (%d).\"` (:104)"
);
duo!(
    AURAAPPLICATIONADDED_HELPFUL,
    "AURAAPPLICATIONADDED",
    "SELF",
    "OTHER",
    tail "HELPFUL",
    [Attacker, Spell, Amount],
    "`\"You gain %s (%d).\"` (:107) / `\"%s gains %s (%d).\"` (:105)"
);
duo!(
    AURAREMOVED,
    "AURAREMOVED",
    "SELF",
    "OTHER",
    [Spell, Attacker],
    "`\"%s fades from you.\"` (:113) / `\"%s fades from %s.\"` (:112) — AURA first, which is the \
     other way round from `AURAADDED*`. The flip is the reference's own (`0x62b480`) and is \
     exactly the kind of thing a single ordered slot list per family exists to pin."
);
duo!(
    AURADISPEL,
    "AURADISPEL",
    "SELF",
    "OTHER",
    [Attacker, Spell],
    "`\"Your %s is removed.\"` (:111) / `\"%s's %s is removed.\"` (:110) — the subject is the aura's \
     BEARER, not the dispeller, who the sentence never names."
);

// ── an enchant landing on or fading from an item ───────────────────────────────────────
family!(
    ITEMENCHANTMENTADD,
    "ITEMENCHANTMENTADD",
    [Attacker, Spell, Victim, Named],
    "`\"%s casts %s on %s's %s.\"` (:2378) — caster, enchant, owner, ITEM. The item name is always \
     last and always spelled."
);
duo!(
    ITEMENCHANTMENTREMOVE,
    "ITEMENCHANTMENTREMOVE",
    "SELF",
    "OTHER",
    [Spell, Attacker, Named],
    "`\"%s has faded from your %s.\"` (:2383) / `\"%s has faded from %s's %s.\"` (:2382) — enchant, \
     owner, item. A fade names no caster, which is why it drops to two keys."
);

// ── what a cast made, fed or opened ────────────────────────────────────────────────────
duo!(
    TRADESKILL_LOG,
    "TRADESKILL_LOG",
    "_FIRSTPERSON",
    "_THIRDPERSON",
    [Attacker, Named],
    "`\"You create %s.\"` (:4273) / `\"%s creates %s.\"` (:4274) — the one family pair whose two-way \
     words are not SELF/OTHER."
);
duo!(
    FEEDPET_LOG,
    "FEEDPET_LOG",
    "_FIRSTPERSON",
    "_THIRDPERSON",
    [Attacker, Named],
    "`\"Your pet begins eating the %s.\"` (:1965) / `\"%s's pet begins eating a %s.\"` (:1966)"
);

// ── the spell-outcome leaves ───────────────────────────────────────────────────────────
family!(
    PROCRESIST,
    "PROCRESIST",
    [Victim, Attacker, Spell],
    "`\"%s resists %s's %s.\"` (:3100) — TARGET first, then caster (the reference's convention B)."
);
family!(
    IMMUNESPELL,
    "IMMUNESPELL",
    [Victim, Attacker, Spell],
    "`\"%s is immune to %s's %s.\"` (:2317) — convention B."
);
family!(
    DISPELFAILED,
    "DISPELFAILED",
    [Attacker, Victim, Spell],
    "`\"%s fails to dispel %s's %s.\"` (:933) — convention C: caster, target, spell."
);
family!(
    SPELLINTERRUPT,
    "SPELLINTERRUPT",
    [Attacker, Victim, Spell],
    "`\"%s interrupts %s's %s.\"` (:3863) — convention C. No `…SELFSELF`."
);
family!(
    SIMPLECAST,
    "SIMPLECAST",
    [Attacker, Spell, Victim],
    "`\"%s casts %s on %s.\"` (:3466) — `0x62adc0`'s line for a cast with a target."
);
family!(
    SIMPLEPERFORM,
    "SIMPLEPERFORM",
    [Attacker, Spell, Victim],
    "`\"%s performs %s on %s.\"` (:3470) — [`SIMPLECAST`] for an ability, `Attributes & 0x10`."
);
duo!(
    SPELLTERSE,
    "SPELLTERSE",
    "_SELF",
    "_OTHER",
    [Attacker, Spell],
    "`\"You cast %s.\"` (:3926) / `\"%s casts %s.\"` (:3925) — `0x62b000`'s line for a cast with \
     no target, keyed on the caster."
);
duo!(
    SPELLTERSEPERFORM,
    "SPELLTERSEPERFORM",
    "_SELF",
    "_OTHER",
    [Attacker, Spell],
    "`\"You perform %s.\"` (:3924) / `\"%s performs %s.\"` (:3923) — [`SPELLTERSE`] for an \
     ability."
);
duo!(
    INSTAKILL,
    "INSTAKILL",
    "SELF",
    "OTHER",
    [Attacker, Spell],
    "`\"You are killed by %s.\"` (:2330) / `\"%s is killed by %s.\"` (:2329) — subject is the unit \
     killed."
);
family!(
    SPELLSPLITDAMAGE,
    "SPELLSPLITDAMAGE",
    [Attacker, Spell, Victim, Amount],
    "`\"%s's %s causes %s %d damage.\"` (:3916) — the `hit_info & 8` split-damage form of \
     `SMSG_SPELLNONMELEEDAMAGELOG`. No `…SELFSELF`."
);
family!(
    SPELLPOWERDRAIN,
    "SPELLPOWERDRAIN",
    [Attacker, Spell, Amount, Power, Victim],
    "`\"%s's %s drains %d %s from %s.\"` (:3900) — [`SPELLPOWERLEECH`]'s twin, chosen when the \
     leech multiplier is effectively zero: the same push block, three slots shorter."
);
duo!(
    SPELLEXTRAATTACKS,
    "SPELLEXTRAATTACKS",
    "SELF",
    "OTHER",
    [Attacker, Amount, Spell],
    "`\"You gain %d extra attacks through %s.\"` (:3851) / `\"%s gains %d extra attacks through \
     %s.\"` (:3849)"
);
duo!(
    SPELLEXTRAATTACKS_SINGULAR,
    "SPELLEXTRAATTACKS",
    "SELF",
    "OTHER",
    tail "_SINGULAR",
    [Attacker, Amount, Spell],
    "`\"You gain %d extra attack through %s.\"` (:3852) — the reference `SStrCat`s `_SINGULAR` onto \
     the same key when the count is 1, so this is the same family with a tail rather than a fifth \
     name."
);
family!(
    SPELLDURABILITYDAMAGE,
    "SPELLDURABILITYDAMAGE",
    [Attacker, Spell, Victim, Named],
    "`\"%s casts %s on %s: %s damaged.\"` (:3842) — the trailing name is the ITEM. No `…SELFSELF`."
);
family!(
    SPELLDURABILITYDAMAGEALL,
    "SPELLDURABILITYDAMAGEALL",
    [Attacker, Spell, Victim],
    "`\"%s casts %s on %s: all items damaged.\"` (:3839) — what the same effect says when its slot \
     and item id are both `-1`. No `…SELFSELF`."
);
duo!(
    SPELLDISMISSPET,
    "SPELLDISMISSPET",
    "SELF",
    "OTHER",
    [Attacker, Named],
    "`\"Your %s is dismissed.\"` (:3834) / `\"%s's %s is dismissed.\"` (:3833) — the trailing name \
     is the PET."
);
duo!(
    SPELLHAPPINESSDRAIN,
    "SPELLHAPPINESSDRAIN",
    "SELF",
    "OTHER",
    [Attacker, Named, Amount],
    "`\"Your %s loses %d happiness.\"` (:3858) / `\"%s's %s loses %d happiness.\"` (:3857) — power \
     type 4 has no `…_POINTS` noun, so happiness gets its own family instead of a `POWERGAIN` row."
);

// ── a cast that failed ─────────────────────────────────────────────────────────────────
duo!(
    SPELLFAILCAST,
    "SPELLFAILCAST",
    "SELF",
    "OTHER",
    [Attacker, Spell, Named],
    "`\"You fail to cast %s: %s.\"` (:3854) / `\"%s fails to cast %s: %s.\"` (:3853) — the trailing \
     name is the REASON, already localized."
);
duo!(
    SPELLFAILPERFORM,
    "SPELLFAILPERFORM",
    "SELF",
    "OTHER",
    [Attacker, Spell, Named],
    "`\"You fail to perform %s: %s.\"` (:3856) / `\"%s fails to perform %s: %s.\"` (:3855)"
);

// ── the miscellaneous 0x19 leaves and the reputation line ──────────────────────────────
single!(
    DURABILITYDAMAGE_DEATH,
    "DURABILITYDAMAGE_DEATH",
    [],
    "`\"Your equipped items suffer a 10%% durability loss.\"` (:963) — `SMSG_DURABILITY_DAMAGE_DEATH` \
     carries an EMPTY body, so the line has no arguments at all."
);
single!(
    PET_LOYALTY_GAIN,
    "PET_LOYALTY_GAIN",
    [],
    "`\"Your pet's loyalty has increased.\"` (:3043). The reference emits it as `(\"%s\", GetText(key))`; \
     a no-slot family is the same sentence by a shorter road."
);
single!(
    PET_LOYALTY_LOSS,
    "PET_LOYALTY_LOSS",
    [],
    "`\"Your pet's loyalty has decreased.\"` (:3044)"
);
single!(
    FACTION_STANDING_INCREASED,
    "FACTION_STANDING_INCREASED",
    [Named, Amount],
    "`\"Your %s reputation has increased by %d.\"` (:1946) — the name is the FACTION's."
);
single!(
    FACTION_STANDING_DECREASED,
    "FACTION_STANDING_DECREASED",
    [Named, Amount],
    "`\"Your %s reputation has decreased by %d.\"` (:1945)"
);

// ── environmental damage ───────────────────────────────────────────────────────────────
/// Declare one `VSENVIRONMENTALDAMAGE_<TYPE>_{SELF,OTHER}` pair, the key the reference builds with
/// `snprintf` over the six type names at `0x80dcac`.
macro_rules! env_family {
    ($name:ident, $stem:literal, $doc:literal) => {
        duo!($name, $stem, "SELF", "OTHER", [Attacker, Amount], $doc);
    };
}
env_family!(
    VSENV_FATIGUE,
    "VSENVIRONMENTALDAMAGE_FATIGUE_",
    "`\"You are exhausted and lose %d health.\"` (:5408) — damage type 0."
);
env_family!(
    VSENV_DROWNING,
    "VSENVIRONMENTALDAMAGE_DROWNING_",
    "`\"You are drowning and lose %d health.\"` (:5404) — type 1."
);
env_family!(
    VSENV_FALLING,
    "VSENVIRONMENTALDAMAGE_FALLING_",
    "`\"You fall and lose %d health.\"` (:5406) — type 2."
);
env_family!(
    VSENV_LAVA,
    "VSENVIRONMENTALDAMAGE_LAVA_",
    "`\"You lose %d health for swimming in lava.\"` (:5412) — type 3."
);
env_family!(
    VSENV_SLIME,
    "VSENVIRONMENTALDAMAGE_SLIME_",
    "`\"You lose %d health for swimming in slime.\"` (:5414) — type 4."
);
env_family!(
    VSENV_FIRE,
    "VSENVIRONMENTALDAMAGE_FIRE_",
    "`\"You suffer %d points of fire damage.\"` (:5410) — type 5."
);

/// The environmental family for a `SMSG_ENVIRONMENTALDAMAGELOG` damage type; `None` past the six,
/// where the reference indexes its table unguarded (`0x62abc5`).
pub(crate) fn env_family(damage_type: u8) -> Option<Family> {
    Some(match damage_type {
        0 => VSENV_FATIGUE,
        1 => VSENV_DROWNING,
        2 => VSENV_FALLING,
        3 => VSENV_LAVA,
        4 => VSENV_SLIME,
        5 => VSENV_FIRE,
        _ => return None,
    })
}

/// Every family this module emits, for the sweep against the shipped templates; a family left out
/// goes unchecked.
#[cfg(test)]
pub(crate) const ALL_FAMILIES: &[Family] = &[
    COMBATHIT,
    COMBATHITCRIT,
    COMBATHITSCHOOL,
    COMBATHITCRITSCHOOL,
    MISSED,
    VSDODGE,
    VSPARRY,
    VSBLOCK,
    VSABSORB,
    VSRESIST,
    VSIMMUNE,
    VSEVADE,
    VSDEFLECT,
    SPELLLOG,
    SPELLLOGCRIT,
    SPELLLOGSCHOOL,
    SPELLLOGCRITSCHOOL,
    SPELLMISS,
    SPELLRESIST,
    SPELLDODGED,
    SPELLPARRIED,
    SPELLBLOCKED,
    SPELLEVADED,
    SPELLIMMUNE,
    SPELLDEFLECTED,
    SPELLLOGABSORB,
    SPELLREFLECT,
    HEALED,
    HEALEDCRIT,
    POWERGAIN,
    SPELLPOWERLEECH,
    PERIODICAURADAMAGE,
    PERIODICAURAHEAL,
    DAMAGESHIELD,
    UNITDIES,
    UNITDESTROYEDOTHER,
    SELFKILLOTHER,
    PARTYKILLOTHER,
    AURAADDED_HARMFUL,
    AURAADDED_HELPFUL,
    AURAAPPLICATIONADDED_HARMFUL,
    AURAAPPLICATIONADDED_HELPFUL,
    AURAREMOVED,
    AURADISPEL,
    ITEMENCHANTMENTADD,
    ITEMENCHANTMENTREMOVE,
    TRADESKILL_LOG,
    FEEDPET_LOG,
    PROCRESIST,
    IMMUNESPELL,
    DISPELFAILED,
    SPELLINTERRUPT,
    SIMPLECAST,
    SIMPLEPERFORM,
    SPELLTERSE,
    SPELLTERSEPERFORM,
    INSTAKILL,
    SPELLSPLITDAMAGE,
    SPELLPOWERDRAIN,
    SPELLEXTRAATTACKS,
    SPELLEXTRAATTACKS_SINGULAR,
    SPELLDURABILITYDAMAGE,
    SPELLDURABILITYDAMAGEALL,
    SPELLDISMISSPET,
    SPELLHAPPINESSDRAIN,
    SPELLFAILCAST,
    SPELLFAILPERFORM,
    DURABILITYDAMAGE_DEATH,
    PET_LOYALTY_GAIN,
    PET_LOYALTY_LOSS,
    FACTION_STANDING_INCREASED,
    FACTION_STANDING_DECREASED,
    VSENV_FATIGUE,
    VSENV_DROWNING,
    VSENV_FALLING,
    VSENV_LAVA,
    VSENV_SLIME,
    VSENV_FIRE,
];
