//! **The idle handler** — `WorldFrame::Render 0x482ea0`, the one reference function that watches
//! how long it has been since the player last touched anything and does three unrelated-looking
//! things about it.
//!
//! One module because it is **one timer**, read once per frame against one global stamp, with a
//! three-way branch hung off it:
//!
//! | idle | what happens |
//! |---|---|
//! | `< 300 000 ms` | nothing at all — `0x482ed8 jl 0x482fbf` leaves the whole block |
//! | `300 000 – 1 799 999 ms` | **auto-sit** (`0x482f91 call 0x5ed430(1)`), then **auto-AFK** (`0x482fba call 0x5eb740(NULL)`) |
//! | `>= 1 800 000 ms` | print `IDLE_MESSAGE`, then `0x5ab000` — and pointedly **no** sit and **no** AFK |
//!
//! Both thresholds are read off the immediates: `0x482ecd lea ecx,[eax-0x493e0]` is `-300 000`, and
//! `0x482ede add eax,0xffe488c0` is `-1 800 000`.
//!
//! **The 30-minute leg is a CAMP, not a kick.** `0x5ab000` is `ClientServices::SendLogout(flag,
//! force)` — the *same* function the game menu's Logout button
//! reaches through `Logout 0x489390`, sending `CMSG_LOGOUT_REQUEST 0x4B`. So the server decides:
//! it refuses outright in combat, logs you out instantly in an inn, and otherwise runs the
//! twenty-second countdown that [`crate::ui_logout`] already narrates as the CAMP dialog. A
//! re-implementation that tore the socket down here would be both unfaithful and much ruder than
//! the thing it was imitating.
//!
//! **Nothing here needs a latch of its own, because every leg is gated on the state it is about to
//! create.** The sit is refused once seated, the auto-AFK is gated on the mirror `[0xb6e5cc] == 0`
//! and sets it, and the logout request is swallowed by the dispatcher's own pending byte
//! `[session+0x1b1d]`. That self-limiting shape is the reference's, and it is why a handler that
//! runs every single frame for twenty-five minutes does not spam the wire.
//!
//! The third of those did not exist here until this landed. [`crate::ui_logout`] knew about
//! `[session+0x1b1d]` in prose and never modelled it, because the only caller was a button a human
//! presses once; a caller that asks sixty times a second is what made the gap load-bearing. It is
//! built where the reference has it — in the dispatcher, covering every caller — rather than as a
//! private flag here.

use std::time::Duration;

use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseButtonInput, MouseWheel};
use bevy::prelude::*;
use bevy::time::Real;

use super::away::{afk_line, push_system, AfkMirror};
use super::feed::ChatLog;

/// **`[0xcf0bc8]` — the last-input millisecond stamp**, the one thing the idle timer subtracts
/// from.
///
/// Elapsed real time, which is the same quantity both sides of the reference's subtraction come
/// from: the stamp is written by exactly one call (`0x42d7bb`, inside the ring writer `0x42d780`)
/// to the same `0x42c010` the handler reads at `0x482ebe`. `GetMessageTime` is not imported and
/// `MSG.time` is never read — the OS message's own clock never enters this.
///
/// **Process-global, zero-initialised, and never reset** — including at world enter, which is the
/// obvious-looking thing to do and is wrong. One cell serves the login screen, character select
/// and the world alike, and it starts at zero, so a client whose first five minutes are untouched
/// is idle by the reference's own reckoning. [`stamp_input`] is ungated for the same reason.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub(crate) struct LastInput(Duration);

impl LastInput {
    /// Stamp the clock as an **unattended probe** — the one caller outside [`stamp_input`].
    ///
    /// A probe is a client with nobody at the keyboard, and this timer is the one place where that
    /// is not a harmless difference: a probe that stands still in a battleground goes AFK at five
    /// minutes exactly as the reference would, and vmangos then removes an AFK player from the
    /// battleground outright (`Player::ToggleAFK` → `LeaveBattleground`). Two faithful behaviours,
    /// one on each side, combining to eject an instrument before a match can end — which is what
    /// `capture::probe_bg`'s first long run found, at t=300 s, with the census reading Stormwind.
    ///
    /// So a probe that means to stand in for a **present** player says so here, rather than the
    /// alternative dodges: turning the idle handler off (it would stop testing the thing the
    /// reference does), or having the probe jiggle the mouse (a synthetic input event to fool our
    /// own dispatcher, which is a lie told one layer lower down and harder to see).
    pub(crate) fn stamp_present(&mut self, now: Duration) {
        self.0 = now;
    }
}

