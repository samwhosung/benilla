//! The combat log's **floating number** leg — the combat-text **spawn table** (decision 0137
//! phase 2; the callers of `SubmitWorldText 0x6c7840`). One fn per packet, mirroring the client's
//! handler → emitter structure; [`super`]'s handlers call it after the chat line: each classifies
//! the damage **source** (the color law's `K` — self / owned-by-me / anything else SUPPRESSED),
//! resolves the outcome **recipient**, applies Gate A (self-anchored damage text is
//! unconditionally suppressed — outgoing damage floats over the victim, incoming never floats
//! over you), picks the category + the effective color override, and writes a
//! [`CombatTextSpawn`]. The XP emitter is the one exception (self-anchored, no gates, its own
//! row color — by the client's design). The law itself — categories, words, colors, cap,
//! timing — lives in [`crate::combat_text`].
//!
//! Each spell arm ALSO feeds the **`UNIT_COMBAT` event** ([`UnitCombatFeedback`] — the portrait
//! hit indicator, decision 0576) — that channel is the worldtext's inverse: **ungated** (no
//! Gate A, no source class; it exists to show what anyone does to you), fired at packet receive.
//! The melee arm's twin rides the swing impact keyframe instead (`ui_unit::melee_unit_combat`).

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

/// Gate A + anchor resolution: the recipient's entity, unless the recipient is us (`0x607140`/
/// `0x6128b0` compare the anchor guid to the active-player cache and return before submitting).
fn gated_anchor(guid: u64, index: &GuidIndex, self_guid: &SelfGuid) -> Option<Entity> {
    if self_guid.0 == Some(guid) {
        return None;
    }
    index.0.get(&guid).copied()
}

/// The `0x5efea0` source-ownership classifier (the color law's `K`): the damage SOURCE resolved
/// against the active player — `Player` (me), `Pet` (a unit whose Summoned/CreatedBy guid is me:
/// pet, guardian, totem), or `None` = every other class, which SUPPRESSES the emit entirely
/// (another unit's damage is never drawn; the real emitter returns before submitting).
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

// NOTE: the MELEE number/word (`SMSG_ATTACKERSTATEUPDATE`) does not spawn here — it defers to the
// attacker swing clip's impact keyframe with the rest of the victim feedback
// (`crate::combat_text::melee_impact_text`, fed by `creature_anim::impact`).

/// The color law's `B` bit for a spell-packet emit, over the catalog the arms carry: resolve the
/// spell's record and hand it to [`crate::combat_text::melee_styled`] (which owns the law and its
/// NULL-record degradation). This is how a ranged basic shot's `SMSG_SPELLNONMELEEDAMAGELOG`
/// floats white (decision 0376) while an ordinary spell floats gold.
pub(super) fn melee_styled(spells: Option<&crate::ui_action::Spells>, spell_id: u32) -> bool {
    crate::combat_text::melee_styled(spells.and_then(|s| s.catalog.get(spell_id)))
}

/// The spell arms' shared `UNIT_COMBAT` split: a landed amount is `WOUND` (`CRITICAL` descriptor
/// on crit), zero damage degrades to the full-`ABSORB`/`RESIST` descriptor, else nothing (a clean
/// spell miss arrives via `SMSG_SPELLLOGMISS` instead). PROVISIONAL string mapping until the
/// reference's UNIT_COMBAT emission is pinned down (decision 0576).
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

/// Spell packet → the center text's messageType: landed damage is ALWAYS `SPELL_DAMAGE` — **a spell
/// crit does NOT fire DAMAGE_CRIT** (no crit-distinct type exists for spell damage; the `0x62cd80`
/// emitter fires 29 regardless, crit changes only the chat template) — zero damage the
/// `SPELL_ABSORBED`/`SPELL_RESISTED` word. Spell-side partial trailers stay a named residual (the
/// melee partials go through helper B `0x6268f0`; the spell emitter's partial path is unread).
fn spell_center_text(
    damage: u32,
    absorb: u32,
    resist: i32,
) -> Option<(&'static str, Option<String>)> {
    if damage > 0 {
        Some(("SPELL_DAMAGE", Some(damage.to_string())))
    } else if absorb > 0 {
        Some(("SPELL_ABSORBED", None))
    } else if resist > 0 {
        Some(("SPELL_RESISTED", None))
    } else {
        None
    }
}

