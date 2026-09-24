//! Clouds — the procedural sky cloud subsystem.
//!
//! One coverage field serves every consumer, exactly like the reference: the [`kernel`] maintains
//! the scrolling 128² byte tile (4-octave toroidal value noise, thresholded by the authored
//! Light.dbc cloud density `C`, tone-curved), the flare gate samples it for the cloud occlusion
//! `occ1` (sun `1−R`, moon the thin-cloud tent), and the visible [`layer`] renders the same bytes
//! as a dome texture. A cloud drifting over the sun therefore dims the glare *and* is the cloud
//! you see — they cannot desynchronize.
//!
//! `C` rides [`WowLighting::cloud_density`] (LightFloatBand sub-3, weather/underwater blends
//! included), so storms with an authored overcast band raise the coverage — and with it the
//! glare dimming — with no extra wiring.
//!
//! **Capture mode freezes the field's clock**: the tile becomes a pure function of `C` (a full
//! rebuild whenever it changes, no phase drift), so capture fixtures stay bit-identical across
//! runs — the settle-frame count varies with real frame timing, and a live phase would smear a
//! few bytes per run. Live runs keep the reference's ~10 Hz scroll + slow morph.

use bevy::pbr::MaterialPlugin;
use bevy::prelude::*;

use crate::lighting::WowLighting;
use benilla_assets::AssetSet;

mod kernel;
mod layer;

pub use layer::CloudMaterial;
mod tables;

pub use kernel::{occ1_moon, occ1_sun};

/// The live cloud coverage field. Read [`CloudCoverage::coverage`] with a camera-relative offset
/// (e.g. the glare's 12-unit sky point) to get `R ∈ [0,1]` at that sky position.
#[derive(Resource)]
pub struct CloudCoverage {
    kernel: kernel::CloudKernel,
    /// Whether the first full rebuild ran (the reference full-rebuilds on init, then scrolls
    /// incrementally — `0x6cff90` vs `0x6cffc0`).
    primed: bool,
    /// Capture mode: freeze the phase clock; rebuild only when `C` changes (see module docs).
    frozen: bool,
    /// The density the tile was last built with (the frozen-mode rebuild trigger).
    last_density: f32,
    /// The color-pass inputs of the last build (the frozen-mode recolor trigger).
    last_frame: Option<kernel::CloudFrame>,
}

impl Default for CloudCoverage {
    fn default() -> Self {
        CloudCoverage {
            kernel: kernel::CloudKernel::default(),
            primed: false,
            frozen: std::env::var_os("WOW_CAPTURE").is_some(),
            last_density: 0.0,
            last_frame: None,
        }
    }
}

impl CloudCoverage {
    /// Coverage `R` toward camera-relative offset `d` (Bevy frame, +Y up).
    pub fn coverage(&self, d: Vec3) -> f32 {
        self.kernel.coverage(d)
    }
}

pub struct CloudsPlugin;

impl Plugin for CloudsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<layer::CloudMaterial>::default())
            .init_resource::<CloudCoverage>()
            .add_systems(Startup, layer::setup_cloud_layer.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // After the submersion verdict: the surfacing edge (wet→dry) must fire the
                    // full rebuild the SAME frame the dome un-hides, or the first visible frame
                    // still shows the submerged-era tile.
                    // …and on the READ side of the lighting resolve: the surfacing rebuild
                    // below takes its density and palette from `WowLighting`, so an unordered
                    // tick rebuilds the field from the atmosphere we just LEFT — the underwater
                    // one, whose cloud density is 0.0 in Stranglethorn.
                    tick_clouds
                        .after(crate::liquid::SubmersionVerdict)
                        .in_set(crate::lighting::LightingConsumeSet),
                    // After the resolve: the dome and the painted skybox must agree WITHIN a frame,
                    // or one frame draws both (the ordering `crate::sky`'s gate already takes) —
                    // and after the submersion verdict, for the same one-frame-agreement reason.
                    layer::apply_cloud_visibility
                        .after(crate::skybox::SkyboxResolve)
                        .after(crate::liquid::SubmersionVerdict),
                ),
            )
            // Camera-anchored placement post-propagation, like the sky dome.
            .add_systems(
                PostUpdate,
                layer::follow_cloud_dome.in_set(crate::billboard::BillboardPlace),
            );
    }
}

