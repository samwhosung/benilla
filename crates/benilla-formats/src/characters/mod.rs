//! Character appearance render data: the geosets an appearance shows ([`CharacterGeosets`], the
//! client's geoset dispatch `0x477520`) and the body skin it wears: body-skin batches
//! (`M2TextureType::Other(1)`) have no texture of their own, and [`CharSections::composite_body`]
//! builds one on the 256² partition `0x475c50` and the section→cell map `0x4782e0`.
//!
//! DBC layouts follow the client's record readers: CharSections `0x575540`, the geoset tables
//! `0x5753b0`/`0x575a80`.

mod customization;
mod geosets;
mod sections;

pub use customization::{CharCreateCatalog, DialRanges, StartOutfitItem};
pub use geosets::{CharacterGeosets, EquipGeosets};
pub use sections::{
    equip_blits, equip_column, equip_region_candidates, equip_tex_dir, equip_tile, forearm_dressed,
    scale_body_tile, BlitSource, CharSections, EmblemLayer, EquipBlit, GuildEmblem,
};

// The loaders' own schemas, for the `benilla-extract dbc` CSV dump (`crate::schema_for`).
pub(crate) use geosets::{char_facial_hair_schema, char_hair_geosets_schema, helmet_vis_schema};
pub(crate) use sections::char_sections_schema;
