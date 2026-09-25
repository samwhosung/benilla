//! The verb-fired event gate: an event the reference fires on a verb's own call path
//! (`reference/1.12-verb-events.tsv`) must be fired by a file that registers the verb, since stock
//! Lua calls such a verb for its event (`0x4d8c90`, the trainer filter's commit, fires
//! `TRAINER_UPDATE`). Otherwise the pair is declared in [`ELSEWHERE`] or [`GAP`], and the verb's
//! own file must not fire it. The check is textual, so it cannot see a fire off the verb's path.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// One row of `reference/1.12-verb-events.tsv`.
struct Pair {
    verb: String,
    event: String,
    shape: String,
    via: String,
}

/// Pairs a feed fires on the state the verb changes, as `(verb, event, file)` with the file
/// matched as a path suffix; each row's comment names the trigger.
const ELSEWHERE: &[(&str, &str, &str)] = &[
    // The sell slot's diff (`auction_sell_item` moved, or `sell_slot_dirty`).
    (
        "ClickAuctionSellItemButton",
        "NEW_AUCTION_UPDATE",
        "benilla-app/src/ui_auction/mod.rs",
    ),
    // The query goes to the wire; the inbox feed announces the list when it lands.
    (
        "CheckInbox",
        "MAIL_INBOX_UPDATE",
        "benilla-app/src/ui_mail/mod.rs",
    ),
    // `craft_close` / `trade_skill_close` are drained by the app, whose store transition
    // `(Some, None)` is the fire.
    ("CloseCraft", "CRAFT_CLOSE", "benilla-app/src/ui_craft.rs"),
    (
        "CloseTradeSkill",
        "TRADE_SKILL_CLOSE",
        "benilla-app/src/ui_tradeskill.rs",
    ),
    // `take_quest_log_collapses`: the app owns the collapse set, re-feeds the list, fires.
    (
        "CollapseQuestHeader",
        "QUEST_LOG_UPDATE",
        "benilla-app/src/ui_quest_log.rs",
    ),
    (
        "ExpandQuestHeader",
        "QUEST_LOG_UPDATE",
        "benilla-app/src/ui_quest_log.rs",
    ),
    // `trade_skill_touched`, set by all four verbs, drained by `take_trade_skill_touched`.
    (
        "CollapseTradeSkillSubClass",
        "TRADE_SKILL_UPDATE",
        "benilla-app/src/ui_tradeskill.rs",
    ),
    (
        "ExpandTradeSkillSubClass",
        "TRADE_SKILL_UPDATE",
        "benilla-app/src/ui_tradeskill.rs",
    ),
    (
        "SetTradeSkillInvSlotFilter",
        "TRADE_SKILL_UPDATE",
        "benilla-app/src/ui_tradeskill.rs",
    ),
    (
        "SetTradeSkillSubClassFilter",
        "TRADE_SKILL_UPDATE",
        "benilla-app/src/ui_tradeskill.rs",
    ),
    // `repeat_count` moved (`repeat_changed`).
    (
        "DoTradeSkill",
        "UPDATE_TRADESKILL_RECAST",
        "benilla-app/src/ui_tradeskill.rs",
    ),
    // `take_loot_roll_confirms`: a Need/Greed on a bind-on-pickup roll is diverted to the popup.
    (
        "ConfirmLootRoll",
        "CANCEL_LOOT_ROLL",
        "benilla-app/src/ui_loot_roll.rs",
    ),
    (
        "ConfirmLootRoll",
        "CONFIRM_LOOT_ROLL",
        "benilla-app/src/ui_loot_roll.rs",
    ),
    (
        "RollOnLoot",
        "CANCEL_LOOT_ROLL",
        "benilla-app/src/ui_loot_roll.rs",
    ),
    (
        "RollOnLoot",
        "CONFIRM_LOOT_ROLL",
        "benilla-app/src/ui_loot_roll.rs",
    ),
    // `keybinds.generation` moves; `sync_dispatch` re-derives the table and fires.
    (
        "SetBinding",
        "UPDATE_BINDINGS",
        "benilla-app/src/bindings.rs",
    ),
    // Fired when the server's reply changes the bar, a round trip after the reference, which
    // fires locally from the toggle.
    (
        "TogglePetAutocast",
        "PET_BAR_UPDATE",
        "benilla-app/src/ui_pet/bar.rs",
    ),
    (
        "ToggleSpellAutocast",
        "PET_BAR_UPDATE",
        "benilla-app/src/ui_pet/bar.rs",
    ),
];

