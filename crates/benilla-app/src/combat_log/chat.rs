//! The combat log's chat-line leg. The reference's display dispatcher `0x629b60` classifies both
//! endpoints, then branches per outcome into a text formatter; the floating number is a separate
//! path (`0x629d30`, [`super::text`]) with its own CVars and source law. The wording law is
//! [`crate::ui_chat::combat`].
//!
//! Every arm classifies both endpoints, picks the family and chat type, and queues the line for
//! its names. The sentence's subject is not always the packet's `attacker`
//! (`SMSG_SPELLDAMAGESHIELD`).

use bevy::prelude::*;

use benilla_protocol::messages::{
    power_display_scale, AttackerState, DamageShield, DispelFailed, EnchantmentLog,
    EnvironmentalDamageLog, PartyKillLog, PeriodicAuraLog, PeriodicTick, SpellDamageLog,
    SpellDispelLog, SpellEnergizeLog, SpellHealLog, SpellInstaKillLog, SpellLogExecute,
    SpellLogMiss, SpellOutcomeLog,
};

use crate::ui_chat::combat::{self, Family, Fills, LineWord, Poses, Stores, UnitClass};
use crate::ui_chat::ChatLog;

use crate::net::{GuidIndex, Reputations, SelfGuid};

/// The classification inputs every line needs, built per packet from `super::Ctx`.
pub(crate) struct ChatCtx<'a> {
    pub self_guid: &'a SelfGuid,
    pub group: Option<&'a crate::ui_party::GroupState>,
    pub index: &'a GuidIndex,
    pub factions: Option<&'a crate::target::ring::Factions>,
    pub reputations: &'a Reputations,
    pub spells: Option<&'a crate::ui_action::Spells>,
    /// The creature records, whose VISIBLE_TO_GHOSTS bit `CanAttack` reads.
    pub names: Option<&'a crate::names::NameCache>,
    /// The display ranges: the reference's `0x8629e0` table, CVar-backed.
    pub ranges: &'a combat::CombatLogRanges,
    /// `CombatLogPeriodicSpells`, for its two chat-only read sites; the whole-packet gate is in
    /// the `PeriodicAuraLog` handler.
    pub periodic: bool,
}

impl ChatCtx<'_> {
    fn classify(&self, guid: u64, stores: &impl Stores) -> UnitClass {
        combat::classify(
            guid,
            self.self_guid,
            self.group,
            self.index,
            stores,
            self.factions,
            self.reputations,
            self.names,
        )
    }

    /// One endpoint's half of the display-range gate, [`combat::in_range`].
    fn in_range(&self, guid: u64, class: UnitClass, poses: &impl Poses) -> bool {
        combat::in_range(
            guid,
            self.ranges.class(class),
            self.self_guid,
            self.index,
            poses,
        )
    }

    /// A spell's display name, or `None` where the formatter writes no line
    /// ([`combat::logged_spell`]).
    fn spell_name(&self, spell_id: u32) -> Option<String> {
        combat::logged_spell(self.spells, spell_id).map(|d| d.name.clone())
    }

    /// `0x6ea280 == 2`, "this spell targets enemies": the predicate the chat-type stubs
    /// `0x627d30`/`0x627d60` use to choose a family's `…_DAMAGE` or `…_BUFF` row. A spell the
    /// catalog does not know reads as harmful, keeping the `…_DAMAGE` row.
    fn spell_harmful(&self, spell_id: u32) -> bool {
        self.spells
            .and_then(|s| s.catalog.get(spell_id))
            .is_none_or(benilla_formats::SpellDisplay::is_harmful)
    }
}

/// `SMSG_ATTACKERSTATEUPDATE`: the melee line and its word, every `COMBAT_*` family. `school`, the
/// first sub-damage's, selects the `…SCHOOL` template; physical (0) takes the plain one.
pub(super) fn attacker_state(
    mut s: AttackerState,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    s.mark_full_block(ctx.index.0.contains_key(&s.victim));
    // `0x625e40`, checked before the dispatcher (`0x629b3e`, `0x629b45`): a swing from a spell
    // that did not land plainly emits no line; the floating number has the same gate.
    if !s.displayed() {
        return;
    }
    let attacker = ctx.classify(s.attacker, stores);
    let victim = ctx.classify(s.victim, stores);
    // `0x62a710` emits nothing for VictimState 0, 1, 4 or 9.
    let Some((family, word)) = combat::melee_outcome(&s) else {
        return;
    };
    // The chat type pair is hits/misses: only a landed hit is "hits", a dodge or parry "misses".
    let landed = family.stem.starts_with("COMBATHIT");
    let Some(kind) = combat::combat_kind(attacker, victim, !landed) else {
        return;
    };
    let fills = Fills {
        spell: String::new(),
        school: (s.school != 0).then_some(s.school),
        amount: i64::from(s.damage),
        // Only a landed hit grows a trailer, and only here can it say GLANCING, CRUSHING or BLOCK:
        // of `0x628410`'s call sites, this one passes a real `blocked` and `HitInfo` (`0x629ee7`).
        trailers: landed.then_some(combat::Trailers {
            absorbed: s.absorb,
            resisted: s.resist,
            blocked: s.blocked,
            hit_info: s.hit_info,
        }),
        ..Default::default()
    };
    queue_worded(
        log,
        ctx,
        poses,
        kind,
        family,
        (s.attacker, attacker),
        (s.victim, victim),
        fills,
        to_me(victim, word),
    );
}

