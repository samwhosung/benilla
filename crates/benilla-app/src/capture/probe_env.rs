//! The probe fleet's **environment registry** — every `WOW_PROBE*` variable the app reads, in one
//! table the code is checked against in both directions.
//!
//! The set used to live in three places that could not see each other: the read sites (one per
//! variable, spread over the fleet), a hand-kept arming list in `dev.rs` for the un-occludable
//! window ([`super::ProbeFocusPlugin`]), and nothing at all that said what the
//! fleet accepts. The arming list is the one that bit: 0906's rule is that *every* scripted
//! probe defends itself against the macOS occlusion throttle, and the list had drifted to ten of
//! the twenty-five variables that schedule on the wall clock — a mail or auction probe launched
//! exactly as the method prescribes ran covered, at ~1 fps, measuring garbage.
//!
//! So the list is now a **column** here ([`ProbeVar::wall_clock`]), the arming reads the column,
//! and two structural tests below keep the table honest: every `"WOW_PROBE…"` string literal in
//! the crate is a row, every row is a literal somewhere other than this file, and every value
//! `WOW_PROBE` itself dispatches on is in [`PROBE_NAMES`]. `WOW_PROBE=list` prints it.
//!
//! What this is **not**: a dispatcher. The variables keep their own read sites and their own
//! shapes — several carry free-form values (Lua chunks, chat lines with spaces and `=`, `A>B`
//! drag specs) and several combine in one run — so folding them into one variable would need an
//! escaping mini-language and would change the launch line every session uses. The defect was
//! the drift, and a registry is the whole cure for drift.

/// One probe-fleet environment variable.
pub(crate) struct ProbeVar {
    /// The variable, exactly as the read site spells it.
    pub name: &'static str,
    /// One line: the value shape and what setting it does, from the comment at the read site.
    pub purpose: &'static str,
    /// Whether the variable **schedules on elapsed real time** — its script fires at `<secs>`
    /// marks, integrates a rate per frame, or steps a phase machine on `Time<Real>` — so the run
    /// is only right while frames keep arriving at full rate.
    ///
    /// Why it matters: macOS drops a fully covered window to ~1 fps drawables,
    /// and on such a window a wall-clock probe does not measure slowly, it **runs the wrong
    /// script** — one leg fired `W@16` and `Space@19` in the same frame and jumped from a
    /// standstill; another integrated its camera at 125 yd/s for 500 and never crossed the
    /// radius it was testing. Every `true` row therefore arms
    /// [`super::ProbeFocusPlugin`] from `dev.rs`, which keeps the probe window un-occludable
    /// (and, for a run that draws no pixels, parks it small in a corner).
    ///
    /// A `false` row is a flag or a modifier: a `_AT`/`_STEP`/`_KEEP` rides its parent's
    /// arming, a pricing lever changes *what* a run draws rather than *when*, and a state
    /// machine that advances on wire replies (the char-create probe) has no clock to be robbed of.
    pub wall_clock: bool,
}

