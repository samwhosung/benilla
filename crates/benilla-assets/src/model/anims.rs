//! The playable-animation surface: the types + resolution logic the game queries at runtime
//! ([`AnimClip`], [`ModelAnimations`], [`ResolvedAnim`], and `ModelAnimations`'s `find`/
//! `pick_variation`/`resolve`) — kept separate from the bake-side builders in [`super`] (meshes,
//! skeleton, attachments, masks/subtree helpers, grip poses, billboard info).

use benilla_formats::AnimDataCatalog;
pub use benilla_formats::PlayableAnim;
use bevy::animation::graph::{AnimationGraph, AnimationNodeIndex};
use bevy::prelude::*;

/// One **event keyframe** on a clip — the baked twin of [`benilla_formats::AnimEvent`], exactly as
/// [`super::ModelMarker`] is the baked twin of `EventMarker`, and carrying the same bone-local
/// Bevy-space offset convention.
///
/// It carries **where** as well as **when**, because the reference's event kernel does: `0x719370`
/// computes `placementMatrix · (boneMatrix[event.bone] · event.position)` and snapshots it by
/// value into the deferred callback record every dispatcher reads. A consumer that plays at the
/// model root is using the placement alone, which is the same point only where the record sits at
/// the origin — and corpus-wide it does not: 149 of 244 `$DSL` records are off-origin, out to
/// 67.6 yd (`Maraudon_Waterfall01.m2`), and every player model's six `$CSD` records ride the head
/// (`benilla-extract eventmarkerscan`).
///
/// **Two points, because there are two cases and both are exact.** [`Self::offset`] is bone-local
/// and wants the joint's live global ([`benilla_world::rig_anim::RigPose::posed_point`]);
/// [`Self::point`] is the model-space rest position and wants the model's own world frame. The
/// second is not an approximation of the first: with no keys a bone's matrix composes to the
/// identity, so `placement · position` *is* the kernel's quantity — which is the whole placed-doodad
/// population, where 0 of 244 `$DSL` records ride a bone any sequence keys.
#[derive(Debug, Clone, Copy)]
pub struct ClipEvent {
    /// Seconds from the clip start.
    pub time: f32,
    /// The forward-stored 4CC (`*b"$SND"`).
    pub ident: [u8; 4],
    /// The payload — a SoundEntries id for `$SND`/`$DSL`/`$DSO`, else 0.
    pub data: u32,
    /// The bone this record rides.
    pub bone: u16,
    /// The authored point **relative to [`Self::bone`]'s pivot**, Bevy space — compose it with the
    /// joint's live global.
    pub offset: Vec3,
    /// The authored point in the model's own space, Bevy space — compose it with the model's world
    /// frame. Equals the kernel's quantity whenever the bone chain is unanimated.
    pub point: Vec3,
}

