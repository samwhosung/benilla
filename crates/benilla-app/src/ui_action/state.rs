//! Each action slot's per-frame state behind `IsUsableAction`, `IsActionInRange`,
//! `IsCurrentAction` and `GetActionCooldown`, diff-pushed into the VM ahead of the world-enter
//! events ([`feed_action_state`]), and the events their producers raise, signalled after the unit
//! snapshots their handlers read ([`fire_action_events`]), each once a frame in the reference's
//! order: the cooldown pair (`0x4b31b0`) on a cooldown change; `ACTIONBAR_UPDATE_USABLE` when a
//! refresh of the usable cache (`0x4e5c00`) finds a pair changed, each reactive spell's
//! `SPELL_ACTIVE` ([`super::reactive`]) and `SPELL_UPDATE_USABLE` from the usable refresh
//! `0x4b31c0`'s callers; `BAG_UPDATE_COOLDOWN` (`0x4f93d0`) from the item legs; then combat enter
//! and leave (`0x6256ff`, `0x625778`) and autorepeat start and stop (`0x6e5952`, `0x6ea170`).
//! `IsCurrentAction` reads the cast state live, and its pair (`0x4b3250`) is signalled where the
//! cast changes ([`crate::ui_cast`]).
//! Usability is `IsSpellUsableNow 0x6e3d60` ([`crate::spell::usable`]), auto-repeat the `0xceac30`
//! key, and range the pre-check `0x6e4440` on the selection ([`crate::spell::range_check`]).

use crate::ui_items::stack_counts;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Instant;

use bevy::prelude::*;

use benilla_formats::RangeTargets;
use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_SPELL};
use benilla_ui::script::{macro_slot_id, ActionState};

use crate::creature_anim::Engaged;
use crate::item_watch::ItemNotice;
use crate::items::Items;
use crate::net::{FieldChanged, NetCommands, ObjectStore, SelfPlayer};
use crate::spell::Cooldowns;

use super::{PlayerActions, Spells};
use crate::query_cache::Ask;
use crate::spell::usable;
use crate::spell::AutoRepeatActive;

/// The feed's memory: what was last pushed, and the edge detectors.
#[derive(Default)]
pub(super) struct StateMemory {
    pushed: HashMap<u32, ActionState>,
    last_generation: Option<u64>,
    /// The cooldown list's usable refreshes and bag flushes ([`Cooldowns`]) as last read.
    usable_refreshes: u64,
    bag_flushes: u64,
    /// The selection's guid and the control flag, whose changes call the usable refresh.
    selection: Option<u64>,
    control_lost: bool,
    /// The VM's macro edits and the bag feed's `BAG_UPDATE`s as last read.
    macro_edits: u64,
    bag_updates: u64,
    /// Item slots whose record was missing at the cooldown query, which `0x6e2ed0` asks with the
    /// callback `0x6e2f70` (`0x6e2efb`).
    awaiting_records: HashSet<u32>,
    engaged: bool,
    auto_repeat: Option<u32>,
    last_cd_trace: Option<Instant>,
}

/// What the action bar's usable cache (`0xbc6b60`, `0xbc67a0`) last stored, which outlives a VM as
/// the reference's does: its zeroing `0x4e4f50` runs only from `InitializeGame 0x48f480`.
#[derive(Default)]
pub(super) struct UsableCache {
    /// Each slot's `(usable, notEnoughMana)` pair, `(false, false)` left out.
    pairs: HashMap<u32, (bool, bool)>,
    /// Each slot's word and count as last seen, whose change is a notify `0x4e58e0`.
    words: HashMap<u32, SlotWord>,
}

/// A slot as `0x4e58e0`'s callers write it: the word `SetActionSlot 0x4e5d60` stores, what it
/// resolves to through a macro's bound spell, and the count a recount stores (`0x4e5caa`).
type SlotWord = (u8, u32, SlotResolve, u32);

