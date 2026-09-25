//! The one cast-send path: every spell and item use leaves through [`send_spell_cast`], which runs
//! the reference's `TryCast 0x6e4b60` and its commit `SendCast 0x6e54f0` rung for rung.
//!
//! The rungs: the auto-repeat toggle-off (the reference's action-button handler, ahead of TryCast;
//! its order against the profession intercept is unobservable), the profession intercept, the
//! targeting abort, in-flight, reagents and totems, the equipped item, target binding and range,
//! then the validator `0x6094f0` (not-ready and GCD, power, crowd control, mounted, water, moving,
//! form), the deferred cast-arm refusal and the targeting cursor. The commit tail follows: ranged
//! stance, auto-repeat arm, the send, the auto-attack start, the GCD. A refusal is local and
//! pre-commit: no packet, no GCD, no pending arm, only the red line.
//!
//! An item use takes the same ladder: `CGItem::Use 0x5d8d00` calls `0x6e5a90`, whose body is
//! `call 0x6e4b60` with the item as TryCast's third argument (read at `6e4d76` and `6e4f33`). Three
//! rungs fork on it, hence [`CastCommit`]: the not-ready rung (`60952b`: item cooldown `0x6e2ed0`
//! and 0x28, else `0x6e2ea0` and 0x3c), the power gate `0x60962c` (items skip it), and the opcode
//! (`0x6e57d8`: `0xab` vs `0x12e`).

use std::time::Instant;

use benilla_protocol::messages::UseItemTarget;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::items::Items;
use crate::net::{ClientCommand, NetCommands, Objects, SelfPlayer};

use super::{cast_target, validator, AutoRepeatActive};
use crate::ui_action::{reagent_totem_refusal, CastErrors, Spells};

/// What the commit sends: `SendCast 0x6e54f0` branches on whether the pending-cast guid
/// (`0xceac48`, the item's when TryCast got one, else the caster's) is the caster's; the item arm
/// sends `CMSG_USE_ITEM` (`0x6e57d8 push 0xab`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CastCommit {
    /// `CMSG_CAST_SPELL` (0x12e).
    Spell,
    /// `CMSG_USE_ITEM` (0xab): the item's wire position and spell ordinal, plus a caller-bound
    /// GameObject.
    Item {
        bag_index: u8,
        slot: u8,
        /// The item's template entry: the not-ready rung queries the `(use_spell, entry)` record.
        entry: u32,
        spell_index: u8,
        /// `CGItem::Use`'s own target argument, set only for a key in a lock; it bypasses the
        /// binder, as TryCast's target resolve (`6e4ef4`) takes the pair it was passed.
        on_object: Option<u64>,
    },
}

impl CastCommit {
    /// Whether this commit carries an item: what every forked rung reads.
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

/// Everything the cast-send path reads, as one [`SystemParam`]: [`CastLadder::send`] is the only
/// way into [`send_spell_cast`], for every caster surface.
#[derive(SystemParam)]
pub(crate) struct CastLadder<'w, 's> {
    pub(crate) commands: Res<'w, NetCommands>,
    pub(crate) self_player:
        Query<'w, 's, (Entity, Has<crate::creature_anim::Engaged>), With<SelfPlayer>>,
    pub(crate) spells: Option<Res<'w, Spells>>,
    pub(crate) items: Res<'w, Items>,
    /// The reagent check walks the bags through it; the item arms resolve an instance guid here.
    pub(crate) objects: Objects<'w, 's>,
    pub(crate) sheath: MessageWriter<'w, crate::creature_anim::SheathRequest>,
    pub(crate) ecs: Commands<'w, 's>,
    pub(crate) pending: ResMut<'w, crate::spell::PendingCast>,
    pub(crate) queued_melee: ResMut<'w, crate::spell::QueuedMeleeSpell>,
    pub(crate) cooldowns: ResMut<'w, crate::spell::Cooldowns>,
    /// The talent spell modifiers the power gate's cost goes through.
    pub(crate) spell_mods: Res<'w, crate::spell::SpellModifiers>,
    pub(crate) cast_errors: ResMut<'w, CastErrors>,
    pub(crate) auto_repeat: ResMut<'w, AutoRepeatActive>,
    pub(crate) trade_skill_opens: ResMut<'w, crate::ui_tradeskill::TradeSkillOpens>,
    pub(crate) ground: ResMut<'w, super::targeting::SpellTargeting>,
}

/// What a targeting-cursor click bound: what `BindLocation 0x6e60f0` or `BindTarget 0x6e5b40`
/// fills into the standing flag word after the ladder has run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TargetedBind {
    /// The terrain click's point in WoW coords: `BindLocation`'s bit-6 arm.
    Dest([f32; 3]),
    /// The terrain click bound to the source slot: `BindLocation`'s bit-5 arm (`6e6105`), tested
    /// first; writes `SPELLCAST+0x30` and wire bit `0x0020`.
    Source([f32; 3]),
    /// The bag or paper-doll click's item guid.
    Item(u64),
    /// The world click's GameObject guid: a chest, door, vein or herb.
    Object(u64),
}

