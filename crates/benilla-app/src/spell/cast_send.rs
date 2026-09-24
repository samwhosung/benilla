//! **The one cast-send path** — every spell benilla casts leaves through [`send_spell_cast`].
//!
//! The action button is only one of its callers; the spellbook (decision 0216 §8), the stance bar,
//! the trade-skill window and the craft window all funnel here too. That is the point: the client's
//! `TryCast 0x6e4b60` → commit `0x6e54f0` is *one* function with a long ladder of local gates and a
//! post-send tail, and duplicating any part of it per caller is how the two paths drift. The ladder
//! below is that function's order, gate for gate (re-pinned end to end in 0948) — profession
//! intercept, auto-repeat toggle, targeting abort, in-flight,
//! reagents/totems, target binding + range, then the validator `0x6094f0`'s opening rungs
//! (not-ready/GCD, power), mounted, moving, form, the deferred cast-arm refusal and
//! targeting-cursor entry — followed
//! by the commit's own tail (ranged stance, auto-repeat arm, the send, the auto-attack start, the
//! GCD arm).
//!
//! A refusal here is **local and pre-commit**, exactly like the reference's: no packet, no GCD, no
//! pending arm, no autorepeat key — just the red error line's reason code.
//!
//! **An item use is a cast, and takes this same ladder** (decision 0914). `CGItem::Use 0x5d8d00`'s
//! ordinary tail (`5d9249`–`5d9258`) calls `0x6e5a90`, whose whole body is `call 0x6e4b60` —
//! `TryCast`, with the **item as an ordinary third argument** (`ret 0xc`: item, targetLo,
//! targetHi). Inside TryCast that argument is read exactly twice — at `6e4d76`, where it only
//! computes a display flag and does *not* skip the IsCasting gate below it, and at `6e4f33`, where
//! it is forwarded to the requirement validator `0x6094f0`. Every rung between entry and the
//! commit is therefore the same code for a spell and for an item, which is why [`CastCommit`] is a
//! *parameter* of this function and not a second path. Three rungs fork on it, and only three:
//! the validator's first rung (`60952b`: item → the item cooldown query
//! `0x6e2ed0` on the (use-spell, ENTRY) pair and error **0x28** "Item is not ready yet."; no item
//! → `0x6e2ea0` and **0x3c** "Spell is not ready yet."), the power gate (`0x60962c` — an item
//! press's clear-query jump lands PAST it: items are never power-gated), and the commit's opcode
//! (`0x6e57d8 push 0xab` vs `push 0x12e`).

use std::time::Instant;

use benilla_protocol::messages::UseItemTarget;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, Objects, SelfPlayer};

use super::{cast_target, validator, AutoRepeatActive};
use crate::ui_action::{reagent_totem_refusal, CastErrors, Spells};

/// **What the commit writes** — `SendCast 0x6e54f0`'s one branch on item-present. The sender
/// discriminates on whether the pending-cast block's guid (`0xceac48`, filled at `6e4f8d`–`6e4fa6`
/// from the ITEM's guid when TryCast was handed one, else the caster's) is the caster's, and the
/// item arm falls through to `0x6e57d8 push 0xab` = `CMSG_USE_ITEM`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CastCommit {
    /// `CMSG_CAST_SPELL 0x12e` — no item bound.
    Spell,
    /// `CMSG_USE_ITEM 0xab` — the item's wire position and template spell ordinal (decision 0666),
    /// plus the GameObject the caller bound explicitly, if any.
    Item {
        bag_index: u8,
        slot: u8,
        /// The item's template ENTRY — the not-ready rung's item-leg query key (the store's
        /// `(use_spell, itemID)` match; decision 0948, closing the entry gap 0914 named).
        entry: u32,
        spell_index: u8,
        /// The key-in-a-lock arm (decision 0769) — `CGItem::Use`'s own target argument, non-zero
        /// only for the lock chain. A caller-bound guid short-circuits the binder below, exactly
        /// as TryCast's target resolve (`6e4ef4`) takes the pair it was passed.
        on_object: Option<u64>,
    },
}

impl CastCommit {
    /// Whether this commit carries an item — the discriminator every forked rung reads.
    pub(crate) fn is_item(self) -> bool {
        matches!(self, CastCommit::Item { .. })
    }

    /// The local not-ready reason for this commit (`0x6094f0`'s forked first rung).
    fn not_ready_reason(self) -> u8 {
        if self.is_item() {
            0x28
        } else {
            0x3c
        }
    }
}

/// **The one cast-send path's whole input set, as ONE [`SystemParam`]** — so a caller needs two
/// params (this and [`cast_target::CastTargeting`]) instead of fifteen, so a new rung's input
/// lands in every caller at once, and so [`CastLadder::send`] is structurally the only way into
/// [`send_spell_cast`] (the root-cause rule: never duplicate a send path — make it impossible).
///
/// Every caster surface takes it: the action bar's two arms, the spellbook, the stance bar, the
/// trade-skill and craft windows, the bag/paper-doll item click, and the ground-targeting commit.
#[derive(SystemParam)]
pub(crate) struct CastLadder<'w, 's> {
    pub(crate) commands: Res<'w, NetCommands>,
    pub(crate) self_player:
        Query<'w, 's, (Entity, Has<crate::creature_anim::Engaged>), With<SelfPlayer>>,
    pub(crate) spells: Option<Res<'w, Spells>>,
    /// The item cache — the item arms resolve templates through it.
    pub(crate) items: Res<'w, Items>,
    /// The one object index (decision 2334) — the pre-send totem/reagent check walks the bags
    /// through it, and the item arms resolve an instance guid to its fields here.
    pub(crate) objects: Objects<'w, 's>,
    pub(crate) sheath: MessageWriter<'w, crate::creature_anim::SheathRequest>,
    pub(crate) ecs: Commands<'w, 's>,
    pub(crate) pending: ResMut<'w, crate::spell::PendingCast>,
    pub(crate) queued_melee: ResMut<'w, crate::spell::QueuedMeleeSpell>,
    pub(crate) cooldowns: ResMut<'w, crate::spell::Cooldowns>,
    /// The talent spell-modifier tables — rung 2's cost goes through them
    /// ([`super::usable::power_cost`]).
    pub(crate) spell_mods: Res<'w, crate::spell::SpellModifiers>,
    pub(crate) cast_errors: ResMut<'w, CastErrors>,
    pub(crate) auto_repeat: ResMut<'w, AutoRepeatActive>,
    pub(crate) trade_skill_opens: ResMut<'w, crate::ui_tradeskill::TradeSkillOpens>,
    pub(crate) ground: ResMut<'w, super::targeting::SpellTargeting>,
}

/// What a targeting-cursor click bound — the three things `BindLocation 0x6e60f0` /
/// `BindTarget 0x6e5b40` can fill into a standing flag_word once the ladder has already run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TargetedBind {
    /// The terrain click's point, in WoW coords (decision 0792) — `BindLocation 0x6e60f0`'s
    /// bit-6 arm.
    Dest([f32; 3]),
    /// The same terrain click's point, bound to the **source** slot instead — `BindLocation
    /// 0x6e60f0`'s bit-5 arm (`6e6105`–`6e6126`), which the reference tests *first* and which
    /// writes `SPELLCAST+0x30` and the wire bit `0x0020`. Decision 2218.
    Source([f32; 3]),
    /// The bag / paper-doll click's item guid (decision 0923).
    Item(u64),
    /// The world click's GameObject guid (decision 0939) — a chest, a door, a vein, a herb.
    Object(u64),
}

