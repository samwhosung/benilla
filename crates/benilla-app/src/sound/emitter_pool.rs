//! The **ambient emitter pool** — the reference's registration table at `0xb06dd8` and its
//! per-frame pump `0x461990`.
//!
//! **Two tenants share it, because the reference's one pool has two registrars**.
//! A *placed doodad* registers through its `$DSL` marker (`0x6951e0`, handle in
//! `[CMapDoodadDef+0x168]`); a *GameObject* registers through **both** its own `$DSL` arm
//! (`0x5f3fe5`) and the display-slot sound player `0x5f4010`'s **loop lane** — the branch
//! `SoundEntries` flag `0x200` selects — with the handle in `[handler+0x18]`, one per object. All
//! of them call the same `0x461d80`/`0x461f80`/`0x462000` register/release/reposition family, so a
//! campfire GameObject's `CampFireLargeLoop` and a placed brazier's share an entry and a channel
//! exactly as two lamps do. The registrars differ only in how they treat a *second* id arriving on
//! a live handle — see [`register`] and [`register_keeping_first`].
//!
//! B345's fix gave every world-placed doodad a clock and routed its `$DSL` key to audio, which is
//! what made a lamp hum at all. It played that hum the obvious way — one channel per doodad, tagged
//! to the doodad — and that is **not the shape the reference has**. The client does not voice
//! doodads; it voices **sound ids**, and an emitter is only a *position* it can put one at:
//!
//! - `$DSL` registers the doodad's position as one **record** inside the pool entry that holds its
//!   SoundEntries id (`0x461d80`, handle `((entry+1) << 16) | record` kept in
//!   `[CMapDoodadDef+0x168]`). Every doodad naming the same id shares one entry.
//! - Each entry runs **exactly one channel**, positioned at whichever of its records is **nearest
//!   the listener** (`0x461ca0`'s 256-wide argmin), repositioned as that changes and **never
//!   restarted** (`0x7a5b10`). One hum follows you down a row of thirty lamps.
//! - The pump walks the 32 entries and lets the first **four** hold a channel (`0x4619c0 cmp
//!   edi,4`). Arbitration is by **ascending entry index — claim order, not distance**: a fifth
//!   distinct ambience is faded out over 3.0 s and does *not* take over when you walk toward it.
//!
//! So the three limits are structural, not a budget bolted on: **32 distinct ids**, **256 emitters
//! per id**, **4 sounding at once**.
//!
//! **What we had instead**, measured rather than assumed (`benilla-extract soundeventscan`): a
//! channel per doodad, parked on its own model and restarted every time you crossed its
//! `DistanceCutoff`. The *count* was held down by two gates that were never about ambience —
//! **45 of the 60** resolvable `$DSL` kits carry the reference's own `NO_DUPLICATES` bit (0x20),
//! which benilla applied on every lane, so three-quarters of doodad ambience already played
//! exactly one channel; the other 15 got up to two from `SAME_KIT_MAX`. One channel, yes — but at
//! whichever doodad happened to fire first rather than the nearest, retriggering at every cutoff
//! crossing, and with **nothing whatever** bounding the number of *distinct* ambiences, so a
//! torch-and-campfire cluster could take a third of the 12-voice ceiling away from the sounds a
//! player is actually listening for. Applying 0x20 here was itself wrong: it is the **one-shot**
//! lane's suppressor (`0x458f40` → `0x7a66a0`), and this lane is now exempt from it
//! ([`super::kit::PlayExtras::dedupe_exempt`]).
//!
//! **What the pool is not.** It is not a cache and not an optimisation — it is the mechanism that
//! decides *where* a doodad ambience comes from and *which* ones you hear. Sharing a channel is
//! what makes the emitter track the nearest lamp; the cap is what makes the fifth one silent.
//!
//! **Deliberate divergences**, both documented at their site: a failed kit is flagged in place
//! rather than by negating the entry's id, and a channel the frame pump culls past the kit's own
//! `DistanceCutoff` is stopped rather than left running at zero gain. Neither changes what is
//! audible; see the notes on [`Entry::failed`] and [`pump_emitters`].

use std::collections::HashSet;

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use benilla_assets::WorldAssets;
use benilla_world::schedule::WorldStage;

use super::kit::{
    self, kit_name, play_kit_ext, set_source_kit_gain, source_kit_playing, stop_source, KitRef,
    PlayExtras, SoundCategory, SoundKits,
};
use super::{math, AudioListener, SoundConfig, SoundOutput};

/// `0x461eac cmp ecx,0x1c200` over the entry stride `0xe10` — **32** entries, one per distinct
/// SoundEntries id. A 33rd distinct id finds no match and no free entry, `0x461d80` returns handle
/// 0, and that doodad is simply untracked until an entry frees.
const POOL_ENTRIES: usize = 32;

/// `[entry+0xC00..+0xCFF]` — **256** emitter records per entry.
const RECORDS_PER_ENTRY: usize = 256;

/// `0x4619c0 cmp edi,4` — at most **four** entries hold a channel at once.
const PLAYING_CAP: usize = 4;

/// `0x7a5a10(3.0f)` — the fade a released or cap-evicted channel gets, in seconds.
const FADE_SECS: f32 = 3.0;

/// The entity a pool entry's one channel rides. Its `Transform` is what [`kit::pump_channels`]'s
/// tracked-follow reads, so moving it *is* the reference's reposition (`0x7a5b10`) — the same
/// trick the liquid ambience loops use for their slewed emitter.
#[derive(Component)]
struct PoolEmitter;

