//! Diagnostic probe: prints each streamed unit's `UNIT_FIELD_FACTIONTEMPLATE`, with level and
//! display id, to check the descriptor decode against `creature_template.faction`.
//!
//! Run: `cargo run -p benilla-protocol --example faction_probe -- probeN <password> [host]`. The
//! account has no default, because a login kicks whoever is on the account.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use benilla_protocol::{decode, EntityKind, SessionEvent, WorldSession, WORLD_PORT};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let user = args
        .next()
        .context("usage: faction_probe -- <probeN> <password> [host] (a probe account)")?;
    let pass = args
        .next()
        .context("usage: faction_probe -- <probeN> <password> [host] (a probe account)")?;
    let host = args.next().unwrap_or_else(|| "localhost".into());

    let logon = benilla_protocol::logon(&host, &user, &pass)?;
    let world_addr = logon
        .realms
        .first()
        .map(|r| r.address.clone())
        .unwrap_or_else(|| format!("{host}:{WORLD_PORT}"));
    let mut session = WorldSession::connect(&world_addr, &user, logon.session_key)?;
    let characters = session.char_enum()?;
    let character = characters.first().context("no characters")?;
    let self_guid = character.guid;
    println!("logging in '{}' (guid {self_guid})", character.name);
    session.player_login(self_guid)?;
    session.set_active_mover(self_guid)?;
    session.set_read_timeout(Some(Duration::from_secs(2)))?;

    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let Ok(msg) = session.recv() else { continue };
        for ev in decode(msg) {
            match ev {
                SessionEvent::ObjectCreate {
                    guid,
                    kind,
                    display_id,
                    fields,
                    ..
                } if matches!(kind, EntityKind::Unit | EntityKind::Player) => {
                    let me = if guid == self_guid { " (self)" } else { "" };
                    println!(
                        "{kind:?} guid={guid}{me} display={display_id:?} level={:?} faction_tpl={:?}",
                        fields.unit_level(),
                        fields.unit_faction_template(),
                    );
                }
                SessionEvent::ObjectValues { guid, fields } => {
                    if let Some(f) = fields.unit_faction_template() {
                        println!("Values guid={guid} faction_tpl={f}");
                    }
                }
                SessionEvent::Reputations { standings } => {
                    let set: Vec<(usize, u8, i32)> = standings
                        .iter()
                        .enumerate()
                        .filter(|(_, &(f, s))| f != 0 || s != 0)
                        .map(|(i, &(f, s))| (i, f, s))
                        .collect();
                    println!(
                        "Reputations: {} slots, non-zero (idx, flags, standing): {set:?}",
                        standings.len()
                    );
                }
                _ => {}
            }
        }
    }
    Ok(())
}
