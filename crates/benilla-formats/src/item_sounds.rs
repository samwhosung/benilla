//! `ItemGroupSounds.dbc`: an item's pickup, put-down and use sounds. A row is an id and four
//! `SoundEntries` ids indexed by the client's gesture (`0x458024`); no caller reads the fourth.
//! An item reaches its group through `ItemDisplayInfo.field11`, and a 0 slot is silent.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, u32_at};

const ITEM_GROUP_SOUNDS: &str = "DBFilesClient\\ItemGroupSounds.dbc";

/// The gesture index `SndInterfacePlayItemSound`'s callers pass in `ecx` (`0x457ff0`, `0x457fb0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemGesture {
    /// Grabbed onto the cursor.
    Pickup = 0,
    /// Placed, or the cursor cleared.
    PutDown = 1,
    /// Used; only groups 1, 2 and 4-6 have a kit for it.
    Use = 2,
}

/// `ItemGroupSounds.dbc`, keyed by group id (`ItemDisplayInfo.field11`).
pub struct ItemGroupSoundsCatalog {
    groups: HashMap<u32, [u32; 4]>,
}

impl ItemGroupSoundsCatalog {
    /// The `SoundEntries` id for a group's gesture; an unknown group (`0x45800f`, `0x45801d`) or a
    /// 0 slot plays nothing in the client.
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

    /// Shipped rows: group 1 has a use kit, group 7 (weapon and armor) has none.
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
