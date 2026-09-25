//! The animation layer's packet handlers: each resolves a guid and writes one message to play on a
//! streamed unit. The swing record and the environmental-damage log also feed
//! [`crate::combat_log`], whose handlers are registered first and so run first.

use benilla_protocol::messages::{AttackSwingError, AttackerState, EnvironmentalDamageLog};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{
    Engaged, EnvDamageTable, KitPush, MountFlourish, PlaySeq, RangedHold, SheathRequest,
    SwingFlush, SwingImpact, SwingMessage,
};
use crate::net::{AiReactionMessage, EmoteKind, EmoteMessage, GuidIndex, NetHandlerApp, SelfGuid};
use crate::swing_refusal::SwingRefusalEdge;
use crate::ui_action::{UiError, UiErrorKeys};
use crate::ui_unit::CombatTextEvent;

/// Register the layer's handlers, from [`super::CreatureAnimPlugin`].
pub(super) fn register(app: &mut App) {
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
        .net_handler(K::AttackerState, on_attacker_state)
        .net_handler(K::EnvironmentalDamageLog, on_environmental_damage_log);
}

fn on_attack_start(In(ev): In<SessionEvent>, mut commands: Commands, index: Res<GuidIndex>) {
    if let SessionEvent::AttackStart { attacker, victim } = ev {
        attack_start(attacker, victim, &mut commands, &index);
    }
}