/// One registered emitter: the reference's `[entry + 12·rec]` position, with presence in the list
/// standing in for its `[+0xC00+rec]` active byte. `owner` is the entity holding the registration
/// — a placed doodad's anim host, or a GameObject.
struct Record {
    owner: Entity,
    pos: Vec3,
}

/// One pool entry — a SoundEntries id, its emitters, and the single channel they share.
#[derive(Default)]
struct Entry {
    /// `[+0xE00]` — the SoundEntries id this entry holds; 0 = free.
    id: u32,
    /// **Divergence.** The reference marks a kit that failed to resolve by *negating* the id
    /// (`[+0xE00] = -id`), which makes the entry stop matching its own id: the doodad's next
    /// `$DSL` then releases and re-registers into a fresh entry, every cycle, for ever. A separate
    /// flag keeps the doodad where it is. Both hold exactly one entry and both are silent, so
    /// nothing observable turns on it — but ours does not churn.
    failed: bool,
    /// `[+0xC00]`'s active set, in claim order (which is what the 257th-record eviction rule
    /// below reads as "first").
    records: Vec<Record>,
    /// `[+0xE04]`'s companion — the entity carrying this entry's channel, `None` when none is
    /// live.
    voice: Option<Entity>,
}

impl Entry {
    /// `0x461b40` steps 1–3: is there a channel coming to this entry at all? (Occupied, its kit
    /// resolvable, and at least one emitter registered.) Entitlement, not audibility — it decides
    /// whether the cap walk *services* the entry, never whether it *counts*; the composite return
    /// of steps 4–6 decides that (see [`cap_step`]).
    fn entitled(&self) -> bool {
        self.id != 0 && !self.failed && !self.records.is_empty()
    }

    /// `0x461ca0` — the position of the **nearest** active record, ties keeping the first
    /// (`0x461cf6` discards a candidate that is not *strictly* closer).
    fn nearest(&self, listener: Vec3) -> Option<Vec3> {
        self.records
            .iter()
            .map(|r| (math::dist_sq(listener, r.pos), r.pos))
            .reduce(|best, cand| if cand.0 < best.0 { cand } else { best })
            .map(|(_, pos)| pos)
    }
}

/// A channel the pool has let go of and is fading out over [`FADE_SECS`].
///
/// It is an **orphan on purpose**: the reference starts the fade and clears `[+0xE04]` in the same
/// breath (`0x461d20`, `0x4619c5`), so the entry is free to be reclaimed by another id — or to
/// start a fresh channel of its own — while the old sound is still dying. Holding the fade on the
/// entry would serialise those.
struct Fading {
    emitter: Entity,
    kit: u32,
    gain: f32,
}

/// The pool itself.
#[derive(Resource)]
pub(super) struct AmbientEmitterPool {
    entries: [Entry; POOL_ENTRIES],
    /// Which entry each owner is registered in — the reference's per-owner handle
    /// (`[CMapDoodadDef+0x168]` for a placed doodad, `[handler+0x18]` for a GameObject), minus the
    /// record half (a record *is* its owner here). One handle per owner is the reason a
    /// GameObject's `$DSL` and its looping display slot contend for the same registration, exactly
    /// as they do in the reference.
    handles: EntityHashMap<usize>,
    fading: Vec<Fading>,
    /// Kit ids already warned about. A `$DSL` re-fires every animation cycle, so an unresolvable
    /// id is a per-cycle failure on a per-doodad stream: the same log flood B345's routing half
    /// had to learn to avoid.
    complained: HashSet<u32>,
    /// The last logged census — `(kit id, is a channel actually live)` per admitted entry — so
    /// the line below reports **edges**, not frames. Liveness is in the key on purpose: a channel
    /// culled past its cutoff and restarted is exactly the churn a broken nearest-follow would
    /// produce, and the census is where that has to be visible.
    last_census: Vec<(u32, bool)>,
    /// The last logged **withheld** set — the entitled ids past the cap. Part of the edge key for
    /// the same reason liveness is: *which* ambience the cap took away is the whole question a
    /// report of a missing layer asks, and the census could only ever say how many (2059).
    last_withheld: Vec<u32>,
}

impl Default for AmbientEmitterPool {
    fn default() -> Self {
        Self {
            entries: std::array::from_fn(|_| Entry::default()),
            handles: EntityHashMap::default(),
            fading: Vec::new(),
            complained: HashSet::new(),
            last_census: Vec::new(),
            last_withheld: Vec::new(),
        }
    }
}

