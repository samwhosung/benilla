//! The quest-log verbs the stock `QuestLogFrame.lua` calls, over a [`QuestLogState`] the app
//! pushes (rows from the `PLAYER_QUEST_LOG` descriptor slots, text from the quest template cache),
//! with the abandon, share and fold intents drained back. The selection lives in the engine, as
//! `QuestLog_SetSelection` sets and re-reads it in one call (`QuestLogFrame.lua:318`, `:342`). The
//! abandon mark and the share hold the quest id selected at the click, as the reference's mark
//! does (`0x4dfb50` copies the selection `0xbb7480`, a quest id, to `0xbb7484`), so a re-index
//! before the popup's Yes cannot retarget them.
//!
//! Not built: `IsUnitOnQuest` and `GetAbandonQuestItems` answer nil; the first needs the party
//! members' quest logs.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::{flag, number_arg};
use super::quest::QuestItemView;
use super::Model;

/// One quest-log row; its 1-based position in [`QuestLogState::entries`] is the index every verb
/// takes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestLogEntryView {
    /// The slot's quest id, never returned to Lua; watches, marks and shares key on it, as
    /// indexes shift.
    pub quest_id: u32,
    pub title: String,
    /// `GetQuestLogTitle`'s return 2, which colors the stock row.
    pub level: u32,
    /// The bare tag word (`Elite`, `Dungeon`, `Raid`, …) from the template's `Type` through
    /// `QuestInfo.dbc`; the stock row adds the parentheses (`QuestLogFrame.lua:195`). `None`,
    /// never `Some("")`, for no tag: the row branches on its presence (`QuestLogFrame.lua:194`).
    pub tag: Option<String>,
    /// A zone header row, which the app builds from each quest's `ZoneOrSort`.
    pub is_header: bool,
    /// `GetQuestLogPushable`: the template's `QUEST_FLAGS_SHARABLE` (`0x8`), false until the
    /// template arrives, as the reference reads the same cache.
    pub pushable: bool,
    /// A collapsed header (`isCollapsed`), whose quests the app leaves out of `entries`.
    pub collapsed: bool,
    /// The slot state: `1` complete, `-1` failed, `0` in progress (`isComplete` 1, -1 or nil).
    pub complete: i32,
    /// The slot's deadline in unix seconds (`time(nullptr) + limitTime`, vmangos
    /// `Player::AddQuest`), 0 when untimed. A stamp, not a countdown: the app diffs this snapshot
    /// to fire `QUEST_LOG_UPDATE`, so a live number here would rebuild the log every frame.
    pub timer: u32,
    /// This row's objective lines; the leaderboard verbs serve any row, selected or not.
    pub objectives: Vec<QuestLogObjectiveView>,
    /// This row's detail pane, `None` on a header. Per row, not per selection: the reference's
    /// detail verbs read the selection `0xbb7480` that `SelectQuestLogEntry` (`0x4dfae0`) wrote and
    /// look the quest up in the same call (`0x4e1130` to `0xc0e1b0`, `0x562a40`), so a
    /// select-then-read answers about the row just selected.
    pub detail: Option<QuestLogDetail>,
}

/// One objective line (`GetQuestLogLeaderBoard`), `text` formatted by the app, as
/// "Kobold Vermin slain: 3/10".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestLogObjectiveView {
    pub text: String,
    /// The type string: `"monster"`, `"item"` or `"object"`.
    pub kind: String,
    /// Done: the stock log greys the line and appends `(Complete)` (`QuestLogFrame.lua:389`).
    pub finished: bool,
    /// The line's `%d/%d`, `cur` clamped to `req`, not returned to Lua. The app's progress
    /// announce compares these, as a turn-in destroys the items a frame or two before the slot
    /// clears and the line dips to `0/req`, which text alone cannot tell from progress.
    pub cur: u32,
    /// See [`Self::cur`].
    pub req: u32,
}

/// A quest's detail pane, from the app's template and name caches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestLogDetail {
    /// `GetQuestLogQuestText`'s return 1.
    pub description: String,
    /// `GetQuestLogQuestText`'s return 2, the objectives paragraph, not the leaderboard lines.
    pub objectives_text: String,
    /// `GetQuestLogRequiredMoney`, in copper.
    pub required_money: u32,
    /// `GetQuestLogRewardMoney`, in copper.
    pub reward_money: u32,
    /// `GetNumQuestLogChoices`/`GetQuestLogChoiceInfo`.
    pub choices: Vec<QuestItemView>,
    /// `GetNumQuestLogRewards`/`GetQuestLogRewardInfo`.
    pub rewards: Vec<QuestLogQuestItem>,
    /// The template's `rewSpell`, as `GetQuestLogRewardSpell` answers it.
    pub reward_spell: Option<super::quest::QuestRewardSpell>,
}

