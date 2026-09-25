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

use crate::ui_chat::combat::{self, Family, Fills, UnitClass};
use crate::ui_chat::ChatLog;

use crate::net::{GuidIndex, ObjectStore, Reputations, SelfGuid};

/// The classification inputs every line needs, built per packet from `super::Ctx`.
pub(crate) struct ChatCtx<'a> {
    pub self_guid: &'a SelfGuid,
    pub group: Option<&'a crate::ui_party::GroupState>,
    pub index: &'a GuidIndex,
    pub factions: Option<&'a crate::target::ring::Factions>,
    pub reputations: &'a Reputations,
    pub spells: Option<&'a crate::ui_action::Spells>,
    /// The display ranges: the reference's `0x8629e0` table, CVar-backed.
    pub ranges: &'a combat::CombatLogRanges,
    /// `CombatLogPeriodicSpells`, for its two chat-only read sites; the whole-packet gate is in
    /// the `PeriodicAuraLog` handler.
    pub periodic: bool,
}

impl ChatCtx<'_> {
    fn classify(&self, guid: u64, stores: &Query<&mut ObjectStore>) -> UnitClass {
        combat::classify(
            guid,
            self.self_guid,
            self.group,
            self.index,
            stores,
            self.factions,
            self.reputations,
        )
    }

    /// One endpoint's half of the display-range gate, [`combat::in_range`].
    fn in_range(&self, guid: u64, class: UnitClass, poses: &Query<&mut Transform>) -> bool {
        combat::in_range(
            guid,
            self.ranges.class(class),
            self.self_guid,
            self.index,
            poses,
        )
    }

    /// A spell's display name, or `None` when the reference emits no line for it (`0x62cd80`, like
    /// every spell formatter):
    /// - `Attributes & 0x180` at `SpellRec+0x18`, `SPELL_ATTR_DO_NOT_DISPLAY` and
    ///   `SPELL_ATTR_DO_NOT_LOG` (vmangos `SpellDefines.h:799-800`), keeping proc triggers and
    ///   aura tickers out;
    /// - an empty localized name.
    ///
    /// A spell the catalog does not know is not gated: the line still carries its numbers, as the
    /// reference degrades through `GetObjectName`'s `"UKNOWNOBJECT"` tail.
    fn spell_name(&self, spell_id: u32) -> Option<String> {
        let Some(display) = self.spells.and_then(|s| s.catalog.get(spell_id)) else {
            return Some(String::new());
        };
        const DO_NOT_DISPLAY_OR_LOG: u32 = 0x180;
        if display.attributes & DO_NOT_DISPLAY_OR_LOG != 0 || display.name.is_empty() {
            return None;
        }
        Some(display.name.clone())
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

/// `SMSG_ATTACKERSTATEUPDATE`: the melee line, every `COMBAT_*` family. `school`, the first
/// sub-damage's, selects the `…SCHOOL` template; physical (0) takes the plain one.
pub(super) fn attacker_state(
    s: AttackerState,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
    log: &mut ChatLog,
) {
    // `0x625e40`, checked before the dispatcher (`0x629b3e`, `0x629b45`): a swing from a spell
    // that did not land plainly emits no line; the floating number has the same gate.
    if !s.displayed() {
        return;
    }
    let attacker = ctx.classify(s.attacker, stores);
    let victim = ctx.classify(s.victim, stores);
    // `0x62a710` emits nothing for VictimState 0, 1, 4 or 9.
    let Some(family) = combat::melee_family(s.hit_info, s.victim_state, s.damage, s.school) else {
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
    queue(
        log,
        ctx,
        poses,
        kind,
        family,
        (s.attacker, attacker),
        (s.victim, victim),
        fills,
    );
}

/// `SMSG_SPELLNONMELEEDAMAGELOG`: a spell's damage line, or the absorbed, blocked or resisted
/// wording when nothing got through. The `periodic` flag routes it to the `SPELL_PERIODIC_*`
/// types and the `PERIODICAURADAMAGE` wording, as the reference does.
pub(super) fn spell_damage_log(
    s: SpellDamageLog,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
    log: &mut ChatLog,
) {
    let attacker = ctx.classify(s.attacker, stores);
    let victim = ctx.classify(s.target, stores);
    // `SPELL_HIT_TYPE_CRIT` (vmangos `SpellDefines.h:179`), the bit the floating text reads.
    let crit = s.hit_info & 0x2 != 0;
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };

    // Split damage (`hit_info & 8`, to `0x62de60`) is tested first, before the periodic split: it
    // reads "%s's %s causes %s %d damage.", has no `…SELFSELF` key and no trailers.
    const SPELL_HIT_TYPE_SPLIT: u32 = 0x8;
    if s.hit_info & SPELL_HIT_TYPE_SPLIT != 0 {
        let Some(kind) = combat::spell_kind(attacker, victim, false) else {
            return;
        };
        return queue(
            log,
            ctx,
            poses,
            kind,
            combat::SPELLSPLITDAMAGE,
            (s.attacker, attacker),
            (s.target, victim),
            Fills {
                spell: spell.clone(),
                amount: i64::from(s.damage),
                ..Default::default()
            },
        );
    }
    if s.periodic {
        // A chat-only `CombatLogPeriodicSpells` read (`0x62d9ae` in `0x62d9a0`): it skips the line
        // emit `0x628100` and nothing else, so the floats still fire.
        if !ctx.periodic {
            return;
        }
        // The target's class, not the caster's: the formatter `0x628100` selects on the victim
        // (`0x628235`).
        let Some(kind) = combat::periodic_kind(victim, false) else {
            return;
        };
        let fills = Fills {
            spell,
            school: Some(s.school),
            amount: i64::from(s.damage),
            // `0x628341` passes zero for blocked and HitInfo: absorb and resist trailers only.
            trailers: Some(combat::Trailers {
                absorbed: s.absorb,
                resisted: s.resist,
                blocked: 0,
                hit_info: 0,
            }),
            ..Default::default()
        };
        return queue(
            log,
            ctx,
            poses,
            kind,
            combat::PERIODICAURADAMAGE,
            (s.attacker, attacker),
            (s.target, victim),
            fills,
        );
    }

    let Some(kind) = combat::spell_kind(attacker, victim, false) else {
        return;
    };
    // Nothing through: worded as the reason, tested absorb, block, then resist (`0x62cd80`).
    let family = if s.damage == 0 && s.absorb > 0 {
        combat::SPELLLOGABSORB
    } else if s.damage == 0 && s.blocked > 0 {
        combat::SPELLBLOCKED
    } else if s.damage == 0 && s.resist > 0 {
        combat::SPELLRESIST
    } else {
        match (crit, s.school != 0) {
            (false, false) => combat::SPELLLOG,
            (true, false) => combat::SPELLLOGCRIT,
            (false, true) => combat::SPELLLOGSCHOOL,
            (true, true) => combat::SPELLLOGCRITSCHOOL,
        }
    };
    // `0x62d03c` passes a real `blocked` but HitInfo 0, so never GLANCING or CRUSHING; the
    // absorbed, blocked and resisted wordings take no trailer.
    let landed = family.stem.starts_with("SPELLLOG") && family.stem != "SPELLLOGABSORB";
    let fills = Fills {
        spell,
        school: Some(s.school),
        amount: i64::from(s.damage),
        trailers: landed.then_some(combat::Trailers {
            absorbed: s.absorb,
            resisted: s.resist,
            blocked: s.blocked,
            hit_info: 0,
        }),
        ..Default::default()
    };
    queue(
        log,
        ctx,
        poses,
        kind,
        family,
        (s.attacker, attacker),
        (s.target, victim),
        fills,
    );
}

/// `SMSG_SPELLLOGMISS`: one line per missed target, worded by its own `SpellMissInfo`.
pub(super) fn spell_log_miss(
    s: &SpellLogMiss,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
    log: &mut ChatLog,
) {
    let attacker = ctx.classify(s.caster, stores);
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    // Every line off this packet is typed as a damage-shield line: its caller passes the
    // formatter's 5th argument as 1 (`0x5e7f31`), routing `0x62bc5a` to the damage-shield selector
    // `0x62c140` instead of the spell matrix. Addons filter on the chat type the reference shows.
    let kind = combat::damage_shield_kind(attacker);
    for &(target, miss_info) in &s.misses {
        let family = combat::miss_family(miss_info);
        let victim = ctx.classify(target, stores);
        let fills = Fills {
            spell: spell.clone(),
            ..Default::default()
        };
        queue(
            log,
            ctx,
            poses,
            kind,
            family,
            (s.caster, attacker),
            (target, victim),
            fills,
        );
    }
}

/// `SMSG_SPELLHEALLOG`: a heal line, a BUFF type.
pub(super) fn spell_heal_log(
    s: SpellHealLog,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
    log: &mut ChatLog,
) {
    let healer = ctx.classify(s.healer, stores);
    let target = ctx.classify(s.target, stores);
    let Some(kind) = combat::spell_kind(healer, target, true) else {
        return;
    };
    let family = if s.critical {
        combat::HEALEDCRIT
    } else {
        combat::HEALED
    };
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    let fills = Fills {
        spell,
        amount: i64::from(s.amount),
        ..Default::default()
    };
    queue(
        log,
        ctx,
        poses,
        kind,
        family,
        (s.healer, healer),
        (s.target, target),
        fills,
    );
}

/// `SMSG_SPELLENERGIZELOG`: a power-gain line, a BUFF type, in the displayed figure: `0x5e8a90`
/// divides by `0x6e7130(powerType)` at `0x5e8af3`, so 10 rage on the wire words as 1.
pub(super) fn spell_energize_log(
    s: SpellEnergizeLog,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
    log: &mut ChatLog,
) {
    let caster = ctx.classify(s.caster, stores);
    let target = ctx.classify(s.target, stores);
    let Some(kind) = combat::spell_kind(caster, target, true) else {
        return;
    };
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    let fills = Fills {
        spell,
        power: Some(s.power),
        amount: power_gain(s.power, s.amount),
        ..Default::default()
    };
    queue(
        log,
        ctx,
        poses,
        kind,
        combat::POWERGAIN,
        (s.caster, caster),
        (s.target, target),
        fills,
    );
}

/// `SMSG_PERIODICAURALOG`: one line per tick, worded by its aura type. The periodic chat-type
/// selectors take one class, not a pair.
pub(super) fn periodic_aura_log(
    s: &PeriodicAuraLog,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
    log: &mut ChatLog,
) {
    let caster = ctx.classify(s.caster, stores);
    let target = ctx.classify(s.target, stores);
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    for tick in &s.ticks {
        let (family, buff, fills) = match *tick {
            PeriodicTick::Damage { amount, school, .. } => (
                combat::PERIODICAURADAMAGE,
                false,
                Fills {
                    spell: spell.clone(),
                    // A u32 on this packet, the same `SPELL_SCHOOL<n>_NAME` index as direct damage.
                    school: u8::try_from(school).ok(),
                    amount: i64::from(amount),
                    ..Default::default()
                },
            ),
            PeriodicTick::Heal { amount } => (
                combat::PERIODICAURAHEAL,
                true,
                Fills {
                    spell: spell.clone(),
                    amount: i64::from(amount),
                    ..Default::default()
                },
            ),
            // The same displayed-figure divide, at `0x627087` in `0x626dd0`.
            PeriodicTick::Energize { power, amount } => (
                combat::POWERGAIN,
                true,
                Fills {
                    spell: spell.clone(),
                    power: Some(power),
                    amount: power_gain(power, amount),
                    ..Default::default()
                },
            ),
            // Aura 64 goes to the same formatter as the execute log's `POWER_DRAIN` (`0x627910`
            // to `0x627930`, periodic flag set), which makes every wording choice.
            PeriodicTick::ManaLeech {
                power,
                amount,
                multiplier,
            } => {
                power_drain_line(
                    log,
                    ctx,
                    stores,
                    poses,
                    &spell,
                    s.spell_id,
                    (s.caster, caster),
                    (s.target, target),
                    power,
                    periodic_leech_amount(power, amount),
                    multiplier,
                    true,
                );
                continue;
            }
        };
        let Some(kind) = combat::periodic_kind(periodic_subject(tick, caster, target), buff) else {
            continue;
        };
        queue(
            log,
            ctx,
            poses,
            kind,
            family,
            (s.caster, caster),
            (s.target, target),
            fills,
        );
    }
}

/// `SMSG_SPELLDAMAGESHIELD`: the Thorns-style return hit. The sentence's subject is the packet's
/// `victim`, who wears the shield; `attacker` struck it and takes the damage
/// (`DAMAGESHIELDSELFOTHER` is "You reflect %d %s damage to %s."). The chat type is a two-way on
/// the bearer: `0x62c140` returns `0x3f` `SPELL_DAMAGESHIELDS_ON_SELF` for class 0 or 1, else
/// `0x40` `…ON_OTHERS`.
pub(super) fn damage_shield(
    s: DamageShield,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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

/// Which endpoint a periodic tick's formatter hands its msg-id selector, choosing
/// `SPELL_PERIODIC_SELF_*` or `…_CREATURE_*`. `0x628100` (`PERIODICAURADAMAGE`) and `0x627240`
/// (`PERIODICAURAHEAL`) hand the target (`0x628235`, `0x62732c`); `0x627520` (`POWERGAIN`) and
/// `0x627930` (`SPELLPOWERLEECH`/`…DRAIN`) the caster (`0x6275fb`, `0x627a0f`/`0x627a3a`).
/// [`power_drain_line`] applies the `ManaLeech` row directly.
fn periodic_subject(tick: &PeriodicTick, caster: UnitClass, target: UnitClass) -> UnitClass {
    match tick {
        PeriodicTick::Damage { .. } | PeriodicTick::Heal { .. } => target,
        PeriodicTick::Energize { .. } | PeriodicTick::ManaLeech { .. } => caster,
    }
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
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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
    )
}

/// A unit's owner guid, `CHARMEDBY` then `CREATEDBY`, the pair [`combat::classify`] reads.
fn unit_owner(guid: u64, ctx: &ChatCtx, stores: &Query<&mut ObjectStore>) -> Option<u64> {
    let entity = ctx.index.0.get(&guid).copied()?;
    let store = stores.get(entity).ok()?;
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
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
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
/// unchanged value prints nothing (`0x62c5f0`). No range gate or classifier: the chat type is the
/// literal `0x55` `COMBAT_FACTION_CHANGE`. The wire's list id is `Faction.dbc`'s `rep_index`.
pub(crate) fn faction_standing(
    deltas: &[(u32, i32)],
    reputations: &Reputations,
    factions: Option<&crate::target::ring::Factions>,
    log: &mut ChatLog,
) {
    let Some(catalog) = factions.map(|f| f.catalog()) else {
        return;
    };
    for &(list_id, standing) in deltas {
        let old = reputations
            .0
            .get(list_id as usize)
            .map_or(0, |(_, standing)| *standing);
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
        });
    }
}

