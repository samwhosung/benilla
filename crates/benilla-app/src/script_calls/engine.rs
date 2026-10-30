//! The engine verbs inside their calls: the follow's start (`0x60fed0`), whose arm signals where
//! it runs, and the getters' lookups with a callback ([`RecordAsks`]).

use bevy::prelude::*;

use benilla_ui::script::{FollowRequest, ItemCallback, RecordAsk, ScriptValue, UiScript};

use super::ScriptCallHost;
use crate::target::FollowStart;
use crate::ui_cast::ReportMark;

impl ScriptCallHost<'_, '_> {
    /// `FollowUnit(unit)` (`0x489e00`) or `FollowByName(name, exact)` (`0x489ec0`), then
    /// `StartAutoMoveToObject(guid, 3)` (`0x60fed0`): the gate's line shown in place
    /// (`0x496720`), or the arm, whose stop of the auto-move it replaces signals `AUTOFOLLOW_END`
    /// for a follow (`0x61113e` → `0x60fb84`) before that move stops and the new one is stored
    /// (`0x611130`), then `AUTOFOLLOW_BEGIN` with the followee's name (`0x610071`), each running
    /// its listeners where it fires.
    pub(super) fn follow(
        &mut self,
        script: &mut UiScript,
        mark: &mut ReportMark,
        request: FollowRequest,
    ) {
        let request = match request {
            FollowRequest::ByUnit(token) => crate::player::FollowRequest::Unit(token),
            FollowRequest::ByName { name, exact } => {
                crate::player::FollowRequest::Name { name, exact }
            }
        };
        let (guid, name, replaced) = match self.appliers.follow().start(&request) {
            FollowStart::Refused(key) => return self.appliers.chat().display_error(script, key),
            FollowStart::Silent => return,
            FollowStart::Started {
                guid,
                name,
                replaced,
            } => (guid, name, replaced),
        };
        if replaced {
            script.fire_event("AUTOFOLLOW_END", vec![]);
            self.at_point(script, mark);
        }
        self.appliers.follow().arm(guid, name.clone());
        script.fire_event("AUTOFOLLOW_BEGIN", vec![ScriptValue::Str(name)]);
        self.at_point(script, mark);
    }
}

/// The getters' lookups with a callback inside their calls ([`RecordAsk`]): each cache sends the
/// query on a miss, once while it is in flight (`0x556aa0`), and keeps the getter's callback.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RecordAsks<'w, 's> {
    /// Each absent in a harness without it, which asks nothing.
    names: Option<Res<'w, crate::names::NameCache>>,
    items: Option<Res<'w, crate::items::Items>>,
    item_names: Option<Res<'w, crate::item_names::ItemNames>>,
    guild: Option<Res<'w, crate::ui_guild::GuildState>>,
    summon: Option<Res<'w, crate::ui_summon::SummonState>>,
    battlefield: Option<ResMut<'w, crate::ui_battlefield_positions::BattlefieldPositions>>,
    quest: Option<ResMut<'w, crate::ui_quest::QuestGiver>>,
    /// The quest cache the rebuild's lookups ask, and the leaderboard's creature waits.
    quest_log: Option<ResMut<'w, crate::ui_quest_log::QuestLog>>,
    latch_refreshes: Option<ResMut<'w, crate::ui_quest::LatchRefreshes>>,
    merchant: Option<ResMut<'w, crate::ui_merchant::MerchantOpen>>,
    stable: Option<ResMut<'w, crate::ui_stable::StableCreatureWaits>>,
    go_templates: Option<Res<'w, crate::go_templates::GameObjectTemplates>>,
    /// The charter records and the item objects whose enchantment slot 0 names one (`0x5ef337`).
    petitions: Option<Res<'w, crate::ui_petition::PetitionRecords>>,
    /// The mail rows' keys and the name and body callbacks their lookups keep.
    mail: crate::ui_mail::MailLookups<'w>,
    objects: crate::net::Objects<'w, 's>,
    me: Option<Res<'w, crate::net::SelfGuid>>,
    net: Res<'w, crate::net::NetCommands>,
}

