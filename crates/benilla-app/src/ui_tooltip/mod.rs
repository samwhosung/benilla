//! The world-mouseover tooltip and the spell tooltip views. The world tooltip rebuilds once per
//! hover-target change and a lost hover arms a fade; the pick is [`Hovered`] or
//! [`HoveredObject`], arbitrated by [`go_is_nearest`] as the click router does.

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

/// The player-dependent inputs of a spell view: the worn set (`0x5f0c50`), the bags, the form and
/// the bind point `$z` names.
struct ViewCtx<'a, 'w, 's> {
    home_area: Option<&'a str>,
    form: u8,
    store: Option<&'a ObjectStore>,
    /// The caster's `UNIT_FIELD_COMBATREACH`; 1.5 is the descriptor default.
    combat_reach: f32,
    /// The auto-attack target's reach, which `0x6e3480` reads itself (`[caster+0xc48]`); with
    /// none, the caster's reach counts twice.
    attack_target_reach: Option<f32>,
    /// The object index the worn-item search and each reagent's carried count resolve through.
    objects: &'a Objects<'w, 's>,
    items: &'a mut Items,
    commands: &'a NetCommands,
    sub_classes: Option<&'a benilla_formats::ItemSubClassCatalog>,
    /// The talent spell-modifier tables: the cost cell shows the modified cost (`power_cost`).
    spell_mods: &'a crate::spell::SpellModifiers,
    /// The VM's `GlobalStrings.lua`, where the keyed cells resolve; `text` is the `%d`-filling form
    /// the `$` tokens take.
    get: &'a dyn Fn(&str) -> Option<String>,
    text: &'a dyn Fn(&str, &[i64]) -> Option<String>,
}

/// Fill a key's template from the VM's `GlobalStrings.lua`; no string shows nothing.
fn keyed(get: &dyn Fn(&str) -> Option<String>, key: &str, args: &[Arg<'_>]) -> Option<String> {
    let text = benilla_ui::strings::fill(&get(key)?, args);
    (!text.is_empty()).then_some(text)
}

/// Build one spell's tooltip view, every cell resolved here where the catalogs live. The range,
/// cast, cooldown, form and item cells fill their `GlobalStrings.lua` keys; the cost, reagents and
/// chance cells are composed in English, where the reference fills `MANA_COST` and its kin,
/// `SPELL_REAGENTS` and `CHANCE_TO_*`.
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
    // The cost cell (`0x52e8ad`): the resolved `power_cost`, named by power type (keys at
    // `0x85416c`) and Health for a type outside 0..=4 (`0x52e8fc`: Bloodrage's -2), never a
    // percentage. Rage shows wire ÷ 10, a per-second column adds `_PER_TIME`, and no cost at all
    // leaves the cell empty.
    let resolved_cost = vctx.store.map_or(d.mana_cost, |s| {
        crate::spell::usable::power_cost(d, s, vctx.spell_mods)
    });
    let cost = {
        // The shared per-type divisor (`0x6e7130`), never a local `power_type == 1` test.
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
    // The range cell: two attribute gates, `GetMinMaxRange` (`0x6e3480`, `0x52e9c2`), then
    // `max <= 0`. There is no melee wording: the melee row (flags bit 0, row 2 only) resolves to
    // `max(reach + casterReach + 1.3333334, 5.0)` and prints like any other, usually "5 yd range".
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
            // `SPELL_RANGE`'s hole is a string: the number is composed first, `"%d"` or `"%d-%d"`
            // (`0x854fb4`, Charge's 8-25), each a `fistp` conversion that rounds to nearest.
            let yd = |v: f32| f64::from(v).round_ties_even() as i64;
            let yards = if min > 0.0 {
                format!("{}-{}", yd(min), yd(max))
            } else {
                format!("{}", yd(max))
            };
            keyed(vctx.get, "SPELL_RANGE", &[Arg::S(&yards)])
        })
        .flatten();
    // The cast line: gate `0x52eb15` also omits it for a TRADE_SKILL or ATTACK `Effect[0]`; then
    // the ladder `0x52eb45`-`0x52ec90`, where a negative base is "Instant cast" (`0x52ebce`) and a
    // zero one is "Instant cast" only for a mana spell with a cost (`0x52ec4b`).
    let cast_time = if d.tooltip_omits_cast_line() {
        None
    } else {
        let base = spells
            .cast_times
            .get(d.casting_time_index)
            .map(|c| c.base_ms as i32)
            .unwrap_or(0);
        // `%.3g` templates: the seconds go over as a real.
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
                "SPELL_CAST_TIME_INSTANT_NO_MANA"
            };
            keyed(vctx.get, key, &[])
        }
    };
    // The larger recovery column (`0x52eada`): Charge's 15 s is its category recovery.
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
    // The required-form line (`0x52f10a`-`0x52f2ae`). With `AttributesEx2` bit 19 set the mask is
    // permissive (Inner Fire in Shadowform), and the line is skipped (`0x52f115`).
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
    // The required-item line (`0x52eea7`-`0x52f10a`): a mask is named whole by
    // `ItemSubClassMask.dbc` before its subclasses are joined; an empty mask or class < 0 skips it.
    let requires_item = (d.targets & TARGET_ITEM == 0 && d.equipped_item_class >= 0)
        .then(|| {
            vctx.sub_classes?
                .requirement_name(d.equipped_item_class as u32, d.equipped_item_subclass_mask)
        })
        .flatten()
        .and_then(|name| keyed(vctx.get, "SPELL_EQUIPPED_ITEM", &[Arg::S(&name)]));
    let chance = chance_line(d, vctx.store);
    let item_met = vctx.store.is_none_or(|s| {
        crate::spell::usable::equipped_item_fits(d, s, vctx.objects, vctx.items, vctx.commands)
    });
    // Reagents (`SPELL_REAGENTS`, `0x854e54`), red when short; one whose template has not
    // streamed is absent until `feed_spell_tooltips` re-pushes on its arrival.
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
        // The aura variant's right column (`0x52f8e5`), gated on `SpellDispelType.dbc` `[+0x28]`.
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

