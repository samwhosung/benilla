//! Every index-reading binding raises its own usage line for a non-number index, the literal it
//! pushes to `luaL_error` (`0x6f4940`) after `is-number` (`0x6f34d0`) fails, the client's typos
//! included; the three that branch instead never raise.

use super::common::script;

/// One row per binding and argument: a table where the index belongs raises the binding's line,
/// with `luaL_error`'s position.
#[test]
fn a_non_number_index_raises_the_bindings_own_usage_line() {
    let s = script();
    for (call, usage) in [
        ("CancelAuction({})", "Usage: SetSelectedAuctionItem(index)"),
        ("CancelAuction()", "Usage: SetSelectedAuctionItem(index)"),
        ("CancelPendingEquip({})", "Usage: CancelPendingEquip(index)"),
        ("CancelPendingEquip()", "Usage: CancelPendingEquip(index)"),
        ("GetActionCooldown({})", "Usage: GetActionCooldown(slot)"),
        ("GetActionCooldown()", "Usage: GetActionCooldown(slot)"),
        ("GetActionCount({})", "Usage: GetActionCount(slot)"),
        ("GetActionCount()", "Usage: GetActionCount(slot)"),
        ("GetActionTexture({})", "Usage: GetActionTexture(slot)"),
        ("GetActionTexture()", "Usage: GetActionTexture(slot)"),
        ("GetActiveTitle({})", "Usage: GetActiveTitle(index)"),
        ("GetActiveTitle()", "Usage: GetActiveTitle(index)"),
        (
            r#"GetAuctionItemTimeLeft("list", {})"#,
            r#"Usage: GetAuctionItemTimeLeft("type", index)"#,
        ),
        (
            r#"GetAuctionItemTimeLeft({}, 1)"#,
            r#"Usage: GetAuctionItemTimeLeft("type", index)"#,
        ),
        ("GetAvailableTitle({})", "Usage: GetAvailableTitle(index)"),
        ("GetAvailableTitle()", "Usage: GetAvailableTitle(index)"),
        ("GetBuybackItemInfo({})", "Usage: GetBuybackItemInfo(slot)"),
        ("GetBuybackItemInfo()", "Usage: GetBuybackItemInfo(slot)"),
        (
            "GetCraftDescription({})",
            "Usage: GetCraftDescription(index)",
        ),
        ("GetCraftDescription()", "Usage: GetCraftDescription(index)"),
        ("GetCraftIcon({})", "Usage: GetTradeSkillIcon(index)"),
        ("GetCraftIcon()", "Usage: GetTradeSkillIcon(index)"),
        ("GetCraftInfo({})", "Usage: GetCraftInfo(index)"),
        ("GetCraftInfo()", "Usage: GetCraftInfo(index)"),
        (
            "GetCraftNumReagents({})",
            "Usage: GetCraftNumReagents(index)",
        ),
        ("GetCraftNumReagents()", "Usage: GetCraftNumReagents(index)"),
        (
            "GetCraftReagentInfo({}, 1)",
            "Usage: GetCraftReagentInfo(index, reagentIndex)",
        ),
        (
            "GetCraftReagentInfo(1, {})",
            "Usage: GetCraftReagentInfo(index, reagentIndex)",
        ),
        (
            "GetCraftReagentInfo(1)",
            "Usage: GetCraftReagentInfo(index, reagentIndex)",
        ),
        ("GetFactionInfo({})", "Usage: GetFactionInfo(index)"),
        ("GetFactionInfo()", "Usage: GetFactionInfo(index)"),
        ("GetInboxHeaderInfo({})", "Usage: GetInboxHeaderInfo(index)"),
        ("GetInboxHeaderInfo()", "Usage: GetInboxHeaderInfo(index)"),
        (
            "GetInboxInvoiceInfo({})",
            "Usage: GetInboxInvoiceInfo(index)",
        ),
        ("GetInboxInvoiceInfo()", "Usage: GetInboxInvoiceInfo(index)"),
        ("GetInboxItem({})", "Usage: GetInboxItem(index)"),
        ("GetInboxItem()", "Usage: GetInboxItem(index)"),
        ("GetInboxText({})", "Usage: GetInboxText(index)"),
        ("GetInboxText()", "Usage: GetInboxText(index)"),
        (
            "GetInventoryAlertStatus({})",
            "Usage: GetInventoryAlertStatus(index)",
        ),
        (
            "GetInventoryAlertStatus()",
            "Usage: GetInventoryAlertStatus(index)",
        ),
        ("GetLootRollItemInfo({})", "Usage: GetLootRollItemInfo(id)"),
        ("GetLootRollItemInfo()", "Usage: GetLootRollItemInfo(id)"),
        ("GetLootRollItemLink({})", "Usage: GetLootRollItemLink(id)"),
        ("GetLootRollItemLink()", "Usage: GetLootRollItemLink(id)"),
        ("GetLootRollTimeLeft({})", "Usage: GetLootRollTimeLeft(id)"),
        ("GetLootRollTimeLeft()", "Usage: GetLootRollTimeLeft(id)"),
        ("GetLootSlotInfo({})", "Usage: GetLootSlotInfo(slot)"),
        ("GetLootSlotInfo()", "Usage: GetLootSlotInfo(slot)"),
        ("GetLootSlotLink({})", "Usage: GetLootSlotLink(slot)"),
        ("GetLootSlotLink()", "Usage: GetLootSlotLink(slot)"),
        ("GetMacroInfo({})", "Usage: GetMacroInfo(index)"),
        ("GetMacroInfo()", "Usage: GetMacroInfo(index)"),
        (
            "GetMasterLootCandidate({})",
            "Usage: GetMasterLootCandidate(index)",
        ),
        (
            "GetMasterLootCandidate()",
            "Usage: GetMasterLootCandidate(index)",
        ),
        (
            "GetMerchantItemInfo({})",
            "Usage: GetMerchantItemInfo(index)",
        ),
        ("GetMerchantItemInfo()", "Usage: GetMerchantItemInfo(index)"),
        (
            "GetMerchantItemLink({})",
            "Usage: GetMerchantItemLink(index)",
        ),
        ("GetMerchantItemLink()", "Usage: GetMerchantItemLink(index)"),
        ("GetNumTalents({})", "Usage: GetNumTalents(tabIndex)"),
        ("GetNumTalents()", "Usage: GetNumTalents(tabIndex)"),
        (
            "GetPetActionCooldown({})",
            "Usage: GetPetActionCooldown(index)",
        ),
        (
            "GetPetActionCooldown()",
            "Usage: GetPetActionCooldown(index)",
        ),
        ("GetPetActionInfo({})", "Usage: GetPetActionInfo(index)"),
        ("GetPetActionInfo()", "Usage: GetPetActionInfo(index)"),
        (
            "GetQuestIndexForTimer({})",
            "Usage: GetQuestIndexForTimer(index)",
        ),
        (
            "GetQuestIndexForTimer()",
            "Usage: GetQuestIndexForTimer(index)",
        ),
        (
            "GetQuestIndexForWatch({})",
            "Usage: GetQuestIndexForWatch(index)",
        ),
        (
            "GetQuestIndexForWatch()",
            "Usage: GetQuestIndexForWatch(index)",
        ),
        (
            "GetQuestLogLeaderBoard({})",
            "Usage: GetQuestLogLeaderBoard(index)",
        ),
        (
            "GetQuestLogLeaderBoard()",
            "Usage: GetQuestLogLeaderBoard(index)",
        ),
        ("GetSkillLineInfo({})", "Usage: GetSkillLineInfo(index)"),
        ("GetSkillLineInfo()", "Usage: GetSkillLineInfo(index)"),
        (
            "GetTalentInfo({}, 1)",
            "Usage: GetTalentInfo(tabIndex, talentIndex)",
        ),
        (
            "GetTalentInfo(1, {})",
            "Usage: GetTalentInfo(tabIndex, talentIndex)",
        ),
        (
            "GetTalentInfo(1)",
            "Usage: GetTalentInfo(tabIndex, talentIndex)",
        ),
        ("GetTalentTabInfo({})", "Usage: GetTalentTabInfo(index)"),
        ("GetTalentTabInfo()", "Usage: GetTalentTabInfo(index)"),
        (
            "GetTradePlayerItemInfo({})",
            "Usage: GetTradePlayerItemInfo(index)",
        ),
        (
            "GetTradePlayerItemInfo()",
            "Usage: GetTradePlayerItemInfo(index)",
        ),
        (
            "GetTradePlayerItemLink({})",
            "Usage: GetTradePlayerItemLink(index)",
        ),
        (
            "GetTradePlayerItemLink()",
            "Usage: GetTradePlayerItemLink(index)",
        ),
        (
            "GetTradeSkillCooldown({})",
            "Usage: GetTradeSkillCooldown(index)",
        ),
        (
            "GetTradeSkillCooldown()",
            "Usage: GetTradeSkillCooldown(index)",
        ),
        ("GetTradeSkillIcon({})", "Usage: GetTradeSkillIcon(index)"),
        ("GetTradeSkillIcon()", "Usage: GetTradeSkillIcon(index)"),
        ("GetTradeSkillInfo({})", "Usage: GetTradeSkillInfo(index)"),
        ("GetTradeSkillInfo()", "Usage: GetTradeSkillInfo(index)"),
        (
            "GetTradeSkillNumMade({})",
            "Usage: GetTradeSkillNumMade(index)",
        ),
        (
            "GetTradeSkillNumMade()",
            "Usage: GetTradeSkillNumMade(index)",
        ),
        (
            "GetTradeSkillNumReagents({})",
            "Usage: GetTradeSkillNumReagents(index)",
        ),
        (
            "GetTradeSkillNumReagents()",
            "Usage: GetTradeSkillNumReagents(index)",
        ),
        (
            "GetTradeSkillReagentInfo({}, 1)",
            "Usage: GetTradeSkillReagentInfo(index, reagentIndex)",
        ),
        (
            "GetTradeSkillReagentInfo(1, {})",
            "Usage: GetTradeSkillReagentInfo(index, reagentIndex)",
        ),
        (
            "GetTradeSkillReagentInfo(1)",
            "Usage: GetTradeSkillReagentInfo(index, reagentIndex)",
        ),
        (
            "GetTradeTargetItemInfo({})",
            "Usage: GetTradeTargetItemInfo(index)",
        ),
        (
            "GetTradeTargetItemInfo()",
            "Usage: GetTradeTargetItemInfo(index)",
        ),
        (
            "GetTradeTargetItemLink({})",
            "Usage: GetTradeTargetItemLink(index)",
        ),
        (
            "GetTradeTargetItemLink()",
            "Usage: GetTradeTargetItemLink(index)",
        ),
        (
            "GetTrainerServiceAbilityReq({}, 1)",
            "Usage: GetTrainerServiceAbilityReq(index)",
        ),
        (
            "GetTrainerServiceAbilityReq(1, {})",
            "Usage: GetTrainerServiceAbilityReq(index)",
        ),
        (
            "GetTrainerServiceAbilityReq(1)",
            "Usage: GetTrainerServiceAbilityReq(index)",
        ),
        (
            "GetTrainerServiceCost({})",
            "Usage: GetTrainerServiceCost(index)",
        ),
        (
            "GetTrainerServiceCost()",
            "Usage: GetTrainerServiceCost(index)",
        ),
        (
            "GetTrainerServiceDescription({})",
            "Usage: GetTrainerServiceDescription(index)",
        ),
        (
            "GetTrainerServiceDescription()",
            "Usage: GetTrainerServiceDescription(index)",
        ),
        (
            "GetTrainerServiceIcon({})",
            "Usage: GetTrainerServiceIcon(index)",
        ),
        (
            "GetTrainerServiceIcon()",
            "Usage: GetTrainerServiceIcon(index)",
        ),
        (
            "GetTrainerServiceInfo({})",
            "Usage: GetTrainerServiceInfo(index)",
        ),
        (
            "GetTrainerServiceInfo()",
            "Usage: GetTrainerServiceInfo(index)",
        ),
        (
            "GetTrainerServiceLevelReq({})",
            "Usage: GetTrainerServiceLevelReq(index)",
        ),
        (
            "GetTrainerServiceLevelReq()",
            "Usage: GetTrainerServiceLevelReq(index)",
        ),
        (
            "GetTrainerServiceNumAbilityReq({})",
            "Usage: GetTrainerServiceAbilityReq(index)",
        ),
        (
            "GetTrainerServiceNumAbilityReq()",
            "Usage: GetTrainerServiceAbilityReq(index)",
        ),
        (
            "GetTrainerServiceSkillLine({})",
            "Usage: GetTrainerServiceSkillLine(index)",
        ),
        (
            "GetTrainerServiceSkillLine()",
            "Usage: GetTrainerServiceSkillLine(index)",
        ),
        (
            "GetTrainerServiceSkillReq({})",
            "Usage: GetTrainerServiceSkillReq(index)",
        ),
        (
            "GetTrainerServiceSkillReq()",
            "Usage: GetTrainerServiceSkillReq(index)",
        ),
        (
            "GetTrainerServiceStepReq({})",
            "Usage: GetTrainerServiceStepReq(index)",
        ),
        (
            "GetTrainerServiceStepReq()",
            "Usage: GetTrainerServiceStepReq(index)",
        ),
        ("HasAction({})", "Usage: HasAction(slot)"),
        ("HasAction()", "Usage: HasAction(slot)"),
        ("IsAttackAction({})", "Usage: IsAttackAction(slot)"),
        ("IsAttackAction()", "Usage: IsAttackAction(slot)"),
        ("IsAutoRepeatAction({})", "Usage: IsAutorepeatAction(slot)"),
        ("IsAutoRepeatAction()", "Usage: IsAutorepeatAction(slot)"),
        ("IsConsumableAction({})", "Usage: IsConsumableAction(slot)"),
        ("IsConsumableAction()", "Usage: IsConsumableAction(slot)"),
        ("IsCurrentAction({})", "Usage: IsCurrentAction(slot)"),
        ("IsCurrentAction()", "Usage: IsCurrentAction(slot)"),
        ("IsEquippedAction({})", "Usage: IsEquippedAction(slot)"),
        ("IsEquippedAction()", "Usage: IsEquippedAction(slot)"),
        ("IsFactionInactive({})", "Usage: IsFactionInactive(index)"),
        ("IsFactionInactive()", "Usage: IsFactionInactive(index)"),
        ("IsPetAttackActive({})", "Usage: IsPetAttackActive(index)"),
        ("IsPetAttackActive()", "Usage: IsPetAttackActive(index)"),
        ("IsQuestWatched({})", "Usage: IsQuestWatched(index)"),
        ("IsQuestWatched()", "Usage: IsQuestWatched(index)"),
        (
            "IsTrainerServiceLearnSpell({})",
            "Usage: IsTrainerServiceLearnSpell(index)",
        ),
        (
            "IsTrainerServiceLearnSpell()",
            "Usage: IsTrainerServiceLearnSpell(index)",
        ),
        (
            "IsTrainerServiceTradeSkill({})",
            "Usage: IsTrainerServiceTradeSkill(index)",
        ),
        (
            "IsTrainerServiceTradeSkill()",
            "Usage: IsTrainerServiceTradeSkill(index)",
        ),
        ("IsUsableAction({})", "Usage: IsUsableAction(slot)"),
        ("IsUsableAction()", "Usage: IsUsableAction(slot)"),
        (
            r#"SetSelectedAuctionItem("list", {})"#,
            r#"Usage: SetSelectedAuctionItem("type", index)"#,
        ),
        (
            r#"SetSelectedAuctionItem({}, 1)"#,
            r#"Usage: SetSelectedAuctionItem("type", index)"#,
        ),
    ] {
        let e = s.run(call).expect_err(call).to_string();
        assert!(e.contains(&format!(":1: {usage}")), "{call}: {e}");
    }
}

/// `IsActionInRange` pushes nil for a non-number (`0x4e7563`); `GetNumQuestLeaderBoards` and
/// `GetQuestLogLeaderBoard`'s quest read the selection (`0x4e0056`, `0x4e01a2`).
#[test]
fn the_index_reads_that_branch_never_raise() {
    let s = script();
    assert!(s.eval::<bool>("return IsActionInRange({}) == nil").unwrap());
    assert_eq!(
        s.eval::<i64>("return GetNumQuestLeaderBoards({})").unwrap(),
        s.eval::<i64>("return GetNumQuestLeaderBoards()").unwrap()
    );
    s.run("GetQuestLogLeaderBoard(1, {})").unwrap();
}
