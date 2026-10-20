//! The combat log's chat-line leg. The reference's display dispatcher `0x629b60` classifies both
//! endpoints, then branches per outcome into a text formatter, which writes the line and then, for
//! a line about you, its `COMBAT_TEXT_UPDATE` word. The floating number and the portrait's
//! `UNIT_COMBAT` are a separate path ([`super::text`], `0x6128b0`, `0x494600`) with their own CVars
//! and source law. The wording law is [`crate::ui_chat::combat`].
//!
//! Every arm classifies both endpoints, picks the family and chat type, and queues the line for
//! its names. The sentence's subject is not always the packet's `attacker`
//! (`SMSG_SPELLDAMAGESHIELD`).

use bevy::prelude::*;

use benilla_protocol::messages::{
    AttackerState, DamageShield, DispelFailed, EnchantmentLog, EnvironmentalDamageLog,
    PartyKillLog, SpellDispelLog, SpellInstaKillLog, SpellLogExecute, SpellOutcomeLog,
};

use crate::ui_chat::combat::{self, Family, Fills, LineWord, Poses, Stores, UnitClass};
use crate::ui_chat::ChatLog;

use crate::net::{GuidIndex, Reputations, SelfGuid};

mod spell;
use spell::power_drain_line;
pub(super) use spell::{
    periodic_aura_log, spell_damage_log, spell_energize_log, spell_heal_log, spell_log_miss,
};

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
    /// The spell-to-skill-line hop of the caster level `0x6e3130`.
    pub skill_lines: Option<&'a benilla_formats::SkillLineCatalog>,
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

/// vmangos `SpellEffects::SPELL_EFFECT_DISMISS_PET`.
const EFFECT_DISMISS_PET: u32 = 102;

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
