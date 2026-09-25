//! The probes: one file per `--flag` scenario, each a [`Probe`] run in registration order.

use anyhow::Result;
use benilla_protocol::{SessionEvent, WorldSession};

use crate::world::World;

/// What a [`Probe`]'s lifecycle methods borrow: the live session and the shared [`World`].
pub(crate) struct Ctx<'a> {
    pub session: &'a mut WorldSession,
    pub world: &'a mut World,
}

/// One scripted wire-verification scenario, driven through this lifecycle in registration order.
pub(crate) trait Probe {
    /// One-time pre-stream staging (GM teleports, cleanup commands).
    fn stage(&mut self, cx: &mut Ctx) -> Result<()> {
        let _ = cx;
        Ok(())
    }
    /// Every pump iteration, before recv.
    fn poll(&mut self, cx: &mut Ctx) -> Result<()> {
        let _ = cx;
        Ok(())
    }
    /// Every decoded event, after [`World::on_event`] has processed it.
    fn on_event(&mut self, ev: &SessionEvent, cx: &mut Ctx) -> Result<()> {
        let _ = (ev, cx);
        Ok(())
    }
    /// Post-stream: assertions and follow-up round trips.
    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let _ = cx;
        Ok(())
    }
}

// Quest constants shared by several probes.

/// Onto Marshal McBride, the `--quest` turn-in NPC and quest 7's giver and ender.
pub(crate) const QUEST_TURNIN_TP: &str = ".go xyz -8902.59 -162.606 82.0223";
pub(crate) const QUEST_TURNIN_ENTRY: u32 = 197; // Marshal McBride, who takes 783
/// `PLAYER_QUEST_LOG_1_1`: `UNIT_END` (188) + 0xA, 3 fields per slot for 20 slots
/// (`UpdateFields_1_12_1.h:128`).
pub(crate) const FIELD_PLAYER_QUEST_LOG_1_1: u16 = 198;

/// Quest 7 "Kobold Camp Cleanup": Marshal McBride (197) gives and takes it
/// (`creature_questrelation`, `creature_involvedrelation`), and its objective, 10 kills of entry
/// 6, is a real counted slot, unlike 783's report-only "A Threat Within".
pub(crate) const QUESTLOG_ID: u32 = 7;

mod attack;
mod aura;
mod charge;
mod death;
mod equip_pack_slot;
mod giverstatus;
mod groundfx;
mod loot;
mod mount_tele;
mod open_item;
mod query_names;
mod quest;
mod quest_item;
mod questlog;
mod questtimer;
mod self_res;
mod speed;
mod spells;
mod spirit;
mod swap_pack_slots;
mod use_pack_slot;
mod vendor;
mod worldstate;

pub(crate) use attack::Attack;
pub(crate) use aura::Aura;
pub(crate) use charge::Charge;
pub(crate) use death::Death;
pub(crate) use equip_pack_slot::EquipPackSlot;
pub(crate) use giverstatus::GiverStatus;
pub(crate) use groundfx::GroundFx;
pub(crate) use loot::Loot;
pub(crate) use mount_tele::MountTele;
pub(crate) use open_item::OpenItem;
pub(crate) use query_names::QueryNames;
pub(crate) use quest::Quest;
pub(crate) use quest_item::QuestItem;
pub(crate) use questlog::QuestLog;
pub(crate) use questtimer::QuestTimer;
pub(crate) use self_res::SelfRes;
pub(crate) use speed::Speed;
pub(crate) use spells::Spells;
pub(crate) use spirit::Spirit;
pub(crate) use swap_pack_slots::SwapPackSlots;
pub(crate) use use_pack_slot::UsePackSlot;
pub(crate) use vendor::Vendor;
pub(crate) use worldstate::WorldState;