impl CastLadder<'_, '_> {
    /// **The targeting cursor's commit tail**, shared by all three seams. The ladder itself already
    /// ran when the cursor went up — the click owes only what `SendCast 0x6e54f0`'s tail owes:
    /// the packet (same block, two opcodes, [`CastCommit`] picking which), the pending arm, and
    /// the GCD. Then the word clears.
    ///
    /// One function because there is one tail: the three clicks differ in exactly one field of one
    /// packet, and a tail written three times is a tail that drifts (the lesson decision 0914 wrote
    /// up one layer above this).
    pub(crate) fn commit_targeted(
        &mut self,
        spell_id: u32,
        commit: CastCommit,
        bound: TargetedBind,
    ) {
        let cmd = match commit {
            // Two opcodes on the spell side (`CMSG_CAST_SPELL` carries the dest and the item bit
            // in different builders), one on the item side — the block itself is the same block.
            CastCommit::Spell => match bound {
                TargetedBind::Dest(dest) => ClientCommand::CastSpellAtDest { spell_id, dest },
                TargetedBind::Source(src) => ClientCommand::CastSpellAtSource { spell_id, src },
                TargetedBind::Item(item_guid) => ClientCommand::CastSpellItem {
                    spell_id,
                    item_guid,
                },
                // The same packet the right-click OPEN_LOCK path sends (decision 0239) — one
                // builder, because `BindTarget`'s GameObject arm is the one that fills the block
                // on both routes.
                TargetedBind::Object(go_guid) => {
                    ClientCommand::CastSpellGameObject { spell_id, go_guid }
                }
            },
            CastCommit::Item {
                bag_index,
                slot,
                spell_index,
                ..
            } => ClientCommand::UseItem {
                bag_index,
                slot,
                spell_index,
                target: match bound {
                    TargetedBind::Dest(dest) => UseItemTarget::Dest(dest),
                    TargetedBind::Source(src) => UseItemTarget::Source(src),
                    TargetedBind::Item(guid) => UseItemTarget::Item(guid),
                    TargetedBind::Object(guid) => UseItemTarget::Object(guid),
                },
            },
        };
        let _ = self.commands.0.send(cmd);
        let now = Instant::now();
        if commit.is_item() {
            self.pending.arm_item(spell_id, now);
        } else {
            // `normal_cast`'s own predicate — a targeted-cursor commit is always an ordinary cast
            // in practice (a ranged shot never raises the cursor), but computing it keeps this
            // arm from drifting from the ladder's if the classes ever meet.
            let guards = self
                .spells
                .as_ref()
                .and_then(|s| s.catalog.get(spell_id))
                .is_none_or(|d| !d.ranged_attack() && !d.on_next_swing());
            self.pending.arm(spell_id, now, guards);
        }
        if let Some(d) = self.spells.as_ref().and_then(|s| s.catalog.get(spell_id)) {
            self.cooldowns.start_gcd(spell_id, d, now);
        }
        self.ground.clear();
    }

    /// Run the ladder for `spell_id` and commit as `commit` says — the only entry point.
    pub(crate) fn send(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        commit: CastCommit,
    ) {
        self.send_bound(spell_id, ctx, commit, None);
    }

    /// The same ladder for a cast the **caller** has already bound to a world GameObject — the
    /// lock chain's opener (decisions 0239 / 0752 / 2199). TryCast takes its target as an ordinary
    /// argument (`6e4ef4` hands `0x612df0` the guid pair it was *passed*), and the GameObject
    /// strategy's use-sender is one of the callers that fills it: `0x5f35c0 → 0x6e5a90 →
    /// 0x6e4b60`. So this is not a second send path — it is the one path, told what the click
    /// already resolved, and the bound guid short-circuits the target binder exactly as the
    /// key-in-a-lock arm's does.
    pub(crate) fn send_at_object(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        go_guid: u64,
    ) {
        self.send_bound(spell_id, ctx, CastCommit::Spell, Some(go_guid));
    }

    fn send_bound(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        commit: CastCommit,
        on_object: Option<u64>,
    ) {
        send_spell_cast(
            spell_id,
            ctx,
            commit,
            on_object,
            &self.commands,
            &self.self_player,
            self.spells.as_deref(),
            &self.objects,
            &self.items,
            &mut self.sheath,
            &mut self.ecs,
            &mut self.pending,
            &mut self.queued_melee,
            &mut self.cooldowns,
            &self.spell_mods,
            &mut self.cast_errors,
            &mut self.auto_repeat,
            &mut self.trade_skill_opens,
            &mut self.ground,
        );
    }
}

