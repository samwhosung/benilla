//! TAB and nearest-unit targeting, and the attack auto-acquires.
//!
//! Deviation: TAB uses Classic Era's screen-space priority, not 1.12's ±30° facing cone and
//! snapshot cursor (`0x493f60`), because the cone skips a close mob in front of the camera, which
//! sits behind the character. On-screen candidates come first, the lowest score wins, and each
//! press skips recently tabbed guids; a fighting-me bonus, not Classic's hard combat lock, ranks
//! attackers first without pinning TAB to them.
//!
//! Kept from 1.12: the validity filters (`0x493e40`, `CanAttack 0x606980`, critters), the
//! [`commit_owing`] through `SetSelection` (`0x493540`), the attack acquire (`0x612df0` at `0x6130b5`),
//! the ATTACKERSTATEUPDATE and spell start acquires (`0x6259c9`–`0x6259fe`, `0x6e7963`-`0x6e799c`)
//! and the 41 yd range.

use benilla_formats::CreatureTypeTable;
use benilla_protocol::{guid, EntityKind};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::creature_anim::StartTarget;
use crate::names::NameCache;
use crate::net::{ClientCommand, Guid, GuidIndex, NetEntity, ObjectStore, SelfPlayer};
use crate::query_cache::Ask;
use benilla_assets::{LockRecover, WorldAssets};

use super::relations::{RelUnit, RelationWorld, Relations};
use super::Selection;

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
/// the mode reaches only the per-candidate filter `0x493e40`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScanSide {
    /// Mode 1 (`0x493e73`): alive by the reads-dead triple `0x605f90`, and `CanAttack 0x606980`.
    Enemy,
    /// Mode 2 (`0x493eca`): `CanAssist 0x6066f0`, friendly or better (`0x60671e`), whose
    /// `IsPvP 0x605ff0` leg refuses a creature without `UNIT_FLAG_PVP`; then health > 0 with no
    /// dynflag leg, so a feigning ally counts.
    Friend,
    /// Mode 3 (`0x493eed`): not us, then `0x4e7f70`, a member of our party slots. No health, dead
    /// or reaction leg.
    Party,
    /// Mode 4 (`0x493f15`): not us, then `0x4918e0`, party (`0x4e7f70`) or raid roster
    /// (`0x4baee0`).
    Raid,
}

