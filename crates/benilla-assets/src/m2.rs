//! The M2 asset loader: one [`ModelSubmesh`] per render batch, with its decoded geometry and a
//! texture dependency but no `Mesh` (see [`ModelSubmesh`]), plus the model's rig, animations,
//! effects and cameras. Materials and creature skins are applied at the spawn site.

use std::sync::Arc;

use benilla_formats::{
    hand_grip_finger_poses, parse_m2_animations, parse_m2_attachments, parse_m2_bounds,
    parse_m2_collision_hull, parse_m2_global_sequence_bones, parse_m2_lights,
    parse_m2_particle_emitters, parse_m2_playable_animation_lookup, parse_m2_portrait_camera,
    parse_m2_render_submeshes, parse_m2_skeleton, CollisionMesh, M2Bounds, M2Light,
    ParticleEmitterDef,
};
use bevy::animation::graph::AnimationGraph;
use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use bevy::reflect::TypePath;

use crate::bone_target_id;
use crate::coords::wow_to_bevy;
use crate::model::{
    arm_subtree_roots, billboard_info, build_animation_clip, build_attachments, build_global_bones,
    build_grip_clip, build_skeleton, finger_subtree_roots, in_subtree, skeleton_pivots,
    upper_subtree_root, AnimClip, ModelAnimations, ModelAttachment, ModelSkeleton, ModelSubmesh,
};

/// A loaded M2: its batches, bounds, collision hull, rig, animations, effects and cameras.
#[derive(Asset, TypePath, Clone)]
pub struct M2Model {
    pub submeshes: Vec<ModelSubmesh>,
    /// The authored bounds (model-local yards), the reference's distance-fade size.
    pub bounds: Option<M2Bounds>,
    /// The collision hull in WoW model space; `None` means the model does not collide.
    pub collision: Option<CollisionMesh>,
    /// Particle emitters, positions in WoW model space.
    pub emitters: Vec<ModelEmitter>,
    /// Ribbon emitters (trails), the reference's `CRibbonEmitter` (simulated at `0x7b7e60`).
    pub ribbons: Vec<ModelRibbon>,
    /// M2 light blocks in WoW model space; fire props and held torches author a point light.
    pub lights: Vec<ModelLight>,
    /// The rest skeleton in Bevy space; empty for a boneless model.
    pub skeleton: ModelSkeleton,
    /// The inverse bind poses, `translate(−pivot)` per bone, as a labeled sub-asset.
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    /// The animations; `None` when no bone, global sequence, emitter or material loop animates.
    pub animations: Option<ModelAnimations>,
    /// The first sequence's length in seconds, the spell-fx completion clock: the reference ends
    /// an effect after one pass of its armed sequence, whatever its bones or loop flag
    /// (`0x7194f8`). The reference arms `animationLookup[0]`'s sequence, not the first; the two
    /// coincide on every effect model checked.
    pub first_seq_span: Option<f32>,
    /// Every parsed sequence in file order, listed even when [`Self::animations`] is `None`: the
    /// UI model pane's clock reads them.
    pub sequences: Vec<M2SequenceInfo>,
    /// Attachment points (hands, sheaths), each on its bone.
    pub attachments: Vec<ModelAttachment>,
    /// The event table's positional markers, queried by 4CC, first match (`0x7130e0`/`0x7131b0`).
    pub markers: Vec<crate::ModelMarker>,
    /// The unit-frame portrait camera, `cameraLookup[0]` (`0x713540`), rendered as authored:
    /// `lookAt` and the record's perspective, with no framing on top (`0x7ac640`).
    pub portrait_camera: Option<PortraitCamera>,
    /// The camera table's first record, the glue scenes' `Model:SetCamera(0)`; their
    /// `cameraLookup[0]` is the `0xffff` none sentinel.
    pub camera0: Option<PortraitCamera>,
    /// The camera table's second record, which a `<PlayerModel>` (paper doll, inspect, pet page)
    /// renders through: raw index 1, not through `cameraLookup` (`0x505b30`, chooser `0x505890`,
    /// frozen at `0x7acf10`). `None` with fewer than two cameras, where the reference builds a
    /// fixed camera.
    pub pane_camera: Option<PortraitCamera>,
    /// The whole camera table by raw index, for `SetCamera(n)`; an index past its end leaves the
    /// pane on the orthographic leg.
    pub cameras: Vec<PaneCamera>,
    /// A bow's `$WTT`/`$WTB` string ends `[top, bottom]` as (bone, Bevy-space point), for the
    /// string drawer `0x611ff0`.
    pub string_anchors: Option<[(u16, Vec3); 2]>,
    /// The fishing pole's `$CCH` rod tip, Bevy space, where the line draw `0x61f780` starts.
    pub cch_marker: Option<Vec3>,
    /// The header's `GlobalModelFlags` (`+0x10`); bits `&3` conform the model to the ground
    /// (`0x7106c0`): 1 pitch, 3 pitch and roll, else level.
    pub global_flags: u32,
}