/// Every `WOW_PROBE*` variable the app reads. Grouped by the probe that owns each; the order is
/// the order `WOW_PROBE=list` prints.
pub(crate) const PROBE_VARS: &[ProbeVar] = &[
    // ── The variable itself, and the run shell every scripted probe rides ────────────────────
    ProbeVar {
        name: "WOW_PROBE",
        purpose: "<name> — a value-dispatched live probe (see the names below); `list` prints this table",
        // Five of the six values step a phase machine on `time.elapsed_secs` (melee's 3 s swing
        // cadence, crossing/taxi/guardpoi's phases, castcancel's press-at-t); `partner` answers
        // invites the frame they land, but rides the same variable.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_EXIT_AT",
        purpose: "<secs> — exit the app after N wall seconds; bounds any scripted live probe's lifetime",
        // Fires on `ProbeClock` at `<secs>`, and it is the one probe variable a trace-only run
        // sets on its own (`WOW_MOVE_TRACE`/`WOW_STREAM_TRACE` legs, docs/METHOD.md) — 0794's
        // throttled camera leg was exactly such a run.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_PARK",
        purpose: "corner|edge|off — where the un-occludable probe window sits, and whether it is pinned on top",
        // A dial ON the occlusion defence itself, not a schedule.
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_RESIZE",
        purpose: "\"<secs>:<W>x<H>\" — resize the primary window mid-run (the headless fullscreen-toggle stand-in)",
        // Fires on `ProbeClock` at `<secs>`.
        wall_clock: true,
    },
    // ── The actuation channels: chat, keys, Lua, pointer ─────────────────────────────────────
    ProbeVar {
        name: "WOW_PROBE_CHAT",
        purpose: "\"<line>[;<line>…]\" — send each line as chat once in-world; the park-the-probe-anywhere instrument (`.go xyz …`)",
        // Line `n` is due at `at + every * n` on `ProbeClock`.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_CHAT_AT",
        purpose: "<secs> — when the first chat line goes out (default 8)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_CHAT_EVERY",
        purpose: "<secs> — space the chat lines apart instead of one burst (do X, wait, then do Y)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_KEY",
        purpose: "\"<key>@<secs>[:<hold>][;…]\" — synthesize key presses once in-world (a jump, a mount flourish, a held W)",
        // Each tap fires at its `@<secs>` and releases `<hold>` later, on `ProbeClock`.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_LUA",
        purpose: "\"<chunk>\" — run a Lua chunk in the live UI VM once per world entry (the press-the-button-headlessly instrument)",
        // First fire at `_AT` seconds, re-armed `_AGAIN` seconds into every later world entry.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_LUA_AT",
        purpose: "<secs> — when the chunk runs after the first world entry (default 10)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_LUA_AGAIN",
        purpose: "<secs> — how long after every LATER world entry (a relog) the chunk runs again (default 4)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_HOVER",
        purpose: "\"<frame>[;<frame>…]\" — sweep the real pointer across the named frames' centres, pressing nothing",
        // Starts at `_AT`, one move per `_STEP` seconds, alternating on `_DUTY` — all wall seconds.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_HOVER_AT",
        purpose: "<secs> — when the sweep starts (default 14)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_HOVER_STEP",
        purpose: "<secs> — seconds per pointer move (default 0.25; ~0.016 sweeps at frame rate, the director's gesture)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_HOVER_JITTER",
        purpose: "<px> — walk the pointer inside a px box each step so it moves while the hovered frame stays put",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_HOVER_DUTY",
        purpose: "<on>:<off> — sweep for `on` seconds, park over nothing for `off`, repeat (a leg that alternates can be read)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_DRAG",
        purpose: "\"<From>><To>[;…]\" — drag one named frame onto another through the real pointer path, one gesture step per frame",
        // Starts at `_AT`, advances one step per `_STEP` seconds, on `ProbeClock`.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_DRAG_AT",
        purpose: "<secs> — when the first drag starts (default 14)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_DRAG_STEP",
        purpose: "<secs> — seconds per gesture step (default 0.1); a press and release in one frame is a click, not a drag",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_DRAG_LUA",
        purpose: "\"<chunk>\" — a Lua chunk evaluated after each drag whose string result is logged as the report",
        wall_clock: false,
    },
    // ── The scripted controller inputs ────────────────────────────────
    ProbeVar {
        name: "WOW_PROBE_LOOK",
        purpose: "\"<deg_per_sec>@<start_s>:<duration_s>[;…]\" — the scripted mouse-turn: turn the avatar's aim at a rate for a while",
        // `90°/s for 6 s` has to produce 540° whatever the frame rate did; integrates rate × dt
        // between `at` and `until` on `ProbeClock`.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_PITCH",
        purpose: "\"<deg>@<start_s>[:<deg_per_sec>][;…]\" — the scripted dive: aim a swimming avatar's nose up or down (+up)",
        // A dive scripted to reach 30° by second 25 has to reach it at second 25, and it is the
        // one script that can be a run's only actuator (a drifting swimmer needs no keys).
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_CAM",
        purpose: "\"<yaw_deg>,<pitch_deg>[,<dist_yd>]@<start_s>[:<pan_deg_per_s>][;…]\" — park the third-person camera at absolute poses",
        // Each pose takes over at its `@<start>` and pans at its rate from then, on `ProbeClock`.
        wall_clock: true,
    },
    // ── The FPS probe's dials (`WOW_LIVE_FPS`, the capture harness) ─────────────────────────
    ProbeVar {
        name: "WOW_PROBE_UNCAP",
        purpose: "immediate|vsync — the FPS probe's present mode instead of the measured-best AutoNoVsync",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_VSYNC",
        purpose: "1 — keep vsync ON in the FPS probe: it then measures the present ceiling the display grants this window",
        wall_clock: false,
    },
    // ── The pricing levers and traces: change what a run draws or logs, never when ───────────
    ProbeVar {
        name: "WOW_PROBE_UI_ONE_TEX",
        purpose: "1 — pricing lever: split UI runs on state flags alone, ignoring texture identity (the draw-count ceiling an atlas would reach)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_SHARED_SKIN",
        purpose: "1 — pricing lever: share character-skin materials across bodies (see `char_skin::build_char_skin_materials`)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_NAME_TRACE",
        purpose: "1 — per-frame NAME_TRACE lines for the self player's plate seat (a smoothness question is measured, never eyeballed)",
        wall_clock: false,
    },
    // ── The `=1` live probes: each parks the body somewhere real and steps a phase machine ───
    // Every one below reads `time.elapsed_secs_f64()` into a `since`/`now` phase machine —
    // waits, settles and timeouts in wall seconds — so each arms the occlusion defence.
    ProbeVar {
        name: "WOW_PROBE_BG_SAMPLES",
        purpose: "n — how many 12 s census samples WOW_PROBE_BG takes inside the battleground (default 12; ~30 reaches vmangos's 5-minute premature finish, i.e. the end of a match)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_BG",
        purpose: "wsg|ab|av — queue for that battleground, take the port through the stock verb and census the instance from inside",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_BGQUEUE",
        purpose: "1 — level past the bracket floor, greet Stormwind's Warsong Gulch battlemaster and queue through his list",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_MAIL",
        purpose: "1 — GM-mail the probe's own character, open the Goldshire mailbox and drive inbox/take/send/delete through the VM",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_AUCTION",
        purpose: "1 — GM-hop to a Stormwind auctioneer and drive browse/throttle/sell/owner-list/cancel through the VM",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_BANK",
        purpose: "1 — GM-hop to a pure banker and drive the six-opcode bank wire (activate/deposit/withdraw/buy-slot/refusal)",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_BINDER",
        purpose: "1 — GM-hop to an innkeeper, select the bind row and answer the confirm through the VM's own ConfirmBinder()",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_SERVICE",
        purpose: "1 — right-click four real NPCs, one per UNIT_NPC_FLAGS shape, and report the service-ladder arm and the window that opened",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_VENDOR_SWAP",
        purpose: "1 — open one Goldshire vendor over the other's window and read the `npc` token, title and portrait at MERCHANT_SHOW",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_MODEL_CAMERA",
        purpose: "1 — build a plain <Model> pane on a camera-bearing file and read the renderer's camera and root back",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_GMTICKET",
        purpose: "1 — drive the five-opcode GM ticket wire through the VM (status, clean slate, file, edit, abandon)",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_CHARTER",
        purpose: "1 — buy a guild charter at the Stormwind registrar, open it with a real bag right-click and rename it",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_BOOK",
        purpose: "1 — teleport to the Old Town plaque and measure what the item-text reader costs per frame, closed vs open (B240)",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_STONE",
        purpose: "1 or <x>,<y>,<z>[,<map>] — join a real meeting stone on the click's own route and read the queue back out of the live VM",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_CHEST",
        purpose: "1 or <x>,<y>,<z>[,<map>] — open a real chest on the click's own route and report the self anim id before/during/after",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_GOQUEST",
        purpose: "1 or <x>,<y>,<z>[,<map>] — park at a GameObject questgiver and report the dialog status below and above MinLevel",
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_CLAM",
        purpose: "1 or <entry> — stock an openable item, right-click it through UseContainerItem and report whether a loot window opens",
        wall_clock: true,
    },
    // ── Modifiers of the value-dispatched probes ────────────────────────────────────────────
    ProbeVar {
        name: "WOW_PROBE_DOCK",
        purpose: "x,y,z[,map] — the dock `WOW_PROBE=crossing` boards from (map defaults to 0 and must be sent)",
        wall_clock: false,
    },
    // ── The character-select probe: advances on wire replies, not on a clock ───────────────
    ProbeVar {
        name: "WOW_PROBE_CHARCREATE",
        purpose: "\"<name>[,race,class,gender[,skin,face,hair,haircolor,facial]]\" — create (and delete) a character at select over the wire",
        // A roster/result state machine: `AwaitingRoster` → create → result byte → delete →
        // result byte, each step on the reply that arrives. No clock read anywhere in it.
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_CHARCREATE_KEEP",
        purpose: "1 — keep the character the probe created instead of deleting it",
        wall_clock: false,
    },
];

