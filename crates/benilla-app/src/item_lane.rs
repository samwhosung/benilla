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
use crate::item_watch::{BagEvent, Freed, ItemNotice, Run, TradeTouch, Unlock};
use crate::lanes::{Stamp, Until};

/// The item watchers' runs, each stamped as its packet's handler readied it
/// ([`crate::lanes::stamp`]), with the trade half a close may run ahead of the lane
/// ([`Self::take_trade_touches`]).
#[derive(Resource, Default)]
pub(crate) struct ItemLane(VecDeque<(Stamp, Run, Option<TradeTouch>)>);

impl ItemLane {
    /// Queue `run` behind everything stamped before it; a run that does nothing is not queued.
    pub(crate) fn push(&mut self, run: Run) {
        if run.is_empty() {
            return;
        }
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

    /// Take every run waiting with its stamp, in order, unfired.
    #[cfg(test)]
    pub(crate) fn drain_stamped(&mut self) -> Vec<(Stamp, Run)> {
        self.0.drain(..).map(|(s, r, _)| (s, r)).collect()
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
        .add_drain_end(|world| {
            if let Some(items) = world.get_resource::<crate::items::Items>() {
                items.end_refusal_walks();
            }
        })
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

/// What a run's firing reads: what the trade scan reads, and the message sink. The recount counts
/// through the host's action states.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RunReads<'w, 's> {
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
/// unlocks, then signals its bag events and its unit signals. Its alert recompute (`0x4c7ee0`)
/// reads the objects as of its place through the lent host: behind a visible item's rebuilds,
/// ahead of its recounts (`0x5dee75`, `0x5dee7a`), else last, behind the stack count's quest log
/// ask (`0x5d9519`, `0x5d952a`), the enchantment's slot signal (`0x5d9b2a`, `0x5d9b2f`), and alone
/// for the flags' and the durability's. The objects it frees go last ([`forget_freed`]); a
/// deactivate's cursor leg goes first ([`let_cursor_go`]).
fn fire_run(vm: &mut RunVm, reads: &mut RunReads, run: Run, touch: Option<TradeTouch>) {
    let Run {
        notice,
        signals,
        unlocks,
        bags,
        alerts,
        freed,
        cursor,
    } = run;
    if let Some(item) = cursor {
        on_vm(vm, |script| let_cursor_go(script, item));
    }
    let Some(notice) = notice else {
        on_vm(vm, |script| fire_unlocks(script, unlocks));
        fire_bags(vm, &bags);
        on_vm(vm, |script| fire_signals(script, &mut reads.sink, signals));
        if alerts {
            on_vm(vm, UiScript::recompute_inventory_alerts);
        }
        on_vm(vm, |script| forget_freed(script, freed));
        return;
    };
    let rebuild = rebuild_at(&notice);
    let alerts_first = alerts
        && matches!(
            notice,
            ItemNotice::Visible { .. } | ItemNotice::ForeignVisible
        );
    let held = vm.run(|script| {
        let Some(script) = script else {
            return false;
        };
        fire_unlocks(script, unlocks);
        if rebuild == Some(Rebuild::BeforeRecount) {
            rebuild_lists(script);
        }
        if alerts_first {
            script.recompute_inventory_alerts();
        }
        true
    });
    if held {
        for entry in notice.recounts() {
            crate::ui_action::recount::recount(vm, entry);
        }
    }
    if let Some(touch) = touch {
        vm.turn(|script, (trade, _)| {
            if let (Some(script), Some(mut trade)) = (script, trade) {
                crate::ui_trade::touch_trade(script, &mut trade, touch, &reads.trade);
            }
        });
    }
    fire_tail(vm, &mut reads.sink, &notice, &bags, signals, rebuild);
    if alerts && !alerts_first {
        on_vm(vm, UiScript::recompute_inventory_alerts);
    }
    on_vm(vm, |script| forget_freed(script, freed));
}

/// [`Run::clearing_cursor`]: `ClearCursor(1, 1)` with the host lent, its `ITEM_LOCK_CHANGED` and
/// `CURSOR_UPDATE` signalled as it runs, when the cursor holds `item` as the run fires.
fn let_cursor_go(script: &mut UiScript, item: u64) {
    if script.cursor_item().is_some_and(|held| held.guid == item) {
        script.clear_cursor_payload();
    }
}

/// The locks of the item objects freed at the run's place, forgotten with no event: each was its
/// object's bit (`item+0x314`), and the next object under the guid is built unlocked (`0x5d821f`,
/// `0x5d823e`).
fn forget_freed(script: &mut UiScript, freed: Freed) {
    match freed {
        Freed::Nothing => {}
        Freed::Items(guids) => {
            for guid in guids {
                script.forget_item_lock(guid);
            }
        }
        Freed::All => script.forget_all_item_locks(),
    }
}

/// `f` on the VM with the host lent; nothing with no VM.
fn on_vm(vm: &mut RunVm, f: impl FnOnce(&mut UiScript)) {
    vm.run(|script| {
        if let Some(script) = script {
            f(script);
        }
    });
}

/// Where a watcher's run rebuilds the open profession and craft lists, `0x4fca20(0)` then
/// `0x4f60c0`, if it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rebuild {
    /// Ahead of the recount: the record set-up (`0x5d85be`), the destroy (`0x5d8788`), the stack
    /// count (`0x5d94d1`) and anyone's visible item (`0x5dee70`).
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
        | ItemNotice::Visible { .. }
        | ItemNotice::ForeignVisible => Some(Rebuild::BeforeRecount),
        ItemNotice::Seat { .. } => Some(Rebuild::BeforeBags),
        ItemNotice::Bag { .. } => Some(Rebuild::AfterBags),
        ItemNotice::EntryRecord { .. }
        | ItemNotice::SpellCharges { .. }
        | ItemNotice::Entry { .. }
        | ItemNotice::Enchantment { .. }
        | ItemNotice::TradeRecord { .. }
        | ItemNotice::TooltipRecord { .. }
        | ItemNotice::LockKeyRecord
        | ItemNotice::ActionRecord { .. } => None,
    }
}

/// The two rebuilds, each reading its list as it stands and signalling its window's update while
/// that window is open.
fn rebuild_lists(script: &mut UiScript) {
    script.rebuild_trade_skill(&mut |_| {});
    if script.rebuild_craft(&mut |_| {}) {
        script.fire_event("CRAFT_UPDATE", vec![]);
    }
}

/// What a run does after its recount and trade half, in its handler's order: the quest log's ask
/// for the item (`0x4df7b0`) ahead of the `BAG_UPDATE` and the slot's signal in the record set-up
/// (`0x5d85d9`, `0x5d85e8`, `0x5d8606`) and the destroy (`0x5d87b2` before `0x5d87c1`), and behind
/// both in the stack count's (`0x5d94ff`, `0x5d950e`, `0x5d9519`); the spell charges'
/// `BAG_UPDATE_COOLDOWN` (`0x4f93d0`); a trade getter's record callback's `TRADE_UPDATE`
/// (`0x4c034f`); the tooltips' record callbacks (`0x52e530`, `0x52f820`, `0x52ad10`); and every
/// other run's bag events, then its signals.
fn fire_tail(
    vm: &mut RunVm,
    sink: &mut crate::ui_action::MessageSink,
    notice: &ItemNotice,
    bags: &[BagEvent],
    signals: Vec<FieldNotice>,
    rebuild: Option<Rebuild>,
) {
    match *notice {
        ItemNotice::Record { entry, .. } | ItemNotice::Destroyed { entry, .. } => {
            on_vm(vm, |script| script.quest_item_changed(entry));
            fire_bags(vm, bags);
            on_vm(vm, |script| fire_signals(script, sink, signals));
        }
        ItemNotice::StackCount { entry, .. } => {
            fire_bags(vm, bags);
            on_vm(vm, |script| {
                fire_signals(script, sink, signals);
                script.quest_item_changed(entry);
            });
        }
        ItemNotice::SpellCharges { .. } => on_vm(vm, |script| {
            script.fire_event("BAG_UPDATE_COOLDOWN", vec![]);
            fire_signals(script, sink, signals);
        }),
        // Literal names: `reference_ui`'s producer test finds fire sites by literal.
        ItemNotice::TradeRecord { .. } => {
            on_vm(vm, |script| script.fire_event("TRADE_UPDATE", vec![]));
        }
        ItemNotice::TooltipRecord { entry } => {
            on_vm(vm, |script| script.answer_item_tooltips(entry));
        }
        ItemNotice::LockKeyRecord => {
            on_vm(vm, |script| {
                script.tooltip_object_refresh();
            });
        }
        _ => {
            if rebuild == Some(Rebuild::BeforeBags) {
                on_vm(vm, rebuild_lists);
            }
            fire_bags(vm, bags);
            if rebuild == Some(Rebuild::AfterBags) {
                on_vm(vm, rebuild_lists);
            }
            on_vm(vm, |script| fire_signals(script, sink, signals));
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

/// A run's bag events, each with its number (`"%d"`, `0x835154`); a `RaiseBagUpdate` refreshes
/// the bar's usable cache first, whatever it signals (`0x4f9374` into `0x4e5c00`).
fn fire_bags(vm: &mut RunVm, bags: &[BagEvent]) {
    use benilla_ui::script::ScriptValue;
    for &event in bags {
        if let BagEvent::Raise(_) = event {
            vm.engine_usable_cache_refresh();
        }
        on_vm(vm, |script| match event {
            BagEvent::Raise(Some(n)) | BagEvent::Update(n) => {
                script.fire_event("BAG_UPDATE", vec![ScriptValue::Int(n)]);
            }
            BagEvent::Raise(None) => {}
            BagEvent::Closed(n) => script.fire_event("BAG_CLOSED", vec![ScriptValue::Int(n)]),
            BagEvent::BankSlots => script.fire_event("PLAYERBANKSLOTS_CHANGED", vec![]),
            BagEvent::Open(n) => script.fire_event("BAG_OPEN", vec![ScriptValue::Int(n)]),
        });
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
        app.world_mut().spawn((
            crate::net::SelfPlayer,
            crate::net::ObjectStore(Default::default()),
        ));
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
