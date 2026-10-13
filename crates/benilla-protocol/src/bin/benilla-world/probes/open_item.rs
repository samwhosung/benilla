//! `--open-item`: the openable-item wire. A bag right-click on an item whose template carries
//! LOOTABLE (`ItemInfo::openable`) sends `CMSG_OPEN_ITEM(bagIndex, slot)`, never `CMSG_USE_ITEM`,
//! and the server answers `SMSG_LOOT_RESPONSE` on the item's own guid. Adds a clam, opens it,
//! releases the window and removes the clam.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use benilla_protocol::{decode, SessionEvent};

use crate::probes::{Ctx, Probe};

/// Small Barnacled Clam: `Flags` has LOOTABLE (0x4) and `LockID` is 0, so it opens as is.
const ITEM_ENTRY: u32 = 7973;

pub(crate) struct OpenItem;

impl Probe for OpenItem {
    fn stage(&mut self, cx: &mut Ctx) -> Result<()> {
        cx.session.send_chat(&format!(".additem {ITEM_ENTRY}"))?;
        println!("sent GM: .additem {ITEM_ENTRY}");
        Ok(())
    }

    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let world = &mut *cx.world;
        let session = &mut *cx.session;

        // 1) Find the clam: the wire addresses it by position (bag 255, player slot), not guid.
        let sf = world
            .self_fields
            .as_ref()
            .context("no self descriptor — can't walk the backpack")?;
        let (slot0, item_guid) = (0..16)
            .filter_map(|i| sf.player_pack_slot(i).filter(|g| *g != 0).map(|g| (i, g)))
            .find(|(_, g)| world.item_entries.get(g).copied() == Some(ITEM_ENTRY))
            .with_context(|| {
                format!(
                    "--open-item: item {ITEM_ENTRY} isn't in the backpack — did `.additem` land? \
                     (needs a GM account; try a longer --seconds)"
                )
            })?;
        let wire_slot = 23 + slot0;
        println!(
            "\nopenable item {ITEM_ENTRY}: guid {item_guid:#x} in pack slot {} → \
             CMSG_OPEN_ITEM bag 255 slot {wire_slot}",
            slot0 + 1
        );

        // 2) Open it: nothing else opens a loot window on a bag item's own guid.
        session.open_item(255, wire_slot)?;
        let drain_until = Instant::now() + Duration::from_secs(5);
        let mut verdict: Option<Result<String, String>> = None;
        while Instant::now() < drain_until && verdict.is_none() {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                match ev {
                    SessionEvent::LootResponse {
                        guid,
                        loot_type,
                        gold,
                        items,
                    } if guid == item_guid => {
                        verdict = Some(Ok(format!(
                            "SMSG_LOOT_RESPONSE on the ITEM guid — type {loot_type}, {} copper, \
                             {} row(s): {}",
                            gold,
                            items.len(),
                            items
                                .iter()
                                .map(|i| format!("{}x{}", i.count, i.item_id))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )));
                    }
                    SessionEvent::LootError { guid, error } if guid == item_guid => {
                        verdict = Some(Err(format!("server refused with loot error {error}")));
                    }
                    SessionEvent::InventoryFailure { reason, .. } => {
                        verdict = Some(Err(format!(
                            "server refused with EQUIP_ERR {reason} (locked? dead? flying?)"
                        )));
                    }
                    _ => {}
                }
            }
        }
        let v = verdict
            .context("--open-item: no reaction to CMSG_OPEN_ITEM within 5s")?
            .map_err(|e| anyhow::anyhow!("--open-item: {e}"))?;
        println!("✅ open item: {v}");

        // 3) Close the window, or the loot session stays with the character.
        session.loot_release(item_guid)?;
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            let Ok(msg) = session.recv() else { continue };
            for ev in decode(msg) {
                if let SessionEvent::LootReleaseResponse { guid, .. } = ev {
                    if guid == item_guid {
                        println!("✅ release: SMSG_LOOT_RELEASE_RESPONSE on {guid:#x}");
                    }
                }
            }
        }

        // Remove the clam, which survives an open that is not emptied.
        session.send_chat(&format!(".additem {ITEM_ENTRY} -1"))?;
        println!("cleanup: .additem {ITEM_ENTRY} -1");
        Ok(())
    }
}
