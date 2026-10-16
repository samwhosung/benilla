//! Widget methods that push the number 1 or nil, never a Lua boolean, and the cursor, action and
//! chat-colour verbs that push nothing at all. Counted through the implicit vararg table's `arg.n` (`select`
//! is 5.1's, not a 1.12 global).

use super::common::script;

/// `ScrollingMessageFrame:AtTop` (`0x793130`) and `:AtBottom` (`0x7931f0`) test one flag and push
/// 1 (`0x7931c3`, `0x793283`) or nil (`0x7931d6`, `0x793296`), one value each.
#[test]
fn scrolling_message_frame_at_top_and_bottom_answer_one_or_nil() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local function shape(...) return arg.n .. ':' .. type(arg[1]) .. ':' .. tostring(arg[1]) end
        local f = CreateFrame("ScrollingMessageFrame", "SMF")
        f:SetMaxLines(128)
        for i = 1, 40 do f:AddMessage("line " .. i) end
        assert(shape(f:AtBottom()) == "1:number:1", shape(f:AtBottom()))
        assert(shape(f:AtTop()) == "1:nil:nil", shape(f:AtTop()))
        f:ScrollToTop()
        assert(shape(f:AtTop()) == "1:number:1", shape(f:AtTop()))
        assert(shape(f:AtBottom()) == "1:nil:nil", shape(f:AtBottom()))
    "#,
    )
    .unwrap();
}

/// `GameTooltip:IsOwned` (`0x530fe0`) pushes 1 (`0x5310ad`) or nil (`0x5310c0`), one value.
#[test]
fn tooltip_is_owned_answers_one_or_nil() {
    let mut s = script();
    s.set_screen_size(800.0, 600.0);
    s.run(
        r#"
        local function shape(...) return arg.n .. ':' .. type(arg[1]) .. ':' .. tostring(arg[1]) end
        local a = CreateFrame("Button", "A"); a:SetPoint("CENTER", 0, 0); a:SetWidth(10); a:SetHeight(10)
        local b = CreateFrame("Button", "B"); b:SetPoint("CENTER", 50, 0); b:SetWidth(10); b:SetHeight(10)
        local tt = CreateFrame("GameTooltip", "TT", nil, "TestTooltipTemplate")
        tt:SetOwner(a, "ANCHOR_RIGHT")
        assert(shape(tt:IsOwned(a)) == "1:number:1", shape(tt:IsOwned(a)))
        assert(shape(tt:IsOwned(b)) == "1:nil:nil", shape(tt:IsOwned(b)))
    "#,
    )
    .unwrap();
}

/// `FontString:SetAlphaGradient` (`0x79cc80`) pushes 1 (`0x79cd58`) while `start` is inside the
/// text and nil (`0x79cd6d`) once it is past, one value.
#[test]
fn set_alpha_gradient_answers_one_or_nil() {
    let s = script();
    s.run(
        r#"
        local function shape(...) return arg.n .. ':' .. type(arg[1]) .. ':' .. tostring(arg[1]) end
        local f = CreateFrame("Frame", "F")
        local fs = f:CreateFontString("FS"); fs:SetFont("Fonts\\FRIZQT__.TTF", 12)
        fs:SetText("hello")
        assert(shape(fs:SetAlphaGradient(0, 5)) == "1:number:1", shape(fs:SetAlphaGradient(0, 5)))
        assert(shape(fs:SetAlphaGradient(5, 5)) == "1:nil:nil", shape(fs:SetAlphaGradient(5, 5)))
    "#,
    )
    .unwrap();
}

/// The cursor, action and chat-colour verbs push no value: `xor eax,eax; ret` ends each handler. A bare
/// `return F()` hands back zero values, not a boolean for whether anything moved.
#[test]
fn the_pickup_and_place_verbs_return_nothing() {
    let s = script();
    for call in [
        "PickupInventoryItem(1)",
        "EquipCursorItem(1)",
        "AutoEquipCursorItem()",
        "SplitContainerItem(0, 1, 1)",
        "PickupAction(1)",
        "PlaceAction(1)",
        "PickupPetAction(1)",
        "PickupSpell(1, BOOKTYPE_SPELL)",
        "PickupMacro(1)",
        "DeleteMacro(1)",
        "ResetChatColors()",
    ] {
        assert_eq!(s.arity(call).unwrap(), 0, "{call}");
    }
}