/// **5 minutes** — `0x482ecd lea ecx,[eax-0x493e0]`, `0x493e0` = 300 000 ms.
const AUTO_AFK_AFTER: Duration = Duration::from_millis(300_000);

/// **30 minutes** — `0x482ede add eax,0xffe488c0`, i.e. `eax - 1 800 000`.
///
/// **Absolute, not five-plus-thirty.** The subtraction above it writes `ecx`, so `eax` still holds
/// the raw elapsed when this one runs. Reading the pair as two chained deductions gives 35 minutes
/// and is the natural misreading of the two-instruction sequence.
const AUTO_LOGOUT_AFTER: Duration = Duration::from_millis(1_800_000);

/// Stamp the clock — the **seven** writers of `[0xcf0bc8]`, all registered on the one input ring
/// at priority `1.0f` (`0x765c04`–`0x765cfe` via `0x41fca0`), all storing the same ring field
/// `payload[3]` (a closed image-wide census of the dword: 37 occurrences, 7 W / 30 R).
///
/// | writer | bus category | what the player did |
/// |---|---|---|
/// | `0x765f34` | 8 | a key went **down** — first press only |
/// | `0x765fec` | 9 | a key came **up** |
/// | `0x7662fb` | 0xb | a mouse button went **down** |
/// | `0x766152` | 0xc | the mouse **moved** (absolute, filtered) |
/// | `0x766291` | 0xd | the mouse **moved** (delta) |
/// | `0x766461` | 0xe | a mouse button came **up** |
/// | `0x76651f` | 0x10 | the **wheel** turned |
///
/// **The keyboard and the mouse follow opposite rules, and there is no single sentence covering
/// both.** A held key stamps once; a moved mouse stamps continuously. What does *not* stamp:
///
/// - **Auto-repeat.** `0x424810` promotes category 8 to 0xa at `0x4248b3` when the key is already
///   held, and `0x766070` (category 0xa) has no store. So **holding a key is idle** — running with
///   W held, or on autorun, reaches the five-minute auto-AFK. This is the single most surprising
///   thing the reference does and it is deliberate here, not an omission.
/// - Zero-motion moves and the pump's synthetic hover re-pick (`0x7660d0`'s `0x766122`–`0x766140`
///   filter, and its `0x766120` bypass).
/// - Double-clicks, `WM_MOUSEHWHEEL`, `WM_CHAR`, the joystick, and any VK outside `0x42d800`'s
///   table.
/// - Mouse **movement and left-button-up during an OS window drag** — a title-bar drag or border
///   resize, mirrored from `SetCapture`/`ReleaseCapture` in `[0x884e64]`. We inherit that for free:
///   an OS-driven window drag delivers no motion to the app at all.
///
/// **Mouselook is NOT an exclusion** — it is `[0x884e5c]`, a different global entirely, and all it
/// picks is which arm enqueues (absolute → category 0xc, delta → category 0xd, both stamping). A
/// player turning the camera with the right button held is *not* idle. This module briefly recorded
/// the opposite, off a mislabelled global; `[0x884e64]`'s writers are `SetCapture`'s.
///
/// **The store sits on the raw input bus, ahead of dispatch**, so a keystroke a frame *consumes*
/// still stamps: typing into the chat edit box is input, and so is a click that only moves a
/// window. That is why this reads the raw messages rather than [`crate::bindings::BindingsState`],
/// which is the post-dispatch view and would let a player who is busy typing go AFK mid-sentence.
///
/// Runs **outside** the in-world gate the handler itself carries, because the stamp is
/// process-global: character select, the login screen and the world all write the one cell.
pub(crate) fn stamp_input(
    time: Res<Time<Real>>,
    mut last: ResMut<LastInput>,
    mut keys: MessageReader<KeyboardInput>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut wheel: MessageReader<MouseWheel>,
    motion: Res<AccumulatedMouseMotion>,
) {
    // `count()` rather than `next().is_some()`, and bitwise `|` rather than `||`: both are about
    // draining. A reader that stops at the first message leaves the rest on its cursor and re-sees
    // them next frame, and a short-circuited `||` never touches the later queues at all — either
    // one keeps the clock warm for frames after the input actually stopped.
    // `repeat` is category 0xa, which has no store — a held key stops counting after its first
    // press. (`Released` never carries the flag, so this one filter covers both edges.)
    let any = (keys.read().filter(|k| !k.repeat).count() != 0)
        | (buttons.read().count() != 0)
        | (wheel.read().count() != 0)
        | (motion.delta != Vec2::ZERO);
    if any {
        last.0 = time.elapsed();
    }
}

