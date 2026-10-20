//! The spell lines: damage, misses, heals, power gains and drains, direct and periodic, one fn
//! per reference formatter, each shared by every packet that reaches it.

use benilla_protocol::messages::{
    power_display_scale, PeriodicAuraLog, PeriodicTick, SpellDamageLog, SpellEnergizeLog,
    SpellHealLog, SpellLogMiss,
};

use super::{queue_reported, queue_worded, to_me, ChatCtx};
use crate::combat_log::feedback;
use crate::ui_chat::combat::{self, Family, Fills, LineWord, Poses, Stores, UnitClass};
use crate::ui_chat::ChatLog;

/// `SMSG_SPELLNONMELEEDAMAGELOG` (`0x5e85e0`): the line (`0x5e87de`), then, for a spell with a
/// record and no `Attributes & 0x100` (`0x5e87e3`-`0x5e87f4`), `UNIT_COMBAT` `WOUND` on the target
/// (`0x5e883f`) with the crit and partial flags, or none for split damage (`0x5e872d`-`0x5e876a`).
pub(in crate::combat_log) fn spell_damage_log(
    s: SpellDamageLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    spell_damage_line(&s, ctx, stores, poses, log);
    if combat::feedback_spell(ctx.spells, s.spell_id).is_some() {
        let flags = if s.hit_info & SPELL_HIT_TYPE_SPLIT != 0 {
            0
        } else {
            feedback::hit_flags(s.hit_info & 0x2 != 0, s.absorb, s.resist, s.blocked)
        };
        feedback::wound(log, s.target, s.damage, flags, u32::from(s.school));
    }
}

/// vmangos `SPELL_HIT_TYPE_SPLIT`, the split-damage bit of the packet's hit info.
const SPELL_HIT_TYPE_SPLIT: u32 = 0x8;

