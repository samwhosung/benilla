//! The HUD minimap renderer, the app half of the `<Minimap>` widget: [`emit_minimap`] fills the
//! widget's `QuadContent::Minimap` hole with the tiles around the player, masked to
//! `MinimapMask.blp`, and the player arrow on top; the widget's FrameXML children draw above.
//!
//! - Tiles: one 256² BLP per ADT tile (533.33 yd), `map<X>_<Y>.blp` resolved through
//!   `md5translate.trs` ([`benilla_formats::MinimapTranslate`]), indexed in ADT order.
//! - Zoom to radius (`0x6da9b0`): two zoom indices, picked by WMO containment, each persisted
//!   (`minimapZoom`, `minimapInsideZoom`); outdoors a chunk-count half-extent, indoors
//!   [`INTERIOR_ZOOM_RADIUS`] yards.
//! - North up: screen up is world +X, screen left world +Y.

pub(crate) mod blips;
/// Where a party member is, shared with the world map so the two surfaces never disagree.
pub(crate) use blips::party_member_pos;
mod composite;
mod interior;
/// The minimap ping, engine-owned and pinned to a world point.
mod ping;
pub(crate) use ping::MinimapPing;

use std::collections::HashMap;

use bevy::math::{Affine3A, Rect};
use bevy::prelude::*;

use benilla_assets::coords::{bevy_to_wow, wow_to_bevy};
use benilla_assets::minimap_grid::group_axis_grid;
use benilla_assets::WmoModel;
use benilla_formats::{tile_to_world, world_to_tile, MinimapTranslate};

use interior::{interior_group_selection, wmo_minimap_stem};

use benilla_ui::widget::MINIMAP_DEFAULT_ZOOM;

use crate::player::Player;
use crate::ui_pass::{UiQuad, UiQuadAppend, UiQuadMask, UiQuads, UvRect};
use benilla_assets::MapCatalogRes;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::wmo_portal::{
    down_ray_seeds, terrain_z_local, WmoPortalInstance, INTERIOR_PROBE_HEIGHT,
};
use benilla_world::world_map::CurrentMap;

/// Yards per ADT tile / per MCNK chunk (16 chunks per tile edge).
const TILE_YARDS: f32 = 533.333_3;
const CHUNK_YARDS: f32 = TILE_YARDS / 16.0;

/// The outdoor zoom table (`0x8116d0`): view diameter in chunks per zoom index; half-extent =
/// `chunks · 0.5 · 33.333` yd (`0x6da9b0`).
const ZOOM_CHUNKS: [f32; 6] = [14.0, 12.0, 10.0, 8.0, 6.0, 4.0];

/// The indoor view radius in yards per indoor zoom index: table `0x8116e8`, indexed by `0x86f69c`
/// (`minimapInsideZoom`). The on-screen radius is exactly this (the composite at `1.5·c`,
/// `0x4ec090`, blits its middle two-thirds, `0x4ec440`), and the same `c` sizes the selection box
/// (`0x6d96b6`).
///
/// Not a constant: the `10.0f` at `0x4ed9eb` is only the field's static initializer, and
/// `0x6d98f5` writes `radiusTable[indoorZoom]` into it whenever the zoom or indoor state changes.
const INTERIOR_ZOOM_RADIUS: [f32; 6] = [150.0, 120.0, 90.0, 60.0, 40.0, 25.0];

/// How far the map reaches, in yards; both branches of [`emit_minimap`] and the ping read it here.
fn view_radius_yd(zoom: u8, inside_zoom: u8, inside: bool) -> f32 {
    let clamp = |z: u8| usize::from(z.min(5));
    if inside {
        INTERIOR_ZOOM_RADIUS[clamp(inside_zoom)]
    } else {
        ZOOM_CHUNKS[clamp(zoom)] * 0.5 * CHUNK_YARDS
    }
}

/// The outer-edge bleed: a tile on its group grid's boundary grows 1 yd on that side, so groups
/// whose bboxes touch overlap by 2 yd (`0x6a549e`…`0x6a54db`, `0xca8098` = `0.5 + 0.5`).
const EDGE_BLEED_YD: f32 = 1.0;

/// The reference's half-texel UV inset as a quad scale: a tile of `extent` yd baked at
/// [`YD_PER_TEXEL`](benilla_assets::minimap_grid::YD_PER_TEXEL) is `W` texels wide, and mapping
/// texel centres to the quad's edges stretches it by `W / (W − 1)`.
fn texel_stretch(extent_yd: f32) -> f32 {
    let texels = extent_yd / benilla_assets::minimap_grid::YD_PER_TEXEL;
    if texels > 1.0 {
        texels / (texels - 1.0)
    } else {
        1.0
    }
}

/// The interior tile draw's alpha-test reference, the reference's `GL_GEQUAL` 224/255. EGxBlend 1
/// (blending off) leaves it to `SetRenderState`'s cascade, `.data 0x85ad20[1] = 224` times the f32
/// reciprocal of 255: one ULP above 224/255, and fragments are compared against those exact bits.
pub(crate) const INTERIOR_TILE_ALPHA_REF: f32 = f32::from_bits(0x3F60_E0E2);

/// The corpse blip's edge as a fraction of the widget side (the POIIcons cell's 16 px on a 140 px
/// minimap); an estimate, the reference's in-range corpse blip size being untraced.
const CORPSE_BLIP_FRACTION: f32 = 0.11;

