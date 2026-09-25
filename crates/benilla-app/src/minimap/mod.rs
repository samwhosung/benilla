//! The HUD minimap renderer (decision 0203 phase 1) — the app half of the `<Minimap>` widget.
//!
//! The engine side (benilla-ui) carries the widget's rect + zoom and emits a
//! `QuadContent::Minimap` hole at the frame's own draw slot; `ui_script::extract::paint_script` parks that
//! in [`MinimapWidget`], and [`emit_minimap`] (in the [`UiQuadAppend`] window) fills it: the
//! streamed tile window around the player, clipped to the widget rect and masked to the
//! `MinimapMask.blp` circle at present time, with the player arrow rotating on top. Children of
//! the widget (border art, buttons, zone text — `MinimapCluster.xml`) draw above per the normal
//! z order.
//!
//! Mechanism per the reference client (transcribed here):
//! - **Tile grid**: one 256² minimap BLP per ADT tile (533.33 yd), named `map<X>_<Y>.blp` in the
//!   map's directory and resolved through `md5translate.trs` to a hashed file under
//!   `textures\Minimap\` ([`benilla_formats::MinimapTranslate`]). Index order = the ADT order
//!   (chain-verified, see the formats re-export note).
//! - **Zoom → world radius** (0x6da9b0): the client keeps **two** zoom indices,
//!   selected by whether the player is inside a WMO. **Outdoors** the chunk-count table
//!   `{14,12,10,8,6,4} · 0.5 · 33.333` yd of half-extent; **indoors** the radius table
//!   `{150,120,90,60,40,25}` yd outright ([`INTERIOR_ZOOM_RADIUS`]). Each index persists separately
//!   (CVars `minimapZoom` / `minimapInsideZoom`).
//! - **North-up orientation**: screen up = world +X (north), screen left = world +Y (west).
//!
//! Tiles stream through the `mpq://` async asset source (the terrain streamer's hitch-free bulk
//! path); handles cache per tile in [`MinimapTileCache`] and reset on a map change.
//!
//! Submodules: [`interior`] — the WMO-interior group selection (portal flood-fill) + tile-name
//! stem; [`blips`] — the phase-3 blip layer (AreaPOI landmark arrows, quest-giver dots, the
//! hover tooltip).

pub(crate) mod blips;
/// The party blip's position law, shared with the world map (report B320): one function decides
/// where a member is, so the two surfaces can never disagree about it.
pub(crate) use blips::party_member_pos;
mod composite;
mod interior;
/// The minimap ping — engine-owned and pinned to a world point.
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

/// The north-up zoom table (`0x8116d0`): view **diameter** in chunks per
/// zoom index; half-extent = `chunks · 0.5 · 33.333` yd (0x6da9b0's unlocked leg).
/// This is the **outdoor** zoom basis.
const ZOOM_CHUNKS: [f32; 6] = [14.0, 12.0, 10.0, 8.0, 6.0, 4.0];

/// The **interior** minimap view radius per *indoor* zoom index, in yards — the client's radius table
/// `0x8116e8`, indexed by the separate indoor zoom index `0x86f69c` (CVar `minimapInsideZoom`).
/// Indoors the minimap has its **own zoom state**: a different index, a different table, and a
/// radius in raw yards rather than the outdoor chunk half-extent. That is the "different zoom states
/// inside vs outside" the director reported (2026-07-09).
///
/// On-screen radius is exactly this value (the client composites the interior to an offscreen
/// target at `1.5·c` in `0x4ec090` and blits its middle two-thirds in `0x4ec440`, netting `1.0·c`).
/// The **same `c`** sizes the tile-selection query box (`0x6d96b6`), so selection and draw stay
/// coherent.
///
/// NB the interior scale is not a compile-time constant (`10.0f` ⇒ a fixed 15 yd,
/// zoom-independent): that `10.0f`, at `0x4ed9eb`, is only the *static initializer*, and the
/// per-frame write `mov [esi+0xc], radiusTable[indoorZoom]` (`0x6d98f5`) reaches the field through
/// a computed pointer. Do not "restore" a constant here.
const INTERIOR_ZOOM_RADIUS: [f32; 6] = [150.0, 120.0, 90.0, 60.0, 40.0, 25.0];

/// **How far the map reaches, in yards** — the one function that answers it. Indoors that is the
/// interior radius table indexed by the *indoor* zoom; outdoors the chunk-count table's half-extent
/// indexed by the outdoor zoom. Both branches of [`emit_minimap`] and the ping's world→normalized
/// relay go through here, so "the view radius" can only ever mean one thing (the twin of
/// [`blips::party_member_pos`]'s reason for existing: two surfaces disagreeing about the same
/// number is a bug you find months later).
fn view_radius_yd(zoom: u8, inside_zoom: u8, inside: bool) -> f32 {
    let clamp = |z: u8| usize::from(z.min(5));
    if inside {
        INTERIOR_ZOOM_RADIUS[clamp(inside_zoom)]
    } else {
        ZOOM_CHUNKS[clamp(zoom)] * 0.5 * CHUNK_YARDS
    }
}

