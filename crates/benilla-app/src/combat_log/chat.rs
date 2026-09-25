//! The combat log's **chat line** leg — one of the two consumers of every combat packet;
//! the floating number is [`super::text`]'s, and [`super`]'s handlers call this leg first.
//!
//! The split is the reference's own. `0x629b60` is the display dispatcher: one classification of
//! both endpoints, then a branch per outcome into a formatter that emits *text*. The floating
//! number comes off a different path entirely (`0x629d30`, which fires `COMBAT_TEXT_UPDATE`), gated by
//! different CVars, with a different source law — which is why our two live in different files
//! rather than in one arm with two tails. Everything here composes; the law it composes with is
//! [`crate::ui_chat::combat`].
//!
//! **Every arm has the same three steps**: classify both endpoints, pick the family and the chat
//! type, queue the line for its names. What differs between them is only which packet field is the
//! sentence's subject, which is not always the packet's own `attacker`
//! (`SMSG_SPELLDAMAGESHIELD` is the standing counter-example).

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

/// The classification inputs every line needs, bundled so seven legs do not each grow six
/// parameters. Assembled per packet by [`super`]'s handlers from [`super::Log`].
pub(crate) struct ChatCtx<'a> {
    pub self_guid: &'a SelfGuid,
    pub group: Option<&'a crate::ui_party::GroupState>,
    pub index: &'a GuidIndex,
    pub factions: Option<&'a crate::target::ring::Factions>,
    pub reputations: &'a Reputations,
    pub spells: Option<&'a crate::ui_action::Spells>,
    /// The live display ranges — the reference's `0x8629e0` table, now CVar-backed.
    pub ranges: &'a combat::CombatLogRanges,
    /// `CombatLogPeriodicSpells`. The whole-packet gate lives at the `PeriodicAuraLog` dispatch
    /// arm (it suppresses the floats too); this carries the two *chat-only* read sites.
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

    /// One endpoint's half of the display-range gate — the law is
    /// [`combat::in_range`]; this only supplies the context it reads.
    fn in_range(&self, guid: u64, class: UnitClass, poses: &Query<&mut Transform>) -> bool {
        combat::in_range(
            guid,
            self.ranges.class(class),
            self.self_guid,
            self.index,
            poses,
        )
    }

    /// A spell's display name, or `None` when the reference would emit **no line at all** for this
    /// spell (`0x62cd80`, like every spell formatter). Two gates, both of which 1571 was missing:
    ///
    /// - **`Attributes & 0x180`** — `SPELL_ATTR_DO_NOT_DISPLAY | SPELL_ATTR_DO_NOT_LOG` (the mask
    ///   and the `SpellRec+0x18` offset are the binary's; the enum names are inferred from vmangos
    ///   `SpellDefines.h:799-800`). This is what keeps the invisible book-keeping spells every
    ///   server casts constantly — proc triggers, aura tickers — out of the log entirely.
    /// - **An empty localized name.** The reference gates on the name it is about to print, so a
    ///   row with no text in this locale produces silence rather than a sentence with a hole in it.
    ///
    /// A spell id the catalog cannot answer at all is *not* gated: that is our own missing data,
    /// not the server's intent, and a nameless sentence still carries the numbers a damage meter
    /// needs. The reference degrades the same way through `GetObjectName`'s `"UKNOWNOBJECT"` tail.
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

    /// `0x6ea280 == 2` — **"this spell targets enemies"**, the predicate the chat-type stubs
    /// `0x627d30`/`0x627d60` run to choose between a family's `…_DAMAGE` and `…_BUFF` rows.
    /// It reads the row, never the endpoints, which is why it lives beside
    /// [`Self::spell_name`] and not beside the classifier.
    ///
    /// A row the catalog cannot answer reads as **harmful**, which is not a claim about the
    /// spell — it is our own data gap, and the reference never reaches the question (it has
    /// already bailed on a missing record at the `SpellRec+0x18` gate above, where we deliberately
    /// carry on: see [`Self::spell_name`]). Harmful is chosen because it is the `…_DAMAGE` row
    /// every one of these sites used unconditionally before the consult existed, so a missing row
    /// keeps the behaviour it had rather than acquiring a new one.
    fn spell_harmful(&self, spell_id: u32) -> bool {
        self.spells
            .and_then(|s| s.catalog.get(spell_id))
            .is_none_or(benilla_formats::SpellDisplay::is_harmful)
    }
}

