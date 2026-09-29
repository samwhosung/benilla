//! The targeting cursor: a cast waiting for the click that binds its target. The reference's
//! targeting mode is a nonzero flag_word (`IsTargeting 0x6e48a0`); [`SpellTargeting`] holds that
//! word and each click seam asks it its own mask test ([`TargetingWants`]).
//!
//! - [`cursor`]: the hover verdict per seam. Terrain is `0x4820f0`'s `CheckGroundPointInRange
//!   0x6e6810`, a GameObject, a unit or a corpse is `0x4828d0`'s `0x6e6460` (the spell-vs-lock
//!   predicate `0x5f8260`, [`SpellTargeting::can_target_unit`] or
//!   [`SpellTargeting::can_target_corpse`], then range), and a word no seam handles is UnableCast.
//! - [`world`]: the world-click dispatcher `0x492ce0`, its terrain leg (`0x492580` → `BindLocation
//!   0x6e60f0`) and its object leg (`0x4925d0` → `SetSelection 0x493540` → `BindTarget 0x6e5b40`),
//!   whose unit arm [`bind_target_unit`] shares with `SpellTargetUnit`
//!   ([`ScriptCursor::spell_target_unit`]), and whose corpse arm is [`corpse`]'s.
//! - [`item`]: the bag click (`PickupContainerItem 0x4f9b30`) and the paper-doll click
//!   (`0x4c7300`), both `IsTargeting`, `TargetingWantsItem 0x6e6330`, then `0x495d60`, whose
//!   confirm popups park the clicked guid (`0xb4e3c0`) with the word still standing.
//!
//! Every seam commits through [`crate::spell::CastLadder::commit_targeted`] (`SendCast 0x6e54f0`).
//! Only the unit and corpse arms have a range gate ([`merge`]): the terrain and GameObject clicks
//! send and the server judges range, and `CheckGroundPointInRange` only colours the cursor. While
//! targeting, the pick flags come from the word alone, so a click over a unit with a dest-only
//! word commits on the ground behind it.
//!
//! Cancels: ESC through `UIParent.lua:1490` ([`feed_targeting_to_vm`], [`ScriptCursor::stop_targeting`]),
//! the right-button down edge ([`cancel_targeting_on_right_press`]), a new spell's press, which
//! aborts and proceeds (`TryCast 0x6e4b60` at `0x6e4d62`), and the bar's re-press of the same
//! spell (`UseAction 0x4e5ee0`). A cancel clears the word and sends nothing; movement never
//! cancels (`0x515090`).

mod corpse;
mod cursor;
mod item;
mod world;

#[cfg(test)]
pub(crate) use corpse::fixture as corpse_fixture;
pub(crate) use corpse::{corpse_pick_admits, publish_corpse_pick, CorpsePick};
pub(crate) use cursor::{drive_targeting_cursor, ground_cast_radius};
pub(crate) use item::{commit_item_cast_on_pick, EnchantConfirmItem};
pub(crate) use world::{commit_ground_cast_on_click, commit_object_cast_on_click};

use bevy::prelude::*;

use benilla_world::interact::WorldRightPress;

/// The inputs of `BindTarget 0x6e5b40`'s unit and corpse arms and of their read-only mirror
/// `0x6e6460` ([`SpellTargeting::can_target_unit`], [`bind_target_unit`],
/// [`SpellTargeting::can_target_corpse`], [`corpse::bind_target_corpse`]): the gates and relation
/// checks the cast arm's selection bind runs ([`super::cast_target::unit_binds`]), the corpse's two
/// facts, and the pre-send range gate's inputs ([`super::cast_target::RangeInputs`]), read as
/// [`super::cast_target::CastTargeting`] reads them.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BindChecks<'w, 's> {
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
    index: Option<Res<'w, crate::net::GuidIndex>>,
    self_q: Query<
        'w,
        's,
        (Entity, Option<&'static crate::net::ObjectStore>),
        With<crate::net::SelfPlayer>,
    >,
    factions: Option<Res<'w, crate::target::Factions>>,
    reputations: Res<'w, crate::net::Reputations>,
    names: Option<Res<'w, crate::names::NameCache>>,
    /// The range leg's positions: the pose the hover picks against, as last frame propagated it,
    /// so the VM feed, the cursor and both binds read one pose. Never the camera's, which
    /// `publish_camera_pose` rewrites mid-frame.
    poses: Query<'w, 's, &'static GlobalTransform, Without<benilla_world::view::WorldCamera>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    spell_mods: Res<'w, super::SpellModifiers>,
}

impl BindChecks<'_, '_> {
    fn is_self(&self, entity: Entity) -> bool {
        self.self_q
            .iter()
            .next()
            .is_some_and(|(me, _)| me == entity)
    }

