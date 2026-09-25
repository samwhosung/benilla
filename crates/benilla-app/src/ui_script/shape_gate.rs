//! The return-shape gate: `reference/1.12-shapes.tsv`, a harvest of every binding the 1.12 client
//! registers, against what this client answers.
//!
//! A 1.12 predicate answers `1` or `nil`, never a boolean (`IsPetAttackActive` aside), so kinds
//! are gated with arity. The table says per row what to trust: `arity_conf = exact`, kinds only
//! where `kinds_conf = agree`, never `argc_conf`. Only query verbs are probed, a call that raises
//! is skipped, and a name registered from several tables (`GetBuildInfo` is 5 in the glue table
//! `0x8373b8`, 3 in-game at `0x83de68`) is read at its non-glue row.

/// Globals known to answer the wrong arity: a name, the reference's arity and the reason. The
/// list may only shrink.
const NOT_YET_ASSERTED: &[(&str, usize, &str)] = &[];

/// The kind gate's list, same rules: a name, the wrong kinds it answers and the reason. An entry
/// whose binding comes into agreement fails the gate.
const KINDS_NOT_YET_ASSERTED: &[(&str, &str, &str)] = &[];

/// The widget kind gate's list, same rules as [`KINDS_NOT_YET_ASSERTED`].
const WIDGET_KINDS_NOT_YET_ASSERTED: &[(&str, &str, &str)] = &[];

/// The widget arity gate's list, same rules as [`NOT_YET_ASSERTED`].
const WIDGET_NOT_YET_ASSERTED: &[&str] = &[];

/// The glue registrar table. This VM is the in-game one, so a duplicated name is read at its
/// other row.
const GLUE_TABLE: &str = "0x8373b8";

struct Row {
    name: String,
    table_va: String,
    arity: usize,
    /// The `kinds` column verbatim, `|`-separated alternatives; empty unless trustworthy.
    kinds: String,
}

fn rows() -> Vec<Row> {
    rows_of_kind("global")
}

/// The `arity_conf = exact` rows of one `table_kind`: `global` is the registrar surface,
/// `baselib` the 36-entry Lua base array looped into `_G` at `0x811e28`.
fn rows_of_kind(kind: &str) -> Vec<Row> {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-shapes.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/1.12-shapes.tsv");
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("name\t"))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            // name fn pair_va table_va table_kind argc argc_conf arity arity_conf kinds …
            if f.len() < 9 || f[4] != kind || f[8] != "exact" {
                return None;
            }
            // A `delegated-push` row's callee pushes on its behalf, unseen by the push trace, so
            // its kinds cover the binding's own ops only; its arity, read from `eax`, holds.
            let delegated = f.len() > 11 && f[11].contains("delegated-push");
            Some(Row {
                name: f[0].to_string(),
                table_va: f[3].to_string(),
                arity: f[7].parse().ok()?,
                // Kinds are advisory unless `kinds_conf` is `agree`, so such a row carries none.
                kinds: if f.len() > 10 && f[10] == "agree" && !delegated {
                    f[9].to_string()
                } else {
                    String::new()
                },
            })
        })
        .collect()
}