/// The **outer-edge bleed**: a minimap tile on the boundary of its group's grid is drawn 1.0 yd
/// larger on that side, so a group's art extends 1 yd past its bbox all the way round and two
/// groups whose boxes touch overlap by 2 yd. Interior cell edges are shared exactly. Byte-verified
/// (`0x6a549e`…`0x6a54db`, the constant `0xca8098` built in the emitter as `0.5 + 0.5`) and fitted
/// to the reference's captured quads with zero error.
const EDGE_BLEED_YD: f32 = 1.0;

/// The client's half-texel UV inset, as the quad scale that reproduces it: a tile spanning
/// `extent` yards is baked at [`YD_PER_TEXEL`](benilla_assets::minimap_grid::YD_PER_TEXEL), so it
/// is `W = extent / 0.5` texels wide, and mapping texel *centres* to the quad's edges instead of
/// texel *edges* stretches it by `W / (W − 1)`. See the call site for why it matters.
fn texel_stretch(extent_yd: f32) -> f32 {
    let texels = extent_yd / benilla_assets::minimap_grid::YD_PER_TEXEL;
    if texels > 1.0 {
        texels / (texels - 1.0)
    } else {
        1.0
    }
}

/// The interior tile draw's **alpha-test reference** — `224/255`, the client's
/// `glAlphaFunc(GL_GEQUAL, 0.87843144)`. It is never set explicitly: the tile draw sets EGxBlend
/// **1** (whose applicator `glDisable`s blending), and `SetRenderState`'s id-7→id-8 cascade reads
/// `.data 0x85ad20[1] = 224` and multiplies by the f32 reciprocal of 255 — `0x3F60E0E2`, one ULP
/// above `224/255`.
/// Written as the exact f32 the client computes rather than the ratio, because that ULP is the
/// value fragments are compared against.
pub(crate) const INTERIOR_TILE_ALPHA_REF: f32 = f32::from_bits(0x3F60_E0E2);

/// The corpse blip's edge as a fraction of the widget side (the POIIcons cell is authored 16px on
/// a 140px minimap ≈ 0.11; INTERIM eyeball beside [`ARROW_FRACTION`]'s).
const CORPSE_BLIP_FRACTION: f32 = 0.11;

/// The day-night tint the reference MODULATEs the **outdoor** (ADT) minimap tiles by before drawing
/// — the tiles are NOT drawn at full white (that reads too bright). Verified in the CWorldFrame
/// minimap draw (tile draw `0x4eccdd`–`0x4ecd69`): from the two global
/// day-night light colours — `color_a` = the Direct/Diffuse band (`LightIntBand` 0 = the light
/// table's `table[0]`), `color_b` = the Ambient band (band 1 = `table[1]`):
///
/// ```text
///   L  = luma601(color_b)                # (r·77 + g·151 + b·28) >> 8, on 0..255 bytes
///   t  = min(L + 96, 255) / 256          # a +96 floor: even pitch-dark tints ~0.375 toward white
///   B' = lerp(color_b, white, t)
///   A' = lerp(color_a, B', 0.75) = 0.25·color_a + 0.75·B'
/// ```
///
/// Inputs and output are **gamma-space** (`WowLighting`'s sRGB 0..1 convention); handed to the UI
/// quad as its vertex colour, whose own linearize→re-encode reproduces the client's gamma-space
/// MODULATE. Interior (WMO) tiles are drawn full white and skip this.
fn minimap_day_tint(ambient: [f32; 3], diffuse: [f32; 3]) -> [f32; 3] {
    let (color_a, color_b) = (diffuse, ambient);
    // Rec.601 luma on 0..255 bytes: the weights (77,151,28) sum to 256, so the parenthesised sum is
    // a 0..1 weighted average; ×255 lifts it to the byte the client's `>> 8` produces.
    let l_byte = 255.0 * (color_b[0] * 77.0 + color_b[1] * 151.0 + color_b[2] * 28.0) / 256.0;
    let t = (l_byte + 96.0).min(255.0) / 256.0;
    let mut out = [0.0_f32; 3];
    for c in 0..3 {
        let b_prime = color_b[c] + (1.0 - color_b[c]) * t; // lerp(color_b, white, t)
        out[c] = color_a[c] * 0.25 + b_prime * 0.75; // lerp(color_a, B', 0.75)
    }
    out
}

