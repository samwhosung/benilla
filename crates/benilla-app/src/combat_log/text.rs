//! The combat log's floating-number leg: the callers of `SubmitWorldText` (`0x6c7840`), one fn
//! per packet. Each classifies the damage source (self, owned by me, or anything else, which is
//! never drawn), resolves the recipient, applies Gate A (no damage text over self), picks the
//! category and colour and writes a [`CombatTextSpawn`]. XP is the exception: over self, no gates,
//! its own row colour. The law itself lives in [`crate::combat_text`].
//!
//! Each spell arm also feeds `UNIT_COMBAT` ([`UnitCombatFeedback`], the portrait hit indicator),
//! ungated and fired at packet receive; the melee arm's rides the swing impact keyframe instead
//! (`ui_unit::melee_unit_combat`).

use bevy::prelude::*;

use benilla_protocol::messages::{
    power_display_scale, DamageShield, ExplorationXp, LevelUpInfo, PeriodicAuraLog, PeriodicTick,
    SpellDamageLog, SpellEnergizeLog, SpellHealLog, SpellLogMiss, XpGain,
};

use crate::combat_text::{
    damage_color, miss_word, spell_text, CombatTextSpawn, DamageSource, DamageTextGates,
};
use crate::names::NameCache;
use crate::ui_chat::ChatLog;
use crate::ui_unit::{CombatTextEvent, UnitCombatFeedback};

use crate::net::{GuidIndex, NetCommands, ObjectStore, SelfGuid};

/// Gate A: the recipient's entity, unless the recipient is us (`0x607140` and `0x6128b0` compare
/// the anchor guid to the active player and return before submitting).
fn gated_anchor(guid: u64, index: &GuidIndex, self_guid: &SelfGuid) -> Option<Entity> {
    if self_guid.0 == Some(guid) {
        return None;
    }
    index.0.get(&guid).copied()
}

/// The source-ownership classifier `0x5efea0` (the colour law's `K`): `Player` for me, `Pet` for a
/// unit whose SummonedBy or CreatedBy is me (pet, guardian, totem), `None` for anything else,
/// whose damage is never drawn.
pub(crate) fn classify_source(
    guid: u64,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
) -> Option<DamageSource> {
    let me = self_guid.0?;
    if guid == me {
        return Some(DamageSource::Player);
    }
    let entity = index.0.get(&guid).copied()?;
    let store = stores.get(entity).ok()?;
    (store.0.unit_summoned_by() == Some(me) || store.0.unit_created_by() == Some(me))
        .then_some(DamageSource::Pet)
}

// The melee number and word (`SMSG_ATTACKERSTATEUPDATE`) spawn at the swing's impact keyframe
// (`crate::combat_text::melee_impact_text`), not here.

/// The colour law's `B` bit for a spell-packet emit, from the spell's record via
/// [`crate::combat_text::melee_styled`]: a ranged basic shot floats white, a spell gold.
pub(super) fn melee_styled(spells: Option<&crate::ui_action::Spells>, spell_id: u32) -> bool {
    crate::combat_text::melee_styled(spells.and_then(|s| s.catalog.get(spell_id)))
}

/// The spell arms' `UNIT_COMBAT` action and descriptor: a landed amount is `WOUND` (`CRITICAL` on
/// a crit); zero damage takes `ABSORB` before `RESIST`, the order the reference's emitter
/// `0x494600` tests them (`0x49463b`); a clean miss arrives via `SMSG_SPELLLOGMISS` instead.
fn spell_feedback(
    damage: u32,
    absorb: u32,
    resist: i32,
    crit: bool,
) -> Option<(&'static str, &'static str)> {
    if damage > 0 {
        Some(("WOUND", if crit { "CRITICAL" } else { "" }))
    } else if absorb > 0 {
        Some(("WOUND", "ABSORB"))
    } else if resist > 0 {
        Some(("WOUND", "RESIST"))
    } else {
        None
    }
}