/// `SMSG_SPELLNONMELEEDAMAGELOG` (`0x5e85e0`): split damage, then a periodic hit, then a direct
/// one, each its own formatter.
pub(super) fn spell_damage_log(
    s: SpellDamageLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let attacker = ctx.classify(s.attacker, stores);
    let victim = ctx.classify(s.target, stores);
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };

    // Split damage (`hit_info & 8`, to `0x62de60`) is tested first, before the periodic split: it
    // reads "%s's %s causes %s %d damage.", has no `…SELFSELF` key and no trailers, and words the
    // split amount to its target (`0x62dfd9`).
    const SPELL_HIT_TYPE_SPLIT: u32 = 0x8;
    if s.hit_info & SPELL_HIT_TYPE_SPLIT != 0 {
        let Some(kind) = combat::spell_kind(attacker, victim, false) else {
            return;
        };
        return queue_worded(
            log,
            ctx,
            poses,
            kind,
            combat::SPELLSPLITDAMAGE,
            (s.attacker, attacker),
            (s.target, victim),
            Fills {
                spell,
                amount: i64::from(s.damage),
                ..Default::default()
            },
            to_me(victim, LineWord::with("SPLIT_DAMAGE", s.damage)),
        );
    }
    if s.periodic {
        // A chat-only `CombatLogPeriodicSpells` read (`0x62d9ae` in `0x62d9a0`): it skips the line
        // emit `0x628100` and nothing else, so the floats still fire.
        if !ctx.periodic {
            return;
        }
        return periodic_damage(
            log,
            ctx,
            poses,
            &spell,
            (s.attacker, attacker),
            (s.target, victim),
            PeriodicHit {
                damage: s.damage,
                school: s.school,
                absorbed: s.absorb,
                resisted: s.resist,
            },
        );
    }
    direct_damage(
        log,
        ctx,
        poses,
        &spell,
        s.spell_id,
        (s.attacker, attacker),
        (s.target, victim),
        &s,
    );
}

/// `0x62cd80`, a direct spell hit. Nothing through is worded as the reason, tested absorbed
/// (`0x6285b0`), blocked (the miss formatter with code 5, `0x62cec7`), then resisted
/// (`0x628720`); nothing at all writes no line. Damage through takes `SPELLLOG` and its crit and
/// school forms, with trailers, and [`combat::spell_hit_word`].
fn direct_damage(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    spell: &str,
    spell_id: u32,
    caster: (u64, UnitClass),
    target: (u64, UnitClass),
    s: &SpellDamageLog,
) {
    let school = Some(s.school);
    if s.damage == 0 {
        if s.absorb != 0 {
            absorbed_line(log, ctx, poses, spell, caster, target, school, false);
        } else if s.blocked != 0 {
            spell_miss(
                log, ctx, poses, spell, spell_id, caster, target, 5, false, school,
            );
        } else if s.resist != 0 {
            resisted_line(log, ctx, poses, spell, caster, target, school, false);
        }
        return;
    }
    let Some(kind) = combat::spell_kind(caster.1, target.1, false) else {
        return;
    };
    // `SPELL_HIT_TYPE_CRIT` (vmangos `SpellDefines.h:179`).
    let crit = s.hit_info & 0x2 != 0;
    let family = match (crit, s.school != 0) {
        (false, false) => combat::SPELLLOG,
        (true, false) => combat::SPELLLOGCRIT,
        (false, true) => combat::SPELLLOGSCHOOL,
        (true, true) => combat::SPELLLOGCRITSCHOOL,
    };
    let fills = Fills {
        spell: spell.to_owned(),
        school,
        amount: i64::from(s.damage),
        // `0x62d03c` passes a real `blocked` but HitInfo 0, so never GLANCING or CRUSHING.
        trailers: Some(combat::Trailers {
            absorbed: s.absorb,
            resisted: s.resist,
            blocked: s.blocked,
            hit_info: 0,
        }),
        ..Default::default()
    };
    let word = combat::spell_hit_word(s.damage, s.absorb, s.resist, s.blocked);
    queue_worded(
        log,
        ctx,
        poses,
        kind,
        family,
        caster,
        target,
        fills,
        to_me(target.1, word),
    );
}

/// One periodic damage tick's figures, as `0x628100` takes them.
pub(super) struct PeriodicHit {
    pub damage: u32,
    pub school: u8,
    pub absorbed: u32,
    pub resisted: i32,
}

/// `0x628100`, a periodic damage tick, from `SMSG_PERIODICAURALOG` and from a periodic
/// `SMSG_SPELLNONMELEEDAMAGELOG` alike. Nothing through is worded absorbed, then resisted, on the
/// periodic types (`0x6281e5`, `0x62820e`), or not at all; damage through takes
/// `PERIODICAURADAMAGE`, typed off the target (`0x628235`), with absorb and resist trailers
/// (`0x628341` passes zero for blocked and HitInfo) and [`combat::periodic_hit_word`].
fn periodic_damage(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    spell: &str,
    caster: (u64, UnitClass),
    target: (u64, UnitClass),
    hit: PeriodicHit,
) {
    let school = Some(hit.school);
    if hit.damage == 0 {
        if hit.absorbed != 0 {
            absorbed_line(log, ctx, poses, spell, caster, target, school, true);
        } else if hit.resisted != 0 {
            resisted_line(log, ctx, poses, spell, caster, target, school, true);
        }
        return;
    }
    let Some(kind) = combat::periodic_kind(target.1, false) else {
        return;
    };
    let fills = Fills {
        spell: spell.to_owned(),
        school,
        amount: i64::from(hit.damage),
        trailers: Some(combat::Trailers {
            absorbed: hit.absorbed,
            resisted: hit.resisted,
            blocked: 0,
            hit_info: 0,
        }),
        ..Default::default()
    };
    let word = combat::periodic_hit_word(hit.damage, hit.absorbed, hit.resisted);
    queue_worded(
        log,
        ctx,
        poses,
        kind,
        combat::PERIODICAURADAMAGE,
        caster,
        target,
        fills,
        to_me(target.1, word),
    );
}