/// A quest-log reward row is the same shape as a questgiver panel row.
pub type QuestLogQuestItem = QuestItemView;

/// The quest-log snapshot the app pushes on each change; an empty log is empty `entries`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestLogState {
    /// The visible rows, in log order; a collapsed header's quests are left out.
    pub entries: Vec<QuestLogEntryView>,
    /// `GetNumQuestLogEntries`'s return 2: every quest, folded ones included, for the
    /// "Quests: N/20" count (`QuestLogFrame.lua:290`).
    pub num_quests: u32,
    /// Quests folded under a collapsed header: in the log, not in `entries`. Their watches and
    /// abandon marks hold, as the reference's watch prune (`0x4de7a7` to `0x4de80f`) and abandon
    /// search (`0xbb71c0`) cover every row, folded ones included.
    pub hidden_quest_ids: Vec<u32>,
}

impl super::UiScript {
    /// Push the quest-log snapshot, dropping the watch of a quest that left the log: the
    /// reference's prune (`0x4de7a7` to `0x4de80f`) keeps a watch while any quest row, visible or
    /// folded, carries its id, and a collapse (`0x4ded30`) never prunes.
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

    /// The watched quest ids, in watch order.
    pub fn quest_log_watched(&self) -> Vec<u32> {
        self.model_ref().quest_log_watched.clone()
    }

    /// The 1-based selection, 0 for none; the app reads it to re-point it across a rebuild.
    pub fn quest_log_selection(&self) -> u32 {
        self.model_ref().quest_log_selection
    }

    /// Drain the abandons, quest ids marked by `SetAbandonQuest` and confirmed by
    /// `AbandonQuest()`; the app sends each slot's `CMSG_QUESTLOG_REMOVE_QUEST`.
    pub fn take_quest_log_abandons(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().quest_log_abandons)
    }

    /// Drain the shares `QuestLogPushQuest()` queued, as quest ids; the app sends one
    /// `CMSG_PUSHQUESTTOPARTY` each.
    pub fn take_quest_log_pushes(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().quest_log_pushes)
    }

    /// Drain the `ConfirmAcceptQuest()` calls as a count: the verb carries no quest id, so the app
    /// answers the confirm it holds.
    pub fn take_quest_confirms(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().quest_confirms)
    }

    /// Drain `CollapseQuestHeader`/`ExpandQuestHeader` as `(1-based index, collapse)`; index 0
    /// is every header, as the collapse-all button passes (`QuestLogFrame.lua:557`, `:561`).
    pub fn take_quest_log_collapses(&mut self) -> Vec<(u32, bool)> {
        std::mem::take(&mut self.model_mut().quest_log_collapses)
    }

    /// Set the selection, re-pointed by the app at the same quest when a rebuild shifts the rows.
    pub fn set_quest_log_selection(&mut self, i: u32) {
        self.model_mut().quest_log_selection = i;
    }

    /// Push the server clock in unix seconds each frame: the app's `SMSG_QUERY_TIME_RESPONSE`
    /// sample, advanced monotonically. The reference keeps an offset `G = local - server` from the
    /// sync and computes `deadline + G - localNow()`, the same value. Deviation: a local clock
    /// step mid-session (an NTP correction) jumps the reference's countdown but not ours, because
    /// the step changes no time the quest has left.
    pub fn set_server_unix_time(&mut self, unix_secs: f64) {
        self.model_mut().server_unix_time = Some(unix_secs);
    }
}

/// Signed seconds left on a row's timer, by the reference's formula at all four of its sites
/// (`0x4e0905`, `0x4e14b9`, `0x4e16c9`, `0x4de6b6`): `slot.timer + G - now - 1`, `G` folded into
/// `now`, so a quest's last second reads 0. A negative value drops the row from the lists (`js` at
/// `0x4e14c5`) and clamps to 0 in `GetQuestLogTimeLeft`. `None` when there is nothing to show:
/// - an untimed row, or a header (skipped at `0x4e1460`);
/// - a failed row: vmangos sets its timer to 1 (`Player.cpp:13430`), and the reference tests the
///   fail bit `[slot+0x7] & 0x2` first;
/// - no server clock yet, the reference's never-synced `G == 0`.
fn seconds_left(entry: &QuestLogEntryView, now: Option<f64>) -> Option<i64> {
    if entry.timer == 0 || entry.complete < 0 {
        return None;
    }
    let now = now?;
    Some((f64::from(entry.timer) - now - 1.0).floor() as i64)
}