/// The day-night tint the reference modulates outdoor tiles by (tile draw `0x4eccdd`–`0x4ecd69`),
/// from `color_a`, the Direct band (`LightIntBand` 0), and `color_b`, the Ambient band (1):
///
/// ```text
///   L  = luma601(color_b)                # (r·77 + g·151 + b·28) >> 8, on 0..255 bytes
///   t  = min(L + 96, 255) / 256          # a +96 floor: even pitch-dark tints ~0.375 toward white
///   B' = lerp(color_b, white, t)
///   A' = lerp(color_a, B', 0.75) = 0.25·color_a + 0.75·B'
/// ```
///
/// Gamma-space in and out, handed to the UI quad as its vertex colour. Interior tiles draw white.
fn minimap_day_tint(ambient: [f32; 3], diffuse: [f32; 3]) -> [f32; 3] {
    let (color_a, color_b) = (diffuse, ambient);
    // Rec.601 luma: the weights sum to 256, so ×255/256 gives the byte the reference's `>> 8` does.
    let l_byte = 255.0 * (color_b[0] * 77.0 + color_b[1] * 151.0 + color_b[2] * 28.0) / 256.0;
    let t = (l_byte + 96.0).min(255.0) / 256.0;
    let mut out = [0.0_f32; 3];
    for c in 0..3 {
        let b_prime = color_b[c] + (1.0 - color_b[c]) * t; // lerp(color_b, white, t)
        out[c] = color_a[c] * 0.25 + b_prime * 0.75; // lerp(color_a, B', 0.75)
    }
    out
}

/// Stream a `md5translate.trs` hit off-thread as a minimap tile (`BlpVariant::MapTile`: gamma
/// bytes, no sRGB decode, as the reference's `GL_SKIP_DECODE_EXT` sampler; clamp, mip 0, linear).
/// Every consumer must decode after the filter or draw ~2× too bright: the outdoor quads set
/// [`UiQuad::gamma_texel`](crate::ui_pass::UiQuad::gamma_texel), and the interior composite's
/// alpha-test arm decodes itself.
fn load_tile(asset_server: &AssetServer) -> impl Fn(&str) -> Handle<Image> + '_ {
    move |hash: &str| {
        asset_server.load_with_settings(
            format!("mpq://textures/Minimap/{hash}"),
            |s: &mut benilla_assets::BlpLoaderSettings| {
                s.variant = benilla_assets::BlpVariant::MapTile;
            },
        )
    }
}

/// Under `WOW_MM_PROBE` alone, a synthetic minimap slot for a server-less capture
/// ([`crate::capture`]), which has no FrameXML to publish one: a top-right square at zoom 3.
fn probe_minimap_widget(mut widget: ResMut<MinimapWidget>, windows: Query<&Window>) {
    if widget.0.is_some() || std::env::var("WOW_MM_PROBE").is_err() {
        return;
    }
    let Ok(win) = windows.single() else { return };
    let side = (win.height() * 0.22).min(win.width() * 0.22);
    let margin = side * 0.15;
    let min = Vec2::new(win.width() - side - margin, margin);
    widget.0 = Some(MinimapSlot {
        rect: Rect::from_corners(min, min + Vec2::splat(side)),
        z: u64::MAX / 2,
        zoom: 3,
        inside_zoom: 3,
        alpha: 1.0,
    });
}

/// This frame's `<Minimap>` slot, from `ui_script::extract::paint_script`; `None` when hidden.
#[derive(Resource, Default)]
pub(crate) struct MinimapWidget(pub(crate) Option<MinimapSlot>);

/// The player's WMO containment this frame, [`feed_minimap_inside`]'s verdict; the reference keeps
/// one flag (`0xceaa60`) for the draw and the zoom buttons.
#[derive(Resource, Default)]
pub(crate) struct MinimapInside(pub(crate) bool);

/// The persisted zoom, CVars `minimapZoom` and `minimapInsideZoom` (default `"3"`, `0x63db90`):
/// read once to seed a fresh widget, written on every `Minimap:SetZoom`, as the reference's
/// `set_zoom` writes both the live index and the CVar.
#[derive(Resource)]
pub(crate) struct MinimapZoom {
    /// `minimapZoom`, the outdoor index.
    pub(crate) outdoor: u8,
    /// `minimapInsideZoom`, the indoor index, persisted apart from the outdoor level.
    pub(crate) inside: u8,
}

/// The zoom CVars' change callback: each lands on its own field, clamped at 5 like the reference's
/// `set_zoom` (`0x6daa10`).
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut zoom: ResMut<MinimapZoom>) {
    match ev.key().as_str() {
        "minimapzoom" => zoom.outdoor = zoom_index(ev.num()),
        "minimapinsidezoom" => zoom.inside = zoom_index(ev.num()),
        _ => {}
    }
}

/// A stored zoom level to an index, truncated and clamped into `[0, MINIMAP_ZOOM_LEVELS)`.
fn zoom_index(v: f32) -> u8 {
    v.clamp(0.0, f32::from(benilla_ui::widget::MINIMAP_ZOOM_LEVELS - 1)) as u8
}

impl Default for MinimapZoom {
    fn default() -> Self {
        Self {
            outdoor: MINIMAP_DEFAULT_ZOOM,
            inside: MINIMAP_DEFAULT_ZOOM,
        }
    }
}

/// One extracted Minimap widget: its rect (y-down logical px), paint key and live state.
pub(crate) struct MinimapSlot {
    pub(crate) rect: Rect,
    pub(crate) z: u64,
    /// The outdoor zoom index; `inside_zoom` is the indoor one, picked by WMO containment.
    pub(crate) zoom: u8,
    pub(crate) inside_zoom: u8,
    pub(crate) alpha: f32,
}

