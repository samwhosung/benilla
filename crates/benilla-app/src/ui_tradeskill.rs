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

use std::collections::VecDeque;
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
use crate::ui_script::{SweepApp, SweepLeg};
use crate::ui_spellbook::SkillLines;
use crate::ui_unit::UnitFeed;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

/// Opener spell ids (effect 47) that `send_spell_cast` intercepted, opened by
/// [`open_trade_skill`] the same frame.
#[derive(Resource, Default)]
pub(crate) struct TradeSkillOpens(pub(crate) Vec<u32>);

/// The open book's skill line (`[0xbde040]`). An opener of it, the Lua close and the session end
/// clear it: a logout replaces the VM without running the old `OnHide`, so the close alone never
/// comes. A `/reload` keeps it.
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
    mut hosted_vm: crate::script_calls::HostedVm,
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
) {
    hosted_vm.run(|script| {
        if cascades.read().next().is_none() {
            return;
        }
        if let Some(script) = script {
            script.reset_crafts_on_world_enter();
        }
    })
}

/// The craft init's type write (`0x4f5ee0` zeroes `[0xbdcfb8]`) on each cascade, with no event:
/// the window is closed under the VM that survives a worldport and under a reload's new one.
pub(crate) fn close_craft_on_world_enter(
    mut cascades: MessageReader<crate::net::WorldEnterCascadeMessage>,
    mut craft_open: ResMut<crate::ui_craft::CraftOpen>,
) {
    if cascades.read().count() > 0 {
        craft_open.line = None;
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
        register_rebuild_triggers(app);
        app.add_sweep_leg(SweepLeg::TradeSkill, cancel_trade_skill_waits);
        app.init_resource::<TradeSkillOpens>()
            .init_resource::<TradeSkillOpen>()
            .init_resource::<TradeSkillItemWaits>()
            .init_resource::<TradeSkillRepeatCarry>()
            .add_systems(Startup, load_spell_focus.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // Opens before the feed, so an opener shows the window this frame; the
                    // drain after the input pass, so a Create click casts this frame.
                    open_trade_skill
                        .before(feed_trade_skill)
                        .in_set(crate::ui_script::UiFeed),
                    feed_trade_skill.in_set(UnitFeed),
                    drain_trade_skill.in_set(crate::ui_script::UiDrain),
                    // In the feed phase, ahead of every feed that pushes the lists.
                    (reset_crafts_on_world_enter, close_craft_on_world_enter)
                        .in_set(crate::ui_script::UiFeed)
                        .before(UnitFeed),
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

/// Toggle the book for each intercepted opener (`0x4fc910`, `0x4f6000`): its line is the opener's
/// `SkillLineAbility` row (3908 Tailoring is line 197), the client's `SkillLineRecIndex_Find`
/// (`0x6de040`). The open line's opener closes it (`0x4fc918`), the open craft type's its window
/// (`0x4f6006`); any other opens.
fn open_trade_skill(
    mut opens: ResMut<TradeSkillOpens>,
    mut open: ResMut<TradeSkillOpen>,
    mut craft_open: ResMut<crate::ui_craft::CraftOpen>,
    skill_lines: Option<Res<SkillLines>>,
    spells: Option<Res<Spells>>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
) {
    let (race, class) = self_store.single().map_or((0, 0), |s| s.race_class());
    for spell_id in opens.0.drain(..) {
        let Some(skill_lines) = skill_lines.as_deref() else {
            warn!("ui_tradeskill: opener {spell_id} before SkillLineAbility loaded — dropped");
            continue;
        };
        let Some(line) = skill_lines.catalog.spell_to_line(spell_id, race, class) else {
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
            if craft_open.line.is_some() && craft_open.craft_type == craft_type {
                debug!("ui_tradeskill: opener {spell_id} closes the CraftFrame");
                craft_open.line = None;
            } else {
                debug!(
                    "ui_tradeskill: opener {spell_id} opens the CraftFrame (line {line}, craft type {craft_type})"
                );
                craft_open.line = Some(line);
                craft_open.craft_type = craft_type;
            }
        } else if open.line == Some(line) {
            debug!("ui_tradeskill: opener {spell_id} closes skill line {line}");
            open.line = None;
        } else {
            debug!("ui_tradeskill: opener {spell_id} opens skill line {line}");
            open.line = Some(line);
        }
    }
}

/// The open line's `PLAYER_SKILL_INFO` slot, `None` for a line the player lacks.
fn skill_slot(
    store: &ObjectStore,
    skill_id: u32,
) -> Option<benilla_protocol::messages::PlayerSkillSlot> {
    (0..PLAYER_SKILL_SLOTS)
        .filter_map(|i| store.0.player_skill(i))
        .find(|s| u32::from(s.skill_id) == skill_id)
}

/// `GetTradeSkillLine`'s rank and maximum (`0x4fdd40`): each half of the slot plus the permanent
/// bonus, read unsigned, when the half is above 0 (`0x4fddf0`-`0x4fddff`, `0x4fde63`-`0x4fde71`).
fn skill_rank(store: &ObjectStore, skill_id: u32) -> (u32, u32) {
    let Some(s) = skill_slot(store, skill_id) else {
        return (0, 0);
    };
    let bonus = u32::from(s.perm_bonus as u16);
    let with_bonus = |half: u16| match u32::from(half) {
        0 => 0,
        v => v + bonus,
    };
    (with_bonus(s.value), with_bonus(s.max))
}

/// The slot's raw value, which the recipe colours band on (`0x4fcc31`-`0x4fcc41`).
fn skill_value(store: &ObjectStore, skill_id: u32) -> u32 {
    skill_slot(store, skill_id).map_or(0, |s| u32::from(s.value))
}

/// What a skill watch's rebuild reads again (`0x5de434`, `0x5de457`): the line's rank and
/// maximum, and each recipe's band on the slot's value (`0x4fcc31`-`0x4fcc41`).
pub(crate) fn reband(
    t: &mut TradeSkillState,
    store: Option<&ObjectStore>,
    skill_lines: Option<&SkillLines>,
) {
    let (Some(store), Some(skill_lines)) = (store, skill_lines) else {
        return;
    };
    let rank = skill_value(store, t.line);
    (t.rank, t.max_rank) = skill_rank(store, t.line);
    let (race, class) = store.race_class();
    for r in &mut t.recipes {
        if let Some(sla) = skill_lines.catalog.ability(r.spell_id, race, class) {
            r.difficulty = difficulty(rank, sla.trivial_low, sla.trivial_high);
        }
    }
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
    owner: u64,
) -> Option<String> {
    let item = d.effect_item_type[0];
    if item == 0 {
        return None;
    }
    let display = items.template(item, owner, ask)?.display_info_id;
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
    let (race, class) = store.race_class();
    let sla = skill_lines.catalog.ability(spell_id, race, class)?;

    // Reagents: `(entry, need)` pairs off `Spell.dbc`; have is the mask-0 count (`0x622130`, `0x4fccfc`).
    let mut reagents = Vec::new();
    let mut num_available = u32::MAX;
    for &(entry, need) in d.reagents.iter().filter(|&&(e, n)| e != 0 && n != 0) {
        let have = count_of(&store.0, objects, entry, InventoryScope::DEFAULT);
        // A reagent is asked by its getter's call alone (`0x4ff6e2`).
        let (name, icon) = match items.template(entry, 0, Ask::Read) {
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
    // The build's lookup of the product, with the callback `0x4fc8e0` and the recipe's
    // `0xb0000000` id as the owner (`0x4fcd21`-`0x4fcd4d`).
    let owner = benilla_ui::script::spell_owner(spell_id);
    let icon = recipe_icon(d, icons, items, ask, owner);
    // The icon getter's item while its record is in flight (`0x4fdb90`).
    let icon_pending = Some(d.effect_item_type[0])
        .filter(|&item| item != 0 && items.template(item, 0, Ask::Read).is_none());
    // The header key (`0x4fca20`): the product's `(class, subclass)` named from
    // `ItemSubClass.dbc`, `None` while its template is in flight, as in the client. The same
    // template gives the `InventoryType` the slot filter reads and the `ItemLevel` the sort
    // breaks ties on (`record+0x14`).
    let (group, product_inv_type, product_item_level) = (product_item != 0)
        .then(|| {
            items.template(product_item, owner, ask).map(|t| {
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
    let mut tools_pending = Vec::new();
    for &t in d.totems.iter().filter(|&&t| t != 0) {
        let have = count_of(&store.0, objects, t, InventoryScope::DEFAULT) > 0;
        // A totem is asked by its getter's call alone (`0x4ffade`).
        match items.template(t, 0, Ask::Read) {
            Some(info) => tools.push((info.name.clone(), have)),
            None => tools_pending.push(t),
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
        icon_pending,
        tools_pending,
    })
}

/// The leave-world sweep's leg (`0x490ae9` → `0x4fc780`): the pending count zeroed.
fn cancel_trade_skill_waits(mut vm: crate::script_calls::HostedVm) {
    vm.run(|script| {
        if let Some(script) = script {
            script.trade_skill_cancel_waits();
        }
    });
}

/// The packets that rebuild the open profession and craft lists, each at its handler's place on
/// a lane of their own ([`ListRebuilds`]): a learn, an unlearn and a supersede's two halves
/// (`0x5e9c20`, `0x5e9fe0`, `0x5e6392`, `0x5e63a4`), and `SMSG_PET_SPELLS` (`0x4bdafa`).
pub(crate) fn register_rebuild_triggers(app: &mut App) {
    use crate::lanes::LaneApp;
    use SessionEventKind as K;
    app.init_resource::<ListRebuilds>()
        .add_lane(
            "list rebuilds",
            |world| world.get_resource::<ListRebuilds>()?.peek(),
            fire_list_rebuilds,
            None,
        )
        .add_systems(
            Update,
            drop_list_rebuilds
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        )
        .net_handler(K::SpellLearned, on_book_change)
        // Ahead of the book's own handlers, which take the spell out of the lists the unlearn
        // tests it against.
        .net_handler_ahead(K::SpellRemoved, on_book_change)
        .net_handler_ahead(K::SpellSuperceded, on_book_change)
        .net_handler(K::PetSpells, on_pet_spells);
}

/// Which list a packet rebuilds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListRebuild {
    /// The profession list, with flag 1 (`0x4fca20(1)`).
    Trade,
    /// The craft list (`0x4f60c0`).
    Craft,
}

/// The rebuilds the handlers queue, each stamped as its handler queued it
/// ([`crate::lanes::stamp`]), fired at their packets' places.
#[derive(Resource, Default)]
pub(crate) struct ListRebuilds(VecDeque<(crate::lanes::Stamp, ListRebuild)>);

impl ListRebuilds {
    /// Queue `rebuild` behind everything stamped before it.
    fn push(&mut self, rebuild: ListRebuild) {
        let stamp = crate::lanes::stamp();
        let at = self.0.partition_point(|(s, _)| *s < stamp);
        self.0.insert(at, (stamp, rebuild));
    }

    fn peek(&self) -> Option<crate::lanes::Stamp> {
        self.0.front().map(|(s, _)| *s)
    }
}

/// Fired or dropped the frame they land, never into a later interface.
fn drop_list_rebuilds(mut rebuilds: ResMut<ListRebuilds>) {
    rebuilds.0.clear();
}

/// The lane's `fire_until` ([`crate::lanes`]): each rebuild `until` admits, over the lists the
/// feeds pushed this frame, its window's update signalled while the window is open
/// (`0x4fd164`, `0x4f661a`); the lane stops after one whose firing stamped anything.
fn fire_list_rebuilds(
    In(until): In<crate::lanes::Until>,
    mut vm: crate::script_calls::HostedVm,
    mut rebuilds: ResMut<ListRebuilds>,
) {
    while let Some(stamp) = rebuilds.peek().filter(|&s| until.admits(s)) {
        let Some((_, rebuild)) = rebuilds.0.pop_front() else {
            return;
        };
        let ((), moved) = crate::lanes::firing(stamp, || {
            vm.run(|script| {
                let Some(script) = script else {
                    return;
                };
                match rebuild {
                    ListRebuild::Trade => {
                        if script.rebuild_trade_skill_with(true, &mut |_| {}) {
                            script.fire_event("TRADE_SKILL_UPDATE", vec![]);
                        }
                    }
                    ListRebuild::Craft => {
                        if script.rebuild_craft(&mut |_| {}) {
                            script.fire_event("CRAFT_UPDATE", vec![]);
                        }
                    }
                }
            })
        });
        if moved {
            return;
        }
    }
}

/// The lists a book change rebuilds, by the spell's kind.
#[derive(bevy::ecs::system::SystemParam)]
struct ListTriggers<'w, 's> {
    /// The player, whose race and class index the ability row (`0x5ea054`).
    self_store: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    spells: Option<Res<'w, Spells>>,
    skill_lines: Option<Res<'w, SkillLines>>,
    actions: Option<Res<'w, PlayerActions>>,
    rebuilds: Option<ResMut<'w, ListRebuilds>>,
}

impl ListTriggers<'_, '_> {
    /// `LearnSpell 0x5e9c20` on a spell with a record (`0x5e9c88`): a profession spell
    /// (`Attributes & 0x20`, `0x5e9c8e`) rebuilds the profession list (`0x5e9d79`), any other
    /// with a `castUI` above 0 the craft list (`0x5e9d88`, `0x5e9e12`).
    fn learn(&mut self, spell_id: u32) {
        let Some(d) = self.spells.as_deref().and_then(|s| s.catalog.get(spell_id)) else {
            return;
        };
        if d.attributes & SPELL_ATTR_IS_TRADESKILL != 0 {
            self.push(ListRebuild::Trade);
        } else if (d.cast_ui as i32) > 0 {
            self.push(ListRebuild::Craft);
        }
    }

    /// `UnlearnSpell 0x5e9fe0` on a spell with a record (`0x5ea02a`): a profession spell with an
    /// ability row (`0x5ea054`-`0x5ea07b`) that its line's list holds (`0x5ea0fe`) rebuilds the
    /// profession list (`0x5ea165`); any other with a `castUI` above 0 that its type's list holds
    /// (`0x5ea1b8`, `0x5ea1c9`) the craft list (`0x5ea240`). A miss rebuilds nothing.
    fn unlearn(&mut self, spell_id: u32) {
        let Some(d) = self.spells.as_deref().and_then(|s| s.catalog.get(spell_id)) else {
            return;
        };
        let held = |order: fn(&PlayerActions) -> &Vec<u32>| {
            self.actions
                .as_deref()
                .is_some_and(|a| order(a).contains(&spell_id))
        };
        if d.attributes & SPELL_ATTR_IS_TRADESKILL != 0 {
            let (race, class) = self.self_store.single().map_or((0, 0), |s| s.race_class());
            let has_row = self
                .skill_lines
                .as_deref()
                .is_some_and(|l| l.catalog.ability(spell_id, race, class).is_some());
            if has_row && held(|a| &a.profession_order) {
                self.push(ListRebuild::Trade);
            }
        } else if (d.cast_ui as i32) > 0 && held(|a| &a.craft_order) {
            self.push(ListRebuild::Craft);
        }
    }

    fn push(&mut self, rebuild: ListRebuild) {
        if let Some(rebuilds) = self.rebuilds.as_deref_mut() {
            rebuilds.push(rebuild);
        }
    }
}

/// A learn, an unlearn, or a supersede, which unlearns the old rank and learns the new one.
fn on_book_change(In(ev): In<SessionEvent>, mut lists: ListTriggers) {
    match ev {
        SessionEvent::SpellLearned { spell_id } => lists.learn(spell_id),
        SessionEvent::SpellRemoved { spell_id } => lists.unlearn(spell_id),
        SessionEvent::SpellSuperceded {
            old_spell_id,
            new_spell_id,
        } => {
            lists.unlearn(old_spell_id);
            lists.learn(new_spell_id);
        }
        _ => {}
    }
}

/// `SMSG_PET_SPELLS` rebuilds the craft list on every arrival, the same book again included
/// (`0x4bd990` → `0x4bdafa`).
fn on_pet_spells(In(ev): In<SessionEvent>, mut lists: ListTriggers) {
    if matches!(ev, SessionEvent::PetSpells(_)) {
        lists.push(ListRebuild::Craft);
    }
}

/// Build the book: the known recipes of the open line, banded against the current rank.
fn feed_trade_skill(
    mut vm: crate::script_calls::VmWith<(
        Res<'static, crate::spell::Cooldowns>,
        Res<'static, TradeSkillOpen>,
        ResMut<'static, TradeSkillItemWaits>,
    )>,
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
    clock: Res<crate::ui_script::UiClock>,
    mut memo: Local<crate::ui_script::VmMemo<TradeMemo>>,
    mut engine: Local<TradeEngine>,
) {
    vm.feed(|script, (cooldowns, open, mut waits), fires| {
        let Some(script) = script else {
            return;
        };
        let memo = memo.get(script);
        // The list's build looks every record up with the callback `0x4fc8e0` (`0x4fcd4d`) at the
        // window's show; the rebuilds between read, as the getters' lookups stand for the stock
        // frame's calls (`0x4fdba8`), which the asks below make.
        let ask = Ask::at(memo.state.is_none() || open.is_changed(), &commands);
        let fresh = (|| -> Option<TradeSkillState> {
            let line = open.line?;
            let spells = spells.as_deref()?;
            let skill_lines = skill_lines.as_deref()?;
            let store = self_store.single().ok()?;
            let rank = skill_value(store, line);
            let (line_rank, max_rank) = skill_rank(store, line);
            let (race, class) = store.race_class();
            let line_name = skill_lines
                .catalog
                .line(line)
                .map(|l| l.name.clone())
                .unwrap_or_else(|| format!("Skill {line}"));
            let now = (clock.anchor, clock.ui_now);
            // The line's list in learn order (`0x5e9d72`), which the build walks (`0x4fcbb7`).
            let recipes: Vec<TradeSkillRecipe> = actions
                .profession_order
                .iter()
                .filter(|&&s| {
                    spells
                        .catalog
                        .get(s)
                        .is_some_and(|d| d.attributes & SPELL_ATTR_IS_TRADESKILL != 0)
                        && skill_lines.catalog.spell_to_line(s, race, class) == Some(line)
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
                rank: line_rank,
                max_rank,
                recipes,
            })
        })();

        // The line stays open while a frame cannot resolve its book (a worldport's purge of our
        // own player): only a close ends it (`0x4fc870`, `0x4fc920`).
        let fresh = match fresh {
            None if open.line.is_some() => memo.state.clone(),
            f => f,
        };
        // `CloseTradeSkill` closes the VM's window and signals `TRADE_SKILL_CLOSE` in the verb
        // (`0x4fc884`).
        let closed_in_place = !script.trade_skill_open();
        // The getters' item callbacks, kept inside their calls ([`ask_item_record`]); each record
        // that landed rebuilds (`0x4fdc30` → `0x4fdc3c`), and a refusal runs it with status 0,
        // which does nothing (`0x4fdc38`).
        let arrivals = landed(&mut waits.0, &items);
        // An open is the open line's change, which a `/reload` keeps (`[0xbde040]`): the new VM
        // takes the list with no event, as its world-enter init left it.
        let opened = fresh.as_ref().is_some_and(|f| engine.shown != Some(f.line));
        if fresh.is_some() || open.line.is_none() {
            engine.shown = fresh.as_ref().map(|f| f.line);
        }
        // Every push is silent: the rebuilds signal, and the getters read the state at their call.
        if fresh != memo.state {
            script.set_trade_skill(fresh.clone());
            // The templates the link verbs read, off the cache: they pass no callback (`0x4ff4b9`,
            // `0x4ff8da`).
            if let Some(f) = &fresh {
                script.read_item_templates(f.recipes.iter().flat_map(|r| {
                    std::iter::once(r.product_item).chain(r.reagents.iter().map(|re| re.item))
                }));
            }
        }
        match (&memo.state, &fresh) {
            (_, Some(f)) if opened => {
                debug!(
                    "ui_tradeskill: window opens — {} recipe(s), first group {:?}",
                    f.recipes.len(),
                    f.recipes.first().and_then(|r| r.group.clone())
                );
                // The open's build signals the update when nothing is pending (`0x4fc985` →
                // `0x4fd164`), then the show while the list holds a row (`0x4fc98a`-`0x4fc999`).
                if script.trade_skill_open_build() {
                    fires.post("TRADE_SKILL_UPDATE", vec![]);
                }
                if script.trade_skill_full_count() > 0 {
                    fires.post("TRADE_SKILL_SHOW", vec![]);
                }
            }
            (Some(_), None) if !closed_in_place => fires.post("TRADE_SKILL_CLOSE", vec![]),
            _ => {}
        }
        // Each landed record's rebuild signals the update; the packets' fire on their lane.
        if fresh.is_some() {
            for _ in 0..arrivals {
                if script.rebuild_trade_skill_with(false, &mut |_| {}) {
                    fires.post("TRADE_SKILL_UPDATE", vec![]);
                }
            }
        }
        // Each product record a build waited on runs its callback as it lands (`0x4fc8e0`),
        // which rebuilds with flag 1 once nothing is pending.
        engine
            .build_waits
            .extend(script.take_trade_skill_build_waits());
        let built = landed(&mut engine.build_waits, &items);
        for _ in 0..built {
            if script.trade_skill_record_landed()
                && script.rebuild_trade_skill_with(true, &mut |_| {})
            {
                fires.post("TRADE_SKILL_UPDATE", vec![]);
            }
        }
        memo.state = fresh;
    })
}

/// What [`feed_trade_skill`] remembers from its last frame, per VM.
#[derive(Default)]
struct TradeMemo {
    /// The state it last pushed.
    state: Option<TradeSkillState>,
}

/// What [`feed_trade_skill`] keeps across VMs, as the engine does across a `/reload`.
#[derive(Default)]
struct TradeEngine {
    /// The line whose open it signalled; a frame that finds no line open clears it, so the next
    /// session's first open signals again.
    shown: Option<u32>,
    /// The product records the builds' callbacks wait on, one per miss (`0x4fc8e0`).
    build_waits: Vec<u32>,
}

/// The item records the trade-skill getters' lookups missed, one per call, each the callback
/// `0x4fdc30` the item cache keeps, which outlives a VM and no cancel removes (`0x4fc890`); the
/// lookups ask inside their calls ([`ask_item_record`]).
#[derive(Resource, Default)]
pub(crate) struct TradeSkillItemWaits(pub(crate) Vec<u32>);

/// A trade-skill getter's lookup of a record in flight, inside its call
/// ([`benilla_ui::script::ItemCallback::TradeSkill`]): the item cache asks a miss, a refused
/// entry again, and keeps one callback per call (`0x4fdb90`, `0x4fdf77`, `0x4ff6cd`, `0x4ffac6`).
pub(crate) fn ask_item_record(
    items: &Items,
    commands: &NetCommands,
    waits: &mut TradeSkillItemWaits,
    entry: u32,
    owner: u64,
) {
    let _ = items.template(entry, owner, Ask::Event(commands));
    waits.0.push(entry);
}

/// Spend the waits whose record answered: the count of those that brought one.
pub(crate) fn landed(waits: &mut Vec<u32>, items: &Items) -> u32 {
    let mut landed = 0;
    waits.retain(|&entry| {
        if !items.template_answered(entry) {
            return true;
        }
        landed += u32::from(items.template(entry, 0, Ask::Read).is_some());
        false
    });
    landed
}

/// Feed the VM's repeat (`0x4fd7b0`) our own cast edges: `DoTradeSkill` arms it and casts inside
/// its call ([`TradeSkillSteps`]), each GO recasts what the hook answers, and a failure disarms it.
fn drain_trade_skill(
    mut vm: crate::script_calls::VmWith<(
        MessageReader<'static, 'static, CastEvent>,
        cast_target::CastTargeting<'static, 'static>,
        CastLadder<'static, 'static>,
    )>,
) {
    let edges = vm.turn(|script, (mut cast_events, _, ladder)| {
        if script.is_none() {
            return Vec::new();
        }
        let self_entity = ladder.self_player.single().ok().map(|(e, _)| e);
        cast_events
            .read()
            .filter(|ev| Some(ev.entity) == self_entity)
            .copied()
            .collect::<Vec<_>>()
    });

    // Each edge is its own packet's hook (`0x6e81d3` → `0x4fd7b0`), whose signal's handlers run
    // before the next packet counts down: a turn each.
    for ev in edges {
        vm.turn(|script, (_, targeting, mut ladder)| {
            let Some(script) = script else {
                return;
            };
            if let Some(spell_id) = repeat_edge(script, &ev) {
                debug!(
                    "ui_tradeskill: recast {spell_id} ({} left)",
                    script.trade_skill_repeat().1
                );
                // The recast, then its signal (`0x4fd7d7`, `0x4fd7dc`).
                ladder.send(spell_id, &targeting.context(), CastCommit::Spell);
                script.trade_skill_recast_sent();
            }
        });
    }
}

/// The profession window's verbs inside their calls ([`crate::script_calls::ScriptCallHost`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct TradeSkillSteps<'w> {
    /// Absent in a world with no profession window, which drops the close.
    open: Option<ResMut<'w, TradeSkillOpen>>,
}

impl TradeSkillSteps<'_> {
    /// `CloseTradeSkill()` (`0x4fd7f0` → `0x4fc870`): the open marker `[0xbde040]` zeroed, no
    /// packet.
    pub(crate) fn close(&mut self) {
        if let Some(open) = self.open.as_deref_mut() {
            debug!("ui_tradeskill: client-side close (no packet)");
            open.line = None;
        }
    }
}

/// `DoTradeSkill(index, count)`'s cast (`0x500314` → `TryCastSpell 0x6e5a90`), inside its call.
/// The verb reads the held list, not the open marker (`0x5002e1`, `0x5002e9`), so a recipe still
/// held after a close casts.
pub(crate) fn do_trade_skill(cast: &mut crate::spell::ScriptCast, spell_id: u32, count: u32) {
    debug!("ui_tradeskill: DoTradeSkill({spell_id}) ×{count}");
    let ctx = cast.targeting.context();
    cast.ladder.send(spell_id, &ctx, CastCommit::Spell);
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

    /// `GetTradeSkillLine` (`0x4fdd40`) adds the slot's permanent bonus to the value and to the
    /// maximum, each only when it is above 0 (`0x4fddf0`-`0x4fddff`, `0x4fde63`-`0x4fde71`): a
    /// gnome's Engineering at 225/300 with Engineering Specialization's +15 reads 240/315.
    #[test]
    fn the_line_rank_adds_the_permanent_bonus_to_each_nonzero_half() {
        let store = |value: u32, max: u32, perm: u32| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (718, 202),
                (719, (max << 16) | value),
                (720, (perm << 16) | 7),
            ]))
        };
        assert_eq!(skill_rank(&store(225, 300, 15), 202), (240, 315));
        assert_eq!(
            skill_rank(&store(0, 0, 15), 202),
            (0, 0),
            "no bonus on a zero"
        );
        assert_eq!(
            skill_rank(&store(225, 300, 15), 164),
            (0, 0),
            "another line"
        );
    }

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
            recipe_icon(&d, Some(&icons), &deps.items, Ask::New(&deps.commands), 0),
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
            recipe_icon(&d, Some(&icons), &deps.items, Ask::New(&deps.commands), 0),
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
            recipe_icon(
                &none,
                Some(&icons),
                &deps.items,
                Ask::New(&deps.commands),
                0
            ),
            None,
        );

        // A template that never lands: nil, asked for once.
        let missing = recipe(SPELL_EFFECT_CREATE_ITEM, 999);
        assert_eq!(
            recipe_icon(
                &missing,
                Some(&icons),
                &deps.items,
                Ask::New(&deps.commands),
                0
            ),
            None,
        );
        assert_eq!(
            recipe_icon(
                &missing,
                Some(&icons),
                &deps.items,
                Ask::New(&deps.commands),
                0
            ),
            None,
        );
        assert_eq!(deps.queried_entries(), vec![999], "ask-once, not ask-often");

        // Template landed, `ItemDisplayInfo` missing: still nil.
        let d = recipe(SPELL_EFFECT_CREATE_ITEM, 777);
        assert_eq!(
            recipe_icon(&d, None, &deps.items, Ask::New(&deps.commands), 0),
            None
        );
    }

    /// No `TRADE_SKILL_UPDATE` sender runs off a cooldown's clock (`0x4fd164` in the rebuild
    /// `0x4fca20`, and the filter setters `0x4fd725`, `0x4fd745`, `0x4fd765`); the getter reads the
    /// seconds left at its call (`0x4fda20`). So a running cooldown pushes the same recipe every
    /// frame.
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
            .init_resource::<TradeSkillItemWaits>()
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
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
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
        crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
            vm.expect("a VM").run("CloseTradeSkill()")
        })
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

    /// The open runs the build with flag 1 (`0x4fc985` → `0x4fca20`), whose tail signals
    /// `TRADE_SKILL_UPDATE` (`0x4fd164`), then `TRADE_SKILL_SHOW` while the list holds a row
    /// (`0x4fc98a`-`0x4fc999`); a change no rebuild runs on, as a cooldown starting, pushes
    /// silently: the getter reads it live (`0x4fda20`).
    #[test]
    fn the_open_signals_update_then_show_and_a_bare_change_signals_nothing() {
        const SPELL: u32 = 18_560;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.spells.insert(SPELL);
        actions.profession_order.push(SPELL);
        let mooncloth = || SpellDisplay {
            name: "Mooncloth".into(),
            attributes: SPELL_ATTR_IS_TRADESKILL,
            recovery_ms: 86_400_000,
            ..Default::default()
        };
        app.init_resource::<TradeSkillOpen>()
            .init_resource::<TradeSkillItemWaits>()
            .insert_resource(actions)
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::ui_script::UiClock>()
            .insert_resource(crate::net::NetCommands(tx))
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(SPELL, mooncloth())].into_iter().collect(),
                ),
                ..Spells::empty_for_tests()
            })
            .insert_resource(SkillLines {
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
            });
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        let script = UiScript::new().unwrap();
        script
            .run(
                "HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_SKILL_SHOW') \
                 f:RegisterEvent('TRADE_SKILL_UPDATE') \
                 f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.add_systems(Update, feed_trade_skill);
        let heard = |app: &mut App| {
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
                .unwrap()
        };
        app.world_mut().resource_mut::<TradeSkillOpen>().line = Some(197);
        assert_eq!(heard(&mut app), ["TRADE_SKILL_UPDATE", "TRADE_SKILL_SHOW"]);
        let d = mooncloth();
        app.world_mut()
            .resource_mut::<crate::spell::Cooldowns>()
            .start_spell(SPELL, &d, 0, Instant::now(), None);
        assert!(heard(&mut app).is_empty());
        assert!(
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<bool>("return GetTradeSkillCooldown(2) ~= nil")
                .unwrap(),
            "pushed, silently"
        );
    }

    /// The build refills the array from the line's list in learn order (`0x5e9d72` appends,
    /// `0x4fcbb7` walks it), and the sort leaves the rows it ties where the CRT's `shortsort`
    /// puts them (`0x4fd51f`, two hidden rows equal): with every group folded, the last learned
    /// comes first past the shown header, whatever the ids.
    #[test]
    fn the_folded_rows_follow_the_learn_order() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        let learned = [300, 100, 200];
        actions.spells.extend(learned);
        actions.profession_order.extend(learned);
        let recipe = |id: u32| {
            (
                id,
                SpellDisplay {
                    name: format!("Recipe {id}"),
                    attributes: SPELL_ATTR_IS_TRADESKILL,
                    ..Default::default()
                },
            )
        };
        let sla = benilla_formats::SlaInfo {
            skill_id: 197,
            req_skill_value: 1,
            forward_spell_id: 0,
            trivial_low: 1,
            trivial_high: 2,
        };
        app.init_resource::<TradeSkillOpen>()
            .init_resource::<TradeSkillItemWaits>()
            .insert_resource(actions)
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::ui_script::UiClock>()
            .insert_resource(crate::net::NetCommands(tx))
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    learned.into_iter().map(recipe).collect(),
                ),
                ..Spells::empty_for_tests()
            })
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_abilities(
                    learned.map(|id| (id, sla)),
                ),
            });
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_trade_skill);
        app.world_mut().resource_mut::<TradeSkillOpen>().line = Some(197);
        app.update();
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.run("CollapseTradeSkillSubClass(0)").unwrap();
        script.rebuild_trade_skill(&mut |_| {});
        let tail = script
            .eval::<Vec<String>>(
                "local t = {} for i = 2, 4 do t[i - 1] = (GetTradeSkillInfo(i)) end return t",
            )
            .unwrap();
        assert_eq!(tail, ["Recipe 200", "Recipe 100", "Recipe 300"]);
    }

    /// The open's build with a product's record in flight counts it pending and leaves before its
    /// update (`0x4fcd58`, `0x4fcf34`), the show still signalled for the rows it holds
    /// (`0x4fc98a`); the record's arrival runs the build's callback, which rebuilds with flag 1
    /// once the pending count is 0 (`0x4fc8e0`-`0x4fc900`), and that rebuild signals the update.
    #[test]
    fn a_product_in_flight_holds_the_update_until_its_record_lands() {
        const SILK: u32 = 4305;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.spells.insert(3839);
        actions.profession_order.push(3839);
        let mut effect_item_type = [0; 3];
        effect_item_type[0] = SILK;
        let bolt = SpellDisplay {
            name: "Bolt of Silk Cloth".into(),
            attributes: SPELL_ATTR_IS_TRADESKILL,
            effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
            effect_item_type,
            ..Default::default()
        };
        app.init_resource::<TradeSkillOpen>()
            .init_resource::<TradeSkillItemWaits>()
            .insert_resource(actions)
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::ui_script::UiClock>()
            .insert_resource(crate::net::NetCommands(tx))
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(3839, bolt)].into_iter().collect(),
                ),
                ..Spells::empty_for_tests()
            })
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_abilities([(
                    3839,
                    benilla_formats::SlaInfo {
                        skill_id: 197,
                        req_skill_value: 1,
                        forward_spell_id: 0,
                        trivial_low: 1,
                        trivial_high: 2,
                    },
                )]),
            });
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        let script = UiScript::new().unwrap();
        script
            .run(
                "HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_SKILL_SHOW') \
                 f:RegisterEvent('TRADE_SKILL_UPDATE') \
                 f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.add_systems(Update, feed_trade_skill);
        let heard = |app: &mut App| {
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
                .unwrap()
        };
        app.world_mut().resource_mut::<TradeSkillOpen>().line = Some(197);
        assert_eq!(heard(&mut app), ["TRADE_SKILL_SHOW"]);
        assert!(heard(&mut app).is_empty());
        app.world_mut().resource_mut::<Items>().insert_template(
            SILK,
            Some(crate::items::test_template("Bolt of Silk Cloth")),
        );
        assert_eq!(heard(&mut app), ["TRADE_SKILL_UPDATE"]);
    }

    /// The window's show asks what the list build looks up, each recipe's product with the
    /// callback `0x4fc8e0` and the recipe's `0xb0000000` id as the owner (`0x4fcd21`-`0x4fcd4d`),
    /// and nothing else: a reagent or a totem is asked by its getter's call alone (`0x4ff6e2`,
    /// `0x4ffade`), and the reagents the link getters read, which pass no callback (`0x4ff8da`),
    /// are read off the cache, never asked.
    #[test]
    fn the_show_asks_the_products_alone() {
        const LINEN: u32 = 2589;
        const BOLT: u32 = 2996;
        const NEEDLE: u32 = 5956;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.spells.extend([2963]);
        actions.profession_order.extend([2963]);
        let mut effects = [0; 3];
        effects[0] = SPELL_EFFECT_CREATE_ITEM;
        let mut effect_item_type = [0; 3];
        effect_item_type[0] = BOLT;
        let bolt = SpellDisplay {
            name: "Bolt of Linen Cloth".into(),
            attributes: SPELL_ATTR_IS_TRADESKILL,
            reagents: [
                (LINEN, 2),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
            ],
            totems: [NEEDLE, 0],
            effects,
            effect_item_type,
            ..Default::default()
        };
        let sla = benilla_formats::SlaInfo {
            skill_id: 197,
            req_skill_value: 1,
            forward_spell_id: 0,
            trivial_low: 1,
            trivial_high: 2,
        };
        app.init_resource::<TradeSkillOpen>()
            .init_resource::<TradeSkillItemWaits>()
            .init_resource::<crate::ui_craft::CraftOpen>()
            .insert_resource(actions)
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::ui_script::UiClock>()
            .insert_resource(crate::net::NetCommands(tx))
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(2963, bolt)].into_iter().collect(),
                ),
                ..Spells::empty_for_tests()
            })
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_abilities([(2963, sla)]),
            });
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        app.insert_non_send_resource(UiScript::new().unwrap());
        app.add_systems(Update, feed_trade_skill.in_set(crate::ui_script::UiFeed))
            .insert_resource(State::new(crate::client_state::ClientState::InWorld));
        app.world_mut().resource_mut::<TradeSkillOpen>().line = Some(197);
        app.update();
        app.update();
        let asked: Vec<_> = crate::query_cache::queries(&rx)
            .into_iter()
            .filter(|c| matches!(c, crate::net::ClientCommand::ItemQuery { .. }))
            .collect();
        assert!(
            matches!(
                asked[..],
                [crate::net::ClientCommand::ItemQuery {
                    entry: BOLT,
                    guid: 0xb000_0000_0000_0b93
                }]
            ),
            "{asked:?}"
        );
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        assert!(script.take_item_stat_asks().is_empty(), "no pre-ask");
    }

    /// A learn or an unlearn of a profession spell rebuilds the open list once per packet
    /// (`0x5e9d79`, `0x5ea165`), whether or not the list moved, a supersede once for each half
    /// (`0x5e6392`, `0x5e63a4`); another spell rebuilds nothing. A record a getter's call waited
    /// on rebuilds it once per call (`0x4fdc30` → `0x4fdc3c`).
    #[test]
    fn each_packet_and_each_waited_record_signals_one_update() {
        const LINEN: u32 = 2589;
        const REAGENTS_LINEN: [(u32, u32); 8] = [
            (LINEN, 2),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
            (0, 0),
        ];
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.spells.extend([2963, 3915]);
        actions.profession_order.extend([2963, 3915]);
        let bolt = || SpellDisplay {
            name: "Bolt of Linen Cloth".into(),
            attributes: SPELL_ATTR_IS_TRADESKILL,
            reagents: REAGENTS_LINEN,
            ..Default::default()
        };
        let sla = benilla_formats::SlaInfo {
            skill_id: 197,
            req_skill_value: 1,
            forward_spell_id: 0,
            trivial_low: 1,
            trivial_high: 2,
        };
        app.init_resource::<TradeSkillOpen>()
            .init_resource::<TradeSkillItemWaits>()
            .init_resource::<crate::ui_craft::CraftOpen>()
            .insert_resource(actions)
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::ui_script::UiClock>()
            .insert_resource(crate::net::NetCommands(tx))
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [
                        (2963, bolt()),
                        (2964, bolt()),
                        (3915, SpellDisplay::default()),
                    ]
                    .into_iter()
                    .collect(),
                ),
                ..Spells::empty_for_tests()
            })
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_abilities([
                    (2963, sla),
                    (2964, sla),
                ]),
            });
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        let script = UiScript::new().unwrap();
        script
            .run(
                "HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_SKILL_SHOW') \
                 f:RegisterEvent('TRADE_SKILL_UPDATE') \
                 f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        app.add_systems(Update, feed_trade_skill.in_set(crate::ui_script::UiFeed))
            .insert_resource(State::new(crate::client_state::ClientState::InWorld));
        register_rebuild_triggers(&mut app);
        let heard = |app: &mut App| {
            app.update();
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
                .unwrap()
        };
        app.world_mut().resource_mut::<TradeSkillOpen>().line = Some(197);
        assert_eq!(heard(&mut app), ["TRADE_SKILL_UPDATE", "TRADE_SKILL_SHOW"]);
        let dispatch =
            |app: &mut App, events| crate::net::handlers::dispatch(app.world_mut(), events);
        dispatch(
            &mut app,
            vec![
                SessionEvent::SpellLearned { spell_id: 2963 },
                SessionEvent::SpellLearned { spell_id: 3915 },
            ],
        );
        assert_eq!(heard(&mut app), ["TRADE_SKILL_UPDATE"]);
        dispatch(
            &mut app,
            vec![SessionEvent::SpellSuperceded {
                old_spell_id: 2963,
                new_spell_id: 2964,
            }],
        );
        assert_eq!(
            heard(&mut app),
            ["TRADE_SKILL_UPDATE", "TRADE_SKILL_UPDATE"]
        );
        // An unlearn the line's list does not hold finds nothing (`0x5ea0fe`) and skips the
        // rebuild (`0x5ea165`).
        dispatch(
            &mut app,
            vec![SessionEvent::SpellRemoved { spell_id: 2964 }],
        );
        assert!(heard(&mut app).is_empty());
        let entries = {
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.keep_calls();
            s.run("GetTradeSkillReagentInfo(2, 1) GetTradeSkillReagentInfo(2, 1)")
                .unwrap();
            s.take_trade_skill_item_waits()
        };
        // The host's lookups inside the calls ([`super::ask_item_record`]).
        app.world_mut()
            .resource_scope(|world, mut waits: Mut<TradeSkillItemWaits>| {
                for entry in entries {
                    super::ask_item_record(
                        world.resource::<Items>(),
                        world.resource::<NetCommands>(),
                        &mut waits,
                        entry,
                        0,
                    );
                }
            });
        assert!(heard(&mut app).is_empty());
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(LINEN, Some(crate::items::test_template("Linen Cloth")));
        assert_eq!(
            heard(&mut app),
            ["TRADE_SKILL_UPDATE", "TRADE_SKILL_UPDATE"]
        );
    }

    /// The skill watches rebuild the open list, each at its place: the value watch on every run,
    /// after its skill-list rebuild and its line past the gate (`0x5de2a7`, `0x5de417`,
    /// `0x5de434`); the maximum's and permanent bonus's ahead of its skill-list rebuild
    /// (`0x5de457`, `0x5de45c`); the temporary bonus's not at all (`0x5de470`).
    #[test]
    fn the_skill_watches_rebuild_the_open_list_in_their_order() {
        use crate::field_watch::FieldNotice;
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.init_resource::<crate::field_watch::NoticeLane>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .init_resource::<crate::names::NameCache>()
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_spell_lines([])
                    .with_line_names([(197, "Tailoring")])
                    .with_race_class_flags([(197, 0)]),
            });
        // A human warrior: race and class 1 (`UNIT_FIELD_BYTES_0`).
        app.world_mut().spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(36, 0x0101)])),
        ));
        let mut script = UiScript::new().unwrap();
        script
            .run(
                r#"ERR_SKILL_GAINED_S = "You have gained the %s skill."
                ERR_SKILL_UP_SI = "Your skill in %s has increased to %d."
                HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_SKILL_UPDATE')
                f:RegisterEvent('SKILL_LINES_CHANGED') f:RegisterEvent('CHAT_MSG_SKILL')
                f:SetScript('OnEvent', function() table.insert(HEARD, event) end)"#,
            )
            .unwrap();
        script.set_trade_skill(Some(TradeSkillState {
            line: 197,
            line_name: "Tailoring".into(),
            rank: 1,
            max_rank: 75,
            recipes: Vec::new(),
        }));
        app.insert_non_send_resource(script);
        let mut fired = |notice: FieldNotice| {
            app.world_mut()
                .resource_mut::<crate::field_watch::NoticeLane>()
                .write_batch([notice]);
            app.world_mut()
                .run_system_once(crate::ui_unit::field_events::fire_field_notices)
                .unwrap();
            let mut s = app.world_mut().non_send_resource_mut::<UiScript>();
            s.resolve();
            s.eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
                .unwrap()
        };
        let value = |old, shown| FieldNotice::SkillValue {
            skill_id: 197,
            old,
            shown,
        };
        assert_eq!(
            fired(value(1, 2)),
            [
                "SKILL_LINES_CHANGED",
                "CHAT_MSG_SKILL",
                "TRADE_SKILL_UPDATE"
            ]
        );
        assert_eq!(
            fired(value(7, 7)),
            ["TRADE_SKILL_UPDATE"],
            "the gate's skip"
        );
        assert_eq!(
            fired(FieldNotice::SkillMax),
            ["TRADE_SKILL_UPDATE", "SKILL_LINES_CHANGED"]
        );
        assert_eq!(fired(FieldNotice::SkillLines), ["SKILL_LINES_CHANGED"]);
    }

    /// Our GOs count the batch down and recast, a GO flagged `0x41` is not ours to count
    /// (`0x6e81de`), and the last craft recasts and signals nothing (`0x4fd7cf`).
    #[test]
    fn our_cast_edges_drive_the_vms_repeat() {
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
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
                icon_pending: None,
                tools_pending: Vec::new(),
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
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
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
                icon_pending: None,
                tools_pending: Vec::new(),
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
        crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
            vm.expect("a VM")
                .run("CloseTradeSkill() DoTradeSkill(2, 1)")
        })
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

    /// Each of our cast edges is its own `SMSG_SPELL_GO` in the reference, whose hook (`0x4fd7b0`)
    /// counts down (`0x4fd7ca`) and signals `UPDATE_TRADESKILL_RECAST` (`0x4fd7dc`) before the next
    /// packet is read, so two edges drained in one pass show their handlers 2, then 1.
    #[test]
    fn each_recasts_handlers_read_the_count_its_edge_left() {
        let (mut app, _rx) = headless_tailor();
        crate::script_calls::with_hosted_vm(app.world_mut(), |vm| vm.expect("a VM").run(
                "SEEN = '' local f = CreateFrame('Frame') \
                 f:RegisterEvent('UPDATE_TRADESKILL_RECAST') \
                 f:SetScript('OnEvent', function() SEEN = SEEN .. GetTradeskillRepeatCount() .. ' ' end) \
                 SelectTradeSkill(2) DoTradeSkill(2, 3)",
            ))
            .unwrap();
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        for seq in 0..2 {
            app.world_mut().write_message(CastEvent {
                entity: me,
                spell_id: 2963,
                kind: CastEventKind::Go,
                cast_flags: 0,
                item_cast: false,
                seq,
            });
        }
        drain(&mut app);
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(script.trade_skill_repeat(), (2963, 1));
        assert_eq!(script.eval::<String>("return SEEN").unwrap(), "3 2 1 ");
    }

    /// A cancelled targeting cursor is the cast abort with its disarm flag (`0x6e4900`,
    /// `0x6e4940(cl=1)` → `0x4fd790`): the armed batch disarms, with its signal, when the cancel's
    /// reports are signalled. Nothing up, nothing cancels (`0x6e4907`).
    #[test]
    fn a_cancelled_cursor_disarms_the_batch() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut app, _rx) = headless_tailor();
        crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
            vm.expect("a VM").run("DoTradeSkill(2, 3)")
        })
        .unwrap();
        drain(&mut app);
        let repeat = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .trade_skill_repeat()
        };
        assert_eq!(repeat(&app), (2963, 3));
        let stop = |app: &mut App| {
            let up = app
                .world_mut()
                .run_system_once(|mut ladder: CastLadder| ladder.stop_targeting())
                .unwrap();
            app.world_mut()
                .run_system_once(crate::ui_cast::signal_reports)
                .unwrap();
            up
        };
        assert!(!stop(&mut app), "nothing up");
        assert_eq!(repeat(&app), (2963, 3), "nothing cancels (`0x6e4907`)");
        app.world_mut()
            .resource_mut::<crate::spell::targeting::SpellTargeting>()
            .enter(133, crate::spell::CastCommit::Spell, 0x2);
        assert!(stop(&mut app));
        assert_eq!(repeat(&app), (0, 3));
    }

    /// The world-enter inits (`0x4fc750`, `0x4f5ee0`) on a worldport's cascade, whose VM
    /// survives: the lists empty, the spell disarms silently and the count stays.
    #[test]
    fn the_world_enter_cascade_empties_the_lists_and_disarms_silently() {
        let mut app = App::new();
        app.add_message::<crate::net::WorldEnterCascadeMessage>()
            .add_systems(Update, reset_crafts_on_world_enter);
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
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
                icon_pending: None,
                tools_pending: Vec::new(),
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

    /// The Bolt of Silk Cloth book behind the Tailoring opener 3908, its product's record in
    /// flight, fed and opened as the client runs them; each VM installed through [`listening`].
    fn silk_book() -> App {
        const SILK: u32 = 4305;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        let mut actions = PlayerActions::default();
        actions.spells.extend([3839, 3908]);
        actions.profession_order.push(3839);
        let mut effect_item_type = [0; 3];
        effect_item_type[0] = SILK;
        let bolt = SpellDisplay {
            name: "Bolt of Silk Cloth".into(),
            attributes: SPELL_ATTR_IS_TRADESKILL,
            effects: [SPELL_EFFECT_CREATE_ITEM, 0, 0],
            effect_item_type,
            ..Default::default()
        };
        let opener = SpellDisplay {
            name: "Tailoring".into(),
            effects: [benilla_formats::SPELL_EFFECT_TRADE_SKILL, 0, 0],
            ..Default::default()
        };
        let sla = benilla_formats::SlaInfo {
            skill_id: 197,
            req_skill_value: 1,
            forward_spell_id: 0,
            trivial_low: 1,
            trivial_high: 2,
        };
        app.init_resource::<TradeSkillOpen>()
            .init_resource::<TradeSkillItemWaits>()
            .init_resource::<TradeSkillOpens>()
            .init_resource::<crate::ui_craft::CraftOpen>()
            .insert_resource(actions)
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<Items>()
            .init_resource::<crate::spell::Cooldowns>()
            .init_resource::<crate::ui_script::UiClock>()
            .insert_resource(crate::net::NetCommands(tx))
            .insert_resource(Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(3839, bolt), (3908, opener)].into_iter().collect(),
                ),
                ..Spells::empty_for_tests()
            })
            .insert_resource(SkillLines {
                catalog: benilla_formats::SkillLineCatalog::from_abilities([
                    (3839, sla),
                    (3908, sla),
                ]),
            });
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        app.insert_non_send_resource(listening());
        app.add_systems(Update, (open_trade_skill, feed_trade_skill).chain());
        app
    }

    /// A VM that records the book's three events.
    fn listening() -> UiScript {
        let script = UiScript::new().unwrap();
        script
            .run(
                "HEARD = {} local f = CreateFrame('Frame') f:RegisterEvent('TRADE_SKILL_SHOW') \
                 f:RegisterEvent('TRADE_SKILL_UPDATE') f:RegisterEvent('TRADE_SKILL_CLOSE') \
                 f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
            )
            .unwrap();
        script
    }

    /// One frame, and what the VM heard in it.
    fn heard(app: &mut App) -> Vec<String> {
        app.update();
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
            .unwrap()
    }

    /// A `/reload` (`0x490bd0`): the leave sweep zeroes the pending count (`0x4fc8cb`), a new VM
    /// replaces the old, and the world-enter init zeroes the list (`0x4fc750` at `0x4909bf`).
    fn reload(app: &mut App) {
        let mut old = app.world_mut().non_send_resource_mut::<UiScript>();
        old.trade_skill_cancel_waits();
        let mut script = listening();
        script.reset_crafts_on_world_enter();
        app.insert_non_send_resource(script);
    }

    /// A `/reload` with the book open keeps its line (`[0xbde040]`) and signals nothing, so the
    /// new interface shows no window and reads an empty list under the line. The product record
    /// the old build waited on still runs its callback (`0x4fc8e0`), whose rebuild signals the
    /// update into the new interface (`0x4fc900`).
    #[test]
    fn a_reload_keeps_the_book_open_unshown_until_a_rebuild() {
        let mut app = silk_book();
        app.world_mut()
            .resource_mut::<TradeSkillOpens>()
            .0
            .push(3908);
        assert_eq!(heard(&mut app), ["TRADE_SKILL_SHOW"]);
        reload(&mut app);
        assert!(heard(&mut app).is_empty(), "no update, no show");
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(
            script
                .eval::<(i64, String)>("return GetNumTradeSkills(), (GetTradeSkillLine())")
                .unwrap(),
            (0, "Skill 197".into())
        );
        app.world_mut().resource_mut::<Items>().insert_template(
            4305,
            Some(crate::items::test_template("Bolt of Silk Cloth")),
        );
        assert_eq!(heard(&mut app), ["TRADE_SKILL_UPDATE"]);
        assert!(
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<i64>("return GetNumTradeSkills()")
                .unwrap()
                > 0
        );
    }

    /// An opener toggles the book (`0x4fc910`): the open line's opener closes it with
    /// `TRADE_SKILL_CLOSE` (`0x4fc918`-`0x4fc930`), and the next opens it again. After a reload
    /// the line is still open, so the first opener closes the unshown book and the second shows it.
    #[test]
    fn an_opener_of_the_open_line_closes_the_book() {
        let mut app = silk_book();
        let open = |app: &mut App| {
            app.world_mut()
                .resource_mut::<TradeSkillOpens>()
                .0
                .push(3908);
            heard(app)
        };
        assert_eq!(open(&mut app), ["TRADE_SKILL_SHOW"]);
        assert_eq!(open(&mut app), ["TRADE_SKILL_CLOSE"]);
        assert_eq!(app.world().resource::<TradeSkillOpen>().line, None);
        assert_eq!(open(&mut app), ["TRADE_SKILL_SHOW"]);
        reload(&mut app);
        assert!(heard(&mut app).is_empty());
        assert_eq!(open(&mut app), ["TRADE_SKILL_CLOSE"]);
        assert_eq!(open(&mut app), ["TRADE_SKILL_SHOW"]);
    }

    /// A frame that cannot resolve the book while its line stays open, as a worldport's purge of
    /// our own player leaves it, closes nothing: only a close ends the line (`0x4fc870`,
    /// `0x4fc920`), and the player's return signals no second open.
    #[test]
    fn a_frame_without_our_player_keeps_the_book_open() {
        let mut app = silk_book();
        app.world_mut()
            .resource_mut::<TradeSkillOpens>()
            .0
            .push(3908);
        assert_eq!(heard(&mut app), ["TRADE_SKILL_SHOW"]);
        let me = app
            .world_mut()
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(app.world())
            .unwrap();
        app.world_mut().despawn(me);
        assert!(heard(&mut app).is_empty(), "no close");
        app.world_mut()
            .spawn((ObjectStore(Default::default()), SelfPlayer));
        assert!(heard(&mut app).is_empty(), "no second open");
        assert!(app
            .world()
            .non_send_resource::<UiScript>()
            .trade_skill_open());
    }
}
