//! The app-side **action seam** (decision 0068 slice 1, extended by decision 0216 §7/slice 4) —
//! the three directions around the engine-free bindings ([`benilla_ui::script`]'s `action`/
//! `cursor::bar` modules). This module owns the seam's *shared state* — the action table, the
//! spell catalogs, the plugin wiring — and each direction lives in its own file:
//!
//! - **Inward — identity** ([`feed`]): the net bridge fills [`PlayerActions`] from
//!   `SMSG_INITIAL_SPELLS` + `SMSG_ACTION_BUTTONS` (and [`drain::drain_action_sets`] writes it
//!   directly, client-side — the bar is client-authoritative, decision 0218 §4); the feed resolves
//!   each occupied slot's icon and count, pushes the 120-slot snapshot into the VM, and fires
//!   `ACTIONBAR_SLOT_CHANGED` per changed slot. What is gated on what is the design there.
//! - **Inward — dynamic state** ([`state`]): cooldown swirl, usability tint, range colour,
//!   checked/flash — the per-frame half, fed after the identity half so a fresh slot's first state
//!   push lands the same frame.
//! - **Outward — use** ([`drain::drain_action_uses`]): a queued `UseAction(n)` becomes wire — a
//!   spell through the one cast-send path ([`crate::spell::CastLadder`]), the auto-attack through
//!   `CMSG_ATTACKSWING`, an item through the two-stage equip-vs-use law ([`drain::item_action_route`],
//!   decision 0666).
//! - **Outward — set** ([`drain::drain_action_sets`]): the cursor seam's `PickupAction`/
//!   `PlaceAction` mutations become `CMSG_SET_ACTION_BUTTON` sends, one per queued entry.
//!
//! The supporting law sits alongside: [`cast_fail`] + [`errors`] (the red error line's two
//! layers) and [`weapon_icon`] (the auto-attack's borrowed weapon icon). The cast ladder itself —
//! the target bind, the validator's rungs, the usable walk, the targeting cursor — is the spell's
//! ([`crate::spell`], decision 2330); this module is one of its callers.

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

/// The cooldown-event cut: [`state::feed_action_state`] fires the store-change flush trio
/// (`ACTIONBAR_UPDATE_COOLDOWN`/`SPELL_UPDATE_COOLDOWN`/`BAG_UPDATE_COOLDOWN`) **synchronously**
/// (`UiScript::fire_event` walks the handlers inline), so every feed that pushes cooldown
/// triples the handlers re-read (the container feed's slot cooldowns, the spellbook feed's, the
/// stance feed's — decision 2009) must run `.before(CooldownEvents)` — or a handler reads last
/// frame's triples and the pie stays missing until the next store change. The action states
/// themselves are safe by construction (pushed by the same system, before it fires).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CooldownEvents;

pub(crate) use errors::{
    attack_actor_blocked, attack_actor_refusal, keyed_line, keyed_line_s, reagent_totem_refusal,
    show_messages, ui_error_text, CastErrors, CastFail, Caster, FillArg, MessageSink, MountErrors,
    PetTameFailures, Shown, UiError, UiErrorKeys, UiErrorTexts,
};
// `pub(crate)`: the spellbook shows the same borrowed weapon icon its bar buttons do, pre-resolved
// once per page (decisions 0230/0231).
pub(crate) use weapon_icon::{melee_auto_attack_icon, ranged_weapon_icon};

/// The auto-attack pseudo-spell (`Attack`, every character's slot-1 default): not a cast — it
/// toggles melee via the attack-swing pair. The USE path (`CMSG_ATTACKSWING`) keys on this id; the
/// ICON substitution keys on the effect type instead ([`benilla_formats::SpellDisplay::is_melee_auto_attack`],
/// decision 0231 — 6603 is simply the only spell carrying `SPELL_EFFECT_ATTACK`).
pub(crate) const SPELL_ATTACK: u32 = 6603;

