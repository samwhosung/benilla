//! The crafting book feed: the client-built TradeSkill window around `benilla_ui::script`'s
//! `tradeskill` module. There is no wire: a profession opener (`SPELL_EFFECT_TRADE_SKILL` in
//! `Effect[0]`) never reaches the send, as `Spell_C::TryCast` (`0x6e4b60`) opens the window
//! client-side, and [`TradeSkillOpens`] is that intercept here.
//!
//! The book (`0x4fca20`) lists each known spell with `SPELL_ATTR_IS_TRADESKILL` (`0x20`, which
//! also hides it from the spellbook) whose `SkillLineAbility` row joins the open line, grouped by
//! the created item's `(ItemClass, ItemSubClass)` named from `ItemSubClass.dbc`. A spell-focus
//! tool never reads red: the reference's `hasTool` for a focus is the literal 1.0 (`0x4ffa8b`),
//! as the client holds no focus id or radius, and the server's refusal is the only feedback.

use std::time::Instant;

use bevy::prelude::*;

use benilla_formats::{SpellFocusCatalog, SPELL_ATTR_IS_TRADESKILL, SPELL_EFFECT_CREATE_ITEM};
use benilla_protocol::messages::PLAYER_SKILL_SLOTS;
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::{
    TradeSkillDifficulty, TradeSkillReagent, TradeSkillRecipe, TradeSkillState, UiScript,
};

use crate::creature_anim::{CastEvent, CastEventKind};
use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::net::{NetCommands, NetHandlerApp, ObjectStore, Objects, SelfPlayer};
use crate::spell::{cast_target, CastCommit, CastLadder};
use crate::ui_action::{PlayerActions, Spells};
use crate::ui_items::{count_of, item_icon, InventoryScope};
use crate::ui_script::UiInput;
use crate::ui_spellbook::SkillLines;
use crate::ui_unit::UnitFeed;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// Opener spell ids (effect 47) that `send_spell_cast` intercepted, opened by
/// [`open_trade_skill`] the same frame.
#[derive(Resource, Default)]
pub(crate) struct TradeSkillOpens(pub(crate) Vec<u32>);

/// The open book's skill line. The Lua close and the session end clear it: a logout replaces the
/// VM without running the old `OnHide`, so the close alone never comes.
#[derive(Resource, Default)]
pub(crate) struct TradeSkillOpen {
    pub(crate) line: Option<u32>,
}

/// The Create All repeat (`DoTradeSkill` `0x500280`): latch the count, cast once, and re-cast on
/// each of our own `SMSG_SPELL_GO`s for the spell until dry; a failure, a close or the session end
/// stops it.
#[derive(Resource, Default)]
pub(crate) struct TradeSkillRepeat {
    pub(crate) spell_id: u32,
    pub(crate) remaining: u32,
}

impl TradeSkillRepeat {
    fn clear(&mut self) {
        self.spell_id = 0;
        self.remaining = 0;
    }
}

/// `SpellFocusObject.dbc`, the names on a "Requires: Anvil" line.
#[derive(Resource)]
pub(crate) struct SpellFocus {
    pub(crate) catalog: SpellFocusCatalog,
}

pub(crate) struct UiTradeSkillPlugin;

impl Plugin for UiTradeSkillPlugin {
    fn build(&self, app: &mut App) {
        app.net_handler(SessionEventKind::Disconnected, on_session_end);
        app.init_resource::<TradeSkillOpens>()
            .init_resource::<TradeSkillOpen>()
            .init_resource::<TradeSkillRepeat>()
            .add_systems(Startup, load_spell_focus.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // Opens before the feed, so an opener shows the window this frame; the
                    // drain after the input pass, so a Create click casts this frame.
                    open_trade_skill.before(feed_trade_skill),
                    feed_trade_skill.in_set(UnitFeed),
                    drain_trade_skill.after(UiInput),
                ),
            );
    }
}

/// The book, its repeat and any queued opener end with the session.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut opens: ResMut<TradeSkillOpens>,
    mut open: ResMut<TradeSkillOpen>,
    mut repeat: ResMut<TradeSkillRepeat>,
) {
    opens.0.clear();
    open.line = None;
    repeat.clear();
}

fn load_spell_focus(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_spell_focus_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            debug!("ui_tradeskill: {} spell-focus name(s)", catalog.len());
            commands.insert_resource(SpellFocus { catalog });
        }
        Err(e) => warn!(
            "ui_tradeskill: SpellFocusObject.dbc failed to load — the Requires line drops the \
             focus name: {e:#}"
        ),
    }
}

