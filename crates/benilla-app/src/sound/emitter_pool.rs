//! The ambient emitter pool: the reference's table at `0xb06dd8` and its pump `0x461990`.
//!
//! The reference voices sound ids, not doodads. A placed doodad's `$DSL` (`0x6951e0`), a
//! GameObject's `$DSL` (`0x5f3fe5`) and a GameObject's looping display slot (`0x5f4010`, the
//! `SoundEntries` flag `0x200` lane) all register a position as one record in the entry holding
//! that id (`0x461d80`/`0x461f80`/`0x462000`), one handle per owner. Each entry runs one channel
//! at its record nearest the listener (`0x461ca0`), moved, never restarted (`0x7a5b10`). The first
//! four entries by index that sound hold a channel (`0x4619c0`): claim order, not distance.
//! Limits: 32 ids, 256 records per id, 4 sounding.

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

/// 32 entries, one per distinct SoundEntries id (`0x461eac`: `0x1c200` over stride `0xe10`); a
/// 33rd id gets handle 0 from `0x461d80` and stays untracked until an entry frees.
const POOL_ENTRIES: usize = 32;

/// 256 emitter records per entry (`[entry+0xC00..+0xCFF]`).
const RECORDS_PER_ENTRY: usize = 256;

/// At most four entries hold a channel at once (`0x4619c0`).
const PLAYING_CAP: usize = 4;

/// The fade a released or cap-evicted channel gets, in seconds (`0x7a5a10(3.0f)`).
const FADE_SECS: f32 = 3.0;

/// The entity an entry's channel rides; moving its `Transform` is the reference's reposition
/// (`0x7a5b10`) through [`kit::pump_channels`]'s tracked follow.
#[derive(Component)]
struct PoolEmitter;

/// One registered emitter (`[entry + 12·rec]`), held by a doodad's anim host or a GameObject.
struct Record {
    owner: Entity,
    pos: Vec3,
}

/// One pool entry: a SoundEntries id, its emitters and the one channel they share.
#[derive(Default)]
struct Entry {
    /// `[+0xE00]`, the SoundEntries id; 0 is free.
    id: u32,
    /// Deviation: the reference marks an unresolvable kit by negating the id (`[+0xE00] = -id`),
    /// so the owner's next `$DSL` re-registers into a fresh entry every cycle; a flag keeps it in
    /// place without the churn. Both are silent.
    failed: bool,
    /// `[+0xC00]`'s active set, in claim order.
    records: Vec<Record>,
    /// The entity carrying this entry's channel (`[+0xE04]`).
    voice: Option<Entity>,
}

impl Entry {
    /// `0x461b40` steps 1-3: occupied, resolvable, with an emitter. This decides whether the cap
    /// walk services the entry, not whether it counts ([`cap_step`]).
    fn entitled(&self) -> bool {
        self.id != 0 && !self.failed && !self.records.is_empty()
    }

    /// The nearest active record (`0x461ca0`); a tie keeps the first (`0x461cf6`).
    fn nearest(&self, listener: Vec3) -> Option<Vec3> {
        self.records
            .iter()
            .map(|r| (math::dist_sq(listener, r.pos), r.pos))
            .reduce(|best, cand| if cand.0 < best.0 { cand } else { best })
            .map(|(_, pos)| pos)
    }
}

/// A channel fading out over [`FADE_SECS`], detached from its entry: the reference clears
/// `[+0xE04]` as the fade starts (`0x461d20`, `0x4619c5`), so the entry is free at once.
struct Fading {
    emitter: Entity,
    kit: u32,
    gain: f32,
}

