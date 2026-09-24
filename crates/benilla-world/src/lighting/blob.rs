//! Building an **off-world light blob** — the portrait booths' frozen studio light, the body
//! panes' reference light, the glue scene's authored-rig light.
//!
//! The shared world light is built by [`super::global_light`] on the frame path. These three are
//! not on that path: they are written once (or once per scene swap) into a buffer of their own,
//! and they all bind against the *same* std430 struct the model shaders declare. That struct's
//! layout is the thing they must not know.
//!
//! [`super::global_light::pack_model_core_rows`] already made half this argument and says so in
//! its own doc — *"a producer that copies the layout goes stale the day the layout moves — so
//! producers don't copy it, they call this"* — after the booth rendered black portraits the day
//! 0354 moved the lit lanes onto rows it never wrote. But it packs only the lit lanes. Every
//! producer still hand-wrote the spec row, the two fog rows, the intensity dial, the point-table
//! count, the point rows' interleave, and the byte offset of the probe region. Six pieces of the
//! same layout, copied three times, one 0354 away from the same failure. This is the rest of that
//! argument: the producers state values, and nothing outside this module states a row index.

use bevy::prelude::*;
use bevy::render::render_resource::{Buffer, BufferDescriptor, BufferUsages};
use bevy::render::renderer::{RenderDevice, RenderQueue};

use super::global_light::{
    commit_raw, light_blob_bytes, pack_model_core_rows, LIGHT_HEADER_ROWS, MAX_POINT_LIGHTS,
};
use super::prop_probes::prop_probe_region_offset;
use super::sh::prop_probe_coeffs;

/// Row 3 `.w` — the terrain shininess convention. Models never read it; every producer writes it
/// so a terrain lane that ever binds an off-world blob degrades sanely.
const SPEC_ROW: [f32; 4] = [0.0, 0.0, 0.0, 20.0];
/// Fog rows with fog OFF: colour black, `.w = 0` (disabled), and a farclip wall far enough away
/// to be inert. The off-world default — a booth has no distance to fog.
const NO_FOG: ([f32; 4], [f32; 4]) = ([0.0; 4], [0.0, 10_000.0, 0.0, 10_000.0]);

/// An off-world light blob under construction: the header rows, an optional point table and an
/// optional interior-probe fold, sized and written as one.
///
/// Start at [`LightBlob::model`] and state only what differs from a plain lit scene.
pub struct LightBlob {
    rows: [[f32; 4]; LIGHT_HEADER_ROWS],
    points: Vec<[f32; 4]>,
    probe: Option<[Vec4; 7]>,
}

impl LightBlob {
    /// A model-lit blob: the core lit lanes folded from `(ambient, diffuse, sun_dir)`, spec at the
    /// terrain convention, fog off, no point lights, no probe.
    ///
    /// `sun_dir` is the light's **propagation** direction (the packer negates it back into a
    /// to-light vector for the SH fold).
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

    /// Fog: colour, far distance, and whether the shader applies it. Near is always 0 and the
    /// farclip wall stays inert — the reference's off-world fog rows (`CharModelFogInfo` and
    /// `AccountLogin.xml`) carry a far and nothing else.
    pub fn fog(self, rgb: [f32; 3], far: f32, on: bool) -> Self {
        self.fog_span(rgb, 0.0, far, on)
    }

    /// Fog with an explicit **near**, the ramp's low end (`fog_params.x`): the factor is
    /// `(far − eye_z)/(far − near)` clamped, so near is where the fog first bites and not a
    /// cosmetic. The one producer that can state one is the `<Model>` widget's `SetFogNear`
    /// (decision 2027) — every other off-world blob leaves it at 0, which is what
    /// [`Self::fog`] passes and what `CharModelFogInfo`'s own `SetFogNear(0)` says.
    /// The farclip wall (`.w`) stays inert at `10_000`: it is a per-pixel *discard*, not a fade,
    /// and an off-world scene has no clip distance to enforce.
    pub fn fog_span(mut self, rgb: [f32; 3], near: f32, far: f32, on: bool) -> Self {
        self.rows[4] = [rgb[0], rgb[1], rgb[2], if on { 1.0 } else { 0.0 }];
        self.rows[5] = [near, far, 0.0, 10_000.0];
        self
    }