/// The named values of `WOW_PROBE` itself, `(value, purpose)` — the one place they are listed.
/// `dev.rs` dispatches on the literals (one plugin per value) and `lib.rs` answers `list`
/// before any window opens; the test below keeps both in step with this table.
pub(crate) const PROBE_NAMES: &[(&str, &str)] = &[
    ("list", "print this table and exit, before any window opens"),
    (
        "melee",
        "auto-fight the nearest enemy so the dbg-trace sink can record the combat-text timeline",
    ),
    (
        "partner",
        "the second client that says yes: auto-accept every group invite and duel challenge (0434/0637)",
    ),
    (
        "crossing",
        "board a cross-continent boat and report the map seam surviving (0455)",
    ),
    (
        "taxi",
        "open the flight-master menu on the wire and ride Stormwind → Sentinel Hill to a verdict (0484)",
    ),
    (
        "guardpoi",
        "ask a Stormwind guard for the weapons trainer and check the SMSG_GOSSIP_POI marker field by field",
    ),
    (
        "castcancel",
        "hearth and press W mid-cast — the local self-cancel's end-to-end timing instrument",
    ),
];

/// The variables whose presence arms the un-occludable probe window — the `wall_clock` column.
pub(crate) fn wall_clock_vars() -> impl Iterator<Item = &'static str> {
    PROBE_VARS.iter().filter(|v| v.wall_clock).map(|v| v.name)
}

