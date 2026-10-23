//! `DoEmote`'s engine half, which the call host runs inside the verb ([`EmoteDoer`]): the binding
//! (`0x49fd30`) finds the `EmotesText` row by name (`0x49fd72`-`0x49fda6`) and the target
//! (`0x49fdb1`-`0x49fdee`), then `0x5ef560` (`0x49fe16`) runs the gates, the stow, the posture or
//! the play, and `CMSG_TEXT_EMOTE`, each inside the call, so the stow's `CMSG_SETSHEATHED` and the
//! posture's `CMSG_STANDSTATECHANGE` go out ahead of the emote's packet, before the verb returns.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::EmoteRequest;

use super::{emote_gate, emote_target, EmoteGate};
use crate::creature_anim::{move_flags, MovementState};
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};

/// The stand states the posture branch sets, `[0x85ff10]`: 0, 1, 3 and 8.
const POSTURE_STATES: u32 = 0x10b;

/// What `DoEmote` reaches outside the VM, as the call host holds it. The writers and the roster
/// are optional, so a world without them still lends a host; a world without the emote tables
/// sends no emote.
#[derive(SystemParam)]
pub(crate) struct EmoteDoer<'w, 's> {
    commands: Res<'w, NetCommands>,
    emotes: Option<Res<'w, crate::sound::EmoteSounds>>,
    /// Our stand state and move flags for the gates, and the entity the stow and play are for.
    player: Query<'w, 's, (Entity, &'static MovementState), With<SelfPlayer>>,
    store: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    /// Our own auto-attack target as the queued unit edits leave it, a gate of the local play.
    edits: Option<Res<'w, crate::creature_anim::UnitEdits>>,
    loot_kneel: Option<Res<'w, crate::ui_loot::LootKneel>>,
    /// Where the moving refusal shows inside the call (`0x496720`).
    sink: crate::ui_action::MessageSink<'w>,
    out: EmoteOut<'w, 's>,
    /// The target the emote names when its argument does not (`0xb4e2d8`).
    selection: Res<'w, crate::target::Selection>,
    names: Res<'w, crate::names::NameCache>,
    group: Option<Res<'w, crate::ui_party::GroupState>>,
    guids: Res<'w, crate::net::GuidIndex>,
    players: crate::target::PlayerLookup<'w, 's>,
    self_guid: Option<Res<'w, crate::net::SelfGuid>>,
}

/// The stow, the posture, the play and their order stamp.
#[derive(SystemParam)]
pub(crate) struct EmoteOut<'w, 's> {
    /// The instant stow (`0x5ef630`) and `SetStandState` (`0x5ef652`), run inside the call.
    stand: crate::player::StandSetter<'w, 's>,
    /// The local play (`0x5ef660`).
    anim: Option<MessageWriter<'w, crate::creature_anim::EmoteAnim>>,
    play_seq: Option<ResMut<'w, crate::creature_anim::PlaySeq>>,
}

/// An emote past its gates, stow and posture or play, owing its packet ([`EmoteDoer::finish`]).
#[derive(Debug)]
pub(crate) struct EmoteStep {
    text_id: u32,
    arg: Option<String>,
    /// The posture changed to a state but 0, so its local apply owes the loot close and the
    /// attack stop (`0x5f07d5`, `0x5f07e7`) ahead of the packet.
    pub(crate) leave: bool,
}

impl EmoteDoer<'_, '_> {
    /// `DoEmote(token, target)` inside its call, up to its packet: a token with no `EmotesText`
    /// row does nothing (`0x49fda8`).
    pub(crate) fn do_emote(
        &mut self,
        script: &mut benilla_ui::script::UiScript,
        request: EmoteRequest,
    ) -> Option<EmoteStep> {
        let text_id = self
            .emotes
            .as_deref()
            .and_then(|c| c.text_id(&request.token));
        match text_id {
            Some(text_id) => self.emote(script, text_id, request.target),
            None => {
                debug!(
                    "chat: DoEmote({:?}): no EmotesText row for that token",
                    request.token
                );
                None
            }
        }
    }