/// One animation in a model's [`ModelAnimations`] graph: its `AnimationData.dbc` id,
/// the graph node an `AnimationPlayer` plays to run it, and whether it loops.
#[derive(Clone)]
pub struct AnimClip {
    pub anim_id: u16,
    /// The sequence's **file slot** ([`benilla_formats::ModelAnimation::seq_index`]) — not this
    /// clip's index in [`ModelAnimations::clips`]. The key into a batch's per-sequence
    /// material-alpha loops (`benilla_formats::AlphaAnim::seq`), so the lane that knows which clip
    /// a unit is playing can sample the batch visibility that sequence authored.
    pub seq_index: usize,
    pub node: AnimationNodeIndex,
    pub looping: bool,
    /// Clip length in seconds — used to seek a one-shot clip (Death) to its end pose, e.g. for a unit
    /// that streamed in already dead so it shows the settled corpse instead of replaying the collapse.
    pub duration: f32,
    /// The sequence's authored design movement speed (`ModelAnimation::move_speed`, yd/s) — the
    /// denominator the locomotion playback-rate scaler divides the unit's live speed by, so a fast
    /// mover cycles its legs proportionally faster. `0.0` for a non-locomotion clip (left at rate 1×).
    ///
    /// **Signed, and the sign is load-bearing — never `abs()` it.** A backwards gait is authored
    /// NEGATIVE (`RidingKodo.m2` seq 14, WalkBackwards: `-2.5`), and the client's rate guard is a
    /// strict `divisor > 0` (`0x5fe2f0`). So an authored
    /// backwards clip plays at a flat 1×, while a model that has *no* WalkBackwards and falls back
    /// to Walk (`+2.5`) gets speed-scaled. That asymmetry is the reference's behaviour; taking the
    /// magnitude here — an inviting-looking tidy-up — inverts it on every model authoring a reverse
    /// gait.
    pub move_speed: f32,
    /// The sequence's blend-in time (`ModelAnimation::blend_time`, seconds) — the cross-fade duration the
    /// spawn site uses when transitioning *into* this clip, so a gait change eases instead of snapping.
    pub blend_time: f32,
    /// The sequence's bounds-sphere centre, **Bevy model space** (the raw `M2Sequence` CAaBox centre
    /// mapped like the mesh vertices, so it shares the rendered parts' frame). With
    /// [`Self::bounds_radius`], the mouse-pick **broad phase**: the real client tests the cursor ray
    /// against the *current* animation's sphere — world-placed + scaled, no pad — before the posed
    /// per-triangle narrow test (`0x7089c0`).
    pub bounds_center: Vec3,
    /// The sequence's bounds-sphere radius (model-local yards). `0.0` ⇒ unauthored (callers fall back).
    pub bounds_radius: f32,
    /// The sequence's CAaBox, **Bevy model space** (componentwise min/max of the WoW-space corners
    /// mapped like the mesh vertices). The unit **blob shadow**'s projection box: the real client
    /// sizes the shadow from the *current* animation's box — clamped into ±5 per axis, scaled by
    /// the world matrix, yaw-rotated then axis-aligned (`0x711a20` + the `0x6d7920` corner build).
    /// `(Vec3::ZERO, Vec3::ZERO)` ⇒ unauthored.
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// The sequence's event keyframes (`$SND`/footsteps/`$CSS`… — decision 0070 slice 3), sorted by
    /// time, seconds on the clip clock. Shared (`Arc`) so cloning a clip stays cheap; empty for the
    /// overwhelming majority of sequences.
    pub events: std::sync::Arc<[ClipEvent]>,
    /// Per-arm masked variants `(right, left)` of this clip — graph nodes that animate **only** the
    /// right/left arm subtree (shoulder keybone down), for the client's per-slot one-shots played
    /// *over* the gait (the draw/stow: mainhand ceremony on the right arm, offhand on the left,
    /// legs untouched). Built only for the sheath family (ids 89/90) on models whose hand
    /// attachments + shoulder keybones resolve; `None` otherwise.
    pub arm_nodes: Option<(AnimationNodeIndex, AnimationNodeIndex)>,
    /// The **upper-body masked variant** of this clip: a graph node that animates only the
    /// SpineLow-subtree ([`upper_subtree_root`] — keyBoneLookup[4]/[6], the legs excluded), for the
    /// one-shot route's masked destination (a swing/emote played *over* a live base:
    /// legs keep running/sitting/jumping while the torso swings or emotes). The general form of the
    /// per-arm [`Self::arm_nodes`] ceremony machinery, one subtree wider. `None` for a model with no
    /// split key-bone (the −1 sentinel) — the route falls back to full-body on the base node.
    pub upper_node: Option<AnimationNodeIndex>,
    /// This variation's weight in the per-play roll (`M2Sequence.frequency` @+0x14): sequences
    /// sharing an `anim_id` are **variations** (the alternating 1H swing arcs, the wound recoils),
    /// and the real client picks one per one-shot play by a `_rand()`-weighted walk of the chain
    /// (op4 `0x71248a..`) — [`ModelAnimations::pick_variation`].
    pub frequency: u16,
    /// The sequence's `(minReplay, maxReplay)` range (`M2Sequence` +0x18/+0x1c): the client rolls a
    /// play count `R = max(1, min + ((rand()·(max−min)) >> 15))` at every arm and multiplies it into
    /// the play window (`windowHi = now + span·R`), so a clamp-flag one-shot runs `R` times before
    /// freezing. Benilla expresses that as a `RepeatAnimation::Count(R)` on the one-shot play.
    /// `(0, 0)` — the overwhelming majority — rolls to `R = 1`.
    ///
    /// **A loop-flag sequence does NOT ignore it** (this doc said so, and decision 0768 corrected
    /// it): the window still governs, and for a *placed doodad* its expiry is what fires the re-arm
    /// that rolls a fresh variation — `(0, 0)` ⇒ `R = 1` ⇒ a re-roll every single loop. That is the
    /// whole mechanism behind the Blasted Lands lightning wandering instead of strobing from one
    /// fixed spot.
    pub replay: (u32, u32),
    /// Whether this clip actually **poses bones** — some bone track produced a curve. `false` for a
    /// sequence whose bones all hold bind pose: it still exists as a clip (playing it is how the
    /// instance's sequence clock names a slot and advances a time, which the emitter tracks, the
    /// material loops and the GameObject arm all read), but skinning to it can only ever reproduce
    /// the static mesh. So a rig is spawned for a *posing* clip, never for a clock-only one —
    /// decision 0941's split between the clock (free) and the rig (a palette slot, a skinned twin).
    pub poses_bones: bool,
}

