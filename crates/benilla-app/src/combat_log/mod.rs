//! The combat log: the handlers for the combat packets. The reference answers each packet from one
//! handler with two consumers, the chat line ([`chat`], the display dispatcher `0x629b60`) and the
//! floating number with the portrait's `UNIT_COMBAT` flash ([`text`]; `0x629d30` fires
//! `COMBAT_TEXT_UPDATE`); they run in that order. The two legs have different CVar gates and
//! classification rules.
//!
//! The completed swing and the environmental-damage log also drive animation; those handlers
//! ([`crate::creature_anim::net`]) register after these, so the line runs before the animation.

use benilla_protocol::messages::SpellOutcomeLog;
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::combat_text::{CombatTextSpawn, DamageTextGates};
use crate::names::NameCache;
use crate::net::{
    GuidIndex, NetCommands, NetHandlerApp, ObjectStore, Reputations, SelfGuid, ServerSoundMessage,
};
use crate::ui_chat::combat::{CombatLogRanges, LogPeriodicSpells};
use crate::ui_chat::ChatLog;
use crate::ui_unit::{CombatTextEvent, UnitCombatFeedback};

pub(crate) mod chat;
pub(crate) mod text;

/// The chat line's read-only inputs, built into a [`chat::ChatCtx`] per packet.
#[derive(SystemParam)]
pub(crate) struct Ctx<'w> {
    self_guid: Res<'w, SelfGuid>,
    group: Res<'w, crate::ui_party::GroupState>,
    index: Res<'w, GuidIndex>,
    factions: Option<Res<'w, crate::target::ring::Factions>>,
    reputations: Res<'w, Reputations>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    /// The eight display ranges.
    ranges: Res<'w, CombatLogRanges>,
    /// `CombatLogPeriodicSpells`.
    periodic: Res<'w, LogPeriodicSpells>,
}

impl Ctx<'_> {
    fn ctx(&self) -> chat::ChatCtx<'_> {
        chat::ChatCtx {
            self_guid: &self.self_guid,
            group: Some(&self.group),
            index: &self.index,
            factions: self.factions.as_deref(),
            reputations: &self.reputations,
            spells: self.spells.as_deref(),
            ranges: &self.ranges,
            periodic: self.periodic.0,
        }
    }
}

/// What the two legs resolve endpoints through and write to.
#[derive(SystemParam)]
pub(crate) struct Sinks<'w, 's> {
    /// `CombatDamage` and the two `Pet*` sub-gates.
    damage_text: Res<'w, DamageTextGates>,
    stores: Query<'w, 's, &'static mut ObjectStore>,
    poses: Query<'w, 's, &'static mut Transform>,
    log: ResMut<'w, ChatLog>,
    names: Res<'w, NameCache>,
    net: Res<'w, NetCommands>,
    text: MessageWriter<'w, CombatTextSpawn>,
    feedback: MessageWriter<'w, UnitCombatFeedback>,
    center: MessageWriter<'w, CombatTextEvent>,
}

/// The combat log's handler registration. Added ahead of
/// [`crate::creature_anim::CreatureAnimPlugin`], so a shared kind runs the line first.
pub(crate) struct CombatLogPlugin;

impl Plugin for CombatLogPlugin {
    fn build(&self, app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::AttackerState, on_attacker_state)
            .net_handler(K::SpellDamageLog, on_spell_damage_log)
            .net_handler(K::PeriodicAuraLog, on_periodic_aura_log)
            .net_handler(K::SpellHealLog, on_spell_heal_log)
            .net_handler(K::SpellEnergizeLog, on_spell_energize_log)
            .net_handler(K::DamageShield, on_damage_shield)
            .net_handler(K::SpellLogMiss, on_spell_log_miss)
            .net_handler(K::PartyKillLog, on_party_kill_log)
            .net_handler(K::SpellInstaKillLog, on_spell_insta_kill_log)
            .net_handler(K::ProcResist, on_spell_outcome_log)
            .net_handler(K::SpellOrDamageImmune, on_spell_outcome_log)
            .net_handler(K::SpellDispelLog, on_spell_dispel_log)
            .net_handler(K::DispelFailed, on_dispel_failed)
            .net_handler(K::EnchantmentLog, on_enchantment_log)
            .net_handler(K::SpellLogExecute, on_spell_log_execute)
            .net_handler(K::EnvironmentalDamageLog, on_environmental_damage_log)
            .net_handler(K::XpGain, on_xp_gain)
            .net_handler(K::ExplorationXp, on_exploration_xp)
            .net_handler(K::LevelUp, on_level_up)
            .net_handler(K::PvpCredit, on_pvp_credit);
    }
}

fn on_attacker_state(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::AttackerState(s) = ev {
        let ctx = c.ctx();
        chat::attacker_state(s, &ctx, &l.stores, &l.poses, &mut l.log);
    }
}

fn on_spell_damage_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::SpellDamageLog(s) = ev {
        let ctx = c.ctx();
        chat::spell_damage_log(s, &ctx, &l.stores, &l.poses, &mut l.log);
        text::spell_damage_log(
            s,
            &c.index,
            &c.self_guid,
            &l.stores,
            c.spells.as_deref(),
            *l.damage_text,
            c.periodic.0,
            &mut l.text,
            &mut l.feedback,
            &mut l.center,
        );
    }
}

