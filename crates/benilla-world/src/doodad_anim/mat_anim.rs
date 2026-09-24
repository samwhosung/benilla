//! Animated **materials** on placed models (decision 0130 phases 2/3): the per-batch baked
//! colour-alpha/weight loops ([`MatAnim`] — composed into the render-alpha `MeshTag` by the
//! visibility authority) and the shared-clock UV-scroll / tint registries
//! ([`UvAnimMaterials`] / [`TintAnimMaterials`]) their samplers tick. Split from the host module
//! ([`super`]) along the concern seam: this file is about *material* channels; the host file is
//! about *bone* channels (the anim host, the variation windows, the draw gate).

use bevy::prelude::*;

use benilla_assets::ModelAnimations;

/// Per-submesh **animated material alpha** (decision 0130 phase 2): the batch's baked
/// colour-alpha/weight loops + how this instance clocks them. [`sample_mat_anim`] keeps
/// [`Self::current`] fresh; the model-`Visibility` authority multiplies it into the render-alpha
/// `MeshTag` it already owns (a composed *input*, not an extra tag writer — the 0066 protocol) and
/// hides the batch at combined 0 (the `A ≤ 0` cull, `0x707b3a`–`0x707b5c`).
///
/// The loops are baked **per sequence** (`benilla_formats::AlphaAnim`), so an instance also has to
/// say *which* sequence it is playing — see [`Self::host`].
#[derive(Component)]
pub struct MatAnim {
    anim: std::sync::Arc<benilla_formats::AlphaAnim>,
    /// The entity whose `AnimationPlayer` decides which sequence's loops to read, re-resolved every
    /// frame — the **unit lane**, where the played sequence changes constantly and the batch's
    /// authored visibility changes with it (a voidwalker's upper armour is weight 0 in Stand and 1
    /// only in Death). `None` for an instance pinned to one sequence for its life: a placed doodad
    /// (armed once at load with `animations[0]`, `0x70ebd0`/`0x7121a0`) or a spell effect —
    /// both then read [`Self::seq`] on the spawn clock, which is the pre-per-sequence behaviour.
    host: Option<Entity>,
    /// The sequence **file slot** to read: fixed at spawn for the pinned lanes, and the last one
    /// resolved from [`Self::host`] for the unit lane. `None` ⇒ slot 0 (the bake's own degrade).
    seq: Option<usize>,
    /// `Time::elapsed_secs` at spawn — the clock origin (arm-time phase, like the bone host). Only
    /// the pinned lanes use it; a hosted instance reads the player's own seek time instead, so its
    /// alpha stays in phase with the pose that drives it.
    spawned_at: f32,
    /// Captures freeze the clock at 0 for deterministic frames (dimming constants still show).
    frozen: bool,
    /// This instance's sampled value drives the render-alpha `MeshTag` field **by itself** (no
    /// `DoodadFade` on the entity): the spell-effect parts (`entities::spell_fx`), whose alpha
    /// channel has no other writer. `false` on the doodad lane — there the visibility authority
    /// owns the tag and composes [`Self::current`] in (for a fade holder multiplied with the
    /// distance fade; for a lit interior prop written alone into the probe payload's alpha field,
    /// bits 0..=15 since the 0355 re-lane) — and on the unit lane, whose own compose is
    /// [`crate::entities::apply_unit_mat_alpha`].
    pub(crate) drives_tag: bool,
    /// This instance belongs to the **unit lane's** tag compose
    /// ([`crate::entities::apply_unit_mat_alpha`]) even though it has no [`Self::host`] to read a
    /// sequence from: the ATTACH-MODEL case (a held weapon, a helm, a pauldron). Such a model
    /// spawns no rig — it rests in its file's first sequence, so its loops are pinned like a
    /// placed doodad's — but it hangs off a unit, so the compose has to be the one ordered against
    /// the wearer's appear-fade and interior classifier rather than the world-model visibility
    /// authority's. See [`Self::resting`].
    unit_lane: bool,
    /// The gseq factors' ATTACH anchor (secs on the shared clock): `None` until the first
    /// [`sample_mat_anim`] pass stamps it — the reference snapshots the scene clock once per
    /// model instance at attach (`CM2Model+0x68`; decisions 0856/0858).
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
        // The seed sample: both clocks at 0 — nothing armed, and the attach anchor (stamped on
        // the first live pass) makes the gseq cursor open at 0 too.
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

    /// The spell-effect-lane constructor: never frozen (the `fxview` instrument ages effects
    /// through captures; golden scenarios spawn no effects), the sampled alpha drives the part's
    /// render-alpha tag directly (see [`Self::drives_tag`]), and the instance is pinned to the one
    /// sequence its rig plays (`seq` — the missile's InFlight, else the file-order-first clip).
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

    /// Read the sequence (and its clock) from `host`'s live `AnimationPlayer` each frame instead of
    /// staying pinned to the slot this instance opened on — for a spell-effect instance that
    /// **advances** through its authored lifecycle (`Stand` → `Hold` → `Decay`, `0x5ff170` arms
    /// Hold, `0x5ff270` the terminal Decay), because each leg has its own authored alpha loops: Ice
    /// Barrier's pulse is as much the `Hold` band's oscillating transparency weights as its bone
    /// scale. `None` leaves the instance pinned (a lane with no rig has no player to ask).
    ///
    /// It keeps [`Self::drives_tag`], so this stays an effect part writing its own tag — it borrows
    /// the unit lane's *sequence source*, not its tag-compose ownership.
    pub fn following_host(mut self, host: Option<Entity>) -> Self {
        self.host = host;
        self
    }

    /// The **unit-lane** constructor: the sequence (and its clock) come from `host`'s live
    /// `AnimationPlayer`, so a creature's batches appear and disappear with the animation exactly
    /// as authored. The tag is composed by [`crate::entities::apply_unit_mat_alpha`], not driven
    /// here — the interior classifier and the appear-fade already own that channel.
    pub fn following(anim: std::sync::Arc<benilla_formats::AlphaAnim>, host: Entity) -> Self {
        Self {
            host: Some(host),
            ..Self::new(anim, 0.0, false)
        }
    }

