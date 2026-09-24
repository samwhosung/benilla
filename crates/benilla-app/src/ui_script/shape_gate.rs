//! **The return-shape gate** — `reference/1.12-shapes.tsv` against what this client actually
//! answers (decision 1842).
//!
//! `reference_surface` compares `_G` to `_G`, so a wrong *name* cannot land. Nothing compared
//! *shapes*, and that gap produced six decisions in two days — 1818, 1819, 1830, 1834, 1836 and
//! 1840 — every one of them a right name with a wrong arity, invisible to every gate we had and
//! invisible to the chain's own files too, because a Lua caller that uses one value of two never
//! notices the second.
//!
//! **Arity is only half a shape.** A binding that answers the right *number* of values of the
//! wrong *kind* is the same silent break one level down, and it is the shape the 1.12 API is most
//! exposed to: the reference has no boolean-returning query at all — every `Is*`/`Can*`/`Has*` row
//! in the table is `(nil) | (number)`, because the client pushes `1` and `nil`, never `true` and
//! `false`. An addon writing `if IsShiftKeyDown() == 1` — and 87 corpus addons compare returns to
//! `1` — reads a `true` as "not held". So the kinds column is gated as well, wherever the table
//! says it is trustworthy (decision 2118).
//!
//! The table is a harvest of the reference binary, vendored: 1722 rows over all 82 registrar
//! tables, one per registered binding, generated and differentially tested against a second
//! harvester. It carries the gate rule **per row** rather than in prose, which is the whole
//! reason it can be enforced instead of remembered:
//!
//! * `arity_conf = exact` — sound. This is the column to gate on.
//! * `kinds_conf = agree` — the row's independent `lua_push*` trace matched `eax` at every `ret`,
//!   which is the table's own statement that the kinds are trustworthy. Gated too, by
//!   [`every_query_binding_answers_the_reference_s_return_kinds`]; a row that did not agree is
//!   advisory and this module never reads its kinds at all.
//! * `argc_conf` — **never** gate on it. It is a deliberate over-flagging superset: any binding
//!   whose direct callee reads a Lua index is marked `lower-bound`, so a `lower-bound` row says
//!   nothing about the real argument count.
//!
//! ## Two things that make this narrower than it looks, both on purpose
//!
//! **Only query-shaped names are probed.** The gate calls each binding to count what it answers,
//! and calling an arbitrary global for its arity would mean calling `AbandonQuest`, `Quit` and
//! `AcceptBattlefieldPort` for theirs. Names are filtered to the query verbs — `Get*`, `Is*`,
//! `Has*`, `Can*`, `Unit*`, `Num*` — which is a loss of coverage, never a wrong assertion.
//!
//! **A call that raises is skipped, not failed.** A raise says nothing about arity, so only a call
//! that *returns* is measured. That rule used to cost the whole unit surface: the unit bindings
//! gate their token and raise on a nil one (1834/1836), so a no-argument probe threw for every one
//! of them, and the 62 `Unit*` rows this table calls `arity_conf = exact` were silently
//! uncovered — the gap named in this doc and never priced. The probe now **seats a player** and
//! calls `Unit*` with a real token, which took coverage from **220 bindings to 249**. All 29 newly
//! measured agreed with the reference on the first run; the value here is that they cannot drift
//! away from it unnoticed, not that anything was found wrong. The remainder still raise, needing
//! state a seated body does not supply, and are still skipped.
//!
//! ## The name is not a key
//!
//! **60 names are registered from more than one table and 21 differ in arity across
//! registrations** — `GetBuildInfo` is arity 5 in the glue table `0x8373b8` and 3 in the in-game
//! core `0x83de68`; `GetAddOnInfo` is 8 and 7. This VM is the in-game one, so a name carrying rows
//! from both is read at its **non-glue** row, and a name whose remaining rows still disagree is
//! skipped rather than guessed at.
//!
//! That hazard is the reason the table is keyed on `fn` upstream, and it is a live merge hazard for
//! `1.12-globals.tsv`, which is name-keyed.

/// Names the gate knows are wrong and does not yet assert — **a list that may only shrink**.
///
/// **It is empty.** It held five entries whose miss-branch *contents* the shapes table does not
/// record — four globals and `Model:GetFogColor` — and 1845 answered all five at the bytes rather
/// than guessing them. The mechanism stays because the next divergence the gate finds will want it:
/// an entry needs an arity, a reason, and a dispatched question.
const NOT_YET_ASSERTED: &[(&str, usize, &str)] = &[];