/// One sequence as the file's table has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct M2SequenceInfo {
    /// The `AnimationData.dbc` id (`M2Sequence+0x00`).
    pub anim_id: u16,
    /// The sequence's file slot, which the per-sequence bakes key on.
    pub seq_index: usize,
    /// `end − start` on the file's timeline, ms.
    pub duration_ms: u32,
    /// The band's start on the file's timeline, ms (`M2Sequence+0x04`): add it to a cursor before
    /// sampling an absolute track, such as a camera's.
    pub start_ms: u32,
    /// Loops (flag bit 0 clear); a clamped sequence holds its last frame.
    pub looping: bool,
}

/// An authored camera in Bevy space, at model scale 1 as the reference bakes portraits.
#[derive(Clone, Copy)]
pub struct PortraitCamera {
    pub eye: Vec3,
    pub target: Vec3,
    /// Roll about the eye-to-target axis, radians.
    pub roll: f32,
    /// The record's field of view in radians, a diagonal angle: at aspect 1, `fovy = fov/√2`.
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

/// One camera-table record: [`Self::still`] in Bevy space, tracks in WoW space behind [`Self::at`].
#[derive(Clone)]
pub struct PaneCamera {
    /// The record's `type` (0 portrait, 1 characterinfo, -1 fly-bys and glue); never selected on.
    pub camera_type: i32,
    /// The rig at rest (bases plus first keys); every camera a `<Model>` pane can name is static.
    pub still: PortraitCamera,
    /// The tracks in WoW space, kept only when one moves (the `Cameras\*.m2` fly-bys).
    tracks: Option<Arc<benilla_formats::M2CameraTracks>>,
}

impl PaneCamera {
    /// The rig at file-timeline `ms`, Bevy space, sampled with the reference's four-way `interp`.
    /// `wow_to_bevy` is linear, so converting `base + track(t)` once is exact.
    pub fn at(&self, ms: u32) -> PortraitCamera {
        let Some(t) = self.tracks.as_deref() else {
            return self.still;
        };
        let add = |b: [f32; 3], v: Option<[f32; 3]>| {
            let v = v.unwrap_or([0.0; 3]);
            wow_to_bevy([b[0] + v[0], b[1] + v[1], b[2] + v[2]])
        };
        PortraitCamera {
            eye: add(t.position_base, t.positions.sample_ms(ms)),
            target: add(t.target_base, t.target.sample_ms(ms)),
            roll: t.roll.sample_ms(ms).unwrap_or(self.still.roll),
            ..self.still
        }
    }
}

/// The billboard bone an emitter's chain reaches; see [`ModelEmitter::billboard`].
#[derive(Clone, Copy, Debug)]
pub struct EmitterBillboard {
    pub kind: benilla_formats::BillboardKind,
    /// The bone's pivot, WoW model space.
    pub pivot: [f32; 3],
    /// The bone, so a rigged consumer seats the frame on its joint, the chain above folded in.
    pub bone: u16,
}

/// One particle emitter: its parsed def and its texture dependency.
#[derive(Clone)]
pub struct ModelEmitter {
    pub def: ParticleEmitterDef,
    pub texture: Option<Handle<Image>>,
    /// The host bone's pivot, WoW model space: `def.position − bone_pivot` is the emitter in the
    /// bone's frame. Zero for an out-of-range bone.
    pub bone_pivot: [f32; 3],
    /// The billboard bone this emitter's chain reaches. The reference folds the position through
    /// the replaced palette matrix (`0x718960` at `0x7190a9`-`0x71910c`, written by `0x715868`),
    /// so the origin is `pivot + camBasis·(def.position − pivot)`. A rigged world host has it in
    /// its palette already; unrigged item models and booth bakes, whose pose drops the camera
    /// arms, need it.
    pub billboard: Option<EmitterBillboard>,
    /// The recursion model, whose emitters become this one's children (`0x7b5dd0`).
    pub recursion: Option<Handle<M2Model>>,
    /// The geometry model, drawn per particle in place of a quad (`0x7b4840`).
    pub geometry: Option<Handle<M2Model>>,
    /// How far the owner model's transparent batches sort from its origin, model-local yards:
    /// the owner-last draw-order rung's size ([`benilla_formats::m2_owner_reach`]).
    pub owner_reach: f32,
    /// The owner model's bound sphere for the water-plane side, Bevy model space: the reference
    /// classifies once per model, above when `d ≥ −r`, `r = |world matrix row 0| × radius`
    /// (`0x7083df`-`0x7084dd`). `(ZERO, 0)` without header bounds.
    pub water_bound: (Vec3, f32),
    /// The owner's loader-idle file slot ([`crate::ModelAnimations::idle_seq`]), baked here for
    /// the lanes with no rig to ask; 0 when the model builds no animations.
    pub idle_seq: usize,
}

/// The doodad content gate: whether looping `anim` differs from the static mesh, the bind pose.
/// A constant pose away from rest counts: `DuelingFlag.m2`'s Stand holds the flag 9 yd below its
/// bind pose. It decides the rig only; the playing sequence is [`ModelAnimations::idle_seq`].
fn idle_pose_differs(anim: &benilla_formats::ModelAnimation) -> bool {
    !anim.is_rest_pose()
}

/// A model path inside an M2 record (`.mdx`/`.mdl`, backslashes) as its `mpq://….m2` URL.
fn m2_dep_url(raw: &str) -> String {
    let p = raw.to_ascii_lowercase().replace('\\', "/");
    let stem = p
        .strip_suffix(".mdx")
        .or_else(|| p.strip_suffix(".mdl"))
        .or_else(|| p.strip_suffix(".m2"))
        .unwrap_or(&p);
    format!("mpq://{stem}.m2")
}

/// One ribbon emitter, riding its bone like [`ModelEmitter::bone_pivot`] (`0x718960`).
#[derive(Clone)]
pub struct ModelRibbon {
    pub def: benilla_formats::RibbonEmitterDef,
    pub texture: Option<Handle<Image>>,
    pub bone_pivot: [f32; 3],
    /// As [`ModelEmitter::owner_reach`]: a trail takes its model's owner-last rung too.
    pub owner_reach: f32,
    /// As [`ModelEmitter::water_bound`]; the ribbon leg reads the model's verdict too (`0x7081f1`).
    pub water_bound: (Vec3, f32),
}

/// One M2 light and its bone's rest pivot: the reference moves the light with the live bone each
/// frame (`0x718960`), so a torch's glow follows the hand.
#[derive(Clone, Copy)]
pub struct ModelLight {
    pub def: M2Light,
    pub bone_pivot: [f32; 3],
}

/// Bevy [`AssetLoader`] decoding `*.m2` → [`M2Model`].
#[derive(Default, TypePath)]
pub struct M2ModelLoader;

impl AssetLoader for M2ModelLoader {
    type Asset = M2Model;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        ctx: &mut LoadContext<'_>,
    ) -> Result<M2Model, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let to_io = |e: anyhow::Error| std::io::Error::other(format!("{e:#}"));

