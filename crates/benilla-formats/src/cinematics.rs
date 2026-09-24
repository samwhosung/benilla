//! `CinematicSequences.dbc` + `CinematicCamera.dbc` — the in-engine cinematic fly-bys, and the
//! world-space camera path each one resolves to.
//!
//! A cinematic in 1.12 is **not** a movie file. `SMSG_TRIGGER_CINEMATIC` carries a
//! `CinematicSequences.dbc` id; the row names up to eight `CinematicCamera.dbc` cameras; each of
//! those names a `Cameras\*.m2` whose single [`M2Camera`](benilla_m2::M2Camera) record *is* the
//! shot — an eye path, a look-at path and a roll, keyed over a timeline in the model's own local
//! frame — plus the world origin and facing to plant that frame at. The client flies its view
//! along it and the world renders normally underneath. (The pre-rendered `.avi` under
//! `Data\<locale>\Interface\Cinematics` is the *other* thing, the opening movie; it is not this.)
//!
//! **The shipped tables, dumped 2026-08-29 (VERIFIED — the bytes, not a wiki):**
//! `CinematicSequences.dbc` is `10 × 10 fields × 40 B`: `ID · soundId · camera[8]`. Every shipped
//! row uses exactly **one** camera and authors `soundId = 0`. `CinematicCamera.dbc` is
//! `10 × 7 × 28 B`: `ID · model (string) · soundId · originX · originY · originZ · originFacing`.
//! The model column ships `.mdx`; the archive file is `.m2` ([`camera_model_path`]). The ten rows
//! are the eight race intros plus `PalantirOfAzora` and `Scry_cam`. Facing is **radians** — the
//! shipped values include `3.14159` and `4.71239` (π and 3π/2) to five decimals, which no degree
//! table would.
//!
//! **The world transform** — `world = origin + Rz(+facing)·local`, `z` straight through — is the
//! one non-obvious step, and it is now settled twice over.
//!
//! It was first checked against an independent oracle rather than assumed: vmangos walks its own
//! server-side copy of these paths (`Player::UpdateCinematic`) off a hand-built
//! `cinematic_waypoints` table of sampled world positions, and our evaluated path lands on those
//! samples (tens of yards over ~600-yard offsets, on a table whose z column is ground-level, not
//! camera-level). The three sign/axis alternatives miss by 700–1800 yards. See
//! [`CinematicPath::sample`].
//!
//! The binary agrees, and an emulated run of its own bytes confirms it. The client applies affines
//! as a **row vector on the left** (`out = in·M`, `0x7bca80`), and the stored 3×3 (`0x50c870`) is
//! `[[cos,sin,0],[−sin,cos,0],[0,0,1]]`. Read as a diagram acting on a *column* vector that is
//! `Rz(−facing)` — which is what a matrix picture invites. The client never applies it that way.
//! The bytes and the oracle never actually disagreed.
//!
//! **The shipped corpus, read out of the ten `Cameras\*.m2` files** (`fov` radians, `d₀` = how far
//! the shot's first eye position sits from its own origin, horizontally):
//!
//! | cam | model | fov | length | pos keys | roll | d₀ |
//! |---|---|---|---|---|---|---|
//! | 1 | PalantirOfAzora | 0.7854 | 14.9 s | 10 | const 0 | 205 yd |
//! | 2 | FlybyUndead | **1.5708** | 102.0 s | 22 | 18 keys | 523 yd |
//! | 122 | FlybyNightElf | 0.7854 | 102.0 s | 32 | 19 keys | 813 yd |
//! | 142 | FlyByHuman | 0.7854 | 87.5 s | 20 | const 2π | 663 yd |
//! | 162 | FlyByGnome | 0.7854 | 76.7 s | 27 | const 0 | 705 yd |
//! | 182 | FlyByTroll | 0.7854 | 57.8 s | 21 | 12 keys | 39 yd |
//! | 202 | FlyByTauren | 0.7854 | 70.3 s | 19 | 7 keys | 1741 yd |
//! | 224 | Scry_cam | 0.7854 | 3.3 s | 1 | const 3π | 0 yd |
//! | 234 | FlyByDwarf | 0.7854 | 59.6 s | 28 | const 2π | 652 yd |
//! | 235 | FlybyOrc | 0.7854 | 70.2 s | 22 | 14 keys | 1100 yd |
//!
//! Three things there are load-bearing and none of them are guessable. **The Undead intro's FOV is
//! 90°, not 45°** — a client that hard-coded the other nine's value would render that one shot at
//! half the intended width. **Roll is authored around multiples of 2π rather than around 0**
//! (`FlyByDwarf` holds a constant `6.2832`; `Scry_cam` holds `3π`), so it is an *angle to apply*,
//! and a "roll is zero, skip it" shortcut happens to be right only by the identity `2π ≡ 0`; five
//! of the ten genuinely animate it. And the shots **start far from their own origin** — a Tauren
//! 1741 yards out — which is why the server re-anchors object visibility to the flying camera
//! while one runs, and why the client has to stream the world from the *camera*
//! and not the avatar for the duration.
//!
//! **The optics in this table are data, not the shot's framing**. A 24-site census
//! settles it: the M2 camera
//! record's `fov`, `nearClip` and `farClip` are written at model load and read only by `0x7ac640`,
//! which is reachable solely from the portrait and `<Model>` frame paths. **On the cinematic path
//! nothing reads any of the three.** A fly-by is rendered through the *world camera's own* optics,
//! re-stamped every frame.
//!
//! That dissolves the puzzle this note used to record: the authored clips are `0.22222` / `27.7778`
//! (8/36 and 1000/36, identical on every shipped shot), and a 27.8-yard far plane would render the
//! dwarf intro's mountains as empty sky. They are not world clips because they are not clips at
//! all here — which is what two floats nobody consumes look like. They stay parsed because they
//! are genuinely in the record and the portrait path does read them.

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

