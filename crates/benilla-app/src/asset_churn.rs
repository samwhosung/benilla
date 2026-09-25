//! `WOW_ASSET_CHURN=1`: once a second, logs per asset type the Added/Modified events per frame,
//! the image megabytes those modifications re-upload, and the images touched in the most frames.
//! Sizes come from the `texture_descriptor`: a `RENDER_WORLD` image's main-side `data` is `None`
//! after extract.

use std::collections::HashMap;

use benilla_assets::image_gpu_bytes;
use bevy::asset::AssetEvent;
use bevy::prelude::*;
use bevy::time::Real;

pub(crate) struct AssetChurnPlugin;

/// Off unless `WOW_ASSET_CHURN=1`, read once; when off the plugin adds no systems.
pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_ASSET_CHURN").as_deref() == Ok("1"))
}

impl Plugin for AssetChurnPlugin {
    fn build(&self, app: &mut App) {
        if !enabled() {
            return;
        }
        app.init_resource::<Churn>()
            .add_systems(Update, (watch_images, watch_meshes, report).chain());
    }
}

#[derive(Resource, Default)]
struct Churn {
    frames: u32,
    /// asset type -> (added, modified) event totals this window.
    counts: HashMap<&'static str, (u64, u64)>,
    /// Per image: distinct frames modified, not events, so the ratio never exceeds 1.
    images: HashMap<AssetId<Image>, ImageChurn>,
    /// Image bytes modification asks the render world to re-upload, this window.
    image_bytes: u64,
    last_report: f32,
}

/// One image's churn within the reporting window.
#[derive(Clone, Copy, Default)]
struct ImageChurn {
    /// Distinct frames this image was modified in.
    frames: u64,
    /// The last frame counted, so several events in one frame count once.
    last_frame: Option<u32>,
    /// Its GPU size when last seen.
    bytes: usize,
}

fn watch_images(
    mut events: MessageReader<AssetEvent<Image>>,
    images: Res<Assets<Image>>,
    mut churn: ResMut<Churn>,
) {
    let churn = churn.as_mut();
    for e in events.read() {
        match *e {
            AssetEvent::Added { .. } => churn.counts.entry("Image").or_default().0 += 1,
            AssetEvent::Modified { id } => {
                churn.counts.entry("Image").or_default().1 += 1;
                let bytes = images.get(id).map_or(0, image_gpu_bytes);
                churn.image_bytes += bytes as u64;
                let frame = churn.frames;
                let slot = churn.images.entry(id).or_default();
                if slot.last_frame != Some(frame) {
                    slot.last_frame = Some(frame);
                    slot.frames += 1;
                }
                slot.bytes = bytes;
            }
            _ => {}
        }
    }
}

fn watch_meshes(mut events: MessageReader<AssetEvent<Mesh>>, mut churn: ResMut<Churn>) {
    for e in events.read() {
        match *e {
            AssetEvent::Added { .. } => churn.counts.entry("Mesh").or_default().0 += 1,
            AssetEvent::Modified { .. } => churn.counts.entry("Mesh").or_default().1 += 1,
            _ => {}
        }
    }
}

fn report(mut churn: ResMut<Churn>, time: Res<Time<Real>>, images: Res<Assets<Image>>) {
    churn.frames += 1;
    if time.elapsed_secs() - churn.last_report < 1.0 {
        return;
    }
    let churn = churn.as_mut();
    churn.last_report = time.elapsed_secs();
    let frames = f64::from(churn.frames.max(1));
    let mut line = format!("[asset-churn] over {} frames:", churn.frames);
    let mut kinds: Vec<_> = churn.counts.iter().collect();
    kinds.sort_by_key(|(k, _)| *k);
    for (kind, (added, modified)) in kinds {
        line.push_str(&format!(
            " {kind}: +{:.1}/frame ~{:.1}/frame",
            *added as f64 / frames,
            *modified as f64 / frames
        ));
    }
    line.push_str(&format!(
        " · image re-upload {:.2} MB/frame",
        churn.image_bytes as f64 / frames / (1024.0 * 1024.0)
    ));
    info!("{line}");

    // The offenders: an asset touched in most frames is a per-frame rewrite, whatever its size.
    let mut top: Vec<_> = churn.images.iter().map(|(id, v)| (*id, *v)).collect();
    top.sort_by_key(|(_, c)| std::cmp::Reverse((c.frames, c.bytes)));
    for (
        id,
        ImageChurn {
            frames: n, bytes, ..
        },
    ) in top.into_iter().take(6)
    {
        let dims = images.get(id).map_or((0, 0), |i| {
            (
                i.texture_descriptor.size.width,
                i.texture_descriptor.size.height,
            )
        });
        info!(
            "[asset-churn]   image {id:?} touched in {n}/{} frames · {}x{} · {:.2} MB",
            churn.frames,
            dims.0,
            dims.1,
            bytes as f64 / (1024.0 * 1024.0)
        );
    }
    churn.frames = 0;
    churn.counts.clear();
    churn.images.clear();
    churn.image_bytes = 0;
}
