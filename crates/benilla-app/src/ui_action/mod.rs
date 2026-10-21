//! The app side of the action bar: the action table, the spell catalogs and the plugin. [`feed`]
//! pushes each slot's identity into the VM and [`state`] its per-frame cooldown, usability, range
//! and checked state; [`drain`] sends `UseAction` as a cast, swing, item use or macro, and
//! `PickupAction`/`PlaceAction` as `CMSG_SET_ACTION_BUTTON`.

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::*;

use benilla_formats::SpellCatalog;
use benilla_protocol::messages::ActionButton;

use crate::ui_script::UiInput;
use crate::ui_unit::UnitFeed;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

mod cast_fail;
mod cast_lines;
pub(crate) use cast_lines::{CastFailLines, CastFailRedisplays};
mod drain;
// A `UseAction` press, applied in call order by `crate::script_calls`.
pub(crate) use drain::{attack_nearest_probe, use_action, ActionPress, AttackPress, UseOutcome};
// The bar's drain of the VM's queued writes, which the macro migration's test runs end to end.
#[cfg(test)]
pub(crate) use drain::drain_action_sets;
// The right-click casts' drain, which the click's tests run after the click.
#[cfg(test)]
pub(crate) use drain::drain_click_casts;
#[cfg(test)]
mod drain_tests;
pub(crate) mod drop_item;
mod errors;
mod feed;
pub(crate) use feed::{spell_action_icon, MISSING_ITEM_ICON};
#[cfg(test)]
mod feed_tests;
mod net;
mod ranks;
// The rank pass, which the macro load orders itself ahead of: it rewrites a bar word, too.
pub(crate) use ranks::normalize_action_ranks;
mod reactive;
#[cfg(test)]
mod reactive_tests;
mod recount;
pub(crate) use recount::recount_item_actions;
mod state;
pub(crate) mod toggle;
mod weapon_icon;

/// Every feed that pushes cooldown triples runs `.before` this set: [`state::fire_action_events`]
/// fires the cooldown events synchronously, so a triple pushed later goes unread until the next
/// cooldown change. The pet list's flush, `ui_pet::bar::fire_pet_cooldown_events`, runs `.after`
/// it on the same rule.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CooldownEvents;

pub(crate) use cast_fail::report_reaches_pop;
#[cfg(test)]
pub(crate) use cast_fail::CAST_FAIL_KEYS;
pub(crate) use errors::{
    attack_actor_refusal, keyed_line, keyed_line_s, reagent_totem_refusal, show_messages,
    ui_error_text, CastFail, Caster, FillArg, MessageSink, MountErrors, PetTameFailures, Shown,
    UiError, UiErrorKeys, UiErrorTexts,
};
pub(crate) use weapon_icon::{melee_auto_attack_icon, ranged_weapon_icon, substitutes_weapon_icon};

/// `Attack`, the auto-attack pseudo-spell: it toggles melee rather than casting. The use path
/// keys on this id; the icon keys on `SPELL_EFFECT_ATTACK`, which only 6603 carries.
pub(crate) const SPELL_ATTACK: u32 = 6603;

