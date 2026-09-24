//! **The targeting cursor** — the client's one "this cast is waiting for a click" machine, and the
//! three seams that can end it: the **terrain** click (decision 0792, closing B132:
//! "ground-targeted AOE all Invalid target"), the **bag / paper-doll** click (decision 0923:
//! poisons, stones, oils, scopes, enchants; 0928: its lockbox word and the two confirm popups), and
//! the **world GameObject** click (decision 0939: Opening / Pick Lock / Mining / Herb Gathering
//! armed from the book or a bar, then clicked at a chest, door or node).
//!
//! The reference's targeting mode IS a nonzero flag_word (`IsTargeting 0x6e48a0`), and that word —
//! not a verdict about it — is what this module holds ([`SpellTargeting`]). Each seam asks it the
//! seam's own one-instruction question ([`TargetingWants`]), which is why a lock spell can arm the
//! bag click and the world click at once.
//!
//! This file is the **state and the input plumbing**; the three seams and the cursor live beside
//! it, each transcribing a piece of the reference (the two pickup seams from 0923's own read):
//!
//! - [`cursor`] — while targeting, the world classifier is pre-empted (the ref's dispatcher step 2
//!   runs before any object resolve), and the verdict is **per-seam** (decision 0949): the pick
//!   flags come from the word, so the word chooses the handler — terrain → `0x4820f0`'s
//!   `CheckGroundPointInRange 0x6e6810`, a GameObject → `0x4828d0`'s `0x6e6460` (the word's
//!   `& 0x4800`, the spell-vs-lock predicate `0x5f8260`, then the range test), and **no handler
//!   at all → UnableCast**. That default is what greys an armed lockpick over open ground and
//!   greys a poison everywhere in the world. Also the reticle's radius ([`ground_cast_radius`]).
//! - [`world`] — the two legs of the world-click dispatcher `0x492ce0`: the **terrain** leg's
//!   ground commit (`0x492580` → `BindLocation 0x6e60f0`) and the **object** leg's GameObject bind
//!   (`0x4925d0` → `SetSelection 0x493540` @ `4935d5` → `BindTarget 0x6e5b40`'s GO arm). Which leg
//!   a click can even take is a function of the pending spell's word, not of the scene.
//! - [`item`] — the bag click (`PickupContainerItem 0x4f9b30` @ `4f9c54`) and the paper-doll click
//!   (`0x4c7300` @ `4c76df`) each carry the same three-instruction rung — IsTargeting,
//!   `TargetingWantsItem 0x6e6330`, then `0x495d60(itemGuid)` and return — and `0x495d60` is
//!   [`item_bind_verdict`] plus the bind. Two of its four exits are the **confirm popups**
//!   (`BIND_ENCHANT` / `REPLACE_ENCHANT`, decision 0928), which park the clicked guid
//!   ([`EnchantConfirmItem`], the ref's `0xb4e3c0`) and return with the word still standing; their
//!   Yes re-enters that same drain.
//!
//! All three end in **one commit tail** ([`crate::spell::CastLadder::commit_targeted`]): the
//! packet (`SendCast 0x6e54f0`'s same block, two opcodes), the pending arm, the GCD, and the word
//! cleared.
//!
//! **The ESC chain** ([`feed_targeting_to_vm`] / [`drain_stop_targeting`]): the real
//! `UIParent.lua:1490` rung (`elseif ( SpellStopTargeting() ) then`) runs in our live VM; the feed
//! pushes the state its `SpellIsTargeting`/`SpellStopTargeting` bindings read (and the item seam's
//! arm, which gates the VM's click reroute), the drain commits the cancel. AbortCast in targeting
//! mode clears the word and sends **nothing**.
//!
//! Entry and the two press-cancel shapes live in the cast path itself: the resolver yields
//! [`crate::spell::cast_target::CastWireTarget::Targeting`] carrying the word, the one
//! cast-send path enters the mode here, a NEW spell's press aborts-and-proceeds (`TryCast
//! 6e4d62`), and the action bar's re-press of the SAME spell toggles the mode off (`UseAction
//! 0x4e5ee0`'s `GetTargetingSpellId`+`StopTargeting` — [`crate::ui_action::drain`]).
//!
//! The reference's click path: the terrain-leg commit
//! `0x492580` has **no range gate and no error path** — it binds and sends regardless, and the
//! server judges range (`CheckGroundPointInRange 0x6e6810` has exactly ONE caller binary-wide, the
//! hover classifier `0x4820f0`: its verdict colours the cursor and nothing else). The object leg
//! has no gate either. While targeting, the pick flags come from the pending spell's mask alone —
//! for a dest-only word a unit is not pickable, so a click over one commits on the ground behind it
//! ([`crate::target::click::select_on_click`]'s gate transcribes the unreachable select).
//! Right-click cancels on the DOWN edge ([`cancel_targeting_on_right_press`]); movement never
//! cancels (`0x515090`'s explicit IsTargeting-skip). The ground reticle draws in [`crate::target`]'s
//! `reticle` module (decision 0797) off [`ground_cast_radius`] + the cursor's range verdict — for
//! the **terrain seam alone**, through [`SpellTargeting::spell_for`] (decision 0943): it is a
//! per-seam surface, and reading the seam-agnostic [`SpellTargeting::spell`] there is what put a
//! green AoE circle under every armed lockpick and enchant.