/// `0x6285b0`, a hit absorbed whole: `SPELLLOGABSORB`, typed by the damage matrix, or by the
/// periodic selector on the target when `periodic` (`0x628603`), and worded `SPELL_ABSORBED` with
/// the spell's name (`0x628696`). `school` is the caller's school-name gate.
fn absorbed_line(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    spell: &str,
    caster: (u64, UnitClass),
    target: (u64, UnitClass),
    school: Option<u8>,
    periodic: bool,
) {
    reason_line(
        log,
        ctx,
        poses,
        combat::SPELLLOGABSORB,
        LineWord::with("SPELL_ABSORBED", spell),
        spell,
        caster,
        target,
        school,
        periodic,
    );
}

/// `0x628720`, a hit resisted whole: [`absorbed_line`]'s shape, `SPELLRESIST` worded
/// `SPELL_RESISTED` (`0x628806`).
fn resisted_line(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    spell: &str,
    caster: (u64, UnitClass),
    target: (u64, UnitClass),
    school: Option<u8>,
    periodic: bool,
) {
    reason_line(
        log,
        ctx,
        poses,
        combat::SPELLRESIST,
        LineWord::with("SPELL_RESISTED", spell),
        spell,
        caster,
        target,
        school,
        periodic,
    );
}

/// The body `0x6285b0` and `0x628720` share.
fn reason_line(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    family: Family,
    word: LineWord,
    spell: &str,
    caster: (u64, UnitClass),
    target: (u64, UnitClass),
    school: Option<u8>,
    periodic: bool,
) {
    let kind = if periodic {
        combat::periodic_kind(target.1, false)
    } else {
        combat::spell_kind(caster.1, target.1, false)
    };
    let Some(kind) = kind else {
        return;
    };
    queue_worded(
        log,
        ctx,
        poses,
        kind,
        family,
        caster,
        target,
        Fills {
            spell: spell.to_owned(),
            school,
            ..Default::default()
        },
        to_me(target.1, word),
    );
}

/// `0x62bab0`, the spell-miss formatter, one line per missed target. Code 0 writes nothing
/// (`0x62babb`). `damage_shield` is its fifth argument: set, the line is typed by the
/// damage-shield selector on the caster (`0x62c140`), clear, by the spell's own damage or buff
/// matrix (`0x627d30`). The word carries the spell's name ([`combat::miss_word`]). `school` is a
/// caller's school-name gate, `0x62cd80`'s.
pub(super) fn spell_miss(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    spell: &str,
    spell_id: u32,
    caster: (u64, UnitClass),
    target: (u64, UnitClass),
    miss_info: u8,
    damage_shield: bool,
    school: Option<u8>,
) {
    if miss_info == 0 {
        return;
    }
    let kind = if damage_shield {
        Some(combat::damage_shield_kind(caster.1))
    } else {
        combat::spell_kind(caster.1, target.1, !ctx.spell_harmful(spell_id))
    };
    let Some(kind) = kind else {
        return;
    };
    queue_worded(
        log,
        ctx,
        poses,
        kind,
        combat::miss_family(miss_info),
        caster,
        target,
        Fills {
            spell: spell.to_owned(),
            school,
            ..Default::default()
        },
        to_me(target.1, combat::miss_word(miss_info, spell)),
    );
}

/// `SMSG_SPELLLOGMISS` (`0x5e7e00`): one line per missed target, worded by its own
/// `SpellMissInfo`. Its caller passes the formatter's fifth argument as 1 (`0x5e7f31`), so every
/// line off this packet is typed as a damage-shield line; addons filter on the chat type the
/// reference shows.
pub(super) fn spell_log_miss(
    s: &SpellLogMiss,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let caster = ctx.classify(s.caster, stores);
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    for &(target, miss_info) in &s.misses {
        let victim = ctx.classify(target, stores);
        spell_miss(
            log,
            ctx,
            poses,
            &spell,
            s.spell_id,
            (s.caster, caster),
            (target, victim),
            miss_info,
            true,
            None,
        );
    }
}

/// `SMSG_SPELLHEALLOG`: a heal line, a BUFF type (`0x62c770`), worded `HEAL` or `HEAL_CRIT` with
/// the healer's name and the amount (`0x62c8ea`).
pub(super) fn spell_heal_log(
    s: SpellHealLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let healer = ctx.classify(s.healer, stores);
    let target = ctx.classify(s.target, stores);
    let Some(kind) = combat::spell_kind(healer, target, true) else {
        return;
    };
    let (family, word) = if s.critical {
        (combat::HEALEDCRIT, "HEAL_CRIT")
    } else {
        (combat::HEALED, "HEAL")
    };
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    let fills = Fills {
        spell,
        amount: i64::from(s.amount),
        ..Default::default()
    };
    queue_worded(
        log,
        ctx,
        poses,
        kind,
        family,
        (s.healer, healer),
        (s.target, target),
        fills,
        to_me(target, LineWord::named(word, s.amount)),
    );
}

/// `SMSG_SPELLENERGIZELOG`: a power-gain line, a BUFF type, in the displayed figure: `0x5e8a90`
/// divides by `0x6e7130(powerType)` at `0x5e8af3`, so 10 rage on the wire words as 1.
pub(super) fn spell_energize_log(
    s: SpellEnergizeLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let caster = ctx.classify(s.caster, stores);
    let target = ctx.classify(s.target, stores);
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    power_gain_line(
        log,
        ctx,
        poses,
        &spell,
        (s.caster, caster),
        (s.target, target),
        s.power,
        s.amount,
        false,
    );
}

/// `0x627520`, a power gain, from `SMSG_SPELLENERGIZELOG` with its periodic flag clear and from a
/// periodic energize tick with it set. The flag picks the chat type, the periodic selector on the
/// caster (`0x6275fb`) or the buff matrix, and only a direct gain raises the word (`0x6276e2`).
fn power_gain_line(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    spell: &str,
    caster: (u64, UnitClass),
    target: (u64, UnitClass),
    power: u32,
    amount: u32,
    periodic: bool,
) {
    let kind = if periodic {
        combat::periodic_kind(caster.1, true)
    } else {
        combat::spell_kind(caster.1, target.1, true)
    };
    let Some(kind) = kind else {
        return;
    };
    let amount = power_gain(power, amount);
    let word = combat::power_gain_word(power, amount).filter(|_| !periodic);
    queue_worded(
        log,
        ctx,
        poses,
        kind,
        combat::POWERGAIN,
        caster,
        target,
        Fills {
            spell: spell.to_owned(),
            power: Some(power),
            amount,
            ..Default::default()
        },
        word.and_then(|w| to_me(target.1, w)),
    );
}

