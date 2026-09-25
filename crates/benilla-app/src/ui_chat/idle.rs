//! The idle handler, the reference's `WorldFrame::Render 0x482ea0`: from 5 minutes without input
//! it sits the player (`0x482f91`) and marks them AFK (`0x482fba`); from 30 minutes it prints
//! `IDLE_MESSAGE` and requests a logout (`0x5ab000`) instead, with no sit and no AFK.
//!
//! The logout is a camp, not a kick: `0x5ab000` sends the game menu's `CMSG_LOGOUT_REQUEST`, which
//! the server refuses in combat, grants at once when resting, or counts down for 20 seconds
//! (vmangos `MiscHandler.cpp:284-339`). The reference skips the whole 30-minute leg, line
//! included, while the logout-pending byte `[session+0x1b1d]` is set (`0x482eee`); here
//! [`crate::ui_logout`] holds that byte and drops only the repeated request, so `IDLE_MESSAGE`
//! prints again every frame.

use std::time::Duration;

use bevy::input::keyboard::KeyboardInput;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseButtonInput, MouseWheel};
use bevy::prelude::*;
use bevy::time::Real;

use super::away::{afk_line, push_system, AfkMirror};
use super::feed::ChatLog;

/// `[0xcf0bc8]`, the last-input stamp: elapsed real time on the clock (`0x42c010`) both its one
/// writer (`0x42d7bb`) and the handler (`0x482ebe`) read. Process-global, zero at start and never
/// reset, not even at world entry, so a client untouched from launch is idle at five minutes.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub(crate) struct LastInput(Duration);

impl LastInput {
    /// Stamp the clock for an unattended probe standing in for a present player, as vmangos
    /// removes an AFK player from a battleground (`Player.cpp:1740-1742`).
    pub(crate) fn stamp_present(&mut self, now: Duration) {
        self.0 = now;
    }
}

/// 5 minutes: `0x482ecd lea ecx,[eax-0x493e0]`, `0x493e0` = 300 000 ms.
const AUTO_AFK_AFTER: Duration = Duration::from_millis(300_000);

/// 30 minutes from the last input, not 5 + 30: `0x482ede add eax,0xffe488c0` subtracts from the
/// raw elapsed, since the 5-minute subtraction wrote `ecx`.
const AUTO_LOGOUT_AFTER: Duration = Duration::from_millis(1_800_000);

/// Stamp the clock on raw input, as the reference's seven writers of `[0xcf0bc8]` do: key down,
/// first press only (`0x765f34`), key up (`0x765fec`), button down (`0x7662fb`) and up
/// (`0x766461`), motion (`0x766152`, `0x766291`, mouselook included) and the wheel (`0x76651f`).
/// Holding a key is idle: auto-repeat is category 0xa (`0x424810`), whose handler `0x766070` has
/// no store, so running with W held reaches the auto-AFK. A zero move does not stamp (`0x7660d0`).
/// A horizontal wheel stamps here; the reference never translates `WM_MOUSEHWHEEL`.
///
/// The store sits ahead of dispatch, so a keystroke the chat edit box consumes still stamps: this
/// reads the raw messages, not [`crate::bindings::BindingsState`], outside the in-world gate.
pub(crate) fn stamp_input(
    time: Res<Time<Real>>,
    mut last: ResMut<LastInput>,
    mut keys: MessageReader<KeyboardInput>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut wheel: MessageReader<MouseWheel>,
    motion: Res<AccumulatedMouseMotion>,
) {
    // `count()` and a bitwise `|` drain every reader, or unread messages warm the clock next frame.
    // `repeat` is category 0xa, which has no store; `Released` never carries the flag.
    let any = (keys.read().filter(|k| !k.repeat).count() != 0)
        | (buttons.read().count() != 0)
        | (wheel.read().count() != 0)
        | (motion.delta != Vec2::ZERO);
    if any {
        last.0 = time.elapsed();
    }
}

/// What the timer decides this frame, one flag per leg.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdleAction {
    /// `0x482f91 call 0x5ed430(1)`.
    pub(crate) sit: bool,
    /// `0x482fba call 0x5eb740(NULL)`.
    pub(crate) afk: bool,
    /// `0x482f1a` and `0x5ab000`: `IDLE_MESSAGE` and the logout request.
    pub(crate) camp: bool,
}