impl AmbientEmitterPool {
    /// The **compare-and-swap** registrar — the placed-doodad `$DSL` arm (`0x69521d`'s compare
    /// followed by `0x462000` reposition or `0x461f80`+`0x461d80` re-register), and, observably,
    /// the display-slot loop lane's arm too (`0x5f4083`: same id returns without even
    /// repositioning, which for a fixed emitter is the same thing).
    ///
    /// Crossing the marker again with the **same** id only moves the record; that is the whole
    /// reason there is no wrap retrigger, and why `NightElfStreetLampLoop` (4.000 s of sample on a
    /// 3.333 s sequence) is not chopped every cycle. A **different** id releases the old
    /// registration and takes a new one, which is what makes `bellows.m2`'s `$DSL` pair —
    /// `BellowOut` at t=0.000, `BellowIN` at t=1.100 on one looping 2 s sequence — *alternate*
    /// through one slot instead of droning together.
    ///
    /// The GameObject `$DSL` arm is the one that does **not** compare — see
    /// [`register_keeping_first`].
    fn register(&mut self, owner: Entity, id: u32, pos: Vec3, listener: Vec3) {
        if let Some(&e) = self.handles.get(&owner) {
            if self.entries[e].id == id {
                self.reposition(e, owner, pos);
                return;
            }
            self.release(owner);
        }
        // `0x461e60`: the entry already holding this id, else the first free one.
        let Some(e) = self
            .entries
            .iter()
            .position(|x| x.id == id)
            .or_else(|| self.entries.iter().position(|x| x.id == 0))
        else {
            return; // all 32 entries hold other ids — untracked until one frees
        };
        if self.entries[e].id == 0 {
            self.entries[e].id = id;
            self.entries[e].failed = false;
        }
        if self.entries[e].records.len() >= RECORDS_PER_ENTRY {
            // `0x461a60` — evict the FIRST record farther from the listener than the newcomer,
            // and reject the newcomer when none is. Not an LRU and not an argmax: first-farther,
            // in claim order.
            let d = math::dist_sq(listener, pos);
            let Some(victim) = self.entries[e]
                .records
                .iter()
                .position(|r| math::dist_sq(listener, r.pos) > d)
            else {
                return;
            };
            let gone = self.entries[e].records.remove(victim);
            self.handles.remove(&gone.owner);
        }
        self.entries[e].records.push(Record { owner, pos });
        self.handles.insert(owner, e);
    }

    /// The **GameObject** lane's `$DSL` arm (`0x5f3fe5`): register when the object holds no
    /// registration at all, else only *reposition* the one it holds. **It never compares the id,
    /// so it never swaps** — the one place the two registrars actually differ.
    ///
    /// It is not a curiosity: Onyxia's lava trap (`ONYZIASLAIRLAVATRAP.M2`, 208 spawns in the
    /// lair) and Stratholme's spore trap author **two** ids — `$DSL(8681)` on the chained Stand
    /// variation and `$DSL(8682)` on Custom0 — so on the compare-and-swap arm the Custom0 hum
    /// would displace the Stand one, and on this arm it never plays at all. What clears the handle
    /// on this lane is not a second marker but the state-machine dispatch
    /// ([`super::gameobject`]'s release) or the object's despawn; there is no `$DSE` arm on the
    /// GameObject dispatcher at all.
    fn register_keeping_first(&mut self, owner: Entity, id: u32, pos: Vec3, listener: Vec3) {
        if let Some(&e) = self.handles.get(&owner) {
            self.reposition(e, owner, pos);
            return;
        }
        self.register(owner, id, pos, listener);
    }

    /// `0x462000` — move `owner`'s record inside entry `e`, leaving the entry's id and channel
    /// alone. The reference writes the position only when it actually changed; ours writes it
    /// unconditionally, which is the same value.
    fn reposition(&mut self, e: usize, owner: Entity, pos: Vec3) {
        if let Some(r) = self.entries[e]
            .records
            .iter_mut()
            .find(|r| r.owner == owner)
        {
            r.pos = pos;
        }
    }

    /// `0x461f80` → `0x461d20` — release `owner`'s record. When it was the entry's **last**, the
    /// id is cleared and the channel starts its 3.0 s fade as an orphan.
    ///
    /// Reached from a placed doodad's `$DSE` (the authored stop token — the elevator and machinery
    /// family, where the loop runs for one leg of the animation), from a GameObject's state-machine
    /// dispatch (`0x5f3cc8 call 0x5f40c0`, [`super::gameobject`]) and from the host's despawn (the
    /// owner's own teardown, `0x6a0840`). There is deliberately **no** map-change reset: the
    /// reference has none either (`0x461a20`'s only caller is the process-shutdown chain), and a
    /// streamed-out tile releases its doodads one by one, which is the same thing done honestly.
    fn release(&mut self, owner: Entity) {
        let Some(e) = self.handles.remove(&owner) else {
            return;
        };
        let entry = &mut self.entries[e];
        entry.records.retain(|r| r.owner != owner);
        if !entry.records.is_empty() {
            return;
        }
        let (kit, voice) = (entry.id, entry.voice);
        *entry = Entry::default();
        if let Some(emitter) = voice {
            self.fading.push(Fading {
                emitter,
                kit,
                gain: 1.0,
            });
        }
    }

    /// Hand this entry's channel to the fade list and leave the entry otherwise intact — the
    /// cap's eviction (`0x4619c5`), which keeps the id and its records so the entry can start a
    /// fresh channel the moment it is back under the cap.
    fn retire(&mut self, e: usize) {
        let (kit, voice) = {
            let entry = &mut self.entries[e];
            (entry.id, entry.voice.take())
        };
        if let Some(emitter) = voice {
            self.fading.push(Fading {
                emitter,
                kit,
                gain: 1.0,
            });
        }
    }
}

/// What the cap walk does with one entry, folded over ascending index
/// (`0x4619c0`–`0x4619f9`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CapStep {
    /// Not entitled — free, failed, or emitter-less. Costs the cap nothing and gets no service.
    Skip,
    /// Under the cap: service it (start or reposition), then let the channel say whether it
    /// counts.
    Service,
    /// Four are already sounding — fade this entry's channel out over [`FADE_SECS`] and leave
    /// everything else on the entry (`0x4619c5`).
    Retire,
}