/// A `.map()` adaptor over a `md5translate.trs` hit: stream the hashed tile off-thread (like a
/// terrain tile) but as a **minimap tile** (`BlpVariant::MapTile`: gamma bytes with no sRGB decode
/// — the reference sampler's `GL_SKIP_DECODE_EXT` — clamp, mip 0, LINEAR). Gamma bytes move the
/// sRGB decode to AFTER the filter, so every consumer must say so or draw the tile ~2× too bright
/// through the UI pass: the outdoor quads carry
/// [`UiQuad::gamma_texel`](crate::ui_pass::UiQuad::gamma_texel), and the interior composite's
/// alpha-test arm decodes explicitly for its un-encoded target. The invariant `benilla-assets`'
/// `minimap_tile_settings_reach_the_async_loader` guards these settings reaching the async loader.
/// Shared by the terrain and interior tile paths.
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

/// The **headless probe's** minimap widget. `WOW_MM_PROBE` drops the player inside a building in a
/// server-less capture ([`crate::capture`]), but the real slot comes from the FrameXML `<Minimap>`
/// extraction, which needs a logged-in UI — so without this the interior branch never runs and the
/// composite cannot be looked at offline. Under the probe (and only then) we synthesise the slot the
/// script would have published: a square in the top-right corner at the client's default zoom.
///
/// This is the instrument the B141 arc needed and did not have — the interior composite was being
/// argued about from screenshots because nothing could render it without a server.
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

/// This frame's extracted `<Minimap>` widget slot, written by `ui_script::extract::paint_script` (the
/// `QuadContent::Minimap` arm) — `None` when no Minimap widget is visible (cluster hidden, no XML).
#[derive(Resource, Default)]
pub(crate) struct MinimapWidget(pub(crate) Option<MinimapSlot>);

/// The player's WMO-containment verdict as of this frame — [`feed_minimap_inside`]'s answer, kept
/// so anything that has to know "which zoom table is live" can read it instead of re-running the
/// portal flood-fill. The client keeps one flag for both consumers (`0xceaa60`) and so do we.
#[derive(Resource, Default)]
pub(crate) struct MinimapInside(pub(crate) bool);

/// The **persisted** half of the minimap zoom — the client's two CVar objects
/// `minimapZoom` / `minimapInsideZoom`, whose registered default is `"3"` in both cases (at the
/// `RegisterCVar 0x63db90` argument slot). The *live* indices are the widget's
/// ([`benilla_ui::widget::MinimapState`]); this is the durable knob [`crate::cvars`] loads out of
/// `config.toml` and saves back into it. It is **read once** — when the in-game UI materializes and
/// the fresh widget is seeded from it (`UiScript::set_minimap_zoom`) — and written whenever
/// `Minimap:SetZoom` reports a new level, which is exactly the client's own split: `set_zoom` writes
/// the live index *and* the CVar, and the minimap reset path re-seeds the index from the CVar.
#[derive(Resource)]
pub(crate) struct MinimapZoom {
    /// `minimapZoom` — the outdoor index (the chunk table's).
    pub(crate) outdoor: u8,
    /// `minimapInsideZoom` — the indoor index (the radius table's), persisted separately so zooming
    /// indoors never disturbs the outdoor level.
    pub(crate) inside: u8,
}

/// The two zoom indices' change callback (1131, 2303): each index lands on its own field,
/// clamped exactly like the client's `set_zoom` (`0x6daa10`: clamp at 5) — the widget clamps
/// again on the way in, so a hand-edited level lands in range whichever path it takes.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut zoom: ResMut<MinimapZoom>) {
    match ev.key().as_str() {
        "minimapzoom" => zoom.outdoor = zoom_index(ev.num()),
        "minimapinsidezoom" => zoom.inside = zoom_index(ev.num()),
        _ => {}
    }
}

/// A stored minimap zoom level → a valid index: truncate to int and clamp into
/// `[0, MINIMAP_ZOOM_LEVELS)`, the client's own `set_zoom` clamp.
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

/// One extracted Minimap widget: where it sits on screen (y-down logical px), its paint key, and
/// its live widget state.
pub(crate) struct MinimapSlot {
    pub(crate) rect: Rect,
    pub(crate) z: u64,
    /// The outdoor zoom index (chunk table); `inside_zoom` is its indoor twin (radius table). The
    /// client persists both and picks by WMO containment — see [`INTERIOR_ZOOM_RADIUS`].
    pub(crate) zoom: u8,
    pub(crate) inside_zoom: u8,
    pub(crate) alpha: f32,
}

/// The loaded minimap fixtures: the tile hash catalog + the circular mask + the arrow art.
/// Inserted at startup once the chain is open; absent = the minimap draws nothing (its XML
/// children still render).
#[derive(Resource)]
struct MinimapAssets {
    translate: MinimapTranslate,
    mask: Option<Handle<Image>>,
    arrow: Option<Handle<Image>>,
    /// The shared POI atlas (`Interface\Minimap\POIIcons`) — the corpse blip's skull cell
    /// and any later POI rides it.
    poi: Option<Handle<Image>>,
    /// The **four** rim-arrow arts — the flat `.blp` stand-ins for the one `minimapArrowModel`
    /// (`Rotating-MinimapArrow.mdx`) the reference re-animates per blip source. See
    /// [`blips::RimArrow`] for the sequence→layer table and why there are four of them.
    rim_arrows: blips::RimArrowArt,
    /// The unit-blip atlas (`Interface\Minimap\ObjectIcons`, five 32-px dot cells) — the
    /// quest-giver dots.
    object_icons: Option<Handle<Image>>,
    /// `SpellShapeshiftForm.dbc` — the tracking dots' creature-type override (a cat-form
    /// druid is a Beast). `None` = no override (unshifted resolution only).
    forms: Option<HashMap<u32, benilla_formats::ShapeshiftForm>>,
}