/// Send one spell cast at the current selection — the client's local cast-send follow-through
/// (the client's `0x6e54f0` tail): a ranged-attribute spell arms the **ranged stance** now
/// (`0x6e5930`'s `SetSheatheState(2,1,1)` — the echo START re-requests it, idempotent), an
/// auto-repeat spell sets the sticky armed state (`0x6e593b`'s `|= 0x200`, the standing Load/Hold
/// idle's gate — decision 0099 phase 5), and the resolved `CMSG_CAST_SPELL` goes out. Shared by
/// `ui_action`'s `drain_action_uses` (a SPELL-kind action button) and
/// `ui_spellbook::drain_spell_casts` (a spellbook cast, decision 0216 §8) — ONE cast-send path, so
/// the follow-through can't drift between the two spell sources (the root-cause rule: never
/// duplicate a send path).
///
/// The `pending` guard is the client's optimistic in-flight refusal (`TryCast`'s IsCasting gate,
/// `6e4d97`; see [`crate::spell::PendingCast`]): a normal cast is dropped at the
/// source while one is already in flight, so mashing a key can no longer fire a duplicate
/// `CMSG_CAST_SPELL` the server bounces back as a spurious cast-bar cancel. Ranged/auto-repeat
/// shots keep their own lifecycle — they never arm the guard and are never blocked by it.
fn send_spell_cast(
    spell_id: u32,
    ctx: &cast_target::CastContext,
    commit: CastCommit,
    // The GameObject the CALLER bound, for a SPELL commit (`CastLadder::send_at_object`). The
    // item commit carries its own on `CastCommit::Item`, because `CGItem::Use` is handed the
    // lock's guid and the commit is what remembers it.
    bound_object: Option<u64>,
    commands: &NetCommands,
    self_player: &Query<(Entity, Has<crate::creature_anim::Engaged>), With<SelfPlayer>>,
    spells: Option<&Spells>,
    objects: &Objects,
    items: &Items,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    ecs: &mut Commands,
    pending: &mut crate::spell::PendingCast,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    cooldowns: &mut crate::spell::Cooldowns,
    spell_mods: &crate::spell::SpellModifiers,
    cast_errors: &mut CastErrors,
    auto_repeat: &mut AutoRepeatActive,
    trade_skill_opens: &mut crate::ui_tradeskill::TradeSkillOpens,
    ground: &mut super::targeting::SpellTargeting,
) {
    let now = Instant::now();
    let def = spells.and_then(|s| s.catalog.get(spell_id));
    // The profession-window intercept (decision 0437): `Spell_C::TryCast 0x6e4b60`'s own first
    // special branch — an `Effect[0] == SPELL_EFFECT_TRADE_SKILL`
    // cast NEVER reaches the wire; the crafting book opens client-side instead. Before the
    // cooldown ladder, exactly where the client dispatches it (`6e4bce`, ahead of every gate).
    if def.is_some_and(|d| d.effects[0] == benilla_formats::SPELL_EFFECT_TRADE_SKILL) {
        debug!("ui_action: cast {spell_id} is a profession opener — the crafting book opens, no packet");
        trade_skill_opens.0.push(spell_id);
        return;
    }
    // The button re-press toggle (`0x4e60da`):
    // re-invoking the spell that IS the running auto-repeat cancels it instead of re-casting —
    // the classic press-again-to-stop. Checked before the cooldown ladder, like the client's
    // action-button handler (which never reaches TryCast for the toggle-off).
    if def.is_some_and(|d| d.auto_repeat()) && auto_repeat.0 == Some(spell_id) {
        debug!("ui_action: cast {spell_id} re-pressed — auto-repeat toggles off");
        let self_e = self_player.single().ok().map(|(e, _)| e);
        crate::creature_anim::cancel_auto_repeat_local(self_e, auto_repeat, ecs, commands);
        return;
    }
    // TryCast's IsTargeting leg (`6e4d62`, decision 0792): a NEW cast pressed while the
    // targeting cursor is up aborts the targeting first — AbortCast in targeting mode clears
    // the word, no packet — and the press proceeds down the ladder. (The SAME spell's re-press
    // on the action bar never reaches here: UseAction's toggle-cancel returns at the drain.)
    if ground.active() {
        debug!("ui_action: cast {spell_id} supersedes the targeting cursor");
        ground.clear();
    }
    // The cast classes at this seam. A ranged/auto-repeat shot (Auto Shot, wand Shoot, Throw) is
    // not a cast-bar cast — it runs the ranged-stance / `AutoRepeatArmed` path, and its own
    // record does not guard (1601), though it is refused like any press while a guarding cast
    // holds. An on-next-swing spell (`Attributes & 0x404` — Heroic Strike, Cleave)
    // queues on the server's melee slot: it arms [`crate::spell::QueuedMeleeSpell`], never the
    // in-flight guard, so a queued strike cannot block the next cast (the ref's `6e4d97`
    // exemption on the inflight rec's 0x404 bits).
    let on_next_swing = def.is_some_and(|d| d.on_next_swing());
    let normal_cast = !def.is_some_and(|d| d.ranged_attack()) && !on_next_swing;
    // Re-pressing the queued strike is the ref's silent same-spell bail (`6e4d43`) — no cancel,
    // no error: 1.12 has no re-press-to-unqueue.
    if on_next_swing && queued_melee.current() == Some(spell_id) {
        debug!("ui_action: cast {spell_id} suppressed — already queued on next swing");
        return;
    }
    if pending.in_flight(now) {
        // The ref's already-casting refusal: the same spell bails silently (`6e4d43`); a
        // different one errors reason 0x61 "Another action is in progress" (`6e4d97` →
        // `HandleCastFailed`). The gate tests the **inflight** rec's `Attributes & 0x404`, never
        // the pressed spell's, and a
        // guarding record here is always an ordinary cast or an item use — so EVERY press class
        // is refused while it holds: an on-next-swing strike, and a ranged/auto-repeat shot too.
        // Exempting the shot let it commit and its non-guarding arm overwrite the running cast's
        // record, leaving that cast unguarded once the server refused the shot.
        if pending.current(now) != Some(spell_id) {
            cast_errors.push_local(spell_id, 0x61);
        }
        debug!("ui_action: cast {spell_id} suppressed — a cast is already in flight");
        return;
    }
    // The pre-send totem/reagent possession check (`CheckReagentsAndTotems 0x6e4000`, TryCast's
    // `0x6e4ded` — decision 0552): a missing tool (Mining Pick) or a short reagent refuses HERE
    // with the client's own 0x78/0x5c red line and NEVER sends. The gate must be local: vmangos
    // answers a sent pickless cast with the wrong code (`ITEM_GONE` "Item is gone"), so without
    // it the real message can't appear. Position pinned by 0948: TryCast runs it BEFORE
    // the validator (`0x6e4ded` precedes the `0x6e4f3b` call), so an on-cooldown press with
    // missing reagents shows the reagent error, never "not ready".
    if reagent_totem_refusal(spell_id, def, ctx.rel.self_store, objects, cast_errors) {
        return;
    }
    // **TryCast rung 7** (`0x6e4e03`, decision 1925) — the equipped-item requirement, which the
    // cast path did not have at all: the greying ladder correctly greyed the button and pressing
    // it still sent. The search is the same one the greying leg and the tooltip requirement line
    // use, so all three now agree, and the reason it refuses with is `AttributesEx3`'s own pick
    // ([`super::usable::equipped_item_reason`]).
    //
    // Position is the reference's: four rungs above the crowd-control validator below, so a
    // player who is both stunned and missing the required weapon is told about **the weapon**.
    if let Some(d) = def {
        if let Some(store) = ctx.rel.self_store {
            if !super::usable::equipped_item_fits_cached(d, store, objects, items) {
                let reason = super::usable::equipped_item_reason(d);
                debug!("ui_action: cast {spell_id} refused locally — equipped item ({reason:#x})");
                cast_errors.push_local(spell_id, reason);
                return;
            }
        }
    }
    // ArmCast (`0x6e5250`): resolve the wire target from the spell's targeting constraints —
    // never the raw selection ([`cast_target`] module docs). A refusal is local and pre-commit,
    // like the ref's residual flag_word: no send, no GCD, no pending arm, no autorepeat key.
    // A caller-bound GameObject short-circuits the walk, exactly as TryCast's target resolve
    // (`6e4ef4` → `0x612df0` over the guid pair it was PASSED) takes an explicit target: the key
    // chain calls `CGItem::Use` with the lock's guid, the bag click with zero (decision 0769).
    let mut pending_word = None;
    let mut item_target = None;
    let mut deferred_refusal = None;
    let explicit_object = match commit {
        CastCommit::Item { on_object, .. } => on_object,
        CastCommit::Spell => bound_object,
    };
    let candidates = cast_target::CastCandidates {
        selection: ctx.selection_guid,
        caster: ctx.self_guid,
        // The ref resolves its candidate guid through `0x468460(typemask 1)` (`6e53bc`) before
        // handing it to the binder: a guid naming no live object binds nothing. Ours is the one
        // object index — an equipped item whose object has not streamed yet is exactly that miss,
        // and falls to the cursor rather than shipping a guid the server would reject.
        main_hand_item: ctx
            .main_hand_item
            .filter(|guid| objects.object(*guid).is_some()),
    };
    let target = match explicit_object {
        Some(_) => None,
        None => {
            match cast_target::resolve_cast_target(def, &candidates, ctx.auto_self_cast, &ctx.rel) {
                cast_target::CastWireTarget::SelfImplicit => None,
                cast_target::CastWireTarget::Unit(guid) => Some(guid),
                // The main-hand auto-pick (decision 1552): `BindTarget`'s item arm ran at cast-arm
                // time, so this press already has its target — it rides the rest of the validator
                // ladder like any bound cast and lands in the commit's ITEM shape below. It is NOT a
                // unit, so the range gate below skips it exactly as the ref does (`0x6e47b0` guards
                // the `SPELLCAST+0x14 & 0x8202` unit binds only).
                cast_target::CastWireTarget::Item(guid) => {
                    item_target = Some(guid);
                    None
                }
                cast_target::CastWireTarget::Targeting(word) => {
                    // The cursor ENTRY is deferred below the validator rungs (decision 0948: the
                    // ref enters targeting at cast-arm `6e50c8`, AFTER the validator `0x6094f0` —
                    // an on-cooldown or unaffordable press refuses before the cursor ever comes
                    // up). The word parks here; nothing is sent, nothing armed either way. The
                    // COMMIT rides the word: the ref keeps the whole pending-cast block (the item
                    // guid at `0xceac48` included) across the cursor, so the click's `0x6e54f0`
                    // still emits USE_ITEM for a grenade / poison / key and CAST_SPELL for an
                    // enchant or opener. ONE arm, not one per seam (decisions 0792 / 0923 / 0939).
                    pending_word = Some(word);
                    None
                }
                cast_target::CastWireTarget::Refused(reason) => {
                    debug!(
                    "ui_action: cast {spell_id} refused locally — unbindable target ({reason:#x})"
                );
                    cast_errors.push_local(spell_id, reason);
                    return;
                }
                // TryCast's own tail after ArmCast returns FALSE (`6e5045`–`6e507b`) — the
                // SIBLING of the targeting-cursor arm, reached from the same place, which is
                // *below* the requirement validator. So it parks here and fires where the cursor
                // would come up (decision 1554): a mounted or shapeshifted press with an empty
                // weapon hand reads the ref's "You are mounted" / "Can't do that while
                // shapeshifted" first, exactly as 0948 established for the cursor.
                cast_target::CastWireTarget::RefusedAtArm(reason) => {
                    deferred_refusal = Some(reason);
                    None
                }
            }
        }
    };
    // The local range gate (the client's TryCast runs `CanTargetUnit 0x6e4440` →
    // `IsTargetInRange 0x6e47b0` BEFORE the commit `0x6e54f0`): an out-of-range / too-close
    // press on a bound unit target refuses here — before the ranged-stance arm below, so a
    // too-close Throw/Auto Shot never draws the bow and never stows the melee weapons (the
    // sheath snap `0x6e5930` lives in the commit tail this refusal never reaches). The bound
    // target is only ever the selection or ourselves; a self-bind (autoSelfCast) is distance 0
    // with a min-0 range in practice, so only the selection leg is tested.
    if let Some(d) = def {
        if target.is_some() && target == ctx.selection_guid && target != ctx.self_guid {
            let row = spells.and_then(|s| s.ranges.get(d.range_index));
            let dist_sq = ctx
                .range
                .self_pos
                .zip(ctx.range.target_pos)
                .map(|(a, b)| a.distance_squared(b));
            if let Some(reason) = validator::cast_range_refusal(
                d,
                row,
                ctx.range.self_reach,
                ctx.range.target_reach,
                dist_sq,
            ) {
                debug!("ui_action: cast {spell_id} refused locally — range ({reason:#x})");
                cast_errors.push_local(spell_id, reason);
                return;
            }
        }
    }
    // ── The validator `0x6094f0`'s opening rungs (decision 0948, which closed 0379's INTERIM) —
    // after IsCasting, reagents and the range test, exactly where the ref calls the validator. ──
    //
    // Rung 1 — not-ready: ONE getter query ([`crate::spell::Cooldowns::not_ready`] =
    // `GetCooldownInfo != 0`), forked by the commit at `0x60952b`: an item press queries the
    // (use-spell, item ENTRY) pair and refuses **0x28** — and per the byte law is never
    // power-gated; a spell press queries (spell, 0) and refuses **0x3c**. The GCD lock rides
    // the same query (the getter's GCD leg — `node.startRecoveryCategory == pressed's`, the
    // pressed spell's own time never consulted): refusing locally keeps the server's NOT_READY
    // fail, whose faithful revert clears the RUNNING GCD, off the wire.
    let queried_item = match commit {
        CastCommit::Item { entry, .. } => entry,
        CastCommit::Spell => 0,
    };
    if cooldowns.not_ready(spell_id, queried_item, def, now) {
        debug!("ui_action: cast {spell_id} refused locally — not ready (the validator's rung 1)");
        // The one packet a local refusal ships (`0x609576–0x60960f`, the SPELL leg only): a
        // running autorepeat whose record carries AttributesEx3 0x400000 (wand Shoot alone in
        // the 1.12 data) is stopped — CMSG_CANCEL_CAST naming it, then the local cancel. The
        // wand-stop feel: a press mid-swing reads "not ready" AND stops the wanding.
        if !commit.is_item() {
            if let Some(cached) = auto_repeat.0 {
                if spells
                    .and_then(|s| s.catalog.get(cached))
                    .is_some_and(|d| d.casting_cancels_autorepeat())
                {
                    debug!("ui_action: the not-ready refusal cancels the wand repeat {cached}");
                    let _ = commands
                        .0
                        .send(ClientCommand::CancelCast { spell_id: cached });
                    let self_e = self_player.single().ok().map(|(e, _)| e);
                    crate::creature_anim::cancel_auto_repeat_local(
                        self_e,
                        auto_repeat,
                        ecs,
                        commands,
                    );
                }
            }
        }
        cast_errors.push_local(spell_id, commit.not_ready_reason());
        return;
    }
    // Rung 2 — the power gate (`0x60962c`, SPELL presses only: the item fork's clear-query jump
    // `je 0x6096b3` lands past it): raw `UNIT_FIELD_POWER[type]` — any negative PowerType reads
    // HEALTH — signed-compared against the computed cost; reason **0x4d** ("Not enough
    // mana/rage/…", the per-power errorId family). The gate must be local: vmangos ACCEPTS the
    // doomed cast and its NO_POWER fail clears a running GCD — the phantom pie-blink on every
    // rage-starved spam press (the 0946 campaign's live capture of the loop).
    if !commit.is_item() {
        if let (Some(d), Some(store)) = (def, ctx.rel.self_store) {
            if !super::usable::can_afford(d, store, spell_mods) {
                debug!("ui_action: cast {spell_id} refused locally — not enough power (0x4d)");
                cast_errors.push_local(spell_id, 0x4d);
                return;
            }
        }
    }
    // The CROWD-CONTROL leg of the requirement validator `0x6094f0` (decision 1903, widened from
    // three arms to the reference's six by 1925), which sits ABOVE its mounted block below — so a
    // stunned mounted caster reads the stun.
    let self_fields = ctx.rel.self_store.map(|s| &s.0);
    if let Some(reason) = validator::cast_cc_refusal(
        self_fields.map_or(0, |f| f.unit_flags()),
        self_fields.and_then(|f| f.unit_health()),
        // The charm arm asks whether somebody ELSE holds the reins (`60994d`'s active-player
        // compare) — a self-charm is not something the reference refuses on.
        self_fields
            .and_then(|f| f.unit_charmed_by())
            .is_some_and(|charmer| Some(charmer) != ctx.self_guid),
        def,
        // The per-arm exemption scan (decision 1946): the caster's RAW aura slot ids — the
        // reference reads them unfiltered — joined to the spell catalog.
        &mut |aura_types: &[u32]| {
            let Some((d, fields)) = def.zip(self_fields) else {
                return benilla_formats::CcExemption::default();
            };
            let catalog = spells.as_ref().map(|s| &s.catalog);
            aura_types
                .iter()
                .map(|&ty| {
                    benilla_formats::cc_exemption(d, fields.unit_aura_ids(), ty, |id| {
                        catalog.and_then(|c| c.get(id))
                    })
                })
                // The arms scan several types; the reference's loop takes the FIRST rejection it
                // meets and stops, and reports exempt only if a match was found and accepted.
                .reduce(|a, b| if a.mechanic != 0 || a.exempt { a } else { b })
                .unwrap_or_default()
        },
    ) {
        let (reason, mechanic) = reason;
        debug!(
            "ui_action: cast {spell_id} refused locally — crowd control ({reason:#x}, \
             mechanic {mechanic:?})"
        );
        match mechanic {
            Some(m) => cast_errors.push_local_arg(spell_id, reason, m),
            None => cast_errors.push_local(spell_id, reason),
        }
        return;
    }
    // The client-side mounted gate (decision 0481;
    // TryCast's requirement validator `0x6094f0`, mounted block `0x609c6c` — a live
    // `UNIT_FIELD_MOUNTDISPLAYID` refuses a non-exempt cast with reason 0x39 "You are
    // mounted" BEFORE the cast-arm's target binding, which is why a targetless mounted click
    // never reads "You have no target"). Exemption: Attributes bit 24 (`0x01000000`,
    // castable-while-mounted). The gate must be LOCAL: vmangos silently dismounts a mounted
    // caster instead of erroring, so without this check the message can never appear. (0948
    // resolved 0481's named micro-divergence: the range gate now runs before this one, the
    // ref's own order — mounted∧out-of-range reads "Out of range." on both.)
    if validator::cast_mounted_refusal(
        ctx.rel
            .self_store
            .is_some_and(|s| s.0.unit_mount_display_id() > 0),
        def,
    ) {
        debug!("ui_action: cast {spell_id} refused locally — mounted (0x39)");
        cast_errors.push_local(spell_id, 0x39);
        return;
    }
    // The ENVIRONMENT leg of the SAME requirement validator (`0x609d39–0x609d7b`, between the
    // mounted block above and the moving block below; decision 1056): a spell whose aura cannot
    // survive the caster's current side of the water surface refuses locally — 0x50 "Cannot use
    // while swimming" for the mount/Travel-Form/food set, 0x58 "Can only use while swimming" for
    // Aquatic Form. The gate must be local: vmangos's CheckCast has no shapeshift or food water
    // arm at all, so without this the server grants a druid aquatic form standing on dry land
    // (ledger B176) and lets you eat mid-swim (B155's eating half). The condition is [`validator`]'s.
    if let Some(reason) = validator::cast_water_refusal(ctx.self_move_flags, def) {
        debug!("ui_action: cast {spell_id} refused locally — water side ({reason:#x})");
        cast_errors.push_local(spell_id, reason);
        return;
    }
    // The moving leg of the SAME requirement validator (`0x609de3`, after the mounted/posture/
    // environment blocks, before the form leg — decision 0862): a
    // cast-time (or movement-sensitive) press while already moving refuses locally with the
    // client's own reason 0x2e "Can't do that while moving" and NEVER sends. The gate must be
    // local: vmangos accepts the sent cast (its CheckCast moving-reject covers only
    // autorepeat/sit-still spells) and then its movement interrupt cancels it mid-bar — the
    // start-then-die cast bar this gate removes. The full condition is [`validator`]'s.
    if let Some(d) = def {
        let caster_level = ctx
            .rel
            .self_store
            .and_then(|s| s.0.unit_level())
            .unwrap_or(0);
        let cast_time_ms = spells.map_or(0, |s| s.cast_time_ms(d, caster_level));
        if validator::cast_moving_refusal(ctx.self_move_flags, cast_time_ms, def) {
            debug!("ui_action: cast {spell_id} refused locally — moving (0x2e)");
            cast_errors.push_local(spell_id, 0x2e);
            return;
        }
    }
    // The shapeshift-form leg of the SAME requirement validator (`0x6094f0` at `0x609e49` →
    // the form gate `0x612480`, not `0x609ca2` — that address is the POSTURE gate, reason
    // 0x3e NOT_STANDING; vmangos corroborates the reason split,
    // `SpellEntry::GetErrorAtShapeshiftedCast`): a form-blocked press refuses locally with the
    // gate's own red line — 0x3d "Can't do that while shapeshifted" / 0x56 needs-a-form — and
    // never sends, exactly like the mounted leg above. This is the whole Ghost Wolf experience:
    // ordinary spells carry NOT_SHAPESHIFT (verified in the 5875 data), so a shifted shaman's
    // every press lands here.
    if let Some(d) = def {
        let form = ctx
            .rel
            .self_store
            .map(|s| s.0.unit_shapeshift_form())
            .unwrap_or(0);
        let form_is_stance = spells
            .and_then(|s| s.forms.get(&u32::from(form)))
            .is_some_and(|f| f.is_stance());
        if let Some(refusal) = d.form_refusal(form, form_is_stance) {
            let reason = refusal.reason();
            debug!("ui_action: cast {spell_id} refused locally — the form gate ({reason:#x})");
            cast_errors.push_local(spell_id, reason);
            return;
        }
    }
    // The deferred ArmCast-FALSE refusal (`6e5050`), the cursor arm's sibling: every validator
    // rung above has passed and the bind still found nothing bindable, so the reference's own
    // red line goes out here — no packet, no GCD, no pending arm (decision 1554).
    if let Some(reason) = deferred_refusal {
        debug!("ui_action: cast {spell_id} refused locally — the cast-arm tail ({reason:#x})");
        cast_errors.push_local(spell_id, reason);
        return;
    }
    // The deferred targeting-cursor entry (the ref's cast-arm position `6e50c8`): every
    // validator rung above has passed — NOW the cursor comes up and waits for its click.
    if let Some(word) = pending_word {
        debug!("ui_action: cast {spell_id} awaits its click — targeting cursor up ({word:#06x})");
        ground.enter(spell_id, commit, word);
        return;
    }
    // The wand-only auto-repeat handoff (the client's `0x60959e` inside TryCast's `0x6094f0`
    // step): a NEW cast cancels the running auto-repeat
    // iff the CACHED spell carries `AttributesEx3 & 0x400000` — wand Shoot 5019 alone in the
    // 1.12 data. Auto Shot survives by construction: hunter shot-weaving. The client first
    // sends `CMSG_CANCEL_CAST` naming the cached wand spell (`0x6095b8`), then runs the local
    // cancel (whose own `CMSG_CANCEL_AUTO_REPEAT` ack follows). The same-spell re-press never
    // reaches here — the toggle above returned.
    if let Some(cached) = auto_repeat.0 {
        if spells
            .and_then(|s| s.catalog.get(cached))
            .is_some_and(|d| d.casting_cancels_autorepeat())
        {
            debug!("ui_action: cast {spell_id} cancels the running wand repeat {cached}");
            let _ = commands
                .0
                .send(ClientCommand::CancelCast { spell_id: cached });
            let self_e = self_player.single().ok().map(|(e, _)| e);
            crate::creature_anim::cancel_auto_repeat_local(self_e, auto_repeat, ecs, commands);
        }
    }
    if let Some(d) = def {
        if let Ok((e, engaged)) = self_player.single() {
            if d.ranged_attack() {
                sheath.write(crate::creature_anim::SheathRequest {
                    entity: e,
                    state: 2,
                    ceremony: false,
                });
            }
            if d.auto_repeat() {
                ecs.entity(e).insert(crate::creature_anim::AutoRepeatArmed);
                // The live autorepeat key (`0xceac30 = SpellRec+0x00` at `0x6e5947`) — what the
                // button's flash/checked state reads until CANCEL_AUTO_REPEAT clears it.
                auto_repeat.0 = Some(spell_id);
                // The commit's own **StopAttack** (`0x6e5976`, guarded by nothing but "the caster
                // is the active player" — the inlined guid compare at `0x6e5957`–`0x6e5972` that
                // `0x5ecac0` then re-does): starting an auto-repeat ends the melee auto-attack and
                // takes the queued on-next-swing strike down with it. This is the end at which
                // the client's melee ⟷ auto-repeat exclusion is actually enforced (decision
                // 1029): the attack-start tail below cancels a repeat only on a press that
                // *starts* the swing, so this call is what keeps "swinging and auto-shooting at
                // once" unreachable — and it is why the reference un-checks both the Attack
                // button and a queued Raptor Strike the instant Auto Shot starts. The arm above
                // runs first, so the repeat we just started is never the thing this cancels.
                crate::creature_anim::stop_attack_local(engaged, queued_melee, commands);
            }
        }
    }
    // The commit's ONE branch (`SendCast 0x6e54f0`): same block, two opcodes.
    let _ = commands.0.send(match commit {
        CastCommit::Spell => match (explicit_object, item_target) {
            // The lock chain's opener, bound by the click that resolved it (decision 2199) — the
            // same builder `commit_targeted`'s `TargetedBind::Object` reaches, because
            // `BindTarget`'s GameObject arm fills the block on both routes.
            (Some(go_guid), _) => ClientCommand::CastSpellGameObject { spell_id, go_guid },
            // `SendCast 0x6e54f0`'s item leg — the same block the bag click's commit reaches, just
            // arrived at without a click (decision 1552).
            (None, Some(item_guid)) => ClientCommand::CastSpellItem {
                spell_id,
                item_guid,
            },
            (None, None) => ClientCommand::CastSpell { spell_id, target },
        },
        CastCommit::Item {
            bag_index,
            slot,
            spell_index,
            on_object,
            ..
        } => ClientCommand::UseItem {
            bag_index,
            slot,
            spell_index,
            target: match (on_object, item_target, target) {
                (Some(go), _, _) => UseItemTarget::Object(go),
                (None, Some(item), _) => UseItemTarget::Item(item),
                (None, None, Some(unit)) => UseItemTarget::Unit(unit),
                (None, None, None) => UseItemTarget::SelfImplicit,
            },
        },
    });
    // **One inflight id, every cast source** (`0xceca88`, written at `0x6e5026` for every commit).
    // The item arm's provisional is shorter because `CMSG_USE_ITEM` has legs vmangos answers with
    // `SMSG_INVENTORY_CHANGE_FAILURE` and no cast result at all —
    // [`crate::spell::PendingCast`]'s own doc, decision 0908.
    //
    // **Every** class is recorded; only `normal_cast` and the item arm *guard* (1601). This used
    // to be one thing: the record was armed only for the classes that refuse on it, so a ranged
    // shot — every hunter shot is `Attributes & 0x2` — left no trace of having been sent, and the
    // `modalNextSpell` chain's "is this reply mine?" test (`0x6e7408`) could never answer yes.
    if commit.is_item() {
        pending.arm_item(spell_id, now);
    } else {
        pending.arm(spell_id, now, normal_cast);
    }
    if on_next_swing {
        queued_melee.arm(spell_id);
    }
    // TryCast's post-send tail (`6e51b5`): a committed send whose rec passes
    // [`SpellDisplay::initiates_auto_attack`] (on-next-swing `0x404` or `AttributesEx & 0x200`,
    // and not the GO-deferred Ex2-bit20; Charge carries none) starts the melee auto-attack at
    // the cast's bound unit target, unless one is already running (`0x60ecb0` over
    // `[player+0xc48]`; our mirror is the wire-echoed `Engaged`). The start is the attack path's
    // own pair (`0x6131a0` → `0x5ecb70`): melee-sheath SNAP + `CMSG_ATTACKSWING` — the same two
    // edges as the Attack button's arm in `ui_action`'s `drain_action_uses`. Path-independent
    // in the ref (button/spellbook/CastSpellByName share the one tail) — matching our one send
    // seam.
    if let (Some(d), Some(guid)) = (def, target) {
        if d.initiates_auto_attack() {
            if let Ok((e, engaged)) = self_player.single() {
                // **`!engaged` is the TAIL's own gate, not StartAttack's** — the distinction cost
                // decision 1028 a wrong half. `Attack 0x5ecb70`'s already-attacking test at
                // `0x5eccda` really does skip only the `CMSG_ATTACKSWING` send, jumping *into*
                // the tail at `0x5ecd78` whose `0x5ecd8c` cancels the auto-repeat — but a cast
                // press never reaches that jump. TryCast's tail gates the CALL: `6e51cb call
                // 0x60ecb0; 6e51d2 jne` skips `0x6131a0` outright when an attack is already
                // running: a strike pressed while already engaged does nothing to a running
                // auto-repeat. So the whole block is gated,
                // and the exclusion holds from the other end instead — the commit's StopAttack
                // above means an auto-repeat can't be running while we swing in the first place.
                if !engaged {
                    debug!("ui_action: cast {spell_id} initiates auto-attack at {guid:#x}");
                    // No stop is ever in flight on this route: the gate above IS the ref's
                    // `6e51d2`, so we only get here with the lock clear.
                    crate::creature_anim::start_attack_local(
                        e,
                        guid,
                        engaged,
                        false,
                        auto_repeat,
                        sheath,
                        ecs,
                        commands,
                    );
                }
            }
        }
    }
    // Arm the GCD at send (`StartGlobalCooldown 0x6e2de0` ← the cast-send arm `0x6e58fb`,
    // byte-verified) — a later `SMSG_CAST_RESULT` failure clears it again (`0x6e1630`).
    if let Some(d) = def {
        cooldowns.start_gcd(spell_id, d, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Reputations;
    use crate::ui_action::CastFail;
    use bevy::ecs::system::RunSystemOnce;
    use crossbeam_channel::Receiver;

    const HEARTHSTONE: u32 = 8690;
    const MOUNT: u32 = 470;
    /// The hearthstone's own wire position on a fresh character: player array, backpack slot 1.
    const HEARTH_COMMIT: CastCommit = CastCommit::Item {
        bag_index: 255,
        slot: 24,
        entry: 6948,
        spell_index: 0,
        on_object: None,
    };

    static EMPTY_REPUTATIONS: Reputations = Reputations(Vec::new());

    /// A context with nothing selected — every rung that needs world state is inert, so these
    /// tests pin the two rungs that fork on the commit and the commit itself.
    fn ctx() -> cast_target::CastContext<'static> {
        cast_target::CastContext {
            selection_guid: None,
            self_guid: Some(0x0000_0000_0000_0007),
            auto_self_cast: false,
            rel: cast_target::TargetRelations {
                target_store: None,
                self_store: None,
                factions: None,
                reputations: &EMPTY_REPUTATIONS,
            },
            range: cast_target::RangeInputs::default(),
            main_hand_item: None,
            self_move_flags: 0,
        }
    }

    /// A World carrying exactly the resources [`CastLadder`] gathers. No `Spells` — an absent
    /// catalog makes `def` `None`, which is the shape of every rung that guards on `if let
    /// Some(d) = def`: they are inert, and what is left is the ladder's spine.
    fn world() -> (World, Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<Items>();
        // The one object index the ladder's item arms resolve an instance guid through (2334).
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::spell::PendingCast>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::Cooldowns>();
        world.init_resource::<crate::spell::SpellModifiers>();
        world.init_resource::<CastErrors>();
        world.init_resource::<AutoRepeatActive>();
        world.init_resource::<crate::ui_tradeskill::TradeSkillOpens>();
        world.init_resource::<crate::spell::SpellTargeting>();
        // The sheath request the commit tail writes for a ranged spell — a bare World has no
        // message storage until it is asked for.
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<Messages<crate::player::StandStateRequest>>();
        (world, rx)
    }

    fn send(world: &mut World, spell_id: u32, commit: CastCommit) {
        world
            .run_system_once(move |mut ladder: CastLadder| {
                ladder.send(spell_id, &ctx(), commit);
            })
            .expect("the ladder runs as a one-shot system");
    }

    fn send_at_go(world: &mut World, spell_id: u32, go_guid: u64) {
        world
            .run_system_once(move |mut ladder: CastLadder| {
                ladder.send_at_object(spell_id, &ctx(), go_guid);
            })
            .expect("the ladder runs as a one-shot system");
    }

    /// **The mashed chest, at the ladder** (decision 2199). Right-clicking a lockable GameObject
    /// resolves to a known `OPEN_LOCK` spell cast *at the object*; before this the arm sent its own
    /// packet, so every extra click shipped another `CMSG_CAST_SPELL`, vmangos answered each one
    /// `SPELL_FAILED_SPELL_IN_PROGRESS`, and that failure — naming the **same** spell as the cast
    /// still running — red-faded its bar while the chest opened anyway.
    ///
    /// Two halves, both the reference's: the bound guid reaches the wire as the GameObject block
    /// (`BindTarget`'s GO arm), and the re-click is `6e4d43`'s **silent** same-spell bail — no
    /// packet, and no red line either, because the client refusing itself says nothing.
    #[test]
    fn a_mashed_gameobject_opener_never_ships_the_duplicate() {
        const OPENING: u32 = 6478;
        const CHEST: u64 = 0xF110_0000_0000_1234;
        let (mut world, rx) = world();

        send_at_go(&mut world, OPENING, CHEST);
        assert!(
            matches!(
                rx.try_recv(),
                Ok(ClientCommand::CastSpellGameObject { spell_id, go_guid })
                    if spell_id == OPENING && go_guid == CHEST
            ),
            "the first click commits as a GameObject-targeted cast and arms the one inflight id"
        );

        send_at_go(&mut world, OPENING, CHEST);
        assert!(rx.try_recv().is_err(), "no duplicate on the wire");
        assert!(
            world.resource::<CastErrors>().0.is_empty(),
            "the same spell's re-press is the ref's SILENT bail (6e4d43), not \"Another action is \
             in progress\""
        );

        // The control: a *different* cast mid-opener is the loud refusal (6e4d97), unchanged.
        send(&mut world, MOUNT, CastCommit::Spell);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            world.resource::<CastErrors>().0,
            vec![CastFail::local(MOUNT, 0x61)],
            "a different spell mid-cast still errors 0x61"
        );
    }

    // ── The melee ⟷ auto-repeat exclusion (the 5875 image, read 2026-08-06) ──────────────────
    //
    // Two client-local calls make Auto Shot and a queued Raptor Strike mutually exclusive, and
    // benilla had neither shape right: the commit's auto-repeat arm calls **StopAttack**
    // (`0x6e5976` → `0x5ecac0` → `CancelQueuedCast 0x6e6f30`), and the melee attack-start's tail
    // (`0x5ecd78`–`0x5ecd95`) cancels the running repeat **whether or not** the
    // already-attacking test at `0x5eccda` skipped the swing send. The tests below pin both, and
    // the two orders they compose into.

    const AUTO_SHOT: u32 = 75;
    const RAPTOR_STRIKE: u32 = 2973;
    const SERPENT_STING: u32 = 1978;
    const MOB: u64 = 0x0000_0000_0000_00f1;

    /// The two combat-initiation classes exactly as the real 5875 rows classify them (the data
    /// itself is pinned by `benilla_formats`' `real_spell_catalog_classifies_combat_initiation`):
    /// Auto Shot is `AttributesEx2 & 0x20` auto-repeat over `Attributes & 0x2` ranged, Raptor
    /// Strike is `Attributes & 0x404` on-next-swing. `Targets & 0x2` is the generic UNIT bind —
    /// the one candidate leg that needs no relation data ([`cast_target::clear_satisfied_bits`]),
    /// so the selection binds here without a faction table standing in for the world.
    fn combat_catalog() -> Spells {
        use std::collections::HashMap;
        let auto_shot = benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes: 0x2,
            attributes_ex2: 0x20,
            ..Default::default()
        };
        let raptor_strike = benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes: 0x404,
            ..Default::default()
        };
        // The third class the seam has to keep apart: an instant hunter shot. Ranged slot
        // (`Attributes & 0x2`) like Auto Shot, but NOT auto-repeat — and it names Auto Shot in
        // `modalNextSpell` (1597).
        let serpent_sting = benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes: 0x0001_0002,
            attributes_ex2: 0x0002_0000,
            modal_next_spell: AUTO_SHOT,
            ..Default::default()
        };
        Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(HashMap::from([
                (AUTO_SHOT, auto_shot),
                (RAPTOR_STRIKE, raptor_strike),
                (SERPENT_STING, serpent_sting),
            ])),
            ..Spells::empty_for_tests()
        }
    }

    /// **A committed cast is recorded as committed — whatever its class** (`0xceca88` is written
    /// at `0x6e5026` for every commit; decision 1601).
    ///
    /// This is the test whose absence shipped B280 broken **twice**. 1597 built the
    /// `modalNextSpell` chain and unit-tested it by arming the in-flight record **by hand** — so
    /// it passed while the real send path never armed it for a hunter shot at all
    /// (`normal_cast = !ranged_attack && !on_next_swing`, and every hunter shot is
    /// `Attributes & 0x2`). The chain's "is this reply mine?" test could not answer yes in play,
    /// and the fix looked green and did nothing. So this goes through [`send`], the real ladder.
    ///
    /// The control is the other half: recorded is not the same as *guarding*. A ranged shot must
    /// not start refusing the next press with `0x61`, which is what arming the old field for it
    /// would have done.
    #[test]
    fn a_committed_ranged_shot_is_recorded_but_does_not_guard() {
        let (mut world, _rx) = combat_world(false);
        send_at(&mut world, SERPENT_STING, MOB);
        let now = Instant::now();
        let pending = world.resource::<crate::spell::PendingCast>();
        assert_eq!(
            pending.committed(now),
            Some(SERPENT_STING),
            "the shot we just sent is the committed cast — this is what `0x6e7408` reads"
        );
        assert!(
            !pending.in_flight(now),
            "...and it does not occupy the refusal: a shot must not block the next press"
        );
        assert_eq!(
            pending.current(now),
            None,
            "...nor light the in-flight ring the ordinary casts drive"
        );

        // An ordinary cast still does both, unchanged.
        let (mut world, _rx) = combat_world(false);
        send_at(&mut world, RAPTOR_STRIKE, MOB);
        let pending = world.resource::<crate::spell::PendingCast>();
        assert_eq!(
            pending.committed(Instant::now()),
            Some(RAPTOR_STRIKE),
            "an on-next-swing strike is committed too"
        );
    }

    /// **The chained Auto Shot survives the ladder** — the wall after the one 1601 knocked down.
    /// Recording the sting as committed is only half: the chain then hands 75 back through
    /// [`CastLadder::send`], which re-runs every rung. If any of them refused it — the
    /// auto-repeat toggle, the in-flight guard the sting just armed, the GCD the sting just
    /// started — the chain would still end in silence, and silence is what the bug looked like.
    ///
    /// So: send the sting, then send what its `modalNextSpell` names, and require the wire to
    /// carry it. The sting's own GCD is running by then (it is armed at the commit above), which
    /// is exactly the state the real chain fires in — Auto Shot's `StartRecoveryCategory` is 0
    /// against the GCD node's 133, so the getter's GCD leg cannot match it.
    #[test]
    fn the_chained_auto_shot_is_not_refused_by_the_sting_it_followed() {
        let (mut world, rx) = combat_world(false);
        send_at(&mut world, SERPENT_STING, MOB);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == SERPENT_STING),
            "the sting goes out first"
        );
        // What `cast_result` returns, and `on_cast_result` hands to the ladder (2330).
        send_at(&mut world, AUTO_SHOT, MOB);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == AUTO_SHOT),
            "and the chained Auto Shot goes out behind it — no rung eats it"
        );
        assert_eq!(
            world.resource::<AutoRepeatActive>().0,
            Some(AUTO_SHOT),
            "...and the commit arms the repeat, which is the whole observable"
        );
    }

    /// **A shot pressed mid-cast is refused, and the cast keeps its guard.** The reference's
    /// already-casting gate (`6e4d97`)
    /// tests the **inflight** rec's `Attributes & 0x404`, never the pressed spell's — so a wand
    /// Shoot / Auto Shot pressed during a Frostbolt meets the same `0x61` as any other press.
    /// Ours exempted the ranged class at the gate, so the shot went out, its non-guarding arm
    /// overwrote the Frostbolt's record, vmangos refused the shot `SPELL_IN_PROGRESS`, and the
    /// still-running Frostbolt was left unguarded for the next mashed key.
    #[test]
    fn a_shot_pressed_mid_cast_is_refused_and_the_cast_keeps_its_guard() {
        const FROSTBOLT: u32 = 116;
        let (mut world, rx) = combat_world(false);
        world
            .resource_mut::<crate::spell::PendingCast>()
            .arm(FROSTBOLT, Instant::now(), true);

        send_at(&mut world, AUTO_SHOT, MOB);

        assert!(rx.try_recv().is_err(), "nothing goes out on the wire");
        assert_eq!(
            world.resource::<CastErrors>().0,
            vec![CastFail::local(AUTO_SHOT, 0x61)],
            "a different spell mid-cast is \"Another action is in progress\", whatever its class"
        );
        assert_eq!(
            world
                .resource::<crate::spell::PendingCast>()
                .current(Instant::now()),
            Some(FROSTBOLT),
            "the Frostbolt still holds the guard"
        );
        assert_eq!(
            world.resource::<AutoRepeatActive>().0,
            None,
            "and the refused shot armed no repeat"
        );
    }

    /// [`world`] plus the catalog and the self player the commit tail needs — `engaged` is our
    /// server-echoed mirror of the ref's attack lock `[+0xc48]`.
    fn combat_world(engaged: bool) -> (World, Receiver<ClientCommand>) {
        let (mut world, rx) = world();
        world.insert_resource(combat_catalog());
        let mut me = world.spawn(SelfPlayer);
        if engaged {
            me.insert(crate::creature_anim::Engaged(0));
        }
        (world, rx)
    }

    fn send_at(world: &mut World, spell_id: u32, target: u64) {
        world
            .run_system_once(move |mut ladder: CastLadder| {
                let ctx = cast_target::CastContext {
                    selection_guid: Some(target),
                    ..ctx()
                };
                ladder.send(spell_id, &ctx, CastCommit::Spell);
            })
            .expect("the ladder runs as a one-shot system");
    }

    /// **The commit's StopAttack** (`0x6e5976`): starting an auto-repeat ends the melee swing and
    /// takes the queued on-next-swing strike with it — `CMSG_ATTACKSTOP` from `0x624370`, then the
    /// tail jump into `CancelQueuedCast 0x6e6f30`, whose `Attributes & 0x404` leg hands the queued
    /// id to `CancelCast 0x6e4940(dl=1)` and its `CMSG_CANCEL_CAST`. Both buttons cannot be lit at
    /// once; benilla used to leave the strike's ring on because this call did not exist here.
    #[test]
    fn an_auto_repeat_press_stops_the_attack_and_takes_the_queued_strike_with_it() {
        let (mut world, rx) = combat_world(true);
        world
            .resource_mut::<crate::spell::QueuedMeleeSpell>()
            .arm(RAPTOR_STRIKE);

        send_at(&mut world, AUTO_SHOT, MOB);

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::AttackStop)),
            "the auto-repeat arm calls StopAttack before the cast leaves"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CancelCast { spell_id }) if spell_id == RAPTOR_STRIKE),
            "the StopAttack tail cancels the queued strike by id"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == AUTO_SHOT),
            "and then the shot itself commits"
        );
        assert_eq!(
            world.resource::<crate::spell::QueuedMeleeSpell>().current(),
            None,
            "the queue is empty — the strike's checked ring goes dark"
        );
    }

    /// `0x5ecac0`'s early-out (`!IsAttacking && [+0xc50] == 0`): with no swing running there is
    /// nothing to stop and nothing is sent — a queued strike, which cannot exist without a swing
    /// to fire it, survives untouched.
    #[test]
    fn an_auto_repeat_press_with_no_swing_running_stops_nothing() {
        let (mut world, rx) = combat_world(false);
        world
            .resource_mut::<crate::spell::QueuedMeleeSpell>()
            .arm(RAPTOR_STRIKE);

        send_at(&mut world, AUTO_SHOT, MOB);

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == AUTO_SHOT),
            "the shot, and only the shot"
        );
        assert!(rx.try_recv().is_err(), "no ATTACKSTOP, no CANCEL_CAST");
        assert_eq!(
            world.resource::<crate::spell::QueuedMeleeSpell>().current(),
            Some(RAPTOR_STRIKE)
        );
    }

    /// **The other direction, and the correction decision 1028 needed**. `Attack 0x5ecb70`'s tail
    /// really does cancel the auto-repeat on a path that skipped the swing send — but a cast press
    /// never reaches it while engaged: TryCast's tail gates the CALL, `6e51cb call 0x60ecb0;
    /// 6e51d2 jne`, so `0x6131a0` is not invoked at all. A strike pressed mid-swing therefore does
    /// nothing to a running auto-repeat, and 1028's un-gated tail was wrong to make it.
    ///
    /// The state this pins is unreachable in ordinary play *because* of the sibling test above —
    /// starting Auto Shot stops the swing — which is the point: the exclusion is enforced at one
    /// end, not both.
    #[test]
    fn a_strike_pressed_while_already_swinging_reaches_no_attack_start_at_all() {
        let (mut world, rx) = combat_world(true);
        world.resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);

        send_at(&mut world, RAPTOR_STRIKE, MOB);

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == RAPTOR_STRIKE)
        );
        assert!(
            rx.try_recv().is_err(),
            "no ATTACKSWING and no CANCEL_AUTO_REPEAT — `6e51d2 jne` skipped the whole attack-start"
        );
        assert_eq!(
            world.resource::<AutoRepeatActive>().0,
            Some(AUTO_SHOT),
            "the repeat survives: the reference cancels it via the attack it STARTS, and it started none"
        );
        assert_eq!(
            world.resource::<crate::spell::QueuedMeleeSpell>().current(),
            Some(RAPTOR_STRIKE),
            "the strike still queues — only the attack-start was skipped"
        );
    }

    /// The same press from the state that *is* reachable: not yet swinging. Here the tail fires,
    /// `0x6131a0` → `0x5ecb70` runs, and its `0x5ecd8c` kills the repeat on the way to the swing —
    /// a transitive route that a census of StartAttack's direct callers misses.
    #[test]
    fn a_strike_pressed_before_the_swing_starts_kills_the_auto_repeat() {
        let (mut world, rx) = combat_world(false);
        world.resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);

        send_at(&mut world, RAPTOR_STRIKE, MOB);

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == RAPTOR_STRIKE)
        );
        // The reference's order, which the hand-rolled copy this seam replaced had backwards:
        // the swing send is `0x5eccfd`, the auto-repeat cancel is `0x5ecd8c` in the tail below it.
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::AttackSwing { guid }) if guid == MOB),
            "the swing goes out first"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CancelAutoRepeat)),
            "then the attack-start tail's `0x6ea080`"
        );
        assert_eq!(
            world.resource::<AutoRepeatActive>().0,
            None,
            "Auto Shot's ring goes dark"
        );
    }

    /// **B200's regression test, at the ladder** (decisions 0908/0914): an item use is a cast
    /// through the same `TryCast` a spell press runs, so the in-flight rung (`6e4d97`) refuses the
    /// double-click — the same spell silently (`6e4d43`), a different one with 0x61 — and neither
    /// reaches the wire. Before this, the duplicate `CMSG_USE_ITEM` drew
    /// `SPELL_FAILED_SPELL_IN_PROGRESS` naming the *running* cast's own spell, which red-faded its
    /// bar while the cast completed anyway.
    #[test]
    fn a_double_clicked_item_never_ships_the_duplicate() {
        let (mut world, rx) = world();

        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::UseItem { .. })),
            "the first click commits as USE_ITEM and arms the one inflight id"
        );

        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert!(rx.try_recv().is_err(), "no duplicate on the wire");
        assert!(
            world.resource::<CastErrors>().0.is_empty(),
            "the same spell's re-press is the ref's SILENT bail (6e4d43), not a red line"
        );

        send(&mut world, MOUNT, HEARTH_COMMIT);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            world.resource::<CastErrors>().0,
            vec![CastFail::local(MOUNT, 0x61)],
            "a different spell mid-cast is 6e4d97's \"Another action is in progress\""
        );
    }

    /// The one rung that forks on item-present: `0x6094f0`'s first leg branches at `60952b` on its
    /// item argument — item → the item cooldown query and reason **0x28** "Item is not ready yet."
    /// (`609549`); no item → the spell query and **0x3c** (`609616`). Ours is one cooldown store
    /// with two reasons. This rung used to live in the action bar's ITEM arm alone, so a bag or
    /// paper-doll click ignored cooldowns entirely and shipped a doomed packet (decision 0914).
    #[test]
    fn the_not_ready_reason_forks_on_item_present() {
        let (mut world, rx) = world();
        let use_spell = benilla_protocol::messages::ItemUseSpell {
            spell_id: HEARTHSTONE,
            cooldown_ms: 1_800_000,
            category: 0,
            category_cooldown_ms: 0,
        };
        world.resource_mut::<crate::spell::Cooldowns>().start_item(
            6948,
            &use_spell,
            None,
            Instant::now(),
        );

        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert!(rx.try_recv().is_err(), "an item on cooldown never sends");
        assert_eq!(
            world.resource::<CastErrors>().0,
            vec![CastFail::local(HEARTHSTONE, 0x28)]
        );

        // The byte law's other half (0948, correcting this test's earlier shape): the record is
        // keyed (use-spell, item ENTRY), and a bare SPELL press queries (spell, 0) — the item
        // record does NOT match it, so the press passes the rung and commits. (One store, two
        // KEYS — no longer "one store keyed by spell id for both".)
        world.resource_mut::<CastErrors>().0.clear();
        world.insert_resource(crate::spell::PendingCast::default());
        send(&mut world, HEARTHSTONE, CastCommit::Spell);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { .. })),
            "the item-keyed record never not-readies a bare spell press"
        );
        assert!(world.resource::<CastErrors>().0.is_empty());
    }

    /// The commit's own branch (`SendCast 0x6e54f0`): one ladder, one targets block, two opcodes.
    /// The key-in-a-lock arm's caller-bound GameObject short-circuits the binder and rides the
    /// block as `TARGET_FLAG_GAMEOBJECT|LOCKED` (decision 0769), which is why `on_object` is part
    /// of the commit rather than a separate send.
    #[test]
    fn the_commit_picks_the_opcode_and_the_block() {
        use benilla_protocol::messages::UseItemTarget;
        let (mut world, rx) = world();

        send(&mut world, HEARTHSTONE, CastCommit::Spell);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell {
                spell_id: HEARTHSTONE,
                target: None
            })
        ));

        // `init_resource` would keep the armed guard — the point here is the commit, not the
        // in-flight rung the test above owns.
        world.insert_resource(crate::spell::PendingCast::default());
        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::UseItem {
                bag_index: 255,
                slot: 24,
                spell_index: 0,
                target: UseItemTarget::SelfImplicit
            })
        ));

        world.insert_resource(crate::spell::PendingCast::default());
        send(
            &mut world,
            HEARTHSTONE,
            CastCommit::Item {
                bag_index: 255,
                slot: 81,
                entry: 6948,
                spell_index: 0,
                on_object: Some(0xF110_000C_1F00_A3B2),
            },
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::UseItem {
                slot: 81,
                target: UseItemTarget::Object(0xF110_000C_1F00_A3B2),
                ..
            })
        ));
    }

    /// The validator's power gate (0948, `0x60962c`): a SPELL press the caster
    /// cannot afford refuses locally with 0x4d and never wires — the gate vmangos cannot supply
    /// (it ACCEPTS the doomed cast, and its NO_POWER fail would clear a running GCD: the
    /// phantom pie-blink on rage-starved spam). An ITEM press skips the gate entirely (the item
    /// fork's clear-query jump lands past it).
    #[test]
    fn an_unaffordable_press_refuses_locally_and_items_skip_the_gate() {
        use crate::net::ObjectStore;
        use benilla_formats::SpellDisplay;
        use benilla_protocol::ObjectFields;

        let (mut world, rx) = world();
        let mut displays = std::collections::HashMap::new();
        displays.insert(
            HEARTHSTONE,
            SpellDisplay {
                power_type: 0,
                mana_cost: 500,
                ..Default::default()
            },
        );
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(displays);
        world.insert_resource(spells);
        // A caster holding 100 mana (field 23 = UNIT_FIELD_POWER1). Leaked: the one-shot
        // system closure must be 'static, and this is a test.
        let store: &'static ObjectStore =
            Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(&[
                (22u16, 100u32),
                (23, 100),
            ]))));

        let base = ctx();
        let with_store = cast_target::CastContext {
            rel: cast_target::TargetRelations {
                self_store: Some(store),
                ..base.rel
            },
            ..base
        };
        world
            .run_system_once(move |mut ladder: CastLadder| {
                ladder.send(HEARTHSTONE, &with_store, CastCommit::Spell);
            })
            .expect("one-shot");
        assert!(rx.try_recv().is_err(), "an unaffordable press never sends");
        assert_eq!(
            world.resource::<CastErrors>().0,
            vec![CastFail::local(HEARTHSTONE, 0x4d)]
        );

        world.resource_mut::<CastErrors>().0.clear();
        let base = ctx();
        let with_store = cast_target::CastContext {
            rel: cast_target::TargetRelations {
                self_store: Some(store),
                ..base.rel
            },
            ..base
        };
        world
            .run_system_once(move |mut ladder: CastLadder| {
                ladder.send(HEARTHSTONE, &with_store, HEARTH_COMMIT);
            })
            .expect("one-shot");
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::UseItem { .. })),
            "an item press is never power-gated"
        );
        assert!(world.resource::<CastErrors>().0.is_empty());
    }

    /// The rung ORDER (0948's C6): the in-flight refusal (`0x61`, TryCast's IsCasting) precedes
    /// the validator's not-ready rung — an on-cooldown press made mid-cast reads "Another
    /// action is in progress", never "not ready".
    #[test]
    fn in_flight_outranks_not_ready() {
        let (mut world, rx) = world();
        // Spell 100 on a long cooldown…
        let use_spell = benilla_protocol::messages::ItemUseSpell {
            spell_id: MOUNT,
            cooldown_ms: 60_000,
            category: 0,
            category_cooldown_ms: 0,
        };
        world.resource_mut::<crate::spell::Cooldowns>().start_item(
            0,
            &use_spell,
            None,
            Instant::now(),
        );
        // …and a DIFFERENT cast in flight.
        send(&mut world, HEARTHSTONE, CastCommit::Spell);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { .. })));

        send(&mut world, MOUNT, CastCommit::Spell);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            world.resource::<CastErrors>().0,
            vec![CastFail::local(MOUNT, 0x61)],
            "mid-cast outranks the cooldown rung (the ref's IsCasting precedes the validator)"
        );
    }

    /// The targeting cursor's ONE commit tail (decisions 0923 / 0939), all six cells of its
    /// commit × bind grid. All three seams share it precisely so this table can't grow a seventh,
    /// divergent copy: a thrown grenade, a poison bottle and a key are the same `CMSG_USE_ITEM`
    /// with a different bit set, and a Flamestrike, a Craft-window enchant and a lockpick are the
    /// same pending-cast block under three `CMSG_CAST_SPELL` builders. Every cell also arms the
    /// pending guard and clears the word — asserted once, since the tail is one function.
    #[test]
    fn the_targeting_commit_tail_covers_every_seam() {
        use benilla_protocol::messages::UseItemTarget;
        const DEST: [f32; 3] = [1.0, 2.0, 3.0];
        const ITEM: u64 = 0xF150_0000_0000_ABCD;
        const GO: u64 = 0xF110_000C_1F00_A3B2;
        let (mut world, rx) = world();
        let commit = |world: &mut World, commit: CastCommit, bound: TargetedBind| {
            world.insert_resource(crate::spell::PendingCast::default());
            world
                .resource_mut::<crate::spell::SpellTargeting>()
                // The lock word — the one that answers both the bag and the world seam.
                .enter(HEARTHSTONE, commit, 0x4800);
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    ladder.commit_targeted(HEARTHSTONE, commit, bound);
                })
                .expect("the tail runs as a one-shot system");
        };

        commit(&mut world, CastCommit::Spell, TargetedBind::Dest(DEST));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellAtDest { dest: DEST, .. })
        ));
        assert!(
            !world.resource::<crate::spell::SpellTargeting>().active(),
            "the commit clears the one word"
        );
        assert!(
            world
                .resource::<crate::spell::PendingCast>()
                .in_flight(Instant::now()),
            "and arms the in-flight guard the click owes"
        );

        commit(&mut world, CastCommit::Spell, TargetedBind::Item(ITEM));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellItem {
                item_guid: ITEM,
                ..
            })
        ));

        commit(&mut world, HEARTH_COMMIT, TargetedBind::Dest(DEST));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::UseItem {
                bag_index: 255,
                slot: 24,
                target: UseItemTarget::Dest(DEST),
                ..
            })
        ));

        commit(&mut world, HEARTH_COMMIT, TargetedBind::Item(ITEM));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::UseItem {
                bag_index: 255,
                slot: 24,
                target: UseItemTarget::Item(ITEM),
                ..
            })
        ));

        // The world seam (decision 0939): the same two opcodes, the GameObject block. The spell
        // arm is byte-identical to the right-click OPEN_LOCK send — one builder, deliberately.
        commit(&mut world, CastCommit::Spell, TargetedBind::Object(GO));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpellGameObject { go_guid: GO, .. })
        ));

        commit(&mut world, HEARTH_COMMIT, TargetedBind::Object(GO));
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::UseItem {
                bag_index: 255,
                slot: 24,
                target: UseItemTarget::Object(GO),
                ..
            })
        ));
    }
}
