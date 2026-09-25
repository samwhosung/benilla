//! The mail arrival layer: the reference's pending-mail countdown behind `HasNewMail()` and the
//! minimap icon. It outlives the mailbox window, and nothing on the inbox path writes it.

use bevy::prelude::*;

/// The epsilon `HasNewMail()` (`0x4afea0`) and the per-frame step (`0x4ade60`) compare `fabs`
/// against: `[0x8029d4]`, 2^-22.
const MAIL_TIME_EPSILON: f32 = f32::from_bits(0x3480_0000);

/// The "no mail" stamp the module init (`0x4acb87`) and the query sender (`0x4ade25`) write; the
/// countdown holds it until the reply.
const MAIL_TIME_NO_MAIL: f32 = -1.0;

/// The reference's pending-mail state: the countdown `0x845eac` and the deferred-refresh flag
/// `[0xb6efcc]`.
#[derive(Resource)]
pub(crate) struct MailPending {
    /// Seconds until mail is waiting (`0x845eac`).
    countdown: f32,
    /// Re-ask the server on close (`[0xb6efcc]`): set by the mark-as-read sender (`0x4adda6`) and
    /// by an arrival while the mailbox is open (`0x4ad642`), read only by the close core
    /// (`0x4acda8`).
    refresh_pending: bool,
    /// A queued `UPDATE_PENDING_MAIL`, which fires at three sites only: the reply (`0x4ad605`), a
    /// near-zero arrival (`0x4ad66b`) and the step crossing under ε (`0x4adeba`).
    notify: bool,
}

impl Default for MailPending {
    /// No mail waiting and nothing deferred, as the module init stamps at every world enter.
    fn default() -> Self {
        Self {
            countdown: MAIL_TIME_NO_MAIL,
            refresh_pending: false,
            notify: false,
        }
    }
}

impl MailPending {
    /// `HasNewMail()` (`0x4afea0`): `|countdown| < ε`, false at equality and for NaN. Not `<= 0`:
    /// vmangos answers "no unread mail" with `-86400.0` (`MailHandler.cpp:912`).
    pub(super) fn has_new_mail(&self) -> bool {
        self.countdown.abs() < MAIL_TIME_EPSILON
    }

    /// The per-frame step (`0x4ade60`): a non-positive countdown stays put; a positive one steps
    /// down to a floor of `0.0`, in f64 as the x87 chain runs it (PC_53), and signals on the step
    /// that lands inside ε.
    pub(super) fn step(&mut self, delta_secs: f32) {
        if self.countdown > 0.0 {
            let diff = f64::from(self.countdown) - f64::from(delta_secs); // fsub, then fcom vs 0.0
            self.countdown = if diff > 0.0 { diff as f32 } else { 0.0 };
            if self.has_new_mail() {
                self.notify = true; // 0x4adeba: the crossing edge, fired once
            }
        }
    }

    /// The `MSG_QUERY_NEXT_MAIL_TIME` reply (`0x4ad5f0`): stored as sent, and it signals whether
    /// or not `HasNewMail()` changed.
    pub(crate) fn apply_query_reply(&mut self, seconds: f32) {
        self.countdown = seconds;
        self.notify = true; // 0x4ad605, unconditional
    }

    /// Sending the query stamps "no mail" (`0x4ade25`) with no signal, so the icon keeps its face
    /// until the reply.
    pub(super) fn on_query_sent(&mut self) {
        self.countdown = MAIL_TIME_NO_MAIL;
    }

    /// `SMSG_RECEIVED_MAIL` (`0x4ad620`); `mailbox_open` is its busy test on the open mailbox's
    /// guid (`[0xb6ef88]|[0xb6ef8c]`).
    pub(crate) fn apply_received_mail(&mut self, seconds: f32, mailbox_open: bool) {
        if mailbox_open {
            self.refresh_pending = true; // 0x4ad642
            return;
        }
        if f64::from(seconds).abs() < f64::from(MAIL_TIME_EPSILON) {
            self.countdown = seconds;
            self.notify = true; // 0x4ad66b
        } else if self.countdown < 0.0 || seconds < self.countdown {
            self.countdown = seconds;
        }
    }

    /// The mark-as-read sender's unconditional arm (`0x4adda6`).
    pub(crate) fn arm_refresh(&mut self) {
        self.refresh_pending = true;
    }

    /// The close core's read of the flag (`0x4acda8`), its only reader.
    pub(super) fn take_refresh(&mut self) -> bool {
        std::mem::take(&mut self.refresh_pending)
    }

