//! The spell tooltip views: the builder `0x52e610`'s cells resolved here where the catalogs live,
//! built at the setter's call, as the reference builds at the hover and keeps nothing
//! ([`super::live`]).

use benilla_protocol::ObjectFields;
use benilla_ui::script::{AuraKey, AuraTooltipView, KeyedArg, TextPart, TooltipText};

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects};
use crate::query_cache::Ask;
use crate::spell::usable;
use crate::spell::SpellLevel;
use crate::ui_action::Spells;
use crate::ui_items::{find_item, ItemSearch};

/// The unit a view's caster cells are computed against, the spell builder `0x52e610`'s fourth
/// argument: the level the `$`-tokens, the cost and the cast time scale by, and the cost's basis.
#[derive(Clone, Copy)]
pub(super) enum ViewCaster<'a> {
    /// The active player ([`ViewCtx::store`]), selector 0.
    Player,
    /// Selector 1: the player's charm, else its summon (`0x6e3159`-`0x6e317f`, `0x6e31fe`-
    /// `0x6e3228`), `None` when neither is streamed.
    Pet(Option<&'a benilla_protocol::ObjectFields>),
}

/// The caster-dependent inputs of a spell view: the worn set (`0x5f0c50`), the bags, the form and
/// the bind point `$z` names.
pub(super) struct ViewCtx<'a, 'w, 's> {
    pub(super) home_area: Option<&'a str>,
    /// The caster's shapeshift form, which the required-form line's colour reads (`0x52f1f2`).
    pub(super) form: u8,
    /// The active player.
    pub(super) store: Option<&'a ObjectStore>,
    /// Whom the level terms and the cost read: the player or its pet. The form and the reaches
    /// are that same unit's.
    pub(super) caster: ViewCaster<'a>,
    /// The caster as `0x6e3480` reads it: its `UNIT_FIELD_COMBATREACH` (1.5 is the descriptor
    /// default) and its motion.
    pub(super) range_caster: benilla_formats::RangeUnit,
    /// The caster's auto-attack target, which `0x6e3480` looks up itself (`[caster+0xc48]`) for
    /// its melee arm alone, reading its reach and motion; with none, the caster's reach counts
    /// twice and there is no moving bonus.
    pub(super) attack_target: Option<benilla_formats::RangeUnit>,
    /// The object index the worn-item search and each reagent's carried count resolve through.
    pub(super) objects: &'a Objects<'w, 's>,
    pub(super) items: &'a Items,
    pub(super) commands: &'a NetCommands,
    pub(super) sub_classes: Option<&'a benilla_formats::ItemSubClassCatalog>,
    /// The spell-to-line hop the `$`-tokens' skill level reads ([`crate::spell::spell_skill_value`]).
    pub(super) skill_lines: Option<&'a benilla_formats::SkillLineCatalog>,
    /// The talent spell-modifier tables: the cost cell shows the modified cost (`power_cost`).
    pub(super) spell_mods: &'a crate::spell::SpellModifiers,
    /// `Faction.dbc`, which names the reputation line's faction and indexes its standing.
    pub(super) factions: Option<&'a benilla_formats::FactionCatalog>,
    /// The player's standings, the reputation line's test.
    pub(super) reputations: Option<&'a crate::net::Reputations>,
}

impl ViewCtx<'_, '_, '_> {
    /// What the player's level for a spell reads: its skill lines, and for a weapon spell the
    /// records and the hands' weapons ([`crate::spell::player_spell_level`]).
    fn player_skills<'s>(&'s self, spells: &'s Spells) -> crate::spell::PlayerSkills<'s> {
        crate::spell::PlayerSkills {
            lines: self.skill_lines,
            spells: Some(&spells.catalog),
            items: Some(self.items),
        }
    }
}

