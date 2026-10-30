//! The action-bar drains: a queued `UseAction` becomes a cast, a swing, an item use or a macro
//! run; a queued `PickupAction`/`PlaceAction` becomes one `CMSG_SET_ACTION_BUTTON`, which gets no
//! reply, so a drag-swap is two independent sends.

use bevy::prelude::*;

use benilla_protocol::messages::{ActionButton, ACTION_KIND_ITEM, ACTION_KIND_SPELL};
use benilla_ui::script::{macro_slot_id, UiScript};

use crate::net::{ClientCommand, NetCommands};

use crate::spell::{cast_target, CastLadder};

use super::{attack_actor_refusal, ActionSlots, UiErrorKeys, SPELL_ATTACK};
use crate::query_cache::Ask;

/// What clicking an item action does, and to which copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ItemRoute {
    /// Use this copy: the wire `(bag_index, slot)` and its instance guid.
    Use((u8, u8, u64)),
    Equip((u8, u8, u64)),
    /// No copy found: the click does nothing.
    Nowhere,
}

/// The reference's equip-or-use decision for an item action (`0x4e5fdd`-`0x4e5ff7`):
/// `InventoryType` 0 always uses; otherwise a worn copy (equipment slots 0..18) uses in place and
/// a carried one equips. `find` is the inventory walk with the entry already bound.
pub(super) fn item_action_route(
    template: &benilla_protocol::ItemInfo,
    find: impl Fn(crate::ui_items::ItemSearch) -> Option<(u8, u8, u64)>,
) -> ItemRoute {
    let anywhere = |live_charges_only| crate::ui_items::ItemSearch {
        equipment_only: false,
        live_charges_only,
    };
    if template.inventory_type == 0 {
        // The use leg's charge filter (`0x4e603a`): spent copies are skipped for finite charges.
        return match find(anywhere(template.has_finite_charges())) {
            Some(pos) => ItemRoute::Use(pos),
            None => ItemRoute::Nowhere,
        };
    }
    if let Some(pos) = find(crate::ui_items::ItemSearch {
        equipment_only: true,
        live_charges_only: false,
    }) {
        return ItemRoute::Use(pos);
    }
    match find(anywhere(false)) {
        Some(pos) => ItemRoute::Equip(pos),
        None => ItemRoute::Nowhere,
    }
}

/// The melee probe's press with nothing selected, the player's Attack, as the ATTACKTARGET
/// binding's `AttackTarget()` is: the validator's pick commits its select, which signals in place
/// ([`crate::script_calls::ScriptCallHost::engine_select`]), then the toggle at what it settled.
pub(crate) fn attack_nearest_probe(
    mut probe: MessageReader<crate::target::AttackNearestRequest>,
    mut vm: crate::script_calls::VmWith<AttackPress<'static, 'static>>,
) {
    if probe.read().last().is_none() {
        return;
    }
    let select = {
        let mut press = vm.own();
        if press.selection.guid.is_some() {
            return;
        }
        let latched = press.seam.pet_latch.as_deref().is_some_and(|l| l.up());
        let mark = crate::lanes::mark();
        let Some(owed) = press.begin_validate(None, true) else {
            return;
        };
        crate::script_calls::EngineSelect {
            before: None,
            latched,
            mark,
            owed: owed.into(),
        }
    };
    vm.engine_select(select, |mut press, debt| {
        debt.pay(&mut press.selection, &mut press.seam)
    });
    let mut press = vm.own();
    if let Some(guid) = press.settle_attack() {
        press.start_attack(guid);
    }
}

/// What the player's Attack reads and writes: `AttackTarget` (`0x489b50`), the ATTACKTARGET
/// binding and `UseAction`'s Attack slot (through `TryCast`, `0x6e4c90`) all run `0x6131a0`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AttackPress<'w, 's> {
    pick: crate::target::AttackPick<'w, 's>,
    selection: ResMut<'w, crate::target::Selection>,
    seam: crate::creature_anim::AttackSeam<'w, 's>,
    ui_errors: Res<'w, UiErrorKeys>,
}