impl UsableCache {
    /// `0x4e58e0`: a slot placed, cleared or recounted stores its new pair (`0x4e590a`,
    /// `0x4e5914`) and signals only `ACTIONBAR_SLOT_CHANGED`. `arrival` is our player's arrival
    /// with its book: the loop `0x5deae2`-`0x5deb12` writes each slot (`0x5deafc`) but an empty one
    /// or a spell word the player does not know (`0x60c740`, skipped at `0x5deaf5`).
    fn notify(
        &mut self,
        words: HashMap<u32, SlotWord>,
        pairs: &HashMap<u32, (bool, bool)>,
        arrival: Option<&BTreeSet<u32>>,
    ) {
        let slots: HashSet<u32> = words.keys().chain(self.words.keys()).copied().collect();
        for action in slots {
            let word = words.get(&action);
            let written = match arrival {
                Some(book) => word.is_some_and(|&(kind, id, _, _)| {
                    kind != ACTION_KIND_SPELL || book.contains(&id)
                }),
                None => word != self.words.get(&action),
            };
            if written {
                match pairs.get(&action) {
                    Some(&pair) => self.pairs.insert(action, pair),
                    None => self.pairs.remove(&action),
                };
            }
        }
        self.words = words;
    }

    /// `0x4e5c00`: every pair recomputed and stored; whether any changed, its
    /// `ACTIONBAR_UPDATE_USABLE` (`0x4e5c4e`).
    fn refresh(&mut self, pairs: HashMap<u32, (bool, bool)>) -> bool {
        std::mem::replace(&mut self.pairs, pairs) != self.pairs
    }
}

/// `UNIT_FIELD_POWER1` to `POWER5`, one watch (`0x604876`).
const UNIT_FIELD_POWERS: std::ops::RangeInclusive<u16> = 23..=27;
/// `UNIT_FIELD_AURASTATE`.
const UNIT_FIELD_AURASTATE: u16 = 125;
/// `PLAYER_FIELD_BYTES`, whose byte 1 is the combo points.
const PLAYER_FIELD_BYTES: u16 = 1222;

/// Whether a descriptor edge is one the usable refresh `0x4b31c0`'s field-watch callers watch,
/// each comparing its own bytes: the active player's health (`0x604848`), powers (`0x604876`) and
/// aura state (`0x6048b6`, the target's as well), its unit flags' bits 0x1c20000 and combat bit
/// 0x80000 (`0x5ff50e`, `0x5ff51b`), `UNIT_FIELD_BYTES_1`'s form byte (`0x5de011`) and byte 3's
/// bit 0x2 (`0x5ff83a`), and its combo points, `PLAYER_FIELD_BYTES` byte 1 (`0x5ddff0`).
fn refreshes_usable(e: &FieldChanged, me: Option<u64>, target: Option<u64>) -> bool {
    use benilla_protocol::field::{FIELD_UNIT_BYTES_1, FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH};
    let flipped = e.old ^ e.new;
    let mine = me == Some(e.guid);
    if !e.is_unit() {
        return false;
    }
    match e.index {
        UNIT_FIELD_AURASTATE => mine || target == Some(e.guid),
        _ if !mine => false,
        FIELD_UNIT_HEALTH => true,
        i if UNIT_FIELD_POWERS.contains(&i) => true,
        FIELD_UNIT_FLAGS => flipped & (0x01c2_0000 | 0x0008_0000) != 0,
        FIELD_UNIT_BYTES_1 => flipped & (0x00ff_0000 | 0x0200_0000) != 0,
        PLAYER_FIELD_BYTES => flipped & 0xff00 != 0,
        _ => false,
    }
}

/// A slot after the macro indirection (`0x4e5a50`), as the usable compute `0x4e5050` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotResolve {
    /// A spell or item slot, or a macro whose bound spell is live (`[rec+0x564] > 0`).
    Action(u8, u32),
    /// A macro whose bound spell came from the pet's book (`[rec+0x568] != 0`): `0x4e5050` asks
    /// the pet gate and the pet's power instead of the usable walk, `GetActionCooldown` the pet's
    /// history, and the range, checked and autorepeat tests, which need book 0, answer nothing.
    PetSpell(u32),
    /// A macro that casts nothing (`[rec+0x564] == 0`): usable while it exists (`0x4e5030`),
    /// with no cooldown, range or check (`0x4e50f4`-`0x4e516f`).
    BareMacro,
    /// A `/cast` that did not resolve (`-1`): grey (refused at `0x4e518b`), with nothing to
    /// report but the cooldown query's enable, which it takes with its -1 (`0x6e2ea0`).
    Unresolved,
    /// A macro that no longer exists: grey, and no cooldown query (`0x4e6cfe`).
    Missing,
}

