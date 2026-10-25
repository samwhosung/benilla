//! Posture: the stand state (the `/sit` family and `X`) and the sheath toggle (`Z`), one
//! mechanism in the reference. A stand state other than 0 or 2 stows drawn weapons through the
//! setter `Z` drives (2 inferred to be the chair sit, vmangos's `UNIT_STAND_STATE_SIT_CHAIR`), and
//! the toggle refuses on the stand state this frame committed. Both refuse silently, with no
//! packet, so the `sit` trace tag ([`super::move_trace::posture`]) records each commit and refusal.
//! Both act on the active player alone (`0x5ed46c`, `0x5eb4ba`), whichever body is driven: under
//! possession `X`, the `/sit` family and `Z` read and change the character, and the possessed
//! unit's movement stands nobody.

use benilla_protocol::ObjectFields;
use bevy::prelude::*;

use super::{
    move_trace, state, BodyQuery, ClientCommand, NetCommands, Player, SelfPosture,
    StandStateRequest,
};
use crate::net::{ObjectStore, SelfPlayer};

/// What the posture verbs read of the player beyond its own components.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PlayerReads {
    /// Our own control lost ([`super::OwnControlLost`]): `IsClientControlled` (`0x5fa5f0`) reads
    /// the unit's `[+0xc58] & 0x400`, which `0x5fa600` writes beside that flag for our character.
    pub(super) control_lost: bool,
}