/// `SMSG_PERIODICAURALOG` (`0x626dd0`): one line per tick, worded by its aura type.
pub(super) fn periodic_aura_log(
    s: &PeriodicAuraLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let caster = (s.caster, ctx.classify(s.caster, stores));
    let target = (s.target, ctx.classify(s.target, stores));
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    for tick in &s.ticks {
        match *tick {
            PeriodicTick::Damage {
                amount,
                school,
                absorb,
                resist,
            } => periodic_damage(
                log,
                ctx,
                poses,
                &spell,
                caster,
                target,
                PeriodicHit {
                    damage: amount,
                    // A u32 on this packet, the same `SPELL_SCHOOL<n>_NAME` index as direct
                    // damage; one past any name fails the formatter's school gate (`0x628117`).
                    school: u8::try_from(school).unwrap_or(u8::MAX),
                    absorbed: absorb,
                    resisted: resist,
                },
            ),
            PeriodicTick::Heal { amount } => {
                periodic_heal(log, ctx, poses, &spell, caster, target, amount)
            }
            // The same displayed-figure divide, at `0x627087` in `0x626dd0`.
            PeriodicTick::Energize { power, amount } => {
                power_gain_line(log, ctx, poses, &spell, caster, target, power, amount, true)
            }
            // Aura 64 goes to the same formatter as the execute log's `POWER_DRAIN` (`0x627910`
            // to `0x627930`, periodic flag set), which makes every wording choice.
            PeriodicTick::ManaLeech {
                power,
                amount,
                multiplier,
            } => power_drain_line(
                log,
                ctx,
                stores,
                poses,
                &spell,
                s.spell_id,
                caster,
                target,
                power,
                periodic_leech_amount(power, amount),
                multiplier,
                true,
            ),
        }
    }
}

/// `0x627240`, a periodic heal tick: `PERIODICAURAHEAL`, typed off the target (`0x62732c`), and
/// worded `PERIODIC_HEAL` with the caster's name and the amount (`0x627414`).
fn periodic_heal(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    spell: &str,
    caster: (u64, UnitClass),
    target: (u64, UnitClass),
    amount: u32,
) {
    let Some(kind) = combat::periodic_kind(target.1, true) else {
        return;
    };
    queue_worded(
        log,
        ctx,
        poses,
        kind,
        combat::PERIODICAURAHEAL,
        caster,
        target,
        Fills {
            spell: spell.to_owned(),
            amount: i64::from(amount),
            ..Default::default()
        },
        to_me(target.1, LineWord::named("PERIODIC_HEAL", amount)),
    );
}

/// `SMSG_SPELLDAMAGESHIELD`: the Thorns-style return hit. The sentence's subject is the packet's
/// `victim`, who wears the shield; `attacker` struck it and takes the damage
/// (`DAMAGESHIELDSELFOTHER` is "You reflect %d %s damage to %s."). The chat type is a two-way on
/// the bearer: `0x62c140` returns `0x3f` `SPELL_DAMAGESHIELDS_ON_SELF` for class 0 or 1, else
/// `0x40` `…ON_OTHERS`.
pub(super) fn damage_shield(
    s: DamageShield,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let bearer = ctx.classify(s.victim, stores);
    let struck = ctx.classify(s.attacker, stores);
    let kind = combat::damage_shield_kind(bearer);
    let fills = Fills {
        school: u8::try_from(s.school).ok(),
        amount: i64::from(s.damage),
        ..Default::default()
    };
    queue(
        log,
        ctx,
        poses,
        kind,
        combat::DAMAGESHIELD,
        (s.victim, bearer),
        (s.attacker, struck),
        fills,
    );
}

/// `SMSG_PARTYKILLLOG`: "You have slain %s!" or "%s is slain by %s!". Only killer class 0
/// (`SELFKILLOTHER`) and 2, a party member (`PARTYKILLOTHER`), produce a line (`0x628890`); a
/// pet's kill gets the plain `UNITDIES*` line instead. The chat type comes off the victim.
pub(super) fn party_kill_log(
    s: PartyKillLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let killer = ctx.classify(s.killer, stores);
    let victim = ctx.classify(s.victim, stores);
    let family = match killer {
        UnitClass::Me => combat::SELFKILLOTHER,
        UnitClass::Party => combat::PARTYKILLOTHER,
        _ => return,
    };
    queue(
        log,
        ctx,
        poses,
        combat::death_kind(victim),
        family,
        (s.victim, victim),
        (s.killer, killer),
        Fills::default(),
    );
}

/// `SMSG_SPELLINSTAKILLLOG`: "You are killed by %s." or "%s is killed by %s.". `0x62cbe0` passes
/// the victim's class in both selector positions (`0x626be0(class, class)`).
pub(super) fn spell_insta_kill_log(
    s: SpellInstaKillLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let victim = ctx.classify(s.victim, stores);
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    let Some(kind) = combat::spell_kind(victim, victim, false) else {
        return;
    };
    queue(
        log,
        ctx,
        poses,
        kind,
        combat::INSTAKILL,
        (s.victim, victim),
        (s.victim, victim),
        Fills {
            spell,
            ..Default::default()
        },
    );
}

