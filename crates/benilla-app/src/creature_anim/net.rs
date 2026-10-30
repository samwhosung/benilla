//! The animation layer's packet handlers: each resolves a guid and writes one message to play on a
//! streamed unit. The environmental-damage log also feeds [`crate::combat_log`], whose handler is
//! registered first and so runs first. The swing's handler registers ahead of the combat log's
//! (`net_handler_ahead`): `0x62578d` flushes and dispatches the victim's feedback before it
//! writes the packet's line, last (`0x625a2c`, `0x625a78`). The attack start and stop signal the
//! active player's combat events in their packet, queued among the lines and feedback around
//! them ([`crate::ui_chat::ChatLog::push_combat_edge`]).

use benilla_protocol::messages::{AttackSwingError, AttackerState, EnvironmentalDamageLog};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::impact::{receive_swing, stop_swing, ImpactSinks};
use super::{
    AttackSeam, Engaged, EnvDamageTable, KitPush, MountFlourish, PendingImpacts, PlaySeq,
    SheathRequest, SwingImpact, SwingMessage,
};
use crate::net::{AiReactionMessage, EmoteKind, EmoteMessage, GuidIndex, NetHandlerApp, SelfGuid};
use crate::swing_refusal::{SwingRefusal, SwingRefusalEdge};
use crate::ui_action::{UiError, UiErrorKeys};

/// Register the layer's handlers, from [`super::CreatureAnimPlugin`].
pub(crate) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::AttackStart, on_attack_start)
        .net_handler(K::AttackStop, on_attack_stop)
        .net_handler(K::AiReaction, on_ai_reaction)
        .net_handler(K::AttackSwingError, on_swing_refusal)
        .net_handler(K::CancelCombat, on_swing_refusal)
        .net_handler(K::FeignDeathResisted, on_feign_death_resisted)
        .net_handler(K::MountSpecial, on_mount_special)
        .net_handler(K::TextEmote, on_text_emote)
        .net_handler(K::Emote, on_emote)
        .net_handler(K::PlaySpellVisual, on_play_spell_visual)
        .net_handler_ahead(K::AttackerState, on_attacker_state)
        .net_handler(K::EnvironmentalDamageLog, on_environmental_damage_log);
}

fn on_attack_start(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    log: Res<crate::ui_chat::ChatLog>,
) {
    if let SessionEvent::AttackStart { attacker, victim } = ev {
        attack_start(attacker, victim, &mut commands, &index, &self_guid, &log);
    }
}

fn on_attack_stop(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    log: Res<crate::ui_chat::ChatLog>,
    mut pending: ResMut<PendingImpacts>,
    mut sinks: ImpactSinks,
    mut stop_word: ResMut<super::AttackStopInFlight>,
) {
    if let SessionEvent::AttackStop { attacker, victim } = ev {
        attack_stop(
            attacker,
            victim,
            &mut commands,
            &index,
            (&self_guid, &log),
            &mut pending,
            &mut sinks,
            &mut stop_word,
        );
    }
}

fn on_ai_reaction(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut reactions: MessageWriter<AiReactionMessage>,
) {
    if let SessionEvent::AiReaction { unit, reaction } = ev {
        ai_reaction(unit, reaction, &index, &mut reactions);
    }
}

/// The refusals' arms, applied inside their packets ([`crate::swing_refusal`]): the latch, or the
/// server's silent stop (`0x625ab8`, `0x5e7dd0`) with what StopAttack runs.
fn on_swing_refusal(
    In(ev): In<SessionEvent>,
    time: Res<Time>,
    // Absent in a harness without the refusal plugin.
    refusal: Option<ResMut<SwingRefusal>>,
    mut seam: AttackSeam,
) {
    let edge = match ev {
        SessionEvent::AttackSwingError(e) => attack_swing_error(e),
        SessionEvent::CancelCombat => cancel_combat(),
        _ => return,
    };
    crate::swing_refusal::apply(
        edge,
        refusal.map(|r| r.into_inner()),
        time.elapsed(),
        Some(&mut seam),
    );
}