/// `SMSG_ATTACKERSTATEUPDATE` → the melee line. The whole `COMBAT_*` half of the block comes from
/// this one packet.
///
/// `school` is the swing's first sub-damage school, which is what selects the `…SCHOOL` template
/// ("You hit X for 5 fire damage." against "You hit X for 5."). Physical (0) takes the plain form.
pub(super) fn attacker_state(
    s: AttackerState,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
    log: &mut ChatLog,
) {
    // `0x625e40`, the first of the packet's two whole-display gates and the one that runs before
    // the dispatcher is even entered (`0x629b3e` → `0x629b45 je`): a swing that came from a spell
    // and did not land plainly emits **no line at all**. The floating number is gated by the same
    // predicate at its own site.
    if !s.displayed() {
        return;
    }
    let attacker = ctx.classify(s.attacker, stores);
    let victim = ctx.classify(s.victim, stores);
    // `0x62a710`'s else-arm declines for VictimState 0/1/4/9 — the reference emits nothing at all
    // for a swing that landed in one of those states, so neither does the packet.
    let Some(family) = combat::melee_family(s.hit_info, s.victim_state, s.damage, s.school) else {
        return;
    };
    // A swing is a MISS line only for the outcomes the reference words as one; everything the
    // victim did about it (dodge, parry, block, …) is still a "misses" chat TYPE, because the type
    // pair is hits/misses and only a landed hit is "hits".
    let landed = family.stem.starts_with("COMBATHIT");
    let Some(kind) = combat::combat_kind(attacker, victim, !landed) else {
        return;
    };
    let fills = Fills {
        spell: String::new(),
        school: (s.school != 0).then_some(s.school),
        amount: i64::from(s.damage),
        // **Only the landed-hit family grows a trailer**, and it is the only family in the whole
        // block that can show GLANCING/CRUSHING/BLOCK: `0x628410`'s four call sites, and this is
        // the one that passes a real `blocked` and a real `HitInfo` (`0x629ee7`). A dodge or a full
        // absorb prints no amount at all — the reference does not append to those lines.
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

/// `SMSG_SPELLNONMELEEDAMAGELOG` → a spell's damage line, or the fully-absorbed / fully-resisted
/// wording when nothing got through.
///
/// The packet's own `periodic` flag routes the line to the `SPELL_PERIODIC_*` types and the
/// `PERIODICAURADAMAGE` wording — a DoT tick the server chose to report here rather than through
/// `SMSG_PERIODICAURALOG` is still a periodic tick, and the reference tells them apart the same way.
pub(super) fn spell_damage_log(
    s: SpellDamageLog,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
    log: &mut ChatLog,
) {
    let attacker = ctx.classify(s.attacker, stores);
    let victim = ctx.classify(s.target, stores);
    // `SPELL_HIT_TYPE_CRIT` (vmangos `SpellDefines.h:179`) — the same bit the floating text reads.
    let crit = s.hit_info & 0x2 != 0;
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };

    // **Split damage takes the packet before anything else does** (`hit_info & 8` → `0x62de60`,
    // the handler's FIRST test at `0x5e8...`, before the periodic split): a Soul Link-style share reads "%s's
    // %s causes %s %d damage.", never "hits %s for %d". It has no `…SELFSELF` key and it grows no
    // trailers.
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
        // The second of `CombatLogPeriodicSpells`' two chat-only read sites: `0x62d9ae` inside
        // `0x62d9a0` skips the `call 0x628100` line emit and nothing else — both branches converge
        // and still reach the float emitters, which is why this returns instead of gating the arm.
        if !ctx.periodic {
            return;
        }
        // **The TARGET's class, not the caster's**: this leg lands in the shared
        // `PERIODICAURADAMAGE` formatter `0x628100`, whose msg-id selector takes `outClassB`
        // (`0x628235`), and B is the victim at every call site.
        let Some(kind) = combat::periodic_kind(victim, false) else {
            return;
        };
        let fills = Fills {
            spell,
            school: Some(s.school),
            amount: i64::from(s.damage),
            // `0x628341`, the periodic call site: absorb and resist only — it passes zero for
            // blocked and for HitInfo, so a periodic line can never say "(blocked)" or
            // "(crushing)".
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
    // Nothing through: the reference words it as the reason rather than as a zero, and the order
    // of the three tests is its own — absorb, then BLOCK, then resist (`0x62cd80`).
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
    // `0x62d03c`, the spell call site: it passes a real `blocked` but **HitInfo 0**, so a spell
    // line can show RESIST/VULNERABLE/BLOCK/ABSORB and never GLANCING or CRUSHING. Only the
    // families that actually print a damage number take one — the reference appends nothing to
    // the "was absorbed / was blocked / was resisted" wordings, which name the reason instead.
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

/// `SMSG_SPELLLOGMISS` → one line per target a cast failed to land on, each worded by its own
/// `SpellMissInfo`.
///
/// The packet is a *list*, and each entry is its own sentence with its own victim — so the chat
/// type is recomputed per entry, not once for the cast.
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
    // **Every line off this packet is typed as a DAMAGE-SHIELD line**, and that is not a guess:
    // the `SMSG_SPELLLOGMISS` caller passes the formatter's 5th argument as 1 (`0x5e7f31 push 1`),
    // which routes `0x62bc5a` to `0x62c140` — the damage-shield two-way selector — instead of the
    // usual eight-row spell matrix. Its five other callers pass 0 and take the normal path.
    //
    // Whether that is deliberate or a 1.12 bug is **not derivable from the binary**. We reproduce
    // the behaviour, because an addon filtering on
    // chat type has to see what the reference shows; the discriminator, if anyone wants it, is a
    // live cast that misses and a look at which ChatFrame filter catches the line. 1571 sent these
    // through `spell_kind` and had them land in `SPELL_*_DAMAGE`.
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

/// `SMSG_SPELLHEALLOG` → a heal line. A heal is a **BUFF** type, not a damage one.
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

/// `SMSG_SPELLENERGIZELOG` → a power-gain line, also a BUFF type.
///
/// **The wire amount is RAW and the sentence wants the displayed figure**: the
/// reference's own handler `0x5e8a90` divides by `0x6e7130(powerType)` at `0x5e8af3` before it
/// hands the number to anything, so a warrior's one point of Unbridled Wrath rage — 10 on the
/// wire — words as *"You gain 1 Rage from …"*, not 10.
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

/// `SMSG_PERIODICAURALOG` → one line per tick, worded by the tick's aura type.
///
/// The periodic chat types split DAMAGE from BUFFS and **ignore the victim entirely** (their two
/// selectors take one argument), which is why the kind is picked from `caster` alone here.
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
                    // The periodic packet's school is a full u32; the template's slot is the same
                    // `SPELL_SCHOOL<n>_NAME` index the direct-damage one takes.
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
            // The same divide as the direct packet, from the same law and the same table — the
            // reference's periodic handler `0x626dd0` applies it at `0x627087` before wording.
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
            // **Aura 64 is not an arm of this match at all.** It reaches the very same function
            // the execute-log's `POWER_DRAIN` effect does — `0x627910` → `0x627930`, with the
            // periodic flag set — so every choice that function makes (the LEECH/DRAIN fork, the
            // happiness arm and its fall-through, the out-of-range drop, the chat-type stub)
            // belongs to it and not to a constant written out here. This arm used to word
            // `SPELLPOWERLEECH` unconditionally against an execute-log arm that forked three ways:
            // one formatter, two call sites that disagreed (2117's named residue, closed by 2157).
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

/// `SMSG_SPELLDAMAGESHIELD` → the Thorns-style return hit.
///
/// **The sentence's subject is the packet's `victim`.** The wire names the fields from the original
/// swing's point of view — `victim` wears the shield, `attacker` struck them and now takes the
/// damage back — while `DAMAGESHIELDSELFOTHER` is "You reflect %d %s damage to %s.", whose subject
/// is the reflector. Getting this backwards would name both endpoints in the wrong halves of every
/// line, and nothing downstream would notice.
///
/// The chat type is a plain two-way split on the **bearer**, not the usual eight-row matrix:
/// `0x62c140` returns `0x3f` (`SPELL_DAMAGESHIELDS_ON_SELF`) for class 0 or 1 and `0x40`
/// (`…ON_OTHERS`) for everything else — "damage shields on self" meaning the shield that is on you.
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

/// `SMSG_PARTYKILLLOG` → "You have slain %s!" / "%s is slain by %s!".
///
/// **Only two killer classes produce a line at all** (`0x628890`, which has no selector): class 0
/// is `SELFKILLOTHER`, class 2 (a party member) is `PARTYKILLOTHER`, and **everything else —
/// including your own pet at class 1 — emits nothing**. That is the reference's own shape, not a
/// simplification: a pet's kill is announced by the plain `UNITDIES*` line instead.
///
/// The chat type comes off the **victim**, through the death selector.
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

/// `SMSG_SPELLINSTAKILLLOG` → "You are killed by %s." / "%s is killed by %s."
///
/// `0x62cbe0` calls the spell-damage selector with the victim's class in **both** positions
/// (`0x626be0(class, class)`), so the type never splits on a second endpoint — there isn't one.
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

/// `SMSG_PROCRESIST` → "%s resists %s's %s." and `SMSG_SPELLORDAMAGE_IMMUNE` → "%s is immune to
/// %s's %s." — one body, two sentences.
///
/// Both word the TARGET first (the reference's convention B), and both take their chat type from
/// the (caster, target) pair — `PROCRESIST` always through the damage/buff stub `0x627d30`, and
/// `IMMUNESPELL` through the plain damage selector `0x626be0`. The difference is real: an immunity
/// to a *helpful* spell still files under `…_DAMAGE`.
///
/// **`IMMUNESPELL`'s `log_format` byte is the reference's "is periodic" flag, and it is now read.**
/// `0x62d25f` inside `0x62d240` gates *this formatter only*, and only when that byte is set — so a
/// direct immunity still prints with `CombatLogPeriodicSpells` off, and a periodic one does not.
/// `PROCRESIST` is not gated at all: the read site is in the `SPELLORDAMAGE_IMMUNE` arm.
pub(super) fn spell_outcome_log(
    s: SpellOutcomeLog,
    immune: bool,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
    log: &mut ChatLog,
) {
    // The periodic gate, on the one arm that has it: `IMMUNESPELL` with the packet's periodic
    // byte set. `PROCRESIST` shares this body but not the gate — its formatter has no read site.
    if immune && s.log_format != 0 && !ctx.periodic {
        return;
    }
    let caster = ctx.classify(s.caster, stores);
    let target = ctx.classify(s.target, stores);
    let Some(spell) = ctx.spell_name(s.spell_id) else {
        return;
    };
    // The two sentences do not share a selector, and this is where that bites: `PROCRESIST` goes
    // through the damage/buff stub `0x627d30`, which asks the SPELL (`0x6ea280 == 2`), while
    // `IMMUNESPELL` goes straight to the plain damage selector `0x626be0`. So an immunity to a
    // *helpful* spell still files under `…_DAMAGE`, and a resisted helpful proc does not.
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

/// `SMSG_SPELLDISPELLOG` → "Your %s is removed." / "%s's %s is removed.", one line per aura.
///
/// The chat type is **hard-coded `0x45` `SPELL_BREAK_AURA`** (`0x62d480`) — it consults no class at
/// all, which is why this is the one arm that never calls a selector. The dispeller is classified
/// only for the range gate; the sentence names the bearer and the aura and nothing else.
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

/// `SMSG_DISPEL_FAILED` → "You fail to dispel %s's %s.", one line per aura that would not come off.
///
/// **The reference picks the format-string variant ONCE, before the loop** (`0x628c20`), so every
/// line in a packet shares it — which is the same answer ours reaches, since both endpoints are the
/// same for the whole packet. The chat type is re-derived per line only because the msg-id stub
/// `0x627d30` consults the spell, and the spells differ.
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
        // Per line, because `0x627d30` consults the spell and the spells differ — which is what
        // this arm's doc has said since it was written, over a `false` hoisted out of the loop.
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

/// `SMSG_ENCHANTMENTLOG` → "You cast %s on your %s." / "%s has faded from your %s."
///
/// **An empty caster guid is how the server says the enchant FADED** (vmangos's own comment on the
/// field, and the reference's two-way at `0x628f40`); the fade names only the owner, which is why
/// it drops from four keys to two. The item name is the last `%s` in every variant and comes from
/// the item cache, so a line whose entry is not cached yet waits rather than printing a hole.
///
/// The chat type is the literal `0x44` `SPELL_ITEM_ENCHANTMENTS` on the fade leg and on the ADD leg
/// when `show_affiliation` is clear — the copy the server sends to the item's own owner. The
/// broadcast copy (affiliation set) takes the BUFF selector instead, which is what puts a
/// bystander's enchant into their `SPELL_*_BUFF` bucket rather than the item block.
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

/// `SMSG_SPELLLOGEXECUTE` → the lines a cast's *effects* produce, as opposed to the damage it
/// dealt: what it created, fed, interrupted, drained, dismissed or damaged.
///
/// **One packet, many formatters.** The wire is a list of groups keyed by spell-effect id, and the
/// reference's own per-effect jump table (`0x5e8074`) sends each to a different formatter with a
/// different family, a different chat type and a different argument convention. So this arm is a
/// switch, not a sentence — the seven effects below are the ones that word themselves.
///
/// **Deliberately not wired, and named rather than dropped**: effects 33/59
/// `OPEN_LOCK` (`OPEN_LOCK_{SELF,OTHER}` — its trailing `%s` is a *gameobject* name, and benilla
/// has no GO-name cache to resolve one from a guid), and the `SIMPLECAST*`/`SIMPLEPERFORM*`/
/// `SPELLTERSE_*` catch-all the reference falls back to for the guid-only tail of the switch (its
/// bail conditions read `AttributesEx4` and the spell's three `Effect` columns, and `Spell.dbc`
/// column 10 is not parsed into `SpellDisplay` yet). Both are additions, not corrections: nothing
/// below changes when they land.
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
                // Effect 8 POWER_DRAIN — the non-periodic entry to the shared formatter
                // [`power_drain_line`] (`0x627930` with the periodic flag clear). Every family
                // choice this row can make lives there, because the periodic packet makes the
                // same ones through the same function.
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
                // Effect 19 ADD_EXTRA_ATTACKS. **The caster guid is never passed** (`0x62d9f0`):
                // the sentence's only endpoint is the unit that gained the attacks, and the
                // singular form is the same key with `_SINGULAR` appended.
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
                // Effect 24 CREATE_ITEM — the tradeskill line, at the literal `0x3e`. No target
                // guid on the wire: the two-way is on the CASTER being you.
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
                // Effect 101 FEED_PET — the same shape as CREATE_ITEM, a different sentence.
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
                // Effect 68 INTERRUPT_CAST. **The spell the sentence names is the INTERRUPTED
                // one**, off the row, not the interrupting cast off the packet header — which is
                // the whole point of the line.
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
                // Effect 111 DURABILITY_DAMAGE. Both fields `-1` is the "all items" form, which
                // has its own family and names no item at all.
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
                // Effect 102 DISMISS_PET arrives in the guid-only tail; the reference's two-way is
                // on the caster being you, at the literal misc-info type, and the named thing is
                // the pet.
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
                // Heals and energizes off this packet are the floating text's business, not the
                // chat log's: the reference words them from SMSG_SPELLHEALLOG / SPELLENERGIZELOG,
                // which arrive separately and already have arms. The rest of the guid-only tail is
                // the SIMPLECAST catch-all named in this function's docs.
                E::Heal { .. } | E::Energize { .. } | E::Target { .. } => {}
            }
        }
    }
}

