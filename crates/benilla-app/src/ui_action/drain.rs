//! **Outward** — the two action-bar drains, and the law that decides what a click *does*.
//!
//! - **Use** ([`drain_action_uses`]): a queued `UseAction(n)` becomes wire. A SPELL action goes
//!   through the one cast-send path ([`crate::spell::CastLadder::send`]); the auto-attack action
//!   (6603) sends `CMSG_ATTACKSWING` at the selection, or acquires the nearest enemy when there is
//!   none; an ITEM action names an *entry*, not a position, so it must first find a copy and then
//!   decide equip-vs-use — [`item_action_route`], the byte-verified two-stage law of decision 0666.
//!   A MACRO action runs its body's lines through the chat-input door (`crate::ui_macro::run`,
//!   decision 0983) — the `0x4f1460` fork of the reference's own `UseAction`.
//! - **Set** ([`drain_action_sets`]): a queued `PickupAction`/`PlaceAction` mutation becomes one
//!   `CMSG_SET_ACTION_BUTTON` per entry (0218 §4: the bar is client-authoritative, there is no
//!   answer packet to lock against, and a drag-swap is two independent sends — never atomic).
//!
//! Both run `.after(UiInput)` so a click's intent goes out the same frame it was made. The two
//! queues are disjoint per gesture, so their relative order does not matter.

use bevy::prelude::*;

use benilla_protocol::messages::{
    ActionButton, ACTION_KIND_ITEM, ACTION_KIND_MACRO, ACTION_KIND_SPELL,
};
use benilla_ui::script::UiScript;

use crate::net::{ClientCommand, NetCommands};

use crate::spell::{cast_target, CastCommit, CastLadder};

use super::{attack_actor_refusal, PlayerActions, UiErrorKeys, SPELL_ATTACK};

/// What clicking an ITEM action does, and to which copy — [`item_action_route`]'s verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ItemRoute {
    /// Use this copy — the wire `(bag_index, slot)` plus the instance guid the shared use fork
    /// needs (`ui_items::item_use_command`, decision 0664).
    Use((u8, u8, u64)),
    /// Equip this copy — the same triple.
    Equip((u8, u8, u64)),
    /// No copy anywhere the walk reaches — the click does nothing.
    Nowhere,
}

