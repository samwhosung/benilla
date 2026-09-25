//! Diagnostic probe: the server's cooldown window for Charge (spell 100: `recoveryTime 0`,
//! category 44, `categoryRecoveryTime 15000`) against the client's 15 s sweep. It charges a
//! Northshire Kobold Vermin (entry 6), then recasts every 200 ms. vmangos checks the cooldown
//! first in `CheckCast` (`Spell.cpp:5369`), so the first result other than
//! `SPELL_FAILED_NOT_READY` (60), or a second `SMSG_SPELL_GO`, marks the server's cooldown end
//! relative to the first GO.
//!
//! Run: `cargo run -p benilla-protocol --example cooldown_probe -- probeN <password> [host]` on a
//! probe account with a warrior (a login kicks whoever is on the account); `.learn` needs
//! gmlevel 5 (`SEC_DEVELOPER`).

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use benilla_protocol::{decode, EntityKind, SessionEvent, WorldSession, WORLD_PORT};

const CHARGE: u32 = 100;
const BATTLE_STANCE: u32 = 2457;
const KOBOLD_VERMIN: u32 = 6;

fn fail_name(reason: u8) -> &'static str {
    match reason {
        0 => "AFFECTING_COMBAT",
        10 => "BAD_TARGETS",
        23 => "DONT_REPORT",
        42 => "LINE_OF_SIGHT",
        50 => "NOPATH",
        56 => "NOT_KNOWN",
        60 => "NOT_READY",
        86 => "ONLY_SHAPESHIFT",
        89 => "OUT_OF_RANGE",
        _ => "?",
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let user = args
        .next()
        .context("usage: cooldown_probe -- <probeN> <password> [host] (a probe account)")?;
    let pass = args
        .next()
        .context("usage: cooldown_probe -- <probeN> <password> [host] (a probe account)")?;
    let host = args.next().unwrap_or_else(|| "localhost".into());

    let logon = benilla_protocol::logon(&host, &user, &pass)?;
    let world_addr = logon
        .realms
        .first()
        .map(|r| r.address.clone())
        .unwrap_or_else(|| format!("{host}:{WORLD_PORT}"));
    let mut session = WorldSession::connect(&world_addr, &user, logon.session_key)?;
    let characters = session.char_enum()?;
    let character = characters
        .iter()
        .find(|c| c.name == "Tri")
        .or_else(|| characters.first())
        .context("no characters")?;
    let self_guid = character.guid;
    println!("logging in '{}' (guid {self_guid})", character.name);
    session.player_login(self_guid)?;
    session.set_active_mover(self_guid)?;
    session.set_read_timeout(Some(Duration::from_millis(100)))?;

    let mut kobolds: Vec<u64> = Vec::new();
    let drain = |session: &mut WorldSession,
                 kobolds: &mut Vec<u64>,
                 secs: f32,
                 mut on_event: &mut dyn FnMut(f64, SessionEvent)|
     -> Result<()> {
        let until = Instant::now() + Duration::from_secs_f32(secs);
        let t0 = Instant::now();
        while Instant::now() < until {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                match &ev {
                    SessionEvent::ObjectCreate {
                        guid, kind, fields, ..
                    } if matches!(kind, EntityKind::Unit)
                        && fields.object_entry() == Some(KOBOLD_VERMIN)
                        && !kobolds.contains(guid) =>
                    {
                        kobolds.push(*guid);
                    }
                    // A `.go` teleport must be acked or the server freezes us and streams nothing.
                    SessionEvent::Teleport { guid, counter, .. } => {
                        println!("  teleport → acking (counter {counter})");
                        session.teleport_ack(*guid, *counter)?;
                    }
                    _ => {}
                }
                on_event(t0.elapsed().as_secs_f64(), ev);
            }
        }
        let _ = &mut on_event;
        Ok(())
    };
    let mut quiet = |_t: f64, ev: SessionEvent| match ev {
        SessionEvent::Chat(m) => println!("  chat: {:?}", m.text),
        SessionEvent::SpellLearned { spell_id } => println!("  learned: {spell_id}"),
        _ => {}
    };

    // Let the world-enter flood settle first.
    drain(&mut session, &mut kobolds, 3.0, &mut quiet)?;
    session.set_selection(self_guid)?;
    session.send_chat(".learn 100")?;
    session.send_chat(".go xyz -8785 -150 82.5")?;
    drain(&mut session, &mut kobolds, 3.0, &mut quiet)?;
    session.cast_spell(BATTLE_STANCE, None)?;
    let mut chatty = |_t: f64, ev: SessionEvent| match ev {
        SessionEvent::Chat(m) => println!("  chat: {:?}", m.text),
        SessionEvent::CastResult {
            spell_id,
            success,
            reason,
            ..
        } => println!(
            "  pre: cast_result spell={spell_id} success={success} reason={reason:?} ({})",
            reason.map(fail_name).unwrap_or("-")
        ),
        _ => {}
    };
    drain(&mut session, &mut kobolds, 2.0, &mut chatty)?;
    println!("kobolds seen: {}", kobolds.len());

    // First Charge: try candidates until one GO lands.
    let mut target: Option<u64> = None;
    let mut charge_go_at: Option<Instant> = None;
    for &guid in kobolds.clone().iter() {
        println!("charging kobold {guid:#x}…");
        session.set_selection(guid)?;
        session.cast_spell(CHARGE, Some(guid))?;
        // Stamp the anchor at GO decode: the drain runs its full 1.5 s regardless.
        let mut verdict: Option<(bool, Instant)> = None;
        let mut watch = |_t: f64, ev: SessionEvent| match ev {
            SessionEvent::SpellGo {
                caster, spell_id, ..
            } if caster == self_guid && spell_id == CHARGE => {
                verdict = Some((true, Instant::now()));
            }
            SessionEvent::CastResult {
                spell_id,
                success: false,
                reason,
                ..
            } if spell_id == CHARGE => {
                println!(
                    "  refused: {:?} ({})",
                    reason,
                    reason.map(fail_name).unwrap_or("-")
                );
                verdict = Some((false, Instant::now()));
            }
            _ => {}
        };
        drain(&mut session, &mut kobolds, 1.5, &mut watch)?;
        if let Some((true, at)) = verdict {
            println!("  GO — cooldown armed");
            target = Some(guid);
            charge_go_at = Some(at);
            break;
        }
    }
    let target = target.context("no charge landed — no measurement")?;
    let t0 = charge_go_at.unwrap();

    // The spam loop: recast every 200 ms for 20 s; every result timestamps the server verdict.
    println!("t=0.00s — first GO received; spamming recasts…");
    let mut last_not_ready: Option<f64> = None;
    let mut first_free: Option<(f64, String)> = None;
    while t0.elapsed() < Duration::from_secs(20) {
        session.cast_spell(CHARGE, Some(target))?;
        let mut watch = |_t: f64, ev: SessionEvent| {
            let at = t0.elapsed().as_secs_f64();
            match ev {
                SessionEvent::CastResult {
                    spell_id,
                    success,
                    reason,
                    ..
                } if spell_id == CHARGE => {
                    let name = reason.map(fail_name).unwrap_or("-");
                    println!(
                        "t={at:5.2}s  cast_result success={success} reason={reason:?} ({name})"
                    );
                    if reason == Some(60) {
                        last_not_ready = Some(at);
                    } else if first_free.is_none() {
                        first_free = Some((at, format!("result {reason:?} ({name})")));
                    }
                }
                SessionEvent::SpellGo {
                    caster, spell_id, ..
                } if caster == self_guid && spell_id == CHARGE => {
                    println!("t={at:5.2}s  SPELL_GO (recast SUCCEEDED)");
                    if first_free.is_none() {
                        first_free = Some((at, "SPELL_GO".into()));
                    }
                }
                SessionEvent::SpellCooldowns { cooldowns, .. } => {
                    println!("t={at:5.2}s  SMSG_SPELL_COOLDOWN {cooldowns:?}");
                }
                SessionEvent::CooldownEvent { spell_id, .. } => {
                    println!("t={at:5.2}s  SMSG_COOLDOWN_EVENT spell={spell_id}");
                }
                SessionEvent::ClearCooldown { spell_id, .. } => {
                    println!("t={at:5.2}s  SMSG_CLEAR_COOLDOWN spell={spell_id}");
                }
                _ => {}
            }
        };
        drain(&mut session, &mut kobolds, 0.2, &mut watch)?;
    }

    println!("---");
    println!("last NOT_READY:  {last_not_ready:?}");
    println!("first non-NOT_READY after GO: {first_free:?}");
    Ok(())
}
