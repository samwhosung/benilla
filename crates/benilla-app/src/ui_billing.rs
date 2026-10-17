//! The play-time warning. Every world entry arms a deadline from the connection's billing minutes
//! (`CGGameUI::InitializeGame`, `0x48f636`-`0x48f679`), and each frame of the in-game interface
//! polls it (`CGGameUI::Update`, `0x495679`-`0x495826`): `BILLING_NAG_DIALOG` with the minutes left
//! once they are 30 or fewer, and, while they are, a `BILLING_NAG_WARNING` system line on the second
//! frame and then at each 5-minute mark, each minute from 5. An Internet Gaming Room (IGR) plan gets
//! `IGR_BILLING_NAG_DIALOG` once, at 600 or fewer, and no line.

use benilla_protocol::messages::Billing;
use benilla_ui::script::{ScriptValue, UiScript};
use bevy::prelude::*;

use crate::net::SessionBilling;
use crate::ui_chat::{ChatEvent, ChatEventKind, ChatLog};
use crate::ui_script::UiFeed;

/// The longest time left that arms the warning, 72 hours in minutes (`0x48f647`).
const ARM_MAX_MINUTES: u32 = 4_320;
/// A minute in the clock's milliseconds (`imul esi,esi,0xea60`, `0x48f666`).
const MINUTE_MS: u32 = 60_000;
/// The line cadence above five minutes left (`0x4957f6`).
const LINE_STEP_MS: u32 = 300_000;
/// The poller's milliseconds-to-minutes factor, the `f32` at `0x804590` (`0x378bcf65`), a hair
/// over 1/60000: a whole minute left reads as two.
const MINUTES_PER_MS: f32 = 1.666_666_7e-5;

/// The three words the poller keeps: the deadline (`0xb4e360`, 0 unarmed), the next line's time
/// (`0xb4e364`, 0 before the first poll) and the dialog's latch (`0xb4e368`). A `ReloadUI` keeps
/// them, as it reaches neither `InitializeGame` nor these words.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub(crate) struct PlayTimeWarning {
    deadline: u32,
    next_line: u32,
    dialog_shown: bool,
}

/// What one poll shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Nag {
    /// `BILLING_NAG_DIALOG` with the minutes left (`0x495700`, `0x49581e`).
    Dialog(u32),
    /// `IGR_BILLING_NAG_DIALOG` (`0x4956e9`, `0x495741`).
    IgrDialog,
    /// A `CHAT_MSG_SYSTEM` line of `BILLING_NAG_WARNING` with the minutes left (`0x4957d1`).
    Line(u32),
}

/// The poller's minutes left: `ceil(left · k)` over the zero-extended dword (`0x49569b`-`0x4956b2`),
/// so a deadline already passed, which wraps, reads as some 71,000 minutes. The product is an
/// f64 one: the client runs the x87 at 53 bits (`0x404100`), so `fild` then `fmul dword` rounds
/// as a double.
fn minutes_left(left_ms: u32) -> u32 {
    (f64::from(left_ms) * f64::from(MINUTES_PER_MS)).ceil() as u32
}

/// `a` is at or after `b` on the wrapping millisecond clock (the `sub`/`js` pairs).
fn reached(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) >= 0
}

impl PlayTimeWarning {
    /// `InitializeGame`'s arm: unarmed, then a deadline `minutes` ahead when the connection has a
    /// nonzero time of at most 72 hours on a plan that is not recurring.
    pub(crate) fn arm(&mut self, billing: Billing, now: u32) {
        self.deadline = 0;
        let minutes = billing.time_remaining;
        if minutes <= ARM_MAX_MINUTES && billing.plan_flags & Billing::RECURRING == 0 && minutes > 0
        {
            self.deadline = now.wrapping_add(minutes * MINUTE_MS);
            self.next_line = 0;
            self.dialog_shown = false;
        }
    }

    /// Whether a deadline is set.
    #[cfg(test)]
    pub(crate) fn armed(&self) -> bool {
        self.deadline != 0
    }

    /// One frame's poll, in the reference's order: a due line, then the dialog.
    pub(crate) fn poll(&mut self, now: u32, plan_flags: u8) -> Vec<Nag> {
        let mut out = Vec::new();
        if self.deadline == 0 {
            return out;
        }
        let left = self.deadline.wrapping_sub(now);
        let minutes = minutes_left(left);
        let igr = plan_flags & Billing::IGR != 0;
        if self.next_line == 0 {
            // The first poll shows the dialog when it is due (`0x4956d8`-`0x49570b`), the same
            // test as below, and starts the line clock now (`0x495715`).
            self.next_line = now;
        } else if reached(now, self.next_line) && !igr {
            if minutes <= 30 {
                out.push(Nag::Line(minutes));
            }
            if reached(now, self.deadline) {
                self.deadline = 0;
            } else {
                // The next mark: the time left down to a whole step (`0x4957ef`-`0x4957ff`).
                let step = if minutes <= 5 {
                    MINUTE_MS
                } else {
                    LINE_STEP_MS
                };
                self.next_line = now.wrapping_add(left % step);
            }
        }
        // The dialog, once (`0x495723`, `0x495805`).
        let (limit, nag) = if igr {
            (600, Nag::IgrDialog)
        } else {
            (30, Nag::Dialog(minutes))
        };
        if minutes <= limit && !self.dialog_shown {
            out.push(nag);
            self.dialog_shown = true;
        }
        out
    }
}