/// Advance the coverage field — a full rebuild the first frame, then the reference's ~10 Hz
/// incremental 32-row band scroll (or the frozen capture clock) — and re-upload the colored
/// RGBA texels when they changed (the reference's per-regen `0x58ac70` upload of the `0x6cfb00`
/// color buffer).
fn tick_clouds(
    mut cov: ResMut<CloudCoverage>,
    light: Res<WowLighting>,
    time: Res<Time>,
    clock: Option<Res<layer::CloudLayer>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<layer::CloudMaterial>>,
    underwater: Res<crate::liquid::Underwater>,
    mut was_submerged: Local<bool>,
) {
    // The surfacing edge — the reference's wet→dry detector `0x680ac3` fires `0x6d2210(1)`, the
    // FULL-rebuild selector (`bl!=0` → `0x6cff90`), the frame the eye leaves the liquid.
    // Without it the incremental 32-row scroll restores the field band-by-band over ~0.4 s+ —
    // the sky pass returns instantly (0902's gate) but the clouds creep back in behind it,
    // the "delayed pop-in" the director sighted. Dry→wet takes no edge, exactly like the
    // reference: the sky pass is skipped while submerged, so nobody sees the tile.
    let submerged = underwater.0.any();
    let surfaced = *was_submerged && !submerged;
    *was_submerged = submerged;
    let density = light.cloud_density;
    // The color-pass inputs (`0x6cfb00`'s per-frame setup), from the resolved lighting.
    let frame = kernel::CloudFrame {
        sun: light.cloud_colors[0],
        slope: light.cloud_colors[1],
        gbase: light.cloud_colors[2],
        bcc: light.storm_bcc,
        glow_dir: light.cloud_glow_dir,
        glow_track: light.cloud_glow_track,
    };
    static CLOUD_DUMP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let cloud_dump = *CLOUD_DUMP.get_or_init(|| std::env::var_os("WOW_CLOUD_DUMP").is_some());
    if surfaced && cloud_dump {
        eprintln!("[cloud] surfaced -> full rebuild (C {density:.3})");
    }
    if cloud_dump && cov.last_frame != Some(frame) {
        eprintln!(
            "[cloud] C {density:.3} sun {:?} slope {:?} gbase {:?} bcc {:.2} glow_dir {:?} track {:.2}",
            frame.sun, frame.slope, frame.gbase, frame.bcc, frame.glow_dir, frame.glow_track
        );
    }
    let changed = if !cov.primed || surfaced || (cov.frozen && density != cov.last_density) {
        cov.primed = true;
        cov.last_density = density;
        cov.last_frame = Some(frame);
        cov.kernel.rebuild(density, &frame);
        true
    } else if cov.frozen {
        // The frozen capture clock: the coverage stays put; recolor only when the palette /
        // glow / weather inputs moved (deterministic — the scenario pins them).
        if cov.last_frame != Some(frame) {
            cov.last_frame = Some(frame);
            cov.kernel.recolor(&frame);
            true
        } else {
            false
        }
    } else {
        cov.kernel.tick(time.delta_secs(), density, &frame)
    };
    if changed {
        if let Some(layer) = clock.as_ref() {
            if let Some(image) = images.get_mut(&layer.image) {
                image
                    .data
                    .as_mut()
                    .expect("cpu-side cloud image")
                    .copy_from_slice(cov.kernel.rgba().as_flattened());
                // Touch the material so its bind group rebuilds against the re-uploaded
                // texture (a modified Image gets a NEW GpuImage; a stale bind group keeps
                // sampling the first upload forever).
                materials.get_mut(&layer.material);
            }
        }
    }
}