    /// The exterior-intensity dial (row 19 `.w`).
    ///
    /// **Currently inert: no shader reads it.** It survives because the three producers disagree
    /// about its value (0.4 in the booths, 1.0 on the glue rig) and that disagreement is recorded
    /// history, not noise — see the callers' comments. Stating it keeps their bytes identical;
    /// when a portrait-light pass settles the question, it dies here, once.
    pub fn dial(mut self, v: f32) -> Self {
        self.rows[19] = [0.0, 0.0, 0.0, v];
        self
    }

    /// Add a point light: position, range, and its diffuse colour **before** the raw commit (the
    /// over-gamut commit law is applied here — see [`commit_raw`]).
    ///
    /// Silently capped at the table's capacity; past it a light is dropped rather than written
    /// over the probe region that follows.
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

    /// Fold an interior SH probe into slot 0 — the rig lane's read side (booth instances carry
    /// `MeshTag` 0). `lobes` are toward-light unit vectors with their committed colours.
    pub fn probe(mut self, ambient: [f32; 3], lobes: &[(Vec3, [f32; 3])]) -> Self {
        self.probe = Some(prop_probe_coeffs(ambient, lobes));
        self
    }

    /// The probe's DC term per channel — for logging what was folded, not for the shader.
    pub fn probe_dc(&self) -> [f32; 3] {
        let p = self.probe.unwrap_or_default();
        [p[0].w, p[1].w, p[2].w]
    }

    /// The packed header rows — for tests pinning a producer's values, and for diagnostics. The
    /// row *indices* are this module's business; a caller that indexes one is asserting about a
    /// lane, not building a blob.
    pub fn header_rows(&self) -> &[[f32; 4]] {
        &self.rows
    }

    /// How many point lights the table carries.
    pub fn point_count(&self) -> usize {
        self.points.len() / 2
    }

    /// Create a buffer this blob fits in.
    ///
    /// Sized to the **full** layout, never to what was written: `wow_model.wgsl` declares the
    /// whole struct and wgpu validates the bound size against it at every draw. wgpu zeroes the
    /// rest, so an unwritten table reads as empty.
    pub fn create(&self, device: &RenderDevice, label: &'static str) -> Buffer {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: light_blob_bytes(),
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Write the blob into `buffer`. Header rows and the point table are contiguous, so they go in
    /// one write; the probe region sits past the per-frame prefix and gets its own.
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

    /// The off-world defaults every producer used to hand-write: spec at the terrain convention,
    /// fog off with an inert farclip wall, an empty point table.
    #[test]
    fn a_plain_model_blob_carries_the_off_world_defaults() {
        let b = LightBlob::model([0.5; 3], [0.8; 3], Vec3::NEG_Y);
        assert_eq!(b.rows[3], SPEC_ROW);
        assert_eq!((b.rows[4], b.rows[5]), NO_FOG);
        assert_eq!(b.rows[20][0], 0.0, "no point lights stated");
        assert_eq!(b.point_count(), 0);
        assert!(b.probe.is_none());
        // The lit lanes are the packer's, not ours — pinned here only so a producer that stops
        // getting them is loud (0354 rendered black portraits exactly this way).
        assert_eq!(b.rows[0][..3], [0.5; 3], "ambient");
        assert_eq!(b.rows[1][..3], [0.8; 3], "diffuse");
    }

    /// A point light lands as the table's two interleaved rows, colour committed RAW —
    /// over-gamut preserved (`0x71ca80` → `0x593040`) — and the header's count follows it.
    /// Negative channels are the one thing the commit floors.
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

    /// Past the table's capacity a light is dropped, never written over the probe region that
    /// follows it in the buffer.
    #[test]
    fn the_point_table_stops_at_capacity() {
        let mut b = LightBlob::model([0.0; 3], [0.0; 3], Vec3::NEG_Y);
        for _ in 0..MAX_POINT_LIGHTS + 32 {
            b = b.point(Vec3::ZERO, 1.0, [1.0; 3]);
        }
        assert_eq!(b.point_count(), MAX_POINT_LIGHTS);
        assert_eq!(b.points.len(), 2 * MAX_POINT_LIGHTS);
    }

    /// Fog states colour and far independently of whether the shader applies it — the glue
    /// scene's `fog` toggle flips the enable and leaves the race's authored rows standing.
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

    /// The probe's DC is the fold's, reported for the log — and a blob without one reads zero
    /// rather than panicking.
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