/// Open the book for each intercepted opener: its line is the opener's `SkillLineAbility` row
/// (3908 Tailoring is line 197), the client's `SkillLineRecIndex_Find` (`0x6de040`).
fn open_trade_skill(
    mut opens: ResMut<TradeSkillOpens>,
    mut open: ResMut<TradeSkillOpen>,
    mut craft_open: ResMut<crate::ui_craft::CraftOpen>,
    skill_lines: Option<Res<SkillLines>>,
    spells: Option<Res<Spells>>,
    mut repeat: ResMut<TradeSkillRepeat>,
) {
    for spell_id in opens.0.drain(..) {
        let Some(skill_lines) = skill_lines.as_deref() else {
            warn!("ui_tradeskill: opener {spell_id} before SkillLineAbility loaded — dropped");
            continue;
        };
        let Some(line) = skill_lines.catalog.spell_to_line(spell_id) else {
            warn!("ui_tradeskill: opener {spell_id} has no SkillLineAbility row — dropped");
            continue;
        };
        // `0x6e4bd7`: a nonzero `EffectMiscValue[0]` opens the CraftFrame (Enchanting 3, Beast
        // Training 1), zero the TradeSkillFrame. The value is the craft type the client keeps at
        // `0xbdcfb8`, which the Craft window filters and sorts on.
        let craft_type = spells
            .as_deref()
            .and_then(|s| s.catalog.get(spell_id))
            .and_then(|d| u32::try_from(d.effect_misc_value[0]).ok())
            .unwrap_or(0);
        if craft_type != 0 {
            debug!(
                "ui_tradeskill: opener {spell_id} opens the CraftFrame (line {line}, craft type {craft_type})"
            );
            craft_open.line = Some(line);
            craft_open.craft_type = craft_type;
        } else {
            debug!("ui_tradeskill: opener {spell_id} opens skill line {line}");
            if open.line != Some(line) {
                repeat.clear();
            }
            open.line = Some(line);
        }
    }
}

/// A skill line's raw `(value, max)` off `PLAYER_SKILL_INFO`, which the rank bar shows as is,
/// where the reference's `GetTradeSkillLine` (`0x4fdd40`) adds the permanent skill bonus to each
/// nonzero half (`0x4fddf7`, `0x4fde6a`).
fn skill_rank(store: &ObjectStore, skill_id: u32) -> (u32, u32) {
    for i in 0..PLAYER_SKILL_SLOTS {
        if let Some(s) = store.0.player_skill(i) {
            if u32::from(s.skill_id) == skill_id {
                return (u32::from(s.value), u32::from(s.max));
            }
        }
    }
    (0, 0)
}

/// The difficulty band (`0x4fcbfc`), with a zero `trivialLow` read as `trivialHigh - 25`; the
/// TradeSkill window passes the raw rank, the Craft window the rank with bonuses.
pub(crate) fn difficulty(rank: u32, low: u32, high: u32) -> TradeSkillDifficulty {
    let low = if low == 0 {
        high.saturating_sub(25)
    } else {
        low
    };
    if rank >= high {
        TradeSkillDifficulty::Trivial
    } else if rank >= (low + high) / 2 {
        TradeSkillDifficulty::Easy
    } else if rank >= low {
        TradeSkillDifficulty::Medium
    } else {
        TradeSkillDifficulty::Optimal
    }
}

/// The row icon, `GetTradeSkillIcon` (`0x4fdae0`): `EffectItemType[0]` as an item id with no
/// `Effect[0]` check, and nil on any miss (a zero id, a template in flight), never the spell's
/// icon; the reference repaints when the item callback fires `TRADE_SKILL_UPDATE`. The Craft
/// window and the trainer resolve icons by other laws; the reference shares no resolver.
fn recipe_icon(
    d: &benilla_formats::SpellDisplay,
    icons: Option<&ItemDisplays>,
    items: &Items,
    commands: &NetCommands,
) -> Option<String> {
    let item = d.effect_item_type[0];
    if item == 0 {
        return None;
    }
    let display = items.template(item, 0, commands)?.display_info_id;
    item_icon(icons, display)
}

