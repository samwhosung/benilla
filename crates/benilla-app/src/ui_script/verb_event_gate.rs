//! **The VERB-FIRED event gate** — `reference/1.12-verb-events.tsv` against the module that
//! registers each verb (decision 2251).
//!
//! ## The third question on the event seam
//!
//! Two gates compare event NAMES (`reference_ui`: a stock listener nothing feeds, 1889; a fire the
//! reference has no name for, 1883) and one compares ARGUMENTS (`event_shape_gate`, 2140). None of
//! them asks **who** fires it, and that is the hole B389/2244 fell through. The reference's
//! `SetTrainerServiceTypeFilter` commits through `0x4d8c90`, whose whole body is *write the mask,
//! re-run the finalizer, fire `TRAINER_UPDATE`*; so the stock `Blizzard_TrainerUI.lua` never
//! repaints the list itself — it calls the verb for its side effect. benilla's verb set the mask
//! and fired nothing. The name gate stayed green because the packet arm fires the same event, and
//! the tests stayed green because they asserted the mask. Our own retired `TrainerFrame.xml` had
//! repainted explicitly, so the behaviour left with the file that was compensating for it (1957),
//! and the director found it twelve days later. Every window that went stock (1751) carries the
//! same exposure: a verb the stock Lua calls *for the event*.
//!
//! ## The rule
//!
//! For every pair the reference fires on the verb's own call path (the table: the verb's body, or
//! a helper that only registered verbs call), and every verb benilla registers: **a file that
//! registers the verb also fires the event** — or the pair is declared below, and the declaration
//! is *checked*, never trusted:
//!
//! * [`ELSEWHERE`] — benilla fires it from a named feed instead, on the state the verb changed
//!   (the verb bumps a generation, sets a touched flag, queues an intent the app drains). That
//!   file must fire it, and the verb's own file must not, or the row is stale.
//! * [`GAP`] — nothing fires it on this verb's path, for a stated reason. The verb's own file
//!   must still not fire it, or the row is stale.
//!
//! A verb no file registers is skipped: unbuilt verbs are `scripts/api-coverage.sh`'s queue
//! (1178), not this gate's.
//!
//! ## What "registers" and "fires" mean here, and the limit
//!
//! Text, not execution: the quoted literal `"Verb"` or `"EVENT"` in a non-test source file, with
//! comment lines dropped and the trailing test module cut. Lenient in exactly one direction — any
//! file that names the verb and fires the event passes — because the honest alternative, proving
//! the path *executes*, is a per-verb fixture, and 2244's own test is the measure of a fixture that
//! asserts the wrong thing. What this cannot see: a fire that is in the file but off the verb's
//! path — an `ELSEWHERE` row is a claim the session that wrote it read at the feed. What it sees
//! every time is the migration class itself: a verb whose module fires nothing.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// One row of `reference/1.12-verb-events.tsv`.
struct Pair {
    verb: String,
    event: String,
    shape: String,
    via: String,
}

/// Pairs benilla fires from a feed, on the state the verb changes. `(verb, event, the file that
/// fires it)`; the file is matched as a path suffix. Each was read at the feed when the row was
/// written — the trigger is named so the next reader can check it in one grep.
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
    // The toggle goes to the wire and the bar's pushed key changes on the reply — one round trip
    // later than the reference, which fires locally from the toggle.
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

/// Pairs nothing fires on the verb's path, each with its reason. A row is a gap with a name,
/// never a reason to stop; the reference's own site is in the table.
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
            // The same file-name rule the sibling gates use: a test's own literals are not a
            // surface we ship — and neither are the gates' own declared lists, this file's
            // included (a `*_gate.rs` is a `#[cfg(test)]` module whose name does not say so).
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

/// **Every event the reference fires from inside a verb is fired by the module that registers
/// the verb** — or declared, with the declaration checked.
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
            continue; // an unbuilt verb: api-coverage.sh's queue, not this gate's
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

/// **A declaration is checked, not trusted** — an `ELSEWHERE` file that stopped firing, a verb
/// module that now fires what its row says it does not, a row for a pair the table no longer
/// carries: each fails until the row is corrected.
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
