//! Death and resurrection: the wire-fed stores, the events into the Lua UI and the intents back
//! to the wire. Dead is health 0 and ghost is `PLAYER_FLAGS` bit 0x10, read off the self
//! descriptor each frame; `PLAYER_DEAD` signals inside our own death's handler
//! ([`signal_death_state`]) and at a VM's first sight of the body dead, and `PLAYER_ALIVE` on
//! release and on a pre-release res.
//! `PLAYER_UNGHOST` is the ghost state's (`0x5de9c0`), signalled where it runs: the ghost bit's
//! clear ([`crate::field_watch`]) and an alive arrival (`crate::ui_unit`).

use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_ui::script::{DeathAction, DeathUiState, ScriptValue, UiScript};

use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfGuid, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_action::Spells;
use crate::ui_script::UiFeed;

pub(crate) mod net;

/// Where our corpse is, the `MSG_CORPSE_QUERY` answer, in raw WoW coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CorpsePoint {
    /// The map to route toward; for a corpse inside an instance, the entrance's.
    pub(crate) display_map: i32,
    /// The corpse, or the dungeon entrance standing in for it.
    pub(crate) position: [f32; 3],
    /// The corpse's real map id, never adjusted.
    #[allow(dead_code)] // for the map markers
    pub(crate) corpse_map: u32,
}

/// A pending resurrection offer (`SMSG_RESURRECT_REQUEST`), the RESURRECT popup's data.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResurrectOffer {
    /// Answered in `CMSG_RESURRECT_RESPONSE`.
    pub(crate) caster: u64,
    /// Empty on the wire for a player caster, resolved through the name cache.
    pub(crate) name: String,
    /// Accepting applies resurrection sickness; picks the popup variant.
    pub(crate) sickness: bool,
    /// The reclaim-delay gate still holds: RESURRECT, not RESURRECT_NO_TIMER.
    pub(crate) has_timer: bool,
}

/// The release timer our own death arms (`0x5e99b0`, `0xb4e340` and its no-timer byte
/// `0xb4e344`), which `GetReleaseTimeRemaining` reads (`0x4962a0`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum ReleaseTimer {
    /// No death this session armed it.
    #[default]
    Unset,
    /// Released at this real time: the death's time plus 360000 ms (`0x5e9a30` → `0x496250`).
    At(f64),
    /// No timer (`0x496280`): `GetReleaseTimeRemaining` answers -1.
    Off,
}

/// The release window `0x5e9a30` arms, 0x57e40 ms.
const RELEASE_WINDOW_SECS: f64 = 360.0;

/// The death state the descriptor does not carry, filled from the wire and cleared at session
/// teardown.
#[derive(Resource, Default)]
pub(crate) struct DeathNet {
    /// The release timer our last death this session armed.
    pub(crate) release: ReleaseTimer,
    /// When the corpse becomes reclaimable: `SMSG_CORPSE_RECLAIM_DELAY` anchored at arrival.
    pub(crate) reclaim_at: Option<f64>,
    /// Bumped per `SMSG_CORPSE_RECLAIM_DELAY`: the 1.12 client's handler re-fires the range
    /// events on each.
    pub(crate) reclaim_generation: u32,
    /// The last `MSG_CORPSE_QUERY` answer; a not-found drops it.
    pub(crate) corpse: Option<CorpsePoint>,
    /// A pending resurrection offer; cleared when answered or when the popup times out.
    pub(crate) resurrect: Option<ResurrectOffer>,
    /// Bumped per `SMSG_RESURRECT_REQUEST`: the offer announces per message (`0x5e7bc0` through
    /// `0x5ded50`), because the stock UI can hide the popup without answering it.
    pub(crate) resurrect_generation: u32,
    /// Our streamed corpse object's guid, which `CMSG_RECLAIM_CORPSE` carries like the 1.12 client.
    pub(crate) corpse_guid: Option<u64>,
    /// The spirit healer awaiting the XP-loss answer; Accept sends `CMSG_SPIRIT_HEALER_ACTIVATE`.
    pub(crate) spirit_healer: Option<u64>,
    /// Bumped per ask: a Cancel clears nothing, so only a fresh ask re-shows the dialog.
    pub(crate) confirm_generation: u32,
    /// Water-walking is granted on our mover; mirrored, not applied: walking on the water surface
    /// is not built.
    #[allow(dead_code)]
    pub(crate) water_walk: bool,
}

impl DeathNet {
    /// Our own death's timer, by `PLAYER_FIELD_BYTES` bit 0x8 as the death finds it (`0x5e9a27`):
    /// armed at `now` plus the window when set, else off.
    pub(crate) fn arm_release(&mut self, timer_bit: bool, now: f64) {
        self.release = if timer_bit {
            ReleaseTimer::At(now + RELEASE_WINDOW_SECS)
        } else {
            ReleaseTimer::Off
        };
    }

    /// Arm the spirit healer's XP-loss question. The reference raises it on the click with no
    /// packet (`0x5df730`), and vmangos also pushes `SMSG_SPIRIT_HEALER_CONFIRM`; both come here.
    pub(crate) fn ask_spirit_healer(&mut self, npc: u64) {
        self.spirit_healer = Some(npc);
        self.confirm_generation = self.confirm_generation.wrapping_add(1);
    }
}

/// The feed's memory in two scopes: the body's state and the release window's anchor survive a
/// `/reload`, while what the VM was told dies with the VM.
#[derive(Resource, Default)]
struct DeathFeedState {
    /// World-scoped: the last `(guid, dead, ghost)` the self body showed.
    mirror: Option<(u64, bool, bool)>,
    /// World-scoped: when the body was first seen dead, the release window's anchor while no
    /// death this session armed the timer ([`ReleaseTimer::Unset`]).
    died_at: Option<f64>,
    /// VM-scoped: the announce latches.
    vm: crate::ui_script::VmMemo<DeathAnnounced>,
}

/// What the live VM has been told; a first snapshot counts as an edge, so logging in or
/// reloading dead brings the popup up.
///
/// Deviation: a `/reload` dead keeps the release window and signals `PLAYER_DEAD` again, where
/// the reference zeroes the timer at every UI start (`0x490131`) and its cascade never calls
/// `0x495850` (`0x490168`), because without both a player who reloads dead has no Release.
#[derive(Default)]
struct DeathAnnounced {
    /// The last `(guid, dead, ghost)` this VM was given an event for.
    last: Option<(u64, bool, bool)>,
    /// The last corpse-range verdict announced.
    corpse_range: Option<CorpseRange>,
    /// A fresh reclaim delay re-fires the range events, as the 1.12 client does (`0x4962d0`),
    /// so RECOVER_CORPSE re-shows with the new delay.
    reclaim_generation: u32,
    /// Held back while a player caster's name is still resolving.
    offer_generation: u32,
    /// The offer whose caster's name was looked up as its packet's (`0x5e7c82`, callback
    /// `0x5e7ca0`): the frames that wait on it read.
    offer_asked: u32,
    /// A fresh ask re-fires `CONFIRM_XP_LOSS`.
    confirm_generation: u32,
}

/// The corpse-range verdicts the reference's events distinguish.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CorpseRange {
    Out,
    In,
    InInstance,
}

/// The corpse-range dialog radius, 40 yd inclusive: the 1.12 client compares d² ≤ `[0xb4e2ac]`
/// (1600.0) against the corpse-query position. The server's reclaim gate,
/// `CORPSE_RECLAIM_RADIUS` 39 (`Corpse.h:40`), sits 1 yd inside it.
const CORPSE_RANGE_SQ: f32 = 40.0 * 40.0;

/// The spirit-healer dialog range: `CheckSpiritHealerDist` compares d² ≤ `[0xc4c28c]` (30.864).
const SPIRIT_HEALER_RANGE_SQ: f32 = 5.5556 * 5.5556;

/// What the death snapshot reads ([`DeathView::snapshot`]): the self body, the death stores and
/// what `HasSoulstone()` walks.
#[derive(bevy::ecs::system::SystemParam)]
struct DeathView<'w, 's> {
    self_q: Query<'w, 's, (&'static ObjectStore, &'static Transform), With<SelfPlayer>>,
    self_guid: Res<'w, SelfGuid>,
    death_net: Res<'w, DeathNet>,
    // Real time: `reclaim_at` is stamped on `Time<Real>` by the net path.
    time: Res<'w, Time<Real>>,
    objects: Objects<'w, 's>,
    transforms: Query<'w, 's, &'static Transform>,
    status: Res<'w, crate::net::NetStatus>,
    spells: Option<Res<'w, Spells>>,
    items: Res<'w, crate::items::Items>,
}

