//! The mouse cursor — the real client's `Interface\Cursor\*.blp` set, shown as a crisp **hardware
//! (OS-composited) cursor** so it has zero lag, matching the reference 1.12 client (whose cursor is
//! lag-free on this same machine). Which cursor shows is the targeting classifier's call
//! ([`crate::target::WorldCursor`], the reference's decision tree at `0x4828d0`): Point by
//! default, the sword over an attackable unit, the speech bubble / pouch / trainer / taxi over
//! service NPCs, loot/skin over corpses — each with its grayed `Unable*` twin out of range. The
//! whole set preloads at startup (18 tiny BLPs); a stem missing from the archives falls back toward
//! the base cursor, then Point.
//!
//! macOS needs native AppKit. winit drives the cursor through the legacy cursor-rect API
//! (`addCursorRect`/`resetCursorRects`), which a continuously-redrawing Metal view doesn't honor on
//! mouse-move — so winit's `CursorIcon::Custom` and `CursorOptions.visible = false` both revert to the
//! arrow the instant you move (verified against winit's source). We bypass that: build an `NSCursor`
//! per mode from its BLP, call `NSWindow::disableCursorRects` to stop AppKit's cursor-rect
//! reconciliation, then `[cursor set]` the current mode directly (re-asserted each non-look frame).
//! During mouselook we `NSCursor::hide()`/`unhide()` (the reliable app-global hide counter). Other
//! platforms use winit's `CursorIcon::Custom` (which works there), swapped on mode change; their
//! hide-on-look goes through `CursorOptions.visible`, handled in `player::control`.
//!
//! **The held cursor payload**: while `UiScript::cursor_payload()` holds a
//! payload with a resolved icon, the HARDWARE cursor becomes that icon instead of the classified
//! mode — the real client composites the item's `Interface\Icons\…` art into its drag bitmap
//! (`0x523840`) and uploads it via the same `SetHardwareCursor` path the mode art uses
//! (`0x523790`). Both platform `drive` fns below check the held payload FIRST, each frame, and fall
//! back to the mode cursor when nothing is held or its icon fails to decode; each caches its built
//! cursor per icon path ([`other::PayloadCursorImages`]/[`macos::PayloadCursors`]) — repeated
//! pickups of the same icon never re-decode. The icons are 64×64 (unlike the mode BLPs, already
//! 32×32); [`box_downsample_32`] shrinks one to the cursor's fixed 32×32 with hotspot `(0, 0)` —
//! the pointer sits at the icon's top-left corner, the icon hangs down-right, matching the
//! reference look (and `ui_script::extract::cursor_icon_quad`'s capture-only stand-in).

use benilla_assets::AssetSet;
use bevy::prelude::*;

/// The held cursor payload's icon path (`Interface\Icons\…`, extensionless — the DBC/FrameXML
/// convention), any arm — `None` if nothing is held or that arm's icon hasn't resolved yet
/// (either way, the caller falls back to the mode cursor).
///
/// **`covered` — the loading cover drops the overlay** (VERIFIED). The reference's
/// world transition ends `0x401900 → 0x495920 → 0x6e4940` with `0x523d20(1)` + `0x523c20(1)`:
/// cursor index **1**, the plain arrow, with any item/spell overlay dropped, set *before* the
/// screen goes up. Without this a portal taken with an item on the cursor showed the item's icon
/// over the loading art for the whole load — the mode half of the same rule is
/// [`drive_displayed_cursor`]'s.
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
        // Mode 10 — the stabled pet's family icon. Always present: a non-empty
        // icon path is the grab's own gate.
        CursorPayload::StablePet(p) => Some(p.texture),
        // Mode 2 (1965) — the coin bitmap by magnitude, `GetCoinIcon`'s own table.
        CursorPayload::Money(m) => {
            Some(benilla_ui::script::coin_icon(i64::from(m.copper)).to_string())
        }
        // Mode 5 — the vendor row's icon. The reference stores the row's `ItemDisplayInfo` id
        // (`0xb4d8ec`) and resolves the art from it; we carry the resolved path, so this is the
        // same picture one hop later.
        CursorPayload::Merchant(m) => m.texture,
    }
}

