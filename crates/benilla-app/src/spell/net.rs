//! The spell's packet handlers: the spell book and action bar, the cast lifecycle, cooldowns,
//! channels, aura durations and spell modifiers. They reach the lifecycle state
//! ([`super::inflight`], [`super::cooldowns`], [`super::mods`]) and the windows' stores through
//! [`Lifecycle`] and [`Scene`].

use std::time::{Duration, Instant};

use benilla_formats::LearnAnnouncement;
use benilla_protocol::messages::{ActionButton, SpellCooldown};
use bevy::prelude::*;

use super::cast_target::CastTargeting;
use super::{ActiveChannel, AutoRepeatActive, CastCommit, CastLadder, Cooldowns, PendingCast};
use crate::creature_anim::{CastEvent, CastEventKind, Casting, SpellGoTargets};
use crate::ui_action::{PlayerActions, Spells, UiError, UiErrorKeys};
use crate::ui_aura::AuraDurations;
use crate::ui_cast::{CastBarEdge, CastReports};
use crate::ui_spellbook::LearnedInTab;

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemParam;

use crate::net::{GuidIndex, NetCommands, NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};
use crate::query_cache::Ask;

/// Register the spell's handlers; called from [`super::SpellPlugin`].
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::SpellBook, on_spell_book)
        .net_handler(K::ActionButtons, on_action_buttons)
        .net_handler(K::SpellLearned, on_spell_learned)
        .net_handler(K::SpellRemoved, on_spell_removed)
        .net_handler(K::SpellSuperceded, on_spell_superceded)
        .net_handler(K::CastResult, on_cast_result)
        .net_handler(K::SpellStart, on_spell_start)
        .net_handler(K::SpellGo, on_spell_go)
        .net_handler(K::SpellChainTargets, on_spell_chain_targets)
        .net_handler(K::SpellFailedOther, on_spell_failed_other)
        .net_handler(K::SpellDelayed, on_spell_delayed)
        .net_handler(K::CancelAutoRepeat, on_cancel_auto_repeat)
        .net_handler(K::SpellCooldowns, on_cooldown_packet)
        .net_handler(K::CooldownEvent, on_cooldown_packet)
        .net_handler(K::ClearCooldown, on_cooldown_packet)
        .net_handler(K::CooldownCheat, on_cooldown_packet)
        .net_handler(K::ItemCooldown, on_item_cooldown)
        .net_handler(K::ChannelStart, on_channel)
        .net_handler(K::ChannelUpdate, on_channel)
        .net_handler(K::AuraDuration, on_aura_duration)
        .net_handler(K::SpellModifier, on_spell_modifier);
}

/// The cast lifecycle's state and catalogs, which every handler here folds its packet into,
/// plus the windows' stores the spell packets write. The cast reply's handler takes the ladder
/// instead ([`on_cast_result`]).
#[derive(SystemParam)]
pub(crate) struct Lifecycle<'w> {
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    spells: Option<Res<'w, Spells>>,
    net: Res<'w, NetCommands>,
    actions: ResMut<'w, PlayerActions>,
    learned_in_tab: ResMut<'w, LearnedInTab>,
    reports: ResMut<'w, CastReports>,
    pending: ResMut<'w, PendingCast>,
    cooldowns: ResMut<'w, Cooldowns>,
    auto_repeat: ResMut<'w, AutoRepeatActive>,
    channel: ResMut<'w, ActiveChannel>,
    aura_durations: ResMut<'w, AuraDurations>,
    spell_mods: ResMut<'w, crate::spell::SpellModifiers>,
    pet_bar: ResMut<'w, crate::ui_pet::PetBar>,
    /// The PlayAnimation call-order counter: every animation-bearing message stamps `next()`,
    /// in packet order.
    play_seq: ResMut<'w, crate::creature_anim::PlaySeq>,
}

/// The scene a cast lands in: the streamed units, the item store, and the animation, text and
/// loot sinks.
#[derive(SystemParam)]
pub(crate) struct Scene<'w, 's> {
    commands: Commands<'w, 's>,
    casting: Query<'w, 's, &'static Casting>,
    engaged_self: Query<'w, 's, (), (With<crate::creature_anim::Engaged>, With<SelfPlayer>)>,
    stores: Query<'w, 's, &'static mut ObjectStore>,
    items: ResMut<'w, crate::items::Items>,
    loot_latch: ResMut<'w, crate::ui_loot::LootLatch>,
    damage_text: Res<'w, crate::combat_text::DamageTextGates>,
    cast_events: MessageWriter<'w, CastEvent>,
    go_targets: MessageWriter<'w, SpellGoTargets>,
    text: MessageWriter<'w, crate::combat_text::CombatTextSpawn>,
    go_lid: MessageWriter<'w, crate::go_anim::GoLidOpen>,
    sheaths: MessageWriter<'w, crate::creature_anim::SheathRequest>,
}

/// `SMSG_INITIAL_SPELLS`, once at login, into the action store the UI feed reads.
fn on_spell_book(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    if let SessionEvent::SpellBook {
        spell_ids,
        cooldowns,
    } = ev
    {
        spell_book(spell_ids, cooldowns, &mut l.actions, &mut l.cooldowns);
    }
}

fn on_action_buttons(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    if let SessionEvent::ActionButtons { buttons } = ev {
        action_buttons(buttons, &mut l.actions);
    }
}

fn on_spell_learned(In(ev): In<SessionEvent>, mut l: Lifecycle, mut errors: ResMut<UiErrorKeys>) {
    if let SessionEvent::SpellLearned { spell_id } = ev {
        learned_spell(
            spell_id,
            &mut l.actions,
            l.spells.as_deref(),
            &mut errors,
            &mut l.learned_in_tab,
        );
    }
}

fn on_spell_removed(In(ev): In<SessionEvent>, mut l: Lifecycle, mut errors: ResMut<UiErrorKeys>) {
    if let SessionEvent::SpellRemoved { spell_id } = ev {
        removed_spell(spell_id, &mut l.actions, l.spells.as_deref(), &mut errors);
    }
}

fn on_spell_superceded(
    In(ev): In<SessionEvent>,
    mut l: Lifecycle,
    mut errors: ResMut<UiErrorKeys>,
) {
    if let SessionEvent::SpellSuperceded {
        old_spell_id,
        new_spell_id,
    } = ev
    {
        superceded_spell(
            old_spell_id,
            new_spell_id,
            &mut l.actions,
            l.spells.as_deref(),
            &mut errors,
            &mut l.learned_in_tab,
        );
    }
}

/// What `HandleCastResult` reaches beyond the ladder.
#[derive(SystemParam)]
pub(crate) struct Reply<'w, 's> {
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    casting: Query<'w, 's, &'static Casting>,
    cast_events: MessageWriter<'w, CastEvent>,
    play_seq: ResMut<'w, crate::creature_anim::PlaySeq>,
    items: Option<Res<'w, crate::items::Items>>,
}

/// `HandleCastResult 0x6e7330`, the one handler that casts: a reply whose spell names a
/// `modalNextSpell` (column 38) chains it through the ladder in the same call (`0x6e74aa call
/// 0x6e5a90` → `TryCast 0x6e4b60`), so it arms the in-flight slot before the next packet.
///
/// The chained cast goes out at the null target guid (`0x6e74a6 push ebx; push ebx`, `ebx = 0`),
/// so it binds through `ArmCast 0x6e5250`'s ordinary walk ([`CastTargeting::context`]) and takes
/// every rung a press takes.
fn on_cast_result(
    In(ev): In<SessionEvent>,
    mut ladder: CastLadder,
    targeting: CastTargeting,
    mut r: Reply,
) {
    if let SessionEvent::CastResult {
        spell_id,
        success,
        reason,
        arg,
    } = ev
    {
        let mut core = super::CastCore {
            pending: &mut ladder.pending,
            queued_melee: &mut ladder.queued_melee,
            ground: &mut ladder.ground,
            reports: &mut ladder.reports,
            net: &ladder.commands,
        };
        let chained = cast_result(
            spell_id,
            success,
            reason,
            arg,
            &mut ladder.ecs,
            &r.self_guid,
            &r.index,
            &r.casting,
            &mut r.cast_events,
            &mut core,
            &mut ladder.cooldowns,
            &mut ladder.auto_repeat,
            ladder.spells.as_deref(),
            r.items.as_deref(),
            &ladder.commands,
            r.play_seq.next(),
        );
        if let Some(next) = chained {
            let ctx = targeting.context();
            ladder.send(next, &ctx, CastCommit::Spell);
        }
    }
}

fn on_spell_start(In(ev): In<SessionEvent>, mut l: Lifecycle, mut sc: Scene) {
    if let SessionEvent::SpellStart {
        caster,
        spell_id,
        cast_flags,
        cast_time_ms,
        target,
        ammo_display_id,
    } = ev
    {
        spell_start(
            caster,
            spell_id,
            cast_flags,
            cast_time_ms,
            target,
            ammo_display_id,
            &mut sc.commands,
            &l.index,
            &mut sc.cast_events,
            &l.self_guid,
            &mut l.reports,
            &mut l.pending,
            l.spells.as_deref(),
            l.play_seq.next(),
        );
    }
}

fn on_spell_go(In(ev): In<SessionEvent>, mut l: Lifecycle, mut sc: Scene) {
    if let SessionEvent::SpellGo {
        caster,
        spell_id,
        cast_flags,
        hits,
        misses,
        target,
        go_target,
        dest,
        ammo_display_id,
        item_caster,
    } = ev
    {
        let engaged = !sc.engaged_self.is_empty();
        spell_go(
            caster,
            spell_id,
            cast_flags,
            hits,
            misses,
            target,
            go_target,
            dest,
            ammo_display_id,
            item_caster,
            &mut sc.commands,
            &l.index,
            &sc.casting,
            &mut sc.cast_events,
            &mut sc.go_targets,
            &l.self_guid,
            &sc.stores,
            &mut sc.text,
            *sc.damage_text,
            &mut sc.go_lid,
            &mut sc.loot_latch,
            (
                &mut l.cooldowns,
                l.spells.as_deref(),
                &mut sc.items,
                &l.net,
                &mut l.pet_bar,
                &l.spell_mods,
            ),
            (&mut l.auto_repeat, &mut sc.sheaths, engaged),
            l.play_seq.next(),
        );
    }
}

fn on_spell_chain_targets(In(ev): In<SessionEvent>, l: Lifecycle, mut sc: Scene) {
    if let SessionEvent::SpellChainTargets {
        caster,
        spell_id,
        targets,
    } = ev
    {
        spell_chain_targets(caster, spell_id, targets, &mut sc.commands, &l.index);
    }
}

fn on_spell_failed_other(In(ev): In<SessionEvent>, mut l: Lifecycle, mut sc: Scene) {
    if let SessionEvent::SpellFailedOther { caster, spell_id } = ev {
        spell_failed_other(
            caster,
            spell_id,
            &mut sc.commands,
            &l.index,
            &sc.casting,
            &mut sc.cast_events,
            &l.self_guid,
            l.play_seq.next(),
        );
    }
}

fn on_spell_delayed(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    if let SessionEvent::SpellDelayed { caster, delay_ms } = ev {
        spell_delayed(
            caster,
            delay_ms,
            &l.self_guid,
            &mut l.reports,
            &mut l.pending,
        );
    }
}

fn on_cancel_auto_repeat(In(ev): In<SessionEvent>, mut l: Lifecycle, mut sc: Scene) {
    if let SessionEvent::CancelAutoRepeat = ev {
        cancel_auto_repeat(
            &mut l.auto_repeat,
            &l.self_guid,
            &l.index,
            &mut sc.commands,
            &l.net,
        );
    }
}

/// The four cooldown packets, each routed through [`addressed_store`] by the caster guid.
fn on_cooldown_packet(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    let Lifecycle {
        self_guid,
        spells,
        cooldowns,
        pet_bar,
        ..
    } = &mut l;
    match ev {
        SessionEvent::SpellCooldowns {
            caster,
            cooldowns: pairs,
        } => {
            if let Some(store) = addressed_store(caster, self_guid, cooldowns, pet_bar) {
                spell_cooldowns(caster, pairs, spells.as_deref(), store);
            }
        }
        SessionEvent::CooldownEvent { spell_id, caster } => {
            if let Some(store) = addressed_store(caster, self_guid, cooldowns, pet_bar) {
                cooldown_event(spell_id, caster, store);
            }
        }
        SessionEvent::ClearCooldown { spell_id, caster } => {
            if let Some(store) = addressed_store(caster, self_guid, cooldowns, pet_bar) {
                clear_cooldown(spell_id, caster, store);
            }
        }
        SessionEvent::CooldownCheat { caster } => {
            if let Some(store) = addressed_store(caster, self_guid, cooldowns, pet_bar) {
                cooldown_cheat(caster, store);
            }
        }
        _ => {}
    }
}

fn on_item_cooldown(In(ev): In<SessionEvent>, mut l: Lifecycle, sc: Scene) {
    if let SessionEvent::ItemCooldown {
        item_guid,
        spell_id,
    } = ev
    {
        item_cooldown(item_guid, spell_id, &l.index, &sc.stores, &mut l.cooldowns);
    }
}

fn on_channel(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    match ev {
        SessionEvent::ChannelStart {
            spell_id,
            duration_ms,
        } => channel_start(spell_id, duration_ms, &mut l.channel, &mut l.reports),
        SessionEvent::ChannelUpdate { remaining_ms } => {
            channel_update(remaining_ms, &mut l.channel, &mut l.reports)
        }
        _ => {}
    }
}

fn on_aura_duration(In(ev): In<SessionEvent>, mut l: Lifecycle, real_clock: Res<Time<Real>>) {
    if let SessionEvent::AuraDuration { slot, remaining_ms } = ev {
        aura_duration(
            slot,
            remaining_ms,
            &mut l.aura_durations,
            real_clock.elapsed_secs_f64(),
        );
    }
}

fn on_spell_modifier(In(ev): In<SessionEvent>, mut l: Lifecycle) {
    if let SessionEvent::SpellModifier {
        flat,
        mask_bit,
        op,
        value,
    } = ev
    {
        set_spell_modifier(flat, mask_bit, op, value, &mut l.spell_mods);
    }
}

/// Which cooldown store a cooldown packet's caster guid addresses: ours or our pet's (the server
/// sends a pet's cooldowns on the pet's guid). Any other guid is dropped, as the reference drops
/// it; its `SMSG_COOLDOWN_CHEAT` handler wipes the self or pet list on this same test.
fn addressed_store<'a>(
    caster: u64,
    self_guid: &SelfGuid,
    player: &'a mut crate::spell::Cooldowns,
    pet: &'a mut crate::ui_pet::PetBar,
) -> Option<&'a mut crate::spell::Cooldowns> {
    if self_guid.0 == Some(caster) {
        Some(player)
    } else if pet.has_bar() && pet.spells.pet_guid == caster {
        Some(&mut pet.cooldowns)
    } else {
        None
    }
}

/// `SMSG_INITIAL_SPELLS`: the book into the action store, and the active cooldowns, whose wire
/// value is remaining ms, into the cooldown store ([`Cooldowns::seed_initial`]).
fn spell_book(
    spell_ids: Vec<u32>,
    initial_cooldowns: Vec<SpellCooldown>,
    actions: &mut PlayerActions,
    cooldowns: &mut Cooldowns,
) {
    debug!(
        "net: spell book — {} spells, {} active cooldown(s)",
        spell_ids.len(),
        initial_cooldowns.len()
    );
    actions.spells = spell_ids.into_iter().collect();
    actions.dirty = true;
    let now = Instant::now();
    for cd in &initial_cooldowns {
        cooldowns.seed_initial(cd, now);
    }
}

/// `SMSG_ACTION_BUTTONS`, at login and on server-side edits, into the action store.
fn action_buttons(buttons: Vec<ActionButton>, actions: &mut PlayerActions) {
    debug!("net: action bar — {} occupied slots", buttons.len());
    actions.buttons = buttons.into_iter().map(|b| (b.slot, b)).collect();
    actions.dirty = true;
}

/// `SMSG_LEARNED_SPELL`: the spell joins the book (the spellbook feed diffs it; learning never
/// bars a spell) and is announced in chat, the reference's own tail: `0x5e61c0` →
/// `AddSpell(id, slot, 1, 1)` → the registrar `0x4b25b0` with its announce flag set.
fn learned_spell(
    spell_id: u32,
    actions: &mut PlayerActions,
    spells: Option<&Spells>,
    errors: &mut UiErrorKeys,
    tab_flash: &mut LearnedInTab,
) {
    debug!("net: learned spell {spell_id}");
    if actions.spells.insert(spell_id) {
        actions.dirty = true;
    }
    announce_learn(spell_id, spells, errors);
    tab_flash.0.push(spell_id);
}