/// The action slots (wire 0..119) and the known spells. The bar is client-authoritative:
/// [`drain::drain_action_sets`] writes it directly, and vmangos stores a slot without a reply
/// (`MasterPlayer::addActionButton`).
#[derive(Resource, Default)]
pub(crate) struct PlayerActions {
    /// Wire slot (0-based) to its packed action; the Lua action id is slot + 1.
    pub buttons: HashMap<u8, ActionButton>,
    /// The known spells (`SMSG_INITIAL_SPELLS` and the learns since), the reference's known-spell
    /// bits `0xb710fc` (`0x4b2603`): a set, which keeps no learn order.
    pub spells: BTreeSet<u32>,
    /// `[0xb700b0]`: the known spells whose `Effect[0]` is `SPELL_EFFECT_OPEN_LOCK`, in learn
    /// order. The login list enters in wire order (`0x5dea8b`-`0x5deab9`), each learn appends at
    /// the tail with no duplicate check (`0x4b2665`-`0x4b2689`), and an unlearn removes the last
    /// copy and closes the gap (`0x4b2d60`-`0x4b2d87`). The lock resolver `0x5f83d0` and the
    /// item tooltip's lock walk `0x5da5e0` scan it from the front, so its order picks between
    /// 6478 "Opening" and 22810 "Opening - No Text", which open the same locks.
    pub openers: Vec<u32>,
    /// Set by every book or bar change, cleared once the feed re-resolves slot identity, which a
    /// landed item template ([`crate::items::Items::template_epoch`]) also triggers. Between
    /// resolves an item slot's count moves only by the item watchers' recount (`recount`).
    pub dirty: bool,
}

/// The world right-click's casts, queued for the cast ladder later in the frame: the click system
/// cannot also hold the ladder's resources, and only the ladder sends a cast, so a chest or a body
/// right-clicked mid-cast meets the attempt's rungs as a button press does (`0x6e4d43`, `0x6e4d97`).
#[derive(Resource, Default)]
pub(crate) struct ClickCasts(pub(crate) Vec<ClickCast>);

/// A right-click's cast: a GameObject's opener ([`crate::target::click::resolve_go_action`]) or
/// the spell a body is interacted with through (`0x5f05e0`).
#[derive(Clone, Copy, Debug)]
pub(crate) enum ClickCast {
    /// A known `OPEN_LOCK` spell, cast with a GameObject target block.
    AtObject { spell_id: u32, go_guid: u64 },
    /// A carried key: `CMSG_USE_ITEM` with `TARGET_FLAG_GAMEOBJECT`, the object's guid in
    /// [`crate::ui_items::ItemUse::on_object`].
    Key(crate::ui_items::ItemUse),
    /// `InteractWithSpell 0x5f05e0`'s cast (`0x5f06dc` → `0x6e5a90`): the attempt with the body's
    /// guid passed in, which its range test and its arm take in the selection's place.
    AtUnit {
        spell_id: u32,
        entity: Entity,
        guid: u64,
        /// The byte `InteractWithSpell` passes `SetAutoLoot` after the cast, its `set` given
        /// (`0x5f06e1`-`0x5f06ef`).
        auto_loot: Option<bool>,
    },
}

/// The spell DBC catalogs, absent without client data; the cast-visual router shares the catalog.
#[derive(Resource)]
pub(crate) struct Spells {
    pub(crate) catalog: SpellCatalog,
    /// Form id to its `SpellShapeshiftForm.dbc` row: `BonusActionBar` pages the bar
    /// (`GetBonusBarOffset 0x4e7620`), `flags1` holds the stance and cancel bits.
    pub(crate) forms: std::collections::HashMap<u32, benilla_formats::ShapeshiftForm>,
    /// `SpellRange.dbc`, the `GetMinMaxRange 0x6e3480` inputs; empty on a failed load.
    pub(crate) ranges: benilla_formats::SpellRangeCatalog,
    /// `SpellCastTimes.dbc`, read through `CastingTimeIndex` (`GetCastTime 0x6e3340`).
    pub(crate) cast_times: benilla_formats::SpellCastTimeCatalog,
    /// `SpellDuration.dbc`: the `$d`/`$o` tokens' source (`GetDuration 0x6ea000`).
    pub(crate) durations: benilla_formats::SpellDurationCatalog,
    /// `SpellRadius.dbc`: the `$a` token's yards.
    pub(crate) radii: benilla_formats::SpellRadiusCatalog,
}