/// The **kind** gate's own shrinking list, same rules as [`NOT_YET_ASSERTED`]: a name, the wrong
/// kinds it currently answers, and the dispatched question that will settle it. An entry whose
/// binding has come into agreement fails the gate, so it cannot outlive its fix — and both of the
/// two it held did exactly that, within the day.
///
/// **It is empty.** It held `GetSendMailItem`, whose no-attachment leg the shapes table types as
/// two numbers without their values. The bytes answer it — `0x4ae590` pushes `(nil, nil, 0, 0)`,
/// and the stray `1` we had was borrowed from `GetAuctionSellItemInfo`'s empty leg — and 2129
/// folded the verdict back.
const KINDS_NOT_YET_ASSERTED: &[(&str, &str, &str)] = &[];

/// The widget kind gate's shrinking list — see [`KINDS_NOT_YET_ASSERTED`].
///
/// **Also empty.** It held `GetFont`, whose height slot the table types as a number and which we
/// answered nil on a FontString that was never given a font. The recorded note read as though a
/// NULL `CGxFont` should nil all three slots; the bytes say otherwise — `0x7727b0`'s unconditional
/// `fld [esi+0xe4]` sits in front of the `+0xe0` test, behind a branch `GetFont` never takes — and
/// 2129 folded that back too.
const WIDGET_KINDS_NOT_YET_ASSERTED: &[(&str, &str, &str)] = &[];

/// The widget half's own shrinking list — see [`NOT_YET_ASSERTED`], same rules. Also empty:
/// `GetFogColor` came off it with 1845, which found the fog colour was a packed `0xAARRGGBB` dword
/// all along.
const WIDGET_NOT_YET_ASSERTED: &[&str] = &[];

/// The glue registrar table. This VM is the in-game one, so a duplicated name is read at its
/// other row (see the module doc).
const GLUE_TABLE: &str = "0x8373b8";

struct Row {
    name: String,
    table_va: String,
    arity: usize,
    /// The `kinds` column, verbatim — `(number,string?)`, or several `|`-separated alternatives.
    /// Empty when the row's `kinds_conf` is not `agree`; see [`KindGate`].
    kinds: String,
}

fn rows() -> Vec<Row> {
    rows_of_kind("global")
}