/// Async tile handles by ADT index, for the [`CurrentMap`] it was filled on. `None` = the tile has
/// no authored minimap art (open ocean) — cached so the translate lookup doesn't re-run per frame.
/// The interior half caches the WMO tiles by `(group, col, row)` for the WMO whose `stem` (its
/// `md5translate.trs` path stem) is resident, cleared when the player enters a different building.
#[derive(Resource, Default)]
struct MinimapTileCache {
    map_id: Option<u32>,
    tiles: HashMap<(u32, u32), Option<Handle<Image>>>,
    interior_stem: Option<String>,
    interior: HashMap<(usize, u32, u32), Option<Handle<Image>>>,
}

/// Loads the translate catalog + the mask/arrow art once the patch chain is open.
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

/// The minimap's **one** containment verdict — which map family draws, and (through
/// [`feed_minimap_inside`]) which zoom index the +/- buttons drive. The client keeps a single flag
/// for both (`0xceaa60`), so this must be computed once and shared: two probes drifting apart is
/// how the map ends up drawn at the wrong scale for the family it is showing.
///
/// The reference hard-switches map families on interior containment: standing inside a WMO group it
/// draws that building's OWN minimap tiles and SUPPRESSES the terrain (mutually exclusive, not a
/// transparent overlay). Returns the placement, model, `md5translate.trs` path stem and seed group
/// of the WMO the player is in, or `None` for the terrain family.
///
/// **The gate is the client's one indoor byte** (`0xbc8300`), and that byte is the CGLight node's
/// down-ray bit `[node+0x90] & 1` (`0x670547` — not a containment group's `0x10`, which is a
/// ctor-set class tag, not a group flag). The predicate is a **position cast, faces only**: the
/// nearest surface within 1000 yd straight down — terrain racing the WMO faces, closer wins and the
/// WMO wins ties — is a WMO face whose group lacks MOGP `0x8`. Terrain below ⇒ outdoors, whatever
/// building you are geometrically inside. That is exactly [`CurrentAreaInterior`]'s law
/// (`wmo_portal::area_down_ray`, the zone-text bit), so the gate reads it rather than re-deriving
/// one: the portal-crossing leg in [`down_ray_seeds`] belongs to the CAMERA's current-group system
/// and claiming an interior through a doorway plane under the eye is the abbey-yard bug's shape.
/// [`down_ray_seeds`] still supplies the flood SEED once the gate has said indoors.
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
    // The down-ray races the terrain, exactly as the interior/zone tracker does — standing on the
    // grass above a mine's tunnels is not standing in the mine.
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

