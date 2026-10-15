//! The lottery and minigame globals the reference registers although its interface never opens
//! either window (`0x4c4140`..`0x4c4e04`; the tables `0x8481b8` and `0x84838c`). No 1.12 server
//! runs either, and benilla builds neither window: the lottery's prize, draw and kiosk globals and
//! the minigame singleton (`0xb71e5c`) are state nothing here fills, so each function answers what
//! the client answers with that state empty.
//!
//! The reference's getters push a number and then return 0 (`xor eax,eax`), so Lua receives no
//! value from them (`tables/bindings.tsv` flags them `stack-leak`); that is kept.
//!
//! Deviation: the three senders (`GetLastLotteryNumbers`, `BuyRandomPicks`, `SubmitNumbers`) check
//! their arguments and answer as the reference does but send nothing. Their `CMSG` (`0x334`,
//! `0x336`) belong to a system no server of 1.12 answers or accepts, and the wire for it is not
//! built.

use mlua::{Lua, MultiValue, Value, Variadic};

use super::binding_abi::{coerced_number, ftol, number_arg};

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

    // GetLastLotteryNumbers (`0x4c4220`): sends opcode `0x334` with no body (module doc).
    g.set(
        "GetLastLotteryNumbers",
        lua.create_function(|_, ()| Ok(()))?,
    )?;

    // SubmitNumbers(a, b, c, d) (`0x4c4290`): no argument gate, each read by `lua_tonumber` and
    // truncated (a missing or non-numeric one reads 0, so is out of range). Valid picks would send
    // `0x336` with the mask (module doc). Invalid ones send nothing and `return 1` without a push,
    // which hands Lua the stack's top: the last argument given, or with none the called function.
    g.set(
        "SubmitNumbers",
        lua.create_function(|lua, args: Variadic<Value>| {
            let pick = |i: usize| ftol(coerced_number(lua, args.get(i).cloned()));
            if pick_mask([pick(0), pick(1), pick(2), pick(3)]).is_some() {
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

    // BuyRandomPicks(n) (`0x4c4400`): `lua_tonumber` with no gate; a count from 1 to 10 would send
    // `0x336` with it (module doc), any other raises.
    g.set(
        "BuyRandomPicks",
        lua.create_function(|lua, n: Option<Value>| {
            if (1..=10).contains(&ftol(coerced_number(lua, n))) {
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

    // GetLotteryPrizeInfo(index) (`0x4c44e0`): an `is-number` gate and the n-th prize record
    // (`0x4c3f50`) both fail into the one error. The window holds no prizes, so every call does.
    g.set(
        "GetLotteryPrizeInfo",
        lua.create_function(|_, _index: Value| -> mlua::Result<()> {
            Err(mlua::Error::runtime(NO_PRIZE))
        })?,
    )?;

    // GetPastDrawResult(index) (`0x4c4690`): `lua_tonumber` with no gate, then the n-th used draw
    // record (`0x4c41a0`); there are none.
    g.set(
        "GetPastDrawResult",
        lua.create_function(|_, _index: Value| -> mlua::Result<()> {
            Err(mlua::Error::runtime(NO_DRAW))
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
    use super::pick_mask;
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

    /// The window holds no prize and no draw, so the readers' errors are the only answer, and the
    /// reference's pushing getters return no value at all.
    #[test]
    fn the_empty_lottery_answers_errors_and_no_values() {
        let s = UiScript::new().unwrap();
        for call in [
            "GetLotteryPrizeInfo(1)",
            "GetLotteryPrizeInfo(nil)",
            "GetLotteryPrizeInfo()",
        ] {
            raises(
                &s,
                call,
                r#"Invalid prize item in GetPrizeItemInfo("type", index)"#,
            );
        }
        for call in [
            "GetPastDrawResult(1)",
            "GetPastDrawResult(0)",
            "GetPastDrawResult()",
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
}