/// The same read, for one `table_kind` — `global` is the registrar surface, `baselib` the
/// 36-entry Lua base array looped straight into `_G` at `0x811e28`.
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
            // **`agree` is necessary and not sufficient, and the second condition is the table's
            // own too** (2150). The kinds come from tracing each binding's `lua_push*` calls; a row
            // whose notes carry `delegated-push` has a CALLEE that pushes on its own behalf, which
            // the trace sees one level too high up, so its tuple covers the binding's own ops only.
            // `eax` is unaffected, so arity stays gated on every row — it is the kinds claim alone
            // that narrows.
            let delegated = f.len() > 11 && f[11].contains("delegated-push");
            Some(Row {
                name: f[0].to_string(),
                table_va: f[3].to_string(),
                arity: f[7].parse().ok()?,
                // ADVISORY unless `kinds_conf == agree` — the table's own column contract. A row
                // that did not agree carries no kinds here at all, so nothing downstream can read
                // one by accident.
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

    // Resolve the duplicated names the way the module doc describes.
    let mut by_name: std::collections::HashMap<&str, Vec<&Row>> = std::collections::HashMap::new();
    for r in &all {
        by_name.entry(&r.name).or_default().push(r);
    }

    let mut s = benilla_ui::script::UiScript::new().expect("VM");
    // **Seat a player, so the unit surface can be probed at all.** The unit bindings gate their
    // token and raise on a nil one (1834/1836), so a no-argument probe measured *nothing* for the
    // 62 `Unit*` rows this table calls `arity_conf = exact` — the gate's own doc named that gap in
    // prose and nobody had priced it. With a body seated they answer instead of raising, which is
    // the difference between covering the unit surface and skipping it.
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
        // Query verbs only — the gate calls what it measures.
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

        // A call that raises still says nothing about arity. The unit bindings are the reason:
        // they gate their arguments and raise on a nil token (1834/1836), so they are called with
        // a real token against the body seated above rather than with nothing.
        //
        // The count is taken with 5.0's own `arg.n`, for the reason [`KIND_HELPER`] states one
        // gate down plus the one that settled it: `select` is 5.1's base library, not a 1.12
        // global, and `lua50::install` removes it. Where the count is wanted on the HOST side we
        // ask `UiScript::arity`; these four probes have to hold it INSIDE a `pcall`, and there
        // the vararg's own `n` is the 1.12 spelling.
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

    // A floor, not a target: it exists so a change that quietly stops the probe from measuring
    // anything fails loudly instead of passing vacuously. Seating a player took the count from
    // **220 to 249** by making the unit bindings answerable, so the old floor of 40 was six times
    // below reality and would have sat green through losing the entire unit surface. Raise this
    // whenever coverage rises; never lower it to make a change fit.
    assert!(
        checked >= 240,
        "the gate measured only {checked} bindings — it has stopped covering anything"
    );
    // The list may only SHRINK. An entry that now agrees is a fix nobody deleted the note for,
    // and leaving it would let the next real divergence hide behind it.
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

/// **The base-library arm of the gate** (decision 2136) — the 36 entries at `0x811e28`, which the
/// other three arms never looked at.
///
/// [`rows`] filters to `table_kind = global`, so every `baselib` row in the vendored table sat
/// outside all of the arity, kinds and widget halves. That is not a small corner: it is `gcinfo`,
/// `collectgarbage`, `loadstring`, `pairs`, `type`, `tostring` — the functions every addon calls
/// most — and it is where two divergences sat with their correct arities recorded beside them the
/// whole time. `gcinfo … 2 exact (number,number) agree` and `collectgarbage … 0 exact () agree`
/// were both in this file before anyone answered either one wrong.
///
/// **The base library is probed by an explicit ladder — a name and the arguments to call it
/// with** — rather than a prefix rule. The global arm can filter to query verbs because
/// `Get*`/`Is*` names them; the base library has no such convention, and calling it blind means
/// calling `error`, `pcall` and `setfenv` for their arities.
///
/// ## Why an argument ladder is safe HERE and was not for the global arm
///
/// 2129 §Why left the ladder question open on exactly one hazard: *a wrong-but-plausible argument
/// can produce a legitimate kind tuple outside the reference's reachable set, which is a false
/// positive — the most expensive gate failure.* That hazard is real for a game binding, where the
/// argument's meaning is the unknown (what index does `GetInboxItem` accept, and what does the
/// mailbox hold when it does?).
///
/// It does not transfer to the base library, and the reason is worth stating rather than
/// assuming: these are stock Lua 5.0 functions whose argument **types** are fixed by the language
/// and whose bodies were read at the bytes. `type` takes any value, `rawget` a table and a key,
/// `pairs` a table. There is no state to be in the wrong shape, so the argument below is not a
/// guess about the reference — it is the only shape the function accepts.
///
/// The arguments are chosen to **discriminate**, not merely to avoid a raise: `assert` is called
/// with TWO arguments precisely because its `arity 1 exact` row is only falsifiable that way — a
/// one-argument call answers one value in both dialects and proves nothing. (That probe is what
/// found 5.1's `return lua_gettop(L)` still in place here; see `script::lua50::install_assert`.)
///
/// Excluded, and why: `error`/`pcall`/`xpcall` and the rows whose `arity_conf` is not `exact`
/// (`next`, `unpack`, `ipairs`, `loadstring`) are filtered out upstream anyway; `setfenv` is left
/// out because it mutates the environment every later probe runs in, which is a different risk
/// from raising.
const BASELIB_PROBES: &[(&str, &str)] = &[
    ("collectgarbage", ""),
    ("date", ""),
    // The five shipped stubs — `xor eax,eax; ret` in the image (`0x7027e0`–`0x702820`), so calling
    // one is as safe here as it is there, and their `0 exact ()` rows are still worth holding.
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
    // ── the ladder (2136's "left open", closed) ──────────────────────────────────────────────
    // TWO arguments on purpose: the row is `arity 1 exact` and only a multi-argument call can
    // tell 5.0's `lua_settop(L,1); return 1` from 5.1's `return lua_gettop(L)`.
    ("assert", "1, 2"),
    ("getglobal", "\"BenillaShapeGateAbsent\""),
    ("getmetatable", "{}"),
    ("pairs", "{}"),
    ("rawequal", "1, 1"),
    ("rawget", "{}, 1"),
    ("rawset", "{}, 1, 1"),
    // Both write, and both are namespaced or self-restoring so the VM the next probe sees is the
    // one it would have seen anyway.
    ("seterrorhandler", "geterrorhandler()"),
    ("setglobal", "\"BenillaShapeGateProbe\", 1"),
    ("setmetatable", "{}, nil"),
    ("tonumber", "\"1\""),
    ("tostring", "nil"),
    ("type", "nil"),
];

/// The base library's own shrinking list — same rules as [`NOT_YET_ASSERTED`]. **Empty.**
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
        // Kinds, wherever the table calls them trustworthy — the same `kinds_conf = agree` rule
        // the global arm uses, and the column that types `gcinfo` as `(number,number)`. The
        // `delegated-push` narrowing is applied once, in [`rows_of_kind`], for every arm.
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

    // A floor, so a change that stops the probe measuring anything fails loudly.
    // A floor, not a target: raise it when coverage rises, never lower it to fit a change.
    assert!(
        checked >= 28,
        "the base-library gate measured only {checked} bindings"
    );
    // **The kinds half gets its own floor.** Arity coverage says nothing about how many rows the
    // kinds comparison actually ran on, and the `kinds_conf`/`delegated-push` narrowing is exactly
    // the kind of change that can quietly take it to zero while the arity floor stays green.
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

/// The widget registrar tables, each mapped to a Lua expression that produces one instance.
///
/// The table address IS the widget class — the reference registers one `{name, fn}` table per
/// widget family, and a method name can appear in several of them with different arities (the
/// `name-not-unique` case). So a widget method is probed on **its own class**, never on whichever
/// object happens to answer to the name.
///
/// Identified by each table's distinctive members: `GetTexCoord` is Texture's, `GetChecked` is
/// CheckButton's, `GetHyperlinkFormat` is SimpleHTML's, `GetScrollChild` is ScrollFrame's, and so
/// on. A table with no entry here is skipped, which costs coverage and never correctness.
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
    // ── The eleven tables this list did NOT cover until 2142 ──────────────────────────────────
    // Twelve of the reference's twenty-three widget tables were mapped, so 11 classes — 190 of
    // its 481 widget rows, `GameTooltip`'s 46 among them — were skipped by BOTH the arity gate
    // and the kind gate, silently and by omission rather than by a stated narrowing. The
    // identities are the corpus census's (2142), each read off its table's own distinctive
    // members rather than guessed: `SetOwner`/`AddDoubleLine` is GameTooltip's, `CopyFontObject`
    // the Font object's, `PingLocation` the Minimap's, `Dress`/`TryOn` DressUpModel's,
    // `CycleVariation` TabardModel's, `StartMovie` MovieFrame's, `SetSlot` LootButton's.
    ("0x854198", "GameTooltip", "PGTip"),
    ("0x84c538", "Minimap", "PGMinimap"),
    ("0x84ee40", "TabardModel", "PGTabard"),
    ("0x84f190", "DressUpModel", "PGDress"),
    // `PlayerModel` is `Model` plus three verbs, so its own table needs its own instance — the
    // `0x878948` row above is now the plain `Model` it always was.
    ("0x84f1fc", "PlayerModel", "PGPlayerModel"),
    ("0x87ab4c", "MovieFrame", "PGMovie"),
    ("0x847ce4", "LootButton", "PGLoot"),
    // The base map every widget misses into (its 19 names are exactly `script/mod.rs`'s
    // `REGION_MAP_METHODS`). It has no class of its own, so it is probed on a Frame — which is
    // what "its own class" means for a base: every widget answers it.
    ("0x87c9b8", "Frame", "PGRegionBase"),
];

/// The two region classes, which are made by a frame rather than by `CreateFrame`.
const REGION_PROBES: &[(&str, &str)] = &[
    ("0x87c128", "PGFrame:CreateTexture('PGTex')"),
    ("0x87c1d8", "PGFrame:CreateFontString('PGFS')"),
    // The FONT OBJECT (`CreateFont`), not a FontString — 22 methods of its own, and the third
    // thing here that `CreateFrame` cannot make (2142).
    ("0x87c7c8", "CreateFont('PGFontObject')"),
];

/// **The widget half of the gate** (decision 1843) — the half that would have caught 1840's
/// `GetTexCoord` without a byte read.
///
/// Same rule as the global half: `arity_conf = exact` only, query verbs only, a call that raises is
/// skipped. The difference is the receiver: each row is probed on an instance of the class its
/// registrar table belongs to, because the same method name lives in several tables with different
/// shapes.
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
        // 121 at 2142, when the probe set went from 12 of the reference's 23 widget tables to
        // all 23. The floor it replaces was 60 — set at the coverage of 1843's day, and never
        // raised, so eleven whole classes could go unprobed without this number moving.
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

/// Split a `kinds` cell into its alternatives, each one a per-slot kind list.
///
/// `(number,string?)` is one alternative of two slots; `(nil) | (number)` is two of one. An
/// arity-0 row is `()` and yields nothing, which is how the kind gates skip the verbs that answer
/// no values at all.
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

/// Does one observed Lua `type()` satisfy one reference slot?
///
/// Two entries in the vocabulary assert less than they look like they do, and both are the table's
/// own doing rather than ours:
///
/// * **`string?` is string-or-nil.** `lua_pushstring 0x6f3890` begins `test edx,edx ; jne` and
///   tail-jumps to `lua_pushnil` when the `char*` is NULL, so a push instruction is not proof that
///   a string reached Lua. The harvester emits plain `string` only where the pointer is a constant
///   resolving to a readable literal; everything else is `string?`. This is *why* a miss path can
///   show a fixed arity with no kind alternation and still answer nil in that slot.
/// * **`any` / `value`** are the slots the walk could not narrow. They accept everything.
///
/// Everything else is exact — and note what is *absent* from the vocabulary across all 1503 rows
/// the table calls trustworthy: two `boolean` slots in the entire client, `IsPetAttackActive` and
/// Lua's own `rawequal`. A 1.12 predicate answers `1` or `nil`.
fn slot_accepts(want: &str, got: &str) -> bool {
    match want {
        "any" | "value" => true,
        "string?" => got == "string" || got == "nil",
        w => w == got,
    }
}

/// The Lua helper both kind gates probe through: answer the `type()` of every value a call
/// returned, in order, comma-joined. A table constructor would swallow the trailing nils, so the
/// count has to come from the vararg itself.
///
/// It reads `arg.n`, not `select('#', ...)`, and that is not a style choice: **`...` as a value is
/// a syntax error in this VM**, because 5.0 has no `TK_DOTS` arm in `simpleexp` and the parser
/// patch reproduces that (1192; `lua50::tests::the_vararg_expression_is_a_syntax_error_as_it_is_on_1_12`).
/// The 5.0 spelling is the implicit `arg` table, which is what all 177 stock FrameXML files use.
const KIND_HELPER: &str = "function PGKinds(...) \
     local o = {} for i = 1, arg.n do o[i] = type(arg[i]) end \
     return table.concat(o, ',') end";

/// Every registered global whose return *kinds* the reference states trustworthily, answered by
/// this client (decision 2118).
///
/// Same three narrowings as the arity gate, for the same reasons: query verbs only, a call that
/// raises is skipped, and a name registered from two tables is read at its non-glue row. A fourth
/// is specific to kinds — a row whose observed arity already disagrees is left to the arity gate,
/// because comparing a 3-tuple against a 4-slot reference reports the same fault twice.
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
        // An arity fault is the arity gate's to report; reporting it here too would double-count.
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

    // A floor, not a target — see the arity gate's. Raise it when coverage rises; never lower it
    // to make a change fit.
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

/// The widget half of the kind gate — same rules, probed on an instance of each method's own
/// registrar class (see [`WIDGET_PROBES`]).
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
        // 119 at 2142 — the same widening. The floor it replaces was 40.
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

/// Argument tuples the boolean gate tries, in order, until one call returns.
///
/// The kind gate probes a binding with nothing (or a unit token) and skips the ones that raise,
/// which leaves half the query surface unmeasured — the reference gates its arguments and raises
/// on a missing one (1834/1836). A *boolean* answer is wrong whatever the arguments are, so this
/// gate can afford a ladder the kind gate cannot: a wrong-but-plausible argument can change which
/// kind tuple comes back, and would make the kind gate cry wolf, but it can never make `1` into
/// `true`.
const ARG_LADDER: &[&str] = &["", "\"player\"", "1", "1, 1", "\"player\", 1"];

/// **1.12 has no boolean query.** Across all 1503 rows this table calls trustworthy, exactly two
/// slots are `boolean`: `IsPetAttackActive` and Lua's own `rawequal`. Everything else that reads
/// as a predicate — every `Is*`, `Can*`, `Has*` — is `(nil) | (number)`, because the client pushes
/// `1` and `nil`.
///
/// The distinction is invisible to `if x then` and decisive to everything else: `x == 1`,
/// `tostring(x)`, a value stored in a saved variable, arithmetic, a table key. `ColorPickerPlus`
/// writes `if IsShiftKeyDown() == 1 then` (decision 2118), and no gate we had could see it.
#[test]
fn no_query_binding_answers_a_lua_boolean() {
    // The reference's own two, by name — the list may only shrink.
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

/// The widget half of [`no_query_binding_answers_a_lua_boolean`], and the reason it is worth having
/// separately from the widget *kind* gate: the kind gate calls each method with no arguments and
/// skips whatever raises, so a predicate that gates its argument — `HasScript("OnClick")`,
/// `IsObjectType("Frame")` — is never measured. The argument ladder reaches those, and `boolean`
/// stays wrong under every one of them.
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

    // A widget predicate's argument is a script or object-type NAME far more often than a number,
    // so the ladder differs from the global one.
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
        // The reference's own boolean, and the only widget one: `IsPetAttackActive` is a global.
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
