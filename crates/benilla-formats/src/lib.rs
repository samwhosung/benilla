//! `benilla-formats` — adapters and CLI tooling over WoW 1.12.1 (build 5875) asset formats.
//!
//! Every asset format is **in-repo** (decision 0021): MPQ archives (`benilla-mpq`), BLP2 textures
//! (`benilla-blp`), DBC tables (`benilla-dbc`), WDT tile tables (`benilla-wdt`), ADT terrain
//! (`benilla-adt`), WMO buildings (`benilla-wmo`), and M2 models (`benilla-m2`). This crate adds our
//! schema layer, the patch-chain helper ([`Chain`]), and the `benilla-extract` CLI, and keeps data in
//! **raw WoW coordinates** (the Bevy transform lives in `benilla`).
//!
//! Phase 1: MPQ list/extract, BLP -> PNG, DBC -> CSV.

use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use benilla_dbc::{DbcParser, FieldType, Schema, SchemaField};

/// The BLP texel forms, re-exported so a consumer of [`BlpMipChain`] can switch on
/// [`BlpMipChain::texels`] without depending on `benilla-blp` directly.
pub use benilla_blp::BlpTexels;

mod chain;
pub use chain::{Chain, ChainEntry};
/// The UI texture table's other half — loose `.tga` art (addon folders; decision 1322).
mod tga;
pub use tga::tga_to_rgba;
/// Where the WoW install is — the one resolver (decision 1175). Paired with [`Chain`]: this says
/// *where*, that opens it.
mod install;
pub use install::{addon_corpus, addon_corpus_candidates, candidates, skipped, wow_data};
mod characters;
pub use characters::{
    equip_blits, equip_column, equip_region_candidates, equip_tex_dir, equip_tile, forearm_dressed,
    BlitSource, CharCreateCatalog, CharSections, CharacterGeosets, DialRanges, EmblemLayer,
    EquipBlit, EquipGeosets, GuildEmblem, StartOutfitItem,
};
mod camera_shakes;
mod cinematics;
mod creatures;
mod dbc;
mod unit_blood;
pub use camera_shakes::{load_camera_shakes, CameraShake, CameraShakeCatalog, SpellShakeGroup};
pub use cinematics::{
    camera_model_path, load_cinematics, CinematicCameraRow, CinematicCatalog, CinematicPath,
    CinematicSequence, CinematicView, SEQUENCE_CAMERAS,
};
pub use creatures::{
    load_creature_catalog, CreatureCatalog, CreatureModel, FootprintParams, NpcAppearance,
};
pub use macro_icons::load_macro_icons;
mod macro_icons;
pub use unit_blood::{load_blood_catalog, BloodCatalog};
mod itemsets;
pub use itemsets::{load_item_sets, ItemSetCatalog, ItemSetInfo};
mod gm_ticket_category;
pub use gm_ticket_category::{
    load_gm_ticket_categories, GmTicketCategory, GmTicketCategoryCatalog,
};
mod itembagfamily;
pub use itembagfamily::{load_item_bag_families, ItemBagFamilyCatalog};
mod itemclass;
pub use itemclass::{load_item_classes, ItemClassCatalog};
mod auction_house;
pub use auction_house::{load_auction_houses, AuctionHouseCatalog, AuctionHouseInfo};
mod itemsubclass;
pub use itemsubclass::{load_item_sub_classes, ItemSubClassCatalog, ItemSubClassInfo};
mod factions;
pub use factions::{
    faction_flags, load_faction_catalog, reputation_rank, FactionCatalog, FactionInfo,
    FactionTemplate, Reaction,
};
mod creature_types;
pub use creature_types::{load_creature_type_flags, CreatureTypeFlags};
/// The chat language scramble — the reference's `0x49b560` (B262). Reads [`LanguageWords`].
mod garble;
pub use garble::{garble, garble_chat, Garble, FLUENT_SKILL};
mod languages;
pub use languages::{
    load_default_languages, load_language_words, load_languages, DefaultLanguages, LanguagePool,
    LanguageWords, Languages,
};
mod creature_families;
pub use creature_families::{
    load_creature_families, load_pet_food_names, CreatureFamilies, CreatureFamily, PetFoodNames,
};
mod gameobjects;
pub use gameobjects::{
    go_sound_slot, load_gameobject_catalog, load_gameobject_sounds, GameObjectCatalog,
    GameObjectSounds,
};
mod durability;
pub use durability::{load_durability_tables, DurabilityTables};
mod exhaustion;
pub use exhaustion::{load_exhaustion, ExhaustionRow};
mod bank_bag_slot_prices;
pub use bank_bag_slot_prices::{load_bank_bag_slot_prices, BankBagSlotPrices};
mod stable_slot_prices;
pub use stable_slot_prices::{load_stable_slot_prices, StableSlotPrices};
mod page_text_material;
pub use page_text_material::{load_page_text_material_catalog, PageTextMaterialCatalog};
mod stationery;
pub use stationery::{
    load_stationery_catalog, StationeryCatalog, StationeryRow, STATIONERY_DEFAULT,
};
mod lock;
pub use lock::{
    load_lock_catalog, LockCatalog, LockSlot, GO_STATE_ACTIVE, GO_STATE_ACTIVE_ALTERNATIVE,
    GO_STATE_READY, LOCK_KEY_ITEM, LOCK_KEY_NONE, LOCK_KEY_SKILL, MAX_LOCK_SLOTS,
};
mod lock_type;
pub use lock_type::{load_lock_type_catalog, LockTypeCatalog};
mod chr_classes;
pub use chr_classes::{load_chr_classes, ChrClasses, PET_NAME_TOKEN_FALLBACK};
mod pet_stats;
pub use pet_stats::{
    load_pet_loyalty_names, load_pet_personalities, PetHappiness, PetLoyaltyNames,
    PetPersonalities, PetPersonality, FALLBACK_PERSONALITY,
};
mod items;
pub use items::{load_item_display_catalog, ItemDisplay, ItemDisplayCatalog};
mod item_sounds;
pub use item_sounds::{load_item_group_sounds, ItemGesture, ItemGroupSoundsCatalog};
mod item_visuals;
pub use item_visuals::{
    load_enchant_catalog, load_item_visual_catalog, EnchantCatalog, ItemVisualCatalog,
    ITEM_VISUAL_SLOTS,
};
mod item_random_properties;
pub use item_random_properties::{
    load_random_property_catalog, RandomProperty, RandomPropertyCatalog,
    RANDOM_PROPERTY_FIRST_SLOT, RANDOM_PROPERTY_SLOTS,
};
mod ground_effects;
pub use ground_effects::{
    load_ground_effect_catalog, scatter_ground_doodads, GroundDoodadPlacement, GroundEffect,
    GroundEffectCatalog, FRILL_DENSITY, FRILL_DENSITY_MAX,
};
mod light;
pub use light::{Atmosphere, LightCatalog, Submersion, ZERO_KEY_COLOR, ZERO_KEY_SCALAR};
mod loading_screen;
pub use loading_screen::{load_loading_screens, LoadingScreenCatalog};
mod liquid;
pub use liquid::{LiquidKind, LiquidMesh};
mod maps;
pub use maps::{load_map_catalog, MapBattlegroundColumns, MapCatalog};
mod anim_data;
pub use anim_data::{load_anim_data_catalog, AnimDataCatalog, AnimEntry};
mod quest_headers;
pub use quest_headers::{load_quest_header_names, QuestHeaderNames};
mod quest_info;
pub use quest_info::{load_quest_tag_names, QuestTagNames};
mod area_trigger;
pub use area_trigger::{load_area_trigger_catalog, AreaTriggerCatalog, AreaTriggerRow};
mod area_sound;
pub use area_sound::{
    load_area_sound_catalog, AreaAudio, AreaEntry, AreaSoundCatalog, SoundAmbienceEntry,
    ZoneIntroEntry, ZoneMusicEntry,
};
mod creature_sound;
pub use creature_sound::{load_creature_voice_catalog, CreatureVoice, CreatureVoiceCatalog};

