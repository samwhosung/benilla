//! Each action slot's state behind `IsUsableAction`, `IsActionInRange`, `IsCurrentAction` and
//! `GetActionCooldown`, read at the getter's call through the lent host
//! ([`ActionStates`]'s live family), as the reference's getters resolve the slot and test the live
//! player inside the call (`0x4e5a50`). The usable cache's refresh `0x4e5c00` alone, with its
//! `ACTIONBAR_UPDATE_USABLE` when a pair changed, runs in its callers' places
//! ([`ActionStates::cache_refresh`]): the world entry's cascade, `EditMacro`'s call and each
//! `RaiseBagUpdate` of an item watcher's run. The usable refresh `0x4b31c0`, with each reactive
//! spell's `SPELL_ACTIVE`
//! ([`super::reactive`]) and `SPELL_UPDATE_USABLE`, runs where its callers make it: a selection's
//! inside `SetSelection` ([`ActionStates::usable_refresh`]), the field watchers' and the control
//! flag's inside their packets on the notice lane
//! ([`crate::field_watch::FieldNotice::UsableRefresh`]), and the cooldown list's on the cooldown
//! lane ([`super::cooldown_lane`]). The cooldown list's flushes (`0x4b31b0`, `0x4f93d0`) fire on the
//! cooldown lane, as the spell charges watcher's `BAG_UPDATE_COOLDOWN` fires on the item lane
//! ([`crate::item_lane`]). Combat enter and leave are signalled in their attack packets
//! (`crate::creature_anim`).
//! `IsCurrentAction` reads the cast state live, and its pair (`0x4b3250`) is signalled where the
//! cast changes, as are autorepeat start and stop where the key is stored and cleared
//! (`0x6e5952`, `0x6ea170`; [`crate::ui_cast`]).
//! Usability is `IsSpellUsableNow 0x6e3d60` ([`crate::spell::usable`]), read off the player's and
//! the target's stores as of the drain's place, and `IsUsableAction` answers the bar's cache of it
//! (`0x4e5230`); auto-repeat is the `0xceac30` key, and range the pre-check `0x6e4440` on the
//! selection ([`crate::spell::range_check`]).

use crate::ui_items::stack_counts;
use std::collections::HashMap;
use std::time::Instant;

use bevy::prelude::*;

use benilla_formats::RangeTargets;
use benilla_protocol::messages::{ACTION_KIND_ITEM, ACTION_KIND_SPELL};
use benilla_ui::script::{
    macro_slot_id, ActionAttackFaceNow, ActionConsumableNow, ActionCountNow, ActionSlot,
    ActionState, ActionStateNow, ActionTextureNow, ActionUsableNow,
};

use crate::items::Items;
use crate::net::{NetCommands, ObjectStore};
use crate::spell::Cooldowns;

use super::{ActionSlots, PlayerActions, Spells};
use crate::query_cache::Ask;
use crate::spell::usable;

/// What the action bar's usable cache (`0xbc6b60`, `0xbc67a0`) stores, which outlives a VM as the
/// reference's does: only a slot write (`0x4e58e0`, `0x4e590a`-`0x4e5914`) and a refresh
/// (`0x4e5c00`) store a pair, and only `ResetActionBarState 0x4e4f50`, from `InitializeGame
/// 0x48f480`, zeroes them.
#[derive(Default)]
pub(super) struct UsableCache {
    /// Each slot's `(usable, notEnoughMana)` pair, `(false, false)` left out.
    pairs: HashMap<u32, (bool, bool)>,
}

impl UsableCache {
    /// `0x4e58e0`'s store of one slot's pair (`0x4e590a`, `0x4e5914`).
    fn store(&mut self, action: u32, pair: (bool, bool)) {
        if pair == (false, false) {
            self.pairs.remove(&action);
        } else {
            self.pairs.insert(action, pair);
        }
    }

    /// `0x4e5c00`: every pair recomputed and stored; whether any changed, its
    /// `ACTIONBAR_UPDATE_USABLE` (`0x4e5c4e`).
    fn refresh(&mut self, pairs: HashMap<u32, (bool, bool)>) -> bool {
        std::mem::replace(&mut self.pairs, pairs) != self.pairs
    }