    /// The **attach-model** constructor (a held weapon, a helm, a pauldron): the loops are pinned
    /// to the file's first sequence — an item model spawns no rig and rests there, the same reason
    /// its emitters and ribbons read Stand — while the tag compose stays the unit lane's, ordered
    /// against the wearer's appear-fade and interior classifier.
    ///
    /// Its clock is irrelevant in practice (a rest-pose model's tracks are the constants the file
    /// authors) so it takes no `now`; sampling still runs every frame like every other lane, so a
    /// keyed track on a rest sequence animates rather than latching at its first key.
    pub fn resting(anim: std::sync::Arc<benilla_formats::AlphaAnim>) -> Self {
        Self {
            unit_lane: true,
            ..Self::new(anim, 0.0, false)
        }
    }

    /// Whether this instance's tag alpha is the unit lane's to compose (see
    /// [`crate::entities::apply_unit_mat_alpha`]) — a hosted creature/player batch, or an
    /// attach model's batch ([`Self::resting`]), and never a self-driving effect part.
    pub fn composes_unit_tag(&self) -> bool {
        (self.host.is_some() || self.unit_lane) && !self.drives_tag
    }
}

/// The sequence slot + clip-local time a host is playing, for [`sample_mat_anim`]: the **base**
/// animation with the greatest blend weight. Masked overlays (a torso-only swing, an arm's draw
/// ceremony, the finger grip) run on their own graph nodes and are deliberately skipped — they
/// pose bones, they don't reselect the sequence the material tracks read. During a cross-fade two
/// base clips are live and the heavier one wins; the reference instead blends the two sampled
/// scalars by λ (`0x71af20`'s blend leg), a sub-blend-time difference on tracks
/// the corpus authors as 0/1 steps — recorded, not modelled.
///
/// A player with **nothing armed** is not "no sequence" — the reference arms the loader-idle clip on
/// every M2 instance at load, so the answer is that sequence at its opening frame
/// ([`ModelAnimations::idle_seq`], decision 0936). benilla's rig tier skips the arm whenever looping
/// the idle would render identically to the static mesh, and that skip used to leak out of the mesh
/// question into this one: both callers read the returned `None` as "keep the pinned slot", which
/// starts life at file slot 0. On a Spawn/Stand/Despawn GameObject slot 0 is the Spawn flourish, so
/// the batch read a sequence the instance was never playing.
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

/// Sample every instance's material-alpha loops: a hosted instance on its host's playing sequence
/// and clip clock, a pinned one on its own spawn clock. Frozen (capture) instances keep their t=0
/// sample. Hidden instances sample too — the visibility authority's alpha cull reads
/// [`MatAnim::current`], so a batch BORN at weight 0 (`HolyLight_Low_Head`'s light column, keyed
/// `0 → 1` at 300 ms) must keep its clock running or the alpha-hide latches forever (the old
/// skip-while-Hidden held `current` at the 0 that caused the hide — the invisible pala-heal flash).
/// Sampling is a pure function of the clock, so an instance hidden for any *other* reason (draw
/// gate, far-clip) still lands right on re-appear, and the reference animation-evaluates the tracks
/// every frame regardless of the cull (`0x707b3a`–`0x707b5c`). Runs before the visibility
/// authority so the tag it composes is this frame's value.
pub fn sample_mat_anim(
    time: Res<Time>,
    hosts: Query<(&AnimationPlayer, &ModelAnimations)>,
    mut q: Query<&mut MatAnim>,
) {
    let now = time.elapsed_secs();
    // The scene clock, full-precision (a long-uptime f32 elapsed drifts whole milliseconds,
    // which a 433 ms twinkle shows): the base the per-instance gseq attach anchor subtracts
    // from (0856).
    let shared = time.elapsed_secs_f64();
    for mut m in &mut q {
        if m.frozen {
            continue;
        }
        // A hosted instance whose host has no player yet (the frame before the rig arms, or a
        // rest-pose GameObject) keeps its last resolved slot and reads it at t=0 — the sequence's
        // opening pose, which is what a model sitting at bind pose shows.
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
        // The instance's gseq cursor: sceneNow − attach, the anchor stamped on this first pass
        // (decisions 0856/0858 — every lane, spell effects included: fresh instance per play).
        let attach = *m.gseq_attach.get_or_insert(shared);
        m.current = m.anim.sample(seq, elapsed, shared - attach);
    }
}

/// The **UV-animated materials** registry (decision 0130 phase 3, `0x716216`'s translation gate):
/// each batch material carrying a texture-transform translation loop, keyed by material asset id.
/// [`tick_anim_materials`] re-samples a *drawn* entry's offset into the material's `sun_scale.zw`
/// each frame — one shared uniform per material, so every instance of a model batch scrolls in
/// phase. A recorded, invisible divergence for BOTH clock laws (0856): the reference phases a
/// seq-band loop per play (arm cursor) and a gseq loop per instance (attach anchor,
/// `CM2Model+0x68`), but one uniform per material cannot phase per instance — meaningless for a
/// seamless scroll either way. Entries drop when the material asset does.
/// The scan marker (1375): a part whose material can ever be a [`UvAnimMaterials`]/
/// [`TintAnimMaterials`] key — inserted at spawn, beside the registration itself, and only for a
/// loop with a real period (a period-0 constant is fully served by its material seed). Without
/// it, [`tick_anim_materials`]'s draw scan visited every `WowModelMaterial` row in the world
/// (~48k at Stormwind) to find the placed instances of ~113 animated models.
#[derive(bevy::prelude::Component)]
pub struct AnimMatPart;

