//! The pet action bar seam (decision 0982) — the eight bindings `PetActionBarFrame.lua` consumes
//! (`GetPetActionInfo`/`GetPetActionsUsable`/`GetPetActionCooldown`/`PetHasActionBar`/
//! `CastPetAction`/`TogglePetAutocast`/`IsPetAttackActive`/`PetStopAttack`) over an app-pushed
//! slot snapshot, in [`super::shapeshift`]'s two-way shape: the app resolves everything (which
//! slot is a command, a reaction or a spell; its icon, name, checked and autocast bits; its
//! cooldown) and pushes it ([`super::UiScript::set_pet_actions`]); the engine drains the click
//! intents ([`super::UiScript::take_pet_actions`] and kin) back out.
//!
//! **The engine holds no pet knowledge**: a slot here is "a name, a subtext, a texture, four bits
//! and a cooldown triple". In particular it does not know that a token slot's `name`/`texture` are
//! *the names of globals* rather than values — that convention belongs to the reference's own
//! `GetPetActionInfo` and is reproduced faithfully by the app, which sets `is_token` and lets the
//! Lua do the `getglobal` (the shipped `PetActionBarFrame.lua:98-104` fork).
//!
//! Return conventions are the 1.12 API's own, matching [`super::action`]: 1/nil booleans, and the
//! cooldown as `(start_s on the GetTime clock, duration_s, enable)` with the same
//! elapsed-goes-cold rule `GetActionCooldown` uses.
//!
//! Two later families joined the bar's eight through the same two-way seam, because they are about
//! the same unit and move on the same push: the **hunter stat block** ([`PetStats`], decision 1005)
//! that the pet paper doll and the happiness icon read, and the **right-click menu**
//! (`PetCanBeAbandoned`/`PetCanBeRenamed`/`PetAbandon`/`PetDismiss`/`PetRename`, decision 1066).

use mlua::{Lua, MultiValue, Value};

use super::Model;

// The slot-word bytes the one-shot orders synthesize — the same values
// `benilla_protocol::messages::pet` names for the wire (the engine stays protocol-free, 0068 §3,
// so they are restated here rather than imported): the type byte, then the command / reaction.
const PET_ACT_COMMAND: u8 = 0x07;
const PET_ACT_REACTION: u8 = 0x06;
const PET_COMMAND_STAY: u32 = 0;
const PET_COMMAND_FOLLOW: u32 = 1;
const PET_COMMAND_ATTACK: u32 = 2;
const PET_REACT_PASSIVE: u32 = 0;
const PET_REACT_DEFENSIVE: u32 = 1;
const PET_REACT_AGGRESSIVE: u32 = 2;

/// A synthesized slot word: the type byte in the top byte, the action in the low bits — the
/// server's own packing (`PetActionEntry`).
const fn order(kind: u8, action: u32) -> u32 {
    (kind as u32) << 24 | action
}

/// One pet bar slot, fully resolved by the app before pushing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetActionView {
    /// `GetPetActionInfo`'s first return, and the slot's OCCUPANCY test — `None` hides the button
    /// (`PetActionBarFrame.lua:122-128`). For a spell slot this is the spell's name; for a token
    /// slot it is **the name of a global** (`"PET_ACTION_ATTACK"`), which the Lua resolves.
    pub name: Option<String>,
    /// The second return — the spell's rank line, or `None` for a token.
    pub subtext: Option<String>,
    /// The third return: an icon path for a spell, **the name of a global**
    /// (`"PET_ATTACK_TEXTURE"`) for a token. `None` leaves the button art empty and swaps its
    /// NormalTexture to the unfilled `UI-Quickslot`.
    pub texture: Option<String>,
    /// Is this a command/reaction token (so `name`/`texture` are global names)?
    pub is_token: bool,
    /// The slot's spell, when it has one — what `GameTooltip:SetPetAction` renders. `None` for a
    /// token and for an empty slot. Not a `GetPetActionInfo` return: the reference's tooltip
    /// channel reaches the pet spellbook itself, and this is that reach.
    pub spell_id: Option<u32>,
    /// The checked ring.
    pub active: bool,
    /// This slot CAN autocast — the static `UI-AutoCastableOverlay` ring.
    pub autocast_allowed: bool,
    /// …and it currently does — the sparkle trail.
    pub autocast_enabled: bool,
    /// Whether a left click on this slot means "call the pet off" rather than "do this"
    /// (`IsPetAttackActive`, the Attack button's second press).
    pub attack_active: bool,
    /// `(start_ms on the GetTime clock, duration_ms, enabled)` — [`super::action::ActionState`]'s
    /// exact shape; `None` = no cooldown.
    pub cooldown: Option<(i64, u32, bool)>,
    /// **The slot's packed word, verbatim** — the one place this seam's "the engine holds no pet
    /// knowledge" rule bends, and deliberately (decision 1010).
    ///
    /// The drag ([`super::cursor::pet`]) is word arithmetic in the reference and cannot be
    /// anything else: `0x4bc9a0` compares occupants under `& 0x3FFFFFFF`, tests a candidate's low
    /// 16 bits, and **writes the source word through unchanged** — `0x4bce00` forwards the cursor's
    /// payload dword without reading a field of it, and the payload itself is only ever a verbatim
    /// copy of a word that already existed in a slot. There is no drop-time encoding to model, so
    /// re-deriving a word from `(kind, action, bits)` at the engine boundary would be inventing one.
    ///
    /// `0` is the empty slot, which is also the reference's own test (the zero dword).
    pub packed: u32,
    /// `Attributes & 0x40` (`SPELL_ATTR_PASSIVE`) for a spell slot — the drop core's one source
    /// filter (`0x4bc9f8`–`0x4bca2e`: a type-1 source whose `SpellRec+0x18 & 0x40` is set is
    /// refused, silently). False for a token and for an empty slot.
    pub passive: bool,
}

/// [`PetActionView`] as stored: the cooldown converted to the `GetTime` clock at push time (the
/// [`super::shapeshift::StoredShapeshiftForm`] pattern).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StoredPetAction {
    pub(crate) view: PetActionView,
    /// `(start_s, duration_s, enabled)` in `GetTime` seconds; `None` = no cooldown.
    pub(crate) cooldown: Option<(f64, f64, bool)>,
}

