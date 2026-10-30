//! The one cast-send path: every spell and item use leaves through [`send_spell_cast`], which runs
//! the reference's `TryCast 0x6e4b60` and its commit `SendCast 0x6e54f0` rung for rung.
//!
//! The rungs: the auto-repeat toggle-off (the reference's action-button handler, ahead of TryCast;
//! its order against the profession intercept is unobservable), the profession intercept, dead or a
//! ghost, the loot window, the targeting abort, in-flight, reagents and totems, the equipped item
//! and its ammo, the ranged weapon, combo points, the attack pick, target binding and range, then
//! the validator `0x6094f0` (not-ready and GCD, power, then [`validator::caster_refusal`]'s rungs),
//! the deferred cast-arm refusal and the targeting cursor. The commit tail follows: ranged stance,
//! auto-repeat arm, the send, the auto-attack start, the GCD. A refusal is local and pre-commit: no
//! packet, no GCD, no pending arm, only the red line.
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

mod unit_caster;
pub(crate) use unit_caster::UnitAttackOwed;
use unit_caster::UnitCaster;

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
        /// The item object's guid, which the in-flight slot keeps beside its spell (`0xcecab0`,
        /// stored at `0x6e4b00`); `None` for a use with no live instance.
        guid: Option<u64>,
        spell_index: u8,
        /// `CGItem::Use`'s own target argument, set only for a key in a lock; it bypasses the
        /// binder, as TryCast's target resolve (`6e4ef4`) takes the pair it was passed.
        on_object: Option<u64>,
    },
    /// `CMSG_PET_CAST_SPELL` (0x1f0): the record's caster (`+0x08`) is a unit other than the
    /// active player (`0x6e5808` asks `0x5fa6d0`), as for the pet bar's client-targeted and
    /// possessed presses (`0x4bd385`); its guid leads the body (`0x6e582d`).
    Pet { caster: u64 },
}

impl CastCommit {
    /// Whether this commit carries an item: what every forked rung reads.
    pub(crate) fn is_item(self) -> bool {
        matches!(self, CastCommit::Item { .. })
    }

    /// The used item's guid, which the attempt's push keeps in the in-flight slot (`0x6e4b00`).
    pub(crate) fn item_guid(self) -> Option<u64> {
        match self {
            CastCommit::Item { guid, .. } => guid,
            CastCommit::Spell | CastCommit::Pet { .. } => None,
        }
    }

    /// The caster when it is not the active player ([`CastCommit::Pet`]).
    pub(crate) fn unit_caster(self) -> Option<u64> {
        match self {
            CastCommit::Pet { caster } => Some(caster),
            CastCommit::Spell | CastCommit::Item { .. } => None,
        }
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

/// Where a call's cast waits at each signal ahead of a later send in it, `SendCast`'s entry
/// (`0x6e54f0`) the first, for the call host to run the listeners there and resume it
/// ([`CastLadder::take_parked`], [`CastLadder::resume_parked`]): `SignalEvent` (`0x703e50`) runs
/// them inside the cast. With no host armed, the cast runs straight through.
#[derive(Resource, Default)]
pub(crate) struct CastPark {
    armed: bool,
    parked: Option<CastStage>,
}

/// What `SendCast 0x6e54f0` sends: the pending record (`0xceac48`) as the last attempt or cursor
/// bind to reach it left it, its targets block built into the packet. A listener's attempt
/// refills the record ([`crate::spell::PendingCast::fill_record`]), so the send a cast resumes
/// past a point reads it again.
#[derive(Clone, Debug)]
pub(crate) struct CastSend {
    spell_id: u32,
    commit: CastCommit,
    packet: ClientCommand,
}

/// What `TryCast`'s tail reads past its send (`0x6e51b5`-`0x6e51e3`): its own spell, its guid
/// argument as the pick left it (`[ebx+0xc]`), the press's time, and whether control was lost
/// (`[0xb4b3e4]`, which its StartAttack reads, `0x5ecbc0`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Attempt {
    spell_id: u32,
    attack_guid: Option<u64>,
    now: Instant,
    control_lost: bool,
}

/// Where a cast stands between a signal and the send after it ([`CastPark`]).
#[derive(Clone, Debug)]
pub(crate) enum CastStage {
    /// At `SendCast`'s entry, past the attempt's push and arm signals (`0x6e5026`, `0x6e5333`) or
    /// a bind's merge (`0x6e60cb`, `0x6e615d`, `0x6e6939`). The arm, the location bind and the
    /// trade-slot bind test the word again past their signal (`0x6e5338`, `0x6e6166`,
    /// `0x6e693e`), `rereads_word`; the object bind tested it before (`0x6e60c1`). `attempt` is
    /// `None` for a cursor's commit, which is past `TryCast`.
    Send {
        rereads_word: bool,
        attempt: Option<Attempt>,
    },
    /// Past the send's own signal (`0x6e589d`), `spell_id` the record's.
    Sent { spell_id: u32, attempt: Attempt },
    /// Past `START_AUTOREPEAT_SPELL` (`0x6e5952`): the repeat's StopAttack next (`0x6e5976`).
    AutoShot { spell_id: u32, attempt: Attempt },
    /// That StopAttack past its tutorials' signal (`0x5ecb17`, `0x5ecb54`): its send and its
    /// next-swing cancel (`0x5ecb5b`, `0x6e6f68`).
    AutoShotSend {
        spell_id: u32,
        stop: crate::creature_anim::StopTail,
        attempt: Attempt,
    },
    /// Past the stop's next-swing cancel: its save's leg (`0x6e6ff3`).
    AutoShotSave { spell_id: u32, attempt: Attempt },
    /// Past the repeat's pop (`0x6e5989`), or the send's tail with nothing to signal:
    /// `TryCast`'s attack tail (`0x6e51b5`).
    Popped { attempt: Attempt },
    /// A unit caster's send past its pop's signal (`0x6e58a2`-`0x6e58bd`): an attempt's attack
    /// tail (`0x6e51b5`-`0x6e51de`), which the host runs, then its return.
    UnitSent {
        attempt: Option<Attempt>,
        caster: u64,
    },
    /// An attempt past its attack tail: it signals as it returns (`0x6e51e3`).
    Returned,
    /// A cursor's commit past the send's own signal (`0x6e589d`): its pop and its GCD.
    CursorSent { spell_id: u32, now: Instant },
    /// A cursor's commit past its pop's signal, with nothing left.
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeldCast {
    pub(crate) spell_id: u32,
    /// Pressed with the self-cast modifier: TryCast's guid argument is then the caster
    /// (`0x4e610e`), which the pick tries before the selection.
    pub(crate) on_self: bool,
    /// The pet bar's press with its unit as the caster (`0x4bd385`), `None` for the active
    /// player's own.
    pub(crate) unit: Option<UnitPress>,
}

/// A unit caster's press held at the attack pick: the caster, the pick's actor (`0x6e4ef4`), and
/// TryCast's guid argument, the press's target, which the pick takes as its candidate (`0x6e4ef7`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UnitPress {
    pub(crate) caster: u64,
    pub(crate) target: Option<u64>,
}

/// Whether the script call being applied passed the protected-action gate's tag 2, which opens
/// `TryCastSpell` (`0x6e4bc1`, `0x494a50`): the answer the Lua verb got, where the stack that decides
/// it was still there. A cast the engine starts (a click, a chain, a held press resumed) asks no
/// gate, so it reads the default, passed; [`crate::script_calls`] sets it per call and puts back
/// what it found, so a call nested at another's signal point leaves the outer call's answer.
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
    /// Our own entity; whether we attack is the queued edits' answer
    /// ([`crate::creature_anim::UnitEdits::attacking`]). The pair's second leg is always set.
    pub(crate) self_player: Query<'w, 's, (Entity, Has<SelfPlayer>), With<SelfPlayer>>,
    pub(crate) spells: Option<Res<'w, Spells>>,
    pub(crate) items: Res<'w, Items>,
    /// `ItemSubClass.dbc`, whose `Flags` bit `0x10` lets a weapon fire without ammo (`0x6e4264`);
    /// absent when the data did not load.
    pub(crate) sub_classes: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    /// The reagent check walks the bags through it; the item arms resolve an instance guid here.
    pub(crate) objects: Objects<'w, 's>,
    pub(crate) sheath: MessageWriter<'w, crate::creature_anim::SheathRequest>,
    /// The local apply of the stand-up an attack-initiating cast makes (`0x5ecc9b`).
    pub(crate) stand: MessageWriter<'w, crate::player::StandStateRequest>,
    /// The auto-repeat arm and clear, applied after the pass.
    pub(crate) edits: Res<'w, crate::creature_anim::UnitEdits>,
    /// Whether a send's spell resolves a visual, which its precast needs (`0x6ec220`).
    pub(crate) precast_visuals: crate::creature_anim::PrecastVisuals<'w, 's>,
    pub(crate) pending: ResMut<'w, crate::spell::PendingCast>,
    pub(crate) queued_melee: ResMut<'w, crate::spell::QueuedMeleeSpell>,
    pub(crate) cooldowns: ResMut<'w, crate::spell::Cooldowns>,
    /// The talent spell modifiers the power gate's cost goes through.
    pub(crate) spell_mods: Res<'w, crate::spell::SpellModifiers>,
    /// The cast's reports, in the order they are made ([`crate::ui_cast`]).
    pub(crate) reports: ResMut<'w, CastReports>,
    /// The tutorial bank ([`CastCore::tutorials`]).
    pub(crate) tutorials: Option<Res<'w, crate::tutorial::Tutorials>>,
    /// The skill lines the cost's and the cast time's level is found through (`0x5ea690`).
    pub(crate) skill_lines: Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    pub(crate) auto_repeat: ResMut<'w, AutoRepeatActive>,
    pub(crate) trade_skill_opens: ResMut<'w, crate::ui_tradeskill::TradeSkillOpens>,
    pub(crate) ground: ResMut<'w, super::targeting::SpellTargeting>,
    pub(crate) held_for_pick: ResMut<'w, HeldForPick>,
    /// Absent in a world with no call host, whose casts run straight through.
    park: Option<ResMut<'w, CastPark>>,
    /// The loot guid `[player+0x1d28]`, which the loot rungs read (`GetLootGuid 0x6126f0`) and an
    /// item open arms; one owner, so a caster surface that sets it holds no second copy. Absent
    /// in a world that never loots.
    pub(crate) loot_latch: Option<ResMut<'w, crate::ui_loot::LootLatch>>,
    /// The active player's guid, which a unit caster's rungs compare against (`0x468550`).
    active: Option<Res<'w, crate::net::SelfGuid>>,
    /// The pet bar, whose book and unit the validator's learn rungs read (`0x60c740`-`0x60c8d0`)
    /// and whose history a unit caster's send starts its GCD in (`0x6e58fb`); absent in a world
    /// with no pet bar.
    pet_bar: Option<ResMut<'w, crate::ui_pet::PetBar>>,
    /// The attack a unit caster's send owes the pet bar (`0x6e51de`); absent in a world with no
    /// pet bar.
    unit_attack: Option<ResMut<'w, UnitAttackOwed>>,
    /// Our stop in flight (`[+0xc54]`).
    stop_word: ResMut<'w, crate::creature_anim::AttackStopInFlight>,
    /// What the validator's learn rungs ask the caster and its pet know (`0x60c740`-`0x60c8d0`).
    known: KnownReads<'w>,
}