impl Spells {
    /// The cast time in ms as `GetCastTime 0x6e3340` returns it for argument 1, the tooltip's
    /// (`52eb4b`): the `CastingTimeIndex` row scaled at `level` (`0x6e3130`, the player's skill
    /// in the spell's line over 5), then spell-mod op 10 (`0x6e33b7`), then
    /// the caster's cast speed, with no clamp, so row 18's negative time survives. A missing row
    /// returns 0 before the modifier (`0x6e338a`). `caster` is the unit the speed is read off, the
    /// player or, for a pet spell, its charm else its summon (`0x6e33e8`-`0x6e3416`), `None` when
    /// it is not streamed, which skips the multiply (`0x6e341d`). `reads` hears the level when the
    /// row scales by it and the speed when it is read.
    pub(crate) fn cast_time_unclamped_ms(
        &self,
        def: &benilla_formats::SpellDisplay,
        caster: Option<&benilla_protocol::ObjectFields>,
        level: crate::spell::SpellLevel,
        mods: &crate::spell::SpellModifiers,
        reads: &mut dyn crate::spell::SpellReads,
    ) -> i32 {
        let Some(row) = self.cast_times.get(def.casting_time_index) else {
            return 0;
        };
        // The row's level, `0x6e3130`, read only by a row that scales by it.
        let caster_level = if row.reads_caster_level() {
            level.read(def, reads)
        } else {
            0
        };
        let resolved = row.resolved_ms(caster_level, def.base_level);
        let time = mods.apply(def, crate::spell::OP_CAST_TIME, resolved);
        // An ability or a trade skill keeps its time (`Attributes & 0x30`, `0x6e33c3`), and so does
        // a time at or below zero (`0x6e3424`).
        let Some(caster) = caster.filter(|_| def.attributes & ATTR_NO_CAST_SPEED == 0 && time > 0)
        else {
            return time;
        };
        reads.unit(crate::spell::UnitField::CastSpeed);
        // A field never sent is the descriptor's 0, which casts in no time; vmangos sends 1.0 for
        // that reason (`Player.cpp:464-466`).
        let speed = caster.unit_mod_cast_speed().unwrap_or(0.0);
        // `|speed - 1|` below the epsilon at `0x8029d4` keeps the time; equal to it, or a NaN, an
        // unordered compare, multiplies (`0x6e3432`-`0x6e3445`: `test ah, 5` then `jnp`).
        if (speed - 1.0).abs() < CAST_SPEED_EPSILON {
            return time;
        }
        // `fimul` on the x87 stack, stored as an `f32`, then `fistp`, which rounds to nearest
        // even and stores the integer indefinite for a NaN or a value out of range
        // (`0x6e3447`-`0x6e3456`). The product is exact in `f64` for any `f32` times an `i32`
        // below 2^29 ms.
        let product = f64::from((f64::from(speed) * f64::from(time)) as f32).round_ties_even();
        if (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&product) {
            product as i32
        } else {
            i32::MIN
        }
    }

    /// [`Self::cast_time_unclamped_ms`] for argument 0, the cast validator's (`0x609e16`): a
    /// result at or below zero reads 0 (`0x6e3461`-`0x6e3470`).
    pub(crate) fn cast_time_ms(
        &self,
        def: &benilla_formats::SpellDisplay,
        caster: Option<&benilla_protocol::ObjectFields>,
        level: crate::spell::SpellLevel,
        mods: &crate::spell::SpellModifiers,
    ) -> u32 {
        let time =
            self.cast_time_unclamped_ms(def, caster, level, mods, &mut crate::spell::Unrecorded);
        time.max(0) as u32
    }
}

/// `Attributes` bits 4 and 5, vmangos `SPELL_ATTR_IS_ABILITY` and `SPELL_ATTR_IS_TRADESKILL`
/// (`SpellDefines.h:796-797`): a cast time the caster's speed does not scale (`0x6e33c3`).
const ATTR_NO_CAST_SPEED: u32 = 0x30;

/// The `f32` at `0x8029d4`, 2^-22: a cast speed this close to 1 leaves the time as it is.
const CAST_SPEED_EPSILON: f32 = f32::from_bits(0x3480_0000);

