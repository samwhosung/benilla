//! The state every probe shares, the acks that keep any session alive, and the [`DeathArc`].

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use anyhow::Result;
use benilla_protocol::{
    AttackSwingError, Character, EntityKind, MoveMode, ObjectFields, ServerPacket, SessionEvent,
    SpeedKind, WorldSession,
};

/// A Kobold Vermin spawn in Northshire (`creature` guid 79992): hostile, level 1, in melee reach
/// on landing. Attack, loot and death staging all teleport here.
pub(crate) const ATTACK_TP: &str = ".go xyz -8780.71 -164.568 81.94";

/// One entity decoded from the stream, as `benilla-world` reports it (raw WoW coords).
pub(crate) struct Tracked {
    pub(crate) kind: EntityKind,
    pub(crate) position: [f32; 3],
    pub(crate) orientation: f32,
}

/// The shared die, release, ghost and corpse sequence the death probes read.
#[derive(Default)]
pub(crate) struct DeathArc {
    pub(crate) die_sent: bool,
    pub(crate) died_at: Option<Instant>,
    pub(crate) death_pos: Option<[f32; 3]>,
    pub(crate) rooted_seen: bool,
    pub(crate) repop_sent: bool,
    pub(crate) unroot_seen: bool,
    pub(crate) water_walk_seen: bool,
    pub(crate) ghost_seen: bool,
    pub(crate) graveyard_pos: Option<[f32; 3]>,
    pub(crate) reclaim_delay_ms: Option<u32>,
    /// Set by `.revive` (death) or the healer activate (spirit); the ghost-clear check needs it.
    pub(crate) revive_initiated: bool,
    pub(crate) revived_seen: bool,
    /// Stop dead and unreleased for `--self-res`: the DEATH dialog exists only until release.
    pub(crate) hold_release: bool,
}

/// Everything more than one probe reads, plus the session-keeping acks and the [`DeathArc`].
pub(crate) struct World {
    pub(crate) self_guid: u64,
    pub(crate) self_name: String,
    pub(crate) self_level: u8,
    pub(crate) self_map: u32,
    /// The `SMSG_CHAR_ENUM` spawn pose, used until our own create streams.
    pub(crate) spawn_pos: [f32; 3],
    pub(crate) tracked: HashMap<u64, Tracked>,
    /// Our merged descriptor; its private equipment and pack slots are sent only to us.
    pub(crate) self_fields: Option<ObjectFields>,
    pub(crate) item_entries: HashMap<u64, u32>,
    pub(crate) item_stacks: HashMap<u64, u32>,
    pub(crate) item_names: HashMap<u32, String>,
    pub(crate) vendors: HashMap<u64, [f32; 3]>,
    /// The last same-map teleport landing point, whichever probe's staging sent it.
    pub(crate) attack_pos: Option<[f32; 3]>,
    /// Every self force-speed change seen (kind, counter, flat speed).
    pub(crate) speed_changes_seen: Vec<(SpeedKind, u32, f32)>,
    pub(crate) player_name_answer: Option<String>,
    pub(crate) creature_name_answer: Option<(u32, Option<String>)>,
    pub(crate) spell_book: Option<Vec<u32>>,
    pub(crate) bar_spells: Option<Vec<u32>>,
    pub(crate) item_asked: Option<u32>,
    pub(crate) item_answer: Option<(u32, Option<String>)>,
    pub(crate) cast_verdict: Option<(u32, bool, Option<u8>)>,
    pub(crate) targeted_verdict: Option<(u32, bool, Option<u8>)>,
    /// The `--spells` ground-cast spell, whose `SMSG_CAST_RESULT` goes to [`Self::dest_verdict`].
    pub(crate) dest_spell: Option<u32>,
    pub(crate) dest_verdict: Option<(u32, bool, Option<u8>)>,
    pub(crate) swings_seen: u32,
    /// Swing refusals: the wire has no reason byte, the opcode is the reason.
    pub(crate) swing_refusals: Vec<AttackSwingError>,
    pub(crate) self_moves: u32,
    pub(crate) tally: BTreeMap<String, u32>,
    pub(crate) total: u32,
    pub(crate) death_arc: Option<DeathArc>,
    /// Shared teleports go out once: whichever probe stages first sets the flag.
    pub(crate) attack_tp_staged: bool,
    pub(crate) mcbride_staged: bool,
}

