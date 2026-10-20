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
    missile_miss, periodic_aura_log, spell_damage_log, spell_energize_log, spell_go_misses,
    spell_heal_log, spell_log_miss,
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
    /// `SpellItemEnchantment.dbc` (`0xc0d7d8`), which the enchantment lines name.
    pub enchants: Option<&'a benilla_formats::EnchantCatalog>,
    /// The item cache (`0xc0e2a0`), which `LogOpenLock` reads for an opened item.
    pub items: Option<&'a crate::items::Items>,
    /// The game object cache (`0xc0e318`), which `LogOpenLock` reads for an opened object.
    pub gameobjects: Option<&'a crate::go_templates::GameObjectTemplates>,
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

    /// Whether the client holds `guid` as a unit, the `0x468460` lookup with type mask 8.
    fn holds_unit(&self, guid: u64, stores: &impl Stores) -> bool {
        combat::holds_unit(guid, self.index, stores)
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

    /// The record name `LogOpenLock` writes for what a lock opened (`0x62dcb9`-`0x62ddb6`): an
    /// item's, by its entry in the item cache, tested first (type bit 1); else a game object's, by
    /// its entry in the game object cache (bit 5). `None` for any other object, one not held and
    /// one with no cached record.
    fn opened_name(&self, guid: u64, stores: &impl Stores) -> Option<String> {
        let fields = &self.index.0.get(&guid).and_then(|&e| stores.store(e))?.0;
        if fields.is_item() {
            let entry = fields.object_entry()?;
            let item = self
                .items?
                .template(entry, 0, crate::query_cache::Ask::Read)?;
            return Some(item.name.clone());
        }
        if fields.object_type() == Some(benilla_protocol::messages::ObjectType::GameObject) {
            return Some(self.gameobjects?.of(fields)?.name.clone());
        }
        None
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
    s: AttackerState,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &ChatLog,
) {
    let s = super::read_swing(s, ctx.index, stores);
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

/// `SMSG_SPELLDAMAGESHIELD` (`0x5e84e0`): the Thorns-style return hit. The sentence's subject is
/// the packet's `victim`, who wears the shield; `attacker` struck it and takes the damage
/// (`DAMAGESHIELDSELFOTHER` is "You reflect %d %s damage to %s."). The chat type is a two-way on
/// the bearer: `0x62c140` returns `0x3f` `SPELL_DAMAGESHIELDS_ON_SELF` for class 0 or 1, else
/// `0x40` `…ON_OTHERS`. The line (`0x62ca20`, from `0x5e8552`) is worded `DAMAGE_SHIELD` with the
/// damage when the struck unit is you (`0x62cb5d`-`0x62cb6b`), and written for no damage not at all
/// (`0x62ca50`). Then `UNIT_COMBAT` `WOUND` on the attacker, with no flags and no gate (`0x5e858c`).
pub(super) fn damage_shield(
    s: DamageShield,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &ChatLog,
) {
    if s.damage != 0 {
        let bearer = ctx.classify(s.victim, stores);
        let struck = ctx.classify(s.attacker, stores);
        let kind = combat::damage_shield_kind(bearer);
        let fills = Fills {
            school: u8::try_from(s.school).ok(),
            amount: i64::from(s.damage),
            ..Default::default()
        };
        queue_worded(
            log,
            ctx,
            poses,
            kind,
            combat::DAMAGESHIELD,
            (s.victim, bearer),
            (s.attacker, struck),
            fills,
            to_me(struck, LineWord::with("DAMAGE_SHIELD", s.damage)),
        );
    }
    super::feedback::wound(log, s.attacker, s.damage, 0, s.school);
}

/// `SMSG_PARTYKILLLOG`: "You have slain %s!" or "%s is slain by %s!". Only killer class 0
/// (`SELFKILLOTHER`) and 2, a party member (`PARTYKILLOTHER`), produce a line (`0x628890`); a
/// pet's kill gets the plain `UNITDIES*` line instead. The chat type comes off the victim.
pub(super) fn party_kill_log(
    s: PartyKillLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &ChatLog,
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
    log: &ChatLog,
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
/// %s's %s."), both worded target first and typed off the (caster, target) pair. When the target,
/// the second unit, is you, each line raises its word with the spell's name: `PROC_RESISTED`
/// (`0x628b8a`-`0x628b96`) or `SPELL_IMMUNE` (`0x62d3ab`-`0x62d3b9`).
///
/// `IMMUNESPELL`'s `log_format` byte is the periodic flag: `0x62d25f` in `0x62d240` gates that
/// formatter alone on `CombatLogPeriodicSpells` when the byte is set; `PROCRESIST` is ungated.
pub(super) fn spell_outcome_log(
    s: SpellOutcomeLog,
    immune: bool,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &ChatLog,
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
    let (family, word) = if immune {
        (combat::IMMUNESPELL, "SPELL_IMMUNE")
    } else {
        (combat::PROCRESIST, "PROC_RESISTED")
    };
    let word = to_me(target, LineWord::with(word, &spell));
    queue_worded(
        log,
        ctx,
        poses,
        kind,
        family,
        (s.caster, caster),
        (s.target, target),
        Fills {
            spell,
            ..Default::default()
        },
        word,
    );
}

/// `SMSG_SPELLDISPELLOG`: "Your %s is removed." or "%s's %s is removed.", one line per aura. The
/// chat type is the literal `0x45` `SPELL_BREAK_AURA` (`0x62d480`); the dispeller is classified
/// only for the range gate. An `AURADISPELSELF` line raises `SPELL_DISPELLED` with the spell's name
/// (`0x62d5ce`-`0x62d5e8`).
pub(super) fn spell_dispel_log(
    s: &SpellDispelLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &ChatLog,
) {
    let bearer = ctx.classify(s.victim, stores);
    let caster = ctx.classify(s.caster, stores);
    for &spell_id in &s.spell_ids {
        let Some(spell) = ctx.spell_name(spell_id) else {
            continue;
        };
        let word = to_me(bearer, LineWord::with("SPELL_DISPELLED", &spell));
        queue_worded(
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
            word,
        );
    }
}

/// `SMSG_DISPEL_FAILED`: "You fail to dispel %s's %s.", one line per aura. The format variant is
/// picked once per packet (`0x628c20`); the chat type per line, since `0x627d30` asks the spell.
/// When the second unit is you, each line raises `DISPEL_FAILED` with its spell's name
/// (`0x628daa`-`0x628db8`).
pub(super) fn dispel_failed(
    s: &DispelFailed,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &ChatLog,
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
        let word = to_me(victim, LineWord::with("DISPEL_FAILED", &spell));
        queue_worded(
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
            word,
        );
    }
}

/// `SMSG_ENCHANTMENTLOG` (`0x628f40`): "You cast %s on your %s." (`0x6291d0`) or "%s has faded
/// from your %s." (`0x628ff0`, an empty caster guid). The first `%s` is the enchantment's name,
/// read through `0xc0d7d8` by the packet's id; no row, or `Flags & 0x2`, and neither writes
/// anything. The item name, the last `%s`, comes from the item cache, so an uncached entry
/// waits. Each raises its word, `ENCHANTMENT_ADDED` or `ENCHANTMENT_REMOVED` with the
/// enchantment's name, when the item is yours (`0x62937a`, `0x6291b6`).
///
/// The fade needs its owner held as a unit (`0x62902c`) within its class's range; an added line
/// takes the two-sided gate. The chat type is the literal `0x44` `SPELL_ITEM_ENCHANTMENTS` for a
/// fade and for the owner's copy (`show_affiliation` clear); the broadcast copy takes the BUFF
/// matrix (`0x62934e`).
pub(super) fn enchantment_log(
    s: EnchantmentLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &ChatLog,
) {
    let owner = ctx.classify(s.owner, stores);
    let caster = ctx.classify(s.caster, stores);
    let Some(name) = ctx.enchants.and_then(|e| e.logged_name(s.enchant_id)) else {
        return;
    };
    let fills = Fills {
        spell: name.to_string(),
        ..Default::default()
    };
    let enchantments = crate::ui_chat::ChatEventKind::SpellItemEnchantments;
    if s.caster == 0 {
        if !ctx.holds_unit(s.owner, stores) {
            return;
        }
        // A fade: the owner is the only endpoint, in both the key and the range gate.
        queue_reported(
            log,
            ctx,
            poses,
            enchantments,
            combat::ITEMENCHANTMENTREMOVE,
            (s.owner, owner),
            (s.owner, owner),
            fills,
            combat::Named::Item(s.item_entry),
            to_me(owner, LineWord::with("ENCHANTMENT_REMOVED", name)),
        );
        return;
    }
    let kind = if s.show_affiliation {
        match combat::spell_kind(caster, owner, true) {
            Some(k) => k,
            None => return,
        }
    } else {
        enchantments
    };
    queue_reported(
        log,
        ctx,
        poses,
        kind,
        combat::ITEMENCHANTMENTADD,
        (s.caster, caster),
        (s.owner, owner),
        fills,
        combat::Named::Item(s.item_entry),
        to_me(owner, LineWord::with("ENCHANTMENT_ADDED", name)),
    );
}

/// `SMSG_SPELLLOGEXECUTE` (`0x5e7f90`): the lines a cast's effects produce, one formatter per
/// effect id through the index table `0x5e845c` and the jump table `0x5e8430`. An effect with no
/// arm of its own takes the cast line [`spell_cast_line`] once per row, with no target
/// (`0x5e83e3`); so does a packet whose one group is a row-less effect 3 (`0x5e8004`-`0x5e8034`).
pub(super) fn spell_log_execute(
    s: &SpellLogExecute,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &ChatLog,
) {
    use benilla_protocol::messages::ExecuteLog as E;

    let caster = ctx.classify(s.caster, stores);
    // Only the arms that pass the packet's spell (`[ebp-0x14]`) to their formatter test it: power
    // drain, extra attacks, open lock, durability damage and the cast line. Create item, feed pet
    // and dismiss pet name no spell, and interrupt names the row's.
    let spell = || ctx.spell_name(s.spell_id);
    let cast_line = |log: &ChatLog, target: Option<u64>| {
        spell_cast_line(
            log,
            ctx,
            stores,
            poses,
            s.spell_id,
            (s.caster, caster),
            target,
        );
    };
    if let [(EFFECT_DUMMY, rows)] = s.effects.as_slice() {
        if rows.is_empty() {
            return cast_line(log, None);
        }
    }
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
                    let Some(spell) = spell() else { continue };
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
                // unit that gained the attacks, typed by the spell's hostility (`0x62b190`, from
                // `0x62db78`) and worded `EXTRA_ATTACKS` with the count when that is you
                // (`0x62db7d`-`0x62dba9`); a count of 0 writes neither (`0x62da2a`).
                E::ExtraAttacks { target, count } => {
                    if count == 0 {
                        continue;
                    }
                    let Some(spell) = spell() else { continue };
                    let victim = ctx.classify(target, stores);
                    let buff = !ctx.spell_harmful(s.spell_id);
                    let Some(kind) = combat::spell_kind(victim, victim, buff) else {
                        continue;
                    };
                    queue_worded(
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
                            spell,
                            amount: i64::from(count),
                            ..Default::default()
                        },
                        to_me(victim, LineWord::with("EXTRA_ATTACKS", count)),
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
                // not the packet's own, and so does the `INTERRUPT` word when the interrupted unit
                // is you (`0x626b53`-`0x626b69`).
                E::InterruptCast { target, spell_id } => {
                    let victim = ctx.classify(target, stores);
                    let Some(interrupted) = ctx.spell_name(spell_id) else {
                        continue;
                    };
                    let Some(kind) = combat::spell_kind(caster, victim, false) else {
                        continue;
                    };
                    let word = to_me(victim, LineWord::with("INTERRUPT", &interrupted));
                    queue_worded(
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
                        word,
                    );
                }
                // Effect 111 DURABILITY_DAMAGE: both fields `-1` is the "all items" form.
                E::DurabilityDamage {
                    target,
                    item_entry,
                    slot,
                } => {
                    let Some(spell) = spell() else { continue };
                    let spell_fill = || Fills {
                        spell: spell.clone(),
                        ..Default::default()
                    };
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
                // Effect 1 INSTAKILL: the cast line on its target, unless the caster killed
                // itself (`0x5e81a2`-`0x5e81b1`).
                E::Target { target } if *_effect == EFFECT_INSTAKILL => {
                    if target != s.caster {
                        cast_line(log, Some(target));
                    }
                }
                E::Target { target } if cast_names_target(*_effect) => {
                    cast_line(log, Some(target));
                }
                // Effects 33 and 59 OPEN_LOCK (`0x5e82b2`): [`open_lock_line`].
                E::Target { target }
                    if matches!(*_effect, EFFECT_OPEN_LOCK | EFFECT_OPEN_LOCK_ITEM) =>
                {
                    open_lock_line(
                        log,
                        ctx,
                        stores,
                        poses,
                        s.spell_id,
                        (s.caster, caster),
                        target,
                    );
                }
                // The default arm, which reads nothing of the row: heals and energizes, which are
                // worded from their own packets, the summons and the rest.
                E::Heal { .. } | E::Energize { .. } | E::Target { .. } => cast_line(log, None),
            }
        }
    }
}

/// vmangos `SpellEffects` ids (`SpellDefines.h`) for the arms keyed on them.
const EFFECT_INSTAKILL: u32 = 1;
const EFFECT_DUMMY: u32 = 3;
const EFFECT_OPEN_LOCK: u32 = 33;
const EFFECT_LEARN_SPELL: u32 = 36;
const EFFECT_OPEN_LOCK_ITEM: u32 = 59;
const EFFECT_DISMISS_PET: u32 = 102;

/// The effects whose rows go to the cast line with their target, arm 2 of the index table
/// `0x5e845c` (`0x5e81d8`): resurrect, dispel, threat, distract, sanctuary, threat-all, dispel
/// mechanic, resurrect-new, attack-me, skin-player-corpse, modify-threat-percent and 126.
fn cast_names_target(effect: u32) -> bool {
    matches!(
        effect,
        18 | 38 | 63 | 69 | 79 | 91 | 108 | 113 | 114 | 116 | 125 | 126
    )
}

/// `LogOpenLock` (`0x62dc10`), the line for `SMSG_SPELLLOGEXECUTE`'s effects 33 and 59: "You perform
/// Pick Lock on Copper Vein." After the spell gates, the caster must be held as a unit within its
/// class's range (`0x628010`). The target must be a held item with a record in the item cache
/// (`0x62dce2`) or a held game object with one in the game object cache (`0x62ddac`), read
/// without asking; the line names its record. `OPEN_LOCK_SELF` or `OPEN_LOCK_OTHER` on the
/// caster, typed by the caster against itself through the spell's hostility (`0x627d30`). No word
/// follows.
fn open_lock_line(
    log: &ChatLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    spell_id: u32,
    caster: (u64, UnitClass),
    target: u64,
) {
    let Some(spell) = ctx.spell_name(spell_id) else {
        return;
    };
    if !ctx.holds_unit(caster.0, stores) {
        return;
    }
    let Some(opened) = ctx.opened_name(target, stores) else {
        return;
    };
    let Some(kind) = combat::spell_kind(caster.1, caster.1, !ctx.spell_harmful(spell_id)) else {
        return;
    };
    queue(
        log,
        ctx,
        poses,
        kind,
        combat::OPEN_LOCK,
        caster,
        caster,
        Fills {
            spell,
            named: opened,
            ..Default::default()
        },
    );
}

/// `SPELL_ATTR_IS_ABILITY` (vmangos `SpellDefines.h:796`): the cast line says "performs" (`0x62aff0`).
const ATTR_IS_ABILITY: u32 = 0x10;

/// `AttributesEx4` bit 0, which keeps a spell out of the cast line (`0x62ad2d`).
const ATTR_EX4_NO_CAST_LOG: u32 = 0x1;

/// `0x62ace0`, the cast line: "Priest casts Cleanse on you.", or with no target "You cast
/// Vanish.". After the spell gates it skips a spell with a learn-spell effect (`0x62ad70`),
/// `AttributesEx4 & 1` (`0x62ad2d`) or an open-lock effect (`0x62ad90`).
///
/// With a target, `0x62adc0`: the two-sided range gate, `SIMPLECAST` or for an ability
/// `SIMPLEPERFORM`, typed by the spell's damage or buff matrix (`0x627d30`), worded `SPELL_CAST`
/// with the spell's name when the target is you (`0x62aef7`). With none, `0x62b000`: only a caster
/// the client holds as a unit, within its own class's range (`0x62b030`-`0x62b0c3`), `SPELLTERSE`
/// or `SPELLTERSEPERFORM` keyed on the caster, typed by the matrix on the caster against itself
/// (`0x62b190`), and worded when the caster is you (`0x62b12f`).
fn spell_cast_line(
    log: &ChatLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    spell_id: u32,
    caster: (u64, UnitClass),
    target: Option<u64>,
) {
    let Some(display) = combat::logged_spell(ctx.spells, spell_id) else {
        return;
    };
    let has_effect = |ids: &[u32]| display.effects.iter().any(|e| ids.contains(e));
    if has_effect(&[EFFECT_LEARN_SPELL])
        || display.attributes_ex4 & ATTR_EX4_NO_CAST_LOG != 0
        || has_effect(&[EFFECT_OPEN_LOCK, EFFECT_OPEN_LOCK_ITEM])
    {
        return;
    }
    let ability = display.attributes & ATTR_IS_ABILITY != 0;
    let buff = !ctx.spell_harmful(spell_id);
    let fills = Fills {
        spell: display.name.clone(),
        ..Default::default()
    };
    let word = LineWord::with("SPELL_CAST", &display.name);
    // A zero guid is no target (`0x62ad3e`).
    match target.filter(|&g| g != 0) {
        Some(target) => {
            let target = (target, ctx.classify(target, stores));
            let Some(kind) = combat::spell_kind(caster.1, target.1, buff) else {
                return;
            };
            let family = if ability {
                combat::SIMPLEPERFORM
            } else {
                combat::SIMPLECAST
            };
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
        None => {
            if !ctx.holds_unit(caster.0, stores) {
                return;
            }
            let Some(kind) = combat::spell_kind(caster.1, caster.1, buff) else {
                return;
            };
            let family = if ability {
                combat::SPELLTERSEPERFORM
            } else {
                combat::SPELLTERSE
            };
            queue_worded(
                log,
                ctx,
                poses,
                kind,
                family,
                caster,
                caster,
                fills,
                to_me(caster.1, word),
            );
        }
    }
}

/// `SMSG_ENVIRONMENTALDAMAGELOG`'s handler `0x624f30`, which writes and signals nothing for a hit
/// of all zeros (`0x624f4c`-`0x624f60`). The line, "You fall and lose %d health." and its five
/// siblings: `0x62aac0` builds the key from a six-entry damage-type table and SELF/OTHER, and calls
/// the melee HITS selector with the victim's class in both positions, so a party member's fall
/// types as `HOSTILEPLAYER_HITS` through the selector's tgt<=3 override, as in the reference. It
/// takes absorb and resist trailers, and on you raises [`combat::damage_word`] (`0x62ac74`). Then,
/// for the active player (`0x624f83`), `UNIT_COMBAT` `WOUND` in the type's school, marked `0x20`
/// for an absorb and `0x40` for a resist (`0x624f8c`-`0x624fc4`).
pub(super) fn environmental_damage_log(
    e: EnvironmentalDamageLog,
    ctx: &ChatCtx,
    stores: &impl Stores,
    poses: &impl Poses,
    log: &ChatLog,
) {
    if e.damage == 0 && e.absorb == 0 && e.resist == 0 {
        return;
    }
    let victim = ctx.classify(e.victim, stores);
    let kind = combat::combat_kind(victim, victim, false);
    if let (Some(family), Some(kind)) = (combat::env_family(e.damage_type), kind) {
        queue_worded(
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
            to_me(victim, combat::damage_word(e.damage, e.absorb, e.resist)),
        );
    }
    let school = ENV_DAMAGE_SCHOOLS.get(usize::from(e.damage_type));
    if let (Some(&school), true) = (school, ctx.self_guid.0 == Some(e.victim)) {
        let marks = super::feedback::hit_flags(false, e.absorb, e.resist, 0);
        super::feedback::wound(log, e.victim, e.damage, marks, school);
    }
}

/// The school of each environmental damage type 0 to 5, the table `0x809274` that `0x624fb3`
/// indexes: fatigue, drowning and falling physical, lava and fire fire, slime nature. The index has
/// no bound; a type past it, which no server sends (vmangos maps its void fall to 2,
/// `Objects/Unit.cpp:5396`), signals nothing here.
const ENV_DAMAGE_SCHOOLS: [u32; 6] = [0, 0, 0, 2, 3, 2];

/// `SMSG_SET_FACTION_STANDING`: "Your %s reputation has increased by %d.". The wire carries the
/// new total and the sentence the delta, so this runs before the store is overwritten, and an
/// unchanged value prints nothing (`0x4d6344`). Only the packet's first entry prints, and only if
/// its faction is not `HIDDEN`: the handler passes `0x4d6330` its print argument as
/// `index == 0 && !(flags & 4)` (`0x4d57dd`-`0x4d57ea`), so the spill-over entries of one kill
/// update silently. No range gate or classifier: the chat type is the literal `0x55`
/// `COMBAT_FACTION_CHANGE`. The wire's list id is `Faction.dbc`'s `rep_index`. The line
/// (`0x62c5f0`) raises `FACTION` with the faction's name and the signed change, whoever's line it
/// is (`0x62c661`-`0x62c667`).
pub(crate) fn faction_standing(
    deltas: &[(u32, i32)],
    reputations: &Reputations,
    factions: Option<&crate::target::ring::Factions>,
    log: &ChatLog,
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
            word: Some(LineWord::text_number("FACTION", name, delta)),
        });
    }
}

/// The tail every arm ends in: build the queued line (dropping a class-9 endpoint) and park it
/// for its names.
fn queue(
    log: &ChatLog,
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
    log: &ChatLog,
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
    log: &ChatLog,
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
    log: &ChatLog,
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
