//! `--loot`: the solo-loot wire. GM-kills a target, waits for it to turn lootable, teleports onto
//! the corpse, then `CMSG_LOOT`, stores every row, takes the money and releases, printing every
//! loot packet.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use benilla_protocol::{decode, guid, EntityKind, SessionEvent};

use crate::probes::{Ctx, Probe};
use crate::world::ATTACK_TP;

pub(crate) struct Loot {
    pub(crate) loot_guid: Option<u64>,
}

impl Probe for Loot {
    fn stage(&mut self, cx: &mut Ctx) -> Result<()> {
        // Without --loot-guid, use the --attack teleport for a creature in range; sent once.
        if self.loot_guid.is_none() && !cx.world.attack_tp_staged {
            cx.session.send_chat(ATTACK_TP)?;
            cx.world.attack_tp_staged = true;
            println!("sent GM teleport: {ATTACK_TP}");
        }
        Ok(())
    }

    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let world = &mut *cx.world;
        let session = &mut *cx.session;
        let self_guid = world.self_guid;

        // `.damage` hits the selected unit at any range. The nearest three creatures are tried,
        // since the nearest may be a corpse an earlier run already looted.
        let candidates: Vec<u64> = if let Some(g) = self.loot_guid {
            vec![g]
        } else {
            let pos = world
                .attack_pos
                .context("--loot: the GM teleport never landed (is the account gmlevel >= 2?)")?;
            let mut by_distance: Vec<(u64, f32)> = world
                .tracked
                .iter()
                .filter(|(g, t)| t.kind == EntityKind::Unit && guid::is_creature_or_pet(**g))
                .map(|(&g, t)| {
                    let d = (t.position[0] - pos[0]).hypot(t.position[1] - pos[1]);
                    (g, d)
                })
                .collect();
            by_distance.sort_by(|a, b| a.1.total_cmp(&b.1));
            by_distance.truncate(3);
            by_distance.iter().map(|(g, _)| *g).collect()
        };
        if candidates.is_empty() {
            bail!("--loot: no creature streamed in (try --loot-guid)");
        }

        // Kill until one turns lootable: `UNIT_DYNFLAG_LOOTABLE` 0x1 (`SharedDefines.h:1153`).
        let mut target = 0u64;
        let mut lootable = false;
        for &candidate in &candidates {
            println!(
                "\n--loot target: guid {candidate:#x} — selecting + GM-killing (.damage 10000)"
            );
            session.set_selection(candidate)?;
            session.send_chat(".damage 10000")?;
            let drain_until = Instant::now() + Duration::from_secs(10);
            while Instant::now() < drain_until && !lootable {
                let Ok(msg) = session.recv() else { continue };
                for ev in decode(msg) {
                    if let SessionEvent::ObjectValues { guid: g, fields } = &ev {
                        if *g == candidate && fields.unit_dynamic_flags() & 0x1 != 0 {
                            lootable = true;
                        }
                    }
                }
            }
            if lootable {
                target = candidate;
                break;
            }
            println!(
                "{candidate:#x} never went lootable within 10s (already-looted corpse, or no loot \
                 table) — trying the next candidate…"
            );
        }
        if !lootable {
            bail!(
                "--loot: none of {} candidate(s) went lootable (all already-looted corpses, or the \
                 .damage command didn't land — is the account a GM?)",
                candidates.len()
            );
        }
        println!("✅ {target:#x} is lootable (UNIT_DYNFLAG_LOOTABLE set)");

