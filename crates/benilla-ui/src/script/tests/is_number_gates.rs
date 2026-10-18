//! Bindings whose 1.12.1 body tests a number argument with `lua_isnumber` (`0x6f34d0`) and, on a
//! miss, pushes nil, keeps a default or skips; or reads it with a bare `lua_tonumber` (`0x6f3620`)
//! so a non-number is 0. None of them raises, and a numeric string still counts as a number.

use super::common::script;
use crate::script::*;

fn gated() -> UiScript {
    let s = script();
    s.run(
        r#"
        Owner = CreateFrame("Frame", "GateOwner")
        TT = CreateFrame("GameTooltip", "GateTT", nil, "TestTooltipTemplate")
        EB = CreateFrame("EditBox", "GateEB")
        EB:SetText("hello")
        function ArgCount(...) return arg.n end
    "#,
    )
    .unwrap();
    s
}

/// `ActionHasRange` `0x4e75d0`: a miss pushes nil (`0x4e7613`).
#[test]
fn action_has_range_answers_nil_for_a_non_number() {
    let s = gated();
    s.run(
        r#"
        for _, v in ipairs({ "x", true }) do assert(ActionHasRange(v) == nil) end
        assert(ActionHasRange(nil) == nil and ActionHasRange({}) == nil and ActionHasRange() == nil)
    "#,
    )
    .unwrap();
}

/// `SignPetition` `0x4f46d0`: a miss keeps the byte 1 (`0x4f46d9`); a number or numeric string is
/// `_ftol`ed.
#[test]
fn sign_petition_keeps_its_default_byte_for_a_non_number() {
    let mut s = gated();
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

/// `PickupMerchantItem` `0x4fb760` takes its refusing leg (`0x4fb83f`).
#[test]
fn pickup_merchant_item_is_quiet_for_a_non_number() {
    let s = gated();
    s.run("PickupMerchantItem('x') PickupMerchantItem({}) PickupMerchantItem(nil) PickupMerchantItem()")
        .unwrap();
}

/// `SetMultisampleFormat` `0x48c640`: a miss reads as no id (`0x48c699`).
#[test]
fn set_multisample_format_is_quiet_for_a_non_number() {
    let s = gated();
    s.run("SetMultisampleFormat('x') SetMultisampleFormat({}) SetMultisampleFormat(nil)")
        .unwrap();
}

/// `GetNumRoutes` `0x4dced0` reads a bare `lua_tonumber` (`0x4dcede`): a non-number is 0, no node.
#[test]
fn get_num_routes_reads_a_non_number_as_zero() {
    let mut s = gated();
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
        assert(GetNumRoutes("x") == 0 and GetNumRoutes(nil) == 0 and GetNumRoutes({}) == 0)
        assert(GetNumRoutes(true) == 0 and GetNumRoutes() == 0)
        assert(GetNumRoutes("2") == 1 and GetNumRoutes(2.9) == 1)
    "#,
    )
    .unwrap();
}

/// `GameTooltip:SetBagItem` `0x534620`: either argument missing the gate (`0x5346ff`, `0x53470f`)
/// returns no value at all (`0x53476e`).
#[test]
fn set_bag_item_returns_nothing_for_a_non_number() {
    let s = gated();
    s.run(
        r#"
        TT:SetOwner(Owner, "ANCHOR_RIGHT")
        assert(ArgCount(TT:SetBagItem(nil, 1)) == 0)
        assert(ArgCount(TT:SetBagItem(0, "x")) == 0)
        assert(ArgCount(TT:SetBagItem({}, {})) == 0)
    "#,
    )
    .unwrap();
}

/// `GameTooltip:SetOwner` `0x5310d0`: each offset has its own gate (`0x53134f`, `0x531388`), a miss
/// keeping 0.
#[test]
fn set_owner_keeps_a_zero_offset_for_a_non_number() {
    let s = gated();
    s.run(
        r#"
        TT:SetOwner(Owner, "ANCHOR_CURSOR", nil, nil)
        TT:SetOwner(Owner, "ANCHOR_CURSOR", "x", {})
        TT:SetOwner(Owner, "ANCHOR_CURSOR", "3", 4)
    "#,
    )
    .unwrap();
}

/// `EditBox:HighlightText` `0x798870`: each argument has its own gate (`0x798907`, `0x79892e`), a
/// miss keeping the default.
#[test]
fn highlight_text_keeps_its_defaults_for_a_non_number() {
    let s = gated();
    s.run(
        r#"
        EB:HighlightText("x", {}) EB:HighlightText(nil, nil) EB:HighlightText({}, 2)
        EB:HighlightText("1", "3")
    "#,
    )
    .unwrap();
}
