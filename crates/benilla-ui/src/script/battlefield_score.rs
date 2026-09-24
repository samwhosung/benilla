//! The battleground **scoreboard** family: the eleven
//! verbs the stock `WorldStateFrame.lua` score frame calls, over a board the app pushes.
//!
//! ## What is the app's and what is the VM's
//!
//! The wire (`MSG_PVP_LOG_DATA`) carries rows in wire order with GUIDs; the reference resolves
//! every name through its name cache first (`0x4aa580` — the board is not rebuilt, and the event
//! not fired, until the last name arrives) and derives each row's **team** from the resolved race
//! (`0` Horde, `1` Alliance, `-1` neither). That resolution is the app's — it owns the name
//! cache and the race tables — so the app pushes [`BattlefieldScores`] with names, teams, race and
//! class strings already in place. The VM owns what the reference's `0x4aa200` rebuild owns: the
//! faction filter, the sort, the filtered count, and the getters' shapes.
//!
//! ## The sort, and the filter that is a sort (`0x4aa350`, `0x4aa200`)
//!
//! Rows are ordered by: the filter's team first (only when a filter is set), then killing blows
//! descending, deaths ascending, honor gained descending, name ascending. `GetNumBattlefieldScores`
//! is the **filtered** count; `GetBattlefieldScore(index)` and `GetBattlefieldStatData` are bounded
//! by the **wire** count — an index past the filtered count still answers a real row of the other
//! team, exactly as the reference does, because the filter sorts rather than compacts.
//!
//! Every raise is the reference's own `Usage:`; `SetBattlefieldScoreFaction` never raises.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::number_arg;
use super::Model;

/// One scoreboard row as the app resolved it (wire order, see the module doc).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldScoreRow {
    /// The bare name, or `Name-Realm` for a cross-realm player (the reference's `%s-%s`).
    pub name: String,
    pub killing_blows: u32,
    pub honorable_kills: u32,
    pub deaths: u32,
    pub honor_gained: u32,
    /// `0` Horde, `1` Alliance, `-1` neither — client-derived from the race, never wire data.
    pub faction: i32,
    pub rank: i32,
    /// The race's localized name, `None` for an id the tables do not carry (a nil return).
    pub race: Option<String>,
    /// The class's localized name, likewise.
    pub class: Option<String>,
    /// The extra-stat dwords, eight slots (the client's block), unfilled ones zero.
    pub stats: [u32; 8],
}

/// One scoreboard column header — a `WorldStateUI.dbc` `Type == 2` row for the map
/// (`GetBattlefieldStatInfo`): text raw (no macro expansion), icon, tooltip.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldStatColumn {
    pub text: String,
    pub icon: String,
    pub tooltip: String,
}

/// The whole board the app pushes ([`super::UiScript::set_battlefield_scores`]).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BattlefieldScores {
    pub rows: Vec<BattlefieldScoreRow>,
    /// The `MSG_PVP_LOG_DATA` "ended" byte — gates `GetBattlefieldWinner` and `LeaveBattlefield`.
    pub ended: bool,
    /// `0` Horde, `1` Alliance; read only when `ended`.
    pub winner: u8,
    pub columns: Vec<BattlefieldStatColumn>,
}

/// The VM's half of the board: the pushed rows, the filter, and the order the filter and sort
/// produce (indices into `rows`).
#[derive(Clone, Debug)]
pub(crate) struct ScoreBoard {
    pub(crate) scores: BattlefieldScores,
    /// `SetBattlefieldScoreFaction`'s store: `-1` every team (the reset value), `0`, `1`.
    pub(crate) filter: i32,
    pub(crate) order: Vec<usize>,
    /// The filtered count (`0xb6ebc4`) — the rows whose team equals the filter, or all with `-1`.
    pub(crate) filtered: usize,
}

impl Default for ScoreBoard {
    /// The filter starts at `-1`, every team — the value the status-3 arm resets it to
    /// (`or ecx,-1; call 0x4aa5a0`).
    fn default() -> Self {
        Self {
            scores: BattlefieldScores::default(),
            filter: -1,
            order: Vec::new(),
            filtered: 0,
        }
    }
}

impl ScoreBoard {
    /// `0x4aa200`'s recount and sort, over the pushed rows.
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
                // The selected team's row appears first, with no score comparison at all.
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

    fn row(&self, index_1based: i32) -> Option<&BattlefieldScoreRow> {
        let i = usize::try_from(index_1based).ok()?.checked_sub(1)?;
        // Bounded by the WIRE count (`0xb6ebc0`), over the sorted order.
        self.order.get(i).map(|&r| &self.scores.rows[r])
    }
}

