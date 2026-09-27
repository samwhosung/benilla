//! The targeting cursor: a cast waiting for the click that binds its target. The reference's
//! targeting mode is a nonzero flag_word (`IsTargeting 0x6e48a0`); [`SpellTargeting`] holds that
//! word and each click seam asks it its own mask test ([`TargetingWants`]).
//!
//! - [`cursor`]: the hover verdict per seam. Terrain is `0x4820f0`'s `CheckGroundPointInRange
//!   0x6e6810`, a GameObject or a unit is `0x4828d0`'s `0x6e6460` (the spell-vs-lock predicate
//!   `0x5f8260` or [`SpellTargeting::can_target_unit`], then range), and a word no seam handles is
//!   UnableCast.
//! - [`world`]: the world-click dispatcher `0x492ce0`, its terrain leg (`0x492580` → `BindLocation
//!   0x6e60f0`) and its object leg (`0x4925d0` → `SetSelection 0x493540` → `BindTarget 0x6e5b40`),
//!   whose unit arm [`bind_target_unit`] shares with `SpellTargetUnit` ([`drain_spell_target_unit`]).
//! - [`item`]: the bag click (`PickupContainerItem 0x4f9b30`) and the paper-doll click
//!   (`0x4c7300`), both `IsTargeting`, `TargetingWantsItem 0x6e6330`, then `0x495d60`, whose
//!   confirm popups park the clicked guid (`0xb4e3c0`) with the word still standing.
//!
//! Every seam commits through [`crate::spell::CastLadder::commit_targeted`] (`SendCast 0x6e54f0`).
//! Only the unit arm has a range gate: the terrain and GameObject clicks send and the server
//! judges range, and `CheckGroundPointInRange` only colours the cursor. While targeting, the pick
//! flags come from the word alone, so a click over a unit with a dest-only word commits on the
//! ground behind it.
//!
//! Cancels: ESC through `UIParent.lua:1490` ([`feed_targeting_to_vm`], [`drain_stop_targeting`]),
//! the right-button down edge ([`cancel_targeting_on_right_press`]), a new spell's press, which
//! aborts and proceeds (`TryCast 0x6e4b60` at `0x6e4d62`), and the bar's re-press of the same
//! spell (`UseAction 0x4e5ee0`). A cancel clears the word and sends nothing; movement never
//! cancels (`0x515090`).

mod cursor;
mod item;
mod world;

pub(crate) use cursor::{drive_targeting_cursor, ground_cast_radius};
pub(crate) use item::{commit_item_cast_on_pick, EnchantConfirmItem};
pub(crate) use world::{commit_ground_cast_on_click, commit_object_cast_on_click};

use bevy::prelude::*;

use benilla_world::interact::WorldRightPress;

/// The inputs of the cursor's two unit functions, [`SpellTargeting::can_target_unit`] and
/// [`bind_target_unit`]: the relation checks the cast arm's selection bind runs
/// ([`super::cast_target::unit_word_binds`]) and the pre-send range gate's inputs
/// ([`super::cast_target::RangeInputs`]), read as [`super::cast_target::CastTargeting`] reads them.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct UnitBindChecks<'w, 's> {
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
    /// The range leg's positions: the pose the hover picks against, as last frame propagated it,
    /// so the VM feed, the cursor and both binds read one pose. Never the camera's, which
    /// `publish_camera_pose` rewrites mid-frame.
    poses: Query<'w, 's, &'static GlobalTransform, Without<benilla_world::view::WorldCamera>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
}

impl UnitBindChecks<'_, '_> {
    fn is_self(&self, entity: Entity) -> bool {
        self.self_q
            .iter()
            .next()
            .is_some_and(|(me, _)| me == entity)
    }

    /// `BindTarget 0x6e5b40`'s relation checks: whether this unit clears the whole word.
    fn relations_clear(&self, word: u16, entity: Entity) -> bool {
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
        };
        super::cast_target::unit_word_binds(word, self.is_self(entity), &rel)
    }

    /// The spell's min/max range against this unit, through the one compare the pre-send gate
    /// asks ([`super::cast_target::RangeInputs::refusal`]); our own body is distance 0. An
    /// unknown spell or a missing row passes.
    fn range_refusal(&self, spell_id: u32, entity: Entity) -> Option<u8> {
        let spells = self.spells.as_deref()?;
        let def = spells.catalog.get(spell_id)?;
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
            target_reach: self
                .stores
                .get(entity)
                .ok()
                .map(|s| s.0.unit_combat_reach()),
            ..Default::default()
        };
        if let Some((_, Some(store))) = me {
            range.self_reach = store.0.unit_combat_reach();
        }
        range.refusal(def, spells.ranges.get(def.range_index))
    }

    /// The caster under a spell with `AttributesEx & 0x80000` (`6e6507`).
    fn excluded_caster(&self, spell_id: u32, entity: Entity) -> bool {
        self.is_self(entity)
            && self
                .spells
                .as_deref()
                .and_then(|s| s.catalog.get(spell_id))
                .is_some_and(|d| d.excludes_caster())
    }
}

