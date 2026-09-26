//! M2 skeleton and animation parsing: the rest skeleton and each sequence's per-bone keyframes.

use std::collections::HashMap;
use std::io::Cursor;

use anyhow::Result;
use benilla_bytes::capped;
use benilla_m2::parse_m2;

use super::{le_f32, le_u16, le_u32};
use crate::BoneSpin;

/// A rest-skeleton bone, raw WoW model space. Vanilla M2 has no inverse bind matrices: the rest
/// pose is identity and the pivot is the bind position (`0x714260`).
#[derive(Debug, Clone, Copy)]
pub struct SkeletonBone {
    pub parent: i16,
    pub pivot: [f32; 3],
    /// `KeyBoneID` (`-1` none; 0/1 left/right arm, 2/3 left/right shoulder, …): how the per-arm
    /// masks find roots.
    pub key_bone: i16,
    /// The billboard arm (flags `0x08/0x10/0x20/0x40`); children inherit it.
    pub billboard: Option<crate::BillboardKind>,
    /// Bone flags `0x1/0x2/0x4`: how the parent matrix is rebuilt ([`crate::ParentArm`]).
    pub parent_arm: Option<crate::ParentArm>,
}

/// A model's bones in M2 file order, which a vertex's `joints` index.
#[derive(Debug, Clone, Default)]
pub struct Skeleton {
    pub bones: Vec<SkeletonBone>,
}

impl Skeleton {
    /// The nearest bone at or above `bone` with a billboard arm. Its rows become the camera basis
    /// and children multiply onto it (`0x7151f9`), so every descendant is camera-dependent, emitter
    /// positions too (`0x7190a9`); a nearer host discards what an outer one wrote.
    pub fn billboard_host(&self, bone: u16) -> Option<usize> {
        let mut i = usize::from(bone);
        // Bounded by the bone count: the guard against a malformed parent cycle.
        for _ in 0..=self.bones.len() {
            let b = self.bones.get(i)?;
            if b.billboard.is_some() {
                return Some(i);
            }
            i = usize::try_from(b.parent).ok()?;
        }
        None
    }
}