/// The hunter-pet stat block behind `GetPetHappiness`/`GetPetLoyalty`/`GetPetTrainingPoints`/
/// `GetPetExperience` and `HasPetUI`'s second return (`0x4be670`; decision 1005), plus the two
/// **family**-derived answers `UnitCreatureFamily("pet")` and `GetPetFoodTypes()` (decision 1062).
///
/// **The four stat bindings share one gate** — `0x6116e0(pet)`, "is this a hunter's pet" — which is
/// why they share one pushed struct: a warlock's imp resolves perfectly well and still answers
/// nothing, because happiness, loyalty and training points are hunter machinery.
///
/// **The two family fields sit on OPPOSITE sides of that gate.**
/// [`Self::family`] is outside it — `UnitCreatureFamily 0x51a310` has no class test, so a warlock
/// minion shows "Imp" and gating it would blank a line the reference fills. [`Self::food_types`] is
/// inside it — `GetPetFoodTypes 0x4bea10` shares `0x6116e0` with the four stats.
///
/// **The stat failure conventions differ, and the difference is the API.** `GetPetLoyalty` fails to
/// **nil**; the two pairs fail to **`(0, 0)` — numbers, not nil**; `GetPetHappiness` fails to
/// **`(nil, 100.0, 0.0)`**, nil in the first slot and numbers in the other two. `PetFrame.lua`
/// hides the happiness icon on `not happiness` alone, so collapsing any of these into a uniform
/// nil-everything would hide the frame in cases the reference shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetStats {
    /// `HasPetUI`'s second return — the hunter-pet discriminator. False makes every field below
    /// answer its own failure convention regardless of what is in it.
    pub hunter_pet: bool,
    /// `GetPetHappiness`'s first return: the **pre-bucketed** `0..=3`. `None` is the gate
    /// failure's nil; `Some(0)` is a real answer and must stay distinguishable from it.
    pub happiness: Option<u32>,
    /// Return 2 — the damage percentage, already scaled by the client's own `100.0f`.
    pub damage_percentage: f32,
    /// Return 3 — the loyalty rate, unscaled and possibly negative.
    pub loyalty_rate: f32,
    /// `GetPetLoyalty`'s single return — the `PetLoyalty.dbc` name, verbatim including the shipped
    /// `"(Loyalty Level N) "` prefix. `None` = nil (level 0, or off the table).
    pub loyalty: Option<String>,
    /// `GetPetTrainingPoints` → `(totalPoints, spent)`, high word first.
    pub training_points: (u16, u16),
    /// `GetPetExperience` → `(currXP, nextXP)`.
    pub experience: (u32, u32),
    /// `UnitCreatureFamily("pet")` — the localized `CreatureFamily.dbc` word ("Imp", "Wind
    /// Serpent"), read from the cached creature-query record (`0x51a310`: `[[unit+0xb30]+0x1c]` →
    /// column `8 + locale`). Exactly **one** return on every path.
    ///
    /// `None` is the binding's **nil**, and the reference reaches it four ways that all mean "no
    /// word to print": no cached record yet (the creature query is still in flight), family id `0`,
    /// an id past the table, and — the one nobody guesses — **a null row in the middle of the id
    /// space**: 10/13/14/18/22 have no row in the shipped file, so this is a lookup miss, never a
    /// bounds check. The page guards its whole level-line `SetText` on this
    /// (ref `PetPaperDollFrame.lua:68-70`), so nil renders no line rather than a half one.
    pub family: Option<String>,
    /// `GetPetIcon()` — the pet's icon path, from the family row's own `CreatureFamily.dbc` icon
    /// column (`Interface\\Icons\\Ability_Hunter_Pet_<Family>` for all 22 shipped families). A pet
    /// has no item behind it, so this column is the only icon there is. `None` = nil, on the same
    /// four lookup-miss paths as [`Self::family`], which is why it sits beside it.
    ///
    /// **Outside the hunter gate, like [`Self::family`] and unlike [`Self::food_types`] — and that
    /// placement is INFERRED** (decision 1676). `GetPetIcon 0x4beb10` is registered adjacent to
    /// both in the same table and has not been read.
    /// It is grouped with the family word because it is a pure family-row lookup like that one,
    /// where the diet's gate is shared with the four *stat* bindings. The choice is unobservable
    /// in the reference's own call sites — every one of them (`PetStable.lua:51`, `161-162`) sits
    /// downstream of the window's own warlock early-return — so a warlock can never reach it there
    /// either way; only an addon could tell the difference.
    pub icon: Option<String>,
    /// `GetPetFoodTypes()` — the localized diet names the family's pet-food mask selects, in
    /// **record order** (`0x4bea10`: bit `1 << (recordID - 1)` against `CreatureFamily` column 7,
    /// the name from `ItemPetFood` column `1 + locale`). Varargs; the client returns the count and
    /// **never a nil**, so empty is zero values rather than one nil.
    ///
    /// **Empty is a real answer**, and it has two independent causes: a family whose mask is `0`
    /// (every warlock minion), and the binding's own `0x6116e0` gate — owner-is-me *and* the local
    /// player is a Hunter — which a charmed beast under a non-hunter fails even though its family
    /// row has a mask. The app applies that gate; this field is what it produced.
    pub food_types: Vec<String>,
}

/// The pet bar's pushed state: the slots plus the two bar-wide bits the reference exposes
/// separately from them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PetBarState {
    /// `PetHasActionBar()` — is there a bar at all. Distinct from "the slot list is empty": a
    /// possessed minion has a bar of pure commands, and a bar of ten empty slots is still a bar.
    pub(crate) has_bar: bool,
    /// `GetPetActionsUsable()` — false desaturates every icon on the bar at once.
    pub(crate) actions_usable: bool,
    pub(crate) slots: Vec<StoredPetAction>,
    /// `HasPetUI`'s FIRST return — "there is a pet with a UI at all", which the reference derives
    /// from the pet resolving plus its `UNIT_FIELD_PETNUMBER` being nonzero (`0x4be697`). Separate
    /// from `has_bar`: the action bar's gate is the cached guid alone, with no pet-number test.
    pub(crate) has_ui: bool,
    pub(crate) stats: PetStats,
    /// May the bar be **rearranged** — `PickupPetAction`'s own gate, and nobody else's
    /// (`0x4be1c1`: `[[pet+0x110]+0xA3] & 1 == 0`, i.e. `UNIT_FLAG_POSSESSED` clear).
    ///
    /// A third bar-wide bit rather than a fold into `actions_usable`, because the reference is
    /// deliberate about keeping them apart: possession is expressly **not** among the flags that
    /// grey the bar (a possessed unit is exactly the case where the buttons must still work), and
    /// the crowd-control flags that do grey it do **not** block a drag. Two gates, two questions.
    ///
    /// It sits before the cursor fork, so it blocks the DROP as well as the pick-up.
    pub(crate) pickup_allowed: bool,
    /// `PetCanBeAbandoned()` — **the pet right-click menu's whole fork** (decision 1066).
    ///
    /// Three of the PET menu's four rows show only when this is true (paperdoll, rename, abandon)
    /// and the fourth — Dismiss — shows only when it is *false* (`UnitPopup.lua:402-417`). So it is
    /// not "am I allowed to abandon"; it is "is this a pet I keep rather than a summon I called",
    /// and it is what makes one menu read *Abandon* on a hunter's pet and *Dismiss* on a demon.
    pub(crate) can_be_abandoned: bool,
    /// `PetCanBeRenamed()` — an independent predicate, ANDed with the one above for the rename row
    /// alone. One-shot server-side: a hunter pet carries it only until its first rename.
    pub(crate) can_be_renamed: bool,
}

