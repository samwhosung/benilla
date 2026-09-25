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
mod drain;
#[cfg(test)]
mod drain_tests;
pub(crate) mod drop_item;
mod errors;
mod feed;
#[cfg(test)]
mod feed_tests;
mod net;
mod ranks;
mod state;
pub(crate) mod toggle;
mod weapon_icon;

/// Every feed that pushes cooldown triples runs `.before` this set: [`state::feed_action_state`]
/// fires the cooldown events synchronously, so a triple pushed later goes unread until the next
/// cooldown change.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CooldownEvents;

pub(crate) use errors::{
    attack_actor_blocked, attack_actor_refusal, keyed_line, keyed_line_s, reagent_totem_refusal,
    show_messages, ui_error_text, CastErrors, CastFail, Caster, FillArg, MessageSink, MountErrors,
    PetTameFailures, Shown, UiError, UiErrorKeys, UiErrorTexts,
};
pub(crate) use weapon_icon::{melee_auto_attack_icon, ranged_weapon_icon};

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
    /// The spell book (`SMSG_INITIAL_SPELLS`), ordered on purpose: the reference's first-hit
    /// scans (the lock resolver `0x5f83d0`) pick between equal spells, 6478 and 22810 "Opening",
    /// by array order, which is ascending id: the server sends the book from a `std::map`.
    pub spells: BTreeSet<u32>,
    /// Set by every book or bar change, cleared once the feed re-resolves slot identity, which a
    /// landed item template ([`crate::items::Items::template_epoch`]) also triggers. Item counts
    /// refresh every frame regardless.
    pub dirty: bool,
}

/// The world right-click's GameObject openers, queued one frame for the cast ladder: the click
/// system cannot also hold the ladder's resources, and only the ladder sends a cast, so a chest
/// right-clicked mid-cast takes the reference's silent same-spell bail (`0x6e4d43`).
#[derive(Resource, Default)]
pub(crate) struct GoOpenerCasts(pub(crate) Vec<GoOpener>);

/// A right-click's resolved opener ([`crate::target::click::resolve_go_action`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum GoOpener {
    /// A known `OPEN_LOCK` spell, cast with a GameObject target block.
    Spell { spell_id: u32, go_guid: u64 },
    /// A carried key: `CMSG_USE_ITEM` with `TARGET_FLAG_GAMEOBJECT`, the object's guid in
    /// [`crate::ui_items::ItemUse::on_object`].
    Key(crate::ui_items::ItemUse),
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
    /// The cast time in ms (`GetCastTime 0x6e3340`), level-scaled from the `CastingTimeIndex`
    /// row; a missing row reads 0. Spell-mod op `0xa` is not applied, so a talent-shortened cast
    /// shows its base length.
    pub(crate) fn cast_time_ms(
        &self,
        def: &benilla_formats::SpellDisplay,
        caster_level: u32,
    ) -> u32 {
        self.cast_times
            .get(def.casting_time_index)
            .map_or(0, |row| row.resolved_ms(caster_level, def.base_level))
    }
}

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
fn track_learned_abilities(
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
        app.init_resource::<PlayerActions>()
            .init_resource::<LearnedAbilities>()
            .init_resource::<CastErrors>()
            .init_resource::<MountErrors>()
            .init_resource::<PetTameFailures>()
            .init_resource::<UiErrorKeys>()
            .init_resource::<UiErrorTexts>()
            .init_resource::<GoOpenerCasts>()
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
                    feed::feed_actions.in_set(UnitFeed),
                    state::feed_action_state
                        .in_set(UnitFeed)
                        .in_set(CooldownEvents)
                        .after(feed::feed_actions),
                    drain::drain_action_sets.after(UiInput),
                    drain::drain_action_uses.after(UiInput),
                    // The ATTACKTARGET binding, after the dispatch wrote this frame's key fires.
                    drain::attack_target_binding.after(UiInput),
                    // The target chain queues the openers earlier in the frame.
                    drain::drain_go_openers.after(UiInput),
                    // The latches must be current before the target chain's cursor reads them.
                    track_learned_abilities
                        .in_set(UnitFeed)
                        .after(feed::feed_actions),
                ),
            );
    }
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