impl RecordAsks<'_, '_> {
    /// `ask` inside its getter.
    pub(crate) fn apply(&mut self, ask: RecordAsk) {
        use crate::query_cache::Ask;
        match ask {
            // `UnitName`'s miss keeps its callback (`0x5171f5`, `0x517290`), once (`0x5171f1`).
            RecordAsk::PlayerName(guid) => {
                if let Some(names) = self.names.as_deref() {
                    if names.resolve_player(guid, Ask::Event(&self.net)).is_none() {
                        names.wait_for_player(guid, crate::names::NameCallback::UnitName);
                    }
                }
            }
            RecordAsk::ItemBuildName(guid) => {
                if let Some(names) = self.names.as_deref() {
                    let _ = names.resolve_player(guid, Ask::Event(&self.net));
                }
            }
            // A set member's name, `CMSG_ITEM_NAME_QUERY` with no owner (`0x52df5a`).
            RecordAsk::ItemName(entry) => {
                if let Some(item_names) = self.item_names.as_deref() {
                    let _ = item_names.name(entry, Ask::Event(&self.net));
                }
            }
            // The record by the petition id the object carries, then its owner's name, each
            // asked on its miss (`0x52b93b`, `0x52b9d2`).
            RecordAsk::Petition(item) => {
                if let (Some(names), Some(petitions)) =
                    (self.names.as_deref(), self.petitions.as_deref())
                {
                    let id = self
                        .objects
                        .object(item)
                        .and_then(|f| f.item_enchant(0))
                        .unwrap_or(0) as u32;
                    petitions.tooltip_view(id, item, names, Ask::Event(&self.net));
                }
            }
            // The attachment's object first (`0x4ae5e1`), then its entry's record with guid 0.
            RecordAsk::SendMailItem(guid) => {
                let entry = self.objects.object(guid).and_then(|f| f.object_entry());
                if let (Some(items), Some(entry)) = (self.items.as_deref(), entry) {
                    if items.template(entry, 0, Ask::Event(&self.net)).is_none() {
                        items.wait(entry, crate::items::RecordCallback::SendMail);
                    }
                }
            }
            // The sell item's object first (`0x4ce24e`, `0x4ce5cf`), then its entry's record with
            // the object's guid (`0x4ce263`, `0x4ce5e8`).
            RecordAsk::AuctionSellItem(guid) => {
                let entry = self.objects.object(guid).and_then(|f| f.object_entry());
                if let (Some(items), Some(entry)) = (self.items.as_deref(), entry) {
                    if items.template(entry, guid, Ask::Event(&self.net)).is_none() {
                        items.wait(entry, crate::items::RecordCallback::AuctionSellItem);
                    }
                }
            }
            RecordAsk::BattlefieldName(guid) => {
                if let (Some(names), Some(state)) =
                    (self.names.as_deref(), self.battlefield.as_deref_mut())
                {
                    state.wait_for_name(names, &self.net, guid);
                }
            }
            RecordAsk::TradeItem { entry, owner } => {
                if let Some(items) = self.items.as_deref() {
                    crate::ui_trade::ask_trade_record(items, &self.net, entry, owner);
                }
            }
            RecordAsk::Guild { guild_id, guid } => {
                if let Some(guild) = self.guild.as_deref() {
                    guild.wait_for_info(guild_id, guid, &self.net);
                }
            }
            RecordAsk::AmmoItem { entry, owner } => {
                if let Some(items) = self.items.as_deref() {
                    crate::ui_char::ask_ammo_record(items, &self.net, entry, owner);
                }
            }
            RecordAsk::CooldownItem { entry, owner } => {
                if let Some(items) = self.items.as_deref() {
                    crate::spell::Cooldowns::query_record(items, &self.net, entry, owner, true);
                }
            }
            RecordAsk::Item {
                entry,
                owner,
                callback,
            } => {
                let Some(items) = self.items.as_deref() else {
                    return;
                };
                match callback {
                    ItemCallback::Trainer => {
                        crate::ui_trainer::ask_item_record(items, &self.net, entry, owner);
                    }
                    ItemCallback::Quest => {
                        if let Some(quest) = self.quest.as_deref() {
                            quest.ask_item_record(items, &self.net, entry);
                        }
                    }
                    ItemCallback::Craft => {
                        crate::ui_craft::ask_item_record(items, &self.net, entry, owner);
                    }
                    ItemCallback::TradeSkill => {
                        crate::ui_tradeskill::ask_item_record(items, &self.net, entry, owner);
                    }
                    // The VM counts the landing; the callback, one per entry as the VM keeps its
                    // waits, signals at its place in the entry's list.
                    ItemCallback::QuestLog => {
                        if items
                            .template(entry, owner, Ask::Event(&self.net))
                            .is_none()
                        {
                            items.wait_once(entry, crate::items::RecordCallback::QuestLog);
                        }
                    }
                    // The VM keeps each tooltip's wait; the callback, one per entry, rebuilds them
                    // at its place in the entry's list.
                    ItemCallback::Tooltip => {
                        if items
                            .template(entry, owner, Ask::Event(&self.net))
                            .is_none()
                        {
                            items.wait_once(entry, crate::items::RecordCallback::Tooltip);
                        }
                    }
                    // A miss registers the callback and counts the window's wait (`0x4faf1a`,
                    // `0x4faf23`).
                    ItemCallback::Merchant => {
                        let owner = self.merchant.as_deref().and_then(|o| o.vendor).unwrap_or(0);
                        if items
                            .template(entry, owner, Ask::Event(&self.net))
                            .is_none()
                        {
                            items.wait(entry, crate::items::RecordCallback::Merchant);
                            if let Some(open) = self.merchant.as_deref_mut() {
                                open.pending += 1;
                            }
                        }
                    }
                    ItemCallback::Buyback => {
                        if items
                            .template(entry, owner, Ask::Event(&self.net))
                            .is_none()
                        {
                            items.wait(entry, crate::items::RecordCallback::Buyback);
                        }
                    }
                }
            }
            RecordAsk::QuestCreature(entry) => {
                if let (Some(names), Some(log)) =
                    (self.names.as_deref(), self.quest_log.as_deref_mut())
                {
                    log.ask_creature(names, &self.net, entry);
                }
            }
            RecordAsk::QuestRecord(quest_id) => {
                if let Some(log) = self.quest_log.as_deref_mut() {
                    log.ask_record(&self.net, quest_id);
                }
            }
            // The sweep `0x5eb070`: every giver's status asked again.
            RecordAsk::QuestGiverStatuses => {
                if let Some(latch) = self.latch_refreshes.as_deref_mut() {
                    latch.0 = latch.0.wrapping_add(1);
                }
            }
            RecordAsk::QuestObject(entry) => {
                if let Some(go) = self.go_templates.as_deref() {
                    go.ask_for_quest(entry, &self.net);
                }
            }
            RecordAsk::StableCreature { entry, paperdoll } => {
                if let (Some(names), Some(waits)) =
                    (self.names.as_deref(), self.stable.as_deref_mut())
                {
                    let owner = self.me.as_deref().and_then(|g| g.0).unwrap_or(0);
                    crate::ui_stable::ask_creature(
                        names, &self.net, waits, owner, entry, paperdoll,
                    );
                }
            }
            // A zero guid is no key and asks nothing (`0x55f080`).
            RecordAsk::Summoner => {
                let summoner = self.summon.as_deref().and_then(|s| s.pending());
                if let (Some(names), Some(guid)) = (self.names.as_deref(), summoner) {
                    let _ = names.resolve_player(guid, Ask::Event(&self.net));
                }
            }
            // The house verbs keep its wait ([`crate::ui_auction::AuctionVerbs::wait_for_name`]).
            RecordAsk::AuctionName { guid, list } => {
                debug!("record ask: auction name {guid:#x} for list {list} has no house here");
            }
            // The loot steps keep its wait ([`crate::ui_loot::LootSteps::wait_for_name`]).
            RecordAsk::MasterLootName(guid) => {
                debug!("record ask: master-loot candidate {guid:#x} has no loot window here");
            }
            // The reader's steps keep its wait ([`crate::ui_item_text::ItemTextSteps`]).
            RecordAsk::ItemTextCreator(guid) => {
                debug!("record ask: item text creator {guid:#x} has no reader here");
            }
            RecordAsk::Mail { mail_id, lookup } => self.mail.look_up(&self.net, mail_id, lookup),
        }
    }
}

/// The calling VM's CVar mirror as a console line reads it inside its `ConsoleExec` call.
pub(super) struct VmCvars<'a>(pub(super) &'a UiScript);

impl crate::console::ConsoleCvarSource for VmCvars<'_> {
    fn rows(&self) -> crate::console::ConsoleCvars {
        crate::console::ConsoleCvars::of_vm(self.0.cvar_rows())
    }

    fn value(&self, name: &str) -> Option<String> {
        self.0.cvar(name)
    }
}