/// The `ERR_LEARN_*` chat line the registrar `0x4b25b0` prints at `0x4b2909` for a spell learned
/// mid-session; which line, and whether it carries the rank, is
/// [`benilla_formats::SpellDisplay::learn_announcement`]'s. Ids `0x37`/`0x38`/`0x39` are chat
/// type 10 rows, so all three land on the system channel. An unknown id says nothing, the
/// registrar's bail at `0x4b25c6`/`0x4b25d2`/`0x4b25e0`.
fn announce_learn(spell_id: u32, spells: Option<&Spells>, errors: &mut UiErrorKeys) {
    let Some(display) = spells.and_then(|s| s.catalog.get(spell_id)) else {
        return;
    };
    let Some(kind) = display.learn_announcement() else {
        return;
    };
    let (key, arg) = match kind {
        LearnAnnouncement::Spell => ("ERR_LEARN_SPELL_S", display.ranked_name()),
        LearnAnnouncement::Ability => ("ERR_LEARN_ABILITY_S", display.ranked_name()),
        // The recipe arm pushes the bare name: it returns before the rank composer.
        LearnAnnouncement::Recipe => ("ERR_LEARN_RECIPE_S", display.name.clone()),
    };
    errors.0.push(UiError::s(key, arg));
}

/// `SMSG_REMOVED_SPELL`: the spell leaves the book. A talent wipe arrives as these (vmangos
/// `ResetTalents` calls `RemoveSpell` on every rank, each sending `SendSpellRemoved`), and
/// [`crate::ui_talent`] derives rank from the book. The feeds diff `spells`, and
/// [`crate::ui_action::LearnedAbilities`] re-derives from it (the reference's unlearn write site,
/// `0x4b2c50`); a bar button naming the spell is left as is.
///
/// It announces "You have unlearned %s." under
/// [`benilla_formats::SpellDisplay::announces_unlearn`]'s gates: `RemoveSpell 0x5e9fe0`'s block at
/// `0x5ea292`, a branch target past its `ret 0x8` at `0x5ea28f`, is reached from this packet's arm
/// (`0x5e43e3`) with the flag set.
fn removed_spell(
    spell_id: u32,
    actions: &mut PlayerActions,
    spells: Option<&Spells>,
    errors: &mut UiErrorKeys,
) {
    debug!("net: removed spell {spell_id}");
    if actions.spells.remove(&spell_id) {
        actions.dirty = true;
    }
    announce_unlearn(spell_id, spells, errors);
}

/// `ERR_SPELL_UNLEARNED_S` (`0x5ea2ab push 0x14a`) with the bare name: `0x5ea2a3` pushes
/// `SpellRec+0x1e0` and this path has no rank composer. Which spells say it is
/// [`benilla_formats::SpellDisplay::announces_unlearn`]'s.
fn announce_unlearn(spell_id: u32, spells: Option<&Spells>, errors: &mut UiErrorKeys) {
    let Some(display) = spells.and_then(|s| s.catalog.get(spell_id)) else {
        return;
    };
    if !display.announces_unlearn() {
        return;
    }
    errors
        .0
        .push(UiError::s("ERR_SPELL_UNLEARNED_S", display.name.clone()));
}

/// `SMSG_SUPERCEDED_SPELL`: the new rank replaces the old in the book. The bar follows from the
/// book in `ui_action::ranks`, not from here: vmangos sends no fresh `SMSG_ACTION_BUTTONS`
/// (`Player::learnSpell`), and suppresses this packet while the character loads (`IsInWorld()`).
fn superceded_spell(
    old_spell_id: u32,
    new_spell_id: u32,
    actions: &mut PlayerActions,
    spells: Option<&Spells>,
    errors: &mut UiErrorKeys,
    tab_flash: &mut LearnedInTab,
) {
    debug!("net: superceded spell {old_spell_id} -> {new_spell_id}");
    actions.spells.remove(&old_spell_id);
    actions.spells.insert(new_spell_id);
    actions.dirty = true;
    // A rank-up announces once, like a first learn: the supersede pair `0x4b2f50` calls the
    // unlearn `0x4b2c50`, which prints nothing, then the registrar `0x4b25b0` with `edx = 1`
    // (`0x4b2f61`).
    announce_learn(new_spell_id, spells, errors);
    // The same flag flashes the tab of the new rank.
    tab_flash.0.push(new_spell_id);
}

/// The server's verdict on our cast (`SMSG_CAST_RESULT`, `HandleCastResult 0x6e7330`).
///
/// A failure goes through the reporter first (`0x6e73ac` → `ReportCastFailure 0x6e1a00` with its
/// server flag set): the tradeskill check, the auto-repeat cancel, the GCD clear, the line and its
/// log line, then its tail ([`super::CastCore::report`]), which cancels the spell in flight with
/// its failed flag and no packet, or fires the failure's event when nothing is in flight.
fn cast_result(
    spell_id: u32,
    success: bool,
    reason: Option<u8>,
    arg: Option<u32>,
    commands: &mut Commands,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    casting: &Query<&Casting>,
    cast_events: &mut MessageWriter<CastEvent>,
    core: &mut super::CastCore,
    cooldowns: &mut Cooldowns,
    auto_repeat: &mut AutoRepeatActive,
    spells: Option<&Spells>,
    items: Option<&crate::items::Items>,
    net: &crate::net::NetCommands,
    seq: u64,
) -> Option<u32> {
    debug!("net: cast result — spell {spell_id} success={success} reason={reason:?}");
    let now = Instant::now();
    if !success {
        // `HandleCastFailed 0x6e1a00` clears the GCD armed at send (`0x6e1d83 → 0x6e1630`), and the
        // bit-25 revert after it drops a parked record; the spell's own recovery starts at
        // SPELL_GO, which a failed cast never reaches. A failing cached auto-repeat spell with reason ≠ 0x17
        // runs the full local cancel (`0x6e1cd9`–`0x6e1cea` → `0x6ea080`, the
        // `SMSG_CANCEL_AUTO_REPEAT` routine). A deselect arrives as this failure (vmangos
        // `HandleSetSelectionOpcode` → `Spell::cancel` → `SendCastResult(INTERRUPTED)`); target
        // death arrives as [`cancel_auto_repeat`].
        // Reason 0x17 DONT_REPORT exits before the GCD clear and the display (`6e1ce1`/`6e1cf7` →
        // `0x6e224f`), and so before the report's tail: the handler's own pop `0x6e741a` finds the
        // spell still in flight. vmangos sends 0x17 for real aborts.
        let dont_report = reason == Some(0x17);
        let self_e = self_guid.0.and_then(|g| index.0.get(&g)).copied();
        if auto_repeat.0 == Some(spell_id) && !dont_report {
            crate::creature_anim::cancel_auto_repeat_local(self_e, auto_repeat, commands, net);
        }
        if !dont_report {
            cooldowns.clear_gcd(spell_id, now);
        }
        let fail = crate::ui_action::CastFail::server(spell_id, reason.unwrap_or(0), arg);
        let reaches = crate::ui_action::report_reaches_pop(
            fail.reason,
            arg,
            spells.and_then(|s| s.catalog.get(spell_id)),
            spells.map(|s| &s.forms),
            // The packet's lookup with the callback `0x6e29b0` (`0x6e1ea4`).
            |item| items.is_some_and(|i| i.template(item, 0, Ask::Event(net)).is_some()),
        );
        // The line, then the tail: the spell in flight popped with its failed flag and no packet
        // (`0x6e2213`), its event fired while the record's caster holds.
        core.report(fail, true, reaches);
        // The bit-25 revert (`0x6e73cc`-`0x6e73e6`), the handler's, after the report: every
        // failure but 0x3c of a cooldown-on-event spell removes its parked record (a failed Feign
        // Death, Stealth or Shadowform), 0x17 too, which leaves only the report early.
        if reason != Some(0x3c)
            && spells
                .and_then(|s| s.catalog.get(spell_id))
                .is_some_and(|d| d.cooldown_on_event())
        {
            cooldowns.clear_spell(spell_id);
        }
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV CAST_RESULT failure — spell {spell_id} reason={reason:?}; reaches_tail={reaches}");
        }
        // Our cast died (vmangos never sends `SMSG_SPELL_FAILURE`): end the self avatar's cast
        // state and precast hold, keyed by spell id.
        if let Some(e) = self_e {
            if casting.get(e).is_ok_and(|c| c.spell_id == spell_id) {
                commands.entity(e).remove::<Casting>();
            }
            cast_events.write(CastEvent {
                entity: e,
                spell_id,
                kind: CastEventKind::Fail,
                cast_flags: 0,
                item_cast: false,
                seq,
            });
        }
    }
    // The `modalNextSpell` chain (`HandleCastResult 0x6e7330`, `0x6e7408`–`0x6e74aa`): the reply
    // to our in-flight cast finishes its slot, then reads column 38 of its spell; non-zero and not
    // the running repeat, the client casts it at the null target guid. Every hunter shot names 75,
    // Auto Shot.
    // - A success chains: `0x6e7356 cmp [ebp+0xf],0x2` / `0x6e735a jne` sends it past the
    //   report straight to the slot block `0x6e73eb`.
    // - A failure chains only if the slot still holds the spell after its report: the report
    //   pops the in-flight slot (`0x6e2201` → `0x6e2218` → `0x6e4940` → `0x6e4ad0(0, 0)`, the
    //   save into the slot), so `0x6e7408` misses and `0x6e74bc` skips the chain. It holds when
    //   the report left before that pop (`report_reaches_pop`: reason 0x17, a form-less 0x56, an
    //   uncached 0x5c or 0x78 item).
    // - The slot clears first (`0x6e741a`), or the IsCasting rung (`0x6e4d97`, our `0x61`) would
    //   refuse the chain. vmangos sends `SMSG_CAST_RESULT` before `SMSG_SPELL_GO` (`Spell::cast`,
    //   `Spell.cpp:3669`, then `3703` or `handle_immediate`'s `3771`), so the guard is still armed.
    // - Equal to the running repeat means re-arm, not re-cast (`0x6e745b` → `0x6e745d`), so a
    //   second sting never resets the swing timer; the record is refilled with the player as its
    //   caster (`0x6e748f`) and nothing is sent.
    // The chained spell is returned; `on_cast_result` hands it to the ladder in the same call.
    //
    // The handler's own `SPELLCAST_STOP` (0x152), at its two sites, never from the GO (`0x6e7a70`
    // calls neither):
    // - the slot's own pop, `0x6e741a` → `0x6e4940(0, 0, 0x1c)`, while the caster word is the
    //   player's: a success, or a failure whose report left the spell in flight. Every cast holds
    //   the slot until here, an instant, a channel and a `ITEM_FLAG_PLAYERCAST` item too, so each
    //   gets its STOP, before its GO, but an auto-repeat spell, which its send popped
    //   (`0x6e5989`);
    // - `0x6e74cb`, a spell not in the slot: when nothing is in flight (`0x6e3d30`) and the status
    //   is not 2 (`0x6e74c5`), with no caster test; the use of an item without the flag lands here.
    // The save `0xcecaa8` is cleared first when it holds the spell (`0x6e73ee`): a queued strike
    // a cast or a cursor sits over dies here, as the server's melee-slot interrupt arrives as a
    // failure. A strike in the slot is the slot's pop's, and a failing one a cast popped back over
    // pops twice: the report's pop copies the save back over it, this pop empties it.
    core.queued_melee.clear_save_if(spell_id);
    let slot = core.slot();
    let slot_holds = slot.map(super::inflight::Inflight::spell_id) == Some(spell_id);
    if slot_holds {
        core.cancel(false, false, super::REASON_ERROR);
    } else if success && slot.is_none() {
        core.reports.bar(CastBarEdge::Stop);
    }
    if *crate::net::CAST_TRACE {
        info!("cast-trace: RECV CAST_RESULT — spell {spell_id} success={success}; in_slot={slot_holds}");
    }
    if slot_holds {
        if let Some(next) = spells
            .and_then(|s| s.catalog.get(spell_id))
            .map(|d| d.modal_next_spell)
            .filter(|&next| next != 0)
        {
            if Some(next) == auto_repeat.0 {
                core.pending.fill_record();
            } else {
                debug!("net: cast result {spell_id} chains modalNextSpell {next}");
                return Some(next);
            }
        }
    }
    None
}

/// `SMSG_SPELL_START`: a unit began a non-triggered cast, instants included (`cast_time_ms == 0`).
fn spell_start(
    caster: u64,
    spell_id: u32,
    cast_flags: u16,
    cast_time_ms: u32,
    target: Option<u64>,
    ammo_display_id: Option<u32>,
    commands: &mut Commands,
    index: &GuidIndex,
    cast_events: &mut MessageWriter<CastEvent>,
    self_guid: &SelfGuid,
    reports: &mut CastReports,
    pending: &mut PendingCast,
    spells: Option<&crate::ui_action::Spells>,
    seq: u64,
) {
    // A nonzero cast time marks the caster `Casting`; an instant gets none, its GO follows at once.
    debug!(
        "net: spell start {spell_id} by {caster:#x} ({cast_time_ms}ms, flags \
         {cast_flags:#x}, target {target:?}, ammo {ammo_display_id:?})"
    );
    // The nocked-ammo refresh (`0x60ba30` at `0x6e78b6`, gated at `0x6e78a1` on `SpellRec+0x20 &
    // 0x20` or `+0x18 & 0x2`, for any caster): a ranged START attaches the wire's ammo display id,
    // or detaches without one. The model persists through Load/Hold and the fire clip.
    if spells
        .and_then(|s| s.catalog.get(spell_id))
        .is_some_and(|d| d.ranged_attack())
    {
        if let Some(&e) = index.0.get(&caster) {
            match ammo_display_id.filter(|id| *id != 0) {
                Some(display_id) => {
                    commands
                        .entity(e)
                        .insert(crate::creature_anim::NockedAmmo { display_id });
                }
                None => {
                    commands
                        .entity(e)
                        .remove::<crate::creature_anim::NockedAmmo>();
                }
            }
        }
    }
    // SPELLCAST_START fires only on `cast_time > 0 && !(SpellRec+0x18 & 2)` (`0x6e7700`): a
    // ranged-slot spell never opens a bar. vmangos pads a non-auto-repeat ranged cast by 500 ms
    // (`SpellEntry::GetCastTime`), so Throw arrives as a 500 ms cast with no bar. An unknown spell
    // keeps the bar.
    let ranged_slot = spells
        .and_then(|s| s.catalog.get(spell_id))
        .is_some_and(|d| d.ranged_slot());
    if *crate::net::CAST_TRACE && self_guid.0 == Some(caster) {
        info!(
            "cast-trace: RECV SPELL_START — spell {spell_id} cast_time={cast_time_ms}ms \
             (bar {})",
            if ranged_slot {
                "suppressed: ranged"
            } else {
                "opens"
            }
        );
    }
    // Our own timed cast opens the bar.
    if self_guid.0 == Some(caster) && cast_time_ms > 0 {
        if !ranged_slot {
            reports.bar(CastBarEdge::Start {
                spell_id,
                cast_time_ms,
            });
        }
        // The real cast time replaces the guard's send-time deadline, ranged casts included.
        pending.refine(cast_time_ms, Instant::now());
    }
    if let Some(&e) = index.0.get(&caster) {
        if cast_time_ms > 0 {
            commands.entity(e).insert(Casting {
                spell_id,
                until: Some(Instant::now() + Duration::from_millis(u64::from(cast_time_ms))),
            });
        }
        // The precast edge, instants included: their GO follows at once and reaps the hold.
        cast_events.write(CastEvent {
            entity: e,
            spell_id,
            kind: CastEventKind::Start,
            cast_flags: 0,
            item_cast: false,
            seq,
        });
    }
}

/// `GAMEOBJECT_TYPE_ID` 3, `CHEST`: the only GameObject type whose `OPEN_LOCK` cast arms the
/// loot-target latch (`0x6e830c`).
const GO_TYPE_CHEST: i32 = 3;

