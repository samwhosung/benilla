//! `benilla-extract`: a CLI over the 1.12.1 (build 5875) MPQ patch chain, where later archives
//! override earlier ones and names come from `patch.MPQ`'s `(listfile)`. This file holds the clap
//! commands, their dispatch and the shared helpers; the subcommands live in modules by concern.

use std::path::PathBuf;

use anyhow::{Context, Result};
use benilla_formats::open_chain;
use clap::{Parser, Subcommand};

mod chaincensus;
mod charatlas;
mod charprocs;
mod glueextent;
mod kitanim;
mod m2dump;
mod scan;
mod shakecensus;
mod spellvis;
mod thudcensus;

/// Read WoW 1.12.1 asset archives (MPQ).
#[derive(Parser)]
#[command(
    name = "benilla-extract",
    version,
    about,
    allow_negative_numbers = true
)]
struct Cli {
    /// A single `.MPQ` file, or a vanilla `Data` directory (opens the full patch chain).
    path: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List files across the chain, sorted by name.
    List,
    /// Extract one file by its internal path, writing raw bytes to `output`.
    Extract {
        /// Internal path inside the archive (forward or back slashes accepted).
        internal_path: String,
        /// Output file to write the extracted bytes to.
        output: PathBuf,
    },
    /// Decode a BLP texture from the archive and write it as a PNG.
    Blp {
        /// Internal path to the `.blp` (forward or back slashes accepted).
        internal_path: String,
        /// Output `.png` file.
        output: PathBuf,
        /// Also write every mip (`<stem>.mip<N>.png`) and census each: transparent and opaque
        /// texels, their luma, and how many sit below 128, which darken under alpha-blind Mod2x.
        #[arg(long)]
        mips: bool,
    },
    /// Composite one character's body atlas and report what painted what.
    Charatlas {
        /// `ChrRaces`: 1 human, 2 orc, 3 dwarf, 4 night elf, 5 undead, 6 tauren, 7 gnome, 8 troll.
        #[arg(long)]
        race: u8,
        /// 0 male · 1 female.
        #[arg(long)]
        sex: u8,
        /// `skinColor`, the CharSections variation the base skin and head sections key on.
        #[arg(long, default_value_t = 0)]
        skin: u8,
        /// `faceType`.
        #[arg(long, default_value_t = 0)]
        face: u8,
        /// `facialHairStyle`.
        #[arg(long, default_value_t = 0)]
        facial_hair: u8,
        /// `hairStyle`.
        #[arg(long, default_value_t = 0)]
        hair_style: u8,
        /// `hairColor`.
        #[arg(long, default_value_t = 0)]
        hair_color: u8,
        /// The eight worn `ItemDisplayInfo` ids, comma-separated in bodyslot order (shirt, chest,
        /// belt, pants, boots, wrist, gloves, tabard); a 0 or a short list leaves slots empty.
        #[arg(long, value_delimiter = ',', default_value = "0")]
        slots: Vec<u32>,
        /// The guild tabard, `emblemStyle,emblemColor,borderStyle,borderColor,backgroundColor` as
        /// `SMSG_GUILD_QUERY_RESPONSE` carries them; only a guild-emblem tabard (20621) shows it.
        #[arg(long, value_delimiter = ',')]
        emblem: Option<Vec<i32>>,
        /// Write the composited atlas here as a PNG at the base skin's authored resolution.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Measure each shipped glue scene's art extent and the window aspects where it runs out.
    Glueextent {
        /// Also print every batch's footprint: front, back and clipped triangles and its extents.
        #[arg(long)]
        batches: bool,
    },
    /// Dump a DBC table to CSV using our schema for it (headers included).
    Dbc {
        /// Internal path to the `.dbc` (forward or back slashes accepted).
        internal_path: String,
        /// Output `.csv` file.
        output: PathBuf,
    },
    /// Dump a spell's visual chain: its `SpellVisual` stages and each kit's contents.
    Spellvis {
        /// The `Spell.dbc` id (e.g. 133 = Fireball).
        spell_id: u32,
    },
    /// Census the beam system: `SpellChainEffects` and every `SpellVisualKit` that draws a beam.
    Chaincensus,
    /// Census the camera-shake system: the 24 `CameraShakes.dbc` presets and all that names one.
    Shakecensus,
    /// Census the death thud a corpse makes on landing (`$DTH`, `0x6236e0`): the
    /// `DeathThudLookups.dbc` grid and which creature models key a `$DTH`.
    Thudcensus,
    /// Census the `SpellVisualKit` CharProc columns, a kit's effect on the body, by stage.
    Charprocs,
    /// Census the `SpellVisualKit` animation column (field 2), a clip on the unit's body, by stage.
    Kitanim,
    /// Dump an M2's collision hull: counts and the model-space AABB (WoW axes, Z up), unscaled.
    M2coll {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's sequences in file order: anim id, loop or clamp, duration, design speed (the
    /// rate divisor), variation frequency and replay range; sequences sharing an id are variations.
    M2seq {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's camera table by raw file index, the index `Model:SetCamera(n)` selects by;
    /// the portrait bake selects through `cameraLookup` instead, printed beside it.
    M2cam {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's animation event keyframes per sequence; whether `$CPP` precedes the impact
    /// tag decides defense animation or flinch.
    M2events {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's attachment points (id and bone); a placement on an absent id hangs nothing.
    M2attach {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's animation channels, texture transforms and every particle emitter in full.
    M2anim {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's bone table, with the parent channels each bone ignores (`flags & 0x7`).
    M2bones {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's render batches as the renderer sees them.
    M2batch {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump an M2's per-sequence material alpha: each colour-alpha and transparency track, then
    /// every batch's combined factor per sequence band. `HIDE` marks a batch the reference skips
    /// in that sequence: `A <= 0` culls before the blend mode is read (`0x707b3a`-`0x707b5c`).
    M2alpha {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Dump one M2's particle emitters in full, with each one's derived count, reach and size.
    M2part {
        /// Internal path to the `.m2` (forward or back slashes accepted).
        internal_path: String,
    },
    /// Sweep every `.m2` for where addressing an attachment by id through AttachLookup
    /// (`0x710310`) disagrees with scanning its records; item glows hang on the item's ids 0..4.
    Attachscan {
        /// Optional path prefix to restrict the sweep (e.g. `Item\ObjectComponents\Weapon`).
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and list the models carrying ribbon emitters (header `0x134`).
    Ribbonscan,
    /// Sweep every `.m2` and census particle flipbook fields: backward ramps (`begin > end`, legal
    /// and shipped), tail ramps unlike the head's, indices past the atlas (the reference wraps the
    /// column, the row runs off), repeat counts, and the reference's two degenerate fallbacks.
    Cellscan,
    /// Sweep every `.m2` for materials with the multiply blends 5 (Mod) and 6 (Mod2x), the
    /// armor-reflect sheen family.
    Blendscan {
        /// Internal-path prefix filter (e.g. `item\objectcomponents\weapon`), case-insensitive;
        /// all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and classify its billboards: which arms it authors (spherical, lock-X,
    /// -Y or -Z) and whether geometry is skinned to the billboard bone or rides a descendant.
    Bbscan {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and classify each billboard batch by facing: a billboard bone turns +X to
    /// the viewer, so a single-sided batch facing -X is culled by the reference from every angle.
    Bbfacescan {
        /// Internal-path prefix filter (e.g. `world\generic`), case-insensitive; all if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for flat ground-plane batches (every vertex at model-space z≈0), which
    /// sloped terrain buries; `QUAD-1BONE` is Battle Shout's crescent shape, a quad on one bone.
    Groundscan {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for batches with zero authored vertex normals: the reference takes one as
    /// is and its order-2 SH collapses to the flat DC term, so the surface draws lit, where
    /// `normalize()` gives NaN and a black batch. `ALL` marks a batch degenerate at every vertex.
    Normalscan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and measure how far its header bounds, the all-animation extent the
    /// reference derives its doodad cull sphere from (`rec+0x5c`/`rec+0x68`), reach past its
    /// bind-pose extent; `SHORT` marks bind-pose geometry outside the header box.
    Animboundscan {
        /// Internal-path prefix filter (e.g. `world\critter`), case-insensitive; all if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for geometry a non-character spawn draws that the reference may not:
    /// multi-geoset models (only the character compositor picks among geosets), untextured batches
    /// and tiny ones (at most 2 faces).
    Geosetscan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `GameObjectDisplayInfo` model and resolve what the reference's GameObject
    /// animation arm plays per reachable state and anim-progress substate (substate table
    /// `0x5f3c30`, LUT `0x8607e4`, missing-sequence remap `0x5f3972`) against the loader's seed
    /// (`0x710153`, animation 0 through `playableAnimationLookup`); a model whose every substate
    /// lands on the seed's sequence is state-blind.
    Goanimscan,
    /// Sweep every `.m2` and census each `M2Event`'s `bone` and `position`: the reference's
    /// dispatchers play at the event's own world point, so per 4CC it counts the records off the
    /// origin and those on a bone some sequence keys.
    Eventmarkerscan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Census `GameObjectDisplayInfo.Sound[0..9]`: only `0x5f4010` reads them, called only from
    /// the GameObject anim-event dispatcher (`0x5f3e20`), `$GO0..5` to slots 0-5 and `$GC0..3` to
    /// 6-9. Per slot: filled, tagged by the display's model, on an armable sequence, and naming a
    /// looping kit (0x200, which picks `0x5f4010`'s emitter-pool lane over its one-shot lane).
    Goslotscan,
    /// Sweep every `.m2` for batches whose visibility changes per sequence, drawn in one
    /// animation and culled by `A <= 0` in another.
    Alphascan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for the effect lifecycle `Stand`(0), `Hold`(158), `Decay`(159): arming
    /// only the clamping Stand freezes the effect on its last frame instead of pulsing in Hold.
    Fxlifescan {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and measure our skeletal parse against the reference's sampler, per bone
    /// track and band: empty bands (our nearest-key clamp, its `interpolation_ranges` window), held
    /// band edges (our hold, its lerp toward the next key) and step tracks (`interp == 0`, which it
    /// copies and we interpolate). It also checks no band's keys fall outside the searched window.
    Bonescan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and census each particle-emitter feature the corpus authors.
    Partcensus {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for emitters whose file slot 0 is dead while a later slot lives
    /// (`peak0 <= 0 < peakN`): the reference samples the playing sequence's rate window, so a
    /// consumer pinned to slot 0 never emits them.
    Partslotscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for batches the bake routes to a per-placement material, whose UV or
    /// M2Color tint loop differs across file sequence slots, read from the bake itself (`uv_seq`
    /// and `rgb_seq` are `Some` exactly when `SeqLoops::uniform()` refuses the shared lane) and
    /// bucketed by why: `DEAD-0`, `WRAP-ONLY`, `KEYS-EPSILON` or `REAL-DIFFERS`. Only placed
    /// doodads and WMO props reach the lane; creatures, spells and GameObjects use the entity lane.
    Uvslotscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for models animated entirely outside the bone tracks: no sequence keys a
    /// bone, so a clock riding a bone clip leaves every per-sequence consumer at slot 0, t = 0.
    /// `[GO]` marks a `GameObjectDisplayInfo` model, where the freeze is total.
    Seqclockscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for batches whose texture is authored clamp on an axis (`M2Texture.flags`
    /// bit 0 or 1 clear) while their UVs run outside 0..1 there on purpose: clamped they fade into
    /// the sheet's transparent border, wrapped they draw solid with a seam where they cross.
    Uvwrapscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every model the spell-visual chain reaches (kit effect slots, missile and
    /// dest-anchored models) for batches whose texture transform animates, classed by what a
    /// consumer running none of it draws, from the texture's alpha through its address mode:
    /// `INVISIBLE` (frozen, it samples only transparent texels while the scroll reaches painted
    /// ones), `FROZEN`, `HELD`, `NEVER` or `UNKNOWN`. It closes with the `INVISIBLE` batches joined
    /// to the spells that reach them.
    Fxuvscan {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all reachable effect
        /// models if omitted.
        prefix: Option<String>,
    },
    /// `fxuvscan`'s census over the entity lane's models, as the tables reach them: every
    /// `CreatureDisplayInfo` body (players included), `GameObjectDisplayInfo` model,
    /// `ItemDisplayInfo` model in whichever `Item\ObjectComponents\` folder holds it (a helm as its
    /// 16 race and sex files) and `<Race><Sex>DeathSkeleton`. A creature's `Monster1/2/3` sheet is
    /// filled per display from `textureVariation`, so each skin is judged. The clock column is
    /// `GSEQ` (a global sequence, which the reference anchors once per instance at attach), `BAND`
    /// (the instance's own play head), `MIXED` or `HOLD`. It then repeats for the M2Color tint
    /// (`BLACK`, `STRONG`, `SLIGHT`, `NEGLIGIBLE`), with the alpha channel counted beside it.
    Entityuvscan {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all reachable entity
        /// models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for batches whose texture coordinates are generated, the sphere-map
    /// stages (`texture_unit_lookup[texCoordSet] > 2`, the reference's gate at `0x70b8bd`). Their
    /// UVs go unused by design; `DEGENERATE` marks those collapsed to one point, which a renderer
    /// reading vertex UVs paints as one texel.
    Envmapscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and report the sampler address modes the corpus asks of each texture
    /// path, and how many paths are asked for more than one.
    Texmodescan {
        /// Internal-path prefix filter, case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and count both halves of the owner-last draw order: each model's effects
    /// (particle emitters and ribbons) and its own transparent-pass batches, with reach and rung.
    Fxordercensus {
        /// Internal-path prefix filter (e.g. `creature`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for 3-D model particles, emitters whose shards draw a geometry model's
    /// submeshes at the owner's draw-order rung: per (owner, geometry) pair, the rung, reach and
    /// material family tuples the shard materials are built from.
    Shardcensus {
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for models whose loader-idle sequence is not file slot 0 while benilla's
    /// render content gate leaves them unarmed, so every per-sequence bake reads slot 0, a sequence
    /// the instance is not playing; on `DuelingFlag`-shaped GameObjects that is the Spawn flourish.
    Idleslotscan {
        /// Internal-path prefix filter (e.g. `world`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for animation sound markers (`$DSL` doodad loop, `$DSE` its release,
    /// `$DSO` doodad one-shot, `$SND` one-shot) with each kit's 3-D parameters, and whether the
    /// carrying sequence is rest-posed, one the render content gate builds no rig for.
    Soundeventscan {
        /// Internal-path prefix to limit the sweep (e.g. `world`); all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` and list the models whose particle emitters carry any of the given
    /// file-flag bits (`M2ParticleEmitter+0x04`).
    Partscan {
        /// The flag mask to match, hex or decimal (e.g. `0x1000`).
        mask: String,
        /// Internal-path prefix filter (e.g. `spells`), case-insensitive; all models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every `.m2` for M2 dynamic lights (`0x718960`): per model its point (`type==1`) and
    /// directional light counts, each point light's bone, position, colour, attenuation and
    /// visibility gate, then totals by content family and a diffuse-palette tally.
    M2lightscan {
        /// Internal-path prefix filter (e.g. `item\objectcomponents`), case-insensitive; all
        /// models if omitted.
        prefix: Option<String>,
    },
    /// Sweep every WMO root and cross-tab its MOSB skybox model against its groups' `0x40000`
    /// flag, which never appears without a MOSB in all 815 roots; the reference tests it on the
    /// flood-visited group (`0x6b42e0`). Also which buildings replace the `Light.dbc` dome with an
    /// authored sky, and in how many groups: only Stratholme's is reachable in 1.12.
    Skyboxscan,
    /// Dump all 18 `LightIntBand` rows of the `Light.dbc` entry covering a position at a time of
    /// day. One params record, raw: near a sphere's falloff edge the live light is mostly the
    /// continent global, and `lightblend` shows what wins there.
    Lightbands {
        /// Map id (0 = Eastern Kingdoms, 1 = Kalimdor).
        map: u32,
        /// World-space X (raw WoW yards; may be negative).
        #[arg(allow_hyphen_values = true)]
        x: f32,
        /// World-space Y.
        #[arg(allow_hyphen_values = true)]
        y: f32,
        /// World-space Z.
        #[arg(allow_hyphen_values = true)]
        z: f32,
        /// Game minute-of-day (0..1440; 720 = noon).
        minute: u32,
    },
    /// Dump every `LightIntBand` and `LightFloatBand` row of a named `LightParams` id, for rows no
    /// position reaches: magma submersion reads the fixed global row 7 and slime row 6
    /// (`0x6d2371`), not the zone's underwater slot.
    Lightparam {
        /// `LightParams.dbc` id (1-based).
        id: u32,
        /// Game minute-of-day (0..1440; 720 = noon).
        minute: u32,
    },
    /// Dump the per-weather-slot light resolve at a position: the `LightParams` id each of the five
    /// `Light.dbc` slots names (clear, clear underwater, storm, storm underwater, death), the unset
    /// ones that fall back to clear, and each one's fog, ambient and diffuse.
    Lightslots {
        /// Map id (0 = Eastern Kingdoms, 1 = Kalimdor).
        map: u32,
        /// World-space X (raw WoW yards; may be negative).
        #[arg(allow_hyphen_values = true)]
        x: f32,
        /// World-space Y.
        #[arg(allow_hyphen_values = true)]
        y: f32,
        /// World-space Z.
        #[arg(allow_hyphen_values = true)]
        z: f32,
        /// Game minute-of-day (0..1440; 720 = noon).
        minute: u32,
    },
    /// Dump every light sphere covering a position (distance, falloff, blend alpha, clear ambient
    /// and sun), then the area-blend result against the single `pick_light` sample.
    Lightblend {
        /// Map id (0 = Eastern Kingdoms, 1 = Kalimdor).
        map: u32,
        /// World-space X (raw WoW yards; may be negative).
        #[arg(allow_hyphen_values = true)]
        x: f32,
        /// World-space Y.
        #[arg(allow_hyphen_values = true)]
        y: f32,
        /// World-space Z.
        #[arg(allow_hyphen_values = true)]
        z: f32,
        /// Game minute-of-day (0..1440; 720 = noon).
        minute: u32,
    },
    /// Scan placed doodads (MDDF) and WMO doodad-set-0 props (MODF, MODS, MODD) over a
    /// `(2·tile_radius+1)²` block of ADT tiles around a position, and report how much animates.
    Doodadscan {
        /// Map directory name (e.g. `Azeroth`).
        map: String,
        /// World-space center X (may be negative).
        #[arg(allow_hyphen_values = true)]
        center_x: f32,
        /// World-space center Y (may be negative).
        #[arg(allow_hyphen_values = true)]
        center_y: f32,
        /// ADT tile radius around the center tile to scan (0 = just the containing tile).
        tile_radius: u32,
    },
    /// Dump a WMO root's placed props: each MODD doodad with its MODS sets and the groups whose
    /// MODR names it. The reference creates props only from a group's MODR refs (`0x695aa0`), so
    /// it never creates one no group names; benilla still spawns those.
    Wmodoodads {
        /// Internal path to the WMO root (forward or back slashes accepted).
        internal_path: String,
        /// Case-insensitive substring of the prop's model path (e.g. `lightray`); all if omitted.
        filter: Option<String>,
    },
    /// Sweep every WMO root for placed MODD props the interior lane lights literal black: its base
    /// light is the MODD colour (ambient `cap96`, diffuse `floor112`, `0x694e90` → `0x6a77e0`), and
    /// the `112/max` floor keeps `#000000` black. An exterior referrer sky-lights a prop instead
    /// (`0x695aa0`); `RESCUED` counts those an interior group names first.
    Darkpropscan {
        /// Internal-path prefix to limit the sweep (e.g. `world\wmo\azeroth`); all if omitted.
        prefix: Option<String>,
    },
    /// List the doodad (MDDF) and WMO (MODF) placements around a position whose model path
    /// contains a substring: position, Euler rotation (deg), scale and uniqueId.
    Placescan {
        /// Map directory name (e.g. `Azeroth`).
        map: String,
        /// World-space center X (may be negative).
        #[arg(allow_hyphen_values = true)]
        center_x: f32,
        /// World-space center Y (may be negative).
        #[arg(allow_hyphen_values = true)]
        center_y: f32,
        /// ADT tile radius around the center tile to scan (0 = just the containing tile).
        tile_radius: u32,
        /// Case-insensitive substring of the model path (e.g. `instanceportal`).
        filter: String,
    },
    /// Print the terrain MCSH baked-shadow bit at a position, which an exterior M2 doodad samples
    /// once at its base (sun scale 1.0 lit, 0.5 shadowed, `0x698cb4`), and the texel
    /// neighbourhood: one ~0.52 yd texel can split adjacent pieces across a shadow edge.
    Shadeat {
        /// Map directory name (e.g. `Azeroth`).
        map: String,
        /// World-space X (raw WoW yards; may be negative).
        #[arg(allow_hyphen_values = true)]
        x: f32,
        /// World-space Y.
        #[arg(allow_hyphen_values = true)]
        y: f32,
    },
}

/// MPQ internal paths are backslash-separated; accept `/` for convenience.
fn normalize(path: &str) -> String {
    path.replace('/', "\\")
}

/// A placed model path's grouping key, lowercase with `.mdx`/`.mdl` as `.m2`; it must match
/// `benilla_formats`' crate-private `model_path` normalization.
fn model_key(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    match lower
        .strip_suffix(".mdx")
        .or_else(|| lower.strip_suffix(".mdl"))
    {
        Some(stem) => format!("{stem}.m2"),
        None => lower,
    }
}

/// Parse a `0x`-prefixed hex or plain-decimal u32 (`partscan`'s mask argument).
fn parse_u32_maybe_hex(s: &str) -> Result<u32> {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).context("invalid hex"),
        None => s.parse().context("invalid decimal"),
    }
}

fn yn(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "-"
    }
}

/// `--emblem` → a [`benilla_formats::GuildEmblem`]: the five wire indices in
/// `SMSG_GUILD_QUERY_RESPONSE` order.
fn guild_emblem(v: Vec<i32>) -> Result<benilla_formats::GuildEmblem> {
    let [emblem_style, emblem_color, border_style, border_color, background_color]: [i32; 5] =
        v.try_into().map_err(|v: Vec<i32>| {
            anyhow::anyhow!(
                "--emblem takes 5 comma-separated indices \
                 (emblemStyle,emblemColor,borderStyle,borderColor,backgroundColor), got {}",
                v.len()
            )
        })?;
    Ok(benilla_formats::GuildEmblem {
        emblem_style,
        emblem_color,
        border_style,
        border_color,
        background_color,
    })
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut chain = open_chain(&cli.path)?;

    match cli.command {
        Command::List => {
            let mut files = chain.list().context("listing chain contents")?;
            files.sort_by(|a, b| a.name.cmp(&b.name));
            for entry in &files {
                println!("{:>12}  {}", entry.size, entry.name);
            }
            eprintln!("{} files", files.len());
        }
        Command::Charatlas {
            race,
            sex,
            skin,
            face,
            facial_hair,
            hair_style,
            hair_color,
            slots,
            emblem,
            out,
        } => {
            let mut worn = [0u32; 8];
            for (dst, src) in worn.iter_mut().zip(&slots) {
                *dst = *src;
            }
            charatlas::charatlas(
                &mut chain,
                &charatlas::Look {
                    race,
                    sex,
                    skin,
                    face,
                    facial_hair,
                    hair_style,
                    hair_color,
                    slots: worn,
                    emblem: emblem.map(guild_emblem).transpose()?,
                },
                out.as_deref(),
            )?;
        }
        Command::Extract {
            internal_path,
            output,
        } => {
            let name = normalize(&internal_path);
            // The archive the file resolved from, which shows an override.
            let source = chain
                .find_file_archive(&name)
                .map(|p| p.display().to_string());
            let data = chain
                .read_file(&name)
                .with_context(|| format!("reading '{name}' from chain"))?;
            std::fs::write(&output, &data)
                .with_context(|| format!("writing {}", output.display()))?;
            match source {
                Some(src) => eprintln!(
                    "wrote {} bytes to {} (from {})",
                    data.len(),
                    output.display(),
                    src
                ),
                None => eprintln!("wrote {} bytes to {}", data.len(), output.display()),
            }
        }
        Command::Blp {
            internal_path,
            output,
            mips,
        } => {
            let name = normalize(&internal_path);
            let data = chain
                .read_file(&name)
                .with_context(|| format!("reading '{name}' from chain"))?;
            if mips {
                let stats = benilla_formats::blp_mips_to_png(&data, &output)
                    .with_context(|| format!("decoding BLP '{name}'"))?;
                let luma = |l: Option<(u8, f32, u8)>| match l {
                    Some((lo, mean, hi)) => format!("{lo:>3}/{mean:>6.1}/{hi:>3}"),
                    None => "      —       ".to_string(),
                };
                println!(
                    "level  size      outside(a=0)  luma lo/mean/hi   inside(a>0)  luma lo/mean/hi   below128"
                );
                for s in &stats {
                    println!(
                        "{:>5}  {:>4}x{:<4}  {:>12}  {:>14}  {:>11}  {:>14}  {:>8}",
                        s.level,
                        s.width,
                        s.height,
                        s.outside,
                        luma(s.outside_luma),
                        s.inside,
                        luma(s.inside_luma),
                        s.below_128,
                    );
                }
                eprintln!("wrote {} level(s) beside {}", stats.len(), output.display());
            } else {
                let (w, h) = benilla_formats::blp_to_png(&data, &output)
                    .with_context(|| format!("decoding BLP '{name}'"))?;
                eprintln!("decoded {w}x{h} -> {}", output.display());
            }
        }
        Command::Dbc {
            internal_path,
            output,
        } => {
            let name = normalize(&internal_path);
            let data = chain
                .read_file(&name)
                .with_context(|| format!("reading '{name}' from chain"))?;
            let (records, fields) = benilla_formats::dbc_to_csv(&data, &name, &output)?;
            eprintln!(
                "wrote {records} records ({fields} fields) -> {}",
                output.display()
            );
        }
        Command::Glueextent { batches } => glueextent::glueextent(&mut chain, batches)?,
        Command::M2coll { internal_path } => m2dump::m2coll(&mut chain, &internal_path)?,
        Command::M2cam { internal_path } => m2dump::m2cam(&mut chain, &internal_path)?,
        Command::M2seq { internal_path } => m2dump::m2seq(&mut chain, &internal_path)?,
        Command::M2events { internal_path } => m2dump::m2events(&mut chain, &internal_path)?,
        Command::M2attach { internal_path } => m2dump::m2attach(&mut chain, &internal_path)?,
        Command::M2anim { internal_path } => m2dump::m2anim(&mut chain, &internal_path)?,
        Command::M2bones { internal_path } => m2dump::m2bones(&mut chain, &internal_path)?,
        Command::M2batch { internal_path } => m2dump::m2batch(&mut chain, &internal_path)?,
        Command::M2alpha { internal_path } => m2dump::m2alpha(&mut chain, &internal_path)?,
        Command::M2part { internal_path } => m2dump::m2part(&mut chain, &internal_path)?,
        Command::Attachscan { prefix } => scan::attachscan(&mut chain, prefix.as_deref())?,
        Command::Ribbonscan => scan::ribbonscan(&mut chain)?,
        Command::Cellscan => scan::cellscan(&mut chain)?,
        Command::Blendscan { prefix } => scan::blendscan(&mut chain, prefix.as_deref())?,
        Command::Bbscan { prefix } => scan::bbscan(&mut chain, prefix.as_deref())?,
        Command::Bbfacescan { prefix } => scan::bbfacescan(&mut chain, prefix.as_deref())?,
        Command::Groundscan { prefix } => scan::groundscan(&mut chain, prefix.as_deref())?,
        Command::Normalscan { prefix } => scan::normalscan(&mut chain, prefix.as_deref())?,
        Command::Animboundscan { prefix } => scan::animboundscan(&mut chain, prefix.as_deref())?,
        Command::Geosetscan { prefix } => scan::geosetscan(&mut chain, prefix.as_deref())?,
        Command::Alphascan { prefix } => scan::alphascan(&mut chain, prefix.as_deref())?,
        Command::Fxlifescan { prefix } => scan::fxlifescan(&mut chain, prefix.as_deref())?,
        Command::Goanimscan => scan::goanimscan(&mut chain)?,
        Command::Eventmarkerscan { prefix } => {
            scan::eventmarkerscan(&mut chain, prefix.as_deref())?
        }
        Command::Goslotscan => scan::goslotscan(&mut chain)?,
        Command::Bonescan { prefix } => scan::bonescan(&mut chain, prefix.as_deref())?,
        Command::Partcensus { prefix } => scan::partcensus(&mut chain, prefix.as_deref())?,
        Command::Partslotscan { prefix } => scan::partslotscan(&mut chain, prefix.as_deref())?,
        Command::Uvslotscan { prefix } => scan::uvslotscan(&mut chain, prefix.as_deref())?,
        Command::Seqclockscan { prefix } => scan::seqclockscan(&mut chain, prefix.as_deref())?,
        Command::Uvwrapscan { prefix } => scan::uvwrapscan(&mut chain, prefix.as_deref())?,
        Command::Fxuvscan { prefix } => scan::fxuvscan(&mut chain, prefix.as_deref())?,
        Command::Entityuvscan { prefix } => scan::entityuvscan(&mut chain, prefix.as_deref())?,
        Command::Envmapscan { prefix } => scan::envmapscan(&mut chain, prefix.as_deref())?,
        Command::Texmodescan { prefix } => scan::texmodescan(&mut chain, prefix.as_deref())?,
        Command::Fxordercensus { prefix } => scan::fxordercensus(&mut chain, prefix.as_deref())?,
        Command::Shardcensus { prefix } => scan::shardcensus(&mut chain, prefix.as_deref())?,
        Command::Idleslotscan { prefix } => scan::idleslotscan(&mut chain, prefix.as_deref())?,
        Command::Soundeventscan { prefix } => scan::soundeventscan(&mut chain, prefix.as_deref())?,
        Command::Partscan { mask, prefix } => {
            let mask = parse_u32_maybe_hex(&mask)
                .with_context(|| format!("parsing flag mask '{mask}'"))?;
            scan::partscan(&mut chain, mask, prefix.as_deref())?;
        }
        Command::M2lightscan { prefix } => scan::m2lightscan(&mut chain, prefix.as_deref())?,
        Command::Skyboxscan => scan::skyboxscan(&mut chain)?,
        Command::Lightbands {
            map,
            x,
            y,
            z,
            minute,
        } => {
            let catalog = benilla_formats::LightCatalog::load(&mut chain)?;
            // `debug_bands` takes the client's half-minute clock (0..2880).
            catalog.debug_bands(map, [x, y, z], minute * 2);
        }
        Command::Lightparam { id, minute } => {
            let catalog = benilla_formats::LightCatalog::load(&mut chain)?;
            catalog.debug_param(id, minute * 2);
        }
        Command::Lightslots {
            map,
            x,
            y,
            z,
            minute,
        } => {
            let catalog = benilla_formats::LightCatalog::load(&mut chain)?;
            catalog.debug_slots(map, [x, y, z], minute * 2);
        }
        Command::Lightblend {
            map,
            x,
            y,
            z,
            minute,
        } => {
            let catalog = benilla_formats::LightCatalog::load(&mut chain)?;
            catalog.debug_blend(map, [x, y, z], minute * 2);
        }
        Command::Wmodoodads {
            internal_path,
            filter,
        } => scan::wmodoodads(&mut chain, &internal_path, filter.as_deref())?,
        Command::Darkpropscan { prefix } => scan::darkpropscan(&mut chain, prefix.as_deref())?,
        Command::Placescan {
            map,
            center_x,
            center_y,
            tile_radius,
            filter,
        } => scan::placescan(&mut chain, &map, center_x, center_y, tile_radius, &filter)?,
        Command::Doodadscan {
            map,
            center_x,
            center_y,
            tile_radius,
        } => scan::doodadscan(&mut chain, &map, center_x, center_y, tile_radius)?,
        Command::Shadeat { map, x, y } => scan::shadeat(&mut chain, &map, x, y)?,
        Command::Spellvis { spell_id } => spellvis::run(&mut chain, spell_id)?,
        Command::Chaincensus => chaincensus::run(&mut chain)?,
        Command::Shakecensus => shakecensus::shakecensus(&mut chain)?,
        Command::Thudcensus => thudcensus::thudcensus(&mut chain)?,
        Command::Charprocs => charprocs::run(&mut chain)?,
        Command::Kitanim => kitanim::run(&mut chain)?,
    }

    Ok(())
}