/// vmangos `Powers`: happiness, the one power with no `…_POINTS` GlobalString — `0x6278f0` returns
/// NULL for it, which is why it takes its own family instead of a `POWERGAIN` row.
const POWER_HAPPINESS: u32 = 4;

/// vmangos `SpellEffects::SPELL_EFFECT_DISMISS_PET`.
const EFFECT_DISMISS_PET: u32 = 102;

/// `|multiplier| >= 2^-22` — the reference's own leech/drain discriminator (`[0x8029d4]`).
const LEECH_EPSILON: f32 = 1.0 / 4_194_304.0;

/// `0x6e7130(powerType)` as the log formatters take it — [`power_display_scale`]'s table, widened
/// for the `i64` arithmetic every power line does.
///
/// It delegates rather than re-tabulating: this file used to carry its own copy of `{1, 10, 1, 1,
/// 1000}`, and the copy was applied at exactly one of the four sites that need it (decision 2117).
fn power_divisor(power: u32) -> i64 {
    i64::from(power_display_scale(power))
}

/// The figure a `POWERGAIN` line words: the wire amount on the display scale.
///
/// The reference does this **in the packet handler**, once per packet — `0x5e8af3` for
/// `SMSG_SPELLENERGIZELOG`, `0x627087` for an energize tick — so the chat line and the
/// `COMBAT_TEXT_UPDATE` push can never disagree. We have two consumers instead of one call, so the
/// law is a named function rather than a local.
fn power_gain(power: u32, amount: u32) -> i64 {
    i64::from(amount) / power_divisor(power)
}