/// The pool itself.
#[derive(Resource)]
pub(super) struct AmbientEmitterPool {
    entries: [Entry; POOL_ENTRIES],
    /// Each owner's entry, the reference's per-owner handle (`[CMapDoodadDef+0x168]`,
    /// `[handler+0x18]`); one per owner, so a GameObject's `$DSL` and its looping display slot
    /// contend for it.
    handles: EntityHashMap<usize>,
    fading: Vec<Fading>,
    /// Kit ids already warned about; a `$DSL` re-fires every animation cycle.
    complained: HashSet<u32>,
    /// The last logged `(kit id, live)` census, so the log line reports edges only.
    last_census: Vec<(u32, bool)>,
    /// The last logged ids withheld by the cap, part of the same edge key.
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
    /// The compare-and-swap registrar: a placed doodad's `$DSL` (`0x69521d`) and the display-slot
    /// loop lane (`0x5f4083`). The same id only moves the record, so a loop never retriggers on
    /// wrap; a different id releases and re-registers, so `bellows.m2`'s two `$DSL` ids alternate
    /// through one slot.
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
            return; // all 32 entries hold other ids: untracked until one frees
        };
        if self.entries[e].id == 0 {
            self.entries[e].id = id;
            self.entries[e].failed = false;
        }
        if self.entries[e].records.len() >= RECORDS_PER_ENTRY {
            // `0x461a60`: evict the first record, in claim order, farther than the newcomer, or
            // reject the newcomer when none is.
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

    /// A GameObject's `$DSL` (`0x5f3fe5`): registers when it holds nothing, else only repositions;
    /// it never compares the id, so a second id (Onyxia's lava trap's `$DSL(8682)`) never
    /// displaces the first. Only the state dispatch or despawn clears it: GameObjects have no
    /// `$DSE` arm.
    fn register_keeping_first(&mut self, owner: Entity, id: u32, pos: Vec3, listener: Vec3) {
        if let Some(&e) = self.handles.get(&owner) {
            self.reposition(e, owner, pos);
            return;
        }
        self.register(owner, id, pos, listener);
    }

    /// Move `owner`'s record in entry `e`, leaving its id and channel alone (`0x462000`).
    fn reposition(&mut self, e: usize, owner: Entity, pos: Vec3) {
        if let Some(r) = self.entries[e]
            .records
            .iter_mut()
            .find(|r| r.owner == owner)
        {
            r.pos = pos;
        }
    }

    /// Release `owner`'s record (`0x461f80` → `0x461d20`); the last one frees the entry and fades
    /// its channel. Reached from a doodad's `$DSE`, a GameObject's state dispatch (`0x5f3cc8`) and
    /// a despawn (`0x6a0840`). There is no map-change reset: the reference's (`0x461a20`) runs only
    /// at shutdown.
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

    /// The cap's eviction (`0x4619c5`): fade the channel, keep the id and records.
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

/// What the cap walk does with one entry, over ascending index (`0x4619c0`-`0x4619f9`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CapStep {
    /// Not entitled: costs the cap nothing and gets no service.
    Skip,
    /// Under the cap: start or reposition, then count it if its channel is live.
    Service,
    /// Four are sounding: fade this channel and keep the entry (`0x4619c5`).
    Retire,
}

/// One step of the cap walk. `sounding_so_far` counts entries whose channel is live and in
/// range, in claim order: `0x461b40` returns `[+0xE04] != 0 && 0x7a5810(ch) && !0x7a5870(ch)`,
/// and `0x7a5870` is the `DistanceCutoff` bit that `0x7a5000` sets as it stops an out-of-range
/// channel. So a far ambience holds none of the four, but among audible ones a nearer fifth never
/// takes over. A kit with `DistanceCutoff` 0 is never culled (`0x7a5cca`) and holds a slot at
/// any distance.
const fn cap_step(entitled: bool, sounding_so_far: usize) -> CapStep {
    if !entitled {
        CapStep::Skip
    } else if sounding_so_far >= PLAYING_CAP {
        CapStep::Retire
    } else {
        CapStep::Service
    }
}

