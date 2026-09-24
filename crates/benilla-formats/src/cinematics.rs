//! `CinematicSequences.dbc` and `CinematicCamera.dbc`: the in-engine cinematic fly-bys and the
//! world-space camera path each one resolves to.
//!
//! A cinematic is not a movie. `SMSG_TRIGGER_CINEMATIC` carries a sequence id, the sequence names
//! up to eight camera rows, and each row names a `Cameras\*.m2` whose one
//! [`M2Camera`](benilla_m2::M2Camera) record is the shot: eye, look-at and roll tracks in the
//! model's local frame, planted at the row's world origin and facing. The world renders normally
//! underneath. A shot ranges far from its origin (the Tauren intro starts 1741 yd out), and the
//! server re-anchors object visibility to the flying camera while one runs (vmangos
//! `Player::UpdateCinematic`), so the world streams from the camera, not the avatar.
//!
//! The plant is `world = origin + Rz(+facing)·local`, z unchanged. The reference applies affines
//! to a row vector on the left (`out = in·M`, `0x7bca80`), so its stored 3×3
//! `[[cos,sin,0],[−sin,cos,0],[0,0,1]]` (`0x50c870`) is `Rz(+facing)`, though it reads as
//! `Rz(−facing)` on a column vector.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};
use benilla_m2::{M2Camera, M2SplineKey, M2Track};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

const SEQUENCES: &str = "DBFilesClient\\CinematicSequences.dbc";
const CAMERAS: &str = "DBFilesClient\\CinematicCamera.dbc";

/// How many camera slots a `CinematicSequences.dbc` row carries (fields 2..=9).
pub const SEQUENCE_CAMERAS: usize = 8;

/// One `CinematicSequences.dbc` row, what a `SMSG_TRIGGER_CINEMATIC` id resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CinematicSequence {
    pub id: u32,
    /// `soundId` (field 1): 0 on every shipped row; a fly-by's sound is its camera row's.
    pub sound_id: u32,
    /// The camera ids to play, in order, trailing zeros dropped. Every shipped row holds one.
    pub cameras: Vec<u32>,
}

/// One `CinematicCamera.dbc` row, a shot: its path model, where to plant it, and its sound.
#[derive(Debug, Clone, PartialEq)]
pub struct CinematicCameraRow {
    pub id: u32,
    /// The path model as the table ships it (`.mdx`); [`camera_model_path`] gives the archive path.
    pub model: String,
    /// `SoundEntries.dbc` id for the shot's audio, 0 for none.
    pub sound_id: u32,
    /// Where the path's local frame is planted, raw WoW world coordinates.
    pub origin: [f32; 3],
    /// The local frame's yaw about `+Z`, in radians.
    pub origin_facing: f32,
}

/// Both cinematic tables, keyed by row id. `Default` is the empty catalog: every lookup misses,
/// so a trigger it cannot resolve is skipped.
#[derive(Default)]
pub struct CinematicCatalog {
    sequences: HashMap<u32, CinematicSequence>,
    cameras: HashMap<u32, CinematicCameraRow>,
}

impl CinematicCatalog {
    /// The sequence a `SMSG_TRIGGER_CINEMATIC` id names.
    pub fn sequence(&self, id: u32) -> Option<&CinematicSequence> {
        self.sequences.get(&id)
    }

    /// One camera row by id.
    pub fn camera(&self, id: u32) -> Option<&CinematicCameraRow> {
        self.cameras.get(&id)
    }

    /// The camera rows a sequence plays, in order, skipping any id the camera table doesn't carry.
    pub fn shots(&self, sequence_id: u32) -> Vec<&CinematicCameraRow> {
        self.sequence(sequence_id)
            .into_iter()
            .flat_map(|s| s.cameras.iter())
            .filter_map(|id| self.camera(*id))
            .collect()
    }

    pub fn sequence_count(&self) -> usize {
        self.sequences.len()
    }

    pub fn camera_count(&self) -> usize {
        self.cameras.len()
    }
}

/// The archive path for a camera row's model: the table ships `.mdx`, the MPQ holds `.m2`.
pub fn camera_model_path(model: &str) -> String {
    let stem = model.rsplit_once('.').map_or(model, |(stem, ext)| {
        match ext.to_ascii_lowercase().as_str() {
            "mdx" | "mdl" | "m2" => stem,
            _ => model,
        }
    });
    format!("{stem}.m2")
}

fn sequences_schema() -> Schema {
    let mut s = Schema::new("CinematicSequences");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("SoundID", FieldType::UInt32));
    for i in 0..SEQUENCE_CAMERAS {
        s.add_field(SchemaField::new(format!("Camera{i}"), FieldType::UInt32));
    }
    s
}