/// [`seconds_left`] for the list verbs, which drop an expired row as the reference's `js` does.
fn live_seconds_left(entry: &QuestLogEntryView, now: Option<f64>) -> Option<i64> {
    seconds_left(entry, now).filter(|&s| s >= 0)
}

/// Register the quest-log globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // `numEntries` counts header rows, `numQuests` only quests (`QuestLogFrame.lua:108`).
    g.set(
        "GetNumQuestLogEntries",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let entries = &model.quest_log.entries;
            Ok((entries.len() as i64, i64::from(model.quest_log.num_quests)))
        })?,
    )?;

    // `title, level, tag, isHeader, isCollapsed, isComplete`, always six (`0x4df930`). The index
    // truncates as `_ftol` does, and a missing or non-number one raises `Usage:`; out of range it
    // answers `nil, 0, nil, nil, nil, nil`, and a header's level is 0 too.
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
            // Returns 4 and 5 are 1 or nil, never booleans; `isCollapsed` is 1 only on a header
            // whose bit in `[0xbb748c]` is clear.
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

    g.set(
        "SelectQuestLogEntry",
        lua.create_function(|lua, i: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.quest_log_selection = i;
            Ok(())
        })?,
    )?;

    // The 1-based selection, 0 for none.
    g.set(
        "GetQuestLogSelection",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.quest_log_selection))
        })?,
    )?;

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

    // Any row's line count, the selection's by default: the watch tracker reads unselected quests
    // (`QuestLogFrame.lua:616`).
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

    // `text, type, finished` of line `i`, for any row as above (`QuestLogFrame.lua:635`).
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

    // ── Detail counts + money ────────────────────────────────────────────────────────────────────
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

    // `name, texture, numItems, quality, isUsable`, the giver panels' shape, as one stock routine
    // lays out both (`QuestFrameItems_Update`, `QuestFrame.lua:311`).
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
                    // The item id, a sixth return that is not 1.12's.
                    Value::Integer(i64::from(it.item_id)),
                ]))
            })?,
        )
    }
    install_detail_item(lua, "GetQuestLogChoiceInfo", |d| &d.choices)?;
    install_detail_item(lua, "GetQuestLogRewardInfo", |d| &d.rewards)?;

    // The item's escaped link, for the reward rows' ctrl and shift clicks (`QuestLogFrame.lua:545`,
    // `:549`), keyed by `this.type`, "choice" or "reward" (set by `QuestFrameItems_Update`). An
    // unknown type, an index out of range or a template not yet arrived answers nil.
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

    // 1 or nil, whether the selected slot has failed; the stock title then appends " - (Failed)"
    // (`QuestLogFrame.lua:354`).
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

    // Header folds, drained by the app, which owns the collapse set; index 0 is every header.
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

    // ── The abandon two-step (mark → confirm) ────────────────────────────────────────────────────
    // Marks the selected quest (`QuestLogFrame.xml:464`). The reference's `0x4dfb50` copies the
    // selection, which there is a quest id (`0x4def30` stores it at `0x4def5d`); ours is a row
    // index, so the id is resolved here, at the click, before the rows can move.
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
    // The marked quest's title, "" for none; the reference (`0x4dfb60`) reads the quest cache by
    // id, the source of the rows' titles too.
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
    // Not built: nil, so the popup never lists the quest items an abandon destroys.
    g.set(
        "GetAbandonQuestItems",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;
    // The popup's Yes (`0x4dfe00` to `0x4df070`): the reference searches every row, folded ones
    // included (`0xbb71c0`), for the marked id; a hit sends and clears the mark, a miss keeps it.
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

    // One return per live timer, in log order, re-read every OnUpdate: `QuestTimerFrame_Update`
    // takes them as varargs and hides on none (`QuestTimerFrame.lua:11`, `:26`, `:37`).
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

    // A timer's 1-based row, headers counted, for the timer buttons' click and tooltip
    // (`QuestTimerFrame.lua:43`, `QuestTimerFrame.xml:20`).
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

    // The selection's seconds left, nil without a timer, which hides the stock "Time Remaining:"
    // row (`QuestLogFrame.lua:364`).
    g.set(
        "GetQuestLogTimeLeft",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let left = (model.quest_log_selection as usize)
                .checked_sub(1)
                .and_then(|n| model.quest_log.entries.get(n))
                .and_then(|e| seconds_left(e, model.server_unix_time));
            Ok(match left {
                // Clamped where the lists drop the row, as in the reference.
                Some(s) => Value::Integer(s.max(0)),
                None => Value::Nil,
            })
        })?,
    )?;

    // ── Reward spell and party ───────────────────────────────────────────────────────────────────
    // `texture, name, isTradeskillSpell` (`0x4e1130`), three nils without a reward spell.
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
    // ── The party share and the quest watch ──────────────────────────────────────────────────────
    /// The selected row, which the share verbs read.
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
    // The share button enables on `GetQuestLogPushable() and GetNumPartyMembers() > 0`
    // (`QuestLogFrame.lua:301`) and clicks `QuestLogPushQuest()` (`QuestLogFrame.xml:512`), both
    // about the selection. `GetQuestLogPushable` answers 1 or nil, never false (`0x4e12b0`).
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
            // The verb checks the party and the sharable bit itself (`0x4e13a6`), as a macro can
            // call it solo. The id is resolved now; a header's `quest_id` is 0 and never queues.
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
    // The watch tracker's set (`QuestLogFrame.lua:469`, `:613`), keyed by quest id. At most
    // `MAX_WATCHABLE_QUESTS`, 5 (`QuestLogFrame.lua:8`), which the stock Lua checks before adding
    // (`:494`); an add past it is a no-op.
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
    // The watched quest's current 1-based row, nil when it is not a visible row.
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

    /// The shape of `0x4df930`.
    #[test]
    fn out_of_range_is_six_values_with_a_zero_level_and_a_bad_arg_raises() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetQuestLogTitle(1)").unwrap(), 6);
        assert!(s
            .eval::<bool>("local t, l = GetQuestLogTitle(1) return t == nil and l == 0")
            .unwrap());

        s.set_quest_log(two_quests());
        for i in ["0", "-1", "99"] {
            assert!(
                s.eval::<bool>(&format!(
                    "local t, l = GetQuestLogTitle({i}) return t == nil and l == 0"
                ))
                .unwrap(),
                "index {i} is out of range, not an error"
            );
        }
        // `_ftol` truncates toward zero: 1.9 is entry 1.
        assert!(s
            .eval::<bool>("return GetQuestLogTitle(1.9) == 'A Threat Within'")
            .unwrap());

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

        assert!(s
            .eval::<bool>(
                "local _, l, _, h, c = GetQuestLogTitle(1) return h == 1 and c == 1 and l == 0"
            )
            .unwrap());
        assert!(s
            .eval::<bool>("local _, _, _, h, c = GetQuestLogTitle(2) return h == 1 and c == nil")
            .unwrap());
        assert!(s
            .eval::<bool>(
                "local _, _, _, h, c = GetQuestLogTitle(3)\n\
                 return h == nil and c == nil and type(h) ~= 'boolean'"
            )
            .unwrap());
    }

    #[test]
    fn tag_is_the_bare_word_and_nil_when_absent() {
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
        assert_eq!(
            s.eval::<i64>("SelectQuestLogEntry(2); return GetQuestLogSelection()")
                .unwrap(),
            2
        );
        assert_eq!(s.quest_log_selection(), 2);
    }

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
        // The mark is the quest id (`0x4dfb50`), not the selection: entry 2 is quest 7.
        s.run("SelectQuestLogEntry(1); AbandonQuest()").unwrap();
        assert_eq!(s.take_quest_log_abandons(), vec![7]);
        assert!(s.take_quest_log_abandons().is_empty(), "drained");
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
        s.run("AddQuestWatch(2)").unwrap(); // quest 7
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 1);
        assert!(s.eval::<bool>("return IsQuestWatched(2)").unwrap());
        assert!(!s.eval::<bool>("return IsQuestWatched(1)").unwrap());
        assert_eq!(s.eval::<i64>("return GetQuestIndexForWatch(1)").unwrap(), 2);
        assert_eq!(s.quest_log_watched(), vec![7]);

        // Quest 783 leaves and quest 7 moves to row 1, keeping its watch.
        let mut shuffled = two_quests();
        shuffled.entries.remove(0);
        s.set_quest_log(shuffled);
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 1);
        assert!(s.eval::<bool>("return IsQuestWatched(1)").unwrap());
        assert_eq!(s.eval::<i64>("return GetQuestIndexForWatch(1)").unwrap(), 1);

        s.run("RemoveQuestWatch(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 0);
        s.run("AddQuestWatch(1)").unwrap();
        s.set_quest_log(QuestLogState::default());
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 0);
    }

    /// The prune (`0x4de7a7` to `0x4de80f`) counts folded quests; a collapse never prunes.
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

        // Collapsed: only the header is visible.
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

        // Quest 7 leaves the log, neither visible nor folded.
        let mut gone = expanded();
        gone.entries.retain(|e| e.quest_id != 7);
        gone.num_quests = 1;
        s.set_quest_log(gone);
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 0);
    }

    /// Every value carries the reference's `-1`: 500 seconds to the deadline reads 499.
    #[test]
    fn timers_count_down_against_the_pushed_server_clock() {
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        state.entries[1].timer = 1_000_900;
        s.set_quest_log(state);

        // No server clock yet.
        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetQuestLogTimeLeft() == nil")
            .unwrap());

        s.set_server_unix_time(1_000_400.0);
        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return (GetQuestTimers())").unwrap(), 499);
        assert_eq!(s.eval::<i64>("return GetQuestIndexForTimer(1)").unwrap(), 2);
        assert!(s
            .eval::<bool>("return GetQuestIndexForTimer(2) == nil")
            .unwrap());

        s.run("SelectQuestLogEntry(1)").unwrap();
        assert!(s
            .eval::<bool>("return GetQuestLogTimeLeft() == nil")
            .unwrap());
        s.run("SelectQuestLogEntry(2)").unwrap();
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 499);

        // Only the clock moves, and the number falls.
        s.set_server_unix_time(1_000_880.0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 19);

        // The last second reads 0.
        s.set_server_unix_time(1_000_899.0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 0);

        // Past the deadline the lists drop the row and `GetQuestLogTimeLeft` clamps to 0.
        s.set_server_unix_time(1_000_910.0);
        assert_eq!(s.eval::<i64>("return GetQuestLogTimeLeft()").unwrap(), 0);
        assert_eq!(s.arity("GetQuestTimers()").unwrap(), 0);
        assert!(s
            .eval::<bool>("return GetQuestIndexForTimer(1) == nil")
            .unwrap());
    }

    /// vmangos sets a failed quest's timer to 1 and leaves it in the log (`Player::FailQuest`).
    #[test]
    fn a_failed_timed_quest_drops_out_of_the_timer_list() {
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        state.entries[0].timer = 1_000_900;
        state.entries[1].timer = 1; // failed: the server's sentinel
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
        // `MAX_WATCHABLE_QUESTS` is 5 (`QuestLogFrame.lua:8`).
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 5);
        s.run("AddQuestWatch(1)").unwrap();
        assert_eq!(s.eval::<i64>("return GetNumQuestWatches()").unwrap(), 5);
        assert_eq!(s.quest_log_watched(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn pushable_follows_the_selection() {
        let mut s = UiScript::new().unwrap();
        let mut state = two_quests();
        state.entries[0].pushable = true; // 783 is sharable, 7 is not
        s.set_quest_log(state);

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

    /// A miss keeps the mark, as `0x4df070` returns before the clear at `0x4df0cf`.
    #[test]
    fn abandon_mark_is_a_quest_id_that_survives_a_reindex() {
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(two_quests());
        s.run("SelectQuestLogEntry(2); SetAbandonQuest()").unwrap(); // quest 7

        // Quest 7 moves to row 1 and a stranger takes row 2.
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

        // Gone from the log: nothing queues, and the mark stays.
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

    /// A party of one other, so `QuestLogPushQuest`'s party check passes.
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

    #[test]
    fn push_without_a_selection_queues_nothing() {
        let mut s = UiScript::new().unwrap();
        s.set_quest_log(two_quests());
        in_a_party(&mut s);
        s.run("QuestLogPushQuest()").unwrap();
        assert!(s.take_quest_log_pushes().is_empty());
    }

    /// `0x4e13a6` checks the party and the sharable bit itself; each guard is tested alone.
    #[test]
    fn push_refuses_solo_and_refuses_an_unsharable_quest() {
        let sharable = || {
            let mut state = two_quests();
            state.entries[0].pushable = true; // quest id 783
            state
        };

        let mut s = UiScript::new().unwrap();
        s.set_quest_log(sharable());
        s.run("SelectQuestLogEntry(1)").unwrap();
        s.run("QuestLogPushQuest()").unwrap();
        assert!(s.take_quest_log_pushes().is_empty(), "solo pushes nothing");

        in_a_party(&mut s);
        s.run("QuestLogPushQuest()").unwrap();
        assert_eq!(s.take_quest_log_pushes(), vec![783]);

        s.run("SelectQuestLogEntry(2)").unwrap();
        s.run("QuestLogPushQuest()").unwrap();
        assert!(
            s.take_quest_log_pushes().is_empty(),
            "an unsharable quest pushes nothing even in a party"
        );
    }

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