/// The process clock in milliseconds, the reference's `OsGetAsyncTimeMs` (`0x42c010`), wrapping
/// as its dword does.
pub(crate) fn now_ms(time: &Time<Real>) -> u32 {
    time.elapsed().as_millis() as u32
}

/// The world entry's arm, run before the interface loads as `InitializeGame` runs it before
/// `UI_Init` (`0x48f681`); a `ReloadUI` never reaches it.
pub(crate) fn arm_at_world_entry(world: &mut World) {
    let billing = world
        .get_resource::<SessionBilling>()
        .map_or_else(Billing::default, |b| b.0);
    let now = world.get_resource::<Time<Real>>().map_or(0, now_ms);
    world
        .get_resource_or_init::<PlayTimeWarning>()
        .arm(billing, now);
}

/// The per-frame poll: each dialog is the stock `UIParent.lua` popup's event, each line a
/// `CHAT_MSG_SYSTEM` of the template `GetText` picks by the minutes (`0x49575a`-`0x495775`).
fn poll_play_time(
    script: Option<NonSendMut<UiScript>>,
    mut warning: ResMut<PlayTimeWarning>,
    billing: Res<SessionBilling>,
    mut chat: ResMut<ChatLog>,
    time: Res<Time<Real>>,
) {
    let Some(mut script) = script else {
        return;
    };
    for nag in warning.poll(now_ms(&time), billing.0.plan_flags) {
        match nag {
            Nag::Dialog(minutes) => script.fire_event(
                "BILLING_NAG_DIALOG",
                vec![ScriptValue::Int(i64::from(minutes))],
            ),
            Nag::IgrDialog => script.fire_event("IGR_BILLING_NAG_DIALOG", vec![]),
            Nag::Line(minutes) => {
                let get = |key: &str| script.lua().globals().get::<String>(key).ok();
                let Some(template) =
                    benilla_ui::strings::plural("BILLING_NAG_WARNING", Some(minutes), &get)
                else {
                    continue;
                };
                let text = benilla_ui::strings::fill(
                    &template,
                    &[benilla_ui::strings::Arg::D(i64::from(minutes))],
                );
                chat.push_event(ChatEvent::text_only(ChatEventKind::System, text));
            }
        }
    }
}

pub(crate) struct UiBillingPlugin;