/// The player's action store: the occupied wire slots (0..119) + the known-spell set. Written by
/// the net bridge (`SMSG_INITIAL_SPELLS`/`SMSG_ACTION_BUTTONS`) AND, since decision 0216 §7,
/// directly by [`drain::drain_action_sets`] (the bar is client-authoritative — a local
/// pickup/place is never echoed back by the server, vmangos `MasterPlayer::addActionButton`/
/// `removeActionButton` send nothing). Read by [`feed::feed_actions`]/[`drain::drain_action_uses`].
#[derive(Resource, Default)]
pub(crate) struct PlayerActions {
    /// Wire slot (0-based) → the slot's packed action. Lua action id = slot + 1.
    pub buttons: HashMap<u8, ActionButton>,
    /// The spell book (`SMSG_INITIAL_SPELLS`).
    ///
    /// **Ordered, and that is load-bearing** (decision 1312). The reference keeps its known spells
    /// in an ARRAY and the scans that hunt it — the GameObject lock resolver `0x5f83d0` above all —
    /// stop at the first hit, so the visit order picks *which* of several equally-qualified spells
    /// wins. A `HashSet` made that pick nondeterministic: every character knows both 6478
    /// "Opening" and 22810 "Opening - No Text" (both `LockType 13`, both trivially sufficient), and
    /// whichever the hash happened to reach first went on the cast bar (B247). Ascending spell id
    /// is the array's own order after login — the server sends the initial batch out of a
    /// `std::map`, so the wire arrives sorted.
    pub spells: BTreeSet<u32>,
    /// Set on every book/bar arrival AND every local `action_sets` drain; cleared by the feed
    /// after re-resolving each slot's identity (icon/kind/action) and pushing. It is only ONE of
    /// the identity resolve's two triggers — a landed item template is the other (decision 0660,
    /// [`Items::template_epoch`]) — and it gates ONLY that resolve: an ITEM slot's bag COUNT is
    /// refreshed unconditionally every frame instead (see [`feed`]'s module doc), since it drifts
    /// independently of both.
    ///
    /// [`Items::template_epoch`]: crate::items::Items::template_epoch
    pub dirty: bool,
}

/// **The world right-click's GameObject-opener queue** — the lock chain's resolved action, carried
/// one frame to the one cast path (decision 2199).
///
/// It exists because the right-click system ([`crate::target::click::act_on_right_click`]) and
/// [`crate::spell::CastLadder`] want the same half-dozen
/// resources, so the click cannot call the ladder in place — a resource reachable twice from one
/// system is a `B0002` panic on the first live frame. A one-frame queue is the seam, and it keeps
/// the rule that **nothing sends a cast except the ladder**.
///
/// Before it, the opener was the last cast in the tree that sent its own packet. That is not a
/// tidiness point: the ladder is where the in-flight rung lives (`6e4d97` — the reference's
/// already-casting refusal, whose same-spell leg `6e4d43` is *silent*), so spamming right-click on
/// a chest shipped a `CMSG_CAST_SPELL` per click, vmangos answered every duplicate
/// `SPELL_FAILED_SPELL_IN_PROGRESS`, and that failure — naming the **same** spell as the running
/// cast — red-faded the running bar while the cast completed anyway. Character for character the
/// B200 report decision 0908 fixed for items; this is the same bug at the arm 0914 named as still
/// open.
#[derive(Resource, Default)]
pub(crate) struct GoOpenerCasts(pub(crate) Vec<GoOpener>);

/// One queued opener — what the lock chain resolved the right-click to
/// ([`crate::target::click::resolve_go_action`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum GoOpener {
    /// A known `OPEN_LOCK` spell the player satisfies, cast **at the object** — `CMSG_CAST_SPELL`
    /// carrying the GameObject target block (decision 0239).
    Spell { spell_id: u32, go_guid: u64 },
    /// A key slot we carry: `CGItem::Use` with the lock's guid, which the commit turns into
    /// `CMSG_USE_ITEM` + `TARGET_FLAG_GAMEOBJECT` (decision 0769). The item carries the bound guid
    /// on its own [`crate::ui_items::ItemUse::on_object`].
    Key(crate::ui_items::ItemUse),
}