        // Skins fill at the spawn site, so no skin directory or list is passed.
        let subs = parse_m2_render_submeshes(&bytes, "", &[]).map_err(to_io)?;
        let bounds = parse_m2_bounds(&bytes).ok();
        // One owner bound for every effect: the reference draws them all after the batches.
        let owner_reach = benilla_formats::m2_owner_reach(&subs);
        // The water-plane sphere: the header box's midpoint and the header sphere's radius.
        let water_bound = bounds.as_ref().map_or((Vec3::ZERO, 0.0), |b| {
            (
                wow_to_bevy([
                    (b.bbox_min[0] + b.bbox_max[0]) * 0.5,
                    (b.bbox_min[1] + b.bbox_max[1]) * 0.5,
                    (b.bbox_min[2] + b.bbox_max[2]) * 0.5,
                ]),
                b.sphere_radius,
            )
        });
        // An empty hull is no hull: the model does not collide.
        let collision = parse_m2_collision_hull(&bytes)
            .ok()
            .filter(|c| !c.is_empty());

        let mut submeshes = Vec::with_capacity(subs.len());
        for sub in subs {
            // The URL is lowercased, since Bevy matches the `blp` extension case-sensitively, and
            // carries the wrap modes, since two modes of one texture are two uploads.
            let texture = sub
                .texture
                .as_deref()
                .map(|t| ctx.load::<Image>(crate::texture_url(t, (sub.wrap_x, sub.wrap_y))));
            submeshes.push(ModelSubmesh {
                texture,
                skin_slot: sub.skin_slot,
                geoset_id: sub.geoset_id,
                char_slot: sub.char_slot,
                icon_slot: sub.icon_slot,
                blend: sub.blend,
                two_sided: sub.two_sided,
                interior: false,        // M2 has no interior/exterior group concept
                emissive: sub.emissive, // M2 UNLIT (0x01) glass/glow → fullbright
                sidn: None,             // WMO-only (MOMT SIDN night glow)
                window: false,          // WMO-only (MOMT WINDOW interior light)
                additive: sub.additive, // M2 blend 3/4 → additive (glow cards)
                env_map: sub.env_map, // texture_unit_lookup > 2 → the runtime generates this stage's UVs
                no_depth_write: sub.no_depth_write, // M2 render flag 0x10
                no_depth_test: sub.no_depth_test, // M2 render flag 0x08
                fog_policy: sub.fog_policy, // M2 render flag 0x02 / the per-blend fog table
                billboard: billboard_info(&sub), // glow cards / chains face the camera
                alpha_anim: sub.alpha_anim.clone().map(std::sync::Arc::new),
                uv_anim: sub.uv_anim.clone().map(std::sync::Arc::new),
                uv_seq: sub.uv_seq.clone().map(std::sync::Arc::new),
                uv_rot_seq: sub.uv_rot_seq.clone().map(std::sync::Arc::new),
                uv_scale_seq: sub.uv_scale_seq.clone().map(std::sync::Arc::new),
                rgb_anim: sub.rgb_anim.clone().map(std::sync::Arc::new),
                rgb_seq: sub.rgb_seq.clone().map(std::sync::Arc::new),
                wmo_batch: None,                // M2 batches have no MOBA section
                ground_quad: sub.ground_quad(), // flat ground-ring quads → the fx decal lane
                geometry: std::sync::Arc::new(sub),
            });
        }
        // The spawn site keeps the casting point lights (`M2Light::casts`).
        let light_defs = parse_m2_lights(&bytes);