/// The known spells the validator's learn rungs read: the active player's book (`0xb710fc`), and
/// the pet's records and family lines, beside the bar the ladder holds.
#[derive(SystemParam)]
pub(crate) struct KnownReads<'w> {
    actions: Option<Res<'w, crate::ui_action::PlayerActions>>,
    names: Option<Res<'w, crate::names::NameCache>>,
    families: Option<Res<'w, crate::ui_pet_stats::PetFamilyTables>>,
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
        // The lookup (`0x6e1ea4`), asking on a miss; the line's own registers the callback
        // `0x6e29b0` ([`crate::ui_action::CastFailLines`]).
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
    /// The partner's trade slot, the bind of `0x6e6900` (mask `0x1000`, guid `{6, 0}`).
    TradeItem(u8),
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
            tutorials: self.tutorials.as_deref(),
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

    /// `UseAction`'s re-press toggle (`0x4e60d1`-`0x4e60da`): a slot whose spell is the running
    /// auto-repeat spell (`0x6e9fd0`) stops it (`0x6ea080`) in place of a cast. Whether it did.
    pub(crate) fn stop_auto_repeat_of(&mut self, spell_id: u32) -> bool {
        if self.auto_repeat.0 != Some(spell_id) {
            return false;
        }
        let me = self.self_player.single().ok().map(|(e, _)| e);
        let mut core = CastCore {
            pending: &mut self.pending,
            queued_melee: &mut self.queued_melee,
            ground: &mut self.ground,
            reports: &mut self.reports,
            net: &self.commands,
            tutorials: self.tutorials.as_deref(),
        };
        crate::creature_anim::cancel_auto_repeat_local(
            me,
            &mut self.auto_repeat,
            &self.edits,
            &mut core,
        );
        true
    }

    /// Set what the gate's tag 2 allowed the script call about to be applied; what it was before,
    /// for the call to put back as it returns.
    pub(crate) fn permit(&mut self, may_cast: bool) -> bool {
        match self.permit.as_mut() {
            Some(permit) => std::mem::replace(&mut permit.0, may_cast),
            None => true,
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
                TargetedBind::TradeItem(slot) => {
                    ClientCommand::CastSpellTradeItem { spell_id, slot }
                }
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
                target: bound.block(),
            },
            CastCommit::Pet { caster } => ClientCommand::PetCastSpell {
                caster_guid: caster,
                spell_id,
                target: bound.block(),
            },
        };
        // The spell its attempt pushed stays in the slot through the send, as for a cast sent at
        // its attempt.
        if self.ground.in_slot() == Some(spell_id) {
            self.pending.push_cast(
                spell_id,
                item_flags(&self.items, commit),
                commit.item_guid(),
            );
        }
        // `BindTarget` wrote the clicked unit, game object or corpse into the record's target
        // (`0x6e5cdc`, `0x6e5f77`, `0x6e5fbe`).
        if let TargetedBind::Unit(guid)
        | TargetedBind::Object(guid)
        | TargetedBind::Corpse(guid, _) = bound
        {
            self.pending.bind_target(guid);
        }
        self.pending.set_record_send(CastSend {
            spell_id,
            commit,
            packet: cmd,
        });
        // The bind emptied the word, and signals before it sends (`0x6e60cb`, `0x6e615d`,
        // `0x6e6939`).
        self.ground.clear();
        self.reports.changed();
        let rereads_word = matches!(
            bound,
            TargetedBind::Dest(_) | TargetedBind::Source(_) | TargetedBind::TradeItem(_)
        );
        self.proceed(CastStage::Send {
            rereads_word,
            attempt: None,
        });
    }

    /// Arm or disarm the park for the call about to be applied ([`CastPark`]); what it was, for
    /// the call to put back as it returns.
    pub(crate) fn arm_park(&mut self, armed: bool) -> bool {
        match self.park.as_mut() {
            Some(park) => std::mem::replace(&mut park.armed, armed),
            None => false,
        }
    }

    /// The cast parked at a signal, for its host to run the listeners and resume it.
    pub(crate) fn take_parked(&mut self) -> Option<CastStage> {
        self.park.as_mut().and_then(|p| p.parked.take())
    }

    /// Park `stage` again, taken out while its host ran a step of its own between
    /// ([`Self::take_parked`]).
    pub(crate) fn park(&mut self, stage: CastStage) {
        if let Some(park) = self.park.as_mut() {
            debug_assert!(park.parked.is_none(), "one cast parked at a time");
            park.parked = Some(stage);
        }
    }

    /// A parked cast past its listeners, up to its next signal or its end.
    pub(crate) fn resume_parked(&mut self, stage: CastStage) {
        if let Some(next) = self.run_stage(stage) {
            self.proceed(next);
        }
    }

    /// Park `stage` for the host when one is armed, else run the cast to its end.
    fn proceed(&mut self, mut stage: CastStage) {
        if let Some(park) = self.park.as_mut().filter(|p| p.armed) {
            debug_assert!(park.parked.is_none(), "one cast parked at a time");
            park.parked = Some(stage);
            return;
        }
        while let Some(next) = self.run_stage(stage) {
            stage = next;
        }
    }

    /// The active player's descriptor as it stands, which the send's StopAttack and `TryCast`'s
    /// attack tail read where they run.
    fn self_store(&self) -> Option<&crate::net::ObjectStore> {
        self.objects.store(self.active.as_ref()?.0?)
    }

    /// One stage of a cast's send, up to the next signal a later send waits behind: the stage
    /// that follows it, `None` at the cast's end.
    fn run_stage(&mut self, stage: CastStage) -> Option<CastStage> {
        let me = self.self_player.single().ok().map(|(e, _)| e);
        match stage {
            CastStage::Send {
                rereads_word,
                attempt,
            } => {
                // A cursor a listener raised stands in the word read again (`0x6e5338`), which
                // takes the arm's bind leg (`0x6e5359`) with that cursor's word, so this send does
                // not go out.
                if rereads_word && self.ground.active() {
                    debug!("cast: a listener's cursor stands at the send; nothing sent");
                    if attempt.is_some() {
                        self.reports.changed();
                    }
                    return None;
                }
                // The record as the listeners left it: a listener's cancel zeroed its caster, which
                // `SendCast` finds no object for (`0x6e4a5a`, `0x6e5572` → `0x6e59b0`), and a
                // listener's attempt that refilled it and never reached its own send left no
                // targets block to send. Neither reaches the send's spell store (`0x6e585a`), so
                // the attempt's attack tail is skipped (`0x6e51be`).
                let Some(send) = self.pending.record_send().cloned() else {
                    debug!("cast: the record a listener left sends nothing");
                    if attempt.is_some() {
                        self.reports.changed();
                    }
                    return None;
                };
                if attempt.is_some() {
                    self.wand_handoff(me);
                }
                // The send stores the record's spell (`0x6e585a`), which `TryCast`'s attack tail
                // compares with its own (`0x6e51be`): a record a listener's attempt refilled
                // starts no swing of this attempt's.
                let attempt = attempt.map(|a| Attempt {
                    attack_guid: a.attack_guid.filter(|_| send.spell_id == a.spell_id),
                    ..a
                });
                let _ = self.commands.0.send(send.packet);
                let now = attempt.map_or_else(Instant::now, |a| a.now);
                if let Some(caster) = send.commit.unit_caster() {
                    self.unit_send_tail(send.spell_id, caster, now);
                    return Some(CastStage::UnitSent { attempt, caster });
                }
                // The send's precast (`0x6e5898`) and own signal (`0x6e589d`), for the active
                // player's send. The reference signals only beside the precast it starts, so not
                // for a spell the unit already precasts (`0x6e588f`, its `[+0xc8c]`); that test
                // reads the unit, which the edit applies after the pass, so benilla signals every
                // send. The visual starts at the `SMSG_SPELL_START` (#1583).
                crate::creature_anim::precast_at_send(
                    me,
                    send.spell_id,
                    &self.edits,
                    &self.precast_visuals,
                );
                self.reports.changed();
                Some(match attempt {
                    Some(attempt) => CastStage::Sent {
                        spell_id: send.spell_id,
                        attempt,
                    },
                    None => CastStage::CursorSent {
                        spell_id: send.spell_id,
                        now,
                    },
                })
            }
            // A cursor's commit: the pop, then the GCD, and no more.
            CastStage::CursorSent { spell_id, now } => {
                let repeats = self
                    .spells
                    .as_ref()
                    .and_then(|s| s.catalog.get(spell_id))
                    .is_some_and(|d| d.auto_repeat());
                if repeats {
                    self.core().pop_if(spell_id);
                }
                if let Some(d) = self.spells.as_ref().and_then(|s| s.catalog.get(spell_id)) {
                    self.cooldowns.start_gcd(spell_id, d, now, &self.spell_mods);
                }
                repeats.then_some(CastStage::Done)
            }
            CastStage::UnitSent { attempt, caster } => {
                let attempt = attempt?;
                self.owe_unit_attack(attempt, caster);
                Some(CastStage::Returned)
            }
            CastStage::Returned => {
                self.reports.changed();
                None
            }
            CastStage::Done => None,
            CastStage::Sent { spell_id, attempt } => {
                let def = self.spells.as_ref().and_then(|s| s.catalog.get(spell_id));
                // The GCD arms at the send (`0x6e2de0` from `0x6e58fb`), after the packet; a
                // failed cast result clears it (`0x6e1630`).
                if let Some(d) = def {
                    self.cooldowns
                        .start_gcd(spell_id, d, attempt.now, &self.spell_mods);
                }
                let (ranged, repeats) =
                    def.map_or((false, false), |d| (d.ranged_attack(), d.auto_repeat()));
                let Some(e) = me else {
                    return self.run_stage(CastStage::Popped { attempt });
                };
                // The ranged stance (`0x6e5930`, `SetSheatheState(2, 1, 1)`) for a ranged or
                // auto-repeat spell (`0x6e591c`, `0x6e5922`).
                if ranged {
                    self.sheath.write(crate::creature_anim::SheathRequest {
                        entity: e,
                        state: 2,
                        ceremony: false,
                    });
                }
                if !repeats {
                    return self.run_stage(CastStage::Popped { attempt });
                }
                // The sticky armed state (`0x6e593b`, `|= 0x200`) and the autorepeat key
                // (`0xceac30`, written at `0x6e594c`) the button's checked state reads, signalled
                // as `START_AUTOREPEAT_SPELL` (`0x6e5952`).
                self.edits
                    .push(crate::creature_anim::UnitEdit::ArmAutoRepeat(e));
                self.auto_repeat.0 = Some(spell_id);
                self.reports.auto_repeat(true);
                Some(CastStage::AutoShot { spell_id, attempt })
            }
            // StopAttack (`0x6e5976` → `0x5ecac0`), gated only on the caster being the active
            // player (`0x6e5957`–`0x6e5972`): starting a repeat ends the melee swing and its
            // queued strike, which the repeat's push moved into the save, so its cancel is the
            // packet alone. This is where melee and auto-repeat are kept exclusive. It runs after
            // the arm above, so it never cancels this repeat.
            CastStage::AutoShot { spell_id, attempt } => {
                let low = crate::tutorial::LowVitals::of(self.self_store());
                let mut core = CastCore {
                    pending: &mut self.pending,
                    queued_melee: &mut self.queued_melee,
                    ground: &mut self.ground,
                    reports: &mut self.reports,
                    net: &self.commands,
                    tutorials: self.tutorials.as_deref(),
                };
                match crate::creature_anim::stop_attack_head(me, low, &self.edits, &mut core) {
                    Some(stop) => Some(CastStage::AutoShotSend {
                        spell_id,
                        stop,
                        attempt,
                    }),
                    None => self.pop_repeat(spell_id, attempt),
                }
            }
            CastStage::AutoShotSend {
                spell_id,
                stop,
                attempt,
            } => {
                let cancelled = crate::creature_anim::stop_attack_send(
                    stop,
                    &mut self.stop_word,
                    &mut CastCore {
                        pending: &mut self.pending,
                        queued_melee: &mut self.queued_melee,
                        ground: &mut self.ground,
                        reports: &mut self.reports,
                        net: &self.commands,
                        tutorials: self.tutorials.as_deref(),
                    },
                );
                if cancelled {
                    return Some(CastStage::AutoShotSave { spell_id, attempt });
                }
                self.core().cancel_queued_save();
                self.pop_repeat(spell_id, attempt)
            }
            CastStage::AutoShotSave { spell_id, attempt } => {
                self.core().cancel_queued_save();
                self.pop_repeat(spell_id, attempt)
            }
            CastStage::Popped { attempt } => {
                self.attack_tail(attempt);
                // The attempt signals as it returns (`0x6e51e3`).
                self.reports.changed();
                None
            }
        }
    }

    /// The in-flight slot (`0xceca88`, `0x6e5026`): every cast holds it until its result, a ranged
    /// shot included; only an auto-repeat spell is popped after the send, last, which signals
    /// (`0x6e597b`-`0x6e5989`).
    fn pop_repeat(&mut self, spell_id: u32, attempt: Attempt) -> Option<CastStage> {
        self.core().pop_if(spell_id);
        Some(CastStage::Popped { attempt })
    }

    /// `TryCast`'s post-send tail (`6e51b5`): a spell that initiates auto-attack (on-next-swing
    /// `0x404` or `AttributesEx & 0x200`, not the GO-deferred Ex2 bit 20) starts the swing at the
    /// attempt's guid argument as the pick left it (`[ebx+0xc]`, `[ebx+0x10]` at `0x6e51d4`),
    /// whatever the spell bound (`0x6131a0` → `0x5ecb70`: sheath snap and `CMSG_ATTACKSWING`).
    fn attack_tail(&mut self, attempt: Attempt) {
        let def = self
            .spells
            .as_ref()
            .and_then(|s| s.catalog.get(attempt.spell_id));
        let (Some(guid), true) = (
            attempt.attack_guid,
            def.is_some_and(|d| d.initiates_auto_attack()),
        ) else {
            return;
        };
        let Ok((e, _)) = self.self_player.single() else {
            return;
        };
        // The tail's own gate (`6e51cb call 0x60ecb0; 6e51d2 jne`, the attack lock
        // `[player+0xc48]`, our `Engaged`) skips `0x6131a0` entirely, so a strike pressed
        // mid-swing leaves a running repeat alone. The attack path's own test at `0x5eccda`
        // would still reach the repeat cancel `0x5ecd8c`; this route never gets there.
        if self.edits.attacking(e) {
            return;
        }
        debug!(
            "ui_action: cast {} initiates auto-attack at {guid:#x}",
            attempt.spell_id
        );
        let gate = crate::creature_anim::StartGate::of(self.self_store(), attempt.control_lost);
        let mut core = CastCore {
            pending: &mut self.pending,
            queued_melee: &mut self.queued_melee,
            ground: &mut self.ground,
            reports: &mut self.reports,
            net: &self.commands,
            tutorials: self.tutorials.as_deref(),
        };
        // The guid is the selection the attempt read, so StartAttack's select (`0x5eccaa`) is
        // the setter's dedup, but for an on-self swap (`0x4b4afa`), whose select of the player is
        // not made here. The cast it follows was sent at that unit, past its own target checks.
        crate::creature_anim::start_attack_local(
            e,
            guid,
            crate::creature_anim::StartTarget::Attackable,
            gate,
            &mut self.stop_word,
            &mut self.auto_repeat,
            &mut self.sheath,
            &mut self.stand,
            &self.edits,
            self.spells.as_deref(),
            &mut core,
        );
    }

    /// The wand handoff ahead of the send: the running repeat is cancelled when it carries
    /// `AttributesEx3 & 0x400000` (wand Shoot 5019), so Auto Shot survives. `CMSG_CANCEL_CAST`
    /// naming it goes first, then the local cancel and its ack.
    fn wand_handoff(&mut self, me: Option<Entity>) {
        let Some(cached) = self.auto_repeat.0 else {
            return;
        };
        if !self
            .spells
            .as_ref()
            .and_then(|s| s.catalog.get(cached))
            .is_some_and(|d| d.casting_cancels_autorepeat())
        {
            return;
        }
        debug!("ui_action: a new cast cancels the running wand repeat {cached}");
        let _ = self
            .commands
            .0
            .send(ClientCommand::CancelCast { spell_id: cached });
        let mut core = CastCore {
            pending: &mut self.pending,
            queued_melee: &mut self.queued_melee,
            ground: &mut self.ground,
            reports: &mut self.reports,
            net: &self.commands,
            tutorials: self.tutorials.as_deref(),
        };
        crate::creature_anim::cancel_auto_repeat_local(
            me,
            &mut self.auto_repeat,
            &self.edits,
            &mut core,
        );
    }

    /// TryCast's attack tail (`0x6e51b5`-`0x6e51de`) on a unit caster, which its caller applies,
    /// at the guid argument the pick settled (`[ebx+0xc]`, `[ebx+0x10]`), bound or not.
    fn owe_unit_attack(&mut self, attempt: Attempt, caster: u64) {
        let engaged = self
            .objects
            .entity(caster)
            .is_some_and(|e| self.edits.attacking(e));
        let initiates = self
            .spells
            .as_ref()
            .and_then(|s| s.catalog.get(attempt.spell_id))
            .is_some_and(|d| d.initiates_auto_attack());
        if let (Some(guid), true, false) = (attempt.attack_guid, initiates, engaged) {
            if let Some(owed) = self.unit_attack.as_deref_mut() {
                owed.0 = Some((caster, guid));
            }
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
        let held = HeldCast {
            spell_id,
            on_self,
            unit: None,
        };
        self.send_bound(spell_id, ctx, CastCommit::Spell, None, Some(held));
    }

    /// The press held at the attack pick, if one is.
    pub(crate) fn take_held(&mut self) -> Option<HeldCast> {
        self.held_for_pick.0.take()
    }

    /// A held press after its pick, at the unit the pick left selected, or for a unit caster at
    /// the guid it settled (`0x6e4efb` writes TryCast's argument). The rungs above the pick run
    /// again; none reads the selection, so each passes as it did.
    pub(crate) fn resume_after_pick(&mut self, held: HeldCast, ctx: &cast_target::CastContext) {
        let commit = held.unit.map_or(CastCommit::Spell, |unit| CastCommit::Pet {
            caster: unit.caster,
        });
        self.send_bound(held.spell_id, ctx, commit, None, None);
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
        let mut sent = None;
        let mut core = CastCore {
            pending: &mut self.pending,
            queued_melee: &mut self.queued_melee,
            ground: &mut self.ground,
            reports: &mut self.reports,
            net: &self.commands,
            tutorials: self.tutorials.as_deref(),
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
            self.sub_classes.as_deref().map(|s| &s.0),
            self.loot_latch.as_deref().and_then(|l| l.0),
            &self.edits,
            &mut core,
            &mut self.cooldowns,
            &self.spell_mods,
            self.skill_lines.as_deref().map(|s| &s.catalog),
            &mut self.auto_repeat,
            &mut self.trade_skill_opens,
            &mut self.held_for_pick,
            UnitCaster {
                active: self.active.as_ref().and_then(|g| g.0),
                pet_bar: self.pet_bar.as_deref_mut(),
                engaged: commit
                    .unit_caster()
                    .and_then(|guid| self.objects.entity(guid))
                    // `[unit+0xc48]`, which `0x60ecb0` tests: the pick reads it.
                    .is_some_and(|e| self.edits.attacking(e)),
            },
            &self.known,
            &mut sent,
        );
        if let Some(stage) = sent {
            self.proceed(stage);
        }
    }
}