    /// `0x5ef560` on text emote `text_id`: the gates in the client's order, then the stow, the
    /// posture or the play, up to the packet [`Self::finish`] sends. `arg` is the target argument.
    pub(crate) fn emote(
        &mut self,
        script: &mut benilla_ui::script::UiScript,
        text_id: u32,
        arg: Option<String>,
    ) -> Option<EmoteStep> {
        let (stand_state, flags) = self
            .player
            .single()
            .map_or((0, 0), |(_, m)| (m.stand_state, m.flags));
        let swimming = flags & move_flags::SWIMMING != 0;
        let emotes = self.emotes.as_deref();
        let emote_id = emotes.and_then(|e| e.text_emote(text_id));
        let posture = emote_id.and_then(|id| emotes?.posture_state(id));
        // The gates (`0x5ef57e`-`0x5ef5c3`): a possessed unit, then the `Emotes.dbc` row, then
        // `CheckEmoteEligible` (`0x47db40`); only its `0x4000` arm refuses out loud. A chat-only
        // text emote (`EmoteID` 0) reads row 0.
        let unit_flags = self.store.single().map_or(0, |s| s.0.unit_flags());
        let gate = match emotes {
            Some(e) => emote_gate(
                e.emote_flags(emote_id.unwrap_or(0)),
                unit_flags,
                stand_state,
                swimming,
                flags,
            ),
            None => EmoteGate::Send,
        };
        // The `0x4000` arm's red line (`0x5ef5d0`), only while self-controlled, so a feared or
        // confused player emotes normally.
        let controlled = || {
            self.store
                .single()
                .is_ok_and(|s| crate::player::self_controlled(s.0.unit_flags()))
        };
        if gate == EmoteGate::Moving && controlled() {
            debug!("chat: emote {text_id} refused — moving (ERR_NOEMOTEWHILERUNNING)");
            let input = script.input_events();
            if let Some(line) = crate::ui_action::keyed_line(script, "ERR_NOEMOTEWHILERUNNING") {
                crate::ui_action::show_now(script, &mut self.sink, "chat", line.under(input));
            }
            return None;
        }
        let eligible = gate != EmoteGate::Suppressed;
        // Gate B (`0x5ef5f3`): asleep, only a posture emote gets through, so `/stand` ends
        // `/sleep` and a `/wave` in bed does nothing.
        let awake_or_posture = stand_state != 3 || posture.is_some();
        if !eligible || !awake_or_posture {
            debug!(
                "chat: emote {text_id} suppressed (eligible {eligible}, awake-or-posture \
                 {awake_or_posture}, stand {stand_state})"
            );
            return None;
        }
        let me = self.player.single().ok().map(|(e, m)| (e, *m));
        // The stow, `SetSheatheState(0, 1, 1)` (`0x5ef630`): every emote past the gates sheathes
        // instantly, mid-fight too, and its `CMSG_SETSHEATHED` goes out now (`0x611dbc`).
        self.out.stand.stow();
        // The posture branch (`EmoteSpecProc == 1` → `SetStandState`) is the whole visible
        // effect: the server plays nothing for SIT, SLEEP and KNEEL (`ChatHandler.cpp:728`). Only
        // a state in the mask `[0x85ff10]` = `0x10b` (stand, sit, sleep, kneel) takes it
        // (`0x5ef63e`-`0x5ef64d`); any other falls through to the play.
        let posture = posture.filter(|&state| (1u32 << (state & 31)) & POSTURE_STATES != 0);
        let mut leave = false;
        if let Some(state) = posture {
            leave = self.out.stand.set(state as u8).unwrap_or(false);
        }
        // The play (`0x5ef660`, `0x5fe2f0` at `0x5ef66f`) is the posture branch's else: a posture
        // emote plays no clip, and the rest play here only while moving. Its `SMSG_EMOTE` echo then
        // finds the id armed and is skipped (`0x5fcd56`).
        else if let (Some(anim), Some((entity, m)), Ok(store)) = (
            emote_id.and_then(|id| emotes?.anim(id)),
            me,
            self.store.single(),
        ) {
            let fields = &store.0;
            let gate = crate::creature_anim::LocalPlay {
                reads_dead: fields.unit_reads_dead(),
                stand_state: m.stand_state,
                move_flags: m.flags,
                vertical_speed: m.vertical_speed,
                flying: m.flying,
                loot_kneel: self.loot_kneel.as_deref().is_some_and(|k| k.0),
                channeling: fields.unit_channel_spell() != 0,
                engaged: self.player.single().is_ok_and(|(me, _)| {
                    self.edits
                        .as_deref()
                        .is_some_and(|edits| edits.attacking(me))
                }),
            };
            if crate::creature_anim::local_play_eligible(&gate) {
                if let (Some(out), Some(seq)) = (self.out.anim.as_mut(), self.out.play_seq.as_mut())
                {
                    out.write(crate::creature_anim::EmoteAnim {
                        entity,
                        anim_id: anim as u16,
                        seq: seq.next(),
                        via_player: false,
                    });
                }
            }
        }
        Some(EmoteStep {
            text_id,
            arg,
            leave,
        })
    }

    /// The emote's tail once the caller has run its leave: the stand state's local apply ends
    /// (`0x6127cc`), then the target and `CMSG_TEXT_EMOTE`.
    pub(crate) fn finish(&mut self, step: EmoteStep) {
        let EmoteStep { text_id, arg, .. } = step;
        self.out.stand.finish();
        let fallback = crate::ui_party::GroupState::default();
        let group = self.group.as_deref().unwrap_or(&fallback);
        let target = emote_target(
            arg.as_deref(),
            &self.selection,
            |name| self.players.find(&self.names, group, &self.guids, name),
            self.self_guid.as_deref().and_then(|g| g.0),
        );
        match self
            .commands
            .0
            .send(ClientCommand::TextEmote { text_id, target })
        {
            Ok(()) => info!("chat: sent text emote {text_id}"),
            Err(_) => warn!("chat: not connected; text emote {text_id} dropped"),
        }
    }
}
