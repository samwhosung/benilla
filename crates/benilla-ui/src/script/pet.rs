//! The pet bindings: the action bar `PetActionBarFrame.lua` drives, the stat block the pet frames
//! read, and the pet menu's predicates and verbs. The app resolves every slot and pushes it
//! ([`super::UiScript::set_pet_actions`]); presses queue back out for the app to send. A token
//! slot's name and texture are the names of globals, which the stock Lua resolves
//! (`PetActionBarFrame.lua:98-104`). Booleans are 1/nil and cooldowns go cold once elapsed, as in
//! [`super::action`].

use mlua::{Lua, MultiValue, Value};

use super::Model;

// The slot-word values the one-shot orders synthesize, restated from
// `benilla_protocol::messages::pet` because this crate does not depend on the protocol.
const PET_ACT_COMMAND: u8 = 0x07;
const PET_ACT_REACTION: u8 = 0x06;
const PET_COMMAND_STAY: u32 = 0;
const PET_COMMAND_FOLLOW: u32 = 1;
const PET_COMMAND_ATTACK: u32 = 2;
const PET_REACT_PASSIVE: u32 = 0;
const PET_REACT_DEFENSIVE: u32 = 1;
const PET_REACT_AGGRESSIVE: u32 = 2;

/// A slot word as the server packs it: the type in the top byte, the action in the low bits.
const fn order(kind: u8, action: u32) -> u32 {
    (kind as u32) << 24 | action
}

/// One pet bar slot, resolved by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetActionView {
    /// `GetPetActionInfo`'s first return and the occupancy test: `None` hides the button
    /// (`PetActionBarFrame.lua:122-128`). The spell's name, or for a token the name of a global.
    pub name: Option<String>,
    /// The second return: the spell's rank line, `None` for a token.
    pub subtext: Option<String>,
    /// The third return: an icon path, or for a token the name of a global.
    pub texture: Option<String>,
    /// A command or reaction token, whose `name` and `texture` are global names.
    pub is_token: bool,
    /// The slot's spell, for `GameTooltip:SetPetAction`; not a `GetPetActionInfo` return.
    pub spell_id: Option<u32>,
    /// The checked ring.
    pub active: bool,
    /// The slot can autocast: the static `UI-AutoCastableOverlay` ring.
    pub autocast_allowed: bool,
    /// Autocast is on: the sparkle trail.
    pub autocast_enabled: bool,
    /// `IsPetAttackActive`: a left click calls the pet off instead of running the slot.
    pub attack_active: bool,
    /// `(start_ms on the GetTime clock, duration_ms, enabled)`, as [`super::action::ActionState`].
    pub cooldown: Option<(i64, u32, bool)>,
    /// The slot's packed word, verbatim, for the drag ([`super::cursor::pet`]): the reference
    /// compares occupants under `& 0x3FFFFFFF` and writes the source word through unchanged
    /// (`0x4bc9a0`, `0x4bce00`). `0` is the empty slot.
    pub packed: u32,
    /// `SPELL_ATTR_PASSIVE` (`Attributes & 0x40`) on a spell slot: the drop silently refuses a
    /// passive source (`0x4bc9f8`-`0x4bca2e`).
    pub passive: bool,
}

/// [`PetActionView`] as stored, its cooldown converted to `GetTime` seconds at push time.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StoredPetAction {
    pub(crate) view: PetActionView,
    pub(crate) cooldown: Option<(f64, f64, bool)>,
}

