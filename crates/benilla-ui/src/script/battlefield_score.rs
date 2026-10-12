//! The battleground scoreboard verbs of `WorldStateFrame.lua`, over a board the app pushes with
//! every name resolved, as the reference waits for the last name (`0x4aa580`); until then the
//! board's rebuild only stores a filter. The filter sorts its team first rather than compacting,
//! so an index past the filtered count answers a real row.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;
use super::Model;

/// One scoreboard row as the app resolved it, in wire order.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldScoreRow {
    /// The bare name, or `Name-Realm` for a cross-realm player (the reference's `%s-%s`).
    pub name: String,
    pub killing_blows: u32,
    pub honorable_kills: u32,
    pub deaths: u32,
    pub honor_gained: u32,
    /// `0` Horde, `1` Alliance, `-1` neither, derived from the race, never sent on the wire.
    pub faction: i32,
    pub rank: i32,
    /// The race's localized name, `None` for an id the tables do not carry (a nil return).
    pub race: Option<String>,
    pub class: Option<String>,
    /// The extra-stat dwords, eight slots (the client's block), unfilled ones zero.
    pub stats: [u32; 8],
}

/// A column header: a `WorldStateUI.dbc` `Type == 2` row for the map, its text not expanded.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldStatColumn {
    pub text: String,
    pub icon: String,
    pub tooltip: String,
}

/// The whole board the app pushes.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldScores {
    pub rows: Vec<BattlefieldScoreRow>,
    /// The `MSG_PVP_LOG_DATA` "ended" byte; gates the winner and `LeaveBattlefield`.
    pub ended: bool,
    /// `0` Horde, `1` Alliance; read only when `ended`.
    pub winner: u8,
    pub columns: Vec<BattlefieldStatColumn>,
}

/// The VM's half of the board: the pushed rows, the filter and the sorted order into `rows`.
#[derive(Clone, Debug)]
pub(crate) struct ScoreBoard {
    pub(crate) scores: BattlefieldScores,
    /// `SetBattlefieldScoreFaction`'s store: `-1` every team (the reset value), `0`, `1`.
    pub(crate) filter: i32,
    pub(crate) order: Vec<usize>,
    /// The filtered count (`0xb6ebc4`).
    pub(crate) filtered: usize,
    /// A newer board's names are still being queried (`[0xb6ebd4]` non-zero): the app holds that
    /// board back, and the rebuild waits for its last name.
    pub(crate) names_pending: bool,
}

impl Default for ScoreBoard {
    /// The filter starts at `-1`, the value the status-3 arm resets it to (`0x4aa5a0`).
    fn default() -> Self {
        Self {
            scores: BattlefieldScores::default(),
            filter: -1,
            order: Vec::new(),
            filtered: 0,
            names_pending: false,
        }
    }
}

impl ScoreBoard {
    /// The reference's recount and sort (`0x4aa200`).
    fn rebuild(&mut self) {
        let rows = &self.scores.rows;
        let filter = self.filter;
        self.filtered = if filter == -1 {
            rows.len()
        } else {
            rows.iter().filter(|r| r.faction == filter).count()
        };
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| {
            let (ra, rb) = (&rows[a], &rows[b]);
            if filter != -1 && ra.faction != rb.faction {
                return (rb.faction == filter).cmp(&(ra.faction == filter));
            }
            rb.killing_blows
                .cmp(&ra.killing_blows)
                .then(ra.deaths.cmp(&rb.deaths))
                .then(rb.honor_gained.cmp(&ra.honor_gained))
                .then(ra.name.cmp(&rb.name))
        });
        self.order = order;
    }

    /// `0x4aa5a0`: store the filter, then the rebuild `0x4aa200`, which returns without rebuilding
    /// or signalling while names are pending (`0x4aa216`–`0x4aa224`); the last name's arrival
    /// rebuilds with the stored filter. Returns whether it rebuilt, when the caller signals
    /// `UPDATE_BATTLEFIELD_SCORE` (`0x4aa308`).
    fn set_filter(&mut self, filter: i32) -> bool {
        self.filter = filter;
        if self.names_pending {
            return false;
        }
        self.rebuild();
        true
    }

    fn row(&self, index_1based: i32) -> Option<&BattlefieldScoreRow> {
        let i = usize::try_from(index_1based).ok()?.checked_sub(1)?;
        // Bounded by the wire count (`0xb6ebc0`), not the filtered one.
        self.order.get(i).map(|&r| &self.scores.rows[r])
    }
}

