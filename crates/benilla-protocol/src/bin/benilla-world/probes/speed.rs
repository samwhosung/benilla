//! `--speed`: ack two `SMSG_FORCE_RUN_SPEED_CHANGE`s from `.modify speed`; a malformed ack drops
//! the session, so surviving both round trips is the proof.

use anyhow::{ensure, Result};
use benilla_protocol::{SessionEvent, SpeedKind};

use crate::probes::{Ctx, Probe};

#[derive(Default)]
pub(crate) struct Speed {
    second_sent: bool,
}

impl Probe for Speed {
    fn stage(&mut self, cx: &mut Ctx) -> Result<()> {
        cx.session.send_chat(".modify speed 1.5")?;
        println!("sent GM: .modify speed 1.5 (self) — expecting SMSG_FORCE_RUN_SPEED_CHANGE");
        Ok(())
    }

    fn on_event(&mut self, ev: &SessionEvent, cx: &mut Ctx) -> Result<()> {
        if let SessionEvent::ForceSpeedChange { guid, .. } = ev {
            // Runs after World's ack, which World sends only with our pose tracked; the same
            // guard holds the second `.modify` back when there was no ack.
            if *guid == cx.world.self_guid
                && cx.world.tracked.contains_key(guid)
                && !self.second_sent
            {
                cx.session.send_chat(".modify speed 1")?;
                self.second_sent = true;
                println!("sent GM: .modify speed 1 — expecting the second change");
            }
        }
        Ok(())
    }

    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        // Base run speed is 7.0 yd/s, so the rates 1.5 and 1 give 10.5 then 7.0.
        let speed_changes_seen = &cx.world.speed_changes_seen;
        ensure!(
            speed_changes_seen.len() >= 2,
            "--speed: expected 2 force-speed changes (got {}) — did the first ack drop the \
             session, or is the account not GM?",
            speed_changes_seen.len()
        );
        let (k1, c1, s1) = speed_changes_seen[0];
        let (k2, c2, s2) = speed_changes_seen[1];
        ensure!(
            k1 == SpeedKind::Run && k2 == SpeedKind::Run,
            "--speed: expected Run changes, got {k1:?} then {k2:?}"
        );
        ensure!(
            (s1 - 10.5).abs() < 0.01 && (s2 - 7.0).abs() < 0.01,
            "--speed: expected flat speeds 10.5 then 7.0 (rates 1.5/1.0 × base 7.0), got {s1} then {s2}"
        );
        ensure!(
            c2 > c1,
            "--speed: movement counter must increment across changes (got {c1} then {c2})"
        );
        println!(
            "\n--speed PASS: {k1:?} 7.0->10.5->7.0 yd/s, counters {c1}->{c2}, both acks accepted \
             (stream survived)"
        );
        Ok(())
    }
}