/// Which endpoint a periodic tick's formatter hands its **msg-id selector** — the choice that
/// decides whether a line types as `SPELL_PERIODIC_SELF_*` or `SPELL_PERIODIC_CREATURE_*`.
///
/// It is a function, exhaustive over the wire enum, because the four periodic formatters disagree
/// and one shared `periodic_kind(caster, …)` was wrong for half of them: `0x628100`
/// (`PERIODICAURADAMAGE`) and `0x627240` (`PERIODICAURAHEAL`) hand the selector `outClassB`, the
/// **target** (`0x628235`, `0x62732c`); `0x627520` (`POWERGAIN`) and `0x627930`
/// (`SPELLPOWERLEECH`/`…DRAIN`) hand it `outClassA`, the **caster** (`0x6275fb`,
/// `0x627a0f`/`0x627a3a`). A new tick kind has to answer this question rather than inherit an
/// answer.
///
/// The `ManaLeech` row is the table's record of the fourth formatter and is what the test pins;
/// the tick itself is worded by [`power_drain_line`], which applies the same row directly (it is
/// handed the caster and the target, not the tick, because its other call site has no tick).
fn periodic_subject(tick: &PeriodicTick, caster: UnitClass, target: UnitClass) -> UnitClass {
    match tick {
        PeriodicTick::Damage { .. } | PeriodicTick::Heal { .. } => target,
        PeriodicTick::Energize { .. } | PeriodicTick::ManaLeech { .. } => caster,
    }
}

