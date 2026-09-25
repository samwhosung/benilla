//! Live probe: the character-select park behaviours the glue screens rely on. A logout ends in
//! `SMSG_LOGOUT_COMPLETE` and a reconnect re-serves the roster; a socket parked at character
//! select for 130 s still logs in, since vmangos does not kick a quiet authenticated socket.
//!
//! Needs the local vmangos and an account in `WOW_USER`/`WOW_PASS`.

use std::time::Duration;

use benilla_protocol::{logon, WorldSession, WORLD_PORT};

fn connect(user: &str, pass: &str) -> anyhow::Result<WorldSession> {
    let l = logon("localhost", user, pass)?;
    let addr = l
        .realms
        .first()
        .map(|r| r.address.clone())
        .unwrap_or_else(|| format!("localhost:{WORLD_PORT}"));
    WorldSession::connect(&addr, user, l.session_key)
}

fn main() -> anyhow::Result<()> {
    let user = std::env::var("WOW_USER").map_err(|_| anyhow::anyhow!("set WOW_USER"))?;
    let pass = std::env::var("WOW_PASS").map_err(|_| anyhow::anyhow!("set WOW_PASS"))?;
    let (user, pass) = (user.as_str(), pass.as_str());

    // ── 1 · logout round-trip ──
    let mut s = connect(user, pass)?;
    let chars = s.char_enum()?;
    println!(
        "roster: {:?}",
        chars.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    let c = &chars[0];
    s.player_login(c.guid)?;
    s.set_active_mover(c.guid)?;
    println!("logged in as {} — requesting logout in 2s", c.name);
    std::thread::sleep(Duration::from_secs(2));
    s.set_read_timeout(Some(Duration::from_secs(1)))?;
    s.logout(Duration::from_secs(25))?;
    println!("PASS: SMSG_LOGOUT_COMPLETE received");
    drop(s);

    // A fresh connection re-serves the roster, as the app's relist does.
    let mut s = connect(user, pass)?;
    let n = s.char_enum()?.len();
    println!("PASS: post-logout reconnect roster has {n} chars");
    drop(s);

    // ── 2 · idle park ──
    let mut s = connect(user, pass)?;
    let chars = s.char_enum()?;
    println!("parking at character select for 130s…");
    std::thread::sleep(Duration::from_secs(130));
    match s.player_login(chars[0].guid).and_then(|()| {
        s.set_active_mover(chars[0].guid)?;
        // Any post-login packet shows the world streams.
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.recv().map(|p| p.name())
    }) {
        Ok(pkt) => println!("PASS: idle-parked login works (first packet: {pkt})"),
        Err(e) => println!("KICKED: idle park failed login: {e:#} (self-heals via relist)"),
    }
    Ok(())
}
