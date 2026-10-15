//! The lottery and minigame globals the reference registers although its interface never opens
//! either window (`0x4c4140`..`0x4c4e04`; the tables `0x8481b8` and `0x84838c`). No 1.12 server
//! runs either, and benilla builds neither window: the lottery's prize, draw and kiosk globals and
//! the minigame singleton (`0xb71e5c`) are state nothing here fills, so each function answers what
//! the client answers with that state empty.
//!
//! The reference's getters push a number and then return 0 (`xor eax,eax`), so Lua receives no
//! value from them (`tables/bindings.tsv` flags them `stack-leak`); that is kept.
//!
//! The three senders (`GetLastLotteryNumbers`, `BuyRandomPicks`, `SubmitNumbers`) queue the send
//! the reference makes ([`UnservedSend`]) for the app to put on the wire: opcodes `0x334` and
//! `0x336`. Their bodies are the instructions' own (`0x4c4220`, `0x4c4290`, `0x4c4400`).

use mlua::{Lua, MultiValue, Value, Variadic};

use super::binding_abi::{coerced_number, ftol, number_arg};
use super::item_stats::MISSING_ITEM_ICON;
use super::Model;

/// A send made by a verb of a system no 1.12 server runs, drained by the app in call order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnservedSend {
    /// `CMSG_LOTTERY_QUERY_OBSOLETE`: `GetLastLotteryNumbers`.
    LotteryQuery,
    /// `CMSG_BUY_LOTTERY_TICKET_OBSOLETE` with its two dwords: `BuyRandomPicks` sends `(count, 0)`,
    /// `SubmitNumbers` `(0, mask)`.
    BuyLotteryTicket { first: u32, second: u32 },
    /// `CMSG_SKILL_BUY_STEP`: `BuySkillTier`, the skill line id.
    SkillBuyStep { skill_id: u32 },
    /// `CMSG_GMSURVEY_SUBMIT`: the survey id, the answered questions in slot order and the overall
    /// comment.
    GmSurveySubmit {
        survey: u32,
        answers: Vec<super::gm_survey::SurveyAnswerSend>,
        comment: String,
    },
}

impl super::UiScript {
    /// Drain the sends queued since the last call.
    pub fn take_unserved_sends(&mut self) -> Vec<UnservedSend> {
        std::mem::take(&mut self.model_mut().unserved_sends)
    }
}

/// Queue a send from a binding.
pub(super) fn queue(lua: &Lua, send: UnservedSend) {
    lua.app_data_mut::<Model>()
        .expect("model app_data")
        .unserved_sends
        .push(send);
}

/// `GetPrizeItemInfo`'s error (`0x848318`), raised for a prize the window holds no record of.
const NO_PRIZE: &str = r#"Invalid prize item in GetPrizeItemInfo("type", index)"#;
/// `GetPastDrawResult`'s error (`0x848350`), raised past the draw records.
const NO_DRAW: &str = r#"Invalid index in GetPastDrawResults("type", index)"#;

/// The lottery window's pick check (`0x4c42f0`..`0x4c4326`): four numbers, each 1 to 31 and none
/// repeated, as the one-bit-each mask `SubmitNumbers` sends; `None` for any other four.
fn pick_mask(picks: [i32; 4]) -> Option<u32> {
    let mut mask = 0u32;
    for pick in picks {
        if !(1..=31).contains(&pick) || mask & (1 << (pick - 1)) != 0 {
            return None;
        }
        mask |= 1 << (pick - 1);
    }
    Some(mask)
}