mod npc_greeting;
pub use npc_greeting::{load_npc_greeting_catalog, NpcGreeting, NpcGreetingCatalog};
mod emotes;
pub use emotes::{load_emote_sound_catalog, EmoteSoundCatalog};
mod emote_text;
pub use emote_text::{load_emote_text_catalog, EmoteLine, EmoteTextCatalog};
mod environmental_damage;
pub use environmental_damage::{load_environmental_damage, EnvironmentalDamageTable};
mod footsteps;
pub use footsteps::{load_footprint_textures, load_footstep_catalog, FootstepCatalog};
mod death_thud;
pub use death_thud::{load_death_thud_catalog, DeathThudCatalog};
mod sound_entries;
pub use sound_entries::{load_sound_kit_catalog, sound_kit_flags, SoundKit, SoundKitCatalog};
mod sound_provider;
pub use sound_provider::{load_sound_provider_catalog, SoundProvider, SoundProviderCatalog};
mod sound_water;
pub use sound_water::{load_water_sound_catalog, WaterSoundCatalog};
mod vocal_ui_sounds;
pub use vocal_ui_sounds::{
    load_vocal_ui_sounds, VocalUiSound, VocalUiSoundCatalog, VOCAL_UI_LINES,
};
mod weapon_impact;
pub use weapon_impact::{
    impact_slot, load_weapon_impact_catalog, WeaponImpactCatalog, WeaponImpactRow,
};
mod sheathe;
pub use sheathe::{load_sheathe_sound_catalog, SheatheSoundCatalog, SheatheSounds};
mod weapon_swing;
pub use weapon_swing::{load_weapon_swing_catalog, WeaponSwingCatalog};
mod material;
pub use material::{load_material_catalog, MaterialCatalog};
mod wmo_area;
pub use wmo_area::{load_wmo_area_catalog, WmoArea, WmoAreaCatalog};
mod spells;
pub use spells::{
    cc_exemption, grants_immunity, load_shapeshift_forms, load_spell_cast_times,
    load_spell_catalog, load_spell_dispel_types, load_spell_durations, load_spell_radii,
    load_spell_ranges, min_max_range, substitute, CcExemption, FormRefusal, LearnAnnouncement,
    LearnEffect, OpenLock, ShapeshiftForm, SpellCastTime, SpellCastTimeCatalog, SpellCatalog,
    SpellDispelTypes, SpellDisplay, SpellDuration, SpellDurationCatalog, SpellRadius,
    SpellRadiusCatalog, SpellRange, SpellRangeCatalog, TokenContext, ATTR_CASTABLE_WHILE_DEAD,
    ATTR_NOT_IN_COMBAT, ATTR_ONLY_STEALTHED, COMBAT_REACH_ADD, MELEE_RANGE_FLOOR,
    ON_NEXT_SWING_RANGE, SPELL_ATTR_IS_TRADESKILL, SPELL_EFFECT_CREATE_ITEM,
    SPELL_EFFECT_ENCHANT_ITEM, SPELL_EFFECT_ENCHANT_ITEM_TEMPORARY, SPELL_EFFECT_LEARN_PET_SPELL,
    SPELL_EFFECT_LEARN_SPELL, SPELL_EFFECT_PROSPECTING, SPELL_EFFECT_SKILL_STEP,
    SPELL_EFFECT_SKINNING, SPELL_EFFECT_TRADE_SKILL,
};
mod skill_lines;
pub use skill_lines::{
    load_skill_line_catalog, SkillLineCatalog, SkillLineInfo, SkillRaceClass, SlaInfo,
};
mod spell_focus;
mod spell_mechanic;
pub use spell_focus::{load_spell_focus_catalog, SpellFocusCatalog};
pub use spell_mechanic::{load_spell_mechanic_catalog, SpellMechanicCatalog};
mod talents;
pub use talents::{load_talent_catalog, Talent, TalentCatalog, TalentTabInfo, MAX_TALENT_RANK};
mod spell_visual;
pub use spell_visual::{
    char_proc_small_int, char_proc_type, load_spell_visual_catalog, ChainEffect, ChainProc,
    CharProc, SpellVisualCatalog, TrailProc, VisualKit, VisualStages, CHAIN_MAX_BEAMS,
    KIT_CHAR_PROCS, KIT_SLOT_TAGS, MISSILE_ATTACH_TABLE, WORLD_EFFECT_TAG,
};
mod emit_timing;
pub use emit_timing::{EmitParams, EmitTiming, ParamsNow};
mod particles;
pub use particles::{
    parse_m2_particle_emitters, CellRamp, OverLife, OverLifeSample, ParticleBlend,
    ParticleEmitterDef, ParticleShape, SplineData,
};
mod ribbons;
pub use ribbons::{parse_m2_ribbon_emitters, RibbonEmitterDef, RibbonVisibility};
mod value_track;
pub use value_track::{TrackValue, ValueTrack};
mod models;
pub use models::{
    accumulate_wmo_group_camera_collision, accumulate_wmo_group_camera_only_collision,
    accumulate_wmo_group_collision, authored_half_height, batch_footprint, glue_art_extent,
    hand_grip_finger_poses, load_m2_animation_summary, load_m2_bone_spins, load_m2_bounds,
    load_m2_collision_hull, load_m2_mesh, load_m2_mesh_skinned, load_object_model, load_wmo,
    load_wmo_collision_tris, m2_bone_spins, m2_owner_reach, m2_ribbon_emitter_count,
    m2_sequence_visible_textures, m2_texture_transform_count, non_separable_billboard_bones,
    owner_last_rung, owner_last_rung_bucket, parse_m2_animation_lookup, parse_m2_animation_summary,
    parse_m2_animations, parse_m2_attachments, parse_m2_bounds, parse_m2_camera,
    parse_m2_cch_marker, parse_m2_collision_hull, parse_m2_event_markers,
    parse_m2_global_sequence_bones, parse_m2_lights, parse_m2_pane_cameras,
    parse_m2_playable_animation_lookup, parse_m2_portrait_camera, parse_m2_render_submeshes,
    parse_m2_skeleton, parse_m2_string_anchors, parse_wmo_fogs, parse_wmo_lights,
    parse_wmo_portals, parse_wmo_root, rotation_2x2, shipped_glue_art_extent, uv_transform,
    wmo_group_doodad_refs, wmo_group_fixed_colors, wmo_group_footprint_tris, wmo_group_header,
    wmo_group_light_refs, wmo_group_liquid_mesh, wmo_group_raw_colors, wmo_group_submeshes,
    wmo_root_id, AlphaAnim, AlphaMap, AlphaSeq, AnimEvent, ArtExtent, BatchFootprint, Billboard,
    BillboardKind, BoneKeys, BoneScaleAnim, BoneSpin, CharSkinSlot, CollisionMesh, Coverage,
    CoverageReader, EmitterBoneLink, EventMarker, FogPolicy, FootprintTris, GlobalSeqBone,
    GlobalSeqChannel, GroundQuad, KeyAnim, M2AnimSummary, M2Attachment, M2Bounds, M2CameraTracks,
    M2Light, M2PaneCamera, M2PortraitCamera, ModelAnimation, ModelBlend, ParentArm, ParentBasis,
    PlayableAnim, RenderSubmesh, RgbAnim, ScalarAnim, SeqLoops, ShippedGlueScene, Skeleton,
    SkeletonBone, StringAnchors, UvAnim, UvRotAnim, WmoBatchClass, WmoDoodad, WmoDoodadSet, WmoFog,
    WmoGroupHeader, WmoGroupInfo, WmoLight, WmoPortalInfo, WmoPortalRef, WmoPortals, WmoRoot,
    ALPHA_KEY_REF, DEGENERATE_RING_FOOTPRINT, GLUE_AUTHORED_ASPECT, NO_GROUP_LIQUID,
    OWNER_RUNG_BUCKETS, SHIPPED_GLUE_SCENES,
};
mod terrain;
pub use terrain::{
    adt_to_tile_mesh, area_id_at, find_tile_near, ground_effect_at, impassable_at, load_tile_mesh,
    load_tiles_around, mcsh_shadowed_at, terrain_height_at, triangle_z_at, ChunkMesh, Doodad,
    MapTiles, TileMesh, WmoInstance, ALPHA_MAP_SIZE, CHUNK_SIZE, SHADOW_MAP_SIZE, STORMWIND_XY,
    TERRAIN_LAYER_TILES, TILE_SIZE,
};
mod wdl;
/// World (x, y) ↔ ADT tile `(col, row)` — the same mapping the streamer uses to pick tiles
/// (and the minimap uses to place its tile art, decision 0203: minimap `map<X>_<Y>.blp` names
/// share the ADT index order, chain-verified — `AhnQiraj_27_46.adt` exists, the swap doesn't).
pub use benilla_wdt::{
    tile_to_world, world_to_chunk, world_to_tile, GlobalWmo, WdtFile, WdtReader, WowVersion,
};
pub use wdl::{WdlFile, WdlTileMesh};