impl ScanSide {
    /// The side a `TargetNearest*` call's mode names.
    pub(crate) fn of(mode: benilla_ui::script::NearestMode) -> Self {
        use benilla_ui::script::NearestMode;
        match mode {
            NearestMode::Enemy => Self::Enemy,
            NearestMode::Friend => Self::Friend,
            NearestMode::PartyMember => Self::Party,
            NearestMode::RaidMember => Self::Raid,
        }
    }
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

/// `CreatureType.dbc`: the flags, whose no-TAB bit is set on type 8, Critter, alone (Totem reads
/// 0), and the type words at the install's locale. Absent after a load failure, when nothing is
/// filtered and no unit has a type word.
#[derive(Resource)]
pub(crate) struct CreatureTypes(pub(crate) CreatureTypeTable);

#[cfg(test)]
impl CreatureTypes {
    /// The words of a koKR install's column, for a test that must tell the table's word from
    /// English: Beast, Demon, Humanoid and "Not specified".
    pub(crate) fn korean_for_tests() -> Self {
        Self(CreatureTypeTable::from_rows([
            (1, "야수", 0),
            (3, "악마", 0),
            (7, "인간형", 0),
            (10, "지정되지 않음", 0),
        ]))
    }
}

pub(super) fn load_creature_types(mut commands: Commands, world_assets: Option<Res<WorldAssets>>) {
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match benilla_formats::load_creature_type_table(&mut chain) {
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
    /// The relation functions' inputs, `CanAttack`'s and `CanAssist`'s.
    relations: RelationWorld<'w, 's>,
    names: Res<'w, NameCache>,
    creature_types: Option<Res<'w, CreatureTypes>>,
    /// Every store, ours included: `CanAssist`'s owner chase must reach our body.
    stores: Query<'w, 's, &'static ObjectStore>,
    /// The owner chase's guid → entity map; `Option` because a UI-only harness has no net stack.
    index: Option<Res<'w, GuidIndex>>,
    /// The party slots and raid roster modes 3 and 4 test; `Option` for the same harnesses.
    group: Option<Res<'w, crate::ui_party::GroupState>>,
}

impl TargetScan<'_, '_> {
    /// The per-candidate filter (`0x493e40`), the one place [`ScanSide`] forks.
    fn is_valid(&self, side: ScanSide, guid: u64, store: Option<&ObjectStore>) -> bool {
        let rel = self.relations.get();
        let pair = self.pair(guid, store);
        match side {
            // Mode 1 (`0x493e73`): the reads-dead triple `0x605f90`, then `CanAttack`.
            ScanSide::Enemy => {
                if store.is_some_and(|s| s.0.unit_reads_dead()) {
                    return false;
                }
                pair.is_some_and(|(me, unit)| rel.can_attack(&me, &unit))
            }
            // Mode 2 (`0x493eca`): `CanAssist 0x6066f0`, then raw health (no reads-dead triple).
            ScanSide::Friend => {
                pair.is_some_and(|(me, unit)| rel.can_assist(&me, &unit))
                    && !store.is_some_and(|s| s.0.unit_is_dead())
            }
            // Modes 3 and 4 open on `cmp esi,ecx`, the candidate is not us, which our own body's
            // absence from [`Self::units`] already gives.
            ScanSide::Party => self
                .group
                .as_ref()
                .is_some_and(|g| g.party_slots().any(|m| m.guid == guid)),
            // A party slot or a stored raid row (`0x4918e0` past its own test).
            ScanSide::Raid => self
                .group
                .as_ref()
                .is_some_and(|g| g.party_or_raid_member(guid)),
        }
    }

    /// The active player and the unit `guid` names off `store`, the scan's two relation sides.
    fn pair(&self, guid: u64, store: Option<&ObjectStore>) -> Option<(RelUnit, RelUnit)> {
        Some((self.relations.me()?, self.relations.unit_of(guid, store?)))
    }

    /// A guid's descriptor, whichever entity holds it.
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

    /// The object `guid` names (`0x468460`, typemask 1).
    fn entity_of(&self, guid: u64) -> Option<Entity> {
        self.index.as_ref()?.0.get(&guid).copied()
    }

    fn store_at(&self, entity: Entity) -> Option<&ObjectStore> {
        self.stores.get(entity).ok()
    }

    /// Filter, project, score and sort every known unit, fresh each press.
    fn build(&self, side: ScanSide) -> Vec<Candidate> {
        let Ok((self_tf, _, self_guid)) = self.self_q.single() else {
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
                self.names
                    .resolve_unit(guid, None, Ask::Read)
                    .unwrap_or("?"),
                (tf.translation - self_tf.translation).length()
            );
        };
        let mut out = Vec::new();
        for (entity, net, guid_c, tf, store) in &self.units {
            if !matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
                continue;
            }
            if !self.is_valid(side, guid_c.0, store) {
                trace_unit(
                    guid_c.0,
                    tf,
                    match side {
                        ScanSide::Enemy => "REJECT dead-or-unattackable",
                        ScanSide::Friend => "REJECT dead-or-unassistable",
                        ScanSide::Party => "REJECT not-in-party",
                        ScanSide::Raid => "REJECT not-in-party-or-raid",
                    },
                );
                continue;
            }
            // The critter gate; an unresolved type passes, like the client's out-of-range index.
            if guid::is_creature_or_pet(guid_c.0) {
                if let (Some(types), Some(ty)) = (
                    self.creature_types.as_deref(),
                    store
                        .and_then(|s| s.0.object_entry())
                        .filter(|&e| e != 0)
                        .and_then(|e| self.names.creature_type(e)),
                ) {
                    if types.0.no_tab_target(ty) {
                        trace_unit(guid_c.0, tf, "REJECT critter-type");
                        continue;
                    }
                }
            }
            // Deviation: no drawn gate (the reference's IsEntityDrawn, `0x6704c0`), because the
            // draw election hides every body outside the frustum, which would empty the off-screen
            // tier.
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
                    self.names
                        .resolve_unit(c.guid, None, Ask::Read)
                        .unwrap_or("?"),
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

/// What one [`commit_owing`] did; `swung` means it ran the re-swing (`0x4938c8`), so a caller must not ask
/// again. StartAttack's own gate may have refused it ([`crate::creature_anim::StartGate`]), and the
/// caller's would refuse alike: `swung` says it was asked, not that `CMSG_ATTACKSWING` went out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CommitOutcome {
    pub(super) changed: bool,
    pub(super) swung: bool,
}

/// The `CMSG_SET_SELECTION` a selection's teardown owes when it called a possessed unit off
/// (`0x493a18`): `PetStopAttack` signals `PET_BAR_UPDATE` (`0x4bd6b4`) with the pair zeroed
/// (`0x4939d3`), and its listeners run before the store (`0x4936e2`) and the send (`0x493871`), or
/// a clear's send (`0x493a2a`).
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwedSend {
    /// A switch to `guid`, whose object is `entity`.
    Select { entity: Option<Entity>, guid: u64 },
    /// A clear, whose send names the selection as the listeners left it.
    Deselect,
}

impl OwedSend {
    /// The store and the light (`0x4936e2`, `0x493701`), over whatever a listener selected, the
    /// lit unit's tutorial ladder (`0x493789`-`0x49383c`, [`crate::tutorial::select_tutorial`]),
    /// and the send.
    pub(crate) fn pay(self, selection: &mut Selection, seam: &crate::creature_anim::AttackSeam) {
        let guid = match self {
            OwedSend::Select { entity, guid } => {
                selection.target = entity;
                selection.guid = Some(guid);
                selection.lit = entity;
                if let (Some(entity), Some(tutorials)) = (entity, seam.slots.tutorials()) {
                    tutorials.selected(entity, guid);
                }
                guid
            }
            // The clear's send reads the selection as it stands (`0x493a49`-`0x493a4f`): a
            // listener's pick, or none.
            OwedSend::Deselect => selection.guid.unwrap_or(0),
        };
        let _ = seam.net.0.send(ClientCommand::SetSelection { guid });
    }
}

/// What a [`commit_owing`] owes after its signals, for a caller that runs their listeners in place
/// first: the send after the teardown's `PET_BAR_UPDATE`, then the re-swing after
/// `PLAYER_TARGET_CHANGED`.
#[must_use]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Owed {
    pub(crate) send: Option<OwedSend>,
    pub(crate) swing: Option<OwedSwing>,
}

impl Owed {
    /// Pay both now, for a test whose signals have no listeners to run first.
    #[cfg(test)]
    pub(crate) fn pay(
        self,
        selection: &mut Selection,
        seam: &mut crate::creature_anim::AttackSeam,
    ) {
        if let Some(send) = self.send {
            send.pay(selection, seam);
        }
        if let Some(swing) = self.swing {
            // Nothing ran since the commit, so the selection is the guid it stored.
            swing.pay(selection, seam, StartTarget::Unheld);
        }
    }
}

/// The re-swing a [`commit_owing`] owes (`0x4938a1`-`0x4938c8`), paid after `SetSelection`'s
/// `PLAYER_TARGET_CHANGED` (`0x49389c`), whose listeners run first.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OwedSwing {
    /// The guid the commit selected.
    guid: u64,
    /// What `StartAttack`'s target checks find of it (`0x5ecbe6`-`0x5ecc35`).
    verdict: StartTarget,
    self_guid: Option<u64>,
}

