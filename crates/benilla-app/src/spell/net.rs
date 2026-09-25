//! The spell's packet handlers (in the net handler table since 2324, moved out of the drain's
//! spells arm file) — the spell book and the action bar, the cast lifecycle (the decision
//! 0099/0107 precast → resolve pipeline), the cooldowns, the channels, the aura durations and the
//! spell modifiers. The lifecycle *state* these fold into is this module's since decision
//! 2328 ([`super::inflight`], [`super::cooldowns`], [`super::mods`]); the handlers reach it,
//! and the windows' stores they also touch, through [`Lifecycle`] and [`Scene`].

use std::time::{Duration, Instant};

use benilla_formats::LearnAnnouncement;
use benilla_protocol::messages::{ActionButton, SpellCooldown};
use bevy::prelude::*;

use super::cast_target::CastTargeting;
use super::{
    ActiveChannel, AutoRepeatActive, CastCommit, CastLadder, Cooldowns, PendingCast,
    QueuedMeleeSpell,
};
use crate::creature_anim::{CastEvent, CastEventKind, Casting, SpellGoTargets};
use crate::ui_action::{CastErrors, PlayerActions, Spells, UiError, UiErrorKeys};
use crate::ui_aura::AuraDurations;
use crate::ui_cast::{CastBarEdge, CastBarFeed};
use crate::ui_spellbook::LearnedInTab;

use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::ecs::system::SystemParam;

use crate::net::{GuidIndex, NetCommands, NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};

/// Register the spell's handlers — called from [`super::SpellPlugin`].
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

/// The cast lifecycle's state and catalogs, as one parameter — what every handler here folds
/// its packet into. The in-flight slot, the cooldowns and the modifiers are the spell's own
/// (2328); the action store, the spellbook's learned-tab list, the aura durations and the pet
/// bar are their windows', written here because the packet is the spell's. The cast reply's
/// handler is the exception — it casts, so it takes the ladder instead ([`on_cast_result`]).
#[derive(SystemParam)]
pub(crate) struct Lifecycle<'w> {
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    spells: Option<Res<'w, Spells>>,
    net: Res<'w, NetCommands>,
    actions: ResMut<'w, PlayerActions>,
    learned_in_tab: ResMut<'w, LearnedInTab>,
    cast_bar: ResMut<'w, CastBarFeed>,
    pending: ResMut<'w, PendingCast>,
    queued_melee: ResMut<'w, QueuedMeleeSpell>,
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

/// The scene a cast lands in — the streamed units, the item store, and the animation, text and
/// loot sinks a resolve writes.
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

/// The spell-book/action-bar pair → the action store the UI feed reads (`crate::ui_action`),
/// sent once at login (and the bar again on server-side edits).
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

/// What `HandleCastResult` reaches beyond the ladder — the reply's own scene.
#[derive(SystemParam)]
pub(crate) struct Reply<'w, 's> {
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    cast_bar: ResMut<'w, CastBarFeed>,
    casting: Query<'w, 's, &'static Casting>,
    cast_events: MessageWriter<'w, CastEvent>,
    play_seq: ResMut<'w, crate::creature_anim::PlaySeq>,
}

/// `HandleCastResult 0x6e7330` — the one handler that *casts*. A reply whose spell names a
/// `modalNextSpell` (column 38) chains it through the ladder in the same call, the reference's
/// `0x6e74aa call 0x6e5a90` → `TryCast`: so this handler takes [`CastLadder`]
/// and [`CastTargeting`] where the others take [`Lifecycle`] — the five spell-state fields the
/// two would share are the ladder's — and reaches the rest through [`Reply`]. Before 2330 the
/// chain was an outbox a `ui_action` drain emptied after the input pass; now the chained cast
/// arms the in-flight slot before the next packet is handled, as in the reference.
///
/// The chained cast goes out at the **null target guid** — `0x6e74a6 push ebx; push ebx` with
/// `ebx = 0` — so it binds through the ordinary target walk (`ArmCast 0x6e5250`: main-hand item
/// bit, then the explicit guid, then the current selection), which is what
/// [`CastTargeting::context`] hands the ladder when no guid is passed. And it takes **every
/// rung**: the reference chains through `0x6e5a90` → `TryCast 0x6e4b60`, the same entry a button
/// press uses, so a chained Auto Shot is range-checked, form-checked and GCD-checked exactly like
/// a pressed one, and refuses with the same red line.
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
        let chained = cast_result(
            spell_id,
            success,
            reason,
            arg,
            &mut ladder.ecs,
            &r.self_guid,
            &r.index,
            &mut ladder.cast_errors,
            &r.casting,
            &mut r.cast_events,
            &mut r.cast_bar,
            &mut ladder.pending,
            &mut ladder.queued_melee,
            &mut ladder.cooldowns,
            &mut ladder.auto_repeat,
            ladder.spells.as_deref(),
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
            &mut l.cast_bar,
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
            &mut l.cast_bar,
            &mut l.pending,
            &mut l.queued_melee,
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
            &mut l.cast_bar,
            &mut l.pending,
            &mut l.queued_melee,
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
            &mut l.cast_bar,
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

/// The four cooldown packets, each resolved once through [`addressed_store`] — ours or the
/// pet's, by the caster guid.
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
        } => channel_start(spell_id, duration_ms, &mut l.channel, &mut l.cast_bar),
        SessionEvent::ChannelUpdate { remaining_ms } => {
            channel_update(remaining_ms, &mut l.channel, &mut l.cast_bar)
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

/// Which unit's cooldown store a wire cooldown packet addresses.
///
/// All four of them (`SMSG_SPELL_COOLDOWN`, `_COOLDOWN_EVENT`, `_CLEAR_COOLDOWN`,
/// `_COOLDOWN_CHEAT`) carry a caster guid, and until the pet bar existed all four answered it the
/// same way: "is it us? then apply, else drop" — four copies of a self-only assumption, each
/// inside its own arm. Since the server sends a pet's cooldowns on the pet's guid, that
/// assumption silently discarded every one of them. Resolving the guid ONCE, here, is what let the
/// pet bar sweep for real without a second copy of any arm; it also matches the reference, whose
/// `SMSG_COOLDOWN_CHEAT` handler wipes "the self/pet cooldown list" off exactly this test.
///
/// `None` = a guid we hold no store for (another player's pet, a stale packet): dropped, as the
/// client drops an unknown guid.
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

/// The player's spell book (`SMSG_INITIAL_SPELLS`, once at login) → the action store the UI feed
/// reads (`crate::ui_action`), plus the active-cooldown list → the cooldown store (the wire
/// carries *remaining* ms — [`Cooldowns::seed_initial`]'s law).
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

/// The player's saved action bar (`SMSG_ACTION_BUTTONS`, once at login, and again on server-side
/// edits) → the action store the UI feed reads.
fn action_buttons(buttons: Vec<ActionButton>, actions: &mut PlayerActions) {
    debug!("net: action bar — {} occupied slots", buttons.len());
    actions.buttons = buttons.into_iter().map(|b| (b.slot, b)).collect();
    actions.dirty = true;
}

/// A spell added to the book after login (`SMSG_LEARNED_SPELL` — a trainer purchase, a quest reward,
/// a level-up rank gain). The spellbook feed diffs `spells` each frame, so the insert
/// is all it needs to surface (the add-gate that decides which known spells are *book* entries is
/// the feed's). No action-bar change — learning a spell does not bar it.
///
/// It also **announces the learn in chat**, which is the reference's own tail on this packet and
/// not a nicety: `0x5e61c0` -> `AddSpell(id, slot, 1, 1)` -> the registrar `0x4b25b0` with its
/// announce flag set ([`announce_learn`]).
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

/// **The learn announcement** — the `ERR_LEARN_*` chat line the registrar `0x4b25b0` prints at
/// `0x4b2909` for a spell learned *mid-session*.
///
/// Which of the three lines, and whether the argText carries the rank, is
/// [`benilla_formats::SpellDisplay::learn_announcement`]'s — it is a `Spell.dbc` `Attributes` read
/// and its byte trail lives with the record. What is this side's is the mapping to catalog keys:
/// ids `0x37`/`0x38`/`0x39` are `MsgKind::Chat` rows carrying chat type `10`, so
/// [`crate::ui_action::UiErrorKeys`] puts all three on the chat window's system channel without
/// this call site naming a surface.
///
/// A missing catalog (`Spells` absent, a DBC that failed to load) says nothing at all, like every
/// other display path here — and so does an unknown id, which is the reference's own bounds/null
/// bail on the `Spell.dbc` store at the registrar's head (`0x4b25c6` / `0x4b25d2` / `0x4b25e0`,
/// all three jumping straight to the epilogue).
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
        // The recipe arm pushes the bare name — it returns before the rank composer.
        LearnAnnouncement::Recipe => ("ERR_LEARN_RECIPE_S", display.name.clone()),
    };
    errors.0.push(UiError::s(key, arg));
}

/// A spell taken back out of the book (`SMSG_REMOVED_SPELL`) — the inverse of
/// [`learned_spell`] above, and the packet a **talent wipe** arrives as: vmangos's `ResetTalents`
/// walks every talent of the class and calls `RemoveSpell` on every rank, each of which tails into
/// `Player::SendSpellRemoved`. Until this arm existed all of them were dropped, so the respec's
/// only visible effect was the points coming back — the talent window went on drawing the ranks it
/// had, because [`crate::ui_talent`] derives rank from exactly this set.
///
/// It **announces the unlearn in chat** — *"You have unlearned %s."* — under
/// [`benilla_formats::SpellDisplay::announces_unlearn`]'s four gates. 2243 claimed
/// the opposite here, on a byte citation, and was wrong: it bounded `RemoveSpell 0x5e9fe0` at the
/// `ret 0x8` at `0x5ea28f` and so never read the block at `0x5ea292`, which is a live branch
/// target past that `ret`. `0x5ea2ab push 0x14a` is that block, and `SMSG_REMOVED_SPELL`'s arm
/// (`0x5e43e3`) is the caller that reaches it with the flag set.
///
/// The insert's mirror image, and deliberately no more than that: the spellbook, talent and pet
/// feeds all diff `spells` and fire their own refresh events, and
/// [`crate::ui_action::LearnedAbilities`] re-derives off the same change (the reference's own
/// unlearn write site, `0x4b2c50`). What happens to a **bar button** still pointing at the removed
/// spell is a separate law this arm deliberately does not invent — see 1584's scope note.
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