        let skeleton_raw = parse_m2_skeleton(&bytes).unwrap_or_default();

        let mut emitters: Vec<ModelEmitter> = parse_m2_particle_emitters(&bytes)
            .unwrap_or_default()
            .into_iter()
            .map(|def| {
                let texture = def.texture.as_deref().map(|t| {
                    ctx.load::<Image>(format!(
                        "mpq://{}",
                        t.replace('\\', "/").to_ascii_lowercase()
                    ))
                });
                let bone_pivot = skeleton_raw
                    .bones
                    .get(def.bone as usize)
                    .map_or([0.0; 3], |b| b.pivot);
                let recursion = def
                    .recursion_model
                    .as_deref()
                    .map(|p| ctx.load::<M2Model>(m2_dep_url(p)));
                let geometry = def
                    .geometry_model
                    .as_deref()
                    .map(|p| ctx.load::<M2Model>(m2_dep_url(p)));
                let billboard = skeleton_raw.billboard_host(def.bone).and_then(|h| {
                    let b = skeleton_raw.bones.get(h)?;
                    Some(EmitterBillboard {
                        kind: b.billboard?,
                        pivot: b.pivot,
                        bone: u16::try_from(h).ok()?,
                    })
                });
                ModelEmitter {
                    def,
                    texture,
                    bone_pivot,
                    billboard,
                    recursion,
                    geometry,
                    owner_reach,
                    water_bound,
                    idle_seq: 0, // stamped below, once the sequences are built
                }
            })
            .collect();

        let ribbons: Vec<ModelRibbon> = benilla_formats::parse_m2_ribbon_emitters(&bytes)
            .unwrap_or_default()
            .into_iter()
            .map(|def| {
                let texture = def.texture.as_deref().map(|t| {
                    ctx.load::<Image>(format!(
                        "mpq://{}",
                        t.replace('\\', "/").to_ascii_lowercase()
                    ))
                });
                let bone_pivot = skeleton_raw
                    .bones
                    .get(def.bone as usize)
                    .map_or([0.0; 3], |b| b.pivot);
                ModelRibbon {
                    def,
                    texture,
                    bone_pivot,
                    owner_reach,
                    water_bound,
                }
            })
            .collect();

