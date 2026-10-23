//! The questgiver window's app side: the handlers fill [`QuestGiver`], [`feed_quest`] pushes it
//! into the VM and fires the panel's event, and the verbs apply in their calls ([`QuestVerbs`]),
//! the teardowns among them.

use std::collections::HashMap;

use benilla_protocol::messages::{
    QuestDetails, QuestGiverList, QuestOfferReward, QuestRequestItems, QuestRewardItem,
    QuestShareMsg,
};
use bevy::prelude::*;

use benilla_ui::script::{QuestItemView, QuestPanel, QuestRewardSpell, QuestState};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_action::{show_messages, ui_error_text, MessageSink, Shown, Spells, UiError};
use crate::ui_script::{SweepApp, SweepLeg, UiFeed};
use crate::ui_session::{CoreCloses, LatchApp, LatchedWindow, NpcWindow};

/// The wire packet that opened the current panel.
pub(crate) enum QuestView {
    Greeting(QuestGiverList),
    Detail(QuestDetails),
    Progress(QuestRequestItems),
    Reward(QuestOfferReward),
}

/// The open questgiver window; the per-guid dialog statuses survive a close.
#[derive(Resource, Default)]
pub(crate) struct QuestGiver {
    pub(crate) completed_fanfare: bool,
    /// The panel's source: an NPC, a GameObject, a quest-starter item or a sharing player.
    pub(crate) npc: Option<u64>,
    pub(crate) view: Option<QuestView>,
    /// The reference's latch `0xbe0824`, written by DETAILS and OFFER_REWARD (`0x500ef0`) and by
    /// REQUEST_ITEMS' `closeOnCancel` (`0x501070`), and zeroed only on world enter (`0x500af0`).
    /// Non-zero turns a plain unit's decline into the silent teardown ([`decline_leg`]). vmangos
    /// sends 1 on every DETAILS and OFFER_REWARD; REQUEST_ITEMS carries 1 on a single-quest
    /// auto-open and 0 on `COMPLETE_QUEST` (`QuestHandler.cpp:390-395`).
    pub(crate) close_on_cancel: u32,
    /// The reference's already-acted latch `0xbe0844`, set by `AcceptQuest` and by a re-opening
    /// `DeclineQuest`, which keeps the panel up; both refuse while it is set (`0x5013a8`,
    /// `0x5013fe`) until a panel publish clears it ([`Self::open`]).
    pub(crate) acted: bool,
    /// The reference's `0xbe0828`: the chosen reward's item id, latched by `GetQuestReward`
    /// (`0x50161a`) for the turn-in's item line (`0x5dc676`-`0x5dc6c6`), and zeroed by a panel
    /// publish (`0x500d8c`), world enter (`0x500b15`) and the turn-in.
    pub(crate) chosen_reward: u32,
    /// The greeting's two pools, which only the selects read.
    pub(crate) pools: QuestPools,
    /// The reference's `0xbe0840`: the last escort confirm's quest, written by its packet
    /// (`0x5019c7`), never cleared, and sent by `ConfirmAcceptQuest` (`0x5021e0`).
    pub(crate) confirm: u32,
    /// Per-guid dialog status from `SMSG_QUESTGIVER_STATUS`, the `!`/`?` marker's value.
    statuses: HashMap<u64, u32>,
    /// Messages queued for [`feed_quest`], each a GlobalStrings key and its fills.
    messages: Vec<UiError>,
    reask: u32,
    /// The dialog statuses dropped at the reference's typemask-8 lookup (GameObject answers, which
    /// vmangos sends), per guid, so a live probe can tell a drop from a query never sent.
    refused: HashMap<u64, u32>,
    refused_count: u32,
    /// Panel publishes so far, and the last one [`feed_quest`] asked for: a publish fires the
    /// panel's event, on which the stock frame calls `GetQuestItemInfo`, which looks the item up
    /// with a callback (`0x5017de`).
    panels: u64,
    panels_asked: u64,
    /// The item records `GetQuestItemInfo`'s calls missed, one per call: the item cache's
    /// callbacks `0x5018c0`, which outlive the panel and fire `QUEST_ITEM_UPDATE` when their
    /// record lands (`0x5018fe`-`0x501903`), whatever the window shows then.
    item_waits: Vec<u32>,
}

/// The greeting's pools, the available quests (`0xbdfe60`, its count `0xbe0834`) with each row's
/// one-click flag, and the active ones (`0xbde690`, `0xbe0838`): 32 rows each, cleared whole and
/// refilled by a greeting (`0x500c41`, `0x500e40`, `0x500e90`) and zeroed at world enter
/// (`0x500b21`), so they outlive the greeting panel and the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestPools {
    pub(crate) available: Vec<(u32, bool)>,
    pub(crate) active: Vec<u32>,
}

impl QuestPools {
    /// The rows each pool holds.
    pub(crate) const ROWS: usize = 32;

    /// A greeting's rows: active for icon 3 or 4, else available, one-click for icon 0
    /// (`0x5dbbfe`-`0x5dbc13`).
    fn of(list: &QuestGiverList) -> Self {
        let mut pools = Self::default();
        for q in &list.quests {
            if row_is_active(q.icon) {
                pools.active.push(q.quest_id);
            } else {
                pools.available.push((q.quest_id, row_is_one_click(q.icon)));
            }
        }
        pools
    }
}

impl QuestGiver {
    /// Open (or replace) the window with a fresh wire view for `npc`.
    pub(crate) fn open(&mut self, npc: u64, view: QuestView) {
        if let QuestView::Greeting(list) = &view {
            self.pools = QuestPools::of(list);
        }
        self.npc = Some(npc);
        self.view = Some(view);
        self.panels = self.panels.wrapping_add(1);
        // `0x500bd0` clears the already-acted latch (`0x500d91`) and the chosen reward
        // (`0x500d8c`) on every panel publish.
        self.acted = false;
        self.chosen_reward = 0;
    }