/// **Which loop a registered material animates on** — and the whole of decision 1408.
///
/// A registry keyed by MATERIAL is shared by every instance of a batch, so it has no sequence to
/// key on. That is exactly right while every sequence bakes the same loop, and structurally unable
/// to be right when they don't: the BRM lava bubbles key their whole flipbook inside a 50 %-weighted
/// variation, and their 15 placements re-roll independently every ~3.3 s (decision 0768), so at any
/// instant some are on slot 0 and some on slot 1. One shared row cannot serve both.
///
/// So a batch whose slots disagree — 22 batch-channels across **6 models**, corpus-wide
/// (`benilla-extract uvslotscan`) — takes a material of its own **per placement**, keyed by its
/// anim host in [`crate::model_render::MatKey`], and this entry remembers that host so the sample
/// rides the sequence the host is actually playing. Everything else keeps the shared material and
/// the shared clock, untouched.
///
/// Per-placement materials rather than a per-instance row in the shared table: the row would have
/// to be addressed from the shader, and every per-instance channel there is spoken for
/// (`MeshTag`'s 32 bits are fully allocated; the instance slot is the lazily-allocated,
/// pressure-reaped palette slot). Against a measured population of six models it buys a new GPU
/// region and a shader branch to save a handful of draw calls on small atmospheric props, and the
/// clones are bounded by the same distance evictor every other material has (`art_scope`,
/// decision 0785). Revisit if the population ever grows.
pub enum UvLoop {
    /// Every sequence bakes the same loop: the shared material, on the free-running shared clock.
    ///
    /// `None` when the batch's transform carries **no translation at all** — a rotate-only or
    /// scale-only transform, whose whole motion is the [`UvAnimEntry::affine`] row beside this
    /// one. Two entity batches in the shipped corpus are exactly that
    /// (`World\Goober\G_ScryingBowl`'s water and `G_ScourgeRuneCircleCrystal`'s rune ring, both
    /// `chans —R—`), so a lane that made the translation mandatory would have to invent a
    /// zero loop for them or drop them silently. The entry still holds its translation row; it
    /// stays at zero, which is what that row means.
    Shared(Option<std::sync::Arc<benilla_formats::UvAnim>>),
    /// The slots disagree: this material belongs to ONE placement, and the loop is whichever slot
    /// `host` is playing right now.
    PerSeq {
        seqs: std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>,
        host: Entity,
    },
    /// The **spell-effect** lane (decision 2282): this material is one effect instance's own
    /// clone, and the scroll rides that instance's clip — `host`'s live `AnimationPlayer`, the
    /// same source [`MatAnim::following_host`] reads, so the two material channels of one cast
    /// stay in step through `Stand` → `Hold` → `Decay`.
    ///
    /// **Per instance, not shared, because on an effect the scroll is not ambience — it IS the
    /// animation.** A waterfall's loop is seamless, so one uniform for every placement on a
    /// free-running clock is indistinguishable from the truth. The druid's claw trail is a
    /// one-shot reveal: `Spells\SwipeCaster.m2` is two strips whose UVs are authored wholly
    /// outside `0..1` against a CLAMP-addressed sheet, and the 1.5 s U-scroll is what drags the
    /// claw along them. On a shared clock every bear in the raid would sweep in one phase none of
    /// their casts chose; frozen at the track's first key — which is what the effect lane did
    /// until 2282 — the strips sample only the sheet's transparent border and the trail does not
    /// exist at all.
    ///
    /// `seqs` (1408's per-slot set) when the batch's slots disagree, `single` otherwise; a batch
    /// whose slot 0 is dead beside a live later slot carries only the first, so both are optional
    /// and at least one is always present.
    Instance {
        seqs: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
        single: Option<std::sync::Arc<benilla_formats::UvAnim>>,
        host: Entity,
        /// `Time::elapsed_secs` at attach — this instance's clock origin. It is the band clock
        /// for an instance whose model arms no player, and the global-sequence anchor
        /// (`sceneNow − attach`, decisions 0856/0858: one fresh instance per play) for every one.
        attached_at: f32,
    },
}

/// One registered UV-scroll material: its sampler, its table slot, and the built seed the delta
/// rows are measured from (the material's own `sun_scale.zw`, which is never mutated again —
/// decision 1381).
pub struct UvAnimEntry {
    pub anim: UvLoop,
    pub slot: u16,
    pub seed: [f32; 2],
    /// The effect lane's **affine half** ([`UvAffine`], decision 2282) — `None` on every other
    /// lane, and on an effect batch whose transform only translates (the common case).
    affine: Option<UvAffine>,
}

/// A texture transform's ROTATION and SCALING, on the effect lane: the two channels the
/// translation row cannot carry, and the second table row that does ([`affine_row`]'s encoding,
/// decision 2019).
///
/// They ride the same instance clock as the translation beside them, and they exist here for the
/// same reason that one does — `Spells\ShieldWall_Impact_Base.mdx`'s halo turns AND scales, and
/// `Spells\GroundingTotem_Impact.mdx` is a **scale-only** transform, so a lane that ran only the
/// scroll would still render both wrong. `benilla-extract fxuvscan` is where that population is
/// read; it is three models today, which is exactly why nobody would have found them by eye.
struct UvAffine {
    rot: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>>>,
    scale: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    /// The affine row's slot (`anim_slots.z`). Its encoding is a delta from the IDENTITY, not
    /// from a material seed — row 0 is the identity, so there is nothing to measure from.
    slot: u16,
}

