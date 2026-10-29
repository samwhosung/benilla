//! The app side of the action bar: the action table, the spell catalogs and the plugin. [`feed`]
//! shows the engine's refusals and seeds a reloaded VM's copy of the bar, and [`state`] answers
//! each slot's cooldown, usability, range and checked state at the getter's call; [`drain`] sends `UseAction` as a cast, swing, item use or macro, and
//! `PickupAction`/`PlaceAction` as `CMSG_SET_ACTION_BUTTON`.

use std::collections::{BTreeSet, HashMap};

use bevy::prelude::*;

use benilla_formats::SpellCatalog;
use benilla_protocol::messages::ActionButton;

use crate::ui_unit::UnitFeed;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};

mod cast_fail;
mod cast_lines;
pub(crate) use cast_lines::CastFailLines;
mod drain;
// A `UseAction` press, applied in call order by `crate::script_calls`.
pub(crate) use drain::{
    attack_nearest_probe, drain_arrival_casts, finish_equip, use_action, ActionPress, AttackPress,
    UseOutcome,
};
// The bar's sends and slot writes inside their calls.
pub(crate) use drain::ActionBarSteps;
// The right-click casts' drain, which the world click runs inside its call.
pub(crate) use drain::{drain_click_casts, ClickCastDrain};
#[cfg(test)]
mod drain_tests;
pub(crate) mod drop_item;
#[cfg(test)]
mod drop_item_tests;
mod errors;
mod feed;
pub(crate) use feed::{bonus_bar_offset, seed_reloaded_vm, spell_action_icon, MISSING_ITEM_ICON};
#[cfg(test)]
mod feed_tests;
mod net;
mod ranks;
// The login's rank pass, run in the book's and the bar's handlers.
pub(crate) use ranks::normalize_ranks;
mod cooldown_lane;
#[cfg(test)]
pub(crate) use cooldown_lane::fire_cooldown_lane;
pub(crate) mod reactive;
#[cfg(test)]
mod reactive_tests;
pub(crate) mod recount;
mod state;
pub(crate) use state::{wears_entry, ActionStateMemory, ActionStates};
mod spell_slots;
pub(crate) mod toggle;
// The bar's legs of the spell packets, run in their handlers.
pub(crate) use spell_slots::SpellSlots;
mod weapon_icon;

pub(crate) use cast_fail::report_reaches_pop;
#[cfg(test)]
pub(crate) use cast_fail::CAST_FAIL_KEYS;
#[cfg(test)]
pub(crate) use errors::fire_ui_error_lane;
pub(crate) use errors::{
    attack_actor_refusal, keyed_line, keyed_line_s, reagent_totem_refusal, show_messages, show_now,
    ui_error_text, CastFail, Caster, FillArg, MessageSink, Shown, UiError, UiErrorKeys,
};
pub(crate) use weapon_icon::{melee_auto_attack_icon, ranged_weapon_icon, substitutes_weapon_icon};

/// `Attack`, the auto-attack pseudo-spell: it toggles melee rather than casting. The use path
/// keys on this id; the icon keys on `SPELL_EFFECT_ATTACK`, which only 6603 carries.
pub(crate) const SPELL_ATTACK: u32 = 6603;

/// The action slots (wire 0..119), the reference's slot arrays, which `0x4e58e0` writes inside the
/// call that places or clears one (`0x4e590a`), so the chunk's next verb reads the write, and
/// inside the spell packets' handlers that clear or replace one ([`spell_slots`]). Apart from
/// [`PlayerActions`] so the call host's bar verbs hold it alone ([`ActionBarSteps`]). The bar is
/// client-authoritative: vmangos stores a slot without a reply (`MasterPlayer::addActionButton`).
#[derive(Resource, Default)]
pub(crate) struct ActionSlots {
    /// Wire slot (0-based) to its packed action; the Lua action id is slot + 1.
    pub buttons: HashMap<u8, ActionButton>,
    /// `0xc4c2b4`: the words `SMSG_ACTION_BUTTONS` staged (`0x5e6680`), empty ones left out,
    /// which only our player's arrival loads into the bar ([`Self::load_staged`]). No game entry
    /// clears them.
    pub(crate) staged: HashMap<u8, ActionButton>,
    /// Whether the staged words are this login's and wait for our player's arrival to load them:
    /// set by `SMSG_ACTION_BUTTONS`, cleared by the load and by the game entry.
    unloaded: bool,
}