impl DeathView<'_, '_> {
    /// The live session's self body: its guid, descriptor and place. A disconnect keeps the self
    /// avatar with its frozen descriptor; only a live session's may drive the state machine.
    fn body(&self) -> Option<(u64, &ObjectStore, &Transform)> {
        if !self.status.connected {
            return None;
        }
        let guid = self.self_guid.0?;
        let (store, self_t) = self.self_q.single().ok()?;
        Some((guid, store, self_t))
    }

    /// The snapshot the death verbs read, pushed before an event fires so its handlers read
    /// current values. `died_at` anchors the release window while no death armed it.
    fn snapshot(
        &self,
        script: &UiScript,
        store: &ObjectStore,
        self_t: &Transform,
        died_at: Option<f64>,
    ) -> DeathUiState {
        let now = self.time.elapsed_secs_f64();
        let death_net = &self.death_net;
        let release_remaining = match death_net.release {
            ReleaseTimer::At(at) => Some((at - now).max(0.0) as f32),
            ReleaseTimer::Off => None, // GetReleaseTimeRemaining() == -1, the no-timer DEATH text
            // Deviation: with no death this session to arm it (dead at login or after a
            // `/reload`), the window runs from the first sight of the body dead, while the bit is
            // set. The reference zeroes the timer at every UI start (`0x490131`) and answers 0
            // (`0x4962a0`), so its DEATH popup never shows and the player has no Release.
            ReleaseTimer::Unset if store.0.player_release_timer_running() => {
                died_at.map(|t| ((t + RELEASE_WINDOW_SECS - now).max(0.0)) as f32)
            }
            ReleaseTimer::Unset => None,
        };
        let recovery_delay = death_net
            .reclaim_at
            .map_or(0.0, |at| (at - now).max(0.0) as f32);
        let spirit_healer_in_range = death_net.spirit_healer.is_some_and(|npc| {
            self.objects
                .entity(npc)
                .and_then(|e| self.transforms.get(e).ok())
                .is_some_and(|t| {
                    t.translation.distance_squared(self_t.translation) <= SPIRIT_HEALER_RANGE_SQ
                })
        });
        let sickness = sickness_duration(store.0.unit_level().unwrap_or(0), &|key: &str| {
            script
                .lua()
                .globals()
                .get::<String>(key)
                .ok()
                .filter(|t| !t.is_empty())
        });
        DeathUiState {
            release_remaining,
            recovery_delay,
            resurrect_sickness: death_net.resurrect.as_ref().is_some_and(|o| o.sickness),
            resurrect_has_timer: death_net.resurrect.as_ref().is_some_and(|o| o.has_timer),
            spirit_healer_in_range,
            sickness_duration: sickness,
            // `HasSoulstone()`; the dead gate is first, so the inventory walk runs only while dead.
            self_res_label: resolve_self_res(
                &store.0,
                &self.objects,
                &self.items,
                self.spells.as_deref(),
            )
            .map(|r| r.label().to_owned()),
        }
    }
}

/// Derive the death state from the self descriptor, push the snapshot, and fire the death events
/// on its edges.
fn feed_death(
    // The death stores, which the death calls write: an engine turn, its events signalled after.
    mut vm: crate::script_calls::VmWith<DeathView<'static, 'static>>,
    mut feed: ResMut<DeathFeedState>,
    names: Res<crate::names::NameCache>,
    net: Res<NetCommands>,
    map: Option<Res<benilla_world::world_map::CurrentMap>>,
) {
    vm.turn(|script, view| {
        let Some(script) = script else {
            return;
        };
        let Some((guid, store, self_t)) = view.body() else {
            return;
        };
        let death_net = &view.death_net;
        let now = view.time.elapsed_secs_f64();
        let dead = store.0.unit_is_dead();
        let ghost = store.0.player_is_ghost();

        // ── The snapshot, set before the events fire so their handlers read current values ──
        let snapshot = view.snapshot(script, store, self_t, feed.died_at);
        script.set_death(snapshot);

        // ── The world edges: the release window and the corpse asks, which a `/reload` must not
        // repeat.
        let prev = feed.mirror.filter(|&(g, ..)| g == guid);
        feed.mirror = Some((guid, dead, ghost));
        match (prev, dead, ghost) {
            // Alive to dead, or logging in dead: arm the release window.
            (Some((_, false, false)) | None, true, false) => {
                feed.died_at = Some(now);
            }
            // Dead to ghost (released), or to alive (a pre-release res).
            (Some((_, true, false)), d, g) if g || !d => {
                feed.died_at = None;
                if g {
                    // Now a ghost: ask where the corpse is.
                    let _ = net.0.send(ClientCommand::CorpseQuery);
                }
            }
            // Ghost to alive: reclaim, spirit healer or an accepted res.
            (Some((_, _, true)), false, false) => {
                feed.died_at = None;
                // The not-found answer drops the map markers. The reference sends nothing on this
                // edge: its ghost-bit watcher (`0x5ee990`) clears the corpse cache, and `0x491f50`
                // queries only while a ghost. The server's own push needs a looter
                // (`Map.cpp:3617-3629`).
                let _ = net.0.send(ClientCommand::CorpseQuery);
            }
            // Logging in a ghost: no edge, just the corpse ask.
            (None, _, true) => {
                let _ = net.0.send(ClientCommand::CorpseQuery);
            }
            _ => {}
        }

        // ── The VM edges: a fresh VM's memo is empty, so reloading dead re-fires PLAYER_DEAD
        // (the Deviation on `DeathAnnounced`).
        let memo = feed.vm.get(script);
        let prev = memo.last.filter(|&(g, ..)| g == guid);
        memo.last = Some((guid, dead, ghost));
        fire_vm_edges(script, prev.map(|(_, d, g)| (d, g)), dead, ghost);

        // ── The offer and confirm announcements ──
        let ask = Ask::at(
            std::mem::replace(&mut memo.offer_asked, death_net.resurrect_generation)
                != death_net.resurrect_generation,
            &net,
        );
        if let Some(name) =
            offer_announcement(memo, death_net, store.0.is_dead_or_ghost(), |offer| {
                // A player caster's name is empty on the wire; hold the popup until the cache
                // resolves it.
                if offer.name.is_empty() {
                    names.resolve_player(offer.caster, ask).map(str::to_owned)
                } else {
                    Some(offer.name.clone())
                }
            })
        {
            script.fire_event("RESURRECT_REQUEST", vec![ScriptValue::Str(name)]);
        }
        if death_net.spirit_healer.is_some()
            && memo.confirm_generation != death_net.confirm_generation
        {
            memo.confirm_generation = death_net.confirm_generation;
            // `arg1` is the XP the res costs: `PLAYER_NEXT_LEVEL_XP` × 0.05, truncated
            // (`0x5df837`), and 0 without the field, as `0x5df806` pushes 0 for a non-local object.
            // No stock FrameXML reads it; `UIParent.lua:399` asks `GetResSicknessDuration()`.
            let xp_cost = store
                .0
                .player_next_level_xp()
                .map_or(0, |next| (f64::from(next) * 0.05) as i64);
            script.fire_event("CONFIRM_XP_LOSS", vec![ScriptValue::Int(xp_cost)]);
        }

        // ── The corpse-run range gate: only a ghost, against the query's display position; the
        // coordinate transform is isometric, so the Bevy-space compare is in yards.
        let range = if ghost {
            death_net.corpse.map(|cp| {
                // Within range on the display map: IN_RANGE when it is the corpse's map, else
                // IN_INSTANCE, at a dungeon's entrance (`0x492130`, `0x4920f0`).
                let on_display_map = map
                    .as_ref()
                    .is_some_and(|m| m.0 == u32::try_from(cp.display_map).unwrap_or(u32::MAX));
                let near = on_display_map
                    && wow_to_bevy(cp.position).distance_squared(self_t.translation)
                        <= CORPSE_RANGE_SQ;
                let same_map = u32::try_from(cp.display_map) == Ok(cp.corpse_map);
                match (near, same_map) {
                    (false, _) => CorpseRange::Out,
                    (true, true) => CorpseRange::In,
                    (true, false) => CorpseRange::InInstance,
                }
            })
        } else {
            None
        };
        if memo.reclaim_generation != death_net.reclaim_generation {
            memo.reclaim_generation = death_net.reclaim_generation;
            memo.corpse_range = None; // re-announce whatever holds now
        }
        if range != memo.corpse_range {
            match range {
                Some(CorpseRange::In) => script.fire_event("CORPSE_IN_RANGE", vec![]),
                Some(CorpseRange::InInstance) => script.fire_event("CORPSE_IN_INSTANCE", vec![]),
                // Leaving range and the ghost state ending both fire the one hide event.
                Some(CorpseRange::Out) | None if memo.corpse_range.is_some() => {
                    script.fire_event("CORPSE_OUT_OF_RANGE", vec![]);
                }
                _ => {}
            }
            memo.corpse_range = range;
        }
    })
}

