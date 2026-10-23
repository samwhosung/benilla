//! The Craft window feed: the client-built book of a craft type (Enchanting, Beast Training),
//! opened by its opener spell with no packet. Reagents and tools come off `Spell.dbc`, and the
//! description's `$` tokens resolve as a tooltip's do. An enchant is an item-targeted cast
//! (`Targets = 0x10`) whose pick is the ordinary targeting cursor ([`crate::spell::targeting`]).

use bevy::prelude::*;

use benilla_formats::{
    SPELL_ATTR_IS_TRADESKILL, SPELL_EFFECT_CREATE_ITEM, SPELL_EFFECT_ENCHANT_ITEM,
    SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, SPELL_EFFECT_LEARN_PET_SPELL, SPELL_EFFECT_LEARN_SPELL,
};
use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    CraftCall, CraftDifficulty, CraftReagent, CraftRecipe, CraftState, CraftTooltip,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp, ObjectStore, Objects, SelfPlayer};
use crate::query_cache::Ask;
use crate::spell::known::{PetReads, PetView};
use crate::spell::{cast_target, CastCommit, CastLadder};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_items::{count_of, item_icon, InventoryScope};
use crate::ui_spellbook::SkillLines;
use crate::ui_tradeskill::SpellFocus;
use crate::ui_unit::UnitFeed;

/// The open Craft window, client-local. A nonzero opener `EffectMiscValue[0]` routes here
/// (`Spell_C::TryCast` `0x6e4b60`) and is the craft type (1 Beast Training, 3 Enchanting) the
/// client keeps at `0xbdcfb8` for admission and row order. The opener of the open type, the
/// world-enter init (`0x4f5ee0`) and the session end clear it too: a logout's fresh VM never runs
/// the old one's `OnHide`.
#[derive(Resource, Default)]
pub(crate) struct CraftOpen {
    pub(crate) line: Option<u32>,
    pub(crate) craft_type: u32,
}

/// `EffectImplicitTargetA`/`B` 5, the pet (`0x4f632a`).
const TARGET_PET: u32 = 5;

pub(crate) struct UiCraftPlugin;

impl Plugin for UiCraftPlugin {
    fn build(&self, app: &mut App) {
        app.net_handler(SessionEventKind::Disconnected, on_session_end);
        app.init_resource::<CraftOpen>().add_systems(
            Update,
            (
                feed_craft.in_set(UnitFeed),
                drain_craft.in_set(crate::ui_script::UiDrain),
            ),
        );
    }
}

/// The Craft window dies with the session.
fn on_session_end(In(_): In<SessionEvent>, mut open: ResMut<CraftOpen>) {
    *open = CraftOpen::default();
}

/// The line's skill total the Craft window bands difficulty on (`0x5ea520` in the build
/// `0x4f60c0`), where the TradeSkill window uses the raw rank: no slot is 0 (`0x5ea537`), else the
/// rank, plus the permanent bonus when the rank is above 0 (`0x5ea571`), plus the signed temporary
/// bonus, never below 0 (`0x5ea5bc`).
fn skill_total(store: &ObjectStore, skill_id: u32) -> i32 {
    let Some(s) = skill_slot(store, skill_id) else {
        return 0;
    };
    let rank = i32::from(s.value);
    let rank = if rank > 0 {
        rank + i32::from(s.perm_bonus as u16)
    } else {
        rank
    };
    (rank + i32::from(s.temp_bonus)).max(0)
}

/// `GetCraftInfo`'s training-point cost and required level (`0x4f6fe6`-`0x4f7093`), from the
/// pet bar's unit (`0xb714a0`): each effect slot that is `LEARN_SPELL` aimed at the pet
/// (`+0x148` 5) or `LEARN_PET_SPELL` (`0x4f702e`-`0x4f7037`) names a learned spell; its ability
/// row's `reqtrainpoints`, written only when found (`0x4f7049`), and its required level,
/// written only when `0x60c840` answers. The last such slot wins; no pet unit is 0 and 0.
fn cost_and_level(
    pet: &PetView,
    d: &benilla_formats::SpellDisplay,
    objects: &Objects,
) -> (u32, i32) {
    let Some(unit) = pet.unit(objects, pet.bar_guid) else {
        return (0, 0);
    };
    let (mut cost, mut level) = (0, 0);
    for i in 0..3 {
        let effect = d.effects[i];
        let pet_learn = (effect == SPELL_EFFECT_LEARN_SPELL
            && d.effect_implicit_target_a[i] == TARGET_PET)
            || effect == SPELL_EFFECT_LEARN_PET_SPELL;
        if !pet_learn {
            continue;
        }
        let taught = d.effect_trigger_spell[i];
        if let Some((_, row)) = pet.ability(&unit, taught) {
            cost = row.train_points;
        }
        if let Some(l) = pet.required_level(&unit, taught) {
            level = l;
        }
    }
    (cost, level)
}

/// A craft row's bucket (`0x4f61a8`-`0x4f63cc`). A missing ability row or a trivial high of 0
/// leaves 0, `none` (`0x4f61dd`, `0x4f61ea`). Otherwise the ladder on the skill total of the row's
/// line (`0x5ea520`), compared signed: a low of 0 is the high less 25, negative below 25
/// (`0x4f61fc`-`0x4f6205`), and the mid is their sum halved toward zero (`0x4f622b`). A row still
/// at 0 is `used` (`0x4f62f7`) when its first `LEARN_SPELL` (`0x4f6310`) teaches a spell the
/// player knows (`0x4f6347` → `0x4f6360`), or, aimed at the pet (either target 5, `0x4f6323`,
/// `0x4f6333`), one the unit `UNIT_FIELD_SUMMON` names knows or has a higher rank of (`0x4f6369`-
/// `0x4f63b7` → `0x4f63cc`).
fn craft_bucket(
    d: &benilla_formats::SpellDisplay,
    sla: Option<&benilla_formats::SlaInfo>,
    store: &ObjectStore,
    known: &std::collections::BTreeSet<u32>,
    pet: Option<&PetView>,
    objects: &Objects,
) -> CraftDifficulty {
    if let Some(sla) = sla.filter(|a| a.trivial_high != 0) {
        let high = sla.trivial_high as i32;
        let low = match sla.trivial_low as i32 {
            0 => high - 25,
            low => low,
        };
        let total = skill_total(store, sla.skill_id);
        return if total < low {
            CraftDifficulty::Optimal
        } else if total < (low + high) / 2 {
            CraftDifficulty::Medium
        } else if total < high {
            CraftDifficulty::Easy
        } else {
            CraftDifficulty::Trivial
        };
    }
    let Some(i) = (0..3).find(|&i| d.effects[i] == SPELL_EFFECT_LEARN_SPELL) else {
        return CraftDifficulty::None;
    };
    let taught = d.effect_trigger_spell[i];
    let at_pet =
        d.effect_implicit_target_a[i] == TARGET_PET || d.effect_implicit_target_b[i] == TARGET_PET;
    let used = if !at_pet {
        known.contains(&taught)
    } else {
        pet.zip(store.0.unit_summon())
            .and_then(|(pet, guid)| Some((pet, pet.unit(objects, guid)?)))
            .is_some_and(|(pet, unit)| {
                pet.knows(&unit, taught) || pet.knows_higher_rank(&unit, taught)
            })
    };
    if used {
        CraftDifficulty::Used
    } else {
        CraftDifficulty::None
    }
}