fn cameras_schema() -> Schema {
    let mut s = Schema::new("CinematicCamera");
    for (name, ty) in [
        ("ID", FieldType::UInt32),
        ("Model", FieldType::String),
        ("SoundID", FieldType::UInt32),
        ("OriginX", FieldType::Float32),
        ("OriginY", FieldType::Float32),
        ("OriginZ", FieldType::Float32),
        ("OriginFacing", FieldType::Float32),
    ] {
        s.add_field(SchemaField::new(name, ty));
    }
    s
}

/// Read both cinematic tables off the patch chain.
pub fn load_cinematics(chain: &mut Chain) -> Result<CinematicCatalog> {
    let bytes = chain
        .read_file(SEQUENCES)
        .with_context(|| format!("reading {SEQUENCES}"))?;
    let rs = parse(&bytes, sequences_schema(), "CinematicSequences")?;
    let mut sequences = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        // A zero slot is "no camera", not camera 0; the list stops at the first one.
        let cameras = (0..SEQUENCE_CAMERAS)
            .map_while(|i| u32_at(r, 2 + i).filter(|&c| c != 0))
            .collect();
        sequences.insert(
            id,
            CinematicSequence {
                id,
                sound_id: u32_at(r, 1).unwrap_or(0),
                cameras,
            },
        );
    }

    let bytes = chain
        .read_file(CAMERAS)
        .with_context(|| format!("reading {CAMERAS}"))?;
    let rs = parse(&bytes, cameras_schema(), "CinematicCamera")?;
    let mut cameras = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        cameras.insert(
            id,
            CinematicCameraRow {
                id,
                model: str_at(&rs, r, 1).unwrap_or_default(),
                sound_id: u32_at(r, 2).unwrap_or(0),
                origin: [
                    f32_at(r, 3).unwrap_or(0.0),
                    f32_at(r, 4).unwrap_or(0.0),
                    f32_at(r, 5).unwrap_or(0.0),
                ],
                origin_facing: f32_at(r, 6).unwrap_or(0.0),
            },
        );
    }

    Ok(CinematicCatalog { sequences, cameras })
}

/// One instant of a cinematic: eye, look-at target and roll, in raw WoW world coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CinematicView {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    /// Roll about the view axis in radians, the authored sign unconverted: an angle to apply, never
    /// tested for 0, as shots hold values such as 2π (`FlyByDwarf`) and 3π (`Scry_cam`).
    pub roll: f32,
}

/// A resolved shot: one camera row's `Cameras\*.m2` path, planted in the world and ready to sample.
pub struct CinematicPath {
    /// The camera row this was built from.
    pub camera_id: u32,
    /// The row's `SoundEntries.dbc` narration id, 0 for a silent shot.
    pub sound_id: u32,
    /// The authored field of view in radians: 45° on every shipped shot but the Undead intro's 90°.
    /// Not the fly-by's framing: the reference reads the record's optics only in `0x7ac640`, on the
    /// portrait and `<Model>` paths, and flies through the world camera's own, set every frame.
    pub fov: f32,
    /// The authored near clip (8/36 on every shipped shot), unread on this path like [`Self::fov`].
    pub near_clip: f32,
    /// The authored far clip (1000/36 on every shipped shot), unread on this path.
    pub far_clip: f32,
    /// How long the shot runs in ms: the width of its sequence band (`end − start`), not its end.
    pub duration_ms: u32,
    /// The band's start on the model's global timeline, added to every sample time.
    band_start: u32,
    origin: [f32; 3],
    facing_sin_cos: (f32, f32),
    camera: M2Camera,
}

impl CinematicPath {
    /// Build a shot from its camera row: the `.m2`'s camera record, planted at the row's origin.
    pub fn load(chain: &mut Chain, row: &CinematicCameraRow) -> Result<Self> {
        let path = camera_model_path(&row.model);
        let bytes = chain
            .read_file(&path)
            .with_context(|| format!("reading cinematic camera model {path}"))?;
        Self::from_m2_bytes(&bytes, row).with_context(|| format!("parsing {path}"))
    }