/// Fills the extracted widget hole: the visible tile quads (clipped to the widget, masked to the
/// circle) and the player arrow, appended at the widget's own z (stable sort keeps append order
/// within a key, so the arrow rides above the tiles and below the widget's children).
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
    // Hover resets every frame; the blip pass below re-establishes it while the map draws.
    *blip_hover = blips::MinimapBlipHover::None;
    // Drain this frame's `Minimap:PingLocation` click BEFORE any early return below, so it is
    // always spent in the frame it was made. Held across frames it would seat against geometry
    // the player never clicked on; and a click made on a frame the map does not draw is simply
    // not a ping.
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
    // `WOW_MM_ZOOM=0..5` forces the zoom level of whichever map is showing — a capture instrument
    // (pairs with the `WOW_MM_PROBE` interior probe). Indoors and outdoors each carry their own
    // persisted index, so the override stands in for both.
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
    // The active branch's world→px scale, for the point blips drawn after the tiles (the corpse
    // marker below; both branches share the same north-up point mapping around `center`).
    let mut blip_px_per_yd = 0.0_f32;

    let mask = assets.mask.as_ref().map(|m| UiQuadMask {
        texture: m.clone(),
        rect: slot.rect,
    });

    let interior = minimap_interior(&player, &instances, &wmos, &world, &asset_server);

    // The player's containment verdict, kept as a bool for the quest-dot grey (the branch
    // below consumes `interior` itself).
    let player_indoors = interior.is_some();
    // How far the map reaches this frame — decided once, by the one function that decides it, and
    // used by whichever family draws below.
    let view_radius = view_radius_yd(zoom, inside_zoom, player_indoors);
    if let Some((world_from_local, model, stem, in_group)) = interior {
        // INTERIOR: the WMO's own per-group tiles, drawn FULL WHITE (the day-night tint is outdoor-
        // only). The tiles are baked in the WMO's MODEL frame (north = model +X, sized to the model
        // footprint — verified against the 97°-yaw Goldshire Inn: group 3's tile is 64×32 px = its
        // model bbox, not the world AABB). So place each tile at its model-space centre mapped
        // through the placement, and rotate the WHOLE set by ONE placement-yaw angle — not per-tile
        // world AABBs (one orientation basis, `0x6da180`). Cached by `(group, col, row)`.
        if cache.interior_stem.as_deref() != Some(stem.as_str()) {
            cache.interior.clear();
            cache.interior_stem = Some(stem.clone());
        }
        // INTERIOR ZOOM: indoors has its OWN zoom index and its own table — the view radius is
        // `radiusTable[inside_zoom]` in raw yards (150 widest … 25 tightest), not the outdoor chunk
        // half-extent. The zoom buttons drive `inside_zoom` while you're inside, and it persists
        // separately from the outdoor level (index `0x86f69c`, table `0x8116e8`).
        let radius = view_radius;
        let px_per_yd = (side * 0.5) / radius;
        blip_px_per_yd = px_per_yd;

        // The tiles are composited into the client's own 256² TARGET, not drawn at the screen —
        // both halves of the mechanism matter and only work together (the module
        // docs in [`composite`] carry the why). Target space: y-UP, origin at the target's centre
        // (= the player), and `RT_HALF_EXTENT_SCALE · radius` yards to an edge.
        #[allow(clippy::cast_precision_loss)] // 256 is exact in f32
        let units_per_yd =
            (composite::RT_SIZE as f32 * 0.5) / (composite::RT_HALF_EXTENT_SCALE * radius);
        // A model point → its north-up target position (through the placement to world, then the
        // same north-up map the terrain tiles use: up = world +X north, left = +Y west).
        let to_target = |m: [f32; 3]| {
            let w = bevy_to_wow(world_from_local.transform_point3(wow_to_bevy(m)));
            Vec2::new((wy - w[1]) * units_per_yd, (w[0] - wx) * units_per_yd)
        };
        // The one placement rotation: where the model +X axis points, as a CLOCKWISE-on-screen
        // angle (the target's y-up frame negates it at the Transform). Same for every tile.
        let x_axis = to_target([1.0, 0.0, 0.0]) - to_target([0.0, 0.0, 0.0]);
        let rotation = (-x_axis.y).atan2(x_axis.x);
        // The target's own edge, in target units, for the window cull below.
        #[allow(clippy::cast_precision_loss)]
        let rt_half = composite::RT_SIZE as f32 * 0.5;
        composite.active = true;

        // GROUP SELECTION: the portal flood-fill from the player's current group (`0x6a5020`) —
        // NOT draw-every-group. Only the groups
        // reached through portals within the query box, whose bbox overlaps the view in XY, are drawn.
        // This is what stops floors the player can't reach (or that are far outside the view) from
        // painting over the current one.
        // The selection query box uses the SAME `c` as the draw radius (`0x6d96b6`) — so we never
        // load tiles we cannot show, and zooming in indoors tightens the box's Z extent too, which
        // is what trims how many stacked floors bleed through.
        let drawable =
            interior_group_selection(model, &world_from_local, player.pos, radius, in_group);
        // Draw ORDER (`0x4ebeb0`, keyed at `0x6d9e30`): the composite is Z-sorted ascending by
        // `Zmidpoint − playerZ`, with the player's OWN group forced to the top (the client keys it
        // FLT_MAX). So floors below draw at the bottom, floors above over them, and the player's
        // current floor LAST of all — a stacked storey shows only through its transparent stairwell
        // gaps, never occluding the room you're in ("basement on top the kitchen", director).
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
                    // The tile's world rect: the grid cell PLUS the client's outer-edge BLEED. The
                    // cells themselves stride exactly `tw`, sharing their interior edges — but a
                    // cell on the grid's boundary is grown by 1.0 yd on that side alone
                    // (`0x6a549e`/`0x6a54ae`/`0x6a54be`/`0x6a54d1`, each an `fsub`/`fadd` of
                    // `0xca8098 = 0.5 + 0.5`; fitted to the reference's own captured quads with
                    // zero error on every bound). A 1×1 grid is therefore `tw + 2` across, an
                    // end cell `tw + 1`, an interior cell exactly `tw`.
                    //
                    // THIS is what makes the joints work. The bleed grows every group's art 1 yd
                    // past its bbox on each outer side, so two groups whose boxes touch OVERLAP by
                    // 2 yd. Without it their art merely abuts — and abutting art does not survive
                    // the composite's alpha test: `GEQUAL 224/255` on a LINEAR-filtered silhouette
                    // reaches only 0.122 of a texel past the last opaque texel centre (nearest
                    // would reach the texel edge, 0.5), so an abutting pair loses ~0.38 texel and
                    // the black clear reads through the strip for the length of the wall. That is
                    // B141's dashed hairline; a 2 yd overlap absorbs the same erosion with four
                    // texels to spare. In the reference's captured frame 16 of 220 inter-group
                    // tile contacts exist ONLY because of the bleed.
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
                    // Window cull: skip tiles whose centre lands well outside the TARGET (which
                    // holds 1.5× what the blit shows, so this is wider than the visible disc —
                    // deliberately: the client composites the same margin).
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
                        // The client's HALF-TEXEL UV INSET, expressed as the scale it is: it
                        // samples `[0.5/W, 1−0.5/W]` across the rect above (verified on the
                        // captured quads — every one carries exactly that UV rect), and sampling
                        // that range across a quad of size `Q` is the same as sampling `[0, 1]`
                        // across `Q·W/(W−1)` about the same centre. So the shared unit mesh keeps
                        // its UVs and the inset rides the Transform like everything else here, and
                        // the texel centres land where the client's do. `W` comes from the TILE
                        // (`tw / 0.5`), never from the bled rect.
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

        // `WOW_MM_STATS=1` reports what the interior branch actually put in the target this frame —
        // how many groups the flood-fill kept out of how many, and how many tiles that came to. The
        // reference's own Stormwind capture emitted 57 tiles at indoor zoom 3, which is the number
        // this is here to be compared against.
        static MM_STATS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *MM_STATS.get_or_init(|| std::env::var_os("WOW_MM_STATS").is_some()) {
            eprintln!(
                "MM-STATS: radius {radius} yd, groups {}/{} selected, {} tiles composited",
                drawable.iter().filter(|d| **d).count(),
                drawable.len(),
                composite.tiles.len(),
            );
        }

        // THE BLIT: the target's middle two-thirds, which is what nets `1.0 · radius` on screen
        // (the client's `0x4ec440` under EGxBlend 2). One quad, masked to the minimap circle — the
        // round cut belongs HERE and not to the tiles, exactly as the reference's does. The target
        // is opaque everywhere (its clear is opaque black), so this quad IS the black backing the
        // screen path used to push separately.
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
        // OUTDOOR: the ADT terrain tiles, MODULATEd by the day-night light tint (not full white —
        // else too bright, the reference's CWorldFrame minimap draw). Absent lighting ⇒ white.
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
                    continue; // unauthored tile (open ocean) — the mask shows the clear color
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
                    // The tile is a SKIP_DECODE upload (gamma bytes — [`load_tile`]), so the
                    // day-night MODULATE above lands on the authored byte, exactly the reference's
                    // fixed-function stage. Without this the ordinary arm re-encodes an
                    // already-encoded byte — the outdoor minimap reads visibly too bright.
                    gamma_texel: true,
                    // No CPU clip (1463): the mask shader already zeroes everything outside
                    // `mask_rect` (`ui_quad.wgsl`'s `inside` test), and clipping a PANNING tile
                    // re-cut its quad every frame — constant positions, churning UVs — which is
                    // exactly the shape the batcher's pan gate cannot ride on a `Transform`.
                    // Unclipped, the tile is a pure translation and never rewrites its mesh.
                    mask: mask.clone(),
                    ..default()
                });
            }
        }
    }

    // ── The blip layer (decision 0203 phase 3; byte law per the 0337 fold-back): landmarks
    // draw under the player arrow; the quest dots draw LAST — above it (the client's own draw
    // order). Hover lands in [`blips::MinimapBlipHover`] for the tooltip drive.
    // Our own descriptor's tracking state (PRIVATE fields — only ever on the self entity).
    let me = self_store.iter().next();
    // Our own guid — the classifier compares a candidate's charm/summon owner against it, so our
    // own pet and minions never take a dot (`0x4eac0d`).
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
        // The player's pan term, quantized to a half-logical-pixel grid (one device px at 2×):
        // every blip offset — and the rim arrows' bearing — derives from `wx`/`wy`, so the whole
        // blip layer steps together a few times a second instead of re-emitting sub-pixel-shifted
        // quads every frame while walking (1463; a 0.07 px/frame slide on a 16 px icon is not a
        // visible motion, but each slide rewrote the batch mesh and armed the world's
        // `AssetChanged` scans). The blips' world positions stay exact — only the shared pan
        // origin snaps.
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
            // The same point in UI space (y-up, ÷s through the 0582/0584 seam — the tooltip's
            // anchor resolves in the VM's 768-virtual units, not window px): the cursor seat.
            cursor_ui: cursor
                .zip(win)
                .map(|(c, w)| Vec2::new(c.x / seam, (w.height() - c.y) / seam)),
            seam,
        }
    });
    let mut hover = blips::MinimapBlipHover::None;
    if let Some(ctx) = &blip_ctx {
        // The guard-directions marker rides this pass as a landmark candidate, the way the
        // reference appends its static blip slot after the DBC scan — so it draws even when
        // `AreaPOI.dbc` failed to load, and the pass runs on the arrow art alone.
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
            // The party/corpse rim arrows (0434 phase 6b, the out-of-range half of `0x6dad10`)
            // draw with the POI arrows — before the player arrow, per the client's order.
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
        // Byte-pinned quad (blips::PLAYER_ARROW_QUAD_PX): the MinimapArrow.m2 single quad at
        // 1280 px/unit on the frozen 140.8 basis, its authored centre offset rotating with
        // the facing (clockwise screen rotation, so the offset rotates by the same angle).
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

    // The object dots draw LAST — above the player arrow (the client's draw order, 0x4ed7b7):
    // tracking dots (cells 0/1) first, then quest dots (cell 3) and party dots (cell 4) — the
    // draw's own cell-list order.
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
                // A dot NPC's own containment — the same faces-only down-ray the entity light
                // classifier stands on (dots are few; the per-frame rays are cheap).
                |feet| world.indoors_at(feet),
                &mut quads,
                &mut hover,
            );
            // The in-range party dots (blue cell 4, 1.3×) draw with the object dots — last.
            blips::emit_party_dots(ctx, &group, &guids, &unit_pos, icons, &mut quads);
        }
    }
    *blip_hover = hover;

    // The corpse blip: in range, the POIIcons skull cell (the same art the
    // ref's world-map corpse uses; the engine-drawn in-range minimap corpse art is INTERIM until
    // named) at the corpse's true position, through the same north-up point mapping as the
    // tiles. OUT of range the corpse is the fifth slot of `0x6dad10`'s placement — the rotating
    // rim arrow drawn with the party arrows above (the byte law replaced the old 0.92
    // edge-clamped skull). Same-map only (the display coords are the entrance for a dungeon
    // corpse).
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

    // A `Minimap:PingLocation` click is seated here, against the geometry standing right here,
    // this frame. The marker itself is the stock `MiniMapPing` `<Model>`, a Lua
    // child of the Minimap that composites over everything the engine drew into the widget's
    // hole — rendered by `crate::ui_models` since.
    if let Some(ctx) = &blip_ctx {
        ping::seat_click(ctx, &mut ping, click);
    }
}

