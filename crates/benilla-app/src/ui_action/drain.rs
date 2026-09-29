//! The action-bar drains: a queued `UseAction` becomes a cast, a swing, an item use or a macro
//! run; a queued `PickupAction`/`PlaceAction` becomes one `CMSG_SET_ACTION_BUTTON`, which gets no
//! reply, so a drag-swap is two independent sends.

use bevy::prelude::*;

use benilla_protocol::messages::{
    ActionButton, ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
};
use benilla_ui::script::UiScript;

use crate::net::{ClientCommand, NetCommands};

use crate::spell::{cast_target, CastLadder};

use super::{attack_actor_refusal, PlayerActions, UiErrorKeys, SPELL_ATTACK};

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

/// The melee probe's press with nothing selected, the player's Attack
/// ([`AttackPress::attack_target`]), as the ATTACKTARGET binding's `AttackTarget()` is.
pub(crate) fn attack_nearest_probe(
    mut probe: MessageReader<crate::target::AttackNearestRequest>,
    mut press: AttackPress,
) {
    if probe.read().last().is_some() && press.selection.guid.is_none() {
        press.attack_target();
    }
}

/// What the player's Attack reads and writes: `AttackTarget` (`0x489b50`), the ATTACKTARGET
/// binding and `UseAction`'s Attack slot (through `TryCast`, `0x6e4c90`) all run `0x6131a0`.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AttackPress<'w, 's> {
    pick: crate::target::AttackPick<'w, 's>,
    selection: ResMut<'w, crate::target::Selection>,
    seam: crate::creature_anim::AttackSeam<'w, 's>,
    ui_errors: ResMut<'w, UiErrorKeys>,
}

impl AttackPress<'_, '_> {
    /// `0x6131a0`: the attack validator `0x612df0` (`0x6131aa`) runs first, its actor checks then
    /// its target pick, which may move the selection; a refusal ends the press. Then the toggle:
    /// attacking (`0x60ecb0`) stops (`0x5ecac0`), else it starts at the validator's target
    /// (`0x5ecb70`). Only the start cancels auto-repeat (`0x5ecd8c`), so stopping melee leaves
    /// Auto Shot running.
    pub(crate) fn attack_target(&mut self) {
        let Some(guid) = self.validate(None) else {
            return;
        };
        let Self { pick, seam, .. } = self;
        let Ok(e) = seam.me.single() else {
            return;
        };
        let engaged = pick.engaged();
        debug!(
            "attack {} at {guid:#x}",
            if engaged { "toggled off" } else { "swing" }
        );
        crate::creature_anim::toggle_attack_local(
            e,
            guid,
            engaged,
            &mut seam.queued_melee,
            &mut seam.auto_repeat,
            &mut seam.sheath,
            &mut seam.ecs,
            &seam.net,
        );
    }

    /// TryCast's attack pick (`0x6e4efb`) for a spell press held there
    /// ([`crate::spell::HeldForPick`]): the validator alone, with no swing after it. Whether the
    /// cast goes on, at the unit the pick left selected.
    pub(crate) fn pick_for_cast(&mut self, held: crate::spell::HeldCast) -> bool {
        let caster = self.pick.player().1.filter(|_| held.on_self);
        self.validate(caster).is_some()
    }

    /// The attack validator `0x612df0`: its actor checks, then its target pick.
    fn validate(&mut self, passed: Option<u64>) -> Option<u64> {
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
        pick.target(passed, selection, seam, ui_errors)
    }
}

/// Sends the queued GameObject openers through the cast ladder. The reference reaches `TryCast`
/// from a right-click as from a button (`0x5f35c0`, `0x6e5a90`, `0x6e4b60`), so every rung
/// applies: a second click on a chest mid-cast is `0x6e4d43`'s silent same-spell bail.
pub(super) fn drain_go_openers(
    mut queue: ResMut<crate::ui_action::GoOpenerCasts>,
    script: Option<NonSendMut<UiScript>>,
    targeting: cast_target::CastTargeting,
    mut ladder: CastLadder,
    // Not a `CastLadder` field: Bevy panics on a resource reachable twice from one system.
    mut ui_errors: ResMut<UiErrorKeys>,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    if queue.0.is_empty() {
        return;
    }
    let ctx = targeting.context();
    let mut script = script;
    for opener in std::mem::take(&mut queue.0) {
        match opener {
            super::GoOpener::Spell { spell_id, go_guid } => {
                debug!("ui_action: gameobject opener casts {spell_id} at {go_guid:#x}");
                ladder.send_at_object(spell_id, &ctx, go_guid);
            }
            // The key goes through the shared `CGItem::Use` fork, as a bag click does.
            super::GoOpener::Key(it) => {
                let Some(script) = script.as_deref_mut() else {
                    continue;
                };
                debug!(
                    "ui_action: gameobject opener uses the key at wire {}/{} on {:?}",
                    it.bag_index, it.slot, it.on_object
                );
                crate::ui_items::send_item_use(
                    it,
                    &ctx,
                    &mut ladder,
                    script,
                    &mut gate,
                    false,
                    &mut ui_errors,
                );
            }
        }
    }
}