// The map arc (decision 0203), phase 0: the minimap tile hash catalog + the world-map DBCs.
mod minimap_translate;
pub use minimap_translate::{load_minimap_translate, MinimapTranslate};
mod world_map_area;
pub use world_map_area::{load_world_map_area_catalog, WorldMapArea, WorldMapAreaCatalog};
mod world_map_overlay;
pub use world_map_overlay::{
    load_world_map_overlay_catalog, WorldMapOverlay, WorldMapOverlayCatalog,
};
mod world_map_continent;
pub use world_map_continent::{
    load_world_map_continent_catalog, WorldMapContinent, WorldMapContinentCatalog,
};
mod area_poi;
pub use area_poi::{load_area_poi_catalog, AreaPoi, AreaPoiCatalog};
mod world_state_ui;
pub use world_state_ui::{load_world_state_ui_catalog, WorldStateUiCatalog, WorldStateUiRow};
mod area_table;
pub use area_table::{load_area_table_catalog, AreaTableCatalog, AreaTableRow};
mod chat_channels;
pub use chat_channels::{
    flags as chat_channel_flags, load_chat_channels_catalog, ChatChannelRow, ChatChannelsCatalog,
};
mod server_messages;
pub use server_messages::{load_server_messages_catalog, ServerMessagesCatalog};
mod game_tips;
pub use game_tips::{load_game_tips, GameTipsCatalog};
mod text_filter_lists;
pub use text_filter_lists::{load_chat_profanity, load_spam_messages, FilterPattern};
mod race_sound;
pub use race_sound::{load_exploration_sound_catalog, ExplorationSoundCatalog};

mod race_pvp_team;
pub use race_pvp_team::load_race_pvp_teams;
mod zone_map;
pub use zone_map::{load_zone_map, ZONE_MAP_EDGE};

// The transport arc (decision 0438), phase 0: the taxi/transport path DBC adapter + timetable.
mod taxi;
pub use taxi::{load_taxi_path_nodes, TaxiPathNode, TaxiPathNodes};
mod transport_period;
mod transports;
pub use transports::{TransportSample, TransportTimetable};
// The transport arc phase 3's second consumer: type-11 elevator/lift keyframe paths.
mod elevators;
pub use elevators::{
    elevator_period_ms, elevator_sample, load_elevator_paths, ElevatorKeyframe, ElevatorPaths,
};

