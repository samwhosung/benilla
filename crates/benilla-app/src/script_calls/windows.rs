//! The NPC windows' close cores inside the calls that enter them: a close verb, and a verb whose
//! core closes the window on the way (`OpenTrainer`, the quest teardowns). Each runs as the
//! reference's core does: its latch release (`0x4931c0`), which re-enters the same core while the
//! window holds the latch (`0x493310`), its cell's clear, and its signal, whose listeners run
//! where it is raised, before the core goes on. The other windows' cells hold no latched guid
//! ([`crate::ui_session::LatchSteps::release`]), so the release re-enters only its own core.

use benilla_ui::script::UiScript;

use super::ScriptCallHost;
use crate::ui_cast::ReportMark;
use crate::ui_session::NpcWindow;

impl ScriptCallHost<'_, '_> {
    /// A core's signal point (`SignalEvent`, `0x703e50`): the listeners of what it fired run
    /// here, before the core goes on.
    fn signalled(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        self.at_point(script, mark);
    }

    /// `0x4931c0(cell, dl = 0)` from a core, and the core's re-entry inside it while its window
    /// holds the latch.
    fn release_into(
        &mut self,
        script: &mut UiScript,
        mark: &mut ReportMark,
        cell: u64,
        core: fn(&mut Self, &mut UiScript, &mut ReportMark),
    ) {
        let reenters = self.appliers.latch().release(cell);
        if reenters {
            core(self, script, mark);
        }
    }

    /// The bank's close core `0x4f84d0` with a zero guid: `BANKFRAME_CLOSED` first and with no
    /// test of the cell (`0x4f84df`), the release on the cell as the listeners left it
    /// (`0x4f84eb`), then the cell cleared (`0x4f84f2`).
    pub(super) fn close_bank(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        script.fire_event("BANKFRAME_CLOSED", vec![]);
        self.signalled(script, mark);
        let banker = self.appliers.bank().banker();
        if let Some(banker) = banker {
            self.release_into(script, mark, banker, Self::close_bank);
        }
        let cleared = self.appliers.bank().clear_banker(script);
        if cleared {
            self.appliers.latch().note_closed(NpcWindow::Bank);
        }
    }

    /// The trainer's close core `0x4d7440` with a zero guid: nothing with no trainer
    /// (`0x4d7459`), else `TRAINER_CLOSED` first (`0x4d7468`), the release on the cell as the
    /// listeners left it (`0x4d7474`), then the cell cleared (`0x4d747c`).
    pub(super) fn close_trainer(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        if self.appliers.trainer().trainer().is_none() {
            return;
        }
        script.fire_event("TRAINER_CLOSED", vec![]);
        self.signalled(script, mark);
        let trainer = self.appliers.trainer().trainer();
        if let Some(trainer) = trainer {
            self.release_into(script, mark, trainer, Self::close_trainer);
        }
        let cleared = self.appliers.trainer().clear_trainer(script);
        if cleared {
            self.appliers.latch().note_closed(NpcWindow::Trainer);
        }
    }

    /// `OpenTrainer()` (`0x4d8cf0`): with the active player (`0x4d8d24`), `TalkToTrainer` on its
    /// own guid (`0x5df680`), which closes the window (`0x5df6bb`) before `CMSG_TRAINER_LIST`
    /// (`0x5df6fc`).
    pub(super) fn open_own_trainer(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(me) = self.appliers.trainer().own_guid() else {
            return;
        };
        self.close_trainer(script, mark);
        self.appliers.trainer().ask_own_list(me);
    }

    /// The auction house's close core `0x4cd5f0`: nothing with no auctioneer (`0x4cd5fb`), else
    /// `AUCTION_HOUSE_CLOSED` first (`0x4cd602`), the release on the cell as the listeners left
    /// it (`0x4cd60e`), the auctioneer, house and flags cleared with the lists left standing
    /// (`0x4cd613`-`0x4cd629`), then the sell slot emptied with its unlock (`0x4cd62e` →
    /// `0x4cd640`).
    pub(super) fn close_auction(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        if self.appliers.auction().auctioneer().is_none() {
            return;
        }
        script.fire_event("AUCTION_HOUSE_CLOSED", vec![]);
        self.signalled(script, mark);
        let auctioneer = self.appliers.auction().auctioneer();
        if let Some(auctioneer) = auctioneer {
            self.release_into(script, mark, auctioneer, Self::close_auction);
        }
        if self.appliers.auction().auctioneer().is_some() {
            self.appliers.auction().clear_auctioneer(script);
            self.appliers.latch().note_closed(NpcWindow::Auction);
        }
        script.clear_auction_sell_item();
        self.signalled(script, mark);
    }

    /// The gossip menu's close core `0x4e1fa0` with a zero guid: nothing with no menu
    /// (`0x4e1fb8`), else the release (`0x4e1fc5`), the cell zeroed (`0x4e1fcc`) and
    /// `GOSSIP_CLOSED` (`0x4e1fdc`); the slots, the text and its latch stay.
    pub(super) fn close_gossip(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(npc) = self.appliers.gossip().npc() else {
            return;
        };
        self.release_into(script, mark, npc, Self::close_gossip);
        self.appliers.gossip().clear_npc();
        self.appliers.latch().note_closed(NpcWindow::Gossip);
        script.fire_event("GOSSIP_CLOSED", vec![]);
        self.signalled(script, mark);
    }