/// The loaded minimap fixtures; absent, the map draws nothing and its XML children still render.
#[derive(Resource)]
struct MinimapAssets {
    translate: MinimapTranslate,
    mask: Option<Handle<Image>>,
    arrow: Option<Handle<Image>>,
    /// The POI atlas (`Interface\Minimap\POIIcons`), for the corpse skull.
    poi: Option<Handle<Image>>,
    /// The four rim-arrow arts: the reference's one `minimapArrowModel`
    /// (`Rotating-MinimapArrow.mdx`) shows exactly one of these layers per blip source's looping
    /// sequence, so one flat sprite each draws the same ([`blips::RimArrow`]).
    rim_arrows: blips::RimArrowArt,
    /// The unit-blip atlas (`Interface\Minimap\ObjectIcons`, five 32-px dot cells).
    object_icons: Option<Handle<Image>>,
    /// `SpellShapeshiftForm.dbc`, the tracking dots' creature-type override (a cat-form druid is a
    /// Beast).
    forms: Option<HashMap<u32, benilla_formats::ShapeshiftForm>>,
}

/// Tile handles by ADT index for one [`CurrentMap`], `None` for a tile with no art (open ocean);
/// the interior half by `(group, col, row)` for the building whose tile stem is resident.
#[derive(Resource, Default)]
struct MinimapTileCache {
    map_id: Option<u32>,
    tiles: HashMap<(u32, u32), Option<Handle<Image>>>,
    interior_stem: Option<String>,
    interior: HashMap<(usize, u32, u32), Option<Handle<Image>>>,
}

/// Load the translate catalog and the minimap art once the patch chain is open.
fn setup_minimap(
    mut commands: Commands,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut assets) = world_assets else {
        return;
    };
    let translate = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_minimap_translate(&mut chain)
    };
    match translate {
        Ok(translate) => {
            info!("minimap: md5translate.trs — {} tiles", translate.len());
            let mask = assets.mask_texture("Textures\\MinimapMask", &mut images);
            let arrow = assets.sprite_texture(blips::PLAYER_ARROW_TEXTURE, &mut images);
            let poi = assets.sprite_texture("Interface\\Minimap\\POIIcons", &mut images);
            let mut rim_arrows = blips::RimArrowArt::default();
            for kind in blips::RimArrow::ALL {
                rim_arrows.set(kind, assets.sprite_texture(kind.texture(), &mut images));
            }
            let object_icons =
                assets.sprite_texture("Interface\\Minimap\\ObjectIcons", &mut images);
            if mask.is_none() {
                warn!("minimap: MinimapMask.blp missing — the map will draw square");
            }
            let forms = {
                let mut chain = assets.chain.lock_recover();
                match benilla_formats::load_shapeshift_forms(&mut chain) {
                    Ok(f) => Some(f),
                    Err(e) => {
                        warn!("minimap: SpellShapeshiftForm.dbc failed — no shapeshift creature-type override: {e:#}");
                        None
                    }
                }
            };
            commands.insert_resource(MinimapAssets {
                translate,
                mask,
                arrow,
                poi,
                rim_arrows,
                object_icons,
                forms,
            });
        }
        Err(e) => error!("minimap: md5translate.trs failed, minimap disabled: {e:#}"),
    }
}

/// The minimap's one containment verdict: the placement, model, tile stem and seed group of the
/// WMO the player is in, or `None` for terrain. Indoors the reference draws the building's own
/// tiles and no terrain, and the same flag (`0xceaa60`) picks the zoom index.
///
/// The gate is the reference's indoor byte (`0xbc8300`), the CGLight node's down-ray bit
/// `[node+0x90] & 1` (`0x670547`): the nearest face within 1000 yd straight down, terrain racing
/// and the WMO winning ties, belongs to a group without MOGP `0x8`. That is the zone-text claim
/// `CurrentAreaInterior`, read through `world.area_interior()`; [`down_ray_seeds`] only supplies
/// the flood seed once the gate says indoors.
fn minimap_interior<'a>(
    player: &Player,
    instances: &Query<&WmoPortalInstance>,
    wmos: &'a Assets<WmoModel>,
    world: &benilla_world::world_point::WorldPoint,
    asset_server: &AssetServer,
) -> Option<(Affine3A, &'a WmoModel, String, usize)> {
    if !player.active || player.detached || world.area_interior().is_none() {
        return None;
    }
    let eye = player.pos + Vec3::Y * INTERIOR_PROBE_HEIGHT;
    // The down-ray races the terrain, as the zone tracker does: grass above a mine is not in it.
    let terrain = world.terrain_height_under(eye);
    instances.iter().find_map(|inst| {
        let model = wmos.get(&inst.handle)?;
        if model.wmo_id == 0 {
            return None;
        }
        let local_from_world = inst.world_from_local.inverse();
        let eye_local = bevy_to_wow(local_from_world.transform_point3(eye));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, eye, z));
        let in_group = down_ray_seeds(model, eye_local, terrain_local).in_group?;
        let stem = asset_server
            .get_path(inst.handle.id())
            .and_then(|p| wmo_minimap_stem(&p.path().to_string_lossy()))?;
        Some((inst.world_from_local, model, stem, in_group))
    })
}

