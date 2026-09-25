//! `benilla-world`: logs in to realmd, enters the world server as a character, and tallies the
//! updates it pushes; the `--<probe>` flags live-verify one wire each.
//!
//! Example: `cargo run -p benilla-protocol --bin benilla-world -- <account> <password> localhost`
//!
//! An account with no character gets a Human Warrior (`--create <name>`). The world address
//! defaults to the realm list's (`--world` overrides).

mod probes;
mod world;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use benilla_protocol::messages::{
    CharCreateReq, CHAR_CREATE_NAME_IN_USE, CHAR_CREATE_SUCCESS, CLASS_WARRIOR, GENDER_MALE,
    RACE_HUMAN,
};
use benilla_protocol::{decode, EntityKind, WorldSession, WORLD_PORT};
use clap::Parser;

use probes::{
    Attack, Aura, Charge, Ctx, Death, EquipPackSlot, GiverStatus, GroundFx, Loot, MountTele,
    OpenItem, Probe, QueryNames, Quest, QuestItem, QuestLog, QuestTimer, SelfRes, Speed, Spells,
    Spirit, SwapPackSlots, UsePackSlot, Vendor, WorldState,
};
use world::{DeathArc, Tracked, World};

/// Connect to a WoW 1.12.1 world server, log in a character, and stream object updates.
#[derive(Parser)]
#[command(name = "benilla-world", version, about)]
struct Cli {
    /// Account name.
    username: String,
    /// Account password.
    password: String,
    /// Auth (realmd) server host.
    #[arg(default_value = "localhost")]
    host: String,
    /// World server address (`host:port`). Defaults to the realm list's advertised address.
    #[arg(long)]
    world: Option<String>,
    /// Character name to create if the account has none.
    #[arg(long, default_value = "One")]
    create: String,
    /// How many seconds to stream packets after entering the world.
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    /// After streaming, walk this many yards forward, log out, and check the server saved the move.
    #[arg(long)]
    walk: Option<f32>,
    /// Live-verify `CMSG_NAME_QUERY` (our own name) and `CMSG_CREATURE_QUERY` (the first creature).
    #[arg(long)]
    query_names: bool,
    /// Live-verify `SMSG_INITIAL_SPELLS` and `SMSG_ACTION_BUTTONS` at login, then require an
    /// `SMSG_CAST_RESULT` for a self cast, a ground cast and a targeted cast.
    #[arg(long)]
    spells: bool,
    /// Capture a ground cast of this spell id at our feet (target mask 0x40): every DynamicObject
    /// create, the SPELL_GO and the removal. Needs GM; pair with `--seconds 25`.
    #[arg(long)]
    groundfx: Option<u32>,
    /// Live-verify melee: teleport to the Northshire kobolds, `CMSG_ATTACKSWING` the nearest
    /// creature and require an `SMSG_ATTACKERSTATEUPDATE`. Needs GM.
    #[arg(long)]
    attack: bool,
    /// Live-verify `CMSG_USE_ITEM` on this 1-based backpack slot (wire bag 255, slot 22 + n):
    /// require a stack delta, a destroy, or a cast-result refusal.
    #[arg(long)]
    use_pack_slot: Option<u8>,
    /// Live-verify `CMSG_OPEN_ITEM` on an added Small Barnacled Clam (7973, lootable, no lock),
    /// requiring `SMSG_LOOT_RESPONSE` on the item's own guid. A bag right-click on an openable
    /// item sends this, never `CMSG_USE_ITEM`. Needs GM.
    #[arg(long)]
    open_item: bool,
    /// Say this line after entering the world, e.g. a GM dot-command; repeatable, sent in order.
    #[arg(long)]
    say: Vec<String>,
    /// Live-verify `CMSG_AUTOEQUIP_ITEM` on this 1-based backpack slot: require the item in an
    /// equipment slot or an `SMSG_INVENTORY_CHANGE_FAILURE`.
    #[arg(long)]
    equip_pack_slot: Option<u8>,
    /// Live-verify `CMSG_SWAP_INV_ITEM` on 1-based backpack slots `A:B`, then swap back. A must be
    /// occupied; an empty B makes it a move.
    #[arg(long, value_parser = parse_slot_pair)]
    swap_pack_slots: Option<(u8, u8)>,
    /// Live-verify the vendor wire on the nearest vendor: `CMSG_LIST_INVENTORY`, then buy the
    /// cheapest row and require the item, a stock update or `SMSG_BUY_FAILED`. No GM needed.
    #[arg(long)]
    vendor: bool,
    /// Live-verify solo loot: GM-kill the nearest creature (or `--loot-guid`), loot every row and
    /// the money, and require `SMSG_LOOT_RELEASE_RESPONSE`. Needs GM.
    #[arg(long)]
    loot: bool,
    /// Loot this guid for `--loot` (decimal or `0x` hex) instead of the nearest creature.
    #[arg(long, value_parser = parse_guid)]
    loot_guid: Option<u64>,
    /// Live-verify the questgiver wire with quest 783, from Deputy Willem (823) to Marshal McBride
    /// (197): details, accept, GM-complete, reward, requiring `SMSG_QUESTGIVER_QUEST_COMPLETE` and
    /// an XP delta. Needs GM.
    #[arg(long)]
    quest: bool,
    /// Live-verify the quest log with quest 7 (10 kills of entry 6, from Marshal McBride):
    /// `CMSG_QUEST_QUERY` must parse a real objective, `.quest complete 7` must set the slot's
    /// `COMPLETE` byte, and `CMSG_QUESTLOG_REMOVE_QUEST` must clear the slot, which has no ack.
    /// Needs GM.
    #[arg(long)]
    questlog: bool,
    /// Live-verify a timed quest: its `PLAYER_QUEST_LOG` timer is an absolute unix stamp, and
    /// minus `CMSG_QUERY_TIME`'s server clock it must fall inside the template's `limit_time`.
    /// Needs GM.
    #[arg(long)]
    questtimer: bool,
    /// Live-verify a quest-starter item, the Northshire Gift Voucher (14646, quest 5805): a bag
    /// right-click sends `CMSG_QUESTGIVER_QUERY_QUEST` to the item's guid, never `CMSG_USE_ITEM`
    /// (refused with `EQUIP_ERR_ITEM_NOT_FOUND`); accepting must log the quest and keep the item,
    /// which 5805 requires. Needs GM.
    #[arg(long)]
    quest_item: bool,
    /// Live-verify `SMSG_FORCE_RUN_SPEED_CHANGE` via `.modify speed 1.5` (10.5 = 1.5 × 7.0) and
    /// back to 1, acking each. A malformed ack drops the session, so surviving both is the proof.
    /// Needs GM.
    #[arg(long)]
    speed: bool,