/// A centre-text line: the messageType, `arg2` and `arg3`.
pub(crate) type CenterText = (&'static str, Option<String>, Option<String>);

/// `SMSG_SPELLNONMELEEDAMAGELOG`'s centre text, routed as the handler `0x5e85e0` routes the log:
/// split damage (`hit_info & 8`, `0x5e8700`) is `0x62de60`'s `SPLIT_DAMAGE` with the damage; a
/// periodic log (`0x5e876d`) takes the periodic tick's words, and none at all while
/// `CombatLogPeriodicSpells` is off (`0x62d9ae`); anything else takes the direct hit's.
fn spell_damage_center_text(s: &SpellDamageLog, log_periodic: bool) -> Option<CenterText> {
    if s.hit_info & 0x8 != 0 {
        Some(("SPLIT_DAMAGE", Some(s.damage.to_string()), None))
    } else if s.periodic {
        log_periodic
            .then(|| periodic_center_text(s.damage, s.absorb, s.resist))
            .flatten()
    } else {
        direct_spell_center_text(s.damage, s.absorb, s.resist, s.blocked)
    }
}

/// The direct spell hit's words (`0x62cd80`), with no crit type. A landed hit takes the first of
/// `SPELL_RESISTED` (damage, resisted; `0x62d048`), `SPELL_BLOCKED` (blocked; `0x62d062`) and
/// `SPELL_ABSORBED` (damage, absorbed; `0x62d07b`), else `SPELL_DAMAGE` (`0x62d098`). Nothing
/// through is worded absorbed, then blocked (miss code 5, `0x62cec7`), then resisted.
fn direct_spell_center_text(
    damage: u32,
    absorb: u32,
    resist: i32,
    blocked: u32,
) -> Option<CenterText> {
    let amount = |n: u32| Some(n.to_string());
    if damage > 0 {
        Some(if resist > 0 {
            ("SPELL_RESISTED", amount(damage), Some(resist.to_string()))
        } else if blocked != 0 {
            ("SPELL_BLOCKED", amount(blocked), None)
        } else if absorb != 0 {
            ("SPELL_ABSORBED", amount(damage), amount(absorb))
        } else {
            ("SPELL_DAMAGE", amount(damage), None)
        })
    } else if absorb != 0 {
        Some(("SPELL_ABSORBED", None, None))
    } else if blocked != 0 {
        Some(("SPELL_BLOCKED", None, None))
    } else if resist != 0 {
        Some(("SPELL_RESISTED", None, None))
    } else {
        None
    }
}

/// A periodic damage tick's words (`0x628100`), the melee family's: a landed tick takes `RESIST`
/// (damage, resisted; `0x628354`), else `ABSORB` (damage, absorbed; `0x62836c`), else `DAMAGE`
/// (`0x62837f`). Nothing through is `SPELL_ABSORBED`, else `SPELL_RESISTED`.
fn periodic_center_text(damage: u32, absorb: u32, resist: i32) -> Option<CenterText> {
    let amount = |n: u32| Some(n.to_string());
    if damage > 0 {
        Some(if resist > 0 {
            ("RESIST", amount(damage), Some(resist.to_string()))
        } else if absorb != 0 {
            ("ABSORB", amount(damage), amount(absorb))
        } else {
            ("DAMAGE", amount(damage), None)
        })
    } else if absorb != 0 {
        Some(("SPELL_ABSORBED", None, None))
    } else if resist != 0 {
        Some(("SPELL_RESISTED", None, None))
    } else {
        None
    }
}

/// Melee packet to the centre text's messageType and args (`0x629d30`), fired at packet receive,
/// not at the impact keyframe. A landed hit with a partial fires the first of resist
/// (`0x629efd`), block (`0x629f15`) and absorb (`0x629f30`) with `(damage, partial)`, ahead of
/// the crit.
pub(crate) fn melee_center_text(
    hit_info: u32,
    victim_state: u32,
    damage: u32,
    absorb: u32,
    resist: i32,
    blocked: u32,
) -> Option<CenterText> {
    match victim_state {
        2 => Some(("DODGE", None, None)),
        3 => Some(("PARRY", None, None)),
        5 => Some(("BLOCK", None, None)),
        6 => Some(("EVADE", None, None)),
        7 => Some(("IMMUNE", None, None)),
        8 => Some(("DEFLECT", None, None)),
        _ => {
            if damage > 0 {
                if resist > 0 {
                    Some(("RESIST", Some(damage.to_string()), Some(resist.to_string())))
                } else if blocked > 0 {
                    Some(("BLOCK", Some(damage.to_string()), Some(blocked.to_string())))
                } else if absorb > 0 {
                    Some(("ABSORB", Some(damage.to_string()), Some(absorb.to_string())))
                } else if hit_info & 0x80 != 0 {
                    Some(("DAMAGE_CRIT", Some(damage.to_string()), None))
                } else {
                    Some(("DAMAGE", Some(damage.to_string()), None))
                }
            } else if hit_info & 0x20 != 0 {
                Some(("ABSORB", None, None))
            } else if hit_info & 0x40 != 0 {
                Some(("RESIST", None, None))
            } else {
                Some(("MISS", None, None))
            }
        }
    }
}

/// vmangos `Powers` id to the centre text's power-gain messageType; happiness has none.
fn power_message_type(power: u32) -> Option<&'static str> {
    Some(match power {
        0 => "MANA",
        1 => "RAGE",
        2 => "FOCUS",
        3 => "ENERGY",
        _ => return None,
    })
}