/// Resolves a slot through its macro: every `Is*Action`/`GetActionCooldown` binding goes through
/// `0x4e5a50`, so a macro that casts Fireball is the Fireball slot, its own icon aside. Spells
/// only: 1.12 has no `/use`, so no macro names an item.
fn resolve_through_macro(
    kind: u8,
    action: u32,
    bound: &crate::ui_macro::MacroBoundSpells,
) -> SlotResolve {
    use crate::ui_macro::BoundSpell;
    match macro_slot_id(kind, action) {
        // The book selector forks only a live spell (`0x4e50ee` sends 0 and -1 elsewhere).
        Some(id) => match bound.0.get(&id).map(|b| (b.spell, b.pet_book)) {
            Some((BoundSpell::Spell(s), false)) => SlotResolve::Action(ACTION_KIND_SPELL, s),
            Some((BoundSpell::Spell(s), true)) => SlotResolve::PetSpell(s),
            Some((BoundSpell::None, _)) => SlotResolve::BareMacro,
            Some((BoundSpell::Unresolved, _)) => SlotResolve::Unresolved,
            None => SlotResolve::Missing,
        },
        None => SlotResolve::Action(kind, action),
    }
}

/// The profession window a profession opener opens (`Effect[0]` 47, `0x4e54fe`), which
/// `IsCurrentAction` holds against the one open: a craft by its `EffectMiscValue[0]` (`0x4e5507`),
/// else a trade skill by its `SkillLineAbility` line (`0x6de040`, `0x4e5522`), as the opener opens
/// it (`0x6e4bd7`). A trade skill whose row is not loaded opens nothing, and checks nothing.
fn profession(
    spell_id: u32,
    d: &benilla_formats::SpellDisplay,
    skill_lines: Option<&crate::ui_spellbook::SkillLines>,
) -> Option<benilla_ui::script::Profession> {
    use benilla_ui::script::Profession;
    if d.effects[0] != benilla_formats::SPELL_EFFECT_TRADE_SKILL {
        return None;
    }
    match u32::try_from(d.effect_misc_value[0]).unwrap_or(0) {
        0 => skill_lines
            .and_then(|s| s.catalog.spell_to_line(spell_id))
            .map(Profession::TradeSkill),
        craft => Some(Profession::Craft(craft)),
    }
}

/// What a macro slot bound to a pet-book spell reads.
struct PetBookLeg<'a> {
    spell: u32,
    d: &'a benilla_formats::SpellDisplay,
    /// The active player's object, whose fields the cost is read from.
    player: Option<&'a ObjectStore>,
    /// The pet's object when the pet gate holds, else `None`.
    pet: Option<&'a ObjectStore>,
    skill_lines: Option<&'a benilla_formats::SkillLineCatalog>,
    mods: &'a crate::spell::SpellModifiers,
    /// The pet's own history, the list at `0xcecb04`.
    cooldowns: &'a Cooldowns,
    now: Instant,
    anchor: Instant,
    ui_now: f64,
}

/// A slot whose macro is bound to a spell out of the pet's book (`[rec+0x568] != 0`, `0x4e5a50`):
/// - usable (`0x4e51b2`-`0x4e5216`): not before the pet gate (`0x4bcf70`), then the spell's cost
///   from the player's fields (`0x6e31b0` with no pet selector, `0x4e5201`) against the pet's
///   power, its health for a negative type; affordable is usable, else only `notEnoughMana`;
/// - cooldown (`0x4e6cf4`-`0x4e6d0b`, `0x6e2ea0`): the pet's history, not the player's;
/// - no range (`0x4e5759` returns nil, `0x4e584d` false), no checked or auto-repeat state, which
///   all need book 0; `IsAttackAction` reads the record alone (`0x4e59b0`).
fn pet_book_state(leg: PetBookLeg) -> ActionState {
    let mut st = ActionState {
        is_attack: leg.d.is_melee_auto_attack(),
        ..Default::default()
    };
    if let (Some(player), Some(pet)) = (leg.player, leg.pet) {
        let cost = usable::power_cost(
            leg.d,
            player,
            leg.skill_lines,
            leg.mods,
            &mut crate::spell::Unrecorded,
        );
        let power_type = leg.d.power_type as i32;
        let power = if power_type < 0 {
            pet.0.unit_health()
        } else {
            u8::try_from(power_type)
                .ok()
                .and_then(|ty| pet.0.unit_power(ty))
        };
        // `0x4e5206 cmp eax,edi; jle`: equal passes, both read signed.
        if cost <= power.unwrap_or(0) as i32 {
            st.usable = true;
        } else {
            st.not_enough_mana = true;
        }
    }
    st.cooldown = leg
        .cooldowns
        .info(leg.spell, 0, Some(leg.d), leg.now)
        .ui_triple(leg.anchor, leg.ui_now);
    st
}