fn on_feign_death_resisted(In(ev): In<SessionEvent>, errors: Res<UiErrorKeys>) {
    if let SessionEvent::FeignDeathResisted = ev {
        feign_death_resisted(&errors);
    }
}

fn on_mount_special(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    index: Res<GuidIndex>,
    mut out: MessageWriter<MountFlourish>,
) {
    if let SessionEvent::MountSpecial { guid } = ev {
        mount_special(guid, &self_guid, &index, &mut out);
    }
}

fn on_text_emote(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut out: MessageWriter<EmoteMessage>,
    chat_log: Res<crate::ui_chat::ChatLog>,
    social: Res<crate::ui_social::SocialState>,
) {
    if let SessionEvent::TextEmote {
        guid,
        text_emote,
        target_name,
    } = ev
    {
        let ignored = social.is_ignored(guid);
        self::text_emote(
            guid,
            text_emote,
            target_name,
            ignored,
            &index,
            &mut out,
            &chat_log,
        );
    }
}

fn on_emote(In(ev): In<SessionEvent>, index: Res<GuidIndex>, mut out: MessageWriter<EmoteMessage>) {
    if let SessionEvent::Emote { guid, emote_id } = ev {
        emote(guid, emote_id, &index, &mut out);
    }
}

fn on_attacker_state(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    self_guid: Res<SelfGuid>,
    mut swings: MessageWriter<SwingMessage>,
    mut sheaths: MessageWriter<SheathRequest>,
    refusal: Option<ResMut<SwingRefusal>>,
    mut pending: ResMut<PendingImpacts>,
    mut sinks: ImpactSinks,
    mut play_seq: ResMut<PlaySeq>,
    tutorials: Option<Res<crate::tutorial::Tutorials>>,
    // The self-defence acquire the swing owes, absent in a harness without the target plugin.
    acquires: Option<ResMut<crate::target::OwedSelects>>,
    // The attacker's auto-move, which the swing ends, and the lane its `AUTOFOLLOW_END` goes on.
    mut auto_move: (
        crate::player::AutoMoveStop,
        Option<ResMut<crate::field_watch::NoticeLane>>,
    ),
) {
    if let SessionEvent::AttackerState(s) = ev {
        let attacker = s.attacker;
        attacker_state(
            tutorials.as_deref(),
            acquires.map(|a| a.into_inner()),
            s,
            &index,
            &self_guid,
            &mut swings,
            &mut sheaths,
            refusal.map(|r| r.into_inner()),
            &mut pending,
            &mut sinks,
            play_seq.next(),
        );
        // A held attacker's auto-move other than mode 2, the facing turn, ends (`0x625a03`-
        // `0x625a21` → `StopAutoMove(1, 1)`), after the self-defence acquire (`0x6259fe`) and ahead
        // of the packet's line (`0x625a2c`); it acts only on the active mover, so only on our own
        // swing.
        if index.0.contains_key(&attacker) {
            let (stop, lane) = &mut auto_move;
            stop.stop_unless_facing_turn(attacker, lane.as_deref_mut());
        }
    }
}

fn on_environmental_damage_log(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    table: Option<Res<EnvDamageTable>>,
    mut play_seq: ResMut<PlaySeq>,
    mut out: MessageWriter<KitPush>,
) {
    if let SessionEvent::EnvironmentalDamageLog(e) = ev {
        environmental_damage_log(e, &index, table.as_deref(), &mut play_seq, &mut out);
    }
}

fn on_play_spell_visual(
    In(ev): In<SessionEvent>,
    index: Res<GuidIndex>,
    mut play_seq: ResMut<PlaySeq>,
    mut out: MessageWriter<KitPush>,
) {
    if let SessionEvent::PlaySpellVisual { unit, kit_id } = ev {
        play_spell_visual(unit, kit_id, &index, &mut play_seq, &mut out);
    }
}