/// **The unlearn announcement** — `ERR_SPELL_UNLEARNED_S` (`0x5ea2ab`), the counterpart to
/// [`announce_learn`] and deliberately not a mirror of it.
///
/// The argText is the **bare** name: `0x5ea2a3` pushes `SpellRec+0x1e0` and there is no rank
/// composer on this path at all, so a respec'd talent rank reads "You have unlearned Improved
/// Fireball." with no "(Rank 3)". Which spells say it is
/// [`benilla_formats::SpellDisplay::announces_unlearn`]'s — the bytes live with the record.
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

/// A rank-up (`SMSG_SUPERCEDED_SPELL`): the new rank replaces the old **in the book** (decision
/// 0237). The server sends no fresh `SMSG_ACTION_BUTTONS` for this (VERIFIED vmangos
/// `Player::learnSpell` — the `supercededOld` path touches only the spell store), so the bar has to
/// follow too; it does, from the book, in `ui_action::ranks`. Re-pointing buttons
/// *here* as well would be a second, weaker copy of that law — weaker because this packet doesn't
/// arrive at all when the rank was gained while the character was loading (vmangos suppresses it
/// with `IsInWorld()`), which is exactly the case that shipped a dead rank-1 button.
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
    // A rank-up announces exactly like a first learn, and only once: the supersede pair `0x4b2f50`
    // calls the unlearn `0x4b2c50` and then the registrar `0x4b25b0` with `mov edx,0x1`
    // (`0x4b2f61`), and the unlearn half holds no `DisplayError` call at all.
    announce_learn(new_spell_id, spells, errors);
    // The rank-up reaches the registrar with the same flag set (`0x4b2f61 mov edx,0x1`), so it
    // flashes the tab too — for the NEW rank, whose tab is the one it lands in (2252).
    tab_flash.0.push(new_spell_id);
}

/// The server's verdict on our cast (`SMSG_CAST_RESULT`).
fn cast_result(
    spell_id: u32,
    success: bool,
    reason: Option<u8>,
    arg: Option<u32>,
    commands: &mut Commands,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    cast_errors: &mut CastErrors,
    casting: &Query<&Casting>,
    cast_events: &mut MessageWriter<CastEvent>,
    cast_bar: &mut CastBarFeed,
    pending: &mut PendingCast,
    queued_melee: &mut QueuedMeleeSpell,
    cooldowns: &mut Cooldowns,
    auto_repeat: &mut AutoRepeatActive,
    spells: Option<&Spells>,
    net: &crate::net::NetCommands,
    seq: u64,
) -> Option<u32> {
    debug!("net: cast result — spell {spell_id} success={success} reason={reason:?}");
    // **Is this the reply to the cast we have outstanding?** (`0x6e7408 cmp ecx,[0xceca88]`.) Read
    // BEFORE either arm touches the guard, because both the failure arm's clear below and the
    // chain's own clear are the reference's ONE `0x6e741a call 0x6e4940(0x1c)` — the in-flight slot
    // is finished on either outcome, and only then is column 38 read.
    let in_flight = pending.committed(Instant::now()) == Some(spell_id);
    if !success {
        // The cast-fail cooldown edges (the client's `HandleCastFailed 0x6e1a00`): a plain
        // interactive-cast failure clears ONLY
        // the GCD armed at send (`0x6e1d83 → 0x6e1630`) — the spell's own recovery was never
        // started pre-launch (it lands at SPELL_GO / SMSG_SPELL_COOLDOWN, which a failed cast
        // never reaches), so there is nothing else to revert — and a failing auto-repeat spell
        // runs the FULL local cancel (`0x6e1cd9`–`0x6e1cea`): iff the failing spellId == the
        // cached auto-repeat spell AND
        // reason ≠ 0x17, the handler jumps into `0x6ea080` — the SAME routine the
        // SMSG_CANCEL_AUTO_REPEAT handler runs — clearing the key, the shooting-idle bits, and
        // the nocked ammo. A deselect/interrupt surfaces as this CAST_RESULT failure
        // (`HandleSetSelectionOpcode` → `Spell::cancel` → `SendCastResult(INTERRUPTED)`).
        // **Correction (2026-08-05):** this is NOT the only live disarm — the long-standing note
        // here that "vmangos never sends SMSG_CANCEL_AUTO_REPEAT (dead packet class, zero send
        // sites)" is FALSE. `SpellCaster::InterruptSpell` (vmangos `SpellCaster.cpp:1826`) calls
        // `Player::SendAutoRepeatCancel()` for every player autorepeat interrupt — target death
        // included (`Unit::_UpdateAutoRepeatSpell` → `CheckCast` fails → `InterruptSpell`) — so
        // [`cancel_auto_repeat`]'s handler is live, not dormant.
        let now = Instant::now();
        // Reason 0x17 DONT_REPORT exits the ref's handler at a bare epilogue BEFORE the GCD
        // clear and the display (`6e1ce1`/`6e1cf7 → 0x6e224f` — 0948's C4): a silent server
        // abort neither reopens the GCD nor prints. Our in-flight/bar bookkeeping below still
        // runs (vmangos uses DONT_REPORT for real aborts whose guard must open).
        let dont_report = reason == Some(0x17);
        if !dont_report {
            cooldowns.clear_gcd(spell_id, now);
            // The handler tail's bit25 full revert (`6e73cc–6e73e6`): a non-0x3c failure of a
            // cooldown-on-event spell force-removes its records — the parked insert a failed
            // Feign Death / Stealth would otherwise leave behind forever.
            if reason != Some(0x3c)
                && spells
                    .and_then(|s| s.catalog.get(spell_id))
                    .is_some_and(|d| d.cooldown_on_event())
            {
                cooldowns.clear_spell(spell_id);
            }
        }
        let self_e = self_guid.0.and_then(|g| index.0.get(&g)).copied();
        if auto_repeat.0 == Some(spell_id) && reason != Some(0x17) {
            crate::creature_anim::cancel_auto_repeat_local(self_e, auto_repeat, commands, net);
        }
        // Does this failure belong to the cast the bar is showing? Keyed to our in-flight
        // `Casting`, like every reap. A pre-start rejection (no `Casting` — LoS, out of range,
        // silenced, or the send-guard's own duplicate that no longer reaches us) never opened a
        // bar; a proc's failure names a different spell. Neither may red-fade the running bar.
        let fails_our_cast =
            self_e.is_some_and(|e| casting.get(e).is_ok_and(|c| c.spell_id == spell_id));
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV CAST_RESULT failure — spell {spell_id} reason={reason:?}; fails_bar={fails_our_cast}");
        }
        // Open the in-flight guard regardless (the send-guard means the spurious "duplicate
        // rejected" failure no longer arrives here at all). A queued on-next-swing strike dies
        // the same way — the server's melee-slot interrupt (target death, cancel, replacement:
        // vmangos `Spell::cancel` on the PREPARING slot) lands here as a failure.
        pending.clear_if(spell_id);
        queued_melee.clear_if(spell_id);
        // The red error line rides `CastErrors` independently of the bar — a pre-start failure
        // still shows "out of range"/"line of sight" even though no bar is (or should be) up.
        if let Some(reason) = reason {
            if !dont_report {
                cast_errors.0.push(crate::ui_action::CastFail {
                    spell_id,
                    reason,
                    arg,
                    // `SMSG_CAST_FAILED` is addressed to the caster, and this handler is ours.
                    caster: crate::ui_action::Caster::Player,
                    redisplay: false,
                });
            }
        }
        // The cast bar's red "Failed" — only the showing cast's own failure turns it red.
        if fails_our_cast {
            cast_bar.0.push(CastBarEdge::Failed);
        }
        // Our own cast died (`SMSG_SPELL_FAILURE` is never sent): end the self
        // avatar's cast state + precast hold, spell-id-keyed like every reap.
        if let Some(e) = self_e {
            if fails_our_cast {
                commands.entity(e).remove::<Casting>();
            }
            cast_events.write(CastEvent {
                entity: e,
                spell_id,
                kind: CastEventKind::Fail,
                seq,
            });
        }
    }
    // **The `modalNextSpell` chain — how a hunter starts shooting** (`HandleCastResult 0x6e7330`
    // @ `0x6e7408`–`0x6e74aa`).
    //
    // The reply to our in-flight cast finishes that cast's slot and then reads **column 38** of
    // the spell it names. Non-zero, and not already the running repeat ⇒ the client casts it,
    // itself, at the null target guid. Every hunter shot's column 38 is **75, Auto Shot** — so
    // casting Serpent Sting starts Auto Shot one round-trip later, with no input and no addon.
    //
    // Three details, each load-bearing:
    // - **It fires on SUCCESS as well as failure.** `0x6e7356 cmp [ebp+0xf],0x2` / `0x6e735a jne`
    //   sends a non-failure result *straight* to the block the failure path falls into at
    //   `0x6e73eb`. That is the whole reason it was missed: the success arm of this handler used
    //   to do nothing at all here, and a successful sting is the ordinary case.
    // - **The slot is cleared first.** The reference's `0x6e741a` finishes the in-flight cast
    //   before chaining, and it must: `TryCast`'s own IsCasting rung (`0x6e4d97`, our reason
    //   `0x61`) would refuse the chained cast otherwise. Ours is [`PendingCast`], cleared here on
    //   both outcomes for the matching spell — where before, success left it to `SPELL_GO`.
    //   vmangos sends `SMSG_CAST_RESULT` before `SMSG_SPELL_GO` (`Spell::cast`: `SendCastResult`
    //   at 3669, `SendSpellGo` at 3703), so the guard is still armed when we get here.
    // - **Equal means re-arm, not re-cast** (`0x6e745b`'s equal branch → `0x6e745d`): a second
    //   sting while Auto Shot is already running must NOT re-cast it, or every special shot would
    //   restart the repeat and reset its swing timer. We have no pending-record to refresh, so the
    //   equal branch is simply "send nothing" — the same observable.
    //
    // The chained spell is this function's **return**: `on_cast_result` hands it to the ladder
    // in the same call (`0x6e74aa call 0x6e5a90`), so the chained cast takes every rung a press
    // takes and arms the in-flight slot before the next packet is handled.
    if in_flight {
        pending.clear_if(spell_id);
        if let Some(next) = spells
            .and_then(|s| s.catalog.get(spell_id))
            .map(|d| d.modal_next_spell)
            .filter(|&next| next != 0 && Some(next) != auto_repeat.0)
        {
            debug!("net: cast result {spell_id} chains modalNextSpell {next}");
            return Some(next);
        }
    }
    None
}