/// Push the player's WMO-containment state onto the Minimap widget (the client's `0xceaa60`), so the
/// zoom buttons drive the **indoor** zoom index while indoors and the outdoor one while outside, each
/// persisting across the transition. Runs before the script tick, so a `SetZoom` fired from a button
/// handler this frame routes to the right index. The verdict is [`minimap_interior`]'s — the SAME
/// one [`emit_minimap`] draws by, because the client keeps one flag for both (it used to read the
/// camera-eye `CurrentWmoInterior` instead, which is a different ray *and*, since 1466, a different
/// mask: the buttons would have kept driving the indoor index on a Stormwind street the map was
/// drawing from terrain tiles).
/// The state is pushed on the inside↔outside *edge* and whenever the VM's Minimap-creation count
/// moved (the cluster XML loads late, and an addon can build one whenever it likes — the counter
/// is what keeps "a widget created after the last transition is still told" true without walking
/// the ~3k-frame arena every frame; the memo resets with a fresh VM, whose rebuilt widgets bump
/// the fresh counter). On the edge we also fire `MINIMAP_UPDATE_ZOOM`: the active zoom index just switched
/// to the other (independent) level, so the cluster must re-sync the +/- buttons' enabled state to it
/// — the client's own signal for "the effective zoom changed" (FrameXML `Minimap_OnEvent`). Without it
/// the buttons keep the level you left (e.g. `ZoomIn` greyed from an outdoor max-zoom, still greyed
/// indoors at level 3), which is the director's report (2026-07-09).
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