/// One `CinematicSequences.dbc` row — what a `SMSG_TRIGGER_CINEMATIC` id resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CinematicSequence {
    pub id: u32,
    /// `soundId` (field 1). **Zero on every shipped row** — the sound a fly-by plays comes from
    /// its *camera* row instead ([`CinematicCameraRow::sound_id`]).
    pub sound_id: u32,
    /// The camera ids to play, in order, trailing zeros dropped. Every shipped row holds one.
    pub cameras: Vec<u32>,
}

/// One `CinematicCamera.dbc` row — a shot: which path model, where to plant it, and the sound.
#[derive(Debug, Clone, PartialEq)]
pub struct CinematicCameraRow {
    pub id: u32,
    /// The path model as the table ships it (`Cameras\FlyByDwarf.mdx`). Use
    /// [`camera_model_path`] for the archive path.
    pub model: String,
    /// `SoundEntries.dbc` id for the shot's audio, `0` for none (`PalantirOfAzora` and `Scry_cam`
    /// are the two silent rows).
    pub sound_id: u32,
    /// Where the path's local frame is planted, raw WoW world coordinates.
    pub origin: [f32; 3],
    /// The local frame's yaw about `+Z`, **radians**.
    pub origin_facing: f32,
}

/// Both cinematic tables, keyed by row id.
///
/// `Default` is the **empty** catalog — what "the DBCs failed to load" already means to a caller
/// (every lookup misses, and a trigger it cannot resolve is a trigger it skips).
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

/// The archive path for a camera row's model: the table's `.mdx` reference mapped to the `.m2` the
/// MPQ actually holds (the same normalisation every other model reference in the client takes).
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
        // Trailing zeros are "no camera", not camera 0 — the shipped rows are one camera then
        // seven zeros. Stop at the first, so a hypothetical gap can't smuggle a zero in.
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

/// One instant of a cinematic: where the view is, what it looks at, and how it is banked — all in
/// **raw WoW world coordinates** (X north, Y west, Z up), the convention this crate keeps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CinematicView {
    pub eye: [f32; 3],
    pub target: [f32; 3],
    /// Roll about the view axis, radians. The authored sign, unconverted.
    pub roll: f32,
}

/// A resolved shot: one camera row's `Cameras\*.m2` path, planted in the world and ready to sample.
pub struct CinematicPath {
    /// The camera row this was built from.
    pub camera_id: u32,
    /// The row's `SoundEntries.dbc` narration id, `0` for a silent shot — carried here so a
    /// consumer that has the shot has everything the shot plays.
    pub sound_id: u32,
    /// The authored field of view, radians — `0.7854` (45°) on fifteen of the sixteen shipped
    /// shots and `1.5708` (90°) on the Undead intro.
    ///
    /// **The reference reads this from nothing on the cinematic path** (a 24-site census: the M2
    /// camera's fov and the two clips are written at model load and read only by `0x7ac640`, which
    /// is reachable
    /// solely from the portrait and `<Model>` frame paths). A fly-by is rendered through the
    /// **world camera's own** optics, re-stamped every frame. So this is a real field of the
    /// record, and it is not what a fly-by is framed with — see decision 1711 for what benilla did
    /// with it before that was known.
    pub fov: f32,
    /// The authored near clip, radians-free yards — read by nothing on this path, like [`Self::fov`].
    ///
    /// It is carried because it is *there*, and because its value is the tell: every shipped shot
    /// authors `8/36` or `1000/36` — `0.2222` and `27.7778` yards. As world clips those are
    /// nonsense (a 27.8-yard far plane renders the dwarf intro's mountains as empty sky), which is
    /// exactly what you would expect of two floats nothing consumes.
    pub near_clip: f32,
    /// The authored far clip — see [`Self::near_clip`].
    pub far_clip: f32,
    /// How long the shot runs, milliseconds — the *width* of the sequence band the tracks are
    /// keyed inside (`end − start`), not its end. See [`Self::sample`] for why the two differ.
    pub duration_ms: u32,
    /// Where that band begins on the model's global timeline; added to the sample time so a shot
    /// whose first key is not at zero (`FlybyNightElf`, `Scry_cam`) starts on its first key
    /// instead of holding it.
    band_start: u32,
    origin: [f32; 3],
    facing_sin_cos: (f32, f32),
    camera: M2Camera,
}