impl AttackPress<'_, '_> {
    /// `0x6131a0`'s second half, past the attack validator `0x612df0` (`0x6131aa`): the toggle at
    /// the validated target. Attacking (`0x60ecb0`) stops (`0x5ecac0`), else it starts there
    /// (`0x5ecb70`). Only the start cancels auto-repeat (`0x5ecd8c`), so stopping melee leaves
    /// Auto Shot running.
    pub(crate) fn start_attack(&mut self, guid: u64) {
        let seam = &mut self.seam;
        if seam.me.single().is_err() {
            return;
        }
        debug!(
            "attack {} at {guid:#x}",
            if seam.engaged() {
                "toggled off"
            } else {
                "swing"
            }
        );
        seam.toggle(guid);
    }

    /// A stand state change's attack stop inside its call, `0x5ecac0` at `0x5f07e7` while we
    /// attack (`0x60ecb0`).
    pub(crate) fn stop_for_stand(&mut self) {
        self.seam.stop();
    }

    /// The validator `0x612df0` up to its selection's re-read, for the call host, which runs the
    /// listeners of the selection its pick commits in place before [`Self::settle_attack`]: the
    /// actor checks, then the pick ([`crate::target::AttackPick::pick`]), what its commit owes;
    /// `None` once an actor check refused. Its `SelectTarget` calls (`0x613100`, `0x4941e6`) may
    /// hand a unit to the targeting cursor, which binds it before the swing goes out, so a caller
    /// that owns the cursor binds between the halves. The candidate is `passed`, else the
    /// selection: `AttackTarget` passes none, a go's deferred start its first hit (`0x6e8402` →
    /// `0x6131a0`).
    pub(crate) fn pick_attack_at(
        &mut self,
        passed: Option<u64>,
        may_pick: bool,
    ) -> Option<crate::target::Owed> {
        self.begin_validate(passed, may_pick)
    }

    /// Whether the validator keeps `guid` as its candidate (`0x6130a3`) rather than dropping it to
    /// the nearest-enemy cycle (`0x6130b5`).
    pub(crate) fn keeps(&self, guid: u64) -> bool {
        self.pick.keeps(guid)
    }

    /// Pay what the pick's commit owes now, for a caller with no listeners to run first.
    pub(crate) fn pay(&mut self, owed: crate::target::Owed) {
        crate::target::SelectDebt::from(owed).pay(&mut self.selection, &mut self.seam);
    }

    /// TryCast's attack pick (`0x6e4efb`) for a spell press held there
    /// ([`crate::spell::HeldForPick`]), [`Self::pick_attack_at`] with the caster as the candidate
    /// for a self-cast press (`0x4e610e`): the validator alone, with no swing after it. The cast
    /// goes on, at the unit the pick left selected, when [`Self::settle_attack`] passes.
    pub(crate) fn pick_for_cast(
        &mut self,
        held: crate::spell::HeldCast,
    ) -> Option<crate::target::Owed> {
        let caster = self.pick.player().1.filter(|_| held.on_self);
        self.begin_validate(caster, true)
    }

    /// The validator's second half (`0x6130c1` on): the selection as the pick's listeners left
    /// it, through the final gate.
    pub(crate) fn settle_attack(&mut self) -> Option<u64> {
        self.pick.settle(&self.selection, &self.ui_errors)
    }

    /// The validator up to its selection's re-read: the actor checks (phase A), then the pick.
    fn begin_validate(
        &mut self,
        passed: Option<u64>,
        may_pick: bool,
    ) -> Option<crate::target::Owed> {
        let Self {
            pick,
            selection,
            seam,
            ui_errors,
        } = self;
        let (me, my_guid) = pick.player();
        if attack_actor_refusal(me, my_guid, ui_errors) {
            return None;
        }
        Some(pick.pick(passed, may_pick, selection, seam))
    }
}

/// [`drain_click_casts`] after a walk's arrival in the frame time, whose dispatch queues the
/// cast the walk owed.
pub(crate) fn drain_arrival_casts(
    vm: crate::script_calls::VmWith<ClickCastDrain<'static, 'static>>,
) {
    drain_click_casts(vm);
}

/// Sends the casts a dispatcher outside any call queued (a crate's `Interact`) through the cast
/// ladder, as an engine turn among the frame's input records. The world click's and a nameplate
/// click's own are drained inside their calls ([`ClickCastDrain::drain`]).
pub(crate) fn drain_click_casts(
    mut vm: crate::script_calls::VmWith<ClickCastDrain<'static, 'static>>,
) {
    vm.turn(|script, mut drain| drain.drain(script));
}