    /// The mailbox's close core, `0x4acd50` with `cl = 0`: the compose tab's silent reset first
    /// and always (`0x4acd55` → `0x4acdc0(0)`), then, with a mailbox open, the release
    /// (`0x4acd85`), the cell cleared (`0x4acd8f`), `MAIL_CLOSED` (`0x4acda3`) and the tail's
    /// next-mail-time query (`0x4acda8`, `0x4acdb1`).
    pub(super) fn close_mail(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        script.reset_compose_tab_silently();
        let Some(mailbox) = self.appliers.mail().mailbox() else {
            return;
        };
        self.release_into(script, mark, mailbox, Self::close_mail);
        self.appliers.mail().clear_mailbox(script);
        self.appliers.latch().note_closed(NpcWindow::Mail);
        script.fire_event("MAIL_CLOSED", vec![]);
        self.signalled(script, mark);
        self.appliers.mail().refresh_after_close();
    }

    /// The vendor's close core `0x4fadf0`: with a vendor open, the release (`0x4fae04`), the cell
    /// cleared (`0x4fae0e`), `MERCHANT_CLOSED` (`0x4fae2c`), then a vendor row held on the cursor
    /// dropped (`0x4fae31`-`0x4fae4a`) and a Repair base mode back to Point (`0x4fae4f`-`0x4fae68`).
    pub(super) fn close_merchant(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(vendor) = self.appliers.merchant().vendor() else {
            return;
        };
        self.release_into(script, mark, vendor, Self::close_merchant);
        self.appliers.merchant().clear_vendor(script);
        self.appliers.latch().note_closed(NpcWindow::Merchant);
        script.fire_event("MERCHANT_CLOSED", vec![]);
        self.signalled(script, mark);
        crate::ui_merchant::close_tail(script);
        self.signalled(script, mark);
    }

    /// The rest of the quest panel's teardown `0x501130` after its cell's zero (`0x501201`),
    /// which [`crate::ui_quest::QuestVerbs`] ran: the release (`0x50120d`), which finds no window
    /// holding the giver, then `QUEST_FINISHED` (`0x501217`), once.
    pub(super) fn finish_quest_teardown(
        &mut self,
        script: &mut UiScript,
        mark: &mut ReportMark,
        giver: u64,
    ) {
        self.appliers.latch().note_closed(NpcWindow::Quest);
        script.set_quest(None);
        let _ = self.appliers.latch().release(giver);
        script.fire_event("QUEST_FINISHED", vec![]);
        self.signalled(script, mark);
    }

    /// The stable's close core `0x4cae10`: with a master stored, the release (`0x4cae24`), the
    /// cell cleared (`0x4cae29`), then `PET_STABLE_CLOSED` (`0x4cae3a`).
    pub(super) fn close_stable(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(master) = self.appliers.stable().master() else {
            return;
        };
        self.release_into(script, mark, master, Self::close_stable);
        self.appliers.stable().clear_master(script);
        self.appliers.latch().note_closed(NpcWindow::Stable);
        script.fire_event("PET_STABLE_CLOSED", vec![]);
        self.signalled(script, mark);
    }

    /// The trade window's close core, `0x4bf4e0(0, 0)`, after the item watchers' trade halves
    /// still queued: with a window open, the done flag stored (`0x4bf4fd`), the release
    /// (`0x4bf50a`), the cell cleared and `TRADE_CLOSED` (`0x4bf522`); then `CMSG_CANCEL_TRADE`,
    /// open or not (`0x4bf527`).
    pub(super) fn close_trade(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        if self.appliers.trade().run_trade_touches(script) {
            self.signalled(script, mark);
        }
        let partner = self.appliers.trade().partner_cell();
        if let Some(partner) = partner {
            script.set_trade_done(false);
            self.release_into(script, mark, partner, Self::close_trade);
            self.appliers.trade().close_window(script);
            self.appliers.latch().note_closed(NpcWindow::Trade);
            script.fire_event("TRADE_CLOSED", vec![]);
            self.signalled(script, mark);
        }
        self.appliers.trade().send_cancel();
    }

    /// The reader's close core `0x4e32e0(0, 0)`, `CloseItemText()` itself (`0x4e3b70`): with an
    /// object read, the release on the cell (`0x4e3357`), the cell cleared (`0x4e3364`) and
    /// `ITEM_TEXT_CLOSED` (`0x4e33a2`).
    pub(super) fn close_item_text(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(object) = self.appliers.item_text().object() else {
            return;
        };
        self.release_into(script, mark, object, Self::close_item_text);
        self.appliers.item_text().clear();
        self.appliers.latch().note_closed(NpcWindow::ItemText);
        script.set_item_text(None);
        script.fire_event("ITEM_TEXT_CLOSED", vec![]);
        self.signalled(script, mark);
    }

    /// The taxi map's close core `0x4dc0f0`: with a map open, the release (`0x4dc105`), the cell
    /// cleared (`0x4dc113`) and each slot's route freed (`0x4dc130`-`0x4dc145`), then
    /// `TAXIMAP_CLOSED` (`0x4dc15f`).
    pub(super) fn close_taxi(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(flightmaster) = self.appliers.taxi().flightmaster() else {
            return;
        };
        self.release_into(script, mark, flightmaster, Self::close_taxi);
        self.appliers.taxi().clear_map(script);
        self.appliers.latch().note_closed(NpcWindow::Taxi);
        script.fire_event("TAXIMAP_CLOSED", vec![]);
        self.signalled(script, mark);
    }
}
