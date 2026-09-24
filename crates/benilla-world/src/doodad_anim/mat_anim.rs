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
            let clip = anims.clips.iter().find(|c| c.node == *node)?;
            Some((clip.seq_index, active.seek_time(), active.weight()))
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(seq, t, _)| (seq, t))
        .or_else(|| anims.idle_seq().map(|seq| (seq, 0.0)))
}

/// Sample every instance: a hosted one on its host's sequence and clip clock, a pinned one on its
/// spawn clock; frozen ones keep their t = 0 sample. Hidden instances sample too, as the reference
/// evaluates the tracks every frame regardless of the cull (`0x707b3a`–`0x707b5c`), or a batch
/// born at weight 0 would stay hidden. Runs before the visibility authority.
pub fn sample_mat_anim(
    time: Res<Time>,
    hosts: Query<(&AnimationPlayer, &ModelAnimations)>,
    mut q: Query<&mut MatAnim>,
) {
    let now = time.elapsed_secs();
    // The scene clock in f64: a long-uptime f32 drifts whole milliseconds.
    let shared = time.elapsed_secs_f64();
    for mut m in &mut q {
        if m.frozen {
            continue;
        }
        // A host with no player yet keeps its last slot at t = 0, a bind-posed model's pose.
        let played = m
            .host
            .and_then(|h| hosts.get(h).ok())
            .and_then(|(p, a)| playing_seq(p, a));
        let (seq, elapsed) = match played {
            Some((seq, t)) => {
                m.seq = Some(seq);
                (Some(seq), t)
            }
            None if m.host.is_some() => (m.seq, 0.0),
            None => (m.seq, now - m.spawned_at),
        };
        // The gseq cursor is sceneNow − attach, the anchor stamped on the first pass.
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
    /// for a rotate- or scale-only transform, whose translation row stays at zero.
    ///
    /// Deviation: every instance scrolls in one phase, where the reference phases a band loop per
    /// arm and a gseq loop from each instance's attach (`CM2Model+0x68`), because one uniform
    /// serves the material and a seamless scroll looks the same at any phase.
    Shared(Option<std::sync::Arc<benilla_formats::UvAnim>>),
    /// The slots disagree: this material belongs to one placement, on whichever slot `host` plays.
    PerSeq {
        seqs: std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>,
        host: Entity,
    },
    /// The spell-effect lane: one effect instance's own clone, scrolling on that instance's clip
    /// (`host`'s player, as [`MatAnim::following_host`] reads it), because an effect's scroll is
    /// its animation: `SwipeCaster.m2`'s UVs lie outside a clamped sheet until the scroll drags
    /// the claw in. `seqs` when the slots disagree, `single` otherwise; at least one is present.
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
            UvLoop::PerSeq { seqs, .. } => playing
                .and_then(|(seq, band_t)| {
                    seqs.seq(Some(seq))
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
                seqs.as_ref()
                    .and_then(|s| s.seq(playing.map(|(seq, _)| seq)))
                    .or(single.as_deref())
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

    /// This entry's affine slot and row, sampled on the clocks [`Self::delta`] uses so all three
    /// channels of one transform read one instant; `None` when the transform only translates.
    pub(crate) fn affine(
        &self,
        now: f32,
        gseq_now: f64,
        playing: Option<(usize, f32)>,
    ) -> Option<(u16, [f32; 4])> {
        let a = self.affine.as_ref()?;
        let (band_t, gseq) = match &self.anim {
            UvLoop::Shared(_) => (now, gseq_now),
            UvLoop::PerSeq { .. } => (playing.map_or(now, |(_, t)| t), gseq_now),
            UvLoop::Instance { attached_at, .. } => {
                let age = now - attached_at;
                (playing.map_or(age, |(_, t)| t), f64::from(age))
            }
        };
        let seq = playing.map(|(seq, _)| seq);
        let q = a
            .rot
            .as_ref()
            .and_then(|r| r.seq(seq))
            .map_or([0.0, 0.0, 0.0, 1.0], |l| l.sample(l.clock(band_t, gseq)));
        let s = a
            .scale
            .as_ref()
            .and_then(|r| r.seq(seq))
            .map_or([1.0, 1.0], |l| l.sample(l.clock(band_t, gseq)));
        Some((a.slot, crate::mat_anim_table::affine_row(q, s)))
    }
}

impl UvAnimEntry {
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
            UvLoop::PerSeq { seqs, .. } => seqs.seq(playing.map(|(seq, _)| seq)),
            UvLoop::Instance { seqs, single, .. } => seqs
                .as_ref()
                .and_then(|s| s.seq(playing.map(|(seq, _)| seq)))
                .or(single.as_deref()),
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
    let Some(entry) = reg.0.get(&id) else {
        return;
    };
    let _ = turns;
    // Both rows written now: an instance attached after this frame's tick would otherwise draw
    // its first frame on zero rows, the identity.
    let (slot, row) = (entry.slot, entry.delta(attached_at, 0.0, None));
    let affine = entry.affine(attached_at, 0.0, None);
    table.set(slot, row);
    if let Some((slot, row)) = affine {
        table.set(slot, row);
    }
}

/// Put one unit, GameObject or held-item batch material on the UV lane: [`UvLoop::PerSeq`] on the
/// caller's own clone when there is a `host` and the slots disagree, else [`UvLoop::Shared`] on
/// the deduped material. A period-0 translation registers only if the batch rotates or scales.
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
    let lane = match (loops.seqs.clone(), host) {
        (Some(seqs), Some(host)) => UvLoop::PerSeq { seqs, host },
        _ => UvLoop::Shared(loops.single.clone().filter(|l| l.period > 0.0)),
    };
    register_uv(reg, table, materials, id, lane);
    attach_affine(
        reg,
        table,
        materials,
        id,
        loops.rot.clone(),
        loops.scale.clone(),
    );
}

/// The four baked channels of one batch's texture transform: the translation as a per-slot set or
/// one loop, and the per-slot rotation and scaling sets.
#[derive(Default)]
pub struct UvLoops {
    pub seqs: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    pub single: Option<std::sync::Arc<benilla_formats::UvAnim>>,
    pub rot: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>>>,
    pub scale: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
}

impl UvLoops {
    /// The one place the four channels are read off a submesh, so the registration and the spawn
    /// marker cannot disagree about which batches animate.
    pub fn of(sub: &benilla_assets::ModelSubmesh) -> Self {
        Self {
            seqs: sub.uv_seq.clone(),
            single: sub.uv_anim.clone(),
            rot: sub.uv_rot_seq.clone(),
            scale: sub.uv_scale_seq.clone(),
        }
    }

    /// Whether any channel is present, the effect attach's test before cloning a material: a
    /// scale-only transform or a dead slot 0 leaves `single` empty.
    pub fn any(&self) -> bool {
        self.seqs.is_some() || self.single.is_some() || self.rot.is_some() || self.scale.is_some()
    }

    /// Whether the batch moves: [`Self::any`] minus a period-0 translation, which is its seed. The
    /// one predicate the entity lane's registration and its [`AnimMatPart`] marker share.
    pub fn animates(&self) -> bool {
        self.seqs.is_some()
            || self.rot.is_some()
            || self.scale.is_some()
            || self.single.as_ref().is_some_and(|l| l.period > 0.0)
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
            table.free(entry.slot);
            if let Some(a) = &entry.affine {
                table.free(a.slot);
            }
            return false;
        }
        // The effect lane skips the draw gate: its parts carry no `AnimMatPart`, and an effect
        // lives about a second, so an undrawn one costs little.
        let fx = entry.instance_lane();
        if (fx || drawn.contains(id)) && (!frozen || fx) {
            let playing = host_seq(&hosts, entry.host());
            table.set(entry.slot, entry.delta(now, gseq_now, playing));
            if let Some((slot, row)) = entry.affine(now, gseq_now, playing) {
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
            table.set(
                entry.slot,
                entry.delta(now, gseq_now, host_seq(&hosts, entry.host())),
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
    reg.0.insert(id, TintAnimEntry { anim, slot, seed });
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
        format!(
            "uv slot {} seed ({:+.4},{:+.4}) row ({:+.4},{:+.4}) live ({:+.4},{:+.4}){affine} {lane} loop {}",
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
                seqs,
                host: Entity::from_raw_u32(7).expect("a valid test entity"),
            },
            slot: 5,
            seed: [0.0, 0.0],
            affine: None,
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
        };
        let d = entry.delta(0.4, 0.4, None);
        let old = benilla_assets::quant255(anim.sample(0.4));
        for i in 0..3 {
            assert_eq!(seed[i] + d[i], old[i], "channel {i}");
        }
    }
}