impl ActionSlots {
    /// `SMSG_ACTION_BUTTONS`'s 120 words staged, every one replaced (`0x5e6680` into `0xc4c2b4`);
    /// the bar itself waits for our player's arrival.
    pub(crate) fn stage(&mut self, buttons: impl IntoIterator<Item = ActionButton>) {
        self.staged = buttons.into_iter().map(|b| (b.slot, b)).collect();
        self.unloaded = true;
    }

    /// Whether this login's staged words wait for the arrival's load ([`Self::stage`],
    /// [`Self::load_staged`]): a worldport's book, which vmangos sends ahead of its bar
    /// (`Player.cpp:19253`, `:19256`), finds the last staging loaded.
    pub(crate) fn staged_unloaded(&self) -> bool {
        self.unloaded
    }

    /// Our player's arrival's load of the staged words (`0x5deae2`-`0x5deb12`): each stored into
    /// the bar through `0x4e5d60`, but a word whose top nibble is clear and that `knows` does not
    /// hold (`0x60c740`, skipped at `0x5deaf5`), an empty word among them, whose slot keeps what it
    /// held. The 1-based ids stored, in slot order.
    pub(crate) fn load_staged(&mut self, knows: impl Fn(u32) -> bool) -> Vec<u32> {
        self.unloaded = false;
        let mut staged: Vec<ActionButton> = self.staged.values().copied().collect();
        staged.sort_by_key(|b| b.slot);
        let mut loaded = Vec::new();
        for b in staged {
            let word = (u32::from(b.kind) << 24) | b.action;
            if word & 0xf000_0000 == 0 && !knows(word) {
                continue;
            }
            self.buttons.insert(b.slot, b);
            loaded.push(u32::from(b.slot) + 1);
        }
        loaded
    }

    /// The game entry's reset (`ResetActionBarState 0x4e4f50`): every word 0, and no staging yet
    /// for this login.
    fn game_entry(&mut self) {
        self.buttons.clear();
        self.unloaded = false;
    }
}