#[cfg(test)]
impl Spells {
    pub(crate) fn empty_for_tests() -> Self {
        Spells {
            catalog: SpellCatalog::from_displays(HashMap::new()),
            forms: HashMap::new(),
            ranges: benilla_formats::SpellRangeCatalog::default(),
            cast_times: Default::default(),
            durations: Default::default(),
            radii: Default::default(),
        }
    }
}

/// One pass of the action-state feed, for a test that reads a slot's pushed state.
#[cfg(test)]
pub(crate) fn run_feed_action_state_once(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(state::feed_action_state)
        .expect("the action-state feed runs as a one-shot system");
}

/// One pass of the action-state feed and the events it owes, for a test that hears them.
#[cfg(test)]
pub(crate) fn run_action_events_once(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    run_feed_action_state_once(world);
    world
        .run_system_once(state::fire_action_events)
        .expect("the action events run as a one-shot system");
}

/// The 5875 spell data; `None` skips where the install is absent.
#[cfg(test)]
pub(crate) fn real_spells() -> Option<Spells> {
    let data = benilla_formats::wow_data_or_skip!(None);
    let mut chain = benilla_formats::open_chain(&data).expect("open chain");
    Some(Spells {
        catalog: benilla_formats::load_spell_catalog(&mut chain).expect("Spell.dbc"),
        forms: benilla_formats::load_shapeshift_forms(&mut chain).expect("forms"),
        ranges: benilla_formats::load_spell_ranges(&mut chain).expect("ranges"),
        cast_times: benilla_formats::load_spell_cast_times(&mut chain).expect("cast times"),
        durations: benilla_formats::load_spell_durations(&mut chain).expect("durations"),
        radii: benilla_formats::load_spell_radii(&mut chain).expect("radii"),
    })
}

/// The reference's learned-ability latches: at learn time `0x4b25e0` stores a spell's id by its
/// `Effect[0]`, and the unlearn path `0x4b2c50` clears it. The skin cursor requires the latch
/// (`0x482589`), so a player who never learned Skinning gets no skin cursor.
#[derive(Resource, Default)]
pub(crate) struct LearnedAbilities {
    /// `[0xb700e4]`: the known `SPELL_EFFECT_SKINNING` spell, which the skin click also casts.
    pub(crate) skinning: Option<u32>,
    /// `[0xb700e8]`: the known `SPELL_EFFECT_SKIN_PLAYER_CORPSE` spell; no cursor reads it yet.
    pub(crate) skin_player_corpse: Option<u32>,
    /// `[0xcecad8]`: the known `SPELL_EFFECT_FEED_PET` spell, latched by `0x6ea1d0` (from
    /// `0x5e9e49`); gate 3 of [`drop_item::drop_item_on_unit`] and the spell it casts.
    pub(crate) feed_pet: Option<u32>,
}

/// The "Remove Insignia" effect, the second one `0x4b25e0` latches.
const SPELL_EFFECT_SKIN_PLAYER_CORPSE: u32 = 0x74;

/// The effect latched into `[0xcecad8]` at learn time (`0x5e9e42`); only Feed Pet 6991 has it.
const SPELL_EFFECT_FEED_PET: u32 = 0x65;

/// Re-derives [`LearnedAbilities`] whenever the book changes, standing in for learn-time writes.
pub(crate) fn track_learned_abilities(
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    mut learned: ResMut<LearnedAbilities>,
) {
    let Some(spells) = spells else { return };
    if !actions.is_changed() && !spells.is_changed() {
        return;
    }
    // The last match in id order: each learned rank overwrites the reference's latch, and a rank
    // chain ascends by id (Skinning 8613, 8617, 8618, 10768).
    let last_with = |effect: u32| {
        actions.spells.iter().copied().rfind(|&id| {
            spells
                .catalog
                .get(id)
                .is_some_and(|d| d.effects[0] == effect)
        })
    };
    let (skinning, skin_player_corpse, feed_pet) = (
        last_with(benilla_formats::SPELL_EFFECT_SKINNING),
        last_with(SPELL_EFFECT_SKIN_PLAYER_CORPSE),
        last_with(SPELL_EFFECT_FEED_PET),
    );
    if (skinning, skin_player_corpse, feed_pet)
        != (
            learned.skinning,
            learned.skin_player_corpse,
            learned.feed_pet,
        )
    {
        *learned = LearnedAbilities {
            skinning,
            skin_player_corpse,
            feed_pet,
        };
    }
}