/// `SMSG_SPELLNONMELEEDAMAGELOG`: the spell-damage number (crit on `SPELL_HIT_TYPE_CRIT` `0x2`) or
/// the zero-damage absorb or resist word over the target (`0x5e85e0`); only my spells and my
/// pet's (`PetSpellDamage`-gated) draw, number and word coloured alike.
pub(super) fn spell_damage_log(
    s: SpellDamageLog,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
    log_periodic: bool,
    text: &mut MessageWriter<CombatTextSpawn>,
    feedback: &mut MessageWriter<UnitCombatFeedback>,
    center: &mut MessageWriter<CombatTextEvent>,
) {
    if benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "fct",
            &format!(
                "recv spelldmg atk={:#x} target={:#x} dmg={} crit={}",
                s.attacker,
                s.target,
                s.damage,
                s.hit_info & 0x2 != 0
            ),
        );
    }
    // UNIT_COMBAT: any source, any recipient including self, before the returns below.
    if let (Some(&unit), Some((action, flags))) = (
        index.0.get(&s.target),
        spell_feedback(s.damage, s.absorb, s.resist, s.hit_info & 0x2 != 0),
    ) {
        feedback.write(UnitCombatFeedback {
            unit,
            action,
            flags,
            amount: s.damage,
            school: u32::from(s.school),
        });
    }
    // The center combat text: self recipient only.
    if self_guid.0 == Some(s.target) {
        if let Some((message_type, data, extra)) = spell_damage_center_text(&s, log_periodic) {
            center.write(CombatTextEvent {
                message_type,
                data,
                extra,
            });
        }
    }
    let Some(source) = classify_source(s.attacker, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    // A ranged basic shot (Throw, Auto Shot: `AttributesEx3 & 0x8000`) floats white.
    let Some(color) = damage_color(gates, source, melee_styled(spells, s.spell_id)) else {
        return; // the CombatDamage / PetSpellDamage gates
    };
    if let (Some(anchor), Some((category, body))) = (
        gated_anchor(s.target, index, self_guid),
        spell_text(s.damage, s.absorb, s.resist, s.hit_info & 0x2 != 0),
    ) {
        text.write(CombatTextSpawn {
            anchor,
            text: body,
            category,
            color,
        });
    }
}

