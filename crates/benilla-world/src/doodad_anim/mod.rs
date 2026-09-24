//! The doodad animation host (phase 1) — world-placed M2 doodads (ADT MDDF + WMO MODD
//! props) animate: flags wave, windmills turn, flame bones jiggle.
//!
//! Byte ground: a doodad's M2
//! instance is armed at load — bone 0, animation id 0, `linkFlag=1` — and then **re-arms itself every
//! play-window, for ever**, rolling a fresh frequency-weighted variation each time (decision
//! 0768: the watchdog `0x719370` fires on `now ≥ windowHi`, enqueues the doodad-only completion
//! callback `0x6951b0` that `0x695100` installed at `[model+0x70]`, and that callback re-runs op4
//! with `variationIdx = -1`, which writes the next `windowHi` and clears the latch — self-sustaining).
//! **Global sequences loop with zero arming**, clock-driven. The cycle is gated on **linkage** — the
//! per-frame walk `0x7074b0` only advances models spliced into the scene list, which the doodad drain
//! `0x683f80` does per **in-range** doodad — not on the draw, so a doodad behind the camera keeps
//! cycling. Because sampling is clock-indexed (`cursor = clock − startOffset`), a re-appearing model
//! shows the pose the shared clock dictates — pausing costs nothing and drifts nothing.
//!
//! benilla's translation: the spawn site ([`crate::terrain_stream`]) classifies each placed model by
//! [`classify`] — the ~90% with no animated channel stay on today's static path (measured,
//! `benilla-extract doodadscan`) — and an animated one spawns the skinned twin + a collapsed
//! [`RigPose`] buffer (no joint entities; consumers ride on-demand anchors) on an
//! anim-root entity carrying [`DoodadAnimHost`]: an `AnimationPlayer` on the armed clip,
//! re-rolled every window by [`reroll_doodad_variation`], and/or a [`GlobalSeqDrive`] for the
//! free-running channels. [`gate_doodad_anim`] here is the draw-time tick made explicit: animation
//! runs iff any of the doodad's submeshes is actually drawn (the `Visibility` verdict the debug-panel
//! authority composes from far-clip + the size-bucketed distance fade + the WMO portal cull), and a
//! resume seeks to `(now − armed_at) mod duration` — the ref's shared-clock phase, measured from the
//! current arm. Note the two gates are deliberately different: the *draw* gates the pose, the
//! *linkage* (here: residency) gates the variation cycle.

use bevy::animation::graph::AnimationNodeIndex;
use bevy::app::AnimationSystems;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use benilla_assets::{AnimClip, M2Model, ModelAnimations, ModelSkeleton};

use crate::rig_anim::{AnimParked, GlobalSeqDrive, RigPose};
use crate::vis_chain::VisChainOnly;

mod lazy;
mod mat_anim;
pub(crate) use lazy::{LazyRig, SkinnedTwin};
use mat_anim::tick_anim_materials;
pub use mat_anim::{
    playing_seq, register_entity_uv, register_fx_uv, register_tint, sample_mat_anim, AnimMatPart,
    MatAnim, TintAnimMaterials, TintLoop, UvAnimMaterials, UvLoops,
};
pub(crate) use mat_anim::{register_uv, UvLoop};

/// What a placed doodad model animates — decision 0130's content gate, decided per model at spawn.
pub enum DoodadAnimTier<'a> {
    /// No animated bone channel: today's static path, untouched (no joints, no player — the ~90%
    /// measured case: trees, fences, barrels, rocks).
    Static,
    /// Only free-running global-sequence channels (candelabra glow pulses): joints +
    /// [`GlobalSeqDrive`], **no** `AnimationPlayer` — the cheapest animated tier.
    GlobalSeqOnly,
    /// The **loader-idle seed** moves bones (flags, windmills, torch flames): joints + an
    /// `AnimationPlayer` looping this clip — the client's one-time load arm. That seed is animation
    /// id 0 (Stand) resolved through the model's own `playableAnimationLookup`, NOT the file-order
    /// first sequence (the variant name predates the correction and is kept because
    /// it is the tier's identity, not a claim about which sequence). Global-sequence channels ride
    /// along if the model also has them.
    FirstSeq(&'a AnimClip),
}

/// The animation-event tags that make a placed doodad SOUND: `$DSL` (the looping emitter), `$DSO`
/// (a positioned one-shot), `$DSE` (the stop token that releases a `$DSL` — `data` is 0 on all 16
/// shipped models) and the generic `$SND`. Routed by `benilla_app::sound::anim_events`; listed here
/// because their presence is what earns a bind-posed model a clock it would otherwise be denied
/// (see [`arms_for_sound`]). Byte law: the model's animation-event callback `0x6951e0` routes on
/// these marker FourCCs.
pub(crate) const SOUND_EVENT_TAGS: [&[u8; 4]; 4] = [b"$DSL", b"$DSO", b"$DSE", b"$SND"];

/// Does this model's **arm chain** — the loader-idle seed and its variations, the only sequences a
/// placed doodad ever plays — carry a sound-event marker?
///
/// This is deliberately NOT part of [`classify`]: the tier answers "what does this model *render*",
/// and 0130's whole point is that a bind-posed sequence renders as the static mesh. That stays true.
/// What it never meant is that the sequence does not *run* — the reference arms every placed doodad
/// it creates (`0x695100` → `0x7121a0`) and cycles it whenever the doodad is in
/// the frame's animate set, and the event track rides that clock. A humming
/// lamp is the proof: `KalidarStreetLamp01.m2` is one looping 3.333 s Stand that keys no bone at all
/// and carries exactly one event, `$DSL` → `NightElfStreetLampLoop`. Gating its clock on the *rig*
/// is what made every world doodad silent (bug B345), and the corpus says the class is not marginal
/// — `benilla-extract soundeventscan`: 273 carrier models, 108 of which no rig gate would ever arm.
///
/// Same shape as [`ModelAnimations::idle_clip`]'s own note about emitters and material loops: arming
/// the idle seed is what gives a consumer a running clock instead of a frozen slot-0 read. Sound is
/// simply the third consumer.
pub(crate) fn arms_for_sound(anims: &ModelAnimations) -> bool {
    let Some(idle) = anims.idle_clip() else {
        return false;
    };
    anims
        .clips
        .iter()
        .filter(|c| c.anim_id == idle.anim_id)
        .any(|c| {
            c.events
                .iter()
                .any(|e| SOUND_EVENT_TAGS.contains(&&e.ident))
        })
}

/// Classify a placed model for the spawn site. Boneless models are `Static` regardless of parsed
/// tracks — their skinned twin carries joint attributes with no joints to index (the 0035 guard).
pub fn classify<'a>(
    skeleton: &ModelSkeleton,
    animations: Option<&'a ModelAnimations>,
) -> DoodadAnimTier<'a> {
    let Some(anims) = animations else {
        return DoodadAnimTier::Static;
    };
    if skeleton.joints.is_empty() {
        return DoodadAnimTier::Static;
    }
    // `first_seq` — 0130's content gate, and only that (decision 0936's split; the loader arm's
    // *identity* answer is `idle_seq`/`idle_clip`, which the entity lane arms from). It stays the
    // rig question here: a seed that holds bind pose renders identically to the static mesh.
    match anims.first_seq.and_then(|i| anims.clips.get(i)) {
        Some(clip) => DoodadAnimTier::FirstSeq(clip),
        None if !anims.global_bones.is_empty() => DoodadAnimTier::GlobalSeqOnly,
        None => DoodadAnimTier::Static,
    }
}

