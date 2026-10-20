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
use benilla_ui::script::{CraftReagent, CraftRecipe, CraftState, CraftTooltip};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp, ObjectStore, Objects, SelfPlayer};
use crate::query_cache::Ask;
use crate::spell::{cast_target, CastCommit, CastLadder};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_items::{count_of, item_icon, InventoryScope};
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
    mut hosted_vm: crate::script_calls::HostedVm,
    open: Res<CraftOpen>,
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
    mut last: Local<crate::ui_script::VmMemo<Option<CraftState>>>,
) {
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        let last = last.get(script);
        // The show stands for the stock frame's getter calls, whose lookups pass the callback
        // `0x4f7710` (`0x4f75f2`, `0x4f7a0a`); the rebuilds between read.
        let ask = Ask::at(last.is_none() || open.is_changed(), &commands);
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
            let global = |key: &str| benilla_ui::strings::global(script.lua(), key);
            let ctx = benilla_formats::TokenContext {
                durations: &spells.durations,
                radii: &spells.radii,
                ranges: Some(&spells.ranges),
                skill: &|id| {
                    crate::spell::spell_skill_value(Some(store), Some(&skill_lines.catalog), id)
                },
                lookup: &|id| spells.catalog.get(id),
                mods: Some(&*spell_mods),
                unmodified_points: false,
                gender: &|| store.0.unit_gender().unwrap_or(0),
                home_area: &|| None,
                global: &global,
                printf: &crate::ui_script::token_printf,
            };
            // Admission (`0x5e9c20`): known, not hidden (`Attributes & 0x20`), and `castUI` equal
            // to the craft type. The client walks its per-type list (`CGPlayer_C + 0x1cd0 +
            // 0x10*type`) and never consults `SkillLineAbility` for membership.
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
                    for &t in d.totems.iter().filter(|&&t| t != 0) {
                        let have = count_of(&store.0, &objects, t, InventoryScope::DEFAULT) > 0;
                        if let Some(info) = items.template(t, 0, ask) {
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
                        // The group key is the ability row's line (`0x4f61b1`).
                        skill_line: skill_lines
                            .catalog
                            .line(sla.skill_id)
                            .map(|l| l.name.clone()),
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

        // `CloseCraft` closes the VM's window and signals `CRAFT_CLOSE` in the verb (`0x4f5fef`).
        let closed_in_place = !script.craft_open();
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
            (Some(_), None) if !closed_in_place => {
                debug!("ui_craft: window closes");
                script.fire_event("CRAFT_CLOSE", vec![]);
            }
            (Some(_), None) | (None, None) => {}
        }
        *last = fresh;
    })
}

/// Drains the Lua intents. `DoCraft` goes down the cast ladder: an enchant's `Targets = 0x10` arms
/// the cursor's item half, a rod craft commits at once. `CloseCraft` closes the window, sends no
/// packet and leaves an armed pick to the ordinary cancels (Escape, right-click, a new cast).
fn drain_craft(
    mut vm: crate::script_calls::VmWith<(
        cast_target::CastTargeting<'static, 'static>,
        CastLadder<'static, 'static>,
    )>,
    mut open: ResMut<CraftOpen>,
    skill_lines: Option<Res<SkillLines>>,
) {
    vm.turn(|script, (targeting, mut ladder)| {
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
        if script.take_craft_close() {
            debug!("ui_craft: client-side close (no packet)");
            open.line = None;
        }
    })
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
        let script = UiScript::new().unwrap();
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
}
