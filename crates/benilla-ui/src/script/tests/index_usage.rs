//! Every index-reading binding raises its own usage line for a non-number index, the literal it
//! pushes to `luaL_error` (`0x6f4940`) after `is-number` (`0x6f34d0`) fails, the client's typos
//! included, and for a non-string where `is-number-or-string` (`0x6f3510`) gates a name beside it;
//! the inventory slot reader (`0x4c8520`) raises its binding's `Invalid inventory slot` line. A
//! negative index is a number and never a conversion error; the ones that branch never raise.

use super::common::script;

/// One row per binding and argument: a table where the index belongs raises the binding's line,
/// with `luaL_error`'s position.
#[test]
fn a_non_number_index_raises_the_bindings_own_usage_line() {
    let mut s = script();
    // `SetLootThreshold` reads its argument only as a group's leader (`0x4e9500`).
    s.set_party(crate::script::PartyState {
        members: vec![crate::script::PartyMemberInfo {
            name: "Mate".into(),
            guid: 0x1,
        }],
        leader_guid: 0x5E1F,
        own_guid: 0x5E1F,
        ..Default::default()
    });
    s.run(r#"IdxTip = CreateFrame("GameTooltip", "IdxTip"); IdxTex = IdxTip:CreateTexture()"#)
        .unwrap();
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
        ("AbandonSkill({})", "Usage: AbandonSkill(index)"),
        (
            r#"AddChatWindowChannel({}, "x")"#,
            r#"Usage: AddChatWindowChannel(index, "channel")"#,
        ),
        (
            "AddChatWindowChannel(1, {})",
            r#"Usage: AddChatWindowChannel(index, "channel")"#,
        ),
        (
            r#"AddChatWindowMessages({}, "x")"#,
            r#"Usage: AddChatWindowMessages(index, "messageGroup")"#,
        ),
        (
            "AddChatWindowMessages(1, {})",
            r#"Usage: AddChatWindowMessages(index, "messageGroup")"#,
        ),
        ("BuyMerchantItem({})", "Usage: BuyMerchantItem(index)"),
        ("BuyTrainerService({})", "Usage: BuyTrainerService(index)"),
        ("BuybackItem({})", "Usage: BuybackItem(slot)"),
        (
            "CalculateAuctionDeposit({})",
            "Usage: CalculateAuctionDeposit(runTime)",
        ),
        ("CastPetAction({})", "Usage: CastPetAction(index)"),
        (
            "CheckInteractDistance({}, 1)",
            r#"Usage: CheckInteractDistance("unit", distIndex)"#,
        ),
        (
            r#"CheckInteractDistance("player", {})"#,
            r#"Usage: CheckInteractDistance("unit", distIndex)"#,
        ),
        ("ClickStablePet({})", "Usage: ClickStablePet(index)"),
        (
            "ClickTargetTradeButton({})",
            "Usage: ClickTargetTradeButton(index)",
        ),
        (
            "CollapseFactionHeader({})",
            "Usage: CollapseFactionHeader(index)",
        ),
        (
            "CollapseQuestHeader({})",
            "Usage: CollapseQuestHeader(index)",
        ),
        (
            "CollapseSkillHeader({})",
            "Usage: CollapseSkillHeader(index)",
        ),
        (
            "CollapseTradeSkillSubClass({})",
            "Usage: CollapseTradeSkillSubClass(index)",
        ),
        (
            "CollapseTrainerSkillLine({})",
            "Usage: CollapseTrainerSkillLine(index)",
        ),
        ("DeleteMacro({})", "Usage: DeleteMacro(index)"),
        ("DoCraft({})", "Usage: DoCraft(index)"),
        ("DoTradeSkill({})", "Usage: DoTradeSkill(index [, repeat])"),
        ("EquipCursorItem({})", "Usage: EquipCursorItem(slot)"),
        ("EquipPendingItem({})", "Usage: EquipPendingItem(index)"),
        (
            "ExpandFactionHeader({})",
            "Usage: ExpandFactionHeader(index)",
        ),
        ("ExpandQuestHeader({})", "Usage: ExpandQuestHeader(index)"),
        ("ExpandSkillHeader({})", "Usage: ExpandSkillHeader(index)"),
        (
            "ExpandTradeSkillSubClass({})",
            "Usage: ExpandTradeSkillSubClass(index)",
        ),
        (
            "ExpandTrainerSkillLine({})",
            "Usage: ExpandTrainerSkillLine(index)",
        ),
        ("FactionToggleAtWar({})", "Usage: FactionToggleAtWar(index)"),
        (
            "GetAuctionInvTypes({}, 1)",
            "Usage: GetAuctionInvTypes(classIndex, subClassIndex)",
        ),
        (
            "GetAuctionInvTypes(1, {})",
            "Usage: GetAuctionInvTypes(classIndex, subClassIndex)",
        ),
        (
            "GetAuctionItemSubClasses({})",
            "Usage: GetAuctionItemSubClasses(index)",
        ),
        ("GetBagName({})", "Usage: GetBagName(index)"),
        (
            "GetChatWindowChannels({})",
            "Usage: GetChatWindowChannels(index)",
        ),
        ("GetChatWindowInfo({})", "Usage: GetChatWindowInfo(index)"),
        (
            "GetChatWindowMessages({})",
            "Usage: GetChatWindowMessages(index)",
        ),
        (
            "GetContainerNumSlots({})",
            "Usage: GetContainerNumSlots(index)",
        ),
        ("GetCraftSpellFocus({})", "Usage: GetCraftSpellFocus(index)"),
        ("GetFriendInfo({})", "Usage: GetFriendInfo(index)"),
        ("GetGuildRosterInfo({})", "Usage: GetGuildRosterInfo(index)"),
        (
            "GetGuildRosterLastOnline({})",
            "Usage: GetGuildRosterLastOnline(index)",
        ),
        ("GetIgnoreName({})", "Usage: GetIgnoreName(index)"),
        (
            "GetItemQualityColor({})",
            "Usage: GetItemQualityColor(index)",
        ),
        ("GetLanguageByIndex({})", "Usage: GetLanguageByIndex(index)"),
        ("GetMapLandmarkInfo({})", "Usage: GetMapLandmarkInfo(index)"),
        ("GetMapOverlayInfo({})", "Usage: GetMapOverlayInfo(index)"),
        ("GetMapZones({})", "Usage: GetMapZones(continentIndex)"),
        (
            "GetPetitionNameInfo({})",
            "Usage: GetPetitionNameInfo(index)",
        ),
        (
            "GetQuestLogItemLink({}, 1)",
            r#"Usage: GetQuestLogItemLink("type", index)"#,
        ),
        (
            r#"GetQuestLogItemLink("x", {})"#,
            r#"Usage: GetQuestLogItemLink("type", index)"#,
        ),
        (
            "GetStablePetFoodTypes({})",
            "Usage: GetStablePetInfo(index)",
        ),
        ("GetStablePetInfo({})", "Usage: GetStablePetInfo(index)"),
        (
            "GetTalentPrereqs({}, 1)",
            "Usage: GetTalentPrereqs(tabIndex, talentIndex)",
        ),
        (
            "GetTalentPrereqs(1, {})",
            "Usage: GetTalentPrereqs(tabIndex, talentIndex)",
        ),
        ("GetTradeSkillTools({})", "Usage: GetTradeSkillTools(index)"),
        ("GetWhoInfo({})", "Usage: GetWhoInfo(index)"),
        (
            "GiveMasterLoot({}, 1)",
            "Usage: GiveMasterLoot(slot, index)",
        ),
        (
            "GiveMasterLoot(1, {})",
            "Usage: GiveMasterLoot(slot, index)",
        ),
        (
            "GuildControlGetRankName({})",
            "Usage: GuildControlGetRankName(index)",
        ),
        (
            "GuildControlSetRank({})",
            "Usage: GuildControlSetRank(rank)",
        ),
        (
            "GuildControlSetRankFlag({})",
            "Usage: GuildControlSetRankFlag(index, enabled)",
        ),
        ("InboxItemCanDelete({})", "Usage: InboxItemCanDelete(index)"),
        (
            "LearnTalent({}, 1)",
            "Usage: LearnTalent(tabIndex, talentIndex)",
        ),
        (
            "LearnTalent(1, {})",
            "Usage: LearnTalent(tabIndex, talentIndex)",
        ),
        ("LootSlot({})", "Usage: LootSlot(slot)"),
        ("LootSlotIsCoin({})", "Usage: LootSlotIsCoin(slot)"),
        ("LootSlotIsItem({})", "Usage: LootSlotIsItem(slot)"),
        ("PickupAction({})", "Usage: PickupAction(slot)"),
        (
            "PickupContainerItem({}, 1)",
            "Usage: PickupContainerItem(index, slot)",
        ),
        (
            "PickupContainerItem(1, {})",
            "Usage: PickupContainerItem(index, slot)",
        ),
        ("PickupMacro({})", "Usage: PickupMacro(index)"),
        ("PickupPetAction({})", "Usage: PickupPetAction(index)"),
        ("PickupStablePet({})", "Usage: PickupStablePet(index)"),
        ("PlaceAction({})", "Usage: PlaceAction(slot)"),
        (
            "PlaceAuctionBid({}, 1, 1)",
            r#"Usage: PlaceAuctionBid("type", index, bid)"#,
        ),
        (
            r#"PlaceAuctionBid("list", {}, 1)"#,
            r#"Usage: PlaceAuctionBid("type", index, bid)"#,
        ),
        (
            r#"PlaceAuctionBid("list", 1, {})"#,
            r#"Usage: PlaceAuctionBid("type", index, bid)"#,
        ),
        (
            r#"RemoveChatWindowChannel({}, "x")"#,
            r#"Usage: RemoveChatWindowChannel(index, "channel")"#,
        ),
        (
            "RemoveChatWindowChannel(1, {})",
            r#"Usage: RemoveChatWindowChannel(index, "channel")"#,
        ),
        (
            r#"RemoveChatWindowMessages({}, "x")"#,
            r#"Usage: RemoveChatWindowMessages(index, "messageGroup")"#,
        ),
        (
            "RemoveChatWindowMessages(1, {})",
            r#"Usage: RemoveChatWindowMessages(index, "messageGroup")"#,
        ),
        ("SelectCraft({})", "Usage: SelectCraft(index)"),
        (
            "SelectGossipActiveQuest({})",
            "Usage: SelectGossipActiveQuest(index)",
        ),
        (
            "SelectGossipAvailableQuest({})",
            "Usage: SelectGossipAvailableQuest(index)",
        ),
        ("SelectGossipOption({})", "Usage: SelectGossipOption(index)"),
        (
            "SelectQuestLogEntry({})",
            "Usage: SelectQuestLogEntry(index)",
        ),
        ("SelectTradeSkill({})", "Usage: SelectTradeSkill(index)"),
        (
            "SelectTrainerService({})",
            "Usage: SelectTrainerService(index)",
        ),
        ("IdxTip:SetAction({})", "Usage: SetAction(slot)"),
        (
            "IdxTip:SetAuctionItem({}, 1)",
            r#"Usage: SetAuctionItem("type", index)"#,
        ),
        (
            r#"IdxTip:SetAuctionItem("list", {})"#,
            r#"Usage: SetAuctionItem("type", index)"#,
        ),
        (
            "SetBagPortaitTexture(IdxTex, {})",
            "Usage: SetBagPortaitTexture(texture, slot)",
        ),
        (
            "SetChatWindowAlpha({}, 1)",
            "Usage: SetChatWindowAlpha(index, alpha)",
        ),
        (
            "SetChatWindowAlpha(1, {})",
            "Usage: SetChatWindowAlpha(index, alpha)",
        ),
        (
            "SetChatWindowColor({}, 1, 1, 1)",
            "Usage: SetChatWindowColor(index, r, g, b)",
        ),
        (
            "SetChatWindowColor(1, {}, 1, 1)",
            "Usage: SetChatWindowColor(index, r, g, b)",
        ),
        (
            "SetChatWindowColor(1, 1, {}, 1)",
            "Usage: SetChatWindowColor(index, r, g, b)",
        ),
        (
            "SetChatWindowColor(1, 1, 1, {})",
            "Usage: SetChatWindowColor(index, r, g, b)",
        ),
        (
            "SetChatWindowDocked({})",
            "Usage: SetChatWindowDocked(index, docked)",
        ),
        (
            "SetChatWindowLocked({})",
            "Usage: SetChatWindowLocked(index, locked)",
        ),
        (
            "SetChatWindowName({})",
            r#"Usage: SetChatWindowName(index, "name")"#,
        ),
        (
            "SetChatWindowShown({})",
            "Usage: SetChatWindowShown(index, shown)",
        ),
        (
            "SetChatWindowSize({}, 1)",
            "Usage: SetChatWindowSize(index, size)",
        ),
        (
            "SetChatWindowSize(1, {})",
            "Usage: SetChatWindowSize(index, size)",
        ),
        (
            "SetGuildRosterSelection({})",
            "Usage: SetGuildRosterSelection(index)",
        ),
        ("IdxTip:SetInboxItem({})", "Usage: SetInboxItem(index)"),
        ("IdxTip:SetLootRollItem({})", "Usage: SetLootRollItem(id)"),
        ("SetLootThreshold({})", "Usage: SetLooThreshold(threshold)"),
        (
            "SetMapZoom({})",
            "Usage: SetMapZoom(continentIndex [,zoneIndex])",
        ),
        ("IdxTip:SetPetAction({})", "Usage: SetPetAction(slot)"),
        (
            "IdxTip:SetPlayerBuff({})",
            "Usage: SetPlayerBuff(buffIndex)",
        ),
        ("SetRaidTarget({}, 1)", "Usage: SetRaidTarget(unit, index)"),
        (
            r#"SetRaidTarget("player", {})"#,
            "Usage: SetRaidTarget(unit, index)",
        ),
        ("SetSelectedFaction({})", "Usage: SetSelectedFaction(index)"),
        ("SetSelectedFriend({})", "Usage: SetSelectedFriend(index)"),
        ("SetSelectedIgnore({})", "Usage: SetSelectedIgnore(index)"),
        ("SetSelectedSkill({})", "Usage: SetSelectedSkill(index)"),
        ("IdxTip:SetShapeshift({})", "Usage: SetShapeshift(slot)"),
        (
            "IdxTip:SetTalent({}, 1)",
            "Usage: SetTalent(tabIndex, talentIndex)",
        ),
        (
            "IdxTip:SetTalent(1, {})",
            "Usage: SetTalent(tabIndex, talentIndex)",
        ),
        (
            "SetWatchedFactionIndex({})",
            "Usage: SetWatchedFactionIndex(index)",
        ),
        (
            "ShowBuybackSellCursor({})",
            "Usage: ShowBuybackSellCursor(index)",
        ),
        (
            "ShowContainerSellCursor({}, 1)",
            "Usage: ShowContainerSellCursor(index, slot)",
        ),
        (
            "ShowContainerSellCursor(1, {})",
            "Usage: ShowContainerSellCursor(index, slot)",
        ),
        (
            "ShowMerchantSellCursor({})",
            "Usage: ShowMerchantSellCursor(index)",
        ),
        (
            "SplitContainerItem({}, 1, 1)",
            "Usage: SplitContainerItem(index, slot, amount)",
        ),
        (
            "SplitContainerItem(1, {}, 1)",
            "Usage: SplitContainerItem(index, slot, amount)",
        ),
        (
            "SplitContainerItem(1, 1, {})",
            "Usage: SplitContainerItem(index, slot, amount)",
        ),
        ("TakeTaxiNode({})", "Usage: TakeTaxiNode(slot)"),
        ("TaxiNodeCost({})", "Usage: TaxiNodeCost(slot)"),
        ("TaxiNodeGetType({})", "Usage: TaxiNodeGetType(slot)"),
        ("TaxiNodeName({})", "Usage: TaxiNodeName(slot)"),
        (
            "TaxiNodePosition({})",
            "Usage: TaxiNodeTaxiNodeLocation(slot)",
        ),
        ("TaxiNodeSetCurrent({})", "Usage: TaxiNodeSetCurrent(slot)"),
        (
            "UnitBuff({}, 1)",
            r#"Usage: UnitBuff("unit", index [,raidFilter])"#,
        ),
        (
            r#"UnitBuff("player", {})"#,
            r#"Usage: UnitBuff("unit", index [,raidFilter])"#,
        ),
        (
            "UnitDebuff({}, 1)",
            r#"Usage: UnitDebuff("unit", index [,raidFilter])"#,
        ),
        (
            r#"UnitDebuff("player", {})"#,
            r#"Usage: UnitDebuff("unit", index [,raidFilter])"#,
        ),
        ("UnstablePet({})", "Usage: UnstablePet(index)"),
        (
            "UseAction({})",
            "Usage: UseAction(slot, [, checkCursor] [, onSelf])",
        ),
        (
            "UseContainerItem({}, 1)",
            "Usage: UseContainerItem(index, slot[, onSelf])",
        ),
        (
            "UseContainerItem(1, {})",
            "Usage: UseContainerItem(index, slot[, onSelf])",
        ),
        ("CastShapeshiftForm({})", "Usage: CastShapeshiftForm(index)"),
        ("CastShapeshiftForm(0)", "Usage: CastShapeshiftForm(index)"),
        ("GetPartyMember({})", "Usage: GetPartyMember(1-4)"),
        ("GetPartyMember(0)", "Usage: GetPartyMember(1-4)"),
        ("GetPartyMember(5)", "Usage: GetPartyMember(1-4)"),
        (
            "GetInventoryItemTexture({}, 1)",
            "Usage: GetInventoryItemTexture(unit, slot)",
        ),
        (
            r#"GetInventoryItemTexture("player", {})"#,
            "Invalid inventory slot in GetInventoryItemTexture",
        ),
        (
            r#"GetInventoryItemTexture("player", 30)"#,
            "Invalid inventory slot in GetInventoryItemTexture",
        ),
        (
            "GetInventoryItemLink({}, 1)",
            "Usage: GetInventoryItemLink(unit, slot)",
        ),
        (
            r#"GetInventoryItemLink("player", {})"#,
            "Invalid inventory slot in GetInventoryItemLink",
        ),
        (
            r#"GetInventoryItemLink("player", 30)"#,
            "Invalid inventory slot in GetInventoryItemLink",
        ),
        (
            "GetInventoryItemQuality({}, 1)",
            "Usage: GetInventoryItemQuality(unit, slot)",
        ),
        (
            r#"GetInventoryItemQuality("player", {})"#,
            "Invalid inventory slot in GetInventoryItemQuality",
        ),
        (
            r#"GetInventoryItemQuality("player", 30)"#,
            "Invalid inventory slot in GetInventoryItemQuality",
        ),
        (
            "GetInventoryItemBroken({}, 1)",
            "Usage: GetInventoryItemBroken(unit, slot)",
        ),
        (
            r#"GetInventoryItemBroken("player", {})"#,
            "Invalid inventory slot in GetInventoryItemBroken",
        ),
        (
            r#"GetInventoryItemBroken("player", 30)"#,
            "Invalid inventory slot in GetInventoryItemBroken",
        ),
        (
            "IdxTip:SetInventoryItem({}, 1)",
            "Usage: SetInventoryItem(unit, slot [, nameOnly])",
        ),
        (
            r#"IdxTip:SetInventoryItem("player", {})"#,
            "Invalid inventory slot in SetInventoryItem",
        ),
        (
            r#"IdxTip:SetInventoryItem("player", 30)"#,
            "Invalid inventory slot in SetInventoryItem",
        ),
        (
            "ChangeChatColor({}, 1, 1, 1)",
            "Usage(ChangeChatColor(chatType, r, g, b)",
        ),
        (
            r#"ChangeChatColor("SAY", {}, 1, 1)"#,
            "Usage(ChangeChatColor(chatType, r, g, b)",
        ),
        (
            r#"ChangeChatColor("SAY", 1, {}, 1)"#,
            "Usage(ChangeChatColor(chatType, r, g, b)",
        ),
        (
            r#"ChangeChatColor("SAY", 1, 1, {})"#,
            "Usage(ChangeChatColor(chatType, r, g, b)",
        ),
        (
            "PickupInventoryItem({})",
            "Invalid inventory slot in PickupInventoryItem",
        ),
        (
            "PickupInventoryItem(30)",
            "Invalid inventory slot in PickupInventoryItem",
        ),
        (
            "CursorCanGoInSlot({})",
            "Invalid inventory slot in CursorCanGoInSlot",
        ),
        (
            "CursorCanGoInSlot(30)",
            "Invalid inventory slot in CursorCanGoInSlot",
        ),
        (
            "UseInventoryItem({})",
            "Invalid inventory slot in UseInventoryItem",
        ),
        (
            "UseInventoryItem(30)",
            "Invalid inventory slot in UseInventoryItem",
        ),
        ("PutItemInBag({})", "Invalid bag slot in PutItemInBag"),
        ("PutItemInBag(19)", "Invalid bag slot in PutItemInBag"),
        ("PutItemInBag(30)", "Invalid bag slot in PutItemInBag"),
        (
            "PickupBagFromSlot({})",
            "Invalid bag slot in PickupBagFromSlot",
        ),
        (
            "PickupBagFromSlot(19)",
            "Invalid bag slot in PickupBagFromSlot",
        ),
        (
            "PickupBagFromSlot(30)",
            "Invalid bag slot in PickupBagFromSlot",
        ),
        (
            "IdxTip:SetTrainerService({})",
            "Invalid trainer service in SetTrainerService(index)",
        ),
        (
            "IdxTip:SetCraftSpell({})",
            "Invalid craft in SetCraftSpell(index)",
        ),
        (
            "IdxTip:SetQuestItem({}, 1)",
            r#"Invalid quest item in SetQuestItem("type", index)"#,
        ),
        (
            r#"IdxTip:SetQuestItem("reward", {})"#,
            r#"Invalid quest item in SetQuestItem("type", index)"#,
        ),
        (
            "IdxTip:SetQuestLogItem({}, 1)",
            r#"Invalid quest item in SetQuestLogItem("type", index)"#,
        ),
        (
            r#"IdxTip:SetQuestLogItem("reward", {})"#,
            r#"Invalid quest item in SetQuestLogItem("type", index)"#,
        ),
        (
            "IdxTip:SetLootItem({})",
            "Invalid loot slot in SetInventoryItem",
        ),
        (
            "IdxTip:SetTradeSkillItem({})",
            "Invalid trade skill item in SetTradeSkillItem(index [,reagent])",
        ),
        (
            "IdxTip:SetCraftItem({}, 1)",
            "Invalid craft item in SetCraftItem(index, reagent)",
        ),
        (
            "IdxTip:SetCraftItem(1, {})",
            "Invalid craft item in SetCraftItem(index, reagent)",
        ),
    ] {
        let e = s.run(call).expect_err(call).to_string();
        assert!(e.contains(&format!(":1: {usage}")), "{call}: {e}");
    }
}

