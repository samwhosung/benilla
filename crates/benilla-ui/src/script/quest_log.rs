//! The quest-**log** bindings (the 0088 arc's second slice) — the Era-shaped `GetQuestLog*` surface
//! the reference `QuestLogFrame.lua` reads, over the same two-way seam as the questgiver panels
//! ([`super::quest`]): the app pushes a whole [`QuestLogState`] snapshot (entries from the
//! `PLAYER_QUEST_LOG` descriptor slots, the selected quest's detail resolved from the
//! `SMSG_QUEST_QUERY_RESPONSE` template cache), and the Lua drains back the **abandon** intent.
//!
//! Two engine-owned bits deliberately live *in the model*, not the pushed state:
//!
//! - **The selection** (`SelectQuestLogEntry`/`GetQuestLogSelection`). In the reference client the
//!   selection is native and *synchronous* — `QuestLogTitleButton_OnClick` calls
//!   `SelectQuestLogEntry(i)` then immediately re-reads `GetQuestLogSelection()` in the same click
//!   (ref `QuestLogFrame.lua:318,308-346`); a drain-next-frame intent would read stale. The app
//!   reads it back each frame ([`super::UiScript::quest_log_selection`]) only to RE-POINT it across
//!   a rebuild. The **detail that selection names is resolved at call time**, off the row's own
//!   [`QuestLogEntryView::detail`] — never from a selection baked into the push.
//!   The reference's detail bindings peek its quest cache inside the same call, so
//!   `SelectQuestLogEntry(i)` immediately changes what `GetQuestLogQuestText()` answers; resolving
//!   one detail per push instead made a whole log walk answer with a single row's text.
//! - **The abandon mark** (`SetAbandonQuest`/`GetAbandonQuestName`/`AbandonQuest`) — the ref's
//!   two-step confirm (mark on button click, act on the popup's Yes — ref `QuestLogFrame.xml:463-472`,
//!   `StaticPopup.lua:749-761`). The mark is the selected **quest id** at click time, as the
//!   reference's is (`0x4dfb50` copies the selection `0xbb7480`, which holds a quest id, into
//!   `0xbb7484`), so a log that re-indexes between click and confirm — a fold, a quest arriving
//!   or leaving — can't retarget the abandon.
//!
//! A third engine-owned bit joined later: the **watch set** (`IsQuestWatched`/`AddQuestWatch`/
//! `RemoveQuestWatch`/`GetNumQuestWatches`/`GetQuestIndexForWatch`) — keyed by the entries' stable
//! [`QuestLogEntryView::quest_id`] under the index-based Era API, pruned on push, cap 5.
//!
//! A fourth engine-owned bit is the **countdown** (`GetQuestTimers`/`GetQuestIndexForTimer`/
//! `GetQuestLogTimeLeft`): the snapshot carries each row's absolute deadline and the
//! bindings subtract the live server clock per call, so the reference's `QuestTimerFrame` can tick
//! from its OnUpdate without the log snapshot changing under it. [`seconds_left`] carries the
//! reference's byte-verified formula and the ways a row has no timer to show.
//!
//! A fifth is the **share** (`GetQuestLogPushable`/`QuestLogPushQuest`): the
//! pushable bit rides each entry (from the app's template cache) and the click resolves the
//! selection to a quest id before it queues, so a log shuffle cannot retarget it.
//!
//! v1 scope (the deliberate `nil`/false stubs, each a later slice): the party quest-log tooltip
//! (`IsUnitOnQuest` — it needs the party members' own logs, which is a different wire),
//! reward spells (`GetQuestLogRewardSpell`), and zone
//! **headers** ([`QuestLogEntryView::is_header`] is plumbed but the app pushes a flat list —
//! headers need the QuestSort/AreaTable DBC join).

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, number_arg};
use super::quest::QuestItemView;
use super::Model;

/// One quest-log list row — the `GetQuestLogTitle` tuple's data half. Plain data; its 1-based
/// position in [`QuestLogState::entries`] is the quest-log index the whole API keys on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestLogEntryView {
    /// The quest id occupying the row's descriptor slot. Never surfaced to Lua (no Era API returns
    /// it) — it is the engine's *stable identity* for the watch set ([`install`]'s
    /// `AddQuestWatch`/…): watching by id survives the log shuffling under the index-based API
    /// (an abandon compacts the rows; a watched *index* would silently retarget).
    pub quest_id: u32,
    /// The quest title (row label).
    pub title: String,
    /// The quest's display level (`GetQuestLogTitle` return 2; the ref colors the row by it).
    pub level: u32,
    /// The quest tag suffix — the bare word, no parentheses: `Elite`, `Dungeon`, `Raid`, `PvP`,
    /// `Life`, `World Event`, `Legendary`. `None` for a plain quest, which is most of them, and
    /// always `None` on a header row. The app resolves it from the cached
    /// `SMSG_QUEST_QUERY_RESPONSE` template's `Type` through `QuestInfo.dbc`
    /// ([`benilla_formats::QuestTagNames`]); `Type` 0 names no row and takes no tag.
    ///
    /// `None`, never `Some("")` — the reference's row Lua branches on the tag's PRESENCE
    /// (`if ( questTag )`, ref `QuestLogFrame.lua:194`) to decide whether to shrink the title and
    /// reseat the watch check, so an empty string would take the wrong branch.
    pub tag: Option<String>,
    /// A zone header row (the app synthesizes these from each quest's ZoneOrSort).
    pub is_header: bool,
    /// Whether this quest may be SHARED with the party — `GetQuestLogPushable`'s answer for the
    /// row. Computed app-side from the cached `SMSG_QUEST_QUERY_RESPONSE`
    /// template's `QUEST_FLAGS_SHARABLE` (`0x8`), so a row whose template has not answered yet is
    /// `false` and turns true when it lands — the reference's own shape, since it reads the same
    /// cache. Never surfaced as a per-index Lua getter: the Era API asks only about the current
    /// selection.
    pub pushable: bool,
    /// A COLLAPSED header (`GetQuestLogTitle`'s isCollapsed; meaningless on quest rows). The app
    /// owns the collapse set and omits a collapsed header's quests from `entries` — the engine
    /// only reports the flag and drains the toggle intents ([`super::UiScript::take_quest_log_collapses`]).
    pub collapsed: bool,
    /// Whole-quest state from the descriptor slot's state byte: `1` complete, `-1` failed, `0`
    /// in progress (`GetQuestLogTitle`'s `isComplete` is `1`/`-1`/`nil` off this).
    pub complete: i32,
    /// A timed quest's **deadline**, as the descriptor slot carries it: absolute unix-epoch
    /// seconds (`time(nullptr) + limitTime`, vmangos `Player::AddQuest`), `0` when the quest is
    /// untimed. Deliberately the raw stamp and not a remaining-seconds count: a remaining count
    /// would change every second, and this snapshot is diffed by the app to decide whether to fire
    /// `QUEST_LOG_UPDATE` — a live number in here would rebuild the whole quest log every frame.
    /// The countdown is subtracted per call instead, against [`super::Model::server_unix_time`].
    pub timer: u32,
    /// The formatted objective lines for THIS entry (`GetNumQuestLeaderBoards(i)` /
    /// `GetQuestLogLeaderBoard(j, i)` serve any index off these — the watch tracker HUD reads
    /// non-selected quests' lines). The selection's no-arg reads come from here too; the
    /// [`QuestLogDetail`] carries only what exists for the selection alone
    /// (description/rewards/money).
    pub objectives: Vec<QuestLogObjectiveView>,
    /// This row's detail pane — description, money, rewards, reward spell. **Per row, not per
    /// selection**: the reference's detail bindings read `ds:0xbb7480` (what
    /// `SelectQuestLogEntry 0x4dfae0` wrote, synchronously) and then PEEK the quest cache in the
    /// same call (`0x4e1130` -> `0xc0e1b0`/`0x562a40`), so a
    /// select-then-read pair inside ONE frame answers about the row just selected. Carrying one
    /// detail for "the selection" instead made `SelectQuestLogEntry` inert until the next push:
    /// every entry of an addon's log walk answered with whichever row the snapshot happened to be
    /// built against. `None` on a header row.
    pub detail: Option<QuestLogDetail>,
}