/// Runs this frame's stand-state decision and sheath toggle on the active player, and returns the
/// driven body's stand state, which its pose reads: the active player's local commit over its
/// echoed byte, or a possessed unit's own byte.
pub(super) fn update(
    player: &mut Player,
    pending: &mut super::StandPending,
    body: &BodyQuery,
    me: &SelfPosture,
    binds: &crate::bindings::BindingsState,
    net: &NetCommands,
    commands: &mut Commands,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    loot_close: &mut crate::ui_loot::LootMoveStart,
    asks: &mut MessageReader<StandStateRequest>,
    server: &mut MessageReader<super::ServerStandState>,
    reads: PlayerReads,
    classes: Option<&benilla_formats::ChrClasses>,
    moving: bool,
    turned: bool,
) -> u8 {
    let me = me.single();
    // The driven body is the active player's own, which the movement stand-up needs.
    let embodied = me.is_some_and(|(.., embodied)| embodied);
    // The echo into `UNIT_FIELD_BYTES_1` drives every observer's pose; `pending`, the local
    // commit ([`apply_locally`]), overlays it until it lands.
    let (stand_byte, gates) = stand_of(me);
    if pending.0 == Some(stand_byte) {
        pending.0 = None; // the echo landed
    }
    // `SMSG_STANDSTATE_UPDATE` reaches the local apply directly, with no refusal gate and no
    // `CMSG_STANDSTATECHANGE` back, before the volunteered logic below; the last packet wins.
    if let Some(s) = server.read().last().map(|m| m.state) {
        let stand_now = pending.stand_state(stand_byte);
        move_trace::posture("server", s, stand_now, player.move_flags());
        apply_locally(pending, s, stand_now, me, commands, sheath, loot_close);
    }
    // The queued asks (the idle sit, the approach's stand, the attack's stand) ran in the
    // reference's message pass, before the X key is read, each `SetStandState` whole in its order,
    // so an ask reads the state the one before it left (`GetStandState` `0x5ed570` answers the
    // prediction). An ask whose packet went out where it was made, past its gate, owes the local
    // apply alone (`0x5ed53f` → `0x6127b0`).
    let flags = player.move_flags();
    for ask in asks.read() {
        let stand_now = pending.stand_state(stand_byte);
        if !ask.sent {
            set_stand_state(
                ask.state, stand_now, gates, flags, me, net, pending, commands, sheath, loot_close,
            );
        } else if me.is_some() {
            move_trace::posture("sent", ask.state, stand_now, flags);
            apply_locally(
                pending, ask.state, stand_now, me, commands, sheath, loot_close,
            );
        }
    }
    let stand_state = pending.stand_state(stand_byte);
    // Movement stands us up, volunteered, as the server never does: translation, a keyboard turn
    // and jump reach the stand wrapper `0x60be30(0)` from any nonzero state; a left-drag orbit
    // does not, nor a right-drag turn (`0x514f50` skips its stand arm while the button is held),
    // though a release within 200 ms is a right-click, whose action stands you. A knockback's move
    // event stands us too (`0x60e139`: `GetStandState` `0x60be50`, then `0x60be30(0)`), read off
    // the latch [`super::wire_in::apply_server_moves`] armed this frame, as the mover runs later.
    // The wrapper acts on the mover, and only a player (`0x60be39`) that is the active one
    // (`0x5ed471`): a possessed unit's movement stands nobody.
    let knocked_out_of_it = player.knockback.is_some();
    if embodied
        && (moving || turned || knocked_out_of_it || binds.fired(crate::bindings::Input::Jump))
        && stand_state != 0
    {
        set_stand_state(
            0,
            stand_state,
            gates,
            flags,
            me,
            net,
            pending,
            commands,
            sheath,
            loot_close,
        );
    }
    let stand_now = pending.stand_state(stand_byte);
    // The sheath toggle (Z) cycles the anim layer's committed sheath state through its one setter
    // ([`crate::creature_anim::SheathRequest`]), which sends `CMSG_SETSHEATHED`; only it plays the
    // ceremony (`bInstant = 0` in `ToggleSheath` `0x5eb480`). No body model yet drops the press.
    if binds.fired(crate::bindings::Input::ToggleSheath) {
        if let Some((e, Some((drv, committed)), store, engaged, wielded, precast, _)) = me {
            let toggle = ToggleGates {
                control_lost: reads.control_lost,
                engaged,
                precast,
                seated: stand_now != 0,
                mid_ceremony: drv.sheath_ceremony_active(),
            };
            if toggle_refused(store.map(|s| &s.0), toggle) {
                debug!("sheath toggle refused ({toggle:?})");
            } else {
                // Melee, ranged, stowed, skipping what is not worn; `None` makes no call at all.
                let w = wielded.copied().unwrap_or_default();
                let worn = toggle_worn(&w, store.map(|s| &s.0), classes);
                let next =
                    crate::creature_anim::toggle_sheath_next(committed.state().unwrap_or(0), worn);
                if let Some(state) = next {
                    sheath.write(crate::creature_anim::SheathRequest {
                        entity: e,
                        state,
                        ceremony: true,
                    });
                }
            }
        }
    }

    if embodied || me.is_none() {
        stand_now
    } else {
        // A possessed unit poses by its own byte; the prediction is the active player's.
        body.single()
            .ok()
            .and_then(|(.., store, _, _, _, _)| store)
            .map_or(0, |s| s.0.unit_stand_state())
    }
}

/// The active player's posture setters run inside a script call, so their packets leave ahead of
/// whatever the call sends next: the instant stow `SetSheatheState(0, 1, 1)` (`0x611cf0`) and
/// `SetStandState` (`0x5ed430`): its gate ([`state::stand_state_refused`]), `CMSG_STANDSTATECHANGE`
/// (`0x5ed501`) and the local apply (`0x5ed53f` → `0x6127b0`). The setter tests no current state:
/// a sit while sitting sends again. A world without the player's resources reads them empty.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct StandSetter<'w, 's> {
    net: Res<'w, NetCommands>,
    /// The mover's flags, the `[[this+0x118]+0x40]` the gate reads.
    player: Option<Res<'w, Player>>,
    /// The prediction `[player+0x1d68]` the local apply writes (`0x5f07a3`).
    pending: Option<ResMut<'w, super::StandPending>>,
    me: Query<'w, 's, (Entity, &'static ObjectStore), With<SelfPlayer>>,
    /// The committed sheath state the stow sets (`[+0xd40]`).
    sheath: Query<'w, 's, &'static mut crate::creature_anim::CommittedSheath, With<SelfPlayer>>,
    /// The precast as the queued unit edits leave it, which a link of the VM pass reads in place
    /// of the component.
    edits: Option<Res<'w, crate::creature_anim::UnitEdits>>,
}

