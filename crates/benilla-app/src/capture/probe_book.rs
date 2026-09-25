//! The world-book live probe (`WOW_PROBE_BOOK=1`): the per-frame cost of the item-text reader
//! open on a real plaque. It hops to the plaque, samples the UI pass's meter
//! ([`crate::ui_script::UiFrameCost`]) with the reader closed, opens it, samples again, checks the
//! page's render, and logs both windows under `PROBE_BOOK:`, with how many frames the extract
//! gate skipped.
//!
//! The object is Stormwind Old Town's Alliance Military Ranks plaque (`gameobject_template` 2857,
//! `GAMEOBJECT_TYPE_TEXT`), whose `data[0]` is `page_text` 2676, a 647-byte HTML body. The
//! switches are `docs/CONTRIBUTING.md`, "Running it unattended".

use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_ui::script::UiScript;

use super::probes::ProbeClock;
use crate::net::{ChatKind, ClientCommand, Guid, NetCommands, NetEntity, ObjectStore, SelfPlayer};
use crate::player::Player;
use crate::ui_item_text::ItemTextOpen;
use crate::ui_script::{UiCostWanted, UiFrameCost};

/// A standing position beside the plaque.
const PLAQUE_AT: [f32; 3] = [-8760.2, 402.3, 103.9];
/// `GAMEOBJECT_TYPE_TEXT`, a book or plaque.
const GO_TYPE_TEXT: i32 = 9;
/// Scan radius around the landing spot, in yards.
const SCAN_RANGE: f32 = 20.0;

/// Frames sampled per window, enough that one hitch cannot carry the mean.
const SAMPLE_FRAMES: usize = 240;
const SETTLE_SECS: f64 = 4.0;
const SCAN_TIMEOUT_SECS: f64 = 20.0;
const READY_TIMEOUT_SECS: f64 = 15.0;

pub(crate) struct ProbeBookPlugin;

impl Plugin for ProbeBookPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BookProbe>()
            .add_systems(Update, book_probe);
    }
}

/// One frame's reading off the UI meter.
#[derive(Clone, Copy)]
struct Sample {
    /// The paint pass: glyph rasterization and the ellipsis seam.
    convert: u128,
    /// The whole UI pass, skipped or not.
    total: u128,
    /// Whether the extract gate skipped the conversion this frame.
    skipped: bool,
}

#[derive(Resource, Default)]
struct BookProbe {
    phase: Phase,
    plaque: Option<u64>,
    closed: Vec<Sample>,
    open: Vec<Sample>,
    /// Characters the whole UI draws once the reader is up.
    page_chars: i64,
    fails: u32,
    exited: bool,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum Phase {
    #[default]
    Wait,
    /// `.go` issued; letting the world stream the plaque in.
    Settling {
        sent_at: f64,
    },
    /// Sampling with the reader closed, the control window.
    Closed,
    /// Reader opened; waiting for `ITEM_TEXT_READY` to paint a body.
    WaitReady {
        since: f64,
    },
    /// Sampling with the reader open.
    Open,
    Done,
}

/// The reader's painted state: `(shown, characters drawn)`, read off the render list because the
/// 1.12 `SimpleHTML` has no `GetText`.
fn reader_state(script: &UiScript) -> (bool, i64) {
    use benilla_ui::script::QuadContent;
    let shown = script
        .eval::<bool>("return ItemTextScrollFrame:IsShown() and 1 or nil")
        .unwrap_or(false);
    let chars = script
        .extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Text { text, .. } => text,
            _ => None,
        })
        .map(|t| t.chars().count() as i64)
        .sum();
    (shown, chars)
}