mod cursor;
mod item;
mod world;

pub(crate) use cursor::{drive_targeting_cursor, ground_cast_radius};
pub(crate) use item::{commit_item_cast_on_pick, EnchantConfirmItem};
pub(crate) use world::{commit_ground_cast_on_click, commit_object_cast_on_click};

use bevy::prelude::*;

use benilla_world::interact::WorldRightPress;

/// Which click seam is asking the standing flag_word whether it may bind — the reference's three
/// *wants* predicates, each a one-instruction mask test on the same word `0xcecac0`, each consulted
/// by exactly one seam. Read off the three (six-instruction) functions themselves:
///
/// - `TargetingWantsLocation 0x6e6320` (`6e6328: testb $0x60, %cl`) → the terrain click's commit.
/// - `TargetingWantsItem 0x6e6330` (`6e6338: testl $0x4010, %ecx`) → the **bag** click's bind
///   (`PickupContainerItem 0x4f9b30` @ `4f9c5d`) and the **paper-doll** click's
///   (`0x4c7300` @ `4c76e8`) — the identical three-instruction rung in both.
/// - `TargetingWantsGameObject 0x6e62d0` (`6e62d8: testb $0x48, %ch`, i.e. `word & 0x4800`) → the
///   **world** click on a GameObject (decision 0939).
///
/// **This is a question asked of a word, not a partition of words.** `0x4010` and `0x4800` overlap
/// on `TARGET_FLAG_LOCKED`, so every lock spell answers *yes* to both the item and the GameObject
/// seam — one armed cursor, two clicks that could end it — and the reference resolves that nowhere
/// but at the click, where `BindTarget 0x6e5b40` picks its arm by the clicked object's typemask
/// (`6e5f17` item bit 1 / `6e5f52` GameObject bit 5) and *then* re-tests the word. Modelling the
/// mode as one-of-N is what kept a Dull Iron Key from opening the door it was standing in front of.
///
/// The word itself is one state, which is why this is a mode of one resource and not three
/// resources: every cancel (ESC, right-press, a new cast's abort-and-proceed) clears the one word,
/// and the reference has exactly one `IsTargeting`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TargetingWants {
    /// Decision 0792 — the terrain click.
    Location,
    /// Decision 0923 — the bag / paper-doll click.
    Item,
    /// Decision 0939 — the world click on a GameObject.
    GameObject,
}

/// The unit-shaped bits of the flag_word — what a UNIT candidate could ever satisfy, and so what
/// `SpellCanTargetUnit` answers for. None of them can appear in a word that reaches the targeting
/// cursor today (the resolver binds or refuses a unit word before it gets here), which is exactly
/// why this is a mask test rather than a `false`.
const UNIT_WORD_BITS: u16 = 0x0002 | 0x0004 | 0x0008 | 0x0080 | 0x0100 | 0x0200 | 0x0400 | 0x8000;

impl TargetingWants {
    /// The seam's own mask test, verbatim.
    fn matches(self, word: u16) -> bool {
        let mask = match self {
            Self::Location => 0x0060,
            Self::Item => 0x4010,
            Self::GameObject => 0x4800,
        };
        word & mask != 0
    }
}

