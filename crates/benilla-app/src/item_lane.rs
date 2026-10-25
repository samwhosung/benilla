//! The item lane: each run of an item watcher's callback ([`Run`]), readied at its packet with the
//! unit signals it ends in, fired at the packet's place among the other lanes ([`crate::lanes`]),
//! whole and in its handler's order. A watcher's callback runs inside the notify pass of the update
//! that moved its field (`0x465570` → `call [ebx+0x10]`, `0x4655dc`), and so do its signals. The
//! record set-up (`0x5d8440`) runs the profession and craft lists' rebuilds (`0x5d85be`), the
//! action bar's recount (`0x5d85ce`), the quest log's (`0x5d85d9`) and the owner's signal
//! (`0x5d8606`); the destroy (`0x5d8660`) the recount
//! (`0x5d8798`), the trade's column (`0x5d87a7`), the quest log (`0x5d87b2`) and the slot's
//! signal (`0x5d87c1`); the stack count (`0x5d9400`) the recount (`0x5d94e1`), the trade
//! (`0x5d94f0`), the slot's signal (`0x5d950e`) and the quest log (`0x5d9519`); the spell charges
//! (`0x5d9540`) the recount (`0x5d9582`) and `BAG_UPDATE_COOLDOWN` (`0x5d9587`). Each opens with
//! its unlocks (`0x5d8590`, `0x5d94af`, `0x4f8e15`, `0x4f912c`, `0x5ddd50`) and signals its bag
//! events where its handler raises them (`0x5d85e8`, `0x5d94ff`, `0x4f8ea6`, `0x4f918e`,
//! `0x4f8f66`, `0x5ddd69`).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::field_watch::FieldNotice;
use crate::item_watch::{BagEvent, ItemNotice, Run, TradeTouch, Unlock};
use crate::lanes::{Stamp, Until};
use crate::net::{ObjectStore, SelfPlayer};

/// The item watchers' runs, each stamped as its packet's handler readied it
/// ([`crate::lanes::stamp`]), with the trade half a close may run ahead of the lane
/// ([`Self::take_trade_touches`]), and the count of the `RaiseBagUpdate` calls readied
/// ([`Self::bag_raises`]).
#[derive(Resource, Default)]
pub(crate) struct ItemLane(VecDeque<(Stamp, Run, Option<TradeTouch>)>, u64);

impl ItemLane {
    /// Queue `run` behind everything stamped before it; a run that does nothing is not queued.
    pub(crate) fn push(&mut self, run: Run) {
        if run.is_empty() {
            return;
        }
        self.1 = self.1.wrapping_add(run.bag_raises() as u64);
        let stamp = crate::lanes::stamp();
        let trade = run.notice.as_ref().and_then(ItemNotice::trade);
        let at = self.0.partition_point(|(s, _, _)| *s < stamp);
        self.0.insert(at, (stamp, run, trade));
    }

    fn peek(&self) -> Option<Stamp> {
        self.0.front().map(|(s, _, _)| *s)
    }

    fn pop(&mut self, until: Until) -> Option<(Stamp, Run, Option<TradeTouch>)> {
        until
            .admits(self.peek()?)
            .then(|| self.0.pop_front())
            .flatten()
    }

    /// Everything queued, dropped.
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    /// How many `RaiseBagUpdate` calls (`0x4f9370`) the runs readied so far make, each the action
    /// bar's usable refresh (`0x4e5c00`, `0x4f9374`), which the action feed runs on its edge.
    pub(crate) fn bag_raises(&self) -> u64 {
        self.1
    }

    /// Take every run waiting with its stamp, in order, unfired.
    #[cfg(test)]
    pub(crate) fn drain_stamped(&mut self) -> Vec<(Stamp, Run)> {
        self.0.drain(..).map(|(s, r, _)| (s, r)).collect()
    }

    /// Take every notice waiting, in order, unfired.
    #[cfg(test)]
    pub(crate) fn drain_notices(&mut self) -> Vec<ItemNotice> {
        self.0.drain(..).filter_map(|(_, r, _)| r.notice).collect()
    }

    /// The trade halves of every run queued, in stamp order, taken so the lane does not run them
    /// again: for a trade close, which runs inside its packet ([`crate::ui_trade`]).
    pub(crate) fn take_trade_touches(&mut self) -> Vec<TradeTouch> {
        self.0.iter_mut().filter_map(|(_, _, t)| t.take()).collect()
    }
}