/// What [`spawn_anim_host`] set up for one animated placement — the spawn site arms the
/// [`DoodadAnimHost`] on `root` with the submesh list once it has spawned them.
///
/// Since decision 1365 there are NO joint entities: the pose lives in a [`RigPose`] buffer,
/// carried here BY VALUE so the caller can mint consumer anchors ([`Self::anchor`]) for the bones
/// something actually rides — billboard cards, emitters, ribbons — and then attach the buffer
/// with [`Self::finish`]. Only consumed bones ever get an entity (the 1354 census: 96 % of the
/// old eagerly-spawned population hosted nothing).
pub struct AnimHostSpawn {
    pub root: Entity,
    /// The collapsed rig's pose buffer, not yet inserted — [`Self::finish`] attaches it to
    /// `root` once every consumer anchor is minted.
    pose: RigPose,
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    /// The looping first-sequence node + duration, `None` on the gseq-only tier.
    pub(crate) clip: Option<(AnimationNodeIndex, f32)>,
    /// The **file sequence slot** the load arm seeded ([`AnimClip::seq_index`]) — the axis every
    /// per-sequence bake is keyed on. `None` on the gseq-only tier (no arm).
    ///
    /// This is the *loader's* var-0 seed only. It is no longer what the placement ends up playing:
    /// [`reroll_doodad_variation`] overrides it on the first frame with a real weighted roll and
    /// keeps re-rolling every play-window, exactly as the reference's holder setup
    /// `0x695100` lands its `variationIdx = -1` arm after the loader's var-0 one. Consumers that
    /// must track the *current* slot therefore read the host's live player instead — the emitters
    /// ride [`crate::particles::EmitClock::Host`].
    pub(crate) seq: Option<usize>,
    /// The animation id the arm rolls its variation over (id 0 — the loader-idle seed's id, resolved
    /// through the model's own `playableAnimationLookup`). `None` on the gseq-only tier: nothing was
    /// armed, so there is no play-window and nothing to re-roll.
    pub(crate) anim_id: Option<u16>,
}

impl AnimHostSpawn {
    /// The anchor entity standing in for `bone` — spawned on first demand, shared on repeat asks
    /// ([`RigPose::anchor_for`]). `None` = the bone is outside this skeleton (the consumer
    /// misses, exactly as `joints.get(bone)` used to).
    pub fn anchor(&mut self, commands: &mut Commands, bone: u16) -> Option<Entity> {
        self.pose.anchor_for(commands, self.root, bone)
    }

    /// The skeleton's bone count — what a palette allocation sizes itself with now that there is
    /// no joint list to measure.
    pub fn bones(&self) -> u32 {
        self.pose.locals.len() as u32
    }

    /// Attach the pose buffer to the host root. Called once, after the caller minted its
    /// anchors; returns the `(bone, anchor)` registry for consumers that resolve after this
    /// point (the placement's emitter/ribbon spawns).
    pub fn finish(self, commands: &mut Commands) -> Vec<(u16, Entity)> {
        let anchors = self.pose.anchors.clone();
        commands.entity(self.root).insert(self.pose);
        anchors
    }
}

/// Whether [`spawn_anim_host`] would rig this model — the model-forms requester's twin of the
/// spawn gate, so the skinned twins are built exactly for the placed models that
/// will draw them. Mirrors both of the host's gates: the tier, and the capture freeze (captures
/// keep every doodad static, so they need no twins either).
pub fn wants_rig(m: &M2Model) -> bool {
    !crate::dev_state::deterministic_run()
        && !matches!(
            classify(&m.skeleton, m.animations.as_ref()),
            DoodadAnimTier::Static
        )
}

/// Spawn the animation host for one placed M2, if [`classify`] says it animates: an anim-root entity
/// at the placement transform carrying the collapsed [`RigPose`] buffer (no joint
/// entities; consumer anchors are minted on demand and cascade with the root),
/// and — per tier — an `AnimationPlayer` looping the **loader-idle seed's** clip (the client's
/// one-time load arm at `0x70ebd0`: `0x7121a0(bone 0, animation id 0 resolved through the model's
/// own `playableAnimationLookup`, linkFlag=1)` once — decision 0637) and/or
/// the free-running [`GlobalSeqDrive`] (bone-index targets, the collapsed lane). `None` ⇒ the
/// model is static and the caller keeps today's path untouched.
pub fn spawn_anim_host(
    commands: &mut Commands,
    m: &M2Model,
    transform: Transform,
) -> Option<AnimHostSpawn> {
    // The visual A/B harness needs deterministic frames; a live animation clock
    // isn't. Captures keep every doodad on the static path — bind pose renders identically to the
    // static mesh, so world baselines stay comparable across runs and branches.
    if crate::dev_state::deterministic_run() {
        return None;
    }
    let tier = classify(&m.skeleton, m.animations.as_ref());
    // The SOUND arm (bug B345), orthogonal to the tier: a model whose pose is bind renders as the
    // static mesh — 0130's gate is right about pixels — but its event track still has to run, and
    // the reference runs it for every placed doodad. So a `Static`/`GlobalSeqOnly` model that
    // authors `$DSL`/`$DSO`/`$SND` on its arm chain still gets a HOST here: `ModelAnimations` and
    // the arm bookkeeping (`clip`/`anim_id`, which is what [`reroll_doodad_variation`] maintains and
    // the event scanner reads), and deliberately **no `AnimationPlayer`** — there is no pose to
    // evaluate, and paying for a rig to run a clock is exactly what 0130 exists to avoid.
    let sound_arm = m.animations.as_ref().is_some_and(arms_for_sound);
    if matches!(tier, DoodadAnimTier::Static) && !sound_arm {
        return None;
    }
    let anims = m
        .animations
        .as_ref()
        .expect("animated tier or sound arm ⇒ animations");
    // Chain-only visibility (`crate::vis_chain`): the host root renders nothing — its parts
    // draw as world roots — and ~1.4k hosts sat in the per-camera sweep at the Goldshire pin.
    let root = commands
        .spawn((transform, Visibility::default()))
        .vis_chain_only()
        .id();
    // The collapsed pose buffer (0724 replayed for this lane): no joint
    // entities, no `AnimatedBy` targets, no `BillboardJointRig`. The 0712 evaluator poses
    // `locals` off the player, the model pass folds + seats the anchors, and the world pass
    // handles the billboard/arm bones and the palette rows.
    //
    // NO palette rig here either. The host's slot allocation is the CALLER's
    // policy: the terrain-stream lane goes lazy — parts spawn on the static form and the draw
    // gate allocates at the first wake ([`LazyRig`]) — because the resident population is
    // thousands of parked hosts while the 2048-slot table is the scarce axis (the 0863 census:
    // 1300–1750 parked of ~1900 live slots, active peak ~630). A gate-less caller (quest
    // markers) allocates eagerly itself, or its host would never skin.
    let pose = RigPose::new(root, &m.skeleton);
    let mut clip_info = None;
    let mut armed_seq = None;
    let mut arm_id = None;
    if let DoodadAnimTier::FirstSeq(head) = tier {
        // (the rigged arm — player + graph + variation chain)
        // The **loader's** seed only — `0x70ebd0`'s var-0 arm on the head of the chain. The real
        // pick is the holder setup's second op4 call (`0x695100`, `variationIdx = -1`), which lands
        // *after* it and is the effective arm; here that second arm is
        // [`reroll_doodad_variation`]'s first pass, which fires the same frame because the host is
        // born with an already-expired window. Splitting it that way is not a convenience: it is
        // the one code path the reference has, since every later re-arm is byte-identical to the
        // holder's first one.
        let mut player = AnimationPlayer::default();
        player.play(head.node).repeat();
        commands.entity(root).insert((
            player,
            AnimationGraphHandle(anims.graph.clone()),
            // The re-roll needs the variation chain, and — now that a placed doodad's played
            // sequence CHANGES — so does every per-sequence consumer that resolves off this host
            // (`playing_seq`): the emitters' rate/enable tracks, the material-alpha sampler.
            anims.clone(),
        ));
        clip_info = Some((head.node, head.duration));
        armed_seq = Some(head.seq_index);
        arm_id = Some(head.anim_id);
    } else if sound_arm {
        // The clock-only arm: same loader seed, same self-sustaining re-roll — `ModelAnimations` is
        // what [`reroll_doodad_variation`] needs, and without it the host would never be matched by
        // that query and its window would never advance. No player, no graph handle: nothing here
        // poses anything, and `gate_doodad_anim` already treats both as optional.
        let head = anims.idle_clip().expect("sound arm ⇒ an idle clip");
        clip_info = Some((head.node, head.duration));
        arm_id = Some(head.anim_id);
        commands.entity(root).insert(anims.clone());
        // `armed_seq` stays `None` ON PURPOSE. It is the *only* input to `PlacementHost::arm`,
        // which is what puts this placement's emitters on `EmitClock::Host` — a clock that reads
        // the host's `AnimationPlayer`. This host has none, so claiming the arm would move a
        // previously-`Pinned` emitter onto a player that does not exist. The sound clock is read
        // from `clip`/`armed_at` instead, which needs no player and no slot number.
    }
    if let Some(drive) = GlobalSeqDrive::new_rig(&anims.global_bones, m.skeleton.joints.len()) {
        commands.entity(root).insert(drive);
    }
    Some(AnimHostSpawn {
        root,
        pose,
        inverse_bindposes: m.inverse_bindposes.clone(),
        clip: clip_info,
        seq: armed_seq,
        anim_id: arm_id,
    })
}