/// `Targets` bit `0x10` (name inferred): set, the required-item line is skipped (`0x52eeaa`).
const TARGET_ITEM: u32 = 0x10;

/// The four `Effect[0]` values that select a chance-to-X line (jump table `0x52f7c4`).
const EFFECT_DODGE: u32 = 20;
const EFFECT_PARRY: u32 = 22;
const EFFECT_BLOCK: u32 = 23;
const EFFECT_ATTACK: u32 = 78;

/// The chance-to-X line (`0x52f5b1`-`0x52f697`): ATTACK skips the passive gate that dodge, parry
/// and block need. The wire's values are already percents.
fn chance_line(d: &benilla_formats::SpellDisplay, store: Option<&ObjectStore>) -> Option<String> {
    let (label, percentage) = match d.effects[0] {
        EFFECT_ATTACK => ("crit", store?.0.player_crit_percentage()?),
        EFFECT_DODGE if d.passive => ("dodge", store?.0.player_dodge_percentage()?),
        EFFECT_PARRY if d.passive => ("parry", store?.0.player_parry_percentage()?),
        EFFECT_BLOCK if d.passive => ("block", store?.0.player_block_percentage()?),
        _ => return None,
    };
    // The enUS text of the reference's `CHANCE_TO_*` keys (`%.2f%% chance to crit`).
    Some(format!("{percentage:.2}% chance to {label}"))
}

/// What a pushed view snapshots at build time; a change to any of it re-pushes every view. The
/// reference rebuilds on every hover and keeps no such snapshot.
#[derive(Default)]
struct SpellFeedMemory {
    pushed: std::collections::HashSet<u32>,
    /// The bind point `$z` names.
    home: Option<String>,
    /// The form the required-form line's colour follows (`0x52f1e3`).
    form: Option<u8>,
    /// The 19 worn-slot guids the required-item line's colour follows (`0x5f0c50`).
    worn: Option<[u64; 19]>,
    /// The block, dodge, parry and crit percentages as bit patterns (`0x52f5b1`).
    avoidance: Option<[u32; 4]>,
    /// The caster's and its auto-attack target's reach as bit patterns.
    combat_reach: Option<(Option<u32>, Option<u32>)>,
    /// Per reagent on show, `(owned count, name resolved)` (the inline red, `0x854120`).
    reagents: std::collections::BTreeMap<u32, (u32, bool)>,
}