/// **What the timer decides this frame** — a value rather than three writes, for the reason
/// [`super::away::AwayOutcome`] is one: the whole content of `0x482ea0`'s block is which legs move
/// together and which are mutually exclusive, and that is exactly what a re-implementer gets wrong
/// by treating "idle" as a single state with three effects.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdleAction {
    /// `0x482f91 call 0x5ed430(1)`.
    pub(crate) sit: bool,
    /// `0x482fba call 0x5eb740(NULL)`.
    pub(crate) afk: bool,
    /// `0x482f1a` + `0x5ab000` — `IDLE_MESSAGE` and the logout request.
    pub(crate) camp: bool,
}

/// The state the two five-minute legs are gated on. Read off our own descriptor; each field is one
/// test in the reference's block, and no two of them share a gate.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdleGates {
    /// `UNIT_FIELD_BYTES_1` byte 0. The sit wants `0`.
    pub(crate) stand_state: u8,
    /// `UNIT_FIELD_FLAGS` bit 19 (`0x80000`) — the sit's.
    pub(crate) in_combat: bool,
    /// `UNIT_FIELD_MOUNTDISPLAYID > 0` — the sit's.
    pub(crate) mounted: bool,
    /// The sit's fourth gate, `0x482f84 call 0x6103a0` / `jne` past the setter — which requires the
    /// predicate **false**, and for a solo player reduces to `[0xc4d888] == 0xc`, its normal
    /// in-world value.
    ///
    /// **Exactly one thing benilla has puts it elsewhere.** The global's writer census is closed —
    /// 25 dword occurrences, 4 writes, three of them the literal `0xc` — so every non-`0xc` value
    /// comes through the one `StartMoveAction 0x611130`, whose eleven kinds are the click-to-move
    /// family (4, 8), the move-to-interact family (5, 6, 7, 9, 0xa) and **`3` = `/follow`**. Mode 3
    /// is co-extensive with follow being active, proven twice over: `0x489e00` is `FollowUnit`, and
    /// `AUTOFOLLOW_BEGIN`/`AUTOFOLLOW_END` (367/368) have exactly one fire site each, the second
    /// gated on `[0xc4d888] == 3`.
    ///
    /// **Autorun never touches the global** — no writer lies in the movement-command band at all;
    /// the only traffic runs the other way (`0x5146d6` *calls the cancel*). So this gate is not a
    /// stand-in for "moving", and it is not redundant with `stand_state_refused`: a follow whose
    /// target has stopped walking is motionless, passes every movement test, and is still the one
    /// player the reference will not seat.
    pub(crate) following: bool,
    /// `UNIT_FIELD_FLAGS` bit 20 (`0x100000`) — the **AFK's**, not the sit's.
    pub(crate) on_taxi: bool,
    /// The optimistic mirror `[0xb6e5cc]` — the AFK's second gate, and the anti-repeat.
    pub(crate) afk: bool,
}

