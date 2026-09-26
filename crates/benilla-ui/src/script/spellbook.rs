//! The spellbook bindings, over a book the app pushes (tabs and a flat slot list, already resolved
//! to name, rank, icon and passive); `CastSpell` and `PickupSpell` queue intents the app drains.
//!
//! FrameXML passes every binding a 1-based book id (`SpellBook_GetSpellID`), and the reference's
//! marshaller (`0x4b3ec0`) subtracts 1, as [`slot_index`] does. `BOOKTYPE_PET` selects the pet's
//! slot list, fed from `SMSG_PET_SPELLS`, where `PickupSpell` and `CastSpell` produce a pet action
//! word and a `CMSG_PET_ACTION` instead.
//!
//! Deviation: `BOOKTYPE_SPELL`/`BOOKTYPE_PET` are engine globals here, where the reference defines
//! them in `SpellBookFrame.lua:5-6`, so this module's tests run without the stock file.

use mlua::{Lua, MultiValue, Value};

use super::binding_abi::flag;
use super::cursor::{queue_cursor_update, CursorPayload, CursorSpell};
use super::Model;

const BOOKTYPE_SPELL: &str = "spell";
const BOOKTYPE_PET: &str = "pet";

/// `HasPetSpells`' second return when no token is resolved: the reference's literal (`0x846a40`),
/// pushed at `0x4b44a6` when the player object does not resolve. FrameXML concatenates it, so
/// never nil.
const PET_TOKEN_FALLBACK: &str = "PET";

/// One skill-line tab as `GetSpellTabInfo` returns it; `offset` is the tab's 0-based start in
/// [`SpellBookState::slots`], computed by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellTabView {
    pub name: String,
    pub texture: Option<String>,
    pub offset: u32,
    pub num_spells: u32,
}

/// One spell in the flat book, every field resolved by the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellSlotView {
    pub spell_id: u32,
    pub name: String,
    /// The rank line, `Spell.dbc`'s `NameSubtext`; `None` shows no second line.
    pub rank: Option<String>,
    pub texture: Option<String>,
    /// `SPELL_ATTR_PASSIVE`: grays the name and refuses `CastSpell` and `CastSpellByName`, never
    /// a pickup (the reference does not block one).
    pub passive: bool,
    /// `IsCurrentCast`, the checked ring: the reference (`0x4b3600`) answers it only for a
    /// shapeshift into the player's current form or the open trade-skill window's spell, never an
    /// ordinary cast.
    pub current: bool,
    /// `(start_ms on the GetTime clock, duration_ms, enabled)`, the triple the action bar carries;
    /// `None` is cold. The start is absolute, so a running cooldown never churns the book diff.
    pub cooldown: Option<(i64, u32, bool)>,
    /// `GetSpellAutocast`'s `(allowed, enabled)`, pet book only, read off the pet's raw word
    /// (`0x4bd160`, bits 31 and 30).
    pub autocast: Option<(bool, bool)>,
    /// The pet slot's packed word as the server sent it, which `PickupSpell(id, "pet")` puts on the
    /// cursor (`0x4b3260` hands `0x494e20` a pointer to it); 0 in the player book.
    pub packed: u32,
}

/// The pet's book, the reference's second flat array (`0xb6f098`, count `0xb71174`):
///
/// - no tabs, so `GetNumSpellTabs` and `GetSpellTabInfo` take no book type: `SpellBookFrame_Update`
///   hides the skill-line tabs for it (`SpellBookFrame.lua:124`) and `SpellBook_GetSpellID` adds
///   no tab offset (`SpellBookFrame.lua:460-462`);
/// - its own add gate (`0x4b2f90`): in `Spell.dbc` with `Attributes & 0x80` (DO_NOT_DISPLAY)
///   clear, without the player book's tradeskill and `castUI` tests;
/// - the player book's order: `0x4b2fd0` sorts it with `0x4b30c0` (spell-line group, name, rank).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetBookState {
    /// `HasPetSpells`' second return, `ChrClasses.dbc` field 4 (`"PET"` or `"DEMON"`), a key
    /// FrameXML looks up as `PET_TYPE_<token>`; `None` only while there is no book.
    pub token: Option<String>,
    pub slots: Vec<SpellSlotView>,
}

/// The player's book: the skill-line tabs and the flat slot list they index into.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellBookState {
    pub tabs: Vec<SpellTabView>,
    pub slots: Vec<SpellSlotView>,
}

impl super::UiScript {
    /// Replace the book; the app fires `SPELLS_CHANGED` on its own diff.
    pub fn set_spellbook(&mut self, state: SpellBookState) {
        self.model_mut().spellbook = state;
    }

    /// Replace the pet's book; the app fires `SPELLS_CHANGED`, which the reference fires for both
    /// books off one re-sort (`0x4b2fd0`).
    pub fn set_pet_book(&mut self, state: PetBookState) {
        self.model_mut().pet_book = state;
    }

    /// Drain the pet spell ids `CastSpell(id, "pet")` queued: each is a `CMSG_PET_ACTION` with a
    /// type-1 word (`0x4b34ce`), not a player cast.
    pub fn take_pet_spell_casts(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().pet_spell_casts)
    }

    /// Drain the spell ids `ToggleSpellAutocast` queued: `CMSG_PET_SPELL_AUTOCAST` (0x2F3, sent by
    /// `0x4bccb0`) names a spell id, where the pet bar's toggle sends `CMSG_PET_SET_ACTION`.
    pub fn take_pet_spell_autocasts(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().pet_spell_autocasts)
    }

    /// The pushed book, for resolving a macro's spell by the rule `CastSpellByName` uses, so the
    /// bar and the cast agree on which rank a bare `/cast` means.
    pub fn spellbook(&self) -> SpellBookState {
        self.model_mut().spellbook.clone()
    }

    /// Drain the spell ids `CastSpell` and `CastSpellByName` queued.
    pub fn take_spell_casts(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.model_mut().spell_casts)
    }

    /// Whether `SpellStopCasting()` has something to stop: an auto-repeat or an in-flight cast,
    /// never a channel (`0x6e6e80` reads only the auto-repeat key `0xceac30` and the in-flight
    /// id `0xceca88`, which is 0 mid-channel). Pushed each frame before the ESC chain runs.
    pub fn set_casting(&mut self, casting: bool) {
        self.model_mut().casting = casting;
    }

    /// Drain the `SpellStopCasting()` trigger; the app stops the auto-repeat first, else the
    /// in-flight cast, the reference's order.
    pub fn take_spell_stop(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().spell_stop)
    }

    /// Whether the spell-targeting cursor is up, for `SpellIsTargeting()` and
    /// `SpellStopTargeting()`; pushed each frame before the input pass runs the ESC chain. Arming
    /// ends repair mode, as `HideRepairCursor` does: the targeting arm writes the Cast base mode
    /// over Repair (`0x6e50b0`), and its end restores Point (`0x6e49f5`, `0x6e554c`), never Repair.
    pub fn set_spell_targeting(&mut self, targeting: bool) {
        let mut model = self.model_mut();
        model.spell_targeting = targeting;
        if targeting {
            model.repair_mode = false;
        }
    }

    /// Drain the `SpellStopTargeting()` trigger, the ESC chain's rung (`UIParent.lua:1490`); the
    /// app clears its targeting mode.
    pub fn take_stop_targeting(&mut self) -> bool {
        std::mem::take(&mut self.model_mut().spell_stop_targeting)
    }
}

