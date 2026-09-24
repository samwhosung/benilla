//! NPC greeting voice — the vocal an NPC plays when the player interacts with it ("Greetings,
//! traveler", race/sex/personality-flavored). A per-**display** property, distinct from the
//! creature-body vocals of [`crate::creature_sound`]:
//!
//! Chain — VERIFIED against build 5875, twice over: this session's raw DBC decode (2026-07-04)
//! AND the binary's bytes (the loaders check exactly these layouts: NPCSounds `0x54afa0`
//! cols==5/recsize==0x14, CreatureDisplayInfo `0x542e90` cols==12/recsize==0x30; the play path
//! reads row`+0x2c`):
//! `UNIT_FIELD_DISPLAYID` → `CreatureDisplayInfo.dbc` **field[11]** (the last field, `NPCSoundID`)
//! → `NPCSounds.dbc` row → **field[1]** (`hello`) / **field[2]** (`goodbye`) / **field[3]**
//! (`pissed`, the variation-overflow line) — each a `SoundEntries` kit id, played through the
//! shared kit player. NOT the `CreatureSoundData.NPCSoundID` the older wowdev notes suggest: that
//! column (field 22) is **0/406 in 1.12** — entirely unused, and the reference confirms
//! CreatureSoundData is not on the greeting path (`0x60c3b0`, `0x623910`) at all.
//!
//! Byte-census of the real tables:
//! - `CreatureDisplayInfo.dbc`: 10534 × 12 fields. field[11] is nonzero on **4509** displays, and
//!   **every** nonzero value (4509/4509) is a valid `NPCSounds` id (distinct 147/156). A beast
//!   display (e.g. id 26) carries field[11]=0 → no greeting; only humanoid/character displays greet.
//! - `NPCSounds.dbc`: 156 × 5 fields (recsize 20), layout `{id(0), hello(1), goodbye(2), pissed(3),
//!   ack(4)}`. `hello` is set on all 156 rows; `goodbye` on 118; `pissed` on 120; `ack` is **0 on
//!   every row** in 1.12 (and the binary never reads it: neither of the store's two readers,
//!   `0x60c3b0` and `0x623910`, touches `+0x10`). The kits are all `SoundEntries`
//!   type 17 (NPC greeting), flags `0x20` (no-duplicates), distance-cutoff 45 yd — a 3D world
//!   emitter at the NPC, not a 2D sound (`0x7a5b10`: `FSOUND_3D_SetAttributes` with the unit's
//!   position).

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

/// The greeting kit set for one NPC (each a `SoundEntries` kit id; `0` = none).
#[derive(Clone, Copy, Debug)]
pub struct NpcGreeting {
    /// `NPCSounds.hello` — the greeting played on interact / target enter.
    pub hello: u32,
    /// `NPCSounds.goodbye` — the farewell, played when the tracked interaction unit clears
    /// (`0x60c3b0(0)` on mouseover/target leave — no discrete gossip-close callback).
    pub goodbye: u32,
    /// `NPCSounds.pissed` — the annoyed line the variation-overflow branch plays once repeat
    /// interacts have cycled past the hello kit's variations (`0x623910`'s overflow reads
    /// row`+0xc`).
    pub pissed: u32,
}

/// display id → greeting pair, joined `CreatureDisplayInfo.field[11]` → `NPCSounds`.
pub struct NpcGreetingCatalog {
    display_to_sound: HashMap<u32, u32>,
    rows: HashMap<u32, NpcGreeting>,
}

impl NpcGreetingCatalog {
    /// The greeting for a creature **display id** (the [`crate`] wire `display_id`), or `None` when
    /// the display carries no `NPCSoundID` (beasts and most non-character models) or the referenced
    /// `NPCSounds` row is missing.
    pub fn for_display(&self, display_id: u32) -> Option<&NpcGreeting> {
        self.rows.get(self.display_to_sound.get(&display_id)?)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// `NPCSounds.dbc` — 5 uint32 fields.
fn npcsounds_schema() -> Schema {
    let mut s = Schema::new("NPCSounds");
    for i in 0..5 {
        s.add_field(SchemaField::new(format!("f{i}"), FieldType::UInt32));
    }
    s
}

/// `CreatureDisplayInfo.dbc` — 12 fields (field[11] = `NPCSoundID`).
fn cdi_schema() -> Schema {
    let mut s = Schema::new("CreatureDisplayInfo");
    for i in 0..12 {
        s.add_field(SchemaField::new(format!("f{i}"), FieldType::UInt32));
    }
    s
}

/// Read `NPCSounds.dbc` + `CreatureDisplayInfo.dbc` off the patch chain into the joined catalog.
pub fn load_npc_greeting_catalog(chain: &mut Chain) -> Result<NpcGreetingCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\NPCSounds.dbc")
        .context("reading NPCSounds.dbc")?;
    let rs = parse(&bytes, npcsounds_schema(), "NPCSounds")?;
    let mut rows = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        rows.insert(
            id,
            NpcGreeting {
                hello: u32_at(r, 1).unwrap_or(0),
                goodbye: u32_at(r, 2).unwrap_or(0),
                pissed: u32_at(r, 3).unwrap_or(0),
            },
        );
    }

    let bytes = chain
        .read_file("DBFilesClient\\CreatureDisplayInfo.dbc")
        .context("reading CreatureDisplayInfo.dbc")?;
    let rs = parse(&bytes, cdi_schema(), "CreatureDisplayInfo")?;
    let mut display_to_sound = HashMap::new();
    for r in rs.records() {
        let (Some(id), Some(npc_sound)) = (u32_at(r, 0), u32_at(r, 11)) else {
            continue;
        };
        if npc_sound != 0 {
            display_to_sound.insert(id, npc_sound);
        }
    }
    Ok(NpcGreetingCatalog {
        display_to_sound,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The join works on the real 5875 tables: all 156 `NPCSounds` rows load, thousands of
    /// displays resolve a greeting, a known character display resolves to a real greeting kit, and
    /// a beast display (26, `NPCSoundID` 0) has no greeting.
    #[test]
    fn real_npc_greeting_resolves() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_npc_greeting_catalog(&mut chain).expect("load npc greetings");
        assert_eq!(cat.len(), 156, "all NPCSounds rows load");

        // Display 793 → NPCSoundID 50 → {hello 5977, goodbye 5978, pissed 5979} (byte-decoded).
        let g = cat.for_display(793).expect("display 793 greets");
        assert_eq!(g.hello, 5977);
        assert_eq!(g.goodbye, 5978);
        assert_eq!(g.pissed, 5979);

        // Display 89 → NPCSoundID 161 → {hello 7094, no goodbye, pissed 7095}.
        let g = cat.for_display(89).expect("display 89 greets");
        assert_eq!(g.hello, 7094);
        assert_eq!(g.goodbye, 0);
        assert_eq!(g.pissed, 7095);

        // A beast model display carries NPCSoundID 0 → no greeting.
        assert!(
            cat.for_display(26).is_none(),
            "beast display does not greet"
        );
    }
}
