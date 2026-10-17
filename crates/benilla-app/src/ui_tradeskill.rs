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
use crate::query_cache::Ask;
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

/// The repeat count (`[0xbde060]`) a dying VM left, seated in the next: the world-enter init a
/// `/reload` runs disarms the spell (`0x4fc76b`) but keeps the count. The session end drops it.
#[derive(Resource, Default)]
pub(crate) struct TradeSkillRepeatCarry(pub(crate) u32);

/// Fold the repeat count out of the dying VM, with the CVar fold.
pub(crate) fn fold_dying_vm_repeat(world: &mut World) {
    let Some((_, count)) = world
        .get_non_send_resource::<UiScript>()
        .map(UiScript::trade_skill_repeat)
    else {
        return;
    };
    if let Some(mut carry) = world.get_resource_mut::<TradeSkillRepeatCarry>() {
        carry.0 = count;
    }
}

/// Seat the folded count in a new in-game VM before its load runs; the spell stays disarmed.
pub(crate) fn seed_repeat(world: &World, script: &mut UiScript) {
    if let Some(carry) = world.get_resource::<TradeSkillRepeatCarry>() {
        script.seed_trade_skill_repeat_count(carry.0);
    }
}

/// The world-enter inits for the craft windows (`0x4fc750` at `0x4909bf`, `0x4f5ee0` at
/// `0x4909c4`) on each cascade, the worldport's included, whose VM survives: the lists and
/// selections go and the armed spell disarms silently, the repeat count kept.
pub(crate) fn reset_crafts_on_world_enter(
    script: Option<NonSendMut<UiScript>>,
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
) {
    if cascades.read().next().is_none() {
        return;
    }
    if let Some(mut script) = script {
        script.reset_crafts_on_world_enter();
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
            .init_resource::<TradeSkillRepeatCarry>()
            .add_systems(Startup, load_spell_focus.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // Opens before the feed, so an opener shows the window this frame; the
                    // drain after the input pass, so a Create click casts this frame.
                    open_trade_skill.before(feed_trade_skill),
                    feed_trade_skill.in_set(UnitFeed),
                    drain_trade_skill.after(UiInput),
                    // In the feed phase, ahead of every feed that pushes the lists.
                    reset_crafts_on_world_enter.in_set(crate::ui_script::UiFeed),
                ),
            );
    }
}