impl OwedSwing {
    /// The re-swing: `StartAttack` at the selection as it stands now, since `0x4938c3` pushes the
    /// selection's address, `0xb4e2d8`, which a listener of the signal may have moved. Our own
    /// guid gets none (`0x4938ae`-`0x4938c1`), nothing selected finds no unit (`0x5ecbef`), and a
    /// unit a listener selected is judged `now`. The swing goes out despite the lock, the stop
    /// just set `[+0xc54]` (`0x5eccda`); a refused target stops again (`0x5ecc48`).
    pub(crate) fn pay(
        self,
        selection: &Selection,
        seam: &mut crate::creature_anim::AttackSeam,
        now: StartTarget,
    ) {
        let Some(guid) = selection.guid else {
            return;
        };
        if self.self_guid == Some(guid) {
            return;
        }
        let verdict = if guid == self.guid { self.verdict } else { now };
        seam.start(guid, verdict);
    }
}

/// Commit a target through `SetSelection` (`0x493540`). A switch while attacking is stop, select,
/// re-swing: StopAttack `0x5ecac0` (`0x493a08`) flags a stop in flight (`[player+0xc54]`),
/// `CMSG_SET_SELECTION` goes out (`0x493857`), then unless the new target is us `Attack 0x5ecb70`
/// swings past its already-attacking gate, or stops again if `new_attackable` is false
/// (`0x4938a1`, `0x5ecc48`).
/// `entity` is the guid's object, which the select lights (`0x493701`); with none the guid is
/// stored all the same (`0x4935c8` → `0x493605`), and neither `IsSelectable` nor the light runs.
/// Whether we are attacking is [`crate::creature_anim::Engaged`] as the queued unit edits leave it
/// ([`crate::creature_anim::AttackSeam::engaged`]), which StartAttack's send sets locally
/// (`[+0xc48]`, `[+0xc50]` = 1, `0x5ecd51` → `0x47bf70`) and the stop's echo clears (`0x624e40`).
/// It owes what follows its signals ([`Owed`]), for a caller that runs their listeners in place
/// before paying it. `old_latches` is [`latches_on`] the selection as it stands, which a
/// selection made at another's signal finds to be that one's pick.
pub(super) fn commit_owing(
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    entity: Option<Entity>,
    guid: u64,
    target_store: Option<&ObjectStore>,
    self_guid: Option<u64>,
    new_attackable: bool,
    old_latches: bool,
) -> (CommitOutcome, Owed) {
    // `SelectTarget`'s charmed return (`0x49359c`) is ahead of everything, for every writer.
    if seam.player_is_charmed() {
        return Default::default();
    }
    // `IsSelectable`, before the dedup (`0x4935ec`–`0x4935f3`): a non-selectable unit is a complete
    // no-op that keeps the current target, for every selection writer.
    if !super::relations::is_selectable(target_store, self_guid) {
        return Default::default();
    }
    if selection.guid == Some(guid) {
        return Default::default(); // the setter's dedup
    }
    let old = selection.guid;
    let had_old = old.is_some();
    // The last-target stamp (`0x49361d`-`0x493628`), past the two early outs.
    selection.last = selection.guid;
    if let Some(old) = selection.guid {
        // The old target's teardown (`0x4936cc` → `0x493910`) closes its loot first.
        seam.close_loot_on(old);
    }
    // The latch's `0x60ecb0` leg, read before the store (`0x49367d`).
    let stop_and_repoint = seam.engaged() && had_old;
    if stop_and_repoint {
        // `0x493a08`: the real StopAttack, which also un-queues a next-swing strike (`0x6e6f30`).
        seam.stop();
    }
    // `0x493a0f`-`0x493a18`: the release calls a possessed unit off after its StopAttack, with the
    // pair zeroed (`0x4939d3`); the store and the send wait for its signal's listeners.
    let called_off = had_old && seam.call_pet_off();
    let send = OwedSend::Select { entity, guid };
    let send = if called_off {
        selection.target = None;
        selection.guid = None;
        selection.lit = None;
        Some(send)
    } else {
        send.pay(selection, seam);
        None
    };
    // The latch `[ebp-1]` was read before the store (`0x4936ac`); the rest of the test reads the
    // selection after the signal (`0x4938ae`).
    // `CanAttack` refuses the player itself (`0x4936a3`), whatever the writer knows of the rest.
    let latched = stop_and_repoint && old_latches && old != self_guid;
    let swung = latched && new_attackable && self_guid != Some(guid);
    let swing = latched.then_some(OwedSwing {
        guid,
        verdict: match (entity, new_attackable) {
            (None, _) => StartTarget::Unheld,
            (Some(_), true) => StartTarget::Attackable,
            (Some(_), false) => StartTarget::Refused,
        },
        self_guid,
    });
    (
        CommitOutcome {
            changed: true,
            swung,
        },
        Owed { send, swing },
    )
}

/// [`commit_owing`] paid at once, for a test whose signals have no listeners to run first.
#[cfg(test)]
pub(super) fn commit(
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    entity: Option<Entity>,
    guid: u64,
    target_store: Option<&ObjectStore>,
    self_guid: Option<u64>,
    new_attackable: bool,
    old_latches: bool,
) -> CommitOutcome {
    let (out, owed) = commit_owing(
        selection,
        seam,
        entity,
        guid,
        target_store,
        self_guid,
        new_attackable,
        old_latches,
    );
    owed.pay(selection, seam);
    out
}

