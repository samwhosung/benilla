//! The **arrival layer** of the mail arc (decisions 0544 P3 / 0904 / 0913) — the real client's
//! pending-mail countdown, modelled field for field.
//!
//! This is the half of [`super`] that has nothing to do with the mailbox *window*: it survives the
//! window closing, it is what `HasNewMail()` answers from, and it is the entire life of the minimap
//! mail icon. The window session lives next door in [`super`]; the seam between them is narrow on
//! purpose — the two only meet at the mailbox close edge, where a read letter turns into the
//! re-query that clears the icon.
//!
//! Every rule here is the reference's, and so is the load-bearing *negative*: **nothing on the
//! inbox path writes the countdown** (`0x845eac`).

use bevy::prelude::*;

/// The `HasNewMail()` / countdown-step epsilon — the real client's `[0x8029d4]` = `2^-22`, the
/// `fabs(x)`-vs-this threshold both `0x4afea0` (`HasNewMail`) and `0x4ade60` (the per-frame step)
/// compare against.
const MAIL_TIME_EPSILON: f32 = f32::from_bits(0x3480_0000); // 2^-22, the reference's `[0x8029d4]`

/// The "no mail waiting" stamp: the literal **`-1.0f`** the mail module's init (`0x4acb87`) and the
/// `MSG_QUERY_NEXT_MAIL_TIME` **sender** (`0x4ade25`) both write into the countdown,
/// unconditionally. Any value outside ε of zero reads "no mail";
/// `-1.0` is simply the one the reference picks, and it is what the countdown holds between asking
/// the server and hearing back.
const MAIL_TIME_NO_MAIL: f32 = -1.0;

/// The client's own pending-mail model (decisions 0544 P3 / 0904 / 0913) — the countdown float
/// `0x845eac` plus the deferred-refresh flag `[0xb6efcc]`, mirrored field for field.
///
/// **Why a countdown and not a bool.** `HasNewMail()` is `|countdown| < ε`; the countdown is
/// seeded by `MSG_QUERY_NEXT_MAIL_TIME`'s reply and `SMSG_RECEIVED_MAIL`, stepped down per frame,
/// and stamped back to [`MAIL_TIME_NO_MAIL`] whenever the client (re-)asks the server. Independent
/// of [`MailOpen`] (that is session-scoped to one open window; this survives it closing).
///
/// **The five writers**: the module init and the query sender stamp `-1.0`
/// ([`Self::on_query_sent`]), the query reply stores the server's float
/// ([`Self::apply_query_reply`]), `SMSG_RECEIVED_MAIL` runs the set-value ladder
/// ([`Self::apply_received_mail`]), and the per-frame step counts down ([`Self::step`]).
/// **Nothing on the inbox path writes it** — that negative is the load-bearing half.
#[derive(Resource)]
pub(crate) struct MailPending {
    /// The countdown float `0x845eac`, in seconds until mail is "waiting".
    countdown: f32,
    /// The deferred-refresh flag `[0xb6efcc]`: "re-ask the server on close". Set by the
    /// mark-as-read sender (`0x4adda6`) and by `SMSG_RECEIVED_MAIL` arriving while the mailbox is
    /// open (`0x4ad642`); read at exactly one site — the close core (`0x4acda8`).
    refresh_pending: bool,
    /// A queued `UPDATE_PENDING_MAIL`, drained by [`feed_mail`]. The event is **edge-triggered at
    /// three sites only** (`0x4ad605` the reply, `0x4ad66b` the near-zero arrival, `0x4adeba` the
    /// step crossing under ε) — not per frame, and not on every value change.
    notify: bool,
}

impl Default for MailPending {
    /// The reference's own resting state: no mail waiting, nothing deferred. (The module init
    /// stamps exactly this at every world-enter, so it is also the post-`PLAYER_ENTERING_WORLD`
    /// value before the reply lands.)
    fn default() -> Self {
        Self {
            countdown: MAIL_TIME_NO_MAIL,
            refresh_pending: false,
            notify: false,
        }
    }
}