impl StandSetter<'_, '_> {
    /// What `SitOrStand()` (`0x48b920`) asks of the active player: a sit (1) when its descriptor's
    /// stand state byte (`[[player+0x110]+0x210]`, `0x48b947`) is 0, else a stand (0); `None` with
    /// no active player (`0x48b93f`). The byte, not the local prediction.
    pub(crate) fn sit_or_stand(&self) -> Option<u8> {
        let (_, store) = self.me.single().ok()?;
        Some(u8::from(store.0.unit_stand_state() == 0))
    }

    /// `SetSheatheState(0, 1, 1)` (`0x611cf0`): stowed now, its `CMSG_SETSHEATHED` sent
    /// (`0x611dbc`) when the state changed.
    pub(crate) fn stow(&mut self) {
        let fields = self.me.single().ok().map(|(_, s)| &s.0);
        if let Ok(mut committed) = self.sheath.single_mut() {
            if committed.set_now(0, fields, None) {
                let _ = self.net.0.send(ClientCommand::SetSheathed { state: 0 });
            }
        }
    }

    /// `SetStandState(s)` up to its local apply's leave: the gate, the packet, and the prediction
    /// written on a change (`0x5f0799`-`0x5f07a3`). `None` when the gate refuses; else whether the
    /// change leaves, to a state but 0 (`0x5f07a9`), which owes the loot close (`0x48f200` at
    /// `0x5f07d5`) and the attack stop (`0x5ecac0` at `0x5f07e7`) the caller runs before
    /// [`Self::finish`]. Not built: the input resume on a stand (`0x5f07ba`).
    pub(crate) fn set(&mut self, s: u8) -> Option<bool> {
        let (byte, gates) = {
            let (me, store) = self.me.single().ok()?;
            let precast = self
                .edits
                .as_deref()
                .is_some_and(|e| e.precast(me).is_some());
            (store.0.unit_stand_state(), stand_gates(&store.0, precast))
        };
        let flags = self.player.as_deref().map_or(0, Player::move_flags);
        if state::stand_state_refused(gates, flags, s) {
            debug!("stand state {s} refused in a call ({gates:?}, the client's `0x5ed430` gate)");
            move_trace::posture("REFUSED", s, byte, flags);
            return None;
        }
        move_trace::posture("call", s, byte, flags);
        let _ = self.net.0.send(ClientCommand::StandStateChange {
            state: u32::from(s),
        });
        let now = self
            .pending
            .as_deref()
            .map_or(byte, |p| p.stand_state(byte));
        if now == s {
            return Some(false);
        }
        if let Some(pending) = self.pending.as_deref_mut() {
            pending.0 = Some(s);
        }
        Some(s != 0)
    }

    /// The local apply's tail (`0x6127cc`-`0x6127fb`): drawn weapons stow for a stand state but 0
    /// or 2.
    pub(crate) fn finish(&mut self) {
        let byte = self.me.single().map_or(0, |(_, s)| s.0.unit_stand_state());
        let now = self
            .pending
            .as_deref()
            .map_or(byte, |p| p.stand_state(byte));
        if now != 0 && now != 2 {
            self.stow();
        }
    }
}

/// `SetStandState`'s descriptor legs (`0x5ed440`-`0x5ed4bd`): a channelling, mounted, flying or
/// dead player changes no stand state. The precast legs (`0x5ed44b`, `0x5ed4cc`) are the
/// caller's.
pub(crate) fn stand_up_refused(fields: &ObjectFields) -> bool {
    state::stand_state_refused(stand_gates(fields, false), 0, 0)
}