/// The spell display catalog + the shapeshift bonus-bar map (absent when the client data isn't —
/// every consumer tolerates that). `pub(crate)`: the cast-visual router
/// (`crate::creature_anim::spell_visual`) resolves spell → visual through the same catalog — one
/// `Spell.dbc` load serves both faces (decision 0107).
#[derive(Resource)]
pub(crate) struct Spells {
    pub(crate) catalog: SpellCatalog,
    /// Form id → the `SpellShapeshiftForm.dbc` row: **BonusActionBar** (the client's own paging
    /// map: `GetBonusBarOffset 0x4e7620` reads a cached copy of exactly this
    /// lookup) + **flags1** (the form gate's stance bit, [`state`]'s usable walk; the
    /// toggle-cancel block bit, `crate::ui_shapeshift`'s drain).
    pub(crate) forms: std::collections::HashMap<u32, benilla_formats::ShapeshiftForm>,
    /// `SpellRange.dbc` — the byte-verified `GetMinMaxRange 0x6e3480` inputs the range indicator
    /// reads ([`state`], decision 0137 phase 4). Empty when the DBC failed (range reads `None`).
    pub(crate) ranges: benilla_formats::SpellRangeCatalog,
    /// `SpellCastTimes.dbc` — the tooltip's cast-time cell (byte-verified `GetCastTime 0x6e3340`
    /// reads `CastingTimeIndex` against it; decision 0274 P2). Empty on a failed load.
    pub(crate) cast_times: benilla_formats::SpellCastTimeCatalog,
    /// `SpellDuration.dbc` — the `$d`/`$o` tokens' source (`GetDuration 0x6ea000`).
    pub(crate) durations: benilla_formats::SpellDurationCatalog,
    /// `SpellRadius.dbc` — the `$a` token's yards.
    pub(crate) radii: benilla_formats::SpellRadiusCatalog,
}