/// Parse the bone hierarchy (stride `0x6c`: parent `+0x08`, pivot `+0x60`, as `0x714260` reads).
pub fn parse_m2_skeleton(bytes: &[u8]) -> Result<Skeleton> {
    let format =
        parse_m2(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing M2: {e}"))?;
    let bones = format
        .model()
        .bones
        .iter()
        .map(|b| SkeletonBone {
            parent: b.parent,
            pivot: [b.pivot.x, b.pivot.y, b.pivot.z],
            key_bone: b.key_bone,
            billboard: crate::BillboardKind::from_bone_flags(b.flags.bits()),
            parent_arm: crate::ParentArm::from_bone_flags(b.flags.bits()),
        })
        .collect();
    Ok(Skeleton { bones })
}

/// One attachment point (`0` shield, `1`/`2` right/left hand), position in raw WoW model space.
#[derive(Debug, Clone, Copy)]
pub struct M2Attachment {
    pub id: u16,
    pub bone: u16,
    pub position: [f32; 3],
}

/// One attachment point per id the AttachLookup resolves (`0x710310`), in id order: of several
/// records under one id, only the one the lookup names is reachable.
pub fn parse_m2_attachments(bytes: &[u8]) -> Result<Vec<M2Attachment>> {
    let format =
        parse_m2(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing M2: {e}"))?;
    let model = format.model();
    Ok((0..model.attach_lookup.len())
        .filter_map(|id| {
            let a = model.attachment(id as u16)?;
            Some(M2Attachment {
                // The lookup's id: the reference never re-reads the record's own id field.
                id: id as u16,
                bone: a.bone,
                position: a.position,
            })
        })
        .collect())
}

/// An event record's position half, looked up by 4CC, first match (`0x7130e0`/`0x7131b0`): the
/// cast-release points `$CSL`/`$CSR`/`$CST` (`0x60c9b0`) and the ranged release `$BWR`.
#[derive(Debug, Clone, Copy)]
pub struct EventMarker {
    /// The identifier 4CC, stored forward (`*b"$CSL"`).
    pub ident: [u8; 4],
    pub bone: u16,
    pub position: [f32; 3],
}

/// The event table's positional markers, in file order since lookups take the first match.
pub fn parse_m2_event_markers(bytes: &[u8]) -> Result<Vec<EventMarker>> {
    let format =
        parse_m2(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing M2: {e}"))?;
    Ok(format
        .model()
        .event_markers
        .iter()
        .map(|m| EventMarker {
            ident: m.ident,
            bone: m.bone,
            position: m.position,
        })
        .collect())
}

/// A bow's `$WTT`/`$WTB` event records, `(bone, position)`, which the string drawer `0x611ff0`
/// spans top to bottom, each posed by its limb-tip bone (`0x7131b0`).
#[derive(Clone, Copy)]
pub struct StringAnchors {
    pub top: (u16, [f32; 3]),
    pub bottom: (u16, [f32; 3]),
}

/// The `$WTT`/`$WTB` anchors from the raw event table (stride 44); `None` unless both exist.
pub fn parse_m2_string_anchors(b: &[u8]) -> Option<StringAnchors> {
    let (ev_count, ev_ofs) = (le_u32(b, 0x114) as usize, le_u32(b, 0x118) as usize);
    let (mut top, mut bottom) = (None, None);
    for e in 0..ev_count {
        let erec = ev_ofs + e * 44;
        if erec + 44 > b.len() {
            break;
        }
        let ident = b.get(erec..erec + 4)?;
        let anchor = (
            le_u32(b, erec + 8) as u16,
            [
                le_f32(b, erec + 12),
                le_f32(b, erec + 16),
                le_f32(b, erec + 20),
            ],
        );
        match ident {
            b"$WTT" => top = Some(anchor),
            b"$WTB" => bottom = Some(anchor),
            _ => {}
        }
    }
    Some(StringAnchors {
        top: top?,
        bottom: bottom?,
    })
}

/// The first `$CCH` marker, `(bone, position)`: the fishing line's near end, found and posed by
/// `0x7131b0`. The line reads only the pole's, though the bobber authors one; among weapons only
/// the fishing pole does, so a held item keys on its presence.
pub fn parse_m2_cch_marker(b: &[u8]) -> Option<(u16, [f32; 3])> {
    let (ev_count, ev_ofs) = (le_u32(b, 0x114) as usize, le_u32(b, 0x118) as usize);
    for e in 0..ev_count {
        let erec = ev_ofs + e * 44;
        if erec + 44 > b.len() {
            break;
        }
        if b.get(erec..erec + 4)? == b"$CCH" {
            return Some((
                le_u32(b, erec + 8) as u16,
                [
                    le_f32(b, erec + 12),
                    le_f32(b, erec + 16),
                    le_f32(b, erec + 20),
                ],
            ));
        }
    }
    None
}

/// One row of the M2's PlayableAnimationLookup, the resolution of a clip the model lacks.
#[derive(Debug, Clone, Copy)]
pub struct PlayableAnim {
    /// The `AnimationData.dbc` id this model actually plays for the row's requested id.
    pub resolved_id: u16,
    /// Direction/variant playback code; carried, not applied.
    pub dir_flags: u16,
}

/// The [`PlayableAnim`] table (`0x711bf0`, header `+0x2c`/`+0x30`); none means identity.
pub fn parse_m2_playable_animation_lookup(bytes: &[u8]) -> Result<Vec<PlayableAnim>> {
    let format =
        parse_m2(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing M2: {e}"))?;
    Ok(format
        .model()
        .playable_animation_lookup
        .iter()
        .map(|p| PlayableAnim {
            resolved_id: p.resolved_id,
            dir_flags: p.dir_flags,
        })
        .collect())
}

/// The AnimationLookup (header `+0x24`/`+0x28`): `AnimationData.dbc` id to first sequence slot,
/// `0xffff` for none, as the ownership test `0x711960` reads it (`M2Model::owns_animation`).
pub fn parse_m2_animation_lookup(bytes: &[u8]) -> Result<Vec<u16>> {
    let format =
        parse_m2(&mut Cursor::new(bytes)).map_err(|e| anyhow::anyhow!("parsing M2: {e}"))?;
    Ok(format.model().animation_lookup.clone())
}

/// The bones that spin rigidly ([`BoneSpin`]) in anim 0, which the load arms play (`0x70ebd0`,
/// doodads `0x695100`). How a WMO skybox is armed is untraced; both shipped ones have one clip.
pub fn m2_bone_spins(bytes: &[u8]) -> HashMap<u16, BoneSpin> {
    let mut out = HashMap::new();
    let Ok(skeleton) = parse_m2_skeleton(bytes) else {
        return out;
    };
    let anims = parse_m2_animations(bytes);
    let Some(seq) = anims.iter().find(|a| a.anim_id == 0) else {
        return out;
    };
    let (bone_count, bone_ofs) = (le_u32(bytes, 0x34) as usize, le_u32(bytes, 0x38) as usize);
    for keys in &seq.bones {
        let idx = keys.bone as usize;
        // Rotation only and at least two keys: one key is a pose, not a spin.
        if keys.rotation.len() < 2 || !keys.translation.is_empty() || !keys.scale.is_empty() {
            continue;
        }
        // Parentless: nothing above composes another motion in.
        let Some(bone) = skeleton.bones.get(idx).filter(|b| b.parent < 0) else {
            continue;
        };
        // The rotation track's `interp_type`, bounds-checked here: `le_u16` panics past the end.
        let interp = idx < bone_count
            && bone_ofs
                .checked_add(idx * 0x6c + 0x28)
                .is_some_and(|t| t + 2 <= bytes.len() && le_u16(bytes, t) != 0);
        out.insert(
            keys.bone,
            BoneSpin {
                pivot: bone.pivot,
                duration: seq.duration,
                interp,
                keys: keys.rotation.clone(),
            },
        );
    }
    out
}

/// One bone's keys for a sequence, raw WoW model space, seconds from its start, rotations
/// `[x, y, z, w]`; a bone absent from [`ModelAnimation::bones`] holds its bind pose.
#[derive(Debug, Clone)]
pub struct BoneKeys {
    pub bone: u16,
    pub translation: Vec<(f32, [f32; 3])>,
    pub rotation: Vec<(f32, [f32; 4])>,
    pub scale: Vec<(f32, [f32; 3])>,
}

/// A bone channel on a global sequence, sampled at the scene clock less the instance's attach
/// time, mod `period_ms` (`0x714352`). Keys are absolute ms, ascending.
#[derive(Debug, Clone)]
pub struct GlobalSeqChannel<T> {
    pub period_ms: u32,
    pub keys: Vec<(u32, T)>,
}

/// A bone's global-sequence channels, composed over the sequence pose at runtime.
#[derive(Debug, Clone)]
pub struct GlobalSeqBone {
    pub bone: u16,
    pub translation: Option<GlobalSeqChannel<[f32; 3]>>,
    pub rotation: Option<GlobalSeqChannel<[f32; 4]>>,
    pub scale: Option<GlobalSeqChannel<[f32; 3]>>,
}

/// One animation event key: a 4CC-tagged trigger on the sequence timeline.
#[derive(Debug, Clone, Copy)]
pub struct AnimEvent {
    /// Seconds from the sequence start, as the bone keys.
    pub time: f32,
    /// The identifier 4CC, stored forward (`*b"$FL0"`).
    pub ident: [u8; 4],
    /// The payload: a SoundEntries id for `$SND`/`$DSL`/`$DSO`, else 0.
    pub data: u32,
    /// The record's bone (`+8`), carried on the key because a 4CC repeats: every player model
    /// authors `$CSD` six times, so a lookup by 4CC finds the wrong point.
    pub bone: u16,
    /// The record's point (`+12`); the event kernel (`0x719370`) snapshots
    /// `placement · boneMatrix[bone] · position` into the callback record.
    pub position: [f32; 3],
}

/// One sequence's per-bone keyframes. Stand is `anim_id` 0 (`0x70ebd0`) but not always record 0,
/// so consumers select by `anim_id`.
#[derive(Debug, Clone)]
pub struct ModelAnimation {
    /// `AnimationData.dbc` id, the selection key (0 Stand, 4 Walk, 5 Run, 13 WalkBackwards, …).
    pub anim_id: u16,
    /// The file slot, which indexes every track's key ranges; zero-length sequences are dropped.
    pub seq_index: usize,
    /// The absolute band (`M2Sequence` `+0x04`/`+0x08`, ms) sequence tracks are cut to.
    pub start_ms: u32,
    pub end_ms: u32,
    pub duration: f32,
    pub looping: bool,
    /// `moveSpeed` (`+0x0c`, yd/s): a locomotion clip plays at `unitSpeed / (moveSpeed · scale)`
    /// (`0x5fe2f0`); `0.0` plays at 1×.
    pub move_speed: f32,
    /// `blendTime` (`+0x20`, seconds): the cross-fade in from the live pose (`0x7121a0`).
    pub blend_time: f32,
    /// The box centre (`+0x24`/`+0x30`): the pick broad phase tests this sphere for the current
    /// animation, placed and scaled, no pad (`0x7089c0`).
    pub bounds_center: [f32; 3],
    /// The sphere radius (`+0x3c`); `0.0` falls back to the header sphere.
    pub bounds_radius: f32,
    /// The box's min corner (`+0x24`). The blob shadow sizes from the Stand sequence's box, clamped
    /// to ±5 per axis (`0x711a20`, `0x6992c0`/`0x699250`).
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// `frequency` (`+0x14`): this variation's weight in the per-play roll (`0x7121a0`).
    pub frequency: u16,
    /// The replay range (`+0x18`/`+0x1c`): each arm rolls a play count
    /// `R = max(1, min + ⌊rand·(max−min)/32768⌋)` (`0x712692..0x7126cd`) into the play window
    /// (`0x7126d8`); a looping sequence ignores it (`0x7145f1`).
    pub min_replay: u32,
    pub max_replay: u32,
    pub bones: Vec<BoneKeys>,
    /// Event keys in this sequence's band, seconds from its start.
    pub events: Vec<AnimEvent>,
}

impl ModelAnimation {
    /// Does every bone hold the identity for the whole band? Then the doodad gate builds no rig,
    /// though the reference still arms the sequence and its other bakes still need it.
    pub fn is_rest_pose(&self) -> bool {
        const EPS: f32 = 1e-4;
        !self.bones.iter().any(|b| {
            b.translation.len() > 1
                || b.rotation.len() > 1
                || b.scale.len() > 1
                || b.translation
                    .iter()
                    .any(|(_, v)| v.iter().any(|c| c.abs() > EPS))
                || b.rotation
                    .iter()
                    .any(|(_, q)| (q[3].abs() - 1.0).abs() > EPS)
                || b.scale
                    .iter()
                    .any(|(_, s)| s.iter().any(|c| (c - 1.0).abs() > EPS))
        })
    }
}

/// One bone-channel `M2Track` (`0x1c` bytes: interp `+0`, gseq `+2`, ranges `+0x04`, timestamps
/// `+0x0c`, values `+0x14`), read once per model and sliced per sequence.
struct ChannelTrack<T> {
    /// `interp_type == 0`: hold each key, no interpolation (`0x713ea0`/`0x71af20`).
    step: bool,
    /// Global-sequence id, `0xffff` for a sequence-timeline track.
    gseq: u16,
    /// Key-index windows `(lo, hi)` by file slot, which the key search selects first (`0x713d50`);
    /// empty means the whole list (`[track+4] == 0`).
    ranges: Vec<(u32, u32)>,
    /// `(absolute ms, value)`: every sequence's keys on one timeline.
    keys: Vec<(u32, T)>,
}

type BoneChannels = (
    ChannelTrack<[f32; 3]>,
    ChannelTrack<[f32; 4]>,
    ChannelTrack<[f32; 3]>,
);

/// Read one `M2Track` whole; out-of-range reads truncate the keys, which vanilla art relies on.
fn read_channel_track<T>(
    b: &[u8],
    track: usize,
    stride: usize,
    read_value: impl Fn(&[u8], usize) -> T,
) -> ChannelTrack<T> {
    if track + 0x1c > b.len() {
        return ChannelTrack {
            step: false,
            gseq: 0xffff,
            ranges: Vec::new(),
            keys: Vec::new(),
        };
    }
    let (rn, ro) = (
        le_u32(b, track + 0x04) as usize,
        le_u32(b, track + 0x08) as usize,
    );
    let (tn, to) = (
        le_u32(b, track + 0x0c) as usize,
        le_u32(b, track + 0x10) as usize,
    );
    let vo = le_u32(b, track + 0x18) as usize;
    let ranges = (0..rn)
        .map_while(|i| {
            let e = ro + i * 8;
            (e + 8 <= b.len()).then(|| (le_u32(b, e), le_u32(b, e + 4)))
        })
        .collect();
    let keys = (0..tn)
        .map_while(|k| {
            let (t_off, v_off) = (to + k * 4, vo + k * stride);
            (t_off + 4 <= b.len() && v_off + stride <= b.len())
                .then(|| (le_u32(b, t_off), read_value(b, v_off)))
        })
        .collect();
    ChannelTrack {
        step: le_u16(b, track) == 0,
        gseq: le_u16(b, track + 0x02),
        ranges,
        keys,
    }
}

impl<T: super::key_anim::Lerp + PartialEq> ChannelTrack<T> {
    /// The keys for file slot `slot` over the band `[start, end]` (absolute ms), in seconds from
    /// its start. In-band keys go by timestamp; `ranges[slot]` is a bracket (its `hi` can sit in a
    /// later band) that decides the edges, where the reference keeps sampling (`0x713d50`): an
    /// empty band takes `keys[lo]` when `lo >= hi`, else the bracket's lerp, and past the last key
    /// the lerp runs toward `k0 + 1`, bounded by the key count, not the window. An edge key is
    /// added only where it differs from the held one. On a global sequence a lone key is a
    /// constant in every clip; a multi-key one belongs to [`parse_m2_global_sequence_bones`].
    fn band(&self, slot: usize, start: u32, end: u32) -> Vec<(f32, T)> {
        if self.keys.is_empty() {
            return Vec::new();
        }
        if self.gseq != 0xffff {
            return match self.keys.len() {
                1 => vec![(0.0, self.keys[0].1)],
                _ => Vec::new(),
            };
        }
        let last = self.keys.len() - 1;
        let (lo, hi) = self
            .ranges
            .get(slot)
            .map_or((0, last), |&(lo, hi)| (lo as usize, hi as usize));
        let at = |t| super::key_anim::sample_window(&self.keys, self.step, lo, hi, t);
        let rebase = |ts: u32| (ts.saturating_sub(start)) as f32 / 1000.0;
        let in_band: Vec<(u32, T)> = self
            .keys
            .iter()
            .copied()
            .filter(|&(ts, _)| ts >= start && ts <= end)
            .collect();
        let Some(&(first_ms, first_v)) = in_band.first() else {
            // An empty band holds the window's value; two keys only when the bracket lerp moves.
            let head = match at(start) {
                Some(v) => v,
                None => return Vec::new(),
            };
            return match at(end) {
                Some(tail) if tail != head => vec![(0.0, head), (rebase(end), tail)],
                _ => vec![(0.0, head)],
            };
        };
        let (last_ms, last_v) = in_band[in_band.len() - 1];
        let mut out = Vec::with_capacity(in_band.len() + 2);
        if first_ms > start && at(start).is_some_and(|h| h != first_v) {
            out.push((0.0, at(start).unwrap()));
        }
        out.extend(in_band.iter().map(|&(ts, v)| (rebase(ts), v)));
        if last_ms < end && at(end).is_some_and(|t| t != last_v) {
            out.push((rebase(end), at(end).unwrap()));
        }
        out
    }
}

/// Each bone's rotation, `[x, y, z, w]`, as the reference samples it at the HandsClosed frame
/// (`AnimationData` 15), through the key window: the grip overlay that keeps the weapon hand closed
/// while the body animates (`CloseHand` `0x479660`). HandsClosed keys no finger bone; its window
/// brackets it.
pub fn hand_grip_finger_poses(bytes: &[u8], bones: &[u16]) -> Vec<(u16, [f32; 4])> {
    let b = bytes;
    if b.len() < 0x40 || &b[0..4] != b"MD20" {
        return Vec::new();
    }
    let (seq_count, seq_ofs) = (le_u32(b, 0x1c) as usize, le_u32(b, 0x20) as usize);
    let (bone_count, bone_ofs) = (le_u32(b, 0x34) as usize, le_u32(b, 0x38) as usize);
    // HandsClosed's file slot, which indexes the key windows, and its start ms.
    let mut hands_closed = None;
    for s in 0..seq_count {
        let rec = seq_ofs + s * 0x44;
        if rec + 0x44 > b.len() {
            break;
        }
        if le_u16(b, rec) == 15 {
            hands_closed = Some((s, le_u32(b, rec + 0x04)));
            break;
        }
    }
    let Some((slot, frame)) = hands_closed else {
        return Vec::new();
    };
    let quat = |b: &[u8], o: usize| {
        [
            le_f32(b, o),
            le_f32(b, o + 4),
            le_f32(b, o + 8),
            le_f32(b, o + 12),
        ]
    };
    let mut out = Vec::new();
    for &bone in bones {
        let bi = bone as usize;
        if bi >= bone_count || bone_ofs + bi * 0x6c + 0x6c > b.len() {
            continue;
        }
        let tr = read_channel_track(b, bone_ofs + bi * 0x6c + 0x28, 16, quat);
        if tr.gseq != 0xffff || tr.keys.is_empty() {
            continue; // a global-sequence track runs on its own clock
        }
        let last = tr.keys.len() - 1;
        let (lo, hi) = tr
            .ranges
            .get(slot)
            .map_or((0, last), |&(lo, hi)| (lo as usize, hi as usize));
        let Some(v) = super::key_anim::sample_window(&tr.keys, tr.step, lo, hi, frame) else {
            continue;
        };
        out.push((bone, v));
    }
    out
}

/// Parse every sequence at the animate kernel's offsets (`0x714260`): sequences at MD20
/// `0x1c`/`0x20` (stride `0x44`), bones at `0x34`/`0x38` (stride `0x6c`). Zero-length sequences
/// are skipped.
pub fn parse_m2_animations(b: &[u8]) -> Vec<ModelAnimation> {
    if b.len() < 0x40 || &b[0..4] != b"MD20" {
        return Vec::new();
    }
    let (seq_count, seq_ofs) = (le_u32(b, 0x1c) as usize, le_u32(b, 0x20) as usize);
    let (bone_count, bone_ofs) = (le_u32(b, 0x34) as usize, le_u32(b, 0x38) as usize);
    let vec3 = |b: &[u8], o: usize| [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)];
    let quat = |b: &[u8], o: usize| {
        [
            le_f32(b, o),
            le_f32(b, o + 4),
            le_f32(b, o + 8),
            le_f32(b, o + 12),
        ]
    };
    // The event table (MD20 `0x114`/`0x118`, stride 44; the `M2TrackBase` at `+24` puts the
    // absolute-ms timestamps at `+36`), read once and cut per sequence like the bone keys.
    let (ev_count, ev_ofs) = (le_u32(b, 0x114) as usize, le_u32(b, 0x118) as usize);
    /// One `M2Event` record as the file holds it.
    struct EventRecord {
        ident: [u8; 4],
        data: u32,
        bone: u16,
        position: [f32; 3],
        times: Vec<u32>,
    }
    // Raw header counts: reservations cap at the bytes present, as a failed allocation aborts.
    let mut model_events: Vec<EventRecord> =
        Vec::with_capacity(capped(ev_count, 44, b.len().saturating_sub(ev_ofs)));
    for e in 0..ev_count {
        let erec = ev_ofs + e * 44;
        if erec + 44 > b.len() {
            break;
        }
        let ident: [u8; 4] = match b.get(erec..erec + 4).and_then(|s| s.try_into().ok()) {
            Some(i) => i,
            None => break,
        };
        let data = le_u32(b, erec + 4);
        let bone = le_u32(b, erec + 8) as u16;
        let position = vec3(b, erec + 12);
        let (nts, ots) = (le_u32(b, erec + 36) as usize, le_u32(b, erec + 40) as usize);
        let mut times = Vec::with_capacity(capped(nts, 4, b.len().saturating_sub(ots)));
        for t in 0..nts {
            let o = ots + t * 4;
            if o + 4 > b.len() {
                break;
            }
            times.push(le_u32(b, o));
        }
        model_events.push(EventRecord {
            ident,
            data,
            bone,
            position,
            times,
        });
    }

    let bone_tracks: Vec<BoneChannels> = (0..bone_count)
        .map_while(|i| {
            let brec = bone_ofs + i * 0x6c;
            (brec + 0x60 <= b.len()).then(|| {
                (
                    read_channel_track(b, brec + 0x0c, 12, vec3),
                    read_channel_track(b, brec + 0x28, 16, quat),
                    read_channel_track(b, brec + 0x44, 12, vec3),
                )
            })
        })
        .collect();

    let mut out = Vec::new();
    for s in 0..seq_count {
        let rec = seq_ofs + s * 0x44;
        if rec + 0x44 > b.len() {
            break;
        }
        let anim_id = le_u16(b, rec);
        let seq_index = s;
        let (start, end, flags) = (
            le_u32(b, rec + 0x04),
            le_u32(b, rec + 0x08),
            le_u32(b, rec + 0x10),
        );
        let move_speed = le_f32(b, rec + 0x0c);
        let blend_time = le_u32(b, rec + 0x20) as f32 / 1000.0;
        let (bmin, bmax) = (vec3(b, rec + 0x24), vec3(b, rec + 0x30));
        let bounds_center = [
            (bmin[0] + bmax[0]) * 0.5,
            (bmin[1] + bmax[1]) * 0.5,
            (bmin[2] + bmax[2]) * 0.5,
        ];
        let bounds_radius = le_f32(b, rec + 0x3c);
        let frequency = le_u16(b, rec + 0x14);
        let (min_replay, max_replay) = (le_u32(b, rec + 0x18), le_u32(b, rec + 0x1c));
        let duration = end.saturating_sub(start) as f32 / 1000.0;
        if duration <= 0.0 {
            continue;
        }
        let looping = flags & 1 == 0; // bit 0 clear loops, set clamps
        let mut bones = Vec::new();
        for (i, tracks) in bone_tracks.iter().enumerate() {
            let (tr, rot, sc) = tracks;
            let translation = tr.band(seq_index, start, end);
            let rotation = rot.band(seq_index, start, end);
            let scale = sc.band(seq_index, start, end);
            if !(translation.is_empty() && rotation.is_empty() && scale.is_empty()) {
                bones.push(BoneKeys {
                    bone: i as u16,
                    translation,
                    rotation,
                    scale,
                });
            }
        }
        let mut events = Vec::new();
        for r in &model_events {
            for &ts in &r.times {
                if ts >= start && ts <= end {
                    events.push(AnimEvent {
                        time: (ts - start) as f32 / 1000.0,
                        ident: r.ident,
                        data: r.data,
                        bone: r.bone,
                        position: r.position,
                    });
                }
            }
        }
        events.sort_by(|a, b| a.time.total_cmp(&b.time));

        out.push(ModelAnimation {
            anim_id,
            seq_index,
            start_ms: start,
            end_ms: end,
            duration,
            looping,
            move_speed,
            blend_time,
            bounds_center,
            bounds_radius,
            bounds_min: bmin,
            bounds_max: bmax,
            frequency,
            min_replay,
            max_replay,
            bones,
            events,
        });
    }
    out
}

/// One bone track as a global-sequence channel: `None` without a global sequence, a non-zero
/// period and two keys (a lone key is a constant [`ChannelTrack::band`] folds into every clip).
fn read_global_channel<T>(
    b: &[u8],
    track: usize,
    stride: usize,
    period_of: &impl Fn(u16) -> Option<u32>,
    read_value: impl Fn(&[u8], usize) -> T,
) -> Option<GlobalSeqChannel<T>> {
    if track + 0x1c > b.len() {
        return None;
    }
    let gseq = le_u16(b, track + 0x02);
    if gseq == 0xffff {
        return None;
    }
    let period_ms = period_of(gseq)?;
    let n = le_u32(b, track + 0x0c) as usize;
    let ts_o = le_u32(b, track + 0x10) as usize;
    let val_o = le_u32(b, track + 0x18) as usize;
    if n <= 1 {
        return None; // a constant, not a loop
    }
    // `n` is a raw count; the timestamp array bounds how many keys the file holds.
    let mut keys = Vec::with_capacity(capped(n, 4, b.len().saturating_sub(ts_o)));
    for k in 0..n {
        let (t_off, v_off) = (ts_o + k * 4, val_o + k * stride);
        if t_off + 4 > b.len() || v_off + stride > b.len() {
            break;
        }
        keys.push((le_u32(b, t_off), read_value(b, v_off)));
    }
    (keys.len() > 1).then_some(GlobalSeqChannel { period_ms, keys })
}

/// Every bone's global-sequence channels, the free-clock loops [`parse_m2_animations`] leaves out
/// (the eye blink's eyelid scale); global sequences at MD20 `0x14`/`0x18`.
pub fn parse_m2_global_sequence_bones(b: &[u8]) -> Vec<GlobalSeqBone> {
    if b.len() < 0x40 || &b[0..4] != b"MD20" {
        return Vec::new();
    }
    let (gseq_count, gseq_ofs) = (le_u32(b, 0x14) as usize, le_u32(b, 0x18) as usize);
    let period_of = |gseq: u16| -> Option<u32> {
        let i = gseq as usize;
        if i >= gseq_count {
            return None;
        }
        let o = gseq_ofs.checked_add(i * 4)?;
        if o + 4 > b.len() {
            return None;
        }
        let d = le_u32(b, o);
        (d > 0).then_some(d)
    };
    let vec3 = |b: &[u8], o: usize| [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)];
    let quat = |b: &[u8], o: usize| {
        [
            le_f32(b, o),
            le_f32(b, o + 4),
            le_f32(b, o + 8),
            le_f32(b, o + 12),
        ]
    };
    let (bone_count, bone_ofs) = (le_u32(b, 0x34) as usize, le_u32(b, 0x38) as usize);
    let mut out = Vec::new();
    for i in 0..bone_count {
        let brec = bone_ofs + i * 0x6c;
        if brec + 0x60 > b.len() {
            break;
        }
        let translation = read_global_channel(b, brec + 0x0c, 12, &period_of, vec3);
        let rotation = read_global_channel(b, brec + 0x28, 16, &period_of, quat);
        let scale = read_global_channel(b, brec + 0x44, 12, &period_of, vec3);
        if translation.is_some() || rotation.is_some() || scale.is_some() {
            out.push(GlobalSeqBone {
                bone: i as u16,
                translation,
                rotation,
                scale,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_event_and_key_counts_reserve_only_what_the_file_holds() {
        let mut b = vec![0u8; 0x11c];
        b[0..4].copy_from_slice(b"MD20");
        // u32::MAX events, of which exactly one is present …
        b[0x114..0x118].copy_from_slice(&u32::MAX.to_le_bytes());
        b[0x118..0x11c].copy_from_slice(&0x11cu32.to_le_bytes());
        // … itself claiming u32::MAX timestamps at offset 0.
        let mut ev = [0u8; 44];
        ev[36..40].copy_from_slice(&u32::MAX.to_le_bytes());
        b.extend_from_slice(&ev);
        assert!(
            parse_m2_animations(&b).is_empty(),
            "no sequences ⇒ no clips, and no abort"
        );

        // A global-sequence track claiming u32::MAX keys walks the 7 that fit its own 28 bytes.
        let mut track = [0u8; 0x1c];
        track[0x0c..0x10].copy_from_slice(&u32::MAX.to_le_bytes());
        let ch = read_global_channel(&track, 0, 4, &|_: u16| Some(1000u32), le_u32)
            .expect("more than one key on a live global sequence is a channel");
        assert_eq!(ch.keys.len(), 7);
    }

    /// `HumanMale.m2`'s one global-sequence bone, the eyelid (75), scales 0 (open) to 1 (shut).
    #[test]
    fn human_male_eyelid_blink_global_sequence() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Character\\Human\\Male\\HumanMale.m2")
            .expect("read HumanMale.m2");
        let gs = parse_m2_global_sequence_bones(&bytes);

        assert_eq!(
            gs.len(),
            1,
            "HumanMale has one global-seq bone (the eyelid)"
        );
        let eyelid = &gs[0];
        assert_eq!(eyelid.bone, 75);
        assert!(
            eyelid.translation.is_none() && eyelid.rotation.is_none(),
            "the eyelid blink is scale-only"
        );
        let scale = eyelid.scale.as_ref().expect("eyelid has a scale channel");
        assert!(
            scale.period_ms > 1000,
            "a real multi-second loop, not the empty 0/1 ms table (got {} ms)",
            scale.period_ms
        );
        assert!(scale.keys.len() >= 4);
        assert_eq!(scale.keys[0].0, 0);
        assert!(
            scale.keys[0].1.iter().all(|&c| c.abs() < 1e-3),
            "loop starts eye-open (scale 0), got {:?}",
            scale.keys[0].1
        );
        assert!(
            scale
                .keys
                .iter()
                .any(|(_, v)| v.iter().all(|&c| (c - 1.0).abs() < 1e-3)),
            "the loop has a shut frame (scale 1) — the blink"
        );
    }

    /// `UI_Human` parks a constant −16.5° yaw on its stage bone to face its off-axis camera; stage
    /// scales are 0.97 on `UI_Human`, 1.04 and 0.7324 (pet) on `UI_Tauren`.
    #[test]
    fn the_glue_stage_bones_carry_the_scenes_authored_rotation_and_scale() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        // (scene token, stage yaw °, character-stage scale, pet-stage scale)
        let scenes = [
            ("Human", -16.5_f32, 0.97_f32, 0.97_f32),
            ("Orc", 0.0, 1.0, 1.0),
            ("Dwarf", 0.0, 1.0, 1.0),
            ("NightElf", 0.0, 1.0, 1.0),
            ("Scourge", 0.0, 1.0, 1.0),
            ("Tauren", 0.0, 1.04, 0.7324),
        ];
        for (token, want_deg, want_scale, want_pet_scale) in scenes {
            let bytes = chain
                .read_file(&format!(
                    "Interface\\Glues\\Models\\UI_{token}\\UI_{token}.m2"
                ))
                .unwrap_or_else(|e| panic!("read UI_{token}.m2: {e}"));
            let parsed = benilla_m2::parse_m2(&mut std::io::Cursor::new(&bytes[..]))
                .unwrap_or_else(|e| panic!("parse UI_{token}.m2: {e:?}"));
            let model = parsed.model();
            let bone_of = |id: u16| {
                model
                    .attachments
                    .iter()
                    .find(|a| a.id == id)
                    .unwrap_or_else(|| panic!("UI_{token} has attachment {id}"))
                    .bone
            };
            let gs_all = parse_m2_global_sequence_bones(&bytes);
            // The reference composes the whole bone matrix (`T(att.pos) · parentBone[att.bone]`),
            // so a parked scale resizes what stands on it.
            let scale_of = |bone: u16| -> f32 {
                gs_all
                    .iter()
                    .find(|g| g.bone == bone)
                    .and_then(|g| g.scale.as_ref())
                    .map_or(1.0, |c| c.keys[0].1[0])
            };
            assert!(
                (scale_of(bone_of(0)) - want_scale).abs() < 5e-4,
                "UI_{token} character-stage scale: want {want_scale}, got {}",
                scale_of(bone_of(0))
            );
            assert!(
                (scale_of(bone_of(1)) - want_pet_scale).abs() < 5e-4,
                "UI_{token} pet-stage scale: want {want_pet_scale}, got {}",
                scale_of(bone_of(1))
            );

            let stage = model
                .attachments
                .iter()
                .find(|a| a.id == 0)
                .unwrap_or_else(|| panic!("UI_{token} has a stage attachment"));

            // Every shipped stage bone is a root; the walk covers a nested one.
            let gs = parse_m2_global_sequence_bones(&bytes);
            let mut yaw = 0.0_f32;
            let mut bone = stage.bone as i16;
            while bone >= 0 {
                if let Some(rot) = gs
                    .iter()
                    .find(|g| g.bone as i16 == bone)
                    .and_then(|g| g.rotation.as_ref())
                {
                    let first = rot.keys[0].1;
                    assert!(
                        rot.keys
                            .iter()
                            .all(|(_, q)| q.iter().zip(first).all(|(a, b)| (a - b).abs() < 1e-6)),
                        "UI_{token}'s stage rotation is a PARKED key, not an animation — a \
                         moving stage would have to be read off the live joint instead"
                    );
                    let [x, y, z, w] = first;
                    assert!(
                        x.abs() < 1e-3 && y.abs() < 1e-3,
                        "UI_{token}'s stage rotation is a pure YAW about +Z — a stage tipped in \
                         pitch or roll would need composing, not summing (got {first:?})"
                    );
                    yaw += 2.0 * f32::atan2(z, w).to_degrees();
                }
                bone = model.bones[bone as usize].parent;
            }
            assert!(
                (yaw - want_deg).abs() < 0.1,
                "UI_{token} stage yaw: want {want_deg}°, got {yaw}°"
            );
        }
    }

    /// HumanMale's waist (bone 27) keys rotation only outside Run's band, which `ranges[Run]`
    /// brackets as `(2, 3)`, so Run carries that value. A bone keyed anywhere is keyed in all.
    #[test]
    fn empty_band_poses_the_bone_and_never_drops_it() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Character\\Human\\Male\\HumanMale.m2")
            .expect("read HumanMale.m2");
        let seqs = parse_m2_animations(&bytes);

        let run = seqs
            .iter()
            .find(|s| s.anim_id == 5)
            .expect("HumanMale has Run");
        let waist = run
            .bones
            .iter()
            .find(|b| b.bone == 27)
            .expect("bone 27 carried into the Run clip");
        assert_eq!(
            waist.rotation.len(),
            1,
            "an empty band emits exactly the one clamp key"
        );
        assert_eq!(waist.rotation[0].0, 0.0);

        let mut keyed = [
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
        ];
        for s in &seqs {
            for b in &s.bones {
                if !b.translation.is_empty() {
                    keyed[0].insert(b.bone);
                }
                if !b.rotation.is_empty() {
                    keyed[1].insert(b.bone);
                }
            }
        }
        for (i, name) in ["translation", "rotation"].iter().enumerate() {
            for s in &seqs {
                for &bone in &keyed[i] {
                    let present = s.bones.iter().any(|b| {
                        b.bone == bone
                            && if i == 0 {
                                !b.translation.is_empty()
                            } else {
                                !b.rotation.is_empty()
                            }
                    });
                    assert!(
                        present,
                        "seq idx anim {} drops bone {bone}'s {name} — a stale-pose freeze",
                        s.anim_id
                    );
                }
            }
        }
    }

    /// `Zombie.m2`'s root translation steps from `(0,0,0)` (3333 ms) to `(0.0047,0,0)` (30000 ms);
    /// Walk's empty band `[26667, 27333]` sits nearer the second, but `ranges` holds key 0.
    #[test]
    fn empty_band_takes_the_window_key_not_a_later_one() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Creature\\Zombie\\Zombie.m2")
            .expect("read Zombie.m2");
        let seqs = parse_m2_animations(&bytes);
        for anim in [4, 5] {
            let s = seqs
                .iter()
                .find(|s| s.anim_id == anim)
                .unwrap_or_else(|| panic!("Zombie has anim {anim}"));
            let root = s
                .bones
                .iter()
                .find(|b| b.bone == 0)
                .expect("the root is carried into the clip");
            assert_eq!(
                root.translation.len(),
                1,
                "a step track's empty band holds one value across the band"
            );
            assert_eq!(
                root.translation[0].1,
                [0.0, 0.0, 0.0],
                "anim {anim} must hold key 0, not step early to the 0.0047 key at 30000 ms"
            );
        }
    }

    /// HandsClosed on `HumanMale.m2` is the 33 ms band `[60000, 60033]`, which keys nothing on
    /// finger bone 102; its window brackets it with the same curl at 43333 and 70000 ms.
    #[test]
    fn hand_grip_reads_the_curled_pose_through_the_window() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Character\\Human\\Male\\HumanMale.m2")
            .expect("read HumanMale.m2");
        let poses = hand_grip_finger_poses(&bytes, &[102]);
        let (bone, q) = *poses.first().expect("bone 102 carries a grip pose");
        assert_eq!(bone, 102);
        let want = [0.2536, -0.016, 0.0577, 0.9654];
        for (i, (&got, &w)) in q.iter().zip(want.iter()).enumerate() {
            assert!(
                (got - w).abs() < 1e-3,
                "grip quat component {i}: got {got}, want ~{w} (the 43333/70000 ms curl)"
            );
        }
    }

    /// A key window's far bracket can sit 14–64 s away (`Bear`) and must not enter the clip.
    #[test]
    fn no_clip_carries_a_key_from_another_sequences_band() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        for model in [
            "Creature\\Bear\\Bear.m2",
            "Creature\\Chicken\\Chicken.m2",
            "Character\\Human\\Male\\HumanMale.m2",
        ] {
            let bytes = chain.read_file(model).expect("read model");
            for s in parse_m2_animations(&bytes) {
                for b in &s.bones {
                    let times = b
                        .translation
                        .iter()
                        .map(|&(t, _)| ("translation", t))
                        .chain(b.rotation.iter().map(|&(t, _)| ("rotation", t)))
                        .chain(b.scale.iter().map(|&(t, _)| ("scale", t)));
                    for (ch, t) in times {
                        assert!(
                            (0.0..=s.duration + 1e-3).contains(&t),
                            "{model} anim {} bone {} {ch}: key at {t}s is outside the \
                             {}s clip — a bracket key leaked in as a playable key",
                            s.anim_id,
                            b.bone,
                            s.duration
                        );
                    }
                }
            }
        }
    }

    /// `LShoulder_Mail_PVPAlliance_C_01.m2`: two sparkle emitters on bones 2 and 3 under billboard
    /// bone 1, each at its own pivot ~0.24 yd from the host's; the held torch is bone 10 under 2.
    #[test]
    fn real_pvp_shoulder_emitters_ride_a_billboard_bone() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Item\\ObjectComponents\\Shoulder\\LShoulder_Mail_PVPAlliance_C_01.m2")
            .expect("read the R14 mail shoulder");
        let skel = parse_m2_skeleton(&bytes).expect("skeleton");
        assert_eq!(skel.bones.len(), 4);
        assert_eq!(
            skel.bones.iter().filter(|b| b.billboard.is_some()).count(),
            1,
            "one billboard bone in the chain"
        );
        assert_eq!(
            skel.bones[1].billboard,
            Some(crate::BillboardKind::Spherical),
            "bone 1 authors flag 0x08"
        );
        let emitters = crate::parse_m2_particle_emitters(&bytes).expect("emitters");
        assert_eq!(emitters.len(), 2);
        for (i, em) in emitters.iter().enumerate() {
            let bone = usize::from(em.bone);
            assert_eq!(bone, 2 + i, "the sparkle pair rides bones 2 and 3");
            assert!(
                skel.bones[bone].billboard.is_none(),
                "…neither of which carries the flag itself"
            );
            assert_eq!(
                skel.billboard_host(em.bone),
                Some(1),
                "so the host is their parent"
            );
            for c in 0..3 {
                assert!(
                    (em.position[c] - skel.bones[bone].pivot[c]).abs() < 1e-3,
                    "emitter {i} position {:?} vs bone pivot {:?}",
                    em.position,
                    skel.bones[bone].pivot
                );
            }
            let d: f32 = (0..3)
                .map(|c| (em.position[c] - skel.bones[1].pivot[c]).powi(2))
                .sum();
            assert!(
                (0.23..0.26).contains(&d.sqrt()),
                "chain offset {} yd",
                d.sqrt()
            );
        }
        let torch = chain
            .read_file("Item\\ObjectComponents\\Weapon\\Club_1H_Torch_A_01.m2")
            .expect("read the torch");
        let tskel = parse_m2_skeleton(&torch).expect("skeleton");
        let tem = crate::parse_m2_particle_emitters(&torch).expect("emitters");
        assert_eq!(tem.len(), 1);
        assert_eq!(tem[0].bone, 10);
        assert_eq!(tskel.billboard_host(10), Some(2));
        assert_eq!(
            tskel.bones[2].billboard,
            Some(crate::BillboardKind::Spherical)
        );
        // Bone 0, the root, has no billboard above it; an out-of-range index is `None`.
        assert_eq!(skel.billboard_host(0), None);
        assert_eq!(skel.billboard_host(999), None);
    }

    /// `Sparkle_A.m2` (`ItemVisuals` 28) is one spherical-billboard bone and no emitters or
    /// ribbons: a preview keeping only emitters draws nothing for it.
    #[test]
    fn a_real_item_glow_model_is_pure_billboard_geometry_with_no_emitters() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("Spells\\Enchantments\\Sparkle_A.m2")
            .expect("read the Sparkle_A item glow");
        let skel = parse_m2_skeleton(&bytes).expect("skeleton");
        assert_eq!(skel.bones.len(), 1, "one bone — the quad's own");
        assert_eq!(
            skel.bones[0].billboard,
            Some(crate::BillboardKind::Spherical),
            "…and it is a spherical billboard, so its geometry is a card"
        );
        assert!(
            crate::parse_m2_particle_emitters(&bytes)
                .expect("emitters")
                .is_empty(),
            "no particle emitters: carrying only emitters carries nothing"
        );
        assert!(
            crate::parse_m2_ribbon_emitters(&bytes)
                .expect("ribbons")
                .is_empty(),
            "nor ribbons"
        );
    }
}

