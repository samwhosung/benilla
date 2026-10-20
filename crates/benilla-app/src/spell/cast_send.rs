//! The one cast-send path: every spell and item use leaves through [`send_spell_cast`], which runs
//! the reference's `TryCast 0x6e4b60` and its commit `SendCast 0x6e54f0` rung for rung.
//!
//! The rungs: the auto-repeat toggle-off (the reference's action-button handler, ahead of TryCast;
//! its order against the profession intercept is unobservable), the profession intercept, dead or a
//! ghost, the targeting abort, in-flight, reagents and totems, the equipped item, combo points, the
//! attack pick, target binding and range, then the validator `0x6094f0` (not-ready and GCD, power,
//! crowd control, mounted, water, moving, form), the deferred cast-arm refusal and the targeting
//! cursor. The commit tail follows: ranged stance, auto-repeat arm, the send, the auto-attack start,
//! the GCD. A refusal is local and pre-commit: no packet, no GCD, no pending arm, only the red line.
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
use crate::query_cache::Ask;

use super::inflight::Inflight;
use super::{cast_target, validator, AutoRepeatActive, CastCore, Held};
use crate::ui_action::{reagent_totem_refusal, CastFail, Spells};
use crate::ui_cast::CastReports;

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

/// The class byte TryCast's finisher rung tests (`6e4e98`).
const CLASS_ROGUE: u8 = 4;

/// A player's spell press held at TryCast's attack pick (`6e4edf` → `0x612df0`), which can move
/// the selection the ladder only reads. [`crate::script_calls`] runs the pick and resumes it.
#[derive(Resource, Default)]
pub(crate) struct HeldForPick(Option<HeldCast>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeldCast {
    pub(crate) spell_id: u32,
    /// Pressed with the self-cast modifier: TryCast's guid argument is then the caster
    /// (`0x4e610e`), which the pick tries before the selection.
    pub(crate) on_self: bool,
}

/// Whether the script call being applied passed the protected-action gate's tag 2, which opens
/// `TryCastSpell` (`0x6e4bc1`, `0x494a50`): the answer the Lua verb got, where the stack that decides
/// it was still there. A cast the engine starts (a click, a chain, a held press resumed) asks no
/// gate, so it reads the default, passed; [`crate::script_calls`] sets it per call and resets it.
#[derive(Resource)]
pub(crate) struct ScriptCastPermit(bool);

impl Default for ScriptCastPermit {
    fn default() -> Self {
        Self(true)
    }
}

impl ScriptCastPermit {
    /// Whether tag 2 passed for the call being applied.
    pub(crate) fn allows(&self) -> bool {
        self.0
    }
}

/// Everything the cast-send path reads, as one [`SystemParam`]: [`CastLadder::send`] is the only
/// way into [`send_spell_cast`], for every caster surface.
#[derive(SystemParam)]
pub(crate) struct CastLadder<'w, 's> {
    /// Absent in a world that never applies script calls.
    permit: Option<ResMut<'w, ScriptCastPermit>>,
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
    /// The cast's reports, in the order they are made ([`crate::ui_cast`]).
    pub(crate) reports: ResMut<'w, CastReports>,
    pub(crate) auto_repeat: ResMut<'w, AutoRepeatActive>,
    pub(crate) trade_skill_opens: ResMut<'w, crate::ui_tradeskill::TradeSkillOpens>,
    pub(crate) ground: ResMut<'w, super::targeting::SpellTargeting>,
    pub(crate) held_for_pick: ResMut<'w, HeldForPick>,
}

/// Whether `ReportCastFailure 0x6e1a00` reaches its tail for `fail`
/// ([`crate::ui_action::report_reaches_pop`]), asking the item cache as its lookup does.
pub(crate) fn reaches_tail(
    fail: &CastFail,
    spells: Option<&Spells>,
    items: &Items,
    net: &NetCommands,
) -> bool {
    crate::ui_action::report_reaches_pop(
        fail.reason,
        fail.arg,
        spells.and_then(|s| s.catalog.get(fail.spell_id)),
        spells.map(|s| &s.forms),
        // The lookup with the callback `0x6e29b0` (`0x6e1ea4`).
        |item| items.template(item, 0, Ask::Event(net)).is_some(),
    )
}

/// The client's own refusal through the reporter: its line, then its tail.
fn refuse(
    core: &mut CastCore,
    spells: Option<&Spells>,
    items: &Items,
    net: &NetCommands,
    fail: CastFail,
) {
    let reaches = reaches_tail(&fail, spells, items, net);
    core.report(fail, false, reaches);
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
    /// The world click's unit guid.
    Unit(u64),
    /// The world click's corpse guid and the corpse bit `BindTarget`'s corpse arm bound.
    Corpse(u64, benilla_protocol::messages::CorpseTarget),
}

impl CastLadder<'_, '_> {
    /// The reporters' state ([`CastCore`]).
    pub(crate) fn core(&mut self) -> CastCore<'_> {
        CastCore {
            pending: &mut self.pending,
            queued_melee: &mut self.queued_melee,
            ground: &mut self.ground,
            reports: &mut self.reports,
            net: &self.commands,
        }
    }

    /// A refusal of the client's own through `ReportCastFailure 0x6e1a00`: its line, then its
    /// tail.
    pub(crate) fn report_local(&mut self, fail: CastFail) {
        let reaches = reaches_tail(&fail, self.spells.as_deref(), &self.items, &self.commands);
        self.core().report(fail, false, reaches);
    }

    /// `StopSpellTargeting 0x6e4900`; whether the cursor was up.
    pub(crate) fn stop_targeting(&mut self) -> bool {
        self.core().stop_targeting()
    }

    /// Set what the gate's tag 2 allowed the script call about to be applied.
    pub(crate) fn permit(&mut self, may_cast: bool) {
        if let Some(permit) = self.permit.as_mut() {
            permit.0 = may_cast;
        }
    }

    /// Whether the ladder may start: `TryCastSpell`'s gate, before its first rung.
    pub(crate) fn permitted(&self) -> bool {
        self.permit.as_ref().is_none_or(|p| p.0)
    }

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
                TargetedBind::Unit(unit_guid) => ClientCommand::CastSpell {
                    spell_id,
                    target: Some(unit_guid),
                },
                TargetedBind::Corpse(corpse_guid, target) => ClientCommand::CastSpellCorpse {
                    spell_id,
                    target,
                    corpse_guid,
                },
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
                    TargetedBind::Unit(guid) => UseItemTarget::Unit(guid),
                    TargetedBind::Corpse(guid, target) => UseItemTarget::Corpse(target, guid),
                },
            },
        };
        let now = Instant::now();
        // The spell its attempt pushed stays in the slot through the send; the guard holds it from
        // here, as for a cast sent at its attempt.
        if self.ground.in_slot() == Some(spell_id) {
            self.pending
                .push(spell_id, item_flags(&self.items, commit), now);
        }
        // The bind emptied the word, and signals before it sends (`0x6e60cb`, `0x6e615d`).
        self.ground.clear();
        self.reports.changed();
        let _ = self.commands.0.send(cmd);
        // The send's own signal (`0x6e589d`), ungated as the attempt's send is (#1583).
        self.reports.changed();
        let auto_repeat = self
            .spells
            .as_ref()
            .and_then(|s| s.catalog.get(spell_id))
            .is_some_and(|d| d.auto_repeat());
        if auto_repeat {
            self.core().pop_if(spell_id);
        }
        if let Some(d) = self.spells.as_ref().and_then(|s| s.catalog.get(spell_id)) {
            self.cooldowns.start_gcd(spell_id, d, now, &self.spell_mods);
        }
    }

    /// TryCast's dead rung alone, for the Attack button, whose short-circuit (`0x6e4c7a`) leaves
    /// the ladder right after it.
    pub(crate) fn dead_refusal(
        &mut self,
        spell_id: u32,
        self_store: Option<&crate::net::ObjectStore>,
    ) -> bool {
        let def = self.spells.as_ref().and_then(|s| s.catalog.get(spell_id));
        let dead = caster_dead(spell_id, def, self_store);
        if dead {
            self.report_local(CastFail::local(spell_id, REASON_DEAD));
        }
        dead
    }

    /// Run the ladder for `spell_id` and commit as `commit` says, with no attack pick: no 1.12
    /// item use, form, craft or `modalNextSpell` chain casts a spell of predicate `0x6e5200`.
    pub(crate) fn send(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        commit: CastCommit,
    ) {
        self.send_bound(spell_id, ctx, commit, None, None);
    }

    /// A player's spell press (`UseAction`, `CastSpell`, `CastSpellByName`): the ladder with
    /// TryCast's attack pick, where the cast may stop in [`HeldForPick`].
    pub(crate) fn send_spell(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        on_self: bool,
    ) {
        let held = HeldCast { spell_id, on_self };
        self.send_bound(spell_id, ctx, CastCommit::Spell, None, Some(held));
    }

    /// The press held at the attack pick, if one is.
    pub(crate) fn take_held(&mut self) -> Option<HeldCast> {
        self.held_for_pick.0.take()
    }

    /// A held press after its pick, at the unit the pick left selected. The rungs above the pick
    /// run again; none reads the selection, so each passes as it did.
    pub(crate) fn resume_after_pick(&mut self, held: HeldCast, ctx: &cast_target::CastContext) {
        self.send_bound(held.spell_id, ctx, CastCommit::Spell, None, None);
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
        self.send_bound(spell_id, ctx, CastCommit::Spell, Some(go_guid), None);
    }

    fn send_bound(
        &mut self,
        spell_id: u32,
        ctx: &cast_target::CastContext,
        commit: CastCommit,
        on_object: Option<u64>,
        hold: Option<HeldCast>,
    ) {
        if !self.permitted() {
            debug!("cast {spell_id} refused by the protected-action gate, silently");
            return;
        }
        let mut core = CastCore {
            pending: &mut self.pending,
            queued_melee: &mut self.queued_melee,
            ground: &mut self.ground,
            reports: &mut self.reports,
            net: &self.commands,
        };
        send_spell_cast(
            spell_id,
            ctx,
            commit,
            on_object,
            hold,
            &self.commands,
            &self.self_player,
            self.spells.as_deref(),
            &self.objects,
            &self.items,
            &mut self.sheath,
            &mut self.ecs,
            &mut core,
            &mut self.cooldowns,
            &self.spell_mods,
            &mut self.auto_repeat,
            &mut self.trade_skill_opens,
            &mut self.held_for_pick,
        );
    }
}

