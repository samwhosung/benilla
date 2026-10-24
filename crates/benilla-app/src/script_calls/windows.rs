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

/// The interaction latch's engine half inside the host, for one statement.
macro_rules! latch {
    ($host:expr) => {
        $host.appliers.p2().p5().p2()
    };
}

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
        let reenters = latch!(self).release(cell);
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
        let banker = self.appliers.p2().p6().p0().banker();
        if let Some(banker) = banker {
            self.release_into(script, mark, banker, Self::close_bank);
        }
        let cleared = self.appliers.p2().p6().p0().clear_banker(script);
        if cleared {
            latch!(self).note_closed(NpcWindow::Bank);
        }
    }

    /// The trainer's close core `0x4d7440` with a zero guid: nothing with no trainer
    /// (`0x4d7459`), else `TRAINER_CLOSED` first (`0x4d7468`), the release on the cell as the
    /// listeners left it (`0x4d7474`), then the cell cleared (`0x4d747c`).
    pub(super) fn close_trainer(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        if self.appliers.p5().p5().trainer().is_none() {
            return;
        }
        script.fire_event("TRAINER_CLOSED", vec![]);
        self.signalled(script, mark);
        let trainer = self.appliers.p5().p5().trainer();
        if let Some(trainer) = trainer {
            self.release_into(script, mark, trainer, Self::close_trainer);
        }
        let cleared = self.appliers.p5().p5().clear_trainer(script);
        if cleared {
            latch!(self).note_closed(NpcWindow::Trainer);
        }
    }

    /// `OpenTrainer()` (`0x4d8cf0`): with the active player (`0x4d8d24`), `TalkToTrainer` on its
    /// own guid (`0x5df680`), which closes the window (`0x5df6bb`) before `CMSG_TRAINER_LIST`
    /// (`0x5df6fc`).
    pub(super) fn open_own_trainer(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(me) = self.appliers.p5().p5().own_guid() else {
            return;
        };
        self.close_trainer(script, mark);
        self.appliers.p5().p5().ask_own_list(me);
    }

    /// The auction house's close core `0x4cd5f0`: nothing with no auctioneer (`0x4cd5fb`), else
    /// `AUCTION_HOUSE_CLOSED` first (`0x4cd602`), the release on the cell as the listeners left
    /// it (`0x4cd60e`), the auctioneer, house and flags cleared with the lists left standing
    /// (`0x4cd613`-`0x4cd629`), then the sell slot emptied with its unlock (`0x4cd62e` →
    /// `0x4cd640`).
    pub(super) fn close_auction(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        if self.appliers.p4().p3().auctioneer().is_none() {
            return;
        }
        script.fire_event("AUCTION_HOUSE_CLOSED", vec![]);
        self.signalled(script, mark);
        let auctioneer = self.appliers.p4().p3().auctioneer();
        if let Some(auctioneer) = auctioneer {
            self.release_into(script, mark, auctioneer, Self::close_auction);
        }
        if self.appliers.p4().p3().auctioneer().is_some() {
            self.appliers.p4().p3().clear_auctioneer(script);
            latch!(self).note_closed(NpcWindow::Auction);
        }
        script.clear_auction_sell_item();
        self.signalled(script, mark);
    }

    /// The gossip menu's close core `0x4e1fa0` with a zero guid: nothing with no menu
    /// (`0x4e1fb8`), else the release (`0x4e1fc5`), the cell zeroed (`0x4e1fcc`) and
    /// `GOSSIP_CLOSED` (`0x4e1fdc`); the slots, the text and its latch stay.
    pub(super) fn close_gossip(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(npc) = self.appliers.p5().p4().npc() else {
            return;
        };
        self.release_into(script, mark, npc, Self::close_gossip);
        self.appliers.p5().p4().clear_npc();
        latch!(self).note_closed(NpcWindow::Gossip);
        script.fire_event("GOSSIP_CLOSED", vec![]);
        self.signalled(script, mark);
    }

    /// The mailbox's close core, `0x4acd50` with `cl = 0`: the compose tab's silent reset first
    /// and always (`0x4acd55` → `0x4acdc0(0)`), then, with a mailbox open, the release
    /// (`0x4acd85`), the cell cleared (`0x4acd8f`), `MAIL_CLOSED` (`0x4acda3`) and the tail's
    /// next-mail-time query (`0x4acda8`, `0x4acdb1`).
    pub(super) fn close_mail(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        script.reset_compose_tab_silently();
        let Some(mailbox) = self.appliers.p5().p2().mailbox() else {
            return;
        };
        self.release_into(script, mark, mailbox, Self::close_mail);
        self.appliers.p5().p2().clear_mailbox(script);
        latch!(self).note_closed(NpcWindow::Mail);
        script.fire_event("MAIL_CLOSED", vec![]);
        self.signalled(script, mark);
        self.appliers.p5().p2().refresh_after_close();
    }

    /// The vendor's close core `0x4fadf0`: with a vendor open, the release (`0x4fae04`), the cell
    /// cleared (`0x4fae0e`), `MERCHANT_CLOSED` (`0x4fae2c`), then a vendor row held on the cursor
    /// dropped (`0x4fae31`-`0x4fae4a`) and a Repair base mode back to Point (`0x4fae4f`-`0x4fae68`).
    pub(super) fn close_merchant(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(vendor) = self.appliers.p5().p7().vendor() else {
            return;
        };
        self.release_into(script, mark, vendor, Self::close_merchant);
        self.appliers.p5().p7().clear_vendor(script);
        latch!(self).note_closed(NpcWindow::Merchant);
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
        latch!(self).note_closed(NpcWindow::Quest);
        script.set_quest(None);
        let _ = latch!(self).release(giver);
        script.fire_event("QUEST_FINISHED", vec![]);
        self.signalled(script, mark);
    }

    /// The stable's close core `0x4cae10`: with a master stored, the release (`0x4cae24`), the
    /// cell cleared (`0x4cae29`), then `PET_STABLE_CLOSED` (`0x4cae3a`).
    pub(super) fn close_stable(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(master) = self.appliers.p6().p2().master() else {
            return;
        };
        self.release_into(script, mark, master, Self::close_stable);
        self.appliers.p6().p2().clear_master(script);
        latch!(self).note_closed(NpcWindow::Stable);
        script.fire_event("PET_STABLE_CLOSED", vec![]);
        self.signalled(script, mark);
    }

    /// The trade window's close core, `0x4bf4e0(0, 0)`, after the item watchers' trade halves
    /// still queued: with a window open, the done flag stored (`0x4bf4fd`), the release
    /// (`0x4bf50a`), the cell cleared and `TRADE_CLOSED` (`0x4bf522`); then `CMSG_CANCEL_TRADE`,
    /// open or not (`0x4bf527`).
    pub(super) fn close_trade(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        if self.appliers.p4().p6().run_trade_touches(script) {
            self.signalled(script, mark);
        }
        let partner = self.appliers.p4().p6().partner_cell();
        if let Some(partner) = partner {
            script.set_trade_done(false);
            self.release_into(script, mark, partner, Self::close_trade);
            self.appliers.p4().p6().close_window(script);
            latch!(self).note_closed(NpcWindow::Trade);
            script.fire_event("TRADE_CLOSED", vec![]);
            self.signalled(script, mark);
        }
        self.appliers.p4().p6().send_cancel();
    }

    /// The taxi map's close core `0x4dc0f0`: with a map open, the release (`0x4dc105`), the cell
    /// cleared (`0x4dc113`) and each slot's route freed (`0x4dc130`-`0x4dc145`), then
    /// `TAXIMAP_CLOSED` (`0x4dc15f`).
    pub(super) fn close_taxi(&mut self, script: &mut UiScript, mark: &mut ReportMark) {
        let Some(flightmaster) = self.appliers.p5().p6().flightmaster() else {
            return;
        };
        self.release_into(script, mark, flightmaster, Self::close_taxi);
        self.appliers.p5().p6().clear_map(script);
        latch!(self).note_closed(NpcWindow::Taxi);
        script.fire_event("TAXIMAP_CLOSED", vec![]);
        self.signalled(script, mark);
    }
}
