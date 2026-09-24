//! The app-side **questgiver feed** (decision 0088) — the inward half of the quest seam around
//! [`benilla_ui::script`]'s `quest` module, the twin of [`crate::ui_gossip`]/[`crate::ui_merchant`].
//!
//! The net bridge fills [`QuestGiver`] from the wire: `SMSG_QUESTGIVER_QUEST_LIST` → the greeting
//! panel, `_QUEST_DETAILS` → the accept panel, `_REQUEST_ITEMS` → the progress panel,
//! `_OFFER_REWARD` → the reward panel, `_QUEST_COMPLETE` → the turn-in result (closes the window),
//! and `SMSG_QUESTGIVER_STATUS` → the per-guid dialog-status store (world markers are a later
//! slice — decision 0088). Each frame [`feed_quest`] resolves the open view into a
//! [`QuestState`] snapshot (item names via the ask-once template cache, icons straight from the
//! wire display id — the merchant's pattern), pushes it ([`UiScript::set_quest`]), and fires the
//! matching FrameXML event (`QUEST_GREETING`/`QUEST_DETAIL`/`QUEST_PROGRESS`/`QUEST_COMPLETE` on a
//! panel change, `QUEST_ITEM_UPDATE` on an in-place content change, `QUEST_FINISHED` on clear).
//! [`drain_quest`] pulls the Lua intents back out and maps each to a `CMSG_QUESTGIVER_*` addressed
//! to `(npc, questId)` from the open view.

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
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::{close_npc_session_out_of_range, npc_switched, NpcSession};

/// The open questgiver view — exactly the wire packet that opened the current panel. The feed turns
/// it into the Lua snapshot; the drain reads the npc/quest ids off it for the outbound CMSG.
pub(crate) enum QuestView {
    Greeting(QuestGiverList),
    Detail(QuestDetails),
    Progress(QuestRequestItems),
    Reward(QuestOfferReward),
}

/// The open questgiver window, filled by the net bridge ([`crate::net`]) and read by [`feed_quest`].
/// Cleared on the turn-in result, a client-side close, and disconnect. The `statuses` map is the
/// DIALOG_STATUS store-now/render-later surface (decision 0088) — it survives the window closing
/// (a per-guid fact, like the gossip greeting cache).
#[derive(Resource, Default)]
pub(crate) struct QuestGiver {
    /// Set by the net apply on `SMSG_QUESTGIVER_QUEST_COMPLETE`; drained by the feed
    /// ([`Self::take_completed_fanfare`]).
    pub(crate) completed_fanfare: bool,
    /// The questgiver whose window is open; `None` = no window open.
    pub(crate) npc: Option<u64>,
    /// The open panel's wire view.
    pub(crate) view: Option<QuestView>,
    /// The reference's `0xbe0824` — a **latch** written by THREE panel packets, not a field of
    /// the open view: `SMSG_QUESTGIVER_QUEST_DETAILS`' trailing `u32` (publisher `0x500ef0`,
    /// `0x50101a`), `SMSG_QUESTGIVER_OFFER_REWARD`'s `u32` after the reward text (the same
    /// publisher), and `SMSG_QUESTGIVER_REQUEST_ITEMS`' `closeOnCancel` (`0x501070`, `0x50111a`);
    /// zeroed only by the world-enter reset `0x500af0`. The greeting packet leaves it alone. Its
    /// one reader is `DeclineQuest`'s fork ([`decline_leg`]), where non-zero diverts a plain
    /// unit's decline from the giver re-open to the silent teardown — how the server says "this
    /// panel was not reached through a menu, there is nothing to go back to" (vmangos sends 1 on
    /// every DETAILS and OFFER_REWARD, and `closeOnCancel` = 1 on a single-quest auto-open,
    /// 0 on the `COMPLETE_QUEST` path, `QuestHandler.cpp:390-395`).
    pub(crate) close_on_cancel: u32,
    /// The reference's "already acted" latch `0xbe0844`: set by every acting leg of
    /// `AcceptQuest` (`0x5013c9`) and `DeclineQuest` (`0x50147a`/`0x5014d5`/`0x501549`), refusing
    /// both verbs while set (`0x5013a8`, `0x5013fe`); cleared by each panel publish (`0x500bd0`,
    /// `0x500d91` — our [`Self::open`]). It matters because a re-opening decline does not tear
    /// the window down: the panel stays up until the server's answer replaces it.
    pub(crate) acted: bool,
    /// Per-guid dialog status (`SMSG_QUESTGIVER_STATUS`) — the `!`/`?` marker's value, stored for a
    /// later world-marker slice.
    statuses: HashMap<u64, u32>,
    /// Client messages the net apply queued for [`feed_quest`] to resolve and show — the
    /// reference's `DisplayError(msgId)` split into (surface, GlobalStrings key + fills), so the
    /// line comes from the VM's own strings and never from a hardcoded English literal
    /// (decision 0669).
    messages: Vec<UiError>,
    /// The re-ask epoch — see [`Self::bump_reask`].
    reask: u32,
    /// What each **refused** `SMSG_QUESTGIVER_STATUS` said, per guid, and how many we have refused
    /// this session.
    ///
    /// The only trace of a packet class benilla deliberately throws on the floor. benilla asks the
    /// server about quest-flagged GameObjects because the reference does, and vmangos — unlike the
    /// real 1.12 service, which never answered one — replies; the reference's own handler drops
    /// that reply at the typemask-8 lookup and so does ours (decision 1872,
    /// [`crate::net::apply`]'s `quest_giver_status`). Without this, the *whole* GameObject half of
    /// the feature is invisible from inside the client: a correct drop and a query that was never
    /// sent both read as "no status", which is exactly the pair a live probe has to tell apart.
    /// Keyed by guid, and pruned with [`Self::retain_statuses`] — **not** a single last-writer
    /// slot, which is what this was and why it read as a regression the first time several quest
    /// objects were in view at once: every one of them is answered in the same packet drain, so
    /// only the last survived to be looked at, and which object that was is archetype order.
    refused: HashMap<u64, u32>,
    refused_count: u32,
}