        let lights = light_defs
            .into_iter()
            .map(|def| ModelLight {
                bone_pivot: skeleton_raw
                    .bones
                    .get(def.bone as usize)
                    .map_or([0.0; 3], |b| b.pivot),
                def,
            })
            .collect();
        let (skeleton, inverse_bindposes) = build_skeleton(&skeleton_raw);
        let inverse_bindposes = ctx.add_labeled_asset(
            "inverse_bindposes".to_string(),
            SkinnedMeshInverseBindposes::from(inverse_bindposes),
        );

        // Attachments, markers and clip events all take the pivots the joints were built from.
        let attachments = build_attachments(
            &parse_m2_attachments(&bytes).unwrap_or_default(),
            &skeleton_pivots(&skeleton_raw),
        );
        let markers = crate::model::build_markers(
            &benilla_formats::parse_m2_event_markers(&bytes).unwrap_or_default(),
            &skeleton_pivots(&skeleton_raw),
        );
        let pivots = skeleton_pivots(&skeleton_raw);

        let sequences = parse_m2_animations(&bytes);
        let first_seq_span = sequences.first().map(|a| a.duration).filter(|d| *d > 0.0);
        let sequence_infos: Vec<M2SequenceInfo> = sequences
            .iter()
            .map(|a| M2SequenceInfo {
                anim_id: a.anim_id,
                seq_index: a.seq_index,
                duration_ms: a.end_ms.saturating_sub(a.start_ms),
                start_ms: a.start_ms,
                looping: a.looping,
            })
            .collect();
        let animations = {
            let mut graph = AnimationGraph::new();
            let root = graph.root;
            // The pose bake mirrors the graph: update it beside every mask group and `add_clip*`.
            let mut pose = crate::model::PoseSource {
                bone_masks: vec![0u64; skeleton_raw.bones.len()],
                ..Default::default()
            };
            // Groups 0 and 1 hold every joint outside the right and the left arm, so a node masked
            // with one animates only that arm.
            let arm_roots = arm_subtree_roots(&skeleton_raw, &attachments);
            if let Some((right_root, left_root)) = arm_roots {
                for i in 0..skeleton_raw.bones.len() {
                    let target = bone_target_id(i as u16);
                    if !in_subtree(&skeleton_raw, i, right_root) {
                        graph.add_target_to_mask_group(target, 0);
                        pose.bone_masks[i] |= 1 << 0;
                    }
                    if !in_subtree(&skeleton_raw, i, left_root) {
                        graph.add_target_to_mask_group(target, 1);
                        pose.bone_masks[i] |= 1 << 1;
                    }
                }
            }
            // Group 2 holds every joint outside the upper body: the legs and the pelvis.
            let upper_root = upper_subtree_root(&skeleton_raw);
            if let Some(upper_root) = upper_root {
                for i in 0..skeleton_raw.bones.len() {
                    if !in_subtree(&skeleton_raw, i, upper_root) {
                        graph.add_target_to_mask_group(bone_target_id(i as u16), 2);
                        pose.bone_masks[i] |= 1 << 2;
                    }
                }
            }
            // Groups 3 and 4 hold every joint outside the right and the left fingers, for the
            // weapon grip (`CloseHand`, `0x479660`).
            let finger_roots = finger_subtree_roots(&skeleton_raw);
            for i in 0..skeleton_raw.bones.len() {
                let target = bone_target_id(i as u16);
                if !finger_roots[0].is_empty()
                    && !finger_roots[0]
                        .iter()
                        .any(|&r| in_subtree(&skeleton_raw, i, r))
                {
                    graph.add_target_to_mask_group(target, 3);
                    pose.bone_masks[i] |= 1 << 3;
                }
                if !finger_roots[1].is_empty()
                    && !finger_roots[1]
                        .iter()
                        .any(|&r| in_subtree(&skeleton_raw, i, r))
                {
                    graph.add_target_to_mask_group(target, 4);
                    pose.bone_masks[i] |= 1 << 4;
                }
            }
            // Read before the clip walk: the loader-idle seed resolves through it.
            let playable_animation_lookup =
                parse_m2_playable_animation_lookup(&bytes).unwrap_or_default();
            let animation_lookup =
                benilla_formats::parse_m2_animation_lookup(&bytes).unwrap_or_default();
            let mut clips = Vec::new();
            // The loader-idle seed (`0x71019b`): id 0 via the playable lookup, not file slot 0.
            let idle_id = playable_animation_lookup
                .first()
                .map_or(0, |p: &benilla_formats::PlayableAnim| p.resolved_id);
            let mut first_seq = None;
            for (i, anim) in sequences.iter().enumerate() {
                // Every sequence becomes a clip, since the clip is the sequence clock;
                // `poses_bones` says whether it also needs a rig.
                {
                    let (clip, pose_clip, poses_bones) = build_animation_clip(anim, &skeleton);
                    // `first_seq` asks whether the seed needs a rig; `ModelAnimations::idle_seq`
                    // makes the same selection without that gate.
                    if first_seq.is_none() && anim.anim_id == idle_id && idle_pose_differs(anim) {
                        first_seq = Some(clips.len());
                    }
                    let pose_idx = pose.clips.len() as u32;
                    pose.clips.push(pose_clip);
                    let clip = ctx.add_labeled_asset(format!("clip{i}"), clip);
                    let node = graph.add_clip(clip.clone(), 1.0, root);
                    pose.set_node(node, pose_idx, 0);
                    // The sheath family (89/90) gets per-arm masked nodes.
                    let arm_nodes =
                        (matches!(anim.anim_id, 89 | 90) && arm_roots.is_some()).then(|| {
                            let nodes = (
                                graph.add_clip_with_mask(clip.clone(), 1 << 0, 1.0, root),
                                graph.add_clip_with_mask(clip.clone(), 1 << 1, 1.0, root),
                            );
                            pose.set_node(nodes.0, pose_idx, 1 << 0);
                            pose.set_node(nodes.1, pose_idx, 1 << 1);
                            nodes
                        });
                    // With an upper-body root, every clip gets an upper-body node, so a one-shot
                    // can play over a live base.
                    let upper_node = upper_root.map(|_| {
                        let n = graph.add_clip_with_mask(clip.clone(), 1 << 2, 1.0, root);
                        pose.set_node(n, pose_idx, 1 << 2);
                        n
                    });
                    clips.push(AnimClip {
                        anim_id: anim.anim_id,
                        seq_index: anim.seq_index,
                        node,
                        looping: anim.looping,
                        duration: anim.duration,
                        move_speed: anim.move_speed,
                        blend_time: anim.blend_time,
                        // Into the mesh vertices' frame, so the pick's world math is one transform.
                        bounds_center: wow_to_bevy(anim.bounds_center),
                        bounds_radius: anim.bounds_radius,
                        // The axis mapping flips signs, so min/max are re-derived componentwise.
                        bounds_min: wow_to_bevy(anim.bounds_min).min(wow_to_bevy(anim.bounds_max)),
                        bounds_max: wow_to_bevy(anim.bounds_min).max(wow_to_bevy(anim.bounds_max)),
                        // `offset` and `point` both use the record's own bone, never a by-4CC
                        // lookup: a model may author a tag more than once.
                        events: anim
                            .events
                            .iter()
                            .map(|e| crate::ClipEvent {
                                time: e.time,
                                ident: e.ident,
                                data: e.data,
                                bone: e.bone,
                                offset: wow_to_bevy(e.position)
                                    - pivots.get(e.bone as usize).copied().unwrap_or(Vec3::ZERO),
                                point: wow_to_bevy(e.position),
                            })
                            .collect::<Vec<_>>()
                            .into(),
                        arm_nodes,
                        upper_node,
                        frequency: anim.frequency,
                        replay: (anim.min_replay, anim.max_replay),
                        poses_bones,
                    });
                }
            }
            // The grip is its own clip of clamped `HandsClosed` finger poses: the sequence read
            // drops fingers keyed off that frame, which `hand_grip_finger_poses` clamps.
            let mut hand_close: [Option<bevy::animation::graph::AnimationNodeIndex>; 2] =
                [None, None];
            // Every bone of each finger subtree, tips included, since `HandsClosed` keys them too.
            let finger_bones: Vec<u16> = (0..skeleton_raw.bones.len())
                .filter(|&i| {
                    finger_roots
                        .iter()
                        .flatten()
                        .any(|&r| in_subtree(&skeleton_raw, i, r))
                })
                .map(|i| i as u16)
                .collect();
            let grip_poses = hand_grip_finger_poses(&bytes, &finger_bones);
            if !grip_poses.is_empty() {
                let (grip_clip, grip_pose) = build_grip_clip(&grip_poses);
                let grip_idx = pose.clips.len() as u32;
                pose.clips.push(grip_pose);
                let grip = ctx.add_labeled_asset("grip_clip".to_string(), grip_clip);
                if !finger_roots[0].is_empty() {
                    let n = graph.add_clip_with_mask(grip.clone(), 1 << 3, 1.0, root);
                    pose.set_node(n, grip_idx, 1 << 3);
                    hand_close[0] = Some(n);
                }
                if !finger_roots[1].is_empty() {
                    let n = graph.add_clip_with_mask(grip.clone(), 1 << 4, 1.0, root);
                    pose.set_node(n, grip_idx, 1 << 4);
                    hand_close[1] = Some(n);
                }
            }
            // Global-sequence channels alone earn `ModelAnimations` (the `GlobalSeqOnly` tier).
            let global_bones =
                build_global_bones(&parse_m2_global_sequence_bones(&bytes), &skeleton);
            // A model animates if a bone poses, a global sequence runs, or something samples the
            // sequence clock: emitters, ribbons, or a material's alpha, colour or UV loop.
            let samples_sequence = !emitters.is_empty()
                || !ribbons.is_empty()
                || submeshes
                    .iter()
                    .any(|s| s.alpha_anim.is_some() || s.uv_anim.is_some() || s.rgb_anim.is_some());
            (clips.iter().any(|c| c.poses_bones)
                || !global_bones.is_empty()
                || (samples_sequence && !clips.is_empty()))
            .then(|| {
                let graph = ctx.add_labeled_asset("anim_graph".to_string(), graph);
                ModelAnimations {
                    graph,
                    clips,
                    playable_animation_lookup,
                    animation_lookup,
                    hand_close,
                    global_bones,
                    first_seq,
                    pose: std::sync::Arc::new(pose),
                }
            })
        };