/// The queued right-click casts and the cast ladder they go through: the reference's release
/// handler reaches the click router (`0x514b70` → `0x481f60`), whose dispatchers try the cast. The
/// reference reaches `TryCast` from a right-click as from a button (`0x5f35c0` and `0x5f06dc`,
/// then `0x6e5a90`, `0x6e4b60`), so every rung applies: a second click on a chest mid-cast is
/// `0x6e4d43`'s silent same-spell bail, and a skin while another spell is in flight `0x6e4d97`'s
/// 0x61.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ClickCastDrain<'w, 's> {
    targeting: cast_target::CastTargeting<'w, 's>,
    ladder: CastLadder<'w, 's>,
    gate: crate::ui_bind_confirm::BindGate<'w>,
    requirements: crate::ui_items::RequirementsLive<'w, 's>,
    queue: ResMut<'w, crate::ui_action::ClickCasts>,
    // Not a `CastLadder` field: Bevy panics on a resource reachable twice from one system.
    ui_errors: Res<'w, UiErrorKeys>,
}

impl ClickCastDrain<'_, '_> {
    /// Send every queued cast, in the order queued.
    pub(crate) fn drain(&mut self, mut script: Option<&mut UiScript>) {
        if self.queue.0.is_empty() {
            return;
        }
        let ctx = self.targeting.context();
        for cast in std::mem::take(&mut self.queue.0) {
            match cast {
                super::ClickCast::AtObject { spell_id, go_guid } => {
                    debug!("ui_action: gameobject opener casts {spell_id} at {go_guid:#x}");
                    self.ladder.send_at_object(spell_id, &ctx, go_guid);
                }
                super::ClickCast::AtUnit {
                    spell_id,
                    entity,
                    guid,
                    auto_loot,
                } => {
                    debug!("ui_action: right-click casts {spell_id} at {guid:#x}");
                    self.ladder.send(
                        spell_id,
                        &self.targeting.context_at(entity, guid),
                        crate::spell::CastCommit::Spell,
                    );
                    // Then the store, whatever the attempt answered (`0x5f06e1`-`0x5f06ef`).
                    if let (Some(byte), Some(script)) = (auto_loot, script.as_deref_mut()) {
                        script.set_auto_loot(byte);
                    }
                }
                // The key goes through the shared `CGItem::Use` fork, as a bag click does.
                super::ClickCast::Key(it) => {
                    let Some(script) = script.as_deref_mut() else {
                        continue;
                    };
                    // The use's requirement gate (`0x5d8e0e`) reads the player as of here.
                    self.requirements.seat(script);
                    debug!(
                        "ui_action: gameobject opener uses the key at wire {}/{} on {:?}",
                        it.bag_index, it.slot, it.on_object
                    );
                    crate::ui_items::send_item_use(
                        it,
                        &ctx,
                        &mut self.ladder,
                        script,
                        &mut self.gate,
                        false,
                        &self.ui_errors,
                    );
                }
            }
        }
    }
}

/// What a `UseAction` press reads and writes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ActionPress<'w, 's> {
    /// The slots as the calls before the press left them.
    slots: Res<'w, ActionSlots>,
    targeting: cast_target::CastTargeting<'w, 's>,
    // The by-key error line. Not a `CastLadder` field: Bevy panics, at runtime and not in unit
    // tests, on a resource reachable twice from one system.
    ui_errors: Res<'w, UiErrorKeys>,
    ladder: CastLadder<'w, 's>,
    gate: crate::ui_bind_confirm::BindGate<'w>,
    /// What the item's requirement gate and equip question read, seated with the host out.
    requirements: crate::ui_items::RequirementsLive<'w, 's>,
    /// The bags the item slot's auto-equip reads, lent with the host out; with no cooldown list,
    /// which the ladder holds to write, as the equip reads none.
    bags: Option<crate::ui_items::BagReads<'w, 's>>,
}