/// `CombatLogPeriodicSpells` gates the whole packet: the handler `0x626dd0` reads it first
/// (`0x626dee`) and a zero jumps to the epilogue `0x6271b4`, so no line, number or word.
fn on_periodic_aura_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::PeriodicAuraLog(s) = ev {
        if !c.periodic.0 {
            return;
        }
        let ctx = c.ctx();
        chat::periodic_aura_log(&s, &ctx, &l.stores, &l.poses, &mut l.log);
        text::periodic_aura_log(
            s,
            &c.index,
            &c.self_guid,
            &l.stores,
            c.spells.as_deref(),
            *l.damage_text,
            &mut l.text,
            &mut l.feedback,
            &mut l.center,
            &l.names,
            &l.net,
        );
    }
}

fn on_spell_heal_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::SpellHealLog(s) = ev {
        let ctx = c.ctx();
        chat::spell_heal_log(s, &ctx, &l.stores, &l.poses, &mut l.log);
        text::spell_heal_log(
            s,
            &c.index,
            &c.self_guid,
            &mut l.feedback,
            &mut l.center,
            &l.names,
            &l.net,
        );
    }
}

fn on_spell_energize_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::SpellEnergizeLog(s) = ev {
        let ctx = c.ctx();
        chat::spell_energize_log(s, &ctx, &l.stores, &l.poses, &mut l.log);
        text::spell_energize_log(s, &c.self_guid, &mut l.center);
    }
}

fn on_damage_shield(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::DamageShield(s) = ev {
        let ctx = c.ctx();
        chat::damage_shield(s, &ctx, &l.stores, &l.poses, &mut l.log);
        text::damage_shield(
            s,
            &c.index,
            &c.self_guid,
            &l.stores,
            *l.damage_text,
            &mut l.text,
            &mut l.feedback,
        );
    }
}

fn on_spell_log_miss(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::SpellLogMiss(s) = ev {
        let ctx = c.ctx();
        chat::spell_log_miss(&s, &ctx, &l.stores, &l.poses, &mut l.log);
        text::spell_log_miss(
            s,
            &c.index,
            &c.self_guid,
            &l.stores,
            c.spells.as_deref(),
            *l.damage_text,
            &mut l.text,
            &mut l.feedback,
            &mut l.center,
        );
    }
}

// ── Chat-only packets: no damage number, so no floating-text leg ──

fn on_party_kill_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::PartyKillLog(k) = ev {
        let ctx = c.ctx();
        chat::party_kill_log(k, &ctx, &l.stores, &l.poses, &mut l.log);
    }
}

fn on_spell_insta_kill_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::SpellInstaKillLog(k) = ev {
        let ctx = c.ctx();
        chat::spell_insta_kill_log(k, &ctx, &l.stores, &l.poses, &mut l.log);
    }
}

fn on_spell_outcome_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    let (o, immune): (SpellOutcomeLog, bool) = match ev {
        SessionEvent::ProcResist(o) => (o, false),
        SessionEvent::SpellOrDamageImmune(o) => (o, true),
        _ => return,
    };
    let ctx = c.ctx();
    chat::spell_outcome_log(o, immune, &ctx, &l.stores, &l.poses, &mut l.log);
}

fn on_spell_dispel_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::SpellDispelLog(d) = ev {
        let ctx = c.ctx();
        chat::spell_dispel_log(&d, &ctx, &l.stores, &l.poses, &mut l.log);
    }
}

fn on_dispel_failed(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::DispelFailed(d) = ev {
        let ctx = c.ctx();
        chat::dispel_failed(&d, &ctx, &l.stores, &l.poses, &mut l.log);
    }
}

fn on_enchantment_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::EnchantmentLog(e) = ev {
        let ctx = c.ctx();
        chat::enchantment_log(e, &ctx, &l.stores, &l.poses, &mut l.log);
    }
}

fn on_spell_log_execute(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::SpellLogExecute(x) = ev {
        let ctx = c.ctx();
        chat::spell_log_execute(&x, &ctx, &l.stores, &l.poses, &mut l.log);
    }
}

fn on_environmental_damage_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::EnvironmentalDamageLog(e) = ev {
        let ctx = c.ctx();
        chat::environmental_damage_log(e, &ctx, &l.stores, &l.poses, &mut l.log);
    }
}

fn on_xp_gain(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::XpGain(x) = ev {
        text::xp_gain(x, &c.index, &c.self_guid, &mut l.text, &mut l.log);
    }
}

fn on_exploration_xp(
    In(ev): In<SessionEvent>,
    c: Ctx,
    mut l: Sinks,
    area_table: Option<Res<crate::area::AreaTableRes>>,
    exploration_sounds: Option<Res<crate::sound::ExplorationSounds>>,
    mut server_sounds: MessageWriter<ServerSoundMessage>,
) {
    if let SessionEvent::ExplorationXp(x) = ev {
        text::exploration_xp(
            x,
            area_table.as_deref(),
            exploration_sounds.as_deref(),
            &c.index,
            &c.self_guid,
            &l.stores,
            &mut server_sounds,
            &mut l.log,
        );
    }
}

fn on_level_up(In(ev): In<SessionEvent>, mut l: Sinks) {
    if let SessionEvent::LevelUp(lv) = ev {
        text::level_up(lv, &mut l.log);
    }
}

/// An honor award: the combat-log line (name-resolved, so it queues) and the `HONOR_GAINED`
/// number. A dishonorable kill carries negative honor, passed signed: the stock handler prefixes
/// "+" only to a positive number (`Blizzard_CombatText.lua:234`).
fn on_pvp_credit(In(ev): In<SessionEvent>, mut l: Sinks) {
    if let SessionEvent::PvpCredit(credit) = ev {
        l.log.push_pvp_credit(
            credit.honor,
            credit.victim_guid,
            u8::try_from(credit.victim_rank).unwrap_or(0),
        );
        l.center.write(CombatTextEvent {
            message_type: "HONOR_GAINED",
            data: Some(credit.honor.to_string()),
            extra: None,
        });
    }
}