impl Spells {
    /// The resolved cast time, ms — `GetCastTime 0x6e3340`'s level-scaled walk: `CastingTimeIndex`
    /// resolves the [`Self::cast_times`] row, `base + perLevel·(casterLevel − baseLevel)` floors to
    /// the row's minimum (row 1, the all-zero instant sentinel, resolves 0). The level term keys on
    /// the `SpellRec+0x70` column ([`SpellDisplay::base_level`]). Spell-mod op `0xa`
    /// (SPELLMOD_CASTING_TIME) is still unread here — the tables themselves are live
    /// (`crate::spell::mods`), only this consumer is not wired to them, so a talent-shortened
    /// cast still shows its untalented length.
    /// A missing row reads 0 (instant), like a failed catalog load everywhere else.
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
    /// An empty catalog set — the usable walk's unit tests need only the `forms` map.
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

/// The client's **learned-ability latches** — `[0xb700e4]` and `[0xb700e8]`, mirrored (decision
/// 0752). The reference does not scan the spell book to answer "can this player skin?": it caches
/// the answer at *learn* time. `0x4b25e0`, right after setting the known-spell bit, tests the
/// freshly-learned spell's `Effect[0]` and stores the spell id into a dedicated global —
/// `0x5f` (`SPELL_EFFECT_SKINNING`) → `[0xb700e4]`, `0x74` (`SPELL_EFFECT_SKIN_PLAYER_CORPSE`) →
/// `[0xb700e8]` — and the unlearn path `0x4b2c50` zeroes whichever global named that spell.
///
/// The world cursor's skin leg then reads `[0xb700e4 + 4×isPlayerTarget]` as a hard precondition
/// (`0x482589`): **a corpse flagged `UNIT_FLAG_SKINNABLE` shows no skin cursor at all to a player
/// who never learned Skinning.** Without it the ladder offers the knife to everyone.
#[derive(Resource, Default)]
pub(crate) struct LearnedAbilities {
    /// `[0xb700e4]` — our known `SPELL_EFFECT_SKINNING` spell (creature skinning), `None` if we
    /// never learned one. It is also the spell the skin click casts, so there is one lookup here,
    /// not a second scan at the click.
    pub(crate) skinning: Option<u32>,
    /// `[0xb700e8]` — our known `SPELL_EFFECT_SKIN_PLAYER_CORPSE` spell (the PvP insignia). Kept
    /// for symmetry with the reference's pair; the insignia arm of the cursor isn't modelled yet.
    pub(crate) skin_player_corpse: Option<u32>,
    /// `[0xcecad8]` — our known `SPELL_EFFECT_FEED_PET` spell (Feed Pet 6991), `None` if we never
    /// learned one. The reference latches it the same way, at learn time: `0x6ea1d0` (←
    /// `0x5e9e49`) stores the spell whose `Effect[0] == 0x65`. It is the **third gate** on
    /// [`targeting::drop_item_on_unit`]'s pet leg, and the spell that leg casts — so, like
    /// `skinning`, one lookup here rather than a second scan at the drop.
    pub(crate) feed_pet: Option<u32>,
}

/// `SpellEffects` value `0x74` — `SPELL_EFFECT_SKIN_PLAYER_CORPSE` (the "Remove Insignia" family),
/// the second of the two effects `0x4b25e0` latches.
const SPELL_EFFECT_SKIN_PLAYER_CORPSE: u32 = 0x74;

/// `SpellEffects` value `0x65` (101) — `SPELL_EFFECT_FEED_PET`, the effect the reference tests at
/// learn time to latch `[0xcecad8]` (`0x5e9e42`).
/// Feed Pet 6991 is the only shipped row carrying it.
const SPELL_EFFECT_FEED_PET: u32 = 0x65;

/// Re-derive [`LearnedAbilities`] whenever the spell book changes — our stand-in for the
/// reference's learn/unlearn write sites. Change-detected, so it is a no-op on almost every frame;
/// rescanning the book beats threading a hook through every spell-arrival path, and it cannot
/// drift from the book the way an incrementally-maintained latch could.
fn track_learned_abilities(
    actions: Res<PlayerActions>,
    spells: Option<Res<Spells>>,
    mut learned: ResMut<LearnedAbilities>,
) {
    let Some(spells) = spells else { return };
    if !actions.is_changed() && !spells.is_changed() {
        return;
    }
    // The **last** matching spell in ascending-id order, not the first: the reference latches at
    // learn time and each new rank overwrites the global, so what it holds is the newest rank
    // learned — and a rank chain is ascending by id (Skinning 8613 → 8617 → 8618 → 10768). Which
    // end we take is only visible when a chain's ranks are known together, but "arbitrary" was
    // never an answer: the set was a `HashSet` until 1312 and this picked at the hash's whim.
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

/// `SpellMechanic.dbc` — the vocabulary that fills `SPELL_FAILED_PREVENTED_BY_MECHANIC`'s `%s`
/// ([`benilla_formats::SpellMechanicCatalog`], decision 1948). Its one reader is the cast-failure
/// resolver's `0x8d` arm.
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
        // Absent data is the strip fallback, not a failure: the refusal still appears, it just
        // shows its bare stem instead of naming what is holding you.
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
                    // Feed rides with the unit feed, before the VM ticks; both drains run after
                    // the input pass so a click's UseAction/PickupAction/PlaceAction goes out the
                    // same frame. The two queues are disjoint per gesture (a checkCursor place
                    // routes entirely to `action_sets`, never also queuing a use), so the drains'
                    // relative order doesn't matter. The dynamic-state feed follows the identity
                    // feed so a fresh slot's first state push lands the same frame.
                    // The rank pass runs on the same `dirty` flag the identity feed consumes,
                    // and strictly before it: a slot corrected here is resolved and pushed with
                    // its right rank the same frame, so a stale rank never reaches a pixel
                    // (decision 0883).
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
                    // The T binding (0997): the attack arm's twin door, after the dispatch wrote
                    // this frame's fires.
                    drain::attack_target_binding.after(UiInput),
                    // The world right-click's opener (2199), beside the chain cast and for the
                    // same reason: the click resolved it, the ladder sends it. After the input
                    // pass like the other drains — the queue is filled by the target chain,
                    // which runs earlier in the frame.
                    drain::drain_go_openers.after(UiInput),
                    // The learned-ability latches must be current before the target chain's
                    // cursor classifier reads them; the book feed runs in `UnitFeed`, so sitting
                    // right after it is enough.
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
