//! The aura tooltip, the builder `0x52f880(spell, level, stack, _)` and the setters that call it:
//! `SetPlayerBuff`, `SetUnitBuff`, `SetUnitDebuff` and `SetTrackingSpell`. The name and the
//! dispel class, both gold; the aura description, white, its `$`-tokens expanded at the aura's
//! level and stack count; and, on `SetPlayerBuff` alone, the gold time-remaining line.
//!
//! The app resolves each aura into an [`AuraTooltipView`] keyed by spell, level and stack
//! ([`AuraKey`]), pushed ahead of a hover for every aura the VM holds; a miss records the key and
//! waits, as a spell view's does ([`super::tooltip_spell`]).

use mlua::{Lua, Table, Value};

use super::binding_abi;
use super::object::frame_handle_of;
use super::tooltip::{append_line, clear};
use super::tooltip_spell::{insert_wait, WaitFor};
use super::{Model, TooltipText};
use crate::widget::FrameHandle;

/// The gold wrapper `0x530380`'s `0xffffd200`, the name's and the dispel class's.
const GOLD: [f32; 4] = [1.0, 210.0 / 255.0, 0.0, 1.0];
/// The description's white, the colour at `0xc0cf60` (`0x52f95c`).
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// The player's buff cache holds 48 records (`0xbc6040`), the positions `GetBuffRecord 0x4e4430`
/// answers.
const PLAYER_BUFF_RECORDS: usize = 48;

/// What the aura builder is called with besides the tooltip: the spell, the level the description's
/// per-level terms read (0 for the spell's own skill level), and the stack count. The setters pass
/// the aura slot's `AURALEVELS` byte and `AURAAPPLICATIONS + 1` (`0x534ceb`, `0x534cd1`; the
/// player's record's `+0x8` and `+0x9`, `0x532bbf`), a party record level 0 and stack 1
/// (`0x534da4`-`0x534da8`), and `SetTrackingSpell` level 1 and stack 1 (`0x532cde`-`0x532ce2`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AuraKey {
    pub spell_id: u32,
    pub level: u8,
    pub stack: u8,
}

/// One aura's tooltip, prepared by the app ([`AuraKey`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuraTooltipView {
    pub name: String,
    /// The gold right column (`0x52f8e5`-`0x52f90d`): the `SpellDispelType.dbc` name, such as
    /// "Magic", when that row's `[+0x28]` flag is set, so Stealth has none.
    pub dispel_type: Option<String>,
    /// `AuraDescription` with its `$`-tokens expanded at the key's level and, for a spell whose
    /// `StackAmount` is above 1, its stack (`0x52f8ca`-`0x52f8dc`, `0x52f955`), the points'
    /// modifiers off. Empty is no line: the builder skips an empty text and falls back to nothing
    /// (`0x52f929`-`0x52f938`).
    pub description: TooltipText,
}

impl super::UiScript {
    /// Store or replace an aura's view, answering its ask: a tooltip waiting on it re-renders
    /// now, a frame after its hover, where the reference builds it at the call (`0x52f880`).
    pub fn set_aura_tooltip(&mut self, key: AuraKey, view: AuraTooltipView) {
        let waiting: Vec<FrameHandle> = {
            let mut model = self.model_mut();
            model.aura_tooltip_asks.remove(&key);
            model.aura_tooltips.insert(key, view);
            model
                .spell_tooltip_waits
                .iter()
                .filter(|(_, w)| matches!(w.subject, WaitFor::Aura { key: k, .. } if k == key))
                .map(|(h, _)| *h)
                .collect()
        };
        for h in waiting {
            if let Err(e) = super::tooltip_spell::answer_wait(
                &self.lua,
                h,
                &|w| matches!(w, WaitFor::Aura { key: k, .. } if *k == key),
            ) {
                self.push_error(e);
            }
        }
    }

    /// Drain the aura keys the setters asked for and the store lacked.
    pub fn take_aura_tooltip_asks(&mut self) -> Vec<AuraKey> {
        self.model_mut().aura_tooltip_asks.drain().collect()
    }

    /// The keys the setters can ask for from the VM's own state: the player's auras, every other
    /// unit's, and the tracking spell at level 1 and stack 1. The app pushes their views ahead of
    /// a hover.
    pub fn aura_tooltip_subjects(&self) -> Vec<AuraKey> {
        let model = self.model_ref();
        let auras = model
            .player_auras
            .iter()
            .chain(model.unit_auras.values().flat_map(|l| &l.auras))
            .map(|a| AuraKey {
                spell_id: a.spell_id,
                level: a.level,
                stack: a.count,
            });
        let tracking = model.tracking.iter().map(|t| tracking_key(t.spell_id));
        auras.chain(tracking).filter(|k| k.spell_id != 0).collect()
    }
}

/// `SetTrackingSpell`'s call: level 1, stack 1 (`0x532cde`-`0x532ce2`).
fn tracking_key(spell_id: u32) -> AuraKey {
    AuraKey {
        spell_id,
        level: 1,
        stack: 1,
    }
}