/// `SMSG_SPELL_GO`: the cast launched. The server schedules damage off `Spell.dbc` Speed; the
/// packet carries the hit and miss lists and the ammo display id, and the flight is rebuilt from
/// the same Speed column ([`SpellGoTargets`]).
fn spell_go(
    caster: u64,
    spell_id: u32,
    cast_flags: u16,
    hits: Vec<u64>,
    misses: Vec<(u64, u8)>,
    target: Option<u64>,
    go_target: Option<u64>,
    dest: Option<[f32; 3]>,
    ammo_display_id: Option<u32>,
    item_caster: Option<u64>,
    commands: &mut Commands,
    index: &GuidIndex,
    casting: &Query<&Casting>,
    cast_events: &mut MessageWriter<CastEvent>,
    go_targets: &mut MessageWriter<SpellGoTargets>,
    self_guid: &SelfGuid,
    stores: &Query<&mut ObjectStore>,
    text: &mut MessageWriter<crate::combat_text::CombatTextSpawn>,
    // The floating-text CVars, read inside the word emitter `0x607140`: they gate miss words too.
    text_gates: crate::combat_text::DamageTextGates,
    go_lid: &mut MessageWriter<crate::go_anim::GoLidOpen>,
    // The loot-target latch, armed here for a chest (`0x6e831b`).
    loot_latch: &mut crate::ui_loot::LootLatch,
    // The cooldown store and what it reads; the last member is the pet's, the second bank this
    // handler inserts into ([`pet_go_cooldown`]).
    cooldown_ctx: (
        &mut Cooldowns,
        Option<&Spells>,
        &mut crate::items::Items,
        &crate::net::NetCommands,
        &mut crate::ui_pet::PetBar,
        &crate::spell::SpellModifiers,
    ),
    // The GO-deferred auto-attack start's writes (`0x6e83c0`) and the attack lock it gates on
    // (`Engaged`, the reference's `[player+0xc48]`).
    attack_ctx: (
        &mut crate::spell::AutoRepeatActive,
        &mut MessageWriter<crate::creature_anim::SheathRequest>,
        bool,
    ),
    seq: u64,
) {
    debug!(
        "net: spell go {spell_id} by {caster:#x} ({} hit(s), {} miss(es), flags \
         {cast_flags:#x}, target {target:?}, go_target {go_target:?}, dest {dest:?}, \
         ammo {ammo_display_id:?})",
        hits.len(),
        misses.len()
    );
    // The `use` trace's third link, after `target::click`: a caster we never streamed drops every
    // visual below. A SPELLCASTER GameObject (type 22) is its own caster; vmangos writes that guid
    // as 0 and the decode resolves it (`benilla_protocol::events`' `spell_caster`).
    if benilla_assets::trace::enabled_for("use") {
        benilla_assets::trace::line(
            "use",
            &format!(
                "SPELL_GO spell={spell_id} caster={caster:#x} caster_indexed={} hits={} misses={}",
                index.0.contains_key(&caster),
                hits.len(),
                misses.len()
            ),
        );
    }
    // A cast naming a GameObject goes to the GameObject animation driver, which opens the lid on
    // an open-lock cast, streamed caster or not.
    if let Some(go_guid) = go_target {
        go_lid.write(crate::go_anim::GoLidOpen { go_guid, spell_id });
    }
    let (cooldowns, spells, items, net_commands, pet_bar, spell_mods) = cooldown_ctx;
    let (auto_repeat, sheath, engaged) = attack_ctx;
    let now = Instant::now();
    let display = spells.and_then(|s| s.catalog.get(spell_id));
    // A chest becomes the loot target at this packet, not at the loot response: `HandleSpellGo
    // 0x6e7a70` reaches `0x6e831b call SetLootTarget 0x5ed5f0`, which writes `[player+0x1d28]` and
    // plays Loot 50. Gates: the local player cast it (`0x6e81b6`), the spell has `OPEN_LOCK`
    // (0x21) or `OPEN_LOCK_ITEM` (0x3b) (`0x6e81f9`/`0x6e8202`), and the target is a CHEST
    // (`0x6e82df`–`0x6e830c`). The reference reads the target off a one-entry hit list
    // (`0x6e82c1`); vmangos sends a GameObject target in `SpellCastTargets`, so this reads
    // `go_target`, a single guid: the same condition. The pose needs no kick:
    // [`crate::ui_loot::LootKneel`] recomputes every frame.
    if self_guid.0 == Some(caster) {
        if let Some(go_guid) = go_target {
            let is_open_lock = display.is_some_and(|d| d.open_lock.is_some());
            let is_chest = index
                .0
                .get(&go_guid)
                .and_then(|&e| stores.get(e).ok())
                .is_some_and(|store| store.0.gameobject_type_id() == GO_TYPE_CHEST);
            if is_open_lock && is_chest {
                debug!("net: spell go — chest {go_guid:#x} becomes the loot target (kneel arm)");
                loot_latch.0 = Some(go_guid);
            }
        }
    }
    // Our own launch. The bar's STOP is not fired here: `0x6e7a70` reaches neither 0x152 site,
    // and the cast result ahead of the GO fired it ([`cast_result`]).
    if self_guid.0 == Some(caster) {
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV SPELL_GO — spell {spell_id} (self)");
        }
        // The in-flight slot is not touched here, a queued strike's included: `0x6e7a70` reads
        // neither `0xceca88` nor its save, and only the spell's `SMSG_CAST_RESULT` pops it
        // (`0x6e7408` → `0x6e741a`), which vmangos sends ahead of the GO (`Spell::cast`).

        // The GO-deferred auto-attack start (`0x6e83c0`), the complement of
        // [`crate::spell::cast_send`]'s send-time tail: an `AttributesEx2 & 0x100000` spell (the
        // stealth openers and Judgement, 36 rows) starts the swing once the server confirms it.
        // The target is `hits[0]`, else the null pair (`0x6e83e9`–`0x6e83fe`), which `0x612df0`
        // resolves to the selection or else the nearest hostile. Deviation: the fallback is the
        // packet's target guid, never the nearest-hostile acquire, because that would attack
        // something the player never named. Gated on `[player+0xc48] == 0` (`0x6e83e7`).
        if display.is_some_and(|d| d.initiates_auto_attack_at_go()) && !engaged {
            if let (Some(&me), Some(guid)) =
                (index.0.get(&caster), hits.first().copied().or(target))
            {
                debug!("net: spell go {spell_id} — deferred auto-attack start at {guid:#x}");
                crate::creature_anim::start_attack_local(
                    me,
                    guid,
                    engaged,
                    false,
                    auto_repeat,
                    sheath,
                    commands,
                    net_commands,
                );
            }
        }

        // Our launch starts the cast's cooldown here, at the GO receive time. The self-insert
        // forks on itemCaster == caster: the spell leg (`0x6e8498`: RecoveryTime, Category,
        // CategoryRecoveryTime, on-hold from Attributes bit 25) or the item leg (`0x6e8566`,
        // per-slot values with SpellRec fallbacks; a non-resident row pends via `0x6e8660` →
        // `0x6e8830`). `SMSG_SPELL_COOLDOWN` is the server's override; vmangos sends none for a
        // plain cast. A ranged-slot cast folds `UNIT_FIELD_RANGEDATTACKTIME` into the category
        // recovery (`0x6e2b60`).
        let ranged_ms = display
            .filter(|d| d.ranged_speed_cooldown())
            .and_then(|_| {
                let e = index.0.get(&caster)?;
                stores.get(*e).ok()?.0.unit_ranged_attack_time()
            })
            .unwrap_or(0);
        // The item's entry, off its own store.
        match item_caster.and_then(|g| stores.get(*index.0.get(&g)?).ok()?.0.object_entry()) {
            Some(entry) => {
                // The item's record, which its create asked.
                let use_spell = items
                    .template(entry, 0, Ask::Read)
                    .and_then(|t| t.use_spell)
                    .filter(|u| u.spell_id == spell_id);
                match use_spell {
                    Some(u) => cooldowns.start_item(entry, &u, display, now),
                    // Template not streamed, or naming another spell: the spell-keyed record,
                    // unmodified, as the item leg never applies op 11 (`0x6e8566`, `0x6e8830`).
                    None => {
                        if let Some(d) = display {
                            cooldowns.start_spell(spell_id, d, ranged_ms, now, None);
                        }
                    }
                }
            }
            None => {
                if let Some(d) = display {
                    cooldowns.start_spell(spell_id, d, ranged_ms, now, Some(spell_mods));
                }
            }
        }
        if benilla_assets::trace::enabled() {
            if let Some(d) = display {
                let (recovery_ms, category_ms) = spell_mods.spell_cooldowns(d, ranged_ms);
                benilla_assets::trace::line(
                    "cd",
                    &format!(
                        "arm spell={spell_id} rec={}ms cat={}:{}ms (GO self-insert)",
                        recovery_ms, d.category, category_ms
                    ),
                );
            }
        }
    }
    // The pet leg of the same insert: an independent `if`, not an else.
    if let Some(d) = display.filter(|_| pet_go_cooldown(caster, self_guid, index, stores)) {
        // No ranged pad: `0x6e2b60` is called on the self leg only (`0x6e845d`). Op 11 still
        // applies to both columns (`0x6e85c5`, `0x6e85d1`) off the player's tables, which gate
        // on the spell's family and never the caster (`0x6e6b30`).
        pet_bar
            .cooldowns
            .start_spell(spell_id, d, 0, now, Some(spell_mods));
        // The flush that follows (`0x6e85fc` `0x4b31b0`, `0x6e8601` `0x4bce90`) fires off the
        // list's generation, which the insert bumped: `ACTIONBAR_UPDATE_COOLDOWN`,
        // `SPELL_UPDATE_COOLDOWN`, `PET_BAR_UPDATE_COOLDOWN`, and no `PET_BAR_UPDATE`.
        if benilla_assets::trace::enabled() {
            let (recovery_ms, category_ms) = spell_mods.spell_cooldowns(d, 0);
            benilla_assets::trace::line(
                "cd",
                &format!(
                    "arm spell={spell_id} rec={}ms cat={}:{}ms (GO pet-insert)",
                    recovery_ms, d.category, category_ms
                ),
            );
        }
    }
    // The miss list's floating words (`0x6e7a70`): one word over each missed target, except
    // REFLECT, which anchors on the caster (`0x6e7e51`). Another caster's misses draw nothing.
    // `0x6e7d4e fld [SpellRec+0x94]; fcomp 0.0; test ah,0x44; jp 0x6e7e71` skips this emit for a
    // nonzero Speed: a travelling spell's word floats on arrival
    // ([`crate::entities::MissileMiss`]).
    // `0x6e7d73`/`0x6e7dcc` push the resolved SpellRec, so the word takes a number's colour (a
    // bit-15-clear spell's "Miss" is spell gold), and the CVar gates in `0x607140` apply.
    if !misses.is_empty() && display.is_none_or(|d| d.speed == 0.0) {
        if let Some(color) = crate::combat_log::text::classify_source(
            caster, index, self_guid, stores,
        )
        .and_then(|source| {
            crate::combat_text::damage_color(
                text_gates,
                source,
                crate::combat_text::melee_styled(display),
            )
        }) {
            for &(guid, code) in &misses {
                let anchor_guid = if code == 11 { caster } else { guid };
                if self_guid.0 == Some(anchor_guid) {
                    continue;
                }
                if let (Some(&anchor), Some((word, category))) = (
                    index.0.get(&anchor_guid),
                    crate::combat_text::miss_word(code),
                ) {
                    text.write(crate::combat_text::CombatTextSpawn {
                        anchor,
                        text: word.to_string(),
                        category,
                        color,
                    });
                }
            }
        }
    }
    // Keyed by spell id, like the reap `0x614150(spellId, 0)`: a proc's GO must not clear another
    // spell's precast state.
    if let Some(&e) = index.0.get(&caster) {
        if casting.get(e).is_ok_and(|c| c.spell_id == spell_id) {
            commands.entity(e).remove::<Casting>();
        }
        // The GO's chain-hop fill (`0x6e800d`), the producer for non-channelled chain spells. It
        // must precede the CastEvent: the cast kit's chain proc consumes it the same frame. The
        // reference gates it on the attribute predicate `0x6e4870`, whose role here is untraced;
        // this fills unconditionally, harmless since consumption needs a chain proc and every fill
        // clears.
        fill_chain_hops(caster, e, &hits, commands, index);
        cast_events.write(CastEvent {
            entity: e,
            spell_id,
            kind: CastEventKind::Go,
            cast_flags,
            item_cast: item_caster.is_some(),
            seq,
        });
        // Targets not streamed to us drop out. The miss code picks the victim's dodge or block
        // clip on the missile's arrival.
        let hits: Vec<Entity> = hits
            .iter()
            .filter_map(|g| index.0.get(g).copied())
            .collect();
        let misses: Vec<(Entity, u8)> = misses
            .iter()
            .filter_map(|&(g, code)| index.0.get(&g).map(|&e| (e, code)))
            .collect();
        // A pure dest cast (ground AoE, empty lists) rides the same message: the point is a target.
        let dest = dest.map(benilla_assets::coords::wow_to_bevy);
        if !hits.is_empty() || !misses.is_empty() || dest.is_some() {
            go_targets.write(SpellGoTargets {
                caster: e,
                spell_id,
                hits,
                misses,
                dest,
                ammo_display_id,
                seq,
            });
        }
    }
}

/// `SMSG_SPELL_FAILED_OTHER`: a cast was interrupted; ends the caster's `Casting` as [`spell_go`]
/// does.
fn spell_failed_other(
    caster: u64,
    spell_id: u32,
    commands: &mut Commands,
    index: &GuidIndex,
    casting: &Query<&Casting>,
    cast_events: &mut MessageWriter<CastEvent>,
    self_guid: &SelfGuid,
    seq: u64,
) {
    debug!("net: spell failed (other) {spell_id} by {caster:#x}");
    // No event and no slot: the handler (`0x6e8e40`) stops the visual (`0x60d040`) and reaps
    // (`0x614150`). Our bar's red "Interrupted" and a queued strike's pop are the failing
    // `SMSG_CAST_RESULT`'s report, which vmangos sends after it (`Spell::cancel`).
    if self_guid.0 == Some(caster) && *crate::net::CAST_TRACE {
        info!("cast-trace: RECV SPELL_FAILED_OTHER — spell {spell_id} (self)");
    }
    // Keyed by spell id, like the reap (the 0x2A6 handler's `0x614150(spellId, 0)`).
    if let Some(&e) = index.0.get(&caster) {
        if casting.get(e).is_ok_and(|c| c.spell_id == spell_id) {
            commands.entity(e).remove::<Casting>();
        }
        cast_events.write(CastEvent {
            entity: e,
            spell_id,
            kind: CastEventKind::Fail,
            cast_flags: 0,
            item_cast: false,
            seq,
        });
    }
}

/// `SMSG_SPELL_DELAYED`: pushback. A hit on our cast extends its timer by `delay_ms` (vmangos
/// `Spell::Delayed`), and the bar's window slides out by the same (`SPELLCAST_DELAYED`). Sent
/// only to the caster, but gated on the caster guid like every own-cast edge.
fn spell_delayed(
    caster: u64,
    delay_ms: u32,
    self_guid: &SelfGuid,
    reports: &mut CastReports,
    pending: &mut PendingCast,
) {
    debug!("net: spell delayed by {caster:#x} (+{delay_ms}ms pushback)");
    if self_guid.0 == Some(caster) {
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV SPELL_DELAYED — +{delay_ms}ms pushback (bar extends, does NOT vanish)");
        }
        reports.bar(CastBarEdge::Delayed { delay_ms });
        // The in-flight guard holds past the stretched end.
        pending.delay(delay_ms, Instant::now());
    }
}

/// `SMSG_CANCEL_AUTO_REPEAT`: the handler (`0x6e99d0` → `0x6ea080`) clears the autorepeat key
/// `0xceac30`, which the button's checked state reads (`STOP_AUTOREPEAT_SPELL`), and both
/// shooting-idle bits of `[+0xd58]`, so the Load/Hold idle drops
/// ([`crate::creature_anim::AutoRepeatArmed`] and `RangedHold`). Nothing sheathes.
///
/// vmangos sends it on every player autorepeat interrupt (`SpellCaster.cpp:1826`), which is how
/// target death stops a volley (`Unit::_UpdateAutoRepeatSpell` → `CheckCast(true)` fails →
/// `InterruptSpell`). Moving interrupts only Category 351, the wand, so Auto Shot stays armed
/// across a run.
fn cancel_auto_repeat(
    auto_repeat: &mut AutoRepeatActive,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    debug!("net: cancel auto-repeat");
    // The same cancel `0x6ea080` as every local trigger, its unconditional CMSG ack included.
    let self_e = self_guid.0.and_then(|g| index.0.get(&g)).copied();
    crate::creature_anim::cancel_auto_repeat_local(self_e, auto_repeat, commands, net);
}