/// Register the lottery and minigame globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    // CloseLottery (`0x4c4210`): `xor eax,eax; ret`.
    g.set("CloseLottery", lua.create_function(|_, ()| Ok(()))?)?;

    // GetLastLotteryNumbers (`0x4c4220`): sends opcode `0x334` with no body.
    g.set(
        "GetLastLotteryNumbers",
        lua.create_function(|lua, ()| {
            queue(lua, UnservedSend::LotteryQuery);
            Ok(())
        })?,
    )?;

    // SubmitNumbers(a, b, c, d) (`0x4c4290`): no argument gate, each read by `lua_tonumber` and
    // truncated (a missing or non-numeric one reads 0, so is out of range). Valid picks send
    // `0x336` with `(0, mask)` (`0x4c4378`, `0x4c4385`). Invalid ones send nothing and `return 1` without a push,
    // which hands Lua the stack's top: the last argument given, or with none the called function.
    g.set(
        "SubmitNumbers",
        lua.create_function(|lua, args: Variadic<Value>| {
            let pick = |i: usize| ftol(coerced_number(lua, args.get(i).cloned()));
            if let Some(mask) = pick_mask([pick(0), pick(1), pick(2), pick(3)]) {
                queue(
                    lua,
                    UnservedSend::BuyLotteryTicket {
                        first: 0,
                        second: mask,
                    },
                );
                return Ok(MultiValue::new());
            }
            let top = match args.last() {
                Some(v) => v.clone(),
                None => lua.globals().get("SubmitNumbers")?,
            };
            Ok(MultiValue::from_vec(vec![top]))
        })?,
    )?;

    // IsVendorActive (`0x4c43d0`) pushes a number and returns 0 values.
    g.set("IsVendorActive", lua.create_function(|_, ()| Ok(()))?)?;

    // BuyRandomPicks(n) (`0x4c4400`): `lua_tonumber` with no gate; a count from 1 to 10 sends
    // `0x336` with `(count, 0)` (`0x4c444e`, `0x4c4457`), any other raises.
    g.set(
        "BuyRandomPicks",
        lua.create_function(|lua, n: Option<Value>| {
            let n = ftol(coerced_number(lua, n));
            if (1..=10).contains(&n) {
                queue(
                    lua,
                    UnservedSend::BuyLotteryTicket {
                        first: n as u32,
                        second: 0,
                    },
                );
                Ok(())
            } else {
                Err(mlua::Error::runtime("Invalid number of randompicks"))
            }
        })?,
    )?;

    // The prize and draw readers: each pushes and returns 0 values (`0x4c44b0`, `0x4c45d0`,
    // `0x4c45f0`, `0x4c4630`, `0x4c4660`).
    for name in [
        "GetNumLotteryPrizes",
        "GetJackpotAmount",
        "GetMoneyPrizes",
        "GetNextDrawTime",
        "GetNumPastDrawResults",
    ] {
        g.set(name, lua.create_function(|_, ()| Ok(()))?)?;
    }

    // GetLotteryPrizeInfo(index) (`0x4c44e0`): an `is-number` gate fails into the error, and so
    // does the record lookup `0x4c3f50` for any `index - 1` above 4 or, with the window holding no
    // prizes, above 0. Its scan counts the nonzero prize ids so far and keeps the last slot where
    // the count equals `n` (`0x4c3fb0`..`0x4c3fc5`): with every id 0 the count is 0 at each slot,
    // so `n == 0` finds slot 3, id 0. The item cache answers null for id 0 without a query
    // (`0x55ba3d`), which leaves the name empty and the usable flag at its initial 1; the icon is
    // the display-0 icon (`0x5d88b0`, no `ItemDisplayInfo` row 0), the missing-item icon; the
    // count is the initial 1 and the quality 0 (`0x4c3f84`..`0x4c3f90`).
    g.set(
        "GetLotteryPrizeInfo",
        lua.create_function(|lua, index: Value| {
            let n = number_arg(lua, index, NO_PRIZE)?.wrapping_sub(1) as u32;
            if n != 0 {
                return Err(mlua::Error::runtime(NO_PRIZE));
            }
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string("")?),
                Value::String(lua.create_string(MISSING_ITEM_ICON)?),
                Value::Integer(1),
                Value::Integer(0),
                Value::Integer(1),
            ]))
        })?,
    )?;

    // GetPastDrawResult(index) (`0x4c4690`): `lua_tonumber` with no gate, then the n-th used draw
    // record (`0x4c41a0`). Its scan bumps the used count before comparing it with `n`
    // (`0x4c41b0`..`0x4c41b8`), so with every record unused the count is 0 and `n == 0` matches
    // the first record, empty: four zeros and the seconds since its stamp (`0x4c41ee`), a stamp
    // of 0. Any other `n` finds nothing.
    g.set(
        "GetPastDrawResult",
        lua.create_function(|lua, index: Option<Value>| {
            let n = ftol(coerced_number(lua, index)).wrapping_sub(1) as u32;
            if n != 0 {
                return Err(mlua::Error::runtime(NO_DRAW));
            }
            // `GetWallClockSeconds` (`0x429580`) less the stamp, pushed as a signed dword.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()) as u32 as i32;
            Ok(MultiValue::from_vec(vec![
                Value::Integer(0),
                Value::Integer(0),
                Value::Integer(0),
                Value::Integer(0),
                Value::Integer(i64::from(now)),
            ]))
        })?,
    )?;

    // GetMinigameType (`0x4c4d50`): the singleton's kind (`0xb71e5c`), 0 with no game, picks the
    // name table `0x848384` at `kind * 4`. That table starts at index 0 with "TicTacToe", so the
    // only kind that passes the gate, 1, reads the empty slot behind it: nil, one value, always.
    g.set(
        "GetMinigameType",
        lua.create_function(|_, ()| Ok(Value::Nil))?,
    )?;

    // MakeMinigameMove(moveType, param) (`0x4c4d80`): both `is-number`-gated, then handed to the
    // game singleton, which is absent: nothing.
    g.set(
        "MakeMinigameMove",
        lua.create_function(|lua, (move_type, param): (Value, Value)| {
            let usage = "Usage: MakeMinigameMove(moveType, param)";
            number_arg(lua, move_type, usage)?;
            number_arg(lua, param, usage)?;
            Ok(())
        })?,
    )?;

    // GetMinigameState (`0x4c4df0`): the singleton's virtual at `+8` decides the count; without a
    // game, `xor eax,eax`.
    g.set("GetMinigameState", lua.create_function(|_, ()| Ok(()))?)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{pick_mask, UnservedSend};
    use crate::script::UiScript;

    /// The raising calls, each with the exe's own text.
    fn raises(s: &UiScript, call: &str, text: &str) {
        let err = s.run(call).expect_err(call).to_string();
        assert!(err.contains(text), "{call}: {err}");
    }

    #[test]
    fn the_pick_mask_wants_four_distinct_numbers_from_one_to_thirty_one() {
        assert_eq!(
            pick_mask([1, 2, 3, 31]),
            Some(1 | 1 << 1 | 1 << 2 | 1 << 30)
        );
        assert_eq!(pick_mask([1, 1, 2, 3]), None, "a repeat");
        assert_eq!(pick_mask([1, 2, 3, 3]), None, "a repeat in the last pick");
        assert_eq!(pick_mask([0, 2, 3, 4]), None);
        assert_eq!(pick_mask([1, 2, 3, 32]), None);
        assert_eq!(pick_mask([-1, 2, 3, 4]), None);
    }

    /// `SubmitNumbers` returns nothing for four good picks and, for any other, `return 1` over a
    /// stack it pushed nothing to: the last argument it was given.
    #[test]
    fn submit_numbers_hands_back_the_last_argument_when_the_picks_are_bad() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("SubmitNumbers(1, 2, 3, 4)").unwrap(), 0);
        assert_eq!(s.arity("SubmitNumbers('1', '2', 3.9, 31)").unwrap(), 0);
        // A repeat: the fourth argument comes back, as itself.
        assert_eq!(
            s.eval::<i64>("return SubmitNumbers(1, 1, 2, 3)").unwrap(),
            3
        );
        assert_eq!(
            s.eval::<String>("return SubmitNumbers(1, 2, 3, 'x')")
                .unwrap(),
            "x"
        );
        // A fifth argument is not a pick: four good ones return nothing whatever follows.
        assert_eq!(s.arity("SubmitNumbers(1, 2, 3, 4, 99)").unwrap(), 0);
        // Fewer arguments: whatever was last. Out of range: the same.
        assert_eq!(s.eval::<i64>("return SubmitNumbers(7)").unwrap(), 7);
        assert_eq!(
            s.eval::<i64>("return SubmitNumbers(1, 2, 3, 32)").unwrap(),
            32
        );
        assert_eq!(
            s.eval::<i64>("return SubmitNumbers(1, 2, 3, 1, 99)")
                .unwrap(),
            99
        );
        assert!(s
            .eval::<bool>("return SubmitNumbers(1, 2, 3, nil) == nil")
            .unwrap());
        // No argument at all: the function itself.
        assert!(s
            .eval::<bool>("return SubmitNumbers() == SubmitNumbers")
            .unwrap());
        assert_eq!(s.arity("SubmitNumbers()").unwrap(), 1);
    }

    #[test]
    fn buy_random_picks_takes_one_to_ten_and_reads_no_type() {
        let s = UiScript::new().unwrap();
        for ok in ["1", "10", "'5'", "9.9"] {
            assert_eq!(
                s.arity(&format!("BuyRandomPicks({ok})")).unwrap(),
                0,
                "{ok}"
            );
        }
        for bad in ["0", "11", "-1", "nil", "'x'", "{}", ""] {
            raises(
                &s,
                &format!("BuyRandomPicks({bad})"),
                "Invalid number of randompicks",
            );
        }
    }

    /// With every prize id and draw record empty the readers' scans still match their first
    /// entry (`0x4c3fb0`, `0x4c41b0`): index 1 answers five values, any other raises. The
    /// reference's pushing getters return no value at all.
    #[test]
    fn the_empty_lottery_answers_its_first_slot_and_errors_elsewhere() {
        let s = UiScript::new().unwrap();
        let (name, icon, count, quality, usable): (String, String, i64, i64, i64) =
            s.eval("return GetLotteryPrizeInfo(1)").unwrap();
        assert_eq!(
            (name.as_str(), icon.as_str(), count, quality, usable),
            ("", "Interface\\Icons\\INV_Misc_QuestionMark", 1, 0, 1)
        );
        assert_eq!(s.arity("GetLotteryPrizeInfo('1')").unwrap(), 5);
        for call in [
            "GetLotteryPrizeInfo(2)",
            "GetLotteryPrizeInfo(0)",
            "GetLotteryPrizeInfo(5)",
            "GetLotteryPrizeInfo(nil)",
            "GetLotteryPrizeInfo()",
            "GetLotteryPrizeInfo('x')",
        ] {
            raises(
                &s,
                call,
                r#"Invalid prize item in GetPrizeItemInfo("type", index)"#,
            );
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let (a, b, c, d, since): (i64, i64, i64, i64, i64) =
            s.eval("return GetPastDrawResult(1)").unwrap();
        assert_eq!((a, b, c, d), (0, 0, 0, 0));
        assert!(
            (since - now).abs() <= 2,
            "the stamp is 0, so the clock: {since} vs {now}"
        );
        assert_eq!(s.arity("GetPastDrawResult(1.9)").unwrap(), 5);
        for call in [
            "GetPastDrawResult(2)",
            "GetPastDrawResult(0)",
            "GetPastDrawResult(-1)",
            "GetPastDrawResult()",
            "GetPastDrawResult('x')",
        ] {
            raises(
                &s,
                call,
                r#"Invalid index in GetPastDrawResults("type", index)"#,
            );
        }
        for call in [
            "CloseLottery()",
            "GetLastLotteryNumbers()",
            "IsVendorActive()",
            "GetNumLotteryPrizes()",
            "GetJackpotAmount()",
            "GetMoneyPrizes()",
            "GetNextDrawTime()",
            "GetNumPastDrawResults()",
            "GetMinigameState()",
        ] {
            assert_eq!(s.arity(call).unwrap(), 0, "{call}");
        }
    }

    /// The minigame: no game, so `GetMinigameType` answers its one nil, and a move needs two
    /// numbers before it finds there is nobody to send to.
    #[test]
    fn the_minigame_without_a_game() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.arity("GetMinigameType()").unwrap(), 1);
        assert!(s.eval::<bool>("return GetMinigameType() == nil").unwrap());
        assert_eq!(s.arity("MakeMinigameMove(1, 5)").unwrap(), 0);
        assert_eq!(s.arity("MakeMinigameMove('1', '5')").unwrap(), 0);
        let usage = "Usage: MakeMinigameMove(moveType, param)";
        for bad in ["", "1", "nil, 1", "1, nil", "'x', 1", "1, {}"] {
            raises(&s, &format!("MakeMinigameMove({bad})"), usage);
        }
    }

    /// Each sender queues its send exactly once, with the body's two dwords as the bytes build
    /// them; a refused call queues nothing.
    #[test]
    fn the_senders_queue_one_send_each_and_the_refused_none() {
        let mut s = UiScript::new().unwrap();
        s.run("GetLastLotteryNumbers()").unwrap();
        assert_eq!(s.take_unserved_sends(), [UnservedSend::LotteryQuery]);
        s.run("BuyRandomPicks(7)").unwrap();
        assert_eq!(
            s.take_unserved_sends(),
            [UnservedSend::BuyLotteryTicket {
                first: 7,
                second: 0
            }]
        );
        // The mask: bit n - 1 per pick (`0x4c431b`..`0x4c4323`).
        s.run("SubmitNumbers(1, 2, 31, 4)").unwrap();
        assert_eq!(
            s.take_unserved_sends(),
            [UnservedSend::BuyLotteryTicket {
                first: 0,
                second: 0x4000_000b
            }]
        );
        assert!(s.take_unserved_sends().is_empty(), "drained");
        // Refused: nothing queued.
        s.run("SubmitNumbers(1, 1, 2, 3) SubmitNumbers(0, 1, 2, 3) SubmitNumbers()")
            .unwrap();
        for bad in ["0", "11", "nil"] {
            let _ = s.run(&format!("BuyRandomPicks({bad})"));
        }
        assert!(s.take_unserved_sends().is_empty());
        // In call order.
        s.run("BuyRandomPicks(1) GetLastLotteryNumbers()").unwrap();
        assert_eq!(
            s.take_unserved_sends(),
            [
                UnservedSend::BuyLotteryTicket {
                    first: 1,
                    second: 0
                },
                UnservedSend::LotteryQuery
            ]
        );
    }
}