impl World {
    pub(crate) fn new(character: &Character) -> Self {
        World {
            self_guid: character.guid,
            self_name: character.name.clone(),
            self_level: character.level,
            self_map: character.map,
            spawn_pos: [
                character.position.x,
                character.position.y,
                character.position.z,
            ],
            tracked: HashMap::new(),
            self_fields: None,
            item_entries: HashMap::new(),
            item_stacks: HashMap::new(),
            item_names: HashMap::new(),
            vendors: HashMap::new(),
            attack_pos: None,
            speed_changes_seen: Vec::new(),
            player_name_answer: None,
            creature_name_answer: None,
            spell_book: None,
            bar_spells: None,
            item_asked: None,
            item_answer: None,
            cast_verdict: None,
            targeted_verdict: None,
            dest_spell: None,
            dest_verdict: None,
            swings_seen: 0,
            swing_refusals: Vec::new(),
            self_moves: 0,
            tally: BTreeMap::new(),
            total: 0,
            death_arc: None,
            attack_tp_staged: false,
            mcbride_staged: false,
        }
    }

    /// Our pose for movement acks: the probe holds still, so the tracked position is current.
    pub(crate) fn self_pose(&self) -> ([f32; 3], f32) {
        match self.tracked.get(&self.self_guid) {
            Some(t) => (t.position, t.orientation),
            None => (self.spawn_pos, 0.0),
        }
    }

    /// Count a received packet, once per `recv` and before decode.
    pub(crate) fn tally_packet(&mut self, msg: &ServerPacket) {
        self.total += 1;
        *self.tally.entry(msg.name()).or_default() += 1;
    }

    /// The [`DeathArc`]'s staging. It runs before the probes' own, so the arc's `.revive` and
    /// teleport precede `--spirit`'s `.repairitems`.
    pub(crate) fn stage(&mut self, session: &mut WorldSession) -> Result<()> {
        if self.death_arc.is_some() {
            // Die away from any graveyard: a death at one re-pops in place, and the graveyard
            // check reads that as a missing port. `.revive` first: a prior run may leave a ghost.
            session.send_chat(".revive")?;
            session.send_chat(ATTACK_TP)?;
            self.attack_tp_staged = true;
            println!("sent GM: .revive (defensive); teleport (death staging): {ATTACK_TP}");
        }
        Ok(())
    }

    /// Every pump iteration, before recv: the [`DeathArc`] die/release steps.
    pub(crate) fn poll(&mut self, session: &mut WorldSession) -> Result<()> {
        if let Some(arc) = &mut self.death_arc {
            // `.die` with nothing selected kills the caster; sent once the staging teleport lands.
            if !arc.die_sent && self.attack_pos.is_some() {
                session.send_chat(".die")?;
                println!("sent GM: .die (self-kill)");
                arc.die_sent = true;
            }
            // Release spirit (`CMSG_REPOP_REQUEST`) once dead and rooted, in whichever order.
            if !arc.hold_release && !arc.repop_sent && arc.died_at.is_some() && arc.rooted_seen {
                session.repop_request()?;
                println!("sent CMSG_REPOP_REQUEST (release spirit)");
                arc.repop_sent = true;
            }
        }
        Ok(())
    }