/// A model's animations as a playable bundle: one shared `AnimationGraph` holding every
/// sequence's clip, plus the per-sequence node/id/loop info. Built once per M2 asset and shared across
/// instances; each creature instance drives this graph with its own `AnimationPlayer`. Also attached to
/// the creature **as a component**, so the model bench can scrub `clips` and gameplay (Milestone C) can
/// select by `AnimationData.dbc` id (Stand now, walk/run later) — both query it on the entity.
#[derive(Clone, Component)]
pub struct ModelAnimations {
    pub graph: Handle<AnimationGraph>,
    pub clips: Vec<AnimClip>,
    /// The **`HandsClosed` finger-grip masked nodes** `(right, left)` — graph nodes that animate only one
    /// hand's finger key-bone subtrees ([`finger_subtree_roots`]) with the model's `HandsClosed`
    /// (AnimationData 15) pose, played as a persistent weighted overlay over the gait while a weapon is
    /// held in that hand (`0x479660`/`0x60b590`: the fingers curl for a held weapon; a
    /// forearm-mounted shield or an empty hand stays open). `None` per hand for a model with no finger
    /// key-bones or no `HandsClosed` clip (beasts, props — they never grip).
    pub hand_close: [Option<AnimationNodeIndex>; 2],
    /// The model's baked **PlayableAnimationLookup** (M2 header `+0x2c/+0x30`, see
    /// [`PlayableAnim`]): row `i` is the model's own precomputed substitute for requested
    /// `AnimationData.dbc` id `i`. Empty for a model whose header carries no table (a boneless
    /// doodad-shaped cube fallback, a malformed M2) — [`Self::resolve`] then degrades to identity.
    pub playable_animation_lookup: Vec<PlayableAnim>,
    /// The model's **AnimationLookup** (M2 header `+0x24`/`+0x28`): `AnimationData.dbc` id → the
    /// model's first sequence slot for it, `0xffff` where it authors none. Kept beside the playable
    /// table because it is a *different question* — "does this model own id X at all", the
    /// reference's `0x711960` — and the GameObject arm's missing-sequence remap branches on it four
    /// times (`0x5f3930`). See [`Self::owns`].
    pub animation_lookup: Vec<u16>,
    /// The model's **global-sequence bone channels** baked to Bevy space (the eye-blink eyelid scale and
    /// kin): free-clock loops the runtime samples independently of the playing animation and composes
    /// over the sequence pose (see [`super::GlobalBone`]). Empty for a model with no global-sequence
    /// tracks.
    pub global_bones: Vec<super::GlobalBone>,
    /// Index into [`Self::clips`] of the model's **loader-idle seed** — the sequence the client's
    /// M2-instance loader arms once at load and plays forever (`0x71019b`): **animation id 0
    /// ("Stand")**, resolved through the model's own `playableAnimationLookup`. NOT the
    /// file-order-first sequence. The two coincide for almost every model and diverge
    /// exactly on one whose first sequence is a Spawn: `DuelingFlag.m2` is Spawn/Stand/Despawn,
    /// and arming file-order-0 looped its Spawn band, leaving the duel flag hanging 9 yards in the
    /// air on a 3.3 s cycle.
    ///
    /// `None` when the resolved idle leaves every bone at **bind pose** — looping that really does
    /// render identically to the static mesh, so the spawn site skips the skin + player (the
    /// content gate, decision 0130; the ~90% of placed doodads `doodadscan` measured). A *constant
    /// but non-rest* pose does NOT qualify for that skip — see `m2::idle_pose_differs`.
    pub first_seq: Option<usize>,
    /// The direct pose evaluator's baked data: the pose twin of every clip above,
    /// the node → (clip, mask) table, and the per-bone mask bits — filled by the same code that
    /// builds [`Self::graph`], so it mirrors the graph by construction. `Arc`: the component is
    /// cloned per instance; the bake is shared.
    pub pose: std::sync::Arc<super::PoseSource>,
}