/// A unit began a non-triggered cast (`SMSG_SPELL_START`), instants included (`cast_time_ms == 0`)
/// — the precast trigger the phase-2 casting animation loop builds on (decision 0099 phase 1).
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
    cast_bar: &mut CastBarFeed,
    pending: &mut PendingCast,
    spells: Option<&crate::ui_action::Spells>,
    seq: u64,
) {
    // The precast trigger (decision 0099 phase 1): a nonzero cast time seeds the
    // `Casting` state seam phase 2's precast loop reads; an instant (timer 0) gets no
    // component — its `SpellGo` follows immediately with nothing to interrupt. No
    // animation consumer yet.
    debug!(
        "net: spell start {spell_id} by {caster:#x} ({cast_time_ms}ms, flags \
         {cast_flags:#x}, target {target:?}, ammo {ammo_display_id:?})"
    );
    // The nocked-ammo refresh (the client's `0x60ba30` @ `0x6e78b6`, gated `SpellRec+0x20&0x20 OR
    // +0x18&0x2` at `0x6e78a1`, on the packet-resolved caster BEFORE the self/other split — any
    // unit): a ranged spell's START either affirms the wire's
    // ammo display id or, with the flag clear / id 0, detaches. The model persists through
    // Load/Hold and the fire clip — `SPELL_GO` never touches it.
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
    // The cast bar's SECOND gate (the client's `0x6e7700`: fire SPELLCAST_START only on
    // `cast_time > 0 && !(SpellRec+0x18 & 2)` — byte-verified, closes decision 0256's "open —
    // the wire feed" item 1): a ranged-slot spell never opens a bar, whatever cast time the
    // server sent. Concretely: vmangos pads every non-auto-repeat ranged spell's cast by a flat
    // +500 ms (`SpellEntry::GetCastTime`), so Throw arrives as a real 500 ms cast — the ref
    // shows no bar for it, and now neither do we. An unknown spell (no catalog,
    // no row) keeps the bar: only a *known ranged* row suppresses.
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
    // Our own timed cast opens the cast bar (an instant shows no bar).
    if self_guid.0 == Some(caster) && cast_time_ms > 0 {
        if !ranged_slot {
            cast_bar.0.push(CastBarEdge::Start {
                spell_id,
                cast_time_ms,
            });
        }
        // The server named the real cast time — tighten the in-flight guard's safety deadline
        // from the send-time provisional to it (the guard is cleared for real by the GO/fail).
        // Ranged too: the in-flight guard is cast-tracking, not bar UI.
        pending.refine(cast_time_ms, Instant::now());
    }
    if let Some(&e) = index.0.get(&caster) {
        if cast_time_ms > 0 {
            commands.entity(e).insert(Casting {
                spell_id,
                until: Some(Instant::now() + Duration::from_millis(u64::from(cast_time_ms))),
            });
        }
        // The anim layer's precast edge — instants included: their
        // GO follows at once and reaps the (subliminal) hold, like the client's
        // stage-4 persist/reap pair.
        cast_events.write(CastEvent {
            entity: e,
            spell_id,
            kind: CastEventKind::Start,
            seq,
        });
    }
}

/// `GAMEOBJECT_TYPE_ID` 3 — `CHEST`, the only GO type whose `OPEN_LOCK` cast arms the loot-target
/// latch (`0x6e830c`; DOOR(0)/BUTTON(1) are skipped explicitly and every other type falls out).
const GO_TYPE_CHEST: i32 = 3;

