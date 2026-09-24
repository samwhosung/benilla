//! The world-mouseover tooltip system (decision 0274 P3) — the app half of the byte-verified
//! mouseover flow (0276): the engine rebuilds the tooltip **once per hover-target change**
//! (`world_tooltip_unit` / `world_tooltip_gameobject`: default anchor via
//! `OnTooltipSetDefaultAnchor`, the verified line laws, `UPDATE_MOUSEOVER_UNIT` for the unit
//! recolor), the health bar tracks the per-frame `set_unit` pushes in between (the HEALTH
//! watcher), and hover loss ARMS a fade (`world_tooltip_fade`) rather than hiding.
//!
//! The picks are the byte-verified pair the cursor already rides: [`Hovered`] (units) and
//! [`HoveredObject`] (GameObjects), arbitrated by [`go_is_nearest`] exactly like the click
//! router. A hovered GameObject shows its template name (gold) and, when flag-locked, the red
//! Lock.dbc requirement lines ("Requires <key item>" / "Requires <skill>") — the verified
//! `0x52aa20` law. The standalone-corpse builder joins when corpse objects stream.

use crate::ui_items::{count_of, InventoryScope};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_ui::script::{TooltipTint, UiScript, UnitState};
use benilla_ui::strings::Arg;

use crate::items::Items;
use crate::names::NameCache;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::target::{
    go_is_nearest, ring_reaction, Hovered, HoveredObject, GO_FLAG_LOCKED, GO_TYPE_GENERIC,
};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_unit::{enrich_unit, snapshot, UnitFeed};

pub struct UiTooltipPlugin;

impl Plugin for UiTooltipPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                drive_mouseover_tooltip.in_set(UnitFeed),
                feed_spell_tooltips.in_set(UnitFeed),
            ),
        );
    }
}

/// Everything a view needs beyond the spell catalogs — the player-dependent halves of the line
/// law: the worn set (the equipped-item test, `0x5f0c50`), the bags (the reagent possession, and
/// the item-name cache the reagent names come from), the current form, and the bind point `$z`
/// substitutes against.
struct ViewCtx<'a, 'w, 's> {
    home_area: Option<&'a str>,
    form: u8,
    store: Option<&'a ObjectStore>,
    /// The caster's own `UNIT_FIELD_COMBATREACH`. 1.5 is the descriptor default, not a guess.
    combat_reach: f32,
    /// The reach of whoever the caster is currently auto-attacking, when it is auto-attacking —
    /// the melee range arm's second term. The tooltip passes no target, but `0x6e3480`'s melee
    /// arm resolves `[caster+0xc48]` itself, so this cell moves with the mob you are swinging at
    /// and falls back to doubling the caster's own reach when nothing is engaged.
    attack_target_reach: Option<f32>,
    /// The one object index (2334) — what the possession cells resolve an instance guid
    /// through: the worn-item line's search and each reagent's carried count.
    objects: &'a Objects<'w, 's>,
    items: &'a mut Items,
    commands: &'a NetCommands,
    sub_classes: Option<&'a benilla_formats::ItemSubClassCatalog>,
    /// The talent spell-modifier tables — the cost cell shows the RESOLVED cost, which since
    /// `SPELLMOD_COST` landed means the modified one (`crate::spell::usable::power_cost`).
    spell_mods: &'a crate::spell::SpellModifiers,
    /// The VM's own `GlobalStrings.lua` (decision 2045) — every cell this builder composes is a
    /// key, and this is where they resolve. `text` is the `%d`-filling twin the `$`-engine's
    /// keyed tokens take (`benilla_formats::TokenContext::text`).
    get: &'a dyn Fn(&str) -> Option<String>,
    text: &'a dyn Fn(&str, &[i64]) -> Option<String>,
}