// The taxi arc (decision 0484), phase 1: the flight-master catalogs — node identity/position/
// per-team mount (`TaxiNodes.dbc`) and the direct-hop fare table (`TaxiPath.dbc`). Distinct from
// `taxi`/`TaxiPathNode.dbc` above, which carries a path's waypoints, not its node list or price.
mod taxi_nodes;
pub use taxi_nodes::{load_taxi_nodes, TaxiNode, TaxiNodes};
mod taxi_path;
pub use taxi_path::{load_taxi_paths, TaxiPath, TaxiPaths};

/// The ten vanilla base content archives, **lowest priority first** — the reference mounter's
/// table (`0x82e12c`) at its fixed priorities (`dbc.MPQ` = 0x36 … `model.MPQ` = 0x3f),
/// reversed so [`Chain`]'s later-wins order reproduces them (decision 1300).
///
/// This is only the *base* set: `patch.MPQ`, the `patch-?.MPQ` archives, and the optional
/// `speech2.MPQ` are **discovered**, not listed — the whole mount law lives in [`Chain::open`].
/// `base.MPQ` is deliberately absent: the reference opens it once for `telemetry.dat` and closes
/// it before the mounter runs, so its contents are unreachable as assets (`0x5aa2d0`).
///
/// The base archives hold the bulk of the data but carry **no `(listfile)`**; the master
/// `(listfile)` (and most overrides) live in the patch archives, which is why the chain — not a
/// single archive — is the correct unit to read from. Verified against the build-5875 client
/// (`patch.MPQ` listfile = 26,350 entries).
pub const VANILLA_BASE_ORDER: &[&str] = &[
    "dbc.MPQ",
    "speech.MPQ",
    "fonts.MPQ",
    "interface.MPQ",
    "misc.MPQ",
    "sound.MPQ",
    "wmo.MPQ",
    "terrain.MPQ",
    "texture.MPQ",
    "model.MPQ",
];

/// Open a [`Chain`] from either a single `.MPQ` file or a vanilla `Data` directory. Thin wrapper over
/// [`Chain::open`], kept as the established entry point used across the crate and the CLI.
pub fn open_chain(path: &Path) -> Result<Chain> {
    Chain::open(path)
}

/// Decode a BLP texture from raw archive bytes and write it as a PNG at `out`.
///
/// Returns the source `(width, height)`. Used by the `benilla-extract` CLI. **1.12 textures are BLP2**
/// (DXT-compressed or paletted Raw1 — NOT BLP1; verified). Decoded in-repo by [`benilla_blp`]
/// (decision 0021); mip level 0 (full resolution) is written.
pub fn blp_to_png(blp_bytes: &[u8], out: &Path) -> Result<(u32, u32)> {
    let (w, h, rgba) = blp_to_rgba(blp_bytes)?;
    image::RgbaImage::from_raw(w, h, rgba)
        .ok_or_else(|| anyhow::anyhow!("BLP RGBA buffer size mismatch"))?
        .save(out)
        .with_context(|| format!("writing PNG {}", out.display()))?;
    Ok((w, h))
}

/// One authored mip level's texel census — what a sampler minifying onto this level actually
/// reads. Split by the alpha byte because the renderer's multiply lanes (Mod2x, `2·src·dst`) read
/// **no alpha**: a texel the author left transparent still modulates the scene by its colour, so
/// the "outside" colour of a cut-out is a look-bearing fact, not padding.
#[derive(Debug, Clone, PartialEq)]
pub struct BlpMipStats {
    pub level: u32,
    pub width: u32,
    pub height: u32,
    /// Texels with alpha 0 (the authored "outside") and with alpha > 0 (the "inside").
    pub outside: usize,
    pub inside: usize,
    /// `(min, mean, max)` of the RGB **luma** (`(r+g+b)/3`) over the outside / inside texels;
    /// `None` when that class is empty.
    pub outside_luma: Option<(u8, f32, u8)>,
    pub inside_luma: Option<(u8, f32, u8)>,
    /// Texels whose luma is below 128 — under Mod2x each of these DARKENS the scene.
    pub below_128: usize,
}

/// Census every authored mip level of an in-memory BLP (see [`BlpMipStats`]), decoding through the
/// same [`benilla_blp::decode`] every CPU consumer uses. The "what does the far sampler see"
/// instrument: a texture whose mip 0 reads neutral can still carry dark tail levels, and the tail
/// is all a minified streak or sprite ever samples.
pub fn blp_mip_stats(blp_bytes: &[u8]) -> Result<Vec<BlpMipStats>> {
    let decoded =
        benilla_blp::decode(blp_bytes).map_err(|e| anyhow::anyhow!("decoding BLP: {e}"))?;
    Ok(decoded
        .mips
        .iter()
        .enumerate()
        .map(|(level, m)| {
            let mut outside = (0usize, u8::MAX, 0u64, u8::MIN);
            let mut inside = (0usize, u8::MAX, 0u64, u8::MIN);
            let mut below_128 = 0usize;
            for px in m.rgba.as_chunks::<4>().0 {
                let luma = ((px[0] as u32 + px[1] as u32 + px[2] as u32) / 3) as u8;
                if luma < 128 {
                    below_128 += 1;
                }
                let acc = if px[3] == 0 {
                    &mut outside
                } else {
                    &mut inside
                };
                acc.0 += 1;
                acc.1 = acc.1.min(luma);
                acc.2 += luma as u64;
                acc.3 = acc.3.max(luma);
            }
            let fold = |(n, lo, sum, hi): (usize, u8, u64, u8)| {
                (n > 0).then(|| (lo, sum as f32 / n as f32, hi))
            };
            BlpMipStats {
                level: level as u32,
                width: m.width,
                height: m.height,
                outside: outside.0,
                inside: inside.0,
                outside_luma: fold(outside),
                inside_luma: fold(inside),
                below_128,
            }
        })
        .collect())
}

/// Write **every** authored mip level of a BLP as `<stem>.mip<N>.png` beside `out` (whose own
/// path receives level 0, exactly as [`blp_to_png`] does), and return the per-level census.
pub fn blp_mips_to_png(blp_bytes: &[u8], out: &Path) -> Result<Vec<BlpMipStats>> {
    let decoded =
        benilla_blp::decode(blp_bytes).map_err(|e| anyhow::anyhow!("decoding BLP: {e}"))?;
    let stem = out.with_extension("");
    for (level, m) in decoded.mips.iter().enumerate() {
        let path = if level == 0 {
            out.to_path_buf()
        } else {
            let mut p = stem.as_os_str().to_owned();
            p.push(format!(".mip{level}.png"));
            std::path::PathBuf::from(p)
        };
        image::RgbaImage::from_raw(m.width, m.height, m.rgba.clone())
            .ok_or_else(|| anyhow::anyhow!("BLP RGBA buffer size mismatch at level {level}"))?
            .save(&path)
            .with_context(|| format!("writing PNG {}", path.display()))?;
    }
    blp_mip_stats(blp_bytes)
}