/// The lane and its drop: [`drop_item_runs`] under the exact complement of the drain's gate, so a
/// run is fired or dropped the frame it is readied, never left to age.
pub(crate) fn add_item_lane(app: &mut App) {
    use crate::lanes::LaneApp;
    app.init_resource::<ItemLane>()
        .add_lane(
            "item watchers",
            |world| world.get_resource::<ItemLane>()?.peek(),
            fire_item_runs,
            None,
        )
        .add_systems(
            Update,
            drop_item_runs
                .in_set(crate::ui_script::UiFeed)
                .before(crate::ui_unit::UnitFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

/// The runs readied while no in-game interface is up, taken unfired: there is no VM to hear them,
/// and the one that comes up later pushes the bar, the bags and the log whole.
fn drop_item_runs(mut lane: ResMut<ItemLane>) {
    lane.clear();
}

/// What a run's firing reads: the active player, and what the trade scan reads, the objects and
/// the item records the recount counts with among them.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RunReads<'w, 's> {
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    trade: crate::ui_trade::TouchReads<'w, 's>,
    sink: crate::ui_action::MessageSink<'w>,
}

/// The VM, the trade session a run's trade half writes, and the lane, whose trade halves a close
/// inside a run's Lua takes.
type RunVm<'w, 's> = crate::script_calls::VmWith<
    'w,
    's,
    (
        Option<ResMut<'static, crate::ui_trade::TradeSession>>,
        ResMut<'static, ItemLane>,
    ),
>;

/// The lane's `fire_until` ([`crate::lanes`]): each run `until` admits, whole, its Lua run with the
/// host lent; the lane stops after a run whose firing stamped anything, so what it pushed fires
/// next. With no VM the runs are dropped.
fn fire_item_runs(In(until): In<Until>, mut vm: RunVm, mut reads: RunReads) {
    while let Some((stamp, run, touch)) = vm.own().1.pop(until) {
        let ((), moved) = crate::lanes::firing(stamp, || fire_run(&mut vm, &mut reads, run, touch));
        if moved {
            return;
        }
    }
}

/// Every run waiting, fired alone, for a test that drives the item lane without the others.
#[cfg(test)]
pub(crate) fn fire_item_lane(world: &mut World) {
    while world
        .resource::<ItemLane>()
        .peek()
        .is_some_and(|s| Until::all().admits(s))
    {
        world
            .run_system_cached_with(fire_item_runs, Until::all())
            .expect("the item lane fires");
        crate::creature_anim::apply_unit_edits(world);
    }
}

/// Seat what the item lane's firing reads that a harness's world need not hold; the commands
/// are the harness's own.
#[cfg(test)]
pub(crate) fn seat_item_lane(world: &mut World) {
    world.init_resource::<ItemLane>();
    world.init_resource::<crate::net::GuidIndex>();
    world.init_resource::<crate::items::Items>();
    world.init_resource::<crate::names::NameCache>();
    world.init_resource::<crate::ui_chat::ChatLog>();
    world.init_resource::<crate::sound::MessageSounds>();
}

/// One run, in its handler's order: its unlocks, the recount of each entry it passes `0x4e5c70`,
/// its trade half unless a close ran it, then its own tail ([`fire_tail`]). A run with no notice
/// unlocks, then signals its bag events and its unit signals.
fn fire_run(vm: &mut RunVm, reads: &mut RunReads, run: Run, touch: Option<TradeTouch>) {
    let Run {
        notice,
        signals,
        unlocks,
        bags,
    } = run;
    let Some(notice) = notice else {
        vm.run(|script| {
            if let Some(script) = script {
                fire_unlocks(script, unlocks);
                fire_bags(script, &bags);
                fire_signals(script, &mut reads.sink, signals);
            }
        });
        return;
    };
    let rebuild = rebuild_at(&notice);
    let held = vm.run(|script| {
        let Some(script) = script else {
            return false;
        };
        fire_unlocks(script, unlocks);
        if rebuild == Some(Rebuild::BeforeRecount) {
            rebuild_lists(script, &|entry| {
                carried(&reads.me, &reads.trade.objects, entry)
            });
        }
        true
    });
    if held {
        let counter = crate::ui_action::recount::Counter {
            store: reads.me.iter().next(),
            objects: &reads.trade.objects,
            items: &reads.trade.items,
            commands: &reads.trade.commands,
        };
        for entry in notice.recounts() {
            crate::ui_action::recount::recount(vm, &counter, entry);
        }
    }
    if let Some(touch) = touch {
        vm.turn(|script, (trade, _)| {
            if let (Some(script), Some(mut trade)) = (script, trade) {
                crate::ui_trade::touch_trade(script, &mut trade, touch, &reads.trade);
            }
        });
    }
    vm.run(|script| {
        if let Some(script) = script {
            let count = |entry| carried(&reads.me, &reads.trade.objects, entry);
            fire_tail(
                script,
                &mut reads.sink,
                &notice,
                &bags,
                signals,
                (rebuild, &count),
            );
        }
    });
}

/// Where a watcher's run rebuilds the open profession and craft lists, `0x4fca20(0)` then
/// `0x4f60c0`, if it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rebuild {
    /// Ahead of the recount: the record set-up (`0x5d85be`), the destroy (`0x5d8788`), the stack
    /// count (`0x5d94d1`) and a visible item (`0x5dee70`).
    BeforeRecount,
    /// After the recounts, ahead of the bag update: a backpack or keyring seat (`0x4f8e65`) and a
    /// container's (`0x4f917c`).
    BeforeBags,
    /// After the bag events: a bag seat (`0x4f8f6d` behind `0x4f8f66`).
    AfterBags,
}

/// The watchers that rebuild the lists, and where; the spell charges, enchantment and entry
/// watchers rebuild nothing.
fn rebuild_at(notice: &ItemNotice) -> Option<Rebuild> {
    match notice {
        ItemNotice::Record { .. }
        | ItemNotice::Destroyed { .. }
        | ItemNotice::StackCount { .. }
        | ItemNotice::Visible { .. } => Some(Rebuild::BeforeRecount),
        ItemNotice::Seat { .. } => Some(Rebuild::BeforeBags),
        ItemNotice::Bag { .. } => Some(Rebuild::AfterBags),
        ItemNotice::EntryRecord { .. }
        | ItemNotice::SpellCharges { .. }
        | ItemNotice::Entry { .. }
        | ItemNotice::Enchantment { .. }
        | ItemNotice::TradeRecord { .. } => None,
    }
}

/// The two rebuilds, each signalling its window's update while that window is open; `count` is
/// the carried count of an entry the builds read (`0x622130`).
fn rebuild_lists(script: &mut UiScript, count: &dyn Fn(u32) -> u32) {
    script.rebuild_trade_skill(&mut |t| t.recount(count));
    if script.rebuild_craft(&mut |c| c.recount(count)) {
        script.fire_event("CRAFT_UPDATE", vec![]);
    }
}

/// The player's carried count of an entry, 0 with no active player.
fn carried(
    me: &Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    objects: &crate::net::Objects,
    entry: u32,
) -> u32 {
    me.iter().next().map_or(0, |s| {
        crate::ui_items::count_of(
            &s.0,
            objects,
            entry,
            crate::ui_items::InventoryScope::DEFAULT,
        )
    })
}

/// What a run does after its recount and trade half, in its handler's order: the quest log's ask
/// for the item (`0x4df7b0`) ahead of the `BAG_UPDATE` and the slot's signal in the record set-up
/// (`0x5d85d9`, `0x5d85e8`, `0x5d8606`) and the destroy (`0x5d87b2` before `0x5d87c1`), and behind
/// both in the stack count's (`0x5d94ff`, `0x5d950e`, `0x5d9519`); the spell charges'
/// `BAG_UPDATE_COOLDOWN` (`0x4f93d0`); a trade getter's record callback's `TRADE_UPDATE`
/// (`0x4c034f`); and every other run's bag events, then its signals.
fn fire_tail(
    script: &mut UiScript,
    sink: &mut crate::ui_action::MessageSink,
    notice: &ItemNotice,
    bags: &[BagEvent],
    signals: Vec<FieldNotice>,
    (rebuild, count): (Option<Rebuild>, &dyn Fn(u32) -> u32),
) {
    match *notice {
        ItemNotice::Record { entry, .. } | ItemNotice::Destroyed { entry, .. } => {
            script.quest_item_changed(entry);
            fire_bags(script, bags);
            fire_signals(script, sink, signals);
        }
        ItemNotice::StackCount { entry, .. } => {
            fire_bags(script, bags);
            fire_signals(script, sink, signals);
            script.quest_item_changed(entry);
        }
        ItemNotice::SpellCharges { .. } => {
            script.fire_event("BAG_UPDATE_COOLDOWN", vec![]);
            fire_signals(script, sink, signals);
        }
        // Literal names: `reference_ui`'s producer test finds fire sites by literal.
        ItemNotice::TradeRecord { .. } => script.fire_event("TRADE_UPDATE", vec![]),
        _ => {
            if rebuild == Some(Rebuild::BeforeBags) {
                rebuild_lists(script, count);
            }
            fire_bags(script, bags);
            if rebuild == Some(Rebuild::AfterBags) {
                rebuild_lists(script, count);
            }
            fire_signals(script, sink, signals);
        }
    }
}

/// A run's unlocks: `UnlockItem` (`0x495420`) on one guid, or an entry's walk and its one
/// `ITEM_LOCK_CHANGED` (`0x6221e0`, `0x5d859a`).
fn fire_unlocks(script: &mut UiScript, unlocks: Vec<Unlock>) {
    for unlock in unlocks {
        match unlock {
            Unlock::Item(guid) => script.unlock_item(guid),
            Unlock::Entry(guids) => script.unlock_entry_items(&guids),
        }
    }
}

/// A run's bag events, each with its number (`"%d"`, `0x835154`).
fn fire_bags(script: &mut UiScript, bags: &[BagEvent]) {
    use benilla_ui::script::ScriptValue;
    for &event in bags {
        match event {
            BagEvent::Raise(Some(n)) | BagEvent::Update(n) => {
                script.fire_event("BAG_UPDATE", vec![ScriptValue::Int(n)]);
            }
            // The usable refresh alone, the action feed's ([`ItemLane::bag_raises`]).
            BagEvent::Raise(None) => {}
            BagEvent::Closed(n) => script.fire_event("BAG_CLOSED", vec![ScriptValue::Int(n)]),
            BagEvent::BankSlots => script.fire_event("PLAYERBANKSLOTS_CHANGED", vec![]),
            BagEvent::Open(n) => script.fire_event("BAG_OPEN", vec![ScriptValue::Int(n)]),
        }
    }
}

/// A run's unit signals, as the field notices fire theirs.
fn fire_signals(
    script: &mut UiScript,
    sink: &mut crate::ui_action::MessageSink,
    signals: Vec<FieldNotice>,
) {
    for signal in signals {
        crate::ui_unit::field_events::fire(script, sink, signal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run readied while no in-game interface is up is dropped by the frame, unfired, so the
    /// interface that comes up later never hears it.
    #[test]
    fn a_run_without_the_in_game_interface_is_dropped_by_the_frame() {
        let mut app = App::new();
        add_item_lane(&mut app);
        app.world_mut().resource_mut::<ItemLane>().push(Run {
            notice: Some(ItemNotice::TradeRecord { entry: 1 }),
            ..Run::default()
        });
        app.update();
        assert!(app.world().resource::<ItemLane>().0.is_empty());
    }

    /// A VM with the profession window and the craft window open, each holding one row, and a
    /// spy on their updates and `BAG_UPDATE`.
    fn open_windows() -> App {
        use benilla_ui::script::{
            CraftDifficulty, CraftRecipe, CraftState, CraftTooltip, TradeSkillDifficulty,
            TradeSkillRecipe, TradeSkillState,
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(crate::net::NetCommands(tx));
        app.insert_non_send_resource(rx);
        app.world_mut()
            .spawn((SelfPlayer, ObjectStore(Default::default())));
        let mut script = UiScript::new().unwrap();
        script.set_trade_skill(Some(TradeSkillState {
            line: 197,
            line_name: "Tailoring".into(),
            rank: 1,
            max_rank: 75,
            recipes: vec![TradeSkillRecipe {
                group: Some((4, 1, "Cloth".into())),
                spell_id: 2963,
                name: "Bolt of Linen Cloth".into(),
                difficulty: TradeSkillDifficulty::Optimal,
                num_available: 0,
                icon: None,
                min_made: 1,
                max_made: 1,
                cooldown: None,
                product_item: 2996,
                product_inv_type: 0,
                product_item_level: 5,
                reagents: Vec::new(),
                tools: Vec::new(),
                icon_pending: None,
                tools_pending: Vec::new(),
            }],
        }));
        script.set_craft(Some(CraftState {
            name: "Enchanting".into(),
            display_line: Some("Enchanting".into()),
            rank: 1,
            max_rank: 75,
            craft_type: 3,
            recipes: vec![CraftRecipe {
                spell_id: 7418,
                name: "Enchant Bracer - Minor Health".into(),
                sub_name: String::new(),
                difficulty: CraftDifficulty::Optimal,
                num_available: 0,
                icon: None,
                description: None,
                needs_item_target: true,
                reagents: Vec::new(),
                tools: Vec::new(),
                tooltip: CraftTooltip::Spell(7418),
                spell_level: 1,
                group: 0,
                skill_line: None,
                training_point_cost: 0,
                required_level: 0,
                tools_pending: Vec::new(),
            }],
        }));
        // No engine: the closes' calls are kept, their VM halves run.
        script.keep_calls();
        script
            .run(
                "HEARD = {} local f = CreateFrame('Frame') \
                 for _, e in ipairs({'TRADE_SKILL_UPDATE', 'CRAFT_UPDATE', 'BAG_UPDATE'}) do \
                 f:RegisterEvent(e) end \
                 f:SetScript('OnEvent', function() table.insert(HEARD, event) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        seat_item_lane(app.world_mut());
        app
    }

    /// Fire `notice`'s run with a `RaiseBagUpdate` of the backpack, and read what it signalled.
    fn heard(app: &mut App, notice: ItemNotice) -> Vec<String> {
        app.world_mut().resource_mut::<ItemLane>().push(Run {
            notice: Some(notice),
            bags: vec![BagEvent::Raise(Some(0))],
            ..Run::default()
        });
        fire_item_lane(app.world_mut());
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("local h = HEARD HEARD = {} return h")
            .unwrap()
    }

    /// Each bag and item watcher's run rebuilds the open lists, whether or not anything changed:
    /// the profession window's (`0x4fca20(0)`, `TRADE_SKILL_UPDATE` at `0x4fd164`), then the craft
    /// window's (`0x4f60c0`, `CRAFT_UPDATE` at `0x4f661a`). A seat's after its recounts and ahead
    /// of its bag update (`0x4f917c` before `0x4f918e`); a bag's after its bag events (`0x4f8f6d`
    /// behind `0x4f8f66`); the record set-up's, the destroy's, the stack count's and a visible
    /// item's ahead of their recounts and bag updates (`0x5d85be`, `0x5d8788`, `0x5d94d1`,
    /// `0x5dee70`). The spell charges and the entry watcher rebuild nothing, nor does a closed
    /// window.
    #[test]
    fn each_bag_and_item_watcher_run_rebuilds_the_open_lists() {
        let mut app = open_windows();
        let rebuild_then_bag = ["TRADE_SKILL_UPDATE", "CRAFT_UPDATE", "BAG_UPDATE"];
        for notice in [
            ItemNotice::Seat {
                old: None,
                new: None,
            },
            ItemNotice::Record { guid: 1, entry: 2 },
            ItemNotice::Destroyed { guid: 1, entry: 2 },
            ItemNotice::StackCount { guid: 1, entry: 2 },
            ItemNotice::Visible { old: 0, new: 2 },
        ] {
            assert_eq!(
                heard(&mut app, notice.clone()),
                rebuild_then_bag,
                "{notice:?}"
            );
        }
        assert_eq!(
            heard(
                &mut app,
                ItemNotice::Bag {
                    entries: Vec::new()
                }
            ),
            ["BAG_UPDATE", "TRADE_SKILL_UPDATE", "CRAFT_UPDATE"]
        );
        assert_eq!(
            heard(&mut app, ItemNotice::SpellCharges { guid: 1, entry: 2 }),
            Vec::<String>::new()
        );
        assert_eq!(
            heard(
                &mut app,
                ItemNotice::Entry {
                    guid: 1,
                    old: 2,
                    new: 3,
                    new_cached: true
                }
            ),
            ["BAG_UPDATE"]
        );
        app.world()
            .non_send_resource::<UiScript>()
            .run("CloseTradeSkill() CloseCraft() HEARD = {}")
            .unwrap();
        assert_eq!(
            heard(&mut app, ItemNotice::StackCount { guid: 1, entry: 2 }),
            ["BAG_UPDATE"]
        );
    }
}
