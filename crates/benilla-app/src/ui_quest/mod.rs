//! The questgiver window's app side: the handlers fill [`QuestGiver`], [`feed_quest`] pushes it
//! into the VM and fires the panel's event, and [`drain_quest`] sends the Lua intents.

use std::collections::HashMap;

use benilla_protocol::messages::{
    QuestDetails, QuestGiverList, QuestOfferReward, QuestRequestItems, QuestRewardItem,
    QuestShareMsg,
};
use bevy::prelude::*;

use benilla_ui::script::{
    QuestAction, QuestItemView, QuestPanel, QuestRewardSpell, QuestState, UiScript,
};

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
use crate::ui_action::{show_messages, ui_error_text, MessageSink, Shown, Spells, UiError};
use crate::ui_script::{SweepApp, SweepLeg, UiFeed, UiInput};
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
    /// Per-guid dialog status from `SMSG_QUESTGIVER_STATUS`, the `!`/`?` marker's value.
    statuses: HashMap<u64, u32>,
    /// Messages queued for [`feed_quest`], each a GlobalStrings key and its fills.
    messages: Vec<UiError>,
    reask: u32,
    /// The dialog statuses dropped at the reference's typemask-8 lookup (GameObject answers, which
    /// vmangos sends), per guid, so a live probe can tell a drop from a query never sent.
    refused: HashMap<u64, u32>,
    refused_count: u32,
}

impl QuestGiver {
    /// Open (or replace) the window with a fresh wire view for `npc`.
    pub(crate) fn open(&mut self, npc: u64, view: QuestView) {
        self.npc = Some(npc);
        self.view = Some(view);
        // `0x500bd0` clears the already-acted latch (`0x500d91`) and the chosen reward
        // (`0x500d8c`) on every panel publish.
        self.acted = false;
        self.chosen_reward = 0;
    }

    #[allow(dead_code)]
    pub(crate) fn is_open(&self) -> bool {
        self.view.is_some()
    }

    /// The one-shot turn-in fanfare, which the reference plays from the packet, not from Lua.
    pub(crate) fn take_completed_fanfare(&mut self) -> bool {
        std::mem::take(&mut self.completed_fanfare)
    }

    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.view = None;
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

