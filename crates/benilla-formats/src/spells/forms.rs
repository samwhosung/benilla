//! `SpellShapeshiftForm.dbc`: the per-form rows the action bar, the form gate, the spell tooltip
//! and tracking read.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, str_at, u32_at};

const SHAPESHIFT_FORM: &str = "DBFilesClient\\SpellShapeshiftForm.dbc";

/// One `SpellShapeshiftForm.dbc` row's consumed fields.
#[derive(Clone, Debug, Default)]
pub struct ShapeshiftForm {
    /// `BonusActionBar` (column 1): the page the action bar flips to, 0 for none.
    pub bonus_bar: u32,
    /// The enUS name (column 2), the tooltip's `SPELL_REQUIRED_FORM` "Requires %s"
    /// (`0x52f10a`-`0x52f2ae`).
    pub name: String,
    /// `flags1` (column 11): bit 0 marks a stance, bit 1 blocks cancelling the active form.
    pub flags: u32,
    /// `creatureType` (column 12): the form's creature-type override, which the resolver
    /// (`0x605570`) reads before the creature template or race, so a cat-form druid tracks as a
    /// Beast; `<= 0` falls back to Humanoid.
    pub creature_type: i32,
    /// `AttackIconID` (column 13) through `SpellIcon.dbc`: the Attack action shows the current
    /// form's icon before the main-hand weapon's (`0x4e6870`); `None` falls through to the weapon.
    pub attack_icon: Option<String>,
}

impl ShapeshiftForm {
    /// A stance (vmangos `SHAPESHIFT_FLAG_STANCE`: warrior stances, Stealth), which the form gate
    /// ([`crate::spells::SpellDisplay::usable_in_form`]) does not count as shapeshifted.
    pub fn is_stance(&self) -> bool {
        self.flags & 1 != 0
    }

    /// Clicking the active form's button cancels its aura (`CMSG_CANCEL_AURA`) unless bit `0x2`
    /// makes it a silent no-op (`CastShapeshiftForm` `0x4b4810`, guard at `0x4b4963`); the
    /// warrior stances (`0x7`) set it on 5875.
    pub fn cancelable(&self) -> bool {
        self.flags & 0x2 == 0
    }
}

/// Load `SpellShapeshiftForm.dbc` by form id. The bonus bar is data, not a stance switch:
/// `GetBonusBarOffset` (`0x4e7620`) returns what the `UPDATE_BONUS_ACTIONBAR` handler (`0x4e4fc0`)
/// read from column 1 for the player's form. `flags1` feeds the form gate (`0x612480`).
pub fn load_shapeshift_forms(chain: &mut Chain) -> Result<HashMap<u32, ShapeshiftForm>> {
    let bytes = chain
        .read_file(SHAPESHIFT_FORM)
        .context("reading SpellShapeshiftForm.dbc")?;
    let mut schema = Schema::new("SpellShapeshiftForm");
    for i in 0..14 {
        match i {
            // Column 2: the enUS slot of the name locstring (2..10, eight locales and a flag word).
            2 => schema.add_field(SchemaField::new("Name", FieldType::String)),
            _ => schema.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32)),
        }
    }
    let set = parse(&bytes, schema, "SpellShapeshiftForm.dbc")?;
    // AttackIconID resolves through SpellIcon.dbc like a spell's own icon (`0x4e68af`-`0x4e68da`).
    let icons = crate::dbc::load_spell_icon_map(chain)?;
    let mut map = HashMap::new();
    for r in set.records() {
        if let Some(id) = u32_at(r, 0) {
            map.insert(
                id,
                ShapeshiftForm {
                    bonus_bar: u32_at(r, 1).unwrap_or(0),
                    name: str_at(&set, r, 2).unwrap_or_default(),
                    flags: u32_at(r, 11).unwrap_or(0),
                    creature_type: i32_at(r, 12).unwrap_or(0),
                    attack_icon: u32_at(r, 13)
                        .filter(|&i| i != 0)
                        .and_then(|i| icons.get(&i).cloned()),
                },
            );
        }
    }
    Ok(map)
}