impl CinematicPath {
    /// Build a shot from its camera row: read the `.m2` off the chain, take its camera record, and
    /// plant it at the row's origin/facing.
    pub fn load(chain: &mut Chain, row: &CinematicCameraRow) -> Result<Self> {
        let path = camera_model_path(&row.model);
        let bytes = chain
            .read_file(&path)
            .with_context(|| format!("reading cinematic camera model {path}"))?;
        Self::from_m2_bytes(&bytes, row).with_context(|| format!("parsing {path}"))
    }

    /// The same, from bytes already in hand (the test/tooling seam).
    pub fn from_m2_bytes(bytes: &[u8], row: &CinematicCameraRow) -> Result<Self> {
        // The camera array alone, not a whole model parse: a `Cameras\*.m2` carries no geometry to
        // parse and the shot is entirely in this one record.
        let camera = benilla_m2::parse_cameras(bytes)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("model carries no camera record"))?;
        // The shot's length is the model's own sequence band (stride 0x44 records at header
        // 0x1c/0x20, band `[start, end]` at +0x04/+0x08 — the same walk the emitter-timing bake
        // uses). The reference arms this as an ordinary M2 animation on sequence 0, so the shot
        // runs the band: it *plays* for `end − start` and its tracks are keyed at absolute
        // global-timeline stamps inside `[start, end]`. Eight of the ten shipped cameras band at
        // `[0, end]` and the distinction is invisible on them — but `FlybyNightElf` bands at
        // `[333, 102333]` and `Scry_cam` at `[33, 3333]`, and reading `end` as the duration
        // played those two 333 ms / 33 ms too long with the opening frames frozen on the first
        // key (which is what sampling `t = 0` against a track that starts at 333 returns).
        // The fallback, for a file that authors no sequence at all, is the last key.
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

    /// Sample the shot at `ms` from its start, **end-clamped** at both ends.
    ///
    /// `ms` is measured from the *shot's* start; the tracks are keyed on the model's global
    /// timeline, so it is offset by [`Self::band_start`] before it reaches them. On eight of the
    /// ten shipped cameras that offset is zero and the two are the same number; on
    /// `FlybyNightElf` (band `[333, 102333]`) and `Scry_cam` (`[33, 3333]`) it is the difference
    /// between opening on the authored first key and holding it for a third of a second first.
    ///
    /// Two steps, in this order. The reference's publish pass composes each track against its base
    /// (`eye = position_base + positions(t)`, `target = target_position_base + target(t)`); then
    /// the local frame is planted in the world by the camera row's origin and facing:
    ///
    /// ```text
    /// world.x = origin.x + local.x·cos(facing) − local.y·sin(facing)
    /// world.y = origin.y + local.x·sin(facing) + local.y·cos(facing)
    /// world.z = origin.z + local.z
    /// ```
    ///
    /// i.e. a plain yaw about `+Z` by the row's facing, then a translation — no scale, no
    /// handedness flip. Checked against vmangos's independently sampled `cinematic_waypoints` for
    /// the dwarf (41) and human (81) intros; the sign-flipped and axis-swapped alternatives are
    /// off by an order of magnitude (module doc).
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

/// A camera track sampled and composed against its base — the reference's publish form.
fn sample_against(track: &M2Track<M2SplineKey<[f32; 3]>>, base: [f32; 3], ms: u32) -> [f32; 3] {
    let d = track.sample_ms(ms).unwrap_or([0.0; 3]);
    std::array::from_fn(|i| base[i] + d[i])
}

/// The model's **first** sequence band, `(start, end)` in global-timeline milliseconds (header
/// `0x1c` count / `0x20` offset, entry stride `0x44`, band at `+0x04`/`+0x08`). `None` for a model
/// with no sequences.
///
/// **`start` is not always zero, and the shipped corpus is its own proof of these offsets.** Eight
/// of the ten cameras band at `[0, end]`, but `FlybyNightElf` bands at `[333, 102333]` and
/// `Scry_cam` at `[33, 3333]` — and in both files the band's `start` is *exactly* the first
/// timestamp on the position and target tracks, while `end` is exactly the last. Two fields that
/// land on the first and last key of every file in the corpus are the first and last key, which
/// is why this is read rather than assumed to be a duration.
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