/// Resolve the timer against the gates — `0x482ea0`'s block, byte for byte.
///
/// **The two bands are exclusive, and that is the easy thing to get wrong.** `0x482ee5 jl
/// 0x482f39` is what jumps *into* the 5–30 minute block, so reaching thirty minutes takes the
/// other arm entirely: the camp leg neither sits you nor marks you AFK. A client that let the
/// thirty-minute case fall through the five-minute one would sit a player down on the same frame
/// it asked to log them out.
///
/// **Nothing latches.** Every leg is gated on the state it is about to create — the sit on
/// `stand_state == 0`, the AFK on the mirror it sets, and the camp on the logout request's own
/// pending latch downstream — so a handler that runs every frame for twenty-five minutes emits
/// exactly one of each.
pub(crate) fn idle_action(idle: Duration, gates: IdleGates) -> IdleAction {
    if idle < AUTO_AFK_AFTER {
        // `0x482ed8 jl 0x482fbf` — the whole block, all three legs, sits below this.
        return IdleAction::default();
    }
    if idle >= AUTO_LOGOUT_AFTER {
        return IdleAction {
            camp: true,
            ..IdleAction::default()
        };
    }
    IdleAction {
        sit: gates.stand_state == 0 && !gates.in_combat && !gates.mounted && !gates.following,
        afk: !gates.on_taxi && !gates.afk,
        camp: false,
    }
}

/// The handler proper. In-world only: `0x482ea0` is `WorldFrame::Render`, so there is no idle
/// timer at the glue screens.
pub(crate) fn idle_handler(
    time: Res<Time<Real>>,
    last: Res<LastInput>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    self_q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    mut mirror: ResMut<AfkMirror>,
    mut chat: ResMut<ChatLog>,
    commands: Res<crate::net::NetCommands>,
    mut stand: MessageWriter<crate::player::StandStateRequest>,
    follow: Res<crate::player::FollowState>,
) {
    let idle = time.elapsed().saturating_sub(last.0);
    if idle < AUTO_AFK_AFTER {
        return;
    }
    let Some(mut script) = script else {
        return;
    };
    let gates = self_q.iter().next().map(|store| {
        let flags = store.0.unit_flags();
        IdleGates {
            stand_state: store.0.unit_stand_state(),
            in_combat: flags & crate::player::UNIT_FLAG_IN_COMBAT != 0,
            mounted: store.0.unit_mount_display_id() != 0,
            on_taxi: flags & crate::player::UNIT_FLAG_TAXI_FLIGHT != 0,
            afk: mirror.is_afk(),
            following: follow.guid.is_some(),
        }
    });
    let action = match gates {
        Some(g) => idle_action(idle, g),
        // No body streamed yet is not "every gate clear": the reference resolves the local player
        // (`0x468550` → `0x468460`) inside the block, so the sit and the AFK have nothing to read.
        // The camp leg never touches the player object, and survives.
        None => IdleAction {
            camp: idle >= AUTO_LOGOUT_AFTER,
            ..IdleAction::default()
        },
    };

    if action.sit {
        // Through the ONE setter, like the reference's own call: `player::posture` runs
        // `stand_state_refused` on the way (so an idle *swimmer* is not seated, bug B155) and
        // drops a request equal to the state it already holds, consulting its own `stand_pending`
        // rather than the descriptor — which is what makes re-asking every frame across the server
        // round trip produce exactly one `CMSG_STANDSTATECHANGE`.
        stand.write(crate::player::StandStateRequest { state: 1 });
    }

    let strings = |key: &str| crate::ui_chat::combat::global_string(&script, key);
    if action.afk {
        // `SetAFK(NULL)` is `/afk` with an empty message and the mirror clear — the one row of the
        // `/afk` truth table this can reach, which is why it goes through the same function rather
        // than composing its own line: same `MARKED_AFK_MESSAGE % DEFAULT_AFK_MESSAGE` echo, the
        // same client-side default substitution reaching the wire, the same optimistic mirror.
        let out = afk_line("", *mirror, &strings);
        if let Some(line) = out.line {
            push_system(&mut chat, line);
        }
        if let Some(v) = out.mirror {
            mirror.0 = v;
        }
        let _ = commands.0.send(crate::net::ClientCommand::Chat {
            kind: crate::net::ChatKind::Afk,
            target: None,
            text: out.body,
        });
    }
    if action.camp {
        if let Some(line) = strings("IDLE_MESSAGE") {
            push_system(&mut chat, line);
        }
        // `0x5ab000` with the game menu's own arguments: `CMSG_LOGOUT_REQUEST`, the server's
        // clock, the CAMP dialog. `crate::ui_logout` owns everything from here.
        script.queue_session_request(benilla_ui::script::SessionRequest::Logout);
    }
}