        // Stamped only now: the idle selection needs the built clips.
        let idle_seq = animations.as_ref().and_then(ModelAnimations::idle_seq);
        for e in &mut emitters {
            e.idle_seq = idle_seq.unwrap_or(0);
        }

        // Cameras take the meshes' WoW-to-Bevy rotation, so eye and target share their frame.
        let to_bevy = |c: benilla_formats::M2PortraitCamera| PortraitCamera {
            eye: wow_to_bevy(c.position),
            target: wow_to_bevy(c.target),
            roll: c.roll,
            fov: c.fov,
            near: c.near_clip,
            far: c.far_clip,
        };
        let portrait_camera = parse_m2_portrait_camera(&bytes).map(to_bevy);
        let camera0 = benilla_formats::parse_m2_camera(&bytes, 0).map(to_bevy);
        let pane_camera = benilla_formats::parse_m2_camera(&bytes, 1).map(to_bevy);
        let cameras: Vec<PaneCamera> = benilla_formats::parse_m2_pane_cameras(&bytes)
            .into_iter()
            .map(|c| PaneCamera {
                camera_type: c.camera_type,
                still: to_bevy(c.still),
                tracks: c.tracks.map(|t| Arc::new(*t)),
            })
            .collect();

        let string_anchors = benilla_formats::parse_m2_string_anchors(&bytes).map(|a| {
            [
                (a.top.0, wow_to_bevy(a.top.1)),
                (a.bottom.0, wow_to_bevy(a.bottom.1)),
            ]
        });

        let cch_marker = benilla_formats::parse_m2_cch_marker(&bytes).map(|(_, p)| wow_to_bevy(p));

        // `GlobalModelFlags` sits at MD20 `+0x10`, outside every sectioned parser.
        let global_flags = bytes
            .get(0x10..0x14)
            .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));

        Ok(M2Model {
            submeshes,
            bounds,
            collision,
            emitters,
            ribbons,
            lights,
            skeleton,
            inverse_bindposes,
            animations,
            first_seq_span,
            sequences: sequence_infos,
            attachments,
            markers,
            portrait_camera,
            camera0,
            pane_camera,
            cameras,
            string_anchors,
            cch_marker,
            global_flags,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["m2"]
    }
}