/// Fill a key's template out of the VM's own `GlobalStrings.lua`, or nothing at all when the
/// chain has no string for it — the reference's data-suppression face, and the reason no cell
/// here carries a fallback sentence (decision 2045).
fn keyed(get: &dyn Fn(&str) -> Option<String>, key: &str, args: &[Arg<'_>]) -> Option<String> {
    let text = benilla_ui::strings::fill(&get(key)?, args);
    (!text.is_empty()).then_some(text)
}

/// Build one spell's tooltip view (decision 0274 P2) — the verified spell line law's inputs,
/// every string resolved here where the catalogs live: the cost cell (the RESOLVED
/// `power_cost` through the power-type key array `0x85416c`, health fallback, `_PER_TIME`
/// composite — 1074; rage prints wire-cost ÷ 10), the range cell ("N yd range", "N-M yd range" when
/// the resolved min is nonzero — the law's `"%d-%d"` fork; the melee family resolves through the
/// same key off the caster's combat reach, and the on-next-swing class shows no cell at all), the cast cell
/// (the full `52eb45` ladder: sec/min, the negative-base sentinel, "Next melee"/"Attack
/// speed"/"Channeled", and the mana-keyed Instant fork — 1074; None = the law's
/// passive gate, which omits the whole line), the cooldown cell
/// (`max(RecoveryTime, CategoryRecoveryTime)` at `0x52eada`), the required-item and
/// required-form lines (`[0x52eea7, 0x52f2ae)`), the reagents line (`SPELL_REAGENTS 0x854e54`),
/// and the $-substituted description/aura-description (byte-exact formulas,
/// `benilla_formats::substitute`).
///
/// Line TEXT is composed in English here, as every cell in this builder is; the reference reads
/// its own GlobalStrings templates ("Reagents: ", "Requires %s"). That is this builder's standing
/// INTERIM shape, not a new debt introduced by these lines.
fn spell_tooltip_view(
    spell_id: u32,
    spells: &Spells,
    vctx: &mut ViewCtx,
) -> Option<benilla_ui::script::SpellTooltipView> {
    let d = spells.catalog.get(spell_id)?;
    let home_area = vctx.home_area;
    let form = vctx.form;
    let ctx = benilla_formats::TokenContext {
        durations: &spells.durations,
        radii: &spells.radii,
        lookup: &|id| spells.catalog.get(id),
        home_area,
        text: vctx.text,
    };
    // The cost cell (the `0x52e8ad` caller): the RESOLVED cost — `GetPowerCost`'s
    // number, the same `power_cost` the usable walk compares (0948) — through the power-type key
    // array (`0x85416c`: Mana/Rage/Focus/Energy/Happiness) with HEALTH as the out-of-range
    // fallback (the `jl`/`cmp 5` fork at `0x52e8fc` — Life Tap's and Bloodrage's −2 → "N Health";
    // B192, 1074). Rage displays wire÷10 (the `0x6e7130` per-type divisor), applied to the flat
    // and per-second values alike; a `manaPerSecond` column composes the `_PER_TIME` string
    // ("11 Health, plus 5 per sec" — Health Funnel), including the cost-0 case. Zero cost with
    // zero per-second leaves the cell EMPTY. The ref never prints a percentage — a pct-only cost
    // resolves to its flat number like any other, which is why the "% of base mana" line this
    // replaces was unfaithful (B152). No store (a DBC-only view) degrades to the flat cost.
    // HAPPINESS_COST has no GlobalStrings entry and no 5875 player spell reaches powerType 4;
    // the unit word is a dead arm kept for the array's shape.
    let resolved_cost = vctx.store.map_or(d.mana_cost, |s| {
        crate::spell::usable::power_cost(d, s, vctx.spell_mods)
    });
    let cost = {
        // The one `0x6e7130` table, not a local `if power_type == 1` — decision 2117 found three
        // hand-rolled copies of it and one of them had been applied at a single site out of four.
        let div = benilla_protocol::messages::power_display_scale(d.power_type);
        let unit = match d.power_type {
            0 => "Mana",
            1 => "Rage",
            2 => "Focus",
            3 => "Energy",
            4 => "Happiness",
            _ => "Health",
        };
        if resolved_cost == 0 && d.mana_per_second == 0 {
            None
        } else if d.mana_per_second > 0 {
            Some(format!(
                "{} {unit}, plus {} per sec",
                resolved_cost / div,
                d.mana_per_second / div
            ))
        } else {
            Some(format!("{} {unit}", resolved_cost / div))
        }
    };
    // The range cell: two attribute gates, then `GetMinMaxRange 0x6e3480` with **`target = NULL`**
    // (`0x52e9c2`), then a third gate on the resolved `max <= 0`.
    //
    // **There is no melee wording anywhere in the client** — `SPELL_RANGE_AREA` is not in
    // `WoW.exe` at all, and the melee family is not a display name either: a melee spell is one
    // whose `SpellRange` row sets flags bit 0 (the single shipped such row is id 2 "Combat
    // Range", 813 spells), and the resolver hands back `max(reach + casterReach + 1.3333334,
    // 5.0)` — which then prints through the SAME `SPELL_RANGE` + `"%d"` path as every other
    // range, normally **"5 yd range"** (decision 2080 named this cell; it printed the invented
    // "Melee Range" until it was converted).
    //
    // The tooltip passes no target, but that does NOT make both reaches the caster's: the melee
    // arm resolves the caster's own `attack_target_guid` and uses that unit's reach, so the cell
    // reads wider while you are auto-attacking something big.
    let range = (!d.tooltip_omits_range_line())
        .then(|| {
            let (min, max) = benilla_formats::min_max_range(
                d,
                spells.ranges.get(d.range_index),
                vctx.combat_reach,
                vctx.attack_target_reach,
            )?;
            if max <= 0.0 {
                return None;
            }
            // `SPELL_RANGE = "%s yd range"` — and its hole is a **string**, which is what makes
            // the law's pair fork (`0x854fb4`'s `"%d-%d"`, Charge: 8-25) expressible at all: the
            // number cell is composed by a nested `SStrPrintf` first and handed over whole. Both
            // holes are `fistp` conversions — round-to-nearest, never truncation (the melee sum
            // is the only value that is ever fractional).
            let yd = |v: f32| f64::from(v).round_ties_even() as i64;
            let yards = if min > 0.0 {
                format!("{}-{}", yd(min), yd(max))
            } else {
                format!("{}", yd(max))
            };
            keyed(vctx.get, "SPELL_RANGE", &[Arg::S(&yards)])
        })
        .flatten();
    // The cast line gate `0x52eb15` — wider than the spellbook's `passive`: a TRADE_SKILL or ATTACK
    // Effect[0] omits the line too ([`SpellDisplay::tooltip_omits_cast_line`]). The arm order is
    // the byte ladder `0x52eb45-0x52ec90` (1074): a positive time prints sec/min at the 60 s
    // threshold; a NEGATIVE SpellCastTimes base is the "Instant cast" sentinel (`52ebce`); at
    // zero — next-swing (attr 0x404) → "Next melee", auto-shot (attr 0x2) → "Attack speed",
    // channeled (attrEx 0x44) → "Channeled", else the mana fork: "Instant cast" iff
    // powerType==0 AND the resolved cost is nonzero (`52ec4b` re-reads `GetPowerCost`'s edi) —
    // every other type reads bare "Instant" whatever it costs (Sinister Strike, Life Tap).
    let cast_time = if d.tooltip_omits_cast_line() {
        None
    } else {
        let base = spells
            .cast_times
            .get(d.casting_time_index)
            .map(|c| c.base_ms as i32)
            .unwrap_or(0);
        // Each arm is a key. The two timed ones are `%.3g` templates — significant digits, not
        // decimals — so the seconds are handed over as a real and the shipped string decides how
        // it reads (decision 2080); this used to round to one decimal on our side.
        if base > 0 {
            let (key, v) = if base >= 60_000 {
                ("SPELL_CAST_TIME_MIN", f64::from(base) / 60_000.0)
            } else {
                ("SPELL_CAST_TIME_SEC", f64::from(base) / 1000.0)
            };
            keyed(vctx.get, key, &[Arg::F(v)])
        } else {
            let key = if base < 0 {
                "SPELL_CAST_TIME_INSTANT"
            } else if d.on_next_swing() {
                "SPELL_ON_NEXT_SWING"
            } else if d.tooltip_on_next_ranged() {
                "SPELL_ON_NEXT_RANGED"
            } else if d.tooltip_channeled() {
                "SPELL_CAST_CHANNELED"
            } else if d.power_type == 0 && resolved_cost > 0 {
                "SPELL_CAST_TIME_INSTANT"
            } else {
                // "Instant" without the "cast" — its own key, and the reason the mana fork above
                // exists at all.
                "SPELL_CAST_TIME_INSTANT_NO_MANA"
            };
            keyed(vctx.get, key, &[])
        }
    };
    // The cooldown cell reads BOTH recovery columns (`max([+0x4c],[+0x50])>0` at `0x52eada` —
    // Charge's 15 s is CategoryRecoveryTime; its RecoveryTime is 0).
    let recovery_ms = d.recovery_ms.max(d.category_recovery_ms);
    let cooldown = (recovery_ms > 0)
        .then(|| {
            let secs = f64::from(recovery_ms) / 1000.0;
            let (key, v) = if secs >= 60.0 {
                ("SPELL_RECAST_TIME_MIN", secs / 60.0)
            } else {
                ("SPELL_RECAST_TIME_SEC", secs)
            };
            keyed(vctx.get, key, &[Arg::F(v)])
        })
        .flatten();
    // The required-form line: the Stances mask's form names off
    // SpellShapeshiftForm.dbc, joined; bit b = form id b+1. Met against the CURRENT form.
    //
    // `Stances != 0` is NOT the gate on its own — the column is overloaded. When `AttributesEx2`
    // bit 19 is set the mask is *permissive* ("may ALSO be cast in these forms"), which is how
    // 5875 encodes Shadowform-castable priest spells, Spirit-of-Redemption-castable heals and
    // Moonkin-castable balance spells; reading those as requirements put a red "Requires
    // Shadowform" on Inner Fire and "Requires Spirit of Redemption" on Flash Heal (1483).
    //
    // The block is `[0x52f10a, 0x52f2ae)`: `52f115` reads AttributesEx2,
    // ands `0x80000`, and jumps *over the whole name loop* — the loop is the only writer of the
    // "nothing appended" flag `[ebp-8]`, so the AddLine is skipped entirely. The line is
    // **suppressed, not recoloured**. Three earned negatives: StancesNot `+0x30` is
    // never read here, `Attributes` bit 16 is never tested, and `0x612480` is never called — the
    // builder duplicates the gate inline rather than sharing the usable walk's helper, which is
    // why the two could drift apart in the first place.
    //
    // Two ref behaviours we knowingly don't model, both unreachable for us: the composed text is
    // capped at 0x80 bytes (our longest join is "Battle Stance, Berserker Stance"), and a *met*
    // requirement is dropped in the COMPACT tooltip — benilla has no compact spell tooltip.
    let requires_form = (d.stances != 0 && !d.form_mask_is_permissive())
        .then(|| {
            let names: Vec<&str> = (0..32u32)
                .filter(|b| d.stances & (1 << b) != 0)
                .filter_map(|b| spells.forms.get(&(b + 1)).map(|f| f.name.as_str()))
                .filter(|n| !n.is_empty())
                .collect();
            (!names.is_empty())
                .then(|| {
                    keyed(
                        vctx.get,
                        "SPELL_REQUIRED_FORM",
                        &[Arg::S(&names.join(", "))],
                    )
                })
                .flatten()
        })
        .flatten();
    let form_met = form != 0 && d.stances & (1u32 << (u32::from(form) - 1)) != 0;
    // The equipped-item-class half of the requirement lines, `[0x52eea7, 0x52f10a)` — "Requires
    // Wands" on the wand Shoot, and "Requires Melee Weapon" on Parry. A mask with several bits set
    // is NOT a reason to print nothing (what we used to do): the reference names the whole mask
    // through ItemSubClassMask.dbc first, and only falls back to comma-joining the individual
    // subclasses; that lookup lives in the catalog. Its three entry gates, all of which skip the
    // line: Targets bit 0x10 set, class < 0, or an empty mask.
    let requires_item = (d.targets & TARGET_ITEM == 0 && d.equipped_item_class >= 0)
        .then(|| {
            vctx.sub_classes?
                .requirement_name(d.equipped_item_class as u32, d.equipped_item_subclass_mask)
        })
        .flatten()
        .and_then(|name| keyed(vctx.get, "SPELL_EQUIPPED_ITEM", &[Arg::S(&name)]));
    // The chance-to-X line (`[0x52f5b1, 0x52f697)`): `Effect[0]` picks which of the player's four
    // avoidance/crit percentages to print, and — except for ATTACK, which bypasses the gate — the
    // spell must be passive. The percentages are already percents on the wire.
    let chance = chance_line(d, vctx.store);
    let item_met = vctx.store.is_none_or(|s| {
        crate::spell::usable::equipped_item_fits(d, s, vctx.objects, vctx.items, vctx.commands)
    });
    // Reagents (`SPELL_REAGENTS 0x854e54`): the named slots, `count > 1` suffixed, a slot the
    // player is short of wrapped in the builder's inline red. A reagent whose item template hasn't
    // streamed yet is simply absent from this snapshot — `feed_spell_tooltips` re-pushes when it
    // lands, which is our shape of the ref's own query-then-redisplay callback.
    let reagents = {
        let mut parts: Vec<String> = Vec::new();
        for (entry, count) in d.reagents.iter().copied().filter(|&(e, _)| e != 0) {
            let Some(name) = vctx
                .items
                .template(entry, 0, vctx.commands)
                .map(|t| t.name.clone())
            else {
                continue;
            };
            let text = if count > 1 {
                format!("{name} ({count})")
            } else {
                name
            };
            let short = vctx.store.is_some_and(|s| {
                count_of(&s.0, vctx.objects, entry, InventoryScope::CARRIED) < count
            });
            parts.push(if short {
                format!("|cffff2020{text}|r")
            } else {
                text
            });
        }
        (!parts.is_empty()).then(|| format!("Reagents: {}", parts.join(", ")))
    };
    Some(benilla_ui::script::SpellTooltipView {
        name: d.name.clone(),
        rank: d.rank.clone(),
        // The aura variant's right column (`[0x52f8e5, 0x52f911)`) — gated inside the catalog by
        // SpellDispelType.dbc's own `[+0x28]`, so Stealth-class auras hand back None.
        dispel_type: spells.catalog.dispel_name(d).map(str::to_string),
        cost,
        range,
        cast_time,
        cooldown,
        requires_item,
        item_met,
        requires_form,
        form_met,
        chance,
        reagents,
        description: d
            .description
            .as_deref()
            .map(|t| benilla_formats::substitute(t, d, &ctx))
            .unwrap_or_default(),
        aura_description: d
            .aura_description
            .as_deref()
            .map(|t| benilla_formats::substitute(t, d, &ctx))
            .unwrap_or_default(),
    })
}

/// `Targets` bit `0x10` — set ⇒ the equipped-item requirement line is skipped whatever the class
/// and mask say (the line's first gate, `0x52eeaa`; the `TARGET_FLAG_ITEM` name is inferred).
const TARGET_ITEM: u32 = 0x10;

/// `SPELL_EFFECT_DODGE` / `_PARRY` / `_BLOCK` / `_ATTACK` — the four `Effect[0]` values that select
/// a chance-to-X line (the jump table `0x52f7c4`).
const EFFECT_DODGE: u32 = 20;
const EFFECT_PARRY: u32 = 22;
const EFFECT_BLOCK: u32 = 23;
const EFFECT_ATTACK: u32 = 78;

/// The chance-to-X line (`[0x52f5b1, 0x52f697)`) — `None` when the spell names none of the four
/// effects, when the gate rejects it, or before the player's descriptor has streamed.
///
/// The predicate is the reference's, and its asymmetry is the point: **ATTACK bypasses the passive
/// gate** while dodge/parry/block require it. That is why Attack — whose `Attributes` carry `0x10`,
/// not the passive `0x40` — still shows a crit line, which is exactly the missing line reported.
fn chance_line(d: &benilla_formats::SpellDisplay, store: Option<&ObjectStore>) -> Option<String> {
    let (label, percentage) = match d.effects[0] {
        EFFECT_ATTACK => ("crit", store?.0.player_crit_percentage()?),
        EFFECT_DODGE if d.passive => ("dodge", store?.0.player_dodge_percentage()?),
        EFFECT_PARRY if d.passive => ("parry", store?.0.player_parry_percentage()?),
        EFFECT_BLOCK if d.passive => ("block", store?.0.player_block_percentage()?),
        _ => return None,
    };
    // The reference's `%.2f%%` lives in its GlobalStrings, not the binary — this is the same shape.
    Some(format!("{percentage:.2}% chance to {label}"))
}

/// The push loop's change detectors — everything a view SNAPSHOTS at build time. A change to any
/// of them means every pushed view is stale, so the loop re-pushes the lot (the reference simply
/// re-runs its builder on every hover, so a stale snapshot is a benilla-only failure mode).
#[derive(Default)]
struct SpellFeedMemory {
    /// Spell ids whose view has been pushed at least once.
    pushed: std::collections::HashSet<u32>,
    /// The bind point `$z` substitutes against.
    home: Option<String>,
    /// The current shapeshift form (the required-form line's white/red, `0x52f1e3`).
    form: Option<u8>,
    /// The 19 worn-slot guids (the equipped-item line's white/red, `0x5f0c50`).
    worn: Option<[u64; 19]>,
    /// The player's block/dodge/parry/crit percentages, as raw bit patterns so the diff needs no
    /// float comparison (the chance-to-X line's printed value, `0x52f5b1` — it moves with gear,
    /// buffs and talents).
    avoidance: Option<[u32; 4]>,
    /// The two reaches the melee range cell reads, as raw bit patterns — the caster's own
    /// (which moves with scale and shapeshift) and its auto-attack target's (which changes with
    /// the mob).
    combat_reach: Option<(Option<u32>, Option<u32>)>,
    /// Per reagent entry currently on show: `(owned count, item name resolved)` — the reagent
    /// line's inline red (`0x854120`) plus the ask-once template landing.
    reagents: std::collections::BTreeMap<u32, (u32, bool)>,
}

/// The spell-tooltip store's push half: every spell the UI can HOVER is owed its view at
/// arrival — the known book (spellbook/bar), the class's talent rank spells (`SetTalent`'s
/// display + next-rank reads), and the live aura spells (`SetPlayerBuff`) — so a first hover
/// never misses, exactly like the reference's all-local reads. The renderers' recorded asks
/// (the odd id outside those sets) answer through the same build as the fallback.
fn feed_spell_tooltips(
    script: Option<NonSendMut<UiScript>>,
    actions: Option<Res<PlayerActions>>,
    spells: Option<Res<Spells>>,
    talents: Option<Res<crate::ui_talent::Talents>>,
    auras: Option<Res<crate::ui_aura::PlayerAuraCache>>,
    selection: Res<crate::target::Selection>,
    stores: Query<&ObjectStore>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    // Who the player is auto-attacking, if anyone — the melee range cell's second reach.
    engaged_q: Query<&crate::creature_anim::Engaged, With<SelfPlayer>>,
    // The object lookup (2334) — the guid index the engaged-target reach resolves through,
    // plus the bag walk each watched reagent's carried count takes.
    objects: Objects,
    home_bind: Option<Res<crate::net::HomeBind>>,
    area_names: Option<Res<crate::ui_quest_log::QuestHeaderNamesRes>>,
    mut items: ResMut<Items>,
    // One tuple param (Bevy's 16-SystemParam ceiling): the two lookups the view builder reads
    // straight through — the item sub-class names for the required-item line, and the talent
    // spell-modifier tables the cost cell resolves through.
    lookups: (
        Option<Res<crate::ui_items::ItemSubClasses>>,
        Res<crate::spell::SpellModifiers>,
    ),
    commands: Res<NetCommands>,
    mut memory: Local<crate::ui_script::VmMemo<SpellFeedMemory>>,
) {
    let (sub_classes, spell_mods) = &lookups;
    let Some(mut script) = script else {
        return;
    };
    let memory = memory.get(&script);
    let Some(spells) = spells.as_deref() else {
        return;
    };
    let mut wanted: Vec<u32> = script.take_spell_tooltip_asks();
    if let Some(actions) = actions.as_deref() {
        wanted.extend(
            actions
                .spells
                .iter()
                .copied()
                .filter(|s| !memory.pushed.contains(s)),
        );
    }
    // The talent window's hoverables: every rank spell of the class's pages (a talent tooltip
    // reads rank max(1, current) + the next rank — pushing ALL ranks stays correct as ranks
    // are learned, and the whole class set is ~250 ids once).
    if let (Some(talents), Ok(store)) = (talents.as_deref(), self_q.single()) {
        let race = store.0.unit_race().unwrap_or(0);
        let class = store.0.unit_class().unwrap_or(0);
        for tab in talents.catalog.tabs_for_class(race, class) {
            for t in talents.catalog.talents_in_tab(tab.id) {
                wanted.extend(
                    t.ranks
                        .iter()
                        .copied()
                        .filter(|s| *s != 0 && !memory.pushed.contains(s)),
                );
            }
        }
    }
    // The buff bar's hoverables: the live aura spells, at arrival.
    if let Some(auras) = auras.as_deref() {
        wanted.extend(auras.spell_ids().filter(|s| !memory.pushed.contains(s)));
    }
    // The minimap tracking icon's hover (SetTrackingSpell): the tracking aura never enters the
    // display cache above (the rebuild's tracking-effect exclusion, `ui_aura`), so pre-feed from
    // the player's raw aura array — covers it at arrival, at worst a few extra views for
    // display-filtered auras nothing hovers.
    if let Ok(store) = self_q.single() {
        wanted.extend(
            store
                .0
                .unit_auras()
                .map(|a| a.spell_id)
                .filter(|s| !memory.pushed.contains(s)),
        );
    }
    // The target frame's aura rows (SetUnitBuff/SetUnitDebuff): the target's live aura spells,
    // at selection/arrival — same first-hover guarantee as the buff bar's.
    if let Some(store) = selection.target.and_then(|e| stores.get(e).ok()) {
        wanted.extend(
            store
                .0
                .unit_auras()
                .map(|a| a.spell_id)
                .filter(|s| !memory.pushed.contains(s)),
        );
    }
    let home_area: Option<String> = home_bind
        .as_deref()
        .and_then(|b| b.0)
        .and_then(|id| area_names.as_deref()?.0.resolve(id as i32))
        .map(str::to_string);
    // A bind-point change re-substitutes every pushed view ($z — Astral Recall's shape).
    if memory.home != home_area {
        memory.home = home_area.clone();
        wanted.extend(memory.pushed.drain());
    }
    // A stance/form change re-pushes too: the required-form line's white/red tracks the CURRENT
    // form (`0x52f1e3`), and the views are static snapshots until re-pushed.
    let form = self_q
        .single()
        .map(|s| s.0.unit_shapeshift_form())
        .unwrap_or(0);
    if memory.form != Some(form) {
        memory.form = Some(form);
        wanted.extend(memory.pushed.drain());
    }
    let self_store = self_q.single().ok();
    // …and so do the two BAG-dependent halves, which the views likewise snapshot: the worn set
    // (the item line, `0x5f0c50`, flips white/red on a weapon swap) and the owned counts of the
    // reagents currently on show (the inline red `0x854120`, plus the item names themselves landing
    // from the ask-once template cache — the ref's query-then-redisplay callback in our shape).
    // Both signatures are tiny: 19 slot guids, and one count per DISTINCT reagent in play.
    let worn =
        self_store.map(|s| std::array::from_fn(|i| s.0.player_inv_slot(i as u8).unwrap_or(0)));
    if memory.worn != worn {
        memory.worn = worn;
        wanted.extend(memory.pushed.drain());
    }
    // …and so does the chance-to-X line's percentage (`0x52f5b1`), which the views snapshot the
    // same way: a weapon swap, a buff or a talent moves it.
    let avoidance = self_store.map(|s| {
        [
            s.0.player_block_percentage(),
            s.0.player_dodge_percentage(),
            s.0.player_parry_percentage(),
            s.0.player_crit_percentage(),
        ]
        .map(|v| v.unwrap_or(0.0).to_bits())
    });
    if memory.avoidance != avoidance {
        memory.avoidance = avoidance;
        wanted.extend(memory.pushed.drain());
    }
    // …and so does the range cell's melee arm, whose inputs are the caster's own reach and the
    // reach of whatever it is auto-attacking (`0x6e3480` resolves the latter itself).
    let attack_target_reach = engaged_q
        .single()
        .ok()
        .and_then(|e| objects.object(e.0))
        .map(|f| f.unit_combat_reach());
    let reaches = (
        self_store.map(|s| s.0.unit_combat_reach().to_bits()),
        attack_target_reach.map(f32::to_bits),
    );
    if memory.combat_reach != Some(reaches) {
        memory.combat_reach = Some(reaches);
        wanted.extend(memory.pushed.drain());
    }
    // …and so does the cost cell, whose resolved number now goes through the talent
    // spell-modifier tables: a respec changes what every affected spell costs, and this feed is
    // the one consumer of `power_cost` that memoizes its build (the action bar recomputes every
    // frame). The reference has no cache here at all — it reads the tables live at every call
    // site — so this is what keeps the cell honest about the same change (`crate::spell_mods`).
    if spell_mods.is_changed() {
        wanted.extend(memory.pushed.drain());
    }
    let watched: Vec<u32> = memory.reagents.keys().copied().collect();
    let reagent_state: std::collections::BTreeMap<u32, (u32, bool)> = watched
        .into_iter()
        .map(|entry| {
            let named = items.template(entry, 0, &commands).is_some();
            let owned = self_store.map_or(0, |s| {
                count_of(&s.0, &objects, entry, InventoryScope::CARRIED)
            });
            (entry, (owned, named))
        })
        .collect();
    if memory.reagents != reagent_state {
        memory.reagents = reagent_state;
        wanted.extend(memory.pushed.drain());
    }
    // The string table is read while the views are BUILT and the store is written when they are
    // PUSHED, so the two passes are split — one borrow of the VM cannot be both, and the build's
    // borrow ends with this block.
    let mut built: Vec<(u32, benilla_ui::script::SpellTooltipView)> = Vec::new();
    {
        let get = |key: &str| benilla_ui::strings::global(script.lua(), key);
        let text = crate::ui_script::token_text(&script);
        let mut vctx = ViewCtx {
            home_area: home_area.as_deref(),
            form,
            store: self_store,
            combat_reach: self_store.map_or(1.5, |s| s.0.unit_combat_reach()),
            attack_target_reach,
            objects: &objects,
            items: &mut items,
            commands: &commands,
            sub_classes: sub_classes.as_deref().map(|c| &c.0),
            spell_mods,
            get: &get,
            text: &text,
        };
        for id in wanted {
            if let Some(view) = spell_tooltip_view(id, spells, &mut vctx) {
                // Register this spell's reagents in the watch set, seeded with the state the view was
                // just built against — so the next recompute re-pushes on a REAL change only.
                if let Some(d) = spells.catalog.get(id) {
                    for (entry, _) in d.reagents.iter().copied().filter(|&(e, _)| e != 0) {
                        if let std::collections::btree_map::Entry::Vacant(slot) =
                            memory.reagents.entry(entry)
                        {
                            let named = vctx.items.template(entry, 0, vctx.commands).is_some();
                            let owned = vctx.store.map_or(0, |s| {
                                count_of(&s.0, vctx.objects, entry, InventoryScope::CARRIED)
                            });
                            slot.insert((owned, named));
                        }
                    }
                }
                built.push((id, view));
                memory.pushed.insert(id);
            }
        }
    }
    for (id, view) in built {
        script.set_spell_tooltip(id, view);
    }
}

/// What the tooltip was last driven for — the change detector (the byte law rebuilds once per
/// hover-target change).
#[derive(Default, PartialEq, Clone, Copy)]
enum LastHover {
    #[default]
    None,
    Unit(u64),
    Go(u64),
    /// A hovered **corpse object** — its own plate, keyed on the corpse's guid (1729).
    Corpse(u64),
}

/// The snapshot fields the unit tooltip's LINES read (everything except the bar's
/// health/power) — the rebuild key: a change here means the rendered lines are stale.
/// The world-hover driver's own memory, bundled — which plate it put up, the line-affecting fields
/// that plate was built from, and the headless probe's say-once-on-change trace line (2255).
///
/// One [`SystemParam`](bevy::ecs::system::SystemParam) rather than three parameters for the reason
/// [`crate::target::hover::GoPickSet`] is one: [`drive_mouseover_tooltip`] sits at Bevy's 16-param
/// function-system ceiling. A bare tuple did the same job and tripped `clippy::type_complexity`,
/// which is the lint asking for exactly this.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct HoverMemo<'s> {
    /// Which world plate is currently up — the driver's "do not rebuild this every frame" memo.
    /// **Faithful**: on an unchanged mouseover the reference makes no call at all (`0x482090`
    /// returns at `0x4820b5`), so a plate Lua takes mid-hover staying gone is reference behaviour
    /// and not a bug to fix here. See 2255.
    last: Local<'s, crate::ui_script::VmMemo<LastHover>>,
    /// The line-affecting fields the current unit plate was built from, so a late-arriving name or
    /// creature-info rebuilds it under the same hover.
    last_lines: Local<'s, crate::ui_script::VmMemo<Option<UnitState>>>,
    /// The headless probe's last trace line, so a stationary probe says it once (2255).
    trace: Local<'s, String>,
}