/// `SMSG_PERIODICAURALOG`: damage ticks float like direct damage, never as a crit (`0x626dd0`);
/// heal, energize and leech ticks float nothing, as heals never float in the 1.12 client.
pub(super) fn periodic_aura_log(
    s: PeriodicAuraLog,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
    feedback: &mut MessageWriter<UnitCombatFeedback>,
    center: &mut MessageWriter<CombatTextEvent>,
    names: &NameCache,
    net: &NetCommands,
) {
    if benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "fct",
            &format!(
                "recv periodic caster={:#x} target={:#x} ticks={}",
                s.caster,
                s.target,
                s.ticks.len()
            ),
        );
    }
    // Centre text, self recipient only: damage ticks the periodic words, heal ticks `PERIODIC_HEAL`
    // (arg2 the caster's name, arg3 the amount), energize ticks the power-gain family.
    if self_guid.0 == Some(s.target) {
        for tick in &s.ticks {
            let ev = match *tick {
                PeriodicTick::Damage {
                    amount,
                    absorb,
                    resist,
                    ..
                } => periodic_center_text(amount, absorb, resist).map(
                    |(message_type, data, extra)| CombatTextEvent {
                        message_type,
                        data,
                        extra,
                    },
                ),
                PeriodicTick::Heal { amount } => Some(CombatTextEvent {
                    message_type: "PERIODIC_HEAL",
                    data: Some(
                        names
                            .resolve(s.caster, net)
                            .map(str::to_string)
                            .unwrap_or_default(),
                    ),
                    extra: Some(amount.to_string()),
                }),
                // The displayed figure: `0x626dd0` divides by `0x6e7130(powerType)` at `0x627087`
                // before both the COMBAT_TEXT push (`0x494770`) and the chat line.
                PeriodicTick::Energize { power, amount } => {
                    power_message_type(power).map(|message_type| CombatTextEvent {
                        message_type,
                        data: Some((amount / power_display_scale(power)).to_string()),
                        extra: None,
                    })
                }
                PeriodicTick::ManaLeech { .. } => None,
            };
            if let Some(ev) = ev {
                center.write(ev);
            }
        }
    }
    // UNIT_COMBAT: every tick, any source; damage is `WOUND`, a heal `HEAL`, which shows on the
    // portrait though heals never float as worldtext.
    if let Some(&unit) = index.0.get(&s.target) {
        for tick in &s.ticks {
            let (action, flags, amount, school) = match *tick {
                PeriodicTick::Damage {
                    amount,
                    school,
                    absorb,
                    resist,
                } => {
                    let Some((action, flags)) = spell_feedback(amount, absorb, resist, false)
                    else {
                        continue;
                    };
                    (action, flags, amount, school)
                }
                PeriodicTick::Heal { amount } => ("HEAL", "", amount, 0),
                // No ENERGIZE: the string is absent from the 1.12 client; power reaches the centre
                // text only.
                PeriodicTick::Energize { .. } | PeriodicTick::ManaLeech { .. } => continue,
            };
            feedback.write(UnitCombatFeedback {
                unit,
                action,
                flags,
                amount,
                school,
            });
        }
    }
    let Some(source) = classify_source(s.caster, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    // `0x626dd0` pushes the spell's record to the same `0x6128b0` law as direct damage.
    let Some(color) = damage_color(gates, source, melee_styled(spells, s.spell_id)) else {
        return;
    };
    let Some(anchor) = gated_anchor(s.target, index, self_guid) else {
        return;
    };
    for tick in &s.ticks {
        let PeriodicTick::Damage {
            amount,
            absorb,
            resist,
            ..
        } = *tick
        else {
            continue;
        };
        if let Some((category, body)) = spell_text(amount, absorb, resist, false) {
            text.write(CombatTextSpawn {
                anchor,
                text: body,
                category,
                color,
            });
        }
    }
}