/// Build one spell's tooltip view, its data resolved here where the catalogs live. Every
/// `GlobalStrings.lua` template a cell or a `$` token names is left by key to the render, which
/// reads `_G` as the builder does at each call (`0x703bf0`), so no string a view holds goes stale.
pub(super) fn build_view(
    spell_id: u32,
    spells: &Spells,
    vctx: &ViewCtx,
) -> Option<benilla_ui::script::SpellTooltipView> {
    let d = spells.catalog.get(spell_id)?;
    let form = vctx.form;
    let caster = vctx.caster;
    // `0x6e3130` for this spell: the level the cost's, the per-second cost's and the cast time's
    // per-level terms read, the player's ([`crate::spell::player_spell_level`]) or the pet's.
    let pet_level = |pet| SpellLevel::Known(d.skill_level(crate::spell::pet_skill_value(pet)));
    let skills = vctx.player_skills(spells);
    let player_level = SpellLevel::Player {
        me: vctx.store,
        skills,
    };
    // The cost cell (`0x52e8ad`): the resolved `power_cost`, named by power type (keys at
    // `0x85416c`) and Health for a type outside 0..=4 (`0x52e8fc`: Bloodrage's -2), never a
    // percentage. Rage shows wire ÷ 10, a per-second cost adds `_PER_TIME`, and a cell whose two
    // quotients are both 0 or less is empty.
    let resolved_cost = match caster {
        ViewCaster::Player => vctx.store.map_or(d.mana_cost as i32, |s| {
            usable::power_cost(d, s, skills, vctx.spell_mods)
        }),
        // Deviation: a pet view with no pet unit costs 0, an empty cell, where `0x6e3233`
        // returns -1 and the unsigned divide (`0x52e8dd`) prints it as "429496729 Rage", or
        // "4294967 Happiness" (divisors 10 and 1000 at `0x86f978`); benilla shows no such garbage
        // cost. For mana, focus and energy, divisor 1, -1 is an empty cell, as 0 is.
        ViewCaster::Pet(pet) => pet.map_or(0, |p| {
            usable::power_cost_at(d, p, pet_level(Some(p)), vctx.spell_mods)
        }),
    };
    // The per-second term (`0x6e3300`, `0x52e8c5`), at the same selector's level.
    let per_second = match caster {
        ViewCaster::Player => usable::per_second_cost(d, player_level),
        ViewCaster::Pet(pet) => usable::per_second_cost(d, pet_level(pet)),
    };
    // Both divided by the shared per-type divisor (`0x6e7130`), never a local `power_type == 1`
    // test, unsigned (`0x52e8dd`, `0x52e8f2`); the gates test the quotients, signed.
    let div = benilla_protocol::messages::power_display_scale(d.power_type);
    let cost_shown = ((resolved_cost as u32) / div) as i32;
    let per_second_shown = ((per_second as u32) / div) as i32;
    let cost = {
        // The key table `0x85416c` for types 0 to 4, `HEALTH_COST` (`0x854fc8`) past them.
        let key = match d.power_type {
            0 => "MANA_COST",
            1 => "RAGE_COST",
            2 => "FOCUS_COST",
            3 => "ENERGY_COST",
            4 => "HAPPINESS_COST",
            _ => "HEALTH_COST",
        };
        let amount = KeyedArg::D(i64::from(cost_shown));
        if per_second_shown > 0 {
            // `"%s_PER_TIME"` of the key (`0x854fbc`, `0x52e925`), whatever the cost.
            Some(TooltipText::keyed(
                format!("{key}_PER_TIME"),
                vec![amount, KeyedArg::D(i64::from(per_second_shown))],
            ))
        } else if cost_shown > 0 {
            Some(TooltipText::keyed(key, vec![amount]))
        } else {
            // Neither quotient above 0, so a rage cost of 1 to 9 has no cell (`0x52e913`-`0x52e919`).
            None
        }
    };
    // The range cell: two attribute gates, `GetMinMaxRange` (`0x6e3480`, `0x52e9c2`), then
    // `max <= 0`. There is no melee wording: the melee row (flags bit 0, row 2 only) resolves to
    // `max(reach + casterReach + 1.3333334, 5.0)` and prints like any other, usually "5 yd range".
    let range = (!d.tooltip_omits_range_line())
        .then(|| {
            let row = spells.ranges.get(d.range_index);
            // The call passes a null target (`0x52e9c2`): the auto-attack target is the melee arm's
            // own lookup, and the ranged arm, with no target, pads nothing.
            let targets = benilla_formats::RangeTargets {
                target: None,
                attack_target: vctx.attack_target,
            };
            let (min, max) = vctx
                .spell_mods
                .min_max_range(d, row, vctx.range_caster, targets)?;
            if max <= 0.0 {
                return None;
            }
            // `SPELL_RANGE`'s hole is a string: the number is composed first, `"%d"` or `"%d-%d"`
            // (`0x854fb4`, Charge's 8-25), each a `fistp` conversion that rounds to nearest.
            let yd = |v: f32| f64::from(v).round_ties_even() as i64;
            let yards = if min > 0.0 {
                format!("{}-{}", yd(min), yd(max))
            } else {
                format!("{}", yd(max))
            };
            Some(TooltipText::keyed("SPELL_RANGE", vec![KeyedArg::S(yards)]))
        })
        .flatten();
    // The cast line: gate `0x52eb15` also omits it for a TRADE_SKILL or ATTACK `Effect[0]`; then
    // the ladder `0x52eb45`-`0x52ec90` over `GetCastTime(1)` (`52eb4d`), unclamped, where a
    // negative time is "Instant cast" (`0x52ebce`) and a zero one is "Instant cast" only for a
    // mana spell with a cost (`0x52ec4b`).
    let cast_time = if d.tooltip_omits_cast_line() {
        None
    } else {
        // `GetCastTime` takes the selector too (`52eb45`): it picks the unit whose cast speed is
        // read and the level the row's per-level term reads (`0x6e3130`).
        let base = match caster {
            ViewCaster::Player => {
                let player = vctx.store.map(|s| &s.0);
                spells.cast_time_unclamped_ms(d, player, player_level, vctx.spell_mods)
            }
            ViewCaster::Pet(pet) => {
                let level = pet_level(pet);
                spells.cast_time_unclamped_ms(d, pet, level, vctx.spell_mods)
            }
        };
        // `%.3g` templates: the seconds go over as a real.
        if base > 0 {
            let (key, v) = if base >= 60_000 {
                ("SPELL_CAST_TIME_MIN", f64::from(base) / 60_000.0)
            } else {
                ("SPELL_CAST_TIME_SEC", f64::from(base) / 1000.0)
            };
            Some(TooltipText::keyed(key, vec![KeyedArg::F(v)]))
        } else {
            let key = if base < 0 {
                "SPELL_CAST_TIME_INSTANT"
            } else if d.on_next_swing() {
                "SPELL_ON_NEXT_SWING"
            } else if d.tooltip_on_next_ranged() {
                "SPELL_ON_NEXT_RANGED"
            } else if d.tooltip_channeled() {
                "SPELL_CAST_CHANNELED"
            } else if d.power_type == 0 && cost_shown > 0 {
                "SPELL_CAST_TIME_INSTANT"
            } else {
                "SPELL_CAST_TIME_INSTANT_NO_MANA"
            };
            Some(TooltipText::keyed(key, Vec::new()))
        }
    };
    // The larger recovery column (`0x52eada`): op 11 follows the ranged-speed category pad. The
    // ranged attack time is the player's, a pet view's too.
    let ranged_ms = if d.ranged_speed_cooldown() {
        vctx.store
            .and_then(|s| s.0.unit_ranged_attack_time())
            .unwrap_or(0)
    } else {
        0
    };
    let (own_recovery, category_recovery) = vctx.spell_mods.spell_cooldowns(d, ranged_ms);
    let recovery_ms = own_recovery.max(category_recovery);
    let cooldown = (recovery_ms > 0)
        .then(|| {
            let secs = f64::from(recovery_ms) / 1000.0;
            let (key, v) = if secs >= 60.0 {
                ("SPELL_RECAST_TIME_MIN", secs / 60.0)
            } else {
                ("SPELL_RECAST_TIME_SEC", secs)
            };
            Some(TooltipText::keyed(key, vec![KeyedArg::F(v)]))
        })
        .flatten();
    // The required-form line (`0x52f10a`-`0x52f2ae`). With `AttributesEx2` bit 19 set the mask is
    // permissive (Inner Fire in Shadowform), and the line is skipped (`0x52f115`).
    let requires_form = (d.stances != 0 && !d.form_mask_is_permissive())
        .then(|| {
            let names: Vec<&str> = (0..32u32)
                .filter(|b| d.stances & (1 << b) != 0)
                .filter_map(|b| spells.forms.get(&(b + 1)).map(|f| f.name.as_str()))
                .filter(|n| !n.is_empty())
                .collect();
            (!names.is_empty()).then(|| names.join(", "))
        })
        .flatten();
    // No caster unit passes the form test (`0x52f1e8`); a spell with no stance mask fails it for
    // every form.
    let form_met = matches!(caster, ViewCaster::Pet(None))
        || (form != 0 && d.stances & (1u32 << (u32::from(form) - 1)) != 0);
    // The required-item line (`0x52eea7`-`0x52f10a`): a mask is named whole by
    // `ItemSubClassMask.dbc` before its subclasses are joined; an empty mask or class < 0 skips it.
    let requires_item = (d.targets & TARGET_ITEM == 0 && d.equipped_item_class >= 0)
        .then(|| {
            vctx.sub_classes?
                .requirement_name(d.equipped_item_class as u32, d.equipped_item_subclass_mask)
        })
        .flatten();
    let requires_reputation = reputation_requirement(d, vctx);
    let chance = chance_line(d, vctx.store);
    // The unit selector passes the item test outright (`0x52f07f`).
    let item_met = matches!(caster, ViewCaster::Pet(_))
        || vctx
            .store
            .is_none_or(|s| usable::equipped_item_fits(d, s, vctx.objects, vctx.items));
    // The tools (`SPELL_TOTEMS`, `0x854ec4`) and the reagents (`SPELL_REAGENTS`, `0x854e54`),
    // each the player does not carry red. One whose template has not streamed is absent, its
    // lookup asked with the tooltip as the answer's cookie (`0x52eda4`, `0x52f3f4`) and counted
    // among the view's misses, whose last answer builds the tooltip again (`0x52f820`).
    let mut item_misses: Vec<u32> = Vec::new();
    let mut item_line = |head: &str, items: &mut dyn Iterator<Item = (u32, u32)>| {
        let mut parts: Vec<String> = Vec::new();
        let mut met = true;
        for (entry, count) in items.filter(|&(e, _)| e != 0) {
            let Some(name) = vctx
                .items
                .template(entry, 0, Ask::Event(vctx.commands))
                .map(|t| t.name.clone())
            else {
                item_misses.push(entry);
                continue;
            };
            // `"%s (%d)"` above one (`0x854b30`, `0x52f461`); a tool has no count.
            let text = if count > 1 {
                format!("{name} ({count})")
            } else {
                name
            };
            // Red only when no copy is found at all (`0x622270` at mask 0, `0x52ee12`,
            // `0x52f49f`); the count in the line is shown, never compared.
            let short = vctx.store.is_some_and(|s| {
                find_item(&s.0, vctx.objects, entry, ItemSearch::default()).is_none()
            });
            met &= !short;
            parts.push(if short {
                format!("|cffff2020{text}|r")
            } else {
                text
            });
        }
        // The head copied, then the names joined by `", "` (`0x52edc6`-`0x52edfc`,
        // `0x52f416`-`0x52f44c`).
        let line = (!parts.is_empty()).then(|| TooltipText::copied(head).then(parts.join(", ")));
        (line, met)
    };
    let (totems, totems_met) = item_line(
        "SPELL_TOTEMS",
        &mut d.totems.iter().map(|&entry| (entry, 1)),
    );
    let (reagents, reagents_met) = item_line("SPELL_REAGENTS", &mut d.reagents.iter().copied());
    // The unit selector skips both lookups (`0x52ed50`, `0x52f398`), so a pet view misses none.
    if matches!(caster, ViewCaster::Pet(_)) {
        item_misses.clear();
    }
    // The drain-all line (`0x52f697`): `AttributesEx` bit `0x2`, keyed by power type.
    let use_all = (d.attributes_ex & ATTR_EX_USE_ALL_POWER != 0).then(|| {
        TooltipText::copied(match d.power_type {
            0 => "SPELL_USE_ALL_MANA",
            1 => "SPELL_USE_ALL_RAGE",
            2 => "SPELL_USE_ALL_FOCUS",
            3 => "SPELL_USE_ALL_ENERGY",
            4 => "SPELL_USE_ALL_HAPPINESS",
            _ => "SPELL_USE_ALL_HEALTH",
        })
    });
    let description = d
        .description
        .as_deref()
        .map(|t| expand(t, d, spells, vctx, Default::default(), false))
        .unwrap_or_default();
    Some(benilla_ui::script::SpellTooltipView {
        name: d.name.clone(),
        rank: d.rank.clone(),
        cost,
        range,
        cast_time,
        cooldown,
        recovery_ms,
        requires_item,
        item_met,
        requires_form,
        form_met,
        requires_reputation,
        chance,
        totems,
        totems_met,
        reagents,
        reagents_met,
        use_all,
        description,
        item_misses,
    })
}