impl CastLadder<'_, '_> {
    /// The targeting cursor's commit: the ladder ran when the cursor went up, so the click owes
    /// only `SendCast 0x6e54f0`'s tail (the packet, the pending arm, the GCD), then clears the
    /// word.
    pub(crate) fn commit_targeted(
        &mut self,
        spell_id: u32,
        commit: CastCommit,
        bound: TargetedBind,
    ) {
        let cmd = match commit {
            // The same targets block under either opcode; the spell side has one builder per bind.
            CastCommit::Spell => match bound {
                TargetedBind::Dest(dest) => ClientCommand::CastSpellAtDest { spell_id, dest },
                TargetedBind::Source(src) => ClientCommand::CastSpellAtSource { spell_id, src },
                TargetedBind::Item(item_guid) => ClientCommand::CastSpellItem {
                    spell_id,
                    item_guid,
                },
                // The right-click OPEN_LOCK packet: `BindTarget`'s GameObject arm fills both.
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
            // The ladder's `normal_cast` predicate; a ranged shot never raises the cursor.
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

    /// Run the ladder for `spell_id` and commit as `commit` says.
    pub(crate) fn send(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        commit: CastCommit,
    ) {
        self.send_bound(spell_id, ctx, commit, None);
    }

    /// The ladder for a cast the caller already bound to a GameObject (a lock opener): TryCast
    /// takes the guid pair it is passed (`6e4ef4` → `0x612df0`), and the GameObject use-sender
    /// passes one (`0x5f35c0 → 0x6e5a90 → 0x6e4b60`).
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

/// Run the ladder for one cast and commit it. The commit tail (`0x6e54f0`): a ranged spell arms the
/// ranged stance (`0x6e5930`, `SetSheatheState(2,1,1)`; the echoed START re-requests it), an
/// auto-repeat spell sets the sticky armed state (`0x6e593b`, `|= 0x200`, the Load/Hold idle's
/// gate), and the packet goes out.
///
/// The `pending` guard is TryCast's IsCasting refusal (`6e4d97`, [`crate::spell::PendingCast`]):
/// any press while a guarding cast is in flight is refused locally. Ranged and on-next-swing
/// commits are recorded but never guard.
fn send_spell_cast(
    spell_id: u32,
    ctx: &cast_target::CastContext,
    commit: CastCommit,
    // A caller-bound GameObject for a spell commit; an item commit carries its own.
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
    // TryCast's first branch (`6e4bce`, ahead of every gate): an `Effect[0] == TRADE_SKILL` cast
    // never reaches the wire; the crafting window opens instead.
    if def.is_some_and(|d| d.effects[0] == benilla_formats::SPELL_EFFECT_TRADE_SKILL) {
        debug!("ui_action: cast {spell_id} is a profession opener — the crafting book opens, no packet");
        trade_skill_opens.0.push(spell_id);
        return;
    }
    // The re-press toggle (`0x4e60da`): pressing the running auto-repeat spell cancels it. The
    // reference does this in the action-button handler, before TryCast is called.
    if def.is_some_and(|d| d.auto_repeat()) && auto_repeat.0 == Some(spell_id) {
        debug!("ui_action: cast {spell_id} re-pressed — auto-repeat toggles off");
        let self_e = self_player.single().ok().map(|(e, _)| e);
        crate::creature_anim::cancel_auto_repeat_local(self_e, auto_repeat, ecs, commands);
        return;
    }
    // TryCast's IsTargeting leg (`6e4d62`): a new press while the cursor is up clears the word,
    // with no packet, and continues down the ladder.
    if ground.active() {
        debug!("ui_action: cast {spell_id} supersedes the targeting cursor");
        ground.clear();
    }
    // A ranged shot's record does not guard. An on-next-swing spell (`Attributes & 0x404`) queues
    // on the melee slot ([`crate::spell::QueuedMeleeSpell`]) and does not guard either: `6e4d97`
    // exempts an in-flight record with the 0x404 bits.
    let on_next_swing = def.is_some_and(|d| d.on_next_swing());
    let normal_cast = !def.is_some_and(|d| d.ranged_attack()) && !on_next_swing;
    // Re-pressing the queued strike is the silent same-spell bail (`6e4d43`): 1.12 has no
    // re-press-to-unqueue.
    if on_next_swing && queued_melee.current() == Some(spell_id) {
        debug!("ui_action: cast {spell_id} suppressed — already queued on next swing");
        return;
    }
    if pending.in_flight(now) {
        // The already-casting refusal: the same spell bails silently (`6e4d43`), another errors
        // 0x61 "Another action is in progress" (`6e4d97`). The gate reads the in-flight record's
        // `Attributes & 0x404`, never the pressed spell's, and a guarding record is always an
        // ordinary cast or an item use, so every press class is refused here.
        if pending.current(now) != Some(spell_id) {
            cast_errors.push_local(spell_id, 0x61);
        }
        debug!("ui_action: cast {spell_id} suppressed — a cast is already in flight");
        return;
    }
    // `CheckReagentsAndTotems 0x6e4000` (TryCast's `0x6e4ded`, before the validator call at
    // `0x6e4f3b`): a missing tool or reagent refuses with 0x78/0x5c. Local because vmangos answers
    // a pickless cast with `ITEM_GONE`.
    if reagent_totem_refusal(spell_id, def, ctx.rel.self_store, objects, cast_errors) {
        return;
    }
    // The equipped-item requirement (`0x6e4e03`), the same search the button greying and tooltip
    // use. It sits above the crowd-control rung, so a stunned caster without the weapon is told
    // about the weapon.
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
    // ArmCast (`0x6e5250`) resolves the wire target from the spell's targeting constraints, never
    // the raw selection. A caller-bound GameObject skips the walk, as TryCast's resolve (`6e4ef4` →
    // `0x612df0`) takes a passed guid pair.
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
        // The reference resolves the candidate through `0x468460(typemask 1)` (`6e53bc`): a guid
        // naming no live object binds nothing and falls to the cursor.
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
                // The main-hand auto-pick: already bound, it commits in the item shape. Not a
                // unit, so the range gate skips it (`0x6e47b0` tests `SPELLCAST+0x14 & 0x8202`).
                cast_target::CastWireTarget::Item(guid) => {
                    item_target = Some(guid);
                    None
                }
                cast_target::CastWireTarget::Targeting(word) => {
                    // The cursor comes up at `6e50c8`, after the validator `0x6094f0`, so the
                    // word parks until the rungs pass. The pending-cast block (item guid at
                    // `0xceac48` included) survives the cursor, so the commit rides the word.
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
                // TryCast's tail after ArmCast returns false (`6e5045`–`6e507b`) runs below the
                // validator, so this parks and fires where the cursor would come up: a mounted
                // press with an empty weapon hand reads "You are mounted" first.
                cast_target::CastWireTarget::RefusedAtArm(reason) => {
                    deferred_refusal = Some(reason);
                    None
                }
            }
        }
    };
    // The range gate (`CanTargetUnit 0x6e4440` → `IsTargetInRange 0x6e47b0`, before the commit):
    // a too-close shot never reaches the sheath snap `0x6e5930`. Only the selection is tested; a
    // self-bind is distance 0.
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
    // ── The validator `0x6094f0` ──
    //
    // Not-ready (`0x60952b`): an item press queries (use-spell, entry) and refuses 0x28, a spell
    // press queries (spell, 0) and refuses 0x3c. The GCD rides the same query, matched on
    // `StartRecoveryCategory`. Local, because the server's NOT_READY fail clears the running GCD.
    let queried_item = match commit {
        CastCommit::Item { entry, .. } => entry,
        CastCommit::Spell => 0,
    };
    if cooldowns.not_ready(spell_id, queried_item, def, now) {
        debug!("ui_action: cast {spell_id} refused locally — not ready (the validator's rung 1)");
        // The one packet a local refusal sends (`0x609576–0x60960f`, spell leg only): a running
        // repeat with AttributesEx3 0x400000 (wand Shoot) gets `CMSG_CANCEL_CAST`, then the local
        // cancel.
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
    // The power gate (`0x60962c`, spells only: the item fork jumps past it to `0x6096b3`): raw
    // `UNIT_FIELD_POWER[type]` (a negative PowerType reads health) signed-compared against the
    // cost, refusing 0x4d. Local, because vmangos accepts the cast and its NO_POWER fail clears
    // the running GCD.
    if !commit.is_item() {
        if let (Some(d), Some(store)) = (def, ctx.rel.self_store) {
            if !super::usable::can_afford(d, store, spell_mods) {
                debug!("ui_action: cast {spell_id} refused locally — not enough power (0x4d)");
                cast_errors.push_local(spell_id, 0x4d);
                return;
            }
        }
    }
    // The validator's six crowd-control arms sit above its mounted block: a stunned mounted
    // caster reads the stun.
    let self_fields = ctx.rel.self_store.map(|s| &s.0);
    if let Some(reason) = validator::cast_cc_refusal(
        self_fields.map_or(0, |f| f.unit_flags()),
        self_fields.and_then(|f| f.unit_health()),
        // The charm arm refuses only when someone else holds the charm (`60994d`).
        self_fields
            .and_then(|f| f.unit_charmed_by())
            .is_some_and(|charmer| Some(charmer) != ctx.self_guid),
        def,
        // The exemption scan reads the caster's raw aura slot ids, unfiltered, as the reference.
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
                // The reference takes the first rejection and stops; exempt only on an accepted
                // match.
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
    // The mounted block (`0x609c6c`): a live `UNIT_FIELD_MOUNTDISPLAYID` refuses 0x39 "You are
    // mounted" unless `Attributes & 0x01000000`. Local, because vmangos silently dismounts
    // instead. The range gate runs first, so mounted and out of range reads "Out of range.".
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
    // The water leg (`0x609d39–0x609d7b`): 0x50 "Cannot use while swimming" for mounts, Travel
    // Form and food, 0x58 "Can only use while swimming" for Aquatic Form. Local, because vmangos's
    // CheckCast has no such arm and would grant Aquatic Form on land.
    if let Some(reason) = validator::cast_water_refusal(ctx.self_move_flags, def) {
        debug!("ui_action: cast {spell_id} refused locally — water side ({reason:#x})");
        cast_errors.push_local(spell_id, reason);
        return;
    }
    // The moving leg (`0x609de3`): a cast-time press while moving refuses 0x2e. Local, because
    // vmangos accepts the cast and then interrupts it mid-bar.
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
    // The form leg (`0x609e49` → `0x612480`; `0x609ca2` is the posture gate): 0x3d "Can't do that
    // while shapeshifted" or 0x56 needs-a-form (vmangos `GetErrorAtShapeshiftedCast`).
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
    // The deferred ArmCast-false refusal (`6e5050`): every rung passed and nothing bound.
    if let Some(reason) = deferred_refusal {
        debug!("ui_action: cast {spell_id} refused locally — the cast-arm tail ({reason:#x})");
        cast_errors.push_local(spell_id, reason);
        return;
    }
    // The deferred cursor entry (`6e50c8`): every rung passed, the cursor waits for its click.
    if let Some(word) = pending_word {
        debug!("ui_action: cast {spell_id} awaits its click — targeting cursor up ({word:#06x})");
        ground.enter(spell_id, commit, word);
        return;
    }
    // The wand handoff (`0x60959e`): a new cast cancels the running repeat only when it carries
    // `AttributesEx3 & 0x400000` (wand Shoot 5019), so Auto Shot survives. `CMSG_CANCEL_CAST`
    // naming it goes first (`0x6095b8`), then the local cancel and its ack.
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
                // The autorepeat key (`0xceac30`, written at `0x6e5947`) the button's checked
                // state reads.
                auto_repeat.0 = Some(spell_id);
                // The commit's StopAttack (`0x6e5976` → `0x5ecac0`), gated only on the caster
                // being the active player (`0x6e5957`–`0x6e5972`): starting a repeat ends the
                // melee swing and its queued strike. This is where melee and auto-repeat are kept
                // exclusive. It runs after the arm above, so it never cancels this repeat.
                crate::creature_anim::stop_attack_local(engaged, queued_melee, commands);
            }
        }
    }
    // `SendCast 0x6e54f0`: one targets block, two opcodes.
    let _ = commands.0.send(match commit {
        CastCommit::Spell => match (explicit_object, item_target) {
            // A lock opener bound by its click; `BindTarget`'s GameObject arm fills the block.
            (Some(go_guid), _) => ClientCommand::CastSpellGameObject { spell_id, go_guid },
            // The item leg, bound by the main-hand auto-pick.
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
    // One in-flight id for every commit (`0xceca88`, written at `0x6e5026`): every class is
    // recorded, so the `modalNextSpell` test (`0x6e7408`) can match a ranged shot; only
    // `normal_cast` and the item arm guard. The item arm's deadline is shorter because vmangos
    // answers some `CMSG_USE_ITEM` legs with `SMSG_INVENTORY_CHANGE_FAILURE` and no cast result.
    if commit.is_item() {
        pending.arm_item(spell_id, now);
    } else {
        pending.arm(spell_id, now, normal_cast);
    }
    if on_next_swing {
        queued_melee.arm(spell_id);
    }
    // TryCast's post-send tail (`6e51b5`): a spell that initiates auto-attack (on-next-swing
    // `0x404` or `AttributesEx & 0x200`, not the GO-deferred Ex2 bit 20) starts the swing at its
    // unit target (`0x6131a0` → `0x5ecb70`: sheath snap and `CMSG_ATTACKSWING`).
    if let (Some(d), Some(guid)) = (def, target) {
        if d.initiates_auto_attack() {
            if let Ok((e, engaged)) = self_player.single() {
                // The tail's own gate (`6e51cb call 0x60ecb0; 6e51d2 jne`, the attack lock
                // `[player+0xc48]`, our `Engaged`) skips `0x6131a0` entirely, so a strike pressed
                // mid-swing leaves a running repeat alone. The attack path's own test at
                // `0x5eccda` would still reach the repeat cancel `0x5ecd8c`; this route never
                // gets there.
                if !engaged {
                    debug!("ui_action: cast {spell_id} initiates auto-attack at {guid:#x}");
                    // No stop is in flight: the gate above guarantees the lock is clear.
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
    // The GCD arms at send (`0x6e2de0` from `0x6e58fb`); a failed cast result clears it
    // (`0x6e1630`).
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
    /// The hearthstone's wire position on a fresh character: player array, backpack slot 1.
    const HEARTH_COMMIT: CastCommit = CastCommit::Item {
        bag_index: 255,
        slot: 24,
        entry: 6948,
        spell_index: 0,
        on_object: None,
    };

    static EMPTY_REPUTATIONS: Reputations = Reputations(Vec::new());

    /// A context with nothing selected: every rung that needs world state is inert.
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

    /// A World with exactly the resources [`CastLadder`] gathers and no `Spells`, so every rung
    /// that needs the spell's row is inert.
    fn world() -> (World, Receiver<ClientCommand>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<Items>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::spell::PendingCast>();
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::Cooldowns>();
        world.init_resource::<crate::spell::SpellModifiers>();
        world.init_resource::<CastErrors>();
        world.init_resource::<AutoRepeatActive>();
        world.init_resource::<crate::ui_tradeskill::TradeSkillOpens>();
        world.init_resource::<crate::spell::SpellTargeting>();
        // A bare World has no message storage until it is asked for.
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

    /// A mashed chest opener goes out once as the GameObject block; the re-click is `6e4d43`'s
    /// silent same-spell bail, with no packet and no red line.
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

        // A different cast mid-opener is the loud refusal (6e4d97).
        send(&mut world, MOUNT, CastCommit::Spell);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            world.resource::<CastErrors>().0,
            vec![CastFail::local(MOUNT, 0x61)],
            "a different spell mid-cast still errors 0x61"
        );
    }

    // ── Melee and auto-repeat exclusion ──
    //
    // The auto-repeat commit calls StopAttack (`0x6e5976` → `0x5ecac0` → `CancelQueuedCast
    // 0x6e6f30`), and the attack-start's tail (`0x5ecd78`–`0x5ecd95`) cancels the running repeat.

    const AUTO_SHOT: u32 = 75;
    const RAPTOR_STRIKE: u32 = 2973;
    const SERPENT_STING: u32 = 1978;
    const MOB: u64 = 0x0000_0000_0000_00f1;

    /// Auto Shot (auto-repeat `AttributesEx2 & 0x20`, ranged `Attributes & 0x2`), Raptor Strike
    /// (on-next-swing `0x404`) and Serpent Sting, as the 1.12 rows classify them. `Targets & 0x2`
    /// binds the selection without faction data.
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
        // An instant hunter shot: ranged, not auto-repeat, and it names Auto Shot in
        // `modalNextSpell`.
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

    /// Every commit is recorded (`0xceca88`, written at `0x6e5026`), but a ranged shot does not
    /// guard: it must not refuse the next press with `0x61`.
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

        // An on-next-swing strike is recorded too.
        let (mut world, _rx) = combat_world(false);
        send_at(&mut world, RAPTOR_STRIKE, MOB);
        let pending = world.resource::<crate::spell::PendingCast>();
        assert_eq!(
            pending.committed(Instant::now()),
            Some(RAPTOR_STRIKE),
            "an on-next-swing strike is committed too"
        );
    }

    /// The chained Auto Shot passes every rung after the sting: the sting's GCD is running, but
    /// Auto Shot's `StartRecoveryCategory` is 0 against the GCD node's 133.
    #[test]
    fn the_chained_auto_shot_is_not_refused_by_the_sting_it_followed() {
        let (mut world, rx) = combat_world(false);
        send_at(&mut world, SERPENT_STING, MOB);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == SERPENT_STING),
            "the sting goes out first"
        );
        // What `cast_result` returns and `on_cast_result` hands to the ladder.
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

    /// A shot pressed mid-cast meets `6e4d97`'s `0x61` like any press (the gate reads the in-flight
    /// record's attributes), and the cast keeps its guard.
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

    /// [`world`] plus the catalog and the self player; `engaged` mirrors the attack lock
    /// `[+0xc48]`.
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

    /// The commit's StopAttack (`0x6e5976`): `CMSG_ATTACKSTOP` (`0x624370`), then `CancelQueuedCast
    /// 0x6e6f30` hands the queued strike to `CancelCast 0x6e4940(dl=1)` and its `CMSG_CANCEL_CAST`.
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

    /// `0x5ecac0`'s early-out (`!IsAttacking && [+0xc50] == 0`): with no swing running nothing is
    /// sent and the queued strike survives.
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

    /// A strike pressed while engaged never reaches `0x6131a0` (`6e51cb call 0x60ecb0; 6e51d2
    /// jne`), so a running auto-repeat survives. Unreachable in play, since starting Auto Shot
    /// stops the swing.
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

    /// Not yet swinging: the tail fires, `0x6131a0` → `0x5ecb70` runs, and its `0x5ecd8c` cancels
    /// the repeat.
    #[test]
    fn a_strike_pressed_before_the_swing_starts_kills_the_auto_repeat() {
        let (mut world, rx) = combat_world(false);
        world.resource_mut::<AutoRepeatActive>().0 = Some(AUTO_SHOT);

        send_at(&mut world, RAPTOR_STRIKE, MOB);

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == RAPTOR_STRIKE)
        );
        // The swing send is `0x5eccfd`, the repeat cancel `0x5ecd8c` after it.
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

    /// An item use runs the same in-flight rung: a double-click bails silently (`6e4d43`), a
    /// different spell errors 0x61 (`6e4d97`), and nothing reaches the wire.
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

    /// The not-ready rung forks at `60952b`: an item refuses 0x28 "Item is not ready yet."
    /// (`609549`), a spell 0x3c (`609616`).
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

        // The record is keyed (use-spell, entry); a spell press queries (spell, 0) and passes.
        world.resource_mut::<CastErrors>().0.clear();
        world.insert_resource(crate::spell::PendingCast::default());
        send(&mut world, HEARTHSTONE, CastCommit::Spell);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { .. })),
            "the item-keyed record never not-readies a bare spell press"
        );
        assert!(world.resource::<CastErrors>().0.is_empty());
    }

    /// `SendCast 0x6e54f0`: two opcodes, one targets block; a key's lock rides it as
    /// `TARGET_FLAG_GAMEOBJECT|LOCKED`.
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

        // Reset the in-flight guard the previous send armed.
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

    /// The power gate (`0x60962c`): an unaffordable spell refuses 0x4d locally; an item press
    /// skips it.
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
        // 100 mana (field 23 = UNIT_FIELD_POWER1). Leaked: the one-shot closure must be 'static.
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

    /// TryCast's IsCasting refusal (`0x61`) precedes the not-ready rung.
    #[test]
    fn in_flight_outranks_not_ready() {
        let (mut world, rx) = world();
        // A long cooldown on MOUNT...
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
        // ...and a different cast in flight.
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

    /// The targeting commit over its commit × bind grid; every cell arms the guard and clears the
    /// word.
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
                // The lock word, which answers both the bag and the world click.
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

        // The world click: the GameObject block, the same builder as the right-click OPEN_LOCK.
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