/// Decode a BLP texture (raw bytes) to RGBA8: `(width, height, pixels)`.
///
/// For feeding GPU textures (the renderer) without a disk round-trip; mip level 0.
pub fn blp_to_rgba(blp_bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let decoded =
        benilla_blp::decode(blp_bytes).map_err(|e| anyhow::anyhow!("decoding BLP: {e}"))?;
    let level0 = decoded
        .mips
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("BLP has no level 0"))?;
    Ok((level0.width, level0.height, level0.rgba))
}

/// Read a texture from the chain by internal path (accepts `/` or `\`) and decode to RGBA8.
pub fn read_texture_rgba(chain: &mut Chain, path: &str) -> Result<(u32, u32, Vec<u8>)> {
    let name = path.replace('/', "\\");
    let bytes = chain
        .read_file(&name)
        .with_context(|| format!("reading texture '{name}'"))?;
    blp_to_rgba(&bytes)
}

/// A BLP's authored mip pyramid, mip-major, level 0 first.
/// **Never empty**: the chain length is `BlpHeader::mipmaps_count().max(1)` — a no-mipmaps BLP (the
/// count formula reports 0) still yields a 1-level chain (level 0), matching `benilla_blp::decode`'s
/// own guarantee that `mips` is never empty (see [`blp_bytes_to_mip_chain`]).
///
/// [`texels`](Self::texels) says what the buffers hold. [`blp_bytes_to_mip_chain`] always decodes to
/// `Rgba8Unorm`; [`blp_bytes_to_native_chain`] keeps DXTC blocks verbatim for a GPU that can eat
/// them. A consumer that reads texels on the CPU must use the former (or check `texels`).
#[derive(Debug, Clone)]
pub struct BlpMipChain {
    /// Mip-0 dimensions; level i's dims are `(width >> i).max(1) × (height >> i).max(1)`.
    pub width: u32,
    pub height: u32,
    /// What every entry of `mips` holds — decoded pixels, or S3TC blocks.
    pub texels: BlpTexels,
    /// One buffer per authored mip level, level 0 first. **Authored, not regenerated** —
    /// the BLP's pre-baked mip levels are what the real 1.12 client uploads ("the BLP's own stored mip
    /// chain is used verbatim — no client-side mip regeneration"). Using these instead of
    /// CPU-resampling mip 0 in gamma byte space is the
    /// C2 fix: gamma-byte averaging of texels darkens mid-tones (the historical "dim/cool"
    /// terrain tone gap).
    pub mips: Vec<Vec<u8>>,
}

impl BlpMipChain {
    /// Width × height of the mip level `i`, clamped to 1×1 minimum (wgpu / GL convention).
    pub fn mip_size(&self, i: u32) -> (u32, u32) {
        ((self.width >> i).max(1), (self.height >> i).max(1))
    }

    /// Are these levels CPU-readable `Rgba8Unorm` pixels? The guard for any consumer that indexes
    /// into a level (resample, alpha stomp, colour read) — block bytes are not pixels.
    pub fn is_rgba8(&self) -> bool {
        !self.texels.is_block_compressed()
    }

    /// This chain with every level decoded to `Rgba8Unorm`; a no-op if it already is.
    ///
    /// The escape hatch for a consumer that asked for the passthrough form and then could not use
    /// it — it costs the decode it was trying to avoid, but it needs no second read of the file and
    /// it cannot fail, so the caller is never left holding blocks it must not upload.
    pub fn into_rgba8(self) -> Self {
        if self.is_rgba8() {
            return self;
        }
        let mips = self
            .mips
            .iter()
            .enumerate()
            .map(|(i, level)| {
                let (w, h) = self.mip_size(i as u32);
                benilla_blp::decode_level(self.texels, w, h, level)
            })
            .collect();
        Self {
            texels: BlpTexels::Rgba8Unorm,
            mips,
            ..self
        }
    }
}

/// Decode every authored mip level of a BLP texture from the chain into `Rgba8Unorm` buffers.
///
/// One RGBA8 allocation per mip level (8 for a 256² with 8 mips). Caller is responsible for any
/// further processing (concatenation, upload, BCn re-encode). The mip pyramid is the BLP's own,
/// not regenerated — the C2-faithful behaviour.
pub fn read_texture_mip_chain(chain: &mut Chain, path: &str) -> Result<BlpMipChain> {
    let name = path.replace('/', "\\");
    let bytes = chain
        .read_file(&name)
        .with_context(|| format!("reading texture '{name}'"))?;
    blp_bytes_to_mip_chain(&bytes).with_context(|| format!("decoding texture '{name}'"))
}

/// Read a texture from the chain by internal path and keep its DXTC blocks verbatim —
/// [`blp_bytes_to_native_chain`]'s chain-reading wrapper, the twin of [`read_texture_mip_chain`].
pub fn read_texture_native_chain(chain: &mut Chain, path: &str) -> Result<BlpMipChain> {
    let name = path.replace('/', "\\");
    let bytes = chain
        .read_file(&name)
        .with_context(|| format!("reading texture '{name}'"))?;
    blp_bytes_to_native_chain(&bytes).with_context(|| format!("decoding texture '{name}'"))
}

/// Decode every authored mip level of an **in-memory** BLP into `Rgba8Unorm` buffers — the BLP's own
/// stored pyramid, used verbatim (no client-side regeneration; the C2-faithful behaviour). This is the
/// bytes-in entry point used by the Bevy `AssetLoader`; [`read_texture_mip_chain`] is the
/// chain-reading wrapper.
pub fn blp_bytes_to_mip_chain(bytes: &[u8]) -> Result<BlpMipChain> {
    let decoded = benilla_blp::decode(bytes).map_err(|e| anyhow::anyhow!("decoding BLP: {e}"))?;
    // `decode` guarantees `mips` is never empty: it always decodes level 0 regardless of the
    // authored mip-chain count (`benilla_blp::decode`, `n = chain.clamp(1, 16)`, so the loop that
    // fills `mips` runs at least once) — a no-mipmaps BLP reports `mip_chain_count() == 0` but still
    // carries `mips[0]`. Take at least that one level: the chain we return is never empty, so a
    // mipmap-less BLP yields a 1-level chain here, not zero.
    let (width, height, count) = (
        decoded.width,
        decoded.height,
        decoded.mip_chain_count().max(1),
    );
    let mips: Vec<Vec<u8>> = decoded
        .mips
        .into_iter()
        .take(count)
        .map(|m| m.rgba)
        .collect();
    Ok(BlpMipChain {
        width,
        height,
        texels: BlpTexels::Rgba8Unorm,
        mips,
    })
}

