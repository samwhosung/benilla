//! The layout cache: the windows the player has moved or resized, the userPlaced bit
//! (`frame+0xb4 & 0x1000`), restored at the next login. The engine half (snapshot, restore, dirty
//! bit) is `benilla_ui::script`'s `layout_cache`; this is the file and when it is written.
//!
//! The reference writes `WTF/Account/<ACC>/<REALM>/<CHAR>/layout-cache.txt`; ours is
//! `benilla-config/layout/<realm>-<character>.txt`, in its form: per frame a `Frame:` line, then
//! `FrameLevel:`, `X:`, `Y:`, `W:` and `H:` (`0x490e60`), each `Name: value` and an integer, the
//! records back to back. A file of benilla's earlier anchor form, one `Point:` line per anchor, is
//! read once and rewritten in this form at the next save.
//!
//! The reference writes the file only in its UI shutdown tail (`0x490bd0`), at `0x490c79`,
//! after `PLAYER_LOGOUT` (`0x490c2a`) and before the flat saved file (`0x490c7e`), on logout,
//! quit, disconnect, exit and `/reload`; [`save_now`] is that step. Deviation: a debounced
//! autosave ([`save_layout`]) also writes a quiet second after a drag, because the reference has
//! no crash-path write and a crash would lose the session's windows.

use std::path::PathBuf;

use bevy::prelude::*;

use benilla_ui::script::{AnchorLayout, FrameLayout, LayoutPoint, UiScript};

use crate::ui_script::VmMemo;

/// How long a moved window sits before the autosave: one gesture coalesced, at most one lost.
const SAVE_QUIET: std::time::Duration = std::time::Duration::from_secs(1);

/// The screen root as a target in the anchor form, where `GetPoint` answers `nil`.
const SCREEN_TOKEN: &str = "-";

/// The file's header, a `#` line the parser skips.
const HEADER: &str = "\
# benilla window layout: every frame the player has moved or resized, as layout-cache.txt has it.
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

/// Render the cache as [`parse`] reads it, frames in the order the engine hands them; a field
/// the row does not carry has no line, as the writer's flag gates leave it (`0x490f99`,
/// `0x49109f`).
pub(crate) fn render(frames: &[FrameLayout]) -> String {
    let mut out = String::from(HEADER);
    for f in frames {
        out.push_str(&format!("Frame: {}\n", f.name));
        let fields = [
            ("FrameLevel", f.level.map(i32::from)),
            ("X", f.x),
            ("Y", f.y),
            ("W", f.width),
            ("H", f.height),
        ];
        for (key, value) in fields {
            if let Some(v) = value {
                out.push_str(&format!("{key}: {v}\n"));
            }
        }
    }
    out
}

/// What a layout file holds.
#[derive(Debug, PartialEq)]
pub(crate) enum Layout {
    /// The reference's rect form.
    Rect(Vec<FrameLayout>),
    /// benilla's earlier anchor form, which carries `Point:` lines.
    Anchors(Vec<AnchorLayout>),
}

/// Parse the cache: keys match case-insensitively, and an unknown key costs only its own line. A
/// value reads as `atoi` reads it (`0x64ac60`). The value lines before any `Frame:`, or after one
/// with no name, belong to no frame and are dropped, as the reference applies them to a null frame
/// (`0x4905e6`); a name no frame has drops its lines at the restore the same way.
///
/// Deviation: a record's fields start empty. The reference's parser clears X, Y, W and the level
/// at each `Frame:` (`ebx`, `edi`, `esi`, `[ebp-0x4]` at `0x49045b`–`0x490469`) but not H
/// (`[ebp-0xc]`), so a record with no `H:` takes the one before's; benilla writes `H:` whenever it
/// writes `W:`, so only a hand-edited file reaches the slip, which would size a window from
/// another window's row.
pub(crate) fn parse(text: &str) -> Layout {
    if lines(text).any(|(key, _)| key.eq_ignore_ascii_case("Point")) {
        return Layout::Anchors(parse_anchor_form(text));
    }
    let mut out: Vec<FrameLayout> = Vec::new();
    let mut current = false;
    for (key, rest) in lines(text) {
        if key.eq_ignore_ascii_case("Frame") {
            current = !rest.is_empty();
            if current {
                out.push(FrameLayout {
                    name: rest.to_owned(),
                    ..FrameLayout::default()
                });
            }
            continue;
        }
        let Some(frame) = out.last_mut().filter(|_| current) else {
            continue; // no frame owns the line
        };
        let v = atoi(rest);
        if key.eq_ignore_ascii_case("FrameLevel") {
            // Below zero is the parser's "none" (`0x490462`), which the apply skips (`0x4905f0`).
            frame.level = (v >= 0).then(|| u16::try_from(v).unwrap_or(u16::MAX));
        } else if key.eq_ignore_ascii_case("X") {
            frame.x = Some(v);
        } else if key.eq_ignore_ascii_case("Y") {
            frame.y = Some(v);
        } else if key.eq_ignore_ascii_case("W") {
            frame.width = Some(v);
        } else if key.eq_ignore_ascii_case("H") {
            frame.height = Some(v);
        }
    }
    Layout::Rect(out)
}