/// `SMSG_PROCRESIST` ("%s resists %s's %s.") and `SMSG_SPELLORDAMAGE_IMMUNE` ("%s is immune to
/// %s's %s."), both worded target first and typed off the (caster, target) pair.
///
/// `IMMUNESPELL`'s `log_format` byte is the periodic flag: `0x62d25f` in `0x62d240` gates that
/// formatter alone on `CombatLogPeriodicSpells` when the byte is set; `PROCRESIST` is ungated.
pub(super) fn spell_outcome_log(
    s: SpellOutcomeLog,
    immune: bool,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    // The periodic gate, on `IMMUNESPELL` alone.
    if immune && s.log_format != 0 && !ctx.periodic {
        return;
    }
    let caster = ctx.classify(s.caster, stores);
    let target = ctx.classify(s.target, stores);
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    // `PROCRESIST` goes through the damage/buff stub `0x627d30`, which asks the spell;
    // `IMMUNESPELL` through the damage selector `0x626be0`, so an immunity is always `…_DAMAGE`.
    let buff = !immune && !ctx.spell_harmful(s.spell_id);
    let Some(kind) = combat::spell_kind(caster, target, buff) else {
        return;
    };
    queue(
        log,
        ctx,
        poses,
        kind,
        if immune {
            combat::IMMUNESPELL
        } else {
            combat::PROCRESIST
        },
        (s.caster, caster),
        (s.target, target),
        Fills {
            spell,
            ..Default::default()
        },
    );
}

/// `SMSG_SPELLDISPELLOG`: "Your %s is removed." or "%s's %s is removed.", one line per aura. The
/// chat type is the literal `0x45` `SPELL_BREAK_AURA` (`0x62d480`); the dispeller is classified
/// only for the range gate.
pub(super) fn spell_dispel_log(
    s: &SpellDispelLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let bearer = ctx.classify(s.victim, stores);
    let caster = ctx.classify(s.caster, stores);
    for &spell_id in &s.spell_ids {
        let Some(spell) = ctx.spell_name(spell_id) else {
            continue;
        };
        queue(
            log,
            ctx,
            poses,
            crate::ui_chat::ChatEventKind::SpellBreakAura,
            combat::AURADISPEL,
            (s.victim, bearer),
            (s.caster, caster),
            Fills {
                spell,
                ..Default::default()
            },
        );
    }
}

/// `SMSG_DISPEL_FAILED`: "You fail to dispel %s's %s.", one line per aura. The format variant is
/// picked once per packet (`0x628c20`); the chat type per line, since `0x627d30` asks the spell.
pub(super) fn dispel_failed(
    s: &DispelFailed,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let caster = ctx.classify(s.caster, stores);
    let victim = ctx.classify(s.victim, stores);
    for &spell_id in &s.spell_ids {
        let Some(spell) = ctx.spell_name(spell_id) else {
            continue;
        };
        // Per line: `0x627d30` asks the spell.
        let Some(kind) = combat::spell_kind(caster, victim, !ctx.spell_harmful(spell_id)) else {
            continue;
        };
        queue(
            log,
            ctx,
            poses,
            kind,
            combat::DISPELFAILED,
            (s.caster, caster),
            (s.victim, victim),
            Fills {
                spell,
                ..Default::default()
            },
        );
    }
}

/// `SMSG_ENCHANTMENTLOG`: "You cast %s on your %s." or "%s has faded from your %s.". An empty
/// caster guid means the enchant faded (`0x628f40`); the fade names only the owner. The item name,
/// the last `%s`, comes from the item cache, so an uncached entry waits.
///
/// The chat type is the literal `0x44` `SPELL_ITEM_ENCHANTMENTS` for a fade and for the owner's
/// copy (`show_affiliation` clear); the broadcast copy takes the BUFF selector.
pub(super) fn enchantment_log(
    s: EnchantmentLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let owner = ctx.classify(s.owner, stores);
    let caster = ctx.classify(s.caster, stores);
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    let fills = Fills {
        spell,
        ..Default::default()
    };
    let enchantments = crate::ui_chat::ChatEventKind::SpellItemEnchantments;
    if s.caster == 0 {
        // A fade: the owner is the only endpoint, in both the key and the range gate.
        return queue_named(
            log,
            ctx,
            poses,
            enchantments,
            combat::ITEMENCHANTMENTREMOVE,
            (s.owner, owner),
            (s.owner, owner),
            fills,
            combat::Named::Item(s.item_entry),
        );
    }
    let kind = if s.show_affiliation {
        match combat::spell_kind(caster, owner, true) {
            Some(k) => k,
            None => return,
        }
    } else {
        enchantments
    };
    queue_named(
        log,
        ctx,
        poses,
        kind,
        combat::ITEMENCHANTMENTADD,
        (s.caster, caster),
        (s.owner, owner),
        fills,
        combat::Named::Item(s.item_entry),
    );
}