/// A click seam's mask test on the flag_word `0xcecac0`, one per reference predicate:
///
/// - `Location`: `TargetingWantsLocation 0x6e6320`, `word & 0x60`, the terrain click.
/// - `Item`: `TargetingWantsItem 0x6e6330`, `word & 0x4010`, the bag and paper-doll clicks.
/// - `GameObject`: `TargetingWantsGameObject 0x6e62d0`, `word & 0x4800`, the world object click.
/// - `Unit`: `SpellCanTargetUnit 0x6e6460`, the unit arm of the world click.
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
        };
        word & mask != 0
    }
}

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
    /// `BindTarget`'s relation checks, the caster refused under `AttributesEx & 0x80000`
    /// (`6e6507`), then min² ≤ d² ≤ max² (`6e677c`–`6e6802`), so a unit inside the minimum is out
    /// too. There is no line-of-sight test.
    pub(crate) fn can_target_unit(&self, entity: Entity, checks: &UnitBindChecks) -> bool {
        self.0.as_ref().is_some_and(|t| {
            checks.relations_clear(t.word, entity)
                && !checks.excluded_caster(t.spell_id, entity)
                && checks.range_refusal(t.spell_id, entity).is_none()
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

/// Push the targeting state into the VM each frame, before the input pass, so a word armed last
/// frame stands for this frame's clicks: the ESC chain's word, the bag and doll pickup reroute's
/// item half, and `CURRENT_SPELL_CAST_CHANGED` on each edge. That event hides the enchant confirm
/// popups (`UIParent.lua:449`); its one emitter `0x4b3250` is called from the cast arm, abort and
/// bind sites, `StopTargeting 0x6e4900` among them. It fires on the spell changing, so a cancel
/// and re-arm in one frame still counts.
pub(crate) fn feed_targeting_to_vm(
    targeting: Res<SpellTargeting>,
    checks: UnitBindChecks,
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

/// Drain the ESC chain's `SpellStopTargeting()` after the input pass: `StopTargeting 0x6e4900`,
/// word cleared, no packet.
pub(crate) fn drain_stop_targeting(
    mut targeting: ResMut<SpellTargeting>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
) {
    let Some(mut script) = script else {
        return;
    };
    if script.take_stop_targeting() {
        debug!("ui_action: targeting cancelled (ESC chain)");
        targeting.clear();
    }
}

/// `BindTarget 0x6e5b40`'s unit arm, for the world click (`0x493540` at `4935d5`) and
/// `SpellTargetUnit` (`0x6e5b10`): a unit failing the relation checks binds nothing and the
/// cursor stays up; one out of the spell's range raises "Out of range." or "Target too close"
/// (`6e6063`); otherwise the cast commits at it. The player's selection never moves.
fn bind_target_unit(
    ladder: &mut crate::spell::CastLadder,
    checks: &UnitBindChecks,
    entity: Entity,
    guid: u64,
) {
    let Some((spell_id, commit, word)) = ladder.ground.pending() else {
        return;
    };
    if !checks.relations_clear(word, entity) {
        return;
    }
    if let Some(reason) = checks.range_refusal(spell_id, entity) {
        // The reference's abort here (`6e6089`) also sends CMSG_CANCEL_CAST and strands the
        // cursor (`6e5b93`) until a cancel: a bug, not copied; targeting stays armed and working.
        debug!("ui_action: cast {spell_id} not bound at {guid:#x} — range ({reason:#x})");
        ladder.cast_errors.push_local(spell_id, reason);
        return;
    }
    debug!("ui_action: cast {spell_id} committed at unit {guid:#x}");
    ladder.commit_targeted(spell_id, commit, super::cast_send::TargetedBind::Unit(guid));
}

/// Drain `SpellTargetUnit(unit)` after the UI click: stock unit frames call this before their
/// ordinary selection arm, and a resolved unit goes to [`bind_target_unit`].
pub(crate) fn drain_spell_target_unit(
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    tokens: crate::ui_unit::UnitTokens,
    selection: Res<crate::target::Selection>,
    checks: UnitBindChecks,
    mut ladder: crate::spell::CastLadder,
) {
    let Some(mut script) = script else {
        return;
    };
    for token in script.take_spell_target_unit() {
        let Some((entity, guid)) = tokens.resolve(&token, &selection) else {
            continue;
        };
        bind_target_unit(&mut ladder, &checks, entity, guid);
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
        let empty = || ObjectStore(benilla_protocol::ObjectFields::default());
        world.spawn((SelfPlayer, Guid(ME), GlobalTransform::default(), empty()));
        let ally = world
            .spawn((
                Guid(ALLY),
                GlobalTransform::from_translation(Vec3::new(distance, 0.0, 0.0)),
                empty(),
            ))
            .id();
        world.insert_resource(crate::target::Selection {
            target: Some(ally),
            guid: Some(ALLY),
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