/// `SignalDeathState 0x495850`, where our own death's `PresentUnitDeath` ends (`0x6251cc`, for
/// the active player alone, `0x5fa6d0` at `0x6251c2`): with our health 0 or less (`jg` at
/// `0x495876`) `PLAYER_DEAD` (`0x113`, `0x49587d`), its handlers reading the timer the death armed,
/// then `ClearCursor(1, 1)` (`0x495889` → `0x495190`); else `PLAYER_ALIVE` (`0x112`, `0x495893`).
/// The VM's memo takes the state, so the feed signals no edge of it again.
fn signal_death(
    mut vm: crate::script_calls::VmWith<DeathView<'static, 'static>>,
    mut feed: ResMut<DeathFeedState>,
) {
    let dead = {
        let (Some(script), set) = vm.parts() else {
            return;
        };
        let view = set.p0();
        let Some((guid, store, self_t)) = view.body() else {
            return;
        };
        let snapshot = view.snapshot(script, store, self_t, feed.died_at);
        script.set_death(snapshot);
        let memo = feed.vm.get(script);
        memo.last = Some((guid, store.0.unit_is_dead(), store.0.player_is_ghost()));
        store.0.unit_health().unwrap_or(0) as i32 <= 0
    };
    // `0x495850`: the signal's listeners run before the cursor clears (`0x49587d`, `0x495889`).
    vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        if dead {
            script.fire_event("PLAYER_DEAD", vec![]);
            script.clear_cursor_payload();
        } else {
            script.fire_event("PLAYER_ALIVE", vec![]);
        }
    });
}

/// [`signal_death`] in place, from our own death's handler (`crate::target`'s `died`).
pub(crate) fn signal_death_state(world: &mut World) {
    if let Err(e) = world.run_system_cached(signal_death) {
        // A harness without the death state signals nothing.
        debug!("death: the death state was not signalled: {e}");
    }
}

/// `SPELL_EFFECT_SELF_RESURRECT`, the `Spell.dbc` effect the item leg scans for (`0x5ed650`).
const SPELL_EFFECT_SELF_RESURRECT: u32 = 94;

/// What `UseSoulstone()` (`0x48ad70`) would spend now; resolved app-side, where the spell catalog
/// and item cache live.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SelfRes {
    /// `PLAYER_SELF_RES_SPELL` is non-zero: send `CMSG_SELF_RES` and the server casts it.
    Spell {
        #[allow(dead_code)] // the server reads its own field
        spell: u32,
        label: String,
    },
    /// The field is zero but a carried item's on-use spell self-resurrects: use the item, named
    /// for the item.
    Item {
        bag_index: u8,
        slot: u8,
        guid: u64,
        entry: u32,
        label: String,
    },
}

impl SelfRes {
    /// What `HasSoulstone()` returns.
    fn label(&self) -> &str {
        match self {
            Self::Spell { label, .. } | Self::Item { label, .. } => label,
        }
    }
}

/// `HasSoulstone()`'s answer (`0x48ac80`): nil while `UNIT_FIELD_HEALTH` is positive, a released
/// ghost's health being 1, else [`self_res`].
fn resolve_self_res(
    store: &benilla_protocol::ObjectFields,
    objects: &Objects,
    items: &crate::items::Items,
    spells: Option<&Spells>,
) -> Option<SelfRes> {
    if !store.unit_is_dead() {
        return None;
    }
    self_res(store, objects, items, spells)
}

/// The self-resurrection `HasSoulstone()` names and `UseSoulstone()` spends (`0x48ac80`,
/// `0x48ad70`), with its gates in its order:
///
/// 1. The field: non-zero is the spell leg, named from `Spell.dbc`, `"UNKNOWN"` when unresolved.
/// 2. On a zero field, the carried inventory at the walker's default mask (`0x47`) for an item
///    whose on-use spell carries [`SPELL_EFFECT_SELF_RESURRECT`] (`0x5ed630`).
///
/// An item template still in flight reads as no self-res until it lands: the item test only reads
/// (`0x5ed68e`).
fn self_res(
    store: &benilla_protocol::ObjectFields,
    objects: &Objects,
    items: &crate::items::Items,
    spells: Option<&Spells>,
) -> Option<SelfRes> {
    if let Some(spell) = store.player_self_res_spell() {
        let label = spells
            .and_then(|s| s.catalog.get(spell))
            .map_or_else(|| "UNKNOWN".to_owned(), |d| d.name.clone());
        return Some(SelfRes::Spell { spell, label });
    }
    let slots = crate::ui_items::collect_inventory(
        store,
        objects,
        crate::ui_items::InventoryScope::DEFAULT,
    );
    slots.into_iter().find_map(|(bag_index, slot, guid)| {
        let entry = objects.object(guid)?.object_entry()?;
        let t = items.template(entry, guid, Ask::Read)?;
        let self_res = t.spells.iter().any(|sp| {
            sp.trigger == 0
                && spells.is_some_and(|c| {
                    c.catalog
                        .get(sp.spell_id)
                        .is_some_and(|d| d.effects.contains(&SPELL_EFFECT_SELF_RESURRECT))
                })
        });
        self_res.then(|| SelfRes::Item {
            bag_index,
            slot,
            guid,
            entry,
            label: t.name.clone(),
        })
    })
}

/// The sickness a spirit-healer res applies at `level`, per vmangos `Player::ResurrectPlayer`:
/// none below 11, `level - 10` minutes through 19, 10 minutes from 20. Worded with `GENERIC_MIN`,
/// the minute arm of the reference's formatter (`0x51a3a0` into `0x52fa50`).
///
/// Deviation: the minutes come from the server's table, not from spell 15007's `Spell.dbc`
/// duration, because the aura the server applies is what the dialog must warn about.
fn sickness_duration(level: u32, get: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let minutes = match level {
        0..=10 => return None,
        11..=19 => level - 10,
        _ => 10,
    };
    // `GetText(token, nil, ordinal)`'s plural pick, through the shared primitive.
    let template = benilla_ui::strings::plural("GENERIC_MIN", Some(minutes), get)?;
    Some(benilla_ui::strings::fill(
        &template,
        &[benilla_ui::strings::Arg::D(i64::from(minutes))],
    ))
}

/// Drive the FFXDeath screen pass off `PLAYER_FLAGS_GHOST`, instant both ways; in the reference
/// the flag's watcher `0x5ee990` also drives the death light and ambience.
fn drive_death_look(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut ffx: ResMut<benilla_world::ffx_glow::FfxDeathFade>,
) {
    let ghost = ghost_probe()
        .unwrap_or_else(|| self_q.single().is_ok_and(|store| store.0.player_is_ghost()));
    let target = if ghost { 1.0 } else { 0.0 };
    if ffx.0 != target {
        ffx.0 = target;
    }
}

/// `WOW_GHOST_PROBE=1|0`: pin the ghost-world look on or off without dying: the screen pass,
/// the death light (`LightParams` slot 4) and the DeathClouds sky. The death events still follow
/// the wire.
pub(crate) fn ghost_probe() -> Option<bool> {
    static PROBE: std::sync::OnceLock<Option<bool>> = std::sync::OnceLock::new();
    *PROBE.get_or_init(|| match std::env::var("WOW_GHOST_PROBE").ok()?.trim() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    })
}