/// The spell-damage line: split damage, then a periodic hit, then a direct one, each its own
/// formatter.
fn spell_damage_line(
    s: &SpellDamageLog,
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
        s,
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
pub(in crate::combat_log) struct PeriodicHit {
    pub damage: u32,
    pub school: u8,
    pub absorbed: u32,
    pub resisted: i32,
}

/// `0x628100`, a periodic damage tick, from `SMSG_PERIODICAURALOG` and from a periodic
/// `SMSG_SPELLNONMELEEDAMAGELOG` alike. Nothing through is worded absorbed, then resisted, on the
/// periodic types (`0x6281e5`, `0x62820e`), or not at all; damage through takes
/// `PERIODICAURADAMAGE`, typed off the target (`0x628235`), with absorb and resist trailers
/// (`0x628341` passes zero for blocked and HitInfo) and [`combat::damage_word`].
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
    let word = combat::damage_word(hit.damage, hit.absorbed, hit.resisted);
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
pub(in crate::combat_log) fn spell_miss(
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

/// `SMSG_SPELLLOGMISS` (`0x5e7e00`): per missed target of a spell with a record and no
/// `Attributes & 0x100` (`0x5e7f0b`-`0x5e7f18`; any other spell has neither), `UNIT_COMBAT`
/// (`0x5e7f23`), then the line, worded by its own `SpellMissInfo` (`0x5e7f3e`). Its caller passes
/// the formatter's fifth argument as 1 (`0x5e7f31`), so every line off this packet is typed as a
/// damage-shield line; addons filter on the chat type the reference shows.
pub(in crate::combat_log) fn spell_log_miss(
    s: &SpellLogMiss,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    if combat::feedback_spell(ctx.spells, s.spell_id).is_none() {
        return;
    }
    let caster = ctx.classify(s.caster, stores);
    let spell = ctx.spell_name(s.spell_id);
    for &(target, miss_info) in &s.misses {
        feedback::miss(log, target, miss_info);
        let Some(spell) = &spell else { continue };
        let victim = ctx.classify(target, stores);
        spell_miss(
            log,
            ctx,
            poses,
            spell,
            s.spell_id,
            (s.caster, caster),
            (target, victim),
            miss_info,
            true,
            None,
        );
    }
}

/// `SMSG_SPELL_GO`'s miss list (`0x6e7d65`-`0x6e7e71`): per entry its miss line, with the
/// ordinary spell chat types (flag 0, `0x6e7d83`), then for a spell without `Attributes & 0x100`
/// (`0x6e7d9a`) its `UNIT_COMBAT` on the target (`0x6e7de5`). A reflect with an outcome
/// (`0x6e7ded`-`0x6e7dfb`) then takes the reflected spell's line, its outcome from `caster` on
/// `reflected_to`, the packet's second guid (`0x6e7dfd`-`0x6e7e14`), and that outcome's
/// `UNIT_COMBAT` on the same guid (`0x6e7e5f`).
pub(in crate::combat_log) fn spell_go_misses(
    caster: u64,
    reflected_to: u64,
    spell_id: u32,
    misses: &[benilla_protocol::messages::SpellGoMiss],
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let spell = ctx.spell_name(spell_id);
    let flashes = combat::feedback_spell(ctx.spells, spell_id).is_some();
    let caster = (caster, ctx.classify(caster, stores));
    let line = |log: &mut ChatLog, target, code| {
        if let Some(spell) = &spell {
            spell_miss(
                log, ctx, poses, spell, spell_id, caster, target, code, false, None,
            );
        }
    };
    for m in misses {
        line(log, (m.target, ctx.classify(m.target, stores)), m.info);
        if flashes {
            feedback::miss(log, m.target, m.info);
        }
        if m.info == super::super::text::SPELL_MISS_REFLECT && m.reflect_result != 0 {
            let reflected = (reflected_to, ctx.classify(reflected_to, stores));
            line(log, reflected, m.reflect_result);
            if flashes {
                feedback::miss(log, reflected_to, m.reflect_result);
            }
        }
    }
}

/// A travelling spell's missed arrival on a live target (`0x61dd50`): for a spell with a record
/// and no `Attributes & 0x100` (`0x61dda3`), its `UNIT_COMBAT` on the target (`0x61ddcc`), then its
/// miss line, with the ordinary spell chat types (`0x61ddeb`, flag 0), from the missile's caster
/// to its target.
pub(in crate::combat_log) fn missile_miss(
    caster: u64,
    target: u64,
    spell_id: u32,
    code: u8,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    if combat::feedback_spell(ctx.spells, spell_id).is_some() {
        feedback::miss(log, target, code);
    }
    let Some(spell) = ctx.spell_name(spell_id) else {
        return;
    };
    let caster = (caster, ctx.classify(caster, stores));
    let target = (target, ctx.classify(target, stores));
    spell_miss(
        log, ctx, poses, &spell, spell_id, caster, target, code, false, None,
    );
}

/// `SMSG_SPELLHEALLOG` (`0x5e89c0`): `UNIT_COMBAT` `HEAL` on the target, with no spell gate
/// (`0x5e8a48`), then the heal line, a BUFF type (`0x62c770`), worded `HEAL` or `HEAL_CRIT` with
/// the healer's name and the amount (`0x62c8ea`).
pub(in crate::combat_log) fn spell_heal_log(
    s: SpellHealLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    feedback::heal(log, s.target, s.amount, s.critical);
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
pub(in crate::combat_log) fn spell_energize_log(
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
    let word = combat::power_amount_word(power, amount).filter(|_| !periodic);
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

/// `SMSG_PERIODICAURALOG` (`0x626dd0`): one line per tick, worded by its aura type. For a spell
/// without `Attributes & 0x100` a damage tick raises `UNIT_COMBAT` `WOUND` with its absorbed and
/// resisted flags (`0x626f87`-`0x626fb4`) and a heal tick `HEAL` (`0x627033`), each before its line
/// (`0x626ff4`, `0x62705e`); energize (`0x494770` is an empty stub) and leech ticks raise none.
pub(in crate::combat_log) fn periodic_aura_log(
    s: &PeriodicAuraLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &mut ChatLog,
) {
    let caster = (s.caster, ctx.classify(s.caster, stores));
    let target = (s.target, ctx.classify(s.target, stores));
    let flashes = combat::feedback_spell(ctx.spells, s.spell_id).is_some();
    let spell = ctx.spell_name(s.spell_id);
    for tick in &s.ticks {
        match *tick {
            PeriodicTick::Damage {
                amount,
                school,
                absorb,
                resist,
            } => {
                if flashes {
                    let flags = feedback::hit_flags(false, absorb, resist, 0);
                    feedback::wound(log, s.target, amount, flags, school);
                }
                let Some(spell) = &spell else { continue };
                periodic_damage(
                    log,
                    ctx,
                    poses,
                    spell,
                    caster,
                    target,
                    PeriodicHit {
                        damage: amount,
                        // A u32 on this packet, the same `SPELL_SCHOOL<n>_NAME` index as direct
                        // damage; one past any name fails the formatter's school gate
                        // (`0x628117`).
                        school: u8::try_from(school).unwrap_or(u8::MAX),
                        absorbed: absorb,
                        resisted: resist,
                    },
                );
            }
            PeriodicTick::Heal { amount } => {
                if flashes {
                    feedback::heal(log, s.target, amount, false);
                }
                let Some(spell) = &spell else { continue };
                periodic_heal(
                    log, ctx, stores, poses, spell, s.spell_id, caster, target, amount,
                );
            }
            // The same displayed-figure divide, at `0x627087` in `0x626dd0`.
            PeriodicTick::Energize { power, amount } => {
                let Some(spell) = &spell else { continue };
                power_gain_line(log, ctx, poses, spell, caster, target, power, amount, true)
            }
            // Aura 64 goes to the same formatter as the execute log's `POWER_DRAIN` (`0x627910`
            // to `0x627930`, periodic flag set), which makes every wording choice.
            PeriodicTick::ManaLeech {
                power,
                amount,
                multiplier,
            } => {
                let Some(spell) = &spell else { continue };
                power_drain_line(
                    log,
                    ctx,
                    stores,
                    poses,
                    spell,
                    s.spell_id,
                    caster,
                    target,
                    power,
                    periodic_leech_amount(power, amount),
                    multiplier,
                    true,
                );
            }
        }
    }
}

/// `0x627240`, a periodic heal tick: `PERIODICAURAHEAL`, typed off the target (`0x62732c`), and
/// worded `PERIODIC_HEAL` with the caster's name and the amount (`0x627414`), but only for a heal
/// over time of 30 s or less ([`periodic_heal_worded`]).
fn periodic_heal(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    spell: &str,
    spell_id: u32,
    caster: (u64, UnitClass),
    target: (u64, UnitClass),
    amount: u32,
) {
    let Some(kind) = combat::periodic_kind(target.1, true) else {
        return;
    };
    let worded = ctx.spells.is_some_and(|spells| {
        let Some(display) = spells.catalog.get(spell_id) else {
            return false;
        };
        let me = ctx
            .self_guid
            .0
            .and_then(|g| ctx.index.0.get(&g))
            .and_then(|&e| stores.store(e));
        let level = display.skill_level(crate::spell::spell_skill_value(
            me,
            ctx.skill_lines,
            spell_id,
        ));
        periodic_heal_worded(spells.durations.get(display.duration_index), level)
    });
    let word = LineWord::named("PERIODIC_HEAL", amount);
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
        worded.then(|| to_me(target.1, word)).flatten(),
    );
}

/// `0x627240`'s word gate: the aura's `SpellDuration` row (`rec+0x78`; no row, no word,
/// `0x6273d6`), its base plus its per-level term at `level`, the active player's
/// `0x6e3130(spell, 0)`, capped at its maximum (`0x6273e7`-`0x627404`), and the word only when that
/// is 30 000 ms or less (`0x627406`). A long heal over time ticks silently.
fn periodic_heal_worded(row: Option<&benilla_formats::SpellDuration>, level: u32) -> bool {
    let Some(row) = row else {
        return false;
    };
    // `imul`, `add`: 32-bit and wrapping, the level read signed.
    let scaled = (level as i32)
        .wrapping_mul(row.per_level_ms)
        .wrapping_add(row.base_ms);
    let duration = if scaled >= row.max_ms {
        row.max_ms
    } else {
        scaled
    };
    duration <= 30_000
}

/// vmangos `Powers` happiness, the one power with no `…_POINTS` GlobalString (`0x6278f0` returns
/// NULL), so it takes its own family.
const POWER_HAPPINESS: u32 = 4;

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
/// 4. The resolve and range gate `0x626630`, [`queue_reported`]'s.
/// 5. The leech/drain fork, [`leech_family`].
/// 6. The periodic flag picks the chat-type stub, `0x627d60` periodic or `0x627d30` direct.
/// 7. `drained == 0` drops the line (`0x627ae5`), after the divide.
///
/// The line raises the power's word with minus the amount drained when its target is you
/// (`0x627b94`-`0x627c5c`), [`combat::power_amount_word`], periodic or not.
pub(super) fn power_drain_line(
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
    let word = combat::power_amount_word(power, -drained).and_then(|w| to_me(target.1, w));
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
            power: Some(power),
            power2: Some(power),
            amount: drained,
            amount2: gained,
            ..Default::default()
        },
        word,
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

#[cfg(test)]
mod tests;