/// Every authored mip level of an in-memory BLP **with its DXTC blocks kept verbatim** — the form
/// the reference client uploads (`glCompressedTexImage2DARB`, the OpenGL arm `0x59f5b0`).
///
/// Raw1/Raw3 BLPs have no block form and come back decoded, reporting
/// [`BlpTexels::Rgba8Unorm`] — so a caller switches on [`BlpMipChain::texels`] rather than
/// assuming. Levels are padded to whole blocks, so the concatenated chain is exactly what wgpu
/// expects for the reported format.
pub fn blp_bytes_to_native_chain(bytes: &[u8]) -> Result<BlpMipChain> {
    let decoded =
        benilla_blp::decode_native(bytes).map_err(|e| anyhow::anyhow!("decoding BLP: {e}"))?;
    // Same floor-at-one reasoning as `blp_bytes_to_mip_chain`.
    let (width, height, count) = (
        decoded.width,
        decoded.height,
        decoded.mip_chain_count().max(1),
    );
    let texels = decoded.texels;
    let mips: Vec<Vec<u8>> = decoded
        .mips
        .into_iter()
        .take(count)
        .map(|m| m.bytes)
        .collect();
    Ok(BlpMipChain {
        width,
        height,
        texels,
        mips,
    })
}

/// A hand-defined schema for a known 1.12 DBC, keyed by base filename, or `None`.
///
/// DBC files carry no column types, so the schema is ours to supply (from wowdev.wiki +
/// the verified file header). Vanilla localized strings occupy **9 dwords** (8 locale
/// offsets + 1 flags); only the enUS slot is populated in an enUS client. As we need more
/// DBCs (Map, AreaTable, ...) add them here.
fn schema_for(dbc_name: &str) -> Option<Schema> {
    let base = dbc_name.rsplit(['/', '\\']).next().unwrap_or(dbc_name);

    // Tables whose schema a typed loader in this crate already declares: reuse *that* constructor
    // rather than re-transcribing the layout here, so a dump can never disagree with what the client
    // actually reads (the six hand-written schemas below predate any loader for their table).
    // The appearance family is registered because "which row does this NPC render from?" is a
    // recurring question; the crate has ~30 more loader schemas that could join this list one line
    // at a time as they are needed.
    for (name, ctor) in [
        (
            "CreatureDisplayInfo.dbc",
            creatures::creature_display_info_schema as fn() -> Schema,
        ),
        (
            "CreatureDisplayInfoExtra.dbc",
            creatures::creature_display_info_extra_schema,
        ),
        (
            "CreatureModelData.dbc",
            creatures::creature_model_data_schema,
        ),
        ("CharHairGeosets.dbc", characters::char_hair_geosets_schema),
        (
            "CharacterFacialHairStyles.dbc",
            characters::char_facial_hair_schema,
        ),
        ("HelmetGeosetVisData.dbc", characters::helmet_vis_schema),
        ("CharSections.dbc", characters::char_sections_schema),
        ("ItemDisplayInfo.dbc", items::item_display_info_schema),
        ("ItemVisuals.dbc", item_visuals::item_visuals_schema),
        (
            "ItemVisualEffects.dbc",
            item_visuals::item_visual_effects_schema,
        ),
        (
            "SpellItemEnchantment.dbc",
            item_visuals::spell_item_enchantment_schema,
        ),
        (
            "ItemRandomProperties.dbc",
            item_random_properties::item_random_properties_schema,
        ),
        ("AreaTrigger.dbc", area_trigger::area_trigger_schema),
    ] {
        if base.eq_ignore_ascii_case(name) {
            return Some(ctor());
        }
    }

    if base.eq_ignore_ascii_case("TaxiNodes.dbc") {
        // 16 fields (record_size 64), verified against build 5875.
        let mut s = Schema::new("TaxiNodes");
        s.add_field(SchemaField::new("ID", FieldType::UInt32));
        s.add_field(SchemaField::new("MapID", FieldType::UInt32));
        s.add_field(SchemaField::new("X", FieldType::Float32));
        s.add_field(SchemaField::new("Y", FieldType::Float32));
        s.add_field(SchemaField::new("Z", FieldType::Float32));
        s.add_field(SchemaField::new("Name", FieldType::String)); // enUS (locale 0)
        s.add_field(SchemaField::new_array(
            "NameOtherLocales",
            FieldType::String,
            7,
        ));
        s.add_field(SchemaField::new("NameFlags", FieldType::UInt32));
        s.add_field(SchemaField::new("MountIdHorde", FieldType::UInt32));
        s.add_field(SchemaField::new("MountIdAlliance", FieldType::UInt32));
        s.set_key_field("ID");
        return Some(s);
    }

    if base.eq_ignore_ascii_case("AreaTable.dbc") {
        // 25 fields (record_size 100), verified against build 5875 (the layout
        // `area_table.rs` / `area_sound.rs` load from).
        let mut s = Schema::new("AreaTable");
        s.add_field(SchemaField::new("ID", FieldType::UInt32));
        s.add_field(SchemaField::new("ContinentID", FieldType::UInt32));
        s.add_field(SchemaField::new("ParentAreaID", FieldType::UInt32));
        s.add_field(SchemaField::new("AreaBit", FieldType::UInt32));
        s.add_field(SchemaField::new("Flags", FieldType::UInt32));
        s.add_field(SchemaField::new("SoundProviderPref", FieldType::UInt32));
        s.add_field(SchemaField::new(
            "SoundProviderPrefUnderwater",
            FieldType::UInt32,
        ));
        s.add_field(SchemaField::new("AmbienceID", FieldType::UInt32));
        s.add_field(SchemaField::new("ZoneMusic", FieldType::UInt32));
        s.add_field(SchemaField::new("IntroSound", FieldType::UInt32));
        s.add_field(SchemaField::new("ExplorationLevel", FieldType::UInt32));
        s.add_field(SchemaField::new("AreaName", FieldType::String)); // enUS (locale 0)
        s.add_field(SchemaField::new_array(
            "AreaNameOtherLocales",
            FieldType::String,
            7,
        ));
        s.add_field(SchemaField::new("AreaNameFlags", FieldType::UInt32));
        s.add_field(SchemaField::new("FactionGroupMask", FieldType::UInt32));
        s.add_field(SchemaField::new_array("LiquidTypeID", FieldType::UInt32, 4));
        s.set_key_field("ID");
        return Some(s);
    }

    if base.eq_ignore_ascii_case("GameObjectDisplayInfo.dbc") {
        // 12 fields (ID, ModelName, Sound[10]) — the layout `gameobjects.rs`'s own typed adapter
        // loads from (verified build 5875).
        let mut s = Schema::new("GameObjectDisplayInfo");
        s.add_field(SchemaField::new("ID", FieldType::UInt32));
        s.add_field(SchemaField::new("ModelName", FieldType::String));
        for i in 0..10 {
            s.add_field(SchemaField::new(format!("Sound{i}"), FieldType::UInt32));
        }
        s.set_key_field("ID");
        return Some(s);
    }

    if base.eq_ignore_ascii_case("TaxiPath.dbc") {
        // 4 fields: ID, FromTaxiNode, ToTaxiNode, Cost — verified build 5875.
        let mut s = Schema::new("TaxiPath");
        for name in ["ID", "FromTaxiNode", "ToTaxiNode", "Cost"] {
            s.add_field(SchemaField::new(name, FieldType::UInt32));
        }
        s.set_key_field("ID");
        return Some(s);
    }

    if base.eq_ignore_ascii_case("TaxiPathNode.dbc") {
        // 9 fields (ID, PathID, NodeIndex, MapID, LocX/Y/Z, Flags, Delay) — the layout
        // `taxi.rs`'s own typed adapter loads from (decision 0438 phase 0, verified build 5875).
        let mut s = Schema::new("TaxiPathNode");
        for name in ["ID", "PathID", "NodeIndex", "MapID"] {
            s.add_field(SchemaField::new(name, FieldType::UInt32));
        }
        for name in ["LocX", "LocY", "LocZ"] {
            s.add_field(SchemaField::new(name, FieldType::Float32));
        }
        for name in ["Flags", "Delay"] {
            s.add_field(SchemaField::new(name, FieldType::UInt32));
        }
        s.set_key_field("ID");
        return Some(s);
    }

    if base.eq_ignore_ascii_case("TransportAnimation.dbc") {
        // 7 fields: ID, TransportID, TimeIndex, PosX, PosY, PosZ, SequenceID — matches vmangos's
        // `TransportAnimationEntry` (`DBCStructure.h:709-718`), which only reads the middle five
        // (TransportEntry/TimeSeg/X/Y/Z) but the row itself carries all 7 (its own `Id` and
        // `MovementId` columns are commented out there, not absent from the file).
        let mut s = Schema::new("TransportAnimation");
        s.add_field(SchemaField::new("ID", FieldType::UInt32));
        s.add_field(SchemaField::new("TransportID", FieldType::UInt32));
        s.add_field(SchemaField::new("TimeIndex", FieldType::UInt32));
        for name in ["PosX", "PosY", "PosZ"] {
            s.add_field(SchemaField::new(name, FieldType::Float32));
        }
        s.add_field(SchemaField::new("SequenceID", FieldType::UInt32));
        s.set_key_field("ID");
        return Some(s);
    }

    None
}