/// `SMSG_SPELLDAMAGESHIELD`: the number floats over the attacker, who struck the shield; the
/// source is the shield bearer (the victim field). The site pushes a NULL record, so it colours
/// melee-styled: my shield white, my pet's orange (`0x5e84e0`).
pub(super) fn damage_shield(
    s: DamageShield,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
    feedback: &mut MessageWriter<UnitCombatFeedback>,
) {
    if s.damage == 0 {
        return;
    }
    // UNIT_COMBAT: the attacker takes the damage, so striking thorns wounds your own portrait.
    if let Some(&unit) = index.0.get(&s.attacker) {
        feedback.write(UnitCombatFeedback {
            unit,
            action: "WOUND",
            flags: "",
            amount: s.damage,
            school: s.school,
        });
    }
    let Some(source) = classify_source(s.victim, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    let Some(color) = damage_color(gates, source, true) else {
        return;
    };
    if let Some(anchor) = gated_anchor(s.attacker, index, self_guid) {
        text.write(CombatTextSpawn {
            anchor,
            text: s.damage.to_string(),
            category: 0,
            color,
        });
    }
}

/// `SMSG_SPELLHEALLOG`: `UNIT_COMBAT` `HEAL` for any streamed recipient, and the centre text's
/// `HEAL`/`HEAL_CRIT` (arg2 the healer's name, arg3 the amount) for self. No worldtext: heals
/// never float in the 1.12 client.
pub(super) fn spell_heal_log(
    s: SpellHealLog,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    feedback: &mut MessageWriter<UnitCombatFeedback>,
    center: &mut MessageWriter<CombatTextEvent>,
    names: &NameCache,
    net: &NetCommands,
) {
    if let Some(&unit) = index.0.get(&s.target) {
        feedback.write(UnitCombatFeedback {
            unit,
            action: "HEAL",
            flags: if s.critical { "CRITICAL" } else { "" },
            amount: s.amount,
            school: 0,
        });
    }
    if self_guid.0 == Some(s.target) {
        center.write(CombatTextEvent {
            message_type: if s.critical { "HEAL_CRIT" } else { "HEAL" },
            data: Some(
                names
                    .resolve(s.healer, net)
                    .map(str::to_string)
                    .unwrap_or_default(),
            ),
            extra: Some(s.amount.to_string()),
        });
    }
}

/// `SMSG_SPELLENERGIZELOG`: the centre text's power-gain line, self only. No `UNIT_COMBAT`: the
/// ENERGIZE string is absent from the 1.12 client, so `CombatFeedback.lua`'s ENERGIZE arm is dead.
/// The amount is the displayed figure: `0x5e8a90` divides by `0x6e7130(powerType)` at
/// `0x5e8af3` before the COMBAT_TEXT push (`0x494770`) and the chat formatter (`0x62ca00`).
pub(super) fn spell_energize_log(
    s: SpellEnergizeLog,
    self_guid: &SelfGuid,
    center: &mut MessageWriter<CombatTextEvent>,
) {
    if self_guid.0 == Some(s.target) {
        if let Some(message_type) = power_message_type(s.power) {
            center.write(CombatTextEvent {
                message_type,
                data: Some((s.amount / power_display_scale(s.power)).to_string()),
                extra: None,
            });
        }
    }
}

/// Miss code (vmangos `SpellMissInfo`) to the centre text's `SPELL_*` outcome word, the jump table
/// `0x62be10` indexed by code - 2 (`0x62bb50`). Code 10 (absorb) has no word of its own: its slot
/// and every code past the table take the default `SPELL_MISSED` (`0x62bdc3`). Code 0 fires
/// nothing (`0x62babb`).
fn miss_center_type(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => return None,
        2 => "SPELL_RESISTED",
        3 => "SPELL_DODGED",
        4 => "SPELL_PARRIED",
        5 => "SPELL_BLOCKED",
        6 => "SPELL_EVADED",
        7 | 8 => "SPELL_IMMUNE",
        9 => "SPELL_DEFLECTED",
        11 => "SPELL_REFLECTED",
        _ => "SPELL_MISSED",
    })
}

/// Miss code (vmangos `SpellMissInfo`, 1 to 11) to the `UNIT_COMBAT` action word, the worldtext
/// words' table in the event's uppercase keys (`CombatFeedbackText`).
fn miss_action(code: u8) -> Option<&'static str> {
    Some(match code {
        1 => "MISS",
        2 => "RESIST",
        3 => "DODGE",
        4 => "PARRY",
        5 => "BLOCK",
        6 => "EVADE",
        7 | 8 => "IMMUNE",
        9 => "DEFLECT",
        10 => "ABSORB",
        11 => "REFLECT",
        _ => return None,
    })
}

