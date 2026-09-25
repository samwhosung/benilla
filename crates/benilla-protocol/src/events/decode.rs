//! The [`ServerPacket`] to [`SessionEvent`] mapping: pure functions of the packet, no state.

use crate::messages::{CastOutcome, MovementBlock, Object, ObjectFields, ObjectType, ServerPacket};
use crate::wire::Vector3d;

use super::{EntityKind, MoveSpeeds, SessionEvent};

fn v3(v: Vector3d) -> [f32; 3] {
    [v.x, v.y, v.z]
}

/// The caster of `SMSG_SPELL_START`/`SMSG_SPELL_GO`. vmangos writes the caster slot from
/// `m_casterUnit`, null for a GameObject caster (`Spell.cpp:102`, `Spell.cpp:4491`), so a
/// GameObject's spell (Lightwell, GO 181102) arrives as 0; the first slot then holds the caster
/// itself, as it does whenever no item is cast (`Spell.cpp:4487-4489`).
fn spell_caster(item_or_caster: u64, caster_slot: u64) -> u64 {
    if caster_slot == 0 {
        item_or_caster
    } else {
        caster_slot
    }
}

/// Decode one server packet into zero or more [`SessionEvent`]s. No wildcard arm: a new
/// `ServerPacket` variant must be a compile error here, never a silent no-op.
pub fn decode(packet: ServerPacket) -> Vec<SessionEvent> {
    match packet {
        ServerPacket::UpdateObject { objects } => decode_objects(objects),
        // In order, each move as if alone: the app rebuilds a mover's arc from packet order.
        ServerPacket::CompressedMoves { packets } => packets.into_iter().flat_map(decode).collect(),
        ServerPacket::CharEnum { characters } => vec![SessionEvent::CharacterList {
            characters,
            realm: None,
        }],
        ServerPacket::CharacterLoginFailed { result } => {
            vec![SessionEvent::CharacterLoginFailed { result }]
        }
        ServerPacket::LogoutComplete => vec![SessionEvent::LoggedOut],
        ServerPacket::LogoutResponse { reason, instant } => {
            vec![SessionEvent::LogoutResponse { reason, instant }]
        }
        ServerPacket::LogoutCancelAck => vec![SessionEvent::LogoutCancelled],
        ServerPacket::PlaySound { sound_id } => vec![SessionEvent::PlaySound { sound_id }],
        ServerPacket::PlayMusic { music_id } => vec![SessionEvent::PlayMusic { music_id }],
        ServerPacket::PlayObjectSound { sound_id, guid } => {
            vec![SessionEvent::PlayObjectSound { sound_id, guid }]
        }
        ServerPacket::Weather {
            weather_type,
            grade,
            sound_id,
            instant,
        } => vec![SessionEvent::Weather {
            weather_type,
            grade,
            sound_id,
            instant,
        }],
        ServerPacket::TextEmote {
            guid,
            text_emote,
            target_name,
        } => {
            vec![SessionEvent::TextEmote {
                guid,
                text_emote,
                target_name,
            }]
        }
        ServerPacket::Emote { guid, emote_id } => vec![SessionEvent::Emote { guid, emote_id }],
        ServerPacket::InitialSpells {
            spell_ids,
            cooldowns,
        } => vec![SessionEvent::SpellBook {
            spell_ids: spell_ids.into_iter().map(u32::from).collect(),
            cooldowns,
        }],
        ServerPacket::ActionButtons { buttons } => vec![SessionEvent::ActionButtons { buttons }],
        ServerPacket::LearnedSpell { spell_id } => vec![SessionEvent::SpellLearned {
            spell_id: u32::from(spell_id),
        }],
        ServerPacket::RemovedSpell { spell_id } => vec![SessionEvent::SpellRemoved {
            spell_id: u32::from(spell_id),
        }],
        ServerPacket::SupercededSpell {
            old_spell_id,
            new_spell_id,
        } => vec![SessionEvent::SpellSuperceded {
            old_spell_id: u32::from(old_spell_id),
            new_spell_id: u32::from(new_spell_id),
        }],
        ServerPacket::CastResult { spell_id, outcome } => vec![SessionEvent::CastResult {
            spell_id,
            success: outcome == CastOutcome::Ok,
            reason: match outcome {
                CastOutcome::Ok => None,
                CastOutcome::Failed { reason, .. } => Some(reason),
            },
            arg: match outcome {
                CastOutcome::Ok => None,
                CastOutcome::Failed { arg, .. } => arg,
            },
        }],
        ServerPacket::PetSpells(spells) => vec![SessionEvent::PetSpells(Box::new(spells))],
        ServerPacket::PetMode(mode) => vec![SessionEvent::PetMode(mode)],
        ServerPacket::PetActionFeedback { reason } => {
            vec![SessionEvent::PetActionFeedback { reason }]
        }
        ServerPacket::PetCastFailed { spell_id, outcome } => vec![SessionEvent::PetCastFailed {
            spell_id,
            reason: match outcome {
                CastOutcome::Ok => None,
                CastOutcome::Failed { reason, .. } => Some(reason),
            },
        }],
        ServerPacket::PetTameFailure { reason } => vec![SessionEvent::PetTameFailure { reason }],
        ServerPacket::PetNameInvalid => vec![SessionEvent::PetNameInvalid],
        ServerPacket::PetBroken => vec![SessionEvent::PetBroken],
        ServerPacket::PetActionSound { pet_guid, talk } => {
            vec![SessionEvent::PetActionSound { pet_guid, talk }]
        }
        ServerPacket::PetDismissSound { model_id, position } => {
            vec![SessionEvent::PetDismissSound {
                model_id,
                position: [position.x, position.y, position.z],
            }]
        }
        ServerPacket::ItemQueryResponse { entry, info } => {
            vec![SessionEvent::ItemTemplate { entry, info }]
        }
        ServerPacket::MessageChat(m) => vec![SessionEvent::Chat(m)],
        ServerPacket::ChannelNotify(n) => vec![SessionEvent::ChannelNotify {
            notice: n.notice,
            channel: n.channel,
            tail: n.tail,
        }],
        ServerPacket::ChannelList {
            channel,
            flags,
            members,
        } => vec![SessionEvent::ChannelList {
            channel,
            flags,
            members,
        }],
        ServerPacket::ChatPlayerNotFound { name } => {
            vec![SessionEvent::ChatPlayerNotFound { name }]
        }
        ServerPacket::ChatWrongFaction => vec![SessionEvent::ChatWrongFaction],
        ServerPacket::Notification { text } => vec![SessionEvent::Notification { text }],
        ServerPacket::AreaTriggerMessage { text } => {
            vec![SessionEvent::AreaTriggerMessage { text }]
        }
        ServerPacket::ServerMessage { message_type, text } => {
            vec![SessionEvent::ServerMessage { message_type, text }]
        }
        ServerPacket::ZoneUnderAttack { area_id } => {
            vec![SessionEvent::ZoneUnderAttack { area_id }]
        }
        ServerPacket::DefenseMessage { zone_id, text } => {
            vec![SessionEvent::DefenseMessage { zone_id, text }]
        }
        ServerPacket::ChatRestricted => vec![SessionEvent::ChatRestricted],
        ServerPacket::PlayedTime { total, level } => {
            vec![SessionEvent::PlayedTime { total, level }]
        }
        ServerPacket::RandomRoll {
            min,
            max,
            roll,
            guid,
        } => vec![SessionEvent::RandomRoll {
            min,
            max,
            roll,
            guid,
        }],
        ServerPacket::InventoryChangeFailure {
            reason,
            required_level,
            item_guid,
            bag_slot,
        } if reason != 0 => {
            vec![SessionEvent::InventoryFailure {
                reason,
                required_level,
                item_guid,
                bag_slot,
            }]
        }
        // Reason 0 is `EQUIP_ERR_OK`, which vmangos sends too (`Player.cpp:11688`): no failure.
        ServerPacket::InventoryChangeFailure { .. } => Vec::new(),
        ServerPacket::AttackStart { attacker, victim } => {
            vec![SessionEvent::AttackStart { attacker, victim }]
        }
        ServerPacket::AttackStop { attacker, victim } => {
            vec![SessionEvent::AttackStop { attacker, victim }]
        }
        ServerPacket::AttackerState(s) => vec![SessionEvent::AttackerState(s)],
        ServerPacket::AttackSwingError(e) => vec![SessionEvent::AttackSwingError(e)],
        ServerPacket::CancelCombat => vec![SessionEvent::CancelCombat],
        ServerPacket::FeignDeathResisted => vec![SessionEvent::FeignDeathResisted],
        ServerPacket::AiReaction { unit, reaction } => {
            vec![SessionEvent::AiReaction { unit, reaction }]
        }
        ServerPacket::SpellStart(s) => vec![SessionEvent::SpellStart {
            caster: spell_caster(s.item_or_caster, s.caster),
            spell_id: s.spell_id,
            cast_flags: s.cast_flags,
            cast_time_ms: s.cast_time_ms,
            target: s.targets.unit_target,
            ammo_display_id: s.ammo_display_id,
        }],
        ServerPacket::SpellGo(s) => {
            let caster = spell_caster(s.item_or_caster, s.caster);
            vec![SessionEvent::SpellGo {
                caster,
                spell_id: s.spell_id,
                cast_flags: s.cast_flags,
                hits: s.hits,
                misses: s.misses,
                target: s.targets.unit_target,
                go_target: s.targets.go_target,
                dest: s.targets.dest.map(|d| [d.x, d.y, d.z]),
                ammo_display_id: s.ammo_display_id,
                // Against the resolved caster: a GameObject's own guid in slot 1 is not an item.
                item_caster: (s.item_or_caster != caster).then_some(s.item_or_caster),
            }]
        }
        ServerPacket::SpellChainTargets(c) => vec![SessionEvent::SpellChainTargets {
            caster: c.caster,
            spell_id: c.spell_id,
            targets: c.targets,
        }],
        ServerPacket::SpellDelayed { caster, delay_ms } => {
            vec![SessionEvent::SpellDelayed { caster, delay_ms }]
        }
        ServerPacket::SpellFailedOther { caster, spell_id } => {
            vec![SessionEvent::SpellFailedOther { caster, spell_id }]
        }
        ServerPacket::CancelAutoRepeat => vec![SessionEvent::CancelAutoRepeat],
        ServerPacket::SpellCooldownList { caster, cooldowns } => {
            vec![SessionEvent::SpellCooldowns { caster, cooldowns }]
        }
        ServerPacket::ItemCooldown {
            item_guid,
            spell_id,
        } => vec![SessionEvent::ItemCooldown {
            item_guid,
            spell_id,
        }],
        ServerPacket::ItemTime { item_guid, seconds } => {
            vec![SessionEvent::ItemTime { item_guid, seconds }]
        }
        ServerPacket::ItemEnchantTime {
            item_guid,
            slot,
            seconds,
        } => vec![SessionEvent::ItemEnchantTime {
            item_guid,
            slot,
            seconds,
        }],
        ServerPacket::SpellModifier {
            flat,
            mask_bit,
            op,
            value,
        } => vec![SessionEvent::SpellModifier {
            flat,
            mask_bit,
            op,
            value,
        }],
        ServerPacket::CooldownEvent { spell_id, caster } => {
            vec![SessionEvent::CooldownEvent { spell_id, caster }]
        }
        ServerPacket::ClearCooldown { spell_id, caster } => {
            vec![SessionEvent::ClearCooldown { spell_id, caster }]
        }
        ServerPacket::CooldownCheat { caster } => vec![SessionEvent::CooldownCheat { caster }],
        ServerPacket::ChannelStart {
            spell_id,
            duration_ms,
        } => vec![SessionEvent::ChannelStart {
            spell_id,
            duration_ms,
        }],
        ServerPacket::ChannelUpdate { remaining_ms } => {
            vec![SessionEvent::ChannelUpdate { remaining_ms }]
        }
        ServerPacket::UpdateAuraDuration { slot, remaining_ms } => {
            vec![SessionEvent::AuraDuration { slot, remaining_ms }]
        }
        ServerPacket::PlaySpellVisual { unit, kit_id } => {
            vec![SessionEvent::PlaySpellVisual { unit, kit_id }]
        }
        ServerPacket::SpellDamageLog(s) => vec![SessionEvent::SpellDamageLog(s)],
        ServerPacket::PeriodicAuraLog(s) => vec![SessionEvent::PeriodicAuraLog(s)],
        ServerPacket::SpellHealLog(s) => vec![SessionEvent::SpellHealLog(s)],
        ServerPacket::SpellEnergizeLog(s) => vec![SessionEvent::SpellEnergizeLog(s)],
        ServerPacket::DamageShield(s) => vec![SessionEvent::DamageShield(s)],
        ServerPacket::EnvironmentalDamageLog(s) => vec![SessionEvent::EnvironmentalDamageLog(s)],
        ServerPacket::SpellLogMiss(s) => vec![SessionEvent::SpellLogMiss(s)],
        ServerPacket::PartyKillLog(s) => vec![SessionEvent::PartyKillLog(s)],
        ServerPacket::SpellInstaKillLog(s) => vec![SessionEvent::SpellInstaKillLog(s)],
        ServerPacket::ProcResist(s) => vec![SessionEvent::ProcResist(s)],
        ServerPacket::SpellOrDamageImmune(s) => vec![SessionEvent::SpellOrDamageImmune(s)],
        ServerPacket::SpellDispelLog(s) => vec![SessionEvent::SpellDispelLog(s)],
        ServerPacket::DispelFailed(s) => vec![SessionEvent::DispelFailed(s)],
        ServerPacket::EnchantmentLog(s) => vec![SessionEvent::EnchantmentLog(s)],
        ServerPacket::SpellLogExecute(s) => vec![SessionEvent::SpellLogExecute(s)],
        ServerPacket::XpGain(s) => vec![SessionEvent::XpGain(s)],
        ServerPacket::ExplorationXp(s) => vec![SessionEvent::ExplorationXp(s)],
        ServerPacket::LevelUp(l) => vec![SessionEvent::LevelUp(l)],
        ServerPacket::GossipMessage {
            npc,
            text_id,
            options,
            quests,
        } => vec![SessionEvent::GossipMenu {
            npc,
            text_id,
            options,
            quests: quests
                .into_iter()
                .map(|q| (q.quest_id, q.icon, q.level, q.title))
                .collect(),
        }],
        ServerPacket::QuestGiverStatus { npc, status } => {
            vec![SessionEvent::QuestGiverStatus { npc, status }]
        }
        ServerPacket::QuestGiverQuestList(l) => vec![SessionEvent::QuestGreeting(l)],
        ServerPacket::QuestGiverDetails(d) => vec![SessionEvent::QuestDetail(d)],
        ServerPacket::QuestGiverRequestItems(p) => vec![SessionEvent::QuestProgress(p)],
        ServerPacket::QuestGiverOfferReward(o) => vec![SessionEvent::QuestOffer(o)],
        ServerPacket::QuestGiverComplete(c) => vec![SessionEvent::QuestComplete(c)],
        ServerPacket::QuestQueryResponse(t) => vec![SessionEvent::QuestTemplate(t)],
        ServerPacket::QuestUpdateAddKill {
            quest_id,
            entry,
            count,
            required,
            guid: _, // the killed unit; the toast keys on the entry
        } => vec![SessionEvent::QuestObjectiveKill {
            quest_id,
            entry,
            count,
            required,
        }],
        ServerPacket::QuestUpdateAddItem { item_id, count } => {
            vec![SessionEvent::QuestObjectiveItem { item_id, count }]
        }
        ServerPacket::QuestUpdateComplete { quest_id } => {
            vec![SessionEvent::QuestObjectivesComplete { quest_id }]
        }
        ServerPacket::QuestUpdateFailed { quest_id } => vec![SessionEvent::QuestFailed {
            quest_id,
            timed: false,
        }],
        ServerPacket::QuestUpdateFailedTimer { quest_id } => vec![SessionEvent::QuestFailed {
            quest_id,
            timed: true,
        }],
        ServerPacket::QuestLogFull => vec![SessionEvent::QuestLogFull],
        ServerPacket::QuestPushResult(r) => vec![SessionEvent::QuestPushResult {
            member: r.member,
            msg: r.msg,
        }],
        ServerPacket::QuestConfirmAccept(c) => vec![SessionEvent::QuestConfirmAccept(c)],
        ServerPacket::QuestGiverInvalid { msg } => {
            vec![SessionEvent::QuestGiverInvalid { reason: msg }]
        }
        ServerPacket::QuestGiverFailed { quest_id, reason } => {
            vec![SessionEvent::QuestGiverFailed { quest_id, reason }]
        }
        ServerPacket::GossipComplete => vec![SessionEvent::GossipComplete],
        ServerPacket::GossipPoi(poi) => vec![SessionEvent::GossipPoi(poi)],
        ServerPacket::NpcText { text_id, blocks } => {
            vec![SessionEvent::NpcGreeting { text_id, blocks }]
        }
        ServerPacket::VendorList { vendor, items } => {
            vec![SessionEvent::VendorInventory { vendor, items }]
        }
        ServerPacket::TrainerList {
            trainer,
            trainer_type,
            services,
            title,
        } => vec![SessionEvent::TrainerList {
            trainer,
            trainer_type,
            services,
            greeting: title,
        }],
        ServerPacket::InvalidatePlayer { guid } => vec![SessionEvent::InvalidatePlayer { guid }],
        ServerPacket::ListStabledPets {
            npc,
            num_stable_slots,
            pets,
        } => vec![SessionEvent::ListStabledPets {
            npc,
            num_stable_slots,
            pets,
        }],
        ServerPacket::StableResult { result } => vec![SessionEvent::StableResult { result }],
        ServerPacket::TrainerBuySucceeded { trainer, spell_id } => {
            vec![SessionEvent::TrainerBuySucceeded { trainer, spell_id }]
        }
        ServerPacket::TrainerBuyFailed {
            trainer,
            spell_id,
            error,
        } => vec![SessionEvent::TrainerBuyFailed {
            trainer,
            spell_id,
            error,
        }],
        ServerPacket::BuyItem {
            vendor,
            slot,
            new_count,
            purchase_count,
        } => vec![SessionEvent::VendorBuyResult {
            vendor,
            slot,
            new_count,
            purchase_count,
        }],
        ServerPacket::SellItemResult {
            vendor,
            item_guid,
            reason,
        } => vec![SessionEvent::VendorSellFailed {
            vendor,
            item_guid,
            reason,
        }],
        ServerPacket::BuyFailed {
            vendor,
            item_entry,
            reason,
        } => vec![SessionEvent::VendorBuyFailed {
            vendor,
            item_entry,
            reason,
        }],
        ServerPacket::ShowBank { banker } => vec![SessionEvent::ShowBank { banker }],
        ServerPacket::BuyBankSlotResult { result } => {
            vec![SessionEvent::BuyBankSlotResult { result }]
        }
        ServerPacket::LootResponse {
            guid,
            loot_type,
            gold,
            items,
        } => vec![SessionEvent::LootResponse {
            guid,
            loot_type,
            gold,
            items,
        }],
        ServerPacket::LootError { guid, error } => vec![SessionEvent::LootError { guid, error }],
        ServerPacket::LootReleaseResponse { guid, .. } => {
            vec![SessionEvent::LootReleaseResponse { guid }]
        }
        ServerPacket::LootRemoved { slot } => vec![SessionEvent::LootRemoved { slot }],
        ServerPacket::LootMoneyNotify { amount } => {
            vec![SessionEvent::LootMoneyNotify { amount }]
        }
        ServerPacket::LootClearMoney => vec![SessionEvent::LootClearMoney],
        ServerPacket::LootStartRoll(p) => vec![SessionEvent::LootStartRoll(p)],
        ServerPacket::LootRoll(p) => vec![SessionEvent::LootRoll(p)],
        ServerPacket::LootRollWon(p) => vec![SessionEvent::LootRollWon(p)],
        ServerPacket::LootAllPassed(p) => vec![SessionEvent::LootAllPassed(p)],
        ServerPacket::LootMasterList { candidates } => {
            vec![SessionEvent::LootMasterList { candidates }]
        }
        ServerPacket::ItemPushResult(p) => vec![SessionEvent::ItemPushResult(p)],
        ServerPacket::CorpseQuery(loc) => vec![SessionEvent::CorpseQuery {
            found: loc.found,
            display_map: loc.display_map,
            position: loc.position,
            corpse_map: loc.corpse_map,
        }],
        ServerPacket::CorpseReclaimDelay { delay_ms } => {
            vec![SessionEvent::CorpseReclaimDelay { delay_ms }]
        }
        ServerPacket::DurabilityDamageDeath => vec![SessionEvent::DurabilityDamageDeath],
        ServerPacket::ResurrectRequest(r) => vec![SessionEvent::ResurrectRequest {
            caster: r.caster,
            name: r.name,
            sickness: r.sickness,
            has_timer: r.has_timer,
        }],
        ServerPacket::SpiritHealerConfirm { npc } => {
            vec![SessionEvent::SpiritHealerConfirm { npc }]
        }
        ServerPacket::MoveMode {
            guid,
            counter,
            mode,
            apply,
        } => vec![SessionEvent::MoveMode {
            guid,
            counter,
            mode,
            apply,
        }],
        ServerPacket::SplineMoveMode { guid, mode, apply } => {
            vec![SessionEvent::SplineMoveMode { guid, mode, apply }]
        }
        ServerPacket::KnockBack {
            guid,
            counter,
            launch,
        } => vec![SessionEvent::KnockBack {
            guid,
            counter,
            launch,
        }],
        ServerPacket::GroupInvite { inviter } => vec![SessionEvent::GroupInvite { inviter }],
        ServerPacket::GroupDecline { name } => vec![SessionEvent::GroupDecline { name }],
        ServerPacket::GroupUninvited => vec![SessionEvent::GroupUninvited],
        ServerPacket::GroupLeaderChanged { name } => {
            vec![SessionEvent::GroupLeaderChanged { name }]
        }
        ServerPacket::GroupDestroyed => vec![SessionEvent::GroupDestroyed],
        ServerPacket::GroupList {
            group_type,
            own_flags,
            members,
            leader,
            loot,
        } => vec![SessionEvent::GroupList {
            group_type,
            own_flags,
            members,
            leader,
            loot,
        }],
        ServerPacket::PartyCommandResult {
            operation,
            member,
            result,
        } => vec![SessionEvent::PartyCommandResult {
            operation,
            member,
            result,
        }],
        ServerPacket::PartyMemberStats { guid, full, info } => {
            vec![SessionEvent::PartyMemberStats { guid, full, info }]
        }
        ServerPacket::MinimapPing { guid, x, y } => {
            vec![SessionEvent::MinimapPing { guid, x, y }]
        }
        ServerPacket::RaidTargetSet { icon, guid } => {
            vec![SessionEvent::RaidTargetSet { icon, guid }]
        }
        ServerPacket::RaidTargetList { entries } => {
            vec![SessionEvent::RaidTargetList { entries }]
        }
        ServerPacket::ReadyCheckRequest => vec![SessionEvent::ReadyCheckRequest],
        ServerPacket::RaidInstanceInfo { entries } => {
            vec![SessionEvent::RaidInstanceInfo { entries }]
        }
        ServerPacket::ReadyCheckAnswer { guid, ready } => {
            vec![SessionEvent::ReadyCheckAnswer { guid, ready }]
        }
        ServerPacket::DuelRequested {
            arbiter,
            challenger,
        } => vec![SessionEvent::DuelRequested {
            arbiter,
            challenger,
        }],
        // Lockouts; ownership narrows to a bool: the reference reads it with `test eax,eax`.
        ServerPacket::RaidInstanceMessage { message } => {
            vec![SessionEvent::RaidInstanceMessage { message }]
        }
        ServerPacket::InstanceSaveCreated { flag } => {
            vec![SessionEvent::InstanceSaveCreated { flag }]
        }
        ServerPacket::InstanceReset { map } => vec![SessionEvent::InstanceReset { map }],
        ServerPacket::InstanceResetFailed { failure } => {
            vec![SessionEvent::InstanceResetFailed { failure }]
        }
        ServerPacket::UpdateLastInstance { map } => vec![SessionEvent::UpdateLastInstance { map }],
        ServerPacket::UpdateInstanceOwnership { owns } => {
            vec![SessionEvent::UpdateInstanceOwnership { owns: owns != 0 }]
        }
        ServerPacket::DuelOutOfBounds => vec![SessionEvent::DuelOutOfBounds],
        ServerPacket::DuelInBounds => vec![SessionEvent::DuelInBounds],
        ServerPacket::DuelComplete { started } => vec![SessionEvent::DuelComplete { started }],
        ServerPacket::DuelWinner {
            fled,
            winner,
            loser,
        } => vec![SessionEvent::DuelWinner {
            fled,
            winner,
            loser,
        }],
        ServerPacket::DuelCountdown { seconds } => vec![SessionEvent::DuelCountdown { seconds }],
        ServerPacket::InspectHonorStats(stats) => vec![SessionEvent::InspectHonorStats(stats)],
        ServerPacket::PvpCredit(credit) => vec![SessionEvent::PvpCredit(credit)],
        ServerPacket::MirrorTimerStart(start) => vec![SessionEvent::MirrorTimerStart(start)],
        ServerPacket::MirrorTimerPause { kind, paused } => {
            vec![SessionEvent::MirrorTimerPause { kind, paused }]
        }
        ServerPacket::MirrorTimerStop { kind } => vec![SessionEvent::MirrorTimerStop { kind }],
        ServerPacket::FriendList { friends } => vec![SessionEvent::FriendList { friends }],
        ServerPacket::IgnoreList { guids } => vec![SessionEvent::IgnoreList { guids }],
        ServerPacket::FriendStatus(status) => vec![SessionEvent::FriendStatus(status)],
        ServerPacket::WhoResults(results) => vec![SessionEvent::WhoResults(results)],
        ServerPacket::GuildQueryResponse(response) => {
            vec![SessionEvent::GuildQueryResponse(response)]
        }
        ServerPacket::GuildRoster(roster) => vec![SessionEvent::GuildRoster(roster)],
        ServerPacket::GuildEvent(notice) => vec![SessionEvent::GuildEvent(notice)],
        ServerPacket::GuildCommandResult(result) => vec![SessionEvent::GuildCommandResult(result)],
        ServerPacket::GuildInvite { inviter, guild } => {
            vec![SessionEvent::GuildInvite { inviter, guild }]
        }
        ServerPacket::GuildDecline { name } => vec![SessionEvent::GuildDecline { name }],
        ServerPacket::GuildInfo(info) => vec![SessionEvent::GuildInfo(info)],
        ServerPacket::PetitionShowList(list) => vec![SessionEvent::PetitionShowList(list)],
        ServerPacket::PetitionShowSignatures(sigs) => {
            vec![SessionEvent::PetitionShowSignatures(sigs)]
        }
        ServerPacket::PetitionSignResults(results) => {
            vec![SessionEvent::PetitionSignResults(results)]
        }
        ServerPacket::PetitionQueryResponse(response) => {
            vec![SessionEvent::PetitionQueryResponse(response)]
        }
        ServerPacket::TurnInPetitionResults { result } => {
            vec![SessionEvent::TurnInPetitionResults { result }]
        }
        ServerPacket::PetitionDeclined { player } => {
            vec![SessionEvent::PetitionDeclined { player }]
        }
        ServerPacket::PetitionRenamed(rename) => vec![SessionEvent::PetitionRenamed(rename)],
        ServerPacket::DestroyObject { guid } => vec![SessionEvent::ObjectDestroyed(guid)],
        ServerPacket::TriggerCinematic { cinematic_id } => {
            vec![SessionEvent::CinematicTriggered { cinematic_id }]
        }
        ServerPacket::MoveTimeSkipped { guid, lag_ms } => {
            vec![SessionEvent::MoveTimeSkipped { guid, lag_ms }]
        }
        ServerPacket::MonsterMove {
            guid,
            transport,
            start,
            spline_id,
            path,
            facing,
            stop,
            duration_ms,
            flying,
            run_mode,
        } => vec![SessionEvent::MonsterMove {
            guid,
            transport,
            start: v3(start),
            spline_id,
            path: path.into_iter().map(v3).collect(),
            facing,
            stop,
            duration_ms,
            flying,
            run_mode,
        }],
        ServerPacket::PlayerMove {
            guid,
            opcode,
            flags,
            position,
            orientation,
            pitch,
            time,
            fall_time,
            jump,
            transport,
        } => vec![SessionEvent::UnitMove {
            guid,
            position: v3(position),
            orientation,
            flags,
            pitch,
            time,
            verb: crate::messages::RelayVerb::of(opcode),
            fall_time,
            jump,
            transport,
        }],
        ServerPacket::Teleport {
            guid,
            counter,
            position,
            orientation,
        } => vec![SessionEvent::Teleport {
            guid,
            counter,
            position: v3(position),
            orientation,
        }],
        ServerPacket::NewWorld {
            map,
            position,
            orientation,
        } => vec![SessionEvent::Worldport {
            map_id: map,
            position: v3(position),
            orientation,
            needs_ack: true,
        }],
        ServerPacket::LoginVerifyWorld {
            map,
            position,
            orientation,
        } => vec![SessionEvent::Worldport {
            map_id: map,
            position: v3(position),
            orientation,
            needs_ack: false,
        }],
        ServerPacket::TransferPending { map, transport } => vec![SessionEvent::TransferPending {
            map_id: map,
            transport_entry: transport.map(|(entry, _old_map)| entry),
        }],
        ServerPacket::TransferAborted { reason } => {
            vec![SessionEvent::TransferAborted { reason }]
        }
        ServerPacket::TimeSpeed {
            hours,
            minutes,
            day_serial,
            timescale,
        } => vec![SessionEvent::TimeSpeed {
            hours,
            minutes,
            day_serial,
            timescale,
        }],
        ServerPacket::QueryTimeResponse { unix_time } => {
            vec![SessionEvent::ServerUnixTime { unix_time }]
        }
        ServerPacket::BindPoint { area, .. } => vec![SessionEvent::BindPoint { area }],
        ServerPacket::GmTicketAnswer { ticket } => vec![SessionEvent::GmTicket { ticket }],
        ServerPacket::GmTicketCreated { response } => {
            vec![SessionEvent::GmTicketCreated { response }]
        }
        ServerPacket::GmTicketUpdated { response } => {
            vec![SessionEvent::GmTicketUpdated { response }]
        }
        ServerPacket::GmTicketDeleted { response } => {
            vec![SessionEvent::GmTicketDeleted { response }]
        }
        ServerPacket::GmTicketSystemStatus { status } => {
            vec![SessionEvent::GmTicketSystemStatus { status }]
        }
        ServerPacket::GmTicketStatusUpdate { status } => {
            vec![SessionEvent::GmTicketStatusUpdate { status }]
        }
        ServerPacket::BinderConfirm { binder } => vec![SessionEvent::BinderConfirm { binder }],
        ServerPacket::SummonRequest {
            summoner,
            zone,
            delay_ms,
        } => vec![SessionEvent::SummonRequest {
            summoner,
            zone,
            delay_ms,
        }],
        ServerPacket::TalentWipeConfirm { trainer, cost } => {
            vec![SessionEvent::TalentWipeConfirm { trainer, cost }]
        }
        ServerPacket::PetUnlearnConfirm { trainer, cost } => {
            vec![SessionEvent::PetUnlearnConfirm { trainer, cost }]
        }
        ServerPacket::RaidGroupOnly { delay_ms, reason } => {
            vec![SessionEvent::RaidGroupOnly { delay_ms, reason }]
        }
        ServerPacket::AreaSpiritHealerTime { healer, ms } => {
            vec![SessionEvent::AreaSpiritHealerTime { healer, ms }]
        }
        ServerPacket::BattlefieldStatus(status) => vec![SessionEvent::BattlefieldStatus(status)],
        ServerPacket::PvpLogData(data) => vec![SessionEvent::PvpLogData(data)],
        ServerPacket::BattlefieldList(list) => vec![SessionEvent::BattlefieldList(list)],
        ServerPacket::BattlefieldPositions(p) => vec![SessionEvent::BattlefieldPositions(p)],
        ServerPacket::TabardVendorActivate(g) => vec![SessionEvent::TabardVendorActivate(g)],
        ServerPacket::SaveGuildEmblemResult(r) => vec![SessionEvent::SaveGuildEmblemResult(r)],
        ServerPacket::GroupJoinedBattleground { result } => {
            vec![SessionEvent::GroupJoinedBattleground { result }]
        }
        ServerPacket::BattlegroundPlayer { guid, joined } => {
            vec![SessionEvent::BattlegroundPlayer { guid, joined }]
        }
        ServerPacket::MeetingStoneSetQueue { area, status } => {
            vec![SessionEvent::MeetingStoneSetQueue { area, status }]
        }
        ServerPacket::MeetingStoneNotice(notice) => vec![SessionEvent::MeetingStoneNotice(notice)],
        ServerPacket::TutorialFlags(flags) => vec![SessionEvent::TutorialFlags(flags.bytes)],
        ServerPacket::PlayerBound { binder, area } => {
            vec![SessionEvent::PlayerBound { binder, area }]
        }
        ServerPacket::SetProficiency {
            item_class,
            subclass_mask,
        } => vec![SessionEvent::Proficiency {
            item_class: u32::from(item_class),
            subclass_mask,
        }],
        ServerPacket::InitializeFactions { standings } => {
            vec![SessionEvent::Reputations { standings }]
        }
        ServerPacket::SetFactionStanding { standings } => {
            vec![SessionEvent::ReputationDelta { standings }]
        }
        ServerPacket::SetFactionVisible { list_id } => {
            vec![SessionEvent::ReputationVisible { list_id }]
        }
        ServerPacket::NameQueryResponse {
            guid,
            name,
            race,
            gender,
            class,
        } => vec![SessionEvent::PlayerName {
            guid,
            name,
            race,
            gender,
            class,
        }],
        ServerPacket::PetNameQueryResponse { pet_number, name } => {
            vec![SessionEvent::PetName { pet_number, name }]
        }
        ServerPacket::CreatureQueryResponse { entry, info } => {
            let (
                name,
                subname,
                creature_type,
                pet_family,
                rank,
                type_flags,
                display_id,
                civilian,
                racial_leader,
            ) = match info {
                Some(i) => (
                    Some(i.name),
                    // An empty subname is none: the reference draws no line for it (`0x608f50`).
                    Some(i.subname).filter(|s| !s.is_empty()),
                    Some(i.creature_type),
                    // Not an `Option`: `CreatureFamily.dbc` has no row 0, so 0 means none.
                    i.pet_family,
                    i.rank,
                    i.type_flags,
                    i.display_id,
                    i.civilian,
                    i.racial_leader,
                ),
                None => (None, None, None, 0, 0, 0, 0, false, false),
            };
            vec![SessionEvent::CreatureName {
                entry,
                name,
                subname,
                creature_type,
                pet_family,
                rank,
                type_flags,
                display_id,
                civilian,
                racial_leader,
            }]
        }
        ServerPacket::PageTextQueryResponse {
            page_id,
            text,
            next_page_id,
        } => vec![SessionEvent::PageText {
            page_id,
            text,
            next_page_id,
        }],
        ServerPacket::GameObjectQueryResponse { entry, info } => {
            let event = match info {
                Some(i) => SessionEvent::GameObjectInfo {
                    entry,
                    type_id: i.type_id,
                    display_id: i.display_id,
                    name: i.name,
                    data: i.data,
                },
                None => SessionEvent::GameObjectInfo {
                    entry,
                    type_id: 0,
                    display_id: 0,
                    name: String::new(),
                    data: [0; 24],
                },
            };
            vec![event]
        }
        ServerPacket::GameObjectCustomAnim { guid, anim_id } => {
            vec![SessionEvent::GameObjectCustomAnim { guid, anim_id }]
        }
        ServerPacket::GameObjectDespawnAnim { guid } => {
            vec![SessionEvent::GameObjectDespawnAnim { guid }]
        }
        ServerPacket::OpenContainer { item } => vec![SessionEvent::OpenContainer { item }],
        ServerPacket::StandStateUpdate { state } => {
            vec![SessionEvent::StandStateUpdate { state }]
        }
        // Nothing: the reference's `0x5e7d70` discards the echoed guid, and the inspect window
        // reads the target's `PLAYER_VISIBLE_ITEM_*` fields. Parsed so no tally calls it dropped.
        ServerPacket::Inspect { .. } => Vec::new(),
        ServerPacket::FishNotHooked => vec![SessionEvent::FishNotHooked],
        ServerPacket::FishEscaped => vec![SessionEvent::FishEscaped],
        ServerPacket::Pong { sequence } => vec![SessionEvent::Pong { sequence }],
        ServerPacket::ForceSpeedChange {
            guid,
            kind,
            counter,
            speed,
        } => vec![SessionEvent::ForceSpeedChange {
            guid,
            kind,
            counter,
            speed,
        }],
        ServerPacket::SplineSpeedChange { guid, kind, speed } => {
            vec![SessionEvent::SpeedChanged { guid, kind, speed }]
        }
        // `MSG_MOVE_SET_*_SPEED` is a speed and a fresh pose: emit both, for the same drain.
        ServerPacket::MoveSetSpeed {
            guid,
            kind,
            flags,
            position,
            orientation,
            pitch,
            time,
            fall_time,
            jump,
            transport,
            speed,
        } => vec![
            SessionEvent::UnitMove {
                guid,
                position: v3(position),
                orientation,
                flags,
                pitch,
                time,
                // The opcode's meaning is the speed, carried by its own event: the pose is plain.
                verb: crate::messages::RelayVerb::Pose,
                fall_time,
                jump,
                transport,
            },
            SessionEvent::SpeedChanged { guid, kind, speed },
        ],
        ServerPacket::MountResult { mount, code } => {
            vec![SessionEvent::MountResult { mount, code }]
        }
        ServerPacket::MountSpecialAnim { guid } => {
            vec![SessionEvent::MountSpecial { guid }]
        }
        ServerPacket::ClientControlUpdate { mover, allow_move } => {
            vec![SessionEvent::ClientControl { mover, allow_move }]
        }
        ServerPacket::ShowTaxiNodes {
            window,
            flightmaster,
            nearest_node,
            known,
        } => {
            // The reference parser gates on this word: zero carries no menu; vmangos sends 1.
            if window == 0 {
                vec![]
            } else {
                vec![SessionEvent::TaxiNodesShown {
                    flightmaster,
                    nearest_node,
                    known_mask: known,
                }]
            }
        }
        ServerPacket::TaxiNodeStatus { guid, known } => {
            vec![SessionEvent::TaxiNodeStatus { guid, known }]
        }
        ServerPacket::ActivateTaxiReply { code } => {
            vec![SessionEvent::ActivateTaxiReply { code }]
        }
        ServerPacket::NewTaxiPath => vec![SessionEvent::NewTaxiPath],
        ServerPacket::MailList { mails } => vec![SessionEvent::MailList { mails }],
        ServerPacket::SendMailResult {
            mail_id,
            action,
            error,
            equip_error,
            item,
        } => vec![SessionEvent::SendMailResult {
            mail_id,
            action,
            error,
            equip_error,
            item,
        }],
        ServerPacket::ItemTextQueryResponse { text_id, text } => {
            vec![SessionEvent::MailItemText { text_id, text }]
        }
        ServerPacket::ReceivedMail { seconds } => vec![SessionEvent::ReceivedMail { seconds }],
        ServerPacket::NextMailTime { seconds } => {
            vec![SessionEvent::NextMailTime { seconds }]
        }
        ServerPacket::AuctionHello {
            auctioneer,
            house_id,
        } => vec![SessionEvent::AuctionHello {
            auctioneer,
            house_id,
        }],
        ServerPacket::AuctionCommandResult {
            auction_id,
            action,
            error,
            tail,
        } => vec![SessionEvent::AuctionCommandResult {
            auction_id,
            action,
            error,
            tail,
        }],
        ServerPacket::AuctionListResult {
            auctions,
            total_count,
        } => vec![SessionEvent::AuctionListResult {
            auctions,
            total_count,
        }],
        ServerPacket::AuctionOwnerListResult {
            auctions,
            total_count,
        } => vec![SessionEvent::AuctionOwnerListResult {
            auctions,
            total_count,
        }],
        ServerPacket::AuctionBidderListResult {
            auctions,
            total_count,
        } => vec![SessionEvent::AuctionBidderListResult {
            auctions,
            total_count,
        }],
        ServerPacket::AuctionBidderNotification(n) => {
            vec![SessionEvent::AuctionBidderNotification(n)]
        }
        ServerPacket::AuctionOwnerNotification(n) => {
            vec![SessionEvent::AuctionOwnerNotification(n)]
        }
        ServerPacket::AuctionRemovedNotification {
            auction_id,
            item_entry,
            random_property_id,
        } => vec![SessionEvent::AuctionRemovedNotification {
            auction_id,
            item_entry,
            random_property_id,
        }],
        ServerPacket::TradeStatus { status } => vec![SessionEvent::TradeStatus { status }],
        ServerPacket::TradeStatusExtended { state } => {
            vec![SessionEvent::TradeStatusExtended { state }]
        }
        ServerPacket::InitWorldStates(w) => vec![SessionEvent::WorldStates {
            scope: Some((w.map, w.zone)),
            states: w.states,
        }],
        ServerPacket::UpdateWorldState { id, value } => vec![SessionEvent::WorldStates {
            scope: None,
            states: vec![(id, value)],
        }],
        // No parse arm at all: surfaced for the app's dropped-opcode tally.
        ServerPacket::Other { opcode } => vec![SessionEvent::PacketDropped {
            opcode,
            unparseable: false,
        }],
        // Handshake-only: `world::session` consumes these before the world loop reaches here.
        ServerPacket::AuthChallenge { .. }
        | ServerPacket::AuthResponse { .. }
        | ServerPacket::CharCreate { .. }
        | ServerPacket::CharDelete { .. }
        | ServerPacket::AddonInfo { .. } => Vec::new(),
    }
}