/// Build one aura's tooltip view, the aura builder `0x52f880`'s lines for `key`. The text is
/// `AuraDescription` alone, empty with none (`0x52f929`-`0x52f938`), its tokens at the key's level
/// and, for a spell whose `StackAmount` is above 1, at its stack, else at 1 (`0x52f8ca`-
/// `0x52f8dc`).
pub(super) fn build_aura_view(
    key: AuraKey,
    spells: &Spells,
    vctx: &ViewCtx,
) -> Option<AuraTooltipView> {
    let d = spells.catalog.get(key.spell_id)?;
    let at = benilla_formats::ExpandAt {
        level: u32::from(key.level),
        stack: if d.stack_amount > 1 {
            u32::from(key.stack)
        } else {
            1
        },
    };
    let description = d
        .aura_description
        .as_deref()
        .map(|t| expand(t, d, spells, vctx, at, true))
        .unwrap_or_default();
    Some(AuraTooltipView {
        name: d.name.clone(),
        // The right column (`0x52f8e5`), gated on `SpellDispelType.dbc` `[+0x28]`.
        dispel_type: spells.catalog.dispel_label(d).map(str::to_string),
        description,
    })
}

/// The reputation line's facts (`0x52f2ae`-`0x52f38e`), read off the active player, the builder's
/// `[ebp-0x18]`, whatever the caster: its standing with `MinFactionId` (`0x4d6370`) against the
/// floor of `MinReputation` in `0x80928c`, and its gender for the label (`0x612bf0`), read as the
/// reputation chat reads it (`net::session`'s `self_facts`, `UNIT_FIELD_BYTES_0` byte 2), where
/// `0x5ed5b0` reads `PLAYER_BYTES_3` byte 0. No line without a faction id.
fn reputation_requirement(
    d: &benilla_formats::SpellDisplay,
    vctx: &ViewCtx,
) -> Option<benilla_ui::script::ReputationRequirement> {
    if d.min_faction_id == 0 {
        return None;
    }
    let faction = vctx
        .factions
        .and_then(|c| c.faction_name(d.min_faction_id))
        .unwrap_or("UNKNOWN")
        .to_string();
    let player = vctx.store.map(|s| &s.0);
    let standing = match (player, vctx.factions, vctx.reputations) {
        (Some(p), Some(c), Some(r)) => {
            let (race, class) = (p.unit_race().unwrap_or(0), p.unit_class().unwrap_or(0));
            crate::ui_reputation::faction_standing(c, r, race, class, d.min_faction_id)
        }
        _ => 0,
    };
    // The rank's floor in the bounds at `0x80928c`. The one row with a faction asks rank 4; past
    // the table's ninth word the client reads on (`0x8092b0`), and this holds it at the last.
    let floor = crate::ui_reputation::RANK_BOUNDS[d.min_reputation.min(8) as usize];
    Some(benilla_ui::script::ReputationRequirement {
        faction,
        rank: d.min_reputation,
        met: standing >= floor,
        female: player.and_then(|p| p.unit_gender()) == Some(1),
    })
}