/// The pet stat block: the four hunter stat bindings and `HasPetUI`'s second return
/// (`0x4be670`), plus the family word, icon and diet. The stats share one hunter-pet gate
/// (`0x6116e0`) and fail their own ways: loyalty to nil, the pairs to `(0, 0)`, happiness to
/// `(nil, 100.0, 0.0)`. The family word has no gate (`0x51a310`); the diet shares the stats'.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetStats {
    /// `HasPetUI`'s second return; false makes every gated binding answer its failure form.
    pub hunter_pet: bool,
    /// `GetPetHappiness`'s first return, bucketed `0..=3` by the client; `Some(0)` is not nil.
    pub happiness: Option<u32>,
    /// Return 2: the damage percentage, already scaled by 100.
    pub damage_percentage: f32,
    /// Return 3: the loyalty rate, unscaled and possibly negative.
    pub loyalty_rate: f32,
    /// `GetPetLoyalty`: the `PetLoyalty.dbc` name verbatim, `"(Loyalty Level N) "` prefix included.
    pub loyalty: Option<String>,
    /// `GetPetTrainingPoints`: `(totalPoints, spent)`, high word first.
    pub training_points: (u16, u16),
    /// `GetPetExperience`: `(currXP, nextXP)`.
    pub experience: (u32, u32),
    /// `UnitCreatureFamily("pet")`: the localized `CreatureFamily.dbc` word off the cached creature
    /// record (`0x51a310`). `None` is nil: no record yet, id 0, or an id with no row (10, 13, 14,
    /// 18 and 22 have none). `PetPaperDollFrame.lua:68-70` guards its level line on it.
    pub family: Option<String>,
    /// `GetPetIcon()`: the family row's `CreatureFamily.dbc` icon, `None` on the family word's
    /// misses. Ungated like the family word; whether the reference gates it (`0x4beb10`) is
    /// untraced, and no warlock reaches its stock callers (`PetStable.lua:51`, `161-162`).
    pub icon: Option<String>,
    /// `GetPetFoodTypes()`: the diet names the family's food mask selects, in record order
    /// (`0x4bea10`: bit `1 << (id - 1)` of `CreatureFamily` column 7, names from `ItemPetFood`).
    /// Empty for a zero mask (every warlock minion) or a failed hunter gate, which the app applies.
    pub food_types: Vec<String>,
}

/// The pushed pet state: the bar's slots and bar-wide bits, the stat block, the menu predicates.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PetBarState {
    /// `PetHasActionBar()`: there is a bar, even one of ten empty slots.
    pub(crate) has_bar: bool,
    /// `GetPetActionsUsable()`: false desaturates the whole bar.
    pub(crate) actions_usable: bool,
    pub(crate) slots: Vec<StoredPetAction>,
    /// `HasPetUI`'s first return: a pet with a nonzero `UNIT_FIELD_PETNUMBER` (`0x4be697`). Not
    /// `has_bar`, whose gate is the cached guid alone.
    pub(crate) has_ui: bool,
    pub(crate) stats: PetStats,
    /// `PickupPetAction`'s gate, `UNIT_FLAG_POSSESSED` clear (`0x4be1c1`), which blocks the drop
    /// as well as the pick-up. Not `actions_usable`: possession does not grey the bar, and the
    /// flags that grey it do not block a drag.
    pub(crate) pickup_allowed: bool,
    /// `PetCanBeAbandoned()`: a kept pet rather than a summon. It forks the pet menu: paperdoll,
    /// rename and abandon show when true, dismiss only when false (`UnitPopup.lua:402-417`).
    pub(crate) can_be_abandoned: bool,
    /// `PetCanBeRenamed()`, ANDed with the above for the rename row; set until the first rename.
    pub(crate) can_be_renamed: bool,
}

impl super::UiScript {
    /// Push the whole pet bar; the app diffs and fires `PET_BAR_UPDATE`.
    pub fn set_pet_actions(
        &mut self,
        has_bar: bool,
        actions_usable: bool,
        pickup_allowed: bool,
        slots: Vec<PetActionView>,
    ) {
        // Field by field: the stat block and the menu predicates beside them move on other clocks.
        let bar = &mut self.model_mut().pet_bar;
        bar.has_bar = has_bar;
        bar.actions_usable = actions_usable;
        bar.pickup_allowed = pickup_allowed;
        bar.slots = slots
            .into_iter()
            .map(|view| {
                // The start is already on the `GetTime` clock, in ms.
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

    /// Push the stat block and `HasPetUI`'s first return, which move on descriptor updates rather
    /// than with the bar.
    pub fn set_pet_stats(&mut self, has_ui: bool, stats: PetStats) {
        let bar = &mut self.model_mut().pet_bar;
        bar.has_ui = has_ui;
        bar.stats = stats;
    }

    /// Push the menu's two predicates, which move with the pet's `UNIT_FIELD_FLAGS`.
    pub fn set_pet_menu(&mut self, can_be_abandoned: bool, can_be_renamed: bool) {
        let bar = &mut self.model_mut().pet_bar;
        bar.can_be_abandoned = can_be_abandoned;
        bar.can_be_renamed = can_be_renamed;
    }

    /// Drain the 1-based slots `CastPetAction` queued; the app decides what each one sends.
    pub fn take_pet_actions(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().pet_actions_pressed)
    }

    /// Drain the 1-based slot indices `TogglePetAutocast` queued.
    pub fn take_pet_autocast_toggles(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().pet_autocast_toggles)
    }

    /// Drain the count of `PetStopAttack()` calls.
    pub fn take_pet_stop_attacks(&mut self) -> u32 {
        std::mem::replace(&mut self.model_mut().pet_stop_attacks, 0)
    }

    /// Drain the one-shot orders (`PetAttack` and the rest), each a packed slot word.
    pub fn take_pet_orders(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().pet_orders)
    }