/// One press on either side, as every `TargetNearest*` shim shares `0x493f60`: score the live
/// world, pool by tier, walk the history forward or back, and [`commit_owing`]; what its commit
/// owes, for the caller to pay once the signals' listeners ran.
pub(super) fn cycle(
    side: ScanSide,
    reverse: bool,
    now: f64,
    scan: &TargetScan,
    history: &mut TabHistory,
    selection: &mut Selection,
    seam: &mut crate::creature_anim::AttackSeam,
    front: &mut super::front::SelectFront,
) -> Owed {
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
        return Owed::default();
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
                return Owed::default();
            };
            (pool[i].entity, pool[i].guid, wrapped)
        }
    };
    // `0x4941e6`: the pick goes to `SelectTarget`, whose front may bind it to the cursor's cast
    // instead. The cycle's index was stored before that call (`0x4941d4`), so the next press moves
    // past the pick whatever became of it; the history, which stands in for the index, records it
    // as visited without a selection to push.
    if front.takes(entity, guid, seam.slots.targeting()) {
        if wrapped {
            history.visited.clear();
        }
        history.push(guid, now);
        return Owed::default();
    }
    if wrapped {
        history.visited.clear();
    }
    // The outgoing selection joins the history (`TargetPriorityContinueFromManualTarget`).
    if let Some(old) = selection.guid {
        history.push(old, now);
    }
    let old_latches = latches_on(&scan.relations, selection.guid);
    let (out, owed) = commit_owing(
        selection,
        seam,
        Some(entity),
        guid,
        scan.store_at(entity),
        // `IsSelectable`'s `CREATEDBY` clause reads it.
        scan.self_guid(),
        // `Attack 0x5ecb70`'s validation, which a mode-2 pick fails (reaction >= 4 against
        // `CanAttack`'s < 4). A duelling friendly passes `CanAttack`'s duel arm, which reads no
        // reaction, so the reference's `Attack` would re-swing at it; here it gets none.
        side == ScanSide::Enemy,
        old_latches,
    );
    if out.changed {
        history.push(guid, now);
    }
    if trace {
        info!(
            "tab-trace: pick {guid:#x} \"{}\"{} — {}",
            scan.names
                .resolve_unit(guid, None, Ask::Read)
                .unwrap_or("?"),
            if wrapped { " (wrapped)" } else { "" },
            if out.changed {
                "selection changed"
            } else {
                "NO-OP (already the selection)"
            }
        );
    }
    owed
}

/// The target half of the attack validator `0x612df0` (`0x61305d`–`0x613198`), shared by the
/// player's Attack (`0x6131aa`) and the pet bar's (`0x4bd40d`): what it reads beyond the
/// selection and the cycler it runs as `TargetNearestEnemy` (`0x6130b5`, `0x493f60(0, 1)`).
#[derive(SystemParam)]
pub(crate) struct AttackPick<'w, 's> {
    scan: TargetScan<'w, 's>,
    history: ResMut<'w, TabHistory>,
    time: Res<'w, Time>,
    /// The `SelectTarget` returns ahead of the validator's two selecting calls.
    front: super::front::SelectFront<'w, 's>,
}

impl AttackPick<'_, '_> {
    /// Our own descriptor and guid, the actor of the player's Attack.
    pub(crate) fn player(&self) -> (Option<&ObjectStore>, Option<u64>) {
        (self.scan.self_store(), self.scan.self_guid())
    }

    /// The validator's target pick up to the selection's re-read (`0x61305d`-`0x6130bc`), what
    /// its commit owes after its signals ([`Owed`]), for a caller that runs their listeners in
    /// place, as `SetSelection` does inside the validator, before it [`Self::settle`]s. The
    /// candidate is the guid the caller passed, else the selection (`0x61306b`): `AttackTarget`
    /// passes none (`0x489b50` → `0x6131a0(0,0)`), nor does the pet bar (`0x4be42b`); TryCast
    /// passes the caster on a self-cast press (`0x4e610e`), and a go's deferred start its first
    /// hit (`0x6e8402`). An empty candidate (`0x61307f`) or a
    /// unit the actor is friendly toward (`0x6130a3`, dropped at `0x6130a8`), the caster included,
    /// runs `TargetNearestEnemy` (`0x6130b5`), which commits its pick inside the validator, so a
    /// call after this one reads it. `may_pick` is the protected-action gate's tag 4, which opens
    /// the cycle (`0x493f9b`): refused, the cycle returns having picked nothing. A unit kept goes
    /// to `SelectTarget` (`0x613100`) and the cycle's pick does too (`0x4941e6`), so with the
    /// targeting cursor up either is bound to the cast and the selection stays
    /// ([`super::front`]).
    pub(crate) fn pick(
        &mut self,
        passed: Option<u64>,
        may_pick: bool,
        selection: &mut Selection,
        seam: &mut crate::creature_anim::AttackSeam,
    ) -> Owed {
        let candidate = passed.or(selection.guid);
        if let Some(guid) = candidate.filter(|&g| self.keeps(g)) {
            return self.select_kept(guid, selection, seam);
        } else if may_pick {
            return cycle(
                ScanSide::Enemy,
                false,
                self.time.elapsed_secs_f64(),
                &self.scan,
                &mut self.history,
                selection,
                seam,
                &mut self.front,
            );
        }
        Owed::default()
    }

    /// `0x613100`: `SelectTarget` on the kept candidate, whatever is selected: the selection's own
    /// guid is the setter's dedup, a passed unit a whole select, and with the targeting cursor up
    /// a held one is bound to the cast instead (`0x4935d5`). A guid that names no object is stored
    /// unlit (`0x4935c8` → `0x493605`).
    fn select_kept(
        &mut self,
        guid: u64,
        selection: &mut Selection,
        seam: &mut crate::creature_anim::AttackSeam,
    ) -> Owed {
        let entity = self.scan.entity_of(guid);
        if let Some(entity) = entity {
            if self.front.takes(entity, guid, seam.slots.targeting()) {
                return Owed::default();
            }
        }
        let store = entity.and_then(|e| self.scan.store_at(e));
        // The re-swing's gate is `StartAttack`'s on the new target (`0x5ecc16`).
        let attackable = store.is_some_and(|store| {
            let unit = self.scan.relations.unit_of(guid, store);
            attack_target_valid(
                &self.scan.relations.get(),
                self.scan.relations.me().as_ref(),
                &unit,
                store,
            )
        });
        let old_latches = latches_on(&self.scan.relations, selection.guid);
        let (_, owed) = commit_owing(
            selection,
            seam,
            entity,
            guid,
            store,
            self.scan.self_guid(),
            attackable,
            old_latches,
        );
        owed
    }

