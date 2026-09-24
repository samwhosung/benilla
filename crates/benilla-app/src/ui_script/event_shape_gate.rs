//! **The event ARGUMENT-shape gate** — `reference/1.12-events.tsv` against what benilla actually
//! pushes at each fire site.
//!
//! Two gates already guard the event seam by NAME. [`super::reference_ui`]'s
//! `every_event_a_chain_file_registers_has_a_producer` asks whether something fires what a stock
//! file listens for (1889); `every_event_we_fire_is_an_event_the_reference_has` asks whether a
//! name we fire is a 1.12 name at all (1883). **Neither has ever looked at the arguments**, and an
//! event name is a plain string at both ends — so a fire with the wrong argument shape is silent
//! on both sides. It is 1842's finding one API over: the right name, the wrong shape.
//!
//! It was not hypothetical. Three live divergences on the first run:
//!
//! * `PLAYERBANKSLOTS_CHANGED` — two producers in the reference with **different** shapes
//!   (`0x5ddd6e` argless, `0x4c728d` `"player"`), one shape here.
//! * `UNIT_PET_EXPERIENCE` and `UNIT_PET_TRAINING_POINTS` — argless here, `%s` there. The second
//!   one reaches `PetPaperDollFrame_OnEvent` only through its final `elseif ( arg1 == "pet" )`
//!   catch-all, so an argless fire reached **nobody**: the stock pet page never repainted its
//!   training points off the event at all.
//!
//! ## What is compared, and what the table can and cannot say
//!
//! The oracle is two byte-level censuses of the reference, vendored by
//! `scripts/gen-reference-events.py`.
//! Its `conf` column carries the gate rule per row, exactly as `1.12-shapes.tsv` does:
//!
//! * `exact` — every producer that reaches the event has a known shape. **This is the column this
//!   gate reads.**
//! * `advisory` — a contributing site declares more varargs than its caller pushes (three sites,
//!   image-wide). Not gated: a faithful client must not copy a bug that hands Lua undefined
//!   values.
//! * `none` — no producer family reaches it in that census. **Not a claim that it has none** (the
//!   census's own header says so in capitals), so a `none` row is skipped, never failed. That is
//!   where the whole `CHAT_MSG_*` family sits: those reach Lua through a formatted wrapper the
//!   two-helper census does not see.
//!
//! ## Two deliberate narrowings
//!
//! **Only a fire whose event name is a literal is measured.** `ui_unit.rs` builds the ten power
//! events with `format!("UNIT_{}", power_token(…))`, and there is no name to key on at the call
//! site. Those are covered one gate over — `reference_ui`'s `CONSTRUCTED` list — and their single
//! `tok()` argument is the same shape all ten share.
//!
//! **Only a literal `ScriptValue` element's KIND is asserted.** An argument built from a variable
//! (`vec![ScriptValue::Int(bag)]` is literal enough — the *variant* is what types it, not the
//! value) is typed by its variant; anything that is not a `ScriptValue::` constructor at all
//! counts toward the arity and matches any kind. Arity is the assertion that catches the bug
//! class; kinds are the free extra.
use std::collections::BTreeMap;

/// One row of `reference/1.12-events.tsv`.
struct Row {
    /// The alternatives, each a per-slot kind list. `%s` -> `s`, `%d`/`%u`/`%f` -> `n`.
    shapes: Vec<Vec<char>>,
    producers: String,
}

fn table() -> BTreeMap<String, Row> {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-events.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/1.12-events.tsv");
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') || line.starts_with("name\t") {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        // name ids producers arg_formats conf note
        if f.len() < 5 || f[4] != "exact" {
            continue;
        }
        let shapes = f[3]
            .split('|')
            .map(|alt| {
                if alt == "()" {
                    return Vec::new();
                }
                alt.split('%')
                    .skip(1)
                    .map(|d| match d.chars().next() {
                        Some('s') => 's',
                        _ => 'n',
                    })
                    .collect()
            })
            .collect();
        out.insert(
            f[0].to_string(),
            Row {
                shapes,
                producers: f[2].to_string(),
            },
        );
    }
    out
}

