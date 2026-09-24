//! `SpellShapeshiftForm.dbc` — bonus bars + the form gate's stance flag: the per-form
//! `BonusActionBar` page the action bar flips to on shifting, and the `flags1` stance bit
//! ([`ShapeshiftForm::is_stance`]) [`crate::spells::SpellDisplay::usable_in_form`] reads to tell a
//! true shapeshift (blocks NOT_SHAPESHIFT spells) from a stance (doesn't).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{i32_at, parse, str_at, u32_at};

const SHAPESHIFT_FORM: &str = "DBFilesClient\\SpellShapeshiftForm.dbc";

/// One `SpellShapeshiftForm.dbc` row's consumed fields.
#[derive(Clone, Debug, Default)]
pub struct ShapeshiftForm {
    /// **BonusActionBar** (field 1) — the stance page the bar flips to (0 = none).
    pub bonus_bar: u32,
    /// **Name** (the locstring at fields 2..10; enUS slot) — "Battle Stance", "Cat Form" —
    /// the `SPELL_REQUIRED_FORM` "Requires %s" cell of the spell tooltip (decision 0276;
    /// `0x52f10a`–`0x52f2ae`).
    pub name: String,
    /// `flags1` (field 11, vmangos `SpellShapeshiftFormEntry`). Bit 0 = the form is a *stance*
    /// (warrior stances, stealth): it does NOT count as "shapeshifted" for the form gate —
    /// vmangos `SHAPESHIFT_FLAG_STANCE`, the `actAsShifted` fork of
    /// [`SpellDisplay::usable_in_form`].
    pub flags: u32,
    /// **creatureType** (field 12, int32 — the `+0x30` read): the form's creature-type OVERRIDE.
    /// The client's creature-type
    /// resolver (`0x605570`) reads a shapeshifted unit's type from HERE before the creature
    /// template or race table — a cat-form druid is a Beast (1) to the minimap tracking
    /// predicates. `<= 0` reads Humanoid (the resolver's fallback; vmangos's own row comment).
    /// Consumed by the tracking dots (decision 0564).
    pub creature_type: i32,
    /// **AttackIconID** (field 13, the `+0x34` read): the form's own attack icon, resolved through
    /// `SpellIcon.dbc` at load. The Attack action's icon resolver (`0x4e6870`) serves the
    /// CURRENT form's icon before the main-hand weapon's, on both the action bar and the
    /// spellbook. `None` = column 0, no form icon — fall through to the weapon (5875 data:
    /// warrior stances and bear forms carry one; Ghost Wolf and Moonkin are 0).
    pub attack_icon: Option<String>,
}

impl ShapeshiftForm {
    /// `flags1 & 1` — see [`ShapeshiftForm::flags`].
    pub fn is_stance(&self) -> bool {
        self.flags & 1 != 0
    }

    /// Clicking this form's stance button while it is ACTIVE cancels the form aura
    /// (`CMSG_CANCEL_AURA`) — unless `flags1` bit `0x2` blocks it (`CastShapeshiftForm
    /// 0x4b4810`'s guard at `0x4b4963`: active + bit
    /// set = silent no-op). 5875: the warrior stances carry `0x7` (blocked — you never cancel
    /// OUT of a stance); druid forms (0x70/0x50), Stealth (0x1), Moonkin (0x41), Shadowform
    /// (0x9), Ghost Wolf (0x40) all cancel.
    pub fn cancelable(&self) -> bool {
        self.flags & 0x2 == 0
    }
}

/// `SpellShapeshiftForm.dbc` → form id → the consumed row ([`ShapeshiftForm`]).
///
/// **BonusActionBar** is the client's own bar mapping
/// (`GetBonusBarOffset 0x4e7620` returns a cached global that the `UPDATE_BONUS_ACTIONBAR`
/// handler `0x4e4fc0` fills by indexing this DBC by the player's shapeshift form and reading
/// `rec->field[1]` — data, not a stance switch). 5875 values: Cat(1)→1, Bear(5)/DireBear(8)→3,
/// Battle(17)→1, Defensive(18)→2, Berserker(19)→3, Stealth(30)→1, everything else 0.
/// **flags1** feeds the form gate (`0x612480`, [`SpellDisplay::usable_in_form`]).
/// 32 records × 14 fields (56 B), verified on the extracted file.
pub fn load_shapeshift_forms(chain: &mut Chain) -> Result<HashMap<u32, ShapeshiftForm>> {
    let bytes = chain
        .read_file(SHAPESHIFT_FORM)
        .context("reading SpellShapeshiftForm.dbc")?;
    let mut schema = Schema::new("SpellShapeshiftForm");
    for i in 0..14 {
        match i {
            // Field 2: the name locstring's enUS slot (fields 2..10 = 8 locales + flag word).
            2 => schema.add_field(SchemaField::new("Name", FieldType::String)),
            _ => schema.add_field(SchemaField::new(format!("F{i}"), FieldType::UInt32)),
        }
    }
    let set = parse(&bytes, schema, "SpellShapeshiftForm.dbc")?;
    // The AttackIconID column resolves through SpellIcon.dbc like a spell's own icon does
    // (`0x4e6870`'s `0x4e68af`–`0x4e68da` id → path hop). 1033 tiny records — loading the map
    // here keeps every call site's signature (six of them) untouched.
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
