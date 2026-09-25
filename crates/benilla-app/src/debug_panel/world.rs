//! The panel's World section: who you are, map and zone, WoW-space position and facing, and tile
//! residency, plus a copy of the vmangos `.go xyz` line and, while free-flying, a land-here
//! button. While free-flying the spot is the camera's, not the frozen avatar's.

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::world_to_tile;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::egui;

use super::OVERLAY_TEXT_DIM;
use crate::player::land::LandHere;

/// The World section's inputs, bundled to stay under `debug_panel_ui`'s 16-param ceiling.
#[derive(SystemParam)]
pub(super) struct WorldReadout<'w, 's> {
    player: Option<Res<'w, crate::player::Player>>,
    map: Option<Res<'w, benilla_world::world_map::CurrentMap>>,
    catalog: Option<Res<'w, benilla_assets::MapCatalogRes>>,
    area: Res<'w, benilla_world::terrain_stream::CurrentArea>,
    areas: Option<Res<'w, crate::area::AreaTableRes>>,
    interior: Res<'w, benilla_world::wmo_portal::CurrentAreaInterior>,
    /// The camera's WMO room claim and the exterior windows it produces, the two inputs of
    /// [`benilla_world::exterior_cull`].
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

/// Render the section: who, map, zone, position and tiles, the buttons last.
pub(super) fn world_section(ui: &mut egui::Ui, world: &mut WorldReadout) {
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

    // Map: id and `Map.dbc` name.
    let map_id = world.map.as_ref().map(|m| m.0);
    if let Some(id) = map_id {
        let name = world
            .catalog
            .as_ref()
            .and_then(|c| c.0.name(id))
            .unwrap_or("?");
        ui.label(format!("map {id} · {name}"));
    }

    // Zone: top zone and leaf area, and the indoor claim, from the same sources the zone text
    // reads, so the two cannot disagree.
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
        // The WMO skybox or the Light.dbc gradient, from the same down-ray as the interior claim.
        // The weight is the 4-second crossfade: `w 0.00` beside a name is a published skybox not
        // yet engaged, which is correct standing outside the gate.
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

    // The exterior-scene gate: the WMO room the camera's down-ray claims and the portal windows
    // its flood leaves onto the outdoors; terrain draws only through a window.
    //   * `room —`: no claim, the whole exterior draws (a `0x8`-flagged group claims nothing, and
    //     the reference draws the world there too).
    //   * many windows, or one covering most of the screen: the doorway rects are too generous.
    //   * `windows 0`: sealed; anything exterior still drawn is not tagged `ExteriorScene`, a bug.
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

    // Position and facing in raw WoW coords; the tile underfoot and the stream's residency.
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

    // Free-flying: the lines above stay the frozen body; this line and the buttons below use
    // the camera's coordinates.
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

    // `.go xyz x y z [mapid]`, vmangos's argument order (`TeleportCommands.cpp:854`).
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
    // The button form of the dev chord's `G`, only while detached: attached, the camera sits
    // behind the avatar.
    if detached_at.is_some() && ui.button("land here").clicked() {
        world.land.write(LandHere);
    }
}