impl MailPending {
    /// `HasNewMail()` (`0x4afea0`: `fld [0x845eac]; fabs; fcomp [0x8029d4]`,
    /// emitted `jp`) — true iff the countdown is **within ε of zero**. Strict `<`, symmetric, and
    /// false at exact equality and for NaN.
    ///
    /// Near-zero, *not* `<= 0`: the sign carries meaning. vmangos answers
    /// `MSG_QUERY_NEXT_MAIL_TIME` with `-86400.0` when nothing is unread (`MailHandler.cpp`
    /// `HandleQueryNextMailTime`: `HasUnreadMail() ? 0.0f : -float(DAY)`), so a `<= 0` predicate
    /// lights the minimap icon on every login for a character with no mail.
    pub(super) fn has_new_mail(&self) -> bool {
        self.countdown.abs() < MAIL_TIME_EPSILON
    }

    /// The per-frame countdown step (`0x4ade60`): a **non-positive countdown is left alone**
    /// (the "no mail" stamp must never drift toward zero, and a reached-zero countdown must not
    /// sail past it into negative — either would flip [`Self::has_new_mail`] the wrong way), and a
    /// positive one steps down floor-clamped at `0.0`. The subtraction runs the client's x87 chain
    /// (f64 under PC_53, narrowed at the store). Signals once, on the step that lands inside ε.
    pub(super) fn step(&mut self, delta_secs: f32) {
        if self.countdown > 0.0 {
            let diff = f64::from(self.countdown) - f64::from(delta_secs); // fsub, then fcom vs 0.0
            self.countdown = if diff > 0.0 { diff as f32 } else { 0.0 };
            if self.has_new_mail() {
                self.notify = true; // 0x4adeba — the crossing edge, fired once
            }
        }
    }

    /// The `MSG_QUERY_NEXT_MAIL_TIME` **reply** (`0x4ad5f0`): store the server's float
    /// verbatim and signal — **unconditionally**, whether or not `HasNewMail()` changed.
    pub(crate) fn apply_query_reply(&mut self, seconds: f32) {
        self.countdown = seconds;
        self.notify = true; // 0x4ad605 — unconditional
    }

    /// Sending `MSG_QUERY_NEXT_MAIL_TIME` (the sender `0x4ade25` and the module init
    /// `0x4acb87` both stamp `-1.0f`): the countdown reads "no mail" from the moment we ask until
    /// the reply lands. **No signal** — the sender does not fire `UPDATE_PENDING_MAIL`, so the icon
    /// keeps its old face for the round trip and updates when the reply arrives.
    pub(super) fn on_query_sent(&mut self) {
        self.countdown = MAIL_TIME_NO_MAIL;
    }

    /// `SMSG_RECEIVED_MAIL` (`0x4ad620`).
    /// `mailbox_open` is the busy-flag pair `[0xb6ef88]|[0xb6ef8c]`, the open mailbox's guid:
    ///
    /// * **busy** → arm the deferred refresh and leave the countdown alone (the icon does not move
    ///   while you stand at the mailbox; the close re-query settles it);
    /// * `|delay| < ε` → store it and signal ("you have mail *now*");
    /// * else if the current countdown is negative → store it (no signal);
    /// * else store the *smaller* of the two (tightening the estimate), no signal.
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

    /// Arm the deferred refresh `[0xb6efcc]` — the mark-as-read sender does this unconditionally
    /// (`0x4adda6`), which is *the* mechanism by which checking your mail clears the icon: the
    /// close core sees the flag and re-asks the server.
    pub(crate) fn arm_refresh(&mut self) {
        self.refresh_pending = true;
    }

    /// The close core's read of `[0xb6efcc]` (`0x4acda8`) — the flag's only reader, consumed.
    pub(super) fn take_refresh(&mut self) -> bool {
        std::mem::take(&mut self.refresh_pending)
    }