impl super::UiScript {
    /// Push the whole pet bar, replacing whatever was there. A bare setter — firing
    /// `PET_BAR_UPDATE` is the app's diff-and-fire job, mirroring `set_shapeshift_forms`.
    pub fn set_pet_actions(
        &mut self,
        has_bar: bool,
        actions_usable: bool,
        pickup_allowed: bool,
        slots: Vec<PetActionView>,
    ) {
        // Field-by-field, never `pet_bar = PetBarState { .. }`: the struct also holds state that
        // moves on OTHER clocks (the stat block, the menu predicates), and rebuilding it wholesale
        // meant every one of those had to be hand-carried across this assignment or be silently
        // reset once a frame. Assigning what this setter owns cannot forget them.
        let bar = &mut self.model_mut().pet_bar;
        bar.has_bar = has_bar;
        bar.actions_usable = actions_usable;
        bar.pickup_allowed = pickup_allowed;
        bar.slots = slots
            .into_iter()
            .map(|view| {
                // The cooldown arrives with its absolute start already on the `GetTime` clock
                // (ms) — storing is a pure unit conversion, `set_shapeshift_forms`' seam.
                let cooldown = view.cooldown.map(|(start_ms, duration_ms, enabled)| {
                    (
                        start_ms as f64 / 1000.0,
                        f64::from(duration_ms) / 1000.0,
                        enabled,
                    )
                });
                StoredPetAction { view, cooldown }
            })
            .collect();
    }

    /// Push the hunter-pet stat block ([`PetStats`]) — the four paper-doll bindings plus
    /// `HasPetUI`. Separate from [`Self::set_pet_actions`] because it changes on a different clock:
    /// `SMSG_PET_SPELLS` replaces the bar wholesale, but happiness moves every few seconds off a
    /// plain descriptor field, so tying the two together would make the bar's diff-and-fire churn
    /// on a number no button draws.
    pub fn set_pet_stats(&mut self, has_ui: bool, stats: PetStats) {
        let bar = &mut self.model_mut().pet_bar;
        bar.has_ui = has_ui;
        bar.stats = stats;
    }

    /// Push the right-click menu's two predicates (decision 1066) — a third clock again, the pet's
    /// own `UNIT_FIELD_FLAGS`, which the rename's one-shot bit moves independently of both.
    pub fn set_pet_menu(&mut self, can_be_abandoned: bool, can_be_renamed: bool) {
        let bar = &mut self.model_mut().pet_bar;
        bar.can_be_abandoned = can_be_abandoned;
        bar.can_be_renamed = can_be_renamed;
    }

    /// Drain the 1-based slot indices `CastPetAction` queued since the last call. What each index
    /// *means* on the wire (a command, a reaction, a cast) is the app's to decide at drain time
    /// from the slot it still owns.
    pub fn take_pet_actions(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().pet_actions_pressed)
    }

    /// Drain the 1-based slot indices `TogglePetAutocast` queued.
    pub fn take_pet_autocast_toggles(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().pet_autocast_toggles)
    }

    /// Drain the `PetStopAttack()` calls queued (a count — the verb carries no argument).
    pub fn take_pet_stop_attacks(&mut self) -> u32 {
        std::mem::replace(&mut self.model_mut().pet_stop_attacks, 0)
    }

    /// Drain the pet one-shot orders (`PetAttack` & co.), each a packed slot word (1958).
    pub fn take_pet_orders(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().pet_orders)
    }

    /// `HasFullControl`'s flag, from the host's control edge (`ui_unit::feed_player_control`).
    pub fn set_player_control(&mut self, in_control: bool) {
        self.model_mut().player_control = in_control;
    }

    /// Drain the pet bar writes the drag queued (decision 1010) — **one `Vec` per
    /// `CMSG_PET_SET_ACTION`**, each of one or two `(0-based position, packed word)` pairs.
    ///
    /// Already-applied on the engine's side: the app's job is to mirror each pair into its own
    /// authoritative ten words and put the batch on the wire whole. Flattening the batches would
    /// break the server's body-size fork between the one- and two-entry forms.
    pub fn take_pet_set_actions(&mut self) -> Vec<Vec<(u32, u32)>> {
        std::mem::take(&mut self.model_mut().pet_set_actions)
    }

    /// Drain the `PetAbandon()` and `PetDismiss()` calls queued, as `(abandons, dismisses)` —
    /// counts, since neither verb carries an argument. Kept apart for the reason on the model's
    /// fields: two bindings, not one, whatever they end up sharing on the wire.
    pub fn take_pet_gives_up(&mut self) -> (u32, u32) {
        let m = &mut *self.model_mut();
        (
            std::mem::replace(&mut m.pet_abandons, 0),
            std::mem::replace(&mut m.pet_dismisses, 0),
        )
    }

    /// Drain the names `PetRename(name)` queued, in order.
    pub fn take_pet_renames(&mut self) -> Vec<String> {
        std::mem::take(&mut self.model_mut().pet_renames)
    }
}

/// The 1-based button index → stored slot, the reference's own indexing.
fn slot_at(model: &Model, i: u32) -> Option<&StoredPetAction> {
    usize::try_from(i.checked_sub(1)?)
        .ok()
        .and_then(|n| model.pet_bar.slots.get(n))
}

