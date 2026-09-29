//! The spell tooltip views: the builder `0x52e610`'s cells resolved here where the catalogs live,
//! and the feed that pushes a view for every spell the UI can hover before it is hovered. The
//! reference builds at the hover and keeps nothing; the feed keeps what each pushed view read
//! ([`super::spell_deps`]) and builds again only the views a change of the player's state reaches.

use std::cell::RefCell;
use std::collections::HashMap;

use bevy::prelude::*;

use benilla_ui::script::UiScript;
use benilla_ui::strings::Arg;

use super::keyed;
use super::spell_deps::{Chance, Changes, Deps, Reagents, Seen, UnitField};
use crate::items::Items;
use crate::net::{NetCommands, ObjectStore, Objects, SelfPlayer};
use crate::spell::usable::{self, CostBasis};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_items::{count_of, InventoryScope};
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

impl ViewCaster<'_> {
    /// The pet's `[vtbl+0xa8]`, `CGUnit`'s `0x60cd80`: `UNIT_FIELD_LEVEL × 5`, which
    /// [`benilla_formats::SpellDisplay::skill_level`] caps at `maxLevel × 5` and divides by 5 as
    /// `0x60cdb2`-`0x60cdb9` and `0x6e3195` do. No unit reads 0 (`0x6e31a4`).
    fn pet_skill_value(pet: Option<&benilla_protocol::ObjectFields>) -> u32 {
        pet.and_then(|p| p.unit_level())
            .unwrap_or(0)
            .saturating_mul(5)
    }
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
    /// The caster's `UNIT_FIELD_COMBATREACH`; 1.5 is the descriptor default.
    pub(super) combat_reach: f32,
    /// The caster's auto-attack target's reach, which `0x6e3480` looks up itself
    /// (`[caster+0xc48]`) for its melee arm alone; with none, the caster's reach counts twice.
    pub(super) attack_target_reach: Option<f32>,
    /// The object index the worn-item search and each reagent's carried count resolve through.
    pub(super) objects: &'a Objects<'w, 's>,
    pub(super) items: &'a mut Items,
    pub(super) commands: &'a NetCommands,
    pub(super) sub_classes: Option<&'a benilla_formats::ItemSubClassCatalog>,
    /// The spell-to-line hop the `$`-tokens' skill level reads ([`crate::spell::spell_skill_value`]).
    pub(super) skill_lines: Option<&'a benilla_formats::SkillLineCatalog>,
    /// The talent spell-modifier tables: the cost cell shows the modified cost (`power_cost`).
    pub(super) spell_mods: &'a crate::spell::SpellModifiers,
    /// The VM's `GlobalStrings.lua`, where the keyed cells and the `$` tokens' templates resolve.
    pub(super) get: &'a dyn Fn(&str) -> Option<String>,
}

