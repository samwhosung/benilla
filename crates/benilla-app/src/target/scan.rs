//! TAB and nearest-unit targeting, and the attack auto-acquires.
//!
//! Deviation: TAB uses Classic Era's screen-space priority, not 1.12's ±30° facing cone and
//! snapshot cursor (`0x493f60`), because the cone skips a close mob in front of the camera, which
//! sits behind the character. On-screen candidates come first, the lowest score wins, and each
//! press skips recently tabbed guids; a fighting-me bonus, not Classic's hard combat lock, ranks
//! attackers first without pinning TAB to them.
//!
//! Kept from 1.12: the validity filters (`0x493e40`, [`can_attack`] `0x606980`, critters), the
//! [`commit`] through `SetSelection` (`0x493540`), the attack acquire (`0x612df0` at `0x6130b5`),
//! the ATTACKERSTATEUPDATE acquire (`0x6259c9`–`0x6259fe`) and the 41 yd range.

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
// Each names the Classic Era cvar it stands in for; the values are tuned.

/// `targetNearestDistance`, the TAB range in yards: 1.12's default (`[0x804510]`).
const TAB_RANGE: f32 = 41.0;
/// `TargetPriorityFrustumPullInSides`: the viewport width fraction pulled in from each side.
const FRUSTUM_PULL_SIDES: f32 = 0.10;
/// `TargetPriorityFrustumPullInTop`: the viewport height fraction pulled down from the top.
const FRUSTUM_PULL_TOP: f32 = 0.10;
/// `TargetPriorityFrustumPullInBot`: the viewport height fraction pulled up from the bottom.
const FRUSTUM_PULL_BOT: f32 = 0.10;
/// `TargetPriorityHighlightHistoryMs`: how long a tabbed guid is skipped by repeated presses.
const HISTORY_SECS: f64 = 4.0;
/// The weight per unit of off-centerness (0 at the centre, about 1 at a corner).
const W_SCREEN: f32 = 1.0;
/// The weight per unit of `dist / TAB_RANGE`.
const W_DIST: f32 = 1.0;
/// Above the other terms' largest sum, so an attacker outranks anything peaceful.
const COMBAT_WITH_ME_BONUS: f32 = 3.0;

/// `WOW_TAB_TRACE=1`: log each press's verdicts, pool and pick under `tab-trace:`.
fn tab_trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_TAB_TRACE").is_some())
}

/// The reference's scan mode: the `TargetNearest*` shims (`0x489a80` enemy 1, `0x489aa0` friend 2,
/// `0x489ac0`/`0x489ae0` party and raid 3/4) all call one cycler, `0x493f60(reverse, mode)`, and
/// the mode reaches only the per-candidate filter `0x493e40`. Modes 3 and 4 are not built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScanSide {
    /// Mode 1 (`0x493e73`): alive by the reads-dead triple `0x605f90`, and `CanAttack 0x606980`.
    Enemy,
    /// Mode 2 (`0x493eca`): `CanAssist 0x6066f0`, friendly or better (`0x60671e`), whose
    /// `IsPvP 0x605ff0` leg refuses a creature without `UNIT_FLAG_PVP`; then health > 0 with no
    /// dynflag leg, so a feigning ally counts.
    Friend,
}

/// `score`, lower is better, includes the fighting-me bonus; `on_screen` is the tier-1 gate.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    entity: Entity,
    guid: u64,
    on_screen: bool,
    score: f32,
}

/// Off-centerness (none off-screen) plus normalized distance, minus the fighting-me bonus.
fn priority_score(off_center: Option<f32>, dist: f32, combat_with_me: bool) -> f32 {
    off_center.map_or(0.0, |c| W_SCREEN * c) + W_DIST * (dist / TAB_RANGE)
        - if combat_with_me {
            COMBAT_WITH_ME_BONUS
        } else {
            0.0
        }
}

/// On-screen before off-screen, then ascending score.
fn candidate_order(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    b.on_screen
        .cmp(&a.on_screen)
        .then(a.score.total_cmp(&b.score))
}

