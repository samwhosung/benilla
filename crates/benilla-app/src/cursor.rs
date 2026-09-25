//! The mouse cursor: the `Interface\Cursor\*.blp` set as a hardware cursor, as the reference
//! shows it. The mode comes from the world classifier (`0x4828d0`) and FrameXML; a held payload's
//! icon replaces it, composited to 32×32 (`0x523840`) and uploaded the same way (`0x523790`).
//!
//! On macOS winit's cursor-rect route reverts to the arrow on every mouse move under a
//! continuously redrawing Metal view, so this builds an `NSCursor` per mode, disables the window's
//! cursor rects and sets the cursor directly; mouselook uses `NSCursor::hide`/`unhide`. Elsewhere
//! winit's `CursorIcon::Custom` works and hide-on-look is `CursorOptions.visible`.

use benilla_assets::AssetSet;
use bevy::prelude::*;

/// The held cursor payload's extensionless icon path. `covered` drops it: the reference's world
/// transition (`0x6e4940`) calls `0x523d20(1)` and `0x523c20(1)` before the loading screen goes up,
/// the plain arrow with no overlay.
fn payload_icon(script: &benilla_ui::script::UiScript, covered: bool) -> Option<String> {
    use benilla_ui::script::CursorPayload;
    if covered {
        return None;
    }
    match script.cursor_payload()? {
        CursorPayload::Item(i) => i.texture,
        CursorPayload::Spell(s) => s.texture,
        CursorPayload::Action(a) => a.texture,
        CursorPayload::Macro(m) => m.texture,
        CursorPayload::PetAction(p) => p.texture,
        // Mode 10: the stabled pet's family icon; a non-empty path gates the grab.
        CursorPayload::StablePet(p) => Some(p.texture),
        // Mode 2: the coin bitmap by magnitude, `GetCoinIcon`'s table.
        CursorPayload::Money(m) => {
            Some(benilla_ui::script::coin_icon(i64::from(m.copper)).to_string())
        }
        // Mode 5: the vendor row's icon, which the reference resolves from its `ItemDisplayInfo`
        // id (`0xb4d8ec`).
        CursorPayload::Merchant(m) => m.texture,
    }
}

/// Box-downsample top-to-bottom RGBA8 to 32×32, each texel averaging a `(w/32)×(h/32)` block.
fn box_downsample_32(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    const OUT: u32 = 32;
    let mut out = vec![0u8; (OUT * OUT * 4) as usize];
    if w == 0 || h == 0 {
        return out;
    }
    for oy in 0..OUT {
        let y0 = oy * h / OUT;
        let y1 = ((oy + 1) * h / OUT).max(y0 + 1).min(h);
        for ox in 0..OUT {
            let x0 = ox * w / OUT;
            let x1 = ((ox + 1) * w / OUT).max(x0 + 1).min(w);
            let mut sum = [0u32; 4];
            let mut n = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * w + x) * 4) as usize;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += u32::from(rgba[i + c]);
                    }
                    n += 1;
                }
            }
            let o = ((oy * OUT + ox) * 4) as usize;
            for (c, byte) in out[o..o + 4].iter_mut().enumerate() {
                *byte = (sum[c] / n) as u8;
            }
        }
    }
    out
}

/// Decode a payload icon to an opaque 32×32 RGBA8 cursor, as the client's drag bitmap does
/// (`0x523840`: a 64×64 icon, 2×2 box filter, alpha 0xFF).
fn decode_payload_cursor_rgba(
    assets: &mut benilla_assets::WorldAssets,
    path: &str,
) -> Option<Vec<u8>> {
    let (w, h, rgba) = assets.decode_rgba(path)?;
    let mut out = box_downsample_32(w, h, &rgba);
    for a in out.iter_mut().skip(3).step_by(4) {
        *a = 0xFF;
    }
    Some(out)
}