/// One objective ("leaderboard") line of the selected quest — the `GetQuestLogLeaderBoard` tuple.
/// The app pre-formats `text` ("Kobold Vermin slain: 3/10") from the template + slot counters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestLogObjectiveView {
    /// The display line, fully formatted.
    pub text: String,
    /// The objective kind (`"monster"` / `"item"` / `"object"` — the Era type string).
    pub kind: String,
    /// Whether this objective is done (darkens the line + `(Complete)` suffix in the ref).
    pub finished: bool,
    /// The progress numbers `text` was formatted from — the `%d/%d` of the line (`cur` already
    /// clamped to `req`). Not exposed to Lua (`GetQuestLogLeaderBoard` returns text/type/finished
    /// only); they exist so the app's progress announce can ask *"did this ADVANCE?"* instead of
    /// *"did the rendered line change?"*. A text-only diff cannot tell progress from a regression,
    /// and a quest turn-in produces exactly that regression — the required items are destroyed a
    /// frame or two before the log slot clears, so the line dips to `0/req` while the quest is
    /// still in the log.
    pub cur: u32,
    /// See [`Self::cur`].
    pub req: u32,
}

/// The selected quest's detail pane — resolved by the app from the `QUEST_QUERY_RESPONSE` template
/// cache (+ item/creature name caches). `None` while the template is still in flight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestLogDetail {
    /// The long description (`GetQuestLogQuestText` return 1).
    pub description: String,
    /// The objectives flavor paragraph (`GetQuestLogQuestText` return 2) — *not* the leaderboard
    /// lines (those live on every [`QuestLogEntryView::objectives`], selection or not).
    pub objectives_text: String,
    /// Money the turn-in demands (`GetQuestLogRequiredMoney`), copper.
    pub required_money: u32,
    /// Reward money (`GetQuestLogRewardMoney`), copper.
    pub reward_money: u32,
    /// Choice rewards (`GetNumQuestLogChoices`/`GetQuestLogChoiceInfo`).
    pub choices: Vec<QuestItemView>,
    /// Fixed rewards (`GetNumQuestLogRewards`/`GetQuestLogRewardInfo`).
    pub rewards: Vec<QuestLogQuestItem>,
    /// The selected quest's reward spell (`rewSpell` on `SMSG_QUEST_QUERY_RESPONSE`), as
    /// `GetQuestLogRewardSpell` answers it.
    pub reward_spell: Option<super::quest::QuestRewardSpell>,
}

/// A quest-log reward row is the same shape as a questgiver panel row.
pub type QuestLogQuestItem = QuestItemView;

/// The whole quest-log snapshot the app pushes each time it changes. Unlike the questgiver panels
/// this is durable state, not a window session — an empty log is `entries: []`, never `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestLogState {
    /// The list rows, in quest-log (descriptor slot) order. 1-based indexing on the Lua side.
    /// A collapsed header's quests are OMITTED here (the app filters) — indexes are visible rows;
    /// their ids ride [`Self::hidden_quest_ids`].
    pub entries: Vec<QuestLogEntryView>,
    /// The total quest count INCLUDING quests hidden under collapsed headers — the "Quests: N/20"
    /// pill must not shrink when a header collapses (`GetNumQuestLogEntries` return 2).
    pub num_quests: u32,
    /// The ids of the quests folded under a collapsed header — in the log, absent from
    /// [`Self::entries`]. No getter indexes them; they exist for the **watch prune** alone, which
    /// in the reference scans the whole row array, hidden rows included (`0x4de7a7`–`0x4de80f`;
    /// a collapsed-group quest is still a row there, sorted past the visible window). Without them
    /// a collapse would read as the
    /// quest leaving the log and silently drop its watch.
    pub hidden_quest_ids: Vec<u32>,
}

impl super::UiScript {
    /// Push the quest-log snapshot (the app calls this whenever slots/templates/selection change).
    /// Also prunes the watch set: a watched quest that left the log (abandon/turn-in) drops its
    /// watch — the rebuild's prune `0x4de7a7`–`0x4de80f`, which keeps a watch while ANY non-header
    /// row carries its id, visible or [hidden](QuestLogState::hidden_quest_ids). A collapse is
    /// not a removal: the reference's `0x4ded30` re-sorts and recounts and never prunes.
    pub fn set_quest_log(&mut self, state: QuestLogState) {
        let mut model = self.model_mut();
        model.quest_log_watched.retain(|id| {
            state
                .entries
                .iter()
                .any(|e| !e.is_header && e.quest_id == *id)
                || state.hidden_quest_ids.contains(id)
        });
        model.quest_log = state;
    }

    /// The watched quest ids, in watch order — the app fires `QUEST_WATCH_UPDATE` when a watched
    /// quest's objectives change, and this is how it knows which those are.
    pub fn quest_log_watched(&self) -> Vec<u32> {
        self.model_ref().quest_log_watched.clone()
    }

    /// The engine-owned 1-based selection (`SelectQuestLogEntry`'s last value; `0` = none) — the
    /// app reads it each frame to know which quest's detail to resolve and push.
    pub fn quest_log_selection(&self) -> u32 {
        self.model_ref().quest_log_selection
    }

    /// Drain the abandon intents: the QUEST ID `SetAbandonQuest` marked at click time, confirmed
    /// by the popup's `AbandonQuest()`. The app maps id → descriptor slot →
    /// `CMSG_QUESTLOG_REMOVE_QUEST`.
    pub fn take_quest_log_abandons(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().quest_log_abandons)
    }

    /// Drain the share intents: the quest ids `QuestLogPushQuest()` queued, already resolved from
    /// the selection at click time. The app sends one `CMSG_PUSHQUESTTOPARTY` per id.
    pub fn take_quest_log_pushes(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().quest_log_pushes)
    }

    /// Drain the escort-confirm answers: how many times `ConfirmAcceptQuest()` was called. A
    /// count, because the verb carries no quest id — the app answers the confirm
    /// it is holding.
    pub fn take_quest_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().quest_confirms)
    }

    /// Drain the header collapse/expand intents: `(1-based entry index, collapse)` from
    /// `CollapseQuestHeader`/`ExpandQuestHeader` — index `0` = ALL headers (the ref's
    /// collapse-all button passes 0, QuestLogFrame.lua:557/:561). The app owns the collapse
    /// set and re-feeds the filtered list (same intent pattern as the abandons above).
    pub fn take_quest_log_collapses(&mut self) -> Vec<(u32, bool)> {
        std::mem::take(&mut self.model_mut().quest_log_collapses)
    }

    /// Set the engine-owned 1-based selection from the app side — the collapse/expand rebuild
    /// shifts entry indexes, and the app re-points the selection at the SAME quest's new index
    /// (selection is an index to the Era API, but an identity to the player).
    pub fn set_quest_log_selection(&mut self, i: u32) {
        self.model_mut().quest_log_selection = i;
    }

    /// Push the server's wall clock (unix-epoch seconds) — the app's `SMSG_QUERY_TIME_RESPONSE`
    /// sample, advanced monotonically. Every countdown the engine answers is subtracted against
    /// this, so it is pushed each frame rather than on change.
    ///
    /// This is the reference's `G` in a different frame of reference, and it is worth naming the
    /// equivalence because the two look nothing alike. The client stores only an **offset**
    /// `G = local_epoch@sync − server_epoch@sync` and computes `deadline + G − localNow()`; we
    /// store the server stamp and advance it monotonically, i.e. `deadline − serverNow()`.
    /// Expand either and both reduce to `remaining_at_sync − elapsed`. The one behavioural
    /// difference is deliberate and in our favour: a local wall-clock step mid-session (an NTP
    /// correction, a manual change) jumps the reference's countdown and cannot move ours.
    pub fn set_server_unix_time(&mut self, unix_secs: f64) {
        self.model_mut().server_unix_time = Some(unix_secs);
    }
}

