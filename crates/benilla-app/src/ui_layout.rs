//! The **layout cache** — where the windows the player has moved or resized are, so they are still
//! there after a relog.
//!
//! The engine has carried the client's userPlaced bit for a long time
//! ([`benilla_ui::script`]'s `movable` cluster — `frame+0xb4 & 0x1000`, set by `SetUserPlaced` and
//! by the drag entries themselves) and **nothing read it**: a chat window dragged across the screen
//! went back to its authored anchor at the next login. This module is the half that was missing.
//! The engine half — enumerate, restore, dirty-bit — is `benilla_ui::script`'s `layout_cache`; this
//! is the two ends the VM cannot own: *where the file lives* and *when it is written*.
//!
//! ## The file, and how it relates to the reference's
//!
//! 1.12 writes `WTF/Account/<ACC>/<REALM>/<CHAR>/layout-cache.txt`, and a real one off the pinned
//! install reads, in full:
//!
//! ```text
//! Frame: ChatFrame2
//! FrameLevel: 4
//! X: 32
//! Y: -578
//! W: 430
//! H: 120
//! ```
//!
//! Ours is `benilla-config/layout/<realm>-<character>.txt`
//! ([`crate::local_state::layout_character_path`]) — the same scope, one folder flatter, with that
//! file's `Frame:`/`W:`/`H:` spellings kept. **Two of its keys are deliberately different, and both
//! are about benilla's own move model rather than about taste:**
//!
//! - **`Point:` lines instead of `X:`/`Y:`.** The reference collapses a dragged frame to a single
//!   screen-space position and re-seats it from a `TOPLEFT` anchor; benilla's drag pump keeps the
//!   frame's authored anchor SET and shifts every offset in it (the divergence `movable`'s own doc
//!   states). Writing an absolute pair would therefore *lose* what the drag produced — ChatFrame1
//!   is anchored `BOTTOMLEFT` to UIParent, and restoring it as a TOPLEFT position would change how
//!   it follows a resolution change. So the file carries what `GetPoint` answers, one line per
//!   anchor: `Point: <point> <relativeTo> <relativePoint> <x> <y>`, with `-` for the screen root
//!   (which is where `GetPoint` answers `nil`).
//! - **No `FrameLevel:`.** The reference persists it because an undocked chat window is raised
//!   above its neighbours and that raise has to survive; benilla's raise is transient and its
//!   docked windows share one level. It joins the file the day a window can be raised for good —
//!   the honest-tree rule (1134 §4), the same one the chat-look file applies next door.
//!
//! ## The write posture — the reference's shutdown tail, plus a debounce it has not got
//!
//! **The reference writes this file in exactly one place**: step three of the UI shutdown's
//! ordered tail `0x490bd0` — `PLAYER_LEAVING_WORLD` → `PLAYER_LOGOUT` (`0x490c2a`) →
//! **`layout-cache.txt` (`0x490c79`)** → the flat saved file (`0x490c7e`) → the per-addon files →
//! `AddOns.txt` → the frame teardown. It rides the same five lifecycle roots as SavedVariables —
//! logout to the character screen, quit, disconnect, application exit and **`/reload`** — and
//! there is no autosave, no dirty bit and no per-frame write.
//!
//! Ours takes that slot: [`save_now`], called from [`crate::ui_script::shutdown_ui_state`]
//! between `PLAYER_LOGOUT` and the flat file. **That is the load-bearing part, and it is what
//! B353 was.** The tail is ONE function called from every root, which is the only way a
//! `/reload` gets a write at all: a reload never leaves `InWorld` — `run_pending_reload` calls
//! the shutdown and the rebuild back to back — so a saver hung off `OnExit(InWorld)` is invisible
//! to it, and an unlocked chat window came back on its authored anchors after every `/reload`
//! with `benilla-config/layout/` never created. The same edge is *also* not an ordering: two
//! unconstrained systems on one state edge are placed by the executor, and measured here (bevy
//! 0.18, five systems on one `OnExit`) the placement moves with nothing but the registration
//! positions — a saver that needs the dying VM ran *after* the exclusive session-ender in one
//! arrangement and before it in another. So this module's `OnExit` saver was reading a VM it was
//! racing for. The tail cannot race: it runs inside `end_ui_session`, ahead of the replacement,
//! by construction. That is the mistake [`crate::ui_script::shutdown_ui_state`]'s own doc says
//! the tail exists to prevent, made once more one module over.
//!
//! **The one thing we keep that the reference has not got is a debounced autosave** —
//! [`save_layout`], dirty flag keyed to the VM ([`VmMemo`]), one quiet second. The reference has
//! no crash-path write either (its exception filter reaches no writer) and we would rather not
//! lose a session's windows to one; a drag is a slider-shaped gesture, so the quiet second
//! coalesces a resize's per-mouse-move writes into one. It is a *second* writer of the same
//! file, never the only one: every orderly end goes through the tail.
//!
//! The VM key on the dirty flag is one-way and load-bearing: the geometry lives in the VM, so a
//! plain `bool` surviving a VM replacement would let a save compose the player's file out of a
//! fresh tree that has no user-placed frame in it at all — i.e. wipe it. A fresh VM starts
//! undirty and cannot write until a drag writes.