    /// `BindTarget 0x6e5b40`'s unit branch: whether this unit binds the word under this spell,
    /// its gates and then its relation checks ([`super::cast_target::unit_binds`]). An unknown
    /// spell has no row to gate on.
    fn unit_binds(&self, spell_id: u32, word: u16, entity: Entity) -> bool {
        let target_store = self.stores.get(entity).ok();
        let target_owner_store = target_store
            .and_then(|store| {
                store
                    .0
                    .unit_owner(benilla_protocol::messages::OwnerFallback::CreatedBy)
            })
            .and_then(|guid| self.index.as_ref()?.0.get(&guid).copied())
            .and_then(|owner| self.stores.get(owner).ok());
        let rel = super::cast_target::TargetRelations {
            target_store,
            target_owner_store,
            self_store: self.self_q.iter().next().and_then(|(_, store)| store),
            factions: self.factions.as_deref(),
            reputations: &self.reputations,
            types: crate::creature_type::CreatureTypeSources {
                names: self.names.as_deref(),
                forms: self.spells.as_deref().map(|s| &s.forms),
            },
        };
        let def = self.spells.as_deref().and_then(|s| s.catalog.get(spell_id));
        super::cast_target::unit_binds(def, word, self.is_self(entity), &rel)
    }

    /// The spell's min/max range against this unit, through the one compare the pre-send gate
    /// asks ([`super::cast_target::RangeInputs::refusal`]); our own body is distance 0. An
    /// unknown spell or a missing row passes.
    fn range_refusal(&self, spell_id: u32, entity: Entity) -> Option<u8> {
        let mut range = self.range_inputs(entity);
        range.target_reach = self
            .stores
            .get(entity)
            .ok()
            .map(|s| s.0.unit_combat_reach());
        self.refusal(spell_id, range)
    }

    /// The same compare against a corpse. `GetMinMaxRange 0x6e3480` pads a corpse's bounds as a
    /// unit's (`6e35fe`), but with no unit to read the second reach from it reads the caster's
    /// again (`6e3605`–`6e361e`).
    fn corpse_range_refusal(&self, spell_id: u32, entity: Entity) -> Option<u8> {
        let mut range = self.range_inputs(entity);
        range.target_reach = Some(range.self_reach);
        self.refusal(spell_id, range)
    }

    /// Our position and reach and the candidate's position, each position the pose the hover
    /// picks against.
    fn range_inputs(&self, entity: Entity) -> super::cast_target::RangeInputs {
        let me = self.self_q.iter().next();
        let mut range = super::cast_target::RangeInputs {
            self_pos: me
                .and_then(|(e, _)| self.poses.get(e).ok())
                .map(GlobalTransform::translation),
            target_pos: self
                .poses
                .get(entity)
                .ok()
                .map(GlobalTransform::translation),
            ..Default::default()
        };
        if let Some((_, Some(store))) = me {
            range.self_reach = store.0.unit_combat_reach();
        }
        range
    }

    fn refusal(&self, spell_id: u32, range: super::cast_target::RangeInputs) -> Option<u8> {
        let spells = self.spells.as_deref()?;
        let def = spells.catalog.get(spell_id)?;
        range.refusal(def, spells.ranges.get(def.range_index), &self.spell_mods)
    }

    /// What the corpse legs read of this corpse: `CORPSE_FLAG_BONES` and the reaction gate
    /// `0x6067d0`. A corpse whose store has not streamed binds nothing.
    fn corpse_facts(&self, entity: Entity) -> Option<corpse::CorpseFacts> {
        Some(corpse::CorpseFacts::of(
            self.stores.get(entity).ok()?,
            self.factions.as_deref(),
            self.self_q.iter().next().and_then(|(_, s)| s),
        ))
    }
}

/// A click seam's mask test on the flag_word `0xcecac0`, one per reference predicate:
///
/// - `Location`: `TargetingWantsLocation 0x6e6320`, `word & 0x60`, the terrain click.
/// - `Item`: `TargetingWantsItem 0x6e6330`, `word & 0x4010`, the bag and paper-doll clicks.
/// - `GameObject`: `TargetingWantsGameObject 0x6e62d0`, `word & 0x4800`, the world object click.
/// - `Unit`: `SpellCanTargetUnit 0x6e6460`, the unit arm of the world click.
/// - `Corpse`: `0x6e6230`, `word & 0x8600`, the pick's corpse flag `0x40` ([`corpse`]).
///
/// The masks overlap on `TARGET_FLAG_LOCKED`, so a lock spell answers both the item and the
/// GameObject seam; the reference settles it only at the click, where `BindTarget 0x6e5b40` picks
/// its arm by the clicked object's typemask.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TargetingWants {
    Location,
    Item,
    GameObject,
    Unit,
    Corpse,
}

/// The unit-shaped bits of the flag_word, the first gate `SpellCanTargetUnit` tests before it runs
/// the candidate through the unit arm's relation and liveness checks.
const UNIT_WORD_BITS: u16 = 0x0002 | 0x0004 | 0x0008 | 0x0080 | 0x0100 | 0x0200 | 0x0400 | 0x8000;

impl TargetingWants {
    fn matches(self, word: u16) -> bool {
        let mask = match self {
            Self::Location => 0x0060,
            Self::Item => 0x4010,
            Self::GameObject => 0x4800,
            Self::Unit => UNIT_WORD_BITS,
            Self::Corpse => 0x8600,
        };
        word & mask != 0
    }
}

/// The word bits `0x6e61a0` lets the world pick take the local player for: unit, raid, party,
/// assist, the explicit gate and the ally corpse, never enemy (`0x80`) or enemy corpse (`0x200`).
const SELF_PICK_BITS: u16 = 0x850e;