/// Every string the UI draws; the only way to read the page, as `SimpleHTML` has no `GetText`
/// (method table `0x87ba80`).
fn drawn_strings(script: &UiScript) -> Vec<String> {
    use benilla_ui::script::QuadContent;
    script
        .extract()
        .into_iter()
        .filter_map(|q| match q.content {
            QuadContent::Text { text, .. } => text,
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// Checks the page's render: no drawn string holds a tag (a failed parse draws the markup), each
/// of the page's lines is its own block, none is ellipsized, and the page fits its window.
fn report_render(script: &UiScript) -> u32 {
    let drawn = drawn_strings(script);
    let mut fails = 0;
    let markup: Vec<&String> = drawn
        .iter()
        .filter(|t| t.contains("<HTML>") || t.contains("<P align") || t.contains("</P>"))
        .collect();
    if markup.is_empty() {
        info!("PROBE_BOOK: (render) PASS — nothing on screen draws its own markup");
    } else {
        fails += 1;
        error!(
            "PROBE_BOOK: (render) FAIL — the page is drawing markup: {:?}",
            markup.iter().take(2).collect::<Vec<_>>()
        );
    }
    // Lines of the plaque's body, each its own centred block.
    let want = [
        "ALLIANCE MILITARY RANKS",
        "OFFICERS",
        "Grand Marshal",
        "Private",
    ];
    let missing: Vec<&str> = want
        .iter()
        .copied()
        .filter(|w| !drawn.iter().any(|t| t == w))
        .collect();
    if missing.is_empty() {
        info!("PROBE_BOOK: (render) PASS — the page's blocks are on screen, one per tag");
    } else {
        fails += 1;
        error!("PROBE_BOOK: (render) FAIL — blocks missing from the page: {missing:?}");
    }
    // A truncated block means the body is height-pinned and the ellipsis seam cuts it.
    if let Some(cut) = drawn.iter().find(|t| t.ends_with("...")) {
        fails += 1;
        error!("PROBE_BOOK: (render) FAIL — a drawn block is ellipsized: {cut:?}");
    } else {
        info!("PROBE_BOOK: (render) PASS — no block is cut off with \"...\"");
    }
    // The reference shows this page whole, so its scroll range (the overhang past the viewport)
    // must be zero.
    let range = script
        .eval::<f64>("return ItemTextScrollFrame:GetVerticalScrollRange()")
        .unwrap_or(-1.0);
    if range == 0.0 {
        info!("PROBE_BOOK: (render) PASS — the page fits the window (scroll range 0)");
    } else {
        fails += 1;
        error!(
            "PROBE_BOOK: (render) FAIL — the page overhangs its window by {range} units; the \
             reference shows this one whole"
        );
    }
    fails
}

/// Mean, median and max of a sample column, in µs.
fn stats(v: &[u128]) -> (f64, u128, u128) {
    if v.is_empty() {
        return (0.0, 0, 0);
    }
    let mut sorted = v.to_vec();
    sorted.sort_unstable();
    #[allow(clippy::cast_precision_loss)]
    let mean = v.iter().sum::<u128>() as f64 / v.len() as f64;
    (mean, sorted[sorted.len() / 2], *sorted.last().unwrap())
}

/// One window's line: the paint phase, the whole pass, and how often the gate skipped.
fn report(label: &str, s: &[Sample]) {
    let (cm, cmed, cmax) = stats(&s.iter().map(|x| x.convert).collect::<Vec<_>>());
    let (tm, tmed, tmax) = stats(&s.iter().map(|x| x.total).collect::<Vec<_>>());
    let skipped = s.iter().filter(|x| x.skipped).count();
    info!(
        "PROBE_BOOK: {label:<7} n={:<4} convert mean={cm:.0}us med={cmed}us max={cmax}us | \
         ui-pass mean={tm:.0}us med={tmed}us max={tmax}us | gate skipped {skipped}/{}",
        s.len(),
        s.len(),
    );
}

fn book_probe(
    time: ProbeClock,
    mut probe: ResMut<BookProbe>,
    mut wanted: ResMut<UiCostWanted>,
    cost: Res<UiFrameCost>,
    mut reader: ResMut<ItemTextOpen>,
    script: Option<NonSendMut<UiScript>>,
    self_player: Query<(), With<SelfPlayer>>,
    player: Res<Player>,
    objects: Query<(&Guid, &NetEntity, &ObjectStore, &Transform), Without<SelfPlayer>>,
    net: Res<NetCommands>,
    mut exit: MessageWriter<AppExit>,
) {
    if self_player.is_empty() {
        return; // not in-world yet
    }
    let Some(script) = script else {
        return; // no UI VM in this build
    };
    let now = time.elapsed_secs_f64();
    let sample = Sample {
        convert: cost.convert,
        total: cost.tick + cost.resolve + cost.measure + cost.extract + cost.convert + cost.diff,
        skipped: cost.skipped,
    };
    let phase = probe.phase;

    match phase {
        Phase::Wait => {
            // Arm the pass's per-phase split for this run.
            wanted.0 = true;
            let [x, y, z] = PLAQUE_AT;
            info!("PROBE_BOOK: heading to the Old Town plaque ({x} {y} {z}) — GameObject type 9, page_text 2676");
            let _ = net.0.send(ClientCommand::Chat {
                kind: ChatKind::Say,
                target: None,
                text: format!(".go xyz {x} {y} {z} 0"),
            });
            probe.phase = Phase::Settling { sent_at: now };
        }
        Phase::Settling { sent_at } => {
            if now - sent_at < SETTLE_SECS {
                return;
            }
            let me = player.pos;
            let plaque = objects.iter().find(|(_, net_e, store, tf)| {
                net_e.kind == EntityKind::GameObject
                    && store.0.gameobject_type_id() == GO_TYPE_TEXT
                    && tf.translation.distance(me) < SCAN_RANGE
            });
            if let Some((guid, ..)) = plaque {
                info!(
                    "PROBE_BOOK: plaque {:#x} in range — sampling {SAMPLE_FRAMES} frames with the reader CLOSED",
                    guid.0
                );
                probe.plaque = Some(guid.0);
                probe.phase = Phase::Closed;
            } else if now - sent_at > SCAN_TIMEOUT_SECS {
                error!("PROBE_BOOK: FAIL — no type-9 GameObject streamed in within {SCAN_TIMEOUT_SECS}s");
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Closed => {
            probe.closed.push(sample);
            if probe.closed.len() >= SAMPLE_FRAMES {
                let guid = probe.plaque.unwrap_or_default();
                info!("PROBE_BOOK: opening the reader on {guid:#x} (the right-click's own route — `ItemTextOpen::open_pages`, decision 1105)");
                reader.open_pages(guid);
                probe.phase = Phase::WaitReady { since: now };
            }
        }
        Phase::WaitReady { since } => {
            let (shown, chars) = reader_state(&script);
            if shown && chars > 0 {
                probe.page_chars = chars;
                probe.fails += report_render(&script);
                info!("PROBE_BOOK: reader painted a {chars}-char body — sampling {SAMPLE_FRAMES} frames with it OPEN");
                probe.phase = Phase::Open;
            } else if now - since > READY_TIMEOUT_SECS {
                error!("PROBE_BOOK: FAIL — reader never painted a page within {READY_TIMEOUT_SECS}s (shown={shown} chars={chars})");
                probe.fails += 1;
                probe.phase = Phase::Done;
            }
        }
        Phase::Open => {
            probe.open.push(sample);
            if probe.open.len() >= SAMPLE_FRAMES {
                probe.phase = Phase::Done;
            }
        }
        Phase::Done => {
            if probe.exited {
                return;
            }
            probe.exited = true;
            report("CLOSED", &probe.closed);
            report("OPEN", &probe.open);
            let (closed_mean, ..) =
                stats(&probe.closed.iter().map(|s| s.convert).collect::<Vec<_>>());
            let (open_mean, ..) = stats(&probe.open.iter().map(|s| s.convert).collect::<Vec<_>>());
            info!(
                "PROBE_BOOK: DONE page={} chars, reader costs {:+.0}us/frame in the paint pass fail={}",
                probe.page_chars,
                open_mean - closed_mean,
                probe.fails
            );
            // `AppExit` plus a hard backstop, so a teardown hang cannot keep the account held.
            exit.write(AppExit::Success);
            std::thread::spawn(|| {
                std::thread::sleep(std::time::Duration::from_secs(5));
                warn!("PROBE_BOOK: still alive 5s after AppExit — hard exit");
                std::process::exit(0);
            });
        }
    }
}