    /// Drain a queued `UPDATE_PENDING_MAIL`.
    pub(super) fn take_notify(&mut self) -> bool {
        std::mem::take(&mut self.notify)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A [`MailPending`] holding `countdown`, as if the server had just answered with it.
    fn pending_at(countdown: f32) -> MailPending {
        MailPending {
            countdown,
            ..Default::default()
        }
    }

    /// `HasNewMail()` is `|countdown| < ε`, not `countdown <= 0`. The regression
    /// this pins: vmangos's "no unread mail" answer is `-86400.0`, and the old `<= 0` predicate
    /// read it as "you have mail" — the phantom minimap icon on every login.
    #[test]
    fn has_new_mail_is_near_zero_not_non_positive() {
        // The resting state — the reference's own `-1.0` init stamp.
        assert!(!MailPending::default().has_new_mail());
        // vmangos's two real answers.
        assert!(pending_at(0.0).has_new_mail());
        assert!(!pending_at(-86400.0).has_new_mail());
        // The threshold itself: inside ε counts, ε and beyond does not — either sign. Strict `<`,
        // so exact equality is false (the emitted `jp` at `0x4afeb3` over the x87 C-bits).
        assert!(pending_at(MAIL_TIME_EPSILON / 2.0).has_new_mail());
        assert!(pending_at(-MAIL_TIME_EPSILON / 2.0).has_new_mail());
        assert!(!pending_at(MAIL_TIME_EPSILON).has_new_mail());
        assert!(!pending_at(-MAIL_TIME_EPSILON).has_new_mail());
        // Unordered compares false, as the x87 predicate does.
        assert!(!pending_at(f32::NAN).has_new_mail());
        // A still-running countdown is not "waiting now".
        assert!(!pending_at(30.0).has_new_mail());
    }

    /// The per-frame step (`0x4ade60`): non-positive is untouched, positive steps down and floors
    /// at exactly `0.0` — so a countdown that expires flips `HasNewMail()` true and *stays* true,
    /// signalling exactly once on the crossing.
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

        // A reached-zero countdown stays at zero rather than sailing past into negative — and does
        // not re-signal every frame (the reference's edge is one-shot).
        let mut expired = pending_at(0.0);
        expired.step(1.0 / 60.0);
        assert_eq!(expired.countdown, 0.0);
        assert!(expired.has_new_mail());
        assert!(!expired.take_notify());

        // A positive countdown steps down, then floors (never overshoots past 0), signalling on the
        // step that lands it inside ε and not before.
        let mut counting = pending_at(0.5);
        counting.step(0.125); // exact in binary — the step's value is pinned, not approximated
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

    /// The query reply stores verbatim and signals **unconditionally** — even when `HasNewMail()`
    /// did not change (`0x4ad605`). A transition-only fire would be a silent divergence.
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

    /// Sending the query stamps "no mail" but does **not** signal — so the icon keeps its face for
    /// the round trip and moves when the reply lands (`0x4ade25` stamps, `0x4ad605` fires).
    #[test]
    fn sending_the_query_stamps_without_signalling() {
        let mut p = pending_at(0.0);
        assert!(p.has_new_mail());
        p.on_query_sent();
        assert_eq!(p.countdown, MAIL_TIME_NO_MAIL);
        assert!(!p.has_new_mail());
        assert!(!p.take_notify());
    }

    /// `SMSG_RECEIVED_MAIL`'s set-value ladder (`0x4ad620`), all four branches.
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

        // Not busy, |delay| < eps: store and signal — vmangos's only case.
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

        // Not busy, a real delay, current countdown positive: keep the *smaller* (tighten), silent.
        let mut tighten = pending_at(120.0);
        tighten.apply_received_mail(60.0, false);
        assert_eq!(tighten.countdown, 60.0);
        tighten.apply_received_mail(300.0, false);
        assert_eq!(tighten.countdown, 60.0, "a later estimate never loosens it");
        assert!(!tighten.take_notify());
    }

    /// The mechanism that actually clears the icon: opening a letter arms the
    /// deferred refresh, and the close consumes it into a re-query whose stamp darkens the icon.
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