/// StartAttack's stand-up for a seated player (`0x5ecc90`-`0x5ecc9b`), `SetStandState(0)`: past
/// its gate (`refused` is its descriptor and precast legs; a stand to 0 reads no movement,
/// `0x5ed4f0`), `CMSG_STANDSTATECHANGE` goes out here (`0x5ed501`), ahead of the select and the
/// swing, and the local apply follows in the posture pass ([`StandStateRequest::sent`]). A stand
/// to 0 leaves nothing to close or stow.
pub(crate) fn stand_up_for_attack(
    refused: bool,
    net: &NetCommands,
    asks: &mut MessageWriter<StandStateRequest>,
) {
    if refused {
        debug!("stand state 0 refused at the attack (the client's `0x5ed430` gate)");
        return;
    }
    let _ = net.0.send(ClientCommand::StandStateChange { state: 0 });
    asks.write(StandStateRequest {
        state: 0,
        sent: true,
    });
}

/// [`state::StandGates`] off the body's descriptor and cast state.
/// The active player's echoed stand byte and the gates `SetStandState` reads ([`stand_gates`]).
fn stand_of(me: Option<SelfPostureItem>) -> (u8, state::StandGates) {
    let precast = me.is_some_and(|(.., precast, _)| precast);
    me.and_then(|(_, _, store, ..)| store)
        .map(|s| (s.0.unit_stand_state(), stand_gates(&s.0, precast)))
        .unwrap_or((
            0,
            state::StandGates {
                precast,
                ..default()
            },
        ))
}

/// The stand-up a command press's tick runs ahead of its start: the forward emitter stands the
/// mover on every tick with a nonzero total (`0x514e0c` → `0x60be30(0)` → `0x5ed430`), which sends
/// `CMSG_STANDSTATECHANGE` at once, before the start is queued (`0x514e6e`). Only for the active
/// player driving its own body (`0x60be39`, `0x5ed471`). Standing applies nothing locally but the
/// prediction ([`local_apply`]), so the frame time's stand-up finds the body standing.
pub(super) fn stand_up_for_movement(
    pending: &mut super::StandPending,
    me: &SelfPosture,
    flags: u32,
    net: &NetCommands,
) {
    let me = me.single();
    if !me.is_some_and(|(.., embodied)| embodied) {
        return;
    }
    let (stand_byte, gates) = stand_of(me);
    let stand_now = pending.stand_state(stand_byte);
    if stand_now != 0 && send_stand_state(0, stand_now, gates, flags, true, net) {
        pending.0 = Some(0);
    }
}

fn stand_gates(fields: &ObjectFields, precast: bool) -> state::StandGates {
    state::StandGates {
        mounted: fields.unit_mount_display_id() != 0,
        on_taxi: fields.unit_flags() & super::UNIT_FLAG_TAXI_FLIGHT != 0,
        // Health 0 or the feign flag, without `unit_reads_dead`'s stand state 7 (`0x5ed4a9`–
        // `0x5ed4bd`).
        reads_dead: fields.unit_is_dead() || fields.unit_dynflag_dead(),
        channelling: fields.unit_channel_spell() != 0,
        precast,
    }
}

/// What `ToggleSheath` (`0x5eb480`) reads of the player ahead of its cycle, past the active-player
/// test, besides the descriptor [`toggle_refused`] reads; each refuses silently, with no call and
/// no packet. Its model tests (`0x5eb542`, `0x5eb580`) are the setter's executor's, which holds
/// the model ([`crate::creature_anim::SheathRequest`]).
#[derive(Clone, Copy, Debug, Default)]
struct ToggleGates {
    /// `IsClientControlled` (`0x5eb4d1`) is clear.
    control_lost: bool,
    /// An attack target, `[+0xc48]` (`0x5eb513`).
    engaged: bool,
    /// A precast spell, `[+0xd58] & 0x400` (`0x5eb49f`) or `[+0xc8c]` (`0x5eb52b`).
    precast: bool,
    /// A stand state above 0 (`0x5eb553`).
    seated: bool,
    /// An arm still plays a sheathe animation (`0x5eb5a0`–`0x5eb5e2`).
    mid_ceremony: bool,
}