    /// Whether the cell `[0xbe0810]` holds a source.
    #[allow(dead_code)]
    pub(crate) fn is_open(&self) -> bool {
        self.npc.is_some()
    }

    /// The one-shot turn-in fanfare, which the reference plays from the packet, not from Lua.
    pub(crate) fn take_completed_fanfare(&mut self) -> bool {
        std::mem::take(&mut self.completed_fanfare)
    }

    /// Zero the cell; the panel and its kind `[0xbe0818]` stay, as `0x501130` leaves them, for
    /// the panel verbs, and the feed shows nothing with the cell zero.
    pub(crate) fn clear(&mut self) {
        self.npc = None;
    }

    pub(crate) fn set_status(&mut self, npc: u64, status: u32) {
        self.statuses.insert(npc, status);
    }

    /// Every stored dialog status, which [`crate::quest_markers`] renders.
    pub(crate) fn statuses(&self) -> &HashMap<u64, u32> {
        &self.statuses
    }

    /// Drop the status of every guid `live` rejects: the reference keeps it on the unit
    /// (`unit+0xcb8`), so it dies with the object, and a map needs the prune.
    pub(crate) fn retain_statuses(&mut self, live: impl Fn(u64) -> bool) {
        self.statuses.retain(|&npc, _| live(npc));
        // The refusals follow the same lifetime.
        self.refused.retain(|&npc, _| live(npc));
    }

    /// Drop one guid's status when it stops being a questgiver, as the reference's teardown does.
    pub(crate) fn clear_status(&mut self, npc: u64) {
        self.statuses.remove(&npc);
    }

    /// Re-ask every visible questgiver's status, as the reference does after
    /// `SMSG_SET_FACTION_STANDING`, `SMSG_GROUP_LIST`, a turn-in and `SMSG_QUESTUPDATE_*`. A
    /// counter, folded into the marker query's generation, so no bump is lost to ordering.
    pub(crate) fn bump_reask(&mut self) {
        self.reask = self.reask.wrapping_add(1);
    }

    pub(crate) fn reask_epoch(&self) -> u32 {
        self.reask
    }

    pub(crate) fn refuse_status(&mut self, npc: u64, status: u32) {
        self.refused.insert(npc, status);
        self.refused_count = self.refused_count.saturating_add(1);
    }

    pub(crate) fn refused_for(&self, npc: u64) -> Option<u32> {
        self.refused.get(&npc).copied()
    }

    pub(crate) fn refused_count(&self) -> u32 {
        self.refused_count
    }

    /// One guid's stored dialog status; the markers read the whole map instead.
    #[allow(dead_code)]
    pub(crate) fn status(&self, npc: u64) -> Option<u32> {
        self.statuses.get(&npc).copied()
    }

    /// Queue a message for [`feed_quest`]; its key's catalog row picks the surface.
    pub(crate) fn push_message(&mut self, msg: UiError) {
        self.messages.push(msg);
    }

    pub(crate) fn take_messages(&mut self) -> Vec<UiError> {
        std::mem::take(&mut self.messages)
    }

    pub(crate) fn clear_session(&mut self) {
        self.clear();
        self.statuses.clear();
        self.messages.clear();
        self.close_on_cancel = 0;
        self.acted = false;
        self.chosen_reward = 0;
        self.view = None;
        self.pools = QuestPools::default();
        self.item_waits.clear();
    }

    /// Spend the waits whose item record answered: how many brought a record, each its own
    /// `QUEST_ITEM_UPDATE`; a refusal runs its callback with status 0, which fires nothing
    /// (`0x5018d6`-`0x5018e0`).
    fn item_answers(&mut self, items: &Items) -> usize {
        let mut landed = 0;
        self.item_waits.retain(|&entry| {
            if !items.template_answered(entry) {
                return true;
            }
            if items.template(entry, 0, Ask::Read).is_some() {
                landed += 1;
            }
            false
        });
        landed
    }
}

/// `SMSG_QUESTGIVER_QUEST_INVALID`'s `QuestFailedReason` as a GlobalStrings key, per the
/// reference's handler `0x5dbca0` (the `0x18f` arm of the quest demux `0x5e5910`), whose default,
/// `ERR_QUEST_NEED_PREREQS`, takes reason 0 and everything unlisted.
pub(crate) fn questgiver_invalid_key(reason: u32) -> &'static str {
    match reason {
        1 => "ERR_QUEST_FAILED_LOW_LEVEL",         // msgId 142
        6 => "ERR_QUEST_FAILED_WRONG_RACE",        // msgId 144
        12 => "ERR_QUEST_ONLY_ONE_TIMED",          // msgId 146
        13 => "ERR_QUEST_ALREADY_ON",              // msgId 148
        20 => "ERR_QUEST_FAILED_MISSING_ITEMS",    // msgId 143
        22 => "ERR_QUEST_FAILED_NOT_ENOUGH_MONEY", // msgId 145
        _ => "ERR_QUEST_NEED_PREREQS",             // msgId 147, the default
    }
}

/// `SMSG_QUESTGIVER_QUEST_FAILED`'s reason as a GlobalStrings key, per the reference's handler
/// `0x5dc840` (the `0x192` arm); each line's `%s` is the quest's title (`questRecord+0x9c`).
pub(crate) fn questgiver_failed_key(reason: u32) -> &'static str {
    match reason {
        4 | 50 => "ERR_QUEST_FAILED_BAG_FULL_S", // msgId 140
        17 => "ERR_QUEST_FAILED_MAX_COUNT_S",    // msgId 141
        _ => "ERR_QUEST_FAILED_S",               // msgId 139
    }
}

