//! Death and resurrection: the wire-fed stores, the events into the Lua UI and the intents back
//! to the wire. Dead is health 0 and ghost is `PLAYER_FLAGS` bit 0x10. The getters read the
//! stores through the host as they run ([`DeathView`]). The death state signals (`PLAYER_DEAD` or
//! `PLAYER_ALIVE`) inside our own death's handler and after each arrival's world-enter cascade
//! ([`signal_death_state`]), and `PLAYER_ALIVE` in the health watcher's revive arm, on release
//! and on a res ([`crate::field_watch`]). The offers and the corpse-range signals that packets
//! raise go on the dialogs' lane at their handlers ([`net`]); the corpse range's own poll runs in
//! the world frame's update ([`poll_corpse`]). The cascade releases a body it finds dead
//! ([`repop_on_world_enter`]). `PLAYER_UNGHOST` is the ghost state's (`0x5de9c0`), signalled where
//! it runs: the ghost bit's clear ([`crate::field_watch`]) and an alive arrival (`crate::ui_unit`).

use bevy::prelude::*;

use benilla_assets::coords::wow_to_bevy;
use benilla_ui::script::{
    DeathAction, DeathQuery, DeathUiState, ResSicknessLevelQuery, ScriptValue, UiScript,
};

use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfGuid, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_action::Spells;

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
    /// The caster's name was asked with the callback `0x5e7ca0` (`0x5e7c82`), whose answer shows
    /// the offer or forgets it.
    pub(crate) waits_name: bool,
}

/// The release timer our own death arms (`0x5e99b0`, `0xb4e340` and its no-timer byte
/// `0xb4e344`), which `GetReleaseTimeRemaining` reads (`0x4962a0`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum ReleaseTimer {
    /// No death since the interface started armed it: `UI_Init` zeroes it at each login and
    /// `/reload` (`0x490131`, `0x490137`), and `GetReleaseTimeRemaining` answers 0 (`0x4962b6`).
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
    /// The last `MSG_CORPSE_QUERY` answer; a not-found drops it.
    pub(crate) corpse: Option<CorpsePoint>,
    /// A pending resurrection offer (`[0xc4d760]`); cleared when answered, or by a refused name.
    pub(crate) resurrect: Option<ResurrectOffer>,
    /// Our streamed corpse object's guid, which `CMSG_RECLAIM_CORPSE` carries like the 1.12 client.
    pub(crate) corpse_guid: Option<u64>,
    /// The spirit healer awaiting the XP-loss answer (`[0xc4d798]`); Accept sends
    /// `CMSG_SPIRIT_HEALER_ACTIVATE`.
    pub(crate) spirit_healer: Option<u64>,
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

    /// Zero the release timer, as `UI_Init` does at each login and `/reload` (`0x490131`,
    /// `0x490137`): only a death since writes it (`0x5e9a35`, `0x5e9a3c`).
    pub(crate) fn zero_release(&mut self) {
        self.release = ReleaseTimer::Unset;
    }
}

/// The interface's death globals beside the stores, world-scoped and cleared at the session's end:
/// the corpse-range latch. A `/reload` keeps it.
#[derive(Resource, Default)]
pub(crate) struct DeathLatches {
    /// The corpse-range latch `[0xb4e324]`: whether the last verdict signalled was in range
    /// (`0x4920f0`), which the cascade's corpse step zeroes into whichever VM is live.
    corpse_in_range: bool,
}

impl DeathLatches {
    /// The corpse-range latch set, as a ghost in range of its corpse left it.
    #[cfg(test)]
    pub(crate) fn latch_corpse_in_range_for_test(&mut self) {
        self.corpse_in_range = true;
    }

    /// `SetCorpseInRange 0x4920f0`: on a change (`0x4920f6`), the latch stored and its signal,
    /// [`CorpseSignal::in_range`] for in range, `CORPSE_OUT_OF_RANGE` (399, `0x49211f`) for out.
    fn set_in_range(
        &mut self,
        in_range: bool,
        corpse: Option<&CorpsePoint>,
    ) -> Option<CorpseSignal> {
        if std::mem::replace(&mut self.corpse_in_range, in_range) == in_range {
            return None;
        }
        Some(if in_range {
            CorpseSignal::in_range(corpse)
        } else {
            CorpseSignal::Out
        })
    }
}