        // Teleport onto the corpse: loot range is combat reach (`Player.cpp:15472-15476`), and a
        // mid-spline kill leaves the tracked position at the spline's last waypoint, so a TOO_FAR
        // refusal retries from the refreshed spot.
        let mut response: Option<(u8, u32, Vec<benilla_protocol::messages::LootItem>)> = None;
        let mut loot_refusal: Option<u8> = None;
        for attempt in 1..=3 {
            if let Some(corpse_pos) = world.tracked.get(&target).map(|t| t.position) {
                let tp = format!(
                    ".go xyz {:.2} {:.2} {:.2}",
                    corpse_pos[0], corpse_pos[1], corpse_pos[2]
                );
                session.send_chat(&tp)?;
                println!("sent GM teleport onto the corpse: {tp}");
                let mut landed = false;
                let drain_until = Instant::now() + Duration::from_secs(5);
                while Instant::now() < drain_until && !landed {
                    let Ok(msg) = session.recv() else { continue };
                    for ev in decode(msg) {
                        if let SessionEvent::Teleport {
                            guid: g, counter, ..
                        } = ev
                        {
                            if g == self_guid {
                                session.teleport_ack(g, counter)?;
                                landed = true;
                            }
                        }
                    }
                }
                if !landed {
                    bail!("--loot: the corpse-repositioning teleport never landed");
                }
            }

            println!("sending CMSG_LOOT");
            loot_refusal = None;
            session.loot(target)?;

            // Drain for either `SMSG_LOOT_RESPONSE` shape, refreshing positions for a retry.
            let drain_until = Instant::now() + Duration::from_secs(5);
            while Instant::now() < drain_until && response.is_none() && loot_refusal.is_none() {
                let Ok(msg) = session.recv() else { continue };
                for ev in decode(msg) {
                    match ev {
                        SessionEvent::LootResponse {
                            guid: g,
                            loot_type,
                            gold,
                            items,
                        } if g == target => {
                            println!(
                                "SMSG_LOOT_RESPONSE: guid {g:#x} type {loot_type} gold {gold} {} item(s)",
                                items.len()
                            );
                            for it in &items {
                                println!(
                                    "  slot {:>2}  item {:>6}  x{:<3}  display {:>6}  randProp {:>4}  slotType {}",
                                    it.slot,
                                    it.item_id,
                                    it.count,
                                    it.display_info_id,
                                    it.random_property_id,
                                    it.slot_type
                                );
                            }
                            response = Some((loot_type, gold, items));
                        }
                        // The master-loot list arrives ahead of its response and carries no loot
                        // guid, so it is printed unconditionally.
                        SessionEvent::LootMasterList { candidates } => {
                            println!(
                                "SMSG_LOOT_MASTER_LIST: {} eligible looter(s)",
                                candidates.len()
                            );
                            for (i, guid) in candidates.iter().enumerate() {
                                println!("  candidate {:>2}  guid {guid:#x}", i + 1);
                            }
                        }
                        SessionEvent::LootError { guid: g, error } if g == target => {
                            println!("SMSG_LOOT_RESPONSE (error): guid {g:#x} error {error}");
                            loot_refusal = Some(error);
                        }
                        SessionEvent::ObjectMove {
                            guid: g,
                            position,
                            orientation,
                        } => {
                            if let Some(t) = world.tracked.get_mut(&g) {
                                t.position = position;
                                t.orientation = orientation;
                            }
                        }
                        _ => {}
                    }
                }
            }
            match loot_refusal {
                Some(error)
                    if error == benilla_protocol::messages::loot_error::TOO_FAR && attempt < 3 =>
                {
                    println!("LOOT_ERROR TOO_FAR — refreshing the corpse position and retrying…");
                }
                _ => break,
            }
        }
        if let Some(error) = loot_refusal {
            bail!("--loot: {target:#x} answered LOOT_ERROR {error} instead of opening the window");
        }
        let (loot_type, gold, items) =
            response.context("--loot: no SMSG_LOOT_RESPONSE arrived within 5s")?;

        // Store every row; each drives LOOT_REMOVED, ITEM_PUSH_RESULT and a bag ItemCreate.
        let mut removed_slots: Vec<u8> = Vec::new();
        let mut pushes: Vec<benilla_protocol::messages::ItemPushResult> = Vec::new();
        let mut items_created: Vec<u64> = Vec::new();
        for it in &items {
            session.autostore_loot_item(it.slot)?;
            println!(
                "sent CMSG_AUTOSTORE_LOOT_ITEM slot {} (item {})",
                it.slot, it.item_id
            );
            let drain_until = Instant::now() + Duration::from_secs(5);
            let mut this_removed = false;
            let mut this_pushed = false;
            while Instant::now() < drain_until && !(this_removed && this_pushed) {
                let Ok(msg) = session.recv() else { continue };
                for ev in decode(msg) {
                    match ev {
                        SessionEvent::LootRemoved { slot } if slot == it.slot => {
                            println!("SMSG_LOOT_REMOVED: slot {slot}");
                            removed_slots.push(slot);
                            this_removed = true;
                        }
                        SessionEvent::ItemPushResult(p) if p.item_entry == it.item_id => {
                            println!(
                                "SMSG_ITEM_PUSH_RESULT: entry {} x{} bagSlot {} itemSlot {:#x} from_npc {} created {}",
                                p.item_entry, p.count, p.bag_slot, p.item_slot, p.from_npc, p.created
                            );
                            pushes.push(p);
                            this_pushed = true;
                        }
                        SessionEvent::ItemCreate {
                            guid: g, fields, ..
                        } if fields.object_entry() == Some(it.item_id) => {
                            println!("ItemCreate: guid {g:#x} entry {}", it.item_id);
                            items_created.push(g);
                        }
                        _ => {}
                    }
                }
            }
            if !this_removed {
                bail!(
                    "--loot: no SMSG_LOOT_REMOVED for slot {} within 5s",
                    it.slot
                );
            }
            if !this_pushed {
                bail!(
                    "--loot: no SMSG_ITEM_PUSH_RESULT for item {} within 5s",
                    it.item_id
                );
            }
        }
        println!(
            "✅ loot items: {} row(s) taken, {} ITEM_PUSH_RESULT(s), {} bag ItemCreate(s).",
            removed_slots.len(),
            pushes.len(),
            items_created.len()
        );