/// The death verbs inside their calls ([`DeathAction`]): their gates, their packets and the
/// soulstone's item use, and the area spirit healer's two.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DeathSteps<'w, 's> {
    /// Absent in a world with no death stores, which drops the calls.
    death_net: Option<ResMut<'w, DeathNet>>,
    spirit: Option<ResMut<'w, crate::ui_dialog_verbs::AreaSpiritHealer>>,
    // `AcceptAreaSpiritHeal`'s scan ahead of its send (`0x492316`).
    spirit_scan: crate::ui_dialog_verbs::SpiritScan<'w, 's>,
    targeting: crate::spell::cast_target::CastTargeting<'w, 's>,
    cast: crate::script_calls::disjoint::Pair<
        'w,
        's,
        crate::spell::CastLadder<'w, 's>,
        crate::spell::SelfCancel<'w>,
    >,
    gate: crate::ui_bind_confirm::BindGate<'w>,
    ui_errors: Res<'w, crate::ui_action::UiErrorKeys>,
    guids: Option<Res<'w, crate::net::GuidIndex>>,
    transforms: Query<'w, 's, &'static Transform>,
    me: Query<'w, 's, Entity, With<SelfPlayer>>,
    net: Res<'w, NetCommands>,
}

impl DeathSteps<'_, '_> {
    /// `action` inside its verb.
    pub(crate) fn apply(&mut self, script: &mut UiScript, action: DeathAction) {
        let store = self.targeting.self_store.single().ok().map(|s| s.0.clone());
        let send = |net: &NetCommands, command| {
            let _ = net.0.send(command);
        };
        match action {
            // `0x5e0ae6`: only while dead or a ghost (`0x605f30`).
            DeathAction::Repop => {
                if store.as_ref().is_some_and(|s| s.is_dead_or_ghost()) {
                    send(&self.net, ClientCommand::RepopRequest);
                }
            }
            // Ungated; with no corpse the guid is 0, which vmangos never reads: it resolves the
            // corpse through the player (`MiscHandler.cpp:573-603`).
            DeathAction::RetrieveCorpse => {
                let corpse = self
                    .death_net
                    .as_deref()
                    .and_then(|d| d.corpse_guid)
                    .unwrap_or(0);
                send(&self.net, ClientCommand::ReclaimCorpse { corpse });
            }
            // With the active player (`0x48a9bd`) and an offer held (`0x5ded91`), the answer, and
            // the offer forgotten (`0x5dedf2`).
            DeathAction::AcceptResurrect | DeathAction::DeclineResurrect => {
                if store.is_none() {
                    return;
                }
                let accept = action == DeathAction::AcceptResurrect;
                if let Some(offer) = self
                    .death_net
                    .as_deref_mut()
                    .and_then(|d| d.resurrect.take())
                {
                    send(
                        &self.net,
                        ClientCommand::ResurrectResponse {
                            caster: offer.caster,
                            accept,
                        },
                    );
                }
            }
            DeathAction::AcceptXpLoss => self.accept_xp_loss(store.as_ref()),
            // `0x492310`: the cached healer's guid, nothing without one (`0x492326`).
            // The scan ran first ([`Self::rescan_spirit`]); then 0x2e3 with the healer it left
            // (`0x49231b`-`0x49235f`), nothing with none.
            DeathAction::AcceptAreaSpiritHeal => {
                if let Some(healer) = self.spirit.as_deref().and_then(|s| s.healer()) {
                    send(&self.net, ClientCommand::AreaSpiritHealerQueue { healer });
                }
            }
            DeathAction::UseSoulstone { .. } => {
                if let Some(store) = store {
                    self.use_soulstone(script, &store);
                }
            }
        }
    }

    /// `AcceptAreaSpiritHeal`'s scan (`0x492310` → `0x4923b0`): the healer changes it makes, in
    /// order, for the host to apply with its signal point.
    pub(crate) fn rescan_spirit(&mut self) -> Vec<crate::ui_dialog_verbs::SetHealerOutcome> {
        match self.spirit.as_deref_mut() {
            Some(spirit) => self.spirit_scan.run(spirit),
            None => Vec::new(),
        }
    }

    /// The socket the death verbs send on.
    pub(crate) fn net(&self) -> &NetCommands {
        &self.net
    }

    /// `0x5df730(0)`: for a ghost (`0x5df74a`) whose remembered healer is a unit in the
    /// interaction range (`0x5df7c1`, `[0xc4c28c]`), `CMSG_SPIRIT_HEALER_ACTIVATE` with it; the guid
    /// stays (`0x5df7e4` stores only a nonzero argument).
    fn accept_xp_loss(&self, store: Option<&benilla_protocol::ObjectFields>) {
        if !store.is_some_and(|s| s.player_is_ghost()) {
            return;
        }
        let Some(npc) = self.death_net.as_deref().and_then(|d| d.spirit_healer) else {
            return;
        };
        let place = |entity: Option<Entity>| {
            entity
                .and_then(|e| self.transforms.get(e).ok())
                .map(|t| t.translation)
        };
        let healer = place(self.guids.as_deref().and_then(|g| g.0.get(&npc).copied()));
        let me = place(self.me.iter().next());
        let in_range = healer
            .zip(me)
            .is_some_and(|(h, m)| h.distance_squared(m) <= SPIRIT_HEALER_RANGE_SQ);
        if in_range {
            let _ = self.net.0.send(ClientCommand::SpiritHealerActivate { npc });
        }
    }

    /// `UseSoulstone()` (`0x48ad70`), with no health test: with `PLAYER_SELF_RES_SPELL` set,
    /// `CMSG_SELF_RES` (`0x48adb6`, `0x48ae05`); else the carried item (`0x5ed630`), the cast
    /// cancelled before its use (`0x48ae6b` into `0x6e4940`, cl=1, dl=1, reason `0x1c`), then an
    /// ordinary item use, `0x5d8d00` (`0x48ae7d`), like every bag click: a locked stone is refused
    /// at its first gate (`0x5d8d32`), after the cancel. Nothing is cleared here: the server zeroes
    /// the field.
    fn use_soulstone(&mut self, script: &mut UiScript, store: &benilla_protocol::ObjectFields) {
        let res = {
            let ladder = self.cast.p0();
            self_res(
                store,
                &ladder.objects,
                &ladder.items,
                ladder.spells.as_deref(),
            )
        };
        let Some(SelfRes::Item {
            bag_index,
            slot,
            guid,
            entry,
            ..
        }) = res
        else {
            if res.is_some() {
                let _ = self.net.0.send(ClientCommand::SelfRes);
            }
            return;
        };
        // Its failed flag disarms the trade-skill repeat on every branch (`0x6e4abc`-`0x6e4ac2`
        // into `0x4fd790`), queued with its event.
        self.cast.p1().abort_cast();
        if crate::ui_items::item_use_refused(script, Some(guid)) {
            debug!("death: soulstone {guid:#x} is locked — refused");
            return;
        }
        let ctx = self.targeting.context();
        let mut ladder = self.cast.p0();
        // The use reads its record (`0x5d8d57`).
        let t = ladder.items.template(entry, guid, Ask::Read).cloned();
        let it = crate::ui_items::ItemUse {
            guid: Some(guid),
            start_quest: t.as_ref().map_or(0, |t| t.start_quest),
            bag_index,
            slot,
            entry,
            spell_index: t.as_ref().and_then(|t| t.use_spell_index()).unwrap_or(0),
            use_spell: t.as_ref().and_then(|t| t.use_spell).map(|u| u.spell_id),
            on_object: None,
            is_charter: false,
        };
        crate::ui_items::send_item_use(
            it,
            &ctx,
            &mut ladder,
            script,
            &mut self.gate,
            false,
            &self.ui_errors,
        );
    }
}

/// Clear [`DeathFeedState`]'s world scope on `DisconnectedMessage`, the edge [`DeathNet`] resets
/// on: a relog as a ghost must find no mirror, or the corpse query is never re-sent.
fn end_session_death_feed(
    mut msgs: MessageReader<crate::net::DisconnectedMessage>,
    mut feed: ResMut<DeathFeedState>,
) {
    if msgs.read().next().is_some() {
        *feed = DeathFeedState::default();
    }
}

