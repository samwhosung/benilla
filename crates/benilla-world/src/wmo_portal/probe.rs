//! The portal-cull probe + trace instrument (instruments as first-class): the live
//! seed/visibility readout the debug panel shows, and the on-demand full-trace dump that turns a
//! director-found "it vanishes here" into an exact fixture (eye coordinates, seed evidence, and every
//! portal hop's verdict). The flood itself lives in the parent module; [`TraceLog`] records through
//! the [`FloodTrace`](super::FloodTrace) tap, so there is no second flood to drift.

use bevy::prelude::*;

use super::{
    floor_z_at, point_in_poly_2d, portal_poly, DownRaySeeds, FloodTrace, Rect, WmoModel, EXTERIOR,
    MAX_FLOOR_DROP,
};

/// The portal-cull probe (instruments as first-class): an on-demand full trace dump —
/// the "found a spot where it vanishes" loop: click dump at the broken spot, and the exact seed evidence
/// + per-portal verdicts land in a file the audit harness can replay as a fixture.
#[derive(Resource, Default)]
pub struct WmoCullProbe {
    /// Set by the panel's dump button: the file the next compute writes the trace to, then
    /// clears. **A path handed in, never one this crate builds.** `benilla-world` has no
    /// `local_state`, and the cwd-relative `target/wmo-cull-trace.txt` this used to write is the
    /// install folder the moment a player launches the binary from inside it — the one write in
    /// this crate, and the one 1486 forbids. The panel resolves the path under
    /// `benilla-config/Diagnostics/`; `WOW_CULLDUMP=<path>` is the headless spelling.
    pub dump_to: Option<std::path::PathBuf>,
    /// **The eye the PVS, the interior claim and the exterior windows were computed from** —
    /// the visibility authority's own pose, recorded so an instrument can compare it against the
    /// pose the frame actually draws from. Ordinary movement makes the two identical to within a
    /// centimetre; a snap is where they can disagree by the whole teleport.
    pub eye: Vec3,
}

/// The probe dump's recorder: the seed's evidence (floor faces + portal crossings under the eye's
/// column) plus every hop verdict, as text.
pub(crate) struct TraceLog {
    pub(crate) text: String,
}

