//! The HUD minimap renderer, the app half of the `<Minimap>` widget: [`emit_minimap`] fills the
//! widget's `QuadContent::Minimap` hole with the tiles around the player, masked to
//! `MinimapMask.blp`, and the player arrow on top; the widget's FrameXML children draw above.
//!
//! - Tiles: one 256² BLP per ADT tile (533.33 yd), `map<X>_<Y>.blp` resolved through
//!   `md5translate.trs` ([`benilla_formats::MinimapTranslate`]), indexed in ADT order.
//! - Zoom to radius (`0x6da9b0`): two zoom indices, picked by WMO containment, each persisted
//!   (`minimapZoom`, `minimapInsideZoom`); outdoors a chunk-count half-extent, indoors a yard
//!   table ([`benilla_ui::widget::minimap_radius_yd`]).
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

use benilla_ui::widget::{MinimapArt, MinimapArtSlot, MINIMAP_DEFAULT_ZOOM};

use crate::player::Player;
use crate::ui_pass::{UiQuad, UiQuadAppend, UiQuadMask, UiQuads, UvRect};
use benilla_assets::MapCatalogRes;
use benilla_assets::{AssetSet, LockRecover, LooseOpen, WorldAssets};
use benilla_world::wmo_portal::{
    down_ray_seeds, terrain_z_local, WmoPortalInstance, INTERIOR_PROBE_HEIGHT,
};
use benilla_world::world_map::CurrentMap;

/// Yards per ADT tile.
const TILE_YARDS: f32 = 533.333_3;

/// How far the map reaches, in yards (`0x6da9b0`), for both branches of [`emit_minimap`]. Indoors
/// the on-screen radius is exactly the table's (the composite at `1.5·c`, `0x4ec090`, blits its
/// middle two-thirds, `0x4ec440`), and the same `c` sizes the selection box (`0x6d96b6`); the
/// `10.0f` at `0x4ed9eb` is only the field's static initializer, which `0x6d98f5` rewrites from
/// the table whenever the zoom or indoor state changes.
fn view_radius_yd(zoom: u32, inside_zoom: u32, inside: bool) -> f32 {
    benilla_ui::widget::minimap_radius_yd(zoom, inside_zoom, inside)
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
        z: u128::MAX / 2,
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
    pub(crate) outdoor: u32,
    /// `minimapInsideZoom`, the indoor index, persisted apart from the outdoor level.
    pub(crate) inside: u32,
}

/// The app's mirror of the two zoom CVars, each on its own field: the record's integer as the
/// reference's raw copy takes it into the zoom cell (`0x6d9013`), unclamped.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut zoom: ResMut<MinimapZoom>) {
    match ev.key().as_str() {
        "minimapzoom" => zoom.outdoor = zoom_index(&ev.new),
        "minimapinsidezoom" => zoom.inside = zoom_index(&ev.new),
        _ => {}
    }
}

/// A stored zoom level as its cell holds it: the record's integer (`rec+0x28`, `SStrToInt` at
/// `0x63e127`) as an unsigned 32-bit value.
fn zoom_index(text: &str) -> u32 {
    benilla_ui::script::s_str_to_int(text) as u32
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
    pub(crate) z: u128,
    /// The outdoor zoom index; `inside_zoom` is the indoor one, picked by WMO containment.
    pub(crate) zoom: u32,
    pub(crate) inside_zoom: u32,
    pub(crate) alpha: f32,
}

/// The loaded minimap fixtures; absent, the map draws nothing and its XML children still render.
#[derive(Resource, Default)]
struct MinimapAssets {
    translate: MinimapTranslate,
    /// The three process-wide textures by [`MinimapArtSlot`]: the disc's mask (`0xbc7968`), the
    /// landmark atlas `Interface\Minimap\POIIcons` (`0xbc827c`) and the dot atlas
    /// `Interface\Minimap\ObjectIcons` (`0xbc822c`). No VM owns them, so a new VM inherits them.
    art: [MinimapTex; 3],
    /// `SpellShapeshiftForm.dbc`, the tracking dots' creature-type override (a cat-form druid is a
    /// Beast).
    forms: Option<HashMap<u32, benilla_formats::ShapeshiftForm>>,
}