/// The anim-root component of one animated doodad placement: the draw gate's state + what to re-arm
/// on resume. The root entity carries the placement transform, the joint hierarchy as children, and
/// (per tier) the `AnimationPlayer`/[`GlobalSeqDrive`]; it lives in the placement's entity list, so
/// it despawns (joints cascading) when the tile streams out.
#[derive(Component)]
pub struct DoodadAnimHost {
    /// The placement's skinned submesh entities — animation runs iff ANY of them is drawn (their
    /// `Visibility` is the composed far-clip + distance-fade + portal-cull verdict).
    pub(crate) meshes: Vec<Entity>,
    /// The placement's **draw-set gate** — the draw-set answer for a MESHLESS host (a
    /// particles-only model like the InstancePortal swirl or Stratholme's burning-building fire:
    /// 0 render batches, so no submesh carries a `Visibility` verdict). Not a fade sphere any
    /// more but the whole [`crate::particles::EmitterFade`], **the same value the placement's
    /// emitters, ribbons and glow lights were built with** (`terrain_stream::spawn::emitter_fade`,
    /// one expression at one call site), so every rider of a placement freezes and resumes
    /// together.
    ///
    /// It used to be `(radius, world_center)`, and [`gate_doodad_anim`] rebuilt the rest as
    /// `instance: None, room: None` on the reasoning that a particles-only model is not a
    /// building's prop. Stratholme's fire is 88 particles-only props of `stratholme_b.wmo`, and
    /// with no instance the exterior-window term asks `ExteriorGate::admits_sphere` — which,
    /// standing in a sealed room, is `Windows([])` and admits **nothing**. Every meshless prop of
    /// the building the camera stood in was parked.
    pub(crate) fade: crate::particles::EmitterFade,
    /// The looping first-sequence graph node + its duration (secs); `None` on the gseq-only tier.
    pub(crate) clip: Option<(AnimationNodeIndex, f32)>,
    /// `Time::elapsed_secs` at the current **arm** — the player's clock origin: the variation
    /// re-arms every play-window, and a resume must seek to `(now − armed_at) mod duration` or it
    /// lands at the phase of a clip this host stopped playing several windows ago. (The
    /// [`GlobalSeqDrive`] needs no origin at all — global sequences sample the shared world clock
    /// directly, decision 0855.)
    pub(crate) armed_at: f32,
    /// When the armed play-window ends, on the shared clock — the reference's `windowHi`
    /// (`[model+0xac]`), rewritten by every arm. `now ≥ window_hi` ⇒ re-roll. A host is born with
    /// this at `NEG_INFINITY` so the first frame performs the holder's `variationIdx = -1` arm.
    pub(crate) window_hi: f32,
    /// The animation id to re-roll over ([`AnimHostSpawn::anim_id`]); `None` on the gseq-only tier,
    /// which has no arm and therefore no window.
    pub(crate) anim_id: Option<u16>,
    /// Whether the host was ticking last frame (edge-triggered pause/resume).
    pub active: bool,
    /// `Time::elapsed_secs` when `active` last flipped false — the reaper's age key (decision
    /// 0863): under table pressure the longest-parked rigged hosts demote first. Never read
    /// while `active`.
    pub(crate) parked_at: f32,
}

impl DoodadAnimHost {
    /// Where the **armed clip's own clock** stands right now: `(node, seek)`, the seek being
    /// `(now − armed_at) mod duration`. `None` when nothing is armed (the gseq-only tier without a
    /// sound arm — free-running channels have no window at all).
    ///
    /// The **shared clock, never the `AnimationPlayer`**, because it is the one answer both arms
    /// can give: a clock-only host has no player to read at all, and a rigged one's resume seeks
    /// the player to exactly this value, so the two never disagree while the host is drawn.
    ///
    /// It is **not** a claim that the cycle runs while the host is parked. *Linkage* means spliced
    /// into the per-frame scene worklist `[CM2Scene+0x20]`, which **is** the drawn/faded set — "a
    /// doodad culled out of
    /// the drain stops advancing and resumes on re-link". Consumers that must honour that read
    /// [`DoodadAnimHost::active`] beside this clock; the placed-doodad sound scanner
    /// (`benilla_app::doodad_events`) does, and decision 2059 is what it cost not to.
    pub fn arm_clock(&self, now: f32) -> Option<(AnimationNodeIndex, f32)> {
        let (node, duration) = self.clip?;
        // A zero-length clip has no phase to compute; it sits at its own t = 0.
        let seek = if duration > 0.0 {
            (now - self.armed_at).rem_euclid(duration)
        } else {
            0.0
        };
        Some((node, seek))
    }
}

/// The doodad's self-sustaining re-arm: when the
/// armed play-window ends, roll a fresh frequency-weighted variation of the same animation id, snap
/// to it, and write the next window. This is the whole of bug B63's residual — the Blasted Lands
/// lightning keys its entire burst in a 5.0 %-weighted variation (`frequency` 1638 of 32767), so
/// with a once-at-load pick ~1.5 of the Tainted Scar's 31 placements strobed from a *fixed* spot
/// every 1.3 s for ever, while the reference re-rolls all 31 every window and the strike wanders.
///
/// Two details that are easy to get wrong, both byte-pinned:
/// - **The re-arm is a snap, not a blend** (`blendFlag = 0` at `0x6951c8`) — hence `stop_all` before
///   the play rather than a cross-fade.
/// - **This system runs over ALL hosts and never consults [`DoodadAnimHost::active`] — a
///   DELIBERATE divergence, not a reading of the bytes.** The reference's per-frame walk covers
///   exactly the models spliced into `[CM2Scene+0x20]`, and the doodad drain splices only what
///   survived frustum + occlusion + the radius-tiered fade cutoff — so in the real client a doodad
///   behind you *stops* advancing and, on re-link, finds `now ≥ windowHi` and re-arms at once
///   (`0x7074b0`/`0x683f80`). Gating this here would therefore be
///   the faithful shape, and it is left ungated on purpose: it would freeze the whole field while
///   you looked away and re-roll all 31 of the Tainted Scar's placements on the frame you turned
///   back, and that burst is an approved *look* to weigh with the director rather than a silent
///   change (decision 2059 names it as the open follow-up). The sound lane is gated; this one is
///   not, and the difference is recorded rather than smoothed over.
///
/// A host that is not currently drawn still re-rolls; it just updates [`DoodadAnimHost::clip`] and
/// leaves the (stopped) player alone, so [`gate_doodad_anim`]'s resume arms whatever the latest
/// window rolled.
fn reroll_doodad_variation(
    time: Res<Time>,
    mut rng: ResMut<benilla_assets::AnimRng>,
    mut hosts: Query<(
        &mut DoodadAnimHost,
        &ModelAnimations,
        Option<&mut AnimationPlayer>,
    )>,
) {
    let now = time.elapsed_secs();
    for (mut host, anims, player) in &mut hosts {
        let Some(anim_id) = host.anim_id else {
            continue; // gseq-only: free-clock loops, never armed
        };
        if now < host.window_hi {
            continue;
        }
        let Some(clip) = anims.pick_variation(anim_id, rng.draw()) else {
            // No chain for this id (a model whose sequence set changed under us): stop asking.
            host.anim_id = None;
            continue;
        };
        let (node, duration) = (clip.node, clip.duration);
        let replay = rng.replay_count(clip.replay);
        host.armed_at = now;
        // `span · R` — for `replay = (0, 0)` that is exactly one loop, so the roll repeats every
        // pass. A zero-duration clip would make this a busy loop, so it costs one window minimum.
        host.window_hi = now + (duration * replay as f32).max(f32::EPSILON);
        host.clip = Some((node, duration));
        if host.active {
            if let Some(mut p) = player {
                p.stop_all(); // the snap
                p.play(node).repeat();
            }
        }
    }
}