impl UvAnimEntry {
    /// The slot's delta row for `now`: the quantized sample minus the built seed — the shader
    /// adds it back onto `sun_scale.zw` (decision 1381's encoding). Quantized exactly as the
    /// old asset-mutating lane quantized its absolute writes, so the first live frame shows the
    /// same number the old path would have written.
    ///
    /// `playing` is the per-placement lane's live `(sequence slot, clip time)`, resolved by the
    /// caller from [`Self::host`]; a host that has gone (despawned mid-frame) or a sequence with no
    /// loop samples the identity, i.e. the batch sits at its built seed — the same degrade a full
    /// table gives.
    pub(crate) fn delta(&self, now: f32, gseq_now: f64, playing: Option<(usize, f32)>) -> [f32; 4] {
        let uv = match &self.anim {
            UvLoop::Shared(anim) => anim.as_ref().map_or([0.0, 0.0], |a| a.sample(now)),
            UvLoop::PerSeq { seqs, .. } => playing
                .and_then(|(seq, band_t)| {
                    seqs.seq(Some(seq))
                        .map(|l| l.sample(l.clock(band_t, gseq_now)))
                })
                .unwrap_or([0.0, 0.0]),
            // The effect lane's clocks are the INSTANCE's, never the scene's (2282): the band
            // clock is the clip this instance is playing — the spawn age while its rig has no
            // player yet, which is `MatAnim`'s own degrade — and the gseq cursor is measured from
            // its attach rather than from process start.
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

    /// This entry's **affine** row and the slot it goes in, or `None` when the transform only
    /// translates. Sampled on **the same clocks [`Self::delta`] uses for this entry's own lane**,
    /// so the three channels of one transform are always read at one instant — a rotation read on
    /// the scene clock beside a translation read on an instance's play head would be two different
    /// moments of the same matrix.
    pub(crate) fn affine(
        &self,
        now: f32,
        gseq_now: f64,
        playing: Option<(usize, f32)>,
    ) -> Option<(u16, [f32; 4])> {
        let a = self.affine.as_ref()?;
        // Per lane, exactly as `delta` does it: the shared lane reads the free-running scene clock
        // on both, the per-placement lane its host's playing clip, and the effect lane its own
        // age-anchored pair (decisions 0856/0858).
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
    /// The placement whose sequence this entry rides, or `None` on the shared lane.
    fn host(&self) -> Option<Entity> {
        match &self.anim {
            UvLoop::Shared(_) => None,
            UvLoop::PerSeq { host, .. } | UvLoop::Instance { host, .. } => Some(*host),
        }
    }

    /// Is this the **spell-effect** lane ([`UvLoop::Instance`])? The one entry kind
    /// [`tick_anim_materials`] keeps sampling inside a deterministic capture, on exactly
    /// [`MatAnim`]'s and `spell_fx::FxTintAnims`' own argument: an effect's clock is its
    /// instance's frame-stepped `AnimationPlayer`, not wall time, so `fxview` can age one without
    /// any golden scenario — which spawns no effects at all — moving a pixel.
    fn instance_lane(&self) -> bool {
        matches!(self.anim, UvLoop::Instance { .. })
    }

    /// The loop this entry samples right now — the instrument's read
    /// ([`crate::doodad_anim::matanim_probe`]), so the probe's period and phase sweep come from
    /// the same resolution [`Self::delta`] makes rather than a hand-rolled twin of it.
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

    /// The `now` a phase-sweep sample passes for this entry. The effect lane measures its clocks
    /// from ATTACH, so handing it a raw phase `t` would read as a negative age; every other lane
    /// is already on the scene clock. (The sweep moves the play head too — see the caller.)
    fn phase_now(&self, t: f32) -> f32 {
        match &self.anim {
            UvLoop::Instance { attached_at, .. } => attached_at + t,
            UvLoop::Shared(_) | UvLoop::PerSeq { .. } => t,
        }
    }
}

impl TintAnimEntry {
    /// [`UvAnimEntry::host`]'s twin.
    fn host(&self) -> Option<Entity> {
        match &self.anim {
            TintLoop::Shared(_) => None,
            TintLoop::PerSeq { host, .. } => Some(*host),
        }
    }
}

/// The anim hosts a per-placement entry reads its sequence from — [`playing_seq`] behind a query,
/// so [`tick_anim_materials`] can resolve one per entry without borrowing the world twice.
pub type SeqHosts<'w, 's> = Query<'w, 's, (&'static AnimationPlayer, &'static ModelAnimations)>;

/// The playing sequence slot + clip time of one anim host, or `None` if the entry is on the shared
/// lane, the host is gone, or it has no sequence at all.
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

/// Register material `id` for the per-frame UV scroll: allocate its table slot, remember the
/// built seed, and bake the slot index into the material's `anim_slots.x` — the ONE material
/// write this lane ever makes (spawn-frame, where the asset is Modified anyway). A full table
/// (never seen below ~500 resident animated materials) skips registration: the batch stays
/// frozen at its built seed — a degraded look, never a wrong pixel.
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
    // **Through the parked half, not `Assets::get_mut`** (decision 2038). A material this
    // spawner just built is not in the store yet — `model_render::lazy` reserves its handle and
    // parks the value until something view-visible binds it — so the store-only read returned
    // `None` for *every* registration, freed the slot again and left the registry empty: no UV
    // scroll and no animated tint anywhere in the world, silently, from the day deferral landed.
    let Some(seed) = crate::model_render::lazy::with_material_mut(materials, id, |mat| {
        let seed = [mat.extension.sun_scale.z, mat.extension.sun_scale.w];
        mat.extension.anim_slots.x = f32::from(slot);
        seed
    }) else {
        // Neither half holds it: a dead or foreign handle, and nothing to animate. Loud, because
        // the silent version of this branch is what cost three days of frozen water.
        bevy::log::warn_once!(
            "mat-anim: no material behind {id} — a UV-scroll batch stays at its seed"
        );
        table.free(slot);
        return;
    };
    match &anim {
        // The breadcrumb for the lanes that have no other tell: a per-placement or per-instance
        // material is invisible in every count (it is one more material, one more row), so "did
        // the bubbles / the claw trail take the new lane at all" would otherwise be a question
        // only the eye could answer. One line per registration, at debug (decisions 1408, 2282).
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

/// Attach the ROTATION/SCALING half to an entry [`register_uv`] has just made, if the batch has
/// one — the second table row, `anim_slots.z`, and 2019's `[cos − 1, sin, sx − 1, sy − 1]`
/// encoding whose zero row IS the identity.
///
/// The translation half decides whether there is an entry at all (a full table, a dead handle);
/// the affine half attaches to it or not at all. A transform that ONLY rotates or scales still
/// takes a translation row that stays at zero — one row, on a population of five models corpus-
/// wide, for a registration path with one shape instead of two.
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

/// Put one **spell-effect material clone** on the per-instance UV lane (decision 2282): the
/// engine's whole door for `entities::spell_fx`, which owns the clone and nothing else about how
/// the scroll is delivered.
///
/// One door rather than the three pieces behind it ([`UvLoop`]'s effect variant, [`register_uv`],
/// the table row), because they are one fact — *this clone scrolls, on that instance's clip* —
/// and the class of bug this lane keeps producing is a caller that took some of the pieces and
/// not the rest (decision 2038's registry of marked-but-unregistered parts is the same shape).
///
/// `seqs` / `single` are the batch's baked loops in 1408's two spellings, at least one present;
/// `host` is the effect instance root whose `AnimationPlayer` names the playing clip, and
/// `attached_at` its `Time::elapsed_secs` origin. The clone's `sun_scale.zw` is read as the seed
/// the delta rows are measured from, exactly as on the shared lane, so a caller that seeds it at
/// the loop's own `t = 0` gets a correct FIRST frame before this lane has written a row.
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
    // **Both rows, written now.** `tick_anim_materials` runs once per frame in `Update` and this
    // registration happens in the same schedule, so an instance that attaches after the tick would
    // otherwise draw its first frame at row 0 — the identity, which on this population is the
    // authored-UV frame the whole decision is about (a Swipe strip renders NOTHING there). One
    // write at spawn costs nothing and removes the class.
    let (slot, row) = (entry.slot, entry.delta(attached_at, 0.0, None));
    let affine = entry.affine(attached_at, 0.0, None);
    table.set(slot, row);
    if let Some((slot, row)) = affine {
        table.set(slot, row);
    }
}

/// Put one **unit / GameObject / held-item** batch material on the UV lane (decision 2295) — the
/// engine's whole door for the entity lane, and the twin of [`register_fx_uv`] for a population
/// that is resident rather than pooled.
///
/// It picks the lane from the authored record rather than making the caller reason about it,
/// which is the same argument [`UvLoops`] itself rests on:
///
/// - **`host` is `Some` and the slots disagree** ⇒ [`UvLoop::PerSeq`]. This material belongs to
///   that one instance (the caller owns the clone), and the loop is whichever file-sequence slot
///   the instance is playing — 1408's lane, reached from the entity side for the first time.
///   `World\Lordaeron\Plagueland\PassiveDoodads\BloodOfHeroes` is the class: slot 0 holds its
///   bubble sheets still while slot 1 sweeps them, at freq 16384/16383, so its placements bubble
///   independently and one shared row cannot serve two of them.
/// - **otherwise** ⇒ [`UvLoop::Shared`], the free-running scene clock on the batch's shared,
///   deduped material — 0136 choice 1. Exactly faithful for a global-sequence loop (the
///   reference clocks those on one per-scene ms cursor), and 0136's recorded, still-invisible
///   divergence for a seamless band loop.
///
/// A period-0 translation is not a loop — its seed IS its forever value, so registering it would
/// buy a per-frame re-write of the same number (1375) — but a batch that only rotates or scales
/// still registers, because its motion is the affine row and its translation row is the zero it
/// is meant to be.
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

/// The four baked channels of one batch's texture transform, as [`register_fx_uv`] takes them —
/// 1408's two translation spellings plus 2019's per-slot rotation and scaling sets. A struct
/// because they are one authored record, and a caller passing four `Option`s positionally is a
/// caller that will eventually swap two of them.
#[derive(Default)]
pub struct UvLoops {
    pub seqs: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    pub single: Option<std::sync::Arc<benilla_formats::UvAnim>>,
    pub rot: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>>>,
    pub scale: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
}

impl UvLoops {
    /// The record as one authored batch carries it — **the one place the four channels are read
    /// off a submesh**, so a lane that registers and a spawner that marks can never disagree about
    /// which batches animate (decision 2038's law, whose violation froze every waterfall in the
    /// game for three days).
    pub fn of(sub: &benilla_assets::ModelSubmesh) -> Self {
        Self {
            seqs: sub.uv_seq.clone(),
            single: sub.uv_anim.clone(),
            rot: sub.uv_rot_seq.clone(),
            scale: sub.uv_scale_seq.clone(),
        }
    }

    /// Does this batch animate its texture transform at all? The test the effect attach makes
    /// before cloning a material: any of the four channels, because a scale-only transform
    /// (`Spells\GroundingTotem_Impact.mdx`) and a dead-slot-0 translation (1408) each answer
    /// `None` to the obvious one.
    pub fn any(&self) -> bool {
        self.seqs.is_some() || self.single.is_some() || self.rot.is_some() || self.scale.is_some()
    }

    /// Does it need a per-frame SAMPLE — i.e. does it move? [`Self::any`] minus the constants: a
    /// translation keyed to one value for the whole clip is its material's seed and nothing more
    /// (1375), while a rotate-only or scale-only batch answers `None` on the translation channel
    /// and still moves. The **one predicate** the entity lane's registration and its
    /// [`AnimMatPart`] marker both ask.
    pub fn animates(&self) -> bool {
        self.seqs.is_some()
            || self.rot.is_some()
            || self.scale.is_some()
            || self.single.as_ref().is_some_and(|l| l.period > 0.0)
    }

    /// The offset the batch opens on — what a caller seeds the clone's `sun_scale.zw` with so its
    /// first frame is the loop's own `t = 0` and not the authored, unscrolled UV.
    pub fn open_offset(&self) -> [f32; 2] {
        self.seqs
            .as_ref()
            .and_then(|s| s.seq(None))
            .or(self.single.as_deref())
            .map_or([0.0, 0.0], |l| l.sample(0.0))
    }
}

/// Re-sample the **drawn** animated materials on the shared clock — the UV scroll
/// ([`UvAnimMaterials`]) and the RGB tint ([`TintAnimMaterials`]) together, because they share the
/// draw scan below. Skipped in captures (materials keep their t = 0 seed — constants still show,
/// frames stay deterministic), **except the spell-effect lane**
/// ([`UvAnimEntry::instance_lane`]), whose clock is an effect instance's own frame-stepped
/// player: `fxview` exists to age an effect inside a capture, and no golden scenario spawns one,
/// so a scene with no effect in it takes the identical early return it always did.
///
/// **The draw gate, and why it is the fix for B131.** Mutating a material asset marks it Modified,
/// which on the Metal non-bindless path re-creates its uniform buffers *and its bind group* that
/// frame. 0130 sized this as "a few uniform writes per frame" on the premise that the resident
/// population is tiny (113 texanim models exist game-wide, a handful in view) — true per view,
/// **false per map session**: the dedup caches hold every material they ever built until a
/// `MapChange` (decision 0729), and a registry entry only evicts when its material dies, so on a
/// single-map traverse both registries grow monotonically and every entry is re-uploaded every
/// frame for ever. Measured on a parked same-map leg by square-waving this system:
/// **+9.85 ms of CPU per frame at 174 resident entries (~57 µs each)** — and residency 4 → 248
/// entries inside ten minutes, recoverable only by a restart or a map change. That is B131.
///
/// Gating on the draw is not a workaround for that growth, it is this lane finally obeying the
/// module's own law: [`gate_doodad_anim`] already gates the *pose* on "any submesh actually drawn",
/// on exactly the byte ground that makes it free — sampling is clock-indexed, so a material that
/// re-appears is written from `now` and shows the value the shared clock dictates, with nothing to
/// catch up (the module docs' "pausing costs nothing and drifts nothing"). A material nothing draws
/// is a per-frame GPU rebuild with no pixel to show for it.
///
/// `Visibility != Hidden` is the same spelling [`gate_doodad_anim`] uses — the authority's verdict
/// (far clip + size-bucketed fade + portal cull), written in `Update` by
/// `debug_panel::ModelVisSet`, which this system is ordered after so the verdict is *this* frame's.
/// It over-includes (a part left `Inherited` under a hidden ancestor counts as drawn), which is the
/// safe direction: an extra write costs a frame's uniform upload, a missed one would freeze a
/// visible scroll.
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
    // The per-placement lane's sequence source (decision 1408): an entry registered `PerSeq` asks
    // its own anim host which slot it is playing, instead of reading the shared clock.
    hosts: SeqHosts,
    mut drawn: Local<
        bevy::platform::collections::HashSet<AssetId<benilla_assets::materials::WowModelMaterial>>,
    >,
) {
    if uv_reg.0.is_empty() && tint_reg.0.is_empty() {
        // **The tripwire for decision 2038's whole class.** [`AnimMatPart`] and the registration
        // next to it share ONE predicate at the spawn site, so a world holding marked parts with
        // an empty registry is never "a scene with no animated content" — it is this lane dead,
        // which is exactly how every waterfall, canal and lava sheet in the game froze silently
        // for three days. Free: the marker is archetype-filtered, so this is one iterator probe
        // on a frame that was about to return anyway, and it says nothing in the ordinary empty
        // case.
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
    // A deterministic run freezes the SHARED-clock lanes at their seed (the whole reason a golden
    // frame is reproducible) but keeps the effect lane running, because that lane's clock is one
    // instance's frame-stepped `AnimationPlayer` — the `fxview` carve-out `MatAnim` and
    // `spell_fx::FxTintAnims` already take (decision 2282). A capture with no effect in it — every
    // golden scenario — holds no such entry, so it returns here exactly as it always has.
    let frozen = crate::dev_state::deterministic_run() && !matanim_live();
    if frozen && !uv_reg.0.values().any(UvAnimEntry::instance_lane) {
        return;
    }
    drawn.clear();
    // Skipped when frozen: the only entries still sampling there are the effect lane's, and they
    // are exempt from the draw gate below — so a capture pays nothing for a scan it cannot read.
    for (mat, vis) in parts.iter().filter(|_| !frozen) {
        if *vis != Visibility::Hidden {
            let id = mat.id();
            // A far-classified instance carries the far TWIN's id — never a registry key. Count
            // it as its near identity, or a batch whose every instance sits beyond the water
            // plane marks its near entry not-drawn and freezes both variants' scroll (1375; the
            // twin itself keeps getting written by `classify_water_side`'s Modified mirror).
            drawn.insert(twins.near_of(id).unwrap_or(id));
        }
    }
    let now = time.elapsed_secs();
    // The samples land in the shared table as DELTAS from each entry's built seed (decision
    // 1381) — the material asset is never touched, so there is no per-frame `Modified`, no
    // bind-group rebuild, no `AssetChanged` walk, and no far-twin re-insert (the twin's clone
    // carries the same slot and seed, so it scrolls in phase off the same row). Quantized
    // because the input drifts continuously: 1/4096 of a texture repeat (1/255 for tint) is
    // below what any face can show — and `MatAnimTable::set` skips same-value writes, so a slow
    // loop uploads nothing most frames. Eviction is unchanged: an entry dies with its material,
    // and its slot zeroes back to identity ([`crate::mat_anim_table`]'s free law).
    let gseq_now = f64::from(now);
    uv_reg.0.retain(|id, entry| {
        // **Alive means either half holds it** (decision 2038): a material still parked by
        // `model_render::lazy` — built, handle reserved, nothing view-visible bound to it yet —
        // is not in the store, and `Assets::contains` alone reads that as death. Evicting there
        // would drop the entry a spawn just made and never rebuild it, because registration
        // happens once, at spawn.
        if !crate::model_render::lazy::holds(&materials, *id) {
            table.free(entry.slot);
            if let Some(a) = &entry.affine {
                table.free(a.slot);
            }
            return false;
        }
        // **The effect lane skips the draw gate.** That gate's argument is a material nothing
        // draws costing a per-frame rebuild for ever, on a population that grows monotonically
        // across a map session (B131). An effect instance is one of a handful and lives about a
        // second, so there is nothing to ratchet — while making it obey the gate would mean
        // threading [`AnimMatPart`] onto every fx part, card and decal, and a part that missed the
        // marker would silently freeze mid-scroll. It is also what keeps the lane running inside a
        // deterministic capture (`fxview`).
        let fx = entry.instance_lane();
        if (fx || drawn.contains(id)) && (!frozen || fx) {
            let playing = host_seq(&hosts, entry.host());
            table.set(entry.slot, entry.delta(now, gseq_now, playing));
            // …and the rotation/scale row beside it, for the effect models that turn or scale
            // their UVs rather than only sliding them (2282).
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

/// **The price-this-system knob** (`WOW_MATANIM_DUTY=<start_s>:<period_s>`, decision 0785): alternate
/// [`tick_anim_materials`] off/on every `period` seconds from `start`. Park at one pin, square-wave
/// it, and the difference between the ON and OFF buckets **is** this system's per-frame cost, with
/// residency, scene, entity count and camera all held identical — a measurement, not an argument.
/// It is how B131's ratchet was priced (+9.85 ms/frame over 174 resident entries) and how the draw
/// gate above was then shown to remove it (+0.22 ms). Kept, not deleted: the ~10 ms-floor ledger
/// (0729's residuals) has more per-frame systems queued for exactly this treatment.
///
/// A **square wave** rather than one flip, because this machine's frame cost drifts on its own —
/// a single before/after pair cannot tell the drift from the signal, and both legs of the verifying
/// run climbed ~10 ms while their ON/OFF difference stayed flat.
///
/// **`Time<Real>`, deliberately:** virtual time is clamped to `max_delta` (250 ms), so on a leg that
/// hitches it lags real time badly. That is what smeared the first attempt at this measurement into
/// nonsense — the probe-chat schedule reads virtual time, so once the leg hitched its hops drifted
/// 40 s → 75 s apart and windows labelled "parked, ticks off" in fact held a teleport and live ticks.
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

/// **`WOW_MATANIM_LIVE=1` — run the shared lanes inside a deterministic capture too.**
///
/// The freeze above is what makes a golden frame reproducible, and it is right for every scenario
/// in the sweep. It also means `fxview` — the fixture whose whole job is to age a subject — cannot
/// show a **unit's or GameObject's** texture transform at all, because those are shared-clock
/// entries (decision 2295); the effect lane is exempt only because its clock is one instance's
/// frame-stepped player. Without this knob the A/B for "does this creature's skin scroll" does not
/// exist, and the answer would have to come from the director's screen, which is not what §7 means
/// by leaving the look to them.
///
/// Off by default, read once, and no golden scenario sets it — so the sweep is untouched.
fn matanim_live() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WOW_MATANIM_LIVE").is_ok_and(|v| v != "0"))
}

/// The **tint-animated materials** registry — the M2Color-RGB twin of [`UvAnimMaterials`]: each
/// batch material whose colour track animates (the vertex bake is skipped for those —
/// `benilla-formats` `m2_batches`), keyed by material asset id. [`tick_anim_materials`]
/// re-samples the tint into the material's `tint` uniform on the same shared clock, for the drawn
/// (the same recorded seq-band phase divergence as the UV scroll — invisible for a placed
/// doodad's ambient loop). Spell-effect instances need real per-instance phase instead (one cast
/// = one 0.9 s pulse), so the effect lane clones its materials and ticks them on the instance
/// clock (`entities::spell_fx`), never through this registry.
/// [`UvLoop`]'s RGB twin, on the same rule and for the same reason — `uvslotscan` finds the tint
/// channel pinned to slot 0 by the same line, and `Spells\\Deterrence_State_Base.m2` tinting
/// **red→blue in Stand and green→red in Hold**, where the pin renders a *wrong colour* rather than
/// a frozen one.
pub enum TintLoop {
    Shared(std::sync::Arc<benilla_formats::RgbAnim>),
    PerSeq {
        seqs: std::sync::Arc<benilla_formats::SeqLoops<[f32; 3]>>,
        host: Entity,
    },
}

/// One registered tint material — [`UvAnimEntry`]'s RGB twin (seed = the built `tint.xyz`).
pub struct TintAnimEntry {
    pub anim: TintLoop,
    pub slot: u16,
    pub seed: [f32; 3],
}

impl TintAnimEntry {
    /// [`UvAnimEntry::delta`]'s RGB twin (1/255 quantization, the display's own step). The
    /// per-placement lane's identity is WHITE — the tint is a multiplier, so an unresolvable host
    /// must leave the batch at its built seed, not black it out.
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

/// [`register_uv`]'s tint twin: slot into `anim_slots.y`, seed from the built `tint.xyz`.
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
    // The parked half, exactly as [`register_uv`] reaches it (decision 2038).
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

/// **The animated-material probe** (`WOW_MATANIM_PROBE=<secs>`): once, `secs` after boot, print
/// one line per live [`AnimMatPart`] — the parts whose material can be a [`UvAnimMaterials`] /
/// [`TintAnimMaterials`] key — with everything that decides whether it actually moves on screen:
///
/// ```text
/// matanim  <model>#<uid>  mat <asset id>[ FAR][ PARKED]  anim_slots (1, 0, 0)  vis Inherited drawn 1
///   uv slot 1 seed (+0.0000,+0.0000) row (+0.0000,-0.2122) live (+0.0000,-0.2122)
///   shared period 9.967s loop (+0.0000,+0.0000) (+0.0000,-0.2542) (+0.0000,-0.5083) (+0.0000,-0.7542)
/// ```
/// (one line per part; wrapped here)
///
/// Four questions, one line each, because a frozen scroll can fail at any of them and the pixels
/// say the same thing every time: is the batch **registered** at all (a missing line is a batch
/// whose loop never reached the registry — an exhausted table, a period-0 constant, a lane that
/// forgot to register); does its **loop** actually move (the four quarter-phase samples — a
/// flat row here is an asset that scrolls nowhere, not a renderer fault); is the **table row**
/// live (the value the shader will read — flat while the loop moves means the tick's draw gate
/// never picked this material up, which is 1375's failure mode); and is the part **drawn** (the
/// gate's own input).
///
/// It samples the loop itself rather than watching the row over time, so it is a **one-shot that
/// works in a deterministic capture** — where [`tick_anim_materials`]'s shared lanes are skipped
/// and their rows stay zero by design (the effect lane keeps running there; decision 2282). In a
/// live run the `row` column is the tick's real output and the two halves can be compared
/// directly.
///
/// A trailing block of `(unmarked)` rows names every registry entry no [`AnimMatPart`] accounts
/// for: the spell-effect lane, which is off the draw gate and therefore off the marker, plus any
/// entry whose part has gone — the readout that says a registered clone exists at all.
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
    // Not latched while the world is still empty: a capture holds the game clock at 0 while the
    // scene builds, so "elapsed >= at" alone would fire the probe on frame one, before a single
    // doodad had spawned, and report nothing.
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
    // One UV entry as a row — hoisted out of the part loop so the unvisited pass below prints
    // exactly the same columns for an entry NO marked part accounts for.
    let uv_row = |e: &UvAnimEntry| {
        let playing = host_seq(&hosts, e.host());
        // The per-placement lane has one loop per sequence, so its phase sweep is meaningless
        // without a play head — it prints the head instead, and samples on the shared clock.
        let (lane, period) = match &e.anim {
            UvLoop::Shared(a) => {
                // `None` = a rotate-/scale-only transform: the translation row is real and stays
                // at zero, and the period the sweep needs is the AFFINE channel's, not this one's.
                let p = a.as_ref().map_or(0.0, |a| a.period);
                (format!("shared period {p:.3}s"), p)
            }
            UvLoop::PerSeq { .. } => (format!("per-seq playing {playing:?}"), 0.0),
            // The effect lane HAS a resolved loop (the clip it opened on), so unlike the
            // per-placement lane its sweep is meaningful — read across the instance's own
            // clip, from attach, beside the age that says where in it this frame sits.
            UvLoop::Instance { attached_at, .. } => (
                format!(
                    "per-instance age {:.3}s playing {playing:?}",
                    now - attached_at
                ),
                e.resolved(playing).map_or(0.0, |l| l.period),
            ),
        };
        // Four quarter-phase samples of the loop itself: a flat set is an asset that scrolls
        // nowhere, whatever the row says. The phase has to move the clock the entry actually
        // reads — for the shared lane that is `now`, for a hosted one the PLAY HEAD, which is why
        // the sweep substitutes the phase into `playing` rather than only into `now` (an entry
        // whose host supplies a band clock ignores `now` entirely, and the sweep read flat).
        let phases: Vec<String> = (0u8..4)
            .map(|i| {
                let t = period * f32::from(i) / 4.0;
                let head = playing.map(|(seq, _)| (seq, t));
                let d = e.delta(e.phase_now(t), f64::from(t), head);
                format!("({:+.4},{:+.4})", d[0], d[1])
            })
            .collect();
        let live = e.delta(now, gseq_now, playing);
        // The affine half, when the transform has one: `[cos − 1, sin, sx − 1, sy − 1]`, so all
        // zeros IS the identity and a flat row there says the turn/scale never reached the shader.
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
        // A far-classified part carries the TWIN's id; the registry is always keyed by the near
        // one — the same fold `tick_anim_materials`' draw scan makes (1375).
        let far = twins.near_of(mat.id());
        let id = far.unwrap_or(mat.id());
        let label = obj.map_or_else(|| "-".to_string(), |o| format!("{}#{}", o.label, o.id));
        // Through the parked half (`lazy::with_material`), or the readout would report every
        // not-yet-drawn material's stamp as a zero it never had — the deferral confusion this
        // probe exists to resolve (decision 2038). `parked` says which half answered.
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
    // **The entries no marked part accounts for.** The spell-effect lane is exactly this
    // population by design — its parts carry no [`AnimMatPart`] because they skip the draw gate
    // (`tick_anim_materials`) — and so is every genuine leak: a registered material whose part is
    // gone, or whose marker was never inserted. Without this pass the probe could not see the
    // claw trail at all, which is the one thing decision 2282 built it to read.
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

/// One animated part as [`matanim_probe`] reads it: the material it is bound to (the far twin's
/// id while it is far-classified), the two halves of the draw verdict the tick's gate turns on,
/// and the placement that names it. A tuple alias because the inline form trips the workspace
/// gate's `type_complexity`, the same shape `debug_panel::inspect`'s readouts take.
type ProbeReadout = (
    &'static MeshMaterial3d<benilla_assets::materials::WowModelMaterial>,
    &'static Visibility,
    Option<&'static bevy::camera::visibility::ViewVisibility>,
    Option<&'static crate::interact::WorldObject>,
);

/// `WOW_MATANIM_PROBE` — unset is off; bare (or unparseable) fires at 20 s. Read once
/// ([`matanim_off`]'s pattern): the system runs every frame to check its own latch.
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

    /// An effect-lane entry over one shared translation loop, attached at `attached_at`.
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

    /// A rotation-only loop, as `G_ScryingBowl`'s water and `G_ScourgeRuneCircleCrystal`'s rune
    /// ring author one: a quarter turn about Z over two seconds, keyed in file slot 0 only.
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

    /// **`animates()` is the one predicate, and it is not `uv_anim.is_some()`** (decision 2295).
    /// Two entity batches in the shipped corpus rotate and never translate, so the channel a
    /// reader would check first answers `None` for them; and a translation keyed to one constant
    /// value is the material's seed, not a loop to sample (1375).
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

    /// **The affine half runs on whichever clock its own lane runs on** (decision 2295 widening
    /// 2282's). A shared entity entry has no host and no attach time, so its rotation reads the
    /// free-running scene clock — which is what makes a rune ring on a shared, deduped material
    /// turn at all.
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
        // No translation channel at all: the row it still owns stays at the zero it means.
        assert_eq!(e.delta(1.0, 1.0, None), [0.0, 0.0, 0.0, 0.0]);
        // Half-way through the turn, and the row is `[cos − 1, sin, sx − 1, sy − 1]` of the
        // **raw, unnormalised** lerped quaternion — which is the reference's own arithmetic, not
        // an oversight: `0x713ea0` lerps each component and `0x7bddb0` consumes the result as is,
        // so between two keys `|q| < 1` and the 2×2 is a rotation with a slight shrink — a
        // re-implementation that slerps, or normalises before building the matrix, diverges from
        // the reference between keys.
        // Here `q = (0, 0, 0.35355, 0.85355)`, so `c = 1 − 2z² = 0.75` and `s = 2zw = 0.60355` —
        // NOT the 0.7071/0.7071 a normalised 45° would give. Asserted exactly so a future
        // "obvious fix" to normalise fails here.
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
        // …and the clock really is `now`: a different scene time gives a different angle.
        let later = e.affine(1.5, 1.5, None).expect("the affine row").1;
        assert!(
            (later[1] - row[1]).abs() > 1e-2,
            "the shared lane's affine must move with the scene clock: {row:?} vs {later:?}"
        );
    }

    /// **The effect lane reads its INSTANCE's play head, not the scene clock** (decision 2282).
    /// The band clock is the clip time the host's `AnimationPlayer` reports, so two casts a minute
    /// apart are at the same point of their own scroll — the property a shared-material lane
    /// structurally cannot have, and the one Swipe's one-shot reveal rests on.
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

    /// With no player to ask — the frame before an effect's rig arms, or a model that never arms
    /// one — the band clock degrades to the instance's own AGE, which is `MatAnim`'s degrade next
    /// door and not the free-running scene clock a shared entry would read.
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

    /// The **affine** half (2019's encoding, zero = identity): sampled on the same clocks as the
    /// translation beside it, and absent — never a zeroed row — when the transform only slides.
    /// `Spells\GroundingTotem_Impact.m2` is the scale-only case this stands for.
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
        // Half-way: scale 0.625, encoded as `sx − 1`; rotation untouched, so `cos − 1` and `sin`
        // are the identity's zeros.
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

    /// The delta law (decision 1381): a row is the quantized live sample minus the BUILT seed,
    /// so the shader's `seed + row` shows exactly the number the old asset-mutating lane wrote.
    /// At t = 0 that is the quantized seed — the same value the old tick's first frame produced.
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

    /// **B98** (decision 1408): the per-placement lane samples the slot the placement's own host is
    /// playing — not slot 0, and not a shared clock.
    ///
    /// The BRM lava bubble's shape, exactly: slot 0 bakes to nothing (a dead hold) and slot 1
    /// carries the flipbook. Reading slot 0, as the shared registry must, is the frozen sprite the
    /// report named; reading the host's live slot is the fix. The unresolved case — a host
    /// despawned mid-frame — must land on the UV identity, i.e. the built seed, never a jump.
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
        // On slot 0 there is no loop at all — the identity, i.e. the built seed.
        assert_eq!(entry.delta(0.0, 0.0, Some((0, 3.0))), [0.0; 4]);
        // …and an unresolvable host degrades to the same identity, never to a shared-clock sample.
        assert_eq!(entry.delta(99.0, 99.0, None), [0.0; 4]);
    }

    /// The tint twin, same law at the display's 1/255 step.
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