    /// Set the flag `HasFullControl` answers.
    pub fn set_player_control(&mut self, in_control: bool) {
        self.model_mut().player_control = in_control;
    }

    /// Drain the drag's bar writes, already applied here: one `Vec` per `CMSG_PET_SET_ACTION` of
    /// one or two `(0-based position, packed word)` pairs. Send each whole: the server tells the
    /// two forms apart by body size.
    pub fn take_pet_set_actions(&mut self) -> Vec<Vec<(u32, u32)>> {
        std::mem::take(&mut self.model_mut().pet_set_actions)
    }

    /// Drain the `PetAbandon()` and `PetDismiss()` call counts, as `(abandons, dismisses)`.
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

/// The stored slot at a 1-based button index.
fn slot_at(model: &Model, i: u32) -> Option<&StoredPetAction> {
    usize::try_from(i.checked_sub(1)?)
        .ok()
        .and_then(|n| model.pet_bar.slots.get(n))
}

/// Register the pet-bar globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    let flag = |b: bool| if b { Value::Integer(1) } else { Value::Nil };

    g.set(
        "PetHasActionBar",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.pet_bar.has_bar))
        })?,
    )?;

    g.set(
        "GetPetActionsUsable",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.pet_bar.actions_usable))
        })?,
    )?;

    // An out-of-range index answers a single nil, which the Lua reads as an empty slot.
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

    // An elapsed or absent cooldown answers (0, 0, 1), so a re-feed never replays the sweep.
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

    // The one pet binding that answers a real boolean (`0x6f39f0`), `false` even out of range.
    g.set(
        "IsPetAttackActive",
        lua.create_function(move |lua, i: u32| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(slot_at(&model, i).is_some_and(|s| s.view.attack_active))
        })?,
    )?;

    // An empty slot, whose button shows only under show-grid, queues nothing; the reference sends
    // its zero word (`0x4bd230` falls to the send at `0x4bd444`), which vmangos ignores
    // (`PetHandler.cpp:178`).
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

    // Only a slot that can autocast queues: the wire verb names a spell, and a token has none.
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

    // Two returns on every path (`0x4be670`), `(nil, nil)` with no pet UI.
    g.set(
        "HasPetUI",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let bar = &model.pet_bar;
            Ok((flag(bar.has_ui), flag(bar.has_ui && bar.stats.hunter_pet)))
        })?,
    )?;

    // The client buckets return 1 off `PetPersonality.dbc` (`0x4be900`), and bucket 0 is the
    // number 0; only the gate failure is nil, and even then returns 2 and 3 are `(100.0, 0.0)`.
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

    // The one stat binding that fails to nil, level 0 included.
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

    // The high then low word of one packed dword; a gate failure is (0, 0), not nil.
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

    // Numbers on every path; the client reads both unsigned, so neither is negative.
    g.set(
        "GetPetExperience",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let s = &model.pet_bar.stats;
            let (cur, next) = if s.hunter_pet { s.experience } else { (0, 0) };
            Ok((f64::from(cur), f64::from(next)))
        })?,
    )?;

    // One return on every path, with no class gate (`0x51a310`). Answers the `"pet"` token only:
    // the reference resolves any unit off its cached creature record (`[[unit+0xb30]+0x1c]`), so
    // a wild boar's `"target"` answers "Boar" there and nil here. The pet page is the only stock
    // caller.
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

    // A real nil, never "": the stable reads it as a predicate and as a texture
    // (`PetStable.lua:51`, `161-162`), and an empty path would draw white.
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

    // One return per diet and never a nil (`0x4bea10`): an empty diet is zero values, for which
    // `BuildListString` answers nil (`PetPaperDollFrame.xml:269`).
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

    // The six one-shot orders (`0x4be450`..`0x4be4a0`) each write the slot word the bar's own
    // reaction or command slot carries and re-enter `0x4bd1d0`, as a `CastPetAction` press does;
    // PetAttack takes the current selection. With no pet bar the order is silently dropped.
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

    // ── The pet menu ─────────────────────────────────────────────────────────

    g.set(
        "PetCanBeAbandoned",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.pet_bar.can_be_abandoned))
        })?,
    )?;

    g.set(
        "PetCanBeRenamed",
        lua.create_function(move |lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(model.pet_bar.can_be_renamed))
        })?,
    )?;

    // The `ABANDON_PET` popup's OnAccept, after its confirm.
    g.set(
        "PetAbandon",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pet_abandons += 1;
            Ok(())
        })?,
    )?;

    // The menu row itself, with no confirm (`UnitPopup.lua:591`).
    g.set(
        "PetDismiss",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            model.pet_dismisses += 1;
            Ok(())
        })?,
    )?;

    // A number coerces, as `lua_tostring` does (`0x6f3690` → `0x6f7c80`); an empty or over-long
    // name is still queued, since the reference raises `ERR_NULL_PETNAME` or truncates at send.
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

    /// Attack (a token, attacking), Claw (a spell, autocasting, cooling down), an empty slot.
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

        assert!(s
            .eval::<bool>(
                "local n, sub, tex, tok, act, allow, on = GetPetActionInfo(2) \
                 return n == 'Claw' and sub == 'Rank 3' and tok == nil and act == nil \
                 and allow == 1 and on == 1 and string.find(tex, 'Icons') ~= nil"
            )
            .unwrap());

        // The empty slot has returns, unlike the out-of-range single nil, but no name.
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

    /// The predicates are checked the way `UnitPopup.lua`'s row conditions read them.
    #[test]
    fn the_menu_predicates_fork_the_rows_and_the_verbs_queue() {
        let mut s = UiScript::new().unwrap();

        // No pet pushed: every row is off.
        assert!(s
            .eval::<bool>("return PetCanBeAbandoned() == nil and PetCanBeRenamed() == nil")
            .unwrap());

        // A hunter's freshly tamed pet.
        s.set_pet_menu(true, true);
        assert_eq!(s.eval::<i64>("return PetCanBeAbandoned()").unwrap(), 1);
        assert!(
            s.eval::<bool>("return PetCanBeAbandoned() and PetCanBeRenamed()")
                .unwrap(),
            "the rename row wants BOTH"
        );
        assert!(!s.eval::<bool>("return not PetCanBeAbandoned()").unwrap());

        // The same pet after one rename.
        s.set_pet_menu(true, false);
        assert!(s.eval::<bool>("return PetCanBeAbandoned() ~= nil").unwrap());
        assert!(s.eval::<bool>("return PetCanBeRenamed() == nil").unwrap());

        // A warlock's demon: only Dismiss shows.
        s.set_pet_menu(false, false);
        assert!(s.eval::<bool>("return not PetCanBeAbandoned()").unwrap());

        assert_eq!(s.take_pet_gives_up(), (0, 0));
        s.run("PetAbandon() PetDismiss() PetDismiss()").unwrap();
        assert_eq!(s.take_pet_gives_up(), (1, 2));
        assert_eq!(s.take_pet_gives_up(), (0, 0), "drain empties");

        // An empty name is queued for the send's error, a number coerces, no argument queues none.
        s.run("PetRename(\"Bruce\") PetRename(\"\") PetRename(7) PetRename()")
            .unwrap();
        assert_eq!(
            s.take_pet_renames(),
            vec!["Bruce".to_string(), String::new(), "7".to_string()]
        );
        assert!(s.take_pet_renames().is_empty(), "drain empties");
    }

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

    #[test]
    fn a_disabled_bar_is_still_a_bar() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_actions(true, false, true, slots());
        assert_eq!(s.eval::<i64>("return PetHasActionBar()").unwrap(), 1);
        assert!(s
            .eval::<bool>("return GetPetActionsUsable() == nil")
            .unwrap());
    }

    /// Reactions are type 6 and commands type 7, each with its state in the low byte.
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
        assert!(s
            .eval::<bool>("local ui, hunter = HasPetUI() return ui == 1 and hunter == nil")
            .unwrap());
    }

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

    #[test]
    fn unit_creature_family_is_nil_on_every_absent_path() {
        let mut s = UiScript::new().unwrap();
        // No pet pushed.
        assert!(s
            .eval::<bool>(r#"return UnitCreatureFamily("pet") == nil"#)
            .unwrap());

        // Family 0 and an unanswered creature query both arrive as `None`.
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

        // With a family: `"pet"` answers it, any other token nil.
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

    #[test]
    fn get_pet_food_types_returns_one_value_per_diet() {
        let mut s = UiScript::new().unwrap();
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

        s.set_pet_stats(
            true,
            PetStats {
                food_types: vec![],
                ..hunter_stats()
            },
        );
        assert_eq!(s.arity("GetPetFoodTypes()").unwrap(), 0);
    }

    /// The diet's hunter gate is the app's to apply, so here a non-hunter's diet is just empty.
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
        assert!(s.eval::<bool>("return GetPetLoyalty() == nil").unwrap());
        assert_eq!(s.arity("GetPetFoodTypes()").unwrap(), 0);
    }
}
