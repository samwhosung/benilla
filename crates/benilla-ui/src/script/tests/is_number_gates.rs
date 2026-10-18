//! Bindings whose 1.12.1 body tests a number argument with `lua_isnumber` (`0x6f34d0`) and, on a
//! miss, pushes nil, keeps a default or skips; or reads it with a bare `lua_tonumber` (`0x6f3620`)
//! so a non-number is 0. None of them raises.

use super::common::script;
use crate::script::*;

/// A non-number in each gated slot raises nothing at every site; the reference's own fallback
/// follows (nil, the default, or the skip), and a numeric string still counts as a number.
#[test]
fn a_non_number_in_a_gated_slot_raises_nothing() {
    let mut s = script();
    s.set_taxi(Some(TaxiUiState {
        art: "Interface\\TaxiFrame\\TAXIMAP1".into(),
        nodes: vec![
            TaxiUiNode {
                name: "A".into(),
                node_type: TaxiNodeType::Current,
                pos: (0.5, 0.6),
                cost: 0,
                routes: vec![],
            },
            TaxiUiNode {
                name: "B".into(),
                node_type: TaxiNodeType::Reachable,
                pos: (0.55, 0.8),
                cost: 110,
                routes: vec![[0.5, 0.6, 0.55, 0.8]],
            },
        ],
    }));
    s.run(
        r#"
        Owner = CreateFrame("Frame", "GateOwner")
        TT = CreateFrame("GameTooltip", "GateTT", nil, "TestTooltipTemplate")
        EB = CreateFrame("EditBox", "GateEB")
        EB:SetText("hello")
    "#,
    )
    .unwrap();
    // `ActionHasRange` `0x4e7613`: nil.
    s.run(
        r##"
        for _, v in ipairs({ "x", true }) do
            assert(ActionHasRange(v) == nil)
        end
        assert(ActionHasRange(nil) == nil and ActionHasRange({}) == nil and ActionHasRange() == nil)
    "##,
    )
    .unwrap();
    // `PickupMerchantItem` `0x4fb7c4` takes the refusing leg; `SetMultisampleFormat` `0x48c659`
    // and `AddQuestWatch`-style gates are quiet.
    s.run(
        r#"
        PickupMerchantItem("x") PickupMerchantItem({}) PickupMerchantItem(nil) PickupMerchantItem()
        SetMultisampleFormat("x") SetMultisampleFormat({}) SetMultisampleFormat(nil)
    "#,
    )
    .unwrap();
    // `GetNumRoutes` reads a bare `lua_tonumber` (`0x4dcede`): a non-number is 0, no node.
    s.run(
        r#"
        assert(GetNumRoutes("x") == 0 and GetNumRoutes(nil) == 0 and GetNumRoutes({}) == 0)
        assert(GetNumRoutes(true) == 0 and GetNumRoutes() == 0)
        assert(GetNumRoutes("2") == 1 and GetNumRoutes(2.9) == 1)
    "#,
    )
    .unwrap();
    // `SetBagItem` `0x5346ff`, `0x53470f`: no value at all; `SetOwner`'s offsets `0x53134f`,
    // `0x531388`: 0; `HighlightText` `0x798907`, `0x79892e`: the defaults.
    s.run(
        r##"
        local function arg_count(...) return arg.n end
        TT:SetOwner(Owner, "ANCHOR_RIGHT")
        assert(arg_count(TT:SetBagItem(nil, 1)) == 0)
        assert(arg_count(TT:SetBagItem(0, "x")) == 0)
        assert(arg_count(TT:SetBagItem({}, {})) == 0)
        TT:SetOwner(Owner, "ANCHOR_CURSOR", nil, nil)
        TT:SetOwner(Owner, "ANCHOR_CURSOR", "x", {})
        TT:SetOwner(Owner, "ANCHOR_CURSOR", "3", 4)
        EB:HighlightText("x", {}) EB:HighlightText(nil, nil) EB:HighlightText({}, 2) EB:HighlightText("1", "3")
    "##,
    )
    .unwrap();
    // `SignPetition` `0x4f46e2`: the byte stays 1; a number or numeric string is `_ftol`ed.
    for call in [
        "SignPetition('x')",
        "SignPetition({})",
        "SignPetition(nil)",
        "SignPetition()",
    ] {
        s.run(call).unwrap();
        assert_eq!(
            s.take_petition_requests(),
            vec![PetitionRequest::Sign(1)],
            "{call}"
        );
    }
    s.run("SignPetition('5') SignPetition(2.9)").unwrap();
    assert_eq!(
        s.take_petition_requests(),
        vec![PetitionRequest::Sign(5), PetitionRequest::Sign(2)]
    );
}
