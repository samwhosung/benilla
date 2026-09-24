//! TAB / nearest-enemy targeting + combat auto-target — the **Classic-priority selection**
//! (decision 0567) over the 1.12 wire laws.
//!
//! The 1.12 byte algorithm (±30° cone about the *character's* facing, cone-first-then-nearest
//! sort, 10-yd out-of-cone bubble, snapshot list + cursor cycling — the TAB core `0x493f60`) was
//! implemented faithfully, live-reproduced, and then **replaced at the director's call**: the
//! authentic law skips a close mob that fills your screen (at 2 yd the cone is ~±1 yd wide, and
//! the camera sits ~9 yd behind — "right in front of my cam" is routinely 40°+ off the
//! character axis), and its snapshot cycle walks into behind-you mobs. Blizzard hit the same
//! wall and rewrote targeting for Legion 7.2 — the system Classic Era ships. We follow that
//! design, pinned from the Classic binary's own cvar-description strings (statically recovered;
//! the code section is packer-encrypted) and the 7.2 patch note:
//!
//! - **Screen-space tiering** — a candidate whose projected point lands inside the camera
//!   frustum with each screen edge pulled in (`TargetPriorityFrustumPullIn{Sides,Top,Bot}`:
//!   "Percentage towards center to pull in [edge] of the screen for in-view checks") is a
//!   tier-1 ("100% correct") pick; off-screen candidates are reachable **only when no
//!   on-screen candidate exists** (`TargetPriorityAllowAnyOnScreen` = 1 semantics).
//! - **A weighted score, not a lexicographic sort** (`TargetPriorityValueBank`): screen
//!   off-centerness + normalized distance − an "attacking me" bonus; lowest wins.
//! - **A fighting-me score bonus, not Classic's hard combat lock**: units attacking me outrank
//!   everything peaceful, but TAB can always walk off them onto a fresh target. Classic's
//!   `TargetPriorityCombatLock` (pool restricted to in-combat units) was implemented and then
//!   retired by the director's feel — decision 0568: with a single attacker it pinned TAB to
//!   that mob with no way off.
//! - **History cycling, no snapshot cursor** (`TargetPriorityHighlightHistoryMs` — "time target
//!   history should be maintained for repeated activations"): every press re-scores the live
//!   world and skips recently-tabbed guids; exhausting the pool clears the history (the wrap).
//!   Shift-TAB steps back through the history. A clicked target joins the history when tabbed
//!   away from (`TargetPriorityContinueFromManualTarget`'s observable).
//!
//! The **numbers** here (pull-in margins, weights, the history window) are tunable stand-ins —
//! Blizzard's defaults live in the encrypted code section; each const names its cvar so a later
//! pin (the director's in-game `GetCVarInfo` dump) slots straight in. Not modeled, disclosed:
//! `TargetPriorityUpdateDelay` (a timer-refreshed list feeding the hold-to-highlight preview we
//! don't have — per-press rebuild covers selection), `TargetPriorityAutoTargetIgnoreWindow`,
//! `AllowAnyOnScreen=2` (out-of-range picks), PvP weighting, and the 7.2 note's
//! character-OR-camera visibility union (we test the camera only).
//!
//! **What stays byte-law from 1.12** (wire/data semantics, unchanged): the validity filters
//! (dead / [`can_attack`] `0x606980` / CreatureType flag-bit-0 critters), the [`commit`]
//! stop→select→re-swing SetSelection law (`0x493540`), attack-with-no-target acquiring the best
//! candidate and swinging (`0x612df0` @ `6130b5`), and the ATTACKERSTATEUPDATE auto-acquire
//! (`6259c9`–`6259fe`). The 41-yd range keeps 1.12's `targetNearestDistance` default — Classic
//! still ships that cvar ("limited to tab targeting range").

use benilla_formats::CreatureTypeFlags;
use benilla_protocol::{guid, EntityKind};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::creature_anim::{Engaged, SwingMessage};
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, GuidIndex, NetEntity, ObjectStore, Reputations, SelfPlayer};
use benilla_assets::{LockRecover, WorldAssets};

use super::relations::{can_assist, can_attack};
use super::ring::reaction_from_player;
use super::{Factions, Selection};

// == The Classic-priority dials ==
// Each names the Classic Era cvar it stands in for. The VALUES are tuned, not pinned — the
// binary's defaults are packer-encrypted; when the director's in-game `GetCVarInfo` dump lands,
// replace these with the real defaults and note it in the 0567 lineage.

/// `targetNearestDistance` — the tab-targeting range (yd). 1.12's byte-verified default 41
/// (`[0x804510]`); Classic still ships the cvar ("limited to tab targeting range").
const TAB_RANGE: f32 = 41.0;
/// `TargetPriorityFrustumPullInSides` — fraction of the viewport width pulled toward center
/// from EACH side edge for the in-view check.
const FRUSTUM_PULL_SIDES: f32 = 0.10;
/// `TargetPriorityFrustumPullInTop` — fraction of the viewport height pulled down from the top.
const FRUSTUM_PULL_TOP: f32 = 0.10;
/// `TargetPriorityFrustumPullInBot` — fraction of the viewport height pulled up from the bottom.
const FRUSTUM_PULL_BOT: f32 = 0.10;
/// `TargetPriorityHighlightHistoryMs` — how long a tabbed guid stays "recently visited" (the
/// skip set for repeated presses).
const HISTORY_SECS: f64 = 4.0;
/// The score's screen term weight: per unit of normalized off-centerness (0 = screen center,
/// 1 ≈ a viewport corner). One slot of the `TargetPriorityValueBank` stand-in.
const W_SCREEN: f32 = 1.0;
/// The score's distance term weight: per unit of `dist / TAB_RANGE`.
const W_DIST: f32 = 1.0;
/// The "attacking me" bonus (`TargetPriorityCombatLock`'s spirit as a score term — the hard
/// lock itself is retired, decision 0568): a mob whose UNIT_FIELD_TARGET is me and whose
/// in-combat flag is set outranks anything peaceful at any screen position (the bonus exceeds
/// the terms' max sum), but never pins the cycle.
const COMBAT_WITH_ME_BONUS: f32 = 3.0;

/// `WOW_TAB_TRACE=1` — the TAB-target field instrument: every press logs each known unit's
/// verdict (the reject reason, or its distance / screen position / score), the sorted pool,
/// the history, and the pick — so a "TAB won't take the mob next to me"
/// report is diagnosable from the log instead of re-guessed. One `OnceLock` read per press when
/// unset; the lines ride `info!` under the `tab-trace:` prefix.
fn tab_trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_TAB_TRACE").is_some())
}

/// Which side a press is looking for — the reference's **mode** argument, and the only thing that
/// forks on the whole path.
///
/// The four `TargetNearest*` shims are one function with one changed immediate: `0x489a80`
/// (`TargetNearestEnemy`) pushes `edx = 1`, `0x489aa0` (`TargetNearestFriend`) pushes `edx = 2`,
/// and `0x489ac0`/`0x489ae0` push 3 and 4 for the party and raid siblings — every one of them then
/// calls the single cycler `0x493f60(ecx = reverse, edx = mode)`. The mode reaches exactly one
/// place: `0x493e40`'s jump table (`0x493f50 = {0x493e73, 0x493eca, 0x493eed, 0x493f15}`). The
/// enumeration, the creature-type table, the range cvars, the scene-attach gate, the scorer
/// `0x494200`, the comparator `0x494450` and the commit `0x493540` are literally the same
/// instructions for both sides — so this is a parameter on the one scan, never a second scanner.
///
/// Modes 3 and 4 (party / raid) are not built: they need the roster-only candidate set, and the
/// two commands that drive them stay in the binding registry's absent table until they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScanSide {
    /// **Mode 1** (`0x493e73`) — the liveness triple (`0x605f90`, health, dynflag/stand-state) and
    /// `CanAttack 0x606980`.
    Enemy,
    /// **Mode 2** (`0x493eca`) — `CanAssist 0x6066f0`, then `UNIT_FIELD_HEALTH > 0`, and nothing
    /// else. Two things fall out of that being *shorter* than mode 1 rather than its mirror: a
    /// **feigning** friendly (health > 0, `UNIT_DYNFLAG_DEAD` set) is still a candidate, because
    /// mode 2 has no dynflag leg; and `CanAssist`'s own ladder is **friendly or better** — neutral
    /// fails (`0x60671e cmp eax,4; jge`, on the internal 0-based rank, so Lua's FRIENDLY(5) is
    /// internal 4). Its NPC arm is `IsPvP 0x605ff0` on the candidate (owner-chased), so a friendly
    /// creature with no `UNIT_FLAG_PVP` is NOT a `TargetNearestFriend` candidate at all — the
    /// same predicate that already denies its buffs (decision 1035).
    Friend,
}

/// One scored candidate. `score`: lower is better. `on_screen`: inside the pulled-in frustum
/// (the tier-1 gate). The fighting-me bonus is already folded into `score` at build time.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    entity: Entity,
    guid: u64,
    on_screen: bool,
    score: f32,
}

/// The priority value (`TargetPriorityValueBank` stand-in): screen off-centerness (absent for
/// off-screen candidates — the tier gate keeps them apart, so their mutual order is distance +
/// combat only) + normalized distance − the attacking-me bonus. Lowest wins.
fn priority_score(off_center: Option<f32>, dist: f32, combat_with_me: bool) -> f32 {
    off_center.map_or(0.0, |c| W_SCREEN * c) + W_DIST * (dist / TAB_RANGE)
        - if combat_with_me {
            COMBAT_WITH_ME_BONUS
        } else {
            0.0
        }
}

/// The sort: tier first (on-screen before off-screen — `AllowAnyOnScreen` keeps the fallback
/// tier from ever outranking a "100% correct" pick), then ascending score.
fn candidate_order(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    b.on_screen
        .cmp(&a.on_screen)
        .then(a.score.total_cmp(&b.score))
}

/// The press's pick pool from the sorted candidates: the on-screen tier when it exists, else
/// everything (`AllowAnyOnScreen=1` — "if no 100% correct target is available, allow selecting
/// any valid in-range target"). Classic's HARD combat lock (`TargetPriorityCombatLock` — the
/// pool restricted to in-combat units while starting from one) was implemented and then
/// **retired by the director's feel** (decision 0568): with a single attacker it pinned TAB to
/// that mob with no way off. The fighting-me *score bonus* stays — attackers come first, but
/// the history walks you off them.
fn select_pool(cands: &[Candidate]) -> Vec<Candidate> {
    if cands.iter().any(|c| c.on_screen) {
        cands.iter().filter(|c| c.on_screen).copied().collect()
    } else {
        cands.to_vec()
    }
}

