//! The glyph cache in Bevy: building it, following the window, and uploading each new cell as a
//! sub-rect `RenderQueue::write_texture` into the sheet's texture, never through `Assets<Image>`
//! ([`crate::ui_text::pack`] says why).

use std::sync::{Arc, Mutex};

use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
};
use bevy::render::renderer::RenderQueue;
use bevy::render::texture::GpuImage;
use bevy::render::{ExtractSchedule, Render, RenderApp, RenderSystems};
use bevy::window::PrimaryWindow;

use benilla_assets::WorldAssets;

use super::{TextEngine, UiFontAtlas};
use crate::ui_text::pack::CellUpload;

/// Cells rasterized this frame, waiting for the render world.
#[derive(Resource, Default)]
struct GlyphUploadQueue(Vec<CellUpload>);

/// The render world's copy, which also keeps cells whose texture was not ready yet.
#[derive(Resource, Default)]
struct GlyphUploads(Vec<CellUpload>);

/// Loads the client's TTFs and drives the on-demand glyph cache.
pub(crate) struct UiTextPlugin;

impl Plugin for UiTextPlugin {
    fn build(&self, app: &mut App) {
        // `init` retries each Update until the patch chain and the window's real `scale_factor`
        // exist. `publish_sheet` runs in `Last`, after every producer of text quads, so a cell
        // rasterized this frame is queued before the render extract.
        app.init_resource::<GlyphUploadQueue>()
            .add_systems(Update, init)
            .add_systems(Last, publish_sheet);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<GlyphUploads>()
            .add_systems(ExtractSchedule, extract_glyph_uploads)
            .add_systems(
                Render,
                // After `prepare_assets::<GpuImage>`, so a sheet made this frame has its texture.
                upload_glyph_cells.in_set(RenderSystems::PrepareResources),
            );
    }
}

/// Build the engine on the first frame with both the patch chain and the window's `scale_factor`.
fn init(
    mut commands: Commands,
    world_assets: Option<Res<WorldAssets>>,
    images: Res<Assets<Image>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    existing: Option<Res<UiFontAtlas>>,
) {
    if existing.is_some() {
        return;
    }
    let (Some(world_assets), Ok(window)) = (world_assets, windows.single()) else {
        return;
    };
    let Some(engine) = TextEngine::load(&world_assets, &images, window.scale_factor()) else {
        return;
    };
    commands.insert_resource(UiFontAtlas {
        engine: Arc::new(Mutex::new(engine)),
        generation: 0,
        ellipsis: crate::ui_text::EllipsisMemo::default(),
    });
}

/// The frame boundary: follow the window's DPI, carry out a pending reset, create the sheet's
/// texture once, and hand this frame's cells to the render world. A reset waits for this boundary
/// so no UV moves while quads that use it are still being pushed.
fn publish_sheet(
    atlas: Option<ResMut<UiFontAtlas>>,
    mut images: ResMut<Assets<Image>>,
    mut queue: ResMut<GlyphUploadQueue>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Some(mut atlas) = atlas else {
        return;
    };
    let dpi = windows.single().map_or(1.0, Window::scale_factor);
    let (generation, dpi_moved) = {
        let mut e = atlas.lock();
        let dpi_moved = (e.dpi - dpi).abs() > 1e-6;
        e.dpi = dpi;
        if e.reset_pending {
            e.reset_pending = false;
            e.generation += 1;
            e.stats.resets += 1;
            e.chars.clear();
            e.cells.clear();
            e.sheet.reset();
        }
        let (announce, mut cells) = e.sheet.take_pending();
        if announce {
            // Inserted once on the reserved handle, never `get_mut` after: that would recreate
            // the texture and blank every glyph. A failed insert has no recovery.
            let _ = images.insert(e.sheet.handle().id(), crate::ui_text::pack::sheet_image());
        }
        queue.0.append(&mut cells);
        (e.generation, dpi_moved)
    };
    // A DPI change does stale the ellipsis memo: its answers fit a box at the old raster size.
    if dpi_moved {
        atlas.ellipsis = crate::ui_text::EllipsisMemo::default();
    }
    atlas.generation = generation;
    report_cache(&atlas);
}

/// Hand the frame's cells to the render world, appending to any a previous frame could not write.
fn extract_glyph_uploads(
    mut main_world: ResMut<bevy::render::MainWorld>,
    mut uploads: ResMut<GlyphUploads>,
) {
    if let Some(mut q) = main_world.get_resource_mut::<GlyphUploadQueue>() {
        uploads.0.append(&mut q.0);
    }
}

/// Write each new cell into the sheet's texture as a sub-rect, so the texture's identity never
/// changes; a cell whose texture is not prepared yet stays queued.
fn upload_glyph_cells(
    mut uploads: ResMut<GlyphUploads>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    render_queue: Res<RenderQueue>,
) {
    uploads.0.retain(|u| {
        let Some(gpu) = gpu_images.get(u.image) else {
            return true; // the texture is not up yet: retry next frame
        };
        render_queue.write_texture(
            TexelCopyTextureInfo {
                texture: &gpu.texture,
                mip_level: 0,
                origin: Origin3d {
                    x: u.x,
                    y: u.y,
                    z: 0,
                },
                aspect: TextureAspect::All,
            },
            &u.rgba,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(u.w * 4),
                rows_per_image: Some(u.h),
            },
            Extent3d {
                width: u.w,
                height: u.h,
                depth_or_array_layers: 1,
            },
        );
        false
    });
}

/// `WOW_GLYPH_CACHE=1`: one line a second of what the cache holds, the measurement that sizes
/// `super::pack::SHEET_SIZE`.
fn report_cache(atlas: &UiFontAtlas) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    if std::env::var_os("WOW_GLYPH_CACHE").is_none_or(|v| v == "0") {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    if LAST.swap(now, Ordering::Relaxed) == now {
        return;
    }
    let e = atlas.lock();
    let (used, total) = e.sheet.occupancy();
    eprintln!(
        "[glyph-cache] {used}/{total} texels ({:.1}%) · {} chars shaped · \
         {} cells rasterized · {} resets · dpi {}",
        100.0 * used as f64 / total as f64,
        e.stats.chars_shaped,
        e.stats.cells_rasterized,
        e.stats.resets,
        e.dpi,
    );
}