/// The on-screen tier when it is not empty, else every candidate (`AllowAnyOnScreen` = 1).
fn select_pool(cands: &[Candidate]) -> Vec<Candidate> {
    if cands.iter().any(|c| c.on_screen) {
        cands.iter().filter(|c| c.on_screen).copied().collect()
    } else {
        cands.to_vec()
    }
}

/// The best candidate neither current nor visited; else the best non-current, wrapped (the caller
/// clears the history); else the current one itself.
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

/// The reverse pick: the newest history guid neither current nor gone from the pool, `None` falling
/// through to [`pick_forward`]. The reference steps its snapshot cursor back one slot instead.
fn pick_back(visited: &[u64], pool: &[Candidate], current: Option<u64>) -> Option<usize> {
    visited.iter().rev().find_map(|g| {
        (Some(*g) != current)
            .then(|| pool.iter().position(|c| c.guid == *g))
            .flatten()
    })
}

/// The unit targets me and has `UNIT_FLAG_IN_COMBAT` (bit 19) set.
fn combat_with_me(store: &ObjectStore, me: Option<u64>) -> bool {
    me.is_some() && store.0.unit_target() == me && store.0.unit_flags() & (1 << 19) != 0
}

/// `CreatureType.dbc`'s flags, whose no-TAB bit is set on type 8, Critter, alone (Totem reads 0).
/// Absent after a load failure, when nothing is filtered.
#[derive(Resource)]
pub(crate) struct CreatureTypes(CreatureTypeFlags);

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

/// Everything the scan reads, shared by TAB, the attack acquire and the pet bar's Attack.
///
/// Deviation: our own body is never a candidate. The reference's friendly scan picks the player at
/// some facings (`CanAssist(P, P)` is true, `0x6061e0`; the cone test `0x47f220`), but our scoring
/// always puts the player dead centre at distance 0, so CTRL-TAB would pick us on every press.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled system param, the app's convention
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
    /// The in-view check's camera; without one every candidate is fallback tier.
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
    /// Every store, ours included: `CanAssist`'s owner chase must reach our body.
    stores: Query<'w, 's, &'static ObjectStore>,
    /// The owner chase's guid → entity map; `Option` because a UI-only harness has no net stack.
    index: Option<Res<'w, GuidIndex>>,
}