/// The forward pick over a sorted pool: the best candidate that is neither the current
/// selection nor recently visited; all visited → the wrap (best non-current, history to be
/// cleared by the caller); only the current one left → itself (the commit dedups to a no-op).
/// Returns `(index, wrapped)`.
fn pick_forward(
    pool: &[Candidate],
    visited: &[u64],
    current: Option<u64>,
) -> Option<(usize, bool)> {
    if pool.is_empty() {
        return None;
    }
    if let Some(i) = pool
        .iter()
        .position(|c| Some(c.guid) != current && !visited.contains(&c.guid))
    {
        return Some((i, false));
    }
    if let Some(i) = pool.iter().position(|c| Some(c.guid) != current) {
        return Some((i, true));
    }
    Some((0, true))
}

/// The **backward** pick (Shift-TAB / `TargetNearestFriend(1)` — the reference's `reverse` flag):
/// walk the history newest-first for a guid that is neither the current selection nor gone from
/// the pool. `None` = nothing to step back to, and the caller falls through to [`pick_forward`],
/// which is what makes the very first reverse press behave like a forward one.
///
/// The reference cycles a *snapshot list* by a cursor and simply decrements it (`cursor == 0 ?
/// count-1 : cursor-1`, no timer check — the asymmetry is its own); decision 0567 replaced the
/// snapshot with this history, so "back" means "the target before this one" rather than "the
/// previous array slot". Pure, and beside its forward twin, so the two are read together.
fn pick_back(visited: &[u64], pool: &[Candidate], current: Option<u64>) -> Option<usize> {
    visited.iter().rev().find_map(|g| {
        (Some(*g) != current)
            .then(|| pool.iter().position(|c| c.guid == *g))
            .flatten()
    })
}

/// "Fighting me": the unit's UNIT_FIELD_TARGET is my guid AND its in-combat flag
/// (UNIT_FLAG_IN_COMBAT, bit 19) is set — the `TargetPriorityCombatLock=2` notion ("in-combat
/// with player"), the one that matters solo.
fn combat_with_me(store: &ObjectStore, me: Option<u64>) -> bool {
    me.is_some() && store.0.unit_target() == me && store.0.unit_flags() & (1 << 19) != 0
}

/// The CreatureType.dbc flags table (see `benilla-formats`); absent (load failure) = no
/// critter filtering, like the missing-catalog fallbacks elsewhere.
///
/// **Critter alone** — the flag column is 1 for CreatureType 8 and for nothing else in the shipped
/// 5875 DBC (Totem reads 0, and 1.12 has no non-combat-pet row). This prose said "critter/totem";
/// the code was always reading the flag rather than a type list, so only the
/// words were wrong.
#[derive(Resource)]
pub(crate) struct CreatureTypes(CreatureTypeFlags);

/// Startup (after the MPQ chain opens): load CreatureType.dbc for the critter TAB filter.
pub(super) fn load_creature_types(mut commands: Commands, world_assets: Option<Res<WorldAssets>>) {
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match benilla_formats::load_creature_type_flags(&mut chain) {
        Ok(flags) => {
            info!("creature types: {} rows", flags.len());
            commands.insert_resource(CreatureTypes(flags));
        }
        Err(e) => warn!("CreatureType.dbc unavailable, critters stay TAB-able: {e:#}"),
    }
}

/// Everything the scan core reads, bundled (the TAB and attack-acquire systems share it).
///
/// `pub(crate)` rather than `pub(super)` because the **pet bar's** ATTACK arm runs the same
/// `TargetNearestEnemy()` the player's does ([`attack_order_target`]), and its drain lives in
/// `ui_pet`.
///
/// # `Without<SelfPlayer>` is a divergence, and a deliberate one
///
/// **The reference's friendly cycler can select YOU.** Nothing on the `0x493f60` chain excludes
/// the player: `CanAssist(P, P)` is true (`0x6061e0` returns 4 when A == B) and the cone test
/// `0x47f220(p, p)` returns exactly π/2, so the self entry is in-cone whenever your facing falls
/// in (π/3, 2π/3) and — at distance² 0 — sorts first. Plain TAB is spared only because
/// `CanAttack(P, P)` is false. (A faithful re-implementation adds the player to both candidate
/// sets.)
///
/// We do not, and the reason is 0567: that behaviour is an artifact of the ±30° facing cone, and
/// 0567 replaced the cone with screen-space priority. Under our scoring the player is at distance
/// 0 and screen-centre *unconditionally*, so porting the byte-level rule would make `CTRL-TAB`
/// self-target on **every** press rather than facing-dependently — faithful to the bytes,
/// unfaithful to the behaviour, which is the trade `docs/METHOD.md` §7 hands to the director rather
/// than to this file. Recorded here so it is a known divergence and not a gap (decision 1745).
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled system param — the app's convention for big query sets
pub(crate) struct TargetScan<'w, 's> {
    units: Query<
        'w,
        's,
        (
            Entity,
            &'static NetEntity,
            &'static Guid,
            &'static Transform,
            Option<&'static ObjectStore>,
        ),
        Without<SelfPlayer>,
    >,
    self_q: Query<
        'w,
        's,
        (
            &'static Transform,
            Option<&'static ObjectStore>,
            Option<&'static Guid>,
        ),
        With<SelfPlayer>,
    >,
    /// The live world camera — the in-view check's frustum. Absent (glue screens, headless):
    /// nothing projects, everything is fallback-tier, and the pick degrades to distance+combat.
    camera: Query<
        'w,
        's,
        (&'static Camera, &'static Transform),
        (With<benilla_world::view::WorldCamera>, Without<SelfPlayer>),
    >,
    factions: Option<Res<'w, Factions>>,
    reputations: Res<'w, Reputations>,
    names: Res<'w, NameCache>,
    creature_types: Option<Res<'w, CreatureTypes>>,
    /// Every store, ours included — read only by the friend arm's `CanAssist`, whose `IsPvP` leg
    /// chases the candidate's owner. `units` above cannot serve: it excludes our own body, which
    /// is exactly who owns our pet.
    stores: Query<'w, 's, &'static ObjectStore>,
    /// The guid → entity map the owner chase lands in. `Option` for the same reason
    /// [`crate::ui_unit::UnitTokens`] carries it that way — a UI-only harness runs with no net
    /// stack, and a bare `Res` would turn that into a system-validation panic.
    index: Option<Res<'w, GuidIndex>>,
}

impl TargetScan<'_, '_> {
    /// The `0x493e40` per-candidate validity filter, both arms — the one thing [`ScanSide`] forks.
    fn is_valid(
        &self,
        side: ScanSide,
        store: Option<&ObjectStore>,
        self_store: Option<&ObjectStore>,
    ) -> bool {
        match side {
            // Mode 1 (`0x493e73`) — liveness + hostility (kept byte-law). The liveness leg is the
            // reference's own reads-dead triple `0x605f90` — health, the `UNIT_DYNFLAG_DEAD` bit
            // (feign death) and stand state 7 — the shared predicate rather than a third
            // transcription of it (decision 1022).
            ScanSide::Enemy => {
                if store.is_some_and(|s| s.0.unit_reads_dead()) {
                    return false;
                }
                can_attack(
                    store,
                    self.factions.as_deref(),
                    &self.reputations,
                    self_store,
                )
            }
            // Mode 2 (`0x493eca`) — `CanAssist 0x6066f0` first, then `[[cand+0x110]+0x40] > 0`.
            // The order is the reference's and so is the *narrowness* of the liveness leg: it
            // reads raw HEALTH, not the reads-dead triple, so this deliberately does NOT reuse
            // `unit_reads_dead` (see [`ScanSide::Friend`]).
            ScanSide::Friend => {
                can_assist(
                    store,
                    self.factions.as_deref(),
                    &self.reputations,
                    self_store,
                    |owner| self.store_of(owner).cloned(),
                ) && !store.is_some_and(|s| s.0.unit_is_dead())
            }
        }
    }

    /// A guid's streamed descriptor, whichever entity holds it — `CanAssist`'s owner chase.
    fn store_of(&self, guid: u64) -> Option<&ObjectStore> {
        let entity = *self.index.as_ref()?.0.get(&guid)?;
        self.stores.get(entity).ok()
    }

    /// Our own descriptor, for whichever leg needs the reaction's second party outside [`build`].
    fn self_store(&self) -> Option<&ObjectStore> {
        self.self_q.single().ok().and_then(|(_, store, _)| store)
    }

    /// Our own guid — `IsSelectable`'s `CREATEDBY` clause needs the active player's, and so does
    /// the commit's self-target exception.
    fn self_guid(&self) -> Option<u64> {
        self.self_q
            .single()
            .ok()
            .and_then(|(_, _, g)| g)
            .map(|g| g.0)
    }

    /// An entity's streamed descriptor — the commit gate's `IsSelectable` input.
    fn store_at(&self, entity: Entity) -> Option<&ObjectStore> {
        self.stores.get(entity).ok()
    }

    /// `0x6130a3`'s keep-or-drop test on a guid we are already holding: is the actor hostile to it?
    ///
    /// The reference reads `0x6061e0(actor, target) >= 4` — the **reaction alone**, not the full
    /// `0x606980` CanAttack, which it saves for the final gate at `0x613167`. A guid whose unit is
    /// not streamed answers `false`, which is the same exit `0x613099` takes when its object lookup
    /// misses.
    ///
    /// **The direction is ACTOR → target**, byte-read at the call: `0x61309b push eax` (the
    /// candidate) `; mov ecx,edi` (the actor) `; call 0x6061e0`, then `cmp eax,4; jl keep`. So it is
    /// [`reaction_from_player`], the leg-3 direction that answers a reputation-slot faction with the
    /// **at-war bit** — *not* [`ring_reaction`], which this used to call and which reads the
    /// standing from the other side (1674; the two are deliberately not each other's mirror, and
    /// substituting one for the other is the defect 1530 named). With the standing here, pressing
    /// Attack while holding a not-at-war Cenarion Circle NPC kept it as the target and then failed
    /// the final gate; the reference drops it and goes and finds a real enemy.
    ///
    /// **The actor is the player here, and in the reference it is the caller's** — the pet on the
    /// pet arm (`0x4bd40d` passes the pet object). The two agree: vmangos gives a pet its owner's
    /// faction template outright (`Pet.cpp:248`), and the reputation leg is the owner's by
    /// definition. Reading the pet's own store instead would also fight [`build`], whose entire
    /// candidate scan is player-relative (screen, distance, "fighting me").
    fn reaction_hostile(&self, guid: u64) -> bool {
        let self_store = self.self_store();
        self.units
            .iter()
            .find(|(_, _, g, _, _)| g.0 == guid)
            .is_some_and(|(_, _, _, _, store)| {
                reaction_from_player(
                    self.factions.as_deref(),
                    &self.reputations,
                    store,
                    self_store,
                ) < 4
            })
    }

    /// The streamed unit behind a guid, as `(entity, store)` — the final gate's input.
    fn unit_by_guid(&self, guid: u64) -> Option<(Entity, Option<&ObjectStore>)> {
        self.units
            .iter()
            .find(|(_, _, g, _, _)| g.0 == guid)
            .map(|(e, _, _, _, store)| (e, store))
    }