/// The known spells and the book's lists the registrar keeps, which the learn, unlearn and
/// supersede handlers write at their packet, the bar's slots apart ([`ActionSlots`]).
#[derive(Resource, Default)]
pub(crate) struct PlayerActions {
    /// The known spells (`SMSG_INITIAL_SPELLS` and the learns since), the reference's known-spell
    /// bits `0xb710fc` (`0x4b2603`): a set, which keeps no learn order.
    pub spells: BTreeSet<u32>,
    /// The order the profession lines' lists hold their spells (`CGPlayer_C + 0x1c74`): the login
    /// list in wire order (`0x5dea8b`-`0x5deab9` replays it through `LearnSpell 0x5e9c20`), each
    /// learn appended with no duplicate check (`0x5e9d72`), an unlearn taking out the last copy
    /// with the rest closing up (`0x5ea0fe`, `0x5ea123`).
    pub profession_order: Vec<u32>,
    /// The order the craft types' lists hold theirs (`+0x1cd0`): a learn appends a spell the list
    /// lacks (`0x5e9db3`, `0x5e9e0f`), an unlearn takes out its first copy (`0x5ea1b8`).
    pub craft_order: Vec<u32>,
    /// `CGPlayer_C+0x1d10`, the spell each effect slot holds (`0x5ea360`): written by a learn
    /// (`list_learn`), never cleared by an unlearn, emptied with the book.
    pub effect_slots: [Option<u32>; 4],
    /// `[0xb700b0]`: the known spells whose `Effect[0]` is `SPELL_EFFECT_OPEN_LOCK`, in learn
    /// order. The login list enters in wire order (`0x5dea8b`-`0x5deab9`), each learn appends at
    /// the tail with no duplicate check (`0x4b2665`-`0x4b2689`), and an unlearn removes the last
    /// copy and closes the gap (`0x4b2d60`-`0x4b2d87`). The lock resolver `0x5f83d0` and the
    /// item tooltip's lock walk `0x5da5e0` scan it from the front, so its order picks between
    /// 6478 "Opening" and 22810 "Opening - No Text", which open the same locks.
    pub openers: Vec<u32>,
    /// `[0xb71118]`: the dispel list, each `EffectMiscValue` of a registered spell's
    /// `SPELL_EFFECT_DISPEL` effects not aimed at an enemy, appended once at the registration
    /// (`0x4b26a2`-`0x4b26ee`). No unlearn takes one out; the book's reset empties it
    /// (`0x4b2486`-`0x4b24c2`). `raidFilter`'s debuff test reads it (`0x4b3920`).
    pub dispels: Vec<u32>,
    /// `[0xb70098]`, the spell `Stuck()` casts: the last registered spell whose `Effect[0]` is
    /// `SPELL_EFFECT_STUCK` (`0x4b2614`-`0x4b261d`); `None` is its 0, after the book's reset
    /// (`0x4b2433`).
    pub stuck_spell: Option<u32>,
    /// `[0xc4d2a8]` (count `[0xc4d2a4]`): `SMSG_INITIAL_SPELLS`'s list in wire order, which our
    /// player's arrival learns one spell at a time (`0x5dea8b`-`0x5deab9`).
    pub initial: Vec<u32>,
    /// `[0xb71108]` (count `[0xb71104]`): the form bar's spells in bar order, which the registrar
    /// grows one learn at a time and the book's reset empties ([`crate::ui_shapeshift`]).
    pub forms: Vec<u32>,
    /// The form-list edits since our latest create, while its arrival waits on the arrival lane:
    /// the reference's arrival runs inside the create's packet, ahead of every later learn, so the
    /// arrival's replay applies these after its own ([`crate::ui_world_enter`]).
    pub form_edits: Option<Vec<crate::ui_shapeshift::FormEdit>>,
}

impl PlayerActions {
    /// Whether a known spell's first effect is `SPELL_EFFECT_DUAL_WIELD` (40): the reference's
    /// `[0xc4d770]`, which a learn of such a spell sets (`0x5e9fc1`-`0x5e9fcd`), its unlearn clears
    /// (`0x5ea269`-`0x5ea271`) and `0x5eab70` reads. No catalog, no such spell.
    pub(crate) fn knows_dual_wield(&self, spells: Option<&Spells>) -> bool {
        spells.is_some_and(|s| {
            self.spells
                .iter()
                .any(|&id| s.catalog.get(id).is_some_and(|sd| sd.effects[0] == 40))
        })
    }
}