/// Pairs nothing fires on the verb's path, each with its reason.
const GAP: &[(&str, &str, &str)] = &[
    (
        "CancelSkillUps",
        "SKILL_LINES_CHANGED",
        "the reference's verb (0x4d3e30) calls the temp-point reset and fires unconditionally at \
         0x4d3e35; here the reset runs over a table this model keeps empty (`skills.rs`), so \
         nothing moves and `ui_char.rs`'s feed has nothing to announce — the stock SkillFrame \
         would only repaint under its own Close",
    ),
    (
        "ClickTargetTradeButton",
        "TRADE_REPLACE_ENCHANT",
        "the reference runs the enchant clash check 0x496170 when a spell is on the cursor and \
         fires this for the replace dialog; casting \
         an enchant onto the partner's slot is not built here — `ui_trade.rs` only mirrors the \
         wire's enchant slot — so the verb is the money arm alone",
    ),
    (
        "CloseTrade",
        "PLAYER_TRADE_MONEY",
        "fired only when the trade did not complete (`[0xb71748] == 0`), \
         zeroing the cancelled offer under a frame TRADE_CLOSED \
         hides; here `TradeSession::begin` resets the whole session at the next trade, so the \
         next window opens at zero either way",
    ),
    (
        "CollapseCraftSkillLine",
        "CRAFT_UPDATE",
        "the reference commits through the 21-byte thunk 0x4f6be0 (the trainer's 0x4d8c90 shape); \
         a no-op here because the craft list is flat — the header law was never ported from \
         TradeSkill (0446, 0530's follow-up) — so there is nothing to repaint. Whether any 1.12 \
         craft list carries more than one group is not established: 0446 covers Enchanting, \
         Beast Training is unchecked",
    ),
    (
        "ExpandCraftSkillLine",
        "CRAFT_UPDATE",
        "the same thunk and the same no-op as CollapseCraftSkillLine",
    ),
    (
        "PetDismiss",
        "PET_DISMISS_START",
        "the worker 0x4bd6e0 sends the dismiss and fires this with a duration (`%d`, 10000); \
         no shipped file listens, the dismiss reaches the \
         wire through the app's drain, and the pet frame follows the unit's departure",
    ),
    (
        "SelectGossipOption",
        "GOSSIP_ENTER_CODE",
        "a coded option raises the code-entry popup in the reference (the worker 0x4e2320); here \
         coded options are greyed and unselectable (0081 v1, `reference_ui`'s UNPRODUCED row), and \
         vmangos's `gossip_menu_option` carries zero coded rows, so no NPC on this server reaches it",
    ),
];

fn table() -> Vec<Pair> {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-verb-events.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/1.12-verb-events.tsv");
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("verb\t"))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 4).then(|| Pair {
                verb: f[0].to_string(),
                event: f[1].to_string(),
                shape: f[2].to_string(),
                via: f[3].to_string(),
            })
        })
        .collect()
}

/// Every non-test `.rs` under `crates/`: comment lines dropped, the trailing test module cut.
fn sources() -> Vec<(PathBuf, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("crates");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            // Skip tests and `*_gate.rs`, test-only modules whose names do not say so.
            let name = p.to_string_lossy();
            if p.extension().is_none_or(|x| x != "rs")
                || name.contains("test")
                || name.ends_with("_gate.rs")
            {
                continue;
            }
            let whole = std::fs::read_to_string(&p).unwrap_or_default();
            let head = whole
                .rfind("#[cfg(test)]\nmod ")
                .map_or(whole.as_str(), |i| &whole[..i]);
            let text: String = head
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            out.push((p, text));
        }
    }
    out.sort();
    out
}

fn quoted(text: &str, name: &str) -> bool {
    text.contains(&format!("\"{name}\""))
}

