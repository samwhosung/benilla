//! The combat log: the handlers for the combat packets. The reference answers each packet from one
//! handler with two consumers: the chat line with the `COMBAT_TEXT_UPDATE` word its formatter
//! raises after it ([`chat`], the display dispatcher `0x629b60`), and the floating number with the
//! portrait's `UNIT_COMBAT` flash ([`text`]); they run in that order. The two legs have different
//! CVar gates and classification rules.
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
    GuidIndex, NetHandlerApp, ObjectStore, Reputations, SelfGuid, ServerSoundMessage,
};
use crate::ui_chat::combat::{CombatLogRanges, LogPeriodicSpells};
use crate::ui_chat::ChatLog;
use crate::ui_unit::UnitCombatFeedback;

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
    names: Option<Res<'w, NameCache>>,
    skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
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
            names: self.names.as_deref(),
            skill_lines: self.skill_lines.as_deref().map(|s| &s.catalog),
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
    text: MessageWriter<'w, CombatTextSpawn>,
    feedback: MessageWriter<'w, UnitCombatFeedback>,
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
            &mut l.text,
            &mut l.feedback,
        );
    }
}

/// `CombatLogPeriodicSpells` gates the whole packet: the handler `0x626dd0` reads it first
/// (`0x626dee`) and a zero jumps to the epilogue `0x6271b4`, so no line, number or word. So does
/// a target the client does not hold as a unit (`0x626e8a`-`0x626ea9`).
fn on_periodic_aura_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::PeriodicAuraLog(s) = ev {
        if !c.periodic.0 || !c.index.0.contains_key(&s.target) {
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
        );
    }
}

fn on_spell_heal_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::SpellHealLog(s) = ev {
        let ctx = c.ctx();
        chat::spell_heal_log(s, &ctx, &l.stores, &l.poses, &mut l.log);
        text::spell_heal_log(s, &c.index, &mut l.feedback);
    }
}

fn on_spell_energize_log(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::SpellEnergizeLog(s) = ev {
        let ctx = c.ctx();
        // `0x494770`, the energize `UNIT_COMBAT`, is an empty stub: no flash, only the line.
        chat::spell_energize_log(s, &ctx, &l.stores, &l.poses, &mut l.log);
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

/// An honor award: the combat-log line, which raises the `HONOR_GAINED` word once written.
fn on_pvp_credit(In(ev): In<SessionEvent>, mut l: Sinks) {
    if let SessionEvent::PvpCredit(credit) = ev {
        l.log.push_pvp_credit(
            credit.honor,
            credit.victim_guid,
            u8::try_from(credit.victim_rank).unwrap_or(0),
        );
    }
}

#[cfg(test)]
mod tests {
    use benilla_protocol::messages::{PeriodicAuraLog, PeriodicTick};
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::target::relations::fixture::{creature, player, store_of, HUMAN, ORC};

    const ME: u64 = 1;
    const MOB: u64 = 2;
    const CORRUPTION: u32 = 172;

    /// The handler's world: you and a creature held, Corruption known, the log empty.
    fn world(held: &[u64]) -> App {
        let mut app = App::new();
        app.add_message::<CombatTextSpawn>()
            .add_message::<UnitCombatFeedback>()
            .init_resource::<GuidIndex>()
            .init_resource::<crate::ui_party::GroupState>()
            .init_resource::<Reputations>()
            .init_resource::<CombatLogRanges>()
            .init_resource::<DamageTextGates>()
            .init_resource::<ChatLog>()
            .insert_resource(SelfGuid(Some(ME)))
            .insert_resource(LogPeriodicSpells(true))
            .insert_resource(crate::ui_action::Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [(
                        CORRUPTION,
                        benilla_formats::SpellDisplay {
                            id: CORRUPTION,
                            name: "Corruption".into(),
                            ..Default::default()
                        },
                    )]
                    .into_iter()
                    .collect(),
                ),
                ..crate::ui_action::Spells::empty_for_tests()
            });
        for unit in [player(ME, HUMAN), creature(MOB, ORC)] {
            if held.contains(&unit.guid) {
                let e = app.world_mut().spawn(store_of(&unit)).id();
                app.world_mut()
                    .resource_mut::<GuidIndex>()
                    .0
                    .insert(unit.guid, e);
            }
        }
        app
    }

    /// Your Corruption ticking on the creature, and what it left: the parked lines and the
    /// flashes.
    fn tick(held: &[u64]) -> (usize, usize) {
        let mut app = world(held);
        let packet = SessionEvent::PeriodicAuraLog(PeriodicAuraLog {
            target: MOB,
            caster: ME,
            spell_id: CORRUPTION,
            ticks: vec![PeriodicTick::Damage {
                amount: 40,
                school: 5,
                absorb: 0,
                resist: 0,
            }],
        });
        app.world_mut()
            .run_system_once_with(on_periodic_aura_log, packet)
            .unwrap();
        let lines = app.world().resource::<ChatLog>().pending_kinds().len();
        let flashes = app
            .world_mut()
            .resource_mut::<Messages<UnitCombatFeedback>>()
            .drain()
            .count();
        (lines, flashes)
    }

    /// `0x626dd0` skips the whole packet when its target is no unit the client holds
    /// (`0x626e8a`-`0x626ea9`): no line, though you, its caster, resolve; no flash.
    #[test]
    fn a_periodic_tick_on_a_target_not_held_leaves_nothing() {
        assert_eq!(tick(&[ME, MOB]), (1, 1));
        assert_eq!(tick(&[ME]), (0, 0));
    }
}