/// A requested `AnimationData.dbc` id resolved to what a specific model actually plays (decision
/// 0082, `0x711bf0`) — see [`ModelAnimations::resolve`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedAnim {
    /// The `AnimationData.dbc` id to look up via [`ModelAnimations::find`].
    pub id: u16,
    /// PATH 1's high16 direction/variant playback code (`0` from PATH 2 / the no-table degrade, which
    /// have no such code of their own). **Plumbed through, not applied**: decision 0082 flags the
    /// exact playback consumption (`~0x7126d2`) as untraced — no consumer reads this yet.
    pub dir_flags: u16,
}

impl ModelAnimations {
    /// The clip for an `AnimationData.dbc` id (0 Stand, 4 Walk, 5 Run, …), or `None` if this model has
    /// no sequence with that id (e.g. a static-Stand creature has no animated id-0 clip — the caller
    /// then falls back to Stand or to bind pose rather than to an arbitrary clip). Returns the id's
    /// **head variation** — the stable identity callers compare/loop on; a one-shot play that should
    /// alternate variations goes through [`Self::pick_variation`] instead.
    pub fn find(&self, anim_id: u16) -> Option<&AnimClip> {
        self.clips.iter().find(|c| c.anim_id == anim_id)
    }

    /// **Does the model author `anim_id`?** — the reference's `0x711960`: a bounds-checked read of
    /// [`Self::animation_lookup`] against the `0xffff` sentinel. Deliberately NOT `find(..).is_some()`:
    /// `clips` drops sequences we can't build (zero-duration ones among them), so a clip scan can
    /// answer "no" where the reference answers "yes" and takes a different branch. Use this to decide
    /// what the reference would *request*; use [`Self::find`] to get something we can actually play.
    pub fn owns(&self, anim_id: u16) -> bool {
        self.animation_lookup
            .get(anim_id as usize)
            .is_some_and(|&slot| slot != 0xffff)
    }

    /// The clip a free/effect model instance runs: the caller's `preferred` id when the model has
    /// it (the thrown-weapon missile asks for InFlight), else the reference's **model-load
    /// bootstrap** — animation id **0 (`Stand`)**, variation 0 (`0x710153`–`0x71019b`:
    /// `0x711bf0(reqId = 0)` → `0x711960` → op4 with an explicit `push 0x0` variation), falling
    /// back to the first sequence record's own id only when the model authors no `Stand` (the
    /// guarded `*(dword*)&sequences[0]` at `0x710181`).
    ///
    /// **Not the file-order-first slot**, which is what this used to be: the two differ on 583
    /// models corpus-wide (`benilla-extract fxlifescan`), and arming slot 0 hands
    /// `Spells\LightningShield_State_Base` its `Decay` sequence from frame one. It is the same
    /// correction `first_seq` already carries for the placed-doodad lane (the duel flag) —
    /// but taken *without* that field's bind-pose content gate, because an effect's emitters read
    /// the posed joints even when the mesh would render identically.
    ///
    /// One selector so the rig (`entities::spell_fx::arm_effect_rig`) and its per-sequence riders
    /// agree on which sequence is playing.
    pub fn preferred_clip(&self, preferred: Option<u16>) -> Option<&AnimClip> {
        preferred
            .and_then(|id| self.find(id))
            .or_else(|| self.find(0))
            .or_else(|| self.clips.first())
    }