/// One of the minimap's process-wide textures as drawn.
#[derive(Default, Clone)]
struct MinimapTex {
    /// The handle the draw uses; `None` is the null handle an empty name leaves.
    current: Option<Handle<Image>>,
    /// What every world entry loads (`0x4ed810`): the default file, else the green solid.
    default: Option<Handle<Image>>,
    /// The 8×8 solid `0xff00ff00` a name that does not load gives (`[0x90582c]`).
    missing: Option<Handle<Image>>,
}

impl MinimapAssets {
    /// The texture drawn for `slot`, `None` for a null handle.
    fn tex(&self, slot: MinimapArtSlot) -> Option<&Handle<Image>> {
        self.art[slot as usize].current.as_ref()
    }
}

/// `TextureCreate`'s green placeholder, `0xff00ff00` (`0x447c00`), as an 8×8 image (`0x44a900`).
fn green_solid(mask: bool) -> Image {
    let rgba = [0u8, 255, 0, 255].repeat(64);
    if mask {
        benilla_assets::mask_image(8, 8, rgba)
    } else {
        benilla_assets::sprite_image(8, 8, rgba)
    }
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
            // The default mask and icons open from the archives alone, as `0x4ed810`'s
            // `TextureCreate` calls pass file flag 0 (`0x4ed862`, `0x4ed88b`, `0x4ed8b4`).
            let archives = LooseOpen::Archives;
            // The three defaults (`0x84c6e4`, `0x84c6c8`, `0x84c6a8`); one that does not load is
            // the green solid, as `TextureCreate` gives it.
            let mut art: [MinimapTex; 3] = Default::default();
            for slot in [
                MinimapArtSlot::Mask,
                MinimapArtSlot::Icons,
                MinimapArtSlot::Blips,
            ] {
                let is_mask = slot == MinimapArtSlot::Mask;
                let missing = images.add(green_solid(is_mask));
                let path = default_art_path(slot);
                let loaded = if is_mask {
                    assets.mask_texture(path, archives, &mut images)
                } else {
                    assets.sprite_texture_opened(path, archives, &mut images)
                };
                if loaded.is_none() {
                    warn!("minimap: {path} missing — the green placeholder stands in");
                }
                let default = Some(loaded.unwrap_or_else(|| missing.clone()));
                art[slot as usize] = MinimapTex {
                    current: default.clone(),
                    default,
                    missing: Some(missing),
                };
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
                art,
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
    corpse: Res<MinimapCorpse>,
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
        group,
        tracked,
        self_store,
        names,
        go_templates,
        locks,
        poi_marker,
        pois,
    ) = blip_inputs;
    // Hover resets every frame; the blip pass re-establishes it while the map draws.
    *blip_hover = blips::MinimapBlipHover::None;
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
    static ZOOM_OVERRIDE: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    let zoom_override = *ZOOM_OVERRIDE.get_or_init(|| {
        std::env::var("WOW_MM_ZOOM")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
    });
    let zoom = zoom_override.unwrap_or(slot.zoom);
    let inside_zoom = zoom_override.unwrap_or(slot.inside_zoom);
    let center = (slot.rect.min + slot.rect.max) * 0.5;

    let wow = bevy_to_wow(player.pos);
    let (wx, wy) = (wow[0], wow[1]);
    // The drawn family's world-to-px scale, for the point blips drawn after the tiles.
    let mut blip_px_per_yd = 0.0_f32;

    let (mask, tile_mask) = stage_masks(assets.tex(MinimapArtSlot::Mask), slot.rect);

    let interior = minimap_interior(&player, &instances, &wmos, &world, &asset_server);

    // For the dots' indoor test; the branch below consumes `interior`.
    let player_indoors = interior.is_some();
    let view_radius = view_radius_yd(zoom, inside_zoom, player_indoors);
    if assets.tex(MinimapArtSlot::Mask).is_none() {
        // An empty mask name leaves no texture, and the render returns at once (`0x4ec959`,
        // `0x4ec968`): no tiles, and with no scale no landmarks, rim arrows or dots either.
    } else if let Some((world_from_local, model, stem, in_group)) = interior {
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
                    mask: tile_mask.clone(),
                    ..default()
                });
            }
        }
    }

    // ── The blip layer: the landmark icons, then the dots, last in the render (`0x4ed7b7`). The
    // arrows are the minimap's engine children, drawn at their own levels above all of it
    // ([`feed_minimap_arrows`]).
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
                "BLIP-PROBE: pois={} map={} wx={wx:.0} wy={wy:.0} px_per_yd={blip_px_per_yd:.3} track_c={:#x} track_r={:#x} track_s={}",
                pois.as_ref().map(|c| c.0.len()).unwrap_or(0),
                map.0,
                tracking.creatures,
                tracking.resources,
                tracking.stealthed,
            );
        }
        let cursor = window.iter().next().and_then(|w| w.cursor_position());
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
        }
    });
    let mut hover = blips::MinimapBlipHover::None;
    if let Some(ctx) = &blip_ctx {
        // The guard-directions marker and the corpse ride this pass as landmarks, as the reference
        // appends its static blip slots after the DBC scan, so they draw even without
        // `AreaPOI.dbc`. The icons need only their atlas (`0x4ed21f`).
        blips::emit_landmarks(
            ctx,
            pois.as_ref().map(|p| &p.0),
            poi_marker.on_map(map.0),
            corpse.0.as_ref(),
            map.0,
            assets.tex(MinimapArtSlot::Icons),
            &mut quads,
            &mut hover,
        );
    }

    // The object dots, last of the minimap's own draw (`0x4ed7b7`): tracking dots (cells 0/1),
    // then quest (3) and party (4) dots.
    if let Some(ctx) = &blip_ctx {
        if let Some(icons) = assets.tex(MinimapArtSlot::Blips) {
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
}

/// Push the player's WMO containment, the indoor byte `0xceaa60`, so the zoom buttons drive the
/// indoor index indoors and the outdoor one outside; the verdict is [`emit_minimap`]'s, as the
/// reference keeps one flag. Pushed on the edge, which a new VM's first frame is. The edge also
/// fires `MINIMAP_UPDATE_ZOOM`, on which the stock `Minimap_OnEvent` re-syncs the +/- buttons to
/// the other index's level.
fn feed_minimap_inside(
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    world: benilla_world::world_point::WorldPoint,
    player: Res<Player>,
    instances: Query<&WmoPortalInstance>,
    wmos: Res<Assets<WmoModel>>,
    asset_server: Res<AssetServer>,
    mut inside_res: ResMut<MinimapInside>,
    mut was_inside: Local<crate::ui_script::VmMemo<Option<bool>>>,
) {
    let inside = minimap_interior(&player, &instances, &wmos, &world, &asset_server).is_some();
    inside_res.0 = inside;
    let Some(mut script) = script else { return };
    if *was_inside.get(&script) != Some(inside) {
        script.set_minimap_inside(inside);
        *was_inside.get(&script) = Some(inside);
        script.fire_event("MINIMAP_UPDATE_ZOOM", vec![]);
    }
}

/// A world entry puts the three default textures back before the interface loads:
/// `CGGameUI::InitializeGame` (`0x48f480`, from `0x401570`, the glue's enter-world step at
/// `0x46c236`) calls `0x4ed810` at `0x48f590`, which loads them over the process-wide handles. So
/// an addon's texture lasts to the next logout (`ClientDestroyGame` `0x401ee0` runs
/// `ShutdownGame`, which releases them at `0x49118c`). A `ReloadUI` (`0x490bd0`, `0x48fbf0`) and a
/// worldport (`0x401bc0`) reach neither, and keep them; an addon that sets one while it loads wins,
/// being applied after this ([`feed_minimap_art`]).
pub(crate) fn restore_default_mask(world: &mut World) {
    if let Some(mut assets) = world.get_resource_mut::<MinimapAssets>() {
        for tex in &mut assets.art {
            tex.current = tex.default.clone();
        }
    }
}

/// The default file of each texture (`0x84c6e4`, `0x84c6c8`, `0x84c6a8`).
fn default_art_path(slot: MinimapArtSlot) -> &'static str {
    match slot {
        MinimapArtSlot::Mask => benilla_ui::widget::MINIMAP_DEFAULT_MASK,
        MinimapArtSlot::Icons => "Interface\\Minimap\\POIIcons",
        MinimapArtSlot::Blips => "Interface\\Minimap\\ObjectIcons",
    }
}