/// Every registered global whose arity the reference states exactly, answered by this client.
#[test]
fn every_query_binding_answers_the_reference_s_return_arity() {
    let all = rows();
    assert!(
        all.len() > 800,
        "the vendored table looks wrong: {}",
        all.len()
    );

    let mut by_name: std::collections::HashMap<&str, Vec<&Row>> = std::collections::HashMap::new();
    for r in &all {
        by_name.entry(&r.name).or_default().push(r);
    }

    let mut s = benilla_ui::script::UiScript::new().expect("VM");
    // A seated player: the `Unit*` bindings raise on a nil token and answer a real one.
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Shapeprobe".into()),
            level: 60,
            ..Default::default()
        }),
    );
    let mut checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for (name, rs) in &by_name {
        // Query verbs only: the gate calls what it measures.
        if !["Get", "Is", "Has", "Can", "Unit", "Num"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let candidates: Vec<&&Row> = if rs.len() == 1 {
            rs.iter().collect()
        } else {
            rs.iter().filter(|r| r.table_va != GLUE_TABLE).collect()
        };
        // Still ambiguous after dropping glue: skip rather than guess.
        let Some(first) = candidates.first() else {
            continue;
        };
        if candidates.iter().any(|r| r.arity != first.arity) {
            continue;
        }
        let want = first.arity;

        // Counted with 5.0's `arg.n` inside the `pcall`: `select` is not a 1.12 global
        // (`lua50::install` removes it).
        let call = if name.starts_with("Unit") {
            format!(r#"{name}("player")"#)
        } else {
            format!("{name}()")
        };
        let probe = format!(
            "if type({name}) ~= 'function' then return -1 end \
             local ok, n = pcall(function() return (function(...) return arg.n end)({call}) end) \
             if not ok then return -1 end return n"
        );
        let got: i64 = match s.eval(&probe) {
            Ok(n) => n,
            Err(_) => continue,
        };
        if got < 0 {
            continue;
        }
        checked += 1;
        if got as usize != want {
            if let Some((_, known, _)) = NOT_YET_ASSERTED.iter().find(|(n, ..)| n == name) {
                assert_eq!(
                    *known, want,
                    "{name} is on the not-yet-asserted list at a stale arity — the table now says \
                     {want}"
                );
                continue;
            }
            mismatches.push(format!("{name}: answers {got}, reference states {want}"));
        }
    }

    // A floor, not a target: raise it when coverage rises, never lower it to fit a change.
    assert!(
        checked >= 240,
        "the gate measured only {checked} bindings — it has stopped covering anything"
    );
    // The list may only shrink: an entry that now answers the reference's arity must come off.
    let stale: Vec<&str> = NOT_YET_ASSERTED
        .iter()
        .map(|(n, ..)| *n)
        .filter(|n| {
            by_name.get(n).is_some_and(|rs| {
                let want = rs[0].arity;
                s.eval::<i64>(&format!(
                    "local ok, n = pcall(function() return (function(...) return arg.n end)({n}()) end) \
                     if not ok then return -1 end return n"
                ))
                .is_ok_and(|got| got >= 0 && got as usize == want)
            })
        })
        .collect();
    assert!(
        stale.is_empty(),
        "these now answer the reference's arity and must come off NOT_YET_ASSERTED: {stale:?}"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {checked} probed bindings answer the wrong number of values:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// The base-library probes, a name and its call arguments: these names follow no query-verb
/// convention. The arguments are what the 5.0 signatures accept, chosen to discriminate; `setfenv`
/// is left out because it would change the environment every later probe runs in.
const BASELIB_PROBES: &[(&str, &str)] = &[
    ("collectgarbage", ""),
    ("date", ""),
    // `debugbreak`, `debugdump`, `debuginfo`, `debugload`, `debugprint` and `debugtimestamp` are
    // the reference's six `xor eax,eax; ret` stubs (`0x7027e0` to `0x702830`), safe to call.
    ("debugbreak", ""),
    ("debugdump", ""),
    ("debuginfo", ""),
    ("debugload", ""),
    ("debugprint", ""),
    ("debugprofilestart", ""),
    ("debugprofilestop", ""),
    ("debugstack", ""),
    ("debugtimestamp", ""),
    ("gcinfo", ""),
    ("geterrorhandler", ""),
    ("getfenv", ""),
    ("time", ""),
    // ── probes that take arguments ──────────────────────────────────────────────────────────
    // Two arguments: only a multi-argument call tells 5.0's `lua_settop(L,1); return 1` from
    // 5.1's `return lua_gettop(L)`.
    ("assert", "1, 2"),
    ("getglobal", "\"BenillaShapeGateAbsent\""),
    ("getmetatable", "{}"),
    ("pairs", "{}"),
    ("rawequal", "1, 1"),
    ("rawget", "{}, 1"),
    ("rawset", "{}, 1, 1"),
    // These write, but namespaced or self-restoring, so every later probe sees the same VM.
    ("seterrorhandler", "geterrorhandler()"),
    ("setglobal", "\"BenillaShapeGateProbe\", 1"),
    ("setmetatable", "{}, nil"),
    ("tonumber", "\"1\""),
    ("tostring", "nil"),
    ("type", "nil"),
];

/// The base library's list, same rules as [`NOT_YET_ASSERTED`].
const BASELIB_NOT_YET_ASSERTED: &[(&str, usize, &str)] = &[];

#[test]
fn the_base_library_answers_the_reference_s_return_arity_and_kinds() {
    let rows = rows_of_kind("baselib");
    assert!(
        rows.len() > 20,
        "the vendored table's baselib rows look wrong: {}",
        rows.len()
    );
    let s = benilla_ui::script::UiScript::new().expect("VM");
    let mut checked = 0usize;
    let mut kinds_checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for r in &rows {
        let Some((_, args)) = BASELIB_PROBES.iter().find(|(n, _)| *n == r.name) else {
            continue;
        };
        let name = &r.name;
        let probe = format!(
            "if type({name}) ~= 'function' then return -1 end \
             local ok, n = pcall(function() return (function(...) return arg.n end)({name}({args})) end) \
             if not ok then return -1 end return n"
        );
        let got: i64 = match s.eval(&probe) {
            Ok(n) => n,
            Err(_) => continue,
        };
        if got < 0 {
            continue;
        }
        checked += 1;
        if got as usize != r.arity {
            if let Some((_, known, _)) = BASELIB_NOT_YET_ASSERTED.iter().find(|(n, ..)| n == name) {
                assert_eq!(*known, r.arity, "{name} is listed at a stale arity");
                continue;
            }
            mismatches.push(format!(
                "{name}: answers {got}, reference states {}",
                r.arity
            ));
            continue;
        }
        // Kinds only where `rows_of_kind` kept them as trustworthy.
        if r.kinds.is_empty() {
            continue;
        }
        let Ok(kinds) = s.eval::<String>(&format!(
            "local t = {{ {name}({args}) }} local out = '' \
             for i = 1, {} do out = out .. (i > 1 and ',' or '') .. type(t[i]) end return out",
            r.arity
        )) else {
            continue;
        };
        kinds_checked += 1;
        let got_tuple = format!("({kinds})");
        let acceptable = r.kinds.split('|').map(str::trim).any(|alt| {
            alt == got_tuple
                || alt
                    .trim_matches(|c| c == '(' || c == ')')
                    .split(',')
                    .map(str::trim)
                    .zip(kinds.split(',').map(str::trim))
                    // `string?` is string-or-nil; `any`/`value` accept anything.
                    .all(|(want, got)| {
                        want == got
                            || (want == "string?" && (got == "string" || got == "nil"))
                            || want == "any"
                            || want == "value"
                    })
        });
        if !acceptable {
            mismatches.push(format!(
                "{name}: answers kinds {got_tuple}, reference states {}",
                r.kinds
            ));
        }
    }

    // A floor, not a target.
    assert!(
        checked >= 28,
        "the base-library gate measured only {checked} bindings"
    );
    // The kinds comparison has its own floor: the trust narrowing can take it to zero while the
    // arity floor stays green.
    assert!(
        kinds_checked >= 16,
        "the base-library gate compared kinds on only {kinds_checked} bindings"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {checked} probed base-library bindings diverge:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// The widget registrar tables with their class and an instance name. A method name can sit in
/// several tables at different arities, so each method is probed on its own table's class; an
/// unmapped table is skipped.
const WIDGET_PROBES: &[(&str, &str, &str)] = &[
    ("0x878ec0", "Frame", "PGFrame"),
    ("0x879d00", "Button", "PGButton"),
    ("0x87bf74", "CheckButton", "PGCheck"),
    ("0x87bb68", "EditBox", "PGEdit"),
    ("0x87b260", "Slider", "PGSlider"),
    ("0x87b010", "StatusBar", "PGStatus"),
    ("0x87b3c0", "ScrollFrame", "PGScroll"),
    ("0x87ba80", "SimpleHTML", "PGHtml"),
    ("0x87abb0", "ColorSelect", "PGColor"),
    ("0x87b960", "MessageFrame", "PGMessage"),
    ("0x87b5c0", "ScrollingMessageFrame", "PGScrollMsg"),
    ("0x878948", "Model", "PGModel"),
    ("0x854198", "GameTooltip", "PGTip"),
    ("0x84c538", "Minimap", "PGMinimap"),
    ("0x84ee40", "TabardModel", "PGTabard"),
    ("0x84f190", "DressUpModel", "PGDress"),
    // `PlayerModel` is `Model` plus three verbs, in a table of its own.
    ("0x84f1fc", "PlayerModel", "PGPlayerModel"),
    ("0x87ab4c", "MovieFrame", "PGMovie"),
    ("0x847ce4", "LootButton", "PGLoot"),
    // The base every widget falls back to (`REGION_MAP_METHODS`' 19 names), probed on a Frame.
    ("0x87c9b8", "Frame", "PGRegionBase"),
];

/// The classes `CreateFrame` cannot make: the two regions, made by a frame, and the font object.
const REGION_PROBES: &[(&str, &str)] = &[
    ("0x87c128", "PGFrame:CreateTexture('PGTex')"),
    ("0x87c1d8", "PGFrame:CreateFontString('PGFS')"),
    // The font object (`CreateFont`), not a FontString: 22 methods of its own.
    ("0x87c7c8", "CreateFont('PGFontObject')"),
];

/// The widget arity gate: the global gate's rules, with each method called on an instance of its
/// own registrar table's class.
#[test]
fn every_widget_method_answers_the_reference_s_return_arity() {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-shapes.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/1.12-shapes.tsv");

    let s = benilla_ui::script::UiScript::new().expect("VM");
    let mut made: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for (table, kind, name) in WIDGET_PROBES {
        if s.run(&format!(
            "{name} = CreateFrame(\"{kind}\", \"{name}\", UIParent)"
        ))
        .is_ok()
        {
            made.insert(table, (*name).to_string());
        }
    }
    for (table, expr) in REGION_PROBES {
        let var = format!("PGR{}", made.len());
        if s.run(&format!("{var} = {expr}")).is_ok() {
            made.insert(table, var);
        }
    }
    assert!(
        made.len() >= 20,
        "only {} widget classes could be instantiated — the probe set is broken",
        made.len()
    );

    let mut checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("name\t"))
    {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 9 || f[4] != "widget" || f[8] != "exact" {
            continue;
        }
        let (name, table) = (f[0], f[3]);
        if !["Get", "Is", "Has", "Can", "Num"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let Some(obj) = made.get(table) else { continue };
        let Ok(want) = f[7].parse::<usize>() else {
            continue;
        };

        let probe = format!(
            "if type({obj}.{name}) ~= 'function' then return -1 end \
             local ok, n = pcall(function() return (function(...) return arg.n end)({obj}:{name}()) end) \
             if not ok then return -1 end return n"
        );
        let Ok(got) = s.eval::<i64>(&probe) else {
            continue;
        };
        if got < 0 {
            continue;
        }
        checked += 1;
        if got as usize != want {
            if WIDGET_NOT_YET_ASSERTED.contains(&name) {
                continue;
            }
            mismatches.push(format!(
                "{obj}:{name} answers {got}, reference states {want}"
            ));
        }
    }

    assert!(
        // A floor under the coverage of all 23 widget tables.
        checked >= 110,
        "the widget gate measured only {checked} methods — it has stopped covering anything"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {checked} probed widget methods answer the wrong number of values:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// Splits a `kinds` cell into alternatives of per-slot kinds: `(nil) | (number)` is two of one
/// slot, and `()` yields none, so the kind gates skip a verb that answers nothing.
fn kind_alternatives(cell: &str) -> Vec<Vec<String>> {
    cell.split('|')
        .map(|alt| {
            alt.trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .split(',')
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect::<Vec<String>>()
        })
        .filter(|a| !a.is_empty())
        .collect()
}

/// Whether an observed Lua `type()` satisfies one reference slot. `string?` is string-or-nil:
/// `lua_pushstring` (`0x6f3890`) pushes nil for a NULL pointer, and the harvest writes plain
/// `string` only for a constant literal. `any` and `value` are slots the trace could not narrow.
fn slot_accepts(want: &str, got: &str) -> bool {
    match want {
        "any" | "value" => true,
        "string?" => got == "string" || got == "nil",
        w => w == got,
    }
}

/// The Lua helper the kind gates probe through: the `type()` of every returned value, in order,
/// comma-joined. It counts with `arg.n`: a table constructor drops trailing nils, and `...` as a
/// value is a syntax error in 5.0 (no `TK_DOTS` arm in `simpleexp`), which this VM's parser keeps.
const KIND_HELPER: &str = "function PGKinds(...) \
     local o = {} for i = 1, arg.n do o[i] = type(arg[i]) end \
     return table.concat(o, ',') end";

/// Every registered global whose return kinds the reference states trustworthily, answered by
/// this client, under the arity gate's narrowings.
#[test]
fn every_query_binding_answers_the_reference_s_return_kinds() {
    let all = rows();
    let mut by_name: std::collections::HashMap<&str, Vec<&Row>> = std::collections::HashMap::new();
    for r in &all {
        by_name.entry(&r.name).or_default().push(r);
    }

    let mut s = benilla_ui::script::UiScript::new().expect("VM");
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Shapeprobe".into()),
            level: 60,
            ..Default::default()
        }),
    );
    s.run(KIND_HELPER).expect("kind helper");

    let mut checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for (name, rs) in &by_name {
        if !["Get", "Is", "Has", "Can", "Unit", "Num"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let candidates: Vec<&&Row> = if rs.len() == 1 {
            rs.iter().collect()
        } else {
            rs.iter().filter(|r| r.table_va != GLUE_TABLE).collect()
        };
        let Some(first) = candidates.first() else {
            continue;
        };
        if candidates.iter().any(|r| r.kinds != first.kinds) {
            continue;
        }
        let want = kind_alternatives(&first.kinds);
        if want.is_empty() {
            continue;
        }

        let call = if name.starts_with("Unit") {
            format!(r#"{name}("player")"#)
        } else {
            format!("{name}()")
        };
        let probe = format!(
            "if type({name}) ~= 'function' then return '?' end \
             local ok, s = pcall(function() return PGKinds({call}) end) \
             if not ok then return '?' end return s"
        );
        let Ok(got) = s.eval::<String>(&probe) else {
            continue;
        };
        if got == "?" {
            continue;
        }
        let got: Vec<&str> = got.split(',').filter(|k| !k.is_empty()).collect();
        // An arity fault is the arity gate's to report.
        if !want.iter().any(|alt| alt.len() == got.len()) {
            continue;
        }
        checked += 1;
        let agrees = want.iter().any(|alt| {
            alt.len() == got.len()
                && alt
                    .iter()
                    .zip(&got)
                    .all(|(w, g)| slot_accepts(w.as_str(), g))
        });
        let observed = got.join(",");
        if !agrees {
            if let Some((_, known, _)) = KINDS_NOT_YET_ASSERTED.iter().find(|(n, ..)| n == name) {
                assert_eq!(
                    *known, observed,
                    "{name} is on the not-yet-asserted list at stale kinds — it now answers                      ({observed})"
                );
                continue;
            }
            mismatches.push(format!(
                "{name}: answers ({observed}), reference states {}",
                first.kinds
            ));
        } else {
            assert!(
                !KINDS_NOT_YET_ASSERTED.iter().any(|(n, ..)| n == name),
                "{name} now agrees with the reference and must come off KINDS_NOT_YET_ASSERTED"
            );
        }
    }

    // A floor, not a target.
    assert!(
        checked >= 150,
        "the kind gate measured only {checked} bindings — it has stopped covering anything"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {checked} probed bindings answer values of the wrong kind:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// The widget kind gate, on an instance of each method's own registrar class.
#[test]
fn every_widget_method_answers_the_reference_s_return_kinds() {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-shapes.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/1.12-shapes.tsv");

    let s = benilla_ui::script::UiScript::new().expect("VM");
    s.run(KIND_HELPER).expect("kind helper");
    let mut made: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for (table, kind, name) in WIDGET_PROBES {
        if s.run(&format!(
            "{name} = CreateFrame(\"{kind}\", \"{name}\", UIParent)"
        ))
        .is_ok()
        {
            made.insert(table, (*name).to_string());
        }
    }
    for (table, expr) in REGION_PROBES {
        let var = format!("PGR{}", made.len());
        if s.run(&format!("{var} = {expr}")).is_ok() {
            made.insert(table, var);
        }
    }

    let mut checked = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("name\t"))
    {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 11 || f[4] != "widget" || f[8] != "exact" || f[10] != "agree" {
            continue;
        }
        let (name, table) = (f[0], f[3]);
        if !["Get", "Is", "Has", "Can", "Num"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        let Some(obj) = made.get(table) else { continue };
        let want = kind_alternatives(f[9]);
        if want.is_empty() {
            continue;
        }

        let probe = format!(
            "if type({obj}.{name}) ~= 'function' then return '?' end \
             local ok, s = pcall(function() return PGKinds({obj}:{name}()) end) \
             if not ok then return '?' end return s"
        );
        let Ok(got) = s.eval::<String>(&probe) else {
            continue;
        };
        if got == "?" {
            continue;
        }
        let got: Vec<&str> = got.split(',').filter(|k| !k.is_empty()).collect();
        if !want.iter().any(|alt| alt.len() == got.len()) {
            continue;
        }
        checked += 1;
        let agrees = want.iter().any(|alt| {
            alt.len() == got.len()
                && alt
                    .iter()
                    .zip(&got)
                    .all(|(w, g)| slot_accepts(w.as_str(), g))
        });
        let observed = got.join(",");
        if !agrees {
            if let Some((_, known, _)) = WIDGET_KINDS_NOT_YET_ASSERTED
                .iter()
                .find(|(n, ..)| *n == name)
            {
                assert_eq!(
                    *known, observed,
                    "{name} is on the widget not-yet-asserted list at stale kinds — it now                      answers ({observed})"
                );
                continue;
            }
            mismatches.push(format!(
                "{obj}:{name} answers ({observed}), reference states {}",
                f[9]
            ));
        }
    }

    assert!(
        // A floor under the coverage of all 23 widget tables.
        checked >= 110,
        "the widget kind gate measured only {checked} methods — it has stopped covering anything"
    );
    assert!(
        mismatches.is_empty(),
        "{} of {checked} probed widget methods answer values of the wrong kind:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// Argument tuples the boolean gate tries in order until a call returns. A plausible wrong
/// argument can change which kinds come back, which is why the kind gate passes only a unit
/// token, but it never turns `1` into `true`.
const ARG_LADDER: &[&str] = &["", "\"player\"", "1", "1, 1", "\"player\", 1"];

/// 1.12 has no boolean query: of the table's trustworthy rows only `IsPetAttackActive` and Lua's
/// own `rawequal` answer a `boolean`, and every other predicate pushes `1` or `nil`, which
/// `x == 1`, `tostring(x)` or a saved variable tell apart from `true`.
#[test]
fn no_query_binding_answers_a_lua_boolean() {
    // The reference's own two; the list may only shrink.
    const REFERENCE_BOOLEANS: &[&str] = &["IsPetAttackActive", "rawequal"];

    let all = rows();
    let mut s = benilla_ui::script::UiScript::new().expect("VM");
    s.set_unit(
        "player",
        Some(benilla_ui::script::UnitState {
            exists: true,
            name: Some("Shapeprobe".into()),
            level: 60,
            ..Default::default()
        }),
    );
    s.run(KIND_HELPER).expect("kind helper");

    let mut names: Vec<&str> = all
        .iter()
        .map(|r| r.name.as_str())
        .filter(|n| {
            ["Get", "Is", "Has", "Can", "Unit", "Num"]
                .iter()
                .any(|p| n.starts_with(p))
        })
        .filter(|n| !REFERENCE_BOOLEANS.contains(n))
        .collect();
    names.sort_unstable();
    names.dedup();

    let mut checked = 0usize;
    let mut booleans: Vec<String> = Vec::new();
    for name in names {
        for args in ARG_LADDER {
            let probe = format!(
                "if type({name}) ~= 'function' then return '?' end \
                 local ok, s = pcall(function() return PGKinds({name}({args})) end) \
                 if not ok then return '?' end return s"
            );
            let Ok(got) = s.eval::<String>(&probe) else {
                continue;
            };
            if got == "?" {
                continue;
            }
            checked += 1;
            if got.split(',').any(|k| k == "boolean") {
                booleans.push(format!("{name}({args}) answers ({got})"));
            }
            break;
        }
    }

    assert!(
        checked >= 300,
        "the boolean gate measured only {checked} bindings — it has stopped covering anything"
    );
    assert!(
        booleans.is_empty(),
        "{} bindings answer a Lua boolean; 1.12 pushes 1/nil and only {REFERENCE_BOOLEANS:?} are \
         boolean in the reference:\n  {}",
        booleans.len(),
        booleans.join("\n  ")
    );
}

/// The widget boolean gate. Its argument ladder reaches the predicates the kind gate skips for
/// raising without one, such as `HasScript("OnClick")` and `IsObjectType("Frame")`.
#[test]
fn no_widget_method_answers_a_lua_boolean() {
    let tsv = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../reference/1.12-shapes.tsv"
    );
    let text = std::fs::read_to_string(tsv).expect("reference/1.12-shapes.tsv");

    let s = benilla_ui::script::UiScript::new().expect("VM");
    s.run(KIND_HELPER).expect("kind helper");
    let mut made: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for (table, kind, name) in WIDGET_PROBES {
        if s.run(&format!(
            "{name} = CreateFrame(\"{kind}\", \"{name}\", UIParent)"
        ))
        .is_ok()
        {
            made.insert(table, (*name).to_string());
        }
    }
    for (table, expr) in REGION_PROBES {
        let var = format!("PGR{}", made.len());
        if s.run(&format!("{var} = {expr}")).is_ok() {
            made.insert(table, var);
        }
    }

    // A widget predicate takes a script or object-type name more often than a number.
    const WIDGET_ARGS: &[&str] = &["", "\"OnClick\"", "\"Frame\"", "1", "1, 1"];

    let mut checked = 0usize;
    let mut booleans: Vec<String> = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("name\t"))
    {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 9 || f[4] != "widget" || f[8] != "exact" {
            continue;
        }
        let (name, table) = (f[0], f[3]);
        if !["Get", "Is", "Has", "Can", "Num"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            continue;
        }
        // No widget method answers a boolean in the reference, so none is exempt here.
        let Some(obj) = made.get(table) else { continue };
        for args in WIDGET_ARGS {
            let probe = format!(
                "if type({obj}.{name}) ~= 'function' then return '?' end \
                 local ok, s = pcall(function() return PGKinds({obj}:{name}({args})) end) \
                 if not ok then return '?' end return s"
            );
            let Ok(got) = s.eval::<String>(&probe) else {
                continue;
            };
            if got == "?" {
                continue;
            }
            checked += 1;
            if got.split(',').any(|k| k == "boolean") {
                booleans.push(format!("{obj}:{name}({args}) answers ({got})"));
            }
            break;
        }
    }

    assert!(
        checked >= 100,
        "the widget boolean gate measured only {checked} methods — it has stopped covering anything"
    );
    assert!(
        booleans.is_empty(),
        "{} widget methods answer a Lua boolean; every predicate row in the reference's widget \
         tables is `(nil) | (number)`:\n  {}",
        booleans.len(),
        booleans.join("\n  ")
    );
}