/// The book, any queued opener and the carried repeat end with the session.
fn on_session_end(
    In(_): In<SessionEvent>,
    mut opens: ResMut<TradeSkillOpens>,
    mut open: ResMut<TradeSkillOpen>,
    mut carry: ResMut<TradeSkillRepeatCarry>,
) {
    opens.0.clear();
    open.line = None;
    carry.0 = 0;
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
    ask: Ask<'_>,
) -> Option<String> {
    let item = d.effect_item_type[0];
    if item == 0 {
        return None;
    }
    let display = items.template(item, 0, ask)?.display_info_id;
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
    ask: Ask<'_>,
    cooldowns: &crate::spell::Cooldowns,
    // The frame's one clock pair ([`crate::ui_script::UiClock`]): a running cooldown then pushes
    // the same start every frame.
    (anchor, ui_now): (Instant, f64),
) -> Option<TradeSkillRecipe> {
    let d = spells.catalog.get(spell_id)?;
    let sla = skill_lines.catalog.ability(spell_id)?;

    // Reagents: `(entry, need)` pairs off `Spell.dbc`; have is the mask-0 count (`0x622130`, `0x4fccfc`).
    let mut reagents = Vec::new();
    let mut num_available = u32::MAX;
    for &(entry, need) in d.reagents.iter().filter(|&&(e, n)| e != 0 && n != 0) {
        let have = count_of(&store.0, objects, entry, InventoryScope::DEFAULT);
        let (name, icon) = match items.template(entry, 0, ask) {
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
    let icon = recipe_icon(d, icons, items, ask);
    // The header key (`0x4fca20`): the product's `(class, subclass)` named from
    // `ItemSubClass.dbc`, `None` while its template is in flight, as in the client. The same
    // template gives the `InventoryType` the slot filter reads and the `ItemLevel` the sort
    // breaks ties on (`record+0x14`).
    let (group, product_inv_type, product_item_level) = (product_item != 0)
        .then(|| {
            items.template(product_item, 0, ask).map(|t| {
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
        let have = count_of(&store.0, objects, t, InventoryScope::DEFAULT) > 0;
        if let Some(info) = items.template(t, 0, ask) {
            tools.push((info.name.clone(), have));
        }
    }

    let cd = cooldowns.info(spell_id, 0, Some(d), anchor);
    Some(TradeSkillRecipe {
        group,
        spell_id,
        name: d.name.clone(),
        difficulty: difficulty(rank, sla.trivial_low, sla.trivial_high),
        num_available,
        icon,
        min_made,
        max_made,
        cooldown: cd.ui_triple(anchor, ui_now),
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
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    objects: Objects,
    items: Res<Items>,
    commands: Res<NetCommands>,
    cooldowns: Res<crate::spell::Cooldowns>,
    clock: Res<crate::ui_script::UiClock>,
    mut last: Local<crate::ui_script::VmMemo<Option<TradeSkillState>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    // The list's build looks every record up with the callback `0x4fc8e0` (`0x4fcd4d`) at the
    // window's show; the rebuilds between read, as the getters' lookups stand for the stock
    // frame's calls (`0x4fdba8`), which the asks below make.
    let ask = Ask::at(last.is_none() || open.is_changed(), &commands);
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
        let now = (clock.anchor, clock.ui_now);
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
                    ask,
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
        })
    })();

    // `CloseTradeSkill` closes the VM's window and signals `TRADE_SKILL_CLOSE` in the verb
    // (`0x4fc884`).
    let closed_in_place = !script.trade_skill_open();
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
        (Some(_), Some(_)) => script.fire_event("TRADE_SKILL_UPDATE", vec![]),
        (Some(_), None) if !closed_in_place => script.fire_event("TRADE_SKILL_CLOSE", vec![]),
        (Some(_), None) | (None, None) => {}
    }
    *last = fresh;
}

/// Drain the Lua intents and feed the VM's repeat (`0x4fd7b0`) our own cast edges: `DoTradeSkill`
/// arms it in the verb and casts through the one cast-send path, each GO recasts what the hook
/// answers, and a failure disarms it.
fn drain_trade_skill(
    script: Option<NonSendMut<UiScript>>,
    mut open: ResMut<TradeSkillOpen>,
    mut cast_events: MessageReader<CastEvent>,
    targeting: cast_target::CastTargeting,
    mut ladder: CastLadder,
    mut seen_aborts: Local<u32>,
) {
    let Some(mut script) = script else {
        return;
    };

    // The verb reads the held list, not the open marker (`0x5002e1`, `0x5002e9`), so a recipe
    // still held after a close casts (`0x500314`).
    for (spell_id, count) in script.take_trade_skill_dos() {
        debug!("ui_tradeskill: DoTradeSkill({spell_id}) ×{count}");
        ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
    }

    // A cancelled targeting cursor is the cast abort with its disarm flag (`StopTargeting
    // 0x6e4900`, `TryCast`'s supersede `0x6e4d71`, both `0x6e4940(cl=1)` → `0x4fd790`).
    let aborts = ladder.ground.aborts();
    if *seen_aborts != aborts {
        *seen_aborts = aborts;
        script.trade_skill_disarm();
    }

    let self_entity = ladder.self_player.single().ok().map(|(e, _)| e);
    for ev in cast_events.read() {
        if Some(ev.entity) != self_entity {
            continue;
        }
        if let Some(spell_id) = repeat_edge(&mut script, ev) {
            debug!(
                "ui_tradeskill: recast {spell_id} ({} left)",
                script.trade_skill_repeat().1
            );
            // The recast, then its signal (`0x4fd7d7`, `0x4fd7dc`).
            ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
            script.trade_skill_recast_sent();
        }
    }

    if script.take_trade_skill_close() {
        debug!("ui_tradeskill: client-side close (no packet)");
        open.line = None;
    }
}

/// One of our own cast edges into the VM's repeat: every GO whose first guid is the player, not
/// an item, and without flag `0x41` runs the hook (`0x6e81d3`-`0x6e81de` → `0x4fd7b0`), and a
/// failure disarms the armed spell (`0x6e1a3c`). Returns the spell to recast.
fn repeat_edge(script: &mut UiScript, ev: &CastEvent) -> Option<u32> {
    match ev.kind {
        CastEventKind::Go if !ev.item_cast && ev.cast_flags & 0x41 == 0 => {
            script.trade_skill_spell_go(ev.spell_id)
        }
        CastEventKind::Fail => {
            script.trade_skill_cast_failed(ev.spell_id);
            None
        }
        _ => None,
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
            recipe_icon(&d, Some(&icons), &deps.items, Ask::New(&deps.commands)),
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
            recipe_icon(&d, Some(&icons), &deps.items, Ask::New(&deps.commands)),
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
            recipe_icon(&none, Some(&icons), &deps.items, Ask::New(&deps.commands)),
            None,
        );

        // A template that never lands: nil, asked for once.
        let missing = recipe(SPELL_EFFECT_CREATE_ITEM, 999);
        assert_eq!(
            recipe_icon(
                &missing,
                Some(&icons),
                &deps.items,
                Ask::New(&deps.commands)
            ),
            None,
        );
        assert_eq!(
            recipe_icon(
                &missing,
                Some(&icons),
                &deps.items,
                Ask::New(&deps.commands)
            ),
            None,
        );
        assert_eq!(deps.queried_entries(), vec![999], "ask-once, not ask-often");

        // Template landed, `ItemDisplayInfo` missing: still nil.
        let d = recipe(SPELL_EFFECT_CREATE_ITEM, 777);
        assert_eq!(
            recipe_icon(&d, None, &deps.items, Ask::New(&deps.commands)),
            None
        );
    }

    /// No `TRADE_SKILL_UPDATE` sender runs off a cooldown's clock (`0x4fd164` in the rebuild
    /// `0x4fca20`, and the filter setters `0x4fd725`, `0x4fd745`, `0x4fd765`); the getter reads the
    /// seconds left at its call (`0x4fda20`). So a running cooldown pushes the same recipe every
    /// frame, and the snapshot diff, which fires the event, never sees it tick.
    #[test]
    fn a_running_cooldown_leaves_the_recipe_unchanged_frame_to_frame() {
        use std::time::Duration;
        const SPELL: u32 = 18_560; // Mooncloth, a day-long cooldown
        let mut deps = TestDeps::new();
        let mooncloth = || SpellDisplay {
            name: "Mooncloth".into(),
            recovery_ms: 86_400_000,
            ..Default::default()
        };
        let d = mooncloth();
        let spells = Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(SPELL, mooncloth())].into_iter().collect(),
            ),
            ..Spells::empty_for_tests()
        };
        let lines = SkillLines {
            catalog: benilla_formats::SkillLineCatalog::from_abilities([(
                SPELL,
                benilla_formats::SlaInfo {
                    skill_id: 197,
                    req_skill_value: 1,
                    forward_spell_id: 0,
                    trivial_low: 1,
                    trivial_high: 2,
                },
            )]),
        };
        let t0 = Instant::now();
        let mut cooldowns = crate::spell::Cooldowns::default();
        cooldowns.start_spell(SPELL, &d, 0, t0, None);
        let store = ObjectStore(benilla_protocol::ObjectFields::default());
        let mut frame = |elapsed: Duration| {
            deps.with_objects(|objects, items, commands| {
                resolve_recipe(
                    SPELL,
                    0,
                    &spells,
                    &lines,
                    None,
                    None,
                    None,
                    &store,
                    objects,
                    items,
                    Ask::New(commands),
                    &cooldowns,
                    (t0 + elapsed, 50.0 + elapsed.as_secs_f64()),
                )
                .expect("a recipe")
            })
        };
        let first = frame(Duration::from_millis(16));
        assert_eq!(first.cooldown, Some((50_000, 86_400_000, true)));
        for secs in [1, 2, 61, 3_600] {
            assert_eq!(
                frame(Duration::from_secs(secs)),
                first,
                "{secs} s on: the same push"
            );
        }
    }

    /// `0x4fccfc` counts a reagent with `0x622130` and `0x4ffb10` finds a tool with `0x622270`,
    /// both at mask 0: a copy worn in a slot or in the keyring counts, one in the bank does not.
    #[test]
    fn a_recipe_counts_a_worn_copy_and_the_keyring_but_not_the_bank() {
        use benilla_protocol::field::{FIELD_ITEM_STACK_COUNT, FIELD_OBJECT_ENTRY};
        use benilla_protocol::ObjectFields;
        const DUST: u32 = 10_940;
        const ROD: u32 = 6_218;
        const SPELL: u32 = 7_000;
        let mut deps = TestDeps::new();
        deps.items
            .insert_template(DUST, Some(test_template("Strange Dust")));
        deps.items
            .insert_template(ROD, Some(test_template("Runed Rod")));
        for (guid, entry, n) in [
            (0x4000_0000_0000_0001u64, DUST, 2u32),
            (0x4000_0000_0000_0002, ROD, 1),
        ] {
            deps.spawn_item(
                guid,
                ObjectFields::from_pairs(&[
                    (FIELD_OBJECT_ENTRY, entry),
                    (FIELD_ITEM_STACK_COUNT, n),
                ]),
            );
        }
        let spells = Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(
                    SPELL,
                    SpellDisplay {
                        name: "Recipe".into(),
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
            ..Spells::empty_for_tests()
        };
        let lines = SkillLines {
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
        };
        let cooldowns = crate::spell::Cooldowns::default();
        let mut read = |dust_slot: u16, rod_slot: u16| -> (u32, bool) {
            let seat = |slot: u16, guid: u64| {
                let lo = 486 + 2 * slot;
                [(lo, guid as u32), (lo + 1, (guid >> 32) as u32)]
            };
            let store = ObjectStore(ObjectFields::from_pairs(
                &[
                    seat(dust_slot, 0x4000_0000_0000_0001),
                    seat(rod_slot, 0x4000_0000_0000_0002),
                ]
                .concat(),
            ));
            deps.with_objects(|objects, items, commands| {
                let r = resolve_recipe(
                    SPELL,
                    0,
                    &spells,
                    &lines,
                    None,
                    None,
                    None,
                    &store,
                    objects,
                    items,
                    Ask::New(commands),
                    &cooldowns,
                    (Instant::now(), 0.0),
                )
                .expect("a recipe");
                (r.reagents[0].have, r.tools[0].1)
            })
        };
        assert_eq!(read(23, 24), (2, true), "the backpack, as before");
        assert_eq!(read(17, 18), (2, true), "worn copies");
        assert_eq!(read(81, 82), (2, true), "the keyring");
        assert_eq!(read(39, 40), (0, false), "the bank is outside mask 0");
    }

    /// `CloseTradeSkill` signals `TRADE_SKILL_CLOSE` in the verb (`0x4fc884`), so the feed that
    /// finds the book closed a frame later signals no second close; a close the VM did not make
    /// still signals from the feed.
    #[test]
    fn a_verbs_close_is_signalled_once() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<TradeSkillOpen>()
            .init_resource::<PlayerActions>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::ui_script::UiClock>()
            .insert_resource(crate::net::NetCommands(tx))
            .insert_resource(Spells::empty_for_tests())
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_spell_lines([]),
            });
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        let script = UiScript::new().unwrap();
        script
            .run(
                "CLOSES = 0 local f = CreateFrame('Frame') f:RegisterEvent('TRADE_SKILL_CLOSE') \
                 f:SetScript('OnEvent', function() CLOSES = CLOSES + 1 end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.add_systems(Update, feed_trade_skill);
        let closes = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<i64>("return CLOSES")
                .unwrap()
        };

        app.world_mut().resource_mut::<TradeSkillOpen>().line = Some(197);
        app.update();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("CloseTradeSkill()")
            .unwrap();
        assert_eq!(closes(&app), 1, "the verb's own signal");
        app.world_mut().resource_mut::<TradeSkillOpen>().line = None;
        app.update();
        assert_eq!(closes(&app), 1, "no second signal from the feed");

        app.world_mut().resource_mut::<TradeSkillOpen>().line = Some(197);
        app.update();
        app.world_mut().resource_mut::<TradeSkillOpen>().line = None;
        app.update();
        assert_eq!(closes(&app), 2, "a close from outside the VM still signals");
    }

    /// Our GOs count the batch down and recast, a GO flagged `0x41` is not ours to count
    /// (`0x6e81de`), and the last craft recasts and signals nothing (`0x4fd7cf`).
    #[test]
    fn our_cast_edges_drive_the_vms_repeat() {
        let mut script = UiScript::new().unwrap();
        script.set_trade_skill(Some(TradeSkillState {
            line: 197,
            line_name: "Tailoring".into(),
            rank: 1,
            max_rank: 75,
            recipes: vec![benilla_ui::script::TradeSkillRecipe {
                group: None,
                spell_id: 2963,
                name: "Bolt of Linen Cloth".into(),
                difficulty: benilla_ui::script::TradeSkillDifficulty::Trivial,
                num_available: 5,
                icon: None,
                min_made: 1,
                max_made: 1,
                cooldown: None,
                product_item: 2996,
                product_inv_type: 0,
                product_item_level: 0,
                reagents: vec![],
                tools: vec![],
            }],
        }));
        script
            .run(
                "RECASTS = 0 local f = CreateFrame('Frame') \
                 f:RegisterEvent('UPDATE_TRADESKILL_RECAST') \
                 f:SetScript('OnEvent', function() RECASTS = RECASTS + 1 end) \
                 DoTradeSkill(2, 3)",
            )
            .unwrap();
        assert_eq!(
            script.trade_skill_repeat(),
            (2963, 3),
            "row 2, under its header"
        );
        let go_from = |cast_flags, item_cast| CastEvent {
            entity: Entity::PLACEHOLDER,
            spell_id: 2963,
            kind: CastEventKind::Go,
            cast_flags,
            item_cast,
            seq: 0,
        };
        let go = |cast_flags| go_from(cast_flags, false);
        assert_eq!(repeat_edge(&mut script, &go(0x01)), None, "flag 0x01");
        assert_eq!(repeat_edge(&mut script, &go(0x40)), None, "flag 0x40");
        assert_eq!(
            repeat_edge(&mut script, &go_from(0, true)),
            None,
            "an item's GO"
        );
        assert_eq!(script.trade_skill_repeat(), (2963, 3), "none counted");
        // The drain's order: the recast, then its signal (`0x4fd7d7`, `0x4fd7dc`).
        let edge = |script: &mut UiScript, ev: &CastEvent| {
            let recast = repeat_edge(script, ev);
            if recast.is_some() {
                script.trade_skill_recast_sent();
            }
            recast
        };
        assert_eq!(edge(&mut script, &go(0x20)), Some(2963));
        assert_eq!(edge(&mut script, &go(0)), Some(2963));
        assert_eq!(edge(&mut script, &go(0)), None);
        assert_eq!(
            script.eval::<i64>("return RECASTS").unwrap(),
            3,
            "the arm and two recasts; the last craft is silent"
        );
    }

    /// The whole client headless, our player seated, Tailoring's Bolt of Linen Cloth (2963) in the
    /// catalog and the VM's book, and the commands it sends.
    fn headless_tailor() -> (App, crossbeam_channel::Receiver<crate::net::ClientCommand>) {
        use crate::net::{Guid, GuidIndex, SelfGuid};
        const ME: u64 = 0x77;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        let me = world
            .spawn((
                Guid(ME),
                SelfPlayer,
                Transform::default(),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (22, 100),
                    (28, 100),
                ])),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(ME, me);
        world.insert_resource(Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(HashMap::from([(
                2963,
                SpellDisplay {
                    name: "Bolt of Linen Cloth".into(),
                    ..Default::default()
                },
            )])),
            ..Spells::empty_for_tests()
        });
        let mut script = UiScript::new().unwrap();
        script.set_trade_skill(Some(TradeSkillState {
            line: 197,
            line_name: "Tailoring".into(),
            rank: 1,
            max_rank: 75,
            recipes: vec![benilla_ui::script::TradeSkillRecipe {
                group: None,
                spell_id: 2963,
                name: "Bolt of Linen Cloth".into(),
                difficulty: benilla_ui::script::TradeSkillDifficulty::Trivial,
                num_available: 5,
                icon: None,
                min_made: 1,
                max_made: 1,
                cooldown: None,
                product_item: 2996,
                product_inv_type: 0,
                product_item_level: 0,
                reagents: vec![],
                tools: vec![],
            }],
        }));
        world.insert_non_send_resource(script);
        (app, rx)
    }

    fn drain(app: &mut App) {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .run_system_once(drain_trade_skill)
            .expect("the drain runs");
    }

    /// `DoTradeSkill` reads the held list, not the open marker (`0x5002e1`), and casts
    /// (`0x500314`): after `CloseTradeSkill` a held recipe still goes out.
    #[test]
    fn a_craft_after_the_close_still_casts() {
        let (mut app, rx) = headless_tailor();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("CloseTradeSkill() DoTradeSkill(2, 1)")
            .unwrap();
        drain(&mut app);
        assert!(
            rx.try_iter().any(|c| matches!(
                c,
                crate::net::ClientCommand::CastSpell { spell_id: 2963, .. }
            )),
            "the craft was dropped"
        );
    }

    /// A cancelled targeting cursor is the cast abort with its disarm flag (`0x6e4900`,
    /// `0x6e4940(cl=1)` → `0x4fd790`): the armed batch disarms, with its signal.
    #[test]
    fn a_cancelled_cursor_disarms_the_batch() {
        let (mut app, _rx) = headless_tailor();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("DoTradeSkill(2, 3)")
            .unwrap();
        drain(&mut app);
        assert_eq!(
            app.world()
                .non_send_resource::<UiScript>()
                .trade_skill_repeat(),
            (2963, 3)
        );
        {
            let mut ground = app
                .world_mut()
                .resource_mut::<crate::spell::targeting::SpellTargeting>();
            ground.cancel();
            assert_eq!(
                ground.aborts(),
                0,
                "nothing up, nothing aborts (`0x6e4907`)"
            );
            ground.enter(133, crate::spell::CastCommit::Spell, 0x2);
            ground.cancel();
        }
        drain(&mut app);
        assert_eq!(
            app.world()
                .non_send_resource::<UiScript>()
                .trade_skill_repeat(),
            (0, 3)
        );
    }

    /// The world-enter inits (`0x4fc750`, `0x4f5ee0`) on a worldport's cascade, whose VM
    /// survives: the lists empty, the spell disarms silently and the count stays.
    #[test]
    fn the_world_enter_cascade_empties_the_lists_and_disarms_silently() {
        let mut app = App::new();
        app.add_message::<crate::net::WorldEnterCascadeMessage>()
            .add_systems(Update, reset_crafts_on_world_enter);
        let mut script = UiScript::new().unwrap();
        script.set_trade_skill(Some(TradeSkillState {
            line: 197,
            line_name: "Tailoring".into(),
            rank: 1,
            max_rank: 75,
            recipes: vec![benilla_ui::script::TradeSkillRecipe {
                group: None,
                spell_id: 2963,
                name: "Bolt of Linen Cloth".into(),
                difficulty: benilla_ui::script::TradeSkillDifficulty::Trivial,
                num_available: 5,
                icon: None,
                min_made: 1,
                max_made: 1,
                cooldown: None,
                product_item: 2996,
                product_inv_type: 0,
                product_item_level: 0,
                reagents: vec![],
                tools: vec![],
            }],
        }));
        script
            .run(
                "RECASTS = 0 local f = CreateFrame('Frame') \
                 f:RegisterEvent('UPDATE_TRADESKILL_RECAST') \
                 f:SetScript('OnEvent', function() RECASTS = RECASTS + 1 end) \
                 DoTradeSkill(2, 3)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.update();
        let count = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<i64>("return GetNumTradeSkills()")
                .unwrap()
        };
        assert_eq!(count(&app), 2, "no cascade, no reset");
        app.world_mut()
            .write_message(crate::net::WorldEnterCascadeMessage);
        app.update();
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(count(&app), 0);
        assert_eq!(script.eval::<i64>("return GetNumCrafts()").unwrap(), 0);
        assert_eq!(script.trade_skill_repeat(), (0, 3));
        assert_eq!(
            script.eval::<i64>("return RECASTS").unwrap(),
            1,
            "the arm's signal only: the init disarms silently"
        );
    }

    /// Driven through the real handler registration.
    #[test]
    fn the_session_end_closes_the_book() {
        let mut app = App::new();
        app.add_plugins(UiTradeSkillPlugin);
        app.world_mut().resource_mut::<TradeSkillOpen>().line = Some(197);

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "logged out".into(),
                end: benilla_protocol::SessionEnd::LoggedOut,
            }],
        );

        assert_eq!(app.world().resource::<TradeSkillOpen>().line, None);
    }
}
