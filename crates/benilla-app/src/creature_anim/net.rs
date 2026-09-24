//! The animation layer's packet handlers (in the net handler table since 2322, moved out of the
//! drain's combat, anim and mount arm files) — the server packets whose whole content is *a thing
//! to play on a streamed unit*: the melee engagement brackets, the aggro/alert flare, the swing
//! refusals, the two emote relays, the spell-visual kit push (decision 0280), a rider's flourish.
//! Each resolves the guid through the index and writes one message; the animation law itself
//! lives in [`super`]. Two kinds here have a second handler: the completed-swing record
//! ([`attacker_state`], with its client-side full-block synthesis) and the environmental-damage
//! kit ([`environmental_damage_log`]) also feed [`crate::combat_log`]'s chat line, which is
//! registered ahead of these and so runs first, as the drain's match ran them.

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

/// Register the layer's handlers — called from [`super::CreatureAnimPlugin`].
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
    // Engagement brackets (decision 0073): the standing Ready idle rides this window —
    // the client's gate is the auto-attack-target GUID being set, mirrored here as a
    // marker component on the attacker (including our own echo).
    debug!("net: attack start {attacker:#x} → {victim:#x}");
    if let Some(&e) = index.0.get(&attacker) {
        // Melee-start drops the `0x400` weapon-visual hold unconditionally (the client's
        // `0x60fc50` sibling clear) — a shooter that closes to melee leaves the drawn idle.
        // The LOCAL player's melee paths additionally run the full cancel funnel at send.
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
        // The client's `0x624e40` (death/stun arrive as this packet too): a pending
        // swing record flushes text-only and clears.
        flushes.write(SwingFlush(e));
    }
}

/// The server refused our melee swing (`SMSG_ATTACKSWING_NOTINRANGE`/`_BADFACING`/`_DEADTARGET`/
/// `_CANT_ATTACK`) — forwarded verbatim to [`crate::swing_refusal`], which owns the latch, the 4 s
/// repeat, and arm 4's silent StopAttack. Nothing is decided here: the arms differ only in what
/// that module does with them, and it holds the write set for all three.
fn attack_swing_error(error: AttackSwingError, edges: &mut MessageWriter<SwingRefusalEdge>) {
    edges.write(SwingRefusalEdge::Refused(error));
}

/// `SMSG_CANCEL_COMBAT` — the server forced our attack to stop. The swing family's fourth arm,
/// and the same act as `0x148`/`0x149`: the reference's handler `0x5e7dd0` is arm 4's body
/// verbatim.
fn cancel_combat(edges: &mut MessageWriter<SwingRefusalEdge>) {
    edges.write(SwingRefusalEdge::CombatCancelled);
}

/// `SMSG_FEIGN_DEATH_RESISTED` — the target shrugged off our Feign Death.
///
/// One red line and nothing else: the reference's handler `0x6e9800` is `push 0x1a5; call
/// 0x496720`, a bare `DisplayError(421)` with no latch, no cooldown and no state — the opposite of
/// its sibling above, and the reason the two do not share a path. Catalog row 421 is
/// `ERR_FEIGN_DEATH_RESISTED`, whose 1.12 string is the single word "Resisted".
///
/// It lives beside the swing arms because vmangos sends it in the same breath as
/// `SMSG_CANCEL_COMBAT` (`Objects/Unit.cpp:9445-9451`: a resisted feign death cancels the attack
/// and says so), and finding one without the other is how this family stayed half-built.
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
    // Aggro (2 HOSTILE) / stealth alert (0 ALERT) flare — pure audio, byte-verified
    // (`0x6056e0` is an exact two-way branch; any other value no-ops, and neither leg
    // touches animation/nameplate/UI — decision 0280). Vocals: `sound::creature`.
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