/// One step of the cap walk. **`sounding_so_far` counts entries that came back LIVE AND IN RANGE,
/// not entitled ones** — the correction of 2026-09-07.
///
/// Index is claim order, so this is first-come — emphatically **not** nearest. That is the cap's
/// one surprising property: standing where five distinct doodad ambiences are *audible*, exactly
/// four sound, and walking toward the fifth does not make it take over.
///
/// **What changed, and why the old reading mattered.** This used to count *entitlement* — id
/// present, kit resolvable, at least one emitter registered — on the reasoning that a channel past
/// its `DistanceCutoff` is still "playing" in FMOD, merely attenuated to nothing. The binary says
/// otherwise. `0x461b40` returns `[+0xE04] != 0 && 0x7a5810(ch) && !0x7a5870(ch)`, and `0x7a5870`
/// is bit `0x100000` of `[ch+0x38]` — **the `DistanceCutoff` verdict**, maintained by a second
/// system the pump never mentions (`0x7a5ca0` registers the channel into the cull list
/// `[0xcf5580]`, and `0x7a5000`, run immediately before `FSOUND_Update`, sets the bit and *stops*
/// the channel out of range, clearing it and re-issuing `FSOUND_Stream_PlayEx` back in range). So a
/// far, inaudible ambience does **not** hold one of the four, and neither does one whose start
/// failed. The one exception is byte-derived too: a kit whose `DistanceCutoff` is **0** is never
/// registered into the cull list (`0x7a5cca jnp`), so it holds a slot at any distance — which our
/// `play_kit_ext` reproduces, since its own cutoff gate is `cutoff > 0.0 && !audible`.
///
/// This is not a detail. At the director's Stratholme pin all four channels were held by
/// `UndeadCampfireSmall` / `CauldronLoop` / `TorchLoop` / `SlimeWaterfall`, **every one past its
/// own cutoff** — nothing audible at all — while the burning-building loop right there was
/// withheld. Under the reference's rule not one of the four would have counted.
const fn cap_step(entitled: bool, sounding_so_far: usize) -> CapStep {
    if !entitled {
        CapStep::Skip
    } else if sounding_so_far >= PLAYING_CAP {
        CapStep::Retire
    } else {
        CapStep::Service
    }
}