/// A corpse-range event, each argumentless (`0x703e50`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CorpseSignal {
    /// `CORPSE_IN_RANGE` (397).
    In,
    /// `CORPSE_IN_INSTANCE` (398).
    InInstance,
    /// `CORPSE_OUT_OF_RANGE` (399).
    Out,
}

impl CorpseSignal {
    /// The in-range event for the cached corpse: [`Self::InInstance`] when the cache's display
    /// map is not the corpse's own, a dungeon's entrance (`0x492102`-`0x492114`,
    /// `0x4962f5`-`0x496307`).
    fn in_range(corpse: Option<&CorpsePoint>) -> Self {
        if corpse.is_some_and(|c| u32::try_from(c.display_map) != Ok(c.corpse_map)) {
            Self::InInstance
        } else {
            Self::In
        }
    }

    /// Signalled in place.
    fn fire(self, script: &mut UiScript) {
        match self {
            Self::In => script.fire_event("CORPSE_IN_RANGE", vec![]),
            Self::InInstance => script.fire_event("CORPSE_IN_INSTANCE", vec![]),
            Self::Out => script.fire_event("CORPSE_OUT_OF_RANGE", vec![]),
        }
    }

    /// Queued on the dialogs' lane at a handler's place.
    fn queue(self, lane: &mut crate::ui_dialog_verbs::DialogLane) {
        match self {
            Self::In => lane.event("CORPSE_IN_RANGE", vec![]),
            Self::InInstance => lane.event("CORPSE_IN_INSTANCE", vec![]),
            Self::Out => lane.event("CORPSE_OUT_OF_RANGE", vec![]),
        }
    }
}

/// The corpse-range dialog radius, 40 yd inclusive: the 1.12 client compares d² ≤ `[0xb4e2ac]`
/// (1600.0) against the corpse-query position. The server's reclaim gate,
/// `CORPSE_RECLAIM_RADIUS` 39 (`Corpse.h:40`), sits 1 yd inside it.
const CORPSE_RANGE_SQ: f32 = 40.0 * 40.0;

/// The spirit-healer dialog range: `CheckSpiritHealerDist` compares d² ≤ `[0xc4c28c]` (30.864).
const SPIRIT_HEALER_RANGE_SQ: f32 = 5.5556 * 5.5556;

/// What the death getters read through the host as they run ([`DeathQuery`]): the self body, the
/// death stores and what `HasSoulstone()` walks.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DeathView<'w, 's> {
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

    /// The death state as the getters read it now: the timers off the stores, and what reads
    /// the live body (the healer's range, the self-res) while one is held.
    fn state(&self) -> DeathUiState {
        let now = self.time.elapsed_secs_f64();
        let death_net = &self.death_net;
        let release_remaining = match death_net.release {
            ReleaseTimer::At(at) => Some((at - now).max(0.0) as f32),
            ReleaseTimer::Off => None, // GetReleaseTimeRemaining() == -1, the no-timer DEATH text
            // A zero timer answers 0 (`0x4962b4`-`0x4962cc`), so no DEATH popup shows.
            ReleaseTimer::Unset => Some(0.0),
        };
        let recovery_delay = death_net
            .reclaim_at
            .map_or(0.0, |at| (at - now).max(0.0) as f32);
        let body = self.body();
        let spirit_healer_in_range = body.is_some_and(|(_, _, self_t)| {
            death_net.spirit_healer.is_some_and(|npc| {
                self.objects
                    .entity(npc)
                    .and_then(|e| self.transforms.get(e).ok())
                    .is_some_and(|t| {
                        t.translation.distance_squared(self_t.translation) <= SPIRIT_HEALER_RANGE_SQ
                    })
            })
        });
        DeathUiState {
            release_remaining,
            recovery_delay,
            resurrect_sickness: death_net.resurrect.as_ref().is_some_and(|o| o.sickness),
            resurrect_has_timer: death_net.resurrect.as_ref().is_some_and(|o| o.has_timer),
            spirit_healer_in_range,
            // Worded by the getter off [`ResSicknessLevelQuery`].
            sickness_duration: None,
            // `HasSoulstone()`; the dead gate is first, so the inventory walk runs only while dead.
            self_res_label: body.and_then(|(_, store, _)| {
                resolve_self_res(&store.0, &self.objects, &self.items, self.spells.as_deref())
                    .map(|r| r.label().to_owned())
            }),
        }
    }
}