/// The targeting mode, `Some` while the flag_word is nonzero. Entered by the cast-send path
/// ([`super::cast_target::CastWireTarget::Targeting`]), cleared by a commit or a cancel.
#[derive(Resource, Default)]
pub(crate) struct SpellTargeting(Option<Targeting>);

struct Targeting {
    spell_id: u32,
    /// The whole pending cast survives the cursor, the cast item's guid (`0xceac48`) included, so
    /// `0x6e54f0` still sends `CMSG_USE_ITEM` for a grenade, a poison bottle or a key.
    commit: super::cast_send::CastCommit,
    /// The standing flag_word `0xcecac0`; more than one seam can answer yes to it.
    word: u16,
}

impl SpellTargeting {
    /// `IsTargeting 0x6e48a0`.
    pub(crate) fn active(&self) -> bool {
        self.0.is_some()
    }

    /// `GetTargetingSpellId 0x6e48e0`, whatever the word wants: for the bar's checked state, the
    /// re-press toggle and the `CURRENT_SPELL_CAST_CHANGED` edge. A seam reads [`Self::spell_for`].
    pub(crate) fn spell(&self) -> Option<u32> {
        self.0.as_ref().map(|t| t.spell_id)
    }

    /// Whether the standing word answers `wants`' mask test.
    pub(crate) fn wants(&self, wants: TargetingWants) -> bool {
        self.0.as_ref().is_some_and(|t| wants.matches(t.word))
    }

    /// `SpellCanTargetUnit`'s predicate `0x6e6460`, its unit leg, with the range flag both callers
    /// pass (the world hover `0x4828d0` at `48290b`, the Lua `SpellCanTargetUnit 0x6e6d00`):
    /// `BindTarget`'s gates and relation checks (`6e6507`–`6e65ab`), then min² ≤ d² ≤ max²
    /// (`6e677c`–`6e6802`), so a unit inside the minimum is out too. There is no line-of-sight
    /// test.
    pub(crate) fn can_target_unit(&self, entity: Entity, checks: &BindChecks) -> bool {
        self.0.as_ref().is_some_and(|t| {
            checks.unit_binds(t.spell_id, t.word, entity)
                && checks.range_refusal(t.spell_id, entity).is_none()
        })
    }

    /// `0x6e61a0`: whether the world pick takes the local player, the pick flag `0x20` that
    /// `0x480610` tests at `48062c`. Never outside targeting (`480638`); while targeting, a word in
    /// `0x850e` whose spell lacks `AttributesEx & 0x80000` (`6e61cf`). An unknown spell passes.
    pub(crate) fn picks_self(&self, spells: Option<&crate::ui_action::Spells>) -> bool {
        self.0.as_ref().is_some_and(|t| {
            t.word & SELF_PICK_BITS != 0
                && !spells
                    .and_then(|s| s.catalog.get(t.spell_id))
                    .is_some_and(|d| d.excludes_caster())
        })
    }

    /// The pending cast and its standing word, whatever seam it wants.
    fn pending(&self) -> Option<(u32, super::cast_send::CastCommit, u16)> {
        self.0.as_ref().map(|t| (t.spell_id, t.commit, t.word))
    }

    /// The pending spell when the standing word answers `wants`. A surface that draws or binds for
    /// one seam reads this, never [`Self::spell`], or an armed lockpick gets an AoE reticle.
    pub(crate) fn spell_for(&self, wants: TargetingWants) -> Option<u32> {
        self.0
            .as_ref()
            .filter(|t| wants.matches(t.word))
            .map(|t| t.spell_id)
    }

    pub(crate) fn enter(&mut self, spell_id: u32, commit: super::cast_send::CastCommit, word: u16) {
        self.0 = Some(Targeting {
            spell_id,
            commit,
            word,
        });
    }

    /// The pending cast when the standing word answers `wants`, so no commit fires on a word its
    /// seam cannot bind.
    fn pending_for(&self, wants: TargetingWants) -> Option<(u32, super::cast_send::CastCommit)> {
        self.0
            .as_ref()
            .filter(|t| wants.matches(t.word))
            .map(|t| (t.spell_id, t.commit))
    }

    /// `BindLocation 0x6e60f0`'s fork: SOURCE (`0x20`) is tested before DEST (`0x40`). The
    /// reference takes a second click for a word carrying both; no 1.12 spell does, so only the
    /// precedence is built.
    fn location_bind(&self, point: [f32; 3]) -> Option<super::cast_send::TargetedBind> {
        let word = self.0.as_ref()?.word;
        if word & 0x0020 != 0 {
            Some(super::cast_send::TargetedBind::Source(point))
        } else if word & 0x0040 != 0 {
            Some(super::cast_send::TargetedBind::Dest(point))
        } else {
            None
        }
    }

    pub(crate) fn clear(&mut self) {
        self.0 = None;
    }
}