fn lines_view(s: &UnitState) -> UnitState {
    UnitState {
        health: 0,
        max_health: 0,
        power: 0,
        max_power: 0,
        ..s.clone()
    }
}

/// The **"Locked" line's colour** (`0x52ab03`-`0x52ab43`, decision 0770).
///
/// The builder seats red `0xc0d3a8` as the default *before* it calls the resolver, then re-colours
/// on the answer. Every non-`Unmet` answer lands on the same green `0xc0d420`, by two separate
/// branches that agree: an opener was found with **no item** and no matched spell (`0x52ab22 je`
/// — the no-requirement case), or an opener was found **with** an item, i.e. a KEY (`0x52ab29
/// jne`). Only a lock nothing can open keeps the red.
///
/// `None` = a flag-locked object with no `Lock.dbc` row at all, which is the reference's
/// no-requirement arm and therefore green — the flag alone is not a refusal.
///
/// **Not modelled:** the third branch, where a *skill* opener satisfied the lock and the colour
/// becomes the difficulty ramp `0x529fa0` (grey `0xc0cf50` / green `0xc0d420` / yellow `0xc0cf18`
/// / orange `0xc0d3a4` / red `0xc0d3a8`, banded at the requirement +0/25/50/100 — the trade-skill
/// ladder). Green is that ramp's comfortable rung, so a well-skilled opener already reads right
/// and only a marginal one reads too green. Pinned in decision 0770; deliberately left for its own
/// change, since it needs the resolver to hand back the margin it currently discards.
fn locked_line_tint(outcome: Option<crate::target::lock::LockOutcome>) -> TooltipTint {
    match outcome {
        Some(crate::target::lock::LockOutcome::Unmet) => TooltipTint::Red,
        _ => TooltipTint::LockOpen,
    }
}