/// Fan an object-update list out into create/move/remove/values events, moving each mask.
fn decode_objects(objects: Vec<Object>) -> Vec<SessionEvent> {
    let mut out = Vec::new();
    for object in objects {
        match object {
            Object::Create {
                guid,
                object_type,
                movement,
                mask,
            } => {
                // Items have no pose: route them before the placement gate, which would drop them.
                if matches!(object_type, ObjectType::Item | ObjectType::Container) {
                    out.push(SessionEvent::ItemCreate {
                        guid,
                        container: object_type == ObjectType::Container,
                        fields: mask,
                    });
                    continue;
                }
                if let Some((position, orientation)) =
                    create_placement(object_type, &mask, &movement)
                {
                    let display = display_id(object_type, &mask);
                    let scale = object_scale(object_type, &mask);
                    let speeds = movement.speeds.map(MoveSpeeds::from_wire);
                    out.push(SessionEvent::ObjectCreate {
                        guid,
                        kind: entity_kind(object_type),
                        display_id: display,
                        position: v3(position),
                        orientation,
                        scale,
                        speeds,
                        mover: movement.mover,
                        transport_progress: movement.transport_progress,
                        transport: movement.transport,
                        spline: movement.spline,
                        fields: mask,
                    });
                }
            }
            Object::Movement { guid, movement } => {
                if let Some((position, orientation)) = movement.position {
                    out.push(SessionEvent::ObjectMove {
                        guid,
                        position: v3(position),
                        orientation,
                    });
                }
            }
            Object::OutOfRange { guids } => {
                out.push(SessionEvent::ObjectsRemoved(guids));
            }
            // `Values`: a descriptor delta with no position.
            Object::Values { guid, mask } => {
                if !mask.is_empty() {
                    out.push(SessionEvent::ObjectValues { guid, fields: mask });
                }
            }
            // `Near` is a guid pre-announce; no state.
            Object::Near { .. } => {}
        }
    }
    out
}

