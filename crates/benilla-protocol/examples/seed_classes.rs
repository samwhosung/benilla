//! Dev helper: gives an account one character of every class, optionally levelled, geared,
//! specced and parked in its capital, over a headless `WorldSession`.
//!
//! ```text
//! cargo run -p benilla-protocol --example seed_classes -- <user> <pass> <Prefix> [flags]
//!   --wipe            delete every character on the account first, irreversibly
//!   --level <n>       `.character level n` + `.learn all_myclass` (all class spells + talents)
//!   --tier <t>        `.character premade gear <role>-<t>` + `.character premade spec <spec>`;
//!                     `<t>` is `phase6-bis` (Naxx BiS), `preraid-bis`, or `r14` (rank-14 PvP
//!                     kit, custom templates 901–909), role is per class below
//!   --home            `.tele` each body to its own race's capital
//!   --spread <a|b>    which race spread to use (default `a`)
//!   --reload-templates  make the server re-read the premade tables before dressing
//!   --host <h>        default `localhost`
//!
//! … -- <account> <password> <Character> --wipe --level 60 --tier phase6-bis --home --spread a
//! ```
//!
//! Without `--wipe` it skips classes the account already has. Creating needs no GM level;
//! `.character premade gear|spec` needs 4 and `--level` needs 5, read from
//! `realmd.account_access`, not `account.gmlevel`. Both factions on one account need
//! `AllowTwoSide.Accounts = 1`, or a Horde create fails `CHAR_CREATE_PVP_TEAMS_VIOLATION` (0x33).
//!
//! Dressing is not idempotent: `.character premade gear` adds a fresh copy of the set on every
//! run, so re-dress only behind `--wipe`. `.learn all_myclass` also grants a Horde mage the
//! ungated Alliance city teleports, so a Horde mage ends on 74 spells and an Alliance one on 68.
//!
//! The server caches the premade tables at startup with no reload command, but
//! `.character premade savegear <name>` re-reads them (`LoadPlayerPremadeTemplates`), so
//! `--reload-templates` saves an inert `zz-seed-reload` template before the first dressing.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use benilla_protocol::{logon, messages, ServerPacket, WorldSession, WORLD_PORT};

/// The inbound `SMSG_MESSAGECHAT` type byte every dot-command reply arrives on.
const CHAT_MSG_SYSTEM: u8 = 0x0a;

/// Per class: the gear role, joined to `--tier` into a `player_premade_item_template.name`
/// (`dps-phase6-bis`), and a verbatim `player_premade_spell_template.name`, which has no tier.
const CLASSES: [(u8, &str, &str, &str); 9] = [
    // class, name, gear role, spec template
    (1, "warrior", "dps", "fury-dw-pve"),
    (2, "paladin", "tank", "protection-pve"),
    (3, "hunter", "dps", "mm-sv-pve"),
    (4, "rogue", "dps", "combat-swords-pve"),
    (5, "priest", "heal", "holy-pve"),
    (7, "shaman", "heal", "resto-pve"),
    (8, "mage", "dps", "arcane-power-frost-pve"),
    (9, "warlock", "dps", "ds-ruin-pve"),
    (11, "druid", "tank", "feral-bear-pve"),
];

/// `(race, gender)` per class, indexed like [`CLASSES`]; each spread covers all eight races in
/// legal 1.12 pairs (paladin Human/Dwarf, shaman Orc/Tauren/Troll, druid Night Elf/Tauren).
const SPREAD_A: [(u8, u8); 9] = [
    (1, 0), // human warrior
    (3, 0), // dwarf paladin
    (4, 1), // night elf hunter
    (7, 0), // gnome rogue
    (8, 1), // troll priest
    (2, 0), // orc shaman
    (5, 1), // undead mage
    (1, 1), // human warlock
    (6, 0), // tauren druid
];