/// The events one pass of [`feed_action_state`] raised, which [`fire_action_events`] signals.
#[derive(Resource, Default)]
pub(crate) struct ActionEvents {
    /// The cooldown pair, `ACTIONBAR_UPDATE_COOLDOWN` and `SPELL_UPDATE_COOLDOWN`.
    cooldowns: bool,
    /// `ACTIONBAR_UPDATE_USABLE`.
    usable_changed: bool,
    /// `SPELL_UPDATE_USABLE`.
    refresh: bool,
    /// Each reactive spell that turned usable in the refresh, by name, a `SPELL_ACTIVE`.
    activated: Vec<String>,
    /// `BAG_UPDATE_COOLDOWN`.
    bags: bool,
    /// `PLAYER_ENTER_COMBAT` (true) or `PLAYER_LEAVE_COMBAT`.
    combat: Option<bool>,
    /// `START_AUTOREPEAT_SPELL` (true) or `STOP_AUTOREPEAT_SPELL`.
    auto_repeat: Option<bool>,
}

/// Computes and diff-pushes every occupied slot's state, and owes the reference's events to
/// [`fire_action_events`].
#[allow(clippy::type_complexity)] // a Bevy system's full input set
pub(super) fn feed_action_state(
    // The cooldowns, the auto-repeat key, the selection with the relations and positions
    // `IsActionInRange`'s pre-check reads, and the pet's bar (the gate and the history a macro
    // bound to a pet spell reads), which the call host also reaches.
    mut vm: crate::script_calls::VmWith<(
        ResMut<'static, Cooldowns>,
        Res<'static, AutoRepeatActive>,
        crate::spell::cast_target::CastTargeting<'static, 'static>,
        Option<Res<'static, crate::ui_pet::PetBar>>,
    )>,
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    clock: Res<crate::ui_script::UiClock>,
    // One tuple for Bevy's 16-param ceiling: macro bindings, spell mods, the range units, the
    // control flag, the skill lines and the active player's creation.
    cast_state: (
        Res<crate::ui_macro::MacroBoundSpells>,
        Res<crate::spell::SpellModifiers>,
        // The caster's and the target's reach and motion, which the range compare reads.
        crate::spell::RangeUnits,
        // `[0xb4b3e4]`, which the usable walk reads.
        Option<Res<crate::player::OwnControlLost>>,
        // A profession opener's line (`0x6de040`), which the checked test reads, and the skill
        // lines the walk's cost finds its level through.
        Option<Res<crate::ui_spellbook::SkillLines>>,
        // The active player's creation, whose arrival (`0x5dea86`) empties the reactive list.
        Query<(), Added<SelfPlayer>>,
    ),
    self_q: Query<(&ObjectStore, &Transform, Has<Engaged>), With<SelfPlayer>>,
    // One param for the guid index and the item store, for the same ceiling.
    objects: crate::net::Objects,
    units: Query<(&ObjectStore, &Transform), Without<SelfPlayer>>,
    relations: crate::target::RelationWorld,
    // The item cache and the channel its misses query on, one param for the same ceiling.
    (items, commands): (Res<Items>, Res<NetCommands>),
    // The usable refresh's field-watch callers, the spell charges watcher, the enter cascade,
    // the bag feed's `BAG_UPDATE`s, the events owed and the memory about the VM.
    (mut edges, mut item_notices, mut entries, bag_updates, mut owed, mut memory): (
        MessageReader<FieldChanged>,
        MessageReader<ItemNotice>,
        MessageReader<crate::net::WorldEnterCascadeMessage>,
        Option<Res<crate::ui_items::feed::BagUpdates>>,
        ResMut<ActionEvents>,
        Local<crate::ui_script::VmMemo<StateMemory>>,
    ),
    // The bar's usable cache, which lives for the session, not for a VM.
    mut cache: Local<UsableCache>,
    // The reactive list, which lives from one arrival to the next, not for a VM.
    mut reactive: Local<super::reactive::ReactiveList>,
) {
    vm.turn(|script, (mut cooldowns, auto_repeat, targeting, pet_bar)| {
        let Some(script) = script else {
            return;
        };
        let (memory, new_vm) = memory.get_reset(script);
        let now = Instant::now();
        // Every cooldown pushes its absolute start on the `GetTime` clock, derived from this one
        // pair, so a running cooldown derives the same start every frame.
        let (anchor, ui_now) = (clock.anchor, clock.ui_now);
        cooldowns.prune(now);
        let gen_changed = memory.last_generation != Some(cooldowns.generation);
        memory.last_generation = Some(cooldowns.generation);
        // The cooldown-clock trace (`WOW_MOVE_TRACE` sink, tag "cd"): once per second.
        let trace_cd = benilla_assets::trace::enabled()
            && memory
                .last_cd_trace
                .is_none_or(|t| now.duration_since(t).as_secs_f32() >= 1.0);
        if trace_cd {
            memory.last_cd_trace = Some(now);
        }

        let (bound, spell_mods, range_units, own_control, skill_lines, created) = &cast_state;
        let skill_catalog = skill_lines.as_deref().map(|s| &s.catalog);
        let me = self_q.iter().next();
        let control_lost = own_control.as_ref().is_some_and(|c| c.0);
        // The bags, walked once per frame for every reagent and totem count below.
        let stacks = me
            .map(|(s, _, _)| stack_counts(&s.0, &objects))
            .unwrap_or_default();
        let engaged = me.is_some_and(|(_, _, e)| e);
        let caster = range_units.caster();
        let selection = &targeting.selection;
        let target = selection
            .guid
            .and_then(|g| objects.entity(g))
            .and_then(|e| units.get(e).ok());
        let cast_ctx = targeting.context();
        // The pet gate `0x4bcf70`, asked once for every slot bound to a pet spell: the bar those
        // slots read the history of, and the pet's object when the gate holds.
        let pet_leg = pet_bar.as_deref().map(|bar| {
            let pet = objects.store(bar.spells.pet_guid);
            let held = crate::ui_pet::pet_actions_usable(
                bar,
                cast_ctx.self_guid,
                me.map(|(s, _, _)| s),
                pet,
            );
            (bar, pet.filter(|_| held))
        });

        // The frame's callers of the usable refresh `0x4b31c0` and of `0x4f93d0`; a new VM meets
        // the state as it stands and hears none.
        let mut refresh = edges.read().fold(false, |hit, e| {
            hit | refreshes_usable(e, cast_ctx.self_guid, selection.guid)
        });
        // The target's select and release (`0x493892`, `0x493a69`) and `SetPlayerControl`
        // (`0x4958fc`, `0x495910`), each on a change.
        refresh |= selection.guid != memory.selection || control_lost != memory.control_lost;
        (memory.selection, memory.control_lost) = (selection.guid, control_lost);
        // The cooldown list's own (`0x6e2e85`, `0x6e3076`).
        refresh |= cooldowns.usable_refreshes != memory.usable_refreshes;
        memory.usable_refreshes = cooldowns.usable_refreshes;
        let mut bags = cooldowns.bag_flushes != memory.bag_flushes;
        memory.bag_flushes = cooldowns.bag_flushes;
        // The spell charges watcher on an item of the player's (`0x5d9587`).
        bags |= item_notices.read().fold(false, |hit, n| {
            hit | matches!(n, ItemNotice::SpellCharges { .. })
        });
        if new_vm {
            (refresh, bags) = (false, false);
        }
        // The usable cache's other refreshes (`0x4e5c00`): world entry (`0x4e4f90`, from the enter
        // cascade at our player's arrival or a reload's), `EditMacro` (`0x4f132e`) and each
        // `BAG_UPDATE` (`0x4f9374`).
        let entered = entries.read().count() > 0;
        // Our player's creation, whose arrival (`0x5dea50`) writes every slot (`0x5deafc`) and
        // empties the reactive list (`0x5dea86`); a reload reaches neither.
        let arrived = !created.is_empty();
        let macro_edits = script.macro_edits();
        let bag_updates = bag_updates.map_or(0, |b| b.0);
        let refresh_cache = refresh
            || entered
            || new_vm
            || macro_edits != memory.macro_edits
            || bag_updates != memory.bag_updates;
        (memory.macro_edits, memory.bag_updates) = (macro_edits, bag_updates);
        // `0x6e2f70`, an awaited record's arrival with its cooldown running (`0x6e2f89`,
        // `0x6e2f8e`).
        let mut record_flush = false;

        let mut fresh: HashMap<u32, ActionState> = HashMap::new();
        for (&slot, button) in &actions.buttons {
            let action = u32::from(slot) + 1;
            let mut st = ActionState::default();
            let (kind, id) = match resolve_through_macro(button.kind, button.action, bound) {
                SlotResolve::Action(kind, id) => (kind, id),
                SlotResolve::BareMacro => {
                    // `0x4e5050`'s spell-less leg: usable while the player-control flag
                    // `[0xb4b3e4]` is set (`0x4e50f4`; `SMSG_CLIENT_CONTROL_UPDATE` clears it,
                    // `0x4958e0`).
                    st.usable = !control_lost;
                    st.no_cooldown_query = true;
                    fresh.insert(action, st);
                    continue;
                }
                SlotResolve::Unresolved => {
                    fresh.insert(action, st);
                    continue;
                }
                SlotResolve::PetSpell(spell) => {
                    // A missing record answers 0 before the book is asked (`0x4e518b`-`0x4e51a0`).
                    let record = spells.as_ref().and_then(|s| s.catalog.get(spell));
                    if let (Some(d), Some((bar, pet))) = (record, pet_leg) {
                        st = pet_book_state(PetBookLeg {
                            d,
                            spell,
                            player: me.map(|(s, _, _)| s),
                            pet,
                            skill_lines: skill_catalog,
                            mods: spell_mods,
                            cooldowns: &bar.cooldowns,
                            now,
                            anchor,
                            ui_now,
                        });
                    }
                    fresh.insert(action, st);
                    continue;
                }
                SlotResolve::Missing => {
                    st.no_cooldown_query = true;
                    fresh.insert(action, st);
                    continue;
                }
            };
            let button = &benilla_protocol::messages::ActionButton {
                slot,
                action: id,
                kind,
            };
            match button.kind {
                ACTION_KIND_SPELL => {
                    let d = spells.as_ref().and_then(|s| s.catalog.get(button.action));
                    let Some(d) = d else {
                        fresh.insert(action, st);
                        continue;
                    };
                    st.is_attack = d.is_melee_auto_attack();
                    // What `IsCurrentAction 0x4e53a0` and `IsAutoRepeatAction 0x4e55b0` hold
                    // against the live cast state. The form arm is not the icon's aura scan: a form
                    // granted by another spell lights the check but not the icon.
                    st.spell = Some(button.action);
                    st.form = d.shapeshift_form;
                    st.profession = profession(button.action, d, skill_lines.as_deref());
                    // The usable walk (`0x6e3d60`); only its power gate sets `notEnoughMana`.
                    if let (Some((store, _, _)), Some(sp)) = (me, spells.as_deref()) {
                        let ctx = usable::UsableCtx {
                            store,
                            target: selection.guid.zip(target.map(|(s, _)| s)),
                            rel: relations.get(),
                            cooldowns: &cooldowns,
                            stacks: &stacks,
                            spell_mods,
                            control_lost,
                            skill_lines: skill_catalog,
                        };
                        let (u, oom) =
                            usable::spell_usable(button.action, d, sp, &ctx, &objects, &items);
                        st.usable = u;
                        st.not_enough_mana = oom;
                    } else {
                        st.usable = true;
                    }
                    let row = spells.as_ref().and_then(|s| s.ranges.get(d.range_index));
                    // `ActionHasRange 0x4e5810` asks `GetMinMaxRange` with a null target
                    // (`0x4e5852`) and tests `|min|` and `|max|` against `FLT_EPSILON`. The
                    // auto-attack target the melee arm falls back to cannot move that: its max is
                    // at least the 5.0 floor.
                    st.has_range = spell_mods
                        .min_max_range(d, row, caster, RangeTargets::default())
                        .is_some_and(|(min, max)| {
                            min.abs() > f32::EPSILON || max.abs() > f32::EPSILON
                        });
                    // `IsActionInRange 0x4e56f0`: nil with no player (`0x4e572b`), else the
                    // pre-check `0x6e4440` on the selection, whose compare `0x6e47b0` hands
                    // `GetMinMaxRange` that target (`0x6e47ca`): its reach pads a ranged row, and
                    // its motion with the caster's earns the moving bonus.
                    st.in_range = me.and_then(|_| {
                        crate::spell::range_check::action_in_range(d, &cast_ctx, row, spell_mods)
                    });
                    let info = cooldowns.info(button.action, 0, Some(d), now);
                    st.cooldown = info.ui_triple(anchor, ui_now);
                    if st.cooldown.is_some() && trace_cd {
                        // The store's clock against the widget's; the sink stamps wall time.
                        benilla_assets::trace::line(
                            "cd",
                            &format!(
                                "tick action={} rem={}ms dur={}ms engine_now={ui_now:.3}",
                                button.action, info.remaining_ms, info.duration_ms,
                            ),
                        );
                    }
                }
                ACTION_KIND_ITEM => {
                    // Only the `Copy` on-use spell, not a clone of the template per slot per frame.
                    // The getter's lookup passes a callback (`0x4e5b3f`) on the bar's events, which
                    // no event here marks: an entry is asked when it first shows.
                    let template = items.template(button.action, 0, Ask::New(&commands));
                    let awaited = match template {
                        None if !items.template_answered_unknown(button.action) => {
                            memory.awaiting_records.insert(button.action);
                            false
                        }
                        _ => memory.awaiting_records.remove(&button.action),
                    };
                    let use_spell = template.and_then(|t| t.use_spell);
                    // `IsCurrentAction`'s item arm holds the entry against the in-flight item
                    // (`0x4e53fc`-`0x4e5428`).
                    st.item = Some(button.action);
                    // `GetItemCooldown 0x6e2ed0` queries only an item whose record is cached and
                    // has an on-use spell (`0x6e2f14`, `0x6e2f20`-`0x6e2f34`).
                    st.no_cooldown_query = use_spell.is_none();
                    // The count the slot holds (`0xbc6390`), which `GetActionCount` answers too:
                    // the gate reads the cache the recount wrote, never the bags (`0x4e50ab`).
                    let count = script.action_slot(action).map_or(0, |slot| slot.count);
                    // `IsEquippedAction`, the green border: `0x622270` at mask 1
                    // (`0x4e5377`-`0x4e5380`), the walker over the worn slots, so a broken copy is
                    // not worn.
                    st.equipped = me.is_some_and(|(s, _, _)| {
                        crate::ui_items::find_item(
                            &s.0,
                            &objects,
                            button.action,
                            crate::ui_items::ItemSearch {
                                equipment_only: true,
                                ..Default::default()
                            },
                        )
                        .is_some()
                    });
                    // `0x4e5050`'s item arm runs the count, `IsItemOnCooldown` and the on-use spell
                    // through the same `0x6e3d60` walk. With no player the reference answers (0, 0)
                    // (`0x4e5080`), the default state.
                    if let Some((store, _, _)) = me {
                        let ctx = usable::UsableCtx {
                            store,
                            target: selection.guid.zip(target.map(|(s, _)| s)),
                            rel: relations.get(),
                            cooldowns: &cooldowns,
                            stacks: &stacks,
                            spell_mods,
                            control_lost,
                            skill_lines: skill_catalog,
                        };
                        let (u, oom) = usable::item_usable(
                            button.action,
                            use_spell.as_ref(),
                            count > 0,
                            &ctx,
                            spells.as_deref(),
                            &objects,
                            &items,
                        );
                        st.usable = u;
                        st.not_enough_mana = oom;
                    }
                    if let Some(u) = use_spell {
                        let d = spells.as_ref().and_then(|s| s.catalog.get(u.spell_id));
                        let info =
                            cooldowns.info_item(u.spell_id, button.action, u.category, d, now);
                        st.cooldown = info.ui_triple(anchor, ui_now);
                        // The callback queries again and flushes while the cooldown ends after now.
                        record_flush |= awaited && info.remaining_ms > 0;
                    }
                }
                _ => {}
            }
            // The triple holds the absolute start, so a running cooldown diffs equal every frame
            // and a re-arm, even within one frame gap, restarts the sweep.
            fresh.insert(action, st);
        }

        // The usable cache: the slots this frame wrote store their pairs silently, then a refresh
        // compares them all.
        let words: HashMap<u32, SlotWord> = actions
            .buttons
            .iter()
            .map(|(&slot, b)| {
                let action = u32::from(slot) + 1;
                let count = script.action_slot(action).map_or(0, |s| s.count);
                let resolved = resolve_through_macro(b.kind, b.action, bound);
                (action, (b.kind, b.action, resolved, count))
            })
            .collect();
        let pairs: HashMap<u32, (bool, bool)> = fresh
            .iter()
            .map(|(&action, st)| (action, (st.usable, st.not_enough_mana)))
            .filter(|&(_, pair)| pair != (false, false))
            .collect();
        cache.notify(words, &pairs, arrived.then_some(&actions.spells));
        let usable_changed = refresh_cache && cache.refresh(pairs);

        let keys: Vec<u32> = fresh
            .keys()
            .chain(memory.pushed.keys())
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        for action in keys {
            let (new, old) = (fresh.get(&action), memory.pushed.get(&action));
            let d = ActionState::default();
            let (n, o) = (new.unwrap_or(&d), old.unwrap_or(&d));
            if new == old {
                continue;
            }
            if n.cooldown != o.cooldown && benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "cd",
                    &format!(
                        "push action={action} cooldown={:?} engine_now={:.3}",
                        n.cooldown,
                        script.now()
                    ),
                );
            }
            script.set_action_state(action, new.copied());
        }
        memory.pushed = fresh;

        // The reactive list: emptied at an arrival, filled from the book, and re-checked inside
        // each usable refresh after the bar (`0x4b31d7`-`0x4b323d`), on the walk the bar takes.
        let list = &mut *reactive;
        if arrived {
            list.arrive();
        }
        if let Some(sp) = spells.as_deref() {
            if arrived || actions.is_changed() || spells.as_ref().is_some_and(|s| s.is_changed()) {
                list.sync(&actions.spells, &sp.catalog);
            }
        }
        let activated = match (me, spells.as_deref()) {
            (Some((store, _, _)), Some(sp)) if refresh => {
                let ctx = usable::UsableCtx {
                    store,
                    target: selection.guid.zip(target.map(|(s, _)| s)),
                    rel: relations.get(),
                    cooldowns: &cooldowns,
                    stacks: &stacks,
                    spell_mods,
                    control_lost,
                    skill_lines: skill_catalog,
                };
                list.refresh(|id| {
                    sp.catalog
                        .get(id)
                        .is_some_and(|d| usable::spell_usable(id, d, sp, &ctx, &objects, &items).0)
                })
            }
            _ => Vec::new(),
        };

        owed.cooldowns |= gen_changed || record_flush;
        owed.usable_changed |= usable_changed;
        owed.refresh |= refresh;
        owed.activated.extend(activated);
        owed.bags |= bags || record_flush;
        if engaged != memory.engaged {
            memory.engaged = engaged;
            owed.combat = Some(engaged);
        }
        if auto_repeat.0 != memory.auto_repeat {
            memory.auto_repeat = auto_repeat.0;
            owed.auto_repeat = Some(auto_repeat.0.is_some());
        }
    })
}