/// Push a view for every spell the UI can hover (the book, the class's talent ranks, the auras)
/// before it is hovered, as the reference reads them all locally; an ask for any other id too.
fn feed_spell_tooltips(
    script: Option<NonSendMut<UiScript>>,
    actions: Option<Res<PlayerActions>>,
    spells: Option<Res<Spells>>,
    talents: Option<Res<crate::ui_talent::Talents>>,
    auras: Option<Res<crate::ui_aura::PlayerAuraCache>>,
    selection: Res<crate::target::Selection>,
    stores: Query<&ObjectStore>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    // The auto-attack target, the melee range cell's second reach.
    engaged_q: Query<&crate::creature_anim::Engaged, With<SelfPlayer>>,
    objects: Objects,
    home_bind: Option<Res<crate::net::HomeBind>>,
    area_names: Option<Res<crate::ui_quest_log::QuestHeaderNamesRes>>,
    mut items: ResMut<Items>,
    // One tuple param, under Bevy's 16-param ceiling.
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
                        .filter(|s| *s != 0 && !memory.pushed.contains(s)),
                );
            }
        }
    }
    if let Some(auras) = auras.as_deref() {
        wanted.extend(auras.spell_ids().filter(|s| !memory.pushed.contains(s)));
    }
    // `SetTrackingSpell`: the display cache leaves the tracking aura out, so read the raw auras.
    if let Ok(store) = self_q.single() {
        wanted.extend(
            store
                .0
                .unit_auras()
                .map(|a| a.spell_id)
                .filter(|s| !memory.pushed.contains(s)),
        );
    }
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
    // A bind-point change re-substitutes every view's `$z` (Astral Recall).
    if memory.home != home_area {
        memory.home = home_area.clone();
        wanted.extend(memory.pushed.drain());
    }
    // So does a form change: the required-form line's colour follows the form (`0x52f1e3`).
    let form = self_q
        .single()
        .map(|s| s.0.unit_shapeshift_form())
        .unwrap_or(0);
    if memory.form != Some(form) {
        memory.form = Some(form);
        wanted.extend(memory.pushed.drain());
    }
    let self_store = self_q.single().ok();
    // So do the worn set and, below, the reagents on show.
    let worn =
        self_store.map(|s| std::array::from_fn(|i| s.0.player_inv_slot(i as u8).unwrap_or(0)));
    if memory.worn != worn {
        memory.worn = worn;
        wanted.extend(memory.pushed.drain());
    }
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
    // So does a spell-modifier change: the cost cell resolves through the talent tables.
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
    // Build, then push: the build's borrow of the VM's strings ends before the store is written.
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
                // Watch this spell's reagents, seeded with the state the view was built against.
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

/// What the world tooltip was last built for; the reference rebuilds once per hover-target change.
#[derive(Default, PartialEq, Clone, Copy)]
enum LastHover {
    #[default]
    None,
    Unit(u64),
    Go(u64),
    /// A hovered corpse object, keyed on the corpse's guid.
    Corpse(u64),
}

/// The world-hover driver's memory, one `SystemParam` because the driver is at Bevy's 16-param
/// ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct HoverMemo<'s> {
    /// Which world plate is up. On an unchanged mouseover the reference makes no call (`0x482090`
    /// returns at `0x4820b5`), so a plate Lua takes mid-hover stays gone.
    last: Local<'s, crate::ui_script::VmMemo<LastHover>>,
    /// The line-affecting fields the current unit plate was built from, so a late-arriving name or
    /// creature-info rebuilds it under the same hover.
    last_lines: Local<'s, crate::ui_script::VmMemo<Option<UnitState>>>,
    /// The hover probe's last trace line, so a stationary probe logs it once.
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

