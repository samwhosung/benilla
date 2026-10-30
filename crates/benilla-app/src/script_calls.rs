//! The script calls that touch the selection, the player's cast or the targeting cursor, applied
//! inside the verb that makes each, through one host ([`ScriptCallHost`]). In the reference each
//! such call is done when it returns (the macro runner `0x4f14e0` fires every line in one pass;
//! `TargetByName` `0x489d60` and `TargetUnit` `0x4899d0` commit through `SetSelection 0x493540`;
//! `CastSpellByName` `0x4b4ab0` reaches `ArmCastTargets 0x6e5250` through direct calls), so
//! `/target Bob` then `/cast Flash Heal` heals Bob, `/cast` then `/target` casts at the old target,
//! and `SpellIsTargeting()` on the line after a cast reads the word it armed.
//!
//! Every system that runs Lua lends the host to the VM around it ([`HostedVm`]), whatever entry the
//! script comes in by: the mouse and key feed, the bindings pass, an `OnUpdate`, the handlers of
//! the events a feed fires, a chunk. A system that holds the host's own state alongside the VM runs
//! as an engine turn ([`VmWith`]): a call its Lua makes waits, as one made while the host applies a
//! call does, and applies before the system ends. Every host runs in one order, the VM pass
//! ([`crate::vm_pass`]). An
//! addon's or a macro's cast passes the gate's tag 2 (`0x494a92`) only inside an input event; the
//! client's own code passes it outside any (`0x494a5a` → `0x494b11`).

use bevy::ecs::system::{SystemParam, SystemParamItem};
use bevy::prelude::*;

use benilla_ui::script::{
    ActionBarCall, AuctionCall, AuraCall, BankCall, BinderCall, CallHost, CallTail, CraftCall,
    DeathAction, DuelRequest, GmTicketCall, GossipCall, InstanceCall, ItemTextCall, LootRollCall,
    MailCall, MeetingStoneCall, MerchantCall, MinimapCall, PetitionCall, PostureCall, PvpCall,
    ScriptCall, StableCall, SummonCall, TabardCall, TaxiCall, TradeCall, TradeSkillCall,
    TrainerCall, UiScript,
};

pub(crate) mod disjoint;
mod engine;
pub(crate) mod live;
mod windows;
mod world_click;
#[cfg(test)]
pub(crate) use world_click::{
    click_plate, error_lines, record_error_lines, release_click, take_error_lines,
};
pub(crate) use world_click::{submit_waiting_clicks, InputHost};

/// `DoEmote` (`0x5ef560`) in the reference's order: a posture emote's stand change to a state but 0
/// runs its local apply's leave (`0x5f0790`), the loot close (`0x48f200` at `0x5f07d5`) then the
/// attack stop (`0x5ecac0` at `0x5f07e7`), ahead of the emote's `CMSG_TEXT_EMOTE`.
macro_rules! emote_in_order {
    ($appliers:expr, $step:expr) => {{
        if let Some(step) = $step {
            if step.leave {
                $appliers.loot().close_for_stand();
                $appliers.attack().stop_for_stand();
            }
            $appliers.emote().finish(step);
        }
    }};
}

disjoint::disjoint_set! {
    /// The appliers, one per owner of the state a call touches, each lent alone: they share state
    /// with one another (the net channel, the cast ladder), never with what holds the host.
    pub(crate) struct Appliers, AppliersState, watched {
        /// The selection.
        select: crate::target::ScriptSelect<'static, 'static>,
        /// The targeting cursor.
        cursor: crate::spell::ScriptCursor<'static, 'static>,
        /// The cast's own state: its cancel, the view the VM reads after each call, and the
        /// reports the call made.
        self_cancel: crate::spell::SelfCancel<'static>,
        cast_state: crate::spell::CastState<'static, 'static>,
        /// What the VM last took of the live cast state, and the views lane's log; absent in a
        /// harness without the cast lanes.
        cast_views: Option<ResMut<'static, crate::ui_cast::CastViews>>,
        reports: crate::ui_cast::CallReports<'static, 'static>,
        /// The character panel's tabs: talents, skills and reputation.
        talents: crate::ui_talent::TalentSteps<'static, 'static>,
        skills: crate::ui_char::SkillSteps<'static>,
        reputation: crate::ui_reputation::ReputationSteps<'static, 'static>,
        /// The action slots' states, whose usable refresh a selection change runs inside its
        /// call; absent in a harness without the action feed's inputs.
        action_states: Option<crate::ui_action::ActionStates<'static, 'static>>,
        /// The session's: the logout and the reload, the tutorial banks, and the interaction
        /// latch the NPC windows' close cores release.
        session: crate::ui_logout::SessionSteps<'static>,
        tutorial: crate::tutorial::TutorialSteps<'static>,
        latch: crate::ui_session::LatchSteps<'static>,
        /// The bank and the profession windows.
        bank: crate::ui_bank::BankSteps<'static, 'static>,
        trade_skill: crate::ui_tradeskill::TradeSkillSteps<'static, 'static>,
        craft: crate::ui_craft::CraftSteps<'static>,
        /// The dialogs and queues: the battlegrounds, death, the duel, the group loot roll, the
        /// innkeeper, the summon, the meeting stone, the lockouts and the PvP flag.
        battlefield: crate::ui_battlefield::BattlefieldSteps<'static>,
        death: crate::death::DeathSteps<'static, 'static>,
        duel: crate::ui_duel::DuelSteps<'static>,
        loot_roll: crate::ui_loot_roll::LootRollSteps<'static>,
        binder: crate::ui_binder::BinderSteps<'static, 'static>,
        summon: crate::ui_summon::SummonSteps<'static>,
        meeting_stone: crate::ui_dialog_verbs::MeetingStoneSteps<'static>,
        instance: crate::ui_instance::InstanceSteps<'static>,
        pvp: crate::ui_unit::PvpSteps<'static>,
        /// The cast ladder and the targeting it reads.
        cast: crate::spell::ScriptCast<'static, 'static>,
        /// The action bar's presses, the chat's sends and emotes, and the social windows'.
        action: crate::ui_action::ActionPress<'static, 'static>,
        chat: crate::ui_chat::ChatSender<'static, 'static>,
        emote: crate::ui_chat::EmoteDoer<'static, 'static>,
        auction: crate::ui_auction::AuctionVerbs<'static, 'static>,
        social: crate::ui_social::SocialSteps<'static>,
        party: crate::ui_party::PartyCalls<'static, 'static>,
        trade: crate::ui_trade::TradeSteps<'static, 'static>,
        /// The guild's windows: the roster, the charter and the tabard designer.
        guild: crate::ui_guild::GuildSteps<'static>,
        petition: crate::ui_petition::PetitionSteps<'static, 'static>,
        tabard: crate::ui_tabard::TabardSteps<'static, 'static>,
        /// The bags', the cursor's and the paper doll's: the item uses and the item verbs' sends.
        item_use: crate::ui_items::ScriptItemUse<'static, 'static>,
        items: crate::ui_items::ItemSteps<'static, 'static>,
        /// The loot window's, the mailbox's, the questgiver's, the gossip menu's and the other
        /// NPC windows'.
        loot: crate::ui_loot::LootSteps<'static, 'static>,
        mail: crate::ui_mail::MailVerbs<'static>,
        quest: crate::ui_quest::QuestVerbs<'static, 'static>,
        gossip: crate::ui_gossip::GossipVerbs<'static, 'static>,
        trainer: crate::ui_trainer::TrainerSteps<'static, 'static>,
        taxi: crate::ui_taxi::TaxiSteps<'static, 'static>,
        merchant: crate::ui_merchant::MerchantSteps<'static, 'static>,
        /// The pet's: the bar's presses, its other verbs and the menu's, and the stable.
        pet_press: crate::ui_pet::PetPress<'static, 'static>,
        pet: crate::ui_pet::PetSteps<'static, 'static>,
        stable: crate::ui_stable::StableSteps<'static, 'static>,
        /// The player's attack, and its stand-state setter.
        attack: crate::ui_action::AttackPress<'static, 'static>,
        stand: crate::player::StandSetter<'static, 'static>,
        /// The sends of the action bar, the worn display, the Help window, the unserved systems,
        /// the inspect window, the reader, the aura cancel, the minimap and the pet trainer.
        action_bar: crate::ui_action::ActionBarSteps<'static>,
        worn: crate::ui_unit::WornDisplaySteps<'static, 'static>,
        gm_ticket: crate::ui_gm_ticket::GmTicketSteps<'static>,
        inspect: crate::ui_inspect::InspectSteps<'static, 'static>,
        item_text: crate::ui_item_text::ItemTextSteps<'static, 'static>,
        aura: crate::ui_aura::AuraSteps<'static, 'static>,
        minimap: crate::minimap::MinimapSteps<'static>,
        pet_unlearn: crate::ui_dialog_verbs::PetUnlearnSteps<'static, 'static>,
        /// The engine's plate bits, and the follow's start.
        plates: crate::vplates::PlateSteps<'static>,
        follow: crate::target::FollowSteps<'static, 'static>,
        /// The console's commands, against what they read and write, and the getters' lookups.
        console: crate::console::ConsoleSteps<'static, 'static>,
        /// The CVar registry and every row's change callback, run inside the write.
        cvars: crate::cvars::CvarSteps<'static>,
        /// The camera's five views, which the view verbs seat and save.
        views: crate::player::camera_view::ViewSteps<'static, 'static>,
        /// The display restart's commit of the latched rows.
        gx_restart: crate::video::GxRestartSteps<'static>,
        /// The dressing room the `DressUpModel` verbs edit.
        dressup: crate::ui_dressup::DressUpSteps<'static>,
        asks: engine::RecordAsks<'static, 'static>,
        /// What the getters read as they run ([`live::LiveReads`]).
        live: live::LiveReads<'static, 'static>,
        /// The world click's arm, its pick and its legs.
        click: crate::target::ClickLegs<'static, 'static>,
        /// The binding functions' input state, and `DropItemOnUnit`'s legs.
        commands: crate::player::CommandSteps<'static, 'static>,
        look: crate::player::LookSteps<'static>,
        sheath: crate::player::SheathToggle<'static, 'static>,
        drop_item: crate::ui_action::drop_item::DropItemLeg<'static, 'static>,
    }
}

/// The engine half of every script call: one applier per owner of the state a call touches.
#[derive(SystemParam)]
pub(crate) struct ScriptCallHost<'w, 's> {
    appliers: Appliers<'w, 's>,
    /// The channel the census counts sends on ([`benilla_ui::script::census`]).
    #[cfg(test)]
    census: Option<Res<'w, crate::net::NetCommands>>,
}

/// What a call's apply carries across its signal points about the selection: the guid its last
/// signal told the listeners of, the pet's attack latch as it stood then (the selection's release
/// can lower it, `0x493a18`), and the reports signalled so far.
struct Signalled {
    selected: Option<u64>,
    latched: bool,
    mark: crate::ui_cast::ReportMark,
}

/// What an engine writer's `SetSelection` outside any call (a world click, a death's or a
/// despawn's deselect) leaves for its signals ([`ScriptCallHost::engine_select`]): the selection
/// and the pet's attack latch it found, the clock as it began, so the cast reports its teardown
/// made are its own, and what its commit owes after its signals.
pub(crate) struct EngineSelect {
    pub(crate) before: Option<u64>,
    pub(crate) latched: bool,
    pub(crate) mark: crate::lanes::Mark,
    pub(crate) owed: crate::target::SelectDebt,
}

impl ScriptCallHost<'_, '_> {
    /// Count the census's sends on this host's channel ([`benilla_ui::script::census`]).
    #[cfg(test)]
    fn census_probe(&self) {
        if let Some(net) = self.census.as_deref() {
            let sent = net.0.clone();
            benilla_ui::script::census::probe(move || sent.len());
        }
    }

    /// End the lend the census is watching ([`disjoint::end_lend`]): the appliers are all unlent.
    #[cfg(test)]
    fn census_done(&mut self) {
        self.appliers.end_lend();
    }

    /// An engine writer's selection outside any call, signalled in place as an engine turn, as
    /// `SetSelection` (`0x493540`) signals inside whatever runs it: the teardown's
    /// `PET_BAR_UPDATE` (`0x4bd6b4`) before the store and the send, the change's signals before
    /// the re-swing (`0x4938c8`), each point running its listeners there, and what they left
    /// applied once the turn ends, with the host lent. A unit the writer's select handed the
    /// targeting cursor (`0x4935d5`), such as the melee probe's nearest-enemy pick (`0x6130bc` →
    /// `0x4941e6`), is bound first, where its select returned.
    pub(crate) fn engine_select(&mut self, script: &mut UiScript, select: EngineSelect) {
        #[cfg(test)]
        self.census_probe();
        script.host_out(|script| {
            self.appliers.cursor().bind_parked();
            let mut at = Signalled {
                selected: select.before,
                latched: select.latched,
                mark: self.appliers.reports().mark_at(select.mark),
            };
            self.settle_selection(script, &mut at, select.owed);
            self.commit_views(script);
            #[cfg(test)]
            self.census_done();
        });
        script.hosted(self, |_| ());
    }

    /// A go's deferred melee start outside any call (`0x6e8402` → `0x6131a0`), the player's Attack
    /// with the first hit as the validator's candidate: the actor checks and the target half
    /// (`0x612df0`), whose select of the hit signals in place, or with the targeting cursor up
    /// binds it and selects nothing; then the swing at the selection as the validator re-read it
    /// (`0x6130c1`), or a refusal's line and no swing. Behind the cast reports made since `mark`.
    pub(crate) fn engine_attack(
        &mut self,
        script: &mut UiScript,
        guid: u64,
        mark: crate::lanes::Mark,
    ) {
        #[cfg(test)]
        self.census_probe();
        script.host_out(|script| {
            let mut at = Signalled {
                selected: self.appliers.select().selected(),
                latched: self.appliers.pet().attack_latched(),
                mark: self.appliers.reports().mark_at(mark),
            };
            // A packet runs in no script: the gate's tag 4 passes (`0x494a5a`).
            self.attack_at(script, &mut at, Some(guid), true);
            self.commit_views(script);
            #[cfg(test)]
            self.census_done();
        });
        script.hosted(self, |_| ());
    }

    /// `SelectTarget(0)` (`0x493540`) from an engine writer outside any call, such as the
    /// world-enter cascade's (`0x490927`): the deselect and its teardown signal in place, a
    /// charmed player's refused (`0x493590`) and an empty selection a no-op.
    pub(crate) fn engine_clear_target(&mut self, script: &mut UiScript, mark: crate::lanes::Mark) {
        #[cfg(test)]
        self.census_probe();
        script.host_out(|script| {
            let mut at = Signalled {
                selected: self.appliers.select().selected(),
                latched: self.appliers.pet().attack_latched(),
                mark: self.appliers.reports().mark_at(mark),
            };
            let owed = self.appliers.select().clear_target();
            self.settle_selection(script, &mut at, owed);
            self.commit_views(script);
            #[cfg(test)]
            self.census_done();
        });
        script.hosted(self, |_| ());
    }

    /// A field watcher's usable refresh (`0x4b31c0`) outside any call, inside its packet: the bar's
    /// states and cache derived off the stores as of the drain's place and pushed, then its signals
    /// with the host lent, so their listeners' calls apply inside the verb. A harness without the
    /// action feed's inputs refreshes nothing.
    pub(crate) fn engine_usable_refresh(&mut self, script: &mut UiScript) {
        let signals = self
            .appliers
            .action_states()
            .as_mut()
            .map(|states| states.usable_refresh(script));
        if let Some(signals) = signals {
            script.hosted(self, |script| signals.fire(script));
        }
    }

    /// The world-enter cascade's usable cache refresh (`0x490979` → `0x4e4f90` → `0x4e5c00`),
    /// in its place ([`crate::ui_action::ActionStates::cache_refresh`]): whether a pair
    /// changed. A harness without the action feed's inputs refreshes nothing.
    pub(crate) fn engine_enter_world_refresh(&mut self, script: &mut UiScript) -> bool {
        self.appliers
            .action_states()
            .as_mut()
            .is_some_and(|states| states.cache_refresh(script))
    }

    /// The bar's usable cache refreshed outside the usable refresh (`RefreshActionBarUsable
    /// 0x4e5c00`), `RaiseBagUpdate`'s in an item watcher's run (`0x4f9374`): every pair recomputed
    /// and stored, and `ACTIONBAR_UPDATE_USABLE` signalled with the host lent when one changed
    /// (`0x4e5c53`). A harness without the action feed's inputs refreshes nothing.
    pub(crate) fn engine_usable_cache_refresh(&mut self, script: &mut UiScript) {
        let changed = self
            .appliers
            .action_states()
            .as_mut()
            .is_some_and(|states| states.cache_refresh(script));
        if changed {
            script.hosted(self, |script| {
                script.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
            });
        }
    }

    /// `EditMacro`'s refresh of the bar's usable cache (`0x4f132e` into `0x4e5c00`), inside the
    /// call: `ACTIONBAR_UPDATE_USABLE` at its signal point when a pair changed (`0x4e5c53`).
    fn refresh_usable_cache(&mut self, script: &mut UiScript, at: &mut Signalled) {
        let changed = self
            .appliers
            .action_states()
            .as_mut()
            .is_some_and(|states| states.cache_refresh(script));
        if changed {
            script.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
            self.at_point(script, at);
        }
    }

    /// A slot write outside any call (`0x4e58e0`), a recount's or a spell packet's: the 1-based
    /// `action`'s usable pair stored and pushed, with nothing signalled
    /// ([`crate::ui_action::ActionStates::slot_written`]).
    pub(crate) fn engine_slot_written(&mut self, script: &mut UiScript, action: u32) {
        if let Some(states) = self.appliers.action_states().as_mut() {
            states.slot_written(script, action);
        }
    }

    /// Our player's arrival's slot pass (`0x5deae2`-`0x5deb12`): the staged words loaded into the
    /// bar but an unknown spell's ([`crate::ui_action::ActionSlots::load_staged`]), then each
    /// loaded word into the VM's copy, each item word recounted and its usable pair stored
    /// ([`crate::ui_action::ActionStates::arrival_slots`]).
    pub(crate) fn engine_arrival_slots(&mut self, script: &mut UiScript) {
        let Some(known) = self
            .appliers
            .action_states()
            .as_ref()
            .map(|states| states.known_spells())
        else {
            return;
        };
        let loaded = self
            .appliers
            .action_bar()
            .load_staged(|word| known.contains(&word));
        if let Some(states) = self.appliers.action_states().as_mut() {
            states.arrival_slots(script, &loaded);
        }
    }

    /// Placements an engine writer makes outside any call on this login's bar, once our player's
    /// arrival has loaded it ([`crate::ui_action::ActionSlots::loaded`]): `writes` picks the
    /// 1-based `(id, word)` pairs off the bar as it stands, and each goes out as a placement's does
    /// ([`crate::ui_action::ActionBarSteps::set`]), `CMSG_SET_ACTION_BUTTON` sent and the slot
    /// stored, into the VM's copy with its usable pair ([`Self::engine_slot_written`]), with
    /// nothing signalled. The ids written, in order; `None` while the bar is not loaded.
    pub(crate) fn engine_bar_writes(
        &mut self,
        script: &mut UiScript,
        writes: impl FnOnce(&[benilla_protocol::messages::ActionButton]) -> Vec<(u32, u32)>,
    ) -> Option<Vec<u32>> {
        let bar = self.appliers.action_bar();
        if !bar.loaded() {
            return None;
        }
        let buttons: Vec<_> = bar.buttons().copied().collect();
        let mut writes = writes(&buttons);
        writes.sort_unstable();
        for &(id, word) in &writes {
            let mut bar = self.appliers.action_bar();
            bar.set(id, word);
            let slot = bar.button(id).map(|b| benilla_ui::script::ActionSlot {
                kind: b.kind,
                action: b.action,
                ..Default::default()
            });
            script.set_action(id, slot);
            self.engine_slot_written(script, id);
        }
        Some(writes.into_iter().map(|(id, _)| id).collect())
    }

    /// `0x4e6d20(entry)`, a recount's one count of its entry (`0x4e5c75`); `None` in a harness
    /// without the action feed's inputs.
    pub(crate) fn engine_item_count(&mut self, entry: u32) -> Option<u32> {
        self.appliers
            .action_states()
            .as_ref()
            .map(|states| states.item_count(entry))
    }

    /// `0x4e5de0`'s notify of the 1-based `action` when it stands for `spell` in the player's book:
    /// its usable pair stored; whether it did
    /// ([`crate::ui_action::ActionStates::spell_slot_notified`]).
    pub(crate) fn engine_spell_slot_notified(
        &mut self,
        script: &mut UiScript,
        action: u32,
        spell: u32,
    ) -> bool {
        self.appliers
            .action_states()
            .as_mut()
            .is_some_and(|states| states.spell_slot_notified(script, action, spell))
    }

    /// A recount's write of the 1-based `action` when it holds `entry`: `count` stored
    /// (`0x4e5caa`), the VM's copy with it, and the slot's usable pair (`0x4e5cb1` into
    /// `0x4e58e0`); whether it held the entry
    /// ([`crate::ui_action::ActionStates::recounted`]).
    pub(crate) fn engine_recounted(
        &mut self,
        script: &mut UiScript,
        action: u32,
        entry: u32,
        count: u32,
    ) -> bool {
        self.appliers
            .action_states()
            .as_mut()
            .is_some_and(|states| states.recounted(script, action, entry, count))
    }

    /// Push the views a call left for the script's next line, and for the listeners at a signal
    /// point inside it.
    fn commit_views(&mut self, script: &mut UiScript) {
        let appliers = &mut self.appliers;
        // The cast state: `SpellStopCasting` (`0x6e6e80`) finds the cast the call sent in flight
        // (`0x6e5026`), and `IsCurrentAction` (`0x4e53a0`) checks its slot, as stock
        // `ActionButtonUp` reads straight after `UseAction` (`ActionButton.lua:54-55`); with the
        // targeting word `SpellIsTargeting` (`0x6e6cd0`) reads. A call that moved none of it
        // leaves the view its listener's packet left ([`crate::ui_cast::CastViews::commit`]).
        let view = appliers.cast_state().view();
        match appliers.cast_views().as_deref_mut() {
            Some(views) => views.commit(script, view),
            None => {
                script.set_cast_view(view);
                script.set_spell_targeting(view.targeting.is_some());
            }
        }
        // The rest of the targeting word's: `SpellTargetUnit` binds to it.
        appliers.cursor().push_view(script);
    }

    /// A signal point inside a call (`SignalEvent`, `0x703e50`, runs every listener before the
    /// verb goes on): the views the call left are pushed, the reports it made since `at`'s mark
    /// are signalled behind its signals, as its tail would, and all of it runs here, in place
    /// ([`UiScript::signal_point`]). A selection a listener made there signalled itself, nested,
    /// so the call's record moves to the selection and the pet latch they left
    /// ([`Self::listened`]), and the call's own signals tell no change of theirs again. The
    /// selecting verbs signal their own selection where they commit it, so the record holds no
    /// change of the call's that a point could swallow.
    fn at_point(&mut self, script: &mut UiScript, at: &mut Signalled) {
        self.point(script, &mut at.mark);
        self.listened(at);
    }

    /// [`Self::at_point`] that leaves the call's record where it was: for `SetSelection` past its
    /// dedup, whose store and signals follow its listeners' picks whatever they were (`0x4936e2`,
    /// `0x49389c`), so a pick of theirs is no change of the call's own.
    fn point(&mut self, script: &mut UiScript, mark: &mut crate::ui_cast::ReportMark) {
        self.signal_reports(script, mark);
        self.commit_views(script);
        #[cfg(test)]
        self.census_done();
        script.signal_point(self);
    }

    /// `AcceptAreaSpiritHeal()` (`0x492310`): the scan first (`0x492316` → `0x4923b0`), each healer
    /// change it makes applied as `0x4921c0` does, its out-of-range signal's listeners running
    /// where it fires, then the send with the healer it left.
    fn accept_area_spirit_heal(&mut self, script: &mut UiScript, at: &mut Signalled) {
        let changes = self.appliers.death().rescan_spirit();
        for change in &changes {
            // `0x4921fc` → `0x6e7040(0xa18)`, the whole cancel, then the new healer's query.
            if change.cancel_aura {
                self.cancel_aura(script, at, crate::ui_dialog_verbs::AREA_SPIRIT_HEALER_AURA);
            }
            crate::ui_dialog_verbs::send_healer_query(change, self.appliers.death().net());
        }
        self.appliers
            .death()
            .apply(script, DeathAction::AcceptAreaSpiritHeal);
    }

    /// The aura cancel `0x6e7040(spell)`: the active player's precast of the spell and its kits
    /// ended (`0x6e7075`, `0x6e707e`), the far-sight gate (`0x6e709e`-`0x6e70b4`), then for spell
    /// 2584 `AREA_SPIRIT_HEALER_OUT_OF_RANGE` (`0x6e70c3`), whose listeners run where it fires,
    /// then `CMSG_CANCEL_AURA` (`0x6e70eb`).
    fn cancel_aura(&mut self, script: &mut UiScript, at: &mut Signalled, spell_id: u32) {
        self.appliers.aura().end_on_player(spell_id);
        if !self.appliers.aura().passes_far_sight(spell_id) {
            return;
        }
        if spell_id == crate::ui_dialog_verbs::AREA_SPIRIT_HEALER_AURA {
            script.fire_event("AREA_SPIRIT_HEALER_OUT_OF_RANGE", vec![]);
            self.at_point(script, at);
        }
        self.appliers.aura().send(spell_id);
    }

    /// `SitOrStand()` (`0x48b920`): `SetStandState` (`0x5ed430`) on the active player, whose
    /// change to a state but 0 runs its local apply's leave, the loot close (`0x48f200` at
    /// `0x5f07d5`) then the attack stop (`0x5ecac0` at `0x5f07e7`), before its tail.
    fn sit_or_stand(&mut self, script: &mut UiScript, at: &mut Signalled) {
        let Some(state) = self.appliers.stand().sit_or_stand() else {
            return;
        };
        let Some(leave) = self.appliers.stand().set(state) else {
            return;
        };
        if leave {
            self.appliers.loot().close_for_stand();
            self.stop_attack(script, at);
        }
        self.appliers.stand().finish();
    }