/// The pump (`0x461990`): fade the orphans, mark unresolvable kits, then service the admitted
/// entries — start a channel where there is none, and otherwise only *move* the one there is.
fn pump_emitters(
    mut pool: ResMut<AmbientEmitterPool>,
    mut emitters: Query<&mut Transform, With<PoolEmitter>>,
    time: Res<Time>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
    mut commands: Commands,
) {
    let (Some(mut kits), Some(assets)) = (kits, assets) else {
        return;
    };
    // Running silent (`WOW_NOSOUND=1`, CI, an unattended probe): a start that cannot succeed must
    // not be retried per frame per entry, and must not warn — the same lesson the liquid loops
    // learned at 3975 identical lines in a 75 s run.
    if out.mixer.is_none() {
        return;
    }
    let listener_pos = listener.pos;
    let step = if FADE_SECS > 0.0 {
        time.delta_secs() / FADE_SECS
    } else {
        1.0
    };
    let pool = &mut *pool;

    // The orphans' fade-out.
    pool.fading.retain_mut(|f| {
        f.gain -= step;
        // Faded out, or nothing left to fade (the frame pump culled the channel past its cutoff,
        // or the voice ceiling stole it): let the entity go now rather than hold it for 3 s of
        // silence.
        if f.gain <= 0.0 || !source_kit_playing(&out, f.emitter, f.kit) {
            stop_source(&mut out, f.emitter);
            commands.entity(f.emitter).despawn();
            return false;
        }
        set_source_kit_gain(&mut out, f.emitter, f.kit, f.gain);
        true
    });

    // `0x45cda0(id)` returning null → `[+0xE00] = -id`, permanently. Done before the arbitration
    // so a dead id never costs a live one its slot, not even for a frame. `NightElfLantern01`'s
    // `$DSL(33764)` is the shipped case: 5875's `SoundEntries` simply has no such row.
    for entry in pool.entries.iter_mut() {
        if entry.id != 0 && !entry.failed && kit_name(&kits, entry.id).is_none() {
            entry.failed = true;
            if pool.complained.insert(entry.id) {
                warn!(
                    "doodad emitter kit {}: no SoundEntries row — entry silenced permanently \
                     (further reports for this kit suppressed)",
                    entry.id
                );
            }
        }
    }

    // The cap walk (`0x461990`), ascending entry index. An entry is SERVICED while fewer than four
    // are sounding, and it counts only if servicing produced a live, in-range channel
    // ([`cap_step`]) — so an out-of-range ambience is kept up to date and re-starts the moment it
    // comes into range, but never costs a near one its slot.
    let mut sounding = 0usize;
    let mut census: Vec<(u32, bool)> = Vec::new();
    let mut withheld: Vec<u32> = Vec::new();
    for e in 0..POOL_ENTRIES {
        match cap_step(pool.entries[e].entitled(), sounding) {
            CapStep::Skip => {
                // Free, failed or emitter-less: nothing to hold, nothing to fade.
                pool.retire(e);
                continue;
            }
            CapStep::Retire => {
                // Over the cap: the channel fades out and the entry keeps everything else.
                withheld.push(pool.entries[e].id);
                pool.retire(e);
                continue;
            }
            CapStep::Service => {}
        }
        let id = pool.entries[e].id;
        let Some(nearest) = pool.entries[e].nearest(listener_pos) else {
            continue; // `entitled()` guarantees a record; belt and braces
        };

        // Put the voice entity at the nearest record. For a live channel this *is* the reference's
        // reposition — the frame pump's tracked-follow ships the new position and the sound never
        // restarts, which is the difference between one hum that follows you down a row of lamps
        // and thirty hums that each retrigger as you pass.
        let emitter = match pool.entries[e].voice {
            Some(em) => {
                match emitters.get_mut(em) {
                    // Compare-then-write (1362): a parked listener's nearest record is the same
                    // point every frame, and a no-op write still re-propagates the transform.
                    Ok(mut tf) => {
                        if tf.translation != nearest {
                            tf.translation = nearest;
                        }
                    }
                    // Ours, spawned on an earlier frame, so this cannot happen — except through a
                    // query-filter mistake, which would silently freeze every ambience at the
                    // first emitter it ever found and leave the whole nearest-follow inert. That
                    // failure is invisible in the audio (a hum is a hum), so it has to be loud
                    // here.
                    Err(err) => {
                        if pool.complained.insert(id) {
                            warn!(
                                "doodad emitter {em}: transform unreachable ({err}) — kit {id} is \
                                 stuck at its first emitter"
                            );
                        }
                    }
                }
                em
            }
            None => {
                let em = commands
                    .spawn((PoolEmitter, Transform::from_translation(nearest)))
                    .id();
                pool.entries[e].voice = Some(em);
                em
            }
        };
        if source_kit_playing(&out, emitter, id) {
            sounding += 1;
            census.push((id, true));
            continue;
        }
        // No channel: either this entry has never sounded, or the frame pump culled ours past the
        // kit's `DistanceCutoff`. **Divergence:** the reference's pool lane has no distance
        // pre-check and leaves the channel running at zero gain past the cutoff, where ours stops
        // and restarts it. For a loop of ambience that is inaudible — restarting a hum you could
        // not hear costs nothing but a phase — and it hands the voice back while it is silent.
        // The *cap* is unaffected, because [`admitted`] counts entitlement, not liveness.
        if let Err(err) = play_kit_ext(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener_pos,
            KitRef::Id(id),
            Some(nearest),
            SoundCategory::Sfx,
            PlayExtras {
                source: Some(emitter),
                // Looping is an entry-point constant on this lane, not a flag: `0x7a54d0` builds
                // mode `0x1002` (`HW3D|LOOP_NORMAL`) and calls `SetLoopCount(-1)`, against
                // `0x7a5490`'s `0x1000` for the one-shot path. Every `$DSL` loops; no `$DSO` does;
                // `SoundEntries.Flags` is consulted for neither.
                force_loop: true,
                // The per-id suppressors are the one-shot lane's; this lane dedupes structurally
                // (one entry per id) and must not be blocked by its own fading predecessor.
                dedupe_exempt: true,
                ..default()
            },
        ) {
            if pool.complained.insert(id) {
                warn!("doodad emitter kit {id}: {err:#} (further reports for this kit suppressed)");
            }
        }
        // Whether the start actually took: `play_kit_ext` succeeds *without playing* when the kit
        // is past its cutoff, held by the loading cover, or refused by the voice ceiling. Reading
        // the channel back is the only honest answer — and since 2065 it is also the cap's own
        // counter (`0x461b40`'s composite return, [`cap_step`]), which is why this is read rather
        // than predicted.
        let live = source_kit_playing(&out, emitter, id);
        if live {
            sounding += 1;
        }
        census.push((id, live));
    }

    // The pool's one observable, in the three states the cap actually has. "Which doodad ambiences
    // are sounding, and which one went quiet" is invisible in the audio itself — five ids in range
    // and four sounding is indistinguishable from four in range unless something says so — and it
    // is precisely the question a retest of the cap has to answer. Logged on the **edge**, so a
    // parked camera prints one line, not one per frame.
    //
    // **Naming the withheld entries is the half the census was missing** (2059): "1 withheld by
    // the cap" cannot say *which* ambience went silent, and that is the only question a report of
    // a missing sound layer asks. Since 2065 the split is three ways rather than two, because the
    // cap's own counter is liveness: `serviced [out of range]` is an entry the walk reached, kept
    // positioned and could not sound — costing the cap **nothing** — while `withheld` is one the
    // walk never reached because four were already sounding. Those were the same word before, and
    // conflating them is exactly the reading the binary refuted.
    if census != pool.last_census || withheld != pool.last_withheld {
        let entitled = pool.entries.iter().filter(|x| x.entitled()).count();
        let named: Vec<String> = census
            .iter()
            .map(|(id, live)| {
                let name = kit_name(&kits, *id).unwrap_or("?");
                format!(
                    "{id} ({name}){}",
                    if *live { "" } else { " [out of range]" }
                )
            })
            .collect();
        let held: Vec<String> = withheld
            .iter()
            .map(|id| format!("{id} ({})", kit_name(&kits, *id).unwrap_or("?")))
            .collect();
        debug!(
            "emitter pool: serviced [{}] — {} sounding, {entitled} entitled entr{}, withheld by \
             the cap [{}], {} fading",
            named.join(", "),
            census.iter().filter(|(_, live)| *live).count(),
            if entitled == 1 { "y" } else { "ies" },
            held.join(", "),
            pool.fading.len(),
        );
        pool.last_census = census;
        pool.last_withheld = withheld;
    }
}

