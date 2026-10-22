//! The item lane: each run of an item watcher's callback ([`Run`]), readied at its packet with the
//! unit signals it ends in, fired at the packet's place among the other lanes ([`crate::lanes`]),
//! whole and in its handler's order. A watcher's callback runs inside the notify pass of the update
//! that moved its field (`0x465570` → `call [ebx+0x10]`, `0x4655dc`), and so do its signals. The
//! record set-up (`0x5d8440`) runs the action bar's recount (`0x5d85ce`), the quest log's
//! (`0x5d85d9`) and the owner's signal (`0x5d8606`); the destroy (`0x5d8660`) the recount
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

    /// The notices queued, in stamp order: what the readers that push ahead of the lane read to
    /// know the bags moved.
    pub(crate) fn notices(&self) -> impl Iterator<Item = &ItemNotice> {
        self.0.iter().filter_map(|(_, r, _)| r.notice.as_ref())
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

/// The VM and the trade session a run's trade half writes.
type RunVm<'w, 's> =
    crate::script_calls::VmWith<'w, 's, Option<ResMut<'static, crate::ui_trade::TradeSession>>>;

/// The lane's `fire_until` ([`crate::lanes`]): each run `until` admits, whole, its Lua run with the
/// host lent; the lane stops after a run whose firing stamped anything, so what it pushed fires
/// next. With no VM the runs are dropped.
fn fire_item_runs(
    In(until): In<Until>,
    mut vm: RunVm,
    mut lane: ResMut<ItemLane>,
    mut reads: RunReads,
) {
    while let Some((stamp, run, touch)) = lane.pop(until) {
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
    vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        fire_unlocks(script, unlocks);
        let counter = crate::ui_action::recount::Counter {
            store: reads.me.iter().next(),
            objects: &reads.trade.objects,
            items: &reads.trade.items,
            commands: &reads.trade.commands,
        };
        for entry in notice.recounts() {
            crate::ui_action::recount::recount(script, &counter, entry);
        }
    });
    if let Some(touch) = touch {
        vm.turn(|script, trade| {
            if let (Some(script), Some(mut trade)) = (script, trade) {
                crate::ui_trade::touch_trade(script, &mut trade, touch, &reads.trade);
            }
        });
    }
    vm.run(|script| {
        if let Some(script) = script {
            fire_tail(script, &mut reads.sink, &notice, &bags, signals);
        }
    });
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
            fire_bags(script, bags);
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
}