/// The death stores, the feed into the UI, the intent drain and the root and water-walk messages.
pub(crate) struct DeathPlugin;

impl Plugin for DeathPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        app.init_resource::<DeathNet>()
            .init_resource::<DeathFeedState>()
            .add_systems(
                Update,
                (
                    feed_death.in_set(UiFeed),
                    drive_death_look,
                    // Before the feed, so the frame a session ends carries no memory of it.
                    end_session_death_feed.before(feed_death),
                ),
            );
    }
}

/// The name `RESURRECT_REQUEST` fires with this frame, if any. Per message, and only while dead
/// or a ghost (`0x5ded50` gated on `0x605f30`); an unresolved name holds without consuming it.
fn offer_announcement(
    memo: &mut DeathAnnounced,
    death_net: &DeathNet,
    dead_or_ghost: bool,
    name_of: impl FnOnce(&ResurrectOffer) -> Option<String>,
) -> Option<String> {
    if !dead_or_ghost || memo.offer_generation == death_net.resurrect_generation {
        return None;
    }
    let name = name_of(death_net.resurrect.as_ref()?)?;
    memo.offer_generation = death_net.resurrect_generation;
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn offer(caster: u64, name: &str) -> ResurrectOffer {
        ResurrectOffer {
            caster,
            name: name.into(),
            sickness: false,
            has_timer: true,
        }
    }

    /// The offer's packet looks a player caster's name up with the callback `0x5e7ca0`
    /// (`0x5e7c82`); the frames the popup waits through read. A refused name is asked by no frame
    /// after it, only by the next offer.
    #[test]
    fn a_refused_resurrecter_is_asked_by_the_next_offer_never_by_a_frame() {
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::ObjectFields;
        const ME: u64 = 0x7;
        const CASTER: u64 = 0x9;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .insert_resource(crate::net::NetStatus {
                connected: true,
                last_reason: None,
            })
            .init_resource::<SelfGuid>()
            .init_resource::<DeathNet>()
            .init_resource::<DeathFeedState>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::items::Items>()
            .init_resource::<Time<Real>>()
            .add_systems(Update, feed_death);
        app.insert_non_send_resource(UiScript::new().unwrap());
        // `UNIT_FIELD_HEALTH` 0 of a `UNIT_FIELD_MAXHEALTH` 100: dead.
        let dead = ObjectFields::from_pairs(&[(22, 0), (28, 100)]).into_created(ObjectType::Player);
        let me = app
            .world_mut()
            .spawn((ObjectStore(dead), Transform::default(), SelfPlayer))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(ME, me);
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
        let offer = |app: &mut App| {
            net::resurrect_request_for_tests(
                CASTER,
                &mut app.world_mut().resource_mut::<DeathNet>(),
            );
            app.update();
        };
        app.update();
        offer(&mut app);
        assert_eq!(crate::query_cache::queries(&rx).len(), 1);
        app.world_mut()
            .resource_mut::<crate::names::NameCache>()
            .insert_player(CASTER, String::new(), None);
        for _ in 0..600 {
            app.update();
        }
        assert!(
            crate::query_cache::queries(&rx).is_empty(),
            "no frame asks a refusal"
        );
        offer(&mut app);
        assert_eq!(
            crate::query_cache::queries(&rx).len(),
            1,
            "the next offer asks again"
        );
    }

    /// `0x495850` signals `PLAYER_DEAD` (`0x49587d`) before it clears the cursor (`0x495889`): a
    /// listener still finds the coins it held.
    #[test]
    fn player_dead_listeners_see_the_cursor_before_it_clears() {
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::ObjectFields;
        const ME: u64 = 0x7;
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut app = App::new();
        app.insert_resource(NetCommands(tx))
            .insert_resource(crate::net::NetStatus {
                connected: true,
                last_reason: None,
            })
            .init_resource::<SelfGuid>()
            .init_resource::<DeathNet>()
            .init_resource::<DeathFeedState>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::items::Items>()
            .init_resource::<Time<Real>>();
        let mut script = UiScript::new().unwrap();
        script.keep_calls();
        script.set_money(1_000);
        script
            .run(
                "HELD = nil local f = CreateFrame('Frame') f:RegisterEvent('PLAYER_DEAD') \
                 f:SetScript('OnEvent', function() HELD = GetCursorMoney() end) \
                 PickupPlayerMoney(10)",
            )
            .unwrap();
        app.insert_non_send_resource(script);
        let dead = ObjectFields::from_pairs(&[(22, 0), (28, 100)]).into_created(ObjectType::Player);
        let me = app
            .world_mut()
            .spawn((ObjectStore(dead), Transform::default(), SelfPlayer))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(ME, me);
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
        signal_death_state(app.world_mut());
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(script.eval::<i64>("return HELD").unwrap(), 10);
        assert!(script.cursor_payload().is_none(), "then the clear");
    }

    /// PLAYER_UNGHOST hides the popup with `StaticPopup_Hide`, which runs no `OnCancel`, so no
    /// Decline clears `resurrect`.
    #[test]
    fn every_resurrect_message_announces_while_dead_and_never_while_alive() {
        let mut net = DeathNet::default();
        let mut memo = DeathAnnounced::default();
        let name = |o: &ResurrectOffer| Some(o.name.clone());

        net.resurrect = Some(offer(1, "Healer"));
        net.resurrect_generation += 1;
        assert_eq!(
            offer_announcement(&mut memo, &net, true, name).as_deref(),
            Some("Healer")
        );
        assert_eq!(
            offer_announcement(&mut memo, &net, true, name),
            None,
            "once per message"
        );

        // Alive with the offer unanswered: a fresh VM must not announce it.
        let mut reloaded = DeathAnnounced::default();
        assert_eq!(offer_announcement(&mut reloaded, &net, false, name), None);

        // Dead again, a new priest offers.
        net.resurrect = Some(offer(2, "Ptwo"));
        net.resurrect_generation += 1;
        assert_eq!(
            offer_announcement(&mut memo, &net, true, name).as_deref(),
            Some("Ptwo"),
            "the second offer announces"
        );

        // A name still resolving holds the announcement without consuming it.
        net.resurrect_generation += 1;
        assert_eq!(offer_announcement(&mut memo, &net, true, |_| None), None);
        assert_eq!(
            offer_announcement(&mut memo, &net, true, name).as_deref(),
            Some("Ptwo")
        );
    }

    const STONE: u64 = 0x4000_0000_0000_5232;
    const SELF_RES: u32 = 20707;
    const FIREBALL: u32 = 133;

    /// A dead player (health 0 of 100) with a soulstone in backpack slot 1, as a quest-starter so
    /// its use sends with no cast ladder to satisfy, and a VM.
    fn soulstone_app() -> (App, crossbeam_channel::Receiver<ClientCommand>) {
        const STONE_ENTRY: u32 = 5232;
        let mut app = crate::game_plugins::schedule_tests::headless_client();
        let (tx, rx) = crossbeam_channel::unbounded();
        app.insert_resource(NetCommands(tx));
        let world = app.world_mut();
        world.spawn((
            SelfPlayer,
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x19),
                (22, 0),
                (28, 100),
                (532, STONE as u32),
                (533, (STONE >> 32) as u32),
            ])),
        ));
        crate::items::test_spawn_item(
            world,
            STONE,
            benilla_protocol::ObjectFields::from_pairs(&[(3, STONE_ENTRY)]),
            false,
        );
        world.resource_mut::<crate::items::Items>().insert_template(
            STONE_ENTRY,
            Some(benilla_protocol::messages::ItemInfo {
                spells: vec![benilla_protocol::messages::ItemSpellEntry {
                    index: 0,
                    spell_id: SELF_RES,
                    trigger: 0,
                    charges: -1,
                    cooldown_ms: -1,
                    category: 0,
                    category_cooldown_ms: -1,
                }],
                start_quest: 42,
                ..crate::items::test_template("Minor Soulstone")
            }),
        );
        let display = benilla_formats::SpellDisplay {
            effects: [SPELL_EFFECT_SELF_RESURRECT, 0, 0],
            ..Default::default()
        };
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays([(SELF_RES, display)].into()),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        let script = UiScript::new().unwrap();
        record_reports(&script);
        world.insert_non_send_resource(script);
        (app, rx)
    }

    /// A recorder of the cast's bar events in `EDGES` and the lines shown in `LINES`, each cast
    /// failure defined as its own key: what a press's call signals inside it.
    fn record_reports(script: &UiScript) {
        let mut lua: String = crate::ui_action::CAST_FAIL_KEYS
            .iter()
            .filter(|k| !k.is_empty())
            .map(|key| format!("{key} = \"{key}\"\n"))
            .collect();
        lua.push_str(
            r#"ERR_SPELL_FAILED_S = "%s"
            EDGES, LINES = {}, {}
            local r = CreateFrame("Frame")
            for _, e in ipairs({"UI_ERROR_MESSAGE", "SPELLCAST_FAILED", "SPELLCAST_INTERRUPTED",
                    "SPELLCAST_STOP"}) do
                r:RegisterEvent(e)
            end
            r:SetScript("OnEvent", function()
                if event == "UI_ERROR_MESSAGE" then
                    table.insert(LINES, arg1)
                else
                    table.insert(EDGES, event)
                end
            end)"#,
        );
        script.run(&lua).expect("the recorder installs");
    }

    /// The lines [`record_reports`] saw since the last read.
    fn lines(app: &mut App) -> Vec<String> {
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .eval("local l = LINES LINES = {} return l")
            .unwrap()
    }

    /// `UseSoulstone()` with the host lent, as a click runs it.
    fn press_soulstone(app: &mut App) {
        crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
            vm.expect("a VM").run("UseSoulstone()")
        })
        .unwrap();
    }

    /// The cast's bar events the press signalled inside its call.
    fn edges(app: &App) -> Vec<&'static str> {
        app.world()
            .non_send_resource::<UiScript>()
            .eval::<Vec<String>>("return EDGES")
            .unwrap()
            .into_iter()
            .map(|e| match e.as_str() {
                "SPELLCAST_STOP" => "STOP",
                "SPELLCAST_FAILED" => "FAILED",
                "SPELLCAST_INTERRUPTED" => "INTERRUPTED",
                _ => "OTHER",
            })
            .collect()
    }

    /// Signal the reports the press queued, as the frame's feed does.
    fn signal(app: &mut App) {
        app.world_mut()
            .run_system_once(crate::ui_cast::signal_reports)
            .unwrap();
    }

    /// `UseSoulstone`'s item leg is `CGItem::Use` (`0x48ae7d`), whose first gate refuses a
    /// locked item (`0x5d8d32`): the stone's use goes out only once the lock lifts.
    #[test]
    fn a_locked_soulstone_is_not_used() {
        let (mut app, rx) = soulstone_app();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(STONE);
        let _ = rx.try_iter().count();
        press_soulstone(&mut app);
        assert_eq!(rx.try_iter().count(), 0, "a locked stone is not used");
        assert!(app
            .world_mut()
            .non_send_resource_mut::<UiScript>()
            .take_ui_errors()
            .is_empty());
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .unlock_item(STONE);
        press_soulstone(&mut app);
        assert!(
            rx.try_iter().count() > 0,
            "unlocked, the stone's use goes out"
        );
    }

    /// Arm a three-craft Create All repeat in the VM, with an `UPDATE_TRADESKILL_RECAST` counter.
    fn arm_repeat(app: &mut App) {
        let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
        script.set_trade_skill(Some(benilla_ui::script::TradeSkillState {
            line: 197,
            line_name: "Tailoring".into(),
            rank: 1,
            max_rank: 75,
            recipes: vec![benilla_ui::script::TradeSkillRecipe {
                group: None,
                spell_id: 2963,
                name: "Bolt of Linen Cloth".into(),
                difficulty: benilla_ui::script::TradeSkillDifficulty::Trivial,
                num_available: 5,
                icon: None,
                min_made: 1,
                max_made: 1,
                cooldown: None,
                product_item: 2996,
                product_inv_type: 0,
                product_item_level: 0,
                reagents: vec![],
                tools: vec![],
                icon_pending: None,
                tools_pending: Vec::new(),
            }],
        }));
        // Armed with no host: the craft's own cast is not under test, only the repeat it arms.
        script.keep_calls();
        script
            .run(
                "RECASTS = 0 local f = CreateFrame('Frame') \
                 f:RegisterEvent('UPDATE_TRADESKILL_RECAST') \
                 f:SetScript('OnEvent', function() RECASTS = RECASTS + 1 end) \
                 DoTradeSkill(2, 3)",
            )
            .unwrap();
        assert_eq!(script.trade_skill_repeat(), (2963, 3));
        assert_eq!(
            script.take_trade_skill_dos().len(),
            1,
            "the craft's call, kept"
        );
    }

    /// The item leg cancels first (`0x48ae6b` into `0x6e4940`, cl=1, dl=1): the cast in flight
    /// ships `CMSG_CANCEL_CAST` ahead of the use (`0x6e4970`, then `0x48ae7d`), the bar's event is
    /// FAILED where a move's cancel arms STOP, and a running trade-skill repeat ends (`0x4fd790`).
    #[test]
    fn the_soulstone_cancels_the_cast_in_flight_before_its_use() {
        let (mut app, rx) = soulstone_app();
        let world = app.world_mut();
        let mut pending = world.resource_mut::<crate::spell::PendingCast>();
        pending.fill_record();
        pending.push(FIREBALL, None, std::time::Instant::now());
        arm_repeat(&mut app);
        let _ = rx.try_iter().count();
        press_soulstone(&mut app);

        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent.first(),
                Some(ClientCommand::CancelCast { spell_id: FIREBALL })
            ),
            "the cancel leads the use: {sent:?}"
        );
        assert!(sent.len() > 1, "and the use follows it: {sent:?}");
        assert!(!app
            .world()
            .resource::<crate::spell::PendingCast>()
            .in_flight());
        assert_eq!(
            edges(&app),
            ["FAILED"],
            "SPELLCAST_FAILED (cl=1, reason 0x1c); the echo's INTERRUPTED comes with the server's \
             answer"
        );
        signal(&mut app);
        let script = app.world().non_send_resource::<UiScript>();
        assert_eq!(
            script.trade_skill_repeat(),
            (0, 3),
            "the armed spell disarms (`mov [0xbde05c],0`) and the count stays"
        );
        assert_eq!(
            script.eval::<i64>("return RECASTS").unwrap(),
            2,
            "arming signalled UPDATE_TRADESKILL_RECAST, and so did the disarm (`0x4fd7a3`)"
        );
    }

    /// A pending targeting spell is dropped in place of a cast (`0x6e4955`): no
    /// `CMSG_CANCEL_CAST`, the cursor down, and the use still goes out.
    #[test]
    fn the_soulstone_drops_a_pending_targeting_spell() {
        let (mut app, rx) = soulstone_app();
        app.world_mut()
            .resource_mut::<crate::spell::PendingCast>()
            .fill_record();
        app.world_mut()
            .resource_mut::<crate::spell::targeting::SpellTargeting>()
            .enter(FIREBALL, crate::spell::CastCommit::Spell, 0x0040);
        arm_repeat(&mut app);
        let _ = rx.try_iter().count();
        press_soulstone(&mut app);

        let targeting = app
            .world()
            .resource::<crate::spell::targeting::SpellTargeting>();
        assert!(!targeting.active(), "the targeting cursor is gone");
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            !sent
                .iter()
                .any(|c| matches!(c, ClientCommand::CancelCast { .. })),
            "a targeting spell was never sent, so nothing cancels on the wire: {sent:?}"
        );
        assert!(!sent.is_empty(), "the use goes out");
        assert_eq!(
            edges(&app),
            ["FAILED"],
            "the event gate follows the targeting branch too (`0x6e4a04`-`0x6e4a1e`)"
        );
        signal(&mut app);
        assert_eq!(
            app.world()
                .non_send_resource::<UiScript>()
                .trade_skill_repeat(),
            (0, 3),
            "and the disarm flag acts on this branch"
        );
    }

    /// With nothing to cancel the abort still disarms the repeat: `0x6e4abc` tests only `cl`.
    #[test]
    fn the_soulstone_disarms_the_repeat_with_nothing_to_cancel() {
        let (mut app, rx) = soulstone_app();
        arm_repeat(&mut app);
        let _ = rx.try_iter().count();
        press_soulstone(&mut app);
        assert!(edges(&app).is_empty());
        signal(&mut app);
        assert_eq!(
            app.world()
                .non_send_resource::<UiScript>()
                .trade_skill_repeat(),
            (0, 3)
        );
    }

    /// The item leg's cancel (`0x48ae6b`) comes before the use (`0x48ae7d`) and its gate, tag 2 at
    /// `TryCastSpell` (`0x6e4bc1`): a macro's press outside an input event cancels, which disarms the
    /// repeat, and the stone's use is refused; the same press from the client sends it.
    #[test]
    fn a_refused_soulstone_press_cancels_and_uses_nothing() {
        let (mut app, rx) = soulstone_app();
        // A stone that is no quest-starter (whose offer is `CGItem::Use`'s own arm, ahead of the
        // cast), whose spell is castable dead and needs a reagent we lack: entering the cast
        // ladder leaves a local refusal (0x5c), and being kept out of it leaves nothing.
        let world = app.world_mut();
        world.resource_mut::<crate::items::Items>().insert_template(
            5232,
            Some(benilla_protocol::messages::ItemInfo {
                spells: vec![benilla_protocol::messages::ItemSpellEntry {
                    index: 0,
                    spell_id: SELF_RES,
                    trigger: 0,
                    charges: -1,
                    cooldown_ms: -1,
                    category: 0,
                    category_cooldown_ms: -1,
                }],
                use_spell: Some(benilla_protocol::messages::ItemUseSpell {
                    spell_id: SELF_RES,
                    cooldown_ms: -1,
                    category: 0,
                    category_cooldown_ms: -1,
                }),
                ..crate::items::test_template("Minor Soulstone")
            }),
        );
        let display = benilla_formats::SpellDisplay {
            effects: [SPELL_EFFECT_SELF_RESURRECT, 0, 0],
            attributes: 0x0080_0000,
            reagents: [
                (9999, 1),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
                (0, 0),
            ],
            ..Default::default()
        };
        world.insert_resource(crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays([(SELF_RES, display)].into()),
            ..crate::ui_action::Spells::empty_for_tests()
        });
        arm_repeat(&mut app);
        let _ = rx.try_iter().count();
        crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
            vm.expect("a VM").run(r#"RunScript("UseSoulstone()")"#)
        })
        .unwrap();
        // The ladder's reagent rung reads the reagent's item record (`0x6e4f3b`); uncached, its
        // refusal shows no line (`0x6e1eab`), so the ask is what shows the ladder ran.
        let asked = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            rx.try_iter()
                .any(|c| matches!(c, ClientCommand::ItemQuery { entry: 9999, .. }))
        };
        assert!(!asked(&rx), "the cast ladder was never entered");
        signal(&mut app);
        assert_eq!(
            app.world()
                .non_send_resource::<UiScript>()
                .trade_skill_repeat(),
            (0, 3),
            "the cancel ran ahead of the gate"
        );
        press_soulstone(&mut app);
        assert!(asked(&rx), "the client's own press enters the ladder");
    }

    /// The soulstone's use is `CGItem::Use` (`0x48ae7d`): without control it meets the control gate
    /// (`0x5d8eaa`) and ends with `ERR_CLIENT_LOCKED_OUT`, the cancel having run first.
    #[test]
    fn the_soulstone_use_meets_the_control_gate() {
        let (mut app, rx) = soulstone_app();
        // Not a quest-starter, whose arm runs ahead of the gates.
        app.world_mut()
            .resource_mut::<crate::items::Items>()
            .insert_template(
                5232,
                Some(benilla_protocol::messages::ItemInfo {
                    spells: vec![benilla_protocol::messages::ItemSpellEntry {
                        index: 0,
                        spell_id: SELF_RES,
                        trigger: 0,
                        charges: -1,
                        cooldown_ms: -1,
                        category: 0,
                        category_cooldown_ms: -1,
                    }],
                    use_spell: Some(benilla_protocol::messages::ItemUseSpell {
                        spell_id: SELF_RES,
                        cooldown_ms: -1,
                        category: 0,
                        category_cooldown_ms: -1,
                    }),
                    ..crate::items::test_template("Minor Soulstone")
                }),
            );
        {
            let mut script = app.world_mut().non_send_resource_mut::<UiScript>();
            script.set_player_control(false);
            script
                .run("ERR_CLIENT_LOCKED_OUT = 'ERR_CLIENT_LOCKED_OUT'")
                .unwrap();
        }
        app.world_mut()
            .resource_mut::<crate::spell::PendingCast>()
            .push(FIREBALL, None, std::time::Instant::now());
        let _ = rx.try_iter().count();
        press_soulstone(&mut app);

        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(sent[..], [ClientCommand::CancelCast { .. }]),
            "only the cancel goes out: {sent:?}"
        );
        // The refusal shows inside the press's call.
        assert_eq!(lines(&mut app), ["ERR_CLIENT_LOCKED_OUT"]);
    }

    /// The cancel runs before the item use's own gates, so a locked stone still cancels
    /// (`0x48ae6b` precedes `0x5d8d32`).
    #[test]
    fn a_locked_soulstone_still_cancels_first() {
        let (mut app, rx) = soulstone_app();
        app.world_mut()
            .non_send_resource_mut::<UiScript>()
            .lock_item(STONE);
        app.world_mut()
            .resource_mut::<crate::spell::PendingCast>()
            .push(FIREBALL, None, std::time::Instant::now());
        let _ = rx.try_iter().count();
        press_soulstone(&mut app);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(sent[..], [ClientCommand::CancelCast { spell_id: FIREBALL }]),
            "only the cancel goes out: {sent:?}"
        );
    }

    /// A `/reload` must not restart the release window; a relog must clear it.
    #[test]
    fn the_session_edge_clears_the_world_scoped_death_memory_and_nothing_else_does() {
        let mut app = App::new();
        app.add_message::<crate::net::DisconnectedMessage>()
            .insert_resource(DeathFeedState {
                mirror: Some((0x1234, true, true)),
                died_at: Some(12.0),
                vm: crate::ui_script::VmMemo::default(),
            });

        // No session edge: the memory stands.
        app.world_mut()
            .run_system_once(end_session_death_feed)
            .expect("the teardown runs");
        assert_eq!(
            app.world().resource::<DeathFeedState>().mirror,
            Some((0x1234, true, true)),
            "nothing but the session edge may clear the body's own state machine"
        );

        // A `/logout` is a session edge although `session_over` is false for it.
        app.world_mut()
            .write_message(crate::net::DisconnectedMessage::new(
                "logged out".into(),
                benilla_protocol::SessionEnd::LoggedOut,
            ));
        app.world_mut()
            .run_system_once(end_session_death_feed)
            .expect("the teardown runs");
        let feed = app.world().resource::<DeathFeedState>();
        assert!(
            feed.mirror.is_none(),
            "a relog must find no mirror, or the login-while-ghost arm cannot fire"
        );
        assert!(
            feed.died_at.is_none(),
            "the release window is the dead session's, and re-arms at the next login"
        );
    }
}

