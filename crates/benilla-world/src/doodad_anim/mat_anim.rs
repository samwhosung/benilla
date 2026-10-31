//! Animated materials: the per-batch colour-alpha and weight loops ([`MatAnim`]) and the
//! texture-transform and tint registries ([`UvAnimMaterials`], [`TintAnimMaterials`]).

use bevy::prelude::*;

use benilla_assets::ModelAnimations;

/// Per-submesh animated material alpha: the batch's per-sequence colour-alpha and weight loops,
/// sampled into [`Self::current`]. The visibility authority multiplies that into the render-alpha
/// `MeshTag` and hides the batch at 0, the reference's `A ≤ 0` cull (`0x707b3a`–`0x707b5c`).
#[derive(Component)]
pub struct MatAnim {
    anim: std::sync::Arc<benilla_formats::AlphaAnim>,
    /// The entity whose `AnimationPlayer` names the sequence to read, re-resolved every frame (the
    /// unit lane: a voidwalker's upper armour is weight 0 in Stand, 1 in Death). `None` pins the
    /// instance to [`Self::seq`] on its spawn clock: a spell effect, or a placed doodad, which
    /// reads slot 0 where the reference reads the sequence each arm picks (op4 `0x7121a0`, called
    /// from the load build `0x70ebd0` and again on every re-roll).
    host: Option<Entity>,
    /// The file sequence slot to read: fixed when pinned, the last one resolved from
    /// [`Self::host`] otherwise; `None` reads slot 0.
    seq: Option<usize>,
    /// `Time::elapsed_secs` at spawn, the pinned lanes' clock origin; a hosted instance reads its
    /// player's seek time, so its alpha stays in phase with its pose.
    spawned_at: f32,
    /// Captures freeze the clock at 0 for deterministic frames.
    frozen: bool,
    /// The sample drives the render-alpha `MeshTag` field by itself: the spell-effect parts, whose
    /// alpha has no other writer. `false` on the doodad lane, where the visibility authority
    /// composes [`Self::current`] in, and on the unit lane (`entities::apply_unit_mat_alpha`).
    pub(crate) drives_tag: bool,
    /// Composed by the unit lane without a [`Self::host`]: an attach model (a held weapon, a helm),
    /// pinned like a doodad but ordered against the wearer's appear-fade. See [`Self::resting`].
    unit_lane: bool,
    /// The gseq attach anchor (secs, shared clock), stamped by the first [`sample_mat_anim`] pass:
    /// the reference snapshots the scene clock once per instance at attach (`CM2Model+0x68`).
    gseq_attach: Option<f64>,
    /// The last sampled combined factor (colour-alpha × weight), read by the visibility authority.
    pub current: f32,
}

impl MatAnim {
    pub(crate) fn new(
        anim: std::sync::Arc<benilla_formats::AlphaAnim>,
        now: f32,
        frozen: bool,
    ) -> Self {
        // Seed at both clocks' 0: nothing is armed, and the gseq cursor opens at its anchor.
        let current = anim.sample(None, 0.0, 0.0);
        Self {
            anim,
            host: None,
            seq: None,
            spawned_at: now,
            frozen,
            drives_tag: false,
            unit_lane: false,
            gseq_attach: None,
            current,
        }
    }

    /// The spell-effect lane: never frozen, driving the part's tag itself, and pinned to the
    /// sequence its rig plays (`seq`: the missile's InFlight, else the file-order-first clip).
    pub fn driving_tag(
        anim: std::sync::Arc<benilla_formats::AlphaAnim>,
        now: f32,
        seq: Option<usize>,
    ) -> Self {
        let mut m = Self::new(anim, now, false);
        m.drives_tag = true;
        m.seq = seq;
        m.current = m.anim.sample(seq, 0.0, 0.0);
        m
    }

    /// Read the sequence and clock from `host`'s live `AnimationPlayer` each frame, for an effect
    /// that advances Stand, Hold, Decay (`0x5ff170` arms Hold, `0x5ff270` the terminal Decay), each
    /// leg with its own alpha loops. It keeps [`Self::drives_tag`]; `None` leaves it pinned.
    pub fn following_host(mut self, host: Option<Entity>) -> Self {
        self.host = host;
        self
    }

    /// The unit lane: sequence and clock come from `host`'s live `AnimationPlayer`, and the tag is
    /// composed by `entities::apply_unit_mat_alpha`, not driven here.
    pub fn following(anim: std::sync::Arc<benilla_formats::AlphaAnim>, host: Entity) -> Self {
        Self {
            host: Some(host),
            ..Self::new(anim, 0.0, false)
        }
    }

    /// The attach-model lane (a held weapon, a helm): pinned to the file's first sequence, where a
    /// rig-less item model rests, and composed by the unit lane.
    pub fn resting(anim: std::sync::Arc<benilla_formats::AlphaAnim>) -> Self {
        Self {
            unit_lane: true,
            ..Self::new(anim, 0.0, false)
        }
    }

    /// The entity whose player this instance follows, `None` when pinned.
    pub fn host(&self) -> Option<Entity> {
        self.host
    }

    /// Whether the unit lane composes this instance's tag: a hosted or attach-model batch, never an
    /// effect part that drives its own.
    pub fn composes_unit_tag(&self) -> bool {
        (self.host.is_some() || self.unit_lane) && !self.drives_tag
    }
}