/// A unit began melee auto-attack (`SMSG_ATTACKSTART`, including our own echo).
fn attack_start(
    attacker: u64,
    victim: u64,
    commands: &mut Commands,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    log: &crate::ui_chat::ChatLog,
) {
    // The Ready idle rides this window: the client's gate is the auto-attack target being set.
    debug!("net: attack start {attacker:#x} → {victim:#x}");
    if let Some(&e) = index.0.get(&attacker) {
        // The precast is not touched: only the local StartAttack clears its bit (`0x5ecd87`).
        commands.entity(e).insert(Engaged(victim));
        // A held attacker that is the active player (`0x6256f1` → `0x5fa6d0`) signals
        // `PLAYER_ENTER_COMBAT` inside the packet (`0x6256ff`).
        if self_guid.0 == Some(attacker) {
            log.push_combat_edge(true);
        }
    }
}

/// A unit stopped melee auto-attack (`SMSG_ATTACKSTOP`).
fn attack_stop(
    attacker: u64,
    victim: u64,
    commands: &mut Commands,
    index: &GuidIndex,
    (self_guid, log): (&SelfGuid, &crate::ui_chat::ChatLog),
    pending: &mut PendingImpacts,
    sinks: &mut ImpactSinks,
    stop_word: &mut super::AttackStopInFlight,
) {
    debug!("net: attack stop {attacker:#x} → {victim:#x}");
    if let Some(&e) = index.0.get(&attacker) {
        // `StopAttack 0x624e40` (`0x625763`) zeroes the attacker's target and its stop in flight
        // (`0x624e6b`).
        commands.entity(e).remove::<Engaged>();
        stop_word.clear(e);
        stop_swing(e, pending, sinks);
        // After `StopAttack`'s swing flush (`0x625763` → `0x624e40`), the active player's
        // `PLAYER_LEAVE_COMBAT` (`0x62576a` → `0x5fa6d0`, `0x625778`).
        if self_guid.0 == Some(attacker) {
            log.push_combat_edge(false);
        }
    }
}

/// The server refused our swing (`SMSG_ATTACKSWING_*`); [`crate::swing_refusal`] owns the rest.
fn attack_swing_error(error: AttackSwingError) -> SwingRefusalEdge {
    SwingRefusalEdge::Refused(error)
}

/// `SMSG_CANCEL_COMBAT`: the server stopped our attack. The reference's handler `0x5e7dd0` is the
/// refusal family's fourth arm verbatim.
fn cancel_combat() -> SwingRefusalEdge {
    SwingRefusalEdge::CombatCancelled
}

/// `SMSG_FEIGN_DEATH_RESISTED`: one red "Resisted" line and nothing else. The reference's handler
/// `0x6e9800` is a bare `DisplayError(421)` (`0x496720`), with no latch and no state. vmangos sends
/// it together with `SMSG_CANCEL_COMBAT` (`Unit.cpp:9469-9470`).
fn feign_death_resisted(errors: &UiErrorKeys) {
    debug!("net: feign death resisted");
    errors.push(UiError::key("ERR_FEIGN_DEATH_RESISTED"));
}

/// A creature flared aggro or a stealth pre-aggro alert (`SMSG_AI_REACTION`).
fn ai_reaction(
    unit: u64,
    reaction: u32,
    index: &GuidIndex,
    reactions: &mut MessageWriter<AiReactionMessage>,
) {
    // Audio only (`0x6056e0`): 2 hostile and 0 alert flare; any other value does nothing.
    debug!("net: ai reaction {reaction} on {unit:#x}");
    if matches!(reaction, 0 | 2) {
        if let Some(&e) = index.0.get(&unit) {
            reactions.write(AiReactionMessage {
                unit: e,
                hostile: reaction == 2,
            });
        }
    }
}