/// Run the death feed once on `world`, for a test of what it pushes.
#[cfg(test)]
pub(crate) fn run_feed_death_once(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(feed_death)
        .expect("the death feed runs");
}

/// The body's VM events between the last `(dead, ghost)` seen, if any, and this one.
fn fire_vm_edges(script: &mut UiScript, prev: Option<(bool, bool)>, dead: bool, ghost: bool) {
    match (prev, dead, ghost) {
        // A VM's first sight of the body dead: logging in or reloading dead. A death seen alive
        // signals inside its handler ([`signal_death_state`]).
        (None, true, false) => {
            script.fire_event("PLAYER_DEAD", vec![]);
        }
        (Some((true, false)), d, g) if g || !d => {
            // The health edge out of 0 (`0x6047f0`) unlocks every item (`0x5fefae`), then fires
            // `PLAYER_ALIVE` (`0x5fefb8`).
            script.unlock_all_items();
            script.fire_event("PLAYER_ALIVE", vec![]);
        }
        _ => {}
    }
}

#[cfg(test)]
mod edge_tests {
    use benilla_ui::script::UiScript;

    /// The revive edge unlocks every item, its one `ITEM_LOCK_CHANGED` ahead of `PLAYER_ALIVE`
    /// (`0x5fefae`, `0x5fefb8`); the ghost's own revive, no health edge, unlocks nothing.
    #[test]
    fn the_revive_edge_unlocks_every_item_before_player_alive() {
        const ITEM: u64 = 0x4000_0000_0000_0042;
        let mut s = UiScript::new().unwrap();
        s.run(
            "SEEN = {} local f = CreateFrame('Frame') \
             f:RegisterEvent('ITEM_LOCK_CHANGED') f:RegisterEvent('PLAYER_ALIVE') \
             f:RegisterEvent('PLAYER_UNGHOST') \
             f:SetScript('OnEvent', function() table.insert(SEEN, event) end)",
        )
        .unwrap();
        let seen = |s: &UiScript| {
            let out = s.eval::<Vec<String>>("return SEEN").unwrap();
            s.run("SEEN = {}").unwrap();
            out
        };
        s.lock_item(ITEM);
        let _ = seen(&s);
        super::fire_vm_edges(&mut s, Some((true, false)), true, true);
        assert_eq!(
            seen(&s),
            ["ITEM_LOCK_CHANGED", "PLAYER_ALIVE"],
            "the release"
        );
        assert!(!s.item_locked(ITEM));
        s.lock_item(ITEM);
        let _ = seen(&s);
        super::fire_vm_edges(&mut s, Some((true, true)), false, false);
        assert!(
            seen(&s).is_empty(),
            "the corpse run's end: its event is the ghost bit's (`0x5de9c0`)"
        );
        assert!(s.item_locked(ITEM));
    }
}