#[cfg(test)]
mod doodad_sound_tests {
    /// `KalidarStreetLamp01.m2`: one looping Stand keying no bone, so no rig, and one `$DSL` at
    /// t = 0 (SoundEntries 3378): the event track must not follow the rig gate.
    #[test]
    fn the_lamps_hum_is_one_rest_posed_looping_dsl_key() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Generic\\NightElf\\Passive Doodads\\Lamps\\KalidarStreetLamp01.m2")
            .expect("the lamp is in the chain");
        let seqs = super::parse_m2_animations(&bytes);
        assert_eq!(seqs.len(), 1, "one authored sequence");

        let stand = &seqs[0];
        assert_eq!(stand.anim_id, 0, "Stand");
        assert!(stand.looping, "the hum's carrier loops");
        assert!(
            stand.is_rest_pose(),
            "keys no bone — the rig gate skips it, and that must not silence it"
        );

        assert_eq!(stand.events.len(), 1, "exactly one event key");
        let ev = &stand.events[0];
        assert_eq!(&ev.ident, b"$DSL", "a doodad sound LOOP tag");
        assert_eq!(ev.data, 3378, "SoundEntries NightElfStreetLampLoop");
        assert!(ev.time.abs() < 1e-6, "keyed at t = 0");
    }

    /// `bellows.m2` keys two `$DSL` at t = 0 and 1.1 on one 2 s loop. Each loops (`0x7a54d0`),
    /// but a doodad holds one registration (`[CMapDoodadDef+0x168]`) and a marker with another id
    /// releases it first (`0x69521d` → `0x461f80`), so the pair alternates (`sound::anim_events`).
    #[test]
    fn a_dsl_pair_on_one_cycle_alternates_through_one_slot() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let bytes = chain
            .read_file("World\\Generic\\Human\\Passive Doodads\\Bellows\\bellows.m2")
            .expect("the bellows is in the chain");
        let seqs = super::parse_m2_animations(&bytes);
        let carrier = seqs
            .iter()
            .find(|s| s.events.iter().any(|e| &e.ident == b"$DSL"))
            .expect("a sequence carries the pair");
        assert!(carrier.looping, "and it loops");

        let mut keys: Vec<_> = carrier
            .events
            .iter()
            .filter(|e| &e.ident == b"$DSL")
            .map(|e| (e.data, e.time))
            .collect();
        keys.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert_eq!(keys.len(), 2, "a pair sharing one registration slot");
        assert!(keys[0].1.abs() < 1e-6, "the first is keyed at t = 0");
        assert!(
            keys[1].1 > 1.0,
            "the second is keyed a second later ({:.3}s) — the eviction is what makes it a pump",
            keys[1].1
        );
        assert_ne!(keys[0].0, keys[1].0, "and they are different kits");
    }
}