impl QuestGiver {
    /// Open (or replace) the window with a fresh wire view for `npc`.
    pub(crate) fn open(&mut self, npc: u64, view: QuestView) {
        self.npc = Some(npc);
        self.view = Some(view);
        // `0x500bd0` clears the already-acted latch on every panel publish (`0x500d91`).
        self.acted = false;
    }

    /// Whether a quest window is currently open (a predicate for callers + the module tests).
    #[allow(dead_code)]
    pub(crate) fn is_open(&self) -> bool {
        self.view.is_some()
    }

    /// Close the open window (turn-in result / client-side close). Keeps the status store.
    /// One-shot: the turn-in result landed (`SMSG_QUESTGIVER_QUEST_COMPLETE`) — the giver feed
    /// drains it into the QUESTCOMPLETED kit (the fanfare the client's C++ fires on this packet,
    /// not any Lua handler).
    pub(crate) fn take_completed_fanfare(&mut self) -> bool {
        std::mem::take(&mut self.completed_fanfare)
    }

    pub(crate) fn clear(&mut self) {
        self.npc = None;
        self.view = None;
    }

    /// Record a dialog status for an NPC guid (store-now; the world marker renders in a later slice).
    pub(crate) fn set_status(&mut self, npc: u64, status: u32) {
        self.statuses.insert(npc, status);
    }

    /// Every stored dialog status, per guid — the overhead-marker renderer's read
    /// ([`crate::quest_markers`]).
    pub(crate) fn statuses(&self) -> &HashMap<u64, u32> {
        &self.statuses
    }

    /// Drop the cached status of every guid `live` rejects. The reference caches its answer on the
    /// unit object itself (`unit+0xcb8`), so the cache dies with the object; ours is a map keyed by
    /// guid and needs the explicit prune, or a unit re-entering view renders its stale marker until
    /// the fresh answer lands (decision 0647).
    pub(crate) fn retain_statuses(&mut self, live: impl Fn(u64) -> bool) {
        self.statuses.retain(|&npc, _| live(npc));
        // The refusal readout follows the same lifetime, for the same reason and to stay bounded.
        self.refused.retain(|&npc, _| live(npc));
    }

    /// Drop one guid's cached status — the NPC stopped being a questgiver, so its marker goes with
    /// it (the reference's own teardown branch; decision 0647).
    pub(crate) fn clear_status(&mut self, npc: u64) {
        self.statuses.remove(&npc);
    }

    /// Re-ask every visible questgiver's status: a packet landed that can change the server's
    /// answer for everyone. The reference sweeps from four such handlers — reputation
    /// (`SMSG_SET_FACTION_STANDING`), the party/raid roster (`SMSG_GROUP_LIST`), the
    /// `SMSG_QUESTGIVER_*` demux (turn-ins) and `SMSG_QUESTUPDATE_*` — the packet half of the
    /// refresh law whose descriptor half is `quest_markers::self_generation` (decision 0654).
    ///
    /// A counter, not a flag: `quest_markers::query_statuses` folds it into its generation, so a
    /// bump can't be lost to system ordering or to a frame that coalesced two packets.
    pub(crate) fn bump_reask(&mut self) {
        self.reask = self.reask.wrapping_add(1);
    }

    /// The current re-ask epoch — see [`Self::bump_reask`].
    pub(crate) fn reask_epoch(&self) -> u32 {
        self.reask
    }

    /// Record a dialog status the typemask gate refused — see [`Self::refused`].
    pub(crate) fn refuse_status(&mut self, npc: u64, status: u32) {
        self.refused.insert(npc, status);
        self.refused_count = self.refused_count.saturating_add(1);
    }

    /// What the server last answered for `npc` before we refused it, if anything.
    pub(crate) fn refused_for(&self, npc: u64) -> Option<u32> {
        self.refused.get(&npc).copied()
    }

    /// How many answers we have refused this session — the summary half of [`Self::refused`].
    pub(crate) fn refused_count(&self) -> u32 {
        self.refused_count
    }

    /// The stored dialog status for `npc`, if any. The store-now half of DIALOG_STATUS
    /// (decision 0088): no consumer yet — the `!`/`?` world marker is a later nameplate slice — so
    /// this accessor is deliberately unused for now.
    #[allow(dead_code)]
    pub(crate) fn status(&self, npc: u64) -> Option<u32> {
        self.statuses.get(&npc).copied()
    }

    /// The open panel's title when the panel IS `quest_id`'s — the `%s` fill for a refusal that
    /// names the quest (decision 0669). The reference reads that title off the quest record it
    /// looked the refusal up in; the panel being refused is the same quest, already in hand.
    pub(crate) fn view_title(&self, quest_id: u32) -> Option<String> {
        match self.view.as_ref()? {
            QuestView::Detail(d) if d.quest_id == quest_id => Some(d.title.clone()),
            QuestView::Progress(p) if p.quest_id == quest_id => Some(p.title.clone()),
            QuestView::Reward(o) if o.quest_id == quest_id => Some(o.title.clone()),
            _ => None,
        }
    }