impl TargetScan<'_, '_> {
    /// The per-candidate filter (`0x493e40`), the one place [`ScanSide`] forks.
    fn is_valid(
        &self,
        side: ScanSide,
        store: Option<&ObjectStore>,
        self_store: Option<&ObjectStore>,
    ) -> bool {
        match side {
            // Mode 1 (`0x493e73`): the reads-dead triple `0x605f90`, then `CanAttack`.
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
            // Mode 2 (`0x493eca`): `CanAssist 0x6066f0`, then raw health (no reads-dead triple).
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

    /// A guid's descriptor, whichever entity holds it: `CanAssist`'s owner chase.
    fn store_of(&self, guid: u64) -> Option<&ObjectStore> {
        let entity = *self.index.as_ref()?.0.get(&guid)?;
        self.stores.get(entity).ok()
    }

    fn self_store(&self) -> Option<&ObjectStore> {
        self.self_q.single().ok().and_then(|(_, store, _)| store)
    }

    fn self_guid(&self) -> Option<u64> {
        self.self_q
            .single()
            .ok()
            .and_then(|(_, _, g)| g)
            .map(|g| g.0)
    }

    fn store_at(&self, entity: Entity) -> Option<&ObjectStore> {
        self.stores.get(entity).ok()
    }

    /// `0x6130a3`'s keep test: the actor's reaction to the held guid alone (`0x61309b`), not the
    /// final gate's `CanAttack`; an unstreamed guid is not hostile (`0x613099`). Actor → target is
    /// [`reaction_from_player`] (the at-war bit), never `ring_reaction`. The reference's actor is
    /// the caller, a pet on the pet arm (`0x4bd40d`), which vmangos gives its owner's faction
    /// (`Pet.cpp:248`).
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

    fn unit_by_guid(&self, guid: u64) -> Option<(Entity, Option<&ObjectStore>)> {
        self.units
            .iter()
            .find(|(_, _, g, _, _)| g.0 == guid)
            .map(|(e, _, _, _, store)| (e, store))
    }

    /// Filter, project, score and sort every known unit, fresh each press.
    fn build(&self, side: ScanSide) -> Vec<Candidate> {
        let Ok((self_tf, self_store, self_guid)) = self.self_q.single() else {
            return Vec::new();
        };
        let me = self_guid.map(|g| g.0);
        let cam = self.camera.single().ok();
        // Off-centerness on the pulled-in screen, 0 at the centre and about 1 at a corner, `None`
        // outside it. Callers project the root + 1 yd, so feet just below the edge still count.
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
            // The critter gate; an unresolved type passes, like the client's out-of-range index.
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
            // Deviation: no scene-attach gate (the reference's `0x6704c0`), because the draw
            // election hides every body outside the frustum, which would empty the off-screen tier.
            let dist = (tf.translation - self_tf.translation).length();
            if dist > TAB_RANGE {
                trace_unit(guid_c.0, tf, "REJECT range (41 yd)");
                continue;
            }
            let off_center = project(tf.translation + Vec3::Y);
            // Enemy side only: a healer in combat and targeting you would head the CTRL-TAB pool.
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

/// Recently picked guids, skipped by the forward pick: the stand-in for 1.12's snapshot cursor.
#[derive(Resource, Default)]
pub(super) struct TabHistory {
    /// `(guid, when)`, oldest first.
    visited: Vec<(u64, f64)>,
    /// A side switch clears the history, as `0x493f60` rebuilds its list on a mode change.
    side: Option<ScanSide>,
}

impl TabHistory {
    fn prune(&mut self, now: f64) {
        self.visited.retain(|&(_, t)| now - t < HISTORY_SECS);
    }
    fn enter(&mut self, side: ScanSide) {
        if self.side != Some(side) {
            self.visited.clear();
            self.side = Some(side);
        }
    }
    fn guids(&self) -> Vec<u64> {
        self.visited.iter().map(|&(g, _)| g).collect()
    }
    /// Record a visit; a re-visit moves the guid to most recent.
    fn push(&mut self, guid: u64, now: f64) {
        self.visited.retain(|&(g, _)| g != guid);
        self.visited.push((guid, now));
    }
}

/// What one [`commit`] did; `swung` means it already sent `CMSG_ATTACKSWING`, so a caller must not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CommitOutcome {
    pub(super) changed: bool,
    pub(super) swung: bool,
}

/// Commit a target through `SetSelection` (`0x493540`). A switch while attacking is stop, select,
/// re-swing: StopAttack `0x5ecac0` (`0x493a08`) flags a stop in flight (`[player+0xc54]`),
/// `CMSG_SET_SELECTION` goes out (`0x493857`), then unless the new target is us `Attack 0x5ecb70`
/// swings past its already-attacking gate, or stops if `new_attackable` is false (`0x4938a1`).
///
/// The latch (`0x493637`) reduces to `engaged && had_old`: vmangos ends [`Engaged`] when the
/// victim dies, before a TAB can land, so the old target's own legs change nothing seen.
/// `engaged` is the server's echo, a round trip behind the reference's local lock, and the
/// reference's second `CMSG_ATTACKSTOP` for an invalid new target is not sent here.
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
    // `IsSelectable`, before the dedup (`0x4935ec`–`0x4935f3`): a non-selectable unit is a complete
    // no-op that keeps the current target, for every selection writer.
    if !super::relations::is_selectable(target_store, self_guid) {
        return CommitOutcome::default();
    }
    if selection.guid == Some(guid) {
        return CommitOutcome::default(); // the setter's dedup
    }
    let had_old = selection.guid.is_some();
    if let Some(old) = selection.guid {
        // The old target's teardown (`0x4936cc` → `0x493910`) closes its loot first.
        seam.close_loot_on(old);
    }
    selection.target = Some(entity);
    selection.guid = Some(guid);
    let stop_and_repoint = engaged && had_old;
    if stop_and_repoint {
        // `0x493a08`: the real StopAttack, which also un-queues a next-swing strike (`0x6e6f30`).
        seam.stop(engaged);
    }
    let _ = seam.net.0.send(ClientCommand::SetSelection { guid });
    let swung = stop_and_repoint && new_attackable && self_guid != Some(guid);
    if swung {
        // `0x4938c8`: the swing goes out despite the lock, and cancels auto-repeat (`0x6ea080`).
        seam.start(guid, engaged, true);
    }
    CommitOutcome {
        changed: true,
        swung,
    }
}

/// One press on either side, as every `TargetNearest*` shim shares `0x493f60`: score the live
/// world, pool by tier, walk the history forward or back, and [`commit`].
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
    // Reverse walks the history back; an empty walk falls through to the forward pick.
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
    // The outgoing selection joins the history (`TargetPriorityContinueFromManualTarget`).
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
        // `IsSelectable`'s `CREATEDBY` clause reads it.
        scan.self_guid(),
        // `Attack 0x5ecb70`'s validation, which a mode-2 pick fails (reaction >= 4 against
        // `CanAttack`'s < 4). A duelling friendly passes `CanAttack`'s duel arm, which reads no
        // reaction, so the reference's `Attack` would re-swing at it; here it gets none.
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

/// The `TARGETNEARESTENEMY` and `TARGETPREVIOUSENEMY` bindings (TAB, SHIFT-TAB); the dispatch has
/// already applied the typing gate and the modifier match.
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

/// Drain `TargetNearestFriend([reverse])` (`0x489aa0` → `0x493f60(reverse, 2)`), one cycle per
/// call in call order; the stock bindings reach it through their `Bindings.xml` bodies.
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

/// The attack acquire (`0x6130b5`) is `TargetNearestEnemy()` itself, `0x493f60(0, 1)`, so it moves
/// the player's target. Deviation: a repeated acquire returns the same head, where the reference's
/// cursor walks on, because this module re-scores the live world on every call.
fn acquire_nearest_enemy(
    scan: &TargetScan,
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    errors: &mut crate::ui_action::UiErrorKeys,
) -> Option<(Entity, u64)> {
    let cands = scan.build(ScanSide::Enemy);
    let Some(c) = cands.first() else {
        // `0x6130d9`: still nothing after the acquire, error `0xa0`.
        debug!("attack acquire: nothing to attack (ERR_NO_ATTACK_TARGET)");
        errors
            .0
            .push(crate::ui_action::UiError::key("ERR_NO_ATTACK_TARGET"));
        return None;
    };
    // Not engaged: every path here held no selection or a non-hostile one.
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

/// `0x612df0`'s target arm: the selection (`0x61306b`) if the actor is hostile to it (`0x6130a3`),
/// else an acquire (`0x6130b5`), then the final gate (`0x613167`). The result is the guid
/// `CMSG_PET_ACTION` carries (`0x4bd491`); a dead selection is `ERR_INVALID_ATTACK_TARGET`, not a
/// reason to acquire.
pub(crate) fn attack_order_target(
    scan: &TargetScan,
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    errors: &mut crate::ui_action::UiErrorKeys,
) -> Option<u64> {
    let guid = match keeps_held_target(selection.guid, |g| scan.reaction_hostile(g)) {
        Some(kept) => kept,
        None => acquire_nearest_enemy(scan, selection, seam, errors)?.1,
    };
    // `0x613167`'s target legs; its actor legs (`0x61312e`) ran in the caller.
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

/// `0x6130a3`: keep a selection the actor is hostile to. A friendly or neutral one is dropped
/// (`0x6130a8`) and the acquire runs, so pet-Attack with a quest giver selected retargets a mob.
fn keeps_held_target(selection: Option<u64>, hostile: impl Fn(u64) -> bool) -> Option<u64> {
    selection.filter(|&g| hostile(g))
}

/// The final gate's target legs (`0x613152`–`0x613169`): alive, where a zero-health target passes
/// iff `UNIT_DYNAMIC_FLAGS` bit 5 is set (`0x613159`), and the full `CanAttack 0x606980`. A target
/// with no descriptor fails [`can_attack`]; the reference always holds a live unit here.
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

/// The attack action fired with no selection: acquire and swing (`0x612df0` at `0x6130b5`).
#[derive(Message)]
pub(crate) struct AttackNearestRequest;

/// Acquire the best candidate and start attacking it; the TAB history is untouched.
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
        return; // selected since the action: the normal path owns it
    }
    // `0x612df0`'s actor checks precede the acquire: a mounted, stunned or dead press never scans.
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
    let Some((_, guid)) =
        acquire_nearest_enemy(&scan, &mut selection, &mut seam, &mut ui_error_keys)
    else {
        return;
    };
    debug!("attack acquire: best candidate {guid:#x} → select + swing");
    // `0x6131a0`: StartAttack through the seam, with no stop in flight.
    seam.start(guid, false, false);
}

/// `TargetLastEnemy`'s memory, the reference's last-attackable guid `[0xb4e2e8]`, read at
/// `0x489b45` and written at `0x49377d` in `SetSelection`. Never cleared: an unresolved guid is a
/// bare `ret` in the select helper `0x489a40`, not a deselect.
#[derive(Resource, Default)]
pub(crate) struct LastEnemy(pub(crate) Option<u64>);

/// Stamp [`LastEnemy`] behind the five-conjunct gate `0x49372f`–`0x493778`. Runs before
/// `ring::update_ring`'s death-clear: a hostile that dies while selected stays remembered.
///
/// Deviation: sampled each frame, not stamped in `SetSelection`, so every selection writer is
/// covered; a selected unit that turns hostile is remembered, where the reference's is not.
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
    // Conjunct 1: the player object resolves.
    let Some(me) = self_store.iter().next() else {
        return;
    };
    // Conjuncts 2 and 3: not dead or a ghost (a ghost's wire health is 1), and not mounted.
    if me.0.unit_is_dead() || me.0.player_is_ghost() || me.0.unit_mount_display_id() != 0 {
        return;
    }
    let store = stores.get(entity).ok();
    // Conjunct 4: health, or the dead-looking flag, so a feigning target stays remembered.
    let alive_enough = store.is_some_and(|s| !s.0.unit_is_dead() || s.0.unit_dynflag_dead());
    if !alive_enough {
        return;
    }
    // Conjunct 5: `CanAttack(player, new)`.
    if can_attack(store, factions.as_deref(), &reputations, Some(me)) {
        last.0 = Some(guid);
    }
}

/// The ATTACKERSTATEUPDATE self-defence acquire (`0x6259c9`–`0x6259fe`): when we are the victim
/// and hold no selection, select the attacker, with no counter-attack.
pub(super) fn auto_acquire_attacker(
    mut swings: MessageReader<SwingMessage>,
    self_player: Query<(Entity, &Guid), With<SelfPlayer>>,
    guids: Query<&Guid>,
    // For `IsSelectable`: a `NOT_SELECTABLE` attacker is not acquired.
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
        // The selection was empty, so no switch law: selection only.
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

    /// `go` with the new target's descriptor; `None` is an unresolved object, which skips
    /// `IsSelectable` (`0x4935c8`).
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

    /// A world with just what the seams need.
    fn commit_world() -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<Messages<crate::player::StandStateRequest>>();
        world.init_resource::<Selection>();
        world.init_resource::<crate::ui_loot::LootState>();
        world.init_resource::<crate::ui_loot::LootLatch>();
        world.spawn(SelfPlayer);
        (world, rx)
    }