/// One completed melee swing (`SMSG_ATTACKERSTATEUPDATE`, decision 0073): the attacker's swing
/// anim starts NOW; the victim feedback (blood/flinch/text/impact sounds) defers to the swing
/// clip's attack-hit keyframe (`creature_anim::impact`, the client's `0x6247d0` router) — EXCEPT
/// the center combat text, which the client fires **synchronously at packet parse**
/// (`0x6255b0 → 0x629d30 → 0x703f50`, one call stack; decision 0580's fold-back).
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
    // Arm 5's `0x6259b6 call 0x5ea800`, whose first act is the swing-refusal latch clear
    // (`0x5ecdb0(0)`) — gated exactly as the reference gates it: the attacker IS the active player
    // (`0x5fa6d0`, a guid compare) and the victim resolves as a streamed unit. Written from here
    // rather than computed downstream because this is the only place holding both guids, and it
    // keeps the clear in packet order with the refusals (`crate::swing_refusal`).
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
    // The client-side FULL-BLOCK synthesis (`0x625e20`, decision 0279): a resolvable
    // victim + zero damage + a nonzero blocked amount rewrites the state to BLOCKS(5)
    // before any consumer sees the record — the only thing the wire's blocked_amount
    // ever does (a PARTIAL block stays state 1, indistinguishable from a plain hit).
    if victim.is_some() && s.damage == 0 && s.blocked != 0 {
        s.victim_state = 5;
    }
    // The center combat text (decision 0578/0580): self victim, at receive, AFTER the full-block
    // synthesis (so a full block reads BLOCK, not MISS) — the packet's absorb/resist/blocked
    // sums feed the confirmed helper-B partial trailers.
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
        // The **observed attacker auto-draws melee** — the ref's SECOND melee draw, independent
        // of the attack-start one, and the reason a swing is never delivered in the wrong stance:
        // `0x625829 cmp [attacker+0xd40],1; jne` → `SetSheatheState(1, bInstant=1, bFireEvent=1)`
        // at `0x62583a`, byte-read here. It sits
        // immediately after the attacker resolve and **before** any hit-info handling, so even a
        // swing whose animation is suppressed (`HitInfo & 0x10000`) still draws. Nothing else in
        // the policy can do this job: the per-animation reconcile's melee force is gated to
        // `CUR != 2` (`0x5fe0f9`/`0x5fe13b`), so a unit swinging with a bow drawn — a ranged
        // stance a shot left behind — would otherwise keep swinging with the bow forever. The
        // setter's own idempotency is the `cmp`: a request equal to the committed state is
        // refused there, so this is free on every swing after the first.
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
        // The receive-time arm goes THROUGH the gated dispatcher — `0x625823 je 0x625a3e` takes
        // the unresolved-attacker leg, which resolves the victim and calls `0x625a6d call
        // 0x624530`, so the LOOTABLE front gate applies here exactly as it does to the tag path
        // (`creature_anim::impact::lootable_victim`). It calls no consequence directly.
        !stores.get(v).is_ok_and(|s| s.0.unit_lootable())
    }) {
        // The client's SMSG-arm fallback: an attacker we can't resolve (out of range)
        // can't animate a swing — its victim feedback fires immediately and in FULL
        // (`0x625a6d`, the only receive-time victim dispatch). The PLACEHOLDER
        // attacker resolves nowhere downstream (blood defaults front).
        impacts.write(SwingImpact {
            swing,
            text_only: false,
            natural: None,
            // The receive-time arm: no tag fired, so the reference has no event point either —
            // the consumer falls back to the victim, the only anchor the packet leaves us.
            pos: None,
        });
    }
}

/// `SMSG_TEXT_EMOTE` — someone performed a `/`-emote (the TextEmote.dbc id; the anim, if any, is
/// the emote row's).
///
/// **Two consequences, and they are independent** (decision 1274): the anim + voice ride the
/// [`EmoteMessage`] and need the performer *streamed*; the chat sentence is queued for
/// [`crate::ui_chat`]'s composer and needs only the performer's *name*. An emote from someone
/// off-screen therefore still prints its line, which is the reference's shape — `0x49dbe0`
/// resolves a name and never touches the object manager.
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

/// `SMSG_EMOTE` — a bare Emotes.dbc anim id on a unit (the server-driven one-shot: NPC scripts,
/// the `/`-emote's own anim leg).
fn emote(guid: u64, emote_id: u32, index: &GuidIndex, out: &mut MessageWriter<EmoteMessage>) {
    out.write(EmoteMessage {
        source: index.0.get(&guid).copied(),
        kind: EmoteKind::Anim(emote_id),
    });
}

/// `SMSG_PLAY_SPELL_VISUAL` — the kit-push opcode (decision 0280): a stage-0 play on the unit, the
/// eat/drink kit cadence and mid-channel swaps. Consumer: `creature_anim::spell_visual`. The
/// [`PlaySeq`] stamp is taken only when the unit is streamed in, so an unstreamed guid never
/// advances the call-order counter.
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

/// `SMSG_ENVIRONMENTALDAMAGELOG`'s consequence (reader
/// `0x624fcc` inside `0x624f30`): the EnvironmentalDamage.dbc 6-slot table picks the damage type's
/// SpellVisualKit — fall's is the DustCloud_Land puff — played on the victim through the ordinary
/// discrete kit play (`0x60edf0`), the same leg the kit-push opcode rides. The pain vocal's exact
/// trigger is open — it folds in as its own edge once pinned.
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

/// `SMSG_MOUNTSPECIAL_ANIM` — a nearby rider's flourish: rear their mount (MountSpecial 94 on the
/// mount child; the hop happens in `creature_anim::flourish_to_anim`).
///
/// Our OWN guid is dropped: we played it locally at send time, and whether the sender gets the
/// SMSG echoed back is a server-config detail (LIVE-VERIFIED 2026-07-17, double-flourish probe:
/// vmangos's `SendMovementMessageToSet(.., false)` only cheat-logs on the flag — the
/// non-broadcaster delivery hardcodes self=true, so our deployment echoes; the optional per-player
/// broadcaster honors it and would not). Self-suppression on receive is correct under both configs.
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