/// Look up an aura's view; a miss records the ask.
fn aura_view_of(lua: &Lua, key: AuraKey) -> Option<AuraTooltipView> {
    let mut model = lua.app_data_mut::<Model>().expect("model app_data");
    let v = model.aura_tooltips.get(&key).cloned();
    if v.is_none() && key.spell_id != 0 {
        model.aura_tooltip_asks.insert(key);
    }
    v
}

/// The aura builder's lines (`0x52f91e`, `0x52f96a`), then `SetPlayerBuff`'s time line.
fn render_aura(
    lua: &Lua,
    this: &Table,
    v: &AuraTooltipView,
    remaining: Option<String>,
) -> mlua::Result<()> {
    let right = v.dispel_type.clone().map(|t| (t, GOLD));
    append_line(lua, this, (v.name.clone(), GOLD), right, false)?;
    if let Some(text) = v.description.line(lua) {
        append_line(lua, this, (text, WHITE), None, true)?;
    }
    remaining_line(lua, this, remaining)
}

/// `SetPlayerBuff`'s last line, gold through `0x530380` with wrap 0 (`0x532c09`): a 1.12.1
/// screenshot reads (255, 210, 0) on it.
fn remaining_line(lua: &Lua, this: &Table, remaining: Option<String>) -> mlua::Result<()> {
    match remaining {
        Some(rem) => append_line(lua, this, (rem, GOLD), None, false),
        None => Ok(()),
    }
}

/// Fill a cleared tooltip with the aura's view, or with its name and a wait on the ask. Spell 0,
/// an empty buff record's, is the builder's miss, which leaves the tooltip empty (`0x52f8ae`);
/// the setter's time line follows either way.
pub(super) fn fill_aura(
    lua: &Lua,
    this: &Table,
    h: FrameHandle,
    key: AuraKey,
    fallback_name: Option<String>,
    remaining: Option<String>,
) -> mlua::Result<()> {
    match aura_view_of(lua, key) {
        Some(v) => render_aura(lua, this, &v, remaining)?,
        None if key.spell_id == 0 => remaining_line(lua, this, remaining)?,
        None => {
            if let Some(name) = fallback_name.clone() {
                append_line(lua, this, (name, GOLD), None, false)?;
            }
            remaining_line(lua, this, remaining.clone())?;
            insert_wait(lua, h, WaitFor::Aura { key, remaining }, fallback_name)?;
        }
    }
    super::tooltip::show_or_hide_empty(lua, h);
    Ok(())
}

/// A setter's call of the aura builder: the clear (`0x52f88d`), the lines, the show (`0x534db9`,
/// `0x532c12`, `0x532cf0`).
fn set_aura(
    lua: &Lua,
    this: &Table,
    key: AuraKey,
    fallback_name: Option<String>,
    remaining: Option<String>,
) -> mlua::Result<()> {
    let h = frame_handle_of(lua, this)?;
    clear(lua, h);
    fill_aura(lua, this, h, key, fallback_name, remaining)
}