/// The targeting-cursor mode — benilla's `flag_word != 0` mirror: `Some` while a cast awaits the
/// click that binds its target. Entered by the one cast-send path
/// ([`super::cast_target::CastWireTarget::Targeting`]), cleared by whichever commit fires, the two
/// press cancels, and the ESC drain.
#[derive(Resource, Default)]
pub(crate) struct SpellTargeting(Option<Targeting>);

struct Targeting {
    spell_id: u32,
    /// What the click will commit. The ref keeps the whole pending-cast block across the cursor —
    /// the cast **item's** guid at `0xceac48` included — so `0x6e54f0`'s discriminator still picks
    /// `CMSG_USE_ITEM` when the click lands: a thrown grenade for the terrain seam (decision
    /// 0914), a poison bottle for the item seam (0923), a key for the GameObject seam (0939).
    commit: super::cast_send::CastCommit,
    /// The standing flag_word `0xcecac0` itself — not a verdict about it. Every seam asks it its
    /// own question; more than one can answer yes.
    word: u16,
}

impl SpellTargeting {
    /// `IsTargeting 0x6e48a0` — the canonical predicate.
    pub(crate) fn active(&self) -> bool {
        self.0.is_some()
    }

    /// `GetTargetingSpellId 0x6e48e0` — the spell awaiting its click, whatever its word wants.
    /// For the whole-word consumers only: the bar's checked state, the press-again toggle, the
    /// `CURRENT_SPELL_CAST_CHANGED` edge. A seam-specific one wants [`Self::spell_for`].
    pub(crate) fn spell(&self) -> Option<u32> {
        self.0.as_ref().map(|t| t.spell_id)
    }

    /// Whether the standing word answers `wants`' mask test — `false` when nothing is targeting.
    pub(crate) fn wants(&self, wants: TargetingWants) -> bool {
        self.0.as_ref().is_some_and(|t| wants.matches(t.word))
    }

    /// The pending spell **when the standing word answers `wants`** — `IsTargeting` and the seam's
    /// own mask test as one read, which is the pair every seam-specific consumer needs and
    /// [`Self::spell`] deliberately is not.
    ///
    /// Any surface that draws or binds for *one* seam must ask through this or through
    /// [`Self::pending_for`]; `spell()` is `GetTargetingSpellId 0x6e48e0` and answers for the word
    /// as a whole (the bar's checked state, the re-press toggle, the
    /// `CURRENT_SPELL_CAST_CHANGED` edge). Reading `spell()` where a seam was meant is what put an
    /// AoE reticle under a lockpick (decision 0943).
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

    /// The pending cast's `(spell, commit)` when the standing word answers `wants` — the shape
    /// every commit system opens with, so none can fire on a word its seam cannot bind.
    fn pending_for(&self, wants: TargetingWants) -> Option<(u32, super::cast_send::CastCommit)> {
        self.0
            .as_ref()
            .filter(|t| wants.matches(t.word))
            .map(|t| (t.spell_id, t.commit))
    }