impl TraceLog {
    pub(crate) fn new(model: &WmoModel, eye: [f32; 3], terrain_z: Option<f32>) -> Self {
        let mut text = format!(
            "wmo {}: eye local ({:.2}, {:.2}, {:.2})\n",
            model.wmo_id, eye[0], eye[1], eye[2]
        );
        // The terrain leg the WMO hit races: a surface above the eye was never crossed by the down
        // segment; one below it beats any WMO face deeper still, and the eye is over open ground.
        text.push_str(&match terrain_z {
            None => "  terrain under: none (off-tile, mid-decode, or an MCNK hole)\n".to_string(),
            Some(z) if z > eye[2] => format!("  terrain under: none (surface z={z:.2} is ABOVE the eye — not on the down segment)\n"),
            Some(z) => format!("  terrain under: z={z:.2} (beats any WMO face below it ⇒ outside)\n"),
        });
        for (gi, g) in model.group_nav.iter().enumerate() {
            text.push_str(&format!(
                "  g{gi:02} flags {:#07x}{} z[{:.2},{:.2}] refs[{}..+{}]\n",
                g.flags,
                if g.flags & EXTERIOR != 0 { " EXT" } else { "" },
                g.bbox_min[2],
                g.bbox_max[2],
                g.ref_start,
                g.ref_count
            ));
        }
        // The seed's evidence: the top collision face per group under the eye's column, and every
        // portal crossing (incl. near-parallel snap candidates) the down-ray's Leg B would consider.
        for (gi, tris) in model.group_collision_tris.iter().enumerate() {
            let mut best = f32::NEG_INFINITY;
            for tri in tris {
                if let Some(z) = floor_z_at(tri, eye[0], eye[1]) {
                    if z <= eye[2] && z > best {
                        best = z;
                    }
                }
            }
            if best > f32::NEG_INFINITY {
                text.push_str(&format!("  face under: g{gi:02} z={best:.2}\n"));
            }
        }
        for (gi, g) in model.group_nav.iter().enumerate() {
            let start = g.ref_start as usize;
            let end = (start + g.ref_count as usize).min(model.portal_refs.len());
            for r in &model.portal_refs[start..end] {
                let Some(info) = model.portal_infos.get(r.portal as usize) else {
                    continue;
                };
                let [nx, ny, nz, d] = info.plane;
                if nz.abs() < super::PORTAL_NEAR_PARALLEL {
                    // A vertical doorway: only the 0.1-yd embedded-in-plane snap can cross it.
                    let dist = nx * eye[0] + ny * eye[1] + nz * eye[2] + d;
                    if dist.abs() <= super::PORTAL_PLANE_SNAP {
                        text.push_str(&format!(
                            "  portal snap: p{} |d|={:.3} (g{gi:02} ref, nbr g{:02})\n",
                            r.portal,
                            dist.abs(),
                            r.group
                        ));
                    }
                    continue;
                }
                let z = -(nx * eye[0] + ny * eye[1] + d) / nz;
                if z > eye[2] || z < eye[2] - MAX_FLOOR_DROP {
                    continue;
                }
                if let Some(verts) = portal_poly(&model.portal_vertices, info) {
                    if point_in_poly_2d(verts.iter().map(|v| (v[0], v[1])), (eye[0], eye[1])) {
                        text.push_str(&format!(
                            "  portal crossing: p{} z={z:.2} (g{gi:02} ref, nbr g{:02})\n",
                            r.portal, r.group
                        ));
                    }
                }
            }
        }
        Self { text }
    }
}

impl FloodTrace for TraceLog {
    fn seed(&mut self, seeds: DownRaySeeds) {
        self.text.push_str(&format!(
            "  seed: in {:?} across {:?}\n",
            seeds.in_group, seeds.across
        ));
    }
    fn side_fail(&mut self, from: usize, portal: u16, to: usize, d: f32) {
        self.text.push_str(&format!(
            "  g{from:02} -x-> g{to:02} p{portal} SIDE d={d:+.3}\n"
        ));
    }
    fn rect_none(&mut self, from: usize, portal: u16, to: usize) {
        self.text.push_str(&format!(
            "  g{from:02} -x-> g{to:02} p{portal} RECT none (behind/off-screen)\n"
        ));
    }
    fn rect_collapse(&mut self, from: usize, portal: u16, to: usize) {
        self.text.push_str(&format!(
            "  g{from:02} -x-> g{to:02} p{portal} RECT collapse\n"
        ));
    }
    fn entered(&mut self, from: usize, portal: u16, to: usize, rect: Rect, on_plane: bool) {
        self.text.push_str(&format!(
            "  g{from:02} ---> g{to:02} p{portal} rect [{:.3},{:.3},{:.3},{:.3}]{}\n",
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            if on_plane { " (in-plane)" } else { "" }
        ));
    }
    fn pass2(&mut self, group: usize, flags: u32, window: usize, admitted: bool) {
        self.text.push_str(&format!(
            "  pass2 g{group:02} EXT flags {flags:#07x} vs window w{window}: {}\n",
            if admitted { "FLOOD ROOT" } else { "culled" }
        ));
    }
    fn pass3(&mut self, group: usize, flags: u32, admitted: bool) {
        self.text.push_str(&format!(
            "  pass3 g{group:02} CALLBACK flags {flags:#07x} vs the full frustum: {}\n",
            if admitted { "DRAWN" } else { "culled" }
        ));
    }
    fn worklist_over_cap(&mut self, windows: usize) {
        self.text.push_str(&format!(
            "  WARN: {windows} deferred windows — the reference's worklist holds 16. Suspect our \
             flood, not the data.\n"
        ));
    }
}