impl Plugin for UiBillingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayTimeWarning>()
            .init_resource::<SessionBilling>()
            .init_resource::<ChatLog>()
            .add_systems(
                Update,
                poll_play_time
                    .in_set(UiFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn billing(time_remaining: u32, plan_flags: u8) -> Billing {
        Billing {
            time_remaining,
            plan_flags,
            time_rested: 0,
        }
    }

    /// Poll every `frame_ms` from `start` until `end`, collecting what each poll shows and when.
    fn run(
        w: &mut PlayTimeWarning,
        start: u32,
        end: u32,
        frame_ms: u32,
        flags: u8,
    ) -> Vec<(u32, Nag)> {
        let mut seen = Vec::new();
        let mut now = start;
        while now <= end {
            seen.extend(w.poll(now, flags).into_iter().map(|n| (now, n)));
            now += frame_ms;
        }
        seen
    }

    /// The `f32` factor is the reference's bits, and it reads a whole minute as two.
    #[test]
    fn the_minutes_are_the_references_ceiling() {
        assert_eq!(MINUTES_PER_MS.to_bits(), 0x378b_cf65);
        assert_eq!(minutes_left(59_999), 1);
        assert_eq!(minutes_left(60_000), 2);
        assert_eq!(minutes_left(0), 0);
        assert!(
            minutes_left(0u32.wrapping_sub(16)) > 600,
            "a passed deadline wraps"
        );
    }

    /// One minute left: the dialog on the first frame, the line on the next, then nothing.
    #[test]
    fn one_minute_left_shows_the_dialog_then_one_line() {
        let mut w = PlayTimeWarning::default();
        w.arm(billing(1, 0x44), 1_000);
        assert_eq!(w.poll(1_016, 0x44), vec![Nag::Dialog(1)]);
        assert_eq!(w.poll(1_032, 0x44), vec![Nag::Line(1)]);
        assert_eq!(run(&mut w, 1_050, 200_000, 16, 0x44), vec![]);
        assert_eq!(w.deadline, 0, "the poll past the deadline disarms");

        // A poll on the deadline's own millisecond reads 0 left, under 30, and prints it.
        w.arm(billing(1, 0), 1_000);
        assert_eq!(w.poll(1_016, 0), vec![Nag::Dialog(1)]);
        assert_eq!(w.poll(1_032, 0), vec![Nag::Line(1)]);
        assert_eq!(w.poll(61_000, 0), vec![Nag::Line(0)]);
        assert_eq!(w.deadline, 0);
    }

    /// From 32 minutes: the dialog at the frame the time reads 30, a line at each 5-minute mark
    /// down to 5 and each minute after, and the poll stops at the deadline.
    #[test]
    fn the_cadence_from_thirty_two_minutes() {
        let mut w = PlayTimeWarning::default();
        w.arm(billing(32, 0), 0x10);
        let seen = run(&mut w, 0x10 + 7, 0x10 + 32 * MINUTE_MS + 100, 10, 0);
        let lines: Vec<u32> = seen
            .iter()
            .filter_map(|(_, n)| match n {
                Nag::Line(m) => Some(*m),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec![30, 25, 20, 15, 10, 5, 4, 3, 2, 1]);
        let dialogs: Vec<(u32, Nag)> = seen
            .iter()
            .copied()
            .filter(|(_, n)| matches!(n, Nag::Dialog(_)))
            .collect();
        assert_eq!(dialogs.len(), 1);
        assert_eq!(dialogs[0].1, Nag::Dialog(30));
        assert!(
            0x10 + 32 * MINUTE_MS - dialogs[0].0 <= 30 * MINUTE_MS,
            "the dialog waits for the time to read 30"
        );
        assert_eq!(w.deadline, 0);
    }

    /// No time, more than 72 hours, or a recurring plan arm nothing.
    #[test]
    fn what_does_not_arm() {
        for b in [
            billing(0, 0),
            billing(4_321, 0),
            billing(10, Billing::RECURRING),
        ] {
            let mut w = PlayTimeWarning::default();
            w.arm(b, 500);
            assert_eq!(
                run(&mut w, 500, 500 + 20 * MINUTE_MS, 1_000, b.plan_flags),
                vec![]
            );
        }
        let mut w = PlayTimeWarning::default();
        w.arm(billing(4_320, 0), 500);
        assert_ne!(w.deadline, 0, "72 hours exactly arms");
    }

    /// An Internet Gaming Room (IGR) plan: its own dialog once, at 600 minutes or fewer, and no lines.
    #[test]
    fn an_igr_plan_gets_its_dialog_once() {
        let mut w = PlayTimeWarning::default();
        w.arm(billing(602, Billing::IGR), 0);
        let seen = run(&mut w, 1, 602 * MINUTE_MS + 1_000, 997, Billing::IGR);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1, Nag::IgrDialog);
        assert!(602 * MINUTE_MS - seen[0].0 <= 600 * MINUTE_MS);
    }

    /// The stock `UIParent.lua` and `StaticPopup.lua` show the dialog, and the line is the
    /// singular template, through the real system on a VM with the stock files.
    #[test]
    fn the_stock_ui_shows_the_nag_and_the_line() {
        benilla_formats::wow_data_or_skip!();
        use crate::ui_script::test_ui::load_ui;
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        for f in [
            r"Interface\FrameXML\GlobalStrings.lua",
            r"Interface\FrameXML\Fonts.xml",
            r"Interface\FrameXML\BasicControls.xml",
            r"Interface\FrameXML\LocaleProperties.lua",
            r"Interface\FrameXML\UIParent.xml",
            r"Interface\FrameXML\MoneyFrame.lua",
            r"Interface\FrameXML\MoneyFrame.xml",
            r"Interface\FrameXML\UIPanelTemplates.lua",
            r"Interface\FrameXML\UIPanelTemplates.xml",
            r"Interface\FrameXML\StaticPopup.xml",
        ] {
            load_ui(&s, f);
        }
        // A hand-driven clock: a frame is 16 ms, and the arm sees the clock's start.
        let start = std::time::Instant::now();
        let mut clock = Time::<Real>::new(start);
        clock.update_with_instant(start);
        let mut app = App::new();
        app.insert_non_send_resource(s)
            .insert_resource(clock)
            .init_resource::<ChatLog>()
            .init_resource::<PlayTimeWarning>()
            .insert_resource(SessionBilling(billing(1, 0)))
            .add_systems(Update, poll_play_time);
        arm_at_world_entry(app.world_mut());
        let frame = |app: &mut App, ms: u64| {
            app.world_mut()
                .resource_mut::<Time<Real>>()
                .update_with_instant(start + std::time::Duration::from_millis(ms));
            app.update();
        };

        frame(&mut app, 16);
        let s = app.world().non_send_resource::<UiScript>();
        assert!(s.eval::<bool>("return StaticPopup1:IsVisible()").unwrap());
        assert_eq!(
            s.eval::<String>("return StaticPopup1Text:GetText()")
                .unwrap(),
            "Your play time expires in 1 Minute"
        );
        frame(&mut app, 32);
        assert_eq!(
            app.world().resource::<ChatLog>().pending_lines(),
            vec!["Your play time expires in 1 minute".to_string()]
        );
    }
}