/// Fill the widget hole: the tiles masked to the circle, then the blips and the player arrow, at
/// the widget's z (a stable sort keeps append order, so all sit below the widget's children).
fn emit_minimap(
    widget: Res<MinimapWidget>,
    assets: Option<Res<MinimapAssets>>,
    mut cache: ResMut<MinimapTileCache>,
    map: Option<Res<CurrentMap>>,
    catalog: Option<Res<MapCatalogRes>>,
    player: Res<Player>,
    lighting: Option<Res<benilla_world::lighting::WowLighting>>,
    instances: Query<&WmoPortalInstance>,
    wmos: Res<Assets<WmoModel>>,
    world: benilla_world::world_point::WorldPoint,
    asset_server: Res<AssetServer>,
    death_net: Res<crate::death::DeathNet>,
    blip_inputs: blips::BlipInputs,
    mut composite: ResMut<composite::MinimapComposite>,
    rig: Option<Res<composite::CompositeRig>>,
    mut quads: ResMut<UiQuads>,
) {
    let (
        quest,
        guids,
        unit_pos,
        window,
        mut blip_hover,
        ui_scale,
        group,
        tracked,
        self_store,
        names,
        go_templates,
        locks,
        poi_marker,
        pois,
        mut ping,
        script,
    ) = blip_inputs;
    // Hover resets every frame; the blip pass re-establishes it while the map draws.
    *blip_hover = blips::MinimapBlipHover::None;
    // Drain the `Minimap:PingLocation` click before any early return: it is spent in the frame it
    // was made, and a click on a frame the map does not draw is not a ping.
    let click = script.and_then(|mut s| s.take_minimap_ping_request());
    let (Some(slot), Some(assets), Some(map), Some(catalog)) =
        (widget.0.as_ref(), assets, map, catalog)
    else {
        return;
    };

    let side = slot.rect.width().min(slot.rect.height());
    if side <= 0.0 {
        return;
    }
    // The composite is off unless the interior branch below turns it on this frame.
    composite.active = false;
    let Some(rt_image) = rig.map(|r| r.image.clone()) else {
        return; // the composite rig's Startup system has not run yet
    };
    // `WOW_MM_ZOOM=0..5` forces both zoom indices, a capture instrument.
    static ZOOM_OVERRIDE: std::sync::OnceLock<Option<u8>> = std::sync::OnceLock::new();
    let zoom_override = *ZOOM_OVERRIDE.get_or_init(|| {
        std::env::var("WOW_MM_ZOOM")
            .ok()
            .and_then(|s| s.parse::<u8>().ok())
    });
    let zoom = zoom_override.unwrap_or(slot.zoom);
    let inside_zoom = zoom_override.unwrap_or(slot.inside_zoom);
    let center = (slot.rect.min + slot.rect.max) * 0.5;

    let wow = bevy_to_wow(player.pos);
    let (wx, wy) = (wow[0], wow[1]);
    // The drawn family's world-to-px scale, for the point blips drawn after the tiles.
    let mut blip_px_per_yd = 0.0_f32;

    let mask = assets.mask.as_ref().map(|m| UiQuadMask {
        texture: m.clone(),
        rect: slot.rect,
    });

    let interior = minimap_interior(&player, &instances, &wmos, &world, &asset_server);

    // For the dots' indoor test; the branch below consumes `interior`.
    let player_indoors = interior.is_some();
    let view_radius = view_radius_yd(zoom, inside_zoom, player_indoors);
    if let Some((world_from_local, model, stem, in_group)) = interior {
        // Interior: the WMO's own per-group tiles, drawn white. They are baked in the model frame
        // (north = model +X), so each sits at its model-space centre through the placement and the
        // whole set turns by one placement yaw (`0x6da180`).
        if cache.interior_stem.as_deref() != Some(stem.as_str()) {
            cache.interior.clear();
            cache.interior_stem = Some(stem.clone());
        }
        let radius = view_radius;
        let px_per_yd = (side * 0.5) / radius;
        blip_px_per_yd = px_per_yd;

        // The tiles go into the reference's 256² target (`composite`), not the screen. Target
        // space: y-up, origin at the player, `RT_HALF_EXTENT_SCALE · radius` yd to an edge.
        #[allow(clippy::cast_precision_loss)] // 256 is exact in f32
        let units_per_yd =
            (composite::RT_SIZE as f32 * 0.5) / (composite::RT_HALF_EXTENT_SCALE * radius);
        // A model point to its north-up target position: placement to world, then up = +X north,
        // left = +Y west.
        let to_target = |m: [f32; 3]| {
            let w = bevy_to_wow(world_from_local.transform_point3(wow_to_bevy(m)));
            Vec2::new((wy - w[1]) * units_per_yd, (w[0] - wx) * units_per_yd)
        };
        // The one placement rotation: where model +X points, clockwise on screen (negated into the
        // target's y-up frame at the Transform).
        let x_axis = to_target([1.0, 0.0, 0.0]) - to_target([0.0, 0.0, 0.0]);
        let rotation = (-x_axis.y).atan2(x_axis.x);
        #[allow(clippy::cast_precision_loss)]
        let rt_half = composite::RT_SIZE as f32 * 0.5;
        composite.active = true;

        // The portal flood-fill from the player's group (`0x6a5020`). Its query box uses the draw
        // radius (`0x6d96b6`), so zooming in also tightens the box's Z extent.
        let drawable =
            interior_group_selection(model, &world_from_local, player.pos, radius, in_group);
        // Draw order (`0x4ebeb0`, keyed at `0x6d9e30`): ascending by `Zmidpoint − playerZ`, the
        // player's own group keyed `FLT_MAX`, so it draws last and no storey covers the room.
        let player_z = bevy_to_wow(world_from_local.inverse().transform_point3(player.pos))[2];
        let sort_key = |gi: usize| -> f32 {
            if gi == in_group {
                f32::MAX
            } else {
                let gn = &model.group_nav[gi];
                0.5 * (gn.bbox_min[2] + gn.bbox_max[2]) - player_z
            }
        };
        let mut order: Vec<usize> = (0..model.group_nav.len())
            .filter(|&gi| drawable[gi])
            .collect();
        order.sort_by(|&a, &b| sort_key(a).total_cmp(&sort_key(b)));
        for gi in order {
            let gn = &model.group_nav[gi];
            let (nx, tw_x) = group_axis_grid(gn.bbox_max[0] - gn.bbox_min[0]);
            let (ny, tw_y) = group_axis_grid(gn.bbox_max[1] - gn.bbox_min[1]);
            let mid_z = 0.5 * (gn.bbox_min[2] + gn.bbox_max[2]);
            for col in 0..nx {
                for row in 0..ny {
                    // The grid cell plus the outer-edge bleed: a boundary cell grows 1 yd on its
                    // outer side (`0x6a549e`/`0x6a54ae`/`0x6a54be`/`0x6a54d1`, each an `fsub` or
                    // `fadd` of `0xca8098`). The bleed matters: `GEQUAL 224/255` on a linear-
                    // filtered edge reaches only 0.122 texel past the last opaque texel centre, so
                    // merely abutting groups would show the black clear along the wall.
                    let x0 = gn.bbox_min[0] + col as f32 * tw_x
                        - if col == 0 { EDGE_BLEED_YD } else { 0.0 };
                    let x1 = gn.bbox_min[0]
                        + (col + 1) as f32 * tw_x
                        + if col + 1 == nx { EDGE_BLEED_YD } else { 0.0 };
                    let y0 = gn.bbox_min[1] + row as f32 * tw_y
                        - if row == 0 { EDGE_BLEED_YD } else { 0.0 };
                    let y1 = gn.bbox_min[1]
                        + (row + 1) as f32 * tw_y
                        + if row + 1 == ny { EDGE_BLEED_YD } else { 0.0 };
                    let tc = to_target([0.5 * (x0 + x1), 0.5 * (y0 + y1), mid_z]);
                    // Cull against the target, which holds 1.5× the visible disc.
                    if tc.length() > rt_half + tw_x.max(tw_y) * units_per_yd {
                        continue;
                    }
                    let handle = cache.interior.entry((gi, col, row)).or_insert_with(|| {
                        let key = format!("{stem}_{gi:03}_{col:02}_{row:02}.blp");
                        assets.translate.get(&key).map(load_tile(&asset_server))
                    });
                    let Some(handle) = handle else {
                        continue; // this group cell has no authored tile
                    };
                    let order = composite.tiles.len();
                    composite.tiles.push(composite::CompositeTile {
                        texture: handle.clone(),
                        center: tc,
                        // The reference's half-texel UV inset, `[0.5/W, 1−0.5/W]`, as the equal
                        // quad scale `W/(W−1)`, so the shared mesh keeps its UVs. `W` comes from
                        // the tile (`tw / 0.5`), never the bled rect.
                        size: Vec2::new(
                            (x1 - x0) * units_per_yd * texel_stretch(tw_x),
                            (y1 - y0) * units_per_yd * texel_stretch(tw_y),
                        ),
                        rotation,
                        order,
                    });
                }
            }
        }

        // `WOW_MM_STATS=1` prints the kept groups and tiles; the reference's Stormwind capture
        // emitted 57 tiles at indoor zoom 3.
        static MM_STATS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *MM_STATS.get_or_init(|| std::env::var_os("WOW_MM_STATS").is_some()) {
            eprintln!(
                "MM-STATS: radius {radius} yd, groups {}/{} selected, {} tiles composited",
                drawable.iter().filter(|d| **d).count(),
                drawable.len(),
                composite.tiles.len(),
            );
        }

        // The blit: the target's middle two-thirds (`0x4ec440`, EGxBlend 2), masked to the circle
        // here and not on the tiles, as the reference does. The opaque target is also the backing.
        let lo = 0.5 - composite::RT_BLIT_FRACTION * 0.5;
        let hi = 0.5 + composite::RT_BLIT_FRACTION * 0.5;
        quads.overlays.push(UiQuad {
            rect: Rect::from_center_size(center, Vec2::splat(side)),
            z_key: slot.z,
            texture: Some(rt_image),
            uv: UvRect::from_tex_coords([lo, hi, lo, hi]),
            color: [1.0, 1.0, 1.0, slot.alpha],
            mask: mask.clone(),
            ..default()
        });
    } else if let Some(dir) = catalog.0.directory(map.0) {
        // Outdoor: the ADT tiles, modulated by the day-night tint; white without lighting.
        let half_extent = view_radius;
        let px_per_yd = (side * 0.5) / half_extent;
        blip_px_per_yd = px_per_yd;
        if cache.map_id != Some(map.0) {
            cache.tiles.clear();
            cache.map_id = Some(map.0);
        }
        let tint = lighting
            .as_ref()
            .map(|l| minimap_day_tint(l.ambient, l.diffuse))
            .unwrap_or([1.0, 1.0, 1.0]);
        // World coords shrink as tile indices grow, so the view square's max-corner gives the low
        // indices. `world_to_tile` clamps to the 64×64 grid.
        let (tx_lo, ty_lo) = world_to_tile(wx + half_extent, wy + half_extent);
        let (tx_hi, ty_hi) = world_to_tile(wx - half_extent, wy - half_extent);
        for ty in ty_lo..=ty_hi {
            for tx in tx_lo..=tx_hi {
                let handle = cache.tiles.entry((tx, ty)).or_insert_with(|| {
                    assets
                        .translate
                        .tile(dir, tx, ty)
                        .map(load_tile(&asset_server))
                });
                let Some(handle) = handle else {
                    continue; // unauthored tile (open ocean): the clear colour shows
                };
                // The tile's max-x/max-y world corner is its north-west corner = screen top-left.
                let (tile_north, tile_west) = tile_to_world(tx, ty);
                let left = center.x + (wy - tile_west) * px_per_yd;
                let top = center.y + (tile_north - wx) * -px_per_yd;
                let size = TILE_YARDS * px_per_yd;
                let rect = Rect::new(left, top, left + size, top + size);
                if rect.intersect(slot.rect).is_empty() {
                    continue;
                }
                quads.overlays.push(UiQuad {
                    rect,
                    z_key: slot.z,
                    texture: Some(handle.clone()),
                    color: [tint[0], tint[1], tint[2], slot.alpha],
                    // A skip-decode tile (`load_tile`): the tint modulates the authored byte, as
                    // the reference's fixed-function stage does; the ordinary arm would re-encode.
                    gamma_texel: true,
                    // No CPU clip: the mask shader zeroes everything outside `mask_rect`, and an
                    // unclipped panning tile is a pure translation that never rewrites its mesh.
                    mask: mask.clone(),
                    ..default()
                });
            }
        }
    }

    // ── The blip layer: landmarks under the player arrow, the dots above it (the reference's
    // order).
    // Our own descriptor's tracking state (private fields, only on the self entity).
    let me = self_store.iter().next();
    // Our own guid: our own pet and minions never take a dot (`0x4eac0d`).
    let self_guid = me.map(|(_, g)| g.0);
    let tracking = me
        .map(|(s, _)| blips::SelfTracking {
            creatures: s.0.player_track_creatures(),
            resources: s.0.player_track_resources(),
            stealthed: s.0.player_track_stealthed(),
        })
        .unwrap_or_default();
    let blip_ctx = (blip_px_per_yd > 0.0).then(|| {
        static BLIP_PROBE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *BLIP_PROBE.get_or_init(|| std::env::var_os("WOW_MM_BLIP_PROBE").is_some()) {
            eprintln!(
                "BLIP-PROBE: arrow_art={} pois={} map={} wx={wx:.0} wy={wy:.0} px_per_yd={blip_px_per_yd:.3} track_c={:#x} track_r={:#x} track_s={}",
                blips::RimArrow::ALL
                    .iter()
                    .filter(|k| assets.rim_arrows.get(**k).is_some())
                    .count(),
                pois.as_ref().map(|c| c.0.len()).unwrap_or(0),
                map.0,
                tracking.creatures,
                tracking.resources,
                tracking.stealthed,
            );
        }
        let win = window.iter().next();
        let cursor = win.and_then(|w| w.cursor_position());
        let seam = win.map_or(1.0, |w| crate::ui_script::seam_scale(w.height(), ui_scale.0));
        // The pan origin snapped to a half-logical-pixel grid: every blip offset and rim bearing
        // derives from `wx`/`wy`, so the layer steps together instead of rewriting the batch mesh
        // every frame while walking. Blip world positions stay exact.
        let q = 0.5 / blip_px_per_yd;
        blips::BlipCtx {
            center,
            side,
            px_per_yd: blip_px_per_yd,
            radius_yd: (side * 0.5) / blip_px_per_yd,
            z: slot.z,
            alpha: slot.alpha,
            wx: (wx / q).round() * q,
            wy: (wy / q).round() * q,
            wz: wow[2],
            cursor,
            // The cursor in UI space (y-up, ÷ the seam scale): the tooltip anchor resolves in
            // 768-virtual units, not window px.
            cursor_ui: cursor
                .zip(win)
                .map(|(c, w)| Vec2::new(c.x / seam, (w.height() - c.y) / seam)),
            seam,
        }
    });
    let mut hover = blips::MinimapBlipHover::None;
    if let Some(ctx) = &blip_ctx {
        // The guard-directions marker rides this pass as a landmark, as the reference appends its
        // static blip slot after the DBC scan, so it draws even without `AreaPOI.dbc`.
        if assets.rim_arrows.any() {
            blips::emit_landmarks(
                ctx,
                pois.as_ref().map(|p| &p.0),
                poi_marker.on_map(map.0),
                map.0,
                &assets.rim_arrows,
                assets.poi.as_ref(),
                &mut quads,
                &mut hover,
            );
            // The party and corpse rim arrows (the out-of-range half of `0x6dad10`), before the
            // player arrow, in the reference's order.
            let corpse = death_net
                .corpse
                .filter(|cp| cp.display_map == map.0 as i32)
                .map(|cp| cp.position);
            blips::emit_party_arrows(
                ctx,
                &group,
                &guids,
                &unit_pos,
                corpse,
                &assets.rim_arrows,
                &mut quads,
            );
        }
    }

    // The player arrow: centered, spun to the facing. WoW orientation 0 = north (screen up),
    // growing counterclockwise (toward west = screen left); our quad rotation is clockwise on
    // screen, so the arrow angle is the negated facing.
    if let Some(arrow) = &assets.arrow {
        // `blips::PLAYER_ARROW_QUAD_PX`: MinimapArrow.m2's single quad at 1280 px/unit on the
        // 140.8 basis, its authored centre offset rotating with the facing.
        let s = side * (blips::PLAYER_ARROW_QUAD_PX / blips::BLIP_BASIS_PX);
        let rotation = -player.facing();
        let (sin, cos) = rotation.sin_cos();
        let off = blips::PLAYER_ARROW_OFFSET_PX * (side / blips::BLIP_BASIS_PX);
        let off = Vec2::new(off.x * cos - off.y * sin, off.x * sin + off.y * cos);
        let rect = Rect::from_center_size(center + off, Vec2::splat(s));
        quads.overlays.push(UiQuad {
            rect,
            z_key: slot.z,
            texture: Some(arrow.clone()),
            color: [1.0, 1.0, 1.0, slot.alpha],
            rotation,
            ..default()
        });
    }

    // The object dots draw last, above the player arrow (`0x4ed7b7`): tracking dots (cells 0/1),
    // then quest (3) and party (4) dots.
    if let Some(ctx) = &blip_ctx {
        if let Some(icons) = &assets.object_icons {
            blips::emit_tracking_dots(
                ctx,
                tracking,
                &tracked,
                quest.statuses(),
                self_guid,
                &names,
                &go_templates,
                locks.as_deref().map(|l| &l.0),
                assets.forms.as_ref(),
                icons,
                player_indoors,
                |feet| world.indoors_at(feet),
                &mut quads,
                &mut hover,
            );
            blips::emit_quest_dots(
                ctx,
                quest.statuses(),
                &tracked,
                self_guid,
                icons,
                player_indoors,
                // A dot NPC's own containment, the faces-only down-ray the entity lights use.
                |feet| world.indoors_at(feet),
                &mut quads,
                &mut hover,
            );
            // The in-range party dots (blue cell 4, 1.3×), last.
            blips::emit_party_dots(ctx, &group, &guids, &unit_pos, icons, &mut quads);
        }
    }
    *blip_hover = hover;

    // The corpse blip in range: the POIIcons skull, the reference's world-map corpse art (its
    // in-range minimap art is untraced). Out of range it is the rim arrow above, the fifth slot
    // of `0x6dad10`. Same map only: a dungeon corpse's display coords are the entrance.
    if let (Some(poi), Some(cp)) = (&assets.poi, death_net.corpse) {
        if cp.display_map == map.0 as i32 && blip_px_per_yd > 0.0 {
            let off = Vec2::new(
                (wy - cp.position[1]) * blip_px_per_yd,
                -(cp.position[0] - wx) * blip_px_per_yd,
            );
            if off.length() <= side * 0.5 * 0.8 {
                let s = side * CORPSE_BLIP_FRACTION;
                let rect = Rect::from_center_size(center + off, Vec2::splat(s));
                quads.overlays.push(UiQuad {
                    rect,
                    z_key: slot.z,
                    texture: Some(poi.clone()),
                    uv: crate::ui_pass::UvRect::from_tex_coords([0.875, 1.0, 0.0, 0.125]),
                    color: [1.0, 1.0, 1.0, slot.alpha],
                    ..default()
                });
            }
        }
    }

    // Seat the click against this frame's geometry; the marker is the stock `MiniMapPing`
    // `<Model>`, drawn over the widget by `crate::ui_models`.
    if let Some(ctx) = &blip_ctx {
        ping::seat_click(ctx, &mut ping, click);
    }
}