/// Right-click cancels on the down edge (WorldFrame `OnMouseDown 0x483c40` → `0x492c20` →
/// `StopTargeting 0x6e4900`), sends nothing and consumes nothing: the press still turns the camera.
/// A held cursor payload pre-empts it (`0x492b50`), and a press over a UI frame never reaches it.
/// Whether a UI-frame right-click also cancels in the reference is untraced. The rotate-placement
/// skip on flag `0xceca90` (effect `0x51`) is not built.
pub(crate) fn cancel_targeting_on_right_press(
    mut presses: MessageReader<WorldRightPress>,
    payload_held: Res<crate::ui_script::CursorPayloadHeld>,
    mut targeting: ResMut<SpellTargeting>,
) {
    if !targeting.active() {
        // A press buffered while idle must not replay as a cancel once the mode turns on.
        presses.clear();
        return;
    }
    if presses.read().last().is_none() || payload_held.0 {
        return;
    }
    debug!("ui_action: targeting cancelled (right-click)");
    targeting.clear();
}

/// [`SpellTargeting::picks_self`] as the frame began, which the world pick reads
/// ([`crate::target`]'s hover): the word the VM's `SpellIsTargeting` answers from, in a declared
/// order against every cast drain that arms or ends the cursor after the input pass.
#[derive(Resource, Default)]
pub(crate) struct PicksSelf(pub(crate) bool);

/// Publish [`PicksSelf`] before the input pass.
pub(crate) fn publish_picks_self(
    targeting: Res<SpellTargeting>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut picks: ResMut<PicksSelf>,
) {
    let now = targeting.picks_self(spells.as_deref());
    if picks.0 != now {
        picks.0 = now;
    }
}

/// Push the targeting state into the VM each frame, before the input pass, so a word armed last
/// frame stands for this frame's clicks: the ESC chain's word, the bag and doll pickup reroute's
/// item half, and `CURRENT_SPELL_CAST_CHANGED` on each edge. That event hides the enchant confirm
/// popups (`UIParent.lua:449`); its one emitter `0x4b3250` is called from the cast arm, abort and
/// bind sites, `StopTargeting 0x6e4900` among them. It fires on the spell changing, so a cancel
/// and re-arm in one frame still counts.
pub(crate) fn feed_targeting_to_vm(
    targeting: Res<SpellTargeting>,
    checks: BindChecks,
    tokens: crate::ui_unit::UnitTokens,
    selection: Res<crate::target::Selection>,
    mut last: Local<crate::ui_script::VmMemo<Option<u32>>>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
) {
    if let Some(mut script) = script {
        let last = last.get(&script);
        script.set_spell_targeting(targeting.active());
        script.set_item_pick_armed(targeting.wants(TargetingWants::Item));
        // `SpellCanTargetUnit(unit)`: resolve each token and ask `0x6e6460`'s unit leg.
        script.set_spell_targetable_units(crate::ui_unit::reach_tokens().filter(|token| {
            tokens
                .resolve(token, &selection)
                .is_some_and(|(entity, _)| targeting.can_target_unit(entity, &checks))
        }));
        if *last != targeting.spell() {
            *last = targeting.spell();
            script.fire_event("CURRENT_SPELL_CAST_CHANGED", vec![]);
        }
    }
}

/// `BindTarget 0x6e5b40`'s unit arm, for the world click (`0x493540` at `4935d5`) and
/// `SpellTargetUnit` (`0x6e5b10`): a unit the gates or the relation checks refuse binds nothing
/// and the cursor stays up; otherwise the merge. The player's selection never moves.
fn bind_target_unit(
    ladder: &mut crate::spell::CastLadder,
    checks: &BindChecks,
    entity: Entity,
    guid: u64,
) {
    let Some((spell_id, commit, word)) = ladder.ground.pending() else {
        return;
    };
    if !checks.unit_binds(spell_id, word, entity) {
        return;
    }
    let range = checks.range_refusal(spell_id, entity);
    merge(
        ladder,
        spell_id,
        commit,
        range,
        super::cast_send::TargetedBind::Unit(guid),
    );
}

/// `BindTarget`'s merge (`6e602c`–`6e60d7`): a bind whose mask is in `0x8202`, a unit or a corpse,
/// runs the range test `0x6e47b0` (`6e6063`). Out of range raises "Out of range." or "Target too
/// close" and binds nothing; otherwise the emptied word commits (`SendCast 0x6e54f0`). The
/// reference's range failure also aborts the cast (`6e6089`), which sends CMSG_CANCEL_CAST and
/// strands the cursor (`6e5b93`) until a cancel: a bug, not copied; targeting stays armed and
/// working.
fn merge(
    ladder: &mut crate::spell::CastLadder,
    spell_id: u32,
    commit: super::cast_send::CastCommit,
    range: Option<u8>,
    bound: super::cast_send::TargetedBind,
) {
    if let Some(reason) = range {
        debug!("ui_action: cast {spell_id} not bound at {bound:x?} — range ({reason:#x})");
        ladder.cast_errors.push_local(spell_id, reason);
        return;
    }
    debug!("ui_action: cast {spell_id} committed at {bound:x?}");
    ladder.commit_targeted(spell_id, commit, bound);
}