/// Build one spell's tooltip view, every cell resolved here where the catalogs live, and what it
/// read ([`Deps`]). The range, cast, cooldown, form and item cells fill their `GlobalStrings.lua`
/// keys; the cost, reagents and chance cells are composed in English, where the reference fills
/// `MANA_COST` and its kin, `SPELL_REAGENTS` and `CHANCE_TO_*`.
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
        ViewCaster::Pet(pet) => ViewCaster::pet_skill_value(pet),
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
    // `0x6e3130` for this spell: the level the cost's and the cast time's per-level terms read.
    let pet_level = |pet| d.skill_level(ViewCaster::pet_skill_value(pet));
    let ctx = benilla_formats::TokenContext {
        durations: &spells.durations,
        radii: &spells.radii,
        ranges: Some(&spells.ranges),
        skill: &skill,
        lookup: &lookup,
        mods: Some(vctx.spell_mods),
        unmodified_points: false,
        home_area: &home_area,
        global: vctx.get,
        printf: &crate::ui_script::token_printf,
    };
    // The aura tooltip `0x52f880` expands the aura text with the points' modifiers off (`52f940`).
    let aura_ctx = benilla_formats::TokenContext {
        unmodified_points: true,
        ..ctx
    };
    // The cost cell (`0x52e8ad`): the resolved `power_cost`, named by power type (keys at
    // `0x85416c`) and Health for a type outside 0..=4 (`0x52e8fc`: Bloodrage's -2), never a
    // percentage. Rage shows wire ÷ 10, a per-second column adds `_PER_TIME`, and no cost at all
    // leaves the cell empty. A pet with no unit costs -1 (`0x6e3233`), which shows as none.
    let resolved_cost = match caster {
        ViewCaster::Player => vctx
            .store
            .map_or(d.mana_cost, |s| usable::power_cost(d, s, vctx.spell_mods)),
        ViewCaster::Pet(pet) => pet.map_or(0, |p| {
            usable::power_cost_at(d, p, pet_level(Some(p)), vctx.spell_mods)
        }),
    };
    // The player's cost reads its level, and the field a percentage scales by; the pet's reads the
    // pet's own, which are pet inputs.
    if matches!(caster, ViewCaster::Player) {
        let mut deps = deps.borrow_mut();
        if usable::cost_reads_level(d) {
            deps.unit(UnitField::Level);
        }
        match usable::cost_basis(d) {
            CostBasis::None => {}
            CostBasis::BaseMana => deps.unit(UnitField::BaseMana),
            CostBasis::MaxHealth => deps.unit(UnitField::MaxHealth),
            CostBasis::MaxPower(ty) => deps.unit(UnitField::MaxPower(ty)),
        }
    }
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
            let row = spells.ranges.get(d.range_index);
            // The pet views read the pet's reaches, which are among the pet's own inputs.
            if matches!(caster, ViewCaster::Player)
                && benilla_formats::min_max_range_reads_reach(d, row)
            {
                deps.borrow_mut().reach = true;
            }
            // The call passes a null target (`0x52e9c2`): the auto-attack target is the melee arm's
            // own lookup, and the ranged arm, with no target, pads nothing.
            let targets = benilla_formats::RangeTargets {
                target: None,
                attack_target: vctx.attack_target_reach,
            };
            let (min, max) = vctx
                .spell_mods
                .min_max_range(d, row, vctx.combat_reach, targets)?;
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
    // the ladder `0x52eb45`-`0x52ec90` over `GetCastTime(1)` (`52eb4d`), unclamped, where a
    // negative time is "Instant cast" (`0x52ebce`) and a zero one is "Instant cast" only for a
    // mana spell with a cost (`0x52ec4b`).
    let cast_time = if d.tooltip_omits_cast_line() {
        None
    } else {
        // `GetCastTime` takes the selector too (`52eb45`), and its level term is `0x6e3130`'s. The
        // player's is its level only where the row scales by one.
        if matches!(caster, ViewCaster::Player)
            && spells
                .cast_times
                .get(d.casting_time_index)
                .is_some_and(|row| row.reads_caster_level())
        {
            deps.borrow_mut().unit(UnitField::Level);
        }
        let level = match caster {
            ViewCaster::Player => vctx.store.and_then(|s| s.0.unit_level()).unwrap_or(0),
            ViewCaster::Pet(pet) => pet_level(pet),
        };
        let base = spells.cast_time_unclamped_ms(d, level, vctx.spell_mods);
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
        .flatten()
        .and_then(|name| keyed(vctx.get, "SPELL_EQUIPPED_ITEM", &[Arg::S(&name)]));
    let chance = chance_line(d, vctx.store, &mut deps.borrow_mut());
    // The unit selector passes the item test outright (`0x52f07f`).
    if matches!(caster, ViewCaster::Player) {
        deps.borrow_mut().item_search(usable::worn_slots_read(d));
    }
    let item_met = matches!(caster, ViewCaster::Pet(_))
        || vctx.store.is_none_or(|s| {
            usable::equipped_item_fits(d, s, vctx.objects, vctx.items, vctx.commands)
        });
    // Reagents (`SPELL_REAGENTS`, `0x854e54`), red when short; one whose template has not
    // streamed is absent until `feed_spell_tooltips` re-pushes on its arrival.
    let reagents = {
        let mut parts: Vec<String> = Vec::new();
        for (entry, count) in d.reagents.iter().copied().filter(|&(e, _)| e != 0) {
            deps.borrow_mut().reagent(entry);
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
    let view = benilla_ui::script::SpellTooltipView {
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
            .map(|t| benilla_formats::substitute(t, d, &aura_ctx))
            .unwrap_or_default(),
    };
    Some((view, deps.into_inner()))
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
fn chance_line(
    d: &benilla_formats::SpellDisplay,
    store: Option<&ObjectStore>,
    deps: &mut Deps,
) -> Option<String> {
    let (which, label) = match d.effects[0] {
        EFFECT_ATTACK => (Chance::Crit, "crit"),
        EFFECT_DODGE if d.passive => (Chance::Dodge, "dodge"),
        EFFECT_PARRY if d.passive => (Chance::Parry, "parry"),
        EFFECT_BLOCK if d.passive => (Chance::Block, "block"),
        _ => return None,
    };
    deps.chance(which);
    // The enUS text of the reference's `CHANCE_TO_*` keys (`%.2f%% chance to crit`).
    Some(format!("{:.2}% chance to {label}", which.read(&store?.0)?))
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
    /// Queue for a rebuild the pushed views, the player's and the pet's, that read what
    /// `changes` moved.
    pub(super) fn requeue(
        &mut self,
        changes: &Changes,
        wanted: &mut Vec<u32>,
        wanted_pet: &mut Vec<u32>,
    ) {
        if changes.is_empty() {
            return;
        }
        for (pushed, queue) in [
            (&mut self.pushed, wanted),
            (&mut self.pet_pushed, wanted_pet),
        ] {
            pushed.retain(|&id, deps| {
                let hit = deps.hits(changes);
                if hit {
                    queue.push(id);
                }
                !hit
            });
        }
    }
}

/// What a reagent's line renders for `entry`: the carried count and whether its name has streamed.
pub(super) fn reagent_state(
    entry: u32,
    store: Option<&ObjectStore>,
    objects: &Objects,
    items: &mut Items,
    commands: &NetCommands,
) -> (u32, bool) {
    let named = items.template(entry, 0, commands).is_some();
    let owned = store.map_or(0, |s| {
        count_of(&s.0, objects, entry, InventoryScope::CARRIED)
    });
    (owned, named)
}

/// The pet unit's fields a pet view reads (`0x52e610` with the unit selector set): its level, the
/// cost bases `0x612c50` reads, its form, and its and its melee target's reach, as bit patterns.
#[derive(Clone, Copy, PartialEq, Default)]
pub(super) struct PetInputs {
    guid: Option<u64>,
    level: u32,
    base_mana: u32,
    max_health: u32,
    max_power: [u32; 5],
    form: u8,
    combat_reach: u32,
    attack_target_reach: Option<u32>,
}

impl PetInputs {
    pub(super) fn of(
        guid: Option<u64>,
        pet: Option<&benilla_protocol::ObjectFields>,
        attack_target_reach: Option<f32>,
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
            max_health: p.unit_max_health().unwrap_or(0),
            max_power: std::array::from_fn(|i| p.unit_max_power(i as u8).unwrap_or(0)),
            form: p.unit_shapeshift_form(),
            combat_reach: p.unit_combat_reach().to_bits(),
            attack_target_reach: attack_target_reach.map(f32::to_bits),
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
/// trainer's services, and what the VM holds: the pet's spells, the quest rewards, the craft's
/// subjects, every unit's auras and the tracking spell) before it is hovered, as the reference
/// reads them all locally; an ask for any other id too.
pub(super) fn feed_spell_tooltips(
    script: Option<NonSendMut<UiScript>>,
    actions: Option<Res<PlayerActions>>,
    spell_sources: SpellTooltipSources,
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    // The player's and the pet's auto-attack target, the melee range cell's second reach.
    engaged_q: Query<&crate::creature_anim::Engaged, With<SelfPlayer>>,
    engaged_units: Query<&crate::creature_anim::Engaged>,
    objects: Objects,
    home_bind: Option<Res<crate::net::HomeBind>>,
    area_names: Option<Res<crate::ui_quest_log::QuestHeaderNamesRes>>,
    mut items: ResMut<Items>,
    // One tuple param, under Bevy's 16-param ceiling.
    lookups: (
        Option<Res<crate::ui_items::ItemSubClasses>>,
        Res<crate::spell::SpellModifiers>,
        Option<Res<crate::ui_spellbook::SkillLines>>,
    ),
    commands: Res<NetCommands>,
    mut memory: Local<crate::ui_script::VmMemo<SpellFeedMemory>>,
) {
    let (sub_classes, spell_mods, skill_lines) = &lookups;
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
    // What the VM holds for a hover: pet spells, quest rewards, craft subjects, auras, tracking.
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
    let target_reach =
        |e: &crate::creature_anim::Engaged| objects.object(e.0).map(|f| f.unit_combat_reach());
    let attack_target_reach = engaged_q.single().ok().and_then(target_reach);
    // What moved since the last frame, which each pushed view is checked against: the bind
    // point (Astral Recall's `$z`), the form, the skills, what the equipped-item search learns of
    // each equipment slot and the disarm flag, the level, cost bases and ranged attack time, the
    // block, dodge, parry and crit percentages, and the two combat reaches.
    let seen = Seen::of(
        self_store,
        home_area.as_deref(),
        attack_target_reach,
        &objects,
        &items,
    );
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
        .map(|&entry| {
            (
                entry,
                reagent_state(entry, self_store, &objects, &mut items, &commands),
            )
        })
        .collect();
    changes = changes.with_reagents(&memory.reagents, &reagent_state);
    memory.reagents = reagent_state;
    memory.requeue(&changes, &mut wanted, &mut wanted_pet);
    // The pet's inputs: a new pet, or its level, bases, form or reach moving, rebuilds the pet
    // views alone.
    let pet_guid = self_store.and_then(|s| s.0.unit_pet_guid());
    let pet = pet_guid.and_then(|g| objects.object(g));
    let pet_target_reach = pet_guid
        .and_then(|g| objects.entity(g))
        .and_then(|e| engaged_units.get(e).ok())
        .and_then(target_reach);
    let pet_inputs = PetInputs::of(pet_guid, pet, pet_target_reach);
    if memory.pet != Some(pet_inputs) {
        memory.pet = Some(pet_inputs);
        wanted_pet.extend(memory.pet_pushed.drain().map(|(id, _)| id));
    }
    // The sources overlap (a self-buff is in the book and on the player): build each id once.
    wanted.sort_unstable();
    wanted.dedup();
    wanted_pet.sort_unstable();
    wanted_pet.dedup();
    // Build, then push: the build's borrow of the VM's strings ends before the store is written.
    let (built, built_pet) = {
        let get = |key: &str| benilla_ui::strings::global(script.lua(), key);
        let mut vctx = ViewCtx {
            home_area: home_area.as_deref(),
            form: self_store.map_or(0, |s| s.0.unit_shapeshift_form()),
            store: self_store,
            caster: ViewCaster::Player,
            combat_reach: self_store.map_or(1.5, |s| s.0.unit_combat_reach()),
            attack_target_reach,
            objects: &objects,
            items: &mut items,
            commands: &commands,
            sub_classes: sub_classes.as_deref().map(|c| &c.0),
            skill_lines: skill_lines.as_deref().map(|s| &s.catalog),
            spell_mods,
            get: &get,
        };
        let built = build_views(wanted, spells, &mut vctx, memory, false);
        // The pet views read the pet's form and reach where the player's read the player's.
        vctx.caster = ViewCaster::Pet(pet);
        vctx.form = pet.map_or(0, |p| p.unit_shapeshift_form());
        vctx.combat_reach = pet.map_or(1.5, |p| p.unit_combat_reach());
        vctx.attack_target_reach = pet_target_reach;
        let built_pet = build_views(wanted_pet, spells, &mut vctx, memory, true);
        (built, built_pet)
    };
    for (id, view) in built {
        script.set_spell_tooltip(id, view);
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
                slot.insert(reagent_state(
                    entry,
                    vctx.store,
                    vctx.objects,
                    vctx.items,
                    vctx.commands,
                ));
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