/// Push the player's WMO containment onto the Minimap widget (`0xceaa60`), so the zoom buttons
/// drive the indoor index indoors and the outdoor one outside; the verdict is [`emit_minimap`]'s,
/// as the reference keeps one flag. Pushed on the edge and whenever the VM's Minimap-creation
/// count moves, so a widget built later is still told. The edge also fires `MINIMAP_UPDATE_ZOOM`,
/// on which the stock `Minimap_OnEvent` re-syncs the +/- buttons to the other index's level.
fn feed_minimap_inside(
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    world: benilla_world::world_point::WorldPoint,
    player: Res<Player>,
    instances: Query<&WmoPortalInstance>,
    wmos: Res<Assets<WmoModel>>,
    asset_server: Res<AssetServer>,
    mut inside_res: ResMut<MinimapInside>,
    mut was_inside: Local<crate::ui_script::VmMemo<Option<bool>>>,
    mut pushed_at: Local<crate::ui_script::VmMemo<u64>>,
) {
    let inside = minimap_interior(&player, &instances, &wmos, &world, &asset_server).is_some();
    inside_res.0 = inside;
    let Some(mut script) = script else { return };
    let edge = *was_inside.get(&script) != Some(inside);
    let created = script.minimap_widgets_created();
    if edge || *pushed_at.get(&script) != created {
        script.set_minimap_inside(inside);
        *pushed_at.get(&script) = created;
    }
    if edge {
        *was_inside.get(&script) = Some(inside);
        script.fire_event("MINIMAP_UPDATE_ZOOM", vec![]);
    }
}