    /// Queue one client message for [`feed_quest`] to resolve and show — the net apply's half of
    /// the reference's `DisplayError(msgId)` (decision 0669). The message carries its own surface
    /// in its key; the caller does not choose one (decision 1770).
    pub(crate) fn push_message(&mut self, msg: UiError) {
        self.messages.push(msg);
    }

    /// Take the queued client messages (drained by [`feed_quest`], which owns the VM).
    pub(crate) fn take_messages(&mut self) -> Vec<UiError> {
        std::mem::take(&mut self.messages)
    }

    /// Disconnect: drop the open window (mirrors the gossip/merchant session clears).
    pub(crate) fn clear_session(&mut self) {
        self.clear();
        self.statuses.clear();
        self.messages.clear();
        self.close_on_cancel = 0;
        self.acted = false;
    }
}

/// `SMSG_QUESTGIVER_QUEST_INVALID`'s `QuestFailedReason` → its GlobalStrings key, resolved to text
/// through the VM's own `GlobalStrings.lua` at the feed. The FULL build-5875 table, read off the
/// handler `0x5dbca0` (the `0x18f` arm of the quest demux `0x5e5910`): `lea eax,[reason-1]` /
/// `cmp eax,0x15` / `movzx edx,[eax+0x5dbd30]` / `jmp [edx*4+0x5dbd14]` — a 22-byte case index into
/// a 7-way jump table, every arm a `DisplayError(msgId)`. **Everything unlisted — including
/// reason 0 and anything past 22 — falls to `ERR_QUEST_NEED_PREREQS`**, which is the ref's own
/// `ja` default, not a guess. Cross-checks against vmangos `QuestDef.h`'s own per-value comments
/// (decision 0669).
pub(crate) fn questgiver_invalid_key(reason: u32) -> &'static str {
    match reason {
        1 => "ERR_QUEST_FAILED_LOW_LEVEL",         // msgId 142
        6 => "ERR_QUEST_FAILED_WRONG_RACE",        // msgId 144
        12 => "ERR_QUEST_ONLY_ONE_TIMED",          // msgId 146
        13 => "ERR_QUEST_ALREADY_ON",              // msgId 148
        20 => "ERR_QUEST_FAILED_MISSING_ITEMS",    // msgId 143
        22 => "ERR_QUEST_FAILED_NOT_ENOUGH_MONEY", // msgId 145
        _ => "ERR_QUEST_NEED_PREREQS",             // msgId 147 — the `ja` default
    }
}

/// `SMSG_QUESTGIVER_QUEST_FAILED`'s reason → its GlobalStrings key. The full table, read off the
/// handler `0x5dc840` (the `0x192` arm of the same demux): a three-way `cmp` chain on the reason —
/// `4`/`0x32` → BAG_FULL, `0x11` → MAX_COUNT, everything else → the plain FAILED line. All three
/// strings carry a `%s` the caller fills with the quest title (the ref pushes `questRecord+0x9c`
/// alongside the msgId). Decision 0669.
///
/// The wording is the player's own and is never restated here (decision 2045) — the msgId is what
/// identifies each arm, and `every_quest_refusal_key_resolves_in_the_real_global_strings` is what
/// checks the three against the shipped table.
pub(crate) fn questgiver_failed_key(reason: u32) -> &'static str {
    match reason {
        4 | 50 => "ERR_QUEST_FAILED_BAG_FULL_S", // msgId 140 — the bag-full wording
        17 => "ERR_QUEST_FAILED_MAX_COUNT_S",    // msgId 141 — the duplicate-item wording
        _ => "ERR_QUEST_FAILED_S",               // msgId 139 — the plain form
    }
}

mod net;

pub(crate) struct UiQuestPlugin;

impl Plugin for UiQuestPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<QuestGiver>().add_systems(
            Update,
            (
                // Range-close before the feed so the clear turns into the panel-close the same
                // frame; push before the input pass so an open/close is on screen the same frame;
                // drain after it so a click's intent goes out the same frame (mirrors
                // ui_gossip/merchant).
                close_npc_session_out_of_range::<QuestGiver>.before(feed_quest),
                feed_quest.in_set(UiFeed),
                drain_quest.after(UiInput),
            ),
        );
    }
}

/// The questgiver panel is an NPC session: the standardized range guard ([`crate::ui_session`])
/// client-side-closes it — the same no-packet clear as its close button — when the player walks out
/// of the giver's service range or the giver despawns. The per-guid `statuses` store survives, like
/// every other close.
impl NpcSession for QuestGiver {
    /// `None` when the open panel's giver is an **item** (decision 0664): a quest-starter item
    /// opens its detail panel with the ITEM's own guid as the giver, and an item is not a world
    /// unit — there is no range to walk out of and no portrait to bake. Reporting the item guid
    /// here would close the panel the frame it opened, since the range guard reads a guid it can't
    /// resolve to a world entity as "the giver despawned". The drain still addresses its
    /// `CMSG_QUESTGIVER_*` to `self.npc` (the item guid is exactly what the server wants back —
    /// vmangos resolves an `HIGHGUID_ITEM` giver through `TYPEMASK_CREATURE_GAMEOBJECT_OR_ITEM`);
    /// only the *session* face, whose whole meaning is "the live NPC this window is bound to",
    /// says there is none.
    fn npc(&self) -> Option<u64> {
        self.npc.filter(|g| !benilla_protocol::guid::is_item(*g))
    }