    pub(super) fn take_notify(&mut self) -> bool {
        std::mem::take(&mut self.notify)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_at(countdown: f32) -> MailPending {
        MailPending {
            countdown,
            ..Default::default()
        }
    }

    #[test]
    fn has_new_mail_is_near_zero_not_non_positive() {
        // The resting state, the reference's `-1.0` init stamp.
        assert!(!MailPending::default().has_new_mail());
        // vmangos's two real answers.
        assert!(pending_at(0.0).has_new_mail());
        assert!(!pending_at(-86400.0).has_new_mail());
        // Inside ε counts and ε itself does not, either sign (the `jp` at `0x4afeb3`).
        assert!(pending_at(MAIL_TIME_EPSILON / 2.0).has_new_mail());
        assert!(pending_at(-MAIL_TIME_EPSILON / 2.0).has_new_mail());
        assert!(!pending_at(MAIL_TIME_EPSILON).has_new_mail());
        assert!(!pending_at(-MAIL_TIME_EPSILON).has_new_mail());
        // Unordered compares false, as the x87 predicate does.
        assert!(!pending_at(f32::NAN).has_new_mail());
        // A still-running countdown is not "waiting now".
        assert!(!pending_at(30.0).has_new_mail());
    }

    #[test]
    fn step_leaves_non_positive_alone_and_floors_at_zero() {
        // The "no mail" stamp never drifts toward zero, however long the session runs.
        let mut none_waiting = pending_at(-86400.0);
        for _ in 0..100 {
            none_waiting.step(1.0 / 60.0);
        }
        assert_eq!(none_waiting.countdown, -86400.0);
        assert!(!none_waiting.has_new_mail());
        assert!(
            !none_waiting.take_notify(),
            "a dormant countdown never signals"
        );

        // A reached-zero countdown stays at zero and does not re-signal.
        let mut expired = pending_at(0.0);
        expired.step(1.0 / 60.0);
        assert_eq!(expired.countdown, 0.0);
        assert!(expired.has_new_mail());
        assert!(!expired.take_notify());

        // A positive countdown steps down, floors at 0 and signals on the step that lands it.
        let mut counting = pending_at(0.5);
        counting.step(0.125); // exact in binary, so the step's value is pinned
        assert_eq!(counting.countdown, 0.375);
        assert!(!counting.has_new_mail());
        assert!(!counting.take_notify());
        counting.step(10.0);
        assert_eq!(counting.countdown, 0.0);
        assert!(counting.has_new_mail());
        assert!(
            counting.take_notify(),
            "the crossing fires UPDATE_PENDING_MAIL"
        );
    }

    #[test]
    fn query_reply_stores_and_always_signals() {
        let mut p = MailPending::default();
        p.apply_query_reply(-86400.0);
        assert_eq!(p.countdown, -86400.0);
        assert!(!p.has_new_mail());
        assert!(p.take_notify(), "false -> false still signals");

        p.apply_query_reply(0.0);
        assert!(p.has_new_mail());
        assert!(p.take_notify());
    }

    #[test]
    fn sending_the_query_stamps_without_signalling() {
        let mut p = pending_at(0.0);
        assert!(p.has_new_mail());
        p.on_query_sent();
        assert_eq!(p.countdown, MAIL_TIME_NO_MAIL);
        assert!(!p.has_new_mail());
        assert!(!p.take_notify());
    }

    #[test]
    fn received_mail_ladder() {
        // Busy (a mailbox window is open): arm the deferred refresh, leave the countdown alone.
        let mut busy = pending_at(-86400.0);
        busy.apply_received_mail(0.0, true);
        assert_eq!(
            busy.countdown, -86400.0,
            "the icon does not move under the player"
        );
        assert!(!busy.take_notify());
        assert!(busy.take_refresh(), "the close will re-ask the server");
        assert!(
            !busy.take_refresh(),
            "and the flag is consumed by that one read"
        );

        // Not busy, |delay| < ε: store and signal, vmangos's only case.
        let mut now = pending_at(-86400.0);
        now.apply_received_mail(0.0, false);
        assert_eq!(now.countdown, 0.0);
        assert!(now.has_new_mail());
        assert!(now.take_notify());

        // Not busy, a real delay, current countdown negative: store it, no signal.
        let mut seeded = pending_at(-1.0);
        seeded.apply_received_mail(120.0, false);
        assert_eq!(seeded.countdown, 120.0);
        assert!(!seeded.take_notify());

        // Not busy, a real delay, current countdown positive: keep the smaller, silently.
        let mut tighten = pending_at(120.0);
        tighten.apply_received_mail(60.0, false);
        assert_eq!(tighten.countdown, 60.0);
        tighten.apply_received_mail(300.0, false);
        assert_eq!(tighten.countdown, 60.0, "a later estimate never loosens it");
        assert!(!tighten.take_notify());
    }

    #[test]
    fn reading_mail_arms_the_refresh_that_the_close_consumes() {
        let mut p = pending_at(0.0);
        assert!(p.has_new_mail());
        assert!(!p.take_refresh(), "nothing armed before a letter is opened");

        p.arm_refresh(); // the mark-as-read sender, on every letter open
        assert!(p.has_new_mail(), "still lit while the window is open");

        // The close core: flag set -> re-ask, and the sender stamps "no mail".
        assert!(p.take_refresh());
        p.on_query_sent();
        assert!(!p.has_new_mail());
    }
}
