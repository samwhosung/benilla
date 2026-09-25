//! The layout cache: the windows the player has moved or resized, the userPlaced bit
//! (`frame+0xb4 & 0x1000`), restored at the next login. The engine half (snapshot, restore, dirty
//! bit) is `benilla_ui::script`'s `layout_cache`; this is the file and when it is written.
//!
//! The reference writes `WTF/Account/<ACC>/<REALM>/<CHAR>/layout-cache.txt` with `Frame:`,
//! `FrameLevel:`, `X:`, `Y:`, `W:` and `H:` lines; ours is
//! `benilla-config/layout/<realm>-<character>.txt`, keeping `Frame:`, `W:` and `H:`.
//! Deviation: one `Point: <point> <relativeTo> <relativePoint> <x> <y>` line per anchor, `-` for
//! the screen root, instead of `X:`/`Y:`, because a benilla drag moves the authored anchors
//! rather than re-anchoring to one TOPLEFT, and a pair would lose them. There is no
//! `FrameLevel:`: the reference saves an undocked window's raise, and benilla's raise does not
//! persist.
//!
//! The reference writes the file only in its UI shutdown tail (`0x490bd0`), at `0x490c79`,
//! after `PLAYER_LOGOUT` (`0x490c2a`) and before the flat saved file (`0x490c7e`), on logout,
//! quit, disconnect, exit and `/reload`; [`save_now`] is that step. Deviation: a debounced
//! autosave ([`save_layout`]) also writes a quiet second after a drag, because the reference has
//! no crash-path write and a crash would lose the session's windows.

use std::path::PathBuf;

use bevy::prelude::*;

use benilla_ui::script::{FrameLayout, LayoutPoint, UiScript};

use crate::ui_script::VmMemo;

/// How long a moved window sits before the autosave: one gesture coalesced, at most one lost.
const SAVE_QUIET: std::time::Duration = std::time::Duration::from_secs(1);

/// The screen root in the file, where `GetPoint` answers `nil`; not `UIParent`, a real frame.
const SCREEN_TOKEN: &str = "-";

/// The file's header, a `#` block the parser skips.
const HEADER: &str = "\
# benilla window layout — every frame the player has moved or resized (the client's userPlaced
# bit). A relative of the reference's layout-cache.txt: same scope, same Frame:/W:/H: keys, but
# anchors instead of its X:/Y: pair, because a benilla drag moves a frame's anchors rather than
# collapsing it to a screen position. `-` as a Point: target means the screen root.
";

/// The character's file: its path, the VM it was restored into, and whether a write is owed.
#[derive(Resource, Default)]
pub(crate) struct LayoutFile {
    path: Option<PathBuf>,
    /// The `(realm, character)` of [`Self::path`], keyed on the VM: a fresh VM needs a restore
    /// even for the same character.
    identity: VmMemo<Option<(String, String)>>,
    /// Whether this VM has unsaved drags. Keyed on the VM, as a flag that outlived its VM would
    /// write a fresh tree, with nothing placed, over the player's file.
    dirty: VmMemo<bool>,
    last_change: Option<std::time::Instant>,
}

/// Render the cache as [`parse`] reads it, frames name-sorted as the engine hands them.
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

/// Parse the cache: keys match case-insensitively, and an unknown key, a malformed `Point:` or a
/// value line before any `Frame:` costs only its own line.
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

/// Seat the saved geometry into the VM, once per character per VM. It runs after the UI tree is
/// built and the load-time `UIParent_ManageFramePositions()` pass has run, which skips
/// user-placed frames on every later pass (`UIParent.lua:1692`).
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
        return; // already restored into the live VM
    }
    file.path = crate::local_state::layout_character_path(&id.0, &id.1);
    *file.identity.get(&script) = Some(id);
    // A fresh VM has nothing placed and owes no write.
    *file.dirty.get(&script) = false;
    file.last_change = None;

    let Some(path) = file.path.clone() else {
        return; // hermetic capture or no state folder: session-only
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
    // Drain the dirty bit the restore set, so the first save does not rewrite what was just read.
    script.take_user_placed_change();
}

/// Drain the engine's user-placed-frame-moved bit into the dirty flag.
fn watch_layout(script: Option<NonSendMut<UiScript>>, mut file: ResMut<LayoutFile>) {
    let Some(mut script) = script else { return };
    if !script.take_user_placed_change() {
        return;
    }
    *file.dirty.get(&script) = true;
    file.last_change = Some(std::time::Instant::now());
}

/// The autosave: dirty and one quiet second rewrite the file atomically. Every orderly end is
/// [`save_now`]'s.
fn save_layout(script: Option<NonSendMut<UiScript>>, mut file: ResMut<LayoutFile>) {
    let Some(script) = script else { return };
    if !*file.dirty.get(&script) {
        return;
    }
    if !file.last_change.is_none_or(|t| t.elapsed() >= SAVE_QUIET) {
        return;
    }
    let Some(path) = file.path.clone() else {
        // Hermetic or session-only: nothing to write, stop retrying.
        *file.dirty.get(&script) = false;
        return;
    };
    let body = render(&script.user_placed_layouts());
    if let Err(e) = crate::local_state::write_atomic(&path, &body) {
        // The flag clears below anyway, so a failing write is not retried every frame.
        warn!("layout: cannot write {}: {e}", path.display());
    }
    *file.dirty.get(&script) = false;
}

/// The reference's step of the UI shutdown (`0x490c79`, after `PLAYER_LOGOUT`, before the flat
/// saved file): write the layout from the live VM now. Called from
/// [`crate::ui_script::shutdown_ui_state`], it runs on every end including `/reload`, which never
/// leaves `InWorld`; an `OnExit(InWorld)` saver would miss a reload and race the VM's teardown.
///
/// It takes the identity the UI loaded under, which the tail holds because the roster's pick can
/// be gone by shutdown, and writes unconditionally: the file is composed whole from the live tree,
/// and the tail does not run for a session whose UI never loaded.
pub(crate) fn save_now(script: &UiScript, identity: Option<&(String, String)>) {
    let Some((realm, character)) = identity else {
        return; // no character loaded under: a capture, a scenario, a test world
    };
    let Some(path) = crate::local_state::layout_character_path(realm, character) else {
        return; // hermetic capture or no state folder: session-only
    };
    let body = render(&script.user_placed_layouts());
    if let Err(e) = crate::local_state::write_atomic(&path, &body) {
        warn!("layout: cannot write {}: {e}", path.display());
    }
}

/// The layout cache's restore, watch and autosave; the plugin owns no session edge, as every
/// orderly write is [`save_now`]'s.
pub(crate) struct UiLayoutPlugin;

impl Plugin for UiLayoutPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LayoutFile>().add_systems(
            Update,
            (
                // In the feed phase, so the tick sees the restore the frame it lands.
                load_layout.in_set(crate::ui_script::UiFeed),
                // After the drag pump in the tick; load precedes watch through the phases, so the
                // watcher never reads the restore's own move.
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

    #[test]
    fn the_header_is_skipped_not_parsed() {
        assert!(render(&[]).starts_with('#'));
        assert_eq!(parse(HEADER), vec![]);
    }

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