/// The pump (`0x461990`): fade the orphans, mark unresolvable kits, then service entries under
/// the cap, starting a channel where there is none and otherwise only moving it.
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
    // With no device a start cannot succeed; retrying it per frame would flood the log.
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
        // Faded out, or already culled or stolen: let the entity go now.
        if f.gain <= 0.0 || !source_kit_playing(&out, f.emitter, f.kit) {
            stop_source(&mut out, f.emitter);
            commands.entity(f.emitter).despawn();
            return false;
        }
        set_source_kit_gain(&mut out, f.emitter, f.kit, f.gain);
        true
    });

    // `0x45cda0(id)` finds no row: failed for good ([`Entry::failed`]). Before the cap walk, so a
    // dead id never costs a live one its slot.
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

    // The cap walk ([`cap_step`]): an out-of-range entry stays positioned but costs no slot.
    let mut sounding = 0usize;
    let mut census: Vec<(u32, bool)> = Vec::new();
    let mut withheld: Vec<u32> = Vec::new();
    for e in 0..POOL_ENTRIES {
        match cap_step(pool.entries[e].entitled(), sounding) {
            CapStep::Skip => {
                pool.retire(e);
                continue;
            }
            CapStep::Retire => {
                withheld.push(pool.entries[e].id);
                pool.retire(e);
                continue;
            }
            CapStep::Service => {}
        }
        let id = pool.entries[e].id;
        let Some(nearest) = pool.entries[e].nearest(listener_pos) else {
            continue; // unreachable: `entitled()` guarantees a record
        };

        // Move the voice to the nearest record; a live channel follows without restarting.
        let emitter = match pool.entries[e].voice {
            Some(em) => {
                match emitters.get_mut(em) {
                    // Compare first: a no-op write still re-propagates the transform.
                    Ok(mut tf) => {
                        if tf.translation != nearest {
                            tf.translation = nearest;
                        }
                    }
                    // Only a query-filter mistake reaches this, freezing the ambience at its
                    // first emitter, which is inaudible, so it warns.
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
        // No channel: never started, or culled past the kit's `DistanceCutoff`, which the
        // reference's cull also stops and re-plays back in range (`0x7a5000`).
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
                // This lane always loops: `0x7a54d0` builds mode `0x1002` (`HW3D|LOOP_NORMAL`),
                // whatever `SoundEntries.Flags` says; the one-shot path (`0x7a5490`) never does.
                force_loop: true,
                // `NO_DUPLICATES` (0x20) is the one-shot lane's (`0x458f40` → `0x7a66a0`); this
                // lane dedupes by entry and must not be blocked by its own fading predecessor.
                dedupe_exempt: true,
                ..default()
            },
        ) {
            if pool.complained.insert(id) {
                warn!("doodad emitter kit {id}: {err:#} (further reports for this kit suppressed)");
            }
        }
        // Read back, not predicted: `play_kit_ext` succeeds without playing past the cutoff,
        // under the loading cover or at the voice ceiling, and liveness is the cap's counter.
        let live = source_kit_playing(&out, emitter, id);
        if live {
            sounding += 1;
        }
        census.push((id, live));
    }

    // Log the pool on each change: sounding, serviced but out of range (no slot spent), and
    // withheld by the cap, each by name.
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

/// Register `owner`'s emitter for kit `id` at `pos`, compare-and-swap: a placed doodad's `$DSL`
/// (`0x6951e0`) and a GameObject's looping display slot (`0x5f4010`).
pub(super) fn register(
    pool: &mut AmbientEmitterPool,
    owner: Entity,
    id: u32,
    pos: Vec3,
    listener: Vec3,
) {
    pool.register(owner, id, pos, listener);
}

/// Register `owner`'s emitter, keeping any id it already holds: a GameObject's `$DSL`
/// (`0x5f3fe5`).
pub(super) fn register_keeping_first(
    pool: &mut AmbientEmitterPool,
    owner: Entity,
    id: u32,
    pos: Vec3,
    listener: Vec3,
) {
    pool.register_keeping_first(owner, id, pos, listener);
}

/// Release `owner`'s emitter: a placed doodad's `$DSE`, or a GameObject's state dispatch
/// (`0x5f40c0`).
pub(super) fn release(pool: &mut AmbientEmitterPool, owner: Entity) {
    pool.release(owner);
}

/// Release the records of despawned owners (`0x7133a0`'s teardown leg for a doodad host); the
/// sound stops only with the last emitter of its id.
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
            // Registrars and the despawn release, then the pump, then the frame pump: an
            // arbitrary order would cost a frame of latency on a stop.
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

    /// Entities are only identities here: the pool never looks one up.
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

    /// N doodads naming one kit share one entry, and so one channel.
    #[test]
    fn every_doodad_naming_one_kit_shares_a_single_entry() {
        let mut pool = AmbientEmitterPool::default();
        let ds = doodads(30);
        for (i, d) in ds.iter().enumerate() {
            pool.register(*d, 3378, Vec3::new(i as f32, 0.0, 0.0), Vec3::ZERO);
        }
        assert_eq!(pool.entries.iter().filter(|e| e.id != 0).count(), 1);
        assert_eq!(pool.entries[0].records.len(), 30);
        // Its channel goes to the nearest of the thirty.
        assert_eq!(
            pool.entries[0]
                .nearest(Vec3::new(29.0, 0.0, 0.0))
                .unwrap()
                .x,
            29.0
        );
        assert_eq!(pool.entries[0].nearest(Vec3::ZERO).unwrap().x, 0.0);
    }

    /// `0x461cf6` discards a candidate that is not strictly closer.
    #[test]
    fn a_tie_for_nearest_keeps_the_first_record() {
        let mut pool = AmbientEmitterPool::default();
        let ds = doodads(2);
        pool.register(ds[0], 7, Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
        pool.register(ds[1], 7, Vec3::new(0.0, 0.0, -5.0), Vec3::ZERO);
        assert_eq!(pool.entries[0].nearest(Vec3::ZERO).unwrap().z, 5.0);
    }

    /// The same id only repositions (`0x462000`).
    #[test]
    fn re_firing_the_same_id_only_moves_the_record() {
        let mut pool = AmbientEmitterPool::default();
        let d = doodads(1)[0];
        pool.register(d, 3378, Vec3::ZERO, Vec3::ZERO);
        pool.register(d, 3378, Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO);
        assert_eq!(pool.entries[0].records.len(), 1);
        assert_eq!(pool.entries[0].records[0].pos, Vec3::new(1.0, 2.0, 3.0));
    }

    /// `0x5f3fe5` never swaps: Onyxia's lava trap's `$DSL(8682)` on Custom0 never displaces its
    /// Stand `$DSL(8681)`.
    #[test]
    fn the_gameobject_arm_keeps_the_id_it_first_registered() {
        let mut pool = AmbientEmitterPool::default();
        let go = doodads(1)[0];
        pool.register_keeping_first(go, 8681, Vec3::ZERO, Vec3::ZERO);
        pool.register_keeping_first(go, 8682, Vec3::new(1.0, 0.0, 0.0), Vec3::ZERO);
        assert_eq!(pool.entries.iter().filter(|e| e.id != 0).count(), 1);
        assert_eq!(pool.entries[0].id, 8681, "8682 must not displace it");
        // The second marker still moved the record.
        assert_eq!(pool.entries[0].records[0].pos, Vec3::new(1.0, 0.0, 0.0));
        pool.release(go);
        pool.register_keeping_first(go, 8682, Vec3::ZERO, Vec3::ZERO);
        assert_eq!(pool.entries[0].id, 8682);
    }

    /// A GameObject's `$DSL` and looping display slot share `[handler+0x18]`, and the display
    /// slot's compare-and-swap displaces.
    #[test]
    fn the_display_slot_lane_shares_the_owners_one_handle() {
        let mut pool = AmbientEmitterPool::default();
        let go = doodads(1)[0];
        pool.register_keeping_first(go, 3880, Vec3::ZERO, Vec3::ZERO); // its `$DSL`
        pool.register(go, 4694, Vec3::ZERO, Vec3::ZERO); // a looping display slot
        assert_eq!(pool.entries.iter().filter(|e| e.id != 0).count(), 1);
        assert_eq!(pool.entries[0].id, 4694);
    }

    /// `bellows.m2`: two `$DSL` ids alternate through the doodad's one handle.
    #[test]
    fn a_different_id_moves_the_doodads_one_registration() {
        let mut pool = AmbientEmitterPool::default();
        let d = doodads(1)[0];
        pool.register(d, 1000, Vec3::ZERO, Vec3::ZERO);
        pool.register(d, 2000, Vec3::ZERO, Vec3::ZERO);
        assert_eq!(pool.entries.iter().filter(|e| e.id != 0).count(), 1);
        assert_eq!(pool.entries.iter().find(|e| e.id != 0).unwrap().id, 2000);
    }

    /// Releasing the last record frees the entry (`0x461d20`).
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

    /// `0x461e60`: a 33rd id evicts nothing; its re-firing `$DSL` takes the next free entry.
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

    /// `0x461a60`: the first record in claim order farther than the newcomer goes.
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

        // A newcomer at x = 10: record 0 (x = 255) is the first farther one and goes.
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

    /// A newcomer farther than every incumbent is rejected.
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

    /// [`cap_step`] folded as [`pump_emitters`] folds it, with liveness given; returns
    /// `(sounding, withheld)` indices.
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

    /// The cap picks by entry index and stops at four sounding entries.
    #[test]
    fn the_cap_admits_the_first_four_sounding_entries_by_index() {
        let pool = pool_with(&[(10, 1), (20, 1), (30, 1), (40, 1), (50, 1), (60, 1)]);
        let (sounding, withheld) = walk(&pool, |_| true);
        assert_eq!(sounding, vec![0, 1, 2, 3]);
        assert_eq!(withheld, vec![4, 5]);
    }

    /// A serviced entry with no live channel consumes no slot (`0x461b40`, `0x7a5870`). The ids are
    /// four out-of-range Stratholme ambiences ahead of an in-range fire.
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

    /// Free, emitter-less and failed entries cost the cap nothing.
    #[test]
    fn unentitled_entries_do_not_consume_a_cap_slot() {
        let mut pool = pool_with(&[(10, 1), (0, 0), (30, 0), (40, 1), (50, 1), (60, 1), (70, 1)]);
        pool.entries[3].failed = true;
        let (sounding, _) = walk(&pool, |_| true);
        assert_eq!(sounding, vec![0, 4, 5, 6]);
    }

    /// Among audible entries distance does not arbitrate: a fifth on the listener stays silent
    /// behind four earlier ones still inside their cutoffs.
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