#[cfg(test)]
mod self_res_tests {
    use std::collections::HashMap;

    use benilla_formats::{SpellCatalog, SpellDisplay};
    use benilla_protocol::messages::{ItemInfo, ItemSpellEntry};
    use benilla_protocol::ObjectFields;

    use super::{resolve_self_res, SelfRes, SPELL_EFFECT_SELF_RESURRECT};
    use crate::items::Items;
    use crate::ui_action::Spells;

    /// Reincarnation's effect spell, what the server writes into the field.
    const REINCARNATION: u32 = 21169;
    /// `PLAYER_SELF_RES_SPELL`.
    const F_SELF_RES: u16 = 1224;
    /// `UNIT_FIELD_HEALTH` and `UNIT_FIELD_MAXHEALTH`: `unit_is_dead()` also needs a non-zero max.
    const F_HEALTH: u16 = 22;
    const F_MAXHEALTH: u16 = 28;
    /// `PLAYER_FIELD_PACK_SLOT_1`: the backpack's 16 slots, two dwords per guid.
    const F_PACK_1: u16 = 532;

    fn catalog(pairs: impl IntoIterator<Item = (u32, SpellDisplay)>) -> Spells {
        Spells {
            catalog: SpellCatalog::from_displays(pairs.into_iter().collect::<HashMap<_, _>>()),
            ..Spells::empty_for_tests()
        }
    }