/// "You are dead", TryCast's dead rung's refusal (`0x6e4c66`).
const REASON_DEAD: u8 = 0x13;

/// TryCast's dead rung (`0x6e4c49`), after the profession intercept and ahead of the Attack
/// short-circuit (`0x6e4c7a`): a dead or ghost caster (`0x605f30`) is refused with 0x13, "You are
/// dead", unless the spell is castable while dead (`Attributes & 0x800000`, `0x6e4c55`).
fn caster_dead(
    spell_id: u32,
    def: Option<&benilla_formats::SpellDisplay>,
    self_store: Option<&crate::net::ObjectStore>,
) -> bool {
    if !self_store.is_some_and(|s| s.0.is_dead_or_ghost())
        || def.is_some_and(|d| d.attributes & benilla_formats::ATTR_CASTABLE_WHILE_DEAD != 0)
    {
        return false;
    }
    debug!("ui_action: cast {spell_id} refused locally — dead or a ghost (0x13)");
    true
}

/// The item's template flags for `commit`, `None` for no item; an item not cached yet reads 0,
/// where the reference's item always has its record.
fn item_flags(items: &Items, commit: CastCommit) -> Option<u32> {
    match commit {
        CastCommit::Spell => None,
        CastCommit::Item { entry, .. } => {
            Some(items.template(entry, 0, Ask::Read).map_or(0, |t| t.flags))
        }
    }
}

/// What the attempt pushes into the in-flight slot (`0x6e4ff5`-`0x6e5026`), if anything: every
/// spell, as the active player's cast time is never negative, but an item's only when its flags
/// carry [`benilla_protocol::messages::ITEM_FLAG_PLAYERCAST`] (`0x6e500b`-`0x6e501f`). An
/// on-next-swing spell (`Attributes & 0x404`) is the queued strike's, a spell that raises the
/// cursor the cursor's.
fn held(
    items: &Items,
    commit: CastCommit,
    def: Option<&benilla_formats::SpellDisplay>,
    cursor: bool,
) -> Option<Held> {
    let item_flags = item_flags(items, commit);
    if item_flags.is_some_and(|f| f & benilla_protocol::messages::ITEM_FLAG_PLAYERCAST == 0) {
        return None;
    }
    Some(if def.is_some_and(|d| d.on_next_swing()) {
        Held::Strike
    } else if cursor {
        Held::Cursor
    } else {
        Held::Cast { item_flags }
    })
}