    /// `0x4e5230`: the slot's stored `(usable, notEnoughMana)`, what `IsUsableAction` answers.
    fn pair(&self, action: u32) -> (bool, bool) {
        self.pairs.get(&action).copied().unwrap_or_default()
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
    (race, class): (u8, u8),
) -> Option<benilla_ui::script::Profession> {
    use benilla_ui::script::Profession;
    if d.effects[0] != benilla_formats::SPELL_EFFECT_TRADE_SKILL {
        return None;
    }
    match u32::try_from(d.effect_misc_value[0]).unwrap_or(0) {
        0 => skill_lines
            .and_then(|s| s.catalog.spell_to_line(spell_id, race, class))
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
    /// What the player's level for the spell reads, its cost's level term.
    skills: crate::spell::PlayerSkills<'a>,
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
        pet_spell: Some(leg.spell),
        ..Default::default()
    };
    if let (Some(player), Some(pet)) = (leg.player, leg.pet) {
        let cost = usable::power_cost(leg.d, player, leg.skills, leg.mods);
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

/// `0x622270` at mask 1, `IsEquippedAction`'s walk (`0x4e5380`): whether the worn slots of the
/// player `store` hold a whole item of `entry`. A broken copy is not worn (`0x6224f4`-`0x62250e`).
/// The lent host's read at the call and the states the bar's writes push are this one walk.
pub(crate) fn wears_entry(
    store: &ObjectStore,
    objects: &(impl crate::ui_items::ItemObjects + ?Sized),
    entry: u32,
) -> bool {
    crate::ui_items::find_item(
        &store.0,
        objects,
        entry,
        crate::ui_items::ItemSearch {
            equipment_only: true,
            ..Default::default()
        },
    )
    .is_some()
}

/// The float `ActionHasRange` compares each range with (`0x8029d4`): 0x34800000, 2^-22.
const HAS_RANGE_BOUND: f32 = f32::from_bits(0x3480_0000);

/// What the two range queries read beside the spell.
struct RangeLeg<'a, 'w> {
    caster: benilla_formats::RangeUnit,
    mods: &'a crate::spell::SpellModifiers,
    cast_ctx: &'a crate::spell::cast_target::CastContext<'w>,
    /// Whether there is an active player, which `IsActionInRange` requires (`0x4e572b`).
    player: bool,
}

/// `ActionHasRange` and `IsActionInRange` for the spell a book-0 slot stands for:
/// - `0x4e5810` asks `GetMinMaxRange` with a null target (`0x4e5852`) and answers whether `|min|`
///   or `|max|` is not below [`HAS_RANGE_BOUND`] (`fcomp`, `test ah,5`, `0x4e5885`, `0x4e5898`),
///   which an unordered compare passes too. The auto-attack target the melee arm falls back to
///   cannot move that: its max is at least the 5.0 floor;
/// - `0x4e56f0` is nil with no player, else the pre-check `0x6e4440` on the selection, whose
///   compare `0x6e47b0` hands `GetMinMaxRange` that target (`0x6e47ca`): its reach pads a ranged
///   row, and its motion with the caster's earns the moving bonus.
fn range_answers(
    d: &benilla_formats::SpellDisplay,
    spells: Option<&Spells>,
    leg: &RangeLeg<'_, '_>,
) -> (bool, Option<bool>) {
    let row = spells.and_then(|s| s.ranges.get(d.range_index));
    let a_range = |x: f32| x.abs() >= HAS_RANGE_BOUND || x.is_nan();
    let has_range = leg
        .mods
        .min_max_range(d, row, leg.caster, RangeTargets::default())
        .is_some_and(|(min, max)| a_range(min) || a_range(max));
    let in_range = leg
        .player
        .then(|| crate::spell::range_check::action_in_range(d, leg.cast_ctx, row, leg.mods))
        .flatten();
    (has_range, in_range)
}

/// What the action slots' states are computed from: the selection with the relations and
/// positions `IsActionInRange`'s pre-check reads, the cooldowns, and the pet's bar (the gate and
/// the history a macro bound to a pet spell reads).
#[derive(bevy::ecs::system::SystemParam)]
#[allow(clippy::type_complexity)] // every input the slot walk reads, as one param
pub(crate) struct ActionWalk<'w, 's> {
    cooldowns: Res<'w, Cooldowns>,
    /// The interface clock's pair every cooldown start is mapped through, absent in a harness
    /// that seeds none.
    clock: Option<Res<'w, crate::ui_script::HostClock>>,
    targeting: crate::spell::cast_target::CastTargeting<'w, 's>,
    pet_bar: Option<Res<'w, crate::ui_pet::PetBar>>,
    /// The slots as the calls so far left them.
    slots: Res<'w, ActionSlots>,
    spells: Option<Res<'w, Spells>>,
    // Spell mods, the control flag and the skill lines. The range compare reads
    // the caster and the target through the targeting's context.
    cast_state: (
        Res<'w, crate::spell::SpellModifiers>,
        // `[0xb4b3e4]`, which the usable walk reads.
        Option<Res<'w, crate::player::OwnControlLost>>,
        // A profession opener's line (`0x6de040`), which the checked test reads, and the skill
        // lines the walk's cost finds its level through.
        Option<Res<'w, crate::ui_spellbook::SkillLines>>,
    ),
    // The guid index and the item store, and the units' stores as of the drain's place: a field
    // watcher's refresh reads its own packet's power, aura state and form.
    objects: crate::net::Objects<'w, 's>,
    views: crate::ui_unit::views::ViewStores<'w, 's>,
    relations: crate::target::RelationWorld<'w, 's>,
    // The item cache and the channel its misses query on.
    items: Res<'w, Items>,
    commands: Res<'w, NetCommands>,
    // `ItemSubClass.dbc`, whose flags the walk's ammo leg reads.
    sub_classes: Option<Res<'w, crate::ui_items::ItemSubClasses>>,
    /// `ItemDisplayInfo.dbc`, which an item slot's icon and the attack faces read.
    icons: Option<Res<'w, crate::entities::ItemDisplays>>,
}

/// The slot walk ([`ActionWalk`]) with what the bar's writes and refreshes store: the usable
/// cache, and the registrar's reactive list, which each usable refresh re-checks
/// (`0x4b31d7`-`0x4b323d`). Each caller runs its own in its place: a slot write's store
/// ([`Self::slot_written`]), our player's arrival's ([`Self::arrival_slots`]), the usable refresh
/// a selection change runs inside `SetSelection` ([`Self::usable_refresh`], `0x493892`,
/// `0x493a69`) and the cache's refresh alone ([`Self::cache_refresh`]).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ActionStates<'w, 's> {
    walk: ActionWalk<'w, 's>,
    memory: ResMut<'w, ActionStateMemory>,
    /// The known spells, which the arrival's load tests (`0x60c740`).
    actions: Res<'w, PlayerActions>,
    /// The reactive list; absent in a harness that seeds none.
    reactive: Option<ResMut<'w, super::ReactiveSpells>>,
}

/// What the action states remember between passes: the bar's usable cache and its item counts,
/// which live for the session, not for a VM.
#[derive(Resource, Default)]
pub(crate) struct ActionStateMemory {
    cache: UsableCache,
    /// `0xbc6390`: each item slot's count, by Lua action id, which only a recount (`0x4e5caa`) and
    /// an item word's write (`0x4e5d90`, `0x4e66f1`) store, and `GetActionCount` reads
    /// (`0x4e6c70`).
    counts: HashMap<u32, u32>,
}

impl ActionStateMemory {
    /// `ResetActionBarState 0x4e4f50`'s zeroing of the usable and no-power flags and the counts,
    /// at the game entry (`0x48f5a4`).
    pub(super) fn game_entry(&mut self) {
        self.cache.pairs.clear();
        self.counts.clear();
    }

    /// The count `0xbc6390` holds for `action`, 0 never stored.
    fn count(&self, action: u32) -> u32 {
        self.counts.get(&action).copied().unwrap_or(0)
    }
}

/// What a usable refresh signals ([`ActionStates::usable_refresh`]), in the reference's order:
/// the bar's `ACTIONBAR_UPDATE_USABLE` when a pair changed (`0x4e5c53`), each reactive spell that
/// turned usable (`SPELL_ACTIVE`, `0x4b3226`), then `SPELL_UPDATE_USABLE` (`0x4b3246`).
pub(crate) struct UsableSignals {
    usable_changed: bool,
    activated: Vec<String>,
}

impl UsableSignals {
    /// Signal them on `script`, in place.
    pub(crate) fn fire(self, script: &mut benilla_ui::script::UiScript) {
        if self.usable_changed {
            script.fire_event("ACTIONBAR_UPDATE_USABLE", vec![]);
        }
        for name in self.activated {
            crate::ui_unit::fire_combat_text(script, "SPELL_ACTIVE", Some(name), None);
        }
        script.fire_event("SPELL_UPDATE_USABLE", vec![]);
    }
}

impl ActionStates<'_, '_> {
    /// The usable refresh `0x4b31c0` a selection change runs inside `SetSelection`, after the
    /// target is stored (`0x4936e2`, `0x4939d3`) and before `PLAYER_TARGET_CHANGED` (`0x493892`
    /// ahead of `0x49389c`, `0x493a69` ahead of `0x493a73`): every slot's state recomputed against
    /// the new selection and pushed, the bar's cache refreshed, the reactive list re-checked; and
    /// the same for its other callers, each in its place.
    pub(crate) fn usable_refresh(
        &mut self,
        script: &mut benilla_ui::script::UiScript,
    ) -> UsableSignals {
        let (fresh, activated) = self.walk.all(
            script,
            &self.memory,
            Instant::now(),
            self.reactive.as_mut().map(|r| &mut r.0),
        );
        let usable_changed = self.memory.cache.refresh(pairs(&fresh));
        push(script, &fresh, &self.memory.cache, None);
        UsableSignals {
            usable_changed,
            activated,
        }
    }

    /// `RefreshActionBarUsable 0x4e5c00` in its caller's place, outside the usable refresh: the
    /// world-enter cascade's (`0x490979` → `0x4e4f90`), `EditMacro`'s (`0x4f132e`) and
    /// `RaiseBagUpdate`'s (`0x4f9374`). Every slot's state recomputed and pushed and its pair
    /// stored; whether a pair changed, which signals `ACTIONBAR_UPDATE_USABLE` (`0x4e5c53`).
    pub(crate) fn cache_refresh(&mut self, script: &mut benilla_ui::script::UiScript) -> bool {
        let fresh = self.walk.all(script, &self.memory, Instant::now(), None).0;
        let changed = self.memory.cache.refresh(pairs(&fresh));
        push(script, &fresh, &self.memory.cache, None);
        changed
    }

    /// `0x4e58e0` for the 1-based `action`, which a write just stored or a notify names: its pair
    /// recomputed and stored (`0x4e5903`-`0x4e5914`) and its state pushed, with no refresh and no
    /// event, ahead of the slot's `ACTIONBAR_SLOT_CHANGED` (`0x4e5998`). The callers: the verbs'
    /// writes inside their calls, `EditMacro`'s notifies (`0x4f1329` → `0x4e5cc0`), the spell
    /// packets' clears and replaces, and the recounts (`0x4e5cb1`).
    pub(crate) fn slot_written(&mut self, script: &mut benilla_ui::script::UiScript, action: u32) {
        let Some(slot) = action
            .checked_sub(1)
            .and_then(|s| u8::try_from(s).ok())
            .filter(|&s| s < 120)
        else {
            return;
        };
        let fresh = self
            .walk
            .slots_states(script, &self.memory, Some(slot), Instant::now(), None)
            .0;
        let pair = fresh
            .get(&action)
            .map_or((false, false), |st| (st.usable, st.not_enough_mana));
        self.memory.cache.store(action, pair);
        push(script, &fresh, &self.memory.cache, Some(action));
    }

    /// The known spells, which the arrival's load tests a spell word against (`0x60c740`).
    pub(crate) fn known_spells(&self) -> std::collections::BTreeSet<u32> {
        self.actions.spells.clone()
    }

    /// Our player's arrival's slot pass (`0x5deae2`-`0x5deb12`) over the 1-based slots `loaded`,
    /// the words its load stored ([`super::ActionSlots::load_staged`]): each written through
    /// `0x4e5d60` with no send and quiet set (`0x5deafc`), into the VM's copy, each item word
    /// recounted first (`0x4e5d90`), and its pair stored (`0x4e58e0`). A slot the load skipped keeps
    /// its word, its count and its pair. Nothing else recounts a worldport: its items are created
    /// again before our player, outside the world latch, and run no record set-up.
    pub(crate) fn arrival_slots(
        &mut self,
        script: &mut benilla_ui::script::UiScript,
        loaded: &[u32],
    ) {
        let mut counted: HashMap<u32, u32> = HashMap::new();
        for &action in loaded {
            let button = self.walk.button(action).copied();
            if let Some(b) = button.filter(|b| b.kind == ACTION_KIND_ITEM) {
                let count = *counted
                    .entry(b.action)
                    .or_insert_with(|| self.walk.item_count(b.action));
                self.memory.counts.insert(action, count);
            }
            self.mirror(script, action);
        }
        let fresh = self.walk.all(script, &self.memory, Instant::now(), None).0;
        for &action in loaded {
            let pair = fresh
                .get(&action)
                .map_or((false, false), |st| (st.usable, st.not_enough_mana));
            self.memory.cache.store(action, pair);
        }
        push(script, &fresh, &self.memory.cache, None);
    }

    /// A new VM's copy of every slot, with the counts the bar holds, and every slot's state, with
    /// nothing signalled: the bar and its tables outlive a VM (`0x4e4f50` runs only at the game
    /// entry, `0x48f5a4`).
    pub(crate) fn seed(&mut self, script: &mut benilla_ui::script::UiScript) {
        for action in 1..=120u32 {
            self.mirror(script, action);
        }
        let fresh = self.walk.all(script, &self.memory, Instant::now(), None).0;
        push(script, &fresh, &self.memory.cache, None);
    }

    /// `0x4e6d20(entry)`, the count a recount stores ([`ActionWalk::item_count`]).
    pub(crate) fn item_count(&self, entry: u32) -> u32 {
        self.walk.item_count(entry)
    }

    /// `0x4e5c70`'s write of the 1-based `action` when the bar holds `entry` there: `count` stored
    /// (`0x4e5caa`), the VM's copy of the slot with it, then the slot's pair (`0x4e5cb1` into
    /// `0x4e58e0`); whether it held the entry.
    pub(crate) fn recounted(
        &mut self,
        script: &mut benilla_ui::script::UiScript,
        action: u32,
        entry: u32,
        count: u32,
    ) -> bool {
        let holds = self
            .walk
            .button(action)
            .is_some_and(|b| b.kind == ACTION_KIND_ITEM && b.action == entry);
        if holds {
            self.memory.counts.insert(action, count);
            self.mirror(script, action);
            self.slot_written(script, action);
        }
        holds
    }

    /// `0x4e5de0`'s step for the 1-based `action`: when the slot resolves to `spell` with book 0
    /// (`0x4e5df5`-`0x4e5e03`), its pair stored as `0x4e58e0` with no send stores it; whether it
    /// did.
    pub(crate) fn spell_slot_notified(
        &mut self,
        script: &mut benilla_ui::script::UiScript,
        action: u32,
        spell: u32,
    ) -> bool {
        let stands = spell != 0 && self.slot_spell(script, action) == Some((spell, false));
        if stands {
            self.slot_written(script, action);
        }
        stands
    }

    /// `GetActionSlotSpell 0x4e5a50` for the 1-based `action`: the spell and its book flag. A spell
    /// word's own with book 0; an item's on-use spell with book 0 (`0x4e5ae0`, its record looked
    /// up with the bar's callback); a macro's record's cached spell and book (`0x4e5ba0`, `+0x564`,
    /// `+0x568`) while the macro exists (`0x4e5030`). `None` for nothing to stand for.
    fn slot_spell(
        &self,
        script: &benilla_ui::script::UiScript,
        action: u32,
    ) -> Option<(u32, bool)> {
        let b = self.walk.button(action)?;
        match b.kind {
            ACTION_KIND_SPELL => Some((b.action, false)),
            ACTION_KIND_ITEM => self
                .walk
                .bar_record(b.action)?
                .use_spell
                .map(|u| (u.spell_id, false)),
            kind => {
                let id = macro_slot_id(kind, b.action)?;
                script.macros().by_id(id)?;
                let record = script.macro_bindings().get(&id).copied()?;
                Some((u32::try_from(record.spell).ok()?, record.pet_book))
            }
        }
    }

    /// An item word's write inside its verb (`PlaceAction`'s `0x4e66e9`-`0x4e66f1`,
    /// `SetActionSlot`'s `0x4e5d90`): the slot's count taken and stored, the VM's copy with it.
    pub(crate) fn item_written(&mut self, script: &mut benilla_ui::script::UiScript, action: u32) {
        let Some(b) = self.walk.button(action).copied() else {
            return;
        };
        if b.kind == ACTION_KIND_ITEM {
            let count = self.walk.item_count(b.action);
            self.memory.counts.insert(action, count);
            self.mirror(script, action);
        }
    }

    /// The VM's copy of the 1-based `action` set to the bar's word, with the count it holds and
    /// what the cache holds of an item's record, asking nothing; cleared for an empty slot.
    fn mirror(&self, script: &mut benilla_ui::script::UiScript, action: u32) {
        let slot = self
            .walk
            .button(action)
            .map(|b| self.walk.mirror_of(b, self.memory.count(action)));
        script.set_action(action, slot);
    }

    /// `GetActionTexture`'s answer for a spell or item slot ([`ActionTextureNow`]).
    fn texture_now(&self, action: u32) -> Option<String> {
        let b = self.walk.button(action)?;
        // `0x4e6a50`'s attack and ranged tests run on the slot's spell ahead of its kind arms
        // (`0x4e6ab2`, `0x4e6acb`), an item word's its on-use spell (`0x4e5a50` → `0x4e5ae0`, its
        // record looked up with the bar's callback); a spell word's are its own face's.
        if b.kind == ACTION_KIND_ITEM {
            let spell = self.walk.bar_record(b.action).and_then(|t| t.use_spell);
            if let Some(face) = spell.and_then(|u| self.walk.attack_face(u.spell_id)) {
                return Some(face);
            }
        }
        self.walk.face(b, true)
    }

    /// A macro slot's attack faces for its bound `spell` ([`ActionAttackFaceNow`]).
    fn attack_face_now(&self, spell: u32) -> Option<String> {
        self.walk.attack_face(spell)
    }

    /// `GetActionCount`'s answer (`0x4e6c70`): the stored count for an item slot, else 0.
    fn count_now(&self, action: u32) -> u32 {
        match self.walk.button(action) {
            Some(b) if b.kind == ACTION_KIND_ITEM => self.memory.count(action),
            _ => 0,
        }
    }

    /// `IsConsumableAction`'s answer (`0x4e5250`): an item slot whose record the cache holds and
    /// that is consumable ([`benilla_protocol::messages::ItemInfo::is_consumable`]).
    fn consumable_now(&self, action: u32) -> bool {
        self.walk.button(action).is_some_and(|b| {
            b.kind == ACTION_KIND_ITEM
                && self
                    .walk
                    .items
                    .template(b.action, 0, Ask::Read)
                    .is_some_and(|t| t.is_consumable())
        })
    }

    /// One slot's state as of here, for a getter's live read ([`ActionStateNow`]): its word's, a
    /// macro word's through the VM's record the getter hands over; `None` for an empty slot. Its
    /// usable pair is unread: `IsUsableAction` asks [`ActionUsableNow`].
    fn state_now(&mut self, q: &ActionStateNow) -> Option<ActionState> {
        self.walk.state_now(q)
    }

    /// `IsUsableAction`'s pair for `action` as the bar's cache holds it (`0x4e5230`), an empty
    /// slot's `(false, false)`.
    fn usable_now(&self, action: u32) -> (bool, bool) {
        let occupied = action
            .checked_sub(1)
            .and_then(|s| u8::try_from(s).ok())
            .is_some_and(|slot| self.walk.slots.buttons.contains_key(&slot));
        if occupied {
            self.memory.cache.pair(action)
        } else {
            (false, false)
        }
    }
}

/// The pairs a walk found, `(false, false)` left out, as the cache holds them.
fn pairs(fresh: &HashMap<u32, ActionState>) -> HashMap<u32, (bool, bool)> {
    fresh
        .iter()
        .map(|(&action, st)| (action, (st.usable, st.not_enough_mana)))
        .filter(|&(_, pair)| pair != (false, false))
        .collect()
}

/// Push `fresh` into the VM, each slot with the pair the cache holds (`IsUsableAction` reads the
/// cache, `0x4e5230`, not the walk), and clear the states of the slots it does not hold: every
/// slot, or `only`. A state the VM already holds is not pushed again.
fn push(
    script: &mut benilla_ui::script::UiScript,
    fresh: &HashMap<u32, ActionState>,
    cache: &UsableCache,
    only: Option<u32>,
) {
    let actions = match only {
        Some(action) => action..=action,
        None => 1..=120,
    };
    for action in actions {
        let new = fresh.get(&action).map(|st| {
            let (usable, not_enough_mana) = cache.pair(action);
            ActionState {
                usable,
                not_enough_mana,
                ..*st
            }
        });
        let old = script.action_state(action);
        if new == old {
            continue;
        }
        if benilla_assets::trace::enabled() && new.map(|n| n.cooldown) != old.map(|o| o.cooldown) {
            benilla_assets::trace::line(
                "cd",
                &format!(
                    "push action={action} cooldown={:?} engine_now={:.3}",
                    new.and_then(|n| n.cooldown),
                    script.now()
                ),
            );
        }
        script.set_action_state(action, new);
    }
}

impl ActionWalk<'_, '_> {
    /// The word the bar holds at the 1-based `action`.
    fn button(&self, action: u32) -> Option<&benilla_protocol::messages::ActionButton> {
        let slot = u8::try_from(action.checked_sub(1)?).ok()?;
        self.slots.buttons.get(&slot)
    }

    /// The active player's object, `None` while none is held.
    fn me_live(&self) -> Option<&ObjectStore> {
        self.objects.player().and_then(|e| self.objects.store_of(e))
    }

    /// The item cache's lookup of `entry` with the bar's callback `0x4e5b80` and the dedupe flag
    /// (`0x55ba30` from `0x4e5b30`, `0x4e6b10`, `0x4e6d69`, each with `push 1`): a miss asks the
    /// record and keeps the callback once, which recounts the entry at the answer
    /// ([`super::recount`]).
    fn bar_record(&self, entry: u32) -> Option<&benilla_protocol::messages::ItemInfo> {
        let template = self.items.template(entry, 0, Ask::Event(&self.commands));
        if template.is_none() {
            self.items
                .wait_once(entry, crate::items::RecordCallback::ActionSlots);
        }
        template
    }

    /// `0x4e6d20(entry)`: 0 with no active player (`0x4e6d5e`), else the entry's record asked with
    /// the bar's callback (`0x4e6d78`) and its charges or stacks over mask 0
    /// ([`crate::ui_items::action_count`]).
    pub(crate) fn item_count(&self, entry: u32) -> u32 {
        let Some(store) = self.me_live() else {
            return 0;
        };
        let template = self.bar_record(entry);
        crate::ui_items::action_count(&store.0, &self.objects, entry, template)
    }

    /// The active player's object as of the drain's place, `None` while none is held.
    fn me_view(&self) -> Option<&ObjectStore> {
        self.objects.player().and_then(|e| self.views.get(e).ok())
    }

    /// `GetActionSlotTexture 0x4e6a50` for a spell or item word, off the player as of the drain's
    /// place, so a toggle's listener reads its own packet's aura (`0x4e55f0`): an item's
    /// display icon, never nil (the resolver `0x5d88b0`'s question mark while the record is not
    /// cached or its display has no icon), looked up with the bar's callback when `ask`
    /// (`0x4e6b24`); a spell's [`super::spell_action_icon`]. A macro's is its record's, the VM's.
    fn face(&self, b: &benilla_protocol::messages::ActionButton, ask: bool) -> Option<String> {
        match b.kind {
            ACTION_KIND_ITEM => {
                let template = if ask {
                    self.bar_record(b.action)
                } else {
                    self.items.template(b.action, 0, Ask::Read)
                };
                Some(
                    template
                        .and_then(|t| {
                            self.icons
                                .as_ref()?
                                .catalog
                                .get(t.display_info_id)?
                                .icon
                                .clone()
                        })
                        .unwrap_or_else(|| super::feed::MISSING_ITEM_ICON.to_string()),
                )
            }
            ACTION_KIND_SPELL => {
                let spells = self.spells.as_deref()?;
                let d = spells.catalog.get(b.action)?;
                super::spell_action_icon(
                    b.action,
                    d,
                    spells,
                    self.me_view(),
                    &self.objects,
                    &self.items,
                    self.icons.as_deref(),
                )
            }
            _ => None,
        }
    }

    /// `0x4e6a50`'s attack tests on `spell` (`0x4e59b0`, `0x4e5a00`) and their faces, off the
    /// player as [`Self::face`] reads it: the melee face for Attack, the ranged weapon's for a
    /// ranged shot while it has one; `None` otherwise.
    fn attack_face(&self, spell: u32) -> Option<String> {
        let spells = self.spells.as_deref()?;
        let d = spells.catalog.get(spell)?;
        super::weapon_icon::auto_attack_icon(
            d,
            self.me_view(),
            &spells.forms,
            &self.objects,
            &self.items,
            self.icons.as_deref(),
        )
    }

    /// The VM's copy of word `b` holding `count`: its icon, its record's consumable test and
    /// on-use spell (`0x4e5ae0`) as the cache holds them, asking nothing.
    fn mirror_of(&self, b: &benilla_protocol::messages::ActionButton, count: u32) -> ActionSlot {
        let item = b.kind == ACTION_KIND_ITEM;
        let template = item
            .then(|| self.items.template(b.action, 0, Ask::Read))
            .flatten();
        ActionSlot {
            texture: self.face(b, false),
            kind: b.kind,
            action: b.action,
            count: if item { count } else { 0 },
            consumable: template.is_some_and(|t| t.is_consumable()),
            use_spell: template.and_then(|t| {
                t.spells
                    .iter()
                    .find(|sp| sp.spell_id != 0 && sp.trigger == 0)
                    .map(|sp| sp.spell_id)
            }),
        }
    }

    /// The (`Instant`, `GetTime`) pair cooldown starts are mapped through: the interface clock's
    /// ([`crate::ui_script::HostClock`]), the same at every pass between two ticks, so a running
    /// cooldown derives the same start; with none, `now` against the VM's clock.
    fn clock_pair_at(&self, now: Instant, ui_now: f64) -> (Instant, f64) {
        self.clock
            .as_deref()
            .map_or((now, ui_now), |c| (c.0.anchor, c.0.ui_now))
    }

    /// Every occupied slot's state, and with `reactive` the reactive spells that turned usable.
    fn all(
        &self,
        script: &benilla_ui::script::UiScript,
        memory: &ActionStateMemory,
        now: Instant,
        reactive: Option<&mut super::reactive::ReactiveList>,
    ) -> (HashMap<u32, ActionState>, Vec<String>) {
        self.slots_states(script, memory, None, now, reactive)
    }

    /// The states of the occupied slots, or of `only`, as the VM's macro records and the bar's
    /// counts stand (`0x4e5ba0`, `0xbc6390`).
    fn slots_states(
        &self,
        script: &benilla_ui::script::UiScript,
        memory: &ActionStateMemory,
        only: Option<u8>,
        now: Instant,
        reactive: Option<&mut super::reactive::ReactiveList>,
    ) -> (HashMap<u32, ActionState>, Vec<String>) {
        let bound = &crate::ui_macro::MacroBoundSpells::of(
            script,
            self.slots
                .buttons
                .values()
                .filter_map(|b| macro_slot_id(b.kind, b.action)),
        );
        let count = |action| memory.count(action);
        let walk = Walk {
            bound,
            counts: &count,
            only,
            now,
            clock: self.clock_pair_at(now, script.now()),
            getter: false,
        };
        self.slot_states(walk, reactive)
    }

    /// One slot's state as of here ([`ActionStates::state_now`]).
    fn state_now(&self, q: &ActionStateNow) -> Option<ActionState> {
        let slot = u8::try_from(q.action.checked_sub(1)?).ok()?;
        let button = self.slots.buttons.get(&slot)?;
        let mut bound = crate::ui_macro::MacroBoundSpells::default();
        if let (Some(id), Some((asked, Some(record)))) =
            (macro_slot_id(button.kind, button.action), q.bound)
        {
            if id == asked {
                bound.0.insert(id, record.into());
            }
        }
        let now = Instant::now();
        let walk = Walk {
            bound: &bound,
            counts: &|_| 0,
            only: Some(slot),
            now,
            clock: self.clock_pair_at(now, q.ui_now),
            getter: true,
        };
        self.slot_states(walk, None).0.remove(&q.action)
    }

    /// The states of the occupied slots `walk` names, and with `reactive` the reactive spells
    /// that turned usable.
    fn slot_states(
        &self,
        walk: Walk,
        reactive: Option<&mut super::reactive::ReactiveList>,
    ) -> (HashMap<u32, ActionState>, Vec<String>) {
        let Walk {
            bound,
            counts,
            only,
            now,
            clock: (anchor, ui_now),
            getter,
        } = walk;
        let Self {
            cooldowns,
            targeting,
            pet_bar,
            slots,
            spells,
            cast_state,
            objects,
            views,
            relations,
            items,
            commands,
            sub_classes,
            ..
        } = self;
        let (spell_mods, own_control, skill_lines) = cast_state;
        let skill_catalog = skill_lines.as_deref().map(|s| &s.catalog);
        let sub_classes = sub_classes.as_deref().map(|s| &s.0);
        // The player and the target as the usable walk reads them, as of the drain's place: a
        // field watcher's refresh walks its own packet's. The rest of a slot's state, which no
        // refresh stores, is derived off the live player, the frame's.
        let me_entity = objects.player();
        let me = me_entity.and_then(|e| views.get(e).ok());
        let me_live = me_entity.and_then(|e| objects.store_of(e));
        let control_lost = own_control.as_ref().is_some_and(|c| c.0);
        // The bags, walked once per pass for every reagent and totem count below.
        // Its items as of there too, a reagent a later packet destroys still held.
        let stacks = me
            .map(|s| stack_counts(&s.0, &views.item_objects(objects)))
            .unwrap_or_default();
        let selection = &targeting.selection;
        let target = selection
            .guid
            .and_then(|g| objects.entity(g))
            .filter(|&e| Some(e) != me_entity)
            .and_then(|e| views.get(e).ok());
        let cast_ctx = targeting.context();
        let range_leg = RangeLeg {
            // The context's caster carries the ranged-slot item the scale reads.
            caster: cast_ctx.range.caster,
            mods: spell_mods,
            cast_ctx: &cast_ctx,
            player: me.is_some(),
        };
        // The pet gate `0x4bcf70`, asked once for every slot bound to a pet spell: the bar those
        // slots read the history of, and the pet's object when the gate holds.
        let pet_leg = pet_bar.as_deref().map(|bar| {
            let guid = bar.spells.pet_guid;
            let pet = views.by_guid(guid, objects.store(guid));
            let held = crate::ui_pet::pet_actions_usable(bar, cast_ctx.self_guid, me, pet);
            (bar, pet.filter(|_| held))
        });

        let mut fresh: HashMap<u32, ActionState> = HashMap::new();
        for (&slot, button) in slots
            .buttons
            .iter()
            .filter(|(&slot, _)| only.is_none_or(|o| o == slot))
        {
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
                            player: me,
                            pet,
                            skills: crate::spell::PlayerSkills {
                                lines: skill_catalog,
                                spells: spells.as_deref().map(|s| &s.catalog),
                                items: Some(items),
                            },
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
                    let race_class = me_live.map_or((0, 0), |store| store.race_class());
                    st.profession =
                        profession(button.action, d, skill_lines.as_deref(), race_class);
                    // The usable walk (`0x6e3d60`); only its power gate sets `notEnoughMana`.
                    if let (Some(store), Some(sp)) = (me, spells.as_deref()) {
                        let ctx = usable::UsableCtx {
                            store,
                            target: selection.guid.zip(target),
                            rel: relations.get(),
                            cooldowns,
                            stacks: &stacks,
                            spell_mods,
                            control_lost,
                            skill_lines: skill_catalog,
                            sub_classes,
                        };
                        let (u, oom) =
                            usable::spell_usable(button.action, d, sp, &ctx, objects, items);
                        st.usable = u;
                        st.not_enough_mana = oom;
                    } else {
                        st.usable = true;
                    }
                    let range = range_answers(d, spells.as_deref(), &range_leg);
                    (st.has_range, st.in_range) = range;
                    let info = cooldowns.info(button.action, 0, Some(d), now);
                    st.cooldown = info.ui_triple(anchor, ui_now);
                }
                ACTION_KIND_ITEM => {
                    // The count the slot holds (`0xbc6390`), which `GetActionCount` answers too:
                    // the gate reads the cache the recount wrote, never the bags (`0x4e50ab`).
                    let count = counts(action);
                    // The usable test asks nothing with no player (`0x4e5080`) or a stored count
                    // of 0 (`0x4e50ab`). Past them it looks the record up twice, each with its
                    // callback and the dedupe flag, so a miss, a refused record asked again among
                    // them, answers through both in that order: `0x6e2fc0` with `0x6e2f70`
                    // (`0x4e50d0`, `0x6e2fe9`), then `0x4e5a50` → `0x4e5ae0` with the bar's
                    // `0x4e5b80` (`0x4e5b30`), which recounts the entry. A getter's read asks as
                    // its own legs do, whatever the count: `GetActionCooldown`'s query with
                    // `0x6e2f70` (`0x6e2efb`), the range tests' `0x4e5a50` → `0x4e5ae0`.
                    let template = if getter || (me.is_some() && count > 0) {
                        Cooldowns::query_record(items, commands, button.action, 0, true);
                        self.bar_record(button.action)
                    } else {
                        items.template(button.action, 0, Ask::Read)
                    };
                    // Only the `Copy` on-use spell, not a clone of the template per slot.
                    let use_spell = template.and_then(|t| t.use_spell);
                    // `IsCurrentAction`'s item arm holds the entry against the in-flight item
                    // (`0x4e53fc`-`0x4e5428`).
                    st.item = Some(button.action);
                    // `GetItemCooldown 0x6e2ed0` queries only an item whose record is cached and
                    // has an on-use spell (`0x6e2f14`, `0x6e2f20`-`0x6e2f34`).
                    st.no_cooldown_query = use_spell.is_none();
                    // `IsEquippedAction`, the green border: `0x622270` at mask 1
                    // (`0x4e5377`-`0x4e5380`), the walker over the worn slots, so a broken copy is
                    // not worn.
                    st.equipped = me_live.is_some_and(|s| wears_entry(s, objects, button.action));
                    // `0x4e5050`'s item arm runs the count, `IsItemOnCooldown` and the on-use spell
                    // through the same `0x6e3d60` walk. With no player the reference answers (0, 0)
                    // (`0x4e5080`), the default state.
                    if let Some(store) = me {
                        let ctx = usable::UsableCtx {
                            store,
                            target: selection.guid.zip(target),
                            rel: relations.get(),
                            cooldowns,
                            stacks: &stacks,
                            spell_mods,
                            control_lost,
                            skill_lines: skill_catalog,
                            sub_classes,
                        };
                        let (u, oom) = usable::item_usable(
                            button.action,
                            use_spell.as_ref(),
                            count > 0,
                            &ctx,
                            spells.as_deref(),
                            objects,
                            items,
                        );
                        st.usable = u;
                        st.not_enough_mana = oom;
                    }
                    if let Some(u) = use_spell {
                        let d = spells.as_ref().and_then(|s| s.catalog.get(u.spell_id));
                        // Both range queries resolve the slot to this spell with book 0
                        // (`0x4e5a50` into `0x4e5ae0`) and test it as a spell slot's; a missing
                        // record answers none (`0x4e5846`, `0x4e5752`).
                        if let Some(d) = d {
                            (st.has_range, st.in_range) =
                                range_answers(d, spells.as_deref(), &range_leg);
                        }
                        let info =
                            cooldowns.info_item(u.spell_id, button.action, u.category, d, now);
                        st.cooldown = info.ui_triple(anchor, ui_now);
                    }
                }
                _ => {}
            }
            // The triple holds the absolute start, so a running cooldown diffs equal every frame
            // and a re-arm, even within one frame gap, restarts the sweep.
            fresh.insert(action, st);
        }

        let activated = match (me, spells.as_deref(), reactive) {
            (Some(store), Some(sp), Some(reactive)) => {
                let ctx = usable::UsableCtx {
                    store,
                    target: selection.guid.zip(target),
                    rel: relations.get(),
                    cooldowns,
                    stacks: &stacks,
                    spell_mods,
                    control_lost,
                    skill_lines: skill_catalog,
                    sub_classes,
                };
                reactive.refresh(|id| {
                    sp.catalog
                        .get(id)
                        .is_some_and(|d| usable::spell_usable(id, d, sp, &ctx, objects, items).0)
                })
            }
            _ => Vec::new(),
        };

        (fresh, activated)
    }
}

/// What one walk of the slots reads beside the states: the VM's macro records and item counts,
/// the slot it is limited to, and its clock.
struct Walk<'a> {
    bound: &'a crate::ui_macro::MacroBoundSpells,
    counts: &'a dyn Fn(u32) -> u32,
    only: Option<u8>,
    now: Instant,
    clock: (Instant, f64),
    /// A getter's read at its call ([`ActionStates::state_now`]), whose lookups are its own legs'.
    getter: bool,
}

impl crate::script_calls::live::LiveFamily for ActionStates<'_, '_> {
    fn answer(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
        ask.answer(|q: &ActionStateNow| Some(self.state_now(q)))
            || ask.answer(|q: &ActionUsableNow| Some(self.usable_now(q.0)))
            || ask.answer(|q: &ActionTextureNow| Some(self.texture_now(q.0)))
            || ask.answer(|q: &ActionAttackFaceNow| Some(self.attack_face_now(q.0)))
            || ask.answer(|q: &ActionCountNow| Some(self.count_now(q.0)))
            || ask.answer(|q: &ActionConsumableNow| Some(self.consumable_now(q.0)))
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "range_tests.rs"]
mod range_tests;