/// The file's `Key: value` lines, trimmed, with blanks and `#` lines skipped.
fn lines(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (key, rest) = line.split_once(':')?;
        Some((key.trim(), rest.trim()))
    })
}

/// C's `atoi`: an optional sign and the leading digits, zero when there are none.
fn atoi(s: &str) -> i32 {
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |n, d| {
            (n * 10 + i64::from(d - b'0')).min(i64::from(i32::MAX) + 1)
        });
    let n = if neg { -n } else { n };
    n.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// The anchor form: `W:`, `H:` and one `Point: <point> <relativeTo> <relativePoint> <x> <y>` line
/// per anchor, `-` for the screen root. A malformed `Point:` costs only its own line.
fn parse_anchor_form(text: &str) -> Vec<AnchorLayout> {
    let mut out: Vec<AnchorLayout> = Vec::new();
    let mut current = false;
    for (key, rest) in lines(text) {
        if key.eq_ignore_ascii_case("Frame") {
            current = !rest.is_empty();
            if current {
                out.push(AnchorLayout {
                    name: rest.to_owned(),
                    width: 0.0,
                    height: 0.0,
                    points: Vec::new(),
                });
            }
            continue;
        }
        let Some(frame) = out.last_mut().filter(|_| current) else {
            continue;
        };
        if key.eq_ignore_ascii_case("W") {
            frame.width = rest.parse().unwrap_or(0.0);
        } else if key.eq_ignore_ascii_case("H") {
            frame.height = rest.parse().unwrap_or(0.0);
        } else if key.eq_ignore_ascii_case("Point") {
            let f: Vec<&str> = rest.split_whitespace().collect();
            if f.len() != 5 {
                warn!("layout: malformed Point line ignored: {key}: {rest}");
                continue;
            }
            let (Ok(x), Ok(y)) = (f[3].parse::<f32>(), f[4].parse::<f32>()) else {
                warn!("layout: Point line with unparsable offsets ignored: {key}: {rest}");
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
    let (count, anchor_form) = match parse(&text) {
        Layout::Rect(frames) => {
            let n = frames.len();
            script.restore_user_placed_layouts(frames);
            (n, false)
        }
        Layout::Anchors(frames) => {
            let n = frames.len();
            script.restore_anchor_layouts(frames);
            (n, true)
        }
    };
    if count == 0 {
        return;
    }
    info!("layout: {count} frames restored from {}", path.display());
    // Drain the dirty bit the restore set, so the first save does not rewrite what was just read.
    script.take_user_placed_change();
    if anchor_form {
        // The anchor form is read once: the autosave rewrites it in the rect form once the
        // restored windows have settled.
        *file.dirty.get(&script) = true;
        file.last_change = Some(std::time::Instant::now());
    }
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

    fn rows(layout: Layout) -> Vec<FrameLayout> {
        match layout {
            Layout::Rect(rows) => rows,
            Layout::Anchors(rows) => panic!("read as the anchor form: {rows:?}"),
        }
    }

    #[test]
    fn the_file_round_trips() {
        let frames = vec![
            FrameLayout {
                name: "ChatFrame1".into(),
                level: Some(0),
                x: Some(38),
                y: Some(-225),
                width: Some(431),
                height: Some(400),
            },
            // Movable only: no `W:`/`H:`, as the reference writes `KLHTM_Frame`.
            FrameLayout {
                name: "Floater".into(),
                level: Some(4),
                x: Some(-2),
                y: Some(0),
                width: None,
                height: None,
            },
        ];
        let text = render(&frames);
        assert!(
            text.ends_with(
                "Frame: ChatFrame1\nFrameLevel: 0\nX: 38\nY: -225\nW: 431\nH: 400\n\
                 Frame: Floater\nFrameLevel: 4\nX: -2\nY: 0\n"
            ),
            "the reference's lines, in its order:\n{text}"
        );
        assert_eq!(rows(parse(&text)), frames);
    }

    #[test]
    fn the_header_is_skipped_not_parsed() {
        assert!(render(&[]).starts_with('#'));
        assert_eq!(parse(HEADER), Layout::Rect(vec![]));
    }

    #[test]
    fn junk_costs_only_its_own_line() {
        let got = rows(parse(
            "W: 100\n\
             Frame: A\n\
             Shade: 4\n\
             FrameLevel: -1\n\
             x: 12.7\n\
             Y: -3px\n\
             W: wide\n\
             H: 200\n\
             Frame:\n\
             X: 99\n\
             H: 7\n",
        ));
        assert_eq!(
            got,
            vec![FrameLayout {
                name: "A".into(),
                level: None,
                x: Some(12),
                y: Some(-3),
                width: Some(0),
                height: Some(200),
            }]
        );
    }

    #[test]
    fn a_point_line_marks_the_anchor_form() {
        let text = "\
# benilla window layout — every frame the player has moved or resized (the client's userPlaced
Frame: ChatFrame1
W: 512
H: 180.5
Point: BOTTOMLEFT UIParent BOTTOMLEFT 40 120
Frame: Floater
W: 0
H: 0
Point: TOPLEFT - TOPLEFT -1.5 2
Point: TOPLEFT -
";
        let point = |p: &str, rel: Option<&str>, rp: &str, x: f32, y: f32| LayoutPoint {
            point: p.into(),
            relative_to: rel.map(str::to_owned),
            relative_point: rp.into(),
            x,
            y,
        };
        assert_eq!(
            parse(text),
            Layout::Anchors(vec![
                AnchorLayout {
                    name: "ChatFrame1".into(),
                    width: 512.0,
                    height: 180.5,
                    points: vec![point(
                        "BOTTOMLEFT",
                        Some("UIParent"),
                        "BOTTOMLEFT",
                        40.0,
                        120.0
                    )],
                },
                AnchorLayout {
                    name: "Floater".into(),
                    width: 0.0,
                    height: 0.0,
                    points: vec![point("TOPLEFT", None, "TOPLEFT", -1.5, 2.0)],
                },
            ])
        );
    }

    /// A file of the anchor form seats the windows where they were and saves back in the rect
    /// form, so a player's windows survive the change of format.
    #[test]
    fn an_anchor_form_file_migrates_to_the_rect_form() {
        let mut s = UiScript::new().unwrap();
        s.set_screen_size(1024.0, 768.0);
        s.run(
            r#"
            local b = CreateFrame("Frame", "Base")
            b:SetPoint("BOTTOMLEFT", 0, 0) b:SetWidth(1024) b:SetHeight(768)
            local f = CreateFrame("Frame", "Moved")
            f:SetPoint("CENTER", 0, 0) f:SetWidth(100) f:SetHeight(50)
            f:SetMovable(true) f:SetResizable(true)
            local g = CreateFrame("Frame", "Glider")
            g:SetPoint("CENTER", 0, 0) g:SetWidth(30) g:SetHeight(30)
            g:SetMovable(true)
            "#,
        )
        .unwrap();
        let old = "\
Frame: Glider
W: 30
H: 30
Point: TOPLEFT - TOPLEFT 10.4 -20.6
Frame: Moved
W: 213.6
H: 80
Point: BOTTOMLEFT Base BOTTOMLEFT 61 29
";
        let Layout::Anchors(rows) = parse(old) else {
            panic!("an old file reads as the anchor form");
        };
        s.restore_anchor_layouts(rows);
        let text = render(&s.user_placed_layouts());
        assert!(
            text.contains(
                "Frame: Moved\nFrameLevel: 0\nX: 61\nY: -659\nW: 214\nH: 80\n\
                 Frame: Glider\nFrameLevel: 0\nX: 10\nY: -21\n"
            ),
            "the same windows, where they were, in the rect form, in creation order:\n{text}"
        );
        assert!(!text.contains("Point:"), "{text}");
        assert!(s.errors().is_empty(), "{:?}", s.errors());
    }
}