#[cfg(test)]
mod tests {
    use benilla_protocol::ObjectFields;
    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::net::{ClientCommand, NetCommands, ObjectStore, SelfPlayer};

    /// Descriptor indices, spelled here so a fixture reads like the field it sets.
    const UNIT_FLAGS: u16 = 46;
    const MOUNT_DISPLAY_ID: u16 = 133;
    const BYTES_1: u16 = 138;

    const FIVE_MIN: Duration = Duration::from_millis(300_000);
    const THIRTY_MIN: Duration = Duration::from_millis(1_800_000);

    /// A body that is standing, out of combat, unmounted, off a taxi and not yet AFK — every gate
    /// open, so a test that flips one field is testing exactly that field.
    fn open() -> IdleGates {
        IdleGates::default()
    }

    /// **The two thresholds, at the millisecond.** `0x493e0` = 300 000 and `0xffe488c0` =
    /// −1 800 000, both read off the immediates rather than off "5" and "30".
    #[test]
    fn the_thresholds_are_the_immediates_not_round_numbers() {
        let one_ms = Duration::from_millis(1);
        assert_eq!(
            idle_action(FIVE_MIN - one_ms, open()),
            IdleAction::default(),
            "4:59.999 does nothing — `0x482ed8 jl 0x482fbf` leaves the whole block"
        );
        assert_eq!(
            idle_action(FIVE_MIN, open()),
            IdleAction {
                sit: true,
                afk: true,
                camp: false
            },
            "300 000 ms exactly is INSIDE the band: the jcc is `jl`, not `jle`"
        );
        assert_eq!(
            idle_action(THIRTY_MIN - one_ms, open()),
            IdleAction {
                sit: true,
                afk: true,
                camp: false
            },
        );
        assert_eq!(
            idle_action(THIRTY_MIN, open()),
            IdleAction {
                camp: true,
                ..IdleAction::default()
            },
            "1 800 000 ms exactly takes the camp arm, again on `jl`"
        );
    }

    /// **The bands are exclusive** — the thirty-minute leg neither sits nor marks, because
    /// `0x482ee5 jl 0x482f39` is what jumps *into* the five-minute block. A fall-through would
    /// seat a player on the frame it asked to log them out.
    #[test]
    fn the_camp_leg_does_not_also_sit_or_mark_afk() {
        for idle in [THIRTY_MIN, THIRTY_MIN + Duration::from_secs(3600)] {
            assert_eq!(
                idle_action(idle, open()),
                IdleAction {
                    camp: true,
                    sit: false,
                    afk: false
                },
            );
        }
    }