/// Swap the disc's mask for the one `Minimap:SetMaskTexture` names (a 1.12 method). Memoised on the
/// path and re-read with a fresh VM; a path that fails to load keeps the current mask and warns.
fn feed_minimap_mask(
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    assets: Option<ResMut<MinimapAssets>>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut last: Local<crate::ui_script::VmMemo<Option<String>>>,
) {
    let (Some(script), Some(mut assets), Some(mut world_assets)) = (script, assets, world_assets)
    else {
        return;
    };
    let want = script.minimap_mask_texture();
    let last = last.get(&script);
    if *last == want {
        return;
    }
    *last = want.clone();
    let path = want
        .as_deref()
        .unwrap_or(benilla_ui::widget::MINIMAP_DEFAULT_MASK);
    match world_assets.mask_texture(path, &mut images) {
        Some(handle) => assets.mask = Some(handle),
        None => {
            warn!("minimap: SetMaskTexture(\"{path}\") — no such texture; keeping the current mask")
        }
    }
}

/// Push the player's facing onto the Minimap's player-arrow `Model` every frame, as the reference's
/// per-frame `CMinimap::SetPlayerFacing` (`0x4eb8e0`) writes `[+0x39c]`, what `Model:GetFacing()`
/// reads. `player.facing()` is an unbounded accumulator, so it is normalized to `[0, 2π)`, the
/// wire value vmangos's `VerifyMovementInfo` requires.
fn feed_minimap_player_facing(
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    player: Res<Player>,
) {
    let Some(mut script) = script else { return };
    script.set_minimap_player_facing(player.facing().rem_euclid(std::f32::consts::TAU));
}