/// `text`'s `$`-tokens expanded for `d` against `vctx`'s caster as `0x5075f0` does for a caller
/// passing `at`. `unmodified_points` is the aura builder's call, the points' modifiers off
/// (`0x52f946`).
fn expand(
    text: &str,
    d: &benilla_formats::SpellDisplay,
    spells: &Spells,
    vctx: &ViewCtx,
    at: benilla_formats::ExpandAt,
    unmodified_points: bool,
) -> TooltipText {
    let caster = vctx.caster;
    let skills = vctx.player_skills(spells);
    // The `$`-tokens' level (`0x5075f0` hands the selector to `0x6e3130`, `507645`-`50764a`).
    let level = |t: &benilla_formats::SpellDisplay| match caster {
        ViewCaster::Player => crate::spell::player_spell_level(vctx.store, skills, t),
        ViewCaster::Pet(pet) => t.skill_level(crate::spell::pet_skill_value(pet)),
    };
    // A cross-referenced spell's own family mask decides its modifiers.
    let lookup = |id| spells.catalog.get(id);
    let home = vctx.home_area;
    let home_area = || home;
    // `$g`/`$G` read the active player's gender byte (`0x508214`), a pet view's too.
    let gender = || vctx.store.and_then(|s| s.0.unit_gender()).unwrap_or(0);
    // `$c` and `$p` ask the caster the selector names (`0x507de0`, `0x507def`): the player's or
    // the pet's level and fields; no caster costs −1 (`0x6e3233`) and has level 0 (`0x6e31a4`).
    let pet_level = |t: &benilla_formats::SpellDisplay, pet| {
        SpellLevel::Known(t.skill_level(crate::spell::pet_skill_value(pet)))
    };
    let power_cost = |t: &benilla_formats::SpellDisplay| match caster {
        ViewCaster::Player => vctx
            .store
            .map_or(-1, |s| usable::power_cost(t, s, skills, vctx.spell_mods)),
        ViewCaster::Pet(pet) => pet.map_or(-1, |p| {
            usable::power_cost_at(t, p, pet_level(t, Some(p)), vctx.spell_mods)
        }),
    };
    let power_per_second = |t: &benilla_formats::SpellDisplay| match caster {
        ViewCaster::Player => {
            let level = SpellLevel::Player {
                me: vctx.store,
                skills,
            };
            usable::per_second_cost(t, level)
        }
        ViewCaster::Pet(pet) => usable::per_second_cost(t, pet_level(t, pet)),
    };
    let ctx = benilla_formats::TokenContext {
        durations: &spells.durations,
        radii: &spells.radii,
        ranges: Some(&spells.ranges),
        level: &level,
        lookup: &lookup,
        mods: Some(vctx.spell_mods),
        power_cost: &power_cost,
        power_per_second: &power_per_second,
        unmodified_points,
        gender: &gender,
        home_area: &home_area,
        // Never read: the parts leave each template to the render.
        global: &|_| None,
        printf: &crate::ui_script::token_printf,
    };
    tooltip_text(benilla_formats::substitute_parts(text, d, &ctx, at))
}