/// Register the pet-bar globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };

    // PetHasActionBar() → 1/nil. The bar frame's whole show/hide gate.
    g.set(
        "PetHasActionBar",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.pet_bar.has_bar))
        })?,
    )?;

    // GetPetActionsUsable() → 1/nil — one answer for the whole bar (the SetDesaturation sweep).
    g.set(
        "GetPetActionsUsable",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.pet_bar.actions_usable))
        })?,
    )?;

    // GetPetActionInfo(i) → name, subtext, texture, isToken, isActive, autoCastAllowed,
    // autoCastEnabled. An out-of-range index answers a single nil, which the Lua's `if (name)`
    // occupancy test reads exactly as an empty slot (the spellbook bindings' shape).
    g.set(
        "GetPetActionInfo",
        lua.create_function(move |lua, i: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(slot) = slot_at(&model, i) else {
                return Ok(MultiValue::from_vec(vec![Value::Nil]));
            };
            let v = &slot.view;
            let text = |s: &Option<String>| match s {
                Some(s) => Ok(Value::String(lua.create_string(s)?)),
                None => Ok::<_, mlua::Error>(Value::Nil),
            };
            Ok(MultiValue::from_vec(vec![
                text(&v.name)?,
                text(&v.subtext)?,
                text(&v.texture)?,
                flag(v.is_token),
                flag(v.active),
                flag(v.autocast_allowed),
                flag(v.autocast_enabled),
            ]))
        })?,
    )?;

    // GetPetActionCooldown(i) → start, duration, enable — GetActionCooldown's triple and its
    // elapsed-goes-cold rule (an elapsed/absent cooldown answers (0, 0, 1) so a re-feed never
    // replays the sweep).
    g.set(
        "GetPetActionCooldown",
        lua.create_function(|lua, i: u32| {
            let now: f64 = lua.globals().get("__benilla_now").unwrap_or(0.0);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match slot_at(&model, i).and_then(|s| s.cooldown) {
                Some((start, duration, enabled)) if start + duration > now || !enabled => {
                    (start, duration, i32::from(enabled))
                }
                _ => (0.0, 0.0, 1),
            })
        })?,
    )?;

    // IsPetAttackActive(i) → a BOOLEAN — the left-click fork: true means the press should call
    // the pet OFF (`PetStopAttack`) instead of running the slot.
    //
    // The odd one out of this file's returns, and deliberately so: it is the single pet binding
    // that pushes a real Lua boolean (`0x6f39f0`) rather than the 1/nil the rest use, so it
    // answers `false`, never nil, even out of range. Consumers only ever test it for truth, so
    // the difference is invisible in use — but a seam that quietly upgraded it to the house
    // convention would be lying about the API.
    g.set(
        "IsPetAttackActive",
        lua.create_function(move |lua, i: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(slot_at(&model, i).is_some_and(|s| s.view.attack_active))
        })?,
    )?;

    // CastPetAction(i) — queue the press. An EMPTY slot queues nothing: the reference's bar hides
    // an unnamed button, so a press on one can only come from the show-grid state, where it must
    // be inert.
    g.set(
        "CastPetAction",
        lua.create_function(|lua, i: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if slot_at(&model, i).is_some_and(|s| s.view.name.is_some()) {
                model.pet_actions_pressed.push(i);
            }
            Ok(())
        })?,
    )?;

    // TogglePetAutocast(i) — queue the right-click. Only a slot that CAN autocast queues: the
    // wire verb names a spell id, and a command token has none.
    g.set(
        "TogglePetAutocast",
        lua.create_function(|lua, i: u32| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if slot_at(&model, i).is_some_and(|s| s.view.autocast_allowed) {
                model.pet_autocast_toggles.push(i);
            }
            Ok(())
        })?,
    )?;

    // HasPetUI() → hasUI, isHunterPet — ALWAYS exactly two returns (`EAX=2` on every path of
    // `0x4be670`), with `(nil, nil)` when there is no pet with a UI. The second return is the
    // gate every stat binding below shares.
    g.set(
        "HasPetUI",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let bar = &model.pet_bar;
            Ok((flag(bar.has_ui), flag(bar.has_ui && bar.stats.hunter_pet)))
        })?,
    )?;

    // GetPetHappiness() → happiness, damagePercentage, loyaltyRate.
    //
    // The client thresholds; Lua does not. Return 1 is a PRE-BUCKETED 0..3 read off
    // PetPersonality.dbc's three columns, and `0` is a real bucket that answers the NUMBER 0 —
    // `PetFrame.lua` hides the icon on `not happiness`, and a 0 leaves it showing with whatever
    // texcoords it had. Only the gate failure is nil, and even then returns 2 and 3 are the
    // numbers (100.0, 0.0), never nil.
    g.set(
        "GetPetHappiness",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let s = &model.pet_bar.stats;
            let happiness = match s.happiness.filter(|_| s.hunter_pet) {
                Some(b) => Value::Integer(i64::from(b)),
                None => Value::Nil,
            };
            let (dmg, rate) = if happiness == Value::Nil {
                (100.0, 0.0)
            } else {
                (s.damage_percentage, s.loyalty_rate)
            };
            Ok(MultiValue::from_vec(vec![
                happiness,
                Value::Number(f64::from(dmg)),
                Value::Number(f64::from(rate)),
            ]))
        })?,
    )?;

    // GetPetLoyalty() → the localized level name, or NIL. The one stat binding whose failure is
    // nil rather than a number — level 0 (no loyalty yet) included.
    g.set(
        "GetPetLoyalty",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let s = &model.pet_bar.stats;
            Ok(match s.loyalty.as_deref().filter(|_| s.hunter_pet) {
                Some(name) => Value::String(lua.create_string(name)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetPetTrainingPoints() → totalPoints, spent — the two halves of one packed dword, HIGH word
    // first. Numbers on every path: a gate failure is (0, 0), not nil.
    g.set(
        "GetPetTrainingPoints",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let s = &model.pet_bar.stats;
            let (total, spent) = if s.hunter_pet {
                s.training_points
            } else {
                (0, 0)
            };
            Ok((f64::from(total), f64::from(spent)))
        })?,
    )?;

    // GetPetExperience() → currXP, nextXP. Numbers on every path, like the pair above; the client
    // converts both `fild qword` off a zero-extended dword, so neither can arrive negative.
    g.set(
        "GetPetExperience",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let s = &model.pet_bar.stats;
            let (cur, next) = if s.hunter_pet { s.experience } else { (0, 0) };
            Ok((f64::from(cur), f64::from(next)))
        })?,
    )?;

    // UnitCreatureFamily(unit) → the localized family word, or NIL. Exactly ONE return on every
    // path (`0x51a310`) — decision 1062.
    //
    // **Scoped to the `"pet"` token, and that narrowing is stated rather than hidden.** The real
    // binding resolves any unit and reads `[[unit+0xb30]+0x1c]` off its cached creature-query
    // record, so `UnitCreatureFamily("target")` on a wild boar answers "Boar" there and nil here —
    // INTERIM, exactly the shape `UnitDefense`'s non-player answer took in 1057. The pet page is
    // the only consumer in the shipped FrameXML, and a pet is the one unit whose record we cannot
    // reach through `guid::entry` (its guid slot holds a pet number, not a template id), so the pet
    // feed resolves it explicitly and the other tokens wait for a second consumer — at which point
    // this moves onto `UnitState` beside `creature_type_name`.
    //
    // Note there is NO class gate here: a warlock's imp answers "Imp". That is the shape, and it
    // is what makes the family word and `GetPetFoodTypes` below behave differently for the same
    // pet.
    //
    // All four of the reference's nil paths arrive as one pushed `None` — no cached record, id 0,
    // id past the table, and a null row (ids 10/13/14/18/22 are absent from the shipped file). A
    // missing or absent token is nil too, through the same match.
    g.set(
        "UnitCreatureFamily",
        lua.create_function(move |lua, token: Option<String>| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let family = token
                .filter(|t| t.eq_ignore_ascii_case("pet"))
                .and_then(|_| model.pet_bar.stats.family.as_deref());
            Ok(match family {
                Some(name) => Value::String(lua.create_string(name)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetPetIcon() → the pet's icon path, or NIL. One return. The stable window reads it both as a
    // predicate ("is there a current pet?", `PetStable.lua:51`) and as the texture itself (`161`),
    // so the nil must be a real nil rather than an empty string — `SetItemButtonTexture` would
    // happily take "" and a path resolving to nothing draws WHITE (decision 1046).
    g.set(
        "GetPetIcon",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(match model.pet_bar.stats.icon.as_deref() {
                Some(path) => Value::String(lua.create_string(path)?),
                None => Value::Nil,
            })
        })?,
    )?;

    // GetPetFoodTypes() → the diet names as VARARGS, one Lua return per food type, in record
    // order — the shape `BuildListString(GetPetFoodTypes())` needs (ref
    // `PetPaperDollFrame.xml:269`). `0x4bea10` returns the pushed COUNT and never a nil, so the
    // empty case is zero values.
    //
    // An empty diet returns NOTHING, not an empty string: the reference's `BuildListString` then
    // reads `arg[1]` as nil and returns nil, which is the behaviour a single empty-string return
    // would silently break. It is reachable two ways, both real — a family whose mask is 0 (every
    // warlock minion), and the binding's own `0x6116e0` hunter gate, which the app applies before
    // filling this list.
    g.set(
        "GetPetFoodTypes",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let values: Vec<Value> = model
                .pet_bar
                .stats
                .food_types
                .iter()
                .map(|f| lua.create_string(f).map(Value::String))
                .collect::<mlua::Result<_>>()?;
            Ok(MultiValue::from_vec(values))
        })?,
    )?;

    // PetStopAttack() — queue the call-off. No argument: the wire carries only the pet's guid.
    // The six one-shot orders (`0x4be450`…`0x4be4a0`): each writes the literal slot word the
    // bar's own reaction or command slot carries — `0x06000000`/1/2 PASSIVE/DEFENSIVE/AGGRESSIVE,
    // `0x07000000`/1/2 WAIT/FOLLOW/ATTACK — and re-enters `0x4bd1d0`, the identical call a
    // `CastPetAction` press
    // makes, PetAttack with the current selection's guid as a bar ATTACK click would. No
    // arguments, no returns; the no-pet gate is silent, so a session without a pet bar has
    // nothing to order and drops it, as the bar's own press would (1958; §10.9 confirmed the
    // reading, including that a target-less PetAttack raises the bar press's own `ERR_ATTACK_*`
    // after its auto-acquire — that leg is the drain's, shared with the press).
    for (name, packed) in [
        ("PetPassiveMode", order(PET_ACT_REACTION, PET_REACT_PASSIVE)),
        (
            "PetDefensiveMode",
            order(PET_ACT_REACTION, PET_REACT_DEFENSIVE),
        ),
        (
            "PetAggressiveMode",
            order(PET_ACT_REACTION, PET_REACT_AGGRESSIVE),
        ),
        ("PetWait", order(PET_ACT_COMMAND, PET_COMMAND_STAY)),
        ("PetFollow", order(PET_ACT_COMMAND, PET_COMMAND_FOLLOW)),
        ("PetAttack", order(PET_ACT_COMMAND, PET_COMMAND_ATTACK)),
    ] {
        g.set(
            name,
            lua.create_function(move |lua, ()| {
                let mut model = lua.app_data_mut::<Model>().expect("model app_data");
                if model.pet_bar.has_bar {
                    model.pet_orders.push(packed);
                }
                Ok(())
            })?,
        )?;
    }

    g.set(
        "PetStopAttack",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pet_stop_attacks += 1;
            Ok(())
        })?,
    )?;

    // ── The right-click menu (decision 1066) ─────────────────────────────────────────────────
    // Two predicates that decide what the PET menu SHOWS, and three verbs it can pick. The
    // predicates are 1/nil like the rest of this file, which is all `UnitPopup.lua` needs — every
    // one of its four uses is a bare `not PetCanBeAbandoned()` or an AND of the two.

    // PetCanBeAbandoned() → 1/nil. Do not read this as "may I abandon": it forks the whole menu.
    g.set(
        "PetCanBeAbandoned",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.pet_bar.can_be_abandoned))
        })?,
    )?;

    // PetCanBeRenamed() → 1/nil. Independent of the above; the rename row wants both.
    g.set(
        "PetCanBeRenamed",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.pet_bar.can_be_renamed))
        })?,
    )?;

    // PetAbandon() — the ABANDON_PET popup's OnAccept, i.e. the confirmed permanent one.
    g.set(
        "PetAbandon",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pet_abandons += 1;
            Ok(())
        })?,
    )?;

    // PetDismiss() — the menu row itself, with NO confirm in front of it (`UnitPopup.lua:590`
    // calls it directly, unlike abandon). Nothing is lost when a summon is sent away.
    g.set(
        "PetDismiss",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pet_dismisses += 1;
            Ok(())
        })?,
    )?;

    // PetRename(name) — the PETRENAMECONFIRM popup's OnAccept, carrying the text the RENAME_PET
    // edit box collected.
    //
    // The argument reaches `lua_tostring 0x6f3690`, which coerces a NUMBER via `0x6f7c80` —
    // `mlua::String` accepts one the same way. What the reference does with the RESULT is the
    // app's business and is queued for it rather than dropped here: an empty name raises
    // `ERR_NULL_PETNAME` and an over-long one is truncated, both at the send. Only a missing or
    // unconvertible argument dies here, because there is nothing to queue.
    g.set(
        "PetRename",
        lua.create_function(|lua, name: Option<mlua::String>| {
            let Some(name) = name else {
                return Ok(());
            };
            let name = name.to_str()?.to_string();
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pet_renames.push(name);
            Ok(())
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PetActionView, PetStats};
    use crate::script::UiScript;

    /// A hunter's bar, cut down to the three slot classes that matter: the Attack command (a
    /// token, currently attacking), Claw (a spell with autocast ON and a running cooldown), and an
    /// empty middle slot.
    fn slots() -> Vec<PetActionView> {
        vec![
            PetActionView {
                name: Some("PET_ACTION_ATTACK".into()),
                texture: Some("PET_ATTACK_TEXTURE".into()),
                is_token: true,
                active: true,
                attack_active: true,
                ..Default::default()
            },
            PetActionView {
                name: Some("Claw".into()),
                subtext: Some("Rank 3".into()),
                texture: Some("Interface\\Icons\\Ability_Druid_Rake".into()),
                autocast_allowed: true,
                autocast_enabled: true,
                cooldown: Some((9400, 1500, true)),
                ..Default::default()
            },
            PetActionView::default(),
        ]
    }

    #[test]
    fn slot_info_reads_and_out_of_range_is_one_nil() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return PetHasActionBar() == nil").unwrap());
        assert!(s.eval::<bool>("return GetPetActionInfo(1) == nil").unwrap());

        s.set_pet_actions(true, true, true, slots());
        assert_eq!(s.eval::<i64>("return PetHasActionBar()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetPetActionsUsable()").unwrap(), 1);

        // The token slot returns GLOBAL NAMES, and says so with isToken.
        let (name, subtext, texture, is_token, active, allowed, enabled) = s
            .eval::<(
                String,
                Option<String>,
                String,
                Option<i64>,
                Option<i64>,
                Option<i64>,
                Option<i64>,
            )>("return GetPetActionInfo(1)")
            .unwrap();
        assert_eq!(
            (
                name.as_str(),
                subtext,
                texture.as_str(),
                is_token,
                active,
                allowed,
                enabled
            ),
            (
                "PET_ACTION_ATTACK",
                None,
                "PET_ATTACK_TEXTURE",
                Some(1),
                Some(1),
                None,
                None
            )
        );

        // The spell slot returns a real name, a rank line and an icon PATH, and is not a token.
        assert!(s
            .eval::<bool>(
                "local n, sub, tex, tok, act, allow, on = GetPetActionInfo(2) \
                 return n == 'Claw' and sub == 'Rank 3' and tok == nil and act == nil \
                 and allow == 1 and on == 1 and string.find(tex, 'Icons') ~= nil"
            )
            .unwrap());

        // The empty slot exists (so it is not the out-of-range single nil) but has no name — the
        // reference's own "hide this button" test.
        assert!(s
            .eval::<bool>("local n, _, tex = GetPetActionInfo(3) return n == nil and tex == nil")
            .unwrap());
        assert!(s.eval::<bool>("return GetPetActionInfo(4) == nil").unwrap());
    }

    #[test]
    fn cooldown_triple_stamps_to_the_vm_clock_and_goes_cold() {
        let mut s = UiScript::new().unwrap();
        s.tick(10.0); // GetTime == 10
        s.set_pet_actions(true, true, true, slots());

        assert_eq!(
            s.eval::<(f64, f64, i32)>("return GetPetActionCooldown(1)")
                .unwrap(),
            (0.0, 0.0, 1),
            "no cooldown reads cold"
        );
        let (start, duration, enable) = s
            .eval::<(f64, f64, i32)>("return GetPetActionCooldown(2)")
            .unwrap();
        assert!((start - 9.4).abs() < 1e-9, "start {start}");
        assert!((duration - 1.5).abs() < 1e-9);
        assert_eq!(enable, 1);

        s.tick(2.0); // now == 12 > 9.4 + 1.5
        assert_eq!(
            s.eval::<(f64, f64, i32)>("return GetPetActionCooldown(2)")
                .unwrap(),
            (0.0, 0.0, 1)
        );
    }

    /// The three intent queues, and the two gates that keep a meaningless intent off the wire: an
    /// empty slot cannot be pressed, and a slot with no autocast cannot be toggled (its wire verb
    /// names a spell id, which a command token has not got).
    #[test]
    fn intents_queue_and_the_meaningless_ones_are_dropped() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, true, true, slots());

        s.run("CastPetAction(1) CastPetAction(3) CastPetAction(9)")
            .unwrap();
        assert_eq!(
            s.take_pet_actions(),
            vec![1],
            "empty + out-of-range dropped"
        );
        assert!(s.take_pet_actions().is_empty(), "drain empties");

        s.run("TogglePetAutocast(1) TogglePetAutocast(2)").unwrap();
        assert_eq!(
            s.take_pet_autocast_toggles(),
            vec![2],
            "only the autocastable slot"
        );

        assert_eq!(s.take_pet_stop_attacks(), 0);
        s.run("PetStopAttack() PetStopAttack()").unwrap();
        assert_eq!(s.take_pet_stop_attacks(), 2);
        assert_eq!(s.take_pet_stop_attacks(), 0, "drain empties");
    }

    /// `IsPetAttackActive` is per-slot, and it is what turns the Attack button's second press into
    /// a call-off — the reference's `PetActionButton_OnClick` fork.
    ///
    /// It answers a **boolean** on every path, including out of range — the one binding here that
    /// does not use the 1/nil convention.
    #[test]
    fn attack_active_is_a_per_slot_boolean() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, true, true, slots());
        assert!(s.eval::<bool>("return IsPetAttackActive(1)").unwrap());
        assert!(!s.eval::<bool>("return IsPetAttackActive(2)").unwrap());
        assert!(s
            .eval::<bool>("return IsPetAttackActive(9) == false")
            .unwrap());
    }

    /// The right-click menu's two predicates and three verbs (decision 1066).
    ///
    /// The predicate half is checked the way `UnitPopup.lua` actually reads them — as the four
    /// row conditions — because that is the only thing they are for, and getting the Dismiss row's
    /// **inverted** sense wrong would show a hunter both Abandon and Dismiss.
    #[test]
    fn the_menu_predicates_fork_the_rows_and_the_verbs_queue() {
        let mut s = UiScript::new().unwrap();

        // No pet pushed: every row is off, which is what keeps a menu of dead rows from opening.
        assert!(s
            .eval::<bool>("return PetCanBeAbandoned() == nil and PetCanBeRenamed() == nil")
            .unwrap());

        // A hunter's freshly tamed pet: abandon/rename/paperdoll show, dismiss hides.
        s.set_pet_menu(true, true);
        assert_eq!(s.eval::<i64>("return PetCanBeAbandoned()").unwrap(), 1);
        assert!(
            s.eval::<bool>("return PetCanBeAbandoned() and PetCanBeRenamed()")
                .unwrap(),
            "the rename row wants BOTH"
        );
        assert!(!s.eval::<bool>("return not PetCanBeAbandoned()").unwrap());

        // The same pet after one rename — the bit is one-shot, so only the rename row goes.
        s.set_pet_menu(true, false);
        assert!(s.eval::<bool>("return PetCanBeAbandoned() ~= nil").unwrap());
        assert!(s.eval::<bool>("return PetCanBeRenamed() == nil").unwrap());

        // A warlock's demon: the fork flips whole. Dismiss is the row that shows.
        s.set_pet_menu(false, false);
        assert!(s.eval::<bool>("return not PetCanBeAbandoned()").unwrap());

        // The verbs. Abandon and dismiss are counted apart; rename carries its text.
        assert_eq!(s.take_pet_gives_up(), (0, 0));
        s.run("PetAbandon() PetDismiss() PetDismiss()").unwrap();
        assert_eq!(s.take_pet_gives_up(), (1, 2));
        assert_eq!(s.take_pet_gives_up(), (0, 0), "drain empties");

        // An empty name IS queued — the reference's empty check raises `ERR_NULL_PETNAME` at the
        // send, so swallowing it here would swallow the error with it. A number coerces
        // (`lua_tostring`); a missing argument has nothing to queue.
        s.run("PetRename(\"Bruce\") PetRename(\"\") PetRename(7) PetRename()")
            .unwrap();
        assert_eq!(
            s.take_pet_renames(),
            vec!["Bruce".to_string(), String::new(), "7".to_string()]
        );
        assert!(s.take_pet_renames().is_empty(), "drain empties");
    }

    /// A bar push must not wipe the state that rides other clocks — the trap the old wholesale
    /// `pet_bar = PetBarState { .. }` assignment set, and the reason [`UiScript::set_pet_actions`]
    /// assigns field by field.
    #[test]
    fn pushing_the_bar_leaves_the_stats_and_the_menu_alone() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_menu(true, true);
        s.set_pet_stats(
            true,
            PetStats {
                hunter_pet: true,
                happiness: Some(3),
                ..Default::default()
            },
        );

        s.set_pet_actions(true, true, true, slots());

        assert_eq!(s.eval::<i64>("return PetCanBeAbandoned()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return PetCanBeRenamed()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return HasPetUI()").unwrap(), 1);
        assert_eq!(s.eval::<i64>("return GetPetHappiness()").unwrap(), 3);
    }

    /// A disabled bar still EXISTS — `PetHasActionBar` stays true while `GetPetActionsUsable`
    /// goes false. The pair is what greys every icon without taking the bar off screen.
    #[test]
    fn a_disabled_bar_is_still_a_bar() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, false, true, slots());
        assert_eq!(s.eval::<i64>("return PetHasActionBar()").unwrap(), 1);
        assert!(s
            .eval::<bool>("return GetPetActionsUsable() == nil")
            .unwrap());
    }

    /// `GetPetIcon` — one return, a real nil when there is no pet. The stable window reads it BOTH
    /// as a predicate and as the texture it passes to `SetItemButtonTexture`, so an empty string
    /// standing in for the nil would draw a white square rather than the empty-slot art
    /// (decision 1046's law, decision 1676's consumer).
    /// The six one-shot orders synthesize the bar's own slot words (1958): the three reactions
    /// as type 6 with the react byte, the three commands as type 7 with the command byte — the
    /// same words `CastPetAction` on those slots would send. Without a bar there is nothing to
    /// order, and none is queued.
    #[test]
    fn the_pet_one_shots_synthesize_the_bars_slot_words() {
        let mut s = UiScript::new().unwrap();
        s.run("PetAttack(); PetFollow(); PetWait(); PetPassiveMode()")
            .unwrap();
        assert!(s.take_pet_orders().is_empty(), "no bar, no order");
        s.set_pet_actions(true, true, true, slots());
        s.run(
            "PetPassiveMode(); PetDefensiveMode(); PetAggressiveMode(); PetWait(); PetFollow(); \
             PetAttack()",
        )
        .unwrap();
        assert_eq!(
            s.take_pet_orders(),
            vec![
                0x0600_0000,
                0x0600_0001,
                0x0600_0002,
                0x0700_0000,
                0x0700_0001,
                0x0700_0002
            ]
        );
        assert!(s.take_pet_orders().is_empty(), "drained");
    }

    /// `HasFullControl()` is the host's control flag — 1 at boot, nil while a possession or fear
    /// holds the body, 1 again after (1958).
    #[test]
    fn has_full_control_follows_the_hosts_flag() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return HasFullControl()").unwrap(), 1);
        s.set_player_control(false);
        assert!(s.eval::<bool>("return HasFullControl() == nil").unwrap());
        s.set_player_control(true);
        assert_eq!(s.eval::<i64>("return HasFullControl()").unwrap(), 1);
    }

    #[test]
    fn the_pet_icon_answers_a_path_or_a_real_nil() {
        let mut s = UiScript::new().unwrap();
        assert!(s.eval::<bool>("return GetPetIcon() == nil").unwrap());

        s.set_pet_stats(true, hunter_stats());
        assert_eq!(
            s.eval::<String>("return GetPetIcon()").unwrap(),
            "Interface\\Icons\\Ability_Hunter_Pet_Boar"
        );

        // Exactly one return — the reference assigns it straight into a single local.
        assert_eq!(s.arity("GetPetIcon()").unwrap(), 1);
    }

    fn hunter_stats() -> PetStats {
        PetStats {
            hunter_pet: true,
            icon: Some("Interface\\Icons\\Ability_Hunter_Pet_Boar".into()),
            happiness: Some(3),
            damage_percentage: 125.0,
            loyalty_rate: 20.0,
            loyalty: Some("(Loyalty Level 6) Best Friend".into()),
            training_points: (170, 130),
            experience: (4200, 8000),
            family: Some("Boar".into()),
            food_types: vec![
                "Meat".into(),
                "Fish".into(),
                "Cheese".into(),
                "Bread".into(),
                "Fungus".into(),
                "Fruit".into(),
            ],
        }
    }

    /// The four stat bindings on a hunter's pet — arities and values, including the pair ORDER
    /// (`total` before `spent`, `curr` before `next`), which is the half a swap makes invisible.
    #[test]
    fn a_hunters_pet_answers_every_stat_binding() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_stats(true, hunter_stats());

        assert!(s
            .eval::<bool>(
                "local h, dmg, rate = GetPetHappiness() \
                 return h == 3 and dmg == 125 and rate == 20"
            )
            .unwrap());
        assert_eq!(
            s.eval::<String>("return GetPetLoyalty()").unwrap(),
            "(Loyalty Level 6) Best Friend",
            "the shipped prefix is pushed verbatim — the client does no stripping"
        );
        assert!(s
            .eval::<bool>("local t, sp = GetPetTrainingPoints() return t == 170 and sp == 130")
            .unwrap());
        assert!(s
            .eval::<bool>("local c, n = GetPetExperience() return c == 4200 and n == 8000")
            .unwrap());
        assert!(s
            .eval::<bool>("local ui, hunter = HasPetUI() return ui == 1 and hunter == 1")
            .unwrap());
    }

    /// **The failure conventions are three different shapes and the API is those shapes.** A
    /// warlock's imp has a pet UI and is not a hunter pet: loyalty goes nil, both pairs go to
    /// `(0, 0)` as NUMBERS, and happiness goes `(nil, 100, 0)` — nil in the first slot only.
    ///
    /// Flattening any of these to nil-everything breaks a different consumer: `PetFrame.lua` hides
    /// its happiness icon on `not happiness`, but `PetPaperDollFrame` does arithmetic on the pairs
    /// and would error on a nil.
    #[test]
    fn a_non_hunter_pet_fails_three_different_ways() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_stats(
            true,
            PetStats {
                hunter_pet: false,
                ..hunter_stats()
            },
        );

        assert!(
            s.eval::<bool>(
                "local h, dmg, rate = GetPetHappiness() \
                 return h == nil and dmg == 100 and rate == 0"
            )
            .unwrap(),
            "happiness is nil but its two numbers are still numbers"
        );
        assert!(s.eval::<bool>("return GetPetLoyalty() == nil").unwrap());
        assert!(s
            .eval::<bool>("local t, sp = GetPetTrainingPoints() return t == 0 and sp == 0")
            .unwrap());
        assert!(s
            .eval::<bool>("local c, n = GetPetExperience() return c == 0 and n == 0")
            .unwrap());
        // HasPetUI still reports the UI — it is the SECOND return that discriminates.
        assert!(s
            .eval::<bool>("local ui, hunter = HasPetUI() return ui == 1 and hunter == nil")
            .unwrap());
    }

    /// **Bucket 0 is a number, not nil** (`GetPetHappiness 0x4be900`): the shipped `PetFrame.lua`
    /// branches on 1/2/3 and hides only on `not happiness`, so a 0 must leave the icon up.
    /// Collapsing it into the failure path hides a frame the reference shows.
    #[test]
    fn happiness_bucket_zero_is_not_the_failure_case() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_stats(
            true,
            PetStats {
                happiness: Some(0),
                damage_percentage: 100.0,
                loyalty_rate: 0.0,
                ..hunter_stats()
            },
        );
        assert!(s
            .eval::<bool>("local h = GetPetHappiness() return h == 0 and h ~= nil")
            .unwrap());
        assert!(
            s.eval::<bool>("return not GetPetHappiness() == false")
                .unwrap(),
            "0 is truthy in Lua, so the reference's `not happiness` hide-test does NOT fire"
        );
    }

    /// With no pet at all every binding takes its failure path and `HasPetUI` answers `(nil, nil)`
    /// — two returns on every path, never zero.
    #[test]
    fn no_pet_still_answers_two_values_from_has_pet_ui() {
        let s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("local ui, hunter = HasPetUI() return ui == nil and hunter == nil")
            .unwrap());
        assert!(s.eval::<bool>("return GetPetLoyalty() == nil").unwrap());
        assert!(s
            .eval::<bool>("local h, dmg = GetPetHappiness() return h == nil and dmg == 100")
            .unwrap());
    }

    /// **`UnitCreatureFamily`'s nil paths — all four of them** (decision 1062). The reference
    /// guards its whole level-line `SetText` on this binding, so an accidental `""` in place of
    /// nil would print a bare "Level 58 " with a trailing space instead of nothing at all.
    #[test]
    fn unit_creature_family_is_nil_on_every_absent_path() {
        let mut s = UiScript::new().unwrap();
        // 1. No pet at all — nothing has ever been pushed.
        assert!(s
            .eval::<bool>(r#"return UnitCreatureFamily("pet") == nil"#)
            .unwrap());

        // 2. A pet whose template carries family 0, and 3. a pet whose creature query has not
        //    answered yet. Both arrive here as the same pushed `None` (the app resolves which is
        //    which); what matters at this seam is that a live pet with no family word is nil and
        //    not an empty string.
        s.set_pet_stats(
            true,
            PetStats {
                family: None,
                ..hunter_stats()
            },
        );
        assert!(s
            .eval::<bool>(r#"return UnitCreatureFamily("pet") == nil"#)
            .unwrap());
        assert!(
            s.eval::<bool>(r#"return UnitCreatureFamily("pet") ~= ''"#)
                .unwrap(),
            "nil, never an empty string — '' is TRUTHY in Lua, so it would pass the ref's guard \
             and print a bare 'Level 58 ' with a trailing space"
        );

        // 4. Any other token: the INTERIM narrowing, stated in the binding's own comment.
        s.set_pet_stats(true, hunter_stats());
        assert_eq!(
            s.eval::<String>(r#"return UnitCreatureFamily("pet")"#)
                .unwrap(),
            "Boar"
        );
        for token in [r#""target""#, r#""player""#, "nil"] {
            assert!(
                s.eval::<bool>(&format!("return UnitCreatureFamily({token}) == nil"))
                    .unwrap(),
                "{token} must answer nil"
            );
        }
    }

    /// `GetPetFoodTypes` returns **varargs**, one value per diet — the shape
    /// `BuildListString(GetPetFoodTypes())` depends on. A single comma-joined string would read
    /// identically in the tooltip and be wrong for every other caller.
    #[test]
    fn get_pet_food_types_returns_one_value_per_diet() {
        let mut s = UiScript::new().unwrap();
        // No pet: ZERO returns, which is what makes the ref's `BuildListString` answer nil.
        assert_eq!(s.arity("GetPetFoodTypes()").unwrap(), 0);

        s.set_pet_stats(true, hunter_stats());
        assert_eq!(
            s.arity("GetPetFoodTypes()").unwrap(),
            6,
            "a boar's six diets are six returns, not one string"
        );
        assert!(s
            .eval::<bool>(
                "local a, b, c = GetPetFoodTypes() \
                 return a == 'Meat' and b == 'Fish' and c == 'Cheese'"
            )
            .unwrap());

        // An empty diet is a real answer (every warlock family ships a zero food mask) and is
        // still zero returns, not one empty string.
        s.set_pet_stats(
            true,
            PetStats {
                food_types: vec![],
                ..hunter_stats()
            },
        );
        assert_eq!(s.arity("GetPetFoodTypes()").unwrap(), 0);
    }

    /// The family **word** answers regardless of the hunter gate — `UnitCreatureFamily 0x51a310`
    /// has no class test at all, so a warlock's minion shows "Imp" on the page's level line while
    /// every hunter-gated binding beside it says nothing.
    ///
    /// (The **diet** is the other way round — it shares `0x6116e0` — but that gate is the app's to
    /// apply, so at this seam it is simply an empty list. `ui_pet_stats` pins the gate itself.)
    #[test]
    fn the_family_word_answers_for_a_non_hunter_pet() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_stats(
            true,
            PetStats {
                hunter_pet: false,
                family: Some("Imp".into()),
                food_types: vec![],
                ..PetStats::default()
            },
        );
        assert_eq!(
            s.eval::<String>(r#"return UnitCreatureFamily("pet")"#)
                .unwrap(),
            "Imp"
        );
        // …while every hunter-gated binding still says nothing.
        assert!(s.eval::<bool>("return GetPetLoyalty() == nil").unwrap());
        assert_eq!(s.arity("GetPetFoodTypes()").unwrap(), 0);
    }
}