/// `SpellMechanic.dbc`: the name in `SPELL_FAILED_PREVENTED_BY_MECHANIC`'s `%s` (reason `0x8d`).
#[derive(Resource)]
pub(crate) struct SpellMechanics {
    pub(crate) catalog: benilla_formats::SpellMechanicCatalog,
}

fn load_spell_mechanics(mut commands: Commands, assets: Option<Res<benilla_assets::WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = benilla_assets::LockRecover::lock_recover(&*assets.chain);
        benilla_formats::load_spell_mechanic_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            debug!("ui_action: {} spell-mechanic name(s)", catalog.len());
            commands.insert_resource(SpellMechanics { catalog });
        }
        Err(e) => warn!(
            "ui_action: SpellMechanic.dbc failed to load — the crowd-control refusal drops the \
             mechanic name: {e:#}"
        ),
    }
}

pub(crate) struct UiActionPlugin;

impl Plugin for UiActionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        // A UI-only harness has no net stack; declaring a message twice is idempotent.
        app.add_message::<crate::item_watch::ItemNotice>()
            .add_message::<crate::net::FieldChanged>()
            .init_resource::<state::ActionEvents>()
            .add_message::<crate::net::WorldEnterCascadeMessage>()
            .init_resource::<PlayerActions>()
            .init_resource::<LearnedAbilities>()
            .init_resource::<CastFailRedisplays>()
            .init_resource::<MountErrors>()
            .init_resource::<PetTameFailures>()
            .init_resource::<UiErrorKeys>()
            .init_resource::<UiErrorTexts>()
            .init_resource::<ClickCasts>()
            .add_systems(Startup, load_spells.after(AssetSet::Open))
            .add_systems(Startup, load_spell_mechanics.after(AssetSet::Open))
            .add_systems(
                Update,
                (
                    // The feeds ride with the unit feed, before the VM ticks: the rank pass before
                    // the identity feed that consumes its `dirty`, the state feed after it, so a
                    // slot's rank fix and first state land the same frame. The drains follow the
                    // input pass, in any order: a gesture fills only one of their queues.
                    ranks::normalize_action_ranks
                        .in_set(UnitFeed)
                        .before(feed::feed_actions),
                    // The item watchers' recounts, after the server's unlocks, which each handler
                    // runs first (`0x5d94af` before `0x5d94e1`).
                    recount::recount_item_actions
                        .in_set(UnitFeed)
                        .after(crate::ui_items::locks::apply_item_releases)
                        .before(feed::feed_actions),
                    // Before the aura feed, whose `raidFilter` verdicts rebuild off the book
                    // when `PlayerActions` moves, so a book this pass settles is read this frame.
                    feed::feed_actions
                        .in_set(UnitFeed)
                        .before(crate::ui_aura::AuraEvents),
                    // A seat: the world-enter events' handlers read the slots' state, which the
                    // reference's arrival stores before its enter cascade (`0x5deafc`, `0x5deb60`).
                    // After the bag feed, so each `BAG_UPDATE`'s cache refresh (`0x4f9374`) lands
                    // in its own frame. After the pet's old-target clear, which writes the bar a
                    // macro bound to a pet spell reads the guid and history of.
                    state::feed_action_state
                        .in_set(UnitFeed)
                        .in_set(crate::ui_unit::WorldEnterSeats)
                        .after(crate::player::ControlRestore)
                        .after(crate::ui_pet::pet_stop_on_old_target_clear)
                        .after(crate::ui_items::feed::feed_containers)
                        .after(feed::feed_actions),
                    // After the unit snapshots, which its handlers read (the target stored at
                    // `0x4936e2`, a watched field written before its watch runs), and ahead of the
                    // events its producers precede: the select's `PLAYER_TARGET_CHANGED` and the
                    // combo points' event (the unit events), the watched fields' own events (the
                    // field notices) and the form's cast change (the cast reports).
                    state::fire_action_events
                        .in_set(UnitFeed)
                        .in_set(CooldownEvents)
                        .after(state::feed_action_state)
                        .after(crate::ui_unit::UnitSnapshots)
                        .before(crate::ui_unit::field_events::fire_field_notices)
                        .before(crate::ui_cast::feed_cast_reports),
                    drain::drain_action_sets.in_set(crate::ui_script::UiDrain),
                    // The target chain queues the right-click casts earlier in the frame.
                    drain::drain_click_casts
                        .after(UiInput)
                        .after(crate::target::TargetUpdate),
                    // The latches must be current before the target chain's cursor reads them.
                    track_learned_abilities
                        .in_set(UnitFeed)
                        .after(feed::feed_actions),
                ),
            )
            .add_systems(
                OnExit(crate::client_state::ClientState::InWorld),
                drop_held_lines,
            );
    }
}