    /// The validator from the selection's re-read on (`0x6130c1`-`0x613198`), once the pick's
    /// listeners ran: the guid the swing, the pet's order or the cast goes at, or `None` once the
    /// refusal is raised. The target is the selection as it stands then, a listener's pick
    /// included: a fruitless scan leaves a friendly unit selected for the final gate to refuse,
    /// and only an empty selection is `ERR_NO_ATTACK_TARGET` (`0x6130d9`).
    pub(crate) fn settle(
        &self,
        selection: &Selection,
        errors: &crate::ui_action::UiErrorKeys,
    ) -> Option<u64> {
        let Some(guid) = selection.guid else {
            debug!("attack: nothing to attack (ERR_NO_ATTACK_TARGET)");
            errors.push(crate::ui_action::UiError::key("ERR_NO_ATTACK_TARGET"));
            return None;
        };
        // `0x613118`, the second resolve: a miss refuses with no message (`0x613123`).
        let Some(store) = self.scan.store_of(guid) else {
            debug!("attack: {guid:#x} does not resolve, refused silently");
            return None;
        };
        // The final gate's target legs; its actor legs (`0x61312e`, `0x613139`) repeat phase A's
        // dead and mounted refusals, which the caller ran.
        let me = self.scan.relations.me();
        let unit = self.scan.relations.unit_of(guid, store);
        if !attack_target_valid(&self.scan.relations.get(), me.as_ref(), &unit, store) {
            debug!("attack: {guid:#x} fails the final gate (ERR_INVALID_ATTACK_TARGET)");
            errors.push(crate::ui_action::UiError::key("ERR_INVALID_ATTACK_TARGET"));
            return None;
        }
        Some(guid)
    }

    /// `0x6130a3`'s keep test: the actor's reaction to the held unit alone (`0x61309b`), not the
    /// final gate's `CanAttack`, so a neutral mob is kept. A guid that does not resolve
    /// (`0x613099`) is kept as well, for the second resolve to refuse. The reaction is the
    /// actor's toward the target, the player's here; the reference's actor is the caller, a pet
    /// on the pet arm (`0x4bd40d`), which vmangos gives its owner's faction (`Pet.cpp:248`). The
    /// reaction to oneself is 4 before any faction rung (`0x606200`), so a passed caster is never
    /// kept, FFA flag or not.
    pub(crate) fn keeps(&self, guid: u64) -> bool {
        if self.scan.self_guid() == Some(guid) {
            return false;
        }
        let rel = self.scan.relations.get();
        self.scan.store_of(guid).is_none_or(|store| {
            self.scan
                .pair(guid, Some(store))
                .is_none_or(|(me, unit)| rel.reaction(&me, &unit) < 4)
        })
    }
}

/// The re-swing latch's legs on the outgoing selection `old` (`0x49365f`-`0x4936a3`), read before
/// the new guid is stored: it resolves as a unit (`0x468460`, typemask 8), it has health or dynamic
/// flag 0x20 (`0x49368d`-`0x4936a0`), and `CanAttack` (`0x606980`) allows it. The player's own legs
/// (`0x60ecb0`, `0x605f30`, `0x49367d`) are [`commit_owing`]'s.
pub(super) fn latches_on(relations: &RelationWorld, old: Option<u64>) -> bool {
    old.and_then(|guid| relations.unit_and_store(guid))
        .is_some_and(|(unit, store)| {
            attack_target_valid(&relations.get(), relations.me().as_ref(), &unit, store)
        })
}

/// The final gate's target legs (`0x613152`–`0x613169`): alive, where a zero-health target passes
/// iff `UNIT_DYNAMIC_FLAGS` bit 5 is set (`0x613159`), and the full `CanAttack 0x606980` from the
/// player. No player fails; the reference always holds one here.
pub(super) fn attack_target_valid(
    rel: &Relations,
    me: Option<&RelUnit>,
    unit: &RelUnit,
    store: &ObjectStore,
) -> bool {
    let alive =
        store.0.unit_health().is_none_or(|h| h > 0) || store.0.unit_dynamic_flags() & (1 << 5) != 0;
    alive && me.is_some_and(|me| rel.can_attack(me, unit))
}

/// The melee probe's press with nothing selected: the player's Attack, which acquires.
#[derive(Message)]
pub(crate) struct AttackNearestRequest;

/// `TargetLastEnemy`'s memory, the reference's last-attackable guid `[0xb4e2e8]`, read at
/// `0x489b45` and written at `0x49377d` in `SetSelection`. Never cleared: an unresolved guid is a
/// bare `ret` in the select helper `0x489a40`, not a deselect.
#[derive(Resource, Default)]
pub(crate) struct LastEnemy(pub(crate) Option<u64>);

/// Stamp [`LastEnemy`] behind the five-conjunct gate `0x49372f`–`0x493778`, on the object
/// `SetSelection` found and lit (`0x4936f3`-`0x493701`, [`Selection::lit`]): one that streams in
/// later under the selected guid was never selected, so it is never stamped. A hostile selected
/// alive was stamped the frame it was selected, so its death's deselect (`super::died`, in a later
/// frame's packet) leaves it remembered.
///
/// Deviation: sampled each frame, not stamped in `SetSelection`, so every selection writer is
/// covered; a selected unit that turns hostile is remembered, where the reference's is not.
pub(super) fn remember_last_enemy(
    selection: Res<Selection>,
    stores: Query<&ObjectStore>,
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    world: RelationWorld,
    mut last: ResMut<LastEnemy>,
) {
    let Some((entity, guid)) = selection.lit.zip(selection.guid) else {
        return;
    };
    // Conjunct 1: the player object resolves.
    let Some(me) = self_store.iter().next() else {
        return;
    };
    // Conjuncts 2 and 3: not dead or a ghost (a ghost's wire health is 1), and not mounted.
    if me.0.is_dead_or_ghost() || me.0.unit_mount_display_id() != 0 {
        return;
    }
    let store = stores.get(entity).ok();
    // Conjunct 4: health, or the dead-looking flag, so a feigning target stays remembered.
    let alive_enough = store.is_some_and(|s| !s.0.unit_is_dead() || s.0.unit_dynflag_dead());
    if !alive_enough {
        return;
    }
    // Conjunct 5: `CanAttack(player, new)`.
    let attackable = (|| {
        let (me, unit) = (world.me()?, world.unit_of(guid, store?));
        Some(world.get().can_attack(&me, &unit))
    })();
    if attackable == Some(true) {
        last.0 = Some(guid);
    }
}