    /// StopAttack (`0x5ecac0`) inside a call, its signals' listeners running where they fire: its
    /// tutorials' before its `CMSG_ATTACKSTOP` (`0x5ecb17`, `0x5ecb54`, `0x5ecb5b`), its
    /// next-swing cancel's (`0x6e6f68`) before the save's `CMSG_CANCEL_CAST` (`0x6e6ff3`).
    fn stop_attack(&mut self, script: &mut UiScript, at: &mut Signalled) {
        let Some(stop) = self.appliers.attack().stop_head() else {
            return;
        };
        self.at_point(script, at);
        if self.appliers.attack().stop_send(stop) {
            self.at_point(script, at);
        }
        self.appliers.attack().stop_save();
    }

    /// `SetCommands` (`0x515090`) inside a binding call: the bit, then the cast cancel (`0x51512a`
    /// → `0x6e4940`, `0x515193` → `0x6e9b70`), whose stop the call's reports signal, then the
    /// tick's legs (`0x5151a0` → `0x514640`, [`crate::player::TickLegs`]): the away clear's line,
    /// a start's `AUTOFOLLOW_END` and its loot close, each signalled in its place, the loot's
    /// `LOOT_CLOSED` in the call's tail.
    fn set_commands(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        input: benilla_ui::script::BindingInput,
    ) -> Option<CallTail> {
        use benilla_ui::script::BindingInput::{Fire, ViewStart, ViewStop};
        use benilla_ui::script::FiredInput;
        match input {
            // The camera's axes are its own state (`[cam+0x128]`), set inside the call.
            ViewStart(axis) | ViewStop(axis) => {
                self.appliers
                    .views()
                    .move_axis(axis, matches!(input, ViewStart(_)));
                return None;
            }
            // The zoom runs the camera's distance axis (`0x50fc60`, `0x50fcd0`).
            Fire(FiredInput::CameraZoomIn(yards)) => {
                self.appliers.views().zoom(false, yards);
                return None;
            }
            Fire(FiredInput::CameraZoomOut(yards)) => {
                self.appliers.views().zoom(true, yards);
                return None;
            }
            // `ToggleSheath` (`0x48a070`) is no command bit: the player's own toggle.
            Fire(FiredInput::ToggleSheath) => {
                self.appliers.sheath().toggle();
                return None;
            }
            _ => {}
        }
        // A bound mouse command's press that sets the first primary bit enters freelook before
        // its tick (`0x514840` → `0x51491f`, then `0x5151a0`).
        let before = self.appliers.look().bits_before(input);
        let called = self.appliers.commands().call(input);
        let looked = self.appliers.look().bound(input, before);
        self.freelook_edges_here(script, at, looked);
        self.run_called(script, at, called)
    }

    /// The world frame's half of a freelook a call entered or left ([`freelook_edges`]), whose
    /// handlers run there, ahead of the call's cast cancel and tick (`0x51491f` inside `0x514840`,
    /// then `0x51512a`, `0x5151a0`) or its look-session work (`0x493070`, then `0x514231`).
    fn freelook_edges_here(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        looked: crate::player::Looked,
    ) {
        freelook_edges(script, looked);
        if looked.entered || looked.left {
            self.at_point(script, at);
        }
    }

    /// A look-session call inside its verb ([`benilla_ui::script::ScriptCall::Look`]): the world
    /// mouse's channel and the freelook it enters or leaves, then for `MouselookStart`/`Stop` the
    /// `SetCommands` work its bit change runs (`0x514231`, `0x51425b`), as [`Self::set_commands`]
    /// runs a bound one's.
    fn look(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        call: benilla_ui::script::LookCall,
    ) -> Option<CallTail> {
        let looked = self.appliers.look().call(call);
        self.freelook_edges_here(script, at, looked);
        let benilla_ui::script::LookCall::Mouselook(on) = call else {
            return None;
        };
        let called = self.appliers.commands().mouselook(on, looked.changed);
        self.run_called(script, at, called)
    }

    /// What a `SetCommands` call leaves its host ([`crate::player::Called`]), run in its order.
    fn run_called(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        mut called: crate::player::Called,
    ) -> Option<CallTail> {
        // The cast cancel's stop (`0x51512a` → `0x6e4940`) runs its listeners before the channel's
        // `CMSG_CANCEL_CHANNELLING` (`0x515193` → `0x6e9b70`) and the tick (`0x5151a0`), whose
        // stand-up and pitch latch read the call as they left it (`0x514e0c`, `0x514ef4`).
        if let Some(mover_is_player) = called.cancel {
            self.appliers.self_cancel().cancel_for_move();
            self.at_point(script, at);
            self.appliers
                .self_cancel()
                .cancel_channel_for_move(mover_is_player);
        }
        let legs = self.appliers.commands().tick_called(&mut called);
        if legs == crate::player::TickLegs::default() {
            return None;
        }
        // The away clear's line (`0x5eb878`) runs its listeners before the mirror's zero and the
        // AFK packet (`0x5eb885`, `0x5eb8d0`).
        if legs.clear_afk && self.appliers.chat().clear_afk_line(script) {
            self.at_point(script, at);
            self.appliers.chat().finish_clear_afk();
        }
        // `StopAutoMove` signals a follow's end before it stops the move (`0x60fb84`, then
        // `0x60e7f0`), and the loot close comes after.
        if legs.stop_auto_move {
            if self.appliers.commands().following() {
                script.fire_event("AUTOFOLLOW_END", vec![]);
                self.at_point(script, at);
            }
            self.appliers.commands().stop_auto_move();
        }
        if legs.close_loot {
            return self.appliers.loot().close_for_move(script);
        }
        None
    }

    /// The reports the call made since `mark` signalled now, ahead of whatever it signals next:
    /// a selection's teardown runs StopAttack (`0x493a08`), whose next-swing cancel
    /// (`0x5ecb63` → `0x6e6f30`) reports there, ahead of the teardown's `PET_BAR_UPDATE`
    /// (`0x493a18`) and the change's `PLAYER_TARGET_CHANGED`; TryCast's cancel reports ahead of
    /// its pick (`0x6e4d71`, `0x6e4efb`).
    fn signal_reports(&mut self, script: &mut UiScript, mark: &mut crate::ui_cast::ReportMark) {
        // The tutorial triggers its steps queued fire ahead of its reports, as StopAttack's Food
        // and Drink (`0x5ecb17`) and `NEED_AMMO`'s Ammunition (`0x6e1cc3`) precede theirs.
        self.appliers.tutorial().fire_since(script, mark.lanes());
        let ready = self.appliers.reports().ready_since(script, *mark);
        crate::ui_cast::fire(script, ready);
        *mark = self.appliers.reports().mark();
    }

    /// Whether the object manager holds `guid` as a unit, as the selection's lookups ask
    /// (`0x468460`, then the type's unit bit).
    fn holds_unit(&mut self, guid: u64) -> bool {
        let mut held = false;
        self.appliers
            .live()
            .units()
            .read_tokens(&mut |objects| held = objects.target_of(guid).is_some());
        held
    }

    /// What `SetSelection` (`0x493540`) signals for a selection this call moved since `at` last
    /// signalled, in its order: the release's `PET_BAR_UPDATE` when it called a possessed unit off
    /// (`0x493a18` → `0x4bd6b4`), the usable refresh (`0x493892`, `0x493a69`), then
    /// `PLAYER_TARGET_CHANGED` (`0x49389c`, `0x493a73`), with `"target"` seated for the
    /// listeners and the script's next line. Whether it signalled.
    fn signal_selection(&mut self, script: &mut UiScript, at: &mut Signalled) -> bool {
        let selected = self.appliers.select().selected();
        if selected == at.selected {
            return false;
        }
        // The release's token flag off the old selection, ahead of its stop's report and its
        // `PET_BAR_UPDATE` (`0x4939cc`, `0x493a08`, `0x493a18`), then after them the store's on the
        // new (`0x49372a`): each on the object its lookup finds as a unit (`0x493999`-`0x4939b8`,
        // `0x4936ee`-`0x493712`), linking or unlinking its unit-field watchers where the flags
        // leave or reach 0.
        use benilla_ui::script::{TokenChange, TOKEN_TARGET};
        if let Some(old) = at.selected.filter(|&g| self.holds_unit(g)) {
            script.unit_token_flags(old, TokenChange::Remove(TOKEN_TARGET));
        }
        self.signal_reports(script, &mut at.mark);
        let appliers = &mut self.appliers;
        script.seat_selection(appliers.select().seated());
        if at.latched && !appliers.pet().attack_latched() {
            script.fire_event("PET_BAR_UPDATE", vec![]);
        }
        if let Some(guid) = selected.filter(|&g| self.holds_unit(g)) {
            script.unit_token_flags(guid, TokenChange::Add(TOKEN_TARGET));
        }
        let appliers = &mut self.appliers;
        if let Some(states) = appliers.action_states().as_mut() {
            states.usable_refresh(script).fire(script);
        }
        script.fire_event("PLAYER_TARGET_CHANGED", vec![]);
        at.selected = selected;
        at.latched = appliers.pet().attack_latched();
        true
    }

    /// A selection's `SetSelection` from its signals on, `owed` being what its commit owes after
    /// them: the teardown's stop report (`0x493a08`) runs its listeners before the pet leg
    /// (`0x493a0f`), whose `PET_BAR_UPDATE` (`0x4bd6b4`) runs its own before the store and the
    /// send (`0x4936e2`, `0x493871`), the change's signals run theirs before the re-swing
    /// (`0x4938c8`), and a payment that selects again (StartAttack's own select, `0x5eccaa`)
    /// signals in turn before what it owes. A selection a listener makes at a point signals
    /// itself, nested, so each point moves the mark to the selection as the listeners left it.
    fn settle_selection(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        mut owed: crate::target::SelectDebt,
    ) {
        loop {
            if let Some(mut send) = owed.take_send() {
                // The teardown's StopAttack runs with the selection zeroed (`0x4939d3`): its
                // tutorials' listeners before its `CMSG_ATTACKSTOP` (`0x5ecb17`, `0x5ecb54`,
                // `0x5ecb5b`), its next-swing cancel's (`0x6e6f68`) before the save's
                // `CMSG_CANCEL_CAST` (`0x6e6ff3`) and the pet leg (`0x493a0f`).
                self.signal_reports(script, &mut at.mark);
                script.seat_selection(self.appliers.select().seated());
                if let Some(stop) = send.take_stop() {
                    self.point(script, &mut at.mark);
                    if self.appliers.select().stop_send(stop) {
                        self.point(script, &mut at.mark);
                    }
                    // A listener's `PetStopAttack` lowered the latch and signalled there, so the
                    // pet leg and the change find it down and signal nothing (`0x4bd650`).
                    at.latched = self.appliers.pet().attack_latched();
                    self.appliers.select().stop_save();
                }
                // `PetStopAttack` lowered the latch and signals (`0x4bd6ae`, `0x4bd6b4`).
                if self.appliers.select().run_pet_leg(&mut send) {
                    script.fire_event("PET_BAR_UPDATE", vec![]);
                    at.latched = false;
                    self.point(script, &mut at.mark);
                }
                // The store over a listener's pick (`0x4936e2`), or for a clear the send of the
                // listener's pick, and either signals after (`0x49389c`, `0x493a73`), so the
                // mark stays the selection the call found.
                self.appliers.select().pay_send(send);
            }
            let signalled = self.signal_selection(script, at);
            if !signalled && !owed.owes_swings() {
                return;
            }
            // `SetSelection` returns, or goes on to its re-swing, once its listeners ran. The
            // re-swing pushes the selection's address (`0x4938c3`), so it goes at a listener's
            // pick; StartAttack reads it after them (`0x5ecbcd`).
            self.at_point(script, at);
            if !owed.owes_swings() {
                return;
            }
            owed = self.appliers.select().pay(owed);
        }
    }

    /// After a point: what its listeners selected they signalled themselves.
    fn listened(&mut self, at: &mut Signalled) {
        at.selected = self.appliers.select().selected();
        at.latched = self.appliers.pet().attack_latched();
    }

    /// The player's Attack (`0x6131a0`): the validator (`0x6131aa` → `0x612df0`), whose pick's
    /// `SetSelection` signals inside it, before the re-read (`0x6130c1`) and the final gate; then
    /// the toggle at the target it settled. Its `SelectTarget`s bind a unit to a standing
    /// targeting cursor's cast inside the validator, so that cast is sent ahead of the swing.
    fn attack(&mut self, script: &mut UiScript, at: &mut Signalled, may_pick: bool) {
        self.attack_at(script, at, None, may_pick);
    }

    /// [`Self::attack`] with `passed` as the validator's candidate (`0x6131a0(guid)`). A unit its
    /// select handed the targeting cursor is bound where the select returned (`0x4935d5`), before
    /// the selection's re-read (`0x6130c1`).
    fn attack_at(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        passed: Option<u64>,
        may_pick: bool,
    ) {
        let Some(owed) = self.appliers.attack().pick_attack_at(passed, may_pick) else {
            return;
        };
        self.appliers.cursor().bind_parked();
        self.settle_selection(script, at, owed.into());
        let target = self.appliers.attack().settle_attack();
        if let Some(guid) = target {
            // The toggle (`0x6131ce`): attacking, the stop signals in place; else the start.
            if self.appliers.attack().toggles_off() {
                self.stop_attack(script, at);
            } else {
                self.appliers.attack().start_attack(guid);
            }
        }
    }

    /// A pet bar press past its dispatcher's gates (`0x4bd1d0`): Attack's validator (`0x4bd40d`)
    /// signals its pick's selection in place, ahead of its latch (`0x4bd42e`) and its
    /// `CMSG_PET_ACTION` (`0x4bd4a4`), and a cast the validator binds to a standing targeting
    /// cursor goes out ahead of the packet. A spell press the bar hands to the attempt
    /// (`0x4bd385`) casts through the ladder instead.
    fn pet_step(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        step: Option<crate::ui_pet::PetStep>,
    ) {
        use crate::ui_pet::PetStep;
        let step = match step {
            Some(PetStep::Attack(attack)) => {
                let Some(owed) = self.appliers.pet_press().pick_attack(&attack) else {
                    return;
                };
                // A unit the validator's select handed the targeting cursor binds where the select
                // returned (`0x613100`, `0x4935d5`), ahead of the re-read (`0x6130c1`).
                self.appliers.cursor().bind_parked();
                self.settle_selection(script, at, owed.into());
                let finished = self.appliers.pet_press().finish_attack(attack);
                if finished.as_ref().is_some_and(|&(_, latched)| latched) {
                    // The latch (`0x4bd42e`) signals `PET_BAR_UPDATE` (`0x4bd438`), whose
                    // listeners run before the order goes (`0x4bd4a4`).
                    script.fire_event("PET_BAR_UPDATE", vec![]);
                    self.at_point(script, at);
                }
                finished.map(|(send, _)| PetStep::Order(send))
            }
            // A reaction's or Stay's and Follow's state write signals `PET_BAR_UPDATE`
            // (`0x4bc956`, `0x4bc979`), whose listeners read the lit token before the send
            // (`0x4bd444`).
            Some(PetStep::Order(send)) if send.signals() => {
                script.fire_event("PET_BAR_UPDATE", vec![]);
                self.at_point(script, at);
                Some(PetStep::Order(send))
            }
            step => step,
        };
        let appliers = &mut self.appliers;
        match step {
            Some(PetStep::Order(send)) => appliers.pet_press().finish_press(send),
            Some(PetStep::Cast(cast)) => crate::ui_pet::cast_pet_press(&mut appliers.cast(), cast),
            Some(PetStep::Attack(_)) | None => {}
        }
    }

    /// TryCast's attack pick with a unit as its actor (`0x6e4efb`, `0x6e4ef4`), its selection
    /// signalled in place: the guid the attempt or the tail's attack goes on at.
    fn unit_pick(
        &mut self,
        script: &mut UiScript,
        at: &mut Signalled,
        press: crate::spell::UnitPress,
    ) -> Option<u64> {
        let owed = self.appliers.pet_press().pick_for_unit_cast(press)?;
        self.settle_selection(script, at, owed.into());
        self.appliers.pet_press().settle_pick()
    }

    /// The registrar's close core `0x4f5010` inside its call: with a registrar open, the latch
    /// release (`0x4f5024`), which re-enters the core while the registrar holds the latch, then
    /// the cell cleared (`0x4f502b`) and `GUILD_REGISTRAR_CLOSED` (`0x4f503a`), its signal once
    /// per run of the core, as the call's tail.
    fn close_registrar(&mut self) -> Option<CallTail> {
        // A parked close's re-entry finds its cell clear ([`crate::ui_session::parked_close`]).
        if crate::ui_session::parked_close(crate::ui_session::NpcWindow::Registrar) {
            return None;
        }
        let npc = self.appliers.petition().registrar_npc()?;
        let reenters = self.appliers.latch().release(npc);
        self.appliers.petition().clear_registrar();
        let runs = 1 + usize::from(reenters);
        Some(Box::new(move |script: &mut UiScript| {
            for _ in 0..runs {
                script.fire_event("GUILD_REGISTRAR_CLOSED", vec![]);
            }
        }))
    }

    /// The designer's close core `0x4f58a0` inside `CloseTabardCreation`'s call: with a vendor
    /// stored, the latch release (`0x4f58da`), which re-enters the core while the designer holds
    /// the latch, then the cell cleared (`0x4f58df`) and `CLOSE_TABARD_FRAME` (`0x4f58f8`), once
    /// per run of the core, as the call's tail, after the preview's undo (`0x4f58ce`).
    fn close_tabard(&mut self) -> Option<CallTail> {
        // A parked close's re-entry finds its cell clear ([`crate::ui_session::parked_close`]).
        if crate::ui_session::parked_close(crate::ui_session::NpcWindow::Tabard) {
            return None;
        }
        let vendor = self.appliers.tabard().vendor()?;
        let reenters = self.appliers.latch().release(vendor);
        self.appliers.tabard().clear_vendor();
        let runs = 1 + usize::from(reenters);
        Some(Box::new(move |script: &mut UiScript| {
            script.end_tabard_preview();
            for _ in 0..runs {
                script.fire_event("CLOSE_TABARD_FRAME", vec![]);
            }
        }))
    }

    /// A probe line's text emote, run as `DoEmote` runs it inside its call (`0x5ef560`).
    pub(crate) fn emote(&mut self, script: &mut UiScript, text_id: u32, arg: Option<String>) {
        let appliers = &mut self.appliers;
        emote_in_order!(appliers, appliers.emote().emote(script, text_id, arg));
    }
}

impl CallHost for ScriptCallHost<'_, '_> {
    fn read_item(
        &mut self,
        key: benilla_ui::script::ItemKey,
        read: &mut dyn FnMut(&dyn benilla_ui::script::ItemRead),
    ) -> bool {
        self.appliers.live().items().read(key, read)
    }

    fn read_unit_object(
        &mut self,
        guid: u64,
        read: &mut dyn FnMut(&dyn benilla_ui::script::UnitObjectRead),
    ) -> bool {
        self.appliers.live().units().read(guid, read)
    }

    fn read_unit_identity(
        &mut self,
        guid: u64,
        read: &mut dyn FnMut(&dyn benilla_ui::script::UnitIdentityRead),
    ) -> bool {
        self.appliers.live().units().read_identity(guid, read)
    }

    fn read_unit_tokens(
        &mut self,
        read: &mut dyn FnMut(&dyn benilla_ui::script::UnitTokenObjects),
    ) -> bool {
        self.appliers.live().units().read_tokens(read);
        true
    }

    fn read_unit_stats(
        &mut self,
        guid: u64,
        read: &mut dyn FnMut(
            &dyn benilla_ui::script::UnitObjectRead,
            &dyn benilla_ui::script::CombatRead,
        ),
    ) -> bool {
        self.appliers.live().stats().read(guid, read)
    }

    fn read_own_stats(
        &mut self,
        read: &mut dyn FnMut(Option<&dyn benilla_ui::script::CombatRead>),
    ) -> bool {
        self.appliers.live().stats().read_own(read);
        true
    }

    fn read_self(&mut self, read: &mut dyn FnMut(&dyn benilla_ui::script::SelfRead)) -> bool {
        self.appliers.live().stats().read_self(read);
        true
    }

    fn read_unit_auras(
        &mut self,
        guid: u64,
        read: &mut dyn FnMut(Option<&dyn benilla_ui::script::AuraListRead>),
    ) -> bool {
        match self.appliers.live().auras() {
            Some(auras) => auras.read(guid, read),
            None => false,
        }
    }

    fn read_live(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        self.appliers.live().answer(ask)
    }

    fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
        #[cfg(test)]
        self.census_probe();
        let appliers = &mut self.appliers;
        // The gate's answer for this call, made where the Lua verb ran: its casts start only if
        // tag 2 passed there. The ladder reads it while this call applies.
        let outer_permit = appliers.cast().ladder.permit(call.may_cast());
        let mut at = Signalled {
            selected: appliers.select().selected(),
            latched: appliers.pet().attack_latched(),
            mark: appliers.reports().mark(),
        };
        let owed_from = appliers.select().owed_mark();
        let mut tail: Option<CallTail> = None;
        // The confirm popup an item pick or a Yes raised, signalled with the call's reports.
        let mut popup = None;
        // What a selecting call owes after its signals (`0x493a18`, `0x4938c8`).
        let mut owed = crate::target::SelectDebt::default();
        match call {
            ScriptCall::Select { request, .. } => owed = appliers.select().select(request),
            ScriptCall::TargetByName { name, exact, .. } => {
                owed = appliers.select().target_by_name(&name, exact);
            }
            ScriptCall::TargetNearest { mode, reverse, .. } => {
                owed = appliers.select().target_nearest(mode, reverse);
            }
            ScriptCall::TargetLastTarget { .. } => owed = appliers.select().target_last_target(),
            ScriptCall::ClearTarget => owed = appliers.select().clear_target(),
            ScriptCall::AttackTarget { may_pick, .. } => self.attack(script, &mut at, may_pick),
            ScriptCall::SpellTargetUnit(token) => appliers.cursor().spell_target_unit(&token),
            ScriptCall::SpellStopTargeting => appliers.cursor().stop_targeting(),
            ScriptCall::SpellStopCasting => appliers.self_cancel().stop_casting(),
            ScriptCall::CastSpell {
                spell_id, on_self, ..
            } => {
                crate::ui_spellbook::cast_spell(&mut appliers.cast(), spell_id, on_self);
            }
            ScriptCall::CastShapeshiftForm { spell_id, .. } => {
                crate::ui_shapeshift::cast_form(&mut appliers.cast(), spell_id);
            }
            ScriptCall::UseAction(press) => {
                match crate::ui_action::use_action(&mut appliers.action(), script, press) {
                    // The Attack slot is a cast, so the gate's tag 4 (the pick) is asked only
                    // after tag 2 passed, and so passes as well.
                    crate::ui_action::UseOutcome::Attack => self.attack(script, &mut at, true),
                    crate::ui_action::UseOutcome::Macro(id) => {
                        tail = Some(Box::new(move |script: &mut UiScript| {
                            if !crate::ui_macro::run_macro(script, id) {
                                debug!("script_calls: macro {id} is empty");
                            }
                        }));
                    }
                    // `SetCursorItem`'s signals (`0x494bb6`, `0x494c54`) run their listeners
                    // before `AutoEquipCursorItem` goes on (`0x5e18d5`), and its send.
                    crate::ui_action::UseOutcome::Equip { picked } => {
                        self.at_point(script, &mut at);
                        self.listened(&mut at);
                        crate::ui_action::finish_equip(&mut self.appliers.action(), script, picked);
                    }
                    crate::ui_action::UseOutcome::Done => {}
                }
            }
            ScriptCall::UseContainerItem(call) => {
                if let Some(open) = appliers.item_use().use_container_item(script, call) {
                    tail = appliers.loot().open_item(script, open);
                }
            }
            ScriptCall::UseInventoryItem { slot, on_self, .. } => appliers
                .item_use()
                .use_inventory_item(script, slot, on_self),
            ScriptCall::Item(benilla_ui::script::ItemCall::BindOnUse { may_cast }) => {
                appliers.item_use().confirm_bind_on_use(script, may_cast)
            }
            ScriptCall::Item(benilla_ui::script::ItemCall::EquipAnswer(answer)) => {
                self.answer_pending_equip(script, &mut at, answer)
            }
            ScriptCall::Item(call) => appliers.items().apply(script, call),
            ScriptCall::CastPetSpell { spell_id, on_self } => {
                let send = crate::ui_pet_book::begin_pet_book_cast(
                    &mut appliers.pet_press(),
                    spell_id,
                    on_self,
                );
                if let Some(send) = send {
                    // `GetTargetingSpellId 0x6e48e0` is this spell: the press lowers the cursor
                    // and sends nothing (`0x4b3474`-`0x4b347d`).
                    if appliers.cast().ladder.ground.spell() == Some(spell_id) {
                        appliers.cast().ladder.stop_targeting();
                    } else {
                        crate::ui_pet_book::finish_pet_book_cast(&mut appliers.pet_press(), send);
                    }
                }
            }
            ScriptCall::PetAction(slot) => {
                let step = appliers.pet_press().begin_slot(slot);
                self.pet_step(script, &mut at, step);
            }
            ScriptCall::PetOrder(packed) => {
                let step = appliers.pet_press().begin_order(packed);
                self.pet_step(script, &mut at, step);
            }
            ScriptCall::SendChat(send) => self.send_chat(script, &mut at, send),
            ScriptCall::DisplayError(key) => appliers.chat().display_error(script, key),
            ScriptCall::RandomRoll { min, max } => appliers.chat().random_roll(min, max),
            ScriptCall::Channel(cmd) => {
                if let Some((packet, refused)) = appliers.chat().channel(script, cmd) {
                    // The join's refusal runs its listeners inside the claim (`0x49b9c5`),
                    // before the body sends (`0x49ee02`).
                    if refused {
                        self.at_point(script, &mut at);
                    }
                    self.appliers.chat().send_channel(packet);
                }
            }
            // A posture emote's leave stops the attack in place, its listeners running there.
            ScriptCall::DoEmote(request) => {
                if let Some(step) = appliers.emote().do_emote(script, request) {
                    if step.leave {
                        self.appliers.loot().close_for_stand();
                        self.stop_attack(script, &mut at);
                    }
                    self.appliers.emote().finish(step);
                }
            }
            ScriptCall::Uninvite(name) => appliers.chat().uninvite(name),
            ScriptCall::Logging { combat } => appliers.chat().logging(script, combat),
            ScriptCall::RequestTimePlayed => appliers.chat().request_time_played(),
            ScriptCall::OpeningCinematic => appliers.chat().opening_cinematic(),
            ScriptCall::AddonMessage(send) => appliers.chat().addon_message(send),
            ScriptCall::PickItem(pick) => popup = appliers.cursor().pick_item(pick),
            ScriptCall::EnchantConfirm(answer) => popup = appliers.cursor().confirm_enchant(answer),
            ScriptCall::Loot(call) => tail = appliers.loot().apply(script, call),
            ScriptCall::Auction(AuctionCall::Close) => self.close_auction(script, &mut at),
            ScriptCall::Auction(call) => appliers.auction().apply(script, call),
            ScriptCall::Social(call) => appliers.social().apply(script, call),
            ScriptCall::Party(request) => appliers.party().apply(script, request),
            ScriptCall::Quest(call) => {
                if let Some(giver) = appliers.quest().apply(script, call) {
                    self.finish_quest_teardown(script, &mut at, giver);
                }
            }
            ScriptCall::Gossip(GossipCall::Close) => self.close_gossip(script, &mut at),
            ScriptCall::Gossip(call) => appliers.gossip().apply(script, call),
            ScriptCall::Mail(MailCall::Close) => self.close_mail(script, &mut at),
            // An unread mail's mark signals (`0x4add9d`) and its listeners run before the open
            // arms the close's re-query (`0x4adda6`).
            ScriptCall::Mail(call @ MailCall::Open { unread: true, .. }) => {
                appliers.mail().apply(script, call);
                self.at_point(script, &mut at);
                self.appliers.mail().arm_close_refresh();
            }
            ScriptCall::Mail(call) => appliers.mail().apply(script, call),
            // The partner's slot 7 is the trade's, the bind the targeting cursor's.
            ScriptCall::Trade(
                call @ (benilla_ui::script::TradeCall::TargetEnchant
                | benilla_ui::script::TradeCall::ReplaceEnchant),
            ) => {
                let partner = appliers.trade().partner_enchant_slot();
                let check = call == benilla_ui::script::TradeCall::TargetEnchant;
                appliers.cursor().trade_enchant(script, partner, check);
            }
            ScriptCall::Trade(TradeCall::Close) => self.close_trade(script, &mut at),
            ScriptCall::Trade(call) => tail = appliers.trade().apply(call),
            ScriptCall::Trainer(TrainerCall::Buy { trainer, spell_id }) => {
                appliers.trainer().buy(trainer, spell_id);
            }
            ScriptCall::Trainer(TrainerCall::OpenOwn) => {
                self.open_own_trainer(script, &mut at);
            }
            ScriptCall::Trainer(TrainerCall::Close) => self.close_trainer(script, &mut at),
            ScriptCall::Taxi(TaxiCall::Close) => self.close_taxi(script, &mut at),
            ScriptCall::Taxi(call) => appliers.taxi().apply(call),
            ScriptCall::Merchant(MerchantCall::Close) => {
                self.close_merchant(script, &mut at);
            }
            ScriptCall::Merchant(call) => appliers.merchant().apply(script, call),
            ScriptCall::Guild(call) => appliers.guild().apply(script, call),
            ScriptCall::Petition(PetitionCall::CloseRegistrar) => tail = self.close_registrar(),
            ScriptCall::Petition(PetitionCall::Close) => self.close_petition(script, &mut at),
            // The designer's opener, then the registrar's close core (`0x4f5355` → `0x4f5010`).
            ScriptCall::Petition(PetitionCall::TabardInfo) => {
                appliers.petition().apply(script, PetitionCall::TabardInfo);
                tail = self.close_registrar();
            }
            ScriptCall::Petition(call) => tail = appliers.petition().apply(script, call),
            ScriptCall::Tabard(TabardCall::Close) => tail = self.close_tabard(),
            ScriptCall::Tabard(call) => appliers.tabard().apply(script, call),
            ScriptCall::Talent(call) => appliers.talents().apply(script, call),
            ScriptCall::Skill(call) => appliers.skills().apply(call),
            ScriptCall::Reputation(call) => tail = appliers.reputation().apply(script, call),
            ScriptCall::Pet(call) => appliers.pet().apply(script, call),
            ScriptCall::Stable(StableCall::Close) => self.close_stable(script, &mut at),
            ScriptCall::Stable(call) => appliers.stable().apply(call),
            ScriptCall::Session(request) => appliers.session().apply(request),
            ScriptCall::Tutorial(call) => appliers.tutorial().apply(script, call),
            ScriptCall::Bank(BankCall::Close) => self.close_bank(script, &mut at),
            ScriptCall::Bank(BankCall::Purchase) => appliers.bank().purchase(script),
            ScriptCall::TradeSkill(TradeSkillCall::Do { spell_id, count }) => {
                crate::ui_tradeskill::do_trade_skill(&mut appliers.cast(), spell_id, count);
            }
            ScriptCall::TradeSkill(TradeSkillCall::Close) => appliers.trade_skill().close(),
            ScriptCall::Craft(CraftCall::Do { spell_id }) => {
                crate::ui_craft::do_craft(&mut appliers.cast(), spell_id);
            }
            ScriptCall::Craft(CraftCall::Close) => appliers.craft().close(),
            ScriptCall::Battlefield(call) => tail = appliers.battlefield().apply(script, call),
            ScriptCall::Death(DeathAction::AcceptAreaSpiritHeal) => {
                self.accept_area_spirit_heal(script, &mut at);
            }
            ScriptCall::Death(action) => appliers.death().apply(script, action),
            ScriptCall::Duel(DuelRequest::StartByGuid(guid)) => {
                let spell = appliers.duel().duel_spell();
                if let Some(spell) = spell {
                    crate::ui_duel::cast_duel(&mut appliers.cast(), spell, guid);
                }
            }
            ScriptCall::Duel(request) => appliers.duel().answer(&request),
            ScriptCall::LootRoll(LootRollCall::Vote { roll_id, roll_type }) => {
                appliers.loot_roll().vote(roll_id, roll_type);
            }
            ScriptCall::LootRoll(LootRollCall::Close { roll_id }) => {
                appliers.loot_roll().close(roll_id);
            }
            ScriptCall::Binder(BinderCall::Confirm) => appliers.binder().confirm(),
            ScriptCall::Summon(SummonCall::Confirm) => appliers.summon().confirm(),
            ScriptCall::MeetingStone(MeetingStoneCall::Cancel) => {
                appliers.meeting_stone().cancel(script);
            }
            ScriptCall::Instance(InstanceCall::Reset) => appliers.instance().reset(),
            ScriptCall::Pvp(PvpCall::Toggle) => appliers.pvp().toggle(),
            ScriptCall::WornDisplay(set) => appliers.worn().set(set),
            ScriptCall::ActionBar(ActionBarCall::RefreshUsable) => {
                self.refresh_usable_cache(script, &mut at);
            }
            ScriptCall::ActionBar(call) => {
                appliers.action_bar().apply(call);
                // An item word's count (`0x4e66e9`-`0x4e66f1`), then the write's store of the
                // slot's pair (`0x4e5903`-`0x4e5914`), ahead of its signal.
                if let ActionBarCall::Set { id, .. } | ActionBarCall::Notify { id } = call {
                    if let Some(states) = appliers.action_states().as_mut() {
                        if matches!(call, ActionBarCall::Set { .. }) {
                            states.item_written(script, id);
                        }
                        states.slot_written(script, id);
                    }
                }
            }
            ScriptCall::GmTicket(GmTicketCall::Ticket(intent)) => {
                appliers.gm_ticket().ticket(intent);
            }
            // With none learned the dword is 0, and benilla casts nothing: the attempt on spell 0
            // (`0x6e4b60`) is not traced.
            ScriptCall::GmTicket(GmTicketCall::Stuck) => {
                if let Some(spell) = appliers.gm_ticket().stuck_spell() {
                    crate::ui_gm_ticket::cast_stuck(&mut appliers.cast(), spell);
                }
            }
            ScriptCall::Unserved(send) => appliers.gm_ticket().unserved(send),
            ScriptCall::Inspect(call) => tail = appliers.inspect().apply(script, call),
            ScriptCall::ItemText(ItemTextCall::Turn(delta)) => {
                self.turn_item_text(script, &mut at, delta);
            }
            ScriptCall::ItemText(ItemTextCall::Close) => self.close_item_text(script, &mut at),
            ScriptCall::Aura(AuraCall::Cancel(spell_id)) => {
                self.cancel_aura(script, &mut at, spell_id);
            }
            ScriptCall::Minimap(MinimapCall::Ping { x, y }) => appliers.minimap().ping(x, y),
            ScriptCall::Minimap(MinimapCall::Ground { x, y }) => {
                if let Some(at) = appliers.minimap().ground_under(x, y) {
                    appliers.cursor().bind_location(at);
                }
            }
            ScriptCall::Posture(PostureCall::SitOrStand) => self.sit_or_stand(script, &mut at),
            ScriptCall::ConfirmPetUnlearn => appliers.pet_unlearn().confirm(script),
            ScriptCall::Nameplates(write) => appliers.plates().apply(write),
            ScriptCall::Follow(request) => self.follow(script, &mut at, request),
            ScriptCall::Console(line) => appliers.console().run(&engine::VmCvars(script), &line),
            ScriptCall::CvarWritten => appliers.cvars().sync(Some(script)),
            ScriptCall::CameraView(request) => {
                let writes = appliers.views().apply(request);
                appliers.cvars().write(script, &writes);
            }
            ScriptCall::RestartGx => appliers.gx_restart().restart(script, 1),
            ScriptCall::DressUp(intent) => appliers.dressup().apply(script, intent),
            ScriptCall::RecordAsk(benilla_ui::script::RecordAsk::AuctionName { guid, list }) => {
                appliers.auction().wait_for_name(guid, list);
            }
            ScriptCall::RecordAsk(benilla_ui::script::RecordAsk::MasterLootName(guid)) => {
                appliers.loot().wait_for_name(guid);
            }
            ScriptCall::RecordAsk(benilla_ui::script::RecordAsk::ItemTextCreator(guid)) => {
                appliers.item_text().wait_for_creator(guid);
            }
            ScriptCall::RecordAsk(ask) => appliers.asks().apply(ask),
            ScriptCall::WorldClick(step) => self.world_click(script, &mut at, step),
            ScriptCall::PlateClick(click) => self.plate_click(script, &mut at, click),
            ScriptCall::Input(input) => tail = self.set_commands(script, &mut at, input),
            ScriptCall::Look(call) => tail = self.look(script, &mut at, call),
            ScriptCall::DropItemOnUnit(token) => appliers.drop_item().drop_on(script, &token),
        }
        // A profession opener the call cast toggles its window inside the cast (`0x6e4c22`,
        // `0x6e4c34`), whose signals' listeners run before the call goes on.
        if self.appliers.trade_skill().toggle_opens(script) {
            self.at_point(script, &mut at);
        }
        self.settle_selection(script, &mut at, owed);
        // A deselect the call's own steps raised, a dead unit's loot close's (`0x48f369`), signals
        // inside it (`0x493a73`), before the call returns.
        while let Some(owed) = self.appliers.select().owed_deselect(owed_from) {
            self.settle_selection(script, &mut at, owed);
        }
        let appliers = &mut self.appliers;
        // A selection this call made with the targeting cursor up was handed to the cast
        // (`SelectTarget`, `0x4935d5`) and is bound now, before the next call reads either.
        appliers.cursor().bind_parked();
        // TryCast's attack pick (`0x6e4efb`) can move the selection mid-cast, and its
        // `SetSelection` signals there, before the attempt goes on to its send (`0x6e585a`), so a
        // press held there is picked, its selection signalled in place, and resumed.
        if let Some(held) = appliers.cast().ladder.take_held() {
            // A held press passed tag 2 to be held, so the pick's tag 4 passes as well.
            match held.unit {
                None => {
                    if let Some(owed) = self.appliers.attack().pick_for_cast(held) {
                        self.settle_selection(script, &mut at, owed.into());
                        if self.appliers.attack().settle_attack().is_some() {
                            let mut cast = self.appliers.cast();
                            let ctx = cast.targeting.context();
                            cast.ladder.resume_after_pick(held, &ctx);
                        }
                    }
                }
                // The pet bar's unit as the pick's actor (`0x6e4ef4`), which settles the guid the
                // attempt goes on with (`0x6e4ef7`).
                Some(press) => {
                    if let Some(guid) = self.unit_pick(script, &mut at, press) {
                        let mut cast = self.appliers.cast();
                        if let Some(ctx) =
                            cast.targeting.context_for_caster(press.caster, Some(guid))
                        {
                            cast.ladder.resume_after_pick(held, &ctx);
                        }
                    }
                }
            }
        }
        // A unit caster's send owes its attack tail (`0x6e51de` → `0x6131a0`): its validator with
        // the unit as the actor (`0x6131aa`), whose pick signals in place, then for the bar's unit
        // the bar's Attack (`0x613217`) before the attempt returns.
        if let Some((caster, guid)) = self.appliers.cast().ladder.take_unit_attack() {
            let press = crate::spell::UnitPress {
                caster,
                target: Some(guid),
            };
            if let Some(guid) = self.unit_pick(script, &mut at, press) {
                let step = self.appliers.pet_press().attack_from_cast(caster, guid);
                self.pet_step(script, &mut at, step);
            }
        }
        let appliers = &mut self.appliers;
        // Whatever casts next is the caller's: the engine's own, which asks no gate, or the call
        // whose signal point this call applied at.
        appliers.cast().ladder.permit(outer_permit);
        // A selection the call moved with no signal of its own yet is signalled as the call
        // returns, inside it and under its input events.
        self.signal_selection(script, &mut at);
        // The reports the call made, in its order: a refusal's `UI_ERROR_MESSAGE` and its
        // `SPELLCAST_FAILED` (`ReportCastFailure 0x6e1a00` inside `TryCast 0x6e4b60`), a cancel's
        // event, `CURRENT_SPELL_CAST_CHANGED` at each of the attempt's signals, and the item gate's
        // confirm (`0x495d60`), all signalled below as the call returns, under its input events.
        let ready = self.appliers.reports().ready_since(script, at.mark);
        self.commit_views(script);
        #[cfg(test)]
        self.census_done();
        if ready.is_empty() && popup.is_none() {
            return tail;
        }
        let macro_lines = tail.take();
        Some(Box::new(move |script: &mut UiScript| {
            crate::ui_cast::fire(script, ready);
            if let Some(popup) = popup {
                popup.fire(script);
            }
            if let Some(lines) = macro_lines {
                lines(script);
            }
        }))
    }
}

/// Run `f` on `script` with `host` lent, when the entry has one: a harness without the appliers'
/// state runs it with none, and its calls are refused, or kept by a harness's VM
/// ([`UiScript::keep_calls`]).
pub(crate) fn lent<R>(
    script: &mut UiScript,
    host: Option<&mut ScriptCallHost>,
    f: impl FnOnce(&mut UiScript) -> R,
) -> R {
    match host {
        Some(host) => script.hosted(host, f),
        None => f(script),
    }
}

/// The VM a system runs Lua on, with the call host to lend it ([`HostedVm::run`]): the param a
/// system that holds the VM takes, so whatever Lua it runs (an event's handlers, a widget's
/// script, an `OnUpdate`, a chunk) makes calls that apply inside the verb.
#[derive(SystemParam)]
pub(crate) struct HostedVm<'w, 's> {
    script: Option<NonSendMut<'w, UiScript>>,
    host: Option<ScriptCallHost<'w, 's>>,
}

impl HostedVm<'_, '_> {
    /// Run `f` on the VM with the host lent around all of it ([`hosted`]); `f` gets `None` with
    /// no VM.
    pub(crate) fn run<R>(&mut self, f: impl FnOnce(Option<&mut UiScript>) -> R) -> R {
        hosted(&mut self.script, &mut self.host, f)
    }
}

/// Run a VM system's body with the host lent around all of it, so every Lua call its feeds,
/// events and scripts make applies inside the verb: the reference has no queue between a verb and
/// its effect, whatever entry the script came in by. `f` gets the VM, `None` with no VM.
pub(crate) fn hosted<R>(
    script: &mut Option<NonSendMut<UiScript>>,
    host: &mut Option<ScriptCallHost>,
    f: impl FnOnce(Option<&mut UiScript>) -> R,
) -> R {
    match script.as_deref_mut() {
        Some(script) => lent(script, host.as_mut(), |script| f(Some(script))),
        None => f(None),
    }
}

/// The world frame's half of a freelook a call entered or left (`0x493070`, `0x493090`), inside
/// the call: the entry turns every plate's mouse off, which ends a hovered plate's hover through
/// its leave (`0x483e9a` → `0x60f830`), then publishes the empty mouseover (`0x483ec3`), the hover
/// driver's teardown, so the next line's `"mouseover"` names nobody, then moves the mouse focus to
/// the world frame (`0x493085` → `0x764dc0`); the leave turns the plates' mouse back on
/// (`0x483e70`) and has the hover picked again (`0x4930a0`).
fn freelook_edges(script: &mut UiScript, looked: crate::player::Looked) {
    if looked.entered {
        script.set_nameplate_mouse(false);
        script.set_unit("mouseover", None);
        script.publish_world_mouseover(None);
        script.focus_world_frame();
    } else if looked.left {
        script.set_nameplate_mouse(true);
        script.repick_hover();
    }
}

/// The VM and the host, for a system that also holds `P`, some of the host's own state: the
/// cast ladder of a drain that applies a click's cast, the pet bar a feed pushes. It runs Lua
/// with the host lent ([`VmWith::run`]), or does its engine work on the VM and `P` together as
/// an engine turn ([`VmWith::turn`]), whose calls apply as the turn ends; `P` alone is
/// [`VmWith::own`].
#[derive(SystemParam)]
pub(crate) struct VmWith<'w, 's, P: SystemParam + 'static> {
    script: Option<NonSendMut<'w, UiScript>>,
    set: disjoint::Pair<'w, 's, P, Option<ScriptCallHost<'w, 's>>>,
}

impl<P: SystemParam + 'static> VmWith<'_, '_, P> {
    /// `P`, read or written outside any Lua.
    pub(crate) fn own(&mut self) -> SystemParamItem<'_, '_, P> {
        self.set.p0()
    }

    /// Run `f` on the VM with the host lent around all of it, as [`HostedVm::run`]; `P` waits
    /// outside, so what a call does to it is there for the next [`Self::own`].
    pub(crate) fn run<R>(&mut self, f: impl FnOnce(Option<&mut UiScript>) -> R) -> R {
        let mut host = self.set.p1();
        match self.script.as_deref_mut() {
            Some(script) => lent(script, host.as_mut(), |script| f(Some(script))),
            None => f(None),
        }
    }

    /// Run `f` on the VM and `P` together as an engine turn ([`UiScript::host_out`]): a call the
    /// Lua it runs makes waits, as one made while the host applies a call does, and applies as the
    /// turn ends, with the host lent. With no VM `f` gets `None`; with no host what waited is
    /// dropped.
    #[track_caller]
    pub(crate) fn turn<R>(
        &mut self,
        f: impl FnOnce(Option<&mut UiScript>, SystemParamItem<'_, '_, P>) -> R,
    ) -> R {
        let Some(script) = self.script.as_deref_mut() else {
            return f(None, self.set.p0());
        };
        #[cfg(test)]
        if let Some(host) = self.set.p1().as_ref() {
            host.census_probe();
        }
        let r = script.host_out(|script| {
            let r = f(Some(script), self.set.p0());
            #[cfg(test)]
            self.set.end_lend();
            r
        });
        match self.set.p1().as_mut() {
            Some(host) => script.hosted(host, |_| ()),
            None => {
                script.refuse_waiting();
            }
        }
        r
    }
}

impl<P: SystemParam + 'static> VmWith<'_, '_, P> {
    /// Signal an engine writer's selection, which it committed on `P` ([`EngineSelect`]), in place
    /// with the host ([`ScriptCallHost::engine_select`]); with no VM or no host to lend, nothing
    /// listens, and `pay` pays what it owes on `P`.
    pub(crate) fn engine_select(
        &mut self,
        select: EngineSelect,
        pay: impl FnOnce(SystemParamItem<'_, '_, P>, crate::target::SelectDebt),
    ) {
        if let Some(script) = self.script.as_deref_mut() {
            if let Some(host) = self.set.p1().as_mut() {
                host.engine_select(script, select);
                return;
            }
        }
        pay(self.set.p0(), select.owed);
    }

    /// A field watcher's usable refresh in place with the host
    /// ([`ScriptCallHost::engine_usable_refresh`]); with no VM or no host, nobody listens.
    pub(crate) fn engine_usable_refresh(&mut self) {
        if let Some(script) = self.script.as_deref_mut() {
            if let Some(host) = self.set.p1().as_mut() {
                host.engine_usable_refresh(script);
            }
        }
    }

    /// The world-enter cascade's cache refresh with the host
    /// ([`ScriptCallHost::engine_enter_world_refresh`]): whether a pair changed; with no VM or no
    /// host, none did.
    pub(crate) fn engine_enter_world_refresh(&mut self) -> bool {
        match (self.script.as_deref_mut(), self.set.p1().as_mut()) {
            (Some(script), Some(host)) => host.engine_enter_world_refresh(script),
            _ => false,
        }
    }

    /// `RaiseBagUpdate`'s cache refresh with the host
    /// ([`ScriptCallHost::engine_usable_cache_refresh`]); with no VM or no host, nobody listens.
    pub(crate) fn engine_usable_cache_refresh(&mut self) {
        if let Some(script) = self.script.as_deref_mut() {
            if let Some(host) = self.set.p1().as_mut() {
                host.engine_usable_cache_refresh(script);
            }
        }
    }

    /// A follow stop's autorun clear on the host's bit, VM or not
    /// ([`crate::player::CommandSteps::clear_autorun`]); with no host, nothing holds the bit.
    pub(crate) fn engine_autorun_clear(&mut self) {
        if let Some(host) = self.set.p1().as_mut() {
            host.appliers.commands().clear_autorun();
        }
    }

    /// A slot write with the host ([`ScriptCallHost::engine_slot_written`]); with no VM or no
    /// host, nothing stores the pair.
    pub(crate) fn engine_slot_written(&mut self, action: u32) {
        if let Some(script) = self.script.as_deref_mut() {
            if let Some(host) = self.set.p1().as_mut() {
                host.engine_slot_written(script, action);
            }
        }
    }

    /// Our player's arrival's slot pass with the host ([`ScriptCallHost::engine_arrival_slots`]).
    pub(crate) fn engine_arrival_slots(&mut self) {
        if let Some(script) = self.script.as_deref_mut() {
            if let Some(host) = self.set.p1().as_mut() {
                host.engine_arrival_slots(script);
            }
        }
    }

    /// A recount's count of `entry` with the host ([`ScriptCallHost::engine_item_count`]); `None`
    /// with no VM or no host, where no slot is read.
    pub(crate) fn engine_item_count(&mut self, entry: u32) -> Option<u32> {
        self.script.as_ref()?;
        self.set.p1().as_mut()?.engine_item_count(entry)
    }

    /// `0x4e5de0`'s notify of one slot with the host
    /// ([`ScriptCallHost::engine_spell_slot_notified`]); with no VM or no host nothing is notified.
    pub(crate) fn engine_spell_slot_notified(&mut self, action: u32, spell: u32) -> bool {
        let Some(script) = self.script.as_deref_mut() else {
            return false;
        };
        self.set
            .p1()
            .as_mut()
            .is_some_and(|host| host.engine_spell_slot_notified(script, action, spell))
    }

    /// A recount's write of one slot with the host ([`ScriptCallHost::engine_recounted`]).
    pub(crate) fn engine_recounted(&mut self, action: u32, entry: u32, count: u32) -> bool {
        let Some(script) = self.script.as_deref_mut() else {
            return false;
        };
        self.set
            .p1()
            .as_mut()
            .is_some_and(|host| host.engine_recounted(script, action, entry, count))
    }

    /// An engine writer's `SelectTarget(0)`, signalled in place with the host
    /// ([`ScriptCallHost::engine_clear_target`]); with no VM or no host nobody listens and the
    /// selection stands.
    pub(crate) fn engine_clear_target(&mut self, mark: crate::lanes::Mark) {
        if let Some(script) = self.script.as_deref_mut() {
            if let Some(host) = self.set.p1().as_mut() {
                host.engine_clear_target(script, mark);
            }
        }
    }

    /// A go's deferred Attack at `guid`, its validator's select signalled in place with the host
    /// ([`ScriptCallHost::engine_attack`]); with no VM or no host, `start` runs it on `P`.
    pub(crate) fn engine_attack(
        &mut self,
        guid: u64,
        mark: crate::lanes::Mark,
        start: impl FnOnce(SystemParamItem<'_, '_, P>),
    ) {
        if let Some(script) = self.script.as_deref_mut() {
            if let Some(host) = self.set.p1().as_mut() {
                host.engine_attack(script, guid, mark);
                return;
            }
        }
        start(self.set.p0());
    }
}

impl<'w, 's, P: SystemParam + 'static> VmWith<'w, 's, P> {
    /// The VM and the set apart, for a drain that lends the host to each item's Lua in turn
    /// ([`lent`]) and reads or writes `P` between them, never while the host is lent.
    pub(crate) fn parts(
        &mut self,
    ) -> (
        Option<&mut UiScript>,
        &mut disjoint::Pair<'w, 's, P, Option<ScriptCallHost<'w, 's>>>,
    ) {
        (self.script.as_deref_mut(), &mut self.set)
    }
}

/// Run `f` on the world's VM with the host lent, for an entry that holds the whole world (a
/// reload, a session's end, a world-leave sweep): the host is built off the world as a system's
/// would be, and what its calls queued applied after. `f` gets `None` with no VM.
pub(crate) fn with_hosted_vm<R>(
    world: &mut World,
    f: impl FnOnce(Option<&mut UiScript>) -> R,
) -> R {
    let mut state = bevy::ecs::system::SystemState::<HostedVm>::new(world);
    let r = state.get_mut(world).run(f);
    state.apply(world);
    crate::creature_anim::apply_unit_edits(world);
    r
}