/// The second spread, sharing no race/class pair with [`SPREAD_A`].
const SPREAD_B: [(u8, u8); 9] = [
    (3, 0), // dwarf warrior
    (1, 0), // human paladin
    (6, 0), // tauren hunter
    (2, 0), // orc rogue
    (5, 1), // undead priest
    (8, 1), // troll shaman
    (7, 1), // gnome mage
    (5, 0), // undead warlock
    (4, 1), // night elf druid
];

/// Race id → (display name, `game_tele` capital); gnomes have no capital row and use Ironforge.
fn race_info(id: u8) -> Option<(&'static str, &'static str)> {
    Some(match id {
        1 => ("Human", "Stormwind"),
        2 => ("Orc", "Orgrimmar"),
        3 => ("Dwarf", "Ironforge"),
        4 => ("Night Elf", "Darnassus"),
        5 => ("Undead", "Undercity"),
        6 => ("Tauren", "ThunderBluff"),
        7 => ("Gnome", "Ironforge"),
        8 => ("Troll", "Orgrimmar"),
        _ => return None,
    })
}

struct Opts {
    user: String,
    pass: String,
    prefix: String,
    host: String,
    wipe: bool,
    level: Option<u8>,
    tier: Option<String>,
    home: bool,
    reload: bool,
    spread: [(u8, u8); 9],
}

const USAGE: &str = "usage: seed_classes <user> <pass> <Prefix> [--wipe] [--level <n>] \
                     [--tier <phase6-bis|preraid-bis|r14>] [--home] [--spread <a|b>] \
                     [--reload-templates] [--host <h>]";

fn parse_opts() -> Result<Opts> {
    let mut a = std::env::args().skip(1);
    let (user, pass, prefix) = (
        a.next().context(USAGE)?,
        a.next().context(USAGE)?,
        a.next().context(USAGE)?,
    );
    let mut o = Opts {
        user,
        pass,
        prefix,
        host: "localhost".into(),
        wipe: false,
        level: None,
        tier: None,
        home: false,
        reload: false,
        spread: SPREAD_A,
    };
    while let Some(flag) = a.next() {
        match flag.as_str() {
            "--wipe" => o.wipe = true,
            "--home" => o.home = true,
            "--reload-templates" => o.reload = true,
            "--level" => o.level = Some(a.next().context("--level needs a number")?.parse()?),
            "--tier" => o.tier = Some(a.next().context("--tier needs a name")?),
            "--host" => o.host = a.next().context("--host needs a hostname")?,
            "--spread" => {
                o.spread = match a.next().context("--spread needs a|b")?.as_str() {
                    "a" | "A" => SPREAD_A,
                    "b" | "B" => SPREAD_B,
                    other => bail!("unknown spread {other:?} — expected a or b"),
                }
            }
            other => bail!("unknown flag {other:?}\n{USAGE}"),
        }
    }
    Ok(o)
}

fn connect(o: &Opts) -> Result<WorldSession> {
    let l = logon(&o.host, &o.user, &o.pass)?;
    let addr = l
        .realms
        .first()
        .map(|r| r.address.clone())
        .unwrap_or_else(|| format!("{}:{WORLD_PORT}", o.host));
    WorldSession::connect(&addr, &o.user, l.session_key)
}

/// Read for `window`, printing every system-chat line, the server's reply to a dot-command.
fn drain(s: &mut WorldSession, window: Duration) {
    let deadline = Instant::now() + window;
    while Instant::now() < deadline {
        if let Ok(ServerPacket::MessageChat(m)) = s.recv() {
            if m.chat_type == CHAT_MSG_SYSTEM {
                println!("      server says — {}", m.text);
            }
        }
    }
}