    /// The **FILE sequence slot** the loader-idle seed arms — [`Self::first_seq`]'s selection taken
    /// *without* its bind-pose content gate, and expressed as the slot the per-sequence bakes are
    /// keyed on rather than as a `clips` index.
    ///
    /// The two fields answer different questions and must not be conflated. The
    /// reference arms this sequence on **every** M2 instance the moment it goes LIVE (`0x70ebd0`'s
    /// tail — no instance in the client ever exists with nothing armed), so "which sequence is this
    /// model playing" always has an answer. `first_seq` answers the narrower *rendering* question
    /// "is it worth building a skin + `AnimationPlayer` for it", and says `None` when looping the
    /// idle would render identically to the static mesh. That skip is sound for the mesh — and
    /// silently wrong for every consumer keyed on the sequence *identity*, which then degraded to
    /// file slot 0 (`EmitTiming::idx`/`EmitParams::sample`/`AlphaAnim::seq` all resolve `None` that
    /// way). Slot 0 is only accidentally the idle one: on the Spawn/Stand/Despawn GameObjects
    /// (`DuelingFlag`-shaped — the battlefield banners, the goobers, `ArenaFlag`) it is the **Spawn**
    /// flourish, whose emitters then pour for ever at its opening frame.
    ///
    /// `None` only for a model with no buildable clip at all (the `GlobalSeqOnly` tier), where there
    /// is no sequence to name.
    pub fn idle_seq(&self) -> Option<usize> {
        self.idle_clip().map(|c| c.seq_index)
    }

    /// The loader-idle seed's **clip** — [`Self::idle_seq`]'s selection, before it is reduced to a
    /// slot number. What a spawn site plays to put an instance on that sequence: since every
    /// sequence builds a clip, a model whose bones never move still has one to arm,
    /// and arming it is what gives its emitters/material loops a running clock instead of a frozen
    /// slot-0 read. Distinct from [`Self::first_seq`], which stays the *rendering* question.
    pub fn idle_clip(&self) -> Option<&AnimClip> {
        let idle_id = self
            .playable_animation_lookup
            .first()
            .map_or(0, |p| p.resolved_id);
        self.clips
            .iter()
            .find(|c| c.anim_id == idle_id)
            .or_else(|| self.clips.first())
    }

    /// Pick a **variation** of `anim_id` for one play, given a fresh `roll` (the client's `_rand()`,
    /// `0..0x7fff`): the weighted walk (op4 `0x7121a0` with variationIdx −1) — `roll < frequency`
    /// picks the current node, else `roll -= frequency` and advance; chain exhaustion falls back to
    /// the **head** (the client leaves the resolved seqIndex untouched on the exhaust path — `edi`
    /// set at `0x71247a`, only the win path overwrites it at `0x7124e9`; correcting decision 0114's
    /// clamp-to-last).
    /// Variations are the sequences sharing `anim_id`, taken in file order (retail exporters bake
    /// the `variationNext` chain contiguously and in order — the same assumption [`Self::find`]'s
    /// head = variation 0 already rests on). `None` iff the model has no sequence for `anim_id`.
    pub fn pick_variation(&self, anim_id: u16, roll: u16) -> Option<&AnimClip> {
        let mut roll = i32::from(roll);
        let mut head = None;
        for c in self.clips.iter().filter(|c| c.anim_id == anim_id) {
            head.get_or_insert(c);
            if roll < i32::from(c.frequency) {
                return Some(c);
            }
            roll -= i32::from(c.frequency);
        }
        head
    }

    /// Resolve a *requested* `AnimationData.dbc` id to the id this model actually plays (decision
    /// 0082 — the two-path `0x711bf0` resolver the real client runs before every sequence lookup,
    /// ahead of [`Self::find`]):
    /// - **PATH 1** (`requested < playable_animation_lookup.len()`): the model's own baked table —
    ///   one dword read, already the precomputed `AnimationData.dbc` Fallback-column walk's result for
    ///   THIS model's actual sequence set (a chicken lacking Attack2H bakes `[18] = 16`).
    /// - **PATH 2** (`requested` ≥ the table length, table non-empty): the guarded `AnimationData.dbc`
    ///   Fallback-column walk over `catalog` — reachable only for the 5 ids ≥ 203 on a normal retail
    ///   asset set (`nPlayableAnimationLookup` is a fixed 203; `AnimationData.dbc` has 208 rows).
    /// - **No table at all** (`playable_animation_lookup` empty — a cube fallback / malformed M2):
    ///   degrades to identity; [`Self::find`] then does its own direct lookup (today's behavior).
    pub fn resolve(&self, requested: u16, catalog: &AnimDataCatalog) -> ResolvedAnim {
        if let Some(row) = self.playable_animation_lookup.get(requested as usize) {
            return ResolvedAnim {
                id: row.resolved_id,
                dir_flags: row.dir_flags,
            };
        }
        if self.playable_animation_lookup.is_empty() {
            return ResolvedAnim {
                id: requested,
                dir_flags: 0,
            };
        }
        self.resolve_path2(requested, catalog)
    }