use std::path::PathBuf;

use bevy::prelude::*;

use benilla_ui::script::{FrameLayout, LayoutPoint, UiScript};

use crate::ui_script::VmMemo;

/// How long a moved window sits before the save fires — [`crate::cvars`]'s own constant and its own
/// reasoning, which a drag fits exactly: long enough to coalesce one gesture, short enough that a
/// crash costs a gesture rather than a session.
const SAVE_QUIET: std::time::Duration = std::time::Duration::from_secs(1);

/// The screen root's spelling in the file — where `GetPoint` answers `nil`. A literal `-` rather
/// than `UIParent`, because benilla has a real frame by that name (`UIParent.xml`) and the two
/// would then be indistinguishable on the way back in.
const SCREEN_TOKEN: &str = "-";

/// The file's header — what these lines are and where the law lives.
const HEADER: &str = "\
# benilla window layout — every frame the player has moved or resized (the client's userPlaced
# bit). A relative of the reference's layout-cache.txt: same scope, same Frame:/W:/H: keys, but
# anchors instead of its X:/Y: pair, because a benilla drag moves a frame's anchors rather than
# collapsing it to a screen position. `-` as a Point: target means the screen root.
";

/// Which character's file we are on, where it lives, whether it has been restored into this VM,
/// and whether it is owed a write.
#[derive(Resource, Default)]
pub(crate) struct LayoutFile {
    path: Option<PathBuf>,
    /// The `(realm, character)` [`Self::path`] was built for. Session-keyed (1290) like the chat
    /// look and the macro loads: the *same* character coming back still meets a fresh VM whose
    /// frames are all back on their authored anchors.
    identity: VmMemo<Option<(String, String)>>,
    /// Whether **this VM** has unsaved drags. Session-keyed for the one-way reason in the module
    /// doc: a `bool` that outlived its VM could compose the file from a tree with nothing placed
    /// in it.
    dirty: VmMemo<bool>,
    last_change: Option<std::time::Instant>,
}

/// Render the cache exactly as [`parse`] reads it, frames in the order the engine hands them
/// (name-sorted, so the file is stable across sessions).
///
/// `pub(crate)` for one reason: `ui_script::chat_resize_tests` drives the *whole* round trip — drag
/// the shipped window, snapshot it, write the text, read it back into a fresh VM — and a test that
/// stops at the engine seam would not have caught a file that cannot express what the seam
/// produced.
pub(crate) fn render(frames: &[FrameLayout]) -> String {
    let mut out = String::from(HEADER);
    for f in frames {
        out.push_str(&format!("Frame: {}\n", f.name));
        out.push_str(&format!("W: {}\n", f.width));
        out.push_str(&format!("H: {}\n", f.height));
        for p in &f.points {
            out.push_str(&format!(
                "Point: {} {} {} {} {}\n",
                p.point,
                p.relative_to.as_deref().unwrap_or(SCREEN_TOKEN),
                p.relative_point,
                p.x,
                p.y
            ));
        }
    }
    out
}

/// Parse the cache back. Permissive the way the reference's own readers are, and for the same
/// reason — a hand edit and a later build's extra key must each cost at most the line they are on:
/// keys match case-insensitively, an unknown key is skipped, and a malformed `Point:` drops that
/// anchor rather than the file. A `Point:`/`W:`/`H:` before any `Frame:` has no owner and is
/// dropped.
pub(crate) fn parse(text: &str) -> Vec<FrameLayout> {
    let mut out: Vec<FrameLayout> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let (key, rest) = (key.trim(), rest.trim());
        if key.eq_ignore_ascii_case("Frame") {
            if rest.is_empty() {
                continue;
            }
            out.push(FrameLayout {
                name: rest.to_owned(),
                width: 0.0,
                height: 0.0,
                points: Vec::new(),
            });
            continue;
        }
        let Some(frame) = out.last_mut() else {
            continue; // a value line with no `Frame:` above it owns nothing
        };
        if key.eq_ignore_ascii_case("W") {
            frame.width = rest.parse().unwrap_or(0.0);
        } else if key.eq_ignore_ascii_case("H") {
            frame.height = rest.parse().unwrap_or(0.0);
        } else if key.eq_ignore_ascii_case("Point") {
            let f: Vec<&str> = rest.split_whitespace().collect();
            if f.len() != 5 {
                warn!("layout: malformed Point line ignored: {line}");
                continue;
            }
            let (Ok(x), Ok(y)) = (f[3].parse::<f32>(), f[4].parse::<f32>()) else {
                warn!("layout: Point line with unparsable offsets ignored: {line}");
                continue;
            };
            frame.points.push(LayoutPoint {
                point: f[0].to_owned(),
                relative_to: (f[1] != SCREEN_TOKEN).then(|| f[1].to_owned()),
                relative_point: f[2].to_owned(),
                x,
                y,
            });
        }
    }
    out
}