/// The draw gate: pause a doodad's animation when none of its submeshes is drawn, resume — seeking
/// both clocks to the shared-clock position — when one is again. Runs before [`AnimationSystems`] so
/// a resume's seek lands the same frame. Steady state (nothing flipped) is one `Visibility` read per
/// mesh, no writes.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn gate_doodad_anim(
    time: Res<Time>,
    mut hosts: Query<(
        Entity,
        &mut DoodadAnimHost,
        Option<&lazy::LazyRig>,
        Option<&RigPose>,
        Has<crate::rig_palette::RigSkin>,
        Option<&mut AnimationPlayer>,
        Option<&mut GlobalSeqDrive>,
    )>,
    vis: Query<&Visibility>,
    cam: Query<
        (
            Ref<GlobalTransform>,
            &bevy::camera::primitives::Frustum,
            Ref<bevy::camera::Projection>,
            // The seat's own write, visible THIS frame: `GlobalTransform` only changes after
            // propagation, which runs after this system — so on a teleport frame the global
            // reads still while the camera has already moved. Reading the local too is what
            // keeps the verdict reuse below from carrying a pre-snap verdict across a snap
            // (the over-bright lamppost glow after a .tele, 2026-09-04).
            Option<Ref<Transform>>,
        ),
        With<crate::view::WorldCamera>,
    >,
    // A meshed host's verdict is its meshes' `Visibility`; when none moved this frame and the
    // camera stood still, every host's verdict is last frame's (decision 1979's floor: ~1.5 k
    // hosts × their submesh lookups on every still frame).
    changed_vis: Query<(), (Changed<Visibility>, With<crate::model_render::ModelPart>)>,
    // The frame's draw-set inputs, as ONE bundle: the far-clip wall, the exterior-window gate
    // (a meshless prop OUTSIDE, seen from a WMO interior, is not in the frame's worklist and must
    // not tick its bones either), the room the camera stands in, and the portal instances
    // a prop's own rooms resolve through (0689/1289). The same `SystemParam` the particle and
    // ribbon sims read, so the three riders of one placement cannot answer the draw-set question
    // differently (2059).
    scene: crate::particles::sim::SceneGates,
    // The lazy-rig lane's wake half ([`lazy`]): a drawn host without a slot
    // promotes here — allocation, row seed, part swap.
    mut palettes: ResMut<crate::rig_palette::RigPalettes>,
    ibps: Res<Assets<SkinnedMeshInverseBindposes>>,
    worlds: Query<&GlobalTransform>,
    mut twin_parts: lazy::TwinParts,
    mut commands: Commands,
    mut logged: Local<bool>,
) {
    // One breadcrumb per session, the first frame any host exists — the machine-readable "doodads
    // are animating" signal (the live count is in the debug panel).
    if !*logged && !hosts.is_empty() {
        *logged = true;
        info!("doodad anim: first host armed (decision 0130 phase 1)");
    }
    let now = time.elapsed_secs();
    let world_cam = cam.single().ok();
    let (farclip, exterior_gate, camera_instance) =
        scene.scene(world_cam.as_ref().map(|(tf, _, proj, _)| (&**tf, &**proj)));
    let verdicts_still = world_cam.as_ref().is_some_and(|(tf, _, proj, local)| {
        !tf.is_changed() && !proj.is_changed() && !local.as_ref().is_some_and(|l| l.is_changed())
    }) && !scene.changed()
        && changed_vis.is_empty();
    for (entity, mut host, lazy, pose, has_rig, player, drive) in &mut hosts {
        // A host born this frame has no verdict to reuse (`active` starts false): a meshless one
        // spawned under a parked camera stayed parked until the camera moved (review
        // 2026-09-04). Read off the host's own tick — a second query on the component would
        // conflict with this one's `&mut`.
        let drawn = if verdicts_still && !host.is_added() {
            host.active
        } else if host.meshes.is_empty() {
            // Meshless (particles-only) host: the emitters' own draw-set law (see the `fade`
            // field doc) — the reference ticks animation for any model in the draw set, and a
            // 0-batch model is admitted on its fade sphere exactly like its emitters are.
            // "Exactly like" is literal: this asks the placement's OWN `EmitterFade` — the very
            // value its emitters and ribbons were handed — through `in_draw_set`, the single
            // spelling of the rule. A second copy of it is how the far-clip term went missing
            // here as well as in the emitters, and how a particles-only WMO prop lost
            // its building identity and was culled from inside its own room (2059).
            let fade = &host.fade;
            world_cam.as_ref().is_some_and(|(cam_tf, frustum, _, _)| {
                fade.in_draw_set(
                    cam_tf.translation(),
                    Vec3::from(cam_tf.forward()),
                    farclip,
                    frustum.intersects_sphere(
                        &bevy::camera::primitives::Sphere {
                            center: fade.center.into(),
                            radius: fade.radius,
                        },
                        false,
                    ),
                    fade.exterior_admitted(&exterior_gate, camera_instance),
                    scene.room_admits(fade),
                )
            })
        } else {
            host.meshes
                .iter()
                .any(|&e| vis.get(e).is_ok_and(|v| *v != Visibility::Hidden))
        };
        // The lazy-rig promote ([`lazy`]): a host DRAWN two frames running (`&&
        // host.active` — last frame's verdict) holding a [`lazy::LazyRig`] and no slot yet — its
        // first wake, or a denial being retried — claims its palette slot now. Checked every
        // frame it stays drawn, not just on the flip edge, so a momentarily full table is a
        // short delay at bind pose, never the permanent statue this lane was built to kill.
        //
        // Two frames, not one, because the first frame is a LIE: a spawned part's `Visibility`
        // defaults to `Inherited` (a `Mesh3d` required component) and the fade/cull authorities
        // ran before the spawner's commands applied — so on its first frame every far doodad in
        // the streamed tile reads "drawn". Promoting on that frame re-created the eager design
        // one frame late (measured on the 0863 verification leg: ~1500 parked hosts holding
        // slots, the pre-fix census's own number). The second frame's verdict is the
        // authorities' real one; a genuinely visible host promotes one frame late, at fade-in
        // distance, invisibly.
        if drawn && host.active && !has_rig {
            if let Some(lazy) = lazy {
                lazy::promote_lazy_rig(
                    &mut commands,
                    &mut palettes,
                    &ibps,
                    &worlds,
                    entity,
                    lazy,
                    pose,
                    &mut twin_parts,
                );
            }
        }
        if drawn == host.active {
            continue;
        }
        host.active = drawn;
        // The rig-machinery half of the park: the marker gates the 0712
        // evaluator, the model compose, and the world finalize — so a hidden host's collapsed
        // rig costs nothing at all, where the joint hierarchy kept paying propagation. The
        // commands apply before `AnimationSystems` (the gate orders `.before` it), so a resume's
        // re-arm below evaluates the same frame the marker drops — 0739's wake law.
        if drawn {
            commands.entity(entity).remove::<AnimParked>();
        } else {
            host.parked_at = now; // the reaper's age key (decision 0863)
            commands.entity(entity).insert(AnimParked);
        }
        if let Some(mut p) = player {
            if drawn {
                if let Some((node, duration)) = host.clip {
                    let anim = p.start(node);
                    anim.repeat();
                    if duration > 0.0 {
                        // Phase from the current ARM, not from spawn: the variation re-arms every
                        // play-window, so `spawned_at` names a clip this host may have stopped
                        // playing many windows ago.
                        anim.seek_to((now - host.armed_at).rem_euclid(duration));
                    }
                }
            } else {
                // Stop (not pause): a paused active animation is still sampled and applied every
                // frame by `animate_targets`; stopping empties the player so a hidden doodad costs
                // nothing. The resume above re-arms + seeks, which is the faithful clock semantics.
                p.stop_all();
            }
        }
        if let Some(mut d) = drive {
            // No re-seek on resume: the drive samples the shared world clock, so
            // a re-appearing doodad lands on the phase the clock dictates by construction.
            d.set_paused(!drawn);
        }
    }
}

