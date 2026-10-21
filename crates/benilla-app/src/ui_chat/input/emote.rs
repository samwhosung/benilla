//! `DoEmote`'s engine half, which the call host runs inside the verb ([`EmoteDoer`]): the binding
//! (`0x49fd30`) finds the `EmotesText` row by name (`0x49fd72`-`0x49fda6`) and the target
//! (`0x49fdb1`-`0x49fdee`), then `0x5ef560` (`0x49fe16`) runs the gates, the stow, the posture or
//! the play, and `CMSG_TEXT_EMOTE`, so the packet goes out before the verb returns.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::EmoteRequest;

use super::{emote_gate, emote_target, EmoteGate};
use crate::creature_anim::{move_flags, MovementState};
use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};

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
    /// Our own auto-attack target, a gate of the local play.
    engaged: Query<'w, 's, (), (With<SelfPlayer>, With<crate::creature_anim::Engaged>)>,
    loot_kneel: Option<Res<'w, crate::ui_loot::LootKneel>>,
    errors: Res<'w, crate::ui_action::UiErrorKeys>,
    out: EmoteOut<'w>,
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
pub(crate) struct EmoteOut<'w> {
    sheath: Option<MessageWriter<'w, crate::creature_anim::SheathRequest>>,
    stand: Option<MessageWriter<'w, crate::player::StandStateRequest>>,
    /// The local play (`0x5ef660`).
    anim: Option<MessageWriter<'w, crate::creature_anim::EmoteAnim>>,
    play_seq: Option<ResMut<'w, crate::creature_anim::PlaySeq>>,
}

impl EmoteDoer<'_, '_> {
    /// `DoEmote(token, target)` inside its call: a token with no `EmotesText` row does nothing
    /// (`0x49fda8`).
    pub(crate) fn do_emote(&mut self, request: EmoteRequest) {
        let text_id = self
            .emotes
            .as_deref()
            .and_then(|c| c.text_id(&request.token));
        match text_id {
            Some(text_id) => self.emote(text_id, request.target.as_deref()),
            None => debug!(
                "chat: DoEmote({:?}): no EmotesText row for that token",
                request.token
            ),
        }
    }

    /// `0x5ef560` on text emote `text_id`: the gates in the client's order, then the stow, the
    /// posture or the play, and the packet. `arg` is the target argument.
    pub(crate) fn emote(&mut self, text_id: u32, arg: Option<&str>) {
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
            self.errors
                .push(crate::ui_action::UiError::key("ERR_NOEMOTEWHILERUNNING"));
            return;
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
            return;
        }
        let me = self.player.single().ok().map(|(e, m)| (e, *m));
        // The stow (`0x5ef630`), unconditional here: every emote past the gates sheathes
        // instantly, mid-fight too.
        if let (Some((entity, _)), Some(sheath)) = (me, self.out.sheath.as_mut()) {
            sheath.write(crate::creature_anim::SheathRequest {
                entity,
                state: 0,
                ceremony: false,
            });
        }
        // The posture branch (`EmoteSpecProc == 1` → `SetStandState`) is the whole visible
        // effect: the server plays nothing for SIT, SLEEP and KNEEL (`ChatHandler.cpp:728`).
        if let Some(state) = posture {
            if let Some(stand) = self.out.stand.as_mut() {
                stand.write(crate::player::StandStateRequest { state: state as u8 });
            }
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
                engaged: !self.engaged.is_empty(),
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
        let fallback = crate::ui_party::GroupState::default();
        let group = self.group.as_deref().unwrap_or(&fallback);
        let target = emote_target(
            arg,
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
