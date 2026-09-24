//! `GameObjectDisplayInfo.dbc`: a game object's `GAMEOBJECT_DISPLAYID` → a direct model path,
//! with no model-data indirection or skins; mostly `.mdx`/`.mdl`, a few `.wmo`.

use std::collections::HashMap;

use crate::Chain;
use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};

const GAMEOBJECT_DISPLAY_INFO: &str = "DBFilesClient\\GameObjectDisplayInfo.dbc";

/// `displayId → model path` from GameObjectDisplayInfo.dbc.
pub struct GameObjectCatalog {
    models: HashMap<u32, String>,
}

impl GameObjectCatalog {
    pub fn model_path(&self, display_id: u32) -> Option<&str> {
        self.models.get(&display_id).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Every `(displayId, model path)` pair, in no order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> {
        self.models.iter().map(|(&id, p)| (id, p.as_str()))
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("GameObjectDisplayInfo");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("ModelName", FieldType::String));
    for i in 0..10 {
        s.add_field(SchemaField::new(format!("Sound{i}"), FieldType::UInt32));
    }
    s
}

/// Load GameObjectDisplayInfo.dbc from the patch chain into a [`GameObjectCatalog`].
pub fn load_gameobject_catalog(chain: &mut Chain) -> Result<GameObjectCatalog> {
    let bytes = chain
        .read_file(GAMEOBJECT_DISPLAY_INFO)
        .with_context(|| format!("reading {GAMEOBJECT_DISPLAY_INFO}"))?;
    let rs = parse(&bytes, schema(), "GameObjectDisplayInfo")?;
    let mut models = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        if let (Some(id), Some(path)) = (u32_at(r, 0), str_at(&rs, r, 1)) {
            models.insert(id, path);
        }
    }
    Ok(GameObjectCatalog { models })
}

/// Each display's ten `SoundEntries` slots (columns 2-11): 0 stand, 1 open, 2 loop, 3 close,
/// 4 destroy, 5 opened, 6-9 custom. Only displays with a nonzero slot are kept.
pub struct GameObjectSounds {
    sounds: HashMap<u32, [u32; 10]>,
}

/// `Sound[10]` slot indices, named as wowdev documents them.
pub mod go_sound_slot {
    pub const OPEN: usize = 1;
    pub const CLOSE: usize = 3;
}

impl GameObjectSounds {
    /// A display's ten slots; `None` when every slot is zero.
    pub fn slots(&self, display_id: u32) -> Option<&[u32; 10]> {
        self.sounds.get(&display_id)
    }

    pub fn len(&self) -> usize {
        self.sounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sounds.is_empty()
    }
}

/// Load the sound slots off GameObjectDisplayInfo.dbc.
pub fn load_gameobject_sounds(chain: &mut Chain) -> Result<GameObjectSounds> {
    let bytes = chain
        .read_file(GAMEOBJECT_DISPLAY_INFO)
        .with_context(|| format!("reading {GAMEOBJECT_DISPLAY_INFO}"))?;
    let rs = parse(&bytes, schema(), "GameObjectDisplayInfo")?;
    let mut sounds = HashMap::new();
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let mut slots = [0u32; 10];
        for (i, slot) in slots.iter_mut().enumerate() {
            *slot = u32_at(r, 2 + i).unwrap_or(0);
        }
        if slots.iter().any(|&s| s != 0) {
            sounds.insert(id, slots);
        }
    }
    Ok(GameObjectSounds { sounds })
}