/// Melee packet → the center text's messageType + args (`0x629d30`; fired **synchronously at
/// packet receive** — the impact-keyframe deferral belongs to the worldtext/UNIT_COMBAT victim
/// dispatch, NOT this Lua event). Helper-B partials CONFIRMED: a landed hit with a partial
/// block/absorb/resist fires the word type with `(damage, partial)` — the addon renders
/// `"25 (10 blocked)"`. The partial-vs-crit precedence when both apply is unpinned — partials
/// win here, flagged in 0580.
pub(crate) fn melee_center_text(
    hit_info: u32,
    victim_state: u32,
    damage: u32,
    absorb: u32,
    resist: i32,
    blocked: u32,
) -> Option<(&'static str, Option<String>, Option<String>)> {
    match victim_state {
        2 => Some(("DODGE", None, None)),
        3 => Some(("PARRY", None, None)),
        5 => Some(("BLOCK", None, None)),
        6 => Some(("EVADE", None, None)),
        7 => Some(("IMMUNE", None, None)),
        8 => Some(("DEFLECT", None, None)),
        _ => {
            if damage > 0 {
                if absorb > 0 {
                    Some(("ABSORB", Some(damage.to_string()), Some(absorb.to_string())))
                } else if blocked > 0 {
                    Some(("BLOCK", Some(damage.to_string()), Some(blocked.to_string())))
                } else if resist > 0 {
                    Some(("RESIST", Some(damage.to_string()), Some(resist.to_string())))
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

/// vmangos `Powers` id → the center text's power-gain messageType (`MANA`/`RAGE`/`FOCUS`/
/// `ENERGY`; happiness has no message type — the addon shows nothing for it).
fn power_message_type(power: u32) -> Option<&'static str> {
    Some(match power {
        0 => "MANA",
        1 => "RAGE",
        2 => "FOCUS",
        3 => "ENERGY",
        _ => return None,
    })
}

/// `SMSG_SPELLNONMELEEDAMAGELOG` → the spell-damage number (crit on `SPELL_HIT_TYPE_CRIT 0x2`) or
/// the zero-damage ABSORB/RESIST word, over the target (`0x5e85e0`). Source-classified: only my
/// (gold) or my pet's (gold, PetSpellDamage-gated) spells draw. The handler pushes the same
/// record/source to number and word twin alike, so the override colors both.
pub(super) fn spell_damage_log(
    s: SpellDamageLog,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    spells: Option<&crate::ui_action::Spells>,
    gates: DamageTextGates,
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
    // The ungated UNIT_COMBAT feed (portrait hit indicator): any source, any recipient — self
    // included — before the worldtext path's source/Gate-A returns below.
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
    // The center combat text (decision 0578): self recipient only.
    if self_guid.0 == Some(s.target) {
        if let Some((message_type, data)) = spell_center_text(s.damage, s.absorb, s.resist) {
            center.write(CombatTextEvent {
                message_type,
                data,
                extra: None,
            });
        }
    }
    let Some(source) = classify_source(s.attacker, index, self_guid, stores) else {
        return; // K = other: never drawn
    };
    // B = the melee-styled bit: a ranged basic shot (Throw/Auto Shot — AttributesEx3 & 0x8000)
    // floats WHITE off this spell packet, exactly like the client (decision 0376).
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

/// `SMSG_PERIODICAURALOG` → DoT ticks float like direct damage (`0x626dd0`, never crit-category);
/// heal/energize/leech ticks float **nothing** (heals never float in 5875).
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
    // The center combat text (decision 0578): self recipient only — damage ticks like direct
    // damage, heal ticks PERIODIC_HEAL (arg2 = the caster's name, arg3 = amount; the name only
    // displays under COMBAT_TEXT_SHOW_FRIENDLY_NAMES), energize ticks the power-gain family.
    if self_guid.0 == Some(s.target) {
        for tick in &s.ticks {
            let ev = match *tick {
                PeriodicTick::Damage {
                    amount,
                    absorb,
                    resist,
                    ..
                } => spell_center_text(amount, absorb, resist).map(|(message_type, data)| {
                    CombatTextEvent {
                        message_type,
                        data,
                        extra: None,
                    }
                }),
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
                // The displayed figure, not the wire's — `0x626dd0` divides by
                // `0x6e7130(powerType)` at `0x627087` **before** it pushes the COMBAT_TEXT tag
                // (`0x494770`) and before it words the chat line, so both consumers see the same
                // number (decision 2117).
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
    // The ungated UNIT_COMBAT feed: every tick, any source. Damage WOUNDs; heal/energize fire
    // their own actions — they never float as worldtext in 5875, but the portrait indicator is
    // exactly where the green/blue numbers live (CombatFeedback.lua's HEAL/ENERGIZE arms).
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
                // NO ENERGIZE: 5875 never emits it (the string is absent binary-wide); the power
                // gain reaches the center text only.
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
    // Same B computation as the direct-damage emit (the periodic emitter `0x626dd0` pushes the
    // resolved record to the same `0x6128b0` law).
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

/// `SMSG_SPELLDAMAGESHIELD` → the reflected damage lands on the **attacker** (the unit that struck
/// the shield bearer) — that's the recipient the number floats over (`0x5e84e0`). The damage
/// SOURCE is the shield bearer (the victim field); the site pushes record NULL, so it colors
/// melee-styled: my shield → white, my pet's → orange.
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
    // The ungated UNIT_COMBAT feed: the reflected damage is damage TAKEN by the attacker —
    // striking a thorns bearer wounds your own portrait.
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

/// `SMSG_SPELLHEALLOG` → the direct-heal feeds: the portrait indicator's `HEAL` (green number)
/// and the center text's `HEAL`/`HEAL_CRIT` (arg2 = the healer's name, arg3 = amount). Heals
/// never float as worldtext in 5875 — no [`CombatTextSpawn`] here. Center text is self-only;
/// the UNIT_COMBAT feed fires for any streamed recipient (the target frame's API surface).
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

/// `SMSG_SPELLENERGIZELOG` → the center text's `MANA`/`RAGE`/`FOCUS`/`ENERGY` line (self-only).
/// NO `UNIT_COMBAT` fires here: the 5875 engine has no ENERGIZE emission — the string is absent
/// from the whole binary (the shipped CombatFeedback.lua ENERGIZE arm is dead code in 1.12).
///
/// The amount is the **displayed** figure: the handler `0x5e8a90` divides by
/// `0x6e7130(powerType)` at `0x5e8af3` and hands that one number to the COMBAT_TEXT push
/// (`0x494770`) and to the chat formatter (`0x62ca00`) alike (decision 2117).
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

/// Miss code (vmangos `SpellMissInfo`, 1–11) → the center text's spell-outcome messageType (the
/// `SPELL_*` word family — decision 0578, PROVISIONAL).
fn miss_center_type(code: u8) -> Option<&'static str> {
    Some(match code {
        1 => "SPELL_MISSED",
        2 => "SPELL_RESISTED",
        3 => "SPELL_DODGED",
        4 => "SPELL_PARRIED",
        5 => "SPELL_BLOCKED",
        6 => "SPELL_EVADED",
        7 | 8 => "SPELL_IMMUNE",
        9 => "SPELL_DEFLECTED",
        10 => "SPELL_ABSORBED",
        11 => "SPELL_REFLECTED",
        _ => return None,
    })
}

/// Miss code (vmangos `SpellMissInfo`, 1–11) → the `UNIT_COMBAT` action word — the same table the
/// worldtext WORDS index, on the event's uppercase key vocabulary (`CombatFeedbackText`).
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

/// `SMSG_SPELLLOGMISS` → one outcome word per missed target (`0x5e7e00`). Disjoint from the
/// SPELL_GO miss list in practice: vmangos routes only impact-time outcomes (immune, evade) here.
/// Source-classified like every emitter (the classifier lives inside the word twin too), and
/// **coloured by the same B/K law as a number** — `0x5e7f63 mov ecx,[ebp-0x8]; 0x5e7f66 push ecx`
/// pushes the resolved SpellRec, not NULL, so this site's words are spell-GOLD (decision 2229).
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
    // The ungated UNIT_COMBAT feed: the outcome word over each missed target, any source — a
    // mob's bolt you resist is YOUR portrait's "Resist". A self target also feeds the center
    // text's SPELL_* word family (decision 0578).
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
    // **The WORD emitter carries the same three CVar gates as the number emitter.** `0x607140`
    // reads `CombatDamage` (`0x60718f`), `PetSpellDamage` (`0x6071a6`) and `PetMeleeDamage`
    // (`0x6071c5`) exactly as `0x6128b0` does, and each failure jumps to the epilogue — so a miss
    // word is suppressed by the same settings that suppress a number. It made no difference while
    // the three were `const bool` = true; it does now.
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

/// `SMSG_LOG_XPGAIN` → `"XP: %d"` over **self**, category 4 — the one emitter that is
/// self-anchored by design and skips Gate A (`0x607260`). Fires for kill and quest XP alike. The
/// same packet also writes the combat log's own chat line.
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
            color: None, // row 4's own purple — XP skips the damage color law entirely
        });
    }
    chat_log.push_xp_gain(&x);
}

/// `SMSG_EXPLORATION_EXPERIENCE` → the discovery announcement (decision 0828; surfaces and
/// sound from the reference, 0829). Three legs, mirroring the real handler's case body
/// `[0x5e41d2, 0x5e42bb]`:
/// - the ERR_ZONE_EXPLORED toast + (xp > 0) the ERR_ZONE_EXPLORED_XP chat line, queued via
///   [`ChatLog::push_exploration`] — the area name is the packet's `AreaTable.dbc` row id
///   resolved through the shared catalog; a miss (no catalog, or an id the DBC doesn't know)
///   shows no message, faithfully;
/// - the race-keyed discovery jingle (`ChrRaces.dbc` col 3 → SoundEntries), played
///   **unconditionally** — even when the area id resolves to nothing — through the same 2D-kit
///   path as `SMSG_PLAY_SOUND`.
///
/// No floating text (the XP itself still arrives via the non-kill `SMSG_LOG_XPGAIN`).
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

/// `SMSG_LEVELUP_INFO` → the ding's chat lines (decision 0304). The talent-count arg is not on the
/// wire — the client computes `(newLevel >= 10) ? 1 : 0` (`0x5e407c`, the 0305 fold-back), exactly
/// this. The ding's VISUAL is deliberately absent here: it rides the UNIT_FIELD_LEVEL
/// change-watcher (`entities` spell_fx::level_up_flash), never this packet.
pub(super) fn level_up(l: LevelUpInfo, chat_log: &mut ChatLog) {
    let talent_points = u32::from(l.level >= 10);
    // Park the raw tuple for `ui_unit`'s `PLAYER_LEVEL_UP`, whose nine args the reference's own
    // `ChatFrame_OnEvent` reads (1884). **That is the whole of the ding's chat output.**
    //
    // This used to compose and print the five-line block in Rust as well, under a comment saying
    // the Rust lines would stay "until that window migrates and can take over printing them".
    // The window migrated — `benilla.toc` sources `Interface\FrameXML\ChatFrame.xml` and our own
    // transcription is gone (1948) — and nobody came back to the condition, so the block was on
    // screen **twice** (`ui_chat::tests::the_ding_block_is_printed_once` reproduces it: four lines
    // from each side). A deliberate temporary that outlives its trigger is indistinguishable from
    // a permanent one; this is what one looks like when it is found.
    chat_log.push_level_up_gains(&l, talent_points);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat_text::COLOR_SPELL_GOLD;
    use crate::net::{Guid, SelfPlayer};
    use bevy::ecs::system::RunSystemOnce;

    /// **`SMSG_SPELLLOGMISS`'s word is spell GOLD** (decision 2229) — the third and last of the
    /// three word paths this client had drawing white on a hardcoded `color: None`.
    ///
    /// `0x5e7f63 mov ecx,[ebp-0x8]` / `0x5e7f66 push ecx` hands `0x607140` the resolved SpellRec,
    /// not the literal NULL the melee swing site pushes — so the emitter's `B` bit comes from the
    /// spell's own `AttributesEx3` and an ordinary spell's word is gold, exactly like its number.
    /// Unlike the GO's inline emit there is **no Speed test** at this site: the word prints on
    /// receipt whatever the spell's travel time.
    #[test]
    fn the_spell_log_miss_word_is_spell_gold() {
        const IMMOLATE: u32 = 348;

        let spells = crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(
                    IMMOLATE,
                    benilla_formats::SpellDisplay {
                        name: "Immolate".into(),
                        speed: 24.0, // travels, and it still prints here
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
                            misses: vec![(20, 7)], // IMMUNE — the outcome vmangos routes here
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