/// The tail every arm ends in: build the queued line (dropping a class-9 endpoint) and park it
/// for its names.
fn queue(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &Query<&mut Transform>,
    kind: crate::ui_chat::ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    object: (u64, UnitClass),
    fills: Fills,
) {
    queue_named(
        log,
        ctx,
        poses,
        kind,
        family,
        subject,
        object,
        fills,
        combat::Named::Ready,
    );
}

/// [`queue`] for a family whose `Named` slot (an item entry or unit guid) the drain resolves,
/// holding the line until it lands (`0x6294b0`).
fn queue_named(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &Query<&mut Transform>,
    kind: crate::ui_chat::ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    object: (u64, UnitClass),
    fills: Fills,
    named: combat::Named,
) {
    queue_reported(log, ctx, poses, kind, family, subject, object, fills, named);
}

/// [`queue_named`], answering whether a line was queued: `0x627de0` reports it to `0x627930` in
/// `al` (set at `0x627fb0`/`0x627fe2`/`0x627ff4`, cleared at `0x628006`), and a failed range gate
/// or empty template is a fall-through there.
fn queue_reported(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    poses: &Query<&mut Transform>,
    kind: crate::ui_chat::ChatEventKind,
    family: Family,
    subject: (u64, UnitClass),
    object: (u64, UnitClass),
    fills: Fills,
    named: combat::Named,
) -> bool {
    // An OR, not an AND (`0x626630`): either endpoint within its class's range shows the line.
    if !ctx.in_range(subject.0, subject.1, poses) && !ctx.in_range(object.0, object.1, poses) {
        return false;
    }
    let Some(line) = combat::queue(kind, family, subject, object, fills, named) else {
        return false;
    };
    log.push_combat(line);
    true
}