    /// The same, from bytes already in hand (the test/tooling seam).
    pub fn from_m2_bytes(bytes: &[u8], row: &CinematicCameraRow) -> Result<Self> {
        // The camera array alone: a `Cameras\*.m2` has no geometry, and the shot is this record.
        let camera = benilla_m2::parse_cameras(bytes)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("model carries no camera record"))?;
        // The reference plays the shot as an ordinary M2 animation on sequence 0: it runs for
        // `end − start`, its keys stamped on the global timeline inside `[start, end]`. A file
        // with no sequence falls back to its last key.
        let (band_start, band_end) = sequence_band(bytes)
            .or_else(|| {
                [
                    camera.positions.keys.last().map(|k| k.0),
                    camera.target.keys.last().map(|k| k.0),
                ]
                .into_iter()
                .flatten()
                .max()
                .map(|end| (0, end))
            })
            .unwrap_or((0, 0));
        let duration_ms = band_end.saturating_sub(band_start);
        Ok(Self {
            camera_id: row.id,
            sound_id: row.sound_id,
            fov: camera.fov,
            near_clip: camera.near_clip,
            far_clip: camera.far_clip,
            duration_ms,
            band_start,
            origin: row.origin,
            facing_sin_cos: row.origin_facing.sin_cos(),
            camera,
        })
    }

    /// Sample the shot `ms` after its start, clamped at both ends: each track composed against its
    /// base, then planted in the world by the row's origin and facing, with no scale or flip.
    pub fn sample(&self, ms: u32) -> CinematicView {
        let ms = self.band_start.saturating_add(ms);
        let eye = self.to_world(sample_against(
            &self.camera.positions,
            self.camera.position_base,
            ms,
        ));
        let target = self.to_world(sample_against(
            &self.camera.target,
            self.camera.target_base,
            ms,
        ));
        CinematicView {
            eye,
            target,
            roll: self.camera.roll.sample_ms(ms).unwrap_or(0.0),
        }
    }

    fn to_world(&self, local: [f32; 3]) -> [f32; 3] {
        let (s, c) = self.facing_sin_cos;
        [
            self.origin[0] + local[0] * c - local[1] * s,
            self.origin[1] + local[0] * s + local[1] * c,
            self.origin[2] + local[2],
        ]
    }
}

/// A camera track sampled and composed against its base, the reference's publish form.
fn sample_against(track: &M2Track<M2SplineKey<[f32; 3]>>, base: [f32; 3], ms: u32) -> [f32; 3] {
    let d = track.sample_ms(ms).unwrap_or([0.0; 3]);
    std::array::from_fn(|i| base[i] + d[i])
}