    /// The full build: walk every known unit, filter (the kept 1.12 legality laws), project
    /// through the live camera, score, sort (tier, then score). Fresh every press — the live
    /// world is the list (no snapshot to go stale).
    ///
    /// `side` reaches exactly one line — [`Self::is_valid`] — which is the reference's own shape:
    /// the mode byte forks `0x493e40` and nothing else on the path.
    ///
    /// **Our own body is never a candidate**: the query is `Without<SelfPlayer>`. For the enemy
    /// side that is free (`CanAttack(me, me)` is false anyway); for the friend side it is a
    /// deliberate divergence, because the reference's enumeration walks ClntObjMgr table #1 —
    /// which holds the player's own object — and no self-compare has been derived on the mode-2
    /// path. Self sits at dist² 0 dead-centre, so a faithful `TargetNearestFriend` would appear to
    /// always self-target; we refuse to ship that on an underived gate. RE dispatched.
    fn build(&self, side: ScanSide) -> Vec<Candidate> {
        let Ok((self_tf, self_store, self_guid)) = self.self_q.single() else {
            return Vec::new();
        };
        let me = self_guid.map(|g| g.0);
        let cam = self.camera.single().ok();
        // Normalized off-centerness of a world point on the pulled-in screen: `None` when
        // off-screen / behind the camera / outside the pulled-in frustum; `Some(0)` at the
        // viewport center, ~1 at a corner. The point is the unit's root + 1 yd (chest-ish) so
        // a mob whose feet sit just below the pulled-in bottom edge still reads as on-screen.
        let project = |world: Vec3| -> Option<f32> {
            let (camera, cam_pose) = cam?;
            let cam_tf = GlobalTransform::from(*cam_pose);
            let vp = camera.logical_viewport_size()?;
            let screen = camera.world_to_viewport(&cam_tf, world).ok()?;
            let x0 = vp.x * FRUSTUM_PULL_SIDES;
            let x1 = vp.x * (1.0 - FRUSTUM_PULL_SIDES);
            let y0 = vp.y * FRUSTUM_PULL_TOP;
            let y1 = vp.y * (1.0 - FRUSTUM_PULL_BOT);
            if screen.x < x0 || screen.x > x1 || screen.y < y0 || screen.y > y1 {
                return None;
            }
            Some(screen.distance(vp * 0.5) / (vp.length() * 0.5))
        };
        let trace = tab_trace_on();
        let trace_unit = |guid: u64, tf: &Transform, verdict: &str| {
            if !trace {
                return;
            }
            info!(
                "tab-trace:   {guid:#x} \"{}\" {:.1} yd — {verdict}",
                self.names.peek(guid).unwrap_or("?"),
                (tf.translation - self_tf.translation).length()
            );
        };
        let mut out = Vec::new();
        for (entity, net, guid_c, tf, store) in &self.units {
            if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
                continue;
            }
            if !self.is_valid(side, store, self_store) {
                trace_unit(
                    guid_c.0,
                    tf,
                    match side {
                        ScanSide::Enemy => "REJECT dead-or-unattackable",
                        ScanSide::Friend => "REJECT dead-or-unassistable",
                    },
                );
                continue;
            }
            // The creature-type gate (CreatureType.dbc flag bit 0 — critters; kept byte-law).
            // An unresolved type (name not yet queried) passes, like the client's
            // out-of-range table index.
            if guid::is_creature_or_pet(guid_c.0) {
                if let (Some(types), Some(ty)) = (
                    self.creature_types.as_deref(),
                    guid::entry(guid_c.0).and_then(|e| self.names.creature_type(e)),
                ) {
                    if types.0.no_tab_target(ty) {
                        trace_unit(guid_c.0, tf, "REJECT critter-type");
                        continue;
                    }
                }
            }
            // **No scene-attach gate.** The reference rejects a not-currently-rendered object
            // here (`0x6704c0`), and this scan kept that while nothing hid a unit root. Since the
            // outdoor draw election (1270/1475) hides every body outside the frustum, reading its
            // `Visibility` would be the frustum by another name — and would empty the off-screen
            // tier the Classic design (0567) keeps as the fallback. Tiering by the camera is the
            // `project` step below; stealth and the like leave through the server's out-of-range.
            let dist = (tf.translation - self_tf.translation).length();
            if dist > TAB_RANGE {
                trace_unit(guid_c.0, tf, "REJECT range (41 yd)");
                continue;
            }
            let off_center = project(tf.translation + Vec3::Y);
            // **Enemy side only.** The bonus exists to surface *the thing attacking me*
            // (`TargetPriorityCombatLock`'s spirit, decisions 0567/0568) — a notion with no
            // friendly meaning, and the reference has no such term on either side. Left ungated it
            // would fire on an ally who is in combat and targeting me — a healer on you — and
            // shove them to the head of the CTRL-TAB pool ahead of everyone, by accident rather
            // than by anybody's decision.
            let cwm = side == ScanSide::Enemy && store.is_some_and(|s| combat_with_me(s, me));
            let score = priority_score(off_center, dist, cwm);
            if trace {
                let screen = off_center
                    .map(|c| format!("center+{c:.2}"))
                    .unwrap_or_else(|| "OFF-SCREEN".into());
                trace_unit(
                    guid_c.0,
                    tf,
                    &format!(
                        "{screen}{} score {score:+.2}",
                        if cwm { " FIGHTING-ME" } else { "" }
                    ),
                );
            }
            out.push(Candidate {
                entity,
                guid: guid_c.0,
                on_screen: off_center.is_some(),
                score,
            });
        }
        out.sort_by(candidate_order);
        if trace {
            for (i, c) in out.iter().enumerate() {
                info!(
                    "tab-trace: list[{i}] {:#x} \"{}\" {} score {:+.2}",
                    c.guid,
                    self.names.peek(c.guid).unwrap_or("?"),
                    if c.on_screen { "on-screen" } else { "fallback" },
                    c.score
                );
            }
        }
        out
    }
}

/// The recent-TAB history (`TargetPriorityHighlightHistoryMs`): guids picked within
/// [`HISTORY_SECS`], skipped by the forward pick so repeated presses walk fresh targets; a wrap
/// clears it. Replaces the 1.12 snapshot list + cursor (decision 0567).
#[derive(Resource, Default)]
pub(super) struct TabHistory {
    /// `(guid, when)` — insertion-ordered; the back is the most recent (Shift-TAB's walk).
    visited: Vec<(u64, f64)>,
    /// Which side the standing history belongs to. **One history, cleared on a side switch** —
    /// the reference's `cachedMode != mode` rebuild condition (`0x493f60`), which resets the
    /// cursor to 0 and drops the candidate list whenever the mode byte changes. Without it a
    /// CTRL-TAB through the friendly pool would poison the enemy cycle's skip set.
    side: Option<ScanSide>,
}

impl TabHistory {
    fn prune(&mut self, now: f64) {
        self.visited.retain(|&(_, t)| now - t < HISTORY_SECS);
    }
    /// `0x493f60`'s cached-mode check: a press on the other side starts from nothing.
    fn enter(&mut self, side: ScanSide) {
        if self.side != Some(side) {
            self.visited.clear();
            self.side = Some(side);
        }
    }
    fn guids(&self) -> Vec<u64> {
        self.visited.iter().map(|&(g, _)| g).collect()
    }
    /// Record a visit (re-visiting moves the guid to most-recent).
    fn push(&mut self, guid: u64, now: f64) {
        self.visited.retain(|&(g, _)| g != guid);
        self.visited.push((guid, now));
    }
}

/// What one [`commit`] did: whether the selection changed, and whether the engaged-switch law
/// already re-pointed the swing (`CMSG_ATTACKSWING` at the new target) — the attack branches
/// read `swung` to avoid double-sending their own swing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CommitOutcome {
    pub(super) changed: bool,
    pub(super) swung: bool,
}

/// Commit a target through the SetSelection path (`0x493540`), byte-complete: dedup, then — on a
/// switch while auto-attacking — the **stop → select → re-swing** law read directly from the
/// binary:
///
/// 1. `0x493637–0x4936ac` latches "attacking AND the old target is still a live attackable unit"
///    into `[ebp-1]` before anything changes;
/// 2. the silent old-target clear (`0x493910(current, ecx=0)`) calls **StopAttack `0x5ecac0`
///    unconditionally at `0x493a08`** (engaged-gated internally) → `CMSG_ATTACKSTOP`, and its
///    builder `0x624370` sets `[player+0xc54] = 1` ("stop in flight");
/// 3. the new guid lands in the globals and `CMSG_SET_SELECTION` goes out (`0x493857`);
/// 4. the tail `0x4938a1–0x4938c8`: iff `[ebp-1]` and the new selection is not MYSELF →
///    `Attack 0x5ecb70`, which validates the NEW target (alive-or-feign + `CanAttack 0x606980`,
///    `5ecc10–5ecc35` — an invalid one STOPS instead, `5ecc37`) and then **sends
///    `CMSG_ATTACKSWING` at it** — `[+0xc54]` bypasses the already-attacking gate (`5eccda`) so
///    the swing goes out despite the still-set old lock, and the lock re-stores to the new guid
///    (`5ecd4b`). The swing FOLLOWS the switch (no sheath ceremony: engaged means the weapon is
///    already out). The `new_attackable` param carries Attack's new-target validation — callers
///    pass their already-computed classification (the cursor's Attack kind / the scan's
///    alive+hostile) rather than re-deriving it here.
///
/// Approximations, disclosed: `[ebp-1]`'s old-target legs (alive-or-feign + `CanAttack` + the
/// `0x605f30`/`+0x1fc` player-state gates) collapse to "an old selection existed" — on vmangos
/// the victim's death tears `Engaged` down via the echoed `SMSG_ATTACKSTOP` before a human can
/// TAB, so `engaged && had_old` covers the same observable; our `engaged` is the server-echoed
/// [`Engaged`], not the ref's instant local lock (one-RTT lag on a switch fired mid-attack-
/// start); and the ref's invalid-new-target Attack sends a second (duplicate) ATTACKSTOP we
/// don't. Shared with the UI's `TargetUnit` drain (`super::target_unit_requests`) so every
/// non-mouse selection writer commits identically — a `TargetUnit("player")` mid-combat stops
/// the swing and does NOT re-point (the tail's self exception).
pub(super) fn commit(
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    entity: Entity,
    guid: u64,
    target_store: Option<&ObjectStore>,
    engaged: bool,
    self_guid: Option<u64>,
    new_attackable: bool,
) -> CommitOutcome {
    // **`IsSelectable`, and it comes BEFORE the dedup** — `0x4935ec`–`0x4935f3`, the third of
    // `0x493540`'s early-outs: the
    // resolved object's slot-`+0x58` vcall answering 0 is a bare RETURN, so a non-selectable unit
    // is a **complete** no-op — no stop, no `CMSG_SET_SELECTION`, no `PLAYER_TARGET_CHANGED`, no
    // re-swing, and the target you already had is left exactly where it was.
    //
    // This is the one gate, at the one place the reference puts it, so every writer — the click,
    // the right-click, TAB, `TargetUnit`/`/target`/`/assist`, the attack auto-acquire and the
    // ATTACKERSTATEUPDATE self-defence acquire — is covered without knowing it exists. Without it
    // a Stratholme Baron Rivendare in his pre-event state (vmangos gives him `NOT_SELECTABLE |
    // SPAWNING` until the ziggurats fall) took a target on the very swing he opened with.
    if !super::relations::is_selectable(target_store, self_guid) {
        return CommitOutcome::default();
    }
    if selection.guid == Some(guid) {
        return CommitOutcome::default(); // the setter's dedup: bail if already current
    }
    let had_old = selection.guid.is_some();
    selection.target = Some(entity);
    selection.guid = Some(guid);
    let stop_and_repoint = engaged && had_old;
    if stop_and_repoint {
        // `0x493a08 call 0x5ecac0` — the real StopAttack, so the switch also cancels a queued
        // on-next-swing strike (the seam's `0x6e6f30` tail). Before this routed through the seam
        // it was a bare `CMSG_ATTACKSTOP` and a queued Heroic Strike survived the switch.
        seam.stop(engaged);
    }
    let _ = seam.net.0.send(ClientCommand::SetSelection { guid });
    let swung = stop_and_repoint && new_attackable && self_guid != Some(guid);
    if swung {
        // `0x4938c8 call 0x5ecb70` with a stop in flight (`[+0xc54]`, set by the call above), so
        // the swing goes out despite the still-set lock. Its tail cancels a running auto-repeat —
        // this chain does reach `0x6ea080`, which a direct-callers-only census misses.
        seam.start(guid, engaged, true);
    }
    CommitOutcome {
        changed: true,
        swung,
    }
}