/// The "Locked" line's colour (`0x52ab03`-`0x52ab43`): red `0xc0d3a8` unless the resolver finds an
/// opener, then green `0xc0d420`, for a key (`0x52ab29`) or no requirement (`0x52ab22`); `None`, a
/// flag-locked object with no `Lock.dbc` row, is no requirement. A skill opener takes the
/// reference's difficulty ramp `0x529fa0` (grey, green, yellow, orange, red at +0/25/50/100);
/// benilla shows its green, since the resolver discards the margin.
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
    // Seats the cursor-anchored GameObject plate.
    window: Query<&Window, With<PrimaryWindow>>,
    stores: Query<&ObjectStore>,
    // The stored GAMEOBJECT_STATE the lock lines' Action gate reads.
    anims: Query<&crate::go_anim::GoAnim>,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    rx: crate::target::ReactionInputs,
    // The lock chain's data, shared with the click router (`target::lock`) so hover and click
    // agree on whether a lock can be opened.
    go_inputs: crate::target::lock::GoLockInputs,
    // The known-spell set the resolver's SKILL arm scans.
    player_actions: Res<crate::ui_action::PlayerActions>,
    // The cursor seat crosses the VM seam: the anchor below is UI units, not px.
    ui_scale: Res<crate::ui_script::UiScaleCvar>,
    mut memo: HoverMemo,
    // `ChrClasses.dbc` field 16, `UnitHasRelicSlot`'s input; without it no class has a relic slot.
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
    // The hovered GameObject, when nothing else was picked or it is nearer. Not gated on
    // highlightability: the publisher `0x492890` sends every object to the builder by kind, so a
    // signpost or an in-use object shows its plate with no interact cursor.
    let go = hovered_go.target.zip(hovered_go.guid).filter(|_| {
        (unit.is_none() && hovered.corpse.is_none()) || go_is_nearest(&hovered, &hovered_go)
    });

    if let Some((guid, state)) = unit.filter(|_| go.is_none()) {
        // Push first: the builder reads the token. Rebuild on a new target or a late name or
        // creature answer; health and power stay out of the key, since the watcher drives the bar.
        let key = lines_view(&state);
        script.set_unit("mouseover", Some(state));
        if *last != LastHover::Unit(guid) || last_lines.as_ref() != Some(&key) {
            script.world_tooltip_unit("mouseover");
            *last = LastHover::Unit(guid);
            *last_lines = Some(key);
        }
        return;
    }
    // The corpse plate, "Corpse of <owner>" (`0x52aef0`): a name and nothing else, corner-seated.
    // The publisher fires no event for a corpse, so `UnitName("mouseover")` on one stays nil.
    if let Some((entity, guid)) = hovered
        .corpse
        .zip(hovered.corpse_guid)
        .filter(|_| go.is_none())
    {
        let Some(store) = stores.get(entity).ok() else {
            return;
        };
        // A bone pile with nothing to take publishes no mouseover: no plate, no brighten.
        if !crate::target::corpse_mouseover_eligible(store) {
            if !matches!(*last, LastHover::None) {
                script.world_tooltip_fade();
                *last = LastHover::None;
            }
            return;
        }
        if *last == LastHover::Corpse(guid) {
            return; // already up; a corner plate never re-seats
        }
        // A corpse carries `CORPSE_FIELD_OWNER`, not a name; the plate waits for the name query.
        let Some(owner) = store.0.corpse_owner() else {
            return;
        };
        let Some(name) = names.resolve(owner, &commands).map(str::to_string) else {
            return;
        };
        // `CORPSE_TOOLTIP`, the builder's key; no string, no plate.
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
        // The plate follows the cursor iff `[vtbl+0x5c]` (`0x5f8630`) finds
        // `data[0x621b00(type, 0x13)]` set, and semantic `0x13` exists only for GENERIC(5), at
        // `data[0]`. `data[1]` (`0x5f4830`) is whether it can be hovered, not where it sits.
        let cursor_seated = stores.get(entity).map(|s| s.0.gameobject_type_id())
            == Ok(GO_TYPE_GENERIC)
            && go_inputs
                .templates
                .get(guid)
                .is_some_and(|t| t.floating_tooltip);
        // Window px to the VM's y-up UI units, as the input seam converts.
        let cursor_ui = cursor_seated
            .then(|| {
                window.iter().next().and_then(|w| {
                    let s = crate::ui_script::seam_scale(w.height(), ui_scale.0);
                    // The hover probe's aim stands in for a missing cursor; a real pointer wins.
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
            return; // cursor off-window: nothing to seat the plate against
        }
        if crate::target::hover_probe_armed() {
            info!(
                "hover probe/tooltip: guid {guid:#x} cursor_seated {cursor_seated} cursor_ui \
                 {cursor_ui:?} template {:?}",
                go_inputs.templates.get(guid).map(|t| t.name.clone()),
            );
        }
        let Some(template) = go_inputs.templates.get(guid).cloned() else {
            // Template in flight: ask once, and show it when it lands.
            go_inputs.templates.request(guid, &commands);
            return;
        };
        // The builder's lock lines (`0x52aa20`): "Locked" iff `GO_FLAG_LOCKED` (`0x52aae5`), then
        // one line from `Lock.dbc` slot 0 only, if it passes the Action gate (`0x52ab7e`): a key in
        // white (`0x52acd9`), an unknown skill in red but only on an unflagged object (`0x52abf7`),
        // so a locked door names its key, never its lockpicking.
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
        // The lock lines' keys all read "Requires %s" in enUS; only the binary tells them apart.
        let go_get = |key: &str| benilla_ui::strings::global(script.lua(), key);
        if flag_locked {
            // Coloured by whether the player can open it: the builder asks the resolver the click
            // uses (`0x52ab14` → `0x5f83d0`), and `locked_line_tint` maps the answer.
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
                        // `LOCKED_WITH_ITEM`, at `0x52acb8`.
                        if let Some(text) = keyed(&go_get, "LOCKED_WITH_ITEM", &[Arg::S(&t.name)]) {
                            lines.push((text, TooltipTint::White));
                        }
                    }
                }
                // Every skill lock takes the opener-unknown arm, silent on a flag-locked object;
                // the known arm, `LOCKED_WITH_SPELL_KNOWN` in the margin ramp's colour
                // (`0x529fa0`), is not built.
                benilla_formats::LOCK_KEY_SKILL if !flag_locked => {
                    // `LOCKED_WITH_SPELL` (`0x52ac04`), named by `LockType.dbc` ("Pick Lock"); the
                    // reference takes the known arm iff any known spell opens it (`0x52abcb`).
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
        // Hover lost: arm the fade. The `mouseover` state stays, so the fading plate keeps it.
        script.world_tooltip_fade();
        *last = LastHover::None;
    }
}

#[cfg(test)]
mod tests;