/// The reference's **two-stage** equip-vs-use decision for an ITEM action (`0x4e5fdd`–`0x4e5ff7`;
/// decision 0666, which supersedes 0216 §7's guessed one):
///
/// ```text
/// InventoryType == 0            → USE          (a consumable is never equipped)
/// InventoryType != 0, worn      → USE IN PLACE (a copy is in equipment slots 0..18)
/// InventoryType != 0, not worn  → EQUIP        (the full walk finds the copy to equip)
/// ```
///
/// The second stage is the whole point. A ONE-stage `equippable → equip` fork can never *use* an
/// equipped trinket — it re-equips it forever — and before 0666 the walk did not look at the
/// equipment slots at all, so an equipped item's button was simply inert (reproduced live
/// 2026-07-26: `item action 1 (entry 25) not in any bag — skipped`).
///
/// `find` is the inventory walk ([`crate::ui_items::find_item`]) with the entry already bound, so
/// this stays a pure function of the template and the walk's answers — the law is testable
/// without a world.
pub(super) fn item_action_route(
    template: &benilla_protocol::ItemInfo,
    find: impl Fn(crate::ui_items::ItemSearch) -> Option<(u8, u8, u64)>,
) -> ItemRoute {
    let anywhere = |live_charges_only| crate::ui_items::ItemSearch {
        equipment_only: false,
        live_charges_only,
    };
    if template.inventory_type == 0 {
        // The use leg's mode-`0x20` charge filter (`0x4e603a`): only when the TEMPLATE says this
        // item carries finite charges does the search skip spent copies.
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

/// ATTACKTARGET through the binding table (0997; default T — 1.12's `AttackTarget()`): exactly
/// the action-bar attack arm below, without the action slot — the Phase A actor refusal first
/// ([`attack_actor_refusal`], the full `0x612df0` gate set), then the with-target attack-start
/// (auto-draw + swing) or the no-target nearest-acquire. One law, two doors, the reference's
/// own shape (`AttackTarget` and `UseAction`'s SPELL_ATTACK both land in `0x612df0`).
pub(super) fn attack_target_binding(
    binds: Res<crate::bindings::BindingsState>,
    targeting: cast_target::CastTargeting,
    mut acquire: MessageWriter<crate::target::AttackNearestRequest>,
    mut ui_errors: ResMut<UiErrorKeys>,
    mut ladder: CastLadder,
) {
    if !binds.fired(crate::bindings::cmd::ATTACK_TARGET) {
        return;
    }
    if attack_actor_refusal(
        targeting.self_store.iter().next(),
        targeting.context().self_guid,
        &mut ui_errors,
    ) {
        return;
    }
    match targeting.selection.guid {
        Some(guid) => {
            let Ok((e, engaged)) = ladder.self_player.single() else {
                return;
            };
            debug!(
                "bindings: ATTACKTARGET {} at {guid:#x}",
                if engaged { "toggled off" } else { "swing" }
            );
            // The same `0x6131a0` this binding's doc says it shares with the action button — so
            // it takes the same seam, toggle and all, instead of a second copy that drifts.
            crate::creature_anim::toggle_attack_local(
                e,
                guid,
                engaged,
                &mut ladder.queued_melee,
                &mut ladder.auto_repeat,
                &mut ladder.sheath,
                &mut ladder.ecs,
                &ladder.commands,
            );
        }
        None => {
            debug!("bindings: ATTACKTARGET with no target — acquiring nearest");
            acquire.write(crate::target::AttackNearestRequest);
        }
    }
}

/// **The world right-click's GameObject opener**, run through the one cast path (decision 2199) —
/// the seam [`crate::ui_action::GoOpenerCasts`] exists for.
///
/// The reference reaches TryCast from the GameObject strategy's use-sender exactly as it does from
/// a button press (`0x5f35c0 → 0x6e5a90 → 0x6e4b60`), so an opener
/// takes **every rung** — in-flight, cooldown/GCD, power, crowd control, mounted, water, moving,
/// form, reagents — and not the two the click used to check on its own. The rung that matters for
/// the report this closes is the in-flight one: the second right-click on a chest whose Opening
/// cast is still running is `6e4d43`'s **silent** same-spell bail, so no duplicate ever reaches the
/// wire and the running bar is never red-faded by the server's answer to a packet we should not
/// have sent.
pub(super) fn drain_go_openers(
    mut queue: ResMut<crate::ui_action::GoOpenerCasts>,
    script: Option<NonSendMut<UiScript>>,
    targeting: cast_target::CastTargeting,
    mut ladder: CastLadder,
    // The by-key local-refusal sink, passed explicitly for the same reason `drain_action_uses`
    // does: a resource reachable twice from one system is a `B0002` panic (decision 1903).
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
            // The key's own `CGItem::Use` fork — one function for every use surface (decision
            // 0664), so the key reaches the wire through the same route a bag click does.
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

/// The **self-cast modifier**, applied to a cast's targeting inputs: `UseAction`'s third argument
/// (1.12's `SELFACTIONBUTTON1`-`12`, `ALT-1`…`ALT-=`, through `ActionButtonUp(id, 1)`).
///
/// It substitutes the caster for the selection as the bind candidate — both halves of the
/// substitution, the guid *and* the store the relation checks read, which is exactly the pair
/// `resolve_cast_target`'s own autoSelfCast fallback swaps at `0x6e53d7`. So a self-cast runs the
/// ordinary binder against the ordinary candidate and needs no leg of its own; a spell that
/// cannot target a friendly unit refuses for the reason it always would, and one that needs no
/// target at all is still `SelfImplicit`.
///
/// **The behaviour is not in doubt; the byte attribution is INFERRED.** The reference's
/// `UseAction 0x4f1460` was not traced for this — the modelling here is that `onSelf` fills
/// `ArmCast 0x6e5250`'s explicit-guid argument (`6e5393`), the slot every benilla caster passes
/// zero in, because that is the one input that already means "bind this unit instead of the
/// selection". A forced-self leg of its own would be observationally identical, so nothing
/// downstream rests on which it is.
fn self_bound<'a>(
    mut ctx: cast_target::CastContext<'a>,
    press: benilla_ui::script::ActionUse,
) -> cast_target::CastContext<'a> {
    if press.on_self {
        ctx.selection_guid = ctx.self_guid;
        ctx.rel.target_store = ctx.rel.self_store;
    }
    ctx
}

pub(super) fn drain_action_uses(
    script: Option<NonSendMut<UiScript>>,
    actions: Res<PlayerActions>,
    targeting: cast_target::CastTargeting,
    mut acquire: MessageWriter<crate::target::AttackNearestRequest>,
    // The by-key local error line — the only sink here that is not the ladder's own
    // (`ladder.cast_errors` is the reason-coded one, `ladder.ground` the targeting mode). It is
    // deliberately NOT a `CastLadder` field: a resource reachable twice from one system is a
    // `B0002` panic on the first live frame, which compiles and passes every unit test
    // (decision 1903).
    mut ui_errors: ResMut<UiErrorKeys>,
    mut ladder: CastLadder,
    mut gate: crate::ui_bind_confirm::BindGate,
) {
    let selection = &targeting.selection;
    let Some(mut script) = script else {
        return;
    };
    for press in script.take_action_uses() {
        let action = press.action;
        let slot = match u8::try_from(action.saturating_sub(1)) {
            Ok(s) => s,
            Err(_) => continue,
        };
        match actions.buttons.get(&slot) {
            Some(b) if b.kind == ACTION_KIND_SPELL && b.action == SPELL_ATTACK => {
                // The attack-start validator's Phase A ([`attack_actor_refusal`]) — for a melee
                // swing the actor is US. It refuses BEFORE the with-target swing and before the
                // nearest-enemy scan (every Phase A gate precedes `0x6130b5`), so both arms gate
                // here. Widened from mounted-only to the rest of `0x612df0`:
                // stunned, pacified, fleeing, confused, charmed by somebody else, and dead now
                // refuse the swing too, each with its own red line.
                if attack_actor_refusal(
                    targeting.self_store.iter().next(),
                    targeting.context().self_guid,
                    &mut ui_errors,
                ) {
                    continue;
                }
                match selection.guid {
                    Some(guid) => {
                        // **The Attack button is a TOGGLE** — `0x6131a0`, which the base Attack
                        // pseudo-spell reaches through `TryCast`'s effect-0x4e short-circuit
                        // (`0x6e4c7a`), forks on `0x60ecb0`: already attacking →
                        // `0x6131d9 call 0x5ecac0` **StopAttack**, else `0x6131ee call 0x5ecb70`
                        // **StartAttack**.
                        //
                        // Both halves were wrong here. There was no toggle-off at all, and the
                        // press cancelled a running auto-repeat *unconditionally* — but only the
                        // START arm reaches `0x5ecd8c`, so in the reference toggling melee OFF
                        // leaves Auto Shot running. The seams carry the sheath snap and the cancel
                        // now, so neither is spelled out twice.
                        let Ok((e, engaged)) = ladder.self_player.single() else {
                            continue;
                        };
                        debug!(
                            "ui_action: attack {} at {guid:#x}",
                            if engaged { "toggled off" } else { "swing" }
                        );
                        crate::creature_anim::toggle_attack_local(
                            e,
                            guid,
                            engaged,
                            &mut ladder.queued_melee,
                            &mut ladder.auto_repeat,
                            &mut ladder.sheath,
                            &mut ladder.ecs,
                            &ladder.commands,
                        );
                    }
                    // No target: the client's attack resolver runs the nearest-enemy core and
                    // swings at the winner (`0x612df0` @ `6130b5`) — `target::scan` answers.
                    None => {
                        debug!("ui_action: attack with no target — acquiring nearest");
                        acquire.write(crate::target::AttackNearestRequest);
                    }
                }
            }
            Some(b) if b.kind == ACTION_KIND_SPELL => {
                // UseAction's toggle-cancel (`0x4e5ee0`: `GetTargetingSpellId 0x6e48e0` +
                // `StopTargeting 0x6e4900`, decision 0792): re-pressing the spell whose
                // targeting cursor is up cancels the targeting instead of re-arming it —
                // press-again-to-cancel, before TryCast ever runs. (A spellbook re-press stays
                // the ref's abort-and-re-enter — it never passes through UseAction.)
                if ladder.ground.spell() == Some(b.action) {
                    debug!(
                        "ui_action: cast {} re-pressed — targeting toggles off",
                        b.action
                    );
                    ladder.ground.clear();
                    continue;
                }
                // The active-action toggle (`0x4e55f0` → the `0x4e60c1` cancel): a live
                // ActiveIconID spell re-pressed on its button cancels its own aura — Ghost Wolf,
                // the druid forms, Stealth. The
                // form-match toggle is deliberately NOT here (the ref's `UseAction` has no such
                // leg — the `CastSpell` dispatcher alone carries it; keep the asymmetry).
                if let Some(d) = ladder.spells.as_ref().and_then(|s| s.catalog.get(b.action)) {
                    if let Some(store) = targeting.self_store.iter().next() {
                        if super::toggle::active_action_toggle(b.action, d, store) {
                            debug!("ui_action: cast {} re-pressed — aura cancels", b.action);
                            let _ = ladder
                                .commands
                                .0
                                .send(crate::net::ClientCommand::CancelAura { spell_id: b.action });
                            continue;
                        }
                    }
                }
                debug!(
                    "ui_action: cast {} (target {:?}{})",
                    b.action,
                    selection.guid,
                    if press.on_self { ", on self" } else { "" }
                );
                ladder.send(
                    b.action,
                    &self_bound(targeting.context(), press),
                    CastCommit::Spell,
                );
            }
            // An item action names an item ENTRY, not a position, so the click has to find a copy
            // — [`item_action_route`] is that law. A miss (the copy left the bags between the
            // click and this drain, or a stale action from a previous session) is a
            // debug-log-and-skip, NOT the red error line: nothing was attempted against the
            // server, so "Item is not ready" would be a lie.
            Some(b) if b.kind == ACTION_KIND_ITEM => {
                let Some(store) = targeting.self_store.iter().next() else {
                    continue;
                };
                let template = ladder
                    .items
                    .template(b.action, 0, &ladder.commands)
                    .cloned();
                // The reference reads the template first and bails on a null record; ours is all
                // but always cached by click time (the icon resolve needed it).
                let Some(template) = template else {
                    debug!(
                        "ui_action: item action {action} (entry {}) has no template yet — skipped",
                        b.action
                    );
                    continue;
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
                        continue;
                    }
                };
                if equip {
                    // Deliberately WITHOUT the bag click's quest guard: the bar's own engine tests
                    // only `[rec+0x2c]` (inventoryType) before the equip route (`0x4e5fdd`), where
                    // `Script::UseContainerItem` also tests `StartQuest` (`0x4fa3c4`) — so an
                    // equippable quest-starter on the bar equips, exactly as the reference does
                    // (decision 0664).
                    debug!("ui_action: item action {action} auto-equip (wire {bag_index}/{slot0})");
                    // Through the one auto-equip sender (decision 1750), which carries the ammo
                    // fork and the soulbind deferral: a BoE dragged to the bar and pressed asks
                    // before it binds, exactly as the same item right-clicked in the bag does.
                    crate::ui_items::send_auto_equip(
                        &mut script,
                        &mut gate,
                        &ladder.objects,
                        &ladder.items,
                        &ladder.commands,
                        bag_index,
                        slot0,
                        Some(guid),
                        false,
                    );
                } else {
                    // …then the shared use fork (`CGItem::Use` — the bar's engine calls the very
                    // same function at `0x4e607b`), so a quest-starter on the bar offers its quest
                    // instead of a `CMSG_USE_ITEM` the server can only refuse (decision 0664). The
                    // wire's third byte is the spell BLOCK ordinal, not a flag (decision 0666).
                    // The fork runs the WHOLE cast ladder — an item use IS a cast through the same
                    // `TryCast` (decisions 0908/0914; [`crate::ui_items::send_item_use`] is the
                    // law), so the cooldown/GCD/in-flight/mounted/moving/form rungs and the local
                    // "Item is not ready yet." live there now, for the bag and doll clicks too.
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
                            is_charter: template.flags
                                & benilla_protocol::messages::ITEM_FLAG_CHARTER
                                != 0,
                        },
                        &targeting.context(),
                        &mut ladder,
                        &mut script,
                        &mut gate,
                        false,
                        &mut ui_errors,
                    );
                }
            }
            // The MACRO arm (`0x4e5ee0`'s `and ecx,0xbfffffff; call 0x4f1460` fork): run the
            // macro's body. Every line goes onto the chat-input queue — the door a typed line
            // comes through — so `/cast`, `/target`, `/script`, the chat types and the 225 emotes
            // all work in a macro by construction (`crate::ui_macro::run`'s module doc).
            Some(b) if b.kind == ACTION_KIND_MACRO => {
                if !crate::ui_macro::run_macro(&mut script, b.action) {
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
    }
}

/// Drain the `(lua action id, packed)` pairs the cursor seam's `PickupAction`/`PlaceAction`
/// queued (decision 0216 §7) — the engine's own local mutation already agrees with what lands
/// here (it wrote the same value into its optimistic `model.actions` mirror before queuing this).
/// Each entry: write `PlayerActions.buttons` (`packed == 0` removes the slot, else inserts),
/// mark `dirty` so [`super::feed::feed_actions`] re-resolves + re-pushes + fires
/// `ACTIONBAR_SLOT_CHANGED` (the existing diff machinery — no bespoke event here), and send ONE
/// `CMSG_SET_ACTION_BUTTON` (0218 §4: client-authoritative, no answer packet, a drag-swap is two
/// independent sends).
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