/// A fresh VM in `world`, for a test of an engine entry outside the feeds.
#[cfg(test)]
pub(crate) fn insert_test_vm(world: &mut World) {
    world.insert_non_send_resource(UiScript::new().expect("a VM"));
}

/// Defer what the world's VM fires while the guard lives ([`UiScript::defer`]), with the mark to
/// park it from ([`park_since`]); `None` with no VM.
pub(crate) fn defer_vm(world: &World) -> Option<(benilla_ui::script::Deferring, usize)> {
    world
        .get_non_send_resource::<UiScript>()
        .map(|vm| (vm.defer(), vm.waiting_mark()))
}

/// [`defer_vm`] split: its calls apply at once through the lent host, its signals and writes park
/// ([`UiScript::defer_split`]).
pub(crate) fn defer_vm_split(world: &World) -> Option<(benilla_ui::script::Deferring, usize)> {
    world
        .get_non_send_resource::<UiScript>()
        .map(|vm| (vm.defer_split(), vm.waiting_mark()))
}

/// Park what the world's VM has held since `mark` ([`UiScript::park_since`]); the VM's session
/// and the key, `None` with nothing held or no VM.
pub(crate) fn park_since(world: &mut World, mark: usize) -> Option<(u64, u64)> {
    let mut vm = world.get_non_send_resource_mut::<UiScript>()?;
    let key = vm.park_since(mark)?;
    Some((vm.session(), key))
}

/// Run what the world's VM parked under `key` ([`UiScript::run_parked`]), with the host lent,
/// if it is still the VM of `session`: a replaced VM's keys count from 0 again in the next.
pub(crate) fn run_parked(world: &mut World, session: u64, key: u64) {
    fn run(In((session, key)): In<(u64, u64)>, mut vm: HostedVm) {
        vm.run(|script| {
            if let Some(script) = script.filter(|s| s.session() == session) {
                script.run_parked(key);
            }
        });
    }
    if let Err(err) = world.run_system_cached_with(run, (session, key)) {
        debug!("script calls: parked signals did not run: {err}");
    }
}

/// What the world's VM answers to `chunk`, for a test of an engine entry outside the feeds.
#[cfg(test)]
pub(crate) fn eval_in_vm<T: benilla_ui::mlua::FromLuaMulti>(world: &World, chunk: &str) -> T {
    world
        .non_send_resource::<UiScript>()
        .eval(chunk)
        .expect("the chunk runs")
}

/// Run `f` on `script`, a VM out of the world (the UI load runs on one, with the world in hand),
/// with a host built off the world lent, as a system's would be, and apply what its calls queued
/// after. A world without the appliers' state (a bare test world) lends none: the calls are
/// dropped as `f` returns ([`UiScript::refusing`]).
pub(crate) fn lent_by_world<R>(
    world: &mut World,
    script: &mut UiScript,
    f: impl FnOnce(&mut UiScript) -> R,
) -> R {
    let mut state = bevy::ecs::system::SystemState::<Option<ScriptCallHost>>::new(world);
    let r = {
        let mut host = state.get_mut(world);
        match host.as_mut() {
            Some(host) => script.hosted(host, f),
            None => script.refusing(f),
        }
    };
    state.apply(world);
    crate::creature_anim::apply_unit_edits(world);
    r
}

/// Run `system` as a one-shot from an exclusive caller, as [`World::run_system_cached`] does, and
/// apply the unit edits it queued as it returns, as that applies its commands: a one-shot that
/// holds the host or a seam leaves nothing for a later flush to find.
pub(crate) fn run_cached<O: 'static, M, S: IntoSystem<(), O, M> + 'static>(
    world: &mut World,
    system: S,
) -> Result<O, bevy::ecs::system::RegisteredSystemError<(), O>> {
    let r = world.run_system_cached(system);
    crate::creature_anim::apply_unit_edits(world);
    r
}

/// Bind the units a selection writer's `SelectTarget` found the targeting cursor up for
/// (`0x4935d5`): a call binds its own as it applies, and a handler at its select
/// ([`crate::spell::PickBind`]).
pub(crate) fn bind_parked_picks(mut bind: crate::spell::PickBind) {
    bind.bind();
}

/// [`bind_parked_picks`] after a walk's arrival in the frame time, whose dispatch selects through
/// the same front.
pub(crate) fn bind_arrival_picks(mut bind: crate::spell::PickBind) {
    bind.bind();
}

/// [`bind_parked_picks`] from an exclusive writer, at its select's place; a harness without the
/// cursor's seams binds nothing.
pub(crate) fn bind_picks_here(world: &mut World) {
    if let Err(err) = run_cached(world, bind_parked_picks) {
        debug!("targeting: the parked picks were not bound: {err}");
    }
}

/// The census list ([`benilla_ui::script::census`]).
#[cfg(test)]
pub(crate) mod census;

/// A host that applies each call with `apply`, for a harness that lends its own engine half.
#[cfg(test)]
pub(crate) struct FnHost<F>(pub(crate) F);

#[cfg(test)]
impl<F: FnMut(&mut UiScript, ScriptCall)> CallHost for FnHost<F> {
    fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
        (self.0)(script, call);
        None
    }
}

/// [`FnHost`] whose getters' live questions `L` answers, as the app's host's families do.
#[cfg(test)]
pub(crate) struct FnHostReading<F, L>(pub(crate) F, pub(crate) L);

#[cfg(test)]
impl<F: FnMut(&mut UiScript, ScriptCall), L: live::LiveFamily> CallHost for FnHostReading<F, L> {
    fn apply(&mut self, script: &mut UiScript, call: ScriptCall) -> Option<CallTail> {
        (self.0)(script, call);
        None
    }