    /// The teardown of the outgoing target (`0x493910`) closes a loot window on it
    /// (`0x493959`–`0x493974`), its release ahead of the new selection; a window on anything else
    /// stays open. The clear (`0x4938f8`) runs the same teardown.
    #[test]
    fn a_selection_change_closes_the_loot_on_the_outgoing_target() {
        use bevy::ecs::system::RunSystemOnce;
        const CORPSE: u64 = 0xF130_0000_0000_0042;
        const NEXT: u64 = 0xF130_0000_0000_0043;
        const CHEST: u64 = 0xF110_0000_0000_1234;
        let (mut world, rx) = commit_world();
        let wire = |rx: &crossbeam_channel::Receiver<ClientCommand>| {
            rx.try_iter()
                .map(|c| match c {
                    ClientCommand::LootRelease { guid } => format!("release {guid:#x}"),
                    ClientCommand::SetSelection { guid } => format!("select {guid:#x}"),
                    _ => "other".to_owned(),
                })
                .collect::<Vec<_>>()
        };
        let open = |world: &mut World, guid: u64| {
            world
                .resource_mut::<crate::ui_loot::LootState>()
                .open(guid, 1, 0, Vec::new());
            world.resource_mut::<crate::ui_loot::LootLatch>().0 = Some(guid);
        };
        let source = |world: &World| world.resource::<crate::ui_loot::LootState>().source();

        // Tab from the looted corpse to the next mob: release, then select.
        assert!(go(&mut world, CORPSE, false, Some(1), false).changed);
        open(&mut world, CORPSE);
        let _ = wire(&rx);
        assert!(go(&mut world, NEXT, false, Some(1), true).changed);
        assert_eq!(
            wire(&rx),
            ["release 0xf130000000000042", "select 0xf130000000000043"]
        );
        assert_eq!(source(&world), None);
        assert_eq!(world.resource::<crate::ui_loot::LootLatch>().0, None);

        // A chest's window is not the selection's: a change leaves it open.
        open(&mut world, CHEST);
        assert!(go(&mut world, CORPSE, false, Some(1), false).changed);
        assert_eq!(wire(&rx), ["select 0xf130000000000042"]);
        assert_eq!(source(&world), Some(CHEST));

        // Clearing the target tears it down the same way.
        open(&mut world, CORPSE);
        world
            .run_system_once(
                |mut selection: ResMut<Selection>, mut seam: crate::creature_anim::AttackSeam| {
                    super::super::click::clear(&mut selection, &mut seam, false);
                },
            )
            .expect("clear runs as a one-shot system");
        assert_eq!(wire(&rx), ["release 0xf130000000000042", "select 0x0"]);
        assert_eq!(source(&world), None);
    }