#[cfg(test)]
mod tests {
    use super::{
        combat, leech_family, leech_figures, periodic_leech_amount, periodic_subject, power_gain,
        rounded_product, PeriodicTick, UnitClass,
    };

    /// A periodic line is typed off the endpoint its own formatter reads: a creature's DoT on you
    /// is `…_PERIODIC_SELF_DAMAGE`, yours on a creature `…_PERIODIC_CREATURE_DAMAGE`.
    #[test]
    fn a_periodic_damage_or_heal_tick_is_typed_off_the_target() {
        let dmg = PeriodicTick::Damage {
            amount: 12,
            school: 3,
            absorb: 0,
            resist: 0,
        };
        let heal = PeriodicTick::Heal { amount: 12 };
        // A creature's DoT ticking me types SELF.
        assert_eq!(
            periodic_subject(&dmg, UnitClass::Creature, UnitClass::Me),
            UnitClass::Me
        );
        // My DoT ticking a creature types CREATURE.
        assert_eq!(
            periodic_subject(&dmg, UnitClass::Me, UnitClass::Creature),
            UnitClass::Creature
        );
        assert_eq!(
            periodic_subject(&heal, UnitClass::Creature, UnitClass::Me),
            UnitClass::Me
        );
        // The other two formatters read the caster.
        let gain = PeriodicTick::Energize {
            power: 1,
            amount: 10,
        };
        let leech = PeriodicTick::ManaLeech {
            power: 0,
            amount: 40,
            multiplier: 1.0,
        };
        assert_eq!(
            periodic_subject(&gain, UnitClass::Creature, UnitClass::Me),
            UnitClass::Creature
        );
        assert_eq!(
            periodic_subject(&leech, UnitClass::Creature, UnitClass::Me),
            UnitClass::Creature
        );
    }