/// **One press, either side** — the whole of the Classic-priority cycle, shared by the enemy TAB
/// ([`tab_target`]) and the friendly one ([`target_nearest_friend_requests`]) because the
/// reference shares it too: all four `TargetNearest*` shims are one call into `0x493f60` with a
/// different mode byte, and the byte reaches only the per-candidate filter ([`ScanSide`]).
///
/// Re-score the live world, pool by tier, skip the recent history forward (or walk it backward),
/// commit through the byte-law [`commit`].
fn cycle(
    side: ScanSide,
    reverse: bool,
    now: f64,
    scan: &TargetScan,
    history: &mut TabHistory,
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    engaged: bool,
) {
    history.enter(side);
    history.prune(now);
    let trace = tab_trace_on();
    if trace {
        info!(
            "tab-trace: {side:?} press (reverse={reverse}), selection {:?}, history {}",
            selection.guid.map(|g| format!("{g:#x}")),
            history.visited.len()
        );
    }
    let cands = scan.build(side);
    if cands.is_empty() {
        if trace {
            info!("tab-trace: no candidates — target unchanged");
        }
        return;
    }
    let pool = select_pool(&cands);
    if trace {
        info!(
            "tab-trace: pool {}/{} ({} tier)",
            pool.len(),
            cands.len(),
            if cands.iter().any(|c| c.on_screen) {
                "on-screen"
            } else {
                "fallback"
            }
        );
    }
    // The pick. Reverse walks the history back ([`pick_back`]); an empty walk falls through to the
    // forward rule.
    let visited = history.guids();
    let back = reverse
        .then(|| pick_back(&visited, &pool, selection.guid))
        .flatten();
    let (entity, guid, wrapped) = match back {
        Some(i) => (pool[i].entity, pool[i].guid, false),
        None => {
            let Some((i, wrapped)) = pick_forward(&pool, &visited, selection.guid) else {
                return;
            };
            (pool[i].entity, pool[i].guid, wrapped)
        }
    };
    if wrapped {
        history.visited.clear();
    }
    // The outgoing selection joins the history — a clicked target counts as visited once
    // tabbed away from (`TargetPriorityContinueFromManualTarget`'s observable).
    if let Some(old) = selection.guid {
        history.push(old, now);
    }
    let out = commit(
        selection,
        seam,
        entity,
        guid,
        scan.store_at(entity),
        engaged,
        // The real guid, where this used to pass `None`: `IsSelectable`'s `CREATEDBY` clause reads
        // it. Inert for the swing leg it was already feeding — the candidate query excludes our own
        // body, so a TAB pick can never *be* us.
        scan.self_guid(),
        // Attack `0x5ecb70`'s new-target validation, which is what this flag carries: a mode-2
        // pick can never pass it. `CanAssist` demands reaction ≥ 4 and `CanAttack`'s mixed arm
        // demands < 4, so the two candidate sets are disjoint on that leg — a CTRL-TAB in the
        // middle of a fight moves the selection and must NOT re-point the swing at an ally.
        // (Disclosed corner: `CanAttack`'s duel / FFA-PvP arms don't read the reaction, so a
        // duelling friendly is attackable AND assistable; we still refuse the re-swing there,
        // which is the conservative half.)
        side == ScanSide::Enemy,
    );
    if out.changed {
        history.push(guid, now);
    }
    if trace {
        info!(
            "tab-trace: pick {guid:#x} \"{}\"{} — {}",
            scan.names.peek(guid).unwrap_or("?"),
            if wrapped { " (wrapped)" } else { "" },
            if out.changed {
                "selection changed"
            } else {
                "NO-OP (already the selection)"
            }
        );
    }
}

/// TARGETNEARESTENEMY / TARGETPREVIOUSENEMY (0997: two commands through the binding table now,
/// defaults TAB / SHIFT-TAB — no shift fork here anymore). The dispatch already applied the typing
/// gate (a focused EditBox owns TAB) and the exact-modifier law.
pub(super) fn tab_target(
    binds: Res<crate::bindings::BindingsState>,
    time: Res<Time>,
    scan: TargetScan,
    mut history: ResMut<TabHistory>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
) {
    let reverse = binds.fired(crate::bindings::cmd::TARGET_PREVIOUS_ENEMY);
    if !reverse && !binds.fired(crate::bindings::cmd::TARGET_NEAREST_ENEMY) {
        return;
    }
    cycle(
        ScanSide::Enemy,
        reverse,
        time.elapsed_secs_f64(),
        &scan,
        &mut history,
        &mut selection,
        &mut seam,
        !engaged.is_empty(),
    );
}

/// Drain `TargetNearestFriend([reverse])` — the same cycle with mode 2 (`0x489aa0` →
/// `0x493f60(reverse, 2)`).
///
/// Lua-driven rather than binding-driven, because that is where the reference puts it: TAB reaches
/// the enemy scan through a Rust command here only because 0997 wired the two enemy commands
/// straight to [`tab_target`]; the friendly pair's `Bindings.xml` bodies are literally
/// `TargetNearestFriend()` and `TargetNearestFriend(1)`, so a binding row for them needs no new
/// mechanism — it needs the global, which now exists. Every FrameXML and addon caller lands here
/// too.
///
/// Each queued press is one cycle, in call order, exactly as repeated key presses would be.
pub(super) fn target_nearest_friend_requests(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    time: Res<Time>,
    scan: TargetScan,
    mut history: ResMut<TabHistory>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
) {
    let Some(mut script) = script else {
        return;
    };
    let presses = script.take_target_nearest_friend_requests();
    if presses.is_empty() {
        return;
    }
    let now = time.elapsed_secs_f64();
    let engaged = !engaged.is_empty();
    for reverse in presses {
        cycle(
            ScanSide::Friend,
            reverse,
            now,
            &scan,
            &mut history,
            &mut selection,
            &mut seam,
            engaged,
        );
    }
}

/// `0x6130b5` — **`TargetNearestEnemy()` itself**, and the whole of "pressing Attack at nothing
/// finds something in front of you".
///
/// The acquire the validator runs is not a private helper: it is `0x493f60(ecx = 0, edx = 1)`, the
/// same cycler the four Lua `TargetNearest*` bindings drive (`0x489a80`–`0x489af3` are the same
/// call with `edx = 1..4` for enemy / friend / party / raid, and `ecx` the *reverse* flag). So
/// "in front of me" is not a separate rule with its own cone — it is this module's priority scan,
/// which is why the acquire commits through [`commit`] like any press: `CMSG_SET_SELECTION` goes
/// out and **your** target really moves, which is what lets the pet's order and your own swing
/// aim at the same thing.
///
/// Returns the committed `(entity, guid)`, or `None` after raising `0x6130d9`'s
/// `ERR_NO_ATTACK_TARGET`.
///
/// Divergence, deliberate and inherited: the reference's cycler keeps a cached, time-expiring
/// candidate list and a cursor into it, so a *repeated* acquire walks; ours re-scores the live
/// world every call and always hands back the head. That is decision 0567's trade for the whole
/// module (history cycling replaces the snapshot cursor), and an acquire is a fresh pick in both.
fn acquire_nearest_enemy(
    scan: &TargetScan,
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    errors: &mut crate::ui_action::UiErrorKeys,
) -> Option<(Entity, u64)> {
    let cands = scan.build(ScanSide::Enemy);
    let Some(c) = cands.first() else {
        // `0x6130d9` — the acquire ran and came back empty, so the selection re-read at `0x6130c1`
        // still finds nothing: errorId `0xa0` `ERR_NO_ATTACK_TARGET`.
        debug!("attack acquire: nothing to attack (ERR_NO_ATTACK_TARGET)");
        errors
            .0
            .push(crate::ui_action::UiError::key("ERR_NO_ATTACK_TARGET"));
        return None;
    };
    // `engaged = false`: the switch law only fires when we were already swinging at the OLD
    // selection, and every path that reaches here had no selection or a non-hostile one — neither
    // is a thing you can be engaged with.
    commit(
        selection,
        seam,
        c.entity,
        c.guid,
        scan.store_at(c.entity),
        false,
        scan.self_guid(),
        false,
    );
    Some((c.entity, c.guid))
}

