//! GameObject display resolution: `displayId` → world model path.
//!
//! A GameObject's `GAMEOBJECT_DISPLAYID` indexes **GameObjectDisplayInfo.dbc**, whose `modelName`
//! column is a direct path — mostly `.mdx`/`.mdl` (M2: chests, mailboxes, doodads) with a few `.wmo`
//! (large structures). Unlike creatures there's no model-data indirection or skins, so the catalog is
//! just `displayId → path`; the renderer dispatches the path through [`crate::load_object_model`].
//!
//! Layout verified against build 5875: 12 fields (ID@0, modelName@1 string, Sound[10]@2..11).

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
    /// The model path for a GameObject display id (`.mdx`/`.mdl`/`.wmo`), or `None`.
    pub fn model_path(&self, display_id: u32) -> Option<&str> {
        self.models.get(&display_id).map(String::as_str)
    }

    /// Number of display entries (diagnostics).
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Every `(displayId, model path)` pair, unordered — the corpus-sweep entry point for "which
    /// models can a GameObject ever be, and which display ids reach each one".
    pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> {
        self.models.iter().map(|(&id, p)| (id, p.as_str()))
    }
}

/// GameObjectDisplayInfo.dbc — 12 fields in build 5875: ID, modelName, then 10 sound refs.
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

/// The per-display **sound-kit slots** (`Sound0..9` — the other 10 columns of the same table):
/// `{0 Stand, 1 Open, 2 Loop, 3 Close, 4 Destroy, 5 Opened, 6..9 Custom}` → SoundEntries.
/// Loaded separately from the model catalog so the audio consumer doesn't reach into the
/// renderer's cache; only displays with at least one non-zero slot are kept
/// (most of the 1638 rows are silent props).
pub struct GameObjectSounds {
    sounds: HashMap<u32, [u32; 10]>,
}

/// The `Sound[10]` slot indices with recorded meanings (wowdev, vanilla family).
pub mod go_sound_slot {
    pub const OPEN: usize = 1;
    pub const CLOSE: usize = 3;
}

impl GameObjectSounds {
    /// The 10 sound-kit slots for a display id; `None` when the display has no sounds at all.
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