    /// **Each gate refuses its own leg and only its own.** The sit's three and the AFK's two are
    /// different tests at different addresses on different fields; the one thing a re-implementer
    /// is tempted to do is fold them into a single "can I go idle" predicate.
    #[test]
    fn the_gates_are_per_leg_never_shared() {
        let cases: [(&str, IdleGates, IdleAction); 7] = [
            (
                "in combat: no sit — but the AFK mark is NOT gated on combat",
                IdleGates {
                    in_combat: true,
                    ..open()
                },
                IdleAction {
                    sit: false,
                    afk: true,
                    camp: false,
                },
            ),
            (
                "mounted: no sit, same asymmetry",
                IdleGates {
                    mounted: true,
                    ..open()
                },
                IdleAction {
                    sit: false,
                    afk: true,
                    camp: false,
                },
            ),
            (
                "already seated: nothing to ask for, and the AFK is untouched",
                IdleGates {
                    stand_state: 1,
                    ..open()
                },
                IdleAction {
                    sit: false,
                    afk: true,
                    camp: false,
                },
            ),
            (
                "on a taxi: no AFK — and the sit is NOT gated on the taxi bit here (its own \
                 `0x5ed430` guard #3 is what covers that downstream)",
                IdleGates {
                    on_taxi: true,
                    ..open()
                },
                IdleAction {
                    sit: true,
                    afk: false,
                    camp: false,
                },
            ),
            (
                "following someone who has stopped walking: no sit — this is the ONLY case the \
                 fourth gate does work nothing else does, since anything else that leaves \
                 `[0xc4d888]` non-`0xc` implies translating, which `stand_state_refused` refuses \
                 downstream. The AFK mark is untouched: it is not one of the sit's gates",
                IdleGates {
                    following: true,
                    ..open()
                },
                IdleAction {
                    sit: false,
                    afk: true,
                    camp: false,
                },
            ),
            (
                "already AFK: the anti-repeat, and it does not suppress the sit",
                IdleGates {
                    afk: true,
                    ..open()
                },
                IdleAction {
                    sit: true,
                    afk: false,
                    camp: false,
                },
            ),
            (
                "seated AND already AFK — the steady state five minutes in: nothing more happens, \
                 every frame, for the next twenty-five minutes",
                IdleGates {
                    stand_state: 1,
                    afk: true,
                    ..open()
                },
                IdleAction::default(),
            ),
        ];
        for (why, gates, want) in cases {
            assert_eq!(idle_action(FIVE_MIN, gates), want, "{why}");
        }
    }

    /// The steady state is what makes the missing latch safe: run the decision a thousand times
    /// with the state each leg creates fed back in, and exactly one sit and one mark come out.
    #[test]
    fn the_handler_is_self_limiting_without_any_latch() {
        let mut gates = open();
        let (mut sits, mut marks) = (0, 0);
        for frame in 0..1000 {
            let a = idle_action(FIVE_MIN + Duration::from_millis(frame), gates);
            if a.sit {
                sits += 1;
                gates.stand_state = 1; // the server's echo of our own CMSG_STANDSTATECHANGE
            }
            if a.afk {
                marks += 1;
                gates.afk = true; // `SetAFK` writes the mirror `[0xb6e5cc]` it is gated on
            }
        }
        assert_eq!((sits, marks), (1, 1));
    }

    /// **The seven writers stamp; auto-repeat does not.** The reference's store sits on the raw
    /// message bus ahead of dispatch, so this reads the raw Bevy messages rather than
    /// [`crate::bindings::BindingsState`] — a keystroke the chat edit box swallows is still a
    /// keystroke, and a player mid-sentence must not be marked away.
    #[test]
    fn every_writer_stamps_and_auto_repeat_pointedly_does_not() {
        use bevy::input::keyboard::{Key, KeyboardInput};
        use bevy::input::mouse::{MouseButtonInput, MouseScrollUnit};
        use bevy::input::ButtonState;

        let stamped = |seed: &dyn Fn(&mut App)| {
            let mut app = App::new();
            app.init_resource::<Time<Real>>()
                .init_resource::<LastInput>()
                .init_resource::<AccumulatedMouseMotion>()
                .add_message::<KeyboardInput>()
                .add_message::<MouseButtonInput>()
                .add_message::<MouseWheel>();
            app.world_mut()
                .resource_mut::<Time<Real>>()
                .advance_by(Duration::from_millis(1234));
            seed(&mut app);
            app.world_mut().run_system_once(stamp_input).unwrap();
            app.world().resource::<LastInput>().0
        };

        let idle = stamped(&|_| {});
        assert_eq!(
            idle,
            Duration::ZERO,
            "a frame with no input leaves it alone"
        );

        let touched = Duration::from_millis(1234);
        assert_eq!(
            stamped(&|app| {
                app.world_mut().write_message(KeyboardInput {
                    key_code: KeyCode::KeyW,
                    logical_key: Key::Character("w".into()),
                    state: ButtonState::Pressed,
                    text: None,
                    repeat: false,
                    window: Entity::PLACEHOLDER,
                });
            }),
            touched,
            "a key press"
        );
        assert_eq!(
            stamped(&|app| {
                app.world_mut().write_message(KeyboardInput {
                    key_code: KeyCode::KeyW,
                    logical_key: Key::Character("w".into()),
                    state: ButtonState::Released,
                    text: None,
                    repeat: false,
                    window: Entity::PLACEHOLDER,
                });
            }),
            touched,
            "and a key RELEASE — letting go of a key is input too"
        );
        assert_eq!(
            stamped(&|app| {
                app.world_mut().write_message(MouseButtonInput {
                    button: MouseButton::Left,
                    state: ButtonState::Pressed,
                    window: Entity::PLACEHOLDER,
                });
            }),
            touched,
            "a mouse button"
        );
        assert_eq!(
            stamped(&|app| {
                app.world_mut().write_message(MouseWheel {
                    unit: MouseScrollUnit::Line,
                    x: 0.0,
                    y: 1.0,
                    window: Entity::PLACEHOLDER,
                });
            }),
            touched,
            "the wheel"
        );
        assert_eq!(
            stamped(&|app| {
                app.world_mut()
                    .resource_mut::<AccumulatedMouseMotion>()
                    .delta = Vec2::new(0.0, 1.0);
            }),
            touched,
            "and bare mouse movement, with no button held"
        );

        // **The one that is not an omission.** `0x424810` promotes a held key from bus category 8
        // to 0xa, and `0x766070` — category 0xa's handler — has no store. So running with W held,
        // or sitting on autorun, walks straight into the five-minute auto-AFK. It is the single
        // most surprising thing the reference does, and a re-implementation
        // that "fixed" it would diverge from the reference in a way a player would notice.
        assert_eq!(
            stamped(&|app| {
                app.world_mut().write_message(KeyboardInput {
                    key_code: KeyCode::KeyW,
                    logical_key: Key::Character("w".into()),
                    state: ButtonState::Pressed,
                    text: None,
                    repeat: true,
                    window: Entity::PLACEHOLDER,
                });
            }),
            Duration::ZERO,
            "an auto-repeat is category 0xa, which has no store — holding a key is IDLE"
        );
    }