    /// The eight race intros, by `ChrRaces.dbc` `CinematicSequence` (field 16) — the ids a first
    /// login actually sends (VERIFIED in the shipped `ChrRaces.dbc`, and matching vmangos's
    /// `SendCinematicStart(rEntry->CinematicSequence)`).
    const RACE_INTROS: [u32; 8] = [2, 21, 41, 61, 81, 101, 121, 141];

    #[test]
    fn model_paths_map_mdx_to_the_archive_m2() {
        assert_eq!(
            camera_model_path("Cameras\\FlyByDwarf.mdx"),
            "Cameras\\FlyByDwarf.m2"
        );
        assert_eq!(camera_model_path("Cameras\\X.MDX"), "Cameras\\X.m2");
        assert_eq!(camera_model_path("Cameras\\X.m2"), "Cameras\\X.m2");
        // A path with no extension at all still names an .m2, and a dot inside a directory name
        // is not an extension.
        assert_eq!(camera_model_path("Cameras\\X"), "Cameras\\X.m2");
    }

    #[test]
    fn real_cinematic_tables_are_the_shipped_shape() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_cinematics(&mut chain).expect("load cinematic tables");
        assert_eq!(cat.sequence_count(), 10);
        assert_eq!(cat.camera_count(), 10);

        // Every shipped sequence names exactly one camera and carries no sound of its own.
        for id in RACE_INTROS {
            let seq = cat.sequence(id).unwrap_or_else(|| panic!("sequence {id}"));
            assert_eq!(seq.cameras.len(), 1, "sequence {id} camera count");
            assert_eq!(seq.sound_id, 0, "sequence {id} sound");
        }

        // The dwarf intro, end to end — the row decision 0196 captured live.
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
            // Cubic Bézier on both vector tracks — the case the four-way interp dispatch exists
            // for, and the one a step/linear-only sampler would silently mangle.
            assert_eq!(path.camera.positions.interp, 2, "sequence {id} position");
            assert_eq!(path.camera.target.interp, 2, "sequence {id} target");
            // **The end condition, and the proof of the band offsets.** The authored sequence
            // band brackets both vector tracks exactly: its `start` is the first key's timestamp
            // and its `end` is the last one's, on every shipped fly-by. Two header fields that
            // land on the first and last key of all eight files are the first and last key — and
            // that is what licenses reading the playback length as `end − start` rather than as
            // `end`, which is the bug this assertion now pins (`FlybyNightElf` bands at
            // `[333, 102333]`, so the two readings differ by a third of a second and by whether
            // the shot opens on its first key or holds it).
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
            // The clips are uniform across the corpus; the FOV is NOT — the Undead intro is 90°
            // where the rest are 45°. Kept as a **data** assertion, and no longer as a reason to
            // read it per shot: decision 1711 established that the reference's cinematic path
            // reads none of these three, so the split is a fact about the files and not about how
            // any shot is framed. It stays because a parser that silently started returning zeros
            // for all three would otherwise pass every other test in this file.
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

    /// The dwarf intro's world path, pinned at three instants.
    ///
    /// These numbers are the **transform's** golden: they were derived independently (a
    /// hand-written evaluator over the raw bytes) and then cross-checked, time-independently,
    /// against vmangos's `cinematic_waypoints` samples for this cinematic — our arc passes within
    /// 59.6 yd horizontally of every one of the server's six samples (mean 35.5). The three
    /// alternative conventions do far worse on the same measure: `Rz(−facing)` 436.8 yd,
    /// axis-swapped 334.0, no rotation at all 859.9. So a sign flip here fails this test loudly.
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
        // Past the end the path holds its last key — it does not wrap to the start.
        assert_eq!(path.sample(u32::MAX).eye, end.eye);
        // Roll here is a single key holding **2π**, not 0 — the authored angles sit around whole
        // turns (`Scry_cam` holds 3π), so roll is applied as an angle and only *happens* to be an
        // identity on this shot.
        assert!(
            (start.roll - std::f32::consts::TAU).abs() < 1e-4,
            "{}",
            start.roll
        );
        assert_eq!(mid.roll, start.roll, "a single-key roll track is constant");
    }

    /// The shots range far from their own origin — the reason the server re-anchors object
    /// visibility to the flying camera while a cinematic runs, and the reason
    /// benilla has to stream the world from the camera rather than the avatar for the duration.
    #[test]
    fn real_flyby_shots_range_far_from_their_origin() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_cinematics(&mut chain).expect("load cinematic tables");
        for id in RACE_INTROS {
            let row = cat.shots(id)[0].clone();
            let path = CinematicPath::load(&mut chain, &row).expect("path");
            // The furthest the eye gets from the body's neighbourhood over the whole shot. The
            // troll intro *starts* only 39 yd out, so this is the reach, not the first frame.
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
