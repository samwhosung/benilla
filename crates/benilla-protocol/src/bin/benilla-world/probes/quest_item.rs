//! `--quest-item`: an item with a non-zero `StartQuest` is its own questgiver. The client sends
//! `CMSG_QUESTGIVER_QUERY_QUEST` and the accept to the item's guid, never `CMSG_USE_ITEM`, which
//! the server refuses with `EQUIP_ERR_ITEM_NOT_FOUND`.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use benilla_protocol::{decode, SessionEvent, WorldSession};

use crate::probes::{Ctx, Probe, FIELD_PLAYER_QUEST_LOG_1_1};

/// A drain handler: `Some(msg)` stops the drain early, `None` keeps pumping.
type QuestEventHandler = Box<dyn FnMut(&SessionEvent) -> Option<String>>;

/// "Northshire Gift Voucher", starter of quest 5805 "Welcome!": level 1, no prerequisite and no
/// race or class gate, so any character can take it. Not equippable and carries no spell.
const ITEM_ENTRY: u32 = 14646;
const QUEST_ID: u32 = 5805;

pub(crate) struct QuestItem;

impl Probe for QuestItem {
    fn stage(&mut self, cx: &mut Ctx) -> Result<()> {
        // Drop the quest so every run accepts it fresh; `verify` removes the item again.
        cx.session.send_chat(&format!(".quest remove {QUEST_ID}"))?;
        cx.session.send_chat(&format!(".additem {ITEM_ENTRY}"))?;
        println!("sent GM: .quest remove {QUEST_ID}; .additem {ITEM_ENTRY}");
        Ok(())
    }