/// Seat the player's saved geometry into the VM, once per character per VM.
///
/// `pub(crate)` so `ui_script::world_entry_tests` can run it against a real post-reload VM:
/// the file half of the loop is only worth anything if a window actually comes BACK, and
/// that is the half a test of `render`/`parse` alone cannot see.
///
/// Runs in `Update` under `InWorld`, which is *after* the UI tree is built and after decision
/// 0272's load-time `UIParent_ManageFramePositions()` bootstrap — both of which matter: the frames
/// have to exist to be looked up by name, and the managed pass has to have had its say first,
/// because from here on it skips these frames (`IsUserPlaced`, `UIParent.xml`).
pub(crate) fn load_layout(
    script: Option<NonSendMut<UiScript>>,
    roster: Res<crate::char_select::Roster>,
    mut file: ResMut<LayoutFile>,
) {
    let Some(mut script) = script else { return };
    let Some(id) = crate::ui_macro::identity(&roster) else {
        return;
    };
    if file.identity.get(&script).as_ref() == Some(&id) {
        return; // already restored for this character, into the VM that is live now
    }
    file.path = crate::local_state::layout_character_path(&id.0, &id.1);
    *file.identity.get(&script) = Some(id);
    // A fresh VM has nothing placed, so a character with no file needs nothing pushed — and the
    // watcher below must not read a change this load made.
    *file.dirty.get(&script) = false;
    file.last_change = None;

    let Some(path) = file.path.clone() else {
        return; // hermetic capture, or no state folder — session-only
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!("layout: cannot read {}: {e}", path.display());
            return;
        }
    };
    let frames = parse(&text);
    if frames.is_empty() {
        return;
    }
    info!(
        "layout: {} frames restored from {}",
        frames.len(),
        path.display()
    );
    script.restore_user_placed_layouts(frames);
    // The restore moved anchors the drag pump would otherwise have moved; drain the engine's own
    // dirty bit so the very first save is not a rewrite of what was just read.
    script.take_user_placed_change();
}

/// Drain the engine's "a user-placed frame moved" bit into the dirty flag. Cheap on a steady frame
/// — the drain is a `take` of a `bool`.
fn watch_layout(script: Option<NonSendMut<UiScript>>, mut file: ResMut<LayoutFile>) {
    let Some(mut script) = script else { return };
    if !script.take_user_placed_change() {
        return;
    }
    *file.dirty.get(&script) = true;
    file.last_change = Some(std::time::Instant::now());
}

/// Dirty + one quiet second → rewrite the file atomically. **The crash writer, and only that**
/// (see the module doc): every orderly end — logout, disconnect, quit, `/reload` — is the
/// shutdown tail's, so this never has to reason about an exit message or an edge.
fn save_layout(script: Option<NonSendMut<UiScript>>, mut file: ResMut<LayoutFile>) {
    let Some(script) = script else { return };
    if !*file.dirty.get(&script) {
        return;
    }
    if !file.last_change.is_none_or(|t| t.elapsed() >= SAVE_QUIET) {
        return;
    }
    let Some(path) = file.path.clone() else {
        // hermetic/session-only: nothing to write, stop retrying
        *file.dirty.get(&script) = false;
        return;
    };
    let body = render(&script.user_placed_layouts());
    if let Err(e) = crate::local_state::write_atomic(&path, &body) {
        // …and don't retry every frame into the same error.
        warn!("layout: cannot write {}: {e}", path.display());
    }
    *file.dirty.get(&script) = false;
}