/// Whether the toggle refuses: on `gates`, and off the descriptor on a mount (`0x5eb4e4`), the
/// stun flag (`0x5eb4f2`), health at or below 0 (`0x5eb502`) and a channel (`0x5eb48e`).
fn toggle_refused(fields: Option<&ObjectFields>, gates: ToggleGates) -> bool {
    let mounted = fields.is_some_and(|f| f.unit_mount_display_id() != 0);
    let stunned = fields.is_some_and(|f| f.unit_flags() & super::UNIT_FLAG_STUNNED != 0);
    let dead = fields.is_some_and(ObjectFields::unit_is_dead);
    let channelling = fields.is_some_and(|f| f.unit_channel_spell() != 0);
    gates.control_lost
        || mounted
        || stunned
        || dead
        || gates.engaged
        || channelling
        || gates.precast
        || gates.seated
        || gates.mid_ceremony
}

/// What the toggle cycles over: a hand holding a weapon and a ranged candidate, as
/// `GetWeapon(slot, 0)` reads them (`0x5eb5f0`, `0x5eb600`, `0x5eb610`: a disarmed hand is not
/// worn), the ranged one kept only for a class with a ranged sheathe state (`0x5eb616`-`0x5eb640`,
/// [`benilla_formats::ChrClasses::has_ranged_sheath`]). Without the table, it is kept.
fn toggle_worn(
    w: &crate::creature_anim::Wielded,
    fields: Option<&ObjectFields>,
    classes: Option<&benilla_formats::ChrClasses>,
) -> (bool, bool) {
    // The class byte `[+0x79]`, `UNIT_FIELD_BYTES_0` byte 1.
    let class = u32::from(fields.and_then(ObjectFields::unit_class).unwrap_or(0));
    (
        w.armed_main().is_some() || w.armed_off().is_some(),
        w.ranged.is_some() && classes.is_none_or(|t| t.has_ranged_sheath(class)),
    )
}

/// The stand state's local apply, the reference's `0x6127b0`, reached from `SetStandState` after
/// its send and from `SMSG_STANDSTATE_UPDATE` (`0x603e50`), doing what [`local_apply`] decides.
/// Not built: the input resume on a stand (`0x5f07ba`) and the follow-camera re-acquire each call
/// (`0x48ec90`).
fn apply_locally(
    pending: &mut super::StandPending,
    s: u8,
    stand_now: u8,
    me: Option<SelfPostureItem>,
    commands: &mut Commands,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    loot_close: &mut crate::ui_loot::LootMoveStart,
) {
    let drawn = me
        .and_then(|(_, drv, ..)| drv)
        .and_then(|(_, committed)| committed.state())
        .unwrap_or(0)
        != 0;
    let apply = local_apply(s, stand_now, drawn);
    if apply.predict {
        pending.0 = Some(s);
    }
    if apply.leave {
        // The same close as the movement start's (`0x60e9dd`), under the same `0x6126b0`.
        loot_close.0 = true;
        commands.run_system_cached(stop_attack_for_stand_state);
    }
    if let (true, Some((e, ..))) = (apply.stow, me) {
        sheath.write(crate::creature_anim::SheathRequest {
            entity: e,
            state: 0,
            ceremony: false,
        });
    }
}