/// Every stem [`crate::target::WorldCursor::stem`] can name, preloaded at startup.
const CURSOR_STEMS: &[&str] = &[
    "Point",
    "Attack",
    "UnableAttack",
    "Speak",
    "UnableSpeak",
    "Pickup",
    "UnablePickup",
    // The loot pouch: auto-loot XOR shift.
    "LootAll",
    "UnableLootAll",
    "Interact",
    "UnableInteract",
    "Buy",
    "UnableBuy",
    "Inspect", // the Ctrl-hover magnifier (ShowInspectCursor) and the text GameObject cursor
    "UnableInspect",
    "Trainer",
    "UnableTrainer",
    "Taxi",
    "UnableTaxi",
    "Skin",
    "UnableSkin",
    "Repair", // the repair-mode base, never grayed: the shipped UnableRepair is unreachable
    // The data-driven GameObject cursors (`0x5f8760`); PickLock is never grayed.
    "Mail",
    "UnableMail",
    "Mine",
    "UnableMine",
    "GatherHerbs",
    "UnableGatherHerbs",
    "PickLock",
    // The spell-targeting pair (`0x4820f0`): Cast(2) and UnableCast(22).
    "Cast",
    "UnableCast",
];

/// A cursor stem's archive path.
fn cursor_path(stem: &str) -> String {
    format!("Interface\\Cursor\\{stem}.blp")
}

/// The displayed cursor, the client's one sticky mode cell `0xbe2c2c`;
/// [`crate::target::WorldCursor`] is one of its two writers.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct DisplayedCursor(pub(crate) crate::target::WorldCursor);

/// Resolve this frame's displayed cursor: one sticky mode with two writers, the world (the
/// WorldFrame's hover handler `0x481790`, only while it holds mouse focus) and FrameXML
/// (`SetCursor`, `ResetCursor`, `ShowContainerSellCursor`, `ShowInspectCursor`). Between writes
/// the last value stands, so a frame that calls no cursor function changes nothing.
fn drive_displayed_cursor(
    world: Res<crate::target::WorldCursor>,
    over_ui: Res<crate::ui_script::PointerOverUi>,
    targeting: Res<crate::spell::targeting::SpellTargeting>,
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    // The base last restored on entering the UI; `None` over the world, so re-entry restores.
    mut last: Local<crate::ui_script::VmMemo<Option<crate::target::WorldCursor>>>,
    mut displayed: ResMut<DisplayedCursor>,
    screen: Res<crate::loading_screen::LoadingScreen>,
) {
    use crate::target::{CursorKind, WorldCursor};
    use benilla_ui::script::UiCursorMode;

    // This runs with no VM too (the character screen); `get_for` treats that as its own session,
    // so the restore re-arms once on each side of the glue phase.
    let last = last.get_for(script.as_deref());

    // Under the loading cover the cursor is the plain arrow, ahead of every other arm: the
    // reference parks index 1 unconditionally before raising the screen (`0x6e49f5`).
    if screen.covering() {
        displayed.0 = WorldCursor::default();
        *last = None;
        return;
    }

    // The base cell (`0xbe2c4c`) is mutable and `ResetCursor` restores its value, not Point.
    // While a spell awaits its click it is Cast(2), so the UI reads blue whatever the hovered item:
    // 1.12 has no hover-time validity verdict. `ShowContainerSellCursor` (`0x4fa460`) bails unless
    // the base is Point.
    let repair = script.as_ref().is_some_and(|s| s.repair_mode());
    let base = if repair {
        // `ShowRepairCursor` (`0x4fbcc0`) parks the base at Repair while it holds.
        WorldCursor {
            kind: CursorKind::Repair,
            unable: false,
        }
    } else if targeting.active()
        || script
            .as_ref()
            .is_some_and(|s| s.gift_wrap_armed().is_some())
    {
        // An armed gift wrap sets both cells to Cast(2) (`0x5edea0`: `SetCursorBaseMode(2)`,
        // `CursorSetMode(2)`), so its cursor survives the pointer crossing the world.
        WorldCursor {
            kind: CursorKind::Cast,
            unable: false,
        }
    } else {
        WorldCursor::default()
    };

    if let Some(mut script) = script {
        if let Some(write) = script.take_cursor_write() {
            displayed.0 = match write {
                Some(UiCursorMode::Buy) => WorldCursor {
                    kind: CursorKind::Buy,
                    unable: false,
                },
                Some(UiCursorMode::UnableBuy) => WorldCursor {
                    kind: CursorKind::Buy,
                    unable: true,
                },
                Some(UiCursorMode::Inspect) => WorldCursor {
                    kind: CursorKind::Inspect,
                    unable: false,
                },
                Some(UiCursorMode::Cast) => WorldCursor {
                    kind: CursorKind::Cast,
                    unable: false,
                },
                Some(UiCursorMode::CastError) => WorldCursor {
                    kind: CursorKind::Cast,
                    unable: true,
                },
                Some(UiCursorMode::Point) => WorldCursor::default(),
                // `ResetCursor`: back to the base mode, Repair included.
                None => base,
            };
            *last = Some(base);
            return;
        }
    }
    if !over_ui.0 {
        displayed.0 = *world;
        *last = None;
        return;
    }
    // Over UI with no FrameXML write: crossing in, or a base change, restores the base once. This
    // one edge stands for the `ResetCursor()` that ends nearly every reference hover handler and
    // the classifier's no-hover restore (`0x523d30`).
    if *last != Some(base) {
        *last = Some(base);
        displayed.0 = base;
    }
}