mod lines;
mod net;
#[cfg(test)]
pub(crate) mod verb_tests;
mod verbs;

#[cfg(test)]
pub(crate) use lines::test_template;
pub(crate) use lines::{QuestLine, QuestLines};
pub(crate) use verbs::QuestVerbs;

pub(crate) struct UiQuestPlugin;

impl Plugin for UiQuestPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        lines::register(app);
        app.init_resource::<QuestGiver>()
            .add_latched_window::<QuestGiver>()
            .add_sweep_leg(SweepLeg::Quest, close_quest_on_leave_world)
            .add_systems(
                Update,
                feed_quest
                    .in_set(UiFeed)
                    .in_set(crate::ui_session::NpcWindowFeed),
            );
    }
}

/// The leave-world sweep's quest leg (`0x490ac6` → `0x500bc0` → `0x501130(0, 1)`): the core with
/// its report set, so a shared quest is declined to its sharer.
fn close_quest_on_leave_world(world: &mut World) {
    close_core(world, true);
}

/// The giver's cell `[0xbe0810]`, which an item's or a GameObject's guid fills as well as an
/// NPC's. `0x493310` calls the core with its report set (`0x4933d3`), and so does `CloseQuest`
/// (`0x501a10`, inside its call), so a walk-away from a shared quest still declines it while an
/// NPC's panel closes with no giver re-open.
impl LatchedWindow for QuestGiver {
    const WINDOW: NpcWindow = NpcWindow::Quest;

    fn cell(&self) -> Option<u64> {
        self.npc
    }

    fn dispatched(world: &mut World) {
        close_core(world, true);
    }
}

/// The panel's one teardown `0x501130(0, report)`, as `0x493310` and the sweep enter it: nothing
/// with no giver (`0x501149`); with `report`, a sharing player is sent `MSG_QUEST_PUSH_RESULT`
/// DECLINE (`0x501159`-`0x5011e0`), as the reference's player lookup there resolves only a
/// player; then the cell is zeroed (`0x501201`) before the latch release (`0x50120d`), so the core
/// never re-enters, and `QUEST_FINISHED` fires once (`0x501217`). The panel kind `[0xbe0818]`
/// stays. The verbs run it inside their calls ([`crate::script_calls::ScriptCallHost`]).
pub(crate) fn close_core(world: &mut World, report: bool) {
    let Some(giver) = world.resource::<QuestGiver>().npc else {
        return;
    };
    if report && benilla_protocol::guid::is_player(giver) {
        debug!("ui_quest: closing {giver:#x}'s shared quest — declining it");
        if let Some(net) = world.get_resource::<NetCommands>() {
            let _ = net.0.send(ClientCommand::QuestPushResult {
                sharer: giver,
                msg: QuestShareMsg::DECLINE_QUEST,
            });
        }
    }
    world.resource_mut::<QuestGiver>().clear();
    crate::ui_session::note_closed(world, NpcWindow::Quest);
    crate::ui_session::with_script(world, |script| script.set_quest(None));
    crate::ui_session::release(world, giver);
    crate::ui_session::fire(world, |script| {
        script.fire_event("QUEST_FINISHED", vec![]);
    });
}

/// A panel's open (`0x500bd0`, every panel packet's): the latch first, with no test of the cell
/// (`0x500d74`), so a panel replacing the giver's own closes it, `QUEST_FINISHED` ahead of the new
/// panel's event; then the cell (`0x500d86`).
pub(crate) fn open_panel(world: &mut World, npc: u64, view: QuestView) {
    crate::ui_session::register(world, npc);
    world.resource_mut::<QuestGiver>().open(npc, view);
}

/// A greeting row is active when its wire icon is 3 or 4 and available otherwise, never by the
/// quest log (`0x5dbbfe`-`0x5dbc08`; gossip's rows too, `0x4e2430`, `0x4e2580`). vmangos's 3 and 4
/// are `DIALOG_STATUS_INCOMPLETE` and `_REWARD_REP` (`QuestDef.h:118-130`), and it marks an
/// auto-complete quest, never in the log, `_REWARD_REP` (`Player.cpp:12577`).
pub(crate) fn row_is_active(icon: u32) -> bool {
    matches!(icon, 3 | 4)
}

/// An available row with icon 0 is one-click: its select sends `COMPLETE_QUEST`, not
/// `QUERY_QUEST` (`0x5dbc13` stores the flag, `0x5012db` reads it). vmangos never sends icon 0 on
/// this packet.
pub(crate) fn row_is_one_click(icon: u32) -> bool {
    icon == 0
}

/// One wire item row for Lua; the name is `None` and the quality the seeded 0 (`0x501728`) until
/// the template lands. Read as `ask` says: `GetQuestItemInfo` looks it up with a callback
/// (`0x5017de`).
fn resolve_item(
    it: &QuestRewardItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    ask: Ask<'_>,
) -> QuestItemView {
    let template = items.template(it.item_id, 0, ask);
    let name = template.map(|t| t.name.clone());
    // The template's quality (`+0x1c`) for an item with an inventory type (`+0x2c`), else -1
    // (`0x5017fa`).
    let quality = template.map_or(0, |t| {
        if t.inventory_type != 0 {
            t.quality as i32
        } else {
            -1
        }
    });
    let texture = icons
        .and_then(|i| i.catalog.get(it.display_id))
        .and_then(|d| d.icon.clone());
    // `GetQuestItemLink`'s payload; `None` until the template gives the name and quality.
    let link = template.map(|t| crate::ui_items::item_link(it.item_id, &t.name, t.quality));
    QuestItemView {
        name,
        texture,
        count: it.count,
        quality,
        item_id: it.item_id,
        link,
    }
}