/// `SMSG_SPELLLOGMISS`: one outcome word per missed target (`0x5e7e00`); vmangos sends only
/// impact-time outcomes (immune, evade) here. Coloured by the same law as a number: `0x5e7f66`
/// pushes the resolved spell record, not NULL, so the words are spell gold.
pub(super) fn spell_log_miss(
    s: SpellLogMiss,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
    text: &mut MessageWriter<CombatTextSpawn>,
    feedback: &mut MessageWriter<UnitCombatFeedback>,
    center: &mut MessageWriter<CombatTextEvent>,
) {
    // UNIT_COMBAT for each missed target, any source; a self target also gets the centre text's
    // `SPELL_*` word.
    for &(target, code) in &s.misses {
        if let (Some(&unit), Some(action)) = (index.0.get(&target), miss_action(code)) {
            feedback.write(UnitCombatFeedback {
                unit,
                action,
                flags: "",
                amount: 0,
                school: 0,
            });
        }
        if self_guid.0 == Some(target) {
            if let Some(message_type) = miss_center_type(code) {
                center.write(CombatTextEvent {
                    message_type,
                    data: None,
                    extra: None,
                });
            }
        }
    }
    // The word emitter `0x607140` checks the same three CVars as the number emitter `0x6128b0`:
    // `CombatDamage` (`0x60718f`), `PetSpellDamage` (`0x6071a6`), `PetMeleeDamage` (`0x6071c5`).
    let Some(source) = classify_source(s.caster, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    let Some(color) = damage_color(gates, source, melee_styled(spells, s.spell_id)) else {
        return; // the CombatDamage / Pet* gates
    };
    for &(target, code) in &s.misses {
        if let (Some(anchor), Some((word, category))) =
            (gated_anchor(target, index, self_guid), miss_word(code))
        {
            text.write(CombatTextSpawn {
                anchor,
                text: word.to_string(),
                category,
                color,
            });
        }
    }
}

/// `SMSG_LOG_XPGAIN`: `"XP: %d"` over self, category 4, the one emitter that skips Gate A
/// (`0x607260`), for kill and quest XP alike; the packet also writes the chat line.
pub(super) fn xp_gain(
    x: XpGain,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    text: &mut MessageWriter<CombatTextSpawn>,
    chat_log: &mut ChatLog,
) {
    if let Some(&me) = self_guid.0.as_ref().and_then(|g| index.0.get(g)) {
        text.write(CombatTextSpawn {
            anchor: me,
            text: format!("XP: {}", x.total),
            category: 4,
            color: None, // row 4's own purple: XP skips the damage colour law
        });
    }
    chat_log.push_xp_gain(&x);
}

/// `SMSG_EXPLORATION_EXPERIENCE`, as the reference's case body `[0x5e41d2, 0x5e42bb]`:
/// - the ERR_ZONE_EXPLORED toast and, when xp > 0, the ERR_ZONE_EXPLORED_XP chat line, via
///   [`ChatLog::push_exploration`]; an `AreaTable.dbc` id with no name shows no message;
/// - the race's discovery jingle (`ChrRaces.dbc` col 3 to SoundEntries), played even when the
///   area resolves to nothing, through the same 2D path as `SMSG_PLAY_SOUND`.
///
/// No floating text: the XP arrives separately as `SMSG_LOG_XPGAIN`.
pub(super) fn exploration_xp(
    x: ExplorationXp,
    area_table: Option<&crate::area::AreaTableRes>,
    sounds: Option<&crate::sound::ExplorationSounds>,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    sound_out: &mut MessageWriter<crate::net::ServerSoundMessage>,
    chat_log: &mut ChatLog,
) {
    let race = self_guid
        .0
        .as_ref()
        .and_then(|g| index.0.get(g))
        .and_then(|&e| stores.get(e).ok())
        .and_then(|s| s.0.unit_race());
    let kit = race.and_then(|r| sounds.and_then(|s| s.0.kit(u32::from(r))));
    if let Some(kit) = kit {
        sound_out.write(crate::net::ServerSoundMessage {
            kind: crate::net::ServerSoundKind::Sound2d,
            sound_id: kit,
            source: None,
        });
    }
    match area_table.and_then(|t| t.0.name(x.area_id)) {
        Some(name) => {
            info!(
                "exploration: discovered {name} (area {}, {} xp, jingle kit {})",
                x.area_id,
                x.xp,
                kit.unwrap_or(0)
            );
            chat_log.push_exploration(name, x.xp);
        }
        None => warn!(
            "exploration: discovered area {} has no AreaTable name — no message (sound only)",
            x.area_id
        ),
    }
}

/// `SMSG_LEVELUP_INFO`: the ding's chat lines. The talent-point arg is not on the wire; the client
/// computes `(newLevel >= 10) ? 1 : 0` (`0x5e407c`). The ding's visual rides the
/// `UNIT_FIELD_LEVEL` change watcher, not this packet.
pub(super) fn level_up(l: LevelUpInfo, chat_log: &mut ChatLog) {
    let talent_points = u32::from(l.level >= 10);
    // Parked for `ui_unit`'s `PLAYER_LEVEL_UP`, whose args the stock `ChatFrame_OnEvent` prints;
    // nothing here prints the block itself, or it shows twice.
    chat_log.push_level_up_gains(&l, talent_points);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat_text::COLOR_SPELL_GOLD;
    use crate::net::{Guid, SelfPlayer};
    use bevy::ecs::system::RunSystemOnce;

    fn words(
        message_type: &'static str,
        data: Option<u32>,
        extra: Option<u32>,
    ) -> Option<CenterText> {
        Some((
            message_type,
            data.map(|d| d.to_string()),
            extra.map(|e| e.to_string()),
        ))
    }

    /// A landed melee hit with several partials takes the first of resist, block and absorb
    /// (`0x629efd`, `0x629f15`, `0x629f30`).
    #[test]
    fn melee_partials_are_tested_resist_block_absorb() {
        // (hit_info, victim_state, damage, absorb, resist, blocked)
        assert_eq!(
            melee_center_text(0, 1, 300, 50, 0, 40),
            words("BLOCK", Some(300), Some(40))
        );
        assert_eq!(
            melee_center_text(0, 1, 300, 50, 20, 0),
            words("RESIST", Some(300), Some(20))
        );
        assert_eq!(
            melee_center_text(0x80, 1, 300, 50, 20, 40),
            words("RESIST", Some(300), Some(20))
        );
        assert_eq!(
            melee_center_text(0x80, 1, 300, 50, 0, 0),
            words("ABSORB", Some(300), Some(50))
        );
        assert_eq!(
            melee_center_text(0x80, 1, 300, 0, 0, 0),
            words("DAMAGE_CRIT", Some(300), None)
        );
    }

    fn spell_hit(damage: u32, absorb: u32, resist: i32, blocked: u32) -> SpellDamageLog {
        SpellDamageLog {
            target: 10,
            attacker: 20,
            spell_id: 133,
            damage,
            school: 2,
            absorb,
            resist,
            periodic: false,
            blocked,
            hit_info: 0,
        }
    }

    /// A direct spell hit on you with partials takes the first of resisted, blocked and absorbed
    /// (`0x62d048`, `0x62d062`, `0x62d07b`), crit or not; blocked carries its amount alone.
    #[test]
    fn a_direct_spell_hit_words_its_first_partial() {
        let direct = |damage, absorb, resist, blocked| {
            spell_damage_center_text(&spell_hit(damage, absorb, resist, blocked), true)
        };
        assert_eq!(
            direct(300, 50, 100, 40),
            words("SPELL_RESISTED", Some(300), Some(100))
        );
        assert_eq!(
            direct(300, 50, 0, 40),
            words("SPELL_BLOCKED", Some(40), None)
        );
        assert_eq!(
            direct(300, 50, 0, 0),
            words("SPELL_ABSORBED", Some(300), Some(50))
        );
        assert_eq!(direct(300, 0, 0, 0), words("SPELL_DAMAGE", Some(300), None));
        // A crit has no word of its own.
        let crit = SpellDamageLog {
            hit_info: 0x2,
            ..spell_hit(300, 0, 0, 0)
        };
        assert_eq!(
            spell_damage_center_text(&crit, true),
            words("SPELL_DAMAGE", Some(300), None)
        );
        // Nothing through: absorbed, then blocked, then resisted, bare.
        assert_eq!(direct(0, 50, 100, 40), words("SPELL_ABSORBED", None, None));
        assert_eq!(direct(0, 0, 100, 40), words("SPELL_BLOCKED", None, None));
        assert_eq!(direct(0, 0, 100, 0), words("SPELL_RESISTED", None, None));
        assert_eq!(direct(0, 0, 0, 0), None);
    }

    /// A periodic tick takes the melee words, `RESIST` before `ABSORB` before `DAMAGE`
    /// (`0x628354`, `0x62836c`, `0x62837f`); nothing through keeps the `SPELL_*` words.
    #[test]
    fn a_periodic_tick_words_resist_absorb_damage() {
        assert_eq!(
            periodic_center_text(300, 50, 100),
            words("RESIST", Some(300), Some(100))
        );
        assert_eq!(
            periodic_center_text(300, 50, 0),
            words("ABSORB", Some(300), Some(50))
        );
        assert_eq!(
            periodic_center_text(300, 0, 0),
            words("DAMAGE", Some(300), None)
        );
        assert_eq!(
            periodic_center_text(0, 50, 100),
            words("SPELL_ABSORBED", None, None)
        );
        assert_eq!(
            periodic_center_text(0, 0, 100),
            words("SPELL_RESISTED", None, None)
        );
        assert_eq!(periodic_center_text(0, 0, 0), None);
    }

    /// `SMSG_SPELLNONMELEEDAMAGELOG` routes as `0x5e85e0`: split damage first (`0x5e8700`), then a
    /// periodic log to the tick's words behind `CombatLogPeriodicSpells` (`0x62d9ae`), which
    /// ignore the blocked amount.
    #[test]
    fn a_spell_damage_log_routes_split_and_periodic_first() {
        let split = SpellDamageLog {
            hit_info: 0x8,
            periodic: true,
            ..spell_hit(120, 0, 30, 0)
        };
        assert_eq!(
            spell_damage_center_text(&split, true),
            words("SPLIT_DAMAGE", Some(120), None)
        );
        let periodic = SpellDamageLog {
            periodic: true,
            ..spell_hit(300, 50, 0, 40)
        };
        assert_eq!(
            spell_damage_center_text(&periodic, true),
            words("ABSORB", Some(300), Some(50))
        );
        assert_eq!(spell_damage_center_text(&periodic, false), None);
    }

    /// The spell-miss word per code, as the jump table `0x62be10`: absorb (10) and every code past
    /// the table are `SPELL_MISSED`, and 0 is nothing.
    #[test]
    fn spell_miss_codes_follow_the_reference_table() {
        let got: Vec<_> = (0..=13).map(miss_center_type).collect();
        assert_eq!(
            got,
            vec![
                None,
                Some("SPELL_MISSED"),
                Some("SPELL_RESISTED"),
                Some("SPELL_DODGED"),
                Some("SPELL_PARRIED"),
                Some("SPELL_BLOCKED"),
                Some("SPELL_EVADED"),
                Some("SPELL_IMMUNE"),
                Some("SPELL_IMMUNE"),
                Some("SPELL_DEFLECTED"),
                Some("SPELL_MISSED"),
                Some("SPELL_REFLECTED"),
                Some("SPELL_MISSED"),
                Some("SPELL_MISSED"),
            ]
        );
        assert_eq!(miss_center_type(255), Some("SPELL_MISSED"));
    }

    /// `SMSG_SPELLLOGMISS`'s word is spell gold: `0x5e7f66` pushes the resolved spell record, not
    /// the melee site's NULL, and unlike the GO's inline emit there is no speed test here.
    #[test]
    fn the_spell_log_miss_word_is_spell_gold() {
        const IMMOLATE: u32 = 348;

        let spells = crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(
                    IMMOLATE,
                    benilla_formats::SpellDisplay {
                        name: "Immolate".into(),
                        speed: 24.0, // travels, and still prints here
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };

        let mut app = App::new();
        app.add_message::<CombatTextSpawn>()
            .add_message::<UnitCombatFeedback>()
            .add_message::<CombatTextEvent>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>();
        let self_e = app
            .world_mut()
            .spawn((Guid(10), SelfPlayer, ObjectStore::default()))
            .id();
        let victim_e = app
            .world_mut()
            .spawn((Guid(20), ObjectStore::default()))
            .id();
        {
            let mut index = app.world_mut().resource_mut::<GuidIndex>();
            index.0.insert(10, self_e);
            index.0.insert(20, victim_e);
        }
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

        app.world_mut()
            .run_system_once(
                move |index: Res<GuidIndex>,
                      self_guid: Res<SelfGuid>,
                      stores: Query<&mut ObjectStore>,
                      mut text: MessageWriter<CombatTextSpawn>,
                      mut feedback: MessageWriter<UnitCombatFeedback>,
                      mut center: MessageWriter<CombatTextEvent>| {
                    spell_log_miss(
                        SpellLogMiss {
                            spell_id: IMMOLATE,
                            caster: 10,
                            misses: vec![(20, 7)], // IMMUNE, an outcome vmangos sends here
                        },
                        &index,
                        &self_guid,
                        &stores,
                        Some(&spells),
                        DamageTextGates::default(),
                        &mut text,
                        &mut feedback,
                        &mut center,
                    );
                },
            )
            .unwrap();

        let spawned: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<CombatTextSpawn>>()
            .drain()
            .map(|s| (s.text, s.category, s.color, s.anchor))
            .collect();
        assert_eq!(
            spawned,
            vec![("Immune".to_string(), 3, Some(COLOR_SPELL_GOLD), victim_e)],
            "gold, category 3, over the immune target"
        );
    }
}