pub(crate) struct CursorPlugin;

impl Plugin for CursorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DisplayedCursor>();
        #[cfg(target_os = "macos")]
        app.add_systems(Startup, macos::setup.after(AssetSet::Open))
            .add_systems(
                Update,
                (drive_displayed_cursor, macos::drive)
                    .chain()
                    // After the tick: a FrameXML `SetCursor` made this frame is read here.
                    .after(crate::ui_script::UiInput),
            );
        #[cfg(not(target_os = "macos"))]
        app.init_resource::<other::PayloadCursorImages>()
            .add_systems(Startup, other::setup.after(AssetSet::Open))
            .add_systems(
                Update,
                (drive_displayed_cursor, other::drive)
                    .chain()
                    // After the tick: a FrameXML `SetCursor` made this frame is read here.
                    .after(crate::ui_script::UiInput),
            );
    }
}

/// Non-macOS: winit's custom cursor, swapped on change; hide-on-look is in `player::control`.
#[cfg(not(target_os = "macos"))]
mod other {
    use super::{cursor_path, payload_icon, CURSOR_STEMS};
    use benilla_assets::{cursor_texture, WorldAssets};
    use bevy::platform::collections::{HashMap, HashSet};
    use bevy::prelude::*;
    use bevy::window::{CursorIcon, CustomCursor, CustomCursorImage, PrimaryWindow};

    /// Stem → decoded cursor image, preloaded at startup.
    #[derive(Resource, Default)]
    pub(super) struct CursorImages(HashMap<String, Handle<Image>>);

    /// Held-payload icon path → its 32×32 cursor, built on first use and cached.
    #[derive(Resource, Default)]
    pub(super) struct PayloadCursorImages(HashMap<String, Handle<Image>>);

    pub(super) fn setup(
        mut commands: Commands,
        world_assets: Option<ResMut<WorldAssets>>,
        mut images: ResMut<Assets<Image>>,
    ) {
        let Some(mut world_assets) = world_assets else {
            return;
        };
        let mut map = HashMap::default();
        for stem in CURSOR_STEMS {
            match world_assets.decode_cursor(&cursor_path(stem), &mut images) {
                Some(handle) => {
                    map.insert(stem.to_string(), handle);
                }
                None => warn!("cursor {stem}.blp missing/undecodable — mode falls back"),
            }
        }
        commands.insert_resource(CursorImages(map));
    }