/// The targeting cursor's script calls, applied in call order by [`crate::script_calls`].
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ScriptCursor<'w, 's> {
    tokens: crate::ui_unit::UnitTokens<'w, 's>,
    selection: Res<'w, crate::target::Selection>,
    checks: BindChecks<'w, 's>,
    ladder: crate::spell::CastLadder<'w, 's>,
}

impl ScriptCursor<'_, '_> {
    /// `SpellTargetUnit(unit)` (`0x6e6d90`). The binding already raised the usage and
    /// unknown-token errors and dropped a call made while not targeting. Here a token that names
    /// no unit raises "Out of range." (0x59) and ends targeting, as the reference's abort clears
    /// the word, and a unit goes to [`bind_target_unit`].
    pub(crate) fn spell_target_unit(&mut self, token: &str) {
        // An earlier call may have bound or ended the cast: not targeting, no-op.
        let Some(spell_id) = self.ladder.ground.spell() else {
            return;
        };
        match self.tokens.resolve(token, &self.selection) {
            Some((entity, guid)) => bind_target_unit(&mut self.ladder, &self.checks, entity, guid),
            None => {
                debug!("ui_action: SpellTargetUnit({token}) names no unit — cast {spell_id} ends");
                self.ladder
                    .cast_errors
                    .push_local(spell_id, super::validator::ERR_OUT_OF_RANGE);
                self.ladder.ground.clear();
            }
        }
    }