fn on_attack_stop(
    In(ev): In<SessionEvent>,
    mut commands: Commands,
    index: Res<GuidIndex>,
    mut flushes: MessageWriter<SwingFlush>,
) {
    if let SessionEvent::AttackStop { attacker, victim } = ev {
        attack_stop(attacker, victim, &mut commands, &index, &mut flushes);
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

fn on_swing_refusal(In(ev): In<SessionEvent>, mut edges: MessageWriter<SwingRefusalEdge>) {
    match ev {
        SessionEvent::AttackSwingError(e) => attack_swing_error(e, &mut edges),
        SessionEvent::CancelCombat => cancel_combat(&mut edges),
        _ => {}
    }
}

fn on_feign_death_resisted(In(ev): In<SessionEvent>, mut errors: ResMut<UiErrorKeys>) {
    if let SessionEvent::FeignDeathResisted = ev {
        feign_death_resisted(&mut errors);
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
    mut chat_log: ResMut<crate::ui_chat::ChatLog>,
) {
    if let SessionEvent::TextEmote {
        guid,
        text_emote,
        target_name,
    } = ev
    {
        self::text_emote(
            guid,
            text_emote,
            target_name,
            &index,
            &mut out,
            &mut chat_log,
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
    mut impacts: MessageWriter<SwingImpact>,
    mut center: MessageWriter<CombatTextEvent>,
    mut sheaths: MessageWriter<SheathRequest>,
    mut edges: MessageWriter<SwingRefusalEdge>,
    stores: Query<&mut crate::net::ObjectStore>,
    mut play_seq: ResMut<PlaySeq>,
) {
    if let SessionEvent::AttackerState(s) = ev {
        attacker_state(
            s,
            &index,
            &self_guid,
            &mut swings,
            &mut impacts,
            &mut center,
            &mut sheaths,
            &mut edges,
            &stores,
            play_seq.next(),
        );
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
fn attack_start(attacker: u64, victim: u64, commands: &mut Commands, index: &GuidIndex) {
    // The Ready idle rides this window: the client's gate is the auto-attack target being set.
    debug!("net: attack start {attacker:#x} → {victim:#x}");
    if let Some(&e) = index.0.get(&attacker) {
        // Melee start drops the `0x400` weapon-visual hold (`0x60fc50`): a shooter closing to
        // melee leaves the drawn idle.
        commands
            .entity(e)
            .insert(Engaged(victim))
            .remove::<RangedHold>();
    }
}

/// A unit stopped melee auto-attack (`SMSG_ATTACKSTOP`).
fn attack_stop(
    attacker: u64,
    victim: u64,
    commands: &mut Commands,
    index: &GuidIndex,
    flushes: &mut MessageWriter<SwingFlush>,
) {
    debug!("net: attack stop {attacker:#x} → {victim:#x}");
    if let Some(&e) = index.0.get(&attacker) {
        commands.entity(e).remove::<Engaged>();
        // `0x624e40`: a pending swing flushes as text only; death and stun arrive as this packet.
        flushes.write(SwingFlush(e));
    }
}

/// The server refused our swing (`SMSG_ATTACKSWING_*`); [`crate::swing_refusal`] owns the rest.
fn attack_swing_error(error: AttackSwingError, edges: &mut MessageWriter<SwingRefusalEdge>) {
    edges.write(SwingRefusalEdge::Refused(error));
}

/// `SMSG_CANCEL_COMBAT`: the server stopped our attack. The reference's handler `0x5e7dd0` is the
/// refusal family's fourth arm verbatim.
fn cancel_combat(edges: &mut MessageWriter<SwingRefusalEdge>) {
    edges.write(SwingRefusalEdge::CombatCancelled);
}

/// `SMSG_FEIGN_DEATH_RESISTED`: one red "Resisted" line and nothing else. The reference's handler
/// `0x6e9800` is a bare `DisplayError(421)` (`0x496720`), with no latch and no state. vmangos sends
/// it together with `SMSG_CANCEL_COMBAT` (`Unit.cpp:9469-9470`).
fn feign_death_resisted(errors: &mut UiErrorKeys) {
    debug!("net: feign death resisted");
    errors.0.push(UiError::key("ERR_FEIGN_DEATH_RESISTED"));
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
/// feedback waits for the clip's attack-hit key (`0x6247d0`), except the center combat text, which
/// the client shows at parse (`0x6255b0` → `0x629d30` → `0x703f50`).
fn attacker_state(
    mut s: AttackerState,
    index: &GuidIndex,
    self_guid: &SelfGuid,
    swings: &mut MessageWriter<SwingMessage>,
    impacts: &mut MessageWriter<SwingImpact>,
    center: &mut MessageWriter<CombatTextEvent>,
    sheaths: &mut MessageWriter<SheathRequest>,
    edges: &mut MessageWriter<SwingRefusalEdge>,
    stores: &Query<&mut crate::net::ObjectStore>,
    seq: u64,
) {
    let victim = index.0.get(&s.victim).copied();
    // Our landed swing clears the refusal latch (`0x6259b6` → `0x5ea800` → `0x5ecdb0(0)`) when we
    // are the attacker (`0x5fa6d0`) and the victim resolves; sent from here to keep packet order.
    if self_guid.0 == Some(s.attacker) && victim.is_some() {
        edges.write(SwingRefusalEdge::Landed);
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
    // The full-block synthesis (`0x625e20`): a resolved victim, no damage and a blocked amount
    // read as state 5, BLOCK, before any consumer; a partial block stays state 1.
    if victim.is_some() && s.damage == 0 && s.blocked != 0 {
        s.victim_state = 5;
    }
    // The center text for a hit on us, after the synthesis so a full block reads BLOCK, not MISS.
    if self_guid.0 == Some(s.victim) {
        if let Some((message_type, data, extra)) = crate::combat_log::text::melee_center_text(
            s.hit_info,
            s.victim_state,
            s.damage,
            s.absorb,
            s.resist,
            s.blocked,
        ) {
            center.write(CombatTextEvent {
                message_type,
                data,
                extra,
            });
        }
    }
    let swing = SwingMessage {
        attacker: Entity::PLACEHOLDER, // filled per branch below
        victim,
        hit_info: s.hit_info,
        victim_state: s.victim_state,
        damage: s.damage,
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
        swings.write(SwingMessage {
            attacker: e,
            ..swing
        });
    } else if swing.victim.is_some_and(|v| {
        // The unresolved-attacker leg (`0x625823` → `0x625a3e`) calls the gated dispatcher
        // (`0x625a6d` → `0x624530`), so the lootable gate applies here too.
        !stores.get(v).is_ok_and(|s| s.0.unit_lootable())
    }) {
        // An attacker we cannot resolve cannot swing, so the victim's feedback fires now, in
        // full. The placeholder attacker resolves nowhere (blood defaults to the front).
        impacts.write(SwingImpact {
            swing,
            text_only: false,
            natural: None,
            pos: None,
        });
    }
}

/// `SMSG_TEXT_EMOTE`: the animation needs the performer streamed, the chat line only needs their
/// name, so an off-screen emote still prints (reference: `0x49dbe0`).
fn text_emote(
    guid: u64,
    text_emote: u32,
    target_name: String,
    index: &GuidIndex,
    out: &mut MessageWriter<EmoteMessage>,
    chat_log: &mut crate::ui_chat::ChatLog,
) {
    out.write(EmoteMessage {
        source: index.0.get(&guid).copied(),
        kind: EmoteKind::Text(text_emote),
    });
    chat_log.push_text_emote(guid, text_emote, target_name);
}

/// `SMSG_EMOTE`: an `Emotes.dbc` id to play on a unit (NPC scripts, a `/`-emote's animation).
fn emote(guid: u64, emote_id: u32, index: &GuidIndex, out: &mut MessageWriter<EmoteMessage>) {
    out.write(EmoteMessage {
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