fn short(p: &std::path::Path) -> String {
    let s = p.to_string_lossy();
    s.split_once("/crates/")
        .map_or(s.to_string(), |(_, t)| t.to_string())
}

/// The files that register `verb`, and whether any of them fires `event`.
fn verb_module_fires(src: &[(PathBuf, String)], verb: &str, event: &str) -> (Vec<String>, bool) {
    let registrars: Vec<&(PathBuf, String)> = src.iter().filter(|(_, t)| quoted(t, verb)).collect();
    let fires = registrars.iter().any(|(_, t)| quoted(t, event));
    (registrars.iter().map(|(p, _)| short(p)).collect(), fires)
}

#[test]
fn every_event_the_reference_fires_from_a_verb_is_fired_by_the_verb_s_module() {
    let src = sources();
    assert!(src.len() > 100, "source walk found {} files", src.len());
    let declared: BTreeSet<(&str, &str)> = ELSEWHERE
        .iter()
        .chain(GAP.iter())
        .map(|(v, e, _)| (*v, *e))
        .collect();

    let mut surprises = Vec::new();
    let mut seen = 0;
    for pair in table() {
        let (registrars, fires) = verb_module_fires(&src, &pair.verb, &pair.event);
        if registrars.is_empty() {
            continue; // an unbuilt verb: `scripts/api-coverage.sh`'s queue
        }
        seen += 1;
        if fires || declared.contains(&(pair.verb.as_str(), pair.event.as_str())) {
            continue;
        }
        let fired_by: Vec<String> = src
            .iter()
            .filter(|(_, t)| quoted(t, &pair.event))
            .map(|(p, _)| short(p))
            .collect();
        surprises.push(format!(
            "{} -> {}  [{}{}]  registered in {}; {}",
            pair.verb,
            pair.event,
            pair.shape,
            if pair.via == "-" {
                String::new()
            } else {
                format!(" via {}", pair.via)
            },
            registrars.join(", "),
            if fired_by.is_empty() {
                "fired NOWHERE in benilla".to_string()
            } else {
                format!("fired only by {}", fired_by.join(", "))
            },
        ));
    }
    assert!(
        seen > 20,
        "only {seen} pairs had a registered verb — the walk or the table is off"
    );
    assert!(
        surprises.is_empty(),
        "the reference fires these events from inside the verb, and the module that registers \
         the verb here fires nothing — the stock Lua that calls the verb for its side effect \
         repaints nothing (2244's class). Fire it from the verb, or declare the pair: ELSEWHERE \
         when a feed fires it on THIS verb's change and you read that at the feed, GAP with the \
         reason otherwise:\n  {}",
        surprises.join("\n  ")
    );
}

#[test]
fn the_declared_rows_are_still_true() {
    let src = sources();
    let table = table();
    let in_table =
        |verb: &str, event: &str| table.iter().any(|p| p.verb == verb && p.event == event);
    let mut wrong = Vec::new();
    for (verb, event, file) in ELSEWHERE {
        if !in_table(verb, event) {
            wrong.push(format!(
                "ELSEWHERE {verb} -> {event}: not a pair in the table any more"
            ));
        }
        let named: Vec<&(PathBuf, String)> = src
            .iter()
            .filter(|(p, _)| p.to_string_lossy().ends_with(file))
            .collect();
        if named.is_empty() {
            wrong.push(format!(
                "ELSEWHERE {verb} -> {event}: no source file ends with {file}"
            ));
        } else if !named.iter().any(|(_, t)| quoted(t, event)) {
            wrong.push(format!(
                "ELSEWHERE {verb} -> {event}: {file} no longer fires it"
            ));
        }
        if verb_module_fires(&src, verb, event).1 {
            wrong.push(format!(
                "ELSEWHERE {verb} -> {event}: the verb's own module fires it now — take the row out"
            ));
        }
    }
    for (verb, event, _) in GAP {
        if !in_table(verb, event) {
            wrong.push(format!(
                "GAP {verb} -> {event}: not a pair in the table any more"
            ));
        }
        if verb_module_fires(&src, verb, event).1 {
            wrong.push(format!(
                "GAP {verb} -> {event}: the verb's own module fires it now — take the row out"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "stale declarations:\n  {}",
        wrong.join("\n  ")
    );
}