/// The cast launched (`SMSG_SPELL_GO`): hit/miss lists + (for a ranged spell) the ammo display id
/// for the projectile visual. The server schedules the *damage* itself off `Spell.dbc` Speed —
/// nothing about missile travel rides this packet; the client (and we) rebuild the flight
/// visually from the same Speed column (decision 0099 phase 4: the target lists go out as
/// [`SpellGoTargets`] for the router's instant-impact/missile branch).
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
    cast_bar: &mut CastBarFeed,
    pending: &mut PendingCast,
    queued_melee: &mut QueuedMeleeSpell,
    text: &mut MessageWriter<crate::combat_text::CombatTextSpawn>,
    // The three floating-text CVars, read inside the word emitter `0x607140` itself — so they
    // gate the miss words below exactly as they gate a damage number.
    text_gates: crate::combat_text::DamageTextGates,
    go_lid: &mut MessageWriter<crate::go_anim::GoLidOpen>,
    // The client-local loot-target latch — armed here for a chest (`0x6e831b`).
    loot_latch: &mut crate::ui_loot::LootLatch,
    // The cooldown store + what its start laws read (grouped: one arm-body concern). The last
    // member is the PET's store — the reference inserts into two banks from this one handler
    // ([`pet_go_cooldown`]).
    cooldown_ctx: (
        &mut Cooldowns,
        Option<&Spells>,
        &mut crate::items::Items,
        &crate::net::NetCommands,
        &mut crate::ui_pet::PetBar,
    ),
    // The GO-deferred melee auto-attack start's write set (`0x6e83c0`, the arm below), plus the
    // attack lock it gates on: our server-echoed `Engaged`, the ref's `[player+0xc48]`.
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
    // **The interact chain's third link** (tag `use`, the same one `target::click` writes): the cast
    // the server answered a `CMSG_GAMEOBJ_USE` with. `caster_indexed=false` is the load-bearing
    // case — every impact, sound and effect model below hangs off `index.0.get(&caster)`, so a
    // caster we never streamed drops the whole visual silently. A GameObject IS the caster for a
    // SPELLCASTER-type object (vmangos leaves `spellCaster = this` for type 22), and it reaches
    // here as a real guid only because the decode seam resolves the pair: vmangos writes the caster
    // slot from a null `m_casterUnit` for such a cast and the wire carries guid **0**
    // (`benilla_protocol::events`' `spell_caster`).
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
    // A cast that names a GameObject (an open-lock cast on a chest / locked door) hands off to the GO
    // animation driver, which gates on the open-lock effect and opens the lid on the cast going off.
    // Independent of the caster being streamed to us — an observed open still animates.
    if let Some(go_guid) = go_target {
        go_lid.write(crate::go_anim::GoLidOpen { go_guid, spell_id });
    }
    let (cooldowns, spells, items, net_commands, pet_bar) = cooldown_ctx;
    let (auto_repeat, sheath, engaged) = attack_ctx;
    let now = Instant::now();
    let display = spells.and_then(|s| s.catalog.get(spell_id));
    // **A chest's loot-target arm** (decision
    // 1477). `Spell_C::HandleSpellGo 0x6e7a70` reaches `0x6e831b call SetLootTarget 0x5ed5f0`,
    // which writes `[player+0x1d28]` and force-plays Loot 50 — so **this packet, not the loot
    // response, is when the reference starts kneeling at a chest**. Its gates, transcribed:
    // caster is the local player (`0x6e81b6`), the spell carries `SPELL_EFFECT_OPEN_LOCK` (0x21)
    // or `OPEN_LOCK_ITEM` (0x3b) (`0x6e81f9`/`0x6e8202`), and the single target resolves as a
    // GameObject whose `GAMEOBJECT_TYPE_ID` is **CHEST(3)** (`0x6e82df`–`0x6e830c`; DOOR/BUTTON
    // are skipped explicitly, everything else falls out).
    //
    // One named divergence: the reference reads its target off the hit list and requires
    // `hitCount == 1` (`0x6e82c1`). vmangos writes a GameObject target into `SpellCastTargets`,
    // not the hit list, so we key on the packet's `go_target` — a single guid by construction,
    // which is the same condition arrived at from the shape of our wire rather than from a count.
    //
    // No force-play is needed here: our loot leg is recomputed every frame from
    // [`crate::ui_loot::LootKneel`], where the reference recomputes only on events and therefore
    // has to kick the pose by hand.
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
    // Our own launch completes the cast bar (a shown bar fills green and fades; auto-repeat
    // shots — GO with no bar showing — no-op in the reference Lua) and opens the in-flight guard
    // (spell-id-keyed, so a triggered proc's GO mid-cast doesn't unblock the running cast early).
    if self_guid.0 == Some(caster) {
        // Only the cast the bar is actually showing may complete it. A triggered proc that lands
        // mid-cast — Frost Armor's Chilled (6136), a weapon proc — is *our own* cast, so its
        // `SMSG_SPELL_GO` arrives here too; pushing an unconditional STOP fired a spurious
        // SPELLCAST_STOP that finished the running bar early (the observed "the bar vanishes when a
        // mob hits me, the spell fires a moment later"). Key it to our in-flight `Casting`, exactly
        // like the spell-id reap below.
        let completes_our_cast = index
            .0
            .get(&caster)
            .is_some_and(|&e| casting.get(e).is_ok_and(|c| c.spell_id == spell_id));
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV SPELL_GO — spell {spell_id} (self); completes_bar={completes_our_cast}");
        }
        if completes_our_cast {
            cast_bar.0.push(CastBarEdge::Stop);
        }
        pending.clear_if(spell_id);
        // The queued on-next-swing strike fired on this swing — the queue (and its checked
        // ring) opens exactly here, like the ref's inflight finish on the matching GO.
        queued_melee.clear_if(spell_id);

        // **The GO-deferred melee auto-attack start** (`HandleSpellGo 0x6e7a70` @ `0x6e83c0`;
        // bytes re-read for decision 1593). This is the exact
        // complement of the send-time tail in [`crate::spell::cast_send`]: a spell carrying
        // `AttributesEx2 & 0x100000` has its optimistic start *suppressed* there and armed here
        // instead, so the swing begins only once the server confirms the strike landed. That is
        // the whole 5875 stealth-opener class — Backstab, Garrote, Ambush, Cheap Shot, Shred,
        // Ravage, Pounce — plus Judgement, none of which started an auto-attack in benilla at all
        // before this (the deferred path was left unbuilt on a ten-row proof by absence; the real
        // file carries the bit on 36 rows, censused in `benilla-formats`' `catalog_tests`).
        //
        // **The target** is the reference's, verbatim (`0x6e83e9`–`0x6e83fe`): `hits[0]` when the
        // GO carries a hit list, else the null pair — which `0x612df0` resolves to the current
        // selection and, failing that, acquires as the nearest hostile. We take the packet's own
        // target guid as that fallback and stop there: the acquire-nearest leg is `target::scan`'s
        // and reaching it from here would target something the player never named. A hostile
        // single-target strike always fills the hit list, so the fallback is the quiet case.
        //
        // **The gate** is `[player+0xc48] == 0` (`0x6e83e7`) — already swinging, nothing happens,
        // which is also what keeps the auto-repeat cancel in `start_attack_local`'s tail from
        // firing on every strike of a fight.
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

        // Our own launch starts the cast's cooldown locally, at the GO: `HandleSpellGo 0x6e7a70`'s
        // self-insert tail forks on the packet guid pair — itemCaster == caster ⇒ the NO-ITEM
        // spell leg (`0x6e8498`: SpellRec RecoveryTime/Category/CategoryRecoveryTime, onHold from
        // Attributes bit 25, start = the GO receive-time); an item cast takes the item leg
        // (`0x6e8566`, per-slot values with SpellRec fallbacks; a non-resident row pends via
        // `0x6e8660` → `0x6e8830`). `SMSG_SPELL_COOLDOWN` is the server OVERRIDE path, not the
        // normal source — this insert is how a Charge sweep appears on vmangos, which sends no
        // cooldown packet for a plain cast. A pre-launch failure never reaches here, so nothing
        // needs reverting.
        // The ranged-shot pad (the category scaler `0x6e2b60`): a ranged-slot cast
        // folds our live
        // `UNIT_FIELD_RANGEDATTACKTIME` (haste-scaled, server-written) into the category
        // recovery — the Throw/wand-Shoot button sweep, no server packet involved.
        let ranged_ms = display
            .filter(|d| d.ranged_speed_cooldown())
            .and_then(|_| {
                let e = index.0.get(&caster)?;
                stores.get(*e).ok()?.0.unit_ranged_attack_time()
            })
            .unwrap_or(0);
        // The item's entry off its own store — an item is an object in the index (2334).
        match item_caster.and_then(|g| stores.get(*index.0.get(&g)?).ok()?.0.object_entry()) {
            Some(entry) => {
                let use_spell = items
                    .template(entry, 0, net_commands)
                    .and_then(|t| t.use_spell)
                    .filter(|u| u.spell_id == spell_id);
                match use_spell {
                    Some(u) => cooldowns.start_item(entry, &u, display, now),
                    // The template hasn't streamed (or names a different spell): fall back to
                    // the spell-keyed record so the sweep still runs.
                    None => {
                        if let Some(d) = display {
                            cooldowns.start_spell(spell_id, d, ranged_ms, now);
                        }
                    }
                }
            }
            None => {
                if let Some(d) = display {
                    cooldowns.start_spell(spell_id, d, ranged_ms, now);
                }
            }
        }
        if benilla_assets::trace::enabled() {
            if let Some(d) = display {
                benilla_assets::trace::line(
                    "cd",
                    &format!(
                        "arm spell={spell_id} rec={}ms cat={}:{}ms (GO self-insert)",
                        d.recovery_ms, d.category, d.category_recovery_ms
                    ),
                );
            }
        }
    }
    // **The PET leg of the same insert** — a second, independent `if` in the very
    // same handler, not an else of the one above.
    if let Some(d) = display.filter(|_| pet_go_cooldown(caster, self_guid, index, stores)) {
        // No ranged pad here: `0x6e2b60` is called on the self leg only (`0x6e845d`), and there is
        // nothing on a pet to read a `UNIT_FIELD_RANGEDATTACKTIME` from that the client uses.
        pet_bar.cooldowns.start_spell(spell_id, d, 0, now);
        // `0x6e85fc` + `0x6e8601`: SPELL_UPDATE_COOLDOWN and PET_BAR_UPDATE_COOLDOWN. benilla
        // collapses both into the pet bar's one repaint event, which the feed fires off its own
        // diff — and the diff moves, because the slot's cooldown triple just changed. The signal
        // bump is belt-and-braces for the case where the same spell re-arms to an identical
        // triple (a zero-length re-cast), which the diff would otherwise swallow.
        pet_bar.bar_signals = pet_bar.bar_signals.wrapping_add(1);
        if benilla_assets::trace::enabled() {
            benilla_assets::trace::line(
                "cd",
                &format!(
                    "arm spell={spell_id} rec={}ms cat={}:{}ms (GO pet-insert)",
                    d.recovery_ms, d.category, d.category_recovery_ms
                ),
            );
        }
    }
    // The miss list's floating words (0137 phase 2, the `0x6e7a70` handler): one outcome word
    // over each missed target — except REFLECT, which re-anchors to the caster (`0x6e7e51`).
    // Gate A applies to whichever unit the word lands over. Source-classified first (the color
    // law's K, inside every emitter): another caster's misses draw nothing.
    //
    // **Two laws phase 2 left open, both closed by 2229.**
    //
    // *Timing* — `0x6e7d4e fld [SpellRec+0x94]; fcomp 0.0; test ah,0x44; jp 0x6e7e71` skips this
    // inline emit whenever `Spell.dbc` Speed is nonzero. A TRAVELLING spell's word is floated by
    // the projectile's own arrival instead ([`crate::entities::MissileMiss`]), so a resisted
    // Fireball reads "Resist" when the ball lands; a Speed-0 ability prints here, now. No
    // catalog degrades to printing here, the way a NULL record degrades everywhere else.
    //
    // *Colour* — `0x6e7d73 mov edi,[ebp-0x8]` / `0x6e7dcc push edi` pushes the resolved SpellRec,
    // not NULL, so the word runs the same B/K override as a number: a bit-15-clear spell's "Miss"
    // is spell-GOLD. The three CVar gates come with it — they are read inside `0x607140`, which
    // every word path calls, so `CombatDamage 0` silences these words too.
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
    // Keyed by spell id, like the client's reap `0x614150(spellId, 0)`: a triggered proc's GO
    // landing mid-cast must not clear a *different* spell's precast state.
    if let Some(&e) = index.0.get(&caster) {
        if casting.get(e).is_ok_and(|c| c.spell_id == spell_id) {
            commands.entity(e).remove::<Casting>();
        }
        // The GO's own chain-hop fill (`0x6e800d`, inside `HandleSpellGo`) — the **second**
        // producer of `unit+0xd44`, and the one that makes a non-channelled chain spell draw at
        // all. It must precede the CastEvent below: the router plays the cast kit off that event,
        // and the kit's chain proc consumes this array the same frame. Named approximation: the
        // reference gates this leg on `0x6e4870`'s return, a predicate still unsettled —
        // we fill unconditionally, which is harmless because consumption still needs a chain proc
        // and because every producer clears before it fills.
        fill_chain_hops(caster, e, &hits, commands, index);
        cast_events.write(CastEvent {
            entity: e,
            spell_id,
            kind: CastEventKind::Go,
            seq,
        });
        // The impact side: targets not streamed to us (out of range) just drop out of the
        // lists — their impacts are invisible anyway. The miss code rides along: the missile's
        // arrival plays the victim's dodge/block clip off it (the deflect *flight* visual stays
        // a later refinement).
        let hits: Vec<Entity> = hits
            .iter()
            .filter_map(|g| index.0.get(g).copied())
            .collect();
        let misses: Vec<(Entity, u8)> = misses
            .iter()
            .filter_map(|&(g, code)| index.0.get(&g).map(|&e| (e, code)))
            .collect();
        // A pure dest cast (ground AOE — empty hit/miss lists) rides the same message: the
        // point is a target too (the B132 follow-up; the launch-side dest visual is the
        // router's to resolve).
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

/// An observed cast was interrupted/cancelled (`SMSG_SPELL_FAILED_OTHER`) — ends the caster's
/// `Casting` state seam the same as [`spell_go`].
fn spell_failed_other(
    caster: u64,
    spell_id: u32,
    commands: &mut Commands,
    index: &GuidIndex,
    casting: &Query<&Casting>,
    cast_events: &mut MessageWriter<CastEvent>,
    self_guid: &SelfGuid,
    cast_bar: &mut CastBarFeed,
    pending: &mut PendingCast,
    queued_melee: &mut QueuedMeleeSpell,
    seq: u64,
) {
    debug!("net: spell failed (other) {spell_id} by {caster:#x}");
    // Our own in-flight cast was interrupted — the bar turns red "Interrupted",
    // but only for the cast the bar is actually showing (keyed to `Casting`, like the reap): a
    // proc's own interrupt must not red-fade a different running bar. The guard opens the same way.
    if self_guid.0 == Some(caster) {
        let interrupts_our_cast = index
            .0
            .get(&caster)
            .is_some_and(|&e| casting.get(e).is_ok_and(|c| c.spell_id == spell_id));
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV SPELL_FAILED_OTHER — spell {spell_id} (self); interrupts_bar={interrupts_our_cast}");
        }
        if interrupts_our_cast {
            cast_bar.0.push(CastBarEdge::Interrupted);
        }
        pending.clear_if(spell_id);
        // The melee-slot interrupt's other half (it arrives beside the failing CAST_RESULT).
        queued_melee.clear_if(spell_id);
    }
    // Spell-id-keyed like the GO reap (the 0x2A6 handler's `0x614150(spellId, 0)`).
    if let Some(&e) = index.0.get(&caster) {
        if casting.get(e).is_ok_and(|c| c.spell_id == spell_id) {
            commands.entity(e).remove::<Casting>();
        }
        cast_events.write(CastEvent {
            entity: e,
            spell_id,
            kind: CastEventKind::Fail,
            seq,
        });
    }
}