    /// `0x493540`: an engaged switch is stop, select, re-swing; the stop un-queues an on-next-swing
    /// strike (`0x6e6f30`) and the re-swing cancels auto-repeat (`0x5ecd8c`).
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

        // Engaged switch onto an attackable unit: stop, select, swing.
        let out = go(&mut world, 0xC, true, Some(1), true);
        assert!(out.changed && out.swung);
        assert_eq!(drain(&rx), ["stop", "select", "swing"]);

        // Engaged switch onto myself (`TargetUnit("player")` mid-combat): stop, no re-point.
        let out = go(&mut world, 0x1, true, Some(1), true);
        assert!(out.changed && !out.swung);
        assert_eq!(drain(&rx), ["stop", "select"]);

        // Engaged switch onto an unattackable unit (vendor/corpse): stop, no swing at it.
        let out = go(&mut world, 0xD, true, Some(1), false);
        assert!(out.changed && !out.swung);
        assert_eq!(drain(&rx), ["stop", "select"]);

        // Engaged first select, no old target: the latch is off, selection only.
        *world.resource_mut::<Selection>() = Selection::default();
        let out = go(&mut world, 0xE, true, Some(1), true);
        assert!(out.changed && !out.swung);
        assert_eq!(drain(&rx), ["select"]);