/// Register the aura setters on the GameTooltip method table.
pub(super) fn install_methods(lua: &Lua, m: &Table) -> mlua::Result<()> {
    // GameTooltip:SetPlayerBuff(buffIndex) (`0x532b00`): the aura builder on the player's buff
    // record, plus the time-remaining line only this entry appends, its text `0x52fa50`'s
    // (`tooltip::duration_text`). The argument is the 0-based cache position `GetPlayerBuff`
    // returns (`BuffFrame.lua:105`), not a filtered ordinal; one outside the cache's 48 records
    // returns at once, leaving the tooltip as it was (`GetBuffRecord 0x4e4430`, `0x532bbd`). A
    // surplus argument is ignored, as in the reference.
    m.set(
        "SetPlayerBuff",
        lua.create_function(|lua, (this, index): (Table, Value)| {
            // `is-number` else `Usage: SetPlayerBuff(buffIndex)` (`0x855258`, `0x532c2b`).
            let index = i64::from(binding_abi::number_arg(
                lua,
                index,
                "Usage: SetPlayerBuff(buffIndex)",
            )?);
            let Some(pos) = usize::try_from(index)
                .ok()
                .filter(|&pos| pos < PLAYER_BUFF_RECORDS)
            else {
                return Ok(());
            };
            let now = crate::script::clock::now(lua);
            let (key, name, remaining_ms) = {
                let model = lua.app_data_mut::<Model>().expect("model app_data");
                match model.player_auras.get(pos) {
                    Some(a) => {
                        // The gate is `untilCancelled`, not a known duration: `0x532b00` skips
                        // the line when the record's `+0xc` is set (`0x532bdf`), a DBC flag that
                        // holds before any `SMSG_UPDATE_AURA_DURATION` lands; a lapsed aura shows
                        // "0 seconds remaining".
                        let ms = (!a.until_cancelled).then(|| {
                            // The reference counts integer milliseconds off `GetTickCount`;
                            // this rounds the float clock to the nearest one.
                            ((a.expiration_time - now) * 1000.0)
                                .round()
                                .clamp(0.0, f64::from(u32::MAX)) as u32
                        });
                        let key = AuraKey {
                            spell_id: a.spell_id,
                            level: a.level,
                            stack: a.count,
                        };
                        (key, a.name.clone(), ms)
                    }
                    // An empty record: spell 0, which the builder clears and hides on
                    // (`0x52f8ae`). Deviation: no time line follows, where the reference's rides
                    // the record's `+0xc` (`0x532bdf`), a byte an empty record keeps from its
                    // history. 0 after `ClearBuffRecord 0x4e4490` prints a lone "0 seconds
                    // remaining" (`0x4e447e`); else it is whatever the last occupant left, as
                    // `ResetBuffCache 0x4e40c0` and the compaction's tail reset (`0x4e4237`)
                    // call `0x4e44b0` with slot -1, which writes `+0x0` and `+0x4` alone
                    // (`0x4e44bc`-`0x4e44c0`). A line on a stale byte is a fault to not ape.
                    None => (AuraKey::default(), None, None),
                }
            };
            // `0x52fa50`'s text from the player's own strings; without them the line is skipped.
            let remaining = remaining_ms.and_then(|ms| {
                let g = lua.globals();
                super::tooltip::duration_text(ms, "SPELL_TIME_REMAINING", true, &|key| {
                    g.get::<String>(key).ok()
                })
            });
            set_aura(lua, &this, key, name, remaining)
        })?,
    )?;
    // GameTooltip:SetUnitBuff/SetUnitDebuff(unit, index [, raidFilter]) (`0x534ac0`, `0x534e30`):
    // the aura builder with no time line, the index counted within the unit's buffs (its first 32
    // aura slots) or debuffs (the 16 after them). An index out of that range, or one that finds no
    // aura, returns before the builder, leaving the tooltip as it was (`0x534bcb`, `0x534c9c`;
    // `0x534f3e`, `0x534f47`, `0x53501d`).
    type SetterArgs = (Table, Value, Value, Option<Value>);
    for (verb, usage, helpful, slots) in [
        (
            "SetUnitBuff",
            r#"Usage: SetUnitBuff("unit", index [,raidFilter])"#,
            true,
            32,
        ),
        (
            "SetUnitDebuff",
            r#"Usage: SetUnitDebuff("unit", index [,raidFilter])"#,
            false,
            16,
        ),
    ] {
        m.set(
            verb,
            lua.create_function(move |lua, (this, token, index, raid_filter): SetterArgs| {
                // `is-string` on the unit and `is-number` on the index, both before either is
                // read, else the usage line (`0x534b5c`, `0x534b70`, `0x534dd2`; `0x855528` for
                // the debuff).
                let (Some(token), Some(index)) =
                    (lua.coerce_string(token)?, lua.coerce_number(index)?)
                else {
                    return Err(mlua::Error::RuntimeError(usage.into()));
                };
                let token = token.to_string_lossy();
                // The token resolves before the index is read (`0x534b8b`), as `UnitBuff`'s does,
                // so an unknown one raises at any index.
                let hit = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    let list = crate::script::aura::auras_of(&model, &token)?;
                    // `_ftol`, then `index - 1` compared unsigned, so 0 and a negative index are
                    // past the range (`0x534bc4`).
                    let ordinal = binding_abi::ftol(index).wrapping_sub(1) as u32;
                    if ordinal >= slots {
                        return Ok(());
                    }
                    // Every aura of the half, past `UnitBuff`'s unit gate, which this walk lacks; a
                    // set `raidFilter` (`0x534bda`) counts the kept ones alone (`0x534c6c`).
                    let raid = crate::script::aura::raid_filter_arg(raid_filter.as_ref());
                    list.and_then(|l| {
                        l.auras
                            .iter()
                            .filter(|a| a.helpful == helpful && (!raid || a.raid_listed))
                            .nth(ordinal as usize)
                    })
                    .map(|a| {
                        let key = AuraKey {
                            spell_id: a.spell_id,
                            level: a.level,
                            stack: a.count,
                        };
                        (key, a.name.clone())
                    })
                };
                let Some((key, name)) = hit else {
                    return Ok(());
                };
                set_aura(lua, &this, key, name, None)
            })?,
        )?;
    }
    // GameTooltip:SetTrackingSpell() (`0x532c50`): the aura builder on the tracking spell
    // (`0xbc6378`) at level 1 and stack 1 (`0x532cde`-`0x532ce7`). With none it returns at once,
    // leaving the tooltip as it was (`0x532cda`).
    m.set(
        "SetTrackingSpell",
        lua.create_function(|lua, this: Table| {
            let hit = {
                let model = lua.app_data_ref::<Model>().expect("model app_data");
                model
                    .tracking
                    .as_ref()
                    .map(|t| (t.spell_id, t.name.clone()))
            };
            let Some((spell_id, name)) = hit.filter(|(id, _)| *id != 0) else {
                return Ok(());
            };
            set_aura(lua, &this, tracking_key(spell_id), name, None)
        })?,
    )?;
    Ok(())
}