/// Signals what [`feed_action_state`] owes, after the unit snapshots: `SelectTarget` stores the
/// target (`0x4936e2`) before its refresh (`0x493892`), and a watched field is written before its
/// watch runs, so a handler reads the units the refresh followed. In the reference's order: each
/// flush's `ACTIONBAR_*` event first; the cooldown pair ahead of the usable refresh a held cooldown
/// or `0x6e3050` runs (`0x6e2e77`, `0x6e3071`), and both ahead of the bags (`0x6e3089`).
/// `ACTIONBAR_UPDATE_USABLE` comes from the cache's refresh alone, `SPELL_UPDATE_USABLE` from
/// `0x4b31c0` alone (`0x4b3246`), so the cache's other callers signal the first without the second.
pub(super) fn fire_action_events(
    mut hosted_vm: crate::script_calls::HostedVm,
    mut owed: ResMut<ActionEvents>,
) {
    let owed = std::mem::take(&mut *owed);
    hosted_vm.run(|script| {
        let Some(script) = script else {
            return;
        };
        if owed.cooldowns {
            script.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
            script.fire_event("SPELL_UPDATE_COOLDOWN", vec![]);
        }
        // `0x4b31c0`: the bar's event on a slot change (`0x4e5c53`), each reactive spell's word as
        // it turns usable (`0x4b3226`), then the spells' event (`0x4b3246`).
        if owed.usable_changed {
            script.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
        }
        for name in owed.activated {
            crate::ui_unit::fire_combat_text(script, "SPELL_ACTIVE", Some(name), None);
        }
        if owed.refresh {
            script.fire_event("SPELL_UPDATE_USABLE", vec![]);
        }
        if owed.bags {
            script.fire_event("BAG_UPDATE_COOLDOWN", vec![]);
        }
        if let Some(engaged) = owed.combat {
            script.fire_event(
                if engaged {
                    "PLAYER_ENTER_COMBAT"
                } else {
                    "PLAYER_LEAVE_COMBAT"
                },
                vec![],
            );
        }
        if let Some(on) = owed.auto_repeat {
            script.fire_event(
                if on {
                    "START_AUTOREPEAT_SPELL"
                } else {
                    "STOP_AUTOREPEAT_SPELL"
                },
                vec![],
            );
        }
    })
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