    /// A rage gain words the displayed figure. `Unbridled Wrath Effect` (12964) energizes rage by
    /// base points 9 + 1 = 10 on the rage field's ×10 scale, and `SendEnergizeSpellLog` sends that
    /// 10 (`SpellCaster.cpp:796-813`); the line reads 1.
    #[test]
    fn a_rage_gain_words_the_displayed_figure_not_the_wire_one() {
        assert_eq!(power_gain(1, 10), 1, "Unbridled Wrath: one point of rage");
        assert_eq!(power_gain(1, 100), 10, "Bloodrage: ten");
        // Mana, focus and energy are one-to-one.
        assert_eq!(power_gain(0, 300), 300);
        assert_eq!(power_gain(2, 20), 20);
        assert_eq!(power_gain(3, 20), 20);
    }

    /// `0x627930`'s three rules, one function for both of its packets.
    #[test]
    fn a_leech_divides_both_figures_and_drops_a_zero_drained_line() {
        // Mana leeches one-to-one: 120 drained, half of it gained.
        assert_eq!(leech_figures(0, 120, 0.5), Some((120, 60)));
        // The product is truncated, then divided: `trunc(25 * 0.9) = 22`, `22 / 10 = 2`, not 1.
        assert_eq!(leech_figures(1, 25, 0.9), Some((2, 2)));
        // Below one displayed point, `drained == 0` drops the line.
        assert_eq!(leech_figures(1, 9, 1.0), None);
        assert_eq!(leech_figures(0, 0, 1.0), None);
        // Happiness is ×1000: 1500 raw is one displayed point.
        assert_eq!(leech_figures(4, 1500, 0.0), Some((1, 0)));
    }