    /// Live-verify a dismounting teleport: `.aura 458` (Brown Horse, a real mount aura, unlike
    /// `.modify mount`), then `.go xyz` into Ragefire Chasm (map 389, no mounts). Requires
    /// `SMSG_NEW_WORLD`, our create block still at mounted speed, then
    /// `SMSG_FORCE_RUN_SPEED_CHANGE` back to 7.0, both written by one
    /// `HandleMoveWorldportAckOpcode`. Needs GM; pair with `--seconds 30`.
    #[arg(long)]
    mount_tele: bool,

    /// Live-verify auras: `.aura` Mark of the Wild (1126) and Shadow Word: Pain (589), requiring
    /// each in `UNIT_FIELD_AURA` (buffs 0–31, debuffs 32–47) with the cancelable flag on the buff
    /// only, our level, a stack of 1 (wire `count - 1`), and its `SMSG_UPDATE_AURA_DURATION`
    /// before the descriptor delta. Needs GM.
    #[arg(long)]
    aura: bool,

    /// Live-verify `SMSG_QUESTGIVER_STATUS` for Marshal McBride after `.quest remove 7`, so an
    /// unaccepted quest 7 reads AVAILABLE (5). Needs GM.
    #[arg(long)]
    giverstatus: bool,

    /// Live-verify `SMSG_INIT_WORLD_STATES` (a zone change into Stormwind) and
    /// `SMSG_UPDATE_WORLD_STATE` (`.debug send worldstate`, which needs gmlevel 5).
    #[arg(long)]
    worldstate: bool,

    /// Live-verify Charge (spell 100): charge a creature 8–25 yd away and require an
    /// `SMSG_MONSTER_MOVE` for our own guid, the same server spline as any creature's; prints it
    /// in full. Needs a GM warrior.
    #[arg(long)]
    charge: bool,

    /// Live-verify death: `.die`, release, then the ghost transition (unroot, water walk,
    /// `SMSG_CORPSE_RECLAIM_DELAY`, `PLAYER_FLAGS_GHOST`, the corpse, the graveyard teleport),
    /// `MSG_CORPSE_QUERY` and `.revive`. Pair with `--seconds 45`. Needs GM.
    #[arg(long)]
    death: bool,