/// Apply each texture setter's call (`SetMaskTexture`, `SetIconTexture`, `SetBlipTexture`) to the
/// process-wide handle, once per call: a file loads with the loose flag the setters pass
/// (`0x4ee5ce`), and one the probe passed that still fails to load is the green solid, as
/// `TextureCreate` gives; the VM's verdict of a miss is the green solid too, and an empty name the
/// null handle. A new VM whose Minimap has set nothing leaves the handles as they are.
fn feed_minimap_art(
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    assets: Option<ResMut<MinimapAssets>>,
    world_assets: Option<ResMut<WorldAssets>>,
    mut images: ResMut<Assets<Image>>,
    mut applied: Local<crate::ui_script::VmMemo<[u64; 3]>>,
) {
    let (Some(script), Some(mut assets)) = (script, assets) else {
        return;
    };
    let mut world_assets = world_assets;
    let applied = applied.get(&script);
    for slot in [
        MinimapArtSlot::Mask,
        MinimapArtSlot::Icons,
        MinimapArtSlot::Blips,
    ] {
        let Some((stamp, art)) = script.minimap_art(slot) else {
            continue;
        };
        if applied[slot as usize] == stamp {
            continue;
        }
        applied[slot as usize] = stamp;
        let tex = &mut assets.art[slot as usize];
        tex.current = match art {
            MinimapArt::File(path) => {
                let loaded = world_assets.as_mut().and_then(|w| {
                    if slot == MinimapArtSlot::Mask {
                        w.mask_texture(&path, LooseOpen::Index, &mut images)
                    } else {
                        w.sprite_texture_opened(&path, LooseOpen::Index, &mut images)
                    }
                });
                loaded.or_else(|| tex.missing.clone())
            }
            MinimapArt::Missing => tex.missing.clone(),
            MinimapArt::Null => None,
        };
    }
}