/// The gates of the five-minute legs, read off our descriptor; no two legs share one.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdleGates {
    /// `UNIT_FIELD_BYTES_1` byte 0; the sit wants `0`.
    pub(crate) stand_state: u8,
    /// `UNIT_FIELD_FLAGS` bit 19 (`0x80000`), the sit's.
    pub(crate) in_combat: bool,
    /// `UNIT_FIELD_MOUNTDISPLAYID > 0`, the sit's.
    pub(crate) mounted: bool,
    /// The sit's fourth gate (`0x482f84 call 0x6103a0`): for a solo player, the movement-action
    /// mode `[0xc4d888]` at its in-world `0xc`. `/follow` sets 3 (`FollowUnit 0x489e00`) and
    /// autorun never moves it, so a follower whose target has stopped is not seated.
    pub(crate) following: bool,
    /// `UNIT_FIELD_FLAGS` bit 20 (`0x100000`), the AFK's, not the sit's.
    pub(crate) on_taxi: bool,
    /// The optimistic mirror `[0xb6e5cc]`: the AFK's second gate, and its anti-repeat.
    pub(crate) afk: bool,
}

/// Resolve the timer against the gates. The bands are exclusive (`0x482ee5 jl 0x482f39`): the
/// thirty-minute leg neither sits nor marks AFK.
pub(crate) fn idle_action(idle: Duration, gates: IdleGates) -> IdleAction {
    if idle < AUTO_AFK_AFTER {
        // `0x482ed8 jl 0x482fbf`: all three legs sit below this.
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

/// The handler, in-world only: `0x482ea0` is `WorldFrame::Render`.
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
        // No body yet: the reference resolves the local player inside the block (`0x468550`,
        // `0x468460`), so the sit and the AFK have nothing to read; the camp leg never reads it.
        None => IdleAction {
            camp: idle >= AUTO_LOGOUT_AFTER,
            ..IdleAction::default()
        },
    };

    if action.sit {
        // Through the one setter, like the reference: `player::posture` refuses a swimmer and
        // drops a request equal to its pending state, so one `CMSG_STANDSTATECHANGE` goes out.
        stand.write(crate::player::StandStateRequest { state: 1 });
    }

    let strings = |key: &str| crate::ui_chat::combat::global_string(&script, key);
    if action.afk {
        // `SetAFK(NULL)` is `/afk` with an empty message and a clear mirror, so it shares that
        // path: the `DEFAULT_AFK_MESSAGE` echo and body, and the optimistic mirror.
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
        // `0x5ab000` with the game menu's arguments; `crate::ui_logout` owns the rest.
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

    /// Every gate open, so a test that flips one field tests that field.
    fn open() -> IdleGates {
        IdleGates::default()
    }

    /// `0x493e0` = 300 000 and `0xffe488c0` = -1 800 000, read off the immediates.
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

    /// A thousand frames with each leg's state fed back in: exactly one sit and one mark.
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

        // Auto-repeat is category 0xa, whose handler `0x766070` has no store.
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

    /// The sit is a [`crate::player::StandStateRequest`]; the mark, the `/afk` line and packet.
    #[test]
    fn five_minutes_idle_sits_you_down_and_marks_you_afk() {
        let (mut app, rx) = world(300_000, &[]);
        app.world_mut().run_system_once(idle_handler).unwrap();

        assert_eq!(lines(&app), vec!["You are now AFK: Away from Keyboard"]);
        assert!(app.world().resource::<AfkMirror>().is_afk());
        // The client's default reaches the wire, never an empty body: it is the auto-reply.
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

    #[test]
    fn just_under_five_minutes_is_completely_silent() {
        let (mut app, rx) = world(299_999, &[]);
        app.world_mut().run_system_once(idle_handler).unwrap();
        assert!(lines(&app).is_empty());
        assert!(!app.world().resource::<AfkMirror>().is_afk());
        assert!(rx.try_iter().next().is_none());
    }

    /// `0x5ab000` is the game menu's Logout: a session request and nothing else.
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

    /// Through the real system, so the descriptor decode is under test too.
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