fn entity_kind(t: ObjectType) -> EntityKind {
    match t {
        ObjectType::Player => EntityKind::Player,
        ObjectType::Unit => EntityKind::Unit,
        ObjectType::GameObject => EntityKind::GameObject,
        ObjectType::DynamicObject => EntityKind::DynamicObject,
        ObjectType::Corpse => EntityKind::Corpse,
        _ => EntityKind::Other,
    }
}

/// A create's display id. A player resolves like a unit (the 1.12 client's `CGPlayer` inherits
/// `CGUnit`'s display build); a corpse's is the player's native display (reference: `0x5d6700`).
/// A bone pile's id is still reported: `entities::corpse` swaps in `<Race><Sex>DeathSkeleton`,
/// and `None` would hide the flesh-to-bones flip the reference reloads the model on.
fn display_id(t: ObjectType, mask: &ObjectFields) -> Option<u32> {
    match t {
        ObjectType::Unit | ObjectType::Player => {
            mask.unit_displayid().filter(|&d| d > 0).map(|d| d as u32)
        }
        ObjectType::GameObject => mask
            .gameobject_displayid()
            .filter(|&d| d > 0)
            .map(|d| d as u32),
        ObjectType::Corpse => mask.corpse_display_id(),
        _ => None,
    }
}

/// `OBJECT_FIELD_SCALE_X`, the complete render scale: the reference sizes by it alone (`0x613ef0`),
/// and vmangos already folds the display scale in (`Unit::GetScaleForDisplayId`).
fn object_scale(t: ObjectType, mask: &ObjectFields) -> f32 {
    let raw = match t {
        // vmangos sets a corpse's scale too (`Corpse::Create` → `SetObjectScale`).
        ObjectType::Unit | ObjectType::Player | ObjectType::GameObject | ObjectType::Corpse => {
            mask.object_scale_x()
        }
        _ => None,
    };
    raw.filter(|s| *s > 0.0).unwrap_or(1.0)
}

/// A create's pose: a GameObject's `GAMEOBJECT_POS_*` when sent, else the movement block, since
/// vmangos never sets those fields on a transport. Gate on `gameobject_pos_sent`, not `.or`: a
/// create reads absent fields as zero, so `gameobject_position()` is `Some` even when unsent.
fn create_placement(
    object_type: ObjectType,
    mask: &ObjectFields,
    movement: &MovementBlock,
) -> Option<(Vector3d, f32)> {
    if object_type == ObjectType::GameObject && mask.gameobject_pos_sent() {
        mask.gameobject_position()
    } else {
        movement.position
    }
}