/// What asked for a unit's select inside its packet ([`OwedSelect::Acquire`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Acquire {
    /// A swing at us (`SMSG_ATTACKERSTATEUPDATE`).
    Attacker,
    /// A spell start aimed at us (`SMSG_SPELL_START`).
    Caster,
}

/// A `SetSelection` a packet's handler owes, at its place in the handler, for the packet's end to
/// run with the VM held ([`run_owed_selects`]).
pub(crate) enum OwedSelect {
    /// The two packet acquires, each `SelectTarget` (`0x6259fe`, `0x6e799c`) as its handler's
    /// last act on the unit it names:
    /// - the ATTACKERSTATEUPDATE self-defence acquire (`0x6259c9`–`0x6259fe`): when we are the
    ///   victim and hold no selection, select the attacker, with no counter-attack;
    /// - the spell start's (`0x6e7963`-`0x6e799c`): another unit casting at us is selected when
    ///   the active player's object is held (`0x6e796a`), the caster cannot assist it
    ///   (`CanAssist 0x6066f0` at `0x6e7973`) and nothing is selected (`[0xb4e2d8]`, `0x6e7980`).
    ///
    /// `SelectTarget`'s front may bind the unit to a standing cursor's cast instead; a packet
    /// handler runs in no script, whose gate context passes tag 2.
    Acquire(Entity, Acquire),
    /// The signals of a select its handler committed in place, as it went on: the loot
    /// response's (`0x48f3a0`).
    Signals(crate::script_calls::EngineSelect),
    /// `DeselectGuid` (`0x493910(guid, 1)`), nothing unless `guid` is still the selection
    /// (`0x493949`), which signals inside itself (`0x493a69`, `0x493a73`) and leaves the
    /// last-target pair: a dead unit's loot close (`0x48f369`), an old pet's teardown (`0x5e26f7`),
    /// a group list's or ending's, a release's ([`super::release`]). A handler's runs at its packet's end, a system's or a lane's on
    /// this queue's lane at its place, a call's inside the call
    /// ([`crate::script_calls::ScriptCallHost`]).
    Deselect(u64),
}

/// The selects the packet's handlers owe, each at its place among what they queued.
#[derive(Resource, Default)]
pub(crate) struct OwedSelects(Vec<(crate::lanes::Stamp, OwedSelect)>);

impl OwedSelects {
    /// Owe `select` here, behind what the handler queued before it.
    pub(crate) fn push(&mut self, select: OwedSelect) {
        self.0.push((crate::lanes::stamp(), select));
    }

    /// Owe the teardown of `guid`'s selection ([`OwedSelect::Deselect`]).
    pub(crate) fn deselect(&mut self, guid: u64) {
        self.push(OwedSelect::Deselect(guid));
    }

    /// Where a call's owed selects begin, for [`Self::take_deselect`].
    pub(crate) fn mark(&self) -> usize {
        self.0.len()
    }

    /// The first deselect owed at `from` or after, taken.
    pub(crate) fn take_deselect(&mut self, from: usize) -> Option<u64> {
        let at = (from..self.0.len()).find(|&i| matches!(self.0[i].1, OwedSelect::Deselect(_)))?;
        match self.0.remove(at).1 {
            OwedSelect::Deselect(guid) => Some(guid),
            _ => None,
        }
    }

    /// The units the owed acquires name, taken, for a test of a handler that owes them.
    #[cfg(test)]
    pub(crate) fn take_acquires(&mut self) -> Vec<Entity> {
        std::mem::take(&mut self.0)
            .into_iter()
            .filter_map(|(_, s)| match s {
                OwedSelect::Acquire(unit, _) => Some(unit),
                _ => None,
            })
            .collect()
    }
}

/// The guids of the deselects owed and not yet run, taken, for a test of a writer that owes them.
#[cfg(test)]
pub(crate) fn take_owed_deselects(world: &mut World) -> Vec<u64> {
    let mut owed = world.get_resource_or_init::<OwedSelects>();
    std::iter::from_fn(|| owed.take_deselect(0)).collect()
}

/// The owed selects' lane's head: what a system or a lane owed, as a packet's end runs its
/// handlers' ([`run_owed_selects`]).
pub(super) fn peek_owed_selects(world: &World) -> Option<crate::lanes::Stamp> {
    world
        .get_resource::<OwedSelects>()?
        .0
        .first()
        .map(|(at, _)| *at)
}

/// Each owed select `until` admits, run as the firing of its stamp with the host lent, signalling
/// in place; the lane stops after one whose firing stamped anything, so what it pushed fires next.
pub(super) fn fire_owed_selects(In(until): In<crate::lanes::Until>, world: &mut World) {
    loop {
        let Some(owed) = world.get_resource_mut::<OwedSelects>() else {
            return;
        };
        if !owed.0.first().is_some_and(|(at, _)| until.admits(*at)) {
            return;
        }
        let (at, select) = owed.into_inner().0.remove(0);
        let ((), moved) = crate::lanes::firing(at, || run_owed_select(world, select));
        if moved {
            return;
        }
    }
}

/// The owed selects' lane fired whole, for a test that drives a writer without the lanes.
#[cfg(test)]
pub(crate) fn fire_owed_selects_lane(world: &mut World) {
    while peek_owed_selects(world).is_some() {
        fire_owed_selects(In(crate::lanes::Until::all()), world);
    }
}

/// One owed select on the world, its unit edits applied as it returns.
fn run_owed_select(world: &mut World, select: OwedSelect) {
    select_in_place(world, select);
    crate::creature_anim::apply_unit_edits(world);
}

