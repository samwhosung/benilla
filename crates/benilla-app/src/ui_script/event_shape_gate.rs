//! The event argument-shape gate: every fire site with a literal name against
//! `reference/event_args.tsv`, since an event name is a plain string at both ends.
//!
//! Only `exact` rows gate: an `advisory` row's site declares more varargs than its caller pushes,
//! and a `none` row has no producer in the census, which does not mean it has none. Only a
//! `ScriptValue::` element's kind is asserted; any other argument counts toward the arity alone.
//! A site of a gated event whose list is not written out at the call (`vec![…]`, `&[…]`,
//! `Vec::new()`) is refused, since nothing then says what it pushes.
use std::collections::BTreeMap;

use super::fire_scan::{scan, Name, Scan};

/// One row of `reference/event_args.tsv`.
struct Row {
    /// The alternatives, each a per-slot kind list. `%s` -> `s`, `%d`/`%u`/`%f` -> `n`.
    shapes: Vec<Vec<char>>,
    producers: String,
}

fn table() -> BTreeMap<String, Row> {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/event_args.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/event_args.tsv");
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

/// Every `(event, argument kinds, where)` the scan finds fired under a name it reads: a literal,
/// or the literal arms of an `if`. The kinds are `None` when the list is built away from the
/// site. A relay carries neither name nor list, and a name held in a binding is read by no gate.
type Fired = (String, Option<Vec<char>>, String);

fn fired_in(scan: &Scan) -> Vec<Fired> {
    let mut out = Vec::new();
    for site in &scan.sites {
        let Name::Lit(names) = &site.name else {
            continue;
        };
        for name in names {
            out.push((name.clone(), site.kinds(), site.at()));
        }
    }
    out
}

fn fire_sites() -> Vec<Fired> {
    fired_in(scan())
}

/// What the table says of `sites`: how many it could measure, the shapes that do not fit, and the
/// gated events whose list is built away from the call.
fn judge(table: &BTreeMap<String, Row>, sites: &[Fired]) -> (usize, Vec<String>, Vec<String>) {
    let (mut checked, mut wrong, mut unreadable) = (0usize, Vec::new(), Vec::new());
    for (name, kinds, at) in sites {
        let Some(row) = table.get(name) else { continue };
        let Some(kinds) = kinds else {
            unreadable.push(format!("{name} from {at}"));
            continue;
        };
        checked += 1;
        let fits = row.shapes.iter().any(|want| {
            want.len() == kinds.len() && want.iter().zip(kinds).all(|(w, g)| *g == '?' || g == w)
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
    wrong.sort();
    wrong.dedup();
    unreadable.sort();
    unreadable.dedup();
    (checked, wrong, unreadable)
}

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

    let (checked, wrong, unreadable) = judge(&table, &sites);
    assert!(
        unreadable.is_empty(),
        "{} fire site(s) of a gated event build their argument list away from the call, so this \
         gate cannot see what they push. Write the list at the call (`vec![…]`, `Vec::new()`), \
         one call per shape:\n  {}",
        unreadable.len(),
        unreadable.join("\n  ")
    );
    assert!(
        checked >= 175,
        "only {checked} fires were measurable against the table — coverage collapsed"
    );
    assert!(
        wrong.is_empty(),
        "{} fire site(s) push the wrong argument shape. An event name is a plain string at both \
         ends, so nothing else in the build can see this:\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
}

/// The in-game event table (`0x703d90`, name array `0xbe1198`) holds the names of
/// `reference/events.tsv` and the glue table the 26 of `reference/glue_events.tsv`; `RegisterEvent`
/// drops any other name (`0x702140`), so an event fired under a name off both reaches no frame
/// that registered it by name.
#[test]
fn every_event_we_fire_is_in_the_reference_table() {
    let tsv = concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference/events.tsv");
    let text = std::fs::read_to_string(tsv).expect("reference/events.tsv");
    let names: std::collections::BTreeSet<&str> = text
        .lines()
        .skip(1)
        .filter_map(|l| l.split('\t').nth(1))
        .chain(benilla_ui::script::glue_events().iter().copied())
        .collect();
    assert!(names.len() > 390, "the catalog stopped being read");
    let mut off: Vec<String> = fire_sites()
        .into_iter()
        .filter(|(name, _, _)| !names.contains(name.as_str()))
        .map(|(name, _, at)| format!("{name} from {at}"))
        .collect();
    off.sort();
    off.dedup();
    assert!(
        off.is_empty(),
        "fired under a name the reference's event table lacks:\n  {}",
        off.join("\n  ")
    );
}

/// `0x51bbb0` registers one watch per named unit-window field, so each of the 25 named event ids
/// below `0xb6` comes from that bridge with the unit token, and each needs a producer here.
#[test]
fn every_unit_field_bridge_event_has_a_producer() {
    /// Bridge events nothing here produces, each with its reason; an entry that gains a producer
    /// fails the gate.
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
            // The ten power events are built with `format!("UNIT_{}", power_token(…))`, so there
            // is no literal to scan; `reference_ui`'s `CONSTRUCTED` list declares the same family.
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

#[cfg(test)]
mod bite {
    use super::*;
    use crate::ui_script::fire_scan::scan_of;

    const WRAPPER: &str = "impl UiScript { fn fire_event(&mut self, event: &str, \
                           args: Vec<ScriptValue>) {} }";

    fn judged(body: &str) -> (Vec<String>, Vec<String>) {
        let s = scan_of(&[("w.rs", WRAPPER), ("a.rs", body)]);
        let (_, wrong, unreadable) = judge(&table(), &fired_in(&s));
        (wrong, unreadable)
    }

    /// An argument on an event the reference signals bare is the wrong shape.
    #[test]
    fn an_argument_on_a_bare_event_is_refused() {
        let (wrong, unreadable) = judged(
            r#"fn f(s: &mut UiScript) {
                s.fire_event("GOSSIP_SHOW", vec![ScriptValue::Str(String::new())]);
            }"#,
        );
        assert_eq!(wrong.len(), 1, "{wrong:?}");
        assert!(unreadable.is_empty());
        let (wrong, unreadable) =
            judged(r#"fn f(s: &mut UiScript) { s.fire_event("GOSSIP_SHOW", vec![]); }"#);
        assert!(wrong.is_empty() && unreadable.is_empty());
    }

    /// A list built away from the call is refused, not skipped: through a call, a binding, or a
    /// closure that forwards it.
    #[test]
    fn a_list_built_away_from_the_call_is_refused() {
        for body in [
            r#"fn f(s: &mut UiScript) { s.fire_event("GOSSIP_SHOW", name_arg()); }"#,
            r#"fn f(s: &mut UiScript) { let args = make(); s.fire_event("GOSSIP_SHOW", args); }"#,
            r#"fn f(lua: &Lua) {
                let fire = |event: &str, args: Vec<ScriptValue>| s.fire_event(event, args);
                fire("GOSSIP_SHOW", name_arg());
            }"#,
        ] {
            let (wrong, unreadable) = judged(body);
            assert!(wrong.is_empty());
            assert_eq!(unreadable.len(), 1, "{body}: {unreadable:?}");
        }
    }

    /// An event the table does not gate (a glue event, an advisory row) is not refused for it.
    #[test]
    fn an_event_the_table_does_not_gate_is_not_refused() {
        let (wrong, unreadable) =
            judged(r#"fn f(s: &mut UiScript) { s.fire_event("NOT_AN_EVENT", name_arg()); }"#);
        assert!(wrong.is_empty() && unreadable.is_empty());
    }
}