/// The death getters' family of the host's live reads.
impl crate::script_calls::live::LiveFamily for DeathView<'_, '_> {
    fn answer(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        ask.answer(|_: &DeathQuery| Some(self.state()))
            || ask.answer(|_: &ResSicknessLevelQuery| {
                self.body()
                    .map(|(_, store, _)| store.0.unit_level().unwrap_or(0))
            })
    }
}

/// What the corpse poll reads and writes beside the host: the self body's place and the session,
/// the death stores, the latches and the map shown.
type CorpsePoll = (
    Query<'static, 'static, &'static Transform, With<SelfPlayer>>,
    Res<'static, SelfGuid>,
    Res<'static, crate::net::NetStatus>,
    Res<'static, DeathNet>,
    ResMut<'static, DeathLatches>,
    Option<Res<'static, benilla_world::world_map::CurrentMap>>,
);

/// `CheckCorpseRange 0x492130`, which the world frame's update runs every frame after its pick
/// (`0x4818c5`, in `0x481790`'s unconditional tail): the active player on the map shown within 40
/// yards of the cached corpse is in range (`0x492196`), and `SetCorpseInRange` signals a change in
/// place ([`DeathLatches::set_in_range`]). The cache holds a corpse only for a ghost: an answer is
/// stored for one alone, and the ghost bit's edges reset it ([`reset_corpse`]).
pub(crate) fn poll_corpse(mut vm: crate::script_calls::VmWith<CorpsePoll>) {
    let signal = {
        let (self_q, self_guid, status, death_net, mut latches, map) = vm.own();
        // A disconnect keeps the self avatar with its frozen descriptor; only a live session's
        // body counts.
        let body = self_guid
            .0
            .filter(|_| status.connected)
            .and_then(|_| self_q.single().ok());
        let Some(self_t) = body else {
            return;
        };
        // Against the query's display position; the coordinate transform is isometric, so the
        // Bevy-space compare is in yards.
        let corpse = death_net.corpse.as_ref();
        let in_range = corpse.is_some_and(|cp| {
            // Deviation: `0x492130` compares the positions whatever map we stand on, so a
            // ghost on another map at the corpse's coordinates reads in range for a reclaim
            // the server refuses; here only the corpse's display map counts.
            let on_display_map = map
                .as_ref()
                .is_some_and(|m| m.0 == u32::try_from(cp.display_map).unwrap_or(u32::MAX));
            on_display_map
                && wow_to_bevy(cp.position).distance_squared(self_t.translation) <= CORPSE_RANGE_SQ
        });
        latches.set_in_range(in_range, corpse)
    };
    if let Some(signal) = signal {
        vm.run(|script| {
            if let Some(script) = script {
                signal.fire(script);
            }
        });
    }
}

/// `SignalDeathState 0x495850`, where our own death's `PresentUnitDeath` ends (`0x6251cc`, for
/// the active player alone, `0x5fa6d0` at `0x6251c2`): with our health 0 or less (`jg` at
/// `0x495876`) `PLAYER_DEAD` (`0x113`, `0x49587d`), its handlers reading the timer the death armed,
/// then `ClearCursor(1, 1)` (`0x495889` → `0x495190`); else `PLAYER_ALIVE` (`0x112`, `0x495893`).
/// Its handlers read the timer the death armed through the host.
pub(crate) fn signal_death(mut vm: crate::script_calls::VmWith<DeathView<'static, 'static>>) {
    signal(&mut vm);
}