/// Run the ladder for one cast and commit it. The commit tail (`0x6e54f0`): the packet goes out,
/// then the GCD, then a ranged spell arms the ranged stance (`0x6e5930`,
/// `SetSheatheState(2,1,1)`; the echoed START re-requests it), and an auto-repeat spell sets the
/// sticky armed state (`0x6e593b`, `|= 0x200`, the Load/Hold idle's gate), stops the attack and
/// leaves the slot.
///
/// The `pending` guard is TryCast's IsCasting refusal (`6e4d97`, [`crate::spell::PendingCast`]):
/// any press while a cast is in flight is refused locally. The spell awaiting its click is in
/// flight too, and a queued on-next-swing strike is exempt.
fn send_spell_cast(
    spell_id: u32,
    ctx: &cast_target::CastContext,
    commit: CastCommit,
    // A caller-bound GameObject for a spell commit; an item commit carries its own.
    bound_object: Option<u64>,
    // Set for a player's spell press, which parks in `held_for_pick` at the attack pick.
    hold: Option<HeldCast>,
    commands: &NetCommands,
    self_player: &Query<(Entity, Has<crate::creature_anim::Engaged>), With<SelfPlayer>>,
    spells: Option<&Spells>,
    objects: &Objects,
    items: &Items,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    ecs: &mut Commands,
    core: &mut CastCore,
    cooldowns: &mut crate::spell::Cooldowns,
    spell_mods: &crate::spell::SpellModifiers,
    auto_repeat: &mut AutoRepeatActive,
    trade_skill_opens: &mut crate::ui_tradeskill::TradeSkillOpens,
    held_for_pick: &mut HeldForPick,
) {
    // A refusal through the reporter, then the attempt's `xor al,al` exit.
    macro_rules! refuse {
        ($fail:expr) => {{
            refuse(core, spells, items, commands, $fail);
            return;
        }};
    }
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
    if caster_dead(spell_id, def, ctx.rel.self_store) {
        refuse!(CastFail::local(spell_id, REASON_DEAD));
    }
    // TryCast's same-spell test (`6e4d43`): the spell in the in-flight slot bails with no red
    // line, whether it is cast, queued on the next swing (1.12 has no re-press-to-unqueue) or
    // awaiting its click, ahead of the IsTargeting leg.
    if core.slot().map(Inflight::spell_id) == Some(spell_id) {
        debug!("ui_action: cast {spell_id} suppressed — already in flight");
        return;
    }
    // TryCast's IsTargeting leg (`6e4d62`): a new press while the cursor is up cancels it with its
    // failed flag (`0x6e4d71`, `0x6e4940(1, 1, 0x1c)`), with no packet, and continues down the
    // ladder.
    if core.stop_targeting() {
        debug!("ui_action: cast {spell_id} supersedes the targeting cursor");
    }
    // The already-casting refusal (`6e4d97`-`6e4dd4`): a spell in flight refuses every press with
    // 0x61 "Another action is in progress", unless it is on-next-swing (`Attributes & 0x404`,
    // `6e4dbe`), the gate reading the slot's spell, never the pressed one's.
    if core
        .slot()
        .is_some_and(|top| !matches!(top, Inflight::Strike(_)))
    {
        debug!("ui_action: cast {spell_id} refused — another spell is in flight (0x61)");
        refuse!(CastFail::local(spell_id, 0x61));
    }
    // `CheckReagentsAndTotems 0x6e4000` (TryCast's `0x6e4ded`, before the validator call at
    // `0x6e4f3b`): a missing tool or reagent refuses with 0x78/0x5c. Local because vmangos answers
    // a pickless cast with `ITEM_GONE`.
    if let Some(fail) = reagent_totem_refusal(spell_id, def, ctx.rel.self_store, objects) {
        refuse!(fail);
    }
    // The equipped-item requirement (`0x6e4e03`), the same search the button greying and tooltip
    // use. It sits above the crowd-control rung, so a stunned caster without the weapon is told
    // about the weapon.
    if let Some(d) = def {
        if let Some(store) = ctx.rel.self_store {
            if !super::usable::equipped_item_fits(d, store, objects, items) {
                let reason = super::usable::equipped_item_reason(d);
                debug!("ui_action: cast {spell_id} refused locally — equipped item ({reason:#x})");
                refuse!(CastFail::local(spell_id, reason));
            }
        }
    }
    // The finisher rung (`6e4e50`–`6e4ecd`), above the attack pick: a finishing move
    // (`AttributesEx & 0x500000`) with the combo-point byte at 0 refuses 0x46 "That ability
    // requires combo points" for a rogue (class byte 4, `6e4e98`), else 0x12. Like the button
    // greying, it reads no target.
    if let (Some(d), Some(store)) = (def, ctx.rel.self_store) {
        if d.needs_combo_points() && store.0.player_combo_points().unwrap_or(0) == 0 {
            let reason = if store.0.unit_class() == Some(CLASS_ROGUE) {
                0x46
            } else {
                0x12
            };
            debug!("ui_action: cast {spell_id} refused locally — no combo points ({reason:#x})");
            refuse!(CastFail::local(spell_id, reason));
        }
    }
    // TryCast's attack pick (`6e4edf`–`6e4f02`): a spell of predicate `0x6e5200`, pressed while
    // not attacking (`0x60ecb0`), goes through the attack validator `0x612df0`, which keeps a
    // hostile selection or selects the nearest enemy. It sits above the bind and the range gate,
    // so an acquired target out of reach reads "Out of range.".
    if let Some(held) = hold {
        let engaged = self_player.single().is_ok_and(|(_, engaged)| engaged);
        if def.is_some_and(|d| d.initiates_combat()) && !engaged {
            debug!("ui_action: cast {spell_id} holds for the attack pick");
            held_for_pick.0 = Some(held);
            return;
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
                    refuse!(CastFail::local(spell_id, reason));
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
            if let Some(reason) = ctx.range.refusal(d, row, spell_mods) {
                debug!("ui_action: cast {spell_id} refused locally — range ({reason:#x})");
                refuse!(CastFail::local(spell_id, reason));
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
    // The item's own category for this spell replaces the spell's in the category leg (`0x6e1492`).
    let item_category = items
        .template(queried_item, 0, Ask::Read)
        .and_then(|t| t.use_spell)
        .filter(|u| u.spell_id == spell_id)
        .map_or(0, |u| u.category);
    if cooldowns.not_ready(spell_id, queried_item, item_category, def, now) {
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
        refuse!(CastFail::local(spell_id, commit.not_ready_reason()));
    }
    // The power gate (`0x60962c`, spells only: the item fork jumps past it to `0x6096b3`): raw
    // `UNIT_FIELD_POWER[type]` (a negative PowerType reads health) signed-compared against the
    // cost, refusing 0x4d. Local, because vmangos accepts the cast and its NO_POWER fail clears
    // the running GCD.
    if !commit.is_item() {
        if let (Some(d), Some(store)) = (def, ctx.rel.self_store) {
            if !super::usable::can_afford(d, store, spell_mods) {
                debug!("ui_action: cast {spell_id} refused locally — not enough power (0x4d)");
                refuse!(CastFail::local(spell_id, 0x4d));
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
            Some(m) => refuse!(CastFail::local_arg(spell_id, reason, m)),
            None => refuse!(CastFail::local(spell_id, reason)),
        }
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
        refuse!(CastFail::local(spell_id, 0x39));
    }
    // The water leg (`0x609d39–0x609d7b`): 0x50 "Cannot use while swimming" for mounts, Travel
    // Form and food, 0x58 "Can only use while swimming" for Aquatic Form. Local, because vmangos's
    // CheckCast has no such arm and would grant Aquatic Form on land.
    if let Some(reason) = validator::cast_water_refusal(ctx.self_move_flags, def) {
        debug!("ui_action: cast {spell_id} refused locally — water side ({reason:#x})");
        refuse!(CastFail::local(spell_id, reason));
    }
    // The moving leg (`0x609de3`): a cast-time press while moving refuses 0x2e. Local, because
    // vmangos accepts the cast and then interrupts it mid-bar.
    if let Some(d) = def {
        let caster_level = ctx
            .rel
            .self_store
            .and_then(|s| s.0.unit_level())
            .unwrap_or(0);
        let cast_time_ms = spells.map_or(0, |s| s.cast_time_ms(d, caster_level, spell_mods));
        if validator::cast_moving_refusal(ctx.self_move_flags, cast_time_ms, def) {
            debug!("ui_action: cast {spell_id} refused locally — moving (0x2e)");
            refuse!(CastFail::local(spell_id, 0x2e));
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
            refuse!(CastFail::local(spell_id, reason));
        }
    }
    // Every rung passed: the attempt fills the pending record with the player as its caster
    // (`0x6e4fb1`), ahead of the arm and whatever it decides.
    core.pending.fill_record();
    // The push (`0x6e5026` → `0x6e4ad0`), which signals: the same spell bailed above, so the slot
    // never holds it here. The arm signals whatever it then decides (`0x6e5333`).
    let held = held(items, commit, def, pending_word.is_some());
    if let Some(held) = held {
        core.push(spell_id, held, now);
    }
    core.reports.changed();
    // The deferred ArmCast-false refusal (`6e5045`-`6e50a9`): every rung passed and nothing
    // bound. The attempt pushed the spell before the arm (`0x6e5026`), so its report finds it in
    // flight and cancels it (`0x6e2201` → `0x6e2218` → `0x6e4940(1, 1, reason)`): the word is
    // already zero (`0x6e5057`, `0x6e5098`), so the cancel goes on the wire (`0x6e4970`), the pop
    // restores the save, and the record's caster fires `SPELLCAST_FAILED`. An item without
    // `ITEM_FLAG_PLAYERCAST` was never pushed, and its report fires the event itself. The tail
    // signals once more as it leaves (`0x6e5069`).
    if let Some(reason) = deferred_refusal {
        debug!("ui_action: cast {spell_id} refused locally — the cast-arm tail ({reason:#x})");
        refuse(
            core,
            spells,
            items,
            commands,
            CastFail::local(spell_id, reason),
        );
        core.reports.changed();
        return;
    }
    // The deferred cursor entry (`6e50c8`): every rung passed, the cursor waits for its click, and
    // the attempt signals as it returns (`0x6e51e3`).
    if let Some(word) = pending_word {
        debug!("ui_action: cast {spell_id} awaits its click — targeting cursor up ({word:#06x})");
        core.ground.enter_as(spell_id, commit, word, held.is_some());
        core.reports.changed();
        return;
    }
    // A candidate the arm bound goes through `BindTarget`'s merge, which signals before it sends
    // (`0x6e60cb`); a word of 0 sends with no bind (`0x6e5349`).
    if explicit_object.is_some() || item_target.is_some() || target.is_some() {
        core.reports.changed();
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
    // `SendCast 0x6e54f0`: one targets block, two opcodes, sent first (`0x6e5872`); everything the
    // send does after it follows the packet.
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
    // The send's own signal (`0x6e589d`), for the active player's send. The reference signals
    // only beside the precast visual it starts there (`0x6e5898`), so not for a spell whose
    // precast already plays (`0x6e588f`, the unit's `[+0xc8c]`, which `0x6ec220` sets through
    // `0x60d020`); benilla starts no precast at the send (#1583), so it signals every send.
    core.reports.changed();
    // The GCD arms at the send (`0x6e2de0` from `0x6e58fb`), after the packet; a failed cast
    // result clears it (`0x6e1630`).
    if let Some(d) = def {
        cooldowns.start_gcd(spell_id, d, now, spell_mods);
    }
    if let Some(d) = def {
        if let Ok((e, engaged)) = self_player.single() {
            // The ranged stance (`0x6e5930`, `SetSheatheState(2, 1, 1)`) for a ranged or
            // auto-repeat spell (`0x6e591c`, `0x6e5922`).
            if d.ranged_attack() {
                sheath.write(crate::creature_anim::SheathRequest {
                    entity: e,
                    state: 2,
                    ceremony: false,
                });
            }
            if d.auto_repeat() {
                // The sticky armed state (`0x6e593b`, `|= 0x200`) and the autorepeat key
                // (`0xceac30`, written at `0x6e594c`) the button's checked state reads.
                ecs.entity(e).insert(crate::creature_anim::AutoRepeatArmed);
                auto_repeat.0 = Some(spell_id);
                // StopAttack (`0x6e5976` → `0x5ecac0`), gated only on the caster being the active
                // player (`0x6e5957`–`0x6e5972`): starting a repeat ends the melee swing and its
                // queued strike, which the repeat's push moved into the save, so its cancel is
                // the packet alone. This is where melee and auto-repeat are kept exclusive. It
                // runs after the arm above, so it never cancels this repeat.
                crate::creature_anim::stop_attack_local(engaged, core);
            }
        }
    }
    // The in-flight slot (`0xceca88`, `0x6e5026`): every cast holds it until its result, a ranged
    // shot included; only an auto-repeat spell is popped after the send, last, which signals
    // (`0x6e597b`-`0x6e5989`).
    if def.is_some_and(|d| d.auto_repeat()) {
        core.pop_if(spell_id);
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
    // The attempt signals as it returns (`0x6e51e3`).
    core.reports.changed();
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

    static EMPTY_REPUTATIONS: Reputations = Reputations::from_slots(Vec::new());

    /// A context with nothing selected: every rung that needs world state is inert.
    fn ctx() -> cast_target::CastContext<'static> {
        cast_target::CastContext {
            selection_guid: None,
            self_guid: Some(0x0000_0000_0000_0007),
            auto_self_cast: false,
            rel: cast_target::TargetRelations {
                target_store: None,
                target_owner_store: None,
                self_store: None,
                factions: None,
                reputations: &EMPTY_REPUTATIONS,
                types: Default::default(),
                group: Default::default(),
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
        world.init_resource::<CastReports>();
        world.init_resource::<AutoRepeatActive>();
        world.init_resource::<crate::ui_tradeskill::TradeSkillOpens>();
        world.init_resource::<crate::spell::SpellTargeting>();
        world.init_resource::<crate::spell::HeldForPick>();
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

    /// The attempt fills its record with the player as the caster once every rung has passed
    /// (`0x6e4fb1`), so a sent cast and a raised cursor have the word `0x6e4940`'s events read; a
    /// refused rung does not reach it, and an arm refusal's report (`0x6e5064` → `0x6e1a00`) pops
    /// the spell just pushed and clears it.
    #[test]
    fn a_passing_attempt_fills_the_caster_word() {
        use benilla_formats::SpellDisplay;
        const PLAIN: u32 = 100;
        const OTHER: u32 = 101;
        const BLIZZARD: u32 = 10;
        const ROCKBITER: u32 = 8017;
        let take = |world: &mut World| {
            world
                .resource_mut::<crate::spell::PendingCast>()
                .take_caster_event()
        };

        let (mut world, _rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (PLAIN, SpellDisplay::default()),
                (OTHER, SpellDisplay::default()),
                (
                    BLIZZARD,
                    SpellDisplay {
                        targets: 0x40,
                        ..Default::default()
                    },
                ),
                (
                    ROCKBITER,
                    SpellDisplay {
                        targets: 0x10,
                        attributes: 0x0005_0200,
                        effects: [benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, 0, 0],
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        world.insert_resource(spells);

        send(&mut world, PLAIN, CastCommit::Spell);
        assert!(take(&mut world), "a sent cast fills it");

        // PLAIN is still in flight, so OTHER is refused at `0x6e4d97`, ahead of the fill.
        send(&mut world, OTHER, CastCommit::Spell);
        assert_eq!(
            world.resource::<CastReports>().refusals().len(),
            1,
            "refused 0x61"
        );
        assert!(!take(&mut world), "a refused attempt fills nothing");
        world
            .resource_mut::<crate::spell::PendingCast>()
            .clear_if(PLAIN);

        send(&mut world, BLIZZARD, CastCommit::Spell);
        assert!(world.resource::<crate::spell::SpellTargeting>().active());
        assert!(take(&mut world), "the cursor's spell has its record too");
        world.resource_mut::<crate::spell::SpellTargeting>().clear();

        world
            .resource_mut::<crate::spell::PendingCast>()
            .fill_record();
        send(&mut world, ROCKBITER, CastCommit::Spell);
        assert!(
            !take(&mut world),
            "the empty weapon hand's report popped the spell and spent the word"
        );
    }

    /// The arm's refusal (`0x6e5045`-`0x6e50a9`) comes after the attempt's push (`0x6e5026`), so
    /// its report finds the spell in flight and cancels it (`0x6e2218` → `0x6e4940(1, 1,
    /// reason)`): no word is up, so the cancel goes on the wire (`0x6e4970`), and the record's
    /// caster fires `SPELLCAST_FAILED` after the line, then the tradeskill disarm. The slot's push
    /// and pop and the attempt's arm and tail each signal `CURRENT_SPELL_CAST_CHANGED`.
    #[test]
    fn an_arm_refusal_cancels_the_spell_it_pushed_on_the_wire() {
        use crate::ui_cast::{CastBarEdge, CastReport};
        use benilla_formats::SpellDisplay;
        // Rockbiter Weapon, `ENCHANT_ITEM_TEMPORARY` on the weapon hand, with none worn.
        const ROCKBITER: u32 = 8017;
        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [(
                ROCKBITER,
                SpellDisplay {
                    targets: 0x10,
                    attributes: 0x0005_0200,
                    effects: [benilla_formats::SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, 0, 0],
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        );
        world.insert_resource(spells);
        send(&mut world, ROCKBITER, CastCommit::Spell);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [ClientCommand::CancelCast {
                    spell_id: ROCKBITER
                }]
            ),
            "the cancel and no cast: {sent:?}"
        );
        let reports = &world.resource::<CastReports>().0;
        assert!(
            matches!(
                reports[..],
                [
                    CastReport::Changed,
                    CastReport::Changed,
                    CastReport::Refused(CastFail {
                        spell_id: ROCKBITER,
                        ..
                    }),
                    CastReport::Changed,
                    CastReport::Bar(CastBarEdge::Failed),
                    CastReport::TradeSkillDisarm,
                    CastReport::Changed,
                ]
            ),
            "the push's and the arm's signals (`0x6e5026`, `0x6e5333`), the line, the pop's \
             signal, its event, the disarm, and the tail's signal (`0x6e5069`)"
        );
        assert!(!world.resource::<crate::spell::PendingCast>().in_flight());
    }

    /// A standing enemy word with nothing to bind is refused by the attempt's tail after the arm
    /// (`0x6e507b`-`0x6e50a9`, "You have no target" with no selection), so the spell was pushed:
    /// its report cancels it on the wire and fires `SPELLCAST_FAILED`.
    #[test]
    fn an_enemy_spell_with_no_target_is_refused_at_the_arm() {
        use benilla_formats::SpellDisplay;
        const FIREBALL: u32 = 133;
        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [(
                FIREBALL,
                SpellDisplay {
                    implicit_target_a1: 6,
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        );
        world.insert_resource(spells);
        send(&mut world, FIREBALL, CastCommit::Spell);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(sent[..], [ClientCommand::CancelCast { spell_id: FIREBALL }]),
            "{sent:?}"
        );
        let reports = world.resource::<CastReports>();
        assert_eq!(
            reports.refusals(),
            vec![CastFail::local(FIREBALL, cast_target::ERR_NO_TARGET)]
        );
        assert!(matches!(
            reports.edges()[..],
            [crate::ui_cast::CastBarEdge::Failed]
        ));
    }

    /// TryCast's dead rung (`0x6e4c49`): a corpse and a ghost (health 1, the `PLAYER_FLAGS` bit)
    /// are refused with 0x13 and no packet, after the profession intercept; `Attributes &
    /// 0x800000` waives it, and the Attack button's own entry takes the same rung.
    #[test]
    fn a_dead_or_ghost_caster_is_refused_as_dead_unless_the_spell_waives_it() {
        use crate::net::ObjectStore;
        use benilla_formats::SpellDisplay;
        use benilla_protocol::ObjectFields;
        const PLAIN: u32 = 100;
        const WHILE_DEAD: u32 = 200;
        const PROFESSION: u32 = 300;
        const ATTACK: u32 = crate::ui_action::SPELL_ATTACK;

        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (PLAIN, SpellDisplay::default()),
                (
                    WHILE_DEAD,
                    SpellDisplay {
                        attributes: benilla_formats::ATTR_CASTABLE_WHILE_DEAD,
                        ..Default::default()
                    },
                ),
                (
                    PROFESSION,
                    SpellDisplay {
                        effects: [benilla_formats::SPELL_EFFECT_TRADE_SKILL, 0, 0],
                        ..Default::default()
                    },
                ),
                // Attack's `Attributes` are `0x10`, no waiver.
                (
                    ATTACK,
                    SpellDisplay {
                        attributes: 0x10,
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        world.insert_resource(spells);
        // Fields 22 health, 28 max health, 190 `PLAYER_FLAGS`. Leaked: the closure must be 'static.
        let leak = |pairs: &[(u16, u32)]| -> &'static ObjectStore {
            Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(pairs))))
        };
        let corpse = leak(&[(22, 0), (28, 100)]);
        let ghost = leak(&[(22, 1), (28, 100), (190, 0x10)]);
        let press = |world: &mut World, spell_id: u32, store: &'static ObjectStore| {
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    let base = ctx();
                    let with_store = cast_target::CastContext {
                        rel: cast_target::TargetRelations {
                            self_store: Some(store),
                            ..base.rel
                        },
                        ..base
                    };
                    ladder.send(spell_id, &with_store, CastCommit::Spell);
                })
                .expect("one-shot");
        };

        for (label, store) in [("corpse", corpse), ("ghost", ghost)] {
            press(&mut world, PLAIN, store);
            assert!(
                rx.try_recv().is_err(),
                "{label}: a refused press never sends"
            );
            assert_eq!(
                world.resource_mut::<CastReports>().take_refusals(),
                vec![CastFail::local(PLAIN, 0x13)],
                "{label}: \"You are dead\""
            );
            let refused = world
                .run_system_once(move |mut ladder: CastLadder| {
                    ladder.dead_refusal(ATTACK, Some(store))
                })
                .expect("one-shot");
            assert!(refused, "{label}: the Attack button stops at the same rung");
            assert_eq!(
                world.resource_mut::<CastReports>().take_refusals(),
                vec![CastFail::local(ATTACK, 0x13)]
            );
        }

        // The profession intercept (`6e4bce`) comes first: the book opens, no error.
        press(&mut world, PROFESSION, ghost);
        assert_eq!(
            world.resource::<crate::ui_tradeskill::TradeSkillOpens>().0,
            vec![PROFESSION]
        );
        assert!(world.resource::<CastReports>().refusals().is_empty());

        press(&mut world, WHILE_DEAD, ghost);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == WHILE_DEAD),
            "the castable-while-dead attribute waives the rung"
        );
        assert!(world.resource::<CastReports>().refusals().is_empty());
    }

    /// A mashed chest opener goes out once as the GameObject block; the re-click is `6e4d43`'s
    /// same-spell bail, with no packet and no red line.
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
            world.resource::<CastReports>().refusals().is_empty(),
            "the same spell's re-press is the reference's same-spell bail (6e4d43), not \"Another \
             action is in progress\""
        );

        // A different cast mid-opener is the loud refusal (6e4d97).
        send(&mut world, MOUNT, CastCommit::Spell);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            world.resource::<CastReports>().refusals(),
            vec![CastFail::local(MOUNT, 0x61)],
            "a different spell mid-cast still errors 0x61"
        );
    }

    // ── Melee and auto-repeat exclusion ──
    //
    // The auto-repeat send calls StopAttack after its packet (`0x6e5976` → `0x5ecac0` →
    // `CancelQueuedCast 0x6e6f30`), and the attack-start's tail (`0x5ecd78`–`0x5ecd95`) cancels the running repeat.

    const AUTO_SHOT: u32 = 75;
    const AIMED_SHOT: u32 = 19434;
    const RAPTOR_STRIKE: u32 = 2973;
    const SERPENT_STING: u32 = 1978;
    const EVISCERATE: u32 = 2098;
    const MOB: u64 = 0x0000_0000_0000_00f1;

    /// Auto Shot (auto-repeat `AttributesEx2 & 0x20`, ranged `Attributes & 0x2`), Raptor Strike
    /// (on-next-swing `0x404`), Serpent Sting and Eviscerate (a finisher, `AttributesEx 0x100000`,
    /// that starts combat, `0x200`), as the 1.12 rows classify them. `Targets & 0x2` binds the
    /// selection without faction data.
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
        let eviscerate = benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes_ex: 0x0010_0200,
            ..Default::default()
        };
        // A timed hunter shot: ranged, not auto-repeat.
        let aimed_shot = benilla_formats::SpellDisplay {
            targets: 0x2,
            attributes: 0x0000_0002,
            modal_next_spell: AUTO_SHOT,
            ..Default::default()
        };
        Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(HashMap::from([
                (AIMED_SHOT, aimed_shot),
                (AUTO_SHOT, auto_shot),
                (RAPTOR_STRIKE, raptor_strike),
                (SERPENT_STING, serpent_sting),
                (EVISCERATE, eviscerate),
            ])),
            ..Spells::empty_for_tests()
        }
    }

    /// The attempt pushes every cast (`0x6e5026`) and only an auto-repeat spell is popped after the
    /// send (`0x6e5935` → `0x6e5989`): a ranged shot that is not auto-repeat holds the slot until
    /// its result, and a press meanwhile meets `6e4d97`'s 0x61, or for the same spell `6e4d43`'s
    /// bail with no red line. Auto Shot and a queued strike never occupy the refusal.
    #[test]
    fn a_ranged_shot_holds_the_slot_until_its_result_and_auto_repeat_is_popped() {
        let (mut world, rx) = combat_world(false);
        send_at(&mut world, AIMED_SHOT, MOB);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == AIMED_SHOT),
            "the shot goes out"
        );
        let pending = world.resource::<crate::spell::PendingCast>();
        assert_eq!(pending.current(), Some(AIMED_SHOT), "and holds the slot");
        assert!(pending.in_flight());

        send_at(&mut world, SERPENT_STING, MOB);
        assert!(
            rx.try_recv().is_err(),
            "a press during the cast is not sent"
        );
        assert_eq!(
            world.resource_mut::<CastReports>().take_refusals(),
            vec![CastFail::local(SERPENT_STING, 0x61)],
            "\"Another action is in progress\""
        );
        send_at(&mut world, AIMED_SHOT, MOB);
        assert!(rx.try_recv().is_err());
        assert!(
            world.resource::<CastReports>().refusals().is_empty(),
            "the same shot again is the same-spell bail, with no red line"
        );

        // The result clears it, as `cast_result` does, and the next press goes out.
        world
            .resource_mut::<crate::spell::PendingCast>()
            .clear_if(AIMED_SHOT);
        send_at(&mut world, SERPENT_STING, MOB);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == SERPENT_STING
        ));

        // Auto Shot is popped right after its send: the slot is empty and the next press goes.
        let (mut world, rx) = combat_world(false);
        send_at(&mut world, AUTO_SHOT, MOB);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == AUTO_SHOT
        ));
        assert_eq!(
            world.resource::<crate::spell::PendingCast>().current(),
            None,
            "an auto-repeat spell is popped after the send"
        );
        assert_eq!(world.resource::<AutoRepeatActive>().0, Some(AUTO_SHOT));
        send_at(&mut world, SERPENT_STING, MOB);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == SERPENT_STING
        ));
        assert!(world.resource::<CastReports>().refusals().is_empty());

        // A queued strike lives in its own slot, which the refusal exempts (`6e4dbe`).
        let (mut world, rx) = combat_world(false);
        send_at(&mut world, RAPTOR_STRIKE, MOB);
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == RAPTOR_STRIKE
        ));
        assert_eq!(
            world.resource::<crate::spell::PendingCast>().current(),
            None
        );
        send_at(&mut world, SERPENT_STING, MOB);
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            sent.iter().any(|c| matches!(
                c,
                ClientCommand::CastSpell { spell_id, .. } if *spell_id == SERPENT_STING
            )),
            "a cast over a queued strike goes out: {sent:?}"
        );
        assert!(world.resource::<CastReports>().refusals().is_empty());
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
        // What `cast_result` returns and `on_cast_result` hands to the ladder, after the result has
        // popped the sting (`0x6e741a`).
        world
            .resource_mut::<crate::spell::PendingCast>()
            .clear_if(SERPENT_STING);
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
            .push(FROSTBOLT, None, Instant::now());

        send_at(&mut world, AUTO_SHOT, MOB);

        assert!(rx.try_recv().is_err(), "nothing goes out on the wire");
        assert_eq!(
            world.resource::<CastReports>().refusals(),
            vec![CastFail::local(AUTO_SHOT, 0x61)],
            "a different spell mid-cast is \"Another action is in progress\", whatever its class"
        );
        assert_eq!(
            world.resource::<crate::spell::PendingCast>().current(),
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

    /// TryCast's attack pick (`6e4edf`): a press of a `0x6e5200` spell stops there, above the
    /// bind, for its owner to pick a unit. Attacking skips the pick (`0x60ecb0`), and neither a
    /// spell without the bits nor a send that is not a press stops.
    #[test]
    fn a_combat_press_holds_at_the_attack_pick_unless_attacking() {
        let press = |world: &mut World, spell_id: u32| {
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    ladder.send_spell(spell_id, &ctx(), false);
                    ladder.take_held()
                })
                .expect("the ladder runs as a one-shot system")
        };
        let (mut world, rx) = combat_world(false);
        assert_eq!(
            press(&mut world, RAPTOR_STRIKE),
            Some(HeldCast {
                spell_id: RAPTOR_STRIKE,
                on_self: false
            })
        );
        assert!(rx.try_recv().is_err(), "a held press sends nothing");
        assert!(world.resource::<CastReports>().refusals().is_empty());
        assert_eq!(press(&mut world, SERPENT_STING), None, "no 0x6e5200 bit");

        let (mut world, _rx) = combat_world(true);
        assert_eq!(press(&mut world, RAPTOR_STRIKE), None, "attacking");

        let (mut world, _rx) = combat_world(false);
        send(&mut world, RAPTOR_STRIKE, CastCommit::Spell);
        assert_eq!(world.resource::<HeldForPick>().0, None, "not a press");
    }

    /// The rungs above the pick refuse before it: a press mid-cast (`6e4d97`) and a finisher with no
    /// combo points (`6e4e50`) hold nothing, so they never select a unit. A rogue reads 0x46, any
    /// other class 0x12 (`6e4e98`).
    #[test]
    fn the_rungs_above_the_attack_pick_refuse_before_it() {
        use benilla_protocol::ObjectFields;
        let (mut world, rx) = combat_world(false);
        world
            .resource_mut::<crate::spell::PendingCast>()
            .push(SERPENT_STING, None, Instant::now());
        let held = world
            .run_system_once(|mut ladder: CastLadder| {
                ladder.send_spell(RAPTOR_STRIKE, &ctx(), false);
                ladder.take_held()
            })
            .expect("the ladder runs as a one-shot system");
        assert_eq!(held, None, "in flight");
        assert_eq!(
            world.resource::<CastReports>().refusals(),
            vec![CastFail::local(RAPTOR_STRIKE, 0x61)]
        );
        assert!(rx.try_recv().is_err());

        // `UNIT_FIELD_BYTES_0` (field 36) byte 1 is the class; `PLAYER_FIELD_BYTES` holds no
        // combo points.
        for (class, reason) in [(4u32, 0x46), (11, 0x12)] {
            let (mut world, rx) = combat_world(false);
            let held = world
                .run_system_once(move |mut ladder: CastLadder| {
                    let me = crate::net::ObjectStore(ObjectFields::from_pairs(&[(36, class << 8)]));
                    let ctx = cast_target::CastContext {
                        rel: cast_target::TargetRelations {
                            self_store: Some(&me),
                            ..ctx().rel
                        },
                        ..ctx()
                    };
                    ladder.send_spell(EVISCERATE, &ctx, false);
                    ladder.take_held()
                })
                .expect("the ladder runs as a one-shot system");
            assert_eq!(held, None, "class {class}: no combo points");
            assert_eq!(
                world.resource::<CastReports>().refusals(),
                vec![CastFail::local(EVISCERATE, reason)],
                "class {class}"
            );
            assert!(rx.try_recv().is_err());
        }
    }

    /// The send's StopAttack (`0x6e5976`), after the packet (`0x6e5872`) and the auto-repeat arm:
    /// `CMSG_ATTACKSTOP` (`0x624370`), then `CancelQueuedCast 0x6e6f30`, whose save leg sends the
    /// queued strike's `CMSG_CANCEL_CAST`; the pop comes last (`0x6e5989`), so the save it copies
    /// back is already empty.
    #[test]
    fn an_auto_repeat_press_stops_the_attack_and_takes_the_queued_strike_with_it() {
        let (mut world, rx) = combat_world(true);
        world
            .resource_mut::<crate::spell::QueuedMeleeSpell>()
            .arm(RAPTOR_STRIKE);

        send_at(&mut world, AUTO_SHOT, MOB);

        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == AUTO_SHOT),
            "the shot leaves first"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::AttackStop)),
            "then the send's StopAttack"
        );
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CancelCast { spell_id }) if spell_id == RAPTOR_STRIKE),
            "whose tail cancels the queued strike by id"
        );
        assert!(rx.try_recv().is_err(), "and nothing else");
        assert_eq!(
            world.resource::<crate::spell::QueuedMeleeSpell>().current(),
            None,
            "the queue is empty — the strike's checked ring goes dark"
        );
        // The shot's push moved the strike into the save, whose cancel (`0x6e6f9d`-`0x6e701a`)
        // signals nothing: the press signals as one with no strike queued.
        let (mut bare, _rx) = combat_world(true);
        send_at(&mut bare, AUTO_SHOT, MOB);
        assert_eq!(stream(&mut world), stream(&mut bare));
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

    /// Cache the template of item `entry` with `flags`.
    fn cache_item(world: &mut World, entry: u32, flags: u32) {
        let mut template = crate::items::test_template("item");
        template.flags = flags;
        world
            .resource_mut::<Items>()
            .insert_template(entry, Some(template));
    }

    /// The reports queued, as names: a change, a bar edge, a refusal, the disarm.
    fn stream(world: &mut World) -> Vec<&'static str> {
        world
            .resource_mut::<CastReports>()
            .0
            .drain(..)
            .map(|r| match r {
                crate::ui_cast::CastReport::Changed => "CHANGED",
                crate::ui_cast::CastReport::Bar(crate::ui_cast::CastBarEdge::Failed) => "FAILED",
                crate::ui_cast::CastReport::Bar(crate::ui_cast::CastBarEdge::Stop) => "STOP",
                crate::ui_cast::CastReport::Bar(_) => "BAR",
                crate::ui_cast::CastReport::Refused(_) => "REFUSED",
                crate::ui_cast::CastReport::TradeSkillDisarm => "DISARM",
            })
            .collect()
    }

    /// An item's cursor holds the slot only when the item carries `ITEM_FLAG_PLAYERCAST`, as the
    /// attempt pushes before the arm raises the cursor (`0x6e500b`-`0x6e5026`). Pressing a grenade
    /// again with its cursor up is then no same-spell bail (`0x6e4d43` reads the slot): the press
    /// supersedes the cursor (`0x6e4d71`), whose cancel signals the word's clear alone and fires
    /// `SPELLCAST_FAILED`, and raises it again; a flagged item's cursor bails.
    #[test]
    fn an_item_cursor_holds_the_slot_only_with_the_flag() {
        const BOMB: u32 = 4360;
        const BOMB_SPELL: u32 = 4064;
        let commit = CastCommit::Item {
            bag_index: 255,
            slot: 24,
            entry: BOMB,
            spell_index: 0,
            on_object: None,
        };
        for (flags, in_slot) in [
            (0, false),
            (benilla_protocol::messages::ITEM_FLAG_PLAYERCAST, true),
        ] {
            let (mut world, rx) = world();
            let mut spells = Spells::empty_for_tests();
            spells.catalog = benilla_formats::SpellCatalog::from_displays(
                [(
                    BOMB_SPELL,
                    benilla_formats::SpellDisplay {
                        targets: 0x40,
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
            );
            world.insert_resource(spells);
            cache_item(&mut world, BOMB, flags);

            send(&mut world, BOMB_SPELL, commit);
            let ground = world.resource::<crate::spell::SpellTargeting>();
            assert_eq!(ground.spell(), Some(BOMB_SPELL), "flags {flags:#x}");
            assert_eq!(ground.in_slot(), in_slot.then_some(BOMB_SPELL));
            let first = stream(&mut world);
            assert_eq!(
                first,
                if in_slot {
                    vec!["CHANGED", "CHANGED", "CHANGED"]
                } else {
                    vec!["CHANGED", "CHANGED"]
                },
                "the push only with the flag, then the arm and the cursor's return"
            );

            send(&mut world, BOMB_SPELL, commit);
            assert_eq!(
                stream(&mut world),
                if in_slot {
                    vec![]
                } else {
                    vec!["CHANGED", "FAILED", "DISARM", "CHANGED", "CHANGED"]
                },
                "flags {flags:#x}"
            );
            assert!(world.resource::<crate::spell::SpellTargeting>().active());
            assert!(rx.try_recv().is_err(), "a cursor sends nothing");
        }
    }

    /// The attempt's in-flight rungs and the report's tail read one slot (`0xceca88`): a cast
    /// running outside it (an item's without `ITEM_FLAG_PLAYERCAST`, or a guard that lapsed) is no
    /// same-spell bail for a press of its spell (`0x6e4d43`), and that press's refusal finds
    /// nothing in flight (`0x6e2201`): `SPELLCAST_FAILED`, and no `CMSG_CANCEL_CAST`.
    #[test]
    fn a_refusal_of_a_cast_outside_the_slot_sends_no_cancel() {
        let (mut world, rx) = world();
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
        world.spawn((
            SelfPlayer,
            crate::creature_anim::Casting {
                spell_id: MOUNT,
                until: None,
            },
        ));

        send(&mut world, MOUNT, CastCommit::Spell);
        assert!(rx.try_recv().is_err(), "nothing goes out");
        assert_eq!(stream(&mut world), ["REFUSED", "FAILED"]);
    }

    /// An item use whose flags carry `ITEM_FLAG_PLAYERCAST` (the Hearthstone's 64) holds the slot
    /// like a spell: a double-click bails with no red line (`6e4d43`), a different spell errors
    /// 0x61 (`6e4d97`), and nothing reaches the wire.
    #[test]
    fn a_double_clicked_item_never_ships_the_duplicate() {
        let (mut world, rx) = world();
        cache_item(&mut world, 6948, 0x40);

        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::UseItem { .. })),
            "the first click commits as USE_ITEM and pushes the one inflight id"
        );

        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert!(rx.try_recv().is_err(), "no duplicate on the wire");
        assert!(
            world.resource::<CastReports>().refusals().is_empty(),
            "the same spell's re-press is the reference's same-spell bail (6e4d43), not a red line"
        );

        send(&mut world, MOUNT, HEARTH_COMMIT);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            world.resource::<CastReports>().refusals(),
            vec![CastFail::local(MOUNT, 0x61)],
            "a different spell mid-cast is 6e4d97's \"Another action is in progress\""
        );
    }

    /// An item whose flags lack `ITEM_FLAG_PLAYERCAST` (a potion, food, a healthstone: 0 in
    /// `item_template`) never enters the slot (`0x6e501d`), so the spell pressed right after goes
    /// out; an item not cached reads the same.
    #[test]
    fn a_potion_never_holds_the_slot_so_the_next_spell_is_not_refused() {
        const MINOR_HEALING_POTION: u32 = 118;
        const POTION_SPELL: u32 = 439;
        let potion = CastCommit::Item {
            bag_index: 255,
            slot: 25,
            entry: MINOR_HEALING_POTION,
            spell_index: 0,
            on_object: None,
        };
        for cached in [true, false] {
            let (mut world, rx) = world();
            if cached {
                cache_item(&mut world, MINOR_HEALING_POTION, 0);
            }
            send(&mut world, POTION_SPELL, potion);
            assert!(matches!(rx.try_recv(), Ok(ClientCommand::UseItem { .. })));
            assert_eq!(
                world.resource::<crate::spell::PendingCast>().current(),
                None,
                "cached={cached}: the potion pushed nothing"
            );

            send(&mut world, MOUNT, CastCommit::Spell);
            assert!(
                matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == MOUNT),
                "cached={cached}: the spell goes out behind the potion"
            );
            assert!(world.resource::<CastReports>().refusals().is_empty());
        }
    }

    /// The spell awaiting its click is in flight (`0x6e5026` precedes the arm that raises the
    /// cursor): pressing it again bails with no red line and the cursor up (`6e4d43`, ahead of the
    /// IsTargeting leg at `6e4d62`), and another spell drops the cursor and goes on.
    #[test]
    fn the_spell_awaiting_its_click_bails_on_a_repress_and_another_supersedes_it() {
        let (mut world, rx) = world();
        world.resource_mut::<crate::spell::SpellTargeting>().enter(
            HEARTHSTONE,
            CastCommit::Spell,
            0x40,
        );

        send(&mut world, HEARTHSTONE, CastCommit::Spell);
        assert!(rx.try_recv().is_err(), "nothing goes out");
        assert!(
            world.resource::<CastReports>().refusals().is_empty(),
            "and no red line"
        );
        assert_eq!(
            world.resource::<crate::spell::SpellTargeting>().spell(),
            Some(HEARTHSTONE),
            "the cursor stays up"
        );

        send(&mut world, MOUNT, CastCommit::Spell);
        assert!(!world.resource::<crate::spell::SpellTargeting>().active());
        assert!(matches!(
            rx.try_recv(),
            Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == MOUNT
        ));
        assert!(world.resource::<CastReports>().refusals().is_empty());
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
            world.resource::<CastReports>().refusals(),
            vec![CastFail::local(HEARTHSTONE, 0x28)]
        );

        // The record is keyed (use-spell, entry); a spell press queries (spell, 0) and passes.
        world.resource_mut::<CastReports>().0.clear();
        world.insert_resource(crate::spell::PendingCast::default());
        send(&mut world, HEARTHSTONE, CastCommit::Spell);
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { .. })),
            "the item-keyed record never not-readies a bare spell press"
        );
        assert!(world.resource::<CastReports>().refusals().is_empty());
    }

    /// The item press queries with its own spell-slot category (`0x6e1457`-`0x6e1499`): another
    /// item's running category-28 timer refuses it 0x28, though the spell row carries no category.
    #[test]
    fn an_item_press_not_readies_on_its_own_category() {
        use benilla_protocol::messages::ItemUseSpell;
        let (mut world, rx) = world();
        let mut template = crate::items::test_template("Hearthstone");
        template.use_spell = Some(ItemUseSpell {
            spell_id: HEARTHSTONE,
            cooldown_ms: 0,
            category: 28,
            category_cooldown_ms: 0,
        });
        world
            .resource_mut::<Items>()
            .insert_template(6948, Some(template));
        let other = ItemUseSpell {
            spell_id: MOUNT,
            cooldown_ms: 0,
            category: 28,
            category_cooldown_ms: 30_000,
        };
        world.resource_mut::<crate::spell::Cooldowns>().start_item(
            7001,
            &other,
            None,
            Instant::now(),
        );

        send(&mut world, HEARTHSTONE, HEARTH_COMMIT);
        assert!(
            rx.try_recv().is_err(),
            "the shared category holds the press"
        );
        assert_eq!(
            world.resource::<CastReports>().refusals(),
            vec![CastFail::local(HEARTHSTONE, 0x28)]
        );
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
            world.resource::<CastReports>().refusals(),
            vec![CastFail::local(HEARTHSTONE, 0x4d)]
        );

        world.resource_mut::<CastReports>().0.clear();
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
        assert!(world.resource::<CastReports>().refusals().is_empty());
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
            world.resource::<CastReports>().refusals(),
            vec![CastFail::local(MOUNT, 0x61)],
            "mid-cast outranks the cooldown rung (the reference's IsCasting precedes the validator)"
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
            world.resource::<crate::spell::PendingCast>().in_flight(),
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