    /// The open panel's title if it shows `quest_id`: the `%s` of a refusal naming the quest.
    pub(crate) fn view_title(&self, quest_id: u32) -> Option<String> {
        match self.view.as_ref()? {
            QuestView::Detail(d) if d.quest_id == quest_id => Some(d.title.clone()),
            QuestView::Progress(p) if p.quest_id == quest_id => Some(p.title.clone()),
            QuestView::Reward(o) if o.quest_id == quest_id => Some(o.title.clone()),
            _ => None,
        }
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

pub(crate) use lines::{QuestLine, QuestLines};

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
                (
                    // The feed before the input pass and the drain after it, so a click goes out
                    // the same frame.
                    feed_quest.in_set(UiFeed),
                    drain_quest.after(UiInput),
                ),
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
/// (`0x501a10`), so a walk-away from a shared quest still declines it while an NPC's panel closes
/// with no giver re-open.
impl LatchedWindow for QuestGiver {
    const WINDOW: NpcWindow = NpcWindow::Quest;

    fn cell(&self) -> Option<u64> {
        self.npc
    }

    fn dispatched(world: &mut World) {
        close_core(world, true);
    }

    fn take_close_verb(script: &mut UiScript) -> bool {
        script.take_quest_close()
    }
}

/// The panel's one teardown `0x501130(0, report)`: nothing with no giver (`0x501149`); with
/// `report`, a sharing player is sent `MSG_QUEST_PUSH_RESULT` DECLINE (`0x501159`-`0x5011e0`), as
/// the reference's player lookup there resolves only a player; then the cell is zeroed
/// (`0x501201`) before the latch release (`0x50120d`), so the core never re-enters, and
/// `QUEST_FINISHED` fires once (`0x501217`).
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

/// One greeting pool as `(quest_id, wire icon)` rows; the icon feeds [`row_is_one_click`].
type Pool = Vec<(u32, u32)>;

/// One wire item row for Lua; the name and quality are `None` and white until the template lands.
fn resolve_item(
    it: &QuestRewardItem,
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> QuestItemView {
    let template = items.template(it.item_id, 0, commands);
    let name = template.map(|t| t.name.clone());
    let quality = template.map(|t| t.quality).unwrap_or(1);
    let texture = icons
        .and_then(|i| i.catalog.get(it.display_id))
        .and_then(|d| d.icon.clone());
    // `GetQuestItemLink`'s payload; `None` until the template gives the name and quality.
    let link = name
        .as_ref()
        .map(|n| crate::ui_items::item_link(it.item_id, n, quality));
    QuestItemView {
        name,
        texture,
        count: it.count,
        quality,
        item_id: it.item_id,
        usable: true, // not computed; the stock frame tints unusable ones red
        link,
    }
}

fn resolve_items(
    src: &[QuestRewardItem],
    items: &Items,
    icons: Option<&ItemDisplays>,
    commands: &NetCommands,
) -> Vec<QuestItemView> {
    src.iter()
        .map(|it| resolve_item(it, items, icons, commands))
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
    commands: &NetCommands,
    macros: &crate::npc_text::MacroContext,
    spells: Option<&Spells>,
) -> Option<QuestState> {
    // The wire sends quest text unexpanded; the client substitutes `$N`, `$B` and `$G`.
    let sub = |t: &str| crate::npc_text::substitute(t, macros);
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
            choices: resolve_items(&d.choices, items, icons, commands),
            rewards: resolve_items(&d.rewards, items, icons, commands),
            reward_money: d.money.max(0) as u32,
            reward_spell: reward_spell_view(d.reward_spell, spells),
            ..Default::default()
        },
        QuestView::Progress(p) => QuestState {
            panel: QuestPanel::Progress,
            title: sub(&p.title),
            body: sub(&p.request_text),
            required: resolve_items(&p.required_items, items, icons, commands),
            required_money: p.required_money,
            completable: p.is_complete,
            ..Default::default()
        },
        QuestView::Reward(o) => QuestState {
            panel: QuestPanel::Reward,
            title: sub(&o.title),
            body: sub(&o.offer_text),
            choices: resolve_items(&o.choices, items, icons, commands),
            rewards: resolve_items(&o.rewards, items, icons, commands),
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

/// Push the quest view into the VM and fire its events: a panel's open event on a new panel,
/// `QUEST_ITEM_UPDATE` on a content change. The close is the core's ([`close_core`]).
pub(crate) fn feed_quest(
    script: Option<NonSendMut<UiScript>>,
    mut giver: ResMut<QuestGiver>,
    objects: crate::net::Objects,
    items: Res<Items>,
    icons: Option<Res<ItemDisplays>>,
    commands: Res<NetCommands>,
    names: Res<NameCache>,
    states: Res<crate::world_state::WorldStates>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    spells: Option<Res<Spells>>,
    go_templates: Res<crate::go_templates::GameObjectTemplates>,
    materials: Option<Res<crate::ui_item_text::PageMaterials>>,
    mut sink: MessageSink,
    mut last: crate::ui_script::WindowMemo<Option<QuestState>>,
    mut last_name: crate::ui_script::WindowMemo<Option<String>>,
    mut closes: CoreCloses,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_name = last_name.get(&script);
    if closes.since_last(&script, NpcWindow::Quest) {
        *last = None;
        *last_name = None;
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
    show_messages(&mut script, &mut sink, "ui_quest", lines);
    let player = crate::npc_text::player_identity(&self_q, &names, &commands);
    let fresh = snapshot(
        &giver,
        &items,
        icons.as_deref(),
        &commands,
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
                &commands,
            )
        });
        st
    });
    let npc_name = giver
        .npc
        .and_then(|g| names.resolve(g, &commands).map(str::to_string));
    let name_changed = *last_name != npc_name;
    if fresh == *last && !name_changed {
        return;
    }
    script.set_quest(fresh.clone());
    // The panel events carry no argument: the frame reads `UnitName("npc")` (`QuestFrame.lua:63`).
    // A close is the core's, which told the VM; a disconnect's clear tells it nothing.
    if let Some(f) = &fresh {
        let same_panel = last.as_ref().is_some_and(|l| l.panel == f.panel);
        let event = if same_panel {
            "QUEST_ITEM_UPDATE"
        } else {
            panel_event(f.panel)
        };
        script.fire_event(event, vec![]);
    }
    *last = fresh;
    *last_name = npc_name;
}

/// `CloseQuest()`'s binding `0x501a10`, and `DeclineQuest`'s teardown leg (`0x5014aa`): the core
/// with its report set, which declines a sharing player's quest and closes anything else
/// silently. The player test is the guid's shape; the reference's `0x468460` also needs the
/// sharer in view, which the watchdog's 14 yd leash ensures.
fn close_quest_reporting(world: &mut World) {
    close_core(world, true);
}

/// `AcceptQuest()`'s close (`0x5013d3`): the core with no report.
fn close_quest_silently(world: &mut World) {
    close_core(world, false);
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

/// `DeclineQuest`'s fork (`0x5013f0`, called by its binding `0x501d30`); the re-open legs leave
/// the window up, and items and players are taken by guid shape. In order:
///
/// 1. the already-acted latch set, or the source unresolvable: nothing;
/// 2. a unit with the gossip NPC flag: `CMSG_GOSSIP_HELLO`, whatever `0xbe0824` says;
/// 3. an item, a player, or a non-zero `0xbe0824`: the teardown `0x501130(0,1)`;
/// 4. a GameObject: its interact virtual `[vtbl+0x60]` (`0x5014d0`), whose override is untraced,
///    so it takes the teardown here;
/// 5. any other unit: `CMSG_QUESTGIVER_HELLO`.
fn decline_leg(npc: u64, acted: bool, close_on_cancel: u32, npc_flags: Option<u32>) -> DeclineLeg {
    use benilla_protocol::guid;
    if acted {
        return DeclineLeg::Nothing;
    }
    if guid::is_player(npc) || guid::is_item(npc) {
        return DeclineLeg::Teardown;
    }
    let Some(flags) = npc_flags else {
        return DeclineLeg::Nothing;
    };
    let unit = guid::is_creature_or_pet(npc);
    if unit && flags & crate::target::cursor_mode::npc_flags::GOSSIP != 0 {
        return DeclineLeg::GossipHello;
    }
    if close_on_cancel != 0 || !unit {
        return DeclineLeg::Teardown;
    }
    DeclineLeg::QuestgiverHello
}

/// Drain the Lua intents: greeting-row selects and the panel buttons, each to its `CMSG`.
fn drain_quest(
    script: Option<NonSendMut<UiScript>>,
    mut giver: ResMut<QuestGiver>,
    commands: Res<NetCommands>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
    mut world_commands: Commands,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some(npc) = giver.npc else {
        // Drain the VM anyway, so intents do not queue against a closed window.
        script.take_quest_selects();
        script.take_quest_actions();
        return;
    };
    // The giver's live `UNIT_NPC_FLAGS`, `None` when it is not streamed.
    let npc_flags = index
        .0
        .get(&npc)
        .and_then(|e| stores.get(*e).ok())
        .map(|s| s.0.unit_npc_flags());

    for sel in script.take_quest_selects() {
        let Some(QuestView::Greeting(list)) = giver.view.as_ref() else {
            continue;
        };
        // The snapshot's split again, keeping the icon for the one-click flag.
        let (mut active, mut available): (Pool, Pool) = (Vec::new(), Vec::new());
        for q in &list.quests {
            if row_is_active(q.icon) {
                active.push((q.quest_id, q.icon));
            } else {
                available.push((q.quest_id, q.icon));
            }
        }
        let pool = if sel.active { &active } else { &available };
        let Some(&(quest, icon)) = sel.index.checked_sub(1).and_then(|i| pool.get(i as usize))
        else {
            debug!("ui_quest: greeting select {sel:?} out of range — ignored");
            continue;
        };
        // `SelectActiveQuest` (`0x501320`) always sends COMPLETE_QUEST; `SelectAvailableQuest`
        // (`0x5012a0`) does too for a one-click row, and QUERY_QUEST otherwise.
        let cmd = if sel.active || row_is_one_click(icon) {
            ClientCommand::QuestgiverComplete { npc, quest }
        } else {
            ClientCommand::QuestgiverQuery { npc, quest }
        };
        let _ = commands.0.send(cmd);
    }

    let view_quest = giver.view.as_ref().and_then(|v| match v {
        QuestView::Detail(d) => Some(d.quest_id),
        QuestView::Progress(p) => Some(p.quest_id),
        QuestView::Reward(o) => Some(o.quest_id),
        QuestView::Greeting(_) => None,
    });
    for action in script.take_quest_actions() {
        match action {
            // A close ends the drain: the reference's verbs after it find no panel.
            QuestAction::Close => {
                crate::ui_session::queue(&mut world_commands, close_quest_reporting);
                return;
            }
            QuestAction::Decline => {
                let leg = decline_leg(npc, giver.acted, giver.close_on_cancel, npc_flags);
                debug!("ui_quest: decline on {npc:#x} — {leg:?}");
                match leg {
                    DeclineLeg::Nothing => {}
                    DeclineLeg::Teardown => {
                        crate::ui_session::queue(&mut world_commands, close_quest_reporting);
                        return;
                    }
                    DeclineLeg::GossipHello => {
                        let _ = commands.0.send(ClientCommand::GossipHello { guid: npc });
                        giver.acted = true;
                    }
                    DeclineLeg::QuestgiverHello => {
                        let _ = commands.0.send(ClientCommand::QuestgiverHello { npc });
                        giver.acted = true;
                    }
                }
            }
            QuestAction::Accept => {
                // `0x501380` refuses while the already-acted latch is set (`0x5013a8`).
                if giver.acted {
                    continue;
                }
                if let Some(quest) = view_quest {
                    giver.acted = true;
                    let _ = commands
                        .0
                        .send(ClientCommand::QuestgiverAccept { npc, quest });
                    // `AcceptQuest`'s tail (`0x5013df`) triggers the Quest Log tutorial.
                    if let Some(t) = tutorials.as_mut() {
                        t.write(crate::tutorial::TutorialEvent::trigger(
                            crate::tutorial::id::QUEST_LOG,
                        ));
                    }
                    // The reference closes on the click, not on an answer: it sends `0x189`
                    // (`0x5eac10`), then clears with `0x501130(0,0)`, firing `QUEST_FINISHED`.
                    crate::ui_session::queue(&mut world_commands, close_quest_silently);
                    return;
                }
            }
            QuestAction::Continue => {
                if let Some(quest) = view_quest {
                    let _ = commands
                        .0
                        .send(ClientCommand::QuestgiverRequestReward { npc, quest });
                }
            }
            QuestAction::Reward(choice) => {
                if let Some(quest) = view_quest {
                    // `0x5015b0` latches the chosen row's item when the panel offers a choice.
                    if let Some(QuestView::Reward(o)) = giver.view.as_ref() {
                        if let Some(it) = o.choices.get(choice as usize) {
                            giver.chosen_reward = it.item_id;
                        }
                    }
                    let _ = commands.0.send(ClientCommand::QuestgiverChooseReward {
                        npc,
                        quest,
                        choice,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::dialog_status;

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
            race: 1,
            class: 1,
            gender: 0,
        };
        let snap = snapshot(
            &giver,
            &items,
            None,
            &commands,
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
        // No template yet: no name, and white.
        assert!(snap.rewards[0].name.is_none());
        assert_eq!(snap.rewards[0].quality, 1);
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
            &commands,
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
            &NetCommands(tx),
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

    #[test]
    fn view_title_answers_only_for_the_open_quest() {
        let mut giver = QuestGiver::default();
        assert_eq!(giver.view_title(100), None);
        giver.open(
            0x42,
            QuestView::Detail(QuestDetails {
                npc: 0x42,
                quest_id: 100,
                title: "A Threat Within".into(),
                details: String::new(),
                objectives: String::new(),
                auto_finish: 0,
                choices: vec![],
                rewards: vec![],
                money: 0,
                reward_spell: 0,
            }),
        );
        assert_eq!(giver.view_title(100).as_deref(), Some("A Threat Within"));
        assert_eq!(giver.view_title(101), None);
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

    // ── DeclineQuest and CloseQuest ──────────────────────────────────────────────

    /// Run `lua` on a window open on `giver` with `0xbe0824` at `close_on_cancel` and return the
    /// sends; `npc_flags` streams the giver with those flags, `None` leaves it unstreamed.
    fn drain_after(
        giver: u64,
        npc_flags: Option<u32>,
        close_on_cancel: u32,
        lua: &str,
    ) -> Vec<ClientCommand> {
        drain_world(giver, npc_flags, close_on_cancel, lua).1
    }

    /// [`drain_after`], keeping the world so a test can read the [`QuestGiver`] afterwards.
    fn drain_world(
        giver: u64,
        npc_flags: Option<u32>,
        close_on_cancel: u32,
        lua: &str,
    ) -> (App, Vec<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx));
        app.init_resource::<GuidIndex>();
        if let Some(flags) = npc_flags {
            // 147 = `UNIT_NPC_FLAGS` (the descriptor index `unit_npc_flags()` reads).
            let fields = benilla_protocol::ObjectFields::from_pairs(&[(147, flags)]);
            let e = app.world_mut().spawn(ObjectStore(fields)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(giver, e);
        }
        let mut quest = QuestGiver::default();
        quest.open(
            giver,
            QuestView::Detail(QuestDetails {
                npc: giver,
                quest_id: 1,
                title: "A Threat Within".into(),
                details: String::new(),
                objectives: String::new(),
                auto_finish: close_on_cancel,
                choices: Vec::new(),
                rewards: Vec::new(),
                money: 0,
                reward_spell: 0,
            }),
        );
        // The latch is written by the packet, after the open (as `net::quest_detail` does).
        quest.close_on_cancel = close_on_cancel;
        app.insert_resource(quest);
        let script = UiScript::new().unwrap();
        script.run(lua).unwrap();
        app.insert_non_send_resource(script);
        app.add_systems(Update, drain_quest);
        app.update();
        let sent = rx.try_iter().collect();
        (app, sent)
    }

    const SHARER: u64 = 0x0000_0000_0000_002A; // HIGHGUID_PLAYER: a zero high word
    const NPC: u64 = 0xF130_0000_0000_0007; // HIGHGUID_UNIT

    /// Both verbs reach the teardown `0x501130(0,1)`: `CloseQuest` directly, `DeclineQuest` through
    /// its player leg.
    #[test]
    fn every_way_out_of_a_shared_quest_answers_the_sharer() {
        for verb in ["DeclineQuest()", "CloseQuest()"] {
            let sent = drain_after(SHARER, None, 0, verb);
            assert!(
                matches!(
                    sent.as_slice(),
                    [ClientCommand::QuestPushResult {
                        sharer: SHARER,
                        msg: QuestShareMsg::DECLINE_QUEST,
                    }]
                ),
                "{verb} must answer the sharer: {sent:?}"
            );
        }
    }

    #[test]
    fn ending_the_session_on_an_npc_reopens_its_list() {
        use crate::target::cursor_mode::npc_flags;

        let sent = drain_after(NPC, Some(0), 0, "DeclineQuest()");
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::QuestgiverHello { npc: NPC }]
            ),
            "a plain questgiver gets QUESTGIVER_HELLO: {sent:?}"
        );

        let sent = drain_after(NPC, Some(npc_flags::GOSSIP), 0, "DeclineQuest()");
        assert!(
            matches!(sent.as_slice(), [ClientCommand::GossipHello { guid: NPC }]),
            "a gossip-flagged NPC gets GOSSIP_HELLO: {sent:?}"
        );
    }

    /// `0xbe0824`'s one reader is `0x5014a2`.
    #[test]
    fn a_non_zero_latch_suppresses_a_plain_units_reopen() {
        let sent = drain_after(NPC, Some(0), 1, "DeclineQuest()");
        assert!(sent.is_empty(), "flag 1 suppresses the re-open: {sent:?}");
    }

    /// `CloseQuest` tears the window down; `DeclineQuest` bails first (`0x50142c`) and leaves it.
    #[test]
    fn an_unstreamed_giver_ends_the_session_silently() {
        let (app, sent) = drain_world(NPC, None, 0, "CloseQuest()");
        assert!(sent.is_empty(), "no unit, no re-open: {sent:?}");
        assert!(!app.world().resource::<QuestGiver>().is_open());

        let (app, sent) = drain_world(NPC, None, 0, "DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert!(app.world().resource::<QuestGiver>().is_open());
    }

    #[test]
    fn declining_an_item_quest_closes_silently() {
        const ITEM: u64 = 0x4000_0000_0000_0099; // HIGHGUID_ITEM
        let (app, sent) = drain_world(ITEM, None, 0, "DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert!(!app.world().resource::<QuestGiver>().is_open());
    }

    /// ESC, the X and the greeting's Goodbye all reach `CloseQuest` through `QuestFrame_OnHide`
    /// (`QuestFrame.lua:293`).
    #[test]
    fn close_quest_on_an_npc_is_network_silent() {
        use crate::target::cursor_mode::npc_flags;
        for flags in [0, npc_flags::GOSSIP] {
            for latch in [0, 1] {
                let (app, sent) = drain_world(NPC, Some(flags), latch, "CloseQuest()");
                assert!(
                    sent.is_empty(),
                    "CloseQuest on an NPC (flags {flags:#x}, latch {latch}) sends nothing: {sent:?}"
                );
                assert!(
                    !app.world().resource::<QuestGiver>().is_open(),
                    "…and tears the window down"
                );
            }
        }
    }

    /// The gossip leg (`0x50143b`) is tested before the latch (`0x5014a2`).
    #[test]
    fn the_gossip_leg_precedes_the_latch() {
        use crate::target::cursor_mode::npc_flags;
        let sent = drain_after(NPC, Some(npc_flags::GOSSIP), 1, "DeclineQuest()");
        assert!(
            matches!(sent.as_slice(), [ClientCommand::GossipHello { guid: NPC }]),
            "a latched gossip NPC still gets GOSSIP_HELLO: {sent:?}"
        );
    }

    /// The acted latch `0xbe0844` refuses the second `DeclineQuest` and the `AcceptQuest`.
    #[test]
    fn a_reopening_decline_keeps_the_window_and_latches_acted() {
        let (app, sent) = drain_world(
            NPC,
            Some(0),
            0,
            "DeclineQuest() DeclineQuest() AcceptQuest()",
        );
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::QuestgiverHello { npc: NPC }]
            ),
            "one re-open, and nothing after it: {sent:?}"
        );
        let giver = app.world().resource::<QuestGiver>();
        assert!(
            giver.is_open(),
            "the window stays up for the server's answer"
        );
    }

    /// The latch-diverted leg (`0x5014aa`) takes the teardown `0x501130(0,1)`.
    #[test]
    fn a_latched_decline_closes_the_window() {
        let (app, sent) = drain_world(NPC, Some(0), 1, "DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert!(!app.world().resource::<QuestGiver>().is_open());
    }
}