    /// Live-verify `CMSG_SELF_RES`: arm Reincarnation (`.learn 20608`, `.additem 17030`; vmangos
    /// checks no class), die unreleased, require `PLAYER_SELF_RES_SPELL` 21169, then stand up
    /// without becoming a ghost. Pair with `--seconds 30`. Needs GM; excludes `--death` and
    /// `--spirit`, which release.
    #[arg(long)]
    self_res: bool,

    /// Live-verify the spirit-healer res: die, release, `CMSG_SPIRIT_HEALER_ACTIVATE`, requiring
    /// the ghost flag to clear and the 25% durability loss as `ITEM_FIELD_DURABILITY` deltas.
    /// Pair with `--seconds 45`. Needs GM.
    #[arg(long)]
    spirit: bool,
}

/// Parse a `--loot-guid` value: decimal, or `0x`-prefixed hex.
fn parse_guid(s: &str) -> Result<u64, String> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| e.to_string())
    } else {
        s.parse::<u64>().map_err(|e| e.to_string())
    }
}

/// Parse a `--swap-pack-slots` value: two 1-based backpack slots as `A:B` (or `A,B`).
fn parse_slot_pair(s: &str) -> Result<(u8, u8), String> {
    let (a, b) = s
        .split_once([':', ','])
        .ok_or_else(|| format!("expected two slots like '1:2', got '{s}'"))?;
    let a: u8 = a.trim().parse().map_err(|e| format!("slot A: {e}"))?;
    let b: u8 = b.trim().parse().map_err(|e| format!("slot B: {e}"))?;
    if a == 0 || b == 0 || a == b {
        return Err("slots are 1-based and must differ".into());
    }
    Ok((a, b))
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // 1. realmd logon: session key and realm list.
    let logon = benilla_protocol::logon(&cli.host, &cli.username, &cli.password)?;
    println!("authenticated as '{}'", cli.username);

    // 2. The world address: the override, else the first realm's, else the auth host's.
    let world_addr = match (&cli.world, logon.realms.first()) {
        (Some(addr), _) => addr.clone(),
        (None, Some(realm)) => {
            println!("realm '{}' @ {}", realm.name, realm.address);
            realm.address.clone()
        }
        (None, None) => format!("{}:{}", cli.host, WORLD_PORT),
    };

    // 3. World handshake (CMSG_AUTH_SESSION + header obfuscation).
    let mut session = WorldSession::connect(&world_addr, &cli.username, logon.session_key)
        .with_context(|| format!("world handshake with {world_addr}"))?;
    println!("world handshake OK ({world_addr}) — header obfuscation active");

    // 4. Character list; create one if empty.
    let mut characters = session.char_enum()?;
    println!("{} character(s) on the account", characters.len());
    if characters.is_empty() {
        println!("creating character '{}' (Human Warrior)…", cli.create);
        let req = CharCreateReq {
            name: cli.create.clone(),
            race: RACE_HUMAN,
            class: CLASS_WARRIOR,
            gender: GENDER_MALE,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
        };
        match session.create_character(&req)? {
            CHAR_CREATE_SUCCESS | CHAR_CREATE_NAME_IN_USE => {}
            other => bail!("character creation failed: result {other:#x}"),
        }
        characters = session.char_enum()?;
        if characters.is_empty() {
            bail!("still no characters after creation");
        }
    }
    let character = &characters[0];
    println!(
        "logging in '{}' (guid {}, level {}, map {})",
        character.name, character.guid, character.level, character.map,
    );

    // 5. Enter the world and claim the mover: vmangos ignores `MSG_MOVE_*` until then, and the
    //    1.12 client sends `CMSG_SET_ACTIVE_MOVER` on login.
    session.player_login(character.guid)?;
    session.set_active_mover(character.guid)?;

    // 6. Stream packets, tallying opcodes and tracking entity positions.
    session.set_read_timeout(Some(Duration::from_secs(2)))?;
    let deadline = Instant::now() + Duration::from_secs(cli.seconds);

    // The world state every probe reads; the death arc is staged for the three death probes.
    let mut world = World::new(character);
    if cli.death || cli.spirit || cli.self_res {
        world.death_arc = Some(DeathArc {
            // --self-res needs the dead, unreleased state, so the arc must not repop.
            hold_release: cli.self_res,
            ..DeathArc::default()
        });
    }

    // One probe per flag; registry order is the poll, on_event, verify and output order.
    let mut probes: Vec<Box<dyn Probe>> = Vec::new();
    if cli.attack {
        probes.push(Box::new(Attack::default()));
    }
    if cli.charge {
        probes.push(Box::new(Charge::default()));
    }
    if cli.query_names {
        probes.push(Box::new(QueryNames::default()));
    }
    if cli.spells {
        probes.push(Box::new(Spells::default()));
    }
    if let Some(spell) = cli.groundfx {
        probes.push(Box::new(GroundFx::new(spell)));
    }
    if let Some(n) = cli.use_pack_slot {
        probes.push(Box::new(UsePackSlot { n }));
    }
    if cli.open_item {
        probes.push(Box::new(OpenItem));
    }
    if let Some(n) = cli.equip_pack_slot {
        probes.push(Box::new(EquipPackSlot { n }));
    }
    if let Some((a, b)) = cli.swap_pack_slots {
        probes.push(Box::new(SwapPackSlots { a, b }));
    }
    if cli.vendor {
        probes.push(Box::new(Vendor));
    }
    if cli.quest {
        probes.push(Box::new(Quest));
    }
    if cli.aura {
        probes.push(Box::new(Aura));
    }
    if cli.giverstatus {
        probes.push(Box::new(GiverStatus));
    }
    if cli.quest_item {
        probes.push(Box::new(QuestItem));
    }
    if cli.questlog {
        probes.push(Box::new(QuestLog));
    }
    if cli.questtimer {
        probes.push(Box::new(QuestTimer::default()));
    }
    if cli.loot {
        probes.push(Box::new(Loot {
            loot_guid: cli.loot_guid,
        }));
    }
    if cli.speed {
        probes.push(Box::new(Speed::default()));
    }
    if cli.mount_tele {
        probes.push(Box::new(MountTele::default()));
    }
    if cli.death {
        probes.push(Box::new(Death::default()));
    }
    if cli.spirit {
        probes.push(Box::new(Spirit::default()));
    }
    if cli.self_res {
        probes.push(Box::new(SelfRes::default()));
    }
    if cli.worldstate {
        probes.push(Box::new(WorldState::default()));
    }

    // Staging order: the death arc (its `.revive` and teleport precede --spirit's `.repairitems`),
    // each probe in registry order, then the `--say` lines.
    world.stage(&mut session)?;
    for probe in probes.iter_mut() {
        let mut cx = Ctx {
            session: &mut session,
            world: &mut world,
        };
        probe.stage(&mut cx)?;
    }
    for line in &cli.say {
        session.send_chat(line)?;
        println!("sent chat: {line}");
    }

    println!("streaming world packets for {}s…", cli.seconds);
    while Instant::now() < deadline {
        world.poll(&mut session)?;
        for probe in probes.iter_mut() {
            let mut cx = Ctx {
                session: &mut session,
                world: &mut world,
            };
            probe.poll(&mut cx)?;
        }
        match session.recv() {
            Ok(msg) => {
                world.tally_packet(&msg);
                for ev in decode(msg) {
                    world.on_event(&ev, &mut session)?;
                    for probe in probes.iter_mut() {
                        let mut cx = Ctx {
                            session: &mut session,
                            world: &mut world,
                        };
                        probe.on_event(&ev, &mut cx)?;
                    }
                }
            }
            // Timeout / quiet stream: keep waiting until the deadline.
            Err(_) => continue,
        }
    }

    let total = world.total;
    println!("\n--- received {total} packet(s) ---");
    for (opcode, count) in &world.tally {
        println!("  {count:>4}  {opcode}");
    }

    // Report the entities we decoded, with raw WoW coordinates.
    let self_guid = world.self_guid;
    println!("\n--- tracked {} entit(ies) ---", world.tracked.len());
    // Every `EntityKind`: this list is not a `match`, so a new variant must be added by hand.
    for kind in [
        EntityKind::Player,
        EntityKind::Unit,
        EntityKind::GameObject,
        EntityKind::DynamicObject,
        EntityKind::Corpse,
        EntityKind::Other,
    ] {
        let mut group: Vec<(&u64, &Tracked)> = world
            .tracked
            .iter()
            .filter(|(_, t)| t.kind == kind)
            .collect();
        if group.is_empty() {
            continue;
        }
        group.sort_by_key(|(guid, _)| **guid);
        println!("{:?} ({}):", kind, group.len());
        for (guid, t) in group.iter().take(8) {
            let me = if **guid == self_guid { " (self)" } else { "" };
            println!(
                "  guid {:<10} pos ({:>9.2}, {:>9.2}, {:>9.2}) o={:>5.2}{}",
                guid, t.position[0], t.position[1], t.position[2], t.orientation, me,
            );
        }
        if group.len() > 8 {
            println!("  … and {} more", group.len() - 8);
        }
    }

    let units = world
        .tracked
        .values()
        .filter(|t| t.kind == EntityKind::Unit)
        .count();
    let have_self = world.tracked.contains_key(&self_guid);
    if units == 0 && !have_self {
        bail!("decoded no positioned entities");
    }
    println!(
        "\n✅ decode: tracked {} entit(ies) with positions (self {}, {} unit/NPC).",
        world.tracked.len(),
        if have_self { "found" } else { "missing" },
        units,
    );

    // Post-stream verification: each probe's assertions + follow-up round trips, in registry order.
    for probe in probes.iter_mut() {
        let mut cx = Ctx {
            session: &mut session,
            world: &mut world,
        };
        probe.verify(&mut cx)?;
    }

    // 7. Optional walk, then log out (which saves the character) and compare the saved position.
    //    It ends the session, so it runs last.
    if let Some(yards) = cli.walk {
        let self_entity = world.tracked.get(&self_guid);
        let start = self_entity.map(|t| t.position).unwrap_or(world.spawn_pos);
        let orientation = self_entity.map(|t| t.orientation).unwrap_or(0.0);
        println!(
            "\nwalking {yards:.1} yd forward from ({:.2}, {:.2}, {:.2})…",
            start[0], start[1], start[2]
        );
        let end = walk_forward(&mut session, start, orientation, yards)?;

        // Drain a few seconds of server packets to see any reaction (teleport-back / correction).
        let drain_until = Instant::now() + Duration::from_secs(3);
        let mut reactions: BTreeMap<String, u32> = BTreeMap::new();
        while Instant::now() < drain_until {
            if let Ok(msg) = session.recv() {
                let name = msg.name();
                if name.contains("MOVE") || name.contains("TELEPORT") || name.contains("FORCE") {
                    *reactions.entry(name).or_default() += 1;
                }
            }
        }
        if reactions.is_empty() {
            println!("(no movement reaction packets from server)");
        } else {
            println!("server movement reactions: {reactions:?}");
        }

        // Log out (persists the character) and read the saved position back.
        session.logout(Duration::from_secs(25))?;
        let after = session.char_enum()?;
        let saved = after
            .iter()
            .find(|c| c.guid == self_guid)
            .map(|c| c.position)
            .context("character vanished after logout")?;

        let moved = ((saved.x - start[0]).powi(2)
            + (saved.y - start[1]).powi(2)
            + (saved.z - start[2]).powi(2))
        .sqrt();
        println!(
            "sent to   ({:.2}, {:.2}, {:.2})\nsaved as  ({:.2}, {:.2}, {:.2})  → moved {moved:.2} yd",
            end[0], end[1], end[2], saved.x, saved.y, saved.z,
        );
        if moved < 1.0 {
            bail!("server did not persist the move (snapped back?) — moved only {moved:.2} yd");
        }
        println!("\n✅ movement: server persisted our movement ({moved:.2} yd).");
    }

    Ok(())
}

/// Walk `yards` forward as start, heartbeats and stop at run speed, returning the destination.
/// Orientation 0 faces +X, so forward is `(cos o, sin o, 0)`, in raw WoW yards.
fn walk_forward(
    session: &mut WorldSession,
    start: [f32; 3],
    orientation: f32,
    yards: f32,
) -> Result<[f32; 3]> {
    const RUN_SPEED: f32 = 7.0; // yd/s, vanilla default run speed
    const STEPS: u32 = 8;

    let (dx, dy) = (orientation.cos(), orientation.sin());
    let step_dist = yards / STEPS as f32;
    let dt = Duration::from_secs_f32((step_dist / RUN_SPEED).max(0.05));

    let pos_at = |i: u32| {
        [
            start[0] + dx * step_dist * i as f32,
            start[1] + dy * step_dist * i as f32,
            start[2],
        ]
    };

    session.start_forward(start, orientation)?;
    for i in 1..STEPS {
        std::thread::sleep(dt);
        session.heartbeat(pos_at(i), orientation)?;
    }
    std::thread::sleep(dt);
    let end = pos_at(STEPS);
    session.stop(end, orientation)?;
    Ok(end)
}