    /// Show the held payload's icon when it resolves, else the displayed mode (then its base stem,
    /// then Point).
    pub(super) fn drive(
        mut commands: Commands,
        cursor: Res<super::DisplayedCursor>,
        cursors: Option<Res<CursorImages>>,
        script: Option<NonSend<benilla_ui::script::UiScript>>,
        world_assets: Option<ResMut<WorldAssets>>,
        mut images: ResMut<Assets<Image>>,
        mut payload_cursors: ResMut<PayloadCursorImages>,
        screen: Res<crate::loading_screen::LoadingScreen>,
        window: Option<Single<Entity, With<PrimaryWindow>>>,
        // The key of the cursor last handed the window: OS state, so not a
        // [`crate::ui_script::VmMemo`].
        mut last_set: Local<Option<String>>,
        mut decode_failed: Local<HashSet<String>>,
    ) {
        let Some(window) = window else {
            return;
        };
        let held_icon = script
            .as_ref()
            .and_then(|s| payload_icon(s, screen.covering()));
        if let Some(icon) = held_icon {
            if last_set.as_deref() == Some(icon.as_str()) {
                return; // already showing this icon
            }
            if !payload_cursors.0.contains_key(&icon) && !decode_failed.contains(&icon) {
                let built = world_assets.and_then(|mut a| {
                    let rgba = super::decode_payload_cursor_rgba(&mut a, &icon)?;
                    Some(images.add(cursor_texture(32, 32, rgba)))
                });
                match built {
                    Some(handle) => {
                        payload_cursors.0.insert(icon.clone(), handle);
                    }
                    None => {
                        warn!("cursor payload icon {icon} missing/undecodable — mode cursor kept");
                        decode_failed.insert(icon.clone());
                    }
                }
            }
            if let Some(handle) = payload_cursors.0.get(&icon) {
                set_custom_cursor(&mut commands, *window, handle.clone());
                *last_set = Some(icon);
                return;
            }
            // Decode failed: fall through to the mode cursor.
        }

        let Some(cursors) = cursors else {
            return;
        };
        let stem = cursor.0.stem();
        if last_set.as_deref() == Some(stem.as_str()) {
            return;
        }
        let handle = cursors
            .0
            .get(&stem)
            .or_else(|| cursors.0.get(stem.trim_start_matches("Unable")))
            .or_else(|| cursors.0.get("Point"));
        if let Some(handle) = handle {
            set_custom_cursor(&mut commands, *window, handle.clone());
        }
        *last_set = Some(stem);
    }