    fn named(name: &str, effects: [u32; 3]) -> SpellDisplay {
        SpellDisplay {
            name: name.to_string(),
            effects,
            ..Default::default()
        }
    }

    /// `0x48acba`: nil while health is positive, including a released ghost's 1
    /// (`BuildPlayerRepop`).
    #[test]
    fn the_dead_gate_precedes_the_field() {
        let items = Items::default();
        // Nothing streamed: the spell leg answers before the inventory walk.
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let spells = catalog([(REINCARNATION, named("Reincarnation", [94, 0, 0]))]);

        let alive = ObjectFields::from_pairs(&[
            (F_MAXHEALTH, 4000),
            (F_HEALTH, 4000),
            (F_SELF_RES, REINCARNATION),
        ]);
        assert_eq!(
            resolve_self_res(&alive, &objects, &items, Some(&spells)),
            None,
            "alive with a self-res owed: nil"
        );

        let ghost = ObjectFields::from_pairs(&[
            (F_MAXHEALTH, 4000),
            (F_HEALTH, 1),
            (F_SELF_RES, REINCARNATION),
        ]);
        assert_eq!(
            resolve_self_res(&ghost, &objects, &items, Some(&spells)),
            None,
            "a released ghost still holds the field, and still answers nil"
        );

        let dead = ObjectFields::from_pairs(&[
            (F_MAXHEALTH, 4000),
            (F_HEALTH, 0),
            (F_SELF_RES, REINCARNATION),
        ]);
        assert_eq!(
            resolve_self_res(&dead, &objects, &items, Some(&spells)),
            Some(SelfRes::Spell {
                spell: REINCARNATION,
                label: "Reincarnation".into(),
            })
        );
    }

    /// The reference pushes the literal (`0x48ad1d`).
    #[test]
    fn an_unresolvable_spell_id_reads_unknown_not_nil() {
        let items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();
        let objects = objs.get();
        let dead =
            ObjectFields::from_pairs(&[(F_MAXHEALTH, 4000), (F_HEALTH, 0), (F_SELF_RES, 999_999)]);
        for spells in [None, Some(catalog([]))] {
            assert_eq!(
                resolve_self_res(&dead, &objects, &items, spells.as_ref()),
                Some(SelfRes::Spell {
                    spell: 999_999,
                    label: "UNKNOWN".into(),
                }),
                "no catalog and an empty catalog are the same miss"
            );
        }
    }

    /// The item leg (`0x5ed630`); the backpack holds a self-res on an equip trigger, an unrelated
    /// on-use spell, and the real one.
    #[test]
    fn a_zero_field_falls_through_to_a_carried_item() {
        let mut items = Items::default();
        let mut objs = crate::ui_items::TestObjects::new();

        let item = |name: &str, spells: Vec<ItemSpellEntry>| ItemInfo {
            spells,
            ..crate::items::test_template(name)
        };
        let block = |spell_id: u32, trigger: u32| ItemSpellEntry {
            index: 0,
            spell_id,
            trigger,
            charges: 0,
            cooldown_ms: -1,
            category: 0,
            category_cooldown_ms: -1,
        };
        // 3026 "Use Soulstone" is the real effect-94 spell; 439 "Healing Potion" is not.
        items.insert_template(10, Some(item("Healthstone", vec![block(439, 0)])));
        items.insert_template(11, Some(item("Worn Trinket", vec![block(3026, 1)])));
        items.insert_template(
            12,
            Some(item("Ankh of Reincarnation", vec![block(3026, 0)])),
        );
        for (guid, entry) in [(0xA0_u64, 10_u32), (0xA1, 11), (0xA2, 12)] {
            objs.spawn(guid, ObjectFields::from_pairs(&[(3, entry)]));
        }
        let objects = objs.get();
        let spells = catalog([
            (
                3026,
                named("Use Soulstone", [SPELL_EFFECT_SELF_RESURRECT, 0, 0]),
            ),
            (439, named("Healing Potion", [6, 0, 0])),
        ]);

        // Backpack slots 23, 24 and 25 hold the three, in that order.
        let mut pairs = vec![(F_MAXHEALTH, 4000), (F_HEALTH, 0)];
        for (i, guid) in [0xA0_u64, 0xA1, 0xA2].iter().enumerate() {
            let f = F_PACK_1 + 2 * i as u16;
            pairs.push((f, (*guid & 0xffff_ffff) as u32));
            pairs.push((f + 1, (*guid >> 32) as u32));
        }
        let dead = ObjectFields::from_pairs(&pairs);

        match resolve_self_res(&dead, &objects, &items, Some(&spells)) {
            Some(SelfRes::Item { entry, label, .. }) => {
                assert_eq!(
                    entry, 12,
                    "the EQUIP-trigger copy and the potion are skipped"
                );
                assert_eq!(label, "Ankh of Reincarnation", "the ITEM names the button");
            }
            other => panic!("expected the item leg, got {other:?}"),
        }

        // Without it the answer is nil, not the spell leg's "UNKNOWN".
        let bare = ObjectFields::from_pairs(&[(F_MAXHEALTH, 4000), (F_HEALTH, 0)]);
        assert_eq!(
            resolve_self_res(&bare, &objects, &items, Some(&spells)),
            None
        );
    }
}
