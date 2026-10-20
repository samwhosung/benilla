//! The spell tooltip views: the builder `0x52e610`'s cells resolved here where the catalogs live,
//! and the feed that pushes a view for every spell the UI can hover before it is hovered. The
//! reference builds at the hover and keeps nothing; the feed keeps what each pushed view read
//! ([`super::spell_deps`]) and builds again only the views a change of the player's state reaches.

use std::cell::RefCell;
use std::collections::HashMap;

use bevy::prelude::*;

use benilla_ui::script::{AuraKey, AuraTooltipView, KeyedArg, TextPart, TooltipText, UiScript};

use super::spell_deps::{Chance, Changes, Deps, RangeSeen, Reagents, Seen};
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::query_cache::Ask;
use crate::spell::usable;
use crate::spell::{SpellLevel, SpellReads, UnitField, Unrecorded};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_items::{count_of, find_item, InventoryScope, ItemSearch};
use crate::ui_trainer::TrainerTooltipSubjects;

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
    pub(super) items: &'a mut Items,
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

/// Build one spell's tooltip view, its data resolved here where the catalogs live, and what it
/// read ([`Deps`]). Every `GlobalStrings.lua` template a cell or a `$` token names is left by key
/// to the render, which reads `_G` as the builder does at each call (`0x703bf0`), so no string a
/// view holds goes stale.
pub(super) fn build_view(
    spell_id: u32,
    spells: &Spells,
    vctx: &mut ViewCtx,
) -> Option<(benilla_ui::script::SpellTooltipView, Deps)> {
    let d = spells.catalog.get(spell_id)?;
    let deps = RefCell::new(Deps::default());
    // The cost, cast, cooldown and range cells and the description's tokens all modify through
    // the spell's own family mask.
    deps.borrow_mut().mod_bits |= vctx.spell_mods.read_bits(d);
    let form = vctx.form;
    let caster = vctx.caster;
    // `0x6e3130` for this spell: the level the cost's, the per-second cost's and the cast time's
    // per-level terms read, the player's skill in the spell's line or the pet's level.
    let pet_level = |pet| SpellLevel::Known(d.skill_level(crate::spell::pet_skill_value(pet)));
    let player_level = SpellLevel::Player {
        me: vctx.store,
        skill_lines: vctx.skill_lines,
    };
    // The cost cell (`0x52e8ad`): the resolved `power_cost`, named by power type (keys at
    // `0x85416c`) and Health for a type outside 0..=4 (`0x52e8fc`: Bloodrage's -2), never a
    // percentage. Rage shows wire ÷ 10, a per-second cost adds `_PER_TIME`, and a cell whose two
    // quotients are both 0 or less is empty.
    // The player's reads are the view's record; the pet's level, basis and cast speed are the
    // pet's own inputs ([`PetInputs`]), which rebuild every pet view.
    let resolved_cost = match caster {
        ViewCaster::Player => vctx.store.map_or(d.mana_cost as i32, |s| {
            let reads: &mut dyn SpellReads = &mut *deps.borrow_mut();
            usable::power_cost(d, s, vctx.skill_lines, vctx.spell_mods, reads)
        }),
        // Deviation: a pet view with no pet unit costs 0, an empty cell, where `0x6e3233`
        // returns -1 and the unsigned divide (`0x52e8dd`) prints it as "429496729 Rage", or
        // "4294967 Happiness" (divisors 10 and 1000 at `0x86f978`); benilla shows no such garbage
        // cost. For mana, focus and energy, divisor 1, -1 is an empty cell, as 0 is.
        ViewCaster::Pet(pet) => pet.map_or(0, |p| {
            usable::power_cost_at(d, p, pet_level(Some(p)), vctx.spell_mods, &mut Unrecorded)
        }),
    };
    // The per-second term (`0x6e3300`, `0x52e8c5`), at the same selector's level.
    let per_second = match caster {
        ViewCaster::Player => usable::per_second_cost(d, player_level, &mut *deps.borrow_mut()),
        ViewCaster::Pet(pet) => usable::per_second_cost(d, pet_level(pet), &mut Unrecorded),
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
            // The pet views read the pet's reaches, which are among the pet's own inputs.
            if matches!(caster, ViewCaster::Player)
                && benilla_formats::min_max_range_reads_units(d, row)
            {
                deps.borrow_mut().range_units = true;
            }
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
                let reads: &mut dyn SpellReads = &mut *deps.borrow_mut();
                spells.cast_time_unclamped_ms(d, player, player_level, vctx.spell_mods, reads)
            }
            ViewCaster::Pet(pet) => {
                let level = pet_level(pet);
                spells.cast_time_unclamped_ms(d, pet, level, vctx.spell_mods, &mut Unrecorded)
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
        deps.borrow_mut().unit(UnitField::RangedTime);
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
    // No caster unit passes the form test (`0x52f1e8`). A spell with no stance mask fails it for
    // every form, so only a stance mask reads the player's form; a pet's is its own input.
    if matches!(caster, ViewCaster::Player) && d.stances != 0 {
        deps.borrow_mut().form = true;
    }
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
    let requires_reputation = reputation_requirement(d, vctx, &mut deps.borrow_mut());
    let chance = chance_line(d, vctx.store, &mut deps.borrow_mut());
    // The unit selector passes the item test outright (`0x52f07f`).
    let item_met = matches!(caster, ViewCaster::Pet(_))
        || vctx.store.is_none_or(|s| {
            let reads: &mut dyn SpellReads = &mut *deps.borrow_mut();
            usable::equipped_item_fits(d, s, vctx.objects, vctx.items, reads)
        });
    // The tools (`SPELL_TOTEMS`, `0x854ec4`) and the reagents (`SPELL_REAGENTS`, `0x854e54`),
    // each the player does not carry red. One whose template has not streamed is absent until
    // `feed_spell_tooltips` re-pushes on its arrival, as the reference looks each up with a
    // callback (`0x52eda4`, `0x52f3f4`) and builds again when the last answers.
    let item_line = |head: &str, items: &mut dyn Iterator<Item = (u32, u32)>| {
        let mut parts: Vec<String> = Vec::new();
        let mut met = true;
        for (entry, count) in items.filter(|&(e, _)| e != 0) {
            deps.borrow_mut().reagent(entry);
            let Some(name) = vctx
                .items
                .template(entry, 0, Ask::Event(vctx.commands))
                .map(|t| t.name.clone())
            else {
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
        .map(|t| expand(t, d, spells, vctx, &deps, Default::default(), false))
        .unwrap_or_default();
    let view = benilla_ui::script::SpellTooltipView {
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
    };
    Some((view, deps.into_inner()))
}

/// Build one aura's tooltip view, the aura builder `0x52f880`'s lines for `key`, and what it read.
/// The text is `AuraDescription` alone, empty with none (`0x52f929`-`0x52f938`), its tokens at the
/// key's level and, for a spell whose `StackAmount` is above 1, at its stack, else at 1
/// (`0x52f8ca`-`0x52f8dc`).
pub(super) fn build_aura_view(
    key: AuraKey,
    spells: &Spells,
    vctx: &ViewCtx,
) -> Option<(AuraTooltipView, Deps)> {
    let d = spells.catalog.get(key.spell_id)?;
    let deps = RefCell::new(Deps::default());
    deps.borrow_mut().mod_bits |= vctx.spell_mods.read_bits(d);
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
        .map(|t| expand(t, d, spells, vctx, &deps, at, true))
        .unwrap_or_default();
    let view = AuraTooltipView {
        name: d.name.clone(),
        // The right column (`0x52f8e5`), gated on `SpellDispelType.dbc` `[+0x28]`.
        dispel_type: spells.catalog.dispel_label(d).map(str::to_string),
        description,
    };
    Some((view, deps.into_inner()))
}

/// The reputation line's facts (`0x52f2ae`-`0x52f38e`), read off the active player, the builder's
/// `[ebp-0x18]`, whatever the caster: its standing with `MinFactionId` (`0x4d6370`) against the
/// floor of `MinReputation` in `0x80928c`, and its gender for the label (`0x612bf0`), read as the
/// reputation chat reads it (`net::session`'s `self_facts`, `UNIT_FIELD_BYTES_0` byte 2), where
/// `0x5ed5b0` reads `PLAYER_BYTES_3` byte 0. No line without a faction id.
fn reputation_requirement(
    d: &benilla_formats::SpellDisplay,
    vctx: &ViewCtx,
    deps: &mut Deps,
) -> Option<benilla_ui::script::ReputationRequirement> {
    if d.min_faction_id == 0 {
        return None;
    }
    deps.reputation = true;
    deps.gender = true;
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
/// passing `at`, each read recorded in `deps`. `unmodified_points` is the aura builder's call,
/// the points' modifiers off (`0x52f946`).
fn expand(
    text: &str,
    d: &benilla_formats::SpellDisplay,
    spells: &Spells,
    vctx: &ViewCtx,
    deps: &RefCell<Deps>,
    at: benilla_formats::ExpandAt,
    unmodified_points: bool,
) -> TooltipText {
    let caster = vctx.caster;
    // The `$`-tokens' level (`0x5075f0` hands the selector to `0x6e3130`, `507645`-`50764a`).
    let skill = |id| match caster {
        ViewCaster::Player => {
            let line = vctx.skill_lines.and_then(|c| c.spell_to_line(id));
            if let Some(line) = line {
                deps.borrow_mut().skill_line(line);
            }
            line.map_or(0, |line| crate::spell::skill_line_value(vctx.store, line))
        }
        // The pet's level is one of the pet's own inputs.
        ViewCaster::Pet(pet) => crate::spell::pet_skill_value(pet),
    };
    // A cross-referenced spell's own family mask decides its modifiers.
    let lookup = |id| {
        let t = spells.catalog.get(id)?;
        deps.borrow_mut().mod_bits |= vctx.spell_mods.read_bits(t);
        Some(t)
    };
    let home = vctx.home_area;
    let home_area = || {
        deps.borrow_mut().home = true;
        home
    };
    // `$g`/`$G` read the active player's gender byte (`0x508214`), a pet view's too.
    let gender = || {
        deps.borrow_mut().gender = true;
        vctx.store.and_then(|s| s.0.unit_gender()).unwrap_or(0)
    };
    let ctx = benilla_formats::TokenContext {
        durations: &spells.durations,
        radii: &spells.radii,
        ranges: Some(&spells.ranges),
        skill: &skill,
        lookup: &lookup,
        mods: Some(vctx.spell_mods),
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
pub(super) fn tooltip_text(parts: Vec<benilla_formats::TokenPart>) -> TooltipText {
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
    deps: &mut Deps,
) -> Option<TooltipText> {
    let (which, key) = match d.effects[0] {
        EFFECT_ATTACK => (Chance::Crit, "CHANCE_TO_CRIT"),
        EFFECT_DODGE if d.passive => (Chance::Dodge, "CHANCE_TO_DODGE"),
        EFFECT_PARRY if d.passive => (Chance::Parry, "CHANCE_TO_PARRY"),
        EFFECT_BLOCK if d.passive => (Chance::Block, "CHANCE_TO_BLOCK"),
        _ => return None,
    };
    deps.chance(which);
    // The player block's float widened to a double (`0x52f652`) into the key's `%.2f%%`.
    let percent = which.read(&store?.0)?;
    Some(TooltipText::keyed(
        key,
        vec![KeyedArg::F(f64::from(percent))],
    ))
}

/// What the feed knows of the views it has pushed: what each was built from and what the inputs
/// were when the last frame looked. The reference rebuilds on every hover and keeps no such
/// snapshot.
#[derive(Default)]
pub(super) struct SpellFeedMemory {
    /// The views built against the player, by spell, with what each read.
    pub(super) pushed: HashMap<u32, Deps>,
    /// The spells whose view built against the pet is in the VM's pet store, and what each read
    /// of the player's side (the pet's own inputs are [`Self::pet`]).
    pub(super) pet_pushed: HashMap<u32, Deps>,
    /// The aura views, by the aura builder's key, with what each read.
    pub(super) aura_pushed: HashMap<AuraKey, Deps>,
    /// What the pet views read off the pet unit.
    pet: Option<PetInputs>,
    /// The player-side inputs the pushed views were checked against last frame.
    pub(super) seen: Option<Seen>,
    /// The talent modifier tables as last frame's check found them.
    pub(super) mods: Option<crate::spell::SpellModifiers>,
    /// Per reagent on show, the state its views were built against.
    pub(super) reagents: Reagents,
}

impl SpellFeedMemory {
    /// Queue for a rebuild the pushed views, the player's, the pet's and the auras', that read
    /// what `changes` moved.
    pub(super) fn requeue(
        &mut self,
        changes: &Changes,
        wanted: &mut Vec<u32>,
        wanted_pet: &mut Vec<u32>,
        wanted_aura: &mut Vec<AuraKey>,
    ) {
        if changes.is_empty() {
            return;
        }
        for (pushed, queue) in [
            (&mut self.pushed, wanted),
            (&mut self.pet_pushed, wanted_pet),
        ] {
            requeue_hits(pushed, changes, queue);
        }
        requeue_hits(&mut self.aura_pushed, changes, wanted_aura);
    }
}

/// Move each view of `pushed` that read what `changes` moved onto `queue`.
fn requeue_hits<K: Copy + Eq + std::hash::Hash>(
    pushed: &mut HashMap<K, Deps>,
    changes: &Changes,
    queue: &mut Vec<K>,
) {
    pushed.retain(|&key, deps| {
        let hit = deps.hits(changes);
        if hit {
            queue.push(key);
        }
        !hit
    });
}

/// What a reagent's line renders for `entry`: the carried count and whether its name has streamed,
/// read without an ask: the view's build asked.
pub(super) fn reagent_state(
    entry: u32,
    store: Option<&ObjectStore>,
    objects: &Objects,
    items: &Items,
) -> (u32, bool) {
    let named = items.template(entry, 0, Ask::Read).is_some();
    let owned = store.map_or(0, |s| {
        count_of(&s.0, objects, entry, InventoryScope::DEFAULT)
    });
    (owned, named)
}

/// The pet unit's fields a pet view reads (`0x52e610` with the unit selector set): its level, the
/// cost bases `0x612c50` reads, its schools' cost terms (`0x6e32ad`, `0x6e32c0`), its cast speed
/// (`0x6e342c`), its form, and its and its melee target's reach and motion.
#[derive(Clone, Copy, PartialEq, Default)]
pub(super) struct PetInputs {
    guid: Option<u64>,
    level: u32,
    base_mana: u32,
    base_health: u32,
    /// `POWER_COST_MODIFIER` and `POWER_COST_MULTIPLIER` (as bits), by school.
    cost_modifier: [i32; 7],
    cost_multiplier: [u32; 7],
    /// As bits, so a NaN compares equal to itself.
    cast_speed: Option<u32>,
    form: u8,
    range_units: RangeSeen,
}

impl PetInputs {
    pub(super) fn of(
        guid: Option<u64>,
        pet: Option<&benilla_protocol::ObjectFields>,
        range_units: RangeSeen,
    ) -> Self {
        let Some(p) = pet else {
            return Self {
                guid,
                ..Self::default()
            };
        };
        Self {
            guid,
            level: p.unit_level().unwrap_or(0),
            base_mana: p.unit_base_mana().unwrap_or(0),
            base_health: p.unit_base_health().unwrap_or(0),
            cost_modifier: std::array::from_fn(|s| {
                p.unit_power_cost_modifier(s as u32).unwrap_or(0)
            }),
            cost_multiplier: std::array::from_fn(|s| {
                p.unit_power_cost_multiplier(s as u32)
                    .map_or(0, f32::to_bits)
            }),
            cast_speed: p.unit_mod_cast_speed().map(f32::to_bits),
            form: p.unit_shapeshift_form(),
            range_units,
        }
    }
}

/// The hoverable spell sources [`feed_spell_tooltips`] reads, one parameter under Bevy's 16.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct SpellTooltipSources<'w> {
    spells: Option<Res<'w, Spells>>,
    trainer_subjects: Option<Res<'w, TrainerTooltipSubjects>>,
    talents: Option<Res<'w, crate::ui_talent::Talents>>,
}

/// Push a view for every spell the UI can hover (the book, the class's talent ranks, the open
/// trainer's services, and what the VM holds: the pet's spells, the quest rewards and the craft's
/// subjects) and for every aura the VM holds (every unit's and the tracking spell) before it is
/// hovered, as the reference reads them all locally; an ask for any other id or aura too.
#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn feed_spell_tooltips(
    script: Option<NonSendMut<UiScript>>,
    actions: Option<Res<PlayerActions>>,
    spell_sources: SpellTooltipSources,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    // The caster's and the pet's reach and motion and each one's auto-attack target, which the
    // melee range cell reads.
    range_units: crate::spell::RangeUnits,
    objects: Objects,
    home_bind: Option<Res<crate::net::HomeBind>>,
    area_names: Option<Res<crate::ui_quest_log::QuestHeaderNamesRes>>,
    mut items: ResMut<Items>,
    // One tuple param, under Bevy's 16-param ceiling.
    lookups: (
        Option<Res<crate::ui_items::ItemSubClasses>>,
        Res<crate::spell::SpellModifiers>,
        Option<Res<crate::ui_spellbook::SkillLines>>,
        // `Faction.dbc` and the standings, the reputation line's.
        Option<Res<crate::target::Factions>>,
        Option<Res<crate::net::Reputations>>,
    ),
    commands: Res<NetCommands>,
    mut memory: Local<crate::ui_script::VmMemo<SpellFeedMemory>>,
) {
    let (sub_classes, spell_mods, skill_lines, factions, reputations) = &lookups;
    let SpellTooltipSources {
        spells,
        trainer_subjects,
        talents,
    } = spell_sources;
    let Some(mut script) = script else {
        return;
    };
    let memory = memory.get(&script);
    let Some(spells) = spells.as_deref() else {
        return;
    };
    let mut wanted: Vec<u32> = script.take_spell_tooltip_asks();
    // What the VM holds for a hover: quest rewards and craft subjects.
    wanted.extend(
        script
            .spell_tooltip_subjects()
            .into_iter()
            .filter(|s| !memory.pushed.contains_key(s)),
    );
    if let Some(actions) = actions.as_deref() {
        wanted.extend(
            actions
                .spells
                .iter()
                .copied()
                .filter(|s| !memory.pushed.contains_key(s)),
        );
    }
    // The aura views: every unit's auras, the tracking spell, and what their setters asked for.
    let mut wanted_aura: Vec<AuraKey> = script.take_aura_tooltip_asks();
    wanted_aura.extend(
        script
            .aura_tooltip_subjects()
            .into_iter()
            .filter(|k| !memory.aura_pushed.contains_key(k)),
    );
    // The pet views: the pet's bar and book, and what their setters asked for.
    let mut wanted_pet: Vec<u32> = script.take_pet_spell_tooltip_asks();
    wanted_pet.extend(
        script
            .pet_spell_tooltip_subjects()
            .into_iter()
            .filter(|s| !memory.pet_pushed.contains_key(s)),
    );
    // The open trainer's services, which the detail icon's `SetTrainerService` renders, a
    // pet-learn service's against the pet.
    if let Some(trainer) = trainer_subjects.as_deref() {
        wanted.extend(
            trainer
                .player
                .iter()
                .copied()
                .filter(|s| !memory.pushed.contains_key(s)),
        );
        wanted_pet.extend(
            trainer
                .pet
                .iter()
                .copied()
                .filter(|s| !memory.pet_pushed.contains_key(s)),
        );
    }
    // Every rank of the class's talents: a talent tooltip reads the current and the next rank.
    if let (Some(talents), Ok(store)) = (talents.as_deref(), self_q.single()) {
        let race = store.0.unit_race().unwrap_or(0);
        let class = store.0.unit_class().unwrap_or(0);
        for tab in talents.catalog.tabs_for_class(race, class) {
            for t in talents.catalog.talents_in_tab(tab.id) {
                wanted.extend(
                    t.ranks
                        .iter()
                        .copied()
                        .filter(|s| *s != 0 && !memory.pushed.contains_key(s)),
                );
            }
        }
    }
    let home_area: Option<String> = home_bind
        .as_deref()
        .and_then(|b| b.0)
        .and_then(|id| area_names.as_deref()?.0.resolve(id as i32))
        .map(str::to_string);
    let self_store = self_q.single().ok();
    let caster_unit = range_units.caster();
    let attack_target = range_units.caster_attack_target();
    // What moved since the last frame, which each pushed view is checked against: the bind
    // point (Astral Recall's `$z`), the form, the skills, what the equipped-item search learns of
    // each equipment slot and the disarm flag, the level, cost bases and ranged attack time, the
    // block, dodge, parry and crit percentages, and the two units the melee range cell reads.
    let mut seen = Seen::of(
        self_store,
        home_area.as_deref(),
        RangeSeen::of(&caster_unit, attack_target.as_ref()),
        &objects,
        &items,
    );
    if let Some(reputations) = reputations.as_deref() {
        seen = seen.with_reputations(reputations);
    }
    let mut changes = memory
        .seen
        .as_ref()
        .map(|prev| seen.changes_since(prev))
        .unwrap_or_default();
    memory.seen = Some(seen);
    // The talent tables: the cost, cast, cooldown and range cells and the description's tokens
    // resolve through them.
    if let Some(prev) = memory.mods.as_ref().filter(|_| spell_mods.is_changed()) {
        changes = changes.with_mods(spell_mods.diff(prev));
    }
    if memory.mods.is_none() || spell_mods.is_changed() {
        memory.mods = Some((**spell_mods).clone());
    }
    // And the reagents on show: their counts and whether their names have streamed.
    let reagent_state: Reagents = memory
        .reagents
        .keys()
        .map(|&entry| (entry, reagent_state(entry, self_store, &objects, &items)))
        .collect();
    changes = changes.with_reagents(&memory.reagents, &reagent_state);
    memory.reagents = reagent_state;
    memory.requeue(&changes, &mut wanted, &mut wanted_pet, &mut wanted_aura);
    // The pet's inputs: a new pet, or its level, bases, form, reach or motion moving, rebuilds the
    // pet views alone.
    let pet_guid = self_store.and_then(|s| s.0.unit_pet_guid());
    let pet = pet_guid.and_then(|g| objects.object(g));
    let pet_entity = pet_guid.and_then(|g| objects.entity(g));
    let pet_unit =
        pet_entity
            .and_then(|e| range_units.unit(e))
            .unwrap_or(benilla_formats::RangeUnit::still(
                crate::spell::DEFAULT_REACH,
            ));
    let pet_target = pet_entity.and_then(|e| range_units.attack_target(e));
    let pet_inputs = PetInputs::of(pet_guid, pet, RangeSeen::of(&pet_unit, pet_target.as_ref()));
    if memory.pet != Some(pet_inputs) {
        memory.pet = Some(pet_inputs);
        wanted_pet.extend(memory.pet_pushed.drain().map(|(id, _)| id));
    }
    // The sources overlap (a self-buff is in the book and on the player): build each id once.
    wanted.sort_unstable();
    wanted.dedup();
    wanted_pet.sort_unstable();
    wanted_pet.dedup();
    wanted_aura.sort_unstable();
    wanted_aura.dedup();
    let (built, built_aura, built_pet) = {
        let mut vctx = ViewCtx {
            factions: factions.as_deref().map(|f| f.catalog()),
            reputations: reputations.as_deref(),
            home_area: home_area.as_deref(),
            form: self_store.map_or(0, |s| s.0.unit_shapeshift_form()),
            store: self_store,
            caster: ViewCaster::Player,
            range_caster: caster_unit,
            attack_target,
            objects: &objects,
            items: &mut items,
            commands: &commands,
            sub_classes: sub_classes.as_deref().map(|c| &c.0),
            skill_lines: skill_lines.as_deref().map(|s| &s.catalog),
            spell_mods,
        };
        let built = build_views(wanted, spells, &mut vctx, memory, false);
        let built_aura: Vec<(AuraKey, AuraTooltipView)> = wanted_aura
            .into_iter()
            .filter_map(|key| {
                let (view, deps) = build_aura_view(key, spells, &vctx)?;
                memory.aura_pushed.insert(key, deps);
                Some((key, view))
            })
            .collect();
        // The pet views read the pet's form and reach where the player's read the player's.
        vctx.caster = ViewCaster::Pet(pet);
        vctx.form = pet.map_or(0, |p| p.unit_shapeshift_form());
        vctx.range_caster = pet_unit;
        vctx.attack_target = pet_target;
        let built_pet = build_views(wanted_pet, spells, &mut vctx, memory, true);
        (built, built_aura, built_pet)
    };
    for (id, view) in built {
        script.set_spell_tooltip(id, view);
    }
    for (key, view) in built_aura {
        script.set_aura_tooltip(key, view);
    }
    for (id, view) in built_pet {
        script.set_pet_spell_tooltip(id, view);
    }
}

/// Build the views of `ids` against `vctx`'s caster, recording each with what it read in the
/// pushed set `pet` names and watching its reagents, seeded with the state the view was built
/// against.
fn build_views(
    ids: Vec<u32>,
    spells: &Spells,
    vctx: &mut ViewCtx,
    memory: &mut SpellFeedMemory,
    pet: bool,
) -> Vec<(u32, benilla_ui::script::SpellTooltipView)> {
    let mut built = Vec::new();
    for id in ids {
        let Some((view, deps)) = build_view(id, spells, vctx) else {
            continue;
        };
        for &entry in deps.reagents() {
            if let std::collections::btree_map::Entry::Vacant(slot) = memory.reagents.entry(entry) {
                slot.insert(reagent_state(entry, vctx.store, vctx.objects, vctx.items));
            }
        }
        built.push((id, view));
        let pushed = if pet {
            &mut memory.pet_pushed
        } else {
            &mut memory.pushed
        };
        pushed.insert(id, deps);
    }
    built
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::field::{
        FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY, FIELD_PLAYER_INV_SLOT_HEAD,
    };
    use benilla_protocol::ObjectFields;

    /// The reagent line's carried test is `0x622270` at mask 0 (`0x52f49f`): a copy in the keyring
    /// or worn in a slot is carried, so its count shows.
    #[test]
    fn a_reagent_line_counts_the_keyring_and_worn_gear() {
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
            let items = &deps.items;
            let mut state = bevy::ecs::system::SystemState::<Objects>::new(&mut deps.world);
            let objects = state.get(&deps.world);
            let (owned, _) = reagent_state(17056, Some(&store), &objects, items);
            assert_eq!(owned, 3, "player slot {slot}");
        }
    }
}