/// Whether `bookType` names the pet book: a case-insensitive compare with `"pet"` alone
/// (`0x4b3f27`), so any other string, `"spell"` included, is the player's book.
fn is_pet_book(book_type: &str) -> bool {
    book_type.eq_ignore_ascii_case(BOOKTYPE_PET)
}

/// The reference's shared spell-slot marshaller (`0x4b3ec0`): arg 1 a number, arg 2 a number or
/// string (`0x4b3ee8`), and the index `trunc(arg1 - 1)` in `[0, 0x400)`, else the binding raises
/// `Invalid spell slot in <Verb>`. Answers the 1-based id and the normalised book type.
fn spell_slot_args(id: Value, book_type: Value, verb: &str) -> mlua::Result<(u32, String)> {
    let invalid = || mlua::Error::runtime(format!("Invalid spell slot in {verb}"));
    // `lua_isnumber` + `lua_tonumber` (`0x6f34d0`/`0x6f3620`): a numeric string is a number.
    let index = match id {
        Value::Integer(i) => i as f64,
        Value::Number(n) => n,
        Value::String(s) => s
            .to_str()
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .ok_or_else(invalid)?,
        _ => return Err(invalid()),
    };
    let book = match book_type {
        Value::String(s) => s.to_str().map(|s| s.to_string()).map_err(|_| invalid())?,
        Value::Integer(_) | Value::Number(_) => String::new(),
        _ => return Err(invalid()),
    };
    let index = (index - 1.0).trunc();
    if !(0.0..1024.0).contains(&index) {
        return Err(invalid());
    }
    let book = if is_pet_book(&book) {
        BOOKTYPE_PET
    } else {
        BOOKTYPE_SPELL
    };
    Ok((index as u32 + 1, book.to_string()))
}

/// The 1-based book id as a 0-based slot index.
fn slot_index(id: u32) -> Option<usize> {
    usize::try_from(id.checked_sub(1)?).ok()
}

/// The slot a `bookType` binding reads. The reference forks `isPet ? [0xb6f098 + 4*i] :
/// [0xb700f0 + 4*i]` in each binding (`0x4b3f5d`, `0x4b40e6`, `0x4b3735`, `0x4b3339`) and in
/// `GameTooltip:SetSpell` (`0x532e1c`/`0x532e2a`), which shares this.
pub(super) fn book_slot<'a>(
    model: &'a Model,
    id: u32,
    book_type: &str,
) -> Option<&'a SpellSlotView> {
    let slots = if is_pet_book(book_type) {
        &model.pet_book.slots
    } else {
        &model.spellbook.slots
    };
    slots.get(slot_index(id)?)
}

/// Resolve a spell by name against the player's book, for `CastSpellByName` and so `/cast`, whose
/// grammar the client's help text gives as `/cast <name> (<subtext>)` (`MACRO_HELP_TEXT_LINE4`):
///
/// - `Fireball`: the highest known rank, by the subtext's leading number (unranked is 0, and a tie
///   goes to the later slot, the order the book lists ranks in);
/// - `Fireball(Rank 1)` or `Fireball (Rank 1)`: that subtext, case-insensitively.
///
/// Names match whole and case-insensitively; passives never match. An unknown spell casts nothing
/// and prints no error, as in the reference (`SlashCmdList["CAST"]` discards the result). The
/// reference cuts the name at `(` untrimmed (`0x4b3950`), so its spaced form keeps a trailing space
/// and misses, and it matches name and rank alone, across the pet's book too (`0x4b3a10`).
pub fn resolve_spell_by_name<'a>(
    book: &'a SpellBookState,
    query: &str,
) -> Option<&'a SpellSlotView> {
    let (name, subtext) = split_subtext(query);
    if name.is_empty() {
        return None;
    }
    book.slots
        .iter()
        .filter(|s| !s.passive && s.name.eq_ignore_ascii_case(name))
        .filter(|s| match subtext {
            Some(want) => s
                .rank
                .as_deref()
                .is_some_and(|r| r.eq_ignore_ascii_case(want)),
            None => true,
        })
        .enumerate()
        .max_by_key(|(i, s)| (rank_number(s.rank.as_deref()), *i))
        .map(|(_, s)| s)
}

/// `Name(Subtext)` or `Name (Subtext)` to `("Name", Some("Subtext"))`; without a closing
/// parenthesis after the opening one, the whole string is the name.
fn split_subtext(query: &str) -> (&str, Option<&str>) {
    let q = query.trim();
    let Some(open) = q.find('(') else {
        return (q, None);
    };
    let Some(close) = q.rfind(')') else {
        return (q, None);
    };
    if close < open {
        return (q, None);
    }
    (q[..open].trim(), Some(q[open + 1..close].trim()))
}

/// The rank in a `NameSubtext` (`"Rank 8"` is 8) by the reference's parse, the first digit run;
/// a subtext without digits, or none, reads 0.
fn rank_number(subtext: Option<&str>) -> u32 {
    subtext
        .unwrap_or("")
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .fold(0u32, |acc, c| acc * 10 + c.to_digit(10).unwrap_or(0))
}

/// `PickupSpell(id, bookType)`, the spell button's drag and shift-click
/// (`SpellBookFrame.lua:266-290`). It refuses while the cursor holds anything, where the
/// reference's setters clear the cursor first (`0x495190`, called at `0x494dab` and `0x494f00`).
fn pickup_spell(model: &mut Model, id: u32, book_type: &str) -> bool {
    if model.cursor.is_some() {
        return false;
    }
    let Some(slot) = book_slot(model, id, book_type) else {
        return false;
    };
    // The pet arm of `0x4b3260` puts the pet's raw word on the cursor (`0x494e20`, cursor mode 4),
    // the player arm a spell id (`0x494d20`, mode 3); only the pet word drops on the pet bar.
    let payload = if is_pet_book(book_type) {
        // `0x494e20` refuses type 0 and types 8 and up.
        let packed = slot.packed;
        if !(1..=7).contains(&((packed >> 24) & 0x3F)) {
            return false;
        }
        CursorPayload::PetAction(super::cursor::CursorPetAction {
            // No source slot: the word comes from the book, not the bar.
            src_slot: 0,
            packed,
            passive: slot.passive,
            texture: slot.texture.clone(),
        })
    } else {
        CursorPayload::Spell(CursorSpell {
            book_slot: id,
            book_type: book_type.to_string(),
            spell_id: slot.spell_id,
            texture: slot.texture.clone(),
            passive: slot.passive,
        })
    };
    model.cursor = Some(payload);
    queue_cursor_update(model);
    true
}