/// The first slot holding `skill_id` (`0x5ea3f0`).
fn skill_slot(
    store: &ObjectStore,
    skill_id: u32,
) -> Option<benilla_protocol::messages::PlayerSkillSlot> {
    (0..PLAYER_SKILL_SLOTS)
        .filter_map(|i| store.0.player_skill(i))
        .find(|s| u32::from(s.skill_id) == skill_id)
}

/// `GetCraftDisplaySkillLine`'s answer (`0x4f6cb0`), for the Enchanting type alone (`0x4f6ce3`):
/// the line of the first spell of the player's type-3 list (`0x4f6d0c`) by its ability row
/// (`0x4f6d14`, `+0x04`, 0 failing at `0x4f6d26`), named by `SkillLine.dbc` (`0x4f6d61`), with the
/// slot's rank and maximum, each plus the permanent bonus word when above 0 (`0x4f6d9d`,
/// `0x4f6e10`). The known spells are a set here, so the first is the lowest id. A player with no
/// slot for the line gives the slot -1, which the reads do not test (`0x4f6d52`): the words 12
/// bytes below the first slot, `PLAYER_XP` for the rank and maximum and `PLAYER_NEXT_LEVEL_XP`'s
/// high word for the bonus.
fn display_line(
    first: Option<u32>,
    skill_lines: &SkillLines,
    store: &ObjectStore,
) -> Option<(String, u32, u32)> {
    let (race, class) = store.race_class();
    let line = skill_lines.catalog.ability(first?, race, class)?.skill_id;
    if line == 0 {
        return None;
    }
    let name = skill_lines.catalog.line(line)?.name.clone();
    let (rank, max, perm) = match skill_slot(store, line) {
        Some(s) => (
            u32::from(s.value),
            u32::from(s.max),
            u32::from(s.perm_bonus as u16),
        ),
        None => {
            let xp = store.0.player_xp().unwrap_or(0);
            let next = store.0.player_next_level_xp().unwrap_or(0);
            (xp & 0xffff, xp >> 16, next >> 16)
        }
    };
    let with_bonus = |v: u32| if v > 0 { v + perm } else { v };
    Some((name, with_bonus(rank), with_bonus(max)))
}

/// Law D, the row icon: always the recipe's own `SpellIconID`, never the product. `GetCraftIcon`
/// (`0x4f7160`-`0x4f7204`) never reads `EffectItemType[0]` (`+0x19c`), so a rod recipe fronts the
/// spell's icon here and the rod's in the TradeSkill window.
fn craft_icon(d: &benilla_formats::SpellDisplay) -> Option<String> {
    d.icon.clone()
}

/// The tooltip (`SetCraftSpell` `0x533e90`), off the recipe's own effects: the first slot that
/// is `LEARN_SPELL` names its trigger spell, unchecked, or `CREATE_ITEM` its item; otherwise the
/// recipe itself, as for every enchant. Unlike the trainer's law it never tests
/// `LEARN_PET_SPELL` and never falls through on an unresolvable trigger.
fn craft_tooltip(spell_id: u32, d: &benilla_formats::SpellDisplay) -> CraftTooltip {
    for i in 0..3 {
        if d.effects[i] == SPELL_EFFECT_LEARN_SPELL {
            return CraftTooltip::Spell(d.effect_trigger_spell[i]);
        }
        if d.effects[i] == SPELL_EFFECT_CREATE_ITEM {
            return CraftTooltip::Item(d.effect_item_type[i]);
        }
    }
    CraftTooltip::Spell(spell_id)
}

