//! The targeting cursor: a cast waiting for the click that binds its target. The reference's
//! targeting mode is a nonzero flag_word (`IsTargeting 0x6e48a0`); [`SpellTargeting`] holds that
//! word and each click seam asks it its own mask test ([`TargetingWants`]).
//!
//! - [`cursor`]: the hover verdict per seam. Terrain is `0x4820f0`'s `CheckGroundPointInRange
//!   0x6e6810`, a GameObject is `0x4828d0`'s `0x6e6460` (the spell-vs-lock predicate `0x5f8260`,
//!   then range), and a word no seam handles is UnableCast.
//! - [`world`]: the world-click dispatcher `0x492ce0`, its terrain leg (`0x492580` → `BindLocation
//!   0x6e60f0`) and its object leg (`0x4925d0` → `SetSelection 0x493540` → `BindTarget 0x6e5b40`).
//! - [`item`]: the bag click (`PickupContainerItem 0x4f9b30`) and the paper-doll click
//!   (`0x4c7300`), both `IsTargeting`, `TargetingWantsItem 0x6e6330`, then `0x495d60`, whose
//!   confirm popups park the clicked guid (`0xb4e3c0`) with the word still standing.
//!
//! Every seam commits through [`crate::spell::CastLadder::commit_targeted`] (`SendCast 0x6e54f0`).
//! Neither world leg has a range gate: the click sends and the server judges range, and
//! `CheckGroundPointInRange` only colours the cursor. While targeting, the pick flags come from
//! the word alone, so a click over a unit with a dest-only word commits on the ground behind it.
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

/// A click seam's mask test on the flag_word `0xcecac0`, one per reference predicate:
///
/// - `Location`: `TargetingWantsLocation 0x6e6320`, `word & 0x60`, the terrain click.
/// - `Item`: `TargetingWantsItem 0x6e6330`, `word & 0x4010`, the bag and paper-doll clicks.
/// - `GameObject`: `TargetingWantsGameObject 0x6e62d0`, `word & 0x4800`, the world object click.
///
/// The masks overlap on `TARGET_FLAG_LOCKED`, so a lock spell answers both the item and the
/// GameObject seam; the reference settles it only at the click, where `BindTarget 0x6e5b40` picks
/// its arm by the clicked object's typemask.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TargetingWants {
    Location,
    Item,
    GameObject,
}

/// The unit-shaped bits of the flag_word, what `SpellCanTargetUnit` tests. The resolver binds or
/// refuses a unit word before it reaches the cursor, so none is set today.
const UNIT_WORD_BITS: u16 = 0x0002 | 0x0004 | 0x0008 | 0x0080 | 0x0100 | 0x0200 | 0x0400 | 0x8000;

impl TargetingWants {
    fn matches(self, word: u16) -> bool {
        let mask = match self {
            Self::Location => 0x0060,
            Self::Item => 0x4010,
            Self::GameObject => 0x4800,
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
    mut last: Local<crate::ui_script::VmMemo<Option<u32>>>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
) {
    if let Some(mut script) = script {
        let last = last.get(&script);
        script.set_spell_targeting(targeting.active());
        script.set_item_pick_armed(targeting.wants(TargetingWants::Item));
        // `SpellCanTargetUnit`, `0x6e6460`'s unit leg, read off the word.
        script.set_spell_can_target_unit(
            targeting
                .0
                .as_ref()
                .is_some_and(|t| t.word & UNIT_WORD_BITS != 0),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

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