/// Arm 5 of `0x627930`: **a real multiplier says the caster gained what the target lost**.
///
/// `6279ff fcomp dword ptr [0x8029d4]` against `|multiplier|`, and `[0x8029d4]` is `2^-22`
/// (`00 00 80 34`) — the reference's own epsilon, not a round number of ours. The compare's `jp`
/// takes the greater-or-equal side, so **equality is a leech**; only a strictly smaller magnitude
/// is the plain `SPELLPOWERDRAIN`.
fn leech_family(multiplier: f32) -> Family {
    if multiplier.abs() >= LEECH_EPSILON {
        combat::SPELLPOWERLEECH
    } else {
        combat::SPELLPOWERDRAIN
    }
}

/// The `(drained, gained)` pair a leech/drain sentence words, or `None` when the line is dropped.
///
/// One function because the reference has one formatter: `0x627930` serves both
/// `SMSG_SPELLLOGEXECUTE` effect 8 `POWER_DRAIN` and `SMSG_PERIODICAURALOG` aura 64, and the three
/// rules are its own — `drained = amount / div`, `gained = round(amount·multiplier) / div` (the
/// product is reduced to an integer *before* the divide, which is why this is arithmetic and not
/// two divides), and **`drained == 0` drops the line** (`627ae3 test esi,esi`).
///
/// **The two figures do not share their arithmetic**, which 2117 recorded as one `trunc`.
/// `drained` is a plain signed integer divide of the wire amount (`627ad9 idiv`), no FPU at all.
/// `gained` goes through the x87: the product is doubled, biased by ∓0.5 on its own sign,
/// `fistp`-ed under the default round-half-to-even, and arithmetic-shifted back down
/// (`627a8f`..`627ac9`) — a hand-rolled rounding that, for the non-negative products a leech
/// actually produces, lands on exactly `floor(p)`, i.e. the truncation the record claimed. So the
/// figures are unchanged and the *mechanism* named here is the byte's, not a paraphrase of it.
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

/// `627a8f`..`627ac9` — the product `amount · multiplier`, reduced to an integer the way the
/// reference reduces it, which is not `as i64` and not `__ftol`.
///
/// `fild` the amount, `fmul` the f32 multiplier, then **`fst dword [ebp-0x10]` rounds the product
/// to f32** (`0x627a95`) — that narrowing is real and is why this is not done in `f64`. The
/// reduction is then a hand-rolled round-half-away-from-zero: double it, bias by ∓0.5 on the
/// product's own sign (`0x627aa5`/`0x627ab5`, against `[0x8628f4] = 0.5`), `fistp` under the
/// default round-half-to-even, and `sar 1` back down (`0x627ac9`).
///
/// For the non-negative products a leech actually produces this lands on `floor(p)` — the
/// truncation 2117 recorded — so the figures are the same and only the edges differ. The edges
/// are the point: at f32 precision an `f64` product can round to the other side of an integer,
/// and the whole family exists so an addon reads the same number off our log as off the
/// reference's.
fn rounded_product(amount: u32, multiplier: f32) -> i32 {
    let product = (f64::from(amount) * f64::from(multiplier)) as f32; // the `fst` narrowing
    let biased = f64::from(product).mul_add(2.0, if product < 0.0 { 0.5 } else { -0.5 });
    // `fistp` truncates to i32 under the default control word: round half to even.
    (biased.round_ties_even() as i32) >> 1
}