/// `SMSG_SPELL_DELAYED` — pushback: our own cast took damage and the server extended its timer by
/// `delay_ms` (vmangos `Spell::Delayed`; a normal hit never interrupts a cast, it pushes it back).
/// The cast bar slides its window out by the same (`SPELLCAST_DELAYED`, the reference Lua's
/// `startTime`/`maxValue` shift — the spark jumps back and the bar keeps running), so a hit no
/// longer lets the bar finish early while the real cast runs on. Self-only on the
/// wire, but the caster guid is on the packet — gate on it like every other own-cast edge.
fn spell_delayed(
    caster: u64,
    delay_ms: u32,
    self_guid: &SelfGuid,
    cast_bar: &mut CastBarFeed,
    pending: &mut PendingCast,
) {
    debug!("net: spell delayed by {caster:#x} (+{delay_ms}ms pushback)");
    if self_guid.0 == Some(caster) {
        if *crate::net::CAST_TRACE {
            info!("cast-trace: RECV SPELL_DELAYED — +{delay_ms}ms pushback (bar extends, does NOT vanish)");
        }
        cast_bar.0.push(CastBarEdge::Delayed { delay_ms });
        // Keep the in-flight guard (decision 0256 / the spam fix) holding past the stretched end.
        pending.delay(delay_ms, Instant::now());
    }
}

/// `SMSG_CANCEL_AUTO_REPEAT` — the client's handler (`0x6e99d0` → `0x6ea080`) clears the
/// autorepeat key `0xceac30`, which is exactly what the action
/// bar's flash/checked state reads — so the button's auto-repeat highlight goes out
/// (`STOP_AUTOREPEAT_SPELL` fires off this edge in the UI feed). The shots themselves were
/// always wire-paced (decision 0099 phase 5: every shot is its own `SPELL_GO`), and nothing
/// stows (sheath-policy's "nothing sheathes on combat-end").
///
/// **INTERIM:** the cancel also disarms the standing
/// Load/Hold idle ([`crate::creature_anim::AutoRepeatArmed`] off) — the director's report: on
/// the reference the shooting visibly STOPS when the server cancels (target too close), while
/// our sticky arm kept the nock idle looping forever. Whether the real handler clears the
/// `[+0xd58] & 0x200` idle bit (0131 recorded "no clearing writer") or the hold-pose layer
/// merely makes the ref look still is open; this is corrected if the mechanism differs.
///
/// **Live against vmangos, not dormant (corrected 2026-08-05).** The prior note here — "vmangos
/// never sends this" — was wrong: `SpellCaster::InterruptSpell` (`SpellCaster.cpp:1826`) sends it
/// on every player autorepeat interrupt, which is how **target death** stops a volley
/// (`Unit::_UpdateAutoRepeatSpell` → `CheckCast(true)` returns dead/bad-targets →
/// `InterruptSpell(CURRENT_AUTOREPEAT_SPELL)` → `SendAutoRepeatCancel`). Movement does NOT
/// (`_UpdateAutoRepeatSpell` interrupts only Category 351, the wand, when moving) — Auto Shot
/// stays armed across a run, exactly like vanilla.
fn cancel_auto_repeat(
    auto_repeat: &mut AutoRepeatActive,
    self_guid: &SelfGuid,
    index: &GuidIndex,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    debug!("net: cancel auto-repeat");
    // The packet thunk `0x6e99d0` funnels into the same cancel `0x6ea080` as every local
    // trigger — including its CMSG ack (unconditional inside the routine). Live against vmangos,
    // not dormant: the doc block above has the send site and the target-death path to it.
    let self_e = self_guid.0.and_then(|g| index.0.get(&g)).copied();
    crate::creature_anim::cancel_auto_repeat_local(self_e, auto_repeat, commands, net);
}

/// **Does this `SMSG_SPELL_GO` arm the PET's cooldown bank?** (`0x6e857a`-`0x6e85ad`.)
///
/// The reference's GO handler makes two independent cooldown inserts, into two banks
/// (`0x6e2ea0`'s `bankHead = 0xcecaec + 24*bank`): the self leg above writes bank 0 at
/// `0x6e8493 mov ecx, 0xcecaec`, and this one writes **bank 1** at `0x6e85f2 mov ecx, 0xcecb04` —
/// the same bank `SMSG_PET_SPELLS`' cooldown tail seeds (`0x4bdaa8 push 1`) and the same bank
/// `GetPetActionCooldown` and `GetSpellCooldown(id, "pet")` read. One packet can arm both.
///
/// The gate is **ownership of the caster**, read off the caster's own descriptor and nothing else
/// — not the pet-bar guid, not `UNIT_FIELD_PETNUMBER`:
///
/// ```text
/// 0x6e858b  edi = fields[0x14] ; edx = fields[0x10]     ; CHARMEDBY as a 64-bit pair
/// 0x6e8594  or edx, edi ; jne -> edi = &fields[0x10]
/// 0x6e859a  else            edi = &fields[0x18]         ; SUMMONEDBY
/// 0x6e859d  call 0x468550                               ; the active player's guid
/// 0x6e85a2  if (*edi, *(edi+4)) != that guid -> skip
/// ```
///
/// i.e. `charmedBy` when set, else **`summonedBy`** — `OwnerFallback::SummonedBy`, the pair the
/// `PET_ATTACK_*` callback uses, and **not** `0x5ee5a0`'s `createdBy` fallback. Picking the wrong
/// one would arm the bank for totems and miss real pets: exactly the silent failure that enum
/// exists to prevent.
///
/// **Why this leg matters at all on vmangos**: the server sends *no* cooldown packet for a pet's
/// own cast. `Creature::AddCooldown` (`Objects/Creature.cpp:3259-3282`) stores the cooldown and
/// returns; its one `SendSpellCooldown` call sits in the `else` branch, reached only by a
/// **charmed non-pet** casting an instant under mind control. So without this insert a hunter's
/// Growl or a warlock imp's Firebolt shows no sweep at all until the next `SMSG_PET_SPELLS`
/// reseeds the bank — and that is a summon, a swap or a learned spell, never a cast.
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

/// `SMSG_SPELL_COOLDOWN` (`0x6e9460`) — server-pushed cooldowns (school lockouts, and the pet's
/// own list). **Which unit's store** it lands in is the caller's decision
/// ([`super::addressed_store`]): the four cooldown packets all carry a caster guid, and resolving
/// it in one place is what let the pet bar have real cooldowns without a second copy of this arm.
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

/// `SMSG_ITEM_COOLDOWN` (`0x6e95d0`) — the fixed 30 s use cooldown, keyed on the item instance's
/// template entry (the client resolves the guid to its item record the same way).
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

/// `SMSG_COOLDOWN_EVENT` (`0x6e9670` → `0x6e3050(force=0)`) — start an on-hold cooldown's parked
/// timers now (Stealth ends, Feign Death drops). Store chosen by [`super::addressed_store`].
fn cooldown_event(spell_id: u32, caster: u64, cooldowns: &mut Cooldowns) {
    debug!("net: cooldown event — spell {spell_id} on {caster:#x}");
    cooldowns.cooldown_event(spell_id, Instant::now());
}

/// `SMSG_CLEAR_COOLDOWN` (`0x6e9670` → `0x6e3050(force=1)`) — remove the spell's record outright.
fn clear_cooldown(spell_id: u32, caster: u64, cooldowns: &mut Cooldowns) {
    debug!("net: clear cooldown — spell {spell_id} on {caster:#x}");
    cooldowns.clear_spell(spell_id);
}

/// `SMSG_COOLDOWN_CHEAT` (`0x6e9730` → `0x6e9700`) — the GM reset wipes the whole list. The
/// reference's own handler wipes "the self/pet cooldown list" on a guid match, which is exactly
/// what routing through [`super::addressed_store`] now reproduces.
fn cooldown_cheat(caster: u64, cooldowns: &mut Cooldowns) {
    debug!("net: cooldown cheat (wipe) for {caster:#x}");
    cooldowns.wipe();
}

/// `MSG_CHANNEL_START` — self-only on the wire (no guid), so it goes straight to the cast bar; the
/// channel *animation* state rides the unit-field pair instead.
fn channel_start(
    spell_id: u32,
    duration_ms: u32,
    channel: &mut ActiveChannel,
    feed: &mut CastBarFeed,
) {
    channel.start(spell_id, duration_ms, Instant::now());
    feed.0.push(CastBarEdge::ChannelStart {
        spell_id,
        duration_ms,
    });
}

/// `MSG_CHANNEL_UPDATE` — the running channel's remaining time (`0` is its stop edge).
fn channel_update(remaining_ms: u32, channel: &mut ActiveChannel, feed: &mut CastBarFeed) {
    channel.update(remaining_ms, Instant::now());
    feed.0.push(CastBarEdge::ChannelUpdate { remaining_ms });
}