/// Swap the disc's mask when an addon has asked for a different one — `Minimap:SetMaskTexture`.
///
/// The renderer already masks the map through its own `UiQuadMask`; this only decides which
/// texture that is, so the whole feature is a load and a handle swap. It is what makes pfUI's
/// square minimap — the single most recognisable thing it does to the default UI — actually
/// square, and it is a real 1.12 method rather than a pfUI invention.
///
/// Memoised on the path, so the load happens on the edge and never per frame; a VM restart resets
/// the memo and re-reads, because the fresh VM's widget starts at the engine default again. A path
/// that fails to load leaves the current mask standing and says so once — a missing file must not
/// silently unmask the map into a square, which is what dropping the handle would do.
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

/// Push the player's world facing onto the Minimap's engine-owned player-arrow `Model` — the
/// client's `CMinimap::SetPlayerFacing 0x4eb8e0`, which writes it verbatim into the arrow's
/// `[+0x39c]`, the field `Model:GetFacing()` reads.
///
/// **Every frame, not on an edge**, because the value is continuous: the reference calls it from
/// the per-frame minimap update with the player's live `GetFacing()`, and an addon polling it in an
/// OnUpdate (which is what reads it — Questie's `GetPlayerFacing`, pfQuest's `compat/client.lua`)
/// would otherwise steer off a stale heading. It costs one arena lookup per Minimap widget, which
/// is what `WidgetArena::minimap_kinds` exists to make true.
///
/// **The one transform, and why it is not a fudge.** `player.facing()` is benilla's `face_yaw`, an
/// unbounded accumulator — a mouse-turn just adds to it — whereas the client's unit orientation is
/// a normalized field, and the same normalization already happens on the wire (`movement_net`'s
/// `[0, 2π)` rule, which vmangos's `VerifyMovementInfo` requires). So the value published here is
/// the wire value, and an addon reading it back sees the same number the server does.
fn feed_minimap_player_facing(
    script: Option<bevy::ecs::system::NonSendMut<benilla_ui::script::UiScript>>,
    player: Res<Player>,
) {
    let Some(mut script) = script else { return };
    script.set_minimap_player_facing(player.facing().rem_euclid(std::f32::consts::TAU));
}