fn main() -> Result<()> {
    let o = parse_opts()?;
    let mut s = connect(&o)?;

    // ── 1 · wipe ──
    // Blocking reads: enum, create and delete loop on `recv`, where a read timeout is an error.
    s.set_read_timeout(None)?;
    let have = s.char_enum()?;
    println!("account {}: {} existing character(s)", o.user, have.len());
    if o.wipe {
        for c in &have {
            match s.delete_character(c.guid)? {
                messages::CHAR_DELETE_SUCCESS => {
                    println!("  wiped {} (guid {}, level {})", c.name, c.guid, c.level);
                }
                other => bail!("deleting {} failed: {other:#x}", c.name),
            }
        }
    }

    // ── 2 · create the nine ──
    let have = s.char_enum()?;
    for (i, (class, class_name, _, _)) in CLASSES.iter().enumerate() {
        let (race, gender) = o.spread[i];
        let (race_name, _) = race_info(race).context("spread holds a known race")?;
        if have.iter().any(|c| c.class == *class) {
            println!("  {class_name}: already present — skipped");
            continue;
        }
        let name = format!("{}{class_name}", o.prefix);
        let req = messages::CharCreateReq {
            name: name.clone(),
            race,
            class: *class,
            gender,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
        };
        match s.create_character(&req)? {
            messages::CHAR_CREATE_SUCCESS => {
                let sex = if gender == 1 { "female" } else { "male" };
                println!("  {class_name}: created {name} ({sex} {race_name})");
            }
            messages::CHAR_CREATE_NAME_IN_USE => {
                println!("  {class_name}: name {name} already in use — skipped");
            }
            0x33 => bail!(
                "{name}: CHAR_CREATE_PVP_TEAMS_VIOLATION — set AllowTwoSide.Accounts = 1 \
                 (see module doc)"
            ),
            other => bail!("{name}: create failed, result {other:#x}"),
        }
    }

    // ── 3 · dress each body ──
    // Re-enum first: `player_login` takes the chat language from the last roster, and vmangos
    // drops chat, dot-commands included, in a language the character does not know.
    let roster = s.char_enum()?;
    let mut dressed = 0usize;
    if o.level.is_some() || o.tier.is_some() || o.home {
        for (i, (class, class_name, role, spec)) in CLASSES.iter().enumerate() {
            let Some(c) = roster.iter().find(|c| c.class == *class) else {
                println!("  {class_name}: not on the account — nothing to dress");
                continue;
            };
            let (_, capital) = race_info(o.spread[i].0).context("spread holds a known race")?;
            let mut steps = Vec::new();
            if o.reload && dressed == 0 {
                // Before the first `.character premade gear`, which reads the cached templates.
                steps.push(".character premade savegear zz-seed-reload".into());
            }
            if let Some(l) = o.level {
                // Order matters: level before gear (a premade only levels up), spells after the
                // level, and the premade spec after `all_myclass` so its talent tree wins.
                steps.push(format!(".character level {l}"));
                steps.push(".learn all_myclass".into());
            }
            if let Some(t) = &o.tier {
                steps.push(format!(".character premade gear {role}-{t}"));
                steps.push(format!(".character premade spec {spec}"));
            }
            if o.home {
                steps.push(format!(".tele {capital}"));
            }

            println!("  {}: entering world…", c.name);
            s.player_login(c.guid)?;
            s.set_active_mover(c.guid)?;
            s.complete_cinematic()?; // a freshly created body is shown the intro cinematic
            s.set_read_timeout(Some(Duration::from_millis(250)))?;
            drain(&mut s, Duration::from_secs(2)); // a command into a half-built session is dropped
            for step in &steps {
                println!("    {step}");
                s.send_chat(step)?;
                drain(&mut s, Duration::from_millis(1200));
            }
            s.logout(Duration::from_secs(25))
                .with_context(|| format!("logging {} back out to character select", c.name))?;
            s.set_read_timeout(None)?;
            dressed += 1;
        }
    }

    // ── 4 · report ──
    println!("final roster on {}:", o.user);
    for c in s.char_enum()? {
        let (race_name, _) = race_info(c.race).unwrap_or(("?", "?"));
        println!(
            "  {:<12} level {:<3} {} {}",
            c.name,
            c.level,
            if c.gender == 1 { "female" } else { "male" },
            race_name,
        );
    }
    println!("dressed {dressed} character(s)");
    Ok(())
}