    /// Each decoded event, before the probes' `on_event`: logging, session acks, death evidence.
    pub(crate) fn on_event(&mut self, ev: &SessionEvent, session: &mut WorldSession) -> Result<()> {
        match ev {
            SessionEvent::ObjectCreate {
                guid,
                kind,
                position,
                orientation,
                fields,
                ..
            } => {
                if *guid == self.self_guid {
                    self.self_fields = Some(fields.clone());
                }
                // `UNIT_NPC_FLAG_VENDOR` is 0x4; in 1.12 0x80 is innkeeper (`UnitDefines.h:659`).
                if *kind == EntityKind::Unit && fields.unit_npc_flags() & 0x4 != 0 {
                    self.vendors.insert(*guid, *position);
                }
                self.tracked.insert(
                    *guid,
                    Tracked {
                        kind: *kind,
                        position: *position,
                        orientation: *orientation,
                    },
                );
            }
            // Our inventory: position-less item/container creates.
            SessionEvent::ItemCreate { guid, fields, .. } => {
                if let Some(entry) = fields.object_entry() {
                    self.item_entries.insert(*guid, entry);
                }
                self.item_stacks
                    .insert(*guid, fields.item_stack_count().unwrap_or(1));
            }
            SessionEvent::ObjectValues { guid, fields } if *guid == self.self_guid => {
                // The server flushes an explicit UNIT_FIELD_HEALTH 0 at the instant of death; read
                // it off this delta, before the merge below.
                let pose = self.self_pose();
                if let Some(arc) = &mut self.death_arc {
                    if arc.died_at.is_none() && fields.unit_health() == Some(0) {
                        arc.died_at = Some(Instant::now());
                        let pos = pose.0;
                        arc.death_pos = Some(pos);
                        println!(
                            "UNIT_FIELD_HEALTH → 0: died at ({:.1}, {:.1}, {:.1})",
                            pos[0], pos[1], pos[2]
                        );
                    }
                }
                if let Some(sf) = &mut self.self_fields {
                    sf.merge(fields.clone());
                    // PLAYER_FLAGS_GHOST (0x10, field 190) sets at release and clears at revive.
                    // Read the merged store: a delta without PLAYER_FLAGS reads as absent.
                    if let Some(arc) = &mut self.death_arc {
                        if !arc.ghost_seen && sf.player_is_ghost() {
                            arc.ghost_seen = true;
                            println!("PLAYER_FLAGS_GHOST set — released as a ghost");
                        } else if arc.ghost_seen
                            && arc.revive_initiated
                            && !arc.revived_seen
                            && !sf.player_is_ghost()
                        {
                            arc.revived_seen = true;
                            println!("PLAYER_FLAGS_GHOST cleared — revived");
                        }
                    }
                }
            }
            SessionEvent::ObjectMove {
                guid,
                position,
                orientation,
            } => {
                if let Some(t) = self.tracked.get_mut(guid) {
                    t.position = *position;
                    t.orientation = *orientation;
                }
            }
            SessionEvent::ObjectsRemoved(guids) => {
                for g in guids {
                    self.tracked.remove(g);
                }
            }
            SessionEvent::ObjectDestroyed(guid) => {
                self.tracked.remove(guid);
            }
            SessionEvent::TimeSpeed {
                hours,
                minutes,
                timescale,
                ..
            } => {
                println!(
                    "server game-time {hours:02}:{minutes:02}  (timescale {timescale} game-min/sec)"
                );
            }
            SessionEvent::PlayerName { guid, name, .. } => {
                println!("SMSG_NAME_QUERY_RESPONSE: guid {guid} → '{name}'");
                self.player_name_answer = Some(name.clone());
            }
            SessionEvent::CreatureName { entry, name, .. } => {
                println!("SMSG_CREATURE_QUERY_RESPONSE: entry {entry} → {name:?}");
                self.creature_name_answer = Some((*entry, name.clone()));
            }
            SessionEvent::SpellBook { spell_ids, .. } => {
                println!(
                    "SMSG_INITIAL_SPELLS: {} spells (first: {:?})",
                    spell_ids.len(),
                    &spell_ids[..spell_ids.len().min(8)]
                );
                self.spell_book = Some(spell_ids.clone());
            }
            SessionEvent::ActionButtons { buttons } => {
                println!("SMSG_ACTION_BUTTONS: {} occupied slot(s)", buttons.len());
                for b in buttons.iter().take(12) {
                    println!(
                        "  slot {:>3}  action {:>5}  kind {:#04x}",
                        b.slot, b.action, b.kind
                    );
                }
                if self.item_asked.is_none() {
                    if let Some(b) = buttons.iter().find(|b| b.kind == 0x80) {
                        session.item_query(b.action, 0)?;
                        println!("sent CMSG_ITEM_QUERY_SINGLE for entry {}", b.action);
                        self.item_asked = Some(b.action);
                    }
                }
                self.bar_spells = Some(
                    buttons
                        .iter()
                        .filter(|b| b.kind == 0)
                        .map(|b| b.action)
                        .collect(),
                );
            }
            SessionEvent::Chat(m) => {
                println!(
                    "chat [{:#04x}] {}{}",
                    m.chat_type,
                    m.sender_name
                        .as_deref()
                        .map(|n| format!("{n}: "))
                        .unwrap_or_default(),
                    m.text
                );
            }
            SessionEvent::ItemTemplate { entry, info } => {
                println!(
                    "SMSG_ITEM_QUERY_SINGLE_RESPONSE: entry {entry} → {:?}",
                    info.as_ref().map(|i| (&i.name, i.class, i.subclass))
                );
                if let Some(i) = info {
                    self.item_names.insert(
                        *entry,
                        format!("{} [class {} subclass {}]", i.name, i.class, i.subclass),
                    );
                }
                self.item_answer = Some((*entry, info.as_ref().map(|i| i.name.clone())));
            }
            SessionEvent::ForceSpeedChange {
                guid,
                kind,
                counter,
                speed,
            } if *guid == self.self_guid => {
                // The ack carries our pose and a zeroed one would move us to (0,0,0), so with no
                // streamed pose there is no ack.
                let Some(t) = self.tracked.get(guid) else {
                    println!(
                        "force {kind:?} speed change before our own create streamed — cannot ack"
                    );
                    return Ok(());
                };
                session.force_speed_ack(
                    *kind,
                    *guid,
                    *counter,
                    *speed,
                    t.position,
                    t.orientation,
                )?;
                println!(
                    "SMSG_FORCE_{kind:?}_SPEED_CHANGE: counter {counter}, {speed} yd/s — acked"
                );
                self.speed_changes_seen.push((*kind, *counter, *speed));
            }
            // Until acked, `HandleMoveWorldportAckOpcode` never runs and the new map streams
            // nothing. Keep the tracked set: acks after arrival need our pose, the landing point.
            SessionEvent::Worldport {
                map_id,
                position,
                orientation,
                needs_ack,
            } => {
                self.self_map = *map_id;
                if let Some(t) = self.tracked.get_mut(&self.self_guid) {
                    t.position = *position;
                    t.orientation = *orientation;
                }
                if *needs_ack {
                    session.worldport_ack()?;
                }
                println!(
                    "SMSG_NEW_WORLD: map {map_id} at ({:.1}, {:.1}, {:.1}){}",
                    position[0],
                    position[1],
                    position[2],
                    if *needs_ack { " — ack sent" } else { "" }
                );
            }
            SessionEvent::Teleport {
                guid,
                counter,
                position,
                ..
            } if *guid == self.self_guid => {
                // Ack the same-map port; the server freezes our movement until we do.
                session.teleport_ack(*guid, *counter)?;
                self.attack_pos = Some(*position);
                if let Some(t) = self.tracked.get_mut(guid) {
                    t.position = *position;
                }
                println!(
                    "teleported to ({:.1}, {:.1}, {:.1}) — ack sent",
                    position[0], position[1], position[2]
                );
                // The graveyard port after release is this same event, already acked above.
                if let Some(arc) = &mut self.death_arc {
                    if arc.repop_sent && arc.graveyard_pos.is_none() {
                        arc.graveyard_pos = Some(*position);
                        println!(
                            "graveyard teleport: ({:.1}, {:.1}, {:.1})",
                            position[0], position[1], position[2]
                        );
                    }
                }
            }
            SessionEvent::AttackStart { attacker, victim } => {
                println!("SMSG_ATTACKSTART: {attacker:#x} → {victim:#x}");
            }
            SessionEvent::AttackStop { attacker, victim } => {
                println!("SMSG_ATTACKSTOP: {attacker:#x} → {victim:#x}");
            }
            SessionEvent::AttackerState(s) => {
                self.swings_seen += 1;
                println!(
                    "SMSG_ATTACKERSTATEUPDATE: {:#x} → {:#x}  hitInfo {:#x}  damage {}  victimState {}",
                    s.attacker, s.victim, s.hit_info, s.damage, s.victim_state
                );
            }
            SessionEvent::AttackSwingError(e) => {
                self.swing_refusals.push(*e);
                println!("SMSG_ATTACKSWING refusal: {e:?}");
            }
            SessionEvent::CastResult {
                spell_id,
                success,
                reason,
                arg,
            } => {
                println!(
                    "SMSG_CAST_RESULT: spell {spell_id} success={success} reason={reason:?} arg={arg:?}"
                );
                if self.dest_spell == Some(*spell_id) {
                    self.dest_verdict = Some((*spell_id, *success, *reason));
                } else if self.cast_verdict.is_none() {
                    self.cast_verdict = Some((*spell_id, *success, *reason));
                } else {
                    self.targeted_verdict = Some((*spell_id, *success, *reason));
                }
            }
            SessionEvent::MonsterMove {
                guid,
                start,
                spline_id,
                path,
                facing,
                stop,
                duration_ms,
                flying,
                ..
            } => {
                let mine = *guid == self.self_guid;
                if mine {
                    self.self_moves += 1;
                }
                let speed = {
                    let len: f32 = path
                        .windows(2)
                        .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
                        .sum();
                    len / ((*duration_ms).max(1) as f32 / 1000.0)
                };
                println!(
                    "SMSG_MONSTER_MOVE {}{guid:#x}: splineId {spline_id}, {} pts, facing {facing:?}, {duration_ms}ms, flying={flying}, stop={stop}, ~{speed:.1} yd/s",
                    if mine { "[SELF] " } else { "" },
                    path.len(),
                );
                if mine {
                    println!(
                        "    start ({:.2}, {:.2}, {:.2})",
                        start[0], start[1], start[2]
                    );
                    for (i, p) in path.iter().enumerate() {
                        println!("    pt[{i}] ({:.2}, {:.2}, {:.2})", p[0], p[1], p[2]);
                    }
                }
            }
            // Root, water walk, feather fall and hover need an ack echoing the counter with our
            // pose, or the server never applies them; the 1.12 client always acks.
            SessionEvent::MoveMode {
                guid,
                counter,
                mode,
                apply,
            } if *guid == self.self_guid => {
                let pose = self.self_pose();
                // The ack's MovementInfo must carry the mode's own bit: vmangos kicks a player
                // whose root ack lacks it (`HandleMoveRootAck`, MovementHandler.cpp:715-722). For
                // the other modes the word becomes the mover's flags; the probe keeps no others.
                let flags = if *apply { mode.flag() } else { 0 };
                session.move_mode_ack(*guid, *counter, *mode, *apply, flags, pose)?;
                if let Some(arc) = &mut self.death_arc {
                    match (mode, apply) {
                        (MoveMode::Root, true) => arc.rooted_seen = true,
                        (MoveMode::Root, false) => arc.unroot_seen = true,
                        (MoveMode::WaterWalk, true) => arc.water_walk_seen = true,
                        _ => {}
                    }
                }
                println!(
                    "mover mode {mode:?} {} — acked",
                    if *apply { "granted" } else { "revoked" }
                );
            }
            SessionEvent::CorpseReclaimDelay { delay_ms } => {
                if let Some(arc) = &mut self.death_arc {
                    arc.reclaim_delay_ms = Some(*delay_ms);
                    println!("SMSG_CORPSE_RECLAIM_DELAY: {delay_ms} ms");
                }
            }
            _ => {}
        }
        Ok(())
    }
}