/// [`signal_death`]'s body; false with no VM or no live body, which signals nothing.
fn signal(vm: &mut crate::script_calls::VmWith<DeathView<'static, 'static>>) -> bool {
    let dead = {
        let (Some(_), set) = vm.parts() else {
            return false;
        };
        let view = set.p0();
        let Some((_, store, _)) = view.body() else {
            return false;
        };
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
    true
}

/// [`DeathNet::zero_release`] in `world`, from the interface load; a world without the death
/// stores has no timer.
pub(crate) fn zero_release_timer(world: &mut World) {
    if let Some(mut death_net) = world.get_resource_mut::<DeathNet>() {
        death_net.zero_release();
    }
}

/// [`signal_death`] in place, from our own death's handler (`crate::target`'s `died`) and from our
/// player's arrival (`0x5deb65`, [`crate::ui_world_enter`]).
pub(crate) fn signal_death_state(world: &mut World) {
    if let Err(e) = world.run_system_cached(signal_death) {
        // A harness without the death state signals nothing.
        debug!("death: the death state was not signalled: {e}");
    }
}

/// `CGPlayer_C::SendRepopRequest` (`0x5e0ae0`): `CMSG_REPOP_REQUEST` (0x15a, `0x5e0b1f`), only
/// while dead or a ghost (`0x5e0ae6` → `0x605f30`).
fn send_repop_request(store: &benilla_protocol::ObjectFields, net: &NetCommands) {
    if store.is_dead_or_ghost() {
        let _ = net.0.send(ClientCommand::RepopRequest);
    }
}

/// The corpse cache and the range latch, beside the host.
type CorpseStores = (
    Option<ResMut<'static, DeathNet>>,
    Option<ResMut<'static, DeathLatches>>,
);

/// `QueryCorpse 0x491f50`'s head: the corpse cache reset (`0x491f74` → `0x492010(-1, …)`) and the
/// range latch zeroed (`0x491f7b` → `0x4920f0(0)`). Whether the latch was set, so its caller
/// signals `CORPSE_OUT_OF_RANGE` (`0x49211f`) in place, ahead of [`ask_corpse`].
pub(crate) fn reset_corpse(
    death_net: Option<&mut DeathNet>,
    latches: Option<&mut DeathLatches>,
) -> bool {
    if let Some(death_net) = death_net {
        death_net.corpse = None;
    }
    latches.is_some_and(|latches| std::mem::take(&mut latches.corpse_in_range))
}

/// `QueryCorpse 0x491f50`'s tail: `MSG_CORPSE_QUERY` (`0x491fb2`-`0x491fdf`) while the active
/// player is a ghost (`PLAYER_FLAGS` bit 4, `0x491fa1`-`0x491fb0`), nothing otherwise.
pub(crate) fn ask_corpse(ghost: bool, net: &NetCommands) {
    if ghost {
        let _ = net.0.send(ClientCommand::CorpseQuery);
    }
}

/// `QueryCorpse 0x491f50` in the world-enter cascade (`0x490a28`), after its other sends
/// ([`reset_corpse`], [`ask_corpse`]). The flags watcher's run on the ghost bit's edges is the
/// update packet's (`0x5eeac1`, [`crate::field_watch::FieldNotice::QueryCorpse`]).
pub(crate) fn query_corpse_on_world_enter(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    mut vm: crate::script_calls::VmWith<CorpseStores>,
    net: Res<NetCommands>,
) {
    let (mut death_net, mut latches) = vm.own();
    let was_in = reset_corpse(death_net.as_deref_mut(), latches.as_deref_mut());
    if was_in {
        vm.run(|script| {
            if let Some(script) = script {
                script.fire_event("CORPSE_OUT_OF_RANGE", vec![]);
            }
        });
    }
    ask_corpse(
        self_q.single().is_ok_and(|store| store.0.player_is_ghost()),
        &net,
    );
}

/// The world-enter cascade's last test (`0x490a2d`-`0x490a38`): with our health 0 or less it
/// releases the body (`0x490a3c` → `0x5e0ae0`), after its 38 calls and ahead of its tutorials. The
/// cascade runs at an arrival, a `/reload` and a reactivation (`0x5deb60`, `0x490168`,
/// `0x5e9c08`), so logging in or reloading dead and unreleased goes to the graveyard
/// (`MiscHandler.cpp:48-67`).
pub(crate) fn repop_on_world_enter(
    self_q: Query<&ObjectStore, With<SelfPlayer>>,
    net: Res<NetCommands>,
) {
    if let Ok(store) = self_q.single() {
        if store.0.unit_is_dead() {
            send_repop_request(&store.0, &net);
        }
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
    /// What the soulstone use's requirement gate reads, seated with the host out.
    requirements: crate::ui_items::RequirementsLive<'w, 's>,
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
            DeathAction::Repop => {
                if let Some(store) = &store {
                    send_repop_request(store, &self.net);
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
            // The host runs the use past the cancel's listeners ([`Self::soulstone_head`]); a
            // harness with no host runs it here.
            DeathAction::UseSoulstone { .. } => {
                if let Some(stone) = self.soulstone_head() {
                    self.soulstone_use(script, stone);
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

    /// `UseSoulstone()` (`0x48ad70`) up to its use, with no health test: with
    /// `PLAYER_SELF_RES_SPELL` set, `CMSG_SELF_RES` (`0x48adb6`, `0x48ae05`); else the carried item
    /// (`0x5ed630`) and the cast cancelled before its use (`0x48ae6b` into `0x6e4940`, cl=1, dl=1,
    /// reason `0x1c`), whose signals' listeners a caller runs before [`Self::soulstone_use`].
    /// Nothing is cleared here: the server zeroes the field.
    pub(crate) fn soulstone_head(&mut self) -> Option<SoulstoneUse> {
        let store = self.targeting.self_store.single().ok()?.0.clone();
        let res = {
            let ladder = self.cast.p0();
            self_res(
                &store,
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
            return None;
        };
        // Its failed flag disarms the trade-skill repeat on every branch (`0x6e4abc`-`0x6e4ac2`
        // into `0x4fd790`), queued with its event.
        self.cast.p1().abort_cast();
        Some(SoulstoneUse {
            bag_index,
            slot,
            guid,
            entry,
        })
    }

    /// `UseSoulstone()` past its cancel: an ordinary item use, `0x5d8d00` (`0x48ae7d`), like every
    /// bag click, a locked stone refused at its first gate (`0x5d8d32`).
    pub(crate) fn soulstone_use(&mut self, script: &mut UiScript, stone: SoulstoneUse) {
        let SoulstoneUse {
            bag_index,
            slot,
            guid,
            entry,
        } = stone;
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
        self.requirements.seat(script);
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

/// The carried soulstone `UseSoulstone` uses past its cast cancel ([`DeathSteps::soulstone_head`]).
pub(crate) struct SoulstoneUse {
    bag_index: u8,
    slot: u8,
    guid: u64,
    entry: u32,
}

/// `SpiritHealerDialog 0x5df730(npc)` asking: for a ghost (`PLAYER_FLAGS` bit 0x10, `0x5df74a`),
/// the healer `npc` held as a unit (`0x5df77a`) within the interaction range (`0x5df7c1`,
/// `[0xc4c28c]`) is remembered (`0x5df7e6`), and `CONFIRM_XP_LOSS` (396, `0x5df837`) is owed with
/// the XP the res costs, `PLAYER_NEXT_LEVEL_XP` × 0.05 truncated (`0x5df815`-`0x5df827`). No stock
/// FrameXML reads the cost; `UIParent.lua:399` asks `GetResSicknessDuration()`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SpiritHealerAsk<'w, 's> {
    death_net: ResMut<'w, DeathNet>,
    me: Query<'w, 's, (&'static ObjectStore, &'static Transform), With<SelfPlayer>>,
    objects: Objects<'w, 's>,
    transforms: Query<'w, 's, &'static Transform>,
}

impl SpiritHealerAsk<'_, '_> {
    /// The ask for `npc`; the cost the signal carries when its gates pass, else nothing is
    /// remembered and nothing signals.
    pub(crate) fn ask(&mut self, npc: u64) -> Option<i64> {
        let (store, self_t) = self.me.single().ok()?;
        if !store.0.player_is_ghost() {
            return None;
        }
        let healer = self
            .objects
            .entity(npc)
            .filter(|_| self.objects.store(npc).is_some_and(|s| s.is_unit()))
            .and_then(|e| self.transforms.get(e).ok())?;
        if healer.translation.distance_squared(self_t.translation) > SPIRIT_HEALER_RANGE_SQ {
            return None;
        }
        self.death_net.spirit_healer = Some(npc);
        Some(
            store
                .0
                .player_next_level_xp()
                .map_or(0, |next| (f64::from(next) * 0.05) as i64),
        )
    }
}

/// [`SpiritHealerAsk`] from the click on a spirit healer, which the reference raises with no
/// packet (`0x5f0130` → `0x5df730`): `CONFIRM_XP_LOSS` signalled in place with the host.
pub(crate) fn ask_spirit_healer(
    In(npc): In<u64>,
    mut vm: crate::script_calls::VmWith<SpiritHealerAsk<'static, 'static>>,
) {
    let Some(cost) = vm.own().ask(npc) else {
        return;
    };
    vm.run(|script| {
        if let Some(script) = script {
            script.fire_event("CONFIRM_XP_LOSS", vec![ScriptValue::Int(cost)]);
        }
    });
}

/// The death stores, the corpse poll, the intent drain and the root and water-walk messages.
pub(crate) struct DeathPlugin;

impl Plugin for DeathPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::ui_dialog_verbs::add_dialog_lane(app);
        app.init_resource::<DeathNet>()
            .init_resource::<DeathLatches>()
            .add_systems(
                Update,
                (
                    // The world frame's update, after its pick (`0x48184a`, then `0x4818c5`), as
                    // the area spirit healer's poll; gated, as the poll re-derives every frame.
                    poll_corpse
                        .after(crate::target::TargetUpdate)
                        .run_if(crate::ui_script::ingame_ui_up)
                        .in_set(crate::client_state::InWorldGated),
                    drive_death_look,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

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
            .init_resource::<DeathLatches>()
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

    /// The item leg's cancel signals (`0x48ae6b` into `0x6e4940`) before the stone's use
    /// (`0x48ae7d`), so its `SPELLCAST_FAILED` listener's packet goes between the cancel's and the
    /// use's.
    #[test]
    fn the_soulstones_cancel_listener_sends_ahead_of_its_use() {
        let (mut app, rx) = soulstone_app();
        let world = app.world_mut();
        let mut pending = world.resource_mut::<crate::spell::PendingCast>();
        pending.fill_record();
        pending.push(FIREBALL, None, std::time::Instant::now());
        crate::script_calls::with_hosted_vm(app.world_mut(), |vm| {
            vm.expect("a VM").run(
                r#"local r = CreateFrame("Frame")
                r:RegisterEvent("SPELLCAST_FAILED")
                r:SetScript("OnEvent", function() RandomRoll(1, 2) end)"#,
            )
        })
        .unwrap();
        let _ = rx.try_iter().count();
        press_soulstone(&mut app);
        let sent: Vec<&str> = rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::CancelCast { .. } => Some("cancel"),
                ClientCommand::RandomRoll { .. } => Some("roll"),
                // The stone's template starts a quest, so its use is the offer's query.
                ClientCommand::UseItem { .. } | ClientCommand::QuestgiverQuery { .. } => {
                    Some("use")
                }
                _ => None,
            })
            .collect();
        assert_eq!(sent, ["cancel", "roll", "use"]);
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

    /// The session's end clears the range latch, so a relog starts out of range. A `/logout` is a
    /// session edge although `session_over` is false for it.
    #[test]
    fn the_session_edge_clears_the_world_scoped_death_memory() {
        let mut world = World::new();
        world.init_resource::<DeathNet>();
        world.insert_resource(DeathLatches {
            corpse_in_range: true,
        });
        world
            .run_system_cached_with(
                net::on_session_end,
                benilla_protocol::SessionEvent::Disconnected {
                    reason: "logged out".into(),
                    end: benilla_protocol::SessionEnd::LoggedOut,
                },
            )
            .expect("the teardown runs");
        let latches = world.resource::<DeathLatches>();
        assert!(!latches.corpse_in_range, "a relog starts out of range");
    }

    /// `PLAYER_FIELD_BYTES`, whose bit 0x8 our own death's timer reads (`0x5e9a27`).
    const BYTES: u16 = 1222;

    /// A world with our player at `health`, the release-timer bit set, the death stores and the
    /// arrival's signal, and a VM that records each death-state event with the timer it reads.
    fn arrival_app(health: u32) -> App {
        use benilla_protocol::field::{FIELD_UNIT_HEALTH, FIELD_UNIT_MAXHEALTH};
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
            .init_resource::<DeathLatches>()
            .init_resource::<crate::names::NameCache>()
            .init_resource::<crate::net::GuidIndex>()
            .init_resource::<crate::items::Items>()
            .init_resource::<Time<Real>>()
            .insert_resource(State::new(crate::client_state::ClientState::InWorld))
            .add_plugins(crate::ui_world_enter::WorldEnterPlugin);
        app.insert_non_send_resource(recording_vm());
        let body = ObjectFields::from_pairs(&[
            (FIELD_UNIT_HEALTH, health),
            (FIELD_UNIT_MAXHEALTH, 100),
            (BYTES, 0x8),
        ])
        .into_created(ObjectType::Player);
        let me = app
            .world_mut()
            .spawn((ObjectStore(body), Transform::default(), SelfPlayer))
            .id();
        app.world_mut()
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(ME, me);
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(ME);
        crate::ui_world_enter::push_arrival(app.world_mut());
        app
    }

    fn recording_vm() -> UiScript {
        let script = UiScript::new().unwrap();
        script
            .run(
                "SEEN = {} local f = CreateFrame('Frame') f:RegisterEvent('PLAYER_DEAD') \
                 f:RegisterEvent('PLAYER_ALIVE') f:SetScript('OnEvent', function() \
                 table.insert(SEEN, event .. ' ' .. GetReleaseTimeRemaining()) end)",
            )
            .unwrap();
        script
    }

    fn seen(app: &App) -> Vec<String> {
        app.world()
            .non_send_resource::<UiScript>()
            .eval("return SEEN")
            .unwrap()
    }

    /// Our player's arrival signals the death state after its cascade (`0x5deb60`, `0x5deb65` →
    /// `0x495850`): `PLAYER_DEAD` for a dead body, `PLAYER_ALIVE` for any other, each once. No
    /// death since `UI_Init` armed the timer (`0x490131`), so it reads 0 (`0x4962b6`) and
    /// `UIParent.lua:240` shows no DEATH popup.
    #[test]
    fn an_arrival_signals_the_death_state_once_over_a_zero_timer() {
        for (health, expected) in [(0, "PLAYER_DEAD 0"), (100, "PLAYER_ALIVE 0")] {
            let mut app = arrival_app(health);
            app.update();
            app.update();
            assert_eq!(seen(&app), [expected], "health {health}");
        }
    }

    /// A `/reload`'s cascade signals no death state (`0x490168` never reaches `0x495850`): the new
    /// VM, dead and unreleased, hears neither event, and the timer `UI_Init` zeroed reads 0
    /// (`0x490131`, `0x4962b6`) whatever the bit says, so no DEATH popup shows.
    #[test]
    fn a_reload_dead_signals_nothing_and_reads_a_zero_timer() {
        let mut app = arrival_app(0);
        app.world_mut()
            .resource_mut::<DeathNet>()
            .arm_release(true, 0.0);
        app.update();
        // The reload: a fresh VM over the same body, the timer zeroed by its load.
        app.insert_non_send_resource(recording_vm());
        zero_release_timer(app.world_mut());
        app.update();
        app.update();
        assert_eq!(seen(&app), Vec::<String>::new());
        let left: f64 = app
            .world()
            .non_send_resource::<UiScript>()
            .eval("return GetReleaseTimeRemaining()")
            .unwrap();
        assert_eq!(left, 0.0);
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