/// `SMSG_UPDATE_AURA_DURATION` — one of our own auras' remaining time, keyed
/// by raw slot and stamped with the receive time. The `ui_aura` feed joins it to the aura in that
/// slot by arrival order; it arrives *before* the descriptor delta that names the slot.
fn aura_duration(slot: u8, remaining_ms: u32, durations: &mut AuraDurations, now_secs: f64) {
    durations.set(slot, remaining_ms, now_secs);
}

/// One cell of one talent spell-modifier table (`SMSG_SET_FLAT_SPELL_MODIFIER` /
/// `SMSG_SET_PCT_SPELL_MODIFIER`) — `HandleSetSpellModifier 0x6e9950`'s whole body, which is a
/// single store: the server sends the absolute value of that `(family bit, op)` pair, never a
/// delta, so there is nothing to accumulate and nothing to invalidate.
///
/// The out-of-range refusal lives on the store ([`crate::spell::SpellModifiers::set`], which
/// documents why it is ours and not the reference's).
fn set_spell_modifier(
    flat: bool,
    mask_bit: u8,
    op: u8,
    value: i32,
    mods: &mut crate::spell::SpellModifiers,
) {
    mods.set(flat, mask_bit, op, value);
}

/// The caster's chain-target hop array, filled from a wire target list — the reference's
/// `0x605780`: it **clears before it fills** and **skips any entry equal to the
/// unit's own guid** (`0x6057bf`/`0x6057c9`). Targets not streamed to us drop out here: an endpoint
/// we cannot place has nothing to draw to, which is what the reference's own hidden-hop path
/// amounts to.
///
/// Both producers land here — this is the shared body, not a helper: `0x605780` has exactly two
/// callers image-wide, `SMSG_SPELL_UPDATE_CHAIN_TARGETS`'s handler and `HandleSpellGo`.
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