/// What a `UseAction` press leaves to another applier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub(crate) enum UseOutcome {
    Done,
    /// The Attack slot passed `TryCast`'s dead rung: the player's Attack ([`AttackPress`]) runs.
    Attack,
    /// A macro slot: its lines run inside the press (`0x4e6098 call 0x4f1460`), once the caller
    /// has the VM's host back, so the calls they make apply line by line.
    Macro(u32),
    /// An item slot's equip, past `SetCursorItem` (`0x5e18c9`): the set's signals fire in place
    /// (`0x494bb6`, `0x494c54`) before `AutoEquipCursorItem` goes on (`0x5e18d5`), so the caller
    /// runs its signal point and then [`finish_equip`], with whether the set put the item on
    /// (with no control it returns before, `0x494b8d` → `0x494c59`).
    Equip {
        picked: bool,
    },
}

/// `UseAction` (`0x4e5ee0`) on the pressed slot: a cast or an item use, at the selection as the
/// calls before it left it, or the Attack slot, a macro or an equip's second half, which it hands
/// back for the caller to run: the player's Attack, the macro's lines (`0x4e6098 call 0x4f1460`)
/// or [`finish_equip`] after the set's signals.
pub(crate) fn use_action(
    p: &mut ActionPress,
    script: &mut UiScript,
    press: benilla_ui::script::ActionUse,
) -> UseOutcome {
    with_bags(p, script, |p, script| press_slot(p, script, press))
}

/// `AutoEquipCursorItem` (`0x5e18d5`) on whatever the cursor holds after the equip's set and the
/// listeners it signalled, then the send: the second half of an item slot's equip
/// ([`UseOutcome::Equip`]); `picked`, whether the set put the item on.
pub(crate) fn finish_equip(p: &mut ActionPress, script: &mut UiScript, picked: bool) {
    with_bags(p, script, |p, script| auto_equip_cursor(p, script, picked));
}

/// Run `f` with the bags lent, when the press has them.
fn with_bags<R>(
    p: &mut ActionPress,
    script: &mut UiScript,
    f: impl FnOnce(&mut ActionPress, &mut UiScript) -> R,
) -> R {
    let reads = p.bags.take();
    let out = match reads.as_ref() {
        Some(reads) => script.reading(&mut reads.with(None), |script| f(p, script)),
        None => f(p, script),
    };
    p.bags = reads;
    out
}

