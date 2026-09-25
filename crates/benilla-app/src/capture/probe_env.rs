//! The probe environment registry: every `WOW_PROBE*` variable the app reads, with whether it
//! schedules on the wall clock ([`ProbeVar::wall_clock`], which arms [`super::ProbeFocusPlugin`]).
//! The tests keep it in step with the code both ways; `WOW_PROBE=list` prints it. The variables
//! keep their own read sites: this is a registry, not a dispatcher.

/// One probe-fleet environment variable.
pub(crate) struct ProbeVar {
    /// The variable, exactly as the read site spells it.
    pub name: &'static str,
    /// One line: the value shape and what setting it does.
    pub purpose: &'static str,
    /// Whether the variable schedules on elapsed real time, so it is only right at full frame
    /// rate: macOS drops a covered window to ~1 fps, and such a run fires its script out of
    /// order. A `true` row arms [`super::ProbeFocusPlugin`] from `dev.rs`, keeping the window
    /// un-occludable; a modifier rides its parent's arming.
    pub wall_clock: bool,
}

/// Every `WOW_PROBE*` variable the app reads, grouped by owner, in `WOW_PROBE=list` order.
pub(crate) const PROBE_VARS: &[ProbeVar] = &[
    // ── The variable itself, and the run shell every scripted probe rides ────────────────────
    ProbeVar {
        name: "WOW_PROBE",
        purpose: "<name> — a value-dispatched live probe (see the names below); `list` prints this table",
        // Every value but `list` and `partner` steps a phase machine on the wall clock.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_EXIT_AT",
        purpose: "<secs> — exit the app after N wall seconds; bounds any scripted live probe's lifetime",
        // Fires on `ProbeClock`; a trace-only run sets it alone.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_PARK",
        purpose: "corner|edge|off — where the un-occludable probe window sits, and whether it is pinned on top",
        // A dial on the occlusion defence itself, not a schedule.
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
        // Starts at `_AT`, one move per `_STEP` seconds, alternating on `_DUTY`, in wall seconds.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_HOVER_AT",
        purpose: "<secs> — when the sweep starts (default 14)",
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_HOVER_STEP",
        purpose: "<secs> — seconds per pointer move (default 0.25; ~0.016 sweeps at frame rate, like a real hand)",
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
        // Integrates rate times dt between `at` and `until` on `ProbeClock`.
        wall_clock: true,
    },
    ProbeVar {
        name: "WOW_PROBE_PITCH",
        purpose: "\"<deg>@<start_s>[:<deg_per_sec>][;…]\" — the scripted dive: aim a swimming avatar's nose up or down (+up)",
        // Each dive target is due at its wall-clock second.
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
    // ── The pricing levers and traces: what a run draws or logs, never when ─────────────────
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
    // Their waits, settles and timeouts are wall seconds, so each arms the occlusion defence.
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
        purpose: "1 — teleport to the Old Town plaque and measure what the item-text reader costs per frame, closed vs open",
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
        // Each step advances on a wire reply; no clock is read.
        wall_clock: false,
    },
    ProbeVar {
        name: "WOW_PROBE_CHARCREATE_KEEP",
        purpose: "1 — keep the character the probe created instead of deleting it",
        wall_clock: false,
    },
];

/// The named values of `WOW_PROBE`, `(value, purpose)`; `dev.rs` dispatches on them and `lib.rs`
/// answers `list`.
pub(crate) const PROBE_NAMES: &[(&str, &str)] = &[
    ("list", "print this table and exit, before any window opens"),
    (
        "melee",
        "auto-fight the nearest enemy so the dbg-trace sink can record the combat-text timeline",
    ),
    (
        "partner",
        "the second client that says yes: auto-accept every group invite and duel challenge",
    ),
    (
        "crossing",
        "board a cross-continent boat and report the map seam surviving",
    ),
    (
        "taxi",
        "open the flight-master menu on the wire and ride Stormwind → Sentinel Hill to a verdict",
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

/// The variables whose presence arms the un-occludable probe window.
pub(crate) fn wall_clock_vars() -> impl Iterator<Item = &'static str> {
    PROBE_VARS.iter().filter(|v| v.wall_clock).map(|v| v.name)
}

/// `WOW_PROBE=list`: prints the registry, one variable per line, then the named values.
pub(crate) fn print() {
    println!("The probe fleet's environment (capture::probe_env).");
    println!(
        "wall-clock: the variable schedules on elapsed real time and arms the un-occludable probe"
    );
    println!("window (ProbeFocusPlugin) — covered, such a run drops to ~1 fps and");
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

    /// This file's path under `src/`, excluded from both scans.
    const SELF: &str = "capture/probe_env.rs";

    /// Every `"WOW_PROBE..."` string literal in the crate is a row, and every row is a literal
    /// outside this file; comment lines are skipped.
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

    /// Every `Ok("<name>")` compared against `WOW_PROBE` is a `PROBE_NAMES` row, and every row
    /// is dispatched on.
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

    /// The source with every comment line dropped: a comment quoting a variable is not a read.
    fn code_lines(text: &str) -> String {
        text.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every whole `"WOW_PROBE[A-Z0-9_]*"` string literal in the code, quote to quote.
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
            // Bounded look-ahead, so a bare `is_ok()` gate is not paired with a later `Ok("...")`.
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