    /// Walking away from a party member's SHARED quest still owes them the decline
    /// (decision 1741). The distance that triggers it is not this method's business — the guard
    /// takes the leash from the giver's own type (`crate::ui_session::leash_sq`: 14.0 yd for a
    /// player, the 5.56 yd service range for an NPC), which is what 1733 got wrong by exempting
    /// the share panel outright.
    ///
    /// **The walk-away sends strictly LESS than the button does**, and that is the reference's,
    /// not a simplification: its watchdog calls the session end directly (`0x4933da` → `0x501130`)
    /// rather than going through `DeclineQuest`, so an NPC's panel closes silently here — no
    /// giver re-open — while the button's path still re-opens it.
    fn walk_away_send(&self, npc: u64) -> Option<ClientCommand> {
        benilla_protocol::guid::is_player(npc).then_some(ClientCommand::QuestPushResult {
            sharer: npc,
            msg: QuestShareMsg::DECLINE_QUEST,
        })
    }

    fn close(&mut self) {
        self.clear();
    }
}

/// The greeting panel's active-vs-available split — decision 0088's deferred item, now resolved by
/// the **wire icon**, which is the reference's own and only predicate.
///
/// The split is `0x5dbbfe-0x5dbc08`: `icon == 3 || icon == 4` → ACTIVE, and **every other `u32`** →
/// AVAILABLE. A flat two-way `cmp`/`je`, no range test and no third arm — and the client never
/// consults its own quest log on this path. The values are vmangos's `__QuestGiverStatus`
/// (`QuestDef.h:118-130`): 3 = `DIALOG_STATUS_INCOMPLETE` (held, unfinished), 4 =
/// `DIALOG_STATUS_REWARD_REP` (hand it in). The gossip packet's quest rows use the identical test,
/// applied lazily (`0x4e2430`/`0x4e2580`), so both seams share this helper.
///
/// **This reverses an earlier call of ours, and the reversal is the whole point.** benilla used to
/// read the icon, saw an auto-complete quest arrive carrying REWARD_REP while absent from the quest
/// log, read that as a misclassification, and switched to log membership. It was not a
/// misclassification: an auto-complete quest is *never* in the log — that is what auto-complete
/// means — and `Player::PrepareQuestMenu` (vmangos `Objects/Player.cpp:12501`) marks it REWARD_REP
/// deliberately, so the client sends COMPLETE_QUEST and gets the request-items panel. Deriving the
/// pool from the log instead made every such quest permanently un-turn-in-able and rendered its
/// (empty) detail text as a blank window — ledger B95, decision 0758.
pub(crate) fn row_is_active(icon: u32) -> bool {
    matches!(icon, 3 | 4)
}

/// An AVAILABLE row's **one-click** flag: `icon == 0` makes the *available* select send
/// COMPLETE_QUEST rather than QUERY_QUEST.
///
/// VERIFIED: `test ebx,ebx; sete cl` at `0x5dbc13` stores it into the pool record's `+0x48`, which
/// `SelectAvailableQuest` (`0x5012a0`) reads at `0x5012db` to pick opcode `0x18a` over `0x186`. (The
/// ACTIVE pool's `+0x48` is written literal `0` and read nowhere in the binary — `SelectActiveQuest`
/// always sends `0x18a`.)
///
/// Exactly one thing here is open: whether a real 1.12 server ever emitted `icon == 0`
/// on this packet at all — vmangos emits only 2..=5, and its `DIALOG_STATUS_NONE` belongs to the
/// separate `SMSG_QUESTGIVER_STATUS` path. The arm is byte-verified and correctly wired; only its
/// live *reachability* is unknown, and the client predicate is total over `u32`, so implementing it
/// as written is faithful either way.
pub(crate) fn row_is_one_click(icon: u32) -> bool {
    icon == 0
}

/// One greeting pool as the drain re-walks it: `(quest_id, wire icon)` per row. The icon rides along
/// because the AVAILABLE arm needs it a second time, for [`row_is_one_click`].
type Pool = Vec<(u32, u32)>;