impl super::UiScript {
    /// Push the resolved board; the filter survives. The app fires `UPDATE_BATTLEFIELD_SCORE`
    /// itself, keeping the reference's order against `UPDATE_BATTLEFIELD_STATUS` on the status-3
    /// message (`0x4aaa5a`/`0x4aab05`).
    pub fn set_battlefield_scores(&mut self, scores: BattlefieldScores) {
        let mut model = self.model_mut();
        model.battlefield_board.scores = scores;
        model.battlefield_board.names_pending = false;
        model.battlefield_board.rebuild();
    }

    /// Whether a newer board waits on names the app is still resolving (`[0xb6ebd4]`).
    pub fn set_battlefield_score_names_pending(&mut self, pending: bool) {
        self.model_mut().battlefield_board.names_pending = pending;
    }

    /// The status-3 arm's `0x4aa5a0(-1)` (`0x4aaa5a`): the filter back to every team, and the
    /// rebuild and `UPDATE_BATTLEFIELD_SCORE` at once unless names are pending.
    pub fn reset_battlefield_score_filter(&mut self) {
        let rebuilt = self.model_mut().battlefield_board.set_filter(-1);
        if rebuilt {
            self.fire_event("UPDATE_BATTLEFIELD_SCORE", Vec::new());
        }
    }

    /// `GetBattlefieldInstanceRunTime()`'s answer in ms, pushed each frame.
    pub fn set_battlefield_run_time_ms(&mut self, ms: u32) {
        self.model_mut().battlefield_run_time_ms = ms;
    }

    /// `RequestBattlefieldScoreData()` calls since the last drain.
    pub fn take_battlefield_score_requests(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().battlefield_score_requests)
    }

    /// `LeaveBattlefield()` calls that passed the "ended" gate; the app sends
    /// `CMSG_LEAVE_BATTLEFIELD` with the active slot's map.
    pub fn take_battlefield_leave_requests(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().battlefield_leave_requests)
    }
}