/// What a `UseAction` press reads and writes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ActionPress<'w, 's> {
    actions: Res<'w, PlayerActions>,
    targeting: cast_target::CastTargeting<'w, 's>,
    // The by-key error line. Not a `CastLadder` field: Bevy panics, at runtime and not in unit
    // tests, on a resource reachable twice from one system.
    ui_errors: ResMut<'w, UiErrorKeys>,
    ladder: CastLadder<'w, 's>,
    gate: crate::ui_bind_confirm::BindGate<'w>,
}

/// What a `UseAction` press leaves to another applier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub(crate) enum UseOutcome {
    Done,
    /// The Attack slot passed `TryCast`'s dead rung: the player's Attack ([`AttackPress`]) runs.
    Attack,
}

/// `UseAction` (`0x4e5ee0`) on the pressed slot: a cast, an item use or a macro run, at the
/// selection as the calls before it left it, or the Attack slot, which it hands back for the
/// player's Attack to run. A macro's lines run inside it (`0x4e6098 call 0x4f1460`), so the calls
/// they make queue for [`crate::script_calls`] to apply next.
pub(crate) fn use_action(
    p: &mut ActionPress,
    script: &mut UiScript,
    press: benilla_ui::script::ActionUse,
) -> UseOutcome {
    let ActionPress {
        actions,
        targeting,
        ui_errors,
        ladder,
        gate,
    } = p;
    let selection = &targeting.selection;
    let action = press.action;
    let slot = match u8::try_from(action.saturating_sub(1)) {
        Ok(s) => s,
        Err(_) => return UseOutcome::Done,
    };
    match actions.buttons.get(&slot) {
        Some(b) if b.kind == ACTION_KIND_SPELL && b.action == SPELL_ATTACK => {
            // `UseAction` casts Attack through TryCast, so its dead rung comes first: "You are
            // dead", where the binding, which skips TryCast, reads "Can't attack while dead."
            // Then TryCast's effect-0x4e short-circuit runs the Attack toggle (`0x6e4c90`).
            if ladder.dead_refusal(SPELL_ATTACK, targeting.self_store.iter().next()) {
                return UseOutcome::Done;
            }
            return UseOutcome::Attack;
        }
        Some(b) if b.kind == ACTION_KIND_SPELL => {
            // In `UseAction 0x4e5ee0` only: re-pressing the spell whose targeting cursor is
            // up cancels it before `TryCast` (`GetTargetingSpellId 0x6e48e0`,
            // `StopTargeting 0x6e4900`). A spellbook re-press aborts and re-enters.
            if ladder.ground.spell() == Some(b.action) {
                debug!(
                    "ui_action: cast {} re-pressed — targeting toggles off",
                    b.action
                );
                ladder.ground.clear();
                return UseOutcome::Done;
            }
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
            let template = ladder
                .items
                .template(b.action, 0, &ladder.commands)
                .cloned();
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
                // The shared auto-equip sender, so a BoE asks before binding, as from a bag.
                crate::ui_items::send_auto_equip(
                    script,
                    gate,
                    &ladder.objects,
                    &ladder.items,
                    &ladder.commands,
                    bag_index,
                    slot0,
                    Some(guid),
                    false,
                );
            } else {
                // The shared `CGItem::Use` fork (called at `0x4e607b`): a quest-starter offers
                // its quest, and the use runs the whole cast ladder. The wire's third byte is
                // the spell block ordinal, not a flag.
                let spell_index = template.use_spell_index().unwrap_or(0);
                debug!(
                    "ui_action: item action {action} use (wire {bag_index}/{slot0}, spell #{spell_index})"
                );
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
        // The macro arm (`0x4e5ee0` calls `0x4f1460`): each body line goes through the chat
        // input, as if typed.
        Some(b) if b.kind == ACTION_KIND_MACRO => {
            if !crate::ui_macro::run_macro(script, b.action) {
                debug!(
                    "ui_action: macro action {action} (macro {}) is empty",
                    b.action
                );
            }
        }
        Some(b) => {
            debug!(
                "ui_action: action {action} kind {:#04x} has no use path",
                b.kind
            );
        }
        None => debug!("ui_action: UseAction({action}) on an empty slot"),
    }
    UseOutcome::Done
}

/// Applies the queued `PickupAction`/`PlaceAction` writes (packed 0 clears) to the store, which
/// the VM already mirrors, marks it dirty for the feed and sends each as `CMSG_SET_ACTION_BUTTON`.
pub(super) fn drain_action_sets(
    script: Option<NonSendMut<UiScript>>,
    mut actions: ResMut<PlayerActions>,
    commands: Res<NetCommands>,
) {
    let Some(mut script) = script else {
        return;
    };
    for (lua_id, packed) in script.take_action_sets() {
        let Ok(slot) = u8::try_from(lua_id.saturating_sub(1)) else {
            debug!("ui_action: set_action_button lua id {lua_id} out of range — ignored");
            continue;
        };
        if packed == 0 {
            actions.buttons.remove(&slot);
        } else {
            actions.buttons.insert(
                slot,
                ActionButton {
                    slot,
                    action: packed & 0x00FF_FFFF,
                    kind: (packed >> 24) as u8,
                },
            );
        }
        actions.dirty = true;
        debug!(
            "ui_action: set_action_button lua {lua_id} (wire slot {slot}) packed {packed:#010x}"
        );
        let _ = commands.0.send(ClientCommand::SetActionButton {
            button: slot,
            packed,
        });
    }
}