/// The periodic handler's **pre-divide** — `0x627126`/`0x627138` scales the tick's amount by
/// `0x6e7130(powerType)` *before* handing it to `0x627930`, which then divides by the very same
/// table again.
///
/// It is a reference bug, and we reproduce it deliberately. The positive control is its own
/// sibling: the energize tick pre-divides identically at `0x627087` and `0x627520` contains no
/// divide at all, so the double is specific to aura 64. Reproduced because this family exists to
/// be *read* — MikScrollingBattleText parses these sentences, and a number that disagrees with the
/// reference's is the bug, whichever client is arithmetically right. It is the same posture
/// [`spell_log_miss`] already takes toward `0x5e7f31`'s chat type.
///
/// Mana is unaffected by construction (divisor 1, so the second divide is the identity), which is
/// every leech anyone has looked at; a periodic rage leech would come out at a hundredth. Whether
/// any 1.12 aura-64 row names a non-mana power is **not measured here**.
fn periodic_leech_amount(power: u32, amount: u32) -> u32 {
    (i64::from(amount) / power_divisor(power)) as u32
}

/// `0x627930` — **the** power leech/drain formatter, for both of its call sites.
///
/// One function because the reference has one function. `SMSG_PERIODICAURALOG` aura 64 enters it
/// through `0x627910` with the periodic flag **set**; `SMSG_SPELLLOGEXECUTE` effect 8
/// `POWER_DRAIN` enters it with the flag **clear**; everything between the two entries is shared.
/// Ours were two arms that had drifted apart — the execute-log one forked three ways and the
/// periodic one worded `SPELLPOWERLEECH` unconditionally, which 2117 named as residue and 2157
/// closes.
///
/// The arms, in the order the bytes test them (read off `0x627930` this session):
///
/// 1. **`powerType == 4` → `0x627de0`**, the *first* test in the function (`62793c cmp edi,4`).
///    It is a **fall-through, not a diversion**: `627953 test al,al` / `627955 jne` leaves only
///    when `0x627de0` actually emitted, so a happiness drain whose pet→owner chain does not
///    resolve carries on down the generic path and words `SPELLPOWERLEECH`/`…DRAIN` with the
///    `HAPPINESS_POINTS` noun.
/// 2. **`powerType >= 5` → no line**, and the gate is `0x6278f0`'s own `cmp ecx,5; jae` answering
///    NULL for the power noun (`627964 test edi,edi` / `627966 je`) — *not* the four `0x64a4c0`
///    name compares much further down, which run after the emit. The compare is **unsigned**, so
///    a negative tag drops here too. Ours is [`combat::power_word`] answering `None`.
/// 3. the spell gates — `SpellRec+0x18 & 0x180` and an empty localized name — which are the
///    caller's [`ChatCtx::spell_name`], already run before we get here.
/// 4. the shared resolve + range gate `0x626630`, which is [`queue`]'s.
/// 5. **the LEECH/DRAIN fork** on `|multiplier| >= 2^-22` (`6279ff fcomp [0x8029d4]`; the `jp`
///    takes greater-or-equal, so equality is a leech).
/// 6. **the periodic flag picks the chat-type stub** — `0x627d60` periodic, `0x627d30` direct —
///    and only here, *after* the fork.
/// 7. **`drained == 0` drops the line** (`627ae3 test esi,esi` / `627ae5 je`), after the divide.
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
    // Arm 1. The happiness sentence is tried first and only *consumes* the packet when it emits.
    if power == POWER_HAPPINESS && happiness_drain_line(log, ctx, stores, poses, target, amount) {
        return;
    }
    // Arm 2. `0x6278f0`'s bound, expressed where the noun is: no noun, no line.
    if !combat::power_has_word(power) {
        return;
    }
    // Arms 5 and 7's arithmetic.
    let Some((drained, gained)) = leech_figures(power, amount, multiplier) else {
        return;
    };
    let family = leech_family(multiplier);
    // Arm 6. Both stubs ask the SPELL, not the family: `0x627d30` is
    // `call 0x6ea280; cmp eax,2; je -> 0x626be0 (damage) else 0x627820 (buff)` and `0x627d60` is
    // its periodic twin over `0x627d80`/`0x6274a0`. So a
    // helpful drain files under `…_BUFF` and a harmful one under `…_DAMAGE`, which is a property
    // of the row in `Spell.dbc` and not something either call site may hardcode — both of ours
    // did, in opposite directions.
    let buff = !ctx.spell_harmful(spell_id);
    // The periodic stub hands its selector `outClassA`, the **caster** (`0x627a0f`/`0x627a3a`) —
    // the row [`periodic_subject`] records for this tick kind.
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