/// [`use_action`] with the bags lent.
fn press_slot(
    p: &mut ActionPress,
    script: &mut UiScript,
    press: benilla_ui::script::ActionUse,
) -> UseOutcome {
    let ActionPress {
        slots,
        targeting,
        ui_errors,
        ladder,
        gate,
        requirements,
        bags: _,
    } = p;
    let selection = &targeting.selection;
    let action = press.action;
    let slot = match u8::try_from(action.saturating_sub(1)) {
        Ok(s) => s,
        Err(_) => return UseOutcome::Done,
    };
    match slots.buttons.get(&slot) {
        Some(b) if b.kind == ACTION_KIND_SPELL && b.action == SPELL_ATTACK => {
            // `UseAction` casts Attack through TryCast, so its dead rung comes first: "You are
            // dead", where the binding, which skips TryCast, reads "Can't attack while dead."
            // Then TryCast's effect-0x4e short-circuit runs the Attack toggle (`0x6e4c90`). The
            // gate (`0x6e4bc1`) is TryCast's first rung, ahead of both.
            if !ladder.permitted() {
                return UseOutcome::Done;
            }
            if ladder.dead_refusal(SPELL_ATTACK, targeting.self_store.iter().next()) {
                return UseOutcome::Done;
            }
            return UseOutcome::Attack;
        }
        Some(b) if b.kind == ACTION_KIND_SPELL => {
            // A live `ActiveIconID` spell re-pressed cancels its aura (`0x4e55f0`, cancel
            // `0x4e60c1`). The form-match toggle is `CastSpell`'s alone, not `UseAction`'s.
            if let Some(d) = ladder.spells.as_ref().and_then(|s| s.catalog.get(b.action)) {
                if let Some(store) = targeting.self_store.iter().next() {
                    if super::toggle::active_action_toggle(b.action, d, store) {
                        debug!("ui_action: cast {} re-pressed — aura cancels", b.action);
                        let _ = ladder
                            .commands
                            .0
                            .send(crate::net::ClientCommand::CancelAura { spell_id: b.action });
                        return UseOutcome::Done;
                    }
                }
            }
            // Then the running auto-repeat spell stops (`0x4e60d1`-`0x4e60da`), a toggle no
            // `CastSpellByName` has: `TryCast` never reads the key.
            if ladder.stop_auto_repeat_of(b.action) {
                debug!(
                    "ui_action: cast {} re-pressed — auto-repeat toggles off",
                    b.action
                );
                return UseOutcome::Done;
            }
            // Then the spell whose targeting cursor is up cancels it before `TryCast`
            // (`GetTargetingSpellId 0x4e60ea`, `StopTargeting 0x4e60f3`), as `CastBookSpell`
            // does for the spellbook (`0x4b3474`, [`crate::ui_spellbook::cast_spell`]).
            if ladder.ground.spell() == Some(b.action) {
                debug!(
                    "ui_action: cast {} re-pressed — targeting toggles off",
                    b.action
                );
                ladder.stop_targeting();
                return UseOutcome::Done;
            }
            debug!(
                "ui_action: cast {} (target {:?}{})",
                b.action,
                selection.guid,
                if press.on_self { ", on self" } else { "" }
            );
            ladder.send_spell(
                b.action,
                &targeting.context().self_bound(press.on_self),
                press.on_self,
            );
        }
        // An item action names an entry, so the click finds a copy. A miss only logs, with no
        // red error line: nothing was attempted.
        Some(b) if b.kind == ACTION_KIND_ITEM => {
            let Some(store) = targeting.self_store.iter().next() else {
                return UseOutcome::Done;
            };
            // The use only reads (`0x4e5fd2`).
            let template = ladder.items.template(b.action, 0, Ask::Read).cloned();
            // The reference bails on a null template too; the icon resolve usually cached it.
            let Some(template) = template else {
                debug!(
                    "ui_action: item action {action} (entry {}) has no template yet — skipped",
                    b.action
                );
                return UseOutcome::Done;
            };
            let route = item_action_route(&template, |s| {
                crate::ui_items::find_item(&store.0, &ladder.objects, b.action, s)
            });
            let ((bag_index, slot0, guid), equip) = match route {
                ItemRoute::Use(pos) => (pos, false),
                ItemRoute::Equip(pos) => (pos, true),
                ItemRoute::Nowhere => {
                    debug!(
                        "ui_action: item action {action} (entry {}) is nowhere in the inventory — skipped",
                        b.action
                    );
                    return UseOutcome::Done;
                }
            };
            if equip {
                // No quest guard: the bar tests only `InventoryType` (`0x4e5fdd`), unlike
                // the bag click (`StartQuest`, `0x4fa3c4`), so a quest-starter equips.
                debug!("ui_action: item action {action} auto-equip (wire {bag_index}/{slot0})");
                // `0x4e602a` runs `0x5e1840`: the bar's item takes the cursor, its inner clear
                // dropping and unlocking a held item. The set's signals run before
                // `AutoEquipCursorItem` ([`finish_equip`]), which asks first for a bind-on-equip
                // item and keeps it held. Nothing locks it, and a projectile's clear unlocks
                // (`0x5e1763`).
                requirements.seat(script);
                return match script.put_on_cursor_for_equip(guid) {
                    Some(picked) => UseOutcome::Equip { picked },
                    None => UseOutcome::Done,
                };
            } else {
                // The shared `CGItem::Use` fork (called at `0x4e607b`): a quest-starter offers
                // its quest, and the use runs the whole cast ladder. The wire's third byte is
                // the spell block ordinal, not a flag. A copy locked when the call was made is
                // refused first.
                if press.item_was_locked(guid) {
                    debug!("ui_action: item action {action} on a locked item — refused");
                    return UseOutcome::Done;
                }
                let spell_index = template.use_spell_index().unwrap_or(0);
                debug!(
                    "ui_action: item action {action} use (wire {bag_index}/{slot0}, spell #{spell_index})"
                );
                requirements.seat(script);
                crate::ui_items::send_item_use(
                    crate::ui_items::ItemUse {
                        guid: Some(guid),
                        start_quest: template.start_quest,
                        bag_index,
                        slot: slot0,
                        entry: b.action,
                        spell_index,
                        use_spell: template.use_spell.map(|u| u.spell_id),
                        on_object: None,
                        is_charter: template.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER
                            != 0,
                    },
                    &targeting.context(),
                    ladder,
                    script,
                    gate,
                    false,
                    ui_errors,
                );
            }
        }
        Some(b) => {
            // The macro arm (`0x4e5ee0` calls `0x4f1460` with the id): each body line goes through
            // the chat input, as if typed, under the press's input events.
            if let Some(id) = macro_slot_id(b.kind, b.action) {
                return UseOutcome::Macro(id);
            }
            debug!(
                "ui_action: action {action} kind {:#04x} has no use path",
                b.kind
            );
        }
        None => debug!("ui_action: UseAction({action}) on an empty slot"),
    }
    UseOutcome::Done
}