/// Register the spellbook globals.
pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set("BOOKTYPE_SPELL", BOOKTYPE_SPELL)?;
    g.set("BOOKTYPE_PET", BOOKTYPE_PET)?;

    // `UpdateSpells()` (`[0x4b43e0,0x4b43ec)`) only signals `SPELLS_CHANGED` (event 260, whose one
    // fire site is `0x4b302f`): no arguments read, no returns, no state change, and no re-sort,
    // which the other `SPELLS_CHANGED` sites do. The repaint is FrameXML's, through the event.
    // `SignalEvent` is synchronous, so this fires now, not on the next tick.
    g.set(
        "UpdateSpells",
        lua.create_function(|lua, ()| {
            super::tick::fire_event_into(lua, "SPELLS_CHANGED", Vec::new());
            Ok(())
        })?,
    )?;

    // `PlayerHasSpells()` is a constant 1 in the reference (it pushes 1.0 with no branch);
    // `MainMenuBarMicroButtons.xml:80` picks the spellbook micro button's tooltip by it.
    g.set("PlayerHasSpells", lua.create_function(|_, ()| Ok(1_i64))?)?;

    g.set(
        "GetNumSpellTabs",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(model.spellbook.tabs.len() as i64)
        })?,
    )?;

    // GetSpellTabInfo(i) -> name, texture, offset, numSpells, four values on every path
    // (`0x4b3ce0`). Out of range, 0 and negatives included (`0x4b3d26`), is `nil, nil, 0, 0`
    // (`0x4b3e12`): `SpellBookFrame.lua:295` and `:328` compare `id > (offset + numSpells)`
    // unguarded, so the zeros hide every button where a nil would raise. In range, the reference
    // answers a nil name and texture for a skill line with no DBC row (`0x4b3dbd`) and a nil
    // texture for one with no icon (SkillLine 733, 753, 754); here the name is always a string.
    // The index is truncated before the decrement, unlike in `0x4b3ec0`, so 0.5 is out of range.
    g.set(
        "GetSpellTabInfo",
        lua.create_function(|lua, i: Value| {
            let i = super::binding_abi::number_arg(lua, i, "Usage: GetSpellTabInfo(index)")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            // Zero or a negative fails `usize::try_from`, as the unsigned compare fails it.
            let tab = usize::try_from(i64::from(i) - 1)
                .ok()
                .and_then(|n| model.spellbook.tabs.get(n));
            let Some(tab) = tab else {
                return Ok(MultiValue::from_vec(vec![
                    Value::Nil,
                    Value::Nil,
                    Value::Integer(0),
                    Value::Integer(0),
                ]));
            };
            let texture = match &tab.texture {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::Nil,
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&tab.name)?),
                texture,
                Value::Integer(i64::from(tab.offset)),
                Value::Integer(i64::from(tab.num_spells)),
            ]))
        })?,
    )?;

    // GetSpellName(id, bookType) -> name, rank, always two values; a rankless spell's rank is "",
    // never nil: `0x4b4076` pushes `NameSubtext` through `0x6f3890`, which tests only pointer
    // nullity, and `SpellRec::Read` (`0x583750`) makes every string offset a pointer. 14,403 of
    // the 22,357 `Spell.dbc` rows have an empty one, `Attack` (spell 6603) among them.
    g.set(
        "GetSpellName",
        lua.create_function(|lua, (id, book_type): (Value, Value)| {
            let (id, book_type) = spell_slot_args(id, book_type, "GetSpellName")?;
            // Out of `[0, 0x400)` already raised in `spell_slot_args`; the binding's own bound
            // check (`0x4b4018`) is never taken.
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let Some(slot) = book_slot(&model, id, &book_type) else {
                // An empty slot inside the range answers two nils (`0x4b4086`).
                return Ok(MultiValue::from_vec(vec![Value::Nil, Value::Nil]));
            };
            Ok(MultiValue::from_vec(vec![
                Value::String(lua.create_string(&slot.name)?),
                Value::String(lua.create_string(slot.rank.as_deref().unwrap_or(""))?),
            ]))
        })?,
    )?;

    g.set(
        "GetSpellTexture",
        lua.create_function(|lua, (id, book_type): (Value, Value)| {
            let (id, book_type) = spell_slot_args(id, book_type, "GetSpellTexture")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let tex = book_slot(&model, id, &book_type).and_then(|s| s.texture.clone());
            match tex {
                Some(t) => Ok(Value::String(lua.create_string(&t)?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;

    // IsCurrentCast(id, bookType) (`0x4b4370`) reads back the app's per-slot verdict,
    // `SpellSlotView::current`.
    g.set(
        "IsCurrentCast",
        lua.create_function(|lua, (id, book_type): (Value, Value)| {
            let (id, book_type) = spell_slot_args(id, book_type, "IsCurrentCast")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let current = book_slot(&model, id, &book_type).is_some_and(|s| s.current);
            // 1 or nil, never false, as the reference's bindings answer.
            match current {
                true => Ok(Value::Integer(1)),
                false => Ok(Value::Nil),
            }
        })?,
    )?;

    // GetSpellCooldown(id, bookType) -> start, duration, enable in `GetTime` seconds, as
    // `GetActionCooldown`: enable 0 for an on-hold cooldown; an expired or empty one answers the
    // reference's no-cooldown `(0, 0, 1)`, so a re-feed cannot replay the finish flash.
    g.set(
        "GetSpellCooldown",
        lua.create_function(|lua, (id, book_type): (Value, Value)| {
            let (id, book_type) = spell_slot_args(id, book_type, "GetSpellCooldown")?;
            let now: f64 = lua.globals().get("__benilla_now").unwrap_or(0.0);
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let cooldown = book_slot(&model, id, &book_type).and_then(|s| s.cooldown);
            Ok(match cooldown {
                Some((start_ms, duration_ms, enabled)) => {
                    let (start, duration) =
                        (start_ms as f64 / 1000.0, f64::from(duration_ms) / 1000.0);
                    if start + duration > now || !enabled {
                        (start, duration, i32::from(enabled))
                    } else {
                        (0.0, 0.0, 1)
                    }
                }
                None => (0.0, 0.0, 1),
            })
        })?,
    )?;

    g.set(
        "IsSpellPassive",
        lua.create_function(|lua, (id, book_type): (Value, Value)| {
            let (id, book_type) = spell_slot_args(id, book_type, "IsSpellPassive")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            Ok(flag(
                book_slot(&model, id, &book_type).is_some_and(|s| s.passive),
            ))
        })?,
    )?;

    // CastSpell(id, bookType), the plain click, queues the slot's spell unless it is passive. On
    // the pet book `0x4b3300` forks at `0x4b34c8` (the player's cast is `0x6e5a90`) to send
    // `CMSG_PET_ACTION` (0x175), `{ u64 [0xb714a0], u32 (spellId & 0xFFFF) | 0x01000000, u64
    // target }` (`0x4b34ce`), so the pet book casts a spell not on the bar; the target falls back
    // to the selection (`0x4b34af`), as in `CastPetAction`. The app sends both at the drain.
    g.set(
        "CastSpell",
        lua.create_function(|lua, (id, book_type): (Value, Value)| {
            let (id, book_type) = spell_slot_args(id, book_type, "CastSpell")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(slot) = book_slot(&model, id, &book_type) {
                if !slot.passive {
                    let spell_id = slot.spell_id;
                    if is_pet_book(&book_type) {
                        model.pet_spell_casts.push(spell_id);
                    } else {
                        model.spell_casts.push(spell_id);
                    }
                }
            }
            Ok(())
        })?,
    )?;

    // HasPetSpells() -> numPetSpells, petToken: always two returns (`0x4b4410`), `(nil, nil)` with
    // no pet spells (`0x4b4420`). The count is a number: `SpellBook_GetCurrentPage` divides by it.
    g.set(
        "HasPetSpells",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let n = model.pet_book.slots.len();
            if n == 0 {
                return Ok(MultiValue::from_vec(vec![Value::Nil, Value::Nil]));
            }
            let token = match &model.pet_book.token {
                Some(t) => Value::String(lua.create_string(t)?),
                None => Value::String(lua.create_string(PET_TOKEN_FALLBACK)?),
            };
            Ok(MultiValue::from_vec(vec![Value::Integer(n as i64), token]))
        })?,
    )?;

    // GetSpellAutocast(id, bookType) -> autoCastAllowed, autoCastEnabled, 1 or nil each: always
    // two returns, `(nil, nil)` for the player book (`0x4b4180` tests the book at `0x4b41cb` and
    // `0x4b41d6`) and for an empty slot.
    g.set(
        "GetSpellAutocast",
        lua.create_function(move |lua, (id, book_type): (Value, Value)| {
            let (id, book_type) = spell_slot_args(id, book_type, "GetSpellAutocast")?;
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            let (allowed, enabled) = book_slot(&model, id, &book_type)
                .filter(|_| is_pet_book(&book_type))
                .and_then(|s| s.autocast)
                .unwrap_or((false, false));
            Ok((flag(allowed), flag(enabled)))
        })?,
    )?;

    // ToggleSpellAutocast(id, bookType), the pet book's right click (`0x4b4240`): queued only for
    // an autocast-allowed word, the gate `0x4bccb0` applies before sending (`0x4bccf5`). The app
    // sends `CMSG_PET_SPELL_AUTOCAST` and flips bit 30 on the book and the bar at the drain.
    g.set(
        "ToggleSpellAutocast",
        lua.create_function(|lua, (id, book_type): (Value, Value)| {
            let (id, book_type) = spell_slot_args(id, book_type, "ToggleSpellAutocast")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            let spell_id = book_slot(&model, id, &book_type)
                .filter(|_| is_pet_book(&book_type))
                .filter(|s| s.autocast.is_some_and(|(allowed, _)| allowed))
                .map(|s| s.spell_id);
            if let Some(spell_id) = spell_id {
                model.pet_spell_autocasts.push(spell_id);
            }
            Ok(())
        })?,
    )?;

    // CastSpellByName(name [, onSelf]) (`0x4b4ab0`) shares the dispatcher `0x4b3300` with
    // `CastSpell`, so it queues on the same list; `SlashCmdList["CAST"]` calls it. `onSelf` is
    // accepted and ignored: self-cast is not built.
    g.set(
        "CastSpellByName",
        lua.create_function(|lua, (name, _on_self): (String, MultiValue)| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if let Some(slot) = resolve_spell_by_name(&model.spellbook, &name) {
                let spell_id = slot.spell_id;
                model.spell_casts.push(spell_id);
            }
            Ok(())
        })?,
    )?;

    g.set(
        "PickupSpell",
        lua.create_function(|lua, (id, book_type): (Value, Value)| {
            let (id, book_type) = spell_slot_args(id, book_type, "PickupSpell")?;
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            Ok(pickup_spell(&mut model, id, &book_type))
        })?,
    )?;

    // SpellStopCasting() (`0x6e6e80`) stops the first of a running auto-repeat (`0x6ea080`,
    // `CMSG_CANCEL_AUTO_REPEAT_SPELL`) or an in-flight cast (`CMSG_CANCEL_CAST`) and answers 1,
    // else nil. A channel answers nil: the channel canceler `0x6e9b70` is never reached, and the
    // in-flight id `0xceca88` is 0 mid-channel (cleared at `0x6e7408`). The nil matters: the ESC
    // chain (`UIParent.lua:1489`) reaches the game menu only through it.
    g.set(
        "SpellStopCasting",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.casting {
                model.spell_stop = true;
                Ok(Value::Integer(1))
            } else {
                Ok(Value::Nil)
            }
        })?,
    )?;

    // SpellIsTargeting() (`0x6e6cd0`): true while the targeting cursor is up, else nil.
    g.set(
        "SpellIsTargeting",
        lua.create_function(|lua, ()| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            if model.spell_targeting {
                Ok(Value::Boolean(true))
            } else {
                Ok(Value::Nil)
            }
        })?,
    )?;

    // SpellCanTargetUnit("unit") (`0x6e6d00`) asks `0x6e6460`'s unit leg whether the targeting
    // word can take the unit. The token is not read: no word benilla can arm (location, item,
    // gameobject) takes a unit, and unit words are not built.
    g.set(
        "SpellCanTargetUnit",
        lua.create_function(|lua, _unit: Option<String>| {
            let model = lua.app_data_ref::<Model>().expect("model app_data");
            if model.spell_can_target_unit {
                Ok(Value::Boolean(true))
            } else {
                Ok(Value::Nil)
            }
        })?,
    )?;

    // SpellStopTargeting() (`0x6e6e30`): while targeting, `0x6e4900` clears the word with no
    // packet and the binding answers 1, else nil, so the ESC chain (`UIParent.lua:1490`) falls
    // through to the game menu.
    g.set(
        "SpellStopTargeting",
        lua.create_function(|lua, ()| {
            let mut model = lua.app_data_mut::<Model>().expect("model app_data");
            if model.spell_targeting {
                model.spell_stop_targeting = true;
                Ok(Value::Integer(1))
            } else {
                Ok(Value::Nil)
            }
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PetBookState, SpellBookState, SpellSlotView, SpellTabView};
    use crate::script::cursor::{CursorAction, CursorPayload};
    use crate::script::UiScript;

    /// Two tabs: Fire (Fireball, and Fire Blast marked passive) and Frost (Frost Armor).
    fn book() -> SpellBookState {
        SpellBookState {
            tabs: vec![
                SpellTabView {
                    name: "Fire".into(),
                    texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
                    offset: 0,
                    num_spells: 2,
                },
                SpellTabView {
                    name: "Frost".into(),
                    texture: Some("Interface\\Icons\\Spell_Frost_FrostBolt02".into()),
                    offset: 2,
                    num_spells: 1,
                },
            ],
            slots: vec![
                SpellSlotView {
                    spell_id: 133,
                    name: "Fireball".into(),
                    rank: Some("Rank 1".into()),
                    texture: Some("Interface\\Icons\\Spell_Fire_FlameBolt".into()),
                    passive: false,
                    current: false,
                    cooldown: None,
                    ..Default::default()
                },
                SpellSlotView {
                    spell_id: 2136,
                    name: "Fire Blast".into(),
                    rank: Some("Rank 1".into()),
                    texture: Some("Interface\\Icons\\Spell_Fire_FireBolt02".into()),
                    passive: true, // not really passive: exercises the refusal
                    current: false,
                    cooldown: None,
                    ..Default::default()
                },
                SpellSlotView {
                    spell_id: 168,
                    name: "Frost Armor".into(),
                    rank: Some("Rank 1".into()),
                    texture: Some("Interface\\Icons\\Spell_Frost_FrostArmor02".into()),
                    passive: false,
                    current: false,
                    cooldown: None,
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn is_current_cast_reads_the_slot_verdict() {
        let mut s = UiScript::new().unwrap();
        assert!(s
            .eval::<bool>("return IsCurrentCast(1, BOOKTYPE_SPELL) == nil")
            .unwrap());
        let mut b = book();
        b.slots[0].current = true;
        s.set_spellbook(b);
        assert_eq!(
            s.eval::<i64>("return IsCurrentCast(1, BOOKTYPE_SPELL)")
                .unwrap(),
            1
        );
        assert!(s
            .eval::<bool>("return IsCurrentCast(2, BOOKTYPE_SPELL) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return IsCurrentCast(1, BOOKTYPE_PET) == nil")
            .unwrap());
        assert!(s
            .eval::<bool>("return IsCurrentCast(99, BOOKTYPE_SPELL) == nil")
            .unwrap());
    }

    #[test]
    fn get_spell_cooldown_reads_the_slot_triple() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());
        s.tick(20.0); // GetTime = 20

        // Cold slot: the reference's no-cooldown shape.
        assert_eq!(
            s.eval::<(f64, f64, i64)>("return GetSpellCooldown(1, BOOKTYPE_SPELL)")
                .unwrap(),
            (0.0, 0.0, 1)
        );

        let mut b = book();
        b.slots[0].cooldown = Some((14_000, 10_000, true)); // running: 4 s elapsed of 10
        b.slots[1].cooldown = Some((2_000, 8_000, false)); // on hold: parked since t=2
        b.slots[2].cooldown = Some((5_000, 10_000, true)); // elapsed at t=15: cold
        s.set_spellbook(b);
        assert_eq!(
            s.eval::<(f64, f64, i64)>("return GetSpellCooldown(1, BOOKTYPE_SPELL)")
                .unwrap(),
            (14.0, 10.0, 1)
        );
        // On hold survives the expiry guard.
        assert_eq!(
            s.eval::<(f64, f64, i64)>("return GetSpellCooldown(2, BOOKTYPE_SPELL)")
                .unwrap(),
            (2.0, 8.0, 0)
        );
        // Elapsed goes cold.
        assert_eq!(
            s.eval::<(f64, f64, i64)>("return GetSpellCooldown(3, BOOKTYPE_SPELL)")
                .unwrap(),
            (0.0, 0.0, 1)
        );
        // No pet book, and past the book, answer cold too.
        assert_eq!(
            s.eval::<(f64, f64, i64)>("return GetSpellCooldown(1, BOOKTYPE_PET)")
                .unwrap(),
            (0.0, 0.0, 1)
        );
        assert_eq!(
            s.eval::<(f64, f64, i64)>("return GetSpellCooldown(99, BOOKTYPE_SPELL)")
                .unwrap(),
            (0.0, 0.0, 1)
        );
    }

    #[test]
    fn tab_info_shapes_and_book_id_offsets() {
        let mut s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSpellTabs()").unwrap(), 0);
        // Lua compares only the first value; `out_of_range_answers_four_values` checks the arity.
        assert!(s
            .eval::<bool>("return (GetSpellTabInfo(1)) == nil")
            .unwrap());

        s.set_spellbook(book());
        assert_eq!(s.eval::<i64>("return GetNumSpellTabs()").unwrap(), 2);

        let (name, texture, offset, num) = s
            .eval::<(String, String, i64, i64)>("return GetSpellTabInfo(1)")
            .unwrap();
        assert_eq!(
            (name.as_str(), texture.as_str(), offset, num),
            ("Fire", "Interface\\Icons\\Spell_Fire_FlameBolt", 0, 2)
        );
        let (name2, _tex2, offset2, num2) = s
            .eval::<(String, String, i64, i64)>("return GetSpellTabInfo(2)")
            .unwrap();
        assert_eq!((name2.as_str(), offset2, num2), ("Frost", 2, 1));

        // Out of range: nil first.
        assert!(s.eval::<bool>("return GetSpellTabInfo(3) == nil").unwrap());
    }

    /// The reference's marshaller, `0x4b3ec0`.
    #[test]
    fn the_slot_marshaller_gates_the_index_and_names_the_book() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());
        s.set_pet_book(pet_book());
        for bad in ["0", "-11", "1025", "nil", "\"x\"", "{}"] {
            let err = s
                .eval::<mlua::Value>(&format!("return GetSpellTexture({bad}, BOOKTYPE_SPELL)"))
                .expect_err(bad)
                .to_string();
            assert!(
                err.contains("Invalid spell slot in GetSpellTexture"),
                "{bad}: {err}"
            );
        }
        assert!(
            s.eval::<mlua::Value>("return GetSpellTexture(1)")
                .unwrap_err()
                .to_string()
                .contains("Invalid spell slot in GetSpellTexture"),
            "the book type is not optional"
        );
        // Inside the range but past the book: nil, not an error.
        assert!(s
            .eval::<bool>("return GetSpellTexture(1024, BOOKTYPE_SPELL) == nil")
            .unwrap());
        // Truncation toward zero: 1.9 is slot 1, and so is 0.99 (`trunc(-0.01)` is 0). A numeric
        // string is a number (`lua_isnumber`).
        for one in ["1.9", "0.99", "\"1\""] {
            assert_eq!(
                s.eval::<String>(&format!("return GetSpellName({one}, BOOKTYPE_SPELL)"))
                    .unwrap(),
                "Fireball",
                "{one}"
            );
        }
        let (name, _) = s
            .eval::<(String, String)>("return GetSpellName(1, \"PET\")")
            .unwrap();
        assert_eq!(
            name,
            pet_book().slots[0].name,
            "the pet list, case-insensitively"
        );
        let (name, _) = s
            .eval::<(String, String)>("return GetSpellName(1, 7)")
            .unwrap();
        assert_eq!(
            name, "Fireball",
            "a number is an accepted, player-book type"
        );
    }

    #[test]
    fn name_and_rank_read_through_the_book_id_seam() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());

        // Book id 1 (tab 1 offset 0 + button 1) -> slot 0 -> Fireball.
        let (name, rank) = s
            .eval::<(String, String)>(r#"return GetSpellName(1, BOOKTYPE_SPELL)"#)
            .unwrap();
        assert_eq!((name.as_str(), rank.as_str()), ("Fireball", "Rank 1"));
        assert_eq!(
            s.eval::<String>(r#"return GetSpellTexture(1, BOOKTYPE_SPELL)"#)
                .unwrap(),
            "Interface\\Icons\\Spell_Fire_FlameBolt"
        );

        // Book id 3 (tab 2 offset 2 + button 1) -> slot 2 -> Frost Armor.
        let (name3, _rank3) = s
            .eval::<(String, String)>(r#"return GetSpellName(3, BOOKTYPE_SPELL)"#)
            .unwrap();
        assert_eq!(name3, "Frost Armor");

        // Past the book, and an absent pet book, answer nil.
        assert!(s
            .eval::<bool>(r#"return GetSpellName(99, BOOKTYPE_SPELL) == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return GetSpellName(1, BOOKTYPE_PET) == nil"#)
            .unwrap());
    }

    #[test]
    fn pickup_spell_payload_and_cursor_update() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());

        s.run(r#"picked = PickupSpell(1, BOOKTYPE_SPELL)"#).unwrap();
        assert!(s.eval::<bool>("return picked").unwrap());
        assert!(s.cursor_payload().is_some());
        let (kind, book_id, book, spell_id) = s
            .eval::<(String, i64, String, i64)>(
                "local k, slot, book, id = GetCursorInfo() return k, slot, book, id",
            )
            .unwrap();
        assert_eq!(
            (kind.as_str(), book_id, book.as_str(), spell_id),
            ("spell", 1, "spell", 133)
        );

        // Tick first to flush the first pickup's `CURSOR_UPDATE`, so the count below is the
        // refused call's alone.
        s.tick(0.0);
        s.run(
            r#"
            cursorUpdates = 0
            local f = CreateFrame("Frame", "CursorListener")
            f:RegisterEvent("CURSOR_UPDATE")
            f:SetScript("OnEvent", function() cursorUpdates = cursorUpdates + 1 end)
            "#,
        )
        .unwrap();
        s.run(r#"PickupSpell(3, BOOKTYPE_SPELL)"#).unwrap(); // already holding -> refused, no-op
        s.tick(0.01);
        assert_eq!(
            s.eval::<i64>("return cursorUpdates").unwrap(),
            0,
            "refused pickup fires no CURSOR_UPDATE"
        );
        // Still holding the first pickup: a refusal never clobbers it.
        assert_eq!(
            s.eval::<(String, i64, String, i64)>(
                "local k, slot, book, id = GetCursorInfo() return k, slot, book, id"
            )
            .unwrap(),
            ("spell".to_string(), 1, "spell".to_string(), 133)
        );
    }

    #[test]
    fn pickup_spell_refuses_while_already_holding_any_payload() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());
        s.set_cursor_for_test(CursorPayload::Action(CursorAction {
            src_slot: 1,
            kind: 0,
            action: 111,
            texture: None,
        }));

        assert!(!s
            .eval::<bool>(r#"return PickupSpell(1, BOOKTYPE_SPELL)"#)
            .unwrap());
        // The original (action) payload survives untouched.
        assert_eq!(
            s.eval::<String>("local k = GetCursorInfo() return k")
                .unwrap(),
            "action"
        );
    }

    #[test]
    fn passive_refuses_the_cast_but_active_queues_it() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());

        s.run(r#"CastSpell(1, BOOKTYPE_SPELL)"#).unwrap(); // Fireball: active
        assert_eq!(s.take_spell_casts(), vec![133]);

        s.run(r#"CastSpell(2, BOOKTYPE_SPELL)"#).unwrap(); // Fire Blast: passive, refused
        assert!(s.take_spell_casts().is_empty());

        assert!(s
            .eval::<bool>(r#"return IsSpellPassive(2, BOOKTYPE_SPELL)"#)
            .unwrap());
        assert!(!s
            .eval::<bool>(r#"return IsSpellPassive(1, BOOKTYPE_SPELL)"#)
            .unwrap());
    }

    /// With no pet book, as when the reference's count `[0xb71174]` is 0, every pet arm answers
    /// nothing.
    #[test]
    fn an_absent_pet_book_answers_empty_everywhere() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());

        assert!(s
            .eval::<bool>(r#"return GetSpellName(1, BOOKTYPE_PET) == nil"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"return GetSpellTexture(1, BOOKTYPE_PET) == nil"#)
            .unwrap());
        assert!(!s
            .eval::<bool>(r#"return IsSpellPassive(1, BOOKTYPE_PET)"#)
            .unwrap());
        assert!(s
            .eval::<bool>(r#"local n, t = HasPetSpells() return n == nil and t == nil"#)
            .unwrap());

        s.run(r#"CastSpell(1, BOOKTYPE_PET)"#).unwrap();
        assert!(s.take_spell_casts().is_empty(), "pet cast is a no-op");
        assert!(s.take_pet_spell_casts().is_empty());

        assert!(!s
            .eval::<bool>(r#"return PickupSpell(1, BOOKTYPE_PET)"#)
            .unwrap());
        assert!(s.cursor_payload().is_none(), "pet pickup is a no-op");
    }

    /// A hunter's pet book: Growl (autocastable, on, cooling down), Claw (autocastable, off) and
    /// Avoidance (a passive, no autocast, `ACT_PASSIVE` 0x01).
    fn pet_book() -> PetBookState {
        PetBookState {
            token: Some("PET".into()),
            slots: vec![
                SpellSlotView {
                    spell_id: 2649,
                    name: "Growl".into(),
                    rank: Some("Rank 1".into()),
                    texture: Some("Interface\\Icons\\Ability_Physical_Taunt".into()),
                    cooldown: Some((9400, 5000, true)),
                    autocast: Some((true, true)),
                    packed: 0xC100_0000 | 2649,
                    ..Default::default()
                },
                SpellSlotView {
                    spell_id: 16827,
                    name: "Claw".into(),
                    rank: Some("Rank 1".into()),
                    texture: Some("Interface\\Icons\\Ability_Druid_Rake".into()),
                    autocast: Some((true, false)),
                    packed: 0x8100_0000 | 16827,
                    ..Default::default()
                },
                SpellSlotView {
                    spell_id: 3025,
                    name: "Avoidance".into(),
                    texture: Some("Interface\\Icons\\Spell_Nature_SpiritArmor".into()),
                    passive: true,
                    autocast: Some((false, false)),
                    packed: 0x0100_0000 | 3025,
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn has_pet_spells_answers_a_count_and_a_class_token() {
        let mut s = UiScript::new().unwrap();
        s.set_pet_book(pet_book());
        assert!(s
            .eval::<bool>(r#"local n, t = HasPetSpells() return n == 3 and t == "PET""#)
            .unwrap());

        // A warlock's token, which FrameXML reads as `PET_TYPE_DEMON`.
        let mut demon = pet_book();
        demon.token = Some("DEMON".into());
        s.set_pet_book(demon);
        assert_eq!(
            s.eval::<String>("local _, t = HasPetSpells() return t")
                .unwrap(),
            "DEMON"
        );

        // No token resolved (no `ChrClasses.dbc`) still answers a string.
        let mut untokened = pet_book();
        untokened.token = None;
        s.set_pet_book(untokened);
        assert_eq!(
            s.eval::<String>("local _, t = HasPetSpells() return t")
                .unwrap(),
            "PET"
        );
    }

    #[test]
    fn one_id_reads_two_different_books() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());
        s.set_pet_book(pet_book());

        assert_eq!(
            s.eval::<String>(r#"return GetSpellName(1, BOOKTYPE_SPELL)"#)
                .unwrap(),
            "Fireball"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetSpellName(1, BOOKTYPE_PET)"#)
                .unwrap(),
            "Growl"
        );
        // `"pet"` alone, without case (`0x4b3f27`); any other string, a typo too, is the player's.
        assert_eq!(
            s.eval::<String>(r#"return GetSpellName(1, "PeT")"#)
                .unwrap(),
            "Growl"
        );
        assert_eq!(
            s.eval::<String>(r#"return GetSpellName(1, "spel")"#)
                .unwrap(),
            "Fireball"
        );
        // Past the pet book's end: nil.
        assert!(s
            .eval::<bool>(r#"return GetSpellName(4, BOOKTYPE_PET) == nil"#)
            .unwrap());
    }

    #[test]
    fn autocast_is_a_pet_only_pair() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());
        s.set_pet_book(pet_book());

        assert!(s
            .eval::<bool>(
                r#"local a, e = GetSpellAutocast(1, BOOKTYPE_PET) return a == 1 and e == 1"#
            )
            .unwrap());
        assert!(s
            .eval::<bool>(
                r#"local a, e = GetSpellAutocast(2, BOOKTYPE_PET) return a == 1 and e == nil"#
            )
            .unwrap());
        assert!(
            s.eval::<bool>(
                r#"local a, e = GetSpellAutocast(3, BOOKTYPE_PET) return a == nil and e == nil"#
            )
            .unwrap(),
            "a passive is not autocastable"
        );
        assert!(
            s.eval::<bool>(
                r#"local a, e = GetSpellAutocast(1, BOOKTYPE_SPELL) return a == nil and e == nil"#
            )
            .unwrap(),
            "the PLAYER book never answers a pair"
        );
        assert!(
            s.eval::<bool>(
                r#"local a, e = GetSpellAutocast(9, BOOKTYPE_PET) return a == nil and e == nil"#
            )
            .unwrap(),
            "still two returns out of range"
        );
    }

    #[test]
    fn only_an_autocastable_pet_slot_queues_a_toggle() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());
        s.set_pet_book(pet_book());

        s.run(
            r#"ToggleSpellAutocast(1, BOOKTYPE_PET)
               ToggleSpellAutocast(3, BOOKTYPE_PET)
               ToggleSpellAutocast(9, BOOKTYPE_PET)
               ToggleSpellAutocast(1, BOOKTYPE_SPELL)"#,
        )
        .unwrap();
        assert_eq!(s.take_pet_spell_autocasts(), vec![2649]);
        assert!(s.take_pet_spell_autocasts().is_empty(), "drain empties");
        assert!(s.take_spell_casts().is_empty());
    }

    #[test]
    fn a_pet_cast_queues_apart_from_a_player_cast() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());
        s.set_pet_book(pet_book());

        s.run(
            r#"CastSpell(1, BOOKTYPE_PET)
               CastSpell(3, BOOKTYPE_PET)
               CastSpell(1, BOOKTYPE_SPELL)"#,
        )
        .unwrap();
        assert_eq!(s.take_pet_spell_casts(), vec![2649], "the passive refused");
        assert_eq!(s.take_spell_casts(), vec![133]);
        assert!(s.take_pet_spell_casts().is_empty(), "drain empties");
    }

    #[test]
    fn a_pet_book_pickup_carries_the_packed_word() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(book());
        s.set_pet_book(pet_book());

        assert!(s
            .eval::<bool>(r#"return PickupSpell(1, BOOKTYPE_PET)"#)
            .unwrap());
        let Some(CursorPayload::PetAction(p)) = s.cursor_payload() else {
            panic!(
                "expected a pet action payload, got {:?}",
                s.cursor_payload()
            );
        };
        assert_eq!(p.packed, 0xC100_0000 | 2649);
        assert_eq!(p.src_slot, 0, "it came out of the book, not off the bar");

        // The player book still produces a spell payload.
        s.run("ClearCursor()").unwrap();
        assert!(s
            .eval::<bool>(r#"return PickupSpell(1, BOOKTYPE_SPELL)"#)
            .unwrap());
        assert!(matches!(s.cursor_payload(), Some(CursorPayload::Spell(_))));
    }

    /// The reference reads a pet book cooldown through `0x6e2ea0` (`edx = isPet`), as
    /// `GetPetActionCooldown` does, so the book and the bar agree.
    #[test]
    fn the_pet_books_cooldown_is_the_pets_own() {
        let mut s = UiScript::new().unwrap();
        s.tick(10.0); // GetTime == 10
        s.set_spellbook(book());
        s.set_pet_book(pet_book());

        let (start, duration, enable) = s
            .eval::<(f64, f64, i32)>(r#"return GetSpellCooldown(1, BOOKTYPE_PET)"#)
            .unwrap();
        assert!((start - 9.4).abs() < 1e-9, "start {start}");
        assert!((duration - 5.0).abs() < 1e-9);
        assert_eq!(enable, 1);
        // The player book's slot 1 has none.
        assert_eq!(
            s.eval::<(f64, f64, i32)>(r#"return GetSpellCooldown(1, BOOKTYPE_SPELL)"#)
                .unwrap(),
            (0.0, 0.0, 1)
        );

        s.tick(5.0); // now == 15 > 9.4 + 5.0
        assert_eq!(
            s.eval::<(f64, f64, i32)>(r#"return GetSpellCooldown(1, BOOKTYPE_PET)"#)
                .unwrap(),
            (0.0, 0.0, 1)
        );
    }

    #[test]
    fn a_rankless_spell_answers_an_empty_rank_and_still_two_values() {
        let mut s = UiScript::new().unwrap();
        s.set_spellbook(SpellBookState {
            tabs: Vec::new(),
            slots: vec![
                SpellSlotView {
                    spell_id: 6603,
                    name: "Attack".into(),
                    rank: None,
                    ..Default::default()
                },
                SpellSlotView {
                    spell_id: 78,
                    name: "Heroic Strike".into(),
                    rank: Some("Rank 1".into()),
                    ..Default::default()
                },
            ],
        });

        assert_eq!(s.arity(r#"GetSpellName(1, "spell")"#).unwrap(), 2);
        let (name, rank) = s
            .eval::<(String, String)>(r#"return GetSpellName(1, "spell")"#)
            .unwrap();
        assert_eq!((name.as_str(), rank.as_str()), ("Attack", ""));
        assert!(
            s.eval::<bool>(r#"local _, r = GetSpellName(1, "spell") return r ~= nil"#)
                .unwrap(),
            "the rankless rank must be a STRING, not nil"
        );
        // Two addon idioms that a nil rank breaks: a table key and a `string.find` argument.
        s.run(r#"local _, r = GetSpellName(1, "spell") local t = {} t[r] = 1"#)
            .expect("a rankless rank must be a legal table key");
        s.run(r#"local _, r = GetSpellName(1, "spell") string.find(r, "(%d+)")"#)
            .expect("a rankless rank must be a legal string.find argument");
        // A ranked spell is unchanged.
        assert_eq!(
            s.eval::<(String, String)>(r#"return GetSpellName(2, "spell")"#)
                .unwrap(),
            ("Heroic Strike".to_string(), "Rank 1".to_string())
        );

        // An empty slot inside the range is two nils.
        assert_eq!(s.arity(r#"GetSpellName(9, "spell")"#).unwrap(), 2);
        assert!(s
            .eval::<bool>(r#"local a, b = GetSpellName(9, "spell") return a == nil and b == nil"#)
            .unwrap());

        // Id 1025 is index 1024, past the `[0, 0x400)` gate: it raises.
        let err = s
            .run(r#"GetSpellName(1025, "spell")"#)
            .expect_err("an out-of-range slot must raise");
        assert!(
            format!("{err}").contains("Invalid spell slot in GetSpellName"),
            "got {err}"
        );
        // Id 1023 is inside it and answers.
        assert_eq!(s.arity(r#"GetSpellName(1023, "spell")"#).unwrap(), 2);
    }

    /// The assertion is that a handler ran, not that a frame repainted: the repaint is FrameXML's.
    #[test]
    fn update_spells_fires_spells_changed_and_touches_nothing() {
        let s = UiScript::new().unwrap();
        s.run(
            r#"
            fired = 0
            local f = CreateFrame("Frame", "SpellsWatcher")
            f:RegisterEvent("SPELLS_CHANGED")
            f:SetScript("OnEvent", function() fired = fired + 1 end)
            "#,
        )
        .unwrap();
        assert_eq!(s.eval::<i64>("return fired").unwrap(), 0);
        s.run("UpdateSpells()").unwrap();
        assert_eq!(
            s.eval::<i64>("return fired").unwrap(),
            1,
            "UpdateSpells must fire SPELLS_CHANGED synchronously, as SignalEvent does"
        );
        // Zero returns, `reference/1.12-shapes.tsv`'s `arity = 0 (exact)`.
        assert_eq!(s.arity("UpdateSpells()").unwrap(), 0);
        assert!(s.errors().is_empty(), "errors: {:?}", s.errors());
    }

    /// On an empty book, where a "has any spells" reading would answer 0.
    #[test]
    fn player_has_spells_is_one_even_with_an_empty_book() {
        let s = UiScript::new().unwrap();
        assert_eq!(s.eval::<i64>("return GetNumSpellTabs()").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return PlayerHasSpells()").unwrap(), 1);
        assert_eq!(s.arity("PlayerHasSpells()").unwrap(), 1);
    }

    /// The count is the assertion: `(GetSpellTabInfo(0)) == nil` holds for a single nil too.
    #[test]
    fn out_of_range_answers_four_values() {
        let s = UiScript::new().unwrap();
        for idx in ["0", "1", "99", "-1", "0.5"] {
            assert_eq!(
                s.arity(&format!("GetSpellTabInfo({idx})")).unwrap(),
                4,
                "GetSpellTabInfo({idx}) must answer four values"
            );
            assert_eq!(
                s.eval::<i64>(&format!(
                    "local _,_,o,n = GetSpellTabInfo({idx}) return o + n"
                ))
                .unwrap(),
                0,
                "GetSpellTabInfo({idx}) slots 3+4 must be NUMBERS summing to 0"
            );
            assert!(s
                .eval::<bool>(&format!("return (GetSpellTabInfo({idx})) == nil"))
                .unwrap());
        }
    }

    #[test]
    fn tab_index_raises_when_absent_and_coerces_a_numeric_string() {
        let mut s = UiScript::new().unwrap();
        for bad in [
            "GetSpellTabInfo()",
            "GetSpellTabInfo(nil)",
            "GetSpellTabInfo({})",
            "GetSpellTabInfo('abc')",
        ] {
            let err = s.run(bad).expect_err(bad);
            assert!(
                format!("{err}").contains("Usage: GetSpellTabInfo(index)"),
                "{bad}: got {err}"
            );
        }
        s.set_spellbook(book());
        // "1" coerces as 1 does.
        assert_eq!(
            s.eval::<String>("return (GetSpellTabInfo('1'))").unwrap(),
            s.eval::<String>("return (GetSpellTabInfo(1))").unwrap()
        );
    }
}