    /// The leech/drain fork on `[0x8029d4] = 2^-22`: a multiplier exactly on it is a leech.
    #[test]
    fn the_leech_drain_fork_is_the_references_own_epsilon() {
        const EPS: f32 = 1.0 / 4_194_304.0;
        assert_eq!(leech_family(1.0).stem, "SPELLPOWERLEECH");
        assert_eq!(
            leech_family(EPS).stem,
            "SPELLPOWERLEECH",
            "equality leeches"
        );
        assert_eq!(leech_family(EPS / 2.0).stem, "SPELLPOWERDRAIN");
        assert_eq!(leech_family(0.0).stem, "SPELLPOWERDRAIN");
        // `fabs` first: the sign does not matter.
        assert_eq!(leech_family(-1.0).stem, "SPELLPOWERLEECH");
    }

    /// `0x6278f0`'s table has five entries, happiness included (`HAPPINESS_POINTS`,
    /// `GlobalStrings.lua:2117`), bounded by an unsigned `cmp ecx,5; jae`. Happiness lacks only a
    /// `COMBAT_TEXT_UPDATE` tag.
    #[test]
    fn every_wire_power_tag_has_a_noun_and_nothing_past_the_table_does() {
        for power in 0..=4 {
            assert!(combat::power_has_word(power), "power {power} has a noun");
        }
        for power in [5, 6, 99, u32::MAX] {
            assert!(!combat::power_has_word(power), "power {power} has none");
        }
    }