/// The sequence slot and clip time a host plays: its base animation with the greatest weight, as
/// masked overlays pose bones without reselecting the sequence. In a cross-fade the heavier clip
/// wins, where the reference blends the two samples by λ (`0x71af20`). With nothing armed it is
/// the loader-idle sequence at t = 0 ([`ModelAnimations::idle_seq`]): the reference arms that clip
/// on every M2 at load, and slot 0 can be another sequence (a GameObject's Spawn).
pub fn playing_seq(player: &AnimationPlayer, anims: &ModelAnimations) -> Option<(usize, f32)> {
    player
        .playing_animations()
        .filter_map(|(node, active)| {
            let clip = anims.clip_at(*node)?;
            Some((clip.seq_index, active.seek_time(), active.weight()))
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(seq, t, _)| (seq, t))
        .or_else(|| anims.idle_seq().map(|seq| (seq, 0.0)))
}

/// What a hosted alpha reads off its host: the rig that names its sequence and clip time, and the
/// scene clock its global sequences run on, either absent.
type AlphaHost = (
    Option<(&'static AnimationPlayer, &'static ModelAnimations)>,
    Option<&'static crate::rig_anim::SceneClock>,
);

/// Sample every instance: a hosted one on its host's sequence and clip clock, and its global
/// sequences on the host's scene clock when it stands in one ([`crate::rig_anim::SceneClock`]);
/// a pinned one on its spawn clock; frozen ones keep their t = 0 sample. Hidden instances sample too, as the reference
/// evaluates the tracks every frame regardless of the cull (`0x707b3a`–`0x707b5c`), or a batch
/// born at weight 0 would stay hidden. Runs before the visibility authority.
pub fn sample_mat_anim(time: Res<Time>, hosts: Query<AlphaHost>, mut q: Query<&mut MatAnim>) {
    let now = time.elapsed_secs();
    // The world clock in f64: a long-uptime f32 drifts whole milliseconds.
    let world = time.elapsed_secs_f64();
    for mut m in &mut q {
        if m.frozen {
            continue;
        }
        // A host with no player yet keeps its last slot at t = 0, a bind-posed model's pose.
        let (rig, clock) = m
            .host
            .and_then(|h| hosts.get(h).ok())
            .unwrap_or((None, None));
        let played = rig.and_then(|(p, a)| playing_seq(p, a));
        let (seq, elapsed) = match played {
            Some((seq, t)) => {
                m.seq = Some(seq);
                (Some(seq), t)
            }
            None if m.host.is_some() => (m.seq, 0.0),
            None => (m.seq, now - m.spawned_at),
        };
        // The gseq cursor is sceneNow − attach, the anchor stamped on the first pass, on the
        // host's own scene clock when it has one.
        let shared = clock.map_or(world, |c| c.0);
        let attach = *m.gseq_attach.get_or_insert(shared);
        m.current = m.anim.sample(seq, elapsed, shared - attach);
    }
}

/// The scan marker: a part whose material can be a [`UvAnimMaterials`] or [`TintAnimMaterials`]
/// key, inserted at spawn beside the registration, so the draw scan visits only animated parts.
#[derive(bevy::prelude::Component)]
pub struct AnimMatPart;

/// Which loop a registered UV material animates on. A material-keyed row serves every instance of
/// the batch, so a batch whose sequence slots bake different loops takes a material per placement
/// (keyed by its anim host in [`crate::model_render::MatKey`]) and follows that host's slot.
pub enum UvLoop {
    /// Every slot bakes the same loop: the shared material on the free-running scene clock. `None`
    /// for a transform with no translation, whose translation row stays at zero.
    ///
    /// Deviation: every instance scrolls in one phase, where the reference phases a band loop per
    /// arm and a gseq loop from each instance's attach (`CM2Model+0x68`), because one uniform
    /// serves the material and a seamless scroll looks the same at any phase.
    Shared(Option<std::sync::Arc<benilla_formats::UvAnim>>),
    /// Either stage's slots disagree: this material belongs to one placement, on whichever slot
    /// `host` plays. Stage 0 reads `seqs` by slot, or its one loop `single` when only stage 1's
    /// slots disagree.
    PerSeq {
        seqs: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
        single: Option<std::sync::Arc<benilla_formats::UvAnim>>,
        host: Entity,
    },
    /// The spell-effect lane: one effect instance's own clone, scrolling on that instance's clip
    /// (`host`'s player, as [`MatAnim::following_host`] reads it), because an effect's scroll is
    /// its animation: `SwipeCaster.m2`'s UVs lie outside a clamped sheet until the scroll drags
    /// the claw in. `seqs` when the slots disagree, `single` otherwise; both are absent when only
    /// stage 1 moves.
    Instance {
        seqs: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
        single: Option<std::sync::Arc<benilla_formats::UvAnim>>,
        host: Entity,
        /// `Time::elapsed_secs` at attach: the band clock when the model arms no player, and the
        /// gseq anchor (`sceneNow − attach`, a fresh instance per play) always.
        attached_at: f32,
    },
}

/// One registered UV material (the texture-transform translation `0x716216` gates): its loop, its
/// table slot, and the built `sun_scale.zw` seed its delta rows are measured from, never mutated.
pub struct UvAnimEntry {
    pub anim: UvLoop,
    pub slot: u16,
    pub seed: [f32; 2],
    /// The rotation and scaling half ([`UvAffine`]), `None` when the transform only translates.
    affine: Option<UvAffine>,
    /// The entity whose [`crate::rig_anim::SceneClock`] this entry runs on, `None` for the
    /// world's clock.
    clock: Option<Entity>,
    /// Stage 1's own rows ([`Stage1Rows`]), `None` when its transform does not move.
    stage1: Option<Stage1Rows>,
}

/// A second texture stage's own transform on its entry's lane and clocks: the reference reads a
/// transform per stage, the lookup at `texture_transform_combo_index + k` (`0x70b897`), and gives
/// each stage its own matrix (`0x70b91f`). Its rows are `stage1_uv.x` and `.y`, as `anim_slots.x`
/// and `.z` are stage 0's.
struct Stage1Rows {
    seqs: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    single: Option<std::sync::Arc<benilla_formats::UvAnim>>,
    /// The translation row's slot, and the built `stage1_uv.zw` seed it is measured from.
    slot: u16,
    seed: [f32; 2],
    /// The rotation and scaling row, `None` when the stage only translates.
    affine: Option<UvAffine>,
}

/// A texture transform's rotation and scaling, which the translation row cannot carry: a second
/// table row (`crate::mat_anim_table::affine_row`'s encoding) on the translation's clocks.
struct UvAffine {
    rot: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>>>,
    scale: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    /// The affine row's slot (`anim_slots.z`); the row is a delta from the identity, not a seed.
    slot: u16,
}

impl UvAnimEntry {
    /// The slot's delta row for `now`: the quantized sample minus the built seed, which the shader
    /// adds back onto `sun_scale.zw`. `playing` is the host's `(slot, clip time)`; a gone host or a
    /// slot with no loop samples a zero offset.
    pub(crate) fn delta(&self, now: f32, gseq_now: f64, playing: Option<(usize, f32)>) -> [f32; 4] {
        let uv = match &self.anim {
            UvLoop::Shared(anim) => anim.as_ref().map_or([0.0, 0.0], |a| a.sample(now)),
            UvLoop::PerSeq { seqs, single, .. } => playing
                .and_then(|(seq, band_t)| {
                    seqs.as_ref()
                        .map_or(single.as_deref(), |s| s.seq(Some(seq)))
                        .map(|l| l.sample(l.clock(band_t, gseq_now)))
                })
                .unwrap_or([0.0, 0.0]),
            // The effect lane's clocks are the instance's: the band clock is its clip time (its
            // age while the rig has no player), and the gseq cursor runs from its attach.
            UvLoop::Instance {
                seqs,
                single,
                attached_at,
                ..
            } => {
                let age = now - attached_at;
                let band_t = playing.map_or(age, |(_, t)| t);
                // The playing slot's own loop where the slots differ, so a dead slot holds
                // still; the slot-0 loop only where they do not.
                seqs.as_ref()
                    .map_or(single.as_deref(), |s| s.seq(playing.map(|(seq, _)| seq)))
                    .map_or([0.0, 0.0], |l| l.sample(l.clock(band_t, f64::from(age))))
            }
        };
        [
            benilla_assets::quantize(uv[0], 4096.0) - self.seed[0],
            benilla_assets::quantize(uv[1], 4096.0) - self.seed[1],
            0.0,
            0.0,
        ]
    }

    /// The band clock and gseq cursor this entry's lane reads at `now`, for every channel of
    /// both stages, so all of them read one instant.
    fn clocks(&self, now: f32, gseq_now: f64, playing: Option<(usize, f32)>) -> (f32, f64) {
        match &self.anim {
            UvLoop::Shared(_) => (now, gseq_now),
            UvLoop::PerSeq { .. } => (playing.map_or(now, |(_, t)| t), gseq_now),
            UvLoop::Instance { attached_at, .. } => {
                let age = now - attached_at;
                (playing.map_or(age, |(_, t)| t), f64::from(age))
            }
        }
    }

    /// This entry's affine slot and row, sampled on the clocks [`Self::delta`] uses so all three
    /// channels of one transform read one instant; `None` when the transform only translates.
    pub(crate) fn affine(
        &self,
        now: f32,
        gseq_now: f64,
        playing: Option<(usize, f32)>,
    ) -> Option<(u16, [f32; 4])> {
        let a = self.affine.as_ref()?;
        let (band_t, gseq) = self.clocks(now, gseq_now, playing);
        Some(a.row(playing.map(|(seq, _)| seq), band_t, gseq))
    }

    /// Stage 1's translation slot and delta row, then its affine slot and row, on this entry's
    /// lane and clocks; empty when stage 1 does not move. A per-placement lane whose host plays
    /// nothing holds the zero offset, as [`Self::delta`] does.
    pub(crate) fn stage1_rows(
        &self,
        now: f32,
        gseq_now: f64,
        playing: Option<(usize, f32)>,
    ) -> Vec<(u16, [f32; 4])> {
        let Some(st) = self.stage1.as_ref() else {
            return Vec::new();
        };
        let (band_t, gseq) = self.clocks(now, gseq_now, playing);
        let seq = playing.map(|(seq, _)| seq);
        let held = matches!(self.anim, UvLoop::PerSeq { .. }) && playing.is_none();
        // The playing slot's own loop where the slots differ, so a dead slot holds still as
        // stage 0's per-placement lane does; the slot-0 loop only where they do not.
        let uv = st
            .seqs
            .as_ref()
            .map_or(st.single.as_deref(), |s| s.seq(seq))
            .filter(|_| !held)
            .map_or([0.0, 0.0], |l| l.sample(l.clock(band_t, gseq)));
        let mut rows = vec![(
            st.slot,
            [
                benilla_assets::quantize(uv[0], 4096.0) - st.seed[0],
                benilla_assets::quantize(uv[1], 4096.0) - st.seed[1],
                0.0,
                0.0,
            ],
        )];
        rows.extend(st.affine.as_ref().map(|a| a.row(seq, band_t, gseq)));
        rows
    }
}

impl UvAffine {
    /// The affine slot and row at slot `seq`'s loops on the given clocks.
    fn row(&self, seq: Option<usize>, band_t: f32, gseq: f64) -> (u16, [f32; 4]) {
        let q = self
            .rot
            .as_ref()
            .and_then(|r| r.seq(seq))
            .map_or([0.0, 0.0, 0.0, 1.0], |l| l.sample(l.clock(band_t, gseq)));
        let s = self
            .scale
            .as_ref()
            .and_then(|r| r.seq(seq))
            .map_or([1.0, 1.0], |l| l.sample(l.clock(band_t, gseq)));
        (self.slot, crate::mat_anim_table::affine_row(q, s))
    }
}

impl UvLoop {
    /// Move a per-sequence or per-instance lane to `host`'s player; a shared one has none.
    fn set_host(&mut self, to: Entity) {
        if let UvLoop::PerSeq { host, .. } | UvLoop::Instance { host, .. } = self {
            *host = to;
        }
    }

    /// This lane for a twin of the material on another instance: the same loops, a per-sequence
    /// or per-instance one following `host`'s player.
    fn for_twin(&self, host: Entity) -> Self {
        match self {
            UvLoop::Shared(a) => UvLoop::Shared(a.clone()),
            UvLoop::PerSeq { seqs, single, .. } => UvLoop::PerSeq {
                seqs: seqs.clone(),
                single: single.clone(),
                host,
            },
            UvLoop::Instance {
                seqs,
                single,
                attached_at,
                ..
            } => UvLoop::Instance {
                seqs: seqs.clone(),
                single: single.clone(),
                host,
                attached_at: *attached_at,
            },
        }
    }
}

impl TintLoop {
    /// As [`UvLoop::set_host`].
    fn set_host(&mut self, to: Entity) {
        if let TintLoop::PerSeq { host, .. } = self {
            *host = to;
        }
    }

    /// As [`UvLoop`]'s: the same loop, a per-sequence one following `host`.
    fn for_twin(&self, host: Entity) -> Self {
        match self {
            TintLoop::Shared(a) => TintLoop::Shared(a.clone()),
            TintLoop::PerSeq { seqs, .. } => TintLoop::PerSeq {
                seqs: seqs.clone(),
                host,
            },
        }
    }
}

impl UvAnimEntry {
    /// This frame's rows, slot and value, on `scene`, the time of the scene clock the entry runs
    /// on, else on the world's `now`: stage 0's translation row first, then its affine row and
    /// stage 1's rows. What the tick writes.
    fn rows(
        &self,
        scene: Option<f64>,
        now: f32,
        gseq_now: f64,
        playing: Option<(usize, f32)>,
    ) -> Vec<(u16, [f32; 4])> {
        let (now, gseq_now) = scene.map_or((now, gseq_now), |t| (t as f32, t));
        let mut rows = vec![(self.slot, self.delta(now, gseq_now, playing))];
        rows.extend(self.affine(now, gseq_now, playing));
        rows.extend(self.stage1_rows(now, gseq_now, playing));
        rows
    }

    /// Every table slot this entry holds.
    fn slots(&self) -> Vec<u16> {
        let stage1 = self.stage1.as_ref();
        [
            Some(self.slot),
            self.affine.as_ref().map(|a| a.slot),
            stage1.map(|s| s.slot),
            stage1.and_then(|s| s.affine.as_ref()).map(|a| a.slot),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn host(&self) -> Option<Entity> {
        match &self.anim {
            UvLoop::Shared(_) => None,
            UvLoop::PerSeq { host, .. } | UvLoop::Instance { host, .. } => Some(*host),
        }
    }

    /// The spell-effect lane, sampled even in a capture: its clock is its instance's player.
    fn instance_lane(&self) -> bool {
        matches!(self.anim, UvLoop::Instance { .. })
    }

    /// The loop this entry samples now, resolved as [`Self::delta`] does, for [`matanim_probe`].
    fn resolved(&self, playing: Option<(usize, f32)>) -> Option<&benilla_formats::UvAnim> {
        match &self.anim {
            UvLoop::Shared(a) => a.as_deref(),
            UvLoop::PerSeq { seqs, single, .. } | UvLoop::Instance { seqs, single, .. } => seqs
                .as_ref()
                .map_or(single.as_deref(), |s| s.seq(playing.map(|(seq, _)| seq))),
        }
    }

    /// A phase sweep's `now`: the effect lane counts from attach, the others on the scene clock.
    fn phase_now(&self, t: f32) -> f32 {
        match &self.anim {
            UvLoop::Instance { attached_at, .. } => attached_at + t,
            UvLoop::Shared(_) | UvLoop::PerSeq { .. } => t,
        }
    }
}

impl TintAnimEntry {
    /// This frame's row on `scene`'s time, else the world's, as [`UvAnimEntry`]'s rows.
    fn row(
        &self,
        scene: Option<f64>,
        now: f32,
        gseq_now: f64,
        playing: Option<(usize, f32)>,
    ) -> [f32; 4] {
        let (now, gseq_now) = scene.map_or((now, gseq_now), |t| (t as f32, t));
        self.delta(now, gseq_now, playing)
    }

    fn host(&self) -> Option<Entity> {
        match &self.anim {
            TintLoop::Shared(_) => None,
            TintLoop::PerSeq { host, .. } => Some(*host),
        }
    }
}

/// The anim hosts a per-placement entry reads its sequence from, for [`playing_seq`].
pub type SeqHosts<'w, 's> = Query<'w, 's, (&'static AnimationPlayer, &'static ModelAnimations)>;

fn host_seq(hosts: &SeqHosts, host: Option<Entity>) -> Option<(usize, f32)> {
    let (player, anims) = hosts.get(host?).ok()?;
    playing_seq(player, anims)
}

#[derive(Resource, Default)]
pub struct UvAnimMaterials(
    pub  std::collections::HashMap<
        bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
        UvAnimEntry,
    >,
);

/// Give `twin`, a booth's relit clone of the registered material `source`, rows of its own for the
/// model `root`: a copy of each of the source's lanes on `root`'s scene clock, a per-sequence one
/// following `root`'s player. The source's rows run on the world's clock and are written only
/// while the world draws it; a scene that owns its clock animates its models on that clock alone.
/// An already registered twin moves to `root`, clock and player both. Returns whether the twin has
/// a row, which its part's [`AnimMatPart`] marker must agree with.
pub fn clocked_twin(
    uv: &mut UvAnimMaterials,
    tint: &mut TintAnimMaterials,
    table: &mut crate::mat_anim_table::MatAnimTable,
    materials: &mut bevy::asset::Assets<benilla_assets::materials::WowModelMaterial>,
    source: bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
    twin: bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
    root: Entity,
) -> bool {
    if source == twin {
        return false; // no twin was made: the source's rows are the world's
    }
    if let Some(src) = uv.0.get(&source) {
        let lane = src.anim.for_twin(root);
        let affine = src
            .affine
            .as_ref()
            .map(|a| (a.rot.clone(), a.scale.clone()));
        let stage1 = src.stage1.as_ref().map(|s| Stage1Loops {
            seqs: s.seqs.clone(),
            single: s.single.clone(),
            rot: s.affine.as_ref().and_then(|a| a.rot.clone()),
            scale: s.affine.as_ref().and_then(|a| a.scale.clone()),
        });
        if !uv.0.contains_key(&twin) {
            register_uv(uv, table, materials, twin, lane);
            if let Some((rot, scale)) = affine {
                attach_affine(uv, table, materials, twin, rot, scale);
            }
            if let Some(stage1) = stage1 {
                attach_stage1(uv, table, materials, twin, &stage1);
            }
        }
    }
    if let Some(src) = tint.0.get(&source) {
        let lane = src.anim.for_twin(root);
        register_tint(tint, table, materials, twin, lane);
    }
    // A twin registered for an earlier model follows this one.
    if let Some(e) = uv.0.get_mut(&twin) {
        e.anim.set_host(root);
    }
    if let Some(e) = tint.0.get_mut(&twin) {
        e.anim.set_host(root);
    }
    scene_clocked(uv, tint, twin, root);
    uv.0.contains_key(&twin) || tint.0.contains_key(&twin)
}

/// Put material `id`'s registered UV and tint entries on the clock of the scene `clock` stands in
/// ([`crate::rig_anim::SceneClock`] on that entity): a scene with its own clock calls it after
/// registering each of its batches.
pub fn scene_clocked(
    uv: &mut UvAnimMaterials,
    tint: &mut TintAnimMaterials,
    id: bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
    clock: Entity,
) {
    if let Some(e) = uv.0.get_mut(&id) {
        e.clock = Some(clock);
    }
    if let Some(e) = tint.0.get_mut(&id) {
        e.clock = Some(clock);
    }
}

/// Register material `id` for the UV scroll: allocate its table slot, record the built seed and
/// write the slot into `anim_slots.x`, the one material write this lane makes. A full table skips
/// registration, leaving the batch at its seed.
pub(crate) fn register_uv(
    reg: &mut UvAnimMaterials,
    table: &mut crate::mat_anim_table::MatAnimTable,
    materials: &mut bevy::asset::Assets<benilla_assets::materials::WowModelMaterial>,
    id: bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
    anim: UvLoop,
) {
    if reg.0.contains_key(&id) {
        return;
    }
    let Some(slot) = table.alloc() else {
        bevy::log::warn_once!("mat-anim table full — a UV-scroll batch stays at its seed");
        return;
    };
    // Through the parked half, not `Assets::get_mut`: `model_render::lazy` parks a just-built
    // material outside the store until something visible binds it.
    let Some(seed) = crate::model_render::lazy::with_material_mut(materials, id, |mat| {
        let seed = [mat.extension.sun_scale.z, mat.extension.sun_scale.w];
        mat.extension.anim_slots.x = f32::from(slot);
        seed
    }) else {
        // Neither half holds it: a dead or foreign handle, warned so a dead lane is never silent.
        bevy::log::warn_once!(
            "mat-anim: no material behind {id} — a UV-scroll batch stays at its seed"
        );
        table.free(slot);
        return;
    };
    match &anim {
        UvLoop::PerSeq { host, .. } => {
            bevy::log::debug!("mat-anim: per-placement UV lane armed for host {host} (slot {slot})")
        }
        UvLoop::Instance { host, .. } => {
            bevy::log::debug!(
                "mat-anim: per-instance UV lane armed for effect {host} (slot {slot})"
            )
        }
        UvLoop::Shared(_) => {}
    }
    reg.0.insert(
        id,
        UvAnimEntry {
            anim,
            slot,
            seed,
            affine: None,
            clock: None,
            stage1: None,
        },
    );
}

/// Attach the rotation and scaling half, if any, to the entry [`register_uv`] just made: a second
/// row, its slot in `anim_slots.z`, encoded `[cos − 1, sin, sx − 1, sy − 1]` so the zero row is
/// the identity. A rotate- or scale-only transform still owns a translation row.
fn attach_affine(
    reg: &mut UvAnimMaterials,
    table: &mut crate::mat_anim_table::MatAnimTable,
    materials: &mut bevy::asset::Assets<benilla_assets::materials::WowModelMaterial>,
    id: bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
    rot: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>>>,
    scale: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
) {
    if rot.is_none() && scale.is_none() {
        return;
    }
    let Some(entry) = reg.0.get_mut(&id) else {
        return;
    };
    match table.alloc() {
        Some(slot) => {
            crate::model_render::lazy::with_material_mut(materials, id, |mat| {
                mat.extension.anim_slots.z = f32::from(slot);
            });
            entry.affine = Some(UvAffine { rot, scale, slot });
        }
        None => {
            bevy::log::warn_once!("mat-anim table full — a UV rotation/scale stays at the identity")
        }
    }
}

/// Attach stage 1's own rows, if it moves, to the entry [`register_uv`] made for `id`: a
/// translation row measured from the built `stage1_uv.zw` seed, its slot in `stage1_uv.x`, and
/// for a stage that turns or scales an affine row in `stage1_uv.y`.
fn attach_stage1(
    reg: &mut UvAnimMaterials,
    table: &mut crate::mat_anim_table::MatAnimTable,
    materials: &mut bevy::asset::Assets<benilla_assets::materials::WowModelMaterial>,
    id: bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
    loops: &Stage1Loops,
) {
    if !loops.animates() {
        return;
    }
    let Some(entry) = reg.0.get_mut(&id) else {
        return;
    };
    if entry.stage1.is_some() {
        return;
    }
    let Some(slot) = table.alloc() else {
        bevy::log::warn_once!("mat-anim table full — a second stage stays at its seed");
        return;
    };
    let Some(seed) = crate::model_render::lazy::with_material_mut(materials, id, |mat| {
        mat.extension.stage1_uv.x = f32::from(slot);
        [mat.extension.stage1_uv.z, mat.extension.stage1_uv.w]
    }) else {
        table.free(slot);
        return;
    };
    let affine = if loops.rot.is_some() || loops.scale.is_some() {
        match table.alloc() {
            Some(a) => {
                crate::model_render::lazy::with_material_mut(materials, id, |mat| {
                    mat.extension.stage1_uv.y = f32::from(a);
                });
                Some(UvAffine {
                    rot: loops.rot.clone(),
                    scale: loops.scale.clone(),
                    slot: a,
                })
            }
            None => {
                bevy::log::warn_once!(
                    "mat-anim table full — a second stage's rotation/scale stays at the identity"
                );
                None
            }
        }
    } else {
        None
    };
    entry.stage1 = Some(Stage1Rows {
        seqs: loops.seqs.clone(),
        single: loops.single.clone(),
        slot,
        seed,
        affine,
    });
}

/// Put one spell-effect material clone on the per-instance UV lane, whole: the [`UvLoop`] effect
/// variant, [`register_uv`] and both table rows. `host` is the effect root whose player names the
/// clip, `attached_at` its `Time::elapsed_secs` origin, and the clone's `sun_scale.zw` the seed.
pub fn register_fx_uv(
    reg: &mut UvAnimMaterials,
    table: &mut crate::mat_anim_table::MatAnimTable,
    materials: &mut bevy::asset::Assets<benilla_assets::materials::WowModelMaterial>,
    id: bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
    loops: UvLoops,
    host: Entity,
    attached_at: f32,
) {
    let UvLoops {
        seqs,
        single,
        rot,
        scale,
        stage1,
    } = loops;
    let turns = rot.is_some() || scale.is_some();
    register_uv(
        reg,
        table,
        materials,
        id,
        UvLoop::Instance {
            seqs,
            single,
            host,
            attached_at,
        },
    );
    attach_affine(reg, table, materials, id, rot, scale);
    attach_stage1(reg, table, materials, id, &stage1);
    let Some(entry) = reg.0.get(&id) else {
        return;
    };
    let _ = turns;
    // Every row written now: an instance attached after this frame's tick would otherwise draw
    // its first frame on zero rows, the identity.
    for (slot, row) in entry.rows(None, attached_at, 0.0, None) {
        table.set(slot, row);
    }
}

/// Put one unit, GameObject or held-item batch material on the UV lane: [`UvLoop::PerSeq`] on the
/// caller's own clone when there is a `host` and either stage's slots disagree, else
/// [`UvLoop::Shared`] on the deduped material. A period-0 translation registers only if the batch rotates or scales or
/// its stage 1 moves, and then samples its constant, the seed, so its row stays at zero.
pub fn register_entity_uv(
    reg: &mut UvAnimMaterials,
    table: &mut crate::mat_anim_table::MatAnimTable,
    materials: &mut bevy::asset::Assets<benilla_assets::materials::WowModelMaterial>,
    id: bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
    loops: &UvLoops,
    host: Option<Entity>,
) {
    if !loops.animates() {
        return;
    }
    register_uv(reg, table, materials, id, loops.lane(host));
    attach_affine(
        reg,
        table,
        materials,
        id,
        loops.rot.clone(),
        loops.scale.clone(),
    );
    attach_stage1(reg, table, materials, id, &loops.stage1);
}

/// The four baked channels of one batch's texture transform: the translation as a per-slot set or
/// one loop, and the per-slot rotation and scaling sets; and its stage 1's own four.
#[derive(Default)]
pub struct UvLoops {
    pub seqs: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    pub single: Option<std::sync::Arc<benilla_formats::UvAnim>>,
    pub rot: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>>>,
    pub scale: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    /// Stage 1's channels ([`benilla_formats::TextureStage::uv_anim`] and beside it).
    pub stage1: Stage1Loops,
}

/// A second texture stage's four transform channels, as [`UvLoops`]' own are stage 0's.
#[derive(Default, Clone)]
pub struct Stage1Loops {
    pub seqs: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    pub single: Option<std::sync::Arc<benilla_formats::UvAnim>>,
    pub rot: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>>>,
    pub scale: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
}

impl Stage1Loops {
    /// The channels of `geometry`'s stage 1, empty without one or for a generated one.
    pub fn of(geometry: &benilla_formats::RenderSubmesh) -> Self {
        geometry
            .stage1
            .as_ref()
            .map_or_else(Self::default, |s| Self {
                seqs: s.uv_seq.clone(),
                single: s.uv_anim.clone(),
                rot: s.uv_rot_seq.clone(),
                scale: s.uv_scale_seq.clone(),
            })
    }

    /// As [`UvLoops::any`].
    pub fn any(&self) -> bool {
        self.seqs.is_some() || self.single.is_some() || self.rot.is_some() || self.scale.is_some()
    }

    /// As [`UvLoops::animates`].
    pub fn animates(&self) -> bool {
        self.seqs.is_some()
            || self.rot.is_some()
            || self.scale.is_some()
            || self.single.as_ref().is_some_and(|l| l.period > 0.0)
    }

    /// The offset stage 1 opens on, to seed a clone's `stage1_uv.zw` at the loop's `t = 0`.
    pub fn open_offset(&self) -> [f32; 2] {
        self.seqs
            .as_ref()
            .and_then(|s| s.seq(None))
            .or(self.single.as_deref())
            .map_or([0.0, 0.0], |l| l.sample(0.0))
    }
}

impl UvLoops {
    /// The one place the channels are read off a submesh, so the registration and the spawn
    /// marker cannot disagree about which batches animate.
    pub fn of(sub: &benilla_assets::ModelSubmesh) -> Self {
        Self {
            seqs: sub.uv_seq.clone(),
            single: sub.uv_anim.clone(),
            rot: sub.uv_rot_seq.clone(),
            scale: sub.uv_scale_seq.clone(),
            stage1: Stage1Loops::of(&sub.geometry),
        }
    }

    /// These loops without either stage's rotation and scaling, the placed-doodad lane's: no
    /// placed doodad turns or scales a stage's coordinates.
    pub fn translation_only(self) -> Self {
        Self {
            rot: None,
            scale: None,
            stage1: Stage1Loops {
                rot: None,
                scale: None,
                ..self.stage1
            },
            ..self
        }
    }

    /// The lane these loops register on: per placement when there is a `host` and either stage's
    /// slots disagree, else shared.
    fn lane(&self, host: Option<Entity>) -> UvLoop {
        let per_seq = self.seqs.is_some() || self.stage1.seqs.is_some();
        match host.filter(|_| per_seq) {
            Some(host) => UvLoop::PerSeq {
                seqs: self.seqs.clone(),
                single: self.single.clone(),
                host,
            },
            None => UvLoop::Shared(self.single.clone()),
        }
    }

    /// Whether any channel of either stage is present, the effect attach's test before cloning a
    /// material: a scale-only transform or a dead slot 0 leaves `single` empty.
    pub fn any(&self) -> bool {
        self.seqs.is_some()
            || self.single.is_some()
            || self.rot.is_some()
            || self.scale.is_some()
            || self.stage1.any()
    }

    /// Whether the batch moves: [`Self::any`] minus a period-0 translation, which is its seed. The
    /// one predicate the entity lane's registration and its [`AnimMatPart`] marker share.
    pub fn animates(&self) -> bool {
        self.seqs.is_some()
            || self.rot.is_some()
            || self.scale.is_some()
            || self.single.as_ref().is_some_and(|l| l.period > 0.0)
            || self.stage1.animates()
    }

    /// The offset the batch opens on, to seed a clone's `sun_scale.zw` at the loop's `t = 0`.
    pub fn open_offset(&self) -> [f32; 2] {
        self.seqs
            .as_ref()
            .and_then(|s| s.seq(None))
            .or(self.single.as_deref())
            .map_or([0.0, 0.0], |l| l.sample(0.0))
    }
}

/// Re-sample the drawn animated materials into the shared table, UV scroll and tint together:
/// sampling is clock-indexed, so a re-appearing one has nothing to catch up. Drawn is
/// `Visibility != Hidden` after `model_render::ModelVisSet`. Captures skip this, except the
/// spell-effect lane ([`UvAnimEntry::instance_lane`]).
pub(super) fn tick_anim_materials(
    time: Res<Time>,
    clocks: Query<&crate::rig_anim::SceneClock>,
    real: Res<Time<bevy::time::Real>>,
    mut uv_reg: ResMut<UvAnimMaterials>,
    mut tint_reg: ResMut<TintAnimMaterials>,
    materials: Res<Assets<benilla_assets::materials::WowModelMaterial>>,
    mut table: ResMut<crate::mat_anim_table::MatAnimTable>,
    parts: Query<
        (
            &MeshMaterial3d<benilla_assets::materials::WowModelMaterial>,
            &Visibility,
        ),
        With<AnimMatPart>,
    >,
    twins: Res<crate::model_render::FarSideTwins>,
    hosts: SeqHosts,
    mut drawn: Local<
        bevy::platform::collections::HashSet<AssetId<benilla_assets::materials::WowModelMaterial>>,
    >,
) {
    if uv_reg.0.is_empty() && tint_reg.0.is_empty() {
        // Marked parts beside an empty registry mean this lane is dead, not that nothing animates.
        if parts.iter().next().is_some() {
            bevy::log::warn_once!(
                "mat-anim: parts are marked animated but NOTHING is registered — the UV/tint lane is dead"
            );
        }
        return;
    }
    if matanim_off(&real) {
        return;
    }
    // A capture freezes the shared-clock lanes at their seed but keeps the effect lane running.
    let frozen = crate::dev_state::deterministic_run() && !matanim_live();
    if frozen && !uv_reg.0.values().any(UvAnimEntry::instance_lane) {
        return;
    }
    drawn.clear();
    // Frozen, only the effect lane samples, and it skips the draw gate: no scan is needed.
    for (mat, vis) in parts.iter().filter(|_| !frozen) {
        if *vis != Visibility::Hidden {
            let id = mat.id();
            // A far-classified part carries its far twin's id, never a registry key: count it as
            // the near one, or a batch drawn only beyond the water plane would freeze.
            drawn.insert(twins.near_of(id).unwrap_or(id));
        }
    }
    let now = time.elapsed_secs();
    // Rows are deltas from each entry's seed, so no material changes per frame and its far twin
    // shares the row; quantized (1/4096, 1/255 for tint) so an unchanged row is not re-uploaded.
    let gseq_now = f64::from(now);
    uv_reg.0.retain(|id, entry| {
        // Alive means either half holds it: a material parked by `model_render::lazy` is not in
        // the store, and registration happens once, at spawn.
        if !crate::model_render::lazy::holds(&materials, *id) {
            for slot in entry.slots() {
                table.free(slot);
            }
            return false;
        }
        // The effect lane skips the draw gate: its parts carry no `AnimMatPart`, and an effect
        // lives about a second, so an undrawn one costs little.
        let fx = entry.instance_lane();
        if (fx || drawn.contains(id)) && (!frozen || fx) {
            let scene = entry.clock.and_then(|e| clocks.get(e).ok()).map(|c| c.0);
            for (slot, row) in entry.rows(scene, now, gseq_now, host_seq(&hosts, entry.host())) {
                table.set(slot, row);
            }
        }
        true
    });
    tint_reg.0.retain(|id, entry| {
        if !crate::model_render::lazy::holds(&materials, *id) {
            table.free(entry.slot);
            return false;
        }
        if drawn.contains(id) && !frozen {
            let scene = entry.clock.and_then(|e| clocks.get(e).ok()).map(|c| c.0);
            table.set(
                entry.slot,
                entry.row(scene, now, gseq_now, host_seq(&hosts, entry.host())),
            );
        }
        true
    });
}

/// `WOW_MATANIM_DUTY=<start_s>:<period_s>` turns [`tick_anim_materials`] off and on every `period`
/// seconds from `start`; the ON/OFF difference over the square wave is its per-frame cost. On
/// `Time<Real>` because virtual time clamps to `max_delta` (250 ms) and lags on a hitching run.
fn matanim_off(time: &Time<bevy::time::Real>) -> bool {
    static SPEC: std::sync::OnceLock<Option<(f32, f32)>> = std::sync::OnceLock::new();
    let spec = SPEC.get_or_init(|| {
        let v = std::env::var("WOW_MATANIM_DUTY").ok()?;
        let (start, period) = v.split_once(':')?;
        Some((start.trim().parse().ok()?, period.trim().parse().ok()?))
    });
    spec.is_some_and(|(start, period)| {
        let t = time.elapsed_secs() - start;
        t >= 0.0 && period > 0.0 && ((t / period) as u32) % 2 == 1
    })
}

/// `WOW_MATANIM_LIVE=1` runs the shared lanes inside a deterministic capture too, so `fxview` can
/// show a unit's or GameObject's texture transform. Off by default; no golden scenario sets it.
fn matanim_live() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_MATANIM_LIVE").is_ok_and(|v| v != "0"))
}

/// Which loop a registered tint material (an animated M2 colour track) animates on, [`UvLoop`]'s
/// RGB twin: `Shared`, with the same phase deviation as [`UvLoop::Shared`], or `PerSeq` on one
/// placement's slot. Spell-effect instances tick their own tint clones and never register here.
pub enum TintLoop {
    Shared(std::sync::Arc<benilla_formats::RgbAnim>),
    PerSeq {
        seqs: std::sync::Arc<benilla_formats::SeqLoops<[f32; 3]>>,
        host: Entity,
    },
}

/// One registered tint material, seeded from the built `tint.xyz`.
pub struct TintAnimEntry {
    pub anim: TintLoop,
    pub slot: u16,
    pub seed: [f32; 3],
    /// As [`UvAnimEntry`]'s: the entity whose scene clock it runs on, `None` for the world's.
    clock: Option<Entity>,
}

impl TintAnimEntry {
    /// The tint's delta row, quantized at the display's 1/255 step. An unresolved host reads white,
    /// the multiplier's identity, so the batch is untinted rather than black.
    pub(crate) fn delta(&self, now: f32, gseq_now: f64, playing: Option<(usize, f32)>) -> [f32; 4] {
        let rgb = match &self.anim {
            TintLoop::Shared(anim) => anim.sample(now),
            TintLoop::PerSeq { seqs, .. } => playing
                .and_then(|(seq, band_t)| {
                    seqs.seq(Some(seq))
                        .map(|l| l.sample(l.clock(band_t, gseq_now)))
                })
                .unwrap_or([1.0, 1.0, 1.0]),
        };
        let rgb = benilla_assets::quant255(rgb);
        [
            rgb[0] - self.seed[0],
            rgb[1] - self.seed[1],
            rgb[2] - self.seed[2],
            0.0,
        ]
    }
}

#[derive(Resource, Default)]
pub struct TintAnimMaterials(
    pub  std::collections::HashMap<
        bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
        TintAnimEntry,
    >,
);

/// Register a tint material: its slot goes in `anim_slots.y`, its seed is the built `tint.xyz`.
pub fn register_tint(
    reg: &mut TintAnimMaterials,
    table: &mut crate::mat_anim_table::MatAnimTable,
    materials: &mut bevy::asset::Assets<benilla_assets::materials::WowModelMaterial>,
    id: bevy::asset::AssetId<benilla_assets::materials::WowModelMaterial>,
    anim: TintLoop,
) {
    if reg.0.contains_key(&id) {
        return;
    }
    let Some(slot) = table.alloc() else {
        bevy::log::warn_once!("mat-anim table full — a tint batch stays at its seed");
        return;
    };
    // Through the parked half, as in `register_uv`.
    let Some(seed) = crate::model_render::lazy::with_material_mut(materials, id, |mat| {
        let seed = [
            mat.extension.tint.x,
            mat.extension.tint.y,
            mat.extension.tint.z,
        ];
        mat.extension.anim_slots.y = f32::from(slot);
        seed
    }) else {
        bevy::log::warn_once!("mat-anim: no material behind {id} — a tint batch stays at its seed");
        table.free(slot);
        return;
    };
    reg.0.insert(
        id,
        TintAnimEntry {
            anim,
            slot,
            seed,
            clock: None,
        },
    );
}

/// `WOW_MATANIM_PROBE=<secs>`: once, `secs` after boot, log one line per [`AnimMatPart`] with what
/// decides whether it moves on screen: registered or not, its loop's four quarter-phase samples,
/// its live table row, and its draw verdict. It samples the loop itself, so it works in a
/// deterministic capture. A trailing `(unmarked)` block lists the registry entries no marked part
/// accounts for: the spell-effect lane, and any leak.
pub(super) fn matanim_probe(
    time: Res<Time>,
    uv_reg: Res<UvAnimMaterials>,
    tint_reg: Res<TintAnimMaterials>,
    table: Res<crate::mat_anim_table::MatAnimTable>,
    materials: Res<Assets<benilla_assets::materials::WowModelMaterial>>,
    twins: Res<crate::model_render::FarSideTwins>,
    hosts: SeqHosts,
    parts: Query<ProbeReadout, With<AnimMatPart>>,
    mut fired: Local<bool>,
) {
    let Some(at) = probe_at() else { return };
    // Not while the world is empty: a capture holds the clock at 0 while the scene builds.
    if *fired
        || time.elapsed_secs() < at
        || (parts.iter().next().is_none() && uv_reg.0.is_empty() && tint_reg.0.is_empty())
    {
        return;
    }
    *fired = true;
    let now = time.elapsed_secs();
    let gseq_now = f64::from(now);
    let mut rows: Vec<String> = Vec::new();
    let uv_row = |e: &UvAnimEntry| {
        let playing = host_seq(&hosts, e.host());
        // A per-placement entry has a loop per slot, so it prints its play head, not a period.
        let (lane, period) = match &e.anim {
            UvLoop::Shared(a) => {
                // `None`: a rotate- or scale-only transform, whose translation row stays at zero.
                let p = a.as_ref().map_or(0.0, |a| a.period);
                (format!("shared period {p:.3}s"), p)
            }
            UvLoop::PerSeq { .. } => (format!("per-seq playing {playing:?}"), 0.0),
            // The effect lane has a resolved loop, so it sweeps its own clip from attach.
            UvLoop::Instance { attached_at, .. } => (
                format!(
                    "per-instance age {:.3}s playing {playing:?}",
                    now - attached_at
                ),
                e.resolved(playing).map_or(0.0, |l| l.period),
            ),
        };
        // Four quarter-phase samples of the loop. The phase moves the clock the entry reads:
        // `now` on the shared lane, the play head on a hosted one, which ignores `now`.
        let phases: Vec<String> = (0u8..4)
            .map(|i| {
                let t = period * f32::from(i) / 4.0;
                let head = playing.map(|(seq, _)| (seq, t));
                let d = e.delta(e.phase_now(t), f64::from(t), head);
                format!("({:+.4},{:+.4})", d[0], d[1])
            })
            .collect();
        let live = e.delta(now, gseq_now, playing);
        let affine = e.affine(now, gseq_now, playing).map_or(String::new(), |(slot, want)| {
            let row = table.row(slot);
            format!(
                " · affine slot {slot} row ({:+.4},{:+.4},{:+.4},{:+.4}) live ({:+.4},{:+.4},{:+.4},{:+.4})",
                row[0], row[1], row[2], row[3], want[0], want[1], want[2], want[3],
            )
        });
        let stage1: String = e
            .stage1_rows(now, gseq_now, playing)
            .iter()
            .map(|(slot, want)| {
                let row = table.row(*slot);
                format!(
                    " · stage1 slot {slot} row ({:+.4},{:+.4}) live ({:+.4},{:+.4})",
                    row[0], row[1], want[0], want[1],
                )
            })
            .collect();
        format!(
            "uv slot {} seed ({:+.4},{:+.4}) row ({:+.4},{:+.4}) live ({:+.4},{:+.4}){affine}{stage1} {lane} loop {}",
            e.slot,
            e.seed[0],
            e.seed[1],
            table.row(e.slot)[0],
            table.row(e.slot)[1],
            live[0],
            live[1],
            phases.join(" "),
        )
    };
    let mut visited: std::collections::HashSet<
        AssetId<benilla_assets::materials::WowModelMaterial>,
    > = std::collections::HashSet::new();
    for (mat, vis, vv, obj) in &parts {
        // A far part carries its twin's id; the registry is keyed by the near one.
        let far = twins.near_of(mat.id());
        let id = far.unwrap_or(mat.id());
        let label = obj.map_or_else(|| "-".to_string(), |o| format!("{}#{}", o.label, o.id));
        // Through the parked half, or an undrawn material's slots read zero; `parked` says which.
        let parked = !materials.contains(id);
        let slots =
            crate::model_render::lazy::with_material(&materials, id, |m| m.extension.anim_slots)
                .unwrap_or_default();
        visited.insert(id);
        let uv = uv_reg.0.get(&id).map(&uv_row);
        let tint = tint_reg.0.get(&id).map(|e| {
            format!(
                "tint slot {} row ({:+.4},{:+.4},{:+.4})",
                e.slot,
                table.row(e.slot)[0],
                table.row(e.slot)[1],
                table.row(e.slot)[2],
            )
        });
        let channels = match (uv, tint) {
            (None, None) => "UNREGISTERED".to_string(),
            (a, b) => [a, b].into_iter().flatten().collect::<Vec<_>>().join(" · "),
        };
        rows.push(format!(
            "matanim  {label}  mat {id}{}{}  anim_slots ({}, {}, {})  vis {vis:?} drawn {}  {channels}",
            if far.is_some() { " FAR" } else { "" },
            if parked { " PARKED" } else { "" },
            slots.x as u32,
            slots.y as u32,
            slots.z as u32,
            u8::from(vv.is_some_and(|v| v.get())),
        ));
    }
    // Entries no marked part accounts for: the effect lane, which carries no marker, and any leak.
    for (id, e) in &uv_reg.0 {
        if visited.contains(id) {
            continue;
        }
        rows.push(format!("matanim  (unmarked)  mat {id}  {}", uv_row(e)));
    }
    rows.sort();
    bevy::log::info!(
        "matanim probe at {now:.2}s — {} animated part(s), {} uv / {} tint registry entries",
        rows.len(),
        uv_reg.0.len(),
        tint_reg.0.len(),
    );
    for r in rows {
        bevy::log::info!("{r}");
    }
}

/// One animated part as [`matanim_probe`] reads it: its material, both halves of its draw
/// verdict, and the placement that names it.
type ProbeReadout = (
    &'static MeshMaterial3d<benilla_assets::materials::WowModelMaterial>,
    &'static Visibility,
    Option<&'static bevy::camera::visibility::ViewVisibility>,
    Option<&'static crate::interact::WorldObject>,
);

/// `WOW_MATANIM_PROBE`, read once: unset is off, bare or unparseable fires at 20 s.
fn probe_at() -> Option<f32> {
    static AT: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
    *AT.get_or_init(|| {
        let v = std::env::var("WOW_MATANIM_PROBE").ok()?;
        Some(v.trim().parse().unwrap_or(20.0))
    })
}

#[cfg(test)]
mod delta_tests {
    use super::*;

    fn fx_entry(attached_at: f32) -> UvAnimEntry {
        UvAnimEntry {
            anim: UvLoop::Instance {
                seqs: None,
                single: Some(uv_loop()),
                host: Entity::from_raw_u32(11).expect("a valid test entity"),
                attached_at,
            },
            slot: 9,
            seed: [0.0, 0.0],
            affine: None,
            clock: None,
            stage1: None,
        }
    }

    /// A rotation-only loop: a quarter turn about Z over two seconds, in file slot 0 only.
    fn quarter_turn() -> std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>> {
        const Q: f32 = std::f32::consts::FRAC_1_SQRT_2;
        const QUARTER: [f32; 4] = [0.0, 0.0, Q, Q];
        std::sync::Arc::new(
            benilla_formats::SeqLoops::new(vec![Some(benilla_formats::KeyAnim {
                period: 2.0,
                step: false,
                wrap: true,
                gseq: false,
                // 90° about Z = (0, 0, sin 45°, cos 45°).
                keys: vec![(0.0, [0.0, 0.0, 0.0, 1.0]), (2.0, QUARTER)],
            })])
            .expect("one animating slot"),
        )
    }

    /// `animates()` is not `uv_anim.is_some()`: a rotate-only batch moves, a constant does not.
    #[test]
    fn the_animates_predicate_sees_a_rotation_only_batch() {
        let rot_only = UvLoops {
            rot: Some(quarter_turn()),
            ..Default::default()
        };
        assert!(rot_only.single.is_none(), "the obvious channel is empty");
        assert!(rot_only.animates(), "and it still moves");

        let constant = UvLoops {
            single: Some(std::sync::Arc::new(benilla_formats::UvAnim {
                period: 0.0,
                step: false,
                wrap: false,
                gseq: false,
                keys: vec![(0.0, [0.25, 0.0])],
            })),
            ..Default::default()
        };
        assert!(constant.any(), "it has a channel");
        assert!(
            !constant.animates(),
            "…but a period-0 translation is a seed, never a per-frame sample"
        );
        assert!(!UvLoops::default().animates());
    }

    #[test]
    fn a_shared_entry_turns_on_the_scene_clock() {
        let e = UvAnimEntry {
            anim: UvLoop::Shared(None),
            slot: 4,
            seed: [0.0, 0.0],
            affine: Some(UvAffine {
                rot: Some(quarter_turn()),
                scale: None,
                slot: 5,
            }),
            clock: None,
            stage1: None,
        };
        // No translation channel: its row stays at zero.
        assert_eq!(e.delta(1.0, 1.0, None), [0.0, 0.0, 0.0, 0.0]);
        // Half-way through the turn the row comes from the raw, unnormalised lerp, as in the
        // reference (`0x713ea0` lerps each component, `0x7bddb0` uses the result as is):
        // `q = (0, 0, 0.35355, 0.85355)`, so `c = 1 − 2z² = 0.75` and `s = 2zw = 0.60355`, not the
        // 0.7071 of a normalised 45°. Normalising fails here.
        let (slot, row) = e.affine(1.0, 1.0, None).expect("the affine row");
        assert_eq!(slot, 5);
        assert!(
            (row[0] + 0.25).abs() < 1e-4 && (row[1] - 0.603_553).abs() < 1e-4,
            "the unnormalised lerp at t = 1 s of a 2 s turn: {row:?}"
        );
        assert!(
            row[2].abs() < 1e-6 && row[3].abs() < 1e-6,
            "no scaling authored, so the scale half is the identity: {row:?}"
        );
        let later = e.affine(1.5, 1.5, None).expect("the affine row").1;
        assert!(
            (later[1] - row[1]).abs() > 1e-2,
            "the shared lane's affine must move with the scene clock: {row:?} vs {later:?}"
        );
    }

    #[test]
    fn the_effect_lane_reads_its_own_play_head() {
        let anim = uv_loop();
        let want = anim.sample(0.75);
        for (attached_at, now) in [(0.0_f32, 0.75_f32), (600.0, 600.75), (600.0, 12.0)] {
            let d = fx_entry(attached_at).delta(now, f64::from(now), Some((0, 0.75)));
            assert!(
                (d[0] - benilla_assets::quantize(want[0], 4096.0)).abs() < 1e-4
                    && (d[1] - benilla_assets::quantize(want[1], 4096.0)).abs() < 1e-4,
                "attach {attached_at}, scene {now}: the play head decides, {d:?}"
            );
        }
    }

    #[test]
    fn a_player_less_effect_falls_back_to_its_age() {
        let anim = uv_loop();
        let want = anim.sample(0.5);
        let d = fx_entry(600.0).delta(600.5, 600.5, None);
        assert!(
            (d[0] - benilla_assets::quantize(want[0], 4096.0)).abs() < 1e-4,
            "age 0.5 s at scene 600.5 s: {d:?}"
        );
    }

    #[test]
    fn the_effect_lane_samples_rotation_and_scale_on_the_same_clock() {
        let mut e = fx_entry(100.0);
        assert!(
            e.affine(100.5, 0.0, Some((0, 0.5))).is_none(),
            "a translation-only transform takes no affine row"
        );
        e.affine = Some(UvAffine {
            rot: None,
            scale: Some(std::sync::Arc::new(
                benilla_formats::SeqLoops::new(vec![Some(benilla_formats::UvAnim {
                    period: 2.0,
                    step: false,
                    wrap: false,
                    gseq: false,
                    keys: vec![(0.0, [1.0, 1.0]), (2.0, [0.25, 0.25])],
                })])
                .expect("one animating slot"),
            )),
            slot: 12,
        });
        let (slot, row) = e
            .affine(100.0, 0.0, Some((0, 1.0)))
            .expect("the affine row");
        assert_eq!(slot, 12);
        // Half-way: scale 0.625, encoded `sx − 1`; no rotation, so `cos − 1` and `sin` are zero.
        assert!(
            row[0].abs() < 1e-6 && row[1].abs() < 1e-6,
            "no rotation authored: {row:?}"
        );
        assert!(
            (row[2] + 0.375).abs() < 1e-3 && (row[3] + 0.375).abs() < 1e-3,
            "scale 0.625 as a delta from the identity: {row:?}"
        );
    }

    /// Stage 1 writes its own rows on its entry's clocks: its translation measured from its own
    /// seed, then its own affine row, beside stage 0's, which they leave alone. Each stage takes
    /// its own transform (`0x70b91f`), and a stage-1-only transform still moves.
    #[test]
    fn stage_one_writes_its_own_rows_on_the_entrys_clocks() {
        let scroll = uv_loop();
        let seed = scroll.sample(0.0);
        let stage1 = Stage1Loops {
            single: Some(scroll.clone()),
            rot: Some(quarter_turn()),
            ..Default::default()
        };
        let loops = UvLoops {
            stage1: stage1.clone(),
            ..Default::default()
        };
        assert!(
            loops.animates() && loops.any(),
            "stage 1 alone moves the batch"
        );
        let mut e = fx_entry(100.0);
        e.stage1 = Some(Stage1Rows {
            seqs: None,
            single: stage1.single.clone(),
            slot: 20,
            seed,
            affine: Some(UvAffine {
                rot: stage1.rot.clone(),
                scale: None,
                slot: 21,
            }),
        });
        // The effect lane at clip time 0.5 s: every row on that one instant.
        let rows = e.rows(None, 100.5, 0.0, Some((0, 0.5)));
        assert_eq!(
            rows.len(),
            3,
            "stage 0's translation, then stage 1's two: {rows:?}"
        );
        assert_eq!(rows[0], (e.slot, e.delta(100.5, 0.0, Some((0, 0.5)))));
        let want = scroll.sample(0.5);
        assert_eq!(rows[1].0, 20);
        assert_eq!(
            [rows[1].1[0], rows[1].1[1]],
            [
                benilla_assets::quantize(want[0], 4096.0) - seed[0],
                benilla_assets::quantize(want[1], 4096.0) - seed[1],
            ],
            "stage 1's delta from its own seed"
        );
        let turn = e.stage1.as_ref().unwrap().affine.as_ref().unwrap();
        assert_eq!(rows[2], turn.row(Some(0), 0.5, f64::from(0.5_f32)));
        assert!(
            rows[2].1[1] > 0.1,
            "a quarter of the way into the turn: {rows:?}"
        );
        assert_eq!(e.slots(), vec![e.slot, 20, 21]);
    }

    /// A batch whose stage 1 alone bakes different loops per slot still takes the per-placement
    /// lane, so its stage 1 follows the slot its host plays; stage 0 keeps its one loop there.
    #[test]
    fn a_stage_one_per_slot_loop_takes_the_per_placement_lane() {
        let host = Entity::from_raw_u32(9).expect("a valid test entity");
        let slots = std::sync::Arc::new(
            benilla_formats::SeqLoops::new(vec![None, Some((*uv_loop()).clone())])
                .expect("slot 1 animates"),
        );
        let loops = UvLoops {
            single: Some(uv_loop()),
            stage1: Stage1Loops {
                seqs: Some(slots.clone()),
                ..Default::default()
            },
            ..Default::default()
        };
        let lane = loops.lane(Some(host));
        assert!(
            matches!(&lane, UvLoop::PerSeq { seqs: None, single: Some(_), host: h } if *h == host),
            "stage 1's slots decide the lane"
        );
        assert!(matches!(loops.lane(None), UvLoop::Shared(Some(_))));
        let mut e = fx_entry(0.0);
        e.anim = lane;
        e.stage1 = Some(Stage1Rows {
            seqs: Some(slots),
            single: None,
            slot: 30,
            seed: [0.0, 0.0],
            affine: None,
        });
        // Slot 0 is dead for stage 1, slot 1 scrolls; stage 0 scrolls on its one loop in both.
        let at = |seq| e.rows(None, 5.0, 5.0, Some((seq, 0.5)));
        assert_eq!(at(0)[1].1, [0.0, 0.0, 0.0, 0.0]);
        assert_ne!(at(1)[1].1, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(at(0)[0].1, at(1)[0].1);
        assert_ne!(at(0)[0].1, [0.0, 0.0, 0.0, 0.0]);
    }

    /// A slot whose stage-1 loop is dead holds stage 1 still, as stage 0 holds in the same slot,
    /// though the slot-0 loop the material seeds from scrolls.
    #[test]
    fn a_dead_slot_holds_stage_one_still() {
        let host = Entity::from_raw_u32(9).expect("a valid test entity");
        let slots = std::sync::Arc::new(
            benilla_formats::SeqLoops::new(vec![Some((*uv_loop()).clone()), None])
                .expect("slot 0 animates"),
        );
        let seed = uv_loop().sample(0.0);
        let mut e = fx_entry(0.0);
        e.anim = UvLoop::PerSeq {
            seqs: Some(slots.clone()),
            single: Some(uv_loop()),
            host,
        };
        e.seed = seed;
        e.stage1 = Some(Stage1Rows {
            seqs: Some(slots),
            single: Some(uv_loop()),
            slot: 30,
            seed,
            affine: None,
        });
        let at = |seq| e.rows(None, 5.0, 5.0, Some((seq, 0.5)));
        let still = [-seed[0], -seed[1], 0.0, 0.0];
        assert_eq!(at(1)[0].1, still, "stage 0 holds in the dead slot");
        assert_eq!(
            at(1)[1].1,
            still,
            "stage 1 holds there too, not on slot 0's loop"
        );
        assert_ne!(at(0)[1].1, still, "slot 0 scrolls");
    }

    /// The effect lane resolves the playing slot as the per-placement lane does: a slot with no
    /// loop beside one that scrolls holds still, never on slot 0's loop.
    #[test]
    fn a_dead_slot_holds_an_effect_still() {
        let mut e = fx_entry(0.0);
        e.anim = UvLoop::Instance {
            seqs: Some(std::sync::Arc::new(
                benilla_formats::SeqLoops::new(vec![Some((*uv_loop()).clone()), None])
                    .expect("slot 0 animates"),
            )),
            single: Some(uv_loop()),
            host: Entity::from_raw_u32(11).expect("a valid test entity"),
            attached_at: 0.0,
        };
        let still = [0.0; 4];
        assert_eq!(e.delta(5.0, 5.0, Some((1, 0.5))), still, "slot 1 holds");
        assert!(e.resolved(Some((1, 0.5))).is_none(), "the probe agrees");
        assert_ne!(e.delta(5.0, 5.0, Some((0, 0.5))), still, "slot 0 scrolls");
        assert_ne!(e.delta(0.5, 5.0, None), still, "no player yet: slot 0");
    }

    /// A batch that turns over a constant translation keeps the translation: the shared lane
    /// carries the period-0 loop, so its row adds nothing to the seed instead of taking it away.
    #[test]
    fn a_turning_batch_keeps_its_constant_translation() {
        let shift = std::sync::Arc::new(benilla_formats::UvAnim {
            period: 0.0,
            step: false,
            wrap: false,
            gseq: false,
            keys: vec![(0.0, [0.25, 0.5])],
        });
        let loops = UvLoops {
            single: Some(shift.clone()),
            rot: Some(quarter_turn()),
            ..Default::default()
        };
        assert!(loops.animates(), "the turn registers the batch");
        let seed = shift.sample(0.0);
        let entry = UvAnimEntry {
            anim: loops.lane(None),
            slot: 3,
            seed,
            affine: None,
            clock: None,
            stage1: None,
        };
        let d = entry.delta(1.0, 1.0, None);
        assert_eq!(
            [seed[0] + d[0], seed[1] + d[1]],
            [
                benilla_assets::quantize(0.25, 4096.0),
                benilla_assets::quantize(0.5, 4096.0)
            ],
            "the shader's seed plus row is the constant shift"
        );
    }

    fn uv_loop() -> std::sync::Arc<benilla_formats::UvAnim> {
        std::sync::Arc::new(benilla_formats::UvAnim {
            period: 2.0,
            step: false,
            wrap: true,
            gseq: false,
            keys: vec![(0.0, [0.1, 0.2]), (1.0, [0.5, 0.6]), (2.0, [0.1, 0.2])],
        })
    }

    #[test]
    fn the_delta_reproduces_the_old_absolute_write() {
        let anim = uv_loop();
        let seed = anim.sample(0.0);
        let entry = UvAnimEntry {
            anim: UvLoop::Shared(Some(anim.clone())),
            slot: 3,
            seed,
            affine: None,
            clock: None,
            stage1: None,
        };
        for t in [0.0_f32, 0.35, 1.0, 1.7] {
            let d = entry.delta(t, f64::from(t), None);
            let s = anim.sample(t);
            let old = [
                benilla_assets::quantize(s[0], 4096.0),
                benilla_assets::quantize(s[1], 4096.0),
            ];
            assert_eq!(
                seed[0] + d[0],
                old[0],
                "t={t}: shader fold == old write (u)"
            );
            assert_eq!(
                seed[1] + d[1],
                old[1],
                "t={t}: shader fold == old write (v)"
            );
            assert_eq!(d[2], 0.0);
            assert_eq!(d[3], 0.0);
        }
    }

    /// A lava bubble's shape: slot 0 bakes nothing and slot 1 the flipbook.
    #[test]
    fn the_per_placement_lane_reads_its_hosts_sequence() {
        let seqs = std::sync::Arc::new(
            benilla_formats::SeqLoops::new(vec![
                None, // slot 0: the dead hold the shared lane is pinned to
                Some(benilla_formats::UvAnim {
                    period: 4.0,
                    step: true,
                    wrap: true,
                    gseq: false,
                    keys: vec![(0.0, [0.0, 0.0]), (2.0, [0.0, 0.605])],
                }),
            ])
            .expect("slot 1 animates"),
        );
        let entry = UvAnimEntry {
            anim: UvLoop::PerSeq {
                seqs: Some(seqs),
                single: None,
                host: Entity::from_raw_u32(7).expect("a valid test entity"),
            },
            slot: 5,
            seed: [0.0, 0.0],
            affine: None,
            clock: None,
            stage1: None,
        };
        // On slot 1, past its step key: the whole V flip shows.
        let d = entry.delta(0.0, 0.0, Some((1, 3.0)));
        assert!(
            (d[1] - 0.605).abs() < 1e-3,
            "the flipbook's V offset: {d:?}"
        );
        // Slot 0 has no loop: the zero offset, here the built seed.
        assert_eq!(entry.delta(0.0, 0.0, Some((0, 3.0))), [0.0; 4]);
        // An unresolvable host degrades to the same identity, never a shared-clock sample.
        assert_eq!(entry.delta(99.0, 99.0, None), [0.0; 4]);
    }

    #[test]
    fn the_tint_delta_reproduces_the_old_absolute_write() {
        let anim = std::sync::Arc::new(benilla_formats::RgbAnim {
            period: 1.0,
            step: false,
            wrap: true,
            gseq: false,
            keys: vec![(0.0, [1.0, 0.5, 0.25]), (1.0, [1.0, 0.5, 0.25])],
        });
        let seed = {
            let s = benilla_assets::quant255(anim.sample(0.0));
            [s[0], s[1], s[2]]
        };
        let entry = TintAnimEntry {
            anim: TintLoop::Shared(anim.clone()),
            slot: 4,
            seed,
            clock: None,
        };
        let d = entry.delta(0.4, 0.4, None);
        let old = benilla_assets::quant255(anim.sample(0.4));
        for i in 0..3 {
            assert_eq!(seed[i] + d[i], old[i], "channel {i}");
        }
    }

    /// A scene that owns its clock animates its texture transforms and colour tracks on it: a
    /// hidden glue pane's clock stops (`0x76d7f0`), and its cloud scroll with it.
    #[test]
    fn a_scene_clocked_material_samples_its_scenes_time_not_the_worlds() {
        let id = bevy::asset::AssetId::<benilla_assets::materials::WowModelMaterial>::Uuid {
            uuid: bevy::asset::uuid::Uuid::from_u128(7),
        };
        let scene = Entity::from_raw_u32(9).unwrap();
        let mut uv = UvAnimMaterials::default();
        uv.0.insert(
            id,
            UvAnimEntry {
                anim: UvLoop::Shared(Some(uv_loop())),
                slot: 3,
                seed: [0.0, 0.0],
                affine: None,
                clock: None,
                stage1: None,
            },
        );
        let pulse = std::sync::Arc::new(benilla_formats::RgbAnim {
            period: 2.0,
            step: false,
            wrap: true,
            gseq: false,
            keys: vec![(0.0, [0.0; 3]), (1.0, [1.0; 3]), (2.0, [0.0; 3])],
        });
        let mut tint = TintAnimMaterials::default();
        tint.0.insert(
            id,
            TintAnimEntry {
                anim: TintLoop::Shared(pulse),
                slot: 4,
                seed: [0.0; 3],
                clock: None,
            },
        );
        scene_clocked(&mut uv, &mut tint, id, scene);
        assert_eq!(uv.0[&id].clock, Some(scene));
        assert_eq!(tint.0[&id].clock, Some(scene));

        // The world is at 10 s, the scene's clock at 0.5 s: the rows are the scene's.
        let row = uv.0[&id].rows(Some(0.5), 10.0, 10.0, None)[0].1;
        assert_eq!(row, uv.0[&id].delta(0.5, 0.5, None));
        assert_ne!(row, uv.0[&id].delta(10.0, 10.0, None));
        let row = tint.0[&id].row(Some(0.5), 10.0, 10.0, None);
        assert_eq!(row, tint.0[&id].delta(0.5, 0.5, None));
        assert_ne!(row, tint.0[&id].delta(10.0, 10.0, None));

        // An entry on no scene clock runs on the world's.
        let other = UvAnimEntry {
            anim: UvLoop::Shared(Some(uv_loop())),
            slot: 5,
            seed: [0.0, 0.0],
            affine: None,
            clock: None,
            stage1: None,
        };
        assert_eq!(
            other.rows(None, 10.0, 10.0, None)[0].1,
            other.delta(10.0, 10.0, None)
        );
    }

    /// A batch alpha following a host that stands in a scene with its own clock samples its
    /// global sequences on that clock: hidden, the scene's vignette or flicker stands still.
    #[test]
    fn a_hosted_alpha_samples_its_hosts_scene_clock() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(100),
        ));
        app.add_systems(Update, sample_mat_anim);
        // A 2 s gseq weight ramp, 0 → 1 → 0.
        let anim = std::sync::Arc::new(
            benilla_formats::AlphaAnim::new(vec![benilla_formats::AlphaSeq {
                color: None,
                weight: Some(benilla_formats::ScalarAnim {
                    period: 2.0,
                    step: false,
                    wrap: true,
                    gseq: true,
                    keys: vec![(0.0, 0.0), (1.0, 1.0), (2.0, 0.0)],
                }),
            }])
            .expect("an animated weight is carried"),
        );
        let host = app.world_mut().spawn(crate::rig_anim::SceneClock(3.0)).id();
        let part = app
            .world_mut()
            .spawn(MatAnim::driving_tag(anim, 0.0, None).following_host(Some(host)))
            .id();
        app.update(); // attach at the scene's 3.0
        let current = |app: &App| app.world().get::<MatAnim>(part).unwrap().current;
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(current(&app), 0.0, "the world ran on; the scene did not");
        app.world_mut()
            .get_mut::<crate::rig_anim::SceneClock>(host)
            .unwrap()
            .0 = 3.5;
        app.update();
        assert!(
            (current(&app) - 0.5).abs() < 1e-4,
            "half a second of the scene's clock past attach: {}",
            current(&app)
        );
    }

    /// A booth's twin of a registered material takes the same loops on its own row, a
    /// per-sequence lane following the booth's model instead of the world's.
    #[test]
    fn a_twin_lane_keeps_its_loops_and_follows_its_own_host() {
        let world_unit = Entity::from_raw_u32(1).unwrap();
        let booth = Entity::from_raw_u32(2).unwrap();
        let seqs = std::sync::Arc::new(
            benilla_formats::SeqLoops::new(vec![Some(benilla_formats::KeyAnim {
                period: 1.0,
                step: false,
                wrap: true,
                gseq: false,
                keys: vec![(0.0, [0.0, 0.0]), (1.0, [1.0, 0.0])],
            })])
            .expect("one animating slot"),
        );
        let per_seq = UvLoop::PerSeq {
            seqs: Some(seqs.clone()),
            single: None,
            host: world_unit,
        };
        match per_seq.for_twin(booth) {
            UvLoop::PerSeq {
                seqs: Some(s),
                host,
                ..
            } => {
                assert!(std::sync::Arc::ptr_eq(&s, &seqs));
                assert_eq!(host, booth);
            }
            _ => panic!("a per-sequence lane stays one"),
        }
        // A twin already registered for another model moves to this one's player.
        let mut moved = UvLoop::PerSeq {
            seqs: Some(seqs.clone()),
            single: None,
            host: world_unit,
        };
        moved.set_host(booth);
        assert!(matches!(moved, UvLoop::PerSeq { host, .. } if host == booth));
        let shared = uv_loop();
        match UvLoop::Shared(Some(shared.clone())).for_twin(booth) {
            UvLoop::Shared(Some(a)) => assert!(std::sync::Arc::ptr_eq(&a, &shared)),
            _ => panic!("a shared lane stays shared"),
        }
        let tint_seqs = std::sync::Arc::new(
            benilla_formats::SeqLoops::new(vec![Some(benilla_formats::KeyAnim {
                period: 1.0,
                step: false,
                wrap: true,
                gseq: false,
                keys: vec![(0.0, [1.0; 3]), (1.0, [0.0; 3])],
            })])
            .expect("one animating slot"),
        );
        match (TintLoop::PerSeq {
            seqs: tint_seqs,
            host: world_unit,
        })
        .for_twin(booth)
        {
            TintLoop::PerSeq { host, .. } => assert_eq!(host, booth),
            TintLoop::Shared(_) => panic!("a per-sequence tint stays one"),
        }
    }
}