/// Whether this `SMSG_SPELL_GO` arms the pet's cooldown bank (`0x6e857a`–`0x6e85ad`). The GO
/// handler inserts into two banks (`0x6e2ea0`'s `bankHead = 0xcecaec + 24*bank`): the self leg
/// bank 0 (`0x6e8493`), this leg bank 1 (`0x6e85f2 mov ecx, 0xcecb04`), which `SMSG_PET_SPELLS`
/// seeds (`0x4bdaa8 push 1`) and `GetPetActionCooldown` reads.
///
/// The gate is the caster's owner off its own descriptor: `charmedBy` when set, else
/// `summonedBy` (`0x6e858b`–`0x6e859a`), compared with the active player's guid (`0x468550`,
/// `0x6e85a2`). Not `0x5ee5a0`'s `createdBy` fallback, which would arm it for totems.
///
/// vmangos sends no cooldown packet for a pet's cast: `Creature::AddCooldown`
/// (`Creature.cpp:3259-3282`) sends one only for a charmed non-pet's instant.
fn pet_go_cooldown(
    caster: u64,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    stores: &Query<&mut ObjectStore>,
) -> bool {
    let Some(me) = self_guid.0 else {
        return false;
    };
    index
        .0
        .get(&caster)
        .and_then(|&e| stores.get(e).ok())
        .and_then(|s| s.0.unit_owner(benilla_protocol::OwnerFallback::SummonedBy))
        == Some(me)
}

/// `SMSG_SPELL_COOLDOWN` (`0x6e9460`): server-pushed cooldowns (school lockouts, the pet's list),
/// into the store [`addressed_store`] picked.
fn spell_cooldowns(
    caster: u64,
    pairs: Vec<(u32, u32)>,
    spells: Option<&Spells>,
    cooldowns: &mut Cooldowns,
) {
    debug!(
        "net: spell cooldowns for {caster:#x} — {} pair(s)",
        pairs.len()
    );
    let now = Instant::now();
    for (spell_id, cooldown_ms) in pairs {
        let display = spells.and_then(|s| s.catalog.get(spell_id));
        cooldowns.apply_wire_cooldown(spell_id, cooldown_ms, display, now);
    }
}

/// `SMSG_ITEM_COOLDOWN` (`0x6e95d0`): the fixed 30 s use cooldown, keyed on the item instance's
/// template entry, as the reference resolves it.
fn item_cooldown(
    item_guid: u64,
    spell_id: u32,
    index: &GuidIndex,
    stores: &Query<&mut ObjectStore>,
    cooldowns: &mut Cooldowns,
) {
    debug!("net: item cooldown — item {item_guid:#x} spell {spell_id}");
    let entry = index
        .0
        .get(&item_guid)
        .and_then(|&e| stores.get(e).ok())
        .and_then(|s| s.0.object_entry());
    if let Some(entry) = entry {
        cooldowns.apply_wire_item_cooldown(entry, spell_id, Instant::now());
    }
}

/// `SMSG_COOLDOWN_EVENT` (`0x6e9670` → `0x6e3050(force=0)`): start an on-hold cooldown's parked
/// timers now (Stealth ends, Feign Death drops).
fn cooldown_event(spell_id: u32, caster: u64, cooldowns: &mut Cooldowns) {
    debug!("net: cooldown event — spell {spell_id} on {caster:#x}");
    cooldowns.cooldown_event(spell_id, Instant::now());
}

/// `SMSG_CLEAR_COOLDOWN` (`0x6e9670` → `0x6e3050(force=1)`): remove the spell's record outright.
fn clear_cooldown(spell_id: u32, caster: u64, cooldowns: &mut Cooldowns) {
    debug!("net: clear cooldown — spell {spell_id} on {caster:#x}");
    cooldowns.clear_spell(spell_id);
}

/// `SMSG_COOLDOWN_CHEAT` (`0x6e9730` → `0x6e9700`): the GM reset wipes the self or pet list the
/// guid names.
fn cooldown_cheat(caster: u64, cooldowns: &mut Cooldowns) {
    debug!("net: cooldown cheat (wipe) for {caster:#x}");
    cooldowns.wipe();
}

/// `MSG_CHANNEL_START`: self-only on the wire (no guid), so it goes straight to the cast bar; the
/// channel animation rides the unit-field pair instead.
fn channel_start(
    spell_id: u32,
    duration_ms: u32,
    channel: &mut ActiveChannel,
    reports: &mut CastReports,
) {
    channel.start(spell_id, duration_ms, Instant::now());
    reports.bar(CastBarEdge::ChannelStart {
        spell_id,
        duration_ms,
    });
}

/// `MSG_CHANNEL_UPDATE`: the running channel's remaining time (`0` is its stop edge).
fn channel_update(remaining_ms: u32, channel: &mut ActiveChannel, reports: &mut CastReports) {
    channel.update(remaining_ms, Instant::now());
    reports.bar(CastBarEdge::ChannelUpdate { remaining_ms });
}

/// `SMSG_UPDATE_AURA_DURATION`: one of our auras' remaining time, by raw slot, stamped with the
/// receive time. It arrives before the descriptor delta that names the slot.
fn aura_duration(slot: u8, remaining_ms: i32, durations: &mut AuraDurations, now_secs: f64) {
    durations.set(slot, remaining_ms, now_secs);
}

/// `SMSG_SET_FLAT_SPELL_MODIFIER` / `SMSG_SET_PCT_SPELL_MODIFIER` (`HandleSetSpellModifier
/// 0x6e9950`): one store of the absolute value for a `(family bit, op)` pair, never a delta. The
/// out-of-range refusal is [`crate::spell::SpellModifiers::set`]'s.
fn set_spell_modifier(
    flat: bool,
    mask_bit: u8,
    op: u8,
    value: i32,
    mods: &mut crate::spell::SpellModifiers,
) {
    mods.set(flat, mask_bit, op, value);
}

/// The caster's chain-hop array from a wire target list, as `0x605780`: clear, then fill,
/// skipping the unit's own guid (`0x6057bf`/`0x6057c9`); unstreamed targets drop out. Its two
/// callers are the `SMSG_SPELL_UPDATE_CHAIN_TARGETS` handler and `HandleSpellGo`.
fn fill_chain_hops(
    caster: u64,
    caster_entity: Entity,
    targets: &[u64],
    commands: &mut Commands,
    index: &GuidIndex,
) {
    let hops: Vec<Entity> = targets
        .iter()
        .filter(|&&g| g != caster)
        .filter_map(|g| index.0.get(g).copied())
        .collect();
    commands
        .entity(caster_entity)
        .try_insert(crate::entities::ChainHops(hops));
}