/// Builds the craft snapshot, `None` while the window is closed or the catalogs are not loaded.
fn feed_craft(
    mut vm: crate::script_calls::VmWith<(PetReads<'static>, Res<'static, CraftOpen>)>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    spell_mods: Res<crate::spell::SpellModifiers>,
    skill_lines: Option<Res<SkillLines>>,
    focus: Option<Res<SpellFocus>>,
    icons: Option<Res<ItemDisplays>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut memo: Local<crate::ui_script::VmMemo<CraftMemo>>,
) {
    vm.feed(|script, (pet_reads, open), fires| {
        let Some(script) = script else {
            return;
        };
        let memo = memo.get(script);
        // The show stands for the stock frame's getter calls, whose lookups pass the callback
        // `0x4f7710` (`0x4f75f2`, `0x4f7a0a`); the rebuilds between read.
        let ask = Ask::at(memo.state.is_none() || open.is_changed(), &commands);
        let fresh = (|| -> Option<CraftState> {
            let line = open.line?;
            let craft_type = open.craft_type;
            let spells = spells.as_deref()?;
            let skill_lines = skill_lines.as_deref()?;
            let store = self_store.single().ok()?;
            let name = skill_lines
                .catalog
                .line(line)
                .map(|l| l.name.clone())
                .unwrap_or_else(|| format!("Skill {line}"));
            let global = |key: &str| benilla_ui::strings::global(script.lua(), key);
            // The player's level for a spell and its cost (`0x6e3130`, `0x6e31b0`, `0x6e3300`).
            let skills = crate::spell::PlayerSkills {
                lines: Some(&skill_lines.catalog),
                spells: Some(&spells.catalog),
                items: Some(&items),
            };
            let ctx = benilla_formats::TokenContext {
                durations: &spells.durations,
                radii: &spells.radii,
                ranges: Some(&spells.ranges),
                level: &|d| {
                    crate::spell::player_spell_level(
                        Some(store),
                        skills,
                        d,
                        &mut crate::spell::Unrecorded,
                    )
                },
                lookup: &|id| spells.catalog.get(id),
                mods: Some(&*spell_mods),
                power_cost: &|d| {
                    crate::spell::usable::power_cost(
                        d,
                        store,
                        skills,
                        &spell_mods,
                        &mut crate::spell::Unrecorded,
                    )
                },
                power_per_second: &|d| {
                    let level = crate::spell::SpellLevel::Player {
                        me: Some(store),
                        skills,
                    };
                    crate::spell::usable::per_second_cost(d, level, &mut crate::spell::Unrecorded)
                },
                unmodified_points: false,
                gender: &|| store.0.unit_gender().unwrap_or(0),
                home_area: &|| None,
                global: &global,
                printf: &crate::ui_script::token_printf,
            };
            // Admission (`0x5e9c20`): known, not hidden (`Attributes & 0x20`), and `castUI` equal
            // to the craft type. The client walks its per-type list (`CGPlayer_C + 0x1cd0 +
            // 0x10*type`), in learn order (`0x5e9e0f`), and never consults `SkillLineAbility` for
            // membership.
            let listed: Vec<u32> = actions
                .craft_order
                .iter()
                .copied()
                .filter(|&s| {
                    spells.catalog.get(s).is_some_and(|d| {
                        d.cast_ui == craft_type && d.attributes & SPELL_ATTR_IS_TRADESKILL == 0
                    })
                })
                .collect();
            let (display_line, rank, max_rank) = (craft_type == 3)
                .then(|| display_line(listed.first().copied(), skill_lines, store))
                .flatten()
                .map_or((None, 0, 0), |(n, r, m)| (Some(n), r, m));
            let pet = PetView::of(&pet_reads, &skill_lines.catalog, &spells.catalog);
            let recipes: Vec<CraftRecipe> = listed
                .iter()
                .filter_map(|&s| {
                    let d = spells.catalog.get(s)?;
                    // A spell with no ability row keeps its row, in group 0 (`0x4f61b6`).
                    let (race, class) = store.race_class();
                    let sla = skill_lines.catalog.ability(s, race, class);
                    let mut reagents = Vec::new();
                    let mut num_available = u32::MAX;
                    for &(entry, need) in d.reagents.iter().filter(|&&(e, n)| e != 0 && n != 0) {
                        let have = count_of(&store.0, &objects, entry, InventoryScope::DEFAULT);
                        // A reagent's icon is the shared item chain (`0x5d88b0`) through its
                        // display.
                        let (name, icon) = match items.template(entry, 0, ask) {
                            Some(t) => (
                                Some(t.name.clone()),
                                item_icon(icons.as_deref(), t.display_info_id),
                            ),
                            None => (None, None),
                        };
                        reagents.push(CraftReagent {
                            item: entry,
                            name,
                            icon,
                            need,
                            have,
                        });
                        num_available = num_available.min(have / need);
                    }
                    if reagents.is_empty() {
                        num_available = 0;
                    }
                    // Focus first, then the totems (`0x4ff980`), the list `GetCraftSpellFocus`
                    // (`0x4f78b0`) returns. The focus's flag is a literal `1.0`, never reddened.
                    let mut tools = Vec::new();
                    if d.requires_spell_focus != 0 {
                        if let Some(n) = focus
                            .as_deref()
                            .and_then(|f| f.catalog.name(d.requires_spell_focus))
                        {
                            tools.push((n.to_string(), true));
                        }
                    }
                    let mut tools_pending = Vec::new();
                    for &t in d.totems.iter().filter(|&&t| t != 0) {
                        let have = count_of(&store.0, &objects, t, InventoryScope::DEFAULT) > 0;
                        match items.template(t, 0, ask) {
                            Some(info) => tools.push((info.name.clone(), have)),
                            None => tools_pending.push(t),
                        }
                    }
                    let needs_item_target = matches!(
                        d.effects[0],
                        SPELL_EFFECT_ENCHANT_ITEM | SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY
                    );
                    let icon = craft_icon(d);
                    let (training_point_cost, required_level) = pet
                        .as_ref()
                        .map_or((0, 0), |p| cost_and_level(p, d, &objects));
                    Some(CraftRecipe {
                        spell_id: s,
                        name: d.name.clone(),
                        sub_name: d.rank.clone().unwrap_or_default(),
                        difficulty: craft_bucket(
                            d,
                            sla,
                            store,
                            &actions.spells,
                            pet.as_ref(),
                            &objects,
                        ),
                        num_available,
                        icon,
                        description: d
                            .description
                            .as_deref()
                            .map(|t| benilla_formats::substitute(t, d, &ctx)),
                        needs_item_target,
                        reagents,
                        tools,
                        tooltip: craft_tooltip(s, d),
                        // `spellLevel` at `+0x74` (column 29), not `baseLevel` (column 28).
                        spell_level: d.spell_level,
                        // The group key is the ability row's line (`0x4f61b1`), 0 with none.
                        group: sla.map_or(0, |a| a.skill_id),
                        skill_line: sla
                            .and_then(|a| skill_lines.catalog.line(a.skill_id))
                            .map(|l| l.name.clone()),
                        training_point_cost,
                        required_level,
                        tools_pending,
                    })
                })
                .collect();
            // No sort here: the engine orders the rows by the craft type's own comparator
            // (`benilla_ui::script::craft::recipe_order`).
            Some(CraftState {
                display_line,
                name,
                rank,
                max_rank,
                craft_type,
                recipes,
            })
        })();

        // The type stays open while a frame cannot resolve its book (a worldport's purge of our
        // own player): only a close or the world-enter init ends it (`0x4f5fe0`, `0x4f5ee0`).
        let fresh = match fresh {
            None if open.line.is_some() => memo.state.clone(),
            f => f,
        };
        // `CloseCraft` closes the VM's window and signals `CRAFT_CLOSE` in the verb (`0x4f5fef`).
        let closed_in_place = !script.craft_open();
        // The getters' item callbacks, kept by their calls; each record that landed rebuilds
        // (`0x4f7710` → `0x4f771a`), and a refusal runs it with status 0, which does nothing.
        memo.item_waits.extend(script.take_craft_item_waits());
        let arrivals = crate::ui_tradeskill::landed(&mut memo.item_waits, &items);
        let opened = match (&memo.state, &fresh) {
            (None, Some(_)) => true,
            (Some(l), Some(f)) => l.craft_type != f.craft_type || l.name != f.name,
            _ => false,
        };
        // Every push is silent: the rebuilds signal, and the getters read the state at their call.
        if fresh != memo.state {
            script.set_craft(fresh.clone());
            // Pre-ask the reagent templates `GetCraftReagentItemLink` reads.
            if let Some(f) = &fresh {
                script.ask_item_templates(
                    f.recipes
                        .iter()
                        .flat_map(|r| r.reagents.iter().map(|re| re.item)),
                );
            }
        }
        match (&memo.state, &fresh) {
            (_, Some(f)) if opened => {
                debug!("ui_craft: window opens — {} recipe(s)", f.recipes.len());
                // The open's resets and build (`0x4f601c`-`0x4f6036`), whose tail signals the
                // update (`0x4f661a`), then the show (`0x4f6040`).
                script.craft_opened();
                fires.post("CRAFT_UPDATE", vec![]);
                fires.post("CRAFT_SHOW", vec![]);
            }
            (Some(_), None) if !closed_in_place => {
                debug!("ui_craft: window closes");
                fires.post("CRAFT_CLOSE", vec![]);
            }
            _ => {}
        }
        // Each landed record's rebuild signals the update; the packets' fire on their lane
        // (`crate::ui_tradeskill::register_rebuild_triggers`).
        if fresh.is_some() {
            for _ in 0..arrivals {
                fires.post("CRAFT_UPDATE", vec![]);
            }
        }
        memo.state = fresh;
    })
}

