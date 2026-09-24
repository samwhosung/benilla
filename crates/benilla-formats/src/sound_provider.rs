//! `SoundProviderPreferences.dbc`: the EAX reverb presets that `AreaTable` columns 5 and 6 (dry,
//! underwater) and `WMOAreaTable` columns 4 and 5 point at. Levels are millibels, times seconds;
//! rows 66-92 carry the EAX SDK's published presets byte for byte, which pins the column meanings.
//! The EAX3 columns (16-23) are near-constant in 1.12 and not parsed.

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{f32_at, parse, str_at, u32_at};
use crate::Chain;

/// One reverb preset's EAX2 listener properties.
pub struct SoundProvider {
    pub id: u32,
    /// "PRESET_CAVE", "Underwater", …; for display only.
    pub name: String,
    pub flags: u32,
    /// Decay time in seconds (EAX range 0.1-20).
    pub decay_time: f32,
    /// Room effect level in mB (-10000..0).
    pub room: i32,
    /// Room high-frequency level in mB (-10000..0), the muffle of the wet signal.
    pub room_hf: i32,
    /// High-frequency to overall decay ratio (0.1-2); below 1 the highs die faster.
    pub decay_hf_ratio: f32,
    /// Early reflections level in mB (-10000..1000).
    pub reflections: i32,
    /// Late reverberation level in mB (-10000..2000).
    pub reverb: i32,
    /// Environment diffusion (0-1): low is echoey, high is smooth.
    pub env_diffusion: f32,
    /// EAX environment size (1-100), roughly metres.
    pub env_size: f32,
}

/// All presets by id.
pub struct SoundProviderCatalog {
    providers: HashMap<u32, SoundProvider>,
}

impl SoundProviderCatalog {
    pub fn get(&self, id: u32) -> Option<&SoundProvider> {
        self.providers.get(&id)
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("SoundProviderPreferences");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Description", FieldType::String));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    s.add_field(SchemaField::new(
        "EAXEnvironmentSelection",
        FieldType::UInt32,
    ));
    s.add_field(SchemaField::new("EAXDecayTime", FieldType::Float32));
    s.add_field(SchemaField::new("EAX2EnvironmentSize", FieldType::Float32));
    s.add_field(SchemaField::new(
        "EAX2EnvironmentDiffusion",
        FieldType::Float32,
    ));
    s.add_field(SchemaField::new("EAX2Room", FieldType::UInt32));
    s.add_field(SchemaField::new("EAX2RoomHF", FieldType::UInt32));
    s.add_field(SchemaField::new("EAX2DecayHFRatio", FieldType::Float32));
    s.add_field(SchemaField::new("EAX2Reflections", FieldType::UInt32));
    s.add_field(SchemaField::new("EAX2ReflectionsDelay", FieldType::Float32));
    s.add_field(SchemaField::new("EAX2Reverb", FieldType::UInt32));
    s.add_field(SchemaField::new("EAX2ReverbDelay", FieldType::Float32));
    s.add_field(SchemaField::new("EAX2RoomRolloff", FieldType::Float32));
    s.add_field(SchemaField::new("EAX2AirAbsorption", FieldType::Float32));
    for name in [
        "EAX3RoomLF",
        "EAX3DecayLFRatio",
        "EAX3EchoTime",
        "EAX3EchoDepth",
        "EAX3ModulationTime",
        "EAX3ModulationDepth",
        "EAX3HFReference",
        "EAX3LFReference",
    ] {
        s.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    s
}

/// Read `SoundProviderPreferences.dbc` off the patch chain.
pub fn load_sound_provider_catalog(chain: &mut Chain) -> Result<SoundProviderCatalog> {
    let bytes = chain
        .read_file("DBFilesClient\\SoundProviderPreferences.dbc")
        .context("reading SoundProviderPreferences.dbc")?;
    let rs = parse(&bytes, schema(), "SoundProviderPreferences")?;
    let mut providers = HashMap::with_capacity(rs.records().len());
    for r in rs.records() {
        let Some(id) = u32_at(r, 0) else { continue };
        let i32_at = |i: usize| u32_at(r, i).unwrap_or(0) as i32;
        providers.insert(
            id,
            SoundProvider {
                id,
                name: str_at(&rs, r, 1).unwrap_or_default(),
                flags: u32_at(r, 2).unwrap_or(0),
                decay_time: f32_at(r, 4).unwrap_or(0.0),
                room: i32_at(7),
                room_hf: i32_at(8),
                decay_hf_ratio: f32_at(r, 9).unwrap_or(1.0),
                reflections: i32_at(10),
                reverb: i32_at(12),
                env_diffusion: f32_at(r, 6).unwrap_or(1.0),
                env_size: f32_at(r, 5).unwrap_or(1.0),
            },
        );
    }
    Ok(SoundProviderCatalog { providers })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PRESET_GENERIC (67) carries the published EAX SDK values; Underwater (11) cuts all highs.
    #[test]
    fn real_provider_table_decodes() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_sound_provider_catalog(&mut chain).expect("load providers");
        assert_eq!(cat.len(), 38);

        let generic = cat.get(67).expect("PRESET_GENERIC");
        assert_eq!(generic.name, "PRESET_GENERIC");
        assert!((generic.decay_time - 1.49).abs() < 1e-3);
        assert_eq!(generic.room, -1000);
        assert_eq!(generic.room_hf, -100);
        assert_eq!(generic.reflections, -2602);
        assert_eq!(generic.reverb, 200);

        let underwater = cat.get(11).expect("Underwater");
        assert_eq!(underwater.name, "Underwater");
        assert_eq!(underwater.room_hf, -10000);
        assert_eq!(underwater.reverb, 1700);
    }
}
