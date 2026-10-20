//! The studied-creature block of the unit tooltip, `0x529a40`: what Beast Lore reveals. The
//! builder adds it after `RESURRECTABLE` for a unit with `UNIT_DYNAMIC_FLAGS` bit 4 (`0x52a8ea`):
//! the damage, armor and health lines, one line per other resistance above 0, the diet, then
//! `TAMEABLE` and the pet abilities, or `NOT_TAMEABLE`. Every line is white (`0xc0cf60`).

use mlua::{Lua, Table};

use super::tooltip::append_line;
use crate::strings::{fill, Arg};

/// `UNIT_DYNAMIC_FLAGS` bit 4, `UNIT_DYNFLAG_SPECIALINFO`, which the server sets on a unit the
/// player has studied (`0x52a8ea`).
pub(super) const UNIT_DYNFLAG_SPECIALINFO: u32 = 0x10;

/// What the block reads of a studied unit beyond its health, resolved by the host.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StudiedCreature {
    /// `UNIT_FIELD_MINDAMAGE` (`fields+0x200`).
    pub min_damage: f32,
    /// `UNIT_FIELD_MAXDAMAGE` (`fields+0x204`).
    pub max_damage: f32,
    /// `UNIT_FIELD_RESISTANCES` (`fields+0x254`-`+0x26c`), by school.
    pub resistances: [i32; 7],
    /// The school whose slot is armor, `Resistances.dbc`'s flagged row (`0x53f830`), which the
    /// resistance lines skip (`0x529b76`); `None` with no row flagged.
    pub armor_school: Option<u8>,
    /// The dword at the armor school's slot (`0x529ae6`): with no school flagged the index is -1,
    /// the dword before the resistances.
    pub armor: i32,
    /// The diet's food names in `ItemPetFood.dbc` record order (`0x529c39`-`0x529cd1`); empty, no
    /// line.
    pub diet: Vec<String>,
    /// `None` for a creature that is not tameable (`0x529d1d`-`0x529d32`): no record, its type
    /// flags' bit 0 clear, or no family. Else the abilities its `CreatureSpellData.dbc` row names
    /// (`0x529d56`-`0x529e47`), each spell's name and rank; empty, no ability line.
    pub tameable: Option<Vec<PetAbility>>,
}

/// One pet ability of a studied creature: `Spell.dbc`'s name and rank (`0x529de5`, `0x529e04`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PetAbility {
    pub name: String,
    /// Empty is no rank.
    pub rank: String,
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The separator `0x84480c` between foods and between abilities.
const SEPARATOR: &str = ", ";

/// Add the block's lines for a studied unit whose raw `UNIT_FIELD_HEALTH` is `health`.
pub(super) fn studied_lines(
    lua: &Lua,
    this: &Table,
    health: u32,
    s: &StudiedCreature,
) -> mlua::Result<()> {
    let line = |key: &str, args: &[Arg<'_>]| -> mlua::Result<()> {
        match crate::strings::global(lua, key) {
            Some(template) => append_line(lua, this, (fill(&template, args), WHITE), None, false),
            None => Ok(()),
        }
    };
    // The minimum truncated, the maximum rounded up (`__ftol`, `ceil`; `0x529a55`-`0x529a7b`);
    // one template when they meet (`0x529a83`).
    let min = s.min_damage as i32;
    let max = f64::from(s.max_damage).ceil() as i32;
    if min == max {
        line("SINGLE_DAMAGE_TEMPLATE", &[Arg::D(min.into())])?;
    } else {
        line("DAMAGE_TEMPLATE", &[Arg::D(min.into()), Arg::D(max.into())])?;
    }
    // The armor, 0 when not above 0 (`0x529aed`-`0x529b08`).
    line("ARMOR_TEMPLATE", &[Arg::D(s.armor.max(0).into())])?;
    line("HP_TEMPLATE", &[Arg::D((health as i32).into())])?;
    // Each other school above 0 with a non-empty `RESISTANCE%d_NAME` (`0x529b71`-`0x529bf8`).
    for (i, &value) in s.resistances.iter().enumerate() {
        if s.armor_school == Some(i as u8) || value <= 0 {
            continue;
        }
        let Some(school) = crate::strings::global(lua, &format!("RESISTANCE{i}_NAME")) else {
            continue;
        };
        line(
            "RESISTANCE_TEMPLATE",
            &[Arg::D(value.into()), Arg::S(&school)],
        )?;
    }
    if !s.diet.is_empty() {
        line("PET_DIET_TEMPLATE", &[Arg::S(&s.diet.join(SEPARATOR))])?;
    }
    let Some(abilities) = &s.tameable else {
        return line("NOT_TAMEABLE", &[]);
    };
    line("TAMEABLE", &[])?;
    if abilities.is_empty() {
        return Ok(());
    }
    // Each name, then its rank in `" (%s)"` (`0x854820`) when it has one (`0x529e0b`).
    let named: Vec<String> = abilities
        .iter()
        .map(|a| match a.rank.as_str() {
            "" => a.name.clone(),
            rank => format!("{} ({rank})", a.name),
        })
        .collect();
    line("PET_SPELLS_TEMPLATE", &[Arg::S(&named.join(SEPARATOR))])
}