/// Box-downsample a raw RGBA8 image (top-to-bottom raster order —
/// [`benilla_assets::WorldAssets::decode_rgba`]'s layout) to a fixed 32×32 buffer: each output
/// texel averages a `(w/32)×(h/32)` block of the source. Vanilla item icons are 64×64 — a clean
/// 2×2 average per output texel — but this degrades gracefully for any other
/// source size.
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

/// Decode a payload icon and box-downsample it to a 32×32 hardware-cursor-ready RGBA8 buffer
/// ([`box_downsample_32`]), alpha forced fully opaque — the client's own drag-bitmap composite
/// (`0x523840`: 64×64 → 2×2 box filter → 32×32, alpha written 0xFF; folded back by 0218). `None`
/// on a missing/undecodable icon.
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

/// Every cursor BLP stem benilla can currently show ([`crate::target::WorldCursor::stem`]): the
/// classifier's modes + their grayed twins. Preloaded once at startup.
const CURSOR_STEMS: &[&str] = &[
    "Point",
    "Attack",
    "UnableAttack",
    "Speak",
    "UnableSpeak",
    "Pickup",
    "UnablePickup",
    // The loot leg's triple pouch: effective auto-loot on (0961's setting XOR shift).
    "LootAll",
    "UnableLootAll",
    "Interact",
    "UnableInteract",
    "Buy",
    "UnableBuy",
    "Inspect", // the Ctrl-hover magnifier (ShowInspectCursor) AND the TEXT-GameObject plaque cursor
    "UnableInspect",
    "Trainer",
    "UnableTrainer",
    "Taxi",
    "UnableTaxi",
    "Skin",
    "UnableSkin",
    "Repair", // the repair-mode base cursor (never grayed — the shipped UnableRepair is unreachable)
    // The data-driven GameObject cursors (`0x5f8760`): a mailbox's Mail, a lock's
    // Mine / GatherHerbs (grayed out of reach), a picked lock's PickLock (never grayed).
    "Mail",
    "UnableMail",
    "Mine",
    "UnableMine",
    "GatherHerbs",
    "UnableGatherHerbs",
    "PickLock",
    // The spell-targeting pair (`0x4820f0`): Cast(2) / UnableCast(22). Cast is
    // the armed-enchant-pick overlay's mode; the grayed twin preloads with it (the overlay's
    // valid/invalid split is its named refinement).
    "Cast",
    "UnableCast",
];

/// A cursor stem's archive path.
fn cursor_path(stem: &str) -> String {
    format!("Interface\\Cursor\\{stem}.blp")
}

/// The **displayed** cursor — the client's single sticky mode global `0xbe2c2c`. Read by the
/// platform drivers instead of [`crate::target::WorldCursor`], which is only ever *one* of its two
/// writers.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct DisplayedCursor(pub(crate) crate::target::WorldCursor);