/// One melee swing (`SMSG_ATTACKERSTATEUPDATE`): the attacker's swing plays now and the victim's
/// feedback waits for the clip's attack-hit key (`0x6247d0`); the combat-log line and its
/// `COMBAT_TEXT_UPDATE` word come off the same packet in [`crate::combat_log`], after this.
fn attacker_state(
    tutorials: Option<&crate::tutorial::Tutorials>,
    acquires: Option<&mut crate::target::OwedSelects>,
    s: AttackerState,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    swings: &mut MessageWriter<SwingMessage>,
    sheaths: &mut MessageWriter<SheathRequest>,
    refusal: Option<&mut SwingRefusal>,
    pending: &mut PendingImpacts,
    sinks: &mut ImpactSinks,
    seq: u64,
) {
    let victim = index.0.get(&s.victim).copied();
    // Our landed swing clears the refusal latch (`0x6259b6` → `0x5ea800` → `0x5ecdb0(0)`) when we
    // are the attacker (`0x5fa6d0`) and the victim resolves.
    if self_guid.0 == Some(s.attacker) && victim.is_some() {
        crate::swing_refusal::apply(
            SwingRefusalEdge::Landed,
            refusal,
            std::time::Duration::ZERO,
            None,
        );
    }
    if benilla_assets::trace::enabled() {
        benilla_assets::trace::line(
            "fct",
            &format!(
                "recv swing atk={:#x} victim={:#x} dmg={} vstate={} hit={:#x}",
                s.attacker, s.victim, s.damage, s.victim_state, s.hit_info
            ),
        );
    }
    let s = crate::combat_log::read_swing(s, index, &sinks.stores);
    // The log step `0x629b10`, which every path ends in: our own swing at victim state 1, a
    // hit, triggers Spells and Abilities (`0x629b1a`-`0x629b37`).
    if self_guid.0 == Some(s.attacker) && s.victim_state == 1 {
        if let Some(tutorials) = tutorials {
            tutorials.trigger_now(crate::tutorial::id::SPELLS_AND_ABILITIES);
        }
    }
    let swing = SwingMessage {
        attacker: Entity::PLACEHOLDER, // filled per branch below
        victim,
        hit_info: s.hit_info,
        victim_state: s.victim_state,
        damage: s.damage,
        school: s.school,
        displayed: s.displayed(),
        seq,
    };
    if let Some(&e) = index.0.get(&s.attacker) {
        // A resolved attacker draws melee (`0x625829` → `SetSheatheState(1)` at `0x62583a`) before
        // any hit-info handling, even when `HitInfo & 0x10000` suppresses the swing's animation;
        // the per-animation reconcile never forces melee over a drawn bow (`0x5fe0f9`/`0x5fe13b`).
        sheaths.write(SheathRequest {
            entity: e,
            state: 1,
            ceremony: false,
        });
        let swing = SwingMessage {
            attacker: e,
            ..swing
        };
        receive_swing(swing, pending, sinks);
        swings.write(swing);
        // A swing at us selects its attacker when nothing is selected (`0x6259c9`-`0x6259fe`),
        // after the victim's feedback and ahead of the packet's line (`0x625a2c`).
        if self_guid.0 == Some(s.victim) && victim.is_some() {
            if let Some(acquires) = acquires {
                acquires.push(crate::target::OwedSelect::Acquire(
                    e,
                    crate::target::Acquire::Attacker,
                ));
            }
        }
    } else if swing.victim.is_some_and(|v| {
        // The unresolved-attacker leg (`0x625823` → `0x625a3e`) calls the gated dispatcher
        // (`0x625a6d` → `0x624530`), so the lootable gate applies here too.
        !sinks.stores.get(v).is_ok_and(|s| s.0.unit_lootable())
    }) {
        // An attacker we cannot resolve cannot swing, so the victim's feedback fires now, in
        // full, before the line. The placeholder attacker resolves nowhere (blood defaults to
        // the front).
        sinks.fire(SwingImpact {
            swing,
            text_only: false,
            natural: None,
            pos: None,
        });
    }
}

/// `SMSG_TEXT_EMOTE`: the animation needs the performer streamed, the chat line only needs their
/// name, so an off-screen emote still prints (reference: `0x49dbe0`). An `ignored` performer's
/// line is dropped at receipt, before anything is composed (`0x49dc36` → `0x49dc3d`); its voice
/// has a gate of its own (`sound::emote`).
fn text_emote(
    guid: u64,
    text_emote: u32,
    target_name: String,
    ignored: bool,
    index: &GuidIndex,
    out: &mut MessageWriter<EmoteMessage>,
    chat_log: &crate::ui_chat::ChatLog,
) {
    out.write(EmoteMessage {
        guid,
        source: index.0.get(&guid).copied(),
        kind: EmoteKind::Text(text_emote),
    });
    if !ignored {
        chat_log.push_text_emote(guid, text_emote, target_name);
    }
}