        // vmangos sends no `SMSG_LOOT_MONEY_NOTIFY` to a solo looter (commented out in
        // `HandleLootMoneyOpcode`): `SMSG_LOOT_CLEAR_MONEY` comes at once and the money as a later
        // `PLAYER_FIELD_COINAGE` delta. The notify still counts, for group loot.
        if gold > 0 {
            let money_before = world.self_fields.as_ref().and_then(|sf| sf.player_money());
            println!(
                "sending CMSG_LOOT_MONEY (gold {gold}); PLAYER_FIELD_COINAGE before = {money_before:?}"
            );
            session.loot_money()?;
            let mut notify: Option<u32> = None;
            let mut cleared = false;
            let mut money_after: Option<u32> = None;
            let drain_until = Instant::now() + Duration::from_secs(15);
            while Instant::now() < drain_until
                && !(cleared && (notify.is_some() || money_after.is_some()))
            {
                let Ok(msg) = session.recv() else { continue };
                for ev in decode(msg) {
                    match ev {
                        SessionEvent::LootMoneyNotify { amount } => {
                            println!("SMSG_LOOT_MONEY_NOTIFY: amount {amount}");
                            notify = Some(amount);
                        }
                        SessionEvent::LootClearMoney => {
                            println!("SMSG_LOOT_CLEAR_MONEY");
                            cleared = true;
                        }
                        SessionEvent::ObjectValues { guid: g, fields } if g == self_guid => {
                            if let Some(sf) = &mut world.self_fields {
                                sf.merge(fields);
                                // Only a changed purse counts: the loot flow also pushes
                                // flags-only self updates (`UNIT_FLAG_LOOTING`).
                                if sf.player_money() != money_before {
                                    money_after = sf.player_money();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            if !cleared {
                bail!("--loot: no SMSG_LOOT_CLEAR_MONEY within 5s");
            }
            match (money_before, money_after) {
                (Some(before), Some(after)) if after > before => {
                    println!(
                        "✅ loot money: PLAYER_FIELD_COINAGE {before} → {after} copper (+{}), coin line cleared{}.",
                        after - before,
                        notify
                            .map(|n| format!(", SMSG_LOOT_MONEY_NOTIFY amount {n}"))
                            .unwrap_or_default()
                    );
                }
                _ => {
                    let amount = notify.context(
                        "--loot: no PLAYER_FIELD_COINAGE delta and no SMSG_LOOT_MONEY_NOTIFY \
                         within 15s — the money never reached us",
                    )?;
                    println!(
                        "✅ loot money: SMSG_LOOT_MONEY_NOTIFY amount {amount}, coin line cleared."
                    );
                }
            }
        } else {
            println!("(no gold on this corpse — skipping CMSG_LOOT_MONEY)");
        }

        println!("sending CMSG_LOOT_RELEASE");
        session.loot_release(target)?;
        let mut released = false;
        let drain_until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < drain_until && !released {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::LootReleaseResponse { guid: g, .. } = ev {
                    if g == target {
                        println!("SMSG_LOOT_RELEASE_RESPONSE: guid {g:#x}");
                        released = true;
                    }
                }
            }
        }
        if !released {
            bail!("--loot: no SMSG_LOOT_RELEASE_RESPONSE within 5s");
        }
        println!(
            "\n✅ loot: killed {target:#x}, loot_type {loot_type}, {} item(s) stored, gold {gold}, window released.",
            items.len()
        );
        Ok(())
    }
}