/// Resolve this frame's displayed cursor — **one sticky mode with two writers**.
///
/// The reference has exactly one cursor-mode cell and everything that wants a cursor calls
/// `CursorSetMode` on it. Two things do:
///
/// - **the world**, from the WorldFrame's hover handler `0x481790` — which runs *only while the
///   WorldFrame is the frame manager's mouse-focus frame* (`4817ae: cmp [[this+0xa0]+0x7c], this;
///   jne`). Over a UI frame it does not run, and writes nothing.
/// - **FrameXML**, from a hover handler — `ShowContainerSellCursor`, `ShowInspectCursor`,
///   `SetCursor`, `ResetCursor`.
///
/// **Between those two, nothing writes it, so the last value simply stands.** That is the whole
/// correction here, and B208's regression was getting it wrong in each direction at once: the old
/// code recomputed the mode from the world every frame (so a UI hover could never keep a cursor),
/// and 1055 then made the world *skip* over UI while the classifier still wrote Point underneath
/// (so every UI element force-reset the cursor). Either way an armed spell lost its cast cursor the
/// instant the mouse touched a spellbook button — which is what the director saw.
///
/// So: a FrameXML write wins the frame it happens; otherwise the world's verdict applies while the
/// pointer is over the world; otherwise the mode is left exactly as it was. The bag's `ResetCursor()`
/// still resets over a bag slot, because it is a *write* — and a spellbook button, which calls no
/// cursor function at all, correctly changes nothing.
fn drive_displayed_cursor(
    world: Res<crate::target::WorldCursor>,
    over_ui: Res<crate::ui_script::PointerOverUi>,
    targeting: Res<crate::spell::targeting::SpellTargeting>,
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    // The base last applied by the UI-entry restore below — `None` while the pointer is over the
    // world, so re-entering the UI always restores once.
    mut last: Local<crate::ui_script::VmMemo<Option<crate::target::WorldCursor>>>,
    mut displayed: ResMut<DisplayedCursor>,
    // The loading cover parks the mode at Point — see the arm at the top of the body.
    screen: Res<crate::loading_screen::LoadingScreen>,
) {
    use crate::target::{CursorKind, WorldCursor};
    use benilla_ui::script::UiCursorMode;

    // A VM memo — the restore edge below is armed by `take_cursor_write`, a VM read — but unlike
    // every other feed this system still drives the cursor with **no VM at all** (the character
    // screen: the VM lives for one login). `get_for` is the shape for exactly that:
    // no VM is a session in its own right, so the base restore re-arms once on each side of the
    // glue phase instead of every frame inside it.
    let last = last.get_for(script.as_deref());

    // **Under the loading cover the cursor is the plain arrow** (VERIFIED). The
    // reference's transition parks cursor index **1** at `0x6e49f5`/`0x6e49ff` before raising the
    // screen; ours is parked here every covered frame instead of once, which reads the same because
    // the cover has already taken the input plane and nothing can write the mode under it. Ahead of
    // every other arm — a FrameXML write, the repair/targeting base, the sticky UI mode — because
    // the reference's park is likewise unconditional. `last` is cleared like the over-world arm, so
    // the first crossing back into the UI after the reveal restores the base once.
    if screen.covering() {
        displayed.0 = WorldCursor::default();
        *last = None;
        return;
    }

    // **The BASE mode is not a constant** (`0xbe2c4c`: it is independently mutable — e.g. a
    // spell-cancel flow parks it at Cast(2)), and that is the piece B208 kept missing.
    // `ResetCursor` restores *the value of this cell*, not a hardcoded Point — so what a bag slot's
    // `ResetCursor()` shows depends entirely on what is parked here.
    //
    // While a spell awaits its click, the base is **Cast(2)**. The director watched the reference
    // do exactly this: with Feed Pet armed the cursor is blue over *"pretty much anything UI
    // related"* and grey only out in the world. Both halves fall out of one cell — over the world
    // the classifier writes the per-seam verdict (grey, for a word with no world handler), and over
    // UI nothing overrides the base, so the blue shows.
    //
    // The corroborating gate is `ShowContainerSellCursor 0x4fa460`'s second test, *"base mode is
    // Point(1), else bail"*: a test that is only ever meaningful because the base is routinely
    // **not** Point while targeting.
    //
    // Note what this blue does NOT mean: it is not a validity verdict. It says "a spell is armed",
    // not "this item is a legal target" — no hover-time item verdict exists anywhere in 1.12,
    // so valid and invalid food look identical here, exactly as they do in the
    // reference.
    let repair = script.as_ref().is_some_and(|s| s.repair_mode());
    let base = if repair {
        // `ShowRepairCursor 0x4fbcc0` parks the base at Repair for as long as it holds; it is the
        // explicit modal, so it wins the cell.
        WorldCursor {
            kind: CursorKind::Repair,
            unable: false,
        }
    } else if targeting.active()
        || script
            .as_ref()
            .is_some_and(|s| s.gift_wrap_armed().is_some())
    {
        // An armed **gift wrap** parks the base at Cast(2) for the same reason a targeting spell
        // does, and by the same act: `0x5edea0`'s three calls are `LockItem`, **`SetCursorBaseMode(2)`**
        // and `CursorSetMode(2)`. Both cells, deliberately — the displayed one so the
        // cursor changes at the click, the BASE one so it survives the pointer crossing the world,
        // where the classifier would otherwise write its own verdict over it. Setting only the
        // displayed mode would lose the wrap cursor the moment the mouse left the bag.
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
                // `ResetCursor` — displayed goes back to the base mode, Repair included.
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
    // Over UI with no FrameXML write this frame. The mode is still sticky between writes — a UI
    // element that calls no cursor function must not disturb it — but **crossing into the UI, and
    // any later change of the base, restores the base**, which is what makes an armed spell read
    // blue over the interface at large rather than dragging the world's grey in behind the mouse.
    //
    // In the reference that restore is spread across the UI rather than centralised: nearly every
    // hover handler ends in `ResetCursor()` (`ContainerFrameItemButton_OnEnter`'s else branch,
    // `CursorUpdate`/`CursorOnUpdate` in `UIParent.lua`, every `OnLeave`), and the world
    // classifier's own no-hover path is the same `0x523d30` restore. Modelling it as one edge here
    // gets the same observable without requiring each of our frames to have grown its
    // `ResetCursor()` call yet — and a frame that DOES write still wins, so the unit-frame
    // lit/grey split survives.
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

/// Non-macOS: winit's custom OS cursor works fine — preload every mode's image and swap the
/// window's `CursorIcon` when the classified mode changes. Hide-on-look is `CursorOptions.visible`,
/// driven by `player::control`.
#[cfg(not(target_os = "macos"))]
mod other {
    use super::{cursor_path, payload_icon, CURSOR_STEMS};
    use benilla_assets::{cursor_texture, WorldAssets};
    use bevy::platform::collections::{HashMap, HashSet};
    use bevy::prelude::*;
    use bevy::window::{CursorIcon, CustomCursor, CustomCursorImage, PrimaryWindow};

    /// stem → decoded cursor image, preloaded at startup.
    #[derive(Resource, Default)]
    pub(super) struct CursorImages(HashMap<String, Handle<Image>>);

    /// Held-payload icon path → its decoded 32×32 hardware cursor, built
    /// lazily on first use (unlike [`CursorImages`]'s fixed startup preload — the icon set is far
    /// too large to preload) and cached so a repeated pickup never re-decodes.
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

    /// Each frame: while a cursor payload with a resolved icon is held, show ITS 32×32 hardware
    /// cursor (decoded/downsampled on first use, then cached by path); otherwise swap to the
    /// classified mode (base-stem fallback, then Point, then OS) — unchanged from before 0216 §5.
    pub(super) fn drive(
        mut commands: Commands,
        cursor: Res<super::DisplayedCursor>,
        cursors: Option<Res<CursorImages>>,
        script: Option<NonSend<benilla_ui::script::UiScript>>,
        world_assets: Option<ResMut<WorldAssets>>,
        mut images: ResMut<Assets<Image>>,
        mut payload_cursors: ResMut<PayloadCursorImages>,
        // The loading cover drops the payload overlay — see [`payload_icon`].
        screen: Res<crate::loading_screen::LoadingScreen>,
        window: Option<Single<Entity, With<PrimaryWindow>>>,
        // The key of the cursor we last handed the window — the macOS arm's `last_set` under the
        // same name, because it is the same fact: OS state, not memory about the VM, so it is not a
        // [`crate::ui_script::VmMemo`] and does not re-seat at a new login (decision 1290's sweep).
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
            // Decode failed (or still pending catalogs): fall through to the mode cursor below.
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

    /// The shared `CustomCursor::Image` insert — hotspot `(0, 0)`: the vanilla mode cursors' active
    /// tip is their top-left pixel, and the held-payload icon is downsampled to match that same
    /// top-left hotspot convention.
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

    /// The built `NSCursor` per mode stem. Held as a non-send resource because AppKit types aren't
    /// `Send`/`Sync` and must only be touched on the main thread. `pub(super)` only so it can appear
    /// in `drive`'s param list (the plugin registers `drive` from the parent module).
    pub(super) struct NativeCursors(HashMap<&'static str, Retained<NSCursor>>);

    /// Held-payload icon path → its built 32×32 `NSCursor` — the mac twin of
    /// [`super::other::PayloadCursorImages`], built lazily on first use and cached by path.
    pub(super) struct PayloadCursors(HashMap<String, Retained<NSCursor>>);

    /// Build every mode's `NSCursor` from its BLP and store them. An **exclusive** system so it runs
    /// on the main thread (AppKit's requirement) and can insert the non-send resource. The
    /// window-level `disableCursorRects` + first `set` happen in [`drive`], once the window
    /// definitely exists.
    pub(super) fn setup(world: &mut World) {
        // Inserted unconditionally (ahead of the native-cursor early-return below) — `drive` reads
        // it every frame regardless of whether any MODE cursor decoded.
        world.insert_non_send_resource(PayloadCursors(HashMap::new()));
        // No client data at all → say it ONCE. The per-stem warning below means *this cursor is
        // missing from an install that exists*, which is worth a line each; with no install it is
        // 31 lines of the same fact, and they were the loudest thing in the log of the frame-one
        // crash that decision 1451 is about.
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

    /// Each frame (main thread, forced by the `NonSend` params): the first frame, stop AppKit's
    /// cursor-rect reconciliation on our window (the thing that reverts winit's cursor on move);
    /// then, while not looking, assert the cursor to show — the held payload's icon (decision 0216
    /// §5, decoded/downsampled to 32×32 on first use and cached by path) if one is held and
    /// resolved, else the classified mode (base-stem fallback, then Point); on entering/leaving
    /// mouselook, hide/show it via the app-global hide counter.
    pub(super) fn drive(
        cursors: Option<NonSend<NativeCursors>>,
        mut payload_cursors: NonSendMut<PayloadCursors>,
        script: Option<NonSend<benilla_ui::script::UiScript>>,
        world_assets: Option<ResMut<WorldAssets>>,
        mode: Res<super::DisplayedCursor>,
        // The loading cover drops the payload overlay — see [`super::payload_icon`].
        screen: Res<crate::loading_screen::LoadingScreen>,
        rig: Res<CameraControl>,
        cinematic: Option<Res<crate::cinematic::Cinematic>>,
        mut focus: MessageReader<bevy::window::WindowFocused>,
        // None of the `Local`s below is a [`crate::ui_script::VmMemo`], deliberately (decision
        // 1290's sweep): every one of them remembers something about the **OS** — what we last told
        // AppKit, which BLPs failed to decode, whether the pointer rects are off — and the OS keeps
        // that state across the VM's death and rebirth, so re-seating them at a new login would
        // re-assert a cursor nothing changed.
        mut was_looking: Local<bool>,
        mut rects_disabled: Local<bool>,
        mut decode_failed: Local<HashSet<String>>,
        mut last_set: Local<Option<String>>,
        // The raw pointer of the NSCursor we last `set` — the drift detector's baseline
        // (`WOW_CURSOR_TRACE`). Kept alive by the `cursors`/`payload_cursors` caches, and only
        // ever compared, never dereferenced.
        mut last_ptr: Local<usize>,
    ) {
        let Some(cursors) = cursors else {
            return;
        };
        // macOS resets the app's cursor to the arrow on every activation — a set that happened
        // while we weren't key (the very first frames run before the window shows; any cmd-tab
        // away) is simply lost. Re-assert on each focus gain, the moments AppKit forgets us.
        // (Before this, the glue screens showed the OS arrow: their cursor never changes off
        // Point, so nothing ever re-asserted after activation; in-world the constant mouselook
        // transitions masked it.)
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
        // **A cinematic hides the pointer too**, and on this platform it can only be done here:
        // the reference's `0x58b590(0)` at StartCinematic / `(1)` at End. `crate::cinematic` writes
        // `CursorOptions.visible`, which is the right lever everywhere else and is *inert on
        // macOS* for the reason this module exists — winit's cursor-rect route does not survive a
        // continuously-redrawing Metal view. So the fly-by's pointer is hidden by joining the
        // look session's own `NSCursor::hide()`/`unhide()` pair, which keeps AppKit's app-global
        // hide counter balanced across both reasons instead of racing a second one against it.
        let looking = rig.is_looking() || cinematic.is_some_and(|c| c.is_playing());
        let stem = mode.0.stem();
        // The held payload's cursor wins over the classified mode (falls back to the mode cursor
        // when nothing is held, or its icon hasn't resolved/decoded this frame). The KEY names what
        // we chose, so the set below fires only on a real cursor change or a detected drift —
        // `NSCursor::set` is a WindowServer round-trip that intermittently stalls the main thread
        // for milliseconds, and asserting it unconditionally every frame was the single biggest
        // line of the 0366 frame-time tail. `disableCursorRects` (below, once) stops AppKit's
        // cursor-RECT reconciliation (the every-mouse-move revert); the drift check in the set
        // block below catches what that can't — the other in-process `set` callers.
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
        // SAFETY: main thread (guaranteed by the `NonSend` params). hide/unhide are balanced across
        // the look transition so the app-global hide counter never drifts.
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
                // The next un-look must re-assert even an unchanged cursor (the hidden period is
                // AppKit's to fiddle with).
                *last_set = None;
            } else if !looking && *was_looking {
                NSCursor::unhide();
            }
            if !looking {
                // Assert on a key change OR on detected **drift**. The set is not fire-and-forget:
                // other in-process actors — winit's AppKit view (tracking-area `cursorUpdate:` /
                // `mouseEntered:` handlers) and AppKit's own activation resets — call `[NSCursor
                // set]` with THEIR cursor (the arrow) at unpredictable moments, replacing ours.
                // The mode cursors self-healed by accident (their `key` churns as the hover
                // classification changes), but a held payload's key is CONSTANT for the whole
                // carry, so one usurped set left the icon gone for good while the payload was
                // still held — the "spell disappears from the cursor but is still stuck to it"
                // bug (reproduced via WOW_CURSOR_TRACE; the drift fired with no input at all).
                // `currentCursor` is the app-local top of the cursor stack — a plain ObjC read,
                // no WindowServer round-trip — so checking it every frame is free, and `set`
                // still fires only on change-or-drift, preserving the 0366 frame-time rule.
                let drifted = *last_ptr != 0
                    && Retained::as_ptr(&NSCursor::currentCursor()) as usize != *last_ptr;
                if drifted || last_set.as_deref() != Some(key.as_str()) {
                    if let Some(cursor) = cursor {
                        cursor.set();
                        *last_ptr = Retained::as_ptr(cursor) as usize;
                        // `WOW_CURSOR_TRACE=1` — the assert/usurp timeline instrument.
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

    /// RGBA → `NSCursor`. Goes via PNG + `NSImage::initWithData` (standard image data) rather than
    /// hand-rolling an `NSBitmapImageRep`.
    fn build_cursor(width: u32, height: u32, rgba: &[u8]) -> Option<Retained<NSCursor>> {
        let buf = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        let data = NSData::with_bytes(&png);
        let image = NSImage::initWithData(NSImage::alloc(), &data)?;
        // Hotspot = the vanilla cursors' top-left active tip.
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

    /// Run one frame of [`drive_displayed_cursor`] with the displayed mode pre-set to `standing`,
    /// and return what it becomes. `lua` is whatever FrameXML did this frame.
    fn frame(standing: WorldCursor, world: WorldCursor, over_ui: bool, lua: &str) -> WorldCursor {
        frame_armed(standing, world, over_ui, lua, false)
    }

    /// One frame, with `armed` deciding whether a spell awaits its click — which is what parks the
    /// BASE mode at Cast.
    fn frame_armed(
        standing: WorldCursor,
        world: WorldCursor,
        over_ui: bool,
        lua: &str,
        armed: bool,
    ) -> WorldCursor {
        frame_covered(standing, world, over_ui, lua, armed, false)
    }

    /// One frame, with `covered` deciding whether the loading cover is up — the arm that parks the
    /// plain arrow ahead of every other rule.
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
            // Feed Pet's own bare ITEM word.
            targeting.enter(6991, crate::spell::CastCommit::Spell, 0x0010);
        }
        app.insert_resource(targeting);
        app.world_mut()
            .run_system_once(drive_displayed_cursor)
            .expect("the displayed cursor drives");
        app.world().resource::<DisplayedCursor>().0
    }

    /// **Under the loading cover the cursor is the plain arrow, whatever else is true** (decision
    /// 1990). The reference's world transition parks cursor index 1 (`0x6e49f5`/`0x6e49ff`) and
    /// drops any item/spell overlay *before* the screen goes up, so a portal taken with a spell
    /// armed — or with an item on the cursor — arrives showing the arrow, not the payload.
    ///
    /// Both of the arms that would otherwise win are exercised here: an armed spell (which parks
    /// the BASE at Cast, so even the UI restore reads blue) and a FrameXML write in the same frame.
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
        // …and the cover is the only reason: the same frame uncovered keeps the world's verdict.
        assert_eq!(
            frame_covered(SWORD, SWORD, false, "", false, false),
            SWORD,
            "no cover, no park"
        );
    }

    /// **The base mode is Cast while a spell awaits its click** (the base cell `0xbe2c4c` is
    /// independently mutable — e.g. a spell-cancel flow parks it at Cast(2)), and `ResetCursor`
    /// restores *that value*, never a hardcoded Point.
    ///
    /// This is what the director watched the reference do with Feed Pet armed: **blue over the
    /// interface at large, grey only out in the world**. Both fall out of the one cell — the world
    /// classifier writes the per-seam verdict (grey, for a word with no world handler), and over UI
    /// nothing overrides the base.
    ///
    /// It is emphatically NOT a validity verdict: no hover-time item verdict exists in 1.12, so
    /// good food and bad food are the same blue. The last assertion pins that, because reading the
    /// blue as "this food is correct" is exactly the misreading that sent B208 round twice.
    #[test]
    fn an_armed_spell_parks_the_base_at_cast_so_the_ui_reads_blue() {
        const GREY: WorldCursor = CAST_GREY;
        const BLUE: WorldCursor = WorldCursor {
            kind: CursorKind::Cast,
            unable: false,
        };
        // Out in the world the classifier's verdict stands: an item-only word greys.
        assert_eq!(frame_armed(BLUE, GREY, false, "", true), GREY);
        // Crossing into the UI restores the base — blue — rather than dragging the world's grey in.
        assert_eq!(frame_armed(GREY, GREY, true, "", true), BLUE);
        // A bag slot's real `ResetCursor()` resolves to the same base, so the food reads blue…
        assert_eq!(frame_armed(GREY, GREY, true, "ResetCursor()", true), BLUE);
        // …and so does a slot holding something the pet would never eat. The blue says "a spell is
        // armed", not "this item is a legal target".
        assert_eq!(frame_armed(GREY, GREY, true, "ResetCursor()", true), BLUE);
        // Nothing armed: the base is Point again, and the UI reads as the ordinary pointer.
        assert_eq!(
            frame_armed(GREY, WorldCursor::default(), true, "ResetCursor()", false),
            WorldCursor::default()
        );
    }

    /// **A UI element that writes no cursor does not get to invent one.** The mode has two
    /// writers and only two — the world (while the pointer is over it) and a FrameXML cursor call.
    /// Crossing into the UI restores the BASE, which is the reference's own behaviour spread across
    /// dozens of `ResetCursor()` calls; what must never happen is a *third* party recomputing a
    /// value each frame, which is what made B208 go round twice.
    #[test]
    fn only_the_world_and_framexml_write_the_cursor() {
        // Over the world the classifier's verdict applies, every frame.
        assert_eq!(frame(CAST_GREY, SWORD, false, ""), SWORD);
        // Over the UI with nothing armed the base is Point — the ordinary pointer, which is what a
        // bag slot shows in the reference.
        assert_eq!(frame(SWORD, SWORD, true, ""), WorldCursor::default());
        assert_eq!(
            frame(SWORD, SWORD, true, "ResetCursor()"),
            WorldCursor::default()
        );
        // An explicit FrameXML write wins and then STAYS — the unit-frame lit/grey split would be
        // pointless if the next frame's base restore stamped over it.
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

    /// `ShowContainerSellCursor` bails on `IsTargeting` at its first instruction, so an armed spell
    /// keeps its cast cursor over a vendor-open bag instead of getting the coin. With a sticky mode
    /// this gate is load-bearing: a Buy write would stamp over the cast cursor and no per-frame
    /// world write would put it back.
    #[test]
    fn the_sell_cursor_does_not_paint_over_an_armed_spell() {
        // The gate reads the app-fed targeting flag, so drive it the way the app does.
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