/// One owed select, then the bind of a unit it handed the targeting cursor (`SelectTarget`,
/// `0x4935d5` → `0x6e5b40`) at the select's place: the acquires' selects (`0x6259fe`,
/// `0x6e799c`) bind inside their packets, so the cast goes out and signals there.
fn select_in_place(world: &mut World, select: OwedSelect) {
    if let Err(err) = world.run_system_cached_with(owed_select, select) {
        // A harness without the selection's seams selects nothing.
        debug!("target: an owed select did not run: {err}");
    }
    crate::script_calls::bind_picks_here(world);
}

/// The packet's owed selects, after its handlers, each run as the firing of its stamp with the
/// world's VM held, so its sends go out in the packet and what it signals is parked on the release
/// lane at its place ([`super::release::park_signals`]): the reference signals inside the packet
/// (`0x49389c`, `0x493a73`).
pub(super) fn run_owed_selects(world: &mut World, _: crate::lanes::Stamp) {
    let Some(mut owed) = world.get_resource_mut::<OwedSelects>() else {
        return;
    };
    for (at, select) in std::mem::take(&mut owed.0) {
        crate::lanes::firing(at, || {
            super::release::park_signals(world, |world| select_in_place(world, select));
        });
    }
}

/// Owe `why`'s select of `unit`, as a handler does, for [`run_owed_selects`] to run.
#[cfg(test)]
pub(super) fn owe_acquire(world: &mut World, unit: Entity, why: Acquire) {
    world
        .get_resource_or_init::<OwedSelects>()
        .push(OwedSelect::Acquire(unit, why));
}

/// [`run_owed_selects`] as a packet's end runs it, for a test of a handler that owes selects.
#[cfg(test)]
pub(super) fn run_owed_selects_now(world: &mut World) {
    run_owed_selects(world, crate::lanes::stamp());
}

/// One owed select: an acquire's commit and its signals, or a committed select's signals, each
/// signalling in place ([`crate::script_calls::ScriptCallHost::engine_select`]).
pub(super) fn owed_select(
    In(owed): In<OwedSelect>,
    mut vm: crate::script_calls::VmWith<AcquireSelect<'static, 'static>>,
) {
    let select = match owed {
        OwedSelect::Acquire(unit, why) => {
            let mut c = vm.own();
            let Some(my_guid) = c.self_player.single().ok().map(|(_, g)| g.0) else {
                return;
            };
            c.select(unit, why, my_guid)
        }
        OwedSelect::Signals(select) => Some(select),
        OwedSelect::Deselect(guid) => {
            let mut c = vm.own();
            if c.selection.guid != Some(guid) {
                return;
            }
            let before = super::click::Standing::of(&c.selection, &c.seam);
            let send = super::click::clear_owing(&mut c.selection, &mut c.seam);
            Some(before.owing(Owed { send, swing: None }))
        }
    };
    if let Some(select) = select {
        vm.engine_select(select, |mut c, owed| {
            owed.pay(&mut c.selection, &mut c.seam);
        });
    }
}