/// `0x627de0` — the happiness sentence, and the one arm of `0x627930` that is not about a power
/// noun at all: the subject is the pet's **owner**, the named thing is the **pet**, and the chat
/// type is the literal `0x19` misc-info rather than anything a selector picks.
///
/// **Answers whether it emitted**, because that is the byte's own contract: `0x627930` tests the
/// returned `al` and only stops when it is set, so a pet whose owner chain does not resolve is not
/// a dropped line — it is a line that comes out of the generic leech/drain path instead.
///
/// The divisor is unconditionally happiness's own 1000 (`627f45 mov ecx,4`), which is what
/// [`power_divisor`] answers for tag 4, so the figure is [`leech_figures`]'s `drained` by another
/// route.
fn happiness_drain_line(
    log: &mut ChatLog,
    ctx: &ChatCtx,
    stores: &Query<&mut ObjectStore>,
    poses: &Query<&mut Transform>,
    pet: (u64, UnitClass),
    amount: u32,
) -> bool {
    let Some(owner) = unit_owner(pet.0, ctx, stores) else {
        return false; // the `+0x110` pet→owner chain did not resolve — fall through, not drop
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

/// A unit's owner guid — `CHARMEDBY` first, then `CREATEDBY`, the same pair
/// [`combat::classify`] reads. `None` when the unit is not streamed or owns itself.
fn unit_owner(guid: u64, ctx: &ChatCtx, stores: &Query<&mut ObjectStore>) -> Option<u64> {
    let entity = ctx.index.0.get(&guid).copied()?;
    let store = stores.get(entity).ok()?;
    store
        .0
        .unit_charmed_by()
        .or_else(|| store.0.unit_created_by())
}

/// `SMSG_ENVIRONMENTALDAMAGELOG` → "You fall and lose %d health." and its five siblings.
///
/// **The only family with no selector at all.** `0x62aac0` builds the key by `snprintf` over a
/// 6-entry damage-type table and a plain SELF/OTHER, and calls the melee HITS msg-id selector with
/// **`ecx = edx = victimClass`** — so a fall on a party member types as `HOSTILEPLAYER_HITS`
/// through that selector's tgt<=3 override, which looks wrong and is what the reference does.
///
/// It grows trailers (absorb/resist/vulnerability), which is how a resisted fire tick reads.
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
                // The environmental call site passes zero for both — it has no wire source for a
                // block or a HitInfo, so GLANCING/CRUSHING/BLOCK can never appear on these lines.
                blocked: 0,
                hit_info: 0,
            }),
            ..Default::default()
        },
    );
}

/// `SMSG_SET_FACTION_STANDING` → "Your %s reputation has increased by %d."
///
/// **The wire carries the new TOTAL, and the sentence wants the DELTA** — so this runs before the
/// store is overwritten, and a slot whose value did not actually move prints nothing (the
/// reference's own `0x62c5f0` guard: "only when the stored value actually changed").
///
/// It is one of the twelve formatters with **no range gate and no classifier** — the chat type is
/// the literal `0x55` `COMBAT_FACTION_CHANGE`, and the only participant is you — so it builds its
/// line directly rather than going through [`queue`], which exists to gate on two endpoints.
///
/// The wire's `reputationListId` is `Faction.dbc`'s `rep_index`, not a faction id; the name comes
/// off the row that carries that index.
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
            // A `Single` family reads no variant, but the field is not optional; `OtherOther` is
            // the one `tests::variants_of` sweeps such a family with.
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

/// The one tail every arm ends in: build the queued line (dropping a class-9 endpoint) and park it
/// for its names.
// facts a queued line is made of. Bundling any of them would only move the list somewhere else.
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

/// [`queue`] for a family whose `Named` slot still has to be looked up — an item entry or a unit
/// guid rides along and the drain resolves it, holding the line until it lands (`0x6294b0`).
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