/// Split `body` at its top-level commas — `vec![…]`'s elements, with nested calls left alone.
fn elements(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut cur, mut in_str, mut esc) = (0i32, String::new(), false, false);
    for c in body.chars() {
        if in_str {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                cur.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out.into_iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect()
}

/// The kind an argument expression pushes: `s` string, `n` number, `b` boolean, `x` nil,
/// `?` not determinable from the source.
fn kind_of(expr: &str) -> char {
    let e = expr.trim_start_matches('&').trim();
    for (variant, k) in [
        ("ScriptValue::Str", 's'),
        ("ScriptValue::Int", 'n'),
        ("ScriptValue::Number", 'n'),
        ("ScriptValue::Bool", 'b'),
        ("ScriptValue::Nil", 'x'),
    ] {
        if e.starts_with(variant) {
            return k;
        }
    }
    '?'
}

/// Every `(event, argument kinds, where)` benilla fires with a literal name.
fn fire_sites() -> Vec<(String, Vec<char>, String)> {
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
            if p.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            // A test's own synthetic fires are not a surface we ship — the same rule
            // `every_event_we_fire_is_an_event_the_reference_has` holds, plus the inline
            // `#[cfg(test)]` tail a file-name rule cannot see.
            if p.to_string_lossy().contains("test") {
                continue;
            }
            let whole = std::fs::read_to_string(&p).unwrap_or_default();
            let text = whole
                .split_once("#[cfg(test)]")
                .map_or(whole.as_str(), |(head, _)| head);
            // Split so this file cannot match its own scanner (the 1883 gate's lesson).
            for call in [
                concat!("fire_event", "("),
                concat!("fire_event_into", "("),
                concat!("pending_events.push", "(("),
            ] {
                let mut from = 0;
                while let Some(i) = text[from..].find(call) {
                    let at = from + i + call.len();
                    from = at;
                    let rest = text[at..].trim_start();
                    let rest = rest.strip_prefix("lua,").map_or(rest, str::trim_start);
                    let Some(body) = rest.strip_prefix('"') else {
                        continue;
                    };
                    let Some(end) = body.find('"') else { continue };
                    let (name, after) = (&body[..end], &body[end + 1..]);
                    let Some(args) = after.trim_start().strip_prefix(',') else {
                        continue;
                    };
                    let args = args.trim_start();
                    let kinds = if let Some(v) = args.strip_prefix("vec![") {
                        let mut depth = 1i32;
                        let mut close = v.len();
                        for (k, c) in v.char_indices() {
                            match c {
                                '[' => depth += 1,
                                ']' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        close = k;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        elements(&v[..close]).iter().map(|e| kind_of(e)).collect()
                    } else if args.starts_with("Vec::new()") {
                        Vec::new()
                    } else {
                        // An argument list built elsewhere — arity unknowable from here.
                        continue;
                    };
                    out.push((name.to_string(), kinds, p.display().to_string()));
                }
            }
        }
    }
    out
}

/// **Every event benilla fires must carry the arguments the reference's producer pushes.**
#[test]
fn every_event_we_fire_carries_the_reference_s_arguments() {
    let table = table();
    assert!(
        table.len() > 250,
        "the events table gave only {} gateable rows — it stopped being read",
        table.len()
    );
    let sites = fire_sites();
    assert!(
        sites.len() > 200,
        "the walker found only {} literal fire sites — it stopped matching, which would make this \
         gate silently vacuous",
        sites.len()
    );

    let mut checked = 0usize;
    let mut wrong: Vec<String> = Vec::new();
    for (name, kinds, at) in &sites {
        let Some(row) = table.get(name) else { continue };
        checked += 1;
        let fits = row.shapes.iter().any(|want| {
            want.len() == kinds.len()
                && want
                    .iter()
                    .zip(kinds)
                    .all(|(w, g)| *g == '?' || *g == *w || (*w == 'n' && *g == 'n'))
        });
        if !fits {
            let shapes: Vec<String> = row
                .shapes
                .iter()
                .map(|s| format!("({})", s.iter().collect::<String>()))
                .collect();
            wrong.push(format!(
                "{name} fired as ({}) from {at} — the reference's {} producer(s) push {}",
                kinds.iter().collect::<String>(),
                row.producers,
                shapes.join(" | ")
            ));
        }
    }
    assert!(
        checked >= 175,
        "only {checked} fires were measurable against the table — coverage collapsed"
    );
    wrong.sort();
    wrong.dedup();
    assert!(
        wrong.is_empty(),
        "{} fire site(s) push the wrong argument shape. An event name is a plain string at both \
         ends, so nothing else in the build can see this:\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
}

/// **Every event the reference's unit-field bridge produces must have a producer here.**
///
/// This is the half that let `UNIT_DYNAMIC_FLAGS` sit unfired for the whole life of the client.
/// [`super::reference_ui::every_event_a_chain_file_registers_has_a_producer`] asks whether
/// anything fires what a **stock chain file** listens for — and *nothing in 1.12 FrameXML
/// registers `UNIT_DYNAMIC_FLAGS`, so the hole was invisible to it by construction. The corpus is
/// where the demand was (`CT_UnitFrames/CT_TargetFrame.xml:200`, `TipBuddy/TipBuddy.lua:17`), and
/// the corpus is not in this repo.
///
/// So this gate asks the question from the REFERENCE's side instead, over the one family where
/// that is bounded and exact: `0x51bbb0` registers one watch per named unit-window field, so
/// every event id below `0xb6` that has a name in the client's own table is produced by that
/// bridge, always with the unit token. Twenty-five names, one producer function on our side
/// ([`crate::ui_unit::fire_transitions`] and the two pet-stat feeds), and no judgment call about
/// which of them "should" exist — the reference fires all of them.
///
/// A CENSUS with a declared set, like 1889's: what must not happen is a new name appearing.
#[test]
fn every_unit_field_bridge_event_has_a_producer() {
    /// Produced by the reference's bridge, by nothing here. Each needs a reason, and an entry
    /// that comes into production fails the gate so the list cannot outlive its fix.
    const UNPRODUCED: &[(&str, &str)] = &[(
        "UNIT_LOYALTY",
        "field 138, the hunter pet's loyalty level. benilla's pet-stat feed (`ui_pet_stats`) \
         reads happiness and training points off `PET_BYTES` but never the loyalty field, and no \
         window it feeds shows a loyalty line — `PetPaperDollFrame` does not register it either. \
         Unbuilt, not wrong: the day the pet page grows the line, this entry goes with it",
    )];

    let table = table();
    let bridge: Vec<&String> = table
        .iter()
        .filter(|(_, r)| r.producers.contains("unit-field-bridge"))
        .map(|(n, _)| n)
        .collect();
    assert!(
        bridge.len() >= 25,
        "only {} bridge events in the table — it stopped being read",
        bridge.len()
    );
    let fired: std::collections::HashSet<String> = fire_sites()
        .into_iter()
        .map(|(n, _, _)| n)
        .chain(
            // The ten power events are built with `format!("UNIT_{}", power_token(…))`, so no
            // literal exists to scan for — the same declared family `reference_ui`'s CONSTRUCTED
            // list carries, and an entry there is a promise that something fires it.
            [
                "MANA",
                "RAGE",
                "FOCUS",
                "ENERGY",
                "HAPPINESS",
                "MAXMANA",
                "MAXRAGE",
                "MAXFOCUS",
                "MAXENERGY",
                "MAXHAPPINESS",
            ]
            .iter()
            .map(|p| format!("UNIT_{p}")),
        )
        .collect();

    let expected: std::collections::HashSet<&str> = UNPRODUCED.iter().map(|(e, _)| *e).collect();
    let missing: Vec<&str> = bridge
        .iter()
        .map(|n| n.as_str())
        .filter(|n| !fired.contains(*n) && !expected.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "the reference fires these off a named unit descriptor field and benilla fires them \
         nowhere. No stock file need listen for one — the corpus does, and an addon that \
         registers an event nothing produces hears silence with no error anywhere:\n  {missing:?}"
    );
    let now_fired: Vec<&str> = UNPRODUCED
        .iter()
        .map(|(e, _)| *e)
        .filter(|e| fired.contains(*e))
        .collect();
    assert!(
        now_fired.is_empty(),
        "these now HAVE a producer — take them out of UNPRODUCED so the list keeps meaning what \
         it says: {now_fired:?}"
    );
}