/// A Lua number or numeric string to `i32`, truncated as the client's `__ftol` does.
fn number_of(v: &Value) -> Option<i32> {
    match v {
        Value::Integer(i) => Some(*i as i32),
        Value::Number(n) => Some(n.trunc() as i64 as i32),
        Value::String(s) => s
            .to_str()
            .ok()
            .and_then(|t| t.trim().parse::<f64>().ok())
            .map(|n| n.trunc() as i64 as i32),
        _ => None,
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetNumBattlefieldScores",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.battlefield_board.filtered as i64)
        })?,
    )?;

    // Nine values on every leg, the fail leg included (`0x4ab9d0`).
    g.set(
        "GetBattlefieldScore",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldScore(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(r) = model.battlefield_board.row(index) else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            let s = |v: &Option<String>| -> mlua::Result<Value> {
                Ok(match v {
                    Some(t) => Value::String(lua.create_string(t)?),
                    None => Value::Nil,
                })
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&r.name)?),
                Value::Integer(i64::from(r.killing_blows)),
                Value::Integer(i64::from(r.honorable_kills)),
                Value::Integer(i64::from(r.deaths)),
                Value::Integer(i64::from(r.honor_gained)),
                Value::Integer(i64::from(r.faction)),
                Value::Integer(i64::from(r.rank)),
                s(&r.race)?,
                s(&r.class)?,
            ]))
        })?,
    )?;

    g.set(
        "GetBattlefieldWinner",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let b = &model.battlefield_board.scores;
            Ok(if b.ended {
                Value::Integer(i64::from(b.winner))
            } else {
                Value::Nil
            })
        })?,
    )?;

    // Never raises; a stored filter rebuilds the board and fires `UPDATE_BATTLEFIELD_SCORE` in
    // place (`0x4abc90` → `0x4aa5a0` → `0x4aa200`, the signal at `0x4aa308`), or, while names
    // are pending, is only stored.
    g.set(
        "SetBattlefieldScoreFaction",
        lua.create_function(|lua, faction: Option<Value>| {
            let f = faction.as_ref().and_then(number_of).unwrap_or(-1);
            if !(-1..=1).contains(&f) {
                return Ok(());
            }
            super::tick::verb(lua, |model| {
                if model.battlefield_board.set_filter(f) {
                    model.raise("UPDATE_BATTLEFIELD_SCORE", Vec::new());
                }
            });
            Ok(())
        })?,
    )?;

    g.set(
        "GetNumBattlefieldStats",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.battlefield_board.scores.columns.len() as i64)
        })?,
    )?;

    // A column the map lacks answers three nils: the reference looks up row id 0 and finds none.
    g.set(
        "GetBattlefieldStatInfo",
        lua.create_function(|lua, index: Value| {
            let index = number_arg(lua, index, "Usage: GetBattlefieldStatInfo(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let col = usize::try_from(index)
                .ok()
                .and_then(|i| i.checked_sub(1))
                .and_then(|i| model.battlefield_board.scores.columns.get(i));
            let Some(c) = col else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Nil,
                ]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&c.text)?),
                Value::String(lua.create_string(&c.icon)?),
                Value::String(lua.create_string(&c.tooltip)?),
            ]))
        })?,
    )?;

    // A bad row or a stat outside 1..=8 answers 0. Deviation: the reference (`0x4abdc0`) admits 9
    // and reads one dword past its block; we answer 0 because that read is out of bounds.
    g.set(
        "GetBattlefieldStatData",
        lua.create_function(|lua, (player, stat): (Value, Value)| {
            let usage = "Usage: GetBattlefieldStatData(playerIndex, statIndex)";
            let player = number_arg(lua, player, usage)?;
            let stat = number_arg(lua, stat, usage)?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let v = model
                .battlefield_board
                .row(player)
                .zip(usize::try_from(stat).ok().and_then(|s| s.checked_sub(1)))
                .and_then(|(r, s)| r.stats.get(s).copied())
                .unwrap_or(0);
            Ok(i64::from(v))
        })?,
    )?;

    // The app sends an empty `MSG_PVP_LOG_DATA` under the client's 5000 ms throttle (`0x4aa170`).
    g.set(
        "RequestBattlefieldScoreData",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.battlefield_score_requests += 1;
            Ok(())
        })?,
    )?;

    // Nothing at all until the scoreboard's "ended" byte has arrived (`0x4abe66`).
    g.set(
        "LeaveBattlefield",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.battlefield_board.scores.ended {
                model.battlefield_leave_requests += 1;
            }
            Ok(())
        })?,
    )?;

    // Ms since the status-3 stamp (`now - [0xb6ebbc]`, no sign guard, 0 with no stamp).
    g.set(
        "GetBattlefieldInstanceRunTime",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(i64::from(model.battlefield_run_time_ms))
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn row(name: &str, faction: i32, kb: u32, deaths: u32, honor: u32) -> BattlefieldScoreRow {
        BattlefieldScoreRow {
            name: name.into(),
            killing_blows: kb,
            deaths,
            honor_gained: honor,
            faction,
            rank: 3,
            race: Some("Orc".into()),
            class: Some("Warrior".into()),
            stats: [1, 2, 0, 0, 0, 0, 0, 0],
            ..Default::default()
        }
    }

    fn board() -> UiScript {
        let mut s = UiScript::new().unwrap();
        s.set_battlefield_scores(BattlefieldScores {
            rows: vec![
                row("Bob", 0, 3, 1, 50),
                row("Alice", 1, 5, 2, 10),
                row("Carl", 1, 5, 1, 10),
                row("Dana", 0, 5, 1, 20),
            ],
            ended: false,
            winner: 0,
            columns: vec![BattlefieldStatColumn {
                text: "Flags Captured".into(),
                icon: "Interface\\PVPFrame\\PVP-ArenaPoints-Icon".into(),
                tooltip: "Flags captured".into(),
            }],
        });
        s
    }

    /// The order of `0x4aa350`; a filter shrinks the count, not the array.
    #[test]
    fn the_board_sorts_like_the_client_and_the_filter_is_a_sort() {
        let s = board();
        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldScores()").unwrap(),
            4
        );
        let names = |s: &UiScript| -> Vec<String> {
            (1..=4)
                .map(|i| {
                    s.eval::<String>(&format!("return (GetBattlefieldScore({i}))"))
                        .unwrap()
                })
                .collect()
        };
        assert_eq!(names(&s), ["Dana", "Carl", "Alice", "Bob"]);
        s.run("SetBattlefieldScoreFaction(1)").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldScores()").unwrap(),
            2
        );
        assert_eq!(
            names(&s),
            ["Carl", "Alice", "Dana", "Bob"],
            "the filter's team first, the rest still reachable"
        );
        s.run("SetBattlefieldScoreFaction(7) SetBattlefieldScoreFaction(\"x\")")
            .unwrap();
        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldScores()").unwrap(),
            4,
            "7 is a no-op; a non-number is -1"
        );
        s.run("SetBattlefieldScoreFaction()").unwrap();
        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldScores()").unwrap(),
            4
        );
    }

    /// `0x4aa216`: while names are pending the verb only stores the filter, and the board the
    /// last name completes is sorted by it; otherwise the handler runs before the verb returns.
    #[test]
    fn the_faction_verb_waits_on_pending_names() {
        let mut s = board();
        crate::script::tick::heard::listen(
            &s,
            &["UPDATE_BATTLEFIELD_SCORE"],
            Some("GetNumBattlefieldScores()"),
        );
        s.run("SetBattlefieldScoreFaction(1)").unwrap();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            ["UPDATE_BATTLEFIELD_SCORE 2"]
        );

        s.set_battlefield_score_names_pending(true);
        s.run("SetBattlefieldScoreFaction(0)").unwrap();
        assert!(crate::script::tick::heard::take(&s).is_empty());
        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldScores()").unwrap(),
            2,
            "no rebuild: the count is the last one's"
        );
        s.reset_battlefield_score_filter();
        assert!(
            crate::script::tick::heard::take(&s).is_empty(),
            "the status-3 reset waits too"
        );
        s.run("SetBattlefieldScoreFaction(0)").unwrap();

        s.set_battlefield_scores(BattlefieldScores {
            rows: vec![row("Eve", 1, 1, 1, 1), row("Finn", 0, 1, 1, 1)],
            ..Default::default()
        });
        assert_eq!(
            s.eval::<i64>("return GetNumBattlefieldScores()").unwrap(),
            1,
            "the stored filter sorts the completed board"
        );
        assert_eq!(
            s.eval::<String>("return (GetBattlefieldScore(1))").unwrap(),
            "Finn"
        );
        s.reset_battlefield_score_filter();
        assert_eq!(
            crate::script::tick::heard::take(&s),
            ["UPDATE_BATTLEFIELD_SCORE 2"],
            "the status-3 arm resets to every team and rebuilds at once"
        );
    }

    #[test]
    fn the_getters_answer_the_clients_shapes() {
        let mut s = board();
        assert!(
            s.eval::<bool>("local n, kb, hk, d, h, f, r, race, class = GetBattlefieldScore(1) return n == \"Dana\" and kb == 5 and d == 1 and h == 20 and f == 0 and r == 3 and race == \"Orc\" and class == \"Warrior\"")
                .unwrap()
        );
        assert!(
            s.eval::<bool>("local n, kb, hk, d, h, f, r, race, class = GetBattlefieldScore(9) return n == nil and kb == 0 and f == 0 and race == nil and class == nil")
                .unwrap(),
            "past the wire count: nil, six zeros, nil, nil"
        );
        for bad in [
            "GetBattlefieldScore(nil)",
            "GetBattlefieldStatInfo(\"x\")",
            "GetBattlefieldStatData(1)",
            "GetBattlefieldStatData(nil, 1)",
        ] {
            assert!(
                s.run(bad).expect_err(bad).to_string().contains("Usage: "),
                "{bad}"
            );
        }
        assert_eq!(s.eval::<i64>("return GetNumBattlefieldStats()").unwrap(), 1);
        assert_eq!(
            s.eval::<(String, String, String)>("return GetBattlefieldStatInfo(1)")
                .unwrap()
                .0,
            "Flags Captured"
        );
        assert!(s.eval::<bool>("local t, i, tt = GetBattlefieldStatInfo(2) return t == nil and i == nil and tt == nil").unwrap());
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldStatData(1, 2)")
                .unwrap(),
            2
        );
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldStatData(1, 9)")
                .unwrap(),
            0
        );
        assert!(s
            .eval::<bool>("return GetBattlefieldWinner() == nil")
            .unwrap());
        s.run("LeaveBattlefield()").unwrap();
        assert_eq!(
            s.take_battlefield_leave_requests(),
            0,
            "not ended: LeaveBattlefield sends nothing"
        );
        s.set_battlefield_scores(BattlefieldScores {
            ended: true,
            winner: 1,
            ..Default::default()
        });
        assert_eq!(s.eval::<i64>("return GetBattlefieldWinner()").unwrap(), 1);
        s.run("LeaveBattlefield() RequestBattlefieldScoreData()")
            .unwrap();
        assert_eq!(s.take_battlefield_leave_requests(), 1);
        assert_eq!(s.take_battlefield_score_requests(), 1);
        s.set_battlefield_run_time_ms(65000);
        assert_eq!(
            s.eval::<i64>("return GetBattlefieldInstanceRunTime()")
                .unwrap(),
            65000
        );
    }
}
