//! Off-world light blobs: the portrait booths' studio light, the body panes' light and the glue
//! scene's rig light, each written into a buffer of its own against the model shaders' std430
//! struct. Producers state values and never a row index, so a layout change cannot strand them.

use bevy::prelude::*;
use bevy::render::render_resource::{Buffer, BufferDescriptor, BufferUsages};
use bevy::render::renderer::{RenderDevice, RenderQueue};

use super::global_light::{
    commit_raw, light_blob_bytes, pack_model_core_rows, LIGHT_HEADER_ROWS, MAX_POINT_LIGHTS,
};
use super::prop_probes::prop_probe_region_offset;
use super::sh::prop_probe_coeffs;

/// Row 3 with the terrain shininess (20) in `.w`; models never read it.
const SPEC_ROW: [f32; 4] = [0.0, 0.0, 0.0, 20.0];
/// Fog rows with the fog off and an inert farclip wall.
const NO_FOG: ([f32; 4], [f32; 4]) = ([0.0; 4], [0.0, 10_000.0, 0.0, 10_000.0]);

/// An off-world light blob: the header rows, an optional point table and an optional interior
/// probe. Start at [`LightBlob::model`] and state only what differs from a plain lit scene.
pub struct LightBlob {
    rows: [[f32; 4]; LIGHT_HEADER_ROWS],
    points: Vec<[f32; 4]>,
    probe: Option<[Vec4; 7]>,
}

impl LightBlob {
    /// A plain model-lit blob: fog off, no point lights, no probe. `sun_dir` is the direction the
    /// light travels.
    pub fn model(ambient: [f32; 3], diffuse: [f32; 3], sun_dir: Vec3) -> Self {
        let mut rows = [[0.0f32; 4]; LIGHT_HEADER_ROWS];
        pack_model_core_rows(&mut rows, ambient, diffuse, sun_dir);
        rows[3] = SPEC_ROW;
        (rows[4], rows[5]) = NO_FOG;
        Self {
            rows,
            points: Vec::new(),
            probe: None,
        }
    }

    /// Fog with near 0: the reference's off-world fog (`CharModelFogInfo`, `AccountLogin.xml`)
    /// states a colour and a far and nothing else.
    pub fn fog(self, rgb: [f32; 3], far: f32, on: bool) -> Self {
        self.fog_span(rgb, 0.0, far, on)
    }

    /// Fog with an explicit near, where the clamped ramp `(far − eye_z)/(far − near)` starts; only
    /// the `<Model>` widget's `SetFogNear` states one. The farclip wall stays inert: it discards,
    /// and an off-world scene has nothing to clip.
    pub fn fog_span(mut self, rgb: [f32; 3], near: f32, far: f32, on: bool) -> Self {
        self.rows[4] = [rgb[0], rgb[1], rgb[2], if on { 1.0 } else { 0.0 }];
        self.rows[5] = [near, far, 0.0, 10_000.0];
        self
    }

    /// The exterior-intensity dial, row 19 `.w`, which no shader reads; the booths state 0.4 and
    /// the glue rig 1.0.
    pub fn dial(mut self, v: f32) -> Self {
        self.rows[19] = [0.0, 0.0, 0.0, v];
        self
    }

    /// Adds a point light, its colour committed raw by [`commit_raw`]. Past the table's capacity a
    /// light is dropped with a warning, never written over the probe region that follows.
    pub fn point(mut self, pos: Vec3, range: f32, color: [f32; 3]) -> Self {
        if self.points.len() / 2 >= MAX_POINT_LIGHTS {
            warn!("light blob: over {MAX_POINT_LIGHTS} point lights — dropping the rest");
            return self;
        }
        let c = commit_raw(color);
        self.points.push([pos.x, pos.y, pos.z, range]);
        self.points.push([c[0], c[1], c[2], 0.0]);
        self.rows[20][0] = (self.points.len() / 2) as f32;
        self
    }

    /// Folds an interior SH probe into slot 0, which the rig lane reads (booth instances carry
    /// `MeshTag` 0); `lobes` are toward-light unit vectors with their committed colours.
    pub fn probe(mut self, ambient: [f32; 3], lobes: &[(Vec3, [f32; 3])]) -> Self {
        self.probe = Some(prop_probe_coeffs(ambient, lobes));
        self
    }

    /// The probe's DC term per channel, for logging what was folded.
    pub fn probe_dc(&self) -> [f32; 3] {
        let p = self.probe.unwrap_or_default();
        [p[0].w, p[1].w, p[2].w]
    }