    fn read_live(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        self.1.answer(ask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{
        ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfGuid, SelfPlayer,
    };
    use crate::target::Selection;
    use benilla_protocol::messages::{
        ActionButton, GroupMemberEntry, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
    };
    use benilla_ui::script::{PetBookState, SpellBookState, SpellSlotView, SpellTabView};
    use bevy::ecs::system::RunSystemOnce;
    use crossbeam_channel::Receiver;

    mod asks;
    mod cast_reports;
    mod census;
    mod closes;
    mod dialogs;
    mod engine;
    mod plates;
    mod refusals;
    mod selection;
    mod sends;
    mod systems;
    mod ticks;
    mod window_verbs;

    const ME: u64 = 0x10;
    /// The unit selected before the script runs.
    const OLD: u64 = 0x21;
    /// `party1`, the unit the script selects.
    const NEW: u64 = 0x22;
    /// Not in the (absent) spell catalog, so the cast's wire target is the selection as it stands.
    const HEAL: u32 = 2050;
    /// A mob the TAB scan can pick: it alone carries a `NetEntity`.
    const MOB: u64 = 0x31;
    /// `UNIT_FIELD_FACTIONTEMPLATE`, `UNIT_FIELD_HEALTH`, `UNIT_FIELD_FLAGS`.
    const TEMPLATE: u16 = 35;
    const HEALTH: u16 = 22;
    const FLAGS: u16 = 46;
    /// The fixture's templates: ours (group 1, friendly toward group 2, enemy of group 4), a
    /// friendly NPC's (group 2) and a hostile mob's (group 4).
    const OURS: u32 = 1;
    const FRIEND: u32 = 2;
    const FOE: u32 = 3;

    struct Frame {
        app: App,
        rx: Receiver<ClientCommand>,
    }

    /// The whole client, headless, with us, `OLD` (selected when `selected`) and `NEW` as
    /// `party1`, and a VM whose book holds Heal.
    fn frame(selected: bool) -> Frame {
        let mut app = crate::game_plugins::schedule_tests::headless_client_unrun();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        // Fields 22 health and 28 max health: every unit is alive.
        let unit = |guid: u64, x: f32| {
            (
                Guid(guid),
                Transform::from_xyz(x, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(x, 0.0, 0.0)),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (22, 100),
                    (28, 100),
                ])),
            )
        };
        // We are a player (`OBJECT_FIELD_TYPE` 0x19, `UNIT_FLAG_PVP_ATTACKABLE`), which selects
        // `CanAttack`'s player arm: with no `FactionTemplate.dbc` a plain unit is attackable.
        let me = world.spawn((SelfPlayer, unit(ME, 0.0))).id();
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
                (46, 0x8),
            ])));
        let old = world.spawn(unit(OLD, 5.0)).id();
        let new = world.spawn(unit(NEW, 6.0)).id();
        let index = &mut world.resource_mut::<GuidIndex>().0;
        index.insert(ME, me);
        index.insert(OLD, old);
        index.insert(NEW, new);
        world.resource_mut::<SelfGuid>().0 = Some(ME);
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .members
            .push(GroupMemberEntry {
                name: "Bob".into(),
                guid: NEW,
                status: 1,
                flags: 0,
            });
        let mut script = UiScript::new().expect("a VM");
        script.set_spellbook(SpellBookState {
            tabs: vec![SpellTabView {
                name: "Holy".into(),
                texture: None,
                offset: 0,
                num_spells: 1,
            }],
            slots: vec![SpellSlotView {
                spell_id: HEAL,
                name: "Heal".into(),
                ..Default::default()
            }],
        });
        record_reports(&script);
        world.insert_non_send_resource(script);
        let mut f = Frame { app, rx };
        if selected {
            f.rx.try_iter().for_each(drop);
            *f.app.world_mut().resource_mut::<Selection>() = Selection {
                target: Some(old),
                guid: Some(OLD),
                ..Default::default()
            };
            // The frame's unit snapshot, as the last feed pushed it.
            f.app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .set_unit(
                    "target",
                    Some(benilla_ui::script::UnitState {
                        exists: true,
                        ..Default::default()
                    }),
                );
        }
        f
    }

    /// Every catalog line and cast-failure string defined as its own key, but the default
    /// cast-failure format, 1.12's bare `"%s"`, which shows the reason's own; and a recorder of
    /// the lines shown (`UI_ERROR_MESSAGE`) in `LINES`, and of those lines and the cast's events
    /// in `REPORTS`, in the order they fire.
    fn record_reports(script: &UiScript) {
        let mut lua = String::new();
        for key in benilla_ui::messages::CATALOG
            .iter()
            .map(|r| r.key)
            .chain(crate::ui_action::CAST_FAIL_KEYS)
            .filter(|k| !k.is_empty())
        {
            lua.push_str(&format!("{key} = \"{key}\"\n"));
        }
        lua.push_str("ERR_SPELL_FAILED_S = \"%s\"\n");
        lua.push_str(
            r#"LINES, REPORTS = {}, {}
            local r = CreateFrame("Frame")
            for _, e in ipairs({"UI_ERROR_MESSAGE", "SPELLCAST_FAILED", "SPELLCAST_INTERRUPTED",
                    "SPELLCAST_STOP"}) do
                r:RegisterEvent(e)
            end
            r:SetScript("OnEvent", function()
                if event == "UI_ERROR_MESSAGE" then
                    table.insert(LINES, arg1)
                    table.insert(REPORTS, arg1)
                else
                    table.insert(REPORTS, event)
                end
            end)"#,
        );
        script.run(&lua).expect("the recorder installs");
    }

    /// The lines and cast events [`record_reports`] saw since the last read, in order.
    fn reports(f: &mut Frame) -> Vec<String> {
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<Vec<String>>("local r = REPORTS REPORTS = {} return r")
            .expect("the recorder reads")
    }

    /// Run `lua` in the VM with the host lent, as every entry that runs Lua lends it.
    fn run(f: &mut Frame, lua: &str) {
        with_hosted_vm(f.app.world_mut(), |s| s.expect("a VM").run(lua)).expect("the script runs");
    }

    /// Evaluate `lua` in the VM with the host lent.
    fn eval<T: benilla_ui::mlua::FromLuaMulti>(f: &mut Frame, lua: &str) -> T {
        with_hosted_vm(f.app.world_mut(), |s| s.expect("a VM").eval::<T>(lua))
            .expect("the chunk runs")
    }

    /// [`run`] with `lua` as a key's body, an input event of the protected-action gate
    /// (`[[0xb71290]+0xd8]`): the press a macro's `/script` line needs to cast.
    fn run_pressed(f: &mut Frame, lua: &str) {
        with_hosted_vm(f.app.world_mut(), |s| s.expect("a VM").run_binding(lua))
            .expect("the script runs");
    }

    /// The `CMSG_CAST_SPELL`s sent, as `(spell, target)`.
    fn casts(f: &Frame) -> Vec<(u32, Option<u64>)> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { spell_id, target } => Some((spell_id, target)),
                _ => None,
            })
            .collect()
    }

    fn selected(f: &Frame) -> Option<u64> {
        f.app.world().resource::<Selection>().guid
    }

    /// `MOB` at 3 yd, alive, with `fields` over its health.
    /// `guid`'s object typed a player (`OBJECT_FIELD_TYPE` 0x19, player-controlled), as the getters
    /// name a unit the object manager holds (typemask 8).
    pub(super) fn as_player(world: &mut World, guid: u64) {
        typed(world, guid, &[(2, 0x19), (46, 0x8)]);
    }

    /// `guid`'s object typed a creature (`OBJECT_FIELD_TYPE` 0x9).
    pub(super) fn as_creature(world: &mut World, guid: u64) {
        typed(world, guid, &[(2, 0x9)]);
    }

    fn typed(world: &mut World, guid: u64, pairs: &[(u16, u32)]) {
        let entity = world.resource::<GuidIndex>().0[&guid];
        world
            .get_mut::<ObjectStore>(entity)
            .unwrap()
            .0
            .merge(benilla_protocol::ObjectFields::from_pairs(pairs));
    }

    fn spawn_mob(f: &mut Frame, fields: &[(u16, u32)]) {
        let world = f.app.world_mut();
        let mut pairs = vec![(HEALTH, 100), (28, 100)];
        pairs.extend_from_slice(fields);
        let mob = world
            .spawn((
                Guid(MOB),
                crate::net::NetEntity {
                    kind: benilla_protocol::EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Transform::from_xyz(3.0, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(3.0, 0.0, 0.0)),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&pairs)),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(MOB, mob);
    }

    /// Give the world a faction table, us [`OURS`] and `OLD` the `old` template with `fields`
    /// over it.
    fn with_factions(f: &mut Frame, old: u32, fields: &[(u16, u32)]) {
        use benilla_formats::{FactionCatalog, FactionTemplate};
        use std::collections::HashMap;
        let tpl = |faction, group_mask, friend_group_mask, enemy_group_mask| FactionTemplate {
            faction,
            group_mask,
            friend_group_mask,
            enemy_group_mask,
            ..Default::default()
        };
        let world = f.app.world_mut();
        world.insert_resource(crate::target::Factions::from_catalog(
            FactionCatalog::from_rows(
                HashMap::from([
                    (OURS, tpl(OURS, 1, 2, 4)),
                    (FRIEND, tpl(FRIEND, 2, 0, 0)),
                    (FOE, tpl(FOE, 4, 0, 0)),
                ]),
                HashMap::new(),
            ),
        ));
        let index = &world.resource::<GuidIndex>().0;
        let (me, old_unit) = (index[&ME], index[&OLD]);
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (HEALTH, 100),
                (28, 100),
                (FLAGS, 0x8),
                (TEMPLATE, OURS),
            ])));
        let mut pairs = vec![(HEALTH, 100), (28, 100), (TEMPLATE, old)];
        pairs.extend_from_slice(fields);
        world
            .entity_mut(old_unit)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(
                &pairs,
            )));
    }

    /// The `CMSG_ATTACKSWING`s sent, and whether a `CMSG_ATTACKSTOP` went out.
    fn swings(f: &Frame) -> (Vec<u64>, bool) {
        let mut stopped = false;
        let swings =
            f.rx.try_iter()
                .filter_map(|c| match c {
                    ClientCommand::AttackSwing { guid } => Some(guid),
                    ClientCommand::AttackStop => {
                        stopped = true;
                        None
                    }
                    _ => None,
                })
                .collect();
        (swings, stopped)
    }

    /// The world points a probe line's drain reads ([`crate::ui_chat::drain_chat_input_once`]).
    fn probe_line_world(world: &mut World) {
        benilla_world::world_point::init_world_point_resources(world);
    }

    /// The keys of the error lines shown, each defined as its own key ([`record_reports`]).
    fn errors(f: &Frame) -> Vec<String> {
        f.app
            .world()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("return LINES")
            .expect("the recorder reads")
    }

    /// `CastSpellByName(name, onSelf)` (`0x4b4ab0`) and `CastSpell(slot, book, onSelf)` (`0x4b42f0`)
    /// swap the target guid for the active player's (`0x4b4afa`, `0x4b4345`) when
    /// `GetBoolOrDefault` reads the flag true, so the cast goes to us whoever is selected, and the
    /// selection stays where it was. Without the flag it goes to the selection.
    #[test]
    fn on_self_casts_at_the_player_and_leaves_the_selection() {
        for (lua, target, selection) in [
            (r#"CastSpellByName("Heal", 1)"#, ME, OLD),
            (r#"CastSpellByName("Heal", "yes")"#, ME, OLD),
            (r#"CastSpell(1, "spell", 1)"#, ME, OLD),
            (
                r#"TargetUnit("party1") CastSpellByName("Heal", 1)"#,
                ME,
                NEW,
            ),
            (r#"CastSpellByName("Heal")"#, OLD, OLD),
            (r#"CastSpellByName("Heal", 0.5)"#, OLD, OLD),
            (r#"CastSpellByName("Heal", "false")"#, OLD, OLD),
            (r#"CastSpell(1, "spell")"#, OLD, OLD),
            (r#"CastSpell(1, "spell", "0")"#, OLD, OLD),
        ] {
            let mut f = frame(true);
            run(&mut f, lua);
            assert_eq!(casts(&f), vec![(HEAL, Some(target))], "{lua}");
            assert_eq!(selected(&f), Some(selection), "{lua}");
        }
    }

    /// Power Word: Fortitude's word, an assist unit (implicit target 21), in the catalog and
    /// the book.
    const FORTITUDE: u32 = 1243;

    fn with_fortitude(f: &mut Frame) {
        let world = f.app.world_mut();
        let display = benilla_formats::SpellDisplay {
            name: "Fortitude".into(),
            implicit_target_a1: 21,
            ..Default::default()
        };
        seat_book(world, FORTITUDE, display);
    }

    /// The bind runs the ordinary relation chain over the player: with a unit we cannot assist
    /// selected and `autoSelfCast` off, the plain call binds nothing (the cursor comes up), while
    /// `onSelf` binds us; with a friendly player selected the plain call binds that player.
    #[test]
    fn on_self_binds_the_player_where_the_selection_is_not_assistable() {
        for (selected_template, lua, target) in [
            (FOE, r#"CastSpellByName("Fortitude")"#, None),
            (FOE, r#"CastSpellByName("Fortitude", 1)"#, Some(ME)),
            (FOE, r#"CastSpell(1, "spell", 1)"#, Some(ME)),
            (FRIEND, r#"CastSpellByName("Fortitude")"#, Some(OLD)),
            (FRIEND, r#"CastSpellByName("Fortitude", 1)"#, Some(ME)),
        ] {
            let mut f = frame(true);
            // A player (`OBJECT_FIELD_TYPE` 0x19, `UNIT_FLAG_PLAYER_CONTROLLED`): `CanAssist`
            // judges him on his own player's rungs, then the templates.
            with_factions(&mut f, selected_template, &[(2, 0x19), (FLAGS, 0x8)]);
            with_fortitude(&mut f);
            run(&mut f, lua);
            let sent: Vec<_> = target.into_iter().map(|t| (FORTITUDE, Some(t))).collect();
            assert_eq!(casts(&f), sent, "{lua} at template {selected_template}");
            assert_eq!(selected(&f), Some(OLD), "{lua}");
        }
    }

    /// The pet's guid, and the pet book's Growl (`CMSG_PET_ACTION`'s type-1 word, `0x4b34ce`).
    const PET: u64 = 0x50;
    const GROWL: u32 = 2649;

    /// The `CMSG_PET_ACTION`s sent, as `(pet, word, target)`.
    fn pet_actions(f: &Frame) -> Vec<(u64, u32, u64)> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::PetAction {
                    pet_guid,
                    packed,
                    target_guid,
                } => Some((pet_guid, packed, target_guid)),
                _ => None,
            })
            .collect()
    }

    /// `0x4b3300`'s pet fork sends the guid it was handed as the order's target (`0x4b34ce`), so
    /// `CastSpell(slot, "pet", 1)` aims the pet's spell at the player, not the selection.
    #[test]
    fn a_pet_book_cast_on_self_aims_at_the_player() {
        for (lua, target) in [
            (r#"CastSpell(1, "pet", 1)"#, ME),
            (r#"CastSpell(1, "pet", "on")"#, ME),
            (r#"CastSpell(1, "pet")"#, OLD),
            (r#"CastSpell(1, "pet", "0")"#, OLD),
        ] {
            let mut f = frame(true);
            f.app
                .world_mut()
                .resource_mut::<crate::ui_pet::PetBar>()
                .spells
                .pet_guid = PET;
            f.app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .set_pet_book(PetBookState {
                    token: Some("PET".into()),
                    slots: vec![SpellSlotView {
                        spell_id: GROWL,
                        name: "Growl".into(),
                        ..Default::default()
                    }],
                });
            run(&mut f, lua);
            assert_eq!(
                pet_actions(&f),
                vec![(PET, 0x0100_0000 | GROWL, target)],
                "{lua}"
            );
            assert_eq!(selected(&f), Some(OLD), "{lua}");
        }
    }

    /// `0x4b3300` stops the targeting cursor when the book spell pressed is the one awaiting its
    /// click (`0x4b3474` `GetTargetingSpellId`, `0x4b347d` `StopTargeting`), ahead of its pet fork,
    /// so `CastSpell(slot, "pet")` on the pet's own armed spell lowers the cursor and sends no order.
    #[test]
    fn a_pet_book_cast_of_the_armed_spell_stops_targeting() {
        let mut f = frame(true);
        let world = f.app.world_mut();
        world
            .resource_mut::<crate::ui_pet::PetBar>()
            .spells
            .pet_guid = PET;
        world
            .non_send_resource_mut::<UiScript>()
            .set_pet_book(PetBookState {
                token: Some("PET".into()),
                slots: vec![SpellSlotView {
                    spell_id: GROWL,
                    name: "Growl".into(),
                    ..Default::default()
                }],
            });
        world.resource_mut::<crate::spell::SpellTargeting>().enter(
            GROWL,
            crate::spell::CastCommit::Pet { caster: PET },
            0x0002,
        );
        run(&mut f, r#"CastSpell(1, "pet")"#);
        assert_eq!(pet_actions(&f), vec![], "no order");
        assert!(
            !f.app
                .world()
                .resource::<crate::spell::SpellTargeting>()
                .active(),
            "the cursor is down"
        );
    }

    /// `/target` then `/cast`: `TargetUnit` commits through `SetSelection 0x493540` before it
    /// returns, and `CastSpellByName` (`0x4b4ab0`) casts at the selection as it stands then.
    #[test]
    fn a_cast_after_a_target_goes_to_the_new_target() {
        let mut f = frame(true);
        run(&mut f, r#"TargetUnit("party1") CastSpellByName("Heal")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(NEW))]);
        assert_eq!(selected(&f), Some(NEW));
    }

    /// `/cast` then `/target`: the cast is done before the selection moves.
    #[test]
    fn a_cast_before_a_target_goes_to_the_old_target() {
        let mut f = frame(true);
        run(&mut f, r#"CastSpellByName("Heal") TargetUnit("party1")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(OLD))]);
        assert_eq!(selected(&f), Some(NEW));
    }

    /// `ClearTarget` (`0x489ff0`) reads the selection at call time and deselects through
    /// `0x493540(0,0)`: after `TargetUnit("player")` there is one to clear, with or without a
    /// target before the script ran.
    #[test]
    fn a_clear_after_a_target_ends_with_nothing_selected() {
        for selected_before in [true, false] {
            let mut f = frame(selected_before);
            run(&mut f, r#"TargetUnit("player") ClearTarget()"#);
            assert_eq!(selected(&f), None, "selected before: {selected_before}");
            let world = f.app.world();
            assert_eq!(world.resource::<Selection>().target, None);
            let sends: Vec<u64> =
                f.rx.try_iter()
                    .filter_map(|c| match c {
                        ClientCommand::SetSelection { guid } => Some(guid),
                        _ => None,
                    })
                    .collect();
            assert_eq!(sends, vec![ME, 0], "selected before: {selected_before}");
        }
    }

    /// The resolver reads the target's `UNIT_FIELD_TARGET` live (`0x515970`, the hop at
    /// `0x515a0f`), so `"targettarget"` follows each selection a call commits: after
    /// `TargetUnit` it names the new target's target, and after `ClearTarget` nobody, whatever
    /// the frame pushed for the selection the chunk started on.
    #[test]
    fn target_of_target_follows_the_selection_a_call_commits() {
        use benilla_ui::script::UnitState;
        let mut f = frame(true);
        let world = f.app.world_mut();
        // `party1`, a player (`OBJECT_FIELD_TYPE`), targets us (`UNIT_FIELD_TARGET`, fields 16
        // and 17).
        let new = world.resource::<GuidIndex>().0[&NEW];
        world
            .entity_mut(new)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
                (16, ME as u32),
                (17, 0),
            ])));
        // Our name as the getters read it off the name cache at the call (`0x609210`).
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(ME, "Me".into(), None);
        let mut script = world.non_send_resource_mut::<UiScript>();
        let named = |guid, name: &str| UnitState {
            exists: true,
            has_object: true,
            guid,
            name: Some(name.into()),
            ..Default::default()
        };
        script.set_unit("player", Some(named(ME, "Me")));
        // The frame's push for the old selection's target.
        script.set_unit("targettarget", Some(named(0x77, "Stale")));
        let tot = eval::<Option<String>>(
            &mut f,
            r#"TargetUnit("party1") return UnitName("targettarget")"#,
        );
        assert_eq!(tot.as_deref(), Some("Me"), "the new target's target");
        let exists =
            eval::<Option<i64>>(&mut f, r#"ClearTarget() return UnitExists("targettarget")"#);
        assert_eq!(exists, None, "no target, so no target of target");
    }

    /// `TargetUnit` then `UnitHealth("target")` in one chunk reads the new target's object: the
    /// verb commits through `SetSelection` (`0x4899d0` → `0x493540`), which stores the guid
    /// (`0x4936e2`) before it returns, and the getter resolves it and reads the object's health
    /// (`0x5174d0`, `+0x40` at `0x517553`), though no snapshot of the unit was ever pushed.
    #[test]
    fn a_unit_targeted_in_the_chunk_reads_its_object() {
        let mut f = frame(false);
        let health = eval::<i64>(
            &mut f,
            r#"TargetUnit("party1") return UnitHealth("target")"#,
        );
        assert_eq!(selected(&f), Some(NEW));
        assert_eq!(health, 100, "the object's health, not an empty seat's");
    }

    /// `TargetNearestEnemy` (`0x489a80` → `0x493f60(0, 1)`) commits through `SetSelection` before
    /// it returns, so the cast after it in the same script goes to the unit the TAB cycle picked.
    #[test]
    fn a_cast_after_target_nearest_enemy_goes_to_the_picked_unit() {
        let mut f = frame(true);
        // OLD and NEW carry no `NetEntity`, so the scan's one candidate is the mob.
        spawn_mob(&mut f, &[]);
        run(&mut f, r#"TargetNearestEnemy() CastSpellByName("Heal")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(MOB))]);
        assert_eq!(selected(&f), Some(MOB));
    }

    /// `TargetLastTarget` (`0x489b00`) reads the pair `SetSelection` stamps with the outgoing
    /// selection (`0x49361d`): a held pair re-selects, which swaps the two; an empty one deselects
    /// through `0x493540(0,0)` (`0x489b2d`), which stamps what it drops.
    #[test]
    fn target_last_target_swaps_and_deselects_on_an_empty_pair() {
        let mut f = frame(false);
        // Selecting from nothing stamps nothing: the pair is empty, so the call deselects.
        run(&mut f, r#"TargetUnit("party1") TargetLastTarget()"#);
        assert_eq!(selected(&f), None);
        // The deselect stamped NEW, so the next call brings it back.
        run(&mut f, "TargetLastTarget()");
        assert_eq!(selected(&f), Some(NEW));
        // A switch stamps the outgoing unit; the call swaps back, and the cast after it in the
        // same script goes to the unit it re-selected.
        run(
            &mut f,
            r#"TargetUnit("player") TargetLastTarget() CastSpellByName("Heal")"#,
        );
        assert_eq!(selected(&f), Some(NEW));
        assert_eq!(casts(&f), vec![(HEAL, Some(NEW))]);
        run(&mut f, "TargetLastTarget()");
        assert_eq!(selected(&f), Some(ME), "and back again");
    }

    /// `AttackTarget` (`0x489b50` → `0x6131a0`) swings at the selection as the calls before it
    /// left it. Its send stores the attack target at once (`0x5ecd51` → `0x47bf70`), so the switch
    /// after it is the attacking switch, whose StopAttack sends (`0x493a08`), and whose re-swing
    /// StartAttack refuses at the party member (`0x5ecc37`).
    #[test]
    fn attack_target_swings_at_the_selection_in_call_order() {
        let mut f = frame(true);
        run(&mut f, r#"AttackTarget() TargetUnit("party1")"#);
        assert_eq!(swings(&f), (vec![OLD], true));
        let mut f = frame(true);
        run(&mut f, r#"TargetUnit("party1") AttackTarget()"#);
        assert_eq!(swings(&f), (vec![NEW], false));
    }

    /// A seated player's `AttackTarget` stands first: StartAttack's `SetStandState(0)`
    /// (`0x5ecc9b` → `0x5ed430`) sends `CMSG_STANDSTATECHANGE` in place, ahead of
    /// `CMSG_ATTACKSWING` (`0x5ecd3c`).
    #[test]
    fn a_seated_attack_stands_before_the_swing() {
        let mut f = frame(true);
        let world = f.app.world_mut();
        let me = world.resource::<GuidIndex>().0[&ME];
        world.get_mut::<ObjectStore>(me).unwrap().0.merge(
            benilla_protocol::ObjectFields::from_pairs(&[(
                benilla_protocol::field::FIELD_UNIT_BYTES_1,
                1,
            )]),
        );
        run(&mut f, "AttackTarget()");
        let sent: Vec<ClientCommand> =
            f.rx.try_iter()
                .filter(|c| {
                    matches!(
                        c,
                        ClientCommand::StandStateChange { .. } | ClientCommand::AttackSwing { .. }
                    )
                })
                .collect();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::StandStateChange { state: 0 },
                    ClientCommand::AttackSwing { guid: OLD }
                ]
            ),
            "{sent:?}"
        );
    }

    /// A second `AttackTarget` in the chunk is the toggle's stop (`0x6131ce` → `0x5ecac0`): the
    /// first one's send stored the attack target locally (`0x5ecd51` → `0x47bf70`), so the stop's
    /// gate passes and `CMSG_ATTACKSTOP` goes out before any echo.
    #[test]
    fn a_second_attack_target_in_the_chunk_stops_the_swing_the_first_sent() {
        let mut f = frame(true);
        run(&mut f, "AttackTarget() AttackTarget()");
        assert_eq!(swings(&f), (vec![OLD], true));
    }

    /// `LOOT_OPENED` is signalled by the response's copier (`0x4c2ac0`), whose listeners run in
    /// place (`SignalEvent`, `0x703e50`), so a selecting verb a handler calls has committed by its
    /// next line, as every verb's has: `TargetUnit` then `UnitName("target")` names the new
    /// target. The window's feed is an engine turn; its handlers run as the turn ends, with the
    /// host lent.
    #[test]
    fn a_loot_opened_handlers_target_call_applies_inside_it() {
        let mut f = frame(true);
        let world = f.app.world_mut();
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(NEW, "Red Whelp".into(), None);
        as_player(world, NEW);
        run(
            &mut f,
            r#"local l = CreateFrame("Frame")
            l:RegisterEvent("LOOT_OPENED")
            l:SetScript("OnEvent", function() TargetUnit("party1") SEEN = UnitName("target") end)"#,
        );
        let world = f.app.world_mut();
        world
            .resource_mut::<crate::ui_loot::LootState>()
            .open(MOB, 1, 0, Vec::new());
        crate::ui_loot::feed_loot_once(world);
        let seen = eval::<Option<String>>(&mut f, "return SEEN");
        assert_eq!(seen.as_deref(), Some("Red Whelp"), "the new target");
        assert_eq!(selected(&f), Some(NEW));
    }

    /// The plate driver hides each live plate in turn as it returns it to the pool (`0x608a10`),
    /// so each plate's `OnHide` runs as its plate goes, with the plates after it still shown, and
    /// a call it makes commits before its verb returns: `TargetUnit` then `UnitName("target")`
    /// names the new target. The driver's turn leaves the widgets to change after it, with the
    /// host lent.
    #[test]
    fn a_plates_on_hide_runs_as_it_goes_with_the_host_lent() {
        use benilla_ui::script::{PlateGeometry, PlateState};
        let mut f = frame(true);
        let world = f.app.world_mut();
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(NEW, "Red Whelp".into(), None);
        as_player(world, NEW);
        let plate = |key: u64| PlateState {
            key,
            top_centre: (400.0, 400.0),
            health: 30.0,
            max_health: 40.0,
            bar_colour: [1.0, 0.0, 0.0],
            name: "Wolf".into(),
            level: Some(12),
            skull: false,
            level_colour: [1.0, 1.0, 0.0],
            raid_icon: None,
            alpha: 255,
            lit: false,
            hovered: false,
        };
        let geometry = PlateGeometry {
            width: 128.0,
            height: 32.0,
            bar_off_x: 4.0,
            bar_off_y: 4.0,
            bar_width: 103.0,
            bar_height: 9.0,
            level_off_x: 11.8,
            level_off_y: 9.1,
            skull_size: 12.8,
            raid_size: 25.6,
            name_height: 12.8,
            level_height: 11.0,
            shadow_offset: 1.0,
        };
        {
            let mut script = world.non_send_resource_mut::<UiScript>();
            script.run(r#"CreateFrame("Frame", "WorldFrame")"#).unwrap();
            script.sync_nameplates(geometry, &[plate(OLD), plate(NEW)]);
            script
                .run(
                    r#"SHOWN = ""
                    local function shown()
                        local n = 0
                        for _, c in ipairs({WorldFrame:GetChildren()}) do
                            if c:IsShown() then n = n + 1 end
                        end
                        return n
                    end
                    for _, c in ipairs({WorldFrame:GetChildren()}) do
                        c:SetScript("OnHide", function()
                            SHOWN = SHOWN .. shown()
                            TargetUnit("party1")
                            SEEN = UnitName("target")
                        end)
                    end"#,
                )
                .unwrap();
        }
        // Neither kind of plate on: the driver retires every live one.
        world.init_resource::<crate::player::CameraControl>();
        crate::vplates::drive_vplates_once(world);
        let (shown, seen) = eval::<(Option<String>, Option<String>)>(&mut f, "return SHOWN, SEEN");
        assert_eq!(shown.as_deref(), Some("10"), "each as its plate goes");
        assert_eq!(seen.as_deref(), Some("Red Whelp"), "the new target");
    }

    /// A probe's `/script` line is `RunScript(msg)`, a verb like any other: the line's `TargetUnit`
    /// commits through `SetSelection 0x493540` before it returns, so the line's next statement
    /// reads the new target. The drain takes the lines in an engine turn and runs each line's Lua
    /// between turns, with the host lent.
    #[test]
    fn a_probe_lines_target_call_applies_inside_it() {
        let mut f = frame(true);
        let world = f.app.world_mut();
        world
            .resource_mut::<crate::names::NameCache>()
            .insert_player(NEW, "Red Whelp".into(), None);
        as_player(world, NEW);
        benilla_world::world_point::init_world_point_resources(world);
        world.insert_resource(crate::ui_chat::commands::SlashCommands::build(
            |name| (name == "SLASH_SCRIPT1").then(|| "/script".to_string()),
            |_| None,
        ));
        world
            .non_send_resource_mut::<UiScript>()
            .push_chat_input(r#"/script TargetUnit("party1") SEEN = UnitName("target")"#.into());
        crate::ui_chat::drain_chat_input_once(world);
        let seen = eval::<Option<String>>(&mut f, "return SEEN");
        assert_eq!(seen.as_deref(), Some("Red Whelp"), "the new target");
        assert_eq!(selected(&f), Some(NEW));
    }

    /// `TargetByName` (`0x489d60`) commits through `SetSelection 0x493540` before it returns, so
    /// `UnitName("target")` on the next line of the same chunk names the new unit, the one the
    /// commit picked.
    #[test]
    fn target_by_name_names_the_new_target_in_the_same_chunk() {
        let mut f = frame(true);
        spawn_mob(&mut f, &[]);
        let world = f.app.world_mut();
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(OLD, "Old Mob".into(), None);
        names.insert_player(MOB, "Red Affinity".into(), None);
        names.insert_player(NEW, "Red Whelp".into(), None);
        as_creature(world, MOB);
        let named = eval::<String>(
            &mut f,
            "TargetByName('Red Affinity', true) return UnitName('target')",
        );
        assert_eq!(named, "Red Affinity");
        assert_eq!(
            selected(&f),
            Some(MOB),
            "the commit picks what the chunk read"
        );
    }

    /// A probe's `/target <name>` runs the stock handler's `TargetByName` (`ChatFrame.lua:748-751`,
    /// `0x489d60`), which selects inside its call, so the next line the drain takes reads the new
    /// target. Main wrote the ask for a reader later in the frame, and the next line read the old.
    #[test]
    fn a_chat_target_line_selects_before_the_next_line() {
        let mut f = frame(true);
        spawn_mob(&mut f, &[]);
        let world = f.app.world_mut();
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(OLD, "Old Mob".into(), None);
        names.insert_player(MOB, "Red Affinity".into(), None);
        as_creature(world, MOB);
        probe_line_world(world);
        world.insert_resource(crate::ui_chat::commands::SlashCommands::build(
            |name| match name {
                "SLASH_TARGET1" => Some("/target".to_string()),
                "SLASH_SCRIPT1" => Some("/script".to_string()),
                _ => None,
            },
            |_| None,
        ));
        {
            let mut script = world.non_send_resource_mut::<UiScript>();
            script.push_chat_input("/target Red Affinity".into());
            script.push_chat_input("/script NAMED = UnitName(\"target\")".into());
        }
        crate::ui_chat::drain_chat_input_once(world);
        assert_eq!(eval::<String>(&mut f, "return NAMED"), "Red Affinity");
        assert_eq!(selected(&f), Some(MOB));
    }

    /// `TargetByName` (`0x489d60`) on a miss leaves the target alone and shows message `0x127`
    /// (`ERR_UNIT_NOT_FOUND`) for a name given (`0x489dbd`, tested on the raw string's first byte,
    /// so blanks count) or `0xb8` (`ERR_GENERIC_NO_TARGET`) for an empty one (`0x489dcf`), inside
    /// the call.
    #[test]
    fn target_by_name_that_finds_nobody_shows_the_references_line() {
        for (name, line) in [
            ("Nobody", "ERR_UNIT_NOT_FOUND"),
            ("   ", "ERR_UNIT_NOT_FOUND"),
            ("", "ERR_GENERIC_NO_TARGET"),
        ] {
            let mut f = frame(true);
            run(&mut f, &format!("TargetByName(\"{name}\")"));
            assert_eq!(errors(&f), vec![line], "{name:?}");
            assert_eq!(selected(&f), Some(OLD), "{name:?}: the target stays");
            assert_eq!(f.rx.try_iter().count(), 0, "{name:?}: nothing is sent");
        }
    }

    /// Two namesakes: the ranking decides, and the chunk shows the one the commit then selects --
    /// the nearer, and of two at one distance the lower guid, whatever order either side walks.
    #[test]
    fn target_by_name_shows_the_namesake_the_commit_picks() {
        /// A namesake of `MOB` on our other side, told apart by its health.
        const TWIN: u64 = 0x30;
        for (twin_x, picked, health) in [(-4.0, MOB, 100), (-3.0, TWIN, 40)] {
            let mut f = frame(true);
            spawn_mob(&mut f, &[]);
            let world = f.app.world_mut();
            let twin = world
                .spawn((
                    Guid(TWIN),
                    Transform::from_xyz(twin_x, 0.0, 0.0),
                    GlobalTransform::from_translation(Vec3::new(twin_x, 0.0, 0.0)),
                    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                        (HEALTH, 40),
                        (28, 100),
                    ])),
                ))
                .id();
            world.resource_mut::<GuidIndex>().0.insert(TWIN, twin);
            let mut names = world.resource_mut::<crate::names::NameCache>();
            names.insert_player(MOB, "Red Affinity".into(), None);
            names.insert_player(TWIN, "Red Affinity".into(), None);
            let shown = eval::<i64>(
                &mut f,
                "TargetByName('Red Affinity') return UnitHealth('target')",
            );
            assert_eq!(shown, health, "the twin {twin_x} yd off");
            assert_eq!(selected(&f), Some(picked), "the twin {twin_x} yd off");
        }
    }

    /// The world's group as a party we lead, `member` alone in `party1`: what the loot setters'
    /// gates read at the call (`0x4e92a6`-`0x4e92eb`).
    fn lead_party(world: &mut World, member: u64, name: &str) {
        let mut group = world.resource_mut::<crate::ui_party::GroupState>();
        group.members = vec![GroupMemberEntry {
            name: name.into(),
            guid: member,
            status: 1,
            flags: 0,
        }];
        group.leader = ME;
    }

    /// The `CMSG_LOOT_METHOD`s `chunk` sends inside its calls, run with the host lent.
    fn loot_methods_sent(f: &mut Frame, chunk: &str) -> Vec<(u32, u64, u32)> {
        let _ = f.rx.try_iter().count();
        with_hosted_vm(f.app.world_mut(), |script| {
            script.expect("a VM").run(chunk).expect("the script runs")
        });
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::LootMethod {
                    method,
                    master,
                    threshold,
                } => Some((method, master, threshold)),
                _ => None,
            })
            .collect()
    }

    /// `SetLootMethod`'s master name runs the resolver in mode 2 (`0x4e9418`, filter `0x493eca`):
    /// a held groupmate resolves through the candidate feed while `CanAssist` and alive, and a
    /// dead one is no candidate, so the name finds nobody and the binding sends nothing.
    #[test]
    fn set_loot_method_resolves_a_held_groupmate_only_while_alive() {
        for (health, resolved) in [(100, true), (0, false)] {
            let mut f = frame(false);
            // OLD is a friendly player, our groupmate "Olly" in `party1`.
            with_factions(&mut f, FRIEND, &[(2, 0x19), (FLAGS, 0x8), (HEALTH, health)]);
            let world = f.app.world_mut();
            let mut names = world.resource_mut::<crate::names::NameCache>();
            names.insert_player(OLD, "Olly".into(), None);
            names.insert_player(ME, "Me".into(), None);
            lead_party(f.app.world_mut(), OLD, "Olly");
            let mut script = f.app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_unit(
                "player",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    has_object: true,
                    guid: ME,
                    ..Default::default()
                }),
            );
            script.set_unit(
                "party1",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    has_object: true,
                    guid: OLD,
                    ..Default::default()
                }),
            );
            let expected = resolved.then_some((2, OLD, 2));
            assert_eq!(
                loot_methods_sent(&mut f, r#"SetLootMethod("master", "Ol")"#),
                expected.into_iter().collect::<Vec<_>>(),
                "{health}"
            );
        }
    }

    /// Our own name resolves through the object pass at distance 0 (`0x493d30` measures from our
    /// own object, which the roster passes skip as held), and passes mode 2 while its health is
    /// above 0 (`0x493eca`): a ghost, at health 1, does; a corpse, at 0, does not.
    #[test]
    fn set_loot_method_resolves_our_own_name_through_the_feed() {
        for (health, resolved) in [(100, true), (1, true), (0, false)] {
            let mut f = frame(false);
            with_factions(&mut f, FRIEND, &[]);
            let world = f.app.world_mut();
            let me = world.resource::<GuidIndex>().0[&ME];
            world
                .entity_mut(me)
                .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (HEALTH, health),
                    (28, 100),
                    (FLAGS, 0x8),
                    (TEMPLATE, OURS),
                ])));
            world
                .resource_mut::<crate::names::NameCache>()
                .insert_player(ME, "Kelvin".into(), None);
            lead_party(f.app.world_mut(), OLD, "Olly");
            let mut script = f.app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_unit(
                "player",
                Some(benilla_ui::script::UnitState {
                    exists: true,
                    has_object: true,
                    guid: ME,
                    ..Default::default()
                }),
            );
            let expected = resolved.then_some((2, ME, 2));
            assert_eq!(
                loot_methods_sent(&mut f, r#"SetLootMethod("master", "kel")"#),
                expected.into_iter().collect::<Vec<_>>(),
                "health {health}"
            );
        }
    }

    /// The commit's `IsSelectable` (`0x4935ee`) refuses a `UNIT_FLAG_NOT_SELECTABLE` unit: the
    /// nearer namesake is the ranked pick and it is refused, so the commit keeps its selection,
    /// not the farther namesake, and the chunk reads the old target.
    #[test]
    fn target_by_name_keeps_the_old_target_when_the_commit_refuses_its_pick() {
        /// `UNIT_FLAG_NOT_SELECTABLE`, `UNIT_FIELD_FLAGS` bit 25.
        const NOT_SELECTABLE: u32 = 1 << 25;
        /// A selectable namesake of `MOB` farther off, told apart by its health.
        const TWIN: u64 = 0x30;
        let mut f = frame(true);
        spawn_mob(&mut f, &[(FLAGS, NOT_SELECTABLE)]);
        let world = f.app.world_mut();
        let twin = world
            .spawn((
                Guid(TWIN),
                Transform::from_xyz(-4.0, 0.0, 0.0),
                GlobalTransform::from_translation(Vec3::new(-4.0, 0.0, 0.0)),
                ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (HEALTH, 40),
                    (28, 100),
                ])),
            ))
            .id();
        world.resource_mut::<GuidIndex>().0.insert(TWIN, twin);
        // The old target told apart from the mob by its health.
        let old = world.resource::<GuidIndex>().0[&OLD];
        world
            .get_mut::<ObjectStore>(old)
            .expect("the old target's store")
            .0
            .merge(benilla_protocol::ObjectFields::from_pairs(&[(HEALTH, 70)]));
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(MOB, "Red Affinity".into(), None);
        names.insert_player(TWIN, "Red Affinity".into(), None);
        let shown = eval::<i64>(
            &mut f,
            "TargetByName('Red Affinity') return UnitHealth('target')",
        );
        assert_eq!(
            shown, 70,
            "the old target's health, not the twin's 40 or the mob's 100"
        );
        assert_eq!(selected(&f), Some(OLD), "the commit refuses the same pick");
    }

    /// `0x6130a3`: a unit we are friendly toward is dropped (`0x6130a8`) and `TargetNearestEnemy`
    /// runs (`0x6130b5`), moving the selection; the swing goes at what it picked (`0x6130c1`).
    /// The Attack button reaches the same `0x6131a0` through `TryCast` (`0x6e4c90`).
    #[test]
    fn attack_with_a_friend_selected_swings_at_the_nearest_enemy_instead() {
        for lua in ["AttackTarget()", "UseAction(1)"] {
            let mut f = frame(true);
            with_factions(&mut f, FRIEND, &[]);
            spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
            f.app
                .world_mut()
                .resource_mut::<crate::ui_action::ActionSlots>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: crate::ui_action::SPELL_ATTACK,
                        kind: benilla_protocol::messages::ACTION_KIND_SPELL,
                    },
                );
            run(&mut f, lua);
            assert_eq!(selected(&f), Some(MOB), "{lua}");
            assert_eq!(swings(&f), (vec![MOB], false), "{lua}");
            assert_eq!(errors(&f), Vec::<String>::new(), "{lua}");
        }
    }

    /// The retarget commits before `AttackTarget` returns, so a cast after it in the same script
    /// goes to the enemy it picked, from a friend or from nothing selected.
    #[test]
    fn a_cast_after_attack_target_goes_to_the_enemy_it_picked() {
        for selected_before in [true, false] {
            let mut f = frame(selected_before);
            with_factions(&mut f, FRIEND, &[]);
            spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
            run(&mut f, r#"AttackTarget() CastSpellByName("Heal")"#);
            assert_eq!(
                casts(&f),
                vec![(HEAL, Some(MOB))],
                "selected before: {selected_before}"
            );
        }
    }

    /// With no enemy in reach the scan moves nothing (`0x493f60`), so the selection re-read at
    /// `0x6130c1` is the friend, which the final gate's `CanAttack` refuses (`0x613171`); only an
    /// empty selection is "There is nothing to attack." (`0x6130d9`). Neither swings.
    #[test]
    fn attack_with_no_enemy_near_refuses_by_what_is_selected() {
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        run(&mut f, "AttackTarget()");
        assert_eq!(selected(&f), Some(OLD));
        assert_eq!(errors(&f), vec!["ERR_INVALID_ATTACK_TARGET"]);
        assert_eq!(swings(&f), (vec![], false));

        let mut f = frame(false);
        with_factions(&mut f, FRIEND, &[]);
        run(&mut f, "AttackTarget()");
        assert_eq!(selected(&f), None);
        assert_eq!(errors(&f), vec!["ERR_NO_ATTACK_TARGET"]);
        assert_eq!(swings(&f), (vec![], false));
    }

    /// A hostile corpse is kept (`0x6130a3` reads the reaction alone), so no scan runs past it to
    /// the live mob, and the final gate refuses a unit neither alive nor feigning (`0x613152`–
    /// `0x613165`): "You cannot attack that target.", and no swing, which vmangos would stop
    /// (`CombatHandler.cpp:53-58`).
    #[test]
    fn attack_at_a_corpse_cannot_attack_it() {
        let mut f = frame(true);
        with_factions(&mut f, FOE, &[(HEALTH, 0)]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        run(&mut f, "AttackTarget()");
        assert_eq!(selected(&f), Some(OLD));
        assert_eq!(errors(&f), vec!["ERR_INVALID_ATTACK_TARGET"]);
        assert_eq!(swings(&f), (vec![], false));
    }

    /// `0x6131a0` toggles only past the validator: attacking a live foe stops, and a press the
    /// validator refuses neither stops nor swings.
    #[test]
    fn attack_toggles_off_only_past_the_validator() {
        for (health, stopped, refused) in [
            (100, true, None),
            (0, false, Some("ERR_INVALID_ATTACK_TARGET")),
        ] {
            let mut f = frame(true);
            with_factions(&mut f, FOE, &[(HEALTH, health)]);
            let world = f.app.world_mut();
            let me = world.resource::<GuidIndex>().0[&ME];
            world
                .entity_mut(me)
                .insert(crate::creature_anim::Engaged(OLD));
            run(&mut f, "AttackTarget()");
            assert_eq!(swings(&f), (vec![], stopped), "health {health}");
            assert_eq!(
                errors(&f),
                refused.into_iter().collect::<Vec<_>>(),
                "health {health}"
            );
        }
    }

    /// `0x6131a0` stops a running attack only with `[0xb4b3e4]` set (`0x6131ce`); without control
    /// it falls to the start (`0x5ecb70`), which returns at the same flag (`0x5ecbc0`) before the
    /// stand, the swing and the sheath: the press neither stops nor swings.
    #[test]
    fn attack_without_control_neither_stops_nor_swings() {
        for engaged in [true, false] {
            let mut f = frame(true);
            with_factions(&mut f, FOE, &[(HEALTH, 100)]);
            let world = f.app.world_mut();
            world.init_resource::<crate::player::Player>();
            world.resource_mut::<crate::player::OwnControlLost>().0 = true;
            if engaged {
                let me = world.resource::<GuidIndex>().0[&ME];
                world
                    .entity_mut(me)
                    .insert(crate::creature_anim::Engaged(OLD));
            }
            run(&mut f, "AttackTarget()");
            assert_eq!(swings(&f), (vec![], false), "engaged {engaged}");
        }
    }

    /// TryCast's tail starts the swing of a cast that initiates combat through `0x6131a0`
    /// (`0x6e51b5`), whose start returns at a clear `[0xb4b3e4]`: the cast goes out, the swing
    /// does not.
    #[test]
    fn an_on_next_swing_cast_without_control_sends_no_swing() {
        const STRIKE: u32 = 78;
        for lost in [false, true] {
            let mut f = frame(true);
            with_factions(&mut f, FOE, &[(HEALTH, 100)]);
            with_book_spell(
                &mut f,
                STRIKE,
                "Strike",
                benilla_formats::SpellDisplay {
                    targets: 0x2,
                    attributes: 0x404,
                    ..Default::default()
                },
            );
            let world = f.app.world_mut();
            world.init_resource::<crate::player::Player>();
            world.resource_mut::<crate::player::OwnControlLost>().0 = lost;
            run(&mut f, r#"CastSpellByName("Strike")"#);
            let sent: Vec<_> = f.rx.try_iter().collect();
            assert!(
                sent.iter().any(
                    |c| matches!(c, ClientCommand::CastSpell { spell_id, .. } if *spell_id == STRIKE)
                ),
                "lost {lost}: the cast goes out: {sent:?}"
            );
            assert_eq!(
                sent.iter()
                    .any(|c| matches!(c, ClientCommand::AttackSwing { guid } if *guid == OLD)),
                !lost,
                "lost {lost}: {sent:?}"
            );
        }
    }

    /// A macro's lines run inside `UseAction` (`0x4e6098 call 0x4f1460`), so what they call lands
    /// before the calls the pressing script makes after it: the macro's cast goes to the old
    /// target, then the script's `TargetUnit` moves the selection.
    #[test]
    fn a_macros_calls_land_inside_the_use_action_that_ran_it() {
        let mut f = frame(true);
        {
            let world = f.app.world_mut();
            let script = world.non_send_resource::<UiScript>();
            // `ChatFrame1`'s `EXECUTE_CHAT_LINE` handler (`ChatFrame.lua:1343`) sends the line
            // through the edit box; this one runs it as Lua, which is all the order needs.
            script
                .run(
                    r#"local f = CreateFrame("Frame")
                    f:RegisterEvent("EXECUTE_CHAT_LINE")
                    f:SetScript("OnEvent", function() RunScript(arg1) end)
                    CreateMacro("heal", 1, 'CastSpellByName("Heal")', 1, nil)"#,
                )
                .expect("the macro is made");
            world
                .resource_mut::<crate::ui_action::ActionSlots>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        // The first account macro's id, one past the counter's start (`0x4eff90`).
                        action: 1,
                        kind: ACTION_KIND_MACRO,
                    },
                );
        }
        // The macro's line is a `RunScript` chunk, which casts only inside an input event: the
        // press is a key's body, and the line runs inside it in the reference.
        run_pressed(&mut f, r#"UseAction(1) TargetUnit("party1")"#);
        assert_eq!(casts(&f), vec![(HEAL, Some(OLD))]);
        assert_eq!(selected(&f), Some(NEW));
    }

    /// The macro's lines run inside the press (`0x4e6098`), so the input event that pressed it is
    /// still up for them; here the press has returned by the time the app runs them, so they run
    /// with it raised again. A press that was no input event (an addon's `OnUpdate`) hands them
    /// none, and a `/script` cast in them is refused, silently.
    #[test]
    fn a_macros_script_line_casts_only_when_its_press_was_an_input_event() {
        for (pressed, want) in [(true, vec![(HEAL, Some(OLD))]), (false, vec![])] {
            let mut f = frame(true);
            {
                let world = f.app.world_mut();
                let script = world.non_send_resource::<UiScript>();
                script
                    .run(
                        r#"local f = CreateFrame("Frame")
                        f:RegisterEvent("EXECUTE_CHAT_LINE")
                        f:SetScript("OnEvent", function() RunScript(arg1) end)
                        CreateMacro("heal", 1, 'CastSpellByName("Heal")', 1, nil)"#,
                    )
                    .expect("the macro is made");
                world
                    .resource_mut::<crate::ui_action::ActionSlots>()
                    .buttons
                    .insert(
                        0,
                        ActionButton {
                            slot: 0,
                            // The first account macro's id (`0x4eff90`).
                            action: 1,
                            kind: ACTION_KIND_MACRO,
                        },
                    );
            }
            if pressed {
                run_pressed(&mut f, "UseAction(1)");
            } else {
                run(&mut f, "UseAction(1)");
            }
            assert_eq!(casts(&f), want, "pressed in an input event: {pressed}");
        }
    }

    /// `UseAction` on a spell is `TryCastSpell`, whose gate is its first rung (`0x6e4bc1`): a macro's
    /// call outside an input event queues the press, which the app finds refused and sends
    /// nothing, where the client's own press casts. The same press on a macro slot still runs it.
    #[test]
    fn a_use_action_the_gate_refused_casts_nothing() {
        for (lua, want) in [
            ("UseAction(1)", vec![(HEAL, Some(OLD))]),
            (r#"RunScript("UseAction(1)")"#, vec![]),
        ] {
            let mut f = frame(true);
            f.app
                .world_mut()
                .resource_mut::<crate::ui_action::ActionSlots>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: HEAL,
                        kind: ACTION_KIND_SPELL,
                    },
                );
            run(&mut f, lua);
            assert_eq!(casts(&f), want, "{lua}");
        }
    }

    /// Ghost Wolf: an aura with an `ActiveIconID`, which a second press cancels.
    const GHOST_WOLF: u32 = 2645;

    /// Ghost Wolf in the catalog, the book and the form bar, and up on us as a cancelable aura.
    /// The bar admits it by `AttributesEx2 & 0x10` (`0x4b2815`), the arm whose cast toggles by its
    /// aura (`0x4b48d4`).
    fn with_ghost_wolf_up(f: &mut Frame, up: bool) {
        let world = f.app.world_mut();
        let display = benilla_formats::SpellDisplay {
            name: "Ghost Wolf".into(),
            active_icon_id: 1,
            attributes_ex2: 0x10,
            ..Default::default()
        };
        seat_book(world, GHOST_WOLF, display);
        let me = world.resource::<GuidIndex>().0[&ME];
        let mut pairs = vec![(2, 0x19), (HEALTH, 100), (28, 100), (FLAGS, 0x8)];
        if up {
            // `UNIT_FIELD_AURA` 47 and the nibble-packed `UNIT_FIELD_AURAFLAGS` 95: occupied and
            // cancelable.
            pairs.extend([(47, GHOST_WOLF), (95, 0xb)]);
        }
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(
                &pairs,
            )));
    }

    /// The `CMSG_CANCEL_AURA`s sent, as spell ids.
    fn cancels(f: &Frame) -> Vec<u32> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::CancelAura { spell_id } => Some(spell_id),
                _ => None,
            })
            .collect()
    }

    /// `CastBookSpell` (`0x4b3300`) and `CastShapeshiftForm` (`0x4b4810`) cancel an active aura or
    /// form before `TryCast` (`0x4b3466`, `0x4b496b`, `0x4b497c`), so the gate's tag 2, which is
    /// `TryCast`'s first rung (`0x6e4bc1`), refuses only the cast: a macro's press outside an input
    /// event cancels the wolf all the same, and casts nothing when it is down.
    #[test]
    fn a_refused_book_or_form_cast_still_runs_its_cancels() {
        for lua in [
            r#"RunScript('CastSpellByName("Ghost Wolf")')"#,
            r#"RunScript("CastSpell(1, BOOKTYPE_SPELL)")"#,
            r#"RunScript("CastShapeshiftForm(1)")"#,
        ] {
            let mut f = frame(true);
            with_ghost_wolf_up(&mut f, true);
            run(&mut f, lua);
            assert_eq!(cancels(&f), vec![GHOST_WOLF], "{lua}: up, cancels");

            let mut f = frame(true);
            with_ghost_wolf_up(&mut f, false);
            run(&mut f, lua);
            assert_eq!(cancels(&f), Vec::<u32>::new(), "{lua}: down");
            assert_eq!(casts(&f), vec![], "{lua}: down, refused, casts nothing");
        }
        // The client's own call casts when the wolf is down.
        let mut f = frame(true);
        with_ghost_wolf_up(&mut f, false);
        run(&mut f, "CastShapeshiftForm(1)");
        assert_eq!(
            casts(&f).iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![GHOST_WOLF]
        );
    }

    /// The stock UI over the frame's world, with an insecure addon, `Hook`, whose handler of
    /// `PLAYER_TARGET_CHANGED` casts Heal.
    fn with_stock_ui_and_a_casting_hook(f: &mut Frame) {
        with_stock_ui(f, true);
    }

    /// The stock UI over the frame's world, and with `hook` the addon above.
    fn with_stock_ui(f: &mut Frame, hook: bool) {
        let _l = crate::local_state::test_env::ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (mut script, failures) = crate::ui_script::layer_tests::production_load_with(
            if hook { "tab-hook" } else { "tab-stock" },
            false,
            "",
            |root| {
                if !hook {
                    return;
                }
                let dir = root.join("Hook");
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("Hook.toc"), "## Interface: 11200\nhook.lua\n").unwrap();
                std::fs::write(
                    dir.join("hook.lua"),
                    r#"local f = CreateFrame("Frame")
                       f:RegisterEvent("PLAYER_TARGET_CHANGED")
                       f:SetScript("OnEvent", function() CastSpellByName("Heal") end)"#,
                )
                .unwrap();
            },
        );
        assert!(failures.is_empty(), "{failures:#?}");
        script.set_spellbook(SpellBookState {
            tabs: vec![SpellTabView {
                name: "Holy".into(),
                texture: None,
                offset: 0,
                num_spells: 1,
            }],
            slots: vec![SpellSlotView {
                spell_id: HEAL,
                name: "Heal".into(),
                ..Default::default()
            }],
        });
        f.app.world_mut().insert_non_send_resource(script);
    }

    /// `SetSelection` signals `PLAYER_TARGET_CHANGED` inside `ExecuteBinding` (`0x4b7b02`-
    /// `0x4b7b11`), so an insecure addon's handler that casts passes after a TAB press: the call
    /// signals it in place, under the key's input events, as the stock `TARGETNEARESTENEMY`
    /// binding moves the target, and the addon's handler casts at the new one.
    #[test]
    fn a_tab_targets_and_the_addons_target_handler_casts() {
        benilla_formats::wow_data_or_skip!();
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_stock_ui_and_a_casting_hook(&mut f);
        f.rx.try_iter().for_each(drop);

        let pressed = with_hosted_vm(f.app.world_mut(), |s| {
            s.expect("a VM").execute_binding("TARGETNEARESTENEMY", true)
        })
        .expect("the binding runs");
        assert!(pressed);
        assert_eq!(selected(&f), Some(MOB), "TAB picked the enemy");
        assert_eq!(
            casts(&f),
            vec![(HEAL, Some(MOB))],
            "the handler's cast passed"
        );
    }

    /// A refusal a call raises is signalled inside the call, under the input events it ran under,
    /// as the reference shows it inside the press: the line's handler runs with the key's event
    /// up, so a movement verb it calls passes the gate's tag 0 (`0x494b1a`, the client's own code
    /// with an input event running), and outside a press it does not.
    #[test]
    fn a_refusal_a_call_raises_carries_the_events_it_ran_under() {
        for (pressed, moved) in [(true, true), (false, false)] {
            let mut f = frame(false);
            with_factions(&mut f, FRIEND, &[]);
            f.app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .run(
                    r#"local h = CreateFrame("Frame")
                    h:RegisterEvent("UI_ERROR_MESSAGE")
                    h:SetScript("OnEvent", function() MoveForwardStart() end)"#,
                )
                .unwrap();
            if pressed {
                run_pressed(&mut f, "AttackTarget()");
            } else {
                run(&mut f, "AttackTarget()");
            }
            assert_eq!(
                errors(&f),
                vec!["ERR_NO_ATTACK_TARGET"],
                "pressed: {pressed}"
            );
            let held = f
                .app
                .world()
                .resource::<crate::bindings::BindingsState>()
                .pressed(crate::bindings::Input::MoveForward);
            assert_eq!(held, moved, "pressed: {pressed}");
        }
    }

    /// The ladder reads the permit as an optional resource, so a world without it passes every
    /// cast; the client must register it, or the gate is silently off.
    #[test]
    fn the_client_registers_the_cast_permit() {
        let f = frame(false);
        assert!(f
            .app
            .world()
            .get_resource::<crate::spell::ScriptCastPermit>()
            .is_some());
    }

    /// A call puts back the cast permit it found as it returns, so a call nested inside another's
    /// apply, at a signal point, leaves the outer call's answer for the rest of that apply.
    #[test]
    fn a_call_puts_back_the_cast_permit_it_found() {
        let mut f = frame(true);
        f.app
            .world_mut()
            .run_system_once(|mut ladder: crate::spell::CastLadder| {
                ladder.permit(false);
            })
            .expect("the ladder runs as a one-shot system");
        run(&mut f, "ClearTarget()");
        assert!(!f
            .app
            .world()
            .resource::<crate::spell::ScriptCastPermit>()
            .allows());
    }

    /// The Attack slot is a cast too (`UseAction` → `TryCast`, `0x6e4c90`), behind the same first
    /// rung: a macro's press outside an input event swings at nothing, not even at a target
    /// already selected.
    #[test]
    fn a_use_action_on_attack_the_gate_refused_swings_at_nothing() {
        for (lua, want) in [
            ("UseAction(1)", vec![OLD]),
            (r#"RunScript("UseAction(1)")"#, vec![]),
        ] {
            let mut f = frame(true);
            f.app
                .world_mut()
                .resource_mut::<crate::ui_action::ActionSlots>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: crate::ui_action::SPELL_ATTACK,
                        kind: ACTION_KIND_SPELL,
                    },
                );
            run(&mut f, lua);
            assert_eq!(swings(&f), (want, false), "{lua}");
        }
    }

    /// The attack validator's pick of the nearest enemy asks the gate's tag 4 (`0x6130bc` →
    /// `0x493f9b`): refused, nothing is picked and the selection stands, so the friend selected is
    /// refused as the target and no swing goes out.
    #[test]
    fn an_attack_target_the_gate_refused_picks_no_enemy() {
        for (lua, selected_after, swung) in [
            ("AttackTarget()", MOB, vec![MOB]),
            (r#"RunScript("AttackTarget()")"#, OLD, vec![]),
        ] {
            let mut f = frame(true);
            with_factions(&mut f, FRIEND, &[]);
            spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
            run(&mut f, lua);
            assert_eq!(selected(&f), Some(selected_after), "{lua}");
            assert_eq!(swings(&f), (swung, false), "{lua}");
        }
    }

    /// Sinister Strike: `AttributesEx & 0x200`, one of predicate `0x6e5200`'s bits, and the enemy
    /// word (implicit target 6).
    const SINISTER_STRIKE: u32 = 1752;

    /// Sinister Strike in the catalog and on action slot 1.
    fn with_strike(f: &mut Frame) {
        let world = f.app.world_mut();
        let strike = benilla_formats::SpellDisplay {
            attributes_ex: 0x200,
            implicit_target_a1: 6,
            ..Default::default()
        };
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(SINISTER_STRIKE, strike)].into(),
            ),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        world
            .resource_mut::<crate::ui_action::ActionSlots>()
            .buttons
            .insert(
                0,
                ActionButton {
                    slot: 0,
                    action: SINISTER_STRIKE,
                    kind: ACTION_KIND_SPELL,
                },
            );
    }

    /// The selections, strikes and swings sent, in wire order.
    fn strike_wire(f: &Frame) -> Vec<(&'static str, u64)> {
        f.rx.try_iter()
            .filter_map(|c| match c {
                ClientCommand::SetSelection { guid } => Some(("select", guid)),
                ClientCommand::CastSpell {
                    spell_id: SINISTER_STRIKE,
                    target: Some(guid),
                } => Some(("cast", guid)),
                ClientCommand::AttackSwing { guid } => Some(("swing", guid)),
                _ => None,
            })
            .collect()
    }

    /// TryCast's attack pick (`0x6e4efb` → `0x612df0`): a strike pressed with no target selects the
    /// nearest enemy before its bind, so the selection goes out ahead of the cast and its swing,
    /// and the call after it reads the selection only once the cast is done.
    #[test]
    fn a_strike_with_no_target_selects_the_nearest_enemy_and_casts_at_it() {
        let mut f = frame(false);
        spawn_mob(&mut f, &[]);
        with_strike(&mut f);
        run(&mut f, r#"UseAction(1) TargetUnit("party1")"#);
        assert_eq!(
            strike_wire(&f),
            vec![
                ("select", MOB),
                ("cast", MOB),
                ("swing", MOB),
                ("select", NEW)
            ]
        );
    }

    /// A self-cast press passes the caster to the pick (`0x4e610e`), and the reaction to oneself is
    /// 4 (`0x606200`), so the pick Tabs to the enemy. With no faction table the caster would read
    /// neutral and be kept.
    #[test]
    fn a_self_cast_strike_selects_the_nearest_enemy() {
        let mut f = frame(false);
        spawn_mob(&mut f, &[]);
        with_strike(&mut f);
        run(&mut f, "UseAction(1, 0, 1)");
        assert_eq!(
            strike_wire(&f),
            vec![("select", MOB), ("cast", MOB), ("swing", MOB)]
        );
    }

    /// The pick's keep test (`0x6130a3`) drops a friend, so a strike at one retargets the nearest
    /// enemy, where the bind alone would refuse it as an invalid target.
    #[test]
    fn a_strike_at_a_friend_retargets_the_nearest_enemy() {
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_strike(&mut f);
        run(&mut f, "UseAction(1)");
        assert_eq!(
            strike_wire(&f),
            vec![("select", MOB), ("cast", MOB), ("swing", MOB)]
        );
    }

    /// `0x6130d9`: with no enemy to acquire, the strike stops at the pick with "There is nothing to
    /// attack.", not the bind's "You have no target.".
    #[test]
    fn a_strike_with_nothing_to_attack_says_so_and_sends_nothing() {
        let mut f = frame(false);
        with_strike(&mut f);
        run(&mut f, "UseAction(1)");
        assert_eq!(casts(&f), vec![]);
        assert_eq!(selected(&f), None);
        assert_eq!(
            reports(&mut f),
            vec!["ERR_NO_ATTACK_TARGET"],
            "the pick's own line, which is no cast failure"
        );
    }

    const POTION: u64 = 0x4000_0000_0000_0f07;
    const POTION_ENTRY: u32 = 929;

    /// Backpack slot 1 holds `POTION`, of a template `t` makes, and bar slot 1 names its entry;
    /// the VM's backpack shows the slot as the container feed pushes it.
    fn with_potion(f: &mut Frame, t: benilla_protocol::messages::ItemInfo) {
        let world = f.app.world_mut();
        let me = world.resource::<GuidIndex>().0[&ME];
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
                (46, 0x8),
                (532, POTION as u32),
                (533, (POTION >> 32) as u32),
            ])));
        crate::items::test_spawn_item(
            world,
            POTION,
            benilla_protocol::ObjectFields::from_pairs(&[(3, POTION_ENTRY)]),
            false,
        );
        let item_class = Some(t.class);
        world
            .resource_mut::<crate::items::Items>()
            .insert_template(POTION_ENTRY, Some(t));
        world
            .resource_mut::<crate::ui_action::ActionSlots>()
            .buttons
            .insert(
                0,
                ActionButton {
                    slot: 0,
                    action: POTION_ENTRY,
                    kind: benilla_protocol::messages::ACTION_KIND_ITEM,
                },
            );
        world.non_send_resource_mut::<UiScript>().set_container(
            0,
            Some(benilla_ui::script::ContainerState {
                num_slots: 16,
                slots: std::collections::HashMap::from([(
                    1,
                    benilla_ui::script::ContainerSlot {
                        item_id: POTION_ENTRY,
                        item_class,
                        count: 1,
                        guid: POTION,
                        // The record cached, as the feed pushes it.
                        quality: Some(1),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
        );
        world
            .non_send_resource_mut::<UiScript>()
            .seat_item_records();
        let _ = f.rx.try_iter().count();
    }

    /// `CGItem::Use` casts an item's on-use spell through `TryCastSpell`, behind the gate's tag 2
    /// (`0x4fa430` and `0x4c7af0` → `0x5d8d00` → `0x5d9258` → `0x6e5a90` → `0x6e4bc1`). A bag
    /// click, a worn item's click and a bar press queue whatever the gate said, and the cast
    /// ladder refuses the refused ones: the client's own call sends `CMSG_USE_ITEM`, a macro's
    /// outside an input event sends nothing.
    #[test]
    fn a_refused_item_use_never_reaches_the_cast() {
        const HEAD_ITEM: u64 = 0x4000_0000_0000_0f08;
        let template = || {
            let mut t = crate::items::test_template("Trinket");
            t.spells = vec![benilla_protocol::messages::ItemSpellEntry {
                index: 0,
                spell_id: 4321,
                trigger: 0,
                charges: 0,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            }];
            t.use_spell = Some(benilla_protocol::messages::ItemUseSpell {
                spell_id: 4321,
                cooldown_ms: -1,
                category: 0,
                category_cooldown_ms: -1,
            });
            t
        };
        let uses = |f: &Frame| {
            f.rx.try_iter()
                .filter(|c| matches!(c, ClientCommand::UseItem { .. }))
                .count()
        };
        for lua in [
            "UseContainerItem(0, 1)",
            "UseAction(1)",
            "UseInventoryItem(1)",
        ] {
            for (call, want) in [(lua.to_string(), 1), (format!("RunScript({lua:?})"), 0)] {
                let mut f = frame(false);
                with_potion(&mut f, template());
                // The worn item: slot 1 of the doll, the head, a copy of the same trinket.
                let world = f.app.world_mut();
                let me = world.resource::<GuidIndex>().0[&ME];
                world.entity_mut(me).insert(ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(&[
                        (2, 0x19),
                        (22, 100),
                        (28, 100),
                        (46, 0x8),
                        (486, HEAD_ITEM as u32),
                        (487, (HEAD_ITEM >> 32) as u32),
                        (532, POTION as u32),
                        (533, (POTION >> 32) as u32),
                    ]),
                ));
                crate::items::test_spawn_item(
                    world,
                    HEAD_ITEM,
                    benilla_protocol::ObjectFields::from_pairs(&[(3, POTION_ENTRY)]),
                    false,
                );
                let mut slots: benilla_ui::script::InventorySlots = Default::default();
                slots[1] = Some(benilla_ui::script::InvSlotView {
                    item_id: POTION_ENTRY,
                    guid: HEAD_ITEM,
                    count: 1,
                    use_spell: 4321,
                    ..Default::default()
                });
                world
                    .non_send_resource_mut::<UiScript>()
                    .set_inventory_slots(slots);
                let _ = f.rx.try_iter().count();
                run(&mut f, &call);
                assert_eq!(uses(&f), want, "{call}");
            }
        }
    }

    /// The bar's item use tests its copy's lock inside the call (`0x4e607b` → `0x5d8d32`): a
    /// pickup after it in the frame does not stop it, and one before it does, though the cursor
    /// let the item go again before the frame applied the press.
    #[test]
    fn a_bar_item_use_reads_the_lock_as_its_call_found_it() {
        let mut f = frame(false);
        with_potion(
            &mut f,
            benilla_protocol::messages::ItemInfo {
                start_quest: 42,
                ..crate::items::test_template("Questing Potion")
            },
        );
        run(&mut f, "UseAction(1) PickupContainerItem(0, 1)");
        assert!(
            f.rx.try_iter().count() > 0,
            "the use goes out; the pickup came after its call"
        );
        run(
            &mut f,
            "ClearCursor() PickupContainerItem(0, 1) UseAction(1) ClearCursor()",
        );
        assert_eq!(
            f.rx.try_iter().count(),
            0,
            "the copy was locked when the use was called"
        );
    }

    /// The bar's equip arm (`0x4e602a` → `0x5e1840` → `0x5e1480`) loads a projectile with
    /// `CMSG_SET_AMMO`, by its class (`0x5e16ff`, `0x5e1704`), and the arm's `ClearCursor(1,1)`
    /// unlocks it, signalling `ITEM_LOCK_CHANGED` though nothing held it (`0x5e1763`).
    #[test]
    fn a_bar_projectile_loads_by_class_and_signals_its_unlock() {
        let mut f = frame(false);
        with_potion(
            &mut f,
            benilla_protocol::messages::ItemInfo {
                class: 6,
                inventory_type: 24,
                ..crate::items::test_template("Rough Arrow")
            },
        );
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                "SIGNALS = 0 local f = CreateFrame('Frame') f:RegisterEvent('ITEM_LOCK_CHANGED') \
                 f:SetScript('OnEvent', function() SIGNALS = SIGNALS + 1 end)",
            )
            .unwrap();
        run(&mut f, "UseAction(1)");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert!(
            sent.iter().any(|c| matches!(
                c,
                ClientCommand::SetAmmo {
                    entry: POTION_ENTRY
                }
            )),
            "{sent:?}"
        );
        let script = f.app.world().non_send_resource::<UiScript>();
        assert_eq!(script.eval::<i64>("return SIGNALS").unwrap(), 1);
        assert!(!script.item_locked(POTION));
    }

    /// The bar's item use reaches `CGItem::Use` (`0x4e607b`), which refuses a locked copy first
    /// (`0x5d8d32`): nothing goes out until the lock lifts.
    #[test]
    fn a_bar_item_use_skips_a_locked_copy() {
        let mut f = frame(false);
        {
            let world = f.app.world_mut();
            let me = world.resource::<GuidIndex>().0[&ME];
            world
                .entity_mut(me)
                .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (22, 100),
                    (28, 100),
                    (46, 0x8),
                    (532, POTION as u32),
                    (533, (POTION >> 32) as u32),
                ])));
            crate::items::test_spawn_item(
                world,
                POTION,
                benilla_protocol::ObjectFields::from_pairs(&[(3, POTION_ENTRY)]),
                false,
            );
            world.resource_mut::<crate::items::Items>().insert_template(
                POTION_ENTRY,
                Some(benilla_protocol::messages::ItemInfo {
                    start_quest: 42,
                    ..crate::items::test_template("Questing Potion")
                }),
            );
            world
                .resource_mut::<crate::ui_action::ActionSlots>()
                .buttons
                .insert(
                    0,
                    ActionButton {
                        slot: 0,
                        action: POTION_ENTRY,
                        kind: benilla_protocol::messages::ACTION_KIND_ITEM,
                    },
                );
            world.non_send_resource_mut::<UiScript>().lock_item(POTION);
        }
        let _ = f.rx.try_iter().count();
        run(&mut f, "UseAction(1)");
        assert_eq!(f.rx.try_iter().count(), 0, "a locked copy is not used");
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(POTION);
        run(&mut f, "UseAction(1)");
        assert!(f.rx.try_iter().count() > 0, "unlocked, the use goes out");
    }
    const HELD: u64 = 0x4000_0000_0000_0001;
    const HELD_ENTRY: u32 = 7001;
    const HELM: u64 = 0x4000_0000_0000_0002;
    const HELM_ENTRY: u32 = 7002;

    /// Backpack slot 1 holds an ordinary item, slot 2 a helm of `bonding`, and bar slot 1 names
    /// the helm's entry; the VM's backpack shows both as the container feed pushes them.
    fn with_bar_helm(f: &mut Frame, bonding: u32) {
        let world = f.app.world_mut();
        let me = world.resource::<GuidIndex>().0[&ME];
        // Backpack slots 1 and 2 are the fields at 532 and 534.
        world
            .entity_mut(me)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
                (46, 0x8),
                (532, HELD as u32),
                (533, (HELD >> 32) as u32),
                (534, HELM as u32),
                (535, (HELM >> 32) as u32),
            ])));
        for (guid, entry) in [(HELD, HELD_ENTRY), (HELM, HELM_ENTRY)] {
            crate::items::test_spawn_item(
                world,
                guid,
                benilla_protocol::ObjectFields::from_pairs(&[(3, entry)]),
                false,
            );
        }
        let mut items = world.resource_mut::<crate::items::Items>();
        items.insert_template(HELD_ENTRY, Some(crate::items::test_template("Held Thing")));
        items.insert_template(
            HELM_ENTRY,
            Some(benilla_protocol::messages::ItemInfo {
                inventory_type: 1,
                bonding,
                ..crate::items::test_template("A Helm")
            }),
        );
        world
            .resource_mut::<crate::ui_action::ActionSlots>()
            .buttons
            .insert(
                0,
                ActionButton {
                    slot: 0,
                    action: HELM_ENTRY,
                    kind: benilla_protocol::messages::ACTION_KIND_ITEM,
                },
            );
        let slot = |guid, entry, equip: Vec<u8>, bonding| benilla_ui::script::ContainerSlot {
            guid,
            item_id: entry,
            count: 1,
            bonding,
            equip_slots: equip,
            // The record cached, as the feed pushes it.
            quality: Some(1),
            ..Default::default()
        };
        let mut slots = std::collections::HashMap::new();
        slots.insert(1, slot(HELD, HELD_ENTRY, vec![], 0));
        slots.insert(2, slot(HELM, HELM_ENTRY, vec![1], bonding));
        let mut script = world.non_send_resource_mut::<UiScript>();
        script.set_container(
            0,
            Some(benilla_ui::script::ContainerState {
                name: None,
                num_slots: 16,
                slots,
            }),
        );
        script.seat_item_records();
    }

    /// The bar's equip runs `0x5e1840` (`0x4e602a`): `SetCursorItem`'s inner clear drops and
    /// unlocks a held item (`0x494bb6`), the bar's item takes the cursor and `AutoEquipCursorItem`
    /// sends it, so a key press leaves the cursor empty and the held item free.
    #[test]
    fn a_bar_equip_drops_the_held_item_and_unlocks_it() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        // Hold the first item, then press the bar key (`checkCursor` 0).
        run(&mut f, "PickupContainerItem(0, 1) UseAction(1, 0)");

        let script = f.app.world().non_send_resource::<UiScript>();
        assert!(
            script.cursor_item().is_none(),
            "the held item was dropped and the bar's item sent off the cursor"
        );
        assert!(!script.item_locked(HELD), "the drop unlocked the held item");
        assert!(!script.item_locked(HELM), "the bar's pick takes no lock");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert!(
            sent.iter().any(|c| matches!(
                c,
                ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 24
                }
            )),
            "and the helm is equipped from backpack slot 2: {sent:?}"
        );
        // The set's pickup and the send's put-down both landed inside the call, so the pair is
        // played here, the held item's own put-down being the cursor diff's.
        let sounds: Vec<_> = f
            .app
            .world_mut()
            .resource_mut::<Messages<crate::sound::AutoEquipSound>>()
            .drain()
            .collect();
        assert_eq!(sounds.len(), 1, "one pair, for the helm");
    }

    /// A bind-on-equip helm from the bar asks first (`0x5e16c6`) and stays on the cursor
    /// (`0x5e16ce`), not locked, since `SetCursorItem` takes no lock; the held item went back.
    #[test]
    fn a_bar_equip_of_a_bind_on_equip_item_holds_it_under_the_question() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 2);
        run(&mut f, "PickupContainerItem(0, 1) UseAction(1, 0)");

        let script = f.app.world().non_send_resource::<UiScript>();
        assert_eq!(script.cursor_item().map(|i| i.guid), Some(HELM));
        assert!(!script.item_locked(HELM));
        assert!(!script.item_locked(HELD), "the held item went back");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert!(sent.is_empty(), "the question sends nothing: {sent:?}");
        assert_eq!(
            f.app
                .world()
                .resource::<crate::ui_bind_confirm::PendingEquips>()
                .live(),
            1,
            "and files its record"
        );
        assert!(
            f.app
                .world_mut()
                .resource_mut::<Messages<crate::sound::AutoEquipSound>>()
                .drain()
                .next()
                .is_none(),
            "a held item plays no pair; the cursor diff plays its pickup"
        );
    }
    /// `SetCursorItem` refuses without control of the character (`0x494b8d`, `[0xb4b3e4]` clear
    /// and no `UNIT_FLAG_TAXI_FLIGHT`) before its clear and its set: the bar's equip puts nothing
    /// on the cursor and, with it empty, `AutoEquipCursorItem` has nothing to send.
    #[test]
    fn a_bar_equip_without_control_does_nothing() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_player_control(false);
        run(&mut f, "UseAction(1, 0)");
        assert_eq!(f.rx.try_iter().count(), 0, "nothing is sent");
        let script = f.app.world().non_send_resource::<UiScript>();
        assert!(script.cursor_item().is_none());
    }

    /// The refused set does not stop `0x5e1840`: `AutoEquipCursorItem` runs on the cursor as it
    /// stood (`0x5e18c9` falls through to `0x5e18d5`), so a held item is the one sent, the bar's
    /// never touched.
    #[test]
    fn a_refused_bar_set_still_auto_equips_what_is_held() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        // Picked up with control: `PickupContainerItem` refuses without it (`0x4f9b87`).
        run(&mut f, "PickupContainerItem(0, 1)");
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_player_control(false);
        run(&mut f, "UseAction(1, 0)");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 23
                }]
            ),
            "the held item from backpack slot 1 goes, not the helm: {sent:?}"
        );
        let script = f.app.world().non_send_resource::<UiScript>();
        assert!(script.cursor_item().is_none());
    }

    /// `PickupContainerItem`'s control gate reads our taxi flag off the object as the call runs
    /// (`0x4f9b87`-`0x4f9ba3`, the pair `0x494b97`-`0x494ba6` tests): without control, the store's
    /// bit, written after every push, admits the pickup, and once cleared the next call is refused,
    /// whatever the VM's copy says.
    #[test]
    fn the_pickup_gate_reads_the_taxi_flag_off_our_object_at_the_call() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_player_control(false);
        let me = f.app.world().resource::<GuidIndex>().0[&ME];
        let flags = |f: &mut Frame, flags: u32| {
            f.app
                .world_mut()
                .get_mut::<ObjectStore>(me)
                .expect("our store")
                .0
                .merge(benilla_protocol::ObjectFields::from_pairs(&[(46, flags)]));
        };
        flags(&mut f, 0x8 | crate::player::UNIT_FLAG_TAXI_FLIGHT);
        run(&mut f, "PickupContainerItem(0, 1)");
        assert!(
            f.app
                .world()
                .non_send_resource::<UiScript>()
                .cursor_item()
                .is_some(),
            "on a taxi: picked up"
        );
        run(&mut f, "ClearCursor()");
        flags(&mut f, 0x8);
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_on_taxi(true);
        run(&mut f, "PickupContainerItem(0, 1)");
        assert!(
            f.app
                .world()
                .non_send_resource::<UiScript>()
                .cursor_item()
                .is_none(),
            "landed: refused, the VM's copy unread"
        );
    }

    /// A taxi passenger has no control but passes the gate, which reads the flag off our object at
    /// the call (`0x494b97`-`0x494ba9`): the store's bit, set after every push, passes it, and the
    /// VM's copy says nothing while the host is lent.
    #[test]
    fn a_bar_equip_on_a_taxi_passes_the_control_gate() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_player_control(false);
        let me = f.app.world().resource::<GuidIndex>().0[&ME];
        f.app
            .world_mut()
            .get_mut::<ObjectStore>(me)
            .expect("our store")
            .0
            .merge(benilla_protocol::ObjectFields::from_pairs(&[(
                46,
                0x8 | crate::player::UNIT_FLAG_TAXI_FLIGHT,
            )]));
        run(&mut f, "UseAction(1, 0)");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::AutoEquipItem {
                    bag_index: 255,
                    slot: 24
                }]
            ),
            "{sent:?}"
        );
    }

    /// `0x5e1840` refuses an item in the trade window with its line (`0x4bfd20`, `0x5e18a5`),
    /// shown inside the call (`0x496720`), and ends before the set: the bar sends nothing and the
    /// held item stays.
    #[test]
    fn a_bar_equip_of_an_item_in_the_trade_window_is_refused() {
        let mut f = frame(false);
        with_bar_helm(&mut f, 0);
        // The refusal tests the partner cell first (`0x4bfd23`): the window is open.
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .open_trade_window(0x7);
        run(
            &mut f,
            "PickupContainerItem(0, 2) ClickTradeButton(1) ClearCursor() PickupContainerItem(0, 1)",
        );
        let _ = f.rx.try_iter().count();
        run(&mut f, r#"UseAction(1, 0) SEEN = table.concat(LINES, ",")"#);

        assert_eq!(f.rx.try_iter().count(), 0, "nothing is sent");
        assert_eq!(
            eval::<String>(&mut f, "return SEEN"),
            "ERR_EQUIP_TRADE_ITEM"
        );
        let mut script = f.app.world_mut().non_send_resource_mut::<UiScript>();
        assert_eq!(script.cursor_item().map(|i| i.guid), Some(HELD));
        assert!(
            script.take_ui_errors().is_empty(),
            "nothing waits for the feed"
        );
    }

    /// The bar's item use reaches `CGItem::Use` (`0x4e607b`), whose control gate (`0x5d8eaa`) ends
    /// it with `ERR_CLIENT_LOCKED_OUT` unless the item's on-use spell is one that passes.
    #[test]
    fn a_bar_item_use_meets_the_control_gate() {
        let mut f = frame(false);
        with_potion(
            &mut f,
            benilla_protocol::messages::ItemInfo {
                use_spell: Some(benilla_protocol::messages::ItemUseSpell {
                    spell_id: 4321,
                    cooldown_ms: -1,
                    category: 0,
                    category_cooldown_ms: -1,
                }),
                ..crate::items::test_template("A Trinket")
            },
        );
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_player_control(false);
        let _ = f.rx.try_iter().count();
        run(&mut f, "UseAction(1)");
        assert_eq!(f.rx.try_iter().count(), 0, "nothing is sent");
        assert_eq!(reports(&mut f), vec!["ERR_CLIENT_LOCKED_OUT"]);

        // The spell is one the app fed as passing: the gate lets the use on to the cast.
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_control_bypass([4321].into());
        run(&mut f, "UseAction(1)");
        let shown = reports(&mut f);
        assert!(shown.is_empty(), "{shown:?}");
    }

    /// A heal's targeting cursor up: `TARGET_FLAG_UNIT` (0x2), which any unit binds, in the app's
    /// word and the VM's view of it.
    fn with_the_cursor_up(f: &mut Frame) {
        // The attempt that raised it filled the record with the player as its caster (`0x6e4fb1`).
        f.app
            .world_mut()
            .resource_mut::<crate::spell::PendingCast>()
            .fill_record();
        f.app
            .world_mut()
            .resource_mut::<crate::spell::SpellTargeting>()
            .enter(HEAL, crate::spell::CastCommit::Spell, 0x0002);
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_spell_targeting(true);
    }

    fn cursor_is_up(f: &Frame) -> bool {
        f.app
            .world()
            .resource::<crate::spell::SpellTargeting>()
            .active()
    }

    /// The `CMSG_SET_SELECTION`s sent.
    fn selections(sent: &[ClientCommand]) -> Vec<u64> {
        sent.iter()
            .filter_map(|c| match c {
                ClientCommand::SetSelection { guid } => Some(*guid),
                _ => None,
            })
            .collect()
    }

    /// The units a selecting verb can find in the frame: `MOB` (an enemy, named, party1's target
    /// and the last enemy) beside `OLD` (selected) and `NEW` (party1, "Bob"), with `OLD` the last
    /// target.
    fn with_things_to_select(f: &mut Frame) {
        // Units (`OBJECT_FIELD_TYPE` 0x9, 0x19 for the player), the typemask `0x489a40` and the
        // assist's basis lookup take (`0x489c9b`).
        spawn_mob(f, &[(2, 0x9)]);
        let world = f.app.world_mut();
        let new = world.resource::<GuidIndex>().0[&NEW];
        world
            .entity_mut(new)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 100),
                (28, 100),
                (16, MOB as u32),
            ])));
        let mut names = world.resource_mut::<crate::names::NameCache>();
        names.insert_player(MOB, "Red Affinity".into(), None);
        names.insert_player(NEW, "Bob".into(), None);
        world.resource_mut::<crate::target::LastEnemy>().0 = Some(MOB);
        world.resource_mut::<Selection>().last = Some(NEW);
    }

    /// Every selecting verb commits through `SetSelection 0x493540` before it returns: the TAB
    /// cycle's pick (`0x4941e6`), and `TargetLastTarget`, `TargetLastEnemy` and `AssistUnit`
    /// through `0x489a40` (`0x489a74`). So `UnitName("target")` on the next line of the same chunk
    /// names the unit the call selected, from the frame's candidates.
    #[test]
    fn the_next_line_reads_the_target_each_selecting_verb_committed() {
        for (lua, name) in [
            ("TargetNearestEnemy()", "Red Affinity"),
            ("TargetLastEnemy()", "Red Affinity"),
            (r#"AssistUnit("party1")"#, "Red Affinity"),
            ("TargetLastTarget()", "Bob"),
        ] {
            let mut f = frame(true);
            with_things_to_select(&mut f);
            let seen = with_hosted_vm(f.app.world_mut(), |s| {
                s.expect("a VM")
                    .eval::<Option<String>>(&format!(r#"{lua} return UnitName("target")"#))
            })
            .unwrap();
            assert_eq!(seen.as_deref(), Some(name), "{lua}");
        }
    }

    /// `SelectTarget` (`0x493540`) with the targeting cursor up binds the unit to the pending cast
    /// and returns without selecting (`0x4935ca`-`0x4935d5`), from every verb that calls it:
    /// `TargetUnit`, `AssistUnit` and `TargetLastEnemy` (`0x489a40`), `AssistByName`,
    /// `TargetLastTarget`, `TargetByName` (`0x489daa`) and `TargetNearestEnemy` (`0x4941e6`). The
    /// bind asks the gate's tag 2 (`0x6e5ba5`): a macro's line outside an input event is refused,
    /// and `SelectTarget` returns all the same, with the cursor still up and nothing selected.
    #[test]
    fn a_selecting_verb_with_the_cursor_up_casts_at_its_unit_and_selects_nothing() {
        for (lua, bound) in [
            (r#"TargetUnit("party1")"#, NEW),
            (r#"AssistUnit("party1")"#, MOB),
            ("TargetLastEnemy()", MOB),
            (r#"TargetByName("Red Affinity")"#, MOB),
            (r#"AssistByName("Bob")"#, MOB),
            ("TargetLastTarget()", NEW),
            ("TargetNearestEnemy()", MOB),
        ] {
            // Pressed: the call is the client's own inside an input event.
            let mut f = frame(true);
            with_things_to_select(&mut f);
            with_the_cursor_up(&mut f);
            run_pressed(&mut f, lua);
            let sent: Vec<_> = f.rx.try_iter().collect();
            assert!(
                matches!(sent.iter().find_map(|c| match c {
                    ClientCommand::CastSpell { spell_id, target } => Some((*spell_id, *target)),
                    _ => None,
                }), Some((HEAL, Some(g))) if g == bound),
                "{lua}: casts at {bound:#x}: {sent:?}"
            );
            assert_eq!(
                selections(&sent),
                Vec::<u64>::new(),
                "{lua}: selects nothing"
            );
            assert_eq!(selected(&f), Some(OLD), "{lua}");
            assert!(!cursor_is_up(&f), "{lua}: the bind emptied the word");

            // The same line as a macro's `/script`: tag 2 refuses the bind.
            let mut f = frame(true);
            with_things_to_select(&mut f);
            with_the_cursor_up(&mut f);
            run(&mut f, &format!("RunScript({lua:?})"));
            let sent: Vec<_> = f.rx.try_iter().collect();
            assert_eq!(casts_of(&sent), vec![], "{lua}: refused, casts nothing");
            assert_eq!(
                selections(&sent),
                Vec::<u64>::new(),
                "{lua}: and selects nothing"
            );
            assert_eq!(selected(&f), Some(OLD), "{lua}");
            assert!(cursor_is_up(&f), "{lua}: the cursor stands");

            // With the cursor down the same verb selects, and casts nothing.
            if lua != "TargetNearestEnemy()" {
                let mut f = frame(true);
                with_things_to_select(&mut f);
                run_pressed(&mut f, lua);
                let sent: Vec<_> = f.rx.try_iter().collect();
                assert_eq!(casts_of(&sent), vec![], "{lua}: cursor down");
                assert_eq!(selected(&f), Some(bound), "{lua}: cursor down");
            }
        }
    }

    /// The `CMSG_CAST_SPELL`s among `sent`.
    fn casts_of(sent: &[ClientCommand]) -> Vec<(u32, Option<u64>)> {
        sent.iter()
            .filter_map(|c| match c {
                ClientCommand::CastSpell { spell_id, target } => Some((*spell_id, *target)),
                _ => None,
            })
            .collect()
    }

    /// TAB with a heal's cursor up, through the stock `TARGETNEARESTENEMY` binding: the cycler picks
    /// the enemy, `SelectTarget` binds it (`0x4941e6` → `0x4935d5`) and the heal goes to it, the
    /// target where it was. A second TAB, the cursor down now, selects as ever.
    #[test]
    fn a_tab_with_a_heals_cursor_up_casts_at_the_tabbed_unit() {
        benilla_formats::wow_data_or_skip!();
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_stock_ui(&mut f, false);
        with_the_cursor_up(&mut f);
        f.rx.try_iter().for_each(drop);

        let tab = |f: &mut Frame| {
            let pressed = with_hosted_vm(f.app.world_mut(), |s| {
                s.expect("a VM").execute_binding("TARGETNEARESTENEMY", true)
            })
            .expect("the binding runs");
            assert!(pressed);
            f.rx.try_iter().collect::<Vec<_>>()
        };
        let sent = tab(&mut f);
        assert_eq!(casts_of(&sent), vec![(HEAL, Some(MOB))], "{sent:?}");
        assert_eq!(selections(&sent), Vec::<u64>::new());
        assert_eq!(selected(&f), Some(OLD), "TAB left the target alone");
        assert!(!cursor_is_up(&f));

        let sent = tab(&mut f);
        assert_eq!(casts_of(&sent), vec![]);
        assert_eq!(selections(&sent), vec![MOB], "the cursor down, TAB selects");
    }

    /// A charmed player's `SelectTarget` returns before anything (`0x493589`-`0x49359c`): the bind
    /// does not happen with the cursor up, nor the selection with it down.
    #[test]
    fn a_charmed_player_neither_binds_nor_selects() {
        for cursor in [true, false] {
            let mut f = frame(true);
            let me = f.app.world().resource::<GuidIndex>().0[&ME];
            f.app.world_mut().entity_mut(me).insert(ObjectStore(
                benilla_protocol::ObjectFields::from_pairs(&[
                    (2, 0x19),
                    (HEALTH, 100),
                    (28, 100),
                    // `UNIT_FIELD_CHARMEDBY`, a guid of two fields.
                    (10, 0x99),
                    (11, 0),
                ]),
            ));
            if cursor {
                with_the_cursor_up(&mut f);
            }
            run_pressed(&mut f, r#"TargetUnit("party1")"#);
            let sent: Vec<_> = f.rx.try_iter().collect();
            assert_eq!(casts_of(&sent), vec![], "cursor {cursor}");
            assert_eq!(selections(&sent), Vec::<u64>::new(), "cursor {cursor}");
            assert_eq!(selected(&f), Some(OLD), "cursor {cursor}");
            assert_eq!(cursor_is_up(&f), cursor, "cursor {cursor}");
        }
    }

    /// A deselect passes guid 0 and skips the bind (`0x4935ac`): `ClearTarget` with the cursor up
    /// clears the target and the cursor stands.
    #[test]
    fn clear_target_with_the_cursor_up_still_clears() {
        let mut f = frame(true);
        with_the_cursor_up(&mut f);
        run_pressed(&mut f, "ClearTarget()");
        assert_eq!(selected(&f), None);
        assert!(cursor_is_up(&f));
        assert_eq!(casts(&f), vec![]);
    }

    /// The attack validator selects a unit twice over: the one it keeps (`0x613100`, when the
    /// selection is not friendly) and the nearest enemy it picks otherwise (`0x4941e6`). With the
    /// cursor up each is bound to the cast and the selection stays; the attack goes on past it.
    #[test]
    fn attack_target_with_the_cursor_up_binds_the_unit_it_would_select() {
        // A hostile selection is kept (`0x6130a3`): the bind is at it, and the swing follows.
        let mut f = frame(true);
        with_factions(&mut f, FOE, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_the_cursor_up(&mut f);
        run_pressed(&mut f, "AttackTarget()");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert_eq!(casts_of(&sent), vec![(HEAL, Some(OLD))], "{sent:?}");
        assert_eq!(selections(&sent), Vec::<u64>::new());
        assert_eq!(selected(&f), Some(OLD));
        // The bind is inside the validator, ahead of the swing it validates.
        let at = |want: fn(&ClientCommand) -> bool| sent.iter().position(want);
        let (cast, swing) = (
            at(|c| matches!(c, ClientCommand::CastSpell { .. })),
            at(|c| matches!(c, ClientCommand::AttackSwing { guid: OLD })),
        );
        assert!(
            cast.is_some() && swing.is_some() && cast < swing,
            "the cast goes out before the swing: {sent:?}"
        );

        // A friendly one is dropped for the nearest enemy, which the bind takes.
        let mut f = frame(true);
        with_factions(&mut f, FRIEND, &[]);
        spawn_mob(&mut f, &[(TEMPLATE, FOE)]);
        with_the_cursor_up(&mut f);
        run_pressed(&mut f, "AttackTarget()");
        let sent: Vec<_> = f.rx.try_iter().collect();
        assert_eq!(casts_of(&sent), vec![(HEAL, Some(MOB))], "{sent:?}");
        assert_eq!(selections(&sent), Vec::<u64>::new());
        assert_eq!(selected(&f), Some(OLD), "the friend stays selected");
    }

    /// `CastBookSpell` (`0x4b3300`) asks `GetTargetingSpellId` (`0x6e48e0`) ahead of the cast and the
    /// gate: if the book spell is the one awaiting its target, the press stops targeting
    /// (`StopSpellTargeting`, `0x4b347d`) and casts nothing, a macro's refused one as well. A
    /// different spell's cursor is not its to stop: the cast goes and the cursor with it.
    #[test]
    fn a_book_cast_of_the_spell_awaiting_its_target_stops_targeting() {
        for lua in [
            r#"CastSpellByName("Heal")"#,
            "CastSpell(1, BOOKTYPE_SPELL)",
            r#"RunScript('CastSpellByName("Heal")')"#,
            r#"RunScript("CastSpell(1, BOOKTYPE_SPELL)")"#,
        ] {
            let mut f = frame(true);
            with_the_cursor_up(&mut f);
            run(&mut f, lua);
            assert!(!cursor_is_up(&f), "{lua}: the press stopped targeting");
            assert_eq!(
                reports(&mut f),
                vec!["SPELLCAST_FAILED"],
                "{lua}: as a cancel with its failed flag (`0x6e4900`), not a commit"
            );
            assert_eq!(casts(&f), vec![], "{lua}: and cast nothing");
        }

        // Heal's cursor, Ghost Wolf's press: nothing to stop on its account; it casts.
        let mut f = frame(true);
        with_ghost_wolf_up(&mut f, false);
        with_the_cursor_up(&mut f);
        run(&mut f, r#"CastSpellByName("Ghost Wolf")"#);
        assert_eq!(
            casts(&f).iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![GHOST_WOLF]
        );
    }

    // ── A cast inside the call (`0x4b4ab0` → `0x4b3300` → `0x6e5a90` → `0x6e4b60` → `0x6e5250`) ──

    /// Mark of the Wild: no `Targets`, `EffectImplicitTargetA` 21, so the arm's word is the assist
    /// bit 0x100 (spell_template 1126).
    const MOTW: u32 = 1126;
    /// Resurrection: `Targets` 0x8000, the dead friendly player's word, implicit target 0
    /// (spell_template 2006).
    const RESURRECTION: u32 = 2006;

    /// Book `name` as slot 1 over `display`.
    fn with_book_spell(
        f: &mut Frame,
        spell_id: u32,
        name: &str,
        display: benilla_formats::SpellDisplay,
    ) {
        let world = f.app.world_mut();
        let display = benilla_formats::SpellDisplay {
            name: name.into(),
            ..display
        };
        seat_book(world, spell_id, display);
    }

    /// `spell_id` known and alone in the book, its record `display`, which the book getters read
    /// through the host.
    fn seat_book(world: &mut World, spell_id: u32, display: benilla_formats::SpellDisplay) {
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays([(spell_id, display)].into()),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        world.resource_scope(|world, mut actions: Mut<crate::ui_action::PlayerActions>| {
            actions.spells.insert(spell_id);
            // The learn's form leg (`0x4b25b0`): a stance-bar spell joins the stored list.
            let catalog = &world.resource::<crate::ui_action::Spells>().catalog;
            crate::ui_shapeshift::form_learned(&mut actions.forms, spell_id, None, catalog);
        });
    }

    /// `party1` (`NEW`) a friendly player-controlled player, alive or dead.
    fn with_friendly_party1(f: &mut Frame, alive: bool) {
        let world = f.app.world_mut();
        let new = world.resource::<GuidIndex>().0[&NEW];
        world
            .entity_mut(new)
            .insert(ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (HEALTH, if alive { 100 } else { 0 }),
                (28, 100),
                (FLAGS, 0x8),
                (TEMPLATE, FRIEND),
            ])));
    }

    /// Run `lua` as a key's body with the app's host lent, as the bindings pass runs one: every
    /// call applies inside its verb, and no drain runs after the chunk.
    fn run_hosted(f: &mut Frame, lua: &str) {
        let lua = lua.to_string();
        f.app
            .world_mut()
            .run_system_once(
                move |mut script: NonSendMut<UiScript>, mut host: ScriptCallHost| {
                    script
                        .hosted(&mut host, |s| s.run_binding(&lua))
                        .expect("the chunk runs");
                },
            )
            .expect("the hosted chunk runs as a one-shot system");
    }

    /// A global the chunk left, as a number; `None` for nil.
    fn global(f: &mut Frame, name: &str) -> Option<i64> {
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<Option<i64>>(&format!("return {name}"))
            .expect("the global reads")
    }

    /// FuBar_CorkFu's buff (`BuffTemplate.lua:156-160`): the cast arms inside `CastSpellByName`, so
    /// the next line's `SpellIsTargeting()` (`0x6e6cd0` → `0x6e48a0`) is 1, `SpellTargetUnit` binds
    /// the pending spell (`0x6e5b10` → `0x6e5b40`) and sends it, and the second test reads nil, all
    /// before the chunk returns.
    #[test]
    fn a_cast_arms_inside_the_call_and_the_next_line_binds_it() {
        let mut f = frame(true);
        // An enemy selected: the arm's candidate fails the assist bit, `autoSelfCast` is off, so
        // the cursor comes up (`0x6e50c8`).
        with_factions(&mut f, FOE, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, true);
        with_book_spell(
            &mut f,
            MOTW,
            "Mark of the Wild",
            benilla_formats::SpellDisplay {
                implicit_target_a1: 21,
                ..Default::default()
            },
        );
        run_hosted(
            &mut f,
            r#"CastSpellByName("Mark of the Wild")
            FIRST = SpellIsTargeting()
            if SpellIsTargeting() then SpellTargetUnit("party1") end
            SECOND = SpellIsTargeting()
            if SpellIsTargeting() then SpellStopTargeting() end"#,
        );
        assert_eq!(global(&mut f, "FIRST"), Some(1), "armed on the next line");
        assert_eq!(
            global(&mut f, "SECOND"),
            None,
            "bound and sent by the line after"
        );
        assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
        assert!(!cursor_is_up(&f));
        assert_eq!(selected(&f), Some(OLD), "the bind leaves the selection");
    }

    /// SmartRes (`SmartRes.lua:173-177`): `SpellTargetUnit(corpse)` right after the cast binds the
    /// dead groupmate, so `not SpellIsTargeting()` tells it the resurrection went out.
    #[test]
    fn a_resurrection_binds_the_dead_unit_named_on_the_next_line() {
        let mut f = frame(false);
        with_factions(&mut f, FRIEND, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, false);
        with_book_spell(
            &mut f,
            RESURRECTION,
            "Resurrection",
            benilla_formats::SpellDisplay {
                targets: 0x8000,
                ..Default::default()
            },
        );
        run_hosted(
            &mut f,
            r#"CastSpellByName("Resurrection", 0)
            ARMED = SpellIsTargeting()
            SpellTargetUnit("party1")
            if not SpellIsTargeting() then SENT = 1 end"#,
        );
        assert_eq!(global(&mut f, "ARMED"), Some(1));
        assert_eq!(global(&mut f, "SENT"), Some(1));
        assert_eq!(casts(&f), vec![(RESURRECTION, Some(NEW))]);
    }

    /// A cast `TryCastSpell` refuses never reaches the arm (`0x6e5038`), so the next line reads no
    /// targeting: a dead caster (`0x6e4c49`, 0x13) and the gate's tag 2 outside an input event, a
    /// macro's line (`0x6e4bc1`, `0x494a92`). The client's own call, alive, arms.
    #[test]
    fn a_refused_cast_does_not_arm() {
        const CAST: &str = r#"CastSpellByName("Mark of the Wild") ARMED = SpellIsTargeting()"#;
        const MACRO_LINE: &str =
            r#"RunScript('CastSpellByName("Mark of the Wild")') ARMED = SpellIsTargeting()"#;
        for (dead, lua, armed) in [
            (false, CAST, true),
            (true, CAST, false),
            (false, MACRO_LINE, false),
        ] {
            let mut f = frame(true);
            with_factions(&mut f, FOE, &[(FLAGS, 0x8)]);
            with_book_spell(
                &mut f,
                MOTW,
                "Mark of the Wild",
                benilla_formats::SpellDisplay {
                    implicit_target_a1: 21,
                    ..Default::default()
                },
            );
            if dead {
                let world = f.app.world_mut();
                let me = world.resource::<GuidIndex>().0[&ME];
                world.entity_mut(me).insert(ObjectStore(
                    benilla_protocol::ObjectFields::from_pairs(&[
                        (2, 0x19),
                        (HEALTH, 0),
                        (28, 100),
                        (FLAGS, 0x8),
                        (TEMPLATE, OURS),
                    ]),
                ));
            }
            let chunk = lua.to_string();
            f.app
                .world_mut()
                .run_system_once(
                    move |mut script: NonSendMut<UiScript>, mut host: ScriptCallHost| {
                        // Outside any input event: the macro line's gate refuses, and the client's
                        // own chunk passes it.
                        script
                            .hosted(&mut host, |s| s.run(&chunk))
                            .expect("the chunk runs");
                    },
                )
                .unwrap();
            assert_eq!(
                global(&mut f, "ARMED"),
                armed.then_some(1),
                "{lua}, dead {dead}"
            );
            assert_eq!(cursor_is_up(&f), armed, "{lua}, dead {dead}");
            assert_eq!(casts(&f), vec![], "{lua}, dead {dead}");
            // "You are dead" for the rung, and with nothing in flight its event (`0x6e224a`); the
            // gate refuses silently.
            let want: Vec<&str> = if dead {
                vec!["SPELL_FAILED_CASTER_DEAD", "SPELLCAST_FAILED"]
            } else {
                vec![]
            };
            assert_eq!(reports(&mut f), want, "{lua}, dead {dead}");
        }
    }

    /// `CURRENT_SPELL_CAST_CHANGED` (`0x4b3250`) is signalled inside the call that armed the
    /// cursor, three times: the slot's push (`0x6e5026`), the arm (`0x6e5333`) and the attempt's
    /// end (`0x6e51e3`); and inside the one that bound it, twice: `BindTarget`'s merge
    /// (`0x6e60cb`) and the send (`0x6e589d`). The next frame's feed signals neither again.
    #[test]
    fn current_spell_cast_changed_fires_inside_the_call_and_not_again_from_the_feed() {
        let mut f = frame(true);
        with_factions(&mut f, FOE, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, true);
        with_book_spell(
            &mut f,
            MOTW,
            "Mark of the Wild",
            benilla_formats::SpellDisplay {
                implicit_target_a1: 21,
                ..Default::default()
            },
        );
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"local f = CreateFrame("Frame")
                f:RegisterEvent("CURRENT_SPELL_CAST_CHANGED")
                f:SetScript("OnEvent", function() FIRED = (FIRED or 0) + 1 end)"#,
            )
            .unwrap();
        let feed = |f: &mut Frame| {
            let world = f.app.world_mut();
            world
                .run_system_once(crate::spell::targeting::feed_targeting_to_input)
                .unwrap();
            world
                .run_system_once(crate::ui_cast::signal_reports)
                .unwrap();
        };
        run_hosted(
            &mut f,
            r#"CastSpellByName("Mark of the Wild") SEEN = FIRED"#,
        );
        assert_eq!(global(&mut f, "SEEN"), Some(3), "inside the arming call");
        feed(&mut f);
        assert_eq!(global(&mut f, "FIRED"), Some(3), "not again from the feed");
        run_hosted(&mut f, r#"SpellTargetUnit("party1") SEEN = FIRED"#);
        assert_eq!(global(&mut f, "SEEN"), Some(5), "inside the binding call");
        feed(&mut f);
        assert_eq!(global(&mut f, "FIRED"), Some(5), "not again from the feed");
        assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
    }

    /// `START_AUTOREPEAT_SPELL` fires inside the send that stores the key (`0x6e594c`,
    /// `0x6e5952`) and `STOP_AUTOREPEAT_SPELL` inside the stop that clears it (`0x6ea16a`,
    /// `0x6ea170`): a macro's `SpellStopCasting()` then `CastSpellByName("Shoot")` fires both, in
    /// that order, before it returns, and the next feed fires neither again.
    #[test]
    fn the_auto_repeat_edges_fire_inside_the_calls_that_make_them() {
        const SHOOT: u32 = 5019;
        let mut f = frame(true);
        with_book_spell(
            &mut f,
            SHOOT,
            "Shoot",
            benilla_formats::SpellDisplay {
                targets: 0x2,
                attributes_ex2: 0x20,
                ..Default::default()
            },
        );
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"SEEN = {}
                local f = CreateFrame("Frame")
                f:RegisterEvent("START_AUTOREPEAT_SPELL")
                f:RegisterEvent("STOP_AUTOREPEAT_SPELL")
                f:SetScript("OnEvent", function() table.insert(SEEN, event) end)"#,
            )
            .unwrap();
        let seen = |f: &mut Frame| {
            f.app
                .world_mut()
                .non_send_resource_mut::<UiScript>()
                .eval::<Vec<String>>("return SEEN")
                .unwrap()
        };
        run_hosted(&mut f, r#"CastSpellByName("Shoot") N = table.getn(SEEN)"#);
        assert_eq!(
            global(&mut f, "N"),
            Some(1),
            "inside the send: {:?}",
            seen(&mut f)
        );
        run_hosted(
            &mut f,
            r#"SpellStopCasting() CastSpellByName("Shoot") N = table.getn(SEEN)"#,
        );
        assert_eq!(global(&mut f, "N"), Some(3), "inside the macro");
        assert_eq!(
            seen(&mut f),
            [
                "START_AUTOREPEAT_SPELL",
                "STOP_AUTOREPEAT_SPELL",
                "START_AUTOREPEAT_SPELL"
            ]
        );
        f.app
            .world_mut()
            .run_system_once(crate::ui_cast::signal_reports)
            .unwrap();
        assert_eq!(seen(&mut f).len(), 3, "not again from the feed");
    }

    /// A key's body runs inside `CBindings::ExecuteBinding` (`0x4b7990`) with its input event up
    /// (`0x4b7b02`), and the bindings pass lends the host around it: SmartRes on a key casts, binds
    /// and reads the result inside the body, with no drain after it.
    #[test]
    fn a_bound_keys_body_casts_and_binds_inside_the_bindings_pass() {
        let mut f = frame(false);
        with_factions(&mut f, FRIEND, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, false);
        with_book_spell(
            &mut f,
            RESURRECTION,
            "Resurrection",
            benilla_formats::SpellDisplay {
                targets: 0x8000,
                ..Default::default()
            },
        );
        let world = f.app.world_mut();
        {
            let mut script = world.non_send_resource_mut::<UiScript>();
            script.set_default_bindings(benilla_ui::script::keybind::parse_bindings_wtf(
                "bind F SMARTRES\r\n",
            ));
            script.load_binding_set(1);
            script.register_bindings(
                &benilla_ui::bindings_xml::parse(
                    r#"<Bindings><Binding name="SMARTRES">
                        CastSpellByName("Resurrection", 0)
                        ARMED = SpellIsTargeting()
                        SpellTargetUnit("party1")
                        if not SpellIsTargeting() then SENT = 1 end
                    </Binding></Bindings>"#,
                )
                .expect("well-formed"),
            );
        }
        world.write_message(bevy::input::keyboard::KeyboardInput {
            key_code: KeyCode::KeyF,
            logical_key: bevy::input::keyboard::Key::Character("f".into()),
            state: bevy::input::ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        crate::bindings::dispatch_once(world);
        assert_eq!(global(&mut f, "ARMED"), Some(1));
        assert_eq!(global(&mut f, "SENT"), Some(1));
        assert_eq!(casts(&f), vec![(RESURRECTION, Some(NEW))]);
    }

    /// FuBar_CorkFu casts from its plugin's click: a button's `OnClick` run by the mouse raises the
    /// click event (`0x779275`), and the input pass lends the host around it, so the cast arms
    /// and the next line binds it inside the handler.
    #[test]
    fn a_clicks_handler_casts_and_binds_inside_the_input_pass() {
        let mut f = frame(true);
        with_factions(&mut f, FOE, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, true);
        with_book_spell(
            &mut f,
            MOTW,
            "Mark of the Wild",
            benilla_formats::SpellDisplay {
                implicit_target_a1: 21,
                ..Default::default()
            },
        );
        let world = f.app.world_mut();
        {
            let mut script = world.non_send_resource_mut::<UiScript>();
            script.set_screen_size(1024.0, 768.0);
            script
                .run(
                    r#"local b = CreateFrame("Button", "CorkButton")
                    b:SetPoint("BOTTOMLEFT", 0, 0) b:SetWidth(1024) b:SetHeight(768)
                    b:SetScript("OnClick", function()
                        CastSpellByName("Mark of the Wild")
                        FIRST = SpellIsTargeting()
                        if SpellIsTargeting() then SpellTargetUnit("party1") end
                        SECOND = SpellIsTargeting()
                    end)"#,
                )
                .expect("the button builds");
            script.resolve();
        }
        let mut window = Window {
            resolution: UVec2::new(1024, 768).into(),
            ..default()
        };
        window.set_cursor_position(Some(Vec2::new(100.0, 100.0)));
        world.spawn((window, bevy::window::PrimaryWindow));
        for down in [true, false] {
            let mut buttons = world.resource_mut::<ButtonInput<MouseButton>>();
            buttons.clear();
            if down {
                buttons.press(MouseButton::Left);
            } else {
                buttons.release(MouseButton::Left);
            }
            crate::ui_script::feed_input_once(world);
        }
        assert_eq!(global(&mut f, "FIRST"), Some(1), "armed on the next line");
        assert_eq!(global(&mut f, "SECOND"), None, "bound and sent");
        assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
    }

    /// CorkFu's lines, `CastSpellByName` then `SpellIsTargeting` and `SpellTargetUnit`, with the
    /// world set up for the arm: an enemy selected, `party1` a friend, the spell a friendly
    /// single-target buff.
    fn cork_frame() -> Frame {
        let mut f = frame(true);
        with_factions(&mut f, FOE, &[(FLAGS, 0x8)]);
        with_friendly_party1(&mut f, true);
        with_book_spell(
            &mut f,
            MOTW,
            "Mark of the Wild",
            benilla_formats::SpellDisplay {
                implicit_target_a1: 21,
                ..Default::default()
            },
        );
        f
    }

    /// The body every entry below runs once, with no input event up: the client's own code, which
    /// the gate's tag 2 passes (`0x494a5a`).
    const CORK: &str = r#"RAN = 1
        CastSpellByName("Mark of the Wild")
        FIRST = SpellIsTargeting()
        if SpellIsTargeting() then SpellTargetUnit("party1") end
        SECOND = SpellIsTargeting()"#;

    /// An `OnUpdate` script's cast arms inside its call: the update walk of the pump (`0x765650`)
    /// runs the script (`0x704f10`) with nothing between a verb and its effect, so the next line
    /// reads the armed word and binds it, as a key's body does.
    #[test]
    fn an_on_update_scripts_cast_arms_and_binds_inside_the_tick() {
        let mut f = cork_frame();
        let world = f.app.world_mut();
        world
            .non_send_resource_mut::<UiScript>()
            .run(&format!(
                r#"local u = CreateFrame("Frame")
                u:SetScript("OnUpdate", function()
                    this:SetScript("OnUpdate", nil)
                    {CORK}
                end)"#
            ))
            .expect("the frame builds");
        world.spawn((Window::default(), bevy::window::PrimaryWindow));
        crate::ui_script::tick_once(world);
        assert_eq!(global(&mut f, "RAN"), Some(1), "the script ran");
        assert_eq!(global(&mut f, "FIRST"), Some(1), "armed on the next line");
        assert_eq!(global(&mut f, "SECOND"), None, "bound and sent");
        assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
    }

    /// An event handler's cast arms inside its call: `SignalEvent` (`0x703e50`) runs the handler in
    /// place (`0x704d50`), here the chat feed's `CHAT_MSG_SYSTEM`, so its next line reads the armed
    /// word and binds it.
    #[test]
    fn an_event_handlers_cast_arms_and_binds_inside_the_feed() {
        let mut f = cork_frame();
        let world = f.app.world_mut();
        world
            .non_send_resource_mut::<UiScript>()
            .run(&format!(
                r#"local e = CreateFrame("Frame")
                e:RegisterEvent("CHAT_MSG_SYSTEM")
                e:SetScript("OnEvent", function()
                    this:UnregisterEvent("CHAT_MSG_SYSTEM")
                    {CORK}
                end)"#
            ))
            .expect("the frame builds");
        world.resource_mut::<crate::ui_chat::ChatLog>().push_event(
            crate::ui_chat::ChatEvent::text_only(
                crate::ui_chat::ChatEventKind::System,
                "Welcome".into(),
            ),
        );
        crate::ui_chat::feed_chat_once(world);
        assert_eq!(global(&mut f, "RAN"), Some(1), "the handler ran");
        assert_eq!(global(&mut f, "FIRST"), Some(1), "armed on the next line");
        assert_eq!(global(&mut f, "SECOND"), None, "bound and sent");
        assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
    }

    /// A cork client whose frame runs [`CORK`] once, from its handler for `event`.
    fn cork_on(event: &str) -> Frame {
        let mut f = cork_frame();
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(&format!(
                r#"local e = CreateFrame("Frame")
                e:RegisterEvent("{event}")
                e:SetScript("OnEvent", function()
                    this:UnregisterEvent("{event}")
                    {CORK}
                end)"#
            ))
            .expect("the frame builds");
        f
    }

    /// A lane's event handler casts inside its call: the cooldown lane signals each flush with
    /// the host lent, each handler run in place (`SignalEvent` `0x703e50`). Here the cheat's
    /// `ACTIONBAR_UPDATE_COOLDOWN` (`0x6e9712`).
    #[test]
    fn a_cooldown_flushs_handler_casts_and_binds_inside_its_call() {
        let mut f = cork_on("ACTIONBAR_UPDATE_COOLDOWN");
        f.app
            .world_mut()
            .resource_mut::<crate::spell::Cooldowns>()
            .wipe();
        crate::lanes::fire_in_order(f.app.world_mut());
        assert_eq!(global(&mut f, "RAN"), Some(1), "the handler ran");
        assert_eq!(global(&mut f, "FIRST"), Some(1), "armed on the next line");
        assert_eq!(global(&mut f, "SECOND"), None, "bound and sent");
        assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
    }

    /// A latch core's event handler casts inside its call: the bank's close core (`0x4f84d0`)
    /// signals `BANKFRAME_CLOSED` (`0x4f84df`), with no test of the cell, from an exclusive entry
    /// that lends the host around the fire.
    #[test]
    fn a_latch_cores_event_handler_casts_and_binds_inside_its_call() {
        let mut f = cork_on("BANKFRAME_CLOSED");
        crate::ui_bank::close_core(f.app.world_mut());
        assert_eq!(global(&mut f, "RAN"), Some(1), "the handler ran");
        assert_eq!(global(&mut f, "FIRST"), Some(1), "armed on the next line");
        assert_eq!(global(&mut f, "SECOND"), None, "bound and sent");
        assert_eq!(casts(&f), vec![(MOTW, Some(NEW))]);
    }

    /// The chat drain's `UNIT_COMBAT` lists its unit's tokens as it signals (`SignalUnitCombat`
    /// `0x494600`): a line's handler earlier in the same drain clears the target inside its call,
    /// so the flash after it on the old target fires no `"target"`.
    #[test]
    fn the_chat_drains_unit_combat_names_the_target_as_it_signals() {
        let mut f = frame(true);
        // The VM shows the selection the frame's feed would have pushed.
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .set_unit_guids(&benilla_ui::script::UnitGuids {
                player: ME,
                target: OLD,
                ..Default::default()
            });
        f.app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(
                r#"ORDER = {}
                local e = CreateFrame("Frame")
                e:RegisterEvent("UNIT_COMBAT")
                e:RegisterEvent("CHAT_MSG_SYSTEM")
                e:SetScript("OnEvent", function()
                    if event == "UNIT_COMBAT" then
                        table.insert(ORDER, arg1 .. " " .. arg2)
                    else
                        table.insert(ORDER, event)
                        ClearTarget()
                    end
                end)"#,
            )
            .expect("the frame builds");
        let flash = |action| crate::ui_unit::UnitCombatFeedback {
            guid: OLD,
            action,
            flags: "",
            amount: 0,
            school: 0,
        };
        {
            let log = f.app.world().resource::<crate::ui_chat::ChatLog>();
            log.push_unit_combat(flash("DODGE"));
            log.push_event(crate::ui_chat::ChatEvent::text_only(
                crate::ui_chat::ChatEventKind::System,
                "a line".into(),
            ));
            log.push_unit_combat(flash("PARRY"));
        }
        crate::ui_chat::feed_chat_once(f.app.world_mut());
        assert_eq!(
            selected(&f),
            None,
            "the handler's ClearTarget applied in its call"
        );
        let order = f
            .app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval::<Vec<String>>("return ORDER")
            .expect("the order reads");
        assert_eq!(order, ["target DODGE", "CHAT_MSG_SYSTEM"]);
    }

    /// The systems of `app` that write the VM, and those of them whose declared access does not
    /// cover the call host's, by schedule and name. An exclusive system declares no access and is
    /// left out.
    fn vm_writers_and_the_unhosted(app: &mut App) -> (Vec<String>, Vec<String>) {
        use bevy::ecs::system::{IntoSystem, System};
        let world = app.world_mut();
        let host = System::initialize(&mut IntoSystem::into_system(|_: ScriptCallHost| {}), world);
        let host = host.combined_access();
        // Each system's access with the host's summed in, into one buffer reused across them.
        let mut with_host = bevy::ecs::query::Access::new();
        let labels: Vec<_> = world
            .resource::<bevy::ecs::schedule::Schedules>()
            .iter()
            .map(|(_, s)| s.label())
            .collect();
        let (mut writers, mut unhosted) = (Vec::new(), Vec::new());
        for label in labels {
            world.schedule_scope(label, |world, schedule| {
                schedule.graph_mut().systems.initialize(world);
                let Some(vm) = world
                    .components()
                    .get_resource_id(std::any::TypeId::of::<UiScript>())
                else {
                    return;
                };
                let graph = schedule.graph();
                for (key, system, _) in graph.systems.iter() {
                    let Some(declared) = graph.systems.get(key) else {
                        continue;
                    };
                    let access = declared.access.combined_access();
                    if system.is_exclusive() || !access.has_resource_write(vm) {
                        continue;
                    }
                    writers.push(system.name().to_string());
                    with_host.clone_from(access);
                    with_host.extend(host);
                    if with_host != *access {
                        unhosted.push(format!("{label:?}: {}", system.name()));
                    }
                }
            });
        }
        unhosted.sort();
        (writers, unhosted)
    }

    /// Every system that writes the VM holds its call host, so a call its Lua makes applies inside
    /// the verb whatever entry the script came in by, as `TargetByName` (`0x489d60`) commits
    /// through `SetSelection` (`0x493540`) before it returns. Read off each system's declared
    /// access in every schedule of the client with every instrument and probe added, where a
    /// system whose own parameters collide with the host's also fails to initialize. An exclusive
    /// entry lends through [`with_hosted_vm`].
    #[test]
    fn every_system_that_writes_the_vm_holds_its_call_host() {
        // The census tells a bare writer from a hosted one.
        let mut probe = App::new();
        probe.add_systems(
            Update,
            (
                |_: Option<NonSendMut<UiScript>>| {},
                |_: HostedVm| {},
                |_: VmWith<Res<'static, Time>>| {},
            ),
        );
        let (writers, unhosted) = vm_writers_and_the_unhosted(&mut probe);
        assert_eq!((writers.len(), unhosted.len()), (3, 1), "{unhosted:?}");

        let mut app = crate::game_plugins::schedule_tests::headless_client_with(
            crate::dev::add_every_instrument,
        );
        let (writers, unhosted) = vm_writers_and_the_unhosted(&mut app);
        // The census reads the client's systems: eighty writers at least in either build, a floor
        // the feeds' going live lowers toward, and where the build carries type names, a feed and
        // two frame polls among them.
        assert!(
            writers.len() >= 80,
            "the census reads the client's VM writers: {} read",
            writers.len()
        );
        if writers.iter().any(|w| w.contains("benilla_app::")) {
            for name in ["feed_framerate", "poll_corpse", "poll_area_spirit_healer"] {
                assert!(
                    writers.iter().any(|w| w.contains(name)),
                    "the census reads the client's VM writers, `{name}` among them"
                );
            }
        }
        assert!(
            unhosted.is_empty(),
            "these systems write the VM without holding its call host (`HostedVm` or `VmWith`), \
             so a call their Lua makes is refused:\n  {}",
            unhosted.join("\n  ")
        );
    }
}