/// The token engine's parts as the render reads them: each template it names by key, its stand-in
/// the token as written.
pub(crate) fn tooltip_text(parts: Vec<benilla_formats::TokenPart>) -> TooltipText {
    use benilla_formats::{TokenNumber, TokenPart};
    TooltipText(
        parts
            .into_iter()
            .map(|p| match p {
                TokenPart::Text(t) => TextPart::Text(t),
                TokenPart::Keyed {
                    key,
                    ordinal,
                    args,
                    raw,
                } => TextPart::Keyed {
                    key,
                    // A negative ordinal reads as none, the bare key (`0x703c89`).
                    ordinal: ordinal.and_then(|n| u32::try_from(n).ok()),
                    args: Some(
                        args.into_iter()
                            .map(|a| match a {
                                TokenNumber::Int(n) => KeyedArg::D(n),
                                TokenNumber::Float(x) => KeyedArg::F(x),
                            })
                            .collect(),
                    ),
                    or: raw,
                },
            })
            .collect(),
    )
}

/// `AttributesEx` bit `0x2`, vmangos `SPELL_ATTR_EX_USE_ALL_MANA` (`SpellDefines.h:833`): the
/// drain-all line (`0x52f697`).
const ATTR_EX_USE_ALL_POWER: u32 = 0x2;