/// `SetStandState(s)` (`0x5ed430`) on the active player over the committed `stand_now`. Its gate
/// ([`state::stand_state_refused`]), silent and before the packet: no change at all while
/// mounted, on a taxi, dead, channelling or casting, and no sitting while translating or
/// swimming. It covers the posture emotes too, whose `Emotes.dbc` gate has no swim bit. The flags
/// word is the live outbound one, a frame old, the `[[this+0x118]+0x40]` the cast gates read. Then
/// `CMSG_STANDSTATECHANGE` (`0x5ed501`) and the local apply (`0x5ed53f`). A state equal to the
/// committed one is dropped, each caller here having asked off its own read of the state.
#[allow(clippy::too_many_arguments)] // the setter's whole input, as `update` holds it
fn set_stand_state(
    s: u8,
    stand_now: u8,
    gates: state::StandGates,
    flags: u32,
    me: Option<SelfPostureItem>,
    net: &NetCommands,
    pending: &mut super::StandPending,
    commands: &mut Commands,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    loot_close: &mut crate::ui_loot::LootMoveStart,
) {
    if send_stand_state(s, stand_now, gates, flags, me.is_some(), net) {
        apply_locally(pending, s, stand_now, me, commands, sheath, loot_close);
    }
}

/// `SetStandState`'s gate and send (`0x5ed430`): whether `s` went out as `CMSG_STANDSTATECHANGE`,
/// not refused silently, a change from `stand_now`, and our own object `present`.
fn send_stand_state(
    s: u8,
    stand_now: u8,
    gates: state::StandGates,
    flags: u32,
    present: bool,
    net: &NetCommands,
) -> bool {
    if state::stand_state_refused(gates, flags, s) {
        debug!(
            "stand state {s} refused ({gates:?}, move flags {flags:#x} — the client's `0x5ed430` gate)"
        );
        move_trace::posture("REFUSED", s, stand_now, flags);
        return false;
    }
    // Nothing to sit before our own object streams in: the setter runs on the active player.
    if s == stand_now || !present {
        return false;
    }
    move_trace::posture("commit", s, stand_now, flags);
    let _ = net.0.send(ClientCommand::StandStateChange {
        state: u32::from(s),
    });
    true
}

/// One row of [`SelfPosture`], the active player as the posture verbs read it.
pub(super) type SelfPostureItem<'a> = (
    Entity,
    Option<(
        &'a crate::creature_anim::AnimDriver,
        &'a crate::creature_anim::CommittedSheath,
    )>,
    Option<&'a crate::net::ObjectStore>,
    bool,
    Option<&'a crate::creature_anim::Wielded>,
    bool,
    bool,
);

/// What the local apply (`0x6127b0`) does for state `s` over the committed `stand_now`, with the
/// body's weapons `drawn` or not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LocalApply {
    /// `0x5f0790` writes the prediction `[player+0x1d68]`, only on a change (`0x5f0799`).
    predict: bool,
    /// That change, to any state but 0, ends the loot window (`0x48f200(1, 1, 0)` at `0x5f07d5`,
    /// while `0x6126b0` holds) and the attack (`0x5ecac0` at `0x5f07e7`, while `0x60ecb0` holds).
    leave: bool,
    /// Any call stows a drawn weapon for a state but 0 or 2 (`0x6127cc`, `0x611cf0(0, 1, 1)`).
    stow: bool,
}

fn local_apply(s: u8, stand_now: u8, drawn: bool) -> LocalApply {
    let predict = s != stand_now;
    LocalApply {
        predict,
        leave: predict && s != 0,
        stow: drawn && s != 0 && s != 2,
    }
}

/// StopAttack (`0x5ecac0`) as the stand state's local apply calls it, on our own attack target
/// (`0x60ecb0`): `CMSG_ATTACKSTOP` and the queued strike's cancel.
fn stop_attack_for_stand_state(
    mut slots: crate::spell::CastSlots,
    net: Res<NetCommands>,
    me: Query<(Entity, Option<&crate::net::ObjectStore>), With<crate::net::SelfPlayer>>,
    edits: Res<crate::creature_anim::UnitEdits>,
    mut word: ResMut<crate::creature_anim::AttackStopInFlight>,
) {
    let own = me.iter().next();
    crate::creature_anim::stop_attack_local(
        own.map(|(e, _)| e),
        crate::tutorial::LowVitals::of(own.and_then(|(_, s)| s)),
        &edits,
        &mut word,
        &mut slots.core(&net),
    );
}

#[cfg(test)]
mod tests;