fn drive_mouseover_tooltip(
    script: Option<NonSendMut<UiScript>>,
    hovered: Res<Hovered>,
    hovered_go: Res<HoveredObject>,
    // The cursor-arm seat of the GO anchor fork (decision 0766).
    window: Query<&Window, With<PrimaryWindow>>,
    stores: Query<&ObjectStore>,
    // The stored GAMEOBJECT_STATE the lock lines' Action gate reads (decision 0752).
    anims: Query<&crate::go_anim::GoAnim>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    // The standing pair as one param — see [`crate::target::ReactionInputs`]. Bundled here and
    // not elsewhere because this system sits on Bevy's tuple limit.
    rx: crate::target::ReactionInputs,
    // The lock chain's own data set, shared verbatim with the click router (`target::lock`) so the
    // hover and the click can never disagree about whether a lock is satisfiable — the same reason
    // `usable` and the click share one resolver (0752). Carries the go-template, Lock.dbc and
    // item caches this system used to take as three separate params.
    go_inputs: crate::target::lock::GoLockInputs,
    // The known-spell set the resolver's SKILL arm scans.
    player_actions: Res<crate::ui_action::PlayerActions>,
    // The cursor seat crosses the VM seam (0582/0584): the anchor below is UI units, not px.
    ui_scale: Res<crate::ui_script::UiScaleCvar>,
    mut memo: HoverMemo,
    // `ChrClasses.dbc` field 16 — `UnitHasRelicSlot`'s only input. Absent when the client data
    // failed to load, in which case no class reads as having a relic slot.
    classes: Option<Res<crate::chr_classes::ChrClassTable>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let (last, last_lines, trace) = (&mut memo.last, &mut memo.last_lines, &mut *memo.trace);
    let last = last.get(&script);
    let last_lines = last_lines.get(&script);
    let self_store = self_q.iter().next();
    let chr = classes.as_deref().map(|t| &t.0);

    // The hovered UNIT's snapshot (a hovered non-unit resolves no store here).
    let unit = hovered.target.zip(hovered.guid).and_then(|(entity, guid)| {
        let store = stores.get(entity).ok()?;
        let name = names
            .resolve_unit(guid, Some(store), &commands)
            .map(str::to_string);
        let reaction = ring_reaction(
            rx.factions.as_deref(),
            &rx.reputations,
            Some(store),
            self_store,
        ) + 1;
        let mut s = snapshot(store, name, reaction, chr);
        enrich_unit(
            &mut s,
            guid,
            &names,
            store,
            rx.factions.as_deref(),
            self_store,
        );
        Some((guid, s))
    });
    // The hovered GAMEOBJECT, when it is the nearer pick (the click router's own arbitration).
    // Deliberately NOT gated on the highlightable predicate (0558/0559): the mouseover publisher
    // `0x492890` dispatches the GO tooltip builder `0x52aa20`
    // by object KIND on both branches; highlightable is never read on the tooltip path (it gates
    // the cursor and the click only). So a GENERIC(5) signpost, a pre-quest INTERACT_COND chest,
    // and an IN_USE object all show the gold name plate while showing NO interact cursor — 0466's
    // "no cursor AND no tooltip" coupling was the regression. Transports never reach here — they
    // are excluded from the pick set itself (0466's correct half).
    // "Nothing else was picked, or the GameObject is the nearer pick." **A hovered corpse counts
    // as something else picked** (decision 1723) — it has no tooltip of its own yet, and the
    // `unit.is_none()` short-circuit would otherwise hand a farther GameObject the gold plate the
    // moment the ray landed on a body instead of a unit.
    let go = hovered_go.target.zip(hovered_go.guid).filter(|_| {
        (unit.is_none() && hovered.corpse.is_none()) || go_is_nearest(&hovered, &hovered_go)
    });

    if let Some((guid, state)) = unit.filter(|_| go.is_none()) {
        // Push first (the engine's builder + the recolor's UnitReaction read the token), then
        // rebuild on a hover-target change OR when a LINE-affecting field changes under the
        // same hover — the late-arriving name/creature-info case: the first render often
        // precedes the SMSG_NAME_QUERY/CREATURE_QUERY answers, and a once-per-guid render
        // would keep the stale (even empty) lines for the whole hover. Health/power stay OUT
        // of the key: the byte law's watcher drives the BAR without a line rebuild.
        let key = lines_view(&state);
        script.set_unit("mouseover", Some(state));
        if *last != LastHover::Unit(guid) || last_lines.as_ref() != Some(&key) {
            script.world_tooltip_unit("mouseover");
            *last = LastHover::Unit(guid);
            *last_lines = Some(key);
        }
        return;
    }
    // The **corpse plate** — "Corpse of <owner>" (the reference's own builder `0x52aef0`). Ahead
    // of the GameObject arm because the corpse is the nearer pick whenever `go` came back empty.
    //
    // It is a *name plate and nothing else*: no health bar, no reaction recolour, no level or
    // creature-type line — a corpse is not a unit, and the publisher deliberately fires **no
    // event** for it, so it never becomes the Lua `mouseover` unit token either (`UnitName
    // ("mouseover")` on a body is nil in the reference, and stays nil here). Corner-seated like a
    // unit's, not cursor-seated like a signpost's.
    if let Some((entity, guid)) = hovered
        .corpse
        .zip(hovered.corpse_guid)
        .filter(|_| go.is_none())
    {
        let Some(store) = stores.get(entity).ok() else {
            return;
        };
        // A bone pile with nothing to take publishes no mouseover at all — no plate, no brighten.
        if !crate::target::corpse_mouseover_eligible(store) {
            if !matches!(*last, LastHover::None) {
                script.world_tooltip_fade();
                *last = LastHover::None;
            }
            return;
        }
        if *last == LastHover::Corpse(guid) {
            return; // already showing; nothing re-seats a corner plate
        }
        // The owner's name is the ask-once cache like any other player name — a corpse carries
        // `CORPSE_FIELD_OWNER`, never a name. Until the answer lands there is no plate to show
        // (`last` stays `None`, so it appears the moment the name arrives).
        let Some(owner) = store.0.corpse_owner() else {
            return;
        };
        let Some(name) = names.resolve(owner, &commands).map(str::to_string) else {
            return;
        };
        // `CORPSE_TOOLTIP = "Corpse of %s"` — the builder's own key (`0x52aef0`). No string, no
        // plate, which is the reference's data-suppression face.
        let Some(plate) = keyed(
            &|key: &str| benilla_ui::strings::global(script.lua(), key),
            "CORPSE_TOOLTIP",
            &[Arg::S(&name)],
        ) else {
            return;
        };
        script.world_tooltip_gameobject(&plate, &[], None);
        *last = LastHover::Corpse(guid);
        return;
    }
    if let Some((entity, guid)) = go {
        // Which arm of the anchor fork this object takes (decision 0766). The reference asks the
        // object's own `[obj->vtbl+0x5c]`; what selects it is not pinned, so we key on the one
        // distinction the director's two reference observations agree on — a **GENERIC(5)**
        // signpost follows the cursor, an interactable GameObject sits in the corner.
        //
        // 0766 keyed this on "is it GENERIC(5)" and said plainly that it was a proxy: the real
        // client asks the object's own `[vtbl+0x5c]`, and what selects it was not then pinned.
        // **It is pinned now, and it is narrower** (decision 2259): `[vtbl+0x5c]` is `0x5f8630`,
        // whose body is `template.data[0x621b00(type, semantic 0x13)] != 0`, and key `0x13`
        // resolves for exactly ONE of the 31 GO types — GENERIC(5), at `data[0]`. Every other type
        // gets `-1` back and `0x5f8150`'s unsigned bound turns that into FALSE.
        //
        // So the cursor arm is **GENERIC with `data[0]` set**, and a GENERIC with it clear is
        // corner-seated like everything else. That also settles 0766's named divergence: the three
        // always-eligible types (SPELL_FOCUS 8 / DUEL_ARBITER 16 / FISHINGHOLE 25) carry no key
        // `0x13`, so all three are corner-seated — 0766's "GENERIC" reading wins over its "not
        // interactable" one.
        //
        // **`data[0]` is not `data[1]`.** The neighbouring slot is the mouseover-ELIGIBILITY column
        // (0762, semantic `0x12`, `0x5f4830`), and the two answer different questions: `data[1]`
        // says whether the object is hoverable at all, `data[0]` only says *where its plate sits*.
        // 342 of the 447 type-5 entries in the reference's own `gameobjectcache.wdb` carry both;
        // the objects that differ are hoverable and corner-seated, not silent.
        let cursor_seated = stores.get(entity).map(|s| s.0.gameobject_type_id())
            == Ok(GO_TYPE_GENERIC)
            && go_inputs
                .templates
                .get(guid)
                .is_some_and(|t| t.floating_tooltip);
        // Window px → the VM's y-up 768-virtual units (÷s, the input seam's own conversion) —
        // the anchor this point seats is resolved in UI units, so a raw-px point lands the
        // plate (s−1)× the cursor's distance from the bottom-left corner away from it.
        let cursor_ui = cursor_seated
            .then(|| {
                window.iter().next().and_then(|w| {
                    let s = crate::ui_script::seam_scale(w.height(), ui_scale.0);
                    // The headless probe's aim stands in for a cursor the window does not have
                    // (2250), so an automated run can carry a GENERIC plate — which is
                    // cursor-seated — all the way to the screen. A person's pointer always wins.
                    w.cursor_position()
                        .or_else(crate::target::hover_probe_point)
                        .map(|c| (c.x / s, (w.height() - c.y) / s))
                })
            })
            .flatten();
        if *last == LastHover::Go(guid) {
            if crate::target::hover_probe_armed() {
                let line = format!(
                    "held Go({guid:#x}) — plate up {}",
                    script.world_tooltip_up()
                );
                if *trace != line {
                    info!("hover probe/tooltip: {line}");
                    *trace = line;
                }
            }
            // The cursor arm follows the pointer; the corner arm has nothing to re-seat.
            if let Some((x, y)) = cursor_ui {
                script.world_tooltip_move(x, y);
            }
            return;
        }
        if cursor_seated && cursor_ui.is_none() {
            return; // cursor off-window: nothing to seat the pointer-anchored plate against
        }
        if crate::target::hover_probe_armed() {
            info!(
                "hover probe/tooltip: guid {guid:#x} cursor_seated {cursor_seated} cursor_ui \
                 {cursor_ui:?} template {:?}",
                go_inputs.templates.get(guid).map(|t| t.name.clone()),
            );
        }
        let Some(template) = go_inputs.templates.get(guid).cloned() else {
            // Template in flight: ask once and retry next frame (`last` stays, so the show
            // fires the moment the name lands).
            go_inputs.templates.request(guid, &commands);
            return;
        };
        // The lock lines, transcribed from the builder `0x52aa20` (decision 0756). Two blocks, in
        // the binary's order, and both are narrower than the sweep we used to print:
        //
        //  A) **"Locked"** — emitted iff `GAMEOBJECT_FLAGS & GO_FLAG_LOCKED` (`0x52aae5`:
        //     `shr 1; test dl,1`). That flag gates this line and nothing else.
        //  B) **ONE requirement line, from Lock.dbc SLOT 0 ONLY** (`[lockRow+4]` / `[lockRow+0x24]`
        //     — the builder never walks the other seven), and only when slot 0 passes the same
        //     per-slot Action gate the resolver uses (`0x52ab7e` → `0x5f81d0`, decision 0752):
        //       · KEY → **white** `LOCKED_WITH_ITEM` "Requires <item>" (`0x854988`; `0x52acd9`
        //         pushes `0xc0cf60` = white)
        //       · SKILL, opener unknown **and** the object is flag-locked → **nothing at all**
        //         (`0x52abf7: jne done`) — a padlocked door says "Locked" and names its key, never
        //         its lockpicking rank
        //       · SKILL, opener unknown, not flagged → **red** "Requires <word>" (the herb node)
        //
        // That is why the reference shows *"Locked" + "Requires Key to Searing Gorge"* on the
        // Searing Gorge door and not the `Requires Lockpicking (225)` line we used to add: the
        // Pick Lock requirement is that lock's slot **1**, and the builder never looks past 0.
        let go_store = stores.get(entity).ok();
        let flags = go_store.map_or(0, |s| s.0.gameobject_flags());
        let flag_locked = flags & GO_FLAG_LOCKED != 0;
        let state = go_store.map_or(benilla_formats::GO_STATE_ACTIVE, |s| {
            crate::go_anim::go_state(anims.get(entity).ok(), s)
        });
        let slots = go_inputs
            .locks
            .as_ref()
            .filter(|_| template.lock_id != 0)
            .and_then(|l| l.0.slots(template.lock_id));
        let mut lines: Vec<(String, TooltipTint)> = Vec::new();
        // The lock lines' string table, taken for the length of the build. Every one of them is a
        // key whose enUS value is "Requires %s" and whose identity only the binary settles.
        let go_get = |key: &str| benilla_ui::strings::global(script.lua(), key);
        if flag_locked {
            // **The "Locked" line is coloured by whether you can actually open it** (director-
            // reported: a door you hold the key for read red). The builder sets red `0xc0d3a8` as
            // the default (`0x52ab03`), calls the SAME resolver the click uses (`0x52ab14` →
            // `0x5f83d0`) and re-colours on its answer (`0x52ab19`-`0x52ab43`):
            //   · resolver says NO opener   -> keep red
            //   · a KEY item satisfied it   -> `0xc0d420` green (spell out-param set AND item
            //     out-param set: `0x52ab29 jne` takes the green arm)
            //   · no lock requirement at all-> the same green (`0x52ab22 je`)
            //   · a SKILL opener satisfied it -> the difficulty ramp `0x529fa0` — NOT modelled;
            //     see this line's follow-up note in decision 0770. Green is its second rung, so a
            //     comfortably-skilled opener already reads correctly; a marginal one reads too
            //     green rather than yellow/orange.
            let facts = crate::target::lock::go_facts(go_store.map(|s| (s, state)));
            let mut matched = None;
            let outcome = slots.map(|slots| {
                crate::target::lock::resolve_lock(
                    slots,
                    &player_actions.spells,
                    go_inputs.spells.as_deref(),
                    go_inputs.skill_lines.as_ref().map(|s| &s.catalog),
                    self_store,
                    &go_inputs.objects,
                    facts,
                    &mut matched,
                )
            });
            if let Some(text) = keyed(&go_get, "LOCKED", &[]) {
                lines.push((text, locked_line_tint(outcome)));
            }
        }
        if let Some(slot0) = slots
            .map(|s| s[0])
            .filter(|s| s.available(state, flag_locked))
        {
            match slot0.key_type {
                benilla_formats::LOCK_KEY_ITEM => {
                    if let Some(t) = go_inputs.items.template(slot0.index, 0, &commands) {
                        // `LOCKED_WITH_ITEM`, resolved at `0x52acb8` — one of eleven 1.12 keys
                        // whose enUS value is exactly "Requires %s", and the only one this arm
                        // reaches.
                        if let Some(text) = keyed(&go_get, "LOCKED_WITH_ITEM", &[Arg::S(&t.name)]) {
                            lines.push((text, TooltipTint::White));
                        }
                    }
                }
                // The opener-*known* arm additionally wants the reference's skill-margin colour
                // ramp (`0x529fa0`) — now pinned (decision 0770) but not modelled here; what we
                // model is the unknown arm, which is what a hovering player almost always is. A
                // flagged object stays silent there, exactly as the binary does.
                benilla_formats::LOCK_KEY_SKILL if !flag_locked => {
                    // **`LOCKED_WITH_SPELL`, not `LOCKED_WITH_SPELL_KNOWN`** — the two are the
                    // same sentence in enUS and the byte test between them is a pure MEMBERSHIP
                    // one: `0x5f83d0` writes a non-zero spell id into its first out-param iff the
                    // player knows *any* spell that opens this LockType, and `0x52abcb` branches
                    // on that. This is the not-known arm (the flag-clear leg at `0x52ac04`);
                    // skill *sufficiency* never changes the key, only the colour.
                    //
                    // The `%s` is `LockType.dbc`'s own localized `Name` — "Pick Lock", not
                    // "Lockpicking", which is what a hand-typed table here used to say.
                    let word = go_inputs
                        .lock_types
                        .as_deref()
                        .and_then(|c| c.0.name(slot0.index));
                    if let Some(text) =
                        word.and_then(|w| keyed(&go_get, "LOCKED_WITH_SPELL", &[Arg::S(w)]))
                    {
                        lines.push((text, TooltipTint::Red));
                    }
                }
                _ => {}
            }
        }
        script.world_tooltip_gameobject(&template.name, &lines, cursor_ui);
        *last = LastHover::Go(guid);
        return;
    }
    if !matches!(*last, LastHover::None) {
        // Hover lost: arm the fade (the byte law's timestamped fade, never an instant hide).
        // The "mouseover" state stays until the next hover overwrites it, so the fading
        // lines/bar keep their last content.
        script.world_tooltip_fade();
        *last = LastHover::None;
    }
}

#[cfg(test)]
mod tests;
