//! `--worldstate`: `.debug send worldstate <id> <value>` must come back as that exact
//! `SMSG_UPDATE_WORLD_STATE` pair, and a zone change must re-send `SMSG_INIT_WORLD_STATES`
//! (`Player::UpdateZone`, `Player.cpp:6736`), which cannot be requested.
//!
//! The command needs SEC_DEVELOPER, 5 (vmangos `Chat.cpp:323`); below it the server only refuses
//! in chat. The level is read from `realmd.account_access` (a row per `RealmID` and a `-1` row),
//! not `realmd.account.gmlevel`: grant both rows with the account offline, then revert.

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use benilla_protocol::SessionEvent;

use crate::probes::{Ctx, Probe};

/// Northshire, zone 9 (Elwynn Forest).
const ELWYNN_TP: &str = ".go xyz -8902.59 -162.606 82.0223";
/// Stormwind's Trade District, zone 1519 on the same map, so the hop is a real zone change.
const STORMWIND_TP: &str = ".go xyz -8913.0 554.0 93.8";
/// Time in Elwynn before the hop, so the second init is clearly a second packet.
const HOP_AFTER: Duration = Duration::from_secs(4);

/// A synthetic `(id, value)` no real content uses.
const PROBE_ID: u32 = 0xBEEF;
const PROBE_VALUE: u32 = 1_234_567;

/// One `SMSG_INIT_WORLD_STATES`: its `(map, zone)` scope and its pairs, terminator included.
struct Init {
    map: u32,
    zone: u32,
    states: Vec<(u32, u32)>,
}

#[derive(Default)]
pub(crate) struct WorldState {
    staged_at: Option<Instant>,
    hopped: bool,
    inits: Vec<Init>,
    /// Every `SMSG_UPDATE_WORLD_STATE` pair.
    updates: Vec<(u32, u32)>,
}

impl Probe for WorldState {
    fn stage(&mut self, cx: &mut Ctx) -> Result<()> {
        cx.session.send_chat(ELWYNN_TP)?;
        self.staged_at = Some(Instant::now());
        println!("worldstate: teleported to Elwynn (zone 9) {ELWYNN_TP}");
        Ok(())
    }

    fn poll(&mut self, cx: &mut Ctx) -> Result<()> {
        if self.hopped || self.staged_at.is_none_or(|t| t.elapsed() < HOP_AFTER) {
            return Ok(());
        }
        self.hopped = true;
        cx.session.send_chat(STORMWIND_TP)?;
        cx.session
            .send_chat(&format!(".debug send worldstate {PROBE_ID} {PROBE_VALUE}"))?;
        println!(
            "worldstate: hopped to Stormwind (zone 1519) and sent .debug send worldstate {PROBE_ID} {PROBE_VALUE}"
        );
        Ok(())
    }

    fn on_event(&mut self, ev: &SessionEvent, _cx: &mut Ctx) -> Result<()> {
        if let SessionEvent::WorldStates { scope, states } = ev {
            match *scope {
                Some((map, zone)) => self.inits.push(Init {
                    map,
                    zone,
                    states: states.clone(),
                }),
                None => self.updates.extend(states.iter().copied()),
            }
        }
        Ok(())
    }

    fn verify(&mut self, _cx: &mut Ctx) -> Result<()> {
        println!("\n--- world states ---");
        for init in &self.inits {
            println!(
                "SMSG_INIT_WORLD_STATES  map {} zone {}  {} pair(s)",
                init.map,
                init.zone,
                init.states.len()
            );
            for (id, value) in &init.states {
                println!("    {id:>6} = {:<12} (raw {value:#010x})", *value as i32);
            }
        }
        for (id, value) in &self.updates {
            println!(
                "SMSG_UPDATE_WORLD_STATE  {id} = {} (raw {value:#010x})",
                *value as i32
            );
        }

        if self.inits.is_empty() {
            bail!("no SMSG_INIT_WORLD_STATES arrived — the zone hop should have forced one");
        }
        // The server counts the (0,0) terminator, so a parsed init must end on it.
        for init in &self.inits {
            if init.states.last() != Some(&(0, 0)) {
                bail!(
                    "init for map {} zone {} did not end on the (0,0) terminator: {:?}",
                    init.map,
                    init.zone,
                    init.states.last()
                );
            }
        }
        if !self.updates.contains(&(PROBE_ID, PROBE_VALUE)) {
            bail!(
                "no SMSG_UPDATE_WORLD_STATE carrying our ({PROBE_ID}, {PROBE_VALUE}) — got {:?}. \
                 `.debug send worldstate` is gmlevel 5 (SEC_DEVELOPER); a lower account is silently refused",
                self.updates
            );
        }
        println!(
            "\nworldstate: OK — {} init packet(s), and the ({PROBE_ID}, {PROBE_VALUE}) round trip landed",
            self.inits.len()
        );
        Ok(())
    }
}