/// What an owed select reads and writes ([`owed_select`]).
#[derive(SystemParam)]
pub(super) struct AcquireSelect<'w, 's> {
    self_player: Query<'w, 's, (Entity, &'static Guid), With<SelfPlayer>>,
    guids: Query<'w, 's, &'static Guid>,
    // For `IsSelectable`: a `NOT_SELECTABLE` attacker is not acquired.
    stores: Query<'w, 's, &'static ObjectStore>,
    relations: RelationWorld<'w, 's>,
    selection: ResMut<'w, Selection>,
    seam: crate::creature_anim::AttackSeam<'w, 's>,
    front: super::front::SelectFront<'w, 's>,
}

impl AcquireSelect<'_, '_> {
    /// One ask's select of `unit`, owing what follows its signals, or `None` when it selects
    /// nothing.
    fn select(
        &mut self,
        unit: Entity,
        why: Acquire,
        my_guid: u64,
    ) -> Option<crate::script_calls::EngineSelect> {
        let guid = self.guids.get(unit).ok()?.0;
        match why {
            Acquire::Attacker => {
                // `0x6259e6` tests the selected guid, so a party member selected out of range
                // keeps the selection.
                if self.selection.guid.is_some() {
                    return None;
                }
                debug!("auto-target: attacker {guid:#x} (victim = me, no target)");
            }
            Acquire::Caster => {
                let rel = self.relations.get();
                let assists = match (self.relations.unit(guid), self.relations.me()) {
                    (Some(caster), Some(player)) => rel.can_assist(&caster, &player),
                    _ => return None,
                };
                if assists || self.selection.guid.is_some() {
                    return None;
                }
                debug!("auto-target: caster {guid:#x} (casting at me, no target)");
            }
        }
        if self.front.takes(unit, guid, self.seam.slots.targeting()) {
            return None;
        }
        let before = super::click::Standing::of(&self.selection, &self.seam);
        // The selection was empty, so no switch law: selection only.
        let (_, owed) = commit_owing(
            &mut self.selection,
            &mut self.seam,
            Some(unit),
            guid,
            self.stores.get(unit).ok(),
            Some(my_guid),
            false,
            false,
        );
        Some(before.owing(owed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::NetCommands;
    use crate::net::Reputations;

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
        engage(world, engaged);
        world
            .run_system_once(
                move |mut selection: ResMut<Selection>,
                      mut seam: crate::creature_anim::AttackSeam| {
                    commit(
                        &mut selection,
                        &mut seam,
                        Some(Entity::PLACEHOLDER),
                        guid,
                        store.as_ref(),
                        self_guid,
                        attackable,
                        true,
                    )
                },
            )
            .expect("commit runs as a one-shot system")
    }

    /// Our attack target set (`[+0xc48]`) or clear, with whatever the last commit queued applied.
    pub(super) fn engage(world: &mut World, engaged: bool) {
        crate::creature_anim::apply_unit_edits(world);
        let me = world
            .query_filtered::<Entity, With<SelfPlayer>>()
            .single(world)
            .expect("our own player");
        if engaged {
            world
                .entity_mut(me)
                .insert(crate::creature_anim::Engaged(0));
        } else {
            world
                .entity_mut(me)
                .remove::<crate::creature_anim::Engaged>();
        }
    }

    /// A world with just what the seams need.
    fn commit_world() -> (World, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::PendingCast>();
        world.init_resource::<crate::spell::SpellTargeting>();
        world.init_resource::<crate::ui_cast::CastReports>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<crate::creature_anim::AttackStopInFlight>();
        world.init_resource::<crate::creature_anim::UnitEdits>();
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
                    super::super::click::clear(&mut selection, &mut seam);
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

        // Off ourselves the latch is off (`CanAttack` refuses us, `0x4936a3`): stop, select.
        let out = go(&mut world, 0xC, true, Some(1), true);
        assert!(out.changed && !out.swung);
        assert_eq!(drain(&rx), ["stop", "select"]);

        // Engaged switch onto an unattackable unit (vendor/corpse): stop, select, and StartAttack's
        // refusal arm stops again (`0x5ecc37` → `0x5ecc48`), no swing at it.
        let out = go(&mut world, 0xD, true, Some(1), false);
        assert!(out.changed && !out.swung);
        assert_eq!(drain(&rx), ["stop", "select", "stop"]);

        // Engaged first select, no old target: the latch is off, selection only.
        *world.resource_mut::<Selection>() = Selection::default();
        let out = go(&mut world, 0xE, true, Some(1), true);
        assert!(out.changed && !out.swung);
        assert_eq!(drain(&rx), ["select"]);

        // A queued strike and auto-repeat: the stop un-queues one, the re-swing ends the other.
        world
            .resource_mut::<crate::spell::QueuedMeleeSpell>()
            .arm(78);
        world
            .resource_mut::<crate::spell::PendingCast>()
            .fill_record();
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
        // `CancelQueuedCast`'s cancel (`0x6e6f68`): the pop's signal, then `SPELLCAST_STOP`; the
        // re-swing's repeat stop signals `STOP_AUTOREPEAT_SPELL` as the key clears (`0x6ea16a`).
        assert!(matches!(
            world.resource::<crate::ui_cast::CastReports>().reports()[..],
            [
                crate::ui_cast::CastReport::Changed,
                crate::ui_cast::CastReport::Bar(crate::ui_cast::CastBarEdge::Stop),
                crate::ui_cast::CastReport::AutoRepeat(false)
            ]
        ));
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
        let me = RelUnit::of(1, &unit(&[(TPL, 1), (FLAGS, 1 << 3)]));
        let rel = Relations {
            factions: None,
            reputations: &reps,
            me: Some(1),
            roster: None,
            units: &crate::target::relations::NoUnits,
        };
        let valid = |s: &ObjectStore| attack_target_valid(&rel, Some(&me), &RelUnit::of(2, s), s);

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
        // No player is refused.
        let mob = unit(&[(HEALTH, 120)]);
        assert!(!attack_target_valid(
            &rel,
            None,
            &RelUnit::of(2, &mob),
            &mob
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

    /// Modes 3 and 4 (`0x493eed`, `0x493f15`): the party slots, then party or raid roster, with no
    /// health or reaction leg, so a dead party member counts and a stranger never does.
    #[test]
    fn the_party_and_raid_modes_take_only_the_roster() {
        use benilla_protocol::messages::GroupMemberEntry;
        use bevy::ecs::system::RunSystemOnce;

        const PARTY: u64 = 0x11;
        const PARTY_DEAD: u64 = 0x12;
        const RAID: u64 = 0x13;
        const STRANGER: u64 = 0x14;
        let mut world = World::new();
        world.init_resource::<Reputations>();
        world.init_resource::<NameCache>();
        world.spawn((
            SelfPlayer,
            Transform::default(),
            Guid(1),
            store(&[(F_HEALTH, 100)]),
        ));
        for (guid, x, health) in [
            (PARTY, 5.0, 100),
            (PARTY_DEAD, 6.0, 0),
            (RAID, 7.0, 100),
            (STRANGER, 3.0, 100),
        ] {
            world.spawn((
                NetEntity {
                    kind: EntityKind::Player,
                    display_id: None,
                    scale: 1.0,
                },
                Guid(guid),
                Transform::from_xyz(x, 0.0, 0.0),
                store(&[(F_HEALTH, health), (F_MAXHEALTH, 100)]),
            ));
        }
        let member = |guid, flags| GroupMemberEntry {
            name: String::new(),
            guid,
            status: 1,
            flags,
        };
        // A raid: our subgroup 0 holds PARTY and PARTY_DEAD; RAID is in subgroup 1.
        let mut group = crate::ui_party::GroupState {
            in_group: true,
            group_type: 1,
            members: vec![member(PARTY, 0), member(PARTY_DEAD, 0), member(RAID, 1)],
            ..Default::default()
        };
        group.rebuild_raid(0x5E1F);
        world.insert_resource(group);
        let pool = |world: &mut World, side: ScanSide| {
            world
                .run_system_once(move |scan: TargetScan| {
                    scan.build(side).iter().map(|c| c.guid).collect::<Vec<_>>()
                })
                .expect("the scan runs as a one-shot system")
        };
        assert_eq!(pool(&mut world, ScanSide::Party), [PARTY, PARTY_DEAD]);
        assert_eq!(pool(&mut world, ScanSide::Raid), [PARTY, PARTY_DEAD, RAID]);
        // Out of a raid, mode 4's raid half has no roster: it is the party test.
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .raid
            .clear();
        assert_eq!(pool(&mut world, ScanSide::Raid), [PARTY, PARTY_DEAD]);
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
            crate::net::Guid(0x1),
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

        // As `SetSelection` stores the guid and lights the object it found (`0x493701`).
        let select = |world: &mut World, target: Option<(Entity, u64)>| {
            let mut sel = world.resource_mut::<Selection>();
            sel.target = target.map(|(e, _)| e);
            sel.guid = target.map(|(_, g)| g);
            sel.lit = sel.target;
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