/// Resolve one wire reward/required triple into a Lua-facing [`QuestItemView`]: icon from the wire
/// display id (immediate), name + quality from the ask-once item-template cache (`None`/white while
/// in flight — the row shows a placeholder and fills in, exactly like a bag slot / vendor row).
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
    // The ctrl/shift click arms' payload (`GetQuestItemLink`, decisions 1059/1060) — built through
    // THE link builder, never a hand-rolled format (`ui_items::item_link`'s own doc). `None` until
    // the template lands: the link needs both the name and the quality.
    let link = name
        .as_ref()
        .map(|n| crate::ui_items::item_link(it.item_id, n, quality));
    QuestItemView {
        name,
        texture,
        count: it.count,
        quality,
        item_id: it.item_id,
        usable: true, // v1: soft gray only, server authoritative (decision 0088)
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

/// Build the Lua-facing snapshot from the open view — `None` when no window is open. Every
/// server-authored text runs the shared chat-macro substitution (`$N`/`$B`/`$G` —
/// [`crate::npc_text`]): the wire delivers quest text un-expanded, the client substitutes.
/// The quest's reward spell as the getters answer it: `rewSpell` off the packet, resolved to the
/// spell's name and icon through the spell catalog (`Spell.dbc`). Zero — no spell — is `None`; an
/// id the catalog cannot place still counts (the id is real), with nothing to paint.
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
        // `isTradeskillSpell` is `Spell.dbc` col 6 `Attributes` bit 5 (`0x20`) — `0x501e59`
        // (1161 of 22357 rows set; craft-cast spells set, `Pattern:` teaching spells clear).
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
    let sub = |t: &str| crate::npc_text::substitute(t, macros);
    Some(match giver.view.as_ref()? {
        QuestView::Greeting(l) => {
            let mut active_titles = Vec::new();
            let mut available_titles = Vec::new();
            for q in &l.quests {
                if row_is_active(q.icon) {
                    active_titles.push(q.title.clone());
                } else {
                    available_titles.push(q.title.clone());
                }
            }
            QuestState {
                panel: QuestPanel::Greeting,
                greeting: sub(&l.greeting),
                active_titles,
                available_titles,
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

/// The FrameXML event a panel opens with (the ref `QuestFrame.lua` names).
fn panel_event(panel: QuestPanel) -> &'static str {
    match panel {
        QuestPanel::Greeting => "QUEST_GREETING",
        QuestPanel::Detail => "QUEST_DETAIL",
        QuestPanel::Progress => "QUEST_PROGRESS",
        QuestPanel::Reward => "QUEST_COMPLETE",
    }
}

/// Push the current quest view into the VM and fire the FrameXML events on a transition (panel
/// change → the panel's open event; same panel, content changed → `QUEST_ITEM_UPDATE`; closed →
/// `QUEST_FINISHED`). Diffed against a `Local`, exactly like the gossip/merchant feeds. The NPC
/// name rides as arg1 (resolved through the NameCache, ask-once — the merchant's pattern).
fn feed_quest(
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
    mut last: Local<crate::ui_script::VmMemo<Option<QuestState>>>,
    mut last_name: Local<crate::ui_script::VmMemo<Option<String>>>,
    mut last_npc: Local<crate::ui_script::VmMemo<Option<u64>>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let last = last.get(&script);
    let last_name = last_name.get(&script);
    let last_npc = last_npc.get(&script);
    // The turn-in fanfare (QUESTCOMPLETED → iQuestComplete.wav): the client's C++ plays it on the
    // QUEST_COMPLETE packet itself — no Lua handler owns it, so the feed queues it directly.
    if giver.take_completed_fanfare() {
        script.queue_sound_kit("QUESTCOMPLETED");
    }
    // The queued client messages (the refusal lines the net apply staged): resolve against the
    // VM's own GlobalStrings first (immutable script), then show each on the surface its message
    // record names — decision 0669's `DisplayError` kind. Drained BEFORE the snapshot's early-out
    // so a refusal that also closes the panel still gets its line out this frame.
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
    // The panel's background material is the SOURCE object's — an item's page material, a
    // GameObject's template data — never a creature's, and never on the wire (1946;
    // `GetQuestBackgroundMaterial 0x502230`). Resolved the way the reader window resolves its
    // own, since the two bindings share a body.
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
    // A different giver while a panel is already open is a real close+open (decision 0096 /
    // [`crate::ui_session::npc_switched`]); a cross-window switch is handled by OnHide → CloseX on
    // panel displacement (decision 0095).
    let switched = npc_switched(*last_npc, giver.npc);
    if fresh == *last && !name_changed && !switched {
        return;
    }
    script.set_quest(fresh.clone());
    // The reference's QUEST_GREETING/DETAIL/PROGRESS/COMPLETE carry no argument: the window
    // reads `UnitName("npc")` for its title (stock QuestFrame.lua:63), and the "npc" unit is
    // the session's (ui_unit.rs). Ours used to pass the name as arg1 (1944 dropped it).
    match (&*last, &fresh) {
        (_, Some(f)) if switched => {
            // A different giver → close the old panel, open the new (both kits play). QUEST_FINISHED
            // routes through OnHide → CloseQuest (decision 0095), which queues a `Close` action —
            // drain the pending actions so it does NOT clear the giver we just re-opened. Safe: a
            // switch is net-driven, so no user action is queued this frame to lose.
            script.fire_event("QUEST_FINISHED", vec![]);
            script.fire_event(panel_event(f.panel), vec![]);
            let _ = script.take_quest_actions();
        }
        (_, Some(f)) => {
            // Same panel + already open → an in-place content refresh (a name landed); a new panel
            // (or a fresh open) → the panel's open event.
            let same_panel = last.as_ref().is_some_and(|l| l.panel == f.panel);
            let event = if same_panel {
                "QUEST_ITEM_UPDATE"
            } else {
                panel_event(f.panel)
            };
            script.fire_event(event, vec![]);
        }
        (Some(_), None) => script.fire_event("QUEST_FINISHED", vec![]),
        (None, None) => {}
    }
    *last = fresh;
    *last_name = npc_name;
    *last_npc = giver.npc;
}

/// `CloseQuest()` — the teardown `0x501130(0,1)` its binding `0x501a10` calls and nothing else:
/// a PLAYER source — a party member whose shared quest we are walking away from — is answered
/// `MSG_QUEST_PUSH_RESULT{DECLINE_QUEST}`; every other source is closed **network-silently**.
/// There is no giver re-open here: that lives only in `DeclineQuest`'s fork ([`decline_leg`]).
/// 1738 put the two verbs on one routine and so re-opened an NPC's list on ESC, which is the
/// reason a multi-quest greeting could not be closed.
///
/// The PLAYER test is the guid's shape, as in [`QuestGiver::walk_away_send`]; the reference's
/// `0x468460(typemask 0x10)` also requires the sharer to resolve, which a sharer out of view does
/// not — a case the range guard closes on its own first.
fn close_quest(npc: u64, commands: &NetCommands) {
    if benilla_protocol::guid::is_player(npc) {
        debug!("ui_quest: closing {npc:#x}'s shared quest — declining it");
        let _ = commands.0.send(ClientCommand::QuestPushResult {
            sharer: npc,
            msg: QuestShareMsg::DECLINE_QUEST,
        });
    }
}

/// What `DeclineQuest()` does — the leg of its core `0x5013f0` a source takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeclineLeg {
    /// The already-acted latch is set (`0x5013fe`), or the source does not resolve (`0x50142c`):
    /// nothing at all — no send, no teardown.
    Nothing,
    /// The teardown `0x501130(0,1)` (`0x5014aa`): answers a PLAYER sharer, closes the window.
    Teardown,
    /// A gossip-flagged unit (`0x50143b`-`0x501474`): `CMSG_GOSSIP_HELLO`; the window stays.
    GossipHello,
    /// A plain unit (`0x5014e4`-`0x501526`): `CMSG_QUESTGIVER_HELLO`; the window stays.
    QuestgiverHello,
}

/// **`DeclineQuest`'s fork** — its binding `0x501d30` calls `0x5013f0`, the one routine that
/// re-opens a giver (`0x5013f0`-`0x501553`),
/// tested in this order:
///
/// 1. the already-acted latch `0xbe0844` set, or the source unresolvable → nothing;
/// 2. a UNIT whose `UNIT_NPC_FLAGS` carries GOSSIP → `CMSG_GOSSIP_HELLO` — **before** the
///    `0xbe0824` test, so a gossip NPC is re-opened whatever the latch says;
/// 3. an ITEM, a PLAYER, or `0xbe0824 != 0` → the teardown `0x501130(0,1)`, which answers a
///    sharer and is silent otherwise;
/// 4. a GAMEOBJECT → its interact virtual `[vtbl+0x60]` (`0x5014d0`);
/// 5. any other unit → `CMSG_QUESTGIVER_HELLO`, which re-opens the quest list.
///
/// Legs 2, 4 and 5 set `0xbe0844` and do NOT tear the window down: the server's answer replaces
/// the panel. **Leg 4 is modelled as the teardown**: the call site is the generic interact
/// virtual, but `CGGameObject_C`'s override of it is not identified, so what goes on the wire is
/// unknown; a silent close is the one outcome that cannot leave a dead window up.
///
/// `npc_flags` is the giver's live `UNIT_NPC_FLAGS`, `None` when it does not resolve to a streamed
/// object. An item giver (0664) is taken by its guid's shape: it lives in our bags, where the
/// reference always resolves it. A player is taken by shape too, as in [`close_quest`].
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

/// Drain the Lua intents: the greeting-row selects (map to the row's quest id → QUERY_QUEST for an
/// available quest, COMPLETE_QUEST for an active one) and the button actions (Accept/Continue/Reward
/// → the matching CMSG; Decline → [`decline_leg`]; Close → [`close_quest`]).
fn drain_quest(
    script: Option<NonSendMut<UiScript>>,
    mut giver: ResMut<QuestGiver>,
    commands: Res<NetCommands>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    let Some(mut script) = script else {
        return;
    };
    let Some(npc) = giver.npc else {
        // Still drain the VM so intents don't queue against a closed window.
        script.take_quest_selects();
        script.take_quest_actions();
        return;
    };
    // The giver's live `UNIT_NPC_FLAGS`, for [`decline_leg`]'s gossip/quest-list fork.
    // `None` for anything that is not a streamed unit — an item giver, a player, a despawn.
    let npc_flags = index
        .0
        .get(&npc)
        .and_then(|e| stores.get(*e).ok())
        .map(|s| s.0.unit_npc_flags());

    // Greeting-row selects: resolve the 1-based row to its quest id off the open greeting view.
    for sel in script.take_quest_selects() {
        let Some(QuestView::Greeting(list)) = giver.view.as_ref() else {
            continue;
        };
        // Re-walk the same split the snapshot used (`row_is_active`, off the wire icon), keeping the
        // id AND the icon this time — the available arm needs the icon again for its one-click flag.
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
        // `SelectActiveQuest` (`0x501320`) sends COMPLETE_QUEST unconditionally. `SelectAvailableQuest`
        // (`0x5012a0`) picks by the row's one-click flag: COMPLETE_QUEST when set, else QUERY_QUEST.
        let cmd = if sel.active || row_is_one_click(icon) {
            ClientCommand::QuestgiverComplete { npc, quest }
        } else {
            ClientCommand::QuestgiverQuery { npc, quest }
        };
        let _ = commands.0.send(cmd);
    }

    // Button actions, addressed to the open view's quest id.
    let view_quest = giver.view.as_ref().and_then(|v| match v {
        QuestView::Detail(d) => Some(d.quest_id),
        QuestView::Progress(p) => Some(p.quest_id),
        QuestView::Reward(o) => Some(o.quest_id),
        QuestView::Greeting(_) => None,
    });
    for action in script.take_quest_actions() {
        match action {
            QuestAction::Close => {
                close_quest(npc, &commands);
                giver.clear();
            }
            QuestAction::Decline => {
                let leg = decline_leg(npc, giver.acted, giver.close_on_cancel, npc_flags);
                debug!("ui_quest: decline on {npc:#x} — {leg:?}");
                match leg {
                    DeclineLeg::Nothing => {}
                    DeclineLeg::Teardown => {
                        close_quest(npc, &commands);
                        giver.clear();
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
                    // `AcceptQuest`'s tail (`0x5013df`): the Quest Log tutorial (1976).
                    if let Some(t) = tutorials.as_mut() {
                        t.write(crate::tutorial::TutorialEvent::trigger(
                            crate::tutorial::id::QUEST_LOG,
                        ));
                    }
                    // `Script::AcceptQuest` (`0x501380`) closes the window on the CLICK, not on any
                    // answer: send `0x189` (`0x5eac10`), then `0x501130(0,0)` — the same clear the
                    // refusal handlers call, firing `QUEST_FINISHED`. Ours used to leave the panel
                    // up waiting for a packet that never comes on a refused accept (decision 0669).
                    giver.clear();
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

    /// The greeting split is the reference's flat two-way test on the WIRE ICON — `{3,4}` ACTIVE,
    /// everything else AVAILABLE (`0x5dbbfe-0x5dbc08`, decision 0758). Pinned because this reverses
    /// an earlier call that read the same icon and concluded the opposite.
    #[test]
    fn greeting_split_reads_the_wire_icon() {
        // 3 = DIALOG_STATUS_INCOMPLETE, 4 = DIALOG_STATUS_REWARD_REP.
        assert!(row_is_active(3));
        assert!(row_is_active(4));
        // 5 = DIALOG_STATUS_AVAILABLE (a genuinely new quest), 2 = DIALOG_STATUS_CHAT.
        assert!(!row_is_active(5));
        assert!(!row_is_active(2));
        // No range test and no third arm: every other u32 is AVAILABLE, including 0, the ones
        // vmangos never emits, and anything a future server invents.
        for icon in [0, 1, 6, 7, 100, u32::MAX] {
            assert!(!row_is_active(icon), "icon {icon} must fall to AVAILABLE");
        }
    }

    /// **The B95 case.** An auto-complete quest (`QuestMethod == 0`) is never in the player's quest
    /// log, and vmangos marks its greeting row `DIALOG_STATUS_REWARD_REP` anyway
    /// (`Objects/Player.cpp:12501`) precisely so the client treats it as a turn-in. Binning that row
    /// off log membership sent `QUERY_QUEST`, which vmangos answers with DETAILS unconditionally —
    /// and quest 7793 "A Donation of Silk" has empty `Details`/`Objectives`, so the window came up
    /// blank. The row must be ACTIVE on the icon alone.
    #[test]
    fn an_autocomplete_row_is_active_though_it_is_not_in_the_log() {
        const REWARD_REP: u32 = 4;
        assert!(
            row_is_active(REWARD_REP),
            "an auto-complete turn-in must bin ACTIVE without ever entering the quest log"
        );
        assert!(!row_is_one_click(REWARD_REP), "it is active, not one-click");
    }

    /// The AVAILABLE pool's one-click flag: `icon == 0` alone (`test ebx,ebx; sete cl` @ `0x5dbc13`),
    /// which makes the *available* select send COMPLETE_QUEST instead of QUERY_QUEST.
    #[test]
    fn the_one_click_flag_is_icon_zero_alone() {
        assert!(row_is_one_click(0));
        for icon in [1, 2, 3, 4, 5, u32::MAX] {
            assert!(!row_is_one_click(icon));
        }
    }

    /// An item-sourced quest window (decision 0664) reports **no** NPC to the session face, so the
    /// range guard can't close it the frame it opens — while the drain's own `self.npc` keeps the
    /// item guid the `CMSG_QUESTGIVER_*` sends must carry.
    #[test]
    fn an_item_giver_is_not_an_npc_session() {
        let item = 0x4000_0000_0000_0BAD_u64; // HIGHGUID_ITEM
        let creature = 0xF130_0000_00C5_0001_u64; // HIGHGUID_UNIT
        let detail = |npc: u64| {
            QuestView::Detail(QuestDetails {
                npc,
                quest_id: 373,
                title: "The Unsent Letter".into(),
                details: String::new(),
                objectives: String::new(),
                auto_finish: 0,
                choices: Vec::new(),
                rewards: Vec::new(),
                money: 0,
                reward_spell: 0,
            })
        };
        let mut giver = QuestGiver::default();
        giver.open(item, detail(item));
        assert_eq!(NpcSession::npc(&giver), None, "an item is not a live NPC");
        assert_eq!(giver.npc, Some(item), "the wire address is unchanged");
        giver.open(creature, detail(creature));
        assert_eq!(NpcSession::npc(&giver), Some(creature));
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
        // The wire text's $N substituted through the shared expander (crate::npc_text).
        assert_eq!(snap.body, "Kill kobolds, Tri.");
        assert_eq!(snap.objectives, "Slay 10.");
        assert_eq!(snap.choices.len(), 1);
        assert_eq!(snap.rewards.len(), 1);
        assert_eq!(snap.reward_money, 1234);
        // Name in flight (no template answer) → nil; quality defaults to white.
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

    /// The `0x18f` table, arm for arm off `0x5dbca0` — including the two edges that are easy to
    /// get wrong: reason 0 and anything past the `cmp eax,0x15` bound fall to NEED_PREREQS (the
    /// ref's `ja` default), and 2..=5 do too even though they sit INSIDE the jump table's range.
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

    /// The `0x192` table off `0x5dc840`: the two named reasons plus the shared default.
    #[test]
    fn questgiver_failed_keys_match_the_reference_table() {
        assert_eq!(questgiver_failed_key(4), "ERR_QUEST_FAILED_BAG_FULL_S");
        assert_eq!(questgiver_failed_key(50), "ERR_QUEST_FAILED_BAG_FULL_S");
        assert_eq!(questgiver_failed_key(17), "ERR_QUEST_FAILED_MAX_COUNT_S");
        for reason in [0, 1, 5, 16, 18, 49, 51] {
            assert_eq!(questgiver_failed_key(reason), "ERR_QUEST_FAILED_S");
        }
    }

    /// The open panel is the `%s` fill for a refusal that names the quest — and only for ITS OWN
    /// quest id (a stale panel must not name the wrong quest).
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

    /// The RUNTIME leg on the real client data (`equip_error`'s pattern): every key either table
    /// can emit resolves to a non-empty string in the shipped 1.12 `GlobalStrings.lua`, the guard
    /// against a typo'd key degrading a real refusal to silence. Also pins the director's repro
    /// (reason 13 → "You are already on that quest") and the `%s` the FAILED family fills.
    /// Skips without client data.
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
        // The director's line, end to end.
        assert_eq!(
            ui_error_text(&UiError::key(questgiver_invalid_key(13)), &g).as_deref(),
            Some("You are already on that quest")
        );
        // …and the named one, filled.
        assert_eq!(
            ui_error_text(&UiError::s(questgiver_failed_key(4), "A Threat Within"), &g).as_deref(),
            Some("A Threat Within failed: Inventory is full.")
        );
        // The log-full line is the RED one and carries no fill.
        assert_eq!(
            ui_error_text(&UiError::key("ERR_QUEST_LOG_FULL"), &g).as_deref(),
            Some("Your quest log is full.")
        );
    }

    // ── The party share's one client-originated verdict (decision 1733) ──────────────────────────

    /// Run `lua` against a quest window open on `giver` with the `0xbe0824` latch at
    /// `close_on_cancel`, and return what the drain sent.
    /// `npc_flags` seats the giver as a streamed unit with those `UNIT_NPC_FLAGS`; `None` leaves it
    /// unstreamed (an item giver, or an NPC that left view).
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

    /// **Ending the session on a SHARED quest answers the sharer — and `CloseQuest` does it too.**
    /// 1733 shipped these as two actions on the assumption that only `DeclineQuest` answered; both
    /// Lua verbs reach the teardown `0x501130(0,1)` (`CloseQuest` directly, `DeclineQuest` through
    /// its PLAYER leg), so ESC-ing a share panel reports the decline exactly as the button does. This test is the corrected form of 1733's
    /// `closing_a_shared_quest_panel_is_not_a_decline`, which asserted the opposite.
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

    /// **`DeclineQuest` on an NPC RE-OPENS the giver**, which benilla asserted for years that it did
    /// not ("vanilla's client-side decline sends no packet"). The fork is on the NPC's own gossip flag:
    /// `CMSG_GOSSIP_HELLO` for a gossip-flagged NPC, `CMSG_QUESTGIVER_HELLO` otherwise. This is the
    /// mechanism behind the reference putting you back in the questgiver's menu after a decline.
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

    /// The `0xbe0824` latch **suppresses** a plain unit's re-open when non-zero — the field
    /// benilla parsed and ignored until its one reader (`0x5014a2`) was found. Ignoring it meant a
    /// suppressed giver was re-opened anyway.
    #[test]
    fn a_non_zero_latch_suppresses_a_plain_units_reopen() {
        let sent = drain_after(NPC, Some(0), 1, "DeclineQuest()");
        assert!(sent.is_empty(), "flag 1 suppresses the re-open: {sent:?}");
    }

    /// A giver that does not resolve sends nothing at all. `CloseQuest` still tears the window
    /// down; `DeclineQuest` bails before its fork (`0x50142c`) and leaves it — the range guard
    /// closes a window whose giver has gone.
    #[test]
    fn an_unstreamed_giver_ends_the_session_silently() {
        let (app, sent) = drain_world(NPC, None, 0, "CloseQuest()");
        assert!(sent.is_empty(), "no unit, no re-open: {sent:?}");
        assert!(!app.world().resource::<QuestGiver>().is_open());

        let (app, sent) = drain_world(NPC, None, 0, "DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert!(app.world().resource::<QuestGiver>().is_open());
    }

    /// An item giver (0664) takes the teardown leg: silent, and the window closes.
    #[test]
    fn declining_an_item_quest_closes_silently() {
        const ITEM: u64 = 0x4000_0000_0000_0099; // HIGHGUID_ITEM
        let (app, sent) = drain_world(ITEM, None, 0, "DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert!(!app.world().resource::<QuestGiver>().is_open());
    }

    /// **`CloseQuest()` is network-silent on an NPC** — ESC, the X, the greeting's Goodbye
    /// (`HideUIPanel` → `QuestFrame_OnHide` → `CloseQuest`). Its binding `0x501a10` calls the
    /// teardown `0x501130(0,1)` and nothing else, whose one send is the PLAYER-filtered push
    /// result. It used to share `DeclineQuest`'s action and so re-open the list the player was
    /// trying to close (a multi-quest NPC's greeting could not be dismissed).
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

    /// `DeclineQuest`'s gossip leg (`0x50143b`-`0x501474`) is tested BEFORE the `0xbe0824` latch
    /// (`0x5014a2`), so a gossip-flagged NPC is re-opened whatever the latch says.
    #[test]
    fn the_gossip_leg_precedes_the_latch() {
        use crate::target::cursor_mode::npc_flags;
        let sent = drain_after(NPC, Some(npc_flags::GOSSIP), 1, "DeclineQuest()");
        assert!(
            matches!(sent.as_slice(), [ClientCommand::GossipHello { guid: NPC }]),
            "a latched gossip NPC still gets GOSSIP_HELLO: {sent:?}"
        );
    }

    /// The re-open legs do NOT tear the window down (no `0x501130` on them) — the server's answer
    /// replaces the panel — and they set the already-acted latch `0xbe0844`, which refuses a
    /// second `DeclineQuest` and an `AcceptQuest` until the next panel arrives.
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

    /// The latch-diverted leg (`0x5014aa` → `0x501130(0,1)`) DOES tear the window down.
    #[test]
    fn a_latched_decline_closes_the_window() {
        let (app, sent) = drain_world(NPC, Some(0), 1, "DeclineQuest()");
        assert!(sent.is_empty(), "{sent:?}");
        assert!(!app.world().resource::<QuestGiver>().is_open());
    }
}