/// `SMSG_EMOTE`: an `Emotes.dbc` id to play on a unit (NPC scripts, a `/`-emote's animation).
fn emote(guid: u64, emote_id: u32, index: &GuidIndex, out: &mut MessageWriter<EmoteMessage>) {
    out.write(EmoteMessage {
        guid,
        source: index.0.get(&guid).copied(),
        kind: EmoteKind::Anim(emote_id),
    });
}

/// `SMSG_PLAY_SPELL_VISUAL`: a stage-0 kit play on the unit (eating, drinking, mid-channel
/// swaps). Only a streamed unit takes a [`PlaySeq`] stamp.
fn play_spell_visual(
    unit: u64,
    kit_id: u32,
    index: &GuidIndex,
    play_seq: &mut PlaySeq,
    out: &mut MessageWriter<KitPush>,
) {
    if let Some(&e) = index.0.get(&unit) {
        out.write(KitPush {
            entity: e,
            kit_id,
            seq: play_seq.next(),
            at: Some(crate::lanes::stamp()),
        });
    }
}

/// `SMSG_ENVIRONMENTALDAMAGELOG`'s visual (`0x624fcc` in `0x624f30`): the damage type's kit plays
/// on the victim like any kit push (`0x60edf0`). The packet plays no sound: a fall's pain grunt
/// is the client's own landing predictor (`0x602d00`, `creature_anim::env_damage`).
fn environmental_damage_log(
    e: EnvironmentalDamageLog,
    index: &GuidIndex,
    table: Option<&EnvDamageTable>,
    play_seq: &mut PlaySeq,
    out: &mut MessageWriter<KitPush>,
) {
    if let Some(&ent) = index.0.get(&e.victim) {
        if let Some(kit_id) = table.and_then(|t| t.0.kit_id(e.damage_type)) {
            debug!(
                "net: environmental damage on {:#x} (type {}, {} dmg) → kit {kit_id}",
                e.victim, e.damage_type, e.damage
            );
            out.write(KitPush {
                entity: ent,
                kit_id,
                seq: play_seq.next(),
                at: Some(crate::lanes::stamp()),
            });
        }
    }
}

/// `SMSG_MOUNTSPECIAL_ANIM`: a nearby rider rears their mount (MountSpecial, 94). Our own guid is
/// dropped, as we played it at send: vmangos asks for no echo (`MovementHandler.cpp:972`) but
/// delivers one anyway unless its per-player broadcaster is on (`Object.cpp:2274`).
fn mount_special(
    guid: u64,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    out: &mut MessageWriter<MountFlourish>,
) {
    if self_guid.0 != Some(guid) {
        if let Some(&e) = index.0.get(&guid) {
            out.write(MountFlourish { unit: e });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// `SMSG_ATTACKSTART` engages the attacker and leaves its precast: of the precast's clears,
    /// only the local StartAttack's reaches the bit (`0x5ecd87`), none the packet's handler.
    #[test]
    fn the_attack_start_packet_leaves_the_precast() {
        const SHOOTER: u64 = 20;
        let mut world = World::new();
        let precast = super::super::Precast {
            spell_id: 75,
            busy: true,
        };
        let e = world.spawn(precast).id();
        let mut index = GuidIndex::default();
        index.0.insert(SHOOTER, e);
        world.insert_resource(index);
        world
            .run_system_once(|mut commands: Commands, index: Res<GuidIndex>| {
                let log = crate::ui_chat::ChatLog::default();
                attack_start(
                    SHOOTER,
                    30,
                    &mut commands,
                    &index,
                    &SelfGuid::default(),
                    &log,
                );
            })
            .unwrap();
        assert!(world.get::<Engaged>(e).is_some());
        assert_eq!(world.get::<super::super::Precast>(e), Some(&precast));
    }
}