/// Push the game clock into `GetGameTime()`'s backing globals when the minute ticks, the API's own
/// resolution. Before the first `SMSG_LOGIN_SETTIMESPEED` they hold 0:00.
fn feed_game_time(
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    time: Res<crate::net::ServerTime>,
    mut last: Local<crate::ui_script::VmMemo<Option<u32>>>,
) {
    let Some(script) = script else { return };
    let last = last.get(&script);
    let Some(gt) = time.0 else { return };
    let minute = gt.minute_of_day();
    if *last == Some(minute) {
        return;
    }
    *last = Some(minute);
    let globals = script.lua().globals();
    let pushed = globals
        .set("__benilla_game_hour", minute / 60)
        .and_then(|()| globals.set("__benilla_game_minute", minute % 60));
    if let Err(e) = pushed {
        warn!("minimap: game-time globals: {e}");
    }
}

/// The app half of the `<Minimap>` widget. The zone label feed is `crate::area`'s: the reference
/// fires `MINIMAP_ZONE_CHANGED` from the same area-update pass as the `ZONE_CHANGED` family
/// (`0x494970` beside `0x494780`).
pub(crate) struct MinimapPlugin;

impl Plugin for MinimapPlugin {
    fn build(&self, app: &mut App) {
        ping::register(app);
        app.add_observer(on_cvar);
        app.init_resource::<MinimapWidget>()
            .init_resource::<MinimapZoom>()
            .init_resource::<MinimapTileCache>()
            .init_resource::<blips::MinimapBlipHover>()
            .init_resource::<MinimapInside>()
            .init_resource::<MinimapPing>()
            .init_resource::<composite::MinimapComposite>()
            .add_systems(Startup, setup_minimap.after(AssetSet::Open))
            .add_systems(Startup, composite::setup_composite)
            .add_systems(
                Update,
                (
                    // The headless probe's synthetic widget, ahead of the emit that reads it.
                    probe_minimap_widget
                        .in_set(UiQuadAppend)
                        .before(emit_minimap),
                    // Not in `LightingConsumeSet` though it reads `WowLighting`: at most one frame
                    // of late tint on a 140 px map, not worth a world API item.
                    emit_minimap.in_set(UiQuadAppend),
                    // After the emit that fills it, so the blit never samples a frame-old target.
                    composite::drive_composite.after(UiQuadAppend),
                    // Before the script tick, so a zoom button routes to the live index.
                    feed_minimap_inside.in_set(crate::ui_script::UiFeed),
                    // Before the script tick, so an addon's OnUpdate reads this frame's heading.
                    feed_minimap_player_facing.in_set(crate::ui_script::UiFeed),
                    // After the tick that can set the mask, before the emit that draws with it.
                    feed_minimap_mask
                        .after(crate::ui_script::UiInput)
                        .before(UiQuadAppend),
                    // Before the script tick, after the containment verdict it reads. Gated on the
                    // in-game UI: a member's ping in the login burst would otherwise spend its
                    // `fresh` latch on the boot VM, which has no `MiniMapPing` frame.
                    ping::drive_minimap_ping
                        .after(feed_minimap_inside)
                        .in_set(crate::ui_script::UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    // Before the script tick, so GameTimeFrame reads this frame's minute.
                    feed_game_time.in_set(crate::ui_script::UiFeed),
                    // After the world-mouseover drive (UnitFeed): a same-frame world-hover→blip
                    // transition must end with the blip tooltip shown, not the fade.
                    blips::drive_blip_tooltip
                        .after(crate::ui_unit::UnitFeed)
                        .in_set(crate::ui_script::UiFeed),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::minimap_day_tint;

    fn approx(a: [f32; 3], b: [f32; 3]) {
        for c in 0..3 {
            assert!((a[c] - b[c]).abs() < 0.01, "{a:?} vs {b:?} @ {c}");
        }
    }

    #[test]
    fn full_white_light_leaves_tiles_at_full_brightness() {
        // Both bands white ⇒ tint white ⇒ the tile draws verbatim (the noon-ish bright case).
        approx(minimap_day_tint([1.0; 3], [1.0; 3]), [1.0; 3]);
    }

    #[test]
    fn default_light_dims_tiles_below_white() {
        // The reference's no-light default, ambient grey 0x40 (≈0.251) and diffuse white:
        // 0.25·1 + 0.75·lerp(0.251, 1, (64+96)/256=0.625) = 0.25 + 0.75·0.719 ≈ 0.789.
        let t = minimap_day_tint([0.251; 3], [1.0; 3]);
        approx(t, [0.789; 3]);
        assert!(t[0] < 0.95, "flat white would be too bright");
    }

    #[test]
    fn pitch_black_light_still_tints_partway_to_white() {
        // The +96 luma floor keeps the map dimly visible with no light: 0.75·(0+1·0.375) ≈ 0.28.
        approx(minimap_day_tint([0.0; 3], [0.0; 3]), [0.281; 3]);
    }
}