/// Register `owner`'s emitter for kit `id` at `pos` — the **compare-and-swap** arm: a placed
/// doodad's `$DSL` (`0x6951e0`) and a GameObject's looping display slot (`0x5f4010`), both called
/// from their own routers.
pub(super) fn register(
    pool: &mut AmbientEmitterPool,
    owner: Entity,
    id: u32,
    pos: Vec3,
    listener: Vec3,
) {
    pool.register(owner, id, pos, listener);
}

/// Register `owner`'s emitter, **keeping whatever id it already holds** — a GameObject's `$DSL`
/// arm (`0x5f3fe5`), which never compares the id. See
/// [`AmbientEmitterPool::register_keeping_first`].
pub(super) fn register_keeping_first(
    pool: &mut AmbientEmitterPool,
    owner: Entity,
    id: u32,
    pos: Vec3,
    listener: Vec3,
) {
    pool.register_keeping_first(owner, id, pos, listener);
}

/// Release `owner`'s emitter — a placed doodad's `$DSE`, and a GameObject's state-machine
/// dispatch (`0x5f40c0`).
pub(super) fn release(pool: &mut AmbientEmitterPool, owner: Entity) {
    pool.release(owner);
}

/// Release the emitters of owners that have gone — a placed doodad's host (`0x7133a0`'s teardown
/// leg) or a GameObject (whose registration lives on the type handler that goes with it).
///
/// This replaces B345's channel-scoped reaper. An owner no longer *owns* a channel, so stopping
/// "its" channel is the wrong verb: what a despawn retires is one **record**, and the sound only
/// stops when the last emitter of that id in the world is gone. Streaming out one of thirty lamps
/// must not silence the other twenty-nine.
fn release_emitters_on_despawn(
    mut doodads: RemovedComponents<benilla_world::doodad_anim::DoodadAnimHost>,
    mut gos: RemovedComponents<crate::go_anim::GoAnim>,
    mut pool: ResMut<AmbientEmitterPool>,
) {
    for entity in doodads.read().chain(gos.read()) {
        pool.release(entity);
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<AmbientEmitterPool>().add_systems(
        Update,
        (
            // Registration (`$DSL`/`$DSE` in `anim_events`, the display-slot loop lane in
            // `gameobject`) and the despawn release both write the pool; the pump reads it and
            // starts channels; the frame pump then applies this
            // frame's positions and gains. Ordered explicitly rather than left to the scheduler,
            // because an arbitrary order costs a frame of latency on a stop.
            release_emitters_on_despawn.before(pump_emitters),
            pump_emitters
                .after(super::anim_events::route_anim_events)
                .after(super::gameobject::go_display_sounds)
                .before(kit::pump_channels),
        )
            .in_set(WorldStage::Present),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entities are only identities here — the pool never looks one up.
    fn doodads(n: u32) -> Vec<Entity> {
        (0..n)
            .map(Entity::from_raw_u32)
            .map(Option::unwrap)
            .collect()
    }

    fn pool_with(entries: &[(u32, usize)]) -> AmbientEmitterPool {
        let mut pool = AmbientEmitterPool::default();
        let ds = doodads(entries.len() as u32 * 2);
        for (i, (id, records)) in entries.iter().enumerate() {
            for r in 0..*records {
                pool.entries[i].records.push(Record {
                    owner: ds[r % ds.len()],
                    pos: Vec3::ZERO,
                });
            }
            pool.entries[i].id = *id;
        }
        pool
    }

    /// The structural dedupe: N doodads naming one kit share ONE entry, which is what makes the
    /// pump run one channel for the lot.
    #[test]
    fn every_doodad_naming_one_kit_shares_a_single_entry() {
        let mut pool = AmbientEmitterPool::default();
        let ds = doodads(30);
        for (i, d) in ds.iter().enumerate() {
            pool.register(*d, 3378, Vec3::new(i as f32, 0.0, 0.0), Vec3::ZERO);
        }
        assert_eq!(pool.entries.iter().filter(|e| e.id != 0).count(), 1);
        assert_eq!(pool.entries[0].records.len(), 30);
        // …and its channel goes to the nearest of the thirty, not to whichever fired last.
        assert_eq!(
            pool.entries[0]
                .nearest(Vec3::new(29.0, 0.0, 0.0))
                .unwrap()
                .x,
            29.0
        );
        assert_eq!(pool.entries[0].nearest(Vec3::ZERO).unwrap().x, 0.0);
    }

    /// `0x461cf6` discards a candidate that is not *strictly* closer.
    #[test]
    fn a_tie_for_nearest_keeps_the_first_record() {
        let mut pool = AmbientEmitterPool::default();
        let ds = doodads(2);
        pool.register(ds[0], 7, Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
        pool.register(ds[1], 7, Vec3::new(0.0, 0.0, -5.0), Vec3::ZERO);
        assert_eq!(pool.entries[0].nearest(Vec3::ZERO).unwrap().z, 5.0);
    }

    /// Re-crossing the marker with the same id REPOSITIONS (`0x462000`); it never takes a second
    /// record and never releases. This is the no-wrap-retrigger property, in the data.
    #[test]
    fn re_firing_the_same_id_only_moves_the_record() {
        let mut pool = AmbientEmitterPool::default();
        let d = doodads(1)[0];
        pool.register(d, 3378, Vec3::ZERO, Vec3::ZERO);
        pool.register(d, 3378, Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO);
        assert_eq!(pool.entries[0].records.len(), 1);
        assert_eq!(pool.entries[0].records[0].pos, Vec3::new(1.0, 2.0, 3.0));
    }

    /// **The GameObject lane never swaps** (`0x5f3fe5`, no id compare) — the one place the two
    /// registrars differ. Onyxia's lava trap authors `$DSL(8681)` on its Stand variation and
    /// `$DSL(8682)` on Custom0; on this arm the second never displaces the first, and what
    /// eventually clears the handle is the state dispatch, not another marker.
    #[test]
    fn the_gameobject_arm_keeps_the_id_it_first_registered() {
        let mut pool = AmbientEmitterPool::default();
        let go = doodads(1)[0];
        pool.register_keeping_first(go, 8681, Vec3::ZERO, Vec3::ZERO);
        pool.register_keeping_first(go, 8682, Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO);
        assert_eq!(pool.entries.iter().filter(|e| e.id != 0).count(), 1);
        assert_eq!(pool.entries[0].id, 8681, "8682 must not displace it");
        // …and the second marker still MOVED the record — `0x462000` runs on both legs.
        assert_eq!(pool.entries[0].records[0].pos, Vec3::new(1.0, 0.0, 0.0));
        // The release is what frees it, and then the next marker takes the entry it names.
        pool.release(go);
        pool.register_keeping_first(go, 8682, Vec3::ZERO, Vec3::ZERO);
        assert_eq!(pool.entries[0].id, 8682);
    }

    /// The two lanes share one handle per owner, so a GameObject's looping display slot and its
    /// own `$DSL` contend for it exactly as `[handler+0x18]` does — and the compare-and-swap arm
    /// the display slot uses does displace.
    #[test]
    fn the_display_slot_lane_shares_the_owners_one_handle() {
        let mut pool = AmbientEmitterPool::default();
        let go = doodads(1)[0];
        pool.register_keeping_first(go, 3880, Vec3::ZERO, Vec3::ZERO); // its `$DSL`
        pool.register(go, 4694, Vec3::ZERO, Vec3::ZERO); // a looping display slot
        assert_eq!(pool.entries.iter().filter(|e| e.id != 0).count(), 1);
        assert_eq!(pool.entries[0].id, 4694);
    }

    /// `bellows.m2`: two `$DSL` keys on one looping sequence alternate through the doodad's single
    /// handle, rather than droning together.
    #[test]
    fn a_different_id_moves_the_doodads_one_registration() {
        let mut pool = AmbientEmitterPool::default();
        let d = doodads(1)[0];
        pool.register(d, 1000, Vec3::ZERO, Vec3::ZERO);
        pool.register(d, 2000, Vec3::ZERO, Vec3::ZERO);
        assert_eq!(pool.entries.iter().filter(|e| e.id != 0).count(), 1);
        assert_eq!(pool.entries.iter().find(|e| e.id != 0).unwrap().id, 2000);
    }

    /// Releasing the last record frees the entry (`0x461d20`) so another id can claim it.
    #[test]
    fn the_last_release_frees_the_entry() {
        let mut pool = AmbientEmitterPool::default();
        let ds = doodads(2);
        pool.register(ds[0], 42, Vec3::ZERO, Vec3::ZERO);
        pool.register(ds[1], 42, Vec3::ZERO, Vec3::ZERO);
        pool.release(ds[0]);
        assert_eq!(pool.entries[0].id, 42, "one emitter left: the id stays");
        pool.release(ds[1]);
        assert_eq!(pool.entries[0].id, 0);
        assert!(pool.handles.is_empty());
    }

    /// `0x461e60`: a 33rd distinct id is UNTRACKED — the registration fails outright rather than
    /// evicting an incumbent. `$DSL` re-fires every cycle, so it takes an entry the moment one
    /// frees.
    #[test]
    fn a_thirty_third_distinct_id_is_untracked_until_an_entry_frees() {
        let mut pool = AmbientEmitterPool::default();
        let ds = doodads(POOL_ENTRIES as u32 + 1);
        for (i, d) in ds.iter().enumerate() {
            pool.register(*d, 100 + i as u32, Vec3::ZERO, Vec3::ZERO);
        }
        assert_eq!(pool.handles.len(), POOL_ENTRIES);
        let late = ds[POOL_ENTRIES];
        assert!(!pool.handles.contains_key(&late));

        pool.release(ds[0]);
        pool.register(late, 100 + POOL_ENTRIES as u32, Vec3::ZERO, Vec3::ZERO);
        assert_eq!(pool.entries[0].id, 100 + POOL_ENTRIES as u32);
    }

    /// `0x461a60` — first-farther-than-the-newcomer, in claim order. Not an LRU, and not the
    /// farthest: the FIRST record farther than the incoming one goes.
    #[test]
    fn the_two_hundred_and_fifty_seventh_record_evicts_the_first_farther_one() {
        let mut pool = AmbientEmitterPool::default();
        let ds = doodads(RECORDS_PER_ENTRY as u32 + 1);
        // Records 0..255 at x = 255 down to 0: record 0 is the farthest, record 255 the nearest.
        for (i, d) in ds.iter().take(RECORDS_PER_ENTRY).enumerate() {
            let x = (RECORDS_PER_ENTRY - 1 - i) as f32;
            pool.register(*d, 9, Vec3::new(x, 0.0, 0.0), Vec3::ZERO);
        }
        assert_eq!(pool.entries[0].records.len(), RECORDS_PER_ENTRY);

        // A newcomer at x = 10 is nearer than record 0 (x = 255), so record 0 — the first one
        // farther than it — is the victim, and the newcomer takes the tail slot.
        let late = ds[RECORDS_PER_ENTRY];
        pool.register(late, 9, Vec3::new(10.0, 0.0, 0.0), Vec3::ZERO);
        assert_eq!(pool.entries[0].records.len(), RECORDS_PER_ENTRY);
        assert!(
            !pool.handles.contains_key(&ds[0]),
            "the evicted one loses its handle"
        );
        assert_eq!(
            pool.entries[0].records[0].pos.x,
            (RECORDS_PER_ENTRY - 2) as f32
        );
        assert_eq!(pool.entries[0].records.last().unwrap().pos.x, 10.0);
    }

    /// …and one farther than every incumbent is REJECTED, not swapped in.
    #[test]
    fn a_record_farther_than_all_of_them_is_rejected() {
        let mut pool = AmbientEmitterPool::default();
        let ds = doodads(RECORDS_PER_ENTRY as u32 + 1);
        for d in ds.iter().take(RECORDS_PER_ENTRY) {
            pool.register(*d, 9, Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO);
        }
        let late = ds[RECORDS_PER_ENTRY];
        pool.register(late, 9, Vec3::new(500.0, 0.0, 0.0), Vec3::ZERO);
        assert_eq!(pool.entries[0].records.len(), RECORDS_PER_ENTRY);
        assert!(!pool.handles.contains_key(&late));
    }

    /// Run the cap walk over a pool, given each entry's liveness verdict for this frame —
    /// [`cap_step`] folded exactly as [`pump_emitters`] folds it, without a mixer. Returns
    /// `(sounding indices, withheld indices)`.
    fn walk(pool: &AmbientEmitterPool, live: impl Fn(usize) -> bool) -> (Vec<usize>, Vec<usize>) {
        let (mut sounding, mut withheld) = (Vec::new(), Vec::new());
        for e in 0..POOL_ENTRIES {
            match cap_step(pool.entries[e].entitled(), sounding.len()) {
                CapStep::Skip => {}
                CapStep::Retire => withheld.push(e),
                CapStep::Service => {
                    if live(e) {
                        sounding.push(e);
                    }
                }
            }
        }
        (sounding, withheld)
    }

    /// The cap picks by ENTRY INDEX — claim order — and stops at four SOUNDING entries.
    #[test]
    fn the_cap_admits_the_first_four_sounding_entries_by_index() {
        let pool = pool_with(&[(10, 1), (20, 1), (30, 1), (40, 1), (50, 1), (60, 1)]);
        let (sounding, withheld) = walk(&pool, |_| true);
        assert_eq!(sounding, vec![0, 1, 2, 3]);
        assert_eq!(withheld, vec![4, 5]);
    }

    /// **The correction of 2065, and the reason Stratholme went silent.** An entry the walk
    /// services but which comes back with no live channel — past its own `DistanceCutoff`, or a
    /// start that failed — consumes **nothing**. `0x461b40` returns false for it (`0x7a5870`, the
    /// cutoff bit), so `0x4619c0`'s counter never advances and the walk keeps going.
    ///
    /// The four here are the real Stratholme pin: `UndeadCampfireSmall`, `CauldronLoop`,
    /// `TorchLoop` and `SlimeWaterfall`, every one out of range, in front of the burning-building
    /// loop with 88 emitters on top of the listener. Under the old entitlement count they held all
    /// four slots and it was withheld; under the binary's rule they hold none.
    #[test]
    fn a_serviced_but_silent_entry_does_not_consume_a_cap_slot() {
        let pool = pool_with(&[(3255, 1), (3379, 1), (4694, 1), (4855, 1), (6437, 1)]);
        let (sounding, withheld) = walk(&pool, |e| e == 4); // only the fire is in range
        assert_eq!(sounding, vec![4], "the audible fifth sounds");
        assert!(
            withheld.is_empty(),
            "nothing was ever withheld: {withheld:?}"
        );
    }

    /// Free, emitter-less and failed entries are not entitled and cost the cap nothing — the
    /// entry after them moves up.
    #[test]
    fn unentitled_entries_do_not_consume_a_cap_slot() {
        let mut pool = pool_with(&[(10, 1), (0, 0), (30, 0), (40, 1), (50, 1), (60, 1), (70, 1)]);
        pool.entries[3].failed = true;
        let (sounding, _) = walk(&pool, |_| true);
        assert_eq!(sounding, vec![0, 4, 5, 6]);
    }

    /// The falsifiable half: distance does NOT arbitrate **among entries that are all audible**.
    /// A fifth id right on top of the listener stays silent behind four claimed earlier — but
    /// only while those four are inside their own cutoffs, which is the qualifier 2065 added
    /// (with far incumbents the prediction is the opposite, and reading it without the qualifier
    /// would tear up a correct mechanism).
    #[test]
    fn distance_never_promotes_a_fifth_entry_over_an_earlier_audible_one() {
        let mut pool = AmbientEmitterPool::default();
        let ds = doodads(5);
        for (i, d) in ds.iter().enumerate().take(4) {
            pool.register(*d, 100 + i as u32, Vec3::new(400.0, 0.0, 0.0), Vec3::ZERO);
        }
        pool.register(ds[4], 999, Vec3::ZERO, Vec3::ZERO);
        let (sounding, withheld) = walk(&pool, |_| true); // all four incumbents audible
        assert_eq!(sounding, vec![0, 1, 2, 3]);
        assert_eq!(
            withheld,
            vec![4],
            "the near fifth is held back by claim order, not distance"
        );
    }
}