/// The mask as the interior blit and the outdoor tiles bind it, in that order. The outdoor tile
/// binds it to stage 1 with preset 1, modulate on colour and alpha (`0x4eae74`, `0x4eae83`), so
/// the tile's colour is times the mask's RGB too. The interior blit binds it (`0x4ec668`) without
/// setting the stage's preset, which it inherits, untraced, so it keeps the coverage alone.
fn stage_masks(
    mask: Option<&Handle<Image>>,
    rect: Rect,
) -> (Option<UiQuadMask>, Option<UiQuadMask>) {
    let blit = mask.map(|m| UiQuadMask {
        texture: m.clone(),
        rect,
        modulate_rgb: false,
    });
    let tile = blit.clone().map(|m| UiQuadMask {
        modulate_rgb: true,
        ..m
    });
    (blit, tile)
}

/// The corpse's static blip while the corpse is on the player's map, set by
/// [`feed_minimap_arrows`] (which reads its `CORPSE_RED` name off the VM) for the landmark draw.
#[derive(Resource, Default)]
pub(crate) struct MinimapCorpse(pub(crate) Option<benilla_formats::AreaPoi>);

/// Aim the minimap's arrow children, the arrow half of its render (`0x4ec950`), after the tick and
/// before the paint that draws them: the landmark selection's three records (rebuilt when its
/// identities change) and the party and pet records, by bearing. With an empty mask name the render
/// returns before any of it (`0x4ec968`), so the arrows keep what they had.
fn feed_minimap_arrows(
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    assets: Option<Res<MinimapAssets>>,
    player: Res<Player>,
    map: Option<Res<CurrentMap>>,
    pois: Option<Res<crate::area_poi::AreaPoiRes>>,
    poi_marker: Res<crate::poi_marker::PoiMarker>,
    group: Res<crate::ui_party::GroupState>,
    guids: Res<crate::net::GuidIndex>,
    // Never a world camera, so it shares nothing with the camera pose's write.
    unit_pos: Query<
        &GlobalTransform,
        (
            With<crate::net::NetEntity>,
            Without<benilla_world::view::WorldCamera>,
        ),
    >,
    death_net: Res<crate::death::DeathNet>,
    self_store: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    mut corpse: ResMut<MinimapCorpse>,
    inside: Res<MinimapInside>,
    mut listed: Local<crate::ui_script::VmMemo<Option<Vec<blips::ArrowKey>>>>,
) {
    let (Some(mut script), Some(assets), Some(map)) = (script, assets, map) else {
        return;
    };
    // Same map only: a dungeon corpse's display coords are the entrance.
    let dead = death_net
        .corpse
        .filter(|cp| cp.display_map == map.0 as i32)
        .map(|cp| cp.position);
    let named = corpse.0.as_ref().map(|c| (c.pos, c.continent_id));
    if dead.map(|p| (p, map.0)) != named {
        corpse.0 = dead.map(|p| blips::corpse_landmark(p, map.0, script.global_text("CORPSE_RED")));
    }
    if assets.tex(MinimapArtSlot::Mask).is_none() || !player.active {
        return;
    }
    let wow = bevy_to_wow(player.pos);
    let (wx, wy) = (wow[0], wow[1]);
    let radius = script.minimap_radius_yd();
    let sel = blips::select_landmarks(
        pois.as_ref()
            .into_iter()
            .flat_map(|p| p.0.rows().map(|(_, poi)| poi)),
        poi_marker.on_map(map.0),
        corpse.0.as_ref(),
        map.0,
        wx,
        wy,
        radius,
    );
    let (rim, keys) = blips::rim_arrow_records(&sel, wx, wy);
    let last = listed.get(&script);
    let rebuilt = last.as_ref() != Some(&keys);
    if rebuilt {
        *last = Some(keys);
    }
    let pet = self_store.iter().next().and_then(|s| s.0.unit_pet_guid());
    let records = blips::group_arrow_records(wx, wy, radius, &group, &guids, &unit_pos, pet);
    script.set_minimap_arrows(&rim, rebuilt, inside.0, &records);
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

/// Push the game clock `GetGameTime()` answers when its hour or minute changes, the API's own
/// resolution: the clock's fields as they stand (`0x515ee0`), -1, -1 while it is unset.
fn feed_game_time(
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    time: Res<crate::net::ServerTime>,
    mut last: Local<crate::ui_script::VmMemo<Option<(i32, i32)>>>,
) {
    let Some(mut script) = script else { return };
    let last = last.get(&script);
    let now = (time.0.hour(), time.0.minute());
    if *last == Some(now) {
        return;
    }
    *last = Some(now);
    script.set_game_time(now.0, now.1);
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
            .init_resource::<MinimapCorpse>()
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
                    emit_minimap
                        .in_set(UiQuadAppend)
                        .before(crate::portrait::PaneComposite),
                    // After the emit that fills it, so the blit never samples a frame-old target.
                    composite::drive_composite.after(UiQuadAppend),
                    // Before the script tick, so a zoom button routes to the live index.
                    feed_minimap_inside.in_set(crate::ui_script::UiFeed),
                    // Before the script tick, so an addon's OnUpdate reads this frame's heading.
                    feed_minimap_player_facing.in_set(crate::ui_script::UiFeed),
                    // After this frame's movement (the plate driver's set is past it), the art
                    // the tick set and every drain that moves a group, death or marker record,
                    // before the paint that draws the arrows, as the reference aims them inside
                    // its draw. Unordered against the terrain stream and the snap-hold release,
                    // which touch nothing it reads, so the paint never waits on streaming.
                    feed_minimap_arrows
                        .after(crate::vplates::VPlateSet)
                        .after(crate::ui_chat::ChatDrain)
                        .after(crate::ui_party::drain_party)
                        .after(crate::death::drain_death)
                        .after(crate::poi_marker::expire_marker)
                        .after(crate::poi_marker::clear_on_world_entry)
                        .after(feed_minimap_art)
                        .before(crate::ui_script::UiPaint),
                    // After the tick that can set the art, before the emit that draws with it.
                    feed_minimap_art
                        .after(crate::ui_script::UiInput)
                        .before(UiQuadAppend),
                    // Before the script tick, after the indoor push the radius reads. Gated on the
                    // in-game UI: a member's ping in the login burst has no player to land on
                    // (`0x4ee341`), so it is dropped, not signalled to the glue VM.
                    ping::drive_minimap_ping
                        .after(feed_minimap_inside)
                        .in_set(crate::ui_script::UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    ping::drop_inbound_pings
                        .after(ping::drive_minimap_ping)
                        .in_set(crate::ui_script::UiFeed)
                        .run_if(not(crate::ui_script::ingame_ui_up)),
                    // After the tick whose `PingLocation` calls it sends, and the chat and group
                    // drains, so a ping from a slash handler goes against this frame's roster.
                    ping::send_own_pings
                        .after(crate::ui_chat::ChatDrain)
                        .after(crate::ui_party::drain_party),
                    // Before the script tick, so GameTimeFrame reads this frame's minute.
                    feed_game_time.in_set(crate::ui_script::UiFeed),
                    // In the feed, off the hover the last walk left; after the world drive only so
                    // the two VM writers keep one order.
                    blips::drive_blip_tooltip
                        .after(crate::ui_unit::UnitFeed)
                        .in_set(crate::ui_script::UiFeed),
                ),
            );
    }
}