impl super::UiScript {
    /// Push the resolved board. The filter is the VM's and survives a push; the order is rebuilt.
    /// The app fires `UPDATE_BATTLEFIELD_SCORE` itself after this, so the reference's ordering
    /// against `UPDATE_BATTLEFIELD_STATUS` on the status-3 message holds (`0x4aaa5a`/`0x4aab05`).
    pub fn set_battlefield_scores(&mut self, scores: BattlefieldScores) {
        let mut model = self.model_mut();
        model.battlefield_board.scores = scores;
        model.battlefield_board.rebuild();
    }

    /// `GetBattlefieldInstanceRunTime()`'s answer, in ms — the app's clock against the stamp the
    /// status-3 message set (`now − Δ₂`), pushed each frame.
    pub fn set_battlefield_run_time_ms(&mut self, ms: u32) {
        self.model_mut().battlefield_run_time_ms = ms;
    }

    /// `RequestBattlefieldScoreData()` calls since the last drain — the app throttles to 5000 ms
    /// and sends `MSG_PVP_LOG_DATA` empty.
    pub fn take_battlefield_score_requests(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().battlefield_score_requests)
    }

    /// `LeaveBattlefield()` calls that passed the "ended" gate — the app sends
    /// `CMSG_LEAVE_BATTLEFIELD` with the active slot's map.
    pub fn take_battlefield_leave_requests(&mut self) -> u32 {
        std::mem::take(&mut self.model_mut().battlefield_leave_requests)
    }
}

/// A Lua number-ish → i32 by the client's `__ftol` (truncate), `None` for a non-number.
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

    // `GetNumBattlefieldScores()` — the FILTERED count `[0xb6ebc4]`.
    g.set(
        "GetNumBattlefieldScores",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.battlefield_board.filtered as i64)
        })?,
    )?;

    // `GetBattlefieldScore(index)` — nine values on every leg (`0x4ab9d0`): name, killingBlows,
    // honorableKills, deaths, honorGained, faction, rank, race, class; the fail leg is
    // `nil, 0,0,0,0,0,0, nil, nil`. Bounded by the wire count over the sorted order.
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

    // `GetBattlefieldWinner()` — nil until the "ended" byte, then the winner (0 Horde, 1 Alliance).
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

    // `SetBattlefieldScoreFaction([faction])` — never raises: a non-number means -1; only
    // {-1, 0, 1} store, and a stored value recounts, re-sorts and fires UPDATE_BATTLEFIELD_SCORE
    // (`0x4abc90`) — here on the deferred lane, the next dispatch.
    g.set(
        "SetBattlefieldScoreFaction",
        lua.create_function(|lua, faction: Option<Value>| {
            let f = faction.as_ref().and_then(number_of).unwrap_or(-1);
            if !(-1..=1).contains(&f) {
                return Ok(());
            }
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.battlefield_board.filter = f;
            model.battlefield_board.rebuild();
            model
                .pending_events
                .push(("UPDATE_BATTLEFIELD_SCORE".to_string(), Vec::new()));
            Ok(())
        })?,
    )?;

    // `GetNumBattlefieldStats()` — the column count, no gate.
    g.set(
        "GetNumBattlefieldStats",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.battlefield_board.scores.columns.len() as i64)
        })?,
    )?;

    // `GetBattlefieldStatInfo(index)` — three values: text (raw), icon, tooltip; three nils
    // for a column the map has not (the reference falls into its lookup with row id 0 and
    // finds no row).
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

    // `GetBattlefieldStatData(playerIndex, statIndex)` — both required numbers; one number on
    // every leg: the row's stat, or 0 for a bad row or a stat index outside 1..=8 (the reference
    // (`0x4abdc0`) admits 9 and reads one dword past its block — ours answers 0 there).
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

    // `RequestBattlefieldScoreData()` — 0 args, 0 returns; the app sends `MSG_PVP_LOG_DATA` empty
    // under the client's 5000 ms throttle (`0x4aa170`).
    g.set(
        "RequestBattlefieldScoreData",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.battlefield_score_requests += 1;
            Ok(())
        })?,
    )?;

    // `LeaveBattlefield()` — a gate, then a payload (`0x4abe66`): nothing at all until the
    // scoreboard's
    // "ended" byte has arrived; then `CMSG_LEAVE_BATTLEFIELD` with the active slot's map (the
    // app's, which holds the queue).
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

    // `GetBattlefieldInstanceRunTime()` — 0 args, one number: ms since the instance's stamp
    // (`now − [0xb6ebbc]`, no sign guard; 0 with no stamp). The app keeps the clock.
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

    /// The sort law (`0x4aa350`): killing blows descending, deaths ascending, honor descending,
    /// name; a filter puts its team first and shrinks the count without compacting the array
    /// (`0xb6ebc4`/`0xb6ebc0`).
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

    /// The getters' shapes: nine values with the fail leg's nils and zeros, the raises, the
    /// column triple, the stat datum, the winner gate and the leave gate.
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