/// [`queue_named`], answering **whether a line was actually queued**.
///
/// One caller reads the verdict and has to: `0x627de0` reports its success to `0x627930` in `al`
/// (`mov al,1` at `0x627fb0`/`0x627fe2`/`0x627ff4` against `0x628006 xor al,al`), and a false
/// there is a **fall-through** to the generic leech/drain path, not a dropped line. Both failure
/// shapes below are among the ones the reference reports: its own range gate, and an empty
/// template.
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
    // **The gate is an OR over the two endpoints, not an AND** — `0x626630`'s own test:
    // `dist²(player, src) < range(srcClass)²` **OR** `dist²(player, tgt) < range(tgtClass)²`. 1571
    // required both and therefore dropped lines the client shows: your own pet (range 100000)
    // fighting something 80 yards away is logged by the reference and was silent here.
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

    /// **A periodic line is typed off the endpoint its own formatter reads**.
    ///
    /// The report: MikScrollingBattleText showed no DoT lines at all, in either direction. Every
    /// periodic tick took `periodic_kind(caster, …)`, so a creature's poison ticking you came out
    /// as `CHAT_MSG_SPELL_PERIODIC_CREATURE_DAMAGE` where the reference sends
    /// `…_PERIODIC_SELF_DAMAGE`, and your own DoT on a creature came out the other way round —
    /// the two swapped. MSBT matches a different GlobalString pattern per event name, so with
    /// both events swapped neither line matched anything and both were dropped.
    #[test]
    fn a_periodic_damage_or_heal_tick_is_typed_off_the_target() {
        let dmg = PeriodicTick::Damage {
            amount: 12,
            school: 3,
            absorb: 0,
            resist: 0,
        };
        let heal = PeriodicTick::Heal { amount: 12 };
        // A creature's DoT ticking me: the subject is ME, so the line types SELF.
        assert_eq!(
            periodic_subject(&dmg, UnitClass::Creature, UnitClass::Me),
            UnitClass::Me
        );
        // My DoT ticking a creature: the subject is the CREATURE.
        assert_eq!(
            periodic_subject(&dmg, UnitClass::Me, UnitClass::Creature),
            UnitClass::Creature
        );
        assert_eq!(
            periodic_subject(&heal, UnitClass::Creature, UnitClass::Me),
            UnitClass::Me
        );
        // The other two formatters read the CASTER, which is why this is a table and not a rule.
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

    /// The report this arithmetic was written for: MSBT read `+10 Rage` off our combat log where
    /// the reference reads `+1 Rage`, for the same swing against the same server.
    ///
    /// The wire number really is ten. `Unbridled Wrath Effect` (12964) is
    /// `effect1 = 30 SPELL_EFFECT_ENERGIZE`, `effectMiscValue1 = 1 POWER_RAGE`,
    /// `effectBasePoints1 = 9` — base points are *n − 1*, so vmangos energizes by **10** and
    /// `SendEnergizeSpellLog` ships that same 10 (`SpellCaster.cpp:796-813`, it logs exactly what
    /// it hands `ModifyPower`, and the stored rage field is the ×10 one). Rage is the only power a
    /// player can gain where raw and displayed differ, which is why it is the one that was seen.
    #[test]
    fn a_rage_gain_words_the_displayed_figure_not_the_wire_one() {
        assert_eq!(power_gain(1, 10), 1, "Unbridled Wrath: one point of rage");
        assert_eq!(power_gain(1, 100), 10, "Bloodrage: ten");
        // Every other power a POWERGAIN line can name is one-to-one, so the divide has to be a
        // table lookup and not an `if rage` — mana, focus and energy must come through untouched.
        assert_eq!(power_gain(0, 300), 300);
        assert_eq!(power_gain(2, 20), 20);
        assert_eq!(power_gain(3, 20), 20);
    }

    /// `0x627930`'s three rules, one function for both of its packets.
    #[test]
    fn a_leech_divides_both_figures_and_drops_a_zero_drained_line() {
        // Mana leeches one-to-one: 120 drained, half of it gained.
        assert_eq!(leech_figures(0, 120, 0.5), Some((120, 60)));
        // The truncation ORDER is observable, and only on a scaled power: the reference truncates
        // the product and divides after (`trunc(25 · 0.9) = 22`, `22 / 10 = 2`). Dividing first
        // would give `(25 / 10) · 0.9 = 1`, and nothing downstream would notice the off-by-one.
        assert_eq!(leech_figures(1, 25, 0.9), Some((2, 2)));
        // Below one displayed point there is no line at all — the reference's own `drained == 0`
        // test, which only a scaled power can reach with a nonzero wire amount.
        assert_eq!(leech_figures(1, 9, 1.0), None);
        assert_eq!(leech_figures(0, 0, 1.0), None);
        // Happiness is the ×1000 arm; a pet's 1500 raw is one displayed point.
        assert_eq!(leech_figures(4, 1500, 0.0), Some((1, 0)));
    }

    /// The LEECH/DRAIN fork the periodic arm did not have — one formatter, and until 2157 its two
    /// call sites disagreed about every choice it makes.
    ///
    /// The epsilon is the reference's own `[0x8029d4] = 2^-22`, and the compare's `jp` takes the
    /// greater-or-equal side, so a multiplier sitting exactly on it is a **leech**. That boundary
    /// is the whole reason this is a named function and not an `if multiplier != 0.0`.
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
        // `fabs` first: the sign is not the question the reference asks.
        assert_eq!(leech_family(-1.0).stem, "SPELLPOWERLEECH");
    }

    /// `0x6278f0`'s table is five entries, not four — **happiness has a noun**.
    ///
    /// `cmp ecx,5; jae` is the bound, and it is unsigned. We stopped the table at energy and
    /// called happiness "no GlobalString", which silently dropped every line the reference words
    /// with `HAPPINESS_POINTS = "Happiness"` (shipped enUS `GlobalStrings.lua:2117`): the generic
    /// leech/drain fall-through out of `0x627de0`, and a happiness `POWERGAIN` tick. What
    /// happiness actually lacks is a `COMBAT_TEXT_UPDATE` tag, which is a different table.
    #[test]
    fn every_wire_power_tag_has_a_noun_and_nothing_past_the_table_does() {
        for power in 0..=4 {
            assert!(combat::power_has_word(power), "power {power} has a noun");
        }
        for power in [5, 6, 99, u32::MAX] {
            assert!(!combat::power_has_word(power), "power {power} has none");
        }
    }

    /// The gained figure is reduced to an integer the way `0x627930` reduces it — through an
    /// **f32** product, not an `f64` one.
    ///
    /// The first two cases are the arithmetic anyone would write anyway. The third is the
    /// mechanism: `fst dword [ebp-0x10]` at `0x627a95` narrows the product to f32 before the
    /// rounding, and 2^24+1 is the smallest amount where that narrowing is visible — the f64
    /// product is exact and the f32 one rounds to even, one below. No leech ships an amount that
    /// large; the case is here because it is the only thing that can tell the two implementations
    /// apart, and an `as i64` over f64 silently passes everything else.
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

    /// **The periodic leg divides twice, and we reproduce it.** `0x627126`/`0x627138` scales the
    /// tick's amount by `0x6e7130(powerType)` before `0x627930` divides by the same table again.
    ///
    /// Mana cannot show it — divisor 1, so the second divide is the identity, and that is every
    /// leech anyone has looked at. Rage can: a wire 100 is 10 through the pre-divide and 1 by the
    /// time the sentence is written.
    #[test]
    fn a_periodic_leech_is_pre_divided_before_the_formatter_divides_again() {
        assert_eq!(periodic_leech_amount(0, 400), 400, "mana: the identity");
        assert_eq!(periodic_leech_amount(1, 100), 10, "rage: the first divide");
        // End to end on the rage case: 100 on the wire words as 1, not 10 and not 100.
        assert_eq!(
            leech_figures(1, periodic_leech_amount(1, 100), 1.0),
            Some((1, 1))
        );
        // The direct packet takes the same wire number through one divide only.
        assert_eq!(leech_figures(1, 100, 1.0), Some((10, 10)));
    }
}