/// `WOW_PROBE=list` — print the registry, one variable per line, then the named values.
pub(crate) fn print() {
    println!("The probe fleet's environment (capture::probe_env, decision 2265 §A5).");
    println!(
        "wall-clock: the variable schedules on elapsed real time and arms the un-occludable probe"
    );
    println!("window (ProbeFocusPlugin, decision 0906) — covered, such a run drops to ~1 fps and");
    println!("executes the wrong script.");
    println!();
    println!("{:<26} {:<10} PURPOSE", "VARIABLE", "WALL-CLOCK");
    for v in PROBE_VARS {
        let wc = if v.wall_clock { "yes" } else { "-" };
        println!("{:<26} {:<10} {}", v.name, wc, v.purpose);
    }
    println!();
    println!("WOW_PROBE=<name>:");
    for (name, purpose) in PROBE_NAMES {
        println!("  {name:<12} {purpose}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This file's own path under `src/` — excluded from both scans, since its table and its
    /// tests are the one place the literals are *supposed* to appear without being read.
    const SELF: &str = "capture/probe_env.rs";

    /// **The table cannot drift from the code, in either direction**.
    ///
    /// Every `"WOW_PROBE…"` string literal in the crate must be a row here — a variable the
    /// fleet reads that the registry does not know is exactly the hand-maintained gap this
    /// module replaced — and every row must be a literal somewhere other than this file, or the
    /// registry is describing a variable nothing reads. String literals only: doc comments
    /// write the backticked form and are skipped by line.
    #[test]
    fn every_probe_variable_is_registered_and_every_registered_variable_is_read() {
        let registered: Vec<&str> = PROBE_VARS.iter().map(|v| v.name).collect();
        for (i, name) in registered.iter().enumerate() {
            assert!(
                !registered[..i].contains(name),
                "`{name}` is registered twice"
            );
            assert!(
                name.starts_with("WOW_PROBE"),
                "`{name}` is not a probe variable — the registry is `WOW_PROBE*` only"
            );
        }

        let mut read: Vec<(String, String)> = Vec::new(); // (literal, file)
        for (rel, text) in crate_sources() {
            if rel == SELF {
                continue;
            }
            for lit in probe_literals(&text) {
                read.push((lit, rel.clone()));
            }
        }

        let unregistered: Vec<String> = read
            .iter()
            .filter(|(lit, _)| !registered.contains(&lit.as_str()))
            .map(|(lit, file)| format!("{file}: \"{lit}\""))
            .collect();
        assert!(
            unregistered.is_empty(),
            "these `WOW_PROBE…` literals are read but not in `PROBE_VARS` — add a row (name, \
             purpose from the read site's comment, and whether it schedules on the wall clock):\n  {}",
            unregistered.join("\n  ")
        );

        let unread: Vec<&str> = registered
            .iter()
            .copied()
            .filter(|name| !read.iter().any(|(lit, _)| lit == name))
            .collect();
        assert!(
            unread.is_empty(),
            "these `PROBE_VARS` rows are not a string literal anywhere else in the crate — \
             nothing reads them; drop the row or restore the read site:\n  {}",
            unread.join("\n  ")
        );
    }

    /// **`WOW_PROBE`'s named values are listed once.** Every `Ok("<name>")` the code compares
    /// `WOW_PROBE`'s value against (the `dev.rs` plugin dispatch, the `lib.rs` `list` exit) must
    /// be a `PROBE_NAMES` row, and every row must be dispatched on — so `WOW_PROBE=list` prints
    /// the values that exist, not the ones someone remembered.
    #[test]
    fn every_dispatched_probe_name_is_listed_and_every_listed_name_is_dispatched() {
        let listed: Vec<&str> = PROBE_NAMES.iter().map(|(n, _)| *n).collect();
        for (i, name) in listed.iter().enumerate() {
            assert!(!listed[..i].contains(name), "`{name}` is listed twice");
        }

        let mut dispatched: Vec<(String, String)> = Vec::new(); // (value, file)
        for (rel, text) in crate_sources() {
            if rel == SELF {
                continue;
            }
            for value in dispatched_probe_values(&text) {
                dispatched.push((value, rel.clone()));
            }
        }
        assert!(
            !dispatched.is_empty(),
            "no `WOW_PROBE` value dispatch found anywhere — the scanner has lost its needle"
        );

        let unlisted: Vec<String> = dispatched
            .iter()
            .filter(|(v, _)| !listed.contains(&v.as_str()))
            .map(|(v, file)| format!("{file}: WOW_PROBE={v}"))
            .collect();
        assert!(
            unlisted.is_empty(),
            "these `WOW_PROBE` values are dispatched on but not in `PROBE_NAMES`:\n  {}",
            unlisted.join("\n  ")
        );

        let undispatched: Vec<&str> = listed
            .iter()
            .copied()
            .filter(|name| !dispatched.iter().any(|(v, _)| v == name))
            .collect();
        assert!(
            undispatched.is_empty(),
            "these `PROBE_NAMES` rows are not dispatched on anywhere (`var(\"WOW_PROBE\")… == \
             Ok(\"<name>\")`):\n  {}",
            undispatched.join("\n  ")
        );
    }

    /// Every `.rs` under this crate's `src/`, as `(path under src/, text)`.
    fn crate_sources() -> Vec<(String, String)> {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("crate source dir is readable") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let rel = path
                        .strip_prefix(&src)
                        .expect("under src")
                        .to_string_lossy()
                        .replace('\\', "/");
                    let text = std::fs::read_to_string(&path).expect("source is readable");
                    out.push((rel, text));
                }
            }
        }
        out.sort();
        out
    }

    /// The source with every comment line (`//`, `///`, `//!`) dropped — the scans below want
    /// code, and a doc comment quoting a variable is not a read of it.
    fn code_lines(text: &str) -> String {
        text.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every `"WOW_PROBE[A-Z0-9_]*"` string literal in the code — the whole literal, quote to
    /// quote, so a variable named inside a longer message is not a match.
    fn probe_literals(text: &str) -> Vec<String> {
        let code = code_lines(text);
        let mut out = Vec::new();
        let mut rest = code.as_str();
        while let Some(i) = rest.find("\"WOW_PROBE") {
            let after = &rest[i + 1..];
            let end = after
                .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
                .unwrap_or(after.len());
            if after[end..].starts_with('"') {
                out.push(after[..end].to_string());
            }
            rest = &rest[i + 1..];
        }
        out
    }

    /// Every value the code compares `WOW_PROBE` against: `var("WOW_PROBE")` followed, within the
    /// same expression, by `Ok("<value>")`.
    fn dispatched_probe_values(text: &str) -> Vec<String> {
        let code = code_lines(text);
        let needle = "var(\"WOW_PROBE\")";
        let mut out = Vec::new();
        let mut rest = code.as_str();
        while let Some(i) = rest.find(needle) {
            let after = &rest[i + needle.len()..];
            // The comparison sits right after the read: `.as_deref() == Ok("…")`. Bound the
            // look-ahead so a bare `is_ok()` gate is not paired with some later `Ok("…")`.
            let window = &after[..after.len().min(48)];
            if let Some(j) = window.find("Ok(\"") {
                let value = &after[j + 4..];
                if let Some(k) = value.find('"') {
                    out.push(value[..k].to_string());
                }
            }
            rest = after;
        }
        out
    }
}