/// `OnExit(InWorld)`: a line still held under the cover dies with the world's interface, as the
/// reference's, drawn at once by `0x4945b0`, dies in the UI teardown (`0x490bd0`).
fn drop_held_lines(mut texts: ResMut<UiErrorTexts>) {
    texts.0.clear();
}

fn load_spells(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_spell_catalog(&mut chain)
    };
    match loaded {
        Ok(catalog) => {
            let forms = {
                let mut chain = assets.chain.lock_recover();
                benilla_formats::load_shapeshift_forms(&mut chain).unwrap_or_else(|e| {
                    warn!("ui_action: SpellShapeshiftForm.dbc failed — stance paging off: {e:#}");
                    Default::default()
                })
            };
            let ranges = {
                let mut chain = assets.chain.lock_recover();
                benilla_formats::load_spell_ranges(&mut chain).unwrap_or_else(|e| {
                    warn!("ui_action: SpellRange.dbc failed — range indicator off: {e:#}");
                    benilla_formats::SpellRangeCatalog::default()
                })
            };
            let cast_times = {
                let mut chain = assets.chain.lock_recover();
                benilla_formats::load_spell_cast_times(&mut chain).unwrap_or_else(|e| {
                    warn!("ui_action: SpellCastTimes.dbc failed — cast-time cell off: {e:#}");
                    Default::default()
                })
            };
            let durations = {
                let mut chain = assets.chain.lock_recover();
                benilla_formats::load_spell_durations(&mut chain).unwrap_or_else(|e| {
                    warn!("ui_action: SpellDuration.dbc failed — $d/$o tokens off: {e:#}");
                    Default::default()
                })
            };
            let radii = {
                let mut chain = assets.chain.lock_recover();
                benilla_formats::load_spell_radii(&mut chain).unwrap_or_else(|e| {
                    warn!("ui_action: SpellRadius.dbc failed — $a token off: {e:#}");
                    Default::default()
                })
            };
            info!(
                "ui_action: {} spells in the display catalog, {} shapeshift forms, {} range rows, \
                 {} cast times, {} durations, {} radii",
                catalog.len(),
                forms.len(),
                ranges.len(),
                cast_times.len(),
                durations.len(),
                radii.len()
            );
            commands.insert_resource(Spells {
                catalog,
                forms,
                ranges,
                cast_times,
                durations,
                radii,
            });
        }
        Err(e) => warn!("ui_action: Spell.dbc failed to load — bar icons disabled: {e:#}"),
    }
}