/// **The reference's own step of the UI shutdown** (`0x490c79`, between `PLAYER_LOGOUT` and the
/// flat saved file): write the player's layout cache from the live VM, now. Called from
/// [`crate::ui_script::shutdown_ui_state`], so it reaches every root the tail does — logout,
/// disconnect, quit and `/reload` — which is what an `OnExit(InWorld)` system could not (module
/// doc, and B353).
///
/// It takes the **identity the tail is holding** rather than reading [`LayoutFile`], for the
/// reason [`crate::ui_script::AddOnIdentity`] exists at all: that is the character the UI
/// actually loaded under, remembered because the roster's pick can be gone by the time the
/// shutdown runs. It is the same `(realm, character)` [`load_layout`] built its path from —
/// both are [`crate::ui_macro::identity`] of the same roster — so the tail writes back to the
/// file the load read.
///
/// Unconditional, like every other resident of the tail: the file is composed whole from the
/// live tree, so a session that placed nothing writes back what it restored, and a session that
/// un-placed a window writes the row's absence. The one guard is the tail's own — a session
/// whose in-game UI never loaded does not run it, so a UI-less VM cannot answer with its
/// emptiness.
pub(crate) fn save_now(script: &UiScript, identity: Option<&(String, String)>) {
    let Some((realm, character)) = identity else {
        return; // no character loaded under — a capture, a scenario, a test world
    };
    let Some(path) = crate::local_state::layout_character_path(realm, character) else {
        return; // hermetic capture, or no state folder — session-only
    };
    let body = render(&script.user_placed_layouts());
    if let Err(e) = crate::local_state::write_atomic(&path, &body) {
        warn!("layout: cannot write {}: {e}", path.display());
    }
}

/// The layout cache's plugin. **Not the chat look's shape any more** (2029): that module keeps
/// its own session edge because its reference is the type-7 chat-cache saver `0x499a80`;
/// this file's reference is `0x490bd0`'s ordered tail, so its every orderly write lives in
/// [`save_now`] and the plugin has no edge left to own.
pub(crate) struct UiLayoutPlugin;

impl Plugin for UiLayoutPlugin {
    fn build(&self, app: &mut App) {
        // **One chain, and no session edges of its own.** Restore, watch, and the crash-only
        // debounce; every orderly end is [`crate::ui_script::shutdown_ui_state`]'s [`save_now`],
        // which is the reference's own single write site and the only one a `/reload` reaches.
        app.init_resource::<LayoutFile>().add_systems(
            Update,
            (
                // The restore is a push the tick should see the frame it lands: the feed phase.
                load_layout.in_set(crate::ui_script::UiFeed),
                // The watcher and the save read what the drag pump — Lua's, in the tick — did:
                // after it. Load precedes watch through the phases, as the old chain had it,
                // so the watcher never reads the restore's own move.
                (watch_layout, save_layout)
                    .chain()
                    .after(crate::ui_script::UiInput),
            )
                .in_set(crate::char_select::InWorldGated),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(p: &str, rel: Option<&str>, rp: &str, x: f32, y: f32) -> LayoutPoint {
        LayoutPoint {
            point: p.into(),
            relative_to: rel.map(str::to_owned),
            relative_point: rp.into(),
            x,
            y,
        }
    }

    /// The file round-trips — what `render` writes is exactly what `parse` reads back, the screen
    /// root's `-` included.
    #[test]
    fn the_file_round_trips() {
        let frames = vec![
            FrameLayout {
                name: "ChatFrame1".into(),
                width: 512.0,
                height: 180.5,
                points: vec![point(
                    "BOTTOMLEFT",
                    Some("UIParent"),
                    "BOTTOMLEFT",
                    40.0,
                    120.0,
                )],
            },
            FrameLayout {
                name: "Floater".into(),
                width: 0.0,
                height: 0.0,
                points: vec![
                    point("TOPLEFT", None, "TOPLEFT", -1.5, 2.0),
                    point("BOTTOMRIGHT", Some("ChatFrame1"), "TOPRIGHT", 0.0, 0.0),
                ],
            },
        ];
        assert_eq!(parse(&render(&frames)), frames);
    }

    /// The header is a comment block and survives the round trip as one — a reader that choked on
    /// its own header would lose the player's windows on the second launch.
    #[test]
    fn the_header_is_skipped_not_parsed() {
        assert!(render(&[]).starts_with('#'));
        assert_eq!(parse(HEADER), vec![]);
    }

    /// Junk costs the line it is on: an orphan value line, a short `Point:`, an unknown key.
    #[test]
    fn junk_costs_only_its_own_line() {
        let got = parse(
            "W: 100\n\
             Frame: A\n\
             FrameLevel: 4\n\
             Point: TOPLEFT -\n\
             W: 200\n\
             point: bottomleft - BOTTOMLEFT 1 2\n",
        );
        assert_eq!(
            got,
            vec![FrameLayout {
                name: "A".into(),
                width: 200.0,
                height: 0.0,
                points: vec![point("bottomleft", None, "BOTTOMLEFT", 1.0, 2.0)],
            }]
        );
    }
}