/// `SMSG_SPELLLOGEXECUTE`: the lines a cast's effects produce, one formatter per effect id via
/// the reference's jump table `0x5e8074`.
///
/// Not built: effects 33 and 59 `OPEN_LOCK` (its `%s` is a gameobject name, and there is no
/// gameobject-name cache), and the `SIMPLECAST*`/`SIMPLEPERFORM*`/`SPELLTERSE_*` fallback, whose
/// bail reads `AttributesEx4` and `Spell.dbc` column 10, not parsed into `SpellDisplay`.
pub(super) fn spell_log_execute(
    s: &SpellLogExecute,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    use benilla_protocol::messages::ExecuteLog as E;

    let caster = ctx.classify(s.caster, stores);
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    let spell_fill = || Fills {
        spell: spell.clone(),
        ..Default::default()
    };
    for (_effect, rows) in &s.effects {
        for row in rows {
            match *row {
                // Effect 8 POWER_DRAIN: [`power_drain_line`] (`0x627930`), periodic flag clear.
                E::PowerDrain {
                    target,
                    amount,
                    power,
                    multiplier,
                } => {
                    let victim = ctx.classify(target, stores);
                    power_drain_line(
                        log,
                        ctx,
                        stores,
                        poses,
                        &spell,
                        s.spell_id,
                        (s.caster, caster),
                        (target, victim),
                        power,
                        amount,
                        multiplier,
                        false,
                    );
                }
                // Effect 19 ADD_EXTRA_ATTACKS: no caster in the sentence (`0x62d9f0`), only the
                // unit that gained the attacks.
                E::ExtraAttacks { target, count } => {
                    let victim = ctx.classify(target, stores);
                    let Some(kind) = combat::spell_kind(victim, victim, false) else {
                        continue;
                    };
                    queue(
                        log,
                        ctx,
                        poses,
                        kind,
                        if count == 1 {
                            combat::SPELLEXTRAATTACKS_SINGULAR
                        } else {
                            combat::SPELLEXTRAATTACKS
                        },
                        (target, victim),
                        (target, victim),
                        Fills {
                            spell: spell.clone(),
                            amount: i64::from(count),
                            ..Default::default()
                        },
                    );
                }
                // Effect 24 CREATE_ITEM: the tradeskill line, literal type `0x3e`, split on the
                // caster being you; no target on the wire.
                E::CreateItem { item_entry } => queue_named(
                    log,
                    ctx,
                    poses,
                    crate::ui_chat::ChatEventKind::SpellTradeskills,
                    combat::TRADESKILL_LOG,
                    (s.caster, caster),
                    (s.caster, caster),
                    Fills::default(),
                    combat::Named::Item(item_entry),
                ),
                // Effect 101 FEED_PET: CREATE_ITEM's shape, a different sentence.
                E::FeedPet { item_entry } => queue_named(
                    log,
                    ctx,
                    poses,
                    crate::ui_chat::ChatEventKind::SpellTradeskills,
                    combat::FEEDPET_LOG,
                    (s.caster, caster),
                    (s.caster, caster),
                    Fills::default(),
                    combat::Named::Item(item_entry),
                ),
                // Effect 68 INTERRUPT_CAST: the sentence names the interrupted spell, off the row,
                // not the packet's own.
                E::InterruptCast { target, spell_id } => {
                    let victim = ctx.classify(target, stores);
                    let Some(interrupted) = ctx.spell_name(spell_id) else {
                        continue;
                    };
                    let Some(kind) = combat::spell_kind(caster, victim, false) else {
                        continue;
                    };
                    queue(
                        log,
                        ctx,
                        poses,
                        kind,
                        combat::SPELLINTERRUPT,
                        (s.caster, caster),
                        (target, victim),
                        Fills {
                            spell: interrupted,
                            ..Default::default()
                        },
                    );
                }
                // Effect 111 DURABILITY_DAMAGE: both fields `-1` is the "all items" form.
                E::DurabilityDamage {
                    target,
                    item_entry,
                    slot,
                } => {
                    let victim = ctx.classify(target, stores);
                    let Some(kind) = combat::spell_kind(caster, victim, false) else {
                        continue;
                    };
                    if item_entry < 0 && slot < 0 {
                        queue(
                            log,
                            ctx,
                            poses,
                            kind,
                            combat::SPELLDURABILITYDAMAGEALL,
                            (s.caster, caster),
                            (target, victim),
                            spell_fill(),
                        );
                        continue;
                    }
                    let Ok(entry) = u32::try_from(item_entry) else {
                        continue;
                    };
                    queue_named(
                        log,
                        ctx,
                        poses,
                        kind,
                        combat::SPELLDURABILITYDAMAGE,
                        (s.caster, caster),
                        (target, victim),
                        spell_fill(),
                        combat::Named::Item(entry),
                    );
                }
                // Effect 102 DISMISS_PET, in the guid-only tail: split on the caster being you, at
                // the literal misc-info type, naming the pet.
                E::Target { target } if *_effect == EFFECT_DISMISS_PET => {
                    let pet = ctx.classify(target, stores);
                    queue_named(
                        log,
                        ctx,
                        poses,
                        crate::ui_chat::ChatEventKind::CombatMiscInfo,
                        combat::SPELLDISMISSPET,
                        (s.caster, caster),
                        (target, pet),
                        Fills::default(),
                        combat::Named::Unit(target),
                    );
                }
                // Heals and energizes are worded from their own packets; the rest of the guid-only
                // tail is the unbuilt SIMPLECAST fallback.
                E::Heal { .. } | E::Energize { .. } | E::Target { .. } => {}
            }
        }
    }
}

/// vmangos `Powers` happiness, the one power with no `…_POINTS` GlobalString (`0x6278f0` returns
/// NULL), so it takes its own family.
const POWER_HAPPINESS: u32 = 4;

/// vmangos `SpellEffects::SPELL_EFFECT_DISMISS_PET`.
const EFFECT_DISMISS_PET: u32 = 102;

/// `|multiplier| >= 2^-22`: the reference's leech/drain discriminator (`[0x8029d4]`).
const LEECH_EPSILON: f32 = 1.0 / 4_194_304.0;

/// `0x6e7130(powerType)`: [`power_display_scale`]'s table, widened to `i64`.
fn power_divisor(power: u32) -> i64 {
    i64::from(power_display_scale(power))
}

/// The figure a `POWERGAIN` line words: the wire amount on the display scale, as the reference's
/// handlers divide it once for both chat and `COMBAT_TEXT_UPDATE` (`0x5e8af3`, `0x627087`).
fn power_gain(power: u32, amount: u32) -> i64 {
    i64::from(amount) / power_divisor(power)
}

/// Arm 5 of `0x627930`: a real multiplier means the caster gained what the target lost.
/// `0x6279ff` compares `|multiplier|` with `[0x8029d4]` = `2^-22`, and equality is a leech.
fn leech_family(multiplier: f32) -> Family {
    if multiplier.abs() >= LEECH_EPSILON {
        combat::SPELLPOWERLEECH
    } else {
        combat::SPELLPOWERDRAIN
    }
}