    /// `BindLocation 0x6e60f0`'s own fork, as one read: which of the two location slots the
    /// terrain click binds this point to. Bit 5 (SOURCE) is tested **before** bit 6 (DEST) and
    /// the arms are exclusive (`0x6e60f0` commits only once the word is 0 — a word
    /// carrying both takes two clicks and sends on the second; no 5875 spell carries both, so
    /// only the precedence is transcribed here, not the two-click walk). Decision 2218.
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

/// Right-click cancels targeting — on the **DOWN edge**, the reference's WorldFrame
/// `OnMouseDown 0x483c40` → `0x492c20`: right button ∧ `IsTargeting` → `StopTargeting
/// 0x6e4900`, no packet — and the handler returns 0, so the press keeps doing everything else
/// it did (the turn-drag, the release's context click; we consume nothing either). Of
/// `StopTargeting`'s callers, this and the
/// ESC/UseAction/TryCast paths are the ONLY input-band cancels — no keyboard caller exists.
///
/// Two qualifications, transcribed: a held cursor payload pre-empts the cancel (`0x492b50`
/// clears the payload and returns before the WorldFrame virtuals dispatch — our payload keeps
/// its own clean-click clear in [`crate::target::click::world_right_click_payload`]); and a
/// press over a UI frame never reaches the WorldFrame — [`WorldRightPress`]'s world gate
/// transcribes the certain half of the one open question (whether a UI-frame right-click also
/// cancels is unsettled). The `0x51`-effect placement-rotate skip (`[0xceca90]`) is
/// unmodelled along with the flag itself (named residual, 0792).
pub(crate) fn cancel_targeting_on_right_press(
    mut presses: MessageReader<WorldRightPress>,
    payload_held: Res<crate::ui_script::CursorPayloadHeld>,
    mut targeting: ResMut<SpellTargeting>,
) {
    if !targeting.active() {
        // Reader hygiene, like the commit's: a press buffered while idle never replays as a
        // cancel the frame the mode turns on.
        presses.clear();
        return;
    }
    if presses.read().last().is_none() || payload_held.0 {
        return;
    }
    debug!("ui_action: targeting cancelled (right-click)");
    targeting.clear();
}

/// Push the targeting state into the live VM each frame, **before** the input pass — so a word
/// armed last frame is already standing when this frame's clicks run. Three consumers, one push:
/// `SpellIsTargeting()` / `SpellStopTargeting()`'s ESC chain reads the word itself, the engine's
/// bag / doll pickup reroute reads the item half (`TargetingWantsItem`'s mirror), and the word's
/// **edges** fire `CURRENT_SPELL_CAST_CHANGED`.
///
/// That last one is the confirm popups' whole teardown (decision 0928): `UIParent.lua` hides both
/// on it, and without it a popup outlives the cast it is asking about — its Yes would then bind a
/// *stale* item guid into whatever cast armed next. The reference fires the event from exactly
/// these edges: `CURRENT_SPELL_CAST_CHANGED` (261) has ONE emitter binary-wide, the two-line
/// `0x4b3250`, and its twelve callers are the `Spell_C` arm/abort/bind sites — **`StopTargeting
/// 0x6e4900` @ `6e495c` among them**. We fire on the spell changing rather than on the raw flag so
/// a cancel-and-rearm in one frame still counts as a change.
pub(crate) fn feed_targeting_to_vm(
    targeting: Res<SpellTargeting>,
    mut last: Local<crate::ui_script::VmMemo<Option<u32>>>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
) {
    if let Some(mut script) = script {
        let last = last.get(&script);
        script.set_spell_targeting(targeting.active());
        script.set_item_pick_armed(targeting.wants(TargetingWants::Item));
        // `SpellCanTargetUnit`'s answer (`0x6e6460`'s unit leg). Derived from the word rather than
        // hardcoded false: none of the three seams benilla models can be satisfied by a unit, so it
        // is false today — and it becomes right on its own if the residual unit-word machine lands.
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

/// Drain the ESC chain's `SpellStopTargeting()` trigger (**after** the input pass) and clear
/// the mode — the ref's `StopTargeting 0x6e4900` → AbortCast-in-targeting: word cleared, no
/// packet.
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

    /// A seam only answers a word its own mask test accepts (decisions 0923 / 0939). Without this,
    /// a terrain click while a poison is armed would ship a DEST block for an item spell.
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

        // Blizzard — the bare DEST word.
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

        // Instant Poison — the bare ITEM word.
        t.enter(8679, commit, 0x0010);
        assert_eq!(t.pending_for(TargetingWants::Item), Some((8679, commit)));
        for seam in [TargetingWants::Location, TargetingWants::GameObject] {
            assert_eq!(t.pending_for(seam), None, "{seam:?} cannot commit a poison");
        }
        // The spell id is the word's, whichever seam — what the action bar's re-press toggle reads.
        assert_eq!(t.spell(), Some(8679));

        t.clear();
        assert!(!t.active());
    }

    /// **The overlap is the point** (decision 0939). A lock spell's word — `LOCKED`, plus arm 23's
    /// `GAMEOBJECT` overlay — is in `TargetingWantsItem`'s `0x4010` *and* in
    /// `TargetingWantsGameObject`'s `0x4800`, so one armed cursor genuinely answers two seams and
    /// whichever click lands first is the one that binds. An enum of halves could not say this, and
    /// saying it wrong is what left a lockpick with no way to reach a chest.
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

        // A bare GAMEOBJECT word (arm 23 over a `Targets`-less row) is the world seam's alone —
        // `0x800 & 0x4010 == 0`.
        t.enter(3365, commit, 0x0800);
        assert!(t.wants(TargetingWants::GameObject));
        assert!(!t.wants(TargetingWants::Item));
    }
}