/// `Targets` bit `0x10` (name inferred): set, the required-item line is skipped (`0x52eeaa`).
const TARGET_ITEM: u32 = 0x10;

/// The four `Effect[0]` values that select a chance-to-X line (jump table `0x52f7c4`).
const EFFECT_DODGE: u32 = 20;
const EFFECT_PARRY: u32 = 22;
const EFFECT_BLOCK: u32 = 23;
const EFFECT_ATTACK: u32 = 78;

/// The chance-to-X line (`0x52f5b1`-`0x52f697`): ATTACK skips the passive gate that dodge, parry
/// and block need. The wire's values are already percents.
fn chance_line(
    d: &benilla_formats::SpellDisplay,
    store: Option<&ObjectStore>,
) -> Option<TooltipText> {
    let (which, key) = match d.effects[0] {
        EFFECT_ATTACK => (Chance::Crit, "CHANCE_TO_CRIT"),
        EFFECT_DODGE if d.passive => (Chance::Dodge, "CHANCE_TO_DODGE"),
        EFFECT_PARRY if d.passive => (Chance::Parry, "CHANCE_TO_PARRY"),
        EFFECT_BLOCK if d.passive => (Chance::Block, "CHANCE_TO_BLOCK"),
        _ => return None,
    };
    // The player block's float widened to a double (`0x52f652`) into the key's `%.2f%%`.
    let percent = which.read(&store?.0)?;
    Some(TooltipText::keyed(
        key,
        vec![KeyedArg::F(f64::from(percent))],
    ))
}