    /// A world with a local player, a live chat log and a wire we can read back.
    fn world(
        idle_ms: u64,
        fields: &[(u16, u32)],
    ) -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<AfkMirror>()
            .init_resource::<ChatLog>()
            .init_resource::<LastInput>()
            .init_resource::<crate::player::FollowState>()
            .add_message::<crate::player::StandStateRequest>()
            .insert_resource(NetCommands(tx));
        // `LastInput` stays at zero and the clock walks forward, so "idle" is just the elapsed.
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(idle_ms));
        app.world_mut()
            .spawn((SelfPlayer, ObjectStore(ObjectFields::from_pairs(fields))));

        let script = benilla_ui::script::UiScript::new().unwrap();
        // The three GlobalStrings this handler reaches for, exactly as the install ships them.
        script
            .run(
                "MARKED_AFK_MESSAGE = \"You are now AFK: %s\"\n\
                 DEFAULT_AFK_MESSAGE = \"Away from Keyboard\"\n\
                 IDLE_MESSAGE = \"You have been inactive for some time and will be logged \
                 out of the game. If you wish to remain logged in, hit the cancel \
                 button.\"",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        (app, rx)
    }

    fn lines(app: &App) -> Vec<String> {
        app.world().resource::<ChatLog>().pending_lines()
    }

    /// **The whole five-minute arm, through the real systems.** The sit is a
    /// [`crate::player::StandStateRequest`] (so it inherits `stand_state_refused`), the mark is the
    /// `/afk` line and the `/afk` packet, and the mirror leads the descriptor exactly as a typed
    /// `/afk` would.
    #[test]
    fn five_minutes_idle_sits_you_down_and_marks_you_afk() {
        let (mut app, rx) = world(300_000, &[]);
        app.world_mut().run_system_once(idle_handler).unwrap();

        assert_eq!(lines(&app), vec!["You are now AFK: Away from Keyboard"]);
        assert!(app.world().resource::<AfkMirror>().is_afk());
        // The CLIENT's default substitution reaches the wire, never an empty body — the server
        // stores this string as the character's auto-reply.
        let sent = rx.try_iter().collect::<Vec<_>>();
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::Chat { kind: crate::net::ChatKind::Afk, target: None, text }]
                    if text == "Away from Keyboard"
            ),
            "{sent:?}"
        );
        let sat = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>()
            .drain()
            .map(|r| r.state)
            .collect::<Vec<_>>();
        assert_eq!(sat, vec![1], "one sit, through the one setter");
    }

    /// Four minutes fifty-nine is not idle. Nothing at all — no line, no packet, no posture.
    #[test]
    fn just_under_five_minutes_is_completely_silent() {
        let (mut app, rx) = world(299_999, &[]);
        app.world_mut().run_system_once(idle_handler).unwrap();
        assert!(lines(&app).is_empty());
        assert!(!app.world().resource::<AfkMirror>().is_afk());
        assert!(rx.try_iter().next().is_none());
    }

    /// **The thirty-minute arm is a CAMP, and it is not an AFK.** `0x5ab000` is
    /// `ClientServices::SendLogout`, the same function the game menu's Logout button reaches, so
    /// what comes out here is a [`benilla_ui::script::SessionRequest::Logout`] and nothing else:
    /// no `/afk` line, no `/afk` packet, no sit.
    #[test]
    fn thirty_minutes_idle_camps_instead_of_marking_afk() {
        let (mut app, rx) = world(1_800_000, &[]);
        app.world_mut().run_system_once(idle_handler).unwrap();

        assert_eq!(
            lines(&app),
            vec![
                "You have been inactive for some time and will be logged out of the game. If you \
                 wish to remain logged in, hit the cancel button."
            ],
            "IDLE_MESSAGE goes out unformatted — `0x482f1a` hands `GetText`'s result straight to \
             the chat sink, with no snprintf in between. It carries no format slot, and the \
             sentence it does carry (\"hit the cancel button\") is the CAMP dialog's, which is \
             the string's own confirmation that this leg is a logout REQUEST and not a kick."
        );
        assert!(
            !app.world().resource::<AfkMirror>().is_afk(),
            "the camp leg pointedly does NOT mark AFK"
        );
        assert!(rx.try_iter().next().is_none(), "and sends no chat packet");
        assert!(
            app.world_mut()
                .non_send_resource_mut::<benilla_ui::script::UiScript>()
                .take_session_requests()
                == vec![benilla_ui::script::SessionRequest::Logout],
        );
        assert!(app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>()
            .drain()
            .next()
            .is_none());
    }

    /// The descriptor is what the gates read — a body in combat is not seated, and one on a taxi
    /// is not marked. Driven through the real system so the field decode is under test too.
    #[test]
    fn the_descriptor_bits_reach_the_gates() {
        // In combat (`UNIT_FIELD_FLAGS` bit 19): marked, not seated.
        let (mut app, _rx) = world(300_000, &[(UNIT_FLAGS, 0x0008_0000)]);
        app.world_mut().run_system_once(idle_handler).unwrap();
        assert!(app.world().resource::<AfkMirror>().is_afk());
        assert!(app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>()
            .drain()
            .next()
            .is_none());

        // On a taxi (bit 20): seated, not marked.
        let (mut app, rx) = world(300_000, &[(UNIT_FLAGS, 0x0010_0000)]);
        app.world_mut().run_system_once(idle_handler).unwrap();
        assert!(!app.world().resource::<AfkMirror>().is_afk());
        assert!(rx.try_iter().next().is_none());
        assert_eq!(
            app.world_mut()
                .resource_mut::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>()
                .drain()
                .count(),
            1
        );

        // Mounted: marked, not seated.
        let (mut app, _rx) = world(300_000, &[(MOUNT_DISPLAY_ID, 6080)]);
        app.world_mut().run_system_once(idle_handler).unwrap();
        assert!(app.world().resource::<AfkMirror>().is_afk());
        assert!(app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>()
            .drain()
            .next()
            .is_none());

        // Already sitting (`UNIT_FIELD_BYTES_1` byte 0 = 1): marked, no second ask.
        let (mut app, _rx) = world(300_000, &[(BYTES_1, 1)]);
        app.world_mut().run_system_once(idle_handler).unwrap();
        assert!(app.world().resource::<AfkMirror>().is_afk());
        assert!(app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>()
            .drain()
            .next()
            .is_none());
    }
}