/// Parse a DBC (raw archive bytes) using our schema for it and write it as CSV with
/// column headers. Returns `(record_count, field_count)`. Errors if we have no schema for
/// the named DBC (with a hint), so a wrong/missing schema fails loudly rather than silently
/// misaligning columns.
pub fn dbc_to_csv(dbc_bytes: &[u8], dbc_name: &str, out: &Path) -> Result<(u32, u32)> {
    let mut cursor = Cursor::new(dbc_bytes);
    let parser =
        DbcParser::parse(&mut cursor).map_err(|e| anyhow::anyhow!("parsing DBC header: {e}"))?;
    let (records, fields) = (parser.header().record_count, parser.header().field_count);

    let schema = schema_for(dbc_name).ok_or_else(|| {
        anyhow::anyhow!(
            "no schema defined for '{dbc_name}' ({fields} fields); known: TaxiNodes.dbc, \
             AreaTable.dbc, GameObjectDisplayInfo.dbc, TaxiPath.dbc, TaxiPathNode.dbc, \
             TransportAnimation.dbc, CreatureDisplayInfo.dbc, CreatureDisplayInfoExtra.dbc, \
             CreatureModelData.dbc, CharHairGeosets.dbc, CharacterFacialHairStyles.dbc, \
             HelmetGeosetVisData.dbc, CharSections.dbc, ItemDisplayInfo.dbc"
        )
    })?;

    let parser = parser
        .with_schema(schema)
        .map_err(|e| anyhow::anyhow!("applying schema (field count mismatch?): {e}"))?;
    let record_set = parser
        .parse_records()
        .map_err(|e| anyhow::anyhow!("parsing DBC records: {e}"))?;

    let file = std::fs::File::create(out).with_context(|| format!("creating {}", out.display()))?;
    benilla_dbc::export_to_csv(&record_set, std::io::BufWriter::new(file))
        .with_context(|| format!("writing CSV {}", out.display()))?;

    Ok((records, fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal, complete BLP2 header (magic..=mip_sizes[16], 148 bytes) — mirrors the private
    /// builder in `benilla_blp`'s own tests (that crate's helper isn't exported).
    fn blp2_header(
        compression: u8,
        alpha_bits: u8,
        alpha_type: u8,
        has_mipmaps: u8,
        width: u32,
        height: u32,
        offsets: [u32; 16],
        sizes: [u32; 16],
    ) -> Vec<u8> {
        const HEADER_SIZE: usize = 148;
        let mut b = vec![0u8; HEADER_SIZE];
        b[0..4].copy_from_slice(b"BLP2");
        b[4..8].copy_from_slice(&1u32.to_le_bytes()); // content: 1 = Direct
        b[8] = compression;
        b[9] = alpha_bits;
        b[10] = alpha_type;
        b[11] = has_mipmaps;
        b[12..16].copy_from_slice(&width.to_le_bytes());
        b[16..20].copy_from_slice(&height.to_le_bytes());
        for (i, o) in offsets.iter().enumerate() {
            b[20 + i * 4..24 + i * 4].copy_from_slice(&o.to_le_bytes());
        }
        for (i, s) in sizes.iter().enumerate() {
            b[84 + i * 4..88 + i * 4].copy_from_slice(&s.to_le_bytes());
        }
        b
    }

    /// THE VERIFIED LATENT PANIC, regression-pinned: a no-mipmaps BLP2 (`has_mipmaps = 0`) makes
    /// `benilla_blp::decode` report `mip_chain_count() == 0`, which `blp_bytes_to_mip_chain` used to
    /// take literally (`.take(0)`), returning an **empty** `mips` — `benilla-assets`' `world_art_image`
    /// then panics on `chain.mips[0]`. `decode` itself always keeps level 0 regardless of the count
    /// (see the comment on `blp_bytes_to_mip_chain`), so the fix is to floor the take at 1: this must
    /// yield a 1-level chain, never an empty one.
    #[test]
    fn no_mipmap_blp_yields_a_one_level_chain_not_empty() {
        const HEADER_SIZE: usize = 148;
        const PALETTE_SIZE: usize = 256 * 4;
        let pixel_offset = (HEADER_SIZE + PALETTE_SIZE) as u32;
        let pixel_size = 2 * 2 * 4;
        let mut offsets = [0u32; 16];
        let mut sizes = [0u32; 16];
        offsets[0] = pixel_offset;
        sizes[0] = pixel_size;
        // compression 3 = Raw3 (uncompressed BGRA8); has_mipmaps 0.
        let mut b = blp2_header(3, 8, 0, 0, 2, 2, offsets, sizes);
        b.resize(HEADER_SIZE + PALETTE_SIZE, 0); // palette bytes: unused by Raw3, present anyway
        b.extend_from_slice(&[
            10, 20, 30, 40, // B G R A
            50, 60, 70, 80, //
            90, 100, 110, 120, //
            130, 140, 150, 160,
        ]);

        let chain = blp_bytes_to_mip_chain(&b).expect("valid no-mipmap BLP decodes");
        assert_eq!(chain.width, 2);
        assert_eq!(chain.height, 2);
        assert_eq!(
            chain.mips.len(),
            1,
            "no-mipmaps BLP must yield 1 level, not 0"
        );
        assert_eq!(chain.mips[0].len(), 2 * 2 * 4);
    }

    /// **B358 / B225, "black rain", pinned on the shipped asset.** `RainDrop01.blp` (16×128
    /// DXT3) under-stores its sub-block levels — 16 bytes for the 2×16 level where the block grid
    /// needs 64, 16 for the 1×8 where it needs 32 — and a decoder that zero-fills the difference
    /// makes those levels three-quarters and half BLACK (an all-zero BC2 block is colour 0 at
    /// alpha 0). The rain streak draws that texture under Mod2x, which reads no alpha, so a far
    /// streak minifying onto levels 3–4 multiplied the scene toward black. The reference completes
    /// a short level from the bytes that follow it in the file (`0x5a5780`), which for this asset
    /// are the next levels' own grey blocks. This is the census the fix was judged by: every level
    /// the sampler can reach stays inside the texture's authored neutral band, and level 3's second
    /// block IS level 4's first — the reference's copy, byte for byte. Skips without the client data.
    #[test]
    fn the_rain_streak_texture_is_neutral_on_every_level_the_sampler_reaches() {
        let data = crate::wow_data_or_skip!();
        let mut chain = open_chain(&data).expect("open chain");
        let path = "textures\\Weather\\RainDrop01.blp";
        let bytes = chain
            .read_file(path)
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        let native = blp_bytes_to_native_chain(&bytes).expect("decodes natively");
        assert_eq!((native.width, native.height), (16, 128));
        assert_eq!(native.texels, benilla_blp::BlpTexels::Bc2);
        assert!(
            native.mips.len() >= 5,
            "the tail levels are what this is about"
        );
        // Level 3 (2×16) is four blocks wide-grid; the file stores one. Blocks 2–4 are the
        // following levels' — block 2 is level 4's own first block.
        assert_eq!(native.mips[3].len(), 64);
        assert_eq!(
            &native.mips[3][16..32],
            &native.mips[4][..16],
            "level 3's second block must be the file's next 16 bytes — level 4's block"
        );
        assert!(
            native.mips[3][16..].iter().any(|&x| x != 0),
            "level 3's completion is never zero-filled"
        );
        // And the look-bearing fact, as the Mod2x lane reads it: no texel on any level darkens
        // the scene by more than the authored grey does (luma ≥ 120; the authored band is 125–164).
        let stats = blp_mip_stats(&bytes).expect("census");
        for s in &stats {
            for (class, luma) in [("outside", s.outside_luma), ("inside", s.inside_luma)] {
                if let Some((lo, _, _)) = luma {
                    assert!(
                        lo >= 120,
                        "level {} ({}x{}) {class} texels reach luma {lo} — a Mod2x streak sampling \
                         this level darkens the scene (B358)",
                        s.level,
                        s.width,
                        s.height
                    );
                }
            }
        }
    }

    /// Every table the CSV dumper claims in its error hint really has a schema, and each dumps
    /// against the **shipped** file — which is the only check that matters, because `with_schema`
    /// rejects a field-count mismatch and `dbc_to_csv` would otherwise fail only when a session
    /// reached for it mid-investigation. Skips without the client data.
    #[test]
    fn registered_dbc_schemas_dump_the_shipped_tables() {
        let data = crate::wow_data_or_skip!();
        let mut chain = open_chain(&data).expect("open chain");
        let out =
            std::env::temp_dir().join(format!("benilla-schema-reg-{}.csv", std::process::id()));
        for table in [
            "CreatureDisplayInfo",
            "CreatureDisplayInfoExtra",
            "CreatureModelData",
            "CharHairGeosets",
            "CharacterFacialHairStyles",
            "HelmetGeosetVisData",
            "CharSections",
            "ItemDisplayInfo",
            "ItemVisuals",
            "ItemVisualEffects",
            "SpellItemEnchantment",
            "ItemRandomProperties",
            "TaxiNodes",
            "AreaTable",
            "GameObjectDisplayInfo",
            "TaxiPath",
            "TaxiPathNode",
            "TransportAnimation",
        ] {
            let path = format!("DBFilesClient\\{table}.dbc");
            let bytes = chain
                .read_file(&path)
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            let (rows, _) = dbc_to_csv(&bytes, &path, &out)
                .unwrap_or_else(|e| panic!("{table} schema does not fit the shipped file: {e}"));
            assert!(rows > 0, "{table} has rows");
        }
        let _ = std::fs::remove_file(&out);
    }
}