    /// PATH 2 — the `AnimationData.dbc` Fallback-column walk (`0x711bf0` at `0x711c1f`): follow
    /// `catalog.fallback` from `requested` until this model actually has a sequence for the current id
    /// (mirrors the real walk's `animationLookup[eax] != 0xffff` success test via [`Self::find`]),
    /// guarded by a 208-entry visited set (`AnimationData.dbc`'s row count — the reference's cycle
    /// guard). `catalog.fallback` already collapses the walk's NULL-row guard, self-fallback
    /// termination, and "Fallback is 0 (Stand)" into a single `None` — each of those is a dead end that
    /// leads to the same exhaustion default, so folding them costs nothing here. Exhaustion → Stand(0),
    /// matching decision 0082's stated simplification (the real client's fuller default — the
    /// GameObject-rest pose 147, or the model's first sequence id — is a further-out RE gap no benilla
    /// id exercises: every id benilla requests resolves through PATH 1).
    fn resolve_path2(&self, requested: u16, catalog: &AnimDataCatalog) -> ResolvedAnim {
        const STAND: ResolvedAnim = ResolvedAnim {
            id: 0,
            dir_flags: 0,
        };
        let mut visited = [false; 208];
        let mut id = requested;
        loop {
            if self.find(id).is_some() {
                return ResolvedAnim { id, dir_flags: 0 };
            }
            match visited.get_mut(id as usize) {
                Some(seen) if !*seen => *seen = true,
                _ => return STAND, // revisited (cycle) or id ≥ 208 (NULL/max-id guard)
            }
            match catalog.fallback(id) {
                Some(next) => id = next,
                None => return STAND, // unknown row / self-fallback / already-Stand → exhaustion
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── ModelAnimations::resolve ──────────────────────────────────────────────

    use bevy::animation::graph::AnimationNodeIndex;

    fn test_clip(anim_id: u16) -> AnimClip {
        AnimClip {
            anim_id,
            seq_index: 0,
            node: AnimationNodeIndex::new(0),
            looping: true,
            duration: 1.0,
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

    fn test_anims(has_clips: &[u16], table: Vec<PlayableAnim>) -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: has_clips.iter().map(|&id| test_clip(id)).collect(),
            playable_animation_lookup: table,
            animation_lookup: Vec::new(),
            hand_close: [None, None],
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }

    fn playable(resolved_id: u16, dir_flags: u16) -> PlayableAnim {
        PlayableAnim {
            resolved_id,
            dir_flags,
        }
    }

    fn empty_catalog() -> AnimDataCatalog {
        AnimDataCatalog::from_rows([])
    }

    /// PATH 1: a table entry within range is read verbatim, low16 as the id and high16 as the dir
    /// code — the chicken-lacking-Attack2H shape (`[18] = {16, 0}`), model has clip 16 not 18.
    #[test]
    fn path1_reads_the_baked_table_entry() {
        let mut table = vec![playable(0, 0); 19];
        table[18] = playable(16, 0);
        let anims = test_anims(&[0, 16], table);
        let resolved = anims.resolve(18, &empty_catalog());
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 16,
                dir_flags: 0
            }
        );
    }

    /// PATH 1 also carries the dir-flags code — the real HumanMale example, read off the shipped
    /// asset (`playableAnimationLookup[6] = 0x00030001`): resolved id 1, dir code 3. PATH 1 never
    /// consults the catalog at all (an empty one is fine here).
    #[test]
    fn path1_carries_dir_flags_through_unapplied() {
        let mut table = vec![playable(0, 0); 7];
        table[6] = playable(1, 3);
        let anims = test_anims(&[0, 1], table);
        let resolved = anims.resolve(6, &empty_catalog());
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 1,
                dir_flags: 3
            }
        );
    }