/// `0x612df0`'s **Phase B** — the attack order's *target* arm, and the reason pressing the pet's
/// Attack button at nothing still sends the pet at something.
///
/// Phase A (the actor's own eligibility — [`crate::ui_action::attack_actor_refusal`]) has already
/// run by the time this is reached; what is left is choosing **what** to attack. The candidate the
/// caller passes is `{0,0}` for a bare press, and it gets replaced by the current selection twice
/// over — once by `CastPetAction` itself for a nil Lua argument (`0x4bd212`), once by the
/// validator for a `{0,0}` candidate (`0x61306b`) — so the candidate *is* the selection:
///
/// ```text
/// 0x61306b  candidate == 0             -> candidate = the current selection
/// 0x61309b  0x6061e0(actor, candidate) -> the actor's reaction toward it
/// 0x6130a3  reaction >= 4              -> candidate = 0   (friendly or neutral is not a target)
/// 0x613100  else 0x493540(candidate)   ; promote it into the selection (a dedup when it came from there)
/// 0x6130b5  candidate == 0             -> 0x493f60(0, 1) == TargetNearestEnemy()
/// 0x6130c1  re-read the selection      ; whatever the acquire committed
/// 0x6130d9  still nothing              -> ERR_NO_ATTACK_TARGET, EAX = 0, no packet
/// 0x613167  final gate                 -> ERR_INVALID_ATTACK_TARGET, EAX = 0, no packet
/// ```
///
/// Two things fall out that are worth knowing before looking at it in game. **Pressing Attack
/// while you hold a friendly target retargets you** — the reaction test drops it and the acquire
/// runs, so your selection moves to the nearest enemy. And the guid this returns is what goes in
/// the packet: the validator writes its answer back through the caller's out-param (`0x6130c6`),
/// and `0x4bd491` reads that slot when it builds `CMSG_PET_ACTION`.
///
/// The final gate is transcribed rather than folded into the reaction test above it, because the
/// reference deliberately keeps them apart: a *dead* selection is `ERR_INVALID_ATTACK_TARGET`, not
/// a reason to go find something else. `0x613159`'s odd second leg is verbatim — a zero-health
/// target passes iff `UNIT_DYNAMIC_FLAGS` bit 5 is set.
pub(crate) fn attack_order_target(
    scan: &TargetScan,
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    errors: &mut crate::ui_action::UiErrorKeys,
) -> Option<u64> {
    // `0x61306b` + `0x6130a3`: the selection is the candidate, and it survives only if the actor is
    // hostile to it. Otherwise `0x6130b5` acquires.
    let guid = match keeps_held_target(selection.guid, |g| scan.reaction_hostile(g)) {
        Some(kept) => kept,
        None => acquire_nearest_enemy(scan, selection, seam, errors)?.1,
    };
    // `0x613167`'s final gate on whatever we ended up with. `0x61312e`'s legs are the ACTOR's and
    // are Phase A's, already run by the caller; these two are the target's.
    let store = scan.unit_by_guid(guid).and_then(|(_, s)| s);
    if !attack_target_valid(
        store,
        scan.factions.as_deref(),
        &scan.reputations,
        scan.self_store(),
    ) {
        debug!("attack order: {guid:#x} fails the final gate (ERR_INVALID_ATTACK_TARGET)");
        errors
            .0
            .push(crate::ui_action::UiError::key("ERR_INVALID_ATTACK_TARGET"));
        return None;
    }
    Some(guid)
}

/// `0x6130a3`'s fork on its own: keep what you are holding, or fall through to the acquire.
///
/// The whole content is that **the reaction alone decides**, and it decides against the *actor*.
/// A friendly or neutral selection is not "an invalid target" — the reference zeroes the candidate
/// (`0x6130a8`) and goes looking, which is why pressing pet-Attack while you have a quest giver
/// selected moves your target to the nearest mob instead of raising an error.
fn keeps_held_target(selection: Option<u64>, hostile: impl Fn(u64) -> bool) -> Option<u64> {
    selection.filter(|&g| hostile(g))
}

/// `0x613152`–`0x613169` — the final gate's two **target** legs, whatever route the guid arrived
/// by.
///
/// - alive, transcribed verbatim including its odd second half: a zero-health target passes iff
///   `UNIT_DYNAMIC_FLAGS` bit 5 is set (`0x613159 shr edx,5; test dl,1`);
/// - `CanAttack 0x606980` — the full predicate this time, not the bare reaction the fork above uses.
///
/// A target with no streamed descriptor is alive by the first leg (nothing known to disqualify) and
/// then **refused by the second**, which is [`can_attack`]'s own missing-data posture since 1674:
/// the reference reaches this gate holding two live CGUnits and has no null path at all, so the
/// honest stand-in is to refuse rather than to swing at an object we know nothing about. The result
/// is `ERR_INVALID_ATTACK_TARGET`, which is the same answer the gate gives for every other way a
/// target can be wrong.
fn attack_target_valid(
    store: Option<&ObjectStore>,
    factions: Option<&Factions>,
    reputations: &Reputations,
    self_store: Option<&ObjectStore>,
) -> bool {
    let alive = store.is_none_or(|s| {
        s.0.unit_health().is_none_or(|h| h > 0) || s.0.unit_dynamic_flags() & (1 << 5) != 0
    });
    alive && can_attack(store, factions, reputations, self_store)
}

/// Request from the action layer: the attack action fired with NO selection — pick the best
/// candidate and swing at it (`0x612df0` @ `6130b5`).
#[derive(Message)]
pub(crate) struct AttackNearestRequest;

/// Auto-acquire on attack (behavior 1): a fresh scan, commit the best candidate (the sort's
/// head — best on-screen, else best overall), then the attack-start pair (auto-draw +
/// `CMSG_ATTACKSWING`, exactly the action-bar attack's path). None found ⇒ the reference shows
/// error `0xa0` "There is nothing to attack" (we log; the red error banner is its own arc). The
/// TAB history is not touched — an auto-pick is not a press (Classic's
/// `TargetPriorityAutoTargetIgnoreWindow` nuance, disclosed unmodeled).
pub(super) fn acquire_and_attack(
    mut requests: MessageReader<AttackNearestRequest>,
    scan: TargetScan,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
    self_store: Query<&crate::net::ObjectStore, With<SelfPlayer>>,
    mut ui_error_keys: ResMut<crate::ui_action::UiErrorKeys>,
) {
    if requests.read().last().is_none() {
        return;
    }
    if selection.guid.is_some() {
        return; // something got selected between the action and this frame — the normal path owns it
    }
    // The actor-eligibility block (decision 0481, widened to `0x612df0`'s full Phase A): every one
    // of its refusals sits BEFORE the nearest-core `0x6130b5` — a mounted, stunned or dead press
    // never even scans. Gated at the responder so every requester (the Attack button's no-target
    // arm, the melee probe) shares it. The actor here is us.
    let self_guid = scan
        .self_q
        .iter()
        .next()
        .and_then(|(_, _, g)| g)
        .map(|g| g.0);
    if crate::ui_action::attack_actor_refusal(
        self_store.iter().next(),
        self_guid,
        &mut ui_error_keys,
    ) {
        return;
    }
    // `0x6130b5` and `0x6130d9`, shared with the pet bar's ATTACK arm — see
    // [`acquire_nearest_enemy`]. The selection is empty (checked above), which is exactly the
    // `candidate == 0` leg of [`attack_order_target`]; the follow-through below is what differs.
    let Some((_, guid)) =
        acquire_nearest_enemy(&scan, &mut selection, &mut seam, &mut ui_error_keys)
    else {
        return;
    };
    debug!("attack acquire: best candidate {guid:#x} → select + swing");
    // `0x6131a0`'s continuation — StartAttack through the one seam, so the acquire path gets the
    // auto-draw AND the auto-repeat cancel its hand-rolled copy used to miss. The selection was
    // empty on entry, so we cannot have been engaged: no stop is in flight.
    seam.start(guid, false, false);
}

/// **`TargetLastEnemy`'s memory** — the last *attackable* unit that was selected.
///
/// The reference keeps two guid pairs beside the current selection, both written inside
/// `SetSelection 0x493540`: `[0xb4e2e0]/[0xb4e2e4]`, the plain outgoing target (`TargetLastTarget`
/// reads it, `0x493622`/`0x493628` write it), and `[0xb4e2e8]/[0xb4e2ec]`, the last **attackable**
/// one (`TargetLastEnemy` reads it at `0x489b45`, `0x49377d` writes it). So the memory belongs at
/// the **selection
/// commit**, which is where this puts it, and it is deliberately not a second thing to remember at
/// each of [`commit`]'s six call sites: [`remember_last_enemy`] reads the frame's settled
/// selection instead, so a selection writer added later cannot forget to stamp.
///
/// **A stale guid is never cleared and never resurrects anything.** The reference's globals are
/// plain guids that nothing zeroes when the unit despawns; what protects it is the shim's route
/// through the select-if-resolves helper `0x489a40`, whose third arm — the guid resolves to no
/// streamed object and is on no roster — is a bare `ret`: **not a deselect**. So we keep the guid
/// and let the drain no-op on it (`crate::target::click`), which is the same observable.
#[derive(Resource, Default)]
pub(crate) struct LastEnemy(pub(crate) Option<u64>);

/// Stamp [`LastEnemy`] from the frame's settled selection — the `0x49377d` write, sampled at the
/// end of the target chain rather than threaded through [`commit`].
///
/// Sampling is not a shortcut: the observable "the last attackable guid I had selected" is the
/// same either way, because the only guid a commit could stamp is the one still standing here.
/// What it buys is that every present and future selection writer — the click, TAB, the two
/// auto-acquires, `TargetUnit`, `/target`, `/assist` — is covered without knowing this exists.
///
/// **Ordered before `ring::update_ring`'s death-clear on purpose**: a hostile that dies while
/// selected must still be remembered, because the reference's shim has no liveness gate either —
/// `TargetLastEnemy` back onto a corpse is faithful, and it is what you want a second after a kill.
///
/// **The whole gate, not just the attackability leg.** `0x49372f`–`0x493778` is five conjuncts and
/// this shipped with one of them: the player object resolves, **the player is
/// not dead or a ghost**, **the player is not mounted**, the new target reads
/// `HEALTH > 0 || UNIT_DYNFLAG_DEAD`, and `CanAttack(player, new)`. Without the middle three we
/// remembered in three states the reference does not — targeting a hostile while mounted, while
/// dead or ghost, or targeting an already-dead one — which is a `TargetLastEnemy` that lands
/// somewhere the real client's would not.
///
/// The odd-looking fourth conjunct is transcribed rather than simplified: `HEALTH > 0` **or** the
/// dead-looking dynflag, so a feigning unit stays remembered while a genuinely dead one does not.
///
/// One disclosed divergence from a commit-time stamp remains, and it is the sampling itself: a
/// unit selected while neutral that *turns* hostile while still selected is remembered here and
/// would not be at the reference's write, which runs once at `SetSelection`. That is the better
/// answer of the two, and it is the only case where they differ.
pub(super) fn remember_last_enemy(
    selection: Res<Selection>,
    stores: Query<&ObjectStore>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    mut last: ResMut<LastEnemy>,
) {
    let Some((entity, guid)) = selection.target.zip(selection.guid) else {
        return;
    };
    // Conjunct 1: the player object resolves at all.
    let Some(me) = self_store.iter().next() else {
        return;
    };
    // Conjuncts 2 and 3 — the two states of OUR body that suppress the stamp. `player_is_ghost`
    // is its own read because a ghost's wire health is 1, so `unit_is_dead` is false for one
    // (`player.rs`'s note on the UnitIsDead/UnitIsGhost/UnitIsDeadOrGhost trio).
    if me.0.unit_is_dead() || me.0.player_is_ghost() || me.0.unit_mount_display_id() != 0 {
        return;
    }
    let store = stores.get(entity).ok();
    // Conjunct 4 — the new target is not a corpse. Verbatim: health, OR the dead-looking flag.
    let alive_enough = store.is_some_and(|s| !s.0.unit_is_dead() || s.0.unit_dynflag_dead());
    if !alive_enough {
        return;
    }
    // Conjunct 5.
    if can_attack(store, factions.as_deref(), &reputations, Some(me)) {
        last.0 = Some(guid);
    }
}

