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
    let wrong: Vec<String> = MISSES
        .iter()
        .filter_map(|(call, want)| {
            let got = s.eval::<String>(&format!("return PGShow({call})")).unwrap();
            (got != *want).then(|| format!("{call}: pushes ({got}), the reference ({want})"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