/// What [`feed_craft`] remembers from its last frame.
#[derive(Default)]
struct CraftMemo {
    /// The state it last pushed.
    state: Option<CraftState>,
    /// The item records the getters' calls wait on, one per call.
    item_waits: Vec<u32>,
}

/// Drains `DoCraft` down the cast ladder: an enchant's `Targets = 0x10` arms the cursor's item
/// half, a rod craft commits at once. `CloseCraft` closes the window inside its call
/// ([`CraftSteps`]), sends no packet and leaves an armed pick to the ordinary cancels (Escape,
/// right-click, a new cast).
fn drain_craft(
    mut vm: crate::script_calls::VmWith<(
        cast_target::CastTargeting<'static, 'static>,
        CastLadder<'static, 'static>,
        Res<'static, CraftOpen>,
    )>,
    skill_lines: Option<Res<SkillLines>>,
) {
    vm.turn(|script, (targeting, mut ladder, open)| {
        let Some(script) = script else {
            return;
        };
        let _ = skill_lines; // the feed resolves lines; the drain just casts what it is handed
        for spell_id in script.take_craft_dos() {
            if open.line.is_none() {
                debug!("ui_craft: DoCraft({spell_id}) with no open window — ignored");
                continue;
            }
            debug!("ui_craft: DoCraft({spell_id})");
            ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
        }
    })
}

/// The craft window's close inside its call ([`crate::script_calls::ScriptCallHost`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CraftSteps<'w> {
    /// Absent in a world with no craft window, which drops the call.
    open: Option<ResMut<'w, CraftOpen>>,
}