/// The `DisplayError` line of a validator rung ([`validator::RefusalLine`]): a spell's name at the
/// client's locale (`[rec + locale·4 + 0x1e0]`, `0x609876`, `0x609920`), `"UNKNOWN"`
/// (`0x838044`) for a taught spell with no record.
fn refusal_line(
    line: validator::RefusalLine,
    spells: Option<&Spells>,
) -> crate::ui_action::UiError {
    use crate::ui_action::{FillArg, UiError};
    let name = |id: u32| {
        spells
            .and_then(|s| s.catalog.get(id))
            .map_or_else(|| "UNKNOWN".to_string(), |d| d.name.clone())
    };
    match line {
        validator::RefusalLine::AlreadyKnown(taught) => UiError {
            key: "ERR_SPELL_ALREADY_KNOWN_S",
            args: vec![FillArg::S(name(taught))],
        },
        validator::RefusalLine::PetAlreadyKnown(cast) => UiError {
            key: "ERR_PET_SPELL_ALREADY_KNOWN_S",
            args: vec![FillArg::S(name(cast))],
        },
        // `0x6e6a20(2)` resolves `PETTAME_TOOMANY` and passes it as `ERR_TAME_FAILED`'s `%s`.
        validator::RefusalLine::TooManyPets => UiError {
            key: "ERR_TAME_FAILED",
            args: vec![FillArg::Global("PETTAME_TOOMANY".to_string())],
        },
    }
}