    /// The packed header rows, for tests and diagnostics; building a blob never needs an index.
    pub fn header_rows(&self) -> &[[f32; 4]] {
        &self.rows
    }

    /// How many point lights the table carries.
    pub fn point_count(&self) -> usize {
        self.points.len() / 2
    }

    /// A buffer of the full layout's size, never just what was written: wgpu validates the bound
    /// size against `wow_model.wgsl`'s whole struct at every draw, and zeroes the unwritten rest.
    pub fn create(&self, device: &RenderDevice, label: &'static str) -> Buffer {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: light_blob_bytes(),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Writes the header rows and the point table in one write and the probe region, past the
    /// per-frame prefix, in another.
    pub fn write(&self, queue: &RenderQueue, buffer: &Buffer) {
        let mut head = self.rows.to_vec();
        head.extend_from_slice(&self.points);
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(&head));
        if let Some(probe) = self.probe {
            let rows: [[f32; 4]; 7] = probe.map(|v| v.to_array());
            queue.write_buffer(
                buffer,
                prop_probe_region_offset(),
                bytemuck::cast_slice(&rows),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_model_blob_carries_the_off_world_defaults() {
        let b = LightBlob::model([0.5; 3], [0.8; 3], Vec3::NEG_Y);
        assert_eq!(b.rows[3], SPEC_ROW);
        assert_eq!((b.rows[4], b.rows[5]), NO_FOG);
        assert_eq!(b.rows[20][0], 0.0, "no point lights stated");
        assert_eq!(b.point_count(), 0);
        assert!(b.probe.is_none());
        // The lit lanes come from the shared packer; a blob without them renders black portraits.
        assert_eq!(b.rows[0][..3], [0.5; 3], "ambient");
        assert_eq!(b.rows[1][..3], [0.8; 3], "diffuse");
    }

    /// A point light lands as two interleaved rows with its colour raw, over-gamut kept
    /// (`0x71ca80` → `0x593040`) and only negatives floored, and the header counts it.
    #[test]
    fn a_point_light_commits_raw_and_counts_itself() {
        let b = LightBlob::model([0.0; 3], [0.0; 3], Vec3::NEG_Y).point(
            Vec3::new(1.0, 2.0, 3.0),
            1.0e6,
            [1.75, 2.0, -0.5],
        );
        assert_eq!(b.point_count(), 1);
        assert_eq!(b.rows[20][0], 1.0);
        assert_eq!(b.points[0], [1.0, 2.0, 3.0, 1.0e6]);
        assert_eq!(
            b.points[1],
            [1.75, 2.0, 0.0, 0.0],
            "raw kept, negative floored"
        );
    }

    #[test]
    fn the_point_table_stops_at_capacity() {
        let mut b = LightBlob::model([0.0; 3], [0.0; 3], Vec3::NEG_Y);
        for _ in 0..MAX_POINT_LIGHTS + 32 {
            b = b.point(Vec3::ZERO, 1.0, [1.0; 3]);
        }
        assert_eq!(b.point_count(), MAX_POINT_LIGHTS);
        assert_eq!(b.points.len(), 2 * MAX_POINT_LIGHTS);
    }

    /// The glue scene's fog toggle flips only the enable and leaves the race's fog rows standing.
    #[test]
    fn fog_states_its_rows_whether_or_not_it_is_enabled() {
        let on =
            LightBlob::model([0.0; 3], [0.0; 3], Vec3::NEG_Y).fog([0.1, 0.2, 0.3], 400.0, true);
        assert_eq!(on.rows[4], [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(on.rows[5], [0.0, 400.0, 0.0, 10_000.0]);
        let off =
            LightBlob::model([0.0; 3], [0.0; 3], Vec3::NEG_Y).fog([0.1, 0.2, 0.3], 400.0, false);
        assert_eq!(off.rows[4], [0.1, 0.2, 0.3, 0.0]);
        assert_eq!(off.rows[5], on.rows[5]);
    }

    /// A blob without a probe reports a zero DC rather than panicking.
    #[test]
    fn the_probe_dc_reports_the_fold() {
        let b = LightBlob::model([0.0; 3], [0.0; 3], Vec3::NEG_Y).probe([0.2, 0.3, 0.4], &[]);
        assert_eq!(
            b.probe_dc(),
            [0.2, 0.3, 0.4],
            "ambient-only fold: DC = ambient"
        );
        let none = LightBlob::model([0.0; 3], [0.0; 3], Vec3::NEG_Y);
        assert_eq!(none.probe_dc(), [0.0; 3]);
    }
}