/// `SMSG_SPELL_UPDATE_CHAIN_TARGETS` → the hop list a **beam** visual runs through.
///
/// The reference parks it on the caster (the growable array at `unit+0xd44`: capacity `+0xd44`,
/// count `+0xd48`, data `+0xd4c`, alloc quantum `+0xd50`), and the next chain `CharProc` consumes
/// it once — `0x60db72` zeroes the count on every path. This packet is one of **two** producers:
/// the reference fills the same array from `SMSG_SPELL_GO`'s hit list as well (`0x6e800d` inside
/// its GO handler), which is how a non-channeled chain spell draws at all. vmangos sends this one
/// only for channelled spells (`Spell::SendChannelStart`), so on this server it is the drains'
/// and Mind Flay's producer, and the GO leg is every other chain spell's.
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
    use benilla_protocol::messages::ACTION_KIND_SPELL;

    /// A catalog holding one row, so the announce tests can name a real `Attributes` value.
    fn catalog_with(id: u32, display: benilla_formats::SpellDisplay) -> Spells {
        let mut spells = Spells::empty_for_tests();
        spells.catalog =
            benilla_formats::SpellCatalog::from_displays([(id, display)].into_iter().collect());
        spells
    }

    /// The report this arm was missing entirely: training at a class trainer printed nothing in
    /// chat. Heroic Strike carries `SPELL_ATTR_ABILITY`, so the reference's `0x4b29b3 add eax,0x37`
    /// lands on `0x38` — the *ability* wording — and the argText is the client's `"%s (%s)"`.
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
        // The same live-mutation flag gates the tab flash, so the queue takes it too (2252).
        assert_eq!(flash.0, vec![78], "queued for LEARNED_SPELL_IN_TAB");
    }

    /// The other two arms of the same block, from the same entry point: a plain row is a *spell*,
    /// and a tradeskill row is a *recipe* whose argText drops the rank (the reference returns from
    /// `0x4b2944` without ever reaching the subtext composer).
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

    /// A rank-up is `SMSG_SUPERCEDED_SPELL`, and the supersede pair `0x4b2f50` reaches the
    /// registrar with `edx = 1` — so it announces, naming the NEW rank, and exactly once (the
    /// unlearn half holds no `DisplayError` call).
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

    /// The silent cases, which are the ones a naive "print on every learn" would get wrong: a
    /// `DO_NOT_DISPLAY` row (every language, every weapon proficiency — the packets a fresh
    /// character receives in bulk), and a spell the catalog does not know at all.
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

    /// A rank-up moves the **book** and marks the store dirty; the bar is not touched here — the
    /// one rank law lives in `ui_action::ranks`, which the dirty flag then runs.
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

    /// The regression the live `WOW_CAST_TRACE` caught: a triggered proc (Frost Armor's Chilled
    /// 6136) that lands mid-cast is *our own* cast, so its `SMSG_SPELL_GO` reaches `spell_go` with
    /// `caster == self`. An unconditional STOP finished the running Fireball bar early (the "bar
    /// vanishes when a mob hits me, the spell fires a moment later" report). The bar-ending edge
    /// must be keyed to our in-flight `Casting`, like the reap beside it.
    #[test]
    fn a_proc_go_mid_cast_does_not_finish_the_running_bar() {
        use crate::combat_text::CombatTextSpawn;
        use crate::creature_anim::Casting;
        use crate::go_anim::GoLidOpen;
        use crate::net::{Guid, SelfPlayer};
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
            .init_resource::<CastBarFeed>()
            .init_resource::<PendingCast>()
            .init_resource::<QueuedMeleeSpell>()
            .init_resource::<Cooldowns>()
            .init_resource::<crate::spell::SpellModifiers>()
            .init_resource::<crate::ui_pet::PetBar>()
            .init_resource::<crate::items::Items>();

        // The self player mid-cast on Fireball (133): the bar is up, `Casting{133}` marks it.
        let self_e = app
            .world_mut()
            .spawn((
                Guid(10),
                SelfPlayer,
                Casting {
                    spell_id: 133,
                    until: None,
                },
                ObjectStore::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GuidIndex>()
            .0
            .insert(10, self_e);
        app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

        // One `spell_go` call, parameterized by the completing spell id — the whole point is that
        // 6136 (a proc) and 133 (the bar's cast) take different branches.
        let fire_go = |app: &mut App, go_spell: u32| {
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
                          mut cast_bar: ResMut<CastBarFeed>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
                          mut text: MessageWriter<CombatTextSpawn>,
                          mut go_lid: MessageWriter<GoLidOpen>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                          mut items: ResMut<crate::items::Items>,
                          mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                        let net_commands = crate::net::NetCommands(tx.clone());
                        spell_go(
                            10,
                            go_spell,
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
                            &mut cast_bar,
                            &mut pending,
                            &mut queued_melee,
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
        };

        // The proc's self-GO must NOT touch the bar.
        fire_go(&mut app, 6136);
        assert!(
            app.world().resource::<CastBarFeed>().0.is_empty(),
            "a proc's self-GO (6136) mid-cast must not push a bar edge"
        );

        // The cast the bar is showing (133) finishes it — one STOP.
        fire_go(&mut app, 133);
        let feed = &app.world().resource::<CastBarFeed>().0;
        assert_eq!(
            feed.len(),
            1,
            "the in-flight cast's own GO finishes the bar"
        );
        assert!(matches!(feed[0], CastBarEdge::Stop), "…with a STOP");
    }

    /// **A broken channel flashes green and fades — it never goes red "Interrupted"**, and the
    /// whole of `Spell::cancel()`'s wire is driven here because the correctness rests on a gate
    /// that does not mention channels.
    ///
    /// vmangos cancels a running channel (moved, turned, target died, `CMSG_CANCEL_CHANNELLING`)
    /// with **two** packets, in this order (`Spell.cpp` `Spell::cancel`, `SPELL_STATE_CASTING`):
    ///
    /// ```text
    /// SendChannelUpdate(0, true)   -> MSG_CHANNEL_UPDATE(0)      -> SPELLCAST_CHANNEL_STOP
    /// SendInterrupted(0)           -> SMSG_SPELL_FAILED_OTHER    -> ??? (SendObjectMessageToSet
    ///                                                                 (…, /*self*/ true))
    /// ```
    ///
    /// The second one **reaches the caster too**, so the obvious reading is that the bar turns red
    /// a frame after it flashed green. It must not, and three independent things say so:
    ///
    /// - vmangos deliberately withholds the *other* interrupt signal —
    ///   `sendInterrupt = !(m_channeled && m_spellState != SPELL_STATE_PREPARING)` gates
    ///   `SendCastResult(SPELL_FAILED_INTERRUPTED)` off, with the comment "channeled spells don't
    ///   display interrupted message even if they are interrupted";
    /// - the reference's own `SMSG_SPELL_FAILED_OTHER` handler (`0x6e8e40`, opcode `0x2a6`) fires
    ///   **no FrameScript event at all** — its whole body is `0x60d040` + `0x614150`, the *unit's*
    ///   cast state, and it makes no call into the UI;
    /// - and stock `CastingBarFrame.lua` guards its red arm with `not this.channeling`.
    ///
    /// Ours lands on the same behaviour by a **fourth** route, which is the one worth pinning:
    /// [`spell_failed_other`]'s red edge is keyed to a live `Casting` component, and a channel
    /// never has one — `Casting` is inserted only for `cast_time_ms > 0`, and `SMSG_SPELL_GO`
    /// (which always precedes `MSG_CHANNEL_START` in `handle_immediate`) removes it regardless.
    /// That is correct and it is also invisible: nothing in `spell_failed_other` says "channel",
    /// so a future change to when `Casting` is armed would turn every broken channel red with no
    /// test to catch it. This is that test.
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
            .init_resource::<CastBarFeed>()
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
            let mut feed = world.remove_resource::<CastBarFeed>().unwrap();
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

        // 3. SendInterrupted(0) — SMSG_SPELL_FAILED_OTHER, addressed to us, for the channel's id.
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      index: Res<GuidIndex>,
                      casting: Query<&Casting>,
                      mut cast_events: MessageWriter<CastEvent>,
                      self_guid: Res<SelfGuid>,
                      mut cast_bar: ResMut<CastBarFeed>,
                      mut pending: ResMut<PendingCast>,
                      mut queued_melee: ResMut<QueuedMeleeSpell>| {
                    spell_failed_other(
                        10,
                        BLIZZARD,
                        &mut commands,
                        &index,
                        &casting,
                        &mut cast_events,
                        &self_guid,
                        &mut cast_bar,
                        &mut pending,
                        &mut queued_melee,
                        1,
                    );
                },
            )
            .unwrap();

        let feed = &app.world().resource::<CastBarFeed>().0;
        assert!(
            !feed
                .iter()
                .any(|e| matches!(e, CastBarEdge::Interrupted | CastBarEdge::Failed)),
            "a cancelled channel pushes NO red edge — the bar flashes green and fades"
        );
        assert_eq!(feed.len(), 2, "exactly the two channel edges");
        assert!(matches!(feed[0], CastBarEdge::ChannelStart { .. }));
        assert!(matches!(
            feed[1],
            CastBarEdge::ChannelUpdate { remaining_ms: 0 }
        ));
    }

    /// **The GO's inline miss word: gold, gated, and only for an INSTANT spell**.
    ///
    /// Phase 2 shipped this emit unconditional and hardcoded white. The binary closes both halves:
    /// - `0x6e7d4e fld [SpellRec+0x94]; fcomp 0.0; test ah,0x44; jp 0x6e7e71` — a spell with a
    ///   **travel Speed** prints nothing here; its word rides the projectile's arrival instead
    ///   ([`crate::entities::MissileMiss`]). Sinister Strike (Speed 0) prints now; Fireball
    ///   (Speed 24) does not;
    /// - `0x6e7d73`/`0x6e7dcc` push the resolved SpellRec, so the word takes the same B/K
    ///   override a number would — **spell gold**, not the category-3 row default;
    /// - and the `CombatDamage` gate lives inside `0x607140`, which this path calls, so it
    ///   silences the word too — the leg this site was missing entirely.
    #[test]
    fn the_gos_inline_miss_word_is_gold_instant_only_and_cvar_gated() {
        use crate::combat_text::{CombatTextSpawn, COLOR_SPELL_GOLD};
        use crate::creature_anim::Casting;
        use crate::go_anim::GoLidOpen;
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        const SINISTER_STRIKE: u32 = 1752; // Speed 0 — an instant melee ability
        const FIREBALL: u32 = 133; // Speed 24 — it travels

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
                .init_resource::<CastBarFeed>()
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
                          mut cast_bar: ResMut<CastBarFeed>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
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
                            &mut cast_bar,
                            &mut pending,
                            &mut queued_melee,
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

    /// **The GO handler's PET leg**: a spell going off on a unit WE own arms the
    /// pet's own cooldown bank, and nothing else does — vmangos sends no cooldown packet for a
    /// pet's cast, so without this the pet bar's sweep never runs in play.
    ///
    /// Three things are pinned, and each is a way the leg could be subtly wrong:
    /// - the insert lands in the **pet's** store, not the player's (two banks, `0xcecaec` /
    ///   `0xcecb04`);
    /// - the owner read falls back to **SUMMONEDBY**, not CREATEDBY (`0x6e859a`) — a totem, which
    ///   carries only CREATEDBY, must not arm it;
    /// - a stranger's cast arms neither.
    #[test]
    fn a_pets_own_go_arms_the_pet_bank_and_only_the_pet_bank() {
        use crate::combat_text::CombatTextSpawn;
        use crate::creature_anim::Casting;
        use crate::go_anim::GoLidOpen;
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        /// `UNIT_FIELD_CHARMEDBY` / `SUMMONEDBY` / `CREATEDBY` — 64-bit, so each takes two dwords.
        const CHARMEDBY: u16 = 10;
        const SUMMONEDBY: u16 = 12;
        const CREATEDBY: u16 = 14;

        const GROWL: u32 = 2649;
        let growl = || benilla_formats::SpellDisplay {
            name: "Growl".into(),
            recovery_ms: 5000,
            ..Default::default()
        };
        let make_spells = || crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [(GROWL, growl())].into_iter().collect(),
            ),
            forms: Default::default(),
            ranges: Default::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        };

        // A world with us (guid 10), our pet (20, SUMMONEDBY us), a totem (30, CREATEDBY us only)
        // and a stranger's pet (40, SUMMONEDBY somebody else).
        let owned = |field: u16, owner: u64| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (field, owner as u32),
                (field + 1, (owner >> 32) as u32),
            ]))
        };
        let fire = |caster: u64, store: ObjectStore| {
            let mut app = App::new();
            app.add_message::<CastEvent>()
                .add_message::<SpellGoTargets>()
                .add_message::<CombatTextSpawn>()
                .add_message::<GoLidOpen>()
                .add_message::<crate::creature_anim::SheathRequest>()
                .add_message::<crate::player::StandStateRequest>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<CastBarFeed>()
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
            let caster_e = app.world_mut().spawn((Guid(caster), store)).id();
            {
                let mut index = app.world_mut().resource_mut::<GuidIndex>();
                index.0.insert(10, self_e);
                index.0.insert(caster, caster_e);
            }
            app.world_mut().resource_mut::<SelfGuid>().0 = Some(10);

            let (tx, _rx) = crossbeam_channel::unbounded();
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
                          mut cast_bar: ResMut<CastBarFeed>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
                          mut text: MessageWriter<CombatTextSpawn>,
                          mut go_lid: MessageWriter<GoLidOpen>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut pet_bar: ResMut<crate::ui_pet::PetBar>,
                          mut items: ResMut<crate::items::Items>,
                          mut sheath: MessageWriter<crate::creature_anim::SheathRequest>| {
                        let net_commands = crate::net::NetCommands(tx.clone());
                        spell_go(
                            caster,
                            GROWL,
                            0,
                            vec![],
                            vec![],
                            None,
                            None,
                            None,
                            None,
                            Some(caster),
                            &mut commands,
                            &index,
                            &casting,
                            &mut cast_events,
                            &mut go_targets,
                            &self_guid,
                            &stores,
                            &mut cast_bar,
                            &mut pending,
                            &mut queued_melee,
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
            let armed = |c: &Cooldowns| c.info(GROWL, 0, Some(&growl()), now).remaining_ms > 0;
            let world = app.world();
            (
                armed(world.resource::<Cooldowns>()),
                armed(&world.resource::<crate::ui_pet::PetBar>().cooldowns),
                world.resource::<crate::ui_pet::PetBar>().bar_signals,
            )
        };

        // Our pet: the PET bank only, and a forced repaint with it.
        let (player, pet, signals) = fire(20, owned(SUMMONEDBY, 10));
        assert!(pet, "our pet's GO arms the pet bank");
        assert!(!player, "…and never the player's");
        assert_eq!(signals, 1, "PET_BAR_UPDATE_COOLDOWN's repaint");

        // A charm reads CHARMEDBY first — the same leg, the other field.
        let (_, charmed, _) = fire(20, owned(CHARMEDBY, 10));
        assert!(charmed, "a charmed unit's GO arms it too");

        // A TOTEM carries CREATEDBY and no SUMMONEDBY. `0x5ee5a0` would accept it; this leg's own
        // fallback (`0x6e859a`) does not, and reading the wrong one is invisible until a shaman
        // drops a totem and the pet bar sweeps.
        let (_, totem, _) = fire(30, owned(CREATEDBY, 10));
        assert!(!totem, "CREATEDBY alone is not this leg's owner test");

        // Somebody else's pet: neither bank.
        let (p2, pet2, _) = fire(40, owned(SUMMONEDBY, 99));
        assert!(!p2 && !pet2, "a stranger's pet arms nothing");
    }

    /// **Both** producers of the caster's chain-hop array: `SMSG_SPELL_GO`'s own
    /// hit list — the leg that makes a non-channelled chain spell draw at all — and the 816 packet.
    /// Each drops the caster's own guid and each clears before it fills; unstreamed targets fall
    /// out as they resolve. Without this the beam lane is inert no matter how right its geometry is.
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
            .init_resource::<CastBarFeed>()
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

        // Leg 1 — `HandleSpellGo`'s own fill (`0x6e800d`). The hit list carries the caster itself
        // (vmangos includes a self-hit on plenty of spells) and an unstreamed target; both drop.
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
                      mut cast_bar: ResMut<CastBarFeed>,
                      mut pending: ResMut<PendingCast>,
                      mut queued_melee: ResMut<QueuedMeleeSpell>,
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
                        &mut cast_bar,
                        &mut pending,
                        &mut queued_melee,
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

        // Leg 2 — the 816 packet, which is what vmangos sends for a CHANNELLED chain. It clears
        // before it fills, so the GO's list above must not survive underneath it.
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

    /// The director's stuck-shooting-idle report: click off the target during Auto Shot and the
    /// Load/Hold stance never drops. A deselect surfaces as a `SMSG_CAST_RESULT` failure for the
    /// cached auto-repeat spell (vmangos `HandleSetSelectionOpcode` → `Spell::cancel` →
    /// `SendCastResult(INTERRUPTED)`), and the client's `6e1cd9` jump into `0x6ea080` makes that
    /// the full local cancel: the key AND the shooting idle both drop. (It is not the only live
    /// disarm — see [`super::cancel_auto_repeat`]: vmangos does send `SMSG_CANCEL_AUTO_REPEAT`.)
    #[test]
    fn a_cast_result_fail_of_the_cached_auto_repeat_spell_disarms_the_shooting_idle() {
        use crate::creature_anim::AutoRepeatArmed;
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        let mut app = App::new();
        app.add_message::<CastEvent>()
            .init_resource::<GuidIndex>()
            .init_resource::<SelfGuid>()
            .init_resource::<CastErrors>()
            .init_resource::<crate::ui_action::UiErrorKeys>()
            .init_resource::<CastBarFeed>()
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
                          mut cast_errors: ResMut<CastErrors>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut cast_bar: ResMut<CastBarFeed>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
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
                            &mut cast_errors,
                            &casting,
                            &mut cast_events,
                            &mut cast_bar,
                            &mut pending,
                            &mut queued_melee,
                            &mut cooldowns,
                            &mut auto_repeat,
                            None,
                            &net,
                            1,
                        );
                    },
                )
                .unwrap();
        };

        // Reason 0x17 is the client's one skip (`6e1ce1 cmp cl,0x17; je`): armed stays armed.
        fire_fail(&mut app, 0x17);
        assert_eq!(app.world().resource::<AutoRepeatActive>().0, Some(75));
        assert!(app
            .world()
            .entity(self_e)
            .get::<AutoRepeatArmed>()
            .is_some());

        // SPELL_FAILED_INTERRUPTED — the deselect's wire face — runs the full cancel.
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

    /// `SMSG_REMOVED_SPELL` shrinks the book and dirties the feeds — the packet a
    /// talent wipe arrives as, one per rank of every talent. The dirty flag is a real EDGE, not a
    /// blanket set: a wipe sends removals for ranks the character never learned too (vmangos walks
    /// the whole class tree), and a repaint per no-op would be a repaint per talent in the game.
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

    /// The unlearn line and its four silent gates — the behaviour decision 2243
    /// asserted, with a byte citation, did not exist. The argText is the BARE name: there is no
    /// rank composer on this path, so a respec'd rank says "Improved Fireball", never
    /// "Improved Fireball (Rank 3)".
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

    /// `castUI` gates the unlearn line and NOT the learn line — the reference tests it only
    /// afterwards, at `0x4b29bf`, to decide the book slot. So a `castUI > 0` spell announces when
    /// it arrives and is silent when it goes. Asserted because the two methods reading the same
    /// field for different answers is exactly the kind of asymmetry a later edit "tidies" away.
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
    /// **The GO-deferred auto-attack start** (`HandleSpellGo` @ `0x6e83c0`) — the
    /// half of the auto-attack-on-cast law benilla shipped without, because ten hand-picked warrior
    /// rows were read as a census of `AttributesEx2 & 0x100000`. The real file carries the bit on
    /// 36, so the class that never started an auto-attack here is every stealth opener and
    /// positional strike: Backstab, Garrote, Ambush, Cheap Shot, Shred, Ravage, Pounce, Judgement.
    ///
    /// Four things, and each is a way this arm could be subtly wrong:
    /// - a bit20 spell's own GO sends `CMSG_ATTACKSWING` **at the GO's first hit target**
    ///   (`0x6e83e9`: `hits[0]`, not the packet's target field, and not our selection);
    /// - a spell without the bit sends nothing — **bug B280's own control**: the hunter's instant
    ///   shots carry Ex2 bit **17** (`DO_NOT_RESET_COMBAT_TIMERS`), not bit 20, so casting Serpent
    ///   Sting starts no attack of either kind, which is what 0994 §4 recorded;
    /// - already swinging sends nothing (`0x6e83e7`'s `[player+0xc48]` gate) — which is also what
    ///   keeps the start's own auto-repeat cancel off every strike of a fight;
    /// - somebody else's Backstab going off sends nothing (the handler's self gate).
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
                    // The real 5875 words for the two: Backstab Ex2 0x100000, Serpent Sting
                    // Ex2 0x20000 (bit 17, one bit below — the whole distinction).
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
                .init_resource::<CastBarFeed>()
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
                          mut cast_bar: ResMut<CastBarFeed>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
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
                            // The packet's own target field is deliberately a DIFFERENT guid: the
                            // arm must take `hits[0]`, and this is what catches it reading here.
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
                            &mut cast_bar,
                            &mut pending,
                            &mut queued_melee,
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
            "Serpent Sting carries Ex2 bit 17, not bit 20 — it starts nothing (B280 / 0994 §4)"
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
    /// **The `modalNextSpell` chain** (`HandleCastResult 0x6e7330` @ `0x6e7408`–`0x6e74aa`,
    /// decision 1597) — how a hunter starts shooting, and the fix for bug B280.
    ///
    /// The reply to our in-flight cast reads **`Spell.dbc` column 38** of the spell it names and,
    /// if that is non-zero and is not already the running repeat, the client casts it itself. Every
    /// hunter shot's column 38 is 75 (Auto Shot). Five things, each a way this could be wrong:
    /// - a successful sting chains — **the success arm is the ordinary case**, and it is the one
    ///   this handler used to ignore entirely (`0x6e735a jne` sends a non-failure straight to the
    ///   chain block);
    /// - a *failed* sting chains too (both paths converge at `0x6e73eb`);
    /// - the in-flight guard is cleared before the chain, or `TryCast`'s IsCasting rung (our `0x61`)
    ///   would refuse the chained cast;
    /// - Auto Shot already running ⇒ **nothing** is sent (`0x6e745b`'s equal branch), so a second
    ///   shot never restarts the repeat or resets its swing timer;
    /// - a reply for a spell we do not have in flight is not ours to chain from (`0x6e7408`).
    #[test]
    fn a_hunter_shots_cast_result_chains_auto_shot_exactly_once() {
        use crate::net::{Guid, SelfPlayer};
        use bevy::ecs::system::RunSystemOnce;

        const SERPENT_STING: u32 = 1978;
        const AUTO_SHOT: u32 = 75;

        let spells = || crate::ui_action::Spells {
            catalog: benilla_formats::SpellCatalog::from_displays(
                [
                    (
                        SERPENT_STING,
                        benilla_formats::SpellDisplay {
                            name: "Serpent Sting".into(),
                            // The shipped 5875 row: ranged slot, and column 38 = 75.
                            attributes: 0x0001_0002,
                            attributes_ex2: 0x0002_0000,
                            modal_next_spell: AUTO_SHOT,
                            ..Default::default()
                        },
                    ),
                    (
                        AUTO_SHOT,
                        benilla_formats::SpellDisplay {
                            name: "Auto Shot".into(),
                            attributes: 0x0005_0012,
                            attributes_ex2: 0x20,
                            // Auto Shot's own column 38 is 0 — this is what makes the chain
                            // exactly one hop instead of a loop.
                            modal_next_spell: 0,
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

        // One CAST_RESULT for `spell_id`, with `in_flight` armed as the outstanding cast and
        // `running` as the live auto-repeat. Returns (what the reply chains, is the guard still
        // armed).
        let fire = |spell_id: u32, success: bool, in_flight: Option<u32>, running: Option<u32>| {
            let mut app = App::new();
            app.add_message::<CastEvent>()
                .init_resource::<GuidIndex>()
                .init_resource::<SelfGuid>()
                .init_resource::<CastErrors>()
                .init_resource::<crate::ui_action::UiErrorKeys>()
                .init_resource::<crate::ui_action::UiErrorKeys>()
                .init_resource::<CastBarFeed>()
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
            app.world_mut().resource_mut::<AutoRepeatActive>().0 = running;
            if let Some(id) = in_flight {
                app.world_mut()
                    .resource_mut::<PendingCast>()
                    // `guards: false` — a hunter shot is `Attributes & 0x2` ranged, so it is
                    // recorded as committed and does NOT occupy the refusal. Arming it the other
                    // way would hide the very regression this test exists for (1601).
                    .arm(id, Instant::now(), false);
            }
            let (tx, _rx) = crossbeam_channel::unbounded();
            let cat = spells();
            let chained = app
                .world_mut()
                .run_system_once(
                    move |mut commands: Commands,
                          self_guid: Res<SelfGuid>,
                          index: Res<GuidIndex>,
                          mut cast_errors: ResMut<CastErrors>,
                          casting: Query<&Casting>,
                          mut cast_events: MessageWriter<CastEvent>,
                          mut cast_bar: ResMut<CastBarFeed>,
                          mut pending: ResMut<PendingCast>,
                          mut queued_melee: ResMut<QueuedMeleeSpell>,
                          mut cooldowns: ResMut<Cooldowns>,
                          mut auto_repeat: ResMut<AutoRepeatActive>| {
                        let net = crate::net::NetCommands(tx.clone());
                        cast_result(
                            spell_id,
                            success,
                            if success { None } else { Some(0x1b) },
                            None,
                            &mut commands,
                            &self_guid,
                            &index,
                            &mut cast_errors,
                            &casting,
                            &mut cast_events,
                            &mut cast_bar,
                            &mut pending,
                            &mut queued_melee,
                            &mut cooldowns,
                            &mut auto_repeat,
                            Some(&cat),
                            &net,
                            1,
                        )
                    },
                )
                .unwrap();
            let still_armed = app
                .world()
                .resource::<PendingCast>()
                .in_flight(Instant::now());
            (chained, still_armed)
        };

        // The ordinary case: the sting lands, and Auto Shot follows by itself.
        assert_eq!(
            fire(SERPENT_STING, true, Some(SERPENT_STING), None),
            (Some(AUTO_SHOT), false),
            "a successful sting chains Auto Shot, and clears the in-flight guard first"
        );
        // And a failed one does too — both results converge on the same block.
        assert_eq!(
            fire(SERPENT_STING, false, Some(SERPENT_STING), None).0,
            Some(AUTO_SHOT),
            "a FAILED sting chains it as well (`0x6e735a jne` → the same `0x6e73eb`)"
        );
        // Already shooting: re-arm, never re-cast.
        assert_eq!(
            fire(SERPENT_STING, true, Some(SERPENT_STING), Some(AUTO_SHOT)).0,
            None,
            "Auto Shot already running: the equal branch sends nothing"
        );
        // Auto Shot's own replies terminate the chain.
        assert_eq!(
            fire(AUTO_SHOT, true, Some(AUTO_SHOT), Some(AUTO_SHOT)).0,
            None,
            "Auto Shot's own column 38 is 0 — no second hop"
        );
        // Not our in-flight cast (a proc's result, a stale reply): not ours to chain from.
        assert_eq!(
            fire(SERPENT_STING, true, Some(133), None).0,
            None,
            "a reply for a spell we do not have in flight chains nothing"
        );
    }
}