/// A negative or zero index is a number, so it passes `is-number` and falls out of range as the
/// 32-bit pattern `_ftol` leaves; it never raises a conversion error.
#[test]
fn a_negative_index_is_read_and_never_a_conversion_error() {
    let mut s = script();
    // `SetLootThreshold` reads its argument only as a group's leader (`0x4e9500`).
    s.set_party(crate::script::PartyState {
        members: vec![crate::script::PartyMemberInfo {
            name: "Mate".into(),
            guid: 0x1,
        }],
        leader_guid: 0x5E1F,
        own_guid: 0x5E1F,
        ..Default::default()
    });
    s.run(r#"IdxTip = CreateFrame("GameTooltip", "IdxTip"); IdxTex = IdxTip:CreateTexture()"#)
        .unwrap();
    for call in [
        "AbandonSkill(-1)",
        r#"AddChatWindowChannel(-1, "x")"#,
        "AddChatWindowChannel(1, -1)",
        r#"AddChatWindowMessages(-1, "x")"#,
        "AddChatWindowMessages(1, -1)",
        "BuyMerchantItem(-1)",
        "BuyTrainerService(-1)",
        "BuybackItem(-1)",
        "CalculateAuctionDeposit(-1)",
        "CastPetAction(-1)",
        r#"CheckInteractDistance("player", -1)"#,
        "CheckInteractDistance(-1, 1)",
        "ClickStablePet(-1)",
        "ClickTargetTradeButton(-1)",
        "CollapseFactionHeader(-1)",
        "CollapseQuestHeader(-1)",
        "CollapseSkillHeader(-1)",
        "CollapseTradeSkillSubClass(-1)",
        "CollapseTrainerSkillLine(-1)",
        "DeleteMacro(-1)",
        "DoCraft(-1)",
        "DoTradeSkill(-1)",
        "EquipCursorItem(-1)",
        "EquipPendingItem(-1)",
        "ExpandFactionHeader(-1)",
        "ExpandQuestHeader(-1)",
        "ExpandSkillHeader(-1)",
        "ExpandTradeSkillSubClass(-1)",
        "ExpandTrainerSkillLine(-1)",
        "FactionToggleAtWar(-1)",
        "GetAuctionInvTypes(-1, 1)",
        "GetAuctionInvTypes(1, -1)",
        "GetAuctionItemSubClasses(-1)",
        "GetBagName(-1)",
        "GetChatWindowChannels(-1)",
        "GetChatWindowInfo(-1)",
        "GetChatWindowMessages(-1)",
        "GetContainerNumSlots(-1)",
        "GetCraftSpellFocus(-1)",
        "GetFriendInfo(-1)",
        "GetGuildRosterInfo(-1)",
        "GetGuildRosterLastOnline(-1)",
        "GetIgnoreName(-1)",
        "GetItemQualityColor(-1)",
        "GetLanguageByIndex(-1)",
        "GetMapLandmarkInfo(-1)",
        "GetMapOverlayInfo(-1)",
        "GetMapZones(-1)",
        "GetPetitionNameInfo(-1)",
        r#"GetQuestLogItemLink("x", -1)"#,
        "GetQuestLogItemLink(-1, 1)",
        "GetStablePetFoodTypes(-1)",
        "GetStablePetInfo(-1)",
        "GetTalentPrereqs(-1, 1)",
        "GetTalentPrereqs(1, -1)",
        "GetTradeSkillTools(-1)",
        "GetWhoInfo(-1)",
        "GiveMasterLoot(-1, 1)",
        "GiveMasterLoot(1, -1)",
        "GuildControlGetRankName(-1)",
        "GuildControlSetRank(-1)",
        "GuildControlSetRankFlag(-1)",
        "IdxTip:SetAction(-1)",
        r#"IdxTip:SetAuctionItem("list", -1)"#,
        "IdxTip:SetAuctionItem(-1, 1)",
        "IdxTip:SetInboxItem(-1)",
        "IdxTip:SetLootRollItem(-1)",
        "IdxTip:SetPetAction(-1)",
        "IdxTip:SetPlayerBuff(-1)",
        "IdxTip:SetShapeshift(-1)",
        "IdxTip:SetTalent(-1, 1)",
        "IdxTip:SetTalent(1, -1)",
        "InboxItemCanDelete(-1)",
        "LearnTalent(-1, 1)",
        "LearnTalent(1, -1)",
        "LootSlot(-1)",
        "LootSlotIsCoin(-1)",
        "LootSlotIsItem(-1)",
        "PickupAction(-1)",
        "PickupContainerItem(-1, 1)",
        "PickupContainerItem(1, -1)",
        "PickupMacro(-1)",
        "PickupPetAction(-1)",
        "PickupStablePet(-1)",
        "PlaceAction(-1)",
        r#"PlaceAuctionBid("list", -1, 1)"#,
        r#"PlaceAuctionBid("list", 1, -1)"#,
        "PlaceAuctionBid(-1, 1, 1)",
        r#"RemoveChatWindowChannel(-1, "x")"#,
        "RemoveChatWindowChannel(1, -1)",
        r#"RemoveChatWindowMessages(-1, "x")"#,
        "RemoveChatWindowMessages(1, -1)",
        "SelectCraft(-1)",
        "SelectGossipActiveQuest(-1)",
        "SelectGossipAvailableQuest(-1)",
        "SelectGossipOption(-1)",
        "SelectQuestLogEntry(-1)",
        "SelectTradeSkill(-1)",
        "SelectTrainerService(-1)",
        "SetBagPortaitTexture(IdxTex, -1)",
        "SetChatWindowAlpha(-1, 1)",
        "SetChatWindowAlpha(1, -1)",
        "SetChatWindowColor(-1, 1, 1, 1)",
        "SetChatWindowColor(1, -1, 1, 1)",
        "SetChatWindowColor(1, 1, -1, 1)",
        "SetChatWindowColor(1, 1, 1, -1)",
        "SetChatWindowDocked(-1)",
        "SetChatWindowLocked(-1)",
        "SetChatWindowName(-1)",
        "SetChatWindowShown(-1)",
        "SetChatWindowSize(-1, 1)",
        "SetChatWindowSize(1, -1)",
        "SetGuildRosterSelection(-1)",
        "SetLootThreshold(-1)",
        "SetMapZoom(-1)",
        r#"SetRaidTarget("player", -1)"#,
        "SetRaidTarget(-1, 1)",
        "SetSelectedFaction(-1)",
        "SetSelectedFriend(-1)",
        "SetSelectedIgnore(-1)",
        "SetSelectedSkill(-1)",
        "SetWatchedFactionIndex(-1)",
        "ShowBuybackSellCursor(-1)",
        "ShowContainerSellCursor(-1, 1)",
        "ShowContainerSellCursor(1, -1)",
        "ShowMerchantSellCursor(-1)",
        "SplitContainerItem(-1, 1, 1)",
        "SplitContainerItem(1, -1, 1)",
        "SplitContainerItem(1, 1, -1)",
        "TakeTaxiNode(-1)",
        "TaxiNodeCost(-1)",
        "TaxiNodeGetType(-1)",
        "TaxiNodeName(-1)",
        "TaxiNodePosition(-1)",
        "TaxiNodeSetCurrent(-1)",
        r#"UnitBuff("player", -1)"#,
        "UnitBuff(-1, 1)",
        r#"UnitDebuff("player", -1)"#,
        "UnitDebuff(-1, 1)",
        "UnstablePet(-1)",
        "UseAction(-1)",
        "UseContainerItem(-1, 1)",
        "UseContainerItem(1, -1)",
    ] {
        for call in [call.to_string(), call.replace("-1", "0")] {
            if let Err(e) = s.run(&call) {
                let e = e.to_string();
                assert!(
                    !e.contains("bad argument") && !e.contains("error converting"),
                    "{call}: {e}"
                );
            }
        }
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
    // `SetTradeSkillItem`'s reagent is absent when not a number (`0x533be7`).
    s.run(r#"CreateFrame("GameTooltip", "IdxTip2"):SetTradeSkillItem(1, {})"#)
        .unwrap();
}