    /// PATH 2: id ≥ the table length walks the DBC Fallback chain (via `catalog`) until the model
    /// actually has a sequence for the current id.
    #[test]
    fn path2_walks_the_dbc_fallback_past_the_table() {
        let cat = AnimDataCatalog::from_rows([
            (
                5,
                benilla_formats::AnimEntry {
                    weapon_flags: 0,
                    fallback: 4,
                },
            ),
            (
                4,
                benilla_formats::AnimEntry {
                    weapon_flags: 0,
                    fallback: 3,
                },
            ),
        ]);
        let anims = test_anims(&[0, 3], vec![playable(0, 0); 3]); // table covers only ids 0..2
        let resolved = anims.resolve(5, &cat);
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 3,
                dir_flags: 0
            }
        );
    }

    /// PATH 2's cycle guard: a Fallback loop (5→6→5→…) terminates at Stand(0) rather than spinning.
    #[test]
    fn path2_cycle_guard_terminates_at_stand() {
        let cat = AnimDataCatalog::from_rows([
            (
                5,
                benilla_formats::AnimEntry {
                    weapon_flags: 0,
                    fallback: 6,
                },
            ),
            (
                6,
                benilla_formats::AnimEntry {
                    weapon_flags: 0,
                    fallback: 5,
                },
            ),
        ]);
        let anims = test_anims(&[], vec![playable(0, 0); 3]); // model has neither 5 nor 6
        let resolved = anims.resolve(5, &cat);
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 0,
                dir_flags: 0
            }
        );
    }

    /// PATH 2's self-fallback / NULL-row termination: an id with no usable Fallback (an unknown row,
    /// or one whose Fallback is itself/0 — `AnimDataCatalog::fallback` already collapses all three to
    /// `None`) exhausts to Stand(0) on the first hop.
    #[test]
    fn path2_self_or_null_fallback_exhausts_to_stand() {
        let anims = test_anims(&[], vec![playable(0, 0); 3]);
        let resolved = anims.resolve(99, &empty_catalog()); // id 99 has no row at all
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 0,
                dir_flags: 0
            }
        );
    }

    /// PATH 2's NULL/max-id guard: an id at or past the 208-row `AnimationData.dbc` bound can't even
    /// be marked visited — it exhausts to Stand(0) immediately, not just when it recurs.
    #[test]
    fn path2_out_of_dbc_range_id_exhausts_to_stand() {
        let anims = test_anims(&[], vec![playable(0, 0); 3]);
        let resolved = anims.resolve(250, &empty_catalog());
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 0,
                dir_flags: 0
            }
        );
    }

    /// A model with no baked table at all (a cube fallback / malformed M2) degrades to identity —
    /// `resolve` hands the id straight back so the caller's own `find` does today's direct lookup.
    #[test]
    fn no_table_degrades_to_identity() {
        let anims = test_anims(&[0], Vec::new());
        let resolved = anims.resolve(42, &empty_catalog());
        assert_eq!(
            resolved,
            ResolvedAnim {
                id: 42,
                dir_flags: 0
            }
        );
    }

    // ── preferred_clip: the model-load bootstrap (`0x710153`–`0x71019b`) ─────────────────────

    /// The bootstrap arms **id 0 `Stand`**, not the file's first slot. `LightningShield_State`'s
    /// real shape: sequence 0 is its `Decay`(159), and arming file-order-first handed a freshly
    /// cast shield its fade-out from frame one.
    #[test]
    fn preferred_clip_arms_stand_over_the_file_order_first_slot() {
        let anims = test_anims(&[159, 0, 158], Vec::new());
        assert_eq!(anims.preferred_clip(None).map(|c| c.anim_id), Some(0));
    }

    /// The guarded fallback (`0x710181`): a model authoring no `Stand` at all — Cripple, Curse of
    /// Tongues, both Holy Word: Shields, whose slot 0 is `Hold` — plays that first record's own id.
    #[test]
    fn preferred_clip_falls_back_to_the_first_record_without_a_stand() {
        let anims = test_anims(&[158, 159], Vec::new());
        assert_eq!(anims.preferred_clip(None).map(|c| c.anim_id), Some(158));
    }

    /// A caller-named id still wins (the thrown-weapon missile's InFlight) — and when the model
    /// lacks it, the resolve falls through to the bootstrap rather than to slot 0.
    #[test]
    fn preferred_clip_honours_a_named_id_then_falls_to_stand() {
        let anims = test_anims(&[159, 0, 158], Vec::new());
        assert_eq!(
            anims.preferred_clip(Some(158)).map(|c| c.anim_id),
            Some(158)
        );
        assert_eq!(anims.preferred_clip(Some(42)).map(|c| c.anim_id), Some(0));
    }

    // ── idle_seq: the loader-idle FILE slot, split from the render content gate ──────────────

    /// `test_anims` in file order, with each clip's `seq_index` set to its slot — the axis the
    /// per-sequence bakes (`EmitTiming`/`EmitParams`/`AlphaAnim`) are keyed on.
    fn slotted(ids: &[u16], table: Vec<PlayableAnim>) -> ModelAnimations {
        let mut a = test_anims(ids, table);
        for (i, c) in a.clips.iter_mut().enumerate() {
            c.seq_index = i;
        }
        a
    }

    /// The Spawn/Stand/Despawn GameObject shape — `DuelingFlag`, the battlefield banners,
    /// `ArenaFlag`, the goobers. The idle slot is **1**, not 0: slot 0 is the Spawn flourish, whose
    /// emitters pour at its opening frame for ever if a consumer degrades to it.
    #[test]
    fn idle_seq_is_the_stand_slot_not_file_slot_zero() {
        let anims = slotted(&[145, 0, 157], Vec::new());
        assert_eq!(anims.idle_seq(), Some(1));
    }

    /// It is `first_seq`'s selection **without** the bind-pose content gate: the field stays `None`
    /// (looping a constant-pose idle renders identically to the static mesh, so the rig tier skips
    /// the arm) while the slot is still known. Conflating the two is the whole defect.
    #[test]
    fn idle_seq_answers_even_when_the_content_gate_skipped_the_arm() {
        let anims = slotted(&[145, 0, 157], Vec::new());
        assert_eq!(anims.first_seq, None, "the render gate declined to arm");
        assert_eq!(
            anims.idle_seq(),
            Some(1),
            "the sequence identity is still known"
        );
    }

    /// The idle id is resolved through the model's own `playableAnimationLookup` — the same
    /// `0x711bf0(reqId = 0)` chain `first_seq` takes. 268 models corpus-wide remap requested
    /// id 0 (`Cripple_State_Base` → 158 Hold), and slot 0 is not their idle either.
    #[test]
    fn idle_seq_resolves_id_zero_through_the_playable_lookup() {
        let anims = slotted(&[159, 158], vec![playable(158, 0)]);
        assert_eq!(anims.idle_seq(), Some(1));
    }

    /// No sequence for the resolved idle id ⇒ the first record's own slot, the guarded
    /// `*(dword*)&sequences[0]` fallback — and `None` only when there is no clip at all to name.
    #[test]
    fn idle_seq_falls_back_to_the_first_record_then_to_none() {
        assert_eq!(slotted(&[158, 159], Vec::new()).idle_seq(), Some(0));
        assert_eq!(slotted(&[], Vec::new()).idle_seq(), None);
    }

    /// The variation pick is the client's weighted walk (op4 `0x7121a0` with variationIdx −1):
    /// `roll < frequency` picks, else `roll -= frequency` and advance; chain exhaustion clamps to
    /// the last variation; a lone variation always wins.
    #[test]
    fn pick_variation_walks_the_weighted_chain() {
        let mut anims = test_anims(&[16, 16, 5], Vec::new());
        anims.clips[0].frequency = 0x6000; // Attack1H variation 0 — the common arc
        anims.clips[1].frequency = 0x1fff; // variation 1 — the off-side swing
        let pick = |anims: &ModelAnimations, id, roll| {
            let c = anims.pick_variation(id, roll).unwrap();
            anims.clips.iter().position(|x| std::ptr::eq(x, c)).unwrap()
        };
        assert_eq!(pick(&anims, 16, 0x0000), 0);
        assert_eq!(pick(&anims, 16, 0x5fff), 0); // last roll inside variation 0's weight
        assert_eq!(pick(&anims, 16, 0x6000), 1); // first roll past it
        assert_eq!(pick(&anims, 16, 0x7ffe), 1);
        // Weights under-cover the roll → the HEAD, not the last (the client's exhaust path leaves
        // the resolved seqIndex untouched — `0x7124de`, corrects decision 0114).
        assert_eq!(pick(&anims, 16, 0x7fff), 0);
        assert_eq!(pick(&anims, 5, 0x7fff), 2); // lone variation wins by exhaustion→head (weight 0)
        assert!(anims.pick_variation(99, 0).is_none());
    }
}
