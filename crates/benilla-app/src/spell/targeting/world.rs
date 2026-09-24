//! The **world** click's two commits — the two legs of the reference's click dispatcher
//! `0x492ce0`, which while targeting are chosen by the *pending spell's word*, not by the scene
//! (the pick flags `0x481050` builds come only from
//! `0xcecac0` when `IsTargeting`).
//!
//! - **terrain leg** (`0x492c90` → `0x492580` → `BindLocation 0x6e60f0`) — [`commit_ground_cast_on_click`]
//! - **object leg** (`0x492ce0` → `SELECT 0x4925d0` → `SetSelection 0x493540` @ `4935d5` →
//!   `BindTarget 0x6e5b40`) — [`commit_object_cast_on_click`]
//!
//! They cannot both fire for one word, and not because we sequence them: the predicates are
//! disjoint on real data (`& 0x60` vs `& 0x4800`), and in the reference the *pick* enforces it —
//! a location-only word yields pick flags `3`, whose `& 0x7c == 0` disables the object pick
//! entirely, which is why an AoE reticle clicks straight through a chest.
//!
//! Neither leg gates on range, validity or the lock. `0x492580`'s complete callee set contains no
//! range call and no error emitter, and the object leg's `BindTarget` arm reads nothing but the
//! clicked object's typemask and the word. The server judges; its refusing
//! `SMSG_CAST_RESULT` is the red line.

use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;

use crate::spell::cast_send::TargetedBind;
use crate::target::go_is_nearest;
#[cfg(test)]
use crate::target::{Hovered, HoveredObject};
use benilla_world::interact::WorldClick;

use super::TargetingWants;

/// The world click's ground commit — the terrain leg's action-1 arm (`0x492580`, tried before
/// anything else the click could mean; [`crate::target::click::select_on_click`] holds its gate
/// while this mode is active, so the click neither selects nor deselects). Binds the frame's
/// pick-occlusion point and sends **unconditionally** — the leg's complete callee set has no
/// range check and no error path (the click
/// never gates on range, the server judges it, and its refusing `SMSG_CAST_RESULT` is the red
/// line) — `CMSG_CAST_SPELL` mask `0x40` + the point (WoW coords), arming the pending cast +
/// the GCD (the `SendCast 0x6e54f0` tail's two live pieces for a ground cast); the mode ends
/// with the send. No world hit (sky) → the nothing leg: no commit, mode kept.
///
/// Runs AFTER `select_on_click` in the target chain: the selection gate reads the mode's state,
/// so the commit that clears it must come later in the same frame.
pub(crate) fn commit_ground_cast_on_click(
    mut clicks: MessageReader<WorldClick>,
    // The point the PRESS ray hit, not this frame's — the reference's `+0x360`, written by the one
    // down-edge pick and read unchanged at the release (decision 1122).
    press: Res<crate::target::PressPick>,
    mut ladder: crate::spell::CastLadder,
) {
    let occlusion = press.occlusion;
    if !ladder.ground.active() {
        // Keep the reader current so a click buffered while idle can never replay as a commit
        // the frame the mode turns on.
        clicks.clear();
        return;
    }
    if clicks.read().last().is_none() {
        return;
    }
    // `TargetingWantsLocation 0x6e6320` — an item-targeting word has no location leg, so the
    // terrain click's `BindLocation` binds nothing and the mode simply stays up.
    let Some((spell_id, commit)) = ladder.ground.pending_for(TargetingWants::Location) else {
        return;
    };
    let Some(point) = occlusion.point else {
        // The ray hit nothing (sky) — the ref's nothing-leg has no ground commit; the mode
        // stays, exactly like the UnableCast cursor said it would.
        return;
    };
    let at = bevy_to_wow(point);
    // `BindLocation 0x6e60f0` has an arm per location bit and tests SOURCE first — the one click
    // binds `SPELLCAST+0x30` (wire `0x0020`) for a `Targets & 0x20` spell and `+0x3c` (wire
    // `0x0040`) for a `& 0x40` one (decision 2218). `None` cannot happen behind
    // `pending_for(Location)`, whose mask is the same `0x60`; it stays a `let else` rather than an
    // unwrap so a future seam edit fails closed.
    let Some(bound) = ladder.ground.location_bind(at) else {
        return;
    };
    debug!(
        "ui_action: ground cast {spell_id} committed at wow ({:.2}, {:.2}, {:.2}) as {bound:?}",
        at[0], at[1], at[2]
    );
    // The shared commit tail — same block, two opcodes (`SendCast 0x6e54f0`'s one discriminator
    // survives the cursor, decision 0914: a thrown grenade commits as `CMSG_USE_ITEM` with the
    // location block), then the pending arm, the GCD, and the word cleared.
    ladder.commit_targeted(spell_id, commit, bound);
}