/// The mask state for the lifecycle tests, which have no install to load a texture from: a
/// [`MinimapAssets`] whose default and current masks are the given handles.
#[cfg(test)]
pub(crate) fn seat_mask_for_test(
    world: &mut World,
    default_mask: Handle<Image>,
    current: Handle<Image>,
) {
    let mut assets = MinimapAssets::default();
    assets.art[MinimapArtSlot::Mask as usize] = MinimapTex {
        current: Some(current),
        default: Some(default_mask),
        missing: None,
    };
    world.insert_resource(assets);
}

/// The mask now drawn.
#[cfg(test)]
pub(crate) fn mask_for_test(world: &World) -> Option<Handle<Image>> {
    world.resource::<MinimapAssets>().art[MinimapArtSlot::Mask as usize]
        .current
        .clone()
}

#[cfg(test)]
mod stage_mask_tests {
    use super::*;

    /// The tiles' mask modulates colour (`0x4eae74`); the interior blit's keeps coverage alone.
    #[test]
    fn the_tiles_mask_modulates_colour_and_the_blits_does_not() {
        let tex = Handle::<Image>::default();
        let rect = Rect::new(0.0, 0.0, 140.0, 140.0);
        let (blit, tile) = stage_masks(Some(&tex), rect);
        assert!(!blit.unwrap().modulate_rgb, "the interior blit");
        let tile = tile.unwrap();
        assert!(tile.modulate_rgb, "the outdoor tiles");
        assert_eq!(tile.rect, rect);
        assert!(matches!(stage_masks(None, rect), (None, None)));
    }
}