/// The signed seconds remaining on a quest-log row's timer, or `None` when the row has no timer at
/// all. **This is the reference's own formula**, byte-verified:
///
/// ```text
/// value = slot.timer + G − now − 1
/// ```
///
/// — computed identically by all four of its consumers (`0x4e0905`, `0x4e14b9`, `0x4e16c9`,
/// `0x4de6b6`). Our `now` already carries `G` folded in (see [`super::UiScript::set_server_unix_time`]),
/// so what is left here is the deadline, the clock, and **the `−1`**.
///
/// That `−1` is not a rounding artefact to tidy away: a quest expiring exactly `N` seconds after
/// the sampled clock reads `N − 1`, and its last second reads `0`. The bias is downward by
/// construction — the reference never overstates the time a player has left, and neither do we.
///
/// Three ways there is nothing to show, each a deliberate answer rather than a fallthrough:
///
/// - `timer == 0` — the quest is untimed. The overwhelming majority of rows, and header rows too
///   (the app pushes them with no timer, matching the reference's own header skip at `0x4e1460`).
/// - The row is **failed** (`complete < 0`). vmangos writes the slot timer to the literal `1` and
///   sets the state's FAIL bit when a timed quest runs out (`Player::FailQuest`,
///   `Player.cpp:13430`), and the row *stays in the log* until the player abandons it. The
///   reference tests the same bit **before** the arithmetic (`test byte [slot+0x7],0x2` at each of
///   the four sites), so that `1` never reaches a subtraction there either.
/// - No server clock yet — the deadline is an absolute stamp in the server's epoch and there is
///   nothing honest to subtract from it. The reference agrees exactly: its `G == 0` is the
///   never-synced sentinel and `GetQuestLogTimeLeft` answers nil on it.
///
/// A **negative** result is returned as such: the two list bindings drop the row (the reference's
/// `js` at `0x4e14c5`), while `GetQuestLogTimeLeft` clamps it at 0. Same split, same reason.
fn seconds_left(entry: &QuestLogEntryView, now: Option<f64>) -> Option<i64> {
    if entry.timer == 0 || entry.complete < 0 {
        return None;
    }
    let now = now?;
    Some((f64::from(entry.timer) - now - 1.0).floor() as i64)
}

/// The list bindings' view: seconds remaining, and only for a row that still has some. Drops the
/// expired rows the reference's `js` drops.
fn live_seconds_left(entry: &QuestLogEntryView, now: Option<f64>) -> Option<i64> {
    seconds_left(entry, now).filter(|&s| s >= 0)
}