/// The model's first sequence band, `(start, end)` in global-timeline ms: count at header `0x1c`,
/// offset at `0x20`, entries `0x44` apart, band at `+0x04`/`+0x08`. `start` is not always 0
/// (`FlybyNightElf` bands at `[333, 102333]`).
fn sequence_band(b: &[u8]) -> Option<(u32, u32)> {
    let le_u32 = |o: usize| -> Option<u32> {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    let (n, o) = (le_u32(0x1c)?, le_u32(0x20)? as usize);
    if n == 0 {
        return None;
    }
    Some((le_u32(o + 0x04)?, le_u32(o + 0x08)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The eight race intros, `ChrRaces.dbc` `CinematicSequence` (field 16), which vmangos sends
    /// on a first login (`CharacterHandler.cpp:581`).
    const RACE_INTROS: [u32; 8] = [2, 21, 41, 61, 81, 101, 121, 141];

    #[test]
    fn model_paths_map_mdx_to_the_archive_m2() {
        assert_eq!(
            camera_model_path("Cameras\\FlyByDwarf.mdx"),
            "Cameras\\FlyByDwarf.m2"
        );
        assert_eq!(camera_model_path("Cameras\\X.MDX"), "Cameras\\X.m2");
        assert_eq!(camera_model_path("Cameras\\X.m2"), "Cameras\\X.m2");
        // A path with no extension still names an `.m2`.
        assert_eq!(camera_model_path("Cameras\\X"), "Cameras\\X.m2");
    }

    #[test]
    fn real_cinematic_tables_are_the_shipped_shape() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_cinematics(&mut chain).expect("load cinematic tables");
        assert_eq!(cat.sequence_count(), 10);
        assert_eq!(cat.camera_count(), 10);

        for id in RACE_INTROS {
            let seq = cat.sequence(id).unwrap_or_else(|| panic!("sequence {id}"));
            assert_eq!(seq.cameras.len(), 1, "sequence {id} camera count");
            assert_eq!(seq.sound_id, 0, "sequence {id} sound");
        }

        // The dwarf intro's shipped row, end to end.
        let dwarf = cat.shots(41);
        assert_eq!(dwarf.len(), 1);
        let cam = dwarf[0];
        assert_eq!(cam.id, 234);
        assert_eq!(cam.model, "Cameras\\FlyByDwarf.mdx");
        assert_eq!(cam.sound_id, 3740);
        assert!((cam.origin[0] - -5579.16).abs() < 0.01);
        assert!((cam.origin[1] - -455.776).abs() < 0.01);
        assert!((cam.origin[2] - 406.476).abs() < 0.01);
        // Radians, not degrees: 4.71239 = 3π/2 to five decimals.
        assert!((cam.origin_facing - std::f32::consts::FRAC_PI_2 * 3.0).abs() < 1e-4);
    }

    #[test]
    fn real_flyby_shots_are_bezier_and_end_on_their_sequence_band() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_cinematics(&mut chain).expect("load cinematic tables");
        for id in RACE_INTROS {
            let row = cat.shots(id)[0].clone();
            let path = CinematicPath::load(&mut chain, &row)
                .unwrap_or_else(|e| panic!("sequence {id} path: {e:#}"));
            // The shot is a real flight, not a single parked key.
            assert!(path.duration_ms > 30_000, "sequence {id} duration");
            assert!(
                path.camera.positions.keys.len() >= 10,
                "sequence {id} is richly keyed"
            );
            // Interp 2 is cubic Bézier, on both vector tracks.
            assert_eq!(path.camera.positions.interp, 2, "sequence {id} position");
            assert_eq!(path.camera.target.interp, 2, "sequence {id} target");
            // The band brackets both tracks exactly: `start` is the first key, `end` the last.
            for (what, track) in [
                ("position", &path.camera.positions),
                ("target", &path.camera.target),
            ] {
                assert_eq!(
                    track.keys.first().map(|k| k.0),
                    Some(path.band_start),
                    "sequence {id} {what} track starts on the band"
                );
                assert_eq!(
                    track.keys.last().map(|k| k.0),
                    Some(path.band_start + path.duration_ms),
                    "sequence {id} {what} track ends on the band"
                );
            }
            // Data only: uniform clips, and a 90° fov on the Undead intro against 45° elsewhere.
            assert!((path.near_clip - 8.0 / 36.0).abs() < 1e-6);
            assert!((path.far_clip - 1000.0 / 36.0).abs() < 1e-4);
            let want_fov = if id == 2 {
                std::f32::consts::FRAC_PI_2
            } else {
                std::f32::consts::FRAC_PI_4
            };
            assert!(
                (path.fov - want_fov).abs() < 1e-4,
                "sequence {id} fov: {} vs {want_fov}",
                path.fov
            );
        }
    }

    /// The golden points come from a separate evaluator over the raw bytes; the arc passes within
    /// 59.6 yd of all six of vmangos's `cinematic_waypoints` samples for this intro.
    #[test]
    fn real_dwarf_intro_flies_the_authored_arc() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_cinematics(&mut chain).expect("load cinematic tables");
        let row = cat.shots(41)[0].clone();
        let path = CinematicPath::load(&mut chain, &row).expect("dwarf path");
        assert_eq!(path.duration_ms, 59_600);

        let near = |got: [f32; 3], want: [f32; 3], what: &str| {
            for i in 0..3 {
                assert!(
                    (got[i] - want[i]).abs() < 0.05,
                    "{what}[{i}]: got {}, want {}",
                    got[i],
                    want[i]
                );
            }
        };
        let start = path.sample(0);
        near(start.eye, [-5041.888, -824.646, 541.267], "start eye");
        near(start.target, [-5021.802, -836.789, 539.912], "start target");
        let mid = path.sample(30_000);
        near(mid.eye, [-5666.181, -425.222, 473.426], "mid eye");
        near(mid.target, [-5715.481, -427.434, 450.394], "mid target");
        let end = path.sample(path.duration_ms);
        near(end.eye, [-6246.921, 333.773, 384.187], "end eye");
        // Past the end the path holds its last key; it does not wrap.
        assert_eq!(path.sample(u32::MAX).eye, end.eye);
        // One roll key, holding 2π rather than 0.
        assert!(
            (start.roll - std::f32::consts::TAU).abs() < 1e-4,
            "{}",
            start.roll
        );
        assert_eq!(mid.roll, start.roll, "a single-key roll track is constant");
    }

    #[test]
    fn real_flyby_shots_range_far_from_their_origin() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_cinematics(&mut chain).expect("load cinematic tables");
        for id in RACE_INTROS {
            let row = cat.shots(id)[0].clone();
            let path = CinematicPath::load(&mut chain, &row).expect("path");
            // The reach over the whole shot, not the first frame: the troll intro starts 39 yd out.
            let reach = (0..=path.duration_ms)
                .step_by(500)
                .map(|ms| {
                    let e = path.sample(ms).eye;
                    ((e[0] - row.origin[0]).powi(2) + (e[1] - row.origin[1]).powi(2)).sqrt()
                })
                .fold(0.0f32, f32::max);
            assert!(reach > 300.0, "sequence {id} reaches only {reach:.0} yd");
        }
    }
}
