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
    /// for a follow (`0x61113e` → `0x60fb84`), then `AUTOFOLLOW_BEGIN` with the followee's name
    /// (`0x610071`), each running its listeners where it fires.
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
        let name = match self.appliers.follow().start(&request) {
            FollowStart::Refused(key) => return self.appliers.chat().display_error(script, key),
            FollowStart::Silent => return,
            FollowStart::Started { name } => name,
        };
        let guid = self.appliers.follow().followee();
        let told = match self.appliers.follow_feed().as_deref_mut() {
            Some(feed) => feed.tell(script, guid),
            None => None,
        };
        // Deviation: the feed's stops fire `END` for every follow ([`crate::ui_follow`]), so the
        // arm's stop fires it for the follow this VM was last told of.
        if told.is_some() {
            script.fire_event("AUTOFOLLOW_END", vec![]);
            self.at_point(script, mark);
        }
        script.fire_event("AUTOFOLLOW_BEGIN", vec![ScriptValue::Str(name)]);
        self.at_point(script, mark);
    }
}

/// The getters' lookups with a callback inside their calls ([`RecordAsk`]): each cache sends the
/// query on a miss, once while it is in flight (`0x556aa0`), and keeps the getter's callback.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct RecordAsks<'w> {
    /// Each absent in a harness without it, which asks nothing.
    names: Option<Res<'w, crate::names::NameCache>>,
    items: Option<Res<'w, crate::items::Items>>,
    guild: Option<Res<'w, crate::ui_guild::GuildState>>,
    summon: Option<Res<'w, crate::ui_summon::SummonState>>,
    battlefield: Option<ResMut<'w, crate::ui_battlefield_positions::BattlefieldPositions>>,
    ammo_hits: Option<ResMut<'w, crate::ui_char::AmmoRecordHits>>,
    cooldowns: Option<ResMut<'w, crate::spell::Cooldowns>>,
    trainer: Option<ResMut<'w, crate::ui_trainer::TrainerOpen>>,
    quest: Option<ResMut<'w, crate::ui_quest::QuestGiver>>,
    craft: Option<ResMut<'w, crate::ui_craft::CraftItemWaits>>,
    trade_skill: Option<ResMut<'w, crate::ui_tradeskill::TradeSkillItemWaits>>,
    net: Res<'w, crate::net::NetCommands>,
}

impl RecordAsks<'_> {
    /// The player-name cache's record for `guid`, read only ([`benilla_ui::script::CallHost::player_name`]).
    pub(crate) fn player_name(&self, guid: u64) -> Option<String> {
        self.names
            .as_deref()?
            .resolve_player(guid, crate::query_cache::Ask::Read)
            .map(str::to_string)
    }

    /// `ask` inside its getter, whose VM a hit the mirror lacked is told.
    pub(crate) fn apply(&mut self, script: &mut UiScript, ask: RecordAsk) {
        use crate::query_cache::Ask;
        match ask {
            RecordAsk::PlayerName(guid) => {
                if let Some(names) = self.names.as_deref() {
                    let _ = names.resolve_player(guid, Ask::Event(&self.net));
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
                if let (Some(items), Some(hits)) =
                    (self.items.as_deref(), self.ammo_hits.as_deref_mut())
                {
                    crate::ui_char::ask_ammo_record(items, &self.net, hits, entry, owner);
                }
            }
            RecordAsk::CooldownItem { entry, owner } => {
                if let (Some(items), Some(cooldowns)) =
                    (self.items.as_deref(), self.cooldowns.as_deref_mut())
                {
                    cooldowns.ask_item_record(items, &self.net, entry, owner);
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
                        if let Some(trainer) = self.trainer.as_deref_mut() {
                            crate::ui_trainer::ask_item_record(
                                items, &self.net, trainer, entry, owner,
                            );
                        }
                    }
                    ItemCallback::Quest => {
                        if let Some(quest) = self.quest.as_deref_mut() {
                            quest.ask_item_record(items, &self.net, entry);
                        }
                    }
                    ItemCallback::Craft => {
                        if let Some(waits) = self.craft.as_deref_mut() {
                            crate::ui_craft::ask_item_record(items, &self.net, waits, entry, owner);
                        }
                    }
                    ItemCallback::TradeSkill => {
                        if let Some(waits) = self.trade_skill.as_deref_mut() {
                            crate::ui_tradeskill::ask_item_record(
                                items, &self.net, waits, entry, owner,
                            );
                        }
                    }
                    // The VM keeps the callback: the cache only asks.
                    ItemCallback::QuestLog => {
                        let _ = items.template(entry, owner, Ask::Event(&self.net));
                    }
                }
            }
            RecordAsk::QuestCreature(entry) => {
                if let Some(names) = self.names.as_deref() {
                    crate::ui_quest_log::ask_quest_creature(names, &self.net, script, entry);
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