        // A queued strike and auto-repeat: the stop un-queues one, the re-swing ends the other.
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

    /// `IsSelectable` refuses the whole commit before the dedup (`0x4935ee`/`0x4935f3`): nothing is
    /// sent and the held target survives. vmangos flags the pre-event Baron Rivendare this way
    /// (`instance_stratholme.cpp:224`).
    #[test]
    fn not_selectable_refuses_the_commit_and_keeps_the_old_target() {
        use benilla_protocol::ObjectFields;
        const FLAGS: u16 = 46;
        const CREATEDBY: u16 = 14;
        const OBJECT_TYPE: u16 = 2;
        const NOT_SELECTABLE: u32 = 1 << 25;
        // `OBJECT_FIELD_TYPE` bit 3, a unit: the only kind whose `+0x58` slot is not the base stub.
        let unit = |pairs: &[(u16, u32)]| {
            let mut all = vec![(OBJECT_TYPE, 0x8u32)];
            all.extend_from_slice(pairs);
            ObjectStore(ObjectFields::from_pairs(&all))
        };

        let (mut world, rx) = commit_world();
        let sent = |rx: &crossbeam_channel::Receiver<ClientCommand>| rx.try_iter().count();

        let plain = unit(&[]);
        assert!(go_with(&mut world, 0xA, Some(plain), false, Some(1), true).changed);
        assert_eq!(sent(&rx), 1);

        // The Baron: flagged, created by nobody, refused; 0xA is still the target.
        let baron = unit(&[(FLAGS, NOT_SELECTABLE | 0x2)]);
        let out = go_with(&mut world, 0xB, Some(baron), true, Some(1), true);
        assert!(!out.changed && !out.swung, "the commit is a complete no-op");
        assert_eq!(sent(&rx), 0, "nothing goes out — not even the stop");
        assert_eq!(
            world.resource::<Selection>().guid,
            Some(0xA),
            "the target already in hand survives the refused click"
        );

        // The `CREATEDBY` clause: the same flag on a unit I created still selects (my totem).
        let mine = unit(&[(FLAGS, NOT_SELECTABLE), (CREATEDBY, 1), (CREATEDBY + 1, 0)]);
        assert!(go_with(&mut world, 0xC, Some(mine), false, Some(1), true).changed);
        assert_eq!(sent(&rx), 1);

        // Somebody else's flagged creation is not mine to click.
        let theirs = unit(&[(FLAGS, NOT_SELECTABLE), (CREATEDBY, 9), (CREATEDBY + 1, 0)]);
        assert!(!go_with(&mut world, 0xD, Some(theirs), false, Some(1), true).changed);
        assert_eq!(sent(&rx), 0);

        // A non-unit object answers the base stub: a GameObject is never the selection.
        let go_obj = ObjectStore(ObjectFields::from_pairs(&[(OBJECT_TYPE, 0x20)]));
        assert!(!go_with(&mut world, 0xE, Some(go_obj), false, Some(1), true).changed);
        assert_eq!(sent(&rx), 0);

        // An unresolved object (an out-of-range party member) skips the check.
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

    #[test]
    fn priority_score_weighs_center_distance_and_combat() {
        assert!(
            priority_score(Some(0.05), 30.0, false) < priority_score(Some(0.9), 3.0, false),
            "a centered far mob outranks a screen-edge close one"
        );
        // Same centrality: the nearest wins.
        assert!(priority_score(Some(0.2), 5.0, false) < priority_score(Some(0.2), 35.0, false));
        assert!(
            priority_score(Some(0.9), 40.0, true) < priority_score(Some(0.0), 1.0, false),
            "the combat bonus dominates the geometric terms"
        );
    }

    /// No combat lock: an attacker in the pool never shrinks it.
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
        // No on-screen candidates at all: the fallback tier is everything.
        let off = [cand(1, 0.3, false), cand(2, 0.1, false)];
        assert_eq!(select_pool(&off).len(), 2);

        // With the attacker current and visited, the pick walks onto the peaceful mob.
        let mixed = [cand(1, -2.0, true), cand(2, 0.2, true)];
        let pool = select_pool(&mixed);
        assert_eq!(pool.len(), 2, "no combat lock — the full tier stays");
        assert_eq!(
            pick_forward(&pool, &[1], Some(1)),
            Some((1, false)),
            "TAB moves off the attacker onto the fresh target"
        );
    }

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
        // Only the current selection in the pool: itself, wrapped, which the commit dedups.
        let solo = [cand(0xA, 0.1, true)];
        assert_eq!(pick_forward(&solo, &[0xA], Some(0xA)), Some((0, true)));
        assert_eq!(pick_forward(&[], &[], None), None);
    }

    /// `0x6130a3`: a friendly selection is a reason to acquire, not an error.
    #[test]
    fn phase_b_keeps_only_a_hostile_selection() {
        let hostile = |g: u64| g == 0xBAD;
        assert_eq!(keeps_held_target(Some(0xBAD), hostile), Some(0xBAD));
        assert_eq!(keeps_held_target(Some(0x600D), hostile), None, "friendly");
        assert_eq!(keeps_held_target(None, hostile), None, "no selection");
    }

    /// `0x613152`–`0x613169`: a zero-health target is invalid unless dynamic-flag bit 5 is set.
    #[test]
    fn the_attack_orders_final_gate_reads_health_then_can_attack() {
        use benilla_protocol::ObjectFields;
        const HEALTH: u16 = 22;
        const DYNFLAGS: u16 = 143;
        const FLAGS: u16 = 46;
        const TPL: u16 = 35;
        let reps = Reputations::default();
        let unit = |pairs: &[(u16, u32)]| ObjectStore(ObjectFields::from_pairs(pairs));
        // With no catalog both reactions are neutral, which `CanAttack`'s mixed arm (< 4) admits,
        // so the legs below decide.
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
        // No descriptor on either side is refused.
        assert!(!attack_target_valid(None, None, &reps, Some(&me)));
        assert!(!attack_target_valid(
            Some(&unit(&[(HEALTH, 120)])),
            None,
            &reps,
            None
        ));
    }

    /// `reverse` is the optional Lua argument of `TargetNearestEnemy` and `TargetNearestFriend`
    /// (fetched with `0x6f1c10`, default 0): back walks the forward order in reverse.
    #[test]
    fn pick_back_walks_the_history_newest_first() {
        let pool = [
            cand(0xA, 0.1, true),
            cand(0xB, 0.2, true),
            cand(0xC, 0.3, true),
        ];
        // Forward over a fresh pool visits A, then B, then C.
        assert_eq!(pick_forward(&pool, &[], None), Some((0, false)));
        assert_eq!(pick_forward(&pool, &[0xA], Some(0xA)), Some((1, false)));
        assert_eq!(
            pick_forward(&pool, &[0xA, 0xB], Some(0xB)),
            Some((2, false))
        );
        // Back from C is B, and back from B is A.
        assert_eq!(pick_back(&[0xA, 0xB, 0xC], &pool, Some(0xC)), Some(1));
        assert_eq!(pick_back(&[0xA, 0xB], &pool, Some(0xB)), Some(0));
        // Nothing behind us: the caller falls through to the forward rule.
        assert_eq!(pick_back(&[], &pool, None), None);
        assert_eq!(pick_back(&[0xA], &pool, Some(0xA)), None);
        // A remembered guid that has left the pool (died, streamed out) is skipped, not picked.
        let shrunk = [cand(0xA, 0.1, true), cand(0xC, 0.3, true)];
        assert_eq!(pick_back(&[0xA, 0xB, 0xC], &shrunk, Some(0xC)), Some(0));
    }

    /// `0x493f60` rebuilds its list when the mode changes.
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
    /// `UNIT_FLAG_PVP_ATTACKABLE`, always on a player: it selects the player arms of `CanAttack`
    /// and `CanAssist`.
    const CONTROLLED: u32 = 0x8;
    /// `OBJECT_FIELD_TYPE` for a Player object (OBJECT|UNIT|PLAYER).
    const TYPE_PLAYER: u32 = 0x19;

    fn store(pairs: &[(u16, u32)]) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs))
    }

    /// With no `FactionTemplate.dbc` every reaction is neutral: attackable (`CanAttack`'s mixed
    /// arm is < 4), not assistable (`CanAssist` needs >= 4). A same-team duel partner is friendly
    /// from descriptor fields alone (`0x6061e0`). Mode 2 keeps a feigning ally and drops a corpse.
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

        // Mode 1: the neutral mob only; `CanAttack`'s player arm refuses every duel-team ally.
        assert_eq!(pool(&mut world, ScanSide::Enemy), [MOB]);
        // Mode 2: with no camera the order is pure distance.
        assert_eq!(pool(&mut world, ScanSide::Friend), [ALLY, ALLY_FEIGN]);
    }

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
            // The election's out-of-view verdict.
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

    /// It follows a hostile switch and survives a friendly selection and a clear (`0x49377d`).
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
        // A friendly selection: the memory holds at B.
        assert_eq!(select(&mut world, Some((friend, 0xF))), Some(0xB));
        // So does a clear.
        assert_eq!(select(&mut world, None), Some(0xB));

        // The four conjuncts besides `CanAttack` (`0x49372f`-`0x493778`), one state at a time.
        let dead_mob = world
            .spawn(store(&[(F_HEALTH, 0), (F_MAXHEALTH, 100)]))
            .id();
        assert_eq!(
            select(&mut world, Some((dead_mob, 0xD))),
            Some(0xB),
            "a corpse is not remembered — the target's health leg"
        );
        // A feigner, with the dead-looking dynflag, is remembered.
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

        // Our own body's three states: with each set, selecting a live hostile leaves 0xE.
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