fn resolve_items(
    src: &[QuestRewardItem],
    items: &Items,
    icons: Option<&ItemDisplays>,
    ask: Ask<'_>,
) -> Vec<QuestItemView> {
    src.iter()
        .map(|it| resolve_item(it, items, icons, ask))
        .collect()
}

/// The quest's reward spell from `rewSpell`, named through `Spell.dbc`; 0 is none, and an id the
/// catalog lacks still counts, with nothing to paint.
pub(crate) fn reward_spell_view(
    spell_id: u32,
    spells: Option<&Spells>,
) -> Option<QuestRewardSpell> {
    if spell_id == 0 {
        return None;
    }
    let d = spells.and_then(|s| s.catalog.get(spell_id));
    Some(QuestRewardSpell {
        spell_id,
        name: d.map(|d| d.name.clone()),
        texture: d.and_then(|d| d.icon.clone()),
        // `isTradeskillSpell`: `Spell.dbc` col 6 `Attributes` bit `0x20` (`0x501e59`).
        tradeskill: d.is_some_and(|d| d.attributes & 0x20 != 0),
    })
}

fn snapshot(
    giver: &QuestGiver,
    items: &Items,
    icons: Option<&ItemDisplays>,
    ask: Ask<'_>,
    macros: &crate::npc_text::MacroContext,
    spells: Option<&Spells>,
) -> Option<QuestState> {
    // The wire sends quest text unexpanded; the client substitutes `$N`, `$B` and `$G`.
    let sub = |t: &str| crate::npc_text::substitute(t, macros);
    // The cell zero is a closed window, whatever panel its kind still names.
    giver.npc?;
    Some(match giver.view.as_ref()? {
        QuestView::Greeting(l) => {
            let mut active_titles = Vec::new();
            let mut available_titles = Vec::new();
            let mut active_levels = Vec::new();
            let mut available_levels = Vec::new();
            for q in &l.quests {
                if row_is_active(q.icon) {
                    active_titles.push(q.title.clone());
                    active_levels.push(q.level);
                } else {
                    available_titles.push(q.title.clone());
                    available_levels.push(q.level);
                }
            }
            QuestState {
                panel: QuestPanel::Greeting,
                greeting: sub(&l.greeting),
                active_titles,
                available_titles,
                active_levels,
                available_levels,
                ..Default::default()
            }
        }
        QuestView::Detail(d) => QuestState {
            panel: QuestPanel::Detail,
            title: sub(&d.title),
            body: sub(&d.details),
            objectives: sub(&d.objectives),
            choices: resolve_items(&d.choices, items, icons, ask),
            rewards: resolve_items(&d.rewards, items, icons, ask),
            reward_money: d.money.max(0) as u32,
            reward_spell: reward_spell_view(d.reward_spell, spells),
            ..Default::default()
        },
        QuestView::Progress(p) => QuestState {
            panel: QuestPanel::Progress,
            title: sub(&p.title),
            body: sub(&p.request_text),
            required: resolve_items(&p.required_items, items, icons, ask),
            required_money: p.required_money,
            completable: p.is_complete,
            ..Default::default()
        },
        QuestView::Reward(o) => QuestState {
            panel: QuestPanel::Reward,
            title: sub(&o.title),
            body: sub(&o.offer_text),
            choices: resolve_items(&o.choices, items, icons, ask),
            rewards: resolve_items(&o.rewards, items, icons, ask),
            reward_money: o.money.max(0) as u32,
            reward_spell: reward_spell_view(o.reward_spell, spells),
            ..Default::default()
        },
    })
}

fn panel_event(panel: QuestPanel) -> &'static str {
    match panel {
        QuestPanel::Greeting => "QUEST_GREETING",
        QuestPanel::Detail => "QUEST_DETAIL",
        QuestPanel::Progress => "QUEST_PROGRESS",
        QuestPanel::Reward => "QUEST_COMPLETE",
    }
}

/// Push the quest view into the VM and fire its events: a panel's event on each publish,
/// `QUEST_ITEM_UPDATE` on each item record a `GetQuestItemInfo` call missed. The close is the
/// core's ([`close_core`]).
pub(crate) fn feed_quest(
    mut vm: crate::script_calls::VmWith<ResMut<'static, QuestGiver>>,
    objects: crate::net::Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    names: Res<NameCache>,
    states: Res<crate::world_state::WorldStates>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    (spells, unit_names): (Option<Res<Spells>>, crate::ui_unit::NameTables),
    go_templates: Res<crate::go_templates::GameObjectTemplates>,
    materials: Option<Res<crate::ui_item_text::PageMaterials>>,
    mut sink: MessageSink,
    mut last: crate::ui_script::WindowMemo<Option<QuestState>>,
    mut closes: CoreCloses,
) {
    // The window is the host's too, so the feed reads and pushes it as an engine turn; the panel
    // events fire after it, with the host lent.
    vm.feed(|script, mut giver, fires| {
        let Some(script) = script else {
            return;
        };
        let last = last.get(script);
        if closes.since_last(script, NpcWindow::Quest) {
            *last = None;
        }
        if giver.take_completed_fanfare() {
            script.queue_sound_kit("QUESTCOMPLETED");
        }
        // Before the snapshot's early-out, so a refusal that closes the panel still prints.
        let lines: Vec<Shown> = giver
            .take_messages()
            .into_iter()
            .filter_map(|msg| {
                let get = |key: &str| script.lua().globals().get::<String>(key).ok();
                ui_error_text(&msg, &get).map(|text| Shown::keyed(msg.key, text))
            })
            .collect();
        show_messages(script, &mut sink, "ui_quest", lines);
        let player = crate::npc_text::player_identity(&self_q, &names, unit_names.get());
        // A panel's publish, or a window this VM has not shown, is the stock frame's getter calls.
        let published = giver.panels != giver.panels_asked;
        let ask = Ask::at(last.is_none() || published, &commands);
        giver.panels_asked = giver.panels;
        let waits = script.take_quest_item_waits();
        giver.item_waits.extend(waits);
        let item_updates = giver.item_answers(&items);
        let fresh = snapshot(
            &giver,
            &items,
            icons.as_deref(),
            ask,
            &crate::npc_text::MacroContext {
                subject: player.as_ref(),
                states: &states,
            },
            spells.as_deref(),
        );
        // The background is the source object's material, never a creature's and never on the wire
        // (`GetQuestBackgroundMaterial`, `0x502230`); the reader window's binding shares its body.
        let fresh = fresh.map(|mut st| {
            st.background_material = giver.npc.and_then(|source| {
                crate::ui_item_text::object_material(
                    source,
                    &objects,
                    &items,
                    &go_templates,
                    materials.as_deref(),
                )
            });
            st
        });
        // The frame reads `UnitName("npc")` (`QuestFrame.lua:63`), which looks the name up with a
        // callback (`0x5171b8`) at the panel's event; its landing fires nothing.
        if let Some(g) = giver.npc {
            let _ = names.resolve(&objects, g, ask);
        }
        // Each panel setter fires its event at its end, every publish (`0x500ee5`, `0x50105d`,
        // `0x501120`); a window this VM has not shown takes it too. No event carries an argument.
        // A close is the core's, which told the VM; a disconnect's clear tells it nothing.
        let shows = fresh
            .as_ref()
            .filter(|_| published || last.is_none())
            .map(|f| f.panel);
        if fresh != *last {
            script.set_quest(fresh.clone());
            *last = fresh;
        }
        if let Some(panel) = shows {
            fires.post(panel_event(panel), vec![]);
        }
        for _ in 0..item_updates {
            fires.post("QUEST_ITEM_UPDATE", vec![]);
        }
    })
}

