//! The panel's **World** section — the `.gps` of the client: who you are, which map/zone you're
//! in (and whether the zone system calls it indoors), the exact WoW-space position + facing, and
//! the terrain stream's tile residency. Pure readout except two affordances:
//!
//! - **copy `.go xyz`** — a click puts the vmangos teleport line for the current spot on the
//!   clipboard (`.go xyz x y z [mapid]`, verified against vmangos `HandleGoXYZCommand`), so "it
//!   looks wrong *here*" becomes a pasteable coordinate for the headless probes
//!   (`WOW_PROBE_CHAT`, live-shot runs) and the FPS-journal loop closes without hand-copying
//!   numbers.
//! - **land here** — the button half of [`crate::player::land`] (whose chord is the dev chord + `G`),
//!   shown only while free-flying.
//!
//! **While free-flying, the spot is the CAMERA's** — the section gains a `camera` line and the
//! copy button switches to it. The subject here is "where you are", and detached that is where you
//! flew to; the frozen avatar is not it. Copying the body's coordinates was the old behaviour, and
//! it was wrong at exactly the moment the button is worth pressing: you fly out to a spot
//! precisely *because* you want its number.

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::world_to_tile;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::egui;

use super::OVERLAY_TEXT_DIM;
use crate::player::land::LandHere;

/// The World section's read side, bundled (the 16-param ceiling of `debug_panel_ui`): where the
/// player is — map, zone leaf, indoor claim, tile residency — and who they are (guid + the
/// ask-once name cache the unit frames use). Plus the free-fly pair: the camera's own transform
/// (the detached spot) and the land-here ask the button writes.
#[derive(SystemParam)]
pub(super) struct WorldReadout<'w, 's> {
    player: Option<Res<'w, crate::player::Player>>,
    map: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
    catalog: Option<Res<'w, benilla_assets::MapCatalogRes>>,
    area: Res<'w, benilla_world::terrain_stream::CurrentArea>,
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    interior: Res<'w, benilla_world::wmo_portal::CurrentAreaInterior>,
    /// The camera's own WMO room claim + the exterior-window worklist it produces — the two inputs
    /// [`benilla_world::exterior_cull`] runs on. See the readout below for why they are worth a line.
    room: Res<'w, benilla_world::wmo_portal::CameraInteriorClaim>,
    windows: Res<'w, benilla_world::wmo_portal::ExteriorWindows>,
    skybox: Res<'w, benilla_world::skybox::CameraSkybox>,
    skybox_weight: Res<'w, benilla_world::skybox::SkyboxWeight>,
    streamer: Res<'w, benilla_world::terrain_stream::TerrainStreamer>,
    self_guid: Res<'w, crate::net::SelfGuid>,
    names: Res<'w, crate::names::NameCache>,
    net_commands: Res<'w, crate::net::NetCommands>,
    camera: Query<'w, 's, &'static Transform, With<benilla_world::view::WorldCamera>>,
    land: MessageWriter<'w, LandHere>,
}

