//! An indexed getter asked for a row it does not have still pushes its full tuple, as the handler's
//! miss path does: a `0x6f37f0` nil or a `0x6f3810` number per slot, and the same count as a hit.
//! Each answer is read as `tostring` of every value joined by commas, counted with 5.0's `arg.n`,
//! so one string pins the count, the types and the values (`nil` or the number).

use super::common::script;

const SHOW: &str = "function PGShow(...) local o = {} \
     for i = 1, arg.n do local v = arg[i] if v == nil then o[i] = 'nil' else o[i] = tostring(v) end end \
     return table.concat(o, ',') end";

/// `(call, the tuple the reference pushes on a miss)`; the model is empty, so every call misses.
const MISSES: &[(&str, &str)] = &[
    // The resolver fails (`0x4f990d`) or finds no item: nothing at all, and three zeros for the
    // cooldown (`0x4f9810`, `0x4f99a5`, `0x4f9ae9`).
    ("GetContainerItemInfo(0, 1)", ""),
    ("GetContainerItemLink(0, 1)", ""),
    ("GetContainerItemCooldown(0, 1)", "0,0,0"),
    // `index - 1` compared unsigned (`0x4ceed9`, `0x4cf375`): the twelve-value miss (`0x4cf1ec`),
    // and nothing for a link (`0x4cf45b`).
    (
        "GetAuctionItemInfo('list', 1)",
        "nil,nil,1,-1,nil,0,0,0,0,0,nil,nil",
    ),
    ("GetAuctionItemLink('list', 1)", ""),
    // `index - 1` against 6, unsigned (`0x501924`); every miss is one nil (`0x50216a`).
    ("GetQuestItemLink('reward', 1)", "nil"),
    ("GetCraftInfo(1)", "nil,nil,nil,0,nil,0,0"),
    ("GetCraftInfo(0)", "nil,nil,nil,0,nil,0,0"),
    ("GetCraftReagentInfo(1, 1)", "nil,nil,nil,nil"),
    ("GetMacroInfo(1)", "nil,nil,nil,nil"),
    ("GetMapLandmarkInfo(1)", "nil,nil,0,0,0"),
    ("GetMapOverlayInfo(1)", "nil,0,0,0,0,0,0"),
    ("GetPetActionInfo(1)", "nil,nil,nil,nil,nil,nil,nil"),
    ("GetSavedInstanceInfo(1)", "nil,0,nil"),
    ("GetShapeshiftFormInfo(1)", "nil,nil,nil,nil"),
    ("GetTalentInfo(1, 1)", "nil,nil,1,1,0,0,nil,nil"),
    ("GetTalentTabInfo(1)", "nil,nil,0,nil"),
    ("GetTradePlayerItemInfo(1)", "nil,nil,0,0,nil"),
    ("GetTradeTargetItemInfo(1)", "nil,nil,0,0,nil,nil"),
    ("GetTradeSkillInfo(1)", "nil,nil,0,nil"),
    ("GetTradeSkillReagentInfo(1, 1)", "nil,nil,nil,nil"),
    ("GetTrainerServiceInfo(1)", "nil,nil,nil,1"),
    ("GetTrainerServiceInfo(0)", "nil,nil,nil,1"),
    ("GetTrainerServiceSkillReq(1)", "nil,0,1"),
    ("GetTrainerServiceAbilityReq(1, 1)", "nil,1"),
    ("GetTrainerServiceStepReq(1)", "nil,1"),
];

#[test]
fn an_indexed_getter_with_no_row_pushes_the_references_full_tuple() {
    let s = script();
    s.run(SHOW).unwrap();
    // A negative index truncates, decrements and compares unsigned (`0x4f6ed8`), so it falls out of
    // range and answers the same miss tuple, as does a fractional one; no getter raises (#1016).
    // `GetShapeshiftFormInfo` raises for a zero alone (`0x4b45d8`).
    let calls: Vec<(String, &str)> = MISSES
        .iter()
        .flat_map(|(call, want)| {
            ["(-1", "(-7.5", "(1e10"]
                .map(|first| (call.replacen("(1", first, 1), *want))
                .into_iter()
                .chain([(call.replacen(", 1)", ", -1)", 1), *want)])
                .chain([(call.to_string(), *want)])
        })
        .collect();
    let wrong: Vec<String> = calls
        .iter()
        .filter_map(|(call, want)| {
            let got = s
                .eval::<String>(&format!("return PGShow({call})"))
                .unwrap_or_else(|e| format!("raises: {e}"));
            (got != *want).then(|| format!("{call}: pushes ({got}), the reference ({want})"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // Zero, an absent index and a non-number raise the usage error where the reference does
    // (`0x4b45d8`, `0x4b49b4`); a negative or a fractional one falls out of range quietly.
    for name in ["GetShapeshiftFormInfo", "GetShapeshiftFormCooldown"] {
        for arg in ["0", "", "nil", "'x'"] {
            assert!(
                s.eval::<bool>(&format!(
                    "local ok, err = pcall({name}{}{arg}) \
                     return not ok and string.find(tostring(err), 'Usage: {name}(index)', 1, true) ~= nil",
                    if arg.is_empty() { "" } else { ", " }
                ))
                .unwrap(),
                "{name}({arg}) raises its usage"
            );
        }
        assert_eq!(
            s.eval::<i64>(&format!(
                "return (function(...) return arg.n end)({name}(-3))"
            ))
            .unwrap(),
            if name.ends_with("Info") { 4 } else { 3 },
            "{name}(-3) is a miss"
        );
        // 0.5 is not zero (`fcomp` against 0.0), so it reaches the table and truncates to a miss.
        assert_eq!(
            s.eval::<i64>(&format!(
                "return (function(...) return arg.n end)({name}(0.5))"
            ))
            .unwrap(),
            if name.ends_with("Info") { 4 } else { 3 },
        );
    }
    // A non-number raises the binding's own usage text (`0x84d860`, `0x84d88c`, `0x84d8b8`).
    for name in [
        "GetContainerItemInfo",
        "GetContainerItemLink",
        "GetContainerItemCooldown",
    ] {
        for call in [
            format!("{name}('x', 1)"),
            format!("{name}(0)"),
            format!("{name}(0, nil)"),
        ] {
            assert!(
                s.eval::<bool>(&format!(
                    "local ok, err = pcall(function() return {call} end) \
                     return not ok and string.find(tostring(err), 'Usage: {name}(index, slot)', 1, true) ~= nil"
                ))
                .unwrap(),
                "{call} raises its usage"
            );
        }
    }
}