    fn verify(&mut self, cx: &mut Ctx) -> Result<()> {
        let world = &mut *cx.world;
        let session = &mut *cx.session;
        let self_guid = world.self_guid;

        let drain = |session: &mut WorldSession,
                     sf: &mut Option<benilla_protocol::messages::ObjectFields>,
                     secs: u64,
                     mut f: QuestEventHandler|
         -> Option<String> {
            let until = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < until {
                let Ok(msg) = session.recv() else { continue };
                for ev in decode(msg) {
                    if let SessionEvent::ObjectValues { guid: g, fields } = &ev {
                        if *g == self_guid {
                            if let Some(sf) = sf.as_mut() {
                                sf.merge(fields.clone());
                            }
                        }
                    }
                    if let Some(done) = f(&ev) {
                        return Some(done);
                    }
                }
            }
            None
        };

        // 1) Find the starter in the backpack: its guid is the questgiver on this wire.
        let sf = world
            .self_fields
            .as_ref()
            .context("no self descriptor — can't walk the backpack")?;
        let item_guid = (0..16)
            .filter_map(|i| sf.player_pack_slot(i).filter(|g| *g != 0))
            .find(|g| world.item_entries.get(g).copied() == Some(ITEM_ENTRY))
            .with_context(|| {
                format!(
                    "--quest-item: item {ITEM_ENTRY} isn't in the backpack — did `.additem` land? \
                     (needs a GM account; try a longer --seconds)"
                )
            })?;
        println!("\nquest-starter item {ITEM_ENTRY}: guid {item_guid:#x} (the giver on this wire)");

        // 2) QUERY_QUEST on the item guid answers with `SMSG_QUESTGIVER_QUEST_DETAILS`.
        println!("CMSG_QUESTGIVER_QUERY_QUEST({QUEST_ID}) on the item guid");
        session.questgiver_query_quest(item_guid, QUEST_ID)?;
        let details = drain(
            session,
            &mut world.self_fields,
            5,
            Box::new(|ev| match ev {
                SessionEvent::QuestDetail(d) if d.quest_id == QUEST_ID => Some(format!(
                    "SMSG_QUESTGIVER_QUEST_DETAILS: \"{}\" — giver {:#x}",
                    d.title, d.npc
                )),
                _ => None,
            }),
        )
        .context(
            "--quest-item: no SMSG_QUESTGIVER_QUEST_DETAILS within 5s — the item guid was not \
             accepted as a questgiver",
        )?;
        println!("✅ details: {details}");

        // 3) Accept on the same guid. vmangos `Player::AddQuest` keeps an item giver the quest
        // requires or names `SrcItemId`, and 5805 does both, so the starter must survive.
        println!("CMSG_QUESTGIVER_ACCEPT_QUEST({QUEST_ID}) on the item guid");
        session.questgiver_accept_quest(item_guid, QUEST_ID)?;
        let mut destroyed = false;
        let mut in_log = false;
        for _ in 0..6 {
            if drain(
                session,
                &mut world.self_fields,
                1,
                Box::new(move |ev| match ev {
                    SessionEvent::ObjectDestroyed(g) if *g == item_guid => Some(String::new()),
                    _ => None,
                }),
            )
            .is_some()
            {
                destroyed = true;
            }
            in_log = world.self_fields.as_ref().is_some_and(|sf| {
                (0..20).any(|i| {
                    sf.raw_fields().any(|(idx, val)| {
                        idx == FIELD_PLAYER_QUEST_LOG_1_1 + 3 * i && val == QUEST_ID
                    })
                })
            });
            if in_log {
                break;
            }
        }
        anyhow::ensure!(
            in_log,
            "--quest-item: quest {QUEST_ID} never landed in a PLAYER_QUEST_LOG field — the \
             item-sourced accept did not take"
        );
        anyhow::ensure!(
            !destroyed,
            "--quest-item: the starter item {item_guid:#x} was destroyed, but quest {QUEST_ID} \
             requires it (ReqItemId1) — vmangos' AddQuest keeps a required starter"
        );
        println!(
            "✅ accept: quest {QUEST_ID} is in PLAYER_QUEST_LOG; the starter item survived (it is \
             the quest's own required turn-in)"
        );

        // 4) Click again while on the quest: `HandleQuestgiverQueryQuestOpcode` has no status gate,
        // so DETAILS returns, and the accept is refused with `SMSG_QUESTGIVER_QUEST_INVALID` reason
        // 13 (`INVALIDREASON_QUEST_ALREADY_ON`, shown as `ERR_QUEST_ALREADY_ON`).
        println!("\nsecond click while ON the quest — QUERY then ACCEPT again");
        session.questgiver_query_quest(item_guid, QUEST_ID)?;
        let reopened = drain(
            session,
            &mut world.self_fields,
            5,
            Box::new(|ev| match ev {
                SessionEvent::QuestDetail(d) if d.quest_id == QUEST_ID => {
                    Some(format!("\"{}\"", d.title))
                }
                _ => None,
            }),
        )
        .context(
            "--quest-item: the re-query got no DETAILS — this server DOES gate the query by \
             quest status, so the panel would not re-open (the probe expects vmangos's \
             ungated handler)",
        )?;
        println!("✅ the panel legitimately re-opens: DETAILS {reopened}");

        session.questgiver_accept_quest(item_guid, QUEST_ID)?;
        let refusal = drain(
            session,
            &mut world.self_fields,
            5,
            Box::new(|ev| match ev {
                SessionEvent::QuestGiverInvalid { reason } => Some(reason.to_string()),
                _ => None,
            }),
        )
        .context(
            "--quest-item: the second accept drew no SMSG_QUESTGIVER_QUEST_INVALID within 5s",
        )?;
        anyhow::ensure!(
            refusal == "13",
            "--quest-item: expected QUEST_INVALID reason 13 (ALREADY_ON, 0x0d), \
             got {refusal}"
        );
        println!("✅ refusal: QUEST_INVALID reason 13 → ERR_QUEST_ALREADY_ON (the ref's 0x5dbca0)");

        // Leave the character as found; the starter survives the accept, so remove it too.
        session.send_chat(&format!(".quest remove {QUEST_ID}"))?;
        session.send_chat(&format!(".additem {ITEM_ENTRY} -1"))?;
        drain(session, &mut world.self_fields, 2, Box::new(|_| None));
        println!("cleanup: .quest remove {QUEST_ID}; .additem {ITEM_ENTRY} -1");
        Ok(())
    }
}
