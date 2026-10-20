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
use crate::entities::MissileMiss;
use crate::names::NameCache;
use crate::net::{
    Guid, GuidIndex, NetHandlerApp, ObjectStore, Reputations, SelfGuid, ServerSoundMessage,
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
    enchants: Option<Res<'w, crate::items::Enchants>>,
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
            enchants: self.enchants.as_deref().map(|e| &e.0),
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
            .net_handler(K::SpellGo, on_spell_go)
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
            .net_handler(K::PvpCredit, on_pvp_credit)
            // Written by the missile flight; declared here too, as registering is idempotent.
            .add_message::<MissileMiss>()
            // After the flight that writes the arrivals, and the party drain whose roster the
            // classifier reads.
            .add_systems(
                Update,
                on_missile_miss
                    .after(crate::entities::EntityVisualsSet)
                    .after(crate::ui_party::drain_party),
            );
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

/// An extended packet hands its entries' floats to a hook and emits nothing (`0x5e7ec1`).
fn on_spell_log_miss(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    if let SessionEvent::SpellLogMiss(s) = ev {
        if s.extended {
            return;
        }
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

/// `SMSG_SPELL_GO`'s miss list, for a spell with a record (`0x6e7cf7`) whose Speed is 0
/// (`0x6e7d4e`-`0x6e7d5f`): a travelling spell's misses wait for its missile
/// ([`on_missile_miss`]).
fn on_spell_go(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    let SessionEvent::SpellGo {
        caster,
        spell_id,
        misses,
        ..
    } = ev
    else {
        return;
    };
    let spells = c.spells.as_deref();
    let Some(display) = spells.and_then(|s| s.catalog.get(spell_id)) else {
        return;
    };
    if misses.is_empty() || display.speed != 0.0 {
        return;
    }
    let ctx = c.ctx();
    chat::spell_go_misses(
        caster, spell_id, &misses, &ctx, &l.stores, &l.poses, &mut l.log,
    );
    text::spell_go_misses(
        caster,
        spell_id,
        &misses,
        &c.index,
        &c.self_guid,
        &l.stores,
        spells,
        *l.damage_text,
        &mut l.text,
        &mut l.feedback,
    );
}

/// A travelling spell's missed arrival on a live target (`0x61dd50`): its `UNIT_COMBAT`, then its
/// line. The missile holds its caster by entity; one gone mid-flight is the line's unresolved
/// endpoint.
fn on_missile_miss(
    mut arrivals: MessageReader<MissileMiss>,
    guids: Query<&Guid>,
    c: Ctx,
    stores: Query<&ObjectStore>,
    poses: Query<&Transform>,
    mut log: ResMut<ChatLog>,
    mut feedback: MessageWriter<UnitCombatFeedback>,
) {
    for arrival in arrivals.read() {
        let Ok(&Guid(target)) = guids.get(arrival.anchor) else {
            continue;
        };
        let caster = guids.get(arrival.caster).map_or(0, |g| g.0);
        text::missile_miss(
            arrival.anchor,
            arrival.spell_id,
            arrival.code,
            c.spells.as_deref(),
            &mut feedback,
        );
        let ctx = c.ctx();
        chat::missile_miss(
            caster,
            target,
            arrival.spell_id,
            arrival.code,
            &ctx,
            &stores,
            &poses,
            &mut log,
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
        text::xp_gain(
            x,
            &c.index,
            &c.self_guid,
            &l.stores,
            &mut l.text,
            &mut l.log,
        );
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

/// An honor credit (`0x625270`): the combat-log line, which raises the `HONOR_GAINED` word once
/// written. A kill needs its victim held as a unit (`0x62529f`, `0x6252ab`), and an honorable one
/// a victim on a side ([`honor_side`]); otherwise nothing at all.
fn on_pvp_credit(In(ev): In<SessionEvent>, c: Ctx, mut l: Sinks) {
    let SessionEvent::PvpCredit(credit) = ev else {
        return;
    };
    let rank = u8::try_from(credit.victim_rank).unwrap_or(0);
    let side = if credit.victim_guid == 0 {
        0
    } else {
        let Some(victim) = c
            .index
            .0
            .get(&credit.victim_guid)
            .and_then(|&e| l.stores.get(e).ok())
            .filter(|s| {
                use benilla_protocol::messages::ObjectType;
                matches!(
                    s.0.object_type(),
                    Some(ObjectType::Unit | ObjectType::Player)
                )
            })
        else {
            return;
        };
        if credit.honor > 0 {
            match honor_side(victim, c.factions.as_deref()) {
                Some(side) => side,
                None => return,
            }
        } else {
            0
        }
    };
    l.log
        .push_pvp_credit(credit.honor, credit.victim_guid, rank, side);
}

/// The side an honorable kill's rank title is read on (`0x6252de`-`0x625321`): the victim's
/// `UNIT_FIELD_FACTIONTEMPLATE` row in `FactionTemplate.dbc`, its group mask (`+0xc`) `4`, the
/// Horde, giving 0, else `2`, the Alliance, giving 1; any other drops the credit.
fn honor_side(
    victim: &ObjectStore,
    factions: Option<&crate::target::ring::Factions>,
) -> Option<u8> {
    let template = factions?
        .catalog()
        .template(victim.0.unit_faction_template()?)?;
    if template.group_mask & 0x4 != 0 {
        Some(0)
    } else if template.group_mask & 0x2 != 0 {
        Some(1)
    } else {
        None
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
    /// Speed 0: an instant ability.
    const STRIKE: u32 = 11976;
    /// Speed 24: it travels.
    const FROSTBOLT: u32 = 116;

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
            .add_message::<MissileMiss>()
            .insert_resource(crate::ui_action::Spells {
                catalog: benilla_formats::SpellCatalog::from_displays(
                    [
                        (CORRUPTION, "Corruption", 0.0),
                        (STRIKE, "Strike", 0.0),
                        (FROSTBOLT, "Frostbolt", 24.0),
                    ]
                    .into_iter()
                    .map(|(id, name, speed)| {
                        (
                            id,
                            benilla_formats::SpellDisplay {
                                id,
                                name: name.into(),
                                speed,
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
                ),
                ..crate::ui_action::Spells::empty_for_tests()
            });
        for unit in [player(ME, HUMAN), creature(MOB, ORC)] {
            if held.contains(&unit.guid) {
                let e = app
                    .world_mut()
                    .spawn((Guid(unit.guid), store_of(&unit)))
                    .id();
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

    /// What a world left after one packet: the parked lines and the flashes.
    fn left(app: &mut App) -> (usize, usize) {
        let lines = app.world().resource::<ChatLog>().pending_kinds().len();
        let flashes = app
            .world_mut()
            .resource_mut::<Messages<UnitCombatFeedback>>()
            .drain()
            .count();
        (lines, flashes)
    }

    /// The creature's `spell_id` missing you in its `SMSG_SPELL_GO`.
    fn go_miss(spell_id: u32) -> (usize, usize) {
        let mut app = world(&[ME, MOB]);
        let packet = SessionEvent::SpellGo {
            caster: MOB,
            spell_id,
            cast_flags: 0,
            hits: vec![],
            misses: vec![benilla_protocol::messages::SpellGoMiss {
                target: ME,
                info: 3,
                reflect_result: 0,
            }],
            target: Some(ME),
            go_target: None,
            dest: None,
            ammo_display_id: None,
            item_caster: None,
        };
        app.world_mut()
            .run_system_once_with(on_spell_go, packet)
            .unwrap();
        left(&mut app)
    }

    /// `0x6e7a70` runs its miss list only for a spell with a record (`0x6e7cf7`) and Speed 0
    /// (`0x6e7d4e`): an instant's dodge has its line and flash at the packet, a travelling spell's
    /// wait for the missile, and an unknown spell's never come.
    #[test]
    fn a_go_miss_logs_at_the_packet_only_for_an_instant_spell() {
        assert_eq!(go_miss(STRIKE), (1, 1));
        assert_eq!(go_miss(FROSTBOLT), (0, 0));
        assert_eq!(go_miss(24_000), (0, 0));
    }

    /// An extended `SMSG_SPELLLOGMISS` hands its floats to a hook and emits nothing (`0x5e7ec1`).
    #[test]
    fn an_extended_spell_log_miss_emits_nothing() {
        let miss = |extended| {
            let mut app = world(&[ME, MOB]);
            let packet = SessionEvent::SpellLogMiss(benilla_protocol::messages::SpellLogMiss {
                spell_id: STRIKE,
                caster: MOB,
                extended,
                misses: vec![(ME, 6)],
            });
            app.world_mut()
                .run_system_once_with(on_spell_log_miss, packet)
                .unwrap();
            left(&mut app)
        };
        assert_eq!(miss(false), (1, 1));
        assert_eq!(miss(true), (0, 0));
    }

    /// A travelling spell's miss logs on arrival (`0x61dd50`): its flash (`0x61ddcc`), then its
    /// line (`0x61ddeb`), the missile's caster to its target.
    #[test]
    fn a_missile_miss_logs_on_arrival() {
        let mut app = world(&[ME, MOB]);
        let entity = |app: &App, guid| app.world().resource::<GuidIndex>().0[&guid];
        let (caster, anchor) = (entity(&app, MOB), entity(&app, ME));
        app.world_mut().write_message(MissileMiss {
            caster,
            anchor,
            spell_id: FROSTBOLT,
            code: 2,
        });
        app.world_mut().run_system_once(on_missile_miss).unwrap();
        let lines: Vec<_> = app
            .world()
            .resource::<ChatLog>()
            .pending_combat()
            .iter()
            .map(|l| (l.family.stem, l.subject, l.object))
            .collect();
        assert_eq!(lines, [("SPELLRESIST", MOB, ME)]);
        assert_eq!(left(&mut app).1, 1);
    }

    /// `0x625270` drops a kill whose victim is no unit the client holds (`0x6252ab`), and an
    /// honorable kill whose victim's faction template group is neither Horde (side 0) nor
    /// Alliance (side 1) (`0x62530c`-`0x625321`); a dishonorable kill takes no side, and an award
    /// has no victim.
    #[test]
    fn an_honor_credit_needs_its_victim_held_and_on_a_side() {
        use crate::target::relations::fixture::factions;
        const ORC_PLAYER: u64 = 3;
        const NEUTRAL: u64 = 4;
        let credit = |victim_guid, honor| {
            let mut app = world(&[ME, MOB]);
            app.insert_resource(factions());
            for unit in [player(ORC_PLAYER, ORC), creature(NEUTRAL, 99)] {
                let e = app
                    .world_mut()
                    .spawn((Guid(unit.guid), store_of(&unit)))
                    .id();
                app.world_mut()
                    .resource_mut::<GuidIndex>()
                    .0
                    .insert(unit.guid, e);
            }
            let packet = SessionEvent::PvpCredit(benilla_protocol::messages::PvpCredit {
                honor,
                victim_guid,
                victim_rank: 8,
            });
            app.world_mut()
                .run_system_once_with(on_pvp_credit, packet)
                .unwrap();
            app.world().resource::<ChatLog>().pending_honor()
        };
        assert_eq!(credit(ORC_PLAYER, 120), [(ORC_PLAYER, 120, 8, 0)]);
        assert_eq!(
            credit(MOB, 120),
            [(MOB, 120, 8, 0)],
            "an orc creature is Horde too"
        );
        assert!(credit(NEUTRAL, 120).is_empty(), "no side");
        assert_eq!(credit(NEUTRAL, -40), [(NEUTRAL, -40, 8, 0)]);
        assert!(credit(77, 120).is_empty(), "not held");
        assert_eq!(credit(0, 25), [(0, 25, 8, 0)]);
    }

    /// `0x637ec0`: a kill of a held unit is named, and floats; a zero one there writes and
    /// floats nothing (`0x625202`); any other award is unnamed and floats; your own guid is an XP
    /// loss, with no float (`0x637f70`).
    #[test]
    fn the_xp_packet_takes_the_reference_arm() {
        let gain = |victim, total, kill| {
            let mut app = world(&[ME, MOB]);
            let packet = SessionEvent::XpGain(benilla_protocol::messages::XpGain {
                victim,
                total,
                base: 40,
                kill,
            });
            app.world_mut()
                .run_system_once_with(on_xp_gain, packet)
                .unwrap();
            let log = app.world().resource::<ChatLog>();
            let (lines, kinds) = (log.pending_xp(), log.pending_kinds());
            let floats = app
                .world_mut()
                .resource_mut::<Messages<CombatTextSpawn>>()
                .drain()
                .count();
            (lines, kinds, floats)
        };
        assert_eq!(gain(MOB, 60, true), (vec![(MOB, 60, 40)], vec!["xp"], 1));
        assert_eq!(
            gain(77, 60, true),
            (vec![(0, 60, 40)], vec!["xp"], 1),
            "not held"
        );
        assert_eq!(
            gain(MOB, 60, false),
            (vec![(0, 60, 40)], vec!["xp"], 1),
            "no kill"
        );
        assert_eq!(gain(MOB, 0, true), (vec![], vec![], 0), "a zero kill");
        assert_eq!(
            gain(0, 0, false),
            (vec![(0, 0, 40)], vec!["xp"], 1),
            "a zero award"
        );
        assert_eq!(
            gain(ME, 60, true),
            (vec![], vec!["xp loss"], 0),
            "your own guid"
        );
    }
}