/// The world click's **GameObject** commit — the object leg (decision 0939). While targeting, a
/// left-click that resolves to an object goes `0x492ce0` → `SELECT 0x4925d0` → `SetSelection
/// 0x493540`, and `0x493540`'s *first* act is the targeting intercept: `4935ca call 0x6e48a0;
/// je 0x4935ec` — targeting ⇒ `4935d5 call 0x6e5b40` `BindTarget(this = the clicked object)` and
/// `ret 8` immediately, never reaching the selection write. So a click that feeds a pending cast
/// **does not change the player's target**, which is why
/// [`crate::target::click::select_on_click`]'s mode gate — already there for the terrain leg —
/// is the whole selection story here too. (It also never reaches the `[vtbl+0x58]`
/// "can be selected" stub that makes a GameObject unselectable: the intercept is upstream of it.)
///
/// `BindTarget` picks its arm from the clicked object's **typemask** — `6e5f52: shrl $0x5, %ecx;
/// testb $0x1, %cl` selects the GameObject arm — and that arm then asks the *word*:
/// `6e5f60: testb $0x48, %ch`, which is `TargetingWantsGameObject 0x6e62d0`'s own `0x4800`. On a
/// match it writes the wire bit `6e5f69: orb $0x8, 0xceac5d` = `TARGET_FLAG_GAMEOBJECT (0x800)`
/// — **not** `LOCKED`, which is a word bit this arm *consumes* and clears (`6e5f70: andb $0xb7,
/// 0xcecac1`) — parks the guid at `0xceac60/64`, and the now-zero word lets the tail fire
/// `SendCast 0x6e54f0` (`6e60c1: cmpw $0, 0xcecac0; 6e60d7: call 0x6e54f0`).
///
/// A click on a **unit** while a lock word stands binds nothing: every unit arm of `0x6e5b40`
/// tests a bit (`0x2/0x4/0x8/0x80/0x100/0x200/0x8000`) the word does not carry, `bl` stays 0, and
/// the function returns having written nothing and left the word alone — the cursor simply stays
/// up. [`go_is_nearest`] is that same discrimination on our side.
///
/// **No gate of any kind before the send** — not range, not the lock, not "is this object even
/// openable". The right-click path ([`crate::target::click`]) resolves the lock itself and can
/// refuse locally with a toast; this path is the reference's blunt one, and its refusal arrives as
/// the server's `SMSG_CAST_RESULT`. That asymmetry is the reference's, not ours: `0x5f33e0` is a
/// lock-routing sender, `0x6e5b40` is a target binder.
///
/// Runs AFTER `select_on_click`, like the terrain commit and for the same reason.
pub(crate) fn commit_object_cast_on_click(
    mut clicks: MessageReader<WorldClick>,
    // The press's pick, as in the terrain leg — the object a gesture binds is the one it started
    // on, whatever the mouse did after (decision 1122).
    press: Res<crate::target::PressPick>,
    mut ladder: crate::spell::CastLadder,
) {
    let (hovered, hovered_object) = (press.hovered, press.object);
    if !ladder.ground.active() {
        // Reader hygiene, as in the terrain leg: a click buffered while idle must not replay as a
        // commit the frame the mode turns on.
        clicks.clear();
        return;
    }
    if clicks.read().last().is_none() {
        return;
    }
    // `TargetingWantsGameObject 0x6e62d0` — a poison's bare `0x10` word has no GameObject leg, so
    // clicking a chest with one armed binds nothing and the mode stays up.
    let Some((spell_id, commit)) = ladder.ground.pending_for(TargetingWants::GameObject) else {
        return;
    };
    // The pick's object leg only wins when a GameObject is what the ray actually hit nearest.
    if !go_is_nearest(&hovered, &hovered_object) {
        return;
    }
    let Some(guid) = hovered_object.guid else {
        return;
    };
    debug!("ui_action: cast {spell_id} committed at gameobject {guid:#x}");
    // The shared commit tail — `CMSG_CAST_SPELL` mask `0x800` + the packed guid for a known
    // opener, `CMSG_USE_ITEM` with the same block for a key's own ON_USE (the pending-cast block
    // survives the cursor, decision 0914) — then the pending arm, the GCD, and the word cleared.
    ladder.commit_targeted(spell_id, commit, TargetedBind::Object(guid));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::{ClientCommand, NetCommands};
    use bevy::ecs::system::SystemId;
    use crossbeam_channel::Receiver;

    const OPENING: u32 = 3365;
    const CHEST: u64 = 0xF110_000C_1F00_A3B2;
    /// The lock word a shipped opener arms: `Targets 0x4000` + implicit arm 23's `|0x800`.
    const LOCK_WORD: u16 = 0x4800;

    /// The ladder's resource set, plus the two hover verdicts and the click message this seam
    /// reads. Kept minimal on purpose: what this test is for is the WIRING — that the system reads
    /// the hover the pick wrote and the word the arm left, and that a click turns them into the
    /// one packet. The packet's own shape is pinned in `benilla-protocol`.
    fn fixture() -> (World, Receiver<ClientCommand>, SystemId) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::items::Items>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::spell::PendingCast>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::Cooldowns>();
        world.init_resource::<crate::spell::SpellModifiers>();
        world.init_resource::<crate::ui_action::CastErrors>();
        world.init_resource::<crate::ui_action::UiErrorKeys>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<crate::ui_tradeskill::TradeSkillOpens>();
        world.init_resource::<super::super::SpellTargeting>();
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<Messages<WorldClick>>();
        // The commit legs read the PRESS latch now (decision 1122), not the live hover.
        world.init_resource::<crate::target::PressPick>();
        // REGISTERED, not `run_system_once`: this seam's reader hygiene is a property of state
        // that survives between frames, and a fresh system per call would start every read at
        // cursor 0 and hide exactly the bug the drain exists to prevent.
        let id = world.register_system(commit_object_cast_on_click);
        (world, rx, id)
    }

    /// Put a GameObject under the cursor at `distance`, nearer than any unit — in the press latch,
    /// which is where a click's subject lives.
    fn hover_go(world: &mut World, distance: f32) {
        world.resource_mut::<crate::target::PressPick>().object = HoveredObject {
            target: Some(Entity::from_raw_u32(1).unwrap()),
            guid: Some(CHEST),
            distance,
        };
    }

    fn click(world: &mut World, id: SystemId) {
        world
            .resource_mut::<Messages<WorldClick>>()
            .write(WorldClick);
        world.run_system(id).expect("the object commit runs");
    }

    /// **`BindLocation 0x6e60f0`'s two arms, from one terrain click** (decision 2218). The same
    /// click, the same point, the same commit tail — the standing word alone decides whether the
    /// point is bound to the SOURCE slot (`Targets 0x20`: Martin Fury's spell 265) or the DEST one
    /// (`Targets 0x40`: Blizzard). Before this, every ground commit wrote DEST, and a `0x20` word
    /// never reached the cursor at all — it drew "Invalid target" (B388).
    #[test]
    fn the_terrain_click_binds_source_or_dest_by_the_standing_word() {
        const BLIZZARD: u32 = 10;
        const AREA_DEATH: u32 = 265;
        let commit = crate::spell::cast_send::CastCommit::Spell;

        let ground = |world: &mut World| {
            world.resource_mut::<crate::target::PressPick>().occlusion =
                crate::target::PickOcclusion {
                    distance: 5.0,
                    point: Some(Vec3::new(1.0, 2.0, 3.0)),
                };
        };
        let run = |world: &mut World, id: SystemId| {
            world
                .resource_mut::<Messages<WorldClick>>()
                .write(WorldClick);
            world.run_system(id).expect("the ground commit runs");
        };

        // DEST word → the dest opcode, as before.
        let (mut world, rx, _) = fixture();
        let id = world.register_system(commit_ground_cast_on_click);
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(BLIZZARD, commit, 0x0040);
        ground(&mut world);
        run(&mut world, id);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellAtDest {
                spell_id: BLIZZARD,
                ..
            })
        ));

        // SOURCE word → the source opcode, from the identical click.
        let (mut world, rx, _) = fixture();
        let id = world.register_system(commit_ground_cast_on_click);
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(AREA_DEATH, commit, 0x0020);
        ground(&mut world);
        run(&mut world, id);
        let sent = rx.try_recv().expect("a source word still commits");
        assert!(
            matches!(
                sent,
                ClientCommand::CastSpellAtSource {
                    spell_id: AREA_DEATH,
                    ..
                }
            ),
            "a 0x20 word binds the SOURCE slot, not the dest: {sent:?}"
        );
        assert!(
            !world.resource::<super::super::SpellTargeting>().active(),
            "and the commit clears the one word"
        );

        // Both bits standing: SOURCE wins — the reference tests bit 5 before bit 6 and the arms
        // are exclusive. No 5875 row carries both; the precedence is transcribed, not the
        // two-click walk.
        let (mut world, rx, _) = fixture();
        let id = world.register_system(commit_ground_cast_on_click);
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(AREA_DEATH, commit, 0x0060);
        ground(&mut world);
        run(&mut world, id);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellAtSource { .. })
        ));
    }

    /// The whole gesture, end to end: a lock word standing, a chest under the cursor, one
    /// left-click ⇒ the OPEN_LOCK cast at that GameObject, and the cursor is down.
    #[test]
    fn a_click_on_a_hovered_gameobject_commits_the_lock_cast() {
        let (mut world, rx, id) = fixture();
        world.resource_mut::<super::super::SpellTargeting>().enter(
            OPENING,
            crate::spell::cast_send::CastCommit::Spell,
            LOCK_WORD,
        );
        hover_go(&mut world, 5.0);

        click(&mut world, id);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellGameObject {
                spell_id: OPENING,
                go_guid: CHEST,
            })
        ));
        assert!(
            !world.resource::<super::super::SpellTargeting>().active(),
            "the commit clears the one word"
        );
    }

    /// Every way the click must NOT commit — each one a rung the reference has, and each one the
    /// difference between a dead click and a wrong packet.
    #[test]
    fn the_object_commit_holds_its_fire() {
        let commit = crate::spell::cast_send::CastCommit::Spell;

        // (a) Nothing armed: the click is not ours at all, and the reader is drained so it cannot
        // replay as a commit the frame the mode turns on.
        let (mut world, rx, id) = fixture();
        hover_go(&mut world, 5.0);
        click(&mut world, id);
        assert!(rx.try_recv().is_err(), "idle binds nothing");
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(OPENING, commit, LOCK_WORD);
        world.run_system(id).expect("the next frame, no new click");
        assert!(
            rx.try_recv().is_err(),
            "a click buffered while idle must not replay once the word stands"
        );

        // (b) A word with no GameObject leg — a poison's bare ITEM word. `0x10 & 0x4800 == 0`, so
        // clicking a chest with one armed binds nothing and the cursor stays up.
        let (mut world, rx, id) = fixture();
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(8679, commit, 0x0010);
        hover_go(&mut world, 5.0);
        click(&mut world, id);
        assert!(rx.try_recv().is_err(), "a poison has no world leg");
        assert!(
            world.resource::<super::super::SpellTargeting>().active(),
            "and the cursor survives the click it cannot consume"
        );

        // (c) Nothing hovered — bare ground or sky. `BindTarget` is never reached; the reference's
        // nothing-leg writes no guid at all.
        let (mut world, rx, id) = fixture();
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(OPENING, commit, LOCK_WORD);
        click(&mut world, id);
        assert!(rx.try_recv().is_err(), "no object, no bind");
        assert!(world.resource::<super::super::SpellTargeting>().active());

        // (d) A UNIT is nearer than the GameObject. `0x6e5b40` picks its arm by the clicked
        // object's typemask, and every unit arm tests a bit this word does not carry — `bl` stays
        // 0 and nothing is written.
        let (mut world, rx, id) = fixture();
        world
            .resource_mut::<super::super::SpellTargeting>()
            .enter(OPENING, commit, LOCK_WORD);
        hover_go(&mut world, 9.0);
        world.resource_mut::<crate::target::PressPick>().hovered = Hovered {
            target: Some(Entity::from_raw_u32(2).unwrap()),
            guid: Some(0x1234),
            distance: 4.0,
            ..Hovered::default()
        };
        click(&mut world, id);
        assert!(rx.try_recv().is_err(), "a unit in front of the chest wins");
        assert!(world.resource::<super::super::SpellTargeting>().active());
    }
}