impl CraftSteps<'_> {
    /// `CloseCraft()` (`0x4f6c00` → `0x4f5fe0`): the open marker `[0xbdcfb8]` zeroed, no packet.
    pub(crate) fn apply(&mut self, call: CraftCall) {
        let CraftCall::Close = call;
        if let Some(open) = self.open.as_deref_mut() {
            debug!("ui_craft: client-side close (no packet)");
            open.line = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::SpellDisplay;
    use benilla_ui::script::UiScript;

    /// `CloseCraft` signals `CRAFT_CLOSE` in the verb (`0x4f5fef`), so the feed that finds the
    /// window closed a frame later signals no second close.
    #[test]
    fn a_verbs_close_is_signalled_once() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<CraftOpen>()
            .init_resource::<PlayerActions>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx))
            .insert_resource(crate::ui_action::Spells::empty_for_tests())
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_spell_lines([]),
            });
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
        script
            .run(
                "CLOSES = 0 local f = CreateFrame('Frame') f:RegisterEvent('CRAFT_CLOSE') \
                 f:SetScript('OnEvent', function() CLOSES = CLOSES + 1 end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.add_systems(Update, feed_craft);
        let closes = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<i64>("return CLOSES")
                .unwrap()
        };

        app.world_mut().resource_mut::<CraftOpen>().line = Some(333);
        app.update();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("CloseCraft()")
            .unwrap();
        assert_eq!(closes(&app), 1, "the verb's own signal");
        app.world_mut().resource_mut::<CraftOpen>().line = None;
        app.update();
        assert_eq!(closes(&app), 1, "no second signal from the feed");

        app.world_mut().resource_mut::<CraftOpen>().line = Some(333);
        app.update();
        app.world_mut().resource_mut::<CraftOpen>().line = None;
        app.update();
        assert_eq!(closes(&app), 2, "a close from outside the VM still signals");
    }

    #[test]
    fn law_d_is_always_the_spells_own_icon_even_for_a_rod_recipe() {
        let rod = SpellDisplay {
            name: "Runed Copper Rod".into(),
            icon: Some("SPELL".into()),
            effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
            effect_item_type: [6218, 0, 0],
            ..Default::default()
        };
        assert_eq!(craft_icon(&rod), Some("SPELL".into()));
    }

    /// The binding pushes nil; there is no item arm to fall through to.
    #[test]
    fn law_d_is_nil_when_the_spell_carries_no_icon() {
        let d = SpellDisplay {
            effect_item_type: [6218, 0, 0],
            ..Default::default()
        };
        assert_eq!(craft_icon(&d), None);
    }
    #[test]
    fn craft_tooltip_reads_the_recipes_own_effects() {
        // An enchant has no matching slot: the recipe spell itself.
        let enchant = SpellDisplay {
            effects: [SPELL_EFFECT_ENCHANT_ITEM, 0, 0],
            ..Default::default()
        };
        assert_eq!(craft_tooltip(7420, &enchant), CraftTooltip::Spell(7420));

        // A rod craft: the item of the same slot.
        let rod = SpellDisplay {
            effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
            effect_item_type: [6218, 0, 0],
            ..Default::default()
        };
        assert_eq!(craft_tooltip(7421, &rod), CraftTooltip::Item(6218));

        // A `LEARN_SPELL` slot hops to its trigger without checking that it resolves.
        let mut teacher = SpellDisplay::default();
        teacher.effects[1] = SPELL_EFFECT_LEARN_SPELL;
        teacher.effect_trigger_spell[1] = 999_999;
        assert_eq!(craft_tooltip(5149, &teacher), CraftTooltip::Spell(999_999));
    }

    /// `0x4f62cc`/`0x4f76b2` count a reagent with `0x622130` and `0x4ffb10` finds a tool with
    /// `0x622270`, both at mask 0: a copy worn in a slot or in the keyring counts, one in the bank
    /// does not.
    #[test]
    fn a_recipe_counts_a_worn_copy_and_the_keyring_but_not_the_bank() {
        use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY};
        use benilla_protocol::ObjectFields;
        const DUST: u32 = 10_940;
        const ROD: u32 = 6_218;
        const SPELL: u32 = 7_000;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.spells.insert(SPELL);
        actions.profession_order.push(SPELL);
        actions.craft_order.push(SPELL);
        let mut items = Items::default();
        items.insert_template(DUST, Some(crate::items::test_template("Strange Dust")));
        items.insert_template(ROD, Some(crate::items::test_template("Runed Rod")));
        app.init_resource::<CraftOpen>()
            .insert_resource(actions)
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::net::GuidIndex>()
            .insert_resource(items)
            .insert_resource(NetCommands(tx))
            .insert_resource(crate::ui_action::Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(
                        SPELL,
                        SpellDisplay {
                            name: "Enchant".into(),
                            cast_ui: 3,
                            reagents: [
                                (DUST, 2),
                                (0, 0),
                                (0, 0),
                                (0, 0),
                                (0, 0),
                                (0, 0),
                                (0, 0),
                                (0, 0),
                            ],
                            totems: [ROD, 0],
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..crate::ui_action::Spells::empty_for_tests()
            })
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_abilities([(
                    SPELL,
                    benilla_formats::SlaInfo {
                        skill_id: 333,
                        req_skill_value: 1,
                        forward_spell_id: 0,
                        trivial_low: 1,
                        trivial_high: 2,
                    },
                )]),
            });
        for (guid, entry, n) in [
            (0x4000_0000_0000_0001u64, DUST, 2u32),
            (0x4000_0000_0000_0002, ROD, 1),
        ] {
            crate::items::test_spawn_item(
                app.world_mut(),
                guid,
                ObjectFields::from_pairs(&[
                    (FIELD_OBJECT_ENTRY, entry),
                    (FIELD_ITEM_STACK_COUNT, n),
                ]),
                false,
            );
        }
        let player = app
            .world_mut()
            .spawn((SelfPlayer, ObjectStore(Default::default())))
            .id();
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_craft);
        {
            let mut open = app.world_mut().resource_mut::<CraftOpen>();
            open.line = Some(333);
            open.craft_type = 3;
        }
        // Where the dust and the rod sit: player slots, 17 worn, 81 the keyring, 39 the bank.
        let mut read = |dust_slot: u16, rod_slot: u16| -> (i64, bool) {
            let seat = |slot: u16, guid: u64| {
                let lo = 486 + 2 * slot;
                [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)]
            };
            app.world_mut()
                .entity_mut(player)
                .insert(ObjectStore(ObjectFields::from_pairs(
                    &[
                        seat(dust_slot, 0x4000_0000_0000_0001),
                        seat(rod_slot, 0x4000_0000_0000_0002),
                    ]
                    .concat(),
                )));
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<(i64, bool)>(
                    "local _, _, _, have = GetCraftReagentInfo(1, 1) \
                     local _, hasTool = GetCraftSpellFocus(1) return have, hasTool == 1",
                )
                .unwrap()
        };
        assert_eq!(read(23, 24), (2, true), "the backpack, as before");
        assert_eq!(read(17, 18), (2, true), "worn copies");
        assert_eq!(read(81, 82), (2, true), "the keyring");
        assert_eq!(read(39, 40), (0, false), "the bank is outside mask 0");
    }

    #[test]
    fn the_session_end_closes_the_craft_window() {
        let mut app = App::new();
        app.add_plugins(UiCraftPlugin);
        {
            let mut open = app.world_mut().resource_mut::<CraftOpen>();
            open.line = Some(333);
            open.craft_type = 3;
        }

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        let open = app.world().resource::<CraftOpen>();
        assert_eq!((open.line, open.craft_type), (None, 0));
    }

    const BEAST_TRAINING: u32 = 261;
    const ENCHANTING: u32 = 333;
    const TAILORING: u32 = 197;
    const PET_WOLF: u32 = 208;
    const PET_GENERIC: u32 = 270;
    const WOLF_FAMILY: u32 = 1;
    const WOLF_ENTRY: u32 = 299;
    const PET: u64 = 0xF140_0000_0000_0042;
    const OTHER_PET: u64 = 0xF140_0000_0000_0043;
    /// `UNIT_FIELD_SUMMON`, the guid pair the `used` test fetches its unit by (`[fields + 8]`).
    const F_SUMMON: u16 = 8;
    const F_TYPE: u16 = 2;
    const F_ENTRY: u16 = 3;
    const F_XP: u16 = 716;
    const F_NEXT_XP: u16 = 717;
    const F_SKILL_1: u16 = 718;

    /// A learn spell of craft type `cast_ui`: `effects[i]` (36 `LEARN_SPELL`, 57 `LEARN_PET_SPELL`)
    /// teach `triggers[i]`, aimed at `targets[i]` (5 the pet).
    fn learner(
        name: &str,
        rank: &str,
        cast_ui: u32,
        effects: [u32; 3],
        targets: [u32; 3],
        triggers: [u32; 3],
    ) -> SpellDisplay {
        SpellDisplay {
            name: name.into(),
            rank: Some(rank.into()),
            cast_ui,
            effects,
            effect_implicit_target_a: targets,
            effect_trigger_spell: triggers,
            spell_level: 10,
            ..Default::default()
        }
    }

    /// A pet spell the learn effects teach, at `spell_level`.
    fn pet_spell(name: &str, spell_level: u32) -> SpellDisplay {
        SpellDisplay {
            name: name.into(),
            spell_level,
            ..Default::default()
        }
    }

    /// An ability row on `line` with the trivial ranks `(low, high)`.
    fn ability(line: u32, low: u32, high: u32) -> benilla_formats::SlaInfo {
        benilla_formats::SlaInfo {
            skill_id: line,
            req_skill_value: 0,
            forward_spell_id: 0,
            trivial_low: low,
            trivial_high: high,
        }
    }

    /// The feed over `displays`, the player knowing `known`, with the window of `craft_type` open
    /// and no pet; [`seat_pet`] adds one.
    fn craft_feed(
        displays: Vec<(u32, SpellDisplay)>,
        known: &[u32],
        catalog: benilla_formats::SkillLineCatalog,
        craft_type: u32,
        player: &[(u16, u32)],
    ) -> App {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.spells.extend(known.iter().copied());
        actions.profession_order.extend(known.iter().copied());
        actions.craft_order.extend(known.iter().copied());
        app.init_resource::<CraftOpen>()
            .insert_resource(actions)
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::names::NameCache>()
            .insert_resource(crate::ui_pet_stats::PetFamilyTables {
                families: benilla_formats::CreatureFamilies::from_skill_lines([(
                    WOLF_FAMILY,
                    [PET_WOLF, PET_GENERIC],
                )]),
                foods: Default::default(),
            })
            .insert_resource(NetCommands(tx))
            .insert_resource(crate::ui_action::Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    displays.into_iter().collect(),
                ),
                ..crate::ui_action::Spells::empty_for_tests()
            })
            .insert_resource(SkillLines { catalog });
        app.insert_non_send_resource(rx);
        app.world_mut().spawn((
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(player)),
            SelfPlayer,
        ));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_craft);
        let mut open = app.world_mut().resource_mut::<CraftOpen>();
        open.line = Some(if craft_type == 3 {
            ENCHANTING
        } else {
            BEAST_TRAINING
        });
        open.craft_type = craft_type;
        app
    }

    /// A wolf at `PET` with `book` in its spellbook, on the pet bar as `bar`, and the player's
    /// `UNIT_FIELD_SUMMON` at `summon`.
    fn seat_pet(app: &mut App, summon: u64, bar: u64, book: &[u32]) {
        let world = app.world_mut();
        let pet = world
            .spawn(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (F_TYPE, 0x9),
                (F_ENTRY, WOLF_ENTRY),
            ])))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(PET, pet);
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_creature(
                WOLF_ENTRY,
                Some(crate::names::CreatureRecord {
                    name: "Wolf".into(),
                    subname: None,
                    creature_type: 1,
                    pet_family: WOLF_FAMILY,
                    rank: 0,
                    type_flags: 0,
                    pet_spell_list: 0,
                    civilian: false,
                    racial_leader: false,
                    display_id: 0,
                }),
            );
        let mut bar_res = world.resource_mut::<crate::ui_pet::PetBar>();
        bar_res.spells.pet_guid = bar;
        bar_res.spells.spells = book
            .iter()
            .map(|&s| benilla_protocol::messages::PetActionEntry { packed: s })
            .collect();
        let mut q = world.query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
        let mut me = q.single_mut(world).unwrap();
        me.0 = benilla_protocol::ObjectFields::from_pairs(&[
            (F_SUMMON, summon as u32),
            (F_SUMMON + 1, (summon >> 32) as u32),
        ]);
    }

    /// Each row's `(name rank, craftType, trainingPointCost, requiredLevel)`, in display order.
    fn craft_rows(app: &mut App) -> Vec<(String, String, i64, i64)> {
        app.update();
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<Vec<Vec<String>>>(
                "local t = {} for i = 1, GetNumCrafts() do \
                 local n, s, k, _, _, tp, l = GetCraftInfo(i) \
                 t[i] = { n .. ' ' .. s, k, tostring(tp), tostring(l) } end return t",
            )
            .unwrap()
            .into_iter()
            .map(|r| {
                (
                    r[0].clone(),
                    r[1].clone(),
                    r[2].parse().unwrap(),
                    r[3].parse().unwrap(),
                )
            })
            .collect()
    }

    /// `0x4f61a8`-`0x4f61e4`: a spell `0x60c7c0` finds no ability row for keeps its row, group 0
    /// and bucket 0, `none`; `GetCraftSkillLine` names no line for group 0.
    #[test]
    fn a_craft_spell_with_no_ability_row_keeps_its_row_at_group_zero_and_bucket_none() {
        let mut app = craft_feed(
            vec![(
                24_533,
                learner(
                    "Great Stamina",
                    "Rank 1",
                    1,
                    [36, 0, 0],
                    [0; 3],
                    [4195, 0, 0],
                ),
            )],
            &[24_533],
            benilla_formats::SkillLineCatalog::from_abilities([]),
            1,
            &[],
        );
        assert_eq!(
            craft_rows(&mut app),
            [("Great Stamina Rank 1".into(), "none".into(), 0, 0)]
        );
        assert!(app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<bool>("return GetCraftSkillLine(1) == nil")
            .unwrap());
    }

    /// Great Stamina's ranks as shipped: 4195 and 4196 teach the pet 4187 and 4188, which
    /// `Pet - Generic` chains (`+0x20`).
    fn beast_training() -> (Vec<(u32, SpellDisplay)>, benilla_formats::SkillLineCatalog) {
        let displays = vec![
            (
                4195,
                learner(
                    "Great Stamina",
                    "Rank 1",
                    1,
                    [36, 0, 0],
                    [5, 0, 0],
                    [4187, 0, 0],
                ),
            ),
            (
                4196,
                learner(
                    "Great Stamina",
                    "Rank 2",
                    1,
                    [36, 0, 0],
                    [5, 0, 0],
                    [4188, 0, 0],
                ),
            ),
            (
                9000,
                learner("Track", "Rank 1", 1, [36, 0, 0], [0; 3], [9001, 0, 0]),
            ),
            (
                9100,
                learner("Bond", "Rank 1", 1, [57, 0, 0], [0; 3], [4187, 0, 0]),
            ),
            (
                9200,
                learner("Twin", "Rank 1", 1, [36, 36, 0], [5, 5, 0], [4187, 4188, 0]),
            ),
            (
                9300,
                learner(
                    "Stray",
                    "Rank 1",
                    1,
                    [36, 36, 0],
                    [5, 5, 0],
                    [4188, 7777, 0],
                ),
            ),
            (4187, pet_spell("Great Stamina", 4)),
            (4188, pet_spell("Great Stamina", 8)),
            (9001, pet_spell("Track", 1)),
        ];
        let catalog = benilla_formats::SkillLineCatalog::from_abilities(
            [4195, 4196, 9000, 9100, 9200, 9300].map(|s| (s, ability(BEAST_TRAINING, 0, 0))),
        )
        .with_line_abilities([
            (
                PET_GENERIC,
                4187,
                benilla_formats::LineAbility {
                    superseded_by: 4188,
                    train_points: 5,
                },
            ),
            (
                PET_GENERIC,
                4188,
                benilla_formats::LineAbility {
                    superseded_by: 0,
                    train_points: 10,
                },
            ),
        ]);
        (displays, catalog)
    }

    /// `0x4f61ea`: an ability row whose trivial high is 0, every Beast Training row's, leaves
    /// bucket 0, `none`. A bucket-0 row's first `LEARN_SPELL` (`0x4f6310`) not aimed at the pet
    /// is `used` when the player knows its spell (`0x4f6347` → `0x4f6360`); aimed at the pet
    /// (`EffectImplicitTargetA` 5, `0x4f6323`), when the unit `UNIT_FIELD_SUMMON` names knows it
    /// or a higher rank (`0x4f63a1`, `0x4f63b7` → `0x4f63cc`), which only the pet bar's unit can
    /// (`0x60c77a`): its book. Bucket 5 sorts last (`0x4f6a34`).
    #[test]
    fn beast_training_rows_read_none_and_used_once_known() {
        let known = [4195, 4196, 9000, 9001];
        let feed = || {
            let (displays, catalog) = beast_training();
            craft_feed(displays, &known, catalog, 1, &[])
        };
        let kinds = |app: &mut App| -> Vec<(String, String)> {
            craft_rows(app)
                .into_iter()
                .filter(|r| r.0.starts_with("Great") || r.0.starts_with("Track"))
                .map(|r| (r.0, r.1))
                .collect()
        };
        let pair = |n: &str, k: &str| (n.to_string(), k.to_string());

        let mut app = feed();
        assert_eq!(
            kinds(&mut app),
            [
                pair("Great Stamina Rank 1", "none"),
                pair("Great Stamina Rank 2", "none"),
                pair("Track Rank 1", "used"),
            ],
            "no pet: only the player's own learn reads used"
        );

        let mut app = feed();
        seat_pet(&mut app, PET, PET, &[4187]);
        assert_eq!(
            kinds(&mut app),
            [
                pair("Great Stamina Rank 2", "none"),
                pair("Great Stamina Rank 1", "used"),
                pair("Track Rank 1", "used"),
            ],
            "the pet knows rank 1"
        );

        let mut app = feed();
        seat_pet(&mut app, PET, PET, &[4188]);
        assert_eq!(
            kinds(&mut app),
            [
                pair("Great Stamina Rank 1", "used"),
                pair("Great Stamina Rank 2", "used"),
                pair("Track Rank 1", "used"),
            ],
            "rank 2 is a higher rank of rank 1"
        );

        let mut app = feed();
        seat_pet(&mut app, OTHER_PET, PET, &[4187, 4188]);
        assert_eq!(
            kinds(&mut app),
            [
                pair("Great Stamina Rank 1", "none"),
                pair("Great Stamina Rank 2", "none"),
                pair("Track Rank 1", "used"),
            ],
            "a summon that is not the pet bar's unit knows nothing"
        );
    }

    /// `0x4f6fe6`-`0x4f7093`: with the pet bar's unit (`0xb714a0`), each effect slot that is 36
    /// aimed at the pet (`+0x148` 5) or 57 names a learned spell (`+0xc0`); its first ability row
    /// on the pet family's lines (`0x60c7c0` over `CreatureFamily` `+0x14`, `+0x18`) gives the cost
    /// (`+0x38`, written only when found, `0x4f7049`) and `0x60c840` the level, the learned spell's
    /// `+0x74` (no `SkillRaceClassInfo` row for a pet line raises it). The last such slot wins; no
    /// pet answers 0 and 0.
    #[test]
    fn a_pet_learn_row_answers_its_training_point_cost_and_required_level() {
        let known = [4195, 4196, 9000, 9100, 9200, 9300];
        let feed = || {
            let (displays, catalog) = beast_training();
            craft_feed(displays, &known, catalog, 1, &[])
        };
        let costs = |app: &mut App| -> Vec<(String, i64, i64)> {
            let mut rows: Vec<_> = craft_rows(app)
                .into_iter()
                .map(|r| (r.0, r.2, r.3))
                .collect();
            rows.sort();
            rows
        };
        let row = |n: &str, tp: i64, l: i64| (n.to_string(), tp, l);
        let mut app = feed();
        assert!(
            costs(&mut app).iter().all(|r| (r.1, r.2) == (0, 0)),
            "no pet"
        );
        let mut app = feed();
        seat_pet(&mut app, PET, PET, &[]);
        assert_eq!(
            costs(&mut app),
            [
                row("Bond Rank 1", 5, 4),
                row("Great Stamina Rank 1", 5, 4),
                row("Great Stamina Rank 2", 10, 8),
                row("Stray Rank 1", 10, 8),
                row("Track Rank 1", 0, 0),
                row("Twin Rank 1", 10, 8),
            ]
        );
    }

    /// `GetCraftDisplaySkillLine` (`0x4f6cb0`) reads the slot of its line's rank and maximum, each
    /// plus the permanent bonus word (`+0x852`) when above 0 (`0x4f6d9d`, `0x4f6e10`). A player
    /// with no slot for the line gets slot -1, untested (`0x4f6d52`), so the words 12 bytes below
    /// the first slot: `PLAYER_XP` for the rank and maximum, `PLAYER_NEXT_LEVEL_XP`'s high word
    /// for the bonus.
    #[test]
    fn the_display_line_reads_its_slot_with_the_permanent_bonus() {
        let displays = || {
            vec![(
                7418,
                SpellDisplay {
                    name: "Enchant Bracer - Minor Health".into(),
                    cast_ui: 3,
                    ..Default::default()
                },
            )]
        };
        let catalog = || {
            benilla_formats::SkillLineCatalog::from_abilities([(7418, ability(ENCHANTING, 1, 70))])
                .with_line_names([(ENCHANTING, "Enchanting")])
        };
        let triple = |app: &mut App| {
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<(Option<String>, i64, i64)>("return GetCraftDisplaySkillLine()")
                .unwrap()
        };
        let slot = |value: u32, max: u32, perm: u32| {
            [
                (F_SKILL_1, ENCHANTING),
                (F_SKILL_1 + 1, value | max << 16),
                (F_SKILL_1 + 2, perm << 16),
            ]
        };
        let mut app = craft_feed(displays(), &[7418], catalog(), 3, &slot(100, 150, 10));
        assert_eq!(triple(&mut app), (Some("Enchanting".into()), 110, 160));
        let mut app = craft_feed(displays(), &[7418], catalog(), 3, &slot(0, 150, 10));
        assert_eq!(triple(&mut app), (Some("Enchanting".into()), 0, 160));
        let mut app = craft_feed(
            displays(),
            &[7418],
            catalog(),
            3,
            &[(F_XP, 25 | 50 << 16), (F_NEXT_XP, 3 << 16)],
        );
        assert_eq!(triple(&mut app), (Some("Enchanting".into()), 28, 53));
    }

    /// The display line is the line of the type's first spell (`0x4f6cb0`), the first the player
    /// learned of its list (`0x5e9e0f` appends), not the lowest id.
    #[test]
    fn the_display_line_follows_the_first_spell_learned() {
        let enchant = |name: &str| SpellDisplay {
            name: name.into(),
            cast_ui: 3,
            ..Default::default()
        };
        let displays = vec![
            (7418, enchant("Enchant Bracer - Minor Health")),
            (7420, enchant("Enchant Chest - Minor Health")),
        ];
        let catalog = benilla_formats::SkillLineCatalog::from_abilities([
            (7418, ability(ENCHANTING, 1, 70)),
            (7420, ability(TAILORING, 1, 70)),
        ])
        .with_line_names([(ENCHANTING, "Enchanting"), (TAILORING, "Tailoring")]);
        let mut app = craft_feed(displays, &[7420, 7418], catalog, 3, &[]);
        app.update();
        let name = app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<Option<String>>("return (GetCraftDisplaySkillLine())")
            .unwrap();
        assert_eq!(name.as_deref(), Some("Tailoring"));
    }

    /// The window's open runs the build (`0x4f6036` → `0x4f60c0`), whose tail signals
    /// `CRAFT_UPDATE` (`0x4f661a`), then `CRAFT_SHOW` (`0x4f6040`); a change between rebuilds, as a
    /// skill's rank, which no watcher rebuilds the craft list on (`0x5de434` is the profession
    /// window's alone), signals nothing.
    #[test]
    fn the_open_signals_update_then_show_and_a_bare_change_signals_nothing() {
        let displays = vec![(
            7418,
            SpellDisplay {
                name: "Enchant Bracer - Minor Health".into(),
                cast_ui: 3,
                ..Default::default()
            },
        )];
        let catalog =
            benilla_formats::SkillLineCatalog::from_abilities([(7418, ability(ENCHANTING, 1, 70))]);
        let mut app = craft_feed(
            displays,
            &[7418],
            catalog,
            3,
            &[(F_SKILL_1, ENCHANTING), (F_SKILL_1 + 1, 1 | 75 << 16)],
        );
        app.world()
            .non_send_resource::<UiScript>()
            .run(
                "HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('CRAFT_SHOW') \
                 f:RegisterEvent('CRAFT_UPDATE') \
                 f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
            )
            .unwrap();
        let heard = |app: &mut App| {
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
                .unwrap()
        };
        assert_eq!(heard(&mut app), ["CRAFT_UPDATE", "CRAFT_SHOW"]);
        let world = app.world_mut();
        let mut q = world.query_filtered::<&mut ObjectStore, With<SelfPlayer>>();
        q.single_mut(world).unwrap().0 = benilla_protocol::ObjectFields::from_pairs(&[
            (F_SKILL_1, ENCHANTING),
            (F_SKILL_1 + 1, 70 | 75 << 16),
        ]);
        assert!(heard(&mut app).is_empty());
        assert_eq!(
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<String>("local _, _, k = GetCraftInfo(1) return k")
                .unwrap(),
            "trivial",
            "pushed, silently"
        );
    }

    /// `SMSG_PET_SPELLS` rebuilds the list on every arrival (`0x4bd990` → `0x4bdafa`), a book
    /// resent unchanged included; a learn or an unlearn of a craft spell rebuilds it once per packet
    /// (`0x5e9e12`, `0x5ea240`), whether or not the list moved; a record a getter's call waited on
    /// rebuilds it once per call (`0x4f7710`).
    #[test]
    fn each_packet_and_each_waited_record_signals_one_update() {
        const DUST: u32 = 10_940;
        let displays = vec![
            (
                7418,
                SpellDisplay {
                    name: "Enchant Bracer - Minor Health".into(),
                    cast_ui: 3,
                    reagents: [
                        (DUST, 1),
                        (0, 0),
                        (0, 0),
                        (0, 0),
                        (0, 0),
                        (0, 0),
                        (0, 0),
                        (0, 0),
                    ],
                    ..Default::default()
                },
            ),
            (
                7420,
                SpellDisplay {
                    name: "Enchant Chest - Minor Health".into(),
                    cast_ui: 3,
                    ..Default::default()
                },
            ),
        ];
        let catalog =
            benilla_formats::SkillLineCatalog::from_abilities([(7418, ability(ENCHANTING, 1, 70))]);
        let mut app = craft_feed(displays, &[7418], catalog, 3, &[]);
        app.insert_resource(State::new(crate::client_state::ClientState::InWorld));
        crate::ui_tradeskill::register_rebuild_triggers(&mut app);
        app.world()
            .non_send_resource::<UiScript>()
            .run(
                "HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('CRAFT_SHOW') \
                 f:RegisterEvent('CRAFT_UPDATE') \
                 f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
            )
            .unwrap();
        let heard = |app: &mut App| {
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
                .unwrap()
        };
        assert_eq!(heard(&mut app), ["CRAFT_UPDATE", "CRAFT_SHOW"]);
        let pet_spells = || SessionEvent::PetSpells(Box::default());
        crate::net::handlers::dispatch(app.world_mut(), vec![pet_spells(), pet_spells()]);
        assert_eq!(heard(&mut app), ["CRAFT_UPDATE", "CRAFT_UPDATE"]);
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![
                SessionEvent::SpellLearned { spell_id: 7418 },
                SessionEvent::SpellRemoved { spell_id: 7418 },
            ],
        );
        assert_eq!(heard(&mut app), ["CRAFT_UPDATE", "CRAFT_UPDATE"]);
        // An unlearn the type's list does not hold skips the rebuild (`0x5ea1c9`).
        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![SessionEvent::SpellRemoved { spell_id: 7420 }],
        );
        assert!(heard(&mut app).is_empty());
        app.world()
            .non_send_resource::<UiScript>()
            .run("GetCraftReagentInfo(1, 1) GetCraftReagentInfo(1, 1)")
            .unwrap();
        assert!(heard(&mut app).is_empty());
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(DUST, Some(crate::items::test_template("Strange Dust")));
        assert_eq!(heard(&mut app), ["CRAFT_UPDATE", "CRAFT_UPDATE"]);
    }

    /// The world-enter init `0x4f5ee0` (`0x4909c4`) zeroes the craft type (`[0xbdcfb8]`) and
    /// signals nothing; `CRAFT_CLOSE` is `0x4f5fe0`'s alone. A worldport's cascade closes the
    /// window under the interface that survives it with no close, and a reload's closes it under
    /// the new one, which hears no show.
    #[test]
    fn the_world_enter_init_closes_the_window_silently() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<CraftOpen>()
            .init_resource::<PlayerActions>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .insert_resource(NetCommands(tx))
            .insert_resource(crate::ui_action::Spells::empty_for_tests())
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_spell_lines([]),
            })
            .add_message::<crate::net::WorldEnterCascadeMessage>();
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        let listening = || {
            let script = UiScript::new().unwrap();
            script
                .run(
                    "HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('CRAFT_SHOW') \
                     f:RegisterEvent('CRAFT_UPDATE') f:RegisterEvent('CRAFT_CLOSE') \
                     f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
                )
                .unwrap();
            script
        };
        app.insert_non_send_resource(listening());
        app.add_systems(
            Update,
            (
                crate::ui_tradeskill::reset_crafts_on_world_enter,
                crate::ui_tradeskill::close_craft_on_world_enter,
                feed_craft,
            )
                .chain(),
        );
        let heard = |app: &mut App| {
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
                .unwrap()
        };
        let open = |app: &mut App| {
            *app.world_mut().resource_mut::<CraftOpen>() = CraftOpen {
                line: Some(333),
                craft_type: 3,
            };
            heard(app)
        };

        assert_eq!(open(&mut app), ["CRAFT_UPDATE", "CRAFT_SHOW"]);
        app.world_mut()
            .write_message(crate::net::WorldEnterCascadeMessage);
        assert!(heard(&mut app).is_empty(), "a worldport closes it silently");
        assert_eq!(app.world().resource::<CraftOpen>().line, None);

        assert_eq!(open(&mut app), ["CRAFT_UPDATE", "CRAFT_SHOW"]);
        app.insert_non_send_resource(listening());
        app.world_mut()
            .write_message(crate::net::WorldEnterCascadeMessage);
        assert!(heard(&mut app).is_empty(), "a reload shows nothing");
        assert_eq!(app.world().resource::<CraftOpen>().line, None);
    }
}