#[cfg(test)]
mod art_tests {
    use super::*;
    use benilla_ui::script::UiScript;

    const DEFAULT: Handle<Image> =
        bevy::asset::uuid_handle!("d0000000-0000-4000-8000-0000000000b1");
    const GREEN: Handle<Image> = bevy::asset::uuid_handle!("d0000000-0000-4000-8000-0000000000b2");

    fn art_app() -> App {
        let mut app = App::new();
        let mut assets = MinimapAssets::default();
        for tex in &mut assets.art {
            *tex = MinimapTex {
                current: Some(DEFAULT),
                default: Some(DEFAULT),
                missing: Some(GREEN),
            };
        }
        app.insert_resource(assets)
            .init_resource::<Assets<Image>>()
            .add_systems(Update, feed_minimap_art);
        let mut script = UiScript::new().unwrap();
        script.set_texture_probe(Box::new(|path: &str| !path.contains("Nope")));
        script
            .run(r#"m = CreateFrame("Minimap", "Minimap")"#)
            .unwrap();
        app.insert_non_send_resource(script);
        app
    }

    fn run(app: &mut App, chunk: &str) {
        let _ = app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .run(chunk);
        app.update();
    }

    fn drawn(app: &App, slot: MinimapArtSlot) -> Option<Handle<Image>> {
        app.world().resource::<MinimapAssets>().tex(slot).cloned()
    }

    /// A missing file draws the green solid the setter stored before it raised; an empty name
    /// leaves the null handle, on which the render draws no map (`0x4ec968`); the next world
    /// entry's load puts the default back, and a new VM that sets nothing changes nothing.
    #[test]
    fn the_mask_setter_s_result_is_what_the_map_draws_with() {
        let mut app = art_app();
        assert_eq!(drawn(&app, MinimapArtSlot::Mask), Some(DEFAULT));
        run(&mut app, r#"m:SetMaskTexture("Interface\\Nope")"#);
        assert_eq!(drawn(&app, MinimapArtSlot::Mask), Some(GREEN));
        run(&mut app, r#"m:SetMaskTexture("")"#);
        assert_eq!(
            drawn(&app, MinimapArtSlot::Mask),
            None,
            "no texture, no map"
        );

        app.insert_non_send_resource(UiScript::new().unwrap());
        app.update();
        assert_eq!(
            drawn(&app, MinimapArtSlot::Mask),
            None,
            "the process's handle stands"
        );

        restore_default_mask(app.world_mut());
        assert_eq!(drawn(&app, MinimapArtSlot::Mask), Some(DEFAULT));
    }

    /// The two atlases follow their own setters the same way, each slot apart.
    #[test]
    fn the_icon_and_blip_atlases_follow_their_setters() {
        let mut app = art_app();
        run(&mut app, r#"m:SetIconTexture("")"#);
        assert_eq!(
            drawn(&app, MinimapArtSlot::Icons),
            None,
            "no landmark icons"
        );
        assert_eq!(drawn(&app, MinimapArtSlot::Blips), Some(DEFAULT));
        run(&mut app, r#"m:SetBlipTexture("Nope")"#);
        assert_eq!(drawn(&app, MinimapArtSlot::Blips), Some(GREEN));
        assert_eq!(drawn(&app, MinimapArtSlot::Mask), Some(DEFAULT));
        restore_default_mask(app.world_mut());
        assert_eq!(drawn(&app, MinimapArtSlot::Icons), Some(DEFAULT));
        assert_eq!(drawn(&app, MinimapArtSlot::Blips), Some(DEFAULT));
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