/// Auto-acquire the attacker (behavior 2): the ATTACKERSTATEUPDATE victim handler's
/// self-defense block (`6259c9`–`6259fe`) — packet victim == me AND my selection strictly
/// empty → `SetSelection(attacker)`. Selection only: no counter-attack, never overwrites.
pub(super) fn auto_acquire_attacker(
    mut swings: MessageReader<SwingMessage>,
    self_player: Query<(Entity, &Guid), With<SelfPlayer>>,
    guids: Query<&Guid>,
    // The attacker's descriptor — `0x493540`'s `IsSelectable` gate reads it. A mob that swings at
    // us while flagged `NOT_SELECTABLE` must not become our target: this is the acquire the
    // Stratholme Baron's opening hit came in on.
    stores: Query<&ObjectStore>,
    mut selection: ResMut<Selection>,
    mut seam: crate::creature_anim::AttackSeam,
) {
    for s in swings.read() {
        if selection.target.is_some() {
            continue;
        }
        let Ok((me, my_guid)) = self_player.single() else {
            continue;
        };
        if s.victim != Some(me) {
            continue;
        }
        let Ok(guid) = guids.get(s.attacker) else {
            continue;
        };
        debug!(
            "auto-target: attacker {:#x} (victim = me, no target)",
            guid.0
        );
        // Selection is empty here (checked above): no engaged-switch law, selection only.
        commit(
            &mut selection,
            &mut seam,
            s.attacker,
            guid.0,
            stores.get(s.attacker).ok(),
            false,
            Some(my_guid.0),
            false,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::NetCommands;

    /// One `commit` through a one-shot system, returning its outcome.
    fn go(
        world: &mut World,
        guid: u64,
        engaged: bool,
        self_guid: Option<u64>,
        attackable: bool,
    ) -> CommitOutcome {
        go_with(world, guid, None, engaged, self_guid, attackable)
    }

    /// …and the same with a descriptor for the new target, which is what `IsSelectable` reads.
    /// `None` is the reference's *unresolved* object (`0x4935c8`), which skips the vcall.
    fn go_with(
        world: &mut World,
        guid: u64,
        store: Option<ObjectStore>,
        engaged: bool,
        self_guid: Option<u64>,
        attackable: bool,
    ) -> CommitOutcome {
        use bevy::ecs::system::RunSystemOnce;
        world
            .run_system_once(
                move |mut selection: ResMut<Selection>,
                      mut seam: crate::creature_anim::AttackSeam| {
                    commit(
                        &mut selection,
                        &mut seam,
                        Entity::PLACEHOLDER,
                        guid,
                        store.as_ref(),
                        engaged,
                        self_guid,
                        attackable,
                    )
                },
            )
            .expect("commit runs as a one-shot system")
    }

    /// A world with everything the seams need, and nothing else — both commit tests' fixture.
    fn commit_world() -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<Messages<crate::player::StandStateRequest>>();
        world.init_resource::<Selection>();
        world.spawn(SelfPlayer);
        (world, rx)
    }

    /// The `SetSelection` wire law, byte-read from `0x493540` (the director's "TAB while
    /// auto-attacking kills the attack" bug): an engaged SWITCH is **stop → select → re-swing**
    /// — the attack follows the new target — while a plain select sends only the selection, a
    /// self-target stops without re-pointing, and an unattackable new target (Attack `0x5ecb70`'s
    /// validation) switches and stops but never swings.
    ///
    /// Driven through a World because `commit` now runs the two real seams
    /// ([`crate::creature_anim::AttackSeam`]) rather than emitting bare packets — which is what
    /// the second half of this test pins: the switch's stop un-queues an on-next-swing strike
    /// (`0x493a08 call 0x5ecac0` → `0x6e6f30`) and its re-swing cancels a running auto-repeat
    /// (`0x4938c8 call 0x5ecb70` → `0x5ecd8c`). Both were missing while this path hand-rolled
    /// its own packets.
    #[test]
    fn commit_follows_the_stop_select_reswing_law() {
        let (mut world, rx) = commit_world();

        let drain = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            rx.try_iter()
                .map(|c| match c {
                    ClientCommand::SetSelection { .. } => "select",
                    ClientCommand::AttackStop => "stop",
                    ClientCommand::AttackSwing { .. } => "swing",
                    ClientCommand::CancelCast { .. } => "cancel-cast",
                    ClientCommand::CancelAutoRepeat => "cancel-repeat",
                    _ => "other",
                })
                .collect::<Vec<_>>()
        };

        // Not engaged: first select and a switch are selection-only; a same-guid re-commit dedups.
        assert!(go(&mut world, 0xA, false, Some(1), true).changed);
        assert!(go(&mut world, 0xB, false, Some(1), true).changed);
        assert!(!go(&mut world, 0xB, false, Some(1), true).changed);
        assert_eq!(drain(&rx), ["select", "select"]);

        // Engaged switch onto an attackable unit: the byte order stop → select → swing.
        let out = go(&mut world, 0xC, true, Some(1), true);
        assert!(out.changed && out.swung);
        assert_eq!(drain(&rx), ["stop", "select", "swing"]);

        // Engaged switch onto MYSELF (TargetUnit("player") mid-combat): stop, no re-point.
        let out = go(&mut world, 0x1, true, Some(1), true);
        assert!(out.changed && !out.swung);
        assert_eq!(drain(&rx), ["stop", "select"]);

        // Engaged switch onto an unattackable unit (vendor/corpse): stop, no swing at it.
        let out = go(&mut world, 0xD, true, Some(1), false);
        assert!(out.changed && !out.swung);
        assert_eq!(drain(&rx), ["stop", "select"]);

        // Engaged FIRST select (no old target): the [ebp-1] latch is off — selection only.
        *world.resource_mut::<Selection>() = Selection::default();
        let out = go(&mut world, 0xE, true, Some(1), true);
        assert!(out.changed && !out.swung);
        assert_eq!(drain(&rx), ["select"]);

        // **The two edges the seam brought.** A queued strike and a running auto-repeat, then an
        // engaged switch onto an attackable unit: the stop un-queues the strike, the re-swing
        // kills the repeat.
        world
            .resource_mut::<crate::spell::QueuedMeleeSpell>()
            .arm(78);
        world.resource_mut::<crate::spell::AutoRepeatActive>().0 = Some(75);
        let out = go(&mut world, 0xF, true, Some(1), true);
        assert!(out.changed && out.swung);
        assert_eq!(
            drain(&rx),
            ["stop", "cancel-cast", "select", "swing", "cancel-repeat"]
        );
        assert_eq!(
            world.resource::<crate::spell::QueuedMeleeSpell>().current(),
            None,
            "the switch un-queued Heroic Strike"
        );
        assert_eq!(
            world.resource::<crate::spell::AutoRepeatActive>().0,
            None,
            "and the re-swing killed Auto Shot"
        );
    }

    /// **`IsSelectable` refuses the whole commit** — `0x493540`'s third early-out
    /// (`0x4935ee`/`0x4935f3`), which sits *before* the dedup, so the refusal is a complete no-op:
    /// nothing on the wire, no `PLAYER_TARGET_CHANGED`, and the target already in hand survives.
    ///
    /// The reported case is Stratholme's Baron Rivendare before the ziggurats fall — vmangos gives
    /// him `UNIT_FLAG_NOT_SELECTABLE | UNIT_FLAG_SPAWNING` (`instance_stratholme.cpp`), so on the
    /// real client he can be neither clicked, tabbed, nor auto-acquired when he opens on you.
    #[test]
    fn not_selectable_refuses_the_commit_and_keeps_the_old_target() {
        use benilla_protocol::ObjectFields;
        const FLAGS: u16 = 46;
        const CREATEDBY: u16 = 14;
        const OBJECT_TYPE: u16 = 2;
        const NOT_SELECTABLE: u32 = 1 << 25;
        // `OBJECT_FIELD_TYPE` bit 3 = a CGUnit_C, the only kind whose slot `+0x58` is not the
        // `xor eax,eax` base stub.
        let unit = |pairs: &[(u16, u32)]| {
            let mut all = vec![(OBJECT_TYPE, 0x8u32)];
            all.extend_from_slice(pairs);
            ObjectStore(ObjectFields::from_pairs(&all))
        };

        let (mut world, rx) = commit_world();
        let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| rx.try_iter().count();

        // A plain live mob commits.
        let plain = unit(&[]);
        assert!(go_with(&mut world, 0xA, Some(plain), false, Some(1), true).changed);
        assert_eq!(sent(&rx), 1);

        // The Baron: flagged, created by nobody. Refused outright — and 0xA is still the target.
        let baron = unit(&[(FLAGS, NOT_SELECTABLE | 0x2)]);
        let out = go_with(&mut world, 0xB, Some(baron), true, Some(1), true);
        assert!(!out.changed && !out.swung, "the commit is a complete no-op");
        assert_eq!(sent(&rx), 0, "nothing goes out — not even the stop");
        assert_eq!(
            world.resource::<Selection>().guid,
            Some(0xA),
            "the target already in hand survives the refused click"
        );

        // The `CREATEDBY` clause: the same flag on a unit **I** created still selects (my totem).
        let mine = unit(&[(FLAGS, NOT_SELECTABLE), (CREATEDBY, 1), (CREATEDBY + 1, 0)]);
        assert!(go_with(&mut world, 0xC, Some(mine), false, Some(1), true).changed);
        assert_eq!(sent(&rx), 1);

        // Somebody else's flagged creation is not mine to click.
        let theirs = unit(&[(FLAGS, NOT_SELECTABLE), (CREATEDBY, 9), (CREATEDBY + 1, 0)]);
        assert!(!go_with(&mut world, 0xD, Some(theirs), false, Some(1), true).changed);
        assert_eq!(sent(&rx), 0);

        // A non-unit object answers the base stub — a GameObject guid can never be the selection.
        let go_obj = ObjectStore(ObjectFields::from_pairs(&[(OBJECT_TYPE, 0x20)]));
        assert!(!go_with(&mut world, 0xE, Some(go_obj), false, Some(1), true).changed);
        assert_eq!(sent(&rx), 0);

        // And an unresolved object skips the vcall entirely (the out-of-range party member).
        assert!(go_with(&mut world, 0xF, None, false, Some(1), true).changed);
        assert_eq!(sent(&rx), 1);
    }

    fn cand(guid: u64, score: f32, on_screen: bool) -> Candidate {
        Candidate {
            entity: Entity::PLACEHOLDER,
            guid,
            on_screen,
            score,
        }
    }

    /// The priority value: a centered far mob beats a screen-edge close one; a close central
    /// mob beats a far central one; the fighting-me bonus dominates both terms.
    #[test]
    fn priority_score_weighs_center_distance_and_combat() {
        // Dead-center at 30 yd vs near-the-edge at 3 yd: center wins.
        assert!(
            priority_score(Some(0.05), 30.0, false) < priority_score(Some(0.9), 3.0, false),
            "a centered far mob outranks a screen-edge close one"
        );
        // Same centrality: nearest wins.
        assert!(priority_score(Some(0.2), 5.0, false) < priority_score(Some(0.2), 35.0, false));
        // Fighting-me beats a perfectly centered peaceful mob at any position in range.
        assert!(
            priority_score(Some(0.9), 40.0, true) < priority_score(Some(0.0), 1.0, false),
            "the combat bonus dominates the geometric terms"
        );
    }

    /// The sort and the pool: on-screen candidates always precede off-screen ones; the pool is
    /// the on-screen tier when it exists (AllowAnyOnScreen=1), everything otherwise. NO combat
    /// lock (decision 0568): a fighting-me attacker in the pool never restricts it — TAB can
    /// always walk off an attacker onto a fresh target (the bonus orders, the history moves).
    #[test]
    fn tier_pool_never_locks() {
        let mut v = [cand(1, 0.1, false), cand(2, 5.0, true), cand(3, 0.5, true)];
        v.sort_by(candidate_order);
        assert_eq!(
            v.map(|c| c.guid),
            [3, 2, 1],
            "on-screen first (even at a worse score), then ascending score"
        );

        // Tier: with any on-screen candidate, off-screen ones drop out of the pool.
        let pool = select_pool(&v);
        assert_eq!(pool.iter().map(|c| c.guid).collect::<Vec<_>>(), [3, 2]);
        // No on-screen candidates at all → the fallback tier is everything.
        let off = [cand(1, 0.3, false), cand(2, 0.1, false)];
        assert_eq!(select_pool(&off).len(), 2);

        // An attacker (fighting-me) in the pool does NOT shrink it — the 0568 unlock: with the
        // attacker current and visited, the pick walks onto the peaceful mob.
        let mixed = [cand(1, -2.0, true), cand(2, 0.2, true)];
        let pool = select_pool(&mixed);
        assert_eq!(pool.len(), 2, "no combat lock — the full tier stays");
        assert_eq!(
            pick_forward(&pool, &[1], Some(1)),
            Some((1, false)),
            "TAB moves off the attacker onto the fresh target"
        );
    }

    /// The forward pick: best unvisited non-current first; exhausted history wraps (caller
    /// clears it) to the best non-current; a pool of only the current selection returns it
    /// (the commit dedups to a no-op).
    #[test]
    fn forward_pick_skips_history_then_wraps() {
        let pool = [
            cand(0xA, 0.1, true),
            cand(0xB, 0.2, true),
            cand(0xC, 0.3, true),
        ];
        // Fresh: best.
        assert_eq!(pick_forward(&pool, &[], None), Some((0, false)));
        // Current is best, nothing visited: next-best.
        assert_eq!(pick_forward(&pool, &[0xA], Some(0xA)), Some((1, false)));
        // Two visited: the third.
        assert_eq!(
            pick_forward(&pool, &[0xA, 0xB], Some(0xB)),
            Some((2, false))
        );
        // All visited: wrap to the best non-current.
        assert_eq!(
            pick_forward(&pool, &[0xA, 0xB, 0xC], Some(0xC)),
            Some((0, true))
        );
        // Only the current selection in the pool: itself, wrapped (dedup no-op downstream).
        let solo = [cand(0xA, 0.1, true)];
        assert_eq!(pick_forward(&solo, &[0xA], Some(0xA)), Some((0, true)));
        assert_eq!(pick_forward(&[], &[], None), None);
    }

    /// **Phase B's fork** (`0x6130a3`): a hostile selection is kept, everything else acquires.
    ///
    /// The case that reads wrong until you have read the binary is the *friendly* one — it is not
    /// an error, it is a reason to go find a real target. That is what makes "press Attack with no
    /// target and the pet goes for what is in front of you" the same code path as "press Attack
    /// while talking to a vendor and your target jumps to the wolf behind him".
    #[test]
    fn phase_b_keeps_only_a_hostile_selection() {
        let hostile = |g: u64| g == 0xBAD;
        assert_eq!(keeps_held_target(Some(0xBAD), hostile), Some(0xBAD));
        assert_eq!(keeps_held_target(Some(0x600D), hostile), None, "friendly");
        assert_eq!(keeps_held_target(None, hostile), None, "no selection");
    }

    /// The final gate's target legs (`0x613152`–`0x613169`), including the transcribed oddity: a
    /// zero-health target is invalid **unless** dynamic-flag bit 5 is set.
    #[test]
    fn the_attack_orders_final_gate_reads_health_then_can_attack() {
        use benilla_protocol::ObjectFields;
        const HEALTH: u16 = 22;
        const DYNFLAGS: u16 = 143;
        const FLAGS: u16 = 46;
        const TPL: u16 = 35;
        let reps = Reputations::default();
        let unit = |pairs: &[(u16, u32)]| ObjectStore(ObjectFields::from_pairs(pairs));
        // `CanAttack` needs both sides now (1674), and with no catalog both reactions resolve to
        // neutral — the mixed arm's `< 4`, so a plain live mob is attackable and the legs below
        // are the ones actually under test.
        let me = unit(&[(TPL, 1), (FLAGS, 1 << 3)]);
        let valid = |s: &ObjectStore| attack_target_valid(Some(s), None, &reps, Some(&me));

        assert!(valid(&unit(&[(HEALTH, 120)])), "a live mob");
        assert!(!valid(&unit(&[(HEALTH, 0)])), "a corpse");
        assert!(
            valid(&unit(&[(HEALTH, 0), (DYNFLAGS, 1 << 5)])),
            "0x613159's second leg, verbatim"
        );
        assert!(
            !valid(&unit(&[(HEALTH, 120), (FLAGS, 1 << 25)])),
            "NOT_SELECTABLE is one of CanAttack's disqualifiers"
        );
        // No descriptor on either side is a REFUSAL now — see the fn doc. The reference holds two
        // live CGUnits here and has no null path; swinging at an object we know nothing about was
        // the approximation's posture, not the binary's.
        assert!(!attack_target_valid(None, None, &reps, Some(&me)));
        assert!(!attack_target_valid(
            Some(&unit(&[(HEALTH, 120)])),
            None,
            &reps,
            None
        ));
    }

    /// The **backward** pick, beside its forward twin: newest-first through the history, skipping
    /// the current selection and anything that has left the pool; nothing to step back to ⇒ `None`,
    /// which is what makes the first reverse press fall through to the forward rule.
    ///
    /// This is the whole of `reverse` — the flag `TargetNearestEnemy`/`TargetNearestFriend` take as
    /// their optional Lua argument (`0x489a80`/`0x489aa0` fetch it with `0x6f1c10`, default 0) and
    /// hand to the one cycler as `ecx`. Forward and backward walk the same order in opposite
    /// directions: with A,B,C visited in that order, back from C is B and back from B is A.
    #[test]
    fn pick_back_walks_the_history_newest_first() {
        let pool = [
            cand(0xA, 0.1, true),
            cand(0xB, 0.2, true),
            cand(0xC, 0.3, true),
        ];
        // Forward over a fresh pool visits A, then B, then C — the order `pick_back` reverses.
        assert_eq!(pick_forward(&pool, &[], None), Some((0, false)));
        assert_eq!(pick_forward(&pool, &[0xA], Some(0xA)), Some((1, false)));
        assert_eq!(
            pick_forward(&pool, &[0xA, 0xB], Some(0xB)),
            Some((2, false))
        );
        // Back from C is B; back from B is A — the exact reverse of the walk above.
        assert_eq!(pick_back(&[0xA, 0xB, 0xC], &pool, Some(0xC)), Some(1));
        assert_eq!(pick_back(&[0xA, 0xB], &pool, Some(0xB)), Some(0));
        // Nothing behind us: the caller falls through to the forward rule.
        assert_eq!(pick_back(&[], &pool, None), None);
        assert_eq!(pick_back(&[0xA], &pool, Some(0xA)), None);
        // A remembered guid that has left the pool (died, streamed out) is skipped, not picked.
        let shrunk = [cand(0xA, 0.1, true), cand(0xC, 0.3, true)];
        assert_eq!(pick_back(&[0xA, 0xB, 0xC], &shrunk, Some(0xC)), Some(0));
    }

    /// The history is **per side**: a friendly press wipes the enemy cycle's skip set and vice
    /// versa — the reference's `cachedMode != mode` rebuild condition (`0x493f60`), which drops the
    /// candidate list and resets the cursor whenever the mode byte changes.
    #[test]
    fn a_side_switch_clears_the_history() {
        let mut h = TabHistory::default();
        h.enter(ScanSide::Enemy);
        h.push(0xA, 0.0);
        h.push(0xB, 1.0);
        h.enter(ScanSide::Enemy); // same side: nothing happens
        assert_eq!(h.guids(), [0xA, 0xB]);
        h.enter(ScanSide::Friend); // the other side: start from nothing
        assert!(h.guids().is_empty());
    }

    /// Field indices, for the store builders below (`benilla-protocol`'s own numbering).
    const F_TYPE: u16 = 2;
    const F_HEALTH: u16 = 22;
    const F_MAXHEALTH: u16 = 28;
    const F_FLAGS: u16 = 46;
    const F_DYNFLAGS: u16 = 143;
    const F_MOUNTDISPLAYID: u16 = 133;
    const F_DUEL_ARBITER: u16 = 188;
    const F_PLAYER_FLAGS: u16 = 190;
    const F_DUEL_TEAM: u16 = 196;
    /// `UNIT_FLAG_PVP_ATTACKABLE` — behaviourally "player-controlled"; the wire always carries it
    /// on a player, which is what selects `CanAttack`'s and `CanAssist`'s player arms.
    const CONTROLLED: u32 = 0x8;
    /// `OBJECT_FIELD_TYPE` for a Player object (OBJECT|UNIT|PLAYER).
    const TYPE_PLAYER: u32 = 0x19;

    fn store(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs))
    }

    /// **The two sides of one scan, over one world** — the mode byte is the only fork, so this
    /// drives the real [`TargetScan::build`] twice and asserts each side's pool.
    ///
    /// With no `FactionTemplate.dbc` loaded every reaction resolves to neutral, which is exactly
    /// the regime that separates the two filters: neutral is **attackable** (`CanAttack`'s mixed
    /// arm is `< 4`) and **not assistable** (`CanAssist`'s ladder is `>= 4`). To get a genuinely
    /// friendly reaction with no catalog the ally here is a **same-team duel partner** — the one
    /// rung of `UnitReaction 0x6061e0` that answers 4 off descriptor fields alone (decision 0633).
    /// It is also a fair model of the real case: `TargetNearestFriend` is mostly about players.
    ///
    /// Four candidates pin four separate claims:
    /// * the neutral mob is an enemy candidate and never a friendly one — **the control that must
    ///   not change**;
    /// * the friendly player is a friendly candidate and never an enemy one;
    /// * a friendly **corpse** is out — mode 2's `UNIT_FIELD_HEALTH > 0` leg;
    /// * a friendly **feigner** is IN — mode 2 has no `UNIT_DYNFLAG_DEAD` leg, though mode 1's
    ///   reads-dead triple does. That asymmetry is transcribed, not tidied.
    #[test]
    fn the_two_sides_of_the_scan_never_pick_each_others_units() {
        use bevy::ecs::system::RunSystemOnce;

        const ARBITER: u32 = 7;
        const MOB: u64 = 0xF0E;
        const ALLY: u64 = 0xA11;
        const ALLY_CORPSE: u64 = 0xDEAD;
        const ALLY_FEIGN: u64 = 0xFE16;

        let mut world = World::new();
        world.init_resource::<Reputations>();
        world.init_resource::<NameCache>();
        // Us: a live player, mid-duel on team 1.
        world.spawn((
            SelfPlayer,
            Transform::default(),
            Guid(1),
            store(&[
                (F_TYPE, TYPE_PLAYER),
                (F_FLAGS, CONTROLLED),
                (F_HEALTH, 100),
                (F_MAXHEALTH, 100),
                (F_DUEL_ARBITER, ARBITER),
                (F_DUEL_TEAM, 1),
            ]),
        ));
        let unit = |guid: u64, x: f32, fields: &[(u16, u32)]| {
            (
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Guid(guid),
                Transform::from_xyz(x, 0.0, 0.0),
                store(fields),
            )
        };
        let ally_fields = |extra: &[(u16, u32)]| {
            let mut v = vec![
                (F_TYPE, TYPE_PLAYER),
                (F_FLAGS, CONTROLLED),
                (F_HEALTH, 100),
                (F_MAXHEALTH, 100),
                (F_DUEL_ARBITER, ARBITER),
                (F_DUEL_TEAM, 1),
            ];
            v.extend_from_slice(extra);
            v
        };
        world.spawn(unit(MOB, 5.0, &[(F_HEALTH, 100), (F_MAXHEALTH, 100)]));
        world.spawn(unit(ALLY, 10.0, &ally_fields(&[])));
        world.spawn(unit(ALLY_CORPSE, 15.0, &ally_fields(&[(F_HEALTH, 0)])));
        world.spawn(unit(
            ALLY_FEIGN,
            20.0,
            &ally_fields(&[(F_DYNFLAGS, 1 << 5)]),
        ));

        let pool = |world: &mut World, side: ScanSide| {
            world
                .run_system_once(move |scan: TargetScan| {
                    scan.build(side).iter().map(|c| c.guid).collect::<Vec<_>>()
                })
                .expect("the scan runs as a one-shot system")
        };

        // Mode 1: the neutral mob only. Every duel-team ally is refused by `CanAttack`'s
        // both-player-controlled arm (a friendly reaction returns 0 outright).
        assert_eq!(pool(&mut world, ScanSide::Enemy), [MOB]);
        // Mode 2: the live ally and the feigning one — never the mob, never the corpse. With no
        // camera in the world nothing projects, so the order is pure distance (ALLY at 10 yd
        // before ALLY_FEIGN at 20).
        assert_eq!(pool(&mut world, ScanSide::Friend), [ALLY, ALLY_FEIGN]);
    }

    /// **A unit the draw election culled is still a TAB candidate.** The outdoor election
    /// (decisions 1270/1475) writes `Visibility::Hidden` on every body root outside the camera
    /// frustum, every frame — so a scan gate on `Hidden` emptied the off-screen tier 0567 keeps
    /// as the fallback, and TAB / attack-with-no-target / pet Attack could never reach a mob
    /// hitting you from behind while nothing was on screen.
    #[test]
    fn a_body_the_draw_election_culled_is_still_a_candidate() {
        use bevy::ecs::system::RunSystemOnce;

        const MOB: u64 = 0xBEEF;
        let mut world = World::new();
        world.init_resource::<Reputations>();
        world.init_resource::<NameCache>();
        world.spawn((
            SelfPlayer,
            Transform::default(),
            Guid(1),
            store(&[
                (F_TYPE, TYPE_PLAYER),
                (F_FLAGS, CONTROLLED),
                (F_HEALTH, 100),
                (F_MAXHEALTH, 100),
            ]),
        ));
        world.spawn((
            NetEntity {
                kind: EntityKind::Unit,
                display_id: None,
                scale: 1.0,
            },
            Guid(MOB),
            Transform::from_xyz(0.0, 0.0, 5.0),
            store(&[(F_HEALTH, 100), (F_MAXHEALTH, 100)]),
            // The election's out-of-view verdict (`exterior_cull`'s body leg).
            Visibility::Hidden,
        ));
        let pool = world
            .run_system_once(|scan: TargetScan| {
                scan.build(ScanSide::Enemy)
                    .iter()
                    .map(|c| c.guid)
                    .collect::<Vec<_>>()
            })
            .expect("the scan runs as a one-shot system");
        assert_eq!(pool, [MOB]);
    }

    /// [`LastEnemy`] tracks the last **attackable** selection and nothing else — `TargetLastEnemy`'s
    /// whole memory (`[0xb4e2e8]/[0xb4e2ec]`, `0x49377d`).
    ///
    /// The three claims that make the verb behave: it follows a hostile switch; a **friendly**
    /// selection does not overwrite it (that is what separates it from `TargetLastTarget`'s pair);
    /// and **clearing the target leaves it standing**, which is the whole point — you press `G`
    /// after an Esc, not before one.
    #[test]
    fn last_enemy_remembers_the_last_attackable_selection_across_a_clear() {
        use bevy::ecs::system::RunSystemOnce;

        const ARBITER: u32 = 7;
        let mut world = World::new();
        world.init_resource::<Reputations>();
        world.init_resource::<Selection>();
        world.init_resource::<LastEnemy>();
        world.spawn((
            SelfPlayer,
            store(&[
                (F_TYPE, TYPE_PLAYER),
                (F_FLAGS, CONTROLLED),
                (F_HEALTH, 100),
                (F_MAXHEALTH, 100),
                (F_DUEL_ARBITER, ARBITER),
                (F_DUEL_TEAM, 1),
            ]),
        ));
        let mob = |world: &mut World| {
            world
                .spawn(store(&[(F_HEALTH, 100), (F_MAXHEALTH, 100)]))
                .id()
        };
        let a = mob(&mut world);
        let b = mob(&mut world);
        // A same-team duel partner: friendly reaction, so `CanAttack`'s PvP arm refuses.
        let friend = world
            .spawn(store(&[
                (F_TYPE, TYPE_PLAYER),
                (F_FLAGS, CONTROLLED),
                (F_HEALTH, 100),
                (F_MAXHEALTH, 100),
                (F_DUEL_ARBITER, ARBITER),
                (F_DUEL_TEAM, 1),
            ]))
            .id();

        let select = |world: &mut World, target: Option<(Entity, u64)>| {
            let mut sel = world.resource_mut::<Selection>();
            sel.target = target.map(|(e, _)| e);
            sel.guid = target.map(|(_, g)| g);
            world
                .run_system_once(remember_last_enemy)
                .expect("the sampler runs as a one-shot system");
            world.resource::<LastEnemy>().0
        };

        assert_eq!(
            world.resource::<LastEnemy>().0,
            None,
            "nothing hostile targeted yet"
        );
        assert_eq!(select(&mut world, Some((a, 0xA))), Some(0xA));
        assert_eq!(select(&mut world, Some((b, 0xB))), Some(0xB));
        // A friendly selection is not an enemy — the memory holds at B.
        assert_eq!(select(&mut world, Some((friend, 0xF))), Some(0xB));
        // …and neither is an empty one. This is the case the verb exists for.
        assert_eq!(select(&mut world, None), Some(0xB));

        // **The four conjuncts besides `CanAttack`** (`0x49372f`-`0x493778`). Each is asserted by
        // moving one state and re-selecting an ordinary hostile that would otherwise stamp: the
        // memory has to hold at B every time. Written as one test rather than four because the
        // whole point is that they are one gate, and a gate with a leg missing is what shipped.
        let dead_mob = world
            .spawn(store(&[(F_HEALTH, 0), (F_MAXHEALTH, 100)]))
            .id();
        assert_eq!(
            select(&mut world, Some((dead_mob, 0xD))),
            Some(0xB),
            "a corpse is not remembered — the target's health leg"
        );
        // …unless it carries the dead-looking dynflag, which is a FEIGN and stays remembered.
        let feigner = world
            .spawn(store(&[
                (F_HEALTH, 0),
                (F_MAXHEALTH, 100),
                (F_DYNFLAGS, 0x20),
            ]))
            .id();
        assert_eq!(
            select(&mut world, Some((feigner, 0xE))),
            Some(0xE),
            "`HEALTH > 0 || dynflag 0x20` is an OR, transcribed not simplified"
        );

        // Our own body's three states. Each is set, a live hostile selected, and the memory must
        // not move off 0xE.
        let with_self = |world: &mut World, fields: &[(u16, u32)]| {
            let me = world
                .query_filtered::<Entity, With<SelfPlayer>>()
                .single(world)
                .expect("one self player");
            let mut base = vec![
                (F_TYPE, TYPE_PLAYER),
                (F_FLAGS, CONTROLLED),
                (F_HEALTH, 100),
                (F_MAXHEALTH, 100),
                (F_DUEL_ARBITER, ARBITER),
                (F_DUEL_TEAM, 1),
            ];
            base.extend_from_slice(fields);
            world.entity_mut(me).insert(store(&base));
        };
        let c = mob(&mut world);
        with_self(&mut world, &[(F_MOUNTDISPLAYID, 1234)]);
        assert_eq!(
            select(&mut world, Some((c, 0xC))),
            Some(0xE),
            "mounted: the reference does not stamp"
        );
        with_self(&mut world, &[(F_HEALTH, 0)]);
        assert_eq!(
            select(&mut world, Some((c, 0xC))),
            Some(0xE),
            "dead: the reference does not stamp"
        );
        with_self(&mut world, &[(F_PLAYER_FLAGS, 0x10)]);
        assert_eq!(
            select(&mut world, Some((c, 0xC))),
            Some(0xE),
            "ghost: a ghost's wire health is 1, so this needs its own leg"
        );
        // The control: put the body back and the same selection stamps.
        with_self(&mut world, &[]);
        assert_eq!(
            select(&mut world, Some((c, 0xC))),
            Some(0xC),
            "the gate is the three states, not the selection"
        );
    }

    /// The history: pruning honors the window, a re-visit moves to most-recent.
    #[test]
    fn history_prunes_and_reorders() {
        let mut h = TabHistory::default();
        h.push(0xA, 0.0);
        h.push(0xB, 1.0);
        h.push(0xA, 2.0); // re-visit: A moves to the back
        assert_eq!(h.guids(), [0xB, 0xA]);
        h.prune(1.0 + HISTORY_SECS); // B (t=1.0) ages out at exactly the window
        assert_eq!(h.guids(), [0xA]);
    }
}