/// Build one recipe row; names still in flight are `None` and resolve when the template lands.
fn resolve_recipe(
    spell_id: u32,
    rank: u32,
    spells: &Spells,
    skill_lines: &SkillLines,
    focus: Option<&SpellFocus>,
    icons: Option<&ItemDisplays>,
    subclasses: Option<&crate::ui_items::ItemSubClasses>,
    store: &ObjectStore,
    objects: &Objects,
    items: &Items,
    commands: &NetCommands,
    cooldowns: &crate::spell::Cooldowns,
    now: Instant,
) -> Option<TradeSkillRecipe> {
    let d = spells.catalog.get(spell_id)?;
    let sla = skill_lines.catalog.ability(spell_id)?;

    // Reagents: `(entry, need)` pairs off `Spell.dbc`; have is the carried count.
    let mut reagents = Vec::new();
    let mut num_available = u32::MAX;
    for &(entry, need) in d.reagents.iter().filter(|&&(e, n)| e != 0 && n != 0) {
        let have = count_of(&store.0, objects, entry, InventoryScope::CARRIED);
        let (name, icon) = match items.template(entry, 0, commands) {
            Some(t) => (Some(t.name.clone()), item_icon(icons, t.display_info_id)),
            None => (None, None),
        };
        reagents.push(TradeSkillReagent {
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

    // The product: `CREATE_ITEM`'s slot-0 item, where every probed 5875 recipe has it. Made
    // count (`GetTradeSkillNumMade` `0x4fdc50`): min `BasePoints + BaseDice`, max
    // `BasePoints + DieSides * BaseDice`, at least 1.
    let (product_item, min_made, max_made) = if d.effects[0] == SPELL_EFFECT_CREATE_ITEM {
        let base = d.effect_base_points[0].max(0) as u32;
        let dice = d.effect_base_dice[0].max(0) as u32;
        let die = d.effect_die_sides[0].max(0) as u32;
        let min = (base + dice).max(1);
        (d.effect_item_type[0], min, (base + die * dice).max(min))
    } else {
        (0, 1, 1)
    };
    let icon = recipe_icon(d, icons, items, commands);
    // The header key (`0x4fca20`): the product's `(class, subclass)` named from
    // `ItemSubClass.dbc`, `None` while its template is in flight, as in the client. The same
    // template gives the `InventoryType` the slot filter reads and the `ItemLevel` the sort
    // breaks ties on (`record+0x14`).
    let (group, product_inv_type, product_item_level) = (product_item != 0)
        .then(|| {
            items.template(product_item, 0, commands).map(|t| {
                let name = subclasses
                    .and_then(|sc| sc.0.name(t.class, t.subclass))
                    .unwrap_or_default()
                    .to_string();
                ((t.class, t.subclass, name), t.inventory_type, t.item_level)
            })
        })
        .flatten()
        .map_or((None, 0, 0), |(g, it, il)| (Some(g), it, il));

    // Tools in `0x4ff980`'s order: the spell focus, `Totem[0]`, `Totem[1]`. The focus is always
    // met (its `hasTool` is the literal 1.0); an unmet totem reads red. A tool whose template has
    // not landed is left out, as the reference does while it asks.
    let mut tools = Vec::new();
    if d.requires_spell_focus != 0 {
        if let Some(name) = focus.and_then(|f| f.catalog.name(d.requires_spell_focus)) {
            tools.push((name.to_string(), true));
        }
    }
    for &t in d.totems.iter().filter(|&&t| t != 0) {
        let have = count_of(&store.0, objects, t, InventoryScope::CARRIED) > 0;
        if let Some(info) = items.template(t, 0, commands) {
            tools.push((info.name.clone(), have));
        }
    }

    let cd = cooldowns.info(spell_id, 0, Some(d), now);
    Some(TradeSkillRecipe {
        group,
        spell_id,
        name: d.name.clone(),
        difficulty: difficulty(rank, sla.trivial_low, sla.trivial_high),
        num_available,
        icon,
        min_made,
        max_made,
        cooldown_secs: (cd.remaining_ms > 0).then(|| u64::from(cd.remaining_ms).div_ceil(1000)),
        product_item,
        product_inv_type,
        product_item_level,
        reagents,
        tools,
    })
}

/// Build the book: the known recipes of the open line, banded against the current rank.
fn feed_trade_skill(
    script: Option<NonSendMut<UiScript>>,
    open: Res<TradeSkillOpen>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    skill_lines: Option<Res<SkillLines>>,
    focus: Option<Res<SpellFocus>>,
    icons: Option<Res<ItemDisplays>>,
    subclasses: Option<Res<crate::ui_items::ItemSubClasses>>,
    repeat: Res<TradeSkillRepeat>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
    cooldowns: Res<crate::spell::Cooldowns>,
    mut last: Local<crate::ui_script::VmMemo<Option<TradeSkillState>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let fresh = (|| -> Option<TradeSkillState> {
        let line = open.line?;
        let spells = spells.as_deref()?;
        let skill_lines = skill_lines.as_deref()?;
        let store = self_store.single().ok()?;
        let (rank, max_rank) = skill_rank(store, line);
        let line_name = skill_lines
            .catalog
            .line(line)
            .map(|l| l.name.clone())
            .unwrap_or_else(|| format!("Skill {line}"));
        let now = Instant::now();
        let recipes: Vec<TradeSkillRecipe> = actions
            .spells
            .iter()
            .filter(|&&s| {
                spells
                    .catalog
                    .get(s)
                    .is_some_and(|d| d.attributes & SPELL_ATTR_IS_TRADESKILL != 0)
                    && skill_lines.catalog.spell_to_line(s) == Some(line)
            })
            .filter_map(|&s| {
                resolve_recipe(
                    s,
                    rank,
                    spells,
                    skill_lines,
                    focus.as_deref(),
                    icons.as_deref(),
                    subclasses.as_deref(),
                    store,
                    &objects,
                    &items,
                    &commands,
                    &cooldowns,
                    now,
                )
            })
            .collect();
        // No sort here: the engine orders the book (`0x4fd180`).
        Some(TradeSkillState {
            line,
            line_name,
            rank,
            max_rank,
            recipes,
            repeat_count: repeat.remaining,
        })
    })();

    let repeat_changed = match (&*last, &fresh) {
        (Some(l), Some(f)) => l.repeat_count != f.repeat_count,
        _ => false,
    };
    if fresh == *last {
        return;
    }
    script.set_trade_skill(fresh.clone());
    // The link verbs never query, as the client has every template cached by the time the list
    // shows, so the feed asks for the missing ones here.
    if let Some(f) = &fresh {
        script.ask_item_templates(f.recipes.iter().flat_map(|r| {
            std::iter::once(r.product_item).chain(r.reagents.iter().map(|re| re.item))
        }));
    }
    match (&*last, &fresh) {
        (None, Some(f)) => {
            debug!(
                "ui_tradeskill: window opens — {} recipe(s), first group {:?}",
                f.recipes.len(),
                f.recipes.first().and_then(|r| r.group.clone())
            );
            script.fire_event("TRADE_SKILL_SHOW", vec![]);
        }
        (Some(_), Some(_)) => {
            script.fire_event("TRADE_SKILL_UPDATE", vec![]);
            if repeat_changed {
                script.fire_event("UPDATE_TRADESKILL_RECAST", vec![]);
            }
        }
        (Some(_), None) => script.fire_event("TRADE_SKILL_CLOSE", vec![]),
        (None, None) => {}
    }
    *last = fresh;
}

/// Drain the Lua intents and run the repeat: `DoTradeSkill` latches and casts through the one
/// cast-send path, our own GO re-casts, and a failure or `CloseTradeSkill` stops it.
fn drain_trade_skill(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<TradeSkillOpen>,
    mut repeat: ResMut<TradeSkillRepeat>,
    mut cast_events: MessageReader<CastEvent>,
    targeting: cast_target::CastTargeting,
    mut ladder: CastLadder,
) {
    let Some(mut script) = script else {
        return;
    };

    for (spell_id, count) in script.take_trade_skill_dos() {
        if open.line.is_none() {
            debug!("ui_tradeskill: DoTradeSkill({spell_id}) with no open book — ignored");
            continue;
        }
        debug!("ui_tradeskill: DoTradeSkill({spell_id}) ×{count}");
        repeat.spell_id = spell_id;
        repeat.remaining = count.max(1);
        ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
    }

    // The repeat's continuation: our own cast edges for the latched spell.
    let self_entity = ladder.self_player.single().ok().map(|(e, _)| e);
    for ev in cast_events.read() {
        if repeat.remaining == 0 || Some(ev.entity) != self_entity || ev.spell_id != repeat.spell_id
        {
            continue;
        }
        match ev.kind {
            CastEventKind::Go => {
                repeat.remaining -= 1;
                if repeat.remaining > 0 {
                    debug!(
                        "ui_tradeskill: recast {} ({} left)",
                        repeat.spell_id, repeat.remaining
                    );
                    ladder.send(repeat.spell_id, &targeting.context(), CastCommit::Spell);
                }
            }
            CastEventKind::Fail => {
                debug!("ui_tradeskill: cast failed — repeat stops");
                repeat.clear();
            }
            _ => {}
        }
    }

    // A filter or expand/collapse fires `TRADE_SKILL_UPDATE` from inside the reference's C call
    // (`0x4fd710`, `0x4fd750`); ours is the touched flag, answered the same frame.
    let touched = script.take_trade_skill_touched();
    if touched && open.line.is_some() {
        script.fire_event("TRADE_SKILL_UPDATE", vec![]);
    }

    if script.take_trade_skill_close() {
        debug!("ui_tradeskill: client-side close (no packet)");
        open.line = None;
        repeat.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{test_template, TestDeps};
    use benilla_formats::{
        ItemDisplay, ItemDisplayCatalog, SpellDisplay, SPELL_EFFECT_ENCHANT_ITEM,
    };
    use std::collections::HashMap;

    /// A recipe with its own spell icon, so a wrong arm names itself in the assertion.
    fn recipe(effect: u32, item: u32) -> SpellDisplay {
        SpellDisplay {
            name: "Runed Copper Breastplate".into(),
            icon: Some("SPELL".into()),
            effects: [effect, 0, 0],
            effect_item_type: [item, 0, 0],
            ..Default::default()
        }
    }

    /// Item 777's template and `ItemDisplayInfo` row, landed.
    fn landed_item(deps: &mut TestDeps) -> ItemDisplays {
        let mut t = test_template("Runed Copper Breastplate");
        t.display_info_id = 5;
        deps.items.insert_template(777, Some(t));
        ItemDisplays::icons_for_tests(ItemDisplayCatalog::from_displays(HashMap::from([(
            5,
            ItemDisplay {
                icon: Some("ITEM".into()),
                ..Default::default()
            },
        )])))
    }

    #[test]
    fn law_c_paints_the_created_items_icon() {
        let mut deps = TestDeps::new();
        let icons = landed_item(&mut deps);
        let d = recipe(SPELL_EFFECT_CREATE_ITEM, 777);
        assert_eq!(
            recipe_icon(&d, Some(&icons), &deps.items, &deps.commands),
            Some("ITEM".into()),
        );
    }

    /// `0x4fdae0` never reads `Effect[0]`.
    #[test]
    fn law_c_does_not_gate_on_the_effect_type() {
        let mut deps = TestDeps::new();
        let icons = landed_item(&mut deps);
        let d = recipe(SPELL_EFFECT_ENCHANT_ITEM, 777);
        assert_eq!(
            recipe_icon(&d, Some(&icons), &deps.items, &deps.commands),
            Some("ITEM".into()),
        );
    }

    /// The binding's last return is a bare `lua_pushnil`.
    #[test]
    fn law_c_pushes_nil_on_every_miss_never_the_spells_icon() {
        let mut deps = TestDeps::new();
        let icons = landed_item(&mut deps);

        // EffectItemType[0] == 0: 0x55ba30 short-circuits on a zero id before hashing.
        let none = recipe(SPELL_EFFECT_ENCHANT_ITEM, 0);
        assert_eq!(
            recipe_icon(&none, Some(&icons), &deps.items, &deps.commands),
            None,
        );

        // A template that never lands: nil, asked for once.
        let missing = recipe(SPELL_EFFECT_CREATE_ITEM, 999);
        assert_eq!(
            recipe_icon(&missing, Some(&icons), &deps.items, &deps.commands),
            None,
        );
        assert_eq!(
            recipe_icon(&missing, Some(&icons), &deps.items, &deps.commands),
            None,
        );
        assert_eq!(deps.queried_entries(), vec![999], "ask-once, not ask-often");

        // Template landed, `ItemDisplayInfo` missing: still nil.
        let d = recipe(SPELL_EFFECT_CREATE_ITEM, 777);
        assert_eq!(recipe_icon(&d, None, &deps.items, &deps.commands), None);
    }

    /// Driven through the real handler registration.
    #[test]
    fn the_session_end_closes_the_book_and_drops_the_repeat() {
        let mut app = App::new();
        app.add_plugins(UiTradeSkillPlugin);
        app.world_mut().resource_mut::<TradeSkillOpen>().line = Some(197);
        {
            let mut repeat = app.world_mut().resource_mut::<TradeSkillRepeat>();
            repeat.spell_id = 3915;
            repeat.remaining = 4;
        }

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "logged out".into(),
                end: benilla_protocol::SessionEnd::LoggedOut,
            }],
        );

        assert_eq!(app.world().resource::<TradeSkillOpen>().line, None);
        let repeat = app.world().resource::<TradeSkillRepeat>();
        assert_eq!((repeat.spell_id, repeat.remaining), (0, 0));
    }
}