pub struct DoodadAnimPlugin;

impl Plugin for DoodadAnimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UvAnimMaterials>();
        app.init_resource::<TintAnimMaterials>();
        // The client's ONE `rand()` stream (`benilla_assets::AnimRng`), seeded here because this
        // is where the engine boots and where `deterministic_run` is knowable — the reference's
        // own `srand(GetTickCount())` runs once per process from CRT static init, pre-`WinMain`
        // (`0x5d1c70`). A capture keeps the
        // CRT's pre-`srand` value, so golden frames stay reproducible.
        app.init_resource::<benilla_assets::AnimRng>();
        let deterministic = crate::dev_state::deterministic_run();
        app.world_mut()
            .resource_mut::<benilla_assets::AnimRng>()
            .seed_for_session(deterministic);
        // The re-roll runs BEFORE the draw gate: a window that expires this frame must arm its new
        // clip before the gate decides what to resume, or a host re-appearing on the same frame
        // resumes the previous window's variation for one frame.
        app.add_systems(
            PostUpdate,
            (reroll_doodad_variation, gate_doodad_anim)
                .chain()
                .before(AnimationSystems),
        );
        // The lazy-rig pressure reaper: one headroom read per frame when the
        // table has room; demotes longest-parked rigged hosts when it doesn't.
        app.add_systems(Update, lazy::reap_parked_rigs);
        // `sample_mat_anim` runs before the visibility authority (`ModelVisSet`): it composes
        // `MatAnim::current` into the render-alpha tag the same frame. The material tick runs
        // *after* it, because its draw gate reads the verdict that authority writes — one frame
        // later would freeze a scroll for a frame on every doodad that comes into view.
        app.add_systems(
            Update,
            (
                sample_mat_anim.before(crate::model_render::ModelVisSet),
                tick_anim_materials.after(crate::model_render::ModelVisSet),
                // The readout of everything that lane just decided (`WOW_MATANIM_PROBE`), after
                // it, so the `row` column is this frame's write and not the previous one's.
                mat_anim::matanim_probe.after(tick_anim_materials),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::{GlobalBone, GlobalSeqChannel};

    fn clip(anim_id: u16) -> AnimClip {
        AnimClip {
            anim_id,
            seq_index: 0,
            node: AnimationNodeIndex::new(1),
            looping: true,
            duration: 2.0,
            move_speed: 0.0,
            blend_time: 0.0,
            bounds_center: Vec3::ZERO,
            bounds_radius: 0.0,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            events: Vec::new().into(),
            arm_nodes: None,
            upper_node: None,
            frequency: 0,
            replay: (0, 0),
            poses_bones: true,
        }
    }

    fn anims(clips: Vec<AnimClip>, first_seq: Option<usize>, gseq: bool) -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips,
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            hand_close: [None, None],
            global_bones: if gseq {
                vec![GlobalBone {
                    bone: 1,
                    translation: None,
                    rotation: None,
                    scale: Some(GlobalSeqChannel {
                        period: 1.167,
                        keys: vec![(0.0, Vec3::ONE), (0.5, Vec3::splat(1.2))],
                    }),
                }]
            } else {
                Vec::new()
            },
            first_seq,
            pose: Default::default(),
        }
    }

    /// One batch's per-sequence alpha, as the bake emits it: slot 0 hidden, slot 1 visible.
    fn two_seq_alpha() -> std::sync::Arc<benilla_formats::AlphaAnim> {
        let hidden = benilla_formats::ScalarAnim {
            period: 0.0,
            step: true,
            wrap: true, // period 0: a constant has no clock
            gseq: false,
            keys: vec![(0.0, 0.0)],
        };
        std::sync::Arc::new(
            benilla_formats::AlphaAnim::new(vec![
                benilla_formats::AlphaSeq {
                    color: None,
                    weight: Some(hidden),
                },
                benilla_formats::AlphaSeq::default(),
            ])
            .expect("a hiding sequence is worth carrying"),
        )
    }

    fn seq_clip(anim_id: u16, seq_index: usize, node: usize) -> AnimClip {
        AnimClip {
            seq_index,
            node: AnimationNodeIndex::new(node),
            ..clip(anim_id)
        }
    }

    /// The unit lane's resolution: a part reads the sequence its HOST is playing, and follows it
    /// when the host changes animation. This is the plumbing B16/B20 turn on — with the old
    /// single-sequence bake there was nothing to follow, so a voidwalker's death-only armour drew
    /// in every animation.
    #[test]
    fn hosted_mat_anim_follows_the_host_sequence() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, sample_mat_anim);

        // Two clips: graph node 1 is file sequence 0 (which hides the batch), node 2 is slot 1.
        let anims = anims(vec![seq_clip(0, 0, 1), seq_clip(1, 1, 2)], Some(0), false);
        let mut player = AnimationPlayer::default();
        player.play(AnimationNodeIndex::new(1)).repeat();
        let host = app.world_mut().spawn((player, anims)).id();
        let part = app
            .world_mut()
            .spawn(MatAnim::following(two_seq_alpha(), host))
            .id();

        app.update();
        assert_eq!(
            app.world().entity(part).get::<MatAnim>().unwrap().current,
            0.0,
            "playing sequence 0 ⇒ the batch is culled"
        );

        // Switch the host to the other sequence: the same part must now draw.
        let world = app.world_mut();
        let mut entity = world.entity_mut(host);
        {
            let mut p = entity.get_mut::<AnimationPlayer>().unwrap();
            p.stop_all();
            p.play(AnimationNodeIndex::new(2)).repeat();
        }
        app.update();
        assert_eq!(
            app.world().entity(part).get::<MatAnim>().unwrap().current,
            1.0,
            "playing sequence 1 ⇒ the batch draws"
        );
    }

    /// A hosted part whose host has no `AnimationPlayer` yet (the frame before the rig arms, a
    /// rest-pose GameObject) still resolves — to slot 0 at t=0, the sequence's opening pose. It
    /// must not read as fully visible by default, or the batch flashes for a frame at spawn.
    #[test]
    fn hosted_mat_anim_without_a_player_reads_the_first_sequence() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_systems(Update, sample_mat_anim);
        let host = app.world_mut().spawn(Transform::default()).id();
        let part = app
            .world_mut()
            .spawn(MatAnim::following(two_seq_alpha(), host))
            .id();
        app.update();
        assert_eq!(
            app.world().entity(part).get::<MatAnim>().unwrap().current,
            0.0
        );
    }

    /// The pinned lanes are unchanged: a doodad/effect instance reads the slot it was built with,
    /// on its own spawn clock, and never consults a host.
    #[test]
    fn pinned_mat_anim_reads_its_own_slot() {
        let a = two_seq_alpha();
        let doodad = MatAnim::new(a.clone(), 0.0, false);
        assert_eq!(doodad.current, 0.0, "slot 0 — the one-time load arm");
        assert!(!doodad.composes_unit_tag());
        let effect = MatAnim::driving_tag(a, 0.0, Some(1));
        assert_eq!(effect.current, 1.0, "the sequence the fx rig armed");
        assert!(effect.drives_tag);
        assert!(!effect.composes_unit_tag());
    }

    fn skeleton(joints: usize) -> ModelSkeleton {
        ModelSkeleton {
            joints: (0..joints)
                .map(|_| benilla_assets::ModelJoint {
                    parent: -1,
                    local_translation: Vec3::ZERO,
                    billboard: None,
                    parent_arm: None,
                })
                .collect(),
            spine_bone: None,
            head_bone: None,
        }
    }

    /// The content gate: no animations / no joints / a motionless first sequence ⇒ static (today's
    /// path); gseq channels alone ⇒ the player-less tier; a moving first sequence ⇒ the looping arm.
    #[test]
    fn classify_picks_the_measured_tiers() {
        // No ModelAnimations at all (a barrel): static.
        assert!(matches!(
            classify(&skeleton(3), None),
            DoodadAnimTier::Static
        ));
        // Boneless model: static even with parsed animations (the 0035 out-of-bounds guard).
        let a = anims(vec![clip(0)], Some(0), true);
        assert!(matches!(
            classify(&skeleton(0), Some(&a)),
            DoodadAnimTier::Static
        ));
        // A motionless first sequence, no gseq (a posed tree): static — no skin, no player.
        let a = anims(vec![clip(0)], None, false);
        assert!(matches!(
            classify(&skeleton(3), Some(&a)),
            DoodadAnimTier::Static
        ));
        // Gseq only (a candelabra glow pulse): the player-less tier.
        let a = anims(Vec::new(), None, true);
        assert!(matches!(
            classify(&skeleton(3), Some(&a)),
            DoodadAnimTier::GlobalSeqOnly
        ));
        // A moving first sequence (a flag / the torch flame bone): loop its clip.
        let a = anims(vec![clip(0)], Some(0), true);
        assert!(matches!(
            classify(&skeleton(3), Some(&a)),
            DoodadAnimTier::FirstSeq(c) if c.anim_id == 0
        ));
        // A CLOCK-ONLY model: every sequence now builds a clip, so a model whose
        // bones never move — the Molten Core rune, the flame ring, 807 corpus models whose
        // animation lives entirely in their emitter tracks — arrives here with clips and NO
        // `first_seq` (the content gate declined it). It must still classify Static: its clock is
        // armed by the entity lane off `idle_clip`, and rigging it could only reproduce the
        // bind-pose mesh. This is the case that used to reach `first_seq: None` by never having
        // built a clip at all.
        let a = anims(vec![clip(0)], None, false);
        assert!(matches!(
            classify(&skeleton(3), Some(&a)),
            DoodadAnimTier::Static
        ));
        // ...and one that also pulses on a global sequence takes the player-less tier, not a rig.
        let a = anims(vec![clip(0)], None, true);
        assert!(matches!(
            classify(&skeleton(3), Some(&a)),
            DoodadAnimTier::GlobalSeqOnly
        ));
    }

    /// The lightning's own chain, read off the bytes (`benilla-extract m2seq` on
    /// `World\Generic\PassiveDoodads\ParticleEmitters\BlastedLandsLightningbolt01.M2`): two
    /// variations of animation id 0, weights 31129 / 1638 — so slot 1, which is where all four
    /// emitters key their entire burst, carries exactly 5.0 %.
    fn lightning_anims() -> ModelAnimations {
        let mut a = anims(vec![seq_clip(0, 0, 1), seq_clip(0, 1, 2)], Some(0), false);
        a.clips[0].frequency = 31129;
        a.clips[0].duration = 1.333;
        a.clips[1].frequency = 1638;
        a.clips[1].duration = 1.300;
        a
    }

    fn reroll_app() -> App {
        // Deliberately NOT MinimalPlugins: `TimePlugin` drives `Time` off the real clock, which
        // would clobber the manual advance these tests need to step whole play-windows.
        let mut app = App::new();
        app.init_resource::<Time>();
        app.init_resource::<benilla_assets::AnimRng>();
        app.add_systems(Update, reroll_doodad_variation);
        app
    }

    fn lightning_host(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                DoodadAnimHost {
                    meshes: Vec::new(),
                    fade: crate::particles::EmitterFade::sphere(1.0, Vec3::ZERO),
                    clip: None,
                    armed_at: 0.0,
                    window_hi: f32::NEG_INFINITY,
                    anim_id: Some(0),
                    active: true,
                    parked_at: 0.0,
                },
                lightning_anims(),
                AnimationPlayer::default(),
            ))
            .id()
    }

    /// **Bug B63's residual, and the whole of decision 0768.** A placed doodad re-rolls its
    /// frequency-weighted variation every play-window, for ever — it does NOT hold the one it
    /// picked at load.
    ///
    /// Under the superseded contract the pick was once-at-load *and* seeded off the placement's
    /// world position, so a Tainted Scar bolt that rolled slot 0 was silent for the life of the
    /// session and one that rolled slot 1 strobed from that same fixed spot every 1.3 s for ever.
    /// The reference re-rolls all 31 placements every window, which is why its strikes wander.
    #[test]
    fn a_placed_doodad_rerolls_its_variation_every_play_window() {
        let mut app = reroll_app();
        let host = lightning_host(&mut app);

        // Frame 1: born with an expired window ⇒ the holder's `variationIdx = -1` arm lands at once.
        app.update();
        let armed = |app: &App| {
            app.world()
                .entity(host)
                .get::<DoodadAnimHost>()
                .unwrap()
                .clip
        };
        assert!(armed(&app).is_some(), "the first frame arms");

        // Step whole windows and record which variation each one landed on.
        let mut seen = std::collections::HashMap::new();
        for _ in 0..400 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(1400));
            app.update();
            let h = app.world().entity(host).get::<DoodadAnimHost>().unwrap();
            let (node, duration) = h.clip.expect("a window always leaves something armed");
            // R = 1 for `replay = (0, 0)`, so the window is exactly the armed clip's own length.
            assert!(
                (h.window_hi - h.armed_at - duration).abs() < 1e-3,
                "window {} != clip duration {duration}",
                h.window_hi - h.armed_at
            );
            *seen.entry(node).or_insert(0u32) += 1;
        }

        let slot0 = seen.get(&AnimationNodeIndex::new(1)).copied().unwrap_or(0);
        let slot1 = seen.get(&AnimationNodeIndex::new(2)).copied().unwrap_or(0);
        assert_eq!(slot0 + slot1, 400, "every window arms one of the two");
        // The bug was a placement stuck on ONE variation for ever. Both must occur.
        assert!(
            slot1 > 0,
            "the 5 % strike variation never came up in 400 windows — the bolt is stuck again"
        );
        assert!(slot0 > 0, "the 95 % silent variation never came up");
        // A sanity band, not a distribution test — 400 windows off one fixed seed is far too few
        // for that (it lands on 10, ~2.3σ under the mean, unremarkable). The weighting itself was
        // measured separately over 200 000 draws: **4.9665 %** taken at the real
        // two-draws-per-window stride, against the authored 1638/32768 = 4.9988 %. MSVC's LCG shows
        // no stride bias here, which is worth having checked: the variation roll and the replay
        // count come off the same stream, and a strided LCG is exactly where a rare variation could
        // quietly under-fire and leave the field half as active as the reference's.
        assert!(
            (2..=60).contains(&slot1),
            "slot 1 came up {slot1}/400 — far enough off its authored 5 % to suspect the roll"
        );
    }

    /// The gseq-only tier has no arm, so it has no play-window and nothing to re-roll: its channels
    /// are free-clock loops the reference drives with zero arming.
    #[test]
    fn a_gseq_only_host_never_rerolls() {
        let mut app = reroll_app();
        let host = app
            .world_mut()
            .spawn((
                DoodadAnimHost {
                    meshes: Vec::new(),
                    fade: crate::particles::EmitterFade::sphere(1.0, Vec3::ZERO),
                    clip: None,
                    armed_at: 0.0,
                    window_hi: f32::NEG_INFINITY,
                    anim_id: None,
                    active: true,
                    parked_at: 0.0,
                },
                anims(Vec::new(), None, true),
                AnimationPlayer::default(),
            ))
            .id();
        for _ in 0..8 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(1400));
            app.update();
        }
        let h = app.world().entity(host).get::<DoodadAnimHost>().unwrap();
        assert!(h.clip.is_none(), "nothing was ever armed");
        assert_eq!(h.window_hi, f32::NEG_INFINITY, "no window was ever opened");
    }

    /// A host that is not currently drawn still advances its window — the reference gates the cycle
    /// on **linkage** (the in-range doodad drain), not on the draw. Gating on the draw would freeze
    /// the field while the camera looks away and then re-roll every placement on the one frame it
    /// turns back: a burst of simultaneous strikes the reference cannot produce. The undrawn host
    /// keeps its stopped player and just updates what a resume will arm.
    #[test]
    fn an_undrawn_host_keeps_cycling() {
        let mut app = reroll_app();
        let host = lightning_host(&mut app);
        app.world_mut()
            .entity_mut(host)
            .get_mut::<DoodadAnimHost>()
            .unwrap()
            .active = false;

        app.update();
        let first = app
            .world()
            .entity(host)
            .get::<DoodadAnimHost>()
            .unwrap()
            .window_hi;
        for _ in 0..3 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(1400));
            app.update();
        }
        let h = app.world().entity(host).get::<DoodadAnimHost>().unwrap();
        assert!(
            h.window_hi > first,
            "an undrawn host still opened new windows"
        );
        assert!(
            h.clip.is_some(),
            "and still has something for the resume to arm"
        );
        assert_eq!(
            app.world()
                .entity(host)
                .get::<AnimationPlayer>()
                .unwrap()
                .playing_animations()
                .count(),
            0,
            "but its stopped player was left alone"
        );
    }

    /// The draw gate stops the player + pauses the gseq drive when every submesh is hidden, and on
    /// re-appearing re-arms at the shared-clock position (`(now − armed_at) mod duration`) — the
    /// ref's clock-indexed pose, so pause history never desyncs phase.
    #[test]
    fn gate_pauses_hidden_and_resumes_on_the_shared_clock() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default())); // Time + asset stores
                                                                   // The gate reads the far-clip wall — this host is mesh-BACKED, so it takes the
                                                                   // `Visibility` branch and never consults it, but the system still needs the resource.
        app.init_resource::<crate::view::ViewDistance>();
        // …and the exterior-window gate's two terms, for the same reason: this host is
        // mesh-BACKED so it never consults them, but the system's params must resolve. Their
        // defaults are the outdoor case (`Unrestricted`, no room claimed).
        app.init_resource::<crate::wmo_portal::ExteriorWindows>();
        app.init_resource::<crate::wmo_portal::CameraInteriorClaim>();
        // The lazy-rig promote's params — this host carries no `LazyRig`, so
        // the wake edge never fires here, but the params must resolve.
        app.init_resource::<crate::rig_palette::RigPalettes>();
        app.init_asset::<SkinnedMeshInverseBindposes>();
        app.add_systems(Update, gate_doodad_anim);

        let mesh = app.world_mut().spawn(Visibility::Inherited).id();
        let node = AnimationNodeIndex::new(1);
        let mut player = AnimationPlayer::default();
        player.play(node).repeat();
        let joint = app.world_mut().spawn(Transform::default()).id();
        let drive = GlobalSeqDrive::new(
            &anims(Vec::new(), None, true).global_bones,
            &[Entity::PLACEHOLDER, joint],
        )
        .expect("one gseq bone maps");
        let host = app
            .world_mut()
            .spawn((
                DoodadAnimHost {
                    meshes: vec![mesh],
                    fade: crate::particles::EmitterFade::sphere(1.0, Vec3::ZERO),
                    clip: Some((node, 2.0)),
                    armed_at: 0.0,
                    // Far future: this test exercises the DRAW gate, so no window may expire
                    // under it and change the armed clip mid-assertion.
                    window_hi: f32::INFINITY,
                    anim_id: Some(0),
                    active: true,
                    parked_at: 0.0,
                },
                player,
                drive,
            ))
            .id();

        // Visible: stays active, player untouched.
        app.update();
        let playing = |app: &mut App, e: Entity| {
            app.world_mut()
                .entity(e)
                .get::<AnimationPlayer>()
                .unwrap()
                .playing_animations()
                .count()
        };
        assert_eq!(playing(&mut app, host), 1, "drawn ⇒ playing");

        // Hide the one submesh: the player empties (stopped, zero per-frame cost).
        *app.world_mut()
            .entity_mut(mesh)
            .get_mut::<Visibility>()
            .unwrap() = Visibility::Hidden;
        app.update();
        assert_eq!(playing(&mut app, host), 0, "hidden ⇒ stopped");
        assert!(
            !app.world()
                .entity(host)
                .get::<DoodadAnimHost>()
                .unwrap()
                .active
        );

        // Show it again: re-armed, looping, sought to the shared-clock position (< duration).
        *app.world_mut()
            .entity_mut(mesh)
            .get_mut::<Visibility>()
            .unwrap() = Visibility::Inherited;
        app.update();
        assert_eq!(playing(&mut app, host), 1, "re-drawn ⇒ re-armed");
        let world = app.world();
        let p = world.entity(host).get::<AnimationPlayer>().unwrap();
        let active = p.animation(node).expect("the first-seq node is active");
        assert!(
            active.seek_time() >= 0.0 && active.seek_time() < 2.0,
            "seek lands inside the loop: {}",
            active.seek_time()
        );
    }

    /// [`DoodadAnimHost::arm_clock`] — the sound lane's phase, which must be the SHARED clock's,
    /// never the (draw-gated) player's. Three claims: it wraps with the clip, it is unaffected by
    /// the host being parked, and an unarmed host has no phase at all.
    /// **A meshless prop of the building the camera stands in keeps animating** — decision 2059's
    /// wiring, pinned where it broke.
    ///
    /// `EmitterFade`'s own tests already pin the LAW (`particles`:
    /// `a_sealed_room_keeps_its_own_props_burning_and_stops_everything_else`). What was wrong here
    /// was the wiring: this gate built its meshless host a fresh `EmitterFade` with
    /// `instance: None, room: None`, so the exterior-window term fell through to
    /// `ExteriorGate::admits_sphere` — and a sealed room is `Windows([])`, which "admits nothing".
    /// Every particles-only WMO prop of the camera's OWN building was therefore parked, which is
    /// Stratholme's 88 burning-building fires: 0 render batches each, so nothing but this branch
    /// ever judged them, and their `$DSL` never fired.
    ///
    /// The control is the second host: same sphere, same place, no building — an ADT map doodad,
    /// which a sealed room really must stop.
    #[test]
    fn a_sealed_room_keeps_its_own_meshless_props_animating() {
        use crate::wmo_portal::{
            CameraInteriorClaim, ExteriorWindows, InteriorClaim, WmoGroupVis, WmoPortalInstance,
            WmoRoom,
        };
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<crate::view::ViewDistance>();
        app.init_resource::<crate::rig_palette::RigPalettes>();
        app.init_asset::<SkinnedMeshInverseBindposes>();
        app.add_systems(Update, gate_doodad_anim);

        // The camera stands inside a sealed room of `building` — no exterior window at all.
        let building = app
            .world_mut()
            .spawn(WmoPortalInstance {
                handle: Handle::default(),
                world_from_local: bevy::math::Affine3A::IDENTITY,
                name_set: 0,
                visible: vec![true],
                interior_fog: vec![false],
                liquid_visited: vec![false],
                flooded: vec![None],
            })
            .id();
        app.insert_resource(ExteriorWindows::Windows(Vec::new()));
        app.insert_resource(CameraInteriorClaim(Some(InteriorClaim {
            room: WmoRoom {
                instance: building,
                group: 0,
            },
            exterior_visible: false,
        })));

        // Looking straight at both hosts, 20 yd out and well inside the wall.
        let seat =
            Transform::from_xyz(0.0, 0.0, 0.0).looking_at(Vec3::new(0.0, 0.0, -20.0), Vec3::Y);
        let projection = bevy::camera::Projection::from(bevy::camera::PerspectiveProjection {
            far: 5000.0,
            ..default()
        });
        let frustum = bevy::camera::primitives::Frustum::from_clip_from_world(
            &(projection.get_clip_from_view() * GlobalTransform::from(seat).affine().inverse()),
        );
        app.world_mut().spawn((
            crate::view::WorldCamera,
            GlobalTransform::from(seat),
            seat,
            frustum,
            projection,
        ));

        let node = AnimationNodeIndex::new(1);
        let mut meshless = |fade: crate::particles::EmitterFade| {
            app.world_mut()
                .spawn(DoodadAnimHost {
                    meshes: Vec::new(), // 0 render batches — the whole point
                    fade,
                    clip: Some((node, 2.0)),
                    armed_at: 0.0,
                    window_hi: f32::INFINITY,
                    anim_id: Some(0),
                    active: false,
                    parked_at: 0.0,
                })
                .id()
        };
        let center = Vec3::new(0.0, 0.0, -20.0);
        let prop = meshless(crate::particles::EmitterFade {
            instance: Some(building),
            room: Some(WmoGroupVis {
                instance: building,
                groups: [0u16].as_slice().into(),
            }),
            ..crate::particles::EmitterFade::sphere(2.0, center)
        });
        let map_doodad = meshless(crate::particles::EmitterFade::sphere(2.0, center));

        app.update();
        let active = |app: &App, e: Entity| {
            app.world()
                .entity(e)
                .get::<DoodadAnimHost>()
                .unwrap()
                .active
        };
        assert!(
            active(&app, prop),
            "a prop of the camera's OWN building is not exterior to it — it keeps animating, and \
             its $DSL keeps firing"
        );
        assert!(
            !active(&app, map_doodad),
            "the control: a map doodad carries no building, so the sealed room stops it"
        );
    }

    /// The teleport frame (2026-09-04): this system runs before transform propagation, so the
    /// camera's `GlobalTransform` still reads still on the frame its `Transform` was re-seated,
    /// and the verdict reuse carried every host's pre-snap verdict across the snap — a lamppost
    /// glow that should have parked kept its clock, a host that should have woken stayed parked.
    /// The gate must read the seat's own write.
    #[test]
    fn a_reseated_camera_is_not_a_still_scene() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.init_resource::<crate::view::ViewDistance>();
        app.init_resource::<crate::wmo_portal::ExteriorWindows>();
        app.init_resource::<crate::wmo_portal::CameraInteriorClaim>();
        app.init_resource::<crate::rig_palette::RigPalettes>();
        app.init_asset::<SkinnedMeshInverseBindposes>();
        app.add_systems(Update, gate_doodad_anim);

        // A camera 1000 yd from the host, looking away: the host's frustum verdict is "not
        // drawn". No propagation plugin, so `GlobalTransform` never follows `Transform` —
        // exactly the ordering the gate sees on the real snap frame.
        let far =
            Transform::from_xyz(1000.0, 0.0, 0.0).looking_at(Vec3::new(2000.0, 0.0, 0.0), Vec3::Y);
        let projection = bevy::camera::Projection::from(bevy::camera::PerspectiveProjection {
            far: 5000.0,
            ..default()
        });
        let frustum = bevy::camera::primitives::Frustum::from_clip_from_world(
            &(projection.get_clip_from_view() * GlobalTransform::from(far).affine().inverse()),
        );
        let cam = app
            .world_mut()
            .spawn((
                crate::view::WorldCamera,
                GlobalTransform::from(far),
                far,
                frustum,
                projection,
            ))
            .id();
        let node = AnimationNodeIndex::new(1);
        let mut player = AnimationPlayer::default();
        player.play(node).repeat();
        let joint = app.world_mut().spawn(Transform::default()).id();
        let drive = GlobalSeqDrive::new(
            &anims(Vec::new(), None, true).global_bones,
            &[Entity::PLACEHOLDER, joint],
        )
        .expect("one gseq bone maps");
        let host = app
            .world_mut()
            .spawn((
                DoodadAnimHost {
                    meshes: Vec::new(),
                    fade: crate::particles::EmitterFade::sphere(1.0, Vec3::ZERO),
                    clip: Some((node, 2.0)),
                    armed_at: 0.0,
                    window_hi: f32::INFINITY,
                    anim_id: Some(0),
                    active: true,
                    parked_at: 0.0,
                },
                player,
                drive,
            ))
            .id();
        let active = |app: &App| {
            app.world()
                .entity(host)
                .get::<DoodadAnimHost>()
                .unwrap()
                .active
        };
        app.update();
        assert!(!active(&app), "far and facing away ⇒ parked");
        app.update();
        app.update();
        assert!(!active(&app), "still ⇒ the verdict is reused, still parked");

        // The snap: the seat writes the camera's local next to the host, facing it. The global
        // (and the frustum built from it) stay stale this frame — and the host must still be
        // re-judged rather than reused.
        let near = Transform::from_xyz(0.0, 0.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y);
        let near_frustum = bevy::camera::primitives::Frustum::from_clip_from_world(
            &(bevy::camera::Projection::from(bevy::camera::PerspectiveProjection {
                far: 5000.0,
                ..default()
            })
            .get_clip_from_view()
                * GlobalTransform::from(near).affine().inverse()),
        );
        {
            let mut e = app.world_mut().entity_mut(cam);
            *e.get_mut::<Transform>().unwrap() = near;
            *e.get_mut::<bevy::camera::primitives::Frustum>().unwrap() = near_frustum;
            // The propagated frame, written WITHOUT a change tick: on the real snap frame it is
            // last frame's value (propagation has not run), and this is the closest a test can
            // stand to that — the only "moved" signal on this frame is the local.
            *e.get_mut::<GlobalTransform>()
                .unwrap()
                .bypass_change_detection() = GlobalTransform::from(near);
        }
        app.update();
        assert!(active(&app), "the snap frame re-judges the host: drawn");
    }

    #[test]
    fn arm_clock_wraps_on_the_shared_clock() {
        let mut host = DoodadAnimHost {
            meshes: Vec::new(),
            fade: crate::particles::EmitterFade::sphere(0.0, Vec3::ZERO),
            clip: Some((AnimationNodeIndex::new(1), 3.333)),
            armed_at: 10.0,
            window_hi: f32::NEG_INFINITY,
            anim_id: Some(0),
            // PARKED — and the clock still answers, which is the point: `arm_clock` is a pure
            // function of the shared clock, so the phase is defined whether or not the host is in
            // the animate set. Honouring that membership is the CALLER's job.
            active: false,
            parked_at: 0.0,
        };

        let (_, seek) = host.arm_clock(10.0).expect("armed");
        assert!(seek.abs() < 1e-6, "at the arm it sits at t = 0");

        let (_, seek) = host.arm_clock(12.0).expect("armed");
        assert!((seek - 2.0).abs() < 1e-4, "2 s in, {seek}");

        // Past one full cycle it wraps rather than running off the end.
        let (_, seek) = host.arm_clock(14.0).expect("armed");
        assert!((seek - (4.0 - 3.333)).abs() < 1e-4, "wrapped to {seek}");

        // A zero-length clip has no phase to compute and must not divide by it.
        host.clip = Some((AnimationNodeIndex::new(1), 0.0));
        let (_, seek) = host.arm_clock(99.0).expect("armed");
        assert!(
            seek.abs() < 1e-6,
            "a zero-length clip sits at its own t = 0"
        );

        // Nothing armed (the gseq-only tier without a sound arm) ⇒ no phase, so nothing fires.
        host.clip = None;
        assert!(host.arm_clock(12.0).is_none());
    }
}
