//! The Craft window feed: the client-built book of a craft type (Enchanting, Beast Training),
//! opened by its opener spell with no packet. Reagents and tools come off `Spell.dbc`, and the
//! description's `$` tokens resolve as a tooltip's do. An enchant is an item-targeted cast
//! (`Targets = 0x10`) whose pick is the ordinary targeting cursor ([`crate::spell::targeting`]).

use bevy::prelude::*;

use benilla_formats::{
    SPELL_ATTR_IS_TRADESKILL, SPELL_EFFECT_CREATE_ITEM, SPELL_EFFECT_ENCHANT_ITEM,
    SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, SPELL_EFFECT_LEARN_SPELL,
};
use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{CraftReagent, CraftRecipe, CraftState, CraftTooltip, UiScript};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp, ObjectStore, Objects, SelfPlayer};
use crate::spell::{cast_target, CastCommit, CastLadder};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_items::{count_of, item_icon, InventoryScope};
use crate::ui_script::UiInput;
use crate::ui_spellbook::SkillLines;
use crate::ui_tradeskill::SpellFocus;
use crate::ui_unit::UnitFeed;

/// The open Craft window, client-local. A nonzero opener `EffectMiscValue[0]` routes here
/// (`Spell_C::TryCast` `0x6e4b60`) and is the craft type (1 Beast Training, 3 Enchanting) the
/// client keeps at `0xbdcfb8` for admission and row order. The session end clears it too: a
/// logout's fresh VM never runs the old one's `OnHide`.
#[derive(Resource, Default)]
pub(crate) struct CraftOpen {
    pub(crate) line: Option<u32>,
    pub(crate) craft_type: u32,
}

pub(crate) struct UiCraftPlugin;

impl Plugin for UiCraftPlugin {
    fn build(&self, app: &mut App) {
        app.net_handler(SessionEventKind::Disconnected, on_session_end);
        app.init_resource::<CraftOpen>().add_systems(
            Update,
            (feed_craft.in_set(UnitFeed), drain_craft.after(UiInput)),
        );
    }
}

/// The Craft window dies with the session.
fn on_session_end(In(_): In<SessionEvent>, mut open: ResMut<CraftOpen>) {
    *open = CraftOpen::default();
}

/// The line's `(rank, max, bonus)`: the Craft window bands difficulty on rank plus bonuses
/// (`0x5ea520` in the Craft build `0x4f60c0`), where the TradeSkill window uses the raw rank.
fn skill_rank(store: &ObjectStore, skill_id: u32) -> (u32, u32, i32) {
    for i in 0..PLAYER_SKILL_SLOTS {
        if let Some(s) = store.0.player_skill(i) {
            if u32::from(s.skill_id) == skill_id {
                return (
                    u32::from(s.value),
                    u32::from(s.max),
                    i32::from(s.temp_bonus) + i32::from(s.perm_bonus),
                );
            }
        }
    }
    (0, 0, 0)
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
    script: Option<NonSendMut<UiScript>>,
    open: Res<CraftOpen>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    skill_lines: Option<Res<SkillLines>>,
    focus: Option<Res<SpellFocus>>,
    icons: Option<Res<ItemDisplays>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
    mut last: Local<crate::ui_script::VmMemo<Option<CraftState>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let fresh = (|| -> Option<CraftState> {
        let line = open.line?;
        let craft_type = open.craft_type;
        let spells = spells.as_deref()?;
        let skill_lines = skill_lines.as_deref()?;
        let store = self_store.single().ok()?;
        let (rank, max_rank, bonus) = skill_rank(store, line);
        let effective = rank.saturating_add_signed(bonus);
        let name = skill_lines
            .catalog
            .line(line)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("Skill {line}"));
        let text = crate::ui_script::token_text(&script);
        let ctx = benilla_formats::TokenContext {
            durations: &spells.durations,
            radii: &spells.radii,
            lookup: &|id| spells.catalog.get(id),
            home_area: None,
            text: &text,
        };
        // Admission (`0x5e9c20`): known, not hidden (`Attributes & 0x20`), and `castUI` equal to
        // the craft type. The client walks its per-type list (`CGPlayer_C + 0x1cd0 + 0x10*type`)
        // and never consults `SkillLineAbility` for membership.
        let recipes: Vec<CraftRecipe> = actions
            .spells
            .iter()
            .filter(|&&s| {
                spells.catalog.get(s).is_some_and(|d| {
                    d.cast_ui == craft_type && d.attributes & SPELL_ATTR_IS_TRADESKILL == 0
                })
            })
            .filter_map(|&s| {
                let d = spells.catalog.get(s)?;
                let sla = skill_lines.catalog.ability(s)?;
                let mut reagents = Vec::new();
                let mut num_available = u32::MAX;
                for &(entry, need) in d.reagents.iter().filter(|&&(e, n)| e != 0 && n != 0) {
                    let have = count_of(&store.0, &objects, entry, InventoryScope::CARRIED);
                    // A reagent's icon is the shared item chain (`0x5d88b0`) through its display.
                    let (name, icon) = match items.template(entry, 0, &commands) {
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
                for &t in d.totems.iter().filter(|&&t| t != 0) {
                    let have = count_of(&store.0, &objects, t, InventoryScope::CARRIED) > 0;
                    if let Some(info) = items.template(t, 0, &commands) {
                        tools.push((info.name.clone(), have));
                    }
                }
                let needs_item_target = matches!(
                    d.effects[0],
                    SPELL_EFFECT_ENCHANT_ITEM | SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY
                );
                let icon = craft_icon(d);
                Some(CraftRecipe {
                    spell_id: s,
                    name: d.name.clone(),
                    sub_name: d.rank.clone().unwrap_or_default(),
                    difficulty: crate::ui_tradeskill::difficulty(
                        effective,
                        sla.trivial_low,
                        sla.trivial_high,
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
                })
            })
            .collect();
        // No sort here: the engine orders the rows by the craft type's own comparator
        // (`benilla_ui::script::craft::recipe_order`).
        Some(CraftState {
            name,
            rank,
            max_rank,
            craft_type,
            recipes,
        })
    })();

    if fresh == *last {
        return;
    }
    script.set_craft(fresh.clone());
    // Pre-ask the reagent templates `GetCraftReagentItemLink` reads.
    if let Some(f) = &fresh {
        script.ask_item_templates(
            f.recipes
                .iter()
                .flat_map(|r| r.reagents.iter().map(|re| re.item)),
        );
    }
    match (&*last, &fresh) {
        (None, Some(f)) => {
            debug!("ui_craft: window opens — {} recipe(s)", f.recipes.len());
            script.fire_event("CRAFT_SHOW", vec![]);
        }
        (Some(_), Some(_)) => script.fire_event("CRAFT_UPDATE", vec![]),
        (Some(_), None) => {
            debug!("ui_craft: window closes");
            script.fire_event("CRAFT_CLOSE", vec![]);
        }
        (None, None) => {}
    }
    *last = fresh;
}

/// Drains the Lua intents. `DoCraft` goes down the cast ladder: an enchant's `Targets = 0x10` arms
/// the cursor's item half, a rod craft commits at once. `CloseCraft` closes the window, sends no
/// packet and leaves an armed pick to the ordinary cancels (Escape, right-click, a new cast).
fn drain_craft(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<CraftOpen>,
    skill_lines: Option<Res<SkillLines>>,
    targeting: cast_target::CastTargeting,
    mut ladder: CastLadder,
) {
    let Some(mut script) = script else {
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
    if script.take_craft_close() {
        debug!("ui_craft: client-side close (no packet)");
        open.line = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_formats::SpellDisplay;

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
}