/// The four percentages a chance-to-X line reads (`0x52f5b1`).
#[derive(Clone, Copy)]
enum Chance {
    Block,
    Dodge,
    Parry,
    Crit,
}

impl Chance {
    /// The player's live percentage, already a percent on the wire.
    fn read(self, player: &ObjectFields) -> Option<f32> {
        match self {
            Chance::Block => player.player_block_percentage(),
            Chance::Dodge => player.player_dodge_percentage(),
            Chance::Parry => player.player_parry_percentage(),
            Chance::Crit => player.player_crit_percentage(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::{
        FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD,
    };

    /// The reagent line's carried test is `0x622270` at mask 0 (`0x52f49f`): a copy in the keyring
    /// or worn in a slot is carried, so its line is not red.
    #[test]
    fn a_reagent_line_carries_the_keyring_and_worn_gear() {
        let mut deps = crate::items::TestDeps::new();
        deps.spawn_item(
            0xA1,
            ObjectFields::from_pairs(&[(FIELD_OBJECT_ENTRY, 17056), (FIELD_ITEM_STACK_COUNT, 3)]),
        );
        let seat = |slot: u16| {
            let lo = FIELD_PLAYER_INV_SLOT_HEAD + 2 * slot;
            [(lo, 0xA1), (lo + 1, 0)]
        };
        for slot in [81, 17] {
            let store = ObjectStore(ObjectFields::from_pairs(&seat(slot)));
            let mut state = bevy::ecs::system::SystemState::<Objects>::new(&mut deps.world);
            let objects = state.get(&deps.world);
            let found = find_item(&store.0, &objects, 17056, ItemSearch::default());
            assert!(found.is_some(), "player slot {slot}");
        }
    }
}
