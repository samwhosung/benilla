//! Character customization render data (Milestone B) — what a player's appearance
//! selects: which **geosets** the body shows ([`CharacterGeosets`]) and which **skin textures** the
//! body wears ([`CharSections`]).
//!
//! **Geosets.** A character model contains *every* hairstyle, facial-hair piece, and body-option geoset;
//! the real client renders only the selected ones. The selection is the compositor's geoset dispatch
//! `0x477520`. For a character with **no equipment** the 8 per-item
//! branches all no-op (their `ItemDisplayInfo` records are null), leaving only the unconditional opening
//! block: disable every geoset, enable geoset 0, then enable the 16 region-base entries of `cc+0x144` —
//! entries 0–3 overwritten by the customization DBCs (the chosen hair + 3 facial-hair geosets), entries
//! 4–15 the default group bases. We render an M2 submesh iff its `skinSectionId` is in that set.
//!
//! **Skin textures.** A character body's `M2TextureType::Other(1)` (body skin) batches have no embedded
//! texture — the client supplies a runtime composite keyed on the appearance ([`CharSections::composite_body`],
//! decision 0044). The **base skin** (`sectionType 0`) is a single full 256² body-layout BLP per (race,
//! sex, skinColor); the face / facial-hair / hair / underwear overlays (`sectionTypes 1–4`) are region
//! BLPs blended on top at fixed atlas tiles (the static partition `0x475c50` writes + the
//! section→cell map of `0x4782e0`).
//!
//! DBC field maps from the client's record readers (CharSections `0x575540`, the geoset tables
//! `0x5753b0`/`0x575a80`); field counts + row semantics cross-checked against the build-5875 files.

mod customization;
mod geosets;
mod sections;

pub use customization::{CharCreateCatalog, DialRanges, StartOutfitItem};
pub use geosets::{CharacterGeosets, EquipGeosets};
pub use sections::{
    equip_blits, equip_column, equip_region_candidates, equip_tex_dir, equip_tile, forearm_dressed,
    BlitSource, CharSections, EmblemLayer, EquipBlit, GuildEmblem,
};

// The appearance tables' schemas, for the `benilla-extract dbc` CSV dump (`crate::schema_for`) —
// the same constructors the loaders above use, never a second transcription of the same layout.
pub(crate) use geosets::{char_facial_hair_schema, char_hair_geosets_schema, helmet_vis_schema};
pub(crate) use sections::char_sections_schema;