/// The `(drained, gained)` pair a leech or drain sentence words, or `None` when the line drops.
/// `drained` is an integer divide of the wire amount (`0x627ad9 idiv`), and zero drops the line
/// (`0x627ae3`); `gained` is [`rounded_product`] divided the same way.
fn leech_figures(power: u32, amount: u32, multiplier: f32) -> Option<(i64, i64)> {
    let div = power_divisor(power);
    let drained = i64::from(amount) / div;
    (drained != 0).then(|| {
        (
            drained,
            i64::from(rounded_product(amount, multiplier)) / div,
        )
    })
}

/// `amount * multiplier` reduced to an integer as `0x627a8f`..`0x627ac9` does it: the product is
/// narrowed to f32 (`fst`, `0x627a95`), doubled, biased by -0.5 or +0.5 on its sign
/// (`0x627aa5`/`0x627ab5`, `[0x8628f4] = 0.5`), `fistp`-ed half-to-even and shifted right one
/// (`0x627ac9`). For a non-negative product that is `floor`; the f32 narrowing shows at the
/// edges.
fn rounded_product(amount: u32, multiplier: f32) -> i32 {
    let product = (f64::from(amount) * f64::from(multiplier)) as f32; // the `fst` narrowing
    let biased = f64::from(product).mul_add(2.0, if product < 0.0 { 0.5 } else { -0.5 });
    // `fistp` under the default control word: round half to even.
    (biased.round_ties_even() as i32) >> 1
}

/// The periodic handler's pre-divide: `0x627126`/`0x627138` scales the tick by
/// `0x6e7130(powerType)` before `0x627930` divides by the same table again. A reference bug,
/// reproduced so the sentence's number matches (addons parse these lines). Mana's divisor is 1,
/// so only a non-mana leech is affected.
fn periodic_leech_amount(power: u32, amount: u32) -> u32 {
    (i64::from(amount) / power_divisor(power)) as u32
}

/// `0x627930`, the power leech and drain formatter: `SMSG_PERIODICAURALOG` aura 64 enters through
/// `0x627910` with the periodic flag set, `SMSG_SPELLLOGEXECUTE` effect 8 with it clear.
///
/// The arms, in the order the bytes test them:
/// 1. `powerType == 4` tries `0x627de0` first (`0x62793c`), and leaves only if it emitted
///    (`0x627955`); otherwise a happiness drain words the generic line with `HAPPINESS_POINTS`.
/// 2. `powerType >= 5`, unsigned, has no power noun (`0x6278f0`, `0x627966`): no line.
/// 3. The spell gates, the caller's [`ChatCtx::spell_name`].
/// 4. The resolve and range gate `0x626630`, [`queue`]'s.
/// 5. The leech/drain fork, [`leech_family`].
/// 6. The periodic flag picks the chat-type stub, `0x627d60` periodic or `0x627d30` direct.
/// 7. `drained == 0` drops the line (`0x627ae5`), after the divide.
fn power_drain_line(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    spell: &str,
    spell_id: u32,
    caster: (u64, UnitClass),
    target: (u64, UnitClass),
    power: u32,
    amount: u32,
    multiplier: f32,
    periodic: bool,
) {
    // Arm 1: the happiness sentence consumes the packet only when it emits.
    if power == POWER_HAPPINESS && happiness_drain_line(log, ctx, stores, poses, target, amount) {
        return;
    }
    // Arm 2: no noun, no line.
    if !combat::power_has_word(power) {
        return;
    }
    // Arms 5 and 7's arithmetic.
    let Some((drained, gained)) = leech_figures(power, amount, multiplier) else {
        return;
    };
    let family = leech_family(multiplier);
    // Arm 6: both stubs ask the spell, `0x6ea280 == 2` (`0x627d30` to `0x626be0` damage or
    // `0x627820` buff; `0x627d60` over `0x627d80`/`0x6274a0`), so a helpful drain is `…_BUFF`.
    let buff = !ctx.spell_harmful(spell_id);
    // The periodic stub hands its selector the caster (`0x627a0f`/`0x627a3a`).
    let kind = if periodic {
        combat::periodic_kind(caster.1, buff)
    } else {
        combat::spell_kind(caster.1, target.1, buff)
    };
    let Some(kind) = kind else {
        return;
    };
    queue(
        log,
        ctx,
        poses,
        kind,
        family,
        caster,
        target,
        Fills {
            spell: spell.to_owned(),
            power: Some(power),
            power2: Some(power),
            amount: drained,
            amount2: gained,
            ..Default::default()
        },
    );
}

/// `0x627de0`, the happiness sentence: the subject is the pet's owner, the named thing the pet,
/// the chat type the literal `0x19` misc-info. Returns whether it emitted, since `0x627930` falls
/// through to the generic path when it did not. The divisor is happiness's 1000 (`0x627f45`).
fn happiness_drain_line(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    pet: (u64, UnitClass),
    amount: u32,
) -> bool {
    let Some(owner) = unit_owner(pet.0, ctx, stores) else {
        return false; // the `+0x110` pet-to-owner chain did not resolve: fall through
    };
    let owner_class = ctx.classify(owner, stores);
    queue_reported(
        log,
        ctx,
        poses,
        crate::ui_chat::ChatEventKind::CombatMiscInfo,
        combat::SPELLHAPPINESSDRAIN,
        (owner, owner_class),
        pet,
        Fills {
            amount: i64::from(amount) / power_divisor(POWER_HAPPINESS),
            ..Default::default()
        },
        combat::Named::Unit(pet.0),
        None,
    )
}

/// A unit's owner guid, `CHARMEDBY` then `CREATEDBY`, the pair [`combat::classify`] reads.
fn unit_owner(guid: u64, ctx: &ChatCtx, stores: &impl Stores) -> Option<u64> {
    let entity = ctx.index.0.get(&guid).copied()?;
    let store = stores.store(entity)?;
    store
        .0
        .unit_charmed_by()
        .or_else(|| store.0.unit_created_by())
}