    /// The ESC chain's `SpellStopTargeting()` (`0x6e6e30`): `StopTargeting 0x6e4900`, word
    /// cleared, no packet.
    pub(crate) fn stop_targeting(&mut self) {
        debug!("ui_action: targeting cancelled (ESC chain)");
        self.ladder.ground.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{ClientCommand, Guid, NetCommands, ObjectStore, SelfPlayer};
    use benilla_ui::script::UiScript;
    use crossbeam_channel::Receiver;
    use std::collections::HashMap;

    const HEAL: u32 = 2050;
    /// A spell that excludes its caster (`AttributesEx & 0x80000`).
    const NOT_SELF: u32 = 2051;
    /// A spell on row 114, 8 to 35 yd, which the reaches pad to 11 to 38.
    const BANDED: u32 = 75;
    const ME: u64 = 0x10;
    const ALLY: u64 = 0xF130_0000_0000_0001;

    /// The cast ladder, the unit-token resolver and a VM, with us at the origin and an ally
    /// selected at `distance`. [`HEAL`] and [`NOT_SELF`] use row 5, 0 to 30 yd, which both 1.5
    /// combat reaches pad to 33.
    fn unit_world(distance: f32) -> (World, Receiver<ClientCommand>, Entity) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::items::Items>();
        world.init_resource::<crate::net::GuidIndex>();
        world.insert_resource(crate::net::Reputations(Vec::new()));
        world.init_resource::<crate::spell::PendingCast>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::Cooldowns>();
        world.init_resource::<crate::spell::SpellModifiers>();
        world.init_resource::<crate::ui_action::CastErrors>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<crate::ui_tradeskill::TradeSkillOpens>();
        world.init_resource::<SpellTargeting>();
        world.init_resource::<crate::spell::HeldForPick>();
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<crate::ui_party::GroupState>();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        let row5 = |attributes_ex| benilla_formats::SpellDisplay {
            range_index: 5,
            attributes_ex,
            ..Default::default()
        };
        spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([
            (HEAL, row5(0)),
            (NOT_SELF, row5(0x0008_0000)),
            (
                BANDED,
                benilla_formats::SpellDisplay {
                    range_index: 114,
                    ..Default::default()
                },
            ),
        ]));
        let row = |min, max| benilla_formats::SpellRange { min, max, flags: 0 };
        spells.ranges = benilla_formats::SpellRangeCatalog::from_rows(HashMap::from([
            (5, row(0.0, 30.0)),
            (114, row(8.0, 35.0)),
        ]));
        world.insert_resource(spells);
        // `UNIT_FIELD_HEALTH` 100: a store with no health reads dead to `BindTarget`'s gates.
        let live = || ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(22, 100)]));
        world.spawn((SelfPlayer, Guid(ME), GlobalTransform::default(), live()));
        let ally = world
            .spawn((
                Guid(ALLY),
                GlobalTransform::from_translation(Vec3::new(distance, 0.0, 0.0)),
                live(),
            ))
            .id();
        world.insert_resource(crate::target::Selection {
            target: Some(ally),
            guid: Some(ALLY),
            ..Default::default()
        });
        let mut script = UiScript::new().expect("a VM");
        script.set_spell_targeting(true);
        world.insert_non_send_resource(script);
        (world, rx, ally)
    }

    fn arm(world: &mut World, spell: u32, word: u16) {
        world.resource_mut::<SpellTargeting>().enter(
            spell,
            super::super::cast_send::CastCommit::Spell,
            word,
        );
    }

    /// Queue `SpellTargetUnit(token)` in the VM, then apply it.
    fn spell_target_unit(world: &mut World, token: &str) {
        world
            .non_send_resource_mut::<UiScript>()
            .run(&format!("SpellTargetUnit({token:?})"))
            .expect("a known token");
        world
            .run_system_cached(
                |mut script: NonSendMut<UiScript>, mut cursor: ScriptCursor| {
                    for token in script.take_spell_target_unit() {
                        cursor.spell_target_unit(&token);
                    }
                },
            )
            .expect("the applier runs");
    }

    fn errors(world: &mut World) -> Vec<crate::ui_action::CastFail> {
        std::mem::take(&mut world.resource_mut::<crate::ui_action::CastErrors>().0)
    }

    /// `0x6e6d90` past the binding's checks: a token naming no unit ends the cast with "Out of
    /// range."; a unit goes to `BindTarget`, which waits on a relation failure, raises "Out of
    /// range." for a unit out of range with the cursor still up, and commits otherwise. The
    /// selection never moves.
    #[test]
    fn spell_target_unit_ends_waits_or_binds_as_the_reference() {
        let fail = |reason| vec![crate::ui_action::CastFail::local(HEAL, reason)];

        // The VM queued a token, but the host is no longer targeting: nothing.
        let (mut world, rx, _) = unit_world(10.0);
        spell_target_unit(&mut world, "target");
        assert!(rx.try_recv().is_err());
        assert!(errors(&mut world).is_empty());

        // A known token naming no unit: "Out of range." and targeting ends.
        let (mut world, rx, _) = unit_world(10.0);
        arm(&mut world, HEAL, 0x0002);
        spell_target_unit(&mut world, "party1");
        assert!(rx.try_recv().is_err(), "no send");
        assert_eq!(errors(&mut world), fail(0x59));
        assert!(
            !world.resource::<SpellTargeting>().active(),
            "the cast ends"
        );

        // A unit the relation refuses (assist, neutral with no catalog): silent, still armed.
        let (mut world, rx, _) = unit_world(10.0);
        arm(&mut world, HEAL, 0x0100);
        spell_target_unit(&mut world, "target");
        assert!(rx.try_recv().is_err());
        assert!(errors(&mut world).is_empty(), "no error");
        assert!(world.resource::<SpellTargeting>().active(), "still armed");

        // Ourselves under a spell that excludes its caster: silent, still armed.
        let (mut world, rx, _) = unit_world(10.0);
        arm(&mut world, NOT_SELF, 0x0002);
        spell_target_unit(&mut world, "player");
        assert!(
            rx.try_recv().is_err(),
            "never ourselves under AttributesEx 0x80000"
        );
        assert!(errors(&mut world).is_empty(), "no error");
        assert!(world.resource::<SpellTargeting>().active(), "still armed");

        // In relation but out of range: "Out of range." and still armed.
        let (mut world, rx, _) = unit_world(40.0);
        arm(&mut world, HEAL, 0x0002);
        spell_target_unit(&mut world, "target");
        assert!(rx.try_recv().is_err());
        assert_eq!(errors(&mut world), fail(0x59));
        assert!(world.resource::<SpellTargeting>().active(), "still armed");

        // In range: the cast goes to the unit and the cursor comes down.
        let (mut world, rx, ally) = unit_world(10.0);
        arm(&mut world, HEAL, 0x0002);
        spell_target_unit(&mut world, "target");
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell {
                spell_id: HEAL,
                target: Some(ALLY),
            })
        ));
        assert!(errors(&mut world).is_empty());
        assert!(!world.resource::<SpellTargeting>().active());
        let selection = world.resource::<crate::target::Selection>();
        assert_eq!((selection.target, selection.guid), (Some(ally), Some(ALLY)));
    }

    /// The press (`resolve_cast_target`), the click and `SpellTargetUnit` ([`bind_target_unit`]) and
    /// the hover verdict (`SpellCanTargetUnit`) all ask [`crate::spell::cast_target::unit_binds`],
    /// so one table of units decides all three: `BindTarget`'s gates refuse the same units at
    /// each, and the control row beside each refusal binds at each.
    #[test]
    fn the_press_the_click_and_the_hover_bind_the_same_units() {
        use crate::names::{CreatureRecord, NameCache};
        use crate::spell::cast_target::{
            cast_target_mask, resolve_cast_target, CastCandidates, CastWireTarget, TargetRelations,
        };
        use benilla_formats::SpellDisplay;
        use benilla_protocol::ObjectFields;

        const SPELL: u32 = 3000;
        /// Absolute descriptor indices: `OBJECT_FIELD_ENTRY`, `UNIT_FIELD_HEALTH`,
        /// `UNIT_FIELD_FLAGS` and `UNIT_DYNAMIC_FLAGS`.
        const ENTRY: u16 = 3;
        const HEALTH: u16 = 22;
        const FLAGS: u16 = 46;
        const DYNAMIC_FLAGS: u16 = 143;
        const HUMANOID: u32 = 69;
        const BEAST: u32 = 70;

        let make_names = || {
            let mut names = NameCache::default();
            for (entry, creature_type) in [(HUMANOID, 7), (BEAST, 1)] {
                names.insert_creature(
                    entry,
                    Some(CreatureRecord {
                        name: String::new(),
                        subname: None,
                        creature_type,
                        pet_family: 0,
                        rank: 0,
                        type_flags: 0,
                        civilian: false,
                        racial_leader: false,
                        display_id: 0,
                    }),
                );
            }
            names
        };
        fn plain() -> SpellDisplay {
            SpellDisplay {
                targets: 0x2,
                ..Default::default()
            }
        }
        fn hibernate() -> SpellDisplay {
            SpellDisplay {
                target_creature_type: 0x3,
                ..plain()
            }
        }
        fn excluding() -> SpellDisplay {
            SpellDisplay {
                attributes_ex: 0x0008_0000,
                ..plain()
            }
        }
        fn skinning() -> SpellDisplay {
            SpellDisplay {
                targets: 0x402,
                ..Default::default()
            }
        }
        fn raise() -> SpellDisplay {
            SpellDisplay {
                attributes_ex2: 1,
                ..plain()
            }
        }
        // (label, spell, the caster is the candidate, the candidate's fields, binds)
        #[allow(clippy::type_complexity)]
        let cases: Vec<(&str, fn() -> SpellDisplay, bool, Vec<(u16, u32)>, bool)> = vec![
            ("a live unit", plain, false, vec![(HEALTH, 100)], true),
            ("a dead unit", plain, false, vec![(HEALTH, 0)], false),
            ("no health streamed", plain, false, vec![], false),
            (
                "a dead unit, AttributesEx2 & 1",
                raise,
                false,
                vec![(HEALTH, 0)],
                true,
            ),
            (
                "a dead unit, the dynamic dead flag",
                plain,
                false,
                vec![(HEALTH, 0), (DYNAMIC_FLAGS, 0x20)],
                true,
            ),
            (
                "UNIT_FIELD_FLAGS & 0x10000",
                plain,
                false,
                vec![(HEALTH, 100), (FLAGS, 0x1_0000)],
                false,
            ),
            (
                "a Humanoid under Hibernate's mask",
                hibernate,
                false,
                vec![(HEALTH, 100), (ENTRY, HUMANOID)],
                false,
            ),
            (
                "a Beast under Hibernate's mask",
                hibernate,
                false,
                vec![(HEALTH, 100), (ENTRY, BEAST)],
                true,
            ),
            (
                "the caster, AttributesEx & 0x80000",
                excluding,
                true,
                vec![(HEALTH, 100)],
                false,
            ),
            (
                "another unit, AttributesEx & 0x80000",
                excluding,
                false,
                vec![(HEALTH, 100)],
                true,
            ),
            (
                "a living unit under the 0x400 word",
                skinning,
                false,
                vec![(HEALTH, 100)],
                false,
            ),
            (
                "a dead unit under the 0x400 word",
                skinning,
                false,
                vec![(HEALTH, 0)],
                true,
            ),
        ];

        for (label, make_def, on_self, fields, binds) in cases {
            let (mut world, rx, ally) = unit_world(10.0);
            let me = world
                .query_filtered::<Entity, With<SelfPlayer>>()
                .single(&world)
                .expect("the player");
            let (entity, guid, token) = if on_self {
                (me, ME, "player")
            } else {
                (ally, ALLY, "target")
            };
            let store = ObjectStore(ObjectFields::from_pairs(&fields));
            world.entity_mut(entity).insert(store.clone());
            let word = cast_target_mask(&make_def());
            let mut spells = world.resource_mut::<crate::ui_action::Spells>();
            spells.catalog = benilla_formats::SpellCatalog::from_displays(HashMap::from([(
                SPELL,
                SpellDisplay {
                    range_index: 5,
                    ..make_def()
                },
            )]));
            world.insert_resource(make_names());
            arm(&mut world, SPELL, word);

            // The hover verdict.
            world
                .run_system_cached(feed_targeting_to_vm)
                .expect("the feed runs");
            let hover = world
                .non_send_resource::<UiScript>()
                .eval::<bool>(&format!("return SpellCanTargetUnit({token:?}) == true"))
                .expect("a boolean");
            assert_eq!(hover, binds, "{label}: the hover verdict");

            // The click, through `SpellTargetUnit`.
            spell_target_unit(&mut world, token);
            let clicked = matches!(
                rx.try_recv(),
                Ok(ClientCommand::CastSpell {
                    spell_id: SPELL,
                    target: Some(sent),
                }) if sent == guid
            );
            assert_eq!(clicked, binds, "{label}: the cursor's click");
            assert_eq!(
                world.resource::<SpellTargeting>().active(),
                !binds,
                "{label}: a bind ends the cursor, a refusal leaves it up"
            );

            // The press, from the same fields.
            let (def, names) = (make_def(), make_names());
            let pressed = resolve_cast_target(
                Some(&def),
                &CastCandidates {
                    selection: Some(guid),
                    caster: Some(ME),
                    main_hand_item: None,
                },
                false,
                &TargetRelations {
                    target_store: Some(&store),
                    target_owner_store: None,
                    self_store: Some(&store),
                    factions: None,
                    reputations: &crate::net::Reputations(Vec::new()),
                    types: crate::creature_type::CreatureTypeSources {
                        names: Some(&names),
                        forms: None,
                    },
                },
            );
            assert_eq!(
                pressed == CastWireTarget::Unit(guid),
                binds,
                "{label}: the press ({pressed:?})"
            );
        }
    }

    /// `SpellCanTargetUnit` asks `0x6e6460`'s unit leg per token: nil out of range, inside the
    /// minimum included, and nil for ourselves under a spell that excludes its caster.
    #[test]
    fn spell_can_target_unit_is_nil_out_of_range() {
        let can = |distance: f32, spell: u32, token: &str| {
            let (mut world, _rx, _) = unit_world(distance);
            arm(&mut world, spell, 0x0002);
            world
                .run_system_cached(feed_targeting_to_vm)
                .expect("the feed runs");
            world
                .non_send_resource::<UiScript>()
                .eval::<bool>(&format!("return SpellCanTargetUnit({token:?}) == true"))
                .expect("a boolean")
        };
        assert!(can(10.0, HEAL, "target"), "in range");
        assert!(!can(40.0, HEAL, "target"), "out of range answers nil");
        assert!(can(40.0, HEAL, "player"), "ourselves at distance 0");
        assert!(
            !can(10.0, NOT_SELF, "player"),
            "never ourselves under AttributesEx 0x80000"
        );
        assert!(can(10.0, NOT_SELF, "target"));
        assert!(can(20.0, BANDED, "target"), "inside the band");
        assert!(
            !can(5.0, BANDED, "target"),
            "inside the minimum answers nil"
        );
    }

    /// `0x6e61a0`: the pick takes us only while targeting, for a word in `0x850e` whose spell
    /// lacks `AttributesEx & 0x80000`; [`PicksSelf`] carries it to the picker.
    #[test]
    fn the_pick_takes_us_only_for_a_friendly_word_that_admits_the_caster() {
        let (mut world, _rx, _) = unit_world(10.0);
        let picks = |world: &mut World| {
            world
                .run_system_cached(publish_picks_self)
                .expect("the flag publishes");
            world.resource::<PicksSelf>().0
        };
        world.init_resource::<PicksSelf>();
        assert!(!picks(&mut world), "never outside targeting");
        for word in [0x0002, 0x0004, 0x0008, 0x0100, 0x0400, 0x8000, 0x0102] {
            arm(&mut world, HEAL, word);
            assert!(picks(&mut world), "{word:#06x} takes us");
        }
        for word in [0x0080, 0x0200, 0x0040, 0x0010, 0x4800] {
            arm(&mut world, HEAL, word);
            assert!(!picks(&mut world), "{word:#06x} never takes us");
        }
        arm(&mut world, NOT_SELF, 0x0100);
        assert!(!picks(&mut world), "a spell that excludes its caster");
        world.resource_mut::<SpellTargeting>().clear();
        assert!(!picks(&mut world), "and off again when the cursor ends");
    }

    /// A terrain click while a poison is armed must not ship a DEST block for an item spell.
    #[test]
    fn each_click_seam_only_sees_a_word_it_can_bind() {
        let commit = super::super::cast_send::CastCommit::Spell;
        let mut t = SpellTargeting::default();
        assert_eq!(
            t.pending_for(TargetingWants::Location),
            None,
            "idle binds nothing"
        );
        assert!(!t.wants(TargetingWants::Item), "idle wants nothing");

        // Blizzard, the bare DEST word.
        t.enter(2120, commit, 0x0040);
        assert!(t.active());
        assert_eq!(
            t.pending_for(TargetingWants::Location),
            Some((2120, commit))
        );
        for seam in [TargetingWants::Item, TargetingWants::GameObject] {
            assert_eq!(
                t.pending_for(seam),
                None,
                "{seam:?} cannot commit a Blizzard"
            );
        }

        // Instant Poison, the bare ITEM word.
        t.enter(8679, commit, 0x0010);
        assert_eq!(t.pending_for(TargetingWants::Item), Some((8679, commit)));
        for seam in [TargetingWants::Location, TargetingWants::GameObject] {
            assert_eq!(t.pending_for(seam), None, "{seam:?} cannot commit a poison");
        }
        // The action bar's re-press toggle reads the spell whatever the seam.
        assert_eq!(t.spell(), Some(8679));

        t.clear();
        assert!(!t.active());
    }

    /// A lock word (`LOCKED` plus arm 23's `GAMEOBJECT`) is in both `0x4010` and `0x4800`, so
    /// whichever click lands first binds.
    #[test]
    fn a_lock_word_answers_both_the_bag_and_the_world_seam() {
        let commit = super::super::cast_send::CastCommit::Spell;
        let mut t = SpellTargeting::default();
        // Opening (3365): `Targets 0x4000` + implicit arm 23 ⇒ `0x4800`.
        t.enter(3365, commit, 0x4800);
        assert_eq!(t.pending_for(TargetingWants::Item), Some((3365, commit)));
        assert_eq!(
            t.pending_for(TargetingWants::GameObject),
            Some((3365, commit))
        );
        assert!(t.wants(TargetingWants::Item));
        assert!(t.wants(TargetingWants::GameObject));
        assert_eq!(
            t.pending_for(TargetingWants::Location),
            None,
            "a lock word still has no terrain leg"
        );

        // A bare GAMEOBJECT word (arm 23 over a `Targets`-less row) is the world seam's alone.
        t.enter(3365, commit, 0x0800);
        assert!(t.wants(TargetingWants::GameObject));
        assert!(!t.wants(TargetingWants::Item));
    }
}