/// `SMSG_SPELL_UPDATE_CHAIN_TARGETS`: the hop list a beam runs through, parked on the caster
/// (`unit+0xd44`: capacity `+0xd44`, count `+0xd48`, data `+0xd4c`, quantum `+0xd50`) and consumed
/// once by the next chain `CharProc` (`0x60db72` zeroes the count). vmangos sends it only for
/// channelled spells (`Spell::SendChannelStart`); other chain spells fill from the GO (`0x6e800d`).
fn spell_chain_targets(
    caster: u64,
    spell_id: u32,
    targets: Vec<u64>,
    commands: &mut Commands,
    index: &GuidIndex,
) {
    debug!(
        "net: chain targets for spell {spell_id} by {caster:#x} — {} hop(s)",
        targets.len()
    );
    if let Some(&e) = index.0.get(&caster) {
        fill_chain_hops(caster, e, &targets, commands, index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::QueuedMeleeSpell;
    use benilla_protocol::messages::ACTION_KIND_SPELL;

    /// `SMSG_COOLDOWN_EVENT` (`0x6e9670`): the event moves the held record's one start, which the
    /// GCD leg reads (`0x6e15d8`), so the global cooldown runs again from the packet.
    #[test]
    fn a_cooldown_event_packet_restarts_the_held_records_gcd() {
        let form = benilla_formats::SpellDisplay {
            start_recovery_category: 133,
            start_recovery_ms: 1_500,
            attributes: 0x0200_0000,
            ..Default::default()
        };
        let t0 = Instant::now();
        let mut cooldowns = Cooldowns::default();
        cooldowns.start_gcd(15473, &form, t0, &crate::spell::SpellModifiers::default());
        std::thread::sleep(Duration::from_millis(40));
        cooldown_event(15473, 10, &mut cooldowns);

        // 20 ms past the original GCD's end: only a restart from the packet is still running.
        let info = cooldowns.info(15473, 0, Some(&form), t0 + Duration::from_millis(1_520));
        assert!(info.remaining_ms >= 10, "the GCD runs from the event");
        assert!(info.start > t0, "and reads the event's start");
    }

    /// The bit-25 revert (`0x6e73cc`-`0x6e73e6`) runs after the report for every failure but
    /// 0x3c, so for 0x17 too, which leaves only the report early (`0x6e1ce4`): the record the
    /// send parked (`StartGlobalCooldown 0x6e2de0`, held from bit 25) goes, and the spell is
    /// usable again; a NOT_READY failure keeps it held.
    #[test]
    fn a_failed_cooldown_on_event_spell_drops_its_held_record_unless_not_ready() {
        const SHADOWFORM: u32 = 15473;
        let form = || benilla_formats::SpellDisplay {
            start_recovery_category: 133,
            start_recovery_ms: 1_500,
            attributes: 0x0200_0000,
            ..Default::default()
        };
        let form_row = form();
        for (reason, held) in [(0x17, false), (0x0a, false), (0x3c, true)] {
            let t0 = Instant::now();
            let mut s = CastSeq::new();
            s.app.insert_resource(catalog_with(SHADOWFORM, form()));
            s.app.world_mut().resource_mut::<Cooldowns>().start_gcd(
                SHADOWFORM,
                &form_row,
                t0,
                &crate::spell::SpellModifiers::default(),
            );
            s.commit(SHADOWFORM, None);
            s.result(SHADOWFORM, Some(reason));
            let info = s.app.world().resource::<Cooldowns>().info(
                SHADOWFORM,
                0,
                Some(&form_row),
                t0 + Duration::from_secs(3),
            );
            assert_eq!(!info.enabled, held, "reason {reason:#04x}");
        }
    }

    /// A catalog holding one row, so the announce tests can name a real `Attributes` value.
    fn catalog_with(id: u32, display: benilla_formats::SpellDisplay) -> Spells {
        let mut spells = Spells::empty_for_tests();
        spells.catalog =
            benilla_formats::SpellCatalog::from_displays([(id, display)].into_iter().collect());
        spells
    }

    /// Heroic Strike carries `SPELL_ATTR_ABILITY`, so `0x4b29b3 add eax,0x37` lands on `0x38`, the
    /// ability wording, with the argText `"%s (%s)"`.
    #[test]
    fn a_learned_ability_announces_the_ability_line_with_its_rank() {
        let spells = catalog_with(
            78,
            benilla_formats::SpellDisplay {
                name: "Heroic Strike".to_string(),
                rank: Some("Rank 1".to_string()),
                attributes: 0x10,
                ..Default::default()
            },
        );
        let mut actions = PlayerActions::default();
        let mut errors = UiErrorKeys::default();
        let mut flash = LearnedInTab::default();
        learned_spell(78, &mut actions, Some(&spells), &mut errors, &mut flash);

        assert_eq!(errors.0.len(), 1, "one line, once");
        assert_eq!(errors.0[0].key, "ERR_LEARN_ABILITY_S");
        assert_eq!(errors.0[0].arg_s(), Some("Heroic Strike (Rank 1)"));
        // The same live-learn flag gates the tab flash.
        assert_eq!(flash.0, vec![78], "queued for LEARNED_SPELL_IN_TAB");
    }

    /// A plain row is a spell; a tradeskill row is a recipe whose argText drops the rank (it
    /// returns from `0x4b2944` before the subtext composer).
    #[test]
    fn the_spell_and_recipe_arms_pick_their_own_key_and_argument() {
        let spells = catalog_with(
            133,
            benilla_formats::SpellDisplay {
                name: "Fireball".to_string(),
                rank: Some("Rank 1".to_string()),
                attributes: 0x10000,
                ..Default::default()
            },
        );
        let mut errors = UiErrorKeys::default();
        learned_spell(
            133,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
            &mut LearnedInTab::default(),
        );
        assert_eq!(errors.0[0].key, "ERR_LEARN_SPELL_S");
        assert_eq!(errors.0[0].arg_s(), Some("Fireball (Rank 1)"));

        let spells = catalog_with(
            2550,
            benilla_formats::SpellDisplay {
                name: "Cooking".to_string(),
                rank: Some("Apprentice".to_string()),
                attributes: 0x20,
                ..Default::default()
            },
        );
        let mut errors = UiErrorKeys::default();
        learned_spell(
            2550,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
            &mut LearnedInTab::default(),
        );
        assert_eq!(errors.0[0].key, "ERR_LEARN_RECIPE_S");
        assert_eq!(
            errors.0[0].arg_s(),
            Some("Cooking"),
            "the recipe arm pushes the bare name"
        );
    }

    /// The supersede pair `0x4b2f50` reaches the registrar with `edx = 1`: one line, naming the new
    /// rank.
    #[test]
    fn a_rank_up_announces_the_new_rank_once() {
        let mut spells = Spells::empty_for_tests();
        spells.catalog = benilla_formats::SpellCatalog::from_displays(
            [
                (
                    78,
                    benilla_formats::SpellDisplay {
                        name: "Heroic Strike".to_string(),
                        rank: Some("Rank 1".to_string()),
                        attributes: 0x10,
                        ..Default::default()
                    },
                ),
                (
                    284,
                    benilla_formats::SpellDisplay {
                        name: "Heroic Strike".to_string(),
                        rank: Some("Rank 2".to_string()),
                        attributes: 0x10,
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        );
        let mut actions = PlayerActions::default();
        actions.spells.insert(78);
        let mut errors = UiErrorKeys::default();
        let mut flash = LearnedInTab::default();
        superceded_spell(
            78,
            284,
            &mut actions,
            Some(&spells),
            &mut errors,
            &mut flash,
        );

        assert_eq!(errors.0.len(), 1, "one line for a rank-up, not two");
        assert_eq!(
            flash.0,
            vec![284],
            "the NEW rank's tab flashes, not the old one's"
        );
        assert_eq!(errors.0[0].key, "ERR_LEARN_ABILITY_S");
        assert_eq!(errors.0[0].arg_s(), Some("Heroic Strike (Rank 2)"));
    }

    /// A `DO_NOT_DISPLAY` row (languages, weapon proficiencies) and an unknown id are silent.
    #[test]
    fn a_do_not_display_spell_and_an_unknown_id_announce_nothing() {
        let spells = catalog_with(
            668,
            benilla_formats::SpellDisplay {
                name: "Language: Common".to_string(),
                attributes: 0xC0,
                ..Default::default()
            },
        );
        let mut errors = UiErrorKeys::default();
        learned_spell(
            668,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
            &mut LearnedInTab::default(),
        );
        learned_spell(
            99999,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
            &mut LearnedInTab::default(),
        );
        assert!(
            errors.0.is_empty(),
            "PASSIVE|DO_NOT_DISPLAY is silent, and so is an id with no record"
        );
    }

    #[test]
    fn learned_spell_adds_to_the_book_once() {
        let mut actions = PlayerActions::default();
        let mut errors = UiErrorKeys::default();
        learned_spell(
            6603,
            &mut actions,
            None,
            &mut errors,
            &mut LearnedInTab::default(),
        );
        assert!(actions.spells.contains(&6603));
        assert!(actions.dirty, "a new spell dirties the feed");

        actions.dirty = false;
        learned_spell(
            6603,
            &mut actions,
            None,
            &mut errors,
            &mut LearnedInTab::default(),
        );
        assert!(
            !actions.dirty,
            "re-learning a known spell is a no-op (insert returns false)"
        );
    }

    /// A rank-up moves the book and marks the store dirty; the bar follows in `ui_action::ranks`.
    #[test]
    fn superceded_spell_swaps_the_book_and_leaves_the_bar_to_the_rank_pass() {
        let mut actions = PlayerActions::default();
        actions.spells.insert(78); // Heroic Strike rank 1, known
        actions.buttons.insert(
            0,
            ActionButton {
                slot: 0,
                action: 78,
                kind: ACTION_KIND_SPELL,
            },
        );

        superceded_spell(
            78,
            284,
            &mut actions,
            None,
            &mut UiErrorKeys::default(),
            &mut LearnedInTab::default(),
        );

        assert!(
            !actions.spells.contains(&78),
            "the old rank leaves the book"
        );
        assert!(
            actions.spells.contains(&284),
            "the new rank enters the book"
        );
        assert_eq!(
            actions.buttons[&0].action, 78,
            "the bar follows from the book, not from here"
        );
        assert!(actions.dirty, "…and `dirty` is what makes it follow");
    }

    /// The self player's own packets, run through the real handlers in the order a server sends
    /// them, with the bar edges each one queues.
    struct CastSeq {
        app: App,
        self_e: Entity,
        /// What the handlers sent.
        sent: crossbeam_channel::Receiver<crate::net::ClientCommand>,
    }

    impl CastSeq {
        const SELF: u64 = 10;

        fn new() -> Self {
            use crate::combat_text::CombatTextSpawn;
            use crate::go_anim::GoLidOpen;
            use crate::net::{Guid, SelfPlayer};

            let mut app = App::new();
            app.add_message::<CastEvent>()
                .add_message::<SpellGoTargets>()
                .add_message::<CombatTextSpawn>()
                .add_message::<GoLidOpen>()
                .add_message::<crate::creature_anim::SheathRequest>()
                .add_message::<crate::player::StandStateRequest>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<CastReports>()
                .init_resource::<PendingCast>()
                .init_resource::<QueuedMeleeSpell>()
                .init_resource::<super::super::SpellTargeting>()
                .init_resource::<ActiveChannel>()
                .init_resource::<AutoRepeatActive>()
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<crate::ui_pet::PetBar>()
                .init_resource::<crate::items::Items>();
            let (tx, sent) = crossbeam_channel::unbounded();
            app.insert_resource(crate::net::NetCommands(tx));
            let self_e = app
                .world_mut()
                .spawn((Guid(Self::SELF), SelfPlayer, ObjectStore::default()))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(Self::SELF, self_e);
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(Self::SELF);
            Self { app, self_e, sent }
        }

        /// The attempt as the ladder commits it: the record fill (`0x6e4fb1`), then the push
        /// (`0x6e5026`), which an item without `ITEM_FLAG_PLAYERCAST` skips. The push's own
        /// signal is the attempt's, not the subject's, so it is dropped.
        fn commit(&mut self, spell_id: u32, item_flags: Option<u32>) {
            self.push(spell_id, crate::spell::Held::Cast { item_flags });
        }

        /// The record fill and the push of `held`, unsignalled.
        fn push(&mut self, spell_id: u32, held: crate::spell::Held) {
            let pushes = !matches!(
                held,
                crate::spell::Held::Cast { item_flags: Some(f) }
                    if f & benilla_protocol::messages::ITEM_FLAG_PLAYERCAST == 0
            );
            let world = self.app.world_mut();
            world.resource_mut::<PendingCast>().fill_record();
            if pushes {
                let before = world.resource::<CastReports>().0.len();
                world
                    .run_system_once(
                        move |mut slots: crate::spell::CastSlots,
                              net: Res<crate::net::NetCommands>| {
                            slots.core(&net).push(spell_id, held, Instant::now());
                        },
                    )
                    .unwrap();
                world.resource_mut::<CastReports>().0.truncate(before);
            }
        }

        /// What the handlers sent since the last call.
        fn sent(&self) -> Vec<crate::net::ClientCommand> {
            self.sent.try_iter().collect()
        }

        fn start(&mut self, spell_id: u32, cast_time_ms: u32) {
            self.app
                .world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          mut cast_events: MessageWriter<CastEvent>,
                          self_guid: Res<SelfGuid>,
                          mut reports: ResMut<CastReports>,
                          mut pending: ResMut<PendingCast>| {
                        spell_start(
                            Self::SELF,
                            spell_id,
                            0,
                            cast_time_ms,
                            None,
                            None,
                            &mut commands,
                            &index,
                            &mut cast_events,
                            &self_guid,
                            &mut reports,
                            &mut pending,
                            None,
                            1,
                        );
                    },
                )
                .unwrap();
        }

        /// `fail` is `None` for status 0, else the status-2 reason.
        fn result(&mut self, spell_id: u32, fail: Option<u8>) {
            self.app
                .world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          self_guid: Res<SelfGuid>,
                          index: Res<GuidIndex>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut core: crate::spell::CastSlots,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut auto_repeat: ResMut<AutoRepeatActive>,
                          net: Res<crate::net::NetCommands>,
                          spells: Option<Res<Spells>>| {
                        cast_result(
                            spell_id,
                            fail.is_none(),
                            fail,
                            None,
                            &mut commands,
                            &self_guid,
                            &index,
                            &casting,
                            &mut cast_events,
                            &mut core.core(&net),
                            &mut cooldowns,
                            &mut auto_repeat,
                            spells.as_deref(),
                            None,
                            &net,
                            1,
                        );
                    },
                )
                .unwrap();
        }

        /// `SMSG_SPELL_FAILED_OTHER` for our own spell.
        fn failed_other(&mut self, spell_id: u32) {
            self.app
                .world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          self_guid: Res<SelfGuid>| {
                        spell_failed_other(
                            Self::SELF,
                            spell_id,
                            &mut commands,
                            &index,
                            &casting,
                            &mut cast_events,
                            &self_guid,
                            1,
                        );
                    },
                )
                .unwrap();
        }

        fn go(&mut self, spell_id: u32) {
            use crate::combat_text::CombatTextSpawn;
            use crate::go_anim::GoLidOpen;
            let (tx, _rx) = crossbeam_channel::unbounded();
            self.app
                .world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut go_targets: MessageWriter<SpellGoTargets>,
                          self_guid: Res<SelfGuid>,
                          stores: Query<&mut ObjectStore>,
                          mut text: MessageWriter<CombatTextSpawn>,
                          mut go_lid: MessageWriter<GoLidOpen>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                          mut items: ResMut<crate::items::Items>,
                          mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                        let net_commands = crate::net::NetCommands(tx.clone());
                        spell_go(
                            Self::SELF,
                            spell_id,
                            0,
                            vec![],
                            vec![],
                            None,
                            None,
                            None,
                            None,
                            None,
                            &mut commands,
                            &index,
                            &casting,
                            &mut cast_events,
                            &mut go_targets,
                            &self_guid,
                            &stores,
                            &mut text,
                            crate::combat_text::DamageTextGates::default(),
                            &mut go_lid,
                            &mut crate::ui_loot::LootLatch::default(),
                            (
                                &mut cooldowns,
                                None,
                                &mut items,
                                &net_commands,
                                &mut pet_bar,
                                &crate::spell::SpellModifiers::default(),
                            ),
                            (
                                &mut crate::spell::AutoRepeatActive::default(),
                                &mut sheath,
                                false,
                            ),
                            1,
                        );
                    },
                )
                .unwrap();
        }

        /// `MSG_CHANNEL_START`, or `MSG_CHANNEL_UPDATE` with `start` false.
        fn channel(&mut self, spell_id: u32, ms: u32, start: bool) {
            let world = self.app.world_mut();
            let mut feed = world.remove_resource::<CastReports>().unwrap();
            let mut channel = world.remove_resource::<ActiveChannel>().unwrap();
            if start {
                channel_start(spell_id, ms, &mut channel, &mut feed);
            } else {
                channel_update(ms, &mut channel, &mut feed);
            }
            world.insert_resource(feed);
            world.insert_resource(channel);
        }

        /// Every report queued since the last call, in order: a refusal by its reason, an edge
        /// by its event name.
        fn stream(&mut self) -> Vec<String> {
            self.app
                .world_mut()
                .resource_mut::<CastReports>()
                .0
                .drain(..)
                .filter_map(|r| match r {
                    crate::ui_cast::CastReport::Refused(f) => {
                        Some(format!("REFUSED {:#04x}", f.reason))
                    }
                    crate::ui_cast::CastReport::Bar(CastBarEdge::Stop) => Some("STOP".into()),
                    crate::ui_cast::CastReport::Bar(CastBarEdge::Failed) => Some("FAILED".into()),
                    crate::ui_cast::CastReport::Bar(CastBarEdge::Interrupted) => {
                        Some("INTERRUPTED".into())
                    }
                    crate::ui_cast::CastReport::Changed => Some("CHANGED".into()),
                    _ => None,
                })
                .collect()
        }

        /// The edges queued since the last call, by event name.
        fn events(&mut self) -> Vec<&'static str> {
            self.app
                .world_mut()
                .resource_mut::<CastReports>()
                .0
                .drain(..)
                .filter_map(|r| match r {
                    crate::ui_cast::CastReport::Bar(e) => Some(e),
                    _ => None,
                })
                .map(|e| match e {
                    CastBarEdge::Start { .. } => "START",
                    CastBarEdge::Stop => "STOP",
                    CastBarEdge::Failed => "FAILED",
                    CastBarEdge::Interrupted => "INTERRUPTED",
                    CastBarEdge::Delayed { .. } => "DELAYED",
                    CastBarEdge::ChannelStart { .. } => "CHANNEL_START",
                    CastBarEdge::ChannelUpdate { remaining_ms: 0 } => "CHANNEL_STOP",
                    CastBarEdge::ChannelUpdate { .. } => "CHANNEL_UPDATE",
                })
                .collect()
        }
    }

    use bevy::ecs::system::RunSystemOnce;

    const FIREBALL: u32 = 133;
    const FORTITUDE: u32 = 1243;
    const ARCANE_MISSILES: u32 = 5143;
    /// Hearthstone's spell; the item's template carries `ITEM_FLAG_PLAYERCAST`.
    const HEARTHSTONE: u32 = 8690;
    /// Minor Healing Potion's spell; the item's template does not.
    const MINOR_HEALING: u32 = 439;

    /// vmangos sends, for every non-triggered cast, `SMSG_SPELL_START` at `Spell::prepare`
    /// (`Spell.cpp:3439`), then at the cast's end `Spell::cast` sends `SMSG_CAST_RESULT`
    /// (`3669`) and at once `SMSG_SPELL_GO` (`3703`, or `handle_immediate`'s `3771`). Each event
    /// is checked after its own packet, so a STOP at the GO fails here.
    #[test]
    fn a_timed_cast_stops_at_its_result_ahead_of_the_go() {
        let mut s = CastSeq::new();
        s.commit(FIREBALL, None);
        s.start(FIREBALL, 3_000);
        assert_eq!(s.events(), ["START"]);
        s.result(FIREBALL, None);
        assert_eq!(
            s.events(),
            ["STOP"],
            "`0x6e741a` pops the slot and `0x6e4a4b` fires STOP: the bar fills and flashes here"
        );
        s.go(FIREBALL);
        assert!(s.events().is_empty(), "`0x6e7a70` reaches no 0x152 site");
    }

    /// An instant opens no bar (`0x6e7a1d`), but it held the slot from its push, so its result
    /// still pops it with STOP.
    #[test]
    fn an_instant_gets_its_stop_too() {
        let mut s = CastSeq::new();
        s.commit(FORTITUDE, None);
        s.start(FORTITUDE, 0);
        s.result(FORTITUDE, None);
        assert_eq!(s.events(), ["STOP"]);
        s.go(FORTITUDE);
        assert!(s.events().is_empty());
        assert!(
            !s.app.world().resource::<PendingCast>().in_flight(),
            "the result opened the slot"
        );
    }

    /// A channel: the result's STOP comes before the bar opens, since vmangos sends
    /// `MSG_CHANNEL_START` after the GO (`handle_immediate`, `Spell.cpp:3781`), and the channel's
    /// own end is `MSG_CHANNEL_UPDATE(0)` (`4157`).
    #[test]
    fn a_channel_stops_at_its_result_before_its_bar_opens() {
        let mut s = CastSeq::new();
        s.commit(ARCANE_MISSILES, None);
        s.start(ARCANE_MISSILES, 0);
        s.result(ARCANE_MISSILES, None);
        s.go(ARCANE_MISSILES);
        s.channel(ARCANE_MISSILES, 5_000, true);
        s.channel(ARCANE_MISSILES, 0, false);
        assert_eq!(s.events(), ["STOP", "CHANNEL_START", "CHANNEL_STOP"]);
    }

    /// An item use takes either site: one with `ITEM_FLAG_PLAYERCAST` holds the slot and pops it
    /// (`0x6e741a`); one without was never pushed (`0x6e500b`–`0x6e501f`), and its result fires
    /// STOP at `0x6e74cb` because nothing is in flight.
    #[test]
    fn an_item_use_gets_its_stop_with_or_without_the_slot() {
        let mut s = CastSeq::new();
        s.commit(HEARTHSTONE, Some(0x40));
        s.start(HEARTHSTONE, 10_000);
        s.result(HEARTHSTONE, None);
        s.go(HEARTHSTONE);
        assert_eq!(s.events(), ["START", "STOP"]);

        let mut s = CastSeq::new();
        s.commit(MINOR_HEALING, Some(0));
        assert!(
            !s.app.world().resource::<PendingCast>().in_flight(),
            "the potion never entered the slot"
        );
        s.start(MINOR_HEALING, 0);
        s.result(MINOR_HEALING, None);
        s.go(MINOR_HEALING);
        assert_eq!(s.events(), ["STOP"]);
    }

    /// `0x6e74bc`–`0x6e74cb`: a spell not in the slot fires STOP only when nothing is in flight
    /// and the status is not 2, with no caster test.
    #[test]
    fn a_result_with_nothing_in_flight_stops_unless_it_failed() {
        let mut s = CastSeq::new();
        s.result(FORTITUDE, None);
        assert_eq!(
            s.events(),
            ["STOP"],
            "no record was ever filled, and the site does not ask"
        );
        s.result(FORTITUDE, Some(0x59));
        assert!(
            s.events().iter().all(|e| *e != "STOP"),
            "`0x6e74c5` skips status 2"
        );

        // Something in flight: our timed cast, or the spell awaiting its click.
        let mut s = CastSeq::new();
        s.commit(FIREBALL, None);
        s.result(FORTITUDE, None);
        assert!(s.events().is_empty(), "`0x6e3d30` answers a cast in flight");
        let mut s = CastSeq::new();
        s.app
            .world_mut()
            .resource_mut::<super::super::SpellTargeting>()
            .enter(FIREBALL, CastCommit::Spell, 0x40);
        s.result(FORTITUDE, None);
        assert!(s.events().is_empty(), "the cursor's spell is in flight too");
    }

    /// A server failure of the spell in flight is reported in the reference's order: the line,
    /// then the tail's cancel with its failed flag and no packet (`0x6e2218` →
    /// `0x6e4940(1, 0, reason)`), whose event is `SPELLCAST_FAILED`, or `SPELLCAST_INTERRUPTED` for
    /// 0x23 and 0x24 (`0x6e4a24`), an instant's as well as a timed cast's. The handler's own pop
    /// then finds the slot empty, so no STOP.
    #[test]
    fn a_failure_in_flight_reports_its_line_then_its_event() {
        for (reason, edge) in [
            (0x0a, "FAILED"),
            (0x23, "INTERRUPTED"),
            (0x24, "INTERRUPTED"),
        ] {
            for timed in [false, true] {
                let mut s = CastSeq::new();
                s.commit(FIREBALL, None);
                if timed {
                    s.start(FIREBALL, 3_000);
                    assert_eq!(s.events(), ["START"]);
                }
                s.result(FIREBALL, Some(reason));
                assert_eq!(
                    s.stream(),
                    [
                        format!("REFUSED {reason:#04x}"),
                        "CHANGED".into(),
                        edge.to_string()
                    ],
                    "reason {reason:#04x}, timed {timed}"
                );
                // The server's own failure cancels with no packet (`0x6e2213 sete`).
                assert!(s.sent().is_empty(), "reason {reason:#04x}, timed {timed}");
                assert!(!s.app.world().resource::<PendingCast>().in_flight());
            }
        }
    }

    /// The handler's own pop (`0x6e741a` → `0x6e4940(0, 0, 0x1c)` → `0x6e49d6`) changes the slot
    /// and signals `CURRENT_SPELL_CAST_CHANGED` ahead of its STOP; a result for a spell in no slot
    /// pops nothing and signals nothing.
    #[test]
    fn a_results_pop_signals_the_slots_change() {
        let mut s = CastSeq::new();
        s.commit(FIREBALL, None);
        s.result(FIREBALL, None);
        assert_eq!(s.stream(), ["CHANGED", "STOP"]);
        let mut s = CastSeq::new();
        s.result(FORTITUDE, None);
        assert_eq!(s.stream(), ["STOP"]);
    }

    /// With nothing in flight the tail fires the failure's event itself, with no caster test
    /// (`0x6e2228`-`0x6e224a`): vmangos's answer to our own cancel, a failing `SMSG_CAST_RESULT`
    /// 0x23, finds the slot the cancel popped and paints the bar's red INTERRUPTED a round trip
    /// later. "Another action is in progress" (0x61) fires nothing (`0x6e2234`).
    #[test]
    fn a_failure_with_nothing_in_flight_fires_its_own_event() {
        let mut s = CastSeq::new();
        s.commit(FIREBALL, None);
        s.start(FIREBALL, 3_000);
        {
            // The local cancel's pop and its STOP (`0x6e4a4b`), which spends the word.
            let mut pending = s.app.world_mut().resource_mut::<PendingCast>();
            pending.clear_if(FIREBALL);
            assert!(pending.take_caster_event());
        }
        assert_eq!(s.events(), ["START"]);
        s.result(FIREBALL, Some(0x23));
        assert_eq!(s.stream(), ["REFUSED 0x23", "INTERRUPTED"]);
        s.result(FIREBALL, Some(0x61));
        assert_eq!(s.stream(), ["REFUSED 0x61"]);
    }

    /// A dead melee slot (vmangos `AttackStop` → `InterruptSpell(CURRENT_MELEE_SPELL)` →
    /// `Spell::cancel`) arrives as `SMSG_SPELL_FAILED_OTHER`, whose handler touches no slot
    /// (`0x6e8e40`), then a failing `SMSG_CAST_RESULT` 0x23, whose report pops the strike from the
    /// slot (`0x6e2201` → `0x6e2218`): its pair, then its event while the record's caster holds.
    #[test]
    fn a_strikes_removal_signals_its_pop_at_the_cast_result() {
        const HEROIC_STRIKE: u32 = 78;
        let mut s = CastSeq::new();
        s.push(HEROIC_STRIKE, crate::spell::Held::Strike);
        s.failed_other(HEROIC_STRIKE);
        assert_eq!(s.stream(), Vec::<String>::new());
        assert_eq!(
            s.app.world().resource::<QueuedMeleeSpell>().current(),
            Some(HEROIC_STRIKE),
            "the strike is still queued"
        );
        s.result(HEROIC_STRIKE, Some(0x23));
        assert_eq!(s.stream(), ["REFUSED 0x23", "CHANGED", "INTERRUPTED"]);
        assert_eq!(s.app.world().resource::<QueuedMeleeSpell>().current(), None);
        assert!(s.sent().is_empty());
    }

    /// A strike that fired on its swing leaves the slot at its `SMSG_CAST_RESULT` (`0x6e7408` →
    /// `0x6e741a`), which vmangos sends ahead of the GO (`Spell::cast`); the GO handler `0x6e7a70`
    /// reads neither the slot nor its save, so a GO alone leaves the strike queued and signals
    /// nothing.
    #[test]
    fn a_strikes_go_leaves_the_slot_to_its_result() {
        const HEROIC_STRIKE: u32 = 78;
        let mut s = CastSeq::new();
        s.push(HEROIC_STRIKE, crate::spell::Held::Strike);
        s.go(HEROIC_STRIKE);
        assert_eq!(s.stream(), Vec::<String>::new());
        assert_eq!(
            s.app.world().resource::<QueuedMeleeSpell>().current(),
            Some(HEROIC_STRIKE),
            "the GO pops nothing"
        );
        // Under a cast, in the save: a GO leaves the save too, which the cast's pop restores.
        s.commit(FORTITUDE, None);
        s.go(HEROIC_STRIKE);
        s.result(FORTITUDE, None);
        assert_eq!(
            s.app.world().resource::<QueuedMeleeSpell>().current(),
            Some(HEROIC_STRIKE),
            "the cast popped the strike back in"
        );
        s.result(HEROIC_STRIKE, None);
        assert_eq!(s.app.world().resource::<QueuedMeleeSpell>().current(), None);
    }

    /// Esc over a cast nested on a queued strike, then a second Esc inside the round trip: the
    /// first cancels the cast and its pop copies the strike back from the save (`0x6e4b2c`); the
    /// second cancels the strike and its pop copies it back again. vmangos answers the first
    /// cancel for both, the cast and the melee slot (`HandleCancelCastOpcode`): the cast's report
    /// finds the strike in flight and fires nothing (`0x6e2228`), the strike's pops it twice, with
    /// no event, as the first press's STOP spent the caster word.
    #[test]
    fn a_second_esc_resends_the_strikes_cancel_and_the_echoes_fire_nothing() {
        use crate::net::ClientCommand;
        const HEROIC_STRIKE: u32 = 78;
        let mut s = CastSeq::new();
        s.app.init_resource::<crate::creature_anim::PlaySeq>();
        s.push(HEROIC_STRIKE, crate::spell::Held::Strike);
        s.commit(FIREBALL, None);
        let esc = |s: &mut CastSeq| {
            s.app
                .world_mut()
                .run_system_once(|mut cancel: crate::spell::SelfCancel| cancel.stop_casting())
                .unwrap();
        };
        let queued = |s: &CastSeq| {
            let q = s.app.world().resource::<QueuedMeleeSpell>();
            (q.current(), q.saved())
        };
        esc(&mut s);
        assert!(matches!(
            s.sent()[..],
            [ClientCommand::CancelCast { spell_id: FIREBALL }]
        ));
        assert_eq!(s.stream(), ["CHANGED", "STOP"]);
        assert_eq!(queued(&s), (Some(HEROIC_STRIKE), Some(HEROIC_STRIKE)));
        esc(&mut s);
        assert!(matches!(
            s.sent()[..],
            [ClientCommand::CancelCast {
                spell_id: HEROIC_STRIKE
            }]
        ));
        assert_eq!(s.stream(), ["CHANGED"]);
        assert_eq!(
            queued(&s),
            (Some(HEROIC_STRIKE), Some(HEROIC_STRIKE)),
            "still in flight"
        );
        s.failed_other(FIREBALL);
        s.result(FIREBALL, Some(0x23));
        assert_eq!(s.stream(), ["REFUSED 0x23"], "no SPELLCAST_INTERRUPTED");
        s.failed_other(HEROIC_STRIKE);
        s.result(HEROIC_STRIKE, Some(0x23));
        assert_eq!(s.stream(), ["REFUSED 0x23", "CHANGED", "CHANGED"]);
        assert_eq!(queued(&s), (None, None));
        assert!(s.sent().is_empty());
    }

    /// A strike a cast popped back over is in the slot and the save (`0x6e4b2c` copies, never
    /// clears): its failure pops twice, the report's pop copying it back (`0x6e2218`), the save
    /// clear (`0x6e73ee`), then the handler's own pop (`0x6e741a`).
    #[test]
    fn a_failing_strike_a_cast_popped_back_over_pops_twice() {
        const HEROIC_STRIKE: u32 = 78;
        let mut s = CastSeq::new();
        s.push(HEROIC_STRIKE, crate::spell::Held::Strike);
        s.commit(FORTITUDE, None);
        s.result(FORTITUDE, None);
        assert_eq!(
            s.stream(),
            ["CHANGED", "STOP"],
            "the cast pops the strike back in"
        );
        assert_eq!(
            s.app.world().resource::<QueuedMeleeSpell>().saved(),
            Some(HEROIC_STRIKE)
        );
        s.failed_other(HEROIC_STRIKE);
        s.result(HEROIC_STRIKE, Some(0x23));
        assert_eq!(
            s.stream(),
            ["REFUSED 0x23", "CHANGED", "CHANGED"],
            "two pops; the cast's STOP spent the caster word"
        );
        let q = s.app.world().resource::<QueuedMeleeSpell>();
        assert_eq!((q.current(), q.saved()), (None, None));
    }

    /// The slot's own pop fires only while the caster word is the player's (`0x6e4a04`–`0x6e4a1e`)
    /// and clears it (`0x6e4a5a`). A failure's report pops first and takes the word with it, so
    /// the handler's own pop of the same spell, popped back in from the save, is silent; a report
    /// that leaves before its pop (reason 0x17) leaves the handler's pop its STOP.
    #[test]
    fn the_slots_pop_stops_once_per_record() {
        const HEROIC_STRIKE: u32 = 78;

        // A strike queued, a cast over it; the cast's result pops the strike back in.
        let mut s = CastSeq::new();
        {
            let world = s.app.world_mut();
            world.resource_mut::<PendingCast>().fill_record();
            world.resource_mut::<QueuedMeleeSpell>().arm(HEROIC_STRIKE);
        }
        s.commit(FORTITUDE, None);
        s.result(FORTITUDE, None);
        assert_eq!(s.events(), ["STOP"]);
        s.result(HEROIC_STRIKE, None);
        assert!(
            s.events().is_empty(),
            "the strike's own result finds the word cleared by the cast's"
        );

        // A failure that pops: no STOP at the handler, and the word is spent.
        let mut s = CastSeq::new();
        s.commit(FIREBALL, None);
        s.result(FIREBALL, Some(0x59));
        assert!(s.events().iter().all(|e| *e != "STOP"));
        assert!(
            !s.app
                .world_mut()
                .resource_mut::<PendingCast>()
                .take_caster_event(),
            "the report's own pop cleared the word"
        );

        // DONT_REPORT leaves the report before its pop (`0x6e1ce4`): the handler pops with STOP.
        let mut s = CastSeq::new();
        s.commit(FIREBALL, None);
        s.result(FIREBALL, Some(0x17));
        assert_eq!(s.events(), ["STOP"]);
    }

    /// A proc's GO mid-cast (Chilled, 6136) and the cast's own GO both leave the bar alone: no GO
    /// fires a bar event.
    #[test]
    fn no_go_pushes_a_bar_edge() {
        let mut s = CastSeq::new();
        s.app.world_mut().entity_mut(s.self_e).insert(Casting {
            spell_id: FIREBALL,
            until: None,
        });
        s.go(6136);
        s.go(FIREBALL);
        assert!(s.events().is_empty());
    }

    /// A cancelled channel flashes green and fades, never red. vmangos `Spell::cancel` sends
    /// `MSG_CHANNEL_UPDATE(0)` then `SMSG_SPELL_FAILED_OTHER`, which reaches the caster too; the
    /// reference's handler for it (`0x6e8e40`, opcode `0x2a6`: `0x60d040` + `0x614150`) fires no
    /// UI event, and neither does [`spell_failed_other`].
    #[test]
    fn a_cancelled_channel_never_turns_the_bar_red() {
        use crate::creature_anim::Casting;
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        const BLIZZARD: u32 = 10;
        let t0 = Instant::now();

        let mut app = App::new();
        app.add_message::<CastEvent>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<CastReports>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<ActiveChannel>();
        let self_e = app.world_mut().spawn((Guid(10), SelfPlayer)).id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(10, self_e);
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

        // 1-2. MSG_CHANNEL_START, then the server's own end: MSG_CHANNEL_UPDATE(0).
        {
            let world = app.world_mut();
            let mut feed = world.remove_resource::<CastReports>().unwrap();
            let mut channel = world.remove_resource::<ActiveChannel>().unwrap();
            channel_start(BLIZZARD, 8_000, &mut channel, &mut feed);
            assert_eq!(
                channel.current(t0 + Duration::from_secs(1)),
                Some(BLIZZARD),
                "the mirror is armed while the channel runs"
            );
            channel_update(0, &mut channel, &mut feed);
            assert_eq!(
                channel.current(t0 + Duration::from_secs(1)),
                None,
                "update 0 closes the mirror, so the action button unlights"
            );
            world.insert_resource(feed);
            world.insert_resource(channel);
        }

        // 3. SendInterrupted(0): SMSG_SPELL_FAILED_OTHER, addressed to us, for the channel's id.
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      casting: Query<&Casting>,
                      mut cast_events: MessageWriter<CastEvent>,
                      self_guid: Res<SelfGuid>| {
                    spell_failed_other(
                        10,
                        BLIZZARD,
                        &mut commands,
                        &index,
                        &casting,
                        &mut cast_events,
                        &self_guid,
                        1,
                    );
                },
            )
            .unwrap();

        let feed = app.world().resource::<CastReports>().edges();
        assert!(
            !feed
                .iter()
                .any(|e| matches!(e, CastBarEdge::Interrupted | CastBarEdge::Failed)),
            "a cancelled channel pushes NO red edge — the bar flashes green and fades"
        );
        assert_eq!(feed.len(), 2, "exactly the two channel edges");
        assert!(matches!(feed[0], &CastBarEdge::ChannelStart { .. }));
        assert!(matches!(
            feed[1],
            &CastBarEdge::ChannelUpdate { remaining_ms: 0 }
        ));
    }

    /// The GO's inline miss word is spell gold (`0x6e7d73`/`0x6e7dcc`), printed only for a
    /// Speed-0 spell (`0x6e7d4e`), and silenced by `CombatDamage` (`0x607140`).
    #[test]
    fn the_gos_inline_miss_word_is_gold_instant_only_and_cvar_gated() {
        use crate::combat_text::{CombatTextSpawn, COLOR_SPELL_GOLD};
        use crate::creature_anim::Casting;
        use crate::go_anim::GoLidOpen;
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        const SINISTER_STRIKE: u32 = 1752; // Speed 0: an instant melee ability
        const FIREBALL: u32 = 133; // Speed 24: it travels

        let make_spells = || crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    (
                        SINISTER_STRIKE,
                        benilla_formats::SpellDisplay {
                            name: "Sinister Strike".into(),
                            ..Default::default()
                        },
                    ),
                    (
                        FIREBALL,
                        benilla_formats::SpellDisplay {
                            name: "Fireball".into(),
                            speed: 24.0,
                            ..Default::default()
                        },
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };

        // Our cast (guid 10) misses a creature (guid 20). Returns the words it floated.
        let fire = |spell: u32, combat_damage: bool| {
            let mut app = App::new();
            app.add_message::<CastEvent>()
                .add_message::<SpellGoTargets>()
                .add_message::<CombatTextSpawn>()
                .add_message::<GoLidOpen>()
                .add_message::<crate::creature_anim::SheathRequest>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<CastReports>()
                .init_resource::<PendingCast>()
                .init_resource::<QueuedMeleeSpell>()
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<crate::ui_pet::PetBar>()
                .init_resource::<crate::items::Items>();
            let self_e = app
                .world_mut()
                .spawn((Guid(10), SelfPlayer, ObjectStore::default()))
                .id();
            let victim_e = app
                .world_mut()
                .spawn((Guid(20), ObjectStore::default()))
                .id();
            {
                let mut index = app.world_mut().resource_mut::<GuidIndex>();
                index.0.insert(10, self_e);
                index.0.insert(20, victim_e);
            }
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

            let (tx, _rx) = crossbeam_channel::unbounded();
            let spells = make_spells();
            let gates = crate::combat_text::DamageTextGates {
                combat_damage,
                ..Default::default()
            };
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut go_targets: MessageWriter<SpellGoTargets>,
                          self_guid: Res<SelfGuid>,
                          stores: Query<&mut ObjectStore>,
                          mut text: MessageWriter<CombatTextSpawn>,
                          mut go_lid: MessageWriter<GoLidOpen>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                          mut items: ResMut<crate::items::Items>,
                          mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                        let net_commands = crate::net::NetCommands(tx.clone());
                        spell_go(
                            10,
                            spell,
                            0,
                            vec![],
                            vec![(20, 1)], // one MISS
                            Some(20),
                            None,
                            None,
                            None,
                            None,
                            &mut commands,
                            &index,
                            &casting,
                            &mut cast_events,
                            &mut go_targets,
                            &self_guid,
                            &stores,
                            &mut text,
                            gates,
                            &mut go_lid,
                            &mut crate::ui_loot::LootLatch::default(),
                            (
                                &mut cooldowns,
                                Some(&spells),
                                &mut items,
                                &net_commands,
                                &mut pet_bar,
                                &crate::spell::SpellModifiers::default(),
                            ),
                            (
                                &mut crate::spell::AutoRepeatActive::default(),
                                &mut sheath,
                                false,
                            ),
                            1,
                        );
                    },
                )
                .unwrap();
            app.world_mut()
                .resource_mut::<Messages<CombatTextSpawn>>()
                .drain()
                .map(|s| (s.text, s.category, s.color))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            fire(SINISTER_STRIKE, true),
            vec![("Miss".to_string(), 3, Some(COLOR_SPELL_GOLD))],
            "an instant ability's miss word prints here, in spell gold — not white"
        );
        assert!(
            fire(FIREBALL, true).is_empty(),
            "Speed 24: `0x6e7d4e` skips the inline emit — the projectile floats it on arrival"
        );
        assert!(
            fire(SINISTER_STRIKE, false).is_empty(),
            "CombatDamage 0 gates the word emitter, not just the number emitter"
        );
    }

    /// `UNIT_FIELD_CHARMEDBY` / `SUMMONEDBY` / `CREATEDBY`, two dwords each.
    const CHARMEDBY: u16 = 10;
    const SUMMONEDBY: u16 = 12;
    const CREATEDBY: u16 = 14;

    /// A unit store whose `field` pair names `owner`.
    fn owned(field: u16, owner: u64) -> ObjectStore {
        ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
            (field, owner as u32),
            (field + 1, (owner >> 32) as u32),
        ]))
    }

    /// One SPELL_GO of `display` (as spell 2649) by `caster` in a world holding us (guid 10) and,
    /// when given, the casting item; `mods` is the player's table. Returns the player bank's and
    /// the pet bank's armed durations, the pet bar's repaint count and the pet list's generation.
    fn go_cooldowns(
        caster: u64,
        store: ObjectStore,
        item: Option<(u64, ObjectStore)>,
        display: fn() -> benilla_formats::SpellDisplay,
        mods: crate::spell::SpellModifiers,
    ) -> (u32, u32, u32, u64) {
        use crate::combat_text::CombatTextSpawn;
        use crate::creature_anim::Casting;
        use crate::go_anim::GoLidOpen;
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        const SPELL: u32 = 2649;
        let spells = crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(SPELL, display())].into_iter().collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };
        let mut app = App::new();
        app.add_message::<CastEvent>()
            .add_message::<SpellGoTargets>()
            .add_message::<CombatTextSpawn>()
            .add_message::<GoLidOpen>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<CastReports>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::items::Items>();
        let self_e = app
            .world_mut()
            .spawn((Guid(10), SelfPlayer, ObjectStore::default()))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(10, self_e);
        if caster != 10 {
            let caster_e = app.world_mut().spawn((Guid(caster), store)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(caster, caster_e);
        }
        let item_caster = item.map(|(guid, item_store)| {
            let e = app.world_mut().spawn((Guid(guid), item_store)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            guid
        });
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

        let (tx, _rx) = crossbeam_channel::unbounded();
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      casting: Query<&Casting>,
                      mut cast_events: MessageWriter<CastEvent>,
                      mut go_targets: MessageWriter<SpellGoTargets>,
                      self_guid: Res<SelfGuid>,
                      stores: Query<&mut ObjectStore>,
                      mut text: MessageWriter<CombatTextSpawn>,
                      mut go_lid: MessageWriter<GoLidOpen>,
                      mut cooldowns: ResMut<Cooldowns>,
                      mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                      mut items: ResMut<crate::items::Items>,
                      mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                    let net_commands = crate::net::NetCommands(tx.clone());
                    spell_go(
                        caster,
                        SPELL,
                        0,
                        vec![],
                        vec![],
                        None,
                        None,
                        None,
                        None,
                        item_caster.or(Some(caster)),
                        &mut commands,
                        &index,
                        &casting,
                        &mut cast_events,
                        &mut go_targets,
                        &self_guid,
                        &stores,
                        &mut text,
                        crate::combat_text::DamageTextGates::default(),
                        &mut go_lid,
                        &mut crate::ui_loot::LootLatch::default(),
                        (
                            &mut cooldowns,
                            Some(&spells),
                            &mut items,
                            &net_commands,
                            &mut pet_bar,
                            &mods,
                        ),
                        (
                            &mut crate::spell::AutoRepeatActive::default(),
                            &mut sheath,
                            false,
                        ),
                        1,
                    );
                },
            )
            .unwrap();
        let now = Instant::now();
        let armed = |c: &Cooldowns| c.info(SPELL, 0, Some(&display()), now).duration_ms;
        let world = app.world();
        (
            armed(world.resource::<Cooldowns>()),
            armed(&world.resource::<crate::ui_pet::PetBar>().cooldowns),
            world.resource::<crate::ui_pet::PetBar>().bar_signals,
            world
                .resource::<crate::ui_pet::PetBar>()
                .cooldowns
                .generation,
        )
    }

    /// Growl's shape: a 5 s recovery, no category.
    fn growl() -> benilla_formats::SpellDisplay {
        benilla_formats::SpellDisplay {
            name: "Growl".into(),
            recovery_ms: 5000,
            ..Default::default()
        }
    }

    /// The GO's pet leg arms the pet's bank (`0xcecb04`), never the player's; the owner read falls
    /// back to SUMMONEDBY, not CREATEDBY (`0x6e859a`); a stranger's cast arms neither.
    #[test]
    fn a_pets_own_go_arms_the_pet_bank_and_only_the_pet_bank() {
        let fire = |caster: u64, store: ObjectStore| {
            let (player, pet, signals, generation) = go_cooldowns(
                caster,
                store,
                None,
                growl,
                crate::spell::SpellModifiers::default(),
            );
            (player > 0, pet > 0, signals, generation)
        };

        // Our pet: the PET bank only. The insert moves the list's generation, which is the flush's
        // edge (`0x6e85fc`, `0x6e8601`: the cooldown events), and signals no `PET_BAR_UPDATE`.
        let (player, pet, signals, generation) = fire(20, owned(SUMMONEDBY, 10));
        assert!(pet, "our pet's GO arms the pet bank");
        assert!(!player, "…and never the player's");
        assert_eq!(generation, 1, "the pet list's flush edge");
        assert_eq!(signals, 0, "the bar's own signal is not the GO's");

        // A charm reads CHARMEDBY first: the same leg, the other field.
        let (_, charmed, _, _) = fire(20, owned(CHARMEDBY, 10));
        assert!(charmed, "a charmed unit's GO arms it too");

        // A totem carries CREATEDBY and no SUMMONEDBY: `0x5ee5a0` would accept it, `0x6e859a` does
        // not.
        let (_, totem, _, _) = fire(30, owned(CREATEDBY, 10));
        assert!(!totem, "CREATEDBY alone is not this leg's owner test");

        // Somebody else's pet: neither bank, no flush.
        let (p2, pet2, _, generation) = fire(40, owned(SUMMONEDBY, 99));
        assert!(!p2 && !pet2, "a stranger's pet arms nothing");
        assert_eq!(generation, 0, "and moves no edge");
    }

    /// Op 11 on the GO insert: the self leg (`0x6e846f`, and `0x6e2b60`'s tail) and the pet leg
    /// (`0x6e85c5`) apply the player's table; the item leg inserts the DBC values (`0x6e8566`),
    /// here through its not-streamed fallback.
    #[test]
    fn op_11_shortens_the_self_and_pet_inserts_and_never_the_item_one() {
        fn display() -> benilla_formats::SpellDisplay {
            benilla_formats::SpellDisplay {
                spell_family: 9,
                spell_family_flags: 1,
                ..growl()
            }
        }
        let mods = || {
            let mut m = crate::spell::SpellModifiers::default();
            m.set_class_family(9);
            m.set(true, 0, crate::spell::OP_COOLDOWN, -2000);
            m
        };
        let own = ObjectStore::default();
        assert_eq!(go_cooldowns(10, own, None, display, mods()).0, 3000);
        let pet = go_cooldowns(20, owned(SUMMONEDBY, 10), None, display, mods());
        assert_eq!(pet.1, 3000, "the pet leg takes the player's op 11");
        let item = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(3, 6948)]));
        let used = go_cooldowns(
            10,
            ObjectStore::default(),
            Some((50, item)),
            display,
            mods(),
        );
        assert_eq!(used.0, 5000, "the item leg applies no modifier");
    }

    /// Both producers of the chain-hop array, the GO's hit list and the 816 packet, drop the
    /// caster's own guid and clear before they fill.
    #[test]
    fn both_wire_producers_fill_the_casters_chain_hop_array() {
        use crate::combat_text::CombatTextSpawn;
        use crate::creature_anim::Casting;
        use crate::entities::ChainHops;
        use crate::go_anim::GoLidOpen;
        use crate::net::Guid;
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_message::<CastEvent>()
            .add_message::<SpellGoTargets>()
            .add_message::<CombatTextSpawn>()
            .add_message::<GoLidOpen>()
            .add_message::<crate::creature_anim::SheathRequest>()
            .add_message::<crate::player::StandStateRequest>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<CastReports>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::items::Items>();

        // A caster and two streamed victims; guid 40 is never streamed to us.
        let mut spawn = |guid: u64| {
            let e = app
                .world_mut()
                .spawn((Guid(guid), ObjectStore::default()))
                .id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(guid, e);
            e
        };
        let (caster, t1, t2) = (spawn(10), spawn(20), spawn(30));

        let hops = |app: &App| {
            app.world()
                .entity(caster)
                .get::<ChainHops>()
                .map(|h| h.0.clone())
        };

        // `HandleSpellGo`'s fill (`0x6e800d`): the caster itself (vmangos includes self-hits) and
        // an unstreamed target both drop.
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      casting: Query<&Casting>,
                      mut cast_events: MessageWriter<CastEvent>,
                      mut go_targets: MessageWriter<SpellGoTargets>,
                      self_guid: Res<SelfGuid>,
                      stores: Query<&mut ObjectStore>,
                      mut text: MessageWriter<CombatTextSpawn>,
                      mut go_lid: MessageWriter<GoLidOpen>,
                      mut cooldowns: ResMut<Cooldowns>,
                      mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                      mut items: ResMut<crate::items::Items>,
                      mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                    let net_commands = crate::net::NetCommands(tx.clone());
                    spell_go(
                        10,
                        421, // Chain Lightning
                        0,
                        vec![10, 20, 40, 30],
                        vec![],
                        None,
                        None,
                        None,
                        None,
                        None,
                        &mut commands,
                        &index,
                        &casting,
                        &mut cast_events,
                        &mut go_targets,
                        &self_guid,
                        &stores,
                        &mut text,
                        crate::combat_text::DamageTextGates::default(),
                        &mut go_lid,
                        &mut crate::ui_loot::LootLatch::default(),
                        (
                            &mut cooldowns,
                            None,
                            &mut items,
                            &net_commands,
                            &mut pet_bar,
                            &crate::spell::SpellModifiers::default(),
                        ),
                        (
                            &mut crate::spell::AutoRepeatActive::default(),
                            &mut sheath,
                            false,
                        ),
                        1,
                    );
                },
            )
            .unwrap();
        assert_eq!(
            hops(&app),
            Some(vec![t1, t2]),
            "the GO fills the array in wire order, minus the caster and the unstreamed target"
        );

        // The 816 packet, vmangos's for a channelled chain, clears the GO's list first.
        app.world_mut()
            .run_system_once(move |mut commands: Commands, index: Res<GuidIndex>| {
                spell_chain_targets(10, 689, vec![30, 10], &mut commands, &index);
            })
            .unwrap();
        assert_eq!(
            hops(&app),
            Some(vec![t2]),
            "clear-before-fill, caster dropped"
        );
    }

    /// A deselect during Auto Shot arrives as a `SMSG_CAST_RESULT` failure of the cached repeat
    /// (vmangos `HandleSetSelectionOpcode` → `Spell::cancel`), and `6e1cd9`'s jump into `0x6ea080`
    /// makes it the full cancel: the key and the shooting idle both drop.
    #[test]
    fn a_cast_result_fail_of_the_cached_auto_repeat_spell_disarms_the_shooting_idle() {
        use crate::creature_anim::AutoRepeatArmed;
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_message::<CastEvent>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<CastReports>()
            .init_resource::<super::super::SpellTargeting>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<AutoRepeatActive>();

        let self_e = app
            .world_mut()
            .spawn((
                Guid(10),
                SelfPlayer,
                AutoRepeatArmed,
                crate::creature_anim::NockedAmmo { display_id: 5996 },
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(10, self_e);
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);
        app.world_mut().resource_mut::<AutoRepeatActive>().0 = Some(75);

        let (tx, rx) = crossbeam_channel::unbounded();
        let fire_fail = |app: &mut App, reason: u8| {
            let tx = tx.clone();
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          self_guid: Res<SelfGuid>,
                          index: Res<GuidIndex>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut core: crate::spell::CastSlots,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut auto_repeat: ResMut<AutoRepeatActive>| {
                        let net = crate::net::NetCommands(tx.clone());
                        cast_result(
                            75,
                            false,
                            Some(reason),
                            None,
                            &mut commands,
                            &self_guid,
                            &index,
                            &casting,
                            &mut cast_events,
                            &mut core.core(&net),
                            &mut cooldowns,
                            &mut auto_repeat,
                            None,
                            None,
                            &net,
                            1,
                        );
                    },
                )
                .unwrap();
        };

        // Reason 0x17 is the one skip (`6e1ce1 cmp cl,0x17; je`): armed stays armed.
        fire_fail(&mut app, 0x17);
        assert_eq!(app.world().resource::<AutoRepeatActive>().0, Some(75));
        assert!(app
            .world()
            .entity(self_e)
            .get::<AutoRepeatArmed>()
            .is_some());

        // SPELL_FAILED_INTERRUPTED, the deselect's wire form, runs the full cancel.
        fire_fail(&mut app, 0x1e);
        assert_eq!(
            app.world().resource::<AutoRepeatActive>().0,
            None,
            "the autorepeat key drops"
        );
        assert!(
            app.world()
                .entity(self_e)
                .get::<AutoRepeatArmed>()
                .is_none(),
            "the shooting-idle gate drops with it — the stuck Load/Hold stance"
        );
        assert!(
            app.world()
                .entity(self_e)
                .get::<crate::creature_anim::NockedAmmo>()
                .is_none(),
            "the cancel un-nocks (the client's 0x6ea140 -> 0x60f530)"
        );
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            sent.iter()
                .any(|c| matches!(c, crate::net::ClientCommand::CancelAutoRepeat)),
            "the cancel acks the server (CMSG_CANCEL_AUTO_REPEAT_SPELL, the 0x6ea0c6 send)"
        );
    }

    /// `SMSG_REMOVED_SPELL` shrinks the book and dirties the feeds only on a real removal: a talent
    /// wipe sends removals for every rank of the class tree, learned or not.
    #[test]
    fn a_removal_shrinks_the_book_and_dirties_the_feeds() {
        let mut actions = PlayerActions::default();
        actions.spells.extend([14522, 14788, 14789]);

        let mut errors = UiErrorKeys::default();
        removed_spell(14788, &mut actions, None, &mut errors);
        assert!(!actions.spells.contains(&14788));
        assert!(actions.dirty);

        actions.dirty = false;
        removed_spell(14788, &mut actions, None, &mut errors);
        assert!(!actions.dirty, "a spell we never knew is not a repaint");
    }

    /// The unlearn line carries the bare name, with no rank, and each of its gates silences it.
    #[test]
    fn an_unlearn_announces_the_bare_name_and_four_things_silence_it() {
        let row = |attributes: u32, cast_ui: u32| benilla_formats::SpellDisplay {
            name: "Improved Fireball".to_string(),
            rank: Some("Rank 3".to_string()),
            attributes,
            cast_ui,
            ..Default::default()
        };

        let mut errors = UiErrorKeys::default();
        removed_spell(
            11069,
            &mut PlayerActions::default(),
            Some(&catalog_with(11069, row(0, 0))),
            &mut errors,
        );
        assert_eq!(errors.0.len(), 1);
        assert_eq!(errors.0[0].key, "ERR_SPELL_UNLEARNED_S");
        assert_eq!(
            errors.0[0].arg_s(),
            Some("Improved Fireball"),
            "no rank on the unlearn path"
        );

        // …and each gate on its own, every one of them silent.
        for (attributes, cast_ui, why) in [
            (
                0x20,
                0,
                "IS_TRADESKILL — the container join zeroes the flag at 0x5ea170",
            ),
            (
                0,
                1,
                "castUI > 0 takes the container walk and never reaches 0x5ea292",
            ),
            (0x80, 0, "DO_NOT_DISPLAY — the sign test at 0x5ea29b"),
        ] {
            let mut errors = UiErrorKeys::default();
            removed_spell(
                11069,
                &mut PlayerActions::default(),
                Some(&catalog_with(11069, row(attributes, cast_ui))),
                &mut errors,
            );
            assert!(errors.0.is_empty(), "{why}");
        }

        // An unknown id says nothing, like every other display path here.
        let mut errors = UiErrorKeys::default();
        removed_spell(
            99999,
            &mut PlayerActions::default(),
            Some(&catalog_with(11069, row(0, 0))),
            &mut errors,
        );
        assert!(errors.0.is_empty());
    }

    /// `castUI` gates the unlearn line but not the learn line, which reads it only afterwards, at
    /// `0x4b29bf`, for the book slot.
    #[test]
    fn cast_ui_silences_the_unlearn_but_not_the_learn() {
        let spells = catalog_with(
            1234,
            benilla_formats::SpellDisplay {
                name: "Some Castbar Spell".to_string(),
                cast_ui: 2,
                ..Default::default()
            },
        );

        let mut errors = UiErrorKeys::default();
        learned_spell(
            1234,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
            &mut LearnedInTab::default(),
        );
        assert_eq!(errors.0.len(), 1, "the learn block never reads castUI");
        assert_eq!(errors.0[0].key, "ERR_LEARN_SPELL_S");

        let mut errors = UiErrorKeys::default();
        removed_spell(
            1234,
            &mut PlayerActions::default(),
            Some(&spells),
            &mut errors,
        );
        assert!(errors.0.is_empty(), "…but the unlearn block does");
    }
    /// The GO-deferred auto-attack start (`0x6e83c0`):
    /// - an Ex2 bit-20 spell's own GO swings at its first hit target (`0x6e83e9`);
    /// - Serpent Sting carries Ex2 bit 17 (`DO_NOT_RESET_COMBAT_TIMERS`), not bit 20: no swing;
    /// - already swinging, nothing (`0x6e83e7`);
    /// - another caster's Backstab, nothing.
    #[test]
    fn a_go_deferred_spell_swings_at_its_first_hit_and_the_hunter_shots_do_not() {
        use crate::combat_text::CombatTextSpawn;
        use crate::creature_anim::{Casting, Engaged};
        use crate::go_anim::GoLidOpen;
        use crate::net::{ClientCommand, Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        const BACKSTAB: u32 = 53;
        const SERPENT_STING: u32 = 1978;
        let spell = |name: &str, ex2: u32| benilla_formats::SpellDisplay {
            name: name.into(),
            attributes_ex2: ex2,
            ..Default::default()
        };
        let make_spells = || crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    // The 1.12 rows: Backstab Ex2 0x100000, Serpent Sting Ex2 0x20000.
                    (BACKSTAB, spell("Backstab", 0x0010_0000)),
                    (SERPENT_STING, spell("Serpent Sting", 0x0002_0000)),
                ]
                .into_iter()
                .collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };

        // `caster` casts `spell_id`, landing on 20; `engaged` is our mirror of `[+0xc48]`.
        // Returns every command the seam put on the wire.
        let fire = |caster: u64, spell_id: u32, engaged: bool| -> Vec<ClientCommand> {
            let mut app = App::new();
            app.add_message::<CastEvent>()
                .add_message::<SpellGoTargets>()
                .add_message::<CombatTextSpawn>()
                .add_message::<GoLidOpen>()
                .add_message::<crate::creature_anim::SheathRequest>()
                .add_message::<crate::player::StandStateRequest>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<CastReports>()
                .init_resource::<PendingCast>()
                .init_resource::<QueuedMeleeSpell>()
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<crate::ui_pet::PetBar>()
                .init_resource::<crate::items::Items>();
            let self_e = app
                .world_mut()
                .spawn((Guid(10), SelfPlayer, ObjectStore::default()))
                .id();
            if engaged {
                app.world_mut().entity_mut(self_e).insert(Engaged(0));
            }
            let other_e = app
                .world_mut()
                .spawn((Guid(11), ObjectStore::default()))
                .id();
            {
                let mut index = app.world_mut().resource_mut::<GuidIndex>();
                index.0.insert(10, self_e);
                index.0.insert(11, other_e);
            }
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

            let (tx, rx) = crossbeam_channel::unbounded();
            let spells = make_spells();
            app.world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          index: Res<GuidIndex>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut go_targets: MessageWriter<SpellGoTargets>,
                          self_guid: Res<SelfGuid>,
                          stores: Query<&mut ObjectStore>,
                          mut text: MessageWriter<CombatTextSpawn>,
                          mut go_lid: MessageWriter<GoLidOpen>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                          mut items: ResMut<crate::items::Items>,
                          mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                        let net_commands = crate::net::NetCommands(tx.clone());
                        spell_go(
                            caster,
                            spell_id,
                            0,
                            vec![20],
                            vec![],
                            // A different guid from `hits[0]`, which the arm must take.
                            Some(99),
                            None,
                            None,
                            None,
                            None,
                            &mut commands,
                            &index,
                            &casting,
                            &mut cast_events,
                            &mut go_targets,
                            &self_guid,
                            &stores,
                            &mut text,
                            crate::combat_text::DamageTextGates::default(),
                            &mut go_lid,
                            &mut crate::ui_loot::LootLatch::default(),
                            (
                                &mut cooldowns,
                                Some(&spells),
                                &mut items,
                                &net_commands,
                                &mut pet_bar,
                                &crate::spell::SpellModifiers::default(),
                            ),
                            (
                                &mut crate::spell::AutoRepeatActive::default(),
                                &mut sheath,
                                engaged,
                            ),
                            1,
                        );
                    },
                )
                .unwrap();
            rx.try_iter().collect()
        };

        let swings = |cmds: &[ClientCommand]| {
            cmds.iter()
                .filter_map(|c| match c {
                    ClientCommand::AttackSwing { guid } => Some(*guid),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(
            swings(&fire(10, BACKSTAB, false)),
            vec![20],
            "a bit20 spell's own GO starts the swing at its first hit target"
        );
        assert!(
            swings(&fire(10, SERPENT_STING, false)).is_empty(),
            "Serpent Sting carries Ex2 bit 17, not bit 20 — it starts nothing"
        );
        assert!(
            swings(&fire(10, BACKSTAB, true)).is_empty(),
            "already swinging: `0x6e83e7`'s attack-lock gate refuses"
        );
        assert!(
            swings(&fire(11, BACKSTAB, false)).is_empty(),
            "somebody else's Backstab is not our attack-start"
        );
    }
    /// The `modalNextSpell` chain (`HandleCastResult 0x6e7330`, `0x6e7408`-`0x6e74aa`):
    /// - a successful sting chains Auto Shot (`0x6e735a jne` past the report to `0x6e73eb`);
    /// - a failed one does not: its report pops the in-flight slot (`0x6e2218`), so `0x6e7408`
    ///   misses; it chains only when the report left before that pop (reason 0x17, `0x6e1ce4`;
    ///   a form-less 0x56, `0x6e20c6`; an uncached 0x5c or 0x78 item, `0x6e1eab`) or when the
    ///   save held the same spell (`0x6e73ee`);
    /// - the in-flight guard clears first, or the IsCasting rung would refuse the chain;
    /// - Auto Shot already running sends nothing (`0x6e745b`'s equal branch);
    /// - a reply for a spell not in flight chains nothing (`0x6e7408`).
    #[test]
    fn a_hunter_shots_cast_result_chains_auto_shot_exactly_once() {
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        const SERPENT_STING: u32 = 1978;
        const AUTO_SHOT: u32 = 75;
        const HEROIC_STRIKE: u32 = 78;
        const OUT_OF_RANGE: u8 = 0x59;
        const DONT_REPORT: u8 = 0x17;
        const ONLY_SHAPESHIFT: u8 = 0x56;
        const REAGENTS: u8 = 0x5c;
        const TOTEMS: u8 = 0x78;
        const TIN_BAR: u32 = 4306;

        // The sting row carries `stances`: the form bits its 0x56 report names.
        let spells = |stances: u32| crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    (
                        SERPENT_STING,
                        benilla_formats::SpellDisplay {
                            name: "Serpent Sting".into(),
                            // The 1.12 row: ranged slot, and column 38 = 75.
                            attributes: 0x0001_0002,
                            attributes_ex2: 0x0002_0000,
                            modal_next_spell: AUTO_SHOT,
                            stances,
                            ..Default::default()
                        },
                    ),
                    (
                        AUTO_SHOT,
                        benilla_formats::SpellDisplay {
                            name: "Auto Shot".into(),
                            attributes: 0x0005_0012,
                            attributes_ex2: 0x20,
                            // Auto Shot's own column 38 is 0: the chain is one hop.
                            modal_next_spell: 0,
                            ..Default::default()
                        },
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            forms: [(
                1,
                benilla_formats::ShapeshiftForm {
                    name: "Cat Form".into(),
                    ..Default::default()
                },
            )]
            .into_iter()
            .collect(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };

        /// What one `SMSG_CAST_RESULT` meets and carries.
        #[derive(Clone, Copy)]
        struct Case {
            spell_id: u32,
            /// `None` is a success, `Some` the failure's reason.
            fail: Option<u8>,
            arg: Option<u32>,
            /// The cast guard and the queued strike on entry.
            pending: Option<u32>,
            queued: Option<u32>,
            /// The live auto-repeat spell.
            running: Option<u32>,
            /// The sting's `Stances` mask.
            stances: u32,
            /// The item cache holds `TIN_BAR`.
            cached: bool,
        }
        let reply = |spell_id: u32, fail: Option<u8>| Case {
            spell_id,
            fail,
            arg: None,
            pending: Some(spell_id),
            queued: None,
            running: None,
            stances: 0,
            cached: false,
        };

        // One CAST_RESULT. Returns what the reply chains, and the guard and the strike left.
        let fire = |r: Case| {
            let mut app = App::new();
            app.add_message::<CastEvent>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<crate::ui_action::UiErrorKeys>()
                .init_resource::<CastReports>()
                .init_resource::<super::super::SpellTargeting>()
                .init_resource::<PendingCast>()
                .init_resource::<QueuedMeleeSpell>()
                .init_resource::<Cooldowns>()
                .init_resource::<crate::spell::SpellModifiers>()
                .init_resource::<AutoRepeatActive>();
            let self_e = app.world_mut().spawn((Guid(10), SelfPlayer)).id();
            app.world_mut()
                .resource_mut::<GuidIndex>()
                .0
                .insert(10, self_e);
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);
            app.world_mut().resource_mut::<AutoRepeatActive>().0 = r.running;
            // A strike with a cast in flight is the save's, which the cast's push filled.
            if let Some(id) = r.pending {
                app.world_mut()
                    .resource_mut::<QueuedMeleeSpell>()
                    .push(r.queued, None);
                app.world_mut()
                    .resource_mut::<PendingCast>()
                    .push(id, None, Instant::now());
            } else if let Some(id) = r.queued {
                app.world_mut().resource_mut::<QueuedMeleeSpell>().arm(id);
            }
            let mut items = crate::items::Items::default();
            if r.cached {
                items.insert_template(TIN_BAR, Some(crate::items::test_template("Tin Bar")));
            }
            let (tx, _rx) = crossbeam_channel::unbounded();
            let cat = spells(r.stances);
            let chained = app
                .world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          self_guid: Res<SelfGuid>,
                          index: Res<GuidIndex>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut core: crate::spell::CastSlots,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut auto_repeat: ResMut<AutoRepeatActive>| {
                        let net = crate::net::NetCommands(tx.clone());
                        cast_result(
                            r.spell_id,
                            r.fail.is_none(),
                            r.fail,
                            r.arg,
                            &mut commands,
                            &self_guid,
                            &index,
                            &casting,
                            &mut cast_events,
                            &mut core.core(&net),
                            &mut cooldowns,
                            &mut auto_repeat,
                            Some(&cat),
                            Some(&items),
                            &net,
                            1,
                        )
                    },
                )
                .unwrap();
            let armed = app.world().resource::<PendingCast>().in_flight();
            let queued = app.world().resource::<QueuedMeleeSpell>().current();
            (chained, armed, queued)
        };
        let sting = |fail| reply(SERPENT_STING, fail);

        // The ordinary case: the sting lands, and Auto Shot follows by itself.
        assert_eq!(
            fire(sting(None)),
            (Some(AUTO_SHOT), false, None),
            "a successful sting chains Auto Shot, and clears the in-flight guard first"
        );
        // Already shooting: re-arm, never re-cast.
        assert_eq!(
            fire(Case {
                running: Some(AUTO_SHOT),
                ..sting(None)
            })
            .0,
            None,
            "Auto Shot already running: the equal branch sends nothing"
        );
        // Auto Shot's own replies terminate the chain.
        assert_eq!(
            fire(Case {
                running: Some(AUTO_SHOT),
                ..reply(AUTO_SHOT, None)
            })
            .0,
            None,
            "Auto Shot's own column 38 is 0 - no second hop"
        );
        // Not our in-flight cast (a proc's result, a stale reply): not ours to chain from.
        assert_eq!(
            fire(Case {
                pending: Some(133),
                ..sting(None)
            })
            .0,
            None,
            "a reply for a spell we do not have in flight chains nothing"
        );

        // A refused shot: the report pops the slot (`0x6e2218`), so `0x6e7408` misses.
        assert_eq!(
            fire(sting(Some(OUT_OF_RANGE))),
            (None, false, None),
            "a failed sting chains nothing, and the guard is open"
        );
        // Reason 0x17 leaves the report before the pop (`0x6e1ce4`, `0x6e1cfa`).
        assert_eq!(
            fire(sting(Some(DONT_REPORT))),
            (Some(AUTO_SHOT), false, None),
            "a DONT_REPORT failure still finds the sting in flight"
        );
        // The same with Auto Shot running: the equal branch re-arms, sends nothing.
        assert_eq!(
            fire(Case {
                running: Some(AUTO_SHOT),
                ..sting(Some(DONT_REPORT))
            })
            .0,
            None
        );

        // 0x56 names the forms of `Stances`; with none named it leaves at `0x6e20c6`.
        assert_eq!(
            fire(sting(Some(ONLY_SHAPESHIFT))).0,
            Some(AUTO_SHOT),
            "a form-less ONLY_SHAPESHIFT report leaves before the pop"
        );
        assert_eq!(
            fire(Case {
                stances: 1,
                ..sting(Some(ONLY_SHAPESHIFT))
            })
            .0,
            None,
            "a named form shows its line, and the pop follows"
        );

        // 0x5c and 0x78 read the item cache by the first word (`0x6e1e7f`): a miss leaves at
        // `0x6e1eab`, a hit shows the line and pops.
        for reason in [REAGENTS, TOTEMS] {
            let named = Case {
                arg: Some(TIN_BAR),
                ..sting(Some(reason))
            };
            assert_eq!(
                fire(named).0,
                Some(AUTO_SHOT),
                "reason {reason:#x} with the item uncached leaves before the pop"
            );
            assert_eq!(
                fire(Case {
                    cached: true,
                    ..named
                })
                .0,
                None,
                "reason {reason:#x} with the item cached pops"
            );
            assert_eq!(
                fire(Case {
                    cached: true,
                    arg: None,
                    ..named
                })
                .0,
                Some(AUTO_SHOT),
                "reason {reason:#x} with no first word asks the cache for -1, a miss"
            );
        }

        // A cast over a queued strike: the report's pop restores the strike into the slot. A
        // different strike is not the spell; the same spell in the save is (`0x6e73ee` clears
        // the save after the report, so `0x6e7408` finds it popped in).
        assert_eq!(
            fire(Case {
                queued: Some(HEROIC_STRIKE),
                ..sting(Some(OUT_OF_RANGE))
            }),
            (None, false, Some(HEROIC_STRIKE)),
            "the strike under a failed shot comes back to the slot and is not the shot"
        );
        assert_eq!(
            fire(Case {
                queued: Some(SERPENT_STING),
                ..sting(Some(OUT_OF_RANGE))
            }),
            (Some(AUTO_SHOT), false, None),
            "the save holding the same spell is popped in, matches, and chains"
        );
        // A strike held under another cast is not the in-flight slot (`0x6e7408`).
        assert_eq!(
            fire(Case {
                pending: Some(133),
                queued: Some(SERPENT_STING),
                ..sting(Some(OUT_OF_RANGE))
            })
            .0,
            None,
            "the save alone is not the in-flight slot"
        );
    }
}