/// `[0xb71148]`: the reactive spells, which the registrar takes in one learn at a time, our
/// player's arrival empties and takes the initial spells into again, and each usable refresh
/// re-checks ([`reactive::ReactiveList`]). Apart from [`PlayerActions`], which the refresh's
/// callers read beside it.
#[derive(Resource, Default)]
pub(crate) struct ReactiveSpells(pub(crate) reactive::ReactiveList);

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
    /// it is not streamed, which skips the multiply (`0x6e341d`).
    pub(crate) fn cast_time_unclamped_ms(
        &self,
        def: &benilla_formats::SpellDisplay,
        caster: Option<&benilla_protocol::ObjectFields>,
        level: crate::spell::SpellLevel,
        mods: &crate::spell::SpellModifiers,
    ) -> i32 {
        let Some(row) = self.cast_times.get(def.casting_time_index) else {
            return 0;
        };
        // The row's level, `0x6e3130`, read only by a row that scales by it.
        let caster_level = if row.reads_caster_level() {
            level.read(def)
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
        let time = self.cast_time_unclamped_ms(def, caster, level, mods);
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

/// The bar's word at the 1-based `action` and the VM's copy of it, as a write leaves both, for a
/// test that places a slot outside the verbs.
#[cfg(test)]
pub(crate) fn hold_for_test(world: &mut World, action: u32, kind: u8, id: u32) {
    let slot = u8::try_from(action - 1).expect("a bar slot");
    world.resource_mut::<ActionSlots>().buttons.insert(
        slot,
        ActionButton {
            slot,
            action: id,
            kind,
        },
    );
    if let Some(mut script) = world.get_non_send_resource_mut::<benilla_ui::script::UiScript>() {
        script.set_action(
            action,
            Some(benilla_ui::script::ActionSlot {
                kind,
                action: id,
                ..Default::default()
            }),
        );
    }
}

/// `chunk`'s answer with the bar's live reads lent as the call host lends them ([`ActionStates`],
/// and the worn walk `IsEquippedAction` asks), for a test without the whole host.
#[cfg(test)]
pub(crate) fn eval_live<T: benilla_ui::mlua::FromLuaMulti>(world: &mut World, chunk: &str) -> T {
    use crate::script_calls::live::LiveFamily;
    type Reads = (
        state::ActionStates<'static, 'static>,
        crate::script_calls::live::StatsLive<'static, 'static>,
    );
    struct Lent<'a, A, B>(&'a mut A, &'a mut B);
    impl<A: LiveFamily, B: LiveFamily> benilla_ui::script::LiveReader for Lent<'_, A, B> {
        fn read_live(&mut self, ask: &mut benilla_ui::script::live::LiveAsk<'_>) -> bool {
            self.0.answer(ask) || self.1.answer(ask)
        }
    }
    world.init_resource::<state::ActionStateMemory>();
    let mut script = world
        .remove_non_send_resource::<benilla_ui::script::UiScript>()
        .expect("a VM");
    let mut state = bevy::ecs::system::SystemState::<Reads>::new(world);
    let r = {
        let (mut states, mut stats) = state.get_mut(world);
        script.reading(&mut Lent(&mut states, &mut stats), |s| {
            s.eval::<T>(chunk)
                .unwrap_or_else(|e| panic!("{chunk}: {e}"))
        })
    };
    state.apply(world);
    world.insert_non_send_resource(script);
    r
}

/// A field watcher's usable refresh (`0x4b31c0`), as its notice runs it inside the drain
/// ([`crate::field_watch::FieldNotice::UsableRefresh`]), for a test with no notice lane.
#[cfg(test)]
pub(crate) fn run_watcher_refresh_once(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(
            |mut states: state::ActionStates,
             mut script: NonSendMut<benilla_ui::script::UiScript>| {
                states.usable_refresh(&mut script).fire(&mut script);
            },
        )
        .expect("the watcher's refresh runs as a one-shot system");
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

/// The reference's learned-ability latches, which the spell packets' handlers write at their
/// packet: the registrar `0x4b25b0` stores a registered spell's id by its `Effect[0]`
/// (`0x4b262c`, `0x4b263b`), `LearnSpell 0x5e9c20` the Feed Pet spell (`0x5e9e49` →
/// `SetFeedPetSpell 0x6ea1d0`), and an unlearn's `0x4b2c50` clears the first two while they hold
/// its spell (`0x4b2c83`-`0x4b2c9d`). The skin cursor requires the latch (`0x482589`), so a player
/// who never learned Skinning gets no skin cursor.
#[derive(Resource, Default)]
pub(crate) struct LearnedAbilities {
    /// `[0xb700e4]`: the known `SPELL_EFFECT_SKINNING` spell, which the skin click also casts.
    pub(crate) skinning: Option<u32>,
    /// `[0xb700e8]`: the known `SPELL_EFFECT_SKIN_PLAYER_CORPSE` spell; no cursor reads it yet.
    pub(crate) skin_player_corpse: Option<u32>,
    /// `[0xcecad8]`: the known `SPELL_EFFECT_FEED_PET` spell, latched by `0x6ea1d0` (from
    /// `0x5e9e49`); gate 3 of [`drop_item::drop_item_on_unit`] and the spell it casts. No unlearn
    /// clears it; the world entry does (`0x6e7311`).
    pub(crate) feed_pet: Option<u32>,
}

impl LearnedAbilities {
    /// The registrar's legs (`0x4b2623`-`0x4b263b`): each registered spell whose `Effect[0]` is
    /// skinning or the corpse skin overwrites its latch, so the last registered holds it.
    pub(crate) fn registered(&mut self, spell_id: u32, spells: Option<&Spells>) {
        let effect = spells
            .and_then(|s| s.catalog.get(spell_id))
            .map(|d| d.effects[0]);
        match effect {
            Some(benilla_formats::SPELL_EFFECT_SKINNING) => self.skinning = Some(spell_id),
            Some(SPELL_EFFECT_SKIN_PLAYER_CORPSE) => self.skin_player_corpse = Some(spell_id),
            _ => {}
        }
    }

    /// `LearnSpell`'s Feed Pet leg: a spell with no `Attributes & 0x20` (`0x5e9c8e`), no cast UI
    /// (`0x5e9d88`) and `SPELL_EFFECT_FEED_PET` first (`0x5e9e42`).
    pub(crate) fn learned(&mut self, spell_id: u32, spells: Option<&Spells>) {
        let latches = spells
            .and_then(|s| s.catalog.get(spell_id))
            .is_some_and(|d| {
                d.attributes & benilla_formats::SPELL_ATTR_IS_TRADESKILL == 0
                    && (d.cast_ui as i32) <= 0
                    && d.effects[0] == SPELL_EFFECT_FEED_PET
            });
        if latches {
            self.feed_pet = Some(spell_id);
        }
    }

    /// `0x4b2c50`'s legs: a latch holding `spell_id` goes to 0 (`0x4b2c83`-`0x4b2c9d`).
    pub(crate) fn unregistered(&mut self, spell_id: u32) {
        for latch in [&mut self.skinning, &mut self.skin_player_corpse] {
            if *latch == Some(spell_id) {
                *latch = None;
            }
        }
    }

    /// `ResetSpellBook 0x4b23a0` zeroes the two registrar latches (`0x4b2439`, `0x4b243f`).
    pub(crate) fn book_reset(&mut self) {
        self.skinning = None;
        self.skin_player_corpse = None;
    }
}

/// The "Remove Insignia" effect, the second one `0x4b25e0` latches.
const SPELL_EFFECT_SKIN_PLAYER_CORPSE: u32 = 0x74;

/// The effect latched into `[0xcecad8]` at learn time (`0x5e9e42`); only Feed Pet 6991 has it.
const SPELL_EFFECT_FEED_PET: u32 = 0x65;

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
        cooldown_lane::add_cooldown_lane(app);
        errors::add_ui_error_lane(app);
        // A UI-only harness has no net stack; declaring a message twice is idempotent.
        app.add_message::<crate::net::FieldChanged>()
            .init_resource::<state::ActionStateMemory>()
            .init_resource::<PlayerActions>()
            .init_resource::<ReactiveSpells>()
            .init_resource::<ActionSlots>()
            .init_resource::<LearnedAbilities>()
            .init_resource::<ClickCasts>()
            .add_systems(Startup, load_spells.after(AssetSet::Open))
            .add_systems(Startup, load_spell_mechanics.after(AssetSet::Open))
            .add_systems(
                Update,
                // The refusals ride with the unit feed, before the VM ticks.
                feed::feed_actions.in_set(UnitFeed),
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