    /// Insert a custom cursor with hotspot `(0, 0)`, the vanilla cursors' top-left tip.
    fn set_custom_cursor(commands: &mut Commands, window: Entity, handle: Handle<Image>) {
        commands
            .entity(window)
            .insert(CursorIcon::Custom(CustomCursor::Image(CustomCursorImage {
                handle,
                texture_atlas: None,
                flip_x: false,
                flip_y: false,
                rect: None,
                hotspot: (0, 0),
            })));
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::collections::{HashMap, HashSet};

    use crate::player::CameraControl;
    use benilla_assets::LockRecover;
    use benilla_assets::WorldAssets;
    use benilla_formats::read_texture_rgba;
    use bevy::prelude::*;
    use objc2::rc::Retained;
    use objc2::ClassType; // brings `alloc()` into scope
    use objc2_app_kit::{NSApplication, NSCursor, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData, NSPoint};

    use super::{cursor_path, payload_icon, CURSOR_STEMS};

    /// The built `NSCursor` per mode stem; non-send, as AppKit types are main-thread only.
    pub(super) struct NativeCursors(HashMap<&'static str, Retained<NSCursor>>);

    /// Held-payload icon path → its 32×32 `NSCursor`, built on first use and cached.
    pub(super) struct PayloadCursors(HashMap<String, Retained<NSCursor>>);

    /// Build every mode's `NSCursor`; exclusive, so it runs on the main thread.
    pub(super) fn setup(world: &mut World) {
        // Ahead of the early returns: `drive` reads it every frame.
        world.insert_non_send_resource(PayloadCursors(HashMap::new()));
        // No client data: one warning, not one per stem.
        if world.get_resource::<WorldAssets>().is_none() {
            warn!("no client data — the OS cursor stands in for every mode cursor");
            return;
        }
        let mut cursors = HashMap::new();
        for stem in CURSOR_STEMS {
            let Some((w, h, rgba)) = world.get_resource_mut::<WorldAssets>().and_then(|wa| {
                read_texture_rgba(&mut wa.chain.lock_recover(), &cursor_path(stem)).ok()
            }) else {
                warn!("cursor {stem}.blp missing/undecodable — mode falls back");
                continue;
            };
            match build_cursor(w, h, &rgba) {
                Some(cursor) => {
                    cursors.insert(*stem, cursor);
                }
                None => warn!("failed to build NSCursor for {stem}.blp — mode falls back"),
            }
        }
        if cursors.is_empty() {
            warn!("no cursor BLPs available — using the OS cursor");
            return;
        }
        world.insert_non_send_resource(NativeCursors(cursors));
    }

    /// Disable the window's cursor rects once, set the payload or mode cursor while not looking,
    /// and hide or unhide it across mouselook.
    pub(super) fn drive(
        cursors: Option<NonSend<NativeCursors>>,
        mut payload_cursors: NonSendMut<PayloadCursors>,
        script: Option<NonSend<benilla_ui::script::UiScript>>,
        world_assets: Option<ResMut<WorldAssets>>,
        mode: Res<super::DisplayedCursor>,
        screen: Res<crate::loading_screen::LoadingScreen>,
        rig: Res<CameraControl>,
        cinematic: Option<Res<crate::cinematic::Cinematic>>,
        mut focus: MessageReader<bevy::window::WindowFocused>,
        // The `Local`s below remember OS state, which outlives the VM, so none is a
        // [`crate::ui_script::VmMemo`].
        mut was_looking: Local<bool>,
        mut rects_disabled: Local<bool>,
        mut decode_failed: Local<HashSet<String>>,
        mut last_set: Local<Option<String>>,
        // The last `set` cursor's pointer, the drift baseline; only compared, never dereferenced.
        mut last_ptr: Local<usize>,
    ) {
        let Some(cursors) = cursors else {
            return;
        };
        // macOS resets the cursor to the arrow on every activation: re-assert on focus gain.
        for ev in focus.read() {
            if ev.focused {
                *last_set = None;
            }
        }
        let held_icon = script
            .as_ref()
            .and_then(|s| payload_icon(s, screen.covering()));
        if let Some(icon) = &held_icon {
            if !payload_cursors.0.contains_key(icon) && !decode_failed.contains(icon) {
                let built = world_assets.and_then(|mut a| {
                    let rgba = super::decode_payload_cursor_rgba(&mut a, icon)?;
                    build_cursor(32, 32, &rgba)
                });
                match built {
                    Some(cursor) => {
                        payload_cursors.0.insert(icon.clone(), cursor);
                    }
                    None => {
                        warn!("cursor payload icon {icon} missing/undecodable — mode cursor kept");
                        decode_failed.insert(icon.clone());
                    }
                }
            }
        }
        // A cinematic hides the pointer too (`0x58b590`); `CursorOptions.visible` is inert here, so
        // it joins the look's hide/unhide pair, keeping AppKit's hide counter balanced.
        let looking = rig.is_looking() || cinematic.is_some_and(|c| c.is_playing());
        let stem = mode.0.stem();
        // The key names the chosen cursor, so `NSCursor::set`, a WindowServer round-trip that can
        // stall the main thread for milliseconds, fires only on a change or a drift.
        let key = held_icon
            .as_ref()
            .filter(|icon| payload_cursors.0.contains_key(icon.as_str()))
            .map(|icon| format!("payload:{icon}"))
            .unwrap_or_else(|| format!("mode:{stem}"));
        let cursor = held_icon
            .as_ref()
            .and_then(|icon| payload_cursors.0.get(icon.as_str()))
            .or_else(|| cursors.0.get(stem.as_str()))
            .or_else(|| cursors.0.get(stem.trim_start_matches("Unable")))
            .or_else(|| cursors.0.get("Point"));
        // SAFETY: main thread, guaranteed by the `NonSend` params; hide and unhide stay balanced
        // across the look transition.
        unsafe {
            if !*rects_disabled {
                if let Some(mtm) = MainThreadMarker::new() {
                    if let Some(window) = NSApplication::sharedApplication(mtm)
                        .windows()
                        .firstObject()
                    {
                        window.disableCursorRects();
                        *rects_disabled = true;
                    }
                }
            }
            if looking && !*was_looking {
                NSCursor::hide();
                // The next un-look re-asserts even an unchanged cursor.
                *last_set = None;
            } else if !looking && *was_looking {
                NSCursor::unhide();
            }
            if !looking {
                // Assert on a key change or on drift: winit's view and AppKit's activation resets
                // set the arrow at any moment, and a held payload's key never changes during a
                // carry. `currentCursor` is an app-local read, free every frame.
                let drifted = *last_ptr != 0
                    && Retained::as_ptr(&NSCursor::currentCursor()) as usize != *last_ptr;
                if drifted || last_set.as_deref() != Some(key.as_str()) {
                    if let Some(cursor) = cursor {
                        cursor.set();
                        *last_ptr = Retained::as_ptr(cursor) as usize;
                        // `WOW_CURSOR_TRACE=1`: the assert and drift timeline.
                        if std::env::var_os("WOW_CURSOR_TRACE").is_some() {
                            eprintln!(
                                "[cursor-trace] set {key} ({:#x}){}",
                                *last_ptr,
                                if drifted { " [healed drift]" } else { "" }
                            );
                        }
                        *last_set = Some(key);
                    }
                }
            }
        }
        *was_looking = looking;
    }

    /// RGBA → `NSCursor`, via PNG and `NSImage::initWithData`.
    fn build_cursor(width: u32, height: u32, rgba: &[u8]) -> Option<Retained<NSCursor>> {
        let buf = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        let data = NSData::with_bytes(&png);
        let image = NSImage::initWithData(NSImage::alloc(), &data)?;
        // Hotspot: the vanilla cursors' top-left tip.
        Some(NSCursor::initWithImage_hotSpot(
            NSCursor::alloc(),
            &image,
            NSPoint { x: 0.0, y: 0.0 },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::{CursorKind, WorldCursor};
    use bevy::ecs::system::RunSystemOnce;

    const CAST_GREY: WorldCursor = WorldCursor {
        kind: CursorKind::Cast,
        unable: true,
    };
    const SWORD: WorldCursor = WorldCursor {
        kind: CursorKind::Attack,
        unable: false,
    };

    /// One frame of [`drive_displayed_cursor`] from `standing`; `lua` is what FrameXML did.
    fn frame(standing: WorldCursor, world: WorldCursor, over_ui: bool, lua: &str) -> WorldCursor {
        frame_armed(standing, world, over_ui, lua, false)
    }

    /// One frame; `armed` parks the base at Cast.
    fn frame_armed(
        standing: WorldCursor,
        world: WorldCursor,
        over_ui: bool,
        lua: &str,
        armed: bool,
    ) -> WorldCursor {
        frame_covered(standing, world, over_ui, lua, armed, false)
    }

    /// One frame; `covered` raises the loading cover.
    fn frame_covered(
        standing: WorldCursor,
        world: WorldCursor,
        over_ui: bool,
        lua: &str,
        armed: bool,
        covered: bool,
    ) -> WorldCursor {
        let mut app = App::new();
        let script = benilla_ui::script::UiScript::new().unwrap();
        script.run(lua).unwrap();
        app.insert_non_send_resource(script);
        app.insert_resource(if covered {
            crate::loading_screen::LoadingScreen::test_covering()
        } else {
            crate::loading_screen::LoadingScreen::default()
        });
        app.insert_resource(world);
        app.insert_resource(crate::ui_script::PointerOverUi(over_ui));
        app.insert_resource(DisplayedCursor(standing));
        let mut targeting = crate::spell::SpellTargeting::default();
        if armed {
            // Feed Pet's own bare item word.
            targeting.enter(6991, crate::spell::CastCommit::Spell, 0x0010);
        }
        app.insert_resource(targeting);
        app.world_mut()
            .run_system_once(drive_displayed_cursor)
            .expect("the displayed cursor drives");
        app.world().resource::<DisplayedCursor>().0
    }

    /// The reference parks cursor index 1 before the loading screen goes up (`0x6e49f5`).
    #[test]
    fn the_loading_cover_parks_the_plain_arrow_over_every_other_rule() {
        for (over_ui, lua, armed) in [
            (false, "", false),
            (true, "", true),
            (true, "SetCursor(\"CAST_CURSOR\")", true),
            (false, "", true),
        ] {
            assert_eq!(
                frame_covered(SWORD, SWORD, over_ui, lua, armed, true),
                WorldCursor::default(),
                "covered: the arrow wins (over_ui={over_ui}, armed={armed}, lua={lua:?})"
            );
        }
        // Uncovered, the same frame keeps the world's verdict.
        assert_eq!(
            frame_covered(SWORD, SWORD, false, "", false, false),
            SWORD,
            "no cover, no park"
        );
    }

    /// `ResetCursor` restores the base cell (`0xbe2c4c`): blue over the UI while armed, grey only
    /// in the world, and no item validity verdict.
    #[test]
    fn an_armed_spell_parks_the_base_at_cast_so_the_ui_reads_blue() {
        const GREY: WorldCursor = CAST_GREY;
        const BLUE: WorldCursor = WorldCursor {
            kind: CursorKind::Cast,
            unable: false,
        };
        // In the world an item-only word greys.
        assert_eq!(frame_armed(BLUE, GREY, false, "", true), GREY);
        // Crossing into the UI restores the base.
        assert_eq!(frame_armed(GREY, GREY, true, "", true), BLUE);
        // A bag slot's `ResetCursor()` resolves to the base, whatever the slot holds:
        assert_eq!(frame_armed(GREY, GREY, true, "ResetCursor()", true), BLUE);
        // the blue means a spell is armed, not that the item is a legal target.
        assert_eq!(frame_armed(GREY, GREY, true, "ResetCursor()", true), BLUE);
        // Nothing armed: the base is Point.
        assert_eq!(
            frame_armed(GREY, WorldCursor::default(), true, "ResetCursor()", false),
            WorldCursor::default()
        );
    }

    /// A UI element that writes no cursor does not invent one.
    #[test]
    fn only_the_world_and_framexml_write_the_cursor() {
        // Over the world the classifier's verdict applies.
        assert_eq!(frame(CAST_GREY, SWORD, false, ""), SWORD);
        // Over the UI with nothing armed the base is Point.
        assert_eq!(frame(SWORD, SWORD, true, ""), WorldCursor::default());
        assert_eq!(
            frame(SWORD, SWORD, true, "ResetCursor()"),
            WorldCursor::default()
        );
        // A FrameXML write wins and stays, or the unit-frame lit/grey split would be stamped over.
        let lit = WorldCursor {
            kind: CursorKind::Cast,
            unable: false,
        };
        assert_eq!(
            frame(
                WorldCursor::default(),
                SWORD,
                true,
                "SetCursor(\"CAST_CURSOR\")"
            ),
            lit
        );
        assert_eq!(
            frame(lit, SWORD, true, "SetCursor(\"CAST_ERROR_CURSOR\")"),
            CAST_GREY
        );
    }

    /// `ShowContainerSellCursor` bails on `IsTargeting` first, so an armed spell keeps its cursor.
    #[test]
    fn the_sell_cursor_does_not_paint_over_an_armed_spell() {
        // The gate reads the app-fed targeting flag.
        let mut app = App::new();
        let mut script = benilla_ui::script::UiScript::new().unwrap();
        script.set_spell_targeting(true);
        script.set_container(
            0,
            Some(benilla_ui::script::ContainerState {
                name: None,
                num_slots: 16,
                slots: std::collections::HashMap::from([(
                    1,
                    benilla_ui::script::ContainerSlot::default(),
                )]),
            }),
        );
        script.run("ShowContainerSellCursor(0, 1)").unwrap();
        app.insert_non_send_resource(script);
        app.insert_resource(crate::loading_screen::LoadingScreen::default());
        app.insert_resource(CAST_GREY);
        app.insert_resource(crate::ui_script::PointerOverUi(true));
        app.insert_resource(DisplayedCursor(CAST_GREY));
        let mut targeting = crate::spell::SpellTargeting::default();
        targeting.enter(6991, crate::spell::CastCommit::Spell, 0x0010);
        app.insert_resource(targeting);
        app.world_mut()
            .run_system_once(drive_displayed_cursor)
            .expect("the displayed cursor drives");
        assert_eq!(
            app.world().resource::<DisplayedCursor>().0,
            WorldCursor {
                kind: CursorKind::Cast,
                unable: false
            },
            "an armed spell suppresses the sell cursor entirely — the armed base shows instead"
        );
    }
}