/// Register the quest-log globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // GetNumQuestLogEntries() → numEntries, numQuests (headers count toward the first only —
    // ref QuestLogFrame.lua:108; flat v1 pushes no headers so the two are equal).
    g.set(
        "GetNumQuestLogEntries",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let entries = &model.quest_log.entries;
            Ok((entries.len() as i64, i64::from(model.quest_log.num_quests)))
        })?,
    )?;

    // GetQuestLogTitle(i) → title, level, tag, isHeader, isCollapsed, isComplete
    // (the load-bearing 6-tuple — ref QuestLogFrame.lua:144/:272/:321/:571).
    // `tag` is the bare word (`Elite`, `Raid`, …) or nil — the ref's Lua wraps it in the
    // parentheses itself (`"("..questTag..")"`, ref l.195). isComplete is 1 / -1 / nil.
    //
    // **The arity is always SIX, and the two failure shapes are different** (`0x4df930`): a
    // MISSING or non-number
    // argument raises `Usage:` (shape A — [`number_arg`], truncating toward zero like the
    // reference's `_ftol`), while an out-of-range NUMBER is not an error at all — it returns
    // `nil, 0, nil, nil, nil, nil` off `mov eax,6` at all three exits. Return 2 is the NUMBER
    // `0` there, never nil, and it is `0` for a header row too. We used to answer a single
    // `Nil` for both, which reads the same through the reference's own
    // `local t, l, … = GetQuestLogTitle(i)` (Lua pads with nil) but hands an addon doing
    // arithmetic on the level a nil where the client gives it a zero.
    g.set(
        "GetQuestLogTitle",
        lua.create_function(|lua, i: Value| {
            let n = number_arg(lua, i, "Usage: GetQuestLogTitle(index)")?;
            let entry = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                usize::try_from(n)
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|n| model.quest_log.entries.get(n))
                    .cloned()
            };
            let Some(e) = entry else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Integer(0),
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            let tag = match &e.tag {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            let complete = match e.complete {
                0 => Value::Nil,
                c => Value::Integer(i64::from(c.signum())),
            };
            // **Returns 4 and 5 are `1`/`nil`, NOT booleans**: `isHeader` is `1` on a header row
            // and `nil` on a quest; `isCollapsed` is `1` only for a header whose bit in
            // `[0xbb748c]` is clear — a quest row and an EXPANDED header both answer `nil`. We
            // pushed `true`/`false`, which every `if ( isHeader )` in FrameXML reads the same and
            // which `isHeader == false` or a `type()` test does not. The two are one expression
            // here because `collapsed` is meaningless off a header (the field's own doc).
            let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&e.title)?),
                Value::Integer(i64::from(e.level)),
                tag,
                flag(e.is_header),
                flag(e.is_header && e.collapsed),
                complete,
            ]))
        })?,
    )?;

    // SelectQuestLogEntry(i) — the synchronous, engine-owned selection (see module doc).
    g.set(
        "SelectQuestLogEntry",
        lua.create_function(|lua, i: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.quest_log_selection = i;
            Ok(())
        })?,
    )?;

    // GetQuestLogSelection() → the 1-based selection, 0 = none.
    g.set(
        "GetQuestLogSelection",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.quest_log_selection))
        })?,
    )?;

    // GetQuestLogQuestText() → description, objectivesText (for the selection — the app already
    // resolved `detail` against it).
    g.set(
        "GetQuestLogQuestText",
        lua.create_function(|lua, ()| {
            let (desc, obj) = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .selected_quest_detail()
                    .map(|d| (d.description.clone(), d.objectives_text.clone()))
                    .unwrap_or_default()
            };
            Ok((
                Value::String(lua.create_string(&desc)?),
                Value::String(lua.create_string(&obj)?),
            ))
        })?,
    )?;

    // GetNumQuestLeaderBoards([questIndex]) — objective-line count for the given entry (1-based),
    // defaulting to the selection. Any index answers (the watch tracker HUD reads non-selected
    // quests' lines — ref QuestLogFrame.lua:613-663).
    g.set(
        "GetNumQuestLeaderBoards",
        lua.create_function(|lua, i: Option<u32>| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let index = i.unwrap_or(model.quest_log_selection) as usize;
            Ok(index
                .checked_sub(1)
                .and_then(|n| model.quest_log.entries.get(n))
                .map(|e| e.objectives.len() as i64)
                .unwrap_or(0))
        })?,
    )?;

    // GetQuestLogLeaderBoard(i[, questIndex]) → text, type, finished — same any-index rule.
    g.set(
        "GetQuestLogLeaderBoard",
        lua.create_function(|lua, (i, quest): (usize, Option<u32>)| {
            let line = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let index = quest.unwrap_or(model.quest_log_selection) as usize;
                index
                    .checked_sub(1)
                    .and_then(|n| model.quest_log.entries.get(n))
                    .and_then(|e| i.checked_sub(1).and_then(|n| e.objectives.get(n)))
                    .cloned()
            };
            let Some(l) = line else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&l.text)?),
                Value::String(lua.create_string(&l.kind)?),
                Value::Boolean(l.finished),
            ]))
        })?,
    )?;

    // ── Detail counts + money ─────────────────────────────────────────────────────────────────────
    fn install_detail_count(
        lua: &Lua,
        name: &str,
        pick: fn(&super::quest_log::QuestLogDetail) -> i64,
    ) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, ()| {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                Ok(model.selected_quest_detail().map(pick).unwrap_or(0))
            })?,
        )
    }
    install_detail_count(lua, "GetNumQuestLogRewards", |d| d.rewards.len() as i64)?;
    install_detail_count(lua, "GetNumQuestLogChoices", |d| d.choices.len() as i64)?;
    install_detail_count(lua, "GetQuestLogRewardMoney", |d| i64::from(d.reward_money))?;
    install_detail_count(lua, "GetQuestLogRequiredMoney", |d| {
        i64::from(d.required_money)
    })?;

    // GetQuestLogChoiceInfo(i) / GetQuestLogRewardInfo(i) → name, texture, numItems, quality,
    // isUsable — the same 5-tuple as the giver panels' GetQuestItemInfo (they share one layout
    // routine in the ref — QuestFrame.lua:311-522).
    fn install_detail_item(
        lua: &Lua,
        name: &str,
        pick: fn(&super::quest_log::QuestLogDetail) -> &Vec<QuestItemView>,
    ) -> mlua::Result<()> {
        lua.globals().set(
            name,
            lua.create_function(move |lua, i: usize| {
                let item = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    model
                        .selected_quest_detail()
                        .and_then(|d| i.checked_sub(1).and_then(|n| pick(d).get(n)).cloned())
                };
                let Some(it) = item else {
                    return Ok(MultiValue::from_vec(vec![Value::Nil]));
                };
                let name_v = match &it.name {
                    Some(n) => Value::String(lua.create_string(n)?),
                    None => Value::Nil,
                };
                let texture = match &it.texture {
                    Some(t) => Value::String(lua.create_string(t)?),
                    None => Value::Nil,
                };
                Ok(MultiValue::from_vec(vec![
                    name_v,
                    texture,
                    Value::Integer(i64::from(it.count)),
                    Value::Integer(i64::from(it.quality)),
                    Value::Boolean(it.usable),
                    // Benilla extension (6th): the item id, the shared tooltip store's key.
                    Value::Integer(i64::from(it.item_id)),
                ]))
            })?,
        )
    }
    install_detail_item(lua, "GetQuestLogChoiceInfo", |d| &d.choices)?;
    install_detail_item(lua, "GetQuestLogRewardInfo", |d| &d.rewards)?;

    // GetQuestLogItemLink(type, index) → the full escaped `|cff…|Hitem:…|h[Name]|h|r` | nil — the
    // quest log's twin of `GetQuestItemLink` ([`super::quest`]), read by the reward rows' ctrl/shift
    // click arms: `DressUpItemLink(GetQuestLogItemLink(this.type, this:GetID()))` (ref
    // QuestLogFrame.lua:545) and the `ChatFrameEditBox:Insert(...)` beside it (l.549). Unlike the
    // *Info* pair above this one is kind-DISPATCHED, because that is the shape the reference's own
    // handler calls it with (`this.type` is "choice"/"reward", set by the shared
    // `QuestFrameItems_Update`); an unknown type, an out-of-range index, or a row whose template
    // answer is still in flight all read nil.
    g.set(
        "GetQuestLogItemLink",
        lua.create_function(|lua, (kind, index): (String, usize)| {
            let link = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model.selected_quest_detail().and_then(|d| {
                    let v = match kind.as_str() {
                        "choice" => &d.choices,
                        "reward" => &d.rewards,
                        _ => return None,
                    };
                    index
                        .checked_sub(1)
                        .and_then(|n| v.get(n))
                        .and_then(|it| it.link.clone())
                })
            };
            match link {
                Some(l) => Ok(Value::String(lua.create_string(&l)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // IsCurrentQuestFailed() → 1/nil: the selection's slot state is FAIL (ref appends
    // " - (Failed)").
    g.set(
        "IsCurrentQuestFailed",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let sel = model.quest_log_selection as usize;
            Ok(flag(
                sel.checked_sub(1)
                    .and_then(|n| model.quest_log.entries.get(n))
                    .is_some_and(|e| e.complete < 0),
            ))
        })?,
    )?;

    // ── The abandon two-step (mark → confirm) ─────────────────────────────────────────────────────
    // CollapseQuestHeader(i)/ExpandQuestHeader(i) — header fold intents, drained by the app
    // (which owns the collapse set and re-feeds the filtered list). Index 0 = ALL headers (the
    // ref's collapse-all button, QuestLogFrame.lua:557/:561). Same intent pattern as the abandons.
    g.set(
        "CollapseQuestHeader",
        lua.create_function(|lua, i: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.quest_log_collapses.push((i, true));
            Ok(())
        })?,
    )?;
    g.set(
        "ExpandQuestHeader",
        lua.create_function(|lua, i: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.quest_log_collapses.push((i, false));
            Ok(())
        })?,
    )?;

    // SetAbandonQuest() — mark the selected QUEST as the abandon target (ref
    // QuestLogFrame.xml:464). The reference's `0x4dfb50` is `mov eax,[0xbb7480]; mov [0xbb7484],eax`:
    // it copies the selection, and the selection is a quest id (`0x4def30` stores the row's +0x0
    // at `0x4def5d`). Ours holds the selection as a row index, so the id is resolved here, at the
    // click — never at the confirm, when the rows may have moved.
    g.set(
        "SetAbandonQuest",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let sel = model.quest_log_selection as usize;
            model.quest_log_abandon_mark = sel
                .checked_sub(1)
                .and_then(|n| model.quest_log.entries.get(n))
                .filter(|e| !e.is_header)
                .map_or(0, |e| e.quest_id);
            Ok(())
        })?,
    )?;
    // GetAbandonQuestName() → the marked quest's title ("" when none — the popup shows it). The
    // reference (`0x4dfb60`) peeks its quest cache by the marked id; the rows carry the same
    // cached title.
    g.set(
        "GetAbandonQuestName",
        lua.create_function(|lua, ()| {
            let title = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                let mark = model.quest_log_abandon_mark;
                model
                    .quest_log
                    .entries
                    .iter()
                    .find(|e| mark != 0 && !e.is_header && e.quest_id == mark)
                    .map(|e| e.title.clone())
                    .unwrap_or_default()
            };
            Ok(Value::String(lua.create_string(&title)?))
        })?,
    )?;
    // GetAbandonQuestItems() → nil in v1 (the "you will also lose these quest items" list needs
    // the template's SrcItem/required-item join against the bags — a dressing follow-up).
    g.set(
        "GetAbandonQuestItems",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;
    // AbandonQuest() — the popup's Yes (`0x4dfe00` → `0x4df070`). The reference searches its
    // whole row table `0xbb71c0` (folded quests included) for a quest row carrying the marked id;
    // a hit sends that row's slot and clears the mark, a miss does nothing and KEEPS the mark.
    // Ours checks the same membership — a visible quest row, or one folded under a collapsed
    // header — and queues the id; the app resolves it to the slot.
    g.set(
        "AbandonQuest",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let mark = model.quest_log_abandon_mark;
            let in_log = mark != 0
                && (model
                    .quest_log
                    .entries
                    .iter()
                    .any(|e| !e.is_header && e.quest_id == mark)
                    || model.quest_log.hidden_quest_ids.contains(&mark));
            if in_log {
                model.quest_log_abandon_mark = 0;
                model.quest_log_abandons.push(mark);
            }
            Ok(())
        })?,
    )?;

    // ── Timed quests ─────────────────────────────────────────────────────────────
    // The engine owns the countdown, as the reference's C bindings do: the pushed snapshot carries
    // each row's absolute DEADLINE, and these subtract the live server clock per call. That split
    // is what lets the ref's QuestTimerFrame re-read GetQuestTimers() every OnUpdate and paint a
    // ticking number, while `set_quest_log` only fires QUEST_LOG_UPDATE when the log really moves.

    // GetQuestTimers() → seconds remaining, ONE RETURN PER TIMED QUEST, in quest-log order — a
    // genuine vararg return (the ref reads it as `QuestTimerFrame_Update(GetQuestTimers())` and
    // walks `arg.n`, QuestTimerFrame.lua:11/:19). No timed quests → no values at all, which is the
    // ref's own "hide the frame" signal.
    g.set(
        "GetQuestTimers",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let now = model.server_unix_time;
            Ok(MultiValue::from_vec(
                model
                    .quest_log
                    .entries
                    .iter()
                    .filter_map(|e| live_seconds_left(e, now))
                    .map(Value::Integer)
                    .collect(),
            ))
        })?,
    )?;

    // GetQuestIndexForTimer(timerIndex) → the 1-based QUEST-LOG index that timer belongs to (the
    // same index space GetQuestLogTitle takes — headers included, since the app pushes them into
    // the same list). The ref's timer buttons carry their 1-based ordinal as the frame id and map
    // back through this for the click and the hover tooltip (QuestTimerFrame.lua:43,
    // QuestTimerFrame.xml:20). Out of range → nil.
    g.set(
        "GetQuestIndexForTimer",
        lua.create_function(|lua, t: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let now = model.server_unix_time;
            let index = t.checked_sub(1).and_then(|n| {
                model
                    .quest_log
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| live_seconds_left(e, now).is_some())
                    .nth(n)
                    .map(|(i, _)| i as i64 + 1)
            });
            Ok(match index {
                Some(i) => Value::Integer(i),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetQuestLogTimeLeft() → the SELECTION's seconds remaining, or nil when it has no timer —
    // the ref branches on exactly that nil to show or hide its "Time Remaining:" row and to
    // re-anchor the objectives beneath it (QuestLogFrame.lua:364-374).
    g.set(
        "GetQuestLogTimeLeft",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let left = (model.quest_log_selection as usize)
                .checked_sub(1)
                .and_then(|n| model.quest_log.entries.get(n))
                .and_then(|e| seconds_left(e, model.server_unix_time));
            Ok(match left {
                // Clamped, not dropped — the reference's own asymmetry against the two list
                // bindings above.
                Some(s) => Value::Integer(s.max(0)),
                None => Value::Nil,
            })
        })?,
    )?;

    // ── v1 stubs (each the seam of a named later slice — see the module doc) ─────────────────────
    // GetQuestLogRewardSpell() → texture, name, isTradeskillSpell (0x4e1130): the selected
    // quest's reward spell, three nils when it has none.
    g.set(
        "GetQuestLogRewardSpell",
        lua.create_function(|lua, ()| {
            let spell = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .selected_quest_detail()
                    .and_then(|d| d.reward_spell.clone())
            };
            super::quest::reward_spell_returns(lua, spell)
        })?,
    )?;
    g.set(
        "IsUnitOnQuest",
        lua.create_function(|_, (_q, _unit): (Value, Value)| Ok(flag(false)))?,
    )?;
    // ── The quest watch (the on-screen tracker's state — ref QuestLogFrame.lua:469-505 shift-click
    // toggle, :613-663 QuestWatch_Update). The set lives engine-side, keyed by the entries' stable
    // quest ids (see QuestLogEntryView::quest_id) while the whole Era API speaks 1-based log
    // indices; set_quest_log prunes watches whose quest left the log. MAX_WATCHABLE_QUESTS = 5
    // (ref QuestLogFrame.lua:494) — AddQuestWatch past the cap is a no-op (the Lua shows the
    // QUEST_WATCH_TOO_MANY error itself, mirroring the ref's guard order).
    /// The entry the current selection points at — what every no-argument getter on this surface
    /// reads (`GetQuestLogPushable`, and the C side of `QuestLogPushQuest`). `None` when nothing
    /// is selected or the selection is out of range.
    fn selected_quest(model: &Model) -> Option<&QuestLogEntryView> {
        (model.quest_log_selection as usize)
            .checked_sub(1)
            .and_then(|n| model.quest_log.entries.get(n))
    }

    fn watch_id_at(model: &Model, index: u32) -> Option<u32> {
        (index as usize)
            .checked_sub(1)
            .and_then(|n| model.quest_log.entries.get(n))
            .map(|e| e.quest_id)
    }
    // ── The party share — the ref's `QuestFramePushQuestButton`, whose enable
    // predicate is `GetQuestLogPushable() and GetNumPartyMembers() > 0` (QuestLogFrame.lua:299-305)
    // and whose OnClick is `QuestLogPushQuest()` (QuestLogFrame.xml:511-513). Both speak about the
    // CURRENT SELECTION and take no argument, exactly as the reference declares them.
    // `GetQuestLogPushable` returns `1` or **nil**, never `false` (`0x4e12b0` tail-calls
    // `0x6f3810`/`0x6f37f0`). Both are falsy to the `and` in the reference's own
    // predicate, so the button reads the same either way — but an addon testing `== nil` would
    // not, and this is the Era idiom the rest of this surface already speaks (`GetPartyMember`).
    g.set(
        "GetQuestLogPushable",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match selected_quest(&model).is_some_and(|e| e.pushable) {
                true => Value::Integer(1),
                false => Value::Nil,
            })
        })?,
    )?;
    g.set(
        "QuestLogPushQuest",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            // The C verb re-tests everything the Lua predicate tests **and one thing it does not**:
            // a party check of its own (`0x4e13a6`). The reference does not trust
            // its own button — an addon or a macro can call this solo, and the send is refused
            // here rather than reaching the server. Resolve the id HERE rather than queueing the
            // index (see `Model::quest_log_pushes`); a HEADER row carries `quest_id` 0, so an
            // unguarded resolve would queue a push for quest 0.
            if model.party.members.is_empty() {
                return Ok(());
            }
            let id = selected_quest(&model)
                .filter(|e| !e.is_header && e.pushable)
                .map(|e| e.quest_id);
            if let Some(id) = id {
                model.quest_log_pushes.push(id);
            }
            Ok(())
        })?,
    )?;
    g.set(
        "GetNumQuestWatches",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.quest_log_watched.len() as i64)
        })?,
    )?;
    g.set(
        "IsQuestWatched",
        lua.create_function(|lua, i: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(
                watch_id_at(&model, i).is_some_and(|id| model.quest_log_watched.contains(&id)),
            ))
        })?,
    )?;
    g.set(
        "AddQuestWatch",
        lua.create_function(|lua, i: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(id) = watch_id_at(&model, i) {
                if !model.quest_log_watched.contains(&id) && model.quest_log_watched.len() < 5 {
                    model.quest_log_watched.push(id);
                }
            }
            Ok(())
        })?,
    )?;
    g.set(
        "RemoveQuestWatch",
        lua.create_function(|lua, i: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(id) = watch_id_at(&model, i) {
                model.quest_log_watched.retain(|w| *w != id);
            }
            Ok(())
        })?,
    )?;
    // GetQuestIndexForWatch(watchSlot) → the watched quest's CURRENT 1-based log index (nil if it
    // somehow isn't in the log — pruning makes that transient at worst).
    g.set(
        "GetQuestIndexForWatch",
        lua.create_function(|lua, w: usize| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let index = w
                .checked_sub(1)
                .and_then(|n| model.quest_log_watched.get(n))
                .and_then(|id| {
                    model
                        .quest_log
                        .entries
                        .iter()
                        .position(|e| e.quest_id == *id)
                })
                .map(|p| p as i64 + 1);
            Ok(match index {
                Some(i) => Value::Integer(i),
                None => Value::Nil,
            })
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{QuestLogDetail, QuestLogEntryView, QuestLogObjectiveView, QuestLogState};
    use crate::script::{QuestItemView, UiScript};

    fn two_quests() -> QuestLogState {
        QuestLogState {
            num_quests: 2,
            hidden_quest_ids: Vec::new(),
            entries: vec![
                QuestLogEntryView {
                    quest_id: 783,
                    title: "A Threat Within".into(),
                    level: 1,
                    complete: 0,
                    objectives: vec![QuestLogObjectiveView {
                        text: "Kobold Vermin slain: 3/10".into(),
                        kind: "monster".into(),
                        finished: false,
                        cur: 3,
                        req: 10,
                    }],
                    detail: Some(QuestLogDetail {
                        description: "Speak with Marshal McBride.".into(),
                        objectives_text: "Report to Marshal McBride.".into(),
                        required_money: 0,
                        reward_money: 40,
                        choices: vec![],
                        rewards: vec![QuestItemView {
                            item_id: 2024,
                            name: Some("Militia Hammer".into()),
                            texture: None,
                            count: 1,
                            quality: 1,
                            usable: true,
                            link: Some("|cffffffff|Hitem:2024:0:0:0|h[Militia Hammer]|h|r".into()),
                        }],
                        reward_spell: None,
                    }),
                    ..Default::default()
                },
                QuestLogEntryView {
                    quest_id: 7,
                    title: "Kobold Camp Cleanup".into(),
                    level: 3,
                    complete: 1,
                    objectives: vec![QuestLogObjectiveView {
                        text: "Kobold Worker slain: 10/10".into(),
                        kind: "monster".into(),
                        finished: true,
                        cur: 10,
                        req: 10,
                    }],
                    detail: Some(QuestLogDetail {
                        description: "The kobolds have overrun the camp.".into(),
                        objectives_text: "Kill 10 Kobold Vermin.".into(),
                        required_money: 0,
                        reward_money: 250,
                        choices: vec![],
                        rewards: vec![],
                        reward_spell: None,
                    }),
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn entries_and_title_tuple() {
        let mut s = UiScript::new().unwrap();
        // Empty log: 0, 0; out-of-range title is nil.
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumQuestLogEntries()")
                .unwrap(),
            (0, 0)
        );
        assert!(s.eval::<bool>("return GetQuestLogTitle(1) == nil").unwrap());

        s.set_quest_log(two_quests());
        assert_eq!(
            s.eval::<(i64, i64)>("return GetNumQuestLogEntries()")
                .unwrap(),
            (2, 2)
        );
        // The 6-tuple, in order; complete=0 → nil, complete=1 → 1.
        assert!(s
            .eval::<bool>(
                "local t, l, tag, h, c, done = GetQuestLogTitle(1)\n\
                 return t == 'A Threat Within' and l == 1 and tag == nil\n\
                    and h == nil and c == nil and done == nil"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local t, _, _, _, _, done = GetQuestLogTitle(2)\n\
                 return t == 'Kobold Camp Cleanup' and done == 1"
            )
            .unwrap());
    }

    /// The arity contract (`0x4df930`): six values ALWAYS, an out-of-range number is not an
    /// error, and return 2 is the number `0`
    /// — while a missing or non-number argument raises `Usage:`.
    #[test]
    fn out_of_range_is_six_values_with_a_zero_level_and_a_bad_arg_raises() {
        let mut s = UiScript::new().unwrap();
        // Empty log: the first return is still nil (so the ref's `if ( questLogTitleText )`
        // reads the same), but the tuple is full and the level is a NUMBER.
        assert_eq!(s.arity("GetQuestLogTitle(1)").unwrap(), 6);
        assert!(s
            .eval::<bool>("local t, l = GetQuestLogTitle(1) return t == nil and l == 0")
            .unwrap());

        s.set_quest_log(two_quests());
        // Index 0 and a negative index are out of range, not errors — same six values.
        for i in ["0", "-1", "99"] {
            assert!(
                s.eval::<bool>(&format!(
                    "local t, l = GetQuestLogTitle({i}) return t == nil and l == 0"
                ))
                .unwrap(),
                "index {i} is out of range, not an error"
            );
        }
        // `_ftol` truncates toward zero, so 1.9 addresses entry 1.
        assert!(s
            .eval::<bool>("return GetQuestLogTitle(1.9) == 'A Threat Within'")
            .unwrap());

        // A missing / non-number argument is the other shape.
        for bad in ["", "nil", "{}", "print"] {
            let e = format!(
                "{:?}",
                s.eval::<mlua::Value>(&format!("return GetQuestLogTitle({bad})"))
                    .unwrap_err()
            );
            assert!(
                e.contains("Usage: GetQuestLogTitle(index)"),
                "arg `{bad}` must raise Usage:, got {e}"
            );
        }
    }

    /// `isHeader`/`isCollapsed`, returns 4 and 5: `1` on a header, `nil` on a quest; `isCollapsed`
    /// is `1` only for a COLLAPSED header — an expanded one and every quest row answer `nil`.
    #[test]
    fn is_header_and_is_collapsed_are_one_or_nil_never_booleans() {
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        state.entries.insert(
            0,
            QuestLogEntryView {
                title: "Elwynn Forest".into(),
                is_header: true,
                collapsed: true,
                ..Default::default()
            },
        );
        state.entries.insert(
            1,
            QuestLogEntryView {
                title: "Westfall".into(),
                is_header: true,
                collapsed: false,
                ..Default::default()
            },
        );
        s.set_quest_log(state);

        // A collapsed header: both flags are the number 1.
        assert!(s
            .eval::<bool>(
                "local _, l, _, h, c = GetQuestLogTitle(1) return h == 1 and c == 1 and l == 0"
            )
            .unwrap());
        // An EXPANDED header: isHeader 1, isCollapsed nil.
        assert!(s
            .eval::<bool>("local _, _, _, h, c = GetQuestLogTitle(2) return h == 1 and c == nil")
            .unwrap());
        // A quest row: both nil — and `false` would be wrong, so pin the type too.
        assert!(s
            .eval::<bool>(
                "local _, _, _, h, c = GetQuestLogTitle(3)\n\
                 return h == nil and c == nil and type(h) ~= 'boolean'"
            )
            .unwrap());
    }

    #[test]
    fn tag_is_the_bare_word_and_nil_when_absent() {
        // The third return is the BARE word — the ref's own Lua adds the parentheses
        // (`"("..questTag..")"`, ref QuestLogFrame.lua:195), so pushing "(Elite)" here would
        // paint "((Elite))". An untagged quest pushes nil, not "", because the ref branches on
        // presence (`if ( questTag )`, ref l.194).
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        state.entries[1].tag = Some("Elite".into());
        s.set_quest_log(state);
        assert!(s
            .eval::<bool>("local _, _, tag = GetQuestLogTitle(1) return tag == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("local _, _, tag = GetQuestLogTitle(2) return tag == 'Elite'")
            .unwrap());
    }

    #[test]
    fn selection_is_synchronous() {
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(two_quests());
        assert_eq!(s.eval::<i64>("return GetQuestLogSelection()").unwrap(), 0);
        // The ref's click flow: select then immediately re-read in the same chunk.
        assert_eq!(
            s.eval::<i64>("SelectQuestLogEntry(2); return GetQuestLogSelection()")
                .unwrap(),
            2
        );
        assert_eq!(s.quest_log_selection(), 2);
    }

    /// **The bug decision 2247 fixes, in the shape every addon hits it.** The classic quest-log
    /// walk — `SelectQuestLogEntry(i)` then `GetQuestLogQuestText()`, once per entry, all inside
    /// ONE frame with no push between — must answer about the row just selected. It used to answer
    /// with whichever row the snapshot had been built against, for every entry: Questie 3.7.1 fed
    /// that text to its `getQuestHash` Levenshtein match and wrote the WRONG same-name chain step
    /// into a character's saved history (two of the director's Tirisfal quests, both resolved to
    /// the shortest sibling — the signature of matching against an unrelated string).
    #[test]
    fn a_log_walk_answers_per_entry_without_a_push_between() {
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(two_quests());
        let walk: String = s
            .eval(
                "local out = ''\n\
                 for i = 1, GetNumQuestLogEntries() do\n\
                   SelectQuestLogEntry(i)\n\
                   local desc, obj = GetQuestLogQuestText()\n\
                   out = out .. i .. '=' .. desc .. '|'\n\
                 end\n\
                 return out",
            )
            .unwrap();
        assert_eq!(
            walk, "1=Speak with Marshal McBride.|2=The kobolds have overrun the camp.|",
            "each entry must answer with its OWN detail inside a single frame"
        );
    }

    /// The same seam for the money/reward getters, which read the detail through their own path.
    #[test]
    fn the_detail_counts_follow_the_selection_too() {
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(two_quests());
        assert_eq!(
            s.eval::<i64>("SelectQuestLogEntry(1); return GetQuestLogRewardMoney()")
                .unwrap(),
            40
        );
        assert_eq!(
            s.eval::<i64>("SelectQuestLogEntry(2); return GetQuestLogRewardMoney()")
                .unwrap(),
            250
        );
        assert_eq!(
            s.eval::<i64>("SelectQuestLogEntry(2); return GetNumQuestLogRewards()")
                .unwrap(),
            0
        );
    }

    /// A header row has no quest under it, so its detail reads as nothing at all rather than as
    /// the previous selection's — the same staleness, one row over.
    #[test]
    fn a_header_selection_has_no_detail() {
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        state.entries.insert(
            0,
            QuestLogEntryView {
                title: "Elwynn Forest".into(),
                is_header: true,
                ..Default::default()
            },
        );
        s.set_quest_log(state);
        assert!(s
            .eval::<bool>(
                "SelectQuestLogEntry(1); local d = GetQuestLogQuestText(); return d == ''"
            )
            .unwrap());
    }

    #[test]
    fn detail_getters_read() {
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(two_quests());
        s.run("SelectQuestLogEntry(1)").unwrap();
        assert!(s
            .eval::<bool>(
                "local d, o = GetQuestLogQuestText()\n\
                 return d == 'Speak with Marshal McBride.' and o == 'Report to Marshal McBride.'"
            )
            .unwrap());
        assert_eq!(
            s.eval::<i64>("return GetNumQuestLeaderBoards()").unwrap(),
            1
        );
        // The optional-arg form answers for ANY entry (the tracker HUD's path).
        assert_eq!(
            s.eval::<i64>("return GetNumQuestLeaderBoards(1)").unwrap(),
            1
        );
        assert_eq!(
            s.eval::<i64>("return GetNumQuestLeaderBoards(2)").unwrap(),
            1
        );
        assert!(s
            .eval::<bool>(
                "local t, k, f = GetQuestLogLeaderBoard(1, 2)\n\
                 return t == 'Kobold Worker slain: 10/10' and k == 'monster' and f == true"
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local t, k, f = GetQuestLogLeaderBoard(1)\n\
                 return t == 'Kobold Vermin slain: 3/10' and k == 'monster' and f == false"
            )
            .unwrap());
        assert_eq!(
            s.eval::<i64>("return GetQuestLogRewardMoney()").unwrap(),
            40
        );
        assert_eq!(s.eval::<i64>("return GetNumQuestLogRewards()").unwrap(), 1);
        assert!(s
            .eval::<bool>(
                "local n, _, c, q, u = GetQuestLogRewardInfo(1)\n\
                 return n == 'Militia Hammer' and c == 1 and q == 1 and u == true"
            )
            .unwrap());
        assert!(s
            .eval::<bool>("return GetQuestLogChoiceInfo(1) == nil")
            .unwrap());

        // GetQuestLogItemLink: kind-dispatched (the shape the ref's QuestLogRewardItem_OnClick
        // calls it with — `this.type`), nil for the empty choice list, an out-of-range index and
        // an unknown type.
        assert_eq!(
            s.eval::<String>("return GetQuestLogItemLink(\"reward\", 1)")
                .unwrap(),
            "|cffffffff|Hitem:2024:0:0:0|h[Militia Hammer]|h|r"
        );
        assert!(s
            .eval::<bool>("return GetQuestLogItemLink(\"choice\", 1) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetQuestLogItemLink(\"reward\", 9) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return GetQuestLogItemLink(\"bogus\", 1) == nil")
            .unwrap());
    }

    #[test]
    fn abandon_two_step_pins_the_click_time_target() {
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(two_quests());
        s.run("SelectQuestLogEntry(2); SetAbandonQuest()").unwrap();
        assert_eq!(
            s.eval::<String>("return GetAbandonQuestName()").unwrap(),
            "Kobold Camp Cleanup"
        );
        // Selection moves before the confirm — the mark must not follow it. What queues is the
        // marked QUEST ID (entry 2 is quest 7), never a row index (0x4dfb50).
        s.run("SelectQuestLogEntry(1); AbandonQuest()").unwrap();
        assert_eq!(s.take_quest_log_abandons(), vec![7]);
        assert!(s.take_quest_log_abandons().is_empty(), "drained");
        // A bare AbandonQuest with no mark queues nothing.
        s.run("AbandonQuest()").unwrap();
        assert!(s.take_quest_log_abandons().is_empty());
    }

    #[test]
    fn failed_state_reads() {
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        state.entries[0].complete = -1;
        s.set_quest_log(state);
        s.run("SelectQuestLogEntry(1)").unwrap();
        assert!(s.eval::<bool>("return IsCurrentQuestFailed()").unwrap());
        assert!(s
            .eval::<bool>("local _, _, _, _, _, done = GetQuestLogTitle(1)\nreturn done == -1")
            .unwrap());
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert!(!s.eval::<bool>("return IsCurrentQuestFailed()").unwrap());
    }

    #[test]
    fn watch_set_is_id_keyed_and_survives_a_log_shuffle() {
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(two_quests());
        // Watch entry 2 (quest 7); the index-based API reads it back.
        s.run("AddQuestWatch(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 1);
        assert!(s.eval::<bool>("return IsQuestWatched(2)").unwrap());
        assert!(!s.eval::<bool>("return IsQuestWatched(1)").unwrap());
        assert_eq!(s.eval::<i64>("return GetQuestIndexForWatch(1)").unwrap(), 2);
        assert_eq!(s.quest_log_watched(), vec![7]);

        // The log shuffles: entry 1 (quest 783) is abandoned, quest 7 compacts to index 1 — the
        // watch FOLLOWS THE QUEST, not the index.
        let mut shuffled = two_quests();
        shuffled.entries.remove(0);
        s.set_quest_log(shuffled);
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 1);
        assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
        assert_eq!(s.eval::<i64>("return GetQuestIndexForWatch(1)").unwrap(), 1);

        // Unwatch by the new index; and a watched quest leaving the log prunes its watch.
        s.run("RemoveQuestWatch(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 0);
        s.run("AddQuestWatch(1)").unwrap();
        s.set_quest_log(QuestLogState::default());
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 0);
    }

    /// A header's collapse folds its quests out of the visible list but NOT out of the log, and
    /// the watch prune counts them — the reference's prune (`0x4de7a7`–`0x4de80f`) scans the whole
    /// row array for a non-header row with the watched id, and a collapsed-group quest is still a
    /// row there, just sorted past the visible window; the collapse itself (`0x4ded30`) re-sorts
    /// and recounts only. So watch →
    /// collapse → expand keeps the watch, and a quest that genuinely LEFT the log still drops it.
    #[test]
    fn a_collapsed_header_keeps_its_quests_watched() {
        let header = |collapsed| QuestLogEntryView {
            title: "Elwynn Forest".into(),
            is_header: true,
            collapsed,
            ..Default::default()
        };
        let expanded = || {
            let mut state = two_quests();
            state.entries.insert(0, header(false));
            state
        };
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(expanded());
        s.run("AddQuestWatch(3)").unwrap(); // quest 7
        assert_eq!(s.quest_log_watched(), vec![7]);

        // The app's collapsed push: the header alone is visible, both quests are hidden under it.
        s.set_quest_log(QuestLogState {
            entries: vec![header(true)],
            num_quests: 2,
            hidden_quest_ids: vec![783, 7],
        });
        assert_eq!(
            s.eval::<i64>("return GetNumQuestWatches()").unwrap(),
            1,
            "a collapse must not prune the watch of a quest still in the log"
        );
        s.set_quest_log(expanded());
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetQuestIndexForWatch(1)").unwrap(), 3);

        // Quest 7 leaves the log (turn-in) — neither visible nor hidden — and its watch goes.
        let mut gone = expanded();
        gone.entries.retain(|e| e.quest_id != 7);
        gone.num_quests = 1;
        s.set_quest_log(gone);
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 0);
    }

    /// The countdown trio. One deadline stamp in the snapshot, one clock
    /// pushed beside it, and every read is a live subtraction — so advancing the clock alone (no
    /// re-push of the log) makes the number fall, which is precisely what the reference's
    /// per-OnUpdate `GetQuestTimers()` depends on.
    ///
    /// Every number below carries the reference's `−1` (`value = deadline + G − now − 1`,
    /// byte-verified at all four of its consumers): 500 seconds of wall gap reads 499.
    #[test]
    fn timers_count_down_against_the_pushed_server_clock() {
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        // Quest 2 is on a 15-minute leash that ends at unix 1_000_900; quest 1 is untimed.
        state.entries[1].timer = 1_000_900;
        s.set_quest_log(state);

        // No clock sample yet: nothing to subtract from, so nothing is claimed.
        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetQuestLogTimeLeft() == nil")
            .unwrap());

        // 400 s in.
        s.set_server_unix_time(1_000_400.0);
        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return (GetQuestTimers())").unwrap(), 499);
        // The timer maps back to entry 2 — the untimed row 1 does not shift the mapping.
        assert_eq!(s.eval::<i64>("return GetQuestIndexForTimer(1)").unwrap(), 2);
        assert!(s
            .eval::<bool>("return GetQuestIndexForTimer(2) == nil")
            .unwrap());

        // The selection's own reading: nil on the untimed row, the count on the timed one — the
        // exact branch the ref's "Time Remaining:" row keys on.
        s.run("SelectQuestLogEntry(1)").unwrap();
        assert!(s
            .eval::<bool>("return GetQuestLogTimeLeft() == nil")
            .unwrap());
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 499);

        // Only the CLOCK moves — no set_quest_log — and the number falls anyway.
        s.set_server_unix_time(1_000_880.0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 19);

        // The last second reads 0, not 1 — the `−1`'s downward bias at the very end of the window.
        s.set_server_unix_time(1_000_899.0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 0);

        // Past the deadline the two bindings SPLIT, exactly as the reference's do: the list drops
        // the row (its `js`), while GetQuestLogTimeLeft clamps at 0 and keeps answering.
        s.set_server_unix_time(1_000_910.0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 0);
        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetQuestIndexForTimer(1) == nil")
            .unwrap());
    }

    /// A FAILED timed quest shows no timer. vmangos writes the slot timer to the literal `1` and
    /// sets the FAIL state bit (`Player::FailQuest`), leaving the row in the log until the player
    /// abandons it — so without the state test the frame would show a clamped "0" for ever.
    #[test]
    fn a_failed_timed_quest_drops_out_of_the_timer_list() {
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        state.entries[0].timer = 1_000_900;
        state.entries[1].timer = 1; // failed: the server's own sentinel
        state.entries[1].complete = -1;
        s.set_quest_log(state);
        s.set_server_unix_time(1_000_400.0);

        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return (GetQuestTimers())").unwrap(), 499);
        assert_eq!(s.eval::<i64>("return GetQuestIndexForTimer(1)").unwrap(), 1);
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert!(s
            .eval::<bool>("return GetQuestLogTimeLeft() == nil")
            .unwrap());
    }

    #[test]
    fn watch_cap_is_five_and_dupes_are_ignored() {
        let mut s = UiScript::new().unwrap();
        let mut state = QuestLogState::default();
        for q in 1..=6u32 {
            state.entries.push(QuestLogEntryView {
                quest_id: q,
                title: format!("Quest {q}"),
                level: 1,
                ..Default::default()
            });
        }
        s.set_quest_log(state);
        for i in 1..=6 {
            s.run(&format!("AddQuestWatch({i})")).unwrap();
        }
        // The sixth add is a no-op (MAX_WATCHABLE_QUESTS = 5, ref QuestLogFrame.lua:494).
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 5);
        // A duplicate add neither grows nor reorders.
        s.run("AddQuestWatch(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 5);
        assert_eq!(s.quest_log_watched(), vec![1, 2, 3, 4, 5]);
    }

    /// `GetQuestLogPushable` answers about the SELECTION, and only about the selection — the Era
    /// API has no per-index form. Selecting the other quest changes the answer.
    #[test]
    fn pushable_follows_the_selection() {
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        state.entries[0].pushable = true; // 783 is sharable, 7 is not
        s.set_quest_log(state);

        // `1` or nil, never true/false (`0x4e12b0`) — an addon testing `== nil`
        // sees the reference's own shape.
        assert_eq!(
            s.eval::<Option<i64>>("return GetQuestLogPushable()")
                .unwrap(),
            None,
            "no selection yet"
        );
        s.run("SelectQuestLogEntry(1)").unwrap();
        assert_eq!(
            s.eval::<Option<i64>>("return GetQuestLogPushable()")
                .unwrap(),
            Some(1)
        );
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert_eq!(
            s.eval::<Option<i64>>("return GetQuestLogPushable()")
                .unwrap(),
            None
        );
    }

    /// The abandon mark follows its QUEST across a re-index, and a mark whose quest has left the
    /// log is a no-op that is KEPT, as `0x4df070`'s miss path is (`ret` before the clear at
    /// `0x4df0cf`). A quest folded under a collapsed header is still in the log.
    #[test]
    fn abandon_mark_is_a_quest_id_that_survives_a_reindex() {
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(two_quests());
        s.run("SelectQuestLogEntry(2); SetAbandonQuest()").unwrap(); // quest 7

        // The log re-indexes under the popup: quest 7 is now row 1, and a stranger takes row 2.
        let mut moved = two_quests();
        moved.entries.swap(0, 1);
        moved.entries[1].quest_id = 999;
        s.set_quest_log(moved.clone());
        assert_eq!(
            s.eval::<String>("return GetAbandonQuestName()").unwrap(),
            "Kobold Camp Cleanup"
        );
        s.run("AbandonQuest()").unwrap();
        assert_eq!(s.take_quest_log_abandons(), vec![7]);

        // Folded away: still abandonable.
        s.run("SelectQuestLogEntry(1); SetAbandonQuest()").unwrap();
        let mut folded = moved.clone();
        folded.entries.remove(0);
        folded.hidden_quest_ids = vec![7];
        s.set_quest_log(folded);
        s.run("AbandonQuest()").unwrap();
        assert_eq!(s.take_quest_log_abandons(), vec![7]);

        // Gone from the log: nothing queues, and the mark stays for the quest's return.
        s.set_quest_log(moved.clone());
        s.run("SelectQuestLogEntry(1); SetAbandonQuest()").unwrap();
        let mut gone = moved.clone();
        gone.entries.remove(0);
        s.set_quest_log(gone);
        s.run("AbandonQuest()").unwrap();
        assert!(s.take_quest_log_abandons().is_empty());
        s.set_quest_log(moved);
        s.run("AbandonQuest()").unwrap();
        assert_eq!(
            s.take_quest_log_abandons(),
            vec![7],
            "the kept mark still names 7"
        );
    }

    /// `QuestLogPushQuest` queues the selected entry's **quest id**, resolved at click time — not
    /// its index, so a log that reshuffles between the click and the app's drain cannot retarget
    /// the push — the same keying as the abandon mark.
    /// A party of one other, so the C verb's own party check (below) passes.
    fn in_a_party(s: &mut UiScript) {
        s.set_party(crate::script::PartyState {
            members: vec![crate::script::PartyMemberInfo {
                name: "Mate".into(),
                guid: 0x300,
            }],
            ..Default::default()
        });
    }

    #[test]
    fn push_queues_the_selected_quest_id_not_its_index() {
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        state.entries[1].pushable = true; // quest id 7
        s.set_quest_log(state);
        in_a_party(&mut s);
        s.run("SelectQuestLogEntry(2)").unwrap();
        s.run("QuestLogPushQuest()").unwrap();
        assert_eq!(s.take_quest_log_pushes(), vec![7], "entry 2 is quest id 7");
        assert!(s.take_quest_log_pushes().is_empty(), "drained");
    }

    /// A push with nothing selected is silently nothing — the window's button is disabled in that
    /// state, so this is only reachable from a macro or an addon, and the client has no quest id
    /// to send.
    #[test]
    fn push_without_a_selection_queues_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(two_quests());
        in_a_party(&mut s);
        s.run("QuestLogPushQuest()").unwrap();
        assert!(s.take_quest_log_pushes().is_empty());
    }

    /// **The C verb re-tests the party and the sharable bit itself** (`0x4e13a6`) —
    /// the reference does not trust its own button, because a macro or an addon can call this with
    /// the window shut. Each guard is moved alone, against an otherwise-valid push.
    #[test]
    fn push_refuses_solo_and_refuses_an_unsharable_quest() {
        let sharable = || {
            let mut state = two_quests();
            state.entries[0].pushable = true; // quest id 783
            state
        };

        // Solo: everything else is right and it still sends nothing.
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(sharable());
        s.run("SelectQuestLogEntry(1)").unwrap();
        s.run("QuestLogPushQuest()").unwrap();
        assert!(s.take_quest_log_pushes().is_empty(), "solo pushes nothing");

        // In a party, the same call goes.
        in_a_party(&mut s);
        s.run("QuestLogPushQuest()").unwrap();
        assert_eq!(s.take_quest_log_pushes(), vec![783]);

        // In a party, on a quest without the sharable bit: nothing again.
        s.run("SelectQuestLogEntry(2)").unwrap();
        s.run("QuestLogPushQuest()").unwrap();
        assert!(
            s.take_quest_log_pushes().is_empty(),
            "an unsharable quest pushes nothing even in a party"
        );
    }

    /// A HEADER row carries `quest_id` 0, so pushing one must queue nothing at all — an unguarded
    /// id resolve would send `CMSG_PUSHQUESTTOPARTY{0}`. The window's button is disabled on a
    /// header, so only a macro or an addon can get here.
    #[test]
    fn push_on_a_header_row_queues_nothing() {
        let mut s = UiScript::new().unwrap();
        in_a_party(&mut s);
        let mut state = two_quests();
        state.entries.insert(
            0,
            QuestLogEntryView {
                quest_id: 0,
                title: "Elwynn Forest".into(),
                is_header: true,
                pushable: true, // even if something wrongly marked it so
                ..Default::default()
            },
        );
        s.set_quest_log(state);
        s.run("SelectQuestLogEntry(1)").unwrap();
        s.run("QuestLogPushQuest()").unwrap();
        assert!(
            s.take_quest_log_pushes().is_empty(),
            "a header is not a quest"
        );
    }
}
