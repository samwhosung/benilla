//! `ItemGroupSounds.dbc` adapter — the per-item **pickup / put-down / use** sound groups
//! (decision 0091: the bag-drag item sounds).
//!
//! Layout — build 5875: **5 fields × 4 = 20 B**
//! per record (loader `0x5477d0` asserts fieldCount 5 @`0x547879`, recordSize 0x14 @`0x5478ae`):
//! `{ id, kit[0], kit[1], kit[2], kit[3] }` — the kits are `SoundEntries.dbc` ids, indexed by the
//! client's **gesture**: `kit[0]` pickup/grab, `kit[1]` put-down/place, `kit[2]` use/activate
//! (`0x458024: mov ecx,[eax+4*ecx+0x4]`); `kit[3]` is unused by any caller. 24 rows in the real
//! DBC. An item reaches its group through `ItemDisplayInfo.field11`
//! ([`crate::items::ItemDisplay::group_sounds`]); a `0` kit slot means the gesture is silent —
//! the client's play tail drops kit id 0.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

const ITEM_GROUP_SOUNDS: &str = "DBFilesClient\\ItemGroupSounds.dbc";

/// The client's gesture index into an [`ItemGroupSoundsCatalog`] row (the `ecx` every
/// `SndInterfacePlayItemSound` caller passes, `0x457ff0`/`0x457fb0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemGesture {
    /// `ecx = 0` — item grabbed onto the cursor.
    Pickup = 0,
    /// `ecx = 1` — item placed / put down / cursor cleared.
    PutDown = 1,
    /// `ecx = 2` — item used/activated (only groups 1–6 populate it).
    Use = 2,
}

/// `ItemGroupSounds.dbc`, keyed by group id (`ItemDisplayInfo.field11`).
pub struct ItemGroupSoundsCatalog {
    groups: HashMap<u32, [u32; 4]>,
}

impl ItemGroupSoundsCatalog {
    /// The `SoundEntries` kit for a group's gesture — `None` when the group id is unknown or the
    /// slot is `0` (both are the client's silent returns: the bounds/null checks at
    /// `0x45800f`/`0x45801d`, and the play tail dropping kit 0).
    pub fn kit(&self, group: u32, gesture: ItemGesture) -> Option<u32> {
        self.groups
            .get(&group)
            .map(|kits| kits[gesture as usize])
            .filter(|&k| k != 0)
    }

    pub fn len(&self) -> usize {
        self.groups.len()
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

fn item_group_sounds_schema() -> Schema {
    let mut s = Schema::new("ItemGroupSounds");
    for name in ["ID", "Pickup", "PutDown", "Use", "Unused3"] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Load `ItemGroupSounds.dbc` off the patch chain.
pub fn load_item_group_sounds(chain: &mut Chain) -> Result<ItemGroupSoundsCatalog> {
    let bytes = chain
        .read_file(ITEM_GROUP_SOUNDS)
        .with_context(|| format!("reading {ITEM_GROUP_SOUNDS}"))?;
    let rs = parse(&bytes, item_group_sounds_schema(), "ItemGroupSounds")?;
    let mut groups = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        groups.insert(id, std::array::from_fn(|i| u32_at(r, 1 + i).unwrap_or(0)));
    }
    Ok(ItemGroupSoundsCatalog { groups })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real 5875 rows: 24 groups; id 1 → kits [273, 274, 275, 0] (a group with a use kit),
    /// id 7 → [1185, 1202, 0, 0] (a weapon/armor group, no use kit — its `Use` gesture resolves
    /// silent).
    #[test]
    fn real_item_group_sounds_resolve() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_item_group_sounds(&mut chain).expect("load ItemGroupSounds");
        assert_eq!(cat.len(), 24, "24 groups in build 5875");
        assert_eq!(cat.kit(1, ItemGesture::Pickup), Some(273));
        assert_eq!(cat.kit(1, ItemGesture::PutDown), Some(274));
        assert_eq!(cat.kit(1, ItemGesture::Use), Some(275));
        assert_eq!(cat.kit(7, ItemGesture::Pickup), Some(1185));
        assert_eq!(cat.kit(7, ItemGesture::PutDown), Some(1202));
        assert_eq!(cat.kit(7, ItemGesture::Use), None, "a 0 slot is silent");
        assert_eq!(cat.kit(999, ItemGesture::Pickup), None, "unknown group");
    }

    /// The display→group→kit join holds on real data: every nonzero `ItemDisplayInfo.field11`
    /// (`group_sounds`) is a valid group id — 20513/20513, through our own two adapters.
    #[test]
    fn real_display_group_ids_all_resolve() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let sounds = load_item_group_sounds(&mut chain).expect("load ItemGroupSounds");
        let displays = crate::load_item_display_catalog(&mut chain).expect("load ItemDisplayInfo");
        let mut nonzero = 0usize;
        let mut valid = 0usize;
        for (_, d) in displays.iter() {
            if d.group_sounds != 0 {
                nonzero += 1;
                if sounds.groups.contains_key(&d.group_sounds) {
                    valid += 1;
                }
            }
        }
        assert_eq!(nonzero, 20513, "the nonzero field-11 count");
        assert_eq!(valid, nonzero, "every nonzero group id resolves");
    }
}