    /// The gained figure goes through an f32 product (`0x627a95`); 2^24+1 is the smallest amount
    /// where the narrowing shows, rounding to even one below.
    #[test]
    fn the_gained_figure_goes_through_the_references_f32_product() {
        assert_eq!(rounded_product(25, 0.9), 22, "floor, not round");
        assert_eq!(rounded_product(120, 0.5), 60);
        assert_eq!(
            rounded_product(16_777_217, 1.0),
            16_777_216,
            "the `fst` narrowing"
        );
    }

    /// The periodic leg divides twice, as the reference does (`0x627126`/`0x627138`, then
    /// `0x627930`): mana is unchanged, a wire 100 rage words as 1.
    #[test]
    fn a_periodic_leech_is_pre_divided_before_the_formatter_divides_again() {
        assert_eq!(periodic_leech_amount(0, 400), 400, "mana: the identity");
        assert_eq!(periodic_leech_amount(1, 100), 10, "rage: the first divide");
        // End to end: 100 rage on the wire words as 1.
        assert_eq!(
            leech_figures(1, periodic_leech_amount(1, 100), 1.0),
            Some((1, 1))
        );
        // The direct packet takes the same wire number through one divide only.
        assert_eq!(leech_figures(1, 100, 1.0), Some((10, 10)));
    }
}