/// `SMSG_ENVIRONMENTALDAMAGELOG`: "You fall and lose %d health." and its five siblings.
/// `0x62aac0` builds the key from a six-entry damage-type table and SELF/OTHER, and calls the
/// melee HITS selector with the victim's class in both positions, so a party member's fall types
/// as `HOSTILEPLAYER_HITS` through the selector's tgt<=3 override, as in the reference. It takes
/// absorb and resist trailers.
pub(super) fn environmental_damage_log(
    e: EnvironmentalDamageLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let victim = ctx.classify(e.victim, stores);
    let Some(family) = combat::env_family(e.damage_type) else {
        return;
    };
    let Some(kind) = combat::combat_kind(victim, victim, false) else {
        return;
    };
    queue(
        log,
        ctx,
        poses,
        kind,
        family,
        (e.victim, victim),
        (e.victim, victim),
        Fills {
            amount: i64::from(e.damage),
            trailers: Some(combat::Trailers {
                absorbed: e.absorb,
                resisted: e.resist,
                // The environmental call site passes zero for both.
                blocked: 0,
                hit_info: 0,
            }),
            ..Default::default()
        },
    );
}

/// `SMSG_SET_FACTION_STANDING`: "Your %s reputation has increased by %d.". The wire carries the
/// new total and the sentence the delta, so this runs before the store is overwritten, and an
/// unchanged value prints nothing (`0x4d6344`). Only the packet's first entry prints, and only if
/// its faction is not `HIDDEN`: the handler passes `0x4d6330` its print argument as
/// `index == 0 && !(flags & 4)` (`0x4d57dd`-`0x4d57ea`), so the spill-over entries of one kill
/// update silently. No range gate or classifier: the chat type is the literal `0x55`
/// `COMBAT_FACTION_CHANGE`. The wire's list id is `Faction.dbc`'s `rep_index`.
pub(crate) fn faction_standing(
    deltas: &[(u32, i32)],
    reputations: &Reputations,
    factions: Option<&crate::target::ring::Factions>,
    log: &mut ChatLog,
) {
    let Some(catalog) = factions.map(|f| f.catalog()) else {
        return;
    };
    for &(list_id, standing) in deltas.iter().take(1) {
        let (flags, old) = reputations
            .slots
            .get(list_id as usize)
            .copied()
            .unwrap_or((0, 0));
        if flags & benilla_formats::faction_flags::HIDDEN != 0 {
            continue;
        }
        let delta = standing - old;
        if delta == 0 {
            continue;
        }
        let Ok(index) = i32::try_from(list_id) else {
            continue;
        };
        let Some(name) = catalog
            .reputation_factions()
            .find(|(_, f)| f.rep_index == index)
            .and_then(|(id, _)| catalog.faction_name(id))
        else {
            continue;
        };
        log.push_combat(combat::PendingCombat {
            kind: crate::ui_chat::ChatEventKind::CombatFactionChange,
            family: if delta > 0 {
                combat::FACTION_STANDING_INCREASED
            } else {
                combat::FACTION_STANDING_DECREASED
            },
            // A `Single` family reads no variant; `OtherOther` is what `tests::variants_of` sweeps.
            variant: combat::Variant::OtherOther,
            subject: 0,
            object: 0,
            fills: Fills {
                named: name.to_string(),
                amount: i64::from(delta.abs()),
                ..Default::default()
            },
            named: combat::Named::Ready,
            tries: 0,
            word: None,
        });
    }
}

/// The tail every arm ends in: build the queued line (dropping a class-9 endpoint) and park it
/// for its names.
fn queue(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    kind: crate::ui_chat::ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    object: (u64, UnitClass),
    fills: Fills,
) {
    queue_reported(
        log,
        ctx,
        poses,
        kind,
        family,
        subject,
        object,
        fills,
        combat::Named::Ready,
        None,
    );
}

/// [`queue`] for a family whose `Named` slot (an item entry or unit guid) the drain resolves,
/// holding the line until it lands (`0x6294b0`).
fn queue_named(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    kind: crate::ui_chat::ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    object: (u64, UnitClass),
    fills: Fills,
    named: combat::Named,
) {
    queue_reported(
        log, ctx, poses, kind, family, subject, object, fills, named, None,
    );
}

/// [`queue`] for a formatter that raises a `COMBAT_TEXT_UPDATE` after its line: the word rides
/// the line and fires only once the line is written.
fn queue_worded(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    kind: crate::ui_chat::ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    object: (u64, UnitClass),
    fills: Fills,
    word: Option<LineWord>,
) {
    queue_reported(
        log,
        ctx,
        poses,
        kind,
        family,
        subject,
        object,
        fills,
        combat::Named::Ready,
        word,
    );
}

/// A word, for a line about you: each formatter tests the class of the unit its line is about
/// for 0 before raising one (`0x629ef1`, `0x62d046`, `0x62834b`).
fn to_me(class: UnitClass, word: LineWord) -> Option<LineWord> {
    (class == UnitClass::Me).then_some(word)
}

/// [`queue_named`], answering whether a line was queued: `0x627de0` reports it to `0x627930` in
/// `al` (set at `0x627fb0`/`0x627fe2`/`0x627ff4`, cleared at `0x628006`), and a failed range gate
/// or empty template is a fall-through there.
fn queue_reported(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &impl Poses,
    kind: crate::ui_chat::ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    object: (u64, UnitClass),
    fills: Fills,
    named: combat::Named,
    word: Option<LineWord>,
) -> bool {
    // An OR, not an AND (`0x626630`): either endpoint within its class's range shows the line.
    if !ctx.in_range(subject.0, subject.1, poses) && !ctx.in_range(object.0, object.1, poses) {
        return false;
    }
    let Some(mut line) = combat::queue(kind, family, subject, object, fills, named) else {
        return false;
    };
    line.word = word;
    log.push_combat(line);
    true
}

#[cfg(test)]
mod tests;