/// "You are dead", TryCast's dead rung's refusal (`0x6e4c66`).
const REASON_DEAD: u8 = 0x13;

/// TryCast's refusal while a loot window is latched, by `Effect[0]` (vmangos `SpellDefines.h`):
/// `SPELL_EFFECT_DISENCHANT` 99 is `0x82` `DISENCHANT_WHILE_LOOTING` (`0x6e4ca2`), effect 127
/// `0x83` `PROSPECT_WHILE_LOOTING` (`0x6e4cd4`), and `ENCHANT_ITEM` 53, `_TEMPORARY` 54 and
/// `ENCHANT_HELD_ITEM` 92 `0x61` `SPELL_IN_PROGRESS` (`0x6e4d0d`-`0x6e4d17`).
fn looting_refusal(effect0: u32) -> Option<u8> {
    match effect0 {
        99 => Some(0x82),
        127 => Some(0x83),
        53 | 54 | 92 => Some(0x61),
        _ => None,
    }
}

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
        CastCommit::Spell | CastCommit::Pet { .. } => None,
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
        Held::Cast {
            item_flags,
            item: commit.item_guid(),
        }
    })
}

/// Run the ladder for one cast up to `SendCast`'s entry (`0x6e54f0`), where it leaves the send in
/// `sent` ([`CastStage::Send`]). The commit tail follows the packet: the GCD, then a ranged spell
/// arms the ranged stance (`0x6e5930`, `SetSheatheState(2,1,1)`; the echoed START re-requests it),
/// and an auto-repeat spell sets the sticky armed state (`0x6e593b`, `|= 0x200`, the Load/Hold
/// idle's gate), stops the attack and leaves the slot.
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
    self_player: &Query<(Entity, Has<SelfPlayer>), With<SelfPlayer>>,
    spells: Option<&Spells>,
    objects: &Objects,
    items: &Items,
    sub_classes: Option<&benilla_formats::ItemSubClassCatalog>,
    loot_guid: Option<u64>,
    edits: &crate::creature_anim::UnitEdits,
    core: &mut CastCore,
    cooldowns: &mut crate::spell::Cooldowns,
    spell_mods: &crate::spell::SpellModifiers,
    skill_lines: Option<&benilla_formats::SkillLineCatalog>,
    auto_repeat: &mut AutoRepeatActive,
    trade_skill_opens: &mut crate::ui_tradeskill::TradeSkillOpens,
    held_for_pick: &mut HeldForPick,
    unit: UnitCaster<'_>,
    known: &KnownReads,
    // Where the attempt leaves its send when it reaches `SendCast`'s entry.
    sent: &mut Option<CastStage>,
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
    if caster_dead(spell_id, def, ctx.rel.self_store) {
        refuse!(CastFail::local(spell_id, REASON_DEAD));
    }
    let player_caster = commit.unit_caster().is_none();
    // The loot rungs, after the Attack short-circuit (`0x6e4ca2`-`0x6e4d31`): with a loot window
    // latched, the first effect refuses. They read the caster's loot guid through `0x6126f0`,
    // which is 0 for a caster that is not the active player (`0x5fa6d0`).
    if player_caster && loot_guid.is_some_and(|g| g != 0) {
        if let Some(reason) = def.and_then(|d| looting_refusal(d.effects[0])) {
            debug!("ui_action: cast {spell_id} refused locally — looting ({reason:#x})");
            refuse!(CastFail::local(spell_id, reason));
        }
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
    // a pickless cast with `ITEM_GONE`. It passes a caster that is not the active player
    // (`0x6e400e`), as the equipped-item check below does (`0x6e40e0`).
    if let Some(fail) =
        reagent_totem_refusal(spell_id, def, ctx.rel.self_store, objects).filter(|_| player_caster)
    {
        refuse!(fail);
    }
    // The equipped-item requirement (`0x6e4e03`), the same search the button greying and tooltip
    // use. It sits above the crowd-control rung, so a stunned caster without the weapon is told
    // about the weapon.
    if let Some(d) = def.filter(|_| player_caster) {
        if let Some(store) = ctx.rel.self_store {
            if !super::usable::equipped_item_fits(d, store, objects, items) {
                let reason = super::usable::equipped_item_reason(d);
                debug!("ui_action: cast {spell_id} refused locally — equipped item ({reason:#x})");
                refuse!(CastFail::local(spell_id, reason));
            }
            // Its ammo leg, the request passing the ammo flag (`0x6e4dfd`).
            let flags = |class, subclass| sub_classes.map_or(0, |c| c.flags(class, subclass));
            let held = |entry| {
                crate::ui_items::count_of(
                    &store.0,
                    objects,
                    entry,
                    crate::ui_items::InventoryScope::DEFAULT,
                )
            };
            if let Some(ammo_type) =
                super::usable::ammo_refusal(d, store, objects, items, &flags, &held)
            {
                debug!("ui_action: cast {spell_id} refused locally — no ammo of type {ammo_type}");
                refuse!(CastFail::local_arg(spell_id, 0x2f, ammo_type));
            }
        }
    }
    // The ranged rung (`0x6e4e13`-`0x6e4e3e`): `Attributes & 0x2` (`SPELL_ATTR_RANGED`) with no
    // ranged weapon, the caster's `GetVirtualItem` (vtable slot 38, hand 2, `raw` 0) answering
    // null, refuses `0x18` `SPELL_FAILED_EQUIPPED_ITEM`. A player's, possessed or not
    // (`0x5ec240`), finds no visible entry in its slot 17 (`0x5ec300`) or one whose flag half has
    // bit 0 (`0x5ec2ee`); a creature's (`0x605e30`) is null for hand 2 only with no display id
    // (`0x605f10`).
    if let (Some(d), Some(store)) = (def, ctx.rel.self_store) {
        let player_object = player_caster
            || store.0.object_type() == Some(benilla_protocol::messages::ObjectType::Player);
        let ranged_weapon = if player_object {
            store
                .0
                .player_visible_item_entry(crate::items::EQUIPMENT_SLOT_RANGED)
                .is_some()
                && store
                    .0
                    .player_visible_item_flags(crate::items::EQUIPMENT_SLOT_RANGED)
                    & 0x1
                    == 0
        } else {
            store.0.unit_virtual_item_display(2).unwrap_or(0) != 0
        };
        if d.ranged_slot() && !ranged_weapon {
            debug!("ui_action: cast {spell_id} refused locally — no ranged weapon (0x18)");
            refuse!(CastFail::local(spell_id, 0x18));
        }
    }
    // The finisher rung (`6e4e50`–`6e4ecd`), above the attack pick: a finishing move
    // (`AttributesEx & 0x500000`) with the combo-point byte at 0 refuses 0x46 "That ability
    // requires combo points" for a rogue (class byte 4, `6e4e98`), else 0x12. Like the button
    // greying, it reads no target. It runs for the active player alone (`0x6e4e5b`).
    if let (Some(d), Some(store), true) = (def, ctx.rel.self_store, player_caster) {
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
        // `0x60ecb0` on the caster (`0x6e4ee8`).
        let engaged = match commit.unit_caster() {
            Some(_) => unit.engaged,
            None => self_player
                .single()
                .is_ok_and(|(me, _)| edits.attacking(me)),
        };
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
    let mut parked = None;
    let mut deferred_refusal = None;
    let explicit_object = match commit {
        CastCommit::Item { on_object, .. } => on_object,
        CastCommit::Spell | CastCommit::Pet { .. } => bound_object,
    };
    // The autoSelfCast fallback binds the active player whoever casts (`0x6e53e3`).
    let (auto_self, auto_self_store) = if player_caster {
        (ctx.self_guid, ctx.rel.self_store)
    } else {
        (
            unit.active,
            unit.active.and_then(|guid| objects.store(guid)),
        )
    };
    let candidates = cast_target::CastCandidates {
        selection: ctx.selection_guid,
        selection_held: ctx.selection_held,
        caster: ctx.self_guid,
        auto_self,
        auto_self_store,
        // The reference resolves the candidate through `0x468460(typemask 1)` (`6e53bc`): a guid
        // naming no live object binds nothing and falls to the cursor.
        main_hand_item: ctx
            .main_hand_item
            .filter(|guid| objects.object(*guid).is_some()),
        // `GetMinMaxRange(caster, spell, &min, &max, 0)` (`0x6e543f`): no unit target, and no
        // auto-attack target to hand, as the range gate passes none.
        long_range: def.is_some_and(|d| {
            let row = spells.and_then(|s| s.ranges.get(d.range_index));
            let targets = benilla_formats::RangeTargets {
                target: None,
                attack_target: None,
            };
            spell_mods
                .min_max_range(d, row, ctx.range.caster, targets)
                .is_some_and(|(_, max)| max > cast_target::LONG_RANGE)
        }),
    };
    let target = match explicit_object {
        Some(_) => None,
        None => {
            match cast_target::resolve_cast_target(def, &candidates, ctx.auto_self_cast, &ctx.rel) {
                cast_target::CastWireTarget::SelfImplicit => None,
                cast_target::CastWireTarget::Parked(guid) => {
                    parked = Some(guid);
                    None
                }
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
        CastCommit::Spell | CastCommit::Pet { .. } => 0,
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
                        edits,
                        core,
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
    // The player's level for a spell reads its skill lines, and for a weapon spell the hands.
    let skills = super::PlayerSkills {
        lines: skill_lines,
        spells: spells.map(|s| &s.catalog),
        items: Some(items),
    };
    if !commit.is_item() {
        if let (Some(d), Some(store)) = (def, ctx.rel.self_store) {
            if !super::usable::can_afford(d, store, skills, spell_mods) {
                debug!("ui_action: cast {spell_id} refused locally — not enough power (0x4d)");
                refuse!(CastFail::local(spell_id, 0x4d));
            }
        }
    }
    // The rungs after power (`0x6096b3`-`0x60a02d`), in the validator's order
    // ([`validator::caster_refusal`]), each refused before any packet as the reference does:
    // vmangos accepts some (it dismounts, grants Aquatic Form on land, interrupts a moving cast
    // mid-bar) and answers the rest a round trip later.
    let no_fields = benilla_protocol::ObjectFields::default();
    let self_fields = ctx.rel.self_store.map_or(&no_fields, |s| &s.0);
    let caster = validator::CasterState {
        fields: self_fields,
        // The charm arm compares the charmer with the active player (`0x60994d` asks `0x468550`),
        // so a unit the player possesses casts.
        guid: unit.active.filter(|_| !player_caster).or(ctx.self_guid),
        active: player_caster,
        stand_state: ctx.self_stand_state,
        sheathe_state: ctx.self_sheathe_state,
        move_flags: ctx.self_move_flags,
        daytime: ctx.daytime,
        outdoors: ctx.self_outdoors,
    };
    let cast_time_ms = def.map_or(0, |d| {
        let level = crate::spell::SpellLevel::Player {
            me: ctx.rel.self_store,
            skills,
        };
        let player = ctx.rel.self_store.map(|s| &s.0);
        spells.map_or(0, |s| s.cast_time_ms(d, player, level, spell_mods))
    });
    let form_is_stance = spells
        .and_then(|s| s.forms.get(&u32::from(self_fields.unit_shapeshift_form())))
        .is_some_and(|f| f.is_stance());
    // `GetFactionStanding 0x4d6370`: the active player's base for its race and class plus the
    // wire, read from its table at `0xb73298` whoever casts (`0x609f29` passes no caster).
    let player_fields = if player_caster {
        self_fields
    } else {
        unit.active
            .and_then(|g| objects.object(g))
            .unwrap_or(&no_fields)
    };
    let standing = |faction: u32| {
        ctx.rel.factions.map_or(0, |f| {
            crate::ui_reputation::faction_standing(
                f.catalog(),
                ctx.rel.reputations,
                player_fields.unit_race().unwrap_or(0),
                player_fields.unit_class().unwrap_or(0),
                faction,
            )
        })
    };
    // `KnowsSpell 0x60c740` on the caster: the active player's book, else the pet bar's unit's.
    let pet_view = spells.zip(skill_lines).and_then(|(s, lines)| {
        super::known::PetView::of(
            unit.pet_bar.as_deref(),
            known.names.as_deref(),
            known.families.as_deref(),
            lines,
            &s.catalog,
        )
    });
    let knows = |spell: u32| match commit.unit_caster() {
        None => known
            .actions
            .as_deref()
            .is_some_and(|a| a.spells.contains(&spell)),
        Some(guid) => pet_view.as_ref().is_some_and(|pet| {
            pet.unit(objects, guid)
                .is_some_and(|unit| pet.knows(&unit, spell))
        }),
    };
    // The pet learn rung asks the pet unit (`0x609792`) what it may learn and what it knows.
    let pet_learn = |guid: u64, taught: u32| {
        let pet = pet_view.as_ref()?;
        let unit = pet.unit(objects, guid)?;
        Some(validator::PetLearn {
            learnable: pet.required_level(&unit, taught).is_some(),
            known: pet.knows(&unit, taught) || pet.knows_higher_rank(&unit, taught),
        })
    };
    let inputs = validator::SpellInputs {
        cast_time_ms,
        form_is_stance,
        standing: &standing,
        knows: &knows,
        pet_learn: &pet_learn,
    };
    // The crowd-control scan `0x6e9ca0` for one aura type, over the caster's raw aura slot ids,
    // unfiltered, as the reference reads them.
    let catalog = spells.map(|s| &s.catalog);
    let mut scan = |aura_type: u32| {
        def.map_or_else(benilla_formats::CcExemption::default, |d| {
            benilla_formats::cc_exemption(d, self_fields.unit_aura_ids(), aura_type, |id| {
                catalog.and_then(|c| c.get(id))
            })
        })
    };
    if let Some(refusal) = validator::caster_refusal(def, &caster, &inputs, &mut scan) {
        debug!("ui_action: cast {spell_id} refused locally — the validator ({refusal:?})");
        if let Some(line) = refusal.line {
            core.reports.line(refusal_line(line, spells));
        }
        match refusal.report {
            Some(r) => refuse!(match r.arg {
                Some(arg) => CastFail::local_arg(spell_id, r.reason, arg),
                None => CastFail::local(spell_id, r.reason),
            }),
            // A learn rung reports nothing: the attempt's `xor al,al` exit alone (`0x60988e`).
            None => return,
        }
    }
    // Every rung passed: the attempt fills the pending record with the spell (`0x6e4f88`), its
    // source (`0x6e4fa1`) and its caster (`0x6e4fb1`), ahead of the arm and whatever it decides.
    if player_caster {
        core.pending.fill_record();
    } else {
        core.pending.fill_unit_record();
    }
    core.pending.fill_record_spell(spell_id);
    core.pending.fill_record_item(commit.item_guid());
    // The arm's bind writes the record's target (`0x6e5b40`, `0x6e5454`), which a unit's death
    // reads (`0x6058c7`).
    if let Some(guid) = target.or(explicit_object).or(parked) {
        core.pending.bind_target(guid);
    }
    // The push (`0x6e5026` → `0x6e4ad0`), which signals: the same spell bailed above, so the slot
    // never holds it here. The arm signals whatever it then decides (`0x6e5333`).
    let held = held(items, commit, def, pending_word.is_some());
    if let Some(held) = held {
        core.push(spell_id, held);
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
    // (`0x6e60cb`) and tested the word before it (`0x6e60c1`); a word of 0 sends with no bind
    // (`0x6e5349`), read again past the arm's signal (`0x6e5338`).
    let bound = explicit_object.is_some() || item_target.is_some() || target.is_some();
    if bound {
        core.reports.changed();
    }
    // `SendCast 0x6e54f0`: one targets block, two opcodes, sent first (`0x6e5872`); everything the
    // send does after it follows the packet ([`CastStage`]).
    let packet = match commit {
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
        CastCommit::Pet { caster } => ClientCommand::PetCastSpell {
            caster_guid: caster,
            spell_id,
            target: match (explicit_object, item_target, target) {
                (Some(go), _, _) => UseItemTarget::Object(go),
                (None, Some(item), _) => UseItemTarget::Item(item),
                (None, None, Some(unit)) => UseItemTarget::Unit(unit),
                (None, None, None) => UseItemTarget::SelfImplicit,
            },
        },
    };
    core.pending.set_record_send(CastSend {
        spell_id,
        commit,
        packet,
    });
    *sent = Some(CastStage::Send {
        rereads_word: !bound,
        attempt: Some(Attempt {
            spell_id,
            attack_guid: ctx.selection_guid,
            now,
            control_lost: ctx.self_control_lost,
        }),
    });
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
        guid: None,
        spell_index: 0,
        on_object: None,
    };

    /// A context with nothing selected: every rung that needs world state is inert.
    pub(super) fn ctx() -> cast_target::CastContext<'static> {
        cast_target::CastContext {
            selection_guid: None,
            selection_held: true,
            self_guid: Some(0x0000_0000_0000_0007),
            auto_self_cast: false,
            rel: cast_target::TargetRelations {
                target_store: None,
                target_owner_store: None,
                self_store: None,
                factions: None,
                reputations: Reputations::empty(),
                types: Default::default(),
                group: Default::default(),
            },
            range: cast_target::RangeInputs::default(),
            main_hand_item: None,
            self_move_flags: 0,
            self_stand_state: 0,
            self_sheathe_state: 0,
            daytime: false,
            self_outdoors: true,
            self_control_lost: false,
        }
    }

    /// A World with exactly the resources [`CastLadder`] gathers and no `Spells`, so every rung
    /// that needs the spell's row is inert.
    pub(super) fn world() -> (World, Receiver<ClientCommand>) {
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
        world.init_resource::<crate::creature_anim::AttackStopInFlight>();
        world.init_resource::<crate::creature_anim::UnitEdits>();
        world.init_resource::<crate::ui_tradeskill::TradeSkillOpens>();
        world.init_resource::<crate::spell::SpellTargeting>();
        world.init_resource::<crate::spell::HeldForPick>();
        // A bare World has no message storage until it is asked for.
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<Messages<crate::player::StandStateRequest>>();
        (world, rx)
    }

    pub(super) fn send(world: &mut World, spell_id: u32, commit: CastCommit) {
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

    /// The attempt records its spell (`0x6e4f88`); the send, the attempt's or a bind's, starts
    /// the player's precast of a spell with a visual (`0x6e5898`).
    #[test]
    fn the_attempt_records_its_spell_and_the_send_starts_its_precast() {
        const FIREBALL: u32 = 133;
        const BLIZZARD: u32 = 10;
        let (mut world, _rx) = world();
        crate::creature_anim::install_visuals_for_tests(&mut world, &[FIREBALL, BLIZZARD], &[]);
        let me = world.spawn(crate::net::SelfPlayer).id();
        let precast = |world: &World| {
            world
                .get::<crate::creature_anim::Precast>(me)
                .map(|p| p.spell_id)
        };
        send(&mut world, FIREBALL, CastCommit::Spell);
        assert_eq!(
            world.resource::<crate::spell::PendingCast>().record_spell(),
            FIREBALL
        );
        crate::creature_anim::apply_unit_edits(&mut world);
        assert_eq!(precast(&world), Some(FIREBALL), "the attempt's send");
        world
            .run_system_once(|mut ladder: CastLadder| {
                ladder.commit_targeted(BLIZZARD, CastCommit::Spell, TargetedBind::Dest([0.0; 3]));
            })
            .expect("the ladder runs as a one-shot system");
        crate::creature_anim::apply_unit_edits(&mut world);
        assert_eq!(precast(&world), Some(BLIZZARD), "a bind's send");
    }

    /// TryCast's tail starts the attack after the send's precast (`0x6e5898`, then `0x6e51b5`),
    /// so StartAttack reads the pressed spell: Slam, which initiates combat, has a visual, and
    /// `InterruptFlags & 0x8`, returns before the swing (`0x5eccd4`).
    #[test]
    fn a_combat_spell_auto_attack_interrupts_starts_no_swing_from_its_own_send() {
        use benilla_formats::{SpellCatalog, SpellDisplay, SpellVisualCatalog, VisualStages};
        use std::collections::HashMap;
        const SLAM: u32 = 1464;
        const STRIKE: u32 = 1752;
        const MOB: u64 = 0xF130_0000_0000_0042;
        const VISUAL: u32 = 1165;
        let swings = |spell_id: u32| {
            let (mut world, rx) = world();
            let combat = |interrupt_flags| SpellDisplay {
                targets: 0x2,
                attributes_ex: 0x200,
                interrupt_flags,
                visual: VISUAL,
                ..Default::default()
            };
            world.insert_resource(Spells {
                catalog: SpellCatalog::from_displays(HashMap::from([
                    (SLAM, combat(0xf)),
                    (STRIKE, combat(0)),
                ])),
                ..Spells::empty_for_tests()
            });
            world.insert_resource(crate::creature_anim::SpellVisuals(
                SpellVisualCatalog::from_tables(
                    HashMap::from([(VISUAL, VisualStages::default())]),
                    HashMap::new(),
                ),
            ));
            world.spawn(SelfPlayer);
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    let ctx = cast_target::CastContext {
                        selection_guid: Some(MOB),
                        ..ctx()
                    };
                    ladder.send(spell_id, &ctx, CastCommit::Spell);
                })
                .expect("the ladder runs as a one-shot system");
            let sent: Vec<_> = rx.try_iter().collect();
            assert!(
                matches!(sent.first(), Some(ClientCommand::CastSpell { spell_id: s, .. }) if *s == spell_id),
                "{spell_id} is sent: {sent:?}"
            );
            sent.iter()
                .filter(|c| matches!(c, ClientCommand::AttackSwing { guid: MOB }))
                .count()
        };
        assert_eq!(swings(SLAM), 0, "Slam's own precast");
        assert_eq!(swings(STRIKE), 1, "a strike auto-attack does not interrupt");
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
        let reports = &world.resource::<CastReports>().reports();
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
                on_self: false,
                unit: None,
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

        // `UNIT_FIELD_BYTES_0` (field 36) byte 1 is the class, `UNIT_FIELD_HEALTH` (22) alive;
        // `PLAYER_FIELD_BYTES` holds no combo points.
        for (class, reason) in [(4u32, 0x46), (11, 0x12)] {
            let (mut world, rx) = combat_world(false);
            let held = world
                .run_system_once(move |mut ladder: CastLadder| {
                    let me = crate::net::ObjectStore(ObjectFields::from_pairs(&[
                        (22, 1),
                        (36, class << 8),
                    ]));
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

    /// The send reports `START_AUTOREPEAT_SPELL` as it stores the key (`0x6e594c`, `0x6e5952`):
    /// after the attempt's push, arm and bind signals and the send's own (`0x6e589d`), ahead of
    /// its StopAttack (`0x6e5976`), whose save leg signals nothing, the pop's signal (`0x6e5989`)
    /// and the attempt's last (`0x6e51e3`).
    #[test]
    fn an_auto_repeat_send_reports_its_start_where_it_stores_the_key() {
        let (mut world, _rx) = combat_world(true);
        world
            .resource_mut::<crate::spell::QueuedMeleeSpell>()
            .arm(RAPTOR_STRIKE);
        send_at(&mut world, AUTO_SHOT, MOB);
        assert_eq!(
            stream(&mut world),
            [
                "CHANGED",
                "CHANGED",
                "CHANGED",
                "CHANGED",
                "START_AUTOREPEAT",
                "CHANGED",
                "CHANGED"
            ]
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

    /// TryCast's tail starts the attack at its guid argument as the pick left it (`[ebx+0xc]`,
    /// `[ebx+0x10]` at `0x6e51d4`-`0x6e51de`), whatever the spell bound: an area melee spell
    /// (`AttributesEx 0x200`, no unit word) binds nothing and still swings at the selection.
    #[test]
    fn an_attack_spell_that_binds_no_unit_still_swings_at_the_pick() {
        const CROWD_PUMMEL: u32 = 10887;
        let (mut world, rx) = world();
        world.insert_resource(Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(
                    CROWD_PUMMEL,
                    benilla_formats::SpellDisplay {
                        attributes_ex: 0x200,
                        ..Default::default()
                    },
                )]
                .into(),
            ),
            ..Spells::empty_for_tests()
        });
        world.spawn(SelfPlayer);

        send_at(&mut world, CROWD_PUMMEL, MOB);

        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            matches!(
                sent[..],
                [
                    ClientCommand::CastSpell {
                        spell_id: CROWD_PUMMEL,
                        target: None
                    },
                    ClientCommand::AttackSwing { guid: MOB }
                ]
            ),
            "{sent:?}"
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
            .take()
            .into_iter()
            .map(|r| match r {
                crate::ui_cast::CastReport::Changed => "CHANGED",
                crate::ui_cast::CastReport::Bar(crate::ui_cast::CastBarEdge::Failed) => "FAILED",
                crate::ui_cast::CastReport::Bar(crate::ui_cast::CastBarEdge::Stop) => "STOP",
                crate::ui_cast::CastReport::Bar(_) => "BAR",
                crate::ui_cast::CastReport::Refused(_) => "REFUSED",
                crate::ui_cast::CastReport::TradeSkillDisarm => "DISARM",
                crate::ui_cast::CastReport::AutoRepeat(true) => "START_AUTOREPEAT",
                crate::ui_cast::CastReport::AutoRepeat(false) => "STOP_AUTOREPEAT",
                crate::ui_cast::CastReport::Line(e) => e.key,
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
            guid: None,
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
    /// running outside it (an item's without `ITEM_FLAG_PLAYERCAST`) is no same-spell bail for a
    /// press of its spell (`0x6e4d43`), and that press's refusal finds
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
            guid: None,
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

    /// The attempt writes the record's source (`0x6e4f8f`-`0x6e4fa1`) whether or not the slot
    /// takes the spell: the used item's guid, which the item's deactivate cancels the record's cast
    /// off (`0x5d86c4`); a spell's attempt writes its caster's, which names no item.
    #[test]
    fn the_attempt_records_the_used_item_as_its_source() {
        const MINOR_HEALING_POTION: u32 = 118;
        const POTION_SPELL: u32 = 439;
        const POTION: u64 = 0x4000_0000_0000_0042;
        let potion = CastCommit::Item {
            bag_index: 255,
            slot: 25,
            entry: MINOR_HEALING_POTION,
            guid: Some(POTION),
            spell_index: 0,
            on_object: None,
        };
        let (mut world, rx) = world();
        send(&mut world, POTION_SPELL, potion);
        assert!(matches!(rx.try_recv(), Ok(ClientCommand::UseItem { .. })));
        let record = |world: &World| world.resource::<crate::spell::PendingCast>().record_item();
        assert_eq!(record(&world), Some(POTION), "the potion's attempt");
        send(&mut world, MOUNT, CastCommit::Spell);
        assert_eq!(record(&world), None, "a spell's attempt");
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
        world.resource_mut::<CastReports>().clear();
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
                guid: None,
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

        world.resource_mut::<CastReports>().clear();
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

    /// The no-target refusal is the attempt's tail after the arm (`0x6e5064`), and the validator
    /// runs before the arm (`0x6e4f3b`): a Fireball pressed with nothing selected says the
    /// validator's line when a rung of it fails, seated `0x3e`, on cooldown `0x3c`.
    #[test]
    fn the_validator_speaks_before_the_arms_no_target_refusal() {
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

        world
            .run_system_once(move |mut ladder: CastLadder| {
                let seated = cast_target::CastContext {
                    self_stand_state: 1,
                    ..ctx()
                };
                ladder.send(FIREBALL, &seated, CastCommit::Spell);
            })
            .expect("one-shot");
        assert!(rx.try_recv().is_err(), "refused before the push: no cancel");
        assert_eq!(
            world.resource_mut::<CastReports>().take_refusals(),
            vec![CastFail::local(FIREBALL, 0x3e)]
        );

        let use_spell = benilla_protocol::messages::ItemUseSpell {
            spell_id: FIREBALL,
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
        send(&mut world, FIREBALL, CastCommit::Spell);
        assert!(rx.try_recv().is_err());
        assert_eq!(
            world.resource_mut::<CastReports>().take_refusals(),
            vec![CastFail::local(FIREBALL, 0x3c)],
            "\"Spell is not ready yet\", not \"You have no target\""
        );
    }

    /// A learn rung shows its line and reports nothing (`0x609854`-`0x60988e`): a press of a spell
    /// teaching the player Fireball, which it knows, queues `ERR_SPELL_ALREADY_KNOWN_S` filled
    /// with "Fireball", no refusal and no packet. The tame rung queues its line, then its 0x17
    /// report (`0x60a01e`, `0x60a02d`).
    #[test]
    fn a_learn_rung_shows_its_line_and_the_tame_rung_reports_after_its_line() {
        use benilla_formats::SpellDisplay;
        const FIREBALL: u32 = 133;
        const TEACH: u32 = 900_300;
        const TAME: u32 = 1515;
        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (
                    FIREBALL,
                    SpellDisplay {
                        id: FIREBALL,
                        name: "Fireball".into(),
                        ..Default::default()
                    },
                ),
                (
                    TEACH,
                    SpellDisplay {
                        id: TEACH,
                        effects: [0x24, 0, 0],
                        effect_trigger_spell: [FIREBALL, 0, 0],
                        ..Default::default()
                    },
                ),
                (
                    TAME,
                    SpellDisplay {
                        id: TAME,
                        attributes_ex2: 0x1_0000,
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        world.insert_resource(spells);
        world.insert_resource(crate::ui_action::PlayerActions {
            spells: [FIREBALL].into(),
            ..Default::default()
        });
        // A player (type 0x19) at full health, controlling a pet (`PLAYER_FIELD_BYTES` bit 0).
        let me = crate::net::ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (2, 0x19),
            (22, 100),
            (1222, 0x1),
        ]));
        let stream_of = |world: &mut World, spell: u32| {
            let me = me.clone();
            world
                .run_system_once(move |mut ladder: CastLadder| {
                    let mut c = ctx();
                    c.rel.self_store = Some(&me);
                    ladder.send(spell, &c, CastCommit::Spell);
                })
                .expect("one-shot");
            stream(world)
        };
        assert_eq!(stream_of(&mut world, TEACH), ["ERR_SPELL_ALREADY_KNOWN_S"]);
        assert!(rx.try_recv().is_err(), "nothing sent");
        assert_eq!(stream_of(&mut world, TAME), ["ERR_TAME_FAILED", "REFUSED"]);
        assert!(rx.try_recv().is_err(), "nothing sent");
        let line = refusal_line(
            validator::RefusalLine::AlreadyKnown(FIREBALL),
            world.get_resource::<crate::ui_action::Spells>(),
        );
        assert_eq!(line.args, [crate::ui_action::FillArg::S("Fireball".into())]);
        let unknown = refusal_line(validator::RefusalLine::AlreadyKnown(1), None);
        assert_eq!(
            unknown.args,
            [crate::ui_action::FillArg::S("UNKNOWN".into())]
        );
    }

    /// The equipped-item check's ammo leg (`0x6e40e0` with the ammo flag, `0x6e4dfd`): a shot with
    /// a bow worn and no arrows loaded is refused `0x2f` with the ammo type (`0x6e4373`), ahead of
    /// the arm's no-target refusal, which it reaches once arrows are loaded and carried.
    #[test]
    fn a_shot_without_ammo_is_refused_before_the_arm() {
        use crate::net::ObjectStore;
        use benilla_formats::SpellDisplay;
        use benilla_protocol::ObjectFields;
        const SHOT: u32 = 3044;
        const BOW: u64 = 0x2a;
        const ARROWS: u64 = 0x2b;
        const BOW_ENTRY: u32 = 2504;
        const ARROW_ENTRY: u32 = 2512;
        // `PLAYER_FIELD_INV_SLOT_HEAD` + 2 × slot: the ranged slot 17 and backpack slot 23.
        const INV_RANGED: u16 = 486 + 2 * 17;
        const INV_BACKPACK: u16 = 486 + 2 * 23;
        const AMMO_ID: u16 = 1223;

        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [(
                SHOT,
                SpellDisplay {
                    implicit_target_a1: 6,
                    equipped_item_class: 2,
                    equipped_item_subclass_mask: 1 << 2,
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        );
        world.insert_resource(spells);
        crate::items::test_spawn_item(
            &mut world,
            BOW,
            ObjectFields::from_pairs(&[(3, BOW_ENTRY)]),
            false,
        );
        crate::items::test_spawn_item(
            &mut world,
            ARROWS,
            ObjectFields::from_pairs(&[(3, ARROW_ENTRY)]),
            false,
        );
        {
            let mut items = world.resource_mut::<Items>();
            items.insert_template(
                BOW_ENTRY,
                Some(benilla_protocol::messages::ItemInfo {
                    class: 2,
                    subclass: 2,
                    ammo_type: 2,
                    ..crate::items::test_template("Bow")
                }),
            );
            items.insert_template(
                ARROW_ENTRY,
                Some(benilla_protocol::messages::ItemInfo {
                    class: 6,
                    subclass: 2,
                    ..crate::items::test_template("Arrow")
                }),
            );
        }
        let press = |world: &mut World, ammo_id: u32| {
            let store: &'static ObjectStore =
                Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(&[
                    (22, 100),
                    (INV_RANGED, BOW as u32),
                    (INV_BACKPACK, ARROWS as u32),
                    (AMMO_ID, ammo_id),
                ]))));
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
                    ladder.send(SHOT, &with_store, CastCommit::Spell);
                })
                .expect("one-shot");
        };

        press(&mut world, 0);
        assert!(rx.try_recv().is_err(), "an empty quiver never sends");
        assert_eq!(
            world.resource_mut::<CastReports>().take_refusals(),
            vec![CastFail::local_arg(SHOT, 0x2f, 2)]
        );
        press(&mut world, ARROW_ENTRY);
        assert_eq!(
            world.resource_mut::<CastReports>().take_refusals(),
            vec![CastFail::local(SHOT, cast_target::ERR_NO_TARGET)],
            "loaded and carried: the arm's line"
        );
        let _ = rx.try_iter().count();
    }

    /// With a loot window latched (`GetLootGuid 0x6126f0`, `[player+0x1d28]`) TryCast refuses by
    /// the spell's first effect, right after its dead rung (`0x6e4ca2`-`0x6e4d31`): Disenchant
    /// (99) `0x82`, effect 127 `0x83`, and the enchants (53, 54, 92) `0x61`. With nothing latched
    /// the cast goes on.
    #[test]
    fn a_latched_loot_window_refuses_disenchants_and_enchants() {
        use benilla_formats::SpellDisplay;
        const DISENCHANT: u32 = 13262;
        let rows: [(u32, u32, u8); 5] = [
            (DISENCHANT, 99, 0x82),
            (901, 127, 0x83),
            (902, 53, 0x61),
            (903, 54, 0x61),
            (904, 92, 0x61),
        ];
        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            rows.iter()
                .map(|&(id, effect, _)| {
                    let d = SpellDisplay {
                        effects: [effect, 0, 0],
                        ..Default::default()
                    };
                    (id, d)
                })
                .collect(),
        );
        world.insert_resource(spells);
        world.insert_resource(crate::ui_loot::LootLatch(Some(0xF110_0000_0000_0042)));
        for (id, effect, reason) in rows {
            send(&mut world, id, CastCommit::Spell);
            assert!(rx.try_recv().is_err(), "effect {effect} never sends");
            assert_eq!(
                world.resource_mut::<CastReports>().take_refusals(),
                vec![CastFail::local(id, reason)],
                "effect {effect}"
            );
        }
        world.insert_resource(crate::ui_loot::LootLatch(None));
        send(&mut world, DISENCHANT, CastCommit::Spell);
        assert!(world
            .resource_mut::<CastReports>()
            .take_refusals()
            .is_empty());
        assert!(
            matches!(rx.try_recv(), Ok(ClientCommand::CastSpell { spell_id, .. }) if spell_id == DISENCHANT),
            "nothing latched: the rung passes"
        );
    }

    /// TryCast's ranged rung (`0x6e4e13`-`0x6e4e3e`): a spell with `Attributes & 0x2` refuses
    /// `0x18` when `GetVirtualItem 0x5ec240` finds no ranged weapon, no visible entry in slot 17
    /// or one whose flag half has bit 0 (`0x5ec2ee`).
    #[test]
    fn a_ranged_spell_without_a_ranged_weapon_is_refused() {
        use crate::net::ObjectStore;
        use benilla_formats::SpellDisplay;
        use benilla_protocol::ObjectFields;
        const RANGED: u32 = 905;
        // `PLAYER_VISIBLE_ITEM_18`: the entry at block +2, the properties dword at +10.
        const VISIBLE_RANGED_ENTRY: u16 = 258 + 12 * 17 + 2;
        const VISIBLE_RANGED_PROPERTIES: u16 = 258 + 12 * 17 + 10;
        let (mut world, rx) = world();
        let mut spells = crate::ui_action::Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [(
                RANGED,
                SpellDisplay {
                    attributes: 0x2,
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
        );
        world.insert_resource(spells);
        let press = |world: &mut World, pairs: &[(u16, u32)]| {
            let mut all = vec![(22u16, 100u32)];
            all.extend_from_slice(pairs);
            let store: &'static ObjectStore =
                Box::leak(Box::new(ObjectStore(ObjectFields::from_pairs(&all))));
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
                    ladder.send(RANGED, &with_store, CastCommit::Spell);
                })
                .expect("one-shot");
            world.resource_mut::<CastReports>().take_refusals()
        };
        assert_eq!(press(&mut world, &[]), vec![CastFail::local(RANGED, 0x18)]);
        assert!(rx.try_recv().is_err(), "refused before the send");
        assert_eq!(
            press(
                &mut world,
                &[
                    (VISIBLE_RANGED_ENTRY, 2504),
                    (VISIBLE_RANGED_PROPERTIES, 0x0001_0000)
                ]
            ),
            vec![CastFail::local(RANGED, 0x18)],
            "the flag half's bit 0 hides the weapon"
        );
        assert!(press(&mut world, &[(VISIBLE_RANGED_ENTRY, 2504)]).is_empty());
        assert!(
            rx.try_iter().any(
                |c| matches!(c, ClientCommand::CastSpell { spell_id, .. } if spell_id == RANGED)
            ),
            "a bow in the slot passes the rung"
        );
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