/// [`UseOutcome::Equip`]'s second half: `AutoEquipCursorItem` (`0x5e1480`) on the cursor as the
/// set and its listeners left it, its send, question or wait handed over in place, so it applies
/// as the call settles, ahead of its clear's signals.
fn auto_equip_cursor(p: &mut ActionPress, script: &mut UiScript, picked: bool) {
    p.requirements.seat(script);
    script.auto_equip_cursor(false, picked);
}

/// The action bar's sends inside their calls ([`benilla_ui::script::ActionBarCall`]): the
/// toggles' packet, and each slot write sent and stored.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ActionBarSteps<'w> {
    /// Absent in a harness without the action store, which sends without storing.
    slots: Option<ResMut<'w, ActionSlots>>,
    net: Res<'w, NetCommands>,
}

impl ActionBarSteps<'_> {
    /// The slots as the writes so far left them; empty in a harness without the store.
    pub(crate) fn buttons(&self) -> impl Iterator<Item = &ActionButton> {
        self.slots.iter().flat_map(|s| s.buttons.values())
    }

    /// The word the 1-based `lua_id` holds.
    pub(crate) fn button(&self, lua_id: u32) -> Option<&ActionButton> {
        let slot = u8::try_from(lua_id.checked_sub(1)?).ok()?;
        self.slots.as_deref()?.buttons.get(&slot)
    }

    /// `call` inside its verb.
    pub(crate) fn apply(&mut self, call: benilla_ui::script::ActionBarCall) {
        use benilla_ui::script::ActionBarCall;
        match call {
            // `0x4e771d`: one packet per call, with no change test.
            ActionBarCall::Toggles(toggles) => {
                let _ = self
                    .net
                    .0
                    .send(ClientCommand::SetActionBarToggles { toggles });
            }
            ActionBarCall::Set { id, packed } => self.set(id, packed),
            // The host's own, with the action states ([`crate::script_calls`]).
            ActionBarCall::RefreshUsable | ActionBarCall::Notify { .. } => {}
        }
    }

    /// Whether our player's arrival has loaded this login's bar ([`ActionSlots::loaded`]); never
    /// in a harness without the store.
    pub(crate) fn loaded(&self) -> bool {
        self.slots.as_deref().is_some_and(ActionSlots::loaded)
    }

    /// Our player's arrival's load of the staged words ([`ActionSlots::load_staged`]); nothing in
    /// a harness without the store.
    pub(crate) fn load_staged(&mut self, knows: impl Fn(u32) -> bool) -> Vec<u32> {
        self.slots
            .as_deref_mut()
            .map(|slots| slots.load_staged(knows))
            .unwrap_or_default()
    }

    /// The slot write `0x4e58e0` for the 1-based `lua_id` (`packed == 0` clears), which the VM
    /// already mirrors: `CMSG_SET_ACTION_BUTTON` sent, ahead of the `ACTIONBAR_SLOT_CHANGED` the
    /// VM signals after it, and the slot stored (`0x4e590a`).
    pub(crate) fn set(&mut self, lua_id: u32, packed: u32) {
        let Ok(slot) = u8::try_from(lua_id.saturating_sub(1)) else {
            debug!("ui_action: set_action_button lua id {lua_id} out of range — ignored");
            return;
        };
        debug!(
            "ui_action: set_action_button lua {lua_id} (wire slot {slot}) packed {packed:#010x}"
        );
        let _ = self.net.0.send(ClientCommand::SetActionButton {
            button: slot,
            packed,
        });
        let Some(slots) = self.slots.as_deref_mut() else {
            return;
        };
        if packed == 0 {
            slots.buttons.remove(&slot);
        } else {
            slots.buttons.insert(
                slot,
                ActionButton {
                    slot,
                    action: packed & 0x00FF_FFFF,
                    kind: (packed >> 24) as u8,
                },
            );
        }
    }
}