/// The leg of `DeclineQuest()`'s core `0x5013f0` a source takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeclineLeg {
    /// The already-acted latch is set (`0x5013fe`) or the source does not resolve (`0x50142c`).
    Nothing,
    /// The teardown `0x501130(0,1)` (`0x5014aa`): declines to a sharer, closes the window.
    Teardown,
    /// A gossip-flagged unit (`0x50143b`-`0x501474`): `CMSG_GOSSIP_HELLO`; the window stays.
    GossipHello,
    /// A plain unit (`0x5014e4`-`0x501526`): `CMSG_QUESTGIVER_HELLO`; the window stays.
    QuestgiverHello,
}

/// `DeclineQuest`'s fork (`0x5013f0`, called by its binding `0x501d30`) on the source's type mask
/// and NPC flags, `None` when it is not in view; the re-open legs leave the window up. In order:
///
/// 1. the already-acted latch set (`0x5013fe`), or the source unresolved (`0x50142c`): nothing;
/// 2. a unit with the gossip NPC flag, and the active player (`0x50143b`-`0x50146b`):
///    `CMSG_GOSSIP_HELLO`, whatever `0xbe0824` says;
/// 3. an item, a player, or a non-zero `0xbe0824` (`0x501496`-`0x5014a2`): the teardown
///    `0x501130(0,1)`;
/// 4. a GameObject (`0x5014bf`): its interact virtual `[vtbl+0x60]` (`0x5014d0`), whose override
///    is untraced, so it takes the teardown here;
/// 5. anything else: `CMSG_QUESTGIVER_HELLO` (`0x501526`).
fn decline_leg(
    acted: bool,
    close_on_cancel: u32,
    source: Option<(u32, u32)>,
    player: bool,
) -> DeclineLeg {
    // `TYPEMASK_ITEM`, `_UNIT`, `_PLAYER` and `_GAMEOBJECT`.
    const ITEM: u32 = 0x2;
    const UNIT: u32 = 0x8;
    const PLAYER: u32 = 0x10;
    const GAMEOBJECT: u32 = 0x20;
    if acted {
        return DeclineLeg::Nothing;
    }
    let Some((mask, flags)) = source else {
        return DeclineLeg::Nothing;
    };
    if mask & UNIT != 0 && flags & crate::target::cursor_mode::npc_flags::GOSSIP != 0 && player {
        return DeclineLeg::GossipHello;
    }
    if mask & (ITEM | PLAYER) != 0 || close_on_cancel != 0 || mask & GAMEOBJECT != 0 {
        return DeclineLeg::Teardown;
    }
    DeclineLeg::QuestgiverHello
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::dialog_status;
    use benilla_ui::script::UiScript;

    fn triple(id: u32) -> QuestRewardItem {
        QuestRewardItem {
            item_id: id,
            count: 1,
            display_id: 100 + id,
        }
    }

    #[test]
    fn greeting_split_reads_the_wire_icon() {
        // 3 = DIALOG_STATUS_INCOMPLETE, 4 = DIALOG_STATUS_REWARD_REP.
        assert!(row_is_active(3));
        assert!(row_is_active(4));
        // 5 = DIALOG_STATUS_AVAILABLE, 2 = DIALOG_STATUS_CHAT.
        assert!(!row_is_active(5));
        assert!(!row_is_active(2));
        // Every other value is available, including ones vmangos never sends.
        for icon in [0, 1, 6, 7, 100, u32::MAX] {
            assert!(!row_is_active(icon), "icon {icon} must fall to AVAILABLE");
        }
    }

    /// vmangos marks an auto-complete quest's row `DIALOG_STATUS_REWARD_REP` though the quest is
    /// never in the log (`Player.cpp:12577`).
    #[test]
    fn an_autocomplete_row_is_active_though_it_is_not_in_the_log() {
        const REWARD_REP: u32 = 4;
        assert!(
            row_is_active(REWARD_REP),
            "an auto-complete turn-in must bin ACTIVE without ever entering the quest log"
        );
        assert!(!row_is_one_click(REWARD_REP), "it is active, not one-click");
    }

    #[test]
    fn the_one_click_flag_is_icon_zero_alone() {
        assert!(row_is_one_click(0));
        for icon in [1, 2, 3, 4, 5, u32::MAX] {
            assert!(!row_is_one_click(icon));
        }
    }

    /// `GetQuestItemInfo` looks each reward up with a callback (`0x5017de`) when the stock frame
    /// calls it, on the panel's event; the frames between ask only what the cache holds nothing
    /// for. A refused reward is asked by no frame, and again by the next panel the giver publishes.
    #[test]
    fn a_refused_reward_is_asked_at_the_next_panel_never_by_a_frame() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .init_resource::<QuestGiver>()
            .init_resource::<Items>()
            .init_resource::<NameCache>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::world_state::WorldStates>()
            .init_resource::<crate::go_templates::GameObjectTemplates>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_systems(Update, feed_quest);
        app.insert_non_send_resource(UiScript::new().unwrap());
        // A player source, so the giver's name is no creature read: its own query is the name's.
        const SOURCE: u64 = 0x42;
        let publish = |app: &mut App| {
            app.world_mut().resource_mut::<QuestGiver>().open(
                SOURCE,
                QuestView::Detail(QuestDetails {
                    npc: SOURCE,
                    quest_id: 100,
                    title: "A Threat Within".into(),
                    details: String::new(),
                    objectives: String::new(),
                    auto_finish: 1,
                    choices: vec![],
                    rewards: vec![triple(3000)],
                    money: 0,
                    reward_spell: 0,
                }),
            );
        };
        let reward_asks = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            crate::query_cache::queries(rx)
                .iter()
                .filter(|c| matches!(c, ClientCommand::ItemQuery { entry: 3000, .. }))
                .count()
        };
        publish(&mut app);
        app.update();
        assert_eq!(reward_asks(&rx), 1);
        app.world_mut()
            .resource_mut::<Items>()
            .insert_template(3000, None);
        for _ in 0..600 {
            app.update();
        }
        assert_eq!(reward_asks(&rx), 0, "no frame asks the refusal");
        publish(&mut app);
        app.update();
        assert_eq!(reward_asks(&rx), 1, "the next panel does, once");
        for _ in 0..600 {
            app.update();
        }
        assert_eq!(reward_asks(&rx), 0);
    }

    #[test]
    fn detail_snapshot_carries_text_and_rows() {
        let mut giver = QuestGiver::default();
        giver.open(
            0x42,
            QuestView::Detail(QuestDetails {
                npc: 0x42,
                quest_id: 100,
                title: "A Threat Within".into(),
                details: "Kill kobolds, $N.".into(),
                objectives: "Slay 10.".into(),
                auto_finish: 1,
                choices: vec![triple(2000)],
                rewards: vec![triple(3000)],
                money: 1234,
                reward_spell: 0,
            }),
        );
        assert!(giver.is_open());
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let player = crate::npc_text::Subject {
            name: "Tri".into(),
            race: "Human".into(),
            class: "Warrior".into(),
            gender: 0,
            player: true,
        };
        let snap = snapshot(
            &giver,
            &items,
            None,
            Ask::New(&commands),
            &crate::npc_text::MacroContext {
                subject: Some(&player),
                states: &crate::world_state::WorldStates::default(),
            },
            None,
        )
        .expect("open");
        assert_eq!(snap.panel, QuestPanel::Detail);
        assert_eq!(snap.title, "A Threat Within");
        assert_eq!(snap.body, "Kill kobolds, Tri.");
        assert_eq!(snap.objectives, "Slay 10.");
        assert_eq!(snap.choices.len(), 1);
        assert_eq!(snap.rewards.len(), 1);
        assert_eq!(snap.reward_money, 1234);
        // No template yet: no name, and the seeded quality 0 (`0x501728`).
        assert!(snap.rewards[0].name.is_none());
        assert_eq!(snap.rewards[0].quality, 0);
    }

    /// `GetQuestItemInfo`'s quality (`0x501700`): -1 for a template whose inventory type is 0
    /// (`0x5017fa`), its own quality otherwise, and the seeded 0 while the template is in flight
    /// (`0x501728`).
    #[test]
    fn a_giver_rows_quality_reads_minus_one_for_no_slot_and_zero_in_flight() {
        let mut items = Items::default();
        let mut boots = crate::items::test_template("Boots");
        (boots.inventory_type, boots.quality) = (8, 2);
        items.insert_template(3000, Some(crate::items::test_template("Ale")));
        items.insert_template(3001, Some(boots));
        let mut giver = QuestGiver::default();
        giver.open(
            0x42,
            QuestView::Detail(QuestDetails {
                npc: 0x42,
                quest_id: 100,
                title: "A Threat Within".into(),
                details: String::new(),
                objectives: String::new(),
                auto_finish: 1,
                choices: Vec::new(),
                rewards: vec![triple(3000), triple(3001), triple(3002)],
                money: 0,
                reward_spell: 0,
            }),
        );
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let snap = snapshot(
            &giver,
            &items,
            None,
            Ask::New(&commands),
            &crate::npc_text::MacroContext {
                subject: None,
                states: &crate::world_state::WorldStates::default(),
            },
            None,
        )
        .expect("open");
        let qualities: Vec<i64> = snap.rewards.iter().map(|r| r.quality as i64).collect();
        assert_eq!(qualities, [-1, 2, 0]);
    }

    #[test]
    fn progress_snapshot_carries_completability() {
        let mut giver = QuestGiver::default();
        giver.open(
            0x42,
            QuestView::Progress(QuestRequestItems {
                npc: 0x42,
                quest_id: 100,
                title: "A Threat Within".into(),
                request_text: "Bring me the tusks.".into(),
                emote: 0,
                close_on_cancel: 1,
                required_money: 500,
                required_items: vec![triple(2001)],
                is_complete: true,
            }),
        );
        let items = Items::default();
        let (tx, _rx) = crossbeam_channel::unbounded();
        let commands = NetCommands(tx);
        let snap = snapshot(
            &giver,
            &items,
            None,
            Ask::New(&commands),
            &crate::npc_text::MacroContext {
                subject: None,
                states: &crate::world_state::WorldStates::default(),
            },
            None,
        )
        .unwrap();
        assert_eq!(snap.panel, QuestPanel::Progress);
        assert_eq!(snap.required.len(), 1);
        assert_eq!(snap.required_money, 500);
        assert!(snap.completable);
    }

    /// The greeting snapshot keeps each quest's level beside its title, split into the two pools
    /// by the icon (`0x5dbb10` stores the wire level at row `+0x04`), for `GetActiveLevel` and
    /// `GetAvailableLevel`.
    #[test]
    fn the_greeting_snapshot_keeps_each_quests_level_beside_its_title() {
        let row = |quest_id, icon, level, title: &str| benilla_protocol::messages::QuestListEntry {
            quest_id,
            icon,
            level,
            title: title.into(),
        };
        let mut giver = QuestGiver::default();
        giver.open(
            0x42,
            QuestView::Greeting(QuestGiverList {
                npc: 0x42,
                greeting: "Hi".into(),
                emote_delay: 0,
                emote: 0,
                quests: vec![
                    row(1, 4, 12, "Active A"),
                    row(2, 2, 3, "Available A"),
                    row(3, 3, 9, "Active B"),
                    row(4, 2, 5, "Available B"),
                ],
            }),
        );
        let (tx, _rx) = crossbeam_channel::unbounded();
        let snap = snapshot(
            &giver,
            &Items::default(),
            None,
            Ask::New(&NetCommands(tx)),
            &crate::npc_text::MacroContext {
                subject: None,
                states: &crate::world_state::WorldStates::default(),
            },
            None,
        )
        .unwrap();
        assert_eq!(snap.active_titles, ["Active A", "Active B"]);
        assert_eq!(snap.active_levels, [12, 9]);
        assert_eq!(snap.available_titles, ["Available A", "Available B"]);
        assert_eq!(snap.available_levels, [3, 5]);
    }

    #[test]
    fn status_store_survives_close() {
        let mut giver = QuestGiver::default();
        giver.set_status(0x99, dialog_status::AVAILABLE);
        giver.open(
            0x99,
            QuestView::Greeting(QuestGiverList {
                npc: 0x99,
                greeting: "Hi".into(),
                emote_delay: 0,
                emote: 0,
                quests: vec![],
            }),
        );
        giver.clear();
        assert!(!giver.is_open());
        assert_eq!(giver.status(0x99), Some(dialog_status::AVAILABLE));
    }

    /// Reasons 2 to 5 sit inside `0x5dbca0`'s jump table and still take its default.
    #[test]
    fn questgiver_invalid_keys_match_the_reference_table() {
        assert_eq!(questgiver_invalid_key(13), "ERR_QUEST_ALREADY_ON");
        assert_eq!(questgiver_invalid_key(1), "ERR_QUEST_FAILED_LOW_LEVEL");
        assert_eq!(questgiver_invalid_key(6), "ERR_QUEST_FAILED_WRONG_RACE");
        assert_eq!(questgiver_invalid_key(12), "ERR_QUEST_ONLY_ONE_TIMED");
        assert_eq!(questgiver_invalid_key(20), "ERR_QUEST_FAILED_MISSING_ITEMS");
        assert_eq!(
            questgiver_invalid_key(22),
            "ERR_QUEST_FAILED_NOT_ENOUGH_MONEY"
        );
        for reason in [0, 2, 3, 4, 5, 7, 11, 14, 19, 21, 23, 99, u32::MAX] {
            assert_eq!(
                questgiver_invalid_key(reason),
                "ERR_QUEST_NEED_PREREQS",
                "reason {reason} must take the ref's `ja` default"
            );
        }
    }

    #[test]
    fn questgiver_failed_keys_match_the_reference_table() {
        assert_eq!(questgiver_failed_key(4), "ERR_QUEST_FAILED_BAG_FULL_S");
        assert_eq!(questgiver_failed_key(50), "ERR_QUEST_FAILED_BAG_FULL_S");
        assert_eq!(questgiver_failed_key(17), "ERR_QUEST_FAILED_MAX_COUNT_S");
        for reason in [0, 1, 5, 16, 18, 49, 51] {
            assert_eq!(questgiver_failed_key(reason), "ERR_QUEST_FAILED_S");
        }
    }

    /// Reads the install's `GlobalStrings.lua`; skips without it.
    #[test]
    fn every_quest_refusal_key_resolves_in_the_real_global_strings() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");
        let g = |key: &str| s.lua().globals().get::<String>(key).ok();

        for reason in 0..=60u32 {
            for key in [
                questgiver_invalid_key(reason),
                questgiver_failed_key(reason),
            ] {
                let text = g(key).unwrap_or_default();
                assert!(!text.is_empty(), "{key} (reason {reason}) missing");
            }
            // The FAILED family names the quest; the INVALID family never does.
            assert!(g(questgiver_failed_key(reason))
                .unwrap_or_default()
                .contains("%s"));
            assert!(!g(questgiver_invalid_key(reason))
                .unwrap_or_default()
                .contains("%s"));
        }
        assert_eq!(
            ui_error_text(&UiError::key(questgiver_invalid_key(13)), &g).as_deref(),
            Some("You are already on that quest")
        );
        assert_eq!(
            ui_error_text(&UiError::s(questgiver_failed_key(4), "A Threat Within"), &g).as_deref(),
            Some("A Threat Within failed: Inventory is full.")
        );
        // The log-full line takes no fill.
        assert_eq!(
            ui_error_text(&UiError::key("ERR_QUEST_LOG_FULL"), &g).as_deref(),
            Some("Your quest log is full.")
        );
    }

    /// `QUEST_ITEM_UPDATE` fires only in the item-cache callback `0x5018c0` (`0x501903`), and the
    /// panel's event once per panel: the giver's name landing fires nothing.
    #[test]
    fn the_givers_name_landing_fires_no_quest_event() {
        const GIVER: u64 = 0x10;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let script = UiScript::new().unwrap();
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') \
                 for _, e in ipairs({'QUEST_GREETING', 'QUEST_ITEM_UPDATE'}) do \
                 f:RegisterEvent(e) end \
                 f:SetScript('OnEvent', function() table.insert(SEEN, event) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        open_panel(
            app.world_mut(),
            GIVER,
            QuestView::Greeting(benilla_protocol::messages::QuestGiverList {
                npc: GIVER,
                greeting: "Greetings.".into(),
                emote_delay: 0,
                emote: 0,
                quests: Vec::new(),
            }),
        );
        // Registered once, so its memo lasts from run to run.
        let feed = app.world_mut().register_system(feed_quest);
        let tick = |app: &mut App| -> Vec<String> {
            app.world_mut().run_system(feed).unwrap();
            let s = app.world_mut().non_send_resource_mut::<UiScript>();
            let seen = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            seen
        };
        assert_eq!(tick(&mut app), ["QUEST_GREETING"]);
        app.world_mut()
            .resource_mut::<NameCache>()
            .insert_player(GIVER, "Giver".into(), None);
        assert!(tick(&mut app).is_empty(), "a landed name fires nothing");
    }

    /// The feed over a headless client's world, registered once so its memo lasts from run to
    /// run, and a VM that records the panel events in `SEEN`.
    fn quest_app() -> (App, bevy::ecs::system::SystemId) {
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let script = UiScript::new().unwrap();
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') \
                 for _, e in ipairs({'QUEST_GREETING', 'QUEST_DETAIL', 'QUEST_PROGRESS', \
                 'QUEST_COMPLETE', 'QUEST_ITEM_UPDATE'}) do f:RegisterEvent(e) end \
                 f:SetScript('OnEvent', function() table.insert(SEEN, event) end)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let feed = app.world_mut().register_system(feed_quest);
        (app, feed)
    }

    fn quest_tick(app: &mut App, feed: bevy::ecs::system::SystemId) -> Vec<String> {
        app.world_mut().run_system(feed).unwrap();
        let s = app.world_mut().non_send_resource_mut::<UiScript>();
        let seen = s.eval::<Vec<String>>("return SEEN").unwrap();
        s.run("SEEN = {}").unwrap();
        seen
    }

    fn offer(quest_id: u32, title: &str, choice: u32) -> QuestView {
        QuestView::Detail(benilla_protocol::messages::QuestDetails {
            npc: 0x10,
            quest_id,
            title: title.into(),
            details: String::new(),
            objectives: String::new(),
            auto_finish: 0,
            choices: vec![triple(choice)],
            rewards: Vec::new(),
            money: 0,
            reward_spell: 0,
        })
    }

    /// Each panel setter fires its event at its end, every time (`0x500ee5`, `0x50105d`,
    /// `0x501120`): a second offer fires `QUEST_DETAIL` again, an identical one too.
    /// `QUEST_ITEM_UPDATE` fires only from the callback `GetQuestItemInfo`'s lookup keeps
    /// (`0x5017ce`), once per call, when the record lands (`0x5018fe`-`0x501903`); a record no
    /// call asked for fires nothing.
    #[test]
    fn panel_events_fire_per_publish_and_item_updates_per_callback() {
        let (mut app, feed) = quest_app();
        let publish = |app: &mut App, view| open_panel(app.world_mut(), 0x10, view);
        publish(&mut app, offer(100, "A Threat Within", 901));
        assert_eq!(quest_tick(&mut app, feed), ["QUEST_DETAIL"]);
        publish(&mut app, offer(101, "Kobold Camp Cleanup", 902));
        assert_eq!(
            quest_tick(&mut app, feed),
            ["QUEST_DETAIL"],
            "a second offer"
        );
        publish(&mut app, offer(101, "Kobold Camp Cleanup", 902));
        assert_eq!(
            quest_tick(&mut app, feed),
            ["QUEST_DETAIL"],
            "an identical one"
        );

        let land = |app: &mut App, entry| {
            app.world_mut()
                .resource_mut::<Items>()
                .insert_template(entry, Some(crate::items::test_template("Sword")));
        };
        // No call asked for 902: its landing repaints in place and fires nothing.
        land(&mut app, 902);
        assert!(quest_tick(&mut app, feed).is_empty());

        publish(&mut app, offer(102, "Westbrook Garrison", 903));
        assert_eq!(quest_tick(&mut app, feed), ["QUEST_DETAIL"]);
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .run("GetQuestItemInfo('choice', 1) GetQuestItemInfo('choice', 1)")
            .unwrap();
        assert!(
            quest_tick(&mut app, feed).is_empty(),
            "the record is still out"
        );
        land(&mut app, 903);
        assert_eq!(
            quest_tick(&mut app, feed),
            ["QUEST_ITEM_UPDATE", "QUEST_ITEM_UPDATE"],
            "one per call"
        );
        assert!(
            quest_tick(&mut app, feed).is_empty(),
            "the callbacks are spent"
        );
    }
}