/// Push the live game clock into the VM when the game minute ticks — `GetGameTime()`'s backing
/// globals (the zone-text family's shape). The reference's GameTimeFrame re-reads GetGameTime
/// every OnUpdate and compares the packed minute against its cached `timeOfDay`, so a
/// minute-granular push is exactly the API's own resolution (the binding returns no seconds).
/// Before the first `SMSG_LOGIN_SETTIMESPEED` the globals stay at their 0:00 stdlib seed.
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

/// The app half of the `<Minimap>` widget (decision 0203 phase 1) — see the module doc. The zone
/// LABEL feed lives with the rest of the zone-text data plane (`crate::area`, decision 0287's
/// fold-back): the client updates the minimap line and fires `MINIMAP_ZONE_CHANGED` from the same
/// area-update pass as the ZONE_CHANGED family (`0x494970` beside `0x494780`), and so does
/// benilla.
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
                    // Deliberately NOT on the lighting resolve's read side, though it reads
                    // `WowLighting`: joining `LightingConsumeSet` would put a
                    // 178th engine item through the world API wall, and what it buys is one
                    // frame of the right day-night tint on a 140 px map — invisible even on a
                    // submersion crossing, which is the one moment that value jumps.
                    emit_minimap.in_set(UiQuadAppend),
                    // After the emit that fills it: the composite camera draws what THIS frame's
                    // interior branch asked for, so the target the blit quad samples is never a
                    // frame behind the pan.
                    composite::drive_composite.after(UiQuadAppend),
                    // Before the script tick, so a zoom button pressed this frame routes to the
                    // indoor/outdoor index that matches where the player actually is.
                    feed_minimap_inside.in_set(crate::ui_script::UiFeed),
                    // Before the script tick, so an addon's OnUpdate steers off this frame's
                    // heading rather than last frame's.
                    feed_minimap_player_facing.in_set(crate::ui_script::UiFeed),
                    // After the tick that can call `Minimap:SetMaskTexture`, before the emit that
                    // reads `MinimapAssets::mask`: a mask set this frame is the one this frame
                    // draws with.
                    feed_minimap_mask
                        .after(crate::ui_script::UiInput)
                        .before(UiQuadAppend),
                    // Before the script tick, and after the containment verdict it reads: the
                    // `MINIMAP_PING` event and `Minimap:GetPingPosition()`'s value land in the
                    // same tick, on a ping the renderer already drew at the end of last frame.
                    // Gated on the interface being up: a group member's
                    // `MSG_MINIMAP_PING` can land in the same drain as the login burst, and the
                    // `fresh` latch it sets is spent by this system's take — on the boot VM, with
                    // no `MiniMapPing` frame to show it, if this ran in 2214's one-frame window.
                    // Gated, the latch simply waits for the first frame with an interface.
                    ping::drive_minimap_ping
                        .after(feed_minimap_inside)
                        .in_set(crate::ui_script::UiFeed)
                        .run_if(crate::ui_script::ingame_ui_up),
                    // Before the script tick, so GameTimeFrame's OnUpdate reads this frame's
                    // minute, not last frame's.
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
        // The client's no-light default: ambient = gray 0x40 (≈0.251), diffuse = white. The tint is
        // ≈0.79, NOT 1.0 — i.e. drawing tiles at flat white is ≈1.27× too bright (the director's
        // report). 0.25·1 + 0.75·lerp(0.251, 1, (64+96)/256=0.625) = 0.25 + 0.75·0.719 ≈ 0.789.
        let t = minimap_day_tint([0.251; 3], [1.0; 3]);
        approx(t, [0.789; 3]);
        assert!(t[0] < 0.95, "flat white would be too bright");
    }

    #[test]
    fn pitch_black_light_still_tints_partway_to_white() {
        // The +96 luma floor keeps the map dimly visible even with zero light: 0.75·(0+1·0.375) ≈ 0.28.
        approx(minimap_day_tint([0.0; 3], [0.0; 3]), [0.281; 3]);
    }
}