/// Render the section: readout lines top-down (who → map → zone → position → tiles), the copy
/// affordance last.
pub(super) fn world_section(ui: &mut egui::Ui, world: &mut WorldReadout) {
    // Who: the character name through the ask-once cache (fills a frame later, like the unit
    // frames), or a plain offline line.
    match world.self_guid.0 {
        Some(guid) => {
            let name = world
                .names
                .resolve(guid, &world.net_commands)
                .unwrap_or("…")
                .to_string();
            ui.strong(name);
            ui.label(egui::RichText::new(format!("guid {guid:#x}")).color(OVERLAY_TEXT_DIM));
        }
        None => {
            ui.label(egui::RichText::new("offline — no character").color(OVERLAY_TEXT_DIM));
        }
    }

    // Map: id + Map.dbc name (the directory is the asset path, the name is the human one).
    let map_id = world.map.as_ref().map(|m| m.0);
    if let Some(id) = map_id {
        let name = world
            .catalog
            .as_ref()
            .and_then(|c| c.0.name(id))
            .unwrap_or("?");
        ui.label(format!("map {id} · {name}"));
    }

    // Zone: the leaf area and its top zone (one line when they coincide), plus the zone-text
    // indoor claim — the same authorities the splash/minimap text reads, so this readout and
    // the player-facing text can never disagree.
    if let Some(leaf) = world.area.0 {
        let (leaf_name, zone_name) = match world.areas.as_ref() {
            Some(a) => (a.0.name(leaf), a.0.top_zone(leaf).and_then(|z| a.0.name(z))),
            None => (None, None),
        };
        let mut line = match (zone_name, leaf_name) {
            (Some(z), Some(l)) if z != l => format!("{z} — {l}"),
            (_, Some(l)) => l.to_string(),
            _ => format!("area {leaf}"),
        };
        if world.interior.0.is_some() {
            line.push_str("  ·  indoors");
        }
        // The WMO skybox engagement, right beside the interior claim it derives from: both come off
        // the camera's down-ray seed, so when the backdrop flips between the building's painted sky
        // and the Light.dbc gradient, this is the line that says which — and whether the claim moved
        // under it. From the chair the two are only distinguishable by colour. The weight is the
        // 4-second crossfade (== the interior-fog blend, one number): `w 0.00` beside a name means
        // the flood published a skybox the crossfade hasn't engaged — faithful standing outside
        // the gate, not a resolve bug.
        match world.skybox.0.as_deref() {
            Some(path) => {
                let leaf = path.rsplit('\\').next().unwrap_or(path);
                line.push_str(&format!(
                    "  ·  skybox {leaf} w {:.2}",
                    world.skybox_weight.0
                ));
            }
            None => line.push_str("  ·  sky gradient"),
        }
        ui.label(line);
        ui.label(egui::RichText::new(format!("leaf area {leaf}")).color(OVERLAY_TEXT_DIM));
    }

    // The exterior-scene gate, in the two terms that decide it: which WMO ROOM the
    // camera's own down-ray claims, and how many portal windows that room's flood left onto the
    // outdoor world. Terrain draws iff a window admits it, so "why can I still see the ground from
    // in here?" has exactly three answers and this line says which:
    //   * `room —` — no claim at all. We are on the OUTSIDE leg and draw the whole exterior, which
    //     is correct *if* the reference is too (a `0x8`-flagged group claims nothing — Stratholme's
    //     entrance hall is EXTERIOR, and the reference draws the world there as well).
    //   * `windows N` with N large, or a window covering most of the screen — we are indoors and the
    //     doorway rects are too generous.
    //   * `windows 0` — sealed, nothing exterior may draw; terrain visible anyway means something
    //     is reaching the screen that is not tagged `ExteriorScene`. Every exterior bucket is
    //     tagged now — WMO placements since 0784, open-world liquid since 1652 — so at `windows 0`
    //     the honest reading of anything still drawn is a bug, not a known gap.
    // Two numbers is the whole diagnosis, and neither was readable from the chair before.
    let room_line = match world.room.0 {
        Some(claim) => format!("room g{:02}", claim.room.group),
        None => "room —".to_string(),
    };
    let window_line = match &*world.windows {
        benilla_world::wmo_portal::ExteriorWindows::Unrestricted => {
            "exterior unrestricted".to_string()
        }
        benilla_world::wmo_portal::ExteriorWindows::Windows(rects) => {
            let widest = rects
                .iter()
                .map(|[x0, y0, x1, y1]| ((x1 - x0) * (y1 - y0)).max(0.0))
                .fold(0.0f32, f32::max);
            format!(
                "exterior {} window(s) · widest {:.0}% of screen",
                rects.len(),
                widest / 4.0 * 100.0
            )
        }
    };
    ui.label(egui::RichText::new(format!("{room_line}  ·  {window_line}")).color(OVERLAY_TEXT_DIM));

    // Position + facing, raw WoW coords (what the wire and every probe speak); the tile the
    // feet are on and how much of the stream window is spawned.
    let Some(player) = world.player.as_ref().filter(|p| p.active) else {
        return;
    };
    let [x, y, z] = bevy_to_wow(player.pos);
    let o = player.facing().rem_euclid(std::f32::consts::TAU);
    let detached = player.detached;
    ui.add_space(4.0);
    ui.label(egui::RichText::new(format!("{x:.1}  {y:.1}  {z:.1}")).monospace());
    ui.label(
        egui::RichText::new(format!("facing {o:.2} rad ({:.0}°)", o.to_degrees()))
            .color(OVERLAY_TEXT_DIM),
    );
    let (tx, ty) = world_to_tile(x, y);
    let (spawned, requested) = world.streamer.residency();
    ui.label(
        egui::RichText::new(format!("tile {tx},{ty}  ·  {spawned}/{requested} spawned"))
            .color(OVERLAY_TEXT_DIM),
    );

    // Free-flying: the camera is the spot (the lines above are the frozen body, kept — knowing
    // where you left it is exactly what you fly back to). This line, and the copy/land pair below,
    // all speak the camera's coordinates while detached.
    let detached_at = detached
        .then(|| {
            world
                .camera
                .single()
                .ok()
                .map(|c| bevy_to_wow(c.translation))
        })
        .flatten();
    if let Some([cx, cy, cz]) = detached_at {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!("camera {cx:.1}  {cy:.1}  {cz:.1}"))
                .monospace()
                .color(OVERLAY_TEXT_DIM),
        );
    }

    // The teleport line: `.go xyz x y z [mapid]` (vmangos argument order). Copied, not shown —
    // the readout above already displays every number.
    let [gx, gy, gz] = detached_at.unwrap_or([x, y, z]);
    let copy_label = if detached_at.is_some() {
        "copy .go xyz (camera)"
    } else {
        "copy .go xyz"
    };
    if ui.button(copy_label).clicked() {
        let line = match map_id {
            Some(id) => format!(".go xyz {gx:.2} {gy:.2} {gz:.2} {id}"),
            None => format!(".go xyz {gx:.2} {gy:.2} {gz:.2}"),
        };
        ui.ctx().copy_text(line);
    }
    // Land here — the button half of the dev chord's `G`. Only while detached, because
    // attached the camera sits behind the avatar and "land at the camera" means a step backwards.
    if detached_at.is_some() && ui.button("land here").clicked() {
        world.land.write(LandHere);
    }
}
